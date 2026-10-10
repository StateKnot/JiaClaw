// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 技能发现和管理

use jiaclaw_core::JiaClawError;
use sha2::Digest;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

mod lock;
pub use lock::{SkillLockPolicy, SkillPolicyEntry, SkillSourcePin};

const SKILL_FILE_BYTES: usize = 128 * 1024;
const CATALOG_BYTES: usize = 2 * 1024 * 1024;
const CATALOG_SKILLS: usize = 64;
const CATALOG_ENTRIES: usize = 256;
const SKILL_RESOURCES: usize = 8;

fn invalid(reason: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::Configuration(format!("存在无效技能文件: {reason}"))
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Administrator-declared UTF-8 reference, pinned to exact file bytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillResource {
    pub path: String,
    pub sha256: String,
}

impl SkillResource {
    fn valid(&self) -> bool {
        let parts: Vec<_> = self.path.split('/').collect();
        self.path.len() <= 256
            && (2..=16).contains(&parts.len())
            && parts[0] == "references"
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.trim() == *part
                    && !part.chars().any(char::is_control)
                    && !matches!(*part, "." | "..")
                    && !part.contains(['\\', ':'])
            })
            && valid_hash(&self.sha256)
    }
}

/// 技能定义
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Skill {
    /// 技能名称
    pub name: String,

    /// 技能描述
    pub description: String,

    /// 技能路径
    #[serde(skip)]
    pub path: PathBuf,

    /// 完整的 SKILL.md 内容（不含 frontmatter）
    #[serde(skip)]
    pub content: String,

    /// 触发关键词列表（可选）
    #[serde(default)]
    pub triggers: Vec<String>,

    /// Exact references permitted by the loaded skill declaration; never auto-read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<SkillResource>,

    /// Loaded administrator origin declaration, never publisher authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SkillSourcePin>,
}

/// YAML frontmatter 结构
#[derive(Debug, Clone, serde::Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    jiaclaw_resources: Vec<SkillResource>,
}

impl Skill {
    /// 从 SKILL.md 文件解析技能
    ///
    /// # Errors
    ///
    /// 如果技能文件不存在或无法读取，返回错误。
    pub fn from_file(skill_dir: &Path) -> Result<Self, JiaClawError> {
        let dir_name = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("技能目录需要 UTF-8 名称"))?;
        let parent = skill_dir
            .parent()
            .ok_or_else(|| invalid("技能目录缺少父目录"))?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        Self::read(parent, &format!("./{dir_name}/SKILL.md"), skill_dir)?
            .map(|(skill, _, _)| skill)
            .ok_or_else(|| invalid("技能文件不存在"))
    }

    // The workspace is the administrator-selected authority. Discovery always reads
    // from it, never from an ambient skills/ or per-skill path checked earlier.
    fn read(
        workspace: &Path,
        relative: &str,
        skill_dir: &Path,
    ) -> Result<Option<(Self, usize, String)>, JiaClawError> {
        let dir_name = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("技能目录需要 UTF-8 名称"))?;
        let Some(file) = crate::memory_io::read_text(workspace, relative, SKILL_FILE_BYTES, false)
            .map_err(invalid)?
        else {
            return Ok(None);
        };
        if !identifier(dir_name) {
            return Err(invalid(
                "技能目录名需要 1..=128 UTF-8 字节且无首尾空白/控制字符",
            ));
        }
        let raw_content = file.text;
        let raw_bytes = raw_content.len();

        let (frontmatter, content) = Self::parse_frontmatter(&raw_content).map_err(invalid)?;

        let name = frontmatter.name.unwrap_or_else(|| dir_name.to_string());
        let description = frontmatter
            .description
            .unwrap_or_else(|| Self::extract_description(&content));
        let triggers = frontmatter.triggers;
        let resources = frontmatter.jiaclaw_resources;
        if !identifier(&name)
            || description.len() > 2048
            || triggers.len() > 32
            || triggers.iter().any(|trigger| !identifier(trigger))
        {
            return Err(invalid("name/trigger 最多 128 字节且非空、无首尾空白/控制字符；description 最多 2048 字节；triggers 最多 32 项"));
        }
        let mut resource_paths = HashSet::new();
        if resources.len() > SKILL_RESOURCES
            || resources
                .iter()
                .any(|resource| !resource.valid() || !resource_paths.insert(resource.path.clone()))
        {
            return Err(invalid("jiaclaw_resources 最多 8 个唯一 references/ 相对路径及小写 SHA-256；路径最多 256 字节/16 组件"));
        }
        Ok(Some((
            Self {
                name,
                description,
                path: skill_dir.to_path_buf(),
                content,
                triggers,
                resources,
                source: None,
            },
            raw_bytes,
            format!("{:x}", sha2::Sha256::digest(raw_content.as_bytes())),
        )))
    }

    /// 解析 YAML frontmatter
    ///
    /// 返回 `(frontmatter, content_without_frontmatter)`
    fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, String), JiaClawError> {
        let trimmed = content.trim_start();

        if !trimmed.starts_with("---") {
            return Ok((
                SkillFrontmatter {
                    name: None,
                    description: None,
                    triggers: Vec::new(),
                    jiaclaw_resources: Vec::new(),
                },
                content.to_string(),
            ));
        }

        let after_first_delimiter = &trimmed[3..];

        if let Some(end_pos) = after_first_delimiter.find("\n---") {
            let yaml_content = &after_first_delimiter[..end_pos];
            let remaining_content = &after_first_delimiter[end_pos + 4..];

            let frontmatter: SkillFrontmatter =
                serde_yaml::from_str(yaml_content).map_err(|e| {
                    JiaClawError::Configuration(format!("无法解析 YAML frontmatter: {e}"))
                })?;

            Ok((frontmatter, remaining_content.trim().to_string()))
        } else {
            Ok((
                SkillFrontmatter {
                    name: None,
                    description: None,
                    triggers: Vec::new(),
                    jiaclaw_resources: Vec::new(),
                },
                content.to_string(),
            ))
        }
    }

    /// 从 SKILL.md 内容中提取描述
    fn extract_description(content: &str) -> String {
        // 查找 ## Description 部分
        let lines: Vec<&str> = content.lines().collect();
        let mut in_description = false;
        let mut description_lines = Vec::new();

        for line in lines {
            let trimmed = line.trim();

            if trimmed.starts_with("## Description") {
                in_description = true;
                continue;
            }

            if in_description {
                if trimmed.starts_with("##") {
                    // 遇到下一个章节，结束
                    break;
                }
                if !trimmed.is_empty() {
                    description_lines.push(trimmed);
                }
            }
        }

        if description_lines.is_empty() {
            // 回退：使用第一段非空内容
            content
                .lines()
                .find(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
                .unwrap_or("无描述")
                .to_string()
        } else {
            description_lines.join(" ")
        }
    }

    /// 生成简短摘要（用于注入系统提示）
    #[must_use]
    pub fn summary(&self) -> String {
        format!("**{}**: {}", self.name, self.description)
    }

    pub(crate) fn content_sha256(&self) -> String {
        format!("{:x}", sha2::Sha256::digest(self.content.as_bytes()))
    }
}

