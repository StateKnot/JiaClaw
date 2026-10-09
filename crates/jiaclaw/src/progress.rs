// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bounded provisional delivery, separate from model settlement and tool authority.

use jiaclaw_core::JiaClawError;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tokio::sync::{mpsc, Notify, OwnedSemaphorePermit, Semaphore};

const STREAMS: usize = 4;
const QUEUE_EVENTS: usize = 8;
const TEXT_BYTES: usize = 1024;
const DELIVERY_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

/// Provisional text and completed-operation metadata. These events never authorize tools.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatProgressEvent {
    /// Remote identity has been persisted before any preview is delivered.
    ModelStarted {
        /// Application turn, distinct from a governed upstream turn.
        turn_id: String,
        /// Original local idempotency operation.
        operation_id: String,
        /// Canonical gateway request identity.
        remote_id: String,
        /// Zero-based model round.
        round: u32,
        /// Fixed logical routing model.
        model: String,
    },
    /// A text delta, provisional until the caller commits the final response.
    Preview {
        /// Zero-based model round; text from different rounds must not be concatenated.
        round: u32,
        /// At most 1024 UTF-8 bytes, never raw tool arguments.
        text: String,
    },
    /// The gateway settled and the local model receipt was committed.
    ModelCompleted {
        /// Original local operation.
        operation_id: String,
        /// Zero-based model round.
        round: u32,
    },
    /// A tool attempt has actually returned; this is not a recovery checkpoint.
    ToolCompleted {
        /// Zero-based model round.
        round: u32,
        /// Original provider call identity.
        tool_call_id: String,
        /// Authorized tool name.
        tool_name: String,
    },
}

struct Delivery {
    canceled: AtomicBool,
    changed: Notify,
    // Both delivery and the submitted model worker retain this ownership.
    _permit: OwnedSemaphorePermit,
}
impl Delivery {
    fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
}

/// A bounded delivery producer. Dropping/canceling delivery does not cancel submitted models.
#[derive(Clone)]
pub struct ChatProgress {
    sender: mpsc::Sender<ChatProgressEvent>,
    delivery: Arc<Delivery>,
}

/// The consumer owns delivery until it is dropped. No automatic reconnect or replay.
pub struct ChatEvents {
    receiver: mpsc::Receiver<ChatProgressEvent>,
    delivery: Arc<Delivery>,
}

impl ChatProgress {
    /// Admit one of four process-wide streams before any model or persistence effects.
    /// Each queue has eight finite events (preview strings are at most 1024 bytes).
    ///
    /// # Errors
    /// All four live delivery/settlement owners are occupied.
    pub fn channel() -> Result<(Self, ChatEvents), JiaClawError> {
        static OWNERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let owners = OWNERS.get_or_init(|| Arc::new(Semaphore::new(STREAMS)));
        Self::with_owners(owners)
    }

    fn with_owners(owners: &Arc<Semaphore>) -> Result<(Self, ChatEvents), JiaClawError> {
        let permit = Arc::clone(owners).try_acquire_owned().map_err(|_| {
            JiaClawError::InvalidRequest("stream_busy: four streams already active".into())
        })?;
        let (sender, receiver) = mpsc::channel(QUEUE_EVENTS);
        let delivery = Arc::new(Delivery {
            canceled: AtomicBool::new(false),
            changed: Notify::new(),
            _permit: permit,
        });
        Ok((
            Self {
                sender,
                delivery: Arc::clone(&delivery),
            },
            ChatEvents { receiver, delivery },
        ))
    }

    /// Stop future model/tool dispatch and delivery; an already submitted model still settles.
    pub fn cancel(&self) {
        self.delivery.cancel();
    }

    pub(crate) fn ensure_open(&self) -> Result<(), JiaClawError> {
        if self.delivery.canceled.load(Ordering::Acquire) || self.sender.is_closed() {
            return Err(JiaClawError::InvalidRequest(
                "stream_delivery_closed: current submitted model may still settle; no replay"
                    .into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn emit(&self, event: ChatProgressEvent) {
        if self.ensure_open().is_err() {
            return;
        }
        if !matches!(
            tokio::time::timeout(DELIVERY_WAIT, self.sender.send(event)).await,
            Ok(Ok(()))
        ) {
            self.cancel();
        }
    }

    pub(crate) async fn preview(&self, round: u32, mut text: &str) {
        while !text.is_empty() && self.ensure_open().is_ok() {
            let mut end = text.len().min(TEXT_BYTES);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.emit(ChatProgressEvent::Preview {
                round,
                text: text[..end].to_owned(),
            })
            .await;
            text = &text[end..];
        }
    }
}

impl ChatEvents {
    /// Drain events already queued after the turn future has returned.
    pub fn try_next(&mut self) -> Option<ChatProgressEvent> {
        if self.delivery.canceled.load(Ordering::Acquire) {
            return None;
        }
        self.receiver.try_recv().ok()
    }
    /// Receive one provisional event. Cancellation closes delivery even with queued previews.
    pub async fn next(&mut self) -> Option<ChatProgressEvent> {
        let changed = self.delivery.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if self.delivery.canceled.load(Ordering::Acquire) {
            return None;
        }
        tokio::select! {
            () = &mut changed => None,
            event = self.receiver.recv() => event,
        }
    }
}
impl Drop for ChatEvents {
    fn drop(&mut self) {
        self.delivery.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slow_delivery_closes_but_owner_survives_until_producer_settles() {
        let owners = Arc::new(Semaphore::new(1));
        let (progress, mut events) = ChatProgress::with_owners(&owners).unwrap();
        for _ in 0..QUEUE_EVENTS + 1 {
            progress.preview(0, "x").await;
        }
        assert!(events.next().await.is_none());
        drop(events);
        assert!(ChatProgress::with_owners(&owners).is_err());
        drop(progress);
        assert!(ChatProgress::with_owners(&owners).is_ok());
    }

    #[tokio::test]
    async fn unicode_fragments_are_bounded_and_cancellation_wakes_waiter() {
        let owners = Arc::new(Semaphore::new(1));
        let (progress, mut events) = ChatProgress::with_owners(&owners).unwrap();
        let text = "🦀".repeat(600);
        progress.preview(1, &text).await;
        let mut received = String::new();
        while received.len() < text.len() {
            let ChatProgressEvent::Preview { round, text } = events.next().await.unwrap() else {
                panic!()
            };
            assert_eq!(round, 1);
            assert!(text.len() <= TEXT_BYTES);
            received.push_str(&text);
        }
        assert_eq!(received, text);
        let waiter = tokio::spawn(async move { events.next().await });
        progress.cancel();
        assert!(waiter.await.unwrap().is_none());
    }
}
