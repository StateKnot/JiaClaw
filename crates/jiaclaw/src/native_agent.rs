// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Brokerrouter native tool loop. A whole model batch is checked before side effects.

use crate::provider::brokerrouter::{failure, BrokerrouterProvider, NativeToolCall, WireMessage};
use crate::schema_work::{process_workers, SchemaPhase};
use crate::JiaClawAgent;
use crate::{ChatProgress, ChatProgressEvent};
use jiaclaw_core::{
    ChatMessage, ChatRequest, ChatResponse, JiaClawError, MessageRole, ModelSelection, RunStatus,
    ToolCall,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;

const MAX_CALLS_PER_ROUND: usize = 32;
const MAX_CALLS_PER_TURN: usize = 128;
const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_RESULT_BYTES: usize = 256 * 1024;
// Each pure preflight has a finite waiting budget, in addition to the enclosing host/cron turn.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);

struct NativeTools {
    definitions: Vec<Value>,
    validators: HashMap<String, jsonschema::Validator>,
}

fn bounded_tree(value: &Value, depth: usize, remaining: &mut usize) -> bool {
    if depth > 24 || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    match value {
        Value::Array(values) => values.iter().all(|v| bounded_tree(v, depth + 1, remaining)),
        Value::Object(values) => values
            .values()
            .all(|v| bounded_tree(v, depth + 1, remaining)),
        _ => true,
    }
}

fn identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn compile_definitions(inputs: Vec<(String, String, Value)>) -> Result<NativeTools, JiaClawError> {
    let mut definitions = Vec::new();
    let mut validators = HashMap::new();
    for (name, description, schema) in inputs {
        if !identifier(&name, 64)
            || !schema.is_object()
            || !bounded_tree(&schema, 0, &mut 2048)
            || serde_json::to_vec(&schema).map_or(true, |b| b.len() > 32 * 1024)
        {
            return Err(JiaClawError::Configuration(
                "invalid or over-limit native tool definition".into(),
            ));
        }
        let validator = jsonschema::options()
            .should_validate_formats(true)
            .with_pattern_options(
                jsonschema::PatternOptions::fancy_regex()
                    .backtrack_limit(10_000)
                    .size_limit(256 * 1024)
                    .dfa_size_limit(256 * 1024),
            )
            .offline()
            .build(&schema)
            .map_err(|_| {
                JiaClawError::Configuration(
                    "native tool schema compilation failed; external references are disabled"
                        .into(),
                )
            })?;
        definitions.push(json!({"type":"function", "function":{"name": name, "description":description, "parameters":schema}}));
        validators.insert(name.clone(), validator);
    }
    Ok(NativeTools {
        definitions,
        validators,
    })
}

fn validate_batch(
    wire_calls: Vec<NativeToolCall>,
    tools: &NativeTools,
    mut seen_ids: HashSet<String>,
) -> Result<(Vec<ToolCall>, HashSet<String>), JiaClawError> {
    if wire_calls.len() > MAX_CALLS_PER_ROUND {
        return Err(failure("tool batch exceeds 32 calls; no tools dispatched"));
    }
    let mut calls = Vec::new();
    // This worker has no tool/network handles: an abandoned preflight cannot dispatch effects.
    for call in wire_calls {
        if call.kind != "function" || !identifier(&call.id, 200) || !seen_ids.insert(call.id) {
            return Err(failure(
                "invalid or repeated tool call ID/type; no tools dispatched",
            ));
        }
        let validator = tools.validators.get(&call.function.name).ok_or_else(|| {
            failure("model requested a tool outside this request's allowlist; no tools dispatched")
        })?;
        if call.function.arguments.len() > MAX_ARGUMENT_BYTES {
            return Err(failure("tool arguments exceed 16 KiB; no tools dispatched"));
        }
        let arguments: Value = serde_json::from_str(&call.function.arguments)
            .map_err(|_| failure("invalid tool argument JSON; no tools dispatched"))?;
        if !arguments.is_object()
            || !bounded_tree(&arguments, 0, &mut 2048)
            || !validator.is_valid(&arguments)
        {
            return Err(failure(
                "tool arguments fail schema or resource limits; no tools dispatched",
            ));
        }
        calls.push(ToolCall {
            tool_name: call.function.name,
            arguments,
            result: None,
        });
    }
    Ok((calls, seen_ids))
}

impl JiaClawAgent {
    pub(crate) async fn execute_brokerrouter_loop(
        &self,
        request: &ChatRequest,
        system_prompt: &str,
        key: &str,
        selection: &ModelSelection,
        progress: Option<&ChatProgress>,
    ) -> Result<ChatResponse, JiaClawError> {
        let allowed = self.allowed_tool_names(request)?;
        if allowed.len() > 128 {
            return Err(JiaClawError::InvalidRequest(
                "at most 128 tools may be enabled per request".into(),
            ));
        }
        let mut names: Vec<_> = allowed.iter().collect();
        names.sort();
        let mut inputs = Vec::new();
        for name in names {
            let tool = self
                .tools
                .get(name)
                .ok_or_else(|| failure("enabled tool disappeared"))?;
            inputs.push((
                name.clone(),
                tool.description().to_owned(),
                tool.parameters_schema(),
            ));
        }
        let workers = process_workers();
        let tools = Arc::new(
            workers
                .run(
                    SchemaPhase::NativeDefinitions,
                    Instant::now() + PREFLIGHT_TIMEOUT,
                    move || compile_definitions(inputs),
                )
                .await?,
        );
        let provider = BrokerrouterProvider::new(&self.config.provider.base_url, key);
        let mut messages = WireMessage::history(system_prompt, &request.messages);
        let mut records = Vec::new();
        let mut seen_ids = HashSet::new();
        let maximum = self.config.effective_max_tool_iterations();
        let turn_id = progress
            .and_then(ChatProgress::turn_id)
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
        let session_hash = request.session_id.as_ref().map(crate::model_calls::digest);
        let outcome: Result<(String, RunStatus), JiaClawError> = async {
        for iteration in 0..maximum {
            if let Some(progress) = progress { progress.ensure_open()?; }
            let message = if let Some(ledger) = &self.model_calls {
                let round = u32::try_from(iteration).map_err(|_| failure("invalid model round"))?;
                if let Some(progress) = progress {
                    let prepared = ledger.prepare_stream(&selection.model, &messages, selection.temperature,
                        selection.max_tokens, &tools.definitions)?;
                    ledger.complete_stream(prepared, turn_id.clone(), selection.purpose, session_hash.clone(), round, progress.clone()).await?
                } else {
                    let prepared = ledger.prepare(&selection.model, &messages, selection.temperature,
                        selection.max_tokens, &tools.definitions)?;
                    ledger.complete(prepared, turn_id.clone(), selection.purpose, session_hash.clone(), round).await?
                }
            } else {
                provider.complete(&selection.model, &messages, selection.temperature,
                    selection.max_tokens, &tools.definitions).await?
            };
            if let Some(progress) = progress { progress.ensure_open()?; }
            if message.tool_calls.is_empty() {
                return Ok((message.content.unwrap_or_default(), RunStatus::Completed));
            }
            let wire_calls = message.tool_calls.clone();
            let validation_tools = tools.clone();
            let previous_ids = std::mem::take(&mut seen_ids);
            let (calls, validated_ids) = workers.run(SchemaPhase::NativeBatch,
                Instant::now() + PREFLIGHT_TIMEOUT,
                move || validate_batch(wire_calls, &validation_tools, previous_ids)).await?;
            seen_ids = validated_ids;
            if iteration + 1 == maximum || records.len() + calls.len() > MAX_CALLS_PER_TURN {
                return Ok(("已达到本轮工具执行预算，剩余工具未执行；请检查已完成的调用后再继续。".into(), RunStatus::RequiresHumanInput));
            }
            let ids: Vec<_> = message.tool_calls.iter().map(|c| c.id.clone()).collect();
            messages.push(message);
            for (call, id) in calls.into_iter().zip(ids) {
                if let Some(progress) = progress { progress.ensure_open()?; }
                let (mut record, _, failure_effect) = self.execute_and_record(call).await;
                // Unknown effects stop every entry point, including ordinary
                // HTTP/CLI without a progress transport. A trusted local pure
                // tool error can still be returned to the model for correction.
                let mut needs_review = failure_effect == Some(crate::tools::ToolFailureEffect::Unknown);
                let mut result = record.result.as_ref().unwrap_or(&Value::Null).to_string();
                if result.len() > MAX_RESULT_BYTES {
                    // Lost result/error details stop dispatch independently of
                    // transport. Preserve whether execution failed, and its
                    // effect classification, instead of claiming completion.
                    needs_review = true;
                    record.result = Some(match failure_effect {
                        Some(effect) => json!({
                            "error":"tool failed but error details exceed 256 KiB; do not replay this operation",
                            "effect_status":effect,
                        }),
                        None => json!({"error":"tool completed but result exceeds 256 KiB; do not replay this operation"}),
                    });
                    result = serde_json::to_string(&record.result)
                        .expect("fixed result is serializable");
                }
                let mut reply = WireMessage::text("tool", result);
                reply.tool_call_id = Some(id.clone());
                messages.push(reply);
                records.push(record);
                if let Some(progress) = progress {
                    progress.emit(ChatProgressEvent::ToolCompleted {
                        round: u32::try_from(iteration).map_err(|_| failure("invalid model round"))?,
                        tool_call_id: id, tool_name: records.last().expect("just inserted").tool_name.clone(),
                    }).await;
                }
                if needs_review { return Err(failure("tool attempt failed or its completed result is unavailable; inspect its effect before continuing")); }
            }
        }
        Err(failure("invalid zero iteration budget"))
        }.await;
        let (content, status) = match outcome {
            Ok(result) => result,
            Err(error) if records.is_empty() => return Err(error),
            Err(error) => (
                format!("本轮已尝试执行 {} 次工具调用（{}），后续步骤停止：{error}。请核查调用结果与工作区，勿直接重试整个请求。", records.len(), records.iter().map(|record| record.tool_name.as_str()).collect::<Vec<_>>().join(", ")),
                RunStatus::RequiresHumanInput,
            ),
        };
        Ok(ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content,
            },
            tool_calls: records,
            status,
            session_id: None,
            routing: None,
        })
    }
}
