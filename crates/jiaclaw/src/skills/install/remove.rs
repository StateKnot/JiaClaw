// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicit retirement under one workspace writer. Captured files and the
//! original directory remain private evidence; publication never reloads.

use super::*;
use update::{capture_old, verify_identity, DirectoryIdentity};

const JOURNAL: &str = ".jiaclaw-skill-removals";

/// Observed publication stage, not runtime activation or a rollback promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRemoveState {
    /// No complete intent; preserve the record for manual inspection.
    PreparationIncomplete,
    /// Original disabled policy and directory remain published.
    Prepared,
    /// Original directory retired; original disabled declaration still present.
    FilesRetired,
    /// Directory retired and declaration removed; receipt may be pending.
    PolicyPublished,
    /// Historical completion receipt, not current path integrity.
    Completed,
}

/// Saved original identity and read-only stage observation.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillRemoveStatus {
    /// Canonical nonzero caller UUID known before the operation starts.
    pub operation_id: String,
    /// Actual disk stage or historical completion.
    pub state: SkillRemoveState,
    /// Exact saved intent digest required by an explicit finish.
    pub intent_sha256: Option<String>,
    /// Original directory once capture is complete.
    pub directory: Option<String>,
    /// Always false: a separate authenticated reload is required.
    pub runtime_applied: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveIntent {
    version: u32,
    operation_id: String,
    bundle_sha256: String,
    original_policy_sha256: String,
    proposed_policy_sha256: String,
    directory: DirectoryIdentity,
}

struct RemoveCapture {
    intent: RemoveIntent,
    intent_sha256: String,
    bundle: Bundle,
    original: String,
    proposed: String,
}

/// Operator-only retirement of a reviewed, registered, disabled private package.
pub struct SkillRemover {
    workspace: PathBuf,
    lock_required: bool,
}

