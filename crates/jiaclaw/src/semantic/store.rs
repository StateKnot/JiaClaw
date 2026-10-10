// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Private semantic cache and durable embedding-operation admission.
//!
//! The canonical workspace and private state directory are administrator-owned.
//! Replacing, relocating or rolling back this database is an administrative
//! recovery operation; a different database cannot discover this one's holds.

use crate::private_state_file;
use jiaclaw_core::JiaClawError;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, JiaClawError>;
const APPLICATION_ID: i32 = 0x4a43_534d;
const SCHEMA_VERSION: i64 = 2;
const MAX_DATABASE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;
const MAX_OPERATIONS: i64 = 1024;
const RETAIN_COMPLETED: i64 = 64;
const MAX_CHUNKS: usize = 512;
const MAX_DIMENSIONS: usize = 3072;
const MAX_BATCH: usize = 8;
const GENERATION_MAGIC: &[u8; 8] = b"JCSMG001";
const RECEIPT_MAGIC: &[u8; 8] = b"JCSMR001";
// 152 bytes of header; each chunk has 20 bytes of lengths/line plus bounded
// path/text/hash and fixed-width vector elements. No JSON escaping expansion.
const MAX_GENERATION_BYTES: usize = 152 + MAX_CHUNKS * (20 + 1024 + 1024 + 64 + MAX_DIMENSIONS * 4);
const MAX_RECEIPT_BYTES: usize = 16 + MAX_BATCH * MAX_DIMENSIONS * 4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredChunk {
    pub path: String,
    pub line: usize,
    pub text: String,
    pub hash: String,
    pub vector: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Generation {
    pub space: String,
    pub manifest: String,
    pub chunks: Vec<StoredChunk>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Operation {
    pub id: String,
    pub kind: String,
    pub space: String,
    /// Irreversible credential fingerprint, never the bearer secret.
    pub credential: String,
    pub request_hash: String,
    pub count: usize,
    pub remote_id: Option<String>,
    pub state: String,
}

/// One connection and one exclusive process-lifetime ownership lock.
/// Calls are synchronous and short; callers must not hold this across network I/O.
pub struct Store {
    connection: Mutex<Connection>,
    _ownership: File,
}

fn failure(message: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("semantic store: {message}"))
}

fn bounded(value: &str, max: usize, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > max
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(failure(format!("invalid {label}")));
    }
    Ok(())
}

fn fingerprint(value: &str, label: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(failure(format!("{label} must be a SHA-256 fingerprint")));
    }
    Ok(())
}

fn operation_id(id: &str) -> Result<()> {
    Uuid::parse_str(id).map_err(|_| failure("invalid operation ID"))?;
    Ok(())
}

fn vectors_valid(vectors: &[Vec<f32>], count: usize) -> Result<()> {
    if vectors.len() != count || count == 0 || count > MAX_CHUNKS {
        return Err(failure("vector count mismatch or limit exceeded"));
    }
    let dimensions = vectors[0].len();
    if dimensions == 0
        || dimensions > MAX_DIMENSIONS
        || vectors.iter().any(|vector| {
            vector.len() != dimensions || vector.iter().any(|value| !value.is_finite())
        })
    {
        return Err(failure("invalid vector dimensions or non-finite values"));
    }
    Ok(())
}

fn generation_valid(generation: &Generation) -> Result<()> {
    fingerprint(&generation.space, "embedding space")?;
    fingerprint(&generation.manifest, "manifest")?;
    if generation.chunks.len() > MAX_CHUNKS {
        return Err(failure("generation chunk limit exceeded"));
    }
    let mut dimensions = None;
    for chunk in &generation.chunks {
        bounded(&chunk.path, 1024, "chunk path")?;
        let path = Path::new(&chunk.path);
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            || !path
                .components()
                .any(|part| matches!(part, Component::Normal(_)))
            || chunk.line == 0
            || chunk.text.len() > 1024
        {
            return Err(failure("invalid stored chunk"));
        }
        fingerprint(&chunk.hash, "chunk hash")?;
        let dim = chunk.vector.len();
        if dim == 0
            || dim > MAX_DIMENSIONS
            || dimensions.is_some_and(|previous| previous != dim)
            || chunk.vector.iter().any(|value| !value.is_finite())
        {
            return Err(failure("invalid generation vector"));
        }
        dimensions = Some(dim);
    }
    Ok(())
}

fn push(output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    if output.len().saturating_add(bytes.len()) > MAX_BLOB_BYTES {
        return Err(failure("encoded semantic data exceeds 8 MiB"));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn push_u32(output: &mut Vec<u8>, value: usize) -> Result<()> {
    push(
        output,
        &u32::try_from(value).map_err(failure)?.to_le_bytes(),
    )
}

fn push_string(output: &mut Vec<u8>, value: &str) -> Result<()> {
    push_u32(output, value.len())?;
    push(output, value.as_bytes())
}

fn push_vector(output: &mut Vec<u8>, vector: &[f32]) -> Result<()> {
    for value in vector {
        push(output, &value.to_le_bytes())?;
    }
    Ok(())
}

struct Decoder<'a> {
    remaining: &'a [u8],
}
impl<'a> Decoder<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.remaining.len() {
            return Err(failure("truncated semantic binary data"));
        }
        let (bytes, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<usize> {
        usize::try_from(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(failure)?,
        ))
        .map_err(failure)
    }
    fn string(&mut self, limit: usize) -> Result<String> {
        let count = self.u32()?;
        if count > limit {
            return Err(failure("semantic string exceeds encoded limit"));
        }
        std::str::from_utf8(self.take(count)?)
            .map(str::to_owned)
            .map_err(|_| failure("invalid encoded UTF-8"))
    }
    fn vector(&mut self, dimensions: usize) -> Result<Vec<f32>> {
        let bytes = self.take(dimensions * 4)?;
        bytes
            .chunks_exact(4)
            .map(|bytes| {
                let value = f32::from_le_bytes(bytes.try_into().map_err(failure)?);
                if !value.is_finite() {
                    return Err(failure("non-finite encoded vector"));
                }
                Ok(value)
            })
            .collect()
    }
    fn finish(self) -> Result<()> {
        if !self.remaining.is_empty() {
            return Err(failure("trailing semantic binary data"));
        }
        Ok(())
    }
}

