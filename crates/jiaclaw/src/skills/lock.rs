// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::{invalid, valid_hash, CATALOG_SKILLS, SKILL_FILE_BYTES};
use jiaclaw_core::JiaClawError;
use std::collections::HashMap;
use std::path::Path;

/// Administrator-declared origin and the exact raw SKILL.md digest, not a signature.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
}

// Owns parsing and exact catalog coverage; callers never interpret lock bytes.
pub(super) struct CatalogLock(HashMap<String, SkillSourcePin>);

impl CatalogLock {
    pub(super) fn read(workspace: &Path) -> Result<Self, JiaClawError> {
        let file =
            crate::memory_io::read_text(workspace, "skills.lock.json", SKILL_FILE_BYTES, false)
                .map_err(|_| invalid("必需技能锁无法安全读取"))?
                .ok_or_else(|| invalid("必需技能锁 skills.lock.json 不存在"))?;
        // Do not echo parse errors or origin data, which could contain credentials.
        let manifest: Manifest =
            serde_json::from_str(&file.text).map_err(|_| invalid("技能锁 JSON/schema 无效"))?;
        if manifest.version != 1 || manifest.skills.len() > CATALOG_SKILLS {
            return Err(invalid("技能锁要求 version=1，最多 64 个唯一目录"));
        }
        let mut entries = HashMap::new();
        for entry in manifest.skills {
            if !directory(&entry.directory)
                || !source(&entry.source)
                || entries.insert(entry.directory, entry.source).is_some()
            {
                return Err(invalid("技能锁目录、来源声明或唯一性无效"));
            }
        }
        Ok(Self(entries))
    }

    pub(super) fn bind(
        &self,
        directory: &str,
        raw_hash: &str,
    ) -> Result<SkillSourcePin, JiaClawError> {
        let pin = self
            .0
            .get(directory)
            .ok_or_else(|| invalid("实际技能未登记在必需锁中"))?;
        if pin.skill_sha256 != raw_hash {
            return Err(invalid("技能原始文件与必需锁不一致"));
        }
        Ok(pin.clone())
    }

    pub(super) fn complete(&self, loaded: usize) -> Result<(), JiaClawError> {
        if loaded != self.0.len() {
            return Err(invalid("必需锁登记的技能文件缺失"));
        }
        Ok(())
    }
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
            serde_json::json!({"version":2,"skills":[]}),
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
}
