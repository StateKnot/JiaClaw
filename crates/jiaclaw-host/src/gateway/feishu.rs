// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Dedicated tenant Feishu ingress; gateway-owned, conservative single-attempt delivery.
use super::{
    config::Config,
    feishu_store::FeishuStore,
    proxy::read_response,
    registry::{FeishuBindingSummary, Registry},
    scheduler::Stop,
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
const IO_CAPACITY_BUSY: &str = "Feishu I/O capacity busy";
const IO_TASK_FAILED: &str = "Feishu I/O task failed";
struct Installation {
    binding: FeishuBindingSummary,
    encrypt_key: String,
    verification_token: String,
    client: crate::feishu_outbound::FeishuSender,
    store: Arc<Mutex<FeishuStore>>,
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
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 1026,
        "Feishu secret must be a bounded ordinary file"
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
            "Feishu secret requires owner-only 0600 and one link"
        );
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && opened.len() <= 1026,
        "invalid Feishu secret descriptor"
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
            "Feishu secret changed during open"
        );
    }
    let mut value = String::new();
    file.take(1027).read_to_string(&mut value)?;
    let value = value.trim_end_matches(['\r', '\n']);
    ensure!(
        (16..=1024).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_graphic()),
        "Feishu secret requires 16..1024 visible ASCII bytes"
    );
    Ok(value.into())
}
fn same_binding(a: &FeishuBindingSummary, b: &FeishuBindingSummary) -> bool {
    a.id == b.id
        && a.user_id == b.user_id
        && a.backend_id == b.backend_id
        && a.human_open_id == b.human_open_id
        && a.tenant_key == b.tenant_key
        && a.app_id == b.app_id
        && a.bot_open_id == b.bot_open_id
        && a.chat_id == b.chat_id
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
    tenant_key: String,
    app_id: String,
    bot_open_id: String,
    human_open_id: String,
    chat_id: String,
}
fn binding_identity(binding: &FeishuBindingSummary) -> Value {
    json!({"protocol":4,"binding_id":binding.id.to_string(),"user_id":binding.user_id.to_string(),
        "backend_id":binding.backend_id,"app_id":binding.app_id,"tenant_key":binding.tenant_key,
        "bot_open_id":binding.bot_open_id,"human_open_id":binding.human_open_id,"chat_id":binding.chat_id})
}
pub(super) async fn configure(
    config: &Config,
    registry: &Registry,
    client: &reqwest::Client,
    backends: &HashMap<String, Backend>,
    backend_tokens: &HashSet<String>,
) -> Result<Option<Arc<Runtime>>> {
    if config.feishu.is_empty() {
        return Ok(None);
    }
    let bindings = registry.list_feishu_bindings()?;
    let mut installations = HashMap::new();
    let mut secrets = backend_tokens.clone();
    for entry in &config.telegram {
        secrets.insert(super::telegram::secret_file(&entry.bot_token_file)?);
        secrets.insert(super::telegram::secret_file(&entry.webhook_secret_file)?);
    }
    for entry in &config.slack {
        secrets.insert(super::telegram::secret_file(&entry.bot_token_file)?);
        secrets.insert(super::telegram::secret_file(&entry.signing_secret_file)?);
    }
    for entry in &config.discord {
        secrets.insert(super::telegram::secret_file(&entry.bot_token_file)?);
        secrets.insert(super::telegram::secret_file(&entry.state_key_file)?);
    }
    for entry in &config.feishu {
        let id = Uuid::parse_str(&entry.binding_id)?;
        let binding = bindings
            .iter()
            .find(|b| b.id == id)
            .context("configured Feishu binding does not exist")?
            .clone();
        ensure!(
            binding.enabled,
            "configured Feishu binding is revoked; remove its runtime configuration"
        );
        let backend = backends
            .get(&binding.backend_id)
            .context("Feishu backend is not configured")?;
        let app_secret = secret_file(&entry.app_secret_file)?;
        let encrypt_key = secret_file(&entry.encrypt_key_file)?;
        let verification_token = secret_file(&entry.verification_token_file)?;
        for secret in [&app_secret, &encrypt_key, &verification_token] {
            ensure!(
                (16..=1024).contains(&secret.len()) && secret.bytes().all(|b| b.is_ascii_graphic()),
                "Feishu secrets must be bounded private ASCII credentials"
            );
            ensure!(
                secrets.insert(secret.clone()),
                "channel and backend credentials must be distinct"
            );
        }
        let installation_id = format!("{}:{}", binding.app_id, binding.tenant_key);
        let sender = crate::feishu_outbound::FeishuSender::new(
            &installation_id,
            app_secret,
            entry.api_base.clone(),
            entry.allow_loopback,
        )?;
        sender
            .verify_installation(&binding.bot_open_id, &binding.tenant_key, &binding.chat_id)
            .await?;
        let db = FeishuStore::open(&config.registry_path, &binding)?;
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            let response = client
                .get(backend.url.join("internal/channels/feishu/status")?)
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
                status.protocol == 4
                    && status.backend_id == binding.backend_id
                    && status.mode == "gateway"
                    && status.max_run_seconds == 120
                    && status.tools == ["datetime_now", "json_query"],
                "gateway channel contract mismatch"
            );
            let identity = binding_identity(&binding);
            let response = client
                .post(backend.url.join("internal/channels/feishu-binding")?)
                .header(header::AUTHORIZATION, backend.token.clone())
                .timeout(Duration::from_secs(5))
                .json(&identity)
                .send()
                .await?;
            ensure!(json_response(&response), "backend Feishu binding rejected");
            let bytes = read_response(response, 4096)
                .await
                .map_err(|()| anyhow::anyhow!("invalid Feishu binding response"))?;
            let actual: BackendIdentity = decode_object(&bytes)?;
            ensure!(
                serde_json::to_value(actual)? == identity,
                "backend Feishu owner mismatch"
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
                encrypt_key,
                verification_token,
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
fn destination(binding: &FeishuBindingSummary) -> Destination {
    Destination {
        channel: Channel::Feishu,
        installation_id: format!("{}:{}", binding.app_id, binding.tenant_key),
        conversation_id: binding.chat_id.clone(),
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
    let Some(runtime) = state.feishu.as_ref() else {
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
    let headers = request.headers().clone();
    let mut content_types = headers.get_all(header::CONTENT_TYPE).iter();
    let valid_type = content_types
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        && content_types.next().is_none();
    if !valid_type {
        return answer(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required");
    }
    let Ok(_global) = runtime.inbound.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let Ok(_local) = installation.inbound.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let body = match tokio::time::timeout(Duration::from_millis(650), async {
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
    let installation_id = format!(
        "{}:{}",
        installation.binding.app_id, installation.binding.tenant_key
    );
    // Ordinary signatures and challenge verification-token validation precede all SQL.
    let callback = match crate::feishu::parse_event(
        &headers,
        &body,
        &installation_id,
        &installation.encrypt_key,
        &installation.verification_token,
        crate::unix_now_secs(),
    ) {
        Ok(event) => event,
        Err(_) => return answer(StatusCode::UNAUTHORIZED, "invalid_callback"),
    };
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    match io(runtime, move || {
        Ok(registry
            .feishu_authorized(id)?
            .is_some_and(|actual| same_binding(&actual, &expected)))
    })
    .await
    {
        Ok(true) => (),
        Ok(false) => return answer(StatusCode::FORBIDDEN, "binding_disabled"),
        Err(_) => return answer(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    }
    let event = match callback {
        crate::feishu::Inbound::Challenge(challenge) => {
            return Json(json!({"challenge":challenge})).into_response()
        }
        crate::feishu::Inbound::Ignored => return answer(StatusCode::OK, "ignored"),
        crate::feishu::Inbound::Message {
            event_id,
            sender_id,
            conversation_id,
            thread_id,
            text,
        } => {
            if sender_id != installation.binding.human_open_id
                || conversation_id != installation.binding.chat_id
                || thread_id.is_some()
                || text.len() > MAX_PROMPT
            {
                return answer(StatusCode::BAD_REQUEST, "event_not_authorized");
            }
            let fingerprint = format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        installation.binding.id.to_string(),
                        &event_id,
                        &sender_id,
                        &conversation_id,
                        &text
                    ))
                    .expect("serialize strings")
                )
            );
            EventSpec {
                event_id,
                session_id: format!("feishu:{}", installation.binding.id),
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
            .map_err(|_| anyhow::anyhow!("Feishu store poisoned"))?
            .inner
            .accept_channel_event(event, now_ms())
    })
    .await
    {
        Ok(_) => answer(StatusCode::OK, "accepted"),
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
        .feishu
        .is_some()
        .then(|| tokio::spawn(coordinate(state, stop.clone())));
    Worker { stop, task }
}
async fn coordinate(state: Arc<State>, stop: Stop) {
    let runtime = state.feishu.as_ref().unwrap().clone();
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
    binding: &FeishuBindingSummary,
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
            "Feishu write hold requires administrator review"
        );
    }
}
async fn process(state: Arc<State>, stop: Stop, installation: Arc<Installation>) {
    let runtime = state.feishu.as_ref().unwrap();
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
            .map_err(|_| anyhow::anyhow!("Feishu store poisoned"))?
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
        "feishu_send"
    } else {
        "feishu_execute"
    };
    let request_id = Uuid::now_v7();
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    let authorized = io(runtime, move || {
        stop.admit(|| {
            registry.admit_feishu(expected.id, request_id, operation, &expected.id.to_string())
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
    let runtime = state.feishu.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Feishu store poisoned"))?
            .inner
            .claim_feishu_event_recorded(now_ms(), &request_id.to_string())
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
            .map_err(|_| anyhow::anyhow!("Feishu store poisoned"))?
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
            && event.spec.session_id == format!("feishu:{}", installation.binding.id)
            && event.spec.sender_id == installation.binding.human_open_id
            && event.spec.enabled_tools == ["datetime_now", "json_query"]
            && event.spec.timeout_secs == 120
            && event.spec.sealed_token.is_none()
            && event.spec.prompt.len() <= MAX_PROMPT,
        "channel authorization mismatch"
    );
    let backend = &state.backends[&installation.binding.backend_id];
    let receipt=tokio::time::timeout(BACKEND_TIMEOUT,async {
        let response=state.client.post(backend.url.join("internal/channels/feishu/execute")?).header(header::AUTHORIZATION,backend.token.clone()).timeout(BACKEND_TIMEOUT).json(&json!({"protocol":4,"request_id":request_id.to_string(),"binding_id":installation.binding.id.to_string(),"session_id":event.spec.session_id,"event_id":event.spec.event_id,"prompt":event.spec.prompt})).send().await?;
        ensure!(json_response(&response),"backend channel request failed");
        let bytes=read_response(response,MAX_REPLY).await.map_err(|()|anyhow::anyhow!("invalid channel response"))?;
        decode_object::<ChatReceipt>(&bytes)
    }).await.context("backend channel deadline")??;
    let reply = &receipt.response;
    ensure!(
        receipt.protocol == 4
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
    outbound::split_text_for(Channel::Feishu, &reply.message.content)
}
async fn send(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.feishu.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Feishu store poisoned"))?
            .inner
            .claim_feishu_delivery_recorded(now_ms(), &request_id.to_string())
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
                .send(&delivery.destination, &delivery.id, &delivery.text),
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
        finish_io(runtime, move || store
            .lock()
            .map_err(|_| anyhow::anyhow!("Feishu store poisoned"))?
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
        .context("stop gateway before inspecting or reconciling Feishu state")?;
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
        .feishu_binding(binding_id)?
        .context("Feishu binding not found")?;
    registry.recover_writes()?;
    let Some(mut store) = FeishuStore::open_existing(&config.registry_path, &binding)? else {
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
        .list_feishu_bindings()?
        .into_iter()
        .find(|b| b.user_id == user)
    else {
        return Ok(());
    };
    if let Some(store) = FeishuStore::open_existing(&config.registry_path, &binding)? {
        super::review_queue(&store.inner)?;
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
    fn review_clear_checks_revoked_retained_feishu_queue_under_one_process_lock() {
        let root = std::env::temp_dir().join(format!("jiaclaw-feishu-review-{}", Uuid::new_v4()));
        let path = root.join("registry.sqlite3");
        let registry = Registry::open(&path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let b = registry
            .add_feishu_binding(
                user.user_id,
                "cli_app",
                "tenant",
                "ou_bot",
                "ou_human",
                "oc_dm",
            )
            .unwrap();
        let config:Config=serde_json::from_value(json!({"registry_path":path,"backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":root.join("backend.token")}]})).unwrap();
        let mut store = FeishuStore::open(&path, &b).unwrap();
        let e = store
            .inner
            .accept_channel_event(
                EventSpec {
                    event_id: "om_private".into(),
                    session_id: format!("feishu:{}", b.id),
                    sender_id: b.human_open_id.clone(),
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
            .claim_feishu_event_recorded(0, &Uuid::now_v7().to_string())
            .unwrap()
            .unwrap();
        drop(store);
        registry.revoke_feishu_binding(b.id).unwrap();
        assert!(super::super::channel_review_guard(&config, &registry, user.user_id).is_err());
        let mut store = FeishuStore::open(&path, &b).unwrap();
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
        let root = std::env::temp_dir().join(format!("jiaclaw-feishu-absent-{}", Uuid::new_v4()));
        let path = root.join("registry.sqlite3");
        let registry = Registry::open(&path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let b = registry
            .add_feishu_binding(
                user.user_id,
                "cli_app",
                "tenant",
                "ou_bot",
                "ou_human",
                "oc_dm",
            )
            .unwrap();
        registry.revoke_feishu_binding(b.id).unwrap();
        let config:Config=serde_json::from_value(json!({"registry_path":path,"backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":root.join("missing.token")}],
            "feishu":[{"binding_id":b.id.to_string(),"app_secret_file":root.join("missing.app"),"encrypt_key_file":root.join("missing.encrypt"),"verification_token_file":root.join("missing.verify")}]})).unwrap();
        for kind in ["events", "deliveries", "operations"] {
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
        assert!(!root.join("feishu").exists());
        drop(registry);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn provider_credentials_support_16_bytes_but_refuse_aliases_permissions_and_overflow() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir().join(format!("jiaclaw-feishu-secrets-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("secret");
        for length in [16, 31, 32, 1024] {
            std::fs::write(&path, format!("{}\r\n", "s".repeat(length))).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(secret_file(&path).unwrap(), "s".repeat(length));
        }
        for value in [
            "s".repeat(15),
            "s".repeat(1025),
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
    fn backend_protocol_rejects_duplicate_fields_unknown_fields_and_positional_arrays() {
        for body in [
            r#"{"protocol":4,"protocol":4,"backend_id":"alice","max_run_seconds":120,"tools":[],"mode":"gateway"}"#,
            r#"[4,"alice",120,[],"gateway"]"#,
            r#"{"protocol":4,"backend_id":"alice","max_run_seconds":120,"tools":[],"mode":"gateway","token":"secret"}"#,
        ] {
            assert!(decode_object::<BackendStatus>(body.as_bytes()).is_err());
        }
    }
}
