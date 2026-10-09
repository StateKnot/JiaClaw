// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicit standalone HTTP turn ownership. No automatic execution after restart.

use super::http_turn_store::{Receipt, MAX_HISTORY_BYTES, MAX_RESULT_BYTES};
use super::{session_turn_mutex, with_session_turn, with_sessions, AppError, AppState};
use anyhow::{ensure, Result};
use axum::{
    body::Bytes,
    extract::{FromRequest, Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use jiaclaw::{ChatEvents, ChatProgress};
use jiaclaw_core::{AgentConfig, ChatMessage, ChatRequest, MessageRole, ModelPurpose, RunStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

#[path = "http_turn_stream.rs"]
mod delivery;

pub(super) fn reserved_session(id: &str) -> bool {
    id.starts_with("http:")
}

pub(super) fn validate_config(config: &AgentConfig, token: Option<&str>) -> Result<()> {
    if !config.http.tracked_turns {
        return Ok(());
    }
    ensure!(
        (1..=300).contains(&config.http.tracked_turn_timeout_secs),
        "tracked HTTP turn timeout must be 1..=300 seconds"
    );
    ensure!(
        config.http.persist && token.is_some_and(|t| !t.trim().is_empty()),
        "tracked HTTP turns require SQLite persistence and an API Token"
    );
    ensure!(
        config.provider.provider_type == "brokerrouter" && config.model_calls.enabled,
        "tracked HTTP turns require Brokerrouter and an explicitly enabled model_calls ledger"
    );
    ensure!(
        config
            .effective_tool_timeout_secs()
            .is_some_and(|s| (1..=30).contains(&s)),
        "tracked HTTP turns require effective tool_timeout_secs in 1..=30"
    );
    ensure!(
        !config.http.gateway_channel_chat && !config.scheduler.gateway_driven,
        "tracked HTTP turns are not qualified for gateway-driven tenant backends"
    );
    Ok(())
}

struct Active {
    id: String,
    progress: ChatProgress,
}
pub(super) struct Runtime {
    owners: Arc<Semaphore>,
    active: Mutex<Option<Active>>,
    budget: Duration,
}
struct Owner {
    runtime: Arc<Runtime>,
    id: String,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Owner {
    fn drop(&mut self) {
        let mut active = self
            .runtime
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.as_ref().is_some_and(|a| a.id == self.id) {
            *active = None;
        }
    }
}
impl Runtime {
    pub(super) fn new(budget: Duration) -> Arc<Self> {
        Arc::new(Self {
            owners: Arc::new(Semaphore::new(1)),
            active: Mutex::new(None),
            budget,
        })
    }
    fn is_active(&self, id: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.id == id)
    }
    fn cancel(&self, id: &str) {
        if let Some(a) = self.active.lock().unwrap().as_ref().filter(|a| a.id == id) {
            a.progress.cancel();
        }
    }
    fn register(&self, id: String, progress: ChatProgress) {
        let mut active = self.active.lock().unwrap();
        // stop() closes admission before inspecting this same active-owner lock.
        // Cover a permit claimed before stop whose registration finishes later.
        if self.owners.is_closed() {
            progress.cancel();
        }
        *active = Some(Active { id, progress });
    }
    pub(super) fn stop(&self) {
        self.owners.close();
        if let Some(a) = self.active.lock().unwrap().as_ref() {
            a.progress.cancel();
        }
    }
    pub(super) async fn drain_until(&self, deadline: Instant) {
        let drained = tokio::time::timeout_at(deadline, async {
            while self.owners.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!("HTTP turn owner exceeded shared shutdown grace; retained identity requires review after restart");
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    session_id: String,
    prompt: String,
    enabled_tools: Vec<String>,
    #[serde(default)]
    enabled_skills: Vec<String>,
}
impl Submission {
    fn validate(&self, state: &AppState) -> Result<(), AppError> {
        let session_uuid = self.session_id.strip_prefix("http:").unwrap_or_default();
        if !super::jobs::valid_creation_id(session_uuid)
            || self.prompt.trim().is_empty()
            || self.prompt.len() > 32 * 1024
            || self.enabled_tools.len() > 128
            || self.enabled_skills.len() > 16
        {
            return Err(AppError::BadRequest("invalid_http_turn_submission".into()));
        }
        // Empty native tool selection means all registered tools. Require an explicit
        // allowlist here rather than accidentally expanding an omitted selection.
        if self.enabled_tools.is_empty() && !state.agent.tools().list().is_empty() {
            return Err(AppError::BadRequest(
                "http_turn_requires_explicit_tools".into(),
            ));
        }
        let mut names = std::collections::HashSet::new();
        for name in &self.enabled_tools {
            if name.len() > 128 || !names.insert(name) || state.agent.tools().get(name).is_none() {
                return Err(AppError::BadRequest("invalid_http_turn_tool".into()));
            }
        }
        names.clear();
        for name in &self.enabled_skills {
            if name.is_empty()
                || name.len() > 128
                || !names.insert(name)
                || !state.agent.skills().iter().any(|s| s.name == *name)
            {
                return Err(AppError::BadRequest("invalid_http_turn_skill".into()));
            }
        }
        Ok(())
    }
    fn fingerprint(&self) -> Result<String, AppError> {
        serde_json::to_vec(self)
            .map(|v| format!("{:x}", Sha256::digest(v)))
            .map_err(|_| AppError::Internal("http_turn_identity_error".into()))
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    use subtle::ConstantTimeEq;
    let mut authorization = headers.get_all(axum::http::header::AUTHORIZATION).iter();
    let token = authorization
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let valid = authorization.next().is_none()
        && !headers.contains_key("x-api-token")
        && token
            .zip(state.api_token.as_deref())
            .is_some_and(|(actual, expected)| {
                actual.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() == 1
            });
    if !valid {
        return Err(AppError::Unauthorized);
    }
    if !state.persist_enabled {
        return Err(AppError::HttpTurnUnavailable);
    }
    Ok(())
}
fn validate_id(id: &str) -> Result<(), AppError> {
    if !super::jobs::valid_creation_id(id) {
        return Err(AppError::BadRequest(
            "canonical UUIDv4 request identity required".into(),
        ));
    }
    Ok(())
}
fn receipt_response(state: &AppState, receipt: &Receipt, status: StatusCode) -> Response {
    let active = state
        .http_turns
        .as_ref()
        .is_some_and(|r| r.is_active(&receipt.id));
    (
        status,
        Json(json!({"protocol":1,"receipt":receipt,"active":active})),
    )
        .into_response()
}

/// Four finite storage/control owners, retained by the actual blocking work even
/// if an HTTP waiter goes away. None of these requests wait for capacity.
async fn control<T: Send + 'static>(
    state: &AppState,
    operation: impl FnOnce(&mut super::SessionStore) -> Result<T> + Send + 'static,
) -> Result<T, AppError> {
    let permit = state
        .http_turn_controls
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::JobConflict("http_turn_control_busy".into()))?;
    with_sessions(state, move |store| {
        let _permit = permit;
        operation(store)
    })
    .await
}

async fn read_body(state: &AppState, request: Request) -> Result<Bytes, Response> {
    let _permit = state
        .http_turn_controls
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::JobConflict("http_turn_control_busy".into()).into_response())?;
    match tokio::time::timeout(Duration::from_secs(5), Bytes::from_request(request, state)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(rejection)) => Err(rejection.into_response()),
        Err(_) => Err((
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({"error":"http_turn_body_timeout"})),
        )
            .into_response()),
    }
}

pub(super) async fn capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &headers)?;
    Ok(Json(
        json!({"protocol":1,"enabled":state.http_turns.is_some(),"streaming":state.http_turns.is_some(),"listing":true,"stream_suffix":"/stream","max_active":1,"turn_budget_secs":state.agent.config().http.tracked_turn_timeout_secs,"max_identities":10000,"max_retained_results":32,"session_prefix":"http:","max_stream_wire_bytes":delivery::WIRE_BYTES,"max_preview_round_bytes":delivery::ROUND_BYTES,"max_preview_total_bytes":delivery::PREVIEW_BYTES}),
    ))
}

