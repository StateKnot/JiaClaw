// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Persisted schedules and conservative execution accounting. Claiming a run is
//! not a guarantee that an external tool effect occurs exactly once.

use super::{schedule::ScheduleSpec, store::SessionStore, SessionRecord};
use anyhow::{bail, Context, Result};
use jiaclaw_core::{ChatResponse, RunStatus};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub(super) const MAX_JOBS: usize = 100;
pub(super) const MAX_RUNS: usize = 10_000;
pub(super) const MAX_RUNS_PER_JOB: usize = 100;
const MAX_CONCURRENT_RUNS: usize = 4;
const MAX_LATENESS_MS: i64 = 5_000;
const MAX_ERROR_BYTES: usize = 4_096;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub(super) struct JobConflict(pub &'static str);

impl std::fmt::Display for JobConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for JobConflict {}

pub(super) const SCHEMA_V2: &str = "
CREATE TABLE jobs (
    id TEXT PRIMARY KEY NOT NULL,
    spec TEXT NOT NULL CHECK(json_valid(spec)),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
    next_due_ms INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    session_id TEXT NOT NULL UNIQUE,
    CHECK(deleted=0 OR enabled=0)
);
CREATE INDEX jobs_due ON jobs(enabled,deleted,next_due_ms);
CREATE TABLE job_runs (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
    scheduled_for_ms INTEGER NOT NULL,
    started_ms INTEGER NOT NULL,
    finished_ms INTEGER,
    status TEXT NOT NULL CHECK(status IN ('running','completed','failed','needs_review','interrupted','skipped')),
    spec TEXT NOT NULL CHECK(json_valid(spec)),
    session_id TEXT NOT NULL,
    response TEXT CHECK(response IS NULL OR json_valid(response)),
    error TEXT,
    UNIQUE(job_id,scheduled_for_ms),
    CHECK((status='running' AND finished_ms IS NULL) OR (status<>'running' AND finished_ms IS NOT NULL))
);
CREATE UNIQUE INDEX job_one_running ON job_runs(job_id) WHERE status='running';
CREATE INDEX job_runs_history ON job_runs(job_id,scheduled_for_ms DESC,id);
PRAGMA user_version=2;
";

fn default_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JobSpec {
    pub name: String,
    pub prompt: String,
    pub schedule: ScheduleSpec,
    pub enabled_tools: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl JobSpec {
    pub(super) fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.len() > 128 {
            bail!("job name must contain 1..128 bytes");
        }
        if self.prompt.trim().is_empty() || self.prompt.len() > 32 * 1024 {
            bail!("job prompt must contain 1..32768 bytes");
        }
        if !(1..=600).contains(&self.timeout_secs) {
            bail!("job timeout_secs must be within 1..600");
        }
        if !(1..=32).contains(&self.enabled_tools.len()) {
            bail!("job enabled_tools must explicitly name 1..32 tools");
        }
        let mut seen = HashSet::new();
        for name in &self.enabled_tools {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                bail!("job tool names must be 1..64 ASCII letters, digits, underscores or hyphens");
            }
            if !seen.insert(name) {
                bail!("job enabled_tools must not contain duplicates");
            }
        }
        self.schedule.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Job {
    pub id: String,
    pub spec: JobSpec,
    pub enabled: bool,
    pub deleted: bool,
    pub next_due_ms: i64,
    pub created_ms: i64,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct JobRun {
    pub id: String,
    pub job_id: String,
    pub scheduled_for_ms: i64,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub status: String,
    pub spec: JobSpec,
    pub session_id: String,
    pub response: Option<ChatResponse>,
    pub error: Option<String>,
}

fn from_json<T: serde::de::DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn job_from_row(row: &Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        spec: from_json(row, 1)?,
        enabled: row.get(2)?,
        deleted: row.get(3)?,
        next_due_ms: row.get(4)?,
        created_ms: row.get(5)?,
        session_id: row.get(6)?,
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<JobRun> {
    let response: Option<String> = row.get(8)?;
    Ok(JobRun {
        id: row.get(0)?,
        job_id: row.get(1)?,
        scheduled_for_ms: row.get(2)?,
        started_ms: row.get(3)?,
        finished_ms: row.get(4)?,
        status: row.get(5)?,
        spec: from_json(row, 6)?,
        session_id: row.get(7)?,
        response: response
            .map(|text| {
                serde_json::from_str(&text).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
        error: row.get(9)?,
    })
}

const JOB_COLUMNS: &str = "id,spec,enabled,deleted,next_due_ms,created_ms,session_id";
const RUN_COLUMNS: &str =
    "id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id,response,error";

fn lookup_job(conn: &Connection, id: &str) -> Result<Option<Job>> {
    Ok(conn
        .query_row(
            &format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id=?1"),
            [id],
            job_from_row,
        )
        .optional()?)
}

fn pagination(limit: usize, offset: usize) -> Result<(i64, i64)> {
    if !(1..=100).contains(&limit) {
        bail!("page limit must be within 1..100");
    }
    Ok((i64::try_from(limit)?, i64::try_from(offset)?))
}

fn next_occurrence(conn: &Connection, job: &Job, now: i64) -> Result<i64> {
    // A resumed job must not revisit previously dispatched UTC occurrences even
    // if the wall clock moved backwards. Normal claims already advance next_due.
    let last: Option<i64> = conn.query_row(
        "SELECT MAX(scheduled_for_ms) FROM job_runs WHERE job_id=?1",
        [&job.id],
        |row| row.get(0),
    )?;
    let after = now.max(last.unwrap_or(now));
    let next = job.spec.schedule.next_after(after)?;
    if next <= after {
        bail!("schedule did not advance to a future occurrence");
    }
    Ok(next)
}

fn trim_history(conn: &Connection, job_id: &str, keep: usize) -> Result<usize> {
    let count: usize = conn.query_row(
        "SELECT count(*) FROM job_runs WHERE job_id=?1",
        [job_id],
        |row| row.get(0),
    )?;
    let excess = count.saturating_sub(keep);
    let removed = conn.execute(
        "DELETE FROM job_runs WHERE id IN (SELECT id FROM job_runs WHERE job_id=?1 AND status IN ('completed','failed','skipped') ORDER BY scheduled_for_ms,id LIMIT ?2)",
        params![job_id, i64::try_from(excess)?],
    )?;
    Ok(count - removed)
}

fn bounded_error(mut error: String) -> String {
    if error.len() > MAX_ERROR_BYTES {
        let suffix = " [truncated]";
        let mut end = MAX_ERROR_BYTES - suffix.len();
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
        error.push_str(suffix);
    }
    error
}

impl SessionStore {
    fn job_conn(&self) -> Result<&Connection> {
        match self {
            Self::Sqlite { conn, .. } => Ok(conn),
            Self::Memory(_) => bail!("scheduler requires SQLite persistence"),
        }
    }

    fn job_conn_mut(&mut self) -> Result<&mut Connection> {
        match self {
            Self::Sqlite { conn, .. } => Ok(conn),
            Self::Memory(_) => bail!("scheduler requires SQLite persistence"),
        }
    }

    pub(super) fn create_job(&mut self, spec: JobSpec, now: i64) -> Result<Job> {
        spec.validate()?;
        let next_due_ms = spec.schedule.next_after(now)?;
        if next_due_ms <= now {
            bail!("schedule did not advance to a future occurrence");
        }
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: usize = tx.query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))?;
        if count >= MAX_JOBS {
            return Err(JobConflict("job quota reached (100 including deleted jobs); explicitly purge deleted jobs to free capacity").into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let job = Job {
            session_id: format!("job:{id}"),
            id,
            spec,
            enabled: true,
            deleted: false,
            next_due_ms,
            created_ms: now,
        };
        tx.execute(
            "INSERT INTO jobs(id,spec,enabled,deleted,next_due_ms,created_ms,session_id) VALUES(?1,?2,1,0,?3,?4,?5)",
            params![job.id, serde_json::to_string(&job.spec)?, job.next_due_ms, job.created_ms, job.session_id],
        )?;
        tx.commit()?;
        Ok(job)
    }

    pub(super) fn list_jobs(&self, limit: usize, offset: usize) -> Result<Vec<Job>> {
        self.list_jobs_including_deleted(limit, offset, false)
    }

    pub(super) fn list_jobs_including_deleted(
        &self,
        limit: usize,
        offset: usize,
        include_deleted: bool,
    ) -> Result<Vec<Job>> {
        let (limit, offset) = pagination(limit, offset)?;
        let mut stmt = self.job_conn()?.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE deleted=0 OR ?3 ORDER BY created_ms,id LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = stmt.query_map(params![limit, offset, include_deleted], job_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub(super) fn get_job(&self, id: &str) -> Result<Option<Job>> {
        lookup_job(self.job_conn()?, id)
    }

    pub(super) fn set_job_enabled(
        &mut self,
        id: &str,
        enabled: bool,
        now: i64,
    ) -> Result<Option<Job>> {
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut job) = lookup_job(&tx, id)? else {
            return Ok(None);
        };
        if job.deleted {
            return Err(JobConflict(
                "cannot change a deleted job; create a new job after reviewing retained runs",
            )
            .into());
        }
        if enabled {
            let running: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM job_runs WHERE job_id=?1 AND status='running')",
                [id],
                |row| row.get(0),
            )?;
            if running {
                return Err(JobConflict(
                    "cannot resume a job while its previous run is still running",
                )
                .into());
            }
            job.spec.validate()?;
            job.next_due_ms = next_occurrence(&tx, &job, now)?;
        }
        job.enabled = enabled;
        tx.execute(
            "UPDATE jobs SET enabled=?2,next_due_ms=?3 WHERE id=?1",
            params![id, enabled, job.next_due_ms],
        )?;
        tx.commit()?;
        Ok(Some(job))
    }

    pub(super) fn delete_job(&mut self, id: &str) -> Result<bool> {
        Ok(self.job_conn_mut()?.execute(
            "UPDATE jobs SET enabled=0,deleted=1 WHERE id=?1 AND deleted=0",
            [id],
        )? == 1)
    }

    /// Explicit destructive cleanup, separate from ordinary deletion.
    pub(super) fn purge_job(&mut self, id: &str) -> Result<bool> {
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(job) = lookup_job(&tx, id)? else {
            return Ok(false);
        };
        if !job.deleted {
            return Err(JobConflict("job must be soft-deleted before explicit purge").into());
        }
        let running: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM job_runs WHERE job_id=?1 AND status='running')",
            [id],
            |row| row.get(0),
        )?;
        if running {
            return Err(JobConflict("cannot purge a job with a running execution").into());
        }
        tx.execute("DELETE FROM job_runs WHERE job_id=?1", [id])?;
        tx.execute("DELETE FROM jobs WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn list_job_runs(
        &self,
        id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<JobRun>> {
        let (limit, offset) = pagination(limit, offset)?;
        let mut stmt = self.job_conn()?.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM job_runs WHERE job_id=?1 ORDER BY scheduled_for_ms DESC,id LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(params![id, limit, offset], run_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub(super) fn claim_due_jobs(&mut self, now: i64, capacity: usize) -> Result<Vec<JobRun>> {
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let running: usize = tx.query_row(
            "SELECT count(*) FROM job_runs WHERE status='running'",
            [],
            |row| row.get(0),
        )?;
        let capacity = capacity.min(MAX_CONCURRENT_RUNS.saturating_sub(running));
        let jobs = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {JOB_COLUMNS} FROM jobs WHERE enabled=1 AND deleted=0 AND next_due_ms<=?1 AND NOT EXISTS(SELECT 1 FROM job_runs WHERE job_runs.job_id=jobs.id AND status='running') ORDER BY next_due_ms,id LIMIT 100"
            ))?;
            let rows = stmt.query_map([now], job_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut claimed = Vec::new();
        for job in jobs {
            let next = next_occurrence(&tx, &job, now)?;
            if now.saturating_sub(job.next_due_ms) > MAX_LATENESS_MS {
                tx.execute(
                    "UPDATE jobs SET next_due_ms=?2 WHERE id=?1",
                    params![job.id, next],
                )?;
                tracing::info!(job_id = %job.id, scheduled_for_ms = job.next_due_ms, "scheduler skipped missed occurrence");
                continue;
            }
            if claimed.len() >= capacity {
                continue;
            }
            job.spec.validate()?;
            let remaining = trim_history(&tx, &job.id, MAX_RUNS_PER_JOB - 1)?;
            let total: usize =
                tx.query_row("SELECT count(*) FROM job_runs", [], |row| row.get(0))?;
            if remaining >= MAX_RUNS_PER_JOB || total >= MAX_RUNS {
                tx.execute("UPDATE jobs SET enabled=0 WHERE id=?1", [&job.id])?;
                tracing::warn!(job_id = %job.id, "scheduler paused job: retained audit records reached quota; explicit review and purge required");
                continue;
            }
            let run = JobRun {
                id: uuid::Uuid::new_v4().to_string(),
                job_id: job.id.clone(),
                scheduled_for_ms: job.next_due_ms,
                started_ms: now,
                finished_ms: None,
                status: "running".into(),
                spec: job.spec,
                session_id: job.session_id,
                response: None,
                error: None,
            };
            let inserted = tx.execute(
                "INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id) VALUES(?1,?2,?3,?4,NULL,'running',?5,?6) ON CONFLICT(job_id,scheduled_for_ms) DO NOTHING",
                params![run.id, run.job_id, run.scheduled_for_ms, now, serde_json::to_string(&run.spec)?, run.session_id],
            )?;
            tx.execute(
                "UPDATE jobs SET next_due_ms=?2 WHERE id=?1",
                params![job.id, next],
            )?;
            if inserted == 1 {
                claimed.push(run);
            }
        }
        tx.commit()?;
        Ok(claimed)
    }

    /// Commit the session and run outcome together. Never call this before the
    /// execution's side effects have been accounted for by the caller.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_job_run(
        &mut self,
        run_id: &str,
        session: Option<(String, SessionRecord)>,
        status: &str,
        response: Option<ChatResponse>,
        error: Option<String>,
        now: i64,
    ) -> Result<bool> {
        if !matches!(
            status,
            "completed" | "failed" | "needs_review" | "interrupted" | "skipped"
        ) {
            bail!("invalid terminal job run status");
        }
        if status == "completed"
            && !response
                .as_ref()
                .is_some_and(|r| r.status == RunStatus::Completed)
        {
            bail!("completed job run requires a completed agent response");
        }
        let serialized_response = response.map(|r| serde_json::to_string(&r)).transpose()?;
        if serialized_response
            .as_ref()
            .is_some_and(|text| text.len() > MAX_RESPONSE_BYTES)
        {
            bail!("job response exceeds 1 MiB storage limit; retain a bounded needs_review outcome instead");
        }
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                "SELECT job_id,session_id,started_ms FROM job_runs WHERE id=?1 AND status='running'",
                [run_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
            )
            .optional()?;
        let Some((job_id, session_id, started)) = current else {
            return Ok(false);
        };
        if let Some((id, record)) = session {
            if id != session_id {
                bail!("job run may commit only its own session");
            }
            tx.execute(
                "INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET messages=excluded.messages,accessed_ms=excluded.accessed_ms",
                params![id, serde_json::to_string(&record.messages)?, now],
            )?;
        }
        tx.execute(
            "UPDATE job_runs SET status=?2,response=?3,error=?4,finished_ms=?5 WHERE id=?1 AND status='running'",
            params![run_id, status, serialized_response, error.map(bounded_error), now.max(started)],
        )?;
        if matches!(status, "failed" | "needs_review" | "interrupted") {
            tx.execute("UPDATE jobs SET enabled=0 WHERE id=?1", [&job_id])?;
        }
        trim_history(&tx, &job_id, MAX_RUNS_PER_JOB)?;
        tx.commit()?;
        Ok(true)
    }

    /// Called once after acquiring exclusive DB ownership, before dispatching
    /// any scheduled work. Unknown effects are retained and never replayed.
    pub(super) fn recover_jobs(&mut self, now: i64) -> Result<usize> {
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE jobs SET enabled=0 WHERE id IN (SELECT job_id FROM job_runs WHERE status='running')",
            [],
        )?;
        let recovered = tx.execute(
            "UPDATE job_runs SET status='interrupted',finished_ms=MAX(?1,started_ms),error='process stopped before a terminal outcome was committed; effects may have occurred; review before resuming' WHERE status='running'",
            [now],
        )?;
        let jobs = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {JOB_COLUMNS} FROM jobs WHERE enabled=1 AND deleted=0 AND next_due_ms<=?1"
            ))?;
            let rows = stmt.query_map([now], job_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for job in jobs {
            let next = next_occurrence(&tx, &job, now)
                .with_context(|| format!("invalid persisted schedule for job {}", job.id))?;
            tx.execute(
                "UPDATE jobs SET next_due_ms=?2 WHERE id=?1",
                params![job.id, next],
            )?;
        }
        tx.commit()?;
        Ok(recovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::{ChatMessage, MessageRole};
    use std::{
        path::Path,
        sync::{Arc, Barrier, Mutex},
    };

    fn spec() -> JobSpec {
        JobSpec {
            name: "daily check".into(),
            prompt: "Report the current time.".into(),
            schedule: ScheduleSpec::Interval { seconds: 60 },
            enabled_tools: vec!["datetime_now".into()],
            timeout_secs: 120,
        }
    }

    fn store() -> SessionStore {
        SessionStore::open(Path::new(":memory:")).unwrap()
    }

    fn reply() -> ChatResponse {
        ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: "done".into(),
            },
            tool_calls: vec![],
            status: RunStatus::Completed,
            session_id: None,
        }
    }

    fn finish(store: &mut SessionStore, run: &JobRun, status: &str, now: i64) {
        let response = (status == "completed").then(reply);
        assert!(store
            .finish_job_run(&run.id, None, status, response, None, now)
            .unwrap());
    }

    #[test]
    fn job_input_is_bounded_and_does_not_accept_caller_sessions_or_implicit_tools() {
        let mut input = serde_json::to_value(spec()).unwrap();
        input.as_object_mut().unwrap().remove("timeout_secs");
        let parsed: JobSpec = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(parsed.timeout_secs, 120);
        parsed.validate().unwrap();
        input["session_id"] = serde_json::json!("another-users-session");
        assert!(serde_json::from_value::<JobSpec>(input).is_err());
        for tools in [
            vec![],
            vec!["datetime_now".into(), "datetime_now".into()],
            vec!["a".repeat(65)],
        ] {
            assert!(JobSpec {
                enabled_tools: tools,
                ..spec()
            }
            .validate()
            .is_err());
        }
        for timeout in [0, 601, u64::MAX] {
            assert!(JobSpec {
                timeout_secs: timeout,
                ..spec()
            }
            .validate()
            .is_err());
        }
        assert!(JobSpec {
            prompt: "x".repeat(32769),
            ..spec()
        }
        .validate()
        .is_err());
        assert!(JobSpec {
            name: "界".repeat(43),
            ..spec()
        }
        .validate()
        .is_err());
        assert!(JobSpec {
            name: " \n".into(),
            ..spec()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn every_scheduler_operation_rejects_the_ephemeral_backend() {
        let mut memory = SessionStore::memory();
        assert!(memory.create_job(spec(), 0).is_err());
        assert!(memory.list_jobs(10, 0).is_err());
        assert!(memory.get_job("unknown").is_err());
        assert!(memory.set_job_enabled("unknown", true, 0).is_err());
        assert!(memory.delete_job("unknown").is_err());
        assert!(memory.purge_job("unknown").is_err());
        assert!(memory.list_job_runs("unknown", 10, 0).is_err());
        assert!(memory.claim_due_jobs(60000, 4).is_err());
        assert!(memory
            .finish_job_run("unknown", None, "failed", None, None, 0)
            .is_err());
        assert!(memory.recover_jobs(0).is_err());
    }

    #[test]
    fn migrates_v1_without_losing_sessions_or_json_migration_markers() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-jobs-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY NOT NULL,messages TEXT NOT NULL CHECK(json_valid(messages)),accessed_ms INTEGER NOT NULL); CREATE INDEX sessions_accessed ON sessions(accessed_ms); CREATE TABLE migration_sources(path TEXT PRIMARY KEY NOT NULL); INSERT INTO sessions VALUES('old','[]',0); INSERT INTO migration_sources VALUES('retained-source'); PRAGMA user_version=1;").unwrap();
        }
        let mut db = SessionStore::open(&path).unwrap();
        assert!(db.get("old").unwrap().is_some());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT path FROM migration_sources", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "retained-source"
        );
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let job = db.create_job(spec(), 0).unwrap();
        drop(db);
        let mut reopened = SessionStore::open(&path).unwrap();
        assert_eq!(
            reopened.get_job(&job.id).unwrap().unwrap().session_id,
            format!("job:{}", job.id)
        );
        assert!(reopened.recover_jobs(1).unwrap() == 0);
        assert_eq!(reopened.claim_due_jobs(60000, 1).unwrap().len(), 1);
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_claimers_cannot_overlap_jobs_or_exceed_global_capacity() {
        let mut db = store();
        for _ in 0..8 {
            db.create_job(spec(), 0).unwrap();
        }
        let shared = Arc::new(Mutex::new(db));
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let shared = shared.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    shared.lock().unwrap().claim_due_jobs(60000, 8).unwrap()
                })
            })
            .collect();
        let runs: Vec<_> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(runs.len(), 4);
        assert_eq!(
            runs.iter().map(|r| &r.job_id).collect::<HashSet<_>>().len(),
            4
        );
        let mut db = shared.lock().unwrap();
        assert!(db.claim_due_jobs(60000, 4).unwrap().is_empty());
        for run in &runs {
            finish(&mut db, run, "completed", 60001);
        }
        assert_eq!(db.claim_due_jobs(60002, 4).unwrap().len(), 4);
    }

    #[test]
    fn only_recent_occurrences_run_and_busy_jobs_do_not_accumulate_backlog() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        assert!(db.claim_due_jobs(59999, 4).unwrap().is_empty());
        let run = db.claim_due_jobs(65000, 1).unwrap().remove(0);
        assert_eq!(run.scheduled_for_ms, 60000);
        assert!(db.claim_due_jobs(180_000, 4).unwrap().is_empty());
        assert!(db.set_job_enabled(&job.id, true, 180_000).is_err());
        finish(&mut db, &run, "completed", 180_001);
        assert!(db.claim_due_jobs(180_002, 4).unwrap().is_empty());
        let next = db.get_job(&job.id).unwrap().unwrap().next_due_ms;
        assert_eq!(next, 240_002);
        assert_eq!(db.list_job_runs(&job.id, 100, 0).unwrap().len(), 1);
        assert_eq!(db.claim_due_jobs(next, 1).unwrap().len(), 1);
    }

    #[test]
    fn claim_transaction_rolls_back_every_run_and_schedule_if_one_insert_fails() {
        let mut db = store();
        let first = db.create_job(spec(), 0).unwrap();
        let second = db.create_job(spec(), 1).unwrap();
        db.job_conn().unwrap().execute_batch(&format!("CREATE TRIGGER fail_second BEFORE INSERT ON job_runs WHEN NEW.job_id='{}' BEGIN SELECT RAISE(ABORT,'injected claim failure'); END;",second.id)).unwrap();
        assert!(db.claim_due_jobs(60001, 4).is_err());
        for job in [&first, &second] {
            assert!(db.list_job_runs(&job.id, 10, 0).unwrap().is_empty());
            assert_eq!(
                db.get_job(&job.id).unwrap().unwrap().next_due_ms,
                job.next_due_ms
            );
        }
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_second")
            .unwrap();
        assert_eq!(db.claim_due_jobs(60001, 4).unwrap().len(), 2);
    }

    #[test]
    fn run_snapshot_does_not_change_with_job_configuration() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let run = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        let changed = JobSpec {
            prompt: "changed later".into(),
            ..spec()
        };
        db.job_conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET spec=?2 WHERE id=?1",
                params![job.id, serde_json::to_string(&changed).unwrap()],
            )
            .unwrap();
        assert_eq!(
            db.list_job_runs(&job.id, 1, 0).unwrap()[0].spec.prompt,
            run.spec.prompt
        );
    }

    #[test]
    fn session_and_terminal_outcome_commit_atomically_and_cannot_be_overwritten() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let run = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        db.insert(
            job.session_id.clone(),
            SessionRecord::new(vec![ChatMessage {
                role: MessageRole::User,
                content: "original".into(),
            }]),
        )
        .unwrap();
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER fail_finish BEFORE UPDATE OF status ON job_runs BEGIN SELECT RAISE(ABORT,'injected finish failure'); END;").unwrap();
        let messages = vec![reply().message];
        assert!(db
            .finish_job_run(
                &run.id,
                Some((job.session_id.clone(), SessionRecord::new(messages.clone()))),
                "completed",
                Some(reply()),
                None,
                60001
            )
            .is_err());
        assert_eq!(
            db.get(&job.session_id).unwrap().unwrap().messages[0].content,
            "original"
        );
        assert_eq!(
            db.list_job_runs(&job.id, 1, 0).unwrap()[0].status,
            "running"
        );
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_finish")
            .unwrap();
        assert!(db
            .finish_job_run(
                &run.id,
                Some((job.session_id.clone(), SessionRecord::new(messages))),
                "completed",
                Some(reply()),
                None,
                60002
            )
            .unwrap());
        assert_eq!(
            db.get(&job.session_id).unwrap().unwrap().messages[0].content,
            "done"
        );
        assert!(!db
            .finish_job_run(
                &run.id,
                Some((job.session_id.clone(), SessionRecord::new(vec![]))),
                "completed",
                Some(reply()),
                None,
                60003
            )
            .unwrap());
        assert_eq!(db.get(&job.session_id).unwrap().unwrap().messages.len(), 1);
        assert_eq!(
            db.list_job_runs(&job.id, 1, 0).unwrap()[0].finished_ms,
            Some(60002)
        );
    }

    #[test]
    fn finish_cannot_cross_sessions_or_claim_noncompleted_agent_output_as_success() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let run = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        assert!(db
            .finish_job_run(
                &run.id,
                Some(("another-session".into(), SessionRecord::new(vec![]))),
                "completed",
                Some(reply()),
                None,
                60001
            )
            .is_err());
        let mut review = reply();
        review.status = RunStatus::RequiresHumanInput;
        assert!(db
            .finish_job_run(
                &run.id,
                None,
                "completed",
                Some(review.clone()),
                None,
                60001
            )
            .is_err());
        let mut huge = reply();
        huge.message.content = "x".repeat(MAX_RESPONSE_BYTES);
        assert!(db
            .finish_job_run(&run.id, None, "completed", Some(huge), None, 60001)
            .is_err());
        assert!(db.get("another-session").unwrap().is_none());
        assert_eq!(
            db.list_job_runs(&job.id, 1, 0).unwrap()[0].status,
            "running"
        );
        assert!(db
            .finish_job_run(
                &run.id,
                None,
                "needs_review",
                Some(review),
                Some("界".repeat(5000)),
                60001
            )
            .unwrap());
        let completed = db.list_job_runs(&job.id, 1, 0).unwrap().remove(0);
        assert!(completed.error.unwrap().len() <= MAX_ERROR_BYTES);
        assert!(!db.get_job(&job.id).unwrap().unwrap().enabled);
    }

    #[test]
    fn restart_marks_unknown_runs_interrupted_and_skips_downtime_without_replay() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-jobs-recover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let mut db = SessionStore::open(&path).unwrap();
        let running = db.create_job(spec(), 0).unwrap();
        let waiting = db.create_job(spec(), 1).unwrap();
        let run = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        assert_eq!(run.job_id, running.id);
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        assert_eq!(db.recover_jobs(300_000).unwrap(), 1);
        let record = db.list_job_runs(&running.id, 1, 0).unwrap().remove(0);
        assert_eq!(record.status, "interrupted");
        assert!(record.error.unwrap().contains("effects may have occurred"));
        assert!(!db.get_job(&running.id).unwrap().unwrap().enabled);
        assert_eq!(
            db.get_job(&waiting.id).unwrap().unwrap().next_due_ms,
            360_000
        );
        assert!(db.claim_due_jobs(300_000, 4).unwrap().is_empty());
        assert_eq!(db.recover_jobs(300_000).unwrap(), 0);
        let resumed = db
            .set_job_enabled(&running.id, true, 1000)
            .unwrap()
            .unwrap();
        assert!(resumed.next_due_ms > run.scheduled_for_ms);
        assert!(db
            .claim_due_jobs(run.scheduled_for_ms, 1)
            .unwrap()
            .is_empty());
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deleted_jobs_keep_audit_and_cannot_be_purged_while_running() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        assert!(db.purge_job(&job.id).is_err());
        let run = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        assert!(db.delete_job(&job.id).unwrap());
        assert!(!db.delete_job(&job.id).unwrap());
        assert!(db.list_jobs(100, 0).unwrap().is_empty());
        assert!(db.get_job(&job.id).unwrap().unwrap().deleted);
        assert!(db
            .set_job_enabled(&job.id, true, 60001)
            .unwrap_err()
            .is::<JobConflict>());
        assert!(db.purge_job(&job.id).is_err());
        finish(&mut db, &run, "interrupted", 60002);
        assert_eq!(db.list_job_runs(&job.id, 100, 0).unwrap().len(), 1);
        assert!(db.claim_due_jobs(120_000, 4).unwrap().is_empty());
        assert!(db.purge_job(&job.id).unwrap());
        assert!(db.get_job(&job.id).unwrap().is_none());
        assert!(db.list_job_runs(&job.id, 100, 0).unwrap().is_empty());
        assert!(!db.purge_job(&job.id).unwrap());
    }

    #[test]
    fn jobs_quota_counts_deleted_rows_and_explicit_purge_restores_capacity() {
        let mut db = store();
        let jobs: Vec<_> = (0..MAX_JOBS)
            .map(|_| db.create_job(spec(), 0).unwrap())
            .collect();
        assert!(db.create_job(spec(), 0).is_err());
        assert!(db.delete_job(&jobs[0].id).unwrap());
        assert!(db.create_job(spec(), 0).is_err());
        assert!(db.purge_job(&jobs[0].id).unwrap());
        assert!(db.create_job(spec(), 0).is_ok());
        assert_eq!(db.list_jobs(20, 80).unwrap().len(), 20);
        assert!(db.list_jobs(0, 0).is_err());
        assert!(db.list_job_runs(&jobs[1].id, 101, 0).is_err());
    }

    #[test]
    fn retention_keeps_recent_outcomes_and_never_evicts_review_records() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let first = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        finish(&mut db, &first, "needs_review", 60001);
        db.set_job_enabled(&job.id, true, 60000).unwrap();
        for _ in 0..105 {
            let due = db.get_job(&job.id).unwrap().unwrap().next_due_ms;
            let run = db.claim_due_jobs(due, 1).unwrap().remove(0);
            finish(&mut db, &run, "completed", due + 1);
        }
        let runs = db.list_job_runs(&job.id, 100, 0).unwrap();
        assert_eq!(runs.len(), 100);
        assert!(runs
            .iter()
            .any(|r| r.id == first.id && r.status == "needs_review"));
        assert_eq!(runs.iter().filter(|r| r.status == "completed").count(), 99);
        assert_eq!(runs[0].scheduled_for_ms, 106 * 60000);
    }

    #[test]
    fn protected_audit_quota_pauses_instead_of_deleting_or_dispatching_more_work() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        for _ in 0..MAX_RUNS_PER_JOB {
            let due = db.get_job(&job.id).unwrap().unwrap().next_due_ms;
            let run = db.claim_due_jobs(due, 1).unwrap().remove(0);
            finish(&mut db, &run, "needs_review", due + 1);
            db.set_job_enabled(&job.id, true, due + 1).unwrap();
        }
        let due = db.get_job(&job.id).unwrap().unwrap().next_due_ms;
        assert!(db.claim_due_jobs(due, 1).unwrap().is_empty());
        assert!(!db.get_job(&job.id).unwrap().unwrap().enabled);
        assert_eq!(db.list_job_runs(&job.id, 100, 0).unwrap().len(), 100);
    }

    #[test]
    fn global_audit_capacity_is_preserved_when_all_history_needs_review() {
        let mut db = store();
        for _ in 0..MAX_JOBS {
            let job = db.create_job(spec(), 0).unwrap();
            db.job_conn().unwrap().execute(
                "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<100) INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id) SELECT ?1 || ':' || n,?1,n,n,n,'interrupted',?2,?3 FROM numbers",
                params![job.id, serde_json::to_string(&job.spec).unwrap(), job.session_id],
            ).unwrap();
        }
        assert!(db.claim_due_jobs(60000, 4).unwrap().is_empty());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_runs", [], |r| r
                    .get::<_, usize>(0))
                .unwrap(),
            MAX_RUNS
        );
        assert!(db.list_jobs(100, 0).unwrap().iter().all(|job| !job.enabled));
    }

    #[test]
    fn explicit_purge_rolls_back_history_deletion_when_job_deletion_fails() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let run = db.claim_due_jobs(60000, 1).unwrap().remove(0);
        finish(&mut db, &run, "interrupted", 60001);
        assert!(db.delete_job(&job.id).unwrap());
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER fail_purge BEFORE DELETE ON jobs BEGIN SELECT RAISE(ABORT,'injected purge failure'); END;").unwrap();
        assert!(db.purge_job(&job.id).is_err());
        assert!(db.get_job(&job.id).unwrap().unwrap().deleted);
        assert_eq!(db.list_job_runs(&job.id, 100, 0).unwrap()[0].id, run.id);
    }
}
