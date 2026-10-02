// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Operator-owned model routes. Request content never selects a purpose or credentials.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{JiaClawError, ProviderConfig};

/// Trusted execution context selected by the application, not request metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelPurpose {
    /// Interactive chat, including streaming chat.
    Chat,
    /// An authenticated inbound channel message.
    Channel,
    /// A durable scheduled job.
    Scheduled,
    /// The workspace heartbeat.
    Heartbeat,
    /// Tool-free conversation compaction.
    Summary,
}

/// One model override within the provider's global output budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    /// Exact gateway model name, 1..=200 UTF-8 bytes, without controls or boundary whitespace.
    pub model: String,
    /// Finite value in 0..=2; omitted values inherit the purpose default.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Positive output budget no greater than `provider.max_tokens`.
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Optional routes; an empty table preserves existing provider configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelRoutingConfig {
    /// Interactive chat override.
    #[serde(default)]
    pub chat: Option<ModelRoute>,
    /// Authenticated channel reply override.
    #[serde(default)]
    pub channel: Option<ModelRoute>,
    /// Scheduled job override.
    #[serde(default)]
    pub scheduled: Option<ModelRoute>,
    /// Workspace heartbeat override.
    #[serde(default)]
    pub heartbeat: Option<ModelRoute>,
    /// Conversation summary override; the effective token budget never exceeds 512.
    #[serde(default)]
    pub summary: Option<ModelRoute>,
}

/// The selected request settings, without API keys or endpoint credentials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSelection {
    /// Application-selected execution purpose.
    pub purpose: ModelPurpose,
    /// Exact model name sent to the configured gateway.
    pub model: String,
    /// Effective sampling temperature.
    pub temperature: f32,
    /// Effective maximum output tokens.
    pub max_tokens: u32,
}

