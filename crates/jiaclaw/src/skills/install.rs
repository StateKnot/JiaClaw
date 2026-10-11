// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Operator-only local installation. One workspace owner spans capture,
//! durable intent, disabled policy, directory publication and the receipt.

use super::{
    invalid, lock, Skill, SkillDiscovery, SkillPolicyEntry, SkillSourcePin, SKILL_FILE_BYTES,
};
use crate::memory_io;
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, MetadataExt};
use jiaclaw_core::JiaClawError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

const JOURNAL: &str = ".jiaclaw-skill-installs";
const BUNDLE_BYTES: usize = 512 * 1024;
const OPERATIONS: usize = 64;
const INTENT_BYTES: usize = 4096;

mod remove;
pub use remove::{SkillRemoveState, SkillRemoveStatus, SkillRemover};

mod update;
pub use update::{SkillUpdateState, SkillUpdateStatus, SkillUpdater};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    version: u32,
    directory: String,
    source: SkillSourcePin,
    files: Vec<BundleFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BundleFile {
    path: String,
    text: String,
}

impl Bundle {
    fn file_text(&self) -> BTreeMap<&str, &str> {
        self.files
            .iter()
            .map(|file| (file.path.as_str(), file.text.as_str()))
            .collect()
    }
    fn proposed_policy(&self, original: &str) -> Result<String, JiaClawError> {
        let mut policy = lock::CatalogLock::parse(original)?.policy;
        if policy
            .skills
            .iter()
            .any(|entry| entry.directory == self.directory)
        {
            return Err(invalid("安装目标已登记；不允许替换"));
        }
        policy.skills.push(SkillPolicyEntry {
            directory: self.directory.clone(),
            enabled: false,
            source: self.source.clone(),
        });
        policy.skills.sort_by(|a, b| a.directory.cmp(&b.directory));
        let next = serde_json::to_string(&serde_json::json!({"version":2,"skills":policy.skills}))
            .map_err(|_| invalid("安装策略编码失败"))?
            + "\n";
        if next.len() > SKILL_FILE_BYTES {
            return Err(invalid("安装策略超过128KiB"));
        }
        lock::CatalogLock::parse(&next)?;
        Ok(next)
    }
    fn parse(text: &str) -> Result<Self, JiaClawError> {
        let bundle: Self =
            serde_json::from_str(text).map_err(|_| invalid("安装包 JSON/schema 无效"))?;
        if text.len() > BUNDLE_BYTES
            || bundle.version != 1
            || !lock::directory(&bundle.directory)
            || !lock::source(&bundle.source)
            || bundle.files.is_empty()
            || bundle.files.len() > 9
        {
            return Err(invalid(
                "安装要求 version=1、有效目录/来源、最多9份有界 UTF8 文件",
            ));
        }
        let files = bundle.file_text();
        if files.len() != bundle.files.len()
            || bundle
                .files
                .iter()
                .any(|file| file.text.len() > SKILL_FILE_BYTES)
        {
            return Err(invalid("安装文件重复或超过128KiB"));
        }
        let raw = files
            .get("SKILL.md")
            .ok_or_else(|| invalid("安装包缺少 SKILL.md"))?;
        let (skill, _, hash) = Skill::parse_raw(raw, Path::new(&bundle.directory))?;
        if hash != bundle.source.skill_sha256
            || files.len() != skill.resources.len() + 1
            || skill.resources.iter().any(|resource| {
                files
                    .get(resource.path.as_str())
                    .is_none_or(|text| digest(text) != resource.sha256)
            })
        {
            return Err(invalid("安装原始正文/全部声明资源摘要不一致，禁止额外文件"));
        }
        Ok(bundle)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    operation_id: String,
    bundle_sha256: String,
    original_policy_sha256: String,
    proposed_policy_sha256: String,
}

struct Captured {
    intent: Intent,
    intent_sha256: String,
    bundle: Bundle,
    original: String,
    proposed: String,
}

/// Actual disk observation under the original workspace writer. Completion is
/// a historical local publication receipt, never runtime activation or origin authentication.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillInstallStatus {
    /// Canonical caller-provided UUID, known before any filesystem write.
    pub operation_id: String,
    /// preparation_incomplete, prepared, policy_published, files_published or completed.
    pub state: String,
    /// Exact captured intent hash; required for an explicit finish request.
    pub intent_sha256: Option<String>,
    /// Captured new top-level directory, once a durable intent exists.
    pub directory: Option<String>,
    /// Installation never changes a running registry.
    pub runtime_applied: bool,
}

