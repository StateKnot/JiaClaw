// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Dedicated tenant Slack ingress; gateway-owned, conservative single-attempt delivery.
use super::{
    config::Config,
    proxy::read_response,
    registry::{Registry, SlackBindingSummary},
    scheduler::Stop,
    slack_store::SlackStore,
    Backend, State,
};
use crate::{
    channel_store::{ChannelDelivery, ChannelEvent},
    channel_types::{Channel, Destination, EventSpec},
    outbound::{self, DeliveryOutcome, OutboundClient},
};
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{Path, Request, State as ExtractState},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};
use uuid::Uuid;

const MAX_BODY: usize = 64 * 1024;
const MAX_REPLY: usize = 2 * 1024 * 1024;
const BACKEND_TIMEOUT: Duration = Duration::from_secs(150);
const IO_CAPACITY_BUSY: &str = "Slack I/O capacity busy";
const IO_TASK_FAILED: &str = "Slack I/O task failed";
struct Installation {
    binding: SlackBindingSummary,
    token: String,
    secret: String,
    api_base: String,
    client: OutboundClient,
    store: Arc<Mutex<SlackStore>>,
    inbound: Arc<Semaphore>,
}
pub(super) struct Runtime {
    installations: HashMap<Uuid, Arc<Installation>>,
    inbound: Arc<Semaphore>,
    io: Arc<Semaphore>,
}
fn now_ms() -> i64 {
    crate::scheduler::now_ms()
}
fn same_binding(a: &SlackBindingSummary, b: &SlackBindingSummary) -> bool {
    a.id == b.id
        && a.user_id == b.user_id
        && a.backend_id == b.backend_id
        && a.bot_id == b.bot_id
        && a.sender_id == b.sender_id
        && a.team_id == b.team_id
        && a.app_id == b.app_id
        && a.bot_user_id == b.bot_user_id
        && a.conversation_id == b.conversation_id
}
fn decode_object<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid channel JSON"))?;
    ensure!(value.is_object(), "channel response must be a JSON object");
    drop(value);
    // Deserialize the original bytes, not Value: preserve duplicate-field rejection.
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid channel response fields"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendStatus {
    protocol: u8,
    backend_id: String,
    max_run_seconds: u64,
    tools: Vec<String>,
    mode: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackendIdentity {
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
fn binding_identity(binding: &SlackBindingSummary) -> Value {
    json!({"protocol":2,"binding_id":binding.id.to_string(),"user_id":binding.user_id.to_string(),
        "backend_id":binding.backend_id,"team_id":binding.team_id,"app_id":binding.app_id,
        "bot_user_id":binding.bot_user_id,"bot_id":binding.bot_id,"sender_id":binding.sender_id,
        "conversation_id":binding.conversation_id})
}
#[derive(Deserialize)]
struct AuthInfo {
    ok: bool,
    team_id: String,
    user_id: String,
    bot_id: String,
    is_enterprise_install: Option<bool>,
    enterprise_id: Option<String>,
}
#[derive(Deserialize)]
struct BotInfo {
    ok: bool,
    bot: BotIdentity,
}
#[derive(Deserialize)]
struct BotIdentity {
    id: String,
    user_id: String,
    app_id: String,
    deleted: bool,
}
#[derive(Deserialize)]
struct HumanInfo {
    ok: bool,
    user: HumanIdentity,
}
#[derive(Deserialize)]
struct HumanIdentity {
    id: String,
    team_id: String,
    deleted: bool,
    is_bot: bool,
    is_app_user: bool,
    is_stranger: Option<bool>,
    is_restricted: Option<bool>,
    is_ultra_restricted: Option<bool>,
}
#[derive(Deserialize)]
struct ConversationInfo {
    ok: bool,
    channel: ConversationIdentity,
}
#[derive(Deserialize)]
struct ConversationIdentity {
    id: String,
    is_im: bool,
    user: String,
    is_user_deleted: bool,
    is_shared: Option<bool>,
    is_ext_shared: Option<bool>,
    is_org_shared: Option<bool>,
    is_mpim: Option<bool>,
    is_channel: Option<bool>,
    is_group: Option<bool>,
    context_team_id: Option<String>,
    shared_team_ids: Option<Vec<String>>,
}
async fn verification_json<T: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T> {
    let response = request.timeout(Duration::from_secs(5)).send().await?;
    ensure!(
        json_response(&response),
        "Slack installation verification rejected"
    );
    let bytes = read_response(response, MAX_BODY)
        .await
        .map_err(|()| anyhow::anyhow!("invalid Slack installation response"))?;
    decode_object(&bytes)
}
async fn verify_installation(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    binding: &SlackBindingSummary,
) -> Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let base = api_base.trim_end_matches('/');
        let auth: AuthInfo =
            verification_json(client.post(format!("{base}/auth.test")).bearer_auth(token)).await?;
        ensure!(
            auth.ok
                && auth.team_id == binding.team_id
                && auth.user_id == binding.bot_user_id
                && auth.bot_id == binding.bot_id
                && auth.is_enterprise_install != Some(true)
                && auth.enterprise_id.is_none(),
            "Slack token identity mismatch"
        );
        let bot: BotInfo = verification_json(
            client
                .get(format!("{base}/bots.info"))
                .bearer_auth(token)
                .query(&[("bot", binding.bot_id.as_str())]),
        )
        .await?;
        ensure!(
            bot.ok
                && !bot.bot.deleted
                && bot.bot.id == binding.bot_id
                && bot.bot.user_id == binding.bot_user_id
                && bot.bot.app_id == binding.app_id,
            "Slack app identity mismatch"
        );
        let human: HumanInfo = verification_json(
            client
                .get(format!("{base}/users.info"))
                .bearer_auth(token)
                .query(&[("user", binding.sender_id.as_str())]),
        )
        .await?;
        ensure!(
            human.ok
                && human.user.id == binding.sender_id
                && human.user.team_id == binding.team_id
                && !human.user.deleted
                && !human.user.is_bot
                && !human.user.is_app_user
                && human.user.is_stranger != Some(true)
                && human.user.is_restricted != Some(true)
                && human.user.is_ultra_restricted != Some(true),
            "Slack member identity mismatch"
        );
        let dm: ConversationInfo = verification_json(
            client
                .get(format!("{base}/conversations.info"))
                .bearer_auth(token)
                .query(&[("channel", binding.conversation_id.as_str())]),
        )
        .await?;
        let dm_channel = dm.channel;
        let dm_identity_ok = dm.ok
            && dm_channel.id == binding.conversation_id
            && dm_channel.is_im
            && dm_channel.user == binding.sender_id
            && !dm_channel.is_user_deleted
            && dm_channel.is_shared != Some(true)
            && dm_channel.is_ext_shared != Some(true)
            && dm_channel.is_org_shared != Some(true)
            && dm_channel.is_mpim != Some(true)
            && dm_channel.is_channel != Some(true)
            && dm_channel.is_group != Some(true)
            && dm_channel
                .context_team_id
                .as_ref()
                .is_none_or(|id| id == &binding.team_id)
            && dm_channel
                .shared_team_ids
                .as_ref()
                .is_none_or(|ids| ids.len() <= 1 && ids.iter().all(|id| id == &binding.team_id));
        ensure!(dm_identity_ok, "Slack DM identity mismatch");
        Ok::<_, anyhow::Error>(())
    })
    .await;
    // Never propagate provider response bodies, configured URLs or token-bearing errors.
    ensure!(
        matches!(result, Ok(Ok(()))),
        "Slack installation identity verification failed"
    );
    Ok(())
}
pub(super) async fn configure(
    config: &Config,
    registry: &Registry,
    client: &reqwest::Client,
    backends: &HashMap<String, Backend>,
    backend_tokens: &HashSet<String>,
) -> Result<Option<Arc<Runtime>>> {
    if config.slack.is_empty() {
        return Ok(None);
    }
    let bindings = registry.list_slack_bindings()?;
    let mut installations = HashMap::new();
    let mut secrets = backend_tokens.clone();
    for entry in &config.telegram {
        secrets.insert(super::telegram::secret_file(&entry.bot_token_file)?);
        secrets.insert(super::telegram::secret_file(&entry.webhook_secret_file)?);
    }
    for entry in &config.slack {
        let id = Uuid::parse_str(&entry.binding_id)?;
        let binding = bindings
            .iter()
            .find(|b| b.id == id)
            .context("configured Slack binding does not exist")?
            .clone();
        ensure!(
            binding.enabled,
            "configured Slack binding is revoked; remove its runtime configuration"
        );
        let backend = backends
            .get(&binding.backend_id)
            .context("Slack backend is not configured")?;
        let token = super::telegram::secret_file(&entry.bot_token_file)?;
        let webhook = super::telegram::secret_file(&entry.signing_secret_file)?;
        ensure!(
            token.starts_with("xoxb-")
                && token.len() <= 2048
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
            "Slack requires a bounded workspace bot token"
        );
        ensure!(
            (32..=256).contains(&webhook.len()),
            "Slack signing secret requires 32..256 bytes"
        );
        ensure!(
            secrets.insert(token.clone()) && secrets.insert(webhook.clone()),
            "Slack, Telegram and backend credentials must be distinct"
        );
        verify_installation(client, &entry.api_base, &token, &binding).await?;
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            let response = client
                .get(backend.url.join("internal/channels/slack/status")?)
                .header(header::AUTHORIZATION, backend.token.clone())
                .timeout(Duration::from_secs(5))
                .send()
                .await?;
            ensure!(
                json_response(&response),
                "gateway channel handshake rejected"
            );
            let bytes = read_response(response, 4096)
                .await
                .map_err(|()| anyhow::anyhow!("invalid gateway channel handshake"))?;
            let status: BackendStatus = decode_object(&bytes)?;
            ensure!(
                status.protocol == 2
                    && status.backend_id == binding.backend_id
                    && status.mode == "gateway"
                    && status.max_run_seconds == 120
                    && status.tools == ["datetime_now", "json_query"],
                "gateway channel contract mismatch"
            );
            let identity = binding_identity(&binding);
            let response = client
                .post(backend.url.join("internal/channels/slack-binding")?)
                .header(header::AUTHORIZATION, backend.token.clone())
                .timeout(Duration::from_secs(5))
                .json(&identity)
                .send()
                .await?;
            ensure!(json_response(&response), "backend Slack binding rejected");
            let bytes = read_response(response, 4096)
                .await
                .map_err(|()| anyhow::anyhow!("invalid Slack binding response"))?;
            let actual: BackendIdentity = decode_object(&bytes)?;
            ensure!(
                serde_json::to_value(actual)? == identity,
                "backend Slack owner mismatch"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("gateway channel handshake timed out")?;
        // Do not expose reqwest errors containing a configured backend URL/token.
        ensure!(status.is_ok(), "gateway channel handshake failed");
        let db = SlackStore::open(&config.registry_path, &binding)?;
        installations.insert(
            id,
            Arc::new(Installation {
                binding,
                token,
                secret: webhook,
                api_base: entry.api_base.clone(),
                client: OutboundClient::new_with_loopback(entry.allow_loopback)?,
                store: Arc::new(Mutex::new(db)),
                inbound: Arc::new(Semaphore::new(1)),
            }),
        );
    }
    Ok(Some(Arc::new(Runtime {
        installations,
        inbound: Arc::new(Semaphore::new(8)),
        io: Arc::new(Semaphore::new(8)),
    })))
}
async fn io<T: Send + 'static>(
    runtime: &Runtime,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = runtime
        .io
        .clone()
        .try_acquire_owned()
        .context(IO_CAPACITY_BUSY)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .context(IO_TASK_FAILED)?
}
fn answer(status: StatusCode, code: &'static str) -> Response {
    (status, Json(json!({"status":code}))).into_response()
}
#[derive(Deserialize)]
struct Authorization {
    team_id: String,
    user_id: String,
    is_bot: bool,
    is_enterprise_install: Option<bool>,
    enterprise_id: Option<String>,
}
#[derive(Deserialize)]
struct Callback {
    #[serde(rename = "type")]
    kind: String,
    team_id: Option<String>,
    api_app_id: Option<String>,
    event_id: Option<String>,
    authorizations: Option<Vec<Authorization>>,
    is_ext_shared_channel: Option<bool>,
    challenge: Option<String>,
    enterprise_id: Option<String>,
    event: Option<SlackMessage>,
}
#[derive(Deserialize)]
struct SlackMessage {
    #[serde(rename = "type")]
    kind: String,
    user: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    text: Option<String>,
    ts: Option<String>,
    thread_ts: Option<String>,
    subtype: Option<Value>,
    bot_id: Option<String>,
    files: Option<Value>,
    attachments: Option<Value>,
    user_team: Option<String>,
    team: Option<String>,
    is_ext_shared_channel: Option<bool>,
}
fn event_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value.starts_with("Ev")
        && value[2..].bytes().all(|b| b.is_ascii_alphanumeric())
}
fn timestamp(value: &str) -> bool {
    value.split_once('.').is_some_and(|(seconds, fraction)| {
        !seconds.is_empty()
            && seconds.len() <= 16
            && !seconds.starts_with('0')
            && seconds.bytes().all(|b| b.is_ascii_digit())
            && fraction.len() == 6
            && fraction.bytes().all(|b| b.is_ascii_digit())
    })
}
fn parse_event(binding: &SlackBindingSummary, value: Callback) -> Result<Option<EventSpec>> {
    ensure!(
        value.team_id.as_deref() == Some(&binding.team_id)
            && value.api_app_id.as_deref() == Some(&binding.app_id)
            && value.is_ext_shared_channel != Some(true)
            && value.enterprise_id.is_none(),
        "Slack installation mismatch"
    );
    let authorizations = value
        .authorizations
        .context("missing Slack authorization")?;
    ensure!(
        authorizations.len() == 1
            && authorizations[0].team_id == binding.team_id
            && authorizations[0].user_id == binding.bot_user_id
            && authorizations[0].is_bot
            && authorizations[0].is_enterprise_install != Some(true)
            && authorizations[0].enterprise_id.is_none(),
        "Slack authorization mismatch"
    );
    let id = value
        .event_id
        .filter(|id| event_id(id))
        .context("invalid Slack event ID")?;
    let message = value.event.context("missing Slack event")?;
    if message.kind != "message" {
        return Ok(None);
    }
    ensure!(
        message.subtype.is_none()
            && message.bot_id.is_none()
            && message.thread_ts.is_none()
            && message.files.is_none()
            && message.attachments.is_none()
            && message.channel_type.as_deref() == Some("im")
            && message.channel.as_deref() == Some(&binding.conversation_id)
            && message.user.as_deref() == Some(&binding.sender_id)
            && message.is_ext_shared_channel != Some(true)
            && message
                .user_team
                .as_ref()
                .is_none_or(|id| id == &binding.team_id)
            && message
                .team
                .as_ref()
                .is_none_or(|id| id == &binding.team_id)
            && message.ts.as_deref().is_some_and(timestamp),
        "Slack private text identity mismatch"
    );
    // Slack's ordinary user messages can carry rich_text blocks. They are ignored:
    // only the signed text enters the model, with no media fetch or block execution.
    let prompt = message.text.context("missing Slack text")?;
    ensure!(
        !prompt.trim().is_empty() && prompt.len() <= 16 * 1024,
        "Slack text is empty or oversized"
    );
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            binding.id.to_string(),
            &id,
            &binding.sender_id,
            &binding.conversation_id,
            &message.ts,
            &prompt
        ))?)
    );
    Ok(Some(EventSpec {
        event_id: id,
        session_id: format!("slack:{}", binding.id),
        sender_id: binding.sender_id.clone(),
        prompt,
        enabled_tools: vec!["datetime_now".into(), "json_query".into()],
        timeout_secs: 120,
        destination: destination(binding),
        sealed_token: None,
        fingerprint,
    }))
}
fn destination(binding: &SlackBindingSummary) -> Destination {
    Destination {
        channel: Channel::Slack,
        installation_id: binding.team_id.clone(),
        conversation_id: binding.conversation_id.clone(),
        thread_id: None,
        interaction_id: None,
        expires_ms: None,
    }
}
pub(super) async fn ingress(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    tokio::time::timeout(
        Duration::from_millis(2800),
        ingress_with_budget(state, id, request),
    )
    .await
    .unwrap_or_else(|_| answer(StatusCode::SERVICE_UNAVAILABLE, "ingress_deadline"))
}
async fn ingress_with_budget(state: Arc<State>, id: String, request: Request) -> Response {
    let Some(runtime) = state.slack.as_ref() else {
        return answer(StatusCode::NOT_FOUND, "disabled");
    };
    let Some(id) = Uuid::parse_str(&id)
        .ok()
        .filter(|uuid| uuid.to_string() == id)
    else {
        return answer(StatusCode::NOT_FOUND, "unknown_binding");
    };
    let Some(installation) = runtime.installations.get(&id).cloned() else {
        return answer(StatusCode::NOT_FOUND, "unknown_binding");
    };
    if request.uri().query().is_some() {
        return answer(StatusCode::BAD_REQUEST, "query_not_allowed");
    }
    let (timestamp, signature) = {
        let single = |name: &'static str| {
            let mut values = request.headers().get_all(name).iter();
            let value = values.next().and_then(|value| value.to_str().ok());
            if values.next().is_some() {
                None
            } else {
                value.map(str::to_owned)
            }
        };
        (
            single("x-slack-request-timestamp"),
            single("x-slack-signature"),
        )
    };
    let header_valid = timestamp.as_deref().is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 20
            && value.bytes().all(|b| b.is_ascii_digit())
            && value
                .parse::<u64>()
                .is_ok_and(|number| number.to_string() == value)
    }) && signature.as_deref().is_some_and(|value| {
        value.len() == 67
            && value.starts_with("v0=")
            && value[3..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    if !header_valid
        || !timestamp
            .as_deref()
            .is_some_and(|value| crate::slack_timestamp_fresh(value, crate::unix_now_secs()))
    {
        return answer(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
    {
        return answer(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required");
    }
    let Ok(_global) = runtime.inbound.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let Ok(_local) = installation.inbound.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let body = match tokio::time::timeout(Duration::from_secs(2), async {
        let mut stream = request.into_body().into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(status)) => return answer(status, "invalid_body"),
        Err(_) => return answer(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
    };
    if !crate::verify_slack_request(
        &installation.secret,
        timestamp.as_deref(),
        signature.as_deref(),
        &body,
        crate::unix_now_secs(),
    ) {
        return answer(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    match io(runtime, move || {
        Ok(registry
            .slack_authorized(id)?
            .is_some_and(|actual| same_binding(&actual, &expected)))
    })
    .await
    {
        Ok(true) => (),
        Ok(false) => return answer(StatusCode::FORBIDDEN, "binding_disabled"),
        Err(_) => return answer(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    }
    let callback: Callback = match decode_object(&body) {
        Ok(callback) => callback,
        Err(_) => return answer(StatusCode::BAD_REQUEST, "invalid_callback"),
    };
    if callback.kind == "url_verification" {
        let Some(challenge) = callback
            .challenge
            .filter(|value| !value.is_empty() && value.len() <= 1024)
        else {
            return answer(StatusCode::BAD_REQUEST, "invalid_challenge");
        };
        return Json(json!({"challenge":challenge})).into_response();
    }
    if callback.kind != "event_callback" {
        return answer(StatusCode::OK, "ignored");
    }
    let event = match parse_event(&installation.binding, callback) {
        Ok(Some(event)) => event,
        Ok(None) => return answer(StatusCode::OK, "ignored"),
        Err(_) => return answer(StatusCode::BAD_REQUEST, "event_not_authorized"),
    };
    let store = installation.store.clone();
    match io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Slack store poisoned"))?
            .inner
            .accept_channel_event(event, now_ms())
    })
    .await
    {
        Ok(_) => answer(StatusCode::OK, "accepted"),
        Err(error)
            if error
                .downcast_ref::<crate::channel_store::ChannelConflict>()
                .is_some() =>
        {
            answer(StatusCode::CONFLICT, "fingerprint_conflict")
        }
        Err(error)
            if error
                .downcast_ref::<crate::channel_store::ChannelCapacity>()
                .is_some() =>
        {
            answer(StatusCode::TOO_MANY_REQUESTS, "queue_full")
        }
        Err(_) => answer(StatusCode::SERVICE_UNAVAILABLE, "admission_failed"),
    }
}

pub(super) struct Worker {
    stop: Stop,
    task: Option<JoinHandle<()>>,
}
impl Worker {
    pub(super) fn stopper(&self) -> Stop {
        self.stop.clone()
    }
    pub(super) async fn shutdown(mut self, grace: Duration) -> bool {
        self.stop.stop();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        matches!(tokio::time::timeout(grace, &mut task).await, Ok(Ok(())))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.stop();
    }
}
pub(super) fn start(state: Arc<State>) -> Worker {
    let stop = Stop::new();
    let task = state
        .slack
        .is_some()
        .then(|| tokio::spawn(coordinate(state, stop.clone())));
    Worker { stop, task }
}
async fn coordinate(state: Arc<State>, stop: Stop) {
    let runtime = state.slack.as_ref().unwrap().clone();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut workers = JoinSet::new();
    let mut running = HashSet::new();
    let mut offset = 0usize;
    while !stop.is_stopped() {
        tokio::select! {
            result = workers.join_next(), if !workers.is_empty() => {
                if let Some(Ok(id)) = result {
                    running.remove(&id);
                }
            },
            _ = ticker.tick() => {
                let mut ids: Vec<_> = runtime.installations.keys().copied().collect();
                ids.sort();
                if !ids.is_empty() {
                    let len = ids.len();
                    ids.rotate_left(offset % len);
                    offset = offset.wrapping_add(1);
                }
                for id in ids {
                    if stop.is_stopped() {
                        break;
                    }
                    if !running.insert(id) {
                        continue;
                    }
                    let state = state.clone();
                    let stop = stop.clone();
                    let installation = runtime.installations[&id].clone();
                    workers.spawn(async move {
                        process(state, stop, installation).await;
                        id
                    });
                }
            }
        }
    }
    while workers.join_next().await.is_some() {}
}
fn finish_error_code(error: &anyhow::Error) -> &'static str {
    // Classify the typed cause without logging its message: SQLite and I/O
    // errors may contain database paths, SQL, or other private values.
    if let Some(error) = error.downcast_ref::<rusqlite::Error>() {
        return match error {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                "registry_busy"
            }
            _ => "registry_storage",
        };
    }
    if error.downcast_ref::<std::io::Error>().is_some() {
        return "registry_io";
    }
    // These two contexts belong to this module. Exact, top-level matching
    // avoids treating arbitrary nested or partially matching text as a cause.
    match error.to_string().as_str() {
        IO_CAPACITY_BUSY => "io_busy",
        IO_TASK_FAILED => "io_task_failed",
        _ => "registry_validation",
    }
}
async fn finish_io<T: Send + 'static>(
    runtime: &Runtime,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    // Finishing an already admitted effect must not lose its durable result just
    // because the bounded pool is temporarily occupied by valid ingress/claims.
    let permit = tokio::time::timeout(Duration::from_secs(5), runtime.io.clone().acquire_owned())
        .await
        .context(IO_CAPACITY_BUSY)?
        .context(IO_CAPACITY_BUSY)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .context(IO_TASK_FAILED)?
}
async fn finish(
    state: &State,
    runtime: &Runtime,
    binding: &SlackBindingSummary,
    request_id: Uuid,
    known: bool,
) {
    let registry = state.registry.clone();
    let user = binding.user_id;
    if let Err(error) = finish_io(runtime, move || {
        registry.finish_write(user, request_id, known)
    })
    .await
    {
        tracing::error!(
            code = finish_error_code(&error),
            request_id = %request_id,
            user_id = %user,
            known,
            "Slack write hold requires administrator review"
        );
    }
}
async fn process(state: Arc<State>, stop: Stop, installation: Arc<Installation>) {
    let runtime = state.slack.as_ref().unwrap();
    let Some(backend) = state.backends.get(&installation.binding.backend_id) else {
        return;
    };
    let Ok(_local) = backend.permit.clone().try_acquire_owned() else {
        return;
    };
    let Ok(_global) = state.permits.clone().try_acquire_owned() else {
        return;
    };
    if stop.is_stopped() {
        return;
    }
    let store = installation.store.clone();
    let Ok((execution, delivery)) = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Slack store poisoned"))?
            .pending(now_ms())
    })
    .await
    else {
        return;
    };
    if !execution && !delivery {
        return;
    }
    let operation = if delivery {
        "slack_send"
    } else {
        "slack_execute"
    };
    let request_id = Uuid::now_v7();
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    let authorized = io(runtime, move || {
        stop.admit(|| {
            registry.admit_slack(expected.id, request_id, operation, &expected.id.to_string())
        })
        .context("gateway stopping")?
    })
    .await;
    let Ok(actual) = authorized else { return };
    if !same_binding(&actual, &installation.binding) {
        finish(&state, runtime, &installation.binding, request_id, false).await;
        return;
    }
    let known = if delivery {
        send(&state, &installation, request_id).await
    } else {
        execute(&state, &installation, request_id).await
    };
    finish(&state, runtime, &installation.binding, request_id, known).await;
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatReceipt {
    protocol: u8,
    backend_id: String,
    request_id: String,
    binding_id: String,
    response: jiaclaw_core::ChatResponse,
}
fn json_response(response: &reqwest::Response) -> bool {
    response.status() == reqwest::StatusCode::OK
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
}
async fn execute(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.slack.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Slack store poisoned"))?
            .inner
            .claim_slack_event_recorded(now_ms(), &request_id.to_string())
    })
    .await;
    let event = match claimed {
        Ok(Some(e)) => e,
        Ok(None) => return true,
        Err(_) => return false,
    };
    let result = run_chat(state, installation, &event, request_id).await;
    let successful = result.is_ok();
    let chunks = result.unwrap_or_default();
    let store = installation.store.clone();
    let committed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Slack store poisoned"))?
            .inner
            .complete_channel_event(
                &event.id,
                None,
                if successful {
                    "completed"
                } else {
                    "needs_review"
                },
                chunks,
                (!successful).then(|| {
                    "gateway channel execution requires review; no automatic replay".into()
                }),
                now_ms(),
            )
    })
    .await;
    matches!(committed, Ok(true)) && successful
}
async fn run_chat(
    state: &State,
    installation: &Installation,
    event: &ChannelEvent,
    request_id: Uuid,
) -> Result<Vec<String>> {
    ensure!(
        event.spec.destination == destination(&installation.binding)
            && event.spec.session_id == format!("slack:{}", installation.binding.id)
            && event.spec.sender_id == installation.binding.sender_id
            && event.spec.enabled_tools == ["datetime_now", "json_query"]
            && event.spec.timeout_secs == 120
            && event.spec.sealed_token.is_none(),
        "channel authorization mismatch"
    );
    let backend = &state.backends[&installation.binding.backend_id];
    let receipt=tokio::time::timeout(BACKEND_TIMEOUT,async {
        let response=state.client.post(backend.url.join("internal/channels/slack/execute")?).header(header::AUTHORIZATION,backend.token.clone()).timeout(BACKEND_TIMEOUT).json(&json!({"protocol":2,"request_id":request_id.to_string(),"binding_id":installation.binding.id.to_string(),"session_id":event.spec.session_id,"event_id":event.spec.event_id,"prompt":event.spec.prompt})).send().await?;
        ensure!(json_response(&response),"backend channel request failed");
        let bytes=read_response(response,MAX_REPLY).await.map_err(|()|anyhow::anyhow!("invalid channel response"))?;
        decode_object::<ChatReceipt>(&bytes)
    }).await.context("backend channel deadline")??;
    let reply = &receipt.response;
    ensure!(
        receipt.protocol == 2
            && receipt.backend_id == installation.binding.backend_id
            && receipt.request_id == request_id.to_string()
            && receipt.binding_id == installation.binding.id.to_string()
            && reply.session_id.as_deref() == Some(event.spec.session_id.as_str())
            && reply.status == jiaclaw_core::RunStatus::Completed
            && reply.message.role == jiaclaw_core::MessageRole::Assistant
            && reply.tool_calls.iter().all(|call| matches!(
                call.tool_name.as_str(),
                "datetime_now" | "json_query"
            ) && call
                .result
                .as_ref()
                .is_some_and(|value| value.get("error").is_none())),
        "channel receipt mismatch or unresolved tool result"
    );
    outbound::split_text_for(Channel::Slack, &reply.message.content)
}
async fn send(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.slack.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Slack store poisoned"))?
            .inner
            .claim_slack_delivery_recorded(now_ms(), &request_id.to_string())
    })
    .await;
    let delivery = match claimed {
        Ok(Some(d)) => d,
        Ok(None) => return true,
        Err(_) => return false,
    };
    let outcome = if delivery.destination == destination(&installation.binding)
        && delivery.event_id.is_some()
        && delivery.job_run_id.is_none()
        && delivery.sealed_token.is_none()
    {
        installation
            .client
            .send(
                &delivery.destination,
                delivery.ordinal as usize,
                &delivery.text,
                &installation.token,
                &installation.api_base,
            )
            .await
    } else {
        DeliveryOutcome::Rejected {
            code: "binding_mismatch",
        }
    };
    settle(runtime, installation, delivery, outcome).await
}
async fn settle(
    runtime: &Runtime,
    installation: &Installation,
    delivery: ChannelDelivery,
    outcome: DeliveryOutcome,
) -> bool {
    let now = now_ms();
    let (status, receipt, error, retry, known) = match outcome {
        DeliveryOutcome::Delivered { receipt } => ("delivered", Some(receipt), None, None, true),
        DeliveryOutcome::RateLimited { retry_after_ms } => (
            "retry_wait",
            None,
            Some("rate_limited".into()),
            now.checked_add(retry_after_ms),
            delivery.attempts < 5,
        ),
        DeliveryOutcome::Unknown { code } => ("unknown", None, Some(code.into()), None, false),
        DeliveryOutcome::Rejected { code } => {
            ("permanent_failed", None, Some(code.into()), None, false)
        }
    };
    let store = installation.store.clone();
    matches!(
        io(runtime, move || store
            .lock()
            .map_err(|_| anyhow::anyhow!("Slack store poisoned"))?
            .inner
            .finish_channel_delivery_with_discord(
                &delivery.id,
                delivery.attempts,
                status,
                receipt,
                error,
                retry,
                now,
                None
            ))
        .await,
        Ok(true)
    ) && known
}