impl SkillRemover {
    /// Use the administrator workspace and its required source-lock policy.
    #[must_use]
    pub fn new(workspace: &Path, lock_required: bool) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            lock_required,
        }
    }

    /// Start once with exact reviewed policy and current raw body hashes.
    pub fn uninstall(
        &self,
        operation_id: &str,
        target: &str,
        policy_hash: &str,
        current_skill_hash: &str,
    ) -> Result<SkillRemoveStatus, JiaClawError> {
        self.validate(operation_id)?;
        if !lock::directory(target)
            || ![policy_hash, current_skill_hash]
                .iter()
                .all(|hash| super::super::valid_hash(hash))
        {
            return Err(invalid("卸载要求有效登记目录与完整策略/原正文SHA256"));
        }
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let original = required(root, "skills.lock.json", SKILL_FILE_BYTES)?;
            if digest(&original) != policy_hash {
                return Err(invalid("卸载策略已改变，未开始"));
            }
            let policy = lock::CatalogLock::parse(&original)?;
            let entry = policy
                .policy
                .skills
                .iter()
                .find(|entry| entry.directory == target && !entry.enabled)
                .ok_or_else(|| invalid("卸载要求已登记且明确停用目标；先独立停用并核对服务"))?;
            if entry.source.skill_sha256 != current_skill_hash {
                return Err(invalid("卸载原正文未被审阅或已改变"));
            }
            let skills = directory(root, "skills", false, false)?
                .ok_or_else(|| invalid("卸载技能根缺失"))?;
            let installed = directory(&skills, target, false, true)?
                .ok_or_else(|| invalid("卸载目标不存在"))?;
            let bundle = capture_old(&installed, entry)?;
            let proposed = proposed_policy(&original, &bundle)?;
            self.scan(&original)?;
            super::fetch::ensure_settled(root, operation_id)?;
            update::ensure_installs_settled(root, operation_id)?;
            update::ensure_settled(root, operation_id)?;
            ensure_settled(root, operation_id)?;
            let journal =
                directory(root, JOURNAL, true, true)?.ok_or_else(|| invalid("卸载记录无法创建"))?;
            if operation_ids(&journal)?.len() >= OPERATIONS {
                return Err(invalid("卸载记录已达64份；由操作员审阅归档"));
            }
            absent(&journal, operation_id)?;
            let op = directory(&journal, operation_id, true, true)?
                .ok_or_else(|| invalid("卸载记录无法创建"))?;
            let raw_bundle = serde_json::to_string(&bundle).map_err(invalid)?;
            write_new(&op, "bundle.json", &raw_bundle)?;
            write_new(&op, "original-policy.json", &original)?;
            write_new(&op, "proposed-policy.json", &proposed)?;
            let intent = RemoveIntent {
                version: 1,
                operation_id: operation_id.into(),
                bundle_sha256: digest(&raw_bundle),
                original_policy_sha256: digest(&original),
                proposed_policy_sha256: digest(&proposed),
                directory: DirectoryIdentity::read(&installed)?,
            };
            write_new(
                &op,
                "intent.json",
                &serde_json::to_string(&intent).map_err(invalid)?,
            )?;
            memory_io::sync_dir(&journal)?;
            memory_io::sync_dir(root)?;
            let c = capture_remove(&op, operation_id)?.ok_or_else(|| invalid("卸载意图缺失"))?;
            self.finish_owned(root, &op, &c)
        })
    }

    /// Read-only observation; never invents or restarts a missing identity.
    pub fn inspect(&self, operation_id: &str) -> Result<SkillRemoveStatus, JiaClawError> {
        self.validate(operation_id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = operation_remove(root, operation_id)?;
            match capture_remove(&op, operation_id)? {
                Some(c) => observe_remove(root, &op, &c),
                None => Ok(SkillRemoveStatus {
                    operation_id: operation_id.into(),
                    state: SkillRemoveState::PreparationIncomplete,
                    intent_sha256: None,
                    directory: None,
                    runtime_applied: false,
                }),
            }
        })
    }

    /// Finish only saved capture after inspection, never re-read caller inputs.
    pub fn finish(
        &self,
        operation_id: &str,
        intent_hash: &str,
    ) -> Result<SkillRemoveStatus, JiaClawError> {
        self.validate(operation_id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = operation_remove(root, operation_id)?;
            let c = capture_remove(&op, operation_id)?
                .ok_or_else(|| invalid("卸载捕获未完成；保留原记录并人工审阅"))?;
            if c.intent_sha256 != intent_hash {
                return Err(invalid("卸载原意图摘要不一致"));
            }
            self.finish_owned(root, &op, &c)
        })
    }

    fn validate(&self, id: &str) -> Result<(), JiaClawError> {
        if !self.lock_required || !valid_id(id) {
            return Err(invalid("卸载要求必需来源锁与非零规范UUID"));
        }
        Ok(())
    }

    fn scan(&self, policy: &str) -> Result<(), JiaClawError> {
        SkillDiscovery::new(&self.workspace)
            .with_lock_required(true)
            .scan_locked(true, Some(lock::CatalogLock::parse(policy)?))?;
        Ok(())
    }

    fn finish_owned(
        &self,
        root: &Dir,
        op: &Dir,
        c: &RemoveCapture,
    ) -> Result<SkillRemoveStatus, JiaClawError> {
        if let Some(receipt) = remove_completed(op, c)? {
            return Ok(receipt);
        }
        let state = observe_remove(root, op, c)?.state;
        let skills =
            directory(root, "skills", false, false)?.ok_or_else(|| invalid("卸载技能根缺失"))?;
        if state == SkillRemoveState::Prepared {
            self.scan(&c.original)?;
            // Retire before removing the declaration. A concurrent strict reload
            // accepts a missing disabled directory while retaining other skills.
            memory_io::rename_entry(
                &skills,
                OsStr::new(&c.bundle.directory),
                op,
                OsStr::new("retired"),
                false,
            )
            .map_err(invalid)?;
        }
        let retired =
            directory(op, "retired", false, true)?.ok_or_else(|| invalid("卸载退休目录缺失"))?;
        verify_identity(&retired, &c.intent.directory)?;
        verify_tree(&retired, &c.bundle, false)?;
        for dir in [&retired, &skills, op, root] {
            memory_io::sync_dir(dir)?;
        }
        self.scan(&c.proposed)?;
        if state != SkillRemoveState::PolicyPublished {
            memory_io::publish(
                root,
                OsStr::new("skills.lock.json"),
                c.proposed.as_bytes(),
                true,
            )?;
        }
        for dir in [&skills, op, root] {
            memory_io::sync_dir(dir)?;
        }
        let receipt = remove_status(c, SkillRemoveState::Completed);
        write_new(
            op,
            "receipt.json",
            &serde_json::to_string(&receipt).map_err(invalid)?,
        )?;
        Ok(receipt)
    }
}

