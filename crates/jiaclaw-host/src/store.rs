// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Authoritative `SQLite` session storage and an explicit ephemeral backend.
use super::{ListSessionsResponse, SessionRecord, SessionSummary};
use anyhow::{Context, Result};
use jiaclaw_core::ChatMessage;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

/// Shared by standalone admission and the authenticated gateway route allowlist.
/// The opaque cursor is lowercase hex of the last ID's exact UTF-8 bytes.
pub(super) struct SessionPageQuery {
    pub(super) limit: usize,
    after: Option<String>,
}
impl SessionPageQuery {
    pub(super) fn parse(raw: Option<&str>) -> Result<Self> {
        let mut query = Self {
            limit: 50,
            after: None,
        };
        let Some(raw) = raw else { return Ok(query) };
        anyhow::ensure!(
            !raw.is_empty() && raw.len() <= 2080,
            "invalid session page query"
        );
        let mut seen = HashSet::new();
        for part in raw.split('&') {
            let (key, value) = part.split_once('=').context("invalid session page query")?;
            anyhow::ensure!(seen.insert(key), "duplicate session page query");
            match key {
                "limit" => {
                    anyhow::ensure!(
                        !value.is_empty()
                            && !value.starts_with('0')
                            && value.bytes().all(|b| b.is_ascii_digit()),
                        "invalid session page limit"
                    );
                    query.limit = value.parse()?;
                    anyhow::ensure!(
                        (1..=50).contains(&query.limit),
                        "invalid session page limit"
                    );
                }
                "after" => {
                    anyhow::ensure!(
                        (2..=2048).contains(&value.len())
                            && value.len() % 2 == 0
                            && value
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                        "invalid session page cursor"
                    );
                    let bytes = (0..value.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&value[i..i + 2], 16))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    query.after = Some(String::from_utf8(bytes)?);
                }
                _ => anyhow::bail!("unknown session page query"),
            }
        }
        Ok(query)
    }
}
fn session_cursor(id: &str) -> String {
    id.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) enum SessionStore {
    Memory(HashMap<String, SessionRecord>),
    Sqlite {
        conn: Connection,
        // Fields drop in declaration order: close SQLite before releasing ownership.
        _ownership: Option<DatabaseOwnership>,
    },
}

