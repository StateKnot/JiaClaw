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
    extract::{Path, Request, State},
    http::{header, HeaderMap, Uri},
    Json,
};
use jiaclaw::JiaClawAgent;
use jiaclaw_core::{
    AgentConfig, ChatMessage, ChatRequest, ChatResponse, MessageRole, ModelPurpose,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
    execute_channel(state, request, session_id, permit, None).await
}

async fn execute_channel(
    state: AppState,
    request: ChannelRequest,
    session_id: String,
    permit: OwnedSemaphorePermit,
    recorded: Option<(BackendProtocol, PrivateChannelRequest)>,
) -> Result<Json<ChannelResponse>, AppError> {
    let protocol = recorded
        .as_ref()
        .map_or(1, |(channel, _)| channel.protocol());
    let prepared = tokio::time::timeout(Duration::from_secs(MAX_RUN_SECONDS), async {
        let guard = session_turn_lock(&state, &session_id).await;
        let (guard, permit) = if let Some((channel, request)) = recorded.clone() {
            // A timed-out/abandoned admission closure retains both ownership
            // guards until SQLite finishes, even if its result is never observed.
            let backend_id = state.agent.config().name.clone();
            let (admitted, guard, permit) = with_sessions(&state, move |store| {
                let admitted =
                    store.admit_private_backend_request(&request, &backend_id, channel)?;
                Ok((admitted, guard, permit))
            })
            .await?;
            if !admitted {
                return Err(AppError::JobConflict(
                    "channel request or event was already admitted; review its receipt".into(),
                ));
            }
            (guard, permit)
        } else {
            (guard, permit)
        };
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
        Ok::<_, AppError>((guard, messages, response, permit))
    })
    .await
    .map_err(|_| {
        AppError::Internal("gateway_channel_execution_timeout; outcome requires review".into())
    })??;
    let (guard, messages, response, permit) = prepared;
    let id = session_id;
    // Keep both permits inside the blocking DB closure. A commit timeout is an
    // unknown outcome, and must not release its slot or session lock early.
    let commit = with_sessions(&state, move |store| {
        let _ownership = (permit, guard);
        if let Some((channel, request)) = recorded {
            store.complete_private_backend_request(&request, &id, &messages, channel)
        } else {
            store.insert(id, SessionRecord::new(messages))?;
            store.flush()
        }
    });
    tokio::time::timeout(COMMIT_TIMEOUT, commit)
        .await
        .map_err(|_| {
            AppError::Internal("gateway_channel_commit_timeout; outcome requires review".into())
        })??;
    Ok(Json(ChannelResponse {
        protocol,
        backend_id: state.agent.config().name.clone(),
        request_id: request.request_id,
        binding_id: request.binding_id,
        response,
    }))
}

// Protocol 2 has its own permanent backend owner and operation ledger. It does
// not mutate the protocol-1 Telegram request shape or its session identities.
const SLACK_BINDING_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS gateway_slack_backend_binding (
 id INTEGER PRIMARY KEY CHECK(id=1),
 identity TEXT NOT NULL CHECK(json_valid(identity))
);
CREATE TRIGGER IF NOT EXISTS gateway_slack_backend_binding_immutable_insert BEFORE INSERT ON gateway_slack_backend_binding
 WHEN EXISTS(SELECT 1 FROM gateway_slack_backend_binding)
 BEGIN SELECT RAISE(ABORT,'Slack backend owner is immutable'); END;
CREATE TRIGGER IF NOT EXISTS gateway_slack_backend_binding_immutable_update BEFORE UPDATE ON gateway_slack_backend_binding
 BEGIN SELECT RAISE(ABORT,'Slack backend owner is immutable'); END;
CREATE TRIGGER IF NOT EXISTS gateway_slack_backend_binding_immutable_delete BEFORE DELETE ON gateway_slack_backend_binding
 BEGIN SELECT RAISE(ABORT,'Slack backend owner is immutable'); END;
CREATE TABLE IF NOT EXISTS gateway_slack_backend_requests (
 request_id TEXT PRIMARY KEY NOT NULL,
 binding_id TEXT NOT NULL,
 event_id TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL,
 prompt_sha256 BLOB NOT NULL CHECK(length(prompt_sha256)=32),
 status TEXT NOT NULL CHECK(status IN ('admitted','completed'))
);
CREATE TRIGGER IF NOT EXISTS gateway_slack_backend_requests_immutable_insert BEFORE INSERT ON gateway_slack_backend_requests
 WHEN EXISTS(SELECT 1 FROM gateway_slack_backend_requests WHERE request_id=NEW.request_id OR event_id=NEW.event_id)
 BEGIN SELECT RAISE(ABORT,'Slack request identity is already admitted'); END;
CREATE TRIGGER IF NOT EXISTS gateway_slack_backend_requests_immutable BEFORE UPDATE ON gateway_slack_backend_requests
 WHEN NEW.request_id<>OLD.request_id OR NEW.binding_id<>OLD.binding_id OR NEW.event_id<>OLD.event_id OR NEW.session_id<>OLD.session_id OR NEW.prompt_sha256<>OLD.prompt_sha256 OR OLD.status='completed'
 BEGIN SELECT RAISE(ABORT,'Slack request identity and completed receipt are immutable'); END;";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SlackBinding {
    protocol: u8,
    binding_id: String,
    user_id: String,
    backend_id: String,
    team_id: String,
    app_id: String,
    bot_user_id: String,
    bot_id: String,
    sender_id: String,
    conversation_id: String,
}