fn proposed_policy(original: &str, bundle: &Bundle) -> Result<String, JiaClawError> {
    let mut policy = lock::CatalogLock::parse(original)?.policy;
    let entry = policy
        .skills
        .iter()
        .find(|entry| entry.directory == bundle.directory && !entry.enabled)
        .ok_or_else(|| invalid("卸载原策略不是已登记停用目标"))?;
    if serde_json::to_value(&entry.source).map_err(invalid)?
        != serde_json::to_value(&bundle.source).map_err(invalid)?
    {
        return Err(invalid("卸载原来源不一致"));
    }
    policy
        .skills
        .retain(|entry| entry.directory != bundle.directory);
    let next = serde_json::to_string(&serde_json::json!({"version":2,"skills":policy.skills}))
        .map_err(invalid)?
        + "\n";
    lock::CatalogLock::parse(&next)?;
    Ok(next)
}

fn operation_remove(root: &Dir, id: &str) -> Result<Dir, JiaClawError> {
    let journal =
        directory(root, JOURNAL, false, true)?.ok_or_else(|| invalid("卸载记录不存在"))?;
    operation_ids(&journal)?;
    directory(&journal, id, false, true)?.ok_or_else(|| invalid("原卸载身份不存在"))
}

fn capture_remove(op: &Dir, id: &str) -> Result<Option<RemoveCapture>, JiaClawError> {
    let mut count = 0;
    for entry in op.entries().map_err(invalid)? {
        count += 1;
        if count > 32 {
            return Err(invalid("卸载记录超过32项"));
        }
        let name = entry
            .map_err(invalid)?
            .file_name()
            .into_string()
            .map_err(|_| invalid("卸载条目名称无效"))?;
        let limit = match name.as_str() {
            "bundle.json" => BUNDLE_BYTES,
            "original-policy.json" | "proposed-policy.json" => SKILL_FILE_BYTES,
            "intent.json" | "receipt.json" => INTENT_BYTES,
            "retired" => {
                let dir = directory(op, "retired", false, true)?
                    .ok_or_else(|| invalid("卸载退休目录缺失"))?;
                tree_files(&dir, "", &mut BTreeSet::new(), &mut 0)?;
                continue;
            }
            _ if name.strip_prefix(".jiaclaw-memory-").is_some_and(valid_id) => BUNDLE_BYTES,
            _ => return Err(invalid("卸载记录有未知条目")),
        };
        snapshot(op, &name, limit)?.ok_or_else(|| invalid("卸载记录消失"))?;
    }
    let Some(raw) = snapshot(op, "intent.json", INTENT_BYTES)? else {
        return Ok(None);
    };
    let intent: RemoveIntent =
        serde_json::from_str(&raw).map_err(|_| invalid("卸载意图schema无效"))?;
    if intent.version != 1 || intent.operation_id != id {
        return Err(invalid("卸载意图身份无效"));
    }
    let bundle_raw =
        snapshot(op, "bundle.json", BUNDLE_BYTES)?.ok_or_else(|| invalid("卸载原包缺失"))?;
    let original = snapshot(op, "original-policy.json", SKILL_FILE_BYTES)?
        .ok_or_else(|| invalid("卸载原策略缺失"))?;
    let proposed = snapshot(op, "proposed-policy.json", SKILL_FILE_BYTES)?
        .ok_or_else(|| invalid("卸载候选缺失"))?;
    if digest(&bundle_raw) != intent.bundle_sha256
        || digest(&original) != intent.original_policy_sha256
        || digest(&proposed) != intent.proposed_policy_sha256
    {
        return Err(invalid("卸载捕获摘要不一致"));
    }
    let bundle = Bundle::parse(&bundle_raw)?;
    if proposed_policy(&original, &bundle)? != proposed {
        return Err(invalid("卸载候选不是单一停用声明移除"));
    }
    if let Some(retired) = directory(op, "retired", false, true)? {
        verify_identity(&retired, &intent.directory)?;
        verify_tree(&retired, &bundle, false)?;
    }
    Ok(Some(RemoveCapture {
        intent,
        intent_sha256: digest(&raw),
        bundle,
        original,
        proposed,
    }))
}

fn remove_status(c: &RemoveCapture, state: SkillRemoveState) -> SkillRemoveStatus {
    SkillRemoveStatus {
        operation_id: c.intent.operation_id.clone(),
        state,
        intent_sha256: Some(c.intent_sha256.clone()),
        directory: Some(c.bundle.directory.clone()),
        runtime_applied: false,
    }
}

fn remove_completed(
    op: &Dir,
    c: &RemoveCapture,
) -> Result<Option<SkillRemoveStatus>, JiaClawError> {
    let Some(raw) = snapshot(op, "receipt.json", INTENT_BYTES)? else {
        return Ok(None);
    };
    let receipt: SkillRemoveStatus =
        serde_json::from_str(&raw).map_err(|_| invalid("卸载收据schema无效"))?;
    if receipt != remove_status(c, SkillRemoveState::Completed)
        || directory(op, "retired", false, true)?.is_none()
    {
        return Err(invalid("卸载收据或退休身份不一致"));
    }
    Ok(Some(receipt))
}

