// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Explicit permission for the existing atomic file rename; never inode identity.
use jiaclaw_core::{JiaClawError, TenantMovePair, TenantMovePolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Exact file-only permission checked by backend, gateway and workbench.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantMoveCapability {
    /// Independent grant protocol; the original SSE protocol is unchanged.
    pub protocol: u8,
    /// Complete approved combinations, never recombined endpoints.
    pub pairs: Vec<TenantMovePair>,
    /// Fixed file-only no-replace mode.
    pub mode: String,
    /// SHA-256 of [1, [[from,to],...], "file_only_no_replace"].
    pub policy_sha256: String,
}
impl TenantMoveCapability {
    /// Capture authority without opening sources or contacting a model.
    ///
    /// # Errors
    /// Invalid grant or serialization failure.
    pub fn capture(policy: &TenantMovePolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let pairs: Vec<_> = policy.pairs.iter().map(|p| (&p.from, &p.to)).collect();
        let raw = serde_json::to_vec(&(1_u8, pairs, "file_only_no_replace"))
            .map_err(|_| JiaClawError::Configuration("tenant move encoding failed".into()))?;
        Ok(Self {
            protocol: 1,
            pairs: policy.pairs.clone(),
            mode: "file_only_no_replace".into(),
            policy_sha256: format!("{:x}", Sha256::digest(raw)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    fn policy() -> TenantMovePolicy {
        TenantMovePolicy {
            pairs: vec![
                TenantMovePair {
                    from: "tenant-documents/a.bin".into(),
                    to: "tenant-documents/b.bin".into(),
                },
                TenantMovePair {
                    from: "tenant-documents/c.bin".into(),
                    to: "tenant-documents/d.bin".into(),
                },
            ],
        }
    }

    #[tokio::test]
    async fn tenant_move_uses_original_rename_without_overwrite_or_directory_authority() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("tenant-documents")).unwrap();
        let policy = policy();
        let tool = crate::WorkspaceMoveTool::restricted(root.path(), &policy).unwrap();
        assert_eq!(tool.failure_effect(), crate::ToolFailureEffect::Unknown);
        let pair = &policy.pairs[0];
        let bytes = [0, 255, 254, 13, 10, 42];
        std::fs::write(root.path().join(&pair.from), bytes).unwrap();
        for bad in [
            serde_json::json!({"from":pair.from,"to":policy.pairs[1].to}),
            serde_json::json!({"from":pair.from,"to":pair.to,"overwrite":false}),
            serde_json::json!({"source":pair.from,"destination":pair.to}),
            serde_json::json!({"from":"tenant-documents/./a.bin","to":pair.to}),
        ] {
            assert!(tool.execute(bad).await.is_err());
            assert_eq!(std::fs::read(root.path().join(&pair.from)).unwrap(), bytes);
            assert!(!root.path().join(&pair.to).exists());
        }
        let args = serde_json::json!({"from":pair.from,"to":pair.to});
        let result: serde_json::Value =
            serde_json::from_str(&tool.execute(args.clone()).await.unwrap()).unwrap();
        assert_eq!(result["kind"], "file");
        assert_eq!(result["overwritten"], false);
        assert!(!root.path().join(&pair.from).exists());
        assert_eq!(std::fs::read(root.path().join(&pair.to)).unwrap(), bytes);
        std::fs::write(root.path().join(&pair.from), b"new source").unwrap();
        assert!(tool.execute(args.clone()).await.is_err());
        assert_eq!(std::fs::read(root.path().join(&pair.to)).unwrap(), bytes);
        assert_eq!(
            std::fs::read(root.path().join(&pair.from)).unwrap(),
            b"new source"
        );
        std::fs::remove_file(root.path().join(&pair.from)).unwrap();
        std::fs::remove_file(root.path().join(&pair.to)).unwrap();
        std::fs::create_dir(root.path().join(&pair.from)).unwrap();
        std::fs::write(root.path().join(&pair.from).join("private"), bytes).unwrap();
        assert!(tool.execute(args).await.is_err());
        assert!(root.path().join(&pair.from).join("private").exists());
        assert!(!root.path().join(&pair.to).exists());
    }

    #[test]
    fn tenant_move_permission_is_canonical_and_independent_from_copy() {
        let p = policy();
        let c = TenantMoveCapability::capture(&p).unwrap();
        let canonical = serde_json::json!([
            1,
            [
                [p.pairs[0].from, p.pairs[0].to],
                [p.pairs[1].from, p.pairs[1].to]
            ],
            "file_only_no_replace"
        ]);
        assert_eq!(
            c.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&canonical).unwrap())
            )
        );
        assert_ne!(
            c.policy_sha256,
            crate::TenantCopyCapability::capture(&jiaclaw_core::TenantCopyPolicy {
                pairs: p.pairs.clone()
            })
            .unwrap()
            .policy_sha256
        );
        for path in [
            "private.bin",
            "tenant-documents/.hidden",
            "tenant-documents/a/../b",
            "tenant-documents/a\\b",
            "tenant-documents/ a",
            "tenant-documents/a\u{feff}",
        ] {
            let mut bad = p.clone();
            bad.pairs[0].from = path.into();
            assert!(TenantMoveCapability::capture(&bad).is_err());
        }
        for pairs in [
            vec![],
            vec![p.pairs[0].clone(); 9],
            vec![p.pairs[1].clone(), p.pairs[0].clone()],
            vec![TenantMovePair {
                from: p.pairs[0].from.clone(),
                to: p.pairs[0].from.clone(),
            }],
        ] {
            assert!(TenantMovePolicy { pairs }.validate().is_err());
        }
        assert!(serde_json::from_value::<TenantMovePolicy>(
            serde_json::json!({"pairs":p.pairs,"overwrite":false})
        )
        .is_err());
    }
}
