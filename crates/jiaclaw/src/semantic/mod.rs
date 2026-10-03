// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicitly refreshed semantic memory. Markdown is authoritative; vectors are
//! a private cache. The operation ledger is not discarded with that cache.

mod store;
mod transport;

#[cfg(test)]
mod lifecycle_tests;

use crate::memory_io;
use jiaclaw_core::{AgentConfig, JiaClawError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use store::{Generation, Store, StoredChunk};
use transport::Transport;

const FILE_BYTES: usize = 128 * 1024;
const CHUNK_BYTES: usize = 1024;
const MAX_CHUNKS: usize = 512;
const MAX_REFRESH: Duration = Duration::from_secs(300);
const CHUNKER: &str = "utf8-lines-v1-symmetric";

fn error(message: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("semantic memory: {message}"))
}
fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
fn normalized_path(raw: &str) -> Result<String, JiaClawError> {
    let path = memory_io::relative(raw)?;
    Ok(path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

struct Snapshot {
    manifest: String,
    chunks: Vec<StoredChunk>,
}

fn snapshot(workspace: &Path, sources: &[String]) -> Result<Snapshot, JiaClawError> {
    let mut entries = Vec::new();
    let mut chunks = Vec::new();
    for path in sources {
        let file = memory_io::read_text(workspace, path, FILE_BYTES, false)?;
        entries.push((path, file.as_ref().map(|f| digest(f.text.as_bytes()))));
        let Some(file) = file else { continue };
        let mut offset = 0;
        let mut line = 1;
        while offset < file.text.len() {
            let mut end = (offset + CHUNK_BYTES).min(file.text.len());
            while !file.text.is_char_boundary(end) {
                end -= 1;
            }
            if end < file.text.len() {
                if let Some(split) = file.text[offset..end].rfind('\n') {
                    if split >= CHUNK_BYTES / 2 {
                        end = offset + split + 1;
                    }
                }
            }
            let text = &file.text[offset..end];
            if !text.trim().is_empty() {
                chunks.push(StoredChunk {
                    path: path.clone(),
                    line,
                    text: text.to_owned(),
                    hash: digest(text),
                    vector: Vec::new(),
                });
                if chunks.len() > MAX_CHUNKS {
                    return Err(error("source chunk budget exceeded"));
                }
            }
            line += text.bytes().filter(|b| *b == b'\n').count();
            offset = end;
        }
    }
    let manifest = digest(serde_json::to_vec(&(CHUNKER, entries)).map_err(error)?);
    Ok(Snapshot { manifest, chunks })
}

fn valid_vector(vector: &[f32], dimensions: usize) -> bool {
    if vector.len() != dimensions || vector.iter().any(|n| !n.is_finite()) {
        return false;
    }
    let norm: f64 = vector.iter().map(|n| f64::from(*n).powi(2)).sum();
    (0.999..=1.001).contains(&norm)
}

/// An opt-in, exclusively owned semantic index and billable-operation ledger.
/// No background indexing or model calls happen during construction.
pub struct SemanticMemory {
    workspace: PathBuf,
    sources: Vec<String>,
    dimensions: usize,
    timeout_secs: u64,
    space: String,
    credential: String,
    transport: Transport,
    store: Arc<Store>,
    admission: Arc<tokio::sync::Semaphore>,
    persistence: Arc<tokio::sync::Semaphore>,
}

impl SemanticMemory {
    /// Validate the administrator configuration and open its private store.
    /// Disabled configuration creates neither state nor network requests.
    ///
    /// # Errors
    /// Invalid configuration, private-path/ownership failures or database errors.
    pub async fn open(config: &AgentConfig) -> Result<Option<Arc<Self>>, JiaClawError> {
        let semantic = &config.memory.semantic;
        semantic.validate()?;
        if !semantic.enabled {
            return Ok(None);
        }
        if !config.tools.memory_search.enabled {
            return Err(error("requires memory_search enabled"));
        }
        if config.provider.provider_type != "brokerrouter" {
            return Err(error("requires brokerrouter provider"));
        }
        let mut provider = config.provider.clone();
        let env_key = std::env::var("JIACLAW_API_KEY").ok();
        provider.api_key =
            crate::JiaClawAgent::resolve_provider_api_key(&provider, env_key.as_deref())?;
        let transport = Transport::new(&provider, semantic)?;
        let space = digest(format!("{}:{CHUNKER}", transport.fingerprint()));
        let credential = transport.credential_hash();
        let configured = if semantic.sources.is_empty() {
            vec![config.memory.path.clone()]
        } else {
            semantic.sources.clone()
        };
        // The inherited legacy memory.path needs the same upload/path contract
        // as explicitly listed sources, before creating a DB or sending text.
        let mut resolved = semantic.clone();
        resolved.sources.clone_from(&configured);
        resolved.validate()?;
        let mut sources = configured
            .iter()
            .map(|p| normalized_path(p))
            .collect::<Result<Vec<_>, _>>()?;
        sources.sort();
        if sources.is_empty()
            || sources.len() > 3
            || sources.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(error("requires one to three distinct sources"));
        }
        let workspace = config.workspace_path.clone();
        let index_path = semantic.index_path.clone();
        let store =
            Arc::new(memory_io::run_blocking(move || Store::open(&workspace, &index_path)).await?);
        Ok(Some(Arc::new(Self {
            workspace: config.workspace_path.clone(),
            sources,
            dimensions: semantic.dimensions,
            timeout_secs: semantic.timeout_secs,
            space,
            credential,
            transport,
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
        // Once a billable operation is admitted, ordinary file-I/O pressure
        // must not reject its receipt/remote identity before persistence.
        // This dedicated single-worker lane waits without occupying a Tokio
        // thread and retains both the DB ownership and capacity on cancellation.
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
        .map_err(|_| error("persistence task failed; inspect status before another request"))?
    }
    async fn snapshot(&self) -> Result<Snapshot, JiaClawError> {
        let workspace = self.workspace.clone();
        let sources = self.sources.clone();
        memory_io::run_blocking(move || snapshot(&workspace, &sources)).await
    }
    async fn no_pending(&self) -> Result<(), JiaClawError> {
        if let Some(operation) = self.database(Store::pending).await? {
            return Err(error(format!(
                "needs_review: operation {} ({}) must be recovered or reconciled first",
                operation.id, operation.state
            )));
        }
        Ok(())
    }
    async fn admitted<T, F, Fut>(self: &Arc<Self>, work: F) -> Result<T, JiaClawError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, JiaClawError>> + Send + 'static,
    {
        let permit = Arc::clone(&self.admission)
            .try_acquire_owned()
            .map_err(|_| error("busy: one operation already running"))?;
        let service = Arc::clone(self);
        // Dropping/aborting the waiting tool never cancels admitted work or
        // releases its store lock and permit before network/commit completion.
        tokio::spawn(async move {
            let _permit = permit;
            work(service).await
        })
        .await
        .map_err(|_| error("worker failed; inspect status before another request"))?
    }
    async fn embeddings(
        &self,
        kind: &'static str,
        input: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, JiaClawError> {
        self.no_pending().await?;
        let request_hash = self.transport.request_hash(&input);
        let space = self.space.clone();
        let cached_hash = request_hash.clone();
        if let Some(vectors) = self
            .database(move |store| store.cached_receipt(&space, &cached_hash))
            .await?
        {
            self.validate_vectors(&vectors, input.len())?;
            return Ok(vectors);
        }
        let space = self.space.clone();
        let credential = self.credential.clone();
        let count = input.len();
        let id = self
            .database(move |store| {
                store.begin_operation(kind, &space, &credential, &request_hash, count)
            })
            .await?;
        match self.transport.embed(&id, &input).await {
            Ok(receipt) => {
                self.validate_vectors(&receipt.vectors, count)?;
                let save = receipt.vectors.clone();
                let operation = id.clone();
                let remote = receipt.request_id;
                self.database(move |store| {
                    store.complete_operation(&operation, Some(&remote), &save)
                })
                .await?;
                Ok(receipt.vectors)
            }
            Err(failed) => {
                let operation = id.clone();
                self.database(move |store| {
                    store.mark_unknown(&operation, failed.request_id.as_deref())
                })
                .await?;
                // No remote body/headers are disclosed and no automatic POST retry.
                Err(error(format!(
                    "needs_review: embedding operation {id}; use local status/recovery"
                )))
            }
        }
    }
    fn validate_vectors(&self, vectors: &[Vec<f32>], count: usize) -> Result<(), JiaClawError> {
        if vectors.len() != count || vectors.iter().any(|v| !valid_vector(v, self.dimensions)) {
            return Err(error("invalid cached/received vectors; inspect index"));
        }
        Ok(())
    }

    /// Refresh a complete generation explicitly, with at most five minutes of
    /// request admission. Completed batches can be reused after an interruption.
    /// # Errors
    /// Source bounds, unknown operations, transport or persistence failures.
    pub async fn refresh(self: &Arc<Self>) -> Result<Value, JiaClawError> {
        self.admitted(|service| async move { service.refresh_inner().await })
            .await
    }
    async fn refresh_inner(&self) -> Result<Value, JiaClawError> {
        self.no_pending().await?;
        let started = Instant::now();
        let mut current = self.snapshot().await?;
        let space = self.space.clone();
        let previous = self
            .database(move |store| store.load_generation(&space))
            .await?;
        let mut vectors: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        if let Some(previous) = previous {
            for chunk in previous.chunks {
                if digest(&chunk.text) != chunk.hash
                    || !valid_vector(&chunk.vector, self.dimensions)
                {
                    return Err(error("index_corrupt: explicitly rebuild the cache"));
                }
                vectors.insert(chunk.hash, chunk.vector);
            }
        }
        let mut missing = BTreeMap::new();
        for chunk in &current.chunks {
            if !vectors.contains_key(&chunk.hash) {
                missing
                    .entry(chunk.hash.clone())
                    .or_insert_with(|| chunk.text.clone());
            }
        }
        let missing = missing.into_iter().collect::<Vec<_>>();
        for batch in missing.chunks(8) {
            if started
                .elapsed()
                .saturating_add(Duration::from_secs(self.timeout_secs))
                > MAX_REFRESH
            {
                return Err(error("refresh deadline reached; completed batches retained; explicitly refresh to continue"));
            }
            // Do not spend on an already outdated source generation.
            if self.snapshot().await?.manifest != current.manifest {
                return Err(error("source_changed: refresh again after edits stop"));
            }
            let input = batch.iter().map(|(_, text)| text.clone()).collect();
            let output = self.embeddings("refresh", input).await?;
            for ((hash, _), vector) in batch.iter().zip(output) {
                vectors.insert(hash.clone(), vector);
            }
        }
        for chunk in &mut current.chunks {
            chunk.vector = vectors
                .get(&chunk.hash)
                .ok_or_else(|| error("incomplete generation"))?
                .clone();
        }
        if self.snapshot().await?.manifest != current.manifest {
            return Err(error("source_changed: generation was not published"));
        }
        let chunks = current.chunks.len();
        let manifest = current.manifest.clone();
        let generation = Generation {
            space: self.space.clone(),
            manifest: current.manifest,
            chunks: current.chunks,
        };
        self.database(move |store| store.publish_generation(&generation))
            .await?;
        Ok(
            json!({"status":"ready", "sources":self.sources.len(), "chunks":chunks, "space":self.space, "manifest":manifest}),
        )
    }

    /// Search the current, explicitly refreshed generation. A stale source hash
    /// refuses the query before embedding and never returns old snippets.
    /// # Errors
    /// Disabled/unauthorized paths, stale index, unresolved operations or IO.
    pub async fn search(
        self: &Arc<Self>,
        query: String,
        max_results: usize,
        paths: Option<Vec<String>>,
    ) -> Result<Value, JiaClawError> {
        if query.trim().is_empty() || query.len() > 1024 || !(1..=20).contains(&max_results) {
            return Err(error("query/max_results exceed bounds"));
        }
        let paths = paths
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.sources.clone());
        if paths.len() > 3 {
            return Err(error("paths may only narrow the configured sources"));
        }
        let selected = paths
            .iter()
            .map(|p| normalized_path(p))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if selected.iter().any(|p| !self.sources.contains(p)) {
            return Err(error("paths may only narrow the configured sources"));
        }
        self.admitted(move |service| async move {
            service.search_inner(query, max_results, selected).await
        })
        .await
    }
    async fn search_inner(
        &self,
        query: String,
        max_results: usize,
        selected: BTreeSet<String>,
    ) -> Result<Value, JiaClawError> {
        self.no_pending().await?;
        let current = self.snapshot().await?;
        let space = self.space.clone();
        let stored = self
            .database(move |store| store.load_generation(&space))
            .await?
            .ok_or_else(|| error("stale_index: run local memory semantic refresh"))?;
        if stored.manifest != current.manifest {
            return Err(error(
                "stale_index: source bytes or allowlist changed; refresh before querying",
            ));
        }
        if stored.chunks.len() != current.chunks.len()
            || stored
                .chunks
                .iter()
                .zip(&current.chunks)
                .any(|(saved, now)| {
                    saved.path != now.path
                        || saved.line != now.line
                        || saved.text != now.text
                        || saved.hash != now.hash
                        || !valid_vector(&saved.vector, self.dimensions)
                })
        {
            return Err(error(
                "index_corrupt: generation does not match current source snapshots",
            ));
        }
        let mut ranked = Vec::new();
        if stored
            .chunks
            .iter()
            .any(|chunk| selected.contains(&chunk.path))
        {
            let query_vectors = self.embeddings("query", vec![query]).await?;
            let vector = &query_vectors[0];
            let mut seen = BTreeSet::new();
            for chunk in stored.chunks {
                if !selected.contains(&chunk.path) || !seen.insert(chunk.hash) {
                    continue;
                }
                let score = chunk
                    .vector
                    .iter()
                    .zip(vector)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum::<f64>()
                    .clamp(-1.0, 1.0);
                ranked.push((score, chunk.path, chunk.line, chunk.text));
            }
            ranked.sort_by(|a, b| {
                b.0.total_cmp(&a.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| a.2.cmp(&b.2))
            });
        }
        if self.snapshot().await?.manifest != current.manifest {
            return Err(error(
                "stale_index: source changed during query; result discarded",
            ));
        }
        let mut bytes = 0;
        let mut matches = Vec::new();
        for (score, path, line, text) in ranked.into_iter().take(max_results) {
            if bytes + text.len() > 16 * 1024 {
                break;
            }
            bytes += text.len();
            matches.push(json!({"path":path,"line":line,"excerpt":text,"score":score}));
        }
        let result = json!({"mode":"semantic", "space":self.space, "manifest":current.manifest, "matches":matches});
        if serde_json::to_vec(&result).map_err(error)?.len() > 64 * 1024 {
            return Err(error("result JSON exceeds 64 KiB"));
        }
        Ok(result)
    }

    /// Return local metadata; never returns source text, vectors or credentials.
    /// # Errors
    /// Database read failure.
    pub async fn status(&self) -> Result<Value, JiaClawError> {
        let mut status = self.database(Store::status).await?;
        status["active_space"] = json!(self.space);
        status["sources"] = json!(self.sources);
        Ok(status)
    }
    /// Recover a saved remote result through GET only. This records its vectors
    /// but does not publish an old source generation or replay a request.
    /// # Errors
    /// Unknown identity, unavailable remote result or credential/space mismatch.
    pub async fn recover(self: &Arc<Self>, operation_id: String) -> Result<Value, JiaClawError> {
        self.admitted(move |service| async move {
            let pending = service
                .database(Store::pending)
                .await?
                .ok_or_else(|| error("no operation needs recovery"))?;
            if pending.id != operation_id
                || pending.space != service.space
                || pending.credential != service.credential
            {
                return Err(error("operation identity/space/credential mismatch; restore its configuration or reconcile manually"));
            }
            let remote = pending.remote_id.ok_or_else(|| {
                error("remote request ID unavailable; operator reconciliation required")
            })?;
            let receipt = service
                .transport
                .recover(&remote, pending.count)
                .await
                .map_err(|_| error("result unavailable; hold retained, no POST replay"))?;
            service.validate_vectors(&receipt.vectors, pending.count)?;
            let operation = operation_id.clone();
            let remote = receipt.request_id;
            let vectors = receipt.vectors;
            service
                .database(move |store| store.complete_operation(&operation, Some(&remote), &vectors))
                .await?;
            Ok(json!({"status":"recovered","operation_id":operation_id}))
        })
        .await
    }

    /// Clear an unknown hold only after explicit operator reconciliation.
    /// # Errors
    /// Active/mismatched operation, invalid note or persistence failure.
    pub async fn review_clear(
        self: &Arc<Self>,
        operation_id: String,
        note: String,
    ) -> Result<Value, JiaClawError> {
        self.admitted(move |service| async move {
            let operation = operation_id.clone();
            service
                .database(move |store| store.review_clear(&operation, &note))
                .await?;
            Ok(json!({"status":"review_cleared","operation_id":operation_id}))
        })
        .await
    }
    /// Remove the published vector generation, retaining the operation ledger.
    /// # Errors
    /// Outstanding hold or persistence failure.
    pub async fn rebuild(self: &Arc<Self>) -> Result<Value, JiaClawError> {
        self.admitted(|service| async move {
            service.no_pending().await?;
            service.database(Store::clear_generation).await?;
            Ok(json!({"status":"cleared"}))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunking_hashes_real_bytes_and_keeps_utf8_lines_and_budgets() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("{}\n{}", "中".repeat(500), "next line\n".repeat(150));
        std::fs::write(dir.path().join("a.md"), &text).unwrap();
        let sources = vec!["a.md".into()];
        let first = snapshot(dir.path(), &sources).unwrap();
        assert_eq!(
            first
                .chunks
                .iter()
                .map(|c| c.text.as_str())
                .collect::<String>(),
            text
        );
        assert!(first
            .chunks
            .iter()
            .all(|c| c.text.len() <= CHUNK_BYTES && c.line > 0 && c.hash == digest(&c.text)));
        let line: usize = first.chunks[..2]
            .iter()
            .map(|c| c.text.bytes().filter(|b| *b == b'\n').count())
            .sum();
        assert_eq!(first.chunks[2].line, line + 1);
        std::fs::write(dir.path().join("a.md"), text.replace("next", "same")).unwrap();
        assert_ne!(
            first.manifest,
            snapshot(dir.path(), &sources).unwrap().manifest
        );
        std::fs::remove_file(dir.path().join("a.md")).unwrap();
        let missing = snapshot(dir.path(), &sources).unwrap();
        assert!(missing.chunks.is_empty());
        assert_ne!(first.manifest, missing.manifest);
        std::fs::write(dir.path().join("a.md"), "x".repeat(FILE_BYTES + 1)).unwrap();
        assert!(snapshot(dir.path(), &sources).is_err());
    }
    #[test]
    fn vector_and_path_policy_are_strict() {
        assert!(valid_vector(&[0.6, 0.8], 2));
        assert!(!valid_vector(&[0.0, 0.0], 2));
        assert!(!valid_vector(&[f32::NAN, 1.0], 2));
        assert!(!valid_vector(&[1.0], 2));
        assert_eq!(normalized_path("./a/./b.md").unwrap(), "a/b.md");
        assert!(normalized_path("../secret").is_err());
    }
}
