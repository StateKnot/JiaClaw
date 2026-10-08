// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Administrator-owned identity registry and durable uncertain-write admission.
use super::keys;
pub use super::keys::IssuedKey;
use anyhow::{ensure, Context, Result};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Serialize, Serializer};
use std::{
    fs::{self, OpenOptions as FileOptions},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const APPLICATION_ID: i32 = 0x4a43_4757;
const MAX_USERS: i64 = 32;
const MAX_ACTIVE_KEYS: i64 = 8;
const MAX_KEYS: i64 = 1024;
const MAX_AUDIT_EVENTS: i64 = 4096;
const MAX_TELEGRAM_BINDINGS: i64 = 32;
const MAX_SLACK_BINDINGS: i64 = 32;
const MAX_DISCORD_BINDINGS: i64 = 32;
const MAX_FEISHU_BINDINGS: i64 = 32;
const MAX_WECOM_BINDINGS: i64 = 32;
const SCHEMA_VERSION: i64 = 7;

/// Authenticated identity. Backend selection is never taken from client metadata.
#[derive(Clone, Debug)]
pub struct Principal {
    pub user_id: Uuid,
    pub key_id: Uuid,
    pub backend_id: String,
    pub read_only: bool,
}

/// User-owned cron identity; independent of any particular API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledUser {
    pub user_id: Uuid,
    pub backend_id: String,
}

/// Permanent administrator binding. Revocation never frees its user or bot identity.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TelegramBindingSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub id: Uuid,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub backend_id: String,
    pub bot_id: String,
    pub sender_id: String,
    pub enabled: bool,
}

/// Permanent dedicated Slack installation and private direct-message ownership.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SlackBindingSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub id: Uuid,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub backend_id: String,
    pub team_id: String,
    pub app_id: String,
    pub bot_user_id: String,
    pub bot_id: String,
    pub sender_id: String,
    pub conversation_id: String,
    pub enabled: bool,
}

/// Permanent dedicated user-installable Discord application and private bot-DM ownership.
/// The verification key is a public pin, never an administrator credential.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DiscordBindingSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub id: Uuid,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub backend_id: String,
    pub application_id: String,
    pub verify_key: String,
    pub bot_user_id: String,
    pub sender_id: String,
    pub conversation_id: String,
    pub command_id: String,
    pub enabled: bool,
}

/// Permanent dedicated Feishu enterprise application and private-chat ownership.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FeishuBindingSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub id: Uuid,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub backend_id: String,
    pub app_id: String,
    pub tenant_key: String,
    pub bot_open_id: String,
    pub human_open_id: String,
    pub chat_id: String,
    pub enabled: bool,
}

/// Permanent dedicated WeCom enterprise application and canonical private-member ownership.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct WecomBindingSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub id: Uuid,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub backend_id: String,
    pub corp_id: String,
    pub agent_id: u32,
    pub human_user_id: String,
    pub enabled: bool,
}

/// A credential has ceased to authorize admission, or an earlier write needs resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAdmissionError {
    Unauthorized,
    ReadOnly,
    Held,
}
impl std::fmt::Display for WriteAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unauthorized => "gateway credential is no longer authorized",
            Self::ReadOnly => "gateway credential is read-only",
            Self::Held => "gateway user has an unresolved write",
        })
    }
}
impl std::error::Error for WriteAdmissionError {}

/// A write admitted before a backend outcome was known.
#[derive(Debug, Serialize)]
pub struct WriteHoldSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub request_id: Uuid,
    pub state: String,
    pub reason: String,
    pub admitted_ms: i64,
    pub updated_ms: i64,
}

/// Safe administrative output, containing no credential or verifier.
#[derive(Debug, Serialize)]
pub struct UserSummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub backend_id: String,
    pub enabled: bool,
    pub active_keys: u32,
    pub revoked_keys: u32,
    pub hold: Option<WriteHoldSummary>,
}

/// Administrative key metadata. Tokens and verifier bytes are never exposed.
#[derive(Debug, Serialize)]
pub struct KeySummary {
    #[serde(serialize_with = "serialize_uuid")]
    pub key_id: Uuid,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub read_only: bool,
    pub created_ms: i64,
    pub revoked_ms: Option<i64>,
}

/// Bounded administrator audit metadata. Sequence values remain exact in JSON.
#[derive(Debug, Serialize)]
pub struct AuditEventSummary {
    pub seq: String,
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub key_id: Option<String>,
    pub request_id: Option<String>,
    pub action: String,
    pub created_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One consistent read snapshot of retained audit history, never an archive.
#[derive(Debug, Serialize)]
pub struct AuditPage {
    #[serde(serialize_with = "serialize_uuid")]
    pub user_id: Uuid,
    pub events: Vec<AuditEventSummary>,
    pub next_after_seq: String,
    pub has_more: bool,
    pub oldest_retained_seq: Option<String>,
    pub latest_seq: String,
    pub retention_gap: bool,
    pub notes_included: bool,
}

fn serialize_uuid<S: Serializer>(id: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&id.to_string())
}

/// Cloneable path handle. Every operation uses its own short-lived `SQLite` connection.
#[derive(Clone)]
pub struct Registry {
    path: PathBuf,
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

fn uuid(value: String) -> Result<Uuid> {
    Uuid::parse_str(&value).context("invalid gateway registry identity")
}

fn private_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "gateway registry files must be regular files, not symlinks"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.permissions().mode() & 0o077 == 0 && metadata.nlink() == 1,
            "gateway registry files require private permissions and one hard link"
        );
    }
    Ok(())
}

