// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Permanent HTTP admission identities, distinct from model receipts and delivery.

use super::store::{now_ms, SessionStore};
use anyhow::{ensure, Result};
use jiaclaw_core::ChatMessage;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

pub(super) const MAX_IDENTITIES: usize = 10_000;
pub(super) const MAX_RESULTS: usize = 32;
pub(super) const MAX_RESULT_BYTES: usize = 2 * 1024 * 1024 + 4096;
pub(super) const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;

pub(super) const SCHEMA_V11: &str = "
CREATE TABLE http_turns (
 id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=36 AND substr(id,15,1)='4'),
 session_id TEXT NOT NULL CHECK(length(session_id)=41 AND substr(session_id,1,5)='http:'),
 request_hash TEXT NOT NULL CHECK(length(request_hash)=64 AND request_hash NOT GLOB '*[^0-9a-f]*'),
 context_hash TEXT NOT NULL CHECK(length(context_hash)=64 AND context_hash NOT GLOB '*[^0-9a-f]*'),
 created_ms INTEGER NOT NULL,
 finished_ms INTEGER,
 state TEXT NOT NULL CHECK(state IN ('running','completed','needs_review')),
 session_committed INTEGER NOT NULL DEFAULT 0 CHECK(session_committed IN (0,1)),
 error TEXT CHECK(error IS NULL OR length(error)<=128),
 result TEXT CHECK(result IS NULL OR (json_valid(result) AND length(CAST(result AS BLOB))<=2101248)),
 cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
 result_purged INTEGER NOT NULL DEFAULT 0 CHECK(result_purged IN (0,1)),
 reviewed_ms INTEGER,
 review_note TEXT CHECK(review_note IS NULL OR length(CAST(review_note AS BLOB))<=1024),
 CHECK((state='running' AND finished_ms IS NULL AND session_committed=0 AND result IS NULL AND result_purged=0 AND reviewed_ms IS NULL)
    OR (state<>'running' AND finished_ms IS NOT NULL)),
 CHECK(state<>'completed' OR (session_committed=1 AND error IS NULL)),
 CHECK(reviewed_ms IS NULL OR (state='needs_review' AND review_note IS NOT NULL)),
 CHECK(result_purged=0 OR result IS NULL)
);
CREATE UNIQUE INDEX http_turn_one_unresolved ON http_turns(session_id)
 WHERE state='running' OR (state='needs_review' AND reviewed_ms IS NULL);
PRAGMA user_version=11;
";

#[derive(Debug)]
pub(super) struct Conflict(pub &'static str);
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Conflict {}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Receipt {
    pub id: String,
    pub session_id: String,
    pub request_hash: String,
    pub context_hash: String,
    pub created_ms: i64,
    pub finished_ms: Option<i64>,
    pub state: String,
    pub session_committed: bool,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
    pub result_purged: bool,
    pub cancel_requested: bool,
    pub reviewed_ms: Option<i64>,
    pub review_note: Option<String>,
}

fn lookup(conn: &Connection, id: &str) -> Result<Option<Receipt>> {
    let row = conn.query_row("SELECT id,session_id,request_hash,context_hash,created_ms,finished_ms,state,session_committed,error,result,result_purged,reviewed_ms,review_note,cancel_requested FROM http_turns WHERE id=?1", [id], |row| {
        Ok((Receipt { id:row.get(0)?, session_id:row.get(1)?, request_hash:row.get(2)?, context_hash:row.get(3)?, created_ms:row.get(4)?, finished_ms:row.get(5)?, state:row.get(6)?, session_committed:row.get(7)?, error:row.get(8)?, result:None, result_purged:row.get(10)?, reviewed_ms:row.get(11)?, review_note:row.get(12)?, cancel_requested:row.get(13)? }, row.get::<_,Option<String>>(9)?))
    }).optional()?;
    row.map(|(mut receipt, result)| {
        receipt.result = result
            .map(|value| serde_json::from_str(&value))
            .transpose()?;
        Ok(receipt)
    })
    .transpose()
}

pub(super) fn ensure_clear(conn: &Connection, session: &str) -> Result<()> {
    let unresolved: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM http_turns WHERE session_id=?1 AND (state='running' OR (state='needs_review' AND reviewed_ms IS NULL)))", [session], |row| row.get(0))?;
    if unresolved {
        return Err(Conflict("http_turn_needs_review").into());
    }
    Ok(())
}

