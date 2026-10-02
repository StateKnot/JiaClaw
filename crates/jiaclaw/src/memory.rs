// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 工作区 `MEMORY.md` 长期记忆：路径校验、提示注入读取、原子写入。

#![allow(clippy::module_name_repetitions)]

use crate::tools::Tool;
use async_trait::async_trait;
use jiaclaw_core::{JiaClawError, MEMORY_PROMPT_MAX_BYTES};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// 工作区约定文件在磁盘上的状态（MEMORY / SOUL / USER，供 `doctor` / CLI 使用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFileStatus {
    /// 解析后的绝对或工作区拼接路径
    pub path: PathBuf,
    /// 文件是否存在
    pub exists: bool,
    /// 文件大小（字节）；不存在时为 0
    pub size_bytes: u64,
}

/// 将配置中的相对路径解析为工作区内文件路径。
///
/// 拒绝绝对路径和任何 `..` 组件，防止路径穿越。SOUL / USER 与 MEMORY 共用此解析。
///
/// # Errors
///
/// 路径为空、绝对路径、包含 `..`，或不落在工作空间内时返回错误。
pub fn resolve_workspace_relative_path(
    workspace: &Path,
    configured: &str,
) -> Result<PathBuf, JiaClawError> {
    let configured = configured.trim();
    if configured.is_empty() {
        return Err(JiaClawError::Configuration("路径不能为空".to_string()));
    }

    let rel = Path::new(configured);
    if rel.is_absolute() {
        return Err(JiaClawError::Configuration(format!(
            "路径必须相对于工作空间，禁止绝对路径: {configured}"
        )));
    }

    for component in rel.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(JiaClawError::Configuration(format!(
                    "路径禁止路径穿越 (..): {configured}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(JiaClawError::Configuration(format!(
                    "路径必须相对于工作空间: {configured}"
                )));
            }
        }
    }

    let workspace_base = canonicalize_existing_or_clone(workspace);
    let joined = workspace_base.join(rel);

    ensure_path_within_workspace(&workspace_base, &joined)?;
    Ok(joined)
}

/// 将配置中的相对路径解析为工作区内的记忆文件路径。
///
/// # Errors
///
/// 路径为空、绝对路径、包含 `..`，或不落在工作空间内时返回错误。
pub fn resolve_memory_path(workspace: &Path, configured: &str) -> Result<PathBuf, JiaClawError> {
    resolve_workspace_relative_path(workspace, configured)
}

/// 读取工作区约定文件以注入系统提示（MEMORY / SOUL / USER 共用）。
///
/// 文件不存在或（trim 后）为空时返回 `Ok(None)`，不报错。
/// 超过 [`MEMORY_PROMPT_MAX_BYTES`] 时截断到 UTF-8 边界并 `warn`。
///
/// # Errors
///
/// 路径非法，或文件存在但无法读取时返回错误。
pub fn load_prompt_file(
    workspace: &Path,
    configured: &str,
    kind: &str,
) -> Result<Option<String>, JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, configured)?;
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        tracing::warn!(
            path = %path.display(),
            kind,
            "{kind} 路径存在但不是文件，跳过注入"
        );
        return Ok(None);
    }

    ensure_existing_within_workspace(workspace, &path)?;

    let raw = std::fs::read_to_string(&path).map_err(|e| {
        JiaClawError::Configuration(format!("无法读取{kind}文件 {}: {e}", path.display()))
    })?;

    if raw.trim().is_empty() {
        return Ok(None);
    }

    if raw.len() > MEMORY_PROMPT_MAX_BYTES {
        tracing::warn!(
            path = %path.display(),
            kind,
            size_bytes = raw.len(),
            limit_bytes = MEMORY_PROMPT_MAX_BYTES,
            "{kind} 文件过大，截断后注入系统提示"
        );
        Ok(Some(
            truncate_utf8(&raw, MEMORY_PROMPT_MAX_BYTES).to_string(),
        ))
    } else {
        Ok(Some(raw))
    }
}

/// 读取记忆文件以注入系统提示。
///
/// 文件不存在或（trim 后）为空时返回 `Ok(None)`，不报错。
/// 超过 [`MEMORY_PROMPT_MAX_BYTES`] 时截断到 UTF-8 边界并 `warn`。
///
/// # Errors
///
/// 路径非法，或文件存在但无法读取时返回错误。
pub fn load_memory_for_prompt(
    workspace: &Path,
    configured: &str,
) -> Result<Option<String>, JiaClawError> {
    load_prompt_file(workspace, configured, "MEMORY")
}

/// 检查工作区约定文件是否存在及其大小。
///
/// # Errors
///
/// 配置路径非法时返回错误。
pub fn inspect_workspace_file(
    workspace: &Path,
    configured: &str,
) -> Result<MemoryFileStatus, JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, configured)?;
    if path.is_file() {
        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(MemoryFileStatus {
            path,
            exists: true,
            size_bytes,
        })
    } else {
        Ok(MemoryFileStatus {
            path,
            exists: false,
            size_bytes: 0,
        })
    }
}

/// 检查记忆文件是否存在及其大小。
///
/// # Errors
///
/// 配置路径非法时返回错误。
pub fn inspect_memory_file(
    workspace: &Path,
    configured: &str,
) -> Result<MemoryFileStatus, JiaClawError> {
    inspect_workspace_file(workspace, configured)
}