/// Installs a reviewed local JSON bundle into a new, disabled source declaration.
/// No downloads, updates, automatic recovery, model calls or runtime reloads.
pub struct SkillInstaller {
    workspace: PathBuf,
    lock_required: bool,
}

impl SkillInstaller {
    /// Use the same administrator workspace and required-lock policy as discovery.
    #[must_use]
    pub fn new(workspace: &Path, lock_required: bool) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            lock_required,
        }
    }

    /// Start exactly one new operation. A reused ID is rejected, even after completion.
    pub fn install(
        &self,
        operation_id: &str,
        input: &str,
        bundle_hash: &str,
        policy_hash: &str,
    ) -> Result<SkillInstallStatus, JiaClawError> {
        self.validate(operation_id)?;
        if !super::valid_hash(bundle_hash) || !super::valid_hash(policy_hash) {
            return Err(invalid("安装要求完整原始包与策略 SHA256"));
        }
        let input = memory_io::relative(input)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            // All preconditions and full candidate coverage precede journal creation.
            let raw =
                read_path(root, input, BUNDLE_BYTES)?.ok_or_else(|| invalid("安装输入不存在"))?;
            if digest(&raw) != bundle_hash {
                return Err(invalid("安装输入已改变，未开始"));
            }
            let bundle = Bundle::parse(&raw)?;
            let original = required(root, "skills.lock.json", SKILL_FILE_BYTES)?;
            if digest(&original) != policy_hash {
                return Err(invalid("技能策略已改变，未开始"));
            }
            let proposed = self.propose(&original, &bundle)?;
            remove::ensure_settled(root, operation_id)?;
            update::ensure_settled(root, operation_id)?;
            let skills = directory(root, "skills", false, false)?;
            if let Some(skills) = &skills {
                absent(skills, &bundle.directory)?;
            }
            let journal = directory(root, JOURNAL, false, true)?;
            if let Some(journal) = &journal {
                let ids = operation_ids(journal)?;
                if ids.len() >= OPERATIONS {
                    return Err(invalid("安装记录已达64份；停止并由操作员审阅归档"));
                }
                for id in ids {
                    if id == operation_id {
                        return Err(invalid("原操作ID已存在；先查询，禁止自动重发"));
                    }
                    let op = directory(journal, &id, false, true)?
                        .ok_or_else(|| invalid("安装记录缺失"))?;
                    let captured = capture(&op, &id)?;
                    if captured
                        .as_ref()
                        .is_none_or(|c| completed(&op, c).ok().flatten().is_none())
                    {
                        return Err(invalid("存在未结算或不可核对的安装；先查询原操作ID"));
                    }
                }
            }
            let journal =
                directory(root, JOURNAL, true, true)?.ok_or_else(|| invalid("安装记录无法创建"))?;
            absent(&journal, operation_id)?;
            let op = directory(&journal, operation_id, true, true)?
                .ok_or_else(|| invalid("安装记录无法创建"))?;
            // Snapshots are durably published before the intent; an interrupted
            // capture has no namespace publication and requires operator inspection.
            write_new(&op, "bundle.json", &raw)?;
            write_new(&op, "original-policy.json", &original)?;
            write_new(&op, "proposed-policy.json", &proposed)?;
            let intent = Intent {
                version: 1,
                operation_id: operation_id.into(),
                bundle_sha256: digest(&raw),
                original_policy_sha256: digest(&original),
                proposed_policy_sha256: digest(&proposed),
            };
            let intent = serde_json::to_string(&intent).map_err(|_| invalid("安装意图编码失败"))?;
            write_new(&op, "intent.json", &intent)?;
            memory_io::sync_dir(&journal)?;
            memory_io::sync_dir(root)?;
            let captured = capture(&op, operation_id)?.ok_or_else(|| invalid("安装意图未保存"))?;
            self.finish_owned(root, &op, &captured)
        })
    }

    /// Read only. Acquires the original nonqueueing writer for a stable compound observation.
    pub fn inspect(&self, operation_id: &str) -> Result<SkillInstallStatus, JiaClawError> {
        self.validate(operation_id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = operation(root, operation_id)?;
            match capture(&op, operation_id)? {
                Some(captured) => observe(root, &op, &captured),
                None => Ok(SkillInstallStatus {
                    operation_id: operation_id.into(),
                    state: "preparation_incomplete".into(),
                    intent_sha256: None,
                    directory: None,
                    runtime_applied: false,
                }),
            }
        })
    }

    /// Explicitly finish only the saved operation, after inspecting its intent hash.
    /// The input path is never reread and no new operation identity is minted.
    pub fn finish(
        &self,
        operation_id: &str,
        intent_hash: &str,
    ) -> Result<SkillInstallStatus, JiaClawError> {
        self.validate(operation_id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = operation(root, operation_id)?;
            let captured = capture(&op, operation_id)?
                .ok_or_else(|| invalid("安装捕获未完成，保留原记录并由操作员审阅；不得自动重发"))?;
            if captured.intent_sha256 != intent_hash {
                return Err(invalid("安装意图已改变，拒绝完成"));
            }
            self.finish_owned(root, &op, &captured)
        })
    }

    fn validate(&self, operation_id: &str) -> Result<(), JiaClawError> {
        if !self.lock_required || !valid_id(operation_id) {
            return Err(invalid("安装要求显式必需来源锁与非零规范UUID操作ID"));
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err(invalid("安装原子目录发布仅支持Linux/macOS"));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Ok(())
    }

    fn propose(&self, original: &str, bundle: &Bundle) -> Result<String, JiaClawError> {
        let next = bundle.proposed_policy(original)?;
        let candidate = lock::CatalogLock::parse(&next)?;
        SkillDiscovery::new(&self.workspace)
            .with_lock_required(true)
            .scan_locked(true, Some(candidate))?;
        Ok(next)
    }

    fn finish_owned(
        &self,
        root: &Dir,
        op: &Dir,
        captured: &Captured,
    ) -> Result<SkillInstallStatus, JiaClawError> {
        if let Some(receipt) = completed(op, captured)? {
            return Ok(receipt);
        }
        let state = observe(root, op, captured)?;
        // Revalidate all saved bytes and full current enabled coverage before any
        // recovery write. An operator edit after a crash is a conflict, not an overwrite.
        let candidate = lock::CatalogLock::parse(&captured.proposed)?;
        SkillDiscovery::new(&self.workspace)
            .with_lock_required(true)
            .scan_locked(true, Some(candidate))?;
        clear_snapshot_temporaries(op)?;
        if state.state != "files_published" {
            stage(op, &captured.bundle)?;
            if state.state == "prepared" {
                memory_io::publish(
                    root,
                    OsStr::new("skills.lock.json"),
                    captured.proposed.as_bytes(),
                    true,
                )?;
            }
            let skills =
                directory(root, "skills", true, false)?.ok_or_else(|| invalid("技能目录不存在"))?;
            absent(&skills, &captured.bundle.directory)?;
            memory_io::rename_entry(
                op,
                OsStr::new("stage"),
                &skills,
                OsStr::new(&captured.bundle.directory),
                false,
            )
            .map_err(invalid)?;
            memory_io::sync_dir(op)?;
            memory_io::sync_dir(&skills)?;
            memory_io::sync_dir(root)?;
        }
        // Both parents are synced even on a recovered post-rename operation.
        let skills =
            directory(root, "skills", false, false)?.ok_or_else(|| invalid("技能目录不存在"))?;
        let installed = directory(&skills, &captured.bundle.directory, false, true)?
            .ok_or_else(|| invalid("安装目录未发布"))?;
        verify_tree(&installed, &captured.bundle, false)?;
        memory_io::sync_dir(&installed)?;
        memory_io::sync_dir(&skills)?;
        memory_io::sync_dir(op)?;
        memory_io::sync_dir(root)?;
        let receipt = status(captured, "completed");
        write_new(
            op,
            "receipt.json",
            &serde_json::to_string(&receipt).map_err(invalid)?,
        )?;
        Ok(receipt)
    }
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == id)
}