impl ModelRoutingConfig {
    /// Whether at least one purpose has an explicit route.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.is_empty()
    }

    /// Whether every purpose inherits the provider configuration.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chat.is_none()
            && self.channel.is_none()
            && self.scheduled.is_none()
            && self.heartbeat.is_none()
            && self.summary.is_none()
    }

    /// Validate enabled routes and all provider defaults they may inherit.
    ///
    /// Empty routing preserves legacy provider validation. Model names retain their
    /// exact spelling; this application additionally rejects control characters and
    /// leading/trailing whitespace rather than silently normalizing identifiers.
    ///
    /// # Errors
    /// Returns a redacted configuration error for unsupported providers or invalid limits.
    pub fn validate(&self, provider: &ProviderConfig) -> Result<(), JiaClawError> {
        if self.is_empty() {
            return Ok(());
        }
        if provider.provider_type != "brokerrouter" {
            return Err(invalid("routing requires provider_type brokerrouter"));
        }
        validate_model(&provider.model, "provider.model")?;
        validate_temperature(provider.temperature, "provider.temperature")?;
        if !(1..=1_000_000).contains(&provider.max_tokens) {
            return Err(invalid("provider.max_tokens must be between 1 and 1000000"));
        }
        for (name, route) in [
            ("chat", self.chat.as_ref()),
            ("channel", self.channel.as_ref()),
            ("scheduled", self.scheduled.as_ref()),
            ("heartbeat", self.heartbeat.as_ref()),
            ("summary", self.summary.as_ref()),
        ] {
            let Some(route) = route else { continue };
            validate_model(&route.model, &format!("routing.{name}.model"))?;
            if let Some(temperature) = route.temperature {
                validate_temperature(temperature, &format!("routing.{name}.temperature"))?;
            }
            if let Some(max_tokens) = route.max_tokens {
                if max_tokens == 0 || max_tokens > provider.max_tokens {
                    return Err(invalid(format!(
                        "routing.{name}.max_tokens must be between 1 and provider.max_tokens"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Select settings after [`Self::validate`] succeeds at initialization.
    ///
    /// Unspecified routes inherit the provider. Summaries retain the existing 0.2
    /// temperature default and a 512-token ceiling, including when routing is empty.
    #[must_use]
    pub fn select(&self, provider: &ProviderConfig, purpose: ModelPurpose) -> ModelSelection {
        let route = match purpose {
            ModelPurpose::Chat => self.chat.as_ref(),
            ModelPurpose::Channel => self.channel.as_ref(),
            ModelPurpose::Scheduled => self.scheduled.as_ref(),
            ModelPurpose::Heartbeat => self.heartbeat.as_ref(),
            ModelPurpose::Summary => self.summary.as_ref(),
        };
        let summary = purpose == ModelPurpose::Summary;
        let default_temperature = if summary { 0.2 } else { provider.temperature };
        let max_tokens = route
            .and_then(|route| route.max_tokens)
            .unwrap_or(provider.max_tokens)
            .min(provider.max_tokens);
        ModelSelection {
            purpose,
            model: route.map_or_else(|| provider.model.clone(), |route| route.model.clone()),
            temperature: route
                .and_then(|route| route.temperature)
                .unwrap_or(default_temperature),
            max_tokens: if summary {
                max_tokens.min(512)
            } else {
                max_tokens
            },
        }
    }
}

fn invalid(message: impl Into<String>) -> JiaClawError {
    JiaClawError::Configuration(message.into())
}

fn validate_model(model: &str, field: &str) -> Result<(), JiaClawError> {
    if model.is_empty()
        || model.len() > 200
        || model.trim() != model
        || model.chars().any(char::is_control)
    {
        return Err(invalid(format!(
            "{field} must contain 1..=200 UTF-8 bytes without controls or boundary whitespace"
        )));
    }
    Ok(())
}

fn validate_temperature(temperature: f32, field: &str) -> Result<(), JiaClawError> {
    if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
        return Err(invalid(format!(
            "{field} must be finite and between 0 and 2"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentConfig, ChatResponse};
    use serde_json::json;

    fn route(model: &str) -> ModelRoute {
        ModelRoute {
            model: model.to_owned(),
            temperature: None,
            max_tokens: None,
        }
    }

    fn enabled() -> ModelRoutingConfig {
        ModelRoutingConfig {
            chat: Some(route("gateway/模型:latest@v1")),
            ..ModelRoutingConfig::default()
        }
    }

    #[test]
    fn empty_routes_preserve_provider_and_summary_defaults() {
        let routing = ModelRoutingConfig::default();
        let provider = ProviderConfig::default();
        assert!(routing.is_empty());
        assert!(!routing.is_enabled());
        routing.validate(&provider).unwrap();
        for purpose in [
            ModelPurpose::Chat,
            ModelPurpose::Channel,
            ModelPurpose::Scheduled,
            ModelPurpose::Heartbeat,
        ] {
            assert_eq!(
                routing.select(&provider, purpose),
                ModelSelection {
                    purpose,
                    model: provider.model.clone(),
                    temperature: provider.temperature,
                    max_tokens: provider.max_tokens,
                }
            );
        }
        let summary = routing.select(&provider, ModelPurpose::Summary);
        assert_eq!(summary.model, provider.model);
        assert_eq!(summary.temperature, 0.2);
        assert_eq!(summary.max_tokens, 512);
        // No new validation applies to legacy configurations until a route is enabled.
        let legacy = ProviderConfig {
            provider_type: "stub".into(),
            model: "".into(),
            temperature: f32::NAN,
            max_tokens: 0,
            ..provider
        };
        routing.validate(&legacy).unwrap();
        let null_routes: ModelRoutingConfig = serde_json::from_value(json!({
            "chat": null, "channel": null, "scheduled": null, "heartbeat": null, "summary": null
        }))
        .unwrap();
        assert!(!null_routes.is_enabled());
    }

    #[test]
    fn overrides_are_isolated_and_preserve_exact_model_names() {
        let mut routing = enabled();
        routing.channel = Some(ModelRoute {
            model: "channel-v1".into(),
            temperature: Some(0.0),
            max_tokens: Some(30),
        });
        routing.scheduled = Some(route("schedule-v2"));
        routing.heartbeat = Some(route("heartbeat-v3"));
        let provider = ProviderConfig::default();
        routing.validate(&provider).unwrap();
        assert!(routing.is_enabled());
        assert_eq!(
            routing.select(&provider, ModelPurpose::Chat).model,
            "gateway/模型:latest@v1"
        );
        assert_eq!(
            routing.select(&provider, ModelPurpose::Channel),
            ModelSelection {
                purpose: ModelPurpose::Channel,
                model: "channel-v1".into(),
                temperature: 0.0,
                max_tokens: 30,
            }
        );
        assert_eq!(
            routing.select(&provider, ModelPurpose::Scheduled).model,
            "schedule-v2"
        );
        assert_eq!(
            routing.select(&provider, ModelPurpose::Heartbeat).model,
            "heartbeat-v3"
        );
        assert_eq!(
            routing.select(&provider, ModelPurpose::Summary).model,
            provider.model
        );
    }

    #[test]
    fn summary_budget_obeys_all_three_ceilings() {
        let mut routing = ModelRoutingConfig {
            summary: Some(route("summary-v1")),
            ..Default::default()
        };
        let mut provider = ProviderConfig::default();
        assert_eq!(
            routing.select(&provider, ModelPurpose::Summary).max_tokens,
            512
        );
        provider.max_tokens = 256;
        assert_eq!(
            routing.select(&provider, ModelPurpose::Summary).max_tokens,
            256
        );
        routing.summary.as_mut().unwrap().max_tokens = Some(128);
        routing.validate(&provider).unwrap();
        let selection = routing.select(&provider, ModelPurpose::Summary);
        assert_eq!(selection.max_tokens, 128);
        assert_eq!(selection.temperature, 0.2);
        routing.summary.as_mut().unwrap().temperature = Some(1.1);
        assert_eq!(
            routing.select(&provider, ModelPurpose::Summary).temperature,
            1.1
        );
        routing.summary.as_mut().unwrap().max_tokens = Some(257);
        assert!(routing.validate(&provider).is_err());
    }

    #[test]
    fn enabled_routes_require_valid_brokerrouter_defaults() {
        let routing = enabled();
        for provider_type in ["stub", "openai_compatible", "BrokerRouter", "brokerrouter "] {
            let provider = ProviderConfig {
                provider_type: provider_type.into(),
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
        }
        for model in [
            "",
            " secret",
            "secret ",
            "secret\n",
            "model\u{0}",
            "\u{2003}model",
        ] {
            let provider = ProviderConfig {
                model: model.into(),
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
        }
        for temperature in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 2.1] {
            let provider = ProviderConfig {
                temperature,
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
        }
        for max_tokens in [0, 1_000_001, u32::MAX] {
            let provider = ProviderConfig {
                max_tokens,
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
        }
        for (temperature, max_tokens) in [(0.0, 1), (2.0, 1_000_000)] {
            let provider = ProviderConfig {
                temperature,
                max_tokens,
                ..Default::default()
            };
            routing.validate(&provider).unwrap();
        }
    }

    #[test]
    fn route_model_validation_is_bounded_and_errors_do_not_echo_values() {
        let provider = ProviderConfig::default();
        for model in [
            "",
            " private-model",
            "private-model ",
            "private-model\n",
            "private\tmodel",
        ] {
            let routing = ModelRoutingConfig {
                channel: Some(route(model)),
                ..Default::default()
            };
            let error = routing.validate(&provider).unwrap_err().to_string();
            assert!(error.contains("routing.channel.model"));
            assert!(!error.contains("private"));
        }
        for model in [
            "x".repeat(200),
            "界".repeat(66),
            "gateway model/v1:@special".into(),
        ] {
            let routing = ModelRoutingConfig {
                summary: Some(route(&model)),
                ..Default::default()
            };
            routing.validate(&provider).unwrap();
            assert_eq!(
                routing.select(&provider, ModelPurpose::Summary).model,
                model
            );
        }
        for model in ["x".repeat(201), "界".repeat(67)] {
            let routing = ModelRoutingConfig {
                scheduled: Some(route(&model)),
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
            let provider = ProviderConfig {
                model,
                ..ProviderConfig::default()
            };
            assert!(enabled().validate(&provider).is_err());
        }
    }

    #[test]
    fn route_limits_reject_nonfinite_temperature_and_global_budget_bypass() {
        let provider = ProviderConfig::default();
        for temperature in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 2.1] {
            let routing = ModelRoutingConfig {
                heartbeat: Some(ModelRoute {
                    temperature: Some(temperature),
                    ..route("heartbeat")
                }),
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
        }
        for max_tokens in [0, provider.max_tokens + 1, u32::MAX] {
            let routing = ModelRoutingConfig {
                summary: Some(ModelRoute {
                    max_tokens: Some(max_tokens),
                    ..route("summary")
                }),
                ..Default::default()
            };
            assert!(routing.validate(&provider).is_err());
        }
    }

    fn base_config() -> serde_json::Value {
        json!({ "agent": { "name": "test", "description": "test", "system_instructions": "test", "max_turns": 1 } })
    }

    fn parse_both(config: &serde_json::Value) -> [AgentConfig; 2] {
        [
            AgentConfig::from_json_str(&config.to_string()).unwrap(),
            AgentConfig::from_toml_str(
                &toml::to_string(&toml::Value::try_from(config).unwrap()).unwrap(),
            )
            .unwrap(),
        ]
    }

    #[test]
    fn toml_and_json_load_nested_routes_and_replace_with_top_level_table() {
        let mut config = base_config();
        config["agent"]["routing"] =
            json!({ "chat": { "model": "nested" }, "summary": { "model": "nested-summary" } });
        for parsed in parse_both(&config) {
            assert_eq!(parsed.routing.chat.unwrap().model, "nested");
            assert_eq!(parsed.routing.summary.unwrap().model, "nested-summary");
        }
        config["routing"] =
            json!({ "channel": { "model": "top-level", "max_tokens": 250, "temperature": 0.3 } });
        let parsed = parse_both(&config);
        assert_eq!(parsed[0].routing, parsed[1].routing);
        for parsed in parsed {
            assert!(parsed.routing.chat.is_none());
            assert!(parsed.routing.summary.is_none());
            assert_eq!(parsed.routing.channel.unwrap().model, "top-level");
        }
        config["routing"] = json!({});
        for parsed in parse_both(&config) {
            assert!(!parsed.routing.is_enabled());
        }
    }

    #[test]
    fn invalid_fields_and_types_are_rejected_in_both_config_formats() {
        for invalid in [
            json!({ "channels": { "model": "m" } }),
            json!({ "chat": { "model": "m", "base_url": "https://untrusted.invalid" } }),
            json!({ "chat": { "temperature": 0.2 } }),
            json!({ "chat": { "model": "m", "temperature": "hot" } }),
            json!({ "chat": { "model": "m", "max_tokens": -1 } }),
            json!({ "chat": { "model": "m", "max_tokens": 4294967296_u64 } }),
        ] {
            for nested in [false, true] {
                let mut config = base_config();
                if nested {
                    config["agent"]["routing"] = invalid.clone();
                } else {
                    config["routing"] = invalid.clone();
                }
                assert!(AgentConfig::from_json_str(&config.to_string()).is_err());
                assert!(AgentConfig::from_toml_str(
                    &toml::to_string(&toml::Value::try_from(&config).unwrap()).unwrap()
                )
                .is_err());
            }
        }
        assert!(serde_json::from_str::<ModelRoute>(r#"{"model":"a","model":"b"}"#).is_err());
        assert!(serde_json::from_str::<ModelRoutingConfig>(
            r#"{"chat":null,"chat":{"model":"a"}}"#
        )
        .is_err());
        let nan: ModelRoutingConfig =
            toml::from_str("[chat]\nmodel='m'\ntemperature=nan\n").unwrap();
        assert!(nan.validate(&ProviderConfig::default()).is_err());
    }

    #[test]
    fn response_routing_round_trips_without_changing_legacy_json() {
        let legacy = json!({ "message": { "role": "assistant", "content": "hello" }, "status": "completed" });
        let mut response: ChatResponse = serde_json::from_value(legacy).unwrap();
        assert!(response.routing.is_none());
        assert!(serde_json::to_value(&response)
            .unwrap()
            .get("routing")
            .is_none());
        response.routing = Some(enabled().select(&ProviderConfig::default(), ModelPurpose::Chat));
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["routing"]["purpose"], "chat");
        assert_eq!(value["routing"].as_object().unwrap().len(), 4);
        let round_trip: ChatResponse = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip.routing, response.routing);
        for (purpose, serialized) in [
            (ModelPurpose::Chat, "chat"),
            (ModelPurpose::Channel, "channel"),
            (ModelPurpose::Scheduled, "scheduled"),
            (ModelPurpose::Heartbeat, "heartbeat"),
            (ModelPurpose::Summary, "summary"),
        ] {
            assert_eq!(serde_json::to_value(purpose).unwrap(), serialized);
            assert_eq!(
                serde_json::from_value::<ModelPurpose>(json!(serialized)).unwrap(),
                purpose
            );
        }
        let schema = serde_json::to_value(schemars::schema_for!(ModelSelection)).unwrap();
        assert!(schema["properties"].get("model").is_some());
        assert!(schema["properties"].get("api_key").is_none());
    }
}