/// 写入工作区约定文件：`replace = true` 覆盖；否则追加 Markdown 段落。
///
/// 只能写约定路径。追加时在已有内容与新段落之间插入换行分隔，并以临时文件 + rename 原子落盘。
///
/// # Errors
///
/// 路径非法、越出工作空间，或 IO 失败时返回错误。
pub fn write_workspace_file(
    workspace: &Path,
    configured: &str,
    content: &str,
    replace: bool,
) -> Result<PathBuf, JiaClawError> {
    write_workspace_file_with_limit(workspace, configured, content, replace, None)
}

/// 写入工作区约定文件；`max_bytes` 若设置，结果超过上限则报错且不落盘。
///
/// # Errors
///
/// 路径非法、越出工作空间、超过上限，或 IO 失败时返回错误。
pub fn write_workspace_file_with_limit(
    workspace: &Path,
    configured: &str,
    content: &str,
    replace: bool,
    max_bytes: Option<usize>,
) -> Result<PathBuf, JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, configured)?;
    ensure_path_within_workspace(workspace, &path)?;

    if path.exists() {
        ensure_existing_within_workspace(workspace, &path)?;
        if !path.is_file() {
            return Err(JiaClawError::ToolExecution(format!(
                "路径不是文件: {}",
                path.display()
            )));
        }
    }

    let new_contents = if replace || !path.exists() {
        content.to_string()
    } else {
        let existing = std::fs::read_to_string(&path).map_err(|e| {
            JiaClawError::ToolExecution(format!("无法读取文件 {}: {e}", path.display()))
        })?;
        join_memory_append(&existing, content)
    };

    if let Some(limit) = max_bytes {
        if new_contents.len() > limit {
            return Err(JiaClawError::ToolExecution(format!(
                "记忆文件超过上限 {limit} 字节（将写入 {} 字节）",
                new_contents.len()
            )));
        }
    }

    atomic_write(&path, &new_contents)?;
    Ok(path)
}

/// 写入记忆文件：默认追加一段 Markdown；`replace = true` 时覆盖。
///
/// 只能写约定路径。追加时在已有内容与新段落之间插入换行分隔，并以临时文件 + rename 原子落盘。
///
/// # Errors
///
/// 路径非法、越出工作空间，或 IO 失败时返回错误。
pub fn write_memory(
    workspace: &Path,
    configured: &str,
    content: &str,
    replace: bool,
) -> Result<PathBuf, JiaClawError> {
    write_memory_with_limit(workspace, configured, content, replace, None)
}

/// 写入记忆文件；`max_bytes` 若设置，结果超过上限则报错且不落盘。
///
/// # Errors
///
/// 路径非法、越出工作空间、超过上限，或 IO 失败时返回错误。
pub fn write_memory_with_limit(
    workspace: &Path,
    configured: &str,
    content: &str,
    replace: bool,
    max_bytes: Option<usize>,
) -> Result<PathBuf, JiaClawError> {
    write_workspace_file_with_limit(workspace, configured, content, replace, max_bytes)
}

/// 追加时用空行分隔已有内容与新段落。
#[must_use]
pub fn join_memory_append(existing: &str, addition: &str) -> String {
    if existing.is_empty() {
        return addition.to_string();
    }
    let mut out = existing.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(addition);
    out
}

/// 按 UTF-8 字符边界截断到最多 `max_bytes` 字节。
#[must_use]
pub fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub(crate) fn canonicalize_existing_or_clone(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn ensure_path_within_workspace(workspace: &Path, target: &Path) -> Result<(), JiaClawError> {
    let ws = canonicalize_existing_or_clone(workspace);

    if target.exists() {
        let check = canonicalize_existing_or_clone(target);
        if check.starts_with(&ws) {
            return Ok(());
        }
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 文件 {} 不在工作空间 {} 内",
            target.display(),
            ws.display()
        )));
    }

    if target.starts_with(&ws) {
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        let parent_check = if parent.exists() {
            canonicalize_existing_or_clone(parent)
        } else {
            parent.to_path_buf()
        };
        if parent_check.starts_with(&ws) {
            return Ok(());
        }
    }

    Err(JiaClawError::ToolExecution(format!(
        "安全错误: 文件 {} 不在工作空间 {} 内",
        target.display(),
        ws.display()
    )))
}

pub(crate) fn ensure_existing_within_workspace(
    workspace: &Path,
    path: &Path,
) -> Result<(), JiaClawError> {
    let ws = canonicalize_existing_or_clone(workspace);
    let canon = path.canonicalize().map_err(|e| {
        JiaClawError::ToolExecution(format!("无法解析文件 {}: {e}", path.display()))
    })?;
    if !canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 文件 {} 指向工作空间外部",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<(), JiaClawError> {
    atomic_write_bytes(path, contents.as_bytes())
}