pub(super) fn validate_schema(conn: &Connection) -> Result<()> {
    fn shape(conn: &Connection) -> Result<Vec<(String, String, Option<String>)>> {
        let mut query=conn.prepare("SELECT type,name,sql FROM sqlite_schema WHERE tbl_name='http_turns' ORDER BY type,name")?;
        let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA_V11)?;
    ensure!(
        shape(conn)? == shape(&expected)?,
        "HTTP turn schema does not match its declared version"
    );
    Ok(())
}

impl SessionStore {
    fn http_connection(&self) -> Result<&Connection> {
        match self {
            Self::Sqlite { conn, .. } => Ok(conn),
            Self::Memory(_) => anyhow::bail!("tracked HTTP turns require SQLite"),
        }
    }

    pub(super) fn http_receipt(&self, id: &str, hash: Option<&str>) -> Result<Option<Receipt>> {
        let receipt = lookup(self.http_connection()?, id)?;
        if receipt
            .as_ref()
            .is_some_and(|r| hash.is_some_and(|hash| r.request_hash != hash))
        {
            return Err(Conflict("http_turn_identity_conflict").into());
        }
        Ok(receipt)
    }

    /// Called under the process database lifetime lock, before any workers start.
    pub(super) fn interrupt_http_turns(&mut self) -> Result<()> {
        if let Self::Sqlite { conn, .. } = self {
            conn.execute("UPDATE http_turns SET state='needs_review',finished_ms=?1,error='process_interrupted' WHERE state='running'", [now_ms()])?;
        }
        Ok(())
    }

