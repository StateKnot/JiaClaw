// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A private, immutable-owner channel database for one gateway Discord binding.
use super::registry::DiscordBindingSummary;
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

const APPLICATION_ID: i32 = 0x4a43_4443;
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXISTING_WAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INITIALIZING_BYTES: u64 = 128 * 1024;
const OWNER_SCHEMA: &str = "
CREATE TABLE gateway_discord_owner (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 protocol INTEGER NOT NULL CHECK(protocol=3),
 binding_id TEXT NOT NULL, user_id TEXT NOT NULL, backend_id TEXT NOT NULL,
 application_id TEXT NOT NULL, verify_key TEXT NOT NULL, bot_user_id TEXT NOT NULL,
 sender_id TEXT NOT NULL, conversation_id TEXT NOT NULL, command_id TEXT NOT NULL,
 state_key_fingerprint TEXT NOT NULL CHECK(length(state_key_fingerprint)=64
 AND length(CAST(state_key_fingerprint AS BLOB))=64
 AND state_key_fingerprint NOT GLOB '*[^0-9a-f]*'
 AND state_key_fingerprint<>'0000000000000000000000000000000000000000000000000000000000000000')
);
CREATE TRIGGER gateway_discord_owner_no_replace BEFORE INSERT ON gateway_discord_owner
 WHEN EXISTS(SELECT 1 FROM gateway_discord_owner)
 BEGIN SELECT RAISE(ABORT,'Discord database owner is immutable'); END;
CREATE TRIGGER gateway_discord_owner_immutable_update BEFORE UPDATE ON gateway_discord_owner
 BEGIN SELECT RAISE(ABORT,'Discord database owner is immutable'); END;
CREATE TRIGGER gateway_discord_owner_immutable_delete BEFORE DELETE ON gateway_discord_owner
 BEGIN SELECT RAISE(ABORT,'Discord database owner is immutable'); END;
";
const OPERATIONS_SCHEMA: &str = "
CREATE TABLE gateway_discord_operations (
 request_id TEXT PRIMARY KEY NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('event','delivery')),
 event_id TEXT NOT NULL REFERENCES channel_events(id) ON DELETE CASCADE,
 delivery_id TEXT REFERENCES channel_outbox(id) ON DELETE CASCADE,
 attempt INTEGER NOT NULL CHECK(attempt BETWEEN 1 AND 5),
 claimed_ms INTEGER NOT NULL,
 CHECK((kind='event' AND delivery_id IS NULL AND attempt=1)
    OR (kind='delivery' AND delivery_id IS NOT NULL))
);
CREATE INDEX gateway_discord_operations_event ON gateway_discord_operations(event_id);
CREATE INDEX gateway_discord_operations_delivery ON gateway_discord_operations(delivery_id);
";

#[derive(Debug, Serialize)]
pub(super) struct DiscordOperation {
    pub request_id: String,
    pub kind: String,
    pub event_id: String,
    pub delivery_id: Option<String>,
    pub attempt: u32,
    pub claimed_ms: i64,
}

pub(super) struct DiscordStore {
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
        "Discord state directories must not be symlinks"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o777 == 0o700,
            "Discord state directories require mode 0700"
        );
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Discord state files must be ordinary files, not links or special files"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.nlink() == 1 && metadata.permissions().mode() & 0o777 == 0o600,
            "Discord state files require mode 0600 and one hard link"
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
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino() && opened.nlink() == 1,
            "Discord state file changed during open"
        );
    }
    if created {
        file.sync_all()?;
        File::open(path.parent().context("Discord file has no parent")?)?.sync_all()?;
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
                    "Discord SQLite sidecar exceeds its startup bound"
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
                    "Discord initialization sidecar exceeds its bound"
                );
                anyhow::bail!(
                    "Discord initialization sidecars require offline administrator review"
                );
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

fn canonical_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

fn verify_initializing_owner(
    conn: &Connection,
    binding: &DiscordBindingSummary,
    expected_fingerprint: Option<&str>,
) -> Result<String> {
    let fingerprint = verify_owner(conn, binding, expected_fingerprint)?;
    let journal: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    ensure!(
        journal == "delete",
        "Discord staging owner requires a single-file DELETE journal"
    );
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        version == 0,
        "Discord staging database must contain only its initial owner"
    );
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(OWNER_SCHEMA)?;
    ensure!(
        owner_schema(conn)? == owner_schema(&expected)?,
        "Discord staging owner schema is incomplete or unrelated"
    );
    let count: i64 = conn.query_row("SELECT count(*) FROM gateway_discord_owner", [], |row| {
        row.get(0)
    })?;
    ensure!(
        count == 1,
        "Discord staging database must contain exactly one owner"
    );
    Ok(fingerprint)
}