fn private(meta: &cap_std::fs::Metadata, dir: bool) -> Result<(), JiaClawError> {
    if meta.file_type().is_symlink()
        || (dir && !meta.is_dir())
        || (!dir && (!meta.is_file() || meta.nlink() != 1))
        || meta.mode() & 0o777 != if dir { 0o700 } else { 0o600 }
    {
        return Err(invalid("安装状态必须普通私有目录0700/单链接文件0600"));
    }
    Ok(())
}

fn directory(
    root: &Dir,
    name: &str,
    create: bool,
    is_private: bool,
) -> Result<Option<Dir>, JiaClawError> {
    match root.symlink_metadata(name) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(invalid("安装目录必须普通目录，禁止链接"))
        }
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if !create {
                return Ok(None);
            }
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            root.create_dir_with(name, &builder).map_err(invalid)?;
            memory_io::sync_dir(root)?;
        }
        Err(error) => return Err(invalid(error)),
    }
    let opened = root
        .open_with(name, &memory_io::read_options(true))
        .map_err(invalid)?;
    let dir = Dir::from_std_file(opened.into_std());
    if is_private {
        private(&dir.dir_metadata().map_err(invalid)?, true)?;
    }
    Ok(Some(dir))
}

fn absent(root: &Dir, name: &str) -> Result<(), JiaClawError> {
    match root.symlink_metadata(name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(invalid("安装目标已存在或不可安全核对")),
    }
}

