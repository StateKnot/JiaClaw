// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Receipts for individual model calls, never automatic tool or turn recovery.
#[cfg(test)]
mod lifecycle_tests;
mod store;

use crate::provider::brokerrouter::{BrokerrouterProvider, PreparedCompletion, WireMessage};
use crate::{ChatProgress, ChatProgressEvent};
use jiaclaw_core::{AgentConfig, JiaClawError, ModelPurpose};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use store::{NewCall, Store};
use uuid::Uuid;

fn error(message: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::StateKnotIntegration(format!("model calls: {message}"))
}
pub(crate) fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

/// Private, exclusively owned ledger. Unknown outcomes block further model calls.
/// Opening and inspecting it never sends a model request.
pub struct ModelCalls {
    provider: BrokerrouterProvider,
    endpoint_hash: String,
    credential_hash: String,
    store: Arc<Store>,
    admission: Arc<tokio::sync::Semaphore>,
    persistence: Arc<tokio::sync::Semaphore>,
}

impl ModelCalls {
    /// Open the configured private ledger, if enabled.
    ///
    /// # Errors
    /// Invalid configuration, private-path/ownership or database failure.
    pub async fn open(config: &AgentConfig) -> Result<Option<Arc<Self>>, JiaClawError> {
        config.model_calls.validate(&config.provider)?;
        if !config.model_calls.enabled {
            return Ok(None);
        }
        let key = crate::JiaClawAgent::resolve_provider_api_key(
            &config.provider,
            std::env::var("JIACLAW_API_KEY").ok().as_deref(),
        )?
        .ok_or_else(|| error("missing gateway credential"))?;
        let endpoint = reqwest::Url::parse(&config.provider.base_url)
            .map_err(|_| error("invalid gateway endpoint"))?
            .as_str()
            .trim_end_matches('/')
            .to_owned();
        let provider = BrokerrouterProvider::new(&endpoint, &key);
        // Validate the configured transport before opening state, without any I/O.
        provider.validate()?;
        let workspace = config.workspace_path.clone();
        let path = config.model_calls.store_path.clone();
        let store =
            Arc::new(crate::memory_io::run_blocking(move || Store::open(&workspace, &path)).await?);
        Ok(Some(Arc::new(Self {
            provider,
            endpoint_hash: digest(endpoint),
            credential_hash: digest(key),
            store,
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            persistence: Arc::new(tokio::sync::Semaphore::new(1)),
        })))
    }

