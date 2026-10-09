// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bot-authorized scheduled text, separate from expiring interaction replies.
//! Contracts checked 2026-10-03, discord/discord-api-docs at
//! c43598daadbefb8afaba48ca74824a15180a8219:
//! - https://docs.discord.com/developers/resources/application#get-current-application
//! - https://docs.discord.com/developers/resources/channel#get-channel
//! - https://docs.discord.com/developers/resources/message#create-message
//! - https://docs.discord.com/developers/topics/rate-limits
//! Nonces only suppress duplicates for a few minutes; unknown POSTs are never replayed.

use anyhow::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use reqwest::{header::HeaderMap, Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    channel_types::{Channel, Destination},
    outbound::{
        http_client, rejected, response_body, unknown, validate_api_base, DeliveryOutcome,
        MAX_PART_UTF16, MAX_TEXT_BYTES,
    },
};

const ATTEMPT_BUDGET: Duration = Duration::from_secs(30);
const MAX_COOLDOWN_MS: i64 = 86_400_000;
const USER_AGENT: &str = concat!(
    "DiscordBot (",
    env!("CARGO_PKG_REPOSITORY"),
    ", ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

#[derive(Debug)]
pub(super) struct DiscordBotOutcome {
    pub outcome: DeliveryOutcome,
    pub cooldown_ms: Option<i64>,
    pub credential_rejected: bool,
}

// Never Debug/Serialize: the long-lived credential stays in memory only.
pub(super) struct DiscordBotSender {
    installation_id: String,
    guild_id: String,
    token: String,
    api_base: String,
    client: Client,
    // Serializes the full attempt (including preflight), so an observed 401
    // prevents even concurrent callers from using the rejected credential again.
    credential_rejected: Mutex<bool>,
}
#[derive(Default)]
struct Attempt {
    post_started: bool,
    cooldown_ms: Option<i64>,
    credential_rejected: bool,
}
impl Attempt {
    fn cooldown(&mut self, delay: i64) {
        self.cooldown_ms = Some(self.cooldown_ms.unwrap_or(0).max(delay));
    }
    fn finish(self, outcome: DeliveryOutcome) -> DiscordBotOutcome {
        DiscordBotOutcome {
            outcome,
            cooldown_ms: self.cooldown_ms,
            credential_rejected: self.credential_rejected,
        }
    }
}

pub(super) fn snowflake(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|id| id > 0)
}

impl DiscordBotSender {
    pub(super) fn new(
        installation_id: &str,
        guild_id: String,
        token: String,
        mut api_base: String,
        allow_loopback: bool,
    ) -> Result<Self> {
        ensure!(
            snowflake(installation_id) && snowflake(&guild_id),
            "invalid Discord application or guild ID"
        );
        ensure!(
            !token.is_empty()
                && token.len() <= 2048
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
            "invalid Discord Bot credential"
        );
        validate_api_base(Channel::Discord, &api_base, allow_loopback)?;
        api_base.truncate(api_base.trim_end_matches('/').len());
        Ok(Self {
            installation_id: installation_id.into(),
            guild_id,
            token,
            api_base,
            client: http_client()?,
            credential_rejected: Mutex::new(false),
        })
    }

    pub(super) fn credential_fingerprint(&self) -> String {
        format!("{:x}", Sha256::digest(self.token.as_bytes()))
    }

    pub(super) async fn send(
        &self,
        destination: &Destination,
        delivery_id: &str,
        text: &str,
    ) -> DiscordBotOutcome {
        self.send_with_budget(destination, delivery_id, text, ATTEMPT_BUDGET)
            .await
    }

