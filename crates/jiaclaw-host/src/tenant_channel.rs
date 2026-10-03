// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Private, single-attempt channel chat. The gateway owns durable admission and
//! never retries this endpoint: a lost response may follow a committed session.
use super::{
    prepare_session_chat_messages, session_turn_lock, with_sessions, AppError, AppState,
    SessionRecord,
};
use anyhow::{ensure, Result};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, HeaderMap, Uri},
    Json,
};
use jiaclaw::JiaClawAgent;
use jiaclaw_core::{
    AgentConfig, ChatMessage, ChatRequest, ChatResponse, MessageRole, ModelPurpose,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;
use uuid::Uuid;

const TOOLS: [&str; 2] = ["datetime_now", "json_query"];
const MAX_RUN_SECONDS: u64 = 120;
const MAX_PROMPT_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 128 * 1024;
const COMMIT_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn validate_config(config: &AgentConfig, token: Option<&str>) -> Result<()> {
    if !config.http.gateway_channel_chat {
        return Ok(());
    }
    ensure!(
        config.http.persist && token.is_some_and(|value| !value.trim().is_empty()),
        "gateway channel chat requires SQLite persistence and an API Token"
    );
    ensure!(
        matches!(
            config.provider.provider_type.as_str(),
            "brokerrouter" | "stub"
        ),
        "gateway channel chat requires brokerrouter or explicit stub provider"
    );
    ensure!(
        config.http.channels.is_empty() && !config.heartbeat.enabled,
        "gateway channel chat requires standalone channels and HEARTBEAT disabled"
    );
    ensure!(
        config
            .http
            .webhook_secret
            .as_deref()
            .is_none_or(|secret| secret.trim().is_empty()),
        "gateway channel chat requires the legacy webhook disabled"
    );
    ensure!(
        !config.scheduler.enabled || config.scheduler.gateway_driven,
        "gateway channel chat only permits gateway-driven scheduling"
    );
    Ok(())
}

pub(super) fn validate_tools(agent: &JiaClawAgent) -> Result<()> {
    ensure!(
        !agent.config().http.gateway_channel_chat
            || TOOLS.iter().all(|name| agent.tools().get(name).is_some()),
        "gateway channel chat requires datetime_now and json_query registered"
    );
    Ok(())
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    if !state.agent.config().http.gateway_channel_chat {
        return Err(AppError::NotFound);
    }
    let mut authorization = headers.get_all(header::AUTHORIZATION).iter();
    let valid = authorization
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| {
            state
                .api_token
                .as_deref()
                .is_some_and(|expected| !expected.is_empty() && token == expected)
        });
    if !valid || authorization.next().is_some() || headers.contains_key("x-api-token") {
        return Err(AppError::Unauthorized);
    }
    if !state.persist_enabled || state.gateway_channel_permit.is_closed() {
        return Err(AppError::ChannelUnavailable);
    }
    Ok(())
}

pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &headers)?;
    if uri.query().is_some() {
        return Err(AppError::BadRequest(
            "query parameters are not supported".into(),
        ));
    }
    let _permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    validate_tools(&state.agent).map_err(|_| AppError::ChannelUnavailable)?;
    Ok(Json(
        json!({"protocol":1,"backend_id":state.agent.config().name,
        "max_run_seconds":MAX_RUN_SECONDS,"tools":TOOLS,"mode":"gateway"}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelRequest {
    request_id: String,
    binding_id: String,
    prompt: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ChannelResponse {
    protocol: u8,
    backend_id: String,
    request_id: String,
    binding_id: String,
    response: ChatResponse,
}

fn canonical_id(raw: &str) -> Option<Uuid> {
    Uuid::parse_str(raw)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == raw)
}

impl ChannelRequest {
    fn session_id(&self) -> Result<String, AppError> {
        if !canonical_id(&self.request_id).is_some_and(|id| id.get_version_num() == 7) {
            return Err(AppError::BadRequest(
                "canonical UUIDv7 request_id required".into(),
            ));
        }
        let binding = canonical_id(&self.binding_id)
            .ok_or_else(|| AppError::BadRequest("canonical binding UUID required".into()))?;
        if self.prompt.trim().is_empty() || self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::BadRequest(
                "prompt must contain 1..16384 UTF-8 bytes of text".into(),
            ));
        }
        Ok(format!("tg-{}", binding.simple()))
    }
}

pub(super) async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Json<ChannelResponse>, AppError> {
    authorize(&state, &headers)?;
    if request.uri().query().is_some() {
        return Err(AppError::BadRequest(
            "query parameters are not supported".into(),
        ));
    }
    let control = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next());
    if !mime.is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json")) {
        return Err(AppError::BadRequest("JSON content type required".into()));
    }
    let limit = super::max_body_bytes_usize(state.agent.config().http.effective_max_body_bytes())
        .min(MAX_BODY_BYTES);
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), limit),
    )
    .await
    .map_err(|_| AppError::ChannelUnavailable)?
    .map_err(|_| {
        AppError::BadRequest("channel request body exceeds limit or is unreadable".into())
    })?;
    // Serde structs also accept positional arrays. Require an object first,
    // then parse the original bytes so duplicate known fields remain errors.
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid channel request JSON".into()))?;
    if !value.is_object() {
        return Err(AppError::BadRequest(
            "channel request must be a JSON object".into(),
        ));
    }
    drop(value);
    let request: ChannelRequest = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid channel request JSON or fields".into()))?;
    let session_id = request.session_id()?;
    validate_tools(&state.agent).map_err(|_| AppError::ChannelUnavailable)?;
    let permit = state
        .gateway_channel_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::JobConflict("gateway channel chat already executing or stopping".into())
        })?;
    drop(control);
    // Dropping the HTTP handler only drops this join handle. The admitted turn
    // keeps its slot through model execution and the final SQLite operation.
    tokio::spawn(execute(state, request, session_id, permit))
        .await
        .map_err(|_| AppError::ChannelUnavailable)?
}

