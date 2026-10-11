// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicit local version replacement. Ledger semantics remain separate from
//! installation; capability file and package validation are shared.

use super::*;

const JOURNAL: &str = ".jiaclaw-skill-updates";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Persisted publication stages observed under the original workspace writer.
pub enum SkillUpdateState {
    /// No complete intent; operator inspection is required.
    PreparationIncomplete,
    /// Both versions captured; original policy and directory still published.
    Prepared,
    /// New disabled source policy published; original directory still current.
    PolicyPublished,
    /// Directory exchange committed; original version retained in private stage.
    FilesPublished,
    /// Historical completion receipt exists.
    Completed,
}

/// A saved operation observation, not a statement about the running registry.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillUpdateStatus {
    /// Canonical caller UUID saved before starting the operation.
    pub operation_id: String,
    /// Actual observed stage, or historical completion.
    pub state: SkillUpdateState,
    /// Original intent digest needed by an explicit finish.
    pub intent_sha256: Option<String>,
    /// Captured directory, once the intent is complete.
    pub directory: Option<String>,
    /// Always false: publication never reloads a running registry.
    pub runtime_applied: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectoryIdentity {
    dev: u64,
    ino: u64,
}

impl DirectoryIdentity {
    pub(super) fn read(dir: &Dir) -> Result<Self, JiaClawError> {
        let meta = dir.dir_metadata().map_err(invalid)?;
        private(&meta, true)?;
        Ok(Self {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateIntent {
    version: u32,
    operation_id: String,
    old_bundle_sha256: String,
    new_bundle_sha256: String,
    original_policy_sha256: String,
    proposed_policy_sha256: String,
    old_directory: DirectoryIdentity,
    new_directory: DirectoryIdentity,
}

struct UpdateCapture {
    intent: UpdateIntent,
    intent_sha256: String,
    old: Bundle,
    new: Bundle,
    original: String,
    proposed: String,
}

/// One operator-owned update. Only disabled, source-pinned private packages
/// are replaced; old files remain in the original operation's private stage.
pub struct SkillUpdater {
    workspace: PathBuf,
    lock_required: bool,
}

impl SkillUpdater {
    /// Use the same operator workspace and required-lock policy as discovery.
    #[must_use]
    pub fn new(workspace: &Path, lock_required: bool) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            lock_required,
        }
    }

    /// Start once with reviewed input, policy and current raw body conditions.
    pub fn update(
        &self,
        operation_id: &str,
        input: &str,
        bundle_hash: &str,
        policy_hash: &str,
        current_skill_hash: &str,
    ) -> Result<SkillUpdateStatus, JiaClawError> {
        self.validate(operation_id)?;
        if ![bundle_hash, policy_hash, current_skill_hash]
            .iter()
            .all(|hash| super::super::valid_hash(hash))
        {
            return Err(invalid("更新要求已审阅包/策略/原正文完整SHA256"));
        }
        let input = memory_io::relative(input)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let raw =
                read_path(root, input, BUNDLE_BYTES)?.ok_or_else(|| invalid("更新输入不存在"))?;
            if digest(&raw) != bundle_hash {
                return Err(invalid("更新输入已改变，未开始"));
            }
            let new = Bundle::parse(&raw)?;
            let original = required(root, "skills.lock.json", SKILL_FILE_BYTES)?;
            if digest(&original) != policy_hash {
                return Err(invalid("更新策略已改变，未开始"));
            }
            let policy = lock::CatalogLock::parse(&original)?;
            let entry = policy
                .policy
                .skills
                .iter()
                .find(|entry| entry.directory == new.directory && !entry.enabled)
                .ok_or_else(|| invalid("更新要求已登记且明确停用目标；先独立停用并核对服务"))?;
            if entry.source.skill_sha256 != current_skill_hash {
                return Err(invalid("原正文未被审阅或已改变"));
            }
            let skills = directory(root, "skills", false, false)?
                .ok_or_else(|| invalid("更新技能根不存在"))?;
            let installed = directory(&skills, &new.directory, false, true)?
                .ok_or_else(|| invalid("更新目标不存在"))?;
            let old = capture_old(&installed, entry)?;
            let proposed = proposed_policy(&original, &old, &new)?;
            self.scan(&proposed)?;
            super::remove::ensure_settled(root, operation_id)?;
            super::fetch::ensure_settled(root, operation_id)?;
            ensure_installs_settled(root, operation_id)?;
            ensure_settled(root, operation_id)?;
            let journal =
                directory(root, JOURNAL, true, true)?.ok_or_else(|| invalid("更新记录无法创建"))?;
            if operation_ids(&journal)?.len() >= OPERATIONS {
                return Err(invalid("更新记录已达64份；由操作员审阅归档"));
            }
            absent(&journal, operation_id)?;
            let op = directory(&journal, operation_id, true, true)?
                .ok_or_else(|| invalid("更新记录无法创建"))?;
            write_new(
                &op,
                "old-bundle.json",
                &serde_json::to_string(&old).map_err(invalid)?,
            )?;
            write_new(&op, "new-bundle.json", &raw)?;
            write_new(&op, "original-policy.json", &original)?;
            write_new(&op, "proposed-policy.json", &proposed)?;
            // A complete private replacement precedes the durable intent.
            // A pre-intent interruption is retained for manual inspection.
            stage(&op, &new)?;
            let staged =
                directory(&op, "stage", false, true)?.ok_or_else(|| invalid("更新暂存目录缺失"))?;
            let intent = UpdateIntent {
                version: 1,
                operation_id: operation_id.into(),
                old_bundle_sha256: digest(&required(&op, "old-bundle.json", BUNDLE_BYTES)?),
                new_bundle_sha256: digest(&raw),
                original_policy_sha256: digest(&original),
                proposed_policy_sha256: digest(&proposed),
                old_directory: DirectoryIdentity::read(&installed)?,
                new_directory: DirectoryIdentity::read(&staged)?,
            };
            if intent.old_directory.dev != intent.new_directory.dev {
                return Err(invalid(
                    "跨文件系统更新被拒绝；保留准备记录，不复制或删除目录",
                ));
            }
            write_new(
                &op,
                "intent.json",
                &serde_json::to_string(&intent).map_err(invalid)?,
            )?;
            memory_io::sync_dir(&journal)?;
            memory_io::sync_dir(root)?;
            let captured =
                capture_update(&op, operation_id)?.ok_or_else(|| invalid("更新意图缺失"))?;
            self.finish_owned(root, &op, &captured)
        })
    }

