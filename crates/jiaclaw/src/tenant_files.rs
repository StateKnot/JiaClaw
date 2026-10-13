// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! One canonical permission descriptor shared by private backend and gateway.
use jiaclaw_core::{JiaClawError, TenantFileReadPolicy, TenantFileWritePolicy};
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

/// Separate write authorization; this hash is a permission, not file-content identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantFileWriteCapability {
    /// Exact permission protocol, independent of SSE protocol 2.
    pub protocol: u8,
    /// Complete exact document target list.
    pub paths: Vec<String>,
    /// Only whole-file overwrite is authorized; append is unavailable.
    pub mode: String,
    /// Original write_file full-content byte limit.
    pub max_bytes: usize,
    /// SHA-256 of [1, paths, "overwrite", max_bytes].
    pub policy_sha256: String,
}
impl TenantFileWriteCapability {
    /// Capture validated write authority without opening files or calling a model.
    ///
    /// # Errors
    /// Invalid document permission or encoding failure.
    pub fn capture(policy: &TenantFileWritePolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let bytes = serde_json::to_vec(&(
            1_u8,
            &policy.paths,
            "overwrite",
            crate::WRITE_FILE_MAX_BYTES,
        ))
        .map_err(|_| {
            JiaClawError::Configuration("tenant write permission encoding failed".into())
        })?;
        Ok(Self {
            protocol: 1,
            paths: policy.paths.clone(),
            mode: "overwrite".into(),
            max_bytes: crate::WRITE_FILE_MAX_BYTES,
            policy_sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn tenant_file_capture_uses_original_complete_reader_and_exact_grant() {
        use crate::Tool;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("tenant-documents")).unwrap();
        let policy = TenantFileReadPolicy {
            paths: vec!["tenant-documents/output.md".into()],
        };
        let target = root.path().join(&policy.paths[0]);
        let tool = crate::FileReadTool::restricted(root.path(), &policy).unwrap();
        let original = "\u{feff}完整🦀\r\n\t tail  \r\n";
        std::fs::write(&target, original).unwrap();
        assert_eq!(
            tool.capture_text_owned(policy.paths[0].clone(), ())
                .await
                .unwrap(),
            original
        );
        let old = tool
            .execute(serde_json::json!({"path":policy.paths[0]}))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&old).unwrap()["content"],
            "\u{feff}完整🦀\n\t tail  "
        );
        assert!(tool
            .capture_text_owned("private.txt".into(), ())
            .await
            .is_err());
        assert!(crate::FileReadTool::new(root.path())
            .capture_text_owned(policy.paths[0].clone(), ())
            .await
            .is_err());
        for bytes in [vec![], vec![b'x'; crate::READ_FILE_MAX_BYTES]] {
            std::fs::write(&target, &bytes).unwrap();
            assert_eq!(
                tool.capture_text_owned(policy.paths[0].clone(), ())
                    .await
                    .unwrap()
                    .as_bytes(),
                bytes
            );
        }
        for bytes in [
            vec![b'x'; crate::READ_FILE_MAX_BYTES + 1],
            vec![0],
            vec![255],
        ] {
            std::fs::write(&target, bytes).unwrap();
            assert!(tool
                .capture_text_owned(policy.paths[0].clone(), ())
                .await
                .is_err());
        }
    }
    #[tokio::test]
    async fn tenant_write_uses_original_atomic_writer_and_exact_complete_content() {
        use crate::Tool;
        let root = tempfile::tempdir().unwrap();
        let policy = TenantFileWritePolicy {
            paths: vec!["tenant-documents/report.md".into()],
        };
        let tool = crate::FileWriteTool::restricted(root.path(), &policy).unwrap();
        let permission = TenantFileWriteCapability::capture(&policy).unwrap();
        assert_eq!(permission.mode, "overwrite");
        assert_ne!(
            permission.policy_sha256,
            TenantFileReadCapability::capture(&TenantFileReadPolicy {
                paths: policy.paths.clone()
            })
            .unwrap()
            .policy_sha256
        );
        assert_eq!(tool.failure_effect(), crate::ToolFailureEffect::Unknown);
        let target = root.path().join(&policy.paths[0]);
        let content = "\u{feff}资料🦀\r\n\0\t end ";
        let output = tool
            .execute(serde_json::json!({"path":policy.paths[0],"content":content}))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), content.as_bytes());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).unwrap()["bytes_written"],
            content.len()
        );
        for bad in [
            serde_json::json!({"path":"MEMORY.md","content":"bad"}),
            serde_json::json!({"path":"tenant-documents/./report.md","content":"bad"}),
            serde_json::json!({"path":policy.paths[0],"content":"bad","mode":"append"}),
            serde_json::json!({"path":policy.paths[0],"content":"bad","extra":null}),
            serde_json::json!({"path":policy.paths[0],"content":"🦀".repeat(crate::WRITE_FILE_MAX_BYTES / 4 + 1)}),
        ] {
            assert!(tool.execute(bad).await.is_err());
            assert_eq!(std::fs::read(&target).unwrap(), content.as_bytes());
        }
        tool.execute(serde_json::json!({"path":policy.paths[0],"content":"x".repeat(crate::WRITE_FILE_MAX_BYTES)})).await.unwrap();
        assert_eq!(
            std::fs::read(&target).unwrap().len(),
            crate::WRITE_FILE_MAX_BYTES
        );
        tool.execute(serde_json::json!({"path":policy.paths[0],"content":""}))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"");
        std::fs::remove_file(&target).unwrap();
        std::fs::write(root.path().join("protected"), "ORIGINAL").unwrap();
        std::fs::hard_link(root.path().join("protected"), &target).unwrap();
        assert!(tool
            .execute(serde_json::json!({"path":policy.paths[0],"content":"bad"}))
            .await
            .is_err());
        assert_eq!(
            std::fs::read(root.path().join("protected")).unwrap(),
            b"ORIGINAL"
        );
    }

    #[test]
    fn tenant_write_policy_is_separate_default_off_and_uses_original_path_rules() {
        assert!(jiaclaw_core::HttpConfig::default()
            .tenant_file_write
            .is_none());
        for path in [
            "MEMORY.md",
            "tenant-documents/.a",
            "tenant-documents/a//b",
            "tenant-documents/../a",
            "tenant-documents/\u{feff}a",
            "tenant-documents/a ",
            "tenant-documents/a\\b",
        ] {
            assert!(TenantFileWriteCapability::capture(&TenantFileWritePolicy {
                paths: vec![path.into()]
            })
            .is_err());
        }
    }
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
