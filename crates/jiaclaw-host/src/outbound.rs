// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A single attempt at delivering one bounded plain-text message part.
//!
//! An interrupted send has an unknown outcome: callers must persist that fact
//! before allowing another worker to claim the delivery. Dropping this future
//! cannot establish that the platform did not accept the message.
//!
//! Platform contracts:
//! - <https://core.telegram.org/bots/api#sendmessage>
//! - <https://docs.slack.dev/reference/methods/chat.postMessage/>
//! - <https://docs.slack.dev/apis/web-api/rate-limits/>
//! - <https://docs.slack.dev/messaging/formatting-message-text/#escaping-text>
//! - <https://docs.discord.com/developers/interactions/receiving-and-responding>
//! - <https://docs.discord.com/developers/topics/rate-limits>
//! - <https://docs.discord.com/developers/reference#user-agent>

use std::time::Duration;

use anyhow::{bail, Result};
use reqwest::{header::HeaderMap, Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::json;

use crate::channel_types::{Channel, Destination};

const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_PARTS: usize = 16;
const MAX_PART_UTF16: usize = 2000;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeliveryOutcome {
    Delivered { receipt: String },
    RateLimited { retry_after_ms: i64 },
    Unknown { code: &'static str },
    Rejected { code: &'static str },
}

#[derive(Clone)]
pub(super) struct OutboundClient {
    client: Client,
    allow_loopback: bool,
}

impl OutboundClient {
    pub(super) fn new() -> Result<Self> {
        Self::new_with_loopback(false)
    }

    /// Loopback is an explicit test/deployment option, never an environment override.
    pub(super) fn new_with_loopback(allow_loopback: bool) -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|_| anyhow::anyhow!("failed to initialize outbound client"))?;
        Ok(Self {
            client,
            allow_loopback,
        })
    }

    #[allow(clippy::too_many_lines)] // Keep the three wire contracts and one attempt together.
    pub(super) async fn send(
        &self,
        destination: &Destination,
        part_index: usize,
        text: &str,
        credential: &str,
        api_base: &str,
    ) -> DeliveryOutcome {
        if validate_destination(destination).is_err() {
            return rejected("invalid_destination");
        }
        if text.is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.encode_utf16().count() > MAX_PART_UTF16
            || part_index >= MAX_PARTS
        {
            return rejected("invalid_text");
        }
        if !valid_credential(destination.channel, credential) {
            return rejected("invalid_credential");
        }
        if validate_api_base(destination.channel, api_base, self.allow_loopback).is_err() {
            return rejected("invalid_api_base");
        }

        // Every URL component is checked before constructing the request. Neither
        // URL-bearing reqwest errors nor response bodies are logged or returned.
        let base = api_base.trim_end_matches('/');
        let request = match destination.channel {
            Channel::Telegram => {
                let mut body = json!({
                    "chat_id": destination.conversation_id,
                    "text": text,
                    "link_preview_options": {"is_disabled": true}
                });
                if let Some(thread) = &destination.thread_id {
                    // validate_destination already checked the integer representation.
                    body["message_thread_id"] = json!(thread.parse::<i64>().unwrap());
                }
                self.client
                    .post(format!("{base}/bot{credential}/sendMessage"))
                    .json(&body)
            }
            Channel::Slack => {
                let mut body = json!({
                    "channel": destination.conversation_id,
                    // Escape Slack's control characters even with mrkdwn disabled:
                    // manual mention/link syntax must remain literal user text.
                    "text": text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"),
                    "mrkdwn": false,
                    "parse": "none",
                    "link_names": false,
                    "unfurl_links": false,
                    "unfurl_media": false
                });
                if let Some(thread) = &destination.thread_id {
                    body["thread_ts"] = json!(thread);
                }
                self.client
                    .post(format!("{base}/chat.postMessage"))
                    .bearer_auth(credential)
                    .json(&body)
            }
            Channel::Discord => {
                let url = format!(
                    "{base}/webhooks/{}/{credential}",
                    destination.installation_id
                );
                let request = if part_index == 0 {
                    self.client.patch(format!("{url}/messages/@original"))
                } else {
                    self.client.post(format!("{url}?wait=true"))
                };
                request
                    .header(
                        reqwest::header::USER_AGENT,
                        concat!(
                            "DiscordBot (https://github.com/jiawenyao401/JiaClaw, ",
                            env!("CARGO_PKG_VERSION"),
                            ")"
                        ),
                    )
                    .json(&json!({
                        "content": text,
                        "allowed_mentions": {"parse": [], "replied_user": false}
                    }))
            }
        };
        let Ok(mut response) = request.send().await else {
            return unknown("transport_error");
        };
        let status = response.status();
        if status.is_server_error() {
            return unknown("http_server_error");
        }
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            return rejected("http_client_error");
        }
        if !status.is_success() && status != StatusCode::TOO_MANY_REQUESTS {
            return unknown("unexpected_http_status");
        }
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return unknown("response_too_large");
        }
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                        return unknown("response_too_large");
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return unknown("response_read_error"),
            }
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return retry_delay(destination.channel, &headers, &body).map_or_else(
                || unknown("invalid_rate_limit"),
                |retry_after_ms| DeliveryOutcome::RateLimited { retry_after_ms },
            );
        }
        parse_receipt(destination, &body)
    }
}

