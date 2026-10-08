// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Authorized channel ingress and supervised persistent message processing.
use super::{
    channel_types::{Channel, Destination, EventSpec, ScheduledDestination},
    outbound::OutboundClient,
    AppError, AppState,
};
use anyhow::{bail, Context, Result};
use axum::{
    body::Bytes,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use jiaclaw_core::{AgentConfig, ChannelBinding};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};

pub(super) struct Installation {
    pub channel: Channel,
    pub policy: ChannelBinding,
    pub credential: String,
    pub inbound_secret: String,
    pub api_base: String,
    pub feishu_sender: Option<Arc<super::feishu_outbound::FeishuSender>>,
    pub feishu_verification_token: Option<String>,
    pub wecom_sender: Option<Arc<super::wecom_outbound::WeComSender>>,
    pub wecom_callback: Option<Arc<super::wecom::Callback>>,
    pub dingtalk_sender: Option<Arc<super::dingtalk_outbound::DingTalkSender>>,
    pub dingtalk_callback: Option<Arc<super::dingtalk::Callback>>,
    pub discord_sender: Option<Arc<super::discord_outbound::DiscordBotSender>>,
}

pub(super) struct ChannelRuntime {
    pub installations: Vec<Installation>,
    pub client: OutboundClient,
    cipher: Option<aead::LessSafeKey>,
    pub health: AtomicU8,
}

fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Telegram => "telegram",
        Channel::Slack => "slack",
        Channel::Discord => "discord",
        Channel::Feishu => "feishu",
        Channel::Wecom => "wecom",
        Channel::Dingtalk => "dingtalk",
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl ChannelRuntime {
    pub(super) fn configure(
        config: &AgentConfig,
        api_token: Option<&str>,
    ) -> Result<Option<Arc<Self>>> {
        let http = &config.http;
        let telegram_secret = std::env::var("JIACLAW_TELEGRAM_SECRET")
            .ok()
            .or_else(|| http.telegram_secret.clone())
            .filter(|s| !s.trim().is_empty());
        let credentials_present = telegram_secret.is_some()
            || http.effective_telegram_bot_token().is_some()
            || http.effective_slack_signing_secret().is_some()
            || http.effective_slack_bot_token().is_some()
            || http.effective_discord_public_key().is_some()
            || http.effective_feishu_app_secret().is_some()
            || http.effective_feishu_encrypt_key().is_some()
            || http.effective_feishu_verification_token().is_some()
            || http.effective_wecom_app_secret().is_some()
            || http.effective_dingtalk_app_secret().is_some()
            || http.effective_wecom_callback_token().is_some()
            || http.effective_wecom_encoding_aes_key().is_some()
            || http.effective_discord_bot_token().is_some();
        if http.channels.is_empty() {
            if credentials_present {
                bail!("channel credentials require explicit http.channels installation/sender/tool policies; see docs/channels.md");
            }
            return Ok(None);
        }
        if !http.persist || api_token.is_none() {
            bail!("channels require SQLite persistence and an API Token");
        }
        if !matches!(
            config.provider.provider_type.as_str(),
            "brokerrouter" | "stub"
        ) {
            bail!("channels require brokerrouter or explicit stub provider");
        }
        if http.channels.len() > 6 {
            bail!("at most one installation per channel is supported");
        }
        let mut seen = HashSet::new();
        let mut installations = Vec::new();
        let mut cipher = None;
        let mut local = false;
        for policy in &http.channels {
            let channel = match policy.channel.as_str() {
                "telegram" => Channel::Telegram,
                "slack" => Channel::Slack,
                "discord" => Channel::Discord,
                "feishu" => Channel::Feishu,
                "wecom" => Channel::Wecom,
                "dingtalk" => Channel::Dingtalk,
                _ => bail!("unsupported channel"),
            };
            if !seen.insert(channel) {
                bail!("duplicate channel installation");
            }
            let valid_installation = if channel == Channel::Feishu {
                super::feishu::validate_installation(&policy.installation_id).is_ok()
                    && policy.app_id.is_none()
            } else if channel == Channel::Dingtalk {
                super::dingtalk::validate_installation(&policy.installation_id).is_ok()
                    && policy
                        .app_id
                        .as_ref()
                        .is_some_and(|id| super::dingtalk::identity(id))
            } else if channel == Channel::Wecom {
                super::wecom::validate_installation(&policy.installation_id).is_ok()
                    && policy.app_id.is_none()
            } else {
                valid_id(&policy.installation_id)
            };
            if !valid_installation || !(1..=600).contains(&policy.timeout_secs) {
                bail!("invalid channel installation or timeout");
            }
            if let Some(guild) = &policy.discord_guild_id {
                if channel != Channel::Discord
                    || !super::discord_outbound::snowflake(guild)
                    || policy.scheduled_destinations.is_empty()
                {
                    bail!("discord_guild_id requires explicit Discord scheduled destinations");
                }
            }
            if channel == Channel::Discord
                && !policy.scheduled_destinations.is_empty()
                && (policy.discord_guild_id.is_none()
                    || !super::discord_outbound::snowflake(&policy.installation_id))
            {
                bail!("Discord scheduled delivery requires a canonical application and guild ID");
            }
            for (index, list) in [
                &policy.allowed_senders,
                &policy.allowed_conversations,
                &policy.enabled_tools,
            ]
            .into_iter()
            .enumerate()
            {
                if list.is_empty()
                    || list.len() > 100
                    || list.iter().any(|v| {
                        if channel == Channel::Wecom && index < 2 {
                            !super::wecom::user_id(v)
                        } else if channel == Channel::Dingtalk && index < 2 {
                            !super::dingtalk::user_id(v)
                        } else {
                            !valid_id(v)
                        }
                    })
                    || list.iter().collect::<HashSet<_>>().len() != list.len()
                {
                    bail!("channel identity and tool allowlists must explicitly contain 1..100 unique IDs");
                }
            }
            if policy.enabled_tools.len() > 32 {
                bail!("channel tools must contain 1..32 names");
            }
            if policy.scheduled_destinations.len() > 100
                || policy
                    .scheduled_destinations
                    .iter()
                    .collect::<HashSet<_>>()
                    .len()
                    != policy.scheduled_destinations.len()
            {
                bail!("scheduled_destinations must contain at most 100 unique exact destinations");
            }
            for destination in &policy.scheduled_destinations {
                ScheduledDestination {
                    channel,
                    installation_id: policy.installation_id.clone(),
                    conversation_id: destination.conversation_id.clone(),
                    thread_id: destination.thread_id.clone(),
                }
                .validate()?;
            }
            let (credential, inbound_secret, default_base) = match channel {
                Channel::Dingtalk => (String::new(), String::new(), "https://api.dingtalk.com"),
                Channel::Wecom => (
                    String::new(),
                    String::new(),
                    "https://qyapi.weixin.qq.com/cgi-bin",
                ),
                Channel::Feishu => (
                    String::new(),
                    http.effective_feishu_encrypt_key()
                        .context("Feishu Encrypt Key is required")?,
                    "https://open.feishu.cn/open-apis",
                ),
                Channel::Telegram => {
                    let token = http
                        .effective_telegram_bot_token()
                        .context("Telegram Bot Token is required")?;
                    if token.split(':').next() != Some(policy.installation_id.as_str()) {
                        bail!("Telegram installation_id must equal the Bot Token numeric prefix");
                    }
                    (
                        token,
                        telegram_secret
                            .clone()
                            .context("Telegram webhook secret is required")?,
                        "https://api.telegram.org",
                    )
                }
                Channel::Slack => {
                    if policy.app_id.as_deref().is_none_or(|v| !valid_id(v)) {
                        bail!("Slack app_id is required");
                    }
                    (
                        http.effective_slack_bot_token()
                            .context("Slack Bot Token is required")?,
                        http.effective_slack_signing_secret()
                            .context("Slack signing secret is required")?,
                        "https://slack.com/api",
                    )
                }
                Channel::Discord => {
                    let secret = http
                        .effective_discord_public_key()
                        .context("Discord public key is required")?;
                    if super::decode_hex(&secret).is_none_or(|v| v.len() != 32) {
                        bail!("invalid Discord public key");
                    }
                    let key = std::env::var("JIACLAW_CHANNEL_STATE_KEY").ok().and_then(|v| super::decode_hex(&v)).filter(|v| v.len() == 32).context("Discord requires JIACLAW_CHANNEL_STATE_KEY: 64 hex characters; retain it with protected state backups")?;
                    cipher = Some(aead::LessSafeKey::new(
                        aead::UnboundKey::new(&aead::AES_256_GCM, &key)
                            .map_err(|_| anyhow::anyhow!("invalid channel state key"))?,
                    ));
                    (String::new(), secret, "https://discord.com/api/v10")
                }
            };
            let api_base = if let Some(base) = &policy.local_test_api_base {
                let url = reqwest::Url::parse(base).context("invalid local channel fixture URL")?;
                if url.scheme() != "http"
                    || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    bail!("local_test_api_base must be explicit HTTP literal loopback without credentials/query/fragment");
                }
                local = true;
                base.trim_end_matches('/').to_owned()
            } else {
                default_base.to_owned()
            };
            super::outbound::validate_api_base(
                channel,
                &api_base,
                policy.local_test_api_base.is_some(),
            )?;
            let (feishu_sender, feishu_verification_token) = if channel == Channel::Feishu {
                let secret = http
                    .effective_feishu_app_secret()
                    .context("Feishu App Secret is required")?;
                let token = http
                    .effective_feishu_verification_token()
                    .context("Feishu Verification Token is required")?;
                if inbound_secret.len() > 1024 || token.len() > 1024 {
                    bail!("Feishu callback secrets exceed limit");
                }
                (
                    Some(Arc::new(super::feishu_outbound::FeishuSender::new(
                        &policy.installation_id,
                        secret,
                        api_base.clone(),
                        policy.local_test_api_base.is_some(),
                    )?)),
                    Some(token),
                )
            } else {
                (None, None)
            };
            let (wecom_sender, wecom_callback) = if channel == Channel::Wecom {
                let callback = super::wecom::Callback::new(
                    &policy.installation_id,
                    http.effective_wecom_callback_token()
                        .context("WeCom callback token is required")?,
                    &http
                        .effective_wecom_encoding_aes_key()
                        .context("WeCom EncodingAESKey is required")?,
                )?;
                let sender = super::wecom_outbound::WeComSender::new(
                    &policy.installation_id,
                    http.effective_wecom_app_secret()
                        .context("WeCom App Secret is required")?,
                    api_base.clone(),
                    policy.local_test_api_base.is_some(),
                )?;
                (Some(Arc::new(sender)), Some(Arc::new(callback)))
            } else {
                (None, None)
            };
            let (dingtalk_sender, dingtalk_callback) = if channel == Channel::Dingtalk {
                let secret = http
                    .effective_dingtalk_app_secret()
                    .context("DingTalk Client Secret is required")?;
                let callback =
                    super::dingtalk::Callback::new(&policy.installation_id, secret.clone())?;
                let sender = super::dingtalk_outbound::DingTalkSender::new(
                    &policy.installation_id,
                    policy
                        .app_id
                        .clone()
                        .context("DingTalk Client ID is required")?,
                    secret,
                    api_base.clone(),
                    policy.local_test_api_base.is_some(),
                )?;
                (Some(Arc::new(sender)), Some(Arc::new(callback)))
            } else {
                (None, None)
            };
            let discord_sender =
                if channel == Channel::Discord && !policy.scheduled_destinations.is_empty() {
                    Some(Arc::new(super::discord_outbound::DiscordBotSender::new(
                        &policy.installation_id,
                        policy
                            .discord_guild_id
                            .clone()
                            .context("Discord guild ID is required")?,
                        http.effective_discord_bot_token()
                            .context("Discord scheduled delivery requires a Bot Token")?,
                        api_base.clone(),
                        policy.local_test_api_base.is_some(),
                    )?))
                } else {
                    None
                };
            installations.push(Installation {
                channel,
                policy: policy.clone(),
                credential,
                inbound_secret,
                api_base,
                feishu_sender,
                feishu_verification_token,
                wecom_sender,
                wecom_callback,
                dingtalk_sender,
                dingtalk_callback,
                discord_sender,
            });
        }
        Ok(Some(Arc::new(Self {
            installations,
            client: if local {
                OutboundClient::new_with_loopback(true)?
            } else {
                OutboundClient::new()?
            },
            cipher,
            health: AtomicU8::new(0),
        })))
    }

    /// Verify the dedicated WeCom credential and explicit member scope before
    /// accepting callbacks, running jobs or submitting any message.
    pub(super) async fn verify_wecom_installation(&self) -> Result<()> {
        let Some(binding) = self.installation(Channel::Wecom) else {
            return Ok(());
        };
        let sender = binding
            .wecom_sender
            .as_ref()
            .context("WeCom sender is unavailable")?;
        // The inbound allowlists and scheduled authorization are independent.
        // Require platform visibility for their whole bounded union rather than
        // infer access from department/tag membership or an opaque token.
        let mut members: Vec<String> = binding
            .policy
            .allowed_senders
            .iter()
            .chain(&binding.policy.allowed_conversations)
            .chain(
                binding
                    .policy
                    .scheduled_destinations
                    .iter()
                    .map(|destination| &destination.conversation_id),
            )
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        members.sort();
        sender.verify_installation(&members).await
    }

    pub(super) fn installation(&self, channel: Channel) -> Option<&Installation> {
        self.installations
            .iter()
            .find(|binding| binding.channel == channel)
    }

    fn seal(&self, secret: &str, aad: &str) -> Result<String> {
        let key = self
            .cipher
            .as_ref()
            .context("channel state encryption unavailable")?;
        let mut nonce = [0u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("nonce generation failed"))?;
        let mut ciphertext = secret.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| anyhow::anyhow!("channel encryption failed"))?;
        let bytes: Vec<_> = nonce.into_iter().chain(ciphertext).collect();
        Ok(hex(&bytes))
    }

    pub(super) fn unseal(&self, ciphertext: &str, aad: &str) -> Result<String> {
        let key = self
            .cipher
            .as_ref()
            .context("channel state key unavailable")?;
        let mut bytes = super::decode_hex(ciphertext)
            .filter(|v| v.len() >= 28 && v.len() <= 4096)
            .context("invalid encrypted channel credential")?;
        let nonce: [u8; 12] = bytes[..12].try_into()?;
        let clear = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad.as_bytes()),
                &mut bytes[12..],
            )
            .map_err(|_| anyhow::anyhow!("channel credential authentication failed"))?;
        Ok(String::from_utf8(clear.to_vec())?)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn token_aad(destination: &Destination) -> String {
    format!(
        "{}:{}:{}",
        channel_name(destination.channel),
        destination.installation_id,
        destination.interaction_id.as_deref().unwrap_or_default()
    )
}

pub(super) fn validate_tools(state: &AppState, tools: &[String]) -> Result<()> {
    if tools.is_empty() || tools.len() > 32 {
        bail!("channel requires an explicit tool allowlist");
    }
    for name in tools {
        if state.agent.tools().get(name).is_none()
            || (!matches!(
                name.as_str(),
                "datetime_now" | "json_query" | "exec" | "shell_exec"
            ) && !name.starts_with("mcp_"))
        {
            bail!("unregistered or unqualified background channel tool");
        }
    }
    Ok(())
}

fn runtime(state: &AppState) -> Result<&Arc<ChannelRuntime>, AppError> {
    state.channel_runtime.as_ref().ok_or(AppError::NotFound)
}
fn identifier(value: Option<&Value>) -> Option<String> {
    let value = match value? {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        _ => return None,
    };
    valid_id(&value).then_some(value)
}
fn required_id(value: Option<&Value>) -> Result<String, AppError> {
    identifier(value)
        .ok_or_else(|| AppError::BadRequest("missing or invalid platform identity".into()))
}
fn text(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}
fn skipped() -> Response {
    Json(json!({"ok":true,"skipped":true})).into_response()
}