fn encode_generation(generation: &Generation) -> Result<Vec<u8>> {
    generation_valid(generation)?;
    let mut output = Vec::new();
    push(&mut output, GENERATION_MAGIC)?;
    push_string(&mut output, &generation.space)?;
    push_string(&mut output, &generation.manifest)?;
    push_u32(&mut output, generation.chunks.len())?;
    push_u32(
        &mut output,
        generation
            .chunks
            .first()
            .map_or(0, |chunk| chunk.vector.len()),
    )?;
    for chunk in &generation.chunks {
        push_string(&mut output, &chunk.path)?;
        push(
            &mut output,
            &u64::try_from(chunk.line).map_err(failure)?.to_le_bytes(),
        )?;
        push_string(&mut output, &chunk.text)?;
        push_string(&mut output, &chunk.hash)?;
        push_vector(&mut output, &chunk.vector)?;
    }
    if output.len() > MAX_GENERATION_BYTES {
        return Err(failure("generation exceeds encoded limit"));
    }
    Ok(output)
}

fn decode_generation(bytes: &[u8]) -> Result<Generation> {
    if bytes.len() > MAX_GENERATION_BYTES {
        return Err(failure("generation exceeds encoded limit"));
    }
    let mut decoder = Decoder { remaining: bytes };
    if decoder.take(8)? != GENERATION_MAGIC {
        return Err(failure("unsupported generation encoding"));
    }
    let space = decoder.string(64)?;
    let manifest = decoder.string(64)?;
    let count = decoder.u32()?;
    let dimensions = decoder.u32()?;
    if count > MAX_CHUNKS
        || (count == 0 && dimensions != 0)
        || (count != 0 && (dimensions == 0 || dimensions > MAX_DIMENSIONS))
    {
        return Err(failure("invalid encoded generation shape"));
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let path = decoder.string(1024)?;
        let line = usize::try_from(u64::from_le_bytes(
            decoder.take(8)?.try_into().map_err(failure)?,
        ))
        .map_err(failure)?;
        let text = decoder.string(1024)?;
        let hash = decoder.string(64)?;
        let vector = decoder.vector(dimensions)?;
        chunks.push(StoredChunk {
            path,
            line,
            text,
            hash,
            vector,
        });
    }
    decoder.finish()?;
    let generation = Generation {
        space,
        manifest,
        chunks,
    };
    generation_valid(&generation)?;
    Ok(generation)
}

fn encode_receipt(vectors: &[Vec<f32>]) -> Result<Vec<u8>> {
    if vectors.len() > MAX_BATCH {
        return Err(failure("receipt batch limit exceeded"));
    }
    vectors_valid(vectors, vectors.len())?;
    let mut output = Vec::new();
    push(&mut output, RECEIPT_MAGIC)?;
    push_u32(&mut output, vectors.len())?;
    push_u32(&mut output, vectors[0].len())?;
    for vector in vectors {
        push_vector(&mut output, vector)?;
    }
    Ok(output)
}

fn decode_receipt(bytes: &[u8], expected_count: usize) -> Result<Vec<Vec<f32>>> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(failure("receipt exceeds encoded limit"));
    }
    let mut decoder = Decoder { remaining: bytes };
    if decoder.take(8)? != RECEIPT_MAGIC {
        return Err(failure("unsupported receipt encoding"));
    }
    let count = decoder.u32()?;
    let dimensions = decoder.u32()?;
    if count != expected_count
        || count == 0
        || count > MAX_BATCH
        || dimensions == 0
        || dimensions > MAX_DIMENSIONS
    {
        return Err(failure("invalid encoded receipt shape"));
    }
    let mut vectors = Vec::with_capacity(count);
    for _ in 0..count {
        vectors.push(decoder.vector(dimensions)?);
    }
    decoder.finish()?;
    Ok(vectors)
}

fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn state_path(workspace: &Path, requested: &Path) -> Result<(PathBuf, PathBuf)> {
    let workspace = workspace.canonicalize().map_err(failure)?;
    if !workspace.is_dir() || workspace.to_str().is_none() {
        return Err(failure("workspace must be a canonical UTF-8 directory"));
    }
    if requested.as_os_str().is_empty() {
        return Err(failure("index path is empty"));
    }
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace.join(requested)
    };
    let mut path = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                if !path.pop() {
                    return Err(failure("invalid state path"));
                }
            }
            Component::CurDir => (),
            Component::Prefix(_) => return Err(failure("unsupported state path")),
            other => path.push(other.as_os_str()),
        }
    }
    if path.starts_with(&workspace) || path.file_name().is_none() {
        return Err(failure(
            "semantic state must be outside the agent workspace",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| failure("state path has no parent"))?;
    let mut current = PathBuf::new();
    for part in parent.components() {
        current.push(part.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => {
                return Err(failure(
                    "state ancestors must be directories without symlinks",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&current).map_err(failure)?;
                #[cfg(unix)]
                File::open(
                    current
                        .parent()
                        .ok_or_else(|| failure("invalid state parent"))?,
                )
                .and_then(|file| file.sync_all())
                .map_err(failure)?;
            }
            Err(error) => return Err(failure(error)),
        }
    }
    let metadata = fs::symlink_metadata(parent).map_err(failure)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(failure(
                "state parent requires mode 0700; existing directories are not chmodded",
            ));
        }
    }
    if parent.canonicalize().map_err(failure)? != parent {
        return Err(failure("state directory changed during validation"));
    }
    Ok((workspace, path))
}

