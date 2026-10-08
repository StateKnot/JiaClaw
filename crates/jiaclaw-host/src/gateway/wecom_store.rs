// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A private, immutable-owner channel database for one gateway Wecom binding.
use super::registry::WecomBindingSummary;
use crate::{channel_store, store::SessionStore};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const APPLICATION_ID: i32 = 0x4a43_5743;
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXISTING_WAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INITIALIZING_BYTES: u64 = 128 * 1024;
const OWNER_SCHEMA: &str = "
CREATE TABLE gateway_wecom_owner (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 protocol INTEGER NOT NULL CHECK(protocol=5),
 binding_id TEXT NOT NULL, user_id TEXT NOT NULL, backend_id TEXT NOT NULL,
 corp_id TEXT NOT NULL, agent_id INTEGER NOT NULL, human_user_id TEXT NOT NULL
);
CREATE TRIGGER gateway_wecom_owner_no_replace BEFORE INSERT ON gateway_wecom_owner
 WHEN EXISTS(SELECT 1 FROM gateway_wecom_owner)
 BEGIN SELECT RAISE(ABORT,'WeCom database owner is immutable'); END;
CREATE TRIGGER gateway_wecom_owner_immutable_update BEFORE UPDATE ON gateway_wecom_owner
 BEGIN SELECT RAISE(ABORT,'WeCom database owner is immutable'); END;
CREATE TRIGGER gateway_wecom_owner_immutable_delete BEFORE DELETE ON gateway_wecom_owner
 BEGIN SELECT RAISE(ABORT,'WeCom database owner is immutable'); END;
";
const OPERATIONS_SCHEMA: &str = "
CREATE TABLE gateway_wecom_operations (
 request_id TEXT PRIMARY KEY NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('event','delivery')),
 event_id TEXT NOT NULL,
 delivery_id TEXT,
 attempt INTEGER NOT NULL CHECK(attempt BETWEEN 1 AND 5),
 claimed_ms INTEGER NOT NULL,
 cached_state TEXT,
 cached_receipt TEXT,
 cached_reviewed_ms INTEGER,
 CHECK((kind='event' AND delivery_id IS NULL AND attempt=1)
    OR (kind='delivery' AND delivery_id IS NOT NULL))
);
CREATE INDEX gateway_wecom_operations_event ON gateway_wecom_operations(event_id);
CREATE INDEX gateway_wecom_operations_delivery ON gateway_wecom_operations(delivery_id);
CREATE TRIGGER gateway_wecom_operation_no_replace BEFORE INSERT ON gateway_wecom_operations
 WHEN EXISTS(SELECT 1 FROM gateway_wecom_operations WHERE request_id=NEW.request_id)
 BEGIN SELECT RAISE(ABORT,'WeCom operation identity is immutable'); END;
CREATE TRIGGER gateway_wecom_operation_immutable_identity BEFORE UPDATE OF request_id,kind,event_id,delivery_id,attempt,claimed_ms ON gateway_wecom_operations
 BEGIN SELECT RAISE(ABORT,'WeCom operation identity is immutable'); END;
CREATE TRIGGER gateway_wecom_operation_immutable_delete BEFORE DELETE ON gateway_wecom_operations
 BEGIN SELECT RAISE(ABORT,'WeCom operation identity is permanent'); END;
";

#[derive(Debug, Serialize)]
pub(super) struct WecomOperation {
    pub request_id: String,
    pub kind: String,
    pub event_id: String,
    pub delivery_id: Option<String>,
    pub attempt: u32,
    pub claimed_ms: i64,
    pub state: Option<String>,
    pub receipt: Option<String>,
    pub reviewed_ms: Option<i64>,
}

impl WecomOperation {
    fn validate(&self) -> Result<()> {
        let canonical_uuid = |value: &str, version: Option<usize>| {
            uuid::Uuid::parse_str(value).is_ok_and(|id| {
                !id.is_nil()
                    && id.get_variant() == uuid::Variant::RFC4122
                    && id.to_string() == value
                    && version.is_none_or(|version| id.get_version_num() == version)
            })
        };
        ensure!(
            canonical_uuid(&self.request_id, Some(7))
                && canonical_uuid(&self.event_id, None)
                && self
                    .delivery_id
                    .as_ref()
                    .is_none_or(|id| canonical_uuid(id, None))
                && ((self.kind == "event" && self.delivery_id.is_none() && self.attempt == 1)
                    || (self.kind == "delivery"
                        && self.delivery_id.is_some()
                        && (1..=5).contains(&self.attempt)))
                && self.claimed_ms >= 0
                && self.reviewed_ms.is_none_or(|ms| ms >= 0),
            "invalid WeCom operation identity or timestamp"
        );
        validate_operation_state(&self.kind, self.state.as_deref())?;
        validate_operation_receipt(self.receipt.as_deref())?;
        ensure!(
            (self.kind != "event" || self.receipt.is_none())
                && (self.state.as_deref() != Some("delivered") || self.receipt.is_some()),
            "invalid WeCom operation receipt association"
        );
        Ok(())
    }
}

fn validate_operation_state(kind: &str, state: Option<&str>) -> Result<()> {
    ensure!(
        state.is_none_or(|state| match kind {
            "event" => matches!(
                state,
                "received" | "processing" | "completed" | "needs_review"
            ),
            "delivery" => matches!(
                state,
                "pending"
                    | "submitting"
                    | "retry_wait"
                    | "delivered"
                    | "unknown"
                    | "permanent_failed"
                    | "cancelled"
                    | "expired"
            ),
            _ => false,
        }),
        "invalid WeCom operation state"
    );
    Ok(())
}

fn validate_operation_receipt(receipt: Option<&str>) -> Result<()> {
    ensure!(
        receipt.is_none_or(|receipt| !receipt.is_empty()
            && receipt.len() <= 4096
            && !receipt.chars().any(char::is_control)),
        "invalid WeCom operation receipt"
    );
    Ok(())
}

pub(super) struct WecomStore {
    pub(super) inner: SessionStore,
}

fn now_ms() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

fn suffix(path: &Path, extra: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(extra);
    PathBuf::from(value)
}

fn private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Wecom state directories must not be symlinks"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.permissions().mode() & 0o777 == 0o700
                && metadata.uid() == rustix::process::geteuid().as_raw(),
            "Wecom state directories require mode 0700"
        );
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Wecom state files must be ordinary files, not links or special files"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.nlink() == 1
                && metadata.permissions().mode() & 0o777 == 0o600
                && metadata.uid() == rustix::process::geteuid().as_raw(),
            "Wecom state files require mode 0600 and one hard link"
        );
    }
    Ok(metadata)
}

fn create_private_file(path: &Path) -> Result<(File, bool)> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let (file, created) = match options.open(path) {
        Ok(file) => (file, true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            private_file(path)?;
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            (options.open(path)?, false)
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = private_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.is_file()
                && opened.dev() == metadata.dev()
                && opened.ino() == metadata.ino()
                && opened.nlink() == 1
                && opened.mode() & 0o777 == 0o600
                && opened.uid() == rustix::process::geteuid().as_raw(),
            "Wecom state file changed during open"
        );
    }
    if created {
        file.sync_all()?;
        File::open(path.parent().context("Wecom file has no parent")?)?.sync_all()?;
    }
    Ok((file, created))
}

