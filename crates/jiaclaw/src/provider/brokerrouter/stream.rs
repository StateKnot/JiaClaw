// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Fixed Brokerrouter SSE contract. Previews are not tool authority or receipts.

use super::{
    completion_receipt, failure, BrokerrouterProvider, CompletionReceipt, PendingCompletion,
    PreparedCompletion, MAX_BODY_BYTES,
};
use crate::ChatProgress;
use jiaclaw_core::JiaClawError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MAX_WIRE: usize = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 100_000;
const MAX_TOOLS: usize = 32;
const MAX_ARGUMENTS: usize = 16 * 1024;

fn invalid() -> JiaClawError {
    failure("invalid, incomplete or over-limit SSE; no tool authority; not retried")
}

// Derived struct deserialization rejects duplicate known fields, including null duplicates.
// Unknown wire fields reject rather than silently gaining authority in a future protocol.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<Choice>,
    usage: Option<Usage>,
    #[serde(default, rename = "system_fingerprint")]
    _fingerprint: Option<Value>,
    #[serde(default, rename = "service_tier")]
    _service_tier: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    index: u64,
    delta: Delta,
    finish_reason: Option<String>,
    logprobs: Option<Value>,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Delta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<ToolDelta>>,
    refusal: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDelta {
    index: u64,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<FunctionDelta>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}