    /// A stable, read-only observation; never creates missing identities.
    pub fn inspect(&self, operation_id: &str) -> Result<SkillUpdateStatus, JiaClawError> {
        self.validate(operation_id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = update_operation(root, operation_id)?;
            match capture_update(&op, operation_id)? {
                Some(captured) => observe_update(root, &op, &captured),
                None => Ok(SkillUpdateStatus {
                    operation_id: operation_id.into(),
                    state: SkillUpdateState::PreparationIncomplete,
                    intent_sha256: None,
                    directory: None,
                    runtime_applied: false,
                }),
            }
        })
    }

    /// Finish only this captured identity. Directory identity makes a second
    /// EXCHANGE impossible, including old and new packages with equal bytes.
    pub fn finish(
        &self,
        operation_id: &str,
        intent_hash: &str,
    ) -> Result<SkillUpdateStatus, JiaClawError> {
        self.validate(operation_id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = update_operation(root, operation_id)?;
            let captured = capture_update(&op, operation_id)?
                .ok_or_else(|| invalid("更新捕获未完成；保留原记录并人工审阅"))?;
            if captured.intent_sha256 != intent_hash {
                return Err(invalid("更新原意图摘要不一致"));
            }
            self.finish_owned(root, &op, &captured)
        })
    }

    fn validate(&self, id: &str) -> Result<(), JiaClawError> {
        if !self.lock_required || !valid_id(id) {
            return Err(invalid("更新要求必需来源锁与非零规范UUID"));
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err(invalid("更新原子交换仅支持Linux/macOS"));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Ok(())
    }

    fn scan(&self, proposed: &str) -> Result<(), JiaClawError> {
        SkillDiscovery::new(&self.workspace)
            .with_lock_required(true)
            .scan_locked(true, Some(lock::CatalogLock::parse(proposed)?))?;
        Ok(())
    }

    fn finish_owned(
        &self,
        root: &Dir,
        op: &Dir,
        c: &UpdateCapture,
    ) -> Result<SkillUpdateStatus, JiaClawError> {
        if let Some(receipt) = update_completed(op, c)? {
            return Ok(receipt);
        }
        let state = observe_update(root, op, c)?.state;
        self.scan(&c.proposed)?;
        // All snapshots, original namespace and complete stage were checked.
        // No cleanup is necessary: an incomplete preparation cannot finish.
        if state == SkillUpdateState::Prepared {
            memory_io::publish(
                root,
                OsStr::new("skills.lock.json"),
                c.proposed.as_bytes(),
                true,
            )?;
        }
        let skills =
            directory(root, "skills", false, false)?.ok_or_else(|| invalid("更新技能根缺失"))?;
        if state != SkillUpdateState::FilesPublished {
            exchange(
                op,
                OsStr::new("stage"),
                &skills,
                OsStr::new(&c.new.directory),
            )
            .map_err(invalid)?;
        }
        let current = directory(&skills, &c.new.directory, false, true)?
            .ok_or_else(|| invalid("更新当前目录缺失"))?;
        let backup =
            directory(op, "stage", false, true)?.ok_or_else(|| invalid("更新旧版本缺失"))?;
        verify_identity(&current, &c.intent.new_directory)?;
        verify_identity(&backup, &c.intent.old_directory)?;
        verify_tree(&current, &c.new, false)?;
        verify_tree(&backup, &c.old, false)?;
        for dir in [&current, &backup, &skills, op, root] {
            memory_io::sync_dir(dir)?;
        }
        let receipt = update_status(c, SkillUpdateState::Completed);
        write_new(
            op,
            "receipt.json",
            &serde_json::to_string(&receipt).map_err(invalid)?,
        )?;
        Ok(receipt)
    }
}

