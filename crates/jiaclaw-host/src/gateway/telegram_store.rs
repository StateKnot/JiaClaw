// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A private, immutable-owner channel database for one gateway Telegram binding.
use super::registry::TelegramBindingSummary;
use crate::{channel_store, store::SessionStore};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const APPLICATION_ID: i32 = 0x4a43_5447;
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXISTING_WAL_BYTES: u64 = 128 * 1024 * 1024;
const OWNER_SCHEMA: &str = "
CREATE TABLE gateway_telegram_owner (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 protocol INTEGER NOT NULL CHECK(protocol=1),
 binding_id TEXT NOT NULL, user_id TEXT NOT NULL, backend_id TEXT NOT NULL,
 bot_id TEXT NOT NULL, sender_id TEXT NOT NULL
);
CREATE TRIGGER gateway_telegram_owner_immutable_update BEFORE UPDATE ON gateway_telegram_owner
 BEGIN SELECT RAISE(ABORT,'Telegram database owner is immutable'); END;
CREATE TRIGGER gateway_telegram_owner_immutable_delete BEFORE DELETE ON gateway_telegram_owner
 BEGIN SELECT RAISE(ABORT,'Telegram database owner is immutable'); END;
";
const OPERATIONS_SCHEMA: &str = "
CREATE TABLE gateway_telegram_operations (
 request_id TEXT PRIMARY KEY NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('event','delivery')),
 event_id TEXT NOT NULL REFERENCES channel_events(id) ON DELETE CASCADE,
 delivery_id TEXT REFERENCES channel_outbox(id) ON DELETE CASCADE,
 attempt INTEGER NOT NULL CHECK(attempt BETWEEN 1 AND 5),
 claimed_ms INTEGER NOT NULL,
 CHECK((kind='event' AND delivery_id IS NULL AND attempt=1)
    OR (kind='delivery' AND delivery_id IS NOT NULL))
);
CREATE INDEX gateway_telegram_operations_event ON gateway_telegram_operations(event_id);
CREATE INDEX gateway_telegram_operations_delivery ON gateway_telegram_operations(delivery_id);
";

#[derive(Debug, Serialize)]
pub(super) struct TelegramOperation {
    pub request_id: String,
    pub kind: String,
    pub event_id: String,
    pub delivery_id: Option<String>,
    pub attempt: u32,
    pub claimed_ms: i64,
}

pub(super) struct TelegramStore {
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
        "Telegram state directories must not be symlinks"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o777 == 0o700,
            "Telegram state directories require mode 0700"
        );
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Telegram state files must be ordinary files, not links or special files"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.nlink() == 1 && metadata.permissions().mode() & 0o777 == 0o600,
            "Telegram state files require mode 0600 and one hard link"
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
            "Telegram state file changed during open"
        );
    }
    if created {
        file.sync_all()?;
        File::open(path.parent().context("Telegram file has no parent")?)?.sync_all()?;
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
                    "Telegram SQLite sidecar exceeds its startup bound"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn verify_owner(conn: &Connection, binding: &TelegramBindingSummary) -> Result<()> {
    let application: i32 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    ensure!(
        application == APPLICATION_ID,
        "refusing to adopt an unrelated Telegram channel database"
    );
    let owner: (i64, String, String, String, String, String) = conn.query_row(
        "SELECT protocol,binding_id,user_id,backend_id,bot_id,sender_id FROM gateway_telegram_owner WHERE singleton=1",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
    ).context("Telegram channel database owner is absent or invalid")?;
    ensure!(
        owner
            == (
                1,
                binding.id.to_string(),
                binding.user_id.to_string(),
                binding.backend_id.clone(),
                binding.bot_id.clone(),
                binding.sender_id.clone()
            ),
        "Telegram channel database belongs to a different binding"
    );
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        version == 0 || version == 10,
        "unsupported Telegram session schema"
    );
    if version == 0 {
        // A crash after committing the owner, before SessionStore's atomic
        // migrations, is safe to resume. Never adopt other unversioned tables.
        let tables: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            tables == 1,
            "unexpected tables in uninitialized Telegram database"
        );
    }
    let page_size: u64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
    let page_count: u64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    ensure!(
        page_size > 0 && page_count <= MAX_DATABASE_BYTES / page_size,
        "Telegram database exceeds 64 MiB"
    );
    Ok(())
}

