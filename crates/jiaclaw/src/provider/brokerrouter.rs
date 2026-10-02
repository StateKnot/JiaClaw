// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Brokerrouter 提供商（推荐生产路径）
//!
//! 通过 [Brokerrouter](https://github.com/StateKnot/Brokerrouter) AI Gateway
//! 路由模型调用到上游提供商（OpenAI, Anthropic, Ollama 等）。
//!
//! ## 特性
//!
//! - ✅ Bearer 虚拟密钥认证
//! - ✅ 自动幂等性密钥生成（每次请求）
//! - ✅ 非流式聊天补全（`stream: false`）
//! - ✅ 请求追踪（捕获 `x-request-id` / `x-brokerrouter-request-id`）
//! - ✅ 健壮的错误处理和映射
//!
//! ## API 契约
//!
//! ```http
//! POST {base_url}/v1/chat/completions
//! Authorization: Bearer <virtual-key>
//! Idempotency-Key: <1-200 printable ASCII>
//! Content-Type: application/json
//!
//! {
//!   "model": "claude-3-5-sonnet-20241022",
//!   "messages": [...],
//!   "temperature": 0.7,
//!   "max_tokens": 1000,
//!   "stream": false
//! }
//! ```
//!
//! 响应头包含：
//! - `x-request-id`: 请求标识符
//! - `x-brokerrouter-request-id`: Brokerrouter 特定的请求 ID
//!
//! ## 已知限制
//!
//! 参见 `docs/brokerrouter-gaps.md` 了解当前限制和跟踪的议题：
//! - [#28](https://github.com/StateKnot/Brokerrouter/issues/28) - 消费方接入指南
//! - [#29](https://github.com/StateKnot/Brokerrouter/issues/29) - Chat SSE `stream:true`
//! - [#30](https://github.com/StateKnot/Brokerrouter/issues/30) - 单人本地配置
//! - [#31](https://github.com/StateKnot/Brokerrouter/issues/31) - Agent tool roundtrip

use jiaclaw_core::{ChatMessage, ChatResponse, JiaClawError, MessageRole, RunStatus};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Brokerrouter Chat Completions API 请求
#[derive(Debug, Serialize)]
struct BrokerrouterChatRequest {
    model: String,
    messages: Vec<BrokerrouterMessage>,
    temperature: f32,
    max_tokens: u32,
    /// 当前应用使用非流式；网关 SSE 已支持，应用逐事件接线尚待实现。
    stream: bool,
}

/// Brokerrouter 消息格式（OpenAI-compatible）
#[derive(Debug, Serialize, Deserialize)]
struct BrokerrouterMessage {
    role: String,
    content: String,
}

/// Brokerrouter Chat Completions API 响应
#[derive(Debug, Deserialize)]
struct BrokerrouterChatResponse {
    choices: Vec<BrokerrouterChoice>,
}

/// Brokerrouter 响应选项
#[derive(Debug, Deserialize)]
struct BrokerrouterChoice {
    message: BrokerrouterMessage,
}

/// Brokerrouter 提供商（推荐生产路径）
#[allow(clippy::module_name_repetitions)]
pub struct BrokerrouterProvider {
    base_url: String,
    virtual_key: String,
}