    async fn send_with_budget(
        &self,
        destination: &Destination,
        delivery_id: &str,
        text: &str,
        budget: Duration,
    ) -> DiscordBotOutcome {
        let mut attempt = Attempt::default();
        if destination.channel != Channel::Discord
            || destination.installation_id != self.installation_id
            || !snowflake(&destination.conversation_id)
            || destination.thread_id.is_some()
            || destination.interaction_id.is_some()
            || destination.expires_ms.is_some()
        {
            return attempt.finish(rejected("invalid_destination"));
        }
        let Ok(id) = Uuid::parse_str(delivery_id) else {
            return attempt.finish(rejected("invalid_delivery_identity"));
        };
        if id.is_nil() || id.to_string() != delivery_id {
            return attempt.finish(rejected("invalid_delivery_identity"));
        }
        if text.is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.encode_utf16().count() > MAX_PART_UTF16
        {
            return attempt.finish(rejected("invalid_text"));
        }
        let nonce = URL_SAFE_NO_PAD.encode(id.as_bytes());
        let result = tokio::time::timeout(budget, async {
            let mut blocked = self.credential_rejected.lock().await;
            if *blocked {
                attempt.credential_rejected = true;
                return rejected("discord_credential_rejected");
            }
            self.send_inner(destination, text, &nonce, &mut blocked, &mut attempt)
                .await
        })
        .await;
        let outcome = result.unwrap_or_else(|_| {
            if attempt.post_started {
                unknown("discord_send_timeout")
            } else {
                rejected("discord_preflight_timeout")
            }
        });
        attempt.finish(outcome)
    }

    async fn send_inner(
        &self,
        destination: &Destination,
        text: &str,
        nonce: &str,
        blocked: &mut bool,
        attempt: &mut Attempt,
    ) -> DeliveryOutcome {
        let app = match self
            .request(Method::GET, "/applications/@me", None, blocked, attempt)
            .await
        {
            Ok(body) => body,
            Err(outcome) => return outcome,
        };
        let Ok(app) = serde_json::from_slice::<Application>(&app) else {
            return rejected("discord_invalid_application");
        };
        if app.id != self.installation_id {
            return rejected("discord_application_mismatch");
        }
        let channel_path = format!("/channels/{}", destination.conversation_id);
        let channel = match self
            .request(Method::GET, &channel_path, None, blocked, attempt)
            .await
        {
            Ok(body) => body,
            Err(outcome) => return outcome,
        };
        let Ok(channel) = serde_json::from_slice::<GuildChannel>(&channel) else {
            return rejected("discord_invalid_channel");
        };
        if channel.id != destination.conversation_id
            || channel.guild_id != self.guild_id
            || channel.kind != 0
        {
            return rejected("discord_channel_mismatch");
        }
        let payload = json!({"content":text,"tts":false,"flags":4,
            "allowed_mentions":{"parse":[],"users":[],"roles":[],"replied_user":false},
            "nonce":nonce,"enforce_nonce":true});
        let body = match self
            .request(
                Method::POST,
                &format!("{channel_path}/messages"),
                Some(&payload),
                blocked,
                attempt,
            )
            .await
        {
            Ok(body) => body,
            Err(outcome) => return outcome,
        };
        let Ok(message) = serde_json::from_slice::<Message>(&body) else {
            return unknown("discord_invalid_receipt");
        };
        if !snowflake(&message.id)
            || message.channel_id != destination.conversation_id
            || message.nonce.as_ref().is_some_and(|value| value != nonce)
        {
            return unknown("discord_receipt_mismatch");
        }
        DeliveryOutcome::Delivered {
            receipt: message.id,
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        blocked: &mut bool,
        attempt: &mut Attempt,
    ) -> std::result::Result<Vec<u8>, DeliveryOutcome> {
        let post = method == Method::POST;
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.api_base))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bot {}", self.token),
            )
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        if post {
            attempt.post_started = true;
        }
        let response = request
            .send()
            .await
            .map_err(|_| stage_error(post, "discord_transport_error"))?;
        let status = response.status();
        let headers = response.headers().clone();
        // Save known response metadata before bounded body IO or any cancellation.
        let bucket = exhausted_bucket(&headers);
        if let Ok(Some(delay)) = bucket {
            attempt.cooldown(delay);
        }
        if status == StatusCode::UNAUTHORIZED {
            *blocked = true;
            attempt.credential_rejected = true;
            return Err(rejected("discord_credential_rejected"));
        }
        // A 429 may supply its authoritative wait in the JSON body instead.
        // On other responses, a known-empty bucket needs a trustworthy reset.
        if bucket.is_err() && status != StatusCode::TOO_MANY_REQUESTS {
            return Err(stage_error(post, "discord_invalid_rate_limit"));
        }
        if !json_content_type(&headers) {
            return Err(stage_error(post, "discord_invalid_content_type"));
        }
        let bytes = response_body(response)
            .await
            .map_err(|code| stage_error(post, code))?;
        if status == StatusCode::TOO_MANY_REQUESTS {
            let delay = rate_limit(&headers, &bytes)
                .ok_or_else(|| stage_error(post, "discord_invalid_rate_limit"))?;
            attempt.cooldown(delay);
            validate_error_envelope(&bytes, post)?;
            return Err(DeliveryOutcome::RateLimited {
                retry_after_ms: attempt.cooldown_ms.unwrap_or(delay),
            });
        }
        if status != StatusCode::OK {
            validate_error_envelope(&bytes, post)?;
            if !post || explicitly_rejected(status, &bytes) {
                return Err(rejected("discord_http_rejected"));
            }
            return Err(unknown("discord_http_unknown"));
        }
        Ok(bytes)
    }
}

