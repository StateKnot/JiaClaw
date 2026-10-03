// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Durable channel admission and conservative outbound delivery accounting.
use super::{
    channel_types::{Channel, Destination, EventSpec},
    store::SessionStore,
    SessionRecord,
};
use anyhow::{bail, ensure, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Serialize, Serializer};
use std::collections::HashSet;

const MAX_EVENTS: usize = 1000;
pub(super) const MAX_DELIVERIES: usize = 10_000;
pub(super) const MAX_EVENT_CHUNKS: usize = 100;
const MAX_TOMBSTONES: usize = 10_000;
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const MAX_CHUNK_BYTES: usize = 256 * 1024;
const MAX_ATTEMPTS: u32 = 5;
const WECOM_BUDGET_WINDOW_MS: i64 = 86_400_000;
const WECOM_DAILY_LIMIT: i64 = 200;
const MAX_WECOM_RESERVATIONS: i64 = 10_000;

#[derive(Debug)]
pub(super) struct ChannelConflict(pub &'static str);
#[derive(Debug)]
pub(super) struct ChannelCapacity(pub &'static str);
macro_rules! store_error {
    ($name:ident) => {
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for $name {}
    };
}
store_error!(ChannelConflict);
store_error!(ChannelCapacity);

pub(super) const SCHEMA_V3: &str = "
CREATE TABLE channel_events (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, event_id TEXT NOT NULL,
 fingerprint TEXT NOT NULL, session_id TEXT NOT NULL,
 spec TEXT NOT NULL CHECK(json_valid(spec)), sealed_token TEXT, expires_ms INTEGER,
 status TEXT NOT NULL CHECK(status IN ('received','processing','completed','needs_review')),
 error TEXT, created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, reviewed_ms INTEGER,
 UNIQUE(channel,installation_id,event_id)
);
CREATE INDEX channel_events_ready ON channel_events(status,seq);
CREATE UNIQUE INDEX channel_session_processing ON channel_events(session_id) WHERE status='processing';
CREATE TABLE channel_outbox (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 event_id TEXT NOT NULL REFERENCES channel_events(id),
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, destination_key TEXT NOT NULL,
 destination TEXT NOT NULL CHECK(json_valid(destination)), sealed_token TEXT, expires_ms INTEGER,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','submitting','retry_wait','delivered','unknown','permanent_failed','expired','cancelled')),
 receipt TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 5), error TEXT,
 created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, next_attempt_ms INTEGER NOT NULL,
 UNIQUE(event_id,ordinal)
);
CREATE INDEX channel_outbox_ready ON channel_outbox(state,next_attempt_ms,seq);
CREATE INDEX channel_outbox_destination ON channel_outbox(channel,installation_id,destination_key,seq);
CREATE UNIQUE INDEX channel_installation_submitting ON channel_outbox(channel,installation_id) WHERE state='submitting';
CREATE TABLE channel_cooldowns (
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, until_ms INTEGER NOT NULL,
 PRIMARY KEY(channel,installation_id)
);
CREATE TABLE channel_dedup (
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, event_id TEXT NOT NULL,
 id TEXT NOT NULL UNIQUE, fingerprint TEXT NOT NULL, retain_until_ms INTEGER NOT NULL,
 PRIMARY KEY(channel,installation_id,event_id)
);
CREATE INDEX channel_dedup_expiry ON channel_dedup(retain_until_ms);
PRAGMA user_version=3;
";

pub(super) const SCHEMA_V4: &str = "
CREATE TABLE channel_outbox_v4 (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 event_id TEXT REFERENCES channel_events(id),
 job_run_id TEXT REFERENCES job_runs(id) ON DELETE RESTRICT,
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, destination_key TEXT NOT NULL,
 destination TEXT NOT NULL CHECK(json_valid(destination)), sealed_token TEXT, expires_ms INTEGER,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','submitting','retry_wait','delivered','unknown','permanent_failed','expired','cancelled')),
 receipt TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 5), error TEXT,
 created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, next_attempt_ms INTEGER NOT NULL,
 CHECK((event_id IS NOT NULL)+(job_run_id IS NOT NULL)=1),
 CHECK(job_run_id IS NULL OR (channel IN ('telegram','slack') AND sealed_token IS NULL AND expires_ms IS NULL)),
 UNIQUE(event_id,ordinal), UNIQUE(job_run_id,ordinal)
);
INSERT INTO channel_outbox_v4(seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms)
 SELECT seq,id,event_id,NULL,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms FROM channel_outbox;
INSERT INTO sqlite_sequence(name,seq) SELECT 'channel_outbox_v4',seq FROM sqlite_sequence WHERE name='channel_outbox' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='channel_outbox_v4');
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='channel_outbox'),0)) WHERE name='channel_outbox_v4';
DROP TABLE channel_outbox;
ALTER TABLE channel_outbox_v4 RENAME TO channel_outbox;
CREATE INDEX channel_outbox_ready ON channel_outbox(state,next_attempt_ms,seq);
CREATE INDEX channel_outbox_destination ON channel_outbox(channel,installation_id,destination_key,seq);
CREATE INDEX channel_outbox_job_run ON channel_outbox(job_run_id);
CREATE UNIQUE INDEX channel_installation_submitting ON channel_outbox(channel,installation_id) WHERE state='submitting';
PRAGMA user_version=4;
";

// Preserve all v4 source identities and delivery evidence when enabling Feishu jobs.
pub(super) const SCHEMA_V5: &str = "
CREATE TABLE channel_outbox_v5 (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 event_id TEXT REFERENCES channel_events(id),
 job_run_id TEXT REFERENCES job_runs(id) ON DELETE RESTRICT,
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, destination_key TEXT NOT NULL,
 destination TEXT NOT NULL CHECK(json_valid(destination)), sealed_token TEXT, expires_ms INTEGER,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','submitting','retry_wait','delivered','unknown','permanent_failed','expired','cancelled')),
 receipt TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 5), error TEXT,
 created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, next_attempt_ms INTEGER NOT NULL,
 CHECK((event_id IS NOT NULL)+(job_run_id IS NOT NULL)=1),
 CHECK(job_run_id IS NULL OR (channel IN ('telegram','slack','feishu') AND sealed_token IS NULL AND expires_ms IS NULL)),
 UNIQUE(event_id,ordinal), UNIQUE(job_run_id,ordinal)
);
INSERT INTO channel_outbox_v5(seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms)
 SELECT seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms FROM channel_outbox;
INSERT INTO sqlite_sequence(name,seq) SELECT 'channel_outbox_v5',seq FROM sqlite_sequence WHERE name='channel_outbox' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='channel_outbox_v5');
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='channel_outbox'),0)) WHERE name='channel_outbox_v5';
DROP TABLE channel_outbox;
ALTER TABLE channel_outbox_v5 RENAME TO channel_outbox;
CREATE INDEX channel_outbox_ready ON channel_outbox(state,next_attempt_ms,seq);
CREATE INDEX channel_outbox_destination ON channel_outbox(channel,installation_id,destination_key,seq);
CREATE INDEX channel_outbox_job_run ON channel_outbox(job_run_id);
CREATE UNIQUE INDEX channel_installation_submitting ON channel_outbox(channel,installation_id) WHERE state='submitting';
PRAGMA user_version=5;
";

// The send budget outlives deletable message audit records. It has no outbox FK.
pub(super) const SCHEMA_V6: &str = "
CREATE TABLE channel_outbox_v6 (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 event_id TEXT REFERENCES channel_events(id),
 job_run_id TEXT REFERENCES job_runs(id) ON DELETE RESTRICT,
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, destination_key TEXT NOT NULL,
 destination TEXT NOT NULL CHECK(json_valid(destination)), sealed_token TEXT, expires_ms INTEGER,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','submitting','retry_wait','delivered','unknown','permanent_failed','expired','cancelled')),
 receipt TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 5), error TEXT,
 created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, next_attempt_ms INTEGER NOT NULL,
 CHECK((event_id IS NOT NULL)+(job_run_id IS NOT NULL)=1),
 CHECK(job_run_id IS NULL OR (channel IN ('telegram','slack','feishu','wecom') AND sealed_token IS NULL AND expires_ms IS NULL)),
 UNIQUE(event_id,ordinal), UNIQUE(job_run_id,ordinal)
);
INSERT INTO channel_outbox_v6(seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms)
 SELECT seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms FROM channel_outbox;
INSERT INTO sqlite_sequence(name,seq) SELECT 'channel_outbox_v6',seq FROM sqlite_sequence WHERE name='channel_outbox' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='channel_outbox_v6');
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='channel_outbox'),0)) WHERE name='channel_outbox_v6';
DROP TABLE channel_outbox;
ALTER TABLE channel_outbox_v6 RENAME TO channel_outbox;
CREATE INDEX channel_outbox_ready ON channel_outbox(state,next_attempt_ms,seq);
CREATE INDEX channel_outbox_destination ON channel_outbox(channel,installation_id,destination_key,seq);
CREATE INDEX channel_outbox_job_run ON channel_outbox(job_run_id);
CREATE UNIQUE INDEX channel_installation_submitting ON channel_outbox(channel,installation_id) WHERE state='submitting';
CREATE TABLE wecom_send_reservations (
 installation_id TEXT NOT NULL, delivery_id TEXT NOT NULL, attempt INTEGER NOT NULL,
 reserved_ms INTEGER NOT NULL, settled_ms INTEGER CHECK(settled_ms>=reserved_ms),
 PRIMARY KEY(installation_id,delivery_id,attempt)
);
CREATE INDEX wecom_send_reservations_window ON wecom_send_reservations(installation_id,settled_ms);
CREATE INDEX wecom_send_reservations_expiry ON wecom_send_reservations(settled_ms);
PRAGMA user_version=6;
";

// Preserve delivery evidence and the independent WeCom ledger while adding DingTalk jobs.
pub(super) const SCHEMA_V7: &str = "
CREATE TABLE channel_outbox_v7 (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 event_id TEXT REFERENCES channel_events(id),
 job_run_id TEXT REFERENCES job_runs(id) ON DELETE RESTRICT,
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, destination_key TEXT NOT NULL,
 destination TEXT NOT NULL CHECK(json_valid(destination)), sealed_token TEXT, expires_ms INTEGER,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','submitting','retry_wait','delivered','unknown','permanent_failed','expired','cancelled')),
 receipt TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 5), error TEXT,
 created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, next_attempt_ms INTEGER NOT NULL,
 CHECK((event_id IS NOT NULL)+(job_run_id IS NOT NULL)=1),
 CHECK(job_run_id IS NULL OR (channel IN ('telegram','slack','feishu','wecom','dingtalk') AND sealed_token IS NULL AND expires_ms IS NULL)),
 UNIQUE(event_id,ordinal), UNIQUE(job_run_id,ordinal)
);
INSERT INTO channel_outbox_v7(seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms)
 SELECT seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms FROM channel_outbox;
INSERT INTO sqlite_sequence(name,seq) SELECT 'channel_outbox_v7',seq FROM sqlite_sequence WHERE name='channel_outbox' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='channel_outbox_v7');
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='channel_outbox'),0)) WHERE name='channel_outbox_v7';
DROP TABLE channel_outbox;
ALTER TABLE channel_outbox_v7 RENAME TO channel_outbox;
CREATE INDEX channel_outbox_ready ON channel_outbox(state,next_attempt_ms,seq);
CREATE INDEX channel_outbox_destination ON channel_outbox(channel,installation_id,destination_key,seq);
CREATE INDEX channel_outbox_job_run ON channel_outbox(job_run_id);
CREATE UNIQUE INDEX channel_installation_submitting ON channel_outbox(channel,installation_id) WHERE state='submitting';
PRAGMA user_version=7;
";

// Add Bot-authenticated Discord jobs without changing interaction token lifetime.
pub(super) const SCHEMA_V9: &str = "
CREATE TABLE channel_outbox_v9 (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
 event_id TEXT REFERENCES channel_events(id),
 job_run_id TEXT REFERENCES job_runs(id) ON DELETE RESTRICT,
 channel TEXT NOT NULL, installation_id TEXT NOT NULL, destination_key TEXT NOT NULL,
 destination TEXT NOT NULL CHECK(json_valid(destination)), sealed_token TEXT, expires_ms INTEGER,
 ordinal INTEGER NOT NULL CHECK(ordinal>=0), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','submitting','retry_wait','delivered','unknown','permanent_failed','expired','cancelled')),
 receipt TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 5), error TEXT,
 created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, next_attempt_ms INTEGER NOT NULL,
 CHECK((event_id IS NOT NULL)+(job_run_id IS NOT NULL)=1),
 CHECK(job_run_id IS NULL OR (channel IN ('telegram','slack','feishu','wecom','dingtalk','discord') AND sealed_token IS NULL AND expires_ms IS NULL)),
 CHECK(job_run_id IS NULL OR channel<>'discord' OR (json_extract(destination,'$.interaction_id') IS NULL AND json_extract(destination,'$.expires_ms') IS NULL AND json_extract(destination,'$.thread_id') IS NULL)),
 UNIQUE(event_id,ordinal), UNIQUE(job_run_id,ordinal)
);
INSERT INTO channel_outbox_v9(seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms)
 SELECT seq,id,event_id,job_run_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms FROM channel_outbox;
INSERT INTO sqlite_sequence(name,seq) SELECT 'channel_outbox_v9',seq FROM sqlite_sequence WHERE name='channel_outbox' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='channel_outbox_v9');
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='channel_outbox'),0)) WHERE name='channel_outbox_v9';
DROP TABLE channel_outbox;
ALTER TABLE channel_outbox_v9 RENAME TO channel_outbox;
CREATE INDEX channel_outbox_ready ON channel_outbox(state,next_attempt_ms,seq);
CREATE INDEX channel_outbox_destination ON channel_outbox(channel,installation_id,destination_key,seq);
CREATE INDEX channel_outbox_job_run ON channel_outbox(job_run_id);
CREATE UNIQUE INDEX channel_installation_submitting ON channel_outbox(channel,installation_id) WHERE state='submitting';
CREATE TABLE discord_bot_auth (
 id INTEGER PRIMARY KEY CHECK(id=1),
 credential_hash TEXT NOT NULL CHECK(length(credential_hash)=64 AND credential_hash NOT GLOB '*[^0-9a-f]*'),
 blocked INTEGER NOT NULL CHECK(blocked IN (0,1))
);
PRAGMA user_version=9;
";

/// Provider metadata applied atomically with one scheduled Discord outcome.
#[derive(Clone)]
pub(super) struct DiscordDeliveryMeta {
    pub credential_hash: String,
    pub cooldown_until_ms: Option<i64>,
    pub credential_rejected: bool,
}
fn validate_discord_credential_hash(hash: &str) -> Result<()> {
    ensure!(
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Discord credential identity must be a canonical SHA-256 digest"
    );
    Ok(())
}

#[derive(Clone, Serialize)]
pub(super) struct ChannelEvent {
    pub id: String,
    #[serde(serialize_with = "public_spec")]
    pub spec: EventSpec,
    pub status: String,
    pub error: Option<String>,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub reviewed_ms: Option<i64>,
}
impl std::fmt::Debug for ChannelEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelEvent")
            .field("id", &self.id)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct EventAcceptance {
    pub id: String,
    pub created: bool,
    pub status: String,
}

#[derive(Clone, Serialize)]
pub(super) struct ChannelDelivery {
    pub id: String,
    pub event_id: Option<String>,
    pub job_run_id: Option<String>,
    pub job_id: Option<String>,
    pub destination: Destination,
    #[serde(skip_serializing)]
    pub sealed_token: Option<String>,
    pub ordinal: u32,
    pub text: String,
    pub state: String,
    pub receipt: Option<String>,
    pub attempts: u32,
    pub error: Option<String>,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub next_attempt_ms: i64,
}
impl std::fmt::Debug for ChannelDelivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelDelivery")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

fn public_spec<S: Serializer>(
    spec: &EventSpec,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let mut value = serde_json::to_value(spec).map_err(serde::ser::Error::custom)?;
    value
        .as_object_mut()
        .expect("event specification is an object")
        .remove("sealed_token");
    value.serialize(serializer)
}

fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Telegram => "telegram",
        Channel::Slack => "slack",
        Channel::Discord => "discord",
        Channel::Feishu => "feishu",
        Channel::Wecom => "wecom",
        Channel::Dingtalk => "dingtalk",
    }
}
// An ambiguous request holds its credit and installation until an operator has
// reconciled the external effect. Completion/review, never claim time, starts
// the rolling window. This evidence survives deletion of message audit records.
pub(super) fn settle_wecom_delivery_budget(conn: &Connection, id: &str, now: i64) -> Result<()> {
    let reservation: Option<(String, i64)> = conn.query_row(
        "SELECT installation_id,MAX(reserved_ms) FROM wecom_send_reservations WHERE delivery_id=?1 AND settled_ms IS NULL GROUP BY installation_id",
        [id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    if let Some((installation, reserved)) = reservation {
        let settled = now.max(reserved);
        settled
            .checked_add(WECOM_BUDGET_WINDOW_MS)
            .ok_or_else(|| anyhow::anyhow!("WeCom budget timestamp overflow"))?;
        conn.execute("UPDATE wecom_send_reservations SET settled_ms=?2 WHERE delivery_id=?1 AND settled_ms IS NULL", params![id, settled])?;
        pace_wecom_installation(conn, &installation, settled)?;
    }
    Ok(())
}

fn pace_wecom_installation(conn: &Connection, installation: &str, now: i64) -> Result<()> {
    let until = now
        .checked_add(4000)
        .ok_or_else(|| anyhow::anyhow!("delivery pacing timestamp overflow"))?;
    conn.execute("INSERT INTO channel_cooldowns(channel,installation_id,until_ms) VALUES('wecom',?1,?2) ON CONFLICT(channel,installation_id) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)", params![installation, until])?;
    Ok(())
}

fn bounded_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value.chars().any(char::is_control)
        && !value.trim().is_empty()
}
fn validate_spec(spec: &EventSpec) -> Result<()> {
    ensure!(
        bounded_id(&spec.event_id, 256)
            && bounded_id(&spec.session_id, 256)
            && bounded_id(&spec.sender_id, 256),
        "invalid channel event identity"
    );
    ensure!(
        !spec.prompt.trim().is_empty() && spec.prompt.len() <= 32 * 1024,
        "channel prompt must contain 1..32768 bytes"
    );
    ensure!(
        (1..=600).contains(&spec.timeout_secs),
        "channel timeout must be within 1..600 seconds"
    );
    ensure!(
        spec.fingerprint.len() == 64 && spec.fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
        "channel fingerprint must be a SHA-256 hex digest"
    );
    ensure!(
        (1..=32).contains(&spec.enabled_tools.len()),
        "channel event must explicitly allow 1..32 tools"
    );
    let mut names = HashSet::new();
    for name in &spec.enabled_tools {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                && names.insert(name),
            "invalid or duplicate channel tool name"
        );
    }
    let destination = &spec.destination;
    ensure!(
        bounded_id(&destination.installation_id, 128)
            && bounded_id(&destination.conversation_id, 256),
        "invalid channel destination"
    );
    ensure!(
        destination
            .thread_id
            .as_ref()
            .is_none_or(|v| bounded_id(v, 256)),
        "invalid thread identity"
    );
    match destination.channel {
        Channel::Discord => {
            ensure!(
                destination
                    .interaction_id
                    .as_ref()
                    .is_some_and(|v| bounded_id(v, 128))
                    && destination.expires_ms.is_some_and(|v| v > 0),
                "Discord interaction identity and expiry are required"
            );
            ensure!(
                spec.sealed_token
                    .as_ref()
                    .is_some_and(|v| bounded_id(v, 16 * 1024)),
                "Discord requires a bounded sealed token"
            );
        }
        _ => ensure!(
            spec.sealed_token.is_none()
                && destination.interaction_id.is_none()
                && destination.expires_ms.is_none(),
            "interaction credentials are only accepted for Discord"
        ),
    }
    Ok(())
}
fn validate_chunks(channel: Channel, chunks: &[String]) -> Result<()> {
    ensure!(
        chunks.len() <= MAX_EVENT_CHUNKS,
        "channel reply exceeds 100 chunks"
    );
    let maximum = match channel {
        Channel::Telegram => 4096,
        Channel::Slack => 40000,
        Channel::Discord => 2000,
        Channel::Feishu | Channel::Dingtalk => 2000,
        Channel::Wecom => 2048,
    };
    let mut bytes = 0usize;
    for chunk in chunks {
        if channel == Channel::Wecom {
            ensure!(
                super::outbound::valid_wecom_text(chunk),
                "WeCom text exceeds 2048 rendered UTF-8 bytes"
            );
        }
        bytes = bytes
            .checked_add(chunk.len())
            .ok_or_else(|| anyhow::anyhow!("channel reply size overflow"))?;
        ensure!(
            !chunk.is_empty() && chunk.chars().count() <= maximum && bytes <= MAX_CHUNK_BYTES,
            "channel reply exceeds bounded chunk limits"
        );
    }
    Ok(())
}
fn bounded_error(mut error: String) -> String {
    if error.len() > 4096 {
        let mut end = 4084;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
        error.push_str(" [truncated]");
    }
    error
}
fn validate_receipt(receipt: &str) -> Result<()> {
    ensure!(
        bounded_id(receipt, 4096),
        "delivery receipt must contain 1..4096 non-control bytes"
    );
    Ok(())
}
fn page(limit: usize, offset: usize) -> Result<(i64, i64)> {
    ensure!(
        (1..=100).contains(&limit),
        "channel page limit must be within 1..100"
    );
    Ok((i64::try_from(limit)?, i64::try_from(offset)?))
}
fn decode<T: serde::de::DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
const EVENT_FIELDS: &str =
    "id,spec,sealed_token,status,error,created_ms,started_ms,finished_ms,reviewed_ms";
const DELIVERY_FIELDS: &str = "id,event_id,destination,sealed_token,ordinal,text,state,receipt,attempts,error,created_ms,started_ms,finished_ms,next_attempt_ms,job_run_id,(SELECT job_id FROM job_runs r WHERE r.id=job_run_id)";
fn event_row(row: &Row<'_>) -> rusqlite::Result<ChannelEvent> {
    let mut spec: EventSpec = decode(row, 1)?;
    spec.sealed_token = row.get(2)?;
    Ok(ChannelEvent {
        id: row.get(0)?,
        spec,
        status: row.get(3)?,
        error: row.get(4)?,
        created_ms: row.get(5)?,
        started_ms: row.get(6)?,
        finished_ms: row.get(7)?,
        reviewed_ms: row.get(8)?,
    })
}
fn delivery_row(row: &Row<'_>) -> rusqlite::Result<ChannelDelivery> {
    Ok(ChannelDelivery {
        id: row.get(0)?,
        event_id: row.get(1)?,
        destination: decode(row, 2)?,
        sealed_token: row.get(3)?,
        ordinal: row.get(4)?,
        text: row.get(5)?,
        state: row.get(6)?,
        receipt: row.get(7)?,
        attempts: row.get(8)?,
        error: row.get(9)?,
        created_ms: row.get(10)?,
        started_ms: row.get(11)?,
        finished_ms: row.get(12)?,
        next_attempt_ms: row.get(13)?,
        job_run_id: row.get(14)?,
        job_id: row.get(15)?,
    })
}
fn find_event(conn: &Connection, id: &str) -> Result<Option<ChannelEvent>> {
    Ok(conn
        .query_row(
            &format!("SELECT {EVENT_FIELDS} FROM channel_events WHERE id=?1"),
            [id],
            event_row,
        )
        .optional()?)
}
fn expire_tokens(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE channel_events SET sealed_token=NULL WHERE expires_ms<=?1",
        [now],
    )?;
    conn.execute(
        "UPDATE channel_outbox SET sealed_token=NULL WHERE expires_ms<=?1",
        [now],
    )?;
    conn.execute("UPDATE channel_outbox SET state='expired',finished_ms=?1,error='interaction delivery credential expired; review required' WHERE expires_ms<=?1 AND state IN ('pending','retry_wait')",[now])?;
    Ok(())
}
fn expire_tombstones(conn: &Connection, now: i64) -> Result<usize> {
    let removed = conn.execute("DELETE FROM channel_dedup WHERE retain_until_ms<=?1 AND NOT EXISTS(SELECT 1 FROM channel_events e WHERE e.id=channel_dedup.id)",[now])?;
    conn.execute("DELETE FROM channel_cooldowns WHERE until_ms<=?1", [now])?;
    Ok(removed)
}

/// Call while holding an IMMEDIATE transaction before creating another durable
/// processing/running row. Both producers reserve the same maximum reply size.
pub(super) fn has_outbox_capacity(conn: &Connection) -> Result<bool> {
    let actual: usize = conn.query_row("SELECT count(*) FROM channel_outbox", [], |r| r.get(0))?;
    let inbound: usize = conn.query_row(
        "SELECT count(*) FROM channel_events WHERE status='processing'",
        [],
        |r| r.get(0),
    )?;
    let scheduled: usize = conn.query_row("SELECT count(*) FROM job_runs WHERE status='running' AND json_extract(spec,'$.delivery') IS NOT NULL", [], |r| r.get(0))?;
    Ok(actual.saturating_add(
        (inbound.saturating_add(scheduled).saturating_add(1)).saturating_mul(MAX_EVENT_CHUNKS),
    ) <= MAX_DELIVERIES)
}

pub(super) fn enqueue_job_delivery(
    conn: &Connection,
    run_id: &str,
    target: &super::channel_types::ScheduledDestination,
    chunks: Vec<String>,
    now: i64,
) -> Result<()> {
    target.validate()?;
    let destination = target.destination();
    validate_chunks(destination.channel, &chunks)?;
    let actual: usize = conn.query_row("SELECT count(*) FROM channel_outbox", [], |r| r.get(0))?;
    ensure!(
        actual.saturating_add(chunks.len()) <= MAX_DELIVERIES,
        "scheduled delivery exceeded reserved outbox capacity"
    );
    let key = serde_json::to_string(&(&destination.conversation_id, &destination.thread_id))?;
    for (ordinal, text) in chunks.into_iter().enumerate() {
        conn.execute("INSERT INTO channel_outbox(id,job_run_id,channel,installation_id,destination_key,destination,ordinal,text,state,attempts,created_ms,next_attempt_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'pending',0,?9,?9)", params![uuid::Uuid::new_v4().to_string(),run_id,channel_name(destination.channel),destination.installation_id,key,serde_json::to_string(&destination)?,i64::try_from(ordinal)?,text,now])?;
    }
    Ok(())
}

// Only the private gateway Telegram store creates this ledger. Keep these
// checks inside the same IMMEDIATE transaction as the claim; ordinary channel
// callers never touch the table.
pub(super) const MAX_TELEGRAM_OPERATIONS: usize = 16_000;

fn check_recorded_claim(conn: &Connection, request_id: &str) -> Result<()> {
    let id = uuid::Uuid::parse_str(request_id)?;
    ensure!(
        id.get_version_num() == 7
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == request_id,
        "recorded channel request must be a canonical UUIDv7"
    );
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM gateway_telegram_operations WHERE request_id=?1)",
        [request_id],
        |row| row.get(0),
    )?;
    ensure!(!exists, "recorded channel request ID was already used");
    let count: usize = conn.query_row(
        "SELECT count(*) FROM gateway_telegram_operations",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        count < MAX_TELEGRAM_OPERATIONS,
        "Telegram operation ledger is full; reconcile and purge reviewed events"
    );
    Ok(())
}

