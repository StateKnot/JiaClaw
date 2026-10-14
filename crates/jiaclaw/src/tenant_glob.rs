// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Independent permission for the original bounded filename scanner; no source I/O.
use jiaclaw_core::{JiaClawError, TenantGlobPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Complete default-off filename grant; does not grant body reads or tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantGlobCapability {
    /// Independent permission protocol; SSE remains protocol 2.
    pub protocol: u8,
    /// Exact selectable roots; ordinary filenames throughout these subtrees are authorized.
    pub roots: Vec<String>,
    /// Complete raw UTF8 pattern budget.
    pub max_pattern_bytes: usize,
    /// Maximum returned file paths.
    pub max_results: usize,
    /// Original relative traversal depth.
    pub max_depth: usize,
    /// Original all-entry scan limit, including unmatched entries.
    pub max_scanned: usize,
    /// Original cooperative deadline; not OS thread interruption.
    pub scan_budget_ms: usize,
    /// Complete pretty JSON output budget.
    pub max_result_bytes: usize,
    /// SHA256 of [1,roots,256,500,32,2000,2000,65536]; not content identity.
    pub policy_sha256: String,
}
impl TenantGlobCapability {
    /// Validate and capture permission without probing directories.
    ///
    /// # Errors
    /// Invalid directory grant or serialization failure.
    pub fn capture(policy: &TenantGlobPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let canonical = (
            1_u8,
            &policy.roots,
            crate::files::GLOB_PATTERN_MAX_BYTES,
            crate::files::GLOB_MAX_RESULTS,
            crate::memory_io::DIRECTORY_DEPTH_LIMIT,
            crate::memory_io::DIRECTORY_SCAN_LIMIT,
            crate::memory_io::DIRECTORY_SCAN_BUDGET_MS,
            crate::memory_io::SEARCH_OUTPUT_BYTES,
        );
        let encoded = serde_json::to_vec(&canonical).map_err(|_| {
            JiaClawError::Configuration("tenant glob permission encoding failed".into())
        })?;
        Ok(Self {
            protocol: canonical.0,
            roots: policy.roots.clone(),
            max_pattern_bytes: canonical.2,
            max_results: canonical.3,
            max_depth: canonical.4,
            max_scanned: canonical.5,
            scan_budget_ms: canonical.6,
            max_result_bytes: canonical.7,
            policy_sha256: format!("{:x}", Sha256::digest(encoded)),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, WorkspaceGlobTool};
    use serde_json::json;

    #[test]
    fn tenant_glob_permission_is_independent_canonical_and_bounded() {
        assert!(jiaclaw_core::HttpConfig::default().tenant_glob.is_none());
        let policy = TenantGlobPolicy {
            roots: vec!["tenant-documents/a".into()],
        };
        let cap = TenantGlobCapability::capture(&policy).unwrap();
        assert_eq!(
            cap.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&json!([1, policy.roots, 256, 500, 32, 2000, 2000, 65536]))
                        .unwrap()
                )
            )
        );
        for roots in [
            vec![],
            vec!["."],
            vec!["tenant-documents/a/"],
            vec!["tenant-documents/.a"],
            vec!["tenant-documents/a", "tenant-documents/a/b"],
            vec!["tenant-documents/b", "tenant-documents/a"],
            vec!["tenant-documents/a", "tenant-documents/a"],
            vec!["tenant-documents/a\u{85}"],
        ] {
            assert!(TenantGlobCapability::capture(&TenantGlobPolicy {
                roots: roots.into_iter().map(str::to_owned).collect()
            })
            .is_err());
        }
    }

    #[tokio::test]
    async fn tenant_glob_uses_original_scanner_with_exact_directory_and_raw_pattern() {
        let ws = tempfile::tempdir().unwrap();
        let root = "tenant-documents/a";
        std::fs::create_dir_all(ws.path().join(root).join("nested")).unwrap();
        std::fs::write(ws.path().join(root).join("nested/data.bin"), [255, 0, 254]).unwrap();
        std::fs::write(ws.path().join(root).join(".hidden"), "PRIVATE BODY").unwrap();
        std::fs::write(ws.path().join("secret"), "OUTSIDE BODY").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(ws.path().join("secret"), ws.path().join(root).join("link"))
                .unwrap();
            std::fs::hard_link(ws.path().join("secret"), ws.path().join(root).join("hard"))
                .unwrap();
        }
        let tool = WorkspaceGlobTool::restricted(
            ws.path(),
            &TenantGlobPolicy {
                roots: vec![root.into()],
            },
        )
        .unwrap();
        let schema = jsonschema::validator_for(&tool.parameters_schema()).unwrap();
        for args in [
            json!({}),
            json!({"path":".","pattern":"*"}),
            json!({"path":format!("{root}/nested"),"pattern":"*"}),
            json!({"path":root,"pattern":"*","max_results":0}),
            json!({"path":root,"pattern":"*","max_results":501}),
            json!({"path":root,"pattern":"*","max_results":1.0}),
            json!({"path":root,"pattern":"*","max_results":null}),
            json!({"path":root,"pattern":"*","extra":true}),
        ] {
            assert!(!schema.is_valid(&args) || args["max_results"].is_f64());
            assert!(tool.execute(args).await.is_err());
        }
        for pattern in [
            " ".into(),
            "*\0".into(),
            format!("{}*", " ".repeat(256)),
            "界".repeat(86),
        ] {
            assert!(tool
                .execute(json!({"path":root,"pattern":pattern}))
                .await
                .is_err());
        }
        let args = json!({"path":root,"pattern":"*"});
        let actual = tool.execute(args.clone()).await.unwrap();
        assert_eq!(
            actual,
            WorkspaceGlobTool::new(ws.path())
                .execute(args)
                .await
                .unwrap()
        );
        let result: serde_json::Value = serde_json::from_str(&actual).unwrap();
        assert_eq!(
            result["matches"],
            json!([format!("{root}/.hidden"), format!("{root}/nested/data.bin")])
        );
        assert!(!actual.contains("BODY"));
        let limited: serde_json::Value = serde_json::from_str(
            &tool
                .execute(json!({"path":root,"pattern":"*","max_results":1}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(limited["truncated"], true);
        assert_eq!(limited["match_count"], 1);
        let parked = ws.path().join("parked");
        std::fs::rename(ws.path().join(root), &parked).unwrap();
        std::fs::write(
            ws.path().join(root),
            "ordinary file is not a directory grant",
        )
        .unwrap();
        assert!(tool
            .execute(json!({"path":root,"pattern":"*"}))
            .await
            .is_err());
        std::fs::remove_file(ws.path().join(root)).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&parked, ws.path().join(root)).unwrap();
            assert!(tool
                .execute(json!({"path":root,"pattern":"*"}))
                .await
                .is_err());
        }
    }
}
