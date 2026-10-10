// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Dedicated tenant Wecom ingress; gateway-owned, conservative single-attempt delivery.
use super::{
    config::Config,
    proxy::read_response,
    registry::{Registry, WecomBindingSummary},
    scheduler::Stop,
    wecom_store::WecomStore,
    Backend, State,
};
use crate::{
    channel_store::{ChannelDelivery, ChannelEvent},
    channel_types::{Channel, Destination, EventSpec},
    outbound::{self, DeliveryOutcome},
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
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};
use uuid::Uuid;

const MAX_BODY: usize = 128 * 1024;
const MAX_PROMPT: usize = 16 * 1024;
const MAX_REPLY: usize = 2 * 1024 * 1024;
const BACKEND_TIMEOUT: Duration = Duration::from_secs(150);
const IO_CAPACITY_BUSY: &str = "Wecom I/O capacity busy";
const IO_TASK_FAILED: &str = "Wecom I/O task failed";
struct Installation {
    binding: WecomBindingSummary,
    callback: crate::wecom::Callback,
    client: crate::wecom_outbound::WeComSender,
    store: Arc<Mutex<WecomStore>>,
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
fn secret_file(path: &std::path::Path) -> Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 4098,
        "Wecom secret must be a bounded ordinary file"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        ensure!(
            metadata.nlink() == 1
                && metadata.permissions().mode() & 0o777 == 0o600
                && metadata.uid() == rustix::process::geteuid().as_raw(),
            "Wecom secret requires owner-only 0600 and one link"
        );
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && opened.len() <= 4098,
        "invalid Wecom secret descriptor"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            opened.dev() == metadata.dev()
                && opened.ino() == metadata.ino()
                && opened.nlink() == 1
                && opened.permissions().mode() & 0o777 == 0o600
                && opened.uid() == rustix::process::geteuid().as_raw(),
            "Wecom secret changed during open"
        );
    }
    let mut value = String::new();
    file.take(4099).read_to_string(&mut value)?;
    let value = value.trim_end_matches(['\r', '\n']);
    ensure!(
        (1..=4096).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_graphic()),
        "Wecom secret requires 1..4096 visible ASCII bytes"
    );
    Ok(value.into())
}
fn same_binding(a: &WecomBindingSummary, b: &WecomBindingSummary) -> bool {
    a.id == b.id
        && a.user_id == b.user_id
        && a.backend_id == b.backend_id
        && a.corp_id == b.corp_id
        && a.agent_id == b.agent_id
        && a.human_user_id == b.human_user_id
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
    corp_id: String,
    agent_id: String,
    human_user_id: String,
}
fn binding_identity(binding: &WecomBindingSummary) -> Value {
    json!({"protocol":5,"binding_id":binding.id.to_string(),"user_id":binding.user_id.to_string(),
        "backend_id":binding.backend_id,"corp_id":binding.corp_id,
        "agent_id":binding.agent_id.to_string(),"human_user_id":binding.human_user_id})
}
pub(super) async fn configure(
    config: &Config,
    registry: &Registry,
    client: &reqwest::Client,
    backends: &HashMap<String, Backend>,
    backend_tokens: &HashSet<String>,
) -> Result<Option<Arc<Runtime>>> {
    if config.wecom.is_empty() {
        return Ok(None);
    }
    let bindings = registry.list_wecom_bindings()?;
    let mut installations = HashMap::new();
    let mut secrets = backend_tokens.clone();
    for path in config
        .telegram
        .iter()
        .flat_map(|e| [&e.bot_token_file, &e.webhook_secret_file])
        .chain(
            config
                .slack
                .iter()
                .flat_map(|e| [&e.bot_token_file, &e.signing_secret_file]),
        )
        .chain(
            config
                .discord
                .iter()
                .flat_map(|e| [&e.bot_token_file, &e.state_key_file]),
        )
        .chain(config.feishu.iter().flat_map(|e| {
            [
                &e.app_secret_file,
                &e.encrypt_key_file,
                &e.verification_token_file,
            ]
        }))
    {
        secrets.insert(secret_file(path)?);
    }
    for entry in &config.wecom {
        let id = Uuid::parse_str(&entry.binding_id)?;
        let binding = bindings
            .iter()
            .find(|b| b.id == id)
            .context("configured Wecom binding does not exist")?
            .clone();
        ensure!(
            binding.enabled,
            "configured Wecom binding is revoked; remove its runtime configuration"
        );
        let backend = backends
            .get(&binding.backend_id)
            .context("Wecom backend is not configured")?;
        let app_secret = secret_file(&entry.app_secret_file)?;
        let callback_token = secret_file(&entry.callback_token_file)?;
        let encoding_aes_key = secret_file(&entry.encoding_aes_key_file)?;
        for secret in [&app_secret, &callback_token, &encoding_aes_key] {
            ensure!(
                secrets.insert(secret.clone()),
                "channel and backend credentials must be distinct"
            );
        }
        let installation_id = format!("{}:{}", binding.corp_id, binding.agent_id);
        let callback =
            crate::wecom::Callback::new(&installation_id, callback_token, &encoding_aes_key)?;
        let sender = crate::wecom_outbound::WeComSender::new(
            &installation_id,
            app_secret,
            entry.api_base.clone(),
            entry.allow_loopback,
        )?;
        sender
            .verify_installation(&[binding.human_user_id.clone()])
            .await?;
        let db = WecomStore::open(&config.registry_path, &binding)?;
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            let response = client
                .get(backend.url.join("internal/channels/wecom/status")?)
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
                status.protocol == 5
                    && status.backend_id == binding.backend_id
                    && status.mode == "gateway"
                    && status.max_run_seconds == 120
                    && status.tools == ["datetime_now", "json_query"],
                "gateway channel contract mismatch"
            );
            let identity = binding_identity(&binding);
            let response = client
                .post(backend.url.join("internal/channels/wecom-binding")?)
                .header(header::AUTHORIZATION, backend.token.clone())
                .timeout(Duration::from_secs(5))
                .json(&identity)
                .send()
                .await?;
            ensure!(json_response(&response), "backend Wecom binding rejected");
            let bytes = read_response(response, 4096)
                .await
                .map_err(|()| anyhow::anyhow!("invalid Wecom binding response"))?;
            let actual: BackendIdentity = decode_object(&bytes)?;
            ensure!(
                serde_json::to_value(actual)? == identity,
                "backend Wecom owner mismatch"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("gateway channel handshake timed out")?;
        // Do not expose reqwest errors containing a configured backend URL/token.
        ensure!(status.is_ok(), "gateway channel handshake failed");
        installations.insert(
            id,
            Arc::new(Installation {
                binding,
                callback,
                client: sender,
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
fn destination(binding: &WecomBindingSummary) -> Destination {
    Destination {
        channel: Channel::Wecom,
        installation_id: format!("{}:{}", binding.corp_id, binding.agent_id),
        conversation_id: binding.human_user_id.clone(),
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
        Duration::from_millis(900),
        ingress_with_budget(state, id, request),
    )
    .await
    .unwrap_or_else(|_| answer(StatusCode::SERVICE_UNAVAILABLE, "ingress_deadline"))
}
async fn ingress_with_budget(state: Arc<State>, id: String, request: Request) -> Response {
    let Some(runtime) = state.wecom.as_ref() else {
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
    let method = request.method().clone();
    if method != axum::http::Method::GET && method != axum::http::Method::POST {
        return answer(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    }
    let Some(query) = request
        .uri()
        .query()
        .filter(|query| query.len() <= 8192)
        .map(str::to_owned)
    else {
        return answer(StatusCode::BAD_REQUEST, "invalid_query");
    };
    let Ok(_global) = runtime.inbound.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let Ok(_local) = installation.inbound.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let maximum = if method == axum::http::Method::GET {
        0
    } else {
        MAX_BODY
    };
    let body = match tokio::time::timeout(Duration::from_millis(650), async {
        let mut stream = request.into_body().into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;
            if bytes.len().saturating_add(chunk.len()) > maximum {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(status)) => return answer(status, "invalid_body"),
        Err(_) => return answer(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
    };
    // Signature/decryption/inner and tail Corp/Agent checks precede all SQL.
    let challenge = if method == axum::http::Method::GET {
        match installation
            .callback
            .challenge(&query, crate::unix_now_secs())
        {
            Ok(value) => Some(value),
            Err(_) => return answer(StatusCode::UNAUTHORIZED, "invalid_callback"),
        }
    } else {
        None
    };
    let callback = if challenge.is_none() {
        match installation
            .callback
            .parse_event(&query, &body, crate::unix_now_secs())
        {
            Ok(value) => Some(value),
            Err(_) => return answer(StatusCode::UNAUTHORIZED, "invalid_callback"),
        }
    } else {
        None
    };
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    match io(runtime, move || {
        Ok(registry
            .wecom_authorized(id)?
            .is_some_and(|actual| same_binding(&actual, &expected)))
    })
    .await
    {
        Ok(true) => (),
        Ok(false) => return answer(StatusCode::FORBIDDEN, "binding_disabled"),
        Err(_) => return answer(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    }
    if let Some(challenge) = challenge {
        return challenge.into_response();
    }
    let event = match callback.expect("GET challenge returned above") {
        crate::wecom::Inbound::Ignored => return StatusCode::OK.into_response(),
        crate::wecom::Inbound::Message {
            event_id,
            sender_id,
            text,
        } => {
            if sender_id != installation.binding.human_user_id {
                return answer(StatusCode::FORBIDDEN, "event_not_authorized");
            }
            if text.len() > MAX_PROMPT {
                return answer(StatusCode::PAYLOAD_TOO_LARGE, "prompt_too_large");
            }
            let fingerprint = format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        installation.binding.id.to_string(),
                        &event_id,
                        &sender_id,
                        &text
                    ))
                    .expect("serialize strings")
                )
            );
            EventSpec {
                event_id,
                session_id: format!("wecom:{}", installation.binding.id),
                sender_id,
                prompt: text,
                enabled_tools: vec!["datetime_now".into(), "json_query".into()],
                timeout_secs: 120,
                destination: destination(&installation.binding),
                sealed_token: None,
                fingerprint,
            }
        }
    };
    let store = installation.store.clone();
    match io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Wecom store poisoned"))?
            .inner
            .accept_wecom_event_recorded(event, now_ms())
    })
    .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e)
            if e.downcast_ref::<crate::channel_store::ChannelConflict>()
                .is_some() =>
        {
            answer(StatusCode::CONFLICT, "fingerprint_conflict")
        }
        Err(e)
            if e.downcast_ref::<crate::channel_store::ChannelCapacity>()
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
        .wecom
        .is_some()
        .then(|| tokio::spawn(coordinate(state, stop.clone())));
    Worker { stop, task }
}
async fn coordinate(state: Arc<State>, stop: Stop) {
    let runtime = state.wecom.as_ref().unwrap().clone();
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
    binding: &WecomBindingSummary,
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
            "Wecom write hold requires administrator review"
        );
    }
}
async fn process(state: Arc<State>, stop: Stop, installation: Arc<Installation>) {
    let runtime = state.wecom.as_ref().unwrap();
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
            .map_err(|_| anyhow::anyhow!("Wecom store poisoned"))?
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
        "wecom_send"
    } else {
        "wecom_execute"
    };
    let request_id = Uuid::now_v7();
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    let authorized = io(runtime, move || {
        stop.admit(|| {
            registry.admit_wecom(expected.id, request_id, operation, &expected.id.to_string())
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
    #[serde(deserialize_with = "decode_chat_response")]
    response: jiaclaw_core::ChatResponse,
}
// Preserve original bytes at every typed object boundary. Serde struct
// deserializers otherwise also accept positional arrays and ignore extensions.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireChatResponse {
    message: Box<serde_json::value::RawValue>,
    #[serde(default)]
    tool_calls: Vec<Box<serde_json::value::RawValue>>,
    status: jiaclaw_core::RunStatus,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    routing: Option<Box<serde_json::value::RawValue>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireChatMessage {
    role: jiaclaw_core::MessageRole,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireToolCall {
    tool_name: String,
    arguments: Value,
    result: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireModelSelection {
    purpose: jiaclaw_core::ModelPurpose,
    model: String,
    temperature: f32,
    max_tokens: u32,
}
fn decode_chat_response<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<jiaclaw_core::ChatResponse, D::Error> {
    let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
    let parse = || -> Result<jiaclaw_core::ChatResponse> {
        let wire: WireChatResponse = decode_object(raw.get().as_bytes())?;
        let message: WireChatMessage = decode_object(wire.message.get().as_bytes())?;
        let tool_calls = wire
            .tool_calls
            .into_iter()
            .map(|raw| {
                let wire: WireToolCall = decode_object(raw.get().as_bytes())?;
                Ok(jiaclaw_core::ToolCall {
                    tool_name: wire.tool_name,
                    arguments: wire.arguments,
                    result: wire.result,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let routing = wire
            .routing
            .map(|raw| -> Result<jiaclaw_core::ModelSelection> {
                let wire: WireModelSelection = decode_object(raw.get().as_bytes())?;
                Ok(jiaclaw_core::ModelSelection {
                    purpose: wire.purpose,
                    model: wire.model,
                    temperature: wire.temperature,
                    max_tokens: wire.max_tokens,
                })
            })
            .transpose()?;
        Ok(jiaclaw_core::ChatResponse {
            message: jiaclaw_core::ChatMessage {
                role: message.role,
                content: message.content,
            },
            tool_calls,
            status: wire.status,
            session_id: wire.session_id,
            routing,
        })
    };
    parse().map_err(|_| serde::de::Error::custom("invalid original channel response objects"))
}
fn json_response(response: &reqwest::Response) -> bool {
    let mut values = response.headers().get_all(header::CONTENT_TYPE).iter();
    let valid = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    response.status() == reqwest::StatusCode::OK && valid && values.next().is_none()
}

async fn execute(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.wecom.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Wecom store poisoned"))?
            .inner
            .claim_wecom_event_recorded(now_ms(), &request_id.to_string())
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
    let committed = finish_io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Wecom store poisoned"))?
            .inner
            .complete_wecom_event_recorded(
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
            && event.spec.session_id == format!("wecom:{}", installation.binding.id)
            && event.spec.sender_id == installation.binding.human_user_id
            && event.spec.enabled_tools == ["datetime_now", "json_query"]
            && event.spec.timeout_secs == 120
            && event.spec.sealed_token.is_none()
            && event.spec.prompt.len() <= MAX_PROMPT,
        "channel authorization mismatch"
    );
    let backend = &state.backends[&installation.binding.backend_id];
    let receipt=tokio::time::timeout(BACKEND_TIMEOUT,async {
        let response=state.client.post(backend.url.join("internal/channels/wecom/execute")?).header(header::AUTHORIZATION,backend.token.clone()).timeout(BACKEND_TIMEOUT).json(&json!({"protocol":5,"request_id":request_id.to_string(),"binding_id":installation.binding.id.to_string(),"session_id":event.spec.session_id,"event_id":event.spec.event_id,"prompt":event.spec.prompt})).send().await?;
        ensure!(json_response(&response),"backend channel request failed");
        let bytes=read_response(response,MAX_REPLY).await.map_err(|()|anyhow::anyhow!("invalid channel response"))?;
        decode_object::<ChatReceipt>(&bytes)
    }).await.context("backend channel deadline")??;
    let reply = &receipt.response;
    ensure!(
        receipt.protocol == 5
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
    outbound::split_text_for(Channel::Wecom, &reply.message.content)
}
async fn send(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.wecom.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        let mut store = store
            .lock()
            .map_err(|_| anyhow::anyhow!("Wecom store poisoned"))?;
        let delivery = store
            .inner
            .claim_wecom_delivery_recorded(now_ms(), &request_id.to_string())?;
        if let Some(delivery) = &delivery {
            let reservation = store.inner.get_wecom_reservation(
                &delivery.destination.installation_id,
                &delivery.id,
                delivery.attempts,
            )?;
            ensure!(
                reservation.is_some_and(|r| r.settled_ms.is_none()),
                "recorded WeCom delivery is missing its unsettled quota evidence"
            );
        }
        Ok(delivery)
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
        tokio::time::timeout(
            Duration::from_secs(25),
            installation
                .client
                .send(&delivery.destination, &delivery.text),
        )
        .await
        .unwrap_or(DeliveryOutcome::Unknown {
            code: "send_deadline",
        })
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
        // This sender has no authenticated retry-wait contract. Preserve an
        // unexpected limiter as unknown rather than borrowing another channel's retries.
        DeliveryOutcome::RateLimited { .. } => (
            "unknown",
            None,
            Some("unexpected_rate_limit".into()),
            None,
            false,
        ),
        DeliveryOutcome::Unknown { code } => ("unknown", None, Some(code.into()), None, false),
        DeliveryOutcome::Rejected { code } => {
            ("permanent_failed", None, Some(code.into()), None, false)
        }
    };
    let store = installation.store.clone();
    matches!(
        finish_io(runtime, move || store
            .lock()
            .map_err(|_| anyhow::anyhow!("Wecom store poisoned"))?
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
        .context("stop gateway before inspecting or reconciling Wecom state")?;
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
        .wecom_binding(binding_id)?
        .context("Wecom binding not found")?;
    registry.recover_writes()?;
    let Some(mut store) = WecomStore::open_existing(&config.registry_path, &binding)? else {
        let result = match action {
            AdminAction::Inspect {
                kind,
                event,
                limit,
                offset,
            } => {
                if let Some(id) = &event {
                    checked_uuid(id)?;
                }
                ensure!(
                    (1..=100).contains(&limit)
                        && offset <= if kind == "operations" { 16000 } else { 10000 },
                    "inspection page out of range"
                );
                match kind.as_str() {
                    "events" => json!({"events":[]}),
                    "deliveries" => json!({"deliveries":[]}),
                    "operations" => json!({"operations":[]}),
                    "reservations" => json!({"reservations":[]}),
                    _ => anyhow::bail!("invalid inspection kind"),
                }
            }
            _ => anyhow::bail!("binding has no persisted operation"),
        };
        return Ok(json!({"binding_id":binding_id.to_string(),"result":result}));
    };
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
                "reservations" => {
                    json!({"reservations":store.inner.list_wecom_reservations(Some(&format!("{}:{}",binding.corp_id,binding.agent_id)),limit,offset)?})
                }
                _ => anyhow::bail!(
                    "inspection kind must be events, deliveries, operations or reservations"
                ),
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
                store
                    .inner
                    .purge_wecom_channel_event_recorded(&event, now_ms())?,
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
        .list_wecom_bindings()?
        .into_iter()
        .find(|b| b.user_id == user)
    else {
        return Ok(());
    };
    if let Some(store) = WecomStore::open_existing(&config.registry_path, &binding)? {
        super::review_queue(&store.inner)?;
        ensure!(
            !store.inner.has_unsettled_wecom_reservations(None)?,
            "reconcile all unknown WeCom quota evidence before clearing the hold"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn review_clear_checks_revoked_retained_wecom_queue_under_one_process_lock() {
        let root = std::env::temp_dir().join(format!("jiaclaw-wecom-review-{}", Uuid::new_v4()));
        let path = root.join("registry.sqlite3");
        let registry = Registry::open(&path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let b = registry
            .add_wecom_binding(user.user_id, "wwtestcorp", 1000002, "alice.member")
            .unwrap();
        let config:Config=serde_json::from_value(json!({"registry_path":path,"backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":root.join("backend.token")}]})).unwrap();
        let mut store = WecomStore::open(&path, &b).unwrap();
        let e = store
            .inner
            .accept_channel_event(
                EventSpec {
                    event_id: "123456".into(),
                    session_id: format!("wecom:{}", b.id),
                    sender_id: b.human_user_id.clone(),
                    prompt: "private input".into(),
                    enabled_tools: vec!["datetime_now".into(), "json_query".into()],
                    timeout_secs: 120,
                    destination: destination(&b),
                    sealed_token: None,
                    fingerprint: "a".repeat(64),
                },
                0,
            )
            .unwrap();
        store
            .inner
            .claim_wecom_event_recorded(0, &Uuid::now_v7().to_string())
            .unwrap()
            .unwrap();
        drop(store);
        registry.revoke_wecom_binding(b.id).unwrap();
        assert!(super::super::channel_review_guard(&config, &registry, user.user_id).is_err());
        let mut store = WecomStore::open(&path, &b).unwrap();
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
    #[test]
    fn never_initialized_revoked_binding_can_be_inspected_without_secret_or_state_creation() {
        let root = std::env::temp_dir().join(format!("jiaclaw-wecom-absent-{}", Uuid::new_v4()));
        let path = root.join("registry.sqlite3");
        let registry = Registry::open(&path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let b = registry
            .add_wecom_binding(user.user_id, "wwtestcorp", 1000002, "alice.member")
            .unwrap();
        registry.revoke_wecom_binding(b.id).unwrap();
        let config:Config=serde_json::from_value(json!({"registry_path":path,"backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":root.join("missing.token")}],
            "wecom":[{"binding_id":b.id.to_string(),"app_secret_file":root.join("missing.app"),"encoding_aes_key_file":root.join("missing.encrypt"),"callback_token_file":root.join("missing.verify")}]})).unwrap();
        for kind in ["events", "deliveries", "operations", "reservations"] {
            let value = admin(
                &config,
                b.id,
                AdminAction::Inspect {
                    kind: kind.into(),
                    event: None,
                    limit: 100,
                    offset: 0,
                },
            )
            .unwrap();
            assert!(value["result"][kind].as_array().unwrap().is_empty());
            assert!(admin(
                &config,
                b.id,
                AdminAction::Inspect {
                    kind: kind.into(),
                    event: Some("bad".into()),
                    limit: 100,
                    offset: 0
                }
            )
            .is_err());
        }
        assert!(
            super::super::channel_review_guard(&config, &registry, user.user_id)
                .unwrap()
                .is_some()
        );
        assert!(admin(
            &config,
            b.id,
            AdminAction::Cancel {
                event: Uuid::new_v4().to_string()
            }
        )
        .is_err());
        assert!(!root.join("wecom").exists());
        drop(registry);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn provider_credentials_support_short_tokens_but_refuse_aliases_permissions_and_overflow() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir().join(format!("jiaclaw-wecom-secrets-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("secret");
        for length in [1, 6, 31, 32, 4096] {
            std::fs::write(&path, format!("{}\r\n", "s".repeat(length))).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(secret_file(&path).unwrap(), "s".repeat(length));
        }
        for value in [
            String::new(),
            "s".repeat(4097),
            format!("{} bad", "s".repeat(16)),
            format!("{}\nbad", "s".repeat(16)),
        ] {
            std::fs::write(&path, value).unwrap();
            assert!(secret_file(&path).is_err());
        }
        std::fs::write(&path, "s".repeat(32)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(secret_file(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = root.join("alias");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(secret_file(&path).is_err());
        std::fs::remove_file(&alias).unwrap();
        symlink(&path, &alias).unwrap();
        assert!(secret_file(&alias).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn typed_settlement_diagnostics_never_classify_private_error_strings_as_storage_causes() {
        let busy = anyhow::Error::new(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("private SQL and path".into()),
        ))
        .context("outer private detail");
        assert_eq!(finish_error_code(&busy), "registry_busy");
        let io = anyhow::Error::new(std::io::Error::other("private token path"))
            .context("private outer detail");
        assert_eq!(finish_error_code(&io), "registry_io");
        for message in [
            "private: database is locked",
            "Wecom I/O capacity busy private",
            "SQLITE_BUSY secret",
        ] {
            assert_eq!(
                finish_error_code(&anyhow::anyhow!(message.to_owned())),
                "registry_validation"
            );
        }
        assert_eq!(
            finish_error_code(&anyhow::anyhow!(IO_CAPACITY_BUSY)),
            "io_busy"
        );
        assert_eq!(
            finish_error_code(&anyhow::anyhow!(IO_TASK_FAILED)),
            "io_task_failed"
        );
    }
    #[test]
    fn handshake_requires_one_json_content_type_and_ok_status() {
        let response = |status, types: &[&str]| {
            let mut result = axum::http::Response::builder().status(status);
            for value in types {
                result = result.header(header::CONTENT_TYPE, *value);
            }
            reqwest::Response::from(result.body(Vec::<u8>::new()).unwrap())
        };
        assert!(json_response(&response(
            200,
            &["application/json; charset=utf-8"]
        )));
        for types in [
            &[][..],
            &["text/plain"][..],
            &["application/json", "application/json"][..],
            &["application/json", "text/plain"][..],
        ] {
            assert!(!json_response(&response(200, types)));
        }
        assert!(!json_response(&response(201, &["application/json"])));
    }
    #[test]
    fn original_nested_receipt_objects_reject_arrays_extensions_and_duplicate_fields() {
        let valid = json!({"protocol":5,"backend_id":"alice","request_id":Uuid::now_v7().to_string(),
            "binding_id":Uuid::new_v4().to_string(),"response":{"message":{"role":"assistant","content":"reply"},
            "tool_calls":[{"tool_name":"datetime_now","arguments":{},"result":{"now":"fixture"}}],
            "status":"completed","session_id":"wecom:fixture","routing":{"purpose":"channel",
            "model":"fixture","temperature":0.2,"max_tokens":100}}});
        assert!(decode_object::<ChatReceipt>(&serde_json::to_vec(&valid).unwrap()).is_ok());
        let mut invalid = vec![];
        for (path, value) in [
            (
                "response",
                json!([
                    ["assistant", "reply"],
                    [],
                    "completed",
                    "wecom:fixture",
                    null
                ]),
            ),
            ("message", json!(["assistant", "reply"])),
            ("call", json!(["datetime_now",{}, {"now":"fixture"}])),
            ("routing", json!(["channel", "fixture", 0.2, 100])),
        ] {
            let mut body = valid.clone();
            match path {
                "response" => body["response"] = value,
                "message" => body["response"]["message"] = value,
                "call" => body["response"]["tool_calls"][0] = value,
                _ => body["response"]["routing"] = value,
            }
            invalid.push(serde_json::to_vec(&body).unwrap());
        }
        let bytes = serde_json::to_string(&valid).unwrap();
        for (needle, replacement) in [
            (
                "\"content\":\"reply\"",
                "\"content\":\"reply\",\"content\":\"reply\"",
            ),
            (
                "\"status\":\"completed\"",
                "\"status\":\"completed\",\"status\":\"completed\"",
            ),
            (
                "\"tool_name\":\"datetime_now\"",
                "\"tool_name\":\"datetime_now\",\"tool_name\":\"datetime_now\"",
            ),
            (
                "\"model\":\"fixture\"",
                "\"model\":\"fixture\",\"model\":\"fixture\"",
            ),
            (
                "\"content\":\"reply\"",
                "\"content\":\"reply\",\"unknown\":true",
            ),
        ] {
            assert!(bytes.contains(needle));
            invalid.push(bytes.replace(needle, replacement).into_bytes());
        }
        for body in invalid {
            assert!(decode_object::<ChatReceipt>(&body).is_err());
        }
    }
    #[test]
    fn backend_protocol_rejects_duplicate_fields_unknown_fields_and_positional_arrays() {
        for body in [
            r#"{"protocol":5,"protocol":5,"backend_id":"alice","max_run_seconds":120,"tools":[],"mode":"gateway"}"#,
            r#"[5,"alice",120,[],"gateway"]"#,
            r#"{"protocol":5,"backend_id":"alice","max_run_seconds":120,"tools":[],"mode":"gateway","token":"secret"}"#,
        ] {
            assert!(decode_object::<BackendStatus>(body.as_bytes()).is_err());
        }
    }
}