/// 技能发现器
pub struct SkillDiscovery {
    /// 技能根目录
    skills_root: PathBuf,
    workspace: PathBuf,
    /// 是否启用自动触发器激活
    auto_trigger_enabled: bool,
    lock_required: bool,
}

impl SkillDiscovery {
    /// 创建新的技能发现器
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            skills_root: workspace_path.join("skills"),
            workspace: workspace_path.to_path_buf(),
            auto_trigger_enabled: true,
            lock_required: false,
        }
    }

    /// Require an exact administrator-owned skills.lock.json catalog.
    #[must_use]
    pub fn with_lock_required(mut self, required: bool) -> Self {
        self.lock_required = required;
        self
    }

    /// 设置是否启用自动触发器激活
    #[must_use]
    pub fn with_auto_trigger(mut self, enabled: bool) -> Self {
        self.auto_trigger_enabled = enabled;
        self
    }

    /// 列出 `skills/` 下的一级子目录。目录不存在时返回空列表。
    fn skill_directories(&self) -> Result<Vec<crate::files::DirEntryInfo>, JiaClawError> {
        // lstat distinguishes an absent directory from a dangling link. This is
        // only an absence check: the capability I/O below enforces access safety.
        match std::fs::symlink_metadata(&self.skills_root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(invalid(e)),
            Ok(_) => (),
        }
        let (entries, truncated) =
            crate::memory_io::list_directory(&self.workspace, "skills", CATALOG_ENTRIES + 1, false)
                .map_err(invalid)?;
        if truncated || entries.len() > CATALOG_ENTRIES {
            return Err(invalid("技能目录扫描不完整或超过 256 个条目"));
        }
        Ok(entries)
    }

    /// 发现所有技能（启动时宽松模式：单个坏文件跳过并 warn）。
    ///
    /// # Errors
    ///
    /// 如果无法读取技能目录，返回错误。
    pub fn discover(&self) -> Result<Vec<Skill>, JiaClawError> {
        self.scan(false)
    }

    /// 严格扫描：任一现存 `SKILL.md` 无法读取或解析则失败（热加载用）。
    ///
    /// 缺少 `SKILL.md` 的子目录会被跳过（不是技能）。目录本身无法读取时返回错误。
    ///
    /// # Errors
    ///
    /// 无法读取 `skills/`，或至少一个技能文件无效。
    pub fn discover_strict(&self) -> Result<Vec<Skill>, JiaClawError> {
        self.scan(true)
    }

    fn scan(&self, strict: bool) -> Result<Vec<Skill>, JiaClawError> {
        let catalog_lock = if self.lock_required {
            Some(lock::CatalogLock::read(&self.workspace)?)
        } else {
            None
        };
        self.scan_locked(strict, catalog_lock)
    }

    /// Inspect the exact disk lock used by one strict scan, including disabled entries.
    ///
    /// # Errors
    /// Requires captured lock policy and a valid complete enabled catalog.
    pub fn inspect_policy(&self) -> Result<SkillLockPolicy, JiaClawError> {
        if !self.lock_required {
            return Err(invalid("技能策略检查要求 skill_lock_required=true"));
        }
        let lock = lock::CatalogLock::read(&self.workspace)?;
        let policy = lock.policy.clone();
        self.scan_locked(true, Some(lock))?;
        Ok(policy)
    }

    fn scan_locked(
        &self,
        strict: bool,
        catalog_lock: Option<lock::CatalogLock>,
    ) -> Result<Vec<Skill>, JiaClawError> {
        let strict = strict || self.lock_required;
        let mut skills = Vec::new();
        let mut names = HashSet::new();
        let mut bytes = 0;
        for entry in self.skill_directories()? {
            let path = self.skills_root.join(&entry.name);
            if entry.kind != "dir" && entry.kind != "symlink" {
                continue;
            }
            // Still reject a linked directory, but never open a disabled leaf.
            if entry.kind == "dir"
                && catalog_lock
                    .as_ref()
                    .is_some_and(|lock| lock.disabled(&entry.name))
            {
                continue;
            }
            // A linked directory is invalid even if its target has no SKILL.md.
            let loaded = if entry.kind == "symlink" {
                Err(invalid("技能目录禁止符号链接"))
            } else {
                Skill::read(
                    &self.workspace,
                    &format!("skills/{}/SKILL.md", entry.name),
                    &path,
                )
            };
            match loaded {
                Ok(Some((mut skill, raw_bytes, raw_hash))) => {
                    if let Some(lock) = &catalog_lock {
                        skill.source = Some(lock.bind(&entry.name, &raw_hash)?);
                    }
                    bytes += raw_bytes;
                    if bytes > CATALOG_BYTES || skills.len() == CATALOG_SKILLS {
                        return Err(invalid("技能目录最多 64 个技能/2 MiB 原始文本"));
                    }
                    if !names.insert(skill.name.clone()) {
                        return Err(invalid("技能名称重复"));
                    }
                    skills.push(skill);
                }
                Ok(None) => (),
                Err(e) => {
                    if strict {
                        return Err(e);
                    }
                    tracing::warn!("跳过无效技能目录 {}: {e}", path.display());
                }
            }
        }
        if let Some(lock) = catalog_lock {
            lock.complete(skills.len())?;
        }
        Ok(skills)
    }

    /// 查找特定技能
    ///
    /// # Errors
    ///
    /// 如果技能文件存在但无法解析，返回错误。
    pub fn find(&self, skill_name: &str) -> Result<Option<Skill>, JiaClawError> {
        if !identifier(skill_name)
            || Path::new(skill_name).components().count() != 1
            || !matches!(
                Path::new(skill_name).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(invalid("技能查询需要单个目录名，禁止路径穿越"));
        }
        if self.lock_required {
            return Ok(self.discover_strict()?.into_iter().find(|skill| {
                skill.path.file_name().and_then(|name| name.to_str()) == Some(skill_name)
            }));
        }
        let skill_dir = self.skills_root.join(skill_name);
        Skill::read(
            &self.workspace,
            &format!("skills/{skill_name}/SKILL.md"),
            &skill_dir,
        )
        .map(|loaded| loaded.map(|(skill, _, _)| skill))
    }

    /// 根据用户消息自动查找应该激活的技能
    ///
    /// 返回所有触发器匹配的技能名称列表
    pub fn auto_trigger_skills(
        &self,
        user_message: &str,
        discovered_skills: &[Skill],
    ) -> Vec<String> {
        if !self.auto_trigger_enabled {
            return Vec::new();
        }

        let message_lower = user_message.to_lowercase();
        let mut triggered = Vec::new();

        for skill in discovered_skills {
            if skill.triggers.is_empty() {
                continue;
            }

            for trigger in &skill.triggers {
                let trigger_lower = trigger.to_lowercase();
                if message_lower.contains(&trigger_lower) {
                    triggered.push(skill.name.clone());
                    tracing::debug!("技能 '{}' 由触发词 '{}' 自动激活", skill.name, trigger);
                    break;
                }
            }
        }

        triggered
    }
}

