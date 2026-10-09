// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicit single-turn JSON-lines consumer; HTTP and tenant streaming are separate contracts.

use super::{
    open_configured_session_store, shutdown_signal, ChatMessage, ChatRequest, JiaClawAgent,
    MessageRole, ModelPurpose, SessionRecord,
};
use anyhow::{Context, Result};
use jiaclaw::ChatProgress;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const WRITE_GRACE: Duration = Duration::from_secs(5);
const STOP_GRACE: Duration = Duration::from_secs(65);
const OUTPUT_FRAGMENT: usize = 4096;

pub(super) fn validate_stdout() -> Result<()> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::fstat(std::io::stdout()).context("stream stdout is unavailable")?;
        anyhow::ensure!(matches!(rustix::fs::FileType::from_raw_mode(stat.st_mode), rustix::fs::FileType::Fifo | rustix::fs::FileType::RegularFile),
            "stream emits JSON lines to an exclusively owned pipe/file; for terminal display use '| cat'");
        Ok(())
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("bounded CLI streaming stdout requires a Unix pipe/file");
    }
}

struct Fragment {
    bytes: Vec<u8>,
    deadline: Instant,
}
struct Output {
    sender: mpsc::Sender<Fragment>,
    worker: tokio::task::JoinHandle<Result<()>>,
}
impl Output {
    fn start(progress: ChatProgress) -> Self {
        // Exactly one writer, eight <=4KiB fragments, no blocking executor queue per event.
        let (sender, mut receiver) = mpsc::channel::<Fragment>(8);
        let worker = tokio::task::spawn_blocking(move || {
            let result = (|| {
                while let Some(fragment) = receiver.blocking_recv() {
                    write_fragment(&fragment)?;
                }
                Ok(())
            })();
            if result.is_err() {
                progress.cancel();
            }
            result
        });
        Self { sender, worker }
    }
    async fn line(&self, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        // A final reply is bounded by the canonical 2MiB model receipt; tool results are omitted.
        anyhow::ensure!(
            bytes.len() <= 2 * 1024 * 1024 + 4096,
            "stream final output exceeds receipt budget"
        );
        bytes.push(b'\n');
        let deadline = Instant::now() + WRITE_GRACE;
        for chunk in bytes.chunks(OUTPUT_FRAGMENT) {
            tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                self.sender.send(Fragment {
                    bytes: chunk.to_vec(),
                    deadline,
                }),
            )
            .await
            .context("stream stdout backpressure deadline")?
            .map_err(|_| anyhow::anyhow!("stream stdout closed; no replay"))?;
        }
        Ok(())
    }
    async fn finish(self) -> Result<()> {
        drop(self.sender);
        self.worker.await.context("stream stdout worker failed")?
    }
}

#[cfg(unix)]
fn write_fragment(fragment: &Fragment) -> Result<()> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    let stdout = std::io::stdout();
    let mut bytes = fragment.bytes.as_slice();
    while !bytes.is_empty() {
        let remaining = fragment
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .context("stream stdout backpressure deadline")?;
        let timeout = Timespec::try_from(remaining).context("stream output timeout")?;
        let mut fds = [PollFd::new(&stdout, PollFlags::OUT)];
        match poll(&mut fds, Some(&timeout)) {
            Err(rustix::io::Errno::INTR) => continue,
            result => {
                result.context("stream stdout readiness failed")?;
            }
        }
        anyhow::ensure!(
            fds[0].revents().contains(PollFlags::OUT),
            "stream stdout closed or deadline expired"
        );
        // <= POSIX minimum PIPE_BUF, one exclusive writer. Never alter shared fd flags.
        match rustix::io::write(&stdout, &bytes[..bytes.len().min(512)]) {
            Err(rustix::io::Errno::INTR) => continue,
            result => {
                let written = result.context("stream stdout write failed")?;
                anyhow::ensure!(written != 0, "stream stdout closed");
                bytes = &bytes[written..];
            }
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn write_fragment(_fragment: &Fragment) -> Result<()> {
    anyhow::bail!("Unix stdout required");
}

pub(super) async fn run(
    agent: &JiaClawAgent,
    message: &str,
    skills: &[String],
    session_id: Option<String>,
    no_auto_skill: bool,
) -> Result<()> {
    // Admit finite ownership before reading history or submitting compaction/model work.
    let (progress, mut events) = ChatProgress::channel()?;
    let mut store = if session_id.is_some() && agent.config().http.persist {
        Some(open_configured_session_store(agent.config())?)
    } else {
        None
    };
    let mut messages = match (&store, &session_id) {
        (Some(store), Some(id)) => store
            .get(id)?
            .map_or_else(Vec::new, |record| record.messages),
        _ => Vec::new(),
    };
    messages.push(ChatMessage {
        role: MessageRole::User,
        content: message.to_owned(),
    });
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let compacted = tokio::select! {
        messages = agent.compact_session_messages(messages) => messages?,
        () = &mut shutdown => {
            progress.cancel();
            agent.settle_model_calls(STOP_GRACE).await?;
            anyhow::bail!("stream stopped during compaction; inspect model receipts; no replay");
        }
    };
    let request = ChatRequest {
        messages: compacted,
        enabled_tools: vec![],
        enabled_skills: skills.to_vec(),
        auto_skills: !no_auto_skill,
        session_id: session_id.clone(),
    };
    let output = Output::start(progress.clone());
    let chat = agent.chat_stream_for(&request, ModelPurpose::Chat, &progress);
    tokio::pin!(chat);
    let mut stopped = false;
    let outcome = loop {
        tokio::select! {
            biased;
            event = events.next(), if !stopped => {
                let delivered = match event {
                    Some(event) => output.line(&serde_json::to_value(event)?).await.is_ok(),
                    None => false,
                };
                if !delivered { stopped = true; break None; }
            },
            () = &mut shutdown => { stopped = true; progress.cancel(); break None; },
            response = &mut chat => break Some(response),
        }
    };
    let stop_deadline = tokio::time::Instant::now() + STOP_GRACE;
    let outcome = if let Some(outcome) = outcome {
        Some(outcome)
    } else {
        progress.cancel();
        tokio::time::timeout_at(stop_deadline, &mut chat).await.ok()
    };
    let result: Result<()> = async {
        let response = outcome.context("stream stop grace expired; inspect ledger and tool effects")??;
        if !stopped {
            while let Some(event) = events.try_next() {
                output.line(&serde_json::to_value(event)?).await?;
            }
        }
        let persisted = if let (Some(store), Some(id)) = (&mut store, &session_id) {
            let mut history = request.messages.clone(); history.push(response.message.clone());
            store.insert(id.clone(), SessionRecord::new(history))?; true
        } else { false };
        anyhow::ensure!(!stopped, "stream delivery stopped; inspect model receipts and any recorded tool review; no replay");
        output.line(&json!({"event":"done", "reply":response.message.content, "status":response.status,
            "session_id":session_id, "persisted":persisted})).await?;
        Ok(())
    }.await;
    if result.is_err() {
        progress.cancel();
        // Bounded diagnostics; private upstream data and partial arguments never leave via errors.
        let _ = output.line(&json!({"event":"error", "code":"stream_requires_review", "message":"No final delivery; inspect original model-calls receipt, session and tool effects. Do not replay the turn."})).await;
    }
    drop(events);
    // Await the owned model transaction even if the turn/consumer was canceled.
    let settlement = agent
        .settle_model_calls(stop_deadline.saturating_duration_since(tokio::time::Instant::now()))
        .await;
    let written = output.finish().await;
    result?;
    settlement?;
    written?;
    Ok(())
}
