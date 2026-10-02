// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One private-member message through an enterprise application's robot.
//!
//! Primary contracts, retrieved 2026-10-03:
//! - <https://open.dingtalk.com/document/development/chatbots-send-one-on-one-chat-messages-in-batches.md>
//! - <https://open.dingtalk.com/document/development/obtain-the-access-token-of-an-internal-app.md>
//! - <https://open.dingtalk.com/document/development/robot-message-type.md>
//! - <https://open.dingtalk.com/document/development/server-api-error-codes-1.md>
//! - <https://github.com/alibabacloud-go/dingtalk/blob/1986c966942afc67b8ccae59d29d57989a45fe76/robot_1_0/client.go>
//!
//! A processQueryKey acknowledges admission, not completed delivery/read status.
//! No sessionWebhook, recipient discovery, message replay or downstream polling.

use anyhow::{ensure, Result};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::{sync::Mutex, time::Instant};

use crate::{
    channel_types::{Channel, Destination},
    outbound::{
        http_client, rejected, response_body, unknown, validate_api_base, validate_destination,
        DeliveryOutcome, MAX_PART_UTF16, MAX_TEXT_BYTES,
    },
};

const TOKEN_BUDGET: Duration = Duration::from_secs(12);
const TOKEN_BACKOFF: Duration = Duration::from_secs(30);
const TOKEN_EXPIRY_MARGIN: u64 = 60;