#[derive(Deserialize)]
struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Deserialize)]
struct RecoveredEnvelope {
    id: String,
    object: String,
    created: i64,
    usage: Usage,
}
pub(super) fn validate_recovery(bytes: &[u8]) -> Result<(), JiaClawError> {
    let body: RecoveredEnvelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if body.id.is_empty()
        || body.id.len() > 512
        || body.object != "chat.completion"
        || body
            .usage
            .prompt_tokens
            .checked_add(body.usage.completion_tokens)
            != Some(body.usage.total_tokens)
    {
        return Err(invalid());
    }
    let _ = body.created;
    Ok(())
}
#[derive(Default)]
struct Tool {
    id: Option<String>,
    kind: Option<String>,
    name: String,
    arguments: String,
}
#[derive(Default)]
struct Aggregate {
    id: Option<String>,
    created: Option<i64>,
    role: bool,
    content: Option<String>,
    refusal: Option<String>,
    tools: BTreeMap<u64, Tool>,
    finish: Option<String>,
    usage: Option<Usage>,
    events: usize,
    text_bytes: usize,
}
impl Aggregate {
    fn push(&mut self, data: &str, model: &str) -> Result<Option<String>, JiaClawError> {
        self.events += 1;
        if self.events > MAX_EVENTS || self.usage.is_some() {
            return Err(invalid());
        }
        let chunk: Chunk = serde_json::from_str(data).map_err(|_| invalid())?;
        if chunk.object != "chat.completion.chunk"
            || chunk.model != model
            || chunk.id.is_empty()
            || chunk.id.len() > 512
            || chunk.choices.len() > 1
            || self.id.as_ref().is_some_and(|id| id != &chunk.id)
            || self.created.is_some_and(|created| created != chunk.created)
        {
            return Err(invalid());
        }
        self.id.get_or_insert(chunk.id);
        self.created.get_or_insert(chunk.created);
        if chunk.choices.is_empty() {
            let usage = chunk.usage.ok_or_else(invalid)?;
            if self.finish.is_none()
                || usage.prompt_tokens.checked_add(usage.completion_tokens)
                    != Some(usage.total_tokens)
            {
                return Err(invalid());
            }
            self.usage = Some(usage);
            return Ok(None);
        }
        if self.finish.is_some() || chunk.usage.is_some() {
            return Err(invalid());
        }
        let choice = chunk.choices.into_iter().next().ok_or_else(invalid)?;
        if choice.index != 0 || choice.logprobs.is_some_and(|value| !value.is_null()) {
            return Err(invalid());
        }
        if let Some(reason) = choice.finish_reason {
            if !matches!(reason.as_str(), "stop" | "tool_calls") {
                return Err(invalid());
            }
            self.finish = Some(reason);
        }
        if let Some(role) = choice.delta.role {
            if role != "assistant" {
                return Err(invalid());
            }
            self.role = true;
        }
        let preview = choice.delta.content;
        if let Some(text) = &preview {
            self.add_bytes(text.len())?;
            self.content.get_or_insert_with(String::new).push_str(text);
        }
        if let Some(refusal) = choice.delta.refusal {
            self.add_bytes(refusal.len())?;
            self.refusal
                .get_or_insert_with(String::new)
                .push_str(&refusal);
        }
        if let Some(calls) = choice.delta.tool_calls {
            if calls.is_empty() || calls.len() > MAX_TOOLS {
                return Err(invalid());
            }
            for call in calls {
                if call.index >= MAX_TOOLS as u64 {
                    return Err(invalid());
                }
                // Account every retained fragment, including metadata, before adding it.
                let bytes = call.id.as_ref().map_or(0, String::len)
                    + call.kind.as_ref().map_or(0, String::len)
                    + call.function.as_ref().map_or(0, |f| {
                        f.name.as_ref().map_or(0, String::len)
                            + f.arguments.as_ref().map_or(0, String::len)
                    });
                self.add_bytes(bytes)?;
                let tool = self.tools.entry(call.index).or_default();
                if let Some(id) = call.id {
                    if id.is_empty()
                        || id.len() > 200
                        || tool.id.as_ref().is_some_and(|existing| existing != &id)
                    {
                        return Err(invalid());
                    }
                    tool.id = Some(id);
                }
                if let Some(kind) = call.kind {
                    if kind != "function"
                        || tool.kind.as_ref().is_some_and(|existing| existing != &kind)
                    {
                        return Err(invalid());
                    }
                    tool.kind = Some(kind);
                }
                if let Some(function) = call.function {
                    if let Some(name) = function.name {
                        tool.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments {
                        tool.arguments.push_str(&arguments);
                    }
                    if tool.name.len() > 64 || tool.arguments.len() > MAX_ARGUMENTS {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(preview)
    }
    fn add_bytes(&mut self, bytes: usize) -> Result<(), JiaClawError> {
        self.text_bytes = self.text_bytes.checked_add(bytes).ok_or_else(invalid)?;
        if self.text_bytes > MAX_BODY_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
    fn finish(self, model: &str) -> Result<Vec<u8>, JiaClawError> {
        if !self.role {
            return Err(invalid());
        }
        let mut message = json!({"role":"assistant", "content":self.content});
        if let Some(refusal) = self.refusal {
            message["refusal"] = refusal.into();
        }
        if !self.tools.is_empty() {
            let mut calls = Vec::with_capacity(self.tools.len());
            for (expected, (index, tool)) in self.tools.into_iter().enumerate() {
                if index != expected as u64
                    || tool.id.is_none()
                    || tool.kind.as_deref() != Some("function")
                    || tool.name.is_empty()
                {
                    return Err(invalid());
                }
                calls.push(json!({"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":tool.arguments}}));
            }
            message["tool_calls"] = calls.into();
        }
        let usage = self.usage.ok_or_else(invalid)?;
        let body = serde_json::to_vec(&json!({"id":self.id.ok_or_else(invalid)?, "object":"chat.completion",
            "created":self.created.ok_or_else(invalid)?, "model":model,
            "choices":[{"index":0,"message":message,"finish_reason":self.finish.ok_or_else(invalid)?}],
            "usage":{"prompt_tokens":usage.prompt_tokens,"completion_tokens":usage.completion_tokens,"total_tokens":usage.total_tokens}}))
            .map_err(|_| invalid())?;
        if body.len() > MAX_BODY_BYTES {
            return Err(invalid());
        }
        Ok(body)
    }
}

/// Incremental SSE framing over bytes, including split UTF-8, CR/LF/CRLF and multi-data lines.
#[derive(Default)]
struct Framer {
    line: Vec<u8>,
    data: Option<String>,
    after_cr: bool,
    first_line: bool,
    wire: usize,
}
impl Framer {
    fn byte(&mut self, byte: u8) -> Result<Option<String>, JiaClawError> {
        self.wire += 1;
        if self.wire > MAX_WIRE {
            return Err(invalid());
        }
        if self.after_cr && byte == b'\n' {
            self.after_cr = false;
            return Ok(None);
        }
        self.after_cr = byte == b'\r';
        if byte == b'\r' || byte == b'\n' {
            let bytes = std::mem::take(&mut self.line);
            let mut line = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
            if !self.first_line {
                self.first_line = true;
                line = line.strip_prefix('\u{feff}').unwrap_or(line);
            }
            if line.is_empty() {
                return Ok(self.data.take());
            }
            if line.starts_with(':') {
                return Ok(None);
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            if field != "data" {
                return Err(invalid());
            }
            let value = value.strip_prefix(' ').unwrap_or(value);
            if let Some(data) = &mut self.data {
                data.push('\n');
                data.push_str(value);
            } else {
                self.data = Some(value.to_owned());
            }
        } else {
            self.line.push(byte);
        }
        Ok(None)
    }
    fn eof(&self) -> Result<(), JiaClawError> {
        if !self.line.is_empty() || self.data.is_some() {
            return Err(invalid());
        }
        Ok(())
    }
}

impl BrokerrouterProvider {
    pub(crate) async fn finish_stream(
        &self,
        pending: PendingCompletion,
        prepared: &PreparedCompletion,
        progress: &ChatProgress,
        round: u32,
    ) -> Result<CompletionReceipt, JiaClawError> {
        if !prepared.streaming {
            return Err(invalid());
        }
        let request_id = pending.remote_id.ok_or_else(invalid)?;
        let mut response = pending.response;
        let mut types = response
            .headers()
            .get_all(reqwest::header::CONTENT_TYPE)
            .iter();
        let mime = types
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next());
        if response.status() != reqwest::StatusCode::OK
            || types.next().is_some()
            || !mime.is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
            || response
                .content_length()
                .is_some_and(|size| size > MAX_WIRE as u64)
        {
            return Err(invalid());
        }
        let read = async {
            let mut framer = Framer::default();
            let mut aggregate = Aggregate::default();
            let mut done = false;
            while let Some(bytes) = response.chunk().await.map_err(|_| invalid())? {
                if bytes.len() > MAX_WIRE.saturating_sub(framer.wire) {
                    return Err(invalid());
                }
                for byte in bytes {
                    if let Some(data) = framer.byte(byte)? {
                        if done {
                            return Err(invalid());
                        }
                        if data == "[DONE]" {
                            done = true;
                        } else if let Some(text) = aggregate.push(&data, prepared.model())? {
                            progress.preview(round, &text).await;
                        }
                    }
                }
                if tokio::time::Instant::now() >= pending.deadline {
                    return Err(invalid());
                }
                // Even a large replay or comment chunk yields to cancellation/deadline polling.
                tokio::task::yield_now().await;
            }
            framer.eof()?;
            if !done {
                return Err(invalid());
            }
            let body = aggregate.finish(prepared.model())?;
            if tokio::time::Instant::now() >= pending.deadline {
                return Err(invalid());
            }
            completion_receipt(body, request_id, prepared.model(), prepared.has_tools())
        };
        tokio::time::timeout_at(pending.deadline, read)
            .await
            .map_err(|_| invalid())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(delta: Value, finish: Value) -> Value {
        json!({"id":"completion","object":"chat.completion.chunk","created":1,"model":"fixture",
            "choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":null})
    }
    fn usage() -> Value {
        json!({"id":"completion","object":"chat.completion.chunk","created":1,"model":"fixture",
            "choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}})
    }
    fn accepted(frames: &[Value]) -> Result<Vec<u8>, JiaClawError> {
        let mut aggregate = Aggregate::default();
        for frame in frames {
            aggregate.push(&frame.to_string(), "fixture")?;
        }
        aggregate.finish("fixture")
    }
    #[test]
    fn text_and_tool_replay_assemble_canonical_receipts() {
        let receipt = accepted(&[
            chunk(json!({"role":"assistant","content":"早🦀"}), json!("stop")),
            usage(),
        ])
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&receipt).unwrap()["choices"][0]["message"]["content"],
            "早🦀"
        );
        let calls = json!([{"index":0,"id":"call-z","type":"function","function":{"name":"file_","arguments":"{\"path\":"}}]);
        let tail =
            json!([{"index":0,"function":{"name":"write","arguments":"\"a\",\"content\":\"b\"}"}}]);
        let receipt = accepted(&[
            chunk(json!({"role":"assistant","tool_calls":calls}), Value::Null),
            chunk(json!({"tool_calls":tail}), json!("tool_calls")),
            usage(),
        ])
        .unwrap();
        let message = super::super::completion_message(&receipt, Some("fixture"), true).unwrap();
        assert_eq!(message.tool_calls[0].function.name, "file_write");
        assert_eq!(
            message.tool_calls[0].function.arguments,
            "{\"path\":\"a\",\"content\":\"b\"}"
        );
        assert!(super::super::completion_message(&receipt, Some("fixture"), false).is_err());
    }
    #[test]
    fn framing_accepts_byte_split_unicode_bom_crlf_and_multiline_data() {
        let payload =
            "\u{feff}:comment\r\ndata: {\r\ndata: \"text\":\"🦀\"}\r\n\r\ndata: [DONE]\n\n";
        let mut framer = Framer::default();
        let mut frames = Vec::new();
        for byte in payload.bytes() {
            if let Some(frame) = framer.byte(byte).unwrap() {
                frames.push(frame);
            }
        }
        framer.eof().unwrap();
        assert_eq!(frames, ["{\n\"text\":\"🦀\"}", "[DONE]"]);
        for bad in [
            "data: x",
            "data: x\n",
            "event: error\n\n",
            "id: x\n\n",
            "retry: 3\n\n",
            "foo: x\n\n",
        ] {
            let mut framer = Framer::default();
            let result: Result<(), _> = bad
                .bytes()
                .try_for_each(|byte| framer.byte(byte).map(|_| ()));
            assert!(result.is_err() || framer.eof().is_err(), "{bad}");
        }
        let mut framer = Framer::default();
        assert!(framer.byte(0xff).is_ok());
        assert!(framer.byte(b'\n').is_err());
    }
    #[test]
    fn conflicting_authority_and_duplicate_null_fields_reject() {
        let base = chunk(json!({"role":"assistant","content":"x"}), json!("stop"));
        for replacement in [
            "\"model\":\"fixture\",\"model\":\"fixture\"",
            "\"model\":null,\"model\":\"fixture\"",
        ] {
            let raw = base
                .to_string()
                .replace("\"model\":\"fixture\"", replacement);
            assert!(Aggregate::default().push(&raw, "fixture").is_err());
        }
        let raw = base
            .to_string()
            .replace("\"content\":\"x\"", "\"content\":null,\"content\":\"x\"");
        assert!(Aggregate::default().push(&raw, "fixture").is_err());
        for (field, value) in [
            ("model", json!("other")),
            ("object", json!("chat.completion")),
            ("id", json!("")),
            ("created", Value::Null),
            ("extra", json!(true)),
            ("choices", json!([])),
        ] {
            let mut bad = base.clone();
            bad[field] = value;
            assert!(
                Aggregate::default()
                    .push(&bad.to_string(), "fixture")
                    .is_err(),
                "{field}"
            );
        }
        for field in ["id", "created"] {
            let mut aggregate = Aggregate::default();
            aggregate
                .push(
                    &chunk(json!({"role":"assistant"}), Value::Null).to_string(),
                    "fixture",
                )
                .unwrap();
            let mut bad = base.clone();
            bad[field] = if field == "id" {
                json!("changed")
            } else {
                json!(2)
            };
            assert!(aggregate.push(&bad.to_string(), "fixture").is_err());
        }
    }
    #[test]
    fn usage_finish_choice_and_tool_limits_are_enforced() {
        let text = chunk(json!({"role":"assistant","content":"x"}), json!("stop"));
        assert!(accepted(&[text.clone()]).is_err());
        assert!(accepted(&[usage(), text.clone()]).is_err());
        assert!(accepted(&[text.clone(), usage(), usage()]).is_err());
        assert!(accepted(&[text.clone(), text.clone(), usage()]).is_err());
        let mut bad_usage = usage();
        bad_usage["usage"]["total_tokens"] = json!(4);
        assert!(accepted(&[text.clone(), bad_usage]).is_err());
        let mut overflow = usage();
        overflow["usage"]["prompt_tokens"] = json!(u64::MAX);
        assert!(accepted(&[text.clone(), overflow]).is_err());
        for field in ["index", "logprobs", "finish_reason"] {
            let mut bad = text.clone();
            bad["choices"][0][field] = if field == "index" {
                json!(1)
            } else {
                json!("invalid")
            };
            assert!(accepted(&[bad, usage()]).is_err());
        }
        for index in [1, 32] {
            let calls = json!([{"index":index,"id":"call","type":"function","function":{"name":"file_write","arguments":"{}"}}]);
            assert!(accepted(&[
                chunk(
                    json!({"role":"assistant","tool_calls":calls}),
                    json!("tool_calls")
                ),
                usage()
            ])
            .is_err());
        }
        let calls = json!([{"index":0,"id":"call","type":"function","function":{"name":"file_write","arguments":"a".repeat(MAX_ARGUMENTS+1)}}]);
        assert!(accepted(&[
            chunk(
                json!({"role":"assistant","tool_calls":calls}),
                json!("tool_calls")
            ),
            usage()
        ])
        .is_err());
        assert!(accepted(&[
            chunk(
                json!({"role":"assistant","content":"x".repeat(MAX_BODY_BYTES)}),
                json!("stop")
            ),
            usage()
        ])
        .is_err());
        let mut events = Aggregate {
            events: MAX_EVENTS,
            ..Aggregate::default()
        };
        assert!(events.push(&text.to_string(), "fixture").is_err());
        let mut bytes = Framer {
            wire: MAX_WIRE,
            ..Framer::default()
        };
        assert!(bytes.byte(b':').is_err());
    }

    #[test]
    fn recovered_stream_requires_complete_envelope_and_exact_usage() {
        let receipt = accepted(&[
            chunk(json!({"role":"assistant","content":"x"}), json!("stop")),
            usage(),
        ])
        .unwrap();
        validate_recovery(&receipt).unwrap();
        let original: Value = serde_json::from_slice(&receipt).unwrap();
        for field in ["usage", "id", "created", "object"] {
            let mut bad = original.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(validate_recovery(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        let mut bad = original;
        bad["usage"]["total_tokens"] = json!(4);
        assert!(validate_recovery(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}
