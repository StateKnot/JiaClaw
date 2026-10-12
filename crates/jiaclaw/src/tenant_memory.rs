// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Keyword permission metadata; reads and matching remain in MemorySearchTool.
use jiaclaw_core::{JiaClawError, TenantMemorySearchPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Complete default-off explicit keyword-search permission, independent of SSE.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantMemorySearchCapability {
    /// Exact permission protocol.
    pub protocol: u8,
    /// Complete configured source subset; never source text.
    pub paths: Vec<String>,
    /// Keyword only, without embedding requests.
    pub mode: String,
    /// Complete per-source capture bound.
    pub max_source_bytes: usize,
    /// Complete aggregate capture bound.
    pub max_total_bytes: usize,
    /// Query UTF8 byte bound.
    pub max_query_bytes: usize,
    /// Result count bound.
    pub max_results: usize,
    /// Per-excerpt UTF8 byte bound.
    pub max_excerpt_bytes: usize,
    /// Serialized search result bound.
    pub max_result_bytes: usize,
    /// Canonical permission hash, not source content, readiness or a generation.
    pub policy_sha256: String,
}
impl TenantMemorySearchCapability {
    /// Capture complete static permission without source or provider access.
    ///
    /// # Errors
    /// Invalid bounded permission or encoding failure.
    pub fn capture(policy: &TenantMemorySearchPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let bytes = serde_json::to_vec(&(
            1_u8,
            &policy.paths,
            "keyword",
            crate::memory::MEMORY_SEARCH_FILE_MAX_BYTES,
            3 * crate::memory::MEMORY_SEARCH_FILE_MAX_BYTES,
            crate::memory::MEMORY_SEARCH_QUERY_MAX_BYTES,
            crate::memory::MEMORY_SEARCH_MAX_RESULTS,
            crate::memory::MEMORY_SEARCH_EXCERPT_MAX_BYTES,
            65536_usize,
        ))
        .map_err(|_| JiaClawError::Configuration("memory permission encoding failed".into()))?;
        Ok(Self {
            protocol: 1,
            paths: policy.paths.clone(),
            mode: "keyword".into(),
            max_source_bytes: crate::memory::MEMORY_SEARCH_FILE_MAX_BYTES,
            max_total_bytes: 3 * crate::memory::MEMORY_SEARCH_FILE_MAX_BYTES,
            max_query_bytes: crate::memory::MEMORY_SEARCH_QUERY_MAX_BYTES,
            max_results: crate::memory::MEMORY_SEARCH_MAX_RESULTS,
            max_excerpt_bytes: crate::memory::MEMORY_SEARCH_EXCERPT_MAX_BYTES,
            max_result_bytes: 65536,
            policy_sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tenant_memory_permission_is_complete_bounded_and_configured() {
        let policy = TenantMemorySearchPolicy {
            paths: vec!["MEMORY.md".into()],
        };
        let capability = TenantMemorySearchCapability::capture(&policy).unwrap();
        assert_eq!(capability.max_total_bytes, 1572864);
        assert_eq!(capability.mode, "keyword");
        assert_ne!(
            capability.policy_sha256,
            TenantMemorySearchCapability::capture(&TenantMemorySearchPolicy {
                paths: vec!["USER.md".into()]
            })
            .unwrap()
            .policy_sha256
        );
        for paths in [
            vec![],
            vec!["MEMORY.md", "MEMORY.md"],
            vec!["USER.md", "MEMORY.md"],
            vec!["./MEMORY.md"],
            vec!["a//b"],
            vec!["a\\b"],
            vec!["a\nb"],
            vec!["\u{feff}MEMORY.md"],
            vec!["a", "b", "c", "d"],
        ] {
            assert!(
                TenantMemorySearchCapability::capture(&TenantMemorySearchPolicy {
                    paths: paths.into_iter().map(str::to_owned).collect()
                })
                .is_err()
            );
        }
        let ws = tempfile::tempdir().unwrap();
        let mut config = jiaclaw_core::AgentConfig {
            workspace_path: ws.path().into(),
            ..Default::default()
        };
        config.http.tracked_turns = true;
        config.http.gateway_channel_chat = true;
        config.http.tenant_memory_search = Some(policy);
        assert!(crate::JiaClawAgent::new(config.clone()).is_ok());
        for mode in 0..4 {
            let mut invalid = config.clone();
            match mode {
                0 => invalid.tools.memory_search.enabled = false,
                1 => invalid.http.tracked_turns = false,
                2 => invalid.http.gateway_channel_chat = false,
                _ => {
                    invalid.http.tenant_memory_search.as_mut().unwrap().paths =
                        vec!["private.txt".into()]
                }
            }
            assert!(crate::JiaClawAgent::new(invalid).is_err());
        }
    }
}