// Memory-only credentials: deliberately no Debug or Serialize.
pub(super) struct DingTalkSender {
    installation_id: String,
    robot_code: String,
    app_key: String,
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

impl DingTalkSender {
    pub(super) fn new(
        installation_id: &str,
        app_key: String,
        app_secret: String,
        mut api_base: String,
        allow_loopback: bool,
    ) -> Result<Self> {
        let (robot_code, _) = super::dingtalk::validate_installation(installation_id)?;
        ensure!(
            super::dingtalk::identity(&app_key),
            "invalid DingTalk app key"
        );
        ensure!(
            !app_secret.is_empty()
                && app_secret.len() <= 4096
                && app_secret.bytes().all(|b| b.is_ascii_graphic()),
            "invalid DingTalk app secret"
        );
        validate_api_base(Channel::Dingtalk, &api_base, allow_loopback)?;
        api_base.truncate(api_base.trim_end_matches('/').len());
        Ok(Self {
            installation_id: installation_id.into(),
            robot_code: robot_code.into(),
            app_key,
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
            // Single refresh flight, with bounded lock wait and HTTP. Arming
            // cooldown before the request also contains refresh cancellation.
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
            .post(format!("{}/v1.0/oauth2/accessToken", self.api_base))
            .json(&json!({"appKey":self.app_key,"appSecret":self.app_secret}))
            .send()
            .await
            .ok()?;
        if response.status() != StatusCode::OK {
            return None;
        }
        let body = response_body(response).await.ok()?;
        let token: TokenEnvelope = serde_json::from_slice(&body).ok()?;
        if token.code.is_some()
            || token.expire_in <= TOKEN_EXPIRY_MARGIN
            || token.access_token.is_empty()
            || token.access_token.len() > 4096
            || !token.access_token.bytes().all(|b| b.is_ascii_graphic())
        {
            return None;
        }
        let usable_until =
            started + Duration::from_secs(token.expire_in.min(7200) - TOKEN_EXPIRY_MARGIN);
        (Instant::now() < usable_until).then_some(CachedToken {
            value: token.access_token,
            usable_until,
        })
    }

    pub(super) async fn send(&self, destination: &Destination, text: &str) -> DeliveryOutcome {
        if destination.channel != Channel::Dingtalk
            || destination.installation_id != self.installation_id
            || validate_destination(destination).is_err()
        {
            return rejected("invalid_destination");
        }
        // Local resource bound, not a claim about the platform's undocumented
        // numeric text limit. The API documents an explicit tooLong refusal.
        if text.is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.encode_utf16().count() > MAX_PART_UTF16
        {
            return rejected("invalid_text");
        }
        let Some(token) = self.access_token().await else {
            return rejected("credential_unavailable");
        };
        let result = self
            .client
            .post(format!(
                "{}/v1.0/robot/oToMessages/batchSend",
                self.api_base
            ))
            .header("x-acs-dingtalk-access-token", &token)
            .json(
                &json!({"robotCode":self.robot_code,"userIds":[destination.conversation_id],
                "msgKey":"sampleText","msgParam":json!({"content":text}).to_string()}),
            )
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
            return unknown("dingtalk_rate_limit");
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
        if envelope.code.as_deref().is_some_and(token_rejected) {
            self.invalidate_token(&token);
        }
        parse_receipt(status, envelope)
    }

    fn invalidate_token(&self, used: &str) {
        // A stale response must not evict a concurrently refreshed token.
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
#[serde(rename_all = "camelCase")]
struct TokenEnvelope {
    access_token: String,
    expire_in: u64,
    code: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageEnvelope {
    process_query_key: Option<String>,
    invalid_staff_id_list: Option<Vec<String>>,
    flow_controlled_staff_id_list: Option<Vec<String>>,
    // The pinned official SDK exposes this even though the prose page omits it.
    filtered_staff_id_list: Option<Vec<String>>,
    code: Option<String>,
}
fn parse_receipt(status: StatusCode, envelope: MessageEnvelope) -> DeliveryOutcome {
    if [
        &envelope.invalid_staff_id_list,
        &envelope.flow_controlled_staff_id_list,
        &envelope.filtered_staff_id_list,
    ]
    .into_iter()
    .flatten()
    .any(|list| !list.is_empty())
    {
        return unknown("dingtalk_partial_delivery");
    }
    if let Some(code) = &envelope.code {
        if envelope
            .process_query_key
            .as_ref()
            .is_some_and(|key| !key.is_empty())
        {
            return unknown("dingtalk_conflicting_receipt");
        }
        // Only the documented status/code combinations prove a refusal.
        if (status == StatusCode::BAD_REQUEST && explicitly_rejected(code))
            || (status == StatusCode::FORBIDDEN
                && matches!(
                    code.as_str(),
                    "Forbidden.AccessDenied.AccessTokenPermissionDenied"
                        | "Forbidden.AccessDenied.IpNotInWhiteList"
                ))
        {
            return rejected("dingtalk_rejected");
        }
        return unknown("dingtalk_error");
    }
    if status != StatusCode::OK {
        return unknown("unexpected_http_status");
    }
    let Some(receipt) = envelope.process_query_key.filter(|key| {
        !key.trim().is_empty() && key.len() <= 4096 && !key.chars().any(char::is_control)
    }) else {
        return unknown("invalid_receipt");
    };
    DeliveryOutcome::Delivered { receipt }
}
fn token_rejected(code: &str) -> bool {
    matches!(
        code,
        "InvalidAuthentication" | "invalidParameter.token.invalid" | "token.notExisted"
    )
}
fn explicitly_rejected(code: &str) -> bool {
    token_rejected(code)
        || matches!(
            code,
            "invalidParameter.robotCode.empty"
                | "invalidParameter.userIds.empty"
                | "invalidParameter.userIds.overMax"
                | "invalidParameter.msgKey.empty"
                | "invalidParameter.msgKey.invalid"
                | "invalidParameter.msgParam.invalid"
                | "invalidParameter.param.invalid"
                | "invalidParameter.msg.unsupport"
                | "invalidParameter.msgParam.tooLong"
                | "invalidParameter.robotCode.notExsit"
                | "invalidParameter.msgBody.invalid"
                | "invalidParameter.userId.empty"
                | "invalidParameter.robotCode.invalid"
                | "template.not.existed"
                | "template.stopped"
                | "send.forbidden"
                | "ip.not.match"
                | "sign.not.match"
                | "illegal.receivers"
                | "staffId.notExisted"
                | "chatbotId.notAllow.sendOTO"
                | "robot.oto.notExist"
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
    const INSTALLATION: &str = "dingRobot:dingCorp";
    const APP_KEY: &str = "dingDifferentClient";
    const SECRET: &str = "fixture-secret?&=+#/:";
    #[derive(Clone, Debug)]
    struct Request {
        method: String,
        path: String,
        query: BTreeMap<String, String>,
        authorization: Option<String>,
        token: Option<String>,
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
            let base = format!("http://{}", listener.local_addr().unwrap());
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
                            token: header("x-acs-dingtalk-access-token"),
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
        fn sender(&self) -> DingTalkSender {
            DingTalkSender::new(
                INSTALLATION,
                APP_KEY.into(),
                SECRET.into(),
                self.base.clone(),
                true,
            )
            .unwrap()
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
            channel: Channel::Dingtalk,
            installation_id: INSTALLATION.into(),
            conversation_id: "Member_One".into(),
            thread_id: None,
            interaction_id: None,
            expires_ms: None,
        }
    }
    fn token(value: &str) -> Reply {
        Reply::json(&json!({"accessToken":value,"expireIn":7200}))
    }
    fn receipt(id: &str) -> Reply {
        Reply::json(&json!({"processQueryKey":id}))
    }

    #[tokio::test]
    async fn private_text_uses_fixed_robot_exact_member_and_distinct_app_key() {
        let server = Server::new(vec![
            token("token?&=+#/"),
            receipt("first"),
            receipt("second"),
        ])
        .await;
        let sender = server.sender();
        let text = "文字 <a> **plain** @all \"quote\" \\path\n🙂";
        for id in ["first", "second"] {
            assert_eq!(
                sender.send(&destination(), text).await,
                DeliveryOutcome::Delivered { receipt: id.into() }
            );
        }
        let records = server.captured();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].method, "POST");
        assert_eq!(records[0].path, "/v1.0/oauth2/accessToken");
        assert_eq!(
            records[0].body.as_ref().unwrap(),
            &json!({"appKey":APP_KEY,"appSecret":SECRET})
        );
        assert!(records[0].token.is_none());
        for record in &records[1..] {
            assert_eq!(record.path, "/v1.0/robot/oToMessages/batchSend");
            assert_eq!(record.method, "POST");
            assert_eq!(record.token.as_deref(), Some("token?&=+#/"));
            assert_eq!(
                record.body.as_ref().unwrap(),
                &json!({"robotCode":"dingRobot","userIds":["Member_One"],"msgKey":"sampleText","msgParam":json!({"content":text}).to_string()})
            );
        }
        assert!(records
            .iter()
            .all(|r| r.authorization.is_none() && r.query.is_empty()));
        assert_eq!(
            records[1].body, records[2].body,
            "independent equal text is sent twice"
        );
    }

    #[tokio::test]
    async fn invalid_destinations_credentials_and_text_never_fetch_tokens() {
        let server = Server::new(vec![]).await;
        for base in [
            "https://attacker.invalid",
            "http://api.dingtalk.com",
            "https://api.dingtalk.com/v1.0",
            "http://localhost:80",
            "http://127.1",
            "http://user@127.0.0.1",
            "http://127.0.0.1/v1.0",
            "http://127.0.0.1?x=y",
            "http://127.0.0.1/#fragment",
        ] {
            assert!(
                DingTalkSender::new(
                    INSTALLATION,
                    APP_KEY.into(),
                    SECRET.into(),
                    base.into(),
                    true
                )
                .is_err(),
                "{base}"
            );
        }
        assert!(DingTalkSender::new(
            INSTALLATION,
            APP_KEY.into(),
            SECRET.into(),
            server.base.clone(),
            false
        )
        .is_err());
        for app in ["", "bad/key", "bad\nkey", &"x".repeat(129)] {
            assert!(DingTalkSender::new(
                INSTALLATION,
                app.into(),
                SECRET.into(),
                server.base.clone(),
                true
            )
            .is_err());
        }
        for secret in ["", "line\nsecret", &"x".repeat(4097)] {
            assert!(DingTalkSender::new(
                INSTALLATION,
                APP_KEY.into(),
                secret.into(),
                server.base.clone(),
                true
            )
            .is_err());
        }
        let sender = server.sender();
        for field in [
            "channel",
            "installation",
            "conversation",
            "thread",
            "interaction",
            "expiry",
        ] {
            let mut dest = destination();
            match field {
                "channel" => dest.channel = Channel::Wecom,
                "installation" => dest.installation_id = "dingOther:dingCorp".into(),
                "conversation" => dest.conversation_id = String::new(),
                "thread" => dest.thread_id = Some("thread".into()),
                "interaction" => dest.interaction_id = Some("interaction".into()),
                _ => dest.expires_ms = Some(1),
            }
            assert_eq!(
                sender.send(&dest, "hello").await,
                rejected("invalid_destination")
            );
        }
        for text in [
            String::new(),
            "a".repeat(2001),
            "🙂".repeat(1001),
            "a".repeat(16385),
        ] {
            assert_eq!(
                sender.send(&destination(), &text).await,
                rejected("invalid_text")
            );
        }
        assert!(server.captured().is_empty());
    }

    #[tokio::test]
    async fn admission_requires_one_valid_key_and_all_three_failure_lists_empty() {
        let mut bodies = vec![
            json!({}),
            json!({"processQueryKey":""}),
            json!({"processQueryKey":" "}),
            json!({"processQueryKey":"key\n"}),
            json!({"processQueryKey":1}),
            json!({"processQueryKey":"x".repeat(4097)}),
        ];
        for field in [
            "invalidStaffIdList",
            "flowControlledStaffIdList",
            "filteredStaffIdList",
        ] {
            for value in [json!(["Member_One"]), json!("bad-type"), json!([null])] {
                let mut body = json!({"processQueryKey":"key"});
                body[field] = value;
                bodies.push(body);
            }
        }
        for body in bodies {
            let server = Server::new(vec![token("token"), Reply::json(&body)]).await;
            assert!(matches!(
                server.sender().send(&destination(), "text").await,
                DeliveryOutcome::Unknown { .. }
            ));
            assert_eq!(server.captured().len(), 2);
        }
        for empty in [Value::Null, json!([])] {
            let server=Server::new(vec![token("token"),Reply::json(&json!({"processQueryKey":"accepted","invalidStaffIdList":empty,"flowControlledStaffIdList":empty,"filteredStaffIdList":empty}))]).await;
            assert_eq!(
                server.sender().send(&destination(), "text").await,
                DeliveryOutcome::Delivered {
                    receipt: "accepted".into()
                }
            );
        }
    }

    #[tokio::test]
    async fn ambiguous_errors_rate_limits_redirects_and_conflicting_receipts_never_retry() {
        let mut replies = vec![
            Reply::raw(400, "", r#"{"processQueryKey":"accepted"}"#),
            Reply::raw(401, "", "private platform body"),
            Reply::raw(403, "", ""),
            Reply::raw(429, "Retry-After: 1\r\n", r#"{"code":"send.too.fast"}"#),
            Reply::raw(502, "", r#"{"code":"system.error"}"#),
            Reply::raw(307, "Location: /elsewhere\r\n", ""),
            Reply::raw(200, "", "not JSON"),
            Reply::raw(200, "", &"x".repeat(65537)),
            Reply {
                wire: None,
                gate: None,
            },
        ];
        for code in [
            "send.too.fast",
            "send.byToken.tooFast",
            "too.many.people",
            "too.many.group",
            "Forbidden.AccessDenied.QpsLimitForAppkeyAndApi",
            "unknown.code",
        ] {
            replies.push(Reply::raw(400, "", &json!({"code":code}).to_string()));
        }
        for status in [200, 400] {
            replies.push(Reply::raw(
                status,
                "",
                &json!({"code":"InvalidAuthentication","processQueryKey":"accepted"}).to_string(),
            ));
        }
        for reply in replies {
            let server = Server::new(vec![token("token"), reply]).await;
            assert!(matches!(
                server.sender().send(&destination(), "text").await,
                DeliveryOutcome::Unknown { .. }
            ));
            assert_eq!(server.captured().len(), 2);
        }
        for (status, code) in [
            (400, "InvalidAuthentication"),
            (400, "invalidParameter.msgParam.tooLong"),
            (400, "robot.oto.notExist"),
            (400, "staffId.notExisted"),
            (403, "Forbidden.AccessDenied.AccessTokenPermissionDenied"),
            (403, "Forbidden.AccessDenied.IpNotInWhiteList"),
        ] {
            let server = Server::new(vec![
                token("token"),
                Reply::raw(status, "", &json!({"code":code}).to_string()),
            ])
            .await;
            assert_eq!(
                server.sender().send(&destination(), "text").await,
                rejected("dingtalk_rejected")
            );
            assert_eq!(server.captured().len(), 2);
        }
    }

    #[tokio::test]
    async fn credential_failures_back_off_without_message_calls_or_secret_errors() {
        for failure in [
            Reply::raw(401, "", "private-secret"),
            Reply::raw(503, "", "private-secret"),
            Reply::raw(302, "Location: /elsewhere\r\n", ""),
            Reply::raw(200, "", "{}"),
            Reply::json(&json!({"accessToken":"token","expireIn":60})),
            Reply::json(
                &json!({"accessToken":"token","expireIn":7200,"code":"invalidClientIdOrSecret"}),
            ),
            Reply::json(&json!({"accessToken":"token\n","expireIn":7200})),
            Reply::json(&json!({"accessToken":"x".repeat(4097),"expireIn":7200})),
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
                    sender.send(&destination(), "text").await,
                    rejected("credential_unavailable")
                );
            }
            assert_eq!(server.captured().len(), 1);
        }
    }

    #[tokio::test]
    async fn one_refresh_serves_concurrent_calls_and_token_expiry_is_capped() {
        let gate = Arc::new(Semaphore::new(0));
        let mut replies = vec![
            Reply::json(&json!({"accessToken":"token","expireIn":u64::MAX})).gated(gate.clone()),
        ];
        replies.extend((0..8).map(|_| receipt("accepted")));
        let server = Server::new(replies).await;
        let sender = Arc::new(server.sender());
        let mut tasks = JoinSet::new();
        for _ in 0..8 {
            let s = sender.clone();
            tasks.spawn(async move { s.send(&destination(), "text").await });
        }
        server.wait_requests(1).await;
        gate.add_permits(1);
        while let Some(result) = tasks.join_next().await {
            assert!(matches!(result.unwrap(), DeliveryOutcome::Delivered { .. }));
        }
        assert_eq!(
            server
                .captured()
                .iter()
                .filter(|r| r.path.ends_with("accessToken"))
                .count(),
            1
        );
        let state = sender.token.lock().await;
        assert!(
            state.cached.as_ref().unwrap().usable_until
                <= Instant::now() + Duration::from_secs(7140)
        );
    }

    #[tokio::test]
    async fn expired_or_invalid_tokens_only_refresh_future_independent_calls() {
        for expired in [false, true] {
            let first = if expired {
                receipt("first")
            } else {
                Reply::raw(400, "", r#"{"code":"InvalidAuthentication"}"#)
            };
            let server =
                Server::new(vec![token("old"), first, token("new"), receipt("second")]).await;
            let sender = server.sender();
            let outcome = sender.send(&destination(), "first").await;
            if expired {
                assert!(matches!(outcome, DeliveryOutcome::Delivered { .. }));
            } else {
                assert_eq!(outcome, rejected("dingtalk_rejected"));
                assert_eq!(
                    sender
                        .send(&destination(), "independent but too soon")
                        .await,
                    rejected("credential_unavailable")
                );
            }
            assert_eq!(server.captured().len(), 2);
            {
                let mut state = sender.token.lock().await;
                if expired {
                    state.cached.as_mut().unwrap().usable_until = Instant::now();
                } else {
                    assert!(state.cached.is_none());
                    assert!(state.retry_at > Instant::now());
                    state.retry_at = Instant::now();
                }
            }
            assert!(matches!(
                sender.send(&destination(), "future").await,
                DeliveryOutcome::Delivered { .. }
            ));
            assert_eq!(server.captured().len(), 4);
            assert_eq!(server.captured()[3].token.as_deref(), Some("new"));
        }
    }

    #[tokio::test]
    async fn stale_auth_response_does_not_evict_new_token() {
        let gate = Arc::new(Semaphore::new(0));
        let server = Server::new(vec![
            token("old"),
            Reply::raw(400, "", r#"{"code":"InvalidAuthentication"}"#).gated(gate.clone()),
            receipt("accepted"),
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
        assert_eq!(task.await.unwrap(), rejected("dingtalk_rejected"));
        assert!(matches!(
            sender.send(&destination(), "next").await,
            DeliveryOutcome::Delivered { .. }
        ));
        assert_eq!(server.captured().len(), 3);
        assert_eq!(server.captured()[2].token.as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn lock_wait_http_and_cancellation_are_bounded() {
        let server = Server::new(vec![]).await;
        let sender = Arc::new(server.sender());
        let lock = sender.token.lock().await;
        tokio::time::pause();
        let active = sender.clone();
        let task = tokio::spawn(async move { active.send(&destination(), "text").await });
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
                vec![token("token"), receipt("accepted").gated(gate)]
            };
            let server = Server::new(replies).await;
            let sender = Arc::new(server.sender());
            let active = sender.clone();
            let task = tokio::spawn(async move { active.send(&destination(), "text").await });
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
        let task = tokio::spawn(async move { active.send(&destination(), "text").await });
        server.wait_requests(1).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            sender.send(&destination(), "next").await,
            rejected("credential_unavailable")
        );
        assert_eq!(server.captured().len(), 1);
    }
}