fn catalog_query(raw: Option<&str>) -> Result<(String, usize, usize), AppError> {
    let invalid = || AppError::BadRequest("invalid_http_turn_query".into());
    let raw = raw.unwrap_or("");
    if raw.len() > 128 {
        return Err(invalid());
    }
    let (mut filter, mut limit, mut offset) = ("unresolved".to_owned(), 20, 0);
    let mut seen = std::collections::HashSet::new();
    if !raw.is_empty() {
        for part in raw.split('&') {
            let (key, value) = part.split_once('=').ok_or_else(invalid)?;
            if !seen.insert(key) {
                return Err(invalid());
            }
            let number = || -> Result<usize, AppError> {
                if value.is_empty()
                    || (value.len() > 1 && value.starts_with('0'))
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(invalid());
                }
                value.parse().map_err(|_| invalid())
            };
            match key {
                "state"
                    if matches!(
                        value,
                        "all" | "unresolved" | "running" | "completed" | "needs_review"
                    ) =>
                {
                    filter = value.into()
                }
                "limit" => limit = number()?,
                "offset" => offset = number()?,
                _ => return Err(invalid()),
            }
        }
    }
    if !(1..=50).contains(&limit) || offset > 10000 {
        return Err(invalid());
    }
    Ok((filter, limit, offset))
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    let (filter, limit, offset) = catalog_query(raw.as_deref())?;
    let query_filter = filter.clone();
    let (turns, has_more) = control(&state, move |store| {
        store.list_http_turns(&query_filter, limit, offset)
    })
    .await?;
    Ok(([("cache-control", "no-store")], Json(json!({"protocol":1,"state":filter,"limit":limit,"offset":offset,"has_more":has_more,"turns":turns}))).into_response())
}
pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    validate_id(&id)?;
    let receipt = control(&state, move |store| store.http_receipt(&id, None))
        .await?
        .ok_or(AppError::HttpTurnNotFound)?;
    Ok(receipt_response(&state, &receipt, StatusCode::OK))
}