/// 进程内技能注册表：短读锁快照，热加载时短写锁替换。
#[derive(Debug)]
pub struct SkillRegistry {
    inner: RwLock<Vec<Skill>>,
    reload_slot: Arc<tokio::sync::Semaphore>,
    lock_required: bool,
}

impl SkillRegistry {
    /// 用已扫描的技能列表构造注册表。
    #[must_use]
    pub fn new(skills: Vec<Skill>) -> Self {
        Self {
            inner: RwLock::new(skills),
            reload_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            lock_required: false,
        }
    }

    /// Capture the immutable administrator lock policy for all later reloads.
    #[must_use]
    pub fn with_lock_required(mut self, required: bool) -> Self {
        self.lock_required = required;
        self
    }

    fn lock_read(&self) -> RwLockReadGuard<'_, Vec<Skill>> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_write(&self) -> RwLockWriteGuard<'_, Vec<Skill>> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 克隆当前技能列表（持锁时间短，调用方随后不再持锁）。
    #[must_use]
    pub fn snapshot(&self) -> Vec<Skill> {
        self.lock_read().clone()
    }

    // Read only the loaded table, never an ambient path. Selecting the body and
    // checking its expected version share the same read lock as the catalog.
    fn read_content(&self, name: &str, expected_hash: &str) -> Result<String, JiaClawError> {
        let guard = self.lock_read();
        let skill = guard
            .iter()
            .find(|skill| skill.name == name)
            .ok_or_else(|| {
                JiaClawError::ToolExecution(
                    "skill is not loaded; start a new turn after reviewing the catalog".into(),
                )
            })?;
        if skill.content.len() > SKILL_FILE_BYTES || skill.content_sha256() != expected_hash {
            return Err(JiaClawError::ToolExecution(
                "skill content version is unavailable; start a new turn after reviewing the catalog"
                    .into(),
            ));
        }
        // The native loop records a JSON string, whose escapes can exceed the
        // raw text budget. Reject before returning a successful result that the
        // loop would otherwise lose and mark for human review.
        if serde_json::to_string(&skill.content).map_or(true, |encoded| {
            encoded.len() > crate::tools::MAX_NATIVE_RESULT_BYTES
        }) {
            return Err(JiaClawError::ToolExecution(
                "skill body exceeds the serialized tool-output budget".into(),
            ));
        }
        Ok(skill.content.clone())
    }

