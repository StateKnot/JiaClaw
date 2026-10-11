// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::{invalid, valid_hash, CATALOG_SKILLS, SKILL_FILE_BYTES};
use jiaclaw_core::JiaClawError;
use std::collections::HashMap;
use std::path::Path;

/// Administrator-declared origin and the exact raw SKILL.md digest, not a signature.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSourcePin {
    /// Declared HTTPS origin, without embedded credentials.
    pub repository: String,
    /// Fixed lower-case Git object ID, not a mutable ref.
    pub revision: String,
    /// SHA-256 of complete original SKILL.md bytes, including frontmatter.
    pub skill_sha256: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    skills: Vec<Entry>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    directory: String,
    source: SkillSourcePin,
    #[serde(default, deserialize_with = "explicit_enabled")]
    enabled: Option<bool>,
}

fn explicit_enabled<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    <bool as serde::Deserialize>::deserialize(deserializer).map(Some)
}

/// Normalized disk policy; inspection does not certify a running registry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillLockPolicy {
    /// Supported source-lock schema version, preserving version 1 semantics.
    pub version: u32,
    /// SHA-256 of the single raw manifest read by this disk inspection.
    pub manifest_sha256: String,
    /// All declarations sorted by directory; at most 64 entries.
    pub skills: Vec<SkillPolicyEntry>,
}

/// Administrator declaration, retained when loading is disabled.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillPolicyEntry {
    /// Canonical top-level directory, distinct from the model-visible skill name.
    pub directory: String,
    /// Whether this declaration participates in strict loading and byte checks.
    pub enabled: bool,
    /// Retained administrator origin; no publisher authentication is implied.
    pub source: SkillSourcePin,
}

// Owns parsing, selection and exact enabled-catalog coverage.
pub(super) struct CatalogLock {
    pub(super) policy: SkillLockPolicy,
    by_directory: HashMap<String, usize>,
}

impl CatalogLock {
    fn entry_mut(&mut self, name: &str) -> Result<&mut SkillPolicyEntry, JiaClawError> {
        let index = self
            .by_directory
            .get(name)
            .copied()
            .ok_or_else(|| invalid("技能修改目录未登记，未提交"))?;
        Ok(&mut self.policy.skills[index])
    }

    pub(super) fn read(workspace: &Path) -> Result<Self, JiaClawError> {
        let file =
            crate::memory_io::read_text(workspace, "skills.lock.json", SKILL_FILE_BYTES, false)
                .map_err(|_| invalid("必需技能锁无法安全读取"))?
                .ok_or_else(|| invalid("必需技能锁 skills.lock.json 不存在"))?;
        Self::parse(&file.text)
    }

    fn parse(text: &str) -> Result<Self, JiaClawError> {
        // Do not echo parse errors or origin data, which could contain credentials.
        let manifest: Manifest =
            serde_json::from_str(text).map_err(|_| invalid("技能锁 JSON/schema 无效"))?;
        if !matches!(manifest.version, 1 | 2) || manifest.skills.len() > CATALOG_SKILLS {
            return Err(invalid("技能锁要求 version=1或2，最多 64 个唯一目录"));
        }
        let mut entries = Vec::new();
        for entry in manifest.skills {
            if !directory(&entry.directory)
                || !source(&entry.source)
                || (manifest.version == 1 && entry.enabled.is_some())
                || (manifest.version == 2 && entry.enabled.is_none())
            {
                return Err(invalid("技能锁目录、来源或显式启停策略无效"));
            }
            entries.push(SkillPolicyEntry {
                directory: entry.directory,
                enabled: entry.enabled.unwrap_or(true),
                source: entry.source,
            });
        }
        entries.sort_by(|left, right| left.directory.cmp(&right.directory));
        let mut by_directory = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            if by_directory
                .insert(entry.directory.clone(), index)
                .is_some()
            {
                return Err(invalid("技能锁目录重复"));
            }
        }
        use sha2::Digest;
        Ok(Self {
            policy: SkillLockPolicy {
                version: manifest.version,
                manifest_sha256: format!("{:x}", sha2::Sha256::digest(text.as_bytes())),
                skills: entries,
            },
            by_directory,
        })
    }

    pub(super) fn disabled(&self, directory: &str) -> bool {
        self.by_directory
            .get(directory)
            .is_some_and(|index| !self.policy.skills[*index].enabled)
    }

    pub(super) fn bind(
        &self,
        directory: &str,
        raw_hash: &str,
    ) -> Result<SkillSourcePin, JiaClawError> {
        let index = self
            .by_directory
            .get(directory)
            .ok_or_else(|| invalid("实际技能未登记在必需锁中"))?;
        let pin = &self.policy.skills[*index].source;
        if pin.skill_sha256 != raw_hash {
            return Err(invalid("技能原始文件与必需锁不一致"));
        }
        Ok(pin.clone())
    }

    pub(super) fn complete(&self, loaded: usize) -> Result<(), JiaClawError> {
        if loaded
            != self
                .policy
                .skills
                .iter()
                .filter(|entry| entry.enabled)
                .count()
        {
            return Err(invalid("必需锁登记的技能文件缺失"));
        }
        Ok(())
    }
}