fn observe_remove(
    root: &Dir,
    op: &Dir,
    c: &RemoveCapture,
) -> Result<SkillRemoveStatus, JiaClawError> {
    if let Some(receipt) = remove_completed(op, c)? {
        return Ok(receipt);
    }
    let policy = required(root, "skills.lock.json", SKILL_FILE_BYTES)?;
    let skills =
        directory(root, "skills", false, false)?.ok_or_else(|| invalid("卸载技能根缺失"))?;
    let installed = directory(&skills, &c.bundle.directory, false, true)?;
    let retired = directory(op, "retired", false, true)?;
    let state = match (installed, retired) {
        (Some(dir), None) if policy == c.original => {
            verify_identity(&dir, &c.intent.directory)?;
            verify_tree(&dir, &c.bundle, false)?;
            SkillRemoveState::Prepared
        }
        (None, Some(_)) if policy == c.original => SkillRemoveState::FilesRetired,
        (None, Some(_)) if policy == c.proposed => SkillRemoveState::PolicyPublished,
        _ => return Err(invalid("卸载目录/策略冲突；保留证据，不覆盖或重发")),
    };
    Ok(remove_status(c, state))
}

pub(super) fn ensure_settled(root: &Dir, new_id: &str) -> Result<(), JiaClawError> {
    if let Some(journal) = directory(root, JOURNAL, false, true)? {
        for id in operation_ids(&journal)? {
            if id == new_id {
                return Err(invalid("原UUID已有卸载记录；禁止新操作"));
            }
            let op =
                directory(&journal, &id, false, true)?.ok_or_else(|| invalid("卸载记录缺失"))?;
            let c =
                capture_remove(&op, &id)?.ok_or_else(|| invalid("有未完成捕获卸载；先人工审阅"))?;
            if remove_completed(&op, &c)?.is_none() {
                return Err(invalid("有未结算卸载；先核对原身份"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, String, String) {
        let work = tempfile::tempdir().unwrap();
        fs::create_dir(work.path().join("skills")).unwrap();
        let empty = "{\"version\":2,\"skills\":[]}\n";
        fs::write(work.path().join("skills.lock.json"), empty).unwrap();
        let body = "---\nname: reviewed\ndescription: remove\n---\nretained body\n";
        let bundle = serde_json::json!({"version":1,"directory":"reviewed","source":{
            "repository":"https://github.com/example/skills","revision":"a".repeat(40),"skill_sha256":digest(body)},
            "files":[{"path":"SKILL.md","text":body}]}).to_string();
        fs::write(work.path().join("install.json"), &bundle).unwrap();
        SkillInstaller::new(work.path(), true)
            .install(
                &uuid::Uuid::new_v4().to_string(),
                "install.json",
                &digest(&bundle),
                &digest(empty),
            )
            .unwrap();
        let policy = fs::read_to_string(work.path().join("skills.lock.json")).unwrap();
        (work, policy, body.into())
    }

    #[test]
    fn local_remove_retains_original_directory_and_historical_identity() {
        let (work, policy, body) = fixture();
        let path = work.path().join("skills/reviewed");
        let original = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        let identity = DirectoryIdentity::read(&original).unwrap();
        let remover = SkillRemover::new(work.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        let receipt = remover
            .uninstall(&id, "reviewed", &digest(&policy), &digest(&body))
            .unwrap();
        assert_eq!(receipt.state, SkillRemoveState::Completed);
        assert!(!receipt.runtime_applied);
        assert!(!path.exists());
        assert_eq!(original.read_to_string("SKILL.md").unwrap(), body);
        let retired = Dir::open_ambient_dir(
            work.path().join(JOURNAL).join(&id).join("retired"),
            cap_std::ambient_authority(),
        )
        .unwrap();
        assert_eq!(DirectoryIdentity::read(&retired).unwrap(), identity);
        assert_eq!(remover.inspect(&id).unwrap(), receipt);
        assert!(remover.finish(&id, &"0".repeat(64)).is_err());
        // Later publication at the same path does not become this operation's target.
        fs::create_dir(&path).unwrap();
        fs::write(path.join("SKILL.md"), "new administrator files").unwrap();
        assert_eq!(
            remover
                .finish(&id, receipt.intent_sha256.as_ref().unwrap())
                .unwrap(),
            receipt
        );
        assert_eq!(
            fs::read_to_string(path.join("SKILL.md")).unwrap(),
            "new administrator files"
        );
    }

    #[test]
    fn local_remove_rejects_active_drifted_or_stale_before_intent() {
        let (work, policy, body) = fixture();
        let remover = SkillRemover::new(work.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        assert!(remover
            .uninstall(&id, "reviewed", &"0".repeat(64), &digest(&body))
            .is_err());
        let mut enabled: serde_json::Value = serde_json::from_str(&policy).unwrap();
        enabled["skills"][0]["enabled"] = true.into();
        let enabled = enabled.to_string();
        fs::write(work.path().join("skills.lock.json"), &enabled).unwrap();
        assert!(remover
            .uninstall(&id, "reviewed", &digest(&enabled), &digest(&body))
            .is_err());
        fs::write(work.path().join("skills.lock.json"), &policy).unwrap();
        fs::write(work.path().join("skills/reviewed/SKILL.md"), "drift").unwrap();
        assert!(remover
            .uninstall(&id, "reviewed", &digest(&policy), &digest(&body))
            .is_err());
        assert!(!work.path().join(JOURNAL).exists());
        assert_eq!(
            fs::read_to_string(work.path().join("skills.lock.json")).unwrap(),
            policy
        );
    }

    #[test]
    fn local_remove_recovers_original_stages_and_preserves_conflict_evidence() {
        for stage in [
            SkillRemoveState::Prepared,
            SkillRemoveState::FilesRetired,
            SkillRemoveState::PolicyPublished,
        ] {
            let (work, policy, body) = fixture();
            let remover = SkillRemover::new(work.path(), true);
            let id = uuid::Uuid::new_v4().to_string();
            let receipt = remover
                .uninstall(&id, "reviewed", &digest(&policy), &digest(&body))
                .unwrap();
            let op = work.path().join(JOURNAL).join(&id);
            fs::remove_file(op.join("receipt.json")).unwrap();
            if stage != SkillRemoveState::PolicyPublished {
                fs::write(work.path().join("skills.lock.json"), &policy).unwrap();
            }
            if stage == SkillRemoveState::Prepared {
                fs::rename(op.join("retired"), work.path().join("skills/reviewed")).unwrap();
            }
            assert_eq!(remover.inspect(&id).unwrap().state, stage);
            let conflict = "{\"version\":2,\"skills\":[]} ";
            let prior = fs::read(work.path().join("skills.lock.json")).unwrap();
            fs::write(work.path().join("skills.lock.json"), conflict).unwrap();
            assert!(remover
                .finish(&id, receipt.intent_sha256.as_ref().unwrap())
                .is_err());
            assert_eq!(
                fs::read_to_string(work.path().join("skills.lock.json")).unwrap(),
                conflict
            );
            fs::write(work.path().join("skills.lock.json"), prior).unwrap();
            assert_eq!(
                remover
                    .finish(&id, receipt.intent_sha256.as_ref().unwrap())
                    .unwrap(),
                receipt
            );
        }
    }

    #[test]
    fn removal_uuid_and_unsettled_intent_exclude_new_install_and_update() {
        let (work, policy, body) = fixture();
        let remover = SkillRemover::new(work.path(), true);
        let id = uuid::Uuid::new_v4().to_string();
        remover
            .uninstall(&id, "reviewed", &digest(&policy), &digest(&body))
            .unwrap();
        let op = work.path().join(JOURNAL).join(&id);
        let current = fs::read_to_string(work.path().join("skills.lock.json")).unwrap();
        let raw = fs::read_to_string(work.path().join("install.json")).unwrap();
        let installer = SkillInstaller::new(work.path(), true);
        assert!(installer
            .install(&id, "install.json", &digest(&raw), &digest(&current))
            .is_err());
        fs::remove_file(op.join("receipt.json")).unwrap();
        assert!(installer
            .install(
                &uuid::Uuid::new_v4().to_string(),
                "install.json",
                &digest(&raw),
                &digest(&current)
            )
            .is_err());
        assert!(!work.path().join("skills/reviewed").exists());
        // Unsafe known status leaves fail closed before a preparation observation.
        fs::remove_file(op.join("intent.json")).unwrap();
        fs::remove_file(op.join("bundle.json")).unwrap();
        std::os::unix::fs::symlink(work.path().join("install.json"), op.join("bundle.json"))
            .unwrap();
        assert!(remover.inspect(&id).is_err());
    }
}