    fn read_resource(
        &self,
        workspace: &Path,
        args: &SkillResourceArgs,
    ) -> Result<String, JiaClawError> {
        // Admission selects the current declaration under one short lock. A later
        // reload cannot retract an already selected read; its exact bytes stay pinned.
        let path = {
            let guard = self.lock_read();
            let skill = guard
                .iter()
                .find(|skill| skill.name == args.name)
                .ok_or_else(|| invalid("技能资源所属技能不再加载"))?;
            if skill.content.len() > SKILL_FILE_BYTES
                || skill.content_sha256() != args.content_sha256
                || skill.resources.len() > SKILL_RESOURCES
                || !skill.resources.iter().any(|resource| {
                    resource.valid()
                        && resource.path == args.path
                        && resource.sha256 == args.resource_sha256
                })
            {
                return Err(invalid("技能资源声明或正文版本已失效，请在新一轮核对目录"));
            }
            let relative = skill
                .path
                .strip_prefix(workspace)
                .map_err(|_| invalid("技能资源不属于配置工作区"))?;
            let parts: Vec<_> = relative.components().collect();
            if parts.len() != 2
                || parts[0].as_os_str() != "skills"
                || !parts
                    .iter()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(invalid("技能资源必须属于工作区一级技能目录"));
            }
            relative
                .join(&args.path)
                .to_str()
                .ok_or_else(|| invalid("技能资源需要 UTF-8 路径"))?
                .to_owned()
        };
        let file = crate::memory_io::read_text(workspace, &path, SKILL_FILE_BYTES, false)?
            .ok_or_else(|| invalid("已声明技能资源文件不存在"))?;
        if format!("{:x}", sha2::Sha256::digest(file.text.as_bytes())) != args.resource_sha256 {
            return Err(invalid("技能资源字节不匹配管理员声明的 SHA-256"));
        }
        if serde_json::to_string(&file.text).map_or(true, |encoded| {
            encoded.len() > crate::tools::MAX_NATIVE_RESULT_BYTES
        }) {
            return Err(invalid("技能资源超过序列化输出预算"));
        }
        Ok(file.text)
    }

    /// 重新扫描工作区 `skills/` 并替换注册表。
    ///
    /// 磁盘扫描在锁外完成；仅成功后短时间持写锁替换。失败时保留旧表。
    ///
    /// # Errors
    ///
    /// 目录无法读取，或任一 `SKILL.md` 无效。此时注册表内容不变。
    pub fn reload(&self, workspace_path: &Path) -> Result<Vec<Skill>, JiaClawError> {
        let _permit = self
            .reload_slot
            .try_acquire()
            .map_err(|_| invalid("技能重载正在进行"))?;
        self.scan_and_publish(workspace_path)
    }

    /// Bounded background reload. Cancellation of the waiter does not cancel the
    /// admitted scan or release its per-registry capacity; inspect the snapshot.
    ///
    /// # Errors
    /// Busy capacity, invalid input or a failed worker; no partial table is published.
    pub async fn reload_async(
        self: &Arc<Self>,
        workspace: &Path,
    ) -> Result<Vec<Skill>, JiaClawError> {
        let registry = Arc::clone(self);
        let workspace = workspace.to_path_buf();
        self.run_reload(move || registry.scan_and_publish(&workspace))
            .await
    }

    async fn run_reload<F>(&self, reload: F) -> Result<Vec<Skill>, JiaClawError>
    where
        F: FnOnce() -> Result<Vec<Skill>, JiaClawError> + Send + 'static,
    {
        let permit = Arc::clone(&self.reload_slot)
            .try_acquire_owned()
            .map_err(|_| invalid("技能重载正在进行，请先核对 /api/skills"))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            reload()
        })
        .await
        .map_err(|_| invalid("技能重载任务失败"))?
    }

    fn scan_and_publish(&self, workspace_path: &Path) -> Result<Vec<Skill>, JiaClawError> {
        let new_skills = SkillDiscovery::new(workspace_path)
            .with_lock_required(self.lock_required)
            .discover_strict()?;
        {
            let mut guard = self.lock_write();
            guard.clone_from(&new_skills);
        }
        Ok(new_skills)
    }
}

pub(crate) struct SkillReadTool {
    registry: Arc<SkillRegistry>,
}

impl SkillReadTool {
    pub(crate) fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillReadArgs {
    name: String,
    content_sha256: String,
}

#[async_trait::async_trait]
impl crate::tools::Tool for SkillReadTool {
    fn name(&self) -> &str {
        "skill_read"
    }

    fn description(&self) -> &str {
        "Read one administrator-reviewed loaded skill body using its exact catalog name and content_sha256. No filesystem paths or new tool permissions."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 128},
                "content_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"}
            },
            "required": ["name", "content_sha256"],
            "additionalProperties": false
        })
    }

    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    async fn execute(&self, args: serde_json::Value) -> Result<String, JiaClawError> {
        let args: SkillReadArgs = serde_json::from_value(args).map_err(|_| {
            JiaClawError::ToolExecution(
                "skill_read requires exactly name and content_sha256".into(),
            )
        })?;
        if !identifier(&args.name) || !valid_hash(&args.content_sha256) {
            return Err(JiaClawError::ToolExecution(
                "skill_read requires a bounded catalog name and lowercase SHA-256".into(),
            ));
        }
        self.registry.read_content(&args.name, &args.content_sha256)
    }
}

pub(crate) struct SkillResourceReadTool {
    registry: Arc<SkillRegistry>,
    workspace: PathBuf,
}

