// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Private model-call evidence, never a replayable prompt or tool-runtime log.
//! Administrator-owned storage must not be relocated, rolled back or replaced
//! to bypass unresolved submissions. The owner keeps this store alive until
//! admitted network work and its final persistence have finished.

use jiaclaw_core::{JiaClawError, ModelPurpose};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, JiaClawError>;
const APPLICATION_ID: i32 = 0x4a43_4d43;
const SCHEMA_VERSION: i64 = 1;
const MAX_DATABASE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RECEIPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_OPERATIONS: i64 = 128;
const MAX_AUDIT: i64 = 256;
const RETAIN_RECEIPTS: i64 = 8;

#[derive(Clone, Debug)]
pub(super) struct NewCall {
    pub id: String,
    pub turn_id: String,
    pub purpose: ModelPurpose,
    pub session_hash: Option<String>,
    pub round: u32,
    pub model: String,
    pub has_tools: bool,
    pub endpoint_hash: String,
    pub credential_hash: String,
    pub body_hash: String,
}

/// Metadata only. Receipt contents are returned only by explicit show_result.
#[derive(Clone, Debug)]
pub(super) struct Operation {
    pub id: String,
    pub turn_id: String,
    pub purpose: ModelPurpose,
    pub session_hash: Option<String>,
    pub round: u32,
    pub model: String,
    pub has_tools: bool,
    pub endpoint_hash: String,
    pub credential_hash: String,
    pub body_hash: String,
    pub remote_id: Option<String>,
    pub state: String,
    pub recovered: bool,
}

pub(super) struct Store {
    connection: Mutex<Connection>,
    _ownership: File,
}

fn failure(message: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("model-call store: {message}"))
}

fn bounded(value: &str, maximum: usize, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(failure(format!("invalid {label}")));
    }
    Ok(())
}

fn canonical_id(value: &str) -> Result<()> {
    if Uuid::parse_str(value)
        .ok()
        .is_none_or(|id| id.to_string() != value)
    {
        return Err(failure("expected a canonical UUID"));
    }
    Ok(())
}

fn fingerprint(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(failure("expected a SHA-256 fingerprint"));
    }
    Ok(())
}

fn purpose_name(value: ModelPurpose) -> &'static str {
    match value {
        ModelPurpose::Chat => "chat",
        ModelPurpose::Channel => "channel",
        ModelPurpose::Scheduled => "scheduled",
        ModelPurpose::Heartbeat => "heartbeat",
        ModelPurpose::Summary => "summary",
    }
}

impl NewCall {
    fn validate(&self) -> Result<()> {
        canonical_id(&self.id)?;
        canonical_id(&self.turn_id)?;
        if let Some(hash) = &self.session_hash {
            fingerprint(hash)?;
        }
        for hash in [&self.endpoint_hash, &self.credential_hash, &self.body_hash] {
            fingerprint(hash)?;
        }
        bounded(&self.model, 200, "model")?;
        if self.round >= 32 {
            return Err(failure("model round must be 0..31"));
        }
        Ok(())
    }
}

fn private_file(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).map_err(failure)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(failure(
            "state files must be ordinary files without symlinks",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(failure(
                "state files require mode 0600 and exactly one hard link",
            ));
        }
    }
    Ok(metadata)
}

fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn create_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            private_file(path)?;
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            options.open(path).map_err(failure)?
        }
        Err(error) => return Err(failure(error)),
    };
    let metadata = private_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata().map_err(failure)?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() || opened.nlink() != 1 {
            return Err(failure("state file changed during open"));
        }
    }
    Ok(file)
}

