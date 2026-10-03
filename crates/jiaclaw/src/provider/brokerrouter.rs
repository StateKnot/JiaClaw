// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bounded, single-attempt Brokerrouter Chat Completions transport.
//! Native tool messages retain provider IDs and arguments across the complete roundtrip.

use jiaclaw_core::{ChatMessage, ChatResponse, JiaClawError, MessageRole, RunStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use stateknot_integrations::ProviderEndpoint;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_ID_HEADER: &str = "x-brokerrouter-request-id";

pub(crate) fn failure(message: &str) -> JiaClawError {
    JiaClawError::StateKnotIntegration(format!("Brokerrouter: {message}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct NativeFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct NativeToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: NativeFunction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WireMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<NativeToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl WireMessage {
    pub fn text(role: &str, content: String) -> Self {
        Self {
            role: role.into(),
            content: Some(content),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    pub fn history(system_prompt: &str, messages: &[ChatMessage]) -> Vec<Self> {
        let mut wire = vec![Self::text("system", system_prompt.into())];
        wire.extend(messages.iter().map(|message| {
            Self::text(
                match message.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "system",
                },
                message.content.clone(),
            )
        }));
        wire
    }
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: &'a [WireMessage],
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [Value]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
}

#[derive(Deserialize)]
struct CompletionResponse {
    #[serde(default)]
    model: Option<String>,
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    index: Option<u64>,
    message: WireMessage,
    finish_reason: String,
}

/// Immutable exact request bytes and identity, prepared before durable admission.
/// No Debug implementation: body/model may contain private user information.
pub(crate) struct PreparedCompletion {
    body: Vec<u8>,
    request_hash: String,
    model: String,
    has_tools: bool,
}
impl PreparedCompletion {
    #[cfg(test)]
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }
    pub(crate) fn request_hash(&self) -> &str {
        &self.request_hash
    }
    pub(crate) fn model(&self) -> &str {
        &self.model
    }
    pub(crate) fn has_tools(&self) -> bool {
        self.has_tools
    }
}

/// Headers are available before reading the body. The coordinator must persist
/// a known remote ID before calling finish; all failures retain its local hold.
pub(crate) struct PendingCompletion {
    response: reqwest::Response,
    remote_id: Option<String>,
    deadline: Instant,
}
impl PendingCompletion {
    pub(crate) fn remote_id(&self) -> Option<&str> {
        self.remote_id.as_deref()
    }
    #[cfg(test)]
    pub(crate) fn status(&self) -> u16 {
        self.response.status().as_u16()
    }
}

/// A validated model response, not evidence of tool execution or session commit.
pub(crate) struct CompletionReceipt {
    pub request_id: String,
    pub message: WireMessage,
    pub body: Vec<u8>,
}
pub(crate) struct RequestStatus {
    pub state: String,
}
#[derive(Deserialize)]
struct StatusResponse {
    id: String,
    model: String,
    purpose: String,
    status: String,
}

fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn response_request_id(
    headers: &reqwest::header::HeaderMap,
) -> Result<Option<String>, JiaClawError> {
    let mut values = headers.get_all(REQUEST_ID_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(failure("ambiguous gateway request identity"));
    }
    let value = value
        .to_str()
        .map_err(|_| failure("invalid gateway request identity"))?;
    if !canonical_uuid(value) {
        return Err(failure("invalid gateway request identity"));
    }
    Ok(Some(value.to_owned()))
}
fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 200
        && model.trim() == model
        && !model.chars().any(char::is_control)
}
fn client(timeout: Duration) -> Result<reqwest::Client, JiaClawError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .map_err(|_| failure("HTTP client initialization failed"))
}
fn json_success(response: &reqwest::Response) -> Result<(), JiaClawError> {
    if response.status() != reqwest::StatusCode::OK {
        return Err(failure(&format!(
            "HTTP status {}; request not retried",
            response.status().as_u16()
        )));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next());
    if !mime.is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json")) {
        return Err(failure("expected JSON response content type"));
    }
    Ok(())
}
async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, JiaClawError> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES as u64)
    {
        return Err(failure("response exceeds 2 MiB"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("response interrupted or timed out; not retried"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(failure("response exceeds 2 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn completion_message(
    bytes: &[u8],
    model: Option<&str>,
    has_tools: bool,
) -> Result<WireMessage, JiaClawError> {
    let mut response: CompletionResponse =
        serde_json::from_slice(bytes).map_err(|_| failure("invalid completion response"))?;
    if model.is_some_and(|model| response.model.as_deref() != Some(model)) {
        return Err(failure("completion model does not match prepared request"));
    }
    if response.choices.len() != 1 {
        return Err(failure("expected exactly one completion choice"));
    }
    let choice = response.choices.remove(0);
    if model.is_some() && choice.index.is_some_and(|index| index != 0) {
        return Err(failure("invalid completion choice index"));
    }
    let message = choice.message;
    if message.role != "assistant" || message.tool_call_id.is_some() {
        return Err(failure("invalid assistant message"));
    }
    match choice.finish_reason.as_str() {
        "stop" if message.tool_calls.is_empty() && message.content.is_some() => Ok(message),
        "tool_calls" if has_tools && !message.tool_calls.is_empty() => Ok(message),
        _ => Err(failure(
            "incomplete or inconsistent completion finish reason; no tools dispatched",
        )),
    }
}
fn completion_receipt(
    bytes: Vec<u8>,
    request_id: String,
    model: &str,
    has_tools: bool,
) -> Result<CompletionReceipt, JiaClawError> {
    let message = completion_message(&bytes, Some(model), has_tools)?;
    Ok(CompletionReceipt {
        request_id,
        message,
        body: bytes,
    })
}

/// Brokerrouter gateway client. Requests have bounded time/size and are never replayed.
#[allow(clippy::module_name_repetitions)]
pub struct BrokerrouterProvider {
    base_url: String,
    virtual_key: String,
}

impl BrokerrouterProvider {
    /// Construct a client. Endpoint/key validation occurs before any network access.
    pub fn new(base_url: &str, virtual_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').into(),
            virtual_key: virtual_key.into(),
        }
    }

    fn generate_idempotency_key() -> String {
        format!("jiaclaw-{}", Uuid::new_v4())
    }

    fn endpoint(&self) -> Result<String, JiaClawError> {
        let valid = if self.base_url.starts_with("https://") {
            ProviderEndpoint::https(&self.base_url)
        } else {
            ProviderEndpoint::loopback_http(&self.base_url)
        };
        if self.base_url.len() > 2048 || valid.is_err() {
            return Err(JiaClawError::Configuration("Brokerrouter endpoint requires HTTPS or literal-loopback HTTP without credentials/query/fragment".into()));
        }
        if self.virtual_key.trim().is_empty()
            || self.virtual_key.len() > 4096
            || self.virtual_key.bytes().any(|b| !b.is_ascii_graphic())
        {
            return Err(JiaClawError::Configuration(
                "invalid Brokerrouter API key".into(),
            ));
        }
        Ok(format!("{}/v1/chat/completions", self.base_url))
    }

    /// Validate endpoint and credential syntax without touching the network.
    pub(crate) fn validate(&self) -> Result<(), JiaClawError> {
        self.endpoint().map(|_| ())
    }

    /// Serialize once before ledger admission. This performs no network IO.
    pub(crate) fn prepare(
        &self,
        model: &str,
        messages: &[WireMessage],
        temperature: f32,
        max_tokens: u32,
        tools: &[Value],
    ) -> Result<PreparedCompletion, JiaClawError> {
        self.endpoint()?;
        if !valid_model(model)
            || messages.is_empty()
            || messages.len() > 1024
            || tools.len() > 128
            || !(1..=1_000_000).contains(&max_tokens)
            || !temperature.is_finite()
            || !(0.0..=2.0).contains(&temperature)
        {
            return Err(failure("invalid tracked request limits"));
        }
        let request = CompletionRequest {
            model,
            messages,
            temperature,
            max_tokens,
            stream: false,
            tools: (!tools.is_empty()).then_some(tools),
            parallel_tool_calls: (!tools.is_empty()).then_some(false),
        };
        let body =
            serde_json::to_vec(&request).map_err(|_| failure("request serialization failed"))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(failure("request exceeds 2 MiB"));
        }
        let request_hash = format!("{:x}", Sha256::digest(&body));
        Ok(PreparedCompletion {
            body,
            request_hash,
            model: model.into(),
            has_tools: !tools.is_empty(),
        })
    }

    /// One POST under an already persisted operation UUID; returns at headers.
    pub(crate) async fn post(
        &self,
        operation_id: &str,
        prepared: &PreparedCompletion,
    ) -> Result<PendingCompletion, JiaClawError> {
        self.post_with_timeout(operation_id, prepared, REQUEST_TIMEOUT)
            .await
    }
    async fn post_with_timeout(
        &self,
        operation_id: &str,
        prepared: &PreparedCompletion,
        timeout: Duration,
    ) -> Result<PendingCompletion, JiaClawError> {
        if !canonical_uuid(operation_id) {
            return Err(failure("invalid local operation identity"));
        }
        let url = self.endpoint()?;
        let client = client(timeout)?;
        let deadline = Instant::now() + timeout;
        let response = tokio::time::timeout_at(
            deadline,
            client
                .post(url)
                .bearer_auth(&self.virtual_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .header("Idempotency-Key", operation_id)
                .body(prepared.body.clone())
                .send(),
        )
        .await
        .map_err(|_| failure("request timed out; outcome may be unknown, not retried"))?
        .map_err(|_| failure("request failed or timed out; outcome may be unknown, not retried"))?;
        // Even an HTTP error can carry a known durable remote identity. Do not
        // reject its status or read its body before the coordinator saves it.
        let remote_id = response_request_id(response.headers()).ok().flatten();
        Ok(PendingCompletion {
            response,
            remote_id,
            deadline,
        })
    }

    /// Called only after the coordinator has durably saved the response identity.
    /// Persistence time consumes the original request's deadline, never a reset.
    pub(crate) async fn finish(
        &self,
        pending: PendingCompletion,
        prepared: &PreparedCompletion,
    ) -> Result<CompletionReceipt, JiaClawError> {
        let request_id = pending
            .remote_id
            .ok_or_else(|| failure("gateway request identity missing or invalid"))?;
        json_success(&pending.response)?;
        let bytes = tokio::time::timeout_at(pending.deadline, bounded_body(pending.response))
            .await
            .map_err(|_| failure("response deadline expired; not retried"))??;
        completion_receipt(bytes, request_id, prepared.model(), prepared.has_tools())
    }

    async fn get(
        &self,
        request_id: &str,
        model: &str,
        result: bool,
    ) -> Result<(reqwest::Response, Instant), JiaClawError> {
        self.endpoint()?;
        if !canonical_uuid(request_id) || !valid_model(model) {
            return Err(failure("invalid recovery identity or model"));
        }
        let url = format!(
            "{}/v1/requests/{request_id}{}",
            self.base_url,
            if result { "/result" } else { "" }
        );
        let client = client(REQUEST_TIMEOUT)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let response = tokio::time::timeout_at(
            deadline,
            client
                .get(url)
                .bearer_auth(&self.virtual_key)
                .header(reqwest::header::ACCEPT, "application/json")
                .send(),
        )
        .await
        .map_err(|_| failure("recovery timed out; not retried"))?
        .map_err(|_| failure("recovery failed or timed out; not retried"))?;
        json_success(&response)?;
        if response_request_id(response.headers())?
            .as_deref()
            .is_some_and(|id| id != request_id)
        {
            return Err(failure("recovery response identity mismatch"));
        }
        Ok((response, deadline))
    }
    /// Metadata lookup only; no POST replay or billable policy check.
    pub(crate) async fn get_status(
        &self,
        request_id: &str,
        model: &str,
    ) -> Result<RequestStatus, JiaClawError> {
        let (response, deadline) = self.get(request_id, model, false).await?;
        let bytes = tokio::time::timeout_at(deadline, bounded_body(response))
            .await
            .map_err(|_| failure("recovery deadline expired; not retried"))??;
        let response: StatusResponse = serde_json::from_slice(&bytes)
            .map_err(|_| failure("invalid request status response"))?;
        if response.id != request_id
            || response.model != model
            || response.purpose != "model"
            || !matches!(
                response.status.as_str(),
                "reserved"
                    | "submitting"
                    | "succeeded"
                    | "failed"
                    | "submission_unknown"
                    | "reconciled"
            )
        {
            return Err(failure("request status contract mismatch"));
        }
        Ok(RequestStatus {
            state: response.status,
        })
    }
    /// Read a retained model result. Returning it never executes tools or resumes a turn.
    pub(crate) async fn get_result(
        &self,
        request_id: &str,
        model: &str,
        has_tools: bool,
    ) -> Result<CompletionReceipt, JiaClawError> {
        let (response, deadline) = self.get(request_id, model, true).await?;
        let bytes = tokio::time::timeout_at(deadline, bounded_body(response))
            .await
            .map_err(|_| failure("recovery deadline expired; not retried"))??;
        completion_receipt(bytes, request_id.into(), model, has_tools)
    }

    /// Submit one model request; cancellation drops the async connection future.
    pub(crate) async fn complete(
        &self,
        model: &str,
        messages: &[WireMessage],
        temperature: f32,
        max_tokens: u32,
        tools: &[Value],
    ) -> Result<WireMessage, JiaClawError> {
        self.complete_with_timeout(
            model,
            messages,
            temperature,
            max_tokens,
            tools,
            REQUEST_TIMEOUT,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_with_timeout(
        &self,
        model: &str,
        messages: &[WireMessage],
        temperature: f32,
        max_tokens: u32,
        tools: &[Value],
        timeout: Duration,
    ) -> Result<WireMessage, JiaClawError> {
        let url = self.endpoint()?;
        if messages.is_empty()
            || messages.len() > 1024
            || tools.len() > 128
            || max_tokens == 0
            || !temperature.is_finite()
        {
            return Err(failure("invalid request limits"));
        }
        let request = CompletionRequest {
            model,
            messages,
            temperature,
            max_tokens,
            stream: false,
            tools: (!tools.is_empty()).then_some(tools),
            parallel_tool_calls: (!tools.is_empty()).then_some(false),
        };
        let body =
            serde_json::to_vec(&request).map_err(|_| failure("request serialization failed"))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(failure("request exceeds 2 MiB"));
        }
        let client = client(timeout)?;
        // No retries: a transport failure may occur after the gateway accepted/billed a request.
        let response = client
            .post(url)
            .bearer_auth(&self.virtual_key)
            .header("Content-Type", "application/json")
            .header("Idempotency-Key", Self::generate_idempotency_key())
            .body(body)
            .send()
            .await
            .map_err(|_| {
                failure("request failed or timed out; outcome may be unknown, not retried")
            })?;
        if response.status() != reqwest::StatusCode::OK {
            // Never reflect remote error bodies, URLs, or credentials to clients/logs.
            return Err(failure(&format!(
                "HTTP status {}; request not retried",
                response.status().as_u16()
            )));
        }
        let bytes = bounded_body(response).await?;
        completion_message(&bytes, None, !tools.is_empty())
    }

    /// Text-only model completion (for example, context summarization).
    pub async fn chat(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[ChatMessage],
        temperature: f32,
        max_tokens: u32,
    ) -> Result<ChatResponse, JiaClawError> {
        let message = self
            .complete(
                model,
                &WireMessage::history(system_prompt, messages),
                temperature,
                max_tokens,
                &[],
            )
            .await?;
        Ok(ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: message.content.unwrap_or_default(),
            },
            tool_calls: vec![],
            status: RunStatus::Completed,
            session_id: None,
            routing: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REMOTE_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const OTHER_ID: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const OPERATION_ID: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn prepared(provider: &BrokerrouterProvider, tools: bool) -> PreparedCompletion {
        let tools = if tools {
            vec![serde_json::json!({"type":"function","function":{"name":"clock"}})]
        } else {
            vec![]
        };
        provider
            .prepare(
                "fixture",
                &[WireMessage::text("user", "private prompt".into())],
                0.7,
                100,
                &tools,
            )
            .unwrap()
    }
    fn tracked_body() -> String {
        serde_json::json!({"id":"chatcmpl-provider-id","object":"chat.completion","model":"fixture",
            "choices":[{"index":0,"message":{"role":"assistant","content":"retained reply"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}).to_string()
    }
    struct Captured {
        headers: String,
        body: Vec<u8>,
    }
    async fn tracked_fixture(
        status: &str,
        headers: &str,
        body: String,
        gate: Option<std::sync::Arc<tokio::sync::Semaphore>>,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<Captured>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let (send, captured) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut chunk = [0; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (end + 4, length);
                }
                assert!(bytes.len() < 8192);
            };
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            }
            let _ = send.send(Captured {
                headers: String::from_utf8(bytes[..header_end].to_vec()).unwrap(),
                body: bytes[header_end..header_end + length].to_vec(),
            });
            socket.write_all(response.as_bytes()).await.unwrap();
            if let Some(gate) = gate {
                let permit = gate.acquire().await.unwrap();
                permit.forget();
            }
            let _ = socket.write_all(body.as_bytes()).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err(),
                "transport must not replay"
            );
        });
        (url, captured, task)
    }

    #[test]
    fn preparation_is_exact_bounded_and_rejects_invalid_request_policy() {
        let provider = BrokerrouterProvider::new("http://127.0.0.1:1", "fixture-key");
        let request = prepared(&provider, true);
        assert_eq!(
            request.request_hash(),
            format!("{:x}", Sha256::digest(request.body()))
        );
        assert_eq!(request.model(), "fixture");
        assert!(request.has_tools());
        let json: Value = serde_json::from_slice(request.body()).unwrap();
        assert_eq!(json["stream"], false);
        assert_eq!(json["parallel_tool_calls"], false);
        let messages = [WireMessage::text("user", "hello".into())];
        for model in ["", " fixture", "fixture\n", &"x".repeat(201)] {
            assert!(provider.prepare(model, &messages, 0.7, 100, &[]).is_err());
        }
        for temperature in [f32::NAN, f32::INFINITY, -0.01, 2.01] {
            assert!(provider
                .prepare("fixture", &messages, temperature, 100, &[])
                .is_err());
        }
        for tokens in [0, 1_000_001] {
            assert!(provider
                .prepare("fixture", &messages, 0.7, tokens, &[])
                .is_err());
        }
        assert!(provider
            .prepare(
                "fixture",
                &[WireMessage::text("user", "x".repeat(MAX_BODY_BYTES))],
                0.7,
                100,
                &[]
            )
            .is_err());
    }

    #[tokio::test]
    async fn tracked_headers_precede_body_and_exact_durable_identity_is_sent_once() {
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let body = tracked_body();
        let (url, captured, task) = tracked_fixture(
            "200 OK",
            &format!("Content-Type: application/json\r\n{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n"),
            body.clone(),
            Some(gate.clone()),
        )
        .await;
        let provider = BrokerrouterProvider::new(&url, "fixture-key");
        let request = prepared(&provider, false);
        let pending = tokio::time::timeout(
            Duration::from_secs(1),
            provider.post(OPERATION_ID, &request),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(pending.remote_id(), Some(REMOTE_ID));
        assert_eq!(pending.status(), 200);
        let captured = captured.await.unwrap();
        assert!(captured
            .headers
            .starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(captured
            .headers
            .to_ascii_lowercase()
            .contains(&format!("idempotency-key: {OPERATION_ID}\r\n")));
        assert_eq!(captured.body, request.body());
        // This barrier models durable saving of REMOTE_ID before body consumption.
        gate.add_permits(1);
        let receipt = provider.finish(pending, &request).await.unwrap();
        assert_eq!(receipt.request_id, REMOTE_ID);
        assert_eq!(receipt.body, body.as_bytes());
        assert_eq!(receipt.message.content.as_deref(), Some("retained reply"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn tracked_post_preserves_error_identity_but_never_accepts_ambiguous_headers() {
        for (status, extra, expected) in [
            (
                "502 Bad Gateway",
                format!("{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n"),
                Some(REMOTE_ID),
            ),
            ("200 OK", String::new(), None),
            (
                "200 OK",
                format!("{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n"),
                None,
            ),
            (
                "200 OK",
                format!("{REQUEST_ID_HEADER}: {}\r\n", REMOTE_ID.to_uppercase()),
                None,
            ),
            (
                "200 OK",
                format!("{REQUEST_ID_HEADER}: secret-invalid-uuid\r\n"),
                None,
            ),
        ] {
            let (url, _, task) = tracked_fixture(
                status,
                &format!("Content-Type: application/json\r\n{extra}"),
                tracked_body(),
                None,
            )
            .await;
            let provider = BrokerrouterProvider::new(&url, "fixture-key");
            let request = prepared(&provider, false);
            let pending = provider.post(OPERATION_ID, &request).await.unwrap();
            assert_eq!(pending.remote_id(), expected);
            let error = provider
                .finish(pending, &request)
                .await
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains("secret-invalid-uuid"));
            assert!(!error.contains("retained reply"));
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn tracked_completion_rejects_mime_model_finish_and_both_body_size_paths() {
        let mut wrong_model: Value = serde_json::from_str(&tracked_body()).unwrap();
        wrong_model["model"] = serde_json::json!("other");
        let mut unfinished: Value = serde_json::from_str(&tracked_body()).unwrap();
        unfinished["choices"][0]["finish_reason"] = serde_json::json!("length");
        for (mime, body) in [
            ("text/plain", tracked_body()),
            ("application/json", wrong_model.to_string()),
            ("application/json", unfinished.to_string()),
            ("application/json", "x".repeat(MAX_BODY_BYTES + 1)),
        ] {
            let (url, _, task) = tracked_fixture(
                "200 OK",
                &format!("Content-Type: {mime}\r\n{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n"),
                body,
                None,
            )
            .await;
            let provider = BrokerrouterProvider::new(&url, "fixture-key");
            let request = prepared(&provider, false);
            let pending = provider.post(OPERATION_ID, &request).await.unwrap();
            assert_eq!(pending.remote_id(), Some(REMOTE_ID));
            assert!(provider.finish(pending, &request).await.is_err());
            task.await.unwrap();
        }
        // No Content-Length: the incremental body limit must also fail closed.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider = BrokerrouterProvider::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture-key",
        );
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let _ = stream.read(&mut request).await.unwrap();
            let headers=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{REQUEST_ID_HEADER}: {REMOTE_ID}\r\nConnection: close\r\n\r\n");
            stream.write_all(headers.as_bytes()).await.unwrap();
            let _ = stream.write_all(&vec![b'x'; MAX_BODY_BYTES + 1]).await;
        });
        let request = prepared(&provider, false);
        let pending = provider.post(OPERATION_ID, &request).await.unwrap();
        assert!(provider
            .finish(pending, &request)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("exceeds 2 MiB"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn saving_header_cannot_reset_the_original_completion_deadline() {
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let (url, _, task) = tracked_fixture(
            "200 OK",
            &format!("Content-Type: application/json\r\n{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n"),
            tracked_body(),
            Some(gate.clone()),
        )
        .await;
        let provider = BrokerrouterProvider::new(&url, "fixture-key");
        let request = prepared(&provider, false);
        let pending = provider
            .post_with_timeout(OPERATION_ID, &request, Duration::from_millis(100))
            .await
            .unwrap();
        assert_eq!(pending.remote_id(), Some(REMOTE_ID));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(provider.finish(pending, &request).await.is_err());
        gate.add_permits(1);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn recovery_is_get_only_and_checks_status_result_and_optional_identity_headers() {
        let status=serde_json::json!({"id":REMOTE_ID,"model":"fixture","purpose":"model","status":"succeeded","cost_cny":"0.001","attempts":[]}).to_string();
        for result in [false, true] {
            let (url, captured, task) = tracked_fixture(
                "200 OK",
                "Content-Type: application/json; charset=utf-8\r\n",
                if result {
                    tracked_body()
                } else {
                    status.clone()
                },
                None,
            )
            .await;
            let provider = BrokerrouterProvider::new(&url, "fixture-key");
            if result {
                let receipt = provider
                    .get_result(REMOTE_ID, "fixture", false)
                    .await
                    .unwrap();
                assert_eq!(receipt.request_id, REMOTE_ID);
            } else {
                let status = provider.get_status(REMOTE_ID, "fixture").await.unwrap();
                assert_eq!(status.state, "succeeded");
            }
            let captured = captured.await.unwrap();
            assert!(captured.headers.starts_with(&format!(
                "GET /v1/requests/{REMOTE_ID}{} HTTP/1.1\r\n",
                if result { "/result" } else { "" }
            )));
            assert!(captured.body.is_empty());
            assert!(!captured
                .headers
                .to_ascii_lowercase()
                .contains("idempotency-key"));
            task.await.unwrap();
        }
        for extra in [
            format!("{REQUEST_ID_HEADER}: {OTHER_ID}\r\n"),
            format!("{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n{REQUEST_ID_HEADER}: {REMOTE_ID}\r\n"),
        ] {
            let (url, _, task) = tracked_fixture(
                "200 OK",
                &format!("Content-Type: application/json\r\n{extra}"),
                tracked_body(),
                None,
            )
            .await;
            assert!(BrokerrouterProvider::new(&url, "fixture-key")
                .get_result(REMOTE_ID, "fixture", false)
                .await
                .is_err());
            task.await.unwrap();
        }
        for (key, value) in [
            ("id", OTHER_ID),
            ("model", "other"),
            ("purpose", "guardrail"),
            ("status", "invented-state"),
        ] {
            let mut invalid: Value = serde_json::from_str(&status).unwrap();
            invalid[key] = serde_json::json!(value);
            let (url, _, task) = tracked_fixture(
                "200 OK",
                "Content-Type: application/json\r\n",
                invalid.to_string(),
                None,
            )
            .await;
            assert!(BrokerrouterProvider::new(&url, "fixture-key")
                .get_status(REMOTE_ID, "fixture")
                .await
                .is_err());
            task.await.unwrap();
        }
        let provider = BrokerrouterProvider::new("http://127.0.0.1:1", "fixture-key");
        assert!(provider
            .post("invalid-operation", &prepared(&provider, false))
            .await
            .is_err());
        assert!(provider.get_status("../escape", "fixture").await.is_err());
        assert!(provider
            .get_result(REMOTE_ID, " fixture", false)
            .await
            .is_err());
    }

    #[test]
    fn endpoint_policy_and_key_are_checked_without_echoing_secrets() {
        for url in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com?secret=value",
            "https://example.com/#secret",
        ] {
            let error = BrokerrouterProvider::new(url, "key")
                .endpoint()
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret"));
        }
        assert!(BrokerrouterProvider::new("http://127.0.0.1:1234/", "key")
            .endpoint()
            .is_ok());
        assert!(BrokerrouterProvider::new("https://example.com", "\nsecret")
            .endpoint()
            .is_err());
        assert_ne!(
            BrokerrouterProvider::generate_idempotency_key(),
            BrokerrouterProvider::generate_idempotency_key()
        );
    }

    async fn fixture(
        status: &str,
        headers: &str,
        body: &str,
        delay: Duration,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0; 8192];
            let _ = socket.read(&mut data).await.unwrap();
            tokio::time::sleep(delay).await;
            let _ = socket.write_all(response.as_bytes()).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err(),
                "request replayed"
            );
        });
        (url, task)
    }

    #[tokio::test]
    async fn native_null_content_and_finish_reason_are_preserved() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"clock","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let (url, task) = fixture("200 OK", "", body, Duration::ZERO).await;
        let result = BrokerrouterProvider::new(&url, "key")
            .complete(
                "fixture",
                &[WireMessage::text("user", "hello".into())],
                0.7,
                100,
                &[serde_json::json!({})],
            )
            .await
            .unwrap();
        assert_eq!(result.tool_calls[0].id, "call_1");
        assert!(result.content.is_none());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn errors_redirects_truncation_and_timeouts_fail_without_replay_or_body_leak() {
        for (status, headers, body, delay, expected) in [
            ("401 Unauthorized", "", "secret-key", Duration::ZERO, "401"),
            (
                "302 Found",
                "Location: http://127.0.0.1:1/secret\r\n",
                "secret-key",
                Duration::ZERO,
                "302",
            ),
            (
                "200 OK",
                "",
                r#"{"choices":[{"message":{"role":"assistant","content":"secret-key"},"finish_reason":"length"}]}"#,
                Duration::ZERO,
                "finish reason",
            ),
            (
                "200 OK",
                "",
                "secret-key",
                Duration::from_millis(500),
                "timed out",
            ),
        ] {
            let (url, task) = fixture(status, headers, body, delay).await;
            let error = BrokerrouterProvider::new(&url, "key")
                .complete_with_timeout(
                    "fixture",
                    &[WireMessage::text("user", "hello".into())],
                    0.7,
                    100,
                    &[],
                    if expected == "timed out" {
                        Duration::from_millis(200)
                    } else {
                        Duration::from_secs(5)
                    },
                )
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("secret-key"));
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn response_body_limit_is_enforced() {
        let body = "x".repeat(MAX_BODY_BYTES + 1);
        let (url, task) = fixture("200 OK", "", &body, Duration::ZERO).await;
        let error = BrokerrouterProvider::new(&url, "key")
            .chat("fixture", "system", &[], 0.7, 100)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("exceeds 2 MiB"));
        task.await.unwrap();
    }
}
