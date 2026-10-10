// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One bounded HTTP transport consumer, independent of submitted model settlement.

use super::{ChatEvents, ChatProgress, Owner, Receipt};
use anyhow::{ensure, Context, Result};
use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jiaclaw::ChatProgressEvent;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

pub(crate) const ROUND_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const PREVIEW_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const WIRE_BYTES: usize = 12 * 1024 * 1024;
const FRAGMENT: usize = 4096;
const WRITE_GRACE: Duration = Duration::from_secs(5);
const TERMINAL_GRACE: Duration = Duration::from_secs(65);
const ERROR_RESERVE: usize = 1024;

struct Consumer {
    receiver: mpsc::Receiver<Bytes>,
    // Actual HTTP body lifetime retains one of the four process delivery slots.
    // A blocked socket is not proof that Hyper has dropped its body.
    progress: ChatProgress,
}
impl Drop for Consumer {
    fn drop(&mut self) {
        self.progress.cancel();
    }
}

struct Output {
    sender: mpsc::Sender<Bytes>,
    wire_bytes: usize,
    preview_bytes: usize,
    round: Option<(u32, usize)>,
    stop_at: Instant,
    incomplete: bool,
}
impl Output {
    async fn bytes(&mut self, bytes: Vec<u8>, reserve: usize) -> Result<()> {
        ensure!(
            self.wire_bytes + bytes.len() <= WIRE_BYTES - reserve,
            "HTTP wire budget"
        );
        self.wire_bytes += bytes.len();
        self.incomplete = true;
        // One original deadline for the complete frame; fragments do not extend it.
        let deadline = (Instant::now() + WRITE_GRACE).min(self.stop_at);
        for fragment in bytes.chunks(FRAGMENT) {
            tokio::time::timeout_at(deadline, self.sender.send(Bytes::copy_from_slice(fragment)))
                .await
                .context("HTTP delivery backpressure")?
                .map_err(|_| anyhow::anyhow!("HTTP consumer closed"))?;
        }
        self.incomplete = false;
        Ok(())
    }
    async fn json(&mut self, name: &str, value: Value, maximum: usize) -> Result<()> {
        let data = serde_json::to_vec(&value)?;
        ensure!(data.len() <= maximum, "HTTP event budget");
        let mut bytes = format!("event: {name}\ndata: ").into_bytes();
        bytes.extend(data);
        bytes.extend_from_slice(b"\n\n");
        self.bytes(bytes, ERROR_RESERVE).await
    }
    async fn progress(&mut self, event: ChatProgressEvent) -> Result<()> {
        if let ChatProgressEvent::Preview { round, text } = &event {
            ensure!(text.len() <= 1024, "HTTP preview fragment budget");
            if self.round.is_none_or(|(previous, _)| previous != *round) {
                ensure!(
                    self.round.is_none_or(|(previous, _)| previous < *round),
                    "HTTP preview round order"
                );
                self.round = Some((*round, 0));
            }
            let bytes = &mut self.round.as_mut().expect("preview round").1;
            *bytes += text.len();
            self.preview_bytes += text.len();
            ensure!(
                *bytes <= ROUND_BYTES && self.preview_bytes <= PREVIEW_BYTES,
                "HTTP preview budget"
            );
        }
        let value = serde_json::to_value(event)?;
        let name = value["event"]
            .as_str()
            .context("HTTP progress event")?
            .to_owned();
        self.json(&name, value, 8192).await
    }
    fn error(&mut self) {
        // Never append an error into a partially queued JSON/SSE frame. EOF
        // without a complete done event requires lookup of the original UUID.
        if self.incomplete {
            return;
        }
        const ERROR: &[u8] = b"event: error\ndata: {\"event\":\"error\",\"code\":\"http_stream_requires_review\",\"message\":\"GET the original request identity; no automatic replay\"}\n\n";
        if self.wire_bytes + ERROR.len() <= WIRE_BYTES {
            // Diagnostics never start a second grace after a failed write.
            let _ = self.sender.try_send(Bytes::from_static(ERROR));
        }
    }
}

pub(super) fn response(
    receipt: Receipt,
    events: ChatEvents,
    progress: ChatProgress,
    terminal: oneshot::Receiver<std::result::Result<Receipt, ()>>,
    owner: Arc<Owner>,
    deadline: Instant,
) -> Response {
    // At most eight <=4KiB transport fragments. This is the sole consumer of
    // the existing eight-event progress queue; it does not start another turn.
    let (sender, receiver) = mpsc::channel(8);
    let consumer = Consumer {
        receiver,
        progress: progress.clone(),
    };
    let id = receipt.id.clone();
    tokio::spawn(async move {
        let _owner = owner;
        let result = drive(
            receipt,
            events,
            progress.clone(),
            terminal,
            sender,
            deadline,
        )
        .await;
        if let Err(error) = result {
            progress.cancel();
            tracing::warn!(request_id=%id,error=%error,"HTTP event delivery stopped; GET original identity; no replay");
        }
    });
    let body = Body::from_stream(futures_util::stream::unfold(
        consumer,
        |mut consumer| async {
            consumer
                .receiver
                .recv()
                .await
                .map(|bytes| (Ok::<_, std::convert::Infallible>(bytes), consumer))
        },
    ));
    (
        StatusCode::ACCEPTED,
        [
            ("content-type", "text/event-stream; charset=utf-8"),
            ("cache-control", "no-store"),
            ("x-accel-buffering", "no"),
        ],
        body,
    )
        .into_response()
}