fn read_path(root: &Dir, path: &Path, limit: usize) -> Result<Option<String>, JiaClawError> {
    let Some(dir) = memory_io::parent(root, path, false)? else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .ok_or_else(|| invalid("安装输入路径无效"))?;
    memory_io::open_regular(&dir, name)?
        .map(|file| memory_io::read_open(file, limit, false).map(|file| file.text))
        .transpose()
}

fn required(root: &Dir, name: &str, limit: usize) -> Result<String, JiaClawError> {
    read_path(root, Path::new(name), limit)?.ok_or_else(|| invalid("安装捕获文件缺失"))
}

fn snapshot(root: &Dir, name: &str, limit: usize) -> Result<Option<String>, JiaClawError> {
    let file = memory_io::open_regular(root, OsStr::new(name))?;
    file.map(|file| {
        private(&file.metadata().map_err(invalid)?, false)?;
        memory_io::read_open(file, limit, false).map(|file| file.text)
    })
    .transpose()
}

fn write_new(root: &Dir, name: &str, text: &str) -> Result<(), JiaClawError> {
    if !memory_io::publish_noreplace(root, OsStr::new(name), text.as_bytes())? {
        return Err(invalid("安装捕获/收据已存在；不得重新发布"));
    }
    Ok(())
}

fn operation_ids(journal: &Dir) -> Result<Vec<String>, JiaClawError> {
    let mut ids = Vec::new();
    for entry in journal.entries().map_err(invalid)? {
        let id = entry
            .map_err(invalid)?
            .file_name()
            .into_string()
            .map_err(|_| invalid("安装记录ID无效"))?;
        if !valid_id(&id) || ids.len() >= OPERATIONS {
            return Err(invalid("安装记录目录无效或超过64份"));
        }
        // Opening through the original capability rejects linked/special directories.
        directory(journal, &id, false, true)?.ok_or_else(|| invalid("安装记录不存在"))?;
        ids.push(id);
    }
    Ok(ids)
}

fn operation(root: &Dir, id: &str) -> Result<Dir, JiaClawError> {
    let journal =
        directory(root, JOURNAL, false, true)?.ok_or_else(|| invalid("安装记录不存在"))?;
    operation_ids(&journal)?;
    directory(&journal, id, false, true)?.ok_or_else(|| invalid("原操作ID不存在"))
}