impl TelegramStore {
    pub(super) fn open(registry_path: &Path, binding: &TelegramBindingSummary) -> Result<Self> {
        ensure!(
            registry_path.is_absolute()
                && !registry_path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir)),
            "Telegram registry path must be absolute without traversal"
        );
        ensure!(
            !binding.id.is_nil()
                && !binding.user_id.is_nil()
                && !binding.backend_id.is_empty()
                && binding.backend_id.len() <= 64
                && binding
                    .backend_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
            "invalid Telegram binding owner"
        );
        for id in [&binding.bot_id, &binding.sender_id] {
            ensure!(
                id.parse::<i64>()
                    .ok()
                    .is_some_and(|number| number > 0 && number.to_string() == *id),
                "Telegram owner IDs must be canonical positive integers"
            );
        }
        let parent = registry_path
            .parent()
            .context("Telegram registry has no parent")?;
        private_directory(parent)?;
        private_file(registry_path)?;
        let parent = parent.canonicalize()?;
        let directory = parent.join("telegram");
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
            .context("another process owns this Telegram channel database")?;
        let ownership = crate::store::DatabaseOwnership::new(ownership);
        verify_sidecars(&path)?;
        let (file, created) = create_private_file(&path)?;
        ensure!(
            file.metadata()?.len() <= MAX_DATABASE_BYTES,
            "Telegram database exceeds 64 MiB"
        );
        drop(file);
        if created {
            let mut conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?;
            conn.busy_timeout(Duration::from_millis(250))?;
            conn.pragma_update(None, "synchronous", "FULL")?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(OWNER_SCHEMA)?;
            tx.execute(
                "INSERT INTO gateway_telegram_owner VALUES(1,1,?1,?2,?3,?4,?5)",
                params![
                    binding.id.to_string(),
                    binding.user_id.to_string(),
                    binding.backend_id,
                    binding.bot_id,
                    binding.sender_id
                ],
            )?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.commit()?;
        }
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
            "Telegram database exceeds its page capacity"
        );
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=2097152; PRAGMA trusted_schema=OFF;",
        )?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='gateway_telegram_operations')",
            [], |r| r.get(0),
        )?;
        if !exists {
            let used: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channel_events) OR EXISTS(SELECT 1 FROM channel_outbox)",
                [], |r| r.get(0),
            )?;
            ensure!(
                !used,
                "Telegram operation ledger is missing from a populated database"
            );
            tx.execute_batch(OPERATIONS_SCHEMA)?;
        }
        tx.commit()?;
        let mut store = Self { inner };
        store.inner.recover_channels(now_ms()?)?;
        verify_sidecars(&path)?;
        Ok(store)
    }

    fn connection(&self) -> &Connection {
        let SessionStore::Sqlite { conn, .. } = &self.inner else {
            unreachable!()
        };
        conn
    }

    pub(super) fn operations(&self, limit: usize, offset: usize) -> Result<Vec<TelegramOperation>> {
        ensure!(
            (1..=100).contains(&limit) && offset <= channel_store::MAX_TELEGRAM_OPERATIONS,
            "Telegram operations page requires limit 1..100 and offset at most 16000"
        );
        let mut statement = self.connection().prepare(
            "SELECT request_id,kind,event_id,delivery_id,attempt,claimed_ms
             FROM gateway_telegram_operations ORDER BY claimed_ms DESC,request_id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(
            params![i64::try_from(limit)?, i64::try_from(offset)?],
            |row| {
                Ok(TelegramOperation {
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
        let count: usize = conn.query_row(
            "SELECT count(*) FROM gateway_telegram_operations",
            [],
            |r| r.get(0),
        )?;
        if count >= channel_store::MAX_TELEGRAM_OPERATIONS {
            return Ok((false, false));
        }
        let execution: bool = conn.query_row(
            "SELECT (SELECT count(*) FROM channel_events WHERE status='processing')<4
             AND EXISTS(SELECT 1 FROM channel_events e WHERE channel='telegram' AND status='received'
             AND NOT EXISTS(SELECT 1 FROM channel_events p WHERE p.session_id=e.session_id AND p.status='processing'))",
            [], |r| r.get(0),
        )?;
        let execution = execution && channel_store::has_outbox_capacity(conn)?;
        let delivery: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_outbox d
             WHERE d.channel='telegram' AND d.event_id IS NOT NULL
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
    use super::*;
    use crate::channel_types::{Channel, Destination, EventSpec};
    use uuid::Uuid;

    struct Fixture {
        directory: PathBuf,
        registry: PathBuf,
        binding: TelegramBindingSummary,
    }
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("jiaclaw-telegram-store-{}", Uuid::new_v4()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory).unwrap();
            let registry = directory.join("registry.sqlite3");
            create_private_file(&registry).unwrap();
            let binding = TelegramBindingSummary {
                id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                backend_id: "alice".into(),
                bot_id: "123456".into(),
                sender_id: "654321".into(),
                enabled: true,
            };
            Self {
                directory,
                registry,
                binding,
            }
        }
        fn open(&self) -> TelegramStore {
            TelegramStore::open(&self.registry, &self.binding).unwrap()
        }
        fn database(&self) -> PathBuf {
            self.directory
                .join("telegram")
                .join(format!("{}.sqlite3", self.binding.id))
        }
        fn spec(&self, event: &str) -> EventSpec {
            EventSpec {
                event_id: event.into(),
                session_id: format!("telegram:{}", self.binding.id),
                sender_id: self.binding.sender_id.clone(),
                prompt: "hello".into(),
                enabled_tools: vec!["datetime_now".into()],
                timeout_secs: 120,
                destination: Destination {
                    channel: Channel::Telegram,
                    installation_id: self.binding.bot_id.clone(),
                    conversation_id: self.binding.sender_id.clone(),
                    thread_id: None,
                    interaction_id: None,
                    expires_ms: None,
                },
                sealed_token: None,
                fingerprint: "f".repeat(64),
            }
        }
        fn complete(&self, store: &mut TelegramStore, event: &str, chunks: &[&str]) -> String {
            let accepted = store
                .inner
                .accept_channel_event(self.spec(event), 0)
                .unwrap();
            let claimed = store
                .inner
                .claim_channel_event_recorded(0, &request())
                .unwrap()
                .unwrap();
            assert_eq!(accepted.id, claimed.id);
            assert!(store
                .inner
                .complete_channel_event(
                    &claimed.id,
                    None,
                    "completed",
                    chunks.iter().map(|s| (*s).into()).collect(),
                    None,
                    0
                )
                .unwrap());
            claimed.id
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
    fn request() -> String {
        Uuid::now_v7().to_string()
    }

    #[test]
    fn private_owner_reopens_without_rebinding_or_adopting_ordinary_session_databases() {
        let fixture = Fixture::new();
        let store = fixture.open();
        assert!(TelegramStore::open(&fixture.registry, &fixture.binding).is_err());
        assert_eq!(
            store
                .connection()
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            store
                .connection()
                .pragma_query_value(None, "max_page_count", |r| r.get::<_, u64>(0))
                .unwrap()
                * store
                    .connection()
                    .pragma_query_value(None, "page_size", |r| r.get::<_, u64>(0))
                    .unwrap(),
            MAX_DATABASE_BYTES
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(fixture.database())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(fixture.database().parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(store
            .connection()
            .execute("UPDATE gateway_telegram_owner SET bot_id='1'", [])
            .is_err());
        assert!(store
            .connection()
            .execute("DELETE FROM gateway_telegram_owner", [])
            .is_err());
        drop(store);
        for field in ["user", "backend", "bot", "sender"] {
            let mut wrong = fixture.binding.clone();
            match field {
                "user" => wrong.user_id = Uuid::new_v4(),
                "backend" => wrong.backend_id = "bob".into(),
                "bot" => wrong.bot_id = "123457".into(),
                _ => wrong.sender_id = "654322".into(),
            }
            assert!(
                TelegramStore::open(&fixture.registry, &wrong).is_err(),
                "{field}"
            );
        }
        let mut disabled = fixture.binding.clone();
        disabled.enabled = false;
        drop(TelegramStore::open(&fixture.registry, &disabled).unwrap());

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
        assert!(TelegramStore::open(&ordinary.registry, &ordinary.binding).is_err());
        assert_eq!(fs::read(ordinary.database()).unwrap(), before);

        let unrelated_binding = Fixture::new();
        let store = unrelated_binding.open();
        drop(store);
        let mut moved = unrelated_binding.binding.clone();
        moved.id = Uuid::new_v4();
        fs::rename(
            unrelated_binding.database(),
            unrelated_binding
                .directory
                .join("telegram")
                .join(format!("{}.sqlite3", moved.id)),
        )
        .unwrap();
        assert!(TelegramStore::open(&unrelated_binding.registry, &moved).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn private_paths_reject_links_special_files_permissions_and_oversized_existing_state() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        for suffix_to_replace in ["", "-wal", "-shm", "-journal", ".lock"] {
            let fixture = Fixture::new();
            drop(fixture.open());
            let target = if suffix_to_replace == ".lock" {
                fixture.database().with_extension("sqlite3.lock")
            } else {
                suffix(&fixture.database(), suffix_to_replace)
            };
            if target.exists() {
                fs::remove_file(&target).unwrap();
            }
            symlink(&fixture.registry, &target).unwrap();
            assert!(
                TelegramStore::open(&fixture.registry, &fixture.binding).is_err(),
                "{suffix_to_replace}"
            );
            assert_eq!(fs::metadata(&fixture.registry).unwrap().len(), 0);
        }
        let fixture = Fixture::new();
        drop(fixture.open());
        fs::hard_link(fixture.database(), fixture.directory.join("hardlink")).unwrap();
        assert!(TelegramStore::open(&fixture.registry, &fixture.binding).is_err());

        let fixture = Fixture::new();
        drop(fixture.open());
        fs::remove_file(fixture.database()).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(fixture.database())
            .status()
            .unwrap()
            .success());
        assert!(TelegramStore::open(&fixture.registry, &fixture.binding).is_err());

        let fixture = Fixture::new();
        drop(fixture.open());
        fs::set_permissions(fixture.database(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(TelegramStore::open(&fixture.registry, &fixture.binding).is_err());

        let fixture = Fixture::new();
        let target = fixture.directory.join("real-telegram");
        fs::create_dir(&target).unwrap();
        symlink(&target, fixture.directory.join("telegram")).unwrap();
        assert!(TelegramStore::open(&fixture.registry, &fixture.binding).is_err());

        let fixture = Fixture::new();
        drop(fixture.open());
        OpenOptions::new()
            .write(true)
            .open(fixture.database())
            .unwrap()
            .set_len(MAX_DATABASE_BYTES + 1)
            .unwrap();
        assert!(TelegramStore::open(&fixture.registry, &fixture.binding).is_err());
    }

    #[test]
    fn recorded_claims_rollback_both_claim_and_operation_when_receipt_storage_fails() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let accepted = store
            .inner
            .accept_channel_event(fixture.spec("rollback"), 0)
            .unwrap();
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER reject_operation BEFORE INSERT ON gateway_telegram_operations
             BEGIN SELECT RAISE(ABORT,'injected operation failure'); END;",
            )
            .unwrap();
        let event_request = request();
        assert!(store
            .inner
            .claim_channel_event_recorded(1, &event_request)
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
            .execute_batch("DROP TRIGGER reject_operation;")
            .unwrap();
        store
            .inner
            .claim_channel_event_recorded(1, &event_request)
            .unwrap()
            .unwrap();
        store
            .inner
            .complete_channel_event(
                &accepted.id,
                None,
                "completed",
                vec!["answer".into()],
                None,
                2,
            )
            .unwrap();
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER reject_operation BEFORE INSERT ON gateway_telegram_operations
             BEGIN SELECT RAISE(ABORT,'injected operation failure'); END;",
            )
            .unwrap();
        let delivery_request = request();
        assert!(store
            .inner
            .claim_channel_delivery_recorded(3, &delivery_request)
            .is_err());
        let delivery = store
            .inner
            .list_channel_deliveries(Some(&accepted.id), 100, 0)
            .unwrap()
            .remove(0);
        assert_eq!((delivery.state.as_str(), delivery.attempts), ("pending", 0));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM channel_cooldowns", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(store.operations(100, 0).unwrap().len(), 1);
        store
            .connection()
            .execute_batch("DROP TRIGGER reject_operation;")
            .unwrap();

        // SQLITE_FULL follows the same rollback path, without a partial attempt.
        store
            .connection()
            .execute_batch(
                "CREATE TABLE injected_full(payload BLOB);
            CREATE TRIGGER full_operation BEFORE INSERT ON gateway_telegram_operations
            BEGIN INSERT INTO injected_full VALUES(zeroblob(1048576)); END;",
            )
            .unwrap();
        let pages: u64 = store
            .connection()
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        store
            .connection()
            .pragma_update(None, "max_page_count", pages + 4)
            .unwrap();
        let error = store
            .inner
            .claim_channel_delivery_recorded(3, &delivery_request)
            .unwrap_err();
        assert!(matches!(error.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(code,_)) if code.code == rusqlite::ErrorCode::DiskFull));
        let delivery = store
            .inner
            .list_channel_deliveries(Some(&accepted.id), 100, 0)
            .unwrap()
            .remove(0);
        assert_eq!((delivery.state.as_str(), delivery.attempts), ("pending", 0));
        assert_eq!(store.operations(100, 0).unwrap().len(), 1);
    }

    #[test]
    fn repeated_request_ids_and_noncanonical_uuids_cannot_claim_another_operation() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let one = store
            .inner
            .accept_channel_event(fixture.spec("one"), 0)
            .unwrap();
        let event_request = request();
        for invalid in [
            Uuid::new_v4().to_string(),
            "01900000-0000-7000-8000-00000000000A".into(),
            "01900000-0000-7000-0000-00000000000a".into(),
            "invalid".into(),
        ] {
            assert!(store
                .inner
                .claim_channel_event_recorded(1, &invalid)
                .is_err());
        }
        store
            .inner
            .claim_channel_event_recorded(1, &event_request)
            .unwrap()
            .unwrap();
        store
            .inner
            .complete_channel_event(&one.id, None, "completed", vec!["one".into()], None, 2)
            .unwrap();
        let two = store
            .inner
            .accept_channel_event(fixture.spec("two"), 3)
            .unwrap();
        assert!(store
            .inner
            .claim_channel_event_recorded(4, &event_request)
            .is_err());
        assert_eq!(
            store
                .inner
                .get_channel_event(&two.id)
                .unwrap()
                .unwrap()
                .status,
            "received"
        );
        assert!(store
            .inner
            .claim_channel_delivery_recorded(4, &event_request)
            .is_err());
        let send_request = request();
        let delivery = store
            .inner
            .claim_channel_delivery_recorded(4, &send_request)
            .unwrap()
            .unwrap();
        store
            .inner
            .finish_channel_delivery(
                &delivery.id,
                delivery.attempts,
                "retry_wait",
                None,
                None,
                Some(5000),
                5,
            )
            .unwrap();
        assert!(store
            .inner
            .claim_channel_delivery_recorded(6000, &send_request)
            .is_err());
        let same = store
            .inner
            .get_channel_delivery(&delivery.id)
            .unwrap()
            .unwrap();
        assert_eq!((same.state.as_str(), same.attempts), ("retry_wait", 1));
        let retried = store
            .inner
            .claim_channel_delivery_recorded(6000, &request())
            .unwrap()
            .unwrap();
        assert_eq!(retried.attempts, 2);
        assert_eq!(store.operations(100, 0).unwrap().len(), 3);
    }

    #[test]
    fn restart_preserves_claim_associations_unknown_fifo_and_atomic_purge_cascades() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let completed = fixture.complete(&mut store, "first", &["one", "two"]);
        let sent = store
            .inner
            .claim_channel_delivery_recorded(0, &request())
            .unwrap()
            .unwrap();
        let received = store
            .inner
            .accept_channel_event(fixture.spec("interrupted"), 0)
            .unwrap();
        store
            .inner
            .claim_channel_event_recorded(0, &request())
            .unwrap()
            .unwrap();
        assert_eq!(store.operations(100, 0).unwrap().len(), 3);
        drop(store);
        let mut store = fixture.open();
        assert_eq!(
            store
                .inner
                .get_channel_event(&received.id)
                .unwrap()
                .unwrap()
                .status,
            "needs_review"
        );
        let unknown = store.inner.get_channel_delivery(&sent.id).unwrap().unwrap();
        assert_eq!((unknown.state.as_str(), unknown.attempts), ("unknown", 1));
        assert_eq!(store.pending(i64::MAX - 10_000).unwrap(), (false, false));
        let operations = store.operations(100, 0).unwrap();
        assert_eq!(operations.len(), 3);
        let send = operations.iter().find(|op| op.kind == "delivery").unwrap();
        assert_eq!(send.delivery_id.as_deref(), Some(sent.id.as_str()));
        assert_eq!(send.attempt, 1);
        assert!(store.inner.purge_channel_event(&completed, 100).is_err());
        store
            .inner
            .resolve_channel_delivery(&sent.id, "confirmed receipt".into(), 100)
            .unwrap();
        assert_eq!(store.pending(3100).unwrap(), (false, true));
        store.inner.cancel_channel_event(&completed, 3200).unwrap();
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER block_event_purge BEFORE DELETE ON channel_events
             BEGIN SELECT RAISE(ABORT,'injected purge failure'); END;",
            )
            .unwrap();
        assert!(store.inner.purge_channel_event(&completed, 4000).is_err());
        assert_eq!(store.operations(100, 0).unwrap().len(), 3);
        assert_eq!(
            store
                .inner
                .list_channel_deliveries(Some(&completed), 100, 0)
                .unwrap()
                .len(),
            2
        );
        store
            .connection()
            .execute_batch("DROP TRIGGER block_event_purge;")
            .unwrap();
        assert!(store.inner.purge_channel_event(&completed, 4000).unwrap());
        assert_eq!(store.operations(100, 0).unwrap().len(), 1);
        store
            .inner
            .cancel_channel_event(&received.id, 4001)
            .unwrap();
        store.inner.purge_channel_event(&received.id, 4001).unwrap();
        assert!(store.operations(100, 0).unwrap().is_empty());
    }

    #[test]
    fn operation_quota_blocks_new_claims_but_allows_reopen_audit_and_reviewed_purge() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let finished = fixture.complete(&mut store, "quota-owner", &[]);
        let SessionStore::Sqlite { conn, .. } = &mut store.inner else {
            unreachable!()
        };
        let tx = conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO gateway_telegram_operations
                VALUES(?1,'event',?2,NULL,1,0)",
                )
                .unwrap();
            for _ in 1..channel_store::MAX_TELEGRAM_OPERATIONS {
                insert.execute(params![request(), finished]).unwrap();
            }
        }
        tx.commit().unwrap();
        let waiting = store
            .inner
            .accept_channel_event(fixture.spec("waiting"), 1)
            .unwrap();
        assert_eq!(store.pending(1).unwrap(), (false, false));
        assert!(store
            .inner
            .claim_channel_event_recorded(1, &request())
            .is_err());
        assert!(store
            .inner
            .claim_channel_delivery_recorded(1, &request())
            .is_err());
        assert_eq!(
            store
                .inner
                .get_channel_event(&waiting.id)
                .unwrap()
                .unwrap()
                .status,
            "received"
        );
        assert_eq!(store.operations(100, 15900).unwrap().len(), 100);
        assert!(
            store.operations(0, 0).is_err()
                && store.operations(101, 0).is_err()
                && store.operations(1, 16001).is_err()
        );
        drop(store);
        let mut store = fixture.open();
        assert_eq!(store.operations(100, 15900).unwrap().len(), 100);
        assert!(store.inner.purge_channel_event(&finished, 2).unwrap());
        assert_eq!(store.pending(2).unwrap(), (true, false));
        assert_eq!(
            store
                .inner
                .claim_channel_event_recorded(2, &request())
                .unwrap()
                .unwrap()
                .id,
            waiting.id
        );
    }

    #[test]
    fn pending_hints_preserve_attempt_cooldown_and_per_destination_fifo() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        assert_eq!(store.pending(0).unwrap(), (false, false));
        let accepted = store
            .inner
            .accept_channel_event(fixture.spec("hints"), 0)
            .unwrap();
        assert_eq!(store.pending(0).unwrap(), (true, false));
        store
            .inner
            .claim_channel_event_recorded(0, &request())
            .unwrap()
            .unwrap();
        assert_eq!(store.pending(0).unwrap(), (false, false));
        store
            .inner
            .complete_channel_event(
                &accepted.id,
                None,
                "completed",
                vec!["one".into(), "two".into()],
                None,
                0,
            )
            .unwrap();
        assert_eq!(store.pending(0).unwrap(), (false, true));
        let first = store
            .inner
            .claim_channel_delivery_recorded(0, &request())
            .unwrap()
            .unwrap();
        assert_eq!(store.pending(5000).unwrap(), (false, false));
        store
            .inner
            .finish_channel_delivery(&first.id, 1, "retry_wait", None, None, Some(5000), 1)
            .unwrap();
        assert_eq!(store.pending(4999).unwrap(), (false, false));
        assert_eq!(store.pending(5000).unwrap(), (false, true));
        let retry = store
            .inner
            .claim_channel_delivery_recorded(5000, &request())
            .unwrap()
            .unwrap();
        store
            .inner
            .finish_channel_delivery(
                &retry.id,
                2,
                "delivered",
                Some("receipt".into()),
                None,
                None,
                5001,
            )
            .unwrap();
        assert_eq!(store.pending(8099).unwrap(), (false, false));
        assert_eq!(store.pending(8100).unwrap(), (false, true));
    }
}