impl SkillResourceReadTool {
    pub(crate) fn new(registry: Arc<SkillRegistry>, workspace: &Path) -> Self {
        Self {
            registry,
            workspace: workspace.to_path_buf(),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillResourceArgs {
    name: String,
    content_sha256: String,
    path: String,
    resource_sha256: String,
}

#[async_trait::async_trait]
impl crate::tools::Tool for SkillResourceReadTool {
    fn name(&self) -> &str {
        "skill_resource_read"
    }

    fn description(&self) -> &str {
        "Read a declared references/ UTF-8 file using exact skill and resource hashes. Original request tool permissions apply; no paths or scripts outside the declaration."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "properties": {
                "name": {"type":"string", "minLength":1, "maxLength":128},
                "content_sha256": {"type":"string", "pattern":"^[0-9a-f]{64}$"},
                "path": {"type":"string", "minLength":1, "maxLength":256},
                "resource_sha256": {"type":"string", "pattern":"^[0-9a-f]{64}$"}
            }, "required":["name", "content_sha256", "path", "resource_sha256"],
            "additionalProperties":false
        })
    }

    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    async fn execute(&self, args: serde_json::Value) -> Result<String, JiaClawError> {
        let args: SkillResourceArgs = serde_json::from_value(args)
            .map_err(|_| invalid("skill_resource_read 需要精确的四个声明参数"))?;
        if !identifier(&args.name)
            || !valid_hash(&args.content_sha256)
            || !(SkillResource {
                path: args.path.clone(),
                sha256: args.resource_sha256.clone(),
            })
            .valid()
        {
            return Err(invalid(
                "skill_resource_read 需要目录名称、references/ 路径及小写 SHA-256",
            ));
        }
        let registry = Arc::clone(&self.registry);
        let workspace = self.workspace.clone();
        crate::memory_io::run_blocking(move || registry.read_resource(&workspace, &args)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;
    use std::fs;

    #[test]
    fn resource_declarations_are_bounded_canonical_and_atomic_on_reload() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&dir).unwrap();
        let declaration =
            serde_json::json!({"path":"references/guide.md", "sha256":"f".repeat(64)});
        let write = |resources: serde_json::Value| {
            fs::write(dir.join("SKILL.md"), format!("---\nname: reviewed\ndescription: catalog\njiaclaw_resources: {resources}\n---\nbody")).unwrap();
        };
        write(serde_json::json!([declaration]));
        let registry = SkillRegistry::new(
            SkillDiscovery::new(workspace.path())
                .discover_strict()
                .unwrap(),
        );
        assert_eq!(registry.snapshot()[0].resources.len(), 1);
        // Existence/contents are deliberately not inspected during discovery.
        for path in [
            "/references/guide.md",
            "references/../outside",
            "references/./guide.md",
            "references//guide.md",
            "references/",
            "scripts/run.sh",
            "references/a\\b",
            "references/C:drive",
            "references/ leading",
            "references/trailing ",
            "references/bad\nname",
        ] {
            write(serde_json::json!([{"path":path,"sha256":"f".repeat(64)}]));
            assert!(registry.reload(workspace.path()).is_err(), "{path}");
            assert_eq!(
                registry.snapshot()[0].resources[0].path,
                "references/guide.md"
            );
        }
        for resources in [serde_json::json!([declaration, declaration]),
            serde_json::json!([{"path":"references/guide.md","sha256":"F".repeat(64)}]),
            serde_json::json!([{"path":"references/guide.md","sha256":"f".repeat(64),"extra":true}]),
            serde_json::json!((0..9).map(|i| serde_json::json!({"path":format!("references/{i}.md"),"sha256":"f".repeat(64)})).collect::<Vec<_>>())] {
            write(resources);
            assert!(registry.reload(workspace.path()).is_err());
        }
        for length in [129, 245] {
            let path = format!("references/{}", "a".repeat(length));
            write(serde_json::json!([{"path":path,"sha256":"f".repeat(64)}]));
            registry.reload(workspace.path()).unwrap();
            assert_eq!(registry.snapshot()[0].resources[0].path, path);
        }
        write(
            serde_json::json!([{"path":format!("references/{}", "a".repeat(246)),"sha256":"f".repeat(64)}]),
        );
        assert!(registry.reload(workspace.path()).is_err());
        assert_eq!(registry.snapshot()[0].resources[0].path.len(), 256);
    }

    #[tokio::test]
    async fn resource_read_pins_bytes_and_current_declaration_without_extending_authority() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join("skills/reviewed");
        fs::create_dir_all(dir.join("references")).unwrap();
        let original = "approved raw resource\n";
        let hash = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
        let write = |resource_hash: &str| {
            fs::write(dir.join("SKILL.md"),
            format!("---\nname: reviewed\ndescription: catalog\njiaclaw_resources: [{{path: references/guide.md, sha256: {resource_hash}}}]\n---\nbody")).unwrap()
        };
        write(&hash);
        fs::write(dir.join("references/guide.md"), original).unwrap();
        let registry = Arc::new(SkillRegistry::new(
            SkillDiscovery::new(workspace.path())
                .discover_strict()
                .unwrap(),
        ));
        let body_hash = registry.snapshot()[0].content_sha256();
        let args = serde_json::json!({"name":"reviewed", "content_sha256":body_hash,
            "path":"references/guide.md", "resource_sha256":hash});
        let tool = SkillResourceReadTool::new(Arc::clone(&registry), workspace.path());
        assert_eq!(tool.execute(args.clone()).await.unwrap(), original);
        fs::write(dir.join("references/guide.md"), "replacement").unwrap();
        assert!(tool.execute(args.clone()).await.is_err());
        fs::write(dir.join("references/guide.md"), original).unwrap();
        write(&"a".repeat(64));
        registry.reload(workspace.path()).unwrap();
        assert_eq!(registry.snapshot()[0].content_sha256(), body_hash);
        assert!(tool.execute(args.clone()).await.is_err());
        write(&hash);
        registry.reload(workspace.path()).unwrap();
        for path in [
            "../outside",
            "references/undeclared.md",
            "references/./guide.md",
        ] {
            let mut bad = args.clone();
            bad["path"] = serde_json::json!(path);
            assert!(tool.execute(bad).await.is_err());
        }
        let outside = tempfile::tempdir().unwrap();
        let mut skills = registry.snapshot();
        skills[0].path = outside.path().join("skills/reviewed");
        let foreign =
            SkillResourceReadTool::new(Arc::new(SkillRegistry::new(skills)), workspace.path());
        assert!(foreign.execute(args.clone()).await.is_err());
        let mut extra = args;
        extra["extra"] = serde_json::json!(true);
        assert!(tool.execute(extra).await.is_err());
        assert_eq!(
            tool.failure_effect(),
            crate::tools::ToolFailureEffect::NoEffect
        );
    }

