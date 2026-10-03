// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Persisted schedules and conservative execution accounting. Claiming a run is
//! not a guarantee that an external tool effect occurs exactly once.

use super::{
    channel_types::ScheduledDestination, schedule::ScheduleSpec, store::SessionStore, SessionRecord,
};
use anyhow::{bail, Context, Result};
use jiaclaw_core::{ChatResponse, RunStatus};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub(super) const MAX_JOBS: usize = 100;
const MAX_JOB_CREATION_RECEIPTS: usize = 10_000;
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

// Deliberately not a foreign key: explicit job purge must not revive a creation ID.
// Receipts contain no prompt and are never automatically evicted.
pub(super) const SCHEMA_V10: &str = "
CREATE TABLE job_creation_receipts (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=36 AND substr(id,15,1)='4'),
    spec_hash TEXT NOT NULL CHECK(length(spec_hash)=64 AND spec_hash NOT GLOB '*[^0-9a-f]*')
);
PRAGMA user_version=10;
";

pub(super) fn valid_creation_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| {
        parsed.get_version_num() == 4
            && parsed.get_variant() == uuid::Variant::RFC4122
            && !parsed.is_nil()
            && parsed.to_string() == id
    })
}

fn creation_hash(spec: &JobSpec) -> Result<String> {
    // Typed serialization normalizes object keys and defaulted fields; tool order
    // is intentionally preserved as part of the authorized specification.
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(spec)?)))
}

fn job_creation_on(conn: &Connection, id: &str, fingerprint: &str) -> Result<Option<Job>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT spec_hash FROM job_creation_receipts WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        Some(stored) if stored != fingerprint => {
            Err(JobConflict("creation ID already belongs to a different job specification").into())
        }
        Some(_) => lookup_job(conn, id)?.map(Some).ok_or_else(|| {
            JobConflict("job was purged; this creation ID is permanently retired").into()
        }),
        None if lookup_job(conn, id)?.is_some() => {
            Err(JobConflict("job ID exists without a matching creation receipt").into())
        }
        None => {
            let retained: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [format!("job:{id}")],
                |row| row.get(0),
            )?;
            if retained {
                return Err(JobConflict(
                    "job session ID is already retained; use a fresh creation ID",
                )
                .into());
            }
            Ok(None)
        }
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<ScheduledDestination>,
}