fn unknown(code: &'static str) -> DeliveryOutcome {
    DeliveryOutcome::Unknown { code }
}

fn rejected(code: &'static str) -> DeliveryOutcome {
    DeliveryOutcome::Rejected { code }
}

/// Retains every Unicode scalar and byte, or rejects the whole input before send.
pub(super) fn split_text(text: &str) -> Result<Vec<String>> {
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        bail!("outbound text must contain 1..=16384 UTF-8 bytes");
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (offset, character) in text.char_indices() {
        if units + character.len_utf16() > MAX_PART_UTF16 {
            parts.push(text[start..offset].to_owned());
            start = offset;
            units = 0;
        }
        units += character.len_utf16();
    }
    parts.push(text[start..].to_owned());
    if parts.len() > MAX_PARTS {
        bail!("outbound text exceeds 16 parts");
    }
    Ok(parts)
}

pub(super) fn validate_destination(destination: &Destination) -> Result<()> {
    let valid = match destination.channel {
        Channel::Telegram => {
            telegram_chat_id(&destination.conversation_id).is_some()
                && destination
                    .thread_id
                    .as_deref()
                    .is_none_or(|id| positive_id(id).is_some_and(|id| i32::try_from(id).is_ok()))
                && destination.interaction_id.is_none()
        }
        Channel::Slack => {
            slack_channel_id(&destination.conversation_id)
                && destination.thread_id.as_deref().is_none_or(slack_timestamp)
                && destination.interaction_id.is_none()
        }
        Channel::Discord => {
            positive_id(&destination.installation_id).is_some()
                && positive_id(&destination.conversation_id).is_some()
                && destination
                    .interaction_id
                    .as_deref()
                    .is_some_and(|id| positive_id(id).is_some())
                && destination.thread_id.is_none()
        }
    };
    if !valid {
        bail!("invalid outbound destination");
    }
    Ok(())
}

pub(super) fn validate_api_base(
    channel: Channel,
    api_base: &str,
    allow_loopback: bool,
) -> Result<()> {
    let official = match channel {
        Channel::Telegram => "https://api.telegram.org",
        Channel::Slack => "https://slack.com/api",
        Channel::Discord => "https://discord.com/api/v10",
    };
    if api_base == official || api_base == format!("{official}/") {
        return Ok(());
    }
    let url = Url::parse(api_base).map_err(|_| anyhow::anyhow!("invalid outbound API base"))?;
    let authority = api_base
        .strip_prefix("http://")
        .or_else(|| api_base.strip_prefix("https://"))
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default();
    let literal_loopback = authority == "127.0.0.1"
        || authority == "[::1]"
        || authority.strip_prefix("127.0.0.1:").is_some_and(port)
        || authority.strip_prefix("[::1]:").is_some_and(port);
    let platform_path = match channel {
        Channel::Telegram => "/",
        Channel::Slack => "/api",
        Channel::Discord => "/api/v10",
    };
    if !allow_loopback
        || !literal_loopback
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.path() == "/"
            || url.path() == platform_path
            || url.path() == format!("{platform_path}/"))
        || api_base.chars().any(char::is_whitespace)
        || api_base.contains('%')
        || api_base.contains('\\')
    {
        bail!("outbound API base must be official HTTPS or explicitly enabled literal loopback");
    }
    Ok(())
}