fn state_path(workspace: &Path, requested: &Path) -> Result<(PathBuf, PathBuf)> {
    let workspace = workspace.canonicalize().map_err(failure)?;
    if !workspace.is_dir() || workspace.to_str().is_none() {
        return Err(failure("workspace must be a canonical UTF-8 directory"));
    }
    if requested.as_os_str().is_empty() {
        return Err(failure("model-call state path is empty"));
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
            "model-call state must be outside the agent workspace",
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
CREATE TABLE model_call_meta(id INTEGER PRIMARY KEY CHECK(id=1),workspace TEXT NOT NULL);
CREATE TABLE model_calls(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 id TEXT UNIQUE NOT NULL CHECK(length(id)=36),
 turn_id TEXT NOT NULL CHECK(length(turn_id)=36),
 purpose TEXT NOT NULL CHECK(purpose IN ('chat','channel','scheduled','heartbeat','summary')),
 session_hash TEXT CHECK(session_hash IS NULL OR length(session_hash)=64),
 round INTEGER NOT NULL CHECK(round BETWEEN 0 AND 31),
 model TEXT NOT NULL CHECK(length(CAST(model AS BLOB)) BETWEEN 1 AND 200),
 has_tools INTEGER NOT NULL CHECK(has_tools IN (0,1)),
 endpoint_hash TEXT NOT NULL CHECK(length(endpoint_hash)=64),
 credential_hash TEXT NOT NULL CHECK(length(credential_hash)=64),
 body_hash TEXT NOT NULL CHECK(length(body_hash)=64),
 remote_id TEXT UNIQUE CHECK(remote_id IS NULL OR length(remote_id)=36),
 state TEXT NOT NULL CHECK(state IN ('submitting','unknown','completed','cleared')),
 recovered INTEGER NOT NULL DEFAULT 0 CHECK(recovered IN (0,1)),
 receipt BLOB CHECK(receipt IS NULL OR length(receipt)<=2097152),
 reservation BLOB,
 UNIQUE(turn_id,purpose,round),
 CHECK((state IN ('submitting','unknown') AND receipt IS NULL AND reservation IS NOT NULL AND length(reservation)=2097152)
    OR (state IN ('completed','cleared') AND reservation IS NULL)),
 CHECK(receipt IS NULL OR state='completed'),
 CHECK(recovered=0 OR state='completed')
);
CREATE UNIQUE INDEX one_unresolved_model_call ON model_calls((1)) WHERE state IN ('submitting','unknown');
CREATE TABLE model_call_audit(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 operation_id TEXT NOT NULL CHECK(length(operation_id)=36),
 action TEXT NOT NULL CHECK(action IN ('admitted','remote_recorded','unknown','completed','recovered','review_cleared','restart_unknown')),
 note TEXT CHECK(note IS NULL OR length(CAST(note AS BLOB)) BETWEEN 1 AND 1024)
);
";

const COLUMNS: &str = "id,turn_id,purpose,session_hash,round,model,has_tools,endpoint_hash,credential_hash,body_hash,remote_id,state,recovered";

fn decode(row: &Row<'_>) -> rusqlite::Result<Operation> {
    let purpose: String = row.get(2)?;
    let purpose = match purpose.as_str() {
        "chat" => ModelPurpose::Chat,
        "channel" => ModelPurpose::Channel,
        "scheduled" => ModelPurpose::Scheduled,
        "heartbeat" => ModelPurpose::Heartbeat,
        "summary" => ModelPurpose::Summary,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(Operation {
        id: row.get(0)?,
        turn_id: row.get(1)?,
        purpose,
        session_hash: row.get(3)?,
        round: row.get(4)?,
        model: row.get(5)?,
        has_tools: row.get(6)?,
        endpoint_hash: row.get(7)?,
        credential_hash: row.get(8)?,
        body_hash: row.get(9)?,
        remote_id: row.get(10)?,
        state: row.get(11)?,
        recovered: row.get(12)?,
    })
}

fn pending_on(conn: &Connection) -> Result<Option<Operation>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM model_calls WHERE state IN ('submitting','unknown')"),
        [],
        decode,
    )
    .optional()
    .map_err(failure)
}

fn audit(conn: &Connection, id: &str, action: &str, note: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO model_call_audit(operation_id,action,note) VALUES(?1,?2,?3)",
        params![id, action, note],
    )
    .map_err(failure)?;
    conn.execute("DELETE FROM model_call_audit WHERE seq NOT IN (SELECT seq FROM model_call_audit ORDER BY seq DESC LIMIT ?1)", [MAX_AUDIT]).map_err(failure)?;
    Ok(())
}

fn prune_receipts(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE model_calls SET receipt=NULL WHERE receipt IS NOT NULL AND seq NOT IN (SELECT seq FROM model_calls WHERE receipt IS NOT NULL ORDER BY seq DESC LIMIT ?1)", [RETAIN_RECEIPTS]).map_err(failure)?;
    Ok(())
}