pub(super) enum AdminAction {
    Inspect {
        kind: String,
        event: Option<String>,
        limit: usize,
        offset: usize,
    },
    Resolve {
        delivery: String,
        receipt: String,
    },
    Cancel {
        event: String,
    },
    Purge {
        event: String,
    },
}
fn stopped(config: &Config) -> Result<File> {
    use fs2::FileExt;
    let lock = super::private_file(&config.registry_path.with_extension("gateway.lock"))?;
    lock.try_lock_exclusive()
        .context("stop gateway before inspecting or reconciling Slack state")?;
    Ok(lock)
}
fn checked_uuid(id: &str) -> Result<()> {
    ensure!(
        Uuid::parse_str(id).is_ok_and(|value| !value.is_nil() && value.to_string() == id),
        "canonical object UUID required"
    );
    Ok(())
}
pub(super) fn admin(config: &Config, binding_id: Uuid, action: AdminAction) -> Result<Value> {
    let registry = Registry::open(&config.registry_path)?;
    let _stopped = stopped(config)?;
    let binding = registry
        .list_slack_bindings()?
        .into_iter()
        .find(|b| b.id == binding_id)
        .context("Slack binding not found")?;
    registry.recover_writes()?;
    let mut store = SlackStore::open(&config.registry_path, &binding)?;
    let value = match action {
        AdminAction::Inspect {
            kind,
            event,
            limit,
            offset,
        } => {
            ensure!(
                (1..=100).contains(&limit)
                    && offset <= if kind == "operations" { 16000 } else { 10000 },
                "inspection page out of range"
            );
            if let Some(id) = &event {
                checked_uuid(id)?;
            }
            match kind.as_str() {
                "events" => json!({"events":store.inner.list_channel_events(limit,offset)?}),
                "deliveries" => {
                    json!({"deliveries":store.inner.list_channel_deliveries(event.as_deref(),limit,offset)?})
                }
                "operations" => json!({"operations":store.operations(limit,offset)?}),
                _ => anyhow::bail!("inspection kind must be events, deliveries or operations"),
            }
        }
        AdminAction::Resolve { delivery, receipt } => {
            checked_uuid(&delivery)?;
            ensure!(
                store
                    .inner
                    .resolve_channel_delivery(&delivery, receipt, now_ms())?,
                "delivery not found"
            );
            json!({"delivery_id":delivery,"reconciled":true})
        }
        AdminAction::Cancel { event } => {
            checked_uuid(&event)?;
            ensure!(
                store.inner.cancel_channel_event(&event, now_ms())?,
                "event not found"
            );
            json!({"event_id":event,"cancelled":true})
        }
        AdminAction::Purge { event } => {
            checked_uuid(&event)?;
            ensure!(
                store.inner.purge_channel_event(&event, now_ms())?,
                "event not found"
            );
            json!({"event_id":event,"purged":true})
        }
    };
    Ok(json!({"binding_id":binding_id.to_string(),"result":value}))
}
/// Called only while the gateway process lock is retained through registry clearing.
pub(super) fn review_pending(config: &Config, registry: &Registry, user: Uuid) -> Result<()> {
    let Some(binding) = registry
        .list_slack_bindings()?
        .into_iter()
        .find(|b| b.user_id == user)
    else {
        return Ok(());
    };
    let store = SlackStore::open(&config.registry_path, &binding)?;
    super::review_queue(&store.inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> SlackBindingSummary {
        SlackBindingSummary {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            backend_id: "alice".into(),
            team_id: "T123".into(),
            app_id: "A123".into(),
            bot_user_id: "W123".into(),
            bot_id: "B123".into(),
            sender_id: "U456".into(),
            conversation_id: "D456".into(),
            enabled: true,
        }
    }
    fn callback() -> Value {
        json!({"type":"event_callback","team_id":"T123","api_app_id":"A123","event_id":"Ev123abc",
            "authorizations":[{"team_id":"T123","user_id":"W123","is_bot":true}],
            "event":{"type":"message","user":"U456","channel":"D456","channel_type":"im",
                "text":"hello <@U789>","ts":"1700000000.123456",
                "blocks":[{"type":"rich_text","elements":[{"text":"ignored block"}]}]}})
    }
    fn parse(binding: &SlackBindingSummary, value: Value) -> Result<Option<EventSpec>> {
        parse_event(binding, decode_object(&serde_json::to_vec(&value)?)?)
    }
    #[test]
    fn signed_text_fixes_owner_destination_and_fingerprint_and_ignores_rich_blocks() {
        let b = binding();
        let first = parse(&b, callback()).unwrap().unwrap();
        assert_eq!(first.prompt, "hello <@U789>");
        assert_eq!(first.destination, destination(&b));
        assert_eq!(first.session_id, format!("slack:{}", b.id));
        assert_eq!(first.enabled_tools, ["datetime_now", "json_query"]);
        assert_eq!(first.timeout_secs, 120);
        assert!(first.sealed_token.is_none());
        let mut value = callback();
        value["event"]["blocks"] = json!([{"text":"different ignored"}]);
        assert_eq!(
            parse(&b, value.clone()).unwrap().unwrap().fingerprint,
            first.fingerprint
        );
        value["event"]["text"] = json!("changed signed text");
        assert_ne!(
            parse(&b, value).unwrap().unwrap().fingerprint,
            first.fingerprint
        );
        let mut other = b.clone();
        other.id = Uuid::new_v4();
        assert_ne!(
            parse(&other, callback()).unwrap().unwrap().fingerprint,
            first.fingerprint
        );
    }
    #[test]
    fn installation_authorization_and_private_root_human_message_are_required() {
        let b = binding();
        for (pointer, replacement) in [
            ("/team_id", json!("T999")),
            ("/api_app_id", json!("A999")),
            ("/is_ext_shared_channel", json!(true)),
            ("/enterprise_id", json!("E123")),
            ("/authorizations", json!([])),
            (
                "/authorizations",
                json!([{"team_id":"T123","user_id":"W123","is_bot":true},{"team_id":"T123","user_id":"W123","is_bot":true}]),
            ),
            ("/authorizations/0/team_id", json!("T999")),
            ("/authorizations/0/user_id", json!("U999")),
            ("/authorizations/0/is_bot", json!(false)),
            ("/authorizations/0/is_enterprise_install", json!(true)),
            ("/authorizations/0/enterprise_id", json!("E123")),
            ("/event/channel", json!("D999")),
            ("/event/channel_type", json!("channel")),
            ("/event/user", json!("W123")),
            ("/event/user_team", json!("T999")),
            ("/event/team", json!("T999")),
            ("/event/is_ext_shared_channel", json!(true)),
            ("/event/thread_ts", json!("1700000000.123456")),
            ("/event/subtype", json!("message_changed")),
            ("/event/bot_id", json!("B123")),
            ("/event/files", json!([])),
            ("/event/attachments", json!([])),
            ("/event/text", json!("  ")),
            ("/event/text", json!("a".repeat(16 * 1024 + 1))),
            ("/event/ts", json!("01700000000.123456")),
            ("/event/ts", json!("1700000000.12345")),
            ("/event_id", json!("EV123")),
            ("/event_id", json!("Ev../private")),
        ] {
            let mut value = callback();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            value.pointer_mut(parent).unwrap()[key] = replacement;
            assert!(parse(&b, value).is_err(), "accepted {pointer}");
        }
        let mut value = callback();
        value["event"]["type"] = json!("app_home_opened");
        assert!(parse(&b, value).unwrap().is_none());
    }
    #[test]
    fn duplicate_security_fields_are_rejected_before_value_can_collapse_them() {
        for input in [
            r#"{"type":"event_callback","type":"url_verification"}"#,
            r#"{"type":"event_callback","team_id":"T123","team_id":"T999"}"#,
            r#"{"type":"event_callback","authorizations":[{"team_id":"T123","user_id":"U1","user_id":"U2","is_bot":true}]}"#,
            r#"{"type":"event_callback","event":{"type":"message","user":"U1","user":"U2"}}"#,
        ] {
            assert!(decode_object::<Callback>(input.as_bytes()).is_err());
        }
    }
    #[tokio::test]
    async fn timed_out_io_retains_capacity_until_blocking_work_finishes() {
        let runtime = Runtime {
            installations: HashMap::new(),
            inbound: Arc::new(Semaphore::new(8)),
            io: Arc::new(Semaphore::new(1)),
        };
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let work = io(&runtime, move || {
            let _ = started_tx.send(());
            release_rx.recv()?;
            Ok(())
        });
        tokio::pin!(work);
        tokio::select! { _=&mut work=>panic!("work completed before release"),_=started_rx=>{} }
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut work)
            .await
            .is_err());
        assert!(io(&runtime, || Ok(())).await.is_err());
        assert_eq!(runtime.io.available_permits(), 0);
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), work)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(runtime.io.available_permits(), 1);
    }
    #[test]
    fn review_clear_checks_revoked_retained_slack_queue_under_one_process_lock() {
        let root = std::env::temp_dir().join(format!("jiaclaw-slack-review-{}", Uuid::new_v4()));
        let path = root.join("registry.sqlite3");
        let registry = Registry::open(&path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let b = registry
            .add_slack_binding(user.user_id, "T123", "A123", "W123", "B123", "U456", "D456")
            .unwrap();
        let config:Config=serde_json::from_value(json!({"registry_path":path,"backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":root.join("backend.token")}]})).unwrap();
        let mut store = SlackStore::open(&path, &b).unwrap();
        let e = store
            .inner
            .accept_channel_event(parse(&b, callback()).unwrap().unwrap(), 0)
            .unwrap();
        store
            .inner
            .claim_slack_event_recorded(0, &Uuid::now_v7().to_string())
            .unwrap()
            .unwrap();
        drop(store);
        registry.revoke_slack_binding(b.id).unwrap();
        assert!(super::super::channel_review_guard(&config, &registry, user.user_id).is_err());
        let mut store = SlackStore::open(&path, &b).unwrap();
        assert!(store.inner.cancel_channel_event(&e.id, now_ms()).unwrap());
        drop(store);
        let guard = super::super::channel_review_guard(&config, &registry, user.user_id)
            .unwrap()
            .unwrap();
        assert!(super::super::channel_review_guard(&config, &registry, user.user_id).is_err());
        drop(guard);
        assert!(
            super::super::channel_review_guard(&config, &registry, user.user_id)
                .unwrap()
                .is_some()
        );
        drop(registry);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn admitted_finish_waits_for_bounded_capacity_instead_of_losing_settlement() {
        let runtime = Runtime {
            installations: HashMap::new(),
            inbound: Arc::new(Semaphore::new(8)),
            io: Arc::new(Semaphore::new(1)),
        };
        let permit = runtime.io.clone().acquire_owned().await.unwrap();
        let work = finish_io(&runtime, || Ok("settled"));
        tokio::pin!(work);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut work)
            .await
            .is_err());
        assert_eq!(runtime.io.available_permits(), 0);
        drop(permit);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), work)
                .await
                .unwrap()
                .unwrap(),
            "settled"
        );
        assert_eq!(runtime.io.available_permits(), 1);
    }
}