fn write_initial_owner(
    conn: &mut Connection,
    binding: &DiscordBindingSummary,
    fingerprint: &str,
) -> Result<()> {
    ensure!(
        canonical_fingerprint(fingerprint),
        "invalid Discord state-key fingerprint"
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(OWNER_SCHEMA)?;
    tx.execute(
        "INSERT INTO gateway_discord_owner VALUES(1,3,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            binding.id.to_string(),
            binding.user_id.to_string(),
            binding.backend_id,
            binding.application_id,
            binding.verify_key,
            binding.bot_user_id,
            binding.sender_id,
            binding.conversation_id,
            binding.command_id,
            fingerprint
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
fn prepare_initial_owner(
    stage: &Path,
    binding: &DiscordBindingSummary,
    expected_fingerprint: Option<&str>,
) -> Result<File> {
    require_no_initializing_sidecars(stage)?;
    let (file, _) = create_private_file(stage)?;
    ensure!(
        file.metadata()?.len() <= MAX_INITIALIZING_BYTES,
        "Discord staging database exceeds 128 KiB"
    );
    if file.metadata()?.len() > 0 {
        // SQLite READ_ONLY may still create WAL/SHM files in a writable
        // directory. Reject WAL and incomplete headers before opening SQLite,
        // so inspecting a foreign stage cannot change even its sidecars.
        ensure!(
            file.metadata()?.len() >= 100,
            "Discord staging SQLite header is incomplete"
        );
        let mut header = [0_u8; 20];
        (&file).read_exact(&mut header)?;
        ensure!(
            &header[..16] == b"SQLite format 3\0" && header[18] == 1 && header[19] == 1,
            "Discord staging requires a complete rollback-journal SQLite header"
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
            verify_initializing_owner(&conn, binding, expected_fingerprint)?;
        }
        empty
    };
    if needs_owner {
        let fingerprint = expected_fingerprint.context("empty Discord initialization requires the configured state-key fingerprint; inspect offline before maintenance")?;
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
            "Discord initialization requires a single-file DELETE journal"
        );
        conn.pragma_update(None, "synchronous", "FULL")?;
        write_initial_owner(&mut conn, binding, fingerprint)?;
        conn.close()
            .map_err(|(_, error)| error)
            .context("Discord initialization connection failed to close")?;
    }
    {
        let conn = Connection::open_with_flags(
            stage,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        verify_initializing_owner(&conn, binding, expected_fingerprint)?;
        conn.close()
            .map_err(|(_, error)| error)
            .context("Discord initialization verification failed to close")?;
    }
    require_no_initializing_sidecars(stage)?;
    let metadata = private_file(stage)?;
    ensure!(
        metadata.len() <= MAX_INITIALIZING_BYTES,
        "Discord staging database exceeds 128 KiB"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            opened.dev() == metadata.dev() && opened.ino() == metadata.ino() && opened.nlink() == 1,
            "Discord staging file changed during initialization"
        );
    }
    file.sync_all()?;
    Ok(file)
}

fn publish_initial_owner(stage: &Path, path: &Path) -> Result<()> {
    let parent = path.parent().context("Discord database has no parent")?;
    ensure!(
        stage.parent() == Some(parent),
        "Discord owner publication must stay in one directory"
    );
    let directory = File::open(parent)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        rustix::fs::renameat_with(
            &directory,
            stage
                .file_name()
                .context("Discord staging file has no name")?,
            &directory,
            path.file_name().context("Discord database has no name")?,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
        .context("Discord owner publication cannot replace existing state")?;
        directory.sync_all()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        anyhow::bail!("atomic Discord owner publication requires Linux or macOS");
    }
}

fn verify_owner(
    conn: &Connection,
    binding: &DiscordBindingSummary,
    expected_fingerprint: Option<&str>,
) -> Result<String> {
    let application: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    ensure!(
        application == APPLICATION_ID,
        "refusing to adopt an unrelated Discord channel database"
    );
    let expected_owner = Connection::open_in_memory()?;
    expected_owner.execute_batch(OWNER_SCHEMA)?;
    let objects = owner_schema(conn)?
        .into_iter()
        .filter(|row| row.2 == "gateway_discord_owner")
        .collect::<Vec<_>>();
    ensure!(
        objects == owner_schema(&expected_owner)?,
        "Discord immutable owner schema is incomplete or unrelated"
    );
    let owner: (i64, String, String, String, String, String, String, String, String, String, String) = conn.query_row(
        "SELECT protocol,binding_id,user_id,backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id,state_key_fingerprint FROM gateway_discord_owner WHERE singleton=1",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?)),
    ).context("Discord channel database owner is absent or invalid")?;
    ensure!(
        owner.0 == 3
            && owner.1 == binding.id.to_string()
            && owner.2 == binding.user_id.to_string()
            && owner.3 == binding.backend_id
            && owner.4 == binding.application_id
            && owner.5 == binding.verify_key
            && owner.6 == binding.bot_user_id
            && owner.7 == binding.sender_id
            && owner.8 == binding.conversation_id
            && owner.9 == binding.command_id,
        "Discord channel database belongs to a different binding"
    );
    ensure!(
        canonical_fingerprint(&owner.10),
        "Discord database state-key fingerprint is invalid"
    );
    ensure!(expected_fingerprint.is_none_or(|expected| expected == owner.10), "Discord database belongs to a different state key; encrypted interaction credentials require the original key");
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        version == 0 || version == 10,
        "unsupported Discord session schema"
    );
    if version == 0 {
        // Owner-only publication can precede SessionStore's atomic migrations.
        // No other unversioned schema objects may be adopted.
        let expected = Connection::open_in_memory()?;
        expected.execute_batch(OWNER_SCHEMA)?;
        ensure!(
            owner_schema(conn)? == owner_schema(&expected)?,
            "unexpected schema in uninitialized Discord database"
        );
    }
    let page_size: u64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let page_count: u64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    ensure!(
        page_size > 0 && page_count <= MAX_DATABASE_BYTES / page_size,
        "Discord database exceeds 64 MiB"
    );
    Ok(owner.10)
}

fn validate_binding(registry_path: &Path, binding: &DiscordBindingSummary) -> Result<()> {
    ensure!(
        registry_path.is_absolute()
            && !registry_path
                .components()
                .any(|part| matches!(part, Component::ParentDir)),
        "Discord registry path must be absolute without traversal"
    );
    ensure!(
        !binding.id.is_nil()
            && binding.id.get_variant() == uuid::Variant::RFC4122
            && !binding.user_id.is_nil()
            && binding.user_id.get_variant() == uuid::Variant::RFC4122
            && !binding.backend_id.is_empty()
            && binding.backend_id.len() <= 64
            && binding
                .backend_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "invalid Discord binding owner"
    );
    for id in [
        &binding.application_id,
        &binding.bot_user_id,
        &binding.sender_id,
        &binding.conversation_id,
        &binding.command_id,
    ] {
        ensure!(
            id.parse::<u64>()
                .ok()
                .is_some_and(|value| value != 0 && value.to_string() == *id),
            "Discord owner IDs must be canonical nonzero unsigned snowflakes"
        );
    }
    ensure!(
        canonical_fingerprint(&binding.verify_key),
        "Discord owner verification key must be canonical nonzero lower-case hex"
    );
    ensure!(
        binding.sender_id != binding.bot_user_id,
        "Discord sender must be a distinct human"
    );
    Ok(())
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn verify_final_header(path: &Path) -> Result<()> {
    let metadata = private_file(path)?;
    ensure!(
        metadata.len() >= 100 && metadata.len() <= MAX_DATABASE_BYTES,
        "Discord SQLite header is incomplete or exceeds its startup bound"
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
            "Discord final state changed during header inspection"
        );
    }
    let mut header = [0_u8; 100];
    (&file).read_exact(&mut header)?;
    ensure!(
        &header[..16] == b"SQLite format 3\0"
            && matches!(header[18], 1 | 2)
            && header[18] == header[19]
            && header[68..72] == APPLICATION_ID.to_be_bytes(),
        "refusing an unrelated or incomplete Discord SQLite header"
    );
    Ok(())
}