fn exchange(from: &Dir, name: &OsStr, to: &Dir, target: &OsStr) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        rustix::fs::renameat_with(from, name, to, target, rustix::fs::RenameFlags::EXCHANGE)
            .map_err(io::Error::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (from, name, to, target);
        Err(io::Error::new(io::ErrorKind::Unsupported, "原子交换不支持"))
    }
}

pub(super) fn capture_old(dir: &Dir, entry: &SkillPolicyEntry) -> Result<Bundle, JiaClawError> {
    let body = required(dir, "SKILL.md", SKILL_FILE_BYTES)?;
    let (skill, _, _) = Skill::parse_raw(&body, Path::new(&entry.directory))?;
    let mut files = vec![BundleFile {
        path: "SKILL.md".into(),
        text: body,
    }];
    for resource in skill.resources {
        files.push(BundleFile {
            text: required(dir, &resource.path, SKILL_FILE_BYTES)?,
            path: resource.path,
        });
    }
    let old = Bundle {
        version: 1,
        directory: entry.directory.clone(),
        source: entry.source.clone(),
        files,
    };
    let raw = serde_json::to_string(&old).map_err(invalid)?;
    let old = Bundle::parse(&raw)?;
    verify_tree(dir, &old, false)?;
    Ok(old)
}

fn proposed_policy(original: &str, old: &Bundle, new: &Bundle) -> Result<String, JiaClawError> {
    if old.directory != new.directory {
        return Err(invalid("更新捕获目标不一致"));
    }
    let mut policy = lock::CatalogLock::parse(original)?.policy;
    let entry = policy
        .skills
        .iter_mut()
        .find(|entry| entry.directory == old.directory && !entry.enabled)
        .ok_or_else(|| invalid("更新原策略不是已登记停用目标"))?;
    if serde_json::to_value(&entry.source).map_err(invalid)?
        != serde_json::to_value(&old.source).map_err(invalid)?
    {
        return Err(invalid("更新原来源不一致"));
    }
    if serde_json::to_value(&old.source).map_err(invalid)?
        == serde_json::to_value(&new.source).map_err(invalid)?
        && old.file_text() == new.file_text()
    {
        return Err(invalid("更新内容/来源未改变，不发布"));
    }
    entry.source = new.source.clone();
    let next = serde_json::to_string(&serde_json::json!({"version":2,"skills":policy.skills}))
        .map_err(invalid)?
        + "\n";
    if next.len() > SKILL_FILE_BYTES {
        return Err(invalid("更新策略超过128KiB"));
    }
    lock::CatalogLock::parse(&next)?;
    Ok(next)
}

