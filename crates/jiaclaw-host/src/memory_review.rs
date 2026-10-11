// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Private, explicitly enabled standalone memory review; no writing endpoint.
use super::{check_api_auth, AppState};
use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use jiaclaw::MemoryEditor;
use jiaclaw_core::AgentConfig;
use serde_json::json;

pub(super) fn validate_config(config: &AgentConfig, token: Option<&str>) -> anyhow::Result<()> {
    if config.http.memory_review
        && (token.is_none_or(|t| t.trim().is_empty())
            || config.http.gateway_channel_chat
            || config.scheduler.gateway_driven)
    {
        anyhow::bail!(
            "memory_review requires a nonempty administrator API Token and standalone scope"
        );
    }
    Ok(())
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let config = state.agent.config();
    if !config.http.memory_review
        || config.http.gateway_channel_chat
        || config.scheduler.gateway_driven
        || state.api_token.as_ref().is_none_or(|t| t.trim().is_empty())
    {
        return Err(reply(
            StatusCode::FORBIDDEN,
            json!({"error":"Memory review is not enabled for this scope"}),
        ));
    }
    if !check_api_auth(state, headers) {
        return Err(reply(
            StatusCode::UNAUTHORIZED,
            json!({"error":"Unauthorized"}),
        ));
    }
    Ok(())
}

pub(super) async fn capabilities(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    reply(
        StatusCode::OK,
        json!({"protocol":1,"scope":"standalone","read_only":true,"max_bytes":32768}),
    )
}

pub(super) async fn inspect(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    // Consume the existing bounded control pool. The real blocking read owns
    // its permit even if the HTTP waiter is cancelled; no queue or fallback.
    let Ok(permit) = state.http_turn_controls.clone().try_acquire_owned() else {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Memory review capacity is busy"}),
        );
    };
    let config = state.agent.config();
    let editor = MemoryEditor::new(&config.workspace_path, config.memory.path.clone());
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        editor.inspect()
    })
    .await;
    match result {
        Ok(Ok(snapshot)) => private(Json(snapshot).into_response()),
        Ok(Err(_)) => reply(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error":"Configured memory cannot be read completely; inspect the local file before retrying"}),
        ),
        Err(_) => reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Memory review worker failed"}),
        ),
    }
}

fn reply(status: StatusCode, value: serde_json::Value) -> Response {
    private((status, Json(value)).into_response())
}
fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

#[cfg(test)]
mod tests {
    use super::validate_config;
    use jiaclaw_core::AgentConfig;

    #[test]
    fn memory_review_requires_opt_in_operator_and_standalone_scope() {
        let mut config = AgentConfig::default();
        assert!(!config.http.memory_review);
        assert!(validate_config(&config, None).is_ok());
        config.http.memory_review = true;
        for token in [None, Some(""), Some(" \n ")] {
            assert!(validate_config(&config, token).is_err());
        }
        assert!(validate_config(&config, Some("operator")).is_ok());
        config.http.gateway_channel_chat = true;
        assert!(validate_config(&config, Some("backend-token")).is_err());
        config.http.gateway_channel_chat = false;
        config.scheduler.gateway_driven = true;
        assert!(validate_config(&config, Some("backend-token")).is_err());
    }

    #[tokio::test]
    async fn memory_review_denies_ambient_authority_and_shares_bounded_controls() {
        use axum::{
            extract::State,
            http::{HeaderMap, HeaderValue, StatusCode},
        };
        let workspace = std::env::temp_dir().join(format!(
            "jiaclaw-memory-review-owner-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        let mut config = state.agent.config().clone();
        config.http.memory_review = true;
        state.agent = std::sync::Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
        assert_eq!(
            super::inspect(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        state.api_token = Some("operator".to_owned());
        assert_eq!(
            super::inspect(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer operator"));
        let held = state
            .http_turn_controls
            .clone()
            .try_acquire_many_owned(4)
            .unwrap();
        assert_eq!(
            super::inspect(State(state.clone()), headers.clone())
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(held);
        std::fs::write(workspace.join("MEMORY.md"), "private current memory").unwrap();
        assert_eq!(
            super::inspect(State(state.clone()), headers).await.status(),
            StatusCode::OK
        );
        assert_eq!(state.http_turn_controls.available_permits(), 4);
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