fn stage_error(post: bool, code: &'static str) -> DeliveryOutcome {
    if post {
        unknown(code)
    } else {
        rejected(code)
    }
}

#[derive(Deserialize)]
struct Application {
    id: String,
}
#[derive(Deserialize)]
struct GuildChannel {
    id: String,
    guild_id: String,
    #[serde(rename = "type")]
    kind: u32,
}
#[derive(Deserialize)]
struct Message {
    id: String,
    channel_id: String,
    #[serde(default, deserialize_with = "present_nonce")]
    nonce: Option<String>,
}
fn present_nonce<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
#[derive(Deserialize)]
struct Limit {
    retry_after: f64,
    global: bool,
}
#[derive(Deserialize)]
struct ApiError {
    code: u32,
    message: String,
}

// Error bodies may include diagnostic extensions, but must not simultaneously
// assert that a message exists. Detect top-level duplicate keys before trusting
// an error classification; typed success envelopes also reject repeated fields.
struct ErrorShape {
    message_evidence: bool,
}
impl<'de> Deserialize<'de> for ErrorShape {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct ShapeVisitor;
        impl<'de> serde::de::Visitor<'de> for ShapeVisitor {
            type Value = ErrorShape;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an unambiguous error object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut keys = std::collections::HashSet::new();
                let mut message_evidence = false;
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key.clone()) {
                        return Err(serde::de::Error::custom("duplicate error field"));
                    }
                    message_evidence |= matches!(
                        key.as_str(),
                        "id" | "channel_id"
                            | "nonce"
                            | "content"
                            | "author"
                            | "timestamp"
                            | "edited_timestamp"
                            | "webhook_id"
                            | "application_id"
                            | "attachments"
                            | "embeds"
                            | "mentions"
                            | "mention_roles"
                    );
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
                Ok(ErrorShape { message_evidence })
            }
        }
        deserializer.deserialize_map(ShapeVisitor)
    }
}
fn validate_error_envelope(bytes: &[u8], post: bool) -> std::result::Result<(), DeliveryOutcome> {
    let shape: ErrorShape = serde_json::from_slice(bytes)
        .map_err(|_| stage_error(post, "discord_invalid_error_response"))?;
    if post && shape.message_evidence {
        return Err(unknown("discord_conflicting_receipt"));
    }
    Ok(())
}