impl DiscordStore {
    /// Runtime initialization requires the exact fingerprint of the configured
    /// sealing key. A replaced key is rejected before recovery or any worker.
    pub(super) fn open(
        registry_path: &Path,
        binding: &DiscordBindingSummary,
        state_key_fingerprint: &str,
    ) -> Result<Self> {
        ensure!(
            canonical_fingerprint(state_key_fingerprint),
            "invalid Discord state-key fingerprint"
        );
        Self::open_owned(registry_path, binding, Some(state_key_fingerprint))?
            .context("Discord runtime did not initialize its channel database")
    }

    /// Stopped maintenance checks the persisted non-secret key identity without
    /// loading a sealing key. Absent state remains absent. Only an already
    /// committed owner stage can be published without a configured fingerprint;
    /// empty, partial or sidecar initialization requires administrator review.
    pub(super) fn open_existing(
        registry_path: &Path,
        binding: &DiscordBindingSummary,
    ) -> Result<Option<Self>> {
        Self::open_owned(registry_path, binding, None)
    }

    fn open_owned(
        registry_path: &Path,
        binding: &DiscordBindingSummary,
        expected_fingerprint: Option<&str>,
    ) -> Result<Option<Self>> {
        validate_binding(registry_path, binding)?;
        let parent = registry_path
            .parent()
            .context("Discord registry has no parent")?;
        private_directory(parent)?;
        private_file(registry_path)?;
        let parent = parent.canonicalize()?;
        let directory = parent.join("discord");
        if expected_fingerprint.is_none() && !exists(&directory)? {
            return Ok(None);
        }
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
        let stage = suffix(&path, ".initializing");
        if expected_fingerprint.is_none() && !exists(&path)? && !exists(&stage)? {
            require_no_initializing_sidecars(&path)?;
            require_no_initializing_sidecars(&stage)?;
            return Ok(None);
        }
        // Acquire SessionStore's exact lifetime flock before opening any SQLite
        // descriptor; POSIX SQLite locks are otherwise released on descriptor close.
        let (ownership, _) = create_private_file(&path.with_extension("sqlite3.lock"))?;
        ownership
            .try_lock_exclusive()
            .context("another process owns this Discord channel database")?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                ensure!(
                    private_file(&path)?.len() <= MAX_DATABASE_BYTES,
                    "Discord database exceeds 64 MiB"
                );
                verify_sidecars(&path)?;
                // The immutable owner/application marker is committed in the
                // published main file before WAL mode. Check it directly so a
                // foreign WAL header cannot create sidecars during inspection.
                verify_final_header(&path)?;
                // A staging residue has no authority over an existing final file;
                // preserve it for explicit offline review.
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                require_no_initializing_sidecars(&path)?;
                if expected_fingerprint.is_none() && !exists(&stage)? {
                    require_no_initializing_sidecars(&stage)?;
                    return Ok(None);
                }
                let staged_file = prepare_initial_owner(&stage, binding, expected_fingerprint)?;
                require_no_initializing_sidecars(&path)?;
                publish_initial_owner(&stage, &path)?;
                drop(staged_file);
            }
            Err(error) => return Err(error.into()),
        }
        let fingerprint = {
            let conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?;
            conn.busy_timeout(Duration::from_millis(250))?;
            verify_owner(&conn, binding, expected_fingerprint)?
        };
        let mut inner = SessionStore::open_with_ownership(&path, Some(ownership))?;
        let SessionStore::Sqlite { conn, .. } = &mut inner else {
            unreachable!()
        };
        conn.busy_timeout(Duration::from_millis(250))?;
        verify_owner(conn, binding, Some(&fingerprint))?;
        let page_size: u64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
        let maximum = MAX_DATABASE_BYTES / page_size;
        let applied: u64 =
            conn.query_row(&format!("PRAGMA max_page_count={maximum}"), [], |row| {
                row.get(0)
            })?;
        ensure!(
            applied == maximum,
            "Discord database exceeds its page capacity"
        );
        conn.execute_batch("PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=2097152; PRAGMA trusted_schema=OFF;")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='gateway_discord_operations')", [], |row| row.get(0))?;
        if !exists {
            let used: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_events) OR EXISTS(SELECT 1 FROM channel_outbox)", [], |row| row.get(0))?;
            ensure!(
                !used,
                "Discord operation ledger is missing from a populated database"
            );
            tx.execute_batch(OPERATIONS_SCHEMA)?;
        }
        tx.commit()?;
        let mut store = Self { inner };
        store.inner.recover_channels(now_ms()?)?;
        verify_sidecars(&path)?;
        Ok(Some(store))
    }

    fn connection(&self) -> &Connection {
        let SessionStore::Sqlite { conn, .. } = &self.inner else {
            unreachable!()
        };
        conn
    }

    pub(super) fn operations(&self, limit: usize, offset: usize) -> Result<Vec<DiscordOperation>> {
        ensure!(
            (1..=100).contains(&limit) && offset <= channel_store::MAX_SLACK_OPERATIONS,
            "Discord operations page requires limit 1..100 and offset at most 16000"
        );
        let mut statement = self.connection().prepare(
            "SELECT request_id,kind,event_id,delivery_id,attempt,claimed_ms
             FROM gateway_discord_operations ORDER BY claimed_ms DESC,request_id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(
            params![i64::try_from(limit)?, i64::try_from(offset)?],
            |row| {
                Ok(DiscordOperation {
                    request_id: row.get(0)?,
                    kind: row.get(1)?,
                    event_id: row.get(2)?,
                    delivery_id: row.get(3)?,
                    attempt: row.get(4)?,
                    claimed_ms: row.get(5)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A non-authorizing hint. The recorded claim and registry admission recheck
    /// their conditions before a model request or an external send can start.
    pub(super) fn pending(&self, now: i64) -> Result<(bool, bool)> {
        let conn = self.connection();
        let count: usize =
            conn.query_row("SELECT count(*) FROM gateway_discord_operations", [], |r| {
                r.get(0)
            })?;
        if count >= channel_store::MAX_SLACK_OPERATIONS {
            return Ok((false, false));
        }
        let execution: bool = conn.query_row(
            "SELECT (SELECT count(*) FROM channel_events WHERE status='processing')<4
             AND EXISTS(SELECT 1 FROM channel_events e WHERE channel='discord' AND status='received'
             AND NOT EXISTS(SELECT 1 FROM channel_events p WHERE p.session_id=e.session_id AND p.status='processing'))",
            [], |r| r.get(0),
        )?;
        let execution = execution && channel_store::has_outbox_capacity(conn)?;
        let delivery: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_outbox expired
             WHERE expired.channel='discord' AND expired.event_id IS NOT NULL
             AND expired.state IN ('pending','retry_wait') AND expired.expires_ms<=?1)
             OR EXISTS(SELECT 1 FROM channel_outbox d
             WHERE d.channel='discord' AND d.event_id IS NOT NULL
             AND state IN ('pending','retry_wait') AND next_attempt_ms<=?1 AND attempts<5
             AND NOT EXISTS(SELECT 1 FROM channel_outbox live WHERE live.channel=d.channel
                AND live.installation_id=d.installation_id AND live.state='submitting')
             AND NOT EXISTS(SELECT 1 FROM channel_cooldowns c WHERE c.channel=d.channel
                AND c.installation_id=d.installation_id AND c.until_ms>?1)
             AND NOT EXISTS(SELECT 1 FROM channel_outbox p WHERE p.channel=d.channel
                AND p.installation_id=d.installation_id AND p.destination_key=d.destination_key
                AND p.seq<d.seq AND p.state<>'delivered'
                AND (p.event_id=d.event_id OR p.job_run_id=d.job_run_id OR p.state<>'cancelled')))",
            [now],
            |r| r.get(0),
        )?;
        Ok((execution, delivery))
    }
}
#[cfg(test)]
mod tests {
    use super::super::registry::Registry;
    use super::*;
    use crate::channel_types::{Channel, Destination, EventSpec};
    use uuid::Uuid;

    const STATE_KEY_FINGERPRINT: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const VERIFY_KEY: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    struct Fixture {
        directory: PathBuf,
        registry: PathBuf,
        binding: DiscordBindingSummary,
    }
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("jiaclaw-discord-owner-{}", Uuid::new_v4()));
            let registry = directory.join("users.sqlite3");
            let identities = Registry::open(&registry).unwrap();
            let user = identities.add_user("alice").unwrap();
            let binding = identities
                .add_discord_binding(user.user_id, "1", VERIFY_KEY, "10", "11", "12", "13")
                .unwrap();
            Self {
                directory,
                registry,
                binding,
            }
        }
        fn database(&self) -> PathBuf {
            self.directory
                .join("discord")
                .join(format!("{}.sqlite3", self.binding.id))
        }
        fn stage(&self) -> PathBuf {
            suffix(&self.database(), ".initializing")
        }
        fn create_directory(&self) {
            let parent = self.database().parent().unwrap().to_path_buf();
            fs::create_dir(&parent).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        fn empty_stage(&self) {
            self.create_directory();
            drop(create_private_file(&self.stage()).unwrap());
        }
        fn committed_stage(&self) {
            self.owner_only();
            fs::rename(self.database(), self.stage()).unwrap();
        }
        fn open(&self) -> DiscordStore {
            DiscordStore::open(&self.registry, &self.binding, STATE_KEY_FINGERPRINT).unwrap()
        }
        fn maintenance(&self) -> Result<Option<DiscordStore>> {
            DiscordStore::open_existing(&self.registry, &self.binding)
        }
        fn spec(&self, name: &str) -> EventSpec {
            EventSpec {
                event_id: format!("interaction-{name}"),
                session_id: format!("discord:{}", self.binding.id),
                sender_id: self.binding.sender_id.clone(),
                prompt: "private Discord text".into(),
                enabled_tools: vec!["datetime_now".into(), "json_query".into()],
                timeout_secs: 120,
                destination: Destination {
                    channel: Channel::Discord,
                    installation_id: self.binding.application_id.clone(),
                    conversation_id: self.binding.conversation_id.clone(),
                    thread_id: None,
                    interaction_id: Some("345678901234567890".into()),
                    expires_ms: Some(now_ms().unwrap() + 900_000),
                },
                sealed_token: Some("sealed-fixture-token".into()),
                fingerprint: "a".repeat(64),
            }
        }
        fn completed(&self, store: &mut DiscordStore, name: &str) -> String {
            let event = store
                .inner
                .accept_channel_event(self.spec(name), 0)
                .unwrap();
            let claimed = store
                .inner
                .claim_discord_event_recorded(1, &Uuid::now_v7().to_string())
                .unwrap()
                .unwrap();
            assert_eq!(claimed.id, event.id);
            assert!(store
                .inner
                .complete_channel_event(
                    &event.id,
                    None,
                    "completed",
                    vec!["private reply".into()],
                    None,
                    2
                )
                .unwrap());
            event.id
        }
        fn owner_only(&self) {
            self.create_directory();
            drop(create_private_file(&self.database()).unwrap());
            let mut conn = Connection::open(self.database()).unwrap();
            write_initial_owner(&mut conn, &self.binding, STATE_KEY_FINGERPRINT).unwrap();
        }
    }

    #[test]
    fn staging_crashes_before_owner_and_after_commit_resume_without_orphan_accumulation() {
        for committed in [false, true] {
            let fixture = Fixture::new();
            if committed {
                fixture.committed_stage();
            } else {
                fixture.empty_stage();
            }
            assert!(!fixture.database().exists());
            let store = fixture.open();
            verify_owner(
                store.connection(),
                &fixture.binding,
                Some(STATE_KEY_FINGERPRINT),
            )
            .unwrap();
            assert!(store.operations(100, 0).unwrap().is_empty());
            assert!(!fixture.stage().exists());
            assert!(!fs::read_dir(fixture.database().parent().unwrap())
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("initializing")));
            drop(store);
            drop(fixture.open());
        }
    }

    #[test]
    fn foreign_or_partial_staging_is_rejected_unchanged_before_any_writable_adoption() {
        for kind in 0..10 {
            let fixture = Fixture::new();
            if (4..7).contains(&kind) {
                fixture.committed_stage();
            } else {
                fixture.empty_stage();
            }
            let conn = Connection::open(fixture.stage()).unwrap();
            match kind {
                0 => conn.execute_batch("CREATE TABLE foreign_state(value TEXT)").unwrap(),
                1 => conn.execute_batch("CREATE VIEW unrelated_view AS SELECT 1").unwrap(),
                2 => conn.pragma_update(None,"application_id",123).unwrap(),
                3 => conn.pragma_update(None,"user_version",1).unwrap(),
                4 => conn.execute_batch("DROP TRIGGER gateway_discord_owner_immutable_update; UPDATE gateway_discord_owner SET backend_id='bob'").unwrap(),
                5 => conn.execute_batch("DROP TRIGGER gateway_discord_owner_no_replace").unwrap(),
                6 => conn.execute_batch("CREATE TABLE extra_state(value TEXT)").unwrap(),
                7 => conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE foreign_wal(value TEXT); PRAGMA wal_checkpoint(TRUNCATE)").unwrap(),
                9 => conn.execute_batch("CREATE TABLE sqliteforeign(value TEXT)").unwrap(),
                _ => (),
            }
            drop(conn);
            // A standalone checkpointed WAL-mode header must not make a
            // read-only identity check create new sidecars.
            if kind == 7 {
                for extra in ["-wal", "-shm"] {
                    let _ = fs::remove_file(suffix(&fixture.stage(), extra));
                }
            }
            if kind == 8 {
                fs::write(fixture.stage(), b"incomplete staging header").unwrap();
            }
            let before = fs::read(fixture.stage()).unwrap();
            assert!(
                DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                    .is_err(),
                "stage {kind}"
            );
            assert_eq!(fs::read(fixture.stage()).unwrap(), before, "stage {kind}");
            for extra in ["-wal", "-shm", "-journal"] {
                assert!(
                    !suffix(&fixture.stage(), extra).exists(),
                    "stage {kind} {extra}"
                );
            }
            assert!(!fixture.database().exists());
        }
    }

    #[test]
    fn staging_and_unpublished_final_sidecars_require_review_without_changing_any_bytes() {
        for staged in [false, true] {
            for extra in ["-wal", "-shm", "-journal"] {
                let fixture = Fixture::new();
                fixture.committed_stage();
                let path = if staged {
                    fixture.stage()
                } else {
                    fixture.database()
                };
                let sidecar = suffix(&path, extra);
                let (mut file, _) = create_private_file(&sidecar).unwrap();
                use std::io::Write;
                file.write_all(b"unverified sidecar residue").unwrap();
                file.sync_all().unwrap();
                drop(file);
                let stage_before = fs::read(fixture.stage()).unwrap();
                let sidecar_before = fs::read(&sidecar).unwrap();
                assert!(
                    DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                        .is_err(),
                    "{staged} {extra}"
                );
                assert_eq!(fs::read(fixture.stage()).unwrap(), stage_before);
                assert_eq!(fs::read(sidecar).unwrap(), sidecar_before);
                assert!(!fixture.database().exists());
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn atomic_owner_publication_never_replaces_an_existing_target() {
        let fixture = Fixture::new();
        fixture.committed_stage();
        let before = fs::read(fixture.stage()).unwrap();
        let (mut file, _) = create_private_file(&fixture.database()).unwrap();
        use std::io::Write;
        file.write_all(b"existing unrelated final state").unwrap();
        drop(file);
        let final_before = fs::read(fixture.database()).unwrap();
        let error = publish_initial_owner(&fixture.stage(), &fixture.database()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(fixture.database()).unwrap(), final_before);
        assert_eq!(fs::read(fixture.stage()).unwrap(), before);
        assert!(
            DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT).is_err()
        );
        assert_eq!(fs::read(fixture.database()).unwrap(), final_before);
        assert_eq!(fs::read(fixture.stage()).unwrap(), before);
    }

    #[test]
    fn initial_owner_sql_failure_rolls_back_before_publication_and_a_clean_stage_can_retry() {
        let fixture = Fixture::new();
        fixture.empty_stage();
        let mut conn = Connection::open(fixture.stage()).unwrap();
        conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA max_page_count=1")
            .unwrap();
        assert!(write_initial_owner(&mut conn, &fixture.binding, STATE_KEY_FINGERPRINT).is_err());
        assert!(owner_schema(&conn).unwrap().is_empty());
        assert_eq!(
            conn.pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
                .unwrap(),
            0
        );
        assert!(!fixture.database().exists());
        conn.close().map_err(|(_, error)| error).unwrap();
        // The failed transaction left no partial owner authority, so the next
        // startup may safely commit the owner and publish this same stage.
        drop(fixture.open());
        assert!(fixture.database().exists());
        assert!(!fixture.stage().exists());
    }

    #[test]
    fn existing_final_ignores_staging_and_never_adopts_an_unknown_empty_final() {
        let fixture = Fixture::new();
        drop(fixture.open());
        drop(create_private_file(&fixture.stage()).unwrap());
        let conn = Connection::open(fixture.stage()).unwrap();
        conn.execute_batch("CREATE TABLE unrelated(value TEXT)")
            .unwrap();
        drop(conn);
        let before = fs::read(fixture.stage()).unwrap();
        drop(fixture.open());
        assert_eq!(fs::read(fixture.stage()).unwrap(), before);
        let empty = Fixture::new();
        empty.committed_stage();
        drop(create_private_file(&empty.database()).unwrap());
        let before = fs::read(empty.stage()).unwrap();
        assert!(
            DiscordStore::open(&empty.registry, &empty.binding, STATE_KEY_FINGERPRINT).is_err()
        );
        assert!(fs::read(empty.database()).unwrap().is_empty());
        assert_eq!(fs::read(empty.stage()).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn private_staging_rejects_links_permissions_and_small_resource_overflow_without_publication() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        for kind in 0..4 {
            let fixture = Fixture::new();
            fixture.empty_stage();
            match kind {
                0 => {
                    fs::remove_file(fixture.stage()).unwrap();
                    symlink(&fixture.registry, fixture.stage()).unwrap();
                }
                1 => fs::hard_link(fixture.stage(), fixture.directory.join("stage-alias")).unwrap(),
                2 => {
                    fs::set_permissions(fixture.stage(), fs::Permissions::from_mode(0o640)).unwrap()
                }
                _ => OpenOptions::new()
                    .write(true)
                    .open(fixture.stage())
                    .unwrap()
                    .set_len(MAX_INITIALIZING_BYTES + 1)
                    .unwrap(),
            }
            assert!(
                DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                    .is_err(),
                "stage {kind}"
            );
            assert!(!fixture.database().exists());
            assert!(fs::symlink_metadata(fixture.stage()).is_ok());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn all_discord_owner_fields_fingerprint_and_lifetime_lock_are_verified_before_reuse() {
        let fixture = Fixture::new();
        let store = fixture.open();
        assert!(
            DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT).is_err()
        );
        assert!(fixture.maintenance().is_err());
        for sql in [
            "UPDATE gateway_discord_owner SET application_id='2'",
            "UPDATE gateway_discord_owner SET state_key_fingerprint='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'",
            "DELETE FROM gateway_discord_owner",
            "INSERT OR REPLACE INTO gateway_discord_owner SELECT * FROM gateway_discord_owner",
        ] { assert!(store.connection().execute(sql, []).is_err(), "{sql}"); }
        drop(store);
        let before = fs::read(fixture.database()).unwrap();
        assert!(DiscordStore::open(&fixture.registry, &fixture.binding, &"b".repeat(64)).is_err());
        assert_eq!(fs::read(fixture.database()).unwrap(), before);
        for field in 0..9 {
            let mut wrong = fixture.binding.clone();
            match field {
                0 => wrong.user_id = Uuid::new_v4(),
                1 => wrong.backend_id = "bob".into(),
                2 => wrong.application_id = "2".into(),
                3 => wrong.verify_key = "e".repeat(64),
                4 => wrong.bot_user_id = "20".into(),
                5 => wrong.sender_id = "21".into(),
                6 => wrong.conversation_id = "22".into(),
                7 => wrong.command_id = "23".into(),
                _ => {
                    wrong.id = Uuid::new_v4();
                    fs::copy(
                        fixture.database(),
                        fixture
                            .directory
                            .join("discord")
                            .join(format!("{}.sqlite3", wrong.id)),
                    )
                    .unwrap();
                }
            }
            assert!(
                DiscordStore::open(&fixture.registry, &wrong, STATE_KEY_FINGERPRINT).is_err(),
                "owner field {field}"
            );
            assert!(
                DiscordStore::open_existing(&fixture.registry, &wrong).is_err(),
                "maintenance owner field {field}"
            );
        }
        let mut revoked = fixture.binding.clone();
        revoked.enabled = false;
        drop(
            DiscordStore::open_existing(&fixture.registry, &revoked)
                .unwrap()
                .unwrap(),
        );
        drop(DiscordStore::open(&fixture.registry, &revoked, STATE_KEY_FINGERPRINT).unwrap());
    }

    #[test]
    fn unrelated_identity_or_future_version_is_rejected_without_migration() {
        for foreign_application in [false, true] {
            let fixture = Fixture::new();
            drop(fixture.open());
            let conn = Connection::open(fixture.database()).unwrap();
            if foreign_application {
                conn.pragma_update(None, "application_id", 0x4a43_5447)
                    .unwrap();
            } else {
                conn.pragma_update(None, "user_version", 11).unwrap();
            }
            drop(conn);
            let before = fs::read(fixture.database()).unwrap();
            assert!(
                DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                    .is_err()
            );
            assert_eq!(fs::read(fixture.database()).unwrap(), before);
        }
        let ordinary = Fixture::new();
        fs::create_dir(ordinary.database().parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                ordinary.database().parent().unwrap(),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        drop(SessionStore::open(&ordinary.database()).unwrap());
        let before = fs::read(ordinary.database()).unwrap();
        assert!(
            DiscordStore::open(&ordinary.registry, &ordinary.binding, STATE_KEY_FINGERPRINT)
                .is_err()
        );
        assert_eq!(fs::read(ordinary.database()).unwrap(), before);
    }

    #[test]
    fn owner_only_crash_boundary_resumes_but_extra_unversioned_tables_are_not_adopted() {
        let fixture = Fixture::new();
        fixture.owner_only();
        let store = fixture.open();
        assert_eq!(
            store
                .connection()
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert!(store.operations(100, 0).unwrap().is_empty());
        drop(store);
        let unrelated = Fixture::new();
        unrelated.owner_only();
        let conn = Connection::open(unrelated.database()).unwrap();
        conn.execute_batch("CREATE TABLE unrelated_private_state(value TEXT)")
            .unwrap();
        drop(conn);
        let before = fs::read(unrelated.database()).unwrap();
        assert!(DiscordStore::open(
            &unrelated.registry,
            &unrelated.binding,
            STATE_KEY_FINGERPRINT
        )
        .is_err());
        assert_eq!(fs::read(unrelated.database()).unwrap(), before);
    }

    #[test]
    fn recorded_discord_claims_are_atomic_and_cannot_claim_another_channel() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let accepted = store
            .inner
            .accept_channel_event(fixture.spec("atomic"), 0)
            .unwrap();
        store.connection().execute_batch("CREATE TRIGGER fail_operation BEFORE INSERT ON gateway_discord_operations BEGIN SELECT RAISE(ABORT,'operation storage failure'); END;").unwrap();
        let request = Uuid::now_v7().to_string();
        assert!(store
            .inner
            .claim_discord_event_recorded(1, &request)
            .is_err());
        assert_eq!(
            store
                .inner
                .get_channel_event(&accepted.id)
                .unwrap()
                .unwrap()
                .status,
            "received"
        );
        assert!(store.operations(100, 0).unwrap().is_empty());
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_operation")
            .unwrap();
        store
            .inner
            .claim_discord_event_recorded(1, &request)
            .unwrap()
            .unwrap();
        store
            .inner
            .complete_channel_event(
                &accepted.id,
                None,
                "completed",
                vec!["reply".into()],
                None,
                2,
            )
            .unwrap();
        store.connection().execute_batch("CREATE TRIGGER fail_operation BEFORE INSERT ON gateway_discord_operations BEGIN SELECT RAISE(ABORT,'operation storage failure'); END;").unwrap();
        assert!(store
            .inner
            .claim_discord_delivery_recorded(3, &Uuid::now_v7().to_string())
            .is_err());
        let delivery = store
            .inner
            .list_channel_deliveries(Some(&accepted.id), 100, 0)
            .unwrap()
            .remove(0);
        assert_eq!((delivery.state.as_str(), delivery.attempts), ("pending", 0));
        assert_eq!(store.operations(100, 0).unwrap().len(), 1);
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM channel_cooldowns", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(store);
        let other = Fixture::new();
        let mut store = other.open();
        let mut spec = other.spec("foreign");
        spec.destination.channel = Channel::Telegram;
        spec.destination.installation_id = "101".into();
        spec.destination.conversation_id = "201".into();
        spec.destination.interaction_id = None;
        spec.destination.expires_ms = None;
        spec.sealed_token = None;
        let event = store.inner.accept_channel_event(spec, 0).unwrap();
        assert!(store
            .inner
            .claim_discord_event_recorded(1, &Uuid::now_v7().to_string())
            .is_err());
        assert_eq!(
            store
                .inner
                .get_channel_event(&event.id)
                .unwrap()
                .unwrap()
                .status,
            "received"
        );
        assert!(store.operations(100, 0).unwrap().is_empty());
    }

    #[test]
    fn restart_marks_unknown_work_and_preserves_original_discord_operation_links_without_replay() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let completed = fixture.completed(&mut store, "sent");
        let send_request = Uuid::now_v7().to_string();
        let delivery = store
            .inner
            .claim_discord_delivery_recorded(3, &send_request)
            .unwrap()
            .unwrap();
        let unresolved = store
            .inner
            .accept_channel_event(fixture.spec("processing"), 4)
            .unwrap();
        let model_request = Uuid::now_v7().to_string();
        store
            .inner
            .claim_discord_event_recorded(5, &model_request)
            .unwrap()
            .unwrap();
        let before = serde_json::to_value(store.operations(100, 0).unwrap()).unwrap();
        drop(store);
        let mut reopened = fixture.open();
        assert_eq!(
            serde_json::to_value(reopened.operations(100, 0).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            reopened
                .inner
                .get_channel_event(&unresolved.id)
                .unwrap()
                .unwrap()
                .status,
            "needs_review"
        );
        let after = reopened
            .inner
            .get_channel_delivery(&delivery.id)
            .unwrap()
            .unwrap();
        assert_eq!((after.state.as_str(), after.attempts), ("unknown", 1));
        assert_eq!(after.event_id.as_deref(), Some(completed.as_str()));
        assert_eq!(reopened.pending(10_000).unwrap(), (false, false));
        assert!(reopened
            .inner
            .claim_discord_event_recorded(10_000, &Uuid::now_v7().to_string())
            .unwrap()
            .is_none());
        assert!(reopened
            .inner
            .claim_discord_delivery_recorded(10_000, &Uuid::now_v7().to_string())
            .unwrap()
            .is_none());
        for (limit, offset) in [(0, 0), (101, 0), (1, 16001)] {
            assert!(reopened.operations(limit, offset).is_err());
        }
    }

    #[test]
    fn missing_ledger_cannot_be_synthesized_for_existing_events_or_deliveries() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        fixture.completed(&mut store, "prior");
        store
            .connection()
            .execute_batch("DROP TABLE gateway_discord_operations")
            .unwrap();
        drop(store);
        assert!(
            DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT).is_err()
        );
        let empty = Fixture::new();
        let store = empty.open();
        store
            .connection()
            .execute_batch("DROP TABLE gateway_discord_operations")
            .unwrap();
        drop(store);
        assert!(empty.open().operations(100, 0).unwrap().is_empty());
    }

    #[test]
    fn sixty_four_mib_page_quota_rejects_growth_atomically_without_losing_owner_or_ledger() {
        let fixture = Fixture::new();
        let store = fixture.open();
        let page_size = store
            .connection()
            .pragma_query_value(None, "page_size", |row| row.get::<_, u64>(0))
            .unwrap();
        let page_limit = store
            .connection()
            .pragma_query_value(None, "max_page_count", |row| row.get::<_, u64>(0))
            .unwrap();
        assert_eq!(page_size * page_limit, MAX_DATABASE_BYTES);
        store
            .connection()
            .execute_batch("CREATE TABLE capacity_probe(payload BLOB)")
            .unwrap();
        let error = store
            .connection()
            .execute(
                "INSERT INTO capacity_probe VALUES(zeroblob(?1))",
                [i64::try_from(MAX_DATABASE_BYTES).unwrap()],
            )
            .unwrap_err();
        assert!(
            matches!(error,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::DiskFull)
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM capacity_probe", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        verify_owner(
            store.connection(),
            &fixture.binding,
            Some(STATE_KEY_FINGERPRINT),
        )
        .unwrap();
        assert!(store.operations(100, 0).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn private_discord_files_reject_link_aliases_permissions_and_oversized_existing_state() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        for extra in ["", "-wal", "-shm", "-journal", ".lock"] {
            let fixture = Fixture::new();
            drop(fixture.open());
            let target = if extra == ".lock" {
                fixture.database().with_extension("sqlite3.lock")
            } else {
                suffix(&fixture.database(), extra)
            };
            if target.exists() {
                fs::remove_file(&target).unwrap();
            }
            symlink(&fixture.registry, &target).unwrap();
            assert!(
                DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                    .is_err(),
                "{extra}"
            );
        }
        let fixture = Fixture::new();
        drop(fixture.open());
        fs::hard_link(fixture.database(), fixture.directory.join("alias.sqlite3")).unwrap();
        assert!(
            DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT).is_err()
        );
        let fixture = Fixture::new();
        drop(fixture.open());
        fs::set_permissions(fixture.database(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(
            DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT).is_err()
        );
        let fixture = Fixture::new();
        drop(fixture.open());
        OpenOptions::new()
            .write(true)
            .open(fixture.database())
            .unwrap()
            .set_len(MAX_DATABASE_BYTES + 1)
            .unwrap();
        assert!(
            DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT).is_err()
        );
    }
    #[test]
    fn maintenance_absence_is_observed_without_creating_any_channel_state() {
        let fixture = Fixture::new();
        let directory = fixture.database().parent().unwrap().to_path_buf();
        assert!(fixture.maintenance().unwrap().is_none());
        assert!(!directory.exists());
        fixture.create_directory();
        let before = fs::read_dir(&directory).unwrap().count();
        assert!(fixture.maintenance().unwrap().is_none());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), before);
        assert!(!fixture.database().with_extension("sqlite3.lock").exists());
        assert!(!fixture.database().exists());
        assert!(!fixture.stage().exists());
    }

    #[test]
    fn maintenance_uses_committed_nonsecret_fingerprint_and_refuses_empty_or_partial_stage() {
        let complete = Fixture::new();
        complete.committed_stage();
        let mut revoked = complete.binding.clone();
        revoked.enabled = false;
        let store = DiscordStore::open_existing(&complete.registry, &revoked)
            .unwrap()
            .unwrap();
        assert_eq!(
            verify_owner(store.connection(), &revoked, None).unwrap(),
            STATE_KEY_FINGERPRINT
        );
        assert!(complete.database().exists());
        assert!(!complete.stage().exists());
        drop(store);
        let reused = DiscordStore::open_existing(&complete.registry, &revoked)
            .unwrap()
            .unwrap();
        assert!(reused.operations(100, 0).unwrap().is_empty());
        drop(reused);
        for kind in 0..3 {
            let fixture = Fixture::new();
            fixture.empty_stage();
            if kind == 1 {
                fs::write(fixture.stage(), b"partial SQLite owner").unwrap();
            } else if kind == 2 {
                let conn = Connection::open(fixture.stage()).unwrap();
                conn.execute_batch("CREATE TABLE unrelated(value TEXT)")
                    .unwrap();
                drop(conn);
            }
            let before = fs::read(fixture.stage()).unwrap();
            assert!(fixture.maintenance().is_err(), "stage {kind}");
            assert_eq!(fs::read(fixture.stage()).unwrap(), before);
            assert!(!fixture.database().exists());
            for extra in ["-wal", "-shm", "-journal"] {
                assert!(!suffix(&fixture.stage(), extra).exists());
            }
        }
    }

    #[test]
    fn maintenance_residual_sidecars_without_database_are_not_misreported_as_absence() {
        use std::io::Write;
        for staged in [false, true] {
            for extra in ["-wal", "-shm", "-journal"] {
                let fixture = Fixture::new();
                fixture.create_directory();
                let base = if staged {
                    fixture.stage()
                } else {
                    fixture.database()
                };
                let sidecar = suffix(&base, extra);
                let (mut file, _) = create_private_file(&sidecar).unwrap();
                file.write_all(b"unowned recovery residue").unwrap();
                drop(file);
                let before = fs::read(&sidecar).unwrap();
                assert!(fixture.maintenance().is_err(), "{staged} {extra}");
                assert_eq!(fs::read(&sidecar).unwrap(), before);
                assert!(!fixture.database().exists());
                assert!(!fixture.stage().exists());
                assert!(!fixture.database().with_extension("sqlite3.lock").exists());
            }
        }
    }

    #[test]
    fn canonical_nonzero_key_fingerprint_is_mandatory_at_runtime_and_in_persisted_owner() {
        for bad in [
            "".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "0".repeat(64),
            "g".repeat(64),
        ] {
            let fixture = Fixture::new();
            assert!(DiscordStore::open(&fixture.registry, &fixture.binding, &bad).is_err());
            assert!(!fixture.database().parent().unwrap().exists());
            assert!(fixture.maintenance().unwrap().is_none());
        }
        let fixture = Fixture::new();
        fixture.committed_stage();
        let before = fs::read(fixture.stage()).unwrap();
        assert!(DiscordStore::open(&fixture.registry, &fixture.binding, &"b".repeat(64)).is_err());
        assert_eq!(fs::read(fixture.stage()).unwrap(), before);
        assert!(!fixture.database().exists());
        for bad in ["0".repeat(64), "A".repeat(64), "a".repeat(63)] {
            let fixture = Fixture::new();
            drop(fixture.open());
            let conn = Connection::open(fixture.database()).unwrap();
            conn.execute_batch("PRAGMA ignore_check_constraints=ON; DROP TRIGGER gateway_discord_owner_immutable_update;").unwrap();
            conn.execute(
                "UPDATE gateway_discord_owner SET state_key_fingerprint=?1",
                [bad],
            )
            .unwrap();
            conn.execute_batch("CREATE TRIGGER gateway_discord_owner_immutable_update BEFORE UPDATE ON gateway_discord_owner\n BEGIN SELECT RAISE(ABORT,'Discord database owner is immutable'); END;").unwrap();
            drop(conn);
            let before = fs::read(fixture.database()).unwrap();
            assert!(fixture.maintenance().is_err());
            assert!(
                DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                    .is_err()
            );
            assert_eq!(fs::read(fixture.database()).unwrap(), before);
        }
    }

    #[test]
    fn foreign_wal_and_partial_final_headers_are_rejected_without_creating_sqlite_sidecars() {
        for kind in 0..3 {
            let fixture = Fixture::new();
            fixture.create_directory();
            drop(create_private_file(&fixture.database()).unwrap());
            if kind == 0 {
                let conn = Connection::open(fixture.database()).unwrap();
                conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE foreign_wal(value TEXT); PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
                drop(conn);
            } else if kind == 1 {
                fs::write(fixture.database(), b"incomplete SQLite header").unwrap();
            }
            for extra in ["-wal", "-shm", "-journal"] {
                let _ = fs::remove_file(suffix(&fixture.database(), extra));
            }
            let before = fs::read(fixture.database()).unwrap();
            assert!(fixture.maintenance().is_err());
            assert!(
                DiscordStore::open(&fixture.registry, &fixture.binding, STATE_KEY_FINGERPRINT)
                    .is_err()
            );
            assert_eq!(fs::read(fixture.database()).unwrap(), before);
            for extra in ["-wal", "-shm", "-journal"] {
                assert!(!suffix(&fixture.database(), extra).exists());
            }
        }
    }

    #[test]
    fn expired_blocked_deliveries_prompt_a_claim_to_persist_expiry_without_recording_a_send() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let event = store
            .inner
            .accept_channel_event(fixture.spec("expires"), 0)
            .unwrap();
        store
            .inner
            .claim_discord_event_recorded(1, &Uuid::now_v7().to_string())
            .unwrap()
            .unwrap();
        assert!(store
            .inner
            .complete_channel_event(
                &event.id,
                None,
                "completed",
                vec!["first".into(), "second".into()],
                None,
                2
            )
            .unwrap());
        // The second part is blocked by the first, by attempts, by a future
        // retry, and by cooldown. Both must still drive durable expiry cleanup.
        store.connection().execute_batch("UPDATE channel_outbox SET expires_ms=20,attempts=5,next_attempt_ms=1000; INSERT INTO channel_cooldowns(channel,installation_id,until_ms) VALUES('discord','1',1000);").unwrap();
        assert_eq!(store.pending(19).unwrap(), (false, false));
        assert_eq!(store.pending(20).unwrap(), (false, true));
        assert!(store
            .inner
            .claim_discord_delivery_recorded(20, &Uuid::now_v7().to_string())
            .unwrap()
            .is_none());
        let deliveries = store
            .inner
            .list_channel_deliveries(Some(&event.id), 100, 0)
            .unwrap();
        assert_eq!(deliveries.len(), 2);
        assert!(deliveries
            .iter()
            .all(|delivery| delivery.state == "expired" && delivery.attempts == 5));
        assert_eq!(store.operations(100, 0).unwrap().len(), 1);
        assert_eq!(store.pending(21).unwrap(), (false, false));
    }
}
