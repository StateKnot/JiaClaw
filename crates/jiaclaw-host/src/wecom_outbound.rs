// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One attempt to send a text message from an enterprise self-built `WeCom` app.
//!
//! Official contracts (retrieved 2026-10-03):
//! - <https://developer.work.weixin.qq.com/document/path/91039>
//! - <https://developer.work.weixin.qq.com/document/path/90236>
//! - <https://developer.work.weixin.qq.com/document/path/90312>
//! - <https://developer.work.weixin.qq.com/document/path/90313>
//!
//! Secrets belong in query parameters under this API's contract. Only the fixed
//! official HTTPS origin (or explicit loopback fixture) receives them; request
//! URLs, reqwest errors and upstream bodies never enter diagnostics or storage.
//! A valid receipt acknowledges platform acceptance, not the member reading it.

use anyhow::{ensure, Result};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::{sync::Mutex, time::Instant};

use crate::{
    channel_types::{Channel, Destination},
    outbound::{
        http_client, rejected, response_body, unknown, valid_wecom_text, validate_api_base,
        validate_destination, wecom_text, DeliveryOutcome,
    },
};

const TOKEN_BUDGET: Duration = Duration::from_secs(12);
const TOKEN_BACKOFF: Duration = Duration::from_secs(30);
const TOKEN_EXPIRY_MARGIN: u64 = 60;

// No Debug/Serialize: this state contains memory-only credentials.
pub(super) struct WeComSender {
    installation_id: String,
    corp_id: String,
    agent_id: u32,
    app_secret: String,
    api_base: String,
    client: Client,
    token: Mutex<TokenState>,
}

struct CachedToken {
    value: String,
    usable_until: Instant,
}

struct TokenState {
    cached: Option<CachedToken>,
    retry_at: Instant,
}

impl WeComSender {
    pub(super) fn new(
        installation_id: &str,
        app_secret: String,
        mut api_base: String,
        allow_loopback: bool,
    ) -> Result<Self> {
        let (corp_id, agent_id) = super::wecom::validate_installation(installation_id)?;
        ensure!(
            !app_secret.is_empty()
                && app_secret.len() <= 4096
                && app_secret.bytes().all(|byte| byte.is_ascii_graphic()),
            "invalid WeCom app secret"
        );
        validate_api_base(Channel::Wecom, &api_base, allow_loopback)?;
        api_base.truncate(api_base.trim_end_matches('/').len());
        Ok(Self {
            installation_id: installation_id.into(),
            corp_id: corp_id.into(),
            agent_id,
            app_secret,
            api_base,
            client: http_client()?,
            token: Mutex::new(TokenState {
                cached: None,
                retry_at: Instant::now(),
            }),
        })
    }

    async fn access_token(&self) -> Option<String> {
        tokio::time::timeout(TOKEN_BUDGET, async {
            // Holding this asynchronous lock across bounded HTTP is the single
            // refresh flight. Pre-arm cooldown so cancellation also backs off.
            let mut state = self.token.lock().await;
            let now = Instant::now();
            if let Some(token) = &state.cached {
                if now < token.usable_until {
                    return Some(token.value.clone());
                }
            }
            if now < state.retry_at {
                return None;
            }
            state.cached = None;
            state.retry_at = now + TOKEN_BACKOFF;
            if let Some(token) = self.fetch_token(now).await {
                let value = token.value.clone();
                state.cached = Some(token);
                state.retry_at = Instant::now();
                return Some(value);
            }
            state.retry_at = Instant::now() + TOKEN_BACKOFF;
            None
        })
        .await
        .ok()
        .flatten()
    }