fn explicitly_rejected(status: StatusCode, bytes: &[u8]) -> bool {
    let Ok(error) = serde_json::from_slice::<ApiError>(bytes) else {
        return false;
    };
    if error.message.is_empty() {
        return false;
    }
    matches!(
        (status.as_u16(), error.code),
        (400, 50035) | (403, 50001 | 50013) | (404, 10003 | 10004)
    )
}
fn unique_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}
fn json_content_type(headers: &HeaderMap) -> bool {
    unique_header(headers, "content-type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}
fn seconds_ms(value: f64) -> Option<i64> {
    if !value.is_finite() || value <= 0.0 || value > MAX_COOLDOWN_MS as f64 / 1000.0 {
        return None;
    }
    Some((value * 1000.0).ceil().max(1.0) as i64)
}
fn decimal_seconds(value: &str) -> Option<i64> {
    if value.is_empty()
        || value.len() > 32
        || value.trim() != value
        || !value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return None;
    }
    seconds_ms(value.parse::<f64>().ok()?)
}
fn exhausted_bucket(headers: &HeaderMap) -> std::result::Result<Option<i64>, ()> {
    let exhausted = headers
        .get_all("x-ratelimit-remaining")
        .iter()
        .any(|value| value.to_str().ok().is_some_and(|value| value == "0"));
    if !exhausted {
        return Ok(None);
    }
    if unique_header(headers, "x-ratelimit-remaining") != Some("0") {
        return Err(());
    }
    decimal_seconds(unique_header(headers, "x-ratelimit-reset-after").ok_or(())?)
        .map(Some)
        .ok_or(())
}
fn rate_limit(headers: &HeaderMap, bytes: &[u8]) -> Option<i64> {
    let limit: Limit = serde_json::from_slice(bytes).ok()?;
    let _scope_is_global = limit.global; // Both scopes conservatively cool the installation.
    let mut delay = seconds_ms(limit.retry_after)?;
    for name in ["retry-after", "x-ratelimit-reset-after"] {
        if headers.contains_key(name) {
            delay = delay.max(decimal_seconds(unique_header(headers, name)?)?);
        }
    }
    Some(delay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const APP: &str = "123456789012345678";
    const GUILD: &str = "234567890123456789";
    const CHANNEL: &str = "345678901234567890";
    const MESSAGE: &str = "456789012345678901";
    const DELIVERY: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const SECRET: &str = "private-bot-token.sentinel";

    fn destination() -> Destination {
        Destination {
            channel: Channel::Discord,
            installation_id: APP.into(),
            conversation_id: CHANNEL.into(),
            thread_id: None,
            interaction_id: None,
            expires_ms: None,
        }
    }
    fn sender(url: &str) -> DiscordBotSender {
        DiscordBotSender::new(APP, GUILD.into(), SECRET.into(), url.into(), true).unwrap()
    }
    struct Reply {
        path: String,
        method: &'static str,
        status: u16,
        headers: String,
        body: String,
        delay: Duration,
    }
    impl Reply {
        fn json(method: &'static str, path: &str, body: Value) -> Self {
            Self {
                path: path.into(),
                method,
                status: 200,
                headers: "Content-Type: application/json\r\n".into(),
                body: body.to_string(),
                delay: Duration::ZERO,
            }
        }
        fn app() -> Self {
            Self::json("GET", "/api/v10/applications/@me", json!({"id":APP}))
        }
        fn channel() -> Self {
            Self::json(
                "GET",
                &format!("/api/v10/channels/{CHANNEL}"),
                json!({"id":CHANNEL,"guild_id":GUILD,"type":0}),
            )
        }
        fn message() -> Self {
            Self::json(
                "POST",
                &format!("/api/v10/channels/{CHANNEL}/messages"),
                json!({"id":MESSAGE,"channel_id":CHANNEL}),
            )
        }
    }
    struct Captured {
        headers: String,
        body: Vec<u8>,
    }
    struct Fixture {
        url: String,
        captured: Arc<StdMutex<Vec<Captured>>>,
        task: tokio::task::JoinHandle<()>,
    }
    async fn fixture(replies: Vec<Reply>) -> Fixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v10", listener.local_addr().unwrap());
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let copy = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(3), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (end, length) = loop {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let head = std::str::from_utf8(&bytes[..end]).unwrap();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                    assert!(bytes.len() < 16384);
                };
                while bytes.len() < end + length {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                assert!(
                    headers.starts_with(&format!("{} {} HTTP/1.1\r\n", reply.method, reply.path)),
                    "{headers}"
                );
                assert!(headers
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bot {SECRET}\r\n")));
                copy.lock().unwrap().push(Captured {
                    headers,
                    body: bytes[end..end + length].to_vec(),
                });
                if reply.status == 0 {
                    continue;
                } // Drop a POST without any receipt.
                let headers = format!(
                    "HTTP/1.1 {} Fixture\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reply.headers,
                    reply.body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                tokio::time::sleep(reply.delay).await;
                let _ = socket.write_all(reply.body.as_bytes()).await;
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(80), listener.accept())
                    .await
                    .is_err(),
                "unexpected retry or request after failed preflight"
            );
        });
        Fixture {
            url,
            captured,
            task,
        }
    }

    #[test]
    fn constructor_identity_and_rate_limit_parsing_are_bounded() {
        for bad in [
            "",
            "0",
            "01",
            "-1",
            "+1",
            "1.0",
            " 1",
            "18446744073709551616",
        ] {
            assert!(!snowflake(bad));
        }
        assert!(snowflake("18446744073709551615"));
        for url in [
            "https://attacker.example/api/v10",
            "http://discord.com/api/v10",
            "http://127.0.0.1:2/api/v10/other",
            "http://user:secret@127.0.0.1:2",
        ] {
            assert!(
                DiscordBotSender::new(APP, GUILD.into(), SECRET.into(), url.into(), true).is_err()
            );
        }
        assert!(DiscordBotSender::new(
            APP,
            GUILD.into(),
            SECRET.into(),
            "http://127.0.0.1:2".into(),
            false
        )
        .is_err());
        assert!(DiscordBotSender::new(
            APP,
            GUILD.into(),
            "token\nprivate".into(),
            "https://discord.com/api/v10".into(),
            false
        )
        .is_err());
        let a = sender("http://127.0.0.1:2/api/v10");
        assert_eq!(
            a.credential_fingerprint(),
            format!("{:x}", Sha256::digest(SECRET))
        );
        assert_eq!(a.credential_fingerprint().len(), 64);
        let headers = HeaderMap::new();
        for body in [
            r#"{"retry_after":0,"global":false}"#,
            r#"{"retry_after":86401,"global":false}"#,
            r#"{"retry_after":1}"#,
            r#"{"retry_after":"1","global":false}"#,
            r#"{"retry_after":1,"global":"false"}"#,
        ] {
            assert!(rate_limit(&headers, body.as_bytes()).is_none());
        }
        assert_eq!(
            rate_limit(&headers, br#"{"retry_after":0.0001,"global":true}"#),
            Some(1)
        );
        assert_eq!(
            rate_limit(&headers, br#"{"retry_after":86400,"global":false}"#),
            Some(MAX_COOLDOWN_MS)
        );
        let mut headers = HeaderMap::new();
        headers.append("retry-after", "2".parse().unwrap());
        headers.append("retry-after", "2".parse().unwrap());
        assert!(rate_limit(&headers, br#"{"retry_after":1,"global":false}"#).is_none());
    }

    #[tokio::test]
    async fn exact_bot_identity_preflight_payload_receipt_and_cooldown() {
        let mut app = Reply::app();
        app.headers
            .push_str("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 1.25\r\n");
        let mut message = Reply::message();
        message
            .headers
            .push_str("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 2.001\r\n");
        message.body=json!({"id":MESSAGE,"channel_id":CHANNEL,"nonce":URL_SAFE_NO_PAD.encode(Uuid::parse_str(DELIVERY).unwrap().as_bytes())}).to_string();
        let f = fixture(vec![app, Reply::channel(), message]).await;
        let text = "hello <@123> @everyone https://example.invalid 😀";
        let outcome = sender(&f.url).send(&destination(), DELIVERY, text).await;
        assert_eq!(
            outcome.outcome,
            DeliveryOutcome::Delivered {
                receipt: MESSAGE.into()
            }
        );
        assert_eq!(outcome.cooldown_ms, Some(2001));
        assert!(!outcome.credential_rejected);
        f.task.await.unwrap();
        let requests = f.captured.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].body.is_empty() && requests[1].body.is_empty());
        assert!(requests[2]
            .headers
            .to_ascii_lowercase()
            .contains("user-agent: discordbot (https://github.com/stateknot/jiaclaw,"));
        let body: Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(body["content"], text);
        assert_eq!(body["enforce_nonce"], true);
        assert_eq!(body["nonce"].as_str().unwrap().len(), 22);
        assert_eq!(
            body["allowed_mentions"],
            json!({"parse":[],"roles":[],"users":[],"replied_user":false})
        );
        assert_eq!(body["tts"], false);
        assert_eq!(body["flags"], 4);
    }

    #[tokio::test]
    async fn authorization_and_malformed_preflight_never_post() {
        for invalid in [json!({"id":MESSAGE}), json!({"id":APP,"id":1}), Value::Null] {
            let mut app = Reply::app();
            app.body = invalid.to_string();
            let f = fixture(vec![app]).await;
            assert!(matches!(
                sender(&f.url)
                    .send(&destination(), DELIVERY, "text")
                    .await
                    .outcome,
                DeliveryOutcome::Rejected { .. }
            ));
            f.task.await.unwrap();
            assert_eq!(f.captured.lock().unwrap().len(), 1);
        }
        for body in [
            json!({"id":MESSAGE,"guild_id":GUILD,"type":0}),
            json!({"id":CHANNEL,"guild_id":MESSAGE,"type":0}),
            json!({"id":CHANNEL,"type":0}),
            json!({"id":CHANNEL,"guild_id":GUILD,"type":1}),
            json!({"id":CHANNEL,"guild_id":GUILD,"type":5}),
            json!({"id":CHANNEL,"guild_id":GUILD,"type":11}),
        ] {
            let mut channel = Reply::channel();
            channel.body = body.to_string();
            let f = fixture(vec![Reply::app(), channel]).await;
            assert!(matches!(
                sender(&f.url)
                    .send(&destination(), DELIVERY, "text")
                    .await
                    .outcome,
                DeliveryOutcome::Rejected { .. }
            ));
            f.task.await.unwrap();
            assert_eq!(f.captured.lock().unwrap().len(), 2);
        }
        let a = sender("http://127.0.0.1:2/api/v10");
        for text in ["", &"😀".repeat(1001), &"a".repeat(MAX_TEXT_BYTES + 1)] {
            assert_eq!(
                a.send(&destination(), DELIVERY, text).await.outcome,
                rejected("invalid_text")
            );
        }
        let mut d = destination();
        d.interaction_id = Some(MESSAGE.into());
        assert_eq!(
            a.send(&d, DELIVERY, "text").await.outcome,
            rejected("invalid_destination")
        );
        d = destination();
        d.thread_id = Some(MESSAGE.into());
        assert_eq!(
            a.send(&d, DELIVERY, "text").await.outcome,
            rejected("invalid_destination")
        );
        d = destination();
        d.expires_ms = Some(1);
        assert_eq!(
            a.send(&d, DELIVERY, "text").await.outcome,
            rejected("invalid_destination")
        );
        assert_eq!(
            a.send(&destination(), "not-uuid", "text").await.outcome,
            rejected("invalid_delivery_identity")
        );
    }

    #[tokio::test]
    async fn rejected_credential_stops_all_later_network_even_with_unread_body() {
        for post in [false, true] {
            let mut denied = if post { Reply::message() } else { Reply::app() };
            denied.status = 401;
            denied.headers="Content-Type: text/html\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 5\r\n".into();
            denied.body = "credential must never be echoed".into();
            denied.delay = Duration::from_millis(200);
            let mut replies = if post {
                vec![Reply::app(), Reply::channel()]
            } else {
                vec![]
            };
            replies.push(denied);
            let f = fixture(replies).await;
            let a = sender(&f.url);
            let first = a.send(&destination(), DELIVERY, "text").await;
            assert_eq!(first.outcome, rejected("discord_credential_rejected"));
            assert!(first.credential_rejected);
            assert_eq!(first.cooldown_ms, Some(5000));
            let second = a.send(&destination(), DELIVERY, "text").await;
            assert!(second.credential_rejected);
            assert_eq!(second.outcome, rejected("discord_credential_rejected"));
            f.task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn only_trusted_rate_limits_allow_another_attempt() {
        for post in [false, true] {
            let mut limit = if post { Reply::message() } else { Reply::app() };
            limit.status = 429;
            limit.body = json!({"retry_after":1.25,"global":true}).to_string();
            limit.headers.push_str("Retry-After: 2\r\n");
            let mut replies = if post {
                vec![Reply::app(), Reply::channel()]
            } else {
                vec![]
            };
            replies.push(limit);
            let f = fixture(replies).await;
            let result = sender(&f.url).send(&destination(), DELIVERY, "text").await;
            assert_eq!(
                result.outcome,
                DeliveryOutcome::RateLimited {
                    retry_after_ms: 2000
                }
            );
            assert_eq!(result.cooldown_ms, Some(2000));
            f.task.await.unwrap();
        }
        let mut invalid = Reply::message();
        invalid.status = 429;
        invalid.body = json!({"retry_after":1}).to_string();
        let f = fixture(vec![Reply::app(), Reply::channel(), invalid]).await;
        assert_eq!(
            sender(&f.url)
                .send(&destination(), DELIVERY, "text")
                .await
                .outcome,
            unknown("discord_invalid_rate_limit")
        );
        f.task.await.unwrap();
    }

    #[tokio::test]
    async fn post_receipt_failures_and_interruption_are_never_replayed_or_leaked() {
        for body in [
            json!({"id":"01","channel_id":CHANNEL}),
            json!({"id":MESSAGE,"channel_id":GUILD}),
            json!({"id":MESSAGE,"channel_id":CHANNEL,"nonce":"wrong"}),
            json!({"id":MESSAGE,"channel_id":CHANNEL,"nonce":null}),
            json!({"id":MESSAGE,"channel_id":CHANNEL,"nonce":123}),
            json!({"message":"private provider body"}),
        ] {
            let mut reply = Reply::message();
            reply.body = body.to_string();
            let f = fixture(vec![Reply::app(), Reply::channel(), reply]).await;
            let result = sender(&f.url).send(&destination(), DELIVERY, "text").await;
            assert!(matches!(result.outcome, DeliveryOutcome::Unknown { .. }));
            assert!(!format!("{result:?}").contains("private provider body"));
            f.task.await.unwrap();
        }
        for (status, body, headers) in [
            (0, String::new(), "Content-Type: application/json\r\n"),
            (502, "{}".into(), "Content-Type: application/json\r\n"),
            (200, "{}".into(), "Content-Type: text/html\r\n"),
            (200, "x".repeat(65537), "Content-Type: application/json\r\n"),
            (
                200,
                "{}".into(),
                "Content-Type: application/json\r\nContent-Type: text/plain\r\n",
            ),
        ] {
            let mut reply = Reply::message();
            reply.status = status;
            reply.body = body;
            reply.headers = headers.into();
            let f = fixture(vec![Reply::app(), Reply::channel(), reply]).await;
            assert!(matches!(
                sender(&f.url)
                    .send(&destination(), DELIVERY, "text")
                    .await
                    .outcome,
                DeliveryOutcome::Unknown { .. }
            ));
            f.task.await.unwrap();
        }
        for (code, status, reject) in [
            (50013, 403, true),
            (50035, 400, true),
            (10003, 404, true),
            (50000, 400, false),
        ] {
            let mut reply = Reply::message();
            reply.status = status;
            reply.body = json!({"code":code,"message":"secret diagnostic"}).to_string();
            let f = fixture(vec![Reply::app(), Reply::channel(), reply]).await;
            let outcome = sender(&f.url)
                .send(&destination(), DELIVERY, "text")
                .await
                .outcome;
            assert_eq!(matches!(outcome, DeliveryOutcome::Rejected { .. }), reject);
            assert!(!format!("{outcome:?}").contains("secret"));
            f.task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn error_receipts_cannot_authorize_retry_or_definitive_rejection() {
        for status in [429, 403] {
            for (field, value) in [
                ("id", json!(MESSAGE)),
                ("channel_id", json!(CHANNEL)),
                ("nonce", json!("received-nonce")),
                ("author", json!({"id":APP})),
                ("content", json!("accepted text")),
            ] {
                let mut reply = Reply::message();
                reply.status = status;
                let mut body = if status == 429 {
                    json!({"retry_after":300,"global":false})
                } else {
                    json!({"code":50013,"message":"no permission"})
                };
                body[field] = value;
                reply.body = body.to_string();
                reply
                    .headers
                    .push_str("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 2\r\n");
                let f = fixture(vec![Reply::app(), Reply::channel(), reply]).await;
                let result = sender(&f.url).send(&destination(), DELIVERY, "text").await;
                assert_eq!(result.outcome, unknown("discord_conflicting_receipt"));
                assert_eq!(
                    result.cooldown_ms,
                    Some(if status == 429 { 300000 } else { 2000 })
                );
                assert!(!result.credential_rejected);
                f.task.await.unwrap();
                assert_eq!(f.captured.lock().unwrap().len(), 3);
            }
        }
    }

    #[tokio::test]
    async fn duplicate_protocol_keys_never_authorize_delivery_or_retry() {
        for (status, body) in [
            (
                429,
                r#"{"retry_after":1,"retry_after":2,"global":false}"#.to_owned(),
            ),
            (
                429,
                r#"{"retry_after":1,"global":false,"trace":"a","trace":"b"}"#.to_owned(),
            ),
            (
                403,
                r#"{"code":50013,"code":50013,"message":"denied"}"#.to_owned(),
            ),
            (
                200,
                format!(r#"{{"id":"{MESSAGE}","id":"{MESSAGE}","channel_id":"{CHANNEL}"}}"#),
            ),
            (
                200,
                format!(r#"{{"id":"{MESSAGE}","channel_id":"{CHANNEL}","nonce":"a","nonce":"b"}}"#),
            ),
        ] {
            let mut reply = Reply::message();
            reply.status = status;
            reply.body = body;
            let f = fixture(vec![Reply::app(), Reply::channel(), reply]).await;
            let result = sender(&f.url).send(&destination(), DELIVERY, "text").await;
            assert!(matches!(result.outcome, DeliveryOutcome::Unknown { .. }));
            f.task.await.unwrap();
        }
        assert!(
            serde_json::from_str::<Application>(&format!(r#"{{"id":"{APP}","id":"{APP}"}}"#))
                .is_err()
        );
        assert!(serde_json::from_str::<GuildChannel>(&format!(
            r#"{{"id":"{CHANNEL}","guild_id":"{GUILD}","guild_id":"{GUILD}","type":0}}"#
        ))
        .is_err());
    }

    #[tokio::test]
    async fn exhausted_bucket_without_trustworthy_reset_never_continues() {
        for post in [false, true] {
            for reset in [
                "",
                "X-RateLimit-Reset-After: bad\r\n",
                "X-RateLimit-Reset-After: 0\r\n",
                "X-RateLimit-Reset-After: 86401\r\n",
                "X-RateLimit-Reset-After: 2\r\nX-RateLimit-Reset-After: 2\r\n",
            ] {
                let mut reply = if post {
                    Reply::message()
                } else {
                    Reply::channel()
                };
                reply
                    .headers
                    .push_str(&format!("X-RateLimit-Remaining: 0\r\n{reset}"));
                let mut app = Reply::app();
                app.headers
                    .push_str("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 1\r\n");
                let mut replies = vec![app];
                if post {
                    replies.push(Reply::channel());
                }
                replies.push(reply);
                let f = fixture(replies).await;
                let result = sender(&f.url).send(&destination(), DELIVERY, "text").await;
                assert_eq!(
                    result.outcome,
                    stage_error(post, "discord_invalid_rate_limit")
                );
                assert_eq!(result.cooldown_ms, Some(1000));
                f.task.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn total_deadline_preserves_known_cooldown_and_submission_phase() {
        for post in [false, true] {
            let mut delayed = if post { Reply::message() } else { Reply::app() };
            delayed
                .headers
                .push_str("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 7\r\n");
            delayed.delay = Duration::from_millis(600);
            let mut replies = if post {
                vec![Reply::app(), Reply::channel()]
            } else {
                vec![]
            };
            replies.push(delayed);
            let f = fixture(replies).await;
            let outcome = sender(&f.url)
                .send_with_budget(&destination(), DELIVERY, "text", Duration::from_millis(400))
                .await;
            assert_eq!(outcome.cooldown_ms, Some(7000));
            assert_eq!(
                matches!(outcome.outcome, DeliveryOutcome::Unknown { .. }),
                post
            );
            assert_eq!(
                matches!(outcome.outcome, DeliveryOutcome::Rejected { .. }),
                !post
            );
            f.task.await.unwrap();
        }
    }
}
