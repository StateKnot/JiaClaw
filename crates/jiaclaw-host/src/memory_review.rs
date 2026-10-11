// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Private standalone memory review and separately opted-in operator correction.
use super::{check_api_auth, AppState};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use jiaclaw::MemoryEditor;
use jiaclaw_core::AgentConfig;
use serde::Deserialize;
use serde_json::json;

pub(super) fn validate_config(config: &AgentConfig, token: Option<&str>) -> anyhow::Result<()> {
    if config.http.memory_edit && !config.http.memory_review {
        anyhow::bail!("memory_edit requires memory_review and its administrator standalone gates");
    }
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

fn authorize_edit(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    authorize(state, headers)?;
    if !state.agent.config().http.memory_edit {
        return Err(reply(
            StatusCode::FORBIDDEN,
            json!({"error":"Memory editing is not enabled"}),
        ));
    }
    Ok(())
}

pub(super) async fn edit_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize_edit(&state, &headers) {
        return response;
    }
    reply(
        StatusCode::OK,
        json!({"protocol":1,"scope":"standalone","operation":"replace","max_bytes":32768,"conditions":"path-and-two-raw-sha256"}),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    path: String,
    content: String,
    if_content_sha256: String,
    if_replacement_sha256: String,
}

pub(super) async fn replace(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize_edit(&state, request.headers()) {
        return response;
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v.split(';').next().map(str::trim) != Some("application/json"))
    {
        return reply(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            json!({"error":"JSON is required"}),
        );
    }
    let Ok(permit) = state.http_turn_controls.clone().try_acquire_owned() else {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Memory control capacity is busy"}),
        );
    };
    // Bounded body capture occurs after authorization. The shared permit then
    // moves into the actual writer and is never owned just by its HTTP waiter.
    // Request consumes Body directly, so DefaultBodyLimit's extractor setting
    // does not enforce the configured global budget on chunked requests.
    // Consume the exact startup-resolved middleware budget, including the
    // existing environment override and zero-value fallback.
    let Some(budget) = request.extensions().get::<super::RequestBodyBudget>() else {
        return reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Memory body budget is unavailable; no writer started"}),
        );
    };
    let body_limit = budget.0.min(256 * 1024);
    let bytes = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        to_bytes(request.into_body(), body_limit),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return reply(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error":"Memory request exceeds its body budget"}),
            )
        }
        Err(_) => {
            return reply(
                StatusCode::REQUEST_TIMEOUT,
                json!({"error":"Memory body capture timed out; no writer started"}),
            )
        }
    };
    let input: Replacement = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(_) => {
            return reply(
                StatusCode::BAD_REQUEST,
                json!({"error":"Invalid memory replacement contract"}),
            )
        }
    };
    let config = state.agent.config();
    if input.path != config.memory.path || input.content.len() > 32768 {
        return reply(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error":"Replacement path or content budget does not match"}),
        );
    }
    let editor = MemoryEditor::new(&config.workspace_path, config.memory.path.clone());
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        editor.replace_text(
            input.content,
            &input.if_content_sha256,
            &input.if_replacement_sha256,
        )
    })
    .await;
    match result {
        Ok(Ok(receipt)) => private(Json(receipt).into_response()),
        _ => {
            // Even a sync failure after rename may have committed. Never report
            // zero effects or authorize replay from a generic error response.
            let mut response = reply(
                StatusCode::CONFLICT,
                json!({"error":"Replacement could not be confirmed; inspect current memory before any new request"}),
            );
            response.headers_mut().insert(
                "x-jiaclaw-write-review",
                HeaderValue::from_static("required"),
            );
            response
        }
    }
}

pub(super) fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
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

pub(super) fn reply(status: StatusCode, value: serde_json::Value) -> Response {
    private((status, Json(value)).into_response())
}
pub(super) fn private(mut response: Response) -> Response {
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
    fn memory_edit_requires_separate_opt_in_and_all_review_gates() {
        let mut config = AgentConfig::default();
        assert!(!config.http.memory_edit);
        config.http.memory_edit = true;
        assert!(validate_config(&config, Some("operator")).is_err());
        config.http.memory_review = true;
        assert!(validate_config(&config, None).is_err());
        assert!(validate_config(&config, Some("operator")).is_ok());
        config.scheduler.gateway_driven = true;
        assert!(validate_config(&config, Some("operator")).is_err());
    }

    #[tokio::test]
    async fn memory_edit_shares_controls_and_does_not_use_read_authority() {
        use axum::{
            body::Body,
            extract::{Request, State},
            http::{HeaderMap, StatusCode},
        };
        let workspace =
            std::env::temp_dir().join(format!("jiaclaw-memory-edit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        let mut config = state.agent.config().clone();
        config.http.memory_review = true;
        state.agent = std::sync::Arc::new(jiaclaw::JiaClawAgent::new(config.clone()).unwrap());
        state.api_token = Some("operator".to_owned());
        let request = || {
            let mut request = Request::builder()
                .header("authorization", "Bearer operator")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            request
                .extensions_mut()
                .insert(crate::RequestBodyBudget(1024));
            request
        };
        assert_eq!(
            super::replace(State(state.clone()), request())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        config.http.memory_edit = true;
        state.agent = std::sync::Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
        assert_eq!(
            super::edit_capabilities(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let held = state
            .http_turn_controls
            .clone()
            .try_acquire_many_owned(4)
            .unwrap();
        assert_eq!(
            super::replace(State(state.clone()), request())
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(held);
        assert_eq!(
            super::replace(State(state.clone()), request())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(state.http_turn_controls.available_permits(), 4);
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }

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