fn slack_id(value: &str, prefix: u8) -> bool {
    (2..=64).contains(&value.len())
        && (value.as_bytes()[0] == prefix || (prefix == b'U' && value.as_bytes()[0] == b'W'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn slack_event_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value.starts_with("Ev")
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

impl SlackBinding {
    fn validate(&self, backend: &str) -> Result<()> {
        ensure!(
            self.protocol == 2 && self.backend_id == backend,
            "Slack backend protocol or identity mismatch"
        );
        ensure!(
            canonical_id(&self.binding_id).is_some() && canonical_id(&self.user_id).is_some(),
            "canonical Slack owner UUIDs required"
        );
        for (value, prefix) in [
            (&self.team_id, b'T'),
            (&self.app_id, b'A'),
            (&self.bot_user_id, b'U'),
            (&self.bot_id, b'B'),
            (&self.sender_id, b'U'),
            (&self.conversation_id, b'D'),
        ] {
            ensure!(
                slack_id(value, prefix),
                "canonical Slack installation and DM identities required"
            );
        }
        ensure!(
            self.sender_id != self.bot_user_id,
            "Slack sender cannot be the bot user"
        );
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateChannelRequest {
    protocol: u8,
    binding_id: String,
    request_id: String,
    session_id: String,
    event_id: String,
    prompt: String,
}
// Request and receipt DTOs are shared; the route chooses a closed, internal
// protocol enum before validation. Clients cannot select a ledger or session.
#[cfg(test)]
type SlackRequest = PrivateChannelRequest;

#[derive(Clone, Copy)]
enum BackendProtocol {
    Slack,
    Discord,
}
impl BackendProtocol {
    fn protocol(self) -> u8 {
        match self {
            Self::Slack => 2,
            Self::Discord => 3,
        }
    }
    fn request_table(self) -> &'static str {
        match self {
            Self::Slack => "gateway_slack_backend_requests",
            Self::Discord => "gateway_discord_backend_requests",
        }
    }
    fn session(self, binding: &str) -> String {
        match self {
            Self::Slack => format!("slack:{binding}"),
            Self::Discord => format!("discord:{binding}"),
        }
    }
    fn event_valid(self, event: &str) -> bool {
        match self {
            Self::Slack => slack_event_id(event),
            Self::Discord => discord_id(event),
        }
    }
    fn capacity(self) -> usize {
        match self {
            Self::Slack => super::channel_store::MAX_SLACK_OPERATIONS,
            Self::Discord => super::channel_store::MAX_DISCORD_OPERATIONS,
        }
    }
}
impl PrivateChannelRequest {
    fn validate(&self, channel: BackendProtocol) -> Result<(), AppError> {
        let request_id = canonical_id(&self.request_id);
        if self.protocol != channel.protocol()
            || !request_id.is_some_and(|id| {
                id.get_version_num() == 7 && id.get_variant() == uuid::Variant::RFC4122
            })
            || canonical_id(&self.binding_id).is_none()
            || self.session_id != channel.session(&self.binding_id)
            || !channel.event_valid(&self.event_id)
            || self.prompt.trim().is_empty()
            || self.prompt.len() > MAX_PROMPT_BYTES
        {
            return Err(AppError::BadRequest(
                "invalid channel protocol, request, binding, session, event or prompt".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub(super) struct PrivateChannelReceipt {
    protocol: u8,
    backend_id: String,
    binding_id: String,
    request_id: String,
    event_id: String,
    session_id: String,
    status: String,
}
pub(super) type SlackReceipt = PrivateChannelReceipt;
pub(super) type DiscordReceipt = PrivateChannelReceipt;

impl super::store::SessionStore {
    fn slack_binding(&self) -> Result<Option<SlackBinding>> {
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("Slack backend requires SQLite")
        };
        let present: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='gateway_slack_backend_binding')", [], |row| row.get(0))?;
        if !present {
            return Ok(None);
        }
        let raw: Option<String> = conn.query_row("SELECT CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity ELSE NULL END FROM gateway_slack_backend_binding WHERE id=1", [], |row| row.get(0)).optional()?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }

    fn bind_slack_backend(&mut self, binding: &SlackBinding) -> Result<bool> {
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("Slack backend requires SQLite")
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SLACK_BINDING_SCHEMA)?;
        let existing: Option<String> = tx.query_row("SELECT CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity ELSE NULL END FROM gateway_slack_backend_binding WHERE id=1", [], |row| row.get(0)).optional()?;
        if let Some(existing) = existing {
            let existing: SlackBinding = serde_json::from_str(&existing)?;
            let matches = existing == *binding;
            tx.commit()?;
            return Ok(matches);
        }
        let records: usize = tx.query_row(
            "SELECT count(*) FROM gateway_slack_backend_requests",
            [],
            |row| row.get(0),
        )?;
        ensure!(
            records == 0,
            "Slack backend ledger exists without its permanent owner"
        );
        tx.execute(
            "INSERT INTO gateway_slack_backend_binding(id,identity) VALUES(1,?1)",
            [serde_json::to_string(binding)?],
        )?;
        tx.commit()?;
        Ok(true)
    }

    #[cfg(test)]
    fn admit_slack_backend_request(
        &mut self,
        request: &SlackRequest,
        backend: &str,
    ) -> Result<bool> {
        self.admit_private_backend_request(request, backend, BackendProtocol::Slack)
    }

    fn private_backend_binding(
        &self,
        channel: BackendProtocol,
        backend: &str,
    ) -> Result<Option<String>> {
        match channel {
            BackendProtocol::Slack => self
                .slack_binding()?
                .map(|binding| {
                    binding.validate(backend)?;
                    Ok(binding.binding_id)
                })
                .transpose(),
            BackendProtocol::Discord => self
                .discord_binding()?
                .map(|binding| {
                    binding.validate(backend)?;
                    Ok(binding.binding_id)
                })
                .transpose(),
        }
    }

    fn admit_private_backend_request(
        &mut self,
        request: &PrivateChannelRequest,
        backend: &str,
        channel: BackendProtocol,
    ) -> Result<bool> {
        request
            .validate(channel)
            .map_err(|_| anyhow::anyhow!("invalid private channel request"))?;
        let binding = self
            .private_backend_binding(channel, backend)?
            .ok_or_else(|| anyhow::anyhow!("channel backend has no permanent binding"))?;
        ensure!(
            binding == request.binding_id,
            "channel request differs from the permanent binding"
        );
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("channel backend requires SQLite")
        };
        let table = channel.request_table();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let duplicate: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE request_id=?1 OR event_id=?2)"),
            params![request.request_id, request.event_id],
            |row| row.get(0),
        )?;
        if duplicate {
            return Ok(false);
        }
        let count: usize = tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        ensure!(count < channel.capacity(), "channel backend request ledger is full; new admission requires administrator maintenance");
        let digest = Sha256::digest(request.prompt.as_bytes());
        tx.execute(&format!("INSERT INTO {table}(request_id,binding_id,event_id,session_id,prompt_sha256,status) VALUES(?1,?2,?3,?4,?5,'admitted')"), params![request.request_id,request.binding_id,request.event_id,request.session_id,&digest[..]])?;
        tx.commit()?;
        Ok(true)
    }

    fn complete_private_backend_request(
        &mut self,
        request: &PrivateChannelRequest,
        session_id: &str,
        messages: &[ChatMessage],
        channel: BackendProtocol,
    ) -> Result<()> {
        ensure!(
            session_id == request.session_id && request.protocol == channel.protocol(),
            "channel session commit identity mismatch"
        );
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("channel backend requires SQLite")
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let updated = tx.execute(&format!("UPDATE {} SET status='completed' WHERE request_id=?1 AND binding_id=?2 AND event_id=?3 AND session_id=?4 AND status='admitted'", channel.request_table()), params![request.request_id,request.binding_id,request.event_id,session_id])?;
        ensure!(
            updated == 1,
            "channel admitted request is missing or already settled"
        );
        tx.execute("INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET messages=excluded.messages,accessed_ms=excluded.accessed_ms", params![session_id,serde_json::to_string(messages)?,super::scheduler::now_ms()])?;
        tx.commit()?;
        Ok(())
    }

    fn slack_backend_receipt(
        &self,
        request_id: &str,
        backend: &str,
    ) -> Result<Option<SlackReceipt>> {
        self.private_backend_receipt(request_id, backend, BackendProtocol::Slack)
    }

    fn private_backend_receipt(
        &self,
        request_id: &str,
        backend: &str,
        channel: BackendProtocol,
    ) -> Result<Option<PrivateChannelReceipt>> {
        let Some(binding) = self.private_backend_binding(channel, backend)? else {
            return Ok(None);
        };
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("channel backend requires SQLite")
        };
        let receipt = conn.query_row(&format!("SELECT CASE WHEN length(CAST(binding_id AS BLOB))=36 THEN binding_id ELSE NULL END,CASE WHEN length(CAST(event_id AS BLOB)) BETWEEN 1 AND 128 THEN event_id ELSE NULL END,CASE WHEN length(CAST(session_id AS BLOB)) BETWEEN 42 AND 44 THEN session_id ELSE NULL END,CASE WHEN status IN ('admitted','completed') THEN status ELSE NULL END FROM {} WHERE request_id=?1", channel.request_table()), [request_id], |row| Ok(PrivateChannelReceipt {
            protocol: channel.protocol(), backend_id: backend.into(), request_id: request_id.into(),
            binding_id: row.get(0)?, event_id: row.get(1)?, session_id: row.get(2)?, status: row.get(3)?,
        })).optional()?;
        if let Some(receipt) = &receipt {
            ensure!(
                receipt.binding_id == binding
                    && receipt.session_id == channel.session(&binding)
                    && channel.event_valid(&receipt.event_id)
                    && matches!(receipt.status.as_str(), "admitted" | "completed"),
                "stored channel request identity is corrupt"
            );
        }
        Ok(receipt)
    }
}

async fn json_bytes(
    state: &AppState,
    headers: &HeaderMap,
    request: Request,
    max_bytes: usize,
) -> Result<Vec<u8>, AppError> {
    if request.uri().query().is_some() {
        return Err(AppError::BadRequest(
            "query parameters are not supported".into(),
        ));
    }
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next());
    if !mime.is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json")) {
        return Err(AppError::BadRequest("JSON content type required".into()));
    }
    let limit = super::max_body_bytes_usize(state.agent.config().http.effective_max_body_bytes())
        .min(max_bytes);
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), limit),
    )
    .await
    .map_err(|_| AppError::ChannelUnavailable)?
    .map_err(|_| AppError::BadRequest("channel body exceeds limit or is unreadable".into()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid channel JSON".into()))?;
    if !value.is_object() {
        return Err(AppError::BadRequest(
            "channel request must be a JSON object".into(),
        ));
    }
    Ok(bytes.to_vec())
}

pub(super) async fn slack_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<Value>, AppError> {
    let Json(mut value) = status(State(state), headers, uri).await?;
    value["protocol"] = json!(2);
    Ok(Json(value))
}

pub(super) async fn slack_bind(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Json<SlackBinding>, AppError> {
    authorize(&state, &headers)?;
    let permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    let bytes = json_bytes(&state, &headers, request, 4096).await?;
    let binding: SlackBinding = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid Slack binding JSON or fields".into()))?;
    binding.validate(&state.agent.config().name).map_err(|_| {
        AppError::BadRequest("invalid Slack backend or installation binding".into())
    })?;
    let owner = binding.clone();
    let bound = with_sessions(&state, move |store| {
        let _permit = permit;
        store.bind_slack_backend(&owner)
    })
    .await?;
    if !bound {
        return Err(AppError::JobConflict(
            "Slack backend is permanently owned by a different binding".into(),
        ));
    }
    Ok(Json(binding))
}

pub(super) async fn slack_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Json<ChannelResponse>, AppError> {
    private_chat(State(state), headers, request, BackendProtocol::Slack).await
}

pub(super) async fn discord_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Json<ChannelResponse>, AppError> {
    private_chat(State(state), headers, request, BackendProtocol::Discord).await
}

async fn private_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    channel: BackendProtocol,
) -> Result<Json<ChannelResponse>, AppError> {
    authorize(&state, &headers)?;
    let control = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    let bytes = json_bytes(&state, &headers, request, MAX_BODY_BYTES).await?;
    let request: PrivateChannelRequest = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid channel execution JSON or fields".into()))?;
    request.validate(channel)?;
    validate_tools(&state.agent).map_err(|_| AppError::ChannelUnavailable)?;
    let permit = state
        .gateway_channel_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::JobConflict("gateway channel chat already executing or stopping".into())
        })?;
    drop(control);
    let plain = ChannelRequest {
        request_id: request.request_id.clone(),
        binding_id: request.binding_id.clone(),
        prompt: request.prompt.clone(),
    };
    let session_id = request.session_id.clone();
    tokio::spawn(execute_channel(
        state,
        plain,
        session_id,
        permit,
        Some((channel, request)),
    ))
    .await
    .map_err(|_| AppError::ChannelUnavailable)?
}