pub(super) fn set_enabled(
    discovery: &super::SkillDiscovery,
    name: &str,
    enabled: bool,
    expected_hash: &str,
) -> Result<SkillLockPolicy, JiaClawError> {
    edit_policy(discovery, name, expected_hash, |lock| {
        let version = lock.policy.version;
        let entry = lock.entry_mut(name)?;
        if version == 2 && entry.enabled == enabled {
            return Err(invalid("技能已处于请求状态，未提交"));
        }
        entry.enabled = enabled;
        Ok(())
    })
}

pub(super) fn set_source(
    discovery: &super::SkillDiscovery,
    name: &str,
    proposed: &SkillSourcePin,
    expected_hash: &str,
) -> Result<SkillLockPolicy, JiaClawError> {
    if !source(proposed) {
        return Err(invalid("技能来源声明无效，未提交"));
    }
    edit_policy(discovery, name, expected_hash, |lock| {
        let entry = lock.entry_mut(name)?;
        if entry.enabled {
            return Err(invalid("修改来源前必须明确停用已登记技能，未提交"));
        }
        if entry.source == *proposed {
            return Err(invalid("技能来源没有变化，未提交"));
        }
        verify_source_body(discovery, name, proposed)?;
        entry.source = proposed.clone();
        Ok(())
    })
}

pub(super) fn register(
    discovery: &super::SkillDiscovery,
    name: &str,
    proposed: &SkillSourcePin,
    expected_hash: &str,
) -> Result<SkillLockPolicy, JiaClawError> {
    if !source(proposed) {
        return Err(invalid("技能来源声明无效，未提交"));
    }
    edit_policy(discovery, name, expected_hash, |lock| {
        if lock.by_directory.contains_key(name) {
            return Err(invalid("技能目录已登记，未提交"));
        }
        if lock.policy.skills.len() >= CATALOG_SKILLS {
            return Err(invalid("技能锁最多64项，未提交"));
        }
        verify_source_body(discovery, name, proposed)?;
        lock.policy.skills.push(SkillPolicyEntry {
            directory: name.to_owned(),
            enabled: false,
            source: proposed.clone(),
        });
        Ok(())
    })
}

fn verify_source_body(
    discovery: &super::SkillDiscovery,
    name: &str,
    proposed: &SkillSourcePin,
) -> Result<(), JiaClawError> {
    let loaded = super::Skill::read(
        &discovery.workspace,
        &format!("skills/{name}/SKILL.md"),
        &discovery.skills_root.join(name),
    )
    .map_err(|_| invalid("新来源正文无法安全读取或解析，未提交"))?
    .ok_or_else(|| invalid("新来源正文不存在，未提交"))?;
    if loaded.2 != proposed.skill_sha256 {
        return Err(invalid("新来源正文与已审核摘要不一致，未提交"));
    }
    Ok(())
}