    async fn fetch_token(&self, started: Instant) -> Option<CachedToken> {
        let response = self
            .client
            .get(format!("{}/gettoken", self.api_base))
            .query(&[
                ("corpid", self.corp_id.as_str()),
                ("corpsecret", self.app_secret.as_str()),
            ])
            .send()
            .await
            .ok()?;
        if response.status() != StatusCode::OK {
            return None;
        }
        let bytes = response_body(response).await.ok()?;
        let token: TokenEnvelope = serde_json::from_slice(&bytes).ok()?;
        if token.errcode != 0
            || token.expires_in <= TOKEN_EXPIRY_MARGIN
            || token.access_token.is_empty()
            || token.access_token.len() > 512
            || !token
                .access_token
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
        {
            return None;
        }
        // The documented normal lifetime is 7200s; expires_in remains
        // authoritative if shorter. Never retain a token beyond two hours.
        let usable_until =
            started + Duration::from_secs(token.expires_in.min(7200) - TOKEN_EXPIRY_MARGIN);
        (Instant::now() < usable_until).then_some(CachedToken {
            value: token.access_token,
            usable_until,
        })
    }

    pub(super) async fn send(&self, destination: &Destination, text: &str) -> DeliveryOutcome {
        if destination.channel != Channel::Wecom
            || destination.installation_id != self.installation_id
            || validate_destination(destination).is_err()
        {
            return rejected("invalid_destination");
        }
        if !valid_wecom_text(text) {
            return rejected("invalid_text");
        }
        let Some(token) = self.access_token().await else {
            // No message POST has been submitted, even if auth HTTP failed.
            return rejected("credential_unavailable");
        };
        let result = self
            .client
            .post(format!("{}/message/send", self.api_base))
            .query(&[("access_token", token.as_str())])
            .json(&json!({
                "touser": destination.conversation_id,
                "agentid": self.agent_id,
                "msgtype": "text",
                "text": {"content": wecom_text(text)},
                "safe": 0,
                "enable_id_trans": 0,
                // Platform deduplication compares request content, not our
                // delivery ID. It would erase distinct identical messages.
                "enable_duplicate_check": 0
            }))
            .send()
            .await;
        let Ok(response) = result else {
            return unknown("transport_error");
        };
        let status = response.status();
        if status.is_server_error() {
            return unknown("http_server_error");
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            // No documented retry-delay header contract: do not invent one.
            return unknown("wecom_rate_limit");
        }
        if !status.is_success() && !status.is_client_error() {
            return unknown("unexpected_http_status");
        }
        let body = match response_body(response).await {
            Ok(body) => body,
            Err(code) => return unknown(code),
        };
        let Ok(envelope) = serde_json::from_slice::<MessageEnvelope>(&body) else {
            return unknown("invalid_response");
        };
        if matches!(envelope.errcode, 40_014 | 42_001) {
            self.invalidate_token(&token);
        }
        parse_receipt(status, envelope)
    }

    fn invalidate_token(&self, used: &str) {
        // A concurrent refresh owns an empty cache. Never clear its result or
        // await it here, and never repeat the message that got an auth error.
        if let Ok(mut state) = self.token.try_lock() {
            if state
                .cached
                .as_ref()
                .is_some_and(|token| token.value == used)
            {
                state.cached = None;
                state.retry_at = Instant::now() + TOKEN_BACKOFF;
            }
        }
    }
}