pub(crate) fn atomic_write_bytes(path: &Path, contents: &[u8]) -> Result<(), JiaClawError> {
    let parent = path.parent().ok_or_else(|| {
        JiaClawError::ToolExecution(format!("无效的文件路径: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent).map_err(|e| {
        JiaClawError::ToolExecution(format!("无法创建文件目录 {}: {e}", parent.display()))
    })?;

    let file_name = path
        .file_name()
        .ok_or_else(|| JiaClawError::ToolExecution(format!("无效的文件名: {}", path.display())))?;
    let tmp = parent.join(format!("{}.tmp", file_name.to_string_lossy()));

    std::fs::write(&tmp, contents).map_err(|e| {
        JiaClawError::ToolExecution(format!("无法写入临时文件 {}: {e}", tmp.display()))
    })?;

    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        JiaClawError::ToolExecution(format!(
            "无法提交文件 {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;

    Ok(())
}

/// `memory_append` 工具：向约定 MEMORY 路径追加或覆盖 Markdown。
pub struct MemoryAppendTool {
    workspace_path: PathBuf,
    memory_rel_path: String,
}

impl MemoryAppendTool {
    /// 创建工具；`memory_rel_path` 相对工作空间，默认 `MEMORY.md`。
    #[must_use]
    pub fn new(workspace_path: &Path, memory_rel_path: impl Into<String>) -> Self {
        let canonical_workspace = canonicalize_existing_or_clone(workspace_path);
        Self {
            workspace_path: canonical_workspace,
            memory_rel_path: memory_rel_path.into(),
        }
    }
}

#[async_trait]
impl Tool for MemoryAppendTool {
    fn name(&self) -> &str {
        "memory_append"
    }

    fn description(&self) -> &str {
        "将一段 Markdown 写入工作区长期记忆文件（默认 MEMORY.md）。replace=false 时追加并换行分隔；replace=true 时覆盖整个文件。只能写约定路径。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "要写入的 Markdown 内容"
                },
                "replace": {
                    "type": "boolean",
                    "description": "true 覆盖整个记忆文件；false（默认）追加",
                    "default": false
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'content'".to_string()))?;

        let replace = args
            .get("replace")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let path = write_memory(
            &self.workspace_path,
            &self.memory_rel_path,
            content,
            replace,
        )?;

        let metadata = std::fs::metadata(&path).ok();
        let size = metadata.map_or(0, |m| m.len());
        let mode = if replace { "覆盖" } else { "追加" };

        Ok(format!(
            "✅ 长期记忆已{mode}: {}\n大小: {size} 字节\n约定路径: {}",
            path.display(),
            self.memory_rel_path
        ))
    }
}

/// `memory_write` 落盘上限（字节），与系统提示注入截断对齐。
pub const MEMORY_WRITE_MAX_BYTES: usize = MEMORY_PROMPT_MAX_BYTES;

/// `memory_write` 写入模式：整文件追加或覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryWriteMode {
    /// 在已有内容后追加（默认）
    Append,
    /// 覆盖整个记忆文件
    Overwrite,
}

impl MemoryWriteMode {
    /// 解析 `append` / `overwrite`（大小写不敏感，首尾空白忽略）。
    ///
    /// # Errors
    ///
    /// 其它字符串返回错误。
    pub fn parse(raw: &str) -> Result<Self, JiaClawError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "append" => Ok(Self::Append),
            "overwrite" => Ok(Self::Overwrite),
            _ => Err(JiaClawError::ToolExecution(
                "参数 'mode' 必须是 append 或 overwrite（默认 append）".to_string(),
            )),
        }
    }

    /// 配置 / JSON 用短名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Overwrite => "overwrite",
        }
    }

    /// `overwrite` 对应 `replace = true`。
    #[must_use]
    pub fn is_overwrite(self) -> bool {
        matches!(self, Self::Overwrite)
    }
}

/// 解析 `memory_write` 参数：必填 `content`，可选 `mode`（默认 `append`）。
///
/// 忽略 `path` / `section` 等额外字段，始终只写配置的 MEMORY 路径。
///
/// # Errors
///
/// `content` 缺失或不是字符串，或 `mode` 非法时返回错误。
pub fn parse_memory_write_args(args: &Value) -> Result<(String, MemoryWriteMode), JiaClawError> {
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'content'".to_string()))?;

    let mode = match args.get("mode") {
        None | Some(Value::Null) => MemoryWriteMode::Append,
        Some(Value::String(raw)) => MemoryWriteMode::parse(raw)?,
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'mode' 必须是 append 或 overwrite（默认 append）".to_string(),
            ));
        }
    };

    Ok((content.to_string(), mode))
}

#[derive(Serialize)]
struct MemoryWriteOutput {
    path: String,
    mode: MemoryWriteMode,
    bytes_written: usize,
}

/// `memory_write` 工具：向约定 MEMORY 路径追加或覆盖 Markdown（不调用 LLM）。
pub struct MemoryWriteTool {
    workspace_path: PathBuf,
    memory_rel_path: String,
}