const SCHEMA: &str = "
CREATE TABLE semantic_meta(id INTEGER PRIMARY KEY CHECK(id=1), workspace TEXT NOT NULL);
CREATE TABLE generation(id INTEGER PRIMARY KEY CHECK(id=1), space TEXT NOT NULL, chunk_count INTEGER NOT NULL CHECK(chunk_count BETWEEN 0 AND 512), body BLOB NOT NULL CHECK(length(body)<=8388608));
CREATE TABLE operations(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 id TEXT NOT NULL UNIQUE,
 kind TEXT NOT NULL, space TEXT NOT NULL, credential TEXT NOT NULL, request_hash TEXT NOT NULL,
 item_count INTEGER NOT NULL CHECK(item_count BETWEEN 1 AND 8),
 remote_id TEXT, state TEXT NOT NULL CHECK(state IN ('pending','unknown','completed','cleared')),
 receipt BLOB CHECK(receipt IS NULL OR length(receipt)<=98320), review_note TEXT,
 CHECK(receipt IS NULL OR state='completed')
);
CREATE UNIQUE INDEX one_unresolved_operation ON operations((1)) WHERE state IN ('pending','unknown');
";

fn pending_on(connection: &Connection) -> Result<Option<Operation>> {
    connection.query_row(
        "SELECT id,kind,space,credential,request_hash,item_count,remote_id,state FROM operations WHERE state IN ('pending','unknown')",
        [], |row| Ok(Operation { id: row.get(0)?, kind: row.get(1)?, space: row.get(2)?, credential: row.get(3)?, request_hash: row.get(4)?, count: row.get(5)?, remote_id: row.get(6)?, state: row.get(7)? }),
    ).optional().map_err(failure)
}

fn no_pending(connection: &Connection) -> Result<()> {
    if pending_on(connection)?.is_some() {
        return Err(failure(
            "needs_review: an embedding operation is unresolved",
        ));
    }
    Ok(())
}