pub(super) async fn slack_receipt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    uri: Uri,
) -> Result<Json<SlackReceipt>, AppError> {
    authorize(&state, &headers)?;
    if uri.query().is_some()
        || !canonical_id(&request_id).is_some_and(|id| {
            id.get_version_num() == 7 && id.get_variant() == uuid::Variant::RFC4122
        })
    {
        return Err(AppError::BadRequest(
            "canonical UUIDv7 request ID without query required".into(),
        ));
    }
    let permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    let backend = state.agent.config().name.clone();
    let receipt = with_sessions(&state, move |store| {
        let _permit = permit;
        store.slack_backend_receipt(&request_id, &backend)
    })
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(receipt))
}

// Discord owns an independent ledger in the same tenant session database.
// Binding identities are permanent, including the public verification key and
// nonsecret state-key fingerprint. No empty/fresh owner can adopt old requests.
const DISCORD_BINDING_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS gateway_discord_backend_binding (
 id INTEGER PRIMARY KEY CHECK(id=1),
 identity TEXT NOT NULL CHECK(json_valid(identity))
);
CREATE TRIGGER IF NOT EXISTS gateway_discord_backend_binding_immutable_insert BEFORE INSERT ON gateway_discord_backend_binding
 WHEN EXISTS(SELECT 1 FROM gateway_discord_backend_binding)
 BEGIN SELECT RAISE(ABORT,'Discord backend owner is immutable'); END;
CREATE TRIGGER IF NOT EXISTS gateway_discord_backend_binding_immutable_update BEFORE UPDATE ON gateway_discord_backend_binding
 BEGIN SELECT RAISE(ABORT,'Discord backend owner is immutable'); END;
CREATE TRIGGER IF NOT EXISTS gateway_discord_backend_binding_immutable_delete BEFORE DELETE ON gateway_discord_backend_binding
 BEGIN SELECT RAISE(ABORT,'Discord backend owner is immutable'); END;
CREATE TABLE IF NOT EXISTS gateway_discord_backend_requests (
 request_id TEXT PRIMARY KEY NOT NULL,
 binding_id TEXT NOT NULL,
 event_id TEXT NOT NULL UNIQUE,
 session_id TEXT NOT NULL,
 prompt_sha256 BLOB NOT NULL CHECK(length(prompt_sha256)=32),
 status TEXT NOT NULL CHECK(status IN ('admitted','completed'))
);
CREATE TRIGGER IF NOT EXISTS gateway_discord_backend_requests_immutable_insert BEFORE INSERT ON gateway_discord_backend_requests
 WHEN EXISTS(SELECT 1 FROM gateway_discord_backend_requests WHERE request_id=NEW.request_id OR event_id=NEW.event_id)
 BEGIN SELECT RAISE(ABORT,'Discord request identity is already admitted'); END;
CREATE TRIGGER IF NOT EXISTS gateway_discord_backend_requests_immutable_update BEFORE UPDATE ON gateway_discord_backend_requests
 WHEN NEW.request_id<>OLD.request_id OR NEW.binding_id<>OLD.binding_id OR NEW.event_id<>OLD.event_id OR NEW.session_id<>OLD.session_id OR NEW.prompt_sha256<>OLD.prompt_sha256 OR OLD.status='completed'
 BEGIN SELECT RAISE(ABORT,'Discord request identity and completed receipt are immutable'); END;
CREATE TRIGGER IF NOT EXISTS gateway_discord_backend_requests_immutable_delete BEFORE DELETE ON gateway_discord_backend_requests
 BEGIN SELECT RAISE(ABORT,'Discord request identity is permanent'); END;";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscordBinding {
    protocol: u8,
    binding_id: String,
    user_id: String,
    backend_id: String,
    application_id: String,
    verify_key: String,
    bot_user_id: String,
    sender_id: String,
    conversation_id: String,
    command_id: String,
    state_key_fingerprint: String,
}

fn discord_id(value: &str) -> bool {
    value
        .parse::<u64>()
        .ok()
        .is_some_and(|id| id != 0 && id.to_string() == value)
}
fn lowercase_nonzero_hex_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && value.bytes().any(|b| b != b'0')
}
impl DiscordBinding {
    fn validate(&self, backend: &str) -> Result<()> {
        ensure!(
            self.protocol == 3 && self.backend_id == backend,
            "Discord backend protocol or identity mismatch"
        );
        ensure!(
            canonical_id(&self.binding_id).is_some() && canonical_id(&self.user_id).is_some(),
            "canonical Discord owner UUIDs required"
        );
        ensure!(
            [
                &self.application_id,
                &self.bot_user_id,
                &self.sender_id,
                &self.conversation_id,
                &self.command_id
            ]
            .iter()
            .all(|id| discord_id(id)),
            "canonical nonzero Discord snowflake identities required"
        );
        ensure!(
            self.bot_user_id != self.sender_id,
            "Discord sender cannot be the bot user"
        );
        ensure!(
            lowercase_nonzero_hex_key(&self.verify_key)
                && lowercase_nonzero_hex_key(&self.state_key_fingerprint),
            "Discord verification key and state fingerprint require nonzero lowercase 32-byte hex"
        );
        Ok(())
    }
}