impl MemoryWriteTool {
    /// 创建工具；`memory_rel_path` 相对工作空间，默认 `MEMORY.md`。
    #[must_use]
    pub fn new(workspace_path: &Path, memory_rel_path: impl Into<String>) -> Self {
        let canonical_workspace = canonicalize_existing_or_clone(workspace_path);
        Self {
            workspace_path: canonical_workspace,
            memory_rel_path: memory_rel_path.into(),
        }
    }
}

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "memory_write"
    }

    fn description(&self) -> &str {
        "将 Markdown 写入工作区长期记忆文件（配置的 MEMORY.md / [memory] path）。mode=append（默认）追加并换行分隔；mode=overwrite 覆盖整个文件。只能写约定路径，禁止穿越。结果文件不得超过 32KiB。返回 {path, mode, bytes_written}。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "要写入的 Markdown 内容"
                },
                "mode": {
                    "type": "string",
                    "enum": ["append", "overwrite"],
                    "description": "append（默认）追加；overwrite 覆盖整个文件",
                    "default": "append"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let (content, mode) = parse_memory_write_args(&args)?;
        let path = write_memory_with_limit(
            &self.workspace_path,
            &self.memory_rel_path,
            &content,
            mode.is_overwrite(),
            Some(MEMORY_WRITE_MAX_BYTES),
        )?;

        let bytes_written = std::fs::metadata(&path)
            .ok()
            .and_then(|m| usize::try_from(m.len()).ok())
            .unwrap_or(0);
        let output = MemoryWriteOutput {
            path: self.memory_rel_path.clone(),
            mode,
            bytes_written,
        };
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化写入结果失败: {e}")))
    }
}

/// `max_results` 缺省值
pub const MEMORY_SEARCH_DEFAULT_MAX_RESULTS: usize = 5;

/// `max_results` 上限（含）
pub const MEMORY_SEARCH_MAX_RESULTS: usize = 20;

/// 单文件读取上限（字节）；超过则截断并 warn
pub const MEMORY_SEARCH_FILE_MAX_BYTES: usize = 512 * 1024;

/// 命中行前后各保留的上下文行数
pub const MEMORY_SEARCH_LINE_RADIUS: usize = 2;

/// 将 `max_results` 钳制到 `1..=20`。
#[must_use]
pub fn clamp_memory_search_max_results(raw: u64) -> usize {
    usize::try_from(raw)
        .unwrap_or(MEMORY_SEARCH_MAX_RESULTS)
        .clamp(1, MEMORY_SEARCH_MAX_RESULTS)
}

/// 解析 `memory_search` 参数：必填 `query`，可选 `max_results`（默认 5，钳制 1..=20），可选 `paths`。
///
/// `paths` 缺省、为 `null` 或空数组时返回 `None`，调用方应使用配置的 MEMORY / SOUL / USER 路径。
///
/// # Errors
///
/// `query` 缺失/空白，`max_results` 不是整数，或 `paths` 不是字符串数组时返回错误。
pub fn parse_memory_search_args(
    args: &Value,
) -> Result<(String, usize, Option<Vec<String>>), JiaClawError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'query'（非空字符串）".to_string()))?;

    let max_results = match args.get("max_results") {
        None => MEMORY_SEARCH_DEFAULT_MAX_RESULTS,
        Some(value) => {
            let raw = value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
            match raw {
                Some(n) => clamp_memory_search_max_results(n),
                None => {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'max_results' 必须是整数（将钳制到 1..=20）".to_string(),
                    ));
                }
            }
        }
    };

    let paths = match args.get("paths") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let raw = item.as_str().ok_or_else(|| {
                    JiaClawError::ToolExecution(
                        "参数 'paths' 必须是字符串数组（工作区相对路径）".to_string(),
                    )
                })?;
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'paths' 中的路径不能为空".to_string(),
                    ));
                }
                out.push(trimmed.to_string());
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'paths' 必须是字符串数组（工作区相对路径）".to_string(),
            ));
        }
    };

    Ok((query.to_string(), max_results, paths))
}

/// 一条记忆检索命中（相对路径、1-indexed 行号、行窗摘录）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemorySearchHit {
    /// 工作区相对路径（调用方传入或默认约定路径）
    pub path: String,
    /// 命中行号（从 1 起）
    pub line: usize,
    /// 命中行及其前后上下文
    pub excerpt: String,
}

/// 在文本中做大小写不敏感子串匹配，返回最多 `max_results` 条行窗。
#[must_use]
pub fn search_memory_windows(
    rel_path: &str,
    content: &str,
    query: &str,
    max_results: usize,
) -> Vec<MemorySearchHit> {
    if query.is_empty() || max_results == 0 {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    let lines: Vec<&str> = content.lines().collect();
    let mut hits = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !line.to_lowercase().contains(&needle) {
            continue;
        }
        let start = idx.saturating_sub(MEMORY_SEARCH_LINE_RADIUS);
        let end = (idx + MEMORY_SEARCH_LINE_RADIUS).min(lines.len().saturating_sub(1));
        let excerpt = lines[start..=end].join("\n");
        hits.push(MemorySearchHit {
            path: rel_path.to_string(),
            line: idx + 1,
            excerpt,
        });
        if hits.len() >= max_results {
            break;
        }
    }
    hits
}

fn unique_relative_paths(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

fn read_search_file_limited(path: &Path) -> Result<(String, bool), JiaClawError> {
    let file = std::fs::File::open(path).map_err(|e| {
        JiaClawError::ToolExecution(format!("无法读取文件 {}: {e}", path.display()))
    })?;
    let mut buf = Vec::new();
    let limit = u64::try_from(MEMORY_SEARCH_FILE_MAX_BYTES).unwrap_or(u64::MAX);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| {
            JiaClawError::ToolExecution(format!("无法读取文件 {}: {e}", path.display()))
        })?;

    let truncated = buf.len() > MEMORY_SEARCH_FILE_MAX_BYTES;
    if truncated {
        buf.truncate(MEMORY_SEARCH_FILE_MAX_BYTES);
        tracing::warn!(
            path = %path.display(),
            size_limit_bytes = MEMORY_SEARCH_FILE_MAX_BYTES,
            "memory_search 单文件过大，截断后检索"
        );
    }

    let text = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(err) => String::from_utf8_lossy(&err.into_bytes()).into_owned(),
    };
    if truncated {
        Ok((
            truncate_utf8(&text, MEMORY_SEARCH_FILE_MAX_BYTES).to_string(),
            true,
        ))
    } else {
        Ok((text, false))
    }
}

