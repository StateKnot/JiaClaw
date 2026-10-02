// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Operator-owned MCP policy. Remote annotations do not grant authority.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reviewed remote servers. No endpoint is contacted by default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// At most eight explicitly configured servers.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// A fixed Streamable HTTP destination and finite resource policy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// Unique ASCII identifier used in local tool names.
    pub name: String,
    /// HTTPS URL, or literal-loopback HTTP URL for a local service.
    pub endpoint: String,
    /// Optional environment variable containing a bearer token; no literal token field.
    pub bearer_token_env: Option<String>,
    /// Total startup and individual call deadline, including queueing (1..=120 seconds).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Complete JSON response and SSE aggregate ceiling (1 KiB..=1 MiB).
    #[serde(default = "default_response_bytes")]
    pub max_response_bytes: usize,
    /// Shared per-server call concurrency (1..=16); excess calls fail without submission.
    #[serde(default = "default_concurrency")]
    pub max_concurrent_calls: usize,
    /// Explicit allowlist. At most 32 tools; discovery never grants new tools.
    pub tools: Vec<McpToolConfig>,
}

/// One reviewed remote capability, exposed as `mcp_{server}_{alias}`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpToolConfig {
    /// Exact remote tool name.
    pub name: String,
    /// Local ASCII identifier, independent of the remote name.
    pub alias: String,
    /// RFC 8785 SHA-256 of the complete reviewed descriptor, `sha256:<64 lower hex>`.
    pub descriptor_sha256: String,
    /// Trusted operator classification, never inferred from remote annotations.
    pub effect: McpToolEffect,
}

/// Effects permitted before durable external-write admission is integrated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpToolEffect {
    /// Operator has reviewed this tool as having no external write effects.
    ReadOnly,
}

const fn default_timeout() -> u64 {
    30
}
const fn default_response_bytes() -> usize {
    256 * 1024
}
const fn default_concurrency() -> usize {
    4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_requires_explicit_effect_and_rejects_typos() {
        let binding = serde_json::json!({
            "name": "lookup", "alias": "lookup", "descriptor_sha256": "pin",
            "effect": "read_only"
        });
        assert!(serde_json::from_value::<McpToolConfig>(binding.clone()).is_ok());
        let mut missing = binding.clone();
        missing.as_object_mut().unwrap().remove("effect");
        assert!(serde_json::from_value::<McpToolConfig>(missing).is_err());
        let mut write = binding.clone();
        write["effect"] = serde_json::json!("write");
        assert!(serde_json::from_value::<McpToolConfig>(write).is_err());
        let mut typo = binding;
        typo["allow_write"] = serde_json::json!(true);
        assert!(serde_json::from_value::<McpToolConfig>(typo).is_err());
        assert!(McpConfig::default().servers.is_empty());
    }

    #[test]
    fn top_level_mcp_is_loaded_in_both_file_formats() {
        use crate::AgentConfig;
        let json = serde_json::json!({
            "agent": {"name": "test", "description": "test", "system_instructions": "test", "max_turns": 1},
            "mcp": {"servers": [{"name": "test", "endpoint": "http://127.0.0.1:3000/mcp/", "tools": [{
                "name": "lookup", "alias": "lookup", "effect": "read_only",
                "descriptor_sha256": format!("sha256:{}", "0".repeat(64))
            }]}]}
        });
        let from_json = AgentConfig::from_json_str(&json.to_string()).unwrap();
        let toml = toml::to_string(&json).unwrap();
        let from_toml = AgentConfig::from_toml_str(&toml).unwrap();
        assert_eq!(from_json.mcp.servers.len(), 1);
        assert_eq!(
            serde_json::to_value(from_json.mcp).unwrap(),
            serde_json::to_value(from_toml.mcp).unwrap()
        );
        let typo = json.to_string().replace("read_only", "read_only_typo");
        assert!(AgentConfig::from_json_str(&typo).is_err());
    }
}
