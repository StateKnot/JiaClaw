// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bounded, single-attempt Brokerrouter Chat Completions transport.
//! Native tool messages retain provider IDs and arguments across the complete roundtrip.

use jiaclaw_core::{ChatMessage, ChatResponse, JiaClawError, MessageRole, RunStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stateknot_integrations::ProviderEndpoint;
use std::time::Duration;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: WireMessage,
    finish_reason: String,
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
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .build()
            .map_err(|_| failure("HTTP client initialization failed"))?;
        // No retries: a transport failure may occur after the gateway accepted/billed a request.
        let mut response = client
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
        let mut response: CompletionResponse =
            serde_json::from_slice(&bytes).map_err(|_| failure("invalid completion response"))?;
        if response.choices.len() != 1 {
            return Err(failure("expected exactly one completion choice"));
        }
        let choice = response.choices.remove(0);
        let message = choice.message;
        if message.role != "assistant" || message.tool_call_id.is_some() {
            return Err(failure("invalid assistant message"));
        }
        match choice.finish_reason.as_str() {
            "stop" if message.tool_calls.is_empty() && message.content.is_some() => Ok(message),
            "tool_calls" if !tools.is_empty() && !message.tool_calls.is_empty() => Ok(message),
            _ => Err(failure(
                "incomplete or inconsistent completion finish reason; no tools dispatched",
            )),
        }
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
