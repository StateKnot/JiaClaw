// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Opt-in administrator inspection of the already-owned local semantic index.
use super::{memory_review, AppState};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use jiaclaw_core::AgentConfig;
use serde_json::json;

pub(super) fn validate_config(config: &AgentConfig, token: Option<&str>) -> anyhow::Result<()> {
    if config.http.semantic_review {
        if !config.http.memory_review || !config.memory.semantic.enabled {
            anyhow::bail!("semantic_review requires memory_review and enabled semantic memory");
        }
        memory_review::validate_config(config, token)?;
    }
    Ok(())
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    memory_review::authorize(state, headers)?;
    if !state.agent.config().http.semantic_review || !state.agent.config().memory.semantic.enabled {
        return Err(memory_review::reply(
            StatusCode::FORBIDDEN,
            json!({"error":"Semantic review is not enabled"}),
        ));
    }
    Ok(())
}

pub(super) async fn capabilities(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    memory_review::reply(
        StatusCode::OK,
        json!({"protocol":1,"scope":"standalone","operation":"semantic-check","read_only":true,"local_only":true,"max_sources":3,"max_source_bytes":131072,"max_total_source_bytes":393216,"max_chunks":512}),
    )
}

pub(super) async fn method_not_allowed(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    let mut response = memory_review::reply(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({"error":"Semantic review only accepts GET"}),
    );
    response
        .headers_mut()
        .insert("allow", "GET, HEAD".parse().unwrap());
    response
}

pub(super) async fn check(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    let Ok(permit) = state.http_turn_controls.clone().try_acquire_owned() else {
        return memory_review::reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Memory control capacity is busy"}),
        );
    };
    let agent = state.agent.clone();
    // The actual worker owns control capacity, the original Agent/store and
    // the existing semantic admission until settlement, even if its waiter drops.
    let result = tokio::spawn(async move {
        let _permit = permit;
        agent.check_semantic_memory().await
    })
    .await;
    match result {
        Ok(Ok(snapshot)) => memory_review::private(Json(snapshot).into_response()),
        Ok(Err(_)) => memory_review::reply(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error":"Semantic index cannot be checked completely; inspect local sources or the original operation before trying again"}),
        ),
        Err(_) => memory_review::reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Semantic review worker failed"}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_review_requires_separate_opt_in_and_original_review_gates() {
        let mut config = AgentConfig::default();
        assert!(!config.http.semantic_review);
        assert!(validate_config(&config, None).is_ok());
        config.http.semantic_review = true;
        assert!(validate_config(&config, Some("operator")).is_err());
        config.http.memory_review = true;
        assert!(validate_config(&config, Some("operator")).is_err());
        config.memory.semantic.enabled = true;
        for token in [None, Some(""), Some(" \n ")] {
            assert!(validate_config(&config, token).is_err());
        }
        assert!(validate_config(&config, Some("operator")).is_ok());
        config.http.gateway_channel_chat = true;
        assert!(validate_config(&config, Some("operator")).is_err());
        config.http.gateway_channel_chat = false;
        config.scheduler.gateway_driven = true;
        assert!(validate_config(&config, Some("operator")).is_err());
    }

    #[tokio::test]
    async fn semantic_review_uses_live_store_and_original_control_capacity() {
        let root =
            std::env::temp_dir().join(format!("jiaclaw-semantic-review-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("MEMORY.md"), "private source").unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        let mut config = state.agent.config().clone();
        config.http.memory_review = true;
        config.http.semantic_review = true;
        config.provider.provider_type = "brokerrouter".into();
        config.provider.base_url = "http://127.0.0.1:9".into();
        config.provider.api_key = Some("disposable-fixture".into());
        config.memory.semantic.enabled = true;
        config.memory.semantic.model = "fixture-embedding".into();
        config.memory.semantic.space_revision = "fixture-v1".into();
        config.memory.semantic.dimensions = 2;
        config.memory.semantic.index_path = "../state/semantic/index.sqlite3".into();
        state.agent = std::sync::Arc::new(
            jiaclaw::JiaClawAgent::connect(config.clone())
                .await
                .unwrap(),
        );
        state.api_token = Some("operator".into());
        assert_eq!(
            capabilities(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer operator".parse().unwrap());
        assert_eq!(
            capabilities(State(state.clone()), headers.clone())
                .await
                .status(),
            StatusCode::OK
        );
        let refusal = method_not_allowed(State(state.clone()), headers.clone()).await;
        assert_eq!(refusal.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(refusal.headers()["allow"], "GET, HEAD");
        assert_eq!(refusal.headers()["cache-control"], "no-store");
        assert_eq!(refusal.headers()["x-content-type-options"], "nosniff");
        assert_eq!(refusal.headers()["referrer-policy"], "no-referrer");
        assert_eq!(
            method_not_allowed(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(jiaclaw::SemanticMemory::check_config(&config)
            .await
            .unwrap_err()
            .to_string()
            .contains("another process owns semantic state"));
        let held = state
            .http_turn_controls
            .clone()
            .try_acquire_many_owned(4)
            .unwrap();
        assert_eq!(
            check(State(state.clone()), headers.clone()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(held);
        assert_eq!(
            check(State(state.clone()), headers).await.status(),
            StatusCode::OK
        );
        assert_eq!(state.http_turn_controls.available_permits(), 4);
        assert!(jiaclaw::SemanticMemory::check_config(&config)
            .await
            .unwrap_err()
            .to_string()
            .contains("another process owns semantic state"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