fn port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u16>().is_ok_and(|p| p > 0)
}

fn positive_id(value: &str) -> Option<u64> {
    if value.is_empty()
        || value.len() > 20
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

fn telegram_chat_id(value: &str) -> Option<i64> {
    let magnitude = positive_id(value.strip_prefix('-').unwrap_or(value))?;
    if magnitude >= (1_u64 << 52) {
        return None;
    }
    value.parse().ok()
}

fn slack_channel_id(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && matches!(value.as_bytes()[0], b'C' | b'G' | b'D')
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn slack_timestamp(value: &str) -> bool {
    value.split_once('.').is_some_and(|(seconds, fraction)| {
        positive_id(seconds).is_some()
            && seconds.len() <= 16
            && fraction.len() == 6
            && fraction.bytes().all(|b| b.is_ascii_digit())
    })
}

fn valid_credential(channel: Channel, credential: &str) -> bool {
    if credential.is_empty() || credential.len() > 2048 {
        return false;
    }
    let safe_token = |token: &str| {
        !token.is_empty()
            && !matches!(token, "." | "..")
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    match channel {
        Channel::Telegram => credential
            .split_once(':')
            .is_some_and(|(bot, token)| positive_id(bot).is_some() && safe_token(token)),
        Channel::Slack | Channel::Discord => safe_token(credential),
    }
}

#[derive(Deserialize)]
struct TelegramEnvelope {
    ok: bool,
    result: Option<TelegramMessage>,
    error_code: Option<u16>,
    parameters: Option<TelegramParameters>,
}

#[derive(Deserialize)]
struct TelegramMessage {
    message_id: i64,
    chat: TelegramChat,
}

#[derive(Deserialize)]
struct TelegramChat {
    id: i64,
}

#[derive(Deserialize)]
struct TelegramParameters {
    retry_after: i64,
}

#[derive(Deserialize)]
struct SlackEnvelope {
    ok: bool,
    channel: Option<String>,
    ts: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct DiscordMessage {
    id: String,
    channel_id: String,
}

fn parse_receipt(destination: &Destination, body: &[u8]) -> DeliveryOutcome {
    match destination.channel {
        Channel::Telegram => {
            let Ok(envelope) = serde_json::from_slice::<TelegramEnvelope>(body) else {
                return unknown("invalid_response");
            };
            if !envelope.ok {
                return unknown("telegram_error");
            }
            if envelope.error_code.is_some() || envelope.parameters.is_some() {
                return unknown("invalid_response");
            }
            if let Some(message) = envelope.result {
                if message.message_id > 0
                    && Some(message.chat.id) == telegram_chat_id(&destination.conversation_id)
                {
                    return DeliveryOutcome::Delivered {
                        receipt: message.message_id.to_string(),
                    };
                }
            }
        }
        Channel::Slack => {
            let Ok(envelope) = serde_json::from_slice::<SlackEnvelope>(body) else {
                return unknown("invalid_response");
            };
            if !envelope.ok {
                return if envelope.error.as_deref().is_some_and(slack_rejected_error) {
                    rejected("slack_rejected")
                } else {
                    unknown("slack_error")
                };
            }
            if envelope.error.is_some() {
                return unknown("invalid_response");
            }
            if envelope.channel.as_deref() == Some(destination.conversation_id.as_str()) {
                if let Some(ts) = envelope.ts.filter(|ts| slack_timestamp(ts)) {
                    return DeliveryOutcome::Delivered { receipt: ts };
                }
            }
        }
        Channel::Discord => {
            let Ok(message) = serde_json::from_slice::<DiscordMessage>(body) else {
                return unknown("invalid_response");
            };
            if message.channel_id == destination.conversation_id
                && positive_id(&message.id).is_some()
            {
                return DeliveryOutcome::Delivered {
                    receipt: message.id,
                };
            }
        }
    }
    unknown("invalid_receipt")
}

// Deliberately conservative subset of documented pre-delivery failures. Slack's
// internal_error/fatal_error explicitly allow partial success; new error codes
// must not silently acquire retry-safe semantics.
fn slack_rejected_error(error: &str) -> bool {
    matches!(
        error,
        "access_denied"
            | "account_inactive"
            | "app_access_restricted"
            | "channel_not_found"
            | "ekm_access_denied"
            | "invalid_auth"
            | "invalid_arguments"
            | "is_archived"
            | "missing_scope"
            | "no_permission"
            | "no_text"
            | "not_allowed_token_type"
            | "not_authed"
            | "not_in_channel"
            | "restricted_action"
            | "restricted_action_non_threadable_channel"
            | "restricted_action_read_only_channel"
            | "restricted_action_thread_locked"
            | "restricted_action_thread_only_channel"
            | "team_access_not_granted"
            | "token_expired"
            | "token_revoked"
    )
}

fn retry_delay(channel: Channel, headers: &HeaderMap, body: &[u8]) -> Option<i64> {
    match channel {
        Channel::Telegram => {
            let envelope: TelegramEnvelope = serde_json::from_slice(body).ok()?;
            if envelope.ok || envelope.error_code != Some(429) || envelope.result.is_some() {
                return None;
            }
            let seconds = envelope.parameters?.retry_after;
            (1..=3600).contains(&seconds).then(|| seconds * 1000)
        }
        Channel::Slack => {
            let mut values = headers.get_all(reqwest::header::RETRY_AFTER).iter();
            let value = values.next()?.to_str().ok()?;
            if values.next().is_some() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let seconds = value.parse::<i64>().ok()?;
            (1..=3600).contains(&seconds).then(|| seconds * 1000)
        }
        Channel::Discord => {
            #[derive(Deserialize)]
            struct RateLimit {
                retry_after: f64,
                #[serde(rename = "global")]
                _global: bool,
            }
            let rate: RateLimit = serde_json::from_slice(body).ok()?;
            let seconds = rate.retry_after;
            if !seconds.is_finite() || seconds <= 0.0 || seconds > 3600.0 {
                return None;
            }
            // Round up, so sub-millisecond precision never shortens a server delay.
            #[allow(clippy::cast_possible_truncation)]
            Some((seconds * 1000.0).ceil() as i64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
        task::JoinHandle,
    };

    fn destination(channel: Channel) -> Destination {
        Destination {
            channel,
            installation_id: "123456789012345678".to_owned(),
            conversation_id: match channel {
                Channel::Telegram => "-123456789",
                Channel::Slack => "C123ABC456",
                Channel::Discord => "234567890123456789",
            }
            .to_owned(),
            thread_id: None,
            interaction_id: (channel == Channel::Discord).then(|| "345678901234567890".to_owned()),
            expires_ms: None,
        }
    }

    fn credential(channel: Channel) -> &'static str {
        match channel {
            Channel::Telegram => "123456:secret_TG-token",
            Channel::Slack => "xoxb-test-token",
            Channel::Discord => "interaction-secret_token",
        }
    }

    struct Fixture {
        base: String,
        request: oneshot::Receiver<Vec<u8>>,
        task: JoinHandle<()>,
    }

    // None closes after consuming the request, Some(empty) leaves it unanswered.
    async fn fixture(response: Option<Vec<u8>>) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (sent, request) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "request ended before full body");
                request.extend_from_slice(&buffer[..read]);
                assert!(request.len() < 64 * 1024);
                if let Some(offset) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..offset]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= offset + 4 + length {
                        break;
                    }
                }
            }
            let _ = sent.send(request);
            if let Some(response) = response {
                if response.is_empty() {
                    // Read observes the client's cancellation/timeout socket close.
                    let _ = socket.read(&mut buffer).await;
                } else {
                    let _ = socket.write_all(&response).await;
                    let _ = socket.shutdown().await;
                }
            }
        });
        Fixture {
            base,
            request,
            task,
        }
    }

    fn response(status: u16, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    async fn send_fixture(
        channel: Channel,
        status: u16,
        headers: &str,
        body: &str,
    ) -> DeliveryOutcome {
        let server = fixture(Some(response(status, headers, body))).await;
        let outcome = OutboundClient::new_with_loopback(true)
            .unwrap()
            .send(
                &destination(channel),
                0,
                "hello",
                credential(channel),
                &server.base,
            )
            .await;
        server.task.await.unwrap();
        outcome
    }

    #[test]
    fn splits_utf16_without_losing_unicode_or_whitespace() {
        for input in [
            "a".repeat(16 * 1024),
            "🙂".repeat(4096),
            format!("{}🙂 e\u{301}\n  尾", "a".repeat(1999)),
        ] {
            let parts = split_text(&input).unwrap();
            assert_eq!(parts.concat(), input);
            assert!(parts.iter().all(|part| part.encode_utf16().count() <= 2000));
            assert!(parts.iter().all(|part| !part.is_empty()));
            assert!(parts.len() <= 16);
        }
        let parts = split_text(&format!("{}🙂", "a".repeat(1999))).unwrap();
        assert_eq!(parts, vec!["a".repeat(1999), "🙂".to_owned()]);
        assert!(split_text("").is_err());
        assert!(split_text(&"a".repeat(16 * 1024 + 1)).is_err());
    }

    #[test]
    fn validates_platform_ids_and_rejects_injection() {
        for channel in [Channel::Telegram, Channel::Slack, Channel::Discord] {
            let mut target = destination(channel);
            assert!(validate_destination(&target).is_ok());
            for invalid in [
                "",
                "0",
                "../messages",
                "1?wait=false",
                "+123",
                "00123",
                "123\n",
            ] {
                target.conversation_id = invalid.to_owned();
                assert!(
                    validate_destination(&target).is_err(),
                    "{channel:?}: {invalid:?}"
                );
            }
            for invalid in [
                "",
                "..",
                ".",
                "token/other",
                "token?x",
                "token#x",
                "token\r\nx:y",
            ] {
                assert!(!valid_credential(channel, invalid));
            }
        }
        let mut telegram = destination(Channel::Telegram);
        telegram.thread_id = Some("42".to_owned());
        assert!(validate_destination(&telegram).is_ok());
        telegram.thread_id = Some("-42".to_owned());
        assert!(validate_destination(&telegram).is_err());
        let mut slack = destination(Channel::Slack);
        slack.thread_id = Some("1234567890.000001".to_owned());
        assert!(validate_destination(&slack).is_ok());
        slack.thread_id = Some("1234567890.1".to_owned());
        assert!(validate_destination(&slack).is_err());
        let mut discord = destination(Channel::Discord);
        discord.installation_id = "123/456".to_owned();
        assert!(validate_destination(&discord).is_err());
    }

    #[test]
    fn api_bases_are_official_or_explicit_literal_loopback() {
        for (channel, official) in [
            (Channel::Telegram, "https://api.telegram.org"),
            (Channel::Slack, "https://slack.com/api"),
            (Channel::Discord, "https://discord.com/api/v10"),
        ] {
            assert!(validate_api_base(channel, official, false).is_ok());
            assert!(validate_api_base(channel, &format!("{official}/"), false).is_ok());
            for base in [
                "http://127.0.0.1:8080",
                "https://[::1]:8080",
                "http://127.0.0.1",
            ] {
                assert!(validate_api_base(channel, base, true).is_ok());
                assert!(validate_api_base(channel, base, false).is_err());
            }
            for base in [
                "http://localhost:8080",
                "http://127.1:8080",
                "http://2130706433",
                "http://0x7f000001",
                "http://127.0.0.1:0",
                "http://192.168.1.1",
                "http://user:secret@127.0.0.1",
                "http://127.0.0.1?x=y",
                "http://127.0.0.1/#x",
                "http://127.0.0.1/other",
                "http://127.0.0.1/%2e%2e",
                "http://127.0.0.1\\evil",
                "https://evil.example",
                "https://api.telegram.org.evil.example",
                "https://api.telegram.org@evil.example",
                "file:///tmp/test",
            ] {
                assert!(validate_api_base(channel, base, true).is_err(), "{base}");
            }
        }
    }

    #[tokio::test]
    async fn successful_requests_preserve_threads_and_disable_mentions() {
        for (channel, body, expected_receipt) in [
            (
                Channel::Telegram,
                r#"{"ok":true,"result":{"message_id":42,"chat":{"id":-123456789}}}"#,
                "42",
            ),
            (
                Channel::Slack,
                r#"{"ok":true,"channel":"C123ABC456","ts":"1234567890.000001"}"#,
                "1234567890.000001",
            ),
            (
                Channel::Discord,
                r#"{"id":"456789012345678901","channel_id":"234567890123456789"}"#,
                "456789012345678901",
            ),
        ] {
            for part_index in [0, 1] {
                let mut target = destination(channel);
                target.thread_id = match channel {
                    Channel::Telegram => Some("42".to_owned()),
                    Channel::Slack => Some("1234567890.000002".to_owned()),
                    Channel::Discord => None,
                };
                let server = fixture(Some(response(200, "", body))).await;
                let outcome = OutboundClient::new_with_loopback(true)
                    .unwrap()
                    .send(
                        &target,
                        part_index,
                        "@everyone <!here> <@U123> 中文🙂",
                        credential(channel),
                        &server.base,
                    )
                    .await;
                assert_eq!(
                    outcome,
                    DeliveryOutcome::Delivered {
                        receipt: expected_receipt.to_owned()
                    }
                );
                let request = String::from_utf8(server.request.await.unwrap()).unwrap();
                let (headers, body) = request.split_once("\r\n\r\n").unwrap();
                let json: serde_json::Value = serde_json::from_str(body).unwrap();
                match channel {
                    Channel::Telegram => {
                        assert!(headers
                            .starts_with("POST /bot123456:secret_TG-token/sendMessage HTTP/1.1"));
                        assert_eq!(json["chat_id"], "-123456789");
                        assert_eq!(json["message_thread_id"], 42);
                        assert!(json.get("parse_mode").is_none());
                    }
                    Channel::Slack => {
                        assert!(headers.starts_with("POST /chat.postMessage HTTP/1.1"));
                        assert!(headers
                            .to_ascii_lowercase()
                            .contains("authorization: bearer xoxb-test-token"));
                        assert_eq!(json["thread_ts"], "1234567890.000002");
                        assert_eq!(json["mrkdwn"], false);
                        assert_eq!(json["text"], "@everyone &lt;!here&gt; &lt;@U123&gt; 中文🙂");
                        assert_eq!(json["parse"], "none");
                        for field in ["link_names", "unfurl_links", "unfurl_media"] {
                            assert_eq!(json[field], false);
                        }
                    }
                    Channel::Discord => {
                        let endpoint = if part_index == 0 {
                            "PATCH /webhooks/123456789012345678/interaction-secret_token/messages/@original HTTP/1.1"
                        } else {
                            "POST /webhooks/123456789012345678/interaction-secret_token?wait=true HTTP/1.1"
                        };
                        assert!(headers.starts_with(endpoint));
                        assert!(!headers.to_ascii_lowercase().contains("authorization:"));
                        assert!(headers.to_ascii_lowercase().contains(
                            "user-agent: discordbot (https://github.com/jiawenyao401/jiaclaw, "
                        ));
                        assert_eq!(json["allowed_mentions"]["parse"], json!([]));
                        assert_eq!(json["allowed_mentions"]["replied_user"], false);
                    }
                }
                server.task.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn success_requires_typed_matching_receipts() {
        for (channel, bodies) in [
            (
                Channel::Telegram,
                vec![
                    "{}",
                    "true",
                    r#"{"ok":true}"#,
                    r#"{"ok":true,"ok":false,"result":{"message_id":42,"chat":{"id":-123456789}}}"#,
                    r#"{"ok":true,"result":{"message_id":42,"chat":{"id":42}}}"#,
                    r#"{"ok":true,"result":{"message_id":0,"chat":{"id":-123456789}}}"#,
                    r#"{"ok":true,"result":{"message_id":"42","chat":{"id":-123456789}}}"#,
                    r#"{"ok":false,"error_code":500}"#,
                ],
            ),
            (
                Channel::Slack,
                vec![
                    "{}",
                    r#"{"ok":"true","channel":"C123ABC456","ts":"1234567890.000001"}"#,
                    r#"{"ok":true,"channel":"C_OTHER","ts":"1234567890.000001"}"#,
                    r#"{"ok":true,"channel":"C123ABC456","ts":"1234567890.1"}"#,
                    r#"{"ok":true,"channel":"C123ABC456","ts":"1234567890.000001","error":"internal_error"}"#,
                ],
            ),
            (
                Channel::Discord,
                vec![
                    "{}",
                    r#"{"id":"456789012345678901","channel_id":"234567890123456780"}"#,
                    r#"{"id":456789012345678901,"channel_id":"234567890123456789"}"#,
                    r#"{"id":"../x","channel_id":"234567890123456789"}"#,
                    r#"{"id":"0","channel_id":"234567890123456789"}"#,
                ],
            ),
        ] {
            for body in bodies {
                assert!(
                    matches!(
                        send_fixture(channel, 200, "", body).await,
                        DeliveryOutcome::Unknown { .. }
                    ),
                    "{channel:?}: {body}"
                );
            }
            assert!(matches!(
                send_fixture(channel, 204, "", "").await,
                DeliveryOutcome::Unknown { .. }
            ));
        }
    }

    #[tokio::test]
    async fn explicit_errors_are_classified_conservatively() {
        for channel in [Channel::Telegram, Channel::Slack, Channel::Discord] {
            for status in [400, 401, 403, 404, 422] {
                assert_eq!(
                    send_fixture(channel, status, "", "secret error body").await,
                    rejected("http_client_error")
                );
            }
            for status in [500, 502, 503] {
                assert_eq!(
                    send_fixture(channel, status, "", "secret error body").await,
                    unknown("http_server_error")
                );
            }
        }
        for error in [
            "invalid_auth",
            "not_in_channel",
            "missing_scope",
            "is_archived",
        ] {
            assert_eq!(
                send_fixture(
                    Channel::Slack,
                    200,
                    "",
                    &json!({"ok":false,"error":error}).to_string()
                )
                .await,
                rejected("slack_rejected")
            );
        }
        for error in [
            "internal_error",
            "fatal_error",
            "service_unavailable",
            "new_error",
            "ratelimited",
        ] {
            assert_eq!(
                send_fixture(
                    Channel::Slack,
                    200,
                    "",
                    &json!({"ok":false,"error":error}).to_string()
                )
                .await,
                unknown("slack_error")
            );
        }
    }

    #[tokio::test]
    async fn rate_limits_require_unambiguous_bounded_platform_delays() {
        assert_eq!(
            send_fixture(
                Channel::Telegram,
                429,
                "",
                r#"{"ok":false,"error_code":429,"parameters":{"retry_after":2}}"#
            )
            .await,
            DeliveryOutcome::RateLimited {
                retry_after_ms: 2000
            }
        );
        assert_eq!(
            send_fixture(Channel::Slack, 429, "Retry-After: 3600\r\n", "").await,
            DeliveryOutcome::RateLimited {
                retry_after_ms: 3_600_000
            }
        );
        assert_eq!(
            send_fixture(
                Channel::Discord,
                429,
                "",
                r#"{"retry_after":0.0001,"global":false}"#
            )
            .await,
            DeliveryOutcome::RateLimited { retry_after_ms: 1 }
        );
        assert_eq!(
            send_fixture(
                Channel::Discord,
                429,
                "",
                r#"{"retry_after":1.2341,"global":true}"#
            )
            .await,
            DeliveryOutcome::RateLimited {
                retry_after_ms: 1235
            }
        );
        for seconds in ["0", "-1", "3601", "9223372036854775807", "\"2\"", "1.5"] {
            let body = format!(
                r#"{{"ok":false,"error_code":429,"parameters":{{"retry_after":{seconds}}}}}"#
            );
            assert_eq!(
                send_fixture(Channel::Telegram, 429, "", &body).await,
                unknown("invalid_rate_limit")
            );
        }
        for header in [
            "",
            "Retry-After: 0\r\n",
            "Retry-After: 3601\r\n",
            "Retry-After: 9223372036854775807\r\n",
            "Retry-After: 1.5\r\n",
            "Retry-After: 2\r\nRetry-After: 3\r\n",
            "Retry-After: Thu, 01 Jan 1970 00:00:01 GMT\r\n",
        ] {
            assert_eq!(
                send_fixture(Channel::Slack, 429, header, "{}").await,
                unknown("invalid_rate_limit")
            );
        }
        for body in [
            "{}",
            r#"{"retry_after":1}"#,
            r#"{"retry_after":"2","global":false}"#,
            r#"{"retry_after":0,"global":false}"#,
            r#"{"retry_after":-1,"global":false}"#,
            r#"{"retry_after":3600.001,"global":false}"#,
            r#"{"retry_after":1e100,"global":false}"#,
        ] {
            assert_eq!(
                send_fixture(Channel::Discord, 429, "", body).await,
                unknown("invalid_rate_limit")
            );
        }
    }

    #[tokio::test]
    async fn disconnected_or_oversized_responses_are_unknown() {
        let client = OutboundClient::new_with_loopback(true).unwrap();
        for raw in [
            None,
            Some(b"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n{".to_vec()),
        ] {
            let server = fixture(raw).await;
            assert!(matches!(
                client
                    .send(
                        &destination(Channel::Slack),
                        0,
                        "hello",
                        credential(Channel::Slack),
                        &server.base
                    )
                    .await,
                DeliveryOutcome::Unknown { .. }
            ));
            server.task.await.unwrap();
        }
        assert_eq!(
            send_fixture(Channel::Slack, 200, "", &"x".repeat(MAX_RESPONSE_BYTES + 1)).await,
            unknown("response_too_large")
        );
        let large_chunk = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let raw = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{large_chunk}\r\n0\r\n\r\n", large_chunk.len());
        let server = fixture(Some(raw.into_bytes())).await;
        assert_eq!(
            client
                .send(
                    &destination(Channel::Slack),
                    0,
                    "hello",
                    credential(Channel::Slack),
                    &server.base
                )
                .await,
            unknown("response_too_large")
        );
        server.task.await.unwrap();
    }

    #[tokio::test]
    async fn redirects_are_not_followed_or_given_credentials() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!(
            "Location: http://{}/capture\r\n",
            target.local_addr().unwrap()
        );
        for status in [301, 302, 307, 308] {
            assert_eq!(
                send_fixture(Channel::Slack, status, &location, "").await,
                unknown("unexpected_http_status")
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(30), target.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn timeout_returns_unknown_and_cancellation_stops_the_request() {
        for cancel in [false, true] {
            let server = fixture(Some(Vec::new())).await;
            let base = server.base.clone();
            let task = tokio::spawn(async move {
                OutboundClient::new_with_loopback(true)
                    .unwrap()
                    .send(
                        &destination(Channel::Slack),
                        0,
                        "hello",
                        credential(Channel::Slack),
                        &base,
                    )
                    .await
            });
            server.request.await.unwrap();
            if cancel {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(11)).await;
                assert_eq!(task.await.unwrap(), unknown("transport_error"));
                tokio::time::resume();
            }
            tokio::time::timeout(Duration::from_secs(1), server.task)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn invalid_inputs_never_connect() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", server.local_addr().unwrap());
        let target = destination(Channel::Slack);
        assert_eq!(
            OutboundClient::new()
                .unwrap()
                .send(&target, 0, "hello", credential(Channel::Slack), &base)
                .await,
            rejected("invalid_api_base")
        );
        let client = OutboundClient::new_with_loopback(true).unwrap();
        for (index, text, token) in [
            (0, "", "xoxb-test"),
            (16, "hello", "xoxb-test"),
            (0, "hello", "bad\r\nheader"),
        ] {
            assert!(matches!(
                client.send(&target, index, text, token, &base).await,
                DeliveryOutcome::Rejected { .. }
            ));
        }
        assert_eq!(
            client
                .send(&target, 0, &"🙂".repeat(1001), "xoxb-test", &base)
                .await,
            rejected("invalid_text")
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(30), server.accept())
                .await
                .is_err()
        );
    }
}
