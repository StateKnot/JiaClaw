// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Dedicated tenant Telegram ingress; gateway-owned, conservative single-attempt delivery.
use super::{
    config::Config,
    proxy::read_response,
    registry::{Registry, TelegramBindingSummary},
    scheduler::Stop,
    telegram_store::TelegramStore,
    Backend, State,
};
use crate::{
    channel_store::{ChannelDelivery, ChannelEvent},
    channel_types::{Channel, Destination, EventSpec},
    outbound::{self, DeliveryOutcome, OutboundClient},
};
use anyhow::{ensure, Context, Result};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State as ExtractState},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};
use uuid::Uuid;

const MAX_BODY: usize = 64 * 1024;
const MAX_REPLY: usize = 2 * 1024 * 1024;
const BACKEND_TIMEOUT: Duration = Duration::from_secs(150);
const IO_CAPACITY_BUSY: &str = "Telegram I/O capacity busy";
const IO_TASK_FAILED: &str = "Telegram I/O task failed";
struct Installation {
    binding: TelegramBindingSummary,
    token: String,
    secret: String,
    api_base: String,
    client: OutboundClient,
    store: Arc<Mutex<TelegramStore>>,
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
pub(super) fn secret_file(path: &std::path::Path) -> Result<String> {
    let metadata = path
        .symlink_metadata()
        .context("inspect Telegram secret file")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 4098,
        "Telegram secret must be a bounded regular file"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("open Telegram secret file")?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && opened.len() <= 4098,
        "Telegram secret must be a bounded regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            opened.dev() == metadata.dev()
                && opened.ino() == metadata.ino()
                && opened.nlink() == 1
                && opened.permissions().mode() & 0o007 == 0,
            "Telegram secret must be stable, without hard links or world access"
        );
    }
    let mut secret = String::new();
    file.take(4099)
        .read_to_string(&mut secret)
        .context("read Telegram secret file")?;
    let secret = secret.trim_end_matches(['\r', '\n']);
    ensure!(
        (32..=4096).contains(&secret.len()) && secret.bytes().all(|b| b.is_ascii_graphic()),
        "Telegram secret must contain 32..4096 visible ASCII bytes"
    );
    Ok(secret.into())
}
fn same_binding(a: &TelegramBindingSummary, b: &TelegramBindingSummary) -> bool {
    a.id == b.id
        && a.user_id == b.user_id
        && a.backend_id == b.backend_id
        && a.bot_id == b.bot_id
        && a.sender_id == b.sender_id
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
pub(super) async fn configure(
    config: &Config,
    registry: &Registry,
    client: &reqwest::Client,
    backends: &HashMap<String, Backend>,
    backend_tokens: &HashSet<String>,
) -> Result<Option<Arc<Runtime>>> {
    if config.telegram.is_empty() {
        return Ok(None);
    }
    let bindings = registry.list_telegram_bindings()?;
    let mut installations = HashMap::new();
    let mut secrets = backend_tokens.clone();
    for entry in &config.telegram {
        let id = Uuid::parse_str(&entry.binding_id)?;
        let binding = bindings
            .iter()
            .find(|b| b.id == id)
            .context("configured Telegram binding does not exist")?
            .clone();
        ensure!(
            binding.enabled,
            "configured Telegram binding is revoked; remove its runtime configuration"
        );
        let backend = backends
            .get(&binding.backend_id)
            .context("Telegram backend is not configured")?;
        let token = secret_file(&entry.bot_token_file)?;
        let webhook = secret_file(&entry.webhook_secret_file)?;
        ensure!(
            token
                .split_once(':')
                .is_some_and(|(bot, suffix)| bot == binding.bot_id
                    && !suffix.is_empty()
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))),
            "Telegram credential must match the bound Bot ID"
        );
        ensure!(
            (32..=256).contains(&webhook.len())
                && webhook
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
            "Telegram webhook secret must contain 32..256 approved characters"
        );
        ensure!(
            secrets.insert(token.clone()) && secrets.insert(webhook.clone()),
            "Telegram and backend credentials must be distinct"
        );
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            let response = client
                .get(backend.url.join("internal/gateway/channel/status")?)
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
                status.protocol == 1
                    && status.backend_id == binding.backend_id
                    && status.mode == "gateway"
                    && status.max_run_seconds == 120
                    && status.tools == ["datetime_now", "json_query"],
                "gateway channel contract mismatch"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("gateway channel handshake timed out")?;
        // Do not expose reqwest errors containing a configured backend URL/token.
        ensure!(status.is_ok(), "gateway channel handshake failed");
        let db = TelegramStore::open(&config.registry_path, &binding)?;
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
fn canonical_id(value: &Value, positive: bool) -> Option<String> {
    value
        .as_i64()
        .filter(|id| if positive { *id > 0 } else { *id >= 0 })
        .map(|id| id.to_string())
}
fn parse_event(binding: &TelegramBindingSummary, value: Value) -> Result<Option<EventSpec>> {
    let id = canonical_id(value.get("update_id").context("missing update")?, false)
        .context("invalid update")?;
    let Some(message) = value.get("message") else {
        return Ok(None);
    };
    ensure!(
        message.pointer("/chat/type").and_then(Value::as_str) == Some("private")
            && message.pointer("/from/is_bot").and_then(Value::as_bool) == Some(false)
            && message.get("message_thread_id").is_none(),
        "only human private chats are authorized"
    );
    let sender = canonical_id(message.pointer("/from/id").context("missing sender")?, true)
        .context("invalid sender")?;
    let chat = canonical_id(message.pointer("/chat/id").context("missing chat")?, true)
        .context("invalid chat")?;
    ensure!(
        sender == binding.sender_id && chat == binding.sender_id,
        "unauthorized Telegram sender or destination"
    );
    let Some(prompt) = message.get("text").and_then(Value::as_str) else {
        return Ok(None);
    };
    ensure!(
        !prompt.trim().is_empty() && prompt.len() <= 16 * 1024,
        "Telegram text is empty or oversized"
    );
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            binding.id.to_string(),
            &id,
            &sender,
            prompt
        ))?)
    );
    Ok(Some(EventSpec {
        event_id: id,
        session_id: format!("tg-{}", binding.id.simple()),
        sender_id: sender,
        prompt: prompt.into(),
        enabled_tools: vec!["datetime_now".into(), "json_query".into()],
        timeout_secs: 120,
        destination: destination(binding),
        sealed_token: None,
        fingerprint,
    }))
}
fn destination(binding: &TelegramBindingSummary) -> Destination {
    Destination {
        channel: Channel::Telegram,
        installation_id: binding.bot_id.clone(),
        conversation_id: binding.sender_id.clone(),
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
    let Some(runtime) = state.telegram.as_ref() else {
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
    let mut values = request
        .headers()
        .get_all("x-telegram-bot-api-secret-token")
        .iter();
    let valid = values
        .next()
        .is_some_and(|v| v.as_bytes().ct_eq(installation.secret.as_bytes()).into())
        && values.next().is_none();
    if !valid {
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
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    match io(runtime, move || {
        Ok(registry
            .telegram_authorized(id)?
            .is_some_and(|actual| same_binding(&actual, &expected)))
    })
    .await
    {
        Ok(true) => (),
        Ok(false) => return answer(StatusCode::FORBIDDEN, "binding_disabled"),
        Err(_) => return answer(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), MAX_BODY),
    )
    .await
    {
        Ok(Ok(body)) => body,
        _ => return answer(StatusCode::BAD_REQUEST, "invalid_body"),
    };
    let event = match serde_json::from_slice(&body)
        .map_err(anyhow::Error::from)
        .and_then(|value| parse_event(&installation.binding, value))
    {
        Ok(Some(event)) => event,
        Ok(None) => return answer(StatusCode::OK, "ignored"),
        Err(_) => return answer(StatusCode::BAD_REQUEST, "event_not_authorized"),
    };
    let store = installation.store.clone();
    match io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Telegram store poisoned"))?
            .inner
            .accept_channel_event(event, now_ms())
    })
    .await
    {
        Ok(_) => answer(StatusCode::OK, "accepted"),
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
        self.stop.close();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        matches!(tokio::time::timeout(grace, &mut task).await, Ok(Ok(())))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.close();
    }
}
pub(super) fn start(state: Arc<State>) -> Worker {
    let stop = Stop::new();
    let task = state
        .telegram
        .is_some()
        .then(|| tokio::spawn(coordinate(state, stop.clone())));
    Worker { stop, task }
}
async fn coordinate(state: Arc<State>, stop: Stop) {
    let runtime = state.telegram.as_ref().unwrap().clone();
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
async fn finish(
    state: &State,
    runtime: &Runtime,
    binding: &TelegramBindingSummary,
    request_id: Uuid,
    known: bool,
) {
    let registry = state.registry.clone();
    let user = binding.user_id;
    if let Err(error) = io(runtime, move || {
        registry.finish_write(user, request_id, known)
    })
    .await
    {
        tracing::error!(
            code = finish_error_code(&error),
            request_id = %request_id,
            user_id = %user,
            known,
            "Telegram write hold requires administrator review"
        );
    }
}
async fn process(state: Arc<State>, stop: Stop, installation: Arc<Installation>) {
    let runtime = state.telegram.as_ref().unwrap();
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
            .map_err(|_| anyhow::anyhow!("Telegram store poisoned"))?
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
        "telegram_send"
    } else {
        "telegram_execute"
    };
    let request_id = Uuid::now_v7();
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    let authorized = io(runtime, move || {
        stop.admit(|| {
            registry.admit_telegram(expected.id, request_id, operation, &expected.id.to_string())
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
    let runtime = state.telegram.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Telegram store poisoned"))?
            .inner
            .claim_channel_event_recorded(now_ms(), &request_id.to_string())
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
            .map_err(|_| anyhow::anyhow!("Telegram store poisoned"))?
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
            && event.spec.session_id == format!("tg-{}", installation.binding.id.simple())
            && event.spec.sender_id == installation.binding.sender_id
            && event.spec.enabled_tools == ["datetime_now", "json_query"]
            && event.spec.timeout_secs == 120
            && event.spec.sealed_token.is_none(),
        "channel authorization mismatch"
    );
    let backend = &state.backends[&installation.binding.backend_id];
    let receipt=tokio::time::timeout(BACKEND_TIMEOUT,async {
        let response=state.client.post(backend.url.join("internal/gateway/channel/chat")?).header(header::AUTHORIZATION,backend.token.clone()).timeout(BACKEND_TIMEOUT).json(&json!({"request_id":request_id.to_string(),"binding_id":installation.binding.id.to_string(),"prompt":event.spec.prompt})).send().await?;
        ensure!(json_response(&response),"backend channel request failed");
        let bytes=read_response(response,MAX_REPLY).await.map_err(|()|anyhow::anyhow!("invalid channel response"))?;
        decode_object::<ChatReceipt>(&bytes)
    }).await.context("backend channel deadline")??;
    let reply = &receipt.response;
    ensure!(
        receipt.protocol == 1
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
    outbound::split_text_for(Channel::Telegram, &reply.message.content)
}
async fn send(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.telegram.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Telegram store poisoned"))?
            .inner
            .claim_channel_delivery_recorded(now_ms(), &request_id.to_string())
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
            .map_err(|_| anyhow::anyhow!("Telegram store poisoned"))?
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
        .context("stop gateway before inspecting or reconciling Telegram state")?;
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
        .list_telegram_bindings()?
        .into_iter()
        .find(|b| b.id == binding_id)
        .context("Telegram binding not found")?;
    registry.recover_writes()?;
    let mut store = TelegramStore::open(&config.registry_path, &binding)?;
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
#[cfg(test)]
pub(super) fn review_guard(
    config: &Config,
    registry: &Registry,
    user: Uuid,
) -> Result<Option<File>> {
    super::channel_review_guard(config, registry, user)
}
pub(super) fn review_pending(config: &Config, registry: &Registry, user: Uuid) -> Result<()> {
    let Some(binding) = registry
        .list_telegram_bindings()?
        .into_iter()
        .find(|b| b.user_id == user)
    else {
        return Ok(());
    };
    let store = TelegramStore::open(&config.registry_path, &binding)?;
    super::review_queue(&store.inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn finish_error_codes_are_static_and_preserve_typed_cause_precedence() {
        let secret = "provider-token SQL SELECT private_note FROM /private/user/registry.sqlite3";
        let mut cases = Vec::new();
        for code in [
            rusqlite::ffi::SQLITE_BUSY,
            rusqlite::ffi::SQLITE_BUSY_SNAPSHOT,
            rusqlite::ffi::SQLITE_LOCKED,
            rusqlite::ffi::SQLITE_LOCKED_SHAREDCACHE,
        ] {
            cases.push((
                anyhow::Error::from(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(code),
                    Some(secret.into()),
                ))
                .context(IO_TASK_FAILED),
                "registry_busy",
            ));
        }
        cases.extend([
            (
                anyhow::Error::from(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
                    Some(secret.into()),
                ))
                .context(secret),
                "registry_storage",
            ),
            (
                anyhow::Error::from(rusqlite::Error::InvalidQuery).context(secret),
                "registry_storage",
            ),
            (
                anyhow::Error::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    secret,
                ))
                .context(IO_CAPACITY_BUSY),
                "registry_io",
            ),
            (anyhow::anyhow!(secret), "registry_validation"),
            (
                anyhow::anyhow!("{IO_CAPACITY_BUSY}: {secret}"),
                "registry_validation",
            ),
            (anyhow::anyhow!(secret).context(IO_CAPACITY_BUSY), "io_busy"),
            (
                anyhow::anyhow!(secret).context(IO_TASK_FAILED),
                "io_task_failed",
            ),
            (
                anyhow::anyhow!(secret)
                    .context(IO_CAPACITY_BUSY)
                    .context(secret),
                "registry_validation",
            ),
        ]);
        for (error, expected) in cases {
            let code = finish_error_code(&error);
            assert_eq!(code, expected);
            assert!(!code.contains(secret));
            assert!(!code.contains("SELECT"));
            assert!(!code.contains("/private"));
        }
    }

    struct Fixture {
        directory: PathBuf,
        config: Config,
        registry: Registry,
        binding: TelegramBindingSummary,
    }
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("jiaclaw-telegram-runtime-{}", Uuid::new_v4()));
            let path = directory.join("registry.sqlite3");
            let registry = Registry::open(&path).unwrap();
            let user = registry.add_user("alice").unwrap();
            let binding = registry
                .add_telegram_binding(user.user_id, "101", "201")
                .unwrap();
            let config:Config=serde_json::from_value(json!({"registry_path":path,
                "backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":directory.join("backend.token")}]
            })).unwrap();
            Self {
                directory,
                config,
                registry,
                binding,
            }
        }
        fn store(&self) -> TelegramStore {
            TelegramStore::open(&self.config.registry_path, &self.binding).unwrap()
        }
        fn runtime(&self) -> (Arc<State>, Arc<Installation>) {
            let installation = Arc::new(Installation {
                binding: self.binding.clone(),
                token: format!("101:{}", "a".repeat(40)),
                secret: "b".repeat(40),
                api_base: "http://127.0.0.1:9".into(),
                client: OutboundClient::new_with_loopback(true).unwrap(),
                store: Arc::new(Mutex::new(self.store())),
                inbound: Arc::new(Semaphore::new(1)),
            });
            let runtime = Arc::new(Runtime {
                installations: HashMap::from([(self.binding.id, installation.clone())]),
                inbound: Arc::new(Semaphore::new(8)),
                io: Arc::new(Semaphore::new(8)),
            });
            let state = Arc::new(State {
                registry: self.registry.clone(),
                backends: HashMap::new(),
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                permits: Arc::new(Semaphore::new(1)),
                timeout: Duration::from_secs(180),
                control: Arc::new(Semaphore::new(1)),
                streams: Arc::new(tokio::sync::Semaphore::new(4)),
                scheduled_jobs: false,
                tracked_turns: false,
                admission: crate::gateway::scheduler::Stop::new(),
                reserved_turns: std::sync::atomic::AtomicUsize::new(0),
                telegram: Some(runtime),
                slack: None,
                discord: None,
                feishu: None,
                wecom: None,
            });
            (state, installation)
        }
        fn queue(&self, store: &mut TelegramStore) -> String {
            let event = parse_event(&self.binding, event()).unwrap().unwrap();
            let accepted = store.inner.accept_channel_event(event, now_ms()).unwrap();
            let claimed = store
                .inner
                .claim_channel_event_recorded(now_ms(), &Uuid::now_v7().to_string())
                .unwrap()
                .unwrap();
            assert_eq!(claimed.id, accepted.id);
            assert!(store
                .inner
                .complete_channel_event(
                    &claimed.id,
                    None,
                    "completed",
                    vec!["reply".into()],
                    None,
                    now_ms()
                )
                .unwrap());
            accepted.id
        }
        fn secret(&self, name: &str, value: &[u8]) -> PathBuf {
            let path = self.directory.join(name);
            std::fs::write(&path, value).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn event() -> Value {
        json!({"update_id":1,"message":{"chat":{"id":201,"type":"private"},"from":{"id":201,"is_bot":false},"text":"hello"}})
    }

    #[test]
    fn parsing_fixes_sender_destination_tools_and_session_and_does_not_trust_injected_fields() {
        let fixture = Fixture::new();
        let mut value = event();
        value["message"]["session_id"] = json!("other-user");
        value["message"]["enabled_tools"] = json!(["shell_exec"]);
        value["message"]["destination"] = json!({"chat_id":999});
        value["message"]["timeout_secs"] = json!(3600);
        let parsed = parse_event(&fixture.binding, value).unwrap().unwrap();
        assert_eq!(parsed.sender_id, "201");
        assert_eq!(parsed.destination, destination(&fixture.binding));
        assert_eq!(
            parsed.session_id,
            format!("tg-{}", fixture.binding.id.simple())
        );
        assert_eq!(parsed.enabled_tools, ["datetime_now", "json_query"]);
        assert_eq!(parsed.timeout_secs, 120);
        assert!(parsed.sealed_token.is_none());
        assert_eq!(
            parsed.fingerprint,
            parse_event(&fixture.binding, event())
                .unwrap()
                .unwrap()
                .fingerprint
        );
        for (pointer, replacement) in [
            ("/update_id", json!(-1)),
            ("/update_id", json!("1")),
            ("/message/chat/id", json!(999)),
            ("/message/from/id", json!(999)),
            ("/message/from/id", json!("201")),
            ("/message/from/is_bot", json!(true)),
            ("/message/chat/type", json!("group")),
            ("/message/text", json!("   ")),
            ("/message/text", json!("x".repeat(16 * 1024 + 1))),
        ] {
            let mut value = event();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(parse_event(&fixture.binding, value).is_err(), "{pointer}");
        }
        let mut value = event();
        value["message"]["message_thread_id"] = json!(1);
        assert!(parse_event(&fixture.binding, value).is_err());
        assert!(
            parse_event(&fixture.binding, json!({"update_id":2,"edited_message":{}}))
                .unwrap()
                .is_none()
        );
        let mut value = event();
        value["message"].as_object_mut().unwrap().remove("text");
        assert!(parse_event(&fixture.binding, value).unwrap().is_none());
    }

    #[test]
    fn credential_files_are_bounded_private_and_do_not_echo_their_contents() {
        let fixture = Fixture::new();
        let valid = fixture.secret("valid", format!("{}\r\n", "q".repeat(40)).as_bytes());
        assert_eq!(secret_file(&valid).unwrap(), "q".repeat(40));
        for (name, bytes) in [
            ("short", vec![b'q'; 31]),
            ("oversized", vec![b'q'; 4099]),
            (
                "whitespace",
                format!("{} secret", "q".repeat(40)).into_bytes(),
            ),
            ("binary", vec![0xff; 40]),
        ] {
            let path = fixture.secret(name, &bytes);
            let error = secret_file(&path).unwrap_err().to_string();
            assert!(!error.contains(&"q".repeat(40)));
        }
        assert!(secret_file(&fixture.directory).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let link = fixture.directory.join("link");
            symlink(&valid, &link).unwrap();
            assert!(secret_file(&link).is_err());
            let hard = fixture.directory.join("hard");
            std::fs::hard_link(&valid, &hard).unwrap();
            assert!(secret_file(&valid).is_err());
            std::fs::remove_file(hard).unwrap();
            std::fs::set_permissions(&valid, std::fs::Permissions::from_mode(0o640)).unwrap();
            assert_eq!(secret_file(&valid).unwrap(), "q".repeat(40));
            std::fs::set_permissions(&valid, std::fs::Permissions::from_mode(0o604)).unwrap();
            assert!(secret_file(&valid).is_err());
            let fifo = fixture.directory.join("fifo");
            assert!(std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success());
            assert!(secret_file(&fifo).is_err());
        }
    }

    #[tokio::test]
    async fn configuration_rejects_reused_secrets_before_any_backend_request_or_store_creation() {
        let mut fixture = Fixture::new();
        let token = format!("101:{}", "a".repeat(40));
        let webhook = "b".repeat(40);
        fixture
            .config
            .telegram
            .push(super::super::config::TelegramConfig {
                binding_id: fixture.binding.id.to_string(),
                bot_token_file: fixture.secret("bot", token.as_bytes()),
                webhook_secret_file: fixture.secret("webhook", webhook.as_bytes()),
                api_base: "https://api.telegram.org".into(),
                allow_loopback: false,
            });
        let backends = HashMap::from([(
            "alice".into(),
            Backend {
                url: reqwest::Url::parse("http://127.0.0.1:9").unwrap(),
                token: header::HeaderValue::from_static(
                    "Bearer synthetic-backend-token-for-tests-only",
                ),
                permit: Arc::new(Semaphore::new(1)),
                control: Arc::new(Semaphore::new(1)),
                streams: Arc::new(tokio::sync::Semaphore::new(4)),
            },
        )]);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for reused in [token, webhook] {
            let error = configure(
                &fixture.config,
                &fixture.registry,
                &client,
                &backends,
                &HashSet::from([reused.clone()]),
            )
            .await
            .err()
            .unwrap();
            assert!(error.to_string().contains("credentials must be distinct"));
            assert!(!format!("{error:#}").contains(&reused));
            assert!(!fixture.directory.join("telegram").exists());
        }
    }

    #[tokio::test]
    async fn rate_limits_use_absolute_deadlines_and_the_fifth_attempt_keeps_a_review_hold() {
        let fixture = Fixture::new();
        let (state, installation) = fixture.runtime();
        fixture.queue(&mut installation.store.lock().unwrap());
        let runtime = state.telegram.as_ref().unwrap();
        let mut claim_time = now_ms();
        for attempt in 1..=5 {
            let request = Uuid::now_v7();
            fixture
                .registry
                .admit_telegram(
                    fixture.binding.id,
                    request,
                    "telegram_send",
                    &fixture.binding.id.to_string(),
                )
                .unwrap();
            let delivery = installation
                .store
                .lock()
                .unwrap()
                .inner
                .claim_channel_delivery_recorded(claim_time, &request.to_string())
                .unwrap()
                .unwrap();
            assert_eq!(delivery.attempts, attempt);
            let id = delivery.id.clone();
            let before = now_ms();
            let known = settle(
                runtime,
                &installation,
                delivery,
                DeliveryOutcome::RateLimited {
                    retry_after_ms: 1000,
                },
            )
            .await;
            let after = now_ms();
            assert_eq!(known, attempt < 5);
            let stored = installation
                .store
                .lock()
                .unwrap()
                .inner
                .get_channel_delivery(&id)
                .unwrap()
                .unwrap();
            assert!((before + 1000..=after + 1000).contains(&stored.next_attempt_ms));
            assert_eq!(
                stored.state,
                if attempt < 5 {
                    "retry_wait"
                } else {
                    "permanent_failed"
                }
            );
            assert!(installation
                .store
                .lock()
                .unwrap()
                .inner
                .claim_channel_delivery_recorded(before, &Uuid::now_v7().to_string())
                .unwrap()
                .is_none());
            finish(&state, runtime, &fixture.binding, request, known).await;
            let hold = fixture.registry.list().unwrap().remove(0).hold;
            if attempt < 5 {
                assert!(hold.is_none());
            } else {
                assert_eq!(hold.unwrap().state, "needs_review");
            }
            // Advance only the injected claim clock beyond both the recorded
            // retry deadline and ordinary installation pacing; no DB state edits.
            claim_time = claim_time.max(stored.next_attempt_ms) + 60_000;
        }
        assert!(fixture
            .registry
            .admit_telegram(
                fixture.binding.id,
                Uuid::now_v7(),
                "telegram_execute",
                &fixture.binding.id.to_string()
            )
            .is_err());
    }

    #[tokio::test]
    async fn unknown_and_invalid_retry_delays_never_release_the_registry_hold() {
        for outcome in [
            DeliveryOutcome::Unknown {
                code: "fixture_unknown",
            },
            DeliveryOutcome::RateLimited { retry_after_ms: -1 },
            DeliveryOutcome::RateLimited {
                retry_after_ms: i64::MAX,
            },
        ] {
            let fixture = Fixture::new();
            let (state, installation) = fixture.runtime();
            fixture.queue(&mut installation.store.lock().unwrap());
            let request = Uuid::now_v7();
            fixture
                .registry
                .admit_telegram(
                    fixture.binding.id,
                    request,
                    "telegram_send",
                    &fixture.binding.id.to_string(),
                )
                .unwrap();
            let delivery = installation
                .store
                .lock()
                .unwrap()
                .inner
                .claim_channel_delivery_recorded(now_ms(), &request.to_string())
                .unwrap()
                .unwrap();
            let id = delivery.id.clone();
            let unknown = matches!(&outcome, DeliveryOutcome::Unknown { .. });
            let runtime = state.telegram.as_ref().unwrap();
            let known = settle(runtime, &installation, delivery, outcome).await;
            assert!(!known);
            finish(&state, runtime, &fixture.binding, request, known).await;
            let stored = installation
                .store
                .lock()
                .unwrap()
                .inner
                .get_channel_delivery(&id)
                .unwrap()
                .unwrap();
            assert_eq!(stored.state, if unknown { "unknown" } else { "submitting" });
            assert_eq!(
                fixture
                    .registry
                    .list()
                    .unwrap()
                    .remove(0)
                    .hold
                    .unwrap()
                    .state,
                "needs_review"
            );
            assert!(fixture
                .registry
                .admit_telegram(
                    fixture.binding.id,
                    Uuid::now_v7(),
                    "telegram_send",
                    &fixture.binding.id.to_string()
                )
                .is_err());
        }
    }

    #[test]
    fn offline_review_requires_stopped_gateway_and_reconciled_events_and_holds_restart_lock() {
        let fixture = Fixture::new();
        let request = Uuid::now_v7();
        fixture
            .registry
            .admit_telegram(
                fixture.binding.id,
                request,
                "telegram_execute",
                &fixture.binding.id.to_string(),
            )
            .unwrap();
        let event_id = {
            let mut store = fixture.store();
            store
                .inner
                .accept_channel_event(
                    parse_event(&fixture.binding, event()).unwrap().unwrap(),
                    now_ms(),
                )
                .unwrap();
            store
                .inner
                .claim_channel_event_recorded(now_ms(), &request.to_string())
                .unwrap()
                .unwrap()
                .id
        };
        let live = stopped(&fixture.config).unwrap();
        assert!(review_guard(&fixture.config, &fixture.registry, fixture.binding.user_id).is_err());
        assert!(admin(
            &fixture.config,
            fixture.binding.id,
            AdminAction::Inspect {
                kind: "events".into(),
                event: None,
                limit: 5,
                offset: 0
            }
        )
        .is_err());
        assert_eq!(
            fixture
                .registry
                .list()
                .unwrap()
                .remove(0)
                .hold
                .unwrap()
                .state,
            "in_flight"
        );
        drop(live);
        assert!(review_guard(&fixture.config, &fixture.registry, fixture.binding.user_id).is_err());
        assert_eq!(
            fixture
                .registry
                .list()
                .unwrap()
                .remove(0)
                .hold
                .unwrap()
                .state,
            "needs_review"
        );
        admin(
            &fixture.config,
            fixture.binding.id,
            AdminAction::Cancel { event: event_id },
        )
        .unwrap();
        let guard = review_guard(&fixture.config, &fixture.registry, fixture.binding.user_id)
            .unwrap()
            .unwrap();
        assert!(
            stopped(&fixture.config).is_err(),
            "review guard must keep the process lock through registry clearing"
        );
        fixture
            .registry
            .clear_review(
                fixture.binding.user_id,
                "Reconciled stopped backend and cancelled event",
            )
            .unwrap();
        assert!(fixture.registry.list().unwrap().remove(0).hold.is_none());
        drop(guard);
        assert!(admin(
            &fixture.config,
            fixture.binding.id,
            AdminAction::Inspect {
                kind: "operations".into(),
                event: None,
                limit: 1,
                offset: 16000
            }
        )
        .is_ok());
        assert!(admin(
            &fixture.config,
            fixture.binding.id,
            AdminAction::Inspect {
                kind: "operations".into(),
                event: None,
                limit: 1,
                offset: 16001
            }
        )
        .is_err());
        assert!(admin(
            &fixture.config,
            fixture.binding.id,
            AdminAction::Inspect {
                kind: "events".into(),
                event: None,
                limit: 1,
                offset: 10001
            }
        )
        .is_err());
    }

    #[test]
    fn backend_contract_requires_objects_and_preserves_duplicate_field_rejection() {
        let status = json!({"protocol":1,"backend_id":"alice","max_run_seconds":120,"tools":["datetime_now","json_query"],"mode":"gateway"});
        assert!(decode_object::<BackendStatus>(&serde_json::to_vec(&status).unwrap()).is_ok());
        let duplicate=br#"{"protocol":1,"backend_id":"alice","backend_id":"other","max_run_seconds":120,"tools":[],"mode":"gateway"}"#;
        assert!(decode_object::<BackendStatus>(duplicate).is_err());
        assert!(decode_object::<BackendStatus>(br#"[1,"alice",120,[],"gateway"]"#).is_err());
        let receipt = json!({"protocol":1,"backend_id":"alice","request_id":Uuid::now_v7().to_string(),
            "binding_id":Uuid::new_v4().to_string(),"response":{"message":{"role":"assistant","content":"ok"},"status":"completed"}});
        assert!(decode_object::<ChatReceipt>(&serde_json::to_vec(&receipt).unwrap()).is_ok());
        let array = json!([
            receipt["protocol"],
            receipt["backend_id"],
            receipt["request_id"],
            receipt["binding_id"],
            receipt["response"]
        ]);
        assert!(decode_object::<ChatReceipt>(&serde_json::to_vec(&array).unwrap()).is_err());
        let duplicate =
            serde_json::to_string(&receipt)
                .unwrap()
                .replacen("{", "{\"protocol\":1,", 1);
        assert!(decode_object::<ChatReceipt>(duplicate.as_bytes()).is_err());
        for invalid in [
            b"null".as_slice(),
            b"[]",
            b"\"secret-response\"",
            b"{invalid secret-response",
        ] {
            let error = decode_object::<ChatReceipt>(invalid).err().unwrap();
            assert!(!format!("{error:#}").contains("secret-response"));
        }
    }

    #[tokio::test]
    async fn actual_handshake_rejects_a_positional_array_before_creating_a_channel_store() {
        let mut fixture = Fixture::new();
        fixture
            .config
            .telegram
            .push(super::super::config::TelegramConfig {
                binding_id: fixture.binding.id.to_string(),
                bot_token_file: fixture.secret("bot", format!("101:{}", "a".repeat(40)).as_bytes()),
                webhook_secret_file: fixture.secret("webhook", "b".repeat(40).as_bytes()),
                api_base: "https://api.telegram.org".into(),
                allow_loopback: false,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(|| async {
            (
                [(header::CONTENT_TYPE, "application/json")],
                r#"[1,"alice",120,["datetime_now","json_query"],"gateway"]"#,
            )
        });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let backend = Backend {
            url: reqwest::Url::parse(&format!("http://{address}")).unwrap(),
            token: header::HeaderValue::from_static(
                "Bearer synthetic-backend-token-for-tests-only",
            ),
            permit: Arc::new(Semaphore::new(1)),
            control: Arc::new(Semaphore::new(1)),
            streams: Arc::new(tokio::sync::Semaphore::new(4)),
        };
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let result = configure(
            &fixture.config,
            &fixture.registry,
            &client,
            &HashMap::from([("alice".into(), backend)]),
            &HashSet::new(),
        )
        .await;
        server.abort();
        assert!(result.is_err());
        assert!(!fixture.directory.join("telegram").exists());
    }
}
