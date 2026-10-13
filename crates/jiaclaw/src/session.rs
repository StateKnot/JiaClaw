// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 会话历史溢出处理：硬截断与可选摘要压缩。
//!
//! 摘要调用只走一次无工具的 LLM 补全（或存根摘要），不会进入 tool loop，
//! 因此 heartbeat / chat 追加路径不会因摘要而递归爆炸。

use async_trait::async_trait;
use jiaclaw_core::{ChatMessage, JiaClawError, MessageRole, MAX_SESSION_MESSAGES};

/// 写入 session 的摘要消息前缀，便于识别压缩产物。
pub const SESSION_SUMMARY_PREFIX: &str = "[session-summary]";

/// 摘要请求的固定中英说明（无工具）。
pub const SESSION_SUMMARY_PROMPT: &str = "Summarize the following conversation for continuity. \
     请用简洁中英要点概括以下对话，保留关键事实、约定与未决事项，便于后续对话继续。不要调用工具。";

/// 摘要补全的 `max_tokens` 上限，避免摘要本身占用过长上下文。
pub const SESSION_SUMMARY_MAX_TOKENS: u32 = 512;

/// 摘要补全温度（偏低，偏向稳定要点）。
pub const SESSION_SUMMARY_TEMPERATURE: f32 = 0.2;

/// Complete formatted summary input, including role labels and separators.
/// This bounds input bytes independently of the summary's output token limit.
pub const SESSION_SUMMARY_INPUT_MAX_BYTES: usize = 256 * 1024;

/// 将会话旧消息折叠成一条摘要。失败时由调用方回退硬截断。
#[async_trait]
pub trait ConversationSummarizer: Send + Sync {
    /// 为给定消息生成连续性摘要文本（不含 role 包装）。
    async fn summarize_conversation(
        &self,
        messages: &[ChatMessage],
    ) -> Result<String, JiaClawError>;
}

/// 硬截断：保留全部 system 消息与最新的非 system 消息，使总数不超过 `max_messages`。
#[must_use]
pub fn hard_truncate_session_messages(
    messages: Vec<ChatMessage>,
    max_messages: usize,
) -> Vec<ChatMessage> {
    if messages.len() <= max_messages {
        return messages;
    }

    let system_messages: Vec<_> = messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .cloned()
        .collect();
    let non_system_messages: Vec<_> = messages
        .into_iter()
        .filter(|m| m.role != MessageRole::System)
        .collect();

    let system_count = system_messages.len();
    let available_slots = max_messages.saturating_sub(system_count);
    let skip_count = non_system_messages.len().saturating_sub(available_slots);

    let mut truncated = system_messages;
    truncated.extend(non_system_messages.into_iter().skip(skip_count));
    truncated
}

/// 将完整会话消息格式化为摘要文本，不截断任何一条消息。
///
/// # Errors
/// The complete UTF-8 transcript must fit within `SESSION_SUMMARY_INPUT_MAX_BYTES`.
/// Refuse before allocating the transcript or submitting a partial summary.
pub fn format_messages_for_summary(messages: &[ChatMessage]) -> Result<String, JiaClawError> {
    let bytes = messages.iter().try_fold(0usize, |total, msg| {
        total
            .checked_add(role_label(&msg.role).len())
            .and_then(|n| n.checked_add(2))
            .and_then(|n| n.checked_add(msg.content.len()))
            .filter(|n| *n <= SESSION_SUMMARY_INPUT_MAX_BYTES)
    });
    let bytes = bytes
        .and_then(|n| n.checked_add(messages.len().saturating_sub(1)))
        .filter(|n| *n <= SESSION_SUMMARY_INPUT_MAX_BYTES)
        .ok_or_else(|| JiaClawError::InvalidRequest("会话摘要完整输入超过256KiB".to_string()))?;
    let mut out = String::with_capacity(bytes);
    for msg in messages {
        let role = role_label(&msg.role);
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(role);
        out.push_str(": ");
        out.push_str(&msg.content);
    }
    Ok(out)
}

