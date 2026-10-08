// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Authoritative `SQLite` session storage and an explicit ephemeral backend.
use super::SessionRecord;
use anyhow::{Context, Result};
use jiaclaw_core::ChatMessage;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    collections::HashMap,
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

pub(super) enum SessionStore {
    Memory(HashMap<String, SessionRecord>),
    Sqlite {
        conn: Connection,
        _ownership: Option<std::fs::File>,
    },
}
fn now_ms() -> i64 {
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
            Some(file)
        };
        Self::open_with_ownership(path, ownership)
    }

    /// Continue opening only after the caller has acquired the database lifetime lock.
    /// Gateway channel stores validate their private file identity under that same lock.
    pub(super) fn open_with_ownership(
        path: &Path,
        ownership: Option<std::fs::File>,
    ) -> Result<Self> {
        let mut conn = Connection::open(path).context("open session SQLite database")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 10 {
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
    pub(super) fn list(&self) -> Result<Vec<(String, SessionRecord)>> {
        match self {
            Self::Memory(map) => Ok(map
                .iter()
                .map(|(id, rec)| (id.clone(), rec.clone()))
                .collect()),
            Self::Sqlite { conn, .. } => {
                let mut stmt =
                    conn.prepare("SELECT id,messages,accessed_ms FROM sessions ORDER BY id")?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?;
                rows.map(|row| {
                    let (id, json, touched) = row?;
                    Ok((id, decode(&json, touched)?))
                })
                .collect()
            }
        }
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
                // Indexed discovery; active turns are exempt until their commit.
                let expired = {
                    let mut stmt = tx.prepare("SELECT id FROM sessions WHERE accessed_ms<=?1")?;
                    let rows = stmt.query_map([cutoff], |row| row.get::<_, String>(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };
                let mut deleted = 0;
                for id in expired {
                    if !active.contains(&id) {
                        deleted += tx.execute(
                            "DELETE FROM sessions WHERE id=?1 AND accessed_ms<=?2 AND NOT EXISTS (SELECT 1 FROM channel_events e WHERE e.session_id=sessions.id AND (e.status IN ('received','processing') OR (e.status='needs_review' AND e.reviewed_ms IS NULL) OR EXISTS (SELECT 1 FROM channel_outbox d WHERE d.event_id=e.id AND d.state NOT IN ('delivered','cancelled')))) AND NOT EXISTS (SELECT 1 FROM job_runs r JOIN channel_outbox d ON d.job_run_id=r.id WHERE r.session_id=sessions.id AND d.state NOT IN ('delivered','cancelled'))",
                            params![id, cutoff],
                        )?;
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
        conn.execute_batch("PRAGMA user_version=11;").unwrap();
        drop(conn);
        assert!(SessionStore::open(&db).is_err());
        let conn = Connection::open(&db).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            11
        );
        drop(conn);
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