// All operator edits keep captured bytes, checks and publication inside the
// same existing workspace writer. No disk mutation precedes full validation.
fn edit_policy(
    discovery: &super::SkillDiscovery,
    name: &str,
    expected_hash: &str,
    edit: impl FnOnce(&mut CatalogLock) -> Result<(), JiaClawError>,
) -> Result<SkillLockPolicy, JiaClawError> {
    if !discovery.lock_required || !directory(name) || !valid_hash(expected_hash) {
        return Err(invalid("技能锁写入要求必需锁、有效目录和原锁 SHA256"));
    }
    crate::memory_io::update_existing_text(
        &discovery.workspace,
        "skills.lock.json",
        SKILL_FILE_BYTES,
        |text| {
            let mut lock = CatalogLock::parse(text)?;
            if lock.policy.manifest_sha256 != expected_hash {
                return Err(invalid("技能锁版本已改变，未提交；重新核对磁盘策略"));
            }
            edit(&mut lock)?;
            let next = serde_json::to_string(&serde_json::json!({
                "version": 2, "skills": lock.policy.skills
            }))
            .map_err(|_| invalid("技能策略无法序列化"))?
                + "\n";
            let candidate = CatalogLock::parse(&next)?;
            let policy = candidate.policy.clone();
            discovery.scan_locked(true, Some(candidate))?;
            Ok((next, policy))
        },
    )
}

