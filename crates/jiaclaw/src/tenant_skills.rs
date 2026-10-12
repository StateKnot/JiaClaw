// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Complete permission metadata; source reads remain in the original registry.
use jiaclaw_core::{JiaClawError, TenantSkillReadEntry, TenantSkillReadPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Opt-in loaded-body permission, independent of SSE protocol2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantSkillReadCapability {
    /// Exact skill permission protocol.
    pub protocol: u8,
    /// Complete sorted exact names and loaded-body versions, never text or paths.
    pub skills: Vec<TenantSkillReadEntry>,
    /// Original complete skill-file/body byte limit.
    pub max_body_bytes: usize,
    /// Original serialized native tool-result byte limit.
    pub max_result_bytes: usize,
    /// Hash of the canonical permission tuple, not a publisher signature or epoch.
    pub policy_sha256: String,
}
impl TenantSkillReadCapability {
    /// Capture metadata without source/network access; Agent separately checks its live table.
    ///
    /// # Errors
    /// Invalid complete permission or encoding failure.
    pub fn capture(policy: &TenantSkillReadPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let pairs: Vec<_> = policy
            .skills
            .iter()
            .map(|s| (&s.name, &s.content_sha256))
            .collect();
        let bytes = serde_json::to_vec(&(
            1_u8,
            pairs,
            crate::skills::SKILL_FILE_BYTES,
            crate::tools::MAX_NATIVE_RESULT_BYTES,
        ))
        .map_err(|_| {
            JiaClawError::Configuration("tenant skill permission encoding failed".into())
        })?;
        Ok(Self {
            protocol: 1,
            skills: policy.skills.clone(),
            max_body_bytes: crate::skills::SKILL_FILE_BYTES,
            max_result_bytes: crate::tools::MAX_NATIVE_RESULT_BYTES,
            policy_sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JiaClawAgent;
    use jiaclaw_core::{AgentConfig, ToolCall};

    fn library(workspace: &std::path::Path, body: &str) {
        let mut entries = Vec::new();
        for (name, content) in [("reviewed", body), ("unshared", "private body")] {
            let directory = workspace.join("skills").join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let raw = format!("---\nname: {name}\ndescription: private metadata\n---\n{content}");
            std::fs::write(directory.join("SKILL.md"), &raw).unwrap();
            entries.push(serde_json::json!({"directory":name,"source":{"repository":"https://github.com/example/reviewed","revision":"a".repeat(40),"skill_sha256":format!("{:x}",Sha256::digest(raw.as_bytes()))}}));
        }
        std::fs::write(
            workspace.join("skills.lock.json"),
            serde_json::to_vec(&serde_json::json!({"version":1,"skills":entries})).unwrap(),
        )
        .unwrap();
    }
    fn permission(body: &str) -> TenantSkillReadPolicy {
        TenantSkillReadPolicy {
            skills: vec![TenantSkillReadEntry {
                name: "reviewed".into(),
                content_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            }],
        }
    }
    fn configured(workspace: &std::path::Path, body: &str) -> AgentConfig {
        let mut config = AgentConfig {
            workspace_path: workspace.into(),
            ..AgentConfig::default()
        };
        config.skill_lock_required = true;
        config.tools.skill_read.enabled = true;
        config.http.gateway_channel_chat = true;
        config.http.tenant_skill_read = Some(permission(body));
        config
    }
    #[test]
    fn tenant_skill_read_permission_is_bounded_complete_and_version_bound() {
        let policy = TenantSkillReadPolicy {
            skills: vec![TenantSkillReadEntry {
                name: "reviewed".into(),
                content_sha256: "b".repeat(64),
            }],
        };
        let capability = TenantSkillReadCapability::capture(&policy).unwrap();
        assert_eq!(
            capability.policy_sha256,
            "a5e0a768f2c015708d14545e90355c528dd97051fc0166724a11e3fa49c43e96"
        );
        for name in ["", " reviewed", "reviewed ", "x\n", "\u{feff}reviewed"] {
            let mut invalid = policy.clone();
            invalid.skills[0].name = name.into();
            assert!(TenantSkillReadCapability::capture(&invalid).is_err());
        }
        let mut duplicate = policy.clone();
        duplicate.skills.push(duplicate.skills[0].clone());
        assert!(TenantSkillReadCapability::capture(&duplicate).is_err());
        let mut version = policy;
        version.skills[0].content_sha256 = "c".repeat(64);
        assert_ne!(
            TenantSkillReadCapability::capture(&version)
                .unwrap()
                .policy_sha256,
            capability.policy_sha256
        );
        version.skills[0].content_sha256 = "A".repeat(64);
        assert!(TenantSkillReadCapability::capture(&version).is_err());
    }
    #[tokio::test]
    async fn tenant_skill_read_retains_original_registry_and_rejects_reloaded_version() {
        let workspace = tempfile::tempdir().unwrap();
        library(workspace.path(), "reviewed body");
        let agent = JiaClawAgent::new(configured(workspace.path(), "reviewed body")).unwrap();
        let args = serde_json::json!({"name":"reviewed","content_sha256":permission("reviewed body").skills[0].content_sha256});
        let call = ToolCall {
            tool_name: "skill_read".into(),
            arguments: args.clone(),
            result: None,
        };
        assert_eq!(agent.tools().execute(&call).await.unwrap(), "reviewed body");
        library(workspace.path(), "replacement body");
        assert_eq!(agent.tools().execute(&call).await.unwrap(), "reviewed body");
        agent.reload_skills_async().await.unwrap();
        assert!(agent.tenant_skill_read_capability().is_err());
        assert!(agent.tools().execute(&call).await.is_err());
        let replacement = ToolCall {
            arguments: serde_json::json!({"name":"reviewed","content_sha256":permission("replacement body").skills[0].content_sha256}),
            ..call
        };
        assert!(agent.tools().execute(&replacement).await.is_err());
    }
    #[tokio::test]
    async fn tenant_skill_read_never_grants_unselected_names_or_extra_arguments() {
        let workspace = tempfile::tempdir().unwrap();
        library(workspace.path(), "reviewed body");
        let agent = JiaClawAgent::new(configured(workspace.path(), "reviewed body")).unwrap();
        for arguments in [
            serde_json::json!({"name":"unshared","content_sha256":permission("private body").skills[0].content_sha256}),
            serde_json::json!({"name":"reviewed","content_sha256":permission("reviewed body").skills[0].content_sha256,"path":"SKILL.md"}),
        ] {
            assert!(agent
                .tools()
                .execute(&ToolCall {
                    tool_name: "skill_read".into(),
                    arguments,
                    result: None
                })
                .await
                .is_err());
        }
        assert_eq!(
            agent.tools().get("skill_read").unwrap().failure_effect(),
            crate::tools::ToolFailureEffect::NoEffect
        );
    }
    #[test]
    fn tenant_skill_read_startup_requires_live_locked_body_and_body_only_tool() {
        let workspace = tempfile::tempdir().unwrap();
        library(workspace.path(), "reviewed body");
        let config = configured(workspace.path(), "reviewed body");
        assert!(JiaClawAgent::new(config.clone()).is_ok());
        for mode in 0..4 {
            let mut invalid = config.clone();
            match mode {
                0 => invalid.skill_lock_required = false,
                1 => invalid.tools.skill_read.enabled = false,
                2 => invalid.tools.skill_read.resources_enabled = true,
                _ => invalid.http.tenant_skill_read = Some(permission("wrong body")),
            };
            assert!(JiaClawAgent::new(invalid).is_err());
        }
        std::fs::remove_file(workspace.path().join("skills.lock.json")).unwrap();
        assert!(JiaClawAgent::new(config).is_err());
    }
}