impl super::store::SessionStore {
    fn discord_binding(&self) -> Result<Option<DiscordBinding>> {
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("Discord backend requires SQLite")
        };
        let present: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='gateway_discord_backend_binding')", [], |row| row.get(0))?;
        if !present {
            return Ok(None);
        }
        let raw: Option<String> = conn.query_row("SELECT CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity ELSE NULL END FROM gateway_discord_backend_binding WHERE id=1", [], |row| row.get(0)).optional()?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }
    fn bind_discord_backend(&mut self, binding: &DiscordBinding) -> Result<bool> {
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("Discord backend requires SQLite")
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(DISCORD_BINDING_SCHEMA)?;
        let existing: Option<String> = tx.query_row("SELECT CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity ELSE NULL END FROM gateway_discord_backend_binding WHERE id=1", [], |row| row.get(0)).optional()?;
        if let Some(existing) = existing {
            let existing: DiscordBinding = serde_json::from_str(&existing)?;
            let matches = existing == *binding;
            tx.commit()?;
            return Ok(matches);
        }
        let records: usize = tx.query_row(
            "SELECT count(*) FROM gateway_discord_backend_requests",
            [],
            |row| row.get(0),
        )?;
        ensure!(
            records == 0,
            "Discord backend ledger exists without its permanent owner"
        );
        tx.execute(
            "INSERT INTO gateway_discord_backend_binding(id,identity) VALUES(1,?1)",
            [serde_json::to_string(binding)?],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

pub(super) async fn discord_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<Value>, AppError> {
    let Json(mut value) = status(State(state), headers, uri).await?;
    value["protocol"] = json!(3);
    Ok(Json(value))
}

pub(super) async fn discord_bind(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Json<DiscordBinding>, AppError> {
    authorize(&state, &headers)?;
    let permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    let bytes = json_bytes(&state, &headers, request, 4096).await?;
    let binding: DiscordBinding = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid Discord binding JSON or fields".into()))?;
    binding.validate(&state.agent.config().name).map_err(|_| {
        AppError::BadRequest("invalid Discord backend or application binding".into())
    })?;
    let owner = binding.clone();
    let bound = tokio::time::timeout(
        COMMIT_TIMEOUT,
        with_sessions(&state, move |store| {
            let _permit = permit;
            store.bind_discord_backend(&owner)
        }),
    )
    .await
    .map_err(|_| AppError::ChannelUnavailable)??;
    if !bound {
        return Err(AppError::JobConflict(
            "Discord backend is permanently owned by a different binding".into(),
        ));
    }
    Ok(Json(binding))
}

pub(super) async fn discord_receipt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    uri: Uri,
) -> Result<Json<DiscordReceipt>, AppError> {
    authorize(&state, &headers)?;
    if uri.query().is_some()
        || !canonical_id(&request_id).is_some_and(|id| {
            id.get_version_num() == 7 && id.get_variant() == uuid::Variant::RFC4122
        })
    {
        return Err(AppError::BadRequest(
            "canonical UUIDv7 request ID without query required".into(),
        ));
    }
    let permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ChannelUnavailable)?;
    let backend = state.agent.config().name.clone();
    let receipt = tokio::time::timeout(
        COMMIT_TIMEOUT,
        with_sessions(&state, move |store| {
            let _permit = permit;
            store.private_backend_receipt(&request_id, &backend, BackendProtocol::Discord)
        }),
    )
    .await
    .map_err(|_| AppError::ChannelUnavailable)??
    .ok_or(AppError::NotFound)?;
    Ok(Json(receipt))
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
    fn slack_owner(binding: Uuid) -> Value {
        json!({"protocol":2,"binding_id":binding.to_string(),"user_id":Uuid::new_v4().to_string(),"backend_id":"tenant-alice","team_id":"T0123","app_id":"A0123","bot_user_id":"U0123","bot_id":"B0123","sender_id":"U0456","conversation_id":"D0123"})
    }
    fn slack_body(binding: Uuid) -> Value {
        json!({"protocol":2,"binding_id":binding.to_string(),"request_id":Uuid::now_v7().to_string(),"session_id":format!("slack:{binding}"),"event_id":"EvFirst123","prompt":"hello private Slack"})
    }
    async fn bind_slack(state: AppState, value: Value) -> Result<Json<SlackBinding>, AppError> {
        slack_bind(
            State(state),
            headers(),
            Request::new(Body::from(value.to_string())),
        )
        .await
    }
    async fn post_slack(state: AppState, value: Value) -> Result<Json<ChannelResponse>, AppError> {
        slack_chat(
            State(state),
            headers(),
            Request::new(Body::from(value.to_string())),
        )
        .await
    }
    async fn receipt_slack(
        state: AppState,
        request_id: &str,
    ) -> Result<Json<SlackReceipt>, AppError> {
        slack_receipt(
            State(state),
            headers(),
            Path(request_id.into()),
            Uri::from_static("/internal/channels/slack/requests/placeholder"),
        )
        .await
    }

    #[tokio::test]
    async fn slack_protocol_defaults_closed_and_rejects_ambiguous_authentication() {
        let fixture = Fixture::new().await;
        let mut closed = fixture.state.clone();
        let mut config = closed.agent.config().clone();
        config.http.gateway_channel_chat = false;
        closed.agent = Arc::new(JiaClawAgent::new(config).unwrap());
        let router = crate::build_router(closed);
        for (method, path) in [
            (Method::GET, "/internal/channels/slack/status"),
            (Method::POST, "/internal/channels/slack-binding"),
            (Method::POST, "/internal/channels/slack/execute"),
            (
                Method::GET,
                "/internal/channels/slack/requests/0195ca8e-0000-7000-8000-00000000000a",
            ),
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
        assert_eq!(
            slack_status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/channels/slack/status")
            )
            .await
            .unwrap()
            .0,
            json!({"protocol":2,"backend_id":"tenant-alice","max_run_seconds":120,"tools":TOOLS,"mode":"gateway"})
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
        ] {
            assert!(matches!(
                slack_status(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Uri::from_static("/internal/channels/slack/status")
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                slack_bind(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Request::new(Body::from("{}"))
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                slack_chat(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Request::new(Body::from("{}"))
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                slack_receipt(
                    State(fixture.state.clone()),
                    bad,
                    Path(Uuid::now_v7().to_string()),
                    Uri::from_static("/internal/channels/slack/requests/placeholder")
                )
                .await,
                Err(AppError::Unauthorized)
            ));
        }
        assert!(fixture.requests.lock().unwrap().is_empty());
        fixture.cleanup();
    }

    #[tokio::test]
    async fn slack_binding_and_execution_require_exact_protocol_identity_and_object_fields() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let owner = slack_owner(binding);
        for (field, value) in [
            ("protocol", json!(1)),
            ("binding_id", json!(Uuid::nil().to_string())),
            ("user_id", json!("0195CA8E-0000-7000-8000-00000000000A")),
            ("backend_id", json!("other-tenant")),
            ("team_id", json!("t0123")),
            ("app_id", json!("T0123")),
            ("bot_user_id", json!("B0123")),
            ("bot_id", json!("U0123")),
            ("sender_id", json!("U0123")),
            ("conversation_id", json!("C0123")),
            ("enabled_tools", json!(["exec"])),
        ] {
            let mut invalid = owner.clone();
            invalid[field] = value;
            assert!(
                matches!(
                    bind_slack(fixture.state.clone(), invalid).await,
                    Err(AppError::BadRequest(_))
                ),
                "{field}"
            );
        }
        for raw in [
            "[]".into(),
            format!(
                "{},\"protocol\":2}}",
                owner.to_string().trim_end_matches('}')
            ),
        ] {
            assert!(matches!(
                slack_bind(
                    State(fixture.state.clone()),
                    headers(),
                    Request::new(Body::from(raw))
                )
                .await,
                Err(AppError::BadRequest(_))
            ));
        }
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .slack_binding()
            .unwrap()
            .is_none());
        let _ = bind_slack(fixture.state.clone(), owner).await.unwrap();
        let valid = slack_body(binding);
        for (field, value) in [
            ("protocol", json!(1)),
            ("binding_id", json!("not-a-uuid")),
            ("request_id", json!(Uuid::new_v4().to_string())),
            ("session_id", json!("tg-forged")),
            ("event_id", json!("Evbad/slash")),
            ("event_id", json!("Ev")),
            ("event_id", json!(format!("Ev{}", "a".repeat(127)))),
            ("prompt", json!(" ")),
            ("prompt", json!("界".repeat(MAX_PROMPT_BYTES / 3 + 1))),
            ("enabled_tools", json!(["exec"])),
            ("user_id", json!(Uuid::new_v4().to_string())),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                matches!(
                    post_slack(fixture.state.clone(), invalid).await,
                    Err(AppError::BadRequest(_))
                ),
                "{field}"
            );
        }
        assert!(matches!(
            slack_bind(
                State(fixture.state.clone()),
                headers(),
                Request::builder()
                    .uri("/internal/channels/slack-binding?x=1")
                    .body(Body::from("{}"))
                    .unwrap()
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            slack_chat(
                State(fixture.state.clone()),
                headers(),
                Request::builder()
                    .uri("/internal/channels/slack/execute?x=1")
                    .body(Body::from(valid.to_string()))
                    .unwrap()
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        let duplicate = format!(
            "{},\"protocol\":2}}",
            valid.to_string().trim_end_matches('}')
        );
        for raw in ["[]".into(), duplicate] {
            assert!(matches!(
                slack_chat(
                    State(fixture.state.clone()),
                    headers(),
                    Request::new(Body::from(raw))
                )
                .await,
                Err(AppError::BadRequest(_))
            ));
        }
        assert!(matches!(
            slack_bind(
                State(fixture.state.clone()),
                headers(),
                Request::new(Body::from(" ".repeat(4097)))
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        let mut wrong_owner = valid.clone();
        let wrong_id = Uuid::new_v4();
        wrong_owner["binding_id"] = json!(wrong_id.to_string());
        wrong_owner["session_id"] = json!(format!("slack:{wrong_id}"));
        assert!(post_slack(fixture.state.clone(), wrong_owner)
            .await
            .is_err());
        assert!(fixture.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 1);
        fixture.cleanup();
    }

    #[tokio::test]
    async fn slack_owner_is_permanent_across_restart_and_telegram_protocol_is_unchanged() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let owner = slack_owner(binding);
        assert_eq!(
            serde_json::to_value(
                bind_slack(fixture.state.clone(), owner.clone())
                    .await
                    .unwrap()
                    .0
            )
            .unwrap(),
            owner
        );
        let _ = bind_slack(fixture.state.clone(), owner.clone())
            .await
            .unwrap();
        for field in [
            "binding_id",
            "user_id",
            "team_id",
            "app_id",
            "bot_user_id",
            "bot_id",
            "sender_id",
            "conversation_id",
        ] {
            let mut altered = owner.clone();
            altered[field] = match field {
                "binding_id" | "user_id" => json!(Uuid::new_v4().to_string()),
                _ => json!(format!("{}99", owner[field].as_str().unwrap())),
            };
            assert!(
                matches!(
                    bind_slack(fixture.state.clone(), altered).await,
                    Err(AppError::JobConflict(_))
                ),
                "{field}"
            );
        }
        let telegram = post_body(fixture.state.clone(), body(binding))
            .await
            .unwrap()
            .0;
        assert_eq!(telegram.protocol, 1);
        assert_eq!(
            telegram.response.session_id.as_deref(),
            Some(format!("tg-{}", binding.simple()).as_str())
        );
        assert_eq!(
            status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/gateway/channel/status")
            )
            .await
            .unwrap()
            .0["protocol"],
            1
        );
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let mut reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.slack_binding().unwrap().unwrap()).unwrap(),
            owner
        );
        let mut altered: SlackBinding = serde_json::from_value(owner.clone()).unwrap();
        altered.user_id = Uuid::new_v4().to_string();
        assert!(!reopened.bind_slack_backend(&altered).unwrap());
        assert!(reopened
            .get(&format!("tg-{}", binding.simple()))
            .unwrap()
            .is_some());
        let SessionStore::Sqlite { conn, .. } = &reopened else {
            panic!("SQLite")
        };
        assert!(conn
            .execute("DELETE FROM gateway_slack_backend_binding", [])
            .is_err());
        assert!(conn
            .execute(
                "INSERT OR REPLACE INTO gateway_slack_backend_binding(id,identity) VALUES(1,?1)",
                [serde_json::to_string(&altered).unwrap()]
            )
            .is_err());
        assert_eq!(
            serde_json::to_value(reopened.slack_binding().unwrap().unwrap()).unwrap(),
            owner
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn slack_request_receipt_is_atomic_private_and_replays_never_reach_model() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let _ = bind_slack(fixture.state.clone(), slack_owner(binding))
            .await
            .unwrap();
        let request = slack_body(binding);
        let reply = post_slack(fixture.state.clone(), request.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(reply.protocol, 2);
        assert_eq!(reply.response.session_id, Some(format!("slack:{binding}")));
        assert_eq!(
            reply.response.routing.as_ref().unwrap().purpose,
            ModelPurpose::Channel
        );
        let receipt = serde_json::to_value(
            receipt_slack(
                fixture.state.clone(),
                request["request_id"].as_str().unwrap(),
            )
            .await
            .unwrap()
            .0,
        )
        .unwrap();
        assert_eq!(
            receipt,
            json!({"protocol":2,"backend_id":"tenant-alice","binding_id":binding.to_string(),"request_id":request["request_id"],"event_id":"EvFirst123","session_id":format!("slack:{binding}"),"status":"completed"})
        );
        assert!(!receipt.to_string().contains("hello private Slack"));
        let mut duplicate_event = request.clone();
        duplicate_event["request_id"] = json!(Uuid::now_v7().to_string());
        let mut duplicate_request = request.clone();
        duplicate_request["event_id"] = json!("EvSecond123");
        for duplicate in [request.clone(), duplicate_event, duplicate_request] {
            assert!(matches!(
                post_slack(fixture.state.clone(), duplicate).await,
                Err(AppError::JobConflict(_))
            ));
        }
        {
            let requests = fixture.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["model"], "exact-channel-route");
            let mut tools = requests[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            tools.sort_unstable();
            assert_eq!(tools, TOOLS);
        }
        assert!(matches!(
            receipt_slack(fixture.state.clone(), &Uuid::new_v4().to_string()).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            receipt_slack(fixture.state.clone(), &Uuid::now_v7().to_string()).await,
            Err(AppError::NotFound)
        ));
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let mut reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        let parsed: SlackRequest = serde_json::from_value(request).unwrap();
        assert_eq!(
            reopened
                .slack_backend_receipt(&parsed.request_id, "tenant-alice")
                .unwrap()
                .unwrap()
                .status,
            "completed"
        );
        assert!(!reopened
            .admit_slack_backend_request(&parsed, "tenant-alice")
            .unwrap());
        assert_eq!(
            reopened
                .get(&parsed.session_id)
                .unwrap()
                .unwrap()
                .messages
                .len(),
            2
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn slack_commit_failure_preserves_unknown_admission_and_rolls_back_receipt() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let _ = bind_slack(fixture.state.clone(), slack_owner(binding))
            .await
            .unwrap();
        {
            let store = fixture.state.sessions.lock().unwrap();
            let SessionStore::Sqlite { conn, .. } = &*store else {
                panic!("SQLite")
            };
            conn.execute_batch("CREATE TRIGGER reject_slack_commit BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'private fixture commit failure'); END;").unwrap();
        }
        let request = slack_body(binding);
        assert!(
            matches!(post_slack(fixture.state.clone(),request.clone()).await,Err(AppError::Internal(message)) if message=="session_storage_error")
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert_eq!(
            receipt_slack(
                fixture.state.clone(),
                request["request_id"].as_str().unwrap()
            )
            .await
            .unwrap()
            .0
            .status,
            "admitted"
        );
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .get(&format!("slack:{binding}"))
            .unwrap()
            .is_none());
        assert!(matches!(
            post_slack(fixture.state.clone(), request.clone()).await,
            Err(AppError::JobConflict(_))
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let mut reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        let parsed: SlackRequest = serde_json::from_value(request).unwrap();
        assert_eq!(
            reopened
                .slack_backend_receipt(&parsed.request_id, "tenant-alice")
                .unwrap()
                .unwrap()
                .status,
            "admitted"
        );
        assert!(!reopened
            .admit_slack_backend_request(&parsed, "tenant-alice")
            .unwrap());
        assert!(reopened.get(&parsed.session_id).unwrap().is_none());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn slack_disconnected_caller_keeps_slot_and_commits_one_durable_receipt() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let _ = bind_slack(fixture.state.clone(), slack_owner(binding))
            .await
            .unwrap();
        let request = slack_body(binding);
        let request_id = request["request_id"].as_str().unwrap().to_string();
        let guard = session_turn_lock(&fixture.state, &format!("slack:{binding}")).await;
        let state = fixture.state.clone();
        let caller = tokio::spawn(async move { post_slack(state, request).await });
        wait_busy(&fixture.state).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 0);
        assert!(slack_status(
            State(fixture.state.clone()),
            headers(),
            Uri::from_static("/internal/channels/slack/status")
        )
        .await
        .is_ok());
        assert!(matches!(
            post_slack(fixture.state.clone(), slack_body(binding)).await,
            Err(AppError::JobConflict(_))
        ));
        drop(guard);
        wait_idle(&fixture.state).await;
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert_eq!(
            receipt_slack(fixture.state.clone(), &request_id)
                .await
                .unwrap()
                .0
                .status,
            "completed"
        );
        fixture.cleanup();
    }

    #[test]
    fn slack_backend_quota_and_duplicate_admission_never_create_partial_records() {
        let mut store = SessionStore::open(std::path::Path::new(":memory:")).unwrap();
        let binding = Uuid::new_v4();
        let owner: SlackBinding = serde_json::from_value(slack_owner(binding)).unwrap();
        store.bind_slack_backend(&owner).unwrap();
        let request: SlackRequest = serde_json::from_value(slack_body(binding)).unwrap();
        assert!(store
            .admit_slack_backend_request(&request, "tenant-alice")
            .unwrap());
        assert!(!store
            .admit_slack_backend_request(&request, "tenant-alice")
            .unwrap());
        let SessionStore::Sqlite { conn, .. } = &store else {
            panic!("SQLite")
        };
        assert!(conn.execute("INSERT OR REPLACE INTO gateway_slack_backend_requests(request_id,binding_id,event_id,session_id,prompt_sha256,status) VALUES(?1,?2,'EvReplacement',?3,zeroblob(32),'completed')",params![request.request_id,binding.to_string(),request.session_id]).is_err());
        assert!(conn.execute("INSERT OR REPLACE INTO gateway_slack_backend_requests(request_id,binding_id,event_id,session_id,prompt_sha256,status) VALUES(?1,?2,?3,?4,zeroblob(32),'completed')",params![Uuid::now_v7().to_string(),binding.to_string(),request.event_id,request.session_id]).is_err());
        conn.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<15999) INSERT INTO gateway_slack_backend_requests(request_id,binding_id,event_id,session_id,prompt_sha256,status) SELECT '0195ca8e-0000-7000-8000-'||printf('%012x',x),?1,'EvOld'||x,?2,zeroblob(32),'admitted' FROM n",params![binding.to_string(),format!("slack:{binding}")]).unwrap();
        let mut next = request.clone();
        next.request_id = Uuid::now_v7().to_string();
        next.event_id = "EvNext123".into();
        assert!(store
            .admit_slack_backend_request(&next, "tenant-alice")
            .is_err());
        assert!(store
            .slack_backend_receipt(&next.request_id, "tenant-alice")
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .slack_backend_receipt(&request.request_id, "tenant-alice")
                .unwrap()
                .unwrap()
                .status,
            "admitted"
        );
    }

    fn discord_owner(binding: Uuid) -> Value {
        json!({"protocol":3,"binding_id":binding.to_string(),"user_id":Uuid::new_v4().to_string(),"backend_id":"tenant-alice","application_id":"111111111111111111","verify_key":"a".repeat(64),"bot_user_id":"111111111111111111","sender_id":"222222222222222222","conversation_id":"333333333333333333","command_id":"444444444444444444","state_key_fingerprint":"b".repeat(64)})
    }
    fn discord_body(binding: Uuid) -> Value {
        json!({"protocol":3,"binding_id":binding.to_string(),"request_id":Uuid::now_v7().to_string(),"session_id":format!("discord:{binding}"),"event_id":"555555555555555555","prompt":"hello private Discord"})
    }
    async fn bind_discord(state: AppState, value: Value) -> Result<Json<DiscordBinding>, AppError> {
        discord_bind(
            State(state),
            headers(),
            Request::new(Body::from(value.to_string())),
        )
        .await
    }
    async fn post_discord(
        state: AppState,
        value: Value,
    ) -> Result<Json<ChannelResponse>, AppError> {
        discord_chat(
            State(state),
            headers(),
            Request::new(Body::from(value.to_string())),
        )
        .await
    }
    async fn receipt_discord(
        state: AppState,
        request_id: &str,
    ) -> Result<Json<DiscordReceipt>, AppError> {
        discord_receipt(
            State(state),
            headers(),
            Path(request_id.into()),
            Uri::from_static("/internal/channels/discord/requests/placeholder"),
        )
        .await
    }

    #[tokio::test]
    async fn discord_default_routes_are_closed_and_every_private_route_authenticates_first() {
        let fixture = Fixture::new().await;
        let mut closed = fixture.state.clone();
        let mut config = closed.agent.config().clone();
        config.http.gateway_channel_chat = false;
        closed.agent = Arc::new(JiaClawAgent::new(config).unwrap());
        let router = crate::build_router(closed);
        for (method, path) in [
            (Method::GET, "/internal/channels/discord/status"),
            (Method::POST, "/internal/channels/discord-binding"),
            (Method::POST, "/internal/channels/discord/execute"),
            (
                Method::GET,
                "/internal/channels/discord/requests/0195ca8e-0000-7000-8000-00000000000a",
            ),
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(path)
                            .body(Body::empty())
                            .unwrap()
                    )
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        drop(router);
        assert_eq!(
            discord_status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/channels/discord/status")
            )
            .await
            .unwrap()
            .0,
            json!({"protocol":3,"backend_id":"tenant-alice","max_run_seconds":120,"tools":TOOLS,"mode":"gateway"})
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
                discord_status(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Uri::from_static("/internal/channels/discord/status?bad=1")
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                discord_bind(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Request::new(Body::from("not json"))
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                discord_chat(
                    State(fixture.state.clone()),
                    bad.clone(),
                    Request::new(Body::from("not json"))
                )
                .await,
                Err(AppError::Unauthorized)
            ));
            assert!(matches!(
                discord_receipt(
                    State(fixture.state.clone()),
                    bad,
                    Path("not uuid".into()),
                    Uri::from_static("/internal/channels/discord/requests/placeholder?bad=1")
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
            discord_status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/channels/discord/status")
            )
            .await,
            Err(AppError::ChannelUnavailable)
        ));
        assert!(matches!(
            bind_discord(fixture.state.clone(), discord_owner(Uuid::new_v4())).await,
            Err(AppError::ChannelUnavailable)
        ));
        assert!(matches!(
            post_discord(fixture.state.clone(), discord_body(Uuid::new_v4())).await,
            Err(AppError::ChannelUnavailable)
        ));
        assert!(matches!(
            receipt_discord(fixture.state.clone(), &Uuid::now_v7().to_string()).await,
            Err(AppError::ChannelUnavailable)
        ));
        drop(controls);
        assert!(fixture.requests.lock().unwrap().is_empty());
        fixture.cleanup();
    }

    #[tokio::test]
    async fn discord_exact_binding_request_fields_and_canonical_identities_precede_model_io() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let owner = discord_owner(binding);
        for (field, value) in [
            ("protocol", json!(2)),
            ("binding_id", json!(Uuid::nil().to_string())),
            ("user_id", json!("0195CA8E-0000-7000-8000-00000000000A")),
            ("backend_id", json!("other-tenant")),
            ("application_id", json!("0111111111111111111")),
            ("bot_user_id", json!("0")),
            ("sender_id", owner["bot_user_id"].clone()),
            ("conversation_id", json!("+333333333333333333")),
            ("command_id", json!("18446744073709551616")),
            ("verify_key", json!("A".repeat(64))),
            ("verify_key", json!("0".repeat(64))),
            ("state_key_fingerprint", json!("g".repeat(64))),
            ("state_key_fingerprint", json!("0".repeat(64))),
            ("enabled_tools", json!(["exec"])),
        ] {
            let mut invalid = owner.clone();
            invalid[field] = value;
            assert!(
                matches!(
                    bind_discord(fixture.state.clone(), invalid).await,
                    Err(AppError::BadRequest(_))
                ),
                "{field}"
            );
        }
        for raw in [
            "[]".into(),
            format!(
                "{},\"application_id\":\"111111111111111111\"}}",
                owner.to_string().trim_end_matches('}')
            ),
        ] {
            assert!(matches!(
                discord_bind(
                    State(fixture.state.clone()),
                    headers(),
                    Request::new(Body::from(raw))
                )
                .await,
                Err(AppError::BadRequest(_))
            ));
        }
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .discord_binding()
            .unwrap()
            .is_none());
        let _ = bind_discord(fixture.state.clone(), owner).await.unwrap();
        let valid = discord_body(binding);
        for (field, value) in [
            ("protocol", json!(2)),
            ("binding_id", json!("bad")),
            ("request_id", json!(Uuid::new_v4().to_string())),
            ("session_id", json!(format!("slack:{binding}"))),
            ("event_id", json!("0")),
            ("event_id", json!("0555555555555555555")),
            ("event_id", json!("18446744073709551616")),
            ("event_id", json!(" 555555555555555555")),
            ("prompt", json!(" \n ")),
            ("prompt", json!("界".repeat(MAX_PROMPT_BYTES / 3 + 1))),
            ("enabled_tools", json!(["exec"])),
            ("state_key_fingerprint", json!("a".repeat(64))),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                matches!(
                    post_discord(fixture.state.clone(), invalid).await,
                    Err(AppError::BadRequest(_))
                ),
                "{field}"
            );
        }
        for raw in [
            "[]".into(),
            format!(
                "{},\"prompt\":\"again\"}}",
                valid.to_string().trim_end_matches('}')
            ),
        ] {
            assert!(matches!(
                discord_chat(
                    State(fixture.state.clone()),
                    headers(),
                    Request::new(Body::from(raw))
                )
                .await,
                Err(AppError::BadRequest(_))
            ));
        }
        for (route, value) in [
            (
                "/internal/channels/discord-binding?x=1",
                discord_owner(binding),
            ),
            ("/internal/channels/discord/execute?x=1", valid.clone()),
        ] {
            let request = Request::builder()
                .uri(route)
                .body(Body::from(value.to_string()))
                .unwrap();
            let result = if route.contains("binding") {
                discord_bind(State(fixture.state.clone()), headers(), request)
                    .await
                    .map(|_| ())
            } else {
                discord_chat(State(fixture.state.clone()), headers(), request)
                    .await
                    .map(|_| ())
            };
            assert!(matches!(result, Err(AppError::BadRequest(_))));
        }
        assert!(matches!(
            discord_status(
                State(fixture.state.clone()),
                headers(),
                Uri::from_static("/internal/channels/discord/status?x=1")
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            discord_bind(
                State(fixture.state.clone()),
                headers(),
                Request::new(Body::from(" ".repeat(4097)))
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        let mut wrong = valid;
        let different = Uuid::new_v4();
        wrong["binding_id"] = json!(different.to_string());
        wrong["session_id"] = json!(format!("discord:{different}"));
        assert!(post_discord(fixture.state.clone(), wrong).await.is_err());
        assert!(fixture.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 1);
        fixture.cleanup();
    }

    #[tokio::test]
    async fn discord_permanent_owner_coexists_with_slack_and_survives_reopen() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let owner = discord_owner(binding);
        let slack = slack_owner(Uuid::new_v4());
        let _ = bind_slack(fixture.state.clone(), slack.clone())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(
                bind_discord(fixture.state.clone(), owner.clone())
                    .await
                    .unwrap()
                    .0
            )
            .unwrap(),
            owner
        );
        let _ = bind_discord(fixture.state.clone(), owner.clone())
            .await
            .unwrap();
        for field in [
            "binding_id",
            "user_id",
            "application_id",
            "bot_user_id",
            "sender_id",
            "conversation_id",
            "command_id",
            "verify_key",
            "state_key_fingerprint",
        ] {
            let mut altered = owner.clone();
            altered[field] = match field {
                "binding_id" | "user_id" => json!(Uuid::new_v4().to_string()),
                "verify_key" | "state_key_fingerprint" => json!("c".repeat(64)),
                _ => json!(format!("{}1", owner[field].as_str().unwrap())),
            };
            assert!(
                matches!(
                    bind_discord(fixture.state.clone(), altered).await,
                    Err(AppError::JobConflict(_))
                ),
                "{field}"
            );
        }
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let mut reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.discord_binding().unwrap().unwrap()).unwrap(),
            owner
        );
        assert_eq!(
            serde_json::to_value(reopened.slack_binding().unwrap().unwrap()).unwrap(),
            slack
        );
        let mut altered: DiscordBinding = serde_json::from_value(owner.clone()).unwrap();
        altered.state_key_fingerprint = "c".repeat(64);
        assert!(!reopened.bind_discord_backend(&altered).unwrap());
        let SessionStore::Sqlite { conn, .. } = &reopened else {
            panic!("SQLite")
        };
        for statement in [
            "DELETE FROM gateway_discord_backend_binding",
            "UPDATE gateway_discord_backend_binding SET identity='{}'",
        ] {
            assert!(conn.execute(statement, []).is_err());
        }
        assert!(conn
            .execute(
                "INSERT OR REPLACE INTO gateway_discord_backend_binding(id,identity) VALUES(1,?1)",
                [serde_json::to_string(&altered).unwrap()]
            )
            .is_err());
        assert_eq!(
            serde_json::to_value(reopened.discord_binding().unwrap().unwrap()).unwrap(),
            owner
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn discord_request_receipt_is_atomic_private_and_replays_never_reach_model() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let _ = bind_discord(fixture.state.clone(), discord_owner(binding))
            .await
            .unwrap();
        let request = discord_body(binding);
        let reply = post_discord(fixture.state.clone(), request.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(reply.protocol, 3);
        assert_eq!(
            reply.response.session_id,
            Some(format!("discord:{binding}"))
        );
        assert_eq!(
            reply.response.routing.as_ref().unwrap().purpose,
            ModelPurpose::Channel
        );
        let receipt = serde_json::to_value(
            receipt_discord(
                fixture.state.clone(),
                request["request_id"].as_str().unwrap(),
            )
            .await
            .unwrap()
            .0,
        )
        .unwrap();
        assert_eq!(
            receipt,
            json!({"protocol":3,"backend_id":"tenant-alice","binding_id":binding.to_string(),"request_id":request["request_id"],"event_id":"555555555555555555","session_id":format!("discord:{binding}"),"status":"completed"})
        );
        assert!(!receipt.to_string().contains("hello private Discord"));
        assert!(!receipt.to_string().contains("prompt_sha256"));
        let mut duplicate_event = request.clone();
        duplicate_event["request_id"] = json!(Uuid::now_v7().to_string());
        let mut duplicate_request = request.clone();
        duplicate_request["event_id"] = json!("555555555555555556");
        for duplicate in [request.clone(), duplicate_event, duplicate_request] {
            assert!(matches!(
                post_discord(fixture.state.clone(), duplicate).await,
                Err(AppError::JobConflict(_))
            ));
        }
        {
            let model = fixture.requests.lock().unwrap();
            assert_eq!(model.len(), 1);
            assert_eq!(model[0]["model"], "exact-channel-route");
            let mut tools = model[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap())
                .collect::<Vec<_>>();
            tools.sort_unstable();
            assert_eq!(tools, TOOLS);
        }
        assert!(matches!(
            receipt_discord(fixture.state.clone(), &Uuid::new_v4().to_string()).await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            receipt_discord(fixture.state.clone(), &Uuid::now_v7().to_string()).await,
            Err(AppError::NotFound)
        ));
        assert!(matches!(
            discord_receipt(
                State(fixture.state.clone()),
                headers(),
                Path(request["request_id"].as_str().unwrap().into()),
                Uri::from_static("/internal/channels/discord/requests/placeholder?x=1")
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let mut reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        let parsed: PrivateChannelRequest = serde_json::from_value(request).unwrap();
        assert_eq!(
            reopened
                .private_backend_receipt(
                    &parsed.request_id,
                    "tenant-alice",
                    BackendProtocol::Discord
                )
                .unwrap()
                .unwrap()
                .status,
            "completed"
        );
        assert!(!reopened
            .admit_private_backend_request(&parsed, "tenant-alice", BackendProtocol::Discord)
            .unwrap());
        assert_eq!(
            reopened
                .get(&parsed.session_id)
                .unwrap()
                .unwrap()
                .messages
                .len(),
            2
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn discord_commit_failure_preserves_unknown_admission_and_rolls_back_receipt() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let _ = bind_discord(fixture.state.clone(), discord_owner(binding))
            .await
            .unwrap();
        {
            let store = fixture.state.sessions.lock().unwrap();
            let SessionStore::Sqlite { conn, .. } = &*store else {
                panic!("SQLite")
            };
            conn.execute_batch("CREATE TRIGGER reject_discord_commit BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'private fixture commit failure'); END;").unwrap();
        }
        let request = discord_body(binding);
        assert!(
            matches!(post_discord(fixture.state.clone(),request.clone()).await,Err(AppError::Internal(message)) if message=="session_storage_error")
        );
        assert_eq!(
            receipt_discord(
                fixture.state.clone(),
                request["request_id"].as_str().unwrap()
            )
            .await
            .unwrap()
            .0
            .status,
            "admitted"
        );
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .get(&format!("discord:{binding}"))
            .unwrap()
            .is_none());
        assert!(matches!(
            post_discord(fixture.state.clone(), request.clone()).await,
            Err(AppError::JobConflict(_))
        ));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        let root = fixture.root.clone();
        fixture.server.abort();
        drop(fixture.state);
        let mut reopened = SessionStore::open(&root.join("sessions.sqlite3")).unwrap();
        let parsed: PrivateChannelRequest = serde_json::from_value(request).unwrap();
        assert_eq!(
            reopened
                .private_backend_receipt(
                    &parsed.request_id,
                    "tenant-alice",
                    BackendProtocol::Discord
                )
                .unwrap()
                .unwrap()
                .status,
            "admitted"
        );
        assert!(!reopened
            .admit_private_backend_request(&parsed, "tenant-alice", BackendProtocol::Discord)
            .unwrap());
        assert!(reopened.get(&parsed.session_id).unwrap().is_none());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn discord_disconnected_caller_keeps_shared_channel_slot_until_durable_completion() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let _ = bind_discord(fixture.state.clone(), discord_owner(binding))
            .await
            .unwrap();
        let _ = bind_slack(fixture.state.clone(), slack_owner(binding))
            .await
            .unwrap();
        let request = discord_body(binding);
        let request_id = request["request_id"].as_str().unwrap().to_string();
        let guard = session_turn_lock(&fixture.state, &format!("discord:{binding}")).await;
        let state = fixture.state.clone();
        let caller = tokio::spawn(async move { post_discord(state, request).await });
        wait_busy(&fixture.state).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(fixture.state.gateway_channel_permit.available_permits(), 0);
        assert!(matches!(
            post_slack(fixture.state.clone(), slack_body(binding)).await,
            Err(AppError::JobConflict(_))
        ));
        assert!(matches!(
            post_body(fixture.state.clone(), body(binding)).await,
            Err(AppError::JobConflict(_))
        ));
        assert!(discord_status(
            State(fixture.state.clone()),
            headers(),
            Uri::from_static("/internal/channels/discord/status")
        )
        .await
        .is_ok());
        drop(guard);
        wait_idle(&fixture.state).await;
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert_eq!(
            receipt_discord(fixture.state.clone(), &request_id)
                .await
                .unwrap()
                .0
                .status,
            "completed"
        );
        fixture.cleanup();
    }

    #[test]
    fn discord_backend_quota_and_permanent_admissions_reject_replace_delete_and_partial_records() {
        let mut store = SessionStore::open(std::path::Path::new(":memory:")).unwrap();
        let binding = Uuid::new_v4();
        let owner: DiscordBinding = serde_json::from_value(discord_owner(binding)).unwrap();
        store.bind_discord_backend(&owner).unwrap();
        let request: PrivateChannelRequest = serde_json::from_value(discord_body(binding)).unwrap();
        assert!(store
            .admit_private_backend_request(&request, "tenant-alice", BackendProtocol::Discord)
            .unwrap());
        assert!(!store
            .admit_private_backend_request(&request, "tenant-alice", BackendProtocol::Discord)
            .unwrap());
        let SessionStore::Sqlite { conn, .. } = &store else {
            panic!("SQLite")
        };
        assert!(conn
            .execute("DELETE FROM gateway_discord_backend_requests", [])
            .is_err());
        assert!(conn
            .execute(
                "UPDATE gateway_discord_backend_requests SET event_id='666666666666666666'",
                []
            )
            .is_err());
        assert!(conn.execute("INSERT OR REPLACE INTO gateway_discord_backend_requests(request_id,binding_id,event_id,session_id,prompt_sha256,status) VALUES(?1,?2,'666666666666666666',?3,zeroblob(32),'completed')",params![request.request_id,binding.to_string(),request.session_id]).is_err());
        assert!(conn.execute("INSERT OR REPLACE INTO gateway_discord_backend_requests(request_id,binding_id,event_id,session_id,prompt_sha256,status) VALUES(?1,?2,?3,?4,zeroblob(32),'completed')",params![Uuid::now_v7().to_string(),binding.to_string(),request.event_id,request.session_id]).is_err());
        conn.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<15999) INSERT INTO gateway_discord_backend_requests(request_id,binding_id,event_id,session_id,prompt_sha256,status) SELECT '0195ca8e-0000-7000-8000-'||printf('%012x',x),?1,CAST(600000000000000000+x AS TEXT),?2,zeroblob(32),'admitted' FROM n",params![binding.to_string(),format!("discord:{binding}")]).unwrap();
        let mut next = request.clone();
        next.request_id = Uuid::now_v7().to_string();
        next.event_id = "666666666666666666".into();
        assert!(store
            .admit_private_backend_request(&next, "tenant-alice", BackendProtocol::Discord)
            .is_err());
        assert!(store
            .private_backend_receipt(&next.request_id, "tenant-alice", BackendProtocol::Discord)
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .private_backend_receipt(
                    &request.request_id,
                    "tenant-alice",
                    BackendProtocol::Discord
                )
                .unwrap()
                .unwrap()
                .status,
            "admitted"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn discord_owner_commit_deadline_retains_io_permit_until_blocking_write_finishes() {
        let fixture = Fixture::new().await;
        let binding = Uuid::new_v4();
        let sessions = fixture.state.sessions.clone();
        let (locked_send, locked_recv) = tokio::sync::oneshot::channel();
        let (release_send, release_recv) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _guard = sessions.lock().unwrap();
            locked_send.send(()).unwrap();
            release_recv.recv().unwrap();
        });
        locked_recv.await.unwrap();
        let state = fixture.state.clone();
        let caller = tokio::spawn(async move { bind_discord(state, discord_owner(binding)).await });
        while fixture.state.tenant_control_permits.available_permits() == 4 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(11)).await;
        assert!(matches!(
            caller.await.unwrap(),
            Err(AppError::ChannelUnavailable)
        ));
        assert_eq!(fixture.state.tenant_control_permits.available_permits(), 3);
        release_send.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.state.tenant_control_permits.available_permits() != 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            fixture
                .state
                .sessions
                .lock()
                .unwrap()
                .discord_binding()
                .unwrap()
                .unwrap()
                .binding_id,
            binding.to_string()
        );
        assert!(fixture.requests.lock().unwrap().is_empty());
        fixture.cleanup();
    }
}