impl Registry {
    /// Initialize or validate a private registry. Existing directories are never chmodded.
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute()
                && !path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir)),
            "gateway registry path must be absolute without parent traversal"
        );
        let parent = path
            .parent()
            .context("gateway registry requires a parent directory")?;
        if !parent.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent)?;
        }
        let metadata = fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "gateway registry parent must be a real private directory"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "gateway registry parent requires private permissions"
            );
        }
        let path = parent.canonicalize()?.join(
            path.file_name()
                .context("gateway registry requires a filename")?,
        );
        let mut options = FileOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        let registry = Self { path };
        let mut conn = registry.raw_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let application: i32 = tx.pragma_query_value(None, "application_id", |row| row.get(0))?;
        if version == 0 && application == 0 {
            let tables: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            ensure!(
                tables == 0,
                "refusing to adopt an unrelated gateway registry database"
            );
            tx.execute_batch(SCHEMA)?;
            tx.execute_batch(SCHEMA_V2)?;
            tx.execute_batch(SCHEMA_V3)?;
            tx.execute_batch(SCHEMA_V4)?;
            tx.execute_batch(SCHEMA_V5)?;
            tx.execute_batch(SCHEMA_V6)?;
            tx.execute_batch(SCHEMA_V7)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else {
            ensure!(
                (1..=SCHEMA_VERSION).contains(&version) && application == APPLICATION_ID,
                "unsupported gateway registry schema or database identity"
            );
            if version == 1 {
                tx.execute_batch(SCHEMA_V2)?;
            }
            if version < 3 {
                tx.execute_batch(SCHEMA_V3)?;
            }
            if version < 4 {
                tx.execute_batch(SCHEMA_V4)?;
            }
            if version < 5 {
                tx.execute_batch(SCHEMA_V5)?;
            }
            if version < 6 {
                tx.execute_batch(SCHEMA_V6)?;
            }
            if version < 7 {
                tx.execute_batch(SCHEMA_V7)?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        let corrupt = tx
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some();
        ensure!(
            !corrupt,
            "gateway registry foreign key integrity check failed"
        );
        tx.commit()?;
        // Establish identity under the initialization write lock before changing a
        // persistent journal setting. Concurrent initializers recheck in that transaction.
        // A failed WAL transition leaves a valid registry that can be reopened safely.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(registry)
    }

    fn raw_connection(&self) -> Result<Connection> {
        private_file(&self.path)?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", self.path.display()));
            match private_file(&sidecar) {
                Ok(()) => (),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_millis(250))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;",
        )?;
        Ok(conn)
    }

    fn connection(&self) -> Result<Connection> {
        let conn = self.raw_connection()?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let application: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
        ensure!(
            version == SCHEMA_VERSION && application == APPLICATION_ID,
            "unsupported gateway registry schema or database identity"
        );
        Ok(conn)
    }

    /// Provision an immutable user-to-backend binding and its first key atomically.
    pub fn add_user(&self, backend_id: &str) -> Result<IssuedKey> {
        self.add_user_with_access(backend_id, false)
    }

    /// Provision a user and its first key with administrator-selected immutable access.
    pub fn add_user_with_access(&self, backend_id: &str, read_only: bool) -> Result<IssuedKey> {
        ensure!(
            !backend_id.is_empty()
                && backend_id.len() <= 64
                && backend_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
            "gateway backend ID must contain 1..64 ASCII letters, digits, underscores or hyphens"
        );
        let user_id = Uuid::new_v4();
        let (issued, verifier) = keys::issue(user_id, read_only)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM users", [], |row| row.get(0))?;
        ensure!(count < MAX_USERS, "gateway user limit reached");
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE backend_id=?1)",
            [backend_id],
            |row| row.get(0),
        )?;
        ensure!(!exists, "gateway backend already belongs to a user");
        let now = now_ms();
        tx.execute(
            "INSERT INTO users(id,backend_id,enabled,created_ms,updated_ms) VALUES(?1,?2,1,?3,?3)",
            params![user_id.to_string(), backend_id, now],
        )?;
        insert_key(&tx, &issued, &verifier, now)?;
        audit(
            &tx,
            user_id,
            Some(issued.key_id),
            None,
            "user_added",
            Some(access_note(read_only)),
            now,
        )?;
        tx.commit()?;
        Ok(issued)
    }

    /// Issue another key for an enabled user, up to eight active keys.
    pub fn add_key(&self, user_id: Uuid) -> Result<IssuedKey> {
        self.add_key_with_access(user_id, false)
    }

    /// Issue an additional key with administrator-selected immutable access.
    pub fn add_key_with_access(&self, user_id: Uuid, read_only: bool) -> Result<IssuedKey> {
        let (issued, verifier) = keys::issue(user_id, read_only)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        enabled_user(&tx, user_id)?;
        let now = now_ms();
        insert_key(&tx, &issued, &verifier, now)?;
        audit(
            &tx,
            user_id,
            Some(issued.key_id),
            None,
            "key_added",
            Some(access_note(read_only)),
            now,
        )?;
        tx.commit()?;
        Ok(issued)
    }

    /// Replace one active key atomically; the old key cannot authorize new requests after commit.
    pub fn rotate(&self, key_id: Uuid) -> Result<IssuedKey> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key: Option<(String, bool)> = tx
            .query_row(
                "SELECT user_id,read_only FROM api_keys WHERE id=?1 AND revoked_ms IS NULL",
                [key_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (user, read_only) = key.context("active gateway key not found")?;
        let user_id = uuid(user)?;
        enabled_user(&tx, user_id)?;
        let (issued, verifier) = keys::issue(user_id, read_only)?;
        let now = now_ms();
        tx.execute(
            "UPDATE api_keys SET revoked_ms=MAX(created_ms,?2) WHERE id=?1",
            params![key_id.to_string(), now],
        )?;
        insert_key(&tx, &issued, &verifier, now)?;
        audit(
            &tx,
            user_id,
            Some(key_id),
            None,
            "key_revoked_by_rotation",
            Some(access_note(read_only)),
            now,
        )?;
        audit(
            &tx,
            user_id,
            Some(issued.key_id),
            None,
            "key_rotated",
            Some(access_note(read_only)),
            now,
        )?;
        tx.commit()?;
        Ok(issued)
    }

    /// List bounded key metadata, including revoked history, for one existing user.
    pub fn list_keys(&self, user_id: Uuid, limit: usize, offset: usize) -> Result<Vec<KeySummary>> {
        ensure!(
            (1..=100).contains(&limit) && offset <= 1024,
            "invalid key pagination"
        );
        let conn = self.connection()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
            [user_id.to_string()],
            |row| row.get(0),
        )?;
        ensure!(exists, "gateway user not found");
        let mut statement = conn.prepare(
            "SELECT id,user_id,read_only,created_ms,revoked_ms FROM api_keys
             WHERE user_id=?1 ORDER BY created_ms,id LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(
            params![
                user_id.to_string(),
                i64::try_from(limit)?,
                i64::try_from(offset)?
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (key, user, read_only, created_ms, revoked_ms) = row?;
            Ok(KeySummary {
                key_id: uuid(key)?,
                user_id: uuid(user)?,
                read_only,
                created_ms,
                revoked_ms,
            })
        })
        .collect()
    }

    /// Read one user's audit metadata from a single snapshot. Notes require
    /// explicit inclusion; this operation never changes authorization or holds.
    pub fn audit_list(
        &self,
        user_id: Uuid,
        after_seq: u64,
        limit: usize,
        include_notes: bool,
    ) -> Result<AuditPage> {
        ensure!(
            (1..=100).contains(&limit) && i64::try_from(after_seq).is_ok(),
            "invalid audit pagination"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let page = audit_page(&tx, user_id, after_seq, limit, include_notes)?;
        tx.commit()?;
        Ok(page)
    }

    /// Revoke an existing key; repeating revocation is harmless.
    pub fn revoke(&self, key_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user: Option<String> = tx
            .query_row(
                "SELECT user_id FROM api_keys WHERE id=?1",
                [key_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let user_id = uuid(user.context("gateway key not found")?)?;
        let now = now_ms();
        if tx.execute(
            "UPDATE api_keys SET revoked_ms=MAX(created_ms,?2) WHERE id=?1 AND revoked_ms IS NULL",
            params![key_id.to_string(), now],
        )? == 1
        {
            audit(&tx, user_id, Some(key_id), None, "key_revoked", None, now)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Disable or explicitly re-enable a user; revoked keys never reactivate.
    pub fn set_enabled(&self, user_id: Uuid, enabled: bool) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_ms();
        ensure!(
            tx.execute(
                "UPDATE users SET enabled=?2,updated_ms=MAX(updated_ms,?3) WHERE id=?1",
                params![user_id.to_string(), enabled, now]
            )? == 1,
            "gateway user not found"
        );
        audit(
            &tx,
            user_id,
            None,
            None,
            if enabled {
                "user_enabled"
            } else {
                "user_disabled"
            },
            None,
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Fetch a bounded consistent administrative snapshot without key material.
    pub fn list(&self) -> Result<Vec<UserSummary>> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let mut result = Vec::new();
        {
            let mut statement = tx.prepare("SELECT id,backend_id,enabled,(SELECT count(*) FROM api_keys k WHERE k.user_id=u.id AND k.revoked_ms IS NULL),(SELECT count(*) FROM api_keys k WHERE k.user_id=u.id AND k.revoked_ms IS NOT NULL) FROM users u ORDER BY created_ms,id LIMIT 32")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let user_id = uuid(row.get(0)?)?;
                let hold = tx.query_row("SELECT request_id,state,reason,admitted_ms,updated_ms FROM write_holds WHERE user_id=?1", [user_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?))
                }).optional()?.map(|(id,state,reason,admitted_ms,updated_ms)| -> Result<_> {
                    Ok(WriteHoldSummary { request_id: uuid(id)?, state, reason, admitted_ms, updated_ms })
                }).transpose()?;
                result.push(UserSummary {
                    user_id,
                    backend_id: row.get(1)?,
                    enabled: row.get(2)?,
                    active_keys: row.get(3)?,
                    revoked_keys: row.get(4)?,
                    hold,
                });
            }
        }
        tx.commit()?;
        Ok(result)
    }

    /// Bounded scheduling candidates. This snapshot grants no authority: admission
    /// transactionally rechecks enabled state, backend binding and the write hold.
    pub fn scheduled_users(&self) -> Result<Vec<ScheduledUser>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id,backend_id FROM users u WHERE enabled=1 AND NOT EXISTS(SELECT 1 FROM write_holds h WHERE h.user_id=u.id) ORDER BY backend_id LIMIT 32")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (user, backend_id) = row?;
            Ok(ScheduledUser {
                user_id: uuid(user)?,
                backend_id,
            })
        })
        .collect()
    }

    /// Cron belongs to the enabled user, not a rotating or revoked login key.
    /// Shares the same sole write hold as foreground chat and mutations.
    pub fn admit_scheduled(&self, user_id: Uuid, backend_id: &str, request_id: Uuid) -> Result<()> {
        ensure!(
            request_id.get_version_num() == 7,
            "scheduled request ID must be UUIDv7"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let authorized: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND backend_id=?2 AND enabled=1)",
            params![user_id.to_string(), backend_id],
            |row| row.get(0),
        )?;
        if !authorized {
            return Err(WriteAdmissionError::Unauthorized.into());
        }
        let now = now_ms();
        insert_hold(&tx, user_id, request_id, now)?;
        audit(
            &tx,
            user_id,
            None,
            Some(request_id),
            "scheduled_write_admitted",
            None,
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Bind one enabled user's permanent backend to a private Telegram sender and bot.
    pub fn add_telegram_binding(
        &self,
        user_id: Uuid,
        bot_id: &str,
        sender_id: &str,
    ) -> Result<TelegramBindingSummary> {
        positive_telegram_id(bot_id)?;
        positive_telegram_id(sender_id)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let backend_id: String = tx
            .query_row(
                "SELECT backend_id FROM users WHERE id=?1 AND enabled=1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .context("enabled gateway user not found")?;
        let count: i64 = tx.query_row("SELECT count(*) FROM telegram_bindings", [], |row| {
            row.get(0)
        })?;
        ensure!(
            count < MAX_TELEGRAM_BINDINGS,
            "gateway Telegram lifetime binding limit reached"
        );
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM telegram_bindings WHERE user_id=?1 OR bot_id=?2)",
            params![user_id.to_string(), bot_id],
            |row| row.get(0),
        )?;
        ensure!(
            !reserved,
            "gateway Telegram user or bot is permanently reserved, including revoked bindings"
        );
        let summary = TelegramBindingSummary {
            id: Uuid::new_v4(),
            user_id,
            backend_id,
            bot_id: bot_id.into(),
            sender_id: sender_id.into(),
            enabled: true,
        };
        let now = now_ms();
        tx.execute("INSERT INTO telegram_bindings(id,user_id,backend_id,bot_id,sender_id,enabled,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,1,?6,?6)",
            params![summary.id.to_string(),user_id.to_string(),summary.backend_id,bot_id,sender_id,now])?;
        audit(
            &tx,
            user_id,
            None,
            None,
            "telegram_binding_added",
            Some(&summary.id.to_string()),
            now,
        )?;
        tx.commit()?;
        Ok(summary)
    }

    /// Bounded administrative history, including permanently revoked bindings.
    pub fn list_telegram_bindings(&self) -> Result<Vec<TelegramBindingSummary>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id,user_id,backend_id,bot_id,sender_id,enabled FROM telegram_bindings ORDER BY created_ms,id LIMIT 32")?;
        let rows = statement.query_map([], telegram_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Permanently revoke future admissions; already admitted work and its hold remain.
    pub fn revoke_telegram_binding(&self, binding_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (user, enabled): (String, bool) = tx
            .query_row(
                "SELECT user_id,enabled FROM telegram_bindings WHERE id=?1",
                [binding_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("gateway Telegram binding not found")?;
        if enabled {
            let now = now_ms();
            tx.execute(
                "UPDATE telegram_bindings SET enabled=0,updated_ms=MAX(updated_ms,?2) WHERE id=?1",
                params![binding_id.to_string(), now],
            )?;
            audit(
                &tx,
                uuid(user)?,
                None,
                None,
                "telegram_binding_revoked",
                Some(&binding_id.to_string()),
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Live ingress identity only; this short read does not grant write admission.
    pub fn telegram_authorized(&self, binding_id: Uuid) -> Result<Option<TelegramBindingSummary>> {
        telegram_on(&self.connection()?, binding_id)
    }

    /// Recheck binding/user ownership and acquire the shared write hold atomically.
    /// No prompt, event body, token or caller-selected backend is persisted here.
    pub fn admit_telegram(
        &self,
        binding_id: Uuid,
        request_id: Uuid,
        operation: &str,
        object_id: &str,
    ) -> Result<TelegramBindingSummary> {
        ensure!(
            request_id.get_version_num() == 7 && request_id.get_variant() == uuid::Variant::RFC4122,
            "Telegram request ID must be RFC4122 UUIDv7"
        );
        ensure!(
            matches!(operation, "telegram_execute" | "telegram_send"),
            "invalid Telegram admission operation"
        );
        let object =
            Uuid::parse_str(object_id).context("Telegram object ID must be a canonical UUID")?;
        ensure!(
            !object.is_nil()
                && object.get_variant() == uuid::Variant::RFC4122
                && object.to_string() == object_id,
            "Telegram object ID must be a canonical non-nil RFC4122 UUID"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding = telegram_on(&tx, binding_id)?.ok_or(WriteAdmissionError::Unauthorized)?;
        let now = now_ms();
        insert_hold(&tx, binding.user_id, request_id, now)?;
        let note = serde_json::json!({"binding_id":binding_id.to_string(),"object_id":object_id})
            .to_string();
        audit(
            &tx,
            binding.user_id,
            None,
            Some(request_id),
            operation,
            Some(&note),
            now,
        )?;
        tx.commit()?;
        Ok(binding)
    }

    /// Bind an enabled user's dedicated backend to one permanent Slack app installation and DM.
    pub fn add_slack_binding(
        &self,
        user_id: Uuid,
        team_id: &str,
        app_id: &str,
        bot_user_id: &str,
        bot_id: &str,
        sender_id: &str,
        conversation_id: &str,
    ) -> Result<SlackBindingSummary> {
        for (value, prefix) in [
            (team_id, b'T'),
            (app_id, b'A'),
            (bot_user_id, b'U'),
            (bot_id, b'B'),
            (sender_id, b'U'),
            (conversation_id, b'D'),
        ] {
            ensure!(
                canonical_slack_id(value, prefix),
                "invalid canonical Slack ID"
            );
        }
        ensure!(
            sender_id != bot_user_id,
            "Slack sender must differ from the bot user"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let backend_id: String = tx
            .query_row(
                "SELECT backend_id FROM users WHERE id=?1 AND enabled=1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .context("enabled gateway user not found")?;
        let count: i64 =
            tx.query_row("SELECT count(*) FROM slack_bindings", [], |row| row.get(0))?;
        ensure!(
            count < MAX_SLACK_BINDINGS,
            "gateway Slack lifetime binding limit reached"
        );
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM slack_bindings WHERE user_id=?1 OR app_id=?2)",
            params![user_id.to_string(), app_id],
            |row| row.get(0),
        )?;
        ensure!(
            !reserved,
            "gateway Slack user or app is permanently reserved, including revoked bindings"
        );
        let summary = SlackBindingSummary {
            id: Uuid::new_v4(),
            user_id,
            backend_id,
            team_id: team_id.into(),
            app_id: app_id.into(),
            bot_user_id: bot_user_id.into(),
            bot_id: bot_id.into(),
            sender_id: sender_id.into(),
            conversation_id: conversation_id.into(),
            enabled: true,
        };
        let now = now_ms();
        tx.execute("INSERT INTO slack_bindings(id,user_id,backend_id,team_id,app_id,bot_user_id,bot_id,sender_id,conversation_id,enabled,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?10,?10)",
            params![summary.id.to_string(),user_id.to_string(),summary.backend_id,team_id,app_id,bot_user_id,bot_id,sender_id,conversation_id,now])?;
        audit(
            &tx,
            user_id,
            None,
            None,
            "slack_binding_added",
            Some(&summary.id.to_string()),
            now,
        )?;
        tx.commit()?;
        Ok(summary)
    }

    /// Bounded lifetime history, including permanently revoked installations.
    pub fn list_slack_bindings(&self) -> Result<Vec<SlackBindingSummary>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id,user_id,backend_id,team_id,app_id,bot_user_id,bot_id,sender_id,conversation_id,enabled FROM slack_bindings ORDER BY created_ms,id LIMIT 32")?;
        let rows = statement.query_map([], slack_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Permanently revoke future admissions without altering already admitted work or its hold.
    pub fn revoke_slack_binding(&self, binding_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (user, enabled): (String, bool) = tx
            .query_row(
                "SELECT user_id,enabled FROM slack_bindings WHERE id=?1",
                [binding_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("gateway Slack binding not found")?;
        if enabled {
            let now = now_ms();
            tx.execute(
                "UPDATE slack_bindings SET enabled=0,updated_ms=MAX(updated_ms,?2) WHERE id=?1",
                params![binding_id.to_string(), now],
            )?;
            audit(
                &tx,
                uuid(user)?,
                None,
                None,
                "slack_binding_revoked",
                Some(&binding_id.to_string()),
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Live ingress identity only; the write transaction must recheck it before every effect.
    pub fn slack_authorized(&self, binding_id: Uuid) -> Result<Option<SlackBindingSummary>> {
        slack_on(&self.connection()?, binding_id)
    }

    /// Recheck enabled ownership and acquire the same hold used by HTTP, cron and Telegram.
    /// Audit stores only immutable binding/object UUIDs; prompts and credentials stay out.
    pub fn admit_slack(
        &self,
        binding_id: Uuid,
        request_id: Uuid,
        operation: &str,
        object_id: &str,
    ) -> Result<SlackBindingSummary> {
        ensure!(
            request_id.get_version_num() == 7 && request_id.get_variant() == uuid::Variant::RFC4122,
            "Slack request ID must be RFC4122 UUIDv7"
        );
        ensure!(
            matches!(operation, "slack_execute" | "slack_send"),
            "invalid Slack admission operation"
        );
        let object =
            Uuid::parse_str(object_id).context("Slack object ID must be a canonical UUID")?;
        ensure!(
            !object.is_nil()
                && object.get_variant() == uuid::Variant::RFC4122
                && object.to_string() == object_id,
            "Slack object ID must be a canonical non-nil RFC4122 UUID"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding = slack_on(&tx, binding_id)?.ok_or(WriteAdmissionError::Unauthorized)?;
        let now = now_ms();
        insert_hold(&tx, binding.user_id, request_id, now)?;
        let note = serde_json::json!({"binding_id":binding_id.to_string(),"object_id":object_id})
            .to_string();
        audit(
            &tx,
            binding.user_id,
            None,
            Some(request_id),
            operation,
            Some(&note),
            now,
        )?;
        tx.commit()?;
        Ok(binding)
    }

    /// Bind an enabled user's dedicated backend to one permanent Discord application and private bot DM.
    pub fn add_discord_binding(
        &self,
        user_id: Uuid,
        application_id: &str,
        verify_key: &str,
        bot_user_id: &str,
        sender_id: &str,
        conversation_id: &str,
        command_id: &str,
    ) -> Result<DiscordBindingSummary> {
        for value in [
            application_id,
            bot_user_id,
            sender_id,
            conversation_id,
            command_id,
        ] {
            ensure!(
                canonical_discord_id(value),
                "invalid canonical Discord snowflake"
            );
        }
        ensure!(
            canonical_discord_key(verify_key),
            "invalid canonical Discord public verification key"
        );
        ensure!(
            sender_id != bot_user_id,
            "Discord sender must differ from the bot user"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let backend_id: String = tx
            .query_row(
                "SELECT backend_id FROM users WHERE id=?1 AND enabled=1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .context("enabled gateway user not found")?;
        let count: i64 = tx.query_row("SELECT count(*) FROM discord_bindings", [], |row| {
            row.get(0)
        })?;
        ensure!(
            count < MAX_DISCORD_BINDINGS,
            "gateway Discord lifetime binding limit reached"
        );
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM discord_bindings WHERE user_id=?1 OR application_id=?2)",
            params![user_id.to_string(), application_id],
            |row| row.get(0),
        )?;
        ensure!(
            !reserved,
            "gateway Discord user or app is permanently reserved, including revoked bindings"
        );
        let summary = DiscordBindingSummary {
            id: Uuid::new_v4(),
            user_id,
            backend_id,
            application_id: application_id.into(),
            verify_key: verify_key.into(),
            bot_user_id: bot_user_id.into(),
            sender_id: sender_id.into(),
            conversation_id: conversation_id.into(),
            command_id: command_id.into(),
            enabled: true,
        };
        let now = now_ms();
        tx.execute("INSERT INTO discord_bindings(id,user_id,backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id,enabled,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?10,?10)",
            params![summary.id.to_string(),user_id.to_string(),summary.backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id,now])?;
        audit(
            &tx,
            user_id,
            None,
            None,
            "discord_binding_added",
            Some(&summary.id.to_string()),
            now,
        )?;
        tx.commit()?;
        Ok(summary)
    }

    /// Bounded lifetime history, including permanently revoked installations.
    pub fn list_discord_bindings(&self) -> Result<Vec<DiscordBindingSummary>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id,user_id,backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id,enabled FROM discord_bindings ORDER BY created_ms,id LIMIT 32")?;
        let rows = statement.query_map([], discord_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Permanently revoke future admissions without altering already admitted work or its hold.
    pub fn revoke_discord_binding(&self, binding_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (user, enabled): (String, bool) = tx
            .query_row(
                "SELECT user_id,enabled FROM discord_bindings WHERE id=?1",
                [binding_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("gateway Discord binding not found")?;
        if enabled {
            let now = now_ms();
            tx.execute(
                "UPDATE discord_bindings SET enabled=0,updated_ms=MAX(updated_ms,?2) WHERE id=?1",
                params![binding_id.to_string(), now],
            )?;
            audit(
                &tx,
                uuid(user)?,
                None,
                None,
                "discord_binding_revoked",
                Some(&binding_id.to_string()),
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Live ingress identity only; the write transaction must recheck it before every effect.
    pub fn discord_authorized(&self, binding_id: Uuid) -> Result<Option<DiscordBindingSummary>> {
        discord_on(&self.connection()?, binding_id)
    }

    /// Recheck enabled ownership and acquire the same hold used by HTTP, cron and Telegram.
    /// Audit stores only immutable binding/object UUIDs; prompts and credentials stay out.
    pub fn admit_discord(
        &self,
        binding_id: Uuid,
        request_id: Uuid,
        operation: &str,
        object_id: &str,
    ) -> Result<DiscordBindingSummary> {
        ensure!(
            request_id.get_version_num() == 7 && request_id.get_variant() == uuid::Variant::RFC4122,
            "Discord request ID must be RFC4122 UUIDv7"
        );
        ensure!(
            matches!(operation, "discord_execute" | "discord_send"),
            "invalid Discord admission operation"
        );
        let object =
            Uuid::parse_str(object_id).context("Discord object ID must be a canonical UUID")?;
        ensure!(
            !object.is_nil()
                && object.get_variant() == uuid::Variant::RFC4122
                && object.to_string() == object_id,
            "Discord object ID must be a canonical non-nil RFC4122 UUID"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding = discord_on(&tx, binding_id)?.ok_or(WriteAdmissionError::Unauthorized)?;
        let now = now_ms();
        insert_hold(&tx, binding.user_id, request_id, now)?;
        let note = serde_json::json!({"binding_id":binding_id.to_string(),"object_id":object_id})
            .to_string();
        audit(
            &tx,
            binding.user_id,
            None,
            Some(request_id),
            operation,
            Some(&note),
            now,
        )?;
        tx.commit()?;
        Ok(binding)
    }

    /// Bind an enabled user's backend to a dedicated Feishu application and private chat.
    pub fn add_feishu_binding(
        &self,
        user_id: Uuid,
        app_id: &str,
        tenant_key: &str,
        bot_open_id: &str,
        human_open_id: &str,
        chat_id: &str,
    ) -> Result<FeishuBindingSummary> {
        for (value, prefix) in [
            (app_id, "cli_"),
            (tenant_key, ""),
            (bot_open_id, "ou_"),
            (human_open_id, "ou_"),
            (chat_id, "oc_"),
        ] {
            ensure!(
                canonical_feishu_id(value, prefix),
                "invalid canonical Feishu ID"
            );
        }
        ensure!(
            app_id.len() + tenant_key.len() + 1 <= 128,
            "Feishu installation ID exceeds its size limit"
        );
        ensure!(
            bot_open_id != human_open_id,
            "Feishu human must differ from the bot"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let backend_id: String = tx
            .query_row(
                "SELECT backend_id FROM users WHERE id=?1 AND enabled=1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .context("enabled gateway user not found")?;
        let count: i64 =
            tx.query_row("SELECT count(*) FROM feishu_bindings", [], |row| row.get(0))?;
        ensure!(
            count < MAX_FEISHU_BINDINGS,
            "gateway Feishu lifetime binding limit reached"
        );
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM feishu_bindings WHERE user_id=?1 OR app_id=?2)",
            params![user_id.to_string(), app_id],
            |row| row.get(0),
        )?;
        ensure!(
            !reserved,
            "gateway Feishu user or app is permanently reserved, including revoked bindings"
        );
        let summary = FeishuBindingSummary {
            id: Uuid::new_v4(),
            user_id,
            backend_id,
            app_id: app_id.into(),
            tenant_key: tenant_key.into(),
            bot_open_id: bot_open_id.into(),
            human_open_id: human_open_id.into(),
            chat_id: chat_id.into(),
            enabled: true,
        };
        let now = now_ms();
        tx.execute("INSERT INTO feishu_bindings(id,user_id,backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id,enabled,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?9)",
            params![summary.id.to_string(),user_id.to_string(),summary.backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id,now])?;
        audit(
            &tx,
            user_id,
            None,
            None,
            "feishu_binding_added",
            Some(&summary.id.to_string()),
            now,
        )?;
        tx.commit()?;
        Ok(summary)
    }

    /// Exact administrative identity, including permanent revocation and disabled users.
    pub fn feishu_binding(&self, binding_id: Uuid) -> Result<Option<FeishuBindingSummary>> {
        Ok(self.connection()?.query_row(
            "SELECT id,user_id,backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id,enabled FROM feishu_bindings WHERE id=?1",
            [binding_id.to_string()], feishu_row,
        ).optional()?)
    }

    /// Bounded lifetime history; secrets and prompts are never stored in this table.
    pub fn list_feishu_bindings(&self) -> Result<Vec<FeishuBindingSummary>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id,user_id,backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id,enabled FROM feishu_bindings ORDER BY created_ms,id LIMIT 32")?;
        let rows = statement.query_map([], feishu_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Permanently revoke future admissions while retaining reservations and existing holds.
    pub fn revoke_feishu_binding(&self, binding_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (user, enabled): (String, bool) = tx
            .query_row(
                "SELECT user_id,enabled FROM feishu_bindings WHERE id=?1",
                [binding_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("gateway Feishu binding not found")?;
        if enabled {
            let now = now_ms();
            tx.execute(
                "UPDATE feishu_bindings SET enabled=0,updated_ms=MAX(updated_ms,?2) WHERE id=?1",
                params![binding_id.to_string(), now],
            )?;
            audit(
                &tx,
                uuid(user)?,
                None,
                None,
                "feishu_binding_revoked",
                Some(&binding_id.to_string()),
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Live ingress identity only; each effect must acquire a fresh shared write hold.
    pub fn feishu_authorized(&self, binding_id: Uuid) -> Result<Option<FeishuBindingSummary>> {
        feishu_on(&self.connection()?, binding_id)
    }

    /// Recheck current ownership and acquire the hold shared by HTTP, cron and all channels.
    pub fn admit_feishu(
        &self,
        binding_id: Uuid,
        request_id: Uuid,
        operation: &str,
        object_id: &str,
    ) -> Result<FeishuBindingSummary> {
        ensure!(
            request_id.get_version_num() == 7 && request_id.get_variant() == uuid::Variant::RFC4122,
            "Feishu request ID must be RFC4122 UUIDv7"
        );
        ensure!(
            matches!(operation, "feishu_execute" | "feishu_send"),
            "invalid Feishu admission operation"
        );
        let object =
            Uuid::parse_str(object_id).context("Feishu object ID must be a canonical UUID")?;
        ensure!(
            !object.is_nil()
                && object.get_variant() == uuid::Variant::RFC4122
                && object.to_string() == object_id,
            "Feishu object ID must be a canonical non-nil RFC4122 UUID"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding = feishu_on(&tx, binding_id)?.ok_or(WriteAdmissionError::Unauthorized)?;
        let now = now_ms();
        insert_hold(&tx, binding.user_id, request_id, now)?;
        let note = serde_json::json!({"binding_id":binding_id.to_string(),"object_id":object_id})
            .to_string();
        audit(
            &tx,
            binding.user_id,
            None,
            Some(request_id),
            operation,
            Some(&note),
            now,
        )?;
        tx.commit()?;
        Ok(binding)
    }

    /// Bind an enabled user's backend to a dedicated WeCom application and private member.
    pub fn add_wecom_binding(
        &self,
        user_id: Uuid,
        corp_id: &str,
        agent_id: u32,
        human_user_id: &str,
    ) -> Result<WecomBindingSummary> {
        crate::wecom::validate_installation(&format!("{corp_id}:{agent_id}"))?;
        ensure!(
            crate::wecom::user_id(human_user_id),
            "invalid canonical WeCom member ID"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let backend_id: String = tx
            .query_row(
                "SELECT backend_id FROM users WHERE id=?1 AND enabled=1",
                [user_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .context("enabled gateway user not found")?;
        let count: i64 =
            tx.query_row("SELECT count(*) FROM wecom_bindings", [], |row| row.get(0))?;
        ensure!(
            count < MAX_WECOM_BINDINGS,
            "gateway WeCom lifetime binding limit reached"
        );
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM wecom_bindings WHERE user_id=?1 OR (corp_id=?2 AND agent_id=?3))",
            params![user_id.to_string(),corp_id,agent_id], |row| row.get(0),
        )?;
        ensure!(
            !reserved,
            "gateway WeCom user or application is permanently reserved, including revoked bindings"
        );
        let summary = WecomBindingSummary {
            id: Uuid::new_v4(),
            user_id,
            backend_id,
            corp_id: corp_id.into(),
            agent_id,
            human_user_id: human_user_id.into(),
            enabled: true,
        };
        let now = now_ms();
        tx.execute("INSERT INTO wecom_bindings(id,user_id,backend_id,corp_id,agent_id,human_user_id,enabled,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7)",
            params![summary.id.to_string(),user_id.to_string(),summary.backend_id,corp_id,agent_id,human_user_id,now])?;
        audit(
            &tx,
            user_id,
            None,
            None,
            "wecom_binding_added",
            Some(&summary.id.to_string()),
            now,
        )?;
        tx.commit()?;
        Ok(summary)
    }

    /// Exact administrative identity, including permanent revocation and disabled users.
    pub fn wecom_binding(&self, binding_id: Uuid) -> Result<Option<WecomBindingSummary>> {
        Ok(self.connection()?.query_row(
            "SELECT id,user_id,backend_id,corp_id,agent_id,human_user_id,enabled FROM wecom_bindings WHERE id=?1",
            [binding_id.to_string()], wecom_row,
        ).optional()?)
    }

    /// Bounded lifetime history; credentials and prompts are never stored in this table.
    pub fn list_wecom_bindings(&self) -> Result<Vec<WecomBindingSummary>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id,user_id,backend_id,corp_id,agent_id,human_user_id,enabled FROM wecom_bindings ORDER BY created_ms,id LIMIT 32")?;
        let rows = statement.query_map([], wecom_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Permanently revoke future admissions, retaining the application reservation and holds.
    pub fn revoke_wecom_binding(&self, binding_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (user, enabled): (String, bool) = tx
            .query_row(
                "SELECT user_id,enabled FROM wecom_bindings WHERE id=?1",
                [binding_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("gateway WeCom binding not found")?;
        if enabled {
            let now = now_ms();
            tx.execute(
                "UPDATE wecom_bindings SET enabled=0,updated_ms=MAX(updated_ms,?2) WHERE id=?1",
                params![binding_id.to_string(), now],
            )?;
            audit(
                &tx,
                uuid(user)?,
                None,
                None,
                "wecom_binding_revoked",
                Some(&binding_id.to_string()),
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Live ingress identity only; each effect must acquire a fresh shared write hold.
    pub fn wecom_authorized(&self, binding_id: Uuid) -> Result<Option<WecomBindingSummary>> {
        wecom_on(&self.connection()?, binding_id)
    }

    /// Recheck current ownership and acquire the hold shared by HTTP, cron and all channels.
    pub fn admit_wecom(
        &self,
        binding_id: Uuid,
        request_id: Uuid,
        operation: &str,
        object_id: &str,
    ) -> Result<WecomBindingSummary> {
        ensure!(
            request_id.get_version_num() == 7 && request_id.get_variant() == uuid::Variant::RFC4122,
            "WeCom request ID must be RFC4122 UUIDv7"
        );
        ensure!(
            matches!(operation, "wecom_execute" | "wecom_send"),
            "invalid WeCom admission operation"
        );
        let object =
            Uuid::parse_str(object_id).context("WeCom object ID must be a canonical UUID")?;
        ensure!(
            !object.is_nil()
                && object.get_variant() == uuid::Variant::RFC4122
                && object.to_string() == object_id,
            "WeCom object ID must be a canonical non-nil RFC4122 UUID"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding = wecom_on(&tx, binding_id)?.ok_or(WriteAdmissionError::Unauthorized)?;
        let now = now_ms();
        insert_hold(&tx, binding.user_id, request_id, now)?;
        let note = serde_json::json!({"binding_id":binding_id.to_string(),"object_id":object_id})
            .to_string();
        audit(
            &tx,
            binding.user_id,
            None,
            Some(request_id),
            operation,
            Some(&note),
            now,
        )?;
        tx.commit()?;
        Ok(binding)
    }

    /// Read live key/user state for every request. No successful-authentication cache exists.
    pub fn authenticate(&self, token: &str) -> Result<Option<Principal>> {
        let Some(parsed) = keys::parse(token) else {
            return Ok(None);
        };
        let conn = self.connection()?;
        let stored = conn.query_row("SELECT k.user_id,u.backend_id,k.verifier,u.enabled,k.revoked_ms IS NULL,k.read_only FROM api_keys k JOIN users u ON u.id=k.user_id WHERE k.id=?1", [parsed.key_id.to_string()], |row| {
            Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,bool>(3)?,row.get::<_,bool>(4)?,row.get::<_,bool>(5)?))
        }).optional()?;
        if let Some((user, backend_id, verifier, enabled, active, read_only)) = stored {
            let user_id = uuid(user)?;
            let valid = keys::matches(&parsed.verifier(user_id), &verifier);
            if valid && enabled && active {
                return Ok(Some(Principal {
                    user_id,
                    key_id: parsed.key_id,
                    backend_id,
                    read_only,
                }));
            }
        } else {
            // A missing public key ID still performs one fixed-length verifier comparison.
            std::hint::black_box(keys::matches(&parsed.verifier(Uuid::nil()), &[0_u8; 32]));
        }
        Ok(None)
    }

    /// Reserve the user's only write slot and recheck live authorization in the same transaction.
    pub fn admit_write(&self, principal: &Principal, request_id: Uuid) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // The authenticated snapshot is not authority for a write: re-read the
        // persisted access bit together with revocation and user state in this tx.
        let read_only: Option<bool> = tx.query_row("SELECT k.read_only FROM api_keys k JOIN users u ON u.id=k.user_id WHERE k.id=?1 AND u.id=?2 AND u.backend_id=?3 AND u.enabled=1 AND k.revoked_ms IS NULL", params![principal.key_id.to_string(), principal.user_id.to_string(), principal.backend_id], |row| row.get(0)).optional()?;
        match read_only {
            None => return Err(WriteAdmissionError::Unauthorized.into()),
            Some(true) => return Err(WriteAdmissionError::ReadOnly.into()),
            Some(false) => {}
        }
        let now = now_ms();
        insert_hold(&tx, principal.user_id, request_id, now)?;
        audit(
            &tx,
            principal.user_id,
            Some(principal.key_id),
            Some(request_id),
            "write_admitted",
            Some(access_note(false)),
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Settle only the matching in-flight write. Recovered/review holds require an operator.
    pub fn finish_write(&self, user_id: Uuid, request_id: Uuid, known_success: bool) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_ms();
        let changed = if known_success {
            tx.execute(
                "DELETE FROM write_holds WHERE user_id=?1 AND request_id=?2 AND state='in_flight'",
                params![user_id.to_string(), request_id.to_string()],
            )?
        } else {
            tx.execute("UPDATE write_holds SET state='needs_review',reason='backend_outcome_unknown',updated_ms=MAX(updated_ms,?3) WHERE user_id=?1 AND request_id=?2 AND state='in_flight'", params![user_id.to_string(),request_id.to_string(),now])?
        };
        if changed == 1 {
            audit(
                &tx,
                user_id,
                None,
                Some(request_id),
                if known_success {
                    "write_completed"
                } else {
                    "write_needs_review"
                },
                None,
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Called after the gateway runtime lock is acquired; never replay uncertain writes.
    pub fn recover_writes(&self) -> Result<usize> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let holds = {
            let mut statement =
                tx.prepare("SELECT user_id,request_id FROM write_holds WHERE state='in_flight'")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = now_ms();
        let changed = tx.execute("UPDATE write_holds SET state='needs_review',reason='gateway_restarted',updated_ms=MAX(updated_ms,?1) WHERE state='in_flight'", [now])?;
        for (user, request) in holds {
            audit(
                &tx,
                uuid(user)?,
                None,
                Some(uuid(request)?),
                "write_recovered_for_review",
                None,
                now,
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Clear an uncertain outcome only after external review, retaining a bounded audit note.
    pub fn clear_review(&self, user_id: Uuid, note: &str) -> Result<()> {
        ensure!(
            !note.trim().is_empty() && note.len() <= 512 && !note.chars().any(char::is_control),
            "review note must contain 1..512 UTF-8 bytes without control characters"
        );
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let hold: Option<(String, String)> = tx
            .query_row(
                "SELECT request_id,state FROM write_holds WHERE user_id=?1",
                [user_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (request, state) = hold.context("gateway user has no review hold")?;
        ensure!(
            state == "needs_review",
            "cannot clear an in-flight write; stop and reconcile gateway/backend work first"
        );
        tx.execute(
            "DELETE FROM write_holds WHERE user_id=?1 AND state='needs_review'",
            [user_id.to_string()],
        )?;
        audit(
            &tx,
            user_id,
            None,
            Some(uuid(request)?),
            "write_review_cleared",
            Some(note),
            now_ms(),
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn positive_telegram_id(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 19
            && value
                .parse::<i64>()
                .ok()
                .is_some_and(|number| number > 0 && number.to_string() == value),
        "Telegram numeric IDs must be canonical positive i64 values"
    );
    Ok(())
}

fn telegram_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TelegramBindingSummary> {
    let id = |index| -> rusqlite::Result<Uuid> {
        let text: String = row.get(index)?;
        Uuid::parse_str(&text).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    Ok(TelegramBindingSummary {
        id: id(0)?,
        user_id: id(1)?,
        backend_id: row.get(2)?,
        bot_id: row.get(3)?,
        sender_id: row.get(4)?,
        enabled: row.get(5)?,
    })
}

fn telegram_on(conn: &Connection, binding_id: Uuid) -> Result<Option<TelegramBindingSummary>> {
    Ok(conn.query_row("SELECT b.id,b.user_id,b.backend_id,b.bot_id,b.sender_id,b.enabled FROM telegram_bindings b JOIN users u ON u.id=b.user_id AND u.backend_id=b.backend_id WHERE b.id=?1 AND b.enabled=1 AND u.enabled=1",
        [binding_id.to_string()],telegram_row).optional()?)
}

fn canonical_slack_id(value: &str, prefix: u8) -> bool {
    (2..=64).contains(&value.len())
        && (value.as_bytes()[0] == prefix || (prefix == b'U' && value.as_bytes()[0] == b'W'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn slack_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SlackBindingSummary> {
    let invalid = |index| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid gateway Slack binding identity",
            )),
        )
    };
    let id = |index| -> rusqlite::Result<Uuid> {
        let text: String = row.get(index)?;
        Uuid::parse_str(&text)
            .ok()
            .filter(|id| {
                !id.is_nil() && id.get_variant() == uuid::Variant::RFC4122 && id.to_string() == text
            })
            .ok_or_else(|| invalid(index))
    };
    let platform_id = |index, prefix| -> rusqlite::Result<String> {
        let text: String = row.get(index)?;
        if !canonical_slack_id(&text, prefix) {
            return Err(invalid(index));
        }
        Ok(text)
    };
    let bot_user_id = platform_id(5, b'U')?;
    let sender_id = platform_id(7, b'U')?;
    if bot_user_id == sender_id {
        return Err(invalid(7));
    }
    let enabled: i64 = row.get(9)?;
    if !matches!(enabled, 0 | 1) {
        return Err(invalid(9));
    }
    Ok(SlackBindingSummary {
        id: id(0)?,
        user_id: id(1)?,
        backend_id: row.get(2)?,
        team_id: platform_id(3, b'T')?,
        app_id: platform_id(4, b'A')?,
        bot_user_id,
        bot_id: platform_id(6, b'B')?,
        sender_id,
        conversation_id: platform_id(8, b'D')?,
        enabled: enabled == 1,
    })
}

fn slack_on(conn: &Connection, binding_id: Uuid) -> Result<Option<SlackBindingSummary>> {
    Ok(conn.query_row("SELECT b.id,b.user_id,b.backend_id,b.team_id,b.app_id,b.bot_user_id,b.bot_id,b.sender_id,b.conversation_id,b.enabled FROM slack_bindings b JOIN users u ON u.id=b.user_id AND u.backend_id=b.backend_id WHERE b.id=?1 AND b.enabled=1 AND u.enabled=1",
        [binding_id.to_string()], slack_row).optional()?)
}

/// Discord snowflakes include the whole unsigned 64-bit range, unlike SQLite INTEGER.
fn canonical_discord_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|id| id > 0)
}

fn canonical_discord_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

fn discord_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscordBindingSummary> {
    let invalid = |index| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid gateway Discord binding identity",
            )),
        )
    };
    let id = |index| -> rusqlite::Result<Uuid> {
        let text: String = row.get(index)?;
        Uuid::parse_str(&text)
            .ok()
            .filter(|id| {
                !id.is_nil() && id.get_variant() == uuid::Variant::RFC4122 && id.to_string() == text
            })
            .ok_or_else(|| invalid(index))
    };
    let platform_id = |index| -> rusqlite::Result<String> {
        let text: String = row.get(index)?;
        if !canonical_discord_id(&text) {
            return Err(invalid(index));
        }
        Ok(text)
    };
    let verify_key: String = row.get(4)?;
    if !canonical_discord_key(&verify_key) {
        return Err(invalid(4));
    }
    let bot_user_id = platform_id(5)?;
    let sender_id = platform_id(6)?;
    if bot_user_id == sender_id {
        return Err(invalid(6));
    }
    let enabled: i64 = row.get(9)?;
    if !matches!(enabled, 0 | 1) {
        return Err(invalid(9));
    }
    Ok(DiscordBindingSummary {
        id: id(0)?,
        user_id: id(1)?,
        backend_id: row.get(2)?,
        application_id: platform_id(3)?,
        verify_key,
        bot_user_id,
        sender_id,
        conversation_id: platform_id(7)?,
        command_id: platform_id(8)?,
        enabled: enabled == 1,
    })
}

fn discord_on(conn: &Connection, binding_id: Uuid) -> Result<Option<DiscordBindingSummary>> {
    Ok(conn.query_row("SELECT b.id,b.user_id,b.backend_id,b.application_id,b.verify_key,b.bot_user_id,b.sender_id,b.conversation_id,b.command_id,b.enabled FROM discord_bindings b JOIN users u ON u.id=b.user_id AND u.backend_id=b.backend_id WHERE b.id=?1 AND b.enabled=1 AND u.enabled=1",
        [binding_id.to_string()], discord_row).optional()?)
}

fn canonical_feishu_id(value: &str, prefix: &str) -> bool {
    value.len() > prefix.len()
        && value.len() <= 128
        && value.starts_with(prefix)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn feishu_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FeishuBindingSummary> {
    let invalid = |index| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid gateway Feishu binding identity",
            )),
        )
    };
    let id = |index| -> rusqlite::Result<Uuid> {
        let text: String = row.get(index)?;
        Uuid::parse_str(&text)
            .ok()
            .filter(|id| {
                !id.is_nil() && id.get_variant() == uuid::Variant::RFC4122 && id.to_string() == text
            })
            .ok_or_else(|| invalid(index))
    };
    let platform_id = |index, prefix| -> rusqlite::Result<String> {
        let text: String = row.get(index)?;
        if !canonical_feishu_id(&text, prefix) {
            return Err(invalid(index));
        }
        Ok(text)
    };
    let app_id = platform_id(3, "cli_")?;
    let tenant_key = platform_id(4, "")?;
    if app_id.len() + tenant_key.len() + 1 > 128 {
        return Err(invalid(4));
    }
    let bot_open_id = platform_id(5, "ou_")?;
    let human_open_id = platform_id(6, "ou_")?;
    if bot_open_id == human_open_id {
        return Err(invalid(6));
    }
    let enabled: i64 = row.get(8)?;
    if !matches!(enabled, 0 | 1) {
        return Err(invalid(8));
    }
    let backend_id: String = row.get(2)?;
    if backend_id.is_empty()
        || backend_id.len() > 64
        || !backend_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(2));
    }
    Ok(FeishuBindingSummary {
        id: id(0)?,
        user_id: id(1)?,
        backend_id,
        app_id,
        tenant_key,
        bot_open_id,
        human_open_id,
        chat_id: platform_id(7, "oc_")?,
        enabled: enabled == 1,
    })
}

fn feishu_on(conn: &Connection, binding_id: Uuid) -> Result<Option<FeishuBindingSummary>> {
    Ok(conn.query_row("SELECT b.id,b.user_id,b.backend_id,b.app_id,b.tenant_key,b.bot_open_id,b.human_open_id,b.chat_id,b.enabled FROM feishu_bindings b JOIN users u ON u.id=b.user_id AND u.backend_id=b.backend_id WHERE b.id=?1 AND b.enabled=1 AND u.enabled=1",
        [binding_id.to_string()], feishu_row).optional()?)
}

fn wecom_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WecomBindingSummary> {
    let invalid = |index| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid gateway WeCom binding identity",
            )),
        )
    };
    let id = |index| -> rusqlite::Result<Uuid> {
        let text: String = row.get(index)?;
        Uuid::parse_str(&text)
            .ok()
            .filter(|id| {
                !id.is_nil() && id.get_variant() == uuid::Variant::RFC4122 && id.to_string() == text
            })
            .ok_or_else(|| invalid(index))
    };
    let corp_id: String = row.get(3)?;
    let agent: i64 = row.get(4)?;
    let agent_id = u32::try_from(agent)
        .ok()
        .filter(|agent| *agent > 0 && *agent <= i32::MAX as u32)
        .ok_or_else(|| invalid(4))?;
    if crate::wecom::validate_installation(&format!("{corp_id}:{agent_id}")).is_err() {
        return Err(invalid(3));
    }
    let human_user_id: String = row.get(5)?;
    if !crate::wecom::user_id(&human_user_id) {
        return Err(invalid(5));
    }
    let backend_id: String = row.get(2)?;
    if backend_id.is_empty()
        || backend_id.len() > 64
        || !backend_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(2));
    }
    let enabled: i64 = row.get(6)?;
    if !matches!(enabled, 0 | 1) {
        return Err(invalid(6));
    }
    Ok(WecomBindingSummary {
        id: id(0)?,
        user_id: id(1)?,
        backend_id,
        corp_id,
        agent_id,
        human_user_id,
        enabled: enabled == 1,
    })
}

fn wecom_on(conn: &Connection, binding_id: Uuid) -> Result<Option<WecomBindingSummary>> {
    Ok(conn.query_row("SELECT b.id,b.user_id,b.backend_id,b.corp_id,b.agent_id,b.human_user_id,b.enabled FROM wecom_bindings b JOIN users u ON u.id=b.user_id AND u.backend_id=b.backend_id WHERE b.id=?1 AND b.enabled=1 AND u.enabled=1",
        [binding_id.to_string()],wecom_row).optional()?)
}

fn insert_hold(tx: &Transaction<'_>, user_id: Uuid, request_id: Uuid, now: i64) -> Result<()> {
    let held: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM write_holds WHERE user_id=?1)",
        [user_id.to_string()],
        |row| row.get(0),
    )?;
    if held {
        return Err(WriteAdmissionError::Held.into());
    }
    tx.execute("INSERT INTO write_holds(user_id,request_id,state,reason,admitted_ms,updated_ms) VALUES(?1,?2,'in_flight','request_in_flight',?3,?3)",
        params![user_id.to_string(),request_id.to_string(),now])?;
    Ok(())
}

fn enabled_user(tx: &Transaction<'_>, user_id: Uuid) -> Result<()> {
    let enabled: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND enabled=1)",
        [user_id.to_string()],
        |row| row.get(0),
    )?;
    ensure!(enabled, "enabled gateway user not found");
    Ok(())
}

fn insert_key(
    tx: &Transaction<'_>,
    issued: &IssuedKey,
    verifier: &[u8; 32],
    now: i64,
) -> Result<()> {
    let active: i64 = tx.query_row(
        "SELECT count(*) FROM api_keys WHERE user_id=?1 AND revoked_ms IS NULL",
        [issued.user_id.to_string()],
        |row| row.get(0),
    )?;
    ensure!(active < MAX_ACTIVE_KEYS, "gateway active key limit reached");
    let total: i64 = tx.query_row("SELECT count(*) FROM api_keys", [], |row| row.get(0))?;
    if total >= MAX_KEYS {
        tx.execute("DELETE FROM api_keys WHERE id IN (SELECT id FROM api_keys WHERE revoked_ms IS NOT NULL ORDER BY revoked_ms,id LIMIT ?1)", [total - MAX_KEYS + 1])?;
        let remaining: i64 = tx.query_row("SELECT count(*) FROM api_keys", [], |row| row.get(0))?;
        ensure!(remaining < MAX_KEYS, "gateway key history limit reached");
    }
    tx.execute(
        "INSERT INTO api_keys(id,user_id,verifier,created_ms,revoked_ms,read_only) VALUES(?1,?2,?3,?4,NULL,?5)",
        params![
            issued.key_id.to_string(),
            issued.user_id.to_string(),
            verifier.as_slice(),
            now,
            issued.read_only
        ],
    )?;
    Ok(())
}

fn access_note(read_only: bool) -> &'static str {
    if read_only {
        r#"{"read_only":true}"#
    } else {
        r#"{"read_only":false}"#
    }
}

fn audit_identity(value: String) -> Result<String> {
    ensure!(
        value.len() == 36 && Uuid::parse_str(&value).is_ok_and(|id| id.to_string() == value),
        "invalid gateway audit identity"
    );
    Ok(value)
}

fn audit_page(
    tx: &Transaction<'_>,
    user_id: Uuid,
    after_seq: u64,
    limit: usize,
    include_notes: bool,
) -> Result<AuditPage> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
        [user_id.to_string()],
        |row| row.get(0),
    )?;
    ensure!(exists, "gateway user not found");
    // AUTOINCREMENT retains the committed global watermark even when all
    // business rows have been pruned. Reading MAX(seq) alone would lose it.
    let (sequence_rows, watermark): (i64, Option<i64>) = tx.query_row(
        "SELECT count(*),min(seq) FROM sqlite_sequence WHERE name='audit_events'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(
        (sequence_rows == 0 && watermark.is_none()) || (sequence_rows == 1 && watermark.is_some()),
        "invalid gateway audit sequence state"
    );
    let latest = watermark.unwrap_or(0);
    ensure!(latest >= 0, "invalid gateway audit sequence state");
    let (oldest, newest): (Option<i64>, Option<i64>) =
        tx.query_row("SELECT min(seq),max(seq) FROM audit_events", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    ensure!(
        oldest.is_none_or(|value| value > 0) && newest.is_none_or(|value| value <= latest),
        "invalid gateway retained audit sequence state"
    );
    let latest_seq = u64::try_from(latest)?;
    ensure!(
        after_seq <= latest_seq,
        "audit cursor exceeds current history; verify the registry and restore point"
    );
    let retention_gap = oldest.map_or(latest_seq > after_seq, |value| {
        u64::try_from(value).is_ok_and(|oldest| oldest > after_seq + 1)
    });
    // CASE is lazy in SQLite: default queries never extract note values. Bounds
    // are checked before allocating strings, including malformed administrator
    // databases; explicit note inclusion must fail rather than truncate.
    let mut statement = tx.prepare(
        "SELECT seq,
         CASE WHEN length(user_id)=36 THEN user_id ELSE NULL END,
         CASE WHEN key_id IS NULL OR length(key_id)=36 THEN key_id ELSE '' END,
         CASE WHEN request_id IS NULL OR length(request_id)=36 THEN request_id ELSE '' END,
         CASE WHEN length(CAST(action AS BLOB)) BETWEEN 1 AND 128 THEN action ELSE NULL END,
         created_ms,
         CASE WHEN ?4 THEN CASE WHEN length(CAST(note AS BLOB)) BETWEEN 1 AND 512 THEN note ELSE NULL END ELSE NULL END,
         CASE WHEN ?4 THEN note IS NOT NULL AND length(CAST(note AS BLOB)) NOT BETWEEN 1 AND 512 ELSE 0 END
         FROM audit_events WHERE user_id=?1 AND seq>?2 ORDER BY seq ASC LIMIT ?3",
    )?;
    let mut rows = statement.query(params![
        user_id.to_string(),
        i64::try_from(after_seq)?,
        i64::try_from(limit + 1)?,
        include_notes,
    ])?;
    let mut events = Vec::with_capacity(limit + 1);
    let mut previous = after_seq;
    while let Some(row) = rows.next()? {
        let seq = u64::try_from(row.get::<_, i64>(0)?)?;
        ensure!(
            seq > previous && seq <= latest_seq,
            "invalid gateway audit sequence"
        );
        let owner = audit_identity(
            row.get::<_, Option<String>>(1)?
                .context("invalid gateway audit user identity")?,
        )?;
        ensure!(owner == user_id.to_string(), "invalid gateway audit owner");
        let key_id = row
            .get::<_, Option<String>>(2)?
            .map(audit_identity)
            .transpose()?;
        let request_id = row
            .get::<_, Option<String>>(3)?
            .map(audit_identity)
            .transpose()?;
        let action = row
            .get::<_, Option<String>>(4)?
            .context("invalid gateway audit action size")?;
        ensure!(
            !action.chars().any(char::is_control),
            "invalid gateway audit action"
        );
        let created_ms = row.get::<_, i64>(5)?;
        ensure!(created_ms >= 0, "invalid gateway audit timestamp");
        let note = row.get::<_, Option<String>>(6)?;
        ensure!(!row.get::<_, bool>(7)?, "invalid gateway audit note size");
        events.push(AuditEventSummary {
            seq: seq.to_string(),
            user_id,
            key_id,
            request_id,
            action,
            created_ms,
            note,
        });
        previous = seq;
    }
    let has_more = events.len() > limit;
    if has_more {
        events.pop();
    }
    let next_after_seq = if has_more {
        events
            .last()
            .context("missing gateway audit page cursor")?
            .seq
            .clone()
    } else {
        latest_seq.to_string()
    };
    let page = AuditPage {
        user_id,
        events,
        next_after_seq,
        has_more,
        oldest_retained_seq: oldest.map(|seq| seq.to_string()),
        latest_seq: latest_seq.to_string(),
        retention_gap,
        notes_included: include_notes,
    };
    ensure!(
        serde_json::to_vec(&page)?.len() <= 512 * 1024,
        "gateway audit page exceeds 512 KiB"
    );
    Ok(page)
}

fn audit(
    tx: &Transaction<'_>,
    user: Uuid,
    key: Option<Uuid>,
    request: Option<Uuid>,
    action: &str,
    note: Option<&str>,
    now: i64,
) -> Result<()> {
    tx.execute("INSERT INTO audit_events(user_id,key_id,request_id,action,note,created_ms) VALUES(?1,?2,?3,?4,?5,?6)", params![user.to_string(),key.map(|id|id.to_string()),request.map(|id|id.to_string()),action,note,now])?;
    tx.execute("DELETE FROM audit_events WHERE seq NOT IN (SELECT seq FROM audit_events ORDER BY seq DESC LIMIT ?1)", [MAX_AUDIT_EVENTS])?;
    Ok(())
}

const SCHEMA: &str = "
CREATE TABLE users(id TEXT PRIMARY KEY NOT NULL, backend_id TEXT NOT NULL UNIQUE,
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL);
CREATE TABLE api_keys(id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
 verifier BLOB NOT NULL CHECK(length(verifier)=32), created_ms INTEGER NOT NULL, revoked_ms INTEGER);
CREATE INDEX keys_by_user ON api_keys(user_id,revoked_ms);
CREATE TABLE write_holds(user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
 request_id TEXT NOT NULL UNIQUE, state TEXT NOT NULL CHECK(state IN ('in_flight','needs_review')),
 reason TEXT NOT NULL CHECK(reason IN ('request_in_flight','backend_outcome_unknown','gateway_restarted')),
 admitted_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL);
CREATE TABLE audit_events(seq INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL,
 key_id TEXT, request_id TEXT, action TEXT NOT NULL, note TEXT CHECK(note IS NULL OR length(CAST(note AS BLOB)) BETWEEN 1 AND 512), created_ms INTEGER NOT NULL);
";

// A user and a bot remain reserved for their original binding for the registry's
// lifetime. Database constraints also prevent accidental future update/delete code
// from weakening that ownership or reactivating a revoked binding.
const SCHEMA_V2: &str = "
CREATE UNIQUE INDEX users_identity_backend ON users(id,backend_id);
CREATE TABLE telegram_bindings(
 id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL UNIQUE, backend_id TEXT NOT NULL,
 bot_id TEXT NOT NULL UNIQUE CHECK(length(bot_id) BETWEEN 1 AND 19 AND CAST(bot_id AS INTEGER)>0 AND bot_id=CAST(CAST(bot_id AS INTEGER) AS TEXT)),
 sender_id TEXT NOT NULL CHECK(length(sender_id) BETWEEN 1 AND 19 AND CAST(sender_id AS INTEGER)>0 AND sender_id=CAST(CAST(sender_id AS INTEGER) AS TEXT)),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
 FOREIGN KEY(user_id,backend_id) REFERENCES users(id,backend_id) ON DELETE RESTRICT);
CREATE TRIGGER telegram_binding_immutable BEFORE UPDATE OF id,user_id,backend_id,bot_id,sender_id ON telegram_bindings
 WHEN NEW.id<>OLD.id OR NEW.user_id<>OLD.user_id OR NEW.backend_id<>OLD.backend_id OR NEW.bot_id<>OLD.bot_id OR NEW.sender_id<>OLD.sender_id
 BEGIN SELECT RAISE(ABORT,'Telegram binding ownership is immutable'); END;
CREATE TRIGGER telegram_binding_no_reactivate BEFORE UPDATE OF enabled ON telegram_bindings WHEN OLD.enabled=0 AND NEW.enabled<>0
 BEGIN SELECT RAISE(ABORT,'Telegram binding revocation is permanent'); END;
CREATE TRIGGER telegram_binding_no_delete BEFORE DELETE ON telegram_bindings
 BEGIN SELECT RAISE(ABORT,'Telegram binding reservations are permanent'); END;
";

// Existing credentials retain full access. Access changes require issuing a new
// key; rotation copies the persisted bit rather than accepting a new permission.
const SCHEMA_V3: &str = "
ALTER TABLE api_keys ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0 CHECK(read_only IN (0,1));
CREATE TRIGGER api_key_access_immutable BEFORE UPDATE OF read_only ON api_keys
 WHEN NEW.read_only IS NOT OLD.read_only
 BEGIN SELECT RAISE(ABORT,'gateway key access is immutable'); END;
";

// Each user requires a dedicated app: Slack's Events Request URL and signing
// secret belong to the app, including all its workspace installations.
// App and user reservations remain permanent after revocation.
const SCHEMA_V4: &str = "
CREATE TABLE slack_bindings(
 id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL UNIQUE, backend_id TEXT NOT NULL,
 team_id TEXT NOT NULL CHECK(length(team_id) BETWEEN 2 AND 64 AND length(team_id)=length(CAST(team_id AS BLOB)) AND substr(team_id,1,1)='T' AND team_id NOT GLOB '*[^A-Z0-9]*'),
 app_id TEXT NOT NULL CHECK(length(app_id) BETWEEN 2 AND 64 AND length(app_id)=length(CAST(app_id AS BLOB)) AND substr(app_id,1,1)='A' AND app_id NOT GLOB '*[^A-Z0-9]*'),
 bot_user_id TEXT NOT NULL CHECK(length(bot_user_id) BETWEEN 2 AND 64 AND length(bot_user_id)=length(CAST(bot_user_id AS BLOB)) AND substr(bot_user_id,1,1) IN ('U','W') AND bot_user_id NOT GLOB '*[^A-Z0-9]*'),
 bot_id TEXT NOT NULL CHECK(length(bot_id) BETWEEN 2 AND 64 AND length(bot_id)=length(CAST(bot_id AS BLOB)) AND substr(bot_id,1,1)='B' AND bot_id NOT GLOB '*[^A-Z0-9]*'),
 sender_id TEXT NOT NULL CHECK(length(sender_id) BETWEEN 2 AND 64 AND length(sender_id)=length(CAST(sender_id AS BLOB)) AND substr(sender_id,1,1) IN ('U','W') AND sender_id NOT GLOB '*[^A-Z0-9]*' AND sender_id<>bot_user_id),
 conversation_id TEXT NOT NULL CHECK(length(conversation_id) BETWEEN 2 AND 64 AND length(conversation_id)=length(CAST(conversation_id AS BLOB)) AND substr(conversation_id,1,1)='D' AND conversation_id NOT GLOB '*[^A-Z0-9]*'),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
 UNIQUE(app_id),
 FOREIGN KEY(user_id,backend_id) REFERENCES users(id,backend_id) ON DELETE RESTRICT);
CREATE TRIGGER slack_binding_lifetime_limit BEFORE INSERT ON slack_bindings WHEN (SELECT count(*) FROM slack_bindings)>=32
 BEGIN SELECT RAISE(ABORT,'Slack lifetime binding limit reached'); END;
CREATE TRIGGER slack_binding_no_replace BEFORE INSERT ON slack_bindings
 WHEN EXISTS(SELECT 1 FROM slack_bindings WHERE id=NEW.id OR user_id=NEW.user_id OR app_id=NEW.app_id)
 BEGIN SELECT RAISE(ABORT,'Slack binding reservations are permanent'); END;
CREATE TRIGGER slack_binding_immutable BEFORE UPDATE OF id,user_id,backend_id,team_id,app_id,bot_user_id,bot_id,sender_id,conversation_id ON slack_bindings
 WHEN NEW.id IS NOT OLD.id OR NEW.user_id IS NOT OLD.user_id OR NEW.backend_id IS NOT OLD.backend_id OR NEW.team_id IS NOT OLD.team_id OR NEW.app_id IS NOT OLD.app_id OR NEW.bot_user_id IS NOT OLD.bot_user_id OR NEW.bot_id IS NOT OLD.bot_id OR NEW.sender_id IS NOT OLD.sender_id OR NEW.conversation_id IS NOT OLD.conversation_id
 BEGIN SELECT RAISE(ABORT,'Slack binding ownership is immutable'); END;
CREATE TRIGGER slack_binding_no_reactivate BEFORE UPDATE OF enabled ON slack_bindings WHEN OLD.enabled=0 AND NEW.enabled<>0
 BEGIN SELECT RAISE(ABORT,'Slack binding revocation is permanent'); END;
CREATE TRIGGER slack_binding_no_delete BEFORE DELETE ON slack_bindings
 BEGIN SELECT RAISE(ABORT,'Slack binding reservations are permanent'); END;
";

// Public application IDs and user reservations are never reused, even after revocation.
const SCHEMA_V5: &str = "
CREATE TABLE discord_bindings(
 id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL UNIQUE, backend_id TEXT NOT NULL,
 application_id TEXT NOT NULL CHECK(length(application_id) BETWEEN 1 AND 20 AND length(application_id)=length(CAST(application_id AS BLOB)) AND substr(application_id,1,1) BETWEEN '1' AND '9' AND application_id NOT GLOB '*[^0-9]*' AND (length(application_id)<20 OR application_id<='18446744073709551615')),
 verify_key TEXT NOT NULL CHECK(length(verify_key)=64 AND length(CAST(verify_key AS BLOB))=64 AND verify_key NOT GLOB '*[^0-9a-f]*' AND verify_key<>'0000000000000000000000000000000000000000000000000000000000000000'),
 bot_user_id TEXT NOT NULL CHECK(length(bot_user_id) BETWEEN 1 AND 20 AND length(bot_user_id)=length(CAST(bot_user_id AS BLOB)) AND substr(bot_user_id,1,1) BETWEEN '1' AND '9' AND bot_user_id NOT GLOB '*[^0-9]*' AND (length(bot_user_id)<20 OR bot_user_id<='18446744073709551615')),
 sender_id TEXT NOT NULL CHECK(length(sender_id) BETWEEN 1 AND 20 AND length(sender_id)=length(CAST(sender_id AS BLOB)) AND substr(sender_id,1,1) BETWEEN '1' AND '9' AND sender_id NOT GLOB '*[^0-9]*' AND (length(sender_id)<20 OR sender_id<='18446744073709551615') AND sender_id<>bot_user_id),
 conversation_id TEXT NOT NULL CHECK(length(conversation_id) BETWEEN 1 AND 20 AND length(conversation_id)=length(CAST(conversation_id AS BLOB)) AND substr(conversation_id,1,1) BETWEEN '1' AND '9' AND conversation_id NOT GLOB '*[^0-9]*' AND (length(conversation_id)<20 OR conversation_id<='18446744073709551615')),
 command_id TEXT NOT NULL CHECK(length(command_id) BETWEEN 1 AND 20 AND length(command_id)=length(CAST(command_id AS BLOB)) AND substr(command_id,1,1) BETWEEN '1' AND '9' AND command_id NOT GLOB '*[^0-9]*' AND (length(command_id)<20 OR command_id<='18446744073709551615')),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
 UNIQUE(application_id),
 FOREIGN KEY(user_id,backend_id) REFERENCES users(id,backend_id) ON DELETE RESTRICT);
CREATE TRIGGER discord_binding_lifetime_limit BEFORE INSERT ON discord_bindings WHEN (SELECT count(*) FROM discord_bindings)>=32
 BEGIN SELECT RAISE(ABORT,'Discord lifetime binding limit reached'); END;
CREATE TRIGGER discord_binding_no_replace BEFORE INSERT ON discord_bindings
 WHEN EXISTS(SELECT 1 FROM discord_bindings WHERE id=NEW.id OR user_id=NEW.user_id OR application_id=NEW.application_id)
 BEGIN SELECT RAISE(ABORT,'Discord binding reservations are permanent'); END;
CREATE TRIGGER discord_binding_immutable BEFORE UPDATE OF id,user_id,backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id ON discord_bindings
 WHEN NEW.id IS NOT OLD.id OR NEW.user_id IS NOT OLD.user_id OR NEW.backend_id IS NOT OLD.backend_id OR NEW.application_id IS NOT OLD.application_id OR NEW.verify_key IS NOT OLD.verify_key OR NEW.bot_user_id IS NOT OLD.bot_user_id OR NEW.sender_id IS NOT OLD.sender_id OR NEW.conversation_id IS NOT OLD.conversation_id OR NEW.command_id IS NOT OLD.command_id
 BEGIN SELECT RAISE(ABORT,'Discord binding ownership is immutable'); END;
CREATE TRIGGER discord_binding_no_reactivate BEFORE UPDATE OF enabled ON discord_bindings WHEN OLD.enabled=0 AND NEW.enabled<>0
 BEGIN SELECT RAISE(ABORT,'Discord binding revocation is permanent'); END;
CREATE TRIGGER discord_binding_no_delete BEFORE DELETE ON discord_bindings
 BEGIN SELECT RAISE(ABORT,'Discord binding reservations are permanent'); END;
";

// One enterprise application's callback credentials and bot are dedicated to one
// user for the registry lifetime, including all revoked installations.
const SCHEMA_V6: &str = "
CREATE TABLE feishu_bindings(
 id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL UNIQUE, backend_id TEXT NOT NULL,
 app_id TEXT NOT NULL UNIQUE CHECK(length(app_id) BETWEEN 5 AND 128 AND length(app_id)=length(CAST(app_id AS BLOB)) AND substr(app_id,1,4)='cli_' AND app_id NOT GLOB '*[^A-Za-z0-9_-]*'),
 tenant_key TEXT NOT NULL CHECK(length(tenant_key) BETWEEN 1 AND 128 AND length(tenant_key)=length(CAST(tenant_key AS BLOB)) AND tenant_key NOT GLOB '*[^A-Za-z0-9_-]*' AND length(app_id)+length(tenant_key)+1<=128),
 bot_open_id TEXT NOT NULL CHECK(length(bot_open_id) BETWEEN 4 AND 128 AND length(bot_open_id)=length(CAST(bot_open_id AS BLOB)) AND substr(bot_open_id,1,3)='ou_' AND bot_open_id NOT GLOB '*[^A-Za-z0-9_-]*'),
 human_open_id TEXT NOT NULL CHECK(length(human_open_id) BETWEEN 4 AND 128 AND length(human_open_id)=length(CAST(human_open_id AS BLOB)) AND substr(human_open_id,1,3)='ou_' AND human_open_id NOT GLOB '*[^A-Za-z0-9_-]*' AND human_open_id<>bot_open_id),
 chat_id TEXT NOT NULL CHECK(length(chat_id) BETWEEN 4 AND 128 AND length(chat_id)=length(CAST(chat_id AS BLOB)) AND substr(chat_id,1,3)='oc_' AND chat_id NOT GLOB '*[^A-Za-z0-9_-]*'),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
 FOREIGN KEY(user_id,backend_id) REFERENCES users(id,backend_id) ON DELETE RESTRICT);
CREATE TRIGGER feishu_binding_lifetime_limit BEFORE INSERT ON feishu_bindings WHEN (SELECT count(*) FROM feishu_bindings)>=32
 BEGIN SELECT RAISE(ABORT,'Feishu lifetime binding limit reached'); END;
CREATE TRIGGER feishu_binding_no_replace BEFORE INSERT ON feishu_bindings
 WHEN EXISTS(SELECT 1 FROM feishu_bindings WHERE id=NEW.id OR user_id=NEW.user_id OR app_id=NEW.app_id)
 BEGIN SELECT RAISE(ABORT,'Feishu binding reservations are permanent'); END;
CREATE TRIGGER feishu_binding_immutable BEFORE UPDATE OF id,user_id,backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id ON feishu_bindings
 WHEN NEW.id IS NOT OLD.id OR NEW.user_id IS NOT OLD.user_id OR NEW.backend_id IS NOT OLD.backend_id OR NEW.app_id IS NOT OLD.app_id OR NEW.tenant_key IS NOT OLD.tenant_key OR NEW.bot_open_id IS NOT OLD.bot_open_id OR NEW.human_open_id IS NOT OLD.human_open_id OR NEW.chat_id IS NOT OLD.chat_id
 BEGIN SELECT RAISE(ABORT,'Feishu binding ownership is immutable'); END;
CREATE TRIGGER feishu_binding_no_reactivate BEFORE UPDATE OF enabled ON feishu_bindings WHEN OLD.enabled=0 AND NEW.enabled<>0
 BEGIN SELECT RAISE(ABORT,'Feishu binding revocation is permanent'); END;
CREATE TRIGGER feishu_binding_no_delete BEFORE DELETE ON feishu_bindings
 BEGIN SELECT RAISE(ABORT,'Feishu binding reservations are permanent'); END;
";

// CorpID identifies the enterprise; AgentID identifies its dedicated application.
// Revocation retains both reservations and the one-member ownership forever.
const SCHEMA_V7: &str = "
CREATE TABLE wecom_bindings(
 id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL UNIQUE, backend_id TEXT NOT NULL,
 corp_id TEXT NOT NULL CHECK(length(corp_id) BETWEEN 1 AND 64 AND length(corp_id)=length(CAST(corp_id AS BLOB)) AND corp_id NOT GLOB '*[^A-Za-z0-9_-]*'),
 agent_id INTEGER NOT NULL CHECK(typeof(agent_id)='integer' AND agent_id BETWEEN 1 AND 2147483647),
 human_user_id TEXT NOT NULL CHECK(length(human_user_id) BETWEEN 1 AND 64 AND length(human_user_id)=length(CAST(human_user_id AS BLOB)) AND substr(human_user_id,1,1) GLOB '[a-z0-9]' AND human_user_id NOT GLOB '*[^a-z0-9_.@-]*'),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
 UNIQUE(corp_id,agent_id), FOREIGN KEY(user_id,backend_id) REFERENCES users(id,backend_id) ON DELETE RESTRICT);
CREATE TRIGGER wecom_binding_lifetime_limit BEFORE INSERT ON wecom_bindings WHEN (SELECT count(*) FROM wecom_bindings)>=32
 BEGIN SELECT RAISE(ABORT,'WeCom lifetime binding limit reached'); END;
CREATE TRIGGER wecom_binding_no_replace BEFORE INSERT ON wecom_bindings
 WHEN EXISTS(SELECT 1 FROM wecom_bindings WHERE id=NEW.id OR user_id=NEW.user_id OR (corp_id=NEW.corp_id AND agent_id=NEW.agent_id))
 BEGIN SELECT RAISE(ABORT,'WeCom binding reservations are permanent'); END;
CREATE TRIGGER wecom_binding_immutable BEFORE UPDATE OF id,user_id,backend_id,corp_id,agent_id,human_user_id ON wecom_bindings
 WHEN NEW.id IS NOT OLD.id OR NEW.user_id IS NOT OLD.user_id OR NEW.backend_id IS NOT OLD.backend_id OR NEW.corp_id IS NOT OLD.corp_id OR NEW.agent_id IS NOT OLD.agent_id OR NEW.human_user_id IS NOT OLD.human_user_id
 BEGIN SELECT RAISE(ABORT,'WeCom binding ownership is immutable'); END;
CREATE TRIGGER wecom_binding_no_reactivate BEFORE UPDATE OF enabled ON wecom_bindings WHEN OLD.enabled=0 AND NEW.enabled<>0
 BEGIN SELECT RAISE(ABORT,'WeCom binding revocation is permanent'); END;
CREATE TRIGGER wecom_binding_no_delete BEFORE DELETE ON wecom_bindings
 BEGIN SELECT RAISE(ABORT,'WeCom binding reservations are permanent'); END;
";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    struct Fixture {
        root: PathBuf,
        registry: Registry,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("jiaclaw-gateway-registry-{}", Uuid::new_v4()));
            let registry = Registry::open(&root.join("users.sqlite3")).unwrap();
            Self { root, registry }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn principal(registry: &Registry, issued: &IssuedKey) -> Principal {
        registry.authenticate(&issued.token).unwrap().unwrap()
    }

    fn wecom_binding(registry: &Registry, user: Uuid, number: u32) -> WecomBindingSummary {
        registry
            .add_wecom_binding(user, "wwEnterprise", number, "alice.member")
            .unwrap()
    }

    #[test]
    fn wecom_permanent_application_ownership_and_revocation_survive_restart() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let carol = registry.add_user("carol").unwrap();
        let binding = wecom_binding(registry, alice.user_id, 1);
        assert_eq!(
            registry.wecom_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        // AgentID is scoped to CorpID; a different application in the same Corp is independent.
        let same_corp = wecom_binding(registry, bob.user_id, 2);
        let other_corp = registry
            .add_wecom_binding(carol.user_id, "wwOther", 1, "alice.member")
            .unwrap();
        assert_ne!(same_corp.id, other_corp.id);
        let conn = registry.connection().unwrap();
        for sql in [
            "UPDATE wecom_bindings SET id='12345678-1234-4234-9234-123456789012'",
            "UPDATE wecom_bindings SET user_id='12345678-1234-4234-9234-123456789012'",
            "UPDATE wecom_bindings SET backend_id='other'",
            "UPDATE wecom_bindings SET corp_id='wwChanged'",
            "UPDATE wecom_bindings SET agent_id=3",
            "UPDATE wecom_bindings SET human_user_id='other.member'",
            "DELETE FROM wecom_bindings",
            "INSERT OR REPLACE INTO wecom_bindings SELECT * FROM wecom_bindings",
        ] {
            assert!(conn.execute_batch(sql).is_err(), "{sql}");
        }
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry.wecom_authorized(binding.id).unwrap().is_none());
        registry.set_enabled(alice.user_id, true).unwrap();
        let request = Uuid::now_v7();
        registry
            .admit_wecom(
                binding.id,
                request,
                "wecom_execute",
                &binding.id.to_string(),
            )
            .unwrap();
        registry.revoke_wecom_binding(binding.id).unwrap();
        registry.revoke_wecom_binding(binding.id).unwrap();
        assert!(registry.wecom_authorized(binding.id).unwrap().is_none());
        assert!(!registry.wecom_binding(binding.id).unwrap().unwrap().enabled);
        assert!(registry
            .add_wecom_binding(alice.user_id, "wwNew", 7, "new.member")
            .is_err());
        let dave = registry.add_user("dave").unwrap();
        assert!(registry
            .add_wecom_binding(dave.user_id, "wwEnterprise", 1, "dave")
            .is_err());
        assert!(conn
            .execute(
                "UPDATE wecom_bindings SET enabled=1 WHERE id=?1",
                [binding.id.to_string()]
            )
            .is_err());
        registry.rotate(alice.key_id).unwrap();
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.list_wecom_bindings().unwrap().len(), 3);
        assert!(!reopened.wecom_binding(binding.id).unwrap().unwrap().enabled);
        assert_eq!(
            reopened
                .list()
                .unwrap()
                .into_iter()
                .find(|u| u.user_id == alice.user_id)
                .unwrap()
                .hold
                .unwrap()
                .request_id,
            request
        );
    }

    #[test]
    fn wecom_invalid_platform_metadata_cannot_create_binding_audit_or_hold() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let before =
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap();
        for (corp, agent, human) in [
            ("", 1, "alice"),
            ("ww/other", 1, "alice"),
            ("ww界", 1, "alice"),
            ("wwCorp", 0, "alice"),
            ("wwCorp", i32::MAX as u32 + 1, "alice"),
            ("wwCorp", u32::MAX, "alice"),
            ("wwCorp", 1, "Alice"),
            ("wwCorp", 1, "@all"),
            ("wwCorp", 1, "_alice"),
            ("wwCorp", 1, "alice|bob"),
            ("wwCorp", 1, "界"),
        ] {
            assert!(registry
                .add_wecom_binding(user.user_id, corp, agent, human)
                .is_err());
        }
        assert!(registry
            .add_wecom_binding(user.user_id, &"w".repeat(65), 1, "alice")
            .is_err());
        assert!(registry
            .add_wecom_binding(user.user_id, "wwCorp", 1, &"a".repeat(65))
            .is_err());
        assert_eq!(
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap(),
            before
        );
        registry.set_enabled(user.user_id, false).unwrap();
        assert!(registry
            .add_wecom_binding(user.user_id, "wwCorp", 1, "alice")
            .is_err());
        registry.set_enabled(user.user_id, true).unwrap();
        // Compare before user-state audit entries are added.
        assert_eq!(
            registry
                .audit_list(user.user_id, 0, 100, true)
                .unwrap()
                .events
                .len(),
            before["events"].as_array().unwrap().len() + 2
        );
        assert!(registry.list_wecom_bindings().unwrap().is_empty());
        assert!(registry.list().unwrap()[0].hold.is_none());
        let binding = registry
            .add_wecom_binding(
                user.user_id,
                &"W".repeat(64),
                i32::MAX as u32,
                &"a".repeat(64),
            )
            .unwrap();
        assert_eq!(binding.agent_id, i32::MAX as u32);
    }

    #[test]
    fn wecom_admission_rechecks_current_auth_and_shares_every_channel_hold() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user_with_access("alice", true).unwrap();
        let alice_read = principal(registry, &alice);
        let full = registry.add_key(alice.user_id).unwrap();
        let alice_full = principal(registry, &full);
        let binding = wecom_binding(registry, alice.user_id, 1);
        let slack = slack_binding(registry, alice.user_id, 1);
        let discord = discord_binding(registry, alice.user_id, 1);
        let feishu = feishu_binding(registry, alice.user_id, 1);
        let telegram = registry
            .add_telegram_binding(alice.user_id, "123", "456")
            .unwrap();
        for (request, operation, object) in [
            (Uuid::new_v4(), "wecom_execute", binding.id.to_string()),
            (Uuid::now_v7(), "write", binding.id.to_string()),
            (Uuid::now_v7(), "wecom_send", Uuid::nil().to_string()),
            (
                Uuid::now_v7(),
                "wecom_execute",
                "ABCDEF01-2345-4678-9ABC-DEF012345678".into(),
            ),
            (Uuid::now_v7(), "wecom_execute", "not-uuid".into()),
        ] {
            assert!(registry
                .admit_wecom(binding.id, request, operation, &object)
                .is_err());
        }
        assert!(registry.list().unwrap()[0].hold.is_none());
        let request = Uuid::now_v7();
        let object = Uuid::new_v4().to_string();
        registry
            .admit_wecom(binding.id, request, "wecom_send", &object)
            .unwrap();
        assert_eq!(
            registry
                .admit_write(&alice_read, Uuid::now_v7())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::ReadOnly)
        );
        assert_eq!(
            registry
                .admit_write(&alice_full, Uuid::now_v7())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        assert_eq!(
            registry
                .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        assert!(registry
            .admit_telegram(telegram.id, Uuid::now_v7(), "telegram_send", &object)
            .is_err());
        assert!(registry
            .admit_slack(slack.id, Uuid::now_v7(), "slack_send", &object)
            .is_err());
        assert!(registry
            .admit_discord(discord.id, Uuid::now_v7(), "discord_send", &object)
            .is_err());
        assert!(registry
            .admit_feishu(feishu.id, Uuid::now_v7(), "feishu_send", &object)
            .is_err());
        let audit = registry.audit_list(alice.user_id, 0, 100, true).unwrap();
        let entry = audit
            .events
            .iter()
            .find(|e| e.action == "wecom_send")
            .unwrap();
        assert_eq!(entry.request_id, Some(request.to_string()));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(entry.note.as_ref().unwrap()).unwrap(),
            serde_json::json!({"binding_id":binding.id.to_string(),"object_id":object})
        );
        registry.finish_write(alice.user_id, request, true).unwrap();
        let http = Uuid::now_v7();
        registry.admit_write(&alice_full, http).unwrap();
        assert_eq!(
            registry
                .admit_wecom(binding.id, Uuid::now_v7(), "wecom_execute", &object)
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        registry.finish_write(alice.user_id, http, false).unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        assert_eq!(
            registry
                .admit_wecom(binding.id, Uuid::now_v7(), "wecom_execute", &object)
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        assert!(registry.list().unwrap()[0].hold.is_some());
    }

    #[test]
    fn wecom_binding_and_admission_rollback_with_audit_failure() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        let blocker="CREATE TRIGGER fail_wecom_audit BEFORE INSERT ON audit_events WHEN NEW.action LIKE 'wecom_%' BEGIN SELECT RAISE(ABORT,'fixture audit failure'); END;";
        conn.execute_batch(blocker).unwrap();
        assert!(registry
            .add_wecom_binding(user.user_id, "wwCorp", 1, "alice")
            .is_err());
        assert!(registry.list_wecom_bindings().unwrap().is_empty());
        conn.execute_batch("DROP TRIGGER fail_wecom_audit").unwrap();
        let binding = wecom_binding(registry, user.user_id, 1);
        conn.execute_batch(blocker).unwrap();
        assert!(registry.revoke_wecom_binding(binding.id).is_err());
        assert!(registry.wecom_authorized(binding.id).unwrap().is_some());
        assert!(registry
            .admit_wecom(
                binding.id,
                Uuid::now_v7(),
                "wecom_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
    }

    #[test]
    fn wecom_lifetime_capacity_and_sql_validation_include_revocations() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let mut first = None;
        for number in 1..=32 {
            let user = registry.add_user(&format!("backend-{number}")).unwrap();
            let binding = wecom_binding(registry, user.user_id, number);
            registry.revoke_wecom_binding(binding.id).unwrap();
            first.get_or_insert(user);
        }
        let first = first.unwrap();
        let error = registry
            .add_wecom_binding(first.user_id, "wwNew", 33, "new")
            .unwrap_err();
        assert!(error.to_string().contains("lifetime binding limit"));
        assert_eq!(registry.list_wecom_bindings().unwrap().len(), 32);
        let conn = registry.connection().unwrap();
        assert!(conn.execute("INSERT INTO wecom_bindings(id,user_id,backend_id,corp_id,agent_id,human_user_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'backend-1','wwNew',33,'new',1,0,0)",params![Uuid::new_v4().to_string(),first.user_id.to_string()]).is_err());
        let other = Fixture::new();
        let user = other.registry.add_user("alice").unwrap();
        let conn = other.registry.connection().unwrap();
        for (corp, agent, human) in [
            ("", 1i64, "alice"),
            ("ww/Corp", 1, "alice"),
            ("wwCorp", 0, "alice"),
            ("wwCorp", 2147483648, "alice"),
            ("wwCorp", 1, "Alice"),
            ("wwCorp", 1, "@all"),
            ("wwCorp", 1, "alice|bob"),
            ("ww界", 1, "alice"),
            ("wwCorp", 1, "a界"),
        ] {
            assert!(conn.execute("INSERT INTO wecom_bindings(id,user_id,backend_id,corp_id,agent_id,human_user_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'alice',?3,?4,?5,1,0,0)",params![Uuid::new_v4().to_string(),user.user_id.to_string(),corp,agent,human]).is_err());
        }
    }

    #[test]
    fn wecom_corrupt_owner_rows_fail_closed_without_admission_effects() {
        for (column, value) in [
            ("corp_id", "bad corp"),
            ("agent_id", "0"),
            ("agent_id", "2147483648"),
            ("human_user_id", "Alice"),
            ("human_user_id", "@all"),
            ("backend_id", "bad/backend"),
            ("id", "not-uuid"),
            ("enabled", "2"),
        ] {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let user = registry.add_user("alice").unwrap();
            let binding = wecom_binding(registry, user.user_id, 1);
            let before = registry
                .audit_list(user.user_id, 0, 100, true)
                .unwrap()
                .events
                .len();
            let conn = registry.connection().unwrap();
            conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON; DROP TRIGGER wecom_binding_immutable").unwrap();
            conn.execute(&format!("UPDATE wecom_bindings SET {column}=?1"), [value])
                .unwrap();
            assert!(registry
                .wecom_authorized(binding.id)
                .map(|b| b.is_none())
                .unwrap_or(true));
            assert!(registry
                .admit_wecom(
                    binding.id,
                    Uuid::now_v7(),
                    "wecom_execute",
                    &binding.id.to_string()
                )
                .is_err());
            assert!(registry.list().unwrap()[0].hold.is_none());
            assert_eq!(
                registry
                    .audit_list(user.user_id, 0, 100, true)
                    .unwrap()
                    .events
                    .len(),
                before
            );
        }
    }

    #[test]
    fn wecom_simultaneous_application_reservations_have_one_winner() {
        let fixture = Fixture::new();
        let registry = Arc::new(fixture.registry.clone());
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles = [alice.user_id, bob.user_id].map(|user| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                registry
                    .add_wecom_binding(user, "wwCorp", 1, "human")
                    .is_ok()
            })
        });
        assert_eq!(
            handles
                .into_iter()
                .map(|h| usize::from(h.join().unwrap()))
                .sum::<usize>(),
            1
        );
        assert_eq!(registry.list_wecom_bindings().unwrap().len(), 1);
        let winner = registry.list_wecom_bindings().unwrap().remove(0).user_id;
        let loser = if winner == alice.user_id {
            bob.user_id
        } else {
            alice.user_id
        };
        assert_eq!(
            registry
                .audit_list(loser, 0, 100, true)
                .unwrap()
                .events
                .len(),
            1
        );
    }

    fn downgrade_before_wecom(conn: &Connection, version: i64) {
        if version < 6 {
            downgrade_before_feishu(conn, version);
        } else {
            conn.execute_batch("DROP TABLE wecom_bindings").unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
        }
    }

    #[test]
    fn wecom_schema_seven_preserves_all_previous_bindings_permissions_audit_and_hold() {
        for version in 1..=6 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry
                .add_user_with_access("alice", version >= 3)
                .unwrap();
            let bob = registry.add_user("bob").unwrap();
            registry.revoke(bob.key_id).unwrap();
            registry.set_enabled(bob.user_id, false).unwrap();
            if version >= 2 {
                let b = registry
                    .add_telegram_binding(alice.user_id, "123", "456")
                    .unwrap();
                registry.revoke_telegram_binding(b.id).unwrap();
            }
            if version >= 4 {
                let b = slack_binding(registry, alice.user_id, 1);
                registry.revoke_slack_binding(b.id).unwrap();
            }
            if version >= 5 {
                let b = discord_binding(registry, alice.user_id, 1);
                registry.revoke_discord_binding(b.id).unwrap();
            }
            if version >= 6 {
                let b = feishu_binding(registry, alice.user_id, 1);
                registry.revoke_feishu_binding(b.id).unwrap();
            }
            let request = Uuid::now_v7();
            registry
                .admit_scheduled(alice.user_id, "alice", request)
                .unwrap();
            registry
                .finish_write(alice.user_id, request, false)
                .unwrap();
            let users = serde_json::to_value(registry.list().unwrap()).unwrap();
            let keys =
                serde_json::to_value(registry.list_keys(alice.user_id, 100, 0).unwrap()).unwrap();
            let telegram = registry.list_telegram_bindings().unwrap();
            let slack = registry.list_slack_bindings().unwrap();
            let discord = registry.list_discord_bindings().unwrap();
            let feishu = registry.list_feishu_bindings().unwrap();
            let audit =
                serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap();
            let conn = registry.connection().unwrap();
            downgrade_before_wecom(&conn, version);
            drop(conn);
            let migrated = Registry::open(&registry.path).unwrap();
            assert_eq!(
                serde_json::to_value(migrated.list().unwrap()).unwrap(),
                users
            );
            assert_eq!(
                serde_json::to_value(migrated.list_keys(alice.user_id, 100, 0).unwrap()).unwrap(),
                keys
            );
            assert_eq!(migrated.list_telegram_bindings().unwrap(), telegram);
            assert_eq!(migrated.list_slack_bindings().unwrap(), slack);
            assert_eq!(migrated.list_discord_bindings().unwrap(), discord);
            assert_eq!(migrated.list_feishu_bindings().unwrap(), feishu);
            assert_eq!(
                serde_json::to_value(migrated.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap(),
                audit
            );
            assert_eq!(principal(&migrated, &alice).read_only, version >= 3);
            assert!(migrated.authenticate(&bob.token).unwrap().is_none());
            assert!(migrated.list_wecom_bindings().unwrap().is_empty());
            let binding = wecom_binding(&migrated, alice.user_id, 1);
            assert_eq!(
                migrated
                    .admit_wecom(
                        binding.id,
                        Uuid::now_v7(),
                        "wecom_execute",
                        &binding.id.to_string()
                    )
                    .unwrap_err()
                    .downcast_ref::<WriteAdmissionError>(),
                Some(&WriteAdmissionError::Held)
            );
            assert_eq!(
                migrated
                    .connection()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                7
            );
        }
    }

    #[test]
    fn wecom_late_migration_failure_rolls_back_all_earlier_steps_and_data() {
        for version in 1..=6 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry.add_user("alice").unwrap();
            let conn = registry.connection().unwrap();
            downgrade_before_wecom(&conn, version);
            conn.execute_batch("CREATE TRIGGER wecom_binding_no_delete BEFORE DELETE ON users BEGIN SELECT RAISE(ABORT,'fixture conflict'); END;").unwrap();
            let schema:String=conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|row|row.get(0)).unwrap();
            let audit: i64 = conn
                .query_row("SELECT count(*) FROM audit_events", [], |row| row.get(0))
                .unwrap();
            assert!(Registry::open(&registry.path).is_err());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|row|row.get::<_,String>(0)).unwrap(),schema);
            assert_eq!(
                conn.query_row("SELECT count(*) FROM audit_events", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                audit
            );
            conn.execute_batch("DROP TRIGGER wecom_binding_no_delete")
                .unwrap();
            drop(conn);
            let migrated = Registry::open(&registry.path).unwrap();
            assert_eq!(principal(&migrated, &alice).backend_id, "alice");
            assert!(migrated.list_wecom_bindings().unwrap().is_empty());
        }
    }

    fn feishu_binding(registry: &Registry, user: Uuid, number: usize) -> FeishuBindingSummary {
        registry
            .add_feishu_binding(
                user,
                &format!("cli_app{number}"),
                &format!("tenant_{number}"),
                &format!("ou_bot{number}"),
                &format!("ou_human{number}"),
                &format!("oc_chat{number}"),
            )
            .unwrap()
    }

    #[test]
    fn feishu_reservations_and_all_owner_fields_remain_permanent_after_revocation() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = feishu_binding(registry, alice.user_id, 1);
        assert_eq!(
            registry.feishu_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        let conn = registry.connection().unwrap();
        for sql in [
            "UPDATE feishu_bindings SET app_id='cli_other'",
            "UPDATE feishu_bindings SET tenant_key='other'",
            "UPDATE feishu_bindings SET bot_open_id='ou_other'",
            "UPDATE feishu_bindings SET human_open_id='ou_other'",
            "UPDATE feishu_bindings SET chat_id='oc_other'",
            "UPDATE feishu_bindings SET backend_id='bob'",
            "DELETE FROM feishu_bindings",
            "INSERT OR REPLACE INTO feishu_bindings SELECT * FROM feishu_bindings",
        ] {
            assert!(conn.execute_batch(sql).is_err(), "{sql}");
        }
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry.feishu_authorized(binding.id).unwrap().is_none());
        registry.set_enabled(alice.user_id, true).unwrap();
        let request = Uuid::now_v7();
        registry
            .admit_feishu(
                binding.id,
                request,
                "feishu_execute",
                &binding.id.to_string(),
            )
            .unwrap();
        registry.revoke_feishu_binding(binding.id).unwrap();
        registry.revoke_feishu_binding(binding.id).unwrap();
        assert!(registry.feishu_authorized(binding.id).unwrap().is_none());
        assert!(
            !registry
                .feishu_binding(binding.id)
                .unwrap()
                .unwrap()
                .enabled
        );
        assert!(registry
            .add_feishu_binding(alice.user_id, "cli_new", "tenant_2", "ou_b", "ou_h", "oc_c")
            .is_err());
        assert!(registry
            .add_feishu_binding(
                bob.user_id,
                &binding.app_id,
                "different_tenant",
                "ou_b",
                "ou_h",
                "oc_c"
            )
            .is_err());
        assert!(conn
            .execute_batch("UPDATE feishu_bindings SET enabled=1")
            .is_err());
        assert_eq!(
            registry
                .list()
                .unwrap()
                .iter()
                .find(|u| u.user_id == alice.user_id)
                .unwrap()
                .hold
                .as_ref()
                .unwrap()
                .request_id,
            request
        );
        registry.rotate(alice.key_id).unwrap();
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.list_feishu_bindings().unwrap().len(), 1);
        assert!(
            !reopened
                .feishu_binding(binding.id)
                .unwrap()
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn feishu_invalid_platform_metadata_cannot_create_binding_hold_or_audit() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let before = registry
            .audit_list(alice.user_id, 0, 100, true)
            .unwrap()
            .events
            .len();
        for (field, value) in [
            (0, "cli_"),
            (0, "app1"),
            (0, "cli_界"),
            (0, "cli_a/b"),
            (1, ""),
            (1, "tenant:other"),
            (1, "tenant\n"),
            (2, "ou_"),
            (2, "ou_界"),
            (3, "ou_bot"),
            (3, "ou_user "),
            (4, "chat"),
            (4, "oc_"),
        ] {
            let mut ids = ["cli_app", "tenant", "ou_bot", "ou_human", "oc_chat"];
            ids[field] = value;
            assert!(registry
                .add_feishu_binding(alice.user_id, ids[0], ids[1], ids[2], ids[3], ids[4])
                .is_err());
        }
        assert!(registry
            .add_feishu_binding(
                alice.user_id,
                &format!("cli_{}", "a".repeat(123)),
                "tt",
                "ou_bot",
                "ou_human",
                "oc_chat"
            )
            .is_err());
        assert!(registry.list_feishu_bindings().unwrap().is_empty());
        assert_eq!(
            registry
                .audit_list(alice.user_id, 0, 100, true)
                .unwrap()
                .events
                .len(),
            before
        );
        assert!(registry.list().unwrap()[0].hold.is_none());
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry
            .add_feishu_binding(
                alice.user_id,
                "cli_app",
                "tenant",
                "ou_bot",
                "ou_human",
                "oc_chat"
            )
            .is_err());
    }

    #[test]
    fn feishu_shared_admission_rechecks_permission_metadata_and_other_effects() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user_with_access("alice", true).unwrap();
        let readonly = principal(registry, &alice);
        let full_key = registry.add_key(alice.user_id).unwrap();
        let full = principal(registry, &full_key);
        let binding = feishu_binding(registry, alice.user_id, 1);
        let slack = slack_binding(registry, alice.user_id, 1);
        let discord = discord_binding(registry, alice.user_id, 1);
        let object = binding.id.to_string();
        for (request, operation, object) in [
            (Uuid::new_v4(), "feishu_execute", object.clone()),
            (Uuid::now_v7(), "arbitrary_write", object.clone()),
            (Uuid::now_v7(), "feishu_send", "om_message".into()),
            (Uuid::now_v7(), "feishu_send", Uuid::nil().to_string()),
            (
                Uuid::now_v7(),
                "feishu_send",
                "ABCDEF01-2345-4678-9ABC-DEF012345678".into(),
            ),
        ] {
            assert!(registry
                .admit_feishu(binding.id, request, operation, &object)
                .is_err());
        }
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert_eq!(
            registry
                .admit_write(&readonly, Uuid::new_v4())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::ReadOnly)
        );
        let request = Uuid::now_v7();
        registry
            .admit_feishu(binding.id, request, "feishu_execute", &object)
            .unwrap();
        for result in [
            registry.admit_write(&full, Uuid::now_v7()).map(|_| ()),
            registry.admit_scheduled(alice.user_id, "alice", Uuid::now_v7()),
            registry
                .admit_slack(slack.id, Uuid::now_v7(), "slack_execute", &object)
                .map(|_| ()),
            registry
                .admit_discord(discord.id, Uuid::now_v7(), "discord_send", &object)
                .map(|_| ()),
        ] {
            assert_eq!(
                result.unwrap_err().downcast_ref::<WriteAdmissionError>(),
                Some(&WriteAdmissionError::Held)
            );
        }
        registry.finish_write(alice.user_id, request, true).unwrap();
        let request = Uuid::now_v7();
        registry.admit_write(&full, request).unwrap();
        assert_eq!(
            registry
                .admit_feishu(binding.id, Uuid::now_v7(), "feishu_send", &object)
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        registry
            .finish_write(alice.user_id, request, false)
            .unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        assert_eq!(
            registry
                .admit_feishu(binding.id, Uuid::now_v7(), "feishu_send", &object)
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        let audit = registry.audit_list(alice.user_id, 0, 100, true).unwrap();
        let entry = audit
            .events
            .iter()
            .find(|e| e.action == "feishu_execute")
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(entry.note.as_ref().unwrap()).unwrap(),
            serde_json::json!({"binding_id":binding.id.to_string(),"object_id":object})
        );
    }

    #[test]
    fn feishu_audit_failure_rolls_back_binding_revocation_and_admission() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        let blocker = "CREATE TRIGGER fail_feishu_audit BEFORE INSERT ON audit_events WHEN NEW.action LIKE 'feishu_%' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;";
        conn.execute_batch(blocker).unwrap();
        assert!(registry
            .add_feishu_binding(
                alice.user_id,
                "cli_app",
                "tenant",
                "ou_bot",
                "ou_human",
                "oc_chat"
            )
            .is_err());
        assert!(registry.list_feishu_bindings().unwrap().is_empty());
        conn.execute_batch("DROP TRIGGER fail_feishu_audit")
            .unwrap();
        let binding = feishu_binding(registry, alice.user_id, 1);
        conn.execute_batch(blocker).unwrap();
        assert!(registry.revoke_feishu_binding(binding.id).is_err());
        assert!(registry.feishu_authorized(binding.id).unwrap().is_some());
        assert!(registry
            .admit_feishu(
                binding.id,
                Uuid::now_v7(),
                "feishu_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
    }

    #[test]
    fn feishu_lifetime_capacity_includes_revocations_and_sql_rejects_invalid_owners() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let mut first = None;
        for number in 0..32 {
            let user = registry.add_user(&format!("backend-{number}")).unwrap();
            let binding = feishu_binding(registry, user.user_id, number);
            registry.revoke_feishu_binding(binding.id).unwrap();
            first.get_or_insert(user);
        }
        let user = first.unwrap();
        let error = registry
            .add_feishu_binding(
                user.user_id,
                "cli_new",
                "tenant",
                "ou_bot",
                "ou_human",
                "oc_chat",
            )
            .unwrap_err();
        assert!(error.to_string().contains("lifetime binding limit"));
        assert_eq!(registry.list_feishu_bindings().unwrap().len(), 32);
        let conn = registry.connection().unwrap();
        assert!(conn.execute("INSERT INTO feishu_bindings(id,user_id,backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'backend-0','cli_new','tenant','ou_bot','ou_human','oc_chat',1,0,0)",params![Uuid::new_v4().to_string(),user.user_id.to_string()]).is_err());
        let other = Fixture::new();
        let user = other.registry.add_user("alice").unwrap();
        let conn = other.registry.connection().unwrap();
        for (app, tenant, bot, human, chat) in [
            ("cli_", "tenant", "ou_bot", "ou_human", "oc_chat"),
            ("cli_app", "bad tenant", "ou_bot", "ou_human", "oc_chat"),
            ("cli_app", "tenant", "ou_bot", "ou_bot", "oc_chat"),
            ("cli_app", "tenant", "ou_bot", "ou_human", "oc_界"),
        ] {
            assert!(conn.execute("INSERT INTO feishu_bindings(id,user_id,backend_id,app_id,tenant_key,bot_open_id,human_open_id,chat_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'alice',?3,?4,?5,?6,?7,1,0,0)",params![Uuid::new_v4().to_string(),user.user_id.to_string(),app,tenant,bot,human,chat]).is_err());
        }
    }

    #[test]
    fn feishu_corrupt_owner_rows_fail_closed_without_new_hold_or_audit() {
        for (column, value) in [
            ("app_id", "cli_"),
            ("tenant_key", "bad tenant"),
            ("bot_open_id", "ou_human1"),
            ("human_open_id", "ou_"),
            ("chat_id", "oc_界"),
            ("enabled", "2"),
        ] {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let user = registry.add_user("alice").unwrap();
            let binding = feishu_binding(registry, user.user_id, 1);
            let before = registry
                .audit_list(user.user_id, 0, 100, true)
                .unwrap()
                .events
                .len();
            let conn = registry.connection().unwrap();
            conn.execute_batch(
                "PRAGMA ignore_check_constraints=ON; DROP TRIGGER feishu_binding_immutable;",
            )
            .unwrap();
            conn.execute(&format!("UPDATE feishu_bindings SET {column}=?1"), [value])
                .unwrap();
            assert!(registry
                .feishu_authorized(binding.id)
                .map(|b| b.is_none())
                .unwrap_or(true));
            assert!(registry
                .admit_feishu(
                    binding.id,
                    Uuid::now_v7(),
                    "feishu_execute",
                    &binding.id.to_string()
                )
                .is_err());
            assert!(registry.list().unwrap()[0].hold.is_none());
            assert_eq!(
                registry
                    .audit_list(user.user_id, 0, 100, true)
                    .unwrap()
                    .events
                    .len(),
                before
            );
        }
    }

    fn downgrade_before_feishu(conn: &Connection, version: i64) {
        conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings")
            .unwrap();
        if version < 5 {
            conn.execute_batch("DROP TABLE discord_bindings").unwrap();
        }
        if version < 4 {
            conn.execute_batch("DROP TABLE slack_bindings").unwrap();
        }
        if version < 3 {
            conn.execute_batch("DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only;").unwrap();
        }
        if version < 2 {
            conn.execute_batch("DROP TABLE telegram_bindings; DROP INDEX users_identity_backend;")
                .unwrap();
        }
        conn.pragma_update(None, "user_version", version).unwrap();
    }

    #[test]
    fn feishu_schema_six_preserves_every_prior_channel_permissions_audit_and_review_hold() {
        for version in 1..=5 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry
                .add_user_with_access("alice", version >= 3)
                .unwrap();
            let bob = registry.add_user("bob").unwrap();
            registry.revoke(bob.key_id).unwrap();
            registry.set_enabled(bob.user_id, false).unwrap();
            if version >= 2 {
                let b = registry
                    .add_telegram_binding(alice.user_id, "123", "456")
                    .unwrap();
                registry.revoke_telegram_binding(b.id).unwrap();
            }
            if version >= 4 {
                let b = slack_binding(registry, alice.user_id, 1);
                registry.revoke_slack_binding(b.id).unwrap();
            }
            if version >= 5 {
                let b = discord_binding(registry, alice.user_id, 1);
                registry.revoke_discord_binding(b.id).unwrap();
            }
            let request = Uuid::now_v7();
            registry
                .admit_scheduled(alice.user_id, "alice", request)
                .unwrap();
            registry
                .finish_write(alice.user_id, request, false)
                .unwrap();
            let users = serde_json::to_value(registry.list().unwrap()).unwrap();
            let keys =
                serde_json::to_value(registry.list_keys(alice.user_id, 100, 0).unwrap()).unwrap();
            let telegram = registry.list_telegram_bindings().unwrap();
            let slack = registry.list_slack_bindings().unwrap();
            let discord = registry.list_discord_bindings().unwrap();
            let audit =
                serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap();
            let conn = registry.connection().unwrap();
            downgrade_before_feishu(&conn, version);
            drop(conn);
            let migrated = Registry::open(&registry.path).unwrap();
            assert_eq!(
                serde_json::to_value(migrated.list().unwrap()).unwrap(),
                users
            );
            assert_eq!(
                serde_json::to_value(migrated.list_keys(alice.user_id, 100, 0).unwrap()).unwrap(),
                keys
            );
            assert_eq!(migrated.list_telegram_bindings().unwrap(), telegram);
            assert_eq!(migrated.list_slack_bindings().unwrap(), slack);
            assert_eq!(migrated.list_discord_bindings().unwrap(), discord);
            assert_eq!(
                serde_json::to_value(migrated.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap(),
                audit
            );
            assert_eq!(principal(&migrated, &alice).read_only, version >= 3);
            assert!(migrated.authenticate(&bob.token).unwrap().is_none());
            assert!(migrated.list_feishu_bindings().unwrap().is_empty());
            let binding = feishu_binding(&migrated, alice.user_id, 1);
            assert_eq!(
                migrated
                    .admit_feishu(
                        binding.id,
                        Uuid::now_v7(),
                        "feishu_execute",
                        &binding.id.to_string()
                    )
                    .unwrap_err()
                    .downcast_ref::<WriteAdmissionError>(),
                Some(&WriteAdmissionError::Held)
            );
            assert_eq!(
                migrated
                    .connection()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn feishu_late_migration_failure_rolls_back_every_prior_schema_step() {
        for version in 1..=5 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry.add_user("alice").unwrap();
            let conn = registry.connection().unwrap();
            downgrade_before_feishu(&conn, version);
            conn.execute_batch("CREATE TRIGGER feishu_binding_no_delete BEFORE DELETE ON users BEGIN SELECT RAISE(ABORT,'fixture conflict'); END;").unwrap();
            let schema:String=conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|row|row.get(0)).unwrap();
            assert!(Registry::open(&registry.path).is_err());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|row|row.get::<_,String>(0)).unwrap(),schema);
            conn.execute_batch("DROP TRIGGER feishu_binding_no_delete")
                .unwrap();
            drop(conn);
            let migrated = Registry::open(&registry.path).unwrap();
            assert_eq!(principal(&migrated, &alice).backend_id, "alice");
            assert!(migrated.list_feishu_bindings().unwrap().is_empty());
        }
    }

    const DISCORD_VERIFY_KEY: &str =
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    fn discord_binding(registry: &Registry, user: Uuid, number: usize) -> DiscordBindingSummary {
        registry
            .add_discord_binding(
                user,
                &number.to_string(),
                DISCORD_VERIFY_KEY,
                &format!("{number}0"),
                &format!("{number}1"),
                &format!("{number}2"),
                &format!("{number}3"),
            )
            .unwrap()
    }

    #[test]
    fn discord_ownership_revocation_and_public_pin_are_permanent_independent_of_keys() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user_with_access("alice", true).unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = discord_binding(registry, alice.user_id, 1);
        registry.rotate(alice.key_id).unwrap();
        assert_eq!(
            registry.discord_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        let conn = registry.connection().unwrap();
        for sql in [
            "UPDATE discord_bindings SET id='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'",
            "UPDATE discord_bindings SET user_id='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'",
            "UPDATE discord_bindings SET backend_id='bob'",
            "UPDATE discord_bindings SET application_id='20'",
            "UPDATE discord_bindings SET verify_key=replace(verify_key,'d','e')",
            "UPDATE discord_bindings SET bot_user_id='20'",
            "UPDATE discord_bindings SET sender_id='21'",
            "UPDATE discord_bindings SET conversation_id='22'",
            "UPDATE discord_bindings SET command_id='23'",
            "DELETE FROM discord_bindings",
            "INSERT OR REPLACE INTO discord_bindings SELECT * FROM discord_bindings LIMIT 1",
        ] {
            assert!(conn.execute_batch(sql).is_err(), "{sql}");
        }
        registry.revoke_discord_binding(binding.id).unwrap();
        registry.revoke_discord_binding(binding.id).unwrap();
        assert!(registry.discord_authorized(binding.id).unwrap().is_none());
        assert!(conn
            .execute(
                "UPDATE discord_bindings SET enabled=1 WHERE id=?1",
                [binding.id.to_string()]
            )
            .is_err());
        assert!(registry
            .add_discord_binding(
                alice.user_id,
                "2",
                DISCORD_VERIFY_KEY,
                "20",
                "21",
                "22",
                "23"
            )
            .is_err());
        assert!(registry
            .add_discord_binding(bob.user_id, "1", DISCORD_VERIFY_KEY, "20", "21", "22", "23")
            .is_err());
        let other = discord_binding(registry, bob.user_id, 2);
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.list_discord_bindings().unwrap().len(), 2);
        assert_eq!(reopened.discord_authorized(other.id).unwrap(), Some(other));
        let audit = reopened.audit_list(alice.user_id, 0, 100, true).unwrap();
        assert_eq!(
            audit
                .events
                .iter()
                .filter(|entry| entry.action == "discord_binding_revoked")
                .count(),
            1
        );
        assert!(!serde_json::to_string(&audit)
            .unwrap()
            .contains(DISCORD_VERIFY_KEY));
    }

    #[test]
    fn discord_snowflakes_cover_unsigned_range_and_invalid_identity_has_no_side_effect() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let before =
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap();
        for bad in [
            "",
            "0",
            "01",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1.0",
            "١",
            "18446744073709551616",
            "100000000000000000000",
            "1\0",
        ] {
            for field in 0..5 {
                let mut ids = ["1", "10", "11", "12", "13"];
                ids[field] = bad;
                assert!(
                    registry
                        .add_discord_binding(
                            user.user_id,
                            ids[0],
                            DISCORD_VERIFY_KEY,
                            ids[1],
                            ids[2],
                            ids[3],
                            ids[4]
                        )
                        .is_err(),
                    "{field} {bad:?}"
                );
            }
        }
        for bad in [
            "".into(),
            "0".repeat(64),
            "f".repeat(63),
            "f".repeat(65),
            DISCORD_VERIFY_KEY.to_uppercase(),
            format!("g{}", "a".repeat(63)),
        ] {
            assert!(registry
                .add_discord_binding(user.user_id, "1", &bad, "10", "11", "12", "13")
                .is_err());
        }
        assert!(registry
            .add_discord_binding(
                user.user_id,
                "1",
                DISCORD_VERIFY_KEY,
                "10",
                "10",
                "12",
                "13"
            )
            .is_err());
        assert!(registry
            .add_discord_binding(
                Uuid::new_v4(),
                "1",
                DISCORD_VERIFY_KEY,
                "10",
                "11",
                "12",
                "13"
            )
            .is_err());
        registry.set_enabled(user.user_id, false).unwrap();
        assert!(registry
            .add_discord_binding(
                user.user_id,
                "1",
                DISCORD_VERIFY_KEY,
                "10",
                "11",
                "12",
                "13"
            )
            .is_err());
        registry.set_enabled(user.user_id, true).unwrap();
        assert!(registry.list_discord_bindings().unwrap().is_empty());
        let audit = registry.audit_list(user.user_id, 0, 100, true).unwrap();
        assert_eq!(
            audit
                .events
                .iter()
                .filter(|entry| entry.action == "discord_binding_added")
                .count(),
            0
        );
        assert_eq!(
            audit.events.len(),
            before["events"].as_array().unwrap().len() + 2
        );
        let max = registry
            .add_discord_binding(
                user.user_id,
                "18446744073709551615",
                DISCORD_VERIFY_KEY,
                "18446744073709551614",
                "18446744073709551613",
                "18446744073709551612",
                "18446744073709551611",
            )
            .unwrap();
        assert_eq!(registry.discord_authorized(max.id).unwrap(), Some(max));
    }

    #[test]
    fn discord_admission_validates_metadata_before_hold_and_shares_all_user_effects() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user_with_access("alice", true).unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = discord_binding(registry, alice.user_id, 1);
        let slack = slack_binding(registry, alice.user_id, 1);
        let tg = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        let before =
            serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                .unwrap();
        for (request, operation, object) in [
            (Uuid::new_v4(), "discord_execute", binding.id.to_string()),
            (Uuid::nil(), "discord_send", binding.id.to_string()),
            (Uuid::now_v7(), "slack_send", binding.id.to_string()),
            (Uuid::now_v7(), "discord_execute", "prompt secret".into()),
            (Uuid::now_v7(), "discord_send", Uuid::nil().to_string()),
            (
                Uuid::now_v7(),
                "discord_send",
                "ABCDEF01-2345-4678-9ABC-DEF012345678".into(),
            ),
            (
                Uuid::now_v7(),
                "discord_send",
                "abcdef01-2345-4678-1abc-def012345678".into(),
            ),
        ] {
            assert!(registry
                .admit_discord(binding.id, request, operation, &object)
                .is_err());
        }
        let mut non_rfc = *Uuid::now_v7().as_bytes();
        non_rfc[8] = 0;
        assert!(registry
            .admit_discord(
                binding.id,
                Uuid::from_bytes(non_rfc),
                "discord_send",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry
            .list()
            .unwrap()
            .iter()
            .all(|user| user.hold.is_none()));
        assert_eq!(
            serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                .unwrap(),
            before
        );
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry.discord_authorized(binding.id).unwrap().is_none());
        assert_eq!(
            registry
                .admit_discord(
                    binding.id,
                    Uuid::now_v7(),
                    "discord_execute",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        registry.set_enabled(alice.user_id, true).unwrap();
        // A read-only API key does not limit this separately administrator-owned channel.
        let request = Uuid::now_v7();
        registry
            .admit_discord(
                binding.id,
                request,
                "discord_execute",
                &binding.id.to_string(),
            )
            .unwrap();
        assert!(registry
            .admit_write(
                &principal(registry, &registry.add_key(alice.user_id).unwrap()),
                Uuid::now_v7()
            )
            .is_err());
        assert!(registry
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .is_err());
        assert!(registry
            .admit_telegram(tg.id, Uuid::now_v7(), "telegram_send", &tg.id.to_string())
            .is_err());
        assert!(registry
            .admit_slack(
                slack.id,
                Uuid::now_v7(),
                "slack_send",
                &slack.id.to_string()
            )
            .is_err());
        assert_eq!(
            registry
                .admit_discord(
                    binding.id,
                    Uuid::now_v7(),
                    "discord_send",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        let other = Uuid::now_v7();
        registry.admit_scheduled(bob.user_id, "bob", other).unwrap();
        registry.finish_write(bob.user_id, other, true).unwrap();
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.recover_writes().unwrap(), 1);
        reopened.finish_write(alice.user_id, request, true).unwrap();
        reopened.revoke_discord_binding(binding.id).unwrap();
        let owner = reopened
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.user_id == alice.user_id)
            .unwrap();
        let hold = owner.hold.unwrap();
        assert_eq!(
            (hold.request_id, hold.state.as_str()),
            (request, "needs_review")
        );
        reopened
            .clear_review(
                alice.user_id,
                "Reviewed all stopped queues and reconciled external effects",
            )
            .unwrap();
        assert_eq!(
            reopened
                .admit_discord(
                    binding.id,
                    Uuid::now_v7(),
                    "discord_send",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        let audit = reopened.audit_list(alice.user_id, 0, 100, true).unwrap();
        let admitted = audit
            .events
            .iter()
            .find(|entry| entry.action == "discord_execute")
            .unwrap();
        assert!(admitted.key_id.is_none());
        assert_eq!(
            admitted.request_id.as_deref(),
            Some(request.to_string().as_str())
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(admitted.note.as_ref().unwrap()).unwrap(),
            serde_json::json!({"binding_id":binding.id.to_string(),"object_id":binding.id.to_string()})
        );
    }

    #[test]
    fn discord_audit_storage_failure_rolls_back_binding_revocation_and_admission() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        let fail = "CREATE TRIGGER fail_discord_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected audit storage failure'); END";
        conn.execute_batch(fail).unwrap();
        assert!(registry
            .add_discord_binding(
                user.user_id,
                "1",
                DISCORD_VERIFY_KEY,
                "10",
                "11",
                "12",
                "13"
            )
            .is_err());
        assert!(registry.list_discord_bindings().unwrap().is_empty());
        conn.execute_batch("DROP TRIGGER fail_discord_audit")
            .unwrap();
        let binding = discord_binding(registry, user.user_id, 1);
        let before =
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap();
        conn.execute_batch(fail).unwrap();
        assert!(registry.revoke_discord_binding(binding.id).is_err());
        assert_eq!(
            registry.discord_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        assert!(registry
            .admit_discord(
                binding.id,
                Uuid::now_v7(),
                "discord_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert_eq!(
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn discord_lifetime_limit_counts_revocations_and_sql_enforces_unsigned_canonical_ids() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        for number in 1..=32 {
            let user = registry.add_user(&format!("backend-{number}")).unwrap();
            let binding = discord_binding(registry, user.user_id, number);
            registry.revoke_discord_binding(binding.id).unwrap();
        }
        assert_eq!(registry.list_discord_bindings().unwrap().len(), 32);
        let user = registry.list().unwrap()[0].user_id;
        assert!(registry
            .add_discord_binding(user, "999", DISCORD_VERIFY_KEY, "990", "991", "992", "993")
            .is_err());
        let conn = registry.connection().unwrap();
        assert!(conn.execute("INSERT INTO discord_bindings(id,user_id,backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'backend-1','999',?3,'990','991','992','993',1,0,0)", params![Uuid::new_v4().to_string(),user.to_string(),DISCORD_VERIFY_KEY]).is_err());
        let fixture = Fixture::new();
        let user = fixture.registry.add_user("alice").unwrap();
        let conn = fixture.registry.connection().unwrap();
        for bad in ["0", "01", "18446744073709551616", "1\0", "١"] {
            assert!(conn.execute("INSERT INTO discord_bindings(id,user_id,backend_id,application_id,verify_key,bot_user_id,sender_id,conversation_id,command_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'alice',?3,?4,'10','11','12','13',1,0,0)", params![Uuid::new_v4().to_string(),user.user_id.to_string(),bad,DISCORD_VERIFY_KEY]).is_err());
        }
        assert!(fixture.registry.list_discord_bindings().unwrap().is_empty());
    }

    #[test]
    fn malformed_discord_owner_rows_fail_closed_without_hold_or_audit_mutation() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let binding = discord_binding(registry, user.user_id, 1);
        let before =
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap();
        let conn = registry.connection().unwrap();
        conn.execute_batch(
            "PRAGMA ignore_check_constraints=ON; DROP TRIGGER discord_binding_immutable;",
        )
        .unwrap();
        conn.execute(
            "UPDATE discord_bindings SET sender_id='18446744073709551616' WHERE id=?1",
            [binding.id.to_string()],
        )
        .unwrap();
        assert!(registry.list_discord_bindings().is_err());
        assert!(registry.discord_authorized(binding.id).is_err());
        assert!(registry
            .admit_discord(
                binding.id,
                Uuid::now_v7(),
                "discord_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert_eq!(
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap(),
            before
        );
        conn.execute(
            "UPDATE discord_bindings SET sender_id='11',verify_key=?1",
            ["0".repeat(64)],
        )
        .unwrap();
        assert!(registry.discord_authorized(binding.id).is_err());
        conn.execute(
            "UPDATE discord_bindings SET verify_key=?1,enabled=2",
            [DISCORD_VERIFY_KEY],
        )
        .unwrap();
        assert!(registry.list_discord_bindings().is_err());
        assert!(registry.discord_authorized(binding.id).unwrap().is_none());
    }

    #[test]
    fn discord_schema_five_preserves_prior_authority_permissions_audit_and_uncertain_holds() {
        for version in 1..=4 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry
                .add_user_with_access("alice", version >= 3)
                .unwrap();
            let bob = registry.add_user("bob").unwrap();
            registry.revoke(bob.key_id).unwrap();
            registry.set_enabled(bob.user_id, false).unwrap();
            if version >= 2 {
                let binding = registry
                    .add_telegram_binding(alice.user_id, "101", "201")
                    .unwrap();
                registry.revoke_telegram_binding(binding.id).unwrap();
            }
            if version == 4 {
                let binding = slack_binding(registry, alice.user_id, 1);
                registry.revoke_slack_binding(binding.id).unwrap();
            }
            registry
                .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
                .unwrap();
            let users = serde_json::to_value(registry.list().unwrap()).unwrap();
            let keys =
                serde_json::to_value(registry.list_keys(alice.user_id, 100, 0).unwrap()).unwrap();
            let telegram = registry.list_telegram_bindings().unwrap();
            let slack = registry.list_slack_bindings().unwrap();
            let audit =
                serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap();
            let conn = registry.connection().unwrap();
            conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings")
                .unwrap();
            if version < 4 {
                conn.execute_batch("DROP TABLE slack_bindings").unwrap();
            }
            if version < 3 {
                conn.execute_batch("DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only;").unwrap();
            }
            if version == 1 {
                conn.execute_batch(
                    "DROP TABLE telegram_bindings; DROP INDEX users_identity_backend;",
                )
                .unwrap();
            }
            conn.pragma_update(None, "user_version", version).unwrap();
            drop(conn);
            assert!(registry.list().is_err());
            let migrated = Registry::open(&registry.path).unwrap();
            assert_eq!(
                serde_json::to_value(migrated.list().unwrap()).unwrap(),
                users
            );
            assert_eq!(
                serde_json::to_value(migrated.list_keys(alice.user_id, 100, 0).unwrap()).unwrap(),
                keys
            );
            assert_eq!(migrated.list_telegram_bindings().unwrap(), telegram);
            assert_eq!(migrated.list_slack_bindings().unwrap(), slack);
            assert_eq!(
                serde_json::to_value(migrated.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap(),
                audit
            );
            assert!(migrated.list_discord_bindings().unwrap().is_empty());
            assert_eq!(principal(&migrated, &alice).read_only, version >= 3);
            assert!(migrated.authenticate(&bob.token).unwrap().is_none());
            let binding = discord_binding(&migrated, alice.user_id, 1);
            assert_eq!(
                migrated
                    .admit_discord(
                        binding.id,
                        Uuid::now_v7(),
                        "discord_execute",
                        &binding.id.to_string()
                    )
                    .unwrap_err()
                    .downcast_ref::<WriteAdmissionError>(),
                Some(&WriteAdmissionError::Held)
            );
            assert_eq!(
                migrated
                    .connection()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn discord_schema_five_late_ddl_failure_rolls_back_all_prior_migrations() {
        for version in 1..=4 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let user = registry.add_user("alice").unwrap();
            let conn = registry.connection().unwrap();
            conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings")
                .unwrap();
            if version < 4 {
                conn.execute_batch("DROP TABLE slack_bindings").unwrap();
            }
            if version < 3 {
                conn.execute_batch("DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only;").unwrap();
            }
            if version == 1 {
                conn.execute_batch(
                    "DROP TABLE telegram_bindings; DROP INDEX users_identity_backend;",
                )
                .unwrap();
            }
            conn.pragma_update(None, "user_version", version).unwrap();
            conn.execute_batch("CREATE TRIGGER discord_binding_no_delete BEFORE DELETE ON users BEGIN SELECT RAISE(ABORT,'migration conflict'); END;").unwrap();
            let schema: String = conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |row| row.get(0)).unwrap();
            assert!(Registry::open(&registry.path).is_err());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |row| row.get::<_,String>(0)).unwrap(), schema);
            conn.execute_batch("DROP TRIGGER discord_binding_no_delete")
                .unwrap();
            drop(conn);
            let migrated = Registry::open(&registry.path).unwrap();
            assert!(!principal(&migrated, &user).read_only);
            assert!(migrated.list_discord_bindings().unwrap().is_empty());
        }
    }

    fn slack_binding(registry: &Registry, user: Uuid, number: usize) -> SlackBindingSummary {
        registry
            .add_slack_binding(
                user,
                &format!("T{number}"),
                &format!("A{number}"),
                &format!("U{number}0"),
                &format!("B{number}"),
                &format!("U{number}1"),
                &format!("D{number}"),
            )
            .unwrap()
    }

    #[test]
    fn slack_bindings_accept_canonical_u_and_w_user_prefixes_and_full_bounded_ids() {
        for (bot, sender) in [
            ("U10", "U11"),
            ("W10", "W11"),
            ("U10", "W11"),
            ("W10", "U11"),
        ] {
            let fixture = Fixture::new();
            let user = fixture.registry.add_user("alice").unwrap();
            let binding = fixture
                .registry
                .add_slack_binding(user.user_id, "T1", "A1", bot, "B1", sender, "D1")
                .unwrap();
            assert_eq!(
                fixture.registry.slack_authorized(binding.id).unwrap(),
                Some(binding)
            );
            let other = fixture.registry.add_user("bob").unwrap();
            let long = |prefix| format!("{prefix}{}", "0".repeat(63));
            let max = fixture
                .registry
                .add_slack_binding(
                    other.user_id,
                    &long('T'),
                    &long('A'),
                    &long('W'),
                    &long('B'),
                    &long('U'),
                    &long('D'),
                )
                .unwrap();
            assert_eq!(
                fixture.registry.slack_authorized(max.id).unwrap(),
                Some(max)
            );
        }
    }

    #[test]
    fn slack_ownership_and_revocation_survive_key_changes_and_cannot_be_replaced() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user_with_access("alice", true).unwrap();
        let bob = registry.add_user("bob").unwrap();
        let tg = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        let binding = slack_binding(registry, alice.user_id, 1);
        assert_eq!(
            registry.slack_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        assert!(registry
            .add_slack_binding(alice.user_id, "T2", "A2", "U20", "B2", "U21", "D2")
            .is_err());
        assert!(registry
            .add_slack_binding(bob.user_id, "T1", "A1", "U30", "B3", "U31", "D3")
            .is_err());
        // The app's single callback URL cannot serve another dedicated user,
        // including installations in a different workspace.
        assert!(registry
            .add_slack_binding(bob.user_id, "T2", "A1", "U20", "B2", "U21", "D2")
            .is_err());
        let other = slack_binding(registry, bob.user_id, 2);
        let rotated = registry.rotate(alice.key_id).unwrap();
        registry.revoke(rotated.key_id).unwrap();
        assert_eq!(
            registry.slack_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        let conn = registry.connection().unwrap();
        for sql in [
            "UPDATE slack_bindings SET id='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'",
            "UPDATE slack_bindings SET user_id='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'",
            "UPDATE slack_bindings SET backend_id='changed'",
            "UPDATE slack_bindings SET team_id='T99'",
            "UPDATE slack_bindings SET app_id='A99'",
            "UPDATE slack_bindings SET bot_user_id='U99'",
            "UPDATE slack_bindings SET bot_id='B99'",
            "UPDATE slack_bindings SET sender_id='U99'",
            "UPDATE slack_bindings SET conversation_id='D99'",
            "DELETE FROM slack_bindings",
            "UPDATE users SET backend_id='changed' WHERE backend_id='alice'",
            "INSERT OR REPLACE INTO slack_bindings SELECT * FROM slack_bindings LIMIT 1",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
        registry.revoke_slack_binding(binding.id).unwrap();
        registry.revoke_slack_binding(binding.id).unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        registry.set_enabled(alice.user_id, true).unwrap();
        registry.add_key(alice.user_id).unwrap();
        assert!(registry.slack_authorized(binding.id).unwrap().is_none());
        assert!(conn
            .execute(
                "UPDATE slack_bindings SET enabled=1 WHERE id=?1",
                [binding.id.to_string()]
            )
            .is_err());
        assert!(registry
            .add_slack_binding(alice.user_id, "T3", "A2", "U30", "B3", "U31", "D3")
            .is_err());
        let charlie = registry.add_user("charlie").unwrap();
        assert!(registry
            .add_slack_binding(charlie.user_id, "T3", "A1", "U30", "B3", "U31", "D3")
            .is_err());
        let history = registry.list_slack_bindings().unwrap();
        assert_eq!(history.len(), 2);
        assert!(
            !history
                .iter()
                .find(|item| item.id == binding.id)
                .unwrap()
                .enabled
        );
        assert_eq!(registry.slack_authorized(other.id).unwrap(), Some(other));
        assert_eq!(registry.telegram_authorized(tg.id).unwrap(), Some(tg));
        let audit = registry.audit_list(alice.user_id, 0, 100, true).unwrap();
        let revoked: Vec<_> = audit
            .events
            .iter()
            .filter(|event| event.action == "slack_binding_revoked")
            .collect();
        assert_eq!(revoked.len(), 1);
        assert_eq!(
            revoked[0].note.as_deref(),
            Some(binding.id.to_string().as_str())
        );
        let encoded = serde_json::to_string(&history).unwrap();
        assert!(!encoded.contains(&alice.token) && !encoded.contains("verifier"));
        drop(conn);
        assert_eq!(
            Registry::open(&registry.path)
                .unwrap()
                .list_slack_bindings()
                .unwrap(),
            history
        );
    }

    #[test]
    fn slack_ids_and_admission_metadata_fail_before_any_hold_or_audit() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let ids = ["T1", "A1", "U10", "B1", "U11", "D1"];
        for index in 0..ids.len() {
            for bad in [
                "",
                "T",
                "wrong-secret-value",
                "t123",
                "Té",
                "T1\0",
                "T1\n",
                "T 1",
                "T-1",
            ] {
                let mut candidate = ids;
                candidate[index] = bad;
                assert!(registry
                    .add_slack_binding(
                        alice.user_id,
                        candidate[0],
                        candidate[1],
                        candidate[2],
                        candidate[3],
                        candidate[4],
                        candidate[5]
                    )
                    .is_err());
            }
            let over = format!("{}{}", ids[index].chars().next().unwrap(), "1".repeat(64));
            let mut candidate = ids;
            candidate[index] = &over;
            assert!(registry
                .add_slack_binding(
                    alice.user_id,
                    candidate[0],
                    candidate[1],
                    candidate[2],
                    candidate[3],
                    candidate[4],
                    candidate[5]
                )
                .is_err());
        }
        assert!(registry
            .add_slack_binding(alice.user_id, "T1", "A1", "U1", "B1", "U1", "D1")
            .is_err());
        assert!(registry
            .add_slack_binding(Uuid::new_v4(), "T1", "A1", "U10", "B1", "U11", "D1")
            .is_err());
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry
            .add_slack_binding(alice.user_id, "T1", "A1", "U10", "B1", "U11", "D1")
            .is_err());
        registry.set_enabled(alice.user_id, true).unwrap();
        assert!(registry.list_slack_bindings().unwrap().is_empty());
        let binding = slack_binding(registry, alice.user_id, 1);
        let before =
            serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                .unwrap();
        for (request, operation, object) in [
            (Uuid::new_v4(), "slack_execute", binding.id.to_string()),
            (Uuid::nil(), "slack_send", binding.id.to_string()),
            (Uuid::now_v7(), "telegram_send", binding.id.to_string()),
            (Uuid::now_v7(), "secret-operation", binding.id.to_string()),
            (Uuid::now_v7(), "slack_send", "secret-object".into()),
            (Uuid::now_v7(), "slack_send", Uuid::nil().to_string()),
            (
                Uuid::now_v7(),
                "slack_send",
                "ABCDEF01-2345-4678-9ABC-DEF012345678".into(),
            ),
            (
                Uuid::now_v7(),
                "slack_send",
                "abcdef01-2345-4678-1abc-def012345678".into(),
            ),
        ] {
            assert!(registry
                .admit_slack(binding.id, request, operation, &object)
                .is_err());
        }
        let mut non_rfc = *Uuid::now_v7().as_bytes();
        non_rfc[8] = 0;
        assert!(registry
            .admit_slack(
                binding.id,
                Uuid::from_bytes(non_rfc),
                "slack_send",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert_eq!(
            serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                .unwrap(),
            before
        );
    }

    #[test]
    fn slack_shares_admission_with_http_cron_and_telegram_and_preserves_review_after_restart() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = slack_binding(registry, alice.user_id, 1);
        let tg = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry.slack_authorized(binding.id).unwrap().is_none());
        assert_eq!(
            registry
                .admit_slack(
                    binding.id,
                    Uuid::now_v7(),
                    "slack_execute",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        registry.set_enabled(alice.user_id, true).unwrap();
        let request = Uuid::now_v7();
        registry
            .admit_slack(
                binding.id,
                request,
                "slack_execute",
                &binding.id.to_string(),
            )
            .unwrap();
        assert!(registry.slack_authorized(binding.id).unwrap().is_some());
        assert_eq!(
            registry
                .admit_slack(
                    binding.id,
                    Uuid::now_v7(),
                    "slack_send",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        assert!(registry
            .admit_write(&principal(registry, &alice), Uuid::new_v4())
            .is_err());
        assert!(registry
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .is_err());
        assert!(registry
            .admit_telegram(tg.id, Uuid::now_v7(), "telegram_send", &tg.id.to_string())
            .is_err());
        let other = Uuid::now_v7();
        registry.admit_scheduled(bob.user_id, "bob", other).unwrap();
        registry.finish_write(bob.user_id, other, true).unwrap();
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.recover_writes().unwrap(), 1);
        reopened.finish_write(alice.user_id, request, true).unwrap();
        let hold = reopened
            .list()
            .unwrap()
            .into_iter()
            .find(|item| item.user_id == alice.user_id)
            .unwrap()
            .hold
            .unwrap();
        assert_eq!(hold.request_id, request);
        assert_eq!(hold.state, "needs_review");
        reopened.revoke_slack_binding(binding.id).unwrap();
        reopened.set_enabled(alice.user_id, false).unwrap();
        reopened.set_enabled(alice.user_id, true).unwrap();
        assert_eq!(
            reopened
                .list()
                .unwrap()
                .into_iter()
                .find(|item| item.user_id == alice.user_id)
                .unwrap()
                .hold
                .unwrap()
                .request_id,
            request
        );
        reopened
            .clear_review(
                alice.user_id,
                "Stopped both channel queues and reconciled external effect",
            )
            .unwrap();
        assert_eq!(
            reopened
                .admit_slack(
                    binding.id,
                    Uuid::now_v7(),
                    "slack_send",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        let audit = reopened.audit_list(alice.user_id, 0, 100, true).unwrap();
        let admitted = audit
            .events
            .iter()
            .find(|event| event.action == "slack_execute")
            .unwrap();
        assert!(admitted.key_id.is_none());
        assert_eq!(
            admitted.request_id.as_deref(),
            Some(request.to_string().as_str())
        );
        let note: serde_json::Value =
            serde_json::from_str(admitted.note.as_ref().unwrap()).unwrap();
        assert_eq!(
            note,
            serde_json::json!({"binding_id":binding.id.to_string(),"object_id":binding.id.to_string()})
        );
    }

    #[test]
    fn discord_and_foreground_concurrent_admission_have_one_winner() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let binding = discord_binding(registry, user.user_id, 1);
        let owner = principal(registry, &user);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                let owner = owner.clone();
                let binding = binding.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    if index == 0 {
                        registry.admit_write(&owner, Uuid::now_v7())
                    } else {
                        registry
                            .admit_discord(
                                binding.id,
                                Uuid::now_v7(),
                                "discord_execute",
                                &binding.id.to_string(),
                            )
                            .map(|_| ())
                    }
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().err().is_some_and(|error| error
                    .downcast_ref::<WriteAdmissionError>(
                ) == Some(
                    &WriteAdmissionError::Held
                )))
                .count(),
            1
        );
        assert!(registry.list().unwrap()[0].hold.is_some());
    }

    #[test]
    fn slack_and_foreground_concurrent_admission_have_one_winner() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let binding = slack_binding(registry, user.user_id, 1);
        let owner = principal(registry, &user);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                let owner = owner.clone();
                let binding = binding.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    if index == 0 {
                        registry.admit_write(&owner, Uuid::now_v7())
                    } else {
                        registry
                            .admit_slack(
                                binding.id,
                                Uuid::now_v7(),
                                "slack_execute",
                                &binding.id.to_string(),
                            )
                            .map(|_| ())
                    }
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().err().is_some_and(|error| error
                    .downcast_ref::<WriteAdmissionError>(
                ) == Some(
                    &WriteAdmissionError::Held
                )))
                .count(),
            1
        );
        assert!(registry.list().unwrap()[0].hold.is_some());
    }

    #[test]
    fn slack_audit_failure_rolls_back_binding_revocation_and_admission() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        let fail = "CREATE TRIGGER fail_slack_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected audit storage failure'); END";
        conn.execute_batch(fail).unwrap();
        assert!(registry
            .add_slack_binding(user.user_id, "T1", "A1", "U10", "B1", "U11", "D1")
            .is_err());
        assert!(registry.list_slack_bindings().unwrap().is_empty());
        conn.execute_batch("DROP TRIGGER fail_slack_audit").unwrap();
        let binding = slack_binding(registry, user.user_id, 1);
        let before =
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap();
        conn.execute_batch(fail).unwrap();
        assert!(registry.revoke_slack_binding(binding.id).is_err());
        assert_eq!(
            registry.slack_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        assert!(registry
            .admit_slack(
                binding.id,
                Uuid::now_v7(),
                "slack_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert_eq!(
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn slack_lifetime_limit_counts_revoked_bindings_and_database_rejects_invalid_ids() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        for number in 0..32 {
            let user = registry.add_user(&format!("backend-{number}")).unwrap();
            let binding = slack_binding(registry, user.user_id, number);
            registry.revoke_slack_binding(binding.id).unwrap();
        }
        assert_eq!(registry.list_slack_bindings().unwrap().len(), 32);
        let user = registry.list().unwrap()[0].user_id;
        assert!(registry
            .add_slack_binding(user, "T999", "A1", "U990", "B99", "U991", "D99")
            .unwrap_err()
            .to_string()
            .contains("lifetime"));
        let conn = registry.connection().unwrap();
        assert!(conn.execute("INSERT INTO slack_bindings(id,user_id,backend_id,team_id,app_id,bot_user_id,bot_id,sender_id,conversation_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'backend-0','T999','A1','U10','B1','U11','D1',1,0,0)", params![Uuid::new_v4().to_string(), user.to_string()]).is_err());
        drop(conn);
        let fixture = Fixture::new();
        let user = fixture.registry.add_user("alice").unwrap();
        let conn = fixture.registry.connection().unwrap();
        for bad in ["t1", "Té", "T1\0", "T1\n", "T-1", "T"] {
            assert!(conn.execute("INSERT INTO slack_bindings(id,user_id,backend_id,team_id,app_id,bot_user_id,bot_id,sender_id,conversation_id,enabled,created_ms,updated_ms) VALUES(?1,?2,'alice',?3,'A1','U10','B1','U11','D1',1,0,0)", params![Uuid::new_v4().to_string(),user.user_id.to_string(),bad]).is_err());
        }
        assert!(fixture.registry.list_slack_bindings().unwrap().is_empty());
    }

    #[test]
    fn malformed_slack_owner_rows_cannot_authorize_an_effect_or_create_a_hold() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let user = registry.add_user("alice").unwrap();
        let binding = slack_binding(registry, user.user_id, 1);
        let before =
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap();
        let conn = registry.connection().unwrap();
        conn.execute_batch(
            "PRAGMA ignore_check_constraints=ON; DROP TRIGGER slack_binding_immutable;",
        )
        .unwrap();
        conn.execute(
            "UPDATE slack_bindings SET sender_id='w-secret-value' WHERE id=?1",
            [binding.id.to_string()],
        )
        .unwrap();
        assert!(registry.list_slack_bindings().is_err());
        assert!(registry.slack_authorized(binding.id).is_err());
        assert!(registry
            .admit_slack(
                binding.id,
                Uuid::now_v7(),
                "slack_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert_eq!(
            serde_json::to_value(registry.audit_list(user.user_id, 0, 100, true).unwrap()).unwrap(),
            before
        );
        conn.execute("UPDATE slack_bindings SET sender_id='U11',enabled=2", [])
            .unwrap();
        assert!(registry.list_slack_bindings().is_err());
        assert!(registry.slack_authorized(binding.id).unwrap().is_none());
        conn.execute(
            "UPDATE slack_bindings SET enabled=1,id='bbbbbbbb-bbbb-4bbb-1bbb-bbbbbbbbbbbb'",
            [],
        )
        .unwrap();
        assert!(registry.list_slack_bindings().is_err());
    }

    #[test]
    fn slack_schema_four_migrates_all_supported_versions_without_changing_prior_authority_or_holds()
    {
        for version in 1..=3 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry
                .add_user_with_access("alice", version == 3)
                .unwrap();
            let bob = registry.add_user("bob").unwrap();
            registry.revoke(bob.key_id).unwrap();
            registry.set_enabled(bob.user_id, false).unwrap();
            if version >= 2 {
                let tg = registry
                    .add_telegram_binding(alice.user_id, "101", "201")
                    .unwrap();
                registry.revoke_telegram_binding(tg.id).unwrap();
            }
            let request = Uuid::now_v7();
            registry
                .admit_scheduled(alice.user_id, "alice", request)
                .unwrap();
            let users = serde_json::to_value(registry.list().unwrap()).unwrap();
            let keys =
                serde_json::to_value(registry.list_keys(alice.user_id, 100, 0).unwrap()).unwrap();
            let tg = registry.list_telegram_bindings().unwrap();
            let audit =
                serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap();
            let conn = registry.connection().unwrap();
            conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings; DROP TABLE slack_bindings")
                .unwrap();
            if version < 3 {
                conn.execute_batch("DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only;").unwrap();
            }
            if version == 1 {
                conn.execute_batch(
                    "DROP TABLE telegram_bindings; DROP INDEX users_identity_backend;",
                )
                .unwrap();
            }
            conn.pragma_update(None, "user_version", version).unwrap();
            drop(conn);
            assert!(registry.list().is_err());
            let migrated = Registry::open(&registry.path).unwrap();
            assert_eq!(
                serde_json::to_value(migrated.list().unwrap()).unwrap(),
                users
            );
            assert_eq!(
                serde_json::to_value(migrated.list_keys(alice.user_id, 100, 0).unwrap()).unwrap(),
                keys
            );
            assert_eq!(migrated.list_telegram_bindings().unwrap(), tg);
            assert_eq!(
                serde_json::to_value(migrated.audit_list(alice.user_id, 0, 100, true).unwrap())
                    .unwrap(),
                audit
            );
            assert!(migrated.list_slack_bindings().unwrap().is_empty());
            assert_eq!(principal(&migrated, &alice).read_only, version == 3);
            assert!(migrated.authenticate(&bob.token).unwrap().is_none());
            let binding = slack_binding(&migrated, alice.user_id, 1);
            assert_eq!(
                migrated
                    .admit_slack(
                        binding.id,
                        Uuid::now_v7(),
                        "slack_execute",
                        &binding.id.to_string()
                    )
                    .unwrap_err()
                    .downcast_ref::<WriteAdmissionError>(),
                Some(&WriteAdmissionError::Held)
            );
            assert_eq!(
                migrated
                    .connection()
                    .unwrap()
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn slack_schema_four_failure_rolls_back_every_prior_migration_step() {
        for version in 1..=3 {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let user = registry.add_user("alice").unwrap();
            let conn = registry.connection().unwrap();
            conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings; DROP TABLE slack_bindings")
                .unwrap();
            if version < 3 {
                conn.execute_batch("DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only;").unwrap();
            }
            if version == 1 {
                conn.execute_batch(
                    "DROP TABLE telegram_bindings; DROP INDEX users_identity_backend;",
                )
                .unwrap();
            }
            conn.pragma_update(None, "user_version", version).unwrap();
            // Conflict late in v4, after its table and earlier triggers were created.
            conn.execute_batch("CREATE TRIGGER slack_binding_no_delete BEFORE DELETE ON users BEGIN SELECT RAISE(ABORT,'migration conflict'); END;").unwrap();
            let schema: String = conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |row| row.get(0)).unwrap();
            assert!(Registry::open(&registry.path).is_err());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(conn.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |row| row.get::<_,String>(0)).unwrap(), schema);
            conn.execute_batch("DROP TRIGGER slack_binding_no_delete")
                .unwrap();
            drop(conn);
            let migrated = Registry::open(&registry.path).unwrap();
            assert!(!principal(&migrated, &user).read_only);
            assert!(migrated.list_slack_bindings().unwrap().is_empty());
            assert!(migrated.list_telegram_bindings().unwrap().is_empty());
        }
    }

    #[test]
    fn audit_query_tracks_owned_key_lifecycle_and_unknown_write_without_changing_authority() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let readonly = registry.add_key_with_access(alice.user_id, true).unwrap();
        let replacement = registry.rotate(alice.key_id).unwrap();
        registry.revoke(readonly.key_id).unwrap();
        let request_id = Uuid::new_v4();
        registry
            .admit_write(&principal(registry, &replacement), request_id)
            .unwrap();
        registry
            .finish_write(alice.user_id, request_id, false)
            .unwrap();
        let before = serde_json::to_value(registry.list().unwrap()).unwrap();
        let keys =
            serde_json::to_value(registry.list_keys(alice.user_id, 100, 0).unwrap()).unwrap();
        let page = registry.audit_list(alice.user_id, 0, 100, false).unwrap();
        assert_eq!(
            page.events
                .iter()
                .map(|event| event.action.as_str())
                .collect::<Vec<_>>(),
            [
                "user_added",
                "key_added",
                "key_revoked_by_rotation",
                "key_rotated",
                "key_revoked",
                "write_admitted",
                "write_needs_review",
            ]
        );
        assert!(page
            .events
            .iter()
            .all(|event| event.user_id == alice.user_id));
        assert!(page.events.iter().all(|event| event.note.is_none()));
        assert!(!page.notes_included);
        assert!(!page.has_more);
        assert!(!page.retention_gap);
        assert_eq!(page.oldest_retained_seq.as_deref(), Some("1"));
        assert_eq!(page.next_after_seq, page.latest_seq);
        let admission = page
            .events
            .iter()
            .find(|event| event.action == "write_admitted")
            .unwrap();
        assert_eq!(
            admission.request_id.as_deref(),
            Some(request_id.to_string().as_str())
        );
        assert_eq!(
            admission.key_id.as_deref(),
            Some(replacement.key_id.to_string().as_str())
        );
        let bob_page = registry.audit_list(bob.user_id, 0, 100, true).unwrap();
        assert_eq!(bob_page.events.len(), 1);
        assert_eq!(bob_page.events[0].user_id, bob.user_id);
        assert_eq!(
            serde_json::to_value(registry.list().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            serde_json::to_value(registry.list_keys(alice.user_id, 100, 0).unwrap()).unwrap(),
            keys
        );
        assert!(registry.authenticate(&alice.token).unwrap().is_none());
        assert!(registry.authenticate(&replacement.token).unwrap().is_some());
        let output = serde_json::to_string(&page).unwrap();
        assert!(!output.contains("verifier"));
        assert!(!output.contains(&alice.token));
        assert!(!output.contains(&readonly.token));
        assert!(!output.contains(&replacement.token));
        assert!(!output.contains(&bob.user_id.to_string()));
    }

    #[test]
    fn audit_cursor_pagination_advances_only_delivered_rows_until_user_history_is_exhausted() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        registry.add_key(alice.user_id).unwrap();
        registry.add_key(bob.user_id).unwrap();
        registry.add_key(alice.user_id).unwrap();
        registry.add_key(bob.user_id).unwrap();
        let first = registry.audit_list(alice.user_id, 0, 1, false).unwrap();
        assert_eq!(first.events[0].seq, "1");
        assert_eq!(first.next_after_seq, "1");
        assert!(first.has_more);
        assert_eq!(first.latest_seq, "6");
        let middle = registry.audit_list(alice.user_id, 1, 1, false).unwrap();
        assert_eq!(middle.events[0].seq, "3");
        assert_eq!(middle.next_after_seq, "3");
        assert!(middle.has_more);
        let last = registry.audit_list(alice.user_id, 3, 1, false).unwrap();
        assert_eq!(last.events[0].seq, "5");
        assert!(!last.has_more);
        assert_eq!(last.next_after_seq, "6");
        let empty = registry.audit_list(alice.user_id, 5, 100, false).unwrap();
        assert!(empty.events.is_empty());
        assert!(!empty.has_more);
        assert_eq!(empty.next_after_seq, "6");
        let current = registry.audit_list(alice.user_id, 6, 100, false).unwrap();
        assert!(current.events.is_empty());
        assert_eq!(current.next_after_seq, "6");
        for (after, limit) in [(0, 0), (0, 101), (7, 1), (u64::MAX, 1)] {
            assert!(registry
                .audit_list(alice.user_id, after, limit, false)
                .is_err());
        }
        assert!(registry.audit_list(Uuid::new_v4(), 0, 1, false).is_err());
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry.audit_list(alice.user_id, 6, 1, false).is_ok());
    }

    #[test]
    fn audit_sequences_serialize_exactly_above_javascript_integer_precision() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        conn.execute(
            "UPDATE sqlite_sequence SET seq=?1 WHERE name='audit_events'",
            [i64::MAX - 1],
        )
        .unwrap();
        registry.add_key(alice.user_id).unwrap();
        let page = registry.audit_list(alice.user_id, 1, 100, false).unwrap();
        let maximum = i64::MAX.to_string();
        assert_eq!(page.events[0].seq, maximum);
        assert_eq!(page.latest_seq, maximum);
        assert_eq!(page.next_after_seq, maximum);
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["latest_seq"].as_str(), Some(maximum.as_str()));
        assert_eq!(json["next_after_seq"].as_str(), Some(maximum.as_str()));
        assert_eq!(json["events"][0]["seq"].as_str(), Some(maximum.as_str()));
        assert_eq!(json["oldest_retained_seq"].as_str(), Some("1"));
        let end = registry
            .audit_list(alice.user_id, i64::MAX as u64, 1, false)
            .unwrap();
        assert!(end.events.is_empty());
        assert!(!end.retention_gap);
    }

    #[test]
    fn audit_retention_gap_uses_global_history_and_survives_empty_retained_history() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let conn = registry.connection().unwrap();
        conn.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO audit_events(user_id,action,created_ms) SELECT ?2,'seed',0 FROM n", params![MAX_AUDIT_EVENTS,bob.user_id.to_string()]).unwrap();
        registry.add_key(bob.user_id).unwrap();
        let page = registry.audit_list(alice.user_id, 0, 100, false).unwrap();
        assert!(page.events.is_empty());
        assert_eq!(page.oldest_retained_seq.as_deref(), Some("4"));
        assert_eq!(page.latest_seq, (MAX_AUDIT_EVENTS + 3).to_string());
        assert_eq!(page.next_after_seq, page.latest_seq);
        assert!(page.retention_gap);
        assert!(!page.has_more);
        let adjacent = registry.audit_list(alice.user_id, 3, 100, false).unwrap();
        assert!(!adjacent.retention_gap);
        conn.execute("DELETE FROM audit_events", []).unwrap();
        let empty = registry.audit_list(alice.user_id, 0, 100, false).unwrap();
        assert!(empty.events.is_empty());
        assert!(empty.oldest_retained_seq.is_none());
        assert_eq!(empty.latest_seq, page.latest_seq);
        assert_eq!(empty.next_after_seq, page.latest_seq);
        assert!(empty.retention_gap);
        assert!(
            !registry
                .audit_list(alice.user_id, empty.latest_seq.parse().unwrap(), 100, false)
                .unwrap()
                .retention_gap
        );
    }

    #[test]
    fn audit_page_metadata_and_rows_share_a_snapshot_while_another_connection_prunes() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let mut conn = registry.connection().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .unwrap();
        // Anchor the read snapshot before a committed writer advances sequence
        // state and prunes every row that was previously visible to this reader.
        assert_eq!(
            tx.query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let writer = registry.clone();
        let user = bob.user_id;
        std::thread::spawn(move || {
            let mut conn = writer.connection().unwrap();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
            tx.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO audit_events(user_id,action,created_ms) SELECT ?2,'seed',0 FROM n", params![MAX_AUDIT_EVENTS,user.to_string()]).unwrap();
            audit(&tx, user, None, None, "seed", None, 0).unwrap();
            tx.commit().unwrap();
        }).join().unwrap();
        let old = audit_page(&tx, alice.user_id, 0, 100, false).unwrap();
        assert_eq!(old.latest_seq, "2");
        assert_eq!(old.oldest_retained_seq.as_deref(), Some("1"));
        assert_eq!(old.events.len(), 1);
        assert_eq!(old.events[0].seq, "1");
        assert_eq!(old.next_after_seq, "2");
        assert!(!old.retention_gap);
        tx.commit().unwrap();
        let new = registry.audit_list(alice.user_id, 0, 100, false).unwrap();
        assert_eq!(new.latest_seq, (MAX_AUDIT_EVENTS + 3).to_string());
        assert_eq!(new.oldest_retained_seq.as_deref(), Some("4"));
        assert!(new.events.is_empty());
        assert!(new.retention_gap);
    }

    #[test]
    fn audit_notes_are_omitted_by_default_and_explicit_output_is_bounded_escaped_json() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let note = "private evidence: \"quoted\"\n第二行 😀";
        let mut conn = registry.connection().unwrap();
        let tx = conn.transaction().unwrap();
        audit(
            &tx,
            alice.user_id,
            None,
            None,
            "operator_note",
            Some(note),
            0,
        )
        .unwrap();
        tx.commit().unwrap();
        let hidden =
            serde_json::to_value(registry.audit_list(alice.user_id, 0, 100, false).unwrap())
                .unwrap();
        assert_eq!(hidden["notes_included"], false);
        assert!(hidden["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event.get("note").is_none()));
        assert!(!hidden.to_string().contains("private evidence"));
        let visible = registry.audit_list(alice.user_id, 0, 100, true).unwrap();
        assert!(visible.notes_included);
        assert_eq!(visible.events[1].note.as_deref(), Some(note));
        let json = serde_json::to_string(&visible).unwrap();
        assert!(json.contains("\\n"));
        assert!(json.contains("\\\"quoted\\\""));
        assert!(!json.contains('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["events"][1]["note"],
            note
        );
        let maximum = "\0".repeat(512);
        let tx = conn.transaction().unwrap();
        for _ in 0..100 {
            audit(
                &tx,
                alice.user_id,
                None,
                None,
                "maximum_note",
                Some(&maximum),
                0,
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let page = registry.audit_list(alice.user_id, 2, 100, true).unwrap();
        assert_eq!(page.events.len(), 100);
        assert!(!page.has_more);
        assert!(serde_json::to_vec(&page).unwrap().len() <= 512 * 1024);
        // Even an oversized corrupt note is neither fetched nor emitted by a
        // default metadata query; explicit inclusion fails without truncation.
        conn.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        conn.execute(
            "UPDATE audit_events SET note=?1 WHERE seq=1",
            ["oversized".repeat(256 * 1024)],
        )
        .unwrap();
        assert!(registry.audit_list(alice.user_id, 0, 1, false).is_ok());
        assert!(registry.audit_list(alice.user_id, 0, 1, true).is_err());
    }

    #[test]
    fn audit_query_rejects_malformed_metadata_and_inconsistent_sequence_state() {
        for (column, value) in [
            ("key_id", "not-a-uuid".to_string()),
            (
                "request_id",
                "ABCDEFAB-1234-4234-9234-123456789ABC".to_string(),
            ),
            ("action", "a".repeat(129)),
            ("action", "line\nfeed".to_string()),
        ] {
            let fixture = Fixture::new();
            let registry = &fixture.registry;
            let alice = registry.add_user("alice").unwrap();
            let conn = registry.connection().unwrap();
            conn.execute(&format!("UPDATE audit_events SET {column}=?1"), [value])
                .unwrap();
            assert!(registry.audit_list(alice.user_id, 0, 1, false).is_err());
        }
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        conn.execute("UPDATE audit_events SET created_ms=-1", [])
            .unwrap();
        assert!(registry.audit_list(alice.user_id, 0, 1, false).is_err());
        conn.execute("UPDATE audit_events SET created_ms=0", [])
            .unwrap();
        conn.execute(
            "UPDATE sqlite_sequence SET seq=0 WHERE name='audit_events'",
            [],
        )
        .unwrap();
        assert!(registry.audit_list(alice.user_id, 0, 1, false).is_err());
        conn.execute("DELETE FROM audit_events", []).unwrap();
        conn.execute(
            "UPDATE sqlite_sequence SET seq=NULL WHERE name='audit_events'",
            [],
        )
        .unwrap();
        assert!(registry.audit_list(alice.user_id, 0, 1, false).is_err());
    }

    #[test]
    fn read_only_access_is_per_key_and_denied_admission_has_no_persisted_effects() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let full = registry.add_user("alice").unwrap();
        let restricted = registry.add_key_with_access(full.user_id, true).unwrap();
        assert!(!full.read_only);
        assert!(!principal(registry, &full).read_only);
        assert!(restricted.read_only);
        let mut readonly = principal(registry, &restricted);
        assert!(readonly.read_only);
        // The persisted key permission is authority even if a caller forges its
        // authenticated snapshot. Denial must precede all hold/audit mutations.
        readonly.read_only = false;
        let before = serde_json::to_value(registry.list().unwrap()).unwrap();
        let conn = registry.connection().unwrap();
        let audit_before: i64 = conn
            .query_row("SELECT count(*) FROM audit_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            registry
                .admit_write(&readonly, Uuid::new_v4())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::ReadOnly)
        );
        assert_eq!(
            serde_json::to_value(registry.list().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM audit_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            audit_before
        );
        let full_principal = principal(registry, &full);
        let request = Uuid::new_v4();
        registry.admit_write(&full_principal, request).unwrap();
        // An existing full-key write hold never converts read-only denial into
        // a hold error or changes the write that was already admitted.
        assert_eq!(
            registry
                .admit_write(&readonly, Uuid::new_v4())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::ReadOnly)
        );
        assert_eq!(
            registry.list().unwrap()[0]
                .hold
                .as_ref()
                .unwrap()
                .request_id,
            request
        );
        registry.finish_write(full.user_id, request, true).unwrap();
        registry.revoke(restricted.key_id).unwrap();
        assert_eq!(
            registry
                .admit_write(&readonly, Uuid::new_v4())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
    }

    #[test]
    fn read_only_rotation_revocation_and_disable_preserve_access_and_other_keys() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let first = registry.add_user_with_access("alice", true).unwrap();
        let full = registry.add_key(first.user_id).unwrap();
        let other = Registry::open(&registry.path).unwrap();
        let rotated = registry.rotate(first.key_id).unwrap();
        assert!(rotated.read_only);
        assert!(principal(&other, &rotated).read_only);
        assert!(other.authenticate(&first.token).unwrap().is_none());
        assert!(!principal(&other, &full).read_only);
        registry.set_enabled(first.user_id, false).unwrap();
        assert!(other.authenticate(&rotated.token).unwrap().is_none());
        assert!(other.authenticate(&full.token).unwrap().is_none());
        assert!(registry.rotate(rotated.key_id).is_err());
        assert!(registry.add_key_with_access(first.user_id, true).is_err());
        registry.set_enabled(first.user_id, true).unwrap();
        assert!(principal(&other, &rotated).read_only);
        registry.revoke(rotated.key_id).unwrap();
        registry.revoke(rotated.key_id).unwrap();
        assert!(other.authenticate(&rotated.token).unwrap().is_none());
        assert!(!principal(&other, &full).read_only);
        assert!(registry.rotate(rotated.key_id).is_err());
        let conn = registry.connection().unwrap();
        let notes: Vec<(String, String)> = conn.prepare(
            "SELECT action,note FROM audit_events WHERE action IN ('user_added','key_added','key_rotated','key_revoked_by_rotation') ORDER BY seq"
        ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(
            notes,
            vec![
                ("user_added".into(), access_note(true).into()),
                ("key_added".into(), access_note(false).into()),
                ("key_revoked_by_rotation".into(), access_note(true).into()),
                ("key_rotated".into(), access_note(true).into()),
            ]
        );
    }

    #[test]
    fn access_is_immutable_and_key_metadata_is_bounded_and_secret_free() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let restricted = registry.add_key_with_access(alice.user_id, true).unwrap();
        let bob = registry.add_user_with_access("bob", true).unwrap();
        registry.revoke(restricted.key_id).unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        let conn = registry.connection().unwrap();
        assert!(conn
            .execute(
                "UPDATE api_keys SET read_only=1 WHERE id=?1",
                [alice.key_id.to_string()]
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE api_keys SET read_only=0 WHERE id=?1",
                [restricted.key_id.to_string()]
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE api_keys SET read_only=2 WHERE id=?1",
                [alice.key_id.to_string()]
            )
            .is_err());
        let all = registry.list_keys(alice.user_id, 100, 0).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|key| key.user_id == alice.user_id));
        assert!(all
            .iter()
            .any(|key| key.key_id == alice.key_id && !key.read_only && key.revoked_ms.is_none()));
        assert!(all.iter().any(|key| key.key_id == restricted.key_id
            && key.read_only
            && key.revoked_ms.is_some()));
        let first_page = registry.list_keys(alice.user_id, 1, 0).unwrap();
        let second_page = registry.list_keys(alice.user_id, 1, 1).unwrap();
        assert_eq!(first_page[0].key_id, all[0].key_id);
        assert_eq!(second_page[0].key_id, all[1].key_id);
        assert!(registry.list_keys(alice.user_id, 1, 2).unwrap().is_empty());
        assert!(registry
            .list_keys(alice.user_id, 1, 1024)
            .unwrap()
            .is_empty());
        for (limit, offset) in [(0, 0), (101, 0), (1, 1025), (usize::MAX, 0)] {
            assert!(registry.list_keys(alice.user_id, limit, offset).is_err());
        }
        assert!(registry.list_keys(Uuid::new_v4(), 20, 0).is_err());
        let serialized = serde_json::to_value(all).unwrap();
        for key in serialized.as_array().unwrap() {
            assert_eq!(key.as_object().unwrap().len(), 5);
            assert!(key.get("verifier").is_none());
            assert!(key.get("token").is_none());
        }
        let output = serialized.to_string();
        assert!(!output.contains(&alice.token));
        assert!(!output.contains(&restricted.token));
        assert!(!output.contains(&bob.user_id.to_string()));
    }

    #[test]
    fn permission_lifecycle_audit_failure_rolls_back_user_key_and_rotation_atomically() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let readonly = registry.add_user_with_access("alice", true).unwrap();
        let conn = registry.connection().unwrap();
        // Fail only the final rotation audit, after its old-key revocation,
        // insertion and first audit have succeeded inside the transaction.
        conn.execute_batch("CREATE TRIGGER fail_rotation_audit BEFORE INSERT ON audit_events WHEN NEW.action='key_rotated' BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
        let before =
            serde_json::to_value(registry.list_keys(readonly.user_id, 100, 0).unwrap()).unwrap();
        let audit_before: i64 = conn
            .query_row("SELECT count(*) FROM audit_events", [], |row| row.get(0))
            .unwrap();
        assert!(registry.rotate(readonly.key_id).is_err());
        assert!(principal(registry, &readonly).read_only);
        assert_eq!(
            serde_json::to_value(registry.list_keys(readonly.user_id, 100, 0).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM audit_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            audit_before
        );
        conn.execute_batch("DROP TRIGGER fail_rotation_audit; CREATE TRIGGER fail_all_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
        assert!(registry.add_user_with_access("bob", true).is_err());
        assert!(registry
            .add_key_with_access(readonly.user_id, true)
            .is_err());
        assert_eq!(registry.list().unwrap().len(), 1);
        assert_eq!(
            serde_json::to_value(registry.list_keys(readonly.user_id, 100, 0).unwrap()).unwrap(),
            before
        );
        conn.execute_batch("DROP TRIGGER fail_all_audit").unwrap();
        assert!(registry.rotate(readonly.key_id).unwrap().read_only);
    }

    #[test]
    fn read_only_credentials_do_not_revoke_user_owned_background_authority() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let readonly = registry.add_user_with_access("alice", true).unwrap();
        let binding = registry
            .add_telegram_binding(readonly.user_id, "101", "201")
            .unwrap();
        let replacement = registry.rotate(readonly.key_id).unwrap();
        registry.revoke(replacement.key_id).unwrap();
        assert!(registry.authenticate(&replacement.token).unwrap().is_none());
        assert_eq!(
            registry.scheduled_users().unwrap(),
            vec![ScheduledUser {
                user_id: readonly.user_id,
                backend_id: "alice".into()
            }]
        );
        let cron = Uuid::now_v7();
        registry
            .admit_scheduled(readonly.user_id, "alice", cron)
            .unwrap();
        registry.finish_write(readonly.user_id, cron, true).unwrap();
        let telegram = Uuid::now_v7();
        registry
            .admit_telegram(
                binding.id,
                telegram,
                "telegram_execute",
                &binding.id.to_string(),
            )
            .unwrap();
        registry
            .finish_write(readonly.user_id, telegram, true)
            .unwrap();
        registry.set_enabled(readonly.user_id, false).unwrap();
        assert!(registry.scheduled_users().unwrap().is_empty());
        assert_eq!(
            registry
                .admit_scheduled(readonly.user_id, "alice", Uuid::now_v7())
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        assert_eq!(
            registry
                .admit_telegram(
                    binding.id,
                    Uuid::now_v7(),
                    "telegram_execute",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
    }

    #[test]
    fn schema_two_permission_migration_preserves_bindings_and_legacy_full_access() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let active = registry.add_user("alice").unwrap();
        let revoked = registry.add_key(active.user_id).unwrap();
        registry.revoke(revoked.key_id).unwrap();
        let disabled = registry.add_user("bob").unwrap();
        registry.set_enabled(disabled.user_id, false).unwrap();
        let binding = registry
            .add_telegram_binding(active.user_id, "101", "201")
            .unwrap();
        let request = Uuid::now_v7();
        registry
            .admit_scheduled(active.user_id, "alice", request)
            .unwrap();
        let users = serde_json::to_value(registry.list().unwrap()).unwrap();
        let bindings = registry.list_telegram_bindings().unwrap();
        let conn = registry.connection().unwrap();
        let audits: String = conn.query_row("SELECT group_concat(action || coalesce(note,''), ';') FROM audit_events ORDER BY seq", [], |row| row.get(0)).unwrap();
        conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings; DROP TABLE slack_bindings; DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only; PRAGMA user_version=2;").unwrap();
        drop(conn);
        assert!(registry.authenticate(&active.token).is_err());
        let migrated = Registry::open(&registry.path).unwrap();
        assert!(!principal(&migrated, &active).read_only);
        assert!(migrated.authenticate(&revoked.token).unwrap().is_none());
        assert!(migrated.authenticate(&disabled.token).unwrap().is_none());
        assert_eq!(
            serde_json::to_value(migrated.list().unwrap()).unwrap(),
            users
        );
        assert_eq!(migrated.list_telegram_bindings().unwrap(), bindings);
        assert_eq!(migrated.list_keys(active.user_id, 100, 0).unwrap().len(), 2);
        assert!(migrated
            .list_keys(active.user_id, 100, 0)
            .unwrap()
            .iter()
            .all(|key| !key.read_only));
        assert!(migrated
            .list_keys(disabled.user_id, 100, 0)
            .unwrap()
            .iter()
            .all(|key| !key.read_only));
        let conn = migrated.connection().unwrap();
        assert_eq!(conn.query_row("SELECT group_concat(action || coalesce(note,''), ';') FROM audit_events ORDER BY seq", [], |row| row.get::<_, String>(0)).unwrap(), audits);
        assert!(conn
            .execute(
                "UPDATE api_keys SET read_only=1 WHERE id=?1",
                [active.key_id.to_string()]
            )
            .is_err());
        drop(conn);
        let reopened = Registry::open(&registry.path).unwrap();
        assert!(!reopened.rotate(active.key_id).unwrap().read_only);
        assert!(
            reopened
                .add_key_with_access(active.user_id, true)
                .unwrap()
                .read_only
        );
        assert_eq!(reopened.list_telegram_bindings().unwrap()[0].id, binding.id);
    }

    #[test]
    fn schema_one_to_three_migration_is_atomic_when_later_schema_step_fails() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let active = registry.add_user("alice").unwrap();
        let conn = registry.connection().unwrap();
        conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings; DROP TABLE slack_bindings; DROP TABLE telegram_bindings; DROP INDEX users_identity_backend; DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only; PRAGMA user_version=1; CREATE TRIGGER api_key_access_immutable BEFORE UPDATE ON api_keys BEGIN SELECT RAISE(ABORT,'migration conflict'); END;").unwrap();
        // The v2 schema and v3 column are created before the v3 trigger conflicts.
        // Both must be rolled back together with user_version.
        assert!(Registry::open(&registry.path).is_err());
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='telegram_bindings')",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap());
        assert!(!conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('api_keys') WHERE name='read_only')",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap());
        conn.execute_batch("DROP TRIGGER api_key_access_immutable")
            .unwrap();
        drop(conn);
        let migrated = Registry::open(&registry.path).unwrap();
        assert!(!principal(&migrated, &active).read_only);
        assert!(migrated.list_telegram_bindings().unwrap().is_empty());
        assert!(!migrated.rotate(active.key_id).unwrap().read_only);
    }

    #[test]
    fn live_key_rotation_revocation_and_user_disable_are_isolated() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let other = Registry::open(&registry.path).unwrap();
        assert_eq!(principal(&other, &alice).backend_id, "alice");
        assert_eq!(principal(&other, &bob).backend_id, "bob");
        let rotated = registry.rotate(alice.key_id).unwrap();
        assert_eq!(rotated.user_id, alice.user_id);
        assert!(other.authenticate(&alice.token).unwrap().is_none());
        assert_eq!(principal(&other, &rotated).user_id, alice.user_id);
        registry.revoke(rotated.key_id).unwrap();
        registry.revoke(rotated.key_id).unwrap();
        assert!(other.authenticate(&rotated.token).unwrap().is_none());
        registry.set_enabled(bob.user_id, false).unwrap();
        assert!(other.authenticate(&bob.token).unwrap().is_none());
        assert!(registry.add_key(bob.user_id).is_err());
        assert!(registry.rotate(bob.key_id).is_err());
        registry.set_enabled(bob.user_id, true).unwrap();
        assert!(other.authenticate(&bob.token).unwrap().is_some());
        let replacement = registry.add_key(alice.user_id).unwrap();
        assert!(other.authenticate(&replacement.token).unwrap().is_some());
        assert!(other.authenticate("not-a-key").unwrap().is_none());
        let (unknown, _) = keys::issue(Uuid::new_v4(), false).unwrap();
        assert!(other.authenticate(&unknown.token).unwrap().is_none());
        let stolen_id = alice
            .token
            .replace(&alice.key_id.to_string(), &bob.key_id.to_string());
        assert!(other.authenticate(&stolen_id).unwrap().is_none());
    }

    #[test]
    fn summaries_and_database_never_store_issued_secrets() {
        let fixture = Fixture::new();
        let issued = fixture.registry.add_user("private").unwrap();
        let list = serde_json::to_string(&fixture.registry.list().unwrap()).unwrap();
        assert!(!list.contains(&issued.token));
        assert!(!list.contains("verifier"));
        assert!(list.contains(&issued.user_id.to_string()));
        let conn = fixture.registry.connection().unwrap();
        let verifier: Vec<u8> = conn
            .query_row("SELECT verifier FROM api_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(verifier.len(), 32);
        assert_ne!(
            verifier.as_slice(),
            keys::parse(&issued.token).unwrap().verifier(Uuid::new_v4())
        );
        let schema: String = conn
            .query_row(
                "SELECT group_concat(sql) FROM sqlite_schema WHERE sql IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!schema.contains(" token "));
        assert!(!fs::read(&fixture.registry.path)
            .unwrap()
            .windows(issued.token.len())
            .any(|window| window == issued.token.as_bytes()));
    }

    #[test]
    fn admission_revalidates_credentials_and_only_owned_completion_changes_holds() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let a = principal(registry, &alice);
        let b = principal(registry, &bob);
        registry.revoke(alice.key_id).unwrap();
        let error = registry.admit_write(&a, Uuid::new_v4()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        let alice = registry.add_key(alice.user_id).unwrap();
        let a = principal(registry, &alice);
        let request = Uuid::new_v4();
        registry.admit_write(&a, request).unwrap();
        assert!(registry
            .admit_write(&a, Uuid::new_v4())
            .unwrap_err()
            .is::<WriteAdmissionError>());
        let bob_request = Uuid::new_v4();
        registry.admit_write(&b, bob_request).unwrap();
        registry
            .finish_write(a.user_id, Uuid::new_v4(), true)
            .unwrap();
        registry.finish_write(b.user_id, request, true).unwrap();
        assert!(registry
            .list()
            .unwrap()
            .iter()
            .all(|user| user.hold.is_some()));
        assert!(registry.clear_review(a.user_id, "reviewed").is_err());
        registry.finish_write(a.user_id, request, true).unwrap();
        registry.finish_write(a.user_id, request, false).unwrap();
        assert!(registry
            .list()
            .unwrap()
            .iter()
            .find(|user| user.user_id == a.user_id)
            .unwrap()
            .hold
            .is_none());
        registry
            .finish_write(b.user_id, bob_request, false)
            .unwrap();
        let users = registry.list().unwrap();
        let hold = users
            .iter()
            .find(|user| user.user_id == b.user_id)
            .unwrap()
            .hold
            .as_ref()
            .unwrap();
        assert_eq!(hold.state, "needs_review");
        assert_eq!(hold.reason, "backend_outcome_unknown");
        registry.finish_write(b.user_id, bob_request, true).unwrap();
        assert!(registry.admit_write(&b, Uuid::new_v4()).is_err());
        registry.set_enabled(b.user_id, false).unwrap();
        registry
            .clear_review(b.user_id, "Backend stopped; mutation reconciled")
            .unwrap();
        let conn = registry.connection().unwrap();
        let note: String = conn
            .query_row(
                "SELECT note FROM audit_events WHERE action='write_review_cleared'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(note, "Backend stopped; mutation reconciled");
        assert!(registry.admit_write(&b, Uuid::new_v4()).is_err());
    }

    #[test]
    fn startup_recovery_requires_manual_review_and_never_accepts_a_late_success() {
        let fixture = Fixture::new();
        let issued = fixture.registry.add_user("alice").unwrap();
        let owner = principal(&fixture.registry, &issued);
        let request = Uuid::new_v4();
        fixture.registry.admit_write(&owner, request).unwrap();
        let restarted = Registry::open(&fixture.registry.path).unwrap();
        assert_eq!(restarted.recover_writes().unwrap(), 1);
        assert_eq!(restarted.recover_writes().unwrap(), 0);
        fixture
            .registry
            .finish_write(owner.user_id, request, true)
            .unwrap();
        let users = restarted.list().unwrap();
        assert_eq!(users[0].hold.as_ref().unwrap().reason, "gateway_restarted");
        assert!(restarted.admit_write(&owner, Uuid::new_v4()).is_err());
        for invalid in ["", "  ", "newline\nnote", &"x".repeat(513)] {
            assert!(restarted.clear_review(owner.user_id, invalid).is_err());
        }
        restarted
            .clear_review(owner.user_id, &"x".repeat(512))
            .unwrap();
        let newer = Uuid::new_v4();
        restarted.admit_write(&owner, newer).unwrap();
        fixture
            .registry
            .finish_write(owner.user_id, request, true)
            .unwrap();
        assert_eq!(
            restarted.list().unwrap()[0]
                .hold
                .as_ref()
                .unwrap()
                .request_id,
            newer
        );
    }

    #[test]
    fn concurrent_write_admission_has_one_winner() {
        let fixture = Fixture::new();
        let issued = fixture.registry.add_user("alice").unwrap();
        let owner = principal(&fixture.registry, &issued);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let registry = fixture.registry.clone();
                let owner = owner.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.admit_write(&owner, Uuid::new_v4())
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| error.downcast_ref::<WriteAdmissionError>()
                        == Some(&WriteAdmissionError::Held)))
                .count(),
            1
        );
    }

    #[test]
    fn active_key_user_and_retained_history_limits_are_transactional() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let first = registry.add_user("first").unwrap();
        for _ in 1..MAX_ACTIVE_KEYS {
            registry.add_key(first.user_id).unwrap();
        }
        assert!(registry.add_key(first.user_id).is_err());
        let rotated = registry.rotate(first.key_id).unwrap();
        assert_eq!(registry.list().unwrap()[0].active_keys, 8);
        assert!(registry.authenticate(&first.token).unwrap().is_none());
        assert!(registry.authenticate(&rotated.token).unwrap().is_some());
        for index in 1..MAX_USERS {
            registry.add_user(&format!("backend-{index}")).unwrap();
        }
        assert!(registry.add_user("overflow").is_err());
        assert!(registry.add_user("first").is_err());
        for invalid in ["", "a.b", "a/b", "credential value"] {
            assert!(registry.add_user(invalid).is_err());
        }
        // Seed only revoked history; pruning must never remove a live credential or hold.
        let mut conn = registry.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let count: i64 = tx
            .query_row("SELECT count(*) FROM api_keys", [], |row| row.get(0))
            .unwrap();
        for _ in count..MAX_KEYS {
            tx.execute("INSERT INTO api_keys(id,user_id,verifier,created_ms,revoked_ms) VALUES(?1,?2,?3,1,2)", params![Uuid::new_v4().to_string(), first.user_id.to_string(), &[0_u8;32][..]]).unwrap();
        }
        tx.commit().unwrap();
        drop(conn);
        let rotated_again = registry.rotate(rotated.key_id).unwrap();
        assert!(registry
            .authenticate(&rotated_again.token)
            .unwrap()
            .is_some());
        let conn = registry.connection().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM api_keys", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            MAX_KEYS
        );
        assert_eq!(registry.list().unwrap()[0].active_keys, 8);
    }

    #[test]
    fn failed_audit_writes_roll_back_admission_completion_recovery_and_review() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let issued = registry.add_user("alice").unwrap();
        let owner = principal(registry, &issued);
        let request = Uuid::new_v4();
        let conn = registry.connection().unwrap();
        let reject = "CREATE TRIGGER fail_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected audit storage failure'); END";
        conn.execute_batch(reject).unwrap();
        assert!(registry
            .admit_scheduled(owner.user_id, &owner.backend_id, Uuid::now_v7())
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert!(registry.admit_write(&owner, request).is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        conn.execute_batch("DROP TRIGGER fail_audit").unwrap();
        registry.admit_write(&owner, request).unwrap();
        conn.execute_batch(reject).unwrap();
        assert!(registry.finish_write(owner.user_id, request, true).is_err());
        assert!(registry
            .finish_write(owner.user_id, request, false)
            .is_err());
        assert!(registry.recover_writes().is_err());
        assert_eq!(
            registry.list().unwrap()[0].hold.as_ref().unwrap().state,
            "in_flight"
        );
        conn.execute_batch("DROP TRIGGER fail_audit").unwrap();
        assert_eq!(registry.recover_writes().unwrap(), 1);
        conn.execute_batch(reject).unwrap();
        assert!(registry
            .clear_review(owner.user_id, "Checked backend")
            .is_err());
        assert_eq!(
            registry.list().unwrap()[0].hold.as_ref().unwrap().state,
            "needs_review"
        );
        conn.execute_batch("DROP TRIGGER fail_audit").unwrap();
        registry
            .clear_review(owner.user_id, "Checked backend")
            .unwrap();
        assert!(registry.list().unwrap()[0].hold.is_none());
    }

    #[test]
    fn foreign_databases_future_versions_and_unsafe_paths_fail_closed() {
        let fixture = Fixture::new();
        assert!(Registry::open(Path::new("relative.sqlite3")).is_err());
        let conn = fixture.registry.connection().unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(Registry::open(&fixture.registry.path).is_err());
        assert!(fixture.registry.list().is_err());
        drop(conn);
        let unrelated = fixture.root.join("other.sqlite3");
        let conn = Connection::open(&unrelated).unwrap();
        conn.execute_batch("CREATE TABLE unrelated(value TEXT)")
            .unwrap();
        drop(conn);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(Registry::open(&unrelated).is_err());
        let unrelated_conn = Connection::open(&unrelated).unwrap();
        let journal: String = unrelated_conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "delete");
        drop(unrelated_conn);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let alias = fixture.root.join("alias.sqlite3");
            symlink(&fixture.registry.path, &alias).unwrap();
            assert!(Registry::open(&alias).is_err());
            fs::remove_file(&alias).unwrap();
            fs::hard_link(&fixture.registry.path, &alias).unwrap();
            assert!(Registry::open(&alias).is_err());
            fs::remove_file(&alias).unwrap();
            fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(Registry::open(&fixture.root.join("new.sqlite3")).is_err());
            assert_eq!(
                fs::metadata(&fixture.root).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }
    #[test]
    fn scheduled_authority_is_user_owned_and_rechecks_disable_binding_and_hold() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        assert_eq!(registry.scheduled_users().unwrap().len(), 2);
        let stale = registry.scheduled_users().unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry
            .scheduled_users()
            .unwrap()
            .iter()
            .all(|user| user.user_id != alice.user_id));
        assert!(registry
            .admit_scheduled(stale[0].user_id, &stale[0].backend_id, Uuid::now_v7())
            .unwrap_err()
            .is::<WriteAdmissionError>());
        registry.set_enabled(alice.user_id, true).unwrap();
        let rotated = registry.rotate(alice.key_id).unwrap();
        registry.revoke(rotated.key_id).unwrap();
        // Even no active login keys does not cancel an enabled user's job.
        assert!(registry
            .scheduled_users()
            .unwrap()
            .iter()
            .any(|user| user.user_id == alice.user_id));
        assert!(registry
            .admit_scheduled(alice.user_id, "bob", Uuid::now_v7())
            .is_err());
        assert!(registry
            .admit_scheduled(alice.user_id, "alice", Uuid::new_v4())
            .is_err());
        let request = Uuid::now_v7();
        registry
            .admit_scheduled(alice.user_id, "alice", request)
            .unwrap();
        assert!(registry
            .scheduled_users()
            .unwrap()
            .iter()
            .all(|user| user.user_id != alice.user_id));
        let key = registry.add_key(alice.user_id).unwrap();
        assert!(registry
            .admit_write(&principal(registry, &key), Uuid::new_v4())
            .unwrap_err()
            .is::<WriteAdmissionError>());
        assert!(registry
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .is_err());
        assert!(registry
            .scheduled_users()
            .unwrap()
            .iter()
            .any(|user| user.user_id == bob.user_id));
        let conn = registry.connection().unwrap();
        let (key_id,audited):(Option<String>,String)=conn.query_row("SELECT key_id,request_id FROM audit_events WHERE action='scheduled_write_admitted'",[],|row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert!(key_id.is_none());
        assert_eq!(audited, request.to_string());
        drop(conn);
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.recover_writes().unwrap(), 1);
        registry.finish_write(alice.user_id, request, true).unwrap();
        assert!(reopened
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .is_err());
        reopened.set_enabled(alice.user_id, false).unwrap();
        reopened.set_enabled(alice.user_id, true).unwrap();
        assert!(reopened
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .is_err());
        reopened
            .clear_review(
                alice.user_id,
                "Backend stopped and exact scheduled run reconciled",
            )
            .unwrap();
        reopened
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .unwrap();
    }

    #[test]
    fn foreground_and_scheduled_admission_have_only_one_winner() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let issued = registry.add_user("alice").unwrap();
        let owner = principal(registry, &issued);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let registry = registry.clone();
                let owner = owner.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    if index == 0 {
                        registry.admit_write(&owner, Uuid::new_v4())
                    } else {
                        registry.admit_scheduled(owner.user_id, &owner.backend_id, Uuid::now_v7())
                    }
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().err().is_some_and(|error| error
                    .downcast_ref::<WriteAdmissionError>(
                ) == Some(
                    &WriteAdmissionError::Held
                )))
                .count(),
            1
        );
        assert!(registry.scheduled_users().unwrap().is_empty());
    }

    #[test]
    fn telegram_schema_one_migration_preserves_users_keys_audit_and_unresolved_writes() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        registry.revoke(bob.key_id).unwrap();
        registry.set_enabled(bob.user_id, false).unwrap();
        let request = Uuid::now_v7();
        registry
            .admit_scheduled(alice.user_id, "alice", request)
            .unwrap();
        let before = serde_json::to_value(registry.list().unwrap()).unwrap();
        let conn = registry.connection().unwrap();
        let audit_before: i64 = conn
            .query_row("SELECT count(*) FROM audit_events", [], |row| row.get(0))
            .unwrap();
        // No bindings exist. Removing the v2/v3/v4 objects reconstructs the exact
        // v1 schema with genuine keys, revoked state, audit and pending work.
        conn.execute_batch("DROP TABLE wecom_bindings; DROP TABLE feishu_bindings; DROP TABLE discord_bindings; DROP TABLE slack_bindings; DROP TABLE telegram_bindings; DROP INDEX users_identity_backend; DROP TRIGGER api_key_access_immutable; ALTER TABLE api_keys DROP COLUMN read_only; PRAGMA user_version=1;").unwrap();
        drop(conn);
        assert!(
            registry.list().is_err(),
            "normal operations must not silently use v1"
        );
        let migrated = Registry::open(&registry.path).unwrap();
        assert_eq!(
            serde_json::to_value(migrated.list().unwrap()).unwrap(),
            before
        );
        assert_eq!(principal(&migrated, &alice).backend_id, "alice");
        assert!(migrated.authenticate(&bob.token).unwrap().is_none());
        assert!(migrated.list_telegram_bindings().unwrap().is_empty());
        let conn = migrated.connection().unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM audit_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            audit_before
        );
        drop(conn);
        let binding = migrated
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        assert_eq!(binding.backend_id, "alice");
        assert_eq!(
            migrated
                .admit_telegram(
                    binding.id,
                    Uuid::now_v7(),
                    "telegram_execute",
                    &binding.id.to_string()
                )
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Held)
        );
        assert_eq!(
            Registry::open(&registry.path)
                .unwrap()
                .list_telegram_bindings()
                .unwrap(),
            vec![binding]
        );
    }

    #[test]
    fn telegram_ownership_and_revocation_are_permanent_and_independent_of_keys() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = registry
            .add_telegram_binding(alice.user_id, "101", "9223372036854775807")
            .unwrap();
        let rotated = registry.rotate(alice.key_id).unwrap();
        registry.revoke(rotated.key_id).unwrap();
        assert_eq!(
            registry.telegram_authorized(binding.id).unwrap(),
            Some(binding.clone())
        );
        assert!(registry
            .add_telegram_binding(alice.user_id, "102", "202")
            .is_err());
        assert!(registry
            .add_telegram_binding(bob.user_id, "101", "202")
            .is_err());
        let conn = registry.connection().unwrap();
        for sql in [
            "UPDATE telegram_bindings SET sender_id='300'",
            "UPDATE telegram_bindings SET bot_id='300'",
            "UPDATE telegram_bindings SET backend_id='bob'",
            "DELETE FROM telegram_bindings",
            "UPDATE users SET backend_id='changed' WHERE backend_id='alice'",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
        registry.revoke_telegram_binding(binding.id).unwrap();
        registry.revoke_telegram_binding(binding.id).unwrap();
        registry.set_enabled(alice.user_id, false).unwrap();
        registry.set_enabled(alice.user_id, true).unwrap();
        registry.add_key(alice.user_id).unwrap();
        assert!(registry.telegram_authorized(binding.id).unwrap().is_none());
        assert!(conn
            .execute("UPDATE telegram_bindings SET enabled=1", [])
            .is_err());
        assert!(registry
            .add_telegram_binding(alice.user_id, "102", "202")
            .is_err());
        assert!(registry
            .add_telegram_binding(bob.user_id, "101", "202")
            .is_err());
        let history = registry.list_telegram_bindings().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, binding.id);
        assert!(!history[0].enabled);
        let encoded = serde_json::to_string(&history).unwrap();
        assert!(!encoded.contains(&alice.token) && !encoded.contains("verifier"));
    }

    #[test]
    fn telegram_disable_and_unknown_holds_cannot_be_bypassed_by_revocation_or_restart() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        let object = binding.id.to_string();
        registry.set_enabled(alice.user_id, false).unwrap();
        assert!(registry.telegram_authorized(binding.id).unwrap().is_none());
        assert_eq!(
            registry
                .admit_telegram(binding.id, Uuid::now_v7(), "telegram_execute", &object)
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
        registry.set_enabled(alice.user_id, true).unwrap();
        let request = Uuid::now_v7();
        assert_eq!(
            registry
                .admit_telegram(binding.id, request, "telegram_execute", &object)
                .unwrap(),
            binding
        );
        // A read snapshot is allowed during a hold but grants no new admission.
        assert!(registry.telegram_authorized(binding.id).unwrap().is_some());
        assert!(registry
            .admit_telegram(binding.id, Uuid::now_v7(), "telegram_send", &object)
            .is_err());
        assert!(registry
            .admit_write(&principal(registry, &alice), Uuid::new_v4())
            .is_err());
        assert!(registry
            .admit_scheduled(alice.user_id, "alice", Uuid::now_v7())
            .is_err());
        let bob_request = Uuid::new_v4();
        registry
            .admit_write(&principal(registry, &bob), bob_request)
            .unwrap();
        registry
            .finish_write(bob.user_id, bob_request, true)
            .unwrap();
        let reopened = Registry::open(&registry.path).unwrap();
        assert_eq!(reopened.recover_writes().unwrap(), 1);
        reopened.finish_write(alice.user_id, request, true).unwrap();
        assert!(reopened
            .admit_telegram(binding.id, Uuid::now_v7(), "telegram_send", &object)
            .is_err());
        reopened.revoke_telegram_binding(binding.id).unwrap();
        reopened.set_enabled(alice.user_id, false).unwrap();
        reopened.set_enabled(alice.user_id, true).unwrap();
        let holds = reopened.list().unwrap();
        assert_eq!(
            holds
                .iter()
                .find(|user| user.user_id == alice.user_id)
                .unwrap()
                .hold
                .as_ref()
                .unwrap()
                .state,
            "needs_review"
        );
        reopened
            .clear_review(
                alice.user_id,
                "Stopped workers and reconciled recorded delivery",
            )
            .unwrap();
        assert_eq!(
            reopened
                .admit_telegram(binding.id, Uuid::now_v7(), "telegram_send", &object)
                .unwrap_err()
                .downcast_ref::<WriteAdmissionError>(),
            Some(&WriteAdmissionError::Unauthorized)
        );
    }

    #[test]
    fn telegram_rejects_noncanonical_ids_and_untrusted_audit_values_before_admission() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        for bad in [
            "",
            "0",
            "-1",
            "+1",
            "01",
            " 1",
            "1 ",
            "1.0",
            "1e2",
            "9223372036854775808",
            "１",
        ] {
            assert!(
                registry
                    .add_telegram_binding(alice.user_id, bad, "201")
                    .is_err(),
                "bot {bad}"
            );
            assert!(
                registry
                    .add_telegram_binding(alice.user_id, "101", bad)
                    .is_err(),
                "sender {bad}"
            );
        }
        assert!(registry
            .add_telegram_binding(Uuid::new_v4(), "101", "201")
            .is_err());
        assert!(registry.list_telegram_bindings().unwrap().is_empty());
        let binding = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        let object = binding.id.to_string();
        assert!(registry
            .admit_telegram(binding.id, Uuid::new_v4(), "telegram_send", &object)
            .is_err());
        let variant = Uuid::parse_str("12345678-1234-7234-1234-123456789012").unwrap();
        assert!(registry
            .admit_telegram(binding.id, variant, "telegram_send", &object)
            .is_err());
        for bad in [
            "not-uuid".to_owned(),
            Uuid::nil().to_string(),
            "ABCDEF01-2345-4678-9ABC-DEF012345678".into(),
            "abcdef01-2345-4678-1abc-def012345678".into(),
        ] {
            assert!(registry
                .admit_telegram(binding.id, Uuid::now_v7(), "telegram_send", &bad)
                .is_err());
        }
        assert!(registry
            .admit_telegram(binding.id, Uuid::now_v7(), "prompt/secret-body", &object)
            .is_err());
        assert!(registry.list().unwrap()[0].hold.is_none());
        assert!(registry
            .telegram_authorized(Uuid::new_v4())
            .unwrap()
            .is_none());
        let conn = registry.connection().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM audit_events WHERE action LIKE 'telegram_%'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(conn.query_row("SELECT count(*) FROM audit_events WHERE action LIKE '%secret%' OR note LIKE '%secret%'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn telegram_audit_failure_rolls_back_binding_revoke_and_hold() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let bob = registry.add_user("bob").unwrap();
        let binding = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        let conn = registry.connection().unwrap();
        conn.execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
        assert!(registry
            .add_telegram_binding(bob.user_id, "102", "202")
            .is_err());
        assert!(registry.revoke_telegram_binding(binding.id).is_err());
        assert!(registry
            .admit_telegram(
                binding.id,
                Uuid::now_v7(),
                "telegram_execute",
                &binding.id.to_string()
            )
            .is_err());
        assert_eq!(
            registry.list_telegram_bindings().unwrap(),
            vec![binding.clone()]
        );
        assert!(registry
            .list()
            .unwrap()
            .iter()
            .all(|user| user.hold.is_none()));
        conn.execute_batch("DROP TRIGGER fail_audit").unwrap();
        assert!(registry
            .add_telegram_binding(bob.user_id, "102", "202")
            .is_ok());
        let request = Uuid::now_v7();
        registry
            .admit_telegram(
                binding.id,
                request,
                "telegram_send",
                &binding.id.to_string(),
            )
            .unwrap();
        registry.finish_write(alice.user_id, request, true).unwrap();
        assert!(registry
            .list()
            .unwrap()
            .iter()
            .all(|user| user.hold.is_none()));
    }

    #[test]
    fn telegram_foreground_and_scheduled_admissions_have_one_transactional_winner() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let alice = registry.add_user("alice").unwrap();
        let owner = principal(registry, &alice);
        let binding = registry
            .add_telegram_binding(alice.user_id, "101", "201")
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = (0..3)
            .map(|index| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                let owner = owner.clone();
                let binding = binding.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    match index {
                        0 => registry.admit_write(&owner, Uuid::new_v4()),
                        1 => registry.admit_scheduled(
                            owner.user_id,
                            &owner.backend_id,
                            Uuid::now_v7(),
                        ),
                        _ => registry
                            .admit_telegram(
                                binding.id,
                                Uuid::now_v7(),
                                "telegram_execute",
                                &binding.id.to_string(),
                            )
                            .map(|_| ()),
                    }
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().err().is_some_and(|error| error
                    .downcast_ref::<WriteAdmissionError>(
                ) == Some(
                    &WriteAdmissionError::Held
                )))
                .count(),
            2
        );
    }

    #[test]
    fn telegram_lifetime_capacity_and_admission_audit_are_bounded_without_secrets() {
        let fixture = Fixture::new();
        let registry = &fixture.registry;
        let mut first = None;
        for index in 1..=MAX_TELEGRAM_BINDINGS {
            let user = registry.add_user(&format!("backend-{index}")).unwrap();
            let binding = registry
                .add_telegram_binding(user.user_id, &index.to_string(), "123")
                .unwrap();
            if index == 1 {
                first = Some(binding);
            } else {
                registry.revoke_telegram_binding(binding.id).unwrap();
            }
        }
        let first = first.unwrap();
        assert_eq!(
            registry.list_telegram_bindings().unwrap().len(),
            MAX_TELEGRAM_BINDINGS as usize
        );
        assert!(registry
            .add_telegram_binding(first.user_id, "1000", "123")
            .unwrap_err()
            .to_string()
            .contains("lifetime binding limit"));
        let conn = registry.connection().unwrap();
        conn.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO audit_events(user_id,action,note,created_ms) SELECT ?2,'seed','fixture',0 FROM n",
            params![MAX_AUDIT_EVENTS,first.user_id.to_string()]).unwrap();
        let request = Uuid::now_v7();
        registry
            .admit_telegram(first.id, request, "telegram_send", &first.id.to_string())
            .unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM audit_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            MAX_AUDIT_EVENTS
        );
        let (user,key,recorded,action,note):(String,Option<String>,String,String,String)=conn.query_row("SELECT user_id,key_id,request_id,action,note FROM audit_events ORDER BY seq DESC LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
        assert_eq!(user, first.user_id.to_string());
        assert!(key.is_none());
        assert_eq!(recorded, request.to_string());
        assert_eq!(action, "telegram_send");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&note).unwrap(),
            serde_json::json!({"binding_id":first.id.to_string(),"object_id":first.id.to_string()})
        );
        assert!(note.len() <= 512);
    }
}