fn verify_sidecars(path: &Path) -> Result<()> {
    for extra in ["-wal", "-shm", "-journal"] {
        let path = suffix(path, extra);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let metadata = private_file(&path)?;
                ensure!(
                    metadata.len() <= MAX_EXISTING_WAL_BYTES,
                    "Wecom SQLite sidecar exceeds its startup bound"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn require_no_initializing_sidecars(path: &Path) -> Result<()> {
    for extra in ["-wal", "-shm", "-journal"] {
        let sidecar = suffix(path, extra);
        match fs::symlink_metadata(&sidecar) {
            Ok(_) => {
                let metadata = private_file(&sidecar)?;
                ensure!(
                    metadata.len() <= MAX_INITIALIZING_BYTES,
                    "Wecom initialization sidecar exceeds its bound"
                );
                anyhow::bail!("Wecom initialization sidecars require offline administrator review");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn owner_schema(conn: &Connection) -> Result<Vec<(String, String, String, String)>> {
    let mut statement = conn.prepare(
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn verify_initializing_owner(conn: &Connection, binding: &WecomBindingSummary) -> Result<()> {
    verify_owner(conn, binding)?;
    let journal: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    ensure!(
        journal == "delete",
        "Wecom staging owner requires a single-file DELETE journal"
    );
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        version == 0,
        "Wecom staging database must contain only its initial owner"
    );
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(OWNER_SCHEMA)?;
    ensure!(
        owner_schema(conn)? == owner_schema(&expected)?,
        "Wecom staging owner schema is incomplete or unrelated"
    );
    let count: i64 = conn.query_row("SELECT count(*) FROM gateway_wecom_owner", [], |row| {
        row.get(0)
    })?;
    ensure!(
        count == 1,
        "Wecom staging database must contain exactly one owner"
    );
    Ok(())
}

fn write_initial_owner(conn: &mut Connection, binding: &WecomBindingSummary) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(OWNER_SCHEMA)?;
    tx.execute(
        "INSERT INTO gateway_wecom_owner VALUES(1,5,?1,?2,?3,?4,?5,?6)",
        params![
            binding.id.to_string(),
            binding.user_id.to_string(),
            binding.backend_id,
            binding.corp_id,
            binding.agent_id,
            binding.human_user_id
        ],
    )?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.commit()?;
    Ok(())
}

/// Only the fixed private staging path may resume an empty initialization. An
/// existing final file is never adopted, even when it is empty. Staging with a
/// sidecar may need SQLite recovery; reject it before any writable connection
/// can change an unverified owner or partially committed schema.
fn prepare_initial_owner(stage: &Path, binding: &WecomBindingSummary) -> Result<File> {
    require_no_initializing_sidecars(stage)?;
    let (file, _) = create_private_file(stage)?;
    ensure!(
        file.metadata()?.len() <= MAX_INITIALIZING_BYTES,
        "Wecom staging database exceeds 128 KiB"
    );
    if file.metadata()?.len() > 0 {
        // SQLite READ_ONLY may still create WAL/SHM files in a writable
        // directory. Reject WAL and incomplete headers before opening SQLite,
        // so inspecting a foreign stage cannot change even its sidecars.
        ensure!(
            file.metadata()?.len() >= 100,
            "Wecom staging SQLite header is incomplete"
        );
        let mut header = [0_u8; 20];
        (&file).read_exact(&mut header)?;
        ensure!(
            &header[..16] == b"SQLite format 3\0" && header[18] == 1 && header[19] == 1,
            "Wecom staging requires a complete rollback-journal SQLite header"
        );
    }
    let needs_owner = {
        let conn = Connection::open_with_flags(
            stage,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        conn.busy_timeout(Duration::from_millis(250))?;
        let application: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let empty = application == 0 && version == 0 && owner_schema(&conn)?.is_empty();
        if !empty {
            verify_initializing_owner(&conn, binding)?;
        }
        empty
    };
    if needs_owner {
        let mut conn = Connection::open_with_flags(
            stage,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        conn.busy_timeout(Duration::from_millis(250))?;
        let journal: String = conn.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
        ensure!(
            journal == "delete",
            "Wecom initialization requires a single-file DELETE journal"
        );
        conn.pragma_update(None, "synchronous", "FULL")?;
        write_initial_owner(&mut conn, binding)?;
        conn.close()
            .map_err(|(_, error)| error)
            .context("Wecom initialization connection failed to close")?;
    }
    {
        let conn = Connection::open_with_flags(
            stage,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        verify_initializing_owner(&conn, binding)?;
        conn.close()
            .map_err(|(_, error)| error)
            .context("Wecom initialization verification failed to close")?;
    }
    require_no_initializing_sidecars(stage)?;
    let metadata = private_file(stage)?;
    ensure!(
        metadata.len() <= MAX_INITIALIZING_BYTES,
        "Wecom staging database exceeds 128 KiB"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino() && opened.nlink() == 1,
            "Wecom staging file changed during initialization"
        );
    }
    file.sync_all()?;
    Ok(file)
}

fn publish_initial_owner(stage: &Path, path: &Path) -> Result<()> {
    let parent = path.parent().context("Wecom database has no parent")?;
    ensure!(
        stage.parent() == Some(parent),
        "Wecom owner publication must stay in one directory"
    );
    let directory = File::open(parent)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        rustix::fs::renameat_with(
            &directory,
            stage
                .file_name()
                .context("Wecom staging file has no name")?,
            &directory,
            path.file_name().context("Wecom database has no name")?,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
        .context("Wecom owner publication cannot replace existing state")?;
        directory.sync_all()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        anyhow::bail!("atomic Wecom owner publication requires Linux or macOS");
    }
}

fn verify_final_header(path: &Path) -> Result<()> {
    let metadata = private_file(path)?;
    ensure!(
        metadata.len() >= 100 && metadata.len() <= MAX_DATABASE_BYTES,
        "Wecom SQLite header is incomplete or exceeds its startup bound"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino() && opened.nlink() == 1,
            "Wecom final state changed during header inspection"
        );
    }
    let mut header = [0_u8; 100];
    (&file).read_exact(&mut header)?;
    ensure!(
        &header[..16] == b"SQLite format 3\0"
            && matches!(header[18], 1 | 2)
            && header[18] == header[19]
            && header[68..72] == APPLICATION_ID.to_be_bytes(),
        "refusing an unrelated or incomplete Wecom SQLite header"
    );
    Ok(())
}

fn immutable_owner_schema(conn: &Connection) -> Result<Vec<(String, String, String, String)>> {
    let mut statement=conn.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE tbl_name='gateway_wecom_owner' ORDER BY type,name")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn verify_immutable_owner(conn: &Connection, binding: &WecomBindingSummary) -> Result<()> {
    let application: i32 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    ensure!(
        application == APPLICATION_ID,
        "refusing to adopt an unrelated Wecom channel database"
    );
    let expected_owner = Connection::open_in_memory()?;
    expected_owner.execute_batch(OWNER_SCHEMA)?;
    let objects = immutable_owner_schema(conn)?;
    ensure!(
        objects == owner_schema(&expected_owner)?,
        "Wecom immutable owner schema is incomplete or unrelated"
    );
    let owner: (i64, String, String, String, String, u32, String) = conn.query_row(
        "SELECT protocol,binding_id,user_id,backend_id,corp_id,agent_id,human_user_id FROM gateway_wecom_owner WHERE singleton=1",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
    ).context("WeCom channel database owner is absent or invalid")?;
    ensure!(
        owner
            == (
                5,
                binding.id.to_string(),
                binding.user_id.to_string(),
                binding.backend_id.clone(),
                binding.corp_id.clone(),
                binding.agent_id,
                binding.human_user_id.clone()
            ),
        "WeCom channel database belongs to a different binding"
    );
    Ok(())
}

/// The published main file already contains the immutable owner committed in
/// DELETE journal mode. Inspect ONLY that owner before SQLite may create SHM for
/// a foreign WAL database. Immutable mode intentionally ignores WAL: it cannot
/// authorize queue/quota inspection, migration or recovery. The real WAL-aware
/// connection below must independently verify the owner and recover all work.
fn verify_main_owner(path: &Path, binding: &WecomBindingSummary) -> Result<()> {
    let mut uri = reqwest::Url::from_file_path(path)
        .map_err(|_| anyhow::anyhow!("invalid WeCom immutable owner file path"))?;
    uri.set_query(Some("mode=ro&immutable=1"));
    let conn = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    verify_immutable_owner(&conn, binding)
}

fn verify_owner(conn: &Connection, binding: &WecomBindingSummary) -> Result<()> {
    verify_immutable_owner(conn, binding)?;
    let expected_owner = Connection::open_in_memory()?;
    expected_owner.execute_batch(OWNER_SCHEMA)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        version == 0 || version == 10,
        "unsupported Wecom session schema"
    );
    if version == 0 {
        // A crash after committing the owner, before SessionStore's atomic
        // migrations, is safe to resume. Never adopt other unversioned tables.
        ensure!(
            owner_schema(conn)? == owner_schema(&expected_owner)?,
            "unexpected schema in uninitialized Wecom database"
        );
    }
    let page_size: u64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
    let page_count: u64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    ensure!(
        page_size > 0 && page_count <= MAX_DATABASE_BYTES / page_size,
        "Wecom database exceeds 64 MiB"
    );
    Ok(())
}

fn validate_binding(binding: &WecomBindingSummary) -> Result<()> {
    ensure!(
        !binding.id.is_nil()
            && !binding.user_id.is_nil()
            && binding.id.get_variant() == uuid::Variant::RFC4122
            && binding.user_id.get_variant() == uuid::Variant::RFC4122
            && !binding.backend_id.is_empty()
            && binding.backend_id.len() <= 64
            && binding
                .backend_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
        "invalid WeCom binding owner"
    );
    ensure!(
        !binding.corp_id.is_empty() && binding.corp_id.len() <= 64,
        "invalid WeCom binding enterprise"
    );
    crate::wecom::validate_installation(&format!("{}:{}", binding.corp_id, binding.agent_id))?;
    ensure!(
        crate::wecom::user_id(&binding.human_user_id),
        "invalid canonical WeCom member"
    );
    Ok(())
}

impl WecomStore {
    pub(super) fn open(registry_path: &Path, binding: &WecomBindingSummary) -> Result<Self> {
        ensure!(
            registry_path.is_absolute()
                && !registry_path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir)),
            "Wecom registry path must be absolute without traversal"
        );
        validate_binding(binding)?;
        let parent = registry_path
            .parent()
            .context("Wecom registry has no parent")?;
        private_directory(parent)?;
        private_file(registry_path)?;
        let parent = parent.canonicalize()?;
        let directory = parent.join("wecom");
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => File::open(&parent)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        private_directory(&directory)?;
        let path = directory.join(format!("{}.sqlite3", binding.id));

        // Acquire the exact SessionStore lifetime lock before touching any DB
        // descriptor. Closing an unrelated DB file descriptor can release POSIX
        // locks belonging to another SQLite connection in this same process.
        let (ownership, _) = create_private_file(&path.with_extension("sqlite3.lock"))?;
        ownership
            .try_lock_exclusive()
            .context("another process owns this Wecom channel database")?;
        let ownership = crate::store::DatabaseOwnership::new(ownership);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                ensure!(
                    private_file(&path)?.len() <= MAX_DATABASE_BYTES,
                    "Wecom database exceeds 64 MiB"
                );
                verify_sidecars(&path)?;
                verify_final_header(&path)?;
                // Preserve any staging leftover for offline inspection. It has
                // no authority over a final file that already exists.
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                require_no_initializing_sidecars(&path)?;
                let stage = suffix(&path, ".initializing");
                let staged_file = prepare_initial_owner(&stage, binding)?;
                // Recheck before publication; a final-sidecar residue must never
                // be applied to this new owner by SessionStore's SQLite open.
                require_no_initializing_sidecars(&path)?;
                publish_initial_owner(&stage, &path)?;
                drop(staged_file);
            }
            Err(error) => return Err(error.into()),
        }
        verify_main_owner(&path, binding)?;
        {
            // Identity is checked before SessionStore can migrate or change the
            // persistent journal mode of any existing file.
            let conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?;
            conn.busy_timeout(Duration::from_millis(250))?;
            verify_owner(&conn, binding)?;
        }
        let mut inner = SessionStore::open_with_ownership(&path, Some(ownership))?;
        let SessionStore::Sqlite { conn, .. } = &mut inner else {
            unreachable!()
        };
        conn.busy_timeout(Duration::from_millis(250))?;
        verify_owner(conn, binding)?;
        let page_size: u64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
        let maximum = MAX_DATABASE_BYTES / page_size;
        let applied: u64 =
            conn.query_row(&format!("PRAGMA max_page_count={maximum}"), [], |r| {
                r.get(0)
            })?;
        ensure!(
            applied == maximum,
            "Wecom database exceeds its page capacity"
        );
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=2097152; PRAGMA trusted_schema=OFF;",
        )?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='gateway_wecom_operations')",
            [], |r| r.get(0),
        )?;
        if !exists {
            let used: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channel_events) OR EXISTS(SELECT 1 FROM channel_outbox) OR EXISTS(SELECT 1 FROM channel_dedup) OR EXISTS(SELECT 1 FROM wecom_send_reservations)",
                [], |r| r.get(0),
            )?;
            ensure!(
                !used,
                "Wecom operation ledger is missing from a populated database"
            );
            tx.execute_batch(OPERATIONS_SCHEMA)?;
        }
        let expected = Connection::open_in_memory()?;
        expected.execute_batch(OPERATIONS_SCHEMA)?;
        ensure!(
            owner_schema(&tx)?
                .into_iter()
                .filter(|r| r.2 == "gateway_wecom_operations")
                .collect::<Vec<_>>()
                == owner_schema(&expected)?,
            "WeCom operation schema is incomplete or unrelated"
        );
        let operation_count: usize =
            tx.query_row("SELECT count(*) FROM gateway_wecom_operations", [], |r| {
                r.get(0)
            })?;
        ensure!(
            operation_count <= channel_store::MAX_WECOM_OPERATIONS,
            "WeCom permanent operation capacity exceeded"
        );
        tx.commit()?;
        let mut store = Self { inner };
        store.inner.recover_channels(now_ms()?)?;
        verify_sidecars(&path)?;
        Ok(store)
    }

    /// Maintenance must not invent an empty database or discard unknown residue.
    /// Caller retains the gateway stopped lock for the complete inspection.
    pub(super) fn open_existing(
        registry_path: &Path,
        binding: &WecomBindingSummary,
    ) -> Result<Option<Self>> {
        validate_binding(binding)?;
        ensure!(
            registry_path.is_absolute()
                && !registry_path
                    .components()
                    .any(|p| matches!(p, Component::ParentDir)),
            "invalid Wecom registry path"
        );
        let parent = registry_path
            .parent()
            .context("Wecom registry has no parent")?;
        private_directory(parent)?;
        private_file(registry_path)?;
        let directory = parent.canonicalize()?.join("wecom");
        match fs::symlink_metadata(&directory) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(_) => private_directory(&directory)?,
        }
        let path = directory.join(format!("{}.sqlite3", binding.id));
        let mut present = false;
        for extra in [
            "",
            "-wal",
            "-shm",
            "-journal",
            ".initializing",
            ".initializing-wal",
            ".initializing-shm",
            ".initializing-journal",
        ] {
            match fs::symlink_metadata(suffix(&path, extra)) {
                Ok(_) => present = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        if !present {
            return Ok(None);
        }
        // Empty staging has no committed owner and is insufficient for maintenance.
        if !path.exists() {
            let stage = suffix(&path, ".initializing");
            require_no_initializing_sidecars(&path)?;
            require_no_initializing_sidecars(&stage)?;
            ensure!(
                private_file(&stage)?.len() > 0,
                "uncommitted Wecom initialization requires administrator review"
            );
        }
        Self::open(registry_path, binding).map(Some)
    }

    fn connection(&self) -> &Connection {
        let SessionStore::Sqlite { conn, .. } = &self.inner else {
            unreachable!()
        };
        conn
    }

    pub(super) fn operations(&self, limit: usize, offset: usize) -> Result<Vec<WecomOperation>> {
        ensure!(
            (1..=100).contains(&limit) && offset <= channel_store::MAX_WECOM_OPERATIONS,
            "WeCom operations page requires limit 1..100 and offset at most 16000"
        );
        let mut statement=self.connection().prepare(
            "SELECT CASE WHEN length(CAST(o.request_id AS BLOB))=36 THEN o.request_id ELSE '' END,
            CASE WHEN length(CAST(o.kind AS BLOB)) BETWEEN 5 AND 8 THEN o.kind ELSE '' END,
            CASE WHEN length(CAST(o.event_id AS BLOB))=36 THEN o.event_id ELSE '' END,
            CASE WHEN o.delivery_id IS NULL OR length(CAST(o.delivery_id AS BLOB))=36 THEN o.delivery_id ELSE '' END,
            o.attempt,o.claimed_ms,
            CASE WHEN COALESCE(d.state,e.status,o.cached_state) IS NULL OR length(CAST(COALESCE(d.state,e.status,o.cached_state) AS BLOB))<=32 THEN COALESCE(d.state,e.status,o.cached_state) ELSE '' END,
            CASE WHEN COALESCE(d.receipt,o.cached_receipt) IS NULL OR length(CAST(COALESCE(d.receipt,o.cached_receipt) AS BLOB))<=4096 THEN COALESCE(d.receipt,o.cached_receipt) ELSE '' END,
            COALESCE(e.reviewed_ms,o.cached_reviewed_ms),
            CASE WHEN o.cached_state IS NULL OR length(CAST(o.cached_state AS BLOB))<=32 THEN o.cached_state ELSE '' END,
            CASE WHEN o.cached_receipt IS NULL OR length(CAST(o.cached_receipt AS BLOB))<=4096 THEN o.cached_receipt ELSE '' END,
            o.cached_reviewed_ms,
            (e.id IS NULL OR (e.channel='wecom' AND e.installation_id=(SELECT corp_id||':'||agent_id FROM gateway_wecom_owner)
                AND e.session_id=(SELECT 'wecom:'||binding_id FROM gateway_wecom_owner)
                AND json_extract(e.spec,'$.sender_id')=(SELECT human_user_id FROM gateway_wecom_owner)))
            AND (d.id IS NULL OR (d.channel='wecom' AND d.event_id=o.event_id
                AND d.installation_id=(SELECT corp_id||':'||agent_id FROM gateway_wecom_owner)
                AND json_extract(d.destination,'$.conversation_id')=(SELECT human_user_id FROM gateway_wecom_owner))),
            (e.id IS NOT NULL OR d.id IS NOT NULL)
            FROM gateway_wecom_operations o LEFT JOIN channel_outbox d ON d.id=o.delivery_id
            LEFT JOIN channel_events e ON e.id=o.event_id
            ORDER BY o.claimed_ms DESC,o.request_id DESC LIMIT ?1 OFFSET ?2")?;
        let rows = statement.query_map(
            params![i64::try_from(limit)?, i64::try_from(offset)?],
            |r| {
                Ok((
                    WecomOperation {
                        request_id: r.get(0)?,
                        kind: r.get(1)?,
                        event_id: r.get(2)?,
                        delivery_id: r.get(3)?,
                        attempt: r.get(4)?,
                        claimed_ms: r.get(5)?,
                        state: r.get(6)?,
                        receipt: r.get(7)?,
                        reviewed_ms: r.get(8)?,
                    },
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, bool>(12)?,
                    r.get::<_, bool>(13)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (operation, cached_state, cached_receipt, cached_reviewed_ms, links_valid, live) =
                row?;
            operation.validate()?;
            validate_operation_state(&operation.kind, cached_state.as_deref())?;
            validate_operation_receipt(cached_receipt.as_deref())?;
            ensure!(
                links_valid
                    && cached_reviewed_ms.is_none_or(|ms| ms >= 0)
                    && (!live
                        || (cached_state.is_none()
                            && cached_receipt.is_none()
                            && cached_reviewed_ms.is_none()))
                    && (cached_state.is_some()
                        || (cached_receipt.is_none() && cached_reviewed_ms.is_none()))
                    && (operation.kind != "event" || cached_receipt.is_none())
                    && (cached_state.as_deref() != Some("delivered") || cached_receipt.is_some()),
                "invalid WeCom operation cached metadata or owner association"
            );
            Ok(operation)
        })
        .collect()
    }

    /// Hints never grant authority: the recorded transaction and registry check
    /// repeat their conditions. NULL reservations also include orphan evidence.
    pub(super) fn pending(&self, now: i64) -> Result<(bool, bool)> {
        let conn = self.connection();
        let count: usize =
            conn.query_row("SELECT count(*) FROM gateway_wecom_operations", [], |r| {
                r.get(0)
            })?;
        if count >= channel_store::MAX_WECOM_OPERATIONS
            || self.inner.has_unsettled_wecom_reservations(None)?
        {
            return Ok((false, false));
        }
        let execution:bool=conn.query_row(
            "SELECT (SELECT count(*) FROM channel_events WHERE status='processing')<4
            AND EXISTS(SELECT 1 FROM channel_events e WHERE channel='wecom' AND status='received'
            AND NOT EXISTS(SELECT 1 FROM channel_events p WHERE p.session_id=e.session_id AND p.status='processing'))",
            [],|r|r.get(0))?;
        let execution = execution && channel_store::has_outbox_capacity(conn)?;
        let window_start = now.saturating_sub(24 * 60 * 60 * 1000);
        let delivery:bool=conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_outbox d WHERE d.channel='wecom' AND d.event_id IS NOT NULL
            AND state IN ('pending','retry_wait') AND next_attempt_ms<=?1 AND attempts<5
            AND (SELECT count(*) FROM wecom_send_reservations r WHERE r.installation_id=d.installation_id
                AND (r.settled_ms IS NULL OR r.settled_ms>?2))<200
            AND (SELECT count(*) FROM wecom_send_reservations r WHERE r.settled_ms IS NULL OR r.settled_ms>?2)<10000
            AND NOT EXISTS(SELECT 1 FROM wecom_send_reservations r WHERE r.installation_id=d.installation_id AND r.settled_ms IS NULL)
            AND NOT EXISTS(SELECT 1 FROM channel_outbox live WHERE live.channel=d.channel AND live.installation_id=d.installation_id AND live.state='submitting')
            AND NOT EXISTS(SELECT 1 FROM channel_cooldowns c WHERE c.channel=d.channel AND c.installation_id=d.installation_id AND c.until_ms>?1)
            AND NOT EXISTS(SELECT 1 FROM channel_outbox p WHERE p.channel=d.channel AND p.installation_id=d.installation_id
                AND p.destination_key=d.destination_key AND p.seq<d.seq AND p.state<>'delivered'
                AND (p.event_id=d.event_id OR p.job_run_id=d.job_run_id OR p.state<>'cancelled')))",
            params![now,window_start],|r|r.get(0))?;
        Ok((execution, delivery))
    }
}

#[cfg(test)]
mod tests {
    use super::super::registry::Registry;
    use super::*;
    use crate::channel_types::{Channel, Destination, EventSpec};
    use uuid::Uuid;

    struct Fixture {
        directory: PathBuf,
        registry: PathBuf,
        binding: WecomBindingSummary,
    }
    impl Fixture {
        fn new() -> Self {
            Self::installation("wwEnterprise", 1)
        }
        fn installation(corp: &str, agent: u32) -> Self {
            let directory =
                std::env::temp_dir().join(format!("jiaclaw-wecom-owner-{}", Uuid::new_v4()));
            let registry = directory.join("users.sqlite3");
            let identities = Registry::open(&registry).unwrap();
            let user = identities.add_user("alice").unwrap();
            let binding = identities
                .add_wecom_binding(user.user_id, corp, agent, "alice.member")
                .unwrap();
            Self {
                directory,
                registry,
                binding,
            }
        }
        fn database(&self) -> PathBuf {
            self.directory
                .join("wecom")
                .join(format!("{}.sqlite3", self.binding.id))
        }
        fn stage(&self) -> PathBuf {
            suffix(&self.database(), ".initializing")
        }
        fn create_directory(&self) {
            let directory = self.database().parent().unwrap().to_path_buf();
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(directory).unwrap();
        }
        fn empty_stage(&self) {
            self.create_directory();
            drop(create_private_file(&self.stage()).unwrap());
        }
        fn owner_only(&self) {
            self.create_directory();
            drop(create_private_file(&self.database()).unwrap());
            let mut conn = Connection::open(self.database()).unwrap();
            write_initial_owner(&mut conn, &self.binding).unwrap();
        }
        fn committed_stage(&self) {
            self.owner_only();
            fs::rename(self.database(), self.stage()).unwrap();
        }
        fn open(&self) -> WecomStore {
            WecomStore::open(&self.registry, &self.binding).unwrap()
        }
        fn spec(&self, number: u64) -> EventSpec {
            EventSpec {
                event_id: number.to_string(),
                session_id: format!("wecom:{}", self.binding.id),
                sender_id: self.binding.human_user_id.clone(),
                prompt: "private WeCom text".into(),
                enabled_tools: vec!["datetime_now".into(), "json_query".into()],
                timeout_secs: 120,
                destination: Destination {
                    channel: Channel::Wecom,
                    installation_id: format!("{}:{}", self.binding.corp_id, self.binding.agent_id),
                    conversation_id: self.binding.human_user_id.clone(),
                    thread_id: None,
                    interaction_id: None,
                    expires_ms: None,
                },
                sealed_token: None,
                fingerprint: "a".repeat(64),
            }
        }
        fn completed(&self, store: &mut WecomStore, number: u64, chunks: Vec<String>) -> String {
            let accepted = store
                .inner
                .accept_wecom_event_recorded(self.spec(number), 0)
                .unwrap();
            let claimed = store
                .inner
                .claim_wecom_event_recorded(1, &Uuid::now_v7().to_string())
                .unwrap()
                .unwrap();
            assert_eq!(claimed.id, accepted.id);
            assert!(store
                .inner
                .complete_channel_event(&accepted.id, None, "completed", chunks, None, 2)
                .unwrap());
            accepted.id
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn empty_and_committed_staging_and_owner_only_crash_boundaries_resume() {
        for boundary in 0..3 {
            let f = Fixture::new();
            match boundary {
                0 => f.empty_stage(),
                1 => f.committed_stage(),
                _ => f.owner_only(),
            }
            let store = f.open();
            verify_owner(store.connection(), &f.binding).unwrap();
            assert!(store.operations(100, 0).unwrap().is_empty());
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT protocol,corp_id,agent_id,human_user_id FROM gateway_wecom_owner",
                        [],
                        |r| Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, u32>(2)?,
                            r.get::<_, String>(3)?
                        ))
                    )
                    .unwrap(),
                (5, "wwEnterprise".into(), 1, "alice.member".into())
            );
            assert!(!f.stage().exists());
            assert_eq!(
                store
                    .connection()
                    .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                10
            );
            drop(store);
            drop(f.open());
        }
    }

    #[test]
    fn foreign_partial_and_wal_staging_is_not_adopted_or_modified() {
        for kind in 0..9 {
            let f = Fixture::new();
            if (3..6).contains(&kind) {
                f.committed_stage();
            } else {
                f.empty_stage();
            }
            let conn = Connection::open(f.stage()).unwrap();
            match kind {
                0=>conn.execute_batch("CREATE TABLE foreign_state(value TEXT)").unwrap(),
                1=>conn.pragma_update(None,"application_id",123).unwrap(),
                2=>conn.pragma_update(None,"user_version",1).unwrap(),
                3=>conn.execute_batch("DROP TRIGGER gateway_wecom_owner_immutable_update; UPDATE gateway_wecom_owner SET agent_id=2").unwrap(),
                4=>conn.execute_batch("DROP TRIGGER gateway_wecom_owner_no_replace").unwrap(),
                5=>conn.execute_batch("CREATE VIEW unrelated_view AS SELECT 1").unwrap(),
                6=>conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE foreign_wal(value TEXT); PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
                7=>conn.execute_batch("CREATE TABLE sqliteforeign(value TEXT)").unwrap(),
                _=>(),
            }
            drop(conn);
            if kind == 6 {
                for extra in ["-wal", "-shm"] {
                    let _ = fs::remove_file(suffix(&f.stage(), extra));
                }
            }
            if kind == 8 {
                fs::write(f.stage(), b"incomplete staging header").unwrap();
            }
            let before = fs::read(f.stage()).unwrap();
            assert!(
                WecomStore::open(&f.registry, &f.binding).is_err(),
                "stage {kind}"
            );
            assert_eq!(fs::read(f.stage()).unwrap(), before);
            assert!(!f.database().exists());
            for extra in ["-wal", "-shm", "-journal"] {
                assert!(!suffix(&f.stage(), extra).exists());
            }
        }
    }

    #[test]
    fn staging_sidecars_and_atomic_publication_do_not_replace_existing_evidence() {
        for staged in [false, true] {
            for extra in ["-wal", "-shm", "-journal"] {
                let f = Fixture::new();
                f.committed_stage();
                let path = suffix(&if staged { f.stage() } else { f.database() }, extra);
                let (mut file, _) = create_private_file(&path).unwrap();
                use std::io::Write;
                file.write_all(b"unverified sidecar evidence").unwrap();
                drop(file);
                let before = fs::read(&path).unwrap();
                let stage = fs::read(f.stage()).unwrap();
                assert!(WecomStore::open(&f.registry, &f.binding).is_err());
                assert_eq!(fs::read(path).unwrap(), before);
                assert_eq!(fs::read(f.stage()).unwrap(), stage);
                assert!(!f.database().exists());
            }
        }
        let f = Fixture::new();
        f.committed_stage();
        let stage = fs::read(f.stage()).unwrap();
        drop(create_private_file(&f.database()).unwrap());
        fs::write(f.database(), b"existing final evidence").unwrap();
        assert!(publish_initial_owner(&f.stage(), &f.database()).is_err());
        assert_eq!(fs::read(f.database()).unwrap(), b"existing final evidence");
        assert_eq!(fs::read(f.stage()).unwrap(), stage);
    }

    #[test]
    fn owner_fields_protocol_and_file_lock_are_immutable_but_revocation_allows_offline_review() {
        let f = Fixture::new();
        let store = f.open();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
        assert!(WecomStore::open_existing(&f.registry, &f.binding).is_err());
        for sql in [
            "UPDATE gateway_wecom_owner SET agent_id=2",
            "DELETE FROM gateway_wecom_owner",
            "INSERT OR REPLACE INTO gateway_wecom_owner SELECT * FROM gateway_wecom_owner",
        ] {
            assert!(store.connection().execute_batch(sql).is_err());
        }
        drop(store);
        for field in 0..6 {
            let mut wrong = f.binding.clone();
            match field {
                0 => wrong.user_id = Uuid::new_v4(),
                1 => wrong.backend_id = "bob".into(),
                2 => wrong.corp_id = "wwOther".into(),
                3 => wrong.agent_id = 2,
                4 => wrong.human_user_id = "bob.member".into(),
                _ => {
                    wrong.id = Uuid::new_v4();
                    fs::copy(
                        f.database(),
                        f.directory
                            .join("wecom")
                            .join(format!("{}.sqlite3", wrong.id)),
                    )
                    .unwrap();
                }
            }
            assert!(
                WecomStore::open(&f.registry, &wrong).is_err(),
                "owner {field}"
            );
        }
        let mut revoked = f.binding.clone();
        revoked.enabled = false;
        drop(
            WecomStore::open_existing(&f.registry, &revoked)
                .unwrap()
                .unwrap(),
        );
        let conn = Connection::open(f.database()).unwrap();
        conn.execute_batch("DROP TRIGGER gateway_wecom_owner_immutable_update; PRAGMA ignore_check_constraints=ON; UPDATE gateway_wecom_owner SET protocol=4").unwrap();
        drop(conn);
        let before = fs::read(f.database()).unwrap();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
        assert_eq!(fs::read(f.database()).unwrap(), before);
    }

    #[test]
    fn unrelated_final_identity_future_version_and_missing_owner_guards_fail_without_migration() {
        for kind in 0..5 {
            let f = Fixture::new();
            drop(f.open());
            let conn = Connection::open(f.database()).unwrap();
            match kind {
                0 => conn
                    .pragma_update(None, "application_id", 0x4a43_4653)
                    .unwrap(),
                1 => conn.pragma_update(None, "user_version", 11).unwrap(),
                2 => conn
                    .execute_batch("DROP TRIGGER gateway_wecom_owner_no_replace")
                    .unwrap(),
                3 => conn
                    .execute_batch("DROP TRIGGER gateway_wecom_owner_immutable_update")
                    .unwrap(),
                _ => conn
                    .execute_batch("DROP TRIGGER gateway_wecom_owner_immutable_delete")
                    .unwrap(),
            }
            drop(conn);
            let before = fs::read(f.database()).unwrap();
            assert!(WecomStore::open(&f.registry, &f.binding).is_err());
            assert_eq!(fs::read(f.database()).unwrap(), before);
        }
        let f = Fixture::new();
        f.owner_only();
        let conn = Connection::open(f.database()).unwrap();
        conn.execute_batch("CREATE TABLE extra_state(value TEXT)")
            .unwrap();
        drop(conn);
        let before = fs::read(f.database()).unwrap();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
        assert_eq!(fs::read(f.database()).unwrap(), before);
    }

    #[test]
    fn foreign_same_protocol_main_owner_with_real_wal_is_rejected_without_creating_shm() {
        let foreign = Fixture::installation("wwOtherEnterprise", 2);
        let mut source = foreign.open();
        source
            .inner
            .accept_wecom_event_recorded(foreign.spec(1), 0)
            .unwrap();
        assert_eq!(
            source
                .connection()
                .query_row("SELECT protocol FROM gateway_wecom_owner", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            source
                .connection()
                .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
                .unwrap(),
            APPLICATION_ID
        );
        assert_eq!(
            source
                .connection()
                .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        let wal = suffix(&foreign.database(), "-wal");
        assert!(fs::metadata(&wal).unwrap().len() > 0);
        let target = Fixture::new();
        target.create_directory();
        fs::copy(foreign.database(), target.database()).unwrap();
        fs::copy(&wal, suffix(&target.database(), "-wal")).unwrap();
        drop(create_private_file(&target.database().with_extension("sqlite3.lock")).unwrap());
        let before = fs::read(target.database()).unwrap();
        let wal_before = fs::read(suffix(&target.database(), "-wal")).unwrap();
        let names = fs::read_dir(target.database().parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!suffix(&target.database(), "-shm").exists());
        assert!(WecomStore::open(&target.registry, &target.binding).is_err());
        assert!(WecomStore::open_existing(&target.registry, &target.binding).is_err());
        assert_eq!(fs::read(target.database()).unwrap(), before);
        assert_eq!(
            fs::read(suffix(&target.database(), "-wal")).unwrap(),
            wal_before
        );
        assert!(!suffix(&target.database(), "-shm").exists());
        assert_eq!(
            fs::read_dir(target.database().parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<std::collections::BTreeSet<_>>(),
            names
        );
        // The source owner and live WAL remain ordinary SQLite state throughout.
        assert_eq!(source.inner.list_channel_events(100, 0).unwrap().len(), 1);
    }

    #[test]
    fn operation_identity_guards_and_original_request_links_survive_atomic_body_purge() {
        let f = Fixture::new();
        let mut store = f.open();
        let event = f.completed(&mut store, 1, vec!["reply".into()]);
        let request = Uuid::now_v7().to_string();
        let delivery = store
            .inner
            .claim_wecom_delivery_recorded(3, &request)
            .unwrap()
            .unwrap();
        assert!(store
            .inner
            .finish_channel_delivery(
                &delivery.id,
                1,
                "delivered",
                Some("platformReceipt".into()),
                None,
                None,
                4
            )
            .unwrap());
        let before = serde_json::to_value(store.operations(100, 0).unwrap()).unwrap();
        for sql in [
            "UPDATE gateway_wecom_operations SET request_id='changed'",
            "UPDATE gateway_wecom_operations SET kind='event'",
            "UPDATE gateway_wecom_operations SET event_id='changed'",
            "UPDATE gateway_wecom_operations SET delivery_id='changed'",
            "UPDATE gateway_wecom_operations SET attempt=2",
            "UPDATE gateway_wecom_operations SET claimed_ms=5",
            "DELETE FROM gateway_wecom_operations",
            "INSERT OR REPLACE INTO gateway_wecom_operations SELECT * FROM gateway_wecom_operations",
        ] {assert!(store.connection().execute_batch(sql).is_err(),"{sql}");}
        store.connection().execute_batch("CREATE TRIGGER fail_purge BEFORE DELETE ON channel_events BEGIN SELECT RAISE(ABORT,'fixture purge failure'); END;").unwrap();
        assert!(store
            .inner
            .purge_wecom_channel_event_recorded(&event, 5)
            .is_err());
        assert!(store.inner.get_channel_event(&event).unwrap().is_some());
        assert!(store
            .inner
            .get_channel_delivery(&delivery.id)
            .unwrap()
            .is_some());
        assert_eq!(
            serde_json::to_value(store.operations(100, 0).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT count(*) FROM gateway_wecom_operations WHERE cached_state IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_purge")
            .unwrap();
        assert!(store
            .inner
            .purge_wecom_channel_event_recorded(&event, 6)
            .unwrap());
        assert!(store.inner.get_channel_event(&event).unwrap().is_none());
        assert!(store
            .inner
            .get_channel_delivery(&delivery.id)
            .unwrap()
            .is_none());
        assert_eq!(
            serde_json::to_value(store.operations(100, 0).unwrap()).unwrap(),
            before
        );
        let accepted = store
            .inner
            .accept_wecom_event_recorded(f.spec(1), 7)
            .unwrap();
        assert!(!accepted.created);
        assert_eq!(accepted.status, "purged");
        let reservation = store
            .inner
            .get_wecom_reservation("wwEnterprise:1", &delivery.id, 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            (reservation.reserved_ms, reservation.settled_ms),
            (3, Some(4))
        );
        drop(store);
        let reopened = f.open();
        assert_eq!(
            serde_json::to_value(reopened.operations(100, 0).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            reopened
                .inner
                .get_wecom_reservation("wwEnterprise:1", &delivery.id, 1)
                .unwrap()
                .unwrap()
                .settled_ms,
            Some(4)
        );
    }

    #[test]
    fn restart_and_orphan_unknown_reservations_preserve_quota_and_never_replay() {
        let f = Fixture::new();
        let mut store = f.open();
        let event = f.completed(&mut store, 1, vec!["first".into(), "never replay".into()]);
        let request = Uuid::now_v7().to_string();
        let delivery = store
            .inner
            .claim_wecom_delivery_recorded(3, &request)
            .unwrap()
            .unwrap();
        drop(store);
        let mut reopened = f.open();
        assert_eq!(
            reopened
                .inner
                .get_channel_delivery(&delivery.id)
                .unwrap()
                .unwrap()
                .state,
            "unknown"
        );
        assert!(reopened
            .inner
            .has_unsettled_wecom_reservations(None)
            .unwrap());
        assert_eq!(reopened.pending(i64::MAX / 2).unwrap(), (false, false));
        assert!(reopened
            .inner
            .claim_wecom_delivery_recorded(i64::MAX / 2, &Uuid::now_v7().to_string())
            .unwrap()
            .is_none());
        let operation = reopened
            .operations(100, 0)
            .unwrap()
            .into_iter()
            .find(|o| o.request_id == request)
            .unwrap();
        assert_eq!(
            (
                operation.event_id,
                operation.delivery_id,
                operation.attempt,
                operation.state
            ),
            (event, Some(delivery.id.clone()), 1, Some("unknown".into()))
        );
        let reservation = reopened
            .inner
            .get_wecom_reservation("wwEnterprise:1", &delivery.id, 1)
            .unwrap()
            .unwrap();
        assert_eq!(reservation.settled_ms, None);
        // An orphan NULL reservation remains blocking even without any event/outbox to inspect.
        drop(reopened);
        let orphan = Fixture::new();
        let store = orphan.open();
        store
            .connection()
            .execute(
                "INSERT INTO wecom_send_reservations VALUES('wwEnterprise:1',?1,1,0,NULL)",
                [Uuid::new_v4().to_string()],
            )
            .unwrap();
        drop(store);
        let recovered = orphan.open();
        assert_eq!(recovered.pending(i64::MAX / 2).unwrap(), (false, false));
        assert!(recovered
            .inner
            .has_unsettled_wecom_reservations(None)
            .unwrap());
        assert!(recovered.operations(100, 0).unwrap().is_empty());
        assert_eq!(
            recovered
                .inner
                .list_wecom_reservations(None, 100, 0)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn missing_operation_ledger_cannot_be_recreated_over_events_tombstones_or_orphan_quota() {
        for kind in 0..3 {
            let f = Fixture::new();
            let mut store = f.open();
            if kind == 0 {
                store
                    .inner
                    .accept_wecom_event_recorded(f.spec(1), 0)
                    .unwrap();
            }
            if kind == 1 {
                let event = f.completed(&mut store, 1, vec![]);
                store
                    .inner
                    .purge_wecom_channel_event_recorded(&event, 3)
                    .unwrap();
            }
            if kind == 2 {
                store
                    .connection()
                    .execute(
                        "INSERT INTO wecom_send_reservations VALUES('wwEnterprise:1',?1,1,0,NULL)",
                        [Uuid::new_v4().to_string()],
                    )
                    .unwrap();
            }
            store
                .connection()
                .execute_batch("DROP TABLE gateway_wecom_operations")
                .unwrap();
            drop(store);
            assert!(
                WecomStore::open(&f.registry, &f.binding).is_err(),
                "residue {kind}"
            );
            let conn = Connection::open(f.database()).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name='gateway_wecom_operations'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            if kind == 2 {
                assert_eq!(
                    conn.query_row(
                        "SELECT count(*) FROM wecom_send_reservations WHERE settled_ms IS NULL",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1
                );
            }
        }
        let clean = Fixture::new();
        let store = clean.open();
        store
            .connection()
            .execute_batch("DROP TABLE gateway_wecom_operations")
            .unwrap();
        drop(store);
        assert!(clean.open().operations(100, 0).unwrap().is_empty());
    }

    #[test]
    fn recorded_send_claim_rollback_preserves_attempt_quota_cooldown_and_operation_link() {
        for table in ["gateway_wecom_operations", "wecom_send_reservations"] {
            let f = Fixture::new();
            let mut store = f.open();
            let event = f.completed(&mut store, 1, vec!["reply".into()]);
            store.connection().execute_batch(&format!("CREATE TRIGGER fail_claim BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")).unwrap();
            assert!(store
                .inner
                .claim_wecom_delivery_recorded(3, &Uuid::now_v7().to_string())
                .is_err());
            let delivery = store
                .inner
                .list_channel_deliveries(Some(&event), 100, 0)
                .unwrap()
                .remove(0);
            assert_eq!((delivery.state.as_str(), delivery.attempts), ("pending", 0));
            assert_eq!(store.operations(100, 0).unwrap().len(), 1);
            assert!(store
                .inner
                .list_wecom_reservations(None, 100, 0)
                .unwrap()
                .is_empty());
            assert_eq!(
                store
                    .connection()
                    .query_row("SELECT count(*) FROM channel_cooldowns", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            store
                .connection()
                .execute_batch("DROP TRIGGER fail_claim")
                .unwrap();
            assert!(store
                .inner
                .claim_wecom_delivery_recorded(3, &Uuid::now_v7().to_string())
                .unwrap()
                .is_some());
            assert_eq!(store.operations(100, 0).unwrap().len(), 2);
        }
    }

    #[test]
    fn pending_respects_rolling_application_quota_and_persistent_send_cooldown() {
        let f = Fixture::new();
        let mut store = f.open();
        f.completed(&mut store, 1, vec!["reply".into()]);
        for _ in 0..200 {
            store
                .connection()
                .execute(
                    "INSERT INTO wecom_send_reservations VALUES('wwEnterprise:1',?1,1,0,1)",
                    [Uuid::new_v4().to_string()],
                )
                .unwrap();
        }
        assert_eq!(store.pending(4).unwrap(), (false, false));
        let next_day = 24 * 60 * 60 * 1000 + 2;
        assert_eq!(store.pending(next_day).unwrap(), (false, true));
        store
            .connection()
            .execute(
                "INSERT INTO channel_cooldowns VALUES('wecom','wwEnterprise:1',?1)",
                [next_day + 4000],
            )
            .unwrap();
        assert_eq!(store.pending(next_day + 3999).unwrap(), (false, false));
        assert_eq!(store.pending(next_day + 4000).unwrap(), (false, true));
        drop(store);
        let reopened = f.open();
        assert_eq!(reopened.pending(next_day + 3999).unwrap(), (false, false));
        assert_eq!(reopened.pending(next_day + 4000).unwrap(), (false, true));
        assert_eq!(
            reopened
                .inner
                .list_wecom_reservations(None, 100, 100)
                .unwrap()
                .len(),
            100
        );
    }

    #[test]
    fn permanent_operation_capacity_and_pages_fail_closed_after_purge() {
        let f = Fixture::new();
        let mut store = f.open();
        let event = f.completed(&mut store, 1, vec![]);
        store
            .inner
            .purge_wecom_channel_event_recorded(&event, 3)
            .unwrap();
        store.connection().execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<15999) INSERT INTO gateway_wecom_operations(request_id,kind,event_id,delivery_id,attempt,claimed_ms,cached_state) SELECT printf('00000000-0000-7000-8000-%012d',x),'event',printf('00000000-0000-4000-8000-%012d',x),NULL,1,x,'completed' FROM n;").unwrap();
        assert_eq!(store.pending(20000).unwrap(), (false, false));
        let duplicate = store
            .inner
            .accept_wecom_event_recorded(f.spec(1), 4)
            .unwrap();
        assert!(!duplicate.created);
        assert_eq!(duplicate.status, "purged");
        let error = store
            .inner
            .accept_wecom_event_recorded(f.spec(2), 4)
            .unwrap_err();
        assert!(error
            .downcast_ref::<channel_store::ChannelCapacity>()
            .is_some());
        assert_eq!(store.operations(100, 15900).unwrap().len(), 100);
        assert!(store.operations(100, 16000).unwrap().is_empty());
        for (limit, offset) in [(0, 0), (101, 0), (1, 16001)] {
            assert!(store.operations(limit, offset).is_err());
        }
        drop(store);
        let reopened = f.open();
        assert_eq!(reopened.pending(20000).unwrap(), (false, false));
        reopened.connection().execute("INSERT INTO gateway_wecom_operations(request_id,kind,event_id,delivery_id,attempt,claimed_ms) VALUES(?1,'event',?2,NULL,1,20000)",params![Uuid::now_v7().to_string(),Uuid::new_v4().to_string()]).unwrap();
        drop(reopened);
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
    }

    #[test]
    fn corrupt_original_and_cached_operation_metadata_is_bounded_and_never_serialized() {
        for (column, value) in [
            ("request_id", "12345678-1234-4234-9234-123456789012".into()),
            ("request_id", "ABCDEF01-2345-7678-9ABC-DEF012345678".into()),
            ("event_id", Uuid::nil().to_string()),
            ("delivery_id", Uuid::nil().to_string()),
            ("kind", "other".into()),
            ("attempt", "0".into()),
            ("claimed_ms", "-1".into()),
            ("cached_state", "secret-invalid-state".into()),
            ("cached_state", "unknown".into()),
            ("cached_state", "x".repeat(4097)),
            ("cached_receipt", "secret\ninvalid-receipt".into()),
            ("cached_receipt", "x".repeat(4097)),
            ("cached_reviewed_ms", "-1".into()),
        ] {
            let f = Fixture::new();
            let mut store = f.open();
            f.completed(&mut store, 1, vec!["reply".into()]);
            let request = Uuid::now_v7().to_string();
            let delivery = store
                .inner
                .claim_wecom_delivery_recorded(3, &request)
                .unwrap()
                .unwrap();
            store
                .inner
                .finish_channel_delivery(
                    &delivery.id,
                    1,
                    "delivered",
                    Some("validReceipt".into()),
                    None,
                    None,
                    4,
                )
                .unwrap();
            // Invalid cached values must also fail while valid live metadata would mask them.
            store.connection().execute_batch("PRAGMA ignore_check_constraints=ON; DROP TRIGGER gateway_wecom_operation_immutable_identity").unwrap();
            store
                .connection()
                .execute(
                    &format!("UPDATE gateway_wecom_operations SET {column}=?1 WHERE request_id=?2"),
                    params![value, request],
                )
                .unwrap();
            let error = store.operations(100, 0).unwrap_err().to_string();
            assert!(!error.contains("secret-invalid-state") && !error.contains("invalid-receipt"));
        }
        for field in ["sender_id", "session_id"] {
            let f = Fixture::new();
            let mut store = f.open();
            let event = f.completed(&mut store, 1, vec![]);
            let sql = if field == "sender_id" {
                "UPDATE channel_events SET spec=json_set(spec,'$.sender_id','another.member') WHERE id=?1"
            } else {
                "UPDATE channel_events SET session_id='wecom:another-owner' WHERE id=?1"
            };
            store.connection().execute(sql, [event]).unwrap();
            assert!(store.operations(100, 0).is_err());
        }
    }

    #[test]
    fn offline_absence_never_creates_files_and_uncommitted_residue_is_preserved() {
        let f = Fixture::new();
        assert!(WecomStore::open_existing(&f.registry, &f.binding)
            .unwrap()
            .is_none());
        assert!(!f.database().parent().unwrap().exists());
        for invalid in [Uuid::nil(), Uuid::from_u128(1)] {
            let mut wrong = f.binding.clone();
            wrong.id = invalid;
            assert!(WecomStore::open_existing(&f.registry, &wrong).is_err());
        }
        f.create_directory();
        assert!(WecomStore::open_existing(&f.registry, &f.binding)
            .unwrap()
            .is_none());
        assert_eq!(
            fs::read_dir(f.database().parent().unwrap())
                .unwrap()
                .count(),
            0
        );
        drop(create_private_file(&f.stage()).unwrap());
        assert!(WecomStore::open_existing(&f.registry, &f.binding).is_err());
        assert!(fs::read(f.stage()).unwrap().is_empty());
        assert!(!f.database().exists());
        assert!(!f.database().with_extension("sqlite3.lock").exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_current_uid_files_reject_permissions_hardlinks_symlinks_and_oversize() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let f = Fixture::new();
        drop(f.open());
        for path in [
            f.registry.clone(),
            f.database(),
            f.database().with_extension("sqlite3.lock"),
        ] {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 1);
        }
        let parent = f.database().parent().unwrap().to_path_buf();
        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        for path in [f.database(), f.database().with_extension("sqlite3.lock")] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(WecomStore::open(&f.registry, &f.binding).is_err());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let alias = path.with_extension("alias");
            fs::hard_link(&path, &alias).unwrap();
            assert!(WecomStore::open(&f.registry, &f.binding).is_err());
            fs::remove_file(alias).unwrap();
        }
        let sidecar = suffix(&f.database(), "-wal");
        symlink(f.database(), &sidecar).unwrap();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
        fs::remove_file(sidecar).unwrap();
        let source = f.database().with_extension("retained");
        fs::rename(f.database(), &source).unwrap();
        symlink(&source, f.database()).unwrap();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
        fs::remove_file(f.database()).unwrap();
        fs::rename(source, f.database()).unwrap();
        OpenOptions::new()
            .write(true)
            .open(f.database())
            .unwrap()
            .set_len(MAX_DATABASE_BYTES + 1)
            .unwrap();
        assert!(WecomStore::open(&f.registry, &f.binding).is_err());
    }
}