async fn drive(
    receipt: Receipt,
    mut events: ChatEvents,
    progress: ChatProgress,
    mut terminal: oneshot::Receiver<std::result::Result<Receipt, ()>>,
    sender: mpsc::Sender<Bytes>,
    deadline: Instant,
) -> Result<()> {
    let stop_at = deadline + TERMINAL_GRACE;
    let mut output = Output {
        sender,
        wire_bytes: 0,
        preview_bytes: 0,
        round: None,
        stop_at,
        incomplete: false,
    };
    let result = async {
        output.json("admitted",json!({"event":"admitted","protocol":1,"receipt":receipt}),16384).await?;
        let mut receiving = true;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                biased;
                ()=output.sender.closed()=>anyhow::bail!("HTTP consumer closed"),
                ()=tokio::time::sleep_until(stop_at)=>anyhow::bail!("HTTP terminal observation deadline"),
                result=&mut terminal=>{
                    let receipt = result.context("HTTP turn owner failed")?
                        .map_err(|()|anyhow::anyhow!("HTTP terminal transaction failed"))?;
                    while let Some(event)=events.try_next() { output.progress(event).await?; }
                    output.json("done",json!({"event":"done","protocol":1,"receipt":receipt}),super::MAX_RESULT_BYTES+16384).await?;
                    return Ok(());
                },
                event=events.next(),if receiving=>{
                    if let Some(event)=event { output.progress(event).await?; } else { receiving=false; }
                },
                _=heartbeat.tick()=>output.bytes(b": keepalive\n\n".to_vec(),ERROR_RESERVE).await?,
            }
        }
    }.await;
    if result.is_err() {
        progress.cancel();
        output.error();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::Runtime;
    use super::*;
    use jiaclaw_core::{ChatMessage, MessageRole};

    fn admitted() -> (
        super::super::super::SessionStore,
        Receipt,
        Arc<Owner>,
        ChatProgress,
        ChatEvents,
    ) {
        let mut store =
            super::super::super::SessionStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let receipt = store
            .admit_http_turn(
                &id,
                &format!("http:{}", uuid::Uuid::new_v4()),
                &"a".repeat(64),
                &"b".repeat(64),
            )
            .unwrap()
            .0;
        let runtime = Runtime::new(Duration::from_secs(300));
        let permit = runtime.owners.clone().try_acquire_owned().unwrap();
        let (progress, events) = ChatProgress::channel_for_turn(&id).unwrap();
        runtime.register(id.clone(), progress.clone());
        let owner = Arc::new(Owner {
            runtime,
            id,
            _permit: permit,
        });
        (store, receipt, owner, progress, events)
    }

    #[tokio::test]
    async fn dropping_the_unpolled_real_body_cancels_without_releasing_execution_owner() {
        let (_store, receipt, execution, progress, events) = admitted();
        let (terminal, _keep_sender) = {
            let (tx, rx) = oneshot::channel();
            (rx, tx)
        };
        let body = response(
            receipt,
            events,
            progress.clone(),
            terminal,
            execution.clone(),
            Instant::now() + Duration::from_secs(300),
        );
        assert!(!progress.is_cancelled());
        drop(body);
        assert!(
            progress.is_cancelled(),
            "the actual Body drop must cancel synchronously"
        );
        assert!(execution.runtime.is_active(&execution.id));
        assert_eq!(execution.runtime.owners.available_permits(), 0);
        tokio::task::yield_now().await;
        assert!(
            execution.runtime.is_active(&execution.id),
            "submitted work retains its own owner"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unpolled_large_terminal_frame_expires_without_releasing_actual_execution_or_fabricating_done(
    ) {
        let (mut store, receipt, execution, progress, events) = admitted();
        let (tx, rx) = oneshot::channel();
        let body = response(
            receipt.clone(),
            events,
            progress.clone(),
            rx,
            execution.clone(),
            Instant::now() + Duration::from_secs(300),
        );
        let text = "x".repeat(1024 * 1024);
        let committed = store
            .finish_http_turn(
                &receipt.id,
                Some(vec![ChatMessage {
                    role: MessageRole::Assistant,
                    content: text.clone(),
                }]),
                Some(json!({"reply":text})),
                None,
                false,
            )
            .unwrap();
        tx.send(Ok(committed)).unwrap();
        tokio::task::yield_now().await; // The real bounded queue is now full.
        assert!(!progress.is_cancelled());
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(
            progress.is_cancelled(),
            "one whole-frame deadline must stop the writer"
        );
        assert_eq!(execution.runtime.owners.available_permits(), 0);
        let runtime = execution.runtime.clone();
        drop(execution);
        assert_eq!(
            runtime.owners.available_permits(),
            1,
            "only the actual execution owner may release execution capacity"
        );
        assert!(
            store
                .http_receipt(&receipt.id, None)
                .unwrap()
                .unwrap()
                .session_committed
        );
        // A failed partial frame cannot be followed by a fabricated error/done.
        use futures_util::StreamExt;
        let mut stream = body.into_body().into_data_stream();
        let mut bytes = Vec::new();
        while let Some(fragment) = stream.next().await {
            bytes.extend_from_slice(&fragment.unwrap());
        }
        assert!(bytes.len() <= 8 * FRAGMENT);
        assert!(
            !bytes.ends_with(b"\n\n"),
            "terminal JSON is incomplete, requiring original lookup"
        );
        assert!(!bytes
            .windows(b"event: error".len())
            .any(|w| w == b"event: error"));
    }
}