fn update_operation(root: &Dir, id: &str) -> Result<Dir, JiaClawError> {
    let journal =
        directory(root, JOURNAL, false, true)?.ok_or_else(|| invalid("更新记录不存在"))?;
    operation_ids(&journal)?;
    directory(&journal, id, false, true)?.ok_or_else(|| invalid("原更新身份不存在"))
}

fn capture_update(op: &Dir, id: &str) -> Result<Option<UpdateCapture>, JiaClawError> {
    let mut count = 0;
    for entry in op.entries().map_err(invalid)? {
        count += 1;
        let name = entry
            .map_err(invalid)?
            .file_name()
            .into_string()
            .map_err(|_| invalid("更新条目名称无效"))?;
        if count > 32 {
            return Err(invalid("更新记录超过32项"));
        }
        let limit = match name.as_str() {
            "old-bundle.json" | "new-bundle.json" => BUNDLE_BYTES,
            "original-policy.json" | "proposed-policy.json" => SKILL_FILE_BYTES,
            "intent.json" | "receipt.json" => INTENT_BYTES,
            "stage" => {
                let dir =
                    directory(op, "stage", false, true)?.ok_or_else(|| invalid("更新stage缺失"))?;
                tree_files(&dir, "", &mut BTreeSet::new(), &mut 0)?;
                continue;
            }
            _ if name.strip_prefix(".jiaclaw-memory-").is_some_and(valid_id) => BUNDLE_BYTES,
            _ => return Err(invalid("更新记录有未知条目")),
        };
        snapshot(op, &name, limit)?.ok_or_else(|| invalid("更新记录消失"))?;
    }
    let Some(raw_intent) = snapshot(op, "intent.json", INTENT_BYTES)? else {
        return Ok(None);
    };
    let intent: UpdateIntent =
        serde_json::from_str(&raw_intent).map_err(|_| invalid("更新意图schema无效"))?;
    if intent.version != 1
        || intent.operation_id != id
        || intent.old_directory == intent.new_directory
    {
        return Err(invalid("更新意图身份无效"));
    }
    let old_raw =
        snapshot(op, "old-bundle.json", BUNDLE_BYTES)?.ok_or_else(|| invalid("原更新包缺失"))?;
    let new_raw =
        snapshot(op, "new-bundle.json", BUNDLE_BYTES)?.ok_or_else(|| invalid("新更新包缺失"))?;
    let original = snapshot(op, "original-policy.json", SKILL_FILE_BYTES)?
        .ok_or_else(|| invalid("原更新策略缺失"))?;
    let proposed = snapshot(op, "proposed-policy.json", SKILL_FILE_BYTES)?
        .ok_or_else(|| invalid("新更新策略缺失"))?;
    if digest(&old_raw) != intent.old_bundle_sha256
        || digest(&new_raw) != intent.new_bundle_sha256
        || digest(&original) != intent.original_policy_sha256
        || digest(&proposed) != intent.proposed_policy_sha256
    {
        return Err(invalid("更新原捕获摘要不一致"));
    }
    let old = Bundle::parse(&old_raw)?;
    let new = Bundle::parse(&new_raw)?;
    if proposed_policy(&original, &old, &new)? != proposed {
        return Err(invalid("更新候选不是单一停用来源替换"));
    }
    let stage = directory(op, "stage", false, true)?.ok_or_else(|| invalid("更新stage缺失"))?;
    let identity = DirectoryIdentity::read(&stage)?;
    if identity == intent.new_directory {
        verify_tree(&stage, &new, false)?;
    } else if identity == intent.old_directory {
        verify_tree(&stage, &old, false)?;
    } else {
        return Err(invalid("更新stage目录身份改变"));
    }
    Ok(Some(UpdateCapture {
        intent,
        intent_sha256: digest(&raw_intent),
        old,
        new,
        original,
        proposed,
    }))
}

