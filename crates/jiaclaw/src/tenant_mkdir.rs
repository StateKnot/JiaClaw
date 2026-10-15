// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Exact nonrecursive directory authority for the original mkdir engine.
use jiaclaw_core::{JiaClawError, TenantMkdirPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Permission metadata, never path existence, inode identity or a snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantMkdirCapability {
    /// Independent permission protocol; SSE remains unchanged.
    pub protocol: u8,
    /// Complete sorted exact directory names.
    pub paths: Vec<String>,
    /// Fixed nonrecursive mode, requiring an existing parent.
    pub mode: String,
    /// SHA-256 of canonical UTF-8 [1, paths, "single_directory"].
    pub policy_sha256: String,
}
impl TenantMkdirCapability {
    /// Capture administrator authority without filesystem I/O.
    ///
    /// # Errors
    /// Invalid grant or encoding failure.
    pub fn capture(policy: &TenantMkdirPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let raw = serde_json::to_vec(&(1_u8, &policy.paths, "single_directory"))
            .map_err(|_| JiaClawError::Configuration("tenant mkdir encoding failed".into()))?;
        Ok(Self {
            protocol: 1,
            paths: policy.paths.clone(),
            mode: "single_directory".into(),
            policy_sha256: format!("{:x}", Sha256::digest(raw)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    #[tokio::test]
    async fn tenant_mkdir_uses_original_writer_without_recursive_authority() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("tenant-documents")).unwrap();
        let paths = vec![
            "tenant-documents/deliveries".into(),
            "tenant-documents/missing/child".into(),
        ];
        let tool = crate::WorkspaceMkdirTool::restricted(root.path(), &TenantMkdirPolicy { paths })
            .unwrap();
        assert_eq!(tool.failure_effect(), crate::ToolFailureEffect::Unknown);
        for args in [
            serde_json::json!({"path":"tenant-documents/private"}),
            serde_json::json!({"path":"tenant-documents/./deliveries"}),
            serde_json::json!({"path":"tenant-documents/deliveries","recursive":false}),
            serde_json::json!({"path":"tenant-documents/deliveries","parents":false}),
            serde_json::json!({"path":"tenant-documents/missing/child"}),
        ] {
            assert!(tool.execute(args).await.is_err());
        }
        assert!(!root.path().join("tenant-documents/missing").exists());
        let args = serde_json::json!({"path":"tenant-documents/deliveries"});
        for created in [true, false] {
            let v: serde_json::Value =
                serde_json::from_str(&tool.execute(args.clone()).await.unwrap()).unwrap();
            assert_eq!(
                v,
                serde_json::json!({"path":"tenant-documents/deliveries","created":created,"existed":!created,"recursive":false})
            );
            assert!(root.path().join("tenant-documents/deliveries").is_dir());
        }
        std::fs::remove_dir(root.path().join("tenant-documents/deliveries")).unwrap();
        std::fs::write(root.path().join("tenant-documents/deliveries"), b"private").unwrap();
        assert!(tool.execute(args).await.is_err());
        assert_eq!(
            std::fs::read(root.path().join("tenant-documents/deliveries")).unwrap(),
            b"private"
        );
    }

    #[test]
    fn tenant_mkdir_permission_is_exact_and_canonical() {
        let p = TenantMkdirPolicy {
            paths: vec!["tenant-documents/a".into(), "tenant-documents/b".into()],
        };
        let cap = TenantMkdirCapability::capture(&p).unwrap();
        assert_eq!(
            cap.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&serde_json::json!([1, p.paths, "single_directory"]))
                        .unwrap()
                )
            )
        );
        for paths in [
            vec![],
            vec![p.paths[0].clone(); 33],
            vec![p.paths[1].clone(), p.paths[0].clone()],
        ] {
            assert!(TenantMkdirPolicy { paths }.validate().is_err());
        }
        for path in [
            "private",
            "tenant-documents/",
            "tenant-documents/.x",
            "tenant-documents/a//b",
            "tenant-documents/a/../b",
            "tenant-documents/ x",
            "tenant-documents/a\\b",
            "tenant-documents/a\u{feff}",
            "tenant-documents/a\n",
        ] {
            assert!(TenantMkdirPolicy {
                paths: vec![path.into()]
            }
            .validate()
            .is_err());
        }
        assert!(serde_json::from_value::<TenantMkdirPolicy>(
            serde_json::json!({"paths":p.paths,"recursive":false})
        )
        .is_err());
    }
}