/// An already-acquired database lifetime lock. Never clone or expose the handle.
/// CLOEXEC does not prevent a concurrent fork from retaining its open description;
/// closing only this descriptor would leave the flock held until that child execs.
pub(super) struct DatabaseOwnership {
    file: std::fs::File,
}
impl DatabaseOwnership {
    pub(super) fn new(file: std::fs::File) -> Self {
        Self { file }
    }
}
impl Drop for DatabaseOwnership {
    fn drop(&mut self) {
        // Unlock this description explicitly; do not wait for inherited aliases.
        // A release failure cannot be reported from Drop; closing the owned handle
        // still follows and a subsequent opener continues to fail closed if busy.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
pub(super) fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}
fn touched_ms(record: &SessionRecord) -> i64 {
    now_ms().saturating_sub(
        i64::try_from(record.last_accessed.elapsed().as_millis()).unwrap_or(i64::MAX),
    )
}
fn decode(messages: &str, accessed: i64) -> Result<SessionRecord> {
    let age = u64::try_from(now_ms().saturating_sub(accessed)).unwrap_or_default();
    Ok(SessionRecord {
        messages: serde_json::from_str(messages).context("invalid stored session JSON")?,
        last_accessed: Instant::now()
            .checked_sub(Duration::from_millis(age))
            .unwrap_or_else(Instant::now),
    })
}
impl SessionStore {
    pub(super) fn memory() -> Self {
        Self::Memory(HashMap::new())
    }
    #[cfg(test)]
    pub(super) fn from_memory(map: HashMap<String, SessionRecord>) -> Self {
        Self::Memory(map)
    }
    pub(super) fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let ownership = if path == Path::new(":memory:") {
            None
        } else {
            let lock_path = path.with_extension("sqlite3.lock");
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(lock_path)?;
            fs2::FileExt::try_lock_exclusive(&file).context("another JiaClaw process owns this session database; use HTTP APIs while serve is running")?;
            let ownership = DatabaseOwnership::new(file);
            // Create private state before SQLite opens it (WAL inherits DB mode).
            let mut db_options = std::fs::OpenOptions::new();
            db_options
                .read(true)
                .write(true)
                .create(true)
                .truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                db_options.mode(0o600);
            }
            db_options.open(path)?;
            Some(ownership)
        };
        Self::open_with_ownership(path, ownership)
    }

    /// Existing standalone v11 only. No migration, creation, interruption, TTL
    /// maintenance or model/runtime initialization. The same lifetime lock as
    /// serve is held until the SQLite connection has closed.
    pub(super) fn open_http_maintenance(path: &Path, writable: bool) -> Result<Self> {
        fn regular_private(metadata: &std::fs::Metadata) -> Result<()> {
            anyhow::ensure!(metadata.is_file(), "maintenance requires a regular file");
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                anyhow::ensure!(metadata.nlink() == 1, "maintenance rejects linked files");
                anyhow::ensure!(
                    metadata.mode() & 0o077 == 0,
                    "maintenance requires private file permissions"
                );
            }
            Ok(())
        }
        regular_private(
            &std::fs::symlink_metadata(path).context("existing session database required")?,
        )?;
        let path = path.canonicalize()?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path.with_extension("sqlite3.lock"))?;
        regular_private(&file.metadata()?)?;
        fs2::FileExt::try_lock_exclusive(&file).context("another JiaClaw process owns this session database; stop serve before HTTP receipt maintenance")?;
        let ownership = DatabaseOwnership::new(file);
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&path)?;
        regular_private(&file.metadata()?)?;
        let mut header = [0u8; 16];
        file.read_exact(&mut header)?;
        anyhow::ensure!(
            &header == b"SQLite format 3\0",
            "maintenance requires an existing SQLite database"
        );
        // Close the header reader before SQLite can acquire process-scoped
        // locks on this inode; an unrelated close must not release those locks.
        drop(file);
        let flags = if writable {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        } | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn = Connection::open_with_flags(&path, flags)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(
            version == 11,
            "HTTP receipt maintenance requires schema 11, found {version}; no migration performed"
        );
        let private_channel: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name GLOB 'gateway_*_owner')",
            [],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            !private_channel,
            "HTTP receipt maintenance rejects gateway channel databases"
        );
        super::http_turn_store::validate_schema(&conn)?;
        let journal: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        anyhow::ensure!(
            journal == "wal",
            "HTTP receipt maintenance requires an existing WAL database"
        );
        conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
        Ok(Self::Sqlite {
            conn,
            _ownership: Some(ownership),
        })
    }

    /// Continue opening only after the caller has acquired the database lifetime lock.
    /// Gateway channel stores validate their private file identity under that same lock.
    pub(super) fn open_with_ownership(
        path: &Path,
        ownership: Option<DatabaseOwnership>,
    ) -> Result<Self> {
        let mut conn = Connection::open(path).context("open session SQLite database")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 11 {
            anyhow::bail!("unsupported session database version {version}; refusing downgrade");
        }
        if version < 1 {
            tx.execute_batch("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY NOT NULL, messages TEXT NOT NULL CHECK(json_valid(messages)), accessed_ms INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS sessions_accessed ON sessions(accessed_ms); CREATE TABLE IF NOT EXISTS migration_sources (path TEXT PRIMARY KEY NOT NULL); PRAGMA user_version=1;")?;
        }
        if version < 2 {
            tx.execute_batch(super::jobs::SCHEMA_V2)?;
        }
        if version < 3 {
            tx.execute_batch(super::channel_store::SCHEMA_V3)?;
        }
        if version < 4 {
            tx.execute_batch(super::channel_store::SCHEMA_V4)?;
        }
        if version < 5 {
            tx.execute_batch(super::channel_store::SCHEMA_V5)?;
        }
        if version < 6 {
            tx.execute_batch(super::channel_store::SCHEMA_V6)?;
        }
        if version < 7 {
            tx.execute_batch(super::channel_store::SCHEMA_V7)?;
        }
        if version < 8 {
            tx.execute_batch(super::jobs::SCHEMA_V8)?;
        }
        if version < 9 {
            tx.execute_batch(super::channel_store::SCHEMA_V9)?;
        }
        if version < 10 {
            tx.execute_batch(super::jobs::SCHEMA_V10)?;
        }
        if version < 11 {
            tx.execute_batch(super::http_turn_store::SCHEMA_V11)?;
        }
        super::http_turn_store::validate_schema(&tx)?;
        let violation = tx
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some();
        anyhow::ensure!(
            !violation,
            "session database foreign key integrity check failed"
        );
        tx.commit()?;
        Ok(Self::Sqlite {
            conn,
            _ownership: ownership,
        })
    }
    // Legacy JSON is imported transactionally once and preserved as a backup.
    // A malformed JSON file aborts startup instead of replacing history with empty data.
    pub(super) fn migrate_json(&mut self, source: &Path) -> Result<()> {
        let Self::Sqlite { conn, .. } = self else {
            return Ok(());
        };
        if !source.exists() {
            return Ok(());
        }
        let source_id = source.canonicalize()?.to_string_lossy().into_owned();
        let already: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM migration_sources WHERE path=?1)",
            [&source_id],
            |row| row.get(0),
        )?;
        if already {
            return Ok(());
        }
        let file = std::fs::File::open(source)?;
        let mut bytes = Vec::new();
        file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 64 * 1024 * 1024 {
            anyhow::bail!("legacy JSON session store exceeds 64 MiB");
        }
        let sessions: HashMap<String, Vec<ChatMessage>> = serde_json::from_slice(&bytes)
            .context("legacy sessions JSON is corrupt; original file retained")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (id, messages) in sessions {
            tx.execute(
                "INSERT OR IGNORE INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3)",
                params![id, serde_json::to_string(&messages)?, now_ms()],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO migration_sources(path) VALUES(?1)",
            [&source_id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        match self {
            Self::Memory(map) => Ok(map.get(id).cloned()),
            Self::Sqlite { conn, .. } => conn
                .query_row(
                    "SELECT messages,accessed_ms FROM sessions WHERE id=?1",
                    [id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?
                .map(|(json, accessed)| decode(&json, accessed))
                .transpose(),
        }
    }
    pub(super) fn insert(&mut self, id: String, rec: SessionRecord) -> Result<()> {
        match self {
            Self::Memory(map) => {
                map.insert(id, rec);
            }
            Self::Sqlite { conn, .. } => {
                super::http_turn_store::ensure_clear(conn, &id)?;
                conn.execute("INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET messages=excluded.messages,accessed_ms=excluded.accessed_ms", params![id, serde_json::to_string(&rec.messages)?, touched_ms(&rec)])?;
            }
        }
        Ok(())
    }
    pub(super) fn import(&mut self, id: &str, rec: SessionRecord, overwrite: bool) -> Result<bool> {
        match self {
            Self::Memory(map) => {
                if map.contains_key(id) && !overwrite {
                    return Ok(false);
                }
                map.insert(id.into(), rec);
                Ok(true)
            }
            Self::Sqlite { conn, .. } => {
                super::http_turn_store::ensure_clear(conn, id)?;
                let sql = if overwrite {
                    "INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET messages=excluded.messages,accessed_ms=excluded.accessed_ms"
                } else {
                    "INSERT OR IGNORE INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3)"
                };
                Ok(conn.execute(
                    sql,
                    params![id, serde_json::to_string(&rec.messages)?, touched_ms(&rec)],
                )? == 1)
            }
        }
    }
    pub(super) fn list_page(
        &self,
        query: &SessionPageQuery,
        hide_jobs: bool,
    ) -> Result<ListSessionsResponse> {
        let (sessions, has_more): (Vec<SessionSummary>, bool) = match self {
            Self::Memory(map) => {
                // Keep at most one page plus a lookahead, even for an unordered
                // ephemeral store. Borrow IDs; never clone message histories.
                let mut page = BTreeMap::new();
                for (id, rec) in map {
                    if query.after.as_ref().is_some_and(|after| id <= after)
                        || (hide_jobs && id.starts_with("job:"))
                    {
                        continue;
                    }
                    page.insert(id.as_str(), rec.messages.len());
                    if page.len() > query.limit + 1 {
                        page.pop_last();
                    }
                }
                let has_more = page.len() > query.limit;
                let sessions = page
                    .into_iter()
                    .take(query.limit)
                    .map(|(id, message_count)| {
                        anyhow::ensure!(
                            (1..=1024).contains(&id.len()),
                            "stored session ID exceeds catalog budget"
                        );
                        Ok(SessionSummary {
                            id: id.into(),
                            message_count,
                        })
                    })
                    .collect::<Result<_>>()?;
                (sessions, has_more)
            }
            Self::Sqlite { conn, .. } => {
                // Discover bounded IDs first. The lookahead never parses its
                // history. Count only the actual page, using one reused local
                // SQLite statement; no body is copied or decoded in Rust.
                let comparison = if query.after.is_some() { ">" } else { ">=" };
                let mut stmt = conn.prepare(&format!(
                    "SELECT CASE WHEN length(CAST(id AS BLOB)) BETWEEN 1 AND 1024 THEN id END FROM sessions WHERE id {comparison} ?1 AND (?2=0 OR substr(id,1,4)!='job:') ORDER BY id LIMIT ?3"
                ))?;
                let mut ids = stmt
                    .query_map(
                        params![
                            query.after.as_deref().unwrap_or(""),
                            hide_jobs,
                            query.limit + 1
                        ],
                        |row| row.get::<_, Option<String>>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let has_more = ids.len() > query.limit;
                ids.truncate(query.limit);
                let mut count = conn.prepare("SELECT CASE WHEN length(CAST(messages AS BLOB)) <= 33554432 AND json_type(messages)='array' THEN json_array_length(messages) END FROM sessions WHERE id=?1")?;
                let sessions = ids
                    .into_iter()
                    .map(|id| {
                        let id = id.context("stored session ID exceeds catalog budget")?;
                        let message_count = count
                            .query_row([&id], |row| row.get::<_, Option<usize>>(0))?
                            .context(
                                "stored session history is invalid or exceeds catalog budget",
                            )?;
                        Ok(SessionSummary { id, message_count })
                    })
                    .collect::<Result<_>>()?;
                (sessions, has_more)
            }
        };
        let next_cursor =
            has_more.then(|| session_cursor(&sessions.last().expect("nonempty page").id));
        Ok(ListSessionsResponse {
            sessions,
            limit: query.limit,
            has_more,
            next_cursor,
        })
    }
    pub(super) fn touch(&mut self, id: &str) -> Result<()> {
        match self {
            Self::Memory(map) => {
                if let Some(rec) = map.get_mut(id) {
                    rec.touch();
                }
            }
            Self::Sqlite { conn, .. } => {
                conn.execute(
                    "UPDATE sessions SET accessed_ms=?1 WHERE id=?2",
                    params![now_ms(), id],
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn remove(&mut self, id: &str) -> Result<bool> {
        match self {
            Self::Memory(map) => Ok(map.remove(id).is_some()),
            Self::Sqlite { conn, .. } => {
                super::http_turn_store::ensure_clear(conn, id)?;
                Ok(conn.execute("DELETE FROM sessions WHERE id=?1", [id])? == 1)
            }
        }
    }
    pub(super) fn len(&self) -> Result<usize> {
        match self {
            Self::Memory(map) => Ok(map.len()),
            Self::Sqlite { conn, .. } => {
                Ok(conn.query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))?)
            }
        }
    }
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.len().is_ok_and(|n| n == 0)
    }
    pub(super) fn purge(&mut self, ttl: Duration, active: &[String]) -> Result<usize> {
        match self {
            Self::Memory(map) => {
                let before = map.len();
                let now = Instant::now();
                map.retain(|id, rec| active.contains(id) || !rec.is_expired(ttl, now));
                Ok(before - map.len())
            }
            Self::Sqlite { conn, .. } => {
                let cutoff =
                    now_ms().saturating_sub(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                // Delete bounded batches in SQLite; a directory refresh must
                // not allocate every expired ID before producing its page.
                // Exempt real owners and unresolved effects before LIMIT so
                // protected rows cannot starve later expired sessions.
                let active_json = serde_json::to_string(active)?;
                let mut deleted = 0;
                loop {
                    let count = tx.execute(
                        "DELETE FROM sessions WHERE id IN (SELECT id FROM sessions WHERE accessed_ms<=?2 AND id NOT IN (SELECT value FROM json_each(?1)) AND NOT EXISTS (SELECT 1 FROM channel_events e WHERE e.session_id=sessions.id AND (e.status IN ('received','processing') OR (e.status='needs_review' AND e.reviewed_ms IS NULL) OR EXISTS (SELECT 1 FROM channel_outbox d WHERE d.event_id=e.id AND d.state NOT IN ('delivered','cancelled')))) AND NOT EXISTS (SELECT 1 FROM http_turns h WHERE h.session_id=sessions.id AND (h.state='running' OR (h.state='needs_review' AND h.reviewed_ms IS NULL))) AND NOT EXISTS (SELECT 1 FROM job_runs r JOIN channel_outbox d ON d.job_run_id=r.id WHERE r.session_id=sessions.id AND d.state NOT IN ('delivered','cancelled')) LIMIT 256)",
                        params![active_json, cutoff],
                    )?;
                    deleted += count;
                    if count == 0 {
                        break;
                    }
                }
                tx.commit()?;
                Ok(deleted)
            }
        }
    }
    pub(super) fn flush(&self) -> Result<()> {
        if let Self::Sqlite { conn, .. } = self {
            conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::MessageRole;

    #[test]
    fn sqlite_has_wal_reset_fix_and_checked_unsigned_conversions() {
        // WAL-reset can corrupt concurrent write/checkpoint connections before
        // SQLite 3.51.3. Test the linked engine and the explicit fallible_uint
        // feature, rather than relying only on a manifest version string.
        let conn = Connection::open_in_memory().unwrap();
        let version: String = conn
            .query_row("SELECT sqlite_version()", [], |row| row.get(0))
            .unwrap();
        assert!(rusqlite::version_number() >= 3_051_003, "SQLite {version}");
        assert!(matches!(
            conn.query_row("SELECT -1", [], |row| row.get::<_, usize>(0)),
            Err(rusqlite::Error::IntegralValueOutOfRange(_, -1))
        ));
        assert!(matches!(
            conn.query_row("SELECT ?1", [u64::MAX], |row| row.get::<_, i64>(0)),
            Err(rusqlite::Error::ToSqlConversionFailure(_))
        ));
        assert_eq!(
            conn.query_row("SELECT ?1", [42_usize], |row| row.get::<_, usize>(0))
                .unwrap(),
            42
        );
    }

    fn message(content: &str) -> SessionRecord {
        SessionRecord::new(vec![ChatMessage {
            role: MessageRole::User,
            content: content.into(),
        }])
    }
    #[test]
    fn committed_create_update_delete_survive_reopening() {
        let path =
            std::env::temp_dir().join(format!("jiaclaw-db-{}.sqlite3", uuid::Uuid::new_v4()));
        {
            let mut store = SessionStore::open(&path).unwrap();
            store
                .insert("empty".into(), SessionRecord::new(vec![]))
                .unwrap();
            store.insert("turn".into(), message("old")).unwrap();
            store.insert("turn".into(), message("new")).unwrap();
        }
        {
            let mut store = SessionStore::open(&path).unwrap();
            assert_eq!(store.len().unwrap(), 2);
            assert_eq!(
                store.get("turn").unwrap().unwrap().messages[0].content,
                "new"
            );
            assert!(store.remove("turn").unwrap());
        }
        assert!(SessionStore::open(&path)
            .unwrap()
            .get("turn")
            .unwrap()
            .is_none());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn migration_is_idempotent_and_never_resurrects_deleted_sessions() {
        let path =
            std::env::temp_dir().join(format!("jiaclaw-migrate-{}.sqlite3", uuid::Uuid::new_v4()));
        let json = path.with_extension("json");
        std::fs::write(&json, r#"{"legacy":[{"role":"user","content":"hello"}]}"#).unwrap();
        {
            let mut s = SessionStore::open(&path).unwrap();
            s.migrate_json(&json).unwrap();
            assert!(s.remove("legacy").unwrap());
        }
        let mut s = SessionStore::open(&path).unwrap();
        s.migrate_json(&json).unwrap();
        assert!(s.get("legacy").unwrap().is_none());
        drop(s);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&json).unwrap();
    }
    #[test]
    fn atomic_import_conflict_preserves_original_messages() {
        let mut s = SessionStore::open(Path::new(":memory:")).unwrap();
        assert!(s.import("s", message("original"), false).unwrap());
        assert!(!s.import("s", message("overwrite"), false).unwrap());
        assert_eq!(s.get("s").unwrap().unwrap().messages[0].content, "original");
    }

    #[test]
    fn exclusive_ownership_future_schema_and_corrupt_migration_fail_closed() {
        let dir = std::env::temp_dir().join(format!("jiaclaw-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("sessions.sqlite3");
        let mut store = SessionStore::open(&db).unwrap();
        assert!(SessionStore::open(&db).is_err());
        store.insert("keep".into(), message("retained")).unwrap();
        let legacy = dir.join("sessions.json");
        std::fs::write(&legacy, "{corrupt").unwrap();
        assert!(store.migrate_json(&legacy).is_err());
        assert_eq!(store.len().unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "{corrupt");
        drop(store);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA user_version=12;").unwrap();
        drop(conn);
        assert!(SessionStore::open(&db).is_err());
        let conn = Connection::open(&db).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            12
        );
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retained_ownership_description_does_not_outlive_store() {
        let dir =
            std::env::temp_dir().join(format!("jiaclaw-lock-retained-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("sessions.sqlite3");
        let mut store = SessionStore::open(&path).unwrap();
        store
            .insert("retained".into(), message("committed"))
            .unwrap();
        // dup and fork retain the same open file description, even with CLOEXEC.
        // No sleep or a guessed overlap with another test's subprocess is needed.
        let retained = match &store {
            SessionStore::Sqlite {
                _ownership: Some(guard),
                ..
            } => guard.file.try_clone().unwrap(),
            _ => panic!("expected database ownership"),
        };
        assert!(SessionStore::open(&path).is_err());
        drop(store);
        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(
            reopened.get("retained").unwrap().unwrap().messages[0].content,
            "committed"
        );
        assert!(SessionStore::open(&path).is_err());
        drop(retained);
        assert!(SessionStore::open(&path).is_err());
        drop(reopened);
        drop(SessionStore::open(&path).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_open_releases_ownership_with_retained_description_and_preserves_schema() {
        let dir =
            std::env::temp_dir().join(format!("jiaclaw-lock-failed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("sessions.sqlite3");
        drop(SessionStore::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version=12;").unwrap();
        drop(conn);
        let lock_path = path.with_extension("sqlite3.lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&file).unwrap();
        let retained = file.try_clone().unwrap();
        let owner = DatabaseOwnership::new(file);
        assert!(SessionStore::open_with_ownership(&path, Some(owner)).is_err());
        let next = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&next).unwrap();
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            12
        );
        drop(conn);
        drop(retained);
        fs2::FileExt::unlock(&next).unwrap();
        drop(next);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ttl_survives_reopening_and_exempts_active_turns() {
        let db = std::env::temp_dir().join(format!("jiaclaw-ttl-{}.sqlite3", uuid::Uuid::new_v4()));
        {
            let mut store = SessionStore::open(&db).unwrap();
            let rec = SessionRecord {
                messages: vec![],
                last_accessed: Instant::now() - Duration::from_secs(60),
            };
            store.insert("expired".into(), rec.clone()).unwrap();
            store.insert("active".into(), rec).unwrap();
            store
                .insert("fresh".into(), SessionRecord::new(vec![]))
                .unwrap();
        }
        let mut store = SessionStore::open(&db).unwrap();
        assert_eq!(
            store
                .purge(Duration::from_secs(30), &["active".into()])
                .unwrap(),
            1
        );
        assert!(store.get("expired").unwrap().is_none());
        assert!(store.get("active").unwrap().is_some());
        assert!(store.get("fresh").unwrap().is_some());
        drop(store);
        std::fs::remove_file(db).unwrap();
    }
}