impl BrokerrouterProvider {
    /// 创建新的 Brokerrouter 提供商实例
    ///
    /// # 参数
    ///
    /// - `base_url`: Brokerrouter Gateway 基础 URL (例如 `https://api.brokerrouter.dev`)
    /// - `virtual_key`: Brokerrouter 虚拟密钥 (以 `brk_` 开头)
    ///
    /// # 注意
    ///
    /// 虚拟密钥应通过环境变量或安全配置提供，不要硬编码到代码中。
    pub fn new(base_url: &str, virtual_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            virtual_key: virtual_key.to_string(),
        }
    }

    /// 生成幂等性密钥
    ///
    /// 根据 Brokerrouter 契约要求：
    /// - 1-200 个可打印 ASCII 字符
    /// - 每次尝试都应该唯一
    /// - 使用 UUID v4 作为基础（符合要求且保证唯一性）
    fn generate_idempotency_key() -> String {
        format!("jiaclaw-{}", Uuid::new_v4())
    }

    /// 调用聊天补全 API
    pub async fn chat(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[ChatMessage],
        temperature: f32,
        max_tokens: u32,
    ) -> Result<ChatResponse, JiaClawError> {
        // 克隆参数以便在 spawn_blocking 中使用
        let base_url = self.base_url.clone();
        let virtual_key = self.virtual_key.clone();
        let model = model.to_string();
        let system_prompt = system_prompt.to_string();
        let messages = messages.to_vec();

        // 使用 spawn_blocking 在线程池中执行同步 HTTP 请求
        tokio::task::spawn_blocking(move || {
            Self::chat_sync(
                &base_url,
                &virtual_key,
                &model,
                &system_prompt,
                &messages,
                temperature,
                max_tokens,
            )
        })
        .await
        .map_err(|e| JiaClawError::StateKnotIntegration(format!("任务执行失败: {e}")))?
    }

    /// 同步的聊天补全实现
    #[allow(clippy::too_many_arguments)]
    fn chat_sync(
        base_url: &str,
        virtual_key: &str,
        model: &str,
        system_prompt: &str,
        messages: &[ChatMessage],
        temperature: f32,
        max_tokens: u32,
    ) -> Result<ChatResponse, JiaClawError> {
        // 构建 Brokerrouter 格式的消息列表
        let mut broker_messages = vec![BrokerrouterMessage {
            role: "system".to_string(),
            content: system_prompt.to_string(),
        }];

        for msg in messages {
            broker_messages.push(BrokerrouterMessage {
                role: match msg.role {
                    MessageRole::User => "user".to_string(),
                    MessageRole::Assistant => "assistant".to_string(),
                    MessageRole::System => "system".to_string(),
                },
                content: msg.content.clone(),
            });
        }

        // 构建请求
        let request = BrokerrouterChatRequest {
            model: model.to_string(),
            messages: broker_messages,
            temperature,
            max_tokens,
            stream: false, // Brokerrouter M2 仅支持非流式
        };

        // 生成幂等性密钥
        let idempotency_key = Self::generate_idempotency_key();

        // 发送请求
        let url = format!("{base_url}/v1/chat/completions");
        tracing::debug!("调用 Brokerrouter API: {}", url);
        tracing::trace!("幂等性密钥: {}", idempotency_key);

        let request_json = serde_json::to_string(&request)
            .map_err(|e| JiaClawError::StateKnotIntegration(format!("序列化请求失败: {e}")))?;

        let response = minreq::post(&url)
            .with_header("Authorization", format!("Bearer {virtual_key}"))
            .with_header("Content-Type", "application/json")
            .with_header("Idempotency-Key", idempotency_key)
            .with_body(request_json)
            .send()
            .map_err(|e| JiaClawError::StateKnotIntegration(format!("API 请求失败: {e}")))?;

        // 捕获请求追踪 ID
        let request_id = response
            .headers
            .get("x-request-id")
            .map(std::string::ToString::to_string);
        let broker_request_id = response
            .headers
            .get("x-brokerrouter-request-id")
            .map(std::string::ToString::to_string);

        if let Some(ref id) = request_id {
            tracing::debug!("x-request-id: {}", id);
        }
        if let Some(ref id) = broker_request_id {
            tracing::debug!("x-brokerrouter-request-id: {}", id);
        }

        // 检查响应状态
        if response.status_code != 200 {
            let status = response.status_code;
            let body = response.as_str().unwrap_or_default();
            return Err(JiaClawError::StateKnotIntegration(format!(
                "Brokerrouter 返回错误状态 {status}: {body}"
            )));
        }

        let broker_response: BrokerrouterChatResponse = response.json().map_err(|e| {
            JiaClawError::StateKnotIntegration(format!("解析 Brokerrouter 响应失败: {e}"))
        })?;

        // 转换为 JiaClaw 响应格式
        let assistant_message = broker_response
            .choices
            .first()
            .ok_or_else(|| {
                JiaClawError::StateKnotIntegration("Brokerrouter 响应中没有选项".to_string())
            })?
            .message
            .content
            .clone();

        Ok(ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: assistant_message,
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
    fn test_provider_creation() {
        let provider = BrokerrouterProvider::new("https://api.brokerrouter.dev", "brk_test_key");
        assert_eq!(provider.base_url, "https://api.brokerrouter.dev");
        assert_eq!(provider.virtual_key, "brk_test_key");
    }

    #[test]
    fn test_provider_trims_slash() {
        let provider = BrokerrouterProvider::new("https://api.brokerrouter.dev/", "brk_test_key");
        assert_eq!(provider.base_url, "https://api.brokerrouter.dev");
    }

    #[test]
    fn test_idempotency_key_generation() {
        let key1 = BrokerrouterProvider::generate_idempotency_key();
        let key2 = BrokerrouterProvider::generate_idempotency_key();

        // 验证格式
        assert!(key1.starts_with("jiaclaw-"));
        assert!(key2.starts_with("jiaclaw-"));

        // 验证唯一性
        assert_ne!(key1, key2);

        // 验证长度（UUID 长度 + 前缀）
        assert!(key1.len() > 10);
        assert!(key1.len() <= 200); // Brokerrouter 限制

        // 验证只包含可打印 ASCII
        assert!(key1.chars().all(|c| c.is_ascii() && !c.is_ascii_control()));
    }

    #[tokio::test]
    async fn test_brokerrouter_chat_with_mock() {
        // 设置 mock 响应
        let response_body = serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "claude-3-5-sonnet-20241022",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "测试响应"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let _mock = mockito::mock("POST", "/v1/chat/completions")
            .match_header("Authorization", "Bearer brk_test_key")
            .match_header("Content-Type", "application/json")
            .match_header("Idempotency-Key", mockito::Matcher::Any)
            .with_status(200)
            .with_header("x-request-id", "req-123")
            .with_header("x-brokerrouter-request-id", "brk-req-456")
            .with_body(response_body.to_string())
            .create();

        // 创建提供商
        let provider = BrokerrouterProvider::new(&mockito::server_url(), "brk_test_key");

        // 调用聊天
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "测试消息".to_string(),
        }];

        let response = provider
            .chat(
                "claude-3-5-sonnet-20241022",
                "你是一个有用的助手",
                &messages,
                0.7,
                1000,
            )
            .await;

        // 验证响应
        assert!(response.is_ok());
        let response = response.unwrap();
        assert_eq!(response.message.content, "测试响应");
        assert_eq!(response.status, RunStatus::Completed);
    }

    #[tokio::test]
    async fn test_brokerrouter_error_handling() {
        // 设置错误响应
        let _mock = mockito::mock("POST", "/v1/chat/completions")
            .with_status(401)
            .with_body("Unauthorized: Invalid API key")
            .create();

        // 创建提供商
        let provider = BrokerrouterProvider::new(&mockito::server_url(), "brk_invalid_key");

        // 调用聊天
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "测试消息".to_string(),
        }];

        let response = provider
            .chat("gpt-4", "你是一个有用的助手", &messages, 0.7, 1000)
            .await;

        // 验证错误
        assert!(response.is_err());
        let err = response.unwrap_err();
        assert!(err.to_string().contains("Brokerrouter 返回错误状态 401"));
    }

    #[tokio::test]
    async fn test_brokerrouter_stream_false() {
        let response_body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "ok"
                }
            }]
        });

        // 验证请求中包含 stream: false
        let _mock = mockito::mock("POST", "/v1/chat/completions")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "model": "gpt-4",
                "messages": [
                    {"role": "system", "content": "system"},
                    {"role": "user", "content": "hello"}
                ],
                "temperature": 0.7,
                "max_tokens": 100,
                "stream": false
            })))
            .with_status(200)
            .with_body(response_body.to_string())
            .create();

        let provider = BrokerrouterProvider::new(&mockito::server_url(), "brk_test");

        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "hello".to_string(),
        }];

        let result = provider.chat("gpt-4", "system", &messages, 0.7, 100).await;

        assert!(result.is_ok());
    }
}