fn capture(op: &Dir, id: &str) -> Result<Option<Captured>, JiaClawError> {
    // Interrupted private publication may leave temporary leaves. Their count
    // and size are bounded; unknown entries fail closed, never get deleted.
    let mut count = 0;
    for entry in op.entries().map_err(invalid)? {
        count += 1;
        let name = entry
            .map_err(invalid)?
            .file_name()
            .into_string()
            .map_err(|_| invalid("安装状态名称无效"))?;
        let known = matches!(
            name.as_str(),
            "bundle.json"
                | "original-policy.json"
                | "proposed-policy.json"
                | "intent.json"
                | "receipt.json"
                | "stage"
        );
        if count > 32 || (!known && !name.strip_prefix(".jiaclaw-memory-").is_some_and(valid_id)) {
            return Err(invalid("安装状态包含未知条目或超过32份"));
        }
        if name == "stage" {
            let stage =
                directory(op, &name, false, true)?.ok_or_else(|| invalid("安装暂存目录消失"))?;
            tree_files(&stage, "", &mut BTreeSet::new(), &mut 0)?;
        } else if known {
            let limit = match name.as_str() {
                "bundle.json" => BUNDLE_BYTES,
                "original-policy.json" | "proposed-policy.json" => SKILL_FILE_BYTES,
                _ => INTENT_BYTES,
            };
            snapshot(op, &name, limit)?.ok_or_else(|| invalid("安装捕获文件消失"))?;
        } else {
            let meta = op.symlink_metadata(&name).map_err(invalid)?;
            private(&meta, false)?;
            if meta.len() > BUNDLE_BYTES as u64 {
                return Err(invalid("安装状态暂存超过512KiB"));
            }
        }
    }
    let Some(raw_intent) = snapshot(op, "intent.json", INTENT_BYTES)? else {
        return Ok(None);
    };
    let intent: Intent =
        serde_json::from_str(&raw_intent).map_err(|_| invalid("安装意图schema无效"))?;
    if intent.version != 1 || intent.operation_id != id {
        return Err(invalid("安装原操作身份不一致"));
    }
    let raw_bundle =
        snapshot(op, "bundle.json", BUNDLE_BYTES)?.ok_or_else(|| invalid("原安装包缺失"))?;
    let original = snapshot(op, "original-policy.json", SKILL_FILE_BYTES)?
        .ok_or_else(|| invalid("原安装策略缺失"))?;
    let proposed = snapshot(op, "proposed-policy.json", SKILL_FILE_BYTES)?
        .ok_or_else(|| invalid("候选安装策略缺失"))?;
    if digest(&raw_bundle) != intent.bundle_sha256
        || digest(&original) != intent.original_policy_sha256
        || digest(&proposed) != intent.proposed_policy_sha256
    {
        return Err(invalid("安装捕获摘要不一致，保留并人工核对"));
    }
    let bundle = Bundle::parse(&raw_bundle)?;
    if proposed != bundle.proposed_policy(&original)? {
        return Err(invalid("安装捕获不是原策略的单一停用新增"));
    }
    if let Some(stage) = directory(op, "stage", false, true)? {
        // Inspect all conflicts before finish can remove any original temporary
        // evidence. Status uses the same read-only validation.
        verify_tree(&stage, &bundle, true)?;
    }
    Ok(Some(Captured {
        intent,
        intent_sha256: digest(&raw_intent),
        bundle,
        original,
        proposed,
    }))
}

fn clear_snapshot_temporaries(op: &Dir) -> Result<(), JiaClawError> {
    // capture has already bounded/validated these single-link private leaves;
    // no namespace publication depends on an unpublished temporary snapshot.
    for entry in op.entries().map_err(invalid)? {
        let name = entry.map_err(invalid)?.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.strip_prefix(".jiaclaw-memory-").is_some_and(valid_id))
        {
            op.remove_file(&name).map_err(invalid)?;
        }
    }
    memory_io::sync_dir(op)?;
    Ok(())
}