pub(super) fn verify_identity(dir: &Dir, expected: &DirectoryIdentity) -> Result<(), JiaClawError> {
    if DirectoryIdentity::read(dir)? != *expected {
        return Err(invalid("更新目录dev/ino身份冲突"));
    }
    Ok(())
}

fn update_status(c: &UpdateCapture, state: SkillUpdateState) -> SkillUpdateStatus {
    SkillUpdateStatus {
        operation_id: c.intent.operation_id.clone(),
        state,
        intent_sha256: Some(c.intent_sha256.clone()),
        directory: Some(c.new.directory.clone()),
        runtime_applied: false,
    }
}

fn update_completed(
    op: &Dir,
    c: &UpdateCapture,
) -> Result<Option<SkillUpdateStatus>, JiaClawError> {
    let Some(raw) = snapshot(op, "receipt.json", INTENT_BYTES)? else {
        return Ok(None);
    };
    let receipt: SkillUpdateStatus =
        serde_json::from_str(&raw).map_err(|_| invalid("更新收据schema无效"))?;
    if receipt != update_status(c, SkillUpdateState::Completed) {
        return Err(invalid("更新收据身份不一致"));
    }
    // Historical receipt can outlive current path edits, but never the saved
    // backup identity/content or original captured operation.
    let backup = directory(op, "stage", false, true)?.ok_or_else(|| invalid("旧版本备份缺失"))?;
    verify_identity(&backup, &c.intent.old_directory)?;
    Ok(Some(receipt))
}

fn observe_update(
    root: &Dir,
    op: &Dir,
    c: &UpdateCapture,
) -> Result<SkillUpdateStatus, JiaClawError> {
    if let Some(receipt) = update_completed(op, c)? {
        return Ok(receipt);
    }
    let policy = required(root, "skills.lock.json", SKILL_FILE_BYTES)?;
    let skills = directory(root, "skills", false, false)?.ok_or_else(|| invalid("更新根缺失"))?;
    let installed = directory(&skills, &c.new.directory, false, true)?
        .ok_or_else(|| invalid("更新目标缺失"))?;
    let stage = directory(op, "stage", false, true)?.ok_or_else(|| invalid("更新stage缺失"))?;
    let current_id = DirectoryIdentity::read(&installed)?;
    let stage_id = DirectoryIdentity::read(&stage)?;
    let state = if current_id == c.intent.old_directory && stage_id == c.intent.new_directory {
        verify_tree(&installed, &c.old, false)?;
        if policy == c.original {
            SkillUpdateState::Prepared
        } else if policy == c.proposed {
            SkillUpdateState::PolicyPublished
        } else {
            return Err(invalid("更新策略冲突，保留证据"));
        }
    } else if current_id == c.intent.new_directory
        && stage_id == c.intent.old_directory
        && policy == c.proposed
    {
        verify_tree(&installed, &c.new, false)?;
        SkillUpdateState::FilesPublished
    } else {
        return Err(invalid("更新目录/策略冲突，禁止重复交换或覆盖"));
    };
    Ok(update_status(c, state))
}

pub(super) fn ensure_settled(root: &Dir, new_id: &str) -> Result<(), JiaClawError> {
    if let Some(journal) = directory(root, JOURNAL, false, true)? {
        for id in operation_ids(&journal)? {
            if id == new_id {
                return Err(invalid("原UUID已有更新记录；禁止启动新安装或更新"));
            }
            let op =
                directory(&journal, &id, false, true)?.ok_or_else(|| invalid("更新记录缺失"))?;
            let c =
                capture_update(&op, &id)?.ok_or_else(|| invalid("有未完成捕获更新；先人工审阅"))?;
            if update_completed(&op, &c)?.is_none() {
                return Err(invalid("有未结算更新；先核对原身份"));
            }
        }
    }
    Ok(())
}