impl JobSpec {
    /// The complete capability contract for gateway-owned background admission.
    pub(super) fn validate_gateway(&self) -> Result<()> {
        self.validate()?;
        if self.timeout_secs > 120
            || self.delivery.is_some()
            || self
                .enabled_tools
                .iter()
                .any(|tool| !matches!(tool.as_str(), "datetime_now" | "json_query"))
        {
            bail!("gateway schedules require timeout_secs <=120, datetime_now/json_query only, and no delivery");
        }
        Ok(())
    }

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
        self.schedule.validate()?;
        if let Some(target) = &self.delivery {
            target.validate()?;
        }
        Ok(())
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

fn trim_history(tx: &Transaction<'_>, job_id: &str, keep: usize) -> Result<usize> {
    let count: usize = tx.query_row(
        "SELECT count(*) FROM job_runs WHERE job_id=?1",
        [job_id],
        |row| row.get(0),
    )?;
    let excess = count.saturating_sub(keep);
    let removable = {
        let mut stmt = tx.prepare("SELECT id FROM job_runs WHERE job_id=?1 AND status IN ('completed','failed','skipped') AND NOT EXISTS(SELECT 1 FROM channel_outbox WHERE job_run_id=job_runs.id AND state NOT IN ('delivered','cancelled')) ORDER BY scheduled_for_ms,id LIMIT ?2")?;
        let rows = stmt.query_map(params![job_id, i64::try_from(excess)?], |r| {
            r.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut removed = 0;
    for run_id in removable {
        // The caller's transaction makes evidence cleanup indivisible. Unknown
        // sends and review/interrupted runs never enter this candidate list.
        tx.execute("DELETE FROM channel_outbox WHERE job_run_id=?1", [&run_id])?;
        removed += tx.execute("DELETE FROM job_runs WHERE id=?1", [&run_id])?;
    }
    Ok(count - removed)
}

fn unresolved_delivery(conn: &Connection, job_id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_outbox d JOIN job_runs r ON r.id=d.job_run_id WHERE r.job_id=?1 AND d.state NOT IN ('delivered','cancelled'))", [job_id], |r|r.get(0))?)
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

pub(super) const SCHEMA_V8: &str = "
CREATE TABLE scheduler_dispatch_clock(id INTEGER PRIMARY KEY CHECK(id=1),highwater_ms INTEGER NOT NULL);
INSERT INTO scheduler_dispatch_clock(id,highwater_ms) VALUES(1,0);
CREATE TABLE scheduler_dispatches(
 request_id TEXT PRIMARY KEY NOT NULL, issued_ms INTEGER NOT NULL, created_ms INTEGER NOT NULL,
 run_id TEXT UNIQUE, job_id TEXT,
 status TEXT NOT NULL CHECK(status IN ('idle','running','completed','failed','needs_review','interrupted','skipped')),
 CHECK((status='idle' AND run_id IS NULL AND job_id IS NULL) OR (status<>'idle' AND run_id IS NOT NULL AND job_id IS NOT NULL))
);
CREATE INDEX scheduler_dispatch_expiry ON scheduler_dispatches(issued_ms) WHERE status IN ('idle','completed');
PRAGMA user_version=8;
";
const MAX_DISPATCHES: usize = 10_000;
const DISPATCH_PAST_MS: i64 = 60_000;
const DISPATCH_FUTURE_MS: i64 = 30_000;
const DISPATCH_RETAIN_MS: i64 = DISPATCH_PAST_MS + 120_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayRunReceipt {
    pub id: String,
    pub job_id: String,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayDispatchReceipt {
    pub request_id: String,
    pub run: Option<GatewayRunReceipt>,
}
pub(super) struct GatewayDispatch {
    pub receipt: GatewayDispatchReceipt,
    pub claimed: Option<JobRun>,
}

/// A canonical, time-bearing identity permits bounded deduplication without
/// accepting an old request again after its receipt has been retired.
pub(super) fn dispatch_issued_ms(request_id: &str) -> Result<i64> {
    let id = uuid::Uuid::parse_str(request_id).context("dispatch requires a canonical UUIDv7")?;
    if id.to_string() != request_id || id.get_version_num() != 7 {
        bail!("dispatch requires a canonical UUIDv7");
    }
    let (seconds, nanos) = id
        .get_timestamp()
        .context("dispatch requires UUIDv7 timestamp")?
        .to_unix();
    i64::try_from(seconds)
        .ok()
        .and_then(|s| s.checked_mul(1000))
        .and_then(|s| s.checked_add(i64::from(nanos / 1_000_000)))
        .context("dispatch timestamp is out of bounds")
}
fn gateway_dispatch_on(
    conn: &Connection,
    request_id: &str,
) -> Result<Option<GatewayDispatchReceipt>> {
    Ok(conn
        .query_row(
            "SELECT run_id,job_id,status FROM scheduler_dispatches WHERE request_id=?1",
            [request_id],
            |row| {
                let id: Option<String> = row.get(0)?;
                let run = id
                    .map(|id| {
                        Ok::<_, rusqlite::Error>(GatewayRunReceipt {
                            id,
                            job_id: row.get(1)?,
                            status: row.get(2)?,
                        })
                    })
                    .transpose()?;
                Ok(GatewayDispatchReceipt {
                    request_id: request_id.to_owned(),
                    run,
                })
            },
        )
        .optional()?)
}

fn claim_due_jobs_in(tx: &Transaction<'_>, now: i64, capacity: usize) -> Result<Vec<JobRun>> {
    let running: usize = tx.query_row(
        "SELECT count(*) FROM job_runs WHERE status='running'",
        [],
        |row| row.get(0),
    )?;
    let capacity = capacity.min(MAX_CONCURRENT_RUNS.saturating_sub(running));
    let jobs = {
        let mut stmt = tx.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE enabled=1 AND deleted=0 AND next_due_ms<=?1 AND NOT EXISTS(SELECT 1 FROM job_runs WHERE job_runs.job_id=jobs.id AND status='running') AND NOT EXISTS(SELECT 1 FROM channel_outbox d JOIN job_runs r ON r.id=d.job_run_id WHERE r.job_id=jobs.id AND d.state NOT IN ('delivered','cancelled')) ORDER BY next_due_ms,id LIMIT 100"
        ))?;
        let rows = stmt.query_map([now], job_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut claimed = Vec::new();
    for job in jobs {
        let next = next_occurrence(tx, &job, now)?;
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
        let remaining = trim_history(tx, &job.id, MAX_RUNS_PER_JOB - 1)?;
        let total: usize = tx.query_row("SELECT count(*) FROM job_runs", [], |row| row.get(0))?;
        if remaining >= MAX_RUNS_PER_JOB || total >= MAX_RUNS {
            tx.execute("UPDATE jobs SET enabled=0 WHERE id=?1", [&job.id])?;
            tracing::warn!(job_id = %job.id, "scheduler paused job: retained audit records reached quota; explicit review and purge required");
            continue;
        }
        if job.spec.delivery.is_some() && !super::channel_store::has_outbox_capacity(tx)? {
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
    Ok(claimed)
}

impl SessionStore {
    pub(super) fn gateway_dispatch(
        &self,
        request_id: &str,
    ) -> Result<Option<GatewayDispatchReceipt>> {
        dispatch_issued_ms(request_id)?;
        gateway_dispatch_on(self.job_conn()?, request_id)
    }

    pub(super) fn gateway_job_due(&self, now: i64) -> Result<bool> {
        Ok(self.job_conn()?.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE enabled=1 AND deleted=0 AND next_due_ms<=?1 AND NOT EXISTS(SELECT 1 FROM job_runs WHERE status='running') AND NOT EXISTS(SELECT 1 FROM channel_outbox d JOIN job_runs r ON r.id=d.job_run_id WHERE r.job_id=jobs.id AND d.state NOT IN ('delivered','cancelled')))", [now], |row| row.get(0))?)
    }

    pub(super) fn claim_gateway_dispatch(
        &mut self,
        request_id: &str,
        now: i64,
    ) -> Result<GatewayDispatch> {
        let issued = dispatch_issued_ms(request_id)?;
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = gateway_dispatch_on(&tx, request_id)? {
            return Ok(GatewayDispatch {
                receipt,
                claimed: None,
            });
        }
        let highwater: i64 = tx.query_row(
            "SELECT highwater_ms FROM scheduler_dispatch_clock WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        let effective_now = now.max(highwater);
        // Persist the anti-rollback bound with every pruning/claim transaction.
        tx.execute(
            "UPDATE scheduler_dispatch_clock SET highwater_ms=?1 WHERE id=1",
            [effective_now],
        )?;
        if issued < effective_now.saturating_sub(DISPATCH_PAST_MS)
            || issued > effective_now.saturating_add(DISPATCH_FUTURE_MS)
        {
            tx.commit()?;
            return Err(JobConflict(
                "dispatch identity expired or outside the accepted clock window",
            )
            .into());
        }
        tx.execute("DELETE FROM scheduler_dispatches WHERE status IN ('idle','completed') AND issued_ms<?1", [effective_now.saturating_sub(DISPATCH_RETAIN_MS)])?;
        let count: usize =
            tx.query_row("SELECT count(*) FROM scheduler_dispatches", [], |row| {
                row.get(0)
            })?;
        if count >= MAX_DISPATCHES {
            tx.commit()?;
            return Err(JobConflict("dispatch audit capacity exhausted; retained uncertain outcomes require operator review").into());
        }
        let claimed = claim_due_jobs_in(&tx, now, 1)?.pop();
        let receipt = GatewayDispatchReceipt {
            request_id: request_id.to_owned(),
            run: claimed.as_ref().map(|run| GatewayRunReceipt {
                id: run.id.clone(),
                job_id: run.job_id.clone(),
                status: run.status.clone(),
            }),
        };
        tx.execute("INSERT INTO scheduler_dispatches(request_id,issued_ms,created_ms,run_id,job_id,status) VALUES(?1,?2,?3,?4,?5,?6)", params![request_id, issued, effective_now, receipt.run.as_ref().map(|r| &r.id), receipt.run.as_ref().map(|r| &r.job_id), receipt.run.as_ref().map_or("idle", |r| r.status.as_str())])?;
        tx.commit()?;
        Ok(GatewayDispatch { receipt, claimed })
    }

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

    /// Read a known creation without changing the job or its next occurrence.
    pub(super) fn get_job_creation(&self, id: &str, spec: &JobSpec) -> Result<Option<Job>> {
        anyhow::ensure!(
            valid_creation_id(id),
            "creation ID must be a canonical UUIDv4"
        );
        job_creation_on(self.job_conn()?, id, &creation_hash(spec)?)
    }

    /// Bind identity, specification and enabled job in one durable transaction.
    /// A retry reads the current record, including paused or deleted state.
    pub(super) fn create_job_with_id(
        &mut self,
        id: &str,
        spec: JobSpec,
        now: i64,
    ) -> Result<(Job, bool)> {
        anyhow::ensure!(
            valid_creation_id(id),
            "creation ID must be a canonical UUIDv4"
        );
        let fingerprint = creation_hash(&spec)?;
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(job) = job_creation_on(&tx, id, &fingerprint)? {
            tx.commit()?;
            return Ok((job, false));
        }
        spec.validate()?;
        let next_due_ms = spec.schedule.next_after(now)?;
        anyhow::ensure!(
            next_due_ms > now,
            "schedule did not advance to a future occurrence"
        );
        let receipts: usize =
            tx.query_row("SELECT count(*) FROM job_creation_receipts", [], |row| {
                row.get(0)
            })?;
        if receipts >= MAX_JOB_CREATION_RECEIPTS {
            return Err(JobConflict("job creation receipt capacity reached (10000 lifetime identities); retained identities cannot be forgotten safely").into());
        }
        let count: usize = tx.query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))?;
        if count >= MAX_JOBS {
            return Err(JobConflict("job quota reached (100 including deleted jobs); explicitly purge deleted jobs to free capacity").into());
        }
        let job = Job {
            id: id.to_owned(),
            session_id: format!("job:{id}"),
            spec,
            enabled: true,
            deleted: false,
            next_due_ms,
            created_ms: now,
        };
        tx.execute(
            "INSERT INTO job_creation_receipts(id,spec_hash) VALUES(?1,?2)",
            params![id, fingerprint],
        )?;
        tx.execute("INSERT INTO jobs(id,spec,enabled,deleted,next_due_ms,created_ms,session_id) VALUES(?1,?2,1,0,?3,?4,?5)",
            params![job.id, serde_json::to_string(&job.spec)?, job.next_due_ms, job.created_ms, job.session_id])?;
        tx.commit()?;
        Ok((job, true))
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
            if unresolved_delivery(&tx, id)? {
                return Err(JobConflict(
                    "resolve or cancel outstanding job deliveries before resuming",
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
        if unresolved_delivery(&tx, id)? {
            return Err(
                JobConflict("resolve or cancel outstanding job deliveries before purging").into(),
            );
        }
        tx.execute("DELETE FROM channel_outbox WHERE job_run_id IN (SELECT id FROM job_runs WHERE job_id=?1)", [id])?;
        tx.execute("DELETE FROM job_runs WHERE job_id=?1", [id])?;
        tx.execute("DELETE FROM jobs WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn get_job_run(&self, run_id: &str) -> Result<Option<JobRun>> {
        Ok(self
            .job_conn()?
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM job_runs WHERE id=?1"),
                [run_id],
                run_from_row,
            )
            .optional()?)
    }

    pub(super) fn cancel_job_delivery(
        &mut self,
        job_id: &str,
        run_id: &str,
        now: i64,
    ) -> Result<bool> {
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM job_runs WHERE id=?1 AND job_id=?2",
                params![run_id, job_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Ok(false);
        };
        let submitting: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_outbox WHERE job_run_id=?1 AND state='submitting')", [run_id], |r|r.get(0))?;
        if status == "running" || submitting {
            return Err(JobConflict(
                "cannot cancel job delivery while execution or submission is in flight",
            )
            .into());
        }
        let cancelled = tx
            .prepare("SELECT id FROM channel_outbox WHERE job_run_id=?1 AND state='unknown'")?
            .query_map([run_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for delivery in cancelled {
            super::channel_store::settle_wecom_delivery_budget(&tx, &delivery, now)?;
        }
        tx.execute("UPDATE channel_outbox SET state='cancelled',finished_ms=?2,error=COALESCE(error,'remaining scheduled delivery explicitly cancelled after review') WHERE job_run_id=?1 AND state<>'delivered'", params![run_id,now])?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn purge_job_delivery(&mut self, job_id: &str, run_id: &str) -> Result<bool> {
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM job_runs WHERE id=?1 AND job_id=?2",
                params![run_id, job_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Ok(false);
        };
        let unresolved: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_outbox WHERE job_run_id=?1 AND state NOT IN ('delivered','cancelled'))", [run_id], |r|r.get(0))?;
        if status == "running" || unresolved {
            return Err(JobConflict("resolve or cancel every job delivery before purging").into());
        }
        tx.execute("DELETE FROM channel_outbox WHERE job_run_id=?1", [run_id])?;
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
        let claimed = claim_due_jobs_in(&tx, now, capacity)?;
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
        let tx = self
            .job_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx.query_row(
            "SELECT job_id,session_id,started_ms,spec FROM job_runs WHERE id=?1 AND status='running'",
            [run_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
        ).optional()?;
        let Some((job_id, session_id, started, spec_json)) = current else {
            return Ok(false);
        };
        let spec: JobSpec = serde_json::from_str(&spec_json)?;
        let mut status = status;
        let mut response = response;
        let mut error = error;
        let chunks = if status == "completed" && spec.delivery.is_some() {
            let answer = response.as_ref().expect("completed response checked above");
            let result = if answer.tool_calls.iter().any(|call| {
                call.result
                    .as_ref()
                    .is_some_and(|value| value.get("error").is_some())
            }) {
                Err(anyhow::anyhow!("tool failure requires review"))
            } else {
                super::outbound::split_text_for(
                    spec.delivery
                        .as_ref()
                        .expect("delivery checked above")
                        .channel,
                    &answer.message.content,
                )
            };
            match result {
                Ok(chunks) => Some(chunks),
                Err(_) => {
                    status = "needs_review";
                    error = Some("scheduled response could not be delivered within bounded text limits or contains failed tools; no message was queued".into());
                    if let Some(answer) = response.as_mut() {
                        answer.status = RunStatus::RequiresHumanInput;
                    }
                    None
                }
            }
        } else {
            None
        };
        let serialized_response = response.map(|r| serde_json::to_string(&r)).transpose()?;
        if serialized_response
            .as_ref()
            .is_some_and(|text| text.len() > MAX_RESPONSE_BYTES)
        {
            bail!("job response exceeds 1 MiB storage limit; retain a bounded needs_review outcome instead");
        }
        if let Some(chunks) = chunks {
            super::channel_store::enqueue_job_delivery(
                &tx,
                run_id,
                spec.delivery.as_ref().expect("delivery validated above"),
                chunks,
                now,
            )?;
        }
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
        tx.execute(
            "UPDATE scheduler_dispatches SET status=?2 WHERE run_id=?1 AND status='running'",
            params![run_id, status],
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
        tx.execute(
            "UPDATE scheduler_dispatches SET status='interrupted' WHERE status='running'",
            [],
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
            delivery: None,
        }
    }

    fn delivery_spec() -> JobSpec {
        JobSpec {
            delivery: Some(ScheduledDestination {
                channel: crate::channel_types::Channel::Telegram,
                installation_id: "123".into(),
                conversation_id: "-100".into(),
                thread_id: None,
            }),
            ..spec()
        }
    }

    fn scheduled_run(db: &mut SessionStore) -> JobRun {
        db.create_job(delivery_spec(), 0).unwrap();
        db.claim_due_jobs(60_000, 1).unwrap().remove(0)
    }

    fn queue_scheduled(db: &mut SessionStore) -> JobRun {
        let run = scheduled_run(db);
        assert!(db
            .finish_job_run(&run.id, None, "completed", Some(reply()), None, 60_001)
            .unwrap());
        run
    }

    fn dispatch_id(milliseconds: i64, serial: u64) -> String {
        let mut bytes = [0_u8; 16];
        bytes[..6].copy_from_slice(&milliseconds.to_be_bytes()[2..]);
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        bytes[10..].copy_from_slice(&serial.to_be_bytes()[2..]);
        uuid::Uuid::from_bytes(bytes).to_string()
    }

    #[test]
    fn gateway_specs_are_a_strict_subset_of_background_capabilities() {
        assert!(spec().validate_gateway().is_ok());
        for tool in ["exec", "shell_exec", "mcp_read", "memory_search"] {
            let mut job = spec();
            job.enabled_tools = vec![tool.into()];
            assert!(job.validate().is_ok());
            assert!(job.validate_gateway().is_err());
        }
        let mut job = spec();
        job.timeout_secs = 121;
        assert!(job.validate_gateway().is_err());
        job.timeout_secs = 120;
        assert!(job.validate_gateway().is_ok());
    }

    #[test]
    fn gateway_claim_and_identity_are_one_transaction_and_repeats_never_claim() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let id = dispatch_id(60_000, 1);
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER reject_dispatch BEFORE INSERT ON scheduler_dispatches BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(db.claim_gateway_dispatch(&id, 60_000).is_err());
        assert!(db.list_job_runs(&job.id, 5, 0).unwrap().is_empty());
        assert_eq!(db.get_job(&job.id).unwrap().unwrap().next_due_ms, 60_000);
        assert!(db.gateway_dispatch(&id).unwrap().is_none());
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_dispatch;")
            .unwrap();
        let dispatch = db.claim_gateway_dispatch(&id, 60_000).unwrap();
        let run = dispatch.claimed.unwrap();
        assert_eq!(dispatch.receipt.run.unwrap().id, run.id);
        let repeated = db.claim_gateway_dispatch(&id, 60_001).unwrap();
        assert!(repeated.claimed.is_none());
        assert_eq!(repeated.receipt.run.unwrap().status, "running");
        db.finish_job_run(&run.id, None, "completed", Some(reply()), None, 60_002)
            .unwrap();
        assert_eq!(
            db.gateway_dispatch(&id)
                .unwrap()
                .unwrap()
                .run
                .unwrap()
                .status,
            "completed"
        );
        db.delete_job(&job.id).unwrap();
        db.purge_job(&job.id).unwrap();
        db.create_job(spec(), 0).unwrap();
        let repeated = db.claim_gateway_dispatch(&id, 60_003).unwrap();
        assert!(repeated.claimed.is_none());
        assert_eq!(repeated.receipt.run.unwrap().id, run.id);
    }

    #[test]
    fn gateway_clock_watermark_survives_restart_and_receipt_retirement() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-dispatch-clock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let mut db = SessionStore::open(&path).unwrap();
        let old = dispatch_id(60_000, 1);
        assert!(db
            .claim_gateway_dispatch(&old, 60_000)
            .unwrap()
            .receipt
            .run
            .is_none());
        let future = dispatch_id(300_001, 2);
        db.claim_gateway_dispatch(&future, 300_001).unwrap();
        assert!(db.gateway_dispatch(&old).unwrap().is_none());
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        assert!(
            db.claim_gateway_dispatch(&old, 60_000).is_err(),
            "clock rollback must not revive a retired identity"
        );
        assert!(db
            .claim_gateway_dispatch(&dispatch_id(330_002, 3), 300_001)
            .is_err());
        assert!(db
            .claim_gateway_dispatch(&dispatch_id(240_000, 4), 300_001)
            .is_err());
        assert!(db
            .claim_gateway_dispatch(&dispatch_id(240_001, 5), 300_001)
            .is_ok());
        assert!(db
            .claim_gateway_dispatch(&uuid::Uuid::new_v4().to_string(), 300_001)
            .is_err());
        assert!(db
            .claim_gateway_dispatch(&future.to_uppercase(), 300_001)
            .is_err());
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn gateway_interruption_receipt_is_atomic_and_never_automatically_pruned() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        let id = dispatch_id(60_000, 1);
        let run = db
            .claim_gateway_dispatch(&id, 60_000)
            .unwrap()
            .claimed
            .unwrap();
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER reject_receipt BEFORE UPDATE OF status ON scheduler_dispatches BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(db
            .finish_job_run(&run.id, None, "completed", Some(reply()), None, 60_001)
            .is_err());
        assert_eq!(db.get_job_run(&run.id).unwrap().unwrap().status, "running");
        assert_eq!(
            db.gateway_dispatch(&id)
                .unwrap()
                .unwrap()
                .run
                .unwrap()
                .status,
            "running"
        );
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_receipt;")
            .unwrap();
        db.recover_jobs(60_002).unwrap();
        assert_eq!(
            db.gateway_dispatch(&id)
                .unwrap()
                .unwrap()
                .run
                .unwrap()
                .status,
            "interrupted"
        );
        assert!(!db.get_job(&job.id).unwrap().unwrap().enabled);
        db.claim_gateway_dispatch(&dispatch_id(400_000, 2), 400_000)
            .unwrap();
        assert_eq!(
            db.gateway_dispatch(&id).unwrap().unwrap().run.unwrap().id,
            run.id
        );
    }

    #[test]
    fn gateway_receipts_do_not_block_normal_run_retention_or_advance_into_replay() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        for number in 1..=120 {
            let now = number * 60_000;
            let run = db
                .claim_gateway_dispatch(&dispatch_id(now, number as u64), now)
                .unwrap()
                .claimed
                .unwrap();
            db.finish_job_run(&run.id, None, "completed", Some(reply()), None, now + 1)
                .unwrap();
        }
        assert_eq!(db.list_job_runs(&job.id, 100, 0).unwrap().len(), 100);
        assert!(db.get_job(&job.id).unwrap().unwrap().enabled);
        let count: usize = db
            .job_conn()
            .unwrap()
            .query_row("SELECT count(*) FROM scheduler_dispatches", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(
            count <= 5,
            "expired terminal receipts should be retired: {count}"
        );
    }

    #[test]
    fn gateway_uncertain_identity_capacity_stops_before_a_new_claim() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        db.job_conn().unwrap().execute_batch("WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<10000) INSERT INTO scheduler_dispatches(request_id,issued_ms,created_ms,run_id,job_id,status) SELECT printf('%08x-0000-7000-8000-%012x',n,n),0,0,printf('run-%d',n),'preserved-job','needs_review' FROM numbers;").unwrap();
        assert!(db
            .claim_gateway_dispatch(&dispatch_id(60_000, 1), 60_000)
            .is_err());
        assert!(db.list_job_runs(&job.id, 5, 0).unwrap().is_empty());
        assert_eq!(db.get_job(&job.id).unwrap().unwrap().next_due_ms, 60_000);
        let count: usize = db
            .job_conn()
            .unwrap()
            .query_row("SELECT count(*) FROM scheduler_dispatches", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, MAX_DISPATCHES);
    }

    #[test]
    fn gateway_due_hint_keeps_overdue_jobs_advancing_without_execution() {
        let mut db = store();
        let job = db.create_job(spec(), 0).unwrap();
        assert!(db.gateway_job_due(120_000).unwrap());
        let receipt = db
            .claim_gateway_dispatch(&dispatch_id(120_000, 1), 120_000)
            .unwrap();
        assert!(receipt.claimed.is_none());
        assert!(receipt.receipt.run.is_none());
        assert!(!db.gateway_job_due(120_000).unwrap());
        assert!(db.list_job_runs(&job.id, 5, 0).unwrap().is_empty());
        assert!(db.get_job(&job.id).unwrap().unwrap().next_due_ms > 120_000);
    }

    #[test]
    fn scheduled_destination_is_explicit_bounded_and_never_accepts_discord_credentials() {
        let mut valid = delivery_spec();
        valid.validate().unwrap();
        let mut json = serde_json::to_value(spec()).unwrap();
        json.as_object_mut().unwrap().remove("delivery");
        assert!(serde_json::from_value::<JobSpec>(json)
            .unwrap()
            .delivery
            .is_none());
        valid.delivery.as_mut().unwrap().channel = crate::channel_types::Channel::Discord;
        assert!(valid.validate().is_err()); // Telegram's negative chat ID is not a Discord channel.
        let discord = valid.delivery.as_mut().unwrap();
        discord.installation_id = "123456789012345678".into();
        discord.conversation_id = "234567890123456789".into();
        discord.thread_id = None;
        valid.validate().unwrap();
        valid.delivery.as_mut().unwrap().thread_id = Some("345678901234567890".into());
        assert!(valid.validate().is_err());
        let mut json = serde_json::to_value(delivery_spec()).unwrap();
        json["delivery"]["sealed_token"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<JobSpec>(json).is_err());
        let mut invalid = delivery_spec();
        invalid.delivery.as_mut().unwrap().thread_id = Some("../thread".into());
        assert!(invalid.validate().is_err());
        let mut invalid = delivery_spec();
        invalid.delivery.as_mut().unwrap().installation_id = " ".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn scheduled_finish_atomically_commits_session_response_and_source_without_fake_event() {
        let mut db = store();
        let run = scheduled_run(&mut db);
        let mut response = reply();
        response.message.content = "😀界".repeat(1200);
        let original = response.message.content.clone();
        let session = SessionRecord::new(vec![response.message.clone()]);
        assert!(db
            .finish_job_run(
                &run.id,
                Some((run.session_id.clone(), session)),
                "completed",
                Some(response),
                None,
                60_001
            )
            .unwrap());
        let chunks = db
            .list_job_deliveries(&run.job_id, &run.id, 100, 0)
            .unwrap();
        assert!(chunks.len() > 1);
        assert_eq!(
            chunks.iter().map(|p| p.text.as_str()).collect::<String>(),
            original
        );
        assert!(chunks.iter().all(|p| p.event_id.is_none()
            && p.job_run_id.as_deref() == Some(run.id.as_str())
            && p.job_id.as_deref() == Some(run.job_id.as_str())
            && p.sealed_token.is_none()));
        assert!(db.list_channel_events(100, 0).unwrap().is_empty());
        assert_eq!(
            db.get(&run.session_id).unwrap().unwrap().messages[0].content,
            original
        );
        assert_eq!(
            db.get_job_run(&run.id).unwrap().unwrap().status,
            "completed"
        );
        assert!(!db
            .finish_job_run(&run.id, None, "completed", Some(reply()), None, 60_002)
            .unwrap());
        assert_eq!(
            db.list_job_deliveries(&run.job_id, &run.id, 100, 0)
                .unwrap()
                .len(),
            chunks.len()
        );
    }

    #[test]
    fn scheduled_outbox_or_terminal_write_failure_rolls_back_entire_completion() {
        let mut db = store();
        let run = scheduled_run(&mut db);
        db.insert(
            run.session_id.clone(),
            SessionRecord::new(vec![ChatMessage {
                role: MessageRole::User,
                content: "before".into(),
            }]),
        )
        .unwrap();
        for trigger in [
            "CREATE TRIGGER fail_commit BEFORE INSERT ON channel_outbox WHEN NEW.job_run_id IS NOT NULL BEGIN SELECT RAISE(ABORT,'outbox failure'); END;",
            "CREATE TRIGGER fail_commit BEFORE UPDATE OF status ON job_runs WHEN NEW.status='completed' BEGIN SELECT RAISE(ABORT,'run failure'); END;",
        ] {
            db.job_conn().unwrap().execute_batch(trigger).unwrap();
            assert!(db.finish_job_run(&run.id,Some((run.session_id.clone(),SessionRecord::new(vec![reply().message]))),"completed",Some(reply()),None,60_001).is_err());
            assert_eq!(db.get(&run.session_id).unwrap().unwrap().messages[0].content,"before");
            assert_eq!(db.get_job_run(&run.id).unwrap().unwrap().status,"running");
            assert!(db.list_job_deliveries(&run.job_id,&run.id,100,0).unwrap().is_empty());
            db.job_conn().unwrap().execute_batch("DROP TRIGGER fail_commit").unwrap();
        }
        assert!(db
            .finish_job_run(&run.id, None, "completed", Some(reply()), None, 60_001)
            .unwrap());
    }

    #[test]
    fn oversized_scheduled_reply_is_reviewable_and_paused_without_global_failure() {
        let mut db = store();
        let run = scheduled_run(&mut db);
        let mut response = reply();
        response.message.content = "x".repeat(16 * 1024 + 1);
        assert!(db
            .finish_job_run(&run.id, None, "completed", Some(response), None, 60_001)
            .unwrap());
        let finished = db.get_job_run(&run.id).unwrap().unwrap();
        assert_eq!(finished.status, "needs_review");
        assert_eq!(
            finished.response.unwrap().status,
            RunStatus::RequiresHumanInput
        );
        assert!(!db.get_job(&run.job_id).unwrap().unwrap().enabled);
        assert!(db
            .list_job_deliveries(&run.job_id, &run.id, 100, 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pending_and_unknown_deliveries_block_runs_and_only_explicit_resume_restarts_job() {
        let mut db = store();
        let run = queue_scheduled(&mut db);
        assert!(db.claim_due_jobs(120_000, 4).unwrap().is_empty());
        assert!(db
            .set_job_enabled(&run.job_id, true, 120_000)
            .unwrap_err()
            .is::<JobConflict>());
        let delivery = db.claim_channel_delivery(60_002).unwrap().unwrap();
        assert!(db
            .cancel_job_delivery(&run.job_id, &run.id, 60_003)
            .unwrap_err()
            .is::<JobConflict>());
        assert!(db
            .purge_job_delivery(&run.job_id, &run.id)
            .unwrap_err()
            .is::<JobConflict>());
        assert!(db
            .finish_channel_delivery(
                &delivery.id,
                delivery.attempts,
                "unknown",
                None,
                Some("uncertain".into()),
                None,
                60_003
            )
            .unwrap());
        assert!(!db.get_job(&run.job_id).unwrap().unwrap().enabled);
        assert!(db
            .set_job_enabled(&run.job_id, true, 120_000)
            .unwrap_err()
            .is::<JobConflict>());
        assert!(db
            .resolve_channel_delivery(&delivery.id, "verified message 123".into(), 120_001)
            .unwrap());
        assert!(!db.get_job(&run.job_id).unwrap().unwrap().enabled);
        assert!(
            db.set_job_enabled(&run.job_id, true, 120_001)
                .unwrap()
                .unwrap()
                .enabled
        );
        assert!(db.purge_job_delivery(&run.job_id, &run.id).unwrap());
        assert!(db.get_job_run(&run.id).unwrap().is_some());
        assert!(db
            .list_job_deliveries(&run.job_id, &run.id, 100, 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn scheduled_delivery_recovery_and_terminal_failures_pause_the_source_job() {
        for outcome in [
            "unknown",
            "permanent_failed",
            "expired",
            "recovery",
            "retry_budget",
        ] {
            let mut db = store();
            let run = queue_scheduled(&mut db);
            let delivery = db.claim_channel_delivery(60_002).unwrap().unwrap();
            if outcome == "recovery" {
                assert_eq!(db.recover_channels(60_003).unwrap(), (0, 1));
            } else if outcome == "retry_budget" {
                for attempt in 1..=5 {
                    let now = 60_003 + i64::from(attempt - 1) * 10_000;
                    db.finish_channel_delivery(
                        &delivery.id,
                        attempt,
                        "retry_wait",
                        None,
                        None,
                        Some(now + 10_000),
                        now,
                    )
                    .unwrap();
                    if attempt < 5 {
                        assert!(db.claim_channel_delivery(now + 10_000).unwrap().is_some());
                    }
                }
            } else {
                db.finish_channel_delivery(&delivery.id, 1, outcome, None, None, None, 60_003)
                    .unwrap();
            }
            assert!(
                !db.get_job(&run.job_id).unwrap().unwrap().enabled,
                "{outcome}"
            );
            assert!(db
                .cancel_job_delivery(&run.job_id, &run.id, 120_000)
                .unwrap());
            assert!(!db.get_job(&run.job_id).unwrap().unwrap().enabled);
            assert!(
                db.set_job_enabled(&run.job_id, true, 120_001)
                    .unwrap()
                    .unwrap()
                    .enabled
            );
        }
    }

    #[test]
    fn job_history_and_purge_preserve_unresolved_delivery_audit_and_ttl_context() {
        let mut db = store();
        let run = queue_scheduled(&mut db);
        db.insert(
            run.session_id.clone(),
            SessionRecord::new(vec![reply().message]),
        )
        .unwrap();
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 0);
        {
            let tx = db.job_conn_mut().unwrap().transaction().unwrap();
            assert_eq!(trim_history(&tx, &run.job_id, 0).unwrap(), 1);
            tx.commit().unwrap();
        }
        assert!(db.delete_job(&run.job_id).unwrap());
        assert!(db.purge_job(&run.job_id).unwrap_err().is::<JobConflict>());
        assert!(!db
            .cancel_job_delivery("other-job", &run.id, 70_000)
            .unwrap());
        assert!(db
            .cancel_job_delivery(&run.job_id, &run.id, 70_000)
            .unwrap());
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 1);
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER prevent_job_purge BEFORE DELETE ON jobs BEGIN SELECT RAISE(ABORT,'purge failure'); END;").unwrap();
        assert!(db.purge_job(&run.job_id).is_err());
        assert_eq!(
            db.list_job_deliveries(&run.job_id, &run.id, 100, 0)
                .unwrap()
                .len(),
            1
        );
        assert!(db.get_job_run(&run.id).unwrap().is_some());
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER prevent_job_purge")
            .unwrap();
        assert!(db.purge_job(&run.job_id).unwrap());
        assert!(db.get_job_run(&run.id).unwrap().is_none());
        assert!(db.list_channel_deliveries(None, 100, 0).unwrap().is_empty());
    }

    #[test]
    fn successful_notifications_continue_past_one_hundred_and_rotate_resolved_history() {
        let mut db = store();
        let job = db.create_job(delivery_spec(), 0).unwrap();
        let mut oldest = None;
        for occurrence in 1..=125 {
            let now = occurrence * 60_000;
            let runs = db.claim_due_jobs(now, 4).unwrap();
            assert_eq!(runs.len(), 1, "successful notifications must keep running");
            let run = &runs[0];
            if oldest.is_none() {
                oldest = Some(run.id.clone());
            }
            db.finish_job_run(&run.id, None, "completed", Some(reply()), None, now + 1)
                .unwrap();
            let delivery = db.claim_channel_delivery(now + 2).unwrap().unwrap();
            db.finish_channel_delivery(
                &delivery.id,
                delivery.attempts,
                "delivered",
                Some("confirmed receipt".into()),
                None,
                None,
                now + 3,
            )
            .unwrap();
            assert!(db.get_job(&job.id).unwrap().unwrap().enabled);
        }
        let runs = db.list_job_runs(&job.id, 100, 0).unwrap();
        assert_eq!(runs.len(), 100);
        assert_eq!(runs.last().unwrap().scheduled_for_ms, 26 * 60_000);
        assert!(db.get_job_run(&oldest.unwrap()).unwrap().is_none());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            100
        );
        assert!(db
            .job_conn()
            .unwrap()
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none());
    }

    #[test]
    fn retention_preserves_all_unresolved_sends_and_review_outcomes() {
        let mut db = store();
        let run = queue_scheduled(&mut db);
        for (index, state) in [
            "retry_wait",
            "submitting",
            "unknown",
            "permanent_failed",
            "expired",
            "delivered",
            "cancelled",
        ]
        .iter()
        .enumerate()
        {
            let id = format!("retained-{index}");
            db.job_conn().unwrap().execute("INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id) VALUES(?1,?2,?3,?3,?3,'completed',?4,?5)", params![id,run.job_id,i64::try_from(index).unwrap(),serde_json::to_string(&run.spec).unwrap(),run.session_id]).unwrap();
            db.job_conn().unwrap().execute("INSERT INTO channel_outbox(id,job_run_id,channel,installation_id,destination_key,destination,ordinal,text,state,receipt,attempts,created_ms,next_attempt_ms) SELECT ?1,?1,channel,installation_id,destination_key,destination,0,text,?2,'old evidence',1,0,0 FROM channel_outbox WHERE job_run_id=?3", params![id,state,run.id]).unwrap();
        }
        for (index, status) in ["needs_review", "interrupted"].iter().enumerate() {
            db.job_conn().unwrap().execute("INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id) VALUES(?1,?2,?3,?3,?3,?4,?5,?6)", params![format!("review-{index}"),run.job_id,100+i64::try_from(index).unwrap(),status,serde_json::to_string(&run.spec).unwrap(),run.session_id]).unwrap();
        }
        {
            let tx = db
                .job_conn_mut()
                .unwrap()
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(trim_history(&tx, &run.job_id, 0).unwrap(), 8);
            tx.commit().unwrap();
        }
        assert!(db.get_job_run("retained-5").unwrap().is_none());
        assert!(db.get_job_run("retained-6").unwrap().is_none());
        for index in 0..5 {
            assert!(db
                .get_job_run(&format!("retained-{index}"))
                .unwrap()
                .is_some());
        }
        assert!(db.get_job_run(&run.id).unwrap().is_some());
        for index in 0..2 {
            assert!(db
                .get_job_run(&format!("review-{index}"))
                .unwrap()
                .is_some());
        }
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            6
        );
    }

    #[test]
    fn claim_trims_resolved_audit_before_reserving_capacity_and_cleanup_is_atomic() {
        let mut db = store();
        let target = db.create_job(delivery_spec(), 0).unwrap();
        for _ in 1..100 {
            db.create_job(delivery_spec(), 0).unwrap();
        }
        // A valid bounded history: 100 jobs each retain 99 successful one-piece
        // notifications, and this target has its hundredth. At 9901 pieces a new
        // 100-piece reservation cannot fit until its oldest audit is cleaned.
        db.job_conn().unwrap().execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<99) INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id) SELECT jobs.id||':'||n.x,jobs.id,n.x*60000,n.x*60000,n.x*60000,'completed',jobs.spec,jobs.session_id FROM jobs CROSS JOIN n;").unwrap();
        db.job_conn().unwrap().execute("INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,finished_ms,status,spec,session_id) VALUES(?1,?2,6000000,6000000,6000000,'completed',?3,?4)", params![format!("{}:100",target.id),target.id,serde_json::to_string(&target.spec).unwrap(),target.session_id]).unwrap();
        let destination = target.spec.delivery.as_ref().unwrap().destination();
        db.job_conn().unwrap().execute("INSERT INTO channel_outbox(id,job_run_id,channel,installation_id,destination_key,destination,ordinal,text,state,receipt,attempts,created_ms,next_attempt_ms) SELECT 'delivery:'||id,id,'telegram','123',?1,?2,0,'done','delivered','verified',1,0,0 FROM job_runs", params![serde_json::to_string(&(&destination.conversation_id,&destination.thread_id)).unwrap(),serde_json::to_string(&destination).unwrap()]).unwrap();
        db.job_conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET enabled=(id=?1),next_due_ms=6060000",
                [&target.id],
            )
            .unwrap();
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER fail_retention BEFORE DELETE ON job_runs BEGIN SELECT RAISE(ABORT,'retention deletion failed'); END;").unwrap();
        assert!(db.claim_due_jobs(6_060_000, 4).is_err());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            9901
        );
        assert!(db
            .get_job_run(&format!("{}:1", target.id))
            .unwrap()
            .is_some());
        assert_eq!(
            db.get_job(&target.id).unwrap().unwrap().next_due_ms,
            6_060_000
        );
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_retention")
            .unwrap();
        let runs = db.claim_due_jobs(6_060_000, 4).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].job_id, target.id);
        assert!(db
            .get_job_run(&format!("{}:1", target.id))
            .unwrap()
            .is_none());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            9900
        );
        assert!(db.get_job(&target.id).unwrap().unwrap().enabled);
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
            routing: None,
        }
    }

    fn finish(store: &mut SessionStore, run: &JobRun, status: &str, now: i64) {
        let response = (status == "completed").then(reply);
        assert!(store
            .finish_job_run(&run.id, None, status, response, None, now)
            .unwrap());
    }

    #[test]
    fn job_creation_identity_survives_restart_recovery_and_purge() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-create-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = SessionStore::open(&path).unwrap();
        let (created, fresh) = db.create_job_with_id(&id, spec(), 0).unwrap();
        assert!(fresh);
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        assert_eq!(db.recover_jobs(60_010).unwrap(), 1);
        let before = db.get_job(&id).unwrap().unwrap();
        let (repeated, fresh) = db.create_job_with_id(&id, spec(), 90_000).unwrap();
        assert!(!fresh);
        assert!(!repeated.enabled);
        assert_eq!(repeated.next_due_ms, before.next_due_ms);
        assert_eq!(repeated.created_ms, created.created_ms);
        assert_eq!(
            db.get_job_run(&run.id).unwrap().unwrap().status,
            "interrupted"
        );
        assert!(db.delete_job(&id).unwrap());
        assert!(db.get_job_creation(&id, &spec()).unwrap().unwrap().deleted);
        assert!(db.purge_job(&id).unwrap());
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        for error in [
            db.get_job_creation(&id, &spec()).unwrap_err(),
            db.create_job_with_id(&id, spec(), 100_000).unwrap_err(),
        ] {
            assert!(error.downcast_ref::<JobConflict>().is_some());
            assert!(error.to_string().contains("permanently retired"));
        }
        assert!(db.get_job(&id).unwrap().is_none());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_creation_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn job_creation_identity_is_atomic_and_conflicts_without_replacing_legacy_jobs() {
        let mut db = store();
        let id = uuid::Uuid::new_v4().to_string();
        db.job_conn().unwrap().execute_batch("CREATE TRIGGER fail_creation BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'fixture rollback'); END;").unwrap();
        assert!(db.create_job_with_id(&id, spec(), 0).is_err());
        assert!(db.get_job_creation(&id, &spec()).unwrap().is_none());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_creation_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_creation")
            .unwrap();
        let (original, _) = db.create_job_with_id(&id, spec(), 0).unwrap();
        let mut other = spec();
        other.prompt = "different authorized work".into();
        assert!(db
            .create_job_with_id(&id, other, 1)
            .unwrap_err()
            .downcast_ref::<JobConflict>()
            .is_some());
        assert_eq!(
            db.get_job(&id).unwrap().unwrap().spec.prompt,
            original.spec.prompt
        );
        let legacy = db.create_job(spec(), 0).unwrap();
        assert!(db
            .create_job_with_id(&legacy.id, spec(), 1)
            .unwrap_err()
            .downcast_ref::<JobConflict>()
            .is_some());
        assert_eq!(db.list_jobs(100, 0).unwrap().len(), 2);
        for invalid in [
            "not-a-uuid".to_owned(),
            uuid::Uuid::nil().to_string(),
            uuid::Uuid::now_v7().to_string(),
            "abcdef01-2345-4678-1abc-def012345678".to_owned(),
            "ABCDEF01-2345-4678-9ABC-DEF012345678".to_owned(),
            id.replace('-', ""),
        ] {
            assert!(db.create_job_with_id(&invalid, spec(), 1).is_err());
        }
    }

    #[test]
    fn job_creation_identity_rejects_retained_or_imported_session_context() {
        let mut db = store();
        let old = db.create_job(spec(), 0).unwrap();
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        db.finish_job_run(
            &run.id,
            Some((
                old.session_id.clone(),
                SessionRecord::new(vec![reply().message]),
            )),
            "completed",
            Some(reply()),
            None,
            60_001,
        )
        .unwrap();
        db.delete_job(&old.id).unwrap();
        db.purge_job(&old.id).unwrap();
        assert!(db.get(&old.session_id).unwrap().is_some());
        assert!(db
            .create_job_with_id(&old.id, spec(), 90_000)
            .unwrap_err()
            .to_string()
            .contains("session ID is already retained"));
        let imported = uuid::Uuid::new_v4().to_string();
        db.insert(
            format!("job:{imported}"),
            SessionRecord::new(vec![reply().message]),
        )
        .unwrap();
        assert!(db.get_job_creation(&imported, &spec()).is_err());
        assert!(db.create_job_with_id(&imported, spec(), 0).is_err());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_creation_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        assert!(db.list_jobs(100, 0).unwrap().is_empty());
    }

    #[test]
    fn job_creation_identity_hash_normalizes_json_defaults_but_preserves_tool_order() {
        let omitted: JobSpec = serde_json::from_str(r#"{"prompt":"Report the current time.","enabled_tools":["datetime_now"],"schedule":{"seconds":60,"kind":"interval"},"name":"daily check"}"#).unwrap();
        assert_eq!(
            creation_hash(&omitted).unwrap(),
            creation_hash(&spec()).unwrap()
        );
        let mut both = spec();
        both.enabled_tools.push("json_query".into());
        let first = creation_hash(&both).unwrap();
        both.enabled_tools.reverse();
        assert_ne!(creation_hash(&both).unwrap(), first);
        let mut db = store();
        let id = uuid::Uuid::new_v4().to_string();
        db.create_job_with_id(&id, spec(), 0).unwrap();
        assert!(!db.create_job_with_id(&id, omitted, 1).unwrap().1);
    }

    #[test]
    fn job_creation_identity_concurrent_callers_create_exactly_one_job() {
        let db = Arc::new(Mutex::new(store()));
        let gate = Arc::new(Barrier::new(8));
        let id = uuid::Uuid::new_v4().to_string();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let (db, gate, id) = (db.clone(), gate.clone(), id.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    db.lock()
                        .unwrap()
                        .create_job_with_id(&id, spec(), 0)
                        .unwrap()
                        .1
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| usize::from(worker.join().unwrap()))
                .sum::<usize>(),
            1
        );
        let db = db.lock().unwrap();
        assert_eq!(db.list_jobs(100, 0).unwrap().len(), 1);
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_creation_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
    }

    #[test]
    fn job_creation_identity_capacity_is_lifetime_bounded_and_known_receipts_still_read() {
        let mut db = store();
        let id = uuid::Uuid::new_v4().to_string();
        db.create_job_with_id(&id, spec(), 0).unwrap();
        db.job_conn().unwrap().execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<9999) INSERT INTO job_creation_receipts(id,spec_hash) SELECT printf('%08x-0000-4000-8000-%012x',x,x),?1 FROM n", ["a".repeat(64)]).unwrap();
        let new_id = uuid::Uuid::new_v4().to_string();
        assert!(db
            .create_job_with_id(&new_id, spec(), 0)
            .unwrap_err()
            .to_string()
            .contains("10000 lifetime"));
        assert!(!db.create_job_with_id(&id, spec(), 9_999).unwrap().1);
        assert!(db.get_job(&new_id).unwrap().is_none());
        // Legacy POST has no receipt guarantee or receipt quota consumption.
        assert!(db.create_job(spec(), 0).is_ok());
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_creation_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            10_000
        );
        let mut full = store();
        for _ in 0..MAX_JOBS {
            full.create_job(spec(), 0).unwrap();
        }
        assert!(full.create_job_with_id(&new_id, spec(), 0).is_err());
        assert!(full.get_job_creation(&new_id, &spec()).unwrap().is_none());
    }

    #[test]
    fn schema9_creation_identity_migration_retains_running_work_and_existing_auth() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-create-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let mut db = SessionStore::open(&path).unwrap();
        let job = db.create_job(spec(), 0).unwrap();
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        db.job_conn()
            .unwrap()
            .execute(
                "INSERT INTO discord_bot_auth(id,credential_hash,blocked) VALUES(1,?1,1)",
                ["f".repeat(64)],
            )
            .unwrap();
        db.job_conn()
            .unwrap()
            .execute_batch("DROP TABLE job_creation_receipts; PRAGMA user_version=9;")
            .unwrap();
        drop(db);
        let db = SessionStore::open(&path).unwrap();
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            db.get_job(&job.id).unwrap().unwrap().spec.prompt,
            job.spec.prompt
        );
        assert_eq!(db.get_job_run(&run.id).unwrap().unwrap().status, "running");
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row(
                    "SELECT blocked FROM discord_bot_auth WHERE id=1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            db.job_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM job_creation_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
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
            10
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