    #[tokio::test]
    async fn skill_read_uses_loaded_version_and_never_an_ambient_path() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: declared-name\ndescription: catalog only\n---\noriginal body",
        )
        .unwrap();
        let registry = Arc::new(SkillRegistry::new(
            SkillDiscovery::new(workspace.path())
                .discover_strict()
                .unwrap(),
        ));
        let original = registry.snapshot().pop().unwrap();
        let tool = SkillReadTool::new(Arc::clone(&registry));
        let args =
            serde_json::json!({"name": original.name, "content_sha256": original.content_sha256()});
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: declared-name\ndescription: catalog only\n---\nreplacement body",
        )
        .unwrap();
        assert_eq!(tool.execute(args.clone()).await.unwrap(), "original body");
        registry.reload(workspace.path()).unwrap();
        assert!(tool.execute(args).await.is_err());
        let replacement = registry.snapshot().pop().unwrap();
        assert_eq!(tool.execute(serde_json::json!({"name": replacement.name, "content_sha256": replacement.content_sha256()})).await.unwrap(), "replacement body");
        fs::remove_dir_all(workspace.path().join("skills")).unwrap();
        registry.reload(workspace.path()).unwrap();
        assert!(tool.execute(serde_json::json!({"name": replacement.name, "content_sha256": replacement.content_sha256()})).await.is_err());
        for name in [
            "../private.txt",
            "/private.txt",
            "",
            " declared-name",
            "bad\nname",
        ] {
            assert!(tool
                .execute(
                    serde_json::json!({"name": name, "content_sha256": original.content_sha256()})
                )
                .await
                .is_err());
        }
        assert_eq!(
            tool.failure_effect(),
            crate::tools::ToolFailureEffect::NoEffect
        );
    }

    #[tokio::test]
    async fn skill_read_rejects_invalid_arguments_and_unbounded_library_inputs() {
        let skill = Skill {
            name: "reviewed".into(),
            description: String::new(),
            path: PathBuf::new(),
            content: "x".repeat(SKILL_FILE_BYTES + 1),
            triggers: Vec::new(),
            resources: Vec::new(),
            source: None,
        };
        let args =
            serde_json::json!({"name": skill.name, "content_sha256": skill.content_sha256()});
        let tool = SkillReadTool::new(Arc::new(SkillRegistry::new(vec![skill])));
        assert!(tool.execute(args.clone()).await.is_err());
        for invalid in [
            serde_json::json!({"name": "reviewed"}),
            serde_json::json!({"name": "reviewed", "content_sha256": "f".repeat(63)}),
            serde_json::json!({"name": "reviewed", "content_sha256": "F".repeat(64)}),
            serde_json::json!({"name": "reviewed", "content_sha256": "g".repeat(64)}),
            serde_json::json!({"name": "reviewed", "content_sha256": "f".repeat(64), "path": "SKILL.md"}),
            serde_json::json!([]),
        ] {
            assert!(tool.execute(invalid).await.is_err());
        }
    }

    #[tokio::test]
    async fn skill_read_checks_serialized_output_before_success() {
        for (body, succeeds) in [
            ("x".repeat(SKILL_FILE_BYTES), true),
            ("\u{0001}".repeat(SKILL_FILE_BYTES), false),
        ] {
            let skill = Skill {
                name: "reviewed".into(),
                description: String::new(),
                path: PathBuf::new(),
                content: body.clone(),
                triggers: Vec::new(),
                resources: Vec::new(),
                source: None,
            };
            let args =
                serde_json::json!({"name": skill.name, "content_sha256": skill.content_sha256()});
            let tool = SkillReadTool::new(Arc::new(SkillRegistry::new(vec![skill])));
            let result = tool.execute(args).await;
            assert_eq!(result.is_ok(), succeeds);
            if succeeds {
                assert_eq!(result.unwrap(), body);
            }
        }
    }

    #[tokio::test]
    async fn cancelled_reload_keeps_capacity_until_worker_finishes_and_isolates_registries() {
        let registry = Arc::new(SkillRegistry::new(Vec::new()));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = Arc::clone(&registry);
        let waiting = tokio::spawn(async move {
            worker
                .run_reload(move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(Vec::new())
                })
                .await
        });
        entered_rx.await.unwrap();
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert!(registry
            .run_reload(|| panic!("busy reload must not dispatch"))
            .await
            .is_err());
        let workspace = tempfile::tempdir().unwrap();
        assert!(registry.reload(workspace.path()).is_err());
        let independent = Arc::new(SkillRegistry::new(Vec::new()));
        assert!(independent
            .reload_async(workspace.path())
            .await
            .unwrap()
            .is_empty());
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if registry.reload_async(workspace.path()).await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn catalog_limits_are_complete_or_rejected() {
        let workspace = tempfile::tempdir().unwrap();
        let discovery = SkillDiscovery::new(workspace.path());
        for index in 0..64 {
            write_skill(
                workspace.path(),
                &format!("skill-{index:02}"),
                "# Body\ntext",
            );
        }
        let skills = discovery.discover_strict().unwrap();
        assert_eq!(skills.len(), 64);
        assert_eq!(skills[0].name, "skill-00");
        write_skill(workspace.path(), "skill-64", "# Body\ntext");
        assert!(discovery.discover().is_err());
        assert!(discovery.discover_strict().is_err());

        let workspace = tempfile::tempdir().unwrap();
        let discovery = SkillDiscovery::new(workspace.path());
        let prefix = "---\ndescription: bounded\n---\n";
        let body = format!("{prefix}{}", "x".repeat(SKILL_FILE_BYTES - prefix.len()));
        for index in 0..16 {
            write_skill(workspace.path(), &format!("skill-{index:02}"), &body);
        }
        assert_eq!(discovery.discover_strict().unwrap().len(), 16);
        write_skill(workspace.path(), "skill-16", "text");
        assert!(discovery.discover().is_err());
        assert!(discovery.discover_strict().is_err());

        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("skills");
        fs::create_dir(&root).unwrap();
        for index in 0..256 {
            fs::create_dir(root.join(format!("empty-{index}"))).unwrap();
        }
        let discovery = SkillDiscovery::new(workspace.path());
        assert!(discovery.discover_strict().unwrap().is_empty());
        fs::write(root.join("extra"), "not a skill").unwrap();
        assert!(discovery.discover().is_err());
        assert!(discovery.discover_strict().is_err());
    }

    #[test]
    fn skill_metadata_limits_and_find_authority() {
        let workspace = tempfile::tempdir().unwrap();
        let content = format!(
            "---\nname: {}\ndescription: {}\ntriggers: [{}]\n---\nbody",
            "n".repeat(128),
            "d".repeat(2048),
            vec!["t".repeat(128); 32].join(", ")
        );
        write_skill(workspace.path(), "valid", &content);
        let discovery = SkillDiscovery::new(workspace.path());
        assert!(discovery.find("valid").unwrap().is_some());
        assert!(discovery.find("missing").unwrap().is_none());
        for name in [
            "../external",
            "/external",
            "valid/../valid",
            ".",
            "..",
            " valid",
            "valid\n",
        ] {
            assert!(discovery.find(name).is_err(), "{name:?}");
        }
        for bad in [
            "---\nname: ''\n---\nbody",
            "---\ntriggers: ['']\n---\nbody",
            "---\nname: \"bad\\nname\"\n---\nbody",
        ] {
            write_skill(workspace.path(), "invalid", bad);
            assert!(discovery.discover_strict().is_err());
            assert_eq!(discovery.discover().unwrap().len(), 1);
        }
    }

    #[cfg(unix)]
    #[test]
    fn direct_skill_read_rejects_linked_directory_and_leaf() {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), "synthetic outside").unwrap();
        let linked = workspace.path().join("linked");
        symlink(outside.path(), &linked).unwrap();
        assert!(Skill::from_file(&linked).is_err());
        let normal = workspace.path().join("normal");
        fs::create_dir(&normal).unwrap();
        symlink(outside.path().join("SKILL.md"), normal.join("SKILL.md")).unwrap();
        assert!(Skill::from_file(&normal).is_err());
    }

    #[test]
    fn test_skill_from_file() {
        let temp_dir = std::env::temp_dir().join("jiaclaw_test_skill");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let skill_content = r"# Test Skill

## Description

This is a test skill for testing purposes.

## Tools

- test_tool

## Usage

Use this for testing.
";

        fs::write(temp_dir.join("SKILL.md"), skill_content).unwrap();

        let skill = Skill::from_file(&temp_dir).unwrap();
        assert_eq!(skill.name, "jiaclaw_test_skill");
        assert!(skill.description.contains("test skill"));
        assert!(skill.triggers.is_empty());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_skill_with_frontmatter() {
        let temp_dir = std::env::temp_dir().join("jiaclaw_test_frontmatter");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let skill_content = r"---
name: web_search
description: 搜索互联网信息
triggers:
  - search
  - 搜索
  - find
---

# Web Search Skill

这是一个网络搜索技能。

## Usage

使用此技能搜索网络信息。
";

        fs::write(temp_dir.join("SKILL.md"), skill_content).unwrap();

        let skill = Skill::from_file(&temp_dir).unwrap();
        assert_eq!(skill.name, "web_search");
        assert_eq!(skill.description, "搜索互联网信息");
        assert_eq!(skill.triggers, vec!["search", "搜索", "find"]);
        assert!(skill.content.contains("这是一个网络搜索技能"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_skill_discovery() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_discovery");
        let _ = fs::remove_dir_all(&temp_workspace);

        let skills_dir = temp_workspace.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        // 创建两个测试技能
        let skill1_dir = skills_dir.join("skill1");
        fs::create_dir_all(&skill1_dir).unwrap();
        fs::write(
            skill1_dir.join("SKILL.md"),
            "# Skill 1\n\n## Description\n\nFirst skill",
        )
        .unwrap();

        let skill2_dir = skills_dir.join("skill2");
        fs::create_dir_all(&skill2_dir).unwrap();
        fs::write(
            skill2_dir.join("SKILL.md"),
            "# Skill 2\n\n## Description\n\nSecond skill",
        )
        .unwrap();

        let discovery = SkillDiscovery::new(&temp_workspace);
        let skills = discovery.discover().unwrap();

        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|s| s.name == "skill1"));
        assert!(skills.iter().any(|s| s.name == "skill2"));

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[test]
    fn test_skill_find() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_find");
        let _ = fs::remove_dir_all(&temp_workspace);

        let skills_dir = temp_workspace.join("skills");
        let skill_dir = skills_dir.join("findme");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Find Me\n\n## Description\n\nTest",
        )
        .unwrap();

        let discovery = SkillDiscovery::new(&temp_workspace);

        let found = discovery.find("findme").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "findme");

        let not_found = discovery.find("notexist").unwrap();
        assert!(not_found.is_none());

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[test]
    fn test_auto_trigger_skills() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_trigger");
        let _ = fs::remove_dir_all(&temp_workspace);

        let skills_dir = temp_workspace.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        let search_skill = r"---