#[derive(Deserialize)]
struct TokenEnvelope {
    errcode: i64,
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct MessageEnvelope {
    errcode: i64,
    msgid: Option<String>,
    invaliduser: Option<String>,
    invalidparty: Option<String>,
    invalidtag: Option<String>,
    unlicenseduser: Option<String>,
}

fn parse_receipt(status: StatusCode, envelope: MessageEnvelope) -> DeliveryOutcome {
    if matches!(envelope.errcode, 45_009 | 45_033) {
        return unknown("wecom_rate_limit");
    }
    if explicitly_rejected(envelope.errcode) {
        // A message ID contradicts a definitive refusal. Keep that attempt
        // reviewable instead of discarding possible acceptance evidence. An
        // invalid-recipient list alone is consistent with all-target refusal
        // (81013), so it does not override an otherwise explicit rejection.
        if envelope.msgid.as_ref().is_some_and(|id| !id.is_empty()) {
            return unknown("wecom_conflicting_receipt");
        }
        return rejected("wecom_rejected");
    }
    if envelope.errcode != 0 || !status.is_success() {
        return unknown("wecom_error");
    }
    if [
        envelope.invaliduser,
        envelope.invalidparty,
        envelope.invalidtag,
        envelope.unlicenseduser,
    ]
    .into_iter()
    .flatten()
    .any(|invalid| !invalid.is_empty())
    {
        return unknown("wecom_partial_delivery");
    }
    let Some(receipt) = envelope.msgid.filter(|id| {
        !id.trim().is_empty() && id.len() <= 4096 && !id.chars().any(char::is_control)
    }) else {
        return unknown("invalid_receipt");
    };
    DeliveryOutcome::Delivered { receipt }
}

// Official global-code definitions establish validation/auth/permission refusal.
// Unknown errors, system busy (-1), conflicts, rate limits and HTTP ambiguity
// remain unknown, regardless of a general platform suggestion to retry.
fn explicitly_rejected(code: i64) -> bool {
    matches!(
        code,
        40_001
            | 40_003
            | 40_008
            | 40_013
            | 40_014
            | 40_031
            | 40_056
            | 40_058
            | 40_063
            | 41_001
            | 41_002
            | 41_004
            | 41_009
            | 42_001
            | 44_004
            | 45_002
            | 48_001
            | 48_002
            | 48_004
            | 48_005
            | 60_011
            | 60_020
            | 60_021
            | 60_031
            | 81_013
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::Arc,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Semaphore,
        task::{JoinHandle, JoinSet},
    };

    const INSTALLATION: &str = "wwtestcorp:1000002";
    const SECRET: &str = "test_secret?&=+#/:";

    #[derive(Clone, Debug)]
    struct Request {
        method: String,
        path: String,
        query: BTreeMap<String, String>,
        authorization: Option<String>,
        body: Option<Value>,
    }

    struct Reply {
        wire: Option<Vec<u8>>,
        gate: Option<Arc<Semaphore>>,
    }
    impl Reply {
        fn raw(status: u16, headers: &str, body: &str) -> Self {
            Self { wire:Some(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",body.len()).into_bytes()), gate:None }
        }
        fn json(value: &Value) -> Self {
            Self::raw(200, "", &value.to_string())
        }
        fn gated(mut self, gate: Arc<Semaphore>) -> Self {
            self.gate = Some(gate);
            self
        }
    }

    struct Server {
        base: String,
        requests: Arc<std::sync::Mutex<Vec<Request>>>,
        task: JoinHandle<()>,
    }
    impl Server {
        async fn new(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}/cgi-bin", listener.local_addr().unwrap());
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let records = requests.clone();
            let mut replies = VecDeque::from(replies);
            let task = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    loop {
                        let read = socket.read(&mut buffer).await.unwrap();
                        if read == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&buffer[..read]);
                        assert!(bytes.len() < 32 * 1024);
                        let Some(offset) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let headers = std::str::from_utf8(&bytes[..offset]).unwrap();
                        let header = |key: &str| {
                            headers.lines().find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case(key)
                                    .then(|| value.trim().to_owned())
                            })
                        };
                        let length = header("content-length")
                            .map_or(0, |value| value.parse::<usize>().unwrap());
                        if bytes.len() < offset + 4 + length {
                            continue;
                        }
                        let mut line = headers.lines().next().unwrap().split_whitespace();
                        let method = line.next().unwrap().into();
                        let target = reqwest::Url::parse(&format!(
                            "http://127.0.0.1{}",
                            line.next().unwrap()
                        ))
                        .unwrap();
                        records.lock().unwrap().push(Request {
                            method,
                            path: target.path().into(),
                            query: target
                                .query_pairs()
                                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                                .collect(),
                            authorization: header("authorization"),
                            body: (length > 0).then(|| {
                                serde_json::from_slice(&bytes[offset + 4..offset + 4 + length])
                                    .unwrap()
                            }),
                        });
                        let reply = replies.pop_front().expect("unexpected hidden HTTP attempt");
                        if let Some(gate) = reply.gate {
                            gate.acquire().await.unwrap().forget();
                        }
                        if let Some(wire) = reply.wire {
                            let _ = socket.write_all(&wire).await;
                        }
                        let _ = socket.shutdown().await;
                        break;
                    }
                }
            });
            Self {
                base,
                requests,
                task,
            }
        }
        fn sender(&self) -> WeComSender {
            WeComSender::new(INSTALLATION, SECRET.into(), self.base.clone(), true).unwrap()
        }
        fn captured(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
        async fn wait_requests(&self, count: usize) {
            tokio::time::timeout(Duration::from_secs(3), async {
                while self.requests.lock().unwrap().len() < count {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    fn destination() -> Destination {
        Destination {
            channel: Channel::Wecom,
            installation_id: INSTALLATION.into(),
            conversation_id: "member.one@example.com".into(),
            thread_id: None,
            interaction_id: None,
            expires_ms: None,
        }
    }
    fn token(value: &str) -> Reply {
        Reply::json(&json!({"errcode":0,"access_token":value,"expires_in":7200}))
    }
    fn receipt(id: &str) -> Reply {
        Reply::json(&json!({"errcode":0,"errmsg":"ok","msgid":id}))
    }

    #[tokio::test]
    async fn sends_single_member_with_query_credentials_and_preserves_identical_messages() {
        let server = Server::new(vec![
            token("token?&=+#/"),
            receipt("first-id"),
            receipt("second-id"),
        ])
        .await;
        let sender = server.sender();
        for id in ["first-id", "second-id"] {
            assert_eq!(
                sender
                    .send(
                        &destination(),
                        "相同回复 <a href=\"https://example.com\">链接</a>\n🙂"
                    )
                    .await,
                DeliveryOutcome::Delivered { receipt: id.into() }
            );
        }
        let requests = server.captured();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/cgi-bin/gettoken");
        assert_eq!(
            requests[0].query,
            BTreeMap::from([
                ("corpid".into(), "wwtestcorp".into()),
                ("corpsecret".into(), SECRET.into())
            ])
        );
        assert!(requests[0].body.is_none());
        for request in &requests[1..] {
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/cgi-bin/message/send");
            assert_eq!(
                request.query,
                BTreeMap::from([("access_token".into(), "token?&=+#/".into())])
            );
            assert!(request.authorization.is_none());
            assert_eq!(
                request.body.as_ref().unwrap(),
                &json!({"touser":"member.one@example.com","agentid":1_000_002,"msgtype":"text","text":{"content":"相同回复 ＜a href=\"https://example.com\"＞链接＜/a＞\n🙂"},"safe":0,"enable_id_trans":0,"enable_duplicate_check":0})
            );
        }
        assert_eq!(requests[1].body,requests[2].body,"identical independent deliveries are both submitted without platform content deduplication");
    }

    #[tokio::test]
    async fn rejects_unsafe_inputs_before_even_requesting_a_token() {
        let server = Server::new(vec![]).await;
        for base in [
            "https://attacker.invalid/cgi-bin",
            "http://qyapi.weixin.qq.com/cgi-bin",
            "https://qyapi.weixin.qq.com/cgi-bin?secret=x",
            "http://localhost:80/cgi-bin",
            "http://127.1:80",
            "http://user@127.0.0.1:80/cgi-bin",
            "http://127.0.0.1:80/elsewhere",
            "http://127.0.0.1:80/#x",
        ] {
            assert!(WeComSender::new(INSTALLATION, SECRET.into(), base.into(), true).is_err());
        }
        assert!(WeComSender::new(INSTALLATION, SECRET.into(), server.base.clone(), false).is_err());
        for installation in [
            "corp:0",
            "corp:01",
            "corp:1:2",
            "corp:2147483648",
            "corp:/send",
        ] {
            assert!(
                WeComSender::new(installation, SECRET.into(), server.base.clone(), true).is_err()
            );
        }
        let sender = server.sender();
        for user in ["@all", "member|other", "Member", "member\n", "", " leading"] {
            let mut target = destination();
            target.conversation_id = user.into();
            assert_eq!(
                sender.send(&target, "hello").await,
                rejected("invalid_destination")
            );
        }
        for field in ["installation", "thread", "interaction", "expiry"] {
            let mut target = destination();
            match field {
                "installation" => target.installation_id = "othercorp:1000002".into(),
                "thread" => target.thread_id = Some("root".into()),
                "interaction" => target.interaction_id = Some("interaction".into()),
                _ => target.expires_ms = Some(1),
            }
            assert_eq!(
                sender.send(&target, "hello").await,
                rejected("invalid_destination")
            );
        }
        for text in [
            String::new(),
            "a".repeat(2049),
            "中".repeat(683),
            "<".repeat(683),
        ] {
            assert_eq!(
                sender.send(&destination(), &text).await,
                rejected("invalid_text")
            );
        }
        assert!(server.captured().is_empty());
    }

    #[tokio::test]
    async fn one_token_refresh_serves_concurrent_calls() {
        let gate = Arc::new(Semaphore::new(0));
        let mut replies = vec![token("shared-token").gated(gate.clone())];
        replies.extend((0..8).map(|_| receipt("id")));
        let server = Server::new(replies).await;
        let sender = Arc::new(server.sender());
        let mut tasks = JoinSet::new();
        for _ in 0..8 {
            let sender = sender.clone();
            tasks.spawn(async move { sender.send(&destination(), "hello").await });
        }
        server.wait_requests(1).await;
        gate.add_permits(1);
        while let Some(result) = tasks.join_next().await {
            assert!(matches!(result.unwrap(), DeliveryOutcome::Delivered { .. }));
        }
        let requests = server.captured();
        assert_eq!(requests.len(), 9);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.path.ends_with("gettoken"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn credential_failures_are_not_sent_and_back_off_without_secret_diagnostics() {
        for failure in [
            Reply::raw(401, "", "echo-app-secret"),
            Reply::raw(503, "", "echo-app-secret"),
            Reply::raw(302, "Location: /elsewhere\r\n", ""),
            Reply::raw(200, "", "{}"),
            Reply::json(&json!({"errcode":40_001,"access_token":"invalid","expires_in":7200})),
            Reply::json(&json!({"errcode":0,"access_token":"invalid\ntoken","expires_in":7200})),
            Reply::json(&json!({"errcode":0,"access_token":"x".repeat(513),"expires_in":7200})),
            Reply::json(&json!({"errcode":0,"access_token":"token","expires_in":60})),
            Reply::raw(200, "", &"x".repeat(65537)),
            Reply {
                wire: None,
                gate: None,
            },
        ] {
            let server = Server::new(vec![failure]).await;
            let sender = server.sender();
            for _ in 0..2 {
                assert_eq!(
                    sender.send(&destination(), "hello").await,
                    rejected("credential_unavailable")
                );
            }
            assert_eq!(server.captured().len(), 1);
        }
    }

    #[tokio::test]
    async fn success_requires_msgid_and_no_invalid_or_unlicensed_recipient() {
        let mut invalid = Vec::new();
        for field in [
            "invaliduser",
            "invalidparty",
            "invalidtag",
            "unlicenseduser",
        ] {
            let mut body = json!({"errcode":0,"msgid":"id"});
            body[field] = json!("member.one@example.com");
            invalid.push(body);
            let mut body = json!({"errcode":0,"msgid":"id"});
            body[field] = json!([]);
            invalid.push(body);
        }
        invalid.extend([
            json!({"errcode":0}),
            json!({"errcode":0,"msgid":""}),
            json!({"errcode":0,"msgid":1}),
            json!({"errcode":0,"msgid":"id\n"}),
        ]);
        for body in invalid {
            let server = Server::new(vec![token("token"), Reply::json(&body)]).await;
            assert!(matches!(
                server.sender().send(&destination(), "hello").await,
                DeliveryOutcome::Unknown { .. }
            ));
            assert_eq!(server.captured().len(), 2);
        }
        let server=Server::new(vec![token("token"),Reply::json(&json!({"errcode":0,"msgid":"id","invaliduser":"","invalidparty":"","invalidtag":"","unlicenseduser":""}))]).await;
        assert_eq!(
            server.sender().send(&destination(), "hello").await,
            DeliveryOutcome::Delivered {
                receipt: "id".into()
            }
        );
    }

    #[tokio::test]
    async fn unknown_errors_limits_redirects_and_disconnects_never_retry() {
        for failure in [
            Reply::raw(400, "", r#"{"errcode":0,"msgid":"id"}"#),
            Reply::raw(400, "", r#"{"errcode":999999}"#),
            Reply::raw(401, "", "unknown"),
            Reply::raw(403, "", "unknown"),
            Reply::raw(429, "Retry-After: 1\r\n", r#"{"errcode":45009}"#),
            Reply::json(&json!({"errcode":45_009})),
            Reply::json(&json!({"errcode":45_033})),
            Reply::json(&json!({"errcode":-1})),
            Reply::json(&json!({"errcode":45_035})),
            Reply::raw(503, "", ""),
            Reply::raw(307, "Location: /elsewhere\r\n", ""),
            Reply::raw(200, "", "invalid JSON"),
            Reply::raw(200, "", &"x".repeat(65537)),
            Reply {
                wire: None,
                gate: None,
            },
        ] {
            let server = Server::new(vec![token("token"), failure]).await;
            assert!(matches!(
                server.sender().send(&destination(), "hello").await,
                DeliveryOutcome::Unknown { .. }
            ));
            assert_eq!(server.captured().len(), 2);
        }
        for status in [200, 400] {
            for code in [40_003, 40_014, 40_056, 42_001, 48_001, 60_020, 81_013] {
                let server = Server::new(vec![
                    token("token"),
                    Reply::raw(status, "", &json!({"errcode":code}).to_string()),
                ])
                .await;
                assert_eq!(
                    server.sender().send(&destination(), "hello").await,
                    rejected("wecom_rejected")
                );
                assert_eq!(server.captured().len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn refusal_with_message_id_remains_unknown_but_all_invalid_is_rejected() {
        for status in [200, 400] {
            for body in [
                json!({"errcode":40_014,"msgid":"accepted-id"}),
                json!({"errcode":40_003,"msgid":"accepted-id","invaliduser":"member.one@example.com"}),
                json!({"errcode":81_013,"msgid":"accepted-id","invaliduser":"member.one@example.com"}),
                json!({"errcode":40_014,"msgid":" "}),
            ] {
                let server = Server::new(vec![
                    token("token"),
                    Reply::raw(status, "", &body.to_string()),
                ])
                .await;
                assert_eq!(
                    server.sender().send(&destination(), "hello").await,
                    unknown("wecom_conflicting_receipt")
                );
                assert_eq!(server.captured().len(), 2, "no hidden message retry");
            }
            for msgid in [Value::Null, json!("")] {
                let body =
                    json!({"errcode":81_013,"invaliduser":"member.one@example.com","msgid":msgid});
                let server = Server::new(vec![
                    token("token"),
                    Reply::raw(status, "", &body.to_string()),
                ])
                .await;
                assert_eq!(
                    server.sender().send(&destination(), "hello").await,
                    rejected("wecom_rejected")
                );
                assert_eq!(server.captured().len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn token_expiry_and_invalidation_only_refresh_future_independent_calls() {
        for code in [40_014, 42_001] {
            let server = Server::new(vec![
                token("old"),
                Reply::json(&json!({"errcode":code})),
                token("new"),
                receipt("id"),
            ])
            .await;
            let sender = server.sender();
            assert_eq!(
                sender.send(&destination(), "first").await,
                rejected("wecom_rejected")
            );
            assert_eq!(
                sender
                    .send(&destination(), "independent-but-too-soon")
                    .await,
                rejected("credential_unavailable")
            );
            assert_eq!(server.captured().len(), 2);
            {
                let mut state = sender.token.lock().await;
                assert!(state.cached.is_none());
                assert!(state.retry_at > Instant::now());
                state.retry_at = Instant::now();
            }
            assert!(matches!(
                sender.send(&destination(), "future independent").await,
                DeliveryOutcome::Delivered { .. }
            ));
            let requests = server.captured();
            assert_eq!(requests.len(), 4);
            assert_eq!(requests[3].query["access_token"], "new");
        }
        let server = Server::new(vec![
            token("old"),
            receipt("one"),
            token("new"),
            receipt("two"),
        ])
        .await;
        let sender = server.sender();
        assert!(matches!(
            sender.send(&destination(), "first").await,
            DeliveryOutcome::Delivered { .. }
        ));
        {
            let mut state = sender.token.lock().await;
            assert!(
                state.cached.as_ref().unwrap().usable_until
                    <= Instant::now() + Duration::from_secs(7140)
            );
            state.cached.as_mut().unwrap().usable_until = Instant::now();
        }
        assert!(matches!(
            sender.send(&destination(), "second").await,
            DeliveryOutcome::Delivered { .. }
        ));
        assert_eq!(server.captured().len(), 4);
    }

    #[tokio::test]
    async fn stale_auth_response_does_not_evict_a_newer_token() {
        let gate = Arc::new(Semaphore::new(0));
        let server = Server::new(vec![
            token("old"),
            Reply::json(&json!({"errcode":42_001})).gated(gate.clone()),
            receipt("id"),
        ])
        .await;
        let sender = Arc::new(server.sender());
        let active = sender.clone();
        let task = tokio::spawn(async move { active.send(&destination(), "first").await });
        server.wait_requests(2).await;
        sender.token.lock().await.cached = Some(CachedToken {
            value: "new".into(),
            usable_until: Instant::now() + Duration::from_secs(60),
        });
        gate.add_permits(1);
        assert_eq!(task.await.unwrap(), rejected("wecom_rejected"));
        assert!(matches!(
            sender.send(&destination(), "second").await,
            DeliveryOutcome::Delivered { .. }
        ));
        assert_eq!(server.captured().len(), 3);
        assert_eq!(server.captured()[2].query["access_token"], "new");
    }

    #[tokio::test]
    async fn refresh_wait_cancellation_and_http_timeouts_are_bounded() {
        let server = Server::new(vec![]).await;
        let sender = Arc::new(server.sender());
        let lock = sender.token.lock().await;
        tokio::time::pause();
        let active = sender.clone();
        let task = tokio::spawn(async move { active.send(&destination(), "hello").await });
        tokio::task::yield_now().await;
        tokio::time::advance(TOKEN_BUDGET + Duration::from_millis(1)).await;
        assert_eq!(task.await.unwrap(), rejected("credential_unavailable"));
        drop(lock);
        tokio::time::resume();
        assert!(server.captured().is_empty());
        for token_stage in [true, false] {
            let gate = Arc::new(Semaphore::new(0));
            let replies = if token_stage {
                vec![token("token").gated(gate)]
            } else {
                vec![token("token"), receipt("id").gated(gate)]
            };
            let server = Server::new(replies).await;
            let sender = Arc::new(server.sender());
            let active = sender.clone();
            let task = tokio::spawn(async move { active.send(&destination(), "hello").await });
            server.wait_requests(if token_stage { 1 } else { 2 }).await;
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(13)).await;
            let outcome = task.await.unwrap();
            tokio::time::resume();
            if token_stage {
                assert_eq!(outcome, rejected("credential_unavailable"));
            } else {
                assert!(matches!(outcome, DeliveryOutcome::Unknown { .. }));
            }
            assert_eq!(server.captured().len(), if token_stage { 1 } else { 2 });
        }
        let gate = Arc::new(Semaphore::new(0));
        let server = Server::new(vec![token("token").gated(gate)]).await;
        let sender = Arc::new(server.sender());
        let active = sender.clone();
        let task = tokio::spawn(async move { active.send(&destination(), "hello").await });
        server.wait_requests(1).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            sender.send(&destination(), "later").await,
            rejected("credential_unavailable")
        );
        assert_eq!(server.captured().len(), 1);
    }
}
