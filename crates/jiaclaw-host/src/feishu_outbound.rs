// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Single-tenant self-built Feishu app delivery, with a memory-only token cache.
//!
//! Contract references:
//! - <https://open.feishu.cn/document/server-docs/im-v1/message/create>
//! - <https://open.feishu.cn/document/server-docs/im-v1/message/reply>
//! - <https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal>
//! - <https://open.feishu.cn/document/ukTMukTMukTM/uUzN04SN3QjL1cDN>
//! - <https://open.feishu.cn/document/ukTMukTMukTM/ugjM14COyUjL4ITN>
//! - <https://github.com/larksuite/oapi-sdk-go/blob/99927aa13e271ea9fe03591204aad7bc6a2d869c/service/im/v1/model.go>
//!
//! `thread_id` is the root `om_` message ID, never an `omt_` thread ID. Text
//! replaces ASCII angle brackets with full-width characters to disable Feishu's
//! inline mention/style syntax. Rich text and cards are deliberately unsupported.

use anyhow::{ensure, Result};
use reqwest::{header::HeaderMap, Client, StatusCode};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{sync::Mutex, time::Instant};

use crate::{
    channel_types::{Channel, Destination},
    outbound::{
        feishu_id, http_client, rejected, response_body, unknown, validate_api_base,
        validate_destination, DeliveryOutcome, MAX_PART_UTF16, MAX_TEXT_BYTES,
    },
};

const TOKEN_BUDGET: Duration = Duration::from_secs(12);
const TOKEN_BACKOFF: Duration = Duration::from_secs(30);
const TOKEN_EXPIRY_MARGIN_SECS: u64 = 60;

// Intentionally no Debug/Serialize implementations: secrets never enter
// diagnostics, stored delivery records, or management API responses.
pub(super) struct FeishuSender {
    installation_id: String,
    app_id: String,
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

impl FeishuSender {
    /// Read-only startup identity evidence. Human/chat association is separately
    /// established by each authenticated p2p callback, not a fabricated member list.
    pub(super) async fn verify_installation(
        &self,
        bot_open_id: &str,
        tenant_key: &str,
        chat_id: &str,
    ) -> Result<()> {
        #[derive(Deserialize)]
        struct BotEnvelope {
            code: i64,
            bot: Box<RawValue>,
        }
        #[derive(Deserialize)]
        struct Bot {
            activate_status: u8,
            open_id: String,
        }
        #[derive(Deserialize)]
        struct Envelope {
            code: i64,
            data: Box<RawValue>,
        }
        #[derive(Deserialize)]
        struct TenantData {
            tenant: Box<RawValue>,
        }
        #[derive(Deserialize)]
        struct Tenant {
            tenant_key: String,
        }
        #[derive(Deserialize)]
        struct Chat {
            chat_mode: String,
            tenant_key: Option<String>,
            external: Option<bool>,
        }
        let verified = tokio::time::timeout(Duration::from_secs(30), async {
            ensure!(
                feishu_id(bot_open_id, "ou_") && feishu_id(chat_id, "oc_"),
                "invalid installation identity"
            );
            let token = self
                .access_token()
                .await
                .ok_or_else(|| anyhow::anyhow!("verification credential unavailable"))?;
            let get = |path: String| {
                self.client
                    .get(format!("{}{}", self.api_base, path))
                    .bearer_auth(&token)
                    .timeout(Duration::from_secs(5))
            };
            let bot: BotEnvelope = self.verification_json(get("/bot/v3/info".into())).await?;
            let identity: Bot = crate::feishu::decode_object(bot.bot.get().as_bytes())?;
            ensure!(
                bot.code == 0 && identity.activate_status == 2 && identity.open_id == bot_open_id,
                "bot identity mismatch"
            );
            let tenant: Envelope = self
                .verification_json(get("/tenant/v2/tenant/query".into()))
                .await?;
            let data: TenantData = crate::feishu::decode_object(tenant.data.get().as_bytes())?;
            let identity: Tenant = crate::feishu::decode_object(data.tenant.get().as_bytes())?;
            ensure!(
                tenant.code == 0 && identity.tenant_key == tenant_key,
                "tenant identity mismatch"
            );
            let chat: Envelope = self
                .verification_json(get(format!("/im/v1/chats/{chat_id}")))
                .await?;
            let identity: Chat = crate::feishu::decode_object(chat.data.get().as_bytes())?;
            ensure!(
                chat.code == 0
                    && identity.chat_mode == "p2p"
                    && identity
                        .tenant_key
                        .as_deref()
                        .is_none_or(|key| key == tenant_key)
                    && identity.external != Some(true),
                "private chat mismatch"
            );
            Ok::<(), anyhow::Error>(())
        })
        .await;
        // Keep response bodies, credentials and reqwest diagnostics out of errors.
        ensure!(
            matches!(verified, Ok(Ok(()))),
            "Feishu installation verification failed"
        );
        Ok(())
    }

    async fn verification_json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let response = request.send().await?;
        ensure!(
            response.status() == StatusCode::OK && json_mime(response.headers()),
            "installation response rejected"
        );
        let bytes = response_body(response)
            .await
            .map_err(|_| anyhow::anyhow!("invalid installation response"))?;
        crate::feishu::decode_object(&bytes)
    }