    async fn database<T, F>(&self, action: F) -> Result<T, JiaClawError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> Result<T, JiaClawError> + Send + 'static,
    {
        let permit = Arc::clone(&self.persistence)
            .acquire_owned()
            .await
            .map_err(|_| error("persistence executor unavailable"))?;
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            action(&store)
        })
        .await
        .map_err(|_| error("persistence worker failed; inspect ledger before another request"))?
    }

    pub(crate) async fn ensure_clear(&self) -> Result<(), JiaClawError> {
        if let Some(pending) = self.database(Store::pending).await? {
            return Err(error(format!(
                "needs_review: operation {} ({}) must be reconciled first",
                pending.id, pending.state
            )));
        }
        Ok(())
    }

    /// Serialize once before persistence, so the stored hash describes the exact POST.
    pub(crate) fn prepare(
        &self,
        model: &str,
        messages: &[WireMessage],
        temperature: f32,
        max_tokens: u32,
        tools: &[Value],
    ) -> Result<PreparedCompletion, JiaClawError> {
        self.provider
            .prepare(model, messages, temperature, max_tokens, tools)
    }

    pub(crate) fn prepare_stream(
        &self,
        model: &str,
        messages: &[WireMessage],
        temperature: f32,
        max_tokens: u32,
        tools: &[Value],
    ) -> Result<PreparedCompletion, JiaClawError> {
        self.provider
            .prepare_stream(model, messages, temperature, max_tokens, tools)
    }

    pub(crate) async fn complete(
        self: &Arc<Self>,
        prepared: PreparedCompletion,
        turn_id: String,
        purpose: ModelPurpose,
        session_hash: Option<String>,
        round: u32,
    ) -> Result<WireMessage, JiaClawError> {
        self.complete_inner(prepared, turn_id, purpose, session_hash, round, None)
            .await
    }

    pub(crate) async fn complete_stream(
        self: &Arc<Self>,
        prepared: PreparedCompletion,
        turn_id: String,
        purpose: ModelPurpose,
        session_hash: Option<String>,
        round: u32,
        progress: ChatProgress,
    ) -> Result<WireMessage, JiaClawError> {
        self.complete_inner(
            prepared,
            turn_id,
            purpose,
            session_hash,
            round,
            Some(progress),
        )
        .await
    }

    async fn complete_inner(
        self: &Arc<Self>,
        prepared: PreparedCompletion,
        turn_id: String,
        purpose: ModelPurpose,
        session_hash: Option<String>,
        round: u32,
        progress: Option<ChatProgress>,
    ) -> Result<WireMessage, JiaClawError> {
        if prepared.streaming() != progress.is_some() {
            return Err(error(
                "request transport mode does not match the receipt codec; not submitted",
            ));
        }
        let permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| error("busy: one model call already running"))?;
        let service = Arc::clone(self);
        // Only the bounded model/receipt transaction survives caller cancellation.
        // The awaiting native loop is never spawned here: canceled callers execute no tools.
        tokio::spawn(async move {
            let _permit = permit;
            if let Some(progress) = &progress { progress.ensure_open()?; }
            service.ensure_clear().await?;
            if let Some(progress) = &progress { progress.ensure_open()?; }
            let id = Uuid::new_v4().to_string();
            let call = NewCall {
                id: id.clone(), turn_id: turn_id.clone(), purpose, session_hash, round,
                model: prepared.model().to_owned(), has_tools: prepared.has_tools(),
                streaming: prepared.streaming(),
                endpoint_hash: service.endpoint_hash.clone(), credential_hash: service.credential_hash.clone(),
                body_hash: prepared.request_hash().to_owned(),
            };
            service.database(move |store| store.begin(call)).await?;
            let outcome: Result<WireMessage, JiaClawError> = async {
                let pending = service.provider.post(&id, &prepared).await?;
                if let Some(remote) = pending.remote_id() {
                    let remote = remote.to_owned();
                    let local = id.clone();
                    service.database(move |store| store.record_remote(&local, &remote)).await?;
                }
                if let Some(progress) = &progress {
                    let remote_id = pending.remote_id().ok_or_else(|| error("missing stream remote identity"))?.to_owned();
                    progress.emit(ChatProgressEvent::ModelStarted {
                        turn_id: turn_id.clone(), operation_id: id.clone(), remote_id, round, model: prepared.model().to_owned(),
                    }).await;
                }
                let receipt = if let Some(progress) = &progress {
                    service.provider.finish_stream(pending, &prepared, progress, round).await?
                } else { service.provider.finish(pending, &prepared).await? };
                let body: Value = serde_json::from_slice(&receipt.body)
                    .map_err(|_| error("invalid validated receipt"))?;
                let local = id.clone();
                let remote = receipt.request_id;
                service.database(move |store| store.complete(&local, &remote, &body, false)).await?;
                if let Some(progress) = &progress {
                    progress.emit(ChatProgressEvent::ModelCompleted { operation_id: id.clone(), round }).await;
                }
                Ok(receipt.message)
            }.await;
            match outcome {
                Ok(message) => Ok(message),
                Err(cause) => {
                    tracing::warn!(operation_id = %id, error = %cause, "model call requires reconciliation");
                    let local = id.clone();
                    // A failed write still leaves the original submitting hold intact.
                    let _ = service.database(move |store| store.mark_unknown(&local)).await;
                    Err(error(format!("needs_review: operation {id} has an uncertain outcome; inspect model-calls status")))
                }
            }
        }).await.map_err(|_| error("model worker failed; inspect ledger before another request"))?
    }

    pub(crate) async fn settle(&self, grace: std::time::Duration) -> Result<(), JiaClawError> {
        let permit = tokio::time::timeout(grace, Arc::clone(&self.admission).acquire_owned())
            .await
            .map_err(|_| {
                error("settlement grace expired; inspect persistent ledger after restart")
            })?
            .map_err(|_| error("settlement owner unavailable"))?;
        drop(permit);
        Ok(())
    }

    /// Return metadata only; model output is available through explicit `result`.
    /// # Errors
    /// Database read failure.
    pub async fn status(&self) -> Result<Value, JiaClawError> {
        self.database(Store::status).await
    }

    /// Read a retained response receipt. This may contain sensitive model output.
    /// # Errors
    /// Invalid identity or database read failure.
    pub async fn result(&self, id: String) -> Result<Value, JiaClawError> {
        self.database(move |store| store.show_result(&id))
            .await?
            .ok_or_else(|| error("receipt unavailable or outside retention window"))
    }

    /// GET-only reconciliation of a known remote request. Never executes returned tools,
    /// appends session history, sends outbound messages or starts another model request.
    /// # Errors
    /// Missing/mismatched identity, still-pending or invalid gateway result, storage failure.
    pub async fn recover(self: &Arc<Self>, id: String) -> Result<Value, JiaClawError> {
        let permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| error("busy: one model call already running"))?;
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let _permit = permit;
            let operation = service.database(Store::pending).await?
                .filter(|operation| operation.id == id && operation.state == "unknown")
                .ok_or_else(|| error("operation is not the pending unknown call"))?;
            if operation.endpoint_hash != service.endpoint_hash || operation.credential_hash != service.credential_hash {
                return Err(error("recovery requires the original endpoint and virtual key"));
            }
            let remote = operation.remote_id.ok_or_else(|| error("remote request ID unavailable; reconcile independently before review-clear; no POST retry is permitted"))?;
            let status = service.provider.get_status(&remote, &operation.model).await?;
            if status.state != "succeeded" {
                return Err(error(format!("remote operation remains {}; hold retained", status.state)));
            }
            let receipt = service.provider.get_result(&remote, &operation.model, operation.has_tools).await?;
            if operation.streaming { service.provider.validate_stream_recovery(&receipt)?; }
            let body: Value = serde_json::from_slice(&receipt.body).map_err(|_| error("invalid recovery receipt"))?;
            let local = id.clone();
            service.database(move |store| store.complete(&local, &remote, &body, true)).await?;
            Ok(json!({"operation_id":id,"state":"completed","recovered":true,"applied_to_turn":false}))
        }).await.map_err(|_| error("recovery worker failed; inspect ledger"))?
    }

    /// Clear an independently reconciled unknown call, preserving the audit note.
    /// # Errors
    /// Invalid operation/note, active model call or storage failure.
    pub async fn review_clear(
        self: &Arc<Self>,
        id: String,
        note: String,
    ) -> Result<Value, JiaClawError> {
        let _permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| error("busy: one model call already running"))?;
        let local = id.clone();
        self.database(move |store| store.review_clear(&local, &note))
            .await?;
        Ok(json!({"operation_id":id,"state":"cleared","recovered":false}))
    }
}