fn event_spec(
    binding: &Installation,
    event_id: String,
    sender_id: String,
    prompt: String,
    destination: Destination,
    sealed_token: Option<String>,
) -> Result<EventSpec, AppError> {
    super::outbound::validate_destination(&destination)
        .map_err(|_| AppError::BadRequest("invalid platform destination".into()))?;
    if !binding.policy.allowed_senders.contains(&sender_id)
        || !binding
            .policy
            .allowed_conversations
            .contains(&destination.conversation_id)
    {
        return Err(AppError::Unauthorized);
    }
    if prompt.len() > 32 * 1024 {
        return Err(AppError::BadRequest("channel text exceeds 32 KiB".into()));
    }
    let identity = serde_json::to_vec(&(
        destination.channel,
        &destination.installation_id,
        &destination.conversation_id,
        &destination.thread_id,
        &sender_id,
    ))
    .map_err(|_| AppError::Internal("channel identity encoding".into()))?;
    let session_id = format!(
        "channel:{}:{}",
        channel_name(binding.channel),
        hex(&Sha256::digest(identity))
    );
    let fingerprint = hex(&Sha256::digest(
        serde_json::to_vec(&(&event_id, &session_id, &prompt))
            .map_err(|_| AppError::Internal("channel fingerprint encoding".into()))?,
    ));
    Ok(EventSpec {
        event_id,
        session_id,
        sender_id,
        prompt,
        enabled_tools: binding.policy.enabled_tools.clone(),
        timeout_secs: binding.policy.timeout_secs,
        destination,
        sealed_token,
        fingerprint,
    })
}

pub(super) async fn telegram(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let rt = runtime(&state)?;
    let binding = rt
        .installation(Channel::Telegram)
        .ok_or(AppError::NotFound)?;
    if headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok())
        != Some(binding.inbound_secret.as_str())
    {
        return Err(AppError::Unauthorized);
    }
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("invalid JSON".into()))?;
    let event_id = required_id(value.get("update_id"))?;
    let Some(message) = value.get("message") else {
        return Ok(skipped());
    };
    if message.pointer("/from/is_bot").and_then(Value::as_bool) == Some(true) {
        return Ok(skipped());
    }
    let Some(prompt) = text(message.get("text")) else {
        return Ok(skipped());
    };
    let sender = required_id(message.pointer("/from/id"))?;
    let conversation = required_id(message.pointer("/chat/id"))?;
    let thread = message
        .get("message_thread_id")
        .map(|v| required_id(Some(v)))
        .transpose()?;
    let destination = Destination {
        channel: Channel::Telegram,
        installation_id: binding.policy.installation_id.clone(),
        conversation_id: conversation,
        thread_id: thread,
        interaction_id: None,
        expires_ms: None,
    };
    accept(
        &state,
        event_spec(binding, event_id, sender, prompt, destination, None)?,
        false,
    )
    .await
}

pub(super) async fn slack(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let rt = runtime(&state)?;
    let binding = rt.installation(Channel::Slack).ok_or(AppError::NotFound)?;
    if !super::verify_slack_request(
        &binding.inbound_secret,
        headers
            .get("x-slack-request-timestamp")
            .and_then(|v| v.to_str().ok()),
        headers
            .get("x-slack-signature")
            .and_then(|v| v.to_str().ok()),
        &body,
        super::unix_now_secs(),
    ) {
        return Err(AppError::Unauthorized);
    }
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("invalid JSON".into()))?;
    if value.get("type").and_then(Value::as_str) == Some("url_verification") {
        let challenge = text(value.get("challenge"))
            .filter(|v| v.len() <= 1024)
            .ok_or_else(|| AppError::BadRequest("invalid challenge".into()))?;
        return Ok(Json(json!({"challenge":challenge})).into_response());
    }
    if value.get("type").and_then(Value::as_str) != Some("event_callback") {
        return Ok(skipped());
    }
    if value.get("team_id").and_then(Value::as_str) != Some(binding.policy.installation_id.as_str())
        || value.get("api_app_id").and_then(Value::as_str) != binding.policy.app_id.as_deref()
    {
        return Err(AppError::Unauthorized);
    }
    let event_id = required_id(value.get("event_id"))?;
    let event = value
        .get("event")
        .ok_or_else(|| AppError::BadRequest("missing event".into()))?;
    if event.get("type").and_then(Value::as_str) != Some("message")
        || event.get("subtype").is_some_and(|v| !v.is_null())
        || event.get("bot_id").is_some()
    {
        return Ok(skipped());
    }
    let Some(prompt) = text(event.get("text")) else {
        return Ok(skipped());
    };
    let sender = required_id(event.get("user"))?;
    let conversation = required_id(event.get("channel"))?;
    // Replies stay in the original message's thread; the binding also isolates senders.
    let thread = required_id(event.get("thread_ts").or_else(|| event.get("ts")))?;
    let destination = Destination {
        channel: Channel::Slack,
        installation_id: binding.policy.installation_id.clone(),
        conversation_id: conversation,
        thread_id: Some(thread),
        interaction_id: None,
        expires_ms: None,
    };
    accept(
        &state,
        event_spec(binding, event_id, sender, prompt, destination, None)?,
        false,
    )
    .await
}

pub(super) async fn feishu(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let binding = runtime(&state)?
        .installation(Channel::Feishu)
        .ok_or(AppError::NotFound)?;
    let inbound = super::feishu::parse_event(
        &headers,
        &body,
        &binding.policy.installation_id,
        &binding.inbound_secret,
        binding
            .feishu_verification_token
            .as_deref()
            .ok_or(AppError::Unauthorized)?,
        super::unix_now_secs(),
    )
    .map_err(|_| AppError::Unauthorized)?;
    match inbound {
        super::feishu::Inbound::Challenge(challenge) => {
            Ok(Json(json!({"challenge":challenge})).into_response())
        }
        super::feishu::Inbound::Ignored => Ok(skipped()),
        super::feishu::Inbound::Message {
            event_id,
            sender_id,
            conversation_id,
            thread_id,
            text,
        } => {
            let destination = Destination {
                channel: Channel::Feishu,
                installation_id: binding.policy.installation_id.clone(),
                conversation_id,
                thread_id,
                interaction_id: None,
                expires_ms: None,
            };
            accept(
                &state,
                event_spec(binding, event_id, sender_id, text, destination, None)?,
                false,
            )
            .await
        }
    }
}

pub(super) async fn dingtalk(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let binding = runtime(&state)?
        .installation(Channel::Dingtalk)
        .ok_or(AppError::NotFound)?;
    let callback = binding
        .dingtalk_callback
        .as_ref()
        .ok_or(AppError::NotFound)?;
    let inbound = callback
        .parse_event(&headers, &body, now_ms())
        .map_err(|_| AppError::Unauthorized)?;
    if let super::dingtalk::Inbound::Message {
        event_id,
        sender_id,
        text,
    } = inbound
    {
        let destination = Destination {
            channel: Channel::Dingtalk,
            installation_id: binding.policy.installation_id.clone(),
            conversation_id: sender_id.clone(),
            thread_id: None,
            interaction_id: None,
            expires_ms: None,
        };
        accept(
            &state,
            event_spec(binding, event_id, sender_id, text, destination, None)?,
            false,
        )
        .await?;
    }
    // Official HTTP robot example permits an empty 200. ACK only after durable admission.
    Ok(StatusCode::OK.into_response())
}

pub(super) async fn wecom_verify(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let callback = runtime(&state)?
        .installation(Channel::Wecom)
        .and_then(|b| b.wecom_callback.as_ref())
        .ok_or(AppError::NotFound)?;
    let echo = callback
        .challenge(query.as_deref().unwrap_or_default(), super::unix_now_secs())
        .map_err(|_| AppError::Unauthorized)?;
    Ok((StatusCode::OK, echo).into_response())
}