    pub(super) fn new(
        installation_id: &str,
        app_secret: String,
        mut api_base: String,
        allow_loopback: bool,
    ) -> Result<Self> {
        let (app_id, _) = super::feishu::validate_installation(installation_id)?;
        ensure!(
            !app_secret.is_empty()
                && app_secret.len() <= 4096
                && app_secret.bytes().all(|b| b.is_ascii_graphic()),
            "invalid Feishu app secret"
        );
        validate_api_base(Channel::Feishu, &api_base, allow_loopback)?;
        api_base.truncate(api_base.trim_end_matches('/').len());
        Ok(Self {
            installation_id: installation_id.into(),
            app_id: app_id.into(),
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
            // This bounded critical section is also the single-flight refresh.
            // No detached task outlives the caller. Pre-arm the failure cooldown
            // before HTTP so cancellation cannot cause a refresh stampede.
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
            let refreshed = self.fetch_token(now).await;
            if let Some(token) = refreshed {
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
            .post(format!(
                "{}/auth/v3/tenant_access_token/internal",
                self.api_base
            ))
            .json(&json!({"app_id": self.app_id, "app_secret": self.app_secret}))
            .send()
            .await
            .ok()?;
        if response.status() != StatusCode::OK {
            return None;
        }
        if !json_mime(response.headers()) {
            return None;
        }
        let body = response_body(response).await.ok()?;
        let token: TokenEnvelope = crate::feishu::decode_object(&body).ok()?;
        if token.code != 0
            || !(TOKEN_EXPIRY_MARGIN_SECS + 1..=7200).contains(&token.expire)
            || token.tenant_access_token.is_empty()
            || token.tenant_access_token.len() > 4096
            || !token
                .tenant_access_token
                .bytes()
                .all(|b| b.is_ascii_graphic())
        {
            return None;
        }
        let usable_until = started + Duration::from_secs(token.expire - TOKEN_EXPIRY_MARGIN_SECS);
        (Instant::now() < usable_until).then_some(CachedToken {
            value: token.tenant_access_token,
            usable_until,
        })
    }

    pub(super) async fn send(
        &self,
        destination: &Destination,
        delivery_id: &str,
        text: &str,
    ) -> DeliveryOutcome {
        if destination.channel != Channel::Feishu
            || destination.installation_id != self.installation_id
            || validate_destination(destination).is_err()
        {
            return rejected("invalid_destination");
        }
        if uuid::Uuid::parse_str(delivery_id).map_or(true, |id| {
            id.is_nil() || id.hyphenated().to_string() != delivery_id
        }) {
            return rejected("invalid_delivery_id");
        }
        if text.is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.encode_utf16().count() > MAX_PART_UTF16
        {
            return rejected("invalid_text");
        }
        let Some(token) = self.access_token().await else {
            // Only auth HTTP has happened. The message POST was never submitted.
            return rejected("credential_unavailable");
        };
        let content = json!({"text": text.replace('<', "＜").replace('>', "＞")}).to_string();
        let mut body = json!({"msg_type":"text", "content":content, "uuid":delivery_id});
        let url = if let Some(root) = &destination.thread_id {
            body["reply_in_thread"] = json!(true);
            format!("{}/im/v1/messages/{root}/reply", self.api_base)
        } else {
            body["receive_id"] = json!(destination.conversation_id);
            format!("{}/im/v1/messages?receive_id_type=chat_id", self.api_base)
        };
        // Once this POST begins every transport/cancellation ambiguity is an
        // unknown outcome. Never refresh and resend after a token error.
        let Ok(response) = self
            .client
            .post(url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
        else {
            return unknown("transport_error");
        };
        let status = response.status();
        if status.is_server_error() {
            return unknown("http_server_error");
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            if status == StatusCode::UNAUTHORIZED {
                self.invalidate_token(&token);
            }
            return rejected("http_client_error");
        }
        if !status.is_success() && !status.is_client_error() {
            return unknown("unexpected_http_status");
        }
        let headers = response.headers().clone();
        if !json_mime(&headers) {
            return unknown("invalid_response_type");
        }
        let body = match response_body(response).await {
            Ok(body) => body,
            Err(code) => return unknown(code),
        };
        if crate::feishu::decode_object::<ErrorCode>(&body)
            .is_ok_and(|error| matches!(error.code, 99_991_663 | 99_991_665))
        {
            self.invalidate_token(&token);
        }
        parse_response(destination, status, &headers, &body)
    }

    fn invalidate_token(&self, used: &str) {
        // A busy lock means a refresh already owns the empty cache. A different
        // cached value belongs to a newer request and must not be invalidated.
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

fn json_mime(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(reqwest::header::CONTENT_TYPE).iter();
    values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        && values.next().is_none()
}

#[derive(Deserialize)]
struct ErrorCode {
    code: i64,
}

#[derive(Deserialize)]
struct TokenEnvelope {
    code: i64,
    expire: u64,
    tenant_access_token: String,
}

#[derive(Deserialize)]
struct MessageEnvelope {
    code: i64,
    data: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct MessageReceipt {
    message_id: String,
    chat_id: String,
    msg_type: String,
    root_id: Option<String>,
    parent_id: Option<String>,
}

fn rate_limit_delay(headers: &HeaderMap) -> Option<i64> {
    let mut values = headers.get_all("x-ogw-ratelimit-reset").iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: i64 = value.parse().ok()?;
    (1..=3600).contains(&seconds).then(|| seconds * 1000)
}

fn parse_response(
    destination: &Destination,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> DeliveryOutcome {
    let Ok(envelope) = crate::feishu::decode_object::<MessageEnvelope>(body) else {
        return unknown("invalid_response");
    };
    if matches!(envelope.code, 230_049 | 18_121) {
        return unknown("feishu_in_progress");
    }
    if status == StatusCode::TOO_MANY_REQUESTS || matches!(envelope.code, 99_991_400 | 230_020) {
        let supported = ((envelope.code == 99_991_400
            && matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS | StatusCode::BAD_REQUEST
            ))
            || (envelope.code == 230_020 && status == StatusCode::BAD_REQUEST))
            && envelope.data.as_ref().is_none_or(|data| {
                serde_json::from_str::<Value>(data.get())
                    .is_ok_and(|value| value.as_object().is_some_and(serde_json::Map::is_empty))
            });
        return if let Some(retry_after_ms) = supported.then(|| rate_limit_delay(headers)).flatten()
        {
            DeliveryOutcome::RateLimited { retry_after_ms }
        } else {
            unknown("invalid_rate_limit")
        };
    }
    if explicitly_rejected(envelope.code) {
        return rejected("feishu_rejected");
    }
    if envelope.code != 0 || !status.is_success() {
        return unknown("feishu_error");
    }
    let Some(data) = envelope.data else {
        return unknown("invalid_receipt");
    };
    let Ok(message) = crate::feishu::decode_object::<MessageReceipt>(data.get().as_bytes()) else {
        return unknown("invalid_receipt");
    };
    let thread_matches = match destination.thread_id.as_deref() {
        Some(root) => {
            message.message_id != root
                && (message.root_id.as_deref() == Some(root)
                    || message.parent_id.as_deref() == Some(root))
                && [message.root_id.as_deref(), message.parent_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .all(|id| id.is_empty() || id == root)
        }
        None => {
            message.parent_id.as_deref().is_none_or(str::is_empty)
                && message
                    .root_id
                    .as_deref()
                    .is_none_or(|id| id.is_empty() || id == message.message_id)
        }
    };
    if message.chat_id != destination.conversation_id
        || !feishu_id(&message.message_id, "om_")
        || message.msg_type != "text"
        || !thread_matches
    {
        return unknown("invalid_receipt");
    }
    DeliveryOutcome::Delivered {
        receipt: message.message_id,
    }
}

// Documented validation/permission/state refusals for create/reply. Unknown
// business codes (even with HTTP 400) cannot establish that no send occurred.
fn explicitly_rejected(code: i64) -> bool {
    matches!(
        code,
        230_001
            | 230_002
            | 230_006
            | 230_011
            | 230_013
            | 230_018
            | 230_019
            | 230_022
            | 230_025
            | 230_027
            | 230_028
            | 230_035
            | 230_050
            | 230_071
            | 230_072
            | 230_075
            | 230_111
            | 232_009
            | 99_991_661
            | 99_991_662
            | 99_991_663
            | 99_991_664
            | 99_991_665
            | 99_991_671
            | 99_991_672
            | 99_991_673
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{collections::VecDeque, sync::Arc};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Semaphore,
        task::{JoinHandle, JoinSet},
    };

    const INSTALLATION: &str = "cli_testapp:tenant-test";
    const DELIVERY: &str = "12345678-1234-4234-9234-123456789abc";

    #[derive(Debug, Clone)]
    struct Request {
        path: String,
        authorization: Option<String>,
        body: Value,
    }

    struct Reply {
        wire: Option<Vec<u8>>,
        gate: Option<Arc<Semaphore>>,
    }

    impl Reply {
        fn raw(status: u16, headers: &str, body: &str) -> Self {
            Self {
                wire: Some(
                    format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                        body.len()
                    )
                    .into_bytes(),
                ),
                gate: None,
            }
        }

        fn json(body: &Value) -> Self {
            Self::raw(200, "", &body.to_string())
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
            let base = format!("http://{}/open-apis", listener.local_addr().unwrap());
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let replies = Arc::new(std::sync::Mutex::new(VecDeque::from(replies)));
            let captured = requests.clone();
            let task = tokio::spawn(async move {
                let mut active = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut socket, _) = accepted.unwrap();
                            let captured = captured.clone();
                            let replies = replies.clone();
                            active.spawn(async move {
                                let mut bytes = Vec::new();
                                let mut buffer = [0_u8; 4096];
                                loop {
                                    let read = socket.read(&mut buffer).await.unwrap();
                                    if read == 0 { return; }
                                    bytes.extend_from_slice(&buffer[..read]);
                                    assert!(bytes.len() < 128 * 1024);
                                    let Some(offset) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else { continue; };
                                    let headers = std::str::from_utf8(&bytes[..offset]).unwrap();
                                    let length = headers.lines().find_map(|line| {
                                        let (name,value)=line.split_once(':')?;
                                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                                    }).unwrap_or(0);
                                    if bytes.len() < offset + 4 + length { continue; }
                                    let request = Request {
                                        path: headers.lines().next().unwrap().split_whitespace().nth(1).unwrap().into(),
                                        authorization: headers.lines().find_map(|line| {
                                            let (name,value)=line.split_once(':')?;
                                            name.eq_ignore_ascii_case("authorization").then(|| value.trim().to_owned())
                                        }),
                                        body: if length == 0 { Value::Null } else { serde_json::from_slice(&bytes[offset + 4..offset + 4 + length]).unwrap() },
                                    };
                                    captured.lock().unwrap().push(request);
                                    let reply = replies.lock().unwrap().pop_front().expect("unexpected extra HTTP attempt");
                                    if let Some(gate) = reply.gate { gate.acquire().await.unwrap().forget(); }
                                    if let Some(wire) = reply.wire { let _ = socket.write_all(&wire).await; }
                                    let _ = socket.shutdown().await;
                                    return;
                                }
                            });
                        },
                        result = active.join_next(), if !active.is_empty() => { result.unwrap().unwrap(); }
                    }
                }
            });
            Self {
                base,
                requests,
                task,
            }
        }

        fn sender(&self) -> FeishuSender {
            FeishuSender::new(
                INSTALLATION,
                "test-app-secret".into(),
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

    fn destination(root: Option<&str>) -> Destination {
        Destination {
            channel: Channel::Feishu,
            installation_id: INSTALLATION.into(),
            conversation_id: "oc_testchat".into(),
            thread_id: root.map(str::to_owned),
            interaction_id: None,
            expires_ms: None,
        }
    }

    fn token(value: &str) -> Reply {
        Reply::json(&json!({"code":0,"expire":7200,"tenant_access_token":value}))
    }

    fn receipt(root: Option<&str>) -> Value {
        let mut data =
            json!({"message_id":"om_receipt", "chat_id":"oc_testchat", "msg_type":"text"});
        if let Some(root) = root {
            data["root_id"] = json!(root);
            data["parent_id"] = json!(root);
        }
        json!({"code":0,"data":data})
    }

    #[tokio::test]
    async fn sends_create_and_root_reply_with_cached_token_uuid_and_literal_text() {
        let server = Server::new(vec![
            token("t-cached"),
            Reply::json(&receipt(None)),
            Reply::json(&receipt(Some("om_root"))),
        ])
        .await;
        let sender = server.sender();
        for root in [None, Some("om_root")] {
            assert_eq!(
                sender
                    .send(
                        &destination(root),
                        DELIVERY,
                        "<at user_id=\"all\">所有人</at>\n1 < 2 & 3 > 2"
                    )
                    .await,
                DeliveryOutcome::Delivered {
                    receipt: "om_receipt".into()
                }
            );
        }
        let requests = server.captured();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0].path,
            "/open-apis/auth/v3/tenant_access_token/internal"
        );
        assert!(requests[0].authorization.is_none());
        assert_eq!(
            requests[0].body,
            json!({"app_id":"cli_testapp","app_secret":"test-app-secret"})
        );
        assert_eq!(
            requests[1].path,
            "/open-apis/im/v1/messages?receive_id_type=chat_id"
        );
        assert_eq!(requests[1].body["receive_id"], "oc_testchat");
        assert!(requests[1].body.get("reply_in_thread").is_none());
        assert_eq!(requests[2].path, "/open-apis/im/v1/messages/om_root/reply");
        assert_eq!(requests[2].body["reply_in_thread"], true);
        assert!(requests[2].body.get("receive_id").is_none());
        for request in &requests[1..] {
            assert_eq!(request.authorization.as_deref(), Some("Bearer t-cached"));
            assert_eq!(request.body["uuid"], DELIVERY);
            assert_eq!(request.body["msg_type"], "text");
            let text: Value =
                serde_json::from_str(request.body["content"].as_str().unwrap()).unwrap();
            assert_eq!(
                text["text"],
                "＜at user_id=\"all\"＞所有人＜/at＞\n1 ＜ 2 & 3 ＞ 2"
            );
            assert!(!request.body.to_string().contains("test-app-secret"));
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_configuration_or_destination_before_token_http() {
        let server = Server::new(vec![]).await;
        for base in [
            "https://attacker.invalid/open-apis",
            "https://open.feishu.cn/open-apis?x=y",
            "http://localhost:8080",
            "http://127.1:8080",
            "http://127.0.0.1:8080/other",
            "http://user@127.0.0.1:8080",
            "http://127.0.0.1:8080/#x",
        ] {
            assert!(FeishuSender::new(INSTALLATION, "secret".into(), base.into(), true).is_err());
        }
        assert!(
            FeishuSender::new(INSTALLATION, "secret".into(), server.base.clone(), false).is_err()
        );
        assert!(FeishuSender::new(
            "cli_app:tenant:other",
            "secret".into(),
            server.base.clone(),
            true
        )
        .is_err());
        let sender = server.sender();
        for mut target in [
            destination(Some("omt_native")),
            destination(Some("om_../other")),
            destination(None),
        ] {
            if target.thread_id.is_none() {
                target.installation_id = "cli_testapp:other-tenant".into();
            }
            assert_eq!(
                sender.send(&target, DELIVERY, "hello").await,
                rejected("invalid_destination")
            );
        }
        for id in [
            "",
            "../anything",
            "12345678123442349234123456789abc",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert_eq!(
                sender.send(&destination(None), id, "hello").await,
                rejected("invalid_delivery_id")
            );
        }
        for text in [String::new(), "a".repeat(2001), "🙂".repeat(1001)] {
            assert_eq!(
                sender.send(&destination(None), DELIVERY, &text).await,
                rejected("invalid_text")
            );
        }
        assert!(server.captured().is_empty());
    }

    #[tokio::test]
    async fn concurrent_sends_share_one_bounded_token_refresh() {
        let gate = Arc::new(Semaphore::new(0));
        let mut replies = vec![token("t-singleflight").gated(gate.clone())];
        replies.extend((0..8).map(|_| Reply::json(&receipt(None))));
        let server = Server::new(replies).await;
        let sender = Arc::new(server.sender());
        let mut tasks = JoinSet::new();
        for _ in 0..8 {
            let sender = sender.clone();
            tasks.spawn(async move {
                sender
                    .send(
                        &destination(None),
                        &uuid::Uuid::new_v4().to_string(),
                        "hello",
                    )
                    .await
            });
        }
        server.wait_requests(1).await;
        assert_eq!(server.captured().len(), 1);
        gate.add_permits(1);
        while let Some(result) = tasks.join_next().await {
            assert!(matches!(result.unwrap(), DeliveryOutcome::Delivered { .. }));
        }
        let requests = server.captured();
        assert_eq!(requests.len(), 9);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.path.contains("/auth/"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn token_failure_never_posts_messages_and_cools_down() {
        let failures = vec![
            Reply::raw(401, "", "secret response text"),
            Reply::raw(503, "", "upstream error"),
            Reply::raw(302, "Location: /elsewhere\r\n", ""),
            Reply::raw(200, "", "{}"),
            Reply::json(&json!({"code":1,"expire":7200,"tenant_access_token":"t-secret"})),
            Reply::json(&json!({"code":0,"expire":60,"tenant_access_token":"t-secret"})),
            Reply::json(&json!({"code":0,"expire":7201,"tenant_access_token":"t-secret"})),
            Reply::json(&json!({"code":0,"expire":7200,"tenant_access_token":"bad\r\ntoken"})),
            Reply::raw(200, "", &"x".repeat(65537)),
            Reply {
                wire: None,
                gate: None,
            },
        ];
        for failure in failures {
            let server = Server::new(vec![failure]).await;
            let sender = server.sender();
            for _ in 0..2 {
                assert_eq!(
                    sender.send(&destination(None), DELIVERY, "hello").await,
                    rejected("credential_unavailable")
                );
            }
            assert_eq!(server.captured().len(), 1);
            assert!(server.captured()[0].path.contains("/auth/"));
        }
    }

    #[tokio::test]
    async fn expired_cache_refreshes_without_reusing_the_old_token() {
        let server = Server::new(vec![
            token("t-old"),
            Reply::json(&receipt(None)),
            token("t-new"),
            Reply::json(&receipt(None)),
        ])
        .await;
        let sender = server.sender();
        assert!(matches!(
            sender.send(&destination(None), DELIVERY, "first").await,
            DeliveryOutcome::Delivered { .. }
        ));
        {
            let mut state = sender.token.lock().await;
            assert!(
                state.cached.as_ref().unwrap().usable_until
                    <= Instant::now() + Duration::from_secs(7140)
            );
            state.cached.as_mut().unwrap().usable_until = Instant::now();
            state.retry_at = Instant::now();
        }
        assert!(matches!(
            sender.send(&destination(None), DELIVERY, "second").await,
            DeliveryOutcome::Delivered { .. }
        ));
        let requests = server.captured();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[1].authorization.as_deref(), Some("Bearer t-old"));
        assert_eq!(requests[3].authorization.as_deref(), Some("Bearer t-new"));
    }

    #[tokio::test]
    async fn only_documented_rate_limit_codes_with_bounded_reset_are_retryable() {
        for (status, code, headers, expected) in [
            (429, 99_991_400, "x-ogw-ratelimit-reset: 2\r\n", Some(2000)),
            (400, 99_991_400, "x-ogw-ratelimit-reset: 1\r\n", Some(1000)),
            (
                400,
                230_020,
                "x-ogw-ratelimit-reset: 3600\r\n",
                Some(3_600_000),
            ),
            (429, 99_991_400, "Retry-After: 2\r\n", None),
            (429, 99_991_400, "x-ogw-ratelimit-reset: 0\r\n", None),
            (429, 99_991_400, "x-ogw-ratelimit-reset: 3601\r\n", None),
            (429, 99_991_400, "x-ogw-ratelimit-reset: 1.5\r\n", None),
            (
                429,
                99_991_400,
                "x-ogw-ratelimit-reset: 1\r\nx-ogw-ratelimit-reset: 2\r\n",
                None,
            ),
            (429, 1, "x-ogw-ratelimit-reset: 2\r\n", None),
            (429, 230_020, "x-ogw-ratelimit-reset: 2\r\n", None),
            (200, 99_991_400, "x-ogw-ratelimit-reset: 2\r\n", None),
        ] {
            let mut body = json!({"code":code});
            if code == 230_020 {
                body["data"] = json!({});
            }
            let server = Server::new(vec![
                token("t-test"),
                Reply::raw(status, headers, &body.to_string()),
            ])
            .await;
            let outcome = server
                .sender()
                .send(&destination(None), DELIVERY, "hello")
                .await;
            if let Some(retry_after_ms) = expected {
                assert_eq!(outcome, DeliveryOutcome::RateLimited { retry_after_ms });
            } else {
                assert!(
                    matches!(outcome, DeliveryOutcome::Unknown { .. }),
                    "{outcome:?}"
                );
            }
            assert_eq!(server.captured().len(), 2);
        }
    }

    #[tokio::test]
    async fn ambiguous_message_errors_and_bad_receipts_are_unknown_without_resend() {
        let mut wrong_chat = receipt(None);
        wrong_chat["data"]["chat_id"] = json!("oc_other");
        let mut wrong_type = receipt(None);
        wrong_type["data"]["msg_type"] = json!("post");
        let mut wrong_id = receipt(None);
        wrong_id["data"]["message_id"] = json!("../om_receipt");
        let mut wrong_thread = receipt(None);
        wrong_thread["data"]["parent_id"] = json!("om_other");
        for failure in [
            Reply::json(&wrong_chat),
            Reply::json(&wrong_type),
            Reply::json(&wrong_id),
            Reply::json(&wrong_thread),
            Reply::raw(200, "", "not-json"),
            Reply::json(&json!({"code":0})),
            Reply::json(&json!({"code":1})),
            Reply::json(&json!({"code":230_049})),
            Reply::raw(400, "", r#"{"code":230049}"#),
            Reply::raw(400, "", r#"{"code":18121}"#),
            Reply::raw(400, "", r#"{"code":18121,"data":{}}"#),
            Reply::raw(400, "", r#"{"code":0,"data":{}}"#),
            Reply::raw(400, "", r#"{"code":987654321,"data":{}}"#),
            Reply::raw(503, "", ""),
            Reply::raw(307, "Location: /elsewhere\r\n", ""),
            Reply::raw(200, "", &"x".repeat(65537)),
            Reply {
                wire: None,
                gate: None,
            },
        ] {
            let server = Server::new(vec![token("t-test"), failure]).await;
            assert!(matches!(
                server
                    .sender()
                    .send(&destination(None), DELIVERY, "hello")
                    .await,
                DeliveryOutcome::Unknown { .. }
            ));
            assert_eq!(server.captured().len(), 2);
        }
        for status in [401, 403, 400] {
            let server = Server::new(vec![
                token("t-test"),
                Reply::raw(status, "", r#"{"code":99991663}"#),
            ])
            .await;
            assert!(matches!(
                server
                    .sender()
                    .send(&destination(None), DELIVERY, "hello")
                    .await,
                DeliveryOutcome::Rejected { .. }
            ));
            assert_eq!(
                server.captured().len(),
                2,
                "token error must never trigger refresh and resend"
            );
        }
    }

    #[tokio::test]
    async fn root_reply_requires_matching_root_or_parent_receipt() {
        for (root, parent, accepted) in [
            (Some("om_root"), None, true),
            (None, Some("om_root"), true),
            (Some("om_root"), Some(""), true),
            (Some("om_root"), Some("om_other"), false),
            (Some("om_other"), Some("om_root"), false),
            (Some("om_other"), Some("om_other"), false),
            (None, None, false),
        ] {
            let mut value = receipt(None);
            value["data"]["root_id"] = json!(root);
            value["data"]["parent_id"] = json!(parent);
            let server = Server::new(vec![token("t-test"), Reply::json(&value)]).await;
            let outcome = server
                .sender()
                .send(&destination(Some("om_root")), DELIVERY, "hello")
                .await;
            assert_eq!(
                matches!(outcome, DeliveryOutcome::Delivered { .. }),
                accepted
            );
        }
    }

    #[tokio::test]
    async fn known_rejections_remain_distinct_from_unknown_business_errors() {
        for status in [200, 400] {
            for code in [
                230_001, 230_002, 230_006, 230_011, 230_013, 230_018, 230_019, 230_022, 230_025,
                230_027, 230_028, 230_035, 230_050, 230_071, 230_072, 230_075, 230_111, 232_009,
                99_991_661, 99_991_662, 99_991_663, 99_991_664, 99_991_665, 99_991_671, 99_991_672,
                99_991_673,
            ] {
                assert_eq!(
                    parse_response(
                        &destination(None),
                        StatusCode::from_u16(status).unwrap(),
                        &HeaderMap::new(),
                        &serde_json::to_vec(&json!({"code":code,"data":{}})).unwrap()
                    ),
                    rejected("feishu_rejected")
                );
            }
        }
    }

    #[tokio::test]
    async fn auth_failure_invalidates_only_for_future_independent_delivery() {
        for (status, code) in [(401, 99_991_663), (400, 99_991_663), (200, 99_991_665)] {
            let server = Server::new(vec![
                token("t-old"),
                Reply::raw(status, "", &json!({"code":code,"data":{}}).to_string()),
                token("t-new"),
                Reply::json(&receipt(None)),
            ])
            .await;
            let sender = server.sender();
            assert!(matches!(
                sender.send(&destination(None), DELIVERY, "first").await,
                DeliveryOutcome::Rejected { .. }
            ));
            assert_eq!(server.captured().len(), 2);
            assert_eq!(
                sender
                    .send(
                        &destination(None),
                        &uuid::Uuid::new_v4().to_string(),
                        "too soon"
                    )
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
                sender
                    .send(
                        &destination(None),
                        &uuid::Uuid::new_v4().to_string(),
                        "future independent event"
                    )
                    .await,
                DeliveryOutcome::Delivered { .. }
            ));
            let requests = server.captured();
            assert_eq!(requests.len(), 4);
            assert_eq!(requests[3].authorization.as_deref(), Some("Bearer t-new"));
            assert_ne!(requests[1].body["uuid"], requests[3].body["uuid"]);
        }
    }

    #[tokio::test]
    async fn late_auth_failure_cannot_discard_a_newer_cached_token() {
        let gate = Arc::new(Semaphore::new(0));
        let server = Server::new(vec![
            token("t-old"),
            Reply::raw(401, "", "").gated(gate.clone()),
            Reply::json(&receipt(None)),
        ])
        .await;
        let sender = Arc::new(server.sender());
        let active = sender.clone();
        let task =
            tokio::spawn(async move { active.send(&destination(None), DELIVERY, "first").await });
        server.wait_requests(2).await;
        sender.token.lock().await.cached = Some(CachedToken {
            value: "t-new".into(),
            usable_until: Instant::now() + Duration::from_secs(60),
        });
        gate.add_permits(1);
        assert!(matches!(
            task.await.unwrap(),
            DeliveryOutcome::Rejected { .. }
        ));
        assert!(matches!(
            sender
                .send(
                    &destination(None),
                    &uuid::Uuid::new_v4().to_string(),
                    "next"
                )
                .await,
            DeliveryOutcome::Delivered { .. }
        ));
        let requests = server.captured();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].authorization.as_deref(), Some("Bearer t-new"));
    }

    #[tokio::test]
    async fn waiting_for_refresh_is_bounded_and_cancellation_keeps_cooldown() {
        let server = Server::new(vec![]).await;
        let sender = Arc::new(server.sender());
        let lock = sender.token.lock().await;
        tokio::time::pause();
        let waiting = sender.clone();
        let task =
            tokio::spawn(async move { waiting.send(&destination(None), DELIVERY, "hello").await });
        tokio::task::yield_now().await;
        tokio::time::advance(TOKEN_BUDGET + Duration::from_millis(1)).await;
        assert_eq!(task.await.unwrap(), rejected("credential_unavailable"));
        drop(lock);
        tokio::time::resume();
        assert!(server.captured().is_empty());

        let gate = Arc::new(Semaphore::new(0));
        let server = Server::new(vec![token("t-test").gated(gate)]).await;
        let sender = Arc::new(server.sender());
        let active = sender.clone();
        let task =
            tokio::spawn(async move { active.send(&destination(None), DELIVERY, "hello").await });
        server.wait_requests(1).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            sender.send(&destination(None), DELIVERY, "hello").await,
            rejected("credential_unavailable")
        );
        assert_eq!(server.captured().len(), 1);
    }

    #[tokio::test]
    async fn token_and_message_timeouts_have_distinct_delivery_certainty() {
        for token_stage in [true, false] {
            let gate = Arc::new(Semaphore::new(0));
            let replies = if token_stage {
                vec![token("t-test").gated(gate)]
            } else {
                vec![token("t-test"), Reply::json(&receipt(None)).gated(gate)]
            };
            let server = Server::new(replies).await;
            let sender = server.sender();
            let task =
                tokio::spawn(
                    async move { sender.send(&destination(None), DELIVERY, "hello").await },
                );
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
    }
    fn verified_bot() -> Reply {
        Reply::json(&json!({"code":0,"bot":{"activate_status":2,"open_id":"ou_testbot"}}))
    }
    fn verified_tenant() -> Reply {
        Reply::json(&json!({"code":0,"data":{"tenant":{"tenant_key":"tenant-test"}}}))
    }
    fn verified_chat() -> Reply {
        Reply::json(&json!({"code":0,"data":{"chat_mode":"p2p"}}))
    }

    #[tokio::test]
    async fn startup_verifies_real_minimal_official_bot_tenant_and_p2p_shapes_with_empty_get_bodies(
    ) {
        let server = Server::new(vec![
            token("t-verify"),
            verified_bot(),
            verified_tenant(),
            verified_chat(),
        ])
        .await;
        server
            .sender()
            .verify_installation("ou_testbot", "tenant-test", "oc_testchat")
            .await
            .unwrap();
        let requests = server.captured();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[1].path, "/open-apis/bot/v3/info");
        assert_eq!(requests[2].path, "/open-apis/tenant/v2/tenant/query");
        assert_eq!(requests[3].path, "/open-apis/im/v1/chats/oc_testchat");
        for request in &requests[1..] {
            assert_eq!(request.body, Value::Null);
            assert_eq!(request.authorization.as_deref(), Some("Bearer t-verify"));
        }
        // The official bot is top-level, enabled=2; tenant has no invented app
        // identity and p2p chat omits group owner/member fields.
        assert!(requests
            .iter()
            .all(|r| !r.path.contains("members") && !r.path.contains("batch_get")));
    }

    #[tokio::test]
    async fn startup_wrong_nonobject_or_duplicate_identity_stops_before_later_identity_reads() {
        let malformed: [(usize, &str); 14] = [
            (
                1,
                r#"{"code":0,"bot":{"activate_status":1,"open_id":"ou_testbot"}}"#,
            ),
            (
                1,
                r#"{"code":0,"bot":{"activate_status":2,"open_id":"ou_other"}}"#,
            ),
            (
                1,
                r#"{"code":0,"data":{"bot":{"activate_status":2,"open_id":"ou_testbot"}}}"#,
            ),
            (1, r#"[0,{"activate_status":2,"open_id":"ou_testbot"}]"#),
            (1, r#"{"code":0,"bot":[2,"ou_testbot"]}"#),
            (
                1,
                r#"{"code":0,"bot":{"activate_status":2,"open_id":"ou_other","open_id":"ou_testbot"}}"#,
            ),
            (
                1,
                r#"{"code":1,"code":0,"bot":{"activate_status":2,"open_id":"ou_testbot"}}"#,
            ),
            (
                2,
                r#"{"code":0,"data":{"tenant":{"tenant_key":"other-tenant"}}}"#,
            ),
            (2, r#"{"code":0,"data":{"tenant":["tenant-test"]}}"#),
            (
                2,
                r#"{"code":0,"data":{"tenant":{"tenant_key":"other-tenant","tenant_key":"tenant-test"}}}"#,
            ),
            (3, r#"{"code":0,"data":{"chat_mode":"group"}}"#),
            (
                3,
                r#"{"code":0,"data":{"chat_mode":"p2p","external":true}}"#,
            ),
            (3, r#"{"code":0,"data":["p2p",null,null]}"#),
            (
                3,
                r#"{"code":0,"data":{"chat_mode":"group","chat_mode":"p2p"}}"#,
            ),
        ];
        for (index, body) in malformed {
            let mut replies = vec![
                token("t-verify"),
                verified_bot(),
                verified_tenant(),
                verified_chat(),
            ];
            replies[index] = Reply::raw(200, "", body);
            let server = Server::new(replies).await;
            let error = server
                .sender()
                .verify_installation("ou_testbot", "tenant-test", "oc_testchat")
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "Feishu installation verification failed");
            assert_eq!(server.captured().len(), index + 1, "{body}");
        }
    }

    #[tokio::test]
    async fn raw_token_duplicates_arrays_and_double_mime_never_admit_a_message_post() {
        for bad in [
            Reply::raw(200, "", r#"[0,7200,"t-unsafe"]"#),
            Reply::raw(
                200,
                "",
                r#"{"code":1,"code":0,"expire":7200,"tenant_access_token":"t-unsafe"}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"expire":7200,"expire":7200,"tenant_access_token":"t-unsafe"}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"expire":7200,"tenant_access_token":"t-first","tenant_access_token":"t-last"}"#,
            ),
            Reply::raw(
                200,
                "Content-Type: application/json\r\n",
                r#"{"code":0,"expire":7200,"tenant_access_token":"t-unsafe"}"#,
            ),
        ] {
            let server = Server::new(vec![bad]).await;
            let sender = server.sender();
            assert_eq!(
                sender.send(&destination(None), DELIVERY, "hello").await,
                rejected("credential_unavailable")
            );
            assert_eq!(
                sender.send(&destination(None), DELIVERY, "again").await,
                rejected("credential_unavailable")
            );
            assert_eq!(server.captured().len(), 1);
        }
    }

    #[tokio::test]
    async fn raw_receipt_objects_and_critical_duplicates_are_unknown_and_never_resubmitted() {
        for bad in [
            Reply::raw(
                200,
                "",
                r#"[0,{"message_id":"om_receipt","chat_id":"oc_testchat","msg_type":"text"}]"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"data":["om_receipt","oc_testchat","text",null,null]}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":1,"code":0,"data":{"message_id":"om_receipt","chat_id":"oc_testchat","msg_type":"text"}}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"data":{"message_id":"om_other","message_id":"om_receipt","chat_id":"oc_testchat","msg_type":"text"}}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"data":{"message_id":"om_receipt","chat_id":"oc_other","chat_id":"oc_testchat","msg_type":"text"}}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"data":{"message_id":"om_receipt","chat_id":"oc_testchat","msg_type":"image","msg_type":"text"}}"#,
            ),
            Reply::raw(
                200,
                "",
                r#"{"code":0,"data":{"message_id":"om_receipt","chat_id":"oc_testchat","msg_type":"text","root_id":"om_wrong","root_id":""}}"#,
            ),
            Reply::raw(
                200,
                "Content-Type: application/json\r\n",
                r#"{"code":0,"data":{"message_id":"om_receipt","chat_id":"oc_testchat","msg_type":"text"}}"#,
            ),
        ] {
            let server = Server::new(vec![token("t-test"), bad]).await;
            assert!(matches!(
                server
                    .sender()
                    .send(&destination(None), DELIVERY, "hello")
                    .await,
                DeliveryOutcome::Unknown { .. }
            ));
            assert_eq!(server.captured().len(), 2);
        }
    }
}
