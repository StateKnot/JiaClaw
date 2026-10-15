// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Permission identity for the existing create-only copy tool; never file identity.
use jiaclaw_core::{JiaClawError, TenantCopyPair, TenantCopyPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Complete authority independently verified by backend, gateway and workbench.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantCopyCapability {
    /// Independent permission protocol; the SSE protocol remains unchanged.
    pub protocol: u8,
    /// Exact paired paths, never a Cartesian product of sources and destinations.
    pub pairs: Vec<TenantCopyPair>,
    /// Only atomic create-if-absent is authorized.
    pub mode: String,
    /// Original full binary source budget, enforced while copying.
    pub max_bytes: u64,
    /// SHA-256 of [1, [[from,to],...], "create_only", 67108864].
    pub policy_sha256: String,
}
impl TenantCopyCapability {
    /// Capture a validated grant without opening files or contacting a model.
    ///
    /// # Errors
    /// Invalid permission or serialization failure.
    pub fn capture(policy: &TenantCopyPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let pairs: Vec<_> = policy.pairs.iter().map(|p| (&p.from, &p.to)).collect();
        let raw = serde_json::to_vec(&(1_u8, pairs, "create_only", crate::COPY_MAX_BYTES))
            .map_err(|_| JiaClawError::Configuration("tenant copy encoding failed".into()))?;
        Ok(Self {
            protocol: 1,
            pairs: policy.pairs.clone(),
            mode: "create_only".into(),
            max_bytes: crate::COPY_MAX_BYTES,
            policy_sha256: format!("{:x}", Sha256::digest(raw)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    fn policy() -> TenantCopyPolicy {
        TenantCopyPolicy {
            pairs: vec![
                TenantCopyPair {
                    from: "tenant-documents/a.bin".into(),
                    to: "tenant-documents/b.bin".into(),
                },
                TenantCopyPair {
                    from: "tenant-documents/c.bin".into(),
                    to: "tenant-documents/d.bin".into(),
                },
            ],
        }
    }

    #[tokio::test]
    async fn tenant_copy_uses_original_binary_engine_and_exact_pairs() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("tenant-documents")).unwrap();
        let p = policy();
        let tool = crate::WorkspaceCopyTool::restricted(root.path(), &p).unwrap();
        assert_eq!(tool.failure_effect(), crate::ToolFailureEffect::Unknown);
        let bytes = [0, 255, 254, 13, 10, 42];
        std::fs::write(root.path().join(&p.pairs[0].from), bytes).unwrap();
        std::fs::write(root.path().join(&p.pairs[1].from), b"other").unwrap();
        for bad in [
            serde_json::json!({"from":p.pairs[0].from,"to":p.pairs[1].to}),
            serde_json::json!({"from":p.pairs[0].from,"to":p.pairs[0].to,"overwrite":false}),
            serde_json::json!({"source":p.pairs[0].from,"destination":p.pairs[0].to}),
            serde_json::json!({"from":"tenant-documents/./a.bin","to":p.pairs[0].to}),
        ] {
            assert!(tool.execute(bad).await.is_err());
            assert!(!root.path().join(&p.pairs[0].to).exists());
            assert!(!root.path().join(&p.pairs[1].to).exists());
        }
        let args = serde_json::json!({"from":p.pairs[0].from,"to":p.pairs[0].to});
        let output: serde_json::Value =
            serde_json::from_str(&tool.execute(args.clone()).await.unwrap()).unwrap();
        assert_eq!(output["bytes"], bytes.len());
        assert_eq!(output["overwritten"], false);
        assert_eq!(
            std::fs::read(root.path().join(&p.pairs[0].from)).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(root.path().join(&p.pairs[0].to)).unwrap(),
            bytes
        );
        std::fs::write(root.path().join(&p.pairs[0].from), b"changed").unwrap();
        assert!(tool.execute(args).await.is_err());
        assert_eq!(
            std::fs::read(root.path().join(&p.pairs[0].to)).unwrap(),
            bytes
        );
    }

    #[test]
    fn tenant_copy_permission_is_canonical_and_not_file_identity() {
        let p = policy();
        let c = TenantCopyCapability::capture(&p).unwrap();
        assert_eq!(
            c.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&serde_json::json!([
                        1,
                        [
                            [p.pairs[0].from, p.pairs[0].to],
                            [p.pairs[1].from, p.pairs[1].to]
                        ],
                        "create_only",
                        67108864
                    ]))
                    .unwrap()
                )
            )
        );
        assert!(TenantCopyPolicy { pairs: vec![] }.validate().is_err());
        let mut bad = p.clone();
        bad.pairs.reverse();
        assert!(bad.validate().is_err());
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
            assert!(bad.validate().is_err());
        }
        let mut same = p.clone();
        same.pairs[0].to = same.pairs[0].from.clone();
        assert!(same.validate().is_err());
        assert!(serde_json::from_value::<TenantCopyPolicy>(
            serde_json::json!({"pairs":p.pairs,"overwrite":true})
        )
        .is_err());
    }
}