struct ReceiptWriter(Vec<u8>);
impl Write for ReceiptWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RECEIPT_BYTES {
            return Err(std::io::Error::other("receipt exceeds 2 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode_receipt(value: &Value) -> Result<Vec<u8>> {
    if !value.is_object() {
        return Err(failure("validated receipt must be a JSON object"));
    }
    let mut output = ReceiptWriter(Vec::new());
    serde_json::to_writer(&mut output, value)
        .map_err(|_| failure("receipt exceeds 2 MiB or cannot be encoded"))?;
    Ok(output.0)
}

impl Store {
    pub fn open(workspace: &Path, requested: &Path) -> Result<Self> {
        let (workspace, path) = state_path(workspace, requested)?;
        let ownership = create_private_file(&suffix(&path, ".lock"))?;
        fs2::FileExt::try_lock_exclusive(&ownership).map_err(|_| {
            failure("another process owns model-call state; stop serve before using the CLI")
        })?;
        for suffix_name in ["-wal", "-shm", "-journal"] {
            let sidecar = suffix(&path, suffix_name);
            match fs::symlink_metadata(&sidecar) {
                Ok(_) => {
                    let metadata = private_file(&sidecar)?;
                    let maximum = if suffix_name == "-shm" {
                        4 * 1024 * 1024
                    } else {
                        2 * MAX_DATABASE_BYTES
                    };
                    if metadata.len() > maximum {
                        return Err(failure("model-call sidecar exceeds recovery size limit"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(failure(error)),
            }
        }
        let file = create_private_file(&path)?;
        let size = file.metadata().map_err(failure)?.len();
        if size > MAX_DATABASE_BYTES {
            return Err(failure("model-call database exceeds 32 MiB"));
        }
        if size != 0 {
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
        let mut conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(failure)?;
        conn.busy_timeout(Duration::from_millis(250))
            .map_err(failure)?;
        conn.execute_batch(
            "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
        )
        .map_err(failure)?;
        let application: i32 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(failure)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(failure)?;
        let new = application == 0 && version == 0;
        if new {
            let tables: i64 = conn
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
                return Err(failure(
                    "unsupported model-call database identity or schema",
                ));
            }
            let bound: String = conn
                .query_row(
                    "SELECT workspace FROM model_call_meta WHERE id=1",
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
                    "model-call database belongs to a different workspace",
                ));
            }
        }
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(failure)?;
        let page_count: u64 = conn
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .map_err(failure)?;
        if page_size == 0 || page_count.saturating_mul(page_size) > MAX_DATABASE_BYTES {
            return Err(failure("model-call database capacity exceeded"));
        }
        conn.pragma_update(None, "max_page_count", MAX_DATABASE_BYTES / page_size)
            .map_err(failure)?;
        if new {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failure)?;
            tx.execute_batch(SCHEMA).map_err(failure)?;
            tx.execute(
                "INSERT INTO model_call_meta(id,workspace) VALUES(1,?1)",
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
        let health: String = conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(failure)?;
        if health != "ok" {
            return Err(failure("model-call database integrity check failed"));
        }
        let (operations, receipts, audits): (i64,i64,i64) = conn.query_row("SELECT (SELECT count(*) FROM model_calls),(SELECT count(*) FROM model_calls WHERE receipt IS NOT NULL),(SELECT count(*) FROM model_call_audit)", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(failure)?;
        if operations > MAX_OPERATIONS || receipts > RETAIN_RECEIPTS || audits > MAX_AUDIT {
            return Err(failure("model-call ledger capacity exceeded"));
        }
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(failure)?;
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(failure)?;
        if journal != "wal" {
            return Err(failure("model-call state requires WAL support"));
        }
        conn.execute_batch("PRAGMA wal_autocheckpoint=64; PRAGMA journal_size_limit=33554432;")
            .map_err(failure)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        if let Some(operation) = pending_on(&tx)? {
            if operation.state == "submitting" {
                tx.execute(
                    "UPDATE model_calls SET state='unknown' WHERE id=?1 AND state='submitting'",
                    [&operation.id],
                )
                .map_err(failure)?;
                audit(&tx, &operation.id, "restart_unknown", None)?;
            }
        }
        tx.commit().map_err(failure)?;
        #[cfg(unix)]
        File::open(
            path.parent()
                .ok_or_else(|| failure("invalid state parent"))?,
        )
        .and_then(|file| file.sync_all())
        .map_err(failure)?;
        Ok(Self {
            connection: Mutex::new(conn),
            _ownership: ownership,
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| failure("model-call database mutex poisoned"))
    }

    pub fn begin(&self, call: NewCall) -> Result<Operation> {
        call.validate()?;
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        if pending_on(&tx)?.is_some() {
            return Err(failure("unresolved model call requires recovery or review"));
        }
        // Never reuse a retained ID/turn ordinal, even when its receipt expired.
        let duplicate: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM model_calls WHERE id=?1 OR (turn_id=?2 AND purpose=?3 AND round=?4))",params![call.id,call.turn_id,purpose_name(call.purpose),call.round],|row|row.get(0)).map_err(failure)?;
        if duplicate {
            return Err(failure("model call identity already exists"));
        }
        tx.execute("DELETE FROM model_calls WHERE state IN ('completed','cleared') AND seq NOT IN (SELECT seq FROM model_calls ORDER BY seq DESC LIMIT ?1)",[MAX_OPERATIONS-1]).map_err(failure)?;
        // A real, durable allocation is reserved before any network submission.
        // Replacing it with a <=2 MiB receipt cannot grow the logical payload.
        tx.execute("INSERT INTO model_calls(id,turn_id,purpose,session_hash,round,model,has_tools,endpoint_hash,credential_hash,body_hash,state,reservation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'submitting',zeroblob(2097152))", params![call.id,call.turn_id,purpose_name(call.purpose),call.session_hash,call.round,call.model,call.has_tools,call.endpoint_hash,call.credential_hash,call.body_hash]).map_err(failure)?;
        audit(&tx, &call.id, "admitted", None)?;
        let operation = pending_on(&tx)?.ok_or_else(|| failure("admission was not recorded"))?;
        tx.commit().map_err(failure)?;
        Ok(operation)
    }

    /// Persist a received header before consuming the response body.
    pub fn record_remote(&self, id: &str, remote_id: &str) -> Result<()> {
        canonical_id(id)?;
        canonical_id(remote_id)?;
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let pending = pending_on(&tx)?.ok_or_else(|| failure("model call is no longer pending"))?;
        if pending.id != id
            || pending
                .remote_id
                .as_deref()
                .is_some_and(|old| old != remote_id)
        {
            return Err(failure("model-call remote identity conflicts"));
        }
        if pending.remote_id.is_none() {
            tx.execute("UPDATE model_calls SET remote_id=?2 WHERE id=?1 AND state IN ('submitting','unknown') AND remote_id IS NULL",params![id,remote_id]).map_err(failure)?;
            audit(&tx, id, "remote_recorded", None)?;
        }
        tx.commit().map_err(failure)
    }

    pub fn mark_unknown(&self, id: &str) -> Result<()> {
        canonical_id(id)?;
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let pending = pending_on(&tx)?.ok_or_else(|| failure("model call is no longer pending"))?;
        if pending.id != id {
            return Err(failure("model-call identity conflicts"));
        }
        if pending.state == "submitting" {
            tx.execute(
                "UPDATE model_calls SET state='unknown' WHERE id=?1 AND state='submitting'",
                [id],
            )
            .map_err(failure)?;
            audit(&tx, id, "unknown", None)?;
        }
        tx.commit().map_err(failure)
    }

    /// The coordinator validates the wire receipt before this call. A recovered
    /// model result is evidence only, never authority to execute tools or append
    /// it to a session. Recovery is an explicit transition from unknown.
    pub fn complete(
        &self,
        id: &str,
        remote_id: &str,
        receipt: &Value,
        recovered: bool,
    ) -> Result<()> {
        canonical_id(id)?;
        canonical_id(remote_id)?;
        let receipt = encode_receipt(receipt)?;
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let pending = pending_on(&tx)?.ok_or_else(|| failure("model call is no longer pending"))?;
        let expected_state = if recovered { "unknown" } else { "submitting" };
        if pending.id != id
            || pending.state != expected_state
            || pending.remote_id.as_deref() != Some(remote_id)
        {
            return Err(failure("model-call receipt identity or state conflicts"));
        }
        tx.execute("UPDATE model_calls SET state='completed',receipt=?2,reservation=NULL,recovered=?3 WHERE id=?1 AND state=?4 AND remote_id=?5",params![id,receipt,recovered,expected_state,remote_id]).map_err(failure)?;
        prune_receipts(&tx)?;
        audit(
            &tx,
            id,
            if recovered { "recovered" } else { "completed" },
            None,
        )?;
        tx.commit().map_err(failure)
    }

    pub fn pending(&self) -> Result<Option<Operation>> {
        let conn = self.connection()?;
        pending_on(&conn)
    }

    #[cfg(test)]
    pub fn get(&self, id: &str) -> Result<Option<Operation>> {
        canonical_id(id)?;
        self.connection()?
            .query_row(
                &format!("SELECT {COLUMNS} FROM model_calls WHERE id=?1"),
                [id],
                decode,
            )
            .optional()
            .map_err(failure)
    }

    /// Explicit local result inspection. Retired receipts return None; there is
    /// no network access, model cache lookup or automatic regeneration.
    pub fn show_result(&self, id: &str) -> Result<Option<Value>> {
        canonical_id(id)?;
        let conn = self.connection()?;
        let bytes: Option<Vec<u8>> = conn.query_row("SELECT receipt FROM model_calls WHERE id=?1 AND state='completed' AND receipt IS NOT NULL AND length(receipt)<=2097152",[id],|row|row.get(0)).optional().map_err(failure)?;
        bytes
            .map(|value| {
                serde_json::from_slice(&value)
                    .map_err(|_| failure("stored receipt is invalid JSON"))
            })
            .transpose()
    }

    pub fn review_clear(&self, id: &str, note: &str) -> Result<()> {
        canonical_id(id)?;
        bounded(note, 1024, "review note")?;
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let changed = tx.execute("UPDATE model_calls SET state='cleared',reservation=NULL WHERE id=?1 AND state='unknown'",[id]).map_err(failure)?;
        if changed != 1 {
            return Err(failure("review requires the exact unknown operation"));
        }
        audit(&tx, id, "review_cleared", Some(note))?;
        tx.commit().map_err(failure)
    }

    /// Bounded metadata. Body/session hashes aid private administrator review;
    /// prompt, receipt, endpoint URL and credential identity remain excluded.
    /// Free-form review notes are retained for audit but not printed by status.
    pub fn status(&self) -> Result<Value> {
        let conn = self.connection()?;
        let (operations, receipts, audits): (i64, i64, i64) = conn.query_row(
            "SELECT (SELECT count(*) FROM model_calls),(SELECT count(*) FROM model_calls WHERE receipt IS NOT NULL),(SELECT count(*) FROM model_call_audit)",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).map_err(failure)?;
        let metadata = |op: Operation| {
            json!({
                "id":op.id,"turn_id":op.turn_id,"purpose":op.purpose,"round":op.round,
                "model":op.model,"has_tools":op.has_tools,"remote_id":op.remote_id,
                "state":op.state,"recovered":op.recovered,
                "body_hash":op.body_hash,"session_hash":op.session_hash
            })
        };
        let pending = pending_on(&conn)?.map(metadata);
        let recent = conn
            .prepare(&format!(
                "SELECT {COLUMNS},receipt IS NOT NULL FROM model_calls ORDER BY seq DESC LIMIT 128"
            ))
            .map_err(failure)?
            .query_map([], |row| Ok((decode(row)?, row.get::<_, bool>(13)?)))
            .map_err(failure)?
            .map(|row| {
                row.map(|(op, has_receipt)| {
                    let mut value = metadata(op);
                    value["has_receipt"] = json!(has_receipt);
                    value
                })
            })
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failure)?;
        let audit_events = conn.prepare("SELECT operation_id,action,note IS NOT NULL FROM model_call_audit ORDER BY seq DESC LIMIT 16")
            .map_err(failure)?.query_map([], |row| Ok(json!({"operation_id":row.get::<_,String>(0)?,"action":row.get::<_,String>(1)?,"has_note":row.get::<_,bool>(2)?})))
            .map_err(failure)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failure)?;
        Ok(
            json!({"schema_version":SCHEMA_VERSION,"database_limit_bytes":MAX_DATABASE_BYTES,
            "retained_operations":operations,"retained_receipts":receipts,"retained_audit_events":audits,
            "pending":pending,"recent":recent,"audit":audit_events}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, Store) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().canonicalize().unwrap().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let store = Store::open(&workspace, Path::new("../calls/index.sqlite3")).unwrap();
        (temp, workspace, store)
    }
    fn call() -> NewCall {
        NewCall {
            id: Uuid::new_v4().to_string(),
            turn_id: Uuid::new_v4().to_string(),
            purpose: ModelPurpose::Chat,
            session_hash: Some("a".repeat(64)),
            round: 0,
            model: "logical-model".into(),
            has_tools: true,
            endpoint_hash: "b".repeat(64),
            credential_hash: "c".repeat(64),
            body_hash: "d".repeat(64),
        }
    }
    fn complete(store: &Store, call: NewCall, receipt: &Value) -> String {
        let operation = store.begin(call).unwrap();
        let remote = Uuid::new_v4().to_string();
        store.record_remote(&operation.id, &remote).unwrap();
        store
            .complete(&operation.id, &remote, receipt, false)
            .unwrap();
        operation.id
    }
    fn connection_size(conn: &Connection) -> u64 {
        let pages: u64 = conn
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .unwrap();
        pages * page_size
    }

    #[test]
    fn private_state_has_workspace_binding_identity_and_exclusive_lifetime_ownership() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Store>();
        let (temp, workspace, store) = setup();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        assert!(Store::open(&workspace, Path::new("inside.sqlite3")).is_err());
        assert!(!workspace.join("inside.sqlite3").exists());
        let conn = store.connection().unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            conn.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))
                .unwrap(),
            250
        );
        let pages: u64 = conn
            .pragma_query_value(None, "max_page_count", |r| r.get(0))
            .unwrap();
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .unwrap();
        assert_eq!(pages * page_size, MAX_DATABASE_BYTES);
        drop(conn);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join("calls"))
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
                    fs::metadata(temp.path().join("calls").join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        drop(store);
        let other = workspace.with_file_name("other");
        fs::create_dir(&other).unwrap();
        assert!(Store::open(&other, Path::new("../calls/index.sqlite3")).is_err());
        Store::open(&workspace, Path::new("../calls/index.sqlite3")).unwrap();
    }

    #[test]
    fn admission_validates_metadata_and_reserves_before_returning() {
        let (_temp, _workspace, store) = setup();
        let mut invalid = call();
        invalid.id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_uppercase();
        assert!(store.begin(invalid).is_err());
        let mut invalid = call();
        invalid.body_hash = "secret".into();
        assert!(store.begin(invalid).is_err());
        let mut invalid = call();
        invalid.round = 32;
        assert!(store.begin(invalid).is_err());
        let mut invalid = call();
        invalid.model = " model".into();
        assert!(store.begin(invalid).is_err());
        let mut invalid = call();
        invalid.model = "x".repeat(201);
        assert!(store.begin(invalid).is_err());
        let mut invalid = call();
        invalid.session_hash = Some("A".repeat(64));
        assert!(store.begin(invalid).is_err());
        assert!(store.pending().unwrap().is_none());
        let expected = call();
        let operation = store.begin(expected.clone()).unwrap();
        assert_eq!(operation.id, expected.id);
        assert_eq!(operation.turn_id, expected.turn_id);
        assert_eq!(operation.body_hash, expected.body_hash);
        assert_eq!(operation.session_hash, expected.session_hash);
        assert_eq!(operation.endpoint_hash, expected.endpoint_hash);
        assert_eq!(operation.credential_hash, expected.credential_hash);
        assert_eq!(
            store
                .connection()
                .unwrap()
                .query_row(
                    "SELECT length(reservation) FROM model_calls WHERE id=?1",
                    [&operation.id],
                    |r| r.get::<_, usize>(0)
                )
                .unwrap(),
            MAX_RECEIPT_BYTES
        );
        assert!(store
            .connection()
            .unwrap()
            .execute(
                "UPDATE model_calls SET reservation=NULL WHERE id=?1",
                [&operation.id]
            )
            .is_err());
        let mut changed = call();
        changed.credential_hash = "e".repeat(64);
        changed.endpoint_hash = "f".repeat(64);
        changed.model = "different".into();
        assert!(store.begin(changed).is_err());
        assert!(store.review_clear(&operation.id, "too soon").is_err());
        assert!(store.show_result(&operation.id).unwrap().is_none());
    }

    #[test]
    fn remote_identity_and_explicit_recovery_are_monotonic_across_restart() {
        let (_temp, workspace, store) = setup();
        let operation = store.begin(call()).unwrap();
        let remote = Uuid::new_v4().to_string();
        assert!(store.record_remote(&operation.id, "not-a-uuid").is_err());
        store.record_remote(&operation.id, &remote).unwrap();
        store.record_remote(&operation.id, &remote).unwrap();
        assert!(store
            .record_remote(&operation.id, &Uuid::new_v4().to_string())
            .is_err());
        drop(store);
        let store = Store::open(&workspace, Path::new("../calls/index.sqlite3")).unwrap();
        let restored = store.pending().unwrap().unwrap();
        assert_eq!(restored.id, operation.id);
        assert_eq!(restored.state, "unknown");
        assert_eq!(restored.remote_id.as_deref(), Some(remote.as_str()));
        assert!(store.begin(call()).is_err());
        let receipt = json!({"message":{"role":"assistant","content":"retained result"}});
        assert!(store
            .complete(&operation.id, &remote, &receipt, false)
            .is_err());
        assert!(store
            .complete(&operation.id, &Uuid::new_v4().to_string(), &receipt, true)
            .is_err());
        store
            .complete(&operation.id, &remote, &receipt, true)
            .unwrap();
        assert!(store.pending().unwrap().is_none());
        assert!(store.get(&operation.id).unwrap().unwrap().recovered);
        assert_eq!(store.show_result(&operation.id).unwrap(), Some(receipt));
        assert!(store.record_remote(&operation.id, &remote).is_err());
        assert!(store.mark_unknown(&operation.id).is_err());
        assert!(store
            .complete(&operation.id, &remote, &json!({}), true)
            .is_err());
        assert!(store
            .connection()
            .unwrap()
            .query_row(
                "SELECT reservation IS NULL FROM model_calls WHERE id=?1",
                [&operation.id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
    }

    #[test]
    fn missing_remote_identity_still_holds_and_review_is_atomic_audited_not_success() {
        let (_temp, workspace, store) = setup();
        let operation = store.begin(call()).unwrap();
        drop(store);
        let store = Store::open(&workspace, Path::new("../calls/index.sqlite3")).unwrap();
        assert!(store.pending().unwrap().unwrap().remote_id.is_none());
        store.mark_unknown(&operation.id).unwrap();
        for note in ["", " ", "two\nlines", &"x".repeat(1025)] {
            assert!(store.review_clear(&operation.id, note).is_err());
        }
        store.connection().unwrap().execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON model_call_audit WHEN NEW.action='review_cleared' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(store
            .review_clear(&operation.id, "supplier evidence")
            .is_err());
        assert_eq!(store.pending().unwrap().unwrap().state, "unknown");
        store
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_audit;")
            .unwrap();
        store
            .review_clear(&operation.id, "supplier evidence")
            .unwrap();
        assert!(store.pending().unwrap().is_none());
        assert_eq!(store.get(&operation.id).unwrap().unwrap().state, "cleared");
        assert!(store.show_result(&operation.id).unwrap().is_none());
        let conn = store.connection().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT note FROM model_call_audit WHERE action='review_cleared'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "supplier evidence"
        );
        drop(conn);
        store.begin(call()).unwrap();
    }

    #[test]
    fn persistence_failure_never_clears_a_hold_or_drops_its_storage_reservation() {
        let (_temp, _workspace, store) = setup();
        store.connection().unwrap().execute_batch("CREATE TRIGGER fail_admit BEFORE INSERT ON model_call_audit WHEN NEW.action='admitted' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(store.begin(call()).is_err());
        assert!(store.pending().unwrap().is_none());
        assert_eq!(store.status().unwrap()["retained_operations"], 0);
        store
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_admit;")
            .unwrap();
        let operation = store.begin(call()).unwrap();
        let remote = Uuid::new_v4().to_string();
        store.record_remote(&operation.id, &remote).unwrap();
        store.connection().unwrap().execute_batch("CREATE TRIGGER fail_complete BEFORE INSERT ON model_call_audit WHEN NEW.action='completed' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(store
            .complete(&operation.id, &remote, &json!({"content":"result"}), false)
            .is_err());
        assert_eq!(store.pending().unwrap().unwrap().state, "submitting");
        assert!(store.show_result(&operation.id).unwrap().is_none());
        assert_eq!(
            store
                .connection()
                .unwrap()
                .query_row("SELECT length(reservation) FROM model_calls", [], |r| r
                    .get::<_, usize>(
                    0
                ))
                .unwrap(),
            MAX_RECEIPT_BYTES
        );
        store
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_complete;")
            .unwrap();
        store
            .complete(&operation.id, &remote, &json!({"content":"result"}), false)
            .unwrap();
    }

    #[test]
    fn insufficient_reservation_capacity_rejects_before_admission() {
        let (_temp, _workspace, store) = setup();
        let conn = store.connection().unwrap();
        let pages: u64 = conn
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .unwrap();
        conn.pragma_update(None, "max_page_count", pages).unwrap();
        drop(conn);
        assert!(store.begin(call()).is_err());
        assert!(store.pending().unwrap().is_none());
        assert_eq!(store.status().unwrap()["retained_operations"], 0);
        assert_eq!(store.status().unwrap()["retained_audit_events"], 0);
        store
            .connection()
            .unwrap()
            .pragma_update(None, "max_page_count", MAX_DATABASE_BYTES / page_size)
            .unwrap();
        let operation = store.begin(call()).unwrap();
        assert_eq!(store.pending().unwrap().unwrap().id, operation.id);
    }

    #[test]
    fn exact_byte_limits_and_bounded_retention_fit_the_real_page_budget() {
        let (temp, workspace, store) = setup();
        let framing = serde_json::to_vec(&json!({"content":""})).unwrap().len();
        let maximum = json!({"content":"x".repeat(MAX_RECEIPT_BYTES-framing)});
        assert_eq!(encode_receipt(&maximum).unwrap().len(), MAX_RECEIPT_BYTES);
        let oversized = json!({"content":"\u{0000}".repeat(MAX_RECEIPT_BYTES/6)});
        assert!(encode_receipt(&oversized).is_err());
        let mut ids = Vec::new();
        for _ in 0..140 {
            ids.push(complete(&store, call(), &maximum));
        }
        let status = store.status().unwrap();
        assert_eq!(status["retained_operations"], MAX_OPERATIONS);
        assert_eq!(status["retained_receipts"], RETAIN_RECEIPTS);
        assert_eq!(status["retained_audit_events"], MAX_AUDIT);
        assert_eq!(status["recent"].as_array().unwrap().len(), 128);
        assert_eq!(status["audit"].as_array().unwrap().len(), 16);
        assert!(store.get(&ids[0]).unwrap().is_none());
        assert!(store.get(&ids[131]).unwrap().is_some());
        assert!(store.show_result(&ids[131]).unwrap().is_none());
        assert_eq!(
            store.show_result(ids.last().unwrap()).unwrap(),
            Some(maximum.clone())
        );
        let operation = store.begin(call()).unwrap();
        let remote = Uuid::new_v4().to_string();
        store.record_remote(&operation.id, &remote).unwrap();
        assert!(store
            .complete(&operation.id, &remote, &oversized, false)
            .is_err());
        assert!(store.pending().unwrap().is_some());
        store.mark_unknown(&operation.id).unwrap();
        let size = connection_size(&store.connection().unwrap());
        assert!(size <= MAX_DATABASE_BYTES);
        drop(store);
        assert!(
            fs::metadata(temp.path().join("calls/index.sqlite3"))
                .unwrap()
                .len()
                <= MAX_DATABASE_BYTES
        );
        let store = Store::open(&workspace, Path::new("../calls/index.sqlite3")).unwrap();
        assert_eq!(store.pending().unwrap().unwrap().id, operation.id);
        assert!(store.begin(call()).is_err());
        store
            .complete(&operation.id, &remote, &maximum, true)
            .unwrap();
        assert_eq!(
            store.status().unwrap()["retained_receipts"],
            RETAIN_RECEIPTS
        );
        assert!(connection_size(&store.connection().unwrap()) <= MAX_DATABASE_BYTES);
    }

    #[test]
    fn result_inspection_is_explicit_and_identity_reuse_is_rejected() {
        let (_temp, _workspace, store) = setup();
        let original = call();
        let id = complete(
            &store,
            original.clone(),
            &json!({"content":"private response"}),
        );
        assert!(store.begin(original.clone()).is_err());
        let mut other = original.clone();
        other.id = Uuid::new_v4().to_string();
        assert!(store.begin(other).is_err());
        assert_eq!(
            store.show_result(&id).unwrap().unwrap()["content"],
            "private response"
        );
        let status = store.status().unwrap().to_string();
        for hidden in [
            "private response",
            original.endpoint_hash.as_str(),
            original.credential_hash.as_str(),
        ] {
            assert!(!status.contains(hidden));
        }
        assert!(status.contains(&original.body_hash));
        assert!(status.contains(original.session_hash.as_deref().unwrap()));
        assert_eq!(store.status().unwrap()["recent"][0]["has_receipt"], true);
        assert!(store
            .show_result(&Uuid::new_v4().to_string())
            .unwrap()
            .is_none());
        assert!(store.show_result("../escape").is_err());
        let known_remote = store.get(&id).unwrap().unwrap().remote_id.unwrap();
        let second = store.begin(call()).unwrap();
        assert!(store.record_remote(&second.id, &known_remote).is_err());
        assert!(store.pending().unwrap().unwrap().remote_id.is_none());
    }

    #[test]
    fn corrupt_foreign_and_future_database_headers_are_never_adopted() {
        let (temp, workspace, store) = setup();
        drop(store);
        let path = temp.path().join("calls/index.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "application_id", 123).unwrap();
        conn.execute_batch(
            "CREATE TABLE foreign_data(body TEXT); INSERT INTO foreign_data VALUES('preserve');",
        )
        .unwrap();
        drop(conn);
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        drop(conn);
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::write(&path, b"not sqlite").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn linked_insecure_and_oversized_state_inputs_are_rejected_without_modification() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (temp, workspace, store) = setup();
        drop(store);
        let parent = temp.path().canonicalize().unwrap().join("calls");
        let database = parent.join("index.sqlite3");
        let original = fs::read(&database).unwrap();
        let moved = parent.join("original.sqlite3");
        fs::rename(&database, &moved).unwrap();
        symlink(&moved, &database).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        fs::remove_file(&database).unwrap();
        fs::hard_link(&moved, &database).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        fs::remove_file(&database).unwrap();
        fs::rename(&moved, &database).unwrap();
        for suffix_name in ["-wal", "-shm", "-journal", ".lock"] {
            let sidecar = suffix(&database, suffix_name);
            fs::remove_file(&sidecar).ok();
            symlink(&database, &sidecar).unwrap();
            assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
            fs::remove_file(sidecar).unwrap();
        }
        let linked = workspace.with_file_name("linked");
        symlink(&parent, &linked).unwrap();
        assert!(Store::open(&workspace, Path::new("../linked/index.sqlite3")).is_err());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&database, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(fs::read(&database).unwrap(), original);
        let sidecar = suffix(&database, "-shm");
        let file = create_private_file(&sidecar).unwrap();
        file.set_len(4 * 1024 * 1024 + 1).unwrap();
        drop(file);
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
        fs::remove_file(sidecar).unwrap();
        let file = OpenOptions::new().write(true).open(&database).unwrap();
        file.set_len(MAX_DATABASE_BYTES + 1).unwrap();
        drop(file);
        assert!(Store::open(&workspace, Path::new("../calls/index.sqlite3")).is_err());
    }
}
