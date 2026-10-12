// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! One canonical permission descriptor shared by private backend and gateway.
use jiaclaw_core::{JiaClawError, TenantFileReadPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Opt-in extension of the existing tracked tenant turn contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantFileReadCapability {
    /// Exact permission protocol, independent of SSE protocol 2.
    pub protocol: u8,
    /// Administrator-approved canonical file names; no content is returned.
    pub paths: Vec<String>,
    /// Full-source byte budget before optional line slicing.
    pub max_bytes: usize,
    /// SHA-256 of the canonical permission descriptor, not a source content hash.
    pub policy_sha256: String,
}
impl TenantFileReadCapability {
    /// Capture the complete validated permission. Does not read sources or contact a model.
    ///
    /// # Errors
    /// Invalid path grant or serialization failure.
    pub fn capture(policy: &TenantFileReadPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let bytes = serde_json::to_vec(&(1_u8, &policy.paths, crate::READ_FILE_MAX_BYTES))
            .map_err(|_| JiaClawError::Configuration("tenant permission encoding failed".into()))?;
        Ok(Self {
            protocol: 1,
            paths: policy.paths.clone(),
            max_bytes: crate::READ_FILE_MAX_BYTES,
            policy_sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tenant_file_permission_is_bounded_exact_and_changes_identity() {
        for paths in [
            vec![],
            vec!["MEMORY.md"],
            vec!["tenant-documents/../secret"],
            vec!["tenant-documents/.hidden"],
            vec!["tenant-documents/"],
            vec!["tenant-documents//a"],
            vec!["tenant-documents/a "],
            vec!["tenant-documents/a\\b"],
            vec!["tenant-documents/a\nb"],
            vec!["tenant-documents/a", "tenant-documents/a"],
            vec!["tenant-documents/b", "tenant-documents/a"],
        ] {
            assert!(TenantFileReadCapability::capture(&TenantFileReadPolicy {
                paths: paths.into_iter().map(str::to_owned).collect()
            })
            .is_err());
        }
        let policy = TenantFileReadPolicy {
            paths: vec!["tenant-documents/资料.txt".into()],
        };
        let captured = TenantFileReadCapability::capture(&policy).unwrap();
        assert_eq!(captured.max_bytes, crate::READ_FILE_MAX_BYTES);
        assert_eq!(captured.policy_sha256.len(), 64);
        assert_ne!(
            captured,
            TenantFileReadCapability::capture(&TenantFileReadPolicy {
                paths: vec!["tenant-documents/other.txt".into()]
            })
            .unwrap()
        );
        assert!(TenantFileReadCapability::capture(&TenantFileReadPolicy {
            paths: (0..33)
                .map(|n| format!("tenant-documents/{n:02}"))
                .collect()
        })
        .is_err());
    }
    #[tokio::test]
    async fn tenant_file_tool_uses_original_full_source_budget_and_refuses_aliases() {
        use crate::Tool;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("tenant-documents")).unwrap();
        let path = root.path().join("tenant-documents/source.txt");
        std::fs::write(&path, "first\nsecond\n").unwrap();
        let policy = TenantFileReadPolicy {
            paths: vec!["tenant-documents/source.txt".into()],
        };
        let tool = crate::FileReadTool::restricted(root.path(), &policy).unwrap();
        let value = tool
            .execute(serde_json::json!({"path":policy.paths[0],"offset":2,"limit":1}))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&value).unwrap()["content"],
            "second"
        );
        for path in [
            "MEMORY.md",
            " tenant-documents/source.txt",
            "tenant-documents/./source.txt",
            "tenant-documents/source.txt ",
        ] {
            assert!(tool
                .execute(serde_json::json!({"path":path}))
                .await
                .is_err());
        }
        std::fs::write(&path, vec![b'x'; crate::READ_FILE_MAX_BYTES + 1]).unwrap();
        assert!(tool
            .execute(serde_json::json!({"path":policy.paths[0],"limit":1}))
            .await
            .is_err());
        std::fs::write(&path, [0xff]).unwrap();
        assert!(tool
            .execute(serde_json::json!({"path":policy.paths[0]}))
            .await
            .is_err());
    }
}