pub(super) async fn wecom(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Result<Response, AppError> {
    let binding = runtime(&state)?
        .installation(Channel::Wecom)
        .ok_or(AppError::NotFound)?;
    let callback = binding.wecom_callback.as_ref().ok_or(AppError::NotFound)?;
    let inbound = callback
        .parse_event(
            query.as_deref().unwrap_or_default(),
            &body,
            super::unix_now_secs(),
        )
        .map_err(|_| AppError::Unauthorized)?;
    if let super::wecom::Inbound::Message {
        event_id,
        sender_id,
        text,
    } = inbound
    {
        let destination = Destination {
            channel: Channel::Wecom,
            installation_id: binding.policy.installation_id.clone(),
            conversation_id: sender_id.clone(),
            thread_id: None,
            interaction_id: None,
            expires_ms: None,
        };
        accept(
            &state,
            event_spec(binding, event_id, sender_id, text, destination, None)?,
            false,
        )
        .await?;
    }
    // The application callback protocol expects an empty HTTP 200 after durable admission.
    Ok(StatusCode::OK.into_response())
}

pub(super) async fn discord(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let rt = runtime(&state)?;
    let binding = rt
        .installation(Channel::Discord)
        .ok_or(AppError::NotFound)?;
    let timestamp = headers
        .get("x-signature-timestamp")
        .and_then(|v| v.to_str().ok());
    if !timestamp.is_some_and(|v| super::slack_timestamp_fresh(v, super::unix_now_secs()))
        || !super::verify_discord_request(
            &binding.inbound_secret,
            timestamp,
            headers
                .get("x-signature-ed25519")
                .and_then(|v| v.to_str().ok()),
            &body,
        )
    {
        return Err(AppError::Unauthorized);
    }
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("invalid JSON".into()))?;
    if value.get("type").and_then(Value::as_u64) == Some(1) {
        return Ok(Json(json!({"type":1})).into_response());
    }
    if value.get("type").and_then(Value::as_u64) != Some(2)
        || value.pointer("/data/type").and_then(Value::as_u64) != Some(1)
    {
        return Ok(Json(json!({"type":4,"data":{"content":"Unsupported command","flags":64,"allowed_mentions":{"parse":[]}}})).into_response());
    }
    if value.get("application_id").and_then(Value::as_str)
        != Some(binding.policy.installation_id.as_str())
    {
        return Err(AppError::Unauthorized);
    }
    let event_id = required_id(value.get("id"))?;
    let created_ms = event_id
        .parse::<u64>()
        .ok()
        .and_then(|id| i64::try_from(id >> 22).ok())
        .and_then(|ms| ms.checked_add(1_420_070_400_000))
        .ok_or_else(|| AppError::BadRequest("invalid interaction snowflake".into()))?;
    let expires_ms = created_ms.saturating_add(14 * 60 * 1000);
    if created_ms > now_ms().saturating_add(30_000) || expires_ms <= now_ms().saturating_add(10_000)
    {
        return Err(AppError::BadRequest(
            "interaction expired or has invalid creation time".into(),
        ));
    }
    let sender = required_id(
        value
            .pointer("/member/user/id")
            .or_else(|| value.pointer("/user/id")),
    )?;
    let conversation = required_id(value.get("channel_id"))?;
    // Only one explicit string prompt option is admitted; no arbitrary nested command expansion.
    let options = value
        .pointer("/data/options")
        .and_then(Value::as_array)
        .filter(|v| v.len() == 1)
        .ok_or_else(|| AppError::BadRequest("command requires one string prompt option".into()))?;
    if options[0].get("type").and_then(Value::as_u64) != Some(3) {
        return Err(AppError::BadRequest(
            "command requires a string prompt".into(),
        ));
    }
    let prompt =
        text(options[0].get("value")).ok_or_else(|| AppError::BadRequest("empty prompt".into()))?;
    let token = text(value.get("token"))
        .filter(|v| v.len() <= 2048)
        .ok_or_else(|| AppError::BadRequest("missing interaction token".into()))?;
    let destination = Destination {
        channel: Channel::Discord,
        installation_id: binding.policy.installation_id.clone(),
        conversation_id: conversation,
        thread_id: None,
        interaction_id: Some(event_id.clone()),
        expires_ms: Some(expires_ms),
    };
    let sealed = rt
        .seal(&token, &token_aad(&destination))
        .map_err(|_| AppError::Internal("channel credential encryption failed".into()))?;
    accept(
        &state,
        event_spec(binding, event_id, sender, prompt, destination, Some(sealed))?,
        true,
    )
    .await
}

pub(super) fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

async fn accept(state: &AppState, spec: EventSpec, discord: bool) -> Result<Response, AppError> {
    if runtime(state)?.health.load(Ordering::Acquire) != 1 {
        return Err(AppError::ChannelUnavailable);
    }
    validate_tools(state, &spec.enabled_tools)
        .map_err(|_| AppError::BadRequest("channel tools unavailable".into()))?;
    let accepted = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        channel_db(state, move |store| {
            store.accept_channel_event(spec, now_ms())
        }),
    )
    .await
    .map_err(|_| AppError::ChannelUnavailable)??;
    if discord {
        Ok(Json(json!({"type":5})).into_response())
    } else {
        Ok((
            StatusCode::OK,
            Json(json!({"ok":true,"event_id":accepted.id,"duplicate":!accepted.created})),
        )
            .into_response())
    }
}

async fn channel_db<T: Send + 'static>(
    state: &AppState,
    operation: impl FnOnce(&mut super::store::SessionStore) -> Result<T> + Send + 'static,
) -> Result<T, AppError> {
    super::with_sessions(state, move |store| match operation(store) {
        Ok(value) => Ok(Ok(value)),
        Err(e)
            if e.downcast_ref::<super::channel_store::ChannelConflict>()
                .is_some() =>
        {
            Ok(Err(AppError::ChannelConflict))
        }
        Err(e)
            if e.downcast_ref::<super::channel_store::ChannelCapacity>()
                .is_some() =>
        {
            Ok(Err(AppError::ChannelUnavailable))
        }
        Err(e) if e.downcast_ref::<super::jobs::JobConflict>().is_some() => {
            Ok(Err(AppError::JobConflict(e.to_string())))
        }
        Err(e) => Err(e),
    })
    .await?
}