async fn execute(
    state: AppState,
    request: ChannelRequest,
    session_id: String,
    permit: OwnedSemaphorePermit,
) -> Result<Json<ChannelResponse>, AppError> {
    let prepared = tokio::time::timeout(Duration::from_secs(MAX_RUN_SECONDS), async {
        let guard = session_turn_lock(&state, &session_id).await;
        let messages = prepare_session_chat_messages(
            &state,
            &session_id,
            vec![ChatMessage {
                role: MessageRole::User,
                content: request.prompt.clone(),
            }],
            &request.request_id,
            "gateway_channel",
        )
        .await
        .map_err(|_| {
            AppError::Internal("gateway_channel_preparation_failed; outcome requires review".into())
        })?;
        let chat = ChatRequest {
            messages,
            enabled_tools: TOOLS.iter().map(|name| (*name).into()).collect(),
            enabled_skills: vec![],
            auto_skills: false,
            session_id: Some(session_id.clone()),
        };
        let mut response = state
            .agent
            .chat_for(&chat, ModelPurpose::Channel)
            .await
            .map_err(|_| {
                AppError::Internal("gateway_channel_model_failed; outcome requires review".into())
            })?;
        if response
            .session_id
            .as_deref()
            .is_some_and(|id| id != session_id)
        {
            return Err(AppError::Internal(
                "gateway_channel_response_identity_mismatch".into(),
            ));
        }
        // The host owns sessions; the native model adapter normally returns None.
        response.session_id = Some(session_id.clone());
        let mut messages = chat.messages;
        messages.push(response.message.clone());
        Ok::<_, AppError>((guard, messages, response))
    })
    .await
    .map_err(|_| {
        AppError::Internal("gateway_channel_execution_timeout; outcome requires review".into())
    })??;
    let (guard, messages, response) = prepared;
    let id = session_id;
    // Keep both permits inside the blocking DB closure. A commit timeout is an
    // unknown outcome, and must not release its slot or session lock early.
    let commit = with_sessions(&state, move |store| {
        let _ownership = (permit, guard);
        store.insert(id, SessionRecord::new(messages))?;
        store.flush()
    });
    tokio::time::timeout(COMMIT_TIMEOUT, commit)
        .await
        .map_err(|_| {
            AppError::Internal("gateway_channel_commit_timeout; outcome requires review".into())
        })??;
    Ok(Json(ChannelResponse {
        protocol: 1,
        backend_id: state.agent.config().name.clone(),
        request_id: request.request_id,
        binding_id: request.binding_id,
        response,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;
    use axum::{
        body::Body,
        http::{Method, StatusCode},
        routing::post,
        Router,
    };
    use jiaclaw_core::ModelRoute;
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };
    use tower::ServiceExt;

    struct Fixture {
        root: PathBuf,
        state: AppState,
        requests: Arc<Mutex<Vec<Value>>>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Fixture {
        async fn new() -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let model_url = format!("http://{}", listener.local_addr().unwrap());
            let app = Router::new().route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
                recorded.lock().unwrap().push(body);
                async { Json(json!({"choices":[{"message":{"role":"assistant","content":"private channel reply"},"finish_reason":"stop"}]})) }
            }));
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let root =
                std::env::temp_dir().join(format!("jiaclaw-tenant-channel-{}", Uuid::new_v4()));
            std::fs::create_dir_all(root.join("workspace")).unwrap();
            let mut state = crate::tests::test_state_for_workspace(root.join("workspace"));
            let mut config = state.agent.config().clone();
            config.name = "tenant-alice".into();
            config.http.gateway_channel_chat = true;
            config.provider.provider_type = "brokerrouter".into();
            config.provider.base_url = model_url;
            config.provider.api_key = Some("channel-local-fixture-key".into());
            config.routing.chat = Some(ModelRoute {
                model: "wrong-chat-route".into(),
                temperature: None,
                max_tokens: None,
            });
            config.routing.channel = Some(ModelRoute {
                model: "exact-channel-route".into(),
                temperature: Some(0.25),
                max_tokens: Some(123),
            });
            state.agent = Arc::new(JiaClawAgent::new(config).unwrap());
            state.sessions = Arc::new(Mutex::new(
                SessionStore::open(&root.join("sessions.sqlite3")).unwrap(),
            ));
            state.persist_enabled = true;
            state.api_token = Some("private-fixture-token".into());
            validate_config(state.agent.config(), state.api_token.as_deref()).unwrap();
            validate_tools(&state.agent).unwrap();
            Self {
                root,
                state,
                requests,
                server,
            }
        }
        fn cleanup(self) {
            self.server.abort();
            drop(self.state);
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }
    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer private-fixture-token".parse().unwrap(),
        );
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers
    }
    fn body(binding: Uuid) -> Value {
        json!({"request_id":Uuid::now_v7().to_string(),"binding_id":binding.to_string(),"prompt":"hello"})
    }
    async fn post_body(state: AppState, value: Value) -> Result<Json<ChannelResponse>, AppError> {
        let request = Request::builder()
            .method(Method::POST)
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap();
        chat(State(state), headers(), request).await
    }
    async fn wait_busy(state: &AppState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while state.gateway_channel_permit.available_permits() > 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn wait_idle(state: &AppState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while state.gateway_channel_permit.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn configuration_defaults_closed_and_rejects_autonomous_backends() {
        let mut config = AgentConfig::default();
        assert!(!config.http.gateway_channel_chat);
        validate_config(&config, None).unwrap();
        config.http.gateway_channel_chat = true;
        config.provider.provider_type = "stub".into();
        assert!(validate_config(&config, None).is_err());
        validate_config(&config, Some("private-token")).unwrap();
        config.http.persist = false;
        assert!(validate_config(&config, Some("private-token")).is_err());
        config.http.persist = true;
        config.heartbeat.enabled = true;
        assert!(validate_config(&config, Some("private-token")).is_err());
        config.heartbeat.enabled = false;
        config.scheduler.enabled = true;
        assert!(validate_config(&config, Some("private-token")).is_err());
        config.scheduler.gateway_driven = true;
        validate_config(&config, Some("private-token")).unwrap();
        config.http.webhook_secret = Some("legacy-webhook".into());
        assert!(validate_config(&config, Some("private-token")).is_err());
        config.http.webhook_secret = None;
        config.provider.provider_type = "openai".into();
        assert!(validate_config(&config, Some("private-token")).is_err());
        config.provider.provider_type = "stub".into();
        config.http.channels.push(serde_json::from_value(json!({"channel":"telegram","installation_id":"123","allowed_senders":["45"],"allowed_conversations":["45"],"enabled_tools":["json_query"]})).unwrap());
        assert!(validate_config(&config, Some("private-token")).is_err());
    }

    #[tokio::test]
    async fn default_routes_are_closed_and_authentication_is_unambiguous() {
        let fixture = Fixture::new().await;
        let mut closed = fixture.state.clone();
        let mut config = closed.agent.config().clone();
        config.http.gateway_channel_chat = false;
        closed.agent = Arc::new(JiaClawAgent::new(config).unwrap());
        let router = crate::build_router(closed);
        for (method, path) in [
            (Method::GET, "/internal/gateway/channel/status"),
            (Method::POST, "/internal/gateway/channel/chat"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        drop(router);
        let valid = status(
            State(fixture.state.clone()),
            headers(),
            Uri::from_static("/internal/gateway/channel/status"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            valid,
            json!({"protocol":1,"backend_id":"tenant-alice","max_run_seconds":120,"tools":TOOLS,"mode":"gateway"})
        );
        for bad in [
            HeaderMap::new(),
            {
                let mut h = headers();
                h.append(
                    header::AUTHORIZATION,
                    "Bearer private-fixture-token".parse().unwrap(),
                );
                h
            },
            {
                let mut h = headers();
                h.insert("x-api-token", "private-fixture-token".parse().unwrap());
                h
            },
            {
                let mut h = headers();
                h.insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
                h
            },
        ] {
            assert!(matches!(
                status(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Uri::from_static("/internal/gateway/channel/status")
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                chat(
                    State(fixture.state.clone()),
                    bad,
                    Request::new(Body::from("{}"))
                )
                .await,
                Err(AppError::Unauthorized)
            ));
        }
        let controls = fixture
            .state
            .tenant_control_permits
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();
        assert!(matches!(
            status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/gateway/channel/status")
            )
            .await,
            Err(AppError::ChannelUnavailable)
        ));
        drop(controls);
        assert!(fixture.requests.lock().unwrap().is_empty());
        fixture.cleanup();
    }

    #[tokio::test]
    async fn forged_authority_and_noncanonical_or_oversize_requests_never_reach_model() {
        let fixture = Fixture::new().await;
        let valid = body(Uuid::new_v4());
        for key in [
            "session_id",
            "tools",
            "enabled_tools",
            "enabled_skills",
            "auto_skills",
            "model",
            "purpose",
            "backend_id",
            "user_id",
        ] {
            let mut request = valid.clone();
            request[key] = json!("forged");
            assert!(matches!(
                post_body(fixture.state.clone(), request).await,
                Err(AppError::BadRequest(_))
            ));
        }
        for (key, value) in [
            ("request_id", json!(Uuid::new_v4().to_string())),
            ("request_id", json!("0195CA8E-0000-7000-8000-00000000000A")),
            ("binding_id", json!(Uuid::nil().to_string())),
            ("binding_id", json!("not-uuid")),
            ("prompt", json!(null)),
            ("prompt", json!(" \n ")),
            ("prompt", json!("界".repeat(MAX_PROMPT_BYTES / 3 + 1))),
        ] {
            let mut request = valid.clone();
            request[key] = value;
            assert!(matches!(
                post_body(fixture.state.clone(), request).await,
                Err(AppError::BadRequest(_))
            ));
        }
        assert!(fixture.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 1);
        fixture.cleanup();
    }

    #[tokio::test]
    async fn positional_arrays_duplicate_fields_and_query_parameters_are_rejected() {
        let fixture = Fixture::new().await;
        let valid = body(Uuid::new_v4());
        let array = json!([valid["request_id"], valid["binding_id"], valid["prompt"]]);
        assert!(matches!(
            post_body(fixture.state.clone(), array).await,
            Err(AppError::BadRequest(_))
        ));
        let duplicate = format!(
            "{{\"request_id\":{},\"binding_id\":{},\"prompt\":\"hello\",\"prompt\":\"again\"}}",
            valid["request_id"], valid["binding_id"]
        );
        let request = Request::new(Body::from(duplicate));
        assert!(matches!(
            chat(State(fixture.state.clone()), headers(), request).await,
            Err(AppError::BadRequest(_))
        ));
        let request = Request::builder()
            .uri("/internal/gateway/channel/chat?model=forged")
            .body(Body::from(valid.to_string()))
            .unwrap();
        assert!(matches!(
            chat(State(fixture.state.clone()), headers(), request).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/gateway/channel/status?backend=other")
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        assert!(fixture.requests.lock().unwrap().is_empty());
        fixture.cleanup();
    }

    #[tokio::test]
    async fn channel_route_exact_tools_and_binding_session_are_persisted() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let request = body(binding);
        let reply = post_body(fixture.state.clone(), request.clone())
            .await
            .unwrap()
            .0;
        let sid = format!("tg-{}", binding.simple());
        assert_eq!(reply.protocol, 1);
        assert_eq!(reply.backend_id, "tenant-alice");
        assert_eq!(reply.request_id, request["request_id"]);
        assert_eq!(reply.binding_id, binding.to_string());
        assert_eq!(reply.response.session_id.as_deref(), Some(sid.as_str()));
        assert_eq!(
            reply.response.routing.as_ref().unwrap().purpose,
            ModelPurpose::Channel
        );
        assert_eq!(reply.response.message.content, "private channel reply");
        {
            let requests = fixture.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["model"], "exact-channel-route");
            assert_eq!(requests[0]["temperature"], 0.25);
            assert_eq!(requests[0]["max_tokens"], 123);
            let mut tools = requests[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            tools.sort_unstable();
            assert_eq!(tools, TOOLS);
        }
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        let saved = reopened.get(&sid).unwrap().unwrap();
        assert_eq!(saved.messages.len(), 2);
        assert_eq!(saved.messages[0].content, "hello");
        assert_eq!(saved.messages[1].content, "private channel reply");
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn disconnect_keeps_execution_slot_and_commits_once_after_session_unlock() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let sid = format!("tg-{}", binding.simple());
        let turn = session_turn_lock(&fixture.state, &sid).await;
        let state = fixture.state.clone();
        let caller = tokio::spawn(async move { post_body(state, body(binding)).await });
        wait_busy(&fixture.state).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 0);
        assert!(matches!(
            post_body(fixture.state.clone(), body(binding)).await,
            Err(AppError::JobConflict(_))
        ));
        assert!(status(
            State(fixture.state.clone()),
            headers(),
            Uri::from_static("/internal/gateway/channel/status")
        )
        .await
        .is_ok());
        assert!(fixture.requests.lock().unwrap().is_empty());
        drop(turn);
        wait_idle(&fixture.state).await;
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert_eq!(
            fixture
                .state
                .sessions
                .lock()
                .unwrap()
                .get(&sid)
                .unwrap()
                .unwrap()
                .messages
                .len(),
            2
        );
        fixture.cleanup();
    }

    #[tokio::test(start_paused = true)]
    async fn execution_deadline_does_not_start_a_late_model_after_lock_timeout() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let sid = format!("tg-{}", binding.simple());
        let turn = session_turn_lock(&fixture.state, &sid).await;
        let state = fixture.state.clone();
        let caller = tokio::spawn(async move { post_body(state, body(binding)).await });
        wait_busy(&fixture.state).await;
        tokio::time::advance(Duration::from_secs(121)).await;
        assert!(
            matches!(caller.await.unwrap(),Err(AppError::Internal(message)) if message.contains("execution_timeout"))
        );
        drop(turn);
        tokio::task::yield_now().await;
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 1);
        assert!(fixture.requests.lock().unwrap().is_empty());
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .get(&sid)
            .unwrap()
            .is_none());
        fixture.cleanup();
    }

    #[tokio::test]
    async fn session_commit_failure_never_returns_a_successful_envelope() {
        let fixture = Fixture::new().await;
        {
            let mut store = fixture.state.sessions.lock().unwrap();
            let SessionStore::Sqlite { conn, .. } = &mut *store else {
                panic!("SQLite fixture")
            };
            conn.execute_batch("CREATE TRIGGER reject_channel_commit BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'fixture commit failure'); END;").unwrap();
        }
        let binding = Uuid::new_v4();
        assert!(matches!(
            post_body(fixture.state.clone(), body(binding)).await,
            Err(AppError::Internal(message)) if message == "session_storage_error"
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .get(&format!("tg-{}", binding.simple()))
            .unwrap()
            .is_none());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 1);
        fixture.cleanup();
    }
}