pub(super) async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    submit_inner(state, headers, id, request, false).await
}

pub(super) async fn submit_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    submit_inner(state, headers, id, request, true).await
}

async fn submit_inner(
    state: AppState,
    headers: HeaderMap,
    id: String,
    request: Request,
    streaming: bool,
) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    validate_id(&id)?;
    let body = match read_body(&state, request).await {
        Ok(body) => body,
        Err(response) => return Ok(response),
    };
    let submission: Submission = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("invalid_http_turn_submission".into()))?;
    // Normalize typed defaults before lookup. A retry never re-authorizes an old
    // payload against changed skills/config or dispatches another operation.
    let hash = submission.fingerprint()?;
    let query_id = id.clone();
    let query_hash = hash.clone();
    if let Some(receipt) = control(&state, move |store| {
        store.http_receipt(&query_id, Some(&query_hash))
    })
    .await?
    {
        return Ok(receipt_response(&state, &receipt, StatusCode::OK));
    }
    let runtime = state
        .http_turns
        .as_ref()
        .ok_or(AppError::HttpTurnUnavailable)?
        .clone();
    submission.validate(&state)?;
    let permit = runtime
        .owners
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::JobConflict("http_turn_busy".into()))?;
    let guard = session_turn_mutex(&state, &submission.session_id)
        .try_lock_owned()
        .map_err(|_| AppError::JobConflict("http_turn_session_busy".into()))?;
    let (progress, events) = ChatProgress::channel_for_turn(&id)
        .map_err(|_| AppError::JobConflict("http_turn_delivery_busy".into()))?;
    // An opaque snapshot binds the admitted credential/config epoch. It is not
    // a persisted secret or a guarantee of the subsequently constructed wire
    // prompt; the model ledger owns that exact prepared payload identity.
    let context = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(state.agent.config(), &state.api_token))
                .map_err(|_| AppError::Internal("http_turn_context_error".into()))?
        )
    );
    let (sender, receiver) = oneshot::channel();
    let worker_state = state.clone();
    // No await between claiming capacity and launching its sole owner. Client
    // disconnect/lost admission reply cannot abandon a committed identity.
    runtime.register(id.clone(), progress.clone());
    let owner = Arc::new(Owner {
        runtime,
        id: id.clone(),
        _permit: permit,
    });
    let deadline = Instant::now() + owner.runtime.budget;
    let mut events = Some(events);
    let worker_events = if streaming { None } else { events.take() };
    let (terminal_tx, terminal_rx) = oneshot::channel();
    let terminal_tx = streaming.then_some(terminal_tx);
    let worker_progress = progress.clone();
    let worker_owner = owner.clone();
    tokio::spawn(async move {
        supervise(
            worker_state,
            id,
            submission,
            hash,
            context,
            guard,
            worker_progress,
            worker_events,
            sender,
            terminal_tx,
            worker_owner,
            deadline,
        )
        .await;
    });
    let (receipt, created) = receiver.await.map_err(|_| {
        AppError::Internal(
            "http_turn_owner_failed; GET the original identity before proceeding".into(),
        )
    })??;
    if streaming && created {
        return Ok(delivery::response(
            receipt,
            events.expect("stream has the sole progress receiver"),
            progress,
            terminal_rx,
            owner,
            deadline,
        ));
    }
    Ok(receipt_response(
        &state,
        &receipt,
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
    ))
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    state: AppState,
    id: String,
    submission: Submission,
    hash: String,
    context: String,
    guard: tokio::sync::OwnedMutexGuard<()>,
    progress: ChatProgress,
    mut events: Option<ChatEvents>,
    admitted: oneshot::Sender<Result<(Receipt, bool), AppError>>,
    terminal: Option<oneshot::Sender<Result<Receipt, ()>>>,
    owner: Arc<Owner>,
    deadline: Instant,
) {
    let admission_id = id.clone();
    let sid = submission.session_id.clone();
    let storage_owner = owner.clone();
    let admission = with_sessions(&state, move |store| {
        let _owner = storage_owner;
        let receipt = store.admit_http_turn(&admission_id, &sid, &hash, &context)?;
        Ok((receipt, guard))
    })
    .await;
    let ((receipt, created), guard) = match admission {
        Ok(value) => value,
        Err(error) => {
            let _ = admitted.send(Err(error));
            return;
        }
    };
    let _ = admitted.send(Ok((receipt, created)));
    if !created {
        return;
    }
    let outcome = execute(
        &state,
        &submission,
        &progress,
        events.as_mut(),
        deadline,
        &owner,
    )
    .await;
    let (history, result, error, review) = match outcome {
        Ok((history, result, review)) => (Some(history), Some(result), None, review),
        Err(cause) => {
            tracing::warn!(request_id=%id,error=%cause,"tracked HTTP turn stopped; inspect receipts and effects; no replay");
            (
                None,
                None,
                Some(if Instant::now() >= deadline {
                    "turn_budget_exceeded_needs_review"
                } else if progress.is_cancelled() {
                    "turn_cancelled_needs_review"
                } else {
                    "turn_failed_needs_review"
                }),
                true,
            )
        }
    };
    let completed = with_session_turn(&state, guard, move |store| {
        let _owner = owner;
        store.finish_http_turn(&id, history, result, error, review)
    })
    .await;
    if completed.is_err() {
        tracing::error!("HTTP turn terminal transaction failed; original admission remains unresolved; no replay");
    }
    if let Some(terminal) = terminal {
        // Only the actual atomic transaction may certify the final receipt.
        let _ = terminal.send(completed.map_err(|_| ()));
    }
}

