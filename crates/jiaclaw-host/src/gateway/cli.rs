// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Local administrator commands. Issued tokens are printed once, only after commit.
use super::{
    config::Config,
    registry::{IssuedKey, Registry},
};
use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use serde_json::json;
use std::{io::Write, path::PathBuf};
use uuid::Uuid;

#[derive(Subcommand)]
pub enum Commands {
    /// Run the authenticated per-user gateway.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Provision a user for one configured dedicated backend and issue its first key.
    UserAdd {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        backend: String,
        /// Restrict this key to read-only HTTP access; user-owned background work is unchanged.
        #[arg(long)]
        read_only: bool,
    },
    /// Issue an additional key for an existing enabled user.
    KeyAdd {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        /// Restrict this key to read-only HTTP access; user-owned background work is unchanged.
        #[arg(long)]
        read_only: bool,
    },
    /// List bounded key metadata for a user, including revoked keys; no credentials.
    KeyList {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=1024))]
        offset: u16,
    },
    /// Atomically replace a key, preserving its access and revoking the old key.
    KeyRotate {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        key: Uuid,
    },
    /// Revoke a key for future admissions; existing backend work is not rolled back.
    KeyRevoke {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        key: Uuid,
    },
    /// Disable future gateway access; already-submitted backend work is not rolled back.
    UserDisable {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
    },
    /// Re-enable gateway access without reviving revoked keys.
    UserEnable {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
    },
    /// List users and key counts without exposing tokens.
    UserList {
        #[arg(long)]
        config: PathBuf,
    },
    /// Permanently bind one enabled user's backend to a Telegram bot and private sender.
    TelegramBind {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        #[arg(long)]
        bot_id: String,
        #[arg(long)]
        sender_id: String,
    },
    /// List all Telegram bindings, including permanent revocations; no bot tokens.
    TelegramBindings {
        #[arg(long)]
        config: PathBuf,
    },
    /// Permanently revoke a Telegram binding without releasing its user or bot reservation.
    TelegramRevoke {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
    },
    /// Inspect private Telegram queues after stopping gateway. No credentials are printed.
    TelegramInspect {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long, value_parser = ["events", "deliveries", "operations"])]
        kind: String,
        #[arg(long)]
        event: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Record a verified platform receipt for an unknown send; never send again.
    TelegramResolve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        delivery: String,
        #[arg(long)]
        receipt: String,
    },
    /// Cancel an event's remaining sends after external review; requires stopped gateway.
    TelegramCancel {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Purge only fully resolved Telegram events, retaining dedup tombstones.
    TelegramPurge {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Clear an uncertain write hold only after checking that its backend is idle.
    ReviewClear {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        #[arg(long)]
        confirm_backend_idle: bool,
        #[arg(long)]
        note: String,
    },
}

fn registry(path: &std::path::Path) -> Result<(Config, Registry)> {
    let config = Config::load(path)?;
    let registry = Registry::open(&config.registry_path)?;
    Ok((config, registry))
}

fn output(value: &serde_json::Value) -> Result<()> {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    serde_json::to_writer(&mut locked, value).context("cannot write command result")?;
    locked
        .write_all(b"\n")
        .context("cannot write command result")?;
    locked.flush().context("cannot flush command result")
}

fn issued(key: IssuedKey) -> Result<()> {
    output(
        &json!({"user_id":key.user_id.to_string(),"key_id":key.key_id.to_string(),"token":key.token,"read_only":key.read_only}),
    )
}

pub async fn run(command: Commands) -> Result<()> {
    match command {
        Commands::Serve { config } => super::serve(Config::load(&config)?).await,
        Commands::UserAdd {
            config,
            backend,
            read_only,
        } => {
            let config = Config::load(&config)?;
            ensure!(
                config.backends.iter().any(|entry| entry.id == backend),
                "backend is not configured"
            );
            let registry = Registry::open(&config.registry_path)?;
            issued(if read_only {
                registry.add_user_with_access(&backend, true)?
            } else {
                registry.add_user(&backend)?
            })
        }
        Commands::KeyAdd {
            config,
            user,
            read_only,
        } => {
            let (_, registry) = registry(&config)?;
            issued(if read_only {
                registry.add_key_with_access(user, true)?
            } else {
                registry.add_key(user)?
            })
        }
        Commands::KeyList {
            config,
            user,
            limit,
            offset,
        } => {
            let (_, registry) = registry(&config)?;
            output(
                &json!({"keys":registry.list_keys(user,usize::from(limit),usize::from(offset))?}),
            )
        }
        Commands::KeyRotate { config, key } => {
            let (_, registry) = registry(&config)?;
            issued(registry.rotate(key)?)
        }
        Commands::KeyRevoke { config, key } => {
            let (_, registry) = registry(&config)?;
            registry.revoke(key)?;
            output(&json!({"key_id":key.to_string(),"revoked":true}))
        }
        Commands::UserDisable { config, user } => {
            let (_, registry) = registry(&config)?;
            registry.set_enabled(user, false)?;
            output(&json!({"user_id":user.to_string(),"enabled":false}))
        }
        Commands::UserEnable { config, user } => {
            let (_, registry) = registry(&config)?;
            registry.set_enabled(user, true)?;
            output(&json!({"user_id":user.to_string(),"enabled":true}))
        }
        Commands::UserList { config } => {
            let (_, registry) = registry(&config)?;
            output(&json!({"users":registry.list()?}))
        }
        Commands::TelegramBind {
            config,
            user,
            bot_id,
            sender_id,
        } => {
            let (config, registry) = registry(&config)?;
            let owner = registry
                .list()?
                .into_iter()
                .find(|entry| entry.user_id == user)
                .context("gateway user not found")?;
            ensure!(
                config
                    .backends
                    .iter()
                    .any(|entry| entry.id == owner.backend_id),
                "user backend is not configured"
            );
            output(&serde_json::to_value(
                registry.add_telegram_binding(user, &bot_id, &sender_id)?,
            )?)
        }
        Commands::TelegramBindings { config } => {
            let (_, registry) = registry(&config)?;
            output(&json!({"bindings":registry.list_telegram_bindings()?}))
        }
        Commands::TelegramRevoke { config, binding } => {
            let (_, registry) = registry(&config)?;
            registry.revoke_telegram_binding(binding)?;
            output(&json!({"binding_id":binding.to_string(),"revoked":true}))
        }
        Commands::TelegramInspect {
            config,
            binding,
            kind,
            event,
            limit,
            offset,
        } => output(&super::telegram::admin(
            &Config::load(&config)?,
            binding,
            super::telegram::AdminAction::Inspect {
                kind,
                event,
                limit,
                offset,
            },
        )?),
        Commands::TelegramResolve {
            config,
            binding,
            delivery,
            receipt,
        } => output(&super::telegram::admin(
            &Config::load(&config)?,
            binding,
            super::telegram::AdminAction::Resolve { delivery, receipt },
        )?),
        Commands::TelegramCancel {
            config,
            binding,
            event,
        } => output(&super::telegram::admin(
            &Config::load(&config)?,
            binding,
            super::telegram::AdminAction::Cancel { event },
        )?),
        Commands::TelegramPurge {
            config,
            binding,
            event,
        } => output(&super::telegram::admin(
            &Config::load(&config)?,
            binding,
            super::telegram::AdminAction::Purge { event },
        )?),
        Commands::ReviewClear {
            config,
            user,
            confirm_backend_idle,
            note,
        } => {
            ensure!(
                confirm_backend_idle,
                "review-clear requires --confirm-backend-idle after checking the backend"
            );
            let (_, registry) = registry(&config)?;
            let current_config = Config::load(&config)?;
            let _channel_guard = super::telegram::review_guard(&current_config, &registry, user)?;
            registry.clear_review(user, &note)?;
            output(&json!({"user_id":user.to_string(),"review_cleared":true}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Args {
        #[command(subcommand)]
        command: Commands,
    }

    #[test]
    fn admin_commands_require_explicit_ids_and_do_not_accept_tokens_as_arguments() {
        let user = "12345678-1234-4234-9234-123456789012";
        assert!(Args::try_parse_from([
            "gateway",
            "user-add",
            "--config",
            "c.json",
            "--backend",
            "alice"
        ])
        .is_ok());
        assert!(
            Args::try_parse_from(["gateway", "key-add", "--config", "c.json", "--user", user])
                .is_ok()
        );
        assert!(Args::try_parse_from([
            "gateway",
            "key-revoke",
            "--config",
            "c.json",
            "--key",
            "not-a-uuid"
        ])
        .is_err());
        assert!(Args::try_parse_from([
            "gateway",
            "key-add",
            "--config",
            "c.json",
            "--user",
            user,
            "--token",
            "provided-secret"
        ])
        .is_err());
        assert!(Args::try_parse_from(["gateway", "serve"]).is_err());
    }

    #[test]
    fn permissions_default_to_full_and_rotation_cannot_upgrade_access() {
        let user = "12345678-1234-4234-9234-123456789012";
        for read_only in [false, true] {
            let mut add = vec![
                "gateway",
                "user-add",
                "--config",
                "c.json",
                "--backend",
                "alice",
            ];
            if read_only {
                add.push("--read-only");
            }
            let Commands::UserAdd {
                read_only: actual, ..
            } = Args::try_parse_from(add).unwrap().command
            else {
                panic!("wrong command")
            };
            assert_eq!(actual, read_only);
            let mut add = vec!["gateway", "key-add", "--config", "c.json", "--user", user];
            if read_only {
                add.push("--read-only");
            }
            let Commands::KeyAdd {
                read_only: actual, ..
            } = Args::try_parse_from(add).unwrap().command
            else {
                panic!("wrong command")
            };
            assert_eq!(actual, read_only);
        }
        assert!(Args::try_parse_from([
            "gateway",
            "key-rotate",
            "--config",
            "c.json",
            "--key",
            user,
            "--read-only"
        ])
        .is_err());
        assert!(Args::try_parse_from([
            "gateway",
            "key-rotate",
            "--config",
            "c.json",
            "--key",
            user,
            "--full-access"
        ])
        .is_err());
    }

    #[test]
    fn key_metadata_pagination_requires_user_and_bounded_values() {
        let user = "12345678-1234-4234-9234-123456789012";
        let Commands::KeyList { limit, offset, .. } =
            Args::try_parse_from(["gateway", "key-list", "--config", "c.json", "--user", user])
                .unwrap()
                .command
        else {
            panic!("wrong command")
        };
        assert_eq!((limit, offset), (20, 0));
        for (limit, offset) in [("1", "0"), ("100", "1024")] {
            assert!(Args::try_parse_from([
                "gateway", "key-list", "--config", "c.json", "--user", user, "--limit", limit,
                "--offset", offset
            ])
            .is_ok());
        }
        for (limit, offset) in [
            ("0", "0"),
            ("101", "0"),
            ("1", "1025"),
            ("-1", "0"),
            ("1", "-1"),
        ] {
            assert!(Args::try_parse_from([
                "gateway", "key-list", "--config", "c.json", "--user", user, "--limit", limit,
                "--offset", offset
            ])
            .is_err());
        }
        assert!(Args::try_parse_from(["gateway", "key-list", "--config", "c.json"]).is_err());
        assert!(Args::try_parse_from([
            "gateway", "key-list", "--config", "c.json", "--user", user, "--token", "secret"
        ])
        .is_err());
    }

    #[tokio::test]
    async fn review_clear_requires_confirmation_before_opening_any_registry() {
        let error = run(Commands::ReviewClear {
            config: PathBuf::from("/not/opened.json"),
            user: Uuid::new_v4(),
            confirm_backend_idle: false,
            note: "checked".into(),
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("--confirm-backend-idle"));
    }

    #[test]
    fn telegram_commands_require_explicit_identity_and_never_accept_bot_credentials_or_backend() {
        let id = "12345678-1234-4234-9234-123456789012";
        assert!(Args::try_parse_from([
            "gateway",
            "telegram-bind",
            "--config",
            "c.json",
            "--user",
            id,
            "--bot-id",
            "101",
            "--sender-id",
            "201"
        ])
        .is_ok());
        assert!(
            Args::try_parse_from(["gateway", "telegram-bindings", "--config", "c.json"]).is_ok()
        );
        assert!(Args::try_parse_from([
            "gateway",
            "telegram-revoke",
            "--config",
            "c.json",
            "--binding",
            id
        ])
        .is_ok());
        assert!(Args::try_parse_from([
            "gateway",
            "telegram-bind",
            "--config",
            "c.json",
            "--user",
            id,
            "--bot-id",
            "101"
        ])
        .is_err());
        for (flag, value) in [("--token", "secret"), ("--backend", "other")] {
            assert!(Args::try_parse_from([
                "gateway",
                "telegram-bind",
                "--config",
                "c.json",
                "--user",
                id,
                "--bot-id",
                "101",
                "--sender-id",
                "201",
                flag,
                value
            ])
            .is_err());
        }
        assert!(Args::try_parse_from([
            "gateway",
            "telegram-revoke",
            "--config",
            "c.json",
            "--binding",
            "not-uuid"
        ])
        .is_err());
        assert!(Args::try_parse_from(["gateway", "telegram-bindings"]).is_err());
    }

    #[tokio::test]
    async fn telegram_bind_checks_the_immutable_users_backend_is_configured_before_mutation() {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir =
            Cleanup(std::env::temp_dir().join(format!("jiaclaw-telegram-cli-{}", Uuid::new_v4())));
        let registry_path = dir.0.join("private/users.sqlite3");
        let registry = Registry::open(&registry_path).unwrap();
        let user = registry.add_user("alice").unwrap();
        let config = dir.0.join("gateway.json");
        std::fs::write(&config,serde_json::to_vec(&json!({
            "registry_path":registry_path,
            "backends":[{"id":"bob","url":"http://127.0.0.1:18080","token_file":dir.0.join("backend-token")}]
        })).unwrap()).unwrap();
        let error = run(Commands::TelegramBind {
            config,
            user: user.user_id,
            bot_id: "101".into(),
            sender_id: "201".into(),
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("backend is not configured"));
        assert!(registry.list_telegram_bindings().unwrap().is_empty());
    }
}
