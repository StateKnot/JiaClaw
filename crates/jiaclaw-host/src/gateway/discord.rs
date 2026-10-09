// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Dedicated tenant Discord ingress; gateway-owned, conservative single-attempt delivery.
use super::{
    config::Config,
    discord_store::DiscordStore,
    proxy::read_response,
    registry::{DiscordBindingSummary, Registry},
    scheduler::Stop,
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
const IO_CAPACITY_BUSY: &str = "Discord I/O capacity busy";
const IO_TASK_FAILED: &str = "Discord I/O task failed";
struct Installation {
    binding: DiscordBindingSummary,
    cipher: Cipher,
    api_base: String,
    client: OutboundClient,
    store: Arc<Mutex<DiscordStore>>,
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
fn same_binding(a: &DiscordBindingSummary, b: &DiscordBindingSummary) -> bool {
    a.id == b.id
        && a.user_id == b.user_id
        && a.backend_id == b.backend_id
        && a.application_id == b.application_id
        && a.verify_key == b.verify_key
        && a.bot_user_id == b.bot_user_id
        && a.sender_id == b.sender_id
        && a.conversation_id == b.conversation_id
        && a.command_id == b.command_id
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
    application_id: String,
    verify_key: String,
    bot_user_id: String,
    sender_id: String,
    conversation_id: String,
    command_id: String,
    state_key_fingerprint: String,
}
fn binding_identity(binding: &DiscordBindingSummary, key_fingerprint: &str) -> Value {
    json!({"protocol":3,"binding_id":binding.id.to_string(),"user_id":binding.user_id.to_string(),
        "backend_id":binding.backend_id,"application_id":binding.application_id,"verify_key":binding.verify_key,
        "bot_user_id":binding.bot_user_id,"sender_id":binding.sender_id,"conversation_id":binding.conversation_id,
        "command_id":binding.command_id,"state_key_fingerprint":key_fingerprint})
}

struct Cipher(ring::aead::LessSafeKey);
impl Cipher {
    fn new(hex_key: &str) -> Result<(Self, String)> {
        ensure!(
            hex_key.len() == 64
                && hex_key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Discord state key must be 32-byte lowercase hex"
        );
        let key = crate::decode_hex(hex_key).context("invalid Discord state key")?;
        ensure!(
            key.iter().any(|b| *b != 0),
            "Discord state key must be nonzero"
        );
        let fingerprint = format!("{:x}", Sha256::digest(&key));
        let key = ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, &key)
            .map_err(|_| anyhow::anyhow!("invalid Discord state key"))?;
        Ok((Self(ring::aead::LessSafeKey::new(key)), fingerprint))
    }
    fn seal(&self, token: &str, aad: &str) -> Result<String> {
        use ring::rand::SecureRandom;
        let mut nonce = [0u8; 12];
        ring::rand::SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("Discord nonce generation failed"))?;
        let mut text = token.as_bytes().to_vec();
        self.0
            .seal_in_place_append_tag(
                ring::aead::Nonce::assume_unique_for_key(nonce),
                ring::aead::Aad::from(aad.as_bytes()),
                &mut text,
            )
            .map_err(|_| anyhow::anyhow!("Discord credential encryption failed"))?;
        Ok(nonce
            .into_iter()
            .chain(text)
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    fn unseal(&self, value: &str, aad: &str) -> Result<String> {
        let mut value = crate::decode_hex(value)
            .filter(|bytes| (29..=2076).contains(&bytes.len()))
            .context("invalid Discord sealed credential")?;
        let nonce: [u8; 12] = value[..12].try_into()?;
        let clear = self
            .0
            .open_in_place(
                ring::aead::Nonce::assume_unique_for_key(nonce),
                ring::aead::Aad::from(aad.as_bytes()),
                &mut value[12..],
            )
            .map_err(|_| anyhow::anyhow!("Discord credential authentication failed"))?;
        String::from_utf8(clear.to_vec())
            .map_err(|_| anyhow::anyhow!("invalid Discord credential encoding"))
    }
}
fn token_aad(binding: &DiscordBindingSummary, destination: &Destination) -> String {
    format!(
        "discord:{}:{}:{}",
        binding.id,
        binding.application_id,
        destination.interaction_id.as_deref().unwrap_or_default()
    )
}
#[derive(Deserialize)]
struct Application {
    id: String,
    verify_key: String,
    integration_types_config: IntegrationTypes,
    bot: Option<UserIdentity>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntegrationTypes {
    #[serde(rename = "1")]
    user: IntegrationType,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntegrationType {
    oauth2_install_params: InstallParams,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallParams {
    scopes: Vec<String>,
    permissions: String,
}
#[derive(Deserialize)]
struct UserIdentity {
    id: String,
    bot: Option<bool>,
    system: Option<bool>,
}
#[derive(Deserialize)]
struct Command {
    id: String,
    application_id: String,
    #[serde(rename = "type")]
    kind: u8,
    name: String,
    options: Vec<CommandOption>,
    integration_types: Vec<u8>,
    contexts: Vec<u8>,
    version: String,
    guild_id: Option<Value>,
}
#[derive(Deserialize)]
struct CommandOption {
    #[serde(rename = "type")]
    kind: u8,
    name: String,
    required: bool,
    autocomplete: Option<bool>,
    choices: Option<Value>,
    options: Option<Value>,
}
async fn verification_json<T: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T> {
    let response = request.timeout(Duration::from_secs(5)).send().await?;
    ensure!(json_response(&response), "Discord verification rejected");
    let bytes = read_response(response, MAX_BODY)
        .await
        .map_err(|()| anyhow::anyhow!("invalid Discord verification response"))?;
    decode_object(&bytes)
}
async fn verify_installation(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    binding: &DiscordBindingSummary,
) -> Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let base = api_base.trim_end_matches('/');
        let auth = format!("Bot {token}");
        let application: Application = verification_json(
            client
                .get(format!("{base}/applications/@me"))
                .header(header::AUTHORIZATION, &auth),
        )
        .await?;
        ensure!(
            application.id == binding.application_id
                && application.verify_key == binding.verify_key
                && application
                    .integration_types_config
                    .user
                    .oauth2_install_params
                    .scopes
                    == ["applications.commands"]
                && application
                    .integration_types_config
                    .user
                    .oauth2_install_params
                    .permissions
                    == "0"
                && application
                    .bot
                    .is_none_or(|bot| bot.id == binding.bot_user_id && bot.bot == Some(true)),
            "Discord application identity or USER_INSTALL-only permissions mismatch"
        );
        let bot: UserIdentity = verification_json(
            client
                .get(format!("{base}/users/@me"))
                .header(header::AUTHORIZATION, &auth),
        )
        .await?;
        ensure!(
            bot.id == binding.bot_user_id && bot.bot == Some(true) && bot.system != Some(true),
            "Discord Bot Token identity mismatch"
        );
        let command: Command = verification_json(
            client
                .get(format!(
                    "{base}/applications/{}/commands/{}",
                    binding.application_id, binding.command_id
                ))
                .header(header::AUTHORIZATION, &auth),
        )
        .await?;
        ensure!(
            command.id == binding.command_id
                && command.application_id == binding.application_id
                && command.kind == 1
                && command.name == "jiaclaw"
                && command.guild_id.is_none()
                && command.integration_types == [1]
                && command.contexts == [1]
                && crate::discord_outbound::snowflake(&command.version)
                && command.options.len() == 1
                && command.options[0].kind == 3
                && command.options[0].name == "prompt"
                && command.options[0].required
                && command.options[0].autocomplete != Some(true)
                && command.options[0].choices.is_none()
                && command.options[0].options.is_none(),
            "Discord fixed private command mismatch"
        );
        // Endpoint URL is deliberately not required: Discord saves it only after
        // a signed PING succeeds against this already running gateway.
        Ok::<_, anyhow::Error>(())
    })
    .await;
    ensure!(
        matches!(result, Ok(Ok(()))),
        "Discord installation identity verification failed"
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
    if config.discord.is_empty() {
        return Ok(None);
    }
    let bindings = registry.list_discord_bindings()?;
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
        let id = Uuid::parse_str(&entry.binding_id)?;
        let binding = bindings
            .iter()
            .find(|b| b.id == id)
            .context("configured Discord binding does not exist")?
            .clone();
        ensure!(
            binding.enabled,
            "configured Discord binding is revoked; remove its runtime configuration"
        );
        let backend = backends
            .get(&binding.backend_id)
            .context("Discord backend is not configured")?;
        let token = super::telegram::secret_file(&entry.bot_token_file)?;
        let state_key = super::telegram::secret_file(&entry.state_key_file)?;
        let (cipher, key_fingerprint) = Cipher::new(&state_key)?;
        ensure!(
            !token.is_empty()
                && token.len() <= 2048
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
            "Discord requires a bounded Bot Token"
        );
        ensure!(
            secrets.insert(token.clone()) && secrets.insert(state_key),
            "Discord, Slack, Telegram and backend credentials must be distinct"
        );
        verify_installation(client, &entry.api_base, &token, &binding).await?;
        let db = DiscordStore::open(&config.registry_path, &binding, &key_fingerprint)?;
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            let response = client
                .get(backend.url.join("internal/channels/discord/status")?)
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
                status.protocol == 3
                    && status.backend_id == binding.backend_id
                    && status.mode == "gateway"
                    && status.max_run_seconds == 120
                    && status.tools == ["datetime_now", "json_query"],
                "gateway channel contract mismatch"
            );
            let identity = binding_identity(&binding, &key_fingerprint);
            let response = client
                .post(backend.url.join("internal/channels/discord-binding")?)
                .header(header::AUTHORIZATION, backend.token.clone())
                .timeout(Duration::from_secs(5))
                .json(&identity)
                .send()
                .await?;
            ensure!(json_response(&response), "backend Discord binding rejected");
            let bytes = read_response(response, 4096)
                .await
                .map_err(|()| anyhow::anyhow!("invalid Discord binding response"))?;
            let actual: BackendIdentity = decode_object(&bytes)?;
            ensure!(
                serde_json::to_value(actual)? == identity,
                "backend Discord owner mismatch"
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
                cipher,
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
struct Callback {
    #[serde(rename = "type")]
    kind: u8,
    id: Option<String>,
    application_id: Option<String>,
    token: Option<String>,
    version: Option<u8>,
    context: Option<u8>,
    authorizing_integration_owners: Option<InstallationOwner>,
    user: Option<UserIdentity>,
    channel_id: Option<String>,
    channel: Option<PartialChannel>,
    guild_id: Option<Value>,
    member: Option<Value>,
    message: Option<Value>,
    data: Option<InteractionData>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallationOwner {
    #[serde(rename = "1")]
    user: String,
}
#[derive(Deserialize)]
struct PartialChannel {
    id: String,
    #[serde(rename = "type")]
    kind: Option<u8>,
}
#[derive(Deserialize)]
struct InteractionData {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: u8,
    options: Vec<InteractionOption>,
    resolved: Option<Value>,
    target_id: Option<Value>,
    guild_id: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InteractionOption {
    name: String,
    #[serde(rename = "type")]
    kind: u8,
    value: String,
}
fn interaction_expiry(id: &str, now: i64) -> Result<i64> {
    ensure!(
        crate::discord_outbound::snowflake(id),
        "invalid Discord interaction ID"
    );
    let created = i64::try_from(id.parse::<u64>()? >> 22)?
        .checked_add(1_420_070_400_000)
        .context("Discord timestamp overflow")?;
    let expires = created
        .checked_add(14 * 60 * 1000)
        .context("Discord expiry overflow")?;
    ensure!(
        created <= now.saturating_add(30_000) && expires > now.saturating_add(165_000),
        "Discord interaction lacks the execution and delivery budget"
    );
    Ok(expires)
}
fn authorized_destination(
    binding: &DiscordBindingSummary,
    destination: &Destination,
    event_id: &str,
) -> bool {
    destination.channel == Channel::Discord
        && destination.installation_id == binding.application_id
        && destination.conversation_id == binding.conversation_id
        && destination.thread_id.is_none()
        && destination.interaction_id.as_deref() == Some(event_id)
        && crate::discord_outbound::snowflake(event_id)
        && destination
            .expires_ms
            .is_some_and(|expires| id_expiry(event_id) == Some(expires))
}
fn id_expiry(id: &str) -> Option<i64> {
    if !crate::discord_outbound::snowflake(id) {
        return None;
    }
    i64::try_from(id.parse::<u64>().ok()? >> 22)
        .ok()?
        .checked_add(1_420_070_400_000)?
        .checked_add(14 * 60 * 1000)
}
fn parse_event(installation: &Installation, value: Callback, now: i64) -> Result<EventSpec> {
    let binding = &installation.binding;
    ensure!(
        value.kind == 2
            && value.version == Some(1)
            && value.application_id.as_deref() == Some(&binding.application_id)
            && value.context == Some(1)
            && value.guild_id.is_none()
            && value.member.is_none()
            && value.message.is_none(),
        "Discord private command context mismatch"
    );
    let owner = value
        .authorizing_integration_owners
        .context("missing Discord USER_INSTALL owner")?;
    let user = value.user.context("missing Discord human identity")?;
    ensure!(
        owner.user == binding.sender_id
            && user.id == binding.sender_id
            && user.bot != Some(true)
            && user.system != Some(true),
        "Discord human installation owner mismatch"
    );
    let channel = value
        .channel_id
        .as_deref()
        .or_else(|| value.channel.as_ref().map(|channel| channel.id.as_str()));
    ensure!(
        channel == Some(binding.conversation_id.as_str())
            && value
                .channel
                .as_ref()
                .is_none_or(|channel| channel.id == binding.conversation_id
                    && channel.kind.is_none_or(|kind| kind == 1)),
        "Discord fixed DM mismatch"
    );
    let data = value.data.context("missing Discord command")?;
    ensure!(
        data.id == binding.command_id
            && data.name == "jiaclaw"
            && data.kind == 1
            && data.options.len() == 1
            && data.options[0].name == "prompt"
            && data.options[0].kind == 3
            && data.resolved.is_none()
            && data.target_id.is_none()
            && data.guild_id.is_none(),
        "Discord fixed text command mismatch"
    );
    let prompt = data.options.into_iter().next().unwrap().value;
    ensure!(
        !prompt.trim().is_empty() && prompt.len() <= 16 * 1024,
        "Discord prompt is empty or oversized"
    );
    let id = value.id.context("missing Discord interaction ID")?;
    let expires = interaction_expiry(&id, now)?;
    let token = value
        .token
        .context("missing Discord interaction credential")?;
    ensure!(
        !token.is_empty()
            && token.len() <= 2048
            && !matches!(token.as_str(), "." | "..")
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "invalid Discord interaction credential"
    );
    let destination = Destination {
        channel: Channel::Discord,
        installation_id: binding.application_id.clone(),
        conversation_id: binding.conversation_id.clone(),
        thread_id: None,
        interaction_id: Some(id.clone()),
        expires_ms: Some(expires),
    };
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            binding.id.to_string(),
            &id,
            &binding.sender_id,
            &binding.application_id,
            &binding.command_id,
            &binding.conversation_id,
            expires,
            &prompt,
            format!("{:x}", Sha256::digest(token.as_bytes()))
        ))?)
    );
    let sealed = installation
        .cipher
        .seal(&token, &token_aad(binding, &destination))?;
    Ok(EventSpec {
        event_id: id,
        session_id: format!("discord:{}", binding.id),
        sender_id: binding.sender_id.clone(),
        prompt,
        enabled_tools: vec!["datetime_now".into(), "json_query".into()],
        timeout_secs: 120,
        destination,
        sealed_token: Some(sealed),
        fingerprint,
    })
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
    let Some(runtime) = state.discord.as_ref() else {
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
            single("x-signature-timestamp"),
            single("x-signature-ed25519"),
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
        value.len() == 128
            && value
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
    if !crate::verify_discord_request(
        &installation.binding.verify_key,
        timestamp.as_deref(),
        signature.as_deref(),
        &body,
    ) {
        return answer(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    match io(runtime, move || {
        Ok(registry
            .discord_authorized(id)?
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
    if callback.kind == 1 {
        return Json(json!({"type":1})).into_response();
    }
    let event = match parse_event(&installation, callback, now_ms()) {
        Ok(event) => event,
        Err(_) => return answer(StatusCode::BAD_REQUEST, "event_not_authorized"),
    };
    let store = installation.store.clone();
    match io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Discord store poisoned"))?
            .inner
            .accept_channel_event(event, now_ms())
    })
    .await
    {
        Ok(_) => Json(json!({"type":5,"data":{"flags":64}})).into_response(),
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
        .discord
        .is_some()
        .then(|| tokio::spawn(coordinate(state, stop.clone())));
    Worker { stop, task }
}
async fn coordinate(state: Arc<State>, stop: Stop) {
    let runtime = state.discord.as_ref().unwrap().clone();
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
    binding: &DiscordBindingSummary,
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
            "Discord write hold requires administrator review"
        );
    }
}
async fn process(state: Arc<State>, stop: Stop, installation: Arc<Installation>) {
    let runtime = state.discord.as_ref().unwrap();
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
            .map_err(|_| anyhow::anyhow!("Discord store poisoned"))?
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
        "discord_send"
    } else {
        "discord_execute"
    };
    let request_id = Uuid::now_v7();
    let registry = state.registry.clone();
    let expected = installation.binding.clone();
    let authorized = io(runtime, move || {
        stop.admit(|| {
            registry.admit_discord(expected.id, request_id, operation, &expected.id.to_string())
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
    let runtime = state.discord.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Discord store poisoned"))?
            .inner
            .claim_discord_event_recorded(now_ms(), &request_id.to_string())
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
        let mut store = store
            .lock()
            .map_err(|_| anyhow::anyhow!("Discord store poisoned"))?;
        complete_execution(&mut store, &event, successful, chunks, now_ms())
    })
    .await;
    matches!(committed, Ok(true))
}
fn complete_execution(
    store: &mut DiscordStore,
    event: &ChannelEvent,
    successful: bool,
    chunks: Vec<String>,
    now: i64,
) -> Result<bool> {
    // Recheck after the blocking I/O slot and store mutex are acquired. The
    // earlier model budget cannot authorize a reply after pause/clock changes.
    let successful = successful
        && event
            .spec
            .destination
            .expires_ms
            .is_some_and(|expires| expires > now.saturating_add(10_000));
    let committed = store.inner.complete_channel_event(
        &event.id,
        None,
        if successful {
            "completed"
        } else {
            "needs_review"
        },
        if successful { chunks } else { Vec::new() },
        (!successful)
            .then(|| "gateway channel execution requires review; no automatic replay".into()),
        now,
    )?;
    // Slow fsync or a clock jump during commit must also preserve the hold.
    Ok(committed
        && successful
        && event
            .spec
            .destination
            .expires_ms
            .is_some_and(|expires| expires > now_ms().saturating_add(5_000)))
}

async fn run_chat(
    state: &State,
    installation: &Installation,
    event: &ChannelEvent,
    request_id: Uuid,
) -> Result<Vec<String>> {
    ensure!(
        authorized_destination(
            &installation.binding,
            &event.spec.destination,
            &event.spec.event_id
        ) && event.spec.session_id == format!("discord:{}", installation.binding.id)
            && event.spec.sender_id == installation.binding.sender_id
            && event.spec.enabled_tools == ["datetime_now", "json_query"]
            && event.spec.timeout_secs == 120
            && event
                .spec
                .sealed_token
                .as_deref()
                .is_some_and(|value| installation
                    .cipher
                    .unseal(
                        value,
                        &token_aad(&installation.binding, &event.spec.destination)
                    )
                    .is_ok())
            && event
                .spec
                .destination
                .expires_ms
                .is_some_and(|expires| expires > now_ms().saturating_add(165_000)),
        "channel authorization mismatch"
    );
    let backend = &state.backends[&installation.binding.backend_id];
    let receipt=tokio::time::timeout(BACKEND_TIMEOUT,async {
        let response=state.client.post(backend.url.join("internal/channels/discord/execute")?).header(header::AUTHORIZATION,backend.token.clone()).timeout(BACKEND_TIMEOUT).json(&json!({"protocol":3,"request_id":request_id.to_string(),"binding_id":installation.binding.id.to_string(),"session_id":event.spec.session_id,"event_id":event.spec.event_id,"prompt":event.spec.prompt})).send().await?;
        ensure!(json_response(&response),"backend channel request failed");
        let bytes=read_response(response,MAX_REPLY).await.map_err(|()|anyhow::anyhow!("invalid channel response"))?;
        decode_object::<ChatReceipt>(&bytes)
    }).await.context("backend channel deadline")??;
    let reply = &receipt.response;
    ensure!(
        receipt.protocol == 3
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
    let parts = outbound::split_text_for(Channel::Discord, &reply.message.content)?;
    ensure!(
        parts.len() <= 6,
        "Discord user installation reply exceeds original plus five followups"
    );
    Ok(parts)
}
async fn send(state: &State, installation: &Installation, request_id: Uuid) -> bool {
    let runtime = state.discord.as_ref().unwrap();
    let store = installation.store.clone();
    let claimed = io(runtime, move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Discord store poisoned"))?
            .inner
            .claim_discord_delivery_recorded(now_ms(), &request_id.to_string())
    })
    .await;
    let delivery = match claimed {
        Ok(Some(d)) => d,
        Ok(None) => return false,
        Err(_) => return false,
    };
    let event_id = delivery
        .destination
        .interaction_id
        .as_deref()
        .unwrap_or_default();
    let token = delivery.sealed_token.as_deref().and_then(|token| {
        installation
            .cipher
            .unseal(
                token,
                &token_aad(&installation.binding, &delivery.destination),
            )
            .ok()
    });
    let outcome = if authorized_destination(&installation.binding, &delivery.destination, event_id)
        && delivery.event_id.is_some()
        && delivery.job_run_id.is_none()
        && token.is_some()
    {
        installation
            .client
            .send_discord_private(
                &delivery.destination,
                delivery.ordinal as usize,
                &delivery.text,
                token.as_deref().unwrap(),
                &installation.api_base,
            )
            .await
    } else {
        DeliveryOutcome::Rejected {
            code: "binding_or_credential_mismatch",
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
        DeliveryOutcome::RateLimited { retry_after_ms }
            if delivery.destination.expires_ms.is_some_and(|expires| {
                now.saturating_add(retry_after_ms).saturating_add(10_000) >= expires
            }) =>
        {
            (
                "expired",
                None,
                Some("rate_limit_exceeds_interaction_lifetime".into()),
                None,
                false,
            )
        }
        DeliveryOutcome::Rejected {
            code: "interaction_expired",
        } => (
            "expired",
            None,
            Some("interaction_expired".into()),
            None,
            false,
        ),
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
            .map_err(|_| anyhow::anyhow!("Discord store poisoned"))?
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
        .context("stop gateway before inspecting or reconciling Discord state")?;
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
        .list_discord_bindings()?
        .into_iter()
        .find(|b| b.id == binding_id)
        .context("Discord binding not found")?;
    registry.recover_writes()?;
    let Some(mut store) = DiscordStore::open_existing(&config.registry_path, &binding)? else {
        if let AdminAction::Inspect {
            kind,
            event,
            limit,
            offset,
        } = action
        {
            ensure!(
                (1..=100).contains(&limit)
                    && offset <= if kind == "operations" { 16000 } else { 10000 },
                "inspection page out of range"
            );
            if let Some(id) = event {
                checked_uuid(&id)?;
            }
            ensure!(
                matches!(kind.as_str(), "events" | "deliveries" | "operations"),
                "inspection kind must be events, deliveries or operations"
            );
            return Ok(json!({"binding_id":binding_id.to_string(),"result":{kind:[]}}));
        }
        anyhow::bail!("Discord state has not been initialized");
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
        .list_discord_bindings()?
        .into_iter()
        .find(|b| b.user_id == user)
    else {
        return Ok(());
    };
    if let Some(store) = DiscordStore::open_existing(&config.registry_path, &binding)? {
        super::review_queue(&store.inner)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> DiscordBindingSummary {
        DiscordBindingSummary {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            backend_id: "alice".into(),
            application_id: "1001".into(),
            verify_key: "11".repeat(32),
            bot_user_id: "1002".into(),
            sender_id: "1003".into(),
            conversation_id: "1004".into(),
            command_id: "1005".into(),
            enabled: true,
        }
    }
    fn installation(binding: DiscordBindingSummary) -> (Installation, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("jiaclaw-discord-runtime-{}", Uuid::new_v4()));
        drop(Registry::open(&root.join("registry.sqlite3")).unwrap());
        let (cipher, fp) = Cipher::new(&"22".repeat(32)).unwrap();
        let store = DiscordStore::open(&root.join("registry.sqlite3"), &binding, &fp).unwrap();
        (
            Installation {
                binding,
                cipher,
                api_base: "https://discord.com/api/v10".into(),
                client: OutboundClient::new().unwrap(),
                store: Arc::new(Mutex::new(store)),
                inbound: Arc::new(Semaphore::new(1)),
            },
            root,
        )
    }
    fn callback(binding: &DiscordBindingSummary, now: i64) -> Value {
        let id = ((u64::try_from(now - 1_420_070_400_000).unwrap() << 22) + 17).to_string();
        json!({"type":2,"version":1,"id":id,"application_id":binding.application_id,"token":"interaction-private-secret","context":1,"authorizing_integration_owners":{"1":binding.sender_id},"user":{"id":binding.sender_id},"channel_id":binding.conversation_id,"data":{"id":binding.command_id,"type":1,"name":"jiaclaw","options":[{"name":"prompt","type":3,"value":"private <@123>"}]}})
    }
    fn parse(installation: &Installation, value: &Value, now: i64) -> Result<EventSpec> {
        parse_event(
            installation,
            decode_object(&serde_json::to_vec(value)?)?,
            now,
        )
    }
    #[test]
    fn private_installation_fixed_command_and_token_fingerprint_are_pinned() {
        let (i, root) = installation(binding());
        let now = now_ms();
        let original = callback(&i.binding, now);
        let a = parse(&i, &original, now).unwrap();
        let b = parse(&i, &original, now).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_ne!(a.sealed_token, b.sealed_token);
        assert_eq!(a.session_id, format!("discord:{}", i.binding.id));
        assert!(authorized_destination(
            &i.binding,
            &a.destination,
            &a.event_id
        ));
        assert_eq!(
            i.cipher
                .unseal(
                    a.sealed_token.as_deref().unwrap(),
                    &token_aad(&i.binding, &a.destination)
                )
                .unwrap(),
            "interaction-private-secret"
        );
        let mut changed = original;
        changed["token"] = json!("different-interaction-token");
        assert_ne!(parse(&i, &changed, now).unwrap().fingerprint, a.fingerprint);
        drop(i);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn owner_context_human_and_plain_text_shape_are_mandatory() {
        let (i, root) = installation(binding());
        let now = now_ms();
        for (pointer, replacement) in [
            ("/type", json!(3)),
            ("/version", json!(2)),
            ("/application_id", json!("999")),
            ("/context", json!(2)),
            ("/guild_id", json!("999")),
            ("/member", json!({})),
            ("/message", json!({})),
            ("/authorizing_integration_owners", json!({"0":"1003"})),
            (
                "/authorizing_integration_owners",
                json!({"1":"1003","0":"999"}),
            ),
            ("/authorizing_integration_owners/1", json!("999")),
            ("/user/id", json!("999")),
            ("/user/bot", json!(true)),
            ("/user/system", json!(true)),
            ("/channel_id", json!("999")),
            ("/channel", json!({"id":"1004","type":3})),
            ("/channel", json!({"id":"999","type":1})),
            ("/data/id", json!("999")),
            ("/data/name", json!("other")),
            ("/data/type", json!(2)),
            ("/data/resolved", json!({})),
            ("/data/target_id", json!("999")),
            ("/data/guild_id", json!("999")),
            ("/data/options", json!([])),
            (
                "/data/options",
                json!([{"name":"prompt","type":3,"value":"one"},{"name":"prompt","type":3,"value":"two"}]),
            ),
            (
                "/data/options",
                json!([{"name":"prompt","type":3,"value":"one","options":[]}]),
            ),
            (
                "/data/options",
                json!([{"name":"file","type":11,"value":"123"}]),
            ),
            (
                "/data/options",
                json!([{"name":"prompt","type":3,"value":"  "}]),
            ),
            ("/token", json!("../secret")),
            ("/id", json!("01")),
        ] {
            let mut value = callback(&i.binding, now);
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            value.pointer_mut(parent).unwrap()[key] = replacement;
            assert!(parse(&i, &value, now).is_err(), "accepted {pointer}");
        }
        drop(i);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn partial_channel_without_legacy_channel_id_is_supported_without_recipient_assumptions() {
        let (i, root) = installation(binding());
        let now = now_ms();
        let mut value = callback(&i.binding, now);
        value.as_object_mut().unwrap().remove("channel_id");
        value["channel"] = json!({"id":i.binding.conversation_id});
        assert!(parse(&i, &value, now).is_ok());
        value.as_object_mut().unwrap().remove("channel");
        assert!(parse(&i, &value, now).is_err());
        drop(i);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn selected_duplicate_fields_and_positional_arrays_are_rejected() {
        for bytes in [
            r#"{"type":1,"type":2}"#,
            r#"{"type":2,"authorizing_integration_owners":{"1":"1003","1":"999"}}"#,
            r#"{"type":2,"user":{"id":"1003","id":"999"}}"#,
            r#"{"type":2,"channel":{"id":"1004","id":"999"}}"#,
            r#"[1]"#,
        ] {
            assert!(decode_object::<Callback>(bytes.as_bytes()).is_err());
        }
        assert_eq!(decode_object::<Callback>(br#"{"type":1}"#).unwrap().kind, 1);
    }
    #[test]
    fn credential_aad_authenticates_binding_application_and_interaction_and_key() {
        let b = binding();
        let now = now_ms();
        let id = ((u64::try_from(now - 1_420_070_400_000).unwrap() << 22) + 1).to_string();
        let d = Destination {
            channel: Channel::Discord,
            installation_id: b.application_id.clone(),
            conversation_id: b.conversation_id.clone(),
            thread_id: None,
            interaction_id: Some(id),
            expires_ms: Some(now + 840_000),
        };
        let (cipher, _) = Cipher::new(&"22".repeat(32)).unwrap();
        let sealed = cipher.seal("private-token", &token_aad(&b, &d)).unwrap();
        let mut other = b.clone();
        other.id = Uuid::new_v4();
        assert!(cipher.unseal(&sealed, &token_aad(&other, &d)).is_err());
        other = b.clone();
        other.application_id = "999".into();
        assert!(cipher.unseal(&sealed, &token_aad(&other, &d)).is_err());
        let mut changed = d.clone();
        changed.interaction_id = Some("999".into());
        assert!(cipher.unseal(&sealed, &token_aad(&b, &changed)).is_err());
        let (other_key, _) = Cipher::new(&"33".repeat(32)).unwrap();
        assert!(other_key.unseal(&sealed, &token_aad(&b, &d)).is_err());
        assert!(Cipher::new(&"00".repeat(32)).is_err());
        assert!(Cipher::new(&"AB".repeat(32)).is_err());
    }
    #[test]
    fn interaction_budget_uses_snowflake_time_and_never_renews() {
        let now = now_ms();
        let id = |created: i64| {
            ((u64::try_from(created - 1_420_070_400_000).unwrap() << 22) + 1).to_string()
        };
        assert_eq!(interaction_expiry(&id(now), now).unwrap(), now + 840_000);
        assert!(interaction_expiry(&id(now - 675_000), now).is_err());
        assert!(interaction_expiry(&id(now + 30_001), now).is_err());
        assert!(interaction_expiry("18446744073709551616", now).is_err());
    }
    #[test]
    fn review_clear_retains_revoked_discord_queue_without_runtime_configuration_or_secret() {
        let root = std::env::temp_dir().join(format!("jiaclaw-discord-review-{}", Uuid::new_v4()));
        let path = root.join("registry.sqlite3");
        let registry = Registry::open(&path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let b = registry
            .add_discord_binding(
                user.user_id,
                "1001",
                &"11".repeat(32),
                "1002",
                "1003",
                "1004",
                "1005",
            )
            .unwrap();
        let config:Config=serde_json::from_value(json!({"registry_path":path,"backends":[{"id":"alice","url":"http://127.0.0.1:9","token_file":root.join("backend.token")}]})).unwrap();
        let (i, other_root) = installation(b.clone());
        let now = now_ms();
        let event = parse(&i, &callback(&b, now), now).unwrap();
        let mut store =
            DiscordStore::open(&path, &b, &format!("{:x}", Sha256::digest([0x22u8; 32]))).unwrap();
        let e = store.inner.accept_channel_event(event, now).unwrap();
        store
            .inner
            .claim_discord_event_recorded(now, &Uuid::now_v7().to_string())
            .unwrap()
            .unwrap();
        drop(store);
        drop(i);
        std::fs::remove_dir_all(other_root).unwrap();
        registry.revoke_discord_binding(b.id).unwrap();
        assert!(super::super::channel_review_guard(&config, &registry, user.user_id).is_err());
        let mut store = DiscordStore::open_existing(&path, &b).unwrap().unwrap();
        store.inner.cancel_channel_event(&e.id, now).unwrap();
        drop(store);
        let guard = super::super::channel_review_guard(&config, &registry, user.user_id)
            .unwrap()
            .unwrap();
        assert!(super::super::channel_review_guard(&config, &registry, user.user_id).is_err());
        drop(guard);
        drop(registry);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn cancelled_io_retains_capacity_and_finish_waits_for_settlement() {
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
        tokio::select! { _=&mut work=>panic!("completed before release"),_=started_rx=>{} }
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut work)
            .await
            .is_err());
        assert!(io(&runtime, || Ok(())).await.is_err());
        release_tx.send(()).unwrap();
        work.await.unwrap();
        let permit = runtime.io.clone().acquire_owned().await.unwrap();
        let finish = finish_io(&runtime, || Ok("settled"));
        tokio::pin!(finish);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut finish)
            .await
            .is_err());
        drop(permit);
        assert_eq!(finish.await.unwrap(), "settled");
    }
    #[test]
    fn reply_expiring_between_model_and_commit_preserves_review_without_outbox() {
        let (i, root) = installation(binding());
        let now = now_ms();
        let mut store = i.store.lock().unwrap();
        let event = parse(&i, &callback(&i.binding, now), now).unwrap();
        store.inner.accept_channel_event(event, now).unwrap();
        let claimed = store
            .inner
            .claim_discord_event_recorded(now, &Uuid::now_v7().to_string())
            .unwrap()
            .unwrap();
        let late = claimed.spec.destination.expires_ms.unwrap() - 10_000;
        assert!(!complete_execution(
            &mut store,
            &claimed,
            true,
            vec!["already computed reply".into()],
            late
        )
        .unwrap());
        let events = store.inner.list_channel_events(100, 0).unwrap();
        assert_eq!(events[0].status, "needs_review");
        assert!(store
            .inner
            .list_channel_deliveries(None, 100, 0)
            .unwrap()
            .is_empty());
        assert_eq!(store.operations(100, 0).unwrap().len(), 1);
        drop(store);
        drop(i);
        std::fs::remove_dir_all(root).unwrap();
    }
}
