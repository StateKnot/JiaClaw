// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 模型提供商适配器
//!
//! ## 架构约束
//!
//! `JiaClaw` 必须通过 [Brokerrouter](https://github.com/StateKnot/Brokerrouter)
//! 作为 AI Gateway 路由所有模型调用。
//!
//! ## 实现状态
//!
//! - ✅ `StubProvider` - 离线存根模式（无需外部服务）
//! - ✅ `BrokerrouterProvider` - 生产模式（推荐）
//! - 🔄 `OpenAICompatibleProvider` - 临时直连模式（已废弃，仅作开发逃生舱）
//!
//! ## 当前推荐
//!
//! 1. **生产路径**: `BrokerrouterProvider` (`provider_type` = `"brokerrouter"`)
//! 2. **开发逃生舱**: `OpenAICompatibleProvider` (`provider_type` = `"openai_compatible"`)
//! 3. **离线模式**: 显式配置 `provider_type` = `"stub"`；缺少 API key 的真实提供商返回配置错误
//!
//! 参见 `docs/brokerrouter-gaps.md` 了解集成需求和议题跟踪。

pub(crate) mod brokerrouter;
mod openai_compatible;

#[allow(clippy::module_name_repetitions)]
pub use brokerrouter::BrokerrouterProvider;
#[allow(clippy::module_name_repetitions)]
pub use openai_compatible::OpenAICompatibleProvider;

/// Validate a model provider endpoint without making a network request.
///
/// Provider URLs are restricted to HTTPS or literal-loopback HTTP, and may not
/// contain user information, a query, or a fragment. The length limit matches
/// the gateway and MCP transport contracts.
///
/// # Errors
/// Returns a sanitized configuration error when the endpoint is not acceptable.
pub fn validate_endpoint(value: &str) -> Result<(), jiaclaw_core::JiaClawError> {
    use stateknot_integrations::ProviderEndpoint;

    let endpoint = if value.starts_with("https://") {
        ProviderEndpoint::https(value)
    } else {
        ProviderEndpoint::loopback_http(value)
    };
    if value.len() > 2048 || endpoint.is_err() {
        return Err(jiaclaw_core::JiaClawError::Configuration(
            "provider endpoint requires HTTPS or literal-loopback HTTP without credentials/query/fragment".into(),
        ));
    }
    Ok(())
}
