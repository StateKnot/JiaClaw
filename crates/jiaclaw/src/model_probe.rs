// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Operator-owned, one-shot model verification through the existing receipt owner.

use crate::model_calls::ModelCalls;
use crate::provider::brokerrouter::{BrokerrouterProvider, PreparedCompletion, WireMessage};
use jiaclaw_core::{AgentConfig, JiaClawError, ModelPurpose, ModelSelection};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MAX_OUTPUT_TOKENS: u32 = 128;

fn invalid(message: &str) -> JiaClawError {
    JiaClawError::Configuration(format!("model probe: {message}"))
}

/// Result of one persisted model response; `verified` also checks the random echo.
/// Neither success nor failure certifies tool execution, supplier billing or recovery.
#[derive(Debug, serde::Serialize)]
pub struct ModelProbeReceipt {
    /// Local durable operation identity, usable with model-calls status/result.
    pub operation_id: String,
    /// Validated Brokerrouter request UUID.
    pub remote_id: String,
    /// Actual Chat route and bounded diagnostic output settings.
    pub routing: ModelSelection,
    /// A completed, persisted response matched this invocation's exact challenge.
    pub verified: bool,
}

/// Owns the private ledger and one prepared no-tools diagnostic request.
/// No workspace content, skills, session store, MCP or embedding is initialized.
pub struct ModelProbe {
    ledger: Arc<ModelCalls>,
    prepared: Option<PreparedCompletion>,
    challenge: String,
    turn_id: String,
    selection: ModelSelection,
}

impl ModelProbe {
    /// Prepare a bounded request and open the configured exclusive private ledger.
    /// Opening sends no network requests. The caller must obtain billing consent.
    ///
    /// # Errors
    /// Requires Brokerrouter, explicit model_calls.enabled, valid route/key/endpoint,
    /// and safe private state. Does not silently enable storage or fall back to stub.
    pub async fn open(config: &AgentConfig) -> Result<Self, JiaClawError> {
        if config.provider.provider_type != "brokerrouter" || !config.model_calls.enabled {
            return Err(invalid(
                "requires brokerrouter and model_calls.enabled=true",
            ));
        }
        config.routing.validate(&config.provider)?;
        config.model_calls.validate(&config.provider)?;
        let key = crate::JiaClawAgent::resolve_provider_api_key(
            &config.provider,
            std::env::var("JIACLAW_API_KEY").ok().as_deref(),
        )?
        .ok_or_else(|| invalid("missing gateway credential"))?;
        let mut selection = config.routing.select(&config.provider, ModelPurpose::Chat);
        selection.temperature = 0.0;
        selection.max_tokens = selection.max_tokens.min(MAX_OUTPUT_TOKENS);
        let challenge = format!("JIACLAW_VERIFY_{}", Uuid::new_v4().simple());
        // Validate exact outgoing bytes before opening state; never load ambient input.
        let prepared = BrokerrouterProvider::new(&config.provider.base_url, &key).prepare(
            &selection.model,
            &[
                WireMessage::text("system", "Return exactly the verification code in the user message. No explanation or additional text.".into()),
                WireMessage::text("user", challenge.clone()),
            ],
            selection.temperature,
            selection.max_tokens,
            &[],
        )?;
        let ledger = ModelCalls::open(config)
            .await?
            .ok_or_else(|| invalid("private ledger is disabled"))?;
        Ok(Self {
            ledger,
            prepared: Some(prepared),
            challenge,
            turn_id: Uuid::new_v4().to_string(),
            selection,
        })
    }

    /// Submit at most once, using the original persisted request/receipt contract.
    /// A canceled waiter never releases the actual model worker or resumes tools.
    ///
    /// # Errors
    /// Consumed request, busy/unknown ledger, transport/protocol/persistence failure.
    /// Unknown outcomes retain the existing hold and operation identity; no retries.
    pub async fn verify(&mut self) -> Result<ModelProbeReceipt, JiaClawError> {
        let prepared = self
            .prepared
            .take()
            .ok_or_else(|| invalid("already attempted; inspect the original receipt"))?;
        let message = self
            .ledger
            .complete(prepared, self.turn_id.clone(), ModelPurpose::Chat, None, 0)
            .await?;
        let status = self.ledger.status().await?;
        let operation = status["recent"]
            .as_array()
            .and_then(|recent| recent.iter().find(|op| op["turn_id"] == self.turn_id))
            .filter(|op| op["state"] == "completed" && op["has_receipt"] == true)
            .ok_or_else(|| invalid("completed receipt unavailable; inspect model-calls status"))?;
        let identity = |key: &str| {
            operation[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid("receipt identity unavailable; inspect model-calls status"))
        };
        Ok(ModelProbeReceipt {
            operation_id: identity("id")?,
            remote_id: identity("remote_id")?,
            routing: self.selection.clone(),
            verified: message
                .content
                .as_deref()
                .is_some_and(|text| text.trim() == self.challenge),
        })
    }

    /// Wait for the actual receipt owner after caller cancellation; sends no requests.
    ///
    /// # Errors
    /// Grace expired or ledger failed. Unsettled state requires inspection on restart.
    pub async fn settle(&self, grace: Duration) -> Result<(), JiaClawError> {
        self.ledger.settle(grace).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unsupported_or_untracked_probe_never_creates_state() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = AgentConfig {
            workspace_path: temp.path().join("workspace"),
            ..AgentConfig::default()
        };
        for provider in ["stub", "openai_compatible", "brokerrouter"] {
            config.provider.provider_type = provider.into();
            config.model_calls.enabled = provider != "brokerrouter";
            assert!(ModelProbe::open(&config).await.is_err());
            assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn invalid_probe_request_is_rejected_before_private_state_open() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = AgentConfig {
            workspace_path: temp.path().join("workspace"),
            ..AgentConfig::default()
        };
        config.provider.provider_type = "brokerrouter".into();
        config.model_calls.enabled = true;
        config.provider.api_key = Some("fixture-key".into());
        config.provider.base_url = "http://127.0.0.1:1".into();
        for model in ["", "bad\nmodel"] {
            config.provider.model = model.into();
            assert!(ModelProbe::open(&config).await.is_err());
            assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn a_failed_attempt_cannot_be_reused_or_erase_its_original_hold() {
        let temp = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let mut config = AgentConfig {
            workspace_path: temp.path().join("workspace"),
            ..AgentConfig::default()
        };
        config.provider.provider_type = "brokerrouter".into();
        config.provider.api_key = Some("fixture-key".into());
        config.provider.base_url = endpoint;
        config.model_calls.enabled = true;
        config.model_calls.store_path = "../state/model-calls/index.sqlite3".into();
        std::fs::create_dir(&config.workspace_path).unwrap();
        let mut probe = ModelProbe::open(&config).await.unwrap();
        assert!(probe.verify().await.is_err());
        let original = probe.ledger.status().await.unwrap();
        assert_eq!(original["pending"]["state"], "unknown");
        assert!(probe
            .verify()
            .await
            .unwrap_err()
            .to_string()
            .contains("already attempted"));
        assert_eq!(probe.ledger.status().await.unwrap(), original);
    }
}
