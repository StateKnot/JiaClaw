// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Bounded tenant delivery. Transport loss never releases a submitted execution.
use super::*;
use crate::http_turns::delivery::{PREVIEW_BYTES, ROUND_BYTES, WIRE_BYTES};
use axum::body::{Body, Bytes};
use serde_json::Value;
use tokio::sync::{mpsc, Semaphore};

const FRAGMENT: usize = 4096;
const FRAME: usize = MAX_RECEIPT + 64;
const ERROR: &[u8] = b"event: error\ndata: {\"event\":\"error\",\"code\":\"http_stream_requires_review\",\"message\":\"GET the original request identity; no automatic replay\"}\n\n";

pub(super) struct Delivery {
    _global: OwnedSemaphorePermit,
    _backend: OwnedSemaphorePermit,
}
impl Delivery {
    pub(super) fn admit(global: &Arc<Semaphore>, backend: &Arc<Semaphore>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            _global: global.clone().try_acquire_owned()?,
            _backend: backend.clone().try_acquire_owned()?,
        }))
    }
}
struct Consumer {
    receiver: mpsc::Receiver<Bytes>,
    _owner: Arc<Delivery>,
}
struct Output {
    sender: mpsc::Sender<Bytes>,
    wire: usize,
    incomplete: bool,
    deadline: Instant,
}
impl Output {
    fn response(owner: Arc<Delivery>, deadline: Instant, request_id: Uuid) -> (Self, Response) {
        let (sender, receiver) = mpsc::channel(8);
        let consumer = Consumer {
            receiver,
            _owner: owner,
        };
        let body = Body::from_stream(futures_util::stream::unfold(consumer, |mut c| async {
            c.receiver
                .recv()
                .await
                .map(|bytes| (Ok::<_, std::convert::Infallible>(bytes), c))
        }));
        let r = response(
            (
                StatusCode::ACCEPTED,
                [
                    ("content-type", "text/event-stream; charset=utf-8"),
                    ("x-accel-buffering", "no"),
                ],
                body,
            ),
            request_id,
        );
        (
            Self {
                sender,
                wire: 0,
                incomplete: false,
                deadline,
            },
            r,
        )
    }
    async fn frame(&mut self, name: &str, value: &Value) -> Result<()> {
        let mut bytes = format!("event: {name}\ndata: ").into_bytes();
        bytes.extend(serde_json::to_vec(value)?);
        bytes.extend_from_slice(b"\n\n");
        self.bytes(&bytes).await
    }
    async fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() <= FRAME && self.wire + bytes.len() <= WIRE_BYTES - ERROR.len(),
            "tenant delivery budget"
        );
        self.wire += bytes.len();
        self.incomplete = true;
        // One deadline for the whole frame, including a large terminal receipt.
        let stop = self.deadline.min(Instant::now() + IO_BUDGET);
        for fragment in bytes.chunks(FRAGMENT) {
            tokio::time::timeout_at(stop, self.sender.send(Bytes::copy_from_slice(fragment)))
                .await??;
        }
        self.incomplete = false;
        Ok(())
    }
    fn error(&self) {
        // Never append diagnostics into a partially emitted frame.
        if !self.incomplete && self.wire + ERROR.len() <= WIRE_BYTES {
            let _ = self.sender.try_send(Bytes::from_static(ERROR));
        }
    }
}