fn status(captured: &Captured, state: &str) -> SkillInstallStatus {
    SkillInstallStatus {
        operation_id: captured.intent.operation_id.clone(),
        state: state.into(),
        intent_sha256: Some(captured.intent_sha256.clone()),
        directory: Some(captured.bundle.directory.clone()),
        runtime_applied: false,
    }
}

fn completed(op: &Dir, captured: &Captured) -> Result<Option<SkillInstallStatus>, JiaClawError> {
    let Some(text) = snapshot(op, "receipt.json", INTENT_BYTES)? else {
        return Ok(None);
    };
    let receipt: SkillInstallStatus =
        serde_json::from_str(&text).map_err(|_| invalid("安装完成收据schema无效"))?;
    if serde_json::to_value(&receipt).map_err(invalid)?
        != serde_json::to_value(status(captured, "completed")).map_err(invalid)?
    {
        return Err(invalid("安装完成收据身份不一致"));
    }
    Ok(Some(receipt))
}

fn observe(root: &Dir, op: &Dir, captured: &Captured) -> Result<SkillInstallStatus, JiaClawError> {
    if let Some(receipt) = completed(op, captured)? {
        return Ok(receipt);
    }
    let current = required(root, "skills.lock.json", SKILL_FILE_BYTES)?;
    let installed = directory(root, "skills", false, false)?
        .map(|skills| directory(&skills, &captured.bundle.directory, false, true))
        .transpose()?
        .flatten();
    let state = if current == captured.original && installed.is_none() {
        "prepared"
    } else if current == captured.proposed {
        if let Some(installed) = installed {
            verify_tree(&installed, &captured.bundle, false)?;
            "files_published"
        } else {
            "policy_published"
        }
    } else {
        return Err(invalid("原安装操作与当前策略/目标冲突；禁止覆盖或重发"));
    };
    Ok(status(captured, state))
}

// Read-only bounded inventory. Recovery must validate the complete partial
// package before removing any known interrupted-publication temporary.
fn tree_files(
    root: &Dir,
    prefix: &str,
    files: &mut BTreeSet<String>,
    entries: &mut usize,
) -> Result<(), JiaClawError> {
    for entry in root.entries().map_err(invalid)? {
        *entries += 1;
        if *entries > 160 {
            return Err(invalid("安装目录扫描超过160项"));
        }
        let name = entry
            .map_err(invalid)?
            .file_name()
            .into_string()
            .map_err(|_| invalid("安装文件名不是UTF8"))?;
        let meta = root.symlink_metadata(&name).map_err(invalid)?;
        let path = format!("{prefix}{name}");
        if meta.is_dir() && !meta.file_type().is_symlink() {
            let dir =
                directory(root, &name, false, true)?.ok_or_else(|| invalid("安装目录消失"))?;
            files.insert(format!("{path}/"));
            tree_files(&dir, &format!("{path}/"), files, entries)?;
        } else {
            private(&meta, false)?;
            let file = memory_io::open_regular(root, OsStr::new(&name))?
                .ok_or_else(|| invalid("安装叶消失"))?;
            let opened = file.metadata().map_err(invalid)?;
            private(&opened, false)?;
            if opened.len() > SKILL_FILE_BYTES as u64 {
                return Err(invalid("安装叶超过128KiB"));
            }
            files.insert(path);
        }
    }
    Ok(())
}

fn expected_files(bundle: &Bundle) -> BTreeSet<String> {
    let mut expected = BTreeSet::new();
    for file in &bundle.files {
        expected.insert(file.path.clone());
        let mut parent = Path::new(&file.path).parent();
        while let Some(path) = parent.filter(|p| !p.as_os_str().is_empty()) {
            expected.insert(format!("{}/", path.to_string_lossy()));
            parent = path.parent();
        }
    }
    expected
}