/// 无 LLM 时的确定性摘要（stub / 未知提供商），不触发工具。
#[must_use]
pub fn local_conversation_digest(messages: &[ChatMessage]) -> String {
    const HEAD: usize = 6;
    let mut parts = Vec::new();
    let head_len = HEAD.min(messages.len());
    for msg in messages.iter().take(head_len) {
        parts.push(format!(
            "{}: {}",
            role_label(&msg.role),
            truncate_chars(&msg.content, 80)
        ));
    }
    if messages.len() > HEAD {
        parts.push(format!("... ({} more messages)", messages.len() - HEAD));
        if let Some(last) = messages.last() {
            parts.push(format!(
                "{}: {}",
                role_label(&last.role),
                truncate_chars(&last.content, 80)
            ));
        }
    }
    format!(
        "Earlier conversation ({} messages): {}",
        messages.len(),
        parts.join(" | ")
    )
}

/// 接近或超过上限时压缩会话历史。
///
/// - 未开启摘要：硬截断（与现网一致）
/// - 开启摘要：把最旧一批折叠为一条 `role=system` 摘要，保留最近 `keep_recent`
/// - 摘要失败：warn 并由本函数回退硬截断，不向上抛错
pub async fn compact_session_history<S: ConversationSummarizer + ?Sized>(
    messages: Vec<ChatMessage>,
    max_messages: usize,
    summarize_on_overflow: bool,
    keep_recent: usize,
    summarizer: &S,
) -> Vec<ChatMessage> {
    if messages.len() <= max_messages {
        return messages;
    }

    if summarize_on_overflow {
        match summarize_overflow(&messages, max_messages, keep_recent, summarizer).await {
            Ok(compacted) => {
                tracing::info!(
                    before = messages.len(),
                    after = compacted.len(),
                    keep_recent,
                    "会话历史已摘要压缩"
                );
                return compacted;
            }
            Err(e) => {
                tracing::warn!("会话摘要压缩失败，回退到硬截断: {e}");
            }
        }
    }

    hard_truncate_session_messages(messages, max_messages)
}

/// 使用默认 `MAX_SESSION_MESSAGES` 的便捷入口。
pub async fn compact_session_history_default<S: ConversationSummarizer + ?Sized>(
    messages: Vec<ChatMessage>,
    summarize_on_overflow: bool,
    keep_recent: usize,
    summarizer: &S,
) -> Vec<ChatMessage> {
    compact_session_history(
        messages,
        MAX_SESSION_MESSAGES,
        summarize_on_overflow,
        keep_recent,
        summarizer,
    )
    .await
}

/// 导入会话时压缩超长历史：复用硬截断/摘要策略，摘要只用本地 digest，不调用 LLM。
pub async fn compact_imported_session_messages(
    messages: Vec<ChatMessage>,
    summarize_on_overflow: bool,
    keep_recent: usize,
) -> Vec<ChatMessage> {
    struct ImportLocalSummarizer;

    #[async_trait]
    impl ConversationSummarizer for ImportLocalSummarizer {
        async fn summarize_conversation(
            &self,
            messages: &[ChatMessage],
        ) -> Result<String, JiaClawError> {
            Ok(local_conversation_digest(messages))
        }
    }

    compact_session_history(
        messages,
        MAX_SESSION_MESSAGES,
        summarize_on_overflow,
        keep_recent,
        &ImportLocalSummarizer,
    )
    .await
}