pub(super) fn ensure_installs_settled(root: &Dir, new_id: &str) -> Result<(), JiaClawError> {
    if let Some(journal) = directory(root, super::JOURNAL, false, true)? {
        for id in operation_ids(&journal)? {
            if id == new_id {
                return Err(invalid("原UUID已有安装记录；禁止启动新更新"));
            }
            let op =
                directory(&journal, &id, false, true)?.ok_or_else(|| invalid("安装记录缺失"))?;
            let c = capture(&op, &id)?.ok_or_else(|| invalid("有未完成捕获安装；先人工审阅"))?;
            if completed(&op, &c)?.is_none() {
                return Err(invalid("有未结算安装；先核对原身份"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(same_body: bool) -> (tempfile::TempDir, String, String, String) {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("skills")).unwrap();
        let empty = "{\"version\":2,\"skills\":[]}\n";
        fs::write(workspace.path().join("skills.lock.json"), empty).unwrap();
        let old_body = "---\nname: reviewed\ndescription: before\n---\nold body\n";
        let raw = serde_json::json!({"version":1,"directory":"reviewed","source":{
            "repository":"https://github.com/example/skills","revision":"a".repeat(40),"skill_sha256":digest(old_body)},
            "files":[{"path":"SKILL.md","text":old_body}]}).to_string();
        fs::write(workspace.path().join("install.json"), raw.as_bytes()).unwrap();
        SkillInstaller::new(workspace.path(), true)
            .install(
                &uuid::Uuid::new_v4().to_string(),
                "install.json",
                &digest(&raw),
                &digest(empty),
            )
            .unwrap();
        let policy = fs::read_to_string(workspace.path().join("skills.lock.json")).unwrap();
        let new_body = if same_body {
            old_body
        } else {
            "---\nname: reviewed\ndescription: after\n---\nnew body\n"
        };
        let new = serde_json::json!({"version":1,"directory":"reviewed","source":{
            "repository":"https://github.com/example/skills","revision":"b".repeat(40),"skill_sha256":digest(new_body)},
            "files":[{"path":"SKILL.md","text":new_body}]}).to_string();
        fs::write(workspace.path().join("update.json"), &new).unwrap();
        (workspace, policy, new, old_body.into())
    }

    #[test]
    fn local_update_retains_old_version_and_never_repeats_exchange() {
        for same_body in [false, true] {
            let (workspace, policy, bundle, old) = fixture(same_body);
            let current_path = workspace.path().join("skills/reviewed");
            let old_cap =
                Dir::open_ambient_dir(&current_path, cap_std::ambient_authority()).unwrap();
            let old_id = DirectoryIdentity::read(&old_cap).unwrap();
            let updater = SkillUpdater::new(workspace.path(), true);
            let id = uuid::Uuid::new_v4().to_string();
            let receipt = updater
                .update(
                    &id,
                    "update.json",
                    &digest(&bundle),
                    &digest(&policy),
                    &digest(&old),
                )
                .unwrap();
            assert_eq!(receipt.state, SkillUpdateState::Completed);
            assert!(!receipt.runtime_applied);
            let new_cap =
                Dir::open_ambient_dir(&current_path, cap_std::ambient_authority()).unwrap();
            let new_id = DirectoryIdentity::read(&new_cap).unwrap();
            assert!(old_id != new_id);
            assert_eq!(old_cap.read_to_string("SKILL.md").unwrap(), old);
            let backup = workspace
                .path()
                .join(JOURNAL)
                .join(&id)
                .join("stage/SKILL.md");
            assert_eq!(fs::read_to_string(backup).unwrap(), old);
            assert!(SkillDiscovery::new(workspace.path())
                .with_lock_required(true)
                .discover_strict()
                .unwrap()
                .is_empty());
            assert_eq!(updater.inspect(&id).unwrap(), receipt);
            assert!(updater.finish(&id, &"0".repeat(64)).is_err());
            assert_eq!(
                updater
                    .finish(&id, receipt.intent_sha256.as_ref().unwrap())
                    .unwrap(),
                receipt
            );
            assert_eq!(
                DirectoryIdentity::read(&new_cap).unwrap(),
                DirectoryIdentity::read(
                    &Dir::open_ambient_dir(&current_path, cap_std::ambient_authority()).unwrap()
                )
                .unwrap()
            );
            assert!(updater
                .update(
                    &id,
                    "update.json",
                    &digest(&bundle),
                    &digest(&policy),
                    &digest(&old)
                )
                .is_err());
        }
    }

    #[test]
    fn local_update_rejects_active_drifted_or_unreviewed_packages_before_intent() {
        let (workspace, original, bundle, old) = fixture(false);
        let updater = SkillUpdater::new(workspace.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        assert!(updater
            .update(
                &id,
                "update.json",
                &"0".repeat(64),
                &digest(&original),
                &digest(&old)
            )
            .is_err());
        let mut policy: serde_json::Value = serde_json::from_str(&original).unwrap();
        policy["skills"][0]["enabled"] = true.into();
        let enabled = policy.to_string();
        fs::write(workspace.path().join("skills.lock.json"), &enabled).unwrap();
        assert!(updater
            .update(
                &id,
                "update.json",
                &digest(&bundle),
                &digest(&enabled),
                &digest(&old)
            )
            .is_err());
        fs::write(workspace.path().join("skills.lock.json"), &original).unwrap();
        fs::write(workspace.path().join("skills/reviewed/SKILL.md"), "drift").unwrap();
        assert!(updater
            .update(
                &id,
                "update.json",
                &digest(&bundle),
                &digest(&original),
                &digest(&old)
            )
            .is_err());
        assert!(!workspace.path().join(JOURNAL).exists());
        assert_eq!(
            fs::read_to_string(workspace.path().join("skills.lock.json")).unwrap(),
            original
        );
    }

    #[test]
    fn installation_and_update_cannot_reuse_each_others_uuid() {
        let (workspace, policy, bundle, old) = fixture(false);
        let installed_id = fs::read_dir(workspace.path().join(super::super::JOURNAL))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        let updater = SkillUpdater::new(workspace.path(), true);
        assert!(updater
            .update(
                &installed_id,
                "update.json",
                &digest(&bundle),
                &digest(&policy),
                &digest(&old)
            )
            .is_err());
        assert!(!workspace.path().join(JOURNAL).exists());
        let id = uuid::Uuid::new_v4().to_string();
        updater
            .update(
                &id,
                "update.json",
                &digest(&bundle),
                &digest(&policy),
                &digest(&old),
            )
            .unwrap();
        let current = fs::read_to_string(workspace.path().join("skills.lock.json")).unwrap();
        let mut next: serde_json::Value = serde_json::from_str(&bundle).unwrap();
        next["directory"] = "another".into();
        let next = next.to_string();
        fs::write(workspace.path().join("another.json"), &next).unwrap();
        assert!(SkillInstaller::new(workspace.path(), true)
            .install(&id, "another.json", &digest(&next), &digest(&current))
            .is_err());
        assert!(!workspace.path().join("skills/another").exists());
        assert_eq!(
            fs::read_to_string(workspace.path().join("skills.lock.json")).unwrap(),
            current
        );
    }

    #[test]
    fn incomplete_update_blocks_new_install_and_rejects_unsafe_status() {
        let (workspace, policy, bundle, old) = fixture(false);
        let updater = SkillUpdater::new(workspace.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        memory_io::with_workspace_writer(workspace.path(), |root| {
            let journal = directory(root, JOURNAL, true, true)?.unwrap();
            directory(&journal, &id, true, true)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            updater.inspect(&id).unwrap().state,
            SkillUpdateState::PreparationIncomplete
        );
        assert!(updater
            .update(
                &uuid::Uuid::new_v4().to_string(),
                "update.json",
                &digest(&bundle),
                &digest(&policy),
                &digest(&old)
            )
            .is_err());
        let mut next: serde_json::Value = serde_json::from_str(&bundle).unwrap();
        next["directory"] = "another".into();
        let next = next.to_string();
        fs::write(workspace.path().join("another.json"), &next).unwrap();
        assert!(SkillInstaller::new(workspace.path(), true)
            .install(
                &uuid::Uuid::new_v4().to_string(),
                "another.json",
                &digest(&next),
                &digest(&policy)
            )
            .is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                workspace.path().join("update.json"),
                workspace
                    .path()
                    .join(JOURNAL)
                    .join(&id)
                    .join("new-bundle.json"),
            )
            .unwrap();
            assert!(updater.inspect(&id).is_err());
        }
        assert_eq!(
            fs::read_to_string(workspace.path().join("skills.lock.json")).unwrap(),
            policy
        );
    }
}