fn authorized_admin(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    if state.api_token.is_none() || !super::check_api_auth(state, headers) {
        return Err(AppError::Unauthorized);
    }
    if !state.persist_enabled {
        return Err(AppError::NotFound);
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    #[serde(default = "page_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    event_id: Option<String>,
}
fn page_limit() -> usize {
    50
}
impl Page {
    fn validate(&self) -> Result<(), AppError> {
        if !(1..=100).contains(&self.limit)
            || self.offset > 10_000
            || self.event_id.as_ref().is_some_and(|v| !valid_id(v))
        {
            return Err(AppError::BadRequest("invalid channel pagination".into()));
        }
        Ok(())
    }
}

pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorized_admin(&state, &headers)?;
    let health = state
        .channel_runtime
        .as_ref()
        .map_or(0, |rt| rt.health.load(Ordering::Acquire));
    Ok(Json(
        json!({"state":match health {1=>"running",2=>"failed",3=>"stopping",_=>"disabled"},"max_processing":4}),
    ))
}
pub(super) async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(page): axum::extract::Query<Page>,
) -> Result<Json<Vec<super::channel_store::ChannelEvent>>, AppError> {
    authorized_admin(&state, &headers)?;
    page.validate()?;
    if page.event_id.is_some() {
        return Err(AppError::BadRequest(
            "event_id applies to deliveries".into(),
        ));
    }
    Ok(Json(
        channel_db(&state, move |store| {
            store.list_channel_events(page.limit, page.offset)
        })
        .await?,
    ))
}
pub(super) async fn event(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<super::channel_store::ChannelEvent>, AppError> {
    authorized_admin(&state, &headers)?;
    Ok(Json(
        channel_db(&state, move |store| store.get_channel_event(&id))
            .await?
            .ok_or(AppError::NotFound)?,
    ))
}
/// Audit remains available when channel workers are disabled, but always needs
/// an administrator token and persistent storage.
pub(super) async fn delivery(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<super::channel_store::ChannelDelivery>, AppError> {
    authorized_admin(&state, &headers)?;
    if !uuid::Uuid::parse_str(&id).is_ok_and(|parsed| !parsed.is_nil() && parsed.to_string() == id)
    {
        return Err(AppError::BadRequest(
            "delivery ID must be a canonical nonzero UUID".into(),
        ));
    }
    Ok(Json(
        channel_db(&state, move |store| store.get_channel_delivery(&id))
            .await?
            .ok_or(AppError::NotFound)?,
    ))
}
pub(super) async fn deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(page): axum::extract::Query<Page>,
) -> Result<Json<Vec<super::channel_store::ChannelDelivery>>, AppError> {
    authorized_admin(&state, &headers)?;
    page.validate()?;
    Ok(Json(
        channel_db(&state, move |store| {
            store.list_channel_deliveries(page.event_id.as_deref(), page.limit, page.offset)
        })
        .await?,
    ))
}
pub(super) async fn purge(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, AppError> {
    authorized_admin(&state, &headers)?;
    if !channel_db(&state, move |store| {
        store.purge_channel_event(&id, now_ms())
    })
    .await?
    {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, AppError> {
    authorized_admin(&state, &headers)?;
    if !channel_db(&state, move |store| {
        store.cancel_channel_event(&id, now_ms())
    })
    .await?
    {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolution {
    action: String,
    #[serde(default)]
    receipt: Option<String>,
}
pub(super) async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Resolution>,
) -> Result<StatusCode, AppError> {
    authorized_admin(&state, &headers)?;
    match body.action.as_str() {
        "delivered" => {
            let receipt = body
                .receipt
                .filter(|v| {
                    !v.trim().is_empty() && v.len() <= 4096 && !v.chars().any(char::is_control)
                })
                .ok_or_else(|| {
                    AppError::BadRequest("manual receipt evidence required (1..4096 bytes)".into())
                })?;
            if !channel_db(&state, move |store| {
                store.resolve_channel_delivery(&id, receipt, now_ms())
            })
            .await?
            {
                return Err(AppError::NotFound);
            }
        }
        "cancel" => {
            let delivery = channel_db(&state, move |store| store.get_channel_delivery(&id))
                .await?
                .ok_or(AppError::NotFound)?;
            channel_db(&state, move |store| {
                match (delivery.event_id, delivery.job_id, delivery.job_run_id) {
                    (Some(event_id), None, None) => store.cancel_channel_event(&event_id, now_ms()),
                    (None, Some(job_id), Some(run_id)) => {
                        store.cancel_job_delivery(&job_id, &run_id, now_ms())
                    }
                    _ => bail!("invalid delivery source"),
                }
            })
            .await?;
        }
        _ => {
            return Err(AppError::BadRequest(
                "action must be delivered or cancel".into(),
            ))
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn job_deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((job_id, run_id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(page): axum::extract::Query<Page>,
) -> Result<Json<Vec<super::channel_store::ChannelDelivery>>, AppError> {
    authorized_admin(&state, &headers)?;
    page.validate()?;
    if page.event_id.is_some() {
        return Err(AppError::BadRequest(
            "event_id does not apply to scheduled deliveries".into(),
        ));
    }
    let records = channel_db(&state, move |store| {
        let run = store.get_job_run(&run_id)?;
        if run.is_none_or(|run| run.job_id != job_id) {
            return Ok(None);
        }
        store
            .list_job_deliveries(&job_id, &run_id, page.limit, page.offset)
            .map(Some)
    })
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(records))
}

pub(super) async fn cancel_job_delivery(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((job_id, run_id)): axum::extract::Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    authorized_admin(&state, &headers)?;
    if !channel_db(&state, move |store| {
        store.cancel_job_delivery(&job_id, &run_id, now_ms())
    })
    .await?
    {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn purge_job_delivery(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((job_id, run_id)): axum::extract::Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    authorized_admin(&state, &headers)?;
    if !channel_db(&state, move |store| {
        store.purge_job_delivery(&job_id, &run_id)
    })
    .await?
    {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) struct ChannelWorkers {
    stop: tokio::sync::watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}
impl ChannelWorkers {
    pub(super) fn stop(&self, state: &AppState) {
        if let Some(rt) = &state.channel_runtime {
            rt.health.store(3, Ordering::Release);
        }
        let _ = self.stop.send(true);
    }
    pub(super) async fn shutdown(mut self, state: &AppState, grace: std::time::Duration) {
        self.stop(state);
        if tokio::time::timeout(grace, &mut self.handle).await.is_err() {
            self.handle.abort();
            let _ = self.handle.await;
        }
        if super::with_sessions(state, |store| store.recover_channels(now_ms()))
            .await
            .is_err()
        {
            tracing::error!("channel interruption accounting requires startup recovery");
        }
    }
}

async fn claim_event(
    state: &AppState,
) -> Result<Option<super::channel_store::ChannelEvent>, AppError> {
    let rt = runtime(state)?.clone();
    super::with_sessions(state, move |store| {
        // This closure can outlive its awaiting task. Check only after the store
        // mutex is held so a cancelled queued claim cannot follow recovery.
        if rt.health.load(Ordering::Acquire) != 1 {
            return Ok(None);
        }
        store.claim_channel_event(now_ms())
    })
    .await
}

async fn claim_delivery(
    state: &AppState,
) -> Result<Option<super::channel_store::ChannelDelivery>, AppError> {
    let rt = runtime(state)?.clone();
    super::with_sessions(state, move |store| {
        if rt.health.load(Ordering::Acquire) != 1 {
            return Ok(None);
        }
        store.claim_channel_delivery(now_ms())
    })
    .await
}

pub(super) async fn start(state: AppState) -> Result<Option<ChannelWorkers>> {
    if state.persist_enabled {
        super::with_sessions(&state, |store| store.recover_channels(now_ms()))
            .await
            .map_err(|_| anyhow::anyhow!("channel recovery failed"))?;
    }
    let Some(rt) = state.channel_runtime.clone() else {
        return Ok(None);
    };
    for binding in &rt.installations {
        validate_tools(&state, &binding.policy.enabled_tools)?;
    }
    rt.health.store(1, Ordering::Release);
    let (stop, mut rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(async move {
        let mut active = tokio::task::JoinSet::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut fault = false;
        loop {
            tokio::select! {biased;
                _=rx.changed()=>break,
                result=active.join_next(),if !active.is_empty()=>{
                    if !matches!(result,Some(Ok(Ok(())))) {fault=true;break;}
                },
                _=tick.tick(),if active.len()<10=>{
                    match claim_event(&state).await {
                        Ok(Some(event))=>{let worker=state.clone();active.spawn(async move {process_event(worker,event).await});},
                        Ok(None)=>{},Err(_)=>{fault=true;break;},
                    }
                    match claim_delivery(&state).await {
                        Ok(Some(delivery))=>{let worker=state.clone();active.spawn(async move {deliver(worker,delivery).await});},
                        Ok(None)=>{},Err(_)=>{fault=true;break;},
                    }
                },
            }
        }
        if fault {
            rt.health.store(2, Ordering::Release);
            active.abort_all();
        }
        while active.join_next().await.is_some() {}
        if fault {
            let _ = super::with_sessions(&state, |store| store.recover_channels(now_ms())).await;
            tracing::error!(
                "channel workers stopped; inspect outcomes, repair storage and restart"
            );
        }
    });
    Ok(Some(ChannelWorkers { stop, handle }))
}

fn still_authorized(state: &AppState, spec: &EventSpec) -> bool {
    state
        .channel_runtime
        .as_ref()
        .and_then(|rt| rt.installation(spec.destination.channel))
        .is_some_and(|b| {
            b.policy.installation_id == spec.destination.installation_id
                && b.policy.allowed_senders.contains(&spec.sender_id)
                && b.policy
                    .allowed_conversations
                    .contains(&spec.destination.conversation_id)
                && spec
                    .enabled_tools
                    .iter()
                    .all(|v| b.policy.enabled_tools.contains(v))
        })
        && validate_tools(state, &spec.enabled_tools).is_ok()
}

fn scheduled_destination_allowed(state: &AppState, destination: &ScheduledDestination) -> bool {
    destination.validate().is_ok()
        && state
            .channel_runtime
            .as_ref()
            .and_then(|rt| rt.installation(destination.channel))
            .is_some_and(|binding| {
                binding.policy.installation_id == destination.installation_id
                    && (destination.channel != Channel::Discord || binding.discord_sender.is_some())
                    && binding.policy.scheduled_destinations.iter().any(|allowed| {
                        allowed.conversation_id == destination.conversation_id
                            && allowed.thread_id == destination.thread_id
                    })
            })
}

/// Proactive destinations never inherit inbound sender/conversation authority.
pub(super) fn validate_scheduled_delivery(
    state: &AppState,
    destination: &ScheduledDestination,
) -> Result<(), AppError> {
    if state
        .channel_runtime
        .as_ref()
        .is_none_or(|rt| rt.health.load(Ordering::Acquire) != 1)
    {
        return Err(AppError::ChannelUnavailable);
    }
    if !scheduled_destination_allowed(state, destination) {
        return Err(AppError::BadRequest(
            "scheduled destination is not explicitly authorized".into(),
        ));
    }
    Ok(())
}

pub(super) fn scheduled_job_authorized(state: &AppState, spec: &super::jobs::JobSpec) -> bool {
    spec.delivery.as_ref().is_some_and(|destination| {
        scheduled_destination_allowed(state, destination)
            && state
                .channel_runtime
                .as_ref()
                .and_then(|rt| rt.installation(destination.channel))
                .is_some_and(|binding| {
                    spec.enabled_tools
                        .iter()
                        .all(|tool| binding.policy.enabled_tools.contains(tool))
                })
    }) && validate_tools(state, &spec.enabled_tools).is_ok()
}

pub(super) fn validate_scheduled_job(
    state: &AppState,
    spec: &super::jobs::JobSpec,
) -> Result<(), AppError> {
    if let Some(destination) = &spec.delivery {
        validate_scheduled_delivery(state, destination)?;
        if !scheduled_job_authorized(state, spec) {
            return Err(AppError::BadRequest(
                "scheduled tools are not authorized for this installation".into(),
            ));
        }
    }
    Ok(())
}

async fn finish_event(
    state: &AppState,
    event: super::channel_store::ChannelEvent,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    messages: Option<Vec<jiaclaw_core::ChatMessage>>,
    status: &str,
    chunks: Vec<String>,
    error: Option<String>,
) -> Result<(), AppError> {
    let status = status.to_owned();
    let session = messages.map(|m| (event.spec.session_id.clone(), super::SessionRecord::new(m)));
    let completed = super::with_sessions(state, move |store| {
        let _guard = guard;
        store.complete_channel_event(&event.id, session, &status, chunks, error, now_ms())
    })
    .await?;
    if !completed {
        return Err(AppError::Internal(
            "channel event completion no longer owned".into(),
        ));
    }
    Ok(())
}

fn execution_budget(spec: &EventSpec, now: i64) -> std::time::Duration {
    let configured = std::time::Duration::from_secs(spec.timeout_secs);
    spec.destination.expires_ms.map_or(configured, |expires| {
        configured.min(std::time::Duration::from_millis(
            u64::try_from(expires.saturating_sub(now).saturating_sub(10_000)).unwrap_or_default(),
        ))
    })
}

async fn process_event(
    state: AppState,
    event: super::channel_store::ChannelEvent,
) -> Result<(), AppError> {
    use jiaclaw_core::{ChatMessage, ChatRequest, MessageRole, ModelPurpose, RunStatus};
    if !still_authorized(&state, &event.spec) {
        return finish_event(
            &state,
            event,
            None,
            None,
            "needs_review",
            vec![],
            Some("channel authorization changed".into()),
        )
        .await;
    }
    if event.spec.destination.channel == Channel::Discord
        && event
            .spec
            .sealed_token
            .as_deref()
            .and_then(|token| {
                state
                    .channel_runtime
                    .as_ref()?
                    .unseal(token, &token_aad(&event.spec.destination))
                    .ok()
            })
            .is_none()
    {
        return finish_event(
            &state,
            event,
            None,
            None,
            "needs_review",
            vec![],
            Some(
                "interaction credential unavailable; inspect state key before any model execution"
                    .into(),
            ),
        )
        .await;
    }
    if event
        .spec
        .destination
        .expires_ms
        .is_some_and(|v| v <= now_ms().saturating_add(10_000))
    {
        return finish_event(
            &state,
            event,
            None,
            None,
            "needs_review",
            vec![],
            Some("interaction expired".into()),
        )
        .await;
    }
    let budget = execution_budget(&event.spec, now_ms());
    let deadline = tokio::time::Instant::now() + budget;
    let Ok(guard) = tokio::time::timeout_at(
        deadline,
        super::session_turn_lock(&state, &event.spec.session_id),
    )
    .await
    else {
        return finish_event(
            &state,
            event,
            None,
            None,
            "needs_review",
            vec![],
            Some("channel session deadline expired".into()),
        )
        .await;
    };
    let prepared = tokio::time::timeout_at(
        deadline,
        super::prepare_session_chat_messages(
            &state,
            &event.spec.session_id,
            vec![ChatMessage {
                role: MessageRole::User,
                content: event.spec.prompt.clone(),
            }],
            &event.id,
            "channel",
        ),
    )
    .await;
    let mut messages = match prepared {
        Ok(Ok(v)) => v,
        _ => {
            return finish_event(
                &state,
                event,
                Some(guard),
                None,
                "needs_review",
                vec![],
                Some("channel preparation failed or timed out".into()),
            )
            .await
        }
    };
    if execution_budget(&event.spec, now_ms()).is_zero() {
        return finish_event(
            &state,
            event,
            Some(guard),
            None,
            "needs_review",
            vec![],
            Some("interaction expired while preparing session".into()),
        )
        .await;
    }
    let request = ChatRequest {
        messages: messages.clone(),
        enabled_tools: event.spec.enabled_tools.clone(),
        enabled_skills: vec![],
        auto_skills: false,
        session_id: Some(event.spec.session_id.clone()),
    };
    match tokio::time::timeout_at(
        deadline,
        state.agent.chat_for(&request, ModelPurpose::Channel),
    )
    .await
    {
        Ok(Ok(response)) => {
            let failed = response.status != RunStatus::Completed
                || response
                    .tool_calls
                    .iter()
                    .any(|c| c.result.as_ref().is_some_and(|v| v.get("error").is_some()));
            let chunks = super::outbound::split_text_for(
                event.spec.destination.channel,
                &response.message.content,
            )
            .and_then(|chunks| {
                if event.spec.destination.channel == Channel::Discord && chunks.len() > 6 {
                    anyhow::bail!("Discord reply exceeds original plus five follow-ups");
                }
                Ok(chunks)
            });
            if failed || chunks.is_err() {
                messages.push(ChatMessage {
                    role: MessageRole::Assistant,
                    content: "Channel run requires operator review; no automatic reply or replay."
                        .into(),
                });
                finish_event(
                    &state,
                    event,
                    Some(guard),
                    Some(messages),
                    "needs_review",
                    vec![],
                    Some("agent incomplete, tool failed, or reply exceeded delivery limits".into()),
                )
                .await
            } else {
                messages.push(response.message);
                finish_event(
                    &state,
                    event,
                    Some(guard),
                    Some(messages),
                    "completed",
                    chunks.unwrap_or_default(),
                    None,
                )
                .await
            }
        }
        _ => {
            finish_event(
                &state,
                event,
                Some(guard),
                None,
                "needs_review",
                vec![],
                Some("agent failed or deadline expired; external outcome may be unknown".into()),
            )
            .await
        }
    }
}

async fn deliver(
    state: AppState,
    delivery: super::channel_store::ChannelDelivery,
) -> Result<(), AppError> {
    use super::outbound::DeliveryOutcome;
    let rt = runtime(&state)?;
    let authorized = match (&delivery.event_id, &delivery.job_run_id) {
        (Some(event_id), None) => {
            let id = event_id.clone();
            let event = super::with_sessions(&state, move |store| store.get_channel_event(&id))
                .await?
                .ok_or_else(|| AppError::Internal("delivery event missing".into()))?;
            event.spec.destination == delivery.destination && still_authorized(&state, &event.spec)
        }
        (None, Some(run_id)) => {
            let id = run_id.clone();
            let run = super::with_sessions(&state, move |store| store.get_job_run(&id))
                .await?
                .ok_or_else(|| AppError::Internal("delivery run missing".into()))?;
            run.spec
                .delivery
                .as_ref()
                .is_some_and(|d| d.destination() == delivery.destination)
                && scheduled_job_authorized(&state, &run.spec)
        }
        _ => return Err(AppError::Internal("invalid delivery source".into())),
    };
    let mut credential = None;
    let mut discord_meta = None;
    let mut outcome = DeliveryOutcome::Rejected {
        code: "authorization_changed",
    };
    let binding = rt.installation(delivery.destination.channel);
    if authorized {
        if delivery
            .destination
            .expires_ms
            .is_some_and(|v| v <= now_ms())
        {
            outcome = DeliveryOutcome::Rejected {
                code: "interaction_expired",
            };
        } else if let Some(b) = binding {
            if b.channel == Channel::Discord && delivery.job_run_id.is_some() {
                if let Some(sender) = &b.discord_sender {
                    let fingerprint = sender.credential_fingerprint();
                    let admission_hash = fingerprint.clone();
                    let admitted = super::with_sessions(&state, move |store| {
                        store.admit_discord_bot(&admission_hash)
                    })
                    .await?;
                    if admitted {
                        let result = sender
                            .send(&delivery.destination, &delivery.id, &delivery.text)
                            .await;
                        discord_meta = Some(super::channel_store::DiscordDeliveryMeta {
                            credential_hash: fingerprint,
                            cooldown_until_ms: result
                                .cooldown_ms
                                .map(|delay| now_ms().saturating_add(delay)),
                            credential_rejected: result.credential_rejected,
                        });
                        outcome = result.outcome;
                    } else {
                        outcome = DeliveryOutcome::Rejected {
                            code: "discord_bot_credential_blocked",
                        };
                    }
                } else {
                    outcome = DeliveryOutcome::Rejected {
                        code: "credential_unavailable",
                    };
                }
            } else if b.channel == Channel::Dingtalk {
                outcome = if let Some(sender) = &b.dingtalk_sender {
                    sender.send(&delivery.destination, &delivery.text).await
                } else {
                    DeliveryOutcome::Rejected {
                        code: "credential_unavailable",
                    }
                };
            } else if b.channel == Channel::Wecom {
                outcome = if let Some(sender) = &b.wecom_sender {
                    sender.send(&delivery.destination, &delivery.text).await
                } else {
                    DeliveryOutcome::Rejected {
                        code: "credential_unavailable",
                    }
                };
            } else if b.channel == Channel::Feishu {
                outcome = if let Some(sender) = &b.feishu_sender {
                    sender
                        .send(&delivery.destination, &delivery.id, &delivery.text)
                        .await
                } else {
                    DeliveryOutcome::Rejected {
                        code: "credential_unavailable",
                    }
                };
            } else {
                credential = if b.channel == Channel::Discord {
                    delivery
                        .sealed_token
                        .as_deref()
                        .and_then(|v| rt.unseal(v, &token_aad(&delivery.destination)).ok())
                } else {
                    Some(b.credential.clone())
                };
                if credential.is_none() {
                    outcome = DeliveryOutcome::Rejected {
                        code: "credential_unavailable",
                    };
                }
            }
        }
    }
    if let (Some(credential), Some(binding)) = (credential.as_deref(), binding) {
        outcome = rt
            .client
            .send(
                &delivery.destination,
                delivery.ordinal as usize,
                &delivery.text,
                credential,
                &binding.api_base,
            )
            .await;
    }
    let (status, receipt, error, retry) = match outcome {
        DeliveryOutcome::Delivered { receipt } => ("delivered", Some(receipt), None, None),
        DeliveryOutcome::RateLimited { retry_after_ms } => (
            "retry_wait",
            None,
            Some("platform_rate_limited".into()),
            Some(retry_after_ms),
        ),
        DeliveryOutcome::Unknown { code } => ("unknown", None, Some(code.into()), None),
        DeliveryOutcome::Rejected { code } => (
            if code == "interaction_expired" {
                "expired"
            } else {
                "permanent_failed"
            },
            None,
            Some(code.into()),
            None,
        ),
    };
    let completed = super::with_sessions(&state, move |store| {
        let completed_now = now_ms();
        let retry = retry.map(|delay| completed_now.saturating_add(delay));
        store.finish_channel_delivery_with_discord(
            &delivery.id,
            delivery.attempts,
            status,
            receipt,
            error,
            retry,
            completed_now,
            discord_meta,
        )
    })
    .await?;
    if !completed {
        return Err(AppError::Internal(
            "delivery completion no longer owned".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "channel_runtime_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "scheduled_runtime_tests.rs"]
mod scheduled_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;

    fn installation(channel: Channel) -> Installation {
        Installation {
            channel,
            policy: ChannelBinding {
                channel: channel_name(channel).into(),
                installation_id: "123456".into(),
                app_id: Some("A123".into()),
                discord_guild_id: None,
                allowed_senders: vec!["7".into()],
                allowed_conversations: vec!["99".into()],
                scheduled_destinations: vec![],
                enabled_tools: vec!["datetime_now".into()],
                timeout_secs: 30,
                local_test_api_base: None,
            },
            credential: "123456:test".into(),
            inbound_secret: "test-secret".into(),
            api_base: "https://api.telegram.org".into(),
            feishu_sender: None,
            feishu_verification_token: None,
            wecom_sender: None,
            wecom_callback: None,
            dingtalk_sender: None,
            dingtalk_callback: None,
            discord_sender: None,
        }
    }

    #[test]
    fn feishu_configuration_binds_app_and_tenant_and_requires_exact_proactive_target() {
        let mut config = AgentConfig::default();
        config.provider.provider_type = "stub".into();
        config.http.feishu_app_secret = Some("fixture-app-secret".into());
        config.http.feishu_encrypt_key = Some("fixture-encrypt-key".into());
        config.http.feishu_verification_token = Some("fixture-verify-token".into());
        let mut policy = installation(Channel::Feishu).policy;
        policy.app_id = None;
        policy.installation_id = "cli_fixture:tenant_fixture".into();
        policy.allowed_senders = vec!["ou_fixture".into()];
        policy.allowed_conversations = vec!["oc_fixture".into()];
        policy.scheduled_destinations = vec![jiaclaw_core::ScheduledChannelDestination {
            conversation_id: "oc_scheduled".into(),
            thread_id: Some("om_root".into()),
        }];
        config.http.channels = vec![policy];
        let runtime = ChannelRuntime::configure(&config, Some("owner"))
            .unwrap()
            .unwrap();
        let binding = runtime.installation(Channel::Feishu).unwrap();
        assert!(binding.feishu_sender.is_some());
        assert!(binding.credential.is_empty());
        assert_eq!(binding.policy.installation_id, "cli_fixture:tenant_fixture");
        assert!(ChannelRuntime::configure(&config, None).is_err());
        config.http.channels[0].app_id = Some("cli_another".into());
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].app_id = None;
        config.http.channels[0].scheduled_destinations[0].thread_id =
            Some("omt_native_thread".into());
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].scheduled_destinations[0].thread_id = None;
        config.http.channels[0].installation_id = "tenant_fixture".into();
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
    }

    #[test]
    fn wecom_configuration_requires_canonical_single_member_targets_and_callback_secrets() {
        let mut config = AgentConfig::default();
        config.provider.provider_type = "stub".into();
        config.http.wecom_app_secret = Some("fixture-secret".into());
        config.http.wecom_callback_token = Some("FixtureToken123".into());
        config.http.wecom_encoding_aes_key =
            Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG".into());
        let mut policy = installation(Channel::Wecom).policy;
        policy.app_id = None;
        policy.installation_id = "wwfixture:1000001".into();
        policy.allowed_senders = vec!["alice@example.com".into()];
        policy.allowed_conversations = vec!["alice@example.com".into()];
        policy.scheduled_destinations = vec![jiaclaw_core::ScheduledChannelDestination {
            conversation_id: "bob".into(),
            thread_id: None,
        }];
        config.http.channels = vec![policy];
        let runtime = ChannelRuntime::configure(&config, Some("owner"))
            .unwrap()
            .unwrap();
        let binding = runtime.installation(Channel::Wecom).unwrap();
        assert!(binding.wecom_sender.is_some() && binding.wecom_callback.is_some());
        assert!(binding.credential.is_empty() && binding.inbound_secret.is_empty());
        config.http.channels[0].allowed_senders = vec!["Alice@example.com".into()];
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].allowed_senders = vec!["alice@example.com".into()];
        config.http.channels[0].scheduled_destinations[0].conversation_id = "@all".into();
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].scheduled_destinations[0].conversation_id = "bob".into();
        config.http.channels[0].app_id = Some("1000002".into());
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].app_id = None;
        config.http.wecom_encoding_aes_key = Some("invalid".into());
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
    }
    #[test]
    fn dingtalk_configuration_binds_client_separately_and_preserves_member_case() {
        let mut config = AgentConfig::default();
        config.provider.provider_type = "stub".into();
        config.http.dingtalk_app_secret = Some("fixture-secret".into());
        let mut policy = installation(Channel::Dingtalk).policy;
        policy.installation_id = "dingRobot:dingCorp".into();
        policy.app_id = Some("dingDistinctClient".into());
        policy.allowed_senders = vec!["Alice@example.com".into()];
        policy.allowed_conversations = vec!["Alice@example.com".into()];
        policy.scheduled_destinations = vec![jiaclaw_core::ScheduledChannelDestination {
            conversation_id: "Bob".into(),
            thread_id: None,
        }];
        config.http.channels = vec![policy];
        let runtime = ChannelRuntime::configure(&config, Some("owner"))
            .unwrap()
            .unwrap();
        let binding = runtime.installation(Channel::Dingtalk).unwrap();
        assert!(binding.dingtalk_sender.is_some() && binding.dingtalk_callback.is_some());
        assert!(binding.credential.is_empty() && binding.inbound_secret.is_empty());
        config.http.channels[0].app_id = None;
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].app_id = Some("dingDistinctClient".into());
        config.http.channels[0].allowed_senders = vec!["@all".into()];
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].allowed_senders = vec!["Alice@example.com".into()];
        config.http.channels[0].scheduled_destinations[0].thread_id = Some("thread".into());
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
        config.http.channels[0].scheduled_destinations[0].thread_id = None;
        config.http.channels[0].local_test_api_base = Some("https://attacker.invalid".into());
        assert!(ChannelRuntime::configure(&config, Some("owner")).is_err());
    }

    fn state() -> (AppState, std::path::PathBuf) {
        let workspace =
            std::env::temp_dir().join(format!("jiaclaw-channel-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        state.persist_enabled = true;
        state.api_token = Some("test-owner".into());
        state.sessions = Arc::new(std::sync::Mutex::new(
            SessionStore::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        state.channel_runtime = Some(Arc::new(ChannelRuntime {
            installations: vec![installation(Channel::Telegram)],
            client: OutboundClient::new().unwrap(),
            cipher: None,
            health: AtomicU8::new(1),
        }));
        (state, workspace)
    }
    async fn ingress(
        state: AppState,
        secret: Option<&str>,
        sender: u64,
        conversation: i64,
        content: &str,
        thread: Option<Value>,
    ) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(secret) = secret {
            headers.insert("x-telegram-bot-api-secret-token", secret.parse().unwrap());
        }
        let mut body = json!({"update_id":100,"message":{"from":{"id":sender},"chat":{"id":conversation},"text":content}});
        if let Some(thread) = thread {
            body["message"]["message_thread_id"] = thread;
        }
        match telegram(State(state), headers, Bytes::from(body.to_string())).await {
            Ok(v) => v,
            Err(e) => e.into_response(),
        }
    }

    #[tokio::test]
    async fn ingress_auth_authorization_and_destination_fail_before_admission() {
        let (state, path) = state();
        for (secret, sender, chat, thread, status) in [
            (None, 7, 99, None, StatusCode::UNAUTHORIZED),
            (Some("wrong"), 7, 99, None, StatusCode::UNAUTHORIZED),
            (Some("test-secret"), 8, 99, None, StatusCode::UNAUTHORIZED),
            (Some("test-secret"), 7, 98, None, StatusCode::UNAUTHORIZED),
            (
                Some("test-secret"),
                7,
                99,
                Some(json!("../secret")),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            assert_eq!(
                ingress(state.clone(), secret, sender, chat, "hello", thread)
                    .await
                    .status(),
                status
            );
        }
        assert!(state
            .sessions
            .lock()
            .unwrap()
            .list_channel_events(100, 0)
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn repeated_ingress_has_one_event_and_payload_conflict_is_rejected() {
        let (state, path) = state();
        for duplicate in [false, true] {
            let response = ingress(state.clone(), Some("test-secret"), 7, 99, "hello", None).await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["duplicate"],
                duplicate
            );
        }
        assert_eq!(
            ingress(state.clone(), Some("test-secret"), 7, 99, "different", None)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            state
                .sessions
                .lock()
                .unwrap()
                .list_channel_events(100, 0)
                .unwrap()
                .len(),
            1
        );
        state
            .channel_runtime
            .as_ref()
            .unwrap()
            .health
            .store(2, Ordering::Release);
        assert_eq!(
            ingress(state.clone(), Some("test-secret"), 7, 99, "hello", None)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn encrypted_interaction_credentials_are_bound_to_installation_and_event() {
        let rt = ChannelRuntime {
            installations: vec![],
            client: OutboundClient::new().unwrap(),
            cipher: Some(aead::LessSafeKey::new(
                aead::UnboundKey::new(&aead::AES_256_GCM, &[23u8; 32]).unwrap(),
            )),
            health: AtomicU8::new(0),
        };
        let first = rt
            .seal("private-interaction-token", "discord:123:456")
            .unwrap();
        let second = rt
            .seal("private-interaction-token", "discord:123:456")
            .unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("private-interaction-token"));
        assert_eq!(
            rt.unseal(&first, "discord:123:456").unwrap(),
            "private-interaction-token"
        );
        assert!(rt.unseal(&first, "discord:123:457").is_err());
        assert!(rt.unseal("00", "discord:123:456").is_err());
        let mut altered = first.into_bytes();
        altered[0] = if altered[0] == b'0' { b'1' } else { b'0' };
        assert!(rt
            .unseal(&String::from_utf8(altered).unwrap(), "discord:123:456")
            .is_err());
    }

    #[test]
    fn signed_slack_body_cannot_be_replaced_or_replayed_outside_window() {
        let body = br#"{"event_id":"E123"}"#;
        let signature = super::super::slack_v0_signature("secret", "1700000000", body).unwrap();
        assert!(super::super::verify_slack_request(
            "secret",
            Some("1700000000"),
            Some(&signature),
            body,
            1_700_000_001
        ));
        assert!(!super::super::verify_slack_request(
            "secret",
            Some("1700000000"),
            Some(&signature),
            b"{}",
            1_700_000_001
        ));
        assert!(!super::super::verify_slack_request(
            "secret",
            Some("1700000000"),
            Some(&signature),
            body,
            1_700_000_301
        ));
    }
}