impl SessionStore {
    fn channel_conn(&self) -> Result<&Connection> {
        match self {
            Self::Sqlite { conn, .. } => Ok(conn),
            Self::Memory(_) => bail!("durable channels require SQLite persistence"),
        }
    }
    fn channel_conn_mut(&mut self) -> Result<&mut Connection> {
        match self {
            Self::Sqlite { conn, .. } => Ok(conn),
            Self::Memory(_) => bail!("durable channels require SQLite persistence"),
        }
    }
    /// Remember only a token digest. A rejected token stays blocked across restarts;
    /// rotation admits a new identity without reviving failed delivery attempts.
    pub(super) fn admit_discord_bot(&mut self, credential_hash: &str) -> Result<bool> {
        validate_discord_credential_hash(credential_hash)?;
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO discord_bot_auth(id,credential_hash,blocked) VALUES(1,?1,0) ON CONFLICT(id) DO UPDATE SET credential_hash=excluded.credential_hash,blocked=0 WHERE discord_bot_auth.credential_hash<>excluded.credential_hash", [credential_hash])?;
        let blocked: bool = tx.query_row(
            "SELECT blocked FROM discord_bot_auth WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(!blocked)
    }
    pub(super) fn accept_channel_event(
        &mut self,
        mut spec: EventSpec,
        now: i64,
    ) -> Result<EventAcceptance> {
        validate_spec(&spec)?;
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let channel = channel_name(spec.destination.channel);
        expire_tombstones(&tx, now)?;
        let existing: Option<(String,String)> = tx.query_row("SELECT id,fingerprint FROM channel_dedup WHERE channel=?1 AND installation_id=?2 AND event_id=?3",params![channel,spec.destination.installation_id,spec.event_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        if let Some((id, fingerprint)) = existing {
            if fingerprint != spec.fingerprint {
                return Err(ChannelConflict(
                    "event identity was reused with a different fingerprint",
                )
                .into());
            }
            let status =
                find_event(&tx, &id)?.map_or_else(|| "purged".into(), |event| event.status);
            return Ok(EventAcceptance {
                id,
                created: false,
                status,
            });
        }
        let events: usize =
            tx.query_row("SELECT count(*) FROM channel_events", [], |r| r.get(0))?;
        let tombstones: usize =
            tx.query_row("SELECT count(*) FROM channel_dedup", [], |r| r.get(0))?;
        let deliveries: usize =
            tx.query_row("SELECT count(*) FROM channel_outbox", [], |r| r.get(0))?;
        if events >= MAX_EVENTS || tombstones >= MAX_TOMBSTONES || deliveries >= MAX_DELIVERIES {
            return Err(ChannelCapacity("channel admission quota reached; purge reviewed events or wait for dedup retention expiry").into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let expires = now
            .checked_add(RETENTION_MS)
            .ok_or_else(|| anyhow::anyhow!("dedup retention overflow"))?;
        let sealed_token = spec.sealed_token.take();
        tx.execute("INSERT INTO channel_events(id,channel,installation_id,event_id,fingerprint,session_id,spec,sealed_token,expires_ms,status,created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'received',?10)",params![id,channel,spec.destination.installation_id,spec.event_id,spec.fingerprint,spec.session_id,serde_json::to_string(&spec)?,sealed_token,spec.destination.expires_ms,now])?;
        tx.execute("INSERT INTO channel_dedup(channel,installation_id,event_id,id,fingerprint,retain_until_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![channel,spec.destination.installation_id,spec.event_id,id,spec.fingerprint,expires])?;
        tx.commit()?;
        Ok(EventAcceptance {
            id,
            created: true,
            status: "received".into(),
        })
    }
    pub(super) fn get_channel_event(&self, id: &str) -> Result<Option<ChannelEvent>> {
        find_event(self.channel_conn()?, id)
    }
    pub(super) fn list_channel_events(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChannelEvent>> {
        let (limit, offset) = page(limit, offset)?;
        let mut stmt = self.channel_conn()?.prepare(&format!(
            "SELECT {EVENT_FIELDS} FROM channel_events ORDER BY seq DESC LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = stmt.query_map(params![limit, offset], event_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub(super) fn claim_channel_event(&mut self, now: i64) -> Result<Option<ChannelEvent>> {
        self.claim_channel_event_impl(now, None)
    }
    pub(super) fn claim_channel_event_recorded(
        &mut self,
        now: i64,
        request_id: &str,
    ) -> Result<Option<ChannelEvent>> {
        self.claim_channel_event_impl(now, Some(request_id))
    }
    fn claim_channel_event_impl(
        &mut self,
        now: i64,
        request_id: Option<&str>,
    ) -> Result<Option<ChannelEvent>> {
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(request_id) = request_id {
            check_recorded_claim(&tx, request_id)?;
        }
        expire_tokens(&tx, now)?;
        let active: usize = tx.query_row(
            "SELECT count(*) FROM channel_events WHERE status='processing'",
            [],
            |r| r.get(0),
        )?;
        if active >= 4 {
            tx.commit()?;
            return Ok(None);
        }
        // Reserve the maximum possible reply before executing any model/tool.
        // Processing rows are the durable reservations, so concurrent completion
        // converts only its own reservation into actual outbox rows.
        if !has_outbox_capacity(&tx)? {
            tx.commit()?;
            return Ok(None);
        }
        let event=tx.query_row(&format!("SELECT {EVENT_FIELDS} FROM channel_events e WHERE status='received' AND NOT EXISTS(SELECT 1 FROM channel_events p WHERE p.session_id=e.session_id AND p.status='processing') ORDER BY seq LIMIT 1"),[],event_row).optional()?;
        let Some(mut event) = event else {
            tx.commit()?;
            return Ok(None);
        };
        tx.execute("UPDATE channel_events SET status='processing',started_ms=?2 WHERE id=?1 AND status='received'",params![event.id,now])?;
        if let Some(request_id) = request_id {
            ensure!(
                event.spec.destination.channel == Channel::Telegram,
                "recorded event claims require a Telegram event"
            );
            tx.execute("INSERT INTO gateway_telegram_operations(request_id,kind,event_id,delivery_id,attempt,claimed_ms) VALUES(?1,'event',?2,NULL,1,?3)",
                params![request_id,event.id,now])?;
        }
        event.status = "processing".into();
        event.started_ms = Some(now);
        tx.commit()?;
        Ok(Some(event))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn complete_channel_event(
        &mut self,
        id: &str,
        session: Option<(String, SessionRecord)>,
        status: &str,
        chunks: Vec<String>,
        error: Option<String>,
        now: i64,
    ) -> Result<bool> {
        ensure!(
            matches!(status, "completed" | "needs_review"),
            "invalid terminal channel event status"
        );
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(event) = find_event(&tx, id)?.filter(|e| e.status == "processing") else {
            return Ok(false);
        };
        validate_chunks(event.spec.destination.channel, &chunks)?;
        let count: usize = tx.query_row("SELECT count(*) FROM channel_outbox", [], |r| r.get(0))?;
        if count.saturating_add(chunks.len()) > MAX_DELIVERIES {
            return Err(ChannelCapacity(
                "channel outbox quota reached; explicit reviewed-event purge required",
            )
            .into());
        }
        if let Some((session_id, record)) = session {
            ensure!(
                session_id == event.spec.session_id,
                "channel event cannot commit another session"
            );
            tx.execute("INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET messages=excluded.messages,accessed_ms=excluded.accessed_ms",params![session_id,serde_json::to_string(&record.messages)?,now])?;
        }
        let destination = &event.spec.destination;
        let key = serde_json::to_string(&(&destination.conversation_id, &destination.thread_id))?;
        for (ordinal, text) in chunks.into_iter().enumerate() {
            tx.execute("INSERT INTO channel_outbox(id,event_id,channel,installation_id,destination_key,destination,sealed_token,expires_ms,ordinal,text,state,attempts,created_ms,next_attempt_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'pending',0,?11,?11)",params![uuid::Uuid::new_v4().to_string(),id,channel_name(destination.channel),destination.installation_id,key,serde_json::to_string(destination)?,event.spec.sealed_token,destination.expires_ms,i64::try_from(ordinal)?,text,now])?;
        }
        tx.execute("UPDATE channel_events SET status=?2,error=?3,finished_ms=?4 WHERE id=?1 AND status='processing'",params![id,status,error.map(bounded_error),now])?;
        expire_tokens(&tx, now)?;
        tx.commit()?;
        Ok(true)
    }
    pub(super) fn get_channel_delivery(&self, id: &str) -> Result<Option<ChannelDelivery>> {
        Ok(self
            .channel_conn()?
            .query_row(
                &format!("SELECT {DELIVERY_FIELDS} FROM channel_outbox WHERE id=?1"),
                [id],
                delivery_row,
            )
            .optional()?)
    }
    pub(super) fn list_channel_deliveries(
        &self,
        event_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChannelDelivery>> {
        let (limit, offset) = page(limit, offset)?;
        let mut stmt=self.channel_conn()?.prepare(&format!("SELECT {DELIVERY_FIELDS} FROM channel_outbox WHERE (?1 IS NULL OR event_id=?1) ORDER BY seq LIMIT ?2 OFFSET ?3"))?;
        let rows = stmt.query_map(params![event_id, limit, offset], delivery_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub(super) fn list_job_deliveries(
        &self,
        job_id: &str,
        run_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChannelDelivery>> {
        let (limit, offset) = page(limit, offset)?;
        let mut stmt = self.channel_conn()?.prepare(&format!("SELECT {DELIVERY_FIELDS} FROM channel_outbox WHERE job_run_id IN (SELECT id FROM job_runs WHERE job_id=?1 AND id=?2) ORDER BY seq LIMIT ?3 OFFSET ?4"))?;
        let rows = stmt.query_map(params![job_id, run_id, limit, offset], delivery_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub(super) fn claim_channel_delivery(&mut self, now: i64) -> Result<Option<ChannelDelivery>> {
        self.claim_channel_delivery_impl(now, None)
    }
    pub(super) fn claim_channel_delivery_recorded(
        &mut self,
        now: i64,
        request_id: &str,
    ) -> Result<Option<ChannelDelivery>> {
        self.claim_channel_delivery_impl(now, Some(request_id))
    }
    fn claim_channel_delivery_impl(
        &mut self,
        now: i64,
        request_id: Option<&str>,
    ) -> Result<Option<ChannelDelivery>> {
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(request_id) = request_id {
            check_recorded_claim(&tx, request_id)?;
        }
        expire_tokens(&tx, now)?;
        // In-flight/unknown requests retain credit indefinitely. A receipt or
        // explicit operator reconciliation starts a fresh rolling-day window;
        // claim time cannot bound when the provider actually accepted a message.
        tx.execute(
            "DELETE FROM wecom_send_reservations WHERE settled_ms<=?1",
            [now.saturating_sub(WECOM_BUDGET_WINDOW_MS)],
        )?;
        tx.execute("UPDATE channel_outbox SET next_attempt_ms=MAX(next_attempt_ms,COALESCE((SELECT MIN(r.settled_ms) FROM wecom_send_reservations r WHERE r.installation_id=channel_outbox.installation_id)+?1,next_attempt_ms)),error='wecom_daily_budget' WHERE channel='wecom' AND state IN ('pending','retry_wait') AND (SELECT COUNT(*) FROM wecom_send_reservations r WHERE r.installation_id=channel_outbox.installation_id)>=?2", params![WECOM_BUDGET_WINDOW_MS,WECOM_DAILY_LIMIT])?;
        tx.execute("UPDATE channel_outbox SET next_attempt_ms=MAX(next_attempt_ms,COALESCE((SELECT MIN(settled_ms) FROM wecom_send_reservations)+?1,next_attempt_ms)),error='wecom_budget_capacity' WHERE channel='wecom' AND state IN ('pending','retry_wait') AND (SELECT COUNT(*) FROM wecom_send_reservations)>=?2", params![WECOM_BUDGET_WINDOW_MS,MAX_WECOM_RESERVATIONS])?;
        tx.execute("UPDATE channel_outbox SET error='wecom_delivery_review_required' WHERE channel='wecom' AND state IN ('pending','retry_wait') AND EXISTS(SELECT 1 FROM channel_outbox u JOIN wecom_send_reservations r ON r.delivery_id=u.id WHERE u.channel='wecom' AND u.installation_id=channel_outbox.installation_id AND u.state='unknown' AND r.settled_ms IS NULL)", [])?;
        let record=tx.query_row(&format!("SELECT {DELIVERY_FIELDS} FROM channel_outbox d WHERE state IN ('pending','retry_wait') AND next_attempt_ms<=?1 AND attempts<5 AND (d.channel<>'wecom' OR ((SELECT COUNT(*) FROM wecom_send_reservations r WHERE r.installation_id=d.installation_id)<?2 AND (SELECT COUNT(*) FROM wecom_send_reservations)<?3 AND NOT EXISTS(SELECT 1 FROM wecom_send_reservations r WHERE r.installation_id=d.installation_id AND r.settled_ms IS NULL))) AND (d.channel<>'discord' OR d.job_run_id IS NULL OR NOT EXISTS(SELECT 1 FROM channel_outbox u WHERE u.channel='discord' AND u.installation_id=d.installation_id AND u.job_run_id IS NOT NULL AND u.state='unknown')) AND NOT EXISTS(SELECT 1 FROM channel_outbox live WHERE live.channel=d.channel AND live.installation_id=d.installation_id AND live.state='submitting') AND NOT EXISTS(SELECT 1 FROM channel_cooldowns c WHERE c.channel=d.channel AND c.installation_id=d.installation_id AND c.until_ms>?1) AND NOT EXISTS(SELECT 1 FROM channel_outbox p WHERE p.channel=d.channel AND p.installation_id=d.installation_id AND p.destination_key=d.destination_key AND p.seq<d.seq AND p.state<>'delivered' AND (p.event_id=d.event_id OR p.job_run_id=d.job_run_id OR p.state<>'cancelled')) ORDER BY seq LIMIT 1"),params![now,WECOM_DAILY_LIMIT,MAX_WECOM_RESERVATIONS],delivery_row).optional()?;
        let Some(mut record) = record else {
            tx.commit()?;
            return Ok(None);
        };
        tx.execute("UPDATE channel_outbox SET state='submitting',attempts=attempts+1,started_ms=?2,error=CASE WHEN channel='wecom' THEN NULL ELSE error END WHERE id=?1 AND state IN ('pending','retry_wait')",params![record.id,now])?;
        let spacing_ms = match record.destination.channel {
            Channel::Telegram => 3100,
            Channel::Slack => 1100,
            Channel::Discord => 300,
            Channel::Feishu => 1100,
            Channel::Wecom | Channel::Dingtalk => 4000,
        };
        let until = now
            .checked_add(spacing_ms)
            .ok_or_else(|| anyhow::anyhow!("delivery pacing timestamp overflow"))?;
        tx.execute("INSERT INTO channel_cooldowns(channel,installation_id,until_ms) VALUES(?1,?2,?3) ON CONFLICT(channel,installation_id) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)",params![channel_name(record.destination.channel),record.destination.installation_id,until])?;
        if record.destination.channel == Channel::Wecom {
            now.checked_add(WECOM_BUDGET_WINDOW_MS)
                .ok_or_else(|| anyhow::anyhow!("WeCom budget timestamp overflow"))?;
            tx.execute("INSERT INTO wecom_send_reservations(installation_id,delivery_id,attempt,reserved_ms) VALUES(?1,?2,?3,?4)", params![record.destination.installation_id,record.id,record.attempts+1,now])?;
        }
        if let Some(request_id) = request_id {
            ensure!(
                record.destination.channel == Channel::Telegram && record.event_id.is_some(),
                "recorded delivery claims require an inbound Telegram event"
            );
            tx.execute("INSERT INTO gateway_telegram_operations(request_id,kind,event_id,delivery_id,attempt,claimed_ms) VALUES(?1,'delivery',?2,?3,?4,?5)",
                params![request_id,record.event_id,record.id,record.attempts + 1,now])?;
        }
        record.state = "submitting".into();
        record.attempts += 1;
        record.started_ms = Some(now);
        if record.destination.channel == Channel::Wecom {
            record.error = None;
        }
        tx.commit()?;
        Ok(Some(record))
    }
    // Owned receipt matches the blocking worker's owned completion message.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub(super) fn finish_channel_delivery(
        &mut self,
        id: &str,
        expected_attempt: u32,
        status: &str,
        receipt: Option<String>,
        error: Option<String>,
        retry_after_ms: Option<i64>,
        now: i64,
    ) -> Result<bool> {
        self.finish_channel_delivery_with_discord(
            id,
            expected_attempt,
            status,
            receipt,
            error,
            retry_after_ms,
            now,
            None,
        )
    }
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub(super) fn finish_channel_delivery_with_discord(
        &mut self,
        id: &str,
        expected_attempt: u32,
        status: &str,
        receipt: Option<String>,
        error: Option<String>,
        retry_after_ms: Option<i64>,
        now: i64,
        discord: Option<DiscordDeliveryMeta>,
    ) -> Result<bool> {
        if let Some(meta) = &discord {
            validate_discord_credential_hash(&meta.credential_hash)?;
            ensure!(
                !meta.credential_rejected || status == "permanent_failed",
                "rejected Discord credentials require a permanent failure"
            );
            ensure!(
                meta.cooldown_until_ms.is_none_or(
                    |until| until >= 0 && until <= now.saturating_add(24 * 60 * 60 * 1000)
                ),
                "Discord cooldown exceeds its bounded deadline"
            );
        }
        ensure!(
            matches!(
                status,
                "delivered" | "unknown" | "permanent_failed" | "expired" | "retry_wait"
            ),
            "invalid delivery outcome"
        );
        if let Some(receipt) = &receipt {
            validate_receipt(receipt)?;
        }
        ensure!(
            status != "delivered" || receipt.is_some(),
            "delivered outcome requires a receipt"
        );
        if status == "retry_wait" {
            ensure!(
                retry_after_ms.is_some_and(
                    |until| until > now && until <= now.saturating_add(24 * 60 * 60 * 1000)
                ),
                "retry deadline must be within the next 24 hours"
            );
        }
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row=tx.query_row("SELECT channel,installation_id,attempts,job_run_id FROM channel_outbox WHERE id=?1 AND state='submitting' AND attempts=?2",params![id,expected_attempt],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u32>(2)?,r.get::<_,Option<String>>(3)?))).optional()?;
        let Some((channel, installation, attempts, job_run_id)) = row else {
            return Ok(false);
        };
        if let Some(meta) = &discord {
            ensure!(
                channel == "discord" && job_run_id.is_some(),
                "Bot metadata requires a scheduled Discord delivery"
            );
            if let Some(until) = meta.cooldown_until_ms {
                tx.execute("INSERT INTO channel_cooldowns(channel,installation_id,until_ms) VALUES('discord',?1,?2) ON CONFLICT(channel,installation_id) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)", params![installation, until])?;
            }
            if meta.credential_rejected {
                // A late old-token result must not disable a newly admitted token.
                tx.execute(
                    "UPDATE discord_bot_auth SET blocked=1 WHERE id=1 AND credential_hash=?1",
                    [&meta.credential_hash],
                )?;
            }
        }
        let terminal = if status == "retry_wait" && attempts >= MAX_ATTEMPTS {
            "permanent_failed"
        } else {
            status
        };
        if status == "retry_wait" {
            tx.execute("INSERT INTO channel_cooldowns(channel,installation_id,until_ms) VALUES(?1,?2,?3) ON CONFLICT(channel,installation_id) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)",params![channel,installation,retry_after_ms])?;
        }
        let error = if terminal == status {
            error.map(bounded_error)
        } else {
            Some("rate-limit retry budget exhausted; review before further action".into())
        };
        tx.execute("UPDATE channel_outbox SET state=?3,receipt=?4,error=?5,next_attempt_ms=?6,finished_ms=?7,sealed_token=CASE WHEN ?3='retry_wait' THEN sealed_token ELSE NULL END WHERE id=?1 AND state='submitting' AND attempts=?2",params![id,expected_attempt,terminal,receipt,error,retry_after_ms.unwrap_or(now),if terminal=="retry_wait"{None}else{Some(now)}])?;
        if channel == "dingtalk" {
            let until = now
                .checked_add(4000)
                .ok_or_else(|| anyhow::anyhow!("delivery pacing timestamp overflow"))?;
            tx.execute("INSERT INTO channel_cooldowns(channel,installation_id,until_ms) VALUES('dingtalk',?1,?2) ON CONFLICT(channel,installation_id) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)", params![installation, until])?;
        }
        if channel == "wecom" {
            if terminal == "unknown" {
                pace_wecom_installation(&tx, &installation, now)?;
            } else {
                settle_wecom_delivery_budget(&tx, id, now)?;
            }
        }
        if matches!(terminal, "unknown" | "permanent_failed" | "expired") {
            tx.execute("UPDATE jobs SET enabled=0 WHERE id IN (SELECT job_id FROM job_runs WHERE id IN (SELECT job_run_id FROM channel_outbox WHERE id=?1))",[id])?;
        }
        expire_tokens(&tx, now)?;
        tx.commit()?;
        Ok(true)
    }
    pub(super) fn recover_channels(&mut self, now: i64) -> Result<(usize, usize)> {
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let events=tx.execute("UPDATE channel_events SET status='needs_review',finished_ms=?1,error='process stopped during agent execution; effects may have occurred; no automatic replay' WHERE status='processing'",[now])?;
        tx.execute("UPDATE jobs SET enabled=0 WHERE id IN (SELECT r.job_id FROM job_runs r JOIN channel_outbox d ON d.job_run_id=r.id WHERE d.state='submitting')", [])?;
        // Recovery preserves every unsettled reservation; no wall-clock timeout
        // proves that the provider stopped processing the interrupted request.
        let wecom_installations = tx.prepare("SELECT DISTINCT installation_id FROM channel_outbox WHERE channel='wecom' AND state='submitting'")?.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for installation in wecom_installations {
            pace_wecom_installation(&tx, &installation, now)?;
        }
        let until = now
            .checked_add(4000)
            .ok_or_else(|| anyhow::anyhow!("delivery pacing timestamp overflow"))?;
        tx.execute("INSERT INTO channel_cooldowns(channel,installation_id,until_ms) SELECT DISTINCT 'dingtalk',installation_id,?1 FROM channel_outbox WHERE channel='dingtalk' AND state='submitting' ON CONFLICT(channel,installation_id) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)", [until])?;
        let deliveries=tx.execute("UPDATE channel_outbox SET state='unknown',finished_ms=?1,sealed_token=NULL,error='process stopped during delivery; provider may have accepted it; reconcile before continuing' WHERE state='submitting'",[now])?;
        expire_tokens(&tx, now)?;
        tx.commit()?;
        Ok((events, deliveries))
    }
    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn resolve_channel_delivery(
        &mut self,
        id: &str,
        receipt: String,
        now: i64,
    ) -> Result<bool> {
        validate_receipt(&receipt)?;
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: Option<String> = tx
            .query_row("SELECT state FROM channel_outbox WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(state) = state else { return Ok(false) };
        if state != "unknown" {
            return Err(
                ChannelConflict("only unknown deliveries can be reconciled as delivered").into(),
            );
        }
        tx.execute("UPDATE channel_outbox SET state='delivered',receipt=?2,error=COALESCE(error,'previous delivery outcome was unknown; administrator supplied receipt'),finished_ms=?3,sealed_token=NULL WHERE id=?1 AND state='unknown'",params![id,receipt,now])?;
        settle_wecom_delivery_budget(&tx, id, now)?;
        tx.commit()?;
        Ok(true)
    }
    pub(super) fn cancel_channel_event(&mut self, id: &str, now: i64) -> Result<bool> {
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(event) = find_event(&tx, id)? else {
            return Ok(false);
        };
        let submitting: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_outbox WHERE event_id=?1 AND state='submitting')",
            [id],
            |r| r.get(0),
        )?;
        if event.status == "processing" || submitting {
            return Err(ChannelConflict(
                "cannot cancel an event while agent execution or delivery is in flight",
            )
            .into());
        }
        let cancelled = tx
            .prepare("SELECT id FROM channel_outbox WHERE event_id=?1 AND state='unknown'")?
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for delivery in cancelled {
            settle_wecom_delivery_budget(&tx, &delivery, now)?;
        }
        tx.execute("UPDATE channel_outbox SET state='cancelled',finished_ms=?2,sealed_token=NULL,error=COALESCE(error,'remaining delivery explicitly cancelled after review') WHERE event_id=?1 AND state<>'delivered'",params![id,now])?;
        tx.execute("UPDATE channel_events SET status=CASE WHEN status='received' THEN 'needs_review' ELSE status END,reviewed_ms=?2,finished_ms=COALESCE(finished_ms,?2),sealed_token=NULL WHERE id=?1",params![id,now])?;
        tx.commit()?;
        Ok(true)
    }
    pub(super) fn purge_channel_event(&mut self, id: &str, now: i64) -> Result<bool> {
        let tx = self
            .channel_conn_mut()?
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(event) = find_event(&tx, id)? else {
            return Ok(false);
        };
        if !(event.status == "completed"
            || (event.status == "needs_review" && event.reviewed_ms.is_some()))
        {
            return Err(ChannelConflict(
                "event must be completed or explicitly reviewed before purge",
            )
            .into());
        }
        let unresolved:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_outbox WHERE event_id=?1 AND state NOT IN ('delivered','cancelled'))",[id],|r|r.get(0))?;
        if unresolved {
            return Err(ChannelConflict(
                "reconcile or cancel every outstanding delivery before purge",
            )
            .into());
        }
        let until = now
            .checked_add(RETENTION_MS)
            .ok_or_else(|| anyhow::anyhow!("dedup retention overflow"))?;
        tx.execute(
            "UPDATE channel_dedup SET retain_until_ms=MAX(retain_until_ms,?2) WHERE id=?1",
            params![id, until],
        )?;
        tx.execute("DELETE FROM channel_outbox WHERE event_id=?1", [id])?;
        tx.execute("DELETE FROM channel_events WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::ChatMessage;
    use std::{
        path::Path,
        sync::{Arc, Barrier, Mutex},
    };

    fn database() -> SessionStore {
        SessionStore::open(Path::new(":memory:")).unwrap()
    }
    fn spec(event: &str) -> EventSpec {
        EventSpec {
            event_id: event.into(),
            session_id: "channel:room".into(),
            sender_id: "user".into(),
            prompt: "hello".into(),
            enabled_tools: vec!["datetime_now".into()],
            timeout_secs: 120,
            destination: Destination {
                channel: Channel::Telegram,
                installation_id: "installation".into(),
                conversation_id: "room".into(),
                thread_id: None,
                interaction_id: None,
                expires_ms: None,
            },
            sealed_token: None,
            fingerprint: "f".repeat(64),
        }
    }
    fn history(content: &str) -> SessionRecord {
        SessionRecord::new(vec![ChatMessage {
            role: jiaclaw_core::MessageRole::User,
            content: content.into(),
        }])
    }
    fn complete(db: &mut SessionStore, input: EventSpec, chunks: &[&str]) -> String {
        let event = db.accept_channel_event(input, 0).unwrap();
        let claimed = db.claim_channel_event(0).unwrap().unwrap();
        assert_eq!(event.id, claimed.id);
        assert!(db
            .complete_channel_event(
                &event.id,
                None,
                "completed",
                chunks.iter().map(|s| (*s).into()).collect(),
                None,
                0
            )
            .unwrap());
        event.id
    }
    fn delivered(db: &mut SessionStore, delivery: &ChannelDelivery, now: i64) {
        assert!(db
            .finish_channel_delivery(
                &delivery.id,
                delivery.attempts,
                "delivered",
                Some(format!("platform:{}", delivery.id)),
                None,
                None,
                now
            )
            .unwrap());
    }

    fn scheduled_spec() -> crate::jobs::JobSpec {
        crate::jobs::JobSpec {
            name: "scheduled delivery".into(),
            prompt: "hello".into(),
            schedule: crate::schedule::ScheduleSpec::Interval { seconds: 60 },
            enabled_tools: vec!["datetime_now".into()],
            timeout_secs: 120,
            delivery: Some(crate::channel_types::ScheduledDestination {
                channel: Channel::Telegram,
                installation_id: "123".into(),
                conversation_id: "-100".into(),
                thread_id: None,
            }),
        }
    }

    fn scheduled_reply() -> jiaclaw_core::ChatResponse {
        jiaclaw_core::ChatResponse {
            message: ChatMessage {
                role: jiaclaw_core::MessageRole::Assistant,
                content: "one".into(),
            },
            tool_calls: vec![],
            status: jiaclaw_core::RunStatus::Completed,
            session_id: None,
            routing: None,
        }
    }

    #[test]
    fn migration_preserves_v3_to_v8_delivery_evidence_and_sequence_even_when_empty() {
        for (version, empty) in [
            (3, false),
            (3, true),
            (4, false),
            (4, true),
            (5, false),
            (5, true),
            (6, false),
            (6, true),
            (7, false),
            (7, true),
            (8, false),
            (8, true),
        ] {
            let directory =
                std::env::temp_dir().join(format!("jiaclaw-v4-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("state.sqlite3");
            let (event_id, outbox_id) = {
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE sessions(id TEXT PRIMARY KEY NOT NULL,messages TEXT NOT NULL CHECK(json_valid(messages)),accessed_ms INTEGER NOT NULL);CREATE TABLE migration_sources(path TEXT PRIMARY KEY NOT NULL);").unwrap();
                conn.execute_batch(crate::jobs::SCHEMA_V2).unwrap();
                conn.execute_batch(SCHEMA_V3).unwrap();
                if version >= 4 {
                    conn.execute_batch(SCHEMA_V4).unwrap();
                }
                if version >= 5 {
                    conn.execute_batch(SCHEMA_V5).unwrap();
                }
                if version >= 6 {
                    conn.execute_batch(SCHEMA_V6).unwrap();
                    conn.execute_batch("INSERT INTO wecom_send_reservations VALUES('wwold:1','pending-evidence',1,500,NULL),('wwold:1','settled-evidence',1,501,999);").unwrap();
                }
                if version >= 7 {
                    conn.execute_batch(SCHEMA_V7).unwrap();
                }
                if version >= 8 {
                    conn.execute_batch(crate::jobs::SCHEMA_V8).unwrap();
                }
                let mut old = SessionStore::Sqlite {
                    conn,
                    _ownership: None,
                };
                let mut discord = spec("legacy");
                discord.destination.channel = Channel::Discord;
                discord.destination.interaction_id = Some("interaction".into());
                discord.destination.expires_ms = Some(999_999);
                discord.sealed_token = Some("retained-ciphertext".into());
                let id = complete(&mut old, discord, &["legacy text"]);
                let outbox_id: String = old
                    .channel_conn()
                    .unwrap()
                    .query_row("SELECT id FROM channel_outbox", [], |r| r.get(0))
                    .unwrap();
                old.channel_conn().unwrap().execute_batch("UPDATE channel_outbox SET seq=100,state='unknown',receipt='receipt-evidence',attempts=2,error='uncertain',started_ms=10,finished_ms=20,next_attempt_ms=30; UPDATE sqlite_sequence SET seq=500 WHERE name='channel_outbox'; INSERT INTO channel_cooldowns VALUES('discord','installation',90000);").unwrap();
                if empty {
                    old.channel_conn()
                        .unwrap()
                        .execute("DELETE FROM channel_outbox", [])
                        .unwrap();
                }
                (id, outbox_id)
            };
            let mut db = SessionStore::open(&path).unwrap();
            if version >= 6 {
                let ledger = db.channel_conn().unwrap().prepare("SELECT delivery_id,reserved_ms,settled_ms FROM wecom_send_reservations ORDER BY delivery_id").unwrap().query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,Option<i64>>(2)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
                assert_eq!(
                    ledger,
                    vec![
                        ("pending-evidence".into(), 500, None),
                        ("settled-evidence".into(), 501, Some(999))
                    ]
                );
            }
            assert_eq!(
                db.channel_conn()
                    .unwrap()
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                10
            );
            assert_eq!(
                db.channel_conn()
                    .unwrap()
                    .query_row(
                        "SELECT seq FROM sqlite_sequence WHERE name='channel_outbox'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                500
            );
            assert_eq!(
                db.channel_conn()
                    .unwrap()
                    .query_row("SELECT until_ms FROM channel_cooldowns", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                90_000
            );
            assert_eq!(
                db.get_channel_event(&event_id)
                    .unwrap()
                    .unwrap()
                    .spec
                    .sealed_token
                    .as_deref(),
                Some("retained-ciphertext")
            );
            if !empty {
                let row = db.get_channel_delivery(&outbox_id).unwrap().unwrap();
                assert_eq!(row.event_id.as_deref(), Some(event_id.as_str()));
                assert!(row.job_run_id.is_none() && row.job_id.is_none());
                assert_eq!(row.sealed_token.as_deref(), Some("retained-ciphertext"));
                assert_eq!(
                    (row.state.as_str(), row.text.as_str(), row.attempts),
                    ("unknown", "legacy text", 2)
                );
                assert_eq!(
                    (row.receipt.as_deref(), row.error.as_deref()),
                    (Some("receipt-evidence"), Some("uncertain"))
                );
                assert_eq!(
                    (row.started_ms, row.finished_ms, row.next_attempt_ms),
                    (Some(10), Some(20), 30)
                );
                assert_eq!(
                    db.channel_conn()
                        .unwrap()
                        .query_row("SELECT seq FROM channel_outbox", [], |r| r.get::<_, i64>(0))
                        .unwrap(),
                    100
                );
            }
            complete(&mut db, spec("new"), &["new text"]);
            assert_eq!(
                db.channel_conn()
                    .unwrap()
                    .query_row("SELECT MAX(seq) FROM channel_outbox", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                501
            );
            assert!(db
                .channel_conn()
                .unwrap()
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none());
            drop(db);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn v4_to_v9_migrate_scheduled_sources_without_weakening_constraints() {
        for old_version in [4, 5, 6] {
            assert_scheduled_sources_survive_migration(old_version);
        }
    }

    #[test]
    fn v7_and_v8_to_v9_preserve_scheduled_sources_and_dispatch_state() {
        assert_scheduled_sources_survive_migration(7);
        assert_scheduled_sources_survive_migration(8);
    }

    fn assert_scheduled_sources_survive_migration(old_version: i64) {
        let directory = std::env::temp_dir().join(format!("jiaclaw-v5-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let (job_id, run_id, delivery_id, before) = {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE sessions(id TEXT PRIMARY KEY NOT NULL,messages TEXT NOT NULL CHECK(json_valid(messages)),accessed_ms INTEGER NOT NULL);CREATE TABLE migration_sources(path TEXT PRIMARY KEY NOT NULL);").unwrap();
            conn.execute_batch(crate::jobs::SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            if old_version >= 5 {
                conn.execute_batch(SCHEMA_V5).unwrap();
            }
            if old_version >= 6 {
                conn.execute_batch(SCHEMA_V6).unwrap();
            }
            if old_version >= 7 {
                conn.execute_batch(SCHEMA_V7).unwrap();
            }
            if old_version >= 8 {
                conn.execute_batch(crate::jobs::SCHEMA_V8).unwrap();
                conn.execute_batch("UPDATE scheduler_dispatch_clock SET highwater_ms=60001; INSERT INTO scheduler_dispatches VALUES('old-dispatch',60000,60001,NULL,NULL,'idle');").unwrap();
            }
            let mut old = SessionStore::Sqlite {
                conn,
                _ownership: None,
            };
            let mut prior = scheduled_spec();
            if old_version >= 5 {
                prior.delivery = Some(super::super::channel_types::ScheduledDestination {
                    channel: Channel::Feishu,
                    installation_id: "cli_fixture:tenant_fixture".into(),
                    conversation_id: "oc_prior".into(),
                    thread_id: Some("om_root".into()),
                });
            }
            let job = old.create_job(prior, 0).unwrap();
            let run = old.claim_due_jobs(60_000, 1).unwrap().remove(0);
            // Seed a real legacy terminal outcome without calling the v8
            // completion path, which also writes scheduler dispatch receipts.
            let tx = old.channel_conn_mut().unwrap().transaction().unwrap();
            tx.execute(
                "UPDATE job_runs SET status='completed',response=?2,finished_ms=60001 WHERE id=?1",
                params![run.id, serde_json::to_string(&scheduled_reply()).unwrap()],
            )
            .unwrap();
            enqueue_job_delivery(
                &tx,
                &run.id,
                job.spec.delivery.as_ref().unwrap(),
                vec!["one".into()],
                60_001,
            )
            .unwrap();
            tx.execute(
                "INSERT INTO sessions(id,messages,accessed_ms) VALUES(?1,'[]',60001)",
                [&run.session_id],
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                old.channel_conn()
                    .unwrap()
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                old_version
            );
            assert_eq!(
                old.channel_conn()
                    .unwrap()
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='scheduler_dispatches'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                i64::from(old_version >= 8)
            );
            let row = old
                .list_job_deliveries(&job.id, &run.id, 100, 0)
                .unwrap()
                .remove(0);
            old.channel_conn().unwrap().execute("UPDATE channel_outbox SET state='unknown',attempts=2,receipt='evidence',error='ambiguous',started_ms=60002,finished_ms=60003 WHERE id=?1", [&row.id]).unwrap();
            let before =
                serde_json::to_value(old.get_channel_delivery(&row.id).unwrap().unwrap()).unwrap();
            (job.id, run.id, row.id, before)
        };
        let mut db = SessionStore::open(&path).unwrap();
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM scheduler_dispatches", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            i64::from(old_version >= 8)
        );
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row(
                    "SELECT highwater_ms FROM scheduler_dispatch_clock WHERE id=1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            if old_version >= 8 { 60001 } else { 0 }
        );
        let preserved_job = db.get_job(&job_id).unwrap().unwrap();
        let preserved_run = db.list_job_runs(&job_id, 10, 0).unwrap().remove(0);
        assert_eq!(preserved_run.id, run_id);
        assert_eq!(preserved_run.status, "completed");
        assert_eq!(
            serde_json::to_value(preserved_run.response).unwrap(),
            serde_json::to_value(Some(scheduled_reply())).unwrap()
        );
        assert_eq!(preserved_run.finished_ms, Some(60_001));
        assert!(db.get(&preserved_job.session_id).unwrap().is_some());
        let after = db.get_channel_delivery(&delivery_id).unwrap().unwrap();
        assert_eq!(serde_json::to_value(&after).unwrap(), before);
        assert_eq!(after.job_id.as_deref(), Some(job_id.as_str()));
        assert_eq!(after.job_run_id.as_deref(), Some(run_id.as_str()));
        assert!(db
            .channel_conn()
            .unwrap()
            .execute("DELETE FROM job_runs WHERE id=?1", [&run_id])
            .is_err());
        let mut next = scheduled_spec();
        next.delivery = Some(super::super::channel_types::ScheduledDestination {
            channel: if old_version >= 6 {
                Channel::Dingtalk
            } else if old_version == 5 {
                Channel::Wecom
            } else {
                Channel::Feishu
            },
            installation_id: if old_version >= 6 {
                "dingrobot:dingcorp"
            } else if old_version == 5 {
                "wwfixture:1"
            } else {
                "cli_fixture:tenant_fixture"
            }
            .into(),
            conversation_id: if old_version >= 5 {
                "alice"
            } else {
                "oc_fixture"
            }
            .into(),
            thread_id: None,
        });
        let job = db.create_job(next, 70_000).unwrap();
        let run = db.claim_due_jobs(130_000, 1).unwrap().remove(0);
        assert_eq!(run.job_id, job.id);
        db.finish_job_run(
            &run.id,
            None,
            "completed",
            Some(scheduled_reply()),
            None,
            130_001,
        )
        .unwrap();
        let row = db
            .list_job_deliveries(&job.id, &run.id, 100, 0)
            .unwrap()
            .remove(0);
        assert_eq!(
            row.destination.channel,
            if old_version >= 6 {
                Channel::Dingtalk
            } else if old_version == 5 {
                Channel::Wecom
            } else {
                Channel::Feishu
            }
        );
        assert!(db
            .channel_conn()
            .unwrap()
            .execute(
                "UPDATE channel_outbox SET sealed_token='forbidden' WHERE id=?1",
                [&row.id]
            )
            .is_err());
        assert!(db
            .channel_conn()
            .unwrap()
            .execute(
                "UPDATE channel_outbox SET channel='discord', destination=json_set(destination,'$.interaction_id','forbidden') WHERE id=?1",
                [&row.id]
            )
            .is_err());
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn discord_scheduled(db: &mut SessionStore, installation: &str, room: &str) -> ChannelDelivery {
        let mut input = scheduled_spec();
        input.delivery = Some(crate::channel_types::ScheduledDestination {
            channel: Channel::Discord,
            installation_id: installation.into(),
            conversation_id: room.into(),
            thread_id: None,
        });
        let job = db.create_job(input, 0).unwrap();
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        assert_eq!(run.job_id, job.id);
        db.finish_job_run(
            &run.id,
            None,
            "completed",
            Some(scheduled_reply()),
            None,
            60_001,
        )
        .unwrap();
        db.list_job_deliveries(&job.id, &run.id, 10, 0)
            .unwrap()
            .remove(0)
    }
    fn discord_meta(hash: &str, until: Option<i64>, rejected: bool) -> DiscordDeliveryMeta {
        DiscordDeliveryMeta {
            credential_hash: hash.into(),
            cooldown_until_ms: until,
            credential_rejected: rejected,
        }
    }
    fn discord_cooldown(db: &SessionStore, installation: &str) -> i64 {
        db.channel_conn().unwrap().query_row("SELECT until_ms FROM channel_cooldowns WHERE channel='discord' AND installation_id=?1", [installation], |r| r.get(0)).unwrap()
    }

    #[test]
    fn discord_scheduled_schema_excludes_interaction_credentials_and_threads() {
        let mut db = database();
        let item = discord_scheduled(&mut db, "111111111111111111", "222222222222222222");
        assert!(item.event_id.is_none() && item.job_run_id.is_some());
        assert!(
            item.sealed_token.is_none()
                && item.destination.interaction_id.is_none()
                && item.destination.expires_ms.is_none()
        );
        for change in [
            "sealed_token='not-a-bot-token'",
            "expires_ms=90000",
            "destination=json_set(destination,'$.interaction_id','interaction')",
            "destination=json_set(destination,'$.expires_ms',90000)",
            "destination=json_set(destination,'$.thread_id','333333333333333333')",
        ] {
            assert!(
                db.channel_conn()
                    .unwrap()
                    .execute(
                        &format!("UPDATE channel_outbox SET {change} WHERE id=?1"),
                        [&item.id]
                    )
                    .is_err(),
                "accepted {change}"
            );
        }
        let mut inbound = spec("invalid-interaction");
        inbound.destination.channel = Channel::Discord;
        assert!(db.accept_channel_event(inbound, 60_002).is_err());
        for hash in ["", "secret-token", &"A".repeat(64), &"g".repeat(64)] {
            assert!(db.admit_discord_bot(hash).is_err());
        }
        for sql in [
            "INSERT INTO discord_bot_auth VALUES(2,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',0)",
            "INSERT INTO discord_bot_auth VALUES(1,'invalid',0)",
            "INSERT INTO discord_bot_auth VALUES(1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',2)",
        ] {
            assert!(db.channel_conn().unwrap().execute_batch(sql).is_err());
        }
    }

    #[test]
    fn discord_success_and_retry_cooldowns_persist_and_never_shorten() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-discord-cooldown-{}", uuid::Uuid::new_v4()));
        let path = directory.join("state.sqlite3");
        let hash = "a".repeat(64);
        let installation = "111111111111111111";
        let mut db = SessionStore::open(&path).unwrap();
        let first = discord_scheduled(&mut db, installation, "222222222222222222");
        let second = discord_scheduled(&mut db, installation, "333333333333333333");
        assert!(db.admit_discord_bot(&hash).unwrap());
        assert_eq!(
            db.claim_channel_delivery(60_002).unwrap().unwrap().id,
            first.id
        );
        assert!(db
            .finish_channel_delivery_with_discord(
                &first.id,
                1,
                "delivered",
                Some("receipt-one".into()),
                None,
                None,
                60_003,
                Some(discord_meta(&hash, Some(90_000), false))
            )
            .unwrap());
        assert_eq!(discord_cooldown(&db, installation), 90_000);
        assert!(!db
            .finish_channel_delivery_with_discord(
                &first.id,
                1,
                "permanent_failed",
                None,
                None,
                None,
                60_004,
                Some(discord_meta(&hash, Some(100_000), true))
            )
            .unwrap());
        assert!(db.admit_discord_bot(&hash).unwrap());
        assert_eq!(discord_cooldown(&db, installation), 90_000);
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        assert!(db.claim_channel_delivery(89_999).unwrap().is_none());
        assert_eq!(
            db.claim_channel_delivery(90_000).unwrap().unwrap().id,
            second.id
        );
        db.finish_channel_delivery_with_discord(
            &second.id,
            1,
            "retry_wait",
            None,
            None,
            Some(100_000),
            90_001,
            Some(discord_meta(&hash, Some(99_000), false)),
        )
        .unwrap();
        assert_eq!(discord_cooldown(&db, installation), 100_000);
        assert!(db.claim_channel_delivery(99_999).unwrap().is_none());
        assert_eq!(
            db.claim_channel_delivery(100_000)
                .unwrap()
                .unwrap()
                .attempts,
            2
        );
        db.finish_channel_delivery_with_discord(
            &second.id,
            2,
            "delivered",
            Some("receipt-two".into()),
            None,
            None,
            100_001,
            Some(discord_meta(&hash, Some(95_000), false)),
        )
        .unwrap();
        assert_eq!(discord_cooldown(&db, installation), 100_300);
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discord_rejected_token_survives_restart_and_late_old_result_cannot_block_rotation() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-discord-auth-{}", uuid::Uuid::new_v4()));
        let path = directory.join("state.sqlite3");
        let (old, new, newest) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let mut db = SessionStore::open(&path).unwrap();
        let first = discord_scheduled(&mut db, "111111111111111111", "222222222222222222");
        let second = discord_scheduled(&mut db, "111111111111111111", "333333333333333333");
        assert!(db.admit_discord_bot(&old).unwrap());
        assert_eq!(
            db.claim_channel_delivery(60_002).unwrap().unwrap().id,
            first.id
        );
        db.finish_channel_delivery_with_discord(
            &first.id,
            1,
            "permanent_failed",
            None,
            Some("discord_unauthorized".into()),
            None,
            60_003,
            Some(discord_meta(&old, None, true)),
        )
        .unwrap();
        assert!(!db.admit_discord_bot(&old).unwrap());
        assert!(
            !db.get_job(first.job_id.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .enabled
        );
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        assert!(!db.admit_discord_bot(&old).unwrap());
        assert!(db.admit_discord_bot(&new).unwrap());
        assert_eq!(
            db.claim_channel_delivery(60_302).unwrap().unwrap().id,
            second.id
        );
        assert!(db.admit_discord_bot(&newest).unwrap());
        db.finish_channel_delivery_with_discord(
            &second.id,
            1,
            "permanent_failed",
            None,
            None,
            None,
            60_303,
            Some(discord_meta(&new, None, true)),
        )
        .unwrap();
        assert!(db.admit_discord_bot(&newest).unwrap());
        assert_eq!(
            db.get_channel_delivery(&first.id).unwrap().unwrap().state,
            "permanent_failed"
        );
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row(
                    "SELECT credential_hash,blocked FROM discord_bot_auth",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
                )
                .unwrap(),
            (newest, false)
        );
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discord_unknown_blocks_other_bot_targets_across_restart_until_operator_review() {
        for (stopped_in_flight, cancel) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let directory = std::env::temp_dir()
                .join(format!("jiaclaw-discord-unknown-{}", uuid::Uuid::new_v4()));
            let path = directory.join("state.sqlite3");
            let installation = "111111111111111111";
            let mut db = SessionStore::open(&path).unwrap();
            let first = discord_scheduled(&mut db, installation, "222222222222222222");
            let waiting = discord_scheduled(&mut db, installation, "333333333333333333");
            assert_eq!(
                db.claim_channel_delivery(60_002).unwrap().unwrap().id,
                first.id
            );
            if !stopped_in_flight {
                db.finish_channel_delivery(&first.id, 1, "unknown", None, None, None, 60_003)
                    .unwrap();
            }
            drop(db);
            let mut db = SessionStore::open(&path).unwrap();
            db.recover_channels(70_000).unwrap();
            assert_eq!(
                db.get_channel_delivery(&first.id).unwrap().unwrap().state,
                "unknown"
            );
            assert!(db.admit_discord_bot(&"b".repeat(64)).unwrap());
            assert!(
                db.claim_channel_delivery(70_000).unwrap().is_none(),
                "changing credentials must not bypass an unknown send"
            );
            assert_eq!(
                db.get_channel_delivery(&waiting.id)
                    .unwrap()
                    .unwrap()
                    .attempts,
                0
            );
            let other = discord_scheduled(&mut db, "444444444444444444", "555555555555555555");
            let claimed_other = db.claim_channel_delivery(70_001).unwrap().unwrap();
            assert_eq!(claimed_other.id, other.id);
            delivered(&mut db, &claimed_other, 70_002);
            let mut inbound = spec("independent-interaction");
            inbound.destination.channel = Channel::Discord;
            inbound.destination.installation_id = installation.into();
            inbound.destination.interaction_id = Some("interaction".into());
            inbound.destination.expires_ms = Some(999_999);
            inbound.sealed_token = Some("sealed-interaction".into());
            let event = complete(&mut db, inbound, &["interaction reply"]);
            let interaction = db.claim_channel_delivery(70_003).unwrap().unwrap();
            assert_eq!(interaction.event_id.as_deref(), Some(event.as_str()));
            delivered(&mut db, &interaction, 70_004);
            assert!(db.claim_channel_delivery(70_303).unwrap().is_none());
            if cancel {
                assert!(db
                    .cancel_job_delivery(
                        first.job_id.as_ref().unwrap(),
                        first.job_run_id.as_ref().unwrap(),
                        70_304
                    )
                    .unwrap());
            } else {
                assert!(db
                    .resolve_channel_delivery(
                        &first.id,
                        "operator verified remote receipt".into(),
                        70_304
                    )
                    .unwrap());
            }
            assert_eq!(
                db.claim_channel_delivery(70_304).unwrap().unwrap().id,
                waiting.id
            );
            assert!(
                !db.get_job(first.job_id.as_ref().unwrap())
                    .unwrap()
                    .unwrap()
                    .enabled,
                "review does not automatically re-enable the original job"
            );
            drop(db);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn discord_metadata_failure_rolls_back_auth_cooldown_and_delivery_together() {
        let mut db = database();
        let hash = "a".repeat(64);
        let installation = "111111111111111111";
        let item = discord_scheduled(&mut db, installation, "222222222222222222");
        assert!(db.admit_discord_bot(&hash).unwrap());
        db.claim_channel_delivery(60_002).unwrap().unwrap();
        db.channel_conn().unwrap().execute_batch("CREATE TRIGGER refuse_discord_finish BEFORE UPDATE OF state ON channel_outbox BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(db
            .finish_channel_delivery_with_discord(
                &item.id,
                1,
                "permanent_failed",
                None,
                None,
                None,
                60_003,
                Some(discord_meta(&hash, Some(90_000), true))
            )
            .is_err());
        assert!(db.admit_discord_bot(&hash).unwrap());
        assert_eq!(discord_cooldown(&db, installation), 60_302);
        assert_eq!(
            db.get_channel_delivery(&item.id).unwrap().unwrap().state,
            "submitting"
        );
        assert!(
            db.get_job(item.job_id.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .enabled
        );
        db.channel_conn().unwrap().execute_batch("DROP TRIGGER refuse_discord_finish; CREATE TRIGGER refuse_discord_auth BEFORE UPDATE OF blocked ON discord_bot_auth BEGIN SELECT RAISE(ABORT,'fixture auth failure'); END;").unwrap();
        assert!(db
            .finish_channel_delivery_with_discord(
                &item.id,
                1,
                "permanent_failed",
                None,
                None,
                None,
                60_003,
                Some(discord_meta(&hash, Some(90_000), true))
            )
            .is_err());
        assert_eq!(discord_cooldown(&db, installation), 60_302);
        assert_eq!(
            db.get_channel_delivery(&item.id).unwrap().unwrap().state,
            "submitting"
        );
        assert!(db.admit_discord_bot(&hash).unwrap());
        db.channel_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_discord_auth;")
            .unwrap();
        db.finish_channel_delivery_with_discord(
            &item.id,
            1,
            "permanent_failed",
            None,
            None,
            None,
            60_004,
            Some(discord_meta(&hash, Some(90_000), true)),
        )
        .unwrap();
        assert!(!db.admit_discord_bot(&hash).unwrap());
        assert_eq!(discord_cooldown(&db, installation), 90_000);

        let event = complete(&mut db, spec("wrong-platform"), &["hello"]);
        let claimed = db.claim_channel_delivery(60_005).unwrap().unwrap();
        assert_eq!(claimed.event_id.as_deref(), Some(event.as_str()));
        assert!(db
            .finish_channel_delivery_with_discord(
                &claimed.id,
                1,
                "delivered",
                Some("receipt".into()),
                None,
                None,
                60_006,
                Some(discord_meta(&hash, Some(100_000), false))
            )
            .is_err());
        assert_eq!(
            db.get_channel_delivery(&claimed.id).unwrap().unwrap().state,
            "submitting"
        );
    }

    #[test]
    fn dingtalk_completion_and_recovery_pacing_persist_with_destination_fifo() {
        let mut db = database();
        let event = |name: &str, member: &str| {
            let mut value = spec(name);
            value.sender_id = member.into();
            value.destination.channel = Channel::Dingtalk;
            value.destination.installation_id = "dingRobot:dingCorp".into();
            value.destination.conversation_id = member.into();
            value
        };
        let first_event = complete(&mut db, event("first", "Alice"), &["first"]);
        let first = db.claim_channel_delivery(0).unwrap().unwrap();
        let second_event = complete(&mut db, event("second", "Alice"), &["second"]);
        assert!(!db
            .finish_channel_delivery(
                &first.id,
                2,
                "delivered",
                Some("stale".into()),
                None,
                None,
                99999
            )
            .unwrap());
        db.finish_channel_delivery(
            &first.id,
            1,
            "delivered",
            Some("accepted".into()),
            None,
            None,
            10000,
        )
        .unwrap();
        assert!(db.claim_channel_delivery(13999).unwrap().is_none());
        let second = db.claim_channel_delivery(14000).unwrap().unwrap();
        assert_eq!(second.event_id.as_deref(), Some(second_event.as_str()));
        let third_event = complete(&mut db, event("third", "Alice"), &["third"]);
        let other_event = complete(&mut db, event("other", "Bob"), &["other"]);
        assert_eq!(db.recover_channels(20000).unwrap().1, 1);
        assert_eq!(
            db.get_channel_delivery(&second.id).unwrap().unwrap().state,
            "unknown"
        );
        assert!(db.claim_channel_delivery(23999).unwrap().is_none());
        let other = db.claim_channel_delivery(24000).unwrap().unwrap();
        assert_eq!(other.event_id.as_deref(), Some(other_event.as_str()));
        assert_eq!(
            db.get_channel_event(&first_event).unwrap().unwrap().status,
            "completed"
        );
        assert!(db
            .list_channel_deliveries(Some(&third_event), 100, 0)
            .unwrap()
            .iter()
            .all(|row| row.attempts == 0));
        db.finish_channel_delivery(
            &other.id,
            1,
            "delivered",
            Some("accepted-other".into()),
            None,
            None,
            24001,
        )
        .unwrap();
        db.resolve_channel_delivery(&second.id, "operator receipt".into(), 25000)
            .unwrap();
        assert!(db.claim_channel_delivery(28000).unwrap().is_none());
        assert_eq!(
            db.claim_channel_delivery(28001)
                .unwrap()
                .unwrap()
                .event_id
                .as_deref(),
            Some(third_event.as_str())
        );
    }

    fn wecom_spec(event: &str, user: &str) -> EventSpec {
        let mut event = spec(event);
        event.sender_id = user.into();
        event.destination.channel = Channel::Wecom;
        event.destination.installation_id = "wwfixture:1".into();
        event.destination.conversation_id = user.into();
        event
    }

    fn wecom_settlement(db: &SessionStore, delivery: &str) -> Option<i64> {
        db.channel_conn()
            .unwrap()
            .query_row(
                "SELECT settled_ms FROM wecom_send_reservations WHERE delivery_id=?1",
                [delivery],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn wecom_completion_anchors_budget_and_pacing_and_stale_finish_cannot_release() {
        let mut db = database();
        let first_event = complete(&mut db, wecom_spec("late", "alice"), &["first"]);
        let first = db.claim_channel_delivery(0).unwrap().unwrap();
        complete(&mut db, wecom_spec("next", "bob"), &["second"]);
        let completion = WECOM_BUDGET_WINDOW_MS * 2 + 100;
        // A network call lasting past the window must retain its reservation.
        assert!(db.claim_channel_delivery(completion).unwrap().is_none());
        assert_eq!(wecom_settlement(&db, &first.id), None);
        assert!(!db
            .finish_channel_delivery(
                &first.id,
                2,
                "delivered",
                Some("stale".into()),
                None,
                None,
                completion
            )
            .unwrap());
        assert_eq!(wecom_settlement(&db, &first.id), None);
        // The outbox result, credit settlement and pacing change are atomic.
        db.channel_conn().unwrap().execute_batch("CREATE TRIGGER refuse_settlement BEFORE UPDATE OF settled_ms ON wecom_send_reservations BEGIN SELECT RAISE(ABORT,'fixture settlement failure'); END;").unwrap();
        assert!(db
            .finish_channel_delivery(
                &first.id,
                1,
                "delivered",
                Some("valid".into()),
                None,
                None,
                completion
            )
            .is_err());
        assert_eq!(
            db.get_channel_delivery(&first.id).unwrap().unwrap().state,
            "submitting"
        );
        assert_eq!(wecom_settlement(&db, &first.id), None);
        db.channel_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_settlement;")
            .unwrap();
        delivered(&mut db, &first, completion);
        assert_eq!(wecom_settlement(&db, &first.id), Some(completion));
        assert!(db.purge_channel_event(&first_event, completion).unwrap());
        assert!(db
            .claim_channel_delivery(completion + 3999)
            .unwrap()
            .is_none());
        let second = db
            .claim_channel_delivery(completion + 4000)
            .unwrap()
            .unwrap();
        assert_eq!(second.destination.conversation_id, "bob");
        db.finish_channel_delivery(
            &second.id,
            1,
            "permanent_failed",
            None,
            Some("rejected".into()),
            None,
            completion + 4001,
        )
        .unwrap();
        assert_eq!(wecom_settlement(&db, &second.id), Some(completion + 4001));
        assert!(db
            .claim_channel_delivery(completion + WECOM_BUDGET_WINDOW_MS - 1)
            .unwrap()
            .is_none());
        assert_eq!(wecom_settlement(&db, &first.id), Some(completion));
        assert!(db
            .claim_channel_delivery(completion + WECOM_BUDGET_WINDOW_MS)
            .unwrap()
            .is_none());
        let retained: i64 = db
            .channel_conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM wecom_send_reservations WHERE delivery_id=?1",
                [&first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 0);
    }

    #[test]
    fn wecom_unknown_restart_blocks_installation_until_explicit_review() {
        for resolve in [false, true] {
            let directory =
                std::env::temp_dir().join(format!("jiaclaw-wecom-review-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("state.sqlite3");
            let mut db = SessionStore::open(&path).unwrap();
            let event = complete(
                &mut db,
                wecom_spec("ambiguous", "alice"),
                &["first", "unsent"],
            );
            let first = db.claim_channel_delivery(0).unwrap().unwrap();
            let next_event = complete(&mut db, wecom_spec("waiting", "bob"), &["second"]);
            drop(db);
            let mut db = SessionStore::open(&path).unwrap();
            let recovery = WECOM_BUDGET_WINDOW_MS * 2;
            assert_eq!(db.recover_channels(recovery).unwrap(), (0, 1));
            assert!(db.claim_channel_delivery(recovery).unwrap().is_none());
            assert_eq!(wecom_settlement(&db, &first.id), None);
            let waiting = db
                .list_channel_deliveries(Some(&next_event), 10, 0)
                .unwrap()
                .remove(0);
            assert_eq!(
                waiting.error.as_deref(),
                Some("wecom_delivery_review_required")
            );
            assert_eq!(waiting.attempts, 0);
            assert!(!db
                .finish_channel_delivery(
                    &first.id,
                    1,
                    "delivered",
                    Some("late worker".into()),
                    None,
                    None,
                    recovery
                )
                .unwrap());
            assert_eq!(wecom_settlement(&db, &first.id), None);
            complete(&mut db, spec("unrelated-installation"), &["ready"]);
            let other = db.claim_channel_delivery(recovery + 1).unwrap().unwrap();
            assert_eq!(other.destination.channel, Channel::Telegram);
            delivered(&mut db, &other, recovery + 2);
            let review = recovery + 100;
            if resolve {
                assert!(db
                    .resolve_channel_delivery(&first.id, "operator checked receipt".into(), review)
                    .unwrap());
            }
            assert!(db.cancel_channel_event(&event, review).unwrap());
            assert!(db.purge_channel_event(&event, review).unwrap());
            assert_eq!(wecom_settlement(&db, &first.id), Some(review));
            let reservations: i64 = db
                .channel_conn()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM wecom_send_reservations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(reservations, 1, "unsent chunks must not consume credit");
            assert!(db.claim_channel_delivery(review + 3999).unwrap().is_none());
            let next = db.claim_channel_delivery(review + 4000).unwrap().unwrap();
            assert_eq!(next.id, waiting.id);
            assert_eq!(next.error, None);
            delivered(&mut db, &next, review + 4001);
            assert_eq!(wecom_settlement(&db, &first.id), Some(review));
            drop(db);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn wecom_scheduled_cancellation_settles_only_submitted_credit() {
        let mut db = database();
        let mut task = scheduled_spec();
        task.delivery = Some(super::super::channel_types::ScheduledDestination {
            channel: Channel::Wecom,
            installation_id: "wwfixture:1".into(),
            conversation_id: "alice".into(),
            thread_id: None,
        });
        let job = db.create_job(task, 0).unwrap();
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        let mut response = scheduled_reply();
        response.message.content = "hello".repeat(500);
        db.finish_job_run(&run.id, None, "completed", Some(response), None, 60_001)
            .unwrap();
        let first = db.claim_channel_delivery(60_002).unwrap().unwrap();
        assert!(db.cancel_job_delivery(&job.id, &run.id, 60_003).is_err());
        db.finish_channel_delivery(&first.id, 1, "unknown", None, None, None, 60_004)
            .unwrap();
        let waiting_event = complete(&mut db, wecom_spec("waiting", "bob"), &["next"]);
        let review = WECOM_BUDGET_WINDOW_MS * 2;
        assert!(db.claim_channel_delivery(review).unwrap().is_none());
        assert_eq!(wecom_settlement(&db, &first.id), None);
        assert!(db.cancel_job_delivery(&job.id, &run.id, review).unwrap());
        assert!(db.purge_job_delivery(&job.id, &run.id).unwrap());
        assert_eq!(wecom_settlement(&db, &first.id), Some(review));
        let reservations: i64 = db
            .channel_conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM wecom_send_reservations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(reservations, 1);
        assert!(db.claim_channel_delivery(review + 3999).unwrap().is_none());
        let next = db.claim_channel_delivery(review + 4000).unwrap().unwrap();
        assert_eq!(next.event_id.as_deref(), Some(waiting_event.as_str()));
    }

    #[test]
    fn wecom_global_budget_holds_unsettled_reservations_without_nullable_deadline_bypass() {
        let mut db = database();
        db.channel_conn().unwrap().execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO wecom_send_reservations(installation_id,delivery_id,attempt,reserved_ms) SELECT 'old:'||x,'delivery:'||x,1,0 FROM n;").unwrap();
        let event = complete(&mut db, wecom_spec("waiting", "bob"), &["next"]);
        assert!(db
            .claim_channel_delivery(WECOM_BUDGET_WINDOW_MS * 2)
            .unwrap()
            .is_none());
        let waiting = db
            .list_channel_deliveries(Some(&event), 10, 0)
            .unwrap()
            .remove(0);
        assert_eq!(waiting.error.as_deref(), Some("wecom_budget_capacity"));
        assert_eq!(waiting.attempts, 0);
        let reservations: i64 = db
            .channel_conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM wecom_send_reservations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(reservations, MAX_WECOM_RESERVATIONS);
    }

    #[test]
    fn wecom_rolling_budget_survives_audit_deletion_and_restart() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-wecom-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let mut db = SessionStore::open(&path).unwrap();
        let mut first_event = String::new();
        for index in 0..200 {
            let mut event = spec(&format!("wecom-{index}"));
            event.sender_id = "alice".into();
            event.destination.channel = Channel::Wecom;
            event.destination.installation_id = "wwfixture:1".into();
            event.destination.conversation_id = "alice".into();
            let id = complete(&mut db, event, &["hello"]);
            if index == 0 {
                first_event = id;
            }
            let now = index * 4100;
            let delivery = db.claim_channel_delivery(now).unwrap().unwrap();
            assert_eq!(delivery.destination.channel, Channel::Wecom);
            delivered(&mut db, &delivery, now + 1);
        }
        assert!(db.purge_channel_event(&first_event, 820_000).unwrap());
        let mut next = spec("waiting-wecom");
        next.sender_id = "bob".into();
        next.destination.channel = Channel::Wecom;
        next.destination.installation_id = "wwfixture:1".into();
        next.destination.conversation_id = "bob".into();
        let queued = complete(&mut db, next, &["waiting"]);
        assert!(db.claim_channel_delivery(820_000).unwrap().is_none());
        let waiting = db
            .list_channel_deliveries(Some(&queued), 10, 0)
            .unwrap()
            .remove(0);
        assert_eq!(waiting.state, "pending");
        assert_eq!(waiting.attempts, 0);
        assert_eq!(waiting.error.as_deref(), Some("wecom_daily_budget"));
        assert_eq!(waiting.next_attempt_ms, WECOM_BUDGET_WINDOW_MS + 1);
        // Exhausting one platform must not block another installation.
        complete(&mut db, spec("telegram-still-ready"), &["other"]);
        let other = db.claim_channel_delivery(820_001).unwrap().unwrap();
        assert_eq!(other.destination.channel, Channel::Telegram);
        delivered(&mut db, &other, 820_002);
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        db.recover_channels(820_003).unwrap();
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM wecom_send_reservations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            200
        );
        assert!(db
            .claim_channel_delivery(WECOM_BUDGET_WINDOW_MS)
            .unwrap()
            .is_none());
        let ready = db
            .claim_channel_delivery(WECOM_BUDGET_WINDOW_MS + 1)
            .unwrap()
            .unwrap();
        assert_eq!(ready.id, waiting.id);
        assert_eq!(ready.attempts, 1);
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM wecom_send_reservations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            200
        );
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn wecom_budget_failure_rolls_back_claim_and_scheduled_text_respects_rendered_bytes() {
        let mut db = database();
        let mut task = scheduled_spec();
        task.delivery = Some(super::super::channel_types::ScheduledDestination {
            channel: Channel::Wecom,
            installation_id: "wwfixture:1".into(),
            conversation_id: "alice@example.com".into(),
            thread_id: None,
        });
        let job = db.create_job(task, 0).unwrap();
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        let mut response = scheduled_reply();
        response.message.content = "中<🙂>".repeat(500);
        let original = response.message.content.clone();
        db.finish_job_run(&run.id, None, "completed", Some(response), None, 60_001)
            .unwrap();
        let parts = db.list_job_deliveries(&job.id, &run.id, 100, 0).unwrap();
        assert!(parts.len() > 1);
        assert_eq!(
            parts
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            original
        );
        assert!(parts
            .iter()
            .all(|part| super::super::outbound::valid_wecom_text(&part.text)));
        db.channel_conn().unwrap().execute_batch("CREATE TRIGGER refuse_budget BEFORE INSERT ON wecom_send_reservations BEGIN SELECT RAISE(ABORT,'fixture budget failure'); END;").unwrap();
        assert!(db.claim_channel_delivery(60_002).is_err());
        let first = db.get_channel_delivery(&parts[0].id).unwrap().unwrap();
        assert_eq!((first.state.as_str(), first.attempts), ("pending", 0));
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM channel_cooldowns", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        db.channel_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_budget;")
            .unwrap();
        let first = db.claim_channel_delivery(60_003).unwrap().unwrap();
        delivered(&mut db, &first, 60_004);
        assert!(db.claim_channel_delivery(64_003).unwrap().is_none());
        assert!(db.claim_channel_delivery(64_004).unwrap().is_some());
    }

    #[test]
    fn inbound_and_scheduled_claims_share_the_same_reply_reservations() {
        let mut db = database();
        for i in 0..97 {
            complete(&mut db, spec(&format!("filled-{i}")), &["chunk"; 100]);
        }
        complete(&mut db, spec("half"), &["chunk"; 50]);
        for _ in 0..3 {
            db.create_job(scheduled_spec(), 0).unwrap();
        }
        let runs = db.claim_due_jobs(60_000, 4).unwrap();
        assert_eq!(runs.len(), 2);
        let inbound = db.accept_channel_event(spec("waiting"), 60_000).unwrap();
        assert!(db.claim_channel_event(60_000).unwrap().is_none());
        db.finish_job_run(
            &runs[0].id,
            None,
            "completed",
            Some(scheduled_reply()),
            None,
            60_001,
        )
        .unwrap();
        assert_eq!(
            db.claim_channel_event(60_001).unwrap().unwrap().id,
            inbound.id
        );
        assert!(db.claim_due_jobs(60_001, 4).unwrap().is_empty());
        db.complete_channel_event(
            &inbound.id,
            None,
            "completed",
            vec!["chunk".into(); 50],
            None,
            60_002,
        )
        .unwrap();
        assert!(db.claim_due_jobs(60_002, 4).unwrap().is_empty());
        db.finish_job_run(
            &runs[1].id,
            None,
            "completed",
            Some(scheduled_reply()),
            None,
            60_003,
        )
        .unwrap();
        assert_eq!(db.claim_due_jobs(60_003, 4).unwrap().len(), 1);
    }

    #[test]
    fn outbox_source_constraints_and_shared_fifo_apply_to_scheduled_runs() {
        let mut db = database();
        let job = db.create_job(scheduled_spec(), 0).unwrap();
        let run = db.claim_due_jobs(60_000, 1).unwrap().remove(0);
        let mut inbound = spec("before-job");
        inbound.destination.installation_id = "123".into();
        inbound.destination.conversation_id = "-100".into();
        let event = complete(&mut db, inbound, &["inbound"]);
        db.finish_job_run(
            &run.id,
            None,
            "completed",
            Some(scheduled_reply()),
            None,
            60_001,
        )
        .unwrap();
        let scheduled = db
            .list_job_deliveries(&job.id, &run.id, 100, 0)
            .unwrap()
            .remove(0);
        assert!(db
            .channel_conn()
            .unwrap()
            .execute(
                "UPDATE channel_outbox SET event_id=?2 WHERE id=?1",
                params![scheduled.id, event]
            )
            .is_err());
        assert!(db
            .channel_conn()
            .unwrap()
            .execute(
                "UPDATE channel_outbox SET job_run_id=NULL WHERE id=?1",
                [&scheduled.id]
            )
            .is_err());
        assert!(db
            .channel_conn()
            .unwrap()
            .execute(
                "UPDATE channel_outbox SET job_run_id='absent' WHERE id=?1",
                [&scheduled.id]
            )
            .is_err());
        let first = db.claim_channel_delivery(60_002).unwrap().unwrap();
        assert_eq!(first.event_id.as_deref(), Some(event.as_str()));
        db.finish_channel_delivery(
            &first.id,
            first.attempts,
            "unknown",
            None,
            None,
            None,
            60_003,
        )
        .unwrap();
        assert!(db.claim_channel_delivery(70_000).unwrap().is_none());
        db.cancel_channel_event(&event, 70_001).unwrap();
        assert_eq!(
            db.claim_channel_delivery(70_002).unwrap().unwrap().id,
            scheduled.id
        );
    }

    #[test]
    fn migrates_v2_transactionally_and_preserves_sessions_and_jobs() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-channels-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let old_job = {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY NOT NULL,messages TEXT NOT NULL CHECK(json_valid(messages)),accessed_ms INTEGER NOT NULL); CREATE TABLE migration_sources(path TEXT PRIMARY KEY NOT NULL); INSERT INTO sessions VALUES('old','[]',0); PRAGMA user_version=1;").unwrap();
            conn.execute_batch(crate::jobs::SCHEMA_V2).unwrap();
            let mut old = SessionStore::Sqlite {
                conn,
                _ownership: None,
            };
            let job = old
                .create_job(
                    crate::jobs::JobSpec {
                        name: "preserved job".into(),
                        prompt: "hello".into(),
                        schedule: crate::schedule::ScheduleSpec::Interval { seconds: 60 },
                        enabled_tools: vec!["datetime_now".into()],
                        timeout_secs: 120,
                        delivery: None,
                    },
                    0,
                )
                .unwrap();
            old.channel_conn().unwrap().execute("INSERT INTO job_runs(id,job_id,scheduled_for_ms,started_ms,status,spec,session_id) VALUES('legacy-run',?1,60000,60000,'running',?2,?3)",params![job.id,serde_json::to_string(&job.spec).unwrap(),job.session_id]).unwrap();
            job.id
        };
        let mut db = SessionStore::open(&path).unwrap();
        assert!(db.get("old").unwrap().is_some());
        assert_eq!(db.list_jobs(10, 0).unwrap()[0].id, old_job);
        assert_eq!(
            db.list_job_runs(&old_job, 10, 0).unwrap()[0].status,
            "running"
        );
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert!(db.accept_channel_event(spec("new"), 0).unwrap().created);
        drop(db);
        let db = SessionStore::open(&path).unwrap();
        assert_eq!(db.list_channel_events(10, 0).unwrap().len(), 1);
        assert!(db.get("old").unwrap().is_some());
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_admission_has_one_identity_and_fingerprint_conflicts_fail_closed() {
        let shared = Arc::new(Mutex::new(database()));
        let barrier = Arc::new(Barrier::new(8));
        let joins: Vec<_> = (0..8)
            .map(|_| {
                let (shared, barrier) = (shared.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    shared
                        .lock()
                        .unwrap()
                        .accept_channel_event(spec("same"), 10)
                        .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.created).count(), 1);
        assert!(results.iter().all(|r| r.id == results[0].id));
        let mut db = shared.lock().unwrap();
        let mut conflicting = spec("same");
        conflicting.fingerprint = "e".repeat(64);
        assert!(db
            .accept_channel_event(conflicting, 20)
            .unwrap_err()
            .is::<ChannelConflict>());
        assert_eq!(
            db.claim_channel_event(30).unwrap().unwrap().id,
            results[0].id
        );
        assert!(db.claim_channel_event(30).unwrap().is_none());
    }

    #[test]
    fn event_claims_are_ordered_session_exclusive_and_globally_bounded() {
        let mut db = database();
        let first = db.accept_channel_event(spec("first"), 0).unwrap().id;
        let blocked = db.accept_channel_event(spec("same-session"), 0).unwrap().id;
        for index in 0..4 {
            let mut next = spec(&format!("independent-{index}"));
            next.session_id = format!("session-{index}");
            db.accept_channel_event(next, 0).unwrap();
        }
        assert_eq!(db.claim_channel_event(1).unwrap().unwrap().id, first);
        for _ in 0..3 {
            assert!(db.claim_channel_event(1).unwrap().is_some());
        }
        assert!(db.claim_channel_event(1).unwrap().is_none());
        assert!(db
            .complete_channel_event(&first, None, "completed", vec![], None, 2)
            .unwrap());
        assert_eq!(db.claim_channel_event(2).unwrap().unwrap().id, blocked);
        assert!(db.claim_channel_event(2).unwrap().is_none());
    }

    #[test]
    fn event_completion_rolls_back_session_and_every_chunk_on_terminal_write_failure() {
        let mut db = database();
        db.insert("channel:room".into(), history("original"))
            .unwrap();
        let id = db.accept_channel_event(spec("atomic"), 0).unwrap().id;
        db.claim_channel_event(0).unwrap().unwrap();
        db.channel_conn().unwrap().execute_batch("CREATE TRIGGER fail_complete BEFORE UPDATE OF status ON channel_events WHEN NEW.status='completed' BEGIN SELECT RAISE(ABORT,'injected event commit failure'); END;").unwrap();
        assert!(db
            .complete_channel_event(
                &id,
                Some(("channel:room".into(), history("new"))),
                "completed",
                vec!["first".into(), "second".into()],
                None,
                5
            )
            .is_err());
        assert_eq!(
            db.get("channel:room").unwrap().unwrap().messages[0].content,
            "original"
        );
        assert_eq!(
            db.get_channel_event(&id).unwrap().unwrap().status,
            "processing"
        );
        assert!(db
            .list_channel_deliveries(Some(&id), 100, 0)
            .unwrap()
            .is_empty());
        db.channel_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_complete;")
            .unwrap();
        assert!(db
            .complete_channel_event(
                &id,
                Some(("wrong-session".into(), history("wrong"))),
                "completed",
                vec![],
                None,
                5
            )
            .is_err());
        assert!(db.get("wrong-session").unwrap().is_none());
        assert!(db
            .complete_channel_event(
                &id,
                Some(("channel:room".into(), history("committed"))),
                "completed",
                vec!["one".into(), "two".into()],
                None,
                5
            )
            .unwrap());
        assert!(!db
            .complete_channel_event(
                &id,
                Some(("channel:room".into(), history("stale"))),
                "completed",
                vec!["duplicate".into()],
                None,
                6
            )
            .unwrap());
        assert_eq!(
            db.get("channel:room").unwrap().unwrap().messages[0].content,
            "committed"
        );
        assert_eq!(
            db.list_channel_deliveries(Some(&id), 100, 0).unwrap().len(),
            2
        );
    }

    #[test]
    fn finish_rolls_back_rate_limit_and_stale_attempts_cannot_overwrite_a_new_claim() {
        let mut db = database();
        complete(&mut db, spec("retry"), &["message"]);
        let first = db.claim_channel_delivery(0).unwrap().unwrap();
        db.channel_conn().unwrap().execute_batch("CREATE TRIGGER fail_delivery BEFORE UPDATE OF state ON channel_outbox WHEN NEW.state='retry_wait' BEGIN SELECT RAISE(ABORT,'injected delivery commit failure'); END;").unwrap();
        assert!(db
            .finish_channel_delivery(
                &first.id,
                1,
                "retry_wait",
                None,
                Some("429".into()),
                Some(10_000),
                1
            )
            .is_err());
        assert_eq!(
            db.get_channel_delivery(&first.id).unwrap().unwrap().state,
            "submitting"
        );
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT until_ms FROM channel_cooldowns", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3100
        );
        db.channel_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_delivery;")
            .unwrap();
        assert!(db
            .finish_channel_delivery(
                &first.id,
                1,
                "retry_wait",
                None,
                Some("429".into()),
                Some(10_000),
                1
            )
            .unwrap());
        assert!(db.claim_channel_delivery(9999).unwrap().is_none());
        let second = db.claim_channel_delivery(10_000).unwrap().unwrap();
        assert_eq!(second.attempts, 2);
        assert!(!db
            .finish_channel_delivery(
                &first.id,
                1,
                "delivered",
                Some("old".into()),
                None,
                None,
                10_001
            )
            .unwrap());
        assert!(!db
            .finish_channel_delivery(&first.id, 1, "retry_wait", None, None, Some(20_000), 10_001)
            .unwrap());
        assert_eq!(
            db.get_channel_delivery(&first.id).unwrap().unwrap().state,
            "submitting"
        );
        delivered(&mut db, &second, 10_001);
        assert!(db
            .get_channel_delivery(&first.id)
            .unwrap()
            .unwrap()
            .receipt
            .unwrap()
            .starts_with("platform:"));
    }

    #[test]
    fn recovery_never_replays_agent_execution_or_an_ambiguous_send() {
        let mut db = database();
        let completed = complete(&mut db, spec("complete"), &["outbound"]);
        let delivery = db.claim_channel_delivery(0).unwrap().unwrap();
        let event = db.accept_channel_event(spec("processing"), 1).unwrap();
        db.claim_channel_event(1).unwrap().unwrap();
        assert_eq!(db.recover_channels(2).unwrap(), (1, 1));
        assert_eq!(
            db.get_channel_event(&event.id).unwrap().unwrap().status,
            "needs_review"
        );
        assert_eq!(
            db.get_channel_event(&completed).unwrap().unwrap().status,
            "completed"
        );
        assert_eq!(
            db.get_channel_delivery(&delivery.id)
                .unwrap()
                .unwrap()
                .state,
            "unknown"
        );
        assert!(db.claim_channel_event(10_000).unwrap().is_none());
        assert!(db.claim_channel_delivery(10_000).unwrap().is_none());
        assert_eq!(db.recover_channels(10_000).unwrap(), (0, 0));
    }

    #[test]
    fn fifo_blocks_ambiguous_destination_but_allows_other_destinations_and_manual_receipts() {
        let mut db = database();
        let first = complete(&mut db, spec("first"), &["part1", "part2"]);
        let later = complete(&mut db, spec("later"), &["later"]);
        let mut other = spec("other");
        other.destination.conversation_id = "elsewhere".into();
        let elsewhere = complete(&mut db, other, &["elsewhere"]);
        let a = db.claim_channel_delivery(0).unwrap().unwrap();
        assert_eq!(
            (a.event_id.as_deref().unwrap(), a.ordinal),
            (first.as_str(), 0)
        );
        assert!(db.claim_channel_delivery(5000).unwrap().is_none()); // installation is in flight
        assert!(db
            .cancel_channel_event(&first, 5000)
            .unwrap_err()
            .is::<ChannelConflict>());
        assert!(db
            .finish_channel_delivery(
                &a.id,
                a.attempts,
                "unknown",
                None,
                Some("timeout after submit".into()),
                None,
                1
            )
            .unwrap());
        let c = db.claim_channel_delivery(3100).unwrap().unwrap();
        assert_eq!(c.event_id, Some(elsewhere));
        delivered(&mut db, &c, 3101);
        assert!(db.claim_channel_delivery(6200).unwrap().is_none());
        assert!(db
            .resolve_channel_delivery(&a.id, " ".into(), 6200)
            .is_err());
        assert!(db
            .resolve_channel_delivery(&a.id, "admin checked message 123".into(), 6200)
            .unwrap());
        assert_eq!(
            db.get_channel_delivery(&a.id)
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some("timeout after submit")
        );
        let second = db.claim_channel_delivery(6200).unwrap().unwrap();
        assert_eq!(
            (second.event_id.as_deref().unwrap(), second.ordinal),
            (first.as_str(), 1)
        );
        delivered(&mut db, &second, 6201);
        let b = db.claim_channel_delivery(9300).unwrap().unwrap();
        assert_eq!(b.event_id, Some(later));
    }

    #[test]
    fn explicit_cancellation_preserves_delivered_receipts_and_releases_future_events() {
        let mut db = database();
        let first = complete(&mut db, spec("first"), &["sent", "uncertain", "remaining"]);
        let second = complete(&mut db, spec("second"), &["future"]);
        let sent = db.claim_channel_delivery(0).unwrap().unwrap();
        delivered(&mut db, &sent, 1);
        let uncertain = db.claim_channel_delivery(3100).unwrap().unwrap();
        db.finish_channel_delivery(
            &uncertain.id,
            uncertain.attempts,
            "permanent_failed",
            None,
            Some("platform refused".into()),
            None,
            3101,
        )
        .unwrap();
        assert!(db.claim_channel_delivery(6200).unwrap().is_none());
        assert!(db
            .purge_channel_event(&first, 6200)
            .unwrap_err()
            .is::<ChannelConflict>());
        assert!(db.cancel_channel_event(&first, 6200).unwrap());
        let all = db.list_channel_deliveries(Some(&first), 100, 0).unwrap();
        assert_eq!(
            all.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(),
            vec!["delivered", "cancelled", "cancelled"]
        );
        assert!(all[0].receipt.is_some());
        assert_eq!(all[1].error.as_deref(), Some("platform refused"));
        assert_eq!(
            db.claim_channel_delivery(6200).unwrap().unwrap().event_id,
            Some(second)
        );
        assert!(db.purge_channel_event(&first, 6200).unwrap());
    }

    #[test]
    fn pacing_and_429_cooldowns_apply_to_whole_installation_and_retry_budget_is_finite() {
        let mut db = database();
        complete(&mut db, spec("limited"), &["reply"]);
        let mut other = spec("other");
        other.destination.conversation_id = "other-room".into();
        complete(&mut db, other, &["unrelated"]);
        for attempt in 1..=5 {
            let now = (i64::from(attempt) - 1) * 10_000;
            let item = db.claim_channel_delivery(now).unwrap().unwrap();
            assert_eq!(item.attempts, attempt);
            db.finish_channel_delivery(
                &item.id,
                attempt,
                "retry_wait",
                None,
                Some("429".into()),
                Some(now + 10_000),
                now + 1,
            )
            .unwrap();
            assert!(db.claim_channel_delivery(now + 9999).unwrap().is_none());
            let updated = db.get_channel_delivery(&item.id).unwrap().unwrap();
            assert_eq!(
                updated.state,
                if attempt == 5 {
                    "permanent_failed"
                } else {
                    "retry_wait"
                }
            );
        }
        let other = db.claim_channel_delivery(50_000).unwrap().unwrap();
        assert_eq!(other.destination.conversation_id, "other-room");
        delivered(&mut db, &other, 50_001);
        assert!(db.claim_channel_delivery(53_100).unwrap().is_none());
    }

    #[test]
    fn discord_sealed_tokens_never_serialize_and_expiry_clears_every_database_copy() {
        let mut db = database();
        let mut discord = spec("interaction");
        discord.destination.channel = Channel::Discord;
        discord.destination.interaction_id = Some("interaction".into());
        discord.destination.expires_ms = Some(1000);
        discord.sealed_token = Some("sealed-secret-sentinel".into());
        let id = complete(&mut db, discord, &["one", "two"]);
        let event = db.get_channel_event(&id).unwrap().unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("sealed_token") && !json.contains("sealed-secret-sentinel"));
        assert!(!format!("{event:?}").contains("sealed-secret-sentinel"));
        let stored: String = db
            .channel_conn()
            .unwrap()
            .query_row("SELECT spec FROM channel_events WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!stored.contains("sealed-secret-sentinel"));
        let claimed = db.claim_channel_delivery(0).unwrap().unwrap();
        assert_eq!(
            claimed.sealed_token.as_deref(),
            Some("sealed-secret-sentinel")
        );
        let json = serde_json::to_string(&claimed).unwrap();
        assert!(!json.contains("sealed_token") && !json.contains("sealed-secret-sentinel"));
        assert!(!format!("{claimed:?}").contains("sealed-secret-sentinel"));
        assert_eq!(db.recover_channels(1000).unwrap(), (0, 1));
        assert!(db
            .get_channel_event(&id)
            .unwrap()
            .unwrap()
            .spec
            .sealed_token
            .is_none());
        let deliveries = db.list_channel_deliveries(Some(&id), 100, 0).unwrap();
        assert!(deliveries.iter().all(|d| d.sealed_token.is_none()));
        assert_eq!(
            deliveries
                .iter()
                .map(|d| d.state.as_str())
                .collect::<Vec<_>>(),
            vec!["unknown", "expired"]
        );
    }

    #[test]
    fn explicit_purge_is_atomic_and_dedup_tombstone_lives_seven_days_after_purge() {
        let mut db = database();
        let id = complete(&mut db, spec("retained"), &["reply"]);
        let d = db.claim_channel_delivery(0).unwrap().unwrap();
        delivered(&mut db, &d, 1);
        db.channel_conn().unwrap().execute_batch("CREATE TRIGGER prevent_purge BEFORE DELETE ON channel_events BEGIN SELECT RAISE(ABORT,'purge failure'); END;").unwrap();
        assert!(db.purge_channel_event(&id, 100).is_err());
        assert!(db.get_channel_event(&id).unwrap().is_some());
        assert!(db.get_channel_delivery(&d.id).unwrap().is_some());
        db.channel_conn()
            .unwrap()
            .execute_batch("DROP TRIGGER prevent_purge;")
            .unwrap();
        assert!(db.purge_channel_event(&id, 100).unwrap());
        assert!(db.get_channel_event(&id).unwrap().is_none());
        assert!(db.get_channel_delivery(&d.id).unwrap().is_none());
        let duplicate = db
            .accept_channel_event(spec("retained"), RETENTION_MS + 99)
            .unwrap();
        assert!(!duplicate.created && duplicate.status == "purged");
        let mut conflict = spec("retained");
        conflict.fingerprint = "a".repeat(64);
        assert!(db
            .accept_channel_event(conflict, RETENTION_MS + 99)
            .unwrap_err()
            .is::<ChannelConflict>());
        assert!(
            db.accept_channel_event(spec("retained"), RETENTION_MS + 100)
                .unwrap()
                .created
        );
        assert!(
            !db.accept_channel_event(spec("retained"), RETENTION_MS * 3)
                .unwrap()
                .created
        );
    }

    #[test]
    fn event_and_tombstone_quotas_keep_evidence_until_explicit_purge_or_retention_expiry() {
        let mut db = database();
        let mut ids = Vec::new();
        for i in 0..MAX_EVENTS {
            ids.push(
                db.accept_channel_event(spec(&format!("event-{i}")), 0)
                    .unwrap()
                    .id,
            );
        }
        assert!(db
            .accept_channel_event(spec("extra"), 1)
            .unwrap_err()
            .is::<ChannelCapacity>());
        assert!(!db.accept_channel_event(spec("event-0"), 1).unwrap().created);
        assert!(db.cancel_channel_event(&ids[0], 1).unwrap());
        assert!(db.purge_channel_event(&ids[0], 1).unwrap());
        assert!(
            db.accept_channel_event(spec("replacement"), 2)
                .unwrap()
                .created
        );
        let mut tombstones = database();
        tombstones.channel_conn().unwrap().execute("WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<10000) INSERT INTO channel_dedup(channel,installation_id,event_id,id,fingerprint,retain_until_ms) SELECT 'telegram','installation','old-'||n,'id-'||n,?1,100 FROM numbers",["f".repeat(64)]).unwrap();
        assert!(tombstones
            .accept_channel_event(spec("new"), 99)
            .unwrap_err()
            .is::<ChannelCapacity>());
        assert!(
            tombstones
                .accept_channel_event(spec("new"), 100)
                .unwrap()
                .created
        );
        assert_eq!(
            tombstones
                .channel_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_dedup", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn full_outbox_rejects_admission_and_waits_for_explicit_purge_before_claiming() {
        let mut db = database();
        for i in 0..99 {
            complete(&mut db, spec(&format!("event-{i}")), &["chunk"; 100]);
        }
        db.insert("channel:room".into(), history("original"))
            .unwrap();
        let final_event = db.accept_channel_event(spec("last-reply"), 1).unwrap();
        let waiting = db.accept_channel_event(spec("waiting"), 1).unwrap();
        assert_eq!(
            db.claim_channel_event(1).unwrap().unwrap().id,
            final_event.id
        );
        assert!(db
            .complete_channel_event(
                &final_event.id,
                None,
                "completed",
                vec!["chunk".into(); 100],
                None,
                2
            )
            .unwrap());
        assert!(db
            .accept_channel_event(spec("overflow"), 2)
            .unwrap_err()
            .is::<ChannelCapacity>());
        assert!(!db.accept_channel_event(spec("waiting"), 2).unwrap().created);
        assert!(
            !db.accept_channel_event(spec("last-reply"), 2)
                .unwrap()
                .created
        );
        assert!(db.claim_channel_event(2).unwrap().is_none());
        assert_eq!(
            db.get("channel:room").unwrap().unwrap().messages[0].content,
            "original"
        );
        assert_eq!(
            db.get_channel_event(&waiting.id).unwrap().unwrap().status,
            "received"
        );
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            10_000
        );
        assert!(db.cancel_channel_event(&final_event.id, 3).unwrap());
        assert!(db.purge_channel_event(&final_event.id, 3).unwrap());
        assert_eq!(db.claim_channel_event(3).unwrap().unwrap().id, waiting.id);
        assert!(db
            .complete_channel_event(
                &waiting.id,
                None,
                "completed",
                vec!["reply".into()],
                None,
                4
            )
            .unwrap());
    }

    #[test]
    fn concurrent_claims_reserve_reply_capacity_and_completion_releases_unused_slots() {
        let mut db = database();
        for i in 0..97 {
            complete(&mut db, spec(&format!("filled-{i}")), &["chunk"; 100]);
        }
        complete(&mut db, spec("half-filled"), &["chunk"; 50]);
        for i in 0..4 {
            let mut input = spec(&format!("queued-{i}"));
            input.session_id = format!("independent-{i}");
            db.accept_channel_event(input, 1).unwrap();
        }
        let shared = Arc::new(Mutex::new(db));
        let barrier = Arc::new(Barrier::new(4));
        let joins: Vec<_> = (0..4)
            .map(|_| {
                let (shared, barrier) = (shared.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    shared.lock().unwrap().claim_channel_event(2).unwrap()
                })
            })
            .collect();
        let claims: Vec<_> = joins
            .into_iter()
            .filter_map(|join| join.join().unwrap())
            .collect();
        assert_eq!(
            claims.len(),
            2,
            "9750 rows permit exactly two 100-row reservations"
        );
        let mut db = shared.lock().unwrap();
        assert!(db.claim_channel_event(2).unwrap().is_none());
        // Fifty emitted chunks free exactly the fifty extra slots needed by the
        // next full reservation; existing processing work keeps its reservation.
        db.complete_channel_event(
            &claims[0].id,
            None,
            "completed",
            vec!["chunk".into(); 50],
            None,
            3,
        )
        .unwrap();
        let third = db.claim_channel_event(3).unwrap().unwrap();
        assert!(db.claim_channel_event(3).unwrap().is_none());
        db.complete_channel_event(
            &claims[1].id,
            None,
            "completed",
            vec!["chunk".into(); 100],
            None,
            4,
        )
        .unwrap();
        assert!(
            db.claim_channel_event(4).unwrap().is_none(),
            "a full reply releases no reserved capacity"
        );
        db.complete_channel_event(&third.id, None, "completed", vec![], None, 5)
            .unwrap();
        let fourth = db.claim_channel_event(5).unwrap().unwrap();
        db.complete_channel_event(
            &fourth.id,
            None,
            "completed",
            vec!["chunk".into(); 100],
            None,
            6,
        )
        .unwrap();
        assert_eq!(
            db.channel_conn()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            10_000
        );
        assert!(db.claim_channel_event(6).unwrap().is_none());
    }

    #[test]
    fn inputs_receipts_chunks_and_pagination_are_bounded() {
        let mut db = database();
        let mut input = spec("invalid");
        input.enabled_tools.clear();
        assert!(db.accept_channel_event(input, 0).is_err());
        let mut input = spec("invalid");
        input.enabled_tools.push("datetime_now".into());
        assert!(db.accept_channel_event(input, 0).is_err());
        let mut input = spec("invalid");
        input.prompt = "x".repeat(32769);
        assert!(db.accept_channel_event(input, 0).is_err());
        let mut input = spec("invalid");
        input.sealed_token = Some("not-discord".into());
        assert!(db.accept_channel_event(input, 0).is_err());
        let mut input = spec("invalid");
        input.fingerprint = "invalid".into();
        assert!(db.accept_channel_event(input, 0).is_err());
        let event = db.accept_channel_event(spec("bounded"), 0).unwrap();
        db.claim_channel_event(0).unwrap().unwrap();
        for chunks in [
            vec![String::new()],
            vec!["x".repeat(4097)],
            vec!["x".into(); 101],
            vec!["x".repeat(4096); 100],
        ] {
            assert!(db
                .complete_channel_event(&event.id, None, "completed", chunks, None, 1)
                .is_err());
        }
        assert!(db
            .complete_channel_event(
                &event.id,
                None,
                "completed",
                vec!["界".repeat(4096)],
                Some("界".repeat(4096)),
                1
            )
            .unwrap());
        assert!(
            db.get_channel_event(&event.id)
                .unwrap()
                .unwrap()
                .error
                .unwrap()
                .len()
                <= 4096
        );
        let delivery = db.claim_channel_delivery(1).unwrap().unwrap();
        assert!(db
            .finish_channel_delivery(&delivery.id, 1, "delivered", None, None, None, 2)
            .is_err());
        assert!(db
            .finish_channel_delivery(
                &delivery.id,
                1,
                "delivered",
                Some("x".repeat(4097)),
                None,
                None,
                2
            )
            .is_err());
        assert!(db
            .finish_channel_delivery(&delivery.id, 1, "retry_wait", None, None, Some(2), 2)
            .is_err());
        assert!(db.list_channel_events(0, 0).is_err());
        assert!(db.list_channel_events(101, 0).is_err());
        assert!(db.list_channel_deliveries(None, 1, usize::MAX).is_err());
    }

    #[test]
    fn ephemeral_backend_cannot_admit_execute_send_or_reconcile_durable_channels() {
        let mut db = SessionStore::memory();
        assert!(db.accept_channel_event(spec("event"), 0).is_err());
        assert!(db.get_channel_event("unknown").is_err());
        assert!(db.list_channel_events(1, 0).is_err());
        assert!(db.claim_channel_event(0).is_err());
        assert!(db
            .complete_channel_event("unknown", None, "completed", vec![], None, 0)
            .is_err());
        assert!(db.get_channel_delivery("unknown").is_err());
        assert!(db.list_channel_deliveries(None, 1, 0).is_err());
        assert!(db.claim_channel_delivery(0).is_err());
        assert!(db
            .finish_channel_delivery("unknown", 1, "unknown", None, None, None, 0)
            .is_err());
        assert!(db.recover_channels(0).is_err());
        assert!(db
            .resolve_channel_delivery("unknown", "receipt".into(), 0)
            .is_err());
        assert!(db.cancel_channel_event("unknown", 0).is_err());
        assert!(db.purge_channel_event("unknown", 0).is_err());
    }

    #[test]
    fn per_platform_pacing_survives_restart_and_never_shortens_platform_cooldown() {
        let directory =
            std::env::temp_dir().join(format!("jiaclaw-pacing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.sqlite3");
        let mut db = SessionStore::open(&path).unwrap();
        for channel in [Channel::Telegram, Channel::Slack, Channel::Discord] {
            let mut input = spec(channel_name(channel));
            input.destination.channel = channel;
            if channel == Channel::Discord {
                input.destination.interaction_id = Some("interaction".into());
                input.destination.expires_ms = Some(100_000);
                input.sealed_token = Some("sealed-credential".into());
            }
            complete(&mut db, input, &["first", "second"]);
        }
        for expected in [Channel::Telegram, Channel::Slack, Channel::Discord] {
            let first = db.claim_channel_delivery(0).unwrap().unwrap();
            assert_eq!(first.destination.channel, expected);
            delivered(&mut db, &first, 1);
        }
        drop(db);
        let mut db = SessionStore::open(&path).unwrap();
        db.recover_channels(2).unwrap();
        assert!(db.claim_channel_delivery(299).unwrap().is_none());
        let discord = db.claim_channel_delivery(300).unwrap().unwrap();
        assert_eq!(discord.destination.channel, Channel::Discord);
        delivered(&mut db, &discord, 301);
        assert!(db.claim_channel_delivery(1099).unwrap().is_none());
        let slack = db.claim_channel_delivery(1100).unwrap().unwrap();
        assert_eq!(slack.destination.channel, Channel::Slack);
        delivered(&mut db, &slack, 1101);
        assert!(db.claim_channel_delivery(3099).unwrap().is_none());
        let telegram = db.claim_channel_delivery(3100).unwrap().unwrap();
        assert_eq!(telegram.destination.channel, Channel::Telegram);
        // A provider's shorter Retry-After cannot erase the conservative baseline.
        db.finish_channel_delivery(
            &telegram.id,
            telegram.attempts,
            "retry_wait",
            None,
            None,
            Some(3200),
            3101,
        )
        .unwrap();
        assert!(db.claim_channel_delivery(6199).unwrap().is_none());
        assert_eq!(
            db.claim_channel_delivery(6200).unwrap().unwrap().id,
            telegram.id
        );
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ttl_retains_queued_processing_and_unreviewed_channel_context() {
        let mut db = database();
        db.insert("unrelated".into(), history("expired")).unwrap();
        db.insert("channel:room".into(), history("conversation"))
            .unwrap();
        let event = db.accept_channel_event(spec("waiting"), 0).unwrap();
        // Age is measured by SQLite wall time; zero TTL covers both records.
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 1);
        assert!(db.get("channel:room").unwrap().is_some());
        db.claim_channel_event(0).unwrap().unwrap();
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 0);
        db.complete_channel_event(&event.id, None, "completed", vec!["reply".into()], None, 0)
            .unwrap();
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 0);
        let delivery = db.claim_channel_delivery(0).unwrap().unwrap();
        db.finish_channel_delivery(
            &delivery.id,
            delivery.attempts,
            "unknown",
            None,
            None,
            None,
            1,
        )
        .unwrap();
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 0);
        db.cancel_channel_event(&event.id, 2).unwrap();
        assert_eq!(db.purge(std::time::Duration::ZERO, &[]).unwrap(), 1);
        assert!(db.get_channel_event(&event.id).unwrap().is_some());
        assert_eq!(
            db.get_channel_delivery(&delivery.id)
                .unwrap()
                .unwrap()
                .state,
            "cancelled"
        );
    }
}
