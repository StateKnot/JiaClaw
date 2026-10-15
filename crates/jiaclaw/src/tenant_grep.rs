// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Independent content permission for the original bounded literal scanner; no source I/O here.
use jiaclaw_core::{JiaClawError, TenantGrepPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Complete default-off subtree body-search grant. Other document permissions remain separate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantGrepCapability {
    /// Independent permission protocol; SSE remains protocol 2.
    pub protocol: u8,
    /// Exact selectable directory roots, including hidden ordinary file bodies below them.
    pub roots: Vec<String>,
    /// Complete raw UTF8 literal budget, without trimming.
    pub max_pattern_bytes: usize,
    /// Optional filename/path glob raw UTF8 budget.
    pub max_glob_bytes: usize,
    /// Original per-file read limit; larger files are skipped.
    pub max_file_bytes: usize,
    /// Original aggregate actual-byte read limit.
    pub max_read_bytes: usize,
    /// Maximum returned matching lines.
    pub max_matches: usize,
    /// Original snippet Unicode character limit, before an optional ellipsis.
    pub max_snippet_chars: usize,
    /// Original relative traversal depth.
    pub max_depth: usize,
    /// Original all-entry scan budget.
    pub max_scanned: usize,
    /// Cooperative deadline, not OS thread interruption.
    pub scan_budget_ms: usize,
    /// Complete pretty JSON output budget.
    pub max_result_bytes: usize,
    /// SHA256 of the complete ordered protocol/roots/budgets tuple; not content identity.
    pub policy_sha256: String,
}
impl TenantGrepCapability {
    /// Validate and capture permission without probing any file or directory.
    /// # Errors
    /// Invalid directory grant or serialization failure.
    pub fn capture(policy: &TenantGrepPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let mut value = Self {
            protocol: 1,
            roots: policy.roots.clone(),
            max_pattern_bytes: crate::files::GREP_PATTERN_MAX_BYTES,
            max_glob_bytes: crate::files::GREP_GLOB_MAX_BYTES,
            max_file_bytes: crate::files::GREP_FILE_MAX_BYTES,
            max_read_bytes: crate::memory_io::SEARCH_READ_BYTES,
            max_matches: crate::files::GREP_MAX_MATCHES,
            max_snippet_chars: crate::files::GREP_SNIPPET_MAX_CHARS,
            max_depth: crate::memory_io::DIRECTORY_DEPTH_LIMIT,
            max_scanned: crate::memory_io::DIRECTORY_SCAN_LIMIT,
            scan_budget_ms: crate::memory_io::DIRECTORY_SCAN_BUDGET_MS,
            max_result_bytes: crate::memory_io::SEARCH_OUTPUT_BYTES,
            policy_sha256: String::new(),
        };
        let encoded = serde_json::to_vec(&serde_json::json!([
            value.protocol,
            value.roots,
            value.max_pattern_bytes,
            value.max_glob_bytes,
            value.max_file_bytes,
            value.max_read_bytes,
            value.max_matches,
            value.max_snippet_chars,
            value.max_depth,
            value.max_scanned,
            value.scan_budget_ms,
            value.max_result_bytes
        ]))
        .map_err(|_| {
            JiaClawError::Configuration("tenant grep permission encoding failed".into())
        })?;
        value.policy_sha256 = format!("{:x}", Sha256::digest(encoded));
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, WorkspaceGrepTool};
    use serde_json::json;

    #[test]
    fn tenant_grep_permission_is_independent_canonical_and_bounded() {
        assert!(jiaclaw_core::HttpConfig::default().tenant_grep.is_none());
        let roots = vec!["tenant-documents/a".into()];
        let cap = TenantGrepCapability::capture(&TenantGrepPolicy {
            roots: roots.clone(),
        })
        .unwrap();
        let expected =
            json!([1, roots, 512, 128, 262144, 16777216, 200, 200, 32, 2000, 2000, 65536]);
        assert_eq!(
            cap.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&expected).unwrap())
            )
        );
        for roots in [
            vec![],
            vec!["."],
            vec!["tenant-documents/.a"],
            vec!["tenant-documents/a/"],
            vec!["tenant-documents/a", "tenant-documents/a/b"],
            vec!["tenant-documents/b", "tenant-documents/a"],
        ] {
            assert!(TenantGrepCapability::capture(&TenantGrepPolicy {
                roots: roots.into_iter().map(str::to_owned).collect()
            })
            .is_err());
        }
    }

    #[tokio::test]
    async fn tenant_grep_uses_original_reader_with_exact_directory_and_raw_literal() {
        let ws = tempfile::tempdir().unwrap();
        let root = "tenant-documents/a";
        let directory = ws.path().join(root);
        std::fs::create_dir_all(directory.join("nested")).unwrap();
        std::fs::write(
            directory.join("nested/report.txt"),
            "heading\n  Needle literal.*\n尾部 Needle\n",
        )
        .unwrap();
        std::fs::write(directory.join(".hidden"), "Needle hidden").unwrap();
        std::fs::write(directory.join("binary"), b"Needle\0private").unwrap();
        std::fs::write(directory.join("oversized"), "Needle".repeat(50000)).unwrap();
        std::fs::write(ws.path().join("secret"), "Needle OUTSIDE").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(ws.path().join("secret"), directory.join("link")).unwrap();
            std::fs::hard_link(ws.path().join("secret"), directory.join("hard")).unwrap();
        }
        let tool = WorkspaceGrepTool::restricted(
            ws.path(),
            &TenantGrepPolicy {
                roots: vec![root.into()],
            },
        )
        .unwrap();
        let schema = jsonschema::validator_for(&tool.parameters_schema()).unwrap();
        for args in [
            json!({}),
            json!({"path":".","pattern":"Needle"}),
            json!({"path":format!("{root}/nested"),"pattern":"Needle"}),
            json!({"path":root,"pattern":"Needle","max_matches":201}),
            json!({"path":root,"pattern":"Needle","max_matches":1.0}),
            json!({"path":root,"pattern":"Needle","glob":null}),
            json!({"path":root,"pattern":"Needle","case_insensitive":null}),
            json!({"path":root,"pattern":"Needle","extra":true}),
        ] {
            assert!(!schema.is_valid(&args) || args["max_matches"].is_f64());
            assert!(tool.execute(args).await.is_err());
        }
        for args in [
            json!({"path":root,"pattern":"界".repeat(171)}),
            json!({"path":root,"pattern":"x\n"}),
            json!({"path":root,"pattern":"x","glob":" *.txt"}),
            json!({"path":root,"pattern":"x","glob":"界".repeat(43)}),
        ] {
            assert!(tool.execute(args).await.is_err());
        }
        let args = json!({"path":root,"pattern":"Needle"});
        let actual = tool.execute(args.clone()).await.unwrap();
        assert_eq!(
            actual,
            WorkspaceGrepTool::new(ws.path())
                .execute(args)
                .await
                .unwrap()
        );
        let value: serde_json::Value = serde_json::from_str(&actual).unwrap();
        assert_eq!(value["match_count"], 3);
        assert_eq!(value["truncated"], false);
        assert!(!actual.contains("OUTSIDE"));
        assert!(!actual.contains("oversized"));
        let literal: serde_json::Value = serde_json::from_str(
            &tool
                .execute(json!({"path":root,"pattern":"  Needle literal.*","glob":"*.txt"}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(literal["match_count"], 1);
        assert_eq!(literal["matches"][0]["line"], 2);
        for pattern in ["x".repeat(512), "界".repeat(170) + "xx"] {
            assert!(tool
                .execute(json!({"path":root,"pattern":pattern}))
                .await
                .is_ok());
        }
        let limited: serde_json::Value = serde_json::from_str(
            &tool
                .execute(
                    json!({"path":root,"pattern":"needle","case_insensitive":true,"max_matches":1}),
                )
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(limited["truncated"], true);
        assert_eq!(limited["match_count"], 1);
        std::fs::rename(&directory, ws.path().join("parked")).unwrap();
        std::fs::write(&directory, "Needle must not become a file grant").unwrap();
        assert!(tool
            .execute(json!({"path":root,"pattern":"Needle"}))
            .await
            .is_err());
        assert!(WorkspaceGrepTool::new(ws.path())
            .execute(json!({"path":root,"pattern":"Needle"}))
            .await
            .is_ok());
    }
}