fn directory(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\', ':'])
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn source(pin: &SkillSourcePin) -> bool {
    let revision = &pin.revision;
    let revision_valid = matches!(revision.len(), 40 | 64)
        && revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let address = &pin.repository;
    let url_valid = address.len() <= 1024
        && !address.chars().any(|c| c.is_whitespace() || c.is_control())
        && !address.contains('\\')
        && reqwest::Url::parse(address).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        });
    revision_valid && url_valid && valid_hash(&pin.skill_sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillDiscovery, SkillRegistry};
    use sha2::Digest;
    use std::fs;

    fn manifest(raw: &str) -> serde_json::Value {
        serde_json::json!({"version":1,"skills":[{"directory":"reviewed","source":{
            "repository":"https://github.com/example/skills","revision":"a".repeat(40),
            "skill_sha256":format!("{:x}",sha2::Sha256::digest(raw.as_bytes()))}}]})
    }

    #[test]
    fn registration_preserves_existing_origins_and_requires_separate_activation() {
        let workspace = tempfile::tempdir().unwrap();
        let old = "---\nname: reviewed\ndescription: original\n---\nold";
        let new = "---\nname: model-name\ndescription: new\n---\nnew";
        for (name, raw) in [("reviewed", old), ("added", new)] {
            fs::create_dir_all(workspace.path().join("skills").join(name)).unwrap();
            fs::write(
                workspace.path().join("skills").join(name).join("SKILL.md"),
                raw,
            )
            .unwrap();
        }
        let file = workspace.path().join("skills.lock.json");
        fs::write(&file, manifest(old).to_string()).unwrap();
        let original = CatalogLock::read(workspace.path()).unwrap().policy;
        let pin = SkillSourcePin {
            skill_sha256: format!("{:x}", sha2::Sha256::digest(new.as_bytes())),
            ..original.skills[0].source.clone()
        };
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        assert!(discovery.discover().is_err());
        let registered = discovery
            .register("added", &pin, &original.manifest_sha256)
            .unwrap();
        assert_eq!(registered.version, 2);
        assert_eq!(registered.skills[0].directory, "added");
        assert!(!registered.skills[0].enabled);
        assert_eq!(registered.skills[0].source, pin);
        assert!(registered.skills[1].enabled);
        assert_eq!(registered.skills[1].source, original.skills[0].source);
        assert_eq!(discovery.discover().unwrap().len(), 1);
        let saved = fs::read(&file).unwrap();
        assert!(discovery
            .register("added", &pin, &registered.manifest_sha256)
            .is_err());
        assert!(discovery
            .register("missing", &pin, &original.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        discovery
            .set_enabled("added", true, &registered.manifest_sha256)
            .unwrap();
        assert_eq!(discovery.discover().unwrap().len(), 2);
    }

    #[test]
    fn registration_rejects_drift_missing_body_and_full_catalog_without_publication() {
        let workspace = tempfile::tempdir().unwrap();
        let raw = "---\nname: reviewed\ndescription: original\n---\nbody";
        let file = workspace.path().join("skills.lock.json");
        fs::create_dir_all(workspace.path().join("skills/reviewed")).unwrap();
        fs::write(workspace.path().join("skills/reviewed/SKILL.md"), raw).unwrap();
        fs::create_dir_all(workspace.path().join("skills/new")).unwrap();
        fs::write(workspace.path().join("skills/new/SKILL.md"), raw).unwrap();
        fs::write(&file, manifest(raw).to_string()).unwrap();
        let original = CatalogLock::read(workspace.path()).unwrap().policy;
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        let saved = fs::read(&file).unwrap();
        let pin = &original.skills[0].source;
        fs::write(workspace.path().join("skills/reviewed/SKILL.md"), "drift").unwrap();
        assert!(discovery
            .register("new", pin, &original.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        fs::write(workspace.path().join("skills/reviewed/SKILL.md"), raw).unwrap();
        fs::remove_file(workspace.path().join("skills/new/SKILL.md")).unwrap();
        assert!(discovery
            .register("new", pin, &original.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        let entries: Vec<_> = (0..64)
            .map(|i| {
                serde_json::json!({
                    "directory": format!("item-{i}"), "enabled": false, "source": pin
                })
            })
            .collect();
        fs::write(
            &file,
            serde_json::json!({"version":2,"skills":entries}).to_string(),
        )
        .unwrap();
        let policy = CatalogLock::read(workspace.path()).unwrap().policy;
        let saved = fs::read(&file).unwrap();
        assert!(discovery
            .register("new", pin, &policy.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        assert!(!fs::read_dir(workspace.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jiaclaw-memory-")));
    }

    #[test]
    fn source_approval_requires_disabled_matching_body_and_keeps_policy() {
        let workspace = tempfile::tempdir().unwrap();
        let folder = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&folder).unwrap();
        let raw = "---\nname: reviewed\ndescription: original\n---\nbody";
        fs::write(folder.join("SKILL.md"), raw).unwrap();
        let file = workspace.path().join("skills.lock.json");
        fs::write(&file, manifest(raw).to_string()).unwrap();
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        let first = discovery.inspect_policy().unwrap();
        let mut proposed = first.skills[0].source.clone();
        proposed.revision = "b".repeat(40);
        assert!(discovery
            .set_source("reviewed", &proposed, &first.manifest_sha256)
            .is_err());
        let disabled = discovery
            .set_enabled("reviewed", false, &first.manifest_sha256)
            .unwrap();
        let original = fs::read(&file).unwrap();
        let changed = raw.replace("original", "approved");
        fs::write(folder.join("SKILL.md"), &changed).unwrap();
        assert!(discovery
            .set_source("reviewed", &proposed, &disabled.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), original);
        proposed.skill_sha256 = format!("{:x}", sha2::Sha256::digest(changed.as_bytes()));
        let approved = discovery
            .set_source("reviewed", &proposed, &disabled.manifest_sha256)
            .unwrap();
        assert!(!approved.skills[0].enabled);
        assert_eq!(approved.skills[0].source, proposed);
        assert!(discovery.discover().unwrap().is_empty());
        let saved = fs::read(&file).unwrap();
        assert!(discovery
            .set_source("reviewed", &proposed, &approved.manifest_sha256)
            .is_err());
        assert!(discovery
            .set_source(
                "reviewed",
                &first.skills[0].source,
                &disabled.manifest_sha256
            )
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), saved);
        discovery
            .set_enabled("reviewed", true, &approved.manifest_sha256)
            .unwrap();
        assert_eq!(discovery.discover().unwrap()[0].description, "approved");
    }

    #[test]
    fn source_approval_rejects_invalid_declaration_and_unreadable_body() {
        let workspace = tempfile::tempdir().unwrap();
        let folder = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&folder).unwrap();
        let raw = "---\nname: reviewed\ndescription: original\n---\nbody";
        fs::write(folder.join("SKILL.md"), raw).unwrap();
        let file = workspace.path().join("skills.lock.json");
        fs::write(&file, manifest(raw).to_string()).unwrap();
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        let first = discovery.inspect_policy().unwrap();
        let disabled = discovery
            .set_enabled("reviewed", false, &first.manifest_sha256)
            .unwrap();
        let bytes = fs::read(&file).unwrap();
        let mut proposed = first.skills[0].source.clone();
        proposed.repository = "https://secret:password@example.test/repo".into();
        let error = discovery
            .set_source("reviewed", &proposed, &disabled.manifest_sha256)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("password") && !error.contains("secret"));
        proposed = first.skills[0].source.clone();
        proposed.revision = "b".repeat(40);
        fs::remove_file(folder.join("SKILL.md")).unwrap();
        assert!(discovery
            .set_source("reviewed", &proposed, &disabled.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert!(fs::read_dir(workspace.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jiaclaw-memory-")));
    }

    #[tokio::test]
    async fn source_lock_pins_raw_metadata_and_retains_policy_on_reload() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&dir).unwrap();
        let raw = "---\nname: approved\ndescription: before\n---\nbody";
        let file = dir.join("SKILL.md");
        let lock = workspace.path().join("skills.lock.json");
        fs::write(&file, raw).unwrap();
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        assert!(discovery.discover().is_err());
        fs::write(&lock, manifest(raw).to_string()).unwrap();
        let skills = discovery.discover().unwrap();
        assert!(skills[0].source.is_some());
        let registry = std::sync::Arc::new(SkillRegistry::new(skills).with_lock_required(true));
        let changed = raw.replace("before", "after");
        fs::write(&file, &changed).unwrap();
        assert!(discovery.find("reviewed").is_err());
        assert!(registry.reload_async(workspace.path()).await.is_err());
        assert_eq!(registry.snapshot()[0].description, "before");
        fs::write(&lock, manifest(&changed).to_string()).unwrap();
        registry.reload_async(workspace.path()).await.unwrap();
        assert_eq!(registry.snapshot()[0].description, "after");
        fs::remove_file(&lock).unwrap();
        assert!(registry.reload(workspace.path()).is_err());
        assert_eq!(registry.snapshot()[0].description, "after");
    }

    #[test]
    fn policy_edit_recovers_drift_and_rejects_stale_or_invalid_enable_without_commit() {
        let workspace = tempfile::tempdir().unwrap();
        let folder = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&folder).unwrap();
        let raw = "---\nname: reviewed\ndescription: tested\n---\nbody";
        fs::write(folder.join("SKILL.md"), raw).unwrap();
        let file = workspace.path().join("skills.lock.json");
        let original = manifest(raw).to_string();
        fs::write(&file, &original).unwrap();
        let expected = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        fs::write(folder.join("SKILL.md"), "drift").unwrap();
        let disabled = discovery.set_enabled("reviewed", false, &expected).unwrap();
        assert_eq!(disabled.version, 2);
        assert!(!disabled.skills[0].enabled);
        let settled = fs::read(&file).unwrap();
        assert!(discovery
            .set_enabled("reviewed", false, &disabled.manifest_sha256)
            .is_err());
        assert!(discovery.set_enabled("reviewed", true, &expected).is_err());
        assert!(discovery
            .set_enabled("reviewed", true, &disabled.manifest_sha256)
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), settled);
        fs::write(folder.join("SKILL.md"), raw).unwrap();
        let enabled = discovery
            .set_enabled("reviewed", true, &disabled.manifest_sha256)
            .unwrap();
        assert!(enabled.skills[0].enabled);
        assert_eq!(
            enabled.skills[0].source.skill_sha256,
            manifest(raw)["skills"][0]["source"]["skill_sha256"]
        );
        assert_eq!(
            discovery.inspect_policy().unwrap().manifest_sha256,
            enabled.manifest_sha256
        );
    }

    #[cfg(unix)]
    #[test]
    fn policy_edit_shares_workspace_writer_owner_and_never_creates_a_missing_lock() {
        let workspace = tempfile::tempdir().unwrap();
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        assert!(discovery
            .set_enabled("reviewed", false, &"a".repeat(64))
            .is_err());
        assert!(!workspace.path().join("skills.lock.json").exists());
        let original = serde_json::json!({"version":1,"skills":[{
            "directory":"reviewed","source":manifest("body")["skills"][0]["source"]
        }]})
        .to_string();
        let file = workspace.path().join("skills.lock.json");
        fs::write(&file, &original).unwrap();
        let hash = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
        let owner = std::fs::File::open(workspace.path()).unwrap();
        fs2::FileExt::try_lock_exclusive(&owner).unwrap();
        assert!(discovery.set_enabled("reviewed", false, &hash).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        fs2::FileExt::unlock(&owner).unwrap();
        assert!(discovery.set_enabled("reviewed", false, &hash).is_ok());
    }

    #[test]
    fn source_lock_requires_exact_catalog_and_explicit_empty_lock() {
        let workspace = tempfile::tempdir().unwrap();
        let lock = workspace.path().join("skills.lock.json");
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        fs::write(&lock, "{\"version\":1,\"skills\":[]}").unwrap();
        assert!(discovery.discover().unwrap().is_empty());
        fs::write(&lock, manifest("body").to_string()).unwrap();
        assert!(discovery.discover().is_err());
        let dir = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "body").unwrap();
        assert_eq!(discovery.discover().unwrap().len(), 1);
        let other = workspace.path().join("skills/unregistered");
        fs::create_dir(&other).unwrap();
        assert_eq!(discovery.discover().unwrap().len(), 1);
        fs::write(other.join("SKILL.md"), "new").unwrap();
        assert!(discovery.discover().is_err());
        assert_eq!(
            SkillDiscovery::new(workspace.path())
                .discover()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn source_lock_rejects_schema_origin_and_directory_ambiguity() {
        let workspace = tempfile::tempdir().unwrap();
        let lock = workspace.path().join("skills.lock.json");
        let valid = manifest("body");
        for bad in [
            serde_json::json!({"version":3,"skills":[]}),
            serde_json::json!({"version":1,"skills":[],"extra":true}),
            serde_json::json!({"version":1,"skills":[valid["skills"][0],valid["skills"][0]]}),
        ] {
            fs::write(&lock, bad.to_string()).unwrap();
            assert!(CatalogLock::read(workspace.path()).is_err());
        }
        for directory in ["../escape", "a/b", " a", "a b", "a\\b", "C:", ".", ".."] {
            let mut bad = valid.clone();
            bad["skills"][0]["directory"] = directory.into();
            fs::write(&lock, bad.to_string()).unwrap();
            assert!(CatalogLock::read(workspace.path()).is_err());
        }
        for url in [
            "http://host/repo",
            "https://user:secret@host/repo",
            "https://host/repo?k=secret",
            "https://host/repo#frag",
            "https://host/ a",
        ] {
            let mut bad = valid.clone();
            bad["skills"][0]["source"]["repository"] = url.into();
            fs::write(&lock, bad.to_string()).unwrap();
            let error = CatalogLock::read(workspace.path())
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains("secret"));
        }
        for revision in ["main".to_string(), "A".repeat(40), "a".repeat(39)] {
            let mut bad = valid.clone();
            bad["skills"][0]["source"]["revision"] = revision.into();
            fs::write(&lock, bad.to_string()).unwrap();
            assert!(CatalogLock::read(workspace.path()).is_err());
        }
        fs::write(&lock, "{\"version\":1,\"version\":1,\"skills\":[]}").unwrap();
        assert!(CatalogLock::read(workspace.path()).is_err());
    }

    #[tokio::test]
    async fn activation_policy_disables_reads_and_persists_across_reload_and_restart() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join("skills/reviewed");
        fs::create_dir_all(&dir).unwrap();
        let raw = "---\nname: approved\n---\nbody";
        fs::write(dir.join("SKILL.md"), raw).unwrap();
        let lock = workspace.path().join("skills.lock.json");
        let mut value = manifest(raw);
        value["version"] = 2.into();
        value["skills"][0]["enabled"] = true.into();
        fs::write(&lock, value.to_string()).unwrap();
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        let registry = std::sync::Arc::new(
            SkillRegistry::new(discovery.discover().unwrap()).with_lock_required(true),
        );
        let hash = registry.snapshot()[0].content_sha256();
        value["skills"][0]["enabled"] = false.into();
        fs::write(&lock, value.to_string()).unwrap();
        // A disk edit alone must not imply that the running registry applied it.
        assert_eq!(registry.read_content("approved", &hash).unwrap(), "body");
        fs::write(dir.join("SKILL.md"), [b'x', 0xff]).unwrap();
        registry.reload_async(workspace.path()).await.unwrap();
        assert!(registry.snapshot().is_empty());
        assert!(registry.read_content("approved", &hash).is_err());
        assert!(discovery.discover().unwrap().is_empty());
        assert!(!discovery.inspect_policy().unwrap().skills[0].enabled);
        fs::remove_file(dir.join("SKILL.md")).unwrap();
        fs::remove_dir(&dir).unwrap();
        assert!(discovery.discover().unwrap().is_empty());
        value["skills"][0]["enabled"] = true.into();
        fs::write(&lock, value.to_string()).unwrap();
        assert!(registry.reload_async(workspace.path()).await.is_err());
        assert!(registry.snapshot().is_empty());
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), raw).unwrap();
        registry.reload_async(workspace.path()).await.unwrap();
        assert_eq!(registry.snapshot()[0].name, "approved");
    }

    #[test]
    fn activation_schema_requires_explicit_boolean_and_retains_one_lock_snapshot() {
        use sha2::Digest;
        let workspace = tempfile::tempdir().unwrap();
        let lock = workspace.path().join("skills.lock.json");
        let discovery = SkillDiscovery::new(workspace.path()).with_lock_required(true);
        let mut value = manifest("body");
        value["version"] = 2.into();
        for enabled in [serde_json::Value::Null, "false".into(), 0.into()] {
            value["skills"][0]["enabled"] = enabled;
            fs::write(&lock, value.to_string()).unwrap();
            assert!(discovery.inspect_policy().is_err());
        }
        value["skills"][0]
            .as_object_mut()
            .unwrap()
            .remove("enabled");
        fs::write(&lock, value.to_string()).unwrap();
        assert!(discovery.inspect_policy().is_err());
        value["skills"][0]["enabled"] = false.into();
        let bytes = value.to_string();
        fs::write(&lock, &bytes).unwrap();
        let captured = CatalogLock::read(workspace.path()).unwrap();
        fs::write(&lock, "{\"version\":2,\"skills\":[]}").unwrap();
        let policy = captured.policy.clone();
        assert!(discovery
            .scan_locked(true, Some(captured))
            .unwrap()
            .is_empty());
        assert_eq!(
            policy.manifest_sha256,
            format!("{:x}", sha2::Sha256::digest(bytes.as_bytes()))
        );
        assert_eq!(policy.skills[0].directory, "reviewed");
        assert!(!policy.skills[0].enabled);
        value["version"] = 1.into();
        for enabled in [serde_json::Value::Null, true.into(), false.into()] {
            value["skills"][0]["enabled"] = enabled;
            fs::write(&lock, value.to_string()).unwrap();
            assert!(discovery.inspect_policy().is_err());
        }
        assert!(SkillDiscovery::new(workspace.path())
            .inspect_policy()
            .is_err());
    }
}