struct Decoder {
    frame: Vec<u8>,
    wire: usize,
    admitted: Option<Receipt>,
    round: i64,
    operation: Option<String>,
    model_done: bool,
    round_bytes: usize,
    preview_bytes: usize,
    terminal: bool,
}
impl Decoder {
    fn new() -> Self {
        Self {
            frame: Vec::new(),
            wire: 0,
            admitted: None,
            round: -1,
            operation: None,
            model_done: false,
            round_bytes: 0,
            preview_bytes: 0,
            terminal: false,
        }
    }
    fn byte(
        &mut self,
        byte: u8,
        id: Uuid,
        hash: &str,
        body: &Submission,
    ) -> Result<Option<(String, Value)>> {
        self.wire += 1;
        ensure!(
            !self.terminal && self.wire <= WIRE_BYTES && self.frame.len() < FRAME,
            "tenant stream budget or trailing event"
        );
        self.frame.push(byte);
        if !self.frame.ends_with(b"\n\n") {
            return Ok(None);
        }
        let frame = std::mem::take(&mut self.frame);
        let text = std::str::from_utf8(&frame[..frame.len() - 2])?;
        if text == ": keepalive" {
            ensure!(self.admitted.is_some(), "tenant heartbeat before admission");
            return Ok(Some(("keepalive".into(), Value::Null)));
        }
        let (name, data) = text
            .strip_prefix("event: ")
            .and_then(|s| s.split_once("\ndata: "))
            .ok_or_else(|| anyhow::anyhow!("invalid tenant event framing"))?;
        ensure!(!data.contains(['\r', '\n']), "invalid tenant event lines");
        let value: Value = serde_json::from_str(data)?;
        ensure!(
            value.is_object() && value["event"] == name,
            "invalid tenant event name"
        );
        if matches!(name, "admitted" | "done") {
            ensure!(
                fields(&value, &["event", "protocol", "receipt"]) && value["protocol"] == 1,
                "invalid tenant receipt event"
            );
            let receipt: Receipt = serde_json::from_value(value["receipt"].clone())?;
            let envelope = Envelope {
                protocol: 1,
                receipt: receipt.clone(),
                active: name == "admitted",
            };
            envelope.validate(id, hash, &body.session_id)?;
            selected_result(&receipt, body)?;
            if let Some(original) = &self.admitted {
                ensure!(
                    name == "done"
                        && receipt.state != "running"
                        && same_context(original, &receipt),
                    "changed tenant receipt identity"
                );
                self.terminal = true;
            } else {
                ensure!(
                    name == "admitted" && data.len() <= 16384 && receipt.state == "running",
                    "tenant admission missing"
                );
                self.admitted = Some(receipt);
            }
        } else {
            ensure!(
                self.admitted.is_some() && data.len() <= 8192,
                "tenant progress before admission or too large"
            );
            match name {
                "model_started" => {
                    ensure!(
                        fields(
                            &value,
                            &[
                                "event",
                                "turn_id",
                                "operation_id",
                                "remote_id",
                                "round",
                                "model"
                            ]
                        ) && value["turn_id"] == id.to_string()
                            && valid_uuid(&value["operation_id"])
                            && valid_uuid(&value["remote_id"])
                            && value["round"].as_i64() == Some(self.round + 1)
                            && self.round < 32
                            && (self.round == -1 || self.model_done)
                            && bounded_text(&value["model"], 1024),
                        "invalid tenant model identity or order"
                    );
                    self.round += 1;
                    self.operation = value["operation_id"].as_str().map(str::to_owned);
                    self.model_done = false;
                    self.round_bytes = 0;
                }
                "preview" => {
                    ensure!(
                        fields(&value, &["event", "round", "text"])
                            && value["round"].as_i64() == Some(self.round)
                            && self.round >= 0
                            && !self.model_done
                            && bounded_text(&value["text"], 1024),
                        "invalid tenant preview"
                    );
                    let bytes = value["text"].as_str().expect("validated text").len();
                    self.round_bytes += bytes;
                    self.preview_bytes += bytes;
                    ensure!(
                        self.round_bytes <= ROUND_BYTES && self.preview_bytes <= PREVIEW_BYTES,
                        "tenant preview budget"
                    );
                }
                "model_completed" => {
                    ensure!(
                        fields(&value, &["event", "operation_id", "round"])
                            && value["round"].as_i64() == Some(self.round)
                            && self.round >= 0
                            && !self.model_done
                            && value["operation_id"].as_str() == self.operation.as_deref(),
                        "invalid tenant model completion"
                    );
                    self.model_done = true;
                }
                "tool_completed" => ensure!(
                    fields(&value, &["event", "round", "tool_call_id", "tool_name"])
                        && value["round"].as_i64() == Some(self.round)
                        && self.round >= 0
                        && self.model_done
                        && bounded_text(&value["tool_call_id"], 1024)
                        && value["tool_name"]
                            .as_str()
                            .is_some_and(|s| body.enabled_tools.iter().any(|t| t == s)),
                    "unauthorized tenant tool completion"
                ),
                "error" => {
                    anyhow::bail!("native tenant delivery stopped; original lookup required")
                }
                _ => anyhow::bail!("unknown tenant progress event"),
            }
        }
        Ok(Some((name.to_owned(), value)))
    }
}
fn fields(value: &Value, names: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|v| v.len() == names.len() && names.iter().all(|n| v.contains_key(*n)))
}
fn bounded_text(value: &Value, limit: usize) -> bool {
    value.as_str().is_some_and(|s| s.len() <= limit)
}
fn valid_uuid(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s))
}
fn same_context(a: &Receipt, b: &Receipt) -> bool {
    a.context_hash == b.context_hash && a.created_ms == b.created_ms
}
fn selected_result(receipt: &Receipt, body: &Submission) -> Result<()> {
    if let Some(result) = &receipt.result {
        ensure!(
            fields(result, &["reply", "status", "routing", "tool_names"])
                && bounded_text(&result["reply"], ROUND_BYTES)
                && bounded_text(&result["status"], 64)
                && result["tool_names"]
                    .as_array()
                    .is_some_and(|names| names.len() <= 2048
                        && names.iter().all(|name| name
                            .as_str()
                            .is_some_and(|s| body.enabled_tools.iter().any(|t| t == s)))),
            "invalid tenant final result or tool authority"
        );
    }
    ensure!(
        receipt.state != "completed"
            || receipt.result_purged
            || receipt
                .result
                .as_ref()
                .is_some_and(|r| r["status"] == "completed"),
        "completed tenant result missing"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    state: Arc<State>,
    p: Principal,
    id: Uuid,
    body: Submission,
    hash: String,
    request_id: Uuid,
    deadline: Instant,
    sender: tokio::sync::oneshot::Sender<Response>,
    delivery: Arc<Delivery>,
) -> bool {
    if Instant::now() >= deadline {
        let registry = state.registry.clone();
        let _ =
            tokio::task::spawn_blocking(move || registry.finish_write(p.user_id, id, false)).await;
        let _ = sender.send(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "admission deadline exceeded; review required",
            request_id,
        ));
        return true; // No backend request was constructed or submitted.
    }
    let backend = &state.backends[&p.backend_id];
    let submitted = tokio::time::timeout_at(deadline.min(Instant::now() + IO_BUDGET), async {
        state
            .client
            .put(backend.url.join(&format!("api/turns/{id}/stream"))?)
            .header(header::AUTHORIZATION, backend.token.clone())
            .header("x-jiaclaw-gateway-turns", "1")
            .timeout(deadline.saturating_duration_since(Instant::now()))
            .json(&body)
            .send()
            .await
            .map_err(anyhow::Error::from)
    })
    .await;
    let mut decoder = Decoder::new();
    let mut output = None;
    let mut reply = Some(sender);
    let mut stream_ok = false;
    let result: Result<()> = async {
        let mut native = submitted.map_err(|_| anyhow::anyhow!("tenant stream headers timed out"))??;
        if native.status() == StatusCode::OK && json_content(native.headers()) {
            // A backend identity may predate this gateway's mapping. Lookup only;
            // never ask it to create a second delivery or replay an execution.
            let bytes = tokio::time::timeout_at(deadline.min(Instant::now()+IO_BUDGET), super::super::proxy::read_response(native, MAX_RECEIPT)).await?.map_err(|()|anyhow::anyhow!("invalid tenant receipt"))?;
            let envelope: Envelope = serde_json::from_slice(&bytes)?;
            envelope.validate(id, &hash, &body.session_id)?;
            selected_result(&envelope.receipt, &body)?;
            let _ = reply.take().expect("single reply").send(response((StatusCode::OK, Json(envelope)), request_id));
            return Ok(());
        }
        ensure!(native.status() == StatusCode::ACCEPTED && native.headers().get_all(header::CONTENT_TYPE).iter().count() == 1 && native.headers()[header::CONTENT_TYPE] == "text/event-stream; charset=utf-8", "invalid tenant stream response");
        let (writer, r) = Output::response(delivery.clone(), deadline, request_id);
        output = Some(writer);
        let _ = reply.take().expect("single reply").send(r);
        let writer = output.as_mut().expect("stream writer");
        while !decoder.terminal {
            let chunk = tokio::select! {
                biased;
                () = writer.sender.closed() => anyhow::bail!("tenant consumer closed"),
                () = tokio::time::sleep_until(deadline) => anyhow::bail!("tenant observation deadline"),
                chunk = native.chunk() => chunk?.ok_or_else(||anyhow::anyhow!("tenant stream ended without terminal receipt"))?,
            };
            for byte in chunk {
                if let Some((name, value)) = decoder.byte(byte, id, &hash, &body)? {
                    if name == "keepalive" { writer.bytes(b": keepalive\n\n").await?; } else if name != "done" { writer.frame(&name, &value).await?; }
                }
            }
        }
        stream_ok = true;
        Ok(())
    }.await;
    let response_valid = result.is_ok();
    if !response_valid {
        tracing::warn!(request_id=%id, "tenant SSE delivery stopped; reconciling only the original identity");
        // Dropping native HTTP delivery stops future dispatch. Persist one cancel
        // intent when the original deadline still permits it; current model owners
        // independently settle against their recorded remote identity.
        let _ = tokio::time::timeout_at(
            deadline,
            remote(
                &state,
                &p,
                id,
                &hash,
                &body.session_id,
                Method::POST,
                None,
                true,
            ),
        )
        .await;
        if let Some(reply) = reply.take() {
            let _ = reply.send(error(
                StatusCode::CONFLICT,
                "original outcome unknown; review required",
                request_id,
            ));
        }
    }
    let mut idle = false;
    let mut success = false;
    let mut terminal = None;
    while Instant::now() < deadline {
        match tokio::time::timeout_at(
            deadline,
            remote(
                &state,
                &p,
                id,
                &hash,
                &body.session_id,
                Method::GET,
                None,
                false,
            ),
        )
        .await
        {
            Ok(Ok((_, envelope))) => {
                if selected_result(&envelope.receipt, &body).is_err() {
                    break;
                }
                if decoder
                    .admitted
                    .as_ref()
                    .is_some_and(|a| !same_context(a, &envelope.receipt))
                {
                    break;
                }
                if !envelope.active && envelope.receipt.state != "running" {
                    idle = true;
                    // Idleness permits resource release; a later successful GET
                    // cannot erase a rejected original delivery contract.
                    success = response_valid && envelope.known_success();
                    terminal = Some(envelope.receipt);
                    break;
                }
            }
            _ => break,
        }
        if tokio::time::timeout_at(deadline, tokio::time::sleep(Duration::from_millis(100)))
            .await
            .is_err()
        {
            break;
        }
    }
    let registry = state.registry.clone();
    let settled = matches!(
        tokio::task::spawn_blocking(move || registry.finish_write(p.user_id, id, success)).await,
        Ok(Ok(()))
    );
    if let Some(mut writer) = output {
        if stream_ok && idle && settled {
            if writer.frame("done", &json!({"event":"done","protocol":1,"receipt":terminal.expect("idle terminal")})).await.is_err() { writer.error(); }
        } else {
            writer.error();
        }
    }
    idle
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Uuid, Submission, Receipt) {
        let id = Uuid::new_v4();
        let body: Submission = serde_json::from_value(json!({"session_id":format!("http:{}",Uuid::new_v4()),"prompt":"test","enabled_tools":["datetime_now"]})).unwrap();
        let receipt: Receipt = serde_json::from_value(json!({"id":id.to_string(),"session_id":body.session_id,"request_hash":body.fingerprint().unwrap(),"context_hash":"b".repeat(64),"created_ms":1,"finished_ms":null,"state":"running","session_committed":false,"error":null,"result":null,"result_purged":false,"cancel_requested":false,"reviewed_ms":null,"review_note":null})).unwrap();
        (id, body, receipt)
    }
    fn feed(decoder: &mut Decoder, value: Value, id: Uuid, body: &Submission) -> Result<()> {
        let wire = format!(
            "event: {}\ndata: {}\n\n",
            value["event"].as_str().unwrap(),
            value
        );
        let hash = body.fingerprint().unwrap();
        for byte in wire.bytes() {
            decoder.byte(byte, id, &hash, body)?;
        }
        Ok(())
    }
    #[tokio::test]
    async fn actual_unpolled_body_retains_both_delivery_slots_after_actor_completion() {
        let global = Arc::new(Semaphore::new(1));
        let backend = Arc::new(Semaphore::new(1));
        let owner = Delivery::admit(&global, &backend).unwrap();
        let (output, response) =
            Output::response(owner.clone(), Instant::now() + IO_BUDGET, Uuid::new_v4());
        drop(output);
        drop(owner);
        assert_eq!(global.available_permits(), 0);
        assert_eq!(backend.available_permits(), 0);
        assert!(Delivery::admit(&global, &backend).is_err());
        drop(response);
        assert!(Delivery::admit(&global, &backend).is_ok());
    }
    #[tokio::test]
    async fn saturated_fragment_queue_keeps_original_frame_deadline_and_no_malformed_error() {
        let owner =
            Delivery::admit(&Arc::new(Semaphore::new(1)), &Arc::new(Semaphore::new(1))).unwrap();
        let deadline = Instant::now() + Duration::from_millis(30);
        let (mut writer, response) = Output::response(owner, deadline, Uuid::new_v4());
        assert!(writer
            .frame("done", &json!({"reply":"x".repeat(128*1024)}))
            .await
            .is_err());
        assert!(writer.incomplete);
        assert!(Instant::now() >= deadline);
        writer.error();
        drop(writer);
        let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap();
        assert!(!bytes.windows(12).any(|s| s == b"event: error"));
        assert_eq!(bytes.len(), 8 * FRAGMENT);
        assert!(!bytes.ends_with(b"\n\n"));
    }
    #[test]
    fn decoded_events_enforce_original_identity_order_tools_and_fixed_preview_budget() {
        let (id, body, receipt) = fixture();
        let admitted = json!({"event":"admitted","protocol":1,"receipt":receipt});
        let started = json!({"event":"model_started","turn_id":id.to_string(),"operation_id":id.to_string(),"remote_id":id.to_string(),"round":0,"model":"fixture"});
        let preview = json!({"event":"preview","round":0,"text":"分割🦀UTF-8"});
        let mut decoder = Decoder::new();
        assert!(feed(&mut decoder, preview.clone(), id, &body).is_err());
        let mut decoder = Decoder::new();
        feed(&mut decoder, admitted.clone(), id, &body).unwrap();
        let mut wrong = started.clone();
        wrong["turn_id"] = json!(Uuid::new_v4().to_string());
        assert!(feed(&mut decoder, wrong, id, &body).is_err());
        let mut decoder = Decoder::new();
        feed(&mut decoder, admitted.clone(), id, &body).unwrap();
        feed(&mut decoder, started.clone(), id, &body).unwrap();
        feed(&mut decoder, preview.clone(), id, &body).unwrap();
        decoder.round_bytes = ROUND_BYTES;
        assert!(feed(&mut decoder, preview, id, &body).is_err());
        let mut decoder = Decoder::new();
        feed(&mut decoder, admitted, id, &body).unwrap();
        feed(&mut decoder, started, id, &body).unwrap();
        feed(
            &mut decoder,
            json!({"event":"model_completed","round":0,"operation_id":id.to_string()}),
            id,
            &body,
        )
        .unwrap();
        assert!(feed(
            &mut decoder,
            json!({"event":"tool_completed","round":0,"tool_call_id":"a","tool_name":"file_write"}),
            id,
            &body
        )
        .is_err());
        let mut done = receipt;
        done.state = "completed".into();
        done.finished_ms = Some(2);
        done.session_committed = true;
        done.result = Some(
            json!({"reply":"done","status":"completed","routing":null,"tool_names":["file_write"]}),
        );
        assert!(selected_result(&done, &body).is_err());
        done.result = Some(
            json!({"reply":"done","status":"completed","routing":null,"tool_names":["datetime_now"]}),
        );
        assert!(selected_result(&done, &body).is_ok());
        done.context_hash = "c".repeat(64);
        assert!(feed(
            &mut decoder,
            json!({"event":"done","protocol":1,"receipt":done}),
            id,
            &body
        )
        .is_err());
    }
}
