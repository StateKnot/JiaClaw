// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Permission metadata for the original bounded tree walker; no source I/O.
use jiaclaw_core::{JiaClawError, TenantTreePolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Complete default-off subtree permission, independent of file-content reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantTreeCapability {
    /// Exact permission protocol, independent of SSE protocol 2.
    pub protocol: u8,
    /// Exact selectable roots; their entire subtree metadata is authorized.
    pub roots: Vec<String>,
    /// Original maximum depth.
    pub max_depth: usize,
    /// Original returned-entry limit.
    pub max_entries: usize,
    /// Original shared scanned-entry limit.
    pub max_scanned: usize,
    /// Original cooperative scan budget, not a hard thread interruption.
    pub scan_budget_ms: usize,
    /// Original full serialized JSON limit.
    pub max_result_bytes: usize,
    /// SHA256 of [1, roots, 32, 1000, 2000, 2000, 65536]; not content identity.
    pub policy_sha256: String,
}
impl TenantTreeCapability {
    /// Validate and capture permission without opening a directory.
    ///
    /// # Errors
    /// Invalid policy or serialization failure.
    pub fn capture(policy: &TenantTreePolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let canonical = (
            1_u8,
            &policy.roots,
            32_usize,
            1000_usize,
            2000_usize,
            2000_usize,
            65536_usize,
        );
        let encoded = serde_json::to_vec(&canonical).map_err(|_| {
            JiaClawError::Configuration("tenant tree permission encoding failed".into())
        })?;
        Ok(Self {
            protocol: 1,
            roots: policy.roots.clone(),
            max_depth: 32,
            max_entries: 1000,
            max_scanned: 2000,
            scan_budget_ms: 2000,
            max_result_bytes: 65536,
            policy_sha256: format!("{:x}", Sha256::digest(encoded)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, WorkspaceTreeTool};
    use serde_json::json;

    #[test]
    fn tenant_tree_permission_is_explicit_bounded_and_canonical() {
        assert!(jiaclaw_core::HttpConfig::default().tenant_tree.is_none());
        let valid = TenantTreePolicy {
            roots: vec!["tenant-documents/project".into()],
        };
        let cap = TenantTreeCapability::capture(&valid).unwrap();
        assert_eq!(
            cap.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&json!([1, valid.roots, 32, 1000, 2000, 2000, 65536]))
                        .unwrap()
                )
            )
        );
        for roots in [
            vec![],
            vec!["."],
            vec!["tenant-documents/a/../b"],
            vec!["tenant-documents/.x"],
            vec!["tenant-documents/a", "tenant-documents/a/b"],
            vec!["tenant-documents/a", "tenant-documents/a"],
            vec!["tenant-documents/a\\b"],
            vec!["tenant-documents/a\u{85}"],
            vec!["tenant-documents/a/"],
        ] {
            assert!(TenantTreeCapability::capture(&TenantTreePolicy {
                roots: roots.into_iter().map(str::to_owned).collect()
            })
            .is_err());
        }
    }

    #[tokio::test]
    async fn tenant_tree_uses_original_walker_without_reading_or_following_links() {
        let ws = tempfile::tempdir().unwrap();
        let root = "tenant-documents/project";
        std::fs::create_dir_all(ws.path().join(root).join("nested")).unwrap();
        std::fs::write(ws.path().join(root).join("data.bin"), [255, 0, 254]).unwrap();
        std::fs::write(ws.path().join(root).join(".hidden"), "private body").unwrap();
        std::fs::write(ws.path().join("secret"), "EXTERNAL CONTENT").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(ws.path().join("secret"), ws.path().join(root).join("link"))
                .unwrap();
            std::fs::hard_link(ws.path().join("secret"), ws.path().join(root).join("hard"))
                .unwrap();
        }
        let tool = WorkspaceTreeTool::restricted(
            ws.path(),
            &TenantTreePolicy {
                roots: vec![root.into()],
            },
        )
        .unwrap();
        let schema = jsonschema::validator_for(&tool.parameters_schema()).unwrap();
        for args in [
            json!({}),
            json!({"path":"."}),
            json!({"path":"tenant-documents/project/nested"}),
            json!({"path":"tenant-documents/project/"}),
            json!({"path":root,"max_depth":33}),
            json!({"path":root,"max_entries":1001}),
            json!({"path":root,"extra":true}),
        ] {
            assert!(!schema.is_valid(&args));
            assert!(tool.execute(args).await.is_err());
        }
        let args = json!({"path":root,"max_depth":2,"max_entries":1000});
        let actual = tool.execute(args.clone()).await.unwrap();
        assert_eq!(
            actual,
            WorkspaceTreeTool::new(ws.path())
                .execute(args)
                .await
                .unwrap()
        );
        assert!(!actual.contains("EXTERNAL CONTENT"));
        assert!(actual.contains(".hidden"));
        assert!(actual.contains("unsupported"));
        let limited: serde_json::Value = serde_json::from_str(
            &tool
                .execute(json!({"path":root,"max_entries":1}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(limited["truncated"], true);
        assert_eq!(limited["entries"].as_array().unwrap().len(), 1);
        #[cfg(unix)]
        {
            let parked = ws.path().join("parked");
            std::fs::rename(ws.path().join("tenant-documents"), &parked).unwrap();
            std::os::unix::fs::symlink(&parked, ws.path().join("tenant-documents")).unwrap();
            assert!(tool.execute(json!({"path":root})).await.is_err());
        }
    }
}