enum SearchTarget {
    Missing,
    NotFile,
    File(PathBuf),
}

fn open_search_target(workspace: &Path, configured: &str) -> Result<SearchTarget, JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, configured)?;
    if !path.exists() {
        return Ok(SearchTarget::Missing);
    }
    ensure_existing_within_workspace(workspace, &path)?;
    if path.is_file() {
        Ok(SearchTarget::File(path))
    } else {
        Ok(SearchTarget::NotFile)
    }
}

#[derive(Serialize)]
struct MemorySearchOutput {
    query: String,
    matches: Vec<MemorySearchHit>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

/// `memory_search` 工具：在工作区记忆类文件中按关键词检索片段。
pub struct MemorySearchTool {
    workspace_path: PathBuf,
    default_paths: Vec<String>,
}

impl MemorySearchTool {
    /// 创建工具；`default_paths` 为配置的 MEMORY / SOUL / USER 相对路径。
    #[must_use]
    pub fn new(
        workspace_path: &Path,
        default_paths: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let canonical_workspace = canonicalize_existing_or_clone(workspace_path);
        Self {
            workspace_path: canonical_workspace,
            default_paths: unique_relative_paths(
                default_paths
                    .into_iter()
                    .map(Into::into)
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty()),
            ),
        }
    }