fn verify_tree(root: &Dir, bundle: &Bundle, partial: bool) -> Result<Vec<String>, JiaClawError> {
    let mut actual = BTreeSet::new();
    tree_files(root, "", &mut actual, &mut 0)?;
    let expected = expected_files(bundle);
    let temporaries: Vec<_> = actual
        .iter()
        .filter(|path| {
            partial
                && !expected.contains(*path)
                && Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.strip_prefix(".jiaclaw-memory-").is_some_and(valid_id))
                && !path.ends_with('/')
        })
        .cloned()
        .collect();
    for path in &temporaries {
        actual.remove(path);
    }
    if (partial && !actual.is_subset(&expected)) || (!partial && actual != expected) {
        return Err(invalid("安装目录包含未捕获文件或缺失文件"));
    }
    for file in &bundle.files {
        if let Some(text) = read_path(root, Path::new(&file.path), SKILL_FILE_BYTES)? {
            if text != file.text {
                return Err(invalid("安装目录原始内容不一致"));
            }
        } else if !partial {
            return Err(invalid("安装目录内容缺失"));
        }
    }
    Ok(temporaries)
}

fn stage(op: &Dir, bundle: &Bundle) -> Result<(), JiaClawError> {
    let stage = directory(op, "stage", true, true)?.ok_or_else(|| invalid("安装暂存目录不存在"))?;
    let temporaries = verify_tree(&stage, bundle, true)?;
    for path in temporaries {
        let path = Path::new(&path);
        let parent =
            memory_io::parent(&stage, path, false)?.ok_or_else(|| invalid("安装暂存父目录消失"))?;
        parent
            .remove_file(
                path.file_name()
                    .ok_or_else(|| invalid("安装暂存叶名称无效"))?,
            )
            .map_err(invalid)?;
        memory_io::sync_dir(&parent)?;
    }
    for file in &bundle.files {
        let mut dir = stage.try_clone().map_err(invalid)?;
        let path = Path::new(&file.path);
        if let Some(parent) = path.parent() {
            for name in parent.components() {
                let name = name
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| invalid("安装路径不是UTF8"))?;
                dir = directory(&dir, name, true, true)?
                    .ok_or_else(|| invalid("安装暂存父目录不存在"))?;
            }
        }
        let name = path.file_name().ok_or_else(|| invalid("安装叶名称无效"))?;
        if memory_io::open_regular(&dir, name)?.is_none() {
            memory_io::publish_noreplace(&dir, name, file.text.as_bytes())?;
        }
    }
    verify_tree(&stage, bundle, false)?;
    memory_io::sync_dir(&stage)?;
    memory_io::sync_dir(op)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, String, String) {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("skills")).unwrap();
        let original = "{\"version\":2,\"skills\":[]}\n".to_string();
        fs::write(workspace.path().join("skills.lock.json"), &original).unwrap();
        let body = "---\nname: reviewed\ndescription: original\n---\nbody";
        let bundle = serde_json::json!({"version":1,"directory":"reviewed","source":{
            "repository":"https://github.com/example/skills","revision":"a".repeat(40),"skill_sha256":digest(body)},
            "files":[{"path":"SKILL.md","text":body}]}).to_string();
        fs::write(workspace.path().join("reviewed.json"), &bundle).unwrap();
        (workspace, original, bundle)
    }

    #[test]
    fn local_install_requires_activation_and_keeps_original_identity() {
        let (workspace, original, bundle) = fixture();
        let installer = SkillInstaller::new(workspace.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        let receipt = installer
            .install(&id, "reviewed.json", &digest(&bundle), &digest(&original))
            .unwrap();
        assert_eq!(receipt.state, "completed");
        assert!(!receipt.runtime_applied);
        assert!(SkillDiscovery::new(workspace.path())
            .with_lock_required(true)
            .discover_strict()
            .unwrap()
            .is_empty());
        assert!(installer
            .install(&id, "reviewed.json", &digest(&bundle), &digest(&original))
            .is_err());
        let saved_policy = fs::read(workspace.path().join("skills.lock.json")).unwrap();
        assert!(installer.finish(&id, &"0".repeat(64)).is_err());
        assert_eq!(
            installer
                .finish(&id, receipt.intent_sha256.as_ref().unwrap())
                .unwrap()
                .state,
            "completed"
        );
        assert_eq!(
            fs::read(workspace.path().join("skills.lock.json")).unwrap(),
            saved_policy
        );
    }

    #[test]
    fn local_install_inspection_rejects_unsafe_incomplete_known_entries() {
        let (workspace, original, bundle) = fixture();
        let installer = SkillInstaller::new(workspace.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        installer
            .install(&id, "reviewed.json", &digest(&bundle), &digest(&original))
            .unwrap();
        let op = workspace.path().join(JOURNAL).join(&id);
        fs::remove_file(op.join("intent.json")).unwrap();
        fs::remove_file(op.join("bundle.json")).unwrap();
        std::os::unix::fs::symlink(
            workspace.path().join("reviewed.json"),
            op.join("bundle.json"),
        )
        .unwrap();
        assert!(installer.inspect(&id).is_err());
        assert!(op
            .join("bundle.json")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn local_install_conflict_preserves_original_temporary_evidence() {
        let (workspace, original, bundle) = fixture();
        let installer = SkillInstaller::new(workspace.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        let receipt = installer
            .install(&id, "reviewed.json", &digest(&bundle), &digest(&original))
            .unwrap();
        let op = workspace.path().join(JOURNAL).join(&id);
        fs::remove_file(op.join("receipt.json")).unwrap();
        fs::rename(workspace.path().join("skills/reviewed"), op.join("stage")).unwrap();
        fs::write(workspace.path().join("skills.lock.json"), &original).unwrap();
        let temporary = op
            .join("stage")
            .join(format!(".jiaclaw-memory-{}", uuid::Uuid::new_v4()));
        let foreign = op.join("stage/foreign.txt");
        for leaf in [&temporary, &foreign] {
            fs::write(leaf, "retained evidence").unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(leaf, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(installer
            .finish(&id, receipt.intent_sha256.as_ref().unwrap())
            .is_err());
        assert!(installer.inspect(&id).is_err());
        assert_eq!(fs::read_to_string(&temporary).unwrap(), "retained evidence");
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "retained evidence");
        assert_eq!(
            fs::read_to_string(workspace.path().join("skills.lock.json")).unwrap(),
            original
        );
        fs::remove_file(foreign).unwrap();
        assert_eq!(
            installer
                .finish(&id, receipt.intent_sha256.as_ref().unwrap())
                .unwrap()
                .state,
            "completed"
        );
    }

    #[test]
    fn local_install_cas_and_capture_integrity_fail_closed() {
        let (workspace, original, bundle) = fixture();
        let installer = SkillInstaller::new(workspace.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        assert!(installer
            .install(&id, "reviewed.json", &digest(&bundle), &"0".repeat(64))
            .is_err());
        assert!(!workspace.path().join(JOURNAL).exists());
        installer
            .install(&id, "reviewed.json", &digest(&bundle), &digest(&original))
            .unwrap();
        // Administrator mutation of the saved package must never be accepted as
        // authenticated completion, even if a previous receipt is present.
        fs::write(
            workspace.path().join(JOURNAL).join(&id).join("bundle.json"),
            "{}",
        )
        .unwrap();
        assert!(installer.inspect(&id).is_err());
        assert!(installer.finish(&id, &"0".repeat(64)).is_err());
    }

    #[test]
    fn local_install_missing_resource_or_invalid_id_never_creates_state() {
        let (workspace, original, bundle) = fixture();
        let installer = SkillInstaller::new(workspace.path(), true);
        assert!(installer
            .install(
                "../bad",
                "reviewed.json",
                &digest(&bundle),
                &digest(&original)
            )
            .is_err());
        let mut value: serde_json::Value = serde_json::from_str(&bundle).unwrap();
        value["files"][0]["text"] = serde_json::json!(format!("---\nname: reviewed\ndescription: original\njiaclaw_resources:\n- path: references/ref.md\n  sha256: {}\n---\nbody", digest("ref")));
        value["source"]["skill_sha256"] =
            serde_json::json!(digest(value["files"][0]["text"].as_str().unwrap()));
        let raw = value.to_string();
        fs::write(workspace.path().join("reviewed.json"), &raw).unwrap();
        assert!(installer
            .install(
                &uuid::Uuid::new_v4().to_string(),
                "reviewed.json",
                &digest(&raw),
                &digest(&original)
            )
            .is_err());
        assert!(!workspace.path().join(JOURNAL).exists());
    }
}