    pub(super) fn admit_http_turn(
        &mut self,
        id: &str,
        session: &str,
        hash: &str,
        context: &str,
    ) -> Result<(Receipt, bool)> {
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("tracked HTTP turns require SQLite")
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = lookup(&tx, id)? {
            if receipt.request_hash != hash {
                return Err(Conflict("http_turn_identity_conflict").into());
            }
            return Ok((receipt, false));
        }
        ensure_clear(&tx, session)?;
        let (identities, results): (usize, usize) = tx.query_row(
            "SELECT count(*),coalesce(sum(result_purged=0),0) FROM http_turns",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if identities >= MAX_IDENTITIES {
            return Err(Conflict("http_turn_identity_capacity").into());
        }
        if results >= MAX_RESULTS {
            return Err(Conflict("http_turn_result_capacity").into());
        }
        tx.execute("INSERT INTO http_turns(id,session_id,request_hash,context_hash,created_ms,state) VALUES(?1,?2,?3,?4,?5,'running')", params![id,session,hash,context,now_ms()])?;
        let receipt = lookup(&tx, id)?.expect("inserted receipt");
        tx.commit()?;
        Ok((receipt, true))
    }

    /// Terminal receipt and history share one FULL/WAL transaction. No model replay.
    pub(super) fn finish_http_turn(
        &mut self,
        id: &str,
        messages: Option<Vec<ChatMessage>>,
        result: Option<serde_json::Value>,
        error: Option<&str>,
        needs_review: bool,
    ) -> Result<Receipt> {
        let result = result.map(|v| serde_json::to_string(&v)).transpose()?;
        ensure!(
            result.as_ref().is_none_or(|v| v.len() <= MAX_RESULT_BYTES),
            "HTTP result exceeds reserved budget"
        );
        let history = messages.map(|m| serde_json::to_string(&m)).transpose()?;
        ensure!(
            history
                .as_ref()
                .is_none_or(|v| v.len() <= MAX_HISTORY_BYTES),
            "HTTP history exceeds reserved budget"
        );
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("tracked HTTP turns require SQLite")
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = lookup(&tx, id)?.ok_or(Conflict("http_turn_not_found"))?;
        if receipt.state != "running" {
            return Err(Conflict("http_turn_already_finished").into());
        }
        ensure!(
            needs_review || (history.is_some() && result.is_some() && error.is_none()),
            "completed HTTP turn requires committed result"
        );
        let now = now_ms();
        if let Some(history) = &history {
            tx.execute("INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET messages=excluded.messages,accessed_ms=excluded.accessed_ms",params![receipt.session_id,history,now])?;
        }
        tx.execute("UPDATE http_turns SET state=?2,finished_ms=?3,session_committed=?4,result=?5,error=?6 WHERE id=?1",params![id, if needs_review {"needs_review"} else {"completed"}, now,history.is_some(),result,error])?;
        let receipt = lookup(&tx, id)?.expect("retained receipt");
        tx.commit()?;
        Ok(receipt)
    }

    pub(super) fn request_http_cancel(&mut self, id: &str) -> Result<Receipt> {
        let conn = self.http_connection()?;
        conn.execute(
            "UPDATE http_turns SET cancel_requested=1 WHERE id=?1 AND state='running'",
            [id],
        )?;
        lookup(conn, id)?.ok_or_else(|| Conflict("http_turn_not_found").into())
    }

    pub(super) fn review_http_turn(&mut self, id: &str, note: &str) -> Result<Receipt> {
        let Self::Sqlite { conn, .. } = self else {
            anyhow::bail!("tracked HTTP turns require SQLite")
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = lookup(&tx, id)?.ok_or(Conflict("http_turn_not_found"))?;
        if receipt.state == "completed" {
            return Err(Conflict("http_turn_review_not_required").into());
        }
        if receipt.reviewed_ms.is_some() && receipt.review_note.as_deref() != Some(note) {
            return Err(Conflict("http_turn_review_already_recorded").into());
        }
        if receipt.reviewed_ms.is_none() {
            // The caller owns the session turn and has checked no supervisor is active.
            // An orphaned running row may remain after a terminal storage failure.
            tx.execute("UPDATE http_turns SET state='needs_review',finished_ms=coalesce(finished_ms,?2),error=coalesce(error,'owner_interrupted'),reviewed_ms=?2,review_note=?3 WHERE id=?1",params![id,now_ms(),note])?;
        }
        let receipt = lookup(&tx, id)?.expect("retained receipt");
        tx.commit()?;
        Ok(receipt)
    }

    pub(super) fn purge_http_result(&mut self, id: &str) -> Result<Receipt> {
        let conn = self.http_connection()?;
        let receipt = lookup(conn, id)?.ok_or(Conflict("http_turn_not_found"))?;
        if receipt.state == "running" {
            return Err(Conflict("http_turn_active").into());
        }
        conn.execute(
            "UPDATE http_turns SET result=NULL,result_purged=1 WHERE id=?1",
            [id],
        )?;
        lookup(conn, id)?.ok_or_else(|| Conflict("http_turn_not_found").into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::SessionRecord;
    use super::*;
    use jiaclaw_core::MessageRole;
    use std::path::Path;

    fn store() -> SessionStore {
        SessionStore::open(Path::new(":memory:")).unwrap()
    }
    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn session() -> String {
        format!("http:{}", new_id())
    }
    fn history(text: &str) -> Vec<ChatMessage> {
        vec![ChatMessage {
            role: MessageRole::Assistant,
            content: text.into(),
        }]
    }
    fn admit(store: &mut SessionStore, id: &str, session: &str) {
        assert!(
            store
                .admit_http_turn(id, session, &"a".repeat(64), &"b".repeat(64))
                .unwrap()
                .1
        );
    }

    #[test]
    fn duplicate_and_purged_results_keep_the_original_admission_identity() {
        let mut store = store();
        let id = new_id();
        let sid = session();
        admit(&mut store, &id, &sid);
        let (original, created) = store
            .admit_http_turn(&id, &sid, &"a".repeat(64), &"c".repeat(64))
            .unwrap();
        assert!(!created);
        assert_eq!(original.context_hash, "b".repeat(64));
        assert!(store.http_receipt(&id, Some(&"c".repeat(64))).is_err());
        let receipt = store
            .finish_http_turn(
                &id,
                Some(history("committed")),
                Some(serde_json::json!({"reply":"committed"})),
                None,
                false,
            )
            .unwrap();
        assert!(receipt.session_committed);
        assert_eq!(
            store.get(&sid).unwrap().unwrap().messages[0].content,
            "committed"
        );
        let purged = store.purge_http_result(&id).unwrap();
        assert!(purged.result_purged && purged.result.is_none());
        assert!(
            !store
                .admit_http_turn(&id, &sid, &"a".repeat(64), &"c".repeat(64))
                .unwrap()
                .1
        );
        assert!(store
            .finish_http_turn(&id, Some(history("replay")), None, None, false)
            .is_err());
        assert_eq!(
            store.get(&sid).unwrap().unwrap().messages[0].content,
            "committed"
        );
    }

    #[test]
    fn a_rejected_terminal_transaction_does_not_partially_replace_history() {
        let mut store = store();
        let id = new_id();
        let sid = session();
        store
            .insert(sid.clone(), SessionRecord::new(history("original")))
            .unwrap();
        admit(&mut store, &id, &sid);
        store.http_connection().unwrap().execute_batch("CREATE TRIGGER reject_http_finish BEFORE UPDATE OF state ON http_turns BEGIN SELECT RAISE(ABORT,'fixture storage fault'); END").unwrap();
        assert!(store
            .finish_http_turn(
                &id,
                Some(history("uncommitted")),
                Some(serde_json::json!({"reply":"uncommitted"})),
                None,
                false
            )
            .is_err());
        assert_eq!(
            store.get(&sid).unwrap().unwrap().messages[0].content,
            "original"
        );
        let receipt = store.http_receipt(&id, None).unwrap().unwrap();
        assert_eq!(receipt.state, "running");
        assert!(receipt.result.is_none() && !receipt.session_committed);
        assert!(store
            .insert(sid.clone(), SessionRecord::new(history("bypass")))
            .is_err());
        assert!(store
            .import(&sid, SessionRecord::new(history("bypass")), true)
            .is_err());
        assert!(store.remove(&sid).is_err());
    }

    #[test]
    fn restart_requires_explicit_review_and_ttl_never_discards_an_unresolved_session() {
        let mut store = store();
        let id = new_id();
        let sid = session();
        store
            .insert(sid.clone(), SessionRecord::new(history("before")))
            .unwrap();
        admit(&mut store, &id, &sid);
        store.interrupt_http_turns().unwrap();
        let interrupted = store.http_receipt(&id, None).unwrap().unwrap();
        assert_eq!(interrupted.state, "needs_review");
        assert_eq!(interrupted.error.as_deref(), Some("process_interrupted"));
        assert_eq!(store.purge(std::time::Duration::ZERO, &[]).unwrap(), 0);
        assert!(store
            .admit_http_turn(&new_id(), &sid, &"d".repeat(64), &"b".repeat(64))
            .is_err());
        let receipt = store
            .review_http_turn(
                &id,
                "model receipts and workspace reviewed; abandon incomplete work",
            )
            .unwrap();
        assert!(receipt.reviewed_ms.is_some());
        assert!(!receipt.session_committed);
        assert!(
            !store
                .admit_http_turn(&id, &sid, &"a".repeat(64), &"b".repeat(64))
                .unwrap()
                .1
        );
        assert_eq!(store.purge(std::time::Duration::ZERO, &[]).unwrap(), 1);
        assert!(store.http_receipt(&id, None).unwrap().is_some());
        assert!(
            store
                .admit_http_turn(&new_id(), &sid, &"d".repeat(64), &"b".repeat(64))
                .unwrap()
                .1
        );
    }

    #[test]
    fn reserved_result_capacity_is_reclaimable_but_permanent_identities_are_not() {
        let mut store = store();
        let ids: Vec<_> = (0..MAX_RESULTS).map(|_| new_id()).collect();
        for id in &ids {
            admit(&mut store, id, &session());
        }
        assert!(store
            .admit_http_turn(&new_id(), &session(), &"a".repeat(64), &"b".repeat(64))
            .unwrap_err()
            .to_string()
            .contains("result_capacity"));
        store
            .finish_http_turn(&ids[0], None, None, Some("fixture_failure"), true)
            .unwrap();
        store.purge_http_result(&ids[0]).unwrap();
        admit(&mut store, &new_id(), &session());
        let conn = store.http_connection().unwrap();
        conn.execute("UPDATE http_turns SET state='needs_review',finished_ms=1,error='fixture',result_purged=1 WHERE state='running'",[]).unwrap();
        let count: usize = conn
            .query_row("SELECT count(*) FROM http_turns", [], |r| r.get(0))
            .unwrap();
        for i in count..MAX_IDENTITIES {
            conn.execute("INSERT INTO http_turns(id,session_id,request_hash,context_hash,created_ms,state,finished_ms,result_purged,reviewed_ms,review_note) VALUES(?1,?2,?3,?3,1,'needs_review',1,1,1,'reviewed fixture')",params![format!("00000000-0000-4000-8000-{i:012}"),session(),"a".repeat(64)]).unwrap();
        }
        assert!(store
            .admit_http_turn(&new_id(), &session(), &"a".repeat(64), &"b".repeat(64))
            .unwrap_err()
            .to_string()
            .contains("identity_capacity"));
        assert!(
            !store
                .admit_http_turn(&ids[0], "irrelevant", &"a".repeat(64), &"b".repeat(64))
                .unwrap()
                .1
        );
    }

    #[test]
    fn output_budgets_preserve_admission_and_unknown_schema_is_rejected() {
        let mut store = store();
        let id = new_id();
        let sid = session();
        admit(&mut store, &id, &sid);
        assert!(store
            .finish_http_turn(
                &id,
                Some(history("x")),
                Some(serde_json::json!({"reply":"x".repeat(MAX_RESULT_BYTES)})),
                None,
                false
            )
            .is_err());
        assert!(store
            .finish_http_turn(
                &id,
                Some(history(&"x".repeat(MAX_HISTORY_BYTES))),
                None,
                Some("too_big"),
                true
            )
            .is_err());
        assert_eq!(
            store.http_receipt(&id, None).unwrap().unwrap().state,
            "running"
        );
        assert!(store.get(&sid).unwrap().is_none());
        let conn = store.http_connection().unwrap();
        validate_schema(conn).unwrap();
        conn.execute_batch("DROP INDEX http_turn_one_unresolved")
            .unwrap();
        assert!(validate_schema(conn).is_err());
    }

    #[test]
    fn schema_ten_migrates_existing_history_atomically_and_future_versions_fail_closed() {
        let file =
            std::env::temp_dir().join(format!("jiaclaw-http-turn-migration-{}.sqlite3", new_id()));
        let sid = session();
        let mut store = SessionStore::open(&file).unwrap();
        store
            .insert(sid.clone(), SessionRecord::new(history("schema-ten")))
            .unwrap();
        store
            .http_connection()
            .unwrap()
            .execute_batch("DROP TABLE http_turns; PRAGMA user_version=10")
            .unwrap();
        drop(store);
        let store = SessionStore::open(&file).unwrap();
        assert_eq!(
            store.get(&sid).unwrap().unwrap().messages[0].content,
            "schema-ten"
        );
        validate_schema(store.http_connection().unwrap()).unwrap();
        store
            .http_connection()
            .unwrap()
            .execute_batch("PRAGMA user_version=12")
            .unwrap();
        drop(store);
        let Err(error) = SessionStore::open(&file) else {
            panic!("future schema accepted")
        };
        assert!(error.to_string().contains("unsupported"));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(file.with_extension("sqlite3.lock"));
    }
}