async fn execute(
    state: &AppState,
    submission: &Submission,
    progress: &ChatProgress,
    mut events: Option<&mut ChatEvents>,
    deadline: Instant,
    owner: &Arc<Owner>,
) -> Result<(Vec<ChatMessage>, Value, bool)> {
    let sid = submission.session_id.clone();
    let storage_owner = owner.clone();
    let mut messages = with_sessions(state, move |store| {
        let _owner = storage_owner;
        Ok(store.get(&sid)?.map_or_else(Vec::new, |r| r.messages))
    })
    .await
    .map_err(|_| anyhow::anyhow!("session storage unavailable"))?;
    messages.push(ChatMessage {
        role: MessageRole::User,
        content: submission.prompt.clone(),
    });
    ensure!(
        serde_json::to_vec(&messages)?.len() <= MAX_HISTORY_BYTES,
        "HTTP history budget exceeded"
    );
    ensure!(messages.len() < jiaclaw_core::MAX_SESSION_MESSAGES, "HTTP history message budget exceeded; explicitly compact/import history or use a fresh session");
    ensure!(
        !progress.is_cancelled() && Instant::now() < deadline,
        "turn stopped before model dispatch"
    );
    let timer = tokio::time::sleep_until(deadline);
    tokio::pin!(timer);
    let mut timed_out = false;
    let request = ChatRequest {
        messages,
        session_id: Some(submission.session_id.clone()),
        enabled_tools: submission.enabled_tools.clone(),
        enabled_skills: submission.enabled_skills.clone(),
        auto_skills: false,
    };
    let mut response = {
        let chat = state
            .agent
            .chat_stream_for(&request, ModelPurpose::Chat, progress);
        tokio::pin!(chat);
        let mut receiving = events.is_some();
        loop {
            tokio::select! {
                biased;
                ()=&mut timer,if !timed_out=>{timed_out=true;progress.cancel();},
                result=&mut chat=>break result?,
                event=async { events.as_mut().expect("JSON progress drain").next().await },if receiving=>{if event.is_none(){receiving=false;}}
            }
        }
    };
    response.session_id = Some(submission.session_id.clone());
    // No raw tool arguments/results are duplicated into the HTTP result ledger.
    let result = json!({"reply":response.message.content,"status":response.status,"routing":response.routing,"tool_names":response.tool_calls.iter().map(|t|&t.tool_name).collect::<Vec<_>>()});
    ensure!(
        serde_json::to_vec(&result)?.len() <= MAX_RESULT_BYTES,
        "HTTP result budget exceeded; inspect receipts"
    );
    let review = response.status != RunStatus::Completed;
    let mut history = request.messages;
    history.push(response.message);
    ensure!(
        serde_json::to_vec(&history)?.len() <= MAX_HISTORY_BYTES,
        "HTTP final history budget exceeded; inspect receipts"
    );
    Ok((history, result, review))
}