name: web_search
description: 搜索互联网
triggers:
  - search
  - 搜索
---
# Web Search
";
        let search_dir = skills_dir.join("web_search");
        fs::create_dir_all(&search_dir).unwrap();
        fs::write(search_dir.join("SKILL.md"), search_skill).unwrap();

        let calc_skill = r"---
name: calculator
description: 计算器
triggers:
  - calculate
  - 计算
---
# Calculator
";
        let calc_dir = skills_dir.join("calculator");
        fs::create_dir_all(&calc_dir).unwrap();
        fs::write(calc_dir.join("SKILL.md"), calc_skill).unwrap();

        let discovery = SkillDiscovery::new(&temp_workspace);
        let skills = discovery.discover().unwrap();

        let triggered = discovery.auto_trigger_skills("请帮我搜索一下", &skills);
        assert_eq!(triggered.len(), 1);
        assert!(triggered.contains(&"web_search".to_string()));

        let triggered = discovery.auto_trigger_skills("help me search and calculate", &skills);
        assert_eq!(triggered.len(), 2);
        assert!(triggered.contains(&"web_search".to_string()));
        assert!(triggered.contains(&"calculator".to_string()));

        let triggered = discovery.auto_trigger_skills("hello world", &skills);
        assert_eq!(triggered.len(), 0);

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[test]
    fn test_auto_trigger_disabled() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_trigger_disabled");
        let _ = fs::remove_dir_all(&temp_workspace);

        let skills_dir = temp_workspace.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        let search_skill = r"---
