// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! OpenAI-compatible 提供商（已废弃）
//!
//! ⚠️ **已废弃**: 这是早期开发的临时实现。
//!
//! **推荐路径**: 使用 `BrokerrouterProvider`，通过 Brokerrouter Gateway
//! 统一路由到上游提供商（OpenAI, Anthropic, Ollama, etc.）。
//!
//! **保留原因**: 仅作为开发时的逃生舱，当 Brokerrouter 不可用时的备选方案。

use jiaclaw_core::{ChatMessage, ChatResponse, JiaClawError, MessageRole, RunStatus};
use serde::{Deserialize, Serialize};

/// `OpenAI` Chat Completions API 请求
#[derive(Debug, Serialize)]
struct OpenAIChatRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    temperature: f32,
    max_tokens: u32,
}

/// `OpenAI` 消息格式
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: String,
}

/// `OpenAI` Chat Completions API 响应
#[derive(Debug, Deserialize)]
struct OpenAIChatResponse {
    choices: Vec<OpenAIChoice>,
}

/// `OpenAI` 响应选项
#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

/// OpenAI-compatible 提供商（已废弃，临时直连实现）
#[allow(clippy::module_name_repetitions)]
pub struct OpenAICompatibleProvider {
    base_url: String,
    api_key: String,
}

impl OpenAICompatibleProvider {
    /// 创建新的提供商实例
    ///
    /// # 注意
    ///
    /// 这是已废弃的临时实现。生产环境应使用 `BrokerrouterProvider`。
    pub fn new(base_url: &str, api_key: &str) -> Self {
        tracing::warn!(
            "使用已废弃的 OpenAICompatibleProvider。推荐使用 BrokerrouterProvider (provider_type = \"brokerrouter\")。"
        );
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    /// 调用聊天补全 API（同步实现，使用 `spawn_blocking`）
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
        let api_key = self.api_key.clone();
        let model = model.to_string();
        let system_prompt = system_prompt.to_string();
        let messages = messages.to_vec();

        // 使用 spawn_blocking 在线程池中执行同步 HTTP 请求
        tokio::task::spawn_blocking(move || {
            Self::chat_sync(
                &base_url,
                &api_key,
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
    fn chat_sync(
        base_url: &str,
        api_key: &str,
        model: &str,
        system_prompt: &str,
        messages: &[ChatMessage],
        temperature: f32,
        max_tokens: u32,
    ) -> Result<ChatResponse, JiaClawError> {
        // 构建 OpenAI 格式的消息列表
        let mut openai_messages = vec![OpenAIMessage {
            role: "system".to_string(),
            content: system_prompt.to_string(),
        }];

        for msg in messages {
            openai_messages.push(OpenAIMessage {
                role: match msg.role {
                    MessageRole::User => "user".to_string(),
                    MessageRole::Assistant => "assistant".to_string(),
                    MessageRole::System => "system".to_string(),
                },
                content: msg.content.clone(),
            });
        }

        // 构建请求
        let request = OpenAIChatRequest {
            model: model.to_string(),
            messages: openai_messages,
            temperature,
            max_tokens,
        };

        // 发送请求
        let url = format!("{base_url}/chat/completions");
        tracing::debug!("调用 OpenAI-compatible API: {}", url);

        let request_json = serde_json::to_string(&request)
            .map_err(|e| JiaClawError::StateKnotIntegration(format!("序列化请求失败: {e}")))?;

        let response = minreq::post(&url)
            .with_header("Authorization", format!("Bearer {api_key}"))
            .with_header("Content-Type", "application/json")
            .with_body(request_json)
            .send()
            .map_err(|e| JiaClawError::StateKnotIntegration(format!("API 请求失败: {e}")))?;

        if response.status_code != 200 {
            let status = response.status_code;
            let body = response.as_str().unwrap_or_default();
            return Err(JiaClawError::StateKnotIntegration(format!(
                "API 返回错误状态 {status}: {body}"
            )));
        }

        let openai_response: OpenAIChatResponse = response
            .json()
            .map_err(|e| JiaClawError::StateKnotIntegration(format!("解析 API 响应失败: {e}")))?;

        // 转换为 JiaClaw 响应格式
        let assistant_message = openai_response
            .choices
            .first()
            .ok_or_else(|| JiaClawError::StateKnotIntegration("API 响应中没有选项".to_string()))?
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
            routing: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = OpenAICompatibleProvider::new("https://api.openai.com/v1", "test-key");
        assert_eq!(provider.base_url, "https://api.openai.com/v1");
        assert_eq!(provider.api_key, "test-key");
    }

    #[test]
    fn test_provider_trims_slash() {
        let provider = OpenAICompatibleProvider::new("https://api.openai.com/v1/", "test-key");
        assert_eq!(provider.base_url, "https://api.openai.com/v1");
    }
}