    fn search_targets(&self, override_paths: Option<Vec<String>>) -> Vec<String> {
        match override_paths {
            Some(paths) => unique_relative_paths(paths),
            None => self.default_paths.clone(),
        }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "在工作区记忆类文件中按关键词检索相关片段（大小写不敏感子串 + 行窗）。默认扫描配置的 MEMORY / SOUL / USER；可选 paths 指定工作区相对路径。返回 {path, line, excerpt}。单文件超过 512KiB 截断。禁止路径穿越。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "检索关键词或子串（必填）"
                },
                "max_results": {
                    "type": "integer",
                    "description": "返回条数（可选，默认 5，钳制到 1..=20）",
                    "minimum": 1,
                    "maximum": 20,
                    "default": 5
                },
                "paths": {
                    "type": "array",
                    "description": "要扫描的工作区相对路径（可选；缺省为配置的 MEMORY / SOUL / USER）",
                    "items": { "type": "string" }
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let (query, max_results, override_paths) = parse_memory_search_args(&args)?;
        let targets = self.search_targets(override_paths);
        if targets.is_empty() {
            return Err(JiaClawError::ToolExecution(
                "没有可扫描的记忆文件路径".to_string(),
            ));
        }

        let mut matches = Vec::new();
        let mut warnings = Vec::new();

        for rel in targets {
            if matches.len() >= max_results {
                break;
            }
            match open_search_target(&self.workspace_path, &rel)? {
                SearchTarget::Missing => {
                    warnings.push(format!("{rel}: 文件不存在，已跳过"));
                }
                SearchTarget::NotFile => {
                    warnings.push(format!("{rel}: 不是文件，已跳过"));
                }
                SearchTarget::File(path) => {
                    let (content, truncated) = read_search_file_limited(&path)?;
                    if truncated {
                        warnings.push(format!(
                            "{rel}: 超过 {MEMORY_SEARCH_FILE_MAX_BYTES} 字节，已截断后检索"
                        ));
                    }
                    let remaining = max_results.saturating_sub(matches.len());
                    matches.extend(search_memory_windows(&rel, &content, &query, remaining));
                }
            }
        }

        let output = MemorySearchOutput {
            query,
            matches,
            warnings,
        };
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化检索结果失败: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::{DEFAULT_MEMORY_PATH, DEFAULT_SOUL_PATH, DEFAULT_USER_PATH};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_default_path() {
        let ws = unique_temp("jiaclaw_mem_resolve");
        let path = resolve_memory_path(&ws, DEFAULT_MEMORY_PATH).unwrap();
        assert_eq!(path, ws.canonicalize().unwrap().join("MEMORY.md"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn custom_relative_path_within_workspace() {
        let ws = unique_temp("jiaclaw_mem_custom");
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes/MEMORY.md"), "custom-path-fact").unwrap();
        let loaded = load_memory_for_prompt(&ws, "notes/MEMORY.md")
            .unwrap()
            .expect("content");
        assert!(loaded.contains("custom-path-fact"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_rejects_parent_dir() {
        let ws = unique_temp("jiaclaw_mem_parent");
        let err = resolve_memory_path(&ws, "../secret.md").unwrap_err();
        assert!(err.to_string().contains("穿越"));
        let err = resolve_memory_path(&ws, "foo/../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("穿越"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_rejects_absolute_path() {
        let ws = unique_temp("jiaclaw_mem_abs");
        let err = resolve_memory_path(&ws, "/etc/passwd").unwrap_err();
        assert!(err.to_string().contains("绝对路径"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_missing_file_is_none() {
        let ws = unique_temp("jiaclaw_mem_missing");
        let loaded = load_memory_for_prompt(&ws, DEFAULT_MEMORY_PATH).unwrap();
        assert!(loaded.is_none());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_empty_file_is_none() {
        let ws = unique_temp("jiaclaw_mem_empty");
        fs::write(ws.join("MEMORY.md"), "   \n\t\n").unwrap();
        let loaded = load_memory_for_prompt(&ws, DEFAULT_MEMORY_PATH).unwrap();
        assert!(loaded.is_none());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_returns_content_verbatim() {
        let ws = unique_temp("jiaclaw_mem_load");
        let body = "# Facts\n\n- prefers rust\n";
        fs::write(ws.join("MEMORY.md"), body).unwrap();
        let loaded = load_memory_for_prompt(&ws, DEFAULT_MEMORY_PATH)
            .unwrap()
            .expect("content");
        assert_eq!(loaded, body);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_truncates_oversize() {
        let ws = unique_temp("jiaclaw_mem_trunc");
        let mut body = "héllo".repeat(MEMORY_PROMPT_MAX_BYTES);
        body.push('中');
        fs::write(ws.join("MEMORY.md"), &body).unwrap();
        let loaded = load_memory_for_prompt(&ws, DEFAULT_MEMORY_PATH)
            .unwrap()
            .expect("truncated");
        assert!(loaded.len() <= MEMORY_PROMPT_MAX_BYTES);
        assert!(loaded.is_char_boundary(loaded.len()));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn append_creates_and_separates() {
        let ws = unique_temp("jiaclaw_mem_append");
        write_memory(&ws, DEFAULT_MEMORY_PATH, "first", false).unwrap();
        write_memory(&ws, DEFAULT_MEMORY_PATH, "second", false).unwrap();
        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(text, "first\n\nsecond");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn replace_overwrites() {
        let ws = unique_temp("jiaclaw_mem_replace");
        write_memory(&ws, DEFAULT_MEMORY_PATH, "old", false).unwrap();
        write_memory(&ws, DEFAULT_MEMORY_PATH, "new", true).unwrap();
        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(text, "new");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn write_rejects_path_traversal() {
        let ws = unique_temp("jiaclaw_mem_write_trav");
        let err = write_memory(&ws, "../evil.md", "nope", false).unwrap_err();
        assert!(err.to_string().contains("穿越") || err.to_string().contains("安全"));
        assert!(!ws.parent().unwrap().join("evil.md").exists());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn inspect_reports_size() {
        let ws = unique_temp("jiaclaw_mem_inspect");
        let missing = inspect_memory_file(&ws, DEFAULT_MEMORY_PATH).unwrap();
        assert!(!missing.exists);
        assert_eq!(missing.size_bytes, 0);

        fs::write(ws.join("MEMORY.md"), "abcd").unwrap();
        let present = inspect_memory_file(&ws, DEFAULT_MEMORY_PATH).unwrap();
        assert!(present.exists);
        assert_eq!(present.size_bytes, 4);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn tool_append_is_visible_on_disk() {
        let ws = unique_temp("jiaclaw_mem_tool");
        let tool = MemoryAppendTool::new(&ws, DEFAULT_MEMORY_PATH);
        assert_eq!(tool.name(), "memory_append");

        let result = tool
            .execute(serde_json::json!({"content": "- likes tea"}))
            .await
            .unwrap();
        assert!(result.contains("追加"));
        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert!(text.contains("- likes tea"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn tool_ignores_path_argument() {
        let ws = unique_temp("jiaclaw_mem_ignore_path");
        let tool = MemoryAppendTool::new(&ws, DEFAULT_MEMORY_PATH);
        tool.execute(serde_json::json!({
            "content": "safe",
            "path": "../evil.md"
        }))
        .await
        .unwrap();
        assert!(ws.join("MEMORY.md").exists());
        assert!(!ws.join("evil.md").exists());
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected_on_read() {
        let ws = unique_temp("jiaclaw_mem_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_mem_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("MEMORY.md")).unwrap();

        let result = load_memory_for_prompt(&ws, DEFAULT_MEMORY_PATH);
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    fn default_search_tool(ws: &Path) -> MemorySearchTool {
        MemorySearchTool::new(
            ws,
            [DEFAULT_MEMORY_PATH, DEFAULT_SOUL_PATH, DEFAULT_USER_PATH],
        )
    }

    #[test]
    fn parse_memory_search_args_requires_non_empty_query() {
        let err = parse_memory_search_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("query"), "{err}");

        let err = parse_memory_search_args(&serde_json::json!({"query": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("query"), "{err}");
    }

    #[test]
    fn parse_memory_search_args_defaults_and_clamps_max_results() {
        let (query, max_results, paths) =
            parse_memory_search_args(&serde_json::json!({"query": " rust "})).unwrap();
        assert_eq!(query, "rust");
        assert_eq!(max_results, MEMORY_SEARCH_DEFAULT_MAX_RESULTS);
        assert!(paths.is_none());

        let (_, max_results, _) =
            parse_memory_search_args(&serde_json::json!({"query": "q", "max_results": 1})).unwrap();
        assert_eq!(max_results, 1);

        let (_, max_results, _) =
            parse_memory_search_args(&serde_json::json!({"query": "q", "max_results": 0})).unwrap();
        assert_eq!(max_results, 1);

        let (_, max_results, _) =
            parse_memory_search_args(&serde_json::json!({"query": "q", "max_results": 99}))
                .unwrap();
        assert_eq!(max_results, MEMORY_SEARCH_MAX_RESULTS);

        let (_, _, paths) = parse_memory_search_args(&serde_json::json!({
            "query": "q",
            "paths": []
        }))
        .unwrap();
        assert!(paths.is_none());

        let err = parse_memory_search_args(&serde_json::json!({
            "query": "q",
            "max_results": "nope"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("max_results"), "{err}");
    }

    #[test]
    fn clamp_memory_search_max_results_bounds() {
        assert_eq!(clamp_memory_search_max_results(0), 1);
        assert_eq!(clamp_memory_search_max_results(5), 5);
        assert_eq!(clamp_memory_search_max_results(20), 20);
        assert_eq!(clamp_memory_search_max_results(21), 20);
    }

    #[test]
    fn search_memory_windows_is_case_insensitive_with_line_window() {
        let content = "alpha\nUser likes TEA\nbeta\n";
        let hits = search_memory_windows("MEMORY.md", content, "tea", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "MEMORY.md");
        assert_eq!(hits[0].line, 2);
        assert!(hits[0].excerpt.contains("User likes TEA"));
        assert!(hits[0].excerpt.contains("alpha"));
        assert!(hits[0].excerpt.contains("beta"));
    }

    #[tokio::test]
    async fn memory_search_hits_and_misses() {
        let ws = unique_temp("jiaclaw_mem_search_hit");
        fs::write(
            ws.join("MEMORY.md"),
            "# Memory\nUser prefers Rust tea.\nAnother line.\n",
        )
        .unwrap();
        fs::write(ws.join("SOUL.md"), "Be helpful.\n").unwrap();
        let tool = default_search_tool(&ws);
        assert_eq!(tool.name(), "memory_search");

        let hit = tool
            .execute(serde_json::json!({"query": "rust"}))
            .await
            .unwrap();
        assert!(hit.contains("MEMORY.md"), "{hit}");
        assert!(hit.contains("prefers Rust tea"), "{hit}");
        assert!(
            hit.contains("\"line\": 2") || hit.contains("\"line\":2"),
            "{hit}"
        );

        let miss = tool
            .execute(serde_json::json!({"query": "definitely-not-present-xyz"}))
            .await
            .unwrap();
        assert!(
            miss.contains("\"matches\": []") || miss.contains("\"matches\":[]"),
            "{miss}"
        );

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_search_defaults_to_scanning_memory() {
        let ws = unique_temp("jiaclaw_mem_search_default");
        fs::write(ws.join("MEMORY.md"), "stable-fact-UNIQUE_MEMORY_TOKEN\n").unwrap();
        fs::write(ws.join("SOUL.md"), "persona only\n").unwrap();
        fs::write(ws.join("USER.md"), "profile only\n").unwrap();
        let tool = default_search_tool(&ws);

        let result = tool
            .execute(serde_json::json!({"query": "unique_memory_token"}))
            .await
            .unwrap();
        assert!(result.contains("MEMORY.md"), "{result}");
        assert!(result.contains("UNIQUE_MEMORY_TOKEN"), "{result}");
        assert!(!result.contains("persona only"), "{result}");

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_search_rejects_path_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_mem_search_trav");
        fs::write(ws.join("MEMORY.md"), "inside\n").unwrap();
        let tool = default_search_tool(&ws);

        let err = tool
            .execute(serde_json::json!({
                "query": "secret",
                "paths": ["../secret.md"]
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({
                "query": "secret",
                "paths": ["/etc/passwd"]
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径") || err.contains("安全"), "{err}");

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_search_clamps_max_results_on_execute() {
        let ws = unique_temp("jiaclaw_mem_search_clamp");
        let mut body = String::new();
        for i in 0..30 {
            body.push_str(&format!("match-line-{i} needle-here\n"));
        }
        fs::write(ws.join("MEMORY.md"), body).unwrap();
        let tool = default_search_tool(&ws);

        let result = tool
            .execute(serde_json::json!({"query": "needle-here", "max_results": 99}))
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let matches = parsed
            .get("matches")
            .and_then(Value::as_array)
            .expect("matches array");
        assert_eq!(matches.len(), MEMORY_SEARCH_MAX_RESULTS);

        let result = tool
            .execute(serde_json::json!({"query": "needle-here", "max_results": 0}))
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let matches = parsed
            .get("matches")
            .and_then(Value::as_array)
            .expect("matches array");
        assert_eq!(matches.len(), 1);

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_search_truncates_oversize_file_and_warns() {
        let ws = unique_temp("jiaclaw_mem_search_trunc");
        let mut body = "NEEDLE-AT-START\n".to_string();
        body.push_str(&"x".repeat(MEMORY_SEARCH_FILE_MAX_BYTES));
        fs::write(ws.join("MEMORY.md"), body).unwrap();
        let tool = default_search_tool(&ws);

        let result = tool
            .execute(serde_json::json!({"query": "NEEDLE-AT-START"}))
            .await
            .unwrap();
        assert!(result.contains("NEEDLE-AT-START"), "{result}");
        assert!(result.contains("截断"), "{result}");

        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memory_search_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_mem_search_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_mem_search_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside-token").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("MEMORY.md")).unwrap();

        let tool = default_search_tool(&ws);
        let result = tool
            .execute(serde_json::json!({"query": "secret-outside-token"}))
            .await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_memory_write_args_requires_content_and_defaults_mode() {
        let err = parse_memory_write_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("content"), "{err}");

        let (content, mode) =
            parse_memory_write_args(&serde_json::json!({"content": "- likes tea"})).unwrap();
        assert_eq!(content, "- likes tea");
        assert_eq!(mode, MemoryWriteMode::Append);
        assert_eq!(MemoryWriteMode::Append.as_str(), "append");
        assert_eq!(MemoryWriteMode::Overwrite.as_str(), "overwrite");

        let (_, mode) = parse_memory_write_args(&serde_json::json!({
            "content": "x",
            "mode": "overwrite"
        }))
        .unwrap();
        assert_eq!(mode, MemoryWriteMode::Overwrite);

        let (_, mode) = parse_memory_write_args(&serde_json::json!({
            "content": "x",
            "mode": " APPEND "
        }))
        .unwrap();
        assert_eq!(mode, MemoryWriteMode::Append);

        let err = parse_memory_write_args(&serde_json::json!({
            "content": "x",
            "mode": "replace"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("mode"), "{err}");
    }

    #[tokio::test]
    async fn memory_write_appends_with_separator_and_reports_json() {
        let ws = unique_temp("jiaclaw_mem_write_append");
        let tool = MemoryWriteTool::new(&ws, DEFAULT_MEMORY_PATH);
        assert_eq!(tool.name(), "memory_write");

        let first = tool
            .execute(serde_json::json!({"content": "first"}))
            .await
            .unwrap();
        assert!(
            first.contains("\"mode\": \"append\"") || first.contains("\"mode\":\"append\""),
            "{first}"
        );
        assert!(first.contains("MEMORY.md"), "{first}");
        assert!(first.contains("bytes_written"), "{first}");

        let second = tool
            .execute(serde_json::json!({"content": "second", "mode": "append"}))
            .await
            .unwrap();
        assert!(second.contains("append"), "{second}");

        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(text, "first\n\nsecond");
        assert_eq!(text.len(), 13);

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_write_overwrite_replaces_file() {
        let ws = unique_temp("jiaclaw_mem_write_overwrite");
        let tool = MemoryWriteTool::new(&ws, DEFAULT_MEMORY_PATH);
        tool.execute(serde_json::json!({"content": "old"}))
            .await
            .unwrap();
        let result = tool
            .execute(serde_json::json!({"content": "new", "mode": "overwrite"}))
            .await
            .unwrap();
        assert!(
            result.contains("\"mode\": \"overwrite\"") || result.contains("\"mode\":\"overwrite\""),
            "{result}"
        );
        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(text, "new");

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_write_ignores_path_argument() {
        let ws = unique_temp("jiaclaw_mem_write_ignore_path");
        let tool = MemoryWriteTool::new(&ws, DEFAULT_MEMORY_PATH);
        tool.execute(serde_json::json!({
            "content": "safe",
            "path": "../evil.md",
            "section": "ignored"
        }))
        .await
        .unwrap();
        assert!(ws.join("MEMORY.md").exists());
        assert!(!ws.join("evil.md").exists());
        assert!(!ws.parent().unwrap().join("evil.md").exists());
        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(text, "safe");

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_write_rejects_configured_path_traversal() {
        let ws = unique_temp("jiaclaw_mem_write_trav");
        let tool = MemoryWriteTool::new(&ws, "../evil.md");
        let err = tool
            .execute(serde_json::json!({"content": "nope"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");
        assert!(!ws.parent().unwrap().join("evil.md").exists());

        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_write_rejects_oversize_and_does_not_write() {
        let ws = unique_temp("jiaclaw_mem_write_oversize");
        let tool = MemoryWriteTool::new(&ws, DEFAULT_MEMORY_PATH);
        fs::write(ws.join("MEMORY.md"), "keep-me").unwrap();

        let too_big = "x".repeat(MEMORY_WRITE_MAX_BYTES + 1);
        let err = tool
            .execute(serde_json::json!({"content": too_big, "mode": "overwrite"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("上限"), "{err}");
        assert!(err.contains(&MEMORY_WRITE_MAX_BYTES.to_string()), "{err}");
        let text = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(text, "keep-me");

        let almost = "y".repeat(MEMORY_WRITE_MAX_BYTES - 1);
        tool.execute(serde_json::json!({"content": almost, "mode": "overwrite"}))
            .await
            .unwrap();
        let err = tool
            .execute(serde_json::json!({"content": "zz", "mode": "append"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("上限"), "{err}");
        let after = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert_eq!(after.len(), MEMORY_WRITE_MAX_BYTES - 1);

        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memory_write_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_mem_write_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_mem_write_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("MEMORY.md")).unwrap();

        let tool = MemoryWriteTool::new(&ws, DEFAULT_MEMORY_PATH);
        let result = tool
            .execute(serde_json::json!({"content": "injected"}))
            .await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let outside_text = fs::read_to_string(&outside).unwrap();
        assert_eq!(outside_text, "secret-outside");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }
}