pub(super) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    validate_id(&id)?;
    let runtime = state.http_turns.clone();
    let receipt = control(&state, move |store| {
        let receipt = store.request_http_cancel(&id)?;
        // The real control owner must signal after persistence even when its
        // HTTP waiter has already been cancelled or disconnected.
        if let Some(runtime) = runtime {
            runtime.cancel(&id);
        }
        Ok(receipt)
    })
    .await?;
    Ok(receipt_response(&state, &receipt, StatusCode::OK))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Review {
    decision: String,
    note: String,
}
pub(super) async fn review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    validate_id(&id)?;
    let bytes = match read_body(&state, request).await {
        Ok(body) => body,
        Err(response) => return Ok(response),
    };
    let body: Review = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid_http_turn_review".into()))?;
    if body.decision != "abandon" || body.note.trim().is_empty() || body.note.len() > 1024 {
        return Err(AppError::BadRequest(
            "review requires decision=abandon and a 1..1024 byte note".into(),
        ));
    }
    let query_id = id.clone();
    let receipt = control(&state, move |store| store.http_receipt(&query_id, None))
        .await?
        .ok_or(AppError::HttpTurnNotFound)?;
    if state.http_turns.as_ref().is_some_and(|r| r.is_active(&id)) {
        return Err(AppError::JobConflict("http_turn_active".into()));
    }
    let guard = session_turn_mutex(&state, &receipt.session_id)
        .try_lock_owned()
        .map_err(|_| AppError::JobConflict("http_turn_session_busy".into()))?;
    let permit = state
        .http_turn_controls
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::JobConflict("http_turn_control_busy".into()))?;
    let receipt = with_session_turn(&state, guard, move |store| {
        let _permit = permit;
        store.review_http_turn(&id, &body.note)
    })
    .await?;
    Ok(receipt_response(&state, &receipt, StatusCode::OK))
}
pub(super) async fn purge_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    validate_id(&id)?;
    let receipt = control(&state, move |store| store.purge_http_result(&id)).await?;
    Ok(receipt_response(&state, &receipt, StatusCode::OK))
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_query_is_finite_unambiguous_and_unencoded() {
        assert_eq!(
            super::catalog_query(None).unwrap(),
            ("unresolved".into(), 20, 0)
        );
        assert_eq!(
            super::catalog_query(Some("offset=10000&limit=50&state=all")).unwrap(),
            ("all".into(), 50, 10000)
        );
        for raw in [
            "limit=0",
            "limit=51",
            "offset=10001",
            "limit=01",
            "limit=+1",
            "limit=%31",
            "state=%61ll",
            "state=all&state=running",
            "limit=1&limit=1",
            "offset=-1",
            "offset=18446744073709551616",
            "state=ALL",
            "active=true",
            "limit=",
            "state=all&",
            "=all",
            "state=all=running",
        ] {
            assert!(super::catalog_query(Some(raw)).is_err(), "{raw}");
        }
        assert!(super::catalog_query(Some(&"x".repeat(129))).is_err());
    }
    use super::*;

    #[test]
    fn an_owner_claimed_before_stop_cannot_dispatch_after_registration() {
        let runtime = Runtime::new(Duration::from_secs(300));
        let permit = runtime.owners.clone().try_acquire_owned().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let (progress, _events) = ChatProgress::channel_for_turn(&id).unwrap();
        assert!(!progress.is_cancelled());
        runtime.stop();
        assert!(runtime.owners.is_closed());
        assert_eq!(runtime.owners.available_permits(), 0);
        runtime.register(id.clone(), progress.clone());
        assert!(runtime.is_active(&id));
        assert!(
            progress.is_cancelled(),
            "stop must cover a claimed owner whose registration finishes later"
        );
        drop(permit);
    }

    #[tokio::test]
    async fn cancelled_cancel_waiter_still_signals_after_durable_intent() {
        let directory = std::env::temp_dir().join(format!(
            "jiaclaw-http-cancel-owner-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("sessions.sqlite3");
        let mut state = super::super::tests::test_state_for_workspace(directory.clone());
        state.api_token = Some("fixture-operator-secret".into());
        state.persist_enabled = true;
        state.persist_path = Arc::new(database.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let session = format!("http:{}", uuid::Uuid::new_v4());
        let mut store = super::super::SessionStore::open(&database).unwrap();
        store
            .admit_http_turn(&id, &session, &"a".repeat(64), &"b".repeat(64))
            .unwrap();
        state.sessions = Arc::new(Mutex::new(store));
        let runtime = Runtime::new(Duration::from_secs(300));
        let permit = runtime.owners.clone().try_acquire_owned().unwrap();
        let (progress, _events) = ChatProgress::channel_for_turn(&id).unwrap();
        runtime.register(id.clone(), progress.clone());
        let owner = Arc::new(Owner {
            runtime: runtime.clone(),
            id: id.clone(),
            _permit: permit,
        });
        state.http_turns = Some(runtime);
        let sessions = state.sessions.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _lock = sessions.lock().unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        });
        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer fixture-operator-secret".parse().unwrap(),
        );
        let waiter = tokio::spawn(cancel(State(state.clone()), headers, Path(id.clone())));
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.http_turn_controls.available_permits() == 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_eq!(state.http_turn_controls.available_permits(), 3);
        assert!(!progress.is_cancelled(), "no durable intent has run yet");
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.http_turn_controls.available_permits() != 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let reader = rusqlite::Connection::open(&database).unwrap();
        let stored: bool = reader
            .query_row(
                "SELECT cancel_requested FROM http_turns WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored, "the actual worker must persist the original intent");
        let signalled = progress.is_cancelled();
        drop(reader);
        drop(state);
        drop(owner);
        std::fs::remove_dir_all(directory).unwrap();
        assert!(
            signalled,
            "persisted cancellation must stop dispatch even after its waiter is gone"
        );
    }

    #[test]
    fn turn_authentication_rejects_duplicate_and_alternate_credentials() {
        let mut state = super::super::tests::test_state_for_workspace(
            std::env::temp_dir().join(format!("jiaclaw-http-auth-{}", uuid::Uuid::new_v4())),
        );
        state.api_token = Some("fixture-operator-secret".into());
        state.persist_enabled = true;
        let mut headers = HeaderMap::new();
        assert!(authorize(&state, &headers).is_err());
        headers.insert(
            "authorization",
            "Bearer fixture-operator-secret".parse().unwrap(),
        );
        assert!(authorize(&state, &headers).is_ok());
        headers.append(
            "authorization",
            "Bearer fixture-operator-secret".parse().unwrap(),
        );
        assert!(authorize(&state, &headers).is_err());
        headers.remove("authorization");
        headers.insert("x-api-token", "fixture-operator-secret".parse().unwrap());
        assert!(authorize(&state, &headers).is_err());
        headers.insert(
            "authorization",
            "Bearer fixture-operator-secret".parse().unwrap(),
        );
        assert!(authorize(&state, &headers).is_err());
        headers.remove("x-api-token");
        headers.insert("authorization", "Bearer wrong-secret".parse().unwrap());
        assert!(authorize(&state, &headers).is_err());
    }

    #[test]
    fn tracked_protocol_requires_explicit_authenticated_persistent_standalone_configuration() {
        let mut config = AgentConfig::default();
        assert!(!config.http.tracked_turns);
        config.http.tracked_turns = true;
        assert!(validate_config(&config, None).is_err());
        config.provider.provider_type = "brokerrouter".into();
        config.model_calls.enabled = true;
        config.tool_timeout_secs = Some(30);
        assert!(validate_config(&config, Some("operator-token")).is_ok());
        for bad in 0..6 {
            let mut config = config.clone();
            match bad {
                0 => config.http.persist = false,
                1 => config.model_calls.enabled = false,
                2 => config.tool_timeout_secs = Some(31),
                3 => config.http.gateway_channel_chat = true,
                4 => config.scheduler.gateway_driven = true,
                _ => config.http.tracked_turn_timeout_secs = 301,
            }
            assert!(validate_config(&config, Some("operator-token")).is_err());
        }
        assert!(validate_id(&uuid::Uuid::now_v7().to_string()).is_err());
        assert!(validate_id(&uuid::Uuid::new_v4().to_string().to_uppercase()).is_err());
    }

    #[tokio::test]
    async fn cancelled_control_waiter_retains_actual_queued_storage_capacity() {
        let state = super::super::tests::test_state_for_workspace(
            std::env::temp_dir().join(format!("jiaclaw-http-control-{}", uuid::Uuid::new_v4())),
        );
        let sessions = state.sessions.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _lock = sessions.lock().unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        });
        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let worker_state = state.clone();
        let (done_tx, done_rx) = oneshot::channel();
        let waiter = tokio::spawn(async move {
            control(&worker_state, move |_| {
                let _ = done_tx.send(());
                Ok(())
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.http_turn_controls.available_permits() == 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        let _ = waiter.await;
        let actual_held = state.http_turn_controls.available_permits();
        let _ = release_tx.send(());
        holder.join().unwrap();
        assert_eq!(
            actual_held, 3,
            "cancelling the waiter must not release its real blocking owner"
        );
        tokio::time::timeout(Duration::from_secs(1), done_rx)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.http_turn_controls.available_permits() != 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
