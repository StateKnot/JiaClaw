// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::{JiaClawError, ProviderConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Operator-owned receipts for single Brokerrouter model calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCallsConfig {
    /// Requires explicit opt-in; disabled configuration creates no database.
    pub enabled: bool,
    /// Private SQLite file outside the model-accessible workspace.
    pub store_path: PathBuf,
}

impl Default for ModelCallsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            store_path: PathBuf::from("../state/model-calls/index.sqlite3"),
        }
    }
}

impl ModelCallsConfig {
    /// Validate configuration before storage or network effects.
    ///
    /// # Errors
    /// Invalid path or enabled non-Brokerrouter provider.
    pub fn validate(&self, provider: &ProviderConfig) -> Result<(), JiaClawError> {
        let path = self.store_path.to_str().unwrap_or_default();
        if path.is_empty()
            || path.len() > 4096
            || path.trim() != path
            || path.chars().any(char::is_control)
            || (self.enabled && provider.provider_type != "brokerrouter")
        {
            return Err(JiaClawError::Configuration(
                "model_calls requires a valid private store_path and Brokerrouter provider when enabled".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentConfig;

    #[test]
    fn config_opt_in_parses_consistently_and_rejects_unknown_fields() {
        let header = "[agent]\nname='test'\ndescription=''\nsystem_instructions=''\nmax_turns=5\n";
        let toml = AgentConfig::from_toml_str(&format!(
            "{header}[model_calls]\nenabled=true\nstore_path='../private/calls.sqlite3'\n"
        ))
        .unwrap();
        let json = AgentConfig::from_json_str(r#"{"agent":{"name":"test","description":"","system_instructions":"","max_turns":5},"model_calls":{"enabled":true,"store_path":"../private/calls.sqlite3"}}"#).unwrap();
        assert!(toml.model_calls.enabled);
        assert_eq!(toml.model_calls.store_path, json.model_calls.store_path);
        assert!(!AgentConfig::default().model_calls.enabled);
        assert!(
            AgentConfig::from_toml_str(&format!("{header}[model_calls]\nunknown=true\n")).is_err()
        );
        let mut provider = ProviderConfig::default();
        provider.provider_type = "stub".into();
        assert!(toml.model_calls.validate(&provider).is_err());
        provider.provider_type = "brokerrouter".into();
        assert!(toml.model_calls.validate(&provider).is_ok());
    }
}
