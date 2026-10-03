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
const SCHEMA_VERSION: i64 = 2;

/// Authenticated identity. Backend selection is never taken from client metadata.
#[derive(Clone, Debug)]
pub struct Principal {
    pub user_id: Uuid,
    pub key_id: Uuid,
    pub backend_id: String,
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

/// A credential has ceased to authorize admission, or an earlier write needs resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAdmissionError {
    Unauthorized,
    Held,
}
impl std::fmt::Display for WriteAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unauthorized => "gateway credential is no longer authorized",
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
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else {
            ensure!(
                (version == 1 || version == SCHEMA_VERSION) && application == APPLICATION_ID,
                "unsupported gateway registry schema or database identity"
            );
            if version == 1 {
                tx.execute_batch(SCHEMA_V2)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
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
        ensure!(
            !backend_id.is_empty()
                && backend_id.len() <= 64
                && backend_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
            "gateway backend ID must contain 1..64 ASCII letters, digits, underscores or hyphens"
        );
        let user_id = Uuid::new_v4();
        let (issued, verifier) = keys::issue(user_id)?;
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
            None,
            now,
        )?;
        tx.commit()?;
        Ok(issued)
    }

    /// Issue another key for an enabled user, up to eight active keys.
    pub fn add_key(&self, user_id: Uuid) -> Result<IssuedKey> {
        let (issued, verifier) = keys::issue(user_id)?;
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
            None,
            now,
        )?;
        tx.commit()?;
        Ok(issued)
    }

    /// Replace one active key atomically; the old key cannot authorize new requests after commit.
    pub fn rotate(&self, key_id: Uuid) -> Result<IssuedKey> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user: Option<String> = tx
            .query_row(
                "SELECT user_id FROM api_keys WHERE id=?1 AND revoked_ms IS NULL",
                [key_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let user_id = uuid(user.context("active gateway key not found")?)?;
        enabled_user(&tx, user_id)?;
        let (issued, verifier) = keys::issue(user_id)?;
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
            None,
            now,
        )?;
        audit(
            &tx,
            user_id,
            Some(issued.key_id),
            None,
            "key_rotated",
            None,
            now,
        )?;
        tx.commit()?;
        Ok(issued)
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

    /// Read live key/user state for every request. No successful-authentication cache exists.
    pub fn authenticate(&self, token: &str) -> Result<Option<Principal>> {
        let Some(parsed) = keys::parse(token) else {
            return Ok(None);
        };
        let conn = self.connection()?;
        let stored = conn.query_row("SELECT k.user_id,u.backend_id,k.verifier,u.enabled,k.revoked_ms IS NULL FROM api_keys k JOIN users u ON u.id=k.user_id WHERE k.id=?1", [parsed.key_id.to_string()], |row| {
            Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,bool>(3)?,row.get::<_,bool>(4)?))
        }).optional()?;
        if let Some((user, backend_id, verifier, enabled, active)) = stored {
            let user_id = uuid(user)?;
            let valid = keys::matches(&parsed.verifier(user_id), &verifier);
            if valid && enabled && active {
                return Ok(Some(Principal {
                    user_id,
                    key_id: parsed.key_id,
                    backend_id,
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
        let authorized: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM api_keys k JOIN users u ON u.id=k.user_id WHERE k.id=?1 AND u.id=?2 AND u.backend_id=?3 AND u.enabled=1 AND k.revoked_ms IS NULL)", params![principal.key_id.to_string(), principal.user_id.to_string(), principal.backend_id], |row| row.get(0))?;
        if !authorized {
            return Err(WriteAdmissionError::Unauthorized.into());
        }
        let now = now_ms();
        insert_hold(&tx, principal.user_id, request_id, now)?;
        audit(
            &tx,
            principal.user_id,
            Some(principal.key_id),
            Some(request_id),
            "write_admitted",
            None,
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
        "INSERT INTO api_keys(id,user_id,verifier,created_ms,revoked_ms) VALUES(?1,?2,?3,?4,NULL)",
        params![
            issued.key_id.to_string(),
            issued.user_id.to_string(),
            verifier.as_slice(),
            now
        ],
    )?;
    Ok(())
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
        let (unknown, _) = keys::issue(Uuid::new_v4()).unwrap();
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
        conn.pragma_update(None, "user_version", 3).unwrap();
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
        // No bindings exist. Removing only the v2 objects reconstructs the exact
        // v1 schema with genuine keys, revoked state, audit and pending work.
        conn.execute_batch("DROP TABLE telegram_bindings; DROP INDEX users_identity_backend; PRAGMA user_version=1;").unwrap();
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
            2
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