name: web_search
triggers:
  - search
---
# Web Search
";
        let search_dir = skills_dir.join("web_search");
        fs::create_dir_all(&search_dir).unwrap();
        fs::write(search_dir.join("SKILL.md"), search_skill).unwrap();

        let discovery = SkillDiscovery::new(&temp_workspace).with_auto_trigger(false);
        let skills = discovery.discover().unwrap();

        let triggered = discovery.auto_trigger_skills("search for something", &skills);
        assert_eq!(triggered.len(), 0);

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    fn unique_workspace(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(workspace: &Path, name: &str, body: &str) {
        let dir = workspace.join("skills").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn discover_strict_loads_valid_skills() {
        let workspace = unique_workspace("jiaclaw_strict_ok");
        write_skill(
            &workspace,
            "alpha",
            "# Alpha\n\n## Description\n\nFirst skill\n",
        );

        let skills = SkillDiscovery::new(&workspace).discover_strict().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "alpha");

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn discover_strict_fails_on_bad_skill_file() {
        let workspace = unique_workspace("jiaclaw_strict_bad");
        write_skill(
            &workspace,
            "alpha",
            "# Alpha\n\n## Description\n\nFirst skill\n",
        );
        write_skill(
            &workspace,
            "broken",
            "---\nname: [not yaml\n---\n# Broken\n",
        );

        let err = SkillDiscovery::new(&workspace)
            .discover_strict()
            .expect_err("坏文件应使严格扫描失败");
        assert!(
            err.to_string().contains("无效技能"),
            "错误应说明无效技能，实际: {err}"
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn registry_reload_replaces_table() {
        let workspace = unique_workspace("jiaclaw_registry_reload");
        write_skill(
            &workspace,
            "alpha",
            "# Alpha\n\n## Description\n\nFirst skill\n",
        );
        let registry = SkillRegistry::new(
            SkillDiscovery::new(&workspace)
                .discover()
                .expect("初始扫描"),
        );
        assert_eq!(registry.snapshot().len(), 1);

        write_skill(
            &workspace,
            "beta",
            "# Beta\n\n## Description\n\nSecond skill\n",
        );
        let reloaded = registry.reload(&workspace).expect("重载应成功");
        assert_eq!(reloaded.len(), 2);
        let names: Vec<_> = registry.snapshot().iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn registry_reload_keeps_old_table_on_bad_file() {
        let workspace = unique_workspace("jiaclaw_registry_keep");
        write_skill(
            &workspace,
            "alpha",
            "# Alpha\n\n## Description\n\nFirst skill\n",
        );
        let registry = SkillRegistry::new(
            SkillDiscovery::new(&workspace)
                .discover()
                .expect("初始扫描"),
        );

        write_skill(
            &workspace,
            "broken",
            "---\nname: [not yaml\n---\n# Broken\n",
        );
        let err = registry.reload(&workspace).expect_err("坏文件应使重载失败");
        assert!(
            err.to_string().contains("无效技能"),
            "错误应说明无效技能，实际: {err}"
        );

        let names: Vec<_> = registry.snapshot().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["alpha".to_string()]);

        let _ = fs::remove_dir_all(&workspace);
    }
}