impl Store {
    pub fn open(workspace: &Path, index_path: &Path) -> Result<Self> {
        let (workspace, path) = state_path(workspace, index_path)?;
        let ownership =
            private_state_file::open_or_create(&suffix(&path, ".lock")).map_err(failure)?;
        fs2::FileExt::try_lock_exclusive(&ownership).map_err(|_| {
            failure("another process owns semantic state; stop serve before using the CLI")
        })?;
        for sidecar in ["-wal", "-shm", "-journal"] {
            let file = suffix(&path, sidecar);
            match fs::symlink_metadata(&file) {
                Ok(_) => {
                    let metadata = private_state_file::inspect(&file).map_err(failure)?;
                    let limit = if sidecar == "-shm" {
                        4 * 1024 * 1024
                    } else {
                        2 * MAX_DATABASE_BYTES
                    };
                    if metadata.len() > limit {
                        return Err(failure("semantic sidecar exceeds its recovery size limit"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(failure(error)),
            }
        }
        let file = private_state_file::open_or_create(&path).map_err(failure)?;
        if file.metadata().map_err(failure)?.len() > MAX_DATABASE_BYTES {
            return Err(failure("semantic database exceeds 32 MiB"));
        }
        // Inspect the immutable database identity before SQLite can attempt any
        // recovery or journal creation. Nonempty unidentified databases are never
        // adopted, even when they happen to have no application tables.
        if file.metadata().map_err(failure)?.len() != 0 {
            let mut header = [0_u8; 100];
            file.try_clone()
                .map_err(failure)?
                .read_exact(&mut header)
                .map_err(failure)?;
            if &header[..16] != b"SQLite format 3\0"
                || i32::from_be_bytes(header[68..72].try_into().map_err(failure)?) != APPLICATION_ID
                || u32::from_be_bytes(header[60..64].try_into().map_err(failure)?)
                    != SCHEMA_VERSION as u32
            {
                return Err(failure(
                    "refusing an unknown or unsupported database header",
                ));
            }
        }
        // POSIX closes release this process's fcntl locks for the inode,
        // including locks taken by a different descriptor. Finish preflight
        // before SQLite opens the file, or another reader can unlink live WAL.
        drop(file);
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(failure)?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(failure)?;
        connection
            .execute_batch(
                "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
            )
            .map_err(failure)?;
        let application: i32 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(failure)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(failure)?;
        let new = application == 0 && version == 0;
        if new {
            let tables: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(failure)?;
            if tables != 0 {
                return Err(failure("refusing to adopt an unrelated database"));
            }
        } else {
            if application != APPLICATION_ID || version != SCHEMA_VERSION {
                return Err(failure("unsupported semantic database identity or schema"));
            }
            let bound: String = connection
                .query_row(
                    "SELECT workspace FROM semantic_meta WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(failure)?;
            if bound
                != workspace
                    .to_str()
                    .ok_or_else(|| failure("invalid workspace"))?
            {
                return Err(failure(
                    "semantic database belongs to a different workspace",
                ));
            }
        }
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(failure)?;
        let pages: u64 = connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .map_err(failure)?;
        if page_size == 0 || pages.saturating_mul(page_size) > MAX_DATABASE_BYTES {
            return Err(failure("semantic database capacity exceeded"));
        }
        connection
            .pragma_update(None, "max_page_count", MAX_DATABASE_BYTES / page_size)
            .map_err(failure)?;
        if new {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failure)?;
            tx.execute_batch(SCHEMA).map_err(failure)?;
            tx.execute(
                "INSERT INTO semantic_meta(id,workspace) VALUES(1,?1)",
                [workspace
                    .to_str()
                    .ok_or_else(|| failure("invalid workspace"))?],
            )
            .map_err(failure)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(failure)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(failure)?;
            tx.commit().map_err(failure)?;
        }
        let health: String = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(failure)?;
        if health != "ok" {
            return Err(failure("semantic database integrity check failed"));
        }
        let total: i64 = connection
            .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
            .map_err(failure)?;
        if total > MAX_OPERATIONS {
            return Err(failure("semantic operation ledger limit exceeded"));
        }
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(failure)?;
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(failure)?;
        if journal != "wal" {
            return Err(failure("semantic state requires WAL support"));
        }
        connection
            .execute_batch("PRAGMA wal_autocheckpoint=64; PRAGMA journal_size_limit=33554432;")
            .map_err(failure)?;
        // The old process may have sent the request. Never infer not_sent from a restart.
        connection
            .execute(
                "UPDATE operations SET state='unknown' WHERE state='pending'",
                [],
            )
            .map_err(failure)?;
        #[cfg(unix)]
        File::open(
            path.parent()
                .ok_or_else(|| failure("invalid state parent"))?,
        )
        .and_then(|file| file.sync_all())
        .map_err(failure)?;
        Ok(Self {
            connection: Mutex::new(connection),
            _ownership: ownership,
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| failure("semantic database mutex poisoned"))
    }

    pub fn load_generation(&self, space: &str) -> Result<Option<Generation>> {
        fingerprint(space, "embedding space")?;
        let connection = self.connection()?;
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT body FROM generation WHERE id=1 AND space=?1",
                [space],
                |row| row.get(0),
            )
            .optional()
            .map_err(failure)?;
        bytes
            .map(|bytes| {
                if bytes.len() > MAX_BLOB_BYTES {
                    return Err(failure("generation exceeds serialized limit"));
                }
                let generation = decode_generation(&bytes)?;
                generation_valid(&generation)?;
                if generation.space != space {
                    return Err(failure("generation space mismatch"));
                }
                Ok(generation)
            })
            .transpose()
    }

    pub fn publish_generation(&self, generation: &Generation) -> Result<()> {
        generation_valid(generation)?;
        let body = encode_generation(generation)?;
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        no_pending(&tx)?;
        tx.execute("INSERT INTO generation(id,space,chunk_count,body) VALUES(1,?1,?2,?3) ON CONFLICT(id) DO UPDATE SET space=excluded.space,chunk_count=excluded.chunk_count,body=excluded.body", params![generation.space, generation.chunks.len(), body]).map_err(failure)?;
        tx.commit().map_err(failure)
    }

    pub fn clear_generation(&self) -> Result<()> {
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        no_pending(&tx)?;
        tx.execute("DELETE FROM generation", []).map_err(failure)?;
        tx.commit().map_err(failure)
    }

    pub fn begin_operation(
        &self,
        kind: &str,
        space: &str,
        credential: &str,
        request_hash: &str,
        count: usize,
    ) -> Result<String> {
        bounded(kind, 32, "operation kind")?;
        fingerprint(space, "embedding space")?;
        fingerprint(credential, "credential")?;
        fingerprint(request_hash, "request hash")?;
        if count == 0 || count > MAX_BATCH {
            return Err(failure("operation count must be 1..8"));
        }
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        no_pending(&tx)?;
        tx.execute("DELETE FROM operations WHERE state IN ('completed','cleared') AND seq NOT IN (SELECT seq FROM operations WHERE state IN ('completed','cleared') ORDER BY seq DESC LIMIT ?1)", [RETAIN_COMPLETED]).map_err(failure)?;
        let total: i64 = tx
            .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
            .map_err(failure)?;
        if total >= MAX_OPERATIONS {
            return Err(failure("semantic operation ledger is full"));
        }
        let id = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO operations(id,kind,space,credential,request_hash,item_count,state) VALUES(?1,?2,?3,?4,?5,?6,'pending')", params![id,kind,space,credential,request_hash,count]).map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(id)
    }

    /// Only for a positively established no-result terminal outcome. Unknown
    /// submissions require recovery or explicit review, never this shortcut.
    #[cfg(test)]
    pub fn finish_operation(&self, id: &str) -> Result<()> {
        operation_id(id)?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE operations SET state='completed' WHERE id=?1 AND state='pending'",
                [id],
            )
            .map_err(failure)?;
        if changed != 1 {
            return Err(failure("operation is not pending"));
        }
        Ok(())
    }

    pub fn mark_unknown(&self, id: &str, remote_id: Option<&str>) -> Result<()> {
        operation_id(id)?;
        if let Some(remote) = remote_id {
            bounded(remote, 200, "remote operation ID")?;
        }
        let connection = self.connection()?;
        let changed = connection.execute("UPDATE operations SET state='unknown',remote_id=coalesce(remote_id,?2) WHERE id=?1 AND state IN ('pending','unknown') AND (remote_id IS NULL OR ?2 IS NULL OR remote_id=?2)", params![id,remote_id]).map_err(failure)?;
        if changed != 1 {
            return Err(failure(
                "operation is resolved or remote identity conflicts",
            ));
        }
        Ok(())
    }

    pub fn pending(&self) -> Result<Option<Operation>> {
        let connection = self.connection()?;
        pending_on(&connection)
    }

    pub fn complete_operation(
        &self,
        id: &str,
        remote_id: Option<&str>,
        vectors: &[Vec<f32>],
    ) -> Result<()> {
        operation_id(id)?;
        if let Some(remote) = remote_id {
            bounded(remote, 200, "remote operation ID")?;
        }
        if vectors.len() > MAX_BATCH {
            return Err(failure("receipt batch limit exceeded"));
        }
        vectors_valid(vectors, vectors.len())?;
        let receipt = encode_receipt(vectors)?;
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let pending =
            pending_on(&tx)?.ok_or_else(|| failure("operation is already resolved or missing"))?;
        if pending.id != id
            || pending.count != vectors.len()
            || pending
                .remote_id
                .as_deref()
                .zip(remote_id)
                .is_some_and(|(old, new)| old != new)
        {
            return Err(failure("operation receipt identity/count mismatch"));
        }
        tx.execute("UPDATE operations SET receipt=?2,remote_id=coalesce(remote_id,?3),state='completed' WHERE id=?1", params![id,receipt,remote_id]).map_err(failure)?;
        tx.commit().map_err(failure)
    }

    #[cfg(test)]
    pub fn receipt(&self, id: &str) -> Result<Option<Vec<Vec<f32>>>> {
        operation_id(id)?;
        let connection = self.connection()?;
        let row: Option<(usize, Option<Vec<u8>>)> = connection
            .query_row(
                "SELECT item_count,receipt FROM operations WHERE id=?1 AND state='completed'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(failure)?;
        let Some((count, Some(bytes))) = row else {
            return Ok(None);
        };
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(failure("receipt exceeds serialized limit"));
        }
        Ok(Some(decode_receipt(&bytes, count)?))
    }

    /// Reuse only a confirmed receipt for the exact space and original POST
    /// body hash. A cache hit cannot conceal a globally unresolved submission.
    pub fn cached_receipt(&self, space: &str, request_hash: &str) -> Result<Option<Vec<Vec<f32>>>> {
        fingerprint(space, "embedding space")?;
        fingerprint(request_hash, "request hash")?;
        let connection = self.connection()?;
        no_pending(&connection)?;
        let row: Option<(usize,Vec<u8>)> = connection.query_row(
            "SELECT item_count,receipt FROM operations WHERE space=?1 AND request_hash=?2 AND state='completed' AND receipt IS NOT NULL ORDER BY seq DESC LIMIT 1",
            params![space,request_hash], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(failure)?;
        let Some((count, bytes)) = row else {
            return Ok(None);
        };
        if count > MAX_BATCH || bytes.len() > MAX_BLOB_BYTES {
            return Err(failure("cached receipt exceeds limits"));
        }
        Ok(Some(decode_receipt(&bytes, count)?))
    }

    /// Administrative authorization is supplied by the CLI, after independent
    /// remote reconciliation. A live pending request cannot be cleared here.
    pub fn review_clear(&self, id: &str, note: &str) -> Result<()> {
        operation_id(id)?;
        bounded(note, 1024, "review note")?;
        let connection = self.connection()?;
        let changed = connection.execute("UPDATE operations SET state='cleared',review_note=?2 WHERE id=?1 AND state='unknown'", params![id,note]).map_err(failure)?;
        if changed != 1 {
            return Err(failure("review requires the exact unknown operation ID"));
        }
        Ok(())
    }

    /// No text, vectors, request bodies, credential fingerprints or request hashes.
    pub fn status(&self) -> Result<Value> {
        let connection = self.connection()?;
        let generation: Option<(String, usize, usize)> = connection
            .query_row(
                "SELECT space,chunk_count,length(body) FROM generation WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(failure)?;
        let total: i64 = connection
            .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
            .map_err(failure)?;
        let pending = pending_on(&connection)?.map(|op| json!({"id":op.id,"kind":op.kind,"state":op.state,"count":op.count,"remote_id":op.remote_id}));
        Ok(
            json!({"schema_version":SCHEMA_VERSION,"database_limit_bytes":MAX_DATABASE_BYTES,
            "generation":generation.map(|(space,chunks,bytes)| json!({"space":space,"chunks":chunks,"bytes":bytes})),
            "retained_operations":total,"pending":pending}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint_for(byte: char) -> String {
        byte.to_string().repeat(64)
    }
    fn workspace(temp: &tempfile::TempDir) -> PathBuf {
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        workspace
    }
    fn index() -> &'static Path {
        Path::new("../semantic/index.sqlite3")
    }
    fn generation() -> Generation {
        Generation {
            space: fingerprint_for('a'),
            manifest: fingerprint_for('b'),
            chunks: vec![StoredChunk {
                path: "MEMORY.md".into(),
                line: 1,
                text: "private note".into(),
                hash: fingerprint_for('c'),
                vector: vec![0.25, 0.75],
            }],
        }
    }
    fn begin(store: &Store) -> String {
        store
            .begin_operation(
                "refresh",
                &fingerprint_for('a'),
                &fingerprint_for('d'),
                &fingerprint_for('e'),
                1,
            )
            .unwrap()
    }

    #[test]
    fn private_external_state_has_identity_binding_capacity_and_lifetime_lock() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Store>();
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        assert!(Store::open(&workspace, Path::new("index.sqlite3")).is_err());
        assert!(!workspace.join("index.sqlite3").exists());
        let store = Store::open(&workspace, index()).unwrap();
        assert!(Store::open(&workspace, index()).is_err());
        let connection = store.connection().unwrap();
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        let synchronous: i64 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        let timeout: i64 = connection
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .unwrap();
        let max_pages: u64 = connection
            .pragma_query_value(None, "max_page_count", |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        assert_eq!(synchronous, 2);
        assert_eq!(timeout, 250);
        assert_eq!(max_pages * page_size, MAX_DATABASE_BYTES);
        drop(connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join("semantic"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            for name in [
                "index.sqlite3",
                "index.sqlite3.lock",
                "index.sqlite3-wal",
                "index.sqlite3-shm",
            ] {
                assert_eq!(
                    fs::metadata(temp.path().join("semantic").join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        drop(store);
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(Store::open(&other, index()).is_err());
        Store::open(&workspace, index()).unwrap();
    }

    #[test]
    fn unrelated_databases_and_unsupported_schema_are_not_reset() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        drop(store);
        let path = temp.path().join("semantic/index.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "application_id", 123).unwrap();
        conn.execute("CREATE TABLE foreign_data(body TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO foreign_data VALUES('preserve me')", [])
            .unwrap();
        drop(conn);
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&workspace, index()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("SELECT body FROM foreign_data", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "preserve me"
        );
        conn.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&workspace, index()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn generation_is_atomic_bounded_and_separate_from_operation_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        let original = generation();
        store.publish_generation(&original).unwrap();
        let mut invalid = original.clone();
        invalid.chunks[0].text = "x".repeat(1025);
        assert!(store.publish_generation(&invalid).is_err());
        invalid = original.clone();
        invalid.chunks[0].vector[0] = f32::NAN;
        assert!(store.publish_generation(&invalid).is_err());
        let mut oversized = original.clone();
        oversized.chunks[0].vector = vec![f32::MAX; MAX_DIMENSIONS];
        oversized.chunks = vec![oversized.chunks[0].clone(); MAX_CHUNKS + 1];
        assert!(store.publish_generation(&oversized).is_err());
        assert_eq!(
            store.load_generation(&original.space).unwrap(),
            Some(original.clone())
        );
        assert!(store
            .load_generation(&fingerprint_for('f'))
            .unwrap()
            .is_none());
        let id = begin(&store);
        assert!(store.publish_generation(&original).is_err());
        assert!(store.clear_generation().is_err());
        assert!(store
            .begin_operation(
                "query",
                &fingerprint_for('f'),
                &fingerprint_for('f'),
                &fingerprint_for('f'),
                1
            )
            .is_err());
        assert!(store
            .cached_receipt(&original.space, &fingerprint_for('e'))
            .is_err());
        assert!(store
            .complete_operation(&id, Some("remote"), &[vec![f32::NAN]])
            .is_err());
        assert!(store
            .complete_operation(&id, Some("remote"), &[vec![1.0], vec![1.0]])
            .is_err());
        store
            .complete_operation(&id, Some("remote"), &[vec![0.25, 0.75]])
            .unwrap();
        assert!(store.pending().unwrap().is_none());
        assert_eq!(store.receipt(&id).unwrap(), Some(vec![vec![0.25, 0.75]]));
        assert_eq!(
            store
                .cached_receipt(&original.space, &fingerprint_for('e'))
                .unwrap(),
            Some(vec![vec![0.25, 0.75]])
        );
        assert!(store
            .cached_receipt(&original.space, &fingerprint_for('f'))
            .unwrap()
            .is_none());
        assert!(store
            .cached_receipt(&fingerprint_for('f'), &fingerprint_for('e'))
            .unwrap()
            .is_none());
        store.clear_generation().unwrap();
        assert!(store.load_generation(&original.space).unwrap().is_none());
        drop(store);
        let store = Store::open(&workspace, index()).unwrap();
        assert_eq!(store.receipt(&id).unwrap(), Some(vec![vec![0.25, 0.75]]));
    }

    #[test]
    fn restart_requires_review_and_preserves_remote_identity_without_retry() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        let id = begin(&store);
        assert!(store.review_clear(&id, "too soon").is_err());
        drop(store);
        let store = Store::open(&workspace, index()).unwrap();
        assert_eq!(store.pending().unwrap().unwrap().state, "unknown");
        assert!(store.finish_operation(&id).is_err());
        store.mark_unknown(&id, Some("original-remote")).unwrap();
        assert!(store.mark_unknown(&id, Some("different-remote")).is_err());
        assert!(store
            .complete_operation(&id, Some("different-remote"), &[vec![1.0]])
            .is_err());
        assert_eq!(
            store.pending().unwrap().unwrap().remote_id.as_deref(),
            Some("original-remote")
        );
        assert!(store.review_clear(&id, "").is_err());
        assert!(store.review_clear(&id, &"x".repeat(1025)).is_err());
        assert!(store
            .review_clear(&Uuid::new_v4().to_string(), "wrong ID")
            .is_err());
        store
            .review_clear(
                &id,
                "Operator reconciled the original operation before clearing.",
            )
            .unwrap();
        assert!(store.pending().unwrap().is_none());
        let next = begin(&store);
        assert_ne!(id, next);
    }

    #[test]
    fn failed_receipt_commit_retains_hold_and_no_partial_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        let id = begin(&store);
        store.connection().unwrap().execute_batch("CREATE TRIGGER fail_receipt BEFORE UPDATE OF receipt ON operations BEGIN SELECT RAISE(ABORT,'fixture transaction failure'); END;").unwrap();
        assert!(store
            .complete_operation(&id, Some("remote"), &[vec![1.0]])
            .is_err());
        assert_eq!(store.pending().unwrap().unwrap().id, id);
        assert!(store.receipt(&id).unwrap().is_none());
        store
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_receipt")
            .unwrap();
        store
            .complete_operation(&id, Some("remote"), &[vec![1.0]])
            .unwrap();
        assert!(store.pending().unwrap().is_none());
        assert_eq!(store.receipt(&id).unwrap(), Some(vec![vec![1.0]]));
    }

    #[test]
    fn sqlite_full_retains_durable_unresolved_operation() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        let id = store
            .begin_operation(
                "refresh",
                &fingerprint_for('a'),
                &fingerprint_for('b'),
                &fingerprint_for('c'),
                8,
            )
            .unwrap();
        {
            let connection = store.connection().unwrap();
            let pages: u64 = connection
                .pragma_query_value(None, "page_count", |row| row.get(0))
                .unwrap();
            connection
                .pragma_update(None, "max_page_count", pages)
                .unwrap();
        }
        let error = store
            .complete_operation(
                &id,
                Some("remote"),
                &vec![vec![f32::MAX; MAX_DIMENSIONS]; 8],
            )
            .unwrap_err();
        assert!(error.to_string().contains("disk is full"), "{error}");
        assert_eq!(store.pending().unwrap().unwrap().id, id);
        assert!(store.receipt(&id).unwrap().is_none());
        drop(store);
        let store = Store::open(&workspace, index()).unwrap();
        assert_eq!(store.pending().unwrap().unwrap().state, "unknown");
        assert_eq!(store.pending().unwrap().unwrap().id, id);
    }

    #[test]
    fn terminal_ledger_pruning_never_removes_unresolved_and_status_contains_no_content() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        store.publish_generation(&generation()).unwrap();
        let first = begin(&store);
        store.finish_operation(&first).unwrap();
        for _ in 0..70 {
            let id = begin(&store);
            store.finish_operation(&id).unwrap();
        }
        let id = begin(&store);
        let status = store.status().unwrap();
        assert_eq!(status["retained_operations"], 65);
        let text = status.to_string();
        for private in [
            "private note",
            "vector",
            "credential",
            "request_hash",
            &fingerprint_for('d'),
            &fingerprint_for('e'),
        ] {
            assert!(!text.contains(private));
        }
        store.mark_unknown(&id, None).unwrap();
        assert!(store.clear_generation().is_err());
        assert!(store
            .begin_operation(
                "refresh",
                &fingerprint_for('f'),
                &fingerprint_for('f'),
                &fingerprint_for('f'),
                8
            )
            .is_err());
        assert_eq!(store.pending().unwrap().unwrap().id, id);
        drop(store);
        let store = Store::open(&workspace, index()).unwrap();
        assert_eq!(store.pending().unwrap().unwrap().id, id);
    }

    #[cfg(unix)]
    #[test]
    fn directory_file_sidecar_links_permissions_and_special_files_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        for suffix_name in ["", ".lock", "-wal", "-shm", "-journal"] {
            for kind in ["symlink", "hardlink", "socket"] {
                let temp = tempfile::tempdir().unwrap();
                let workspace = workspace(&temp);
                let directory = temp.path().join("semantic");
                fs::create_dir(&directory).unwrap();
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
                let outside = temp.path().join("outside");
                fs::write(&outside, "never touch").unwrap();
                fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
                let target = directory.join(format!("index.sqlite3{suffix_name}"));
                let mut socket = None;
                match kind {
                    "symlink" => symlink(&outside, &target).unwrap(),
                    "hardlink" => fs::hard_link(&outside, &target).unwrap(),
                    _ => {
                        socket = Some(std::os::unix::net::UnixListener::bind(&target).unwrap());
                    }
                }
                let error = Store::open(&workspace, index())
                    .err()
                    .expect("unsafe leaf accepted");
                let expected = if kind == "hardlink" {
                    "semantic store: state files require mode 0600 and exactly one hard link"
                } else {
                    "semantic store: state files must be ordinary files without symlinks"
                };
                assert!(
                    matches!(error, JiaClawError::ToolExecution(message) if message == expected)
                );
                assert_eq!(fs::read_to_string(outside).unwrap(), "never touch");
                drop(socket);
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&outside, temp.path().join("semantic")).unwrap();
        assert!(Store::open(&workspace, index()).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(temp.path().join("semantic")).unwrap();
        fs::create_dir(temp.path().join("semantic")).unwrap();
        fs::set_permissions(
            temp.path().join("semantic"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(Store::open(&workspace, index()).is_err());
        assert_eq!(
            fs::metadata(temp.path().join("semantic"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
    #[cfg(unix)]
    #[test]
    fn oversized_recovery_sidecars_are_rejected_before_database_creation() {
        use std::os::unix::fs::PermissionsExt;
        for (name, limit) in [
            ("-wal", 2 * MAX_DATABASE_BYTES),
            ("-journal", 2 * MAX_DATABASE_BYTES),
            ("-shm", 4 * 1024 * 1024),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let workspace = workspace(&temp);
            let directory = temp.path().join("semantic");
            fs::create_dir(&directory).unwrap();
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            let sidecar =
                private_state_file::open_or_create(&directory.join(format!("index.sqlite3{name}")))
                    .unwrap();
            sidecar.set_len(limit + 1).unwrap();
            drop(sidecar);
            assert!(Store::open(&workspace, index()).is_err());
            assert!(!directory.join("index.sqlite3").exists());
        }
    }
    #[test]
    fn maximum_generation_receipts_and_replacements_fit_without_growth() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        let vectors = vec![vec![f32::MAX; MAX_DIMENSIONS]; MAX_BATCH];
        assert_eq!(encode_receipt(&vectors).unwrap().len(), MAX_RECEIPT_BYTES);
        let mut maximum = generation();
        let chunk = StoredChunk {
            path: "p".repeat(1024),
            line: usize::MAX,
            text: "\0".repeat(1024),
            hash: fingerprint_for('c'),
            vector: vec![f32::MAX; MAX_DIMENSIONS],
        };
        maximum.chunks = vec![chunk; MAX_CHUNKS];
        assert_eq!(
            encode_generation(&maximum).unwrap().len(),
            MAX_GENERATION_BYTES
        );
        assert!(2 * MAX_GENERATION_BYTES + 65 * MAX_RECEIPT_BYTES < MAX_DATABASE_BYTES as usize);
        store.publish_generation(&maximum).unwrap();
        for number in 0..65 {
            let id = store
                .begin_operation(
                    &"k".repeat(32),
                    &maximum.space,
                    &fingerprint_for('d'),
                    &format!("{number:064x}"),
                    MAX_BATCH,
                )
                .unwrap();
            store
                .complete_operation(&id, Some(&"r".repeat(200)), &vectors)
                .unwrap();
        }
        assert_eq!(store.status().unwrap()["retained_operations"], 65);
        let original = maximum.clone();
        maximum.manifest = fingerprint_for('f');
        store.connection().unwrap().execute_batch("CREATE TRIGGER fail_generation BEFORE UPDATE ON generation BEGIN SELECT RAISE(ABORT,'fixture generation failure'); END;").unwrap();
        assert!(store.publish_generation(&maximum).is_err());
        assert_eq!(
            store.load_generation(&original.space).unwrap(),
            Some(original)
        );
        store
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_generation")
            .unwrap();
        let mut sizes = Vec::new();
        for number in 0..6 {
            maximum.manifest = format!("{number:064x}");
            for chunk in &mut maximum.chunks {
                chunk.text = char::from(b'a' + number).to_string().repeat(1024);
                chunk
                    .vector
                    .fill(if number % 2 == 0 { -f32::MAX } else { f32::MAX });
            }
            store.publish_generation(&maximum).unwrap();
            assert_eq!(
                store.load_generation(&maximum.space).unwrap(),
                Some(maximum.clone())
            );
            let connection = store.connection().unwrap();
            let page_size: u64 = connection
                .pragma_query_value(None, "page_size", |row| row.get(0))
                .unwrap();
            let page_count: u64 = connection
                .pragma_query_value(None, "page_count", |row| row.get(0))
                .unwrap();
            assert!(page_size * page_count <= MAX_DATABASE_BYTES);
            sizes.push(page_count);
        }
        assert!(
            sizes[2..].windows(2).all(|pair| pair[0] == pair[1]),
            "repeated replacements grew: {sizes:?}"
        );
        drop(store);
        let store = Store::open(&workspace, index()).unwrap();
        assert_eq!(
            store.load_generation(&maximum.space).unwrap(),
            Some(maximum)
        );
        assert_eq!(
            store
                .cached_receipt(&fingerprint_for('a'), &format!("{:064x}", 64))
                .unwrap(),
            Some(vectors)
        );
        assert_eq!(store.status().unwrap()["retained_operations"], 65);
    }

    #[test]
    fn binary_codec_rejects_corrupt_or_ambiguous_data_and_preserves_float_bits() {
        let generation = generation();
        let bytes = encode_generation(&generation).unwrap();
        assert_eq!(decode_generation(&bytes).unwrap(), generation);
        let mut invalid = bytes.clone();
        invalid.push(0);
        assert!(decode_generation(&invalid).is_err());
        assert!(decode_generation(&bytes[..bytes.len() - 1]).is_err());
        for (offset, value) in [(8, u32::MAX), (144, u32::MAX), (148, u32::MAX)] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode_generation(&invalid).is_err());
        }
        let mut invalid = bytes.clone();
        invalid[0] = 0;
        assert!(decode_generation(&invalid).is_err());
        let mut invalid = bytes;
        invalid[12] = 0xff;
        assert!(decode_generation(&invalid).is_err());
        let vectors = vec![vec![
            -0.0,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
        ]];
        let encoded = encode_receipt(&vectors).unwrap();
        let decoded = decode_receipt(&encoded, 1).unwrap();
        assert_eq!(
            vectors[0]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            decoded[0]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert!(decode_receipt(&encoded, 2).is_err());
        assert!(decode_receipt(&encoded[..encoded.len() - 1], 1).is_err());
        let mut invalid = encoded.clone();
        invalid.push(0);
        assert!(decode_receipt(&invalid, 1).is_err());
        let mut invalid = encoded.clone();
        invalid[16..20].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_receipt(&invalid, 1).is_err());
        let mut invalid = encoded;
        invalid[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_receipt(&invalid, 1).is_err());
    }

    #[test]
    fn old_unpublished_json_schema_is_rejected_without_clearing_its_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(&temp);
        let store = Store::open(&workspace, index()).unwrap();
        let id = begin(&store);
        drop(store);
        let path = temp.path().join("semantic/index.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&workspace, index()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT state FROM operations WHERE id=?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "pending"
        );
    }
}