async fn summarize_overflow<S: ConversationSummarizer + ?Sized>(
    messages: &[ChatMessage],
    max_messages: usize,
    keep_recent: usize,
    summarizer: &S,
) -> Result<Vec<ChatMessage>, JiaClawError> {
    let keep = keep_recent.clamp(1, max_messages.saturating_sub(1));
    if messages.len() <= keep {
        return Ok(messages.to_vec());
    }

    let split = messages.len() - keep;
    let old = &messages[..split];
    let recent = &messages[split..];
    if old.is_empty() {
        return Ok(messages.to_vec());
    }

    let summary = summarizer.summarize_conversation(old).await?;
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(JiaClawError::InvalidRequest("摘要为空".to_string()));
    }

    let mut out = Vec::with_capacity(1 + recent.len());
    out.push(ChatMessage {
        role: MessageRole::System,
        content: format!("{SESSION_SUMMARY_PREFIX}\n{summary}"),
    });
    out.extend(recent.iter().cloned());

    if out.len() > max_messages {
        return Ok(hard_truncate_session_messages(out, max_messages));
    }
    Ok(out)
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::User => "User",
        MessageRole::Assistant => "Assistant",
        MessageRole::System => "System",
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::DEFAULT_SESSION_KEEP_RECENT;

    struct OkSummarizer {
        text: String,
    }

    #[async_trait]
    impl ConversationSummarizer for OkSummarizer {
        async fn summarize_conversation(
            &self,
            messages: &[ChatMessage],
        ) -> Result<String, JiaClawError> {
            assert!(!messages.is_empty(), "应摘要旧消息");
            Ok(self.text.clone())
        }
    }

    struct FailSummarizer;

    #[async_trait]
    impl ConversationSummarizer for FailSummarizer {
        async fn summarize_conversation(
            &self,
            _messages: &[ChatMessage],
        ) -> Result<String, JiaClawError> {
            Err(JiaClawError::StateKnotIntegration(
                "mock provider down".to_string(),
            ))
        }
    }

    fn user(n: usize) -> ChatMessage {
        ChatMessage {
            role: MessageRole::User,
            content: format!("消息 {n}"),
        }
    }

    fn system(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::System,
            content: text.to_string(),
        }
    }

    #[test]
    fn summary_input_keeps_complete_long_utf8_material_and_roles() {
        let material = format!("\u{feff}\t{}\r\n尾部約定 000123  ", "材料".repeat(900));
        let messages = vec![
            system("system\nconstraint"),
            ChatMessage {
                role: MessageRole::User,
                content: material.clone(),
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: "done\0".into(),
            },
        ];
        assert_eq!(
            format_messages_for_summary(&messages).unwrap(),
            format!("System: system\nconstraint\nUser: {material}\nAssistant: done\0")
        );
    }

    #[test]
    fn summary_input_counts_roles_separators_and_exact_utf8_boundary() {
        let messages = vec![
            user(0),
            ChatMessage {
                role: MessageRole::User,
                content: "界".repeat((SESSION_SUMMARY_INPUT_MAX_BYTES - 21) / 3),
            },
        ];
        let mut messages = messages;
        let missing =
            SESSION_SUMMARY_INPUT_MAX_BYTES - format_messages_for_summary(&messages).unwrap().len();
        messages[1].content.push_str(&"x".repeat(missing));
        assert_eq!(
            format_messages_for_summary(&messages).unwrap().len(),
            SESSION_SUMMARY_INPUT_MAX_BYTES
        );
        messages[1].content.push('x');
        assert!(format_messages_for_summary(&messages).is_err());
    }

    #[test]
    fn summary_input_refuses_total_budget_without_individual_truncation() {
        let mut messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "x".repeat(SESSION_SUMMARY_INPUT_MAX_BYTES - 6),
        }];
        assert_eq!(
            format_messages_for_summary(&messages).unwrap().len(),
            SESSION_SUMMARY_INPUT_MAX_BYTES
        );
        messages.push(ChatMessage {
            role: MessageRole::Assistant,
            content: String::new(),
        });
        let error = format_messages_for_summary(&messages)
            .unwrap_err()
            .to_string();
        assert!(error.contains("256KiB"));
        assert!(!error.contains(&"x".repeat(20)));
        assert_eq!(format_messages_for_summary(&[]).unwrap(), "");
    }

    #[test]
    fn hard_truncate_keeps_system_and_newest() {
        let mut messages = vec![system("你是一个助手")];
        messages.extend((0..60).map(user));
        let truncated = hard_truncate_session_messages(messages, 50);
        assert_eq!(truncated.len(), 50);
        assert_eq!(truncated[0], system("你是一个助手"));
        assert_eq!(truncated.last().unwrap().content, "消息 59");
        assert!(truncated.iter().all(|m| m.content != "消息 0"));
    }

    #[test]
    fn hard_truncate_noop_when_under_limit() {
        let messages = vec![user(1), user(2)];
        let out = hard_truncate_session_messages(messages.clone(), 50);
        assert_eq!(out, messages);
    }

    #[tokio::test]
    async fn disabled_summarize_matches_hard_truncate() {
        let mut messages = vec![system("sys")];
        messages.extend((0..60).map(user));
        let expected = hard_truncate_session_messages(messages.clone(), 50);
        let actual = compact_session_history(
            messages,
            50,
            false,
            DEFAULT_SESSION_KEEP_RECENT,
            &OkSummarizer {
                text: "SHOULD_NOT_BE_CALLED".to_string(),
            },
        )
        .await;
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn enabled_summarize_replaces_old_with_one_summary_and_keeps_recent() {
        let keep = 10;
        let mut messages = Vec::new();
        for i in 0..51 {
            messages.push(user(i));
        }
        let summarizer = OkSummarizer {
            text: "用户讨论了编号 0-40 的话题".to_string(),
        };
        let compacted = compact_session_history(messages, 50, true, keep, &summarizer).await;
        assert_eq!(compacted.len(), 1 + keep);
        assert_eq!(compacted[0].role, MessageRole::System);
        assert!(
            compacted[0].content.contains(SESSION_SUMMARY_PREFIX),
            "摘要应带标记: {}",
            compacted[0].content
        );
        assert!(compacted[0].content.contains("用户讨论了编号 0-40 的话题"));
        assert_eq!(compacted[1].content, "消息 41");
        assert_eq!(compacted.last().unwrap().content, "消息 50");
        assert!(compacted.iter().all(|m| m.content != "消息 0"));
    }

    #[tokio::test]
    async fn summarize_provider_error_falls_back_to_hard_truncate() {
        let mut messages = vec![system("sys")];
        messages.extend((0..60).map(user));
        let expected = hard_truncate_session_messages(messages.clone(), 50);
        let actual = compact_session_history(messages, 50, true, 10, &FailSummarizer).await;
        assert_eq!(actual, expected);
        assert_eq!(actual[0], system("sys"));
        assert!(actual.iter().any(|m| m.content == "消息 59"));
        assert!(actual.iter().all(|m| m.content != "消息 0"));
    }

    #[tokio::test]
    async fn summarize_empty_result_falls_back() {
        struct EmptySummarizer;
        #[async_trait]
        impl ConversationSummarizer for EmptySummarizer {
            async fn summarize_conversation(
                &self,
                _messages: &[ChatMessage],
            ) -> Result<String, JiaClawError> {
                Ok("   ".to_string())
            }
        }

        let messages: Vec<_> = (0..51).map(user).collect();
        let expected = hard_truncate_session_messages(messages.clone(), 50);
        let actual = compact_session_history(messages, 50, true, 10, &EmptySummarizer).await;
        assert_eq!(actual, expected);
    }

    #[test]
    fn local_digest_includes_counts_and_head() {
        let messages: Vec<_> = (0..12).map(user).collect();
        let digest = local_conversation_digest(&messages);
        assert!(digest.contains("12 messages"));
        assert!(digest.contains("消息 0"));
        assert!(digest.contains("消息 11"));
    }

    #[tokio::test]
    async fn import_compact_off_hard_truncates_without_llm() {
        let messages: Vec<_> = (0..60).map(user).collect();
        let expected = hard_truncate_session_messages(messages.clone(), MAX_SESSION_MESSAGES);
        let actual = compact_imported_session_messages(messages, false, 10).await;
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), MAX_SESSION_MESSAGES);
    }

    #[tokio::test]
    async fn import_compact_on_uses_local_digest_not_provider() {
        let messages: Vec<_> = (0..51).map(user).collect();
        let compacted = compact_imported_session_messages(messages, true, 10).await;
        assert_eq!(compacted.len(), 11);
        assert_eq!(compacted[0].role, MessageRole::System);
        assert!(compacted[0].content.contains(SESSION_SUMMARY_PREFIX));
        assert!(compacted[0].content.contains("消息 0"));
        assert_eq!(compacted[1].content, "消息 41");
        assert_eq!(compacted.last().unwrap().content, "消息 50");
    }
}
