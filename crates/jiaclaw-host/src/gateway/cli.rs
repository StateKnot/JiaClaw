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
    },
    /// Issue an additional key for an existing enabled user.
    KeyAdd {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
    },
    /// Atomically issue a replacement key and revoke the named old key.
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
        &json!({"user_id":key.user_id.to_string(),"key_id":key.key_id.to_string(),"token":key.token}),
    )
}

pub async fn run(command: Commands) -> Result<()> {
    match command {
        Commands::Serve { config } => super::serve(Config::load(&config)?).await,
        Commands::UserAdd { config, backend } => {
            let config = Config::load(&config)?;
            ensure!(
                config.backends.iter().any(|entry| entry.id == backend),
                "backend is not configured"
            );
            let registry = Registry::open(&config.registry_path)?;
            issued(registry.add_user(&backend)?)
        }
        Commands::KeyAdd { config, user } => {
            let (_, registry) = registry(&config)?;
            issued(registry.add_key(user)?)
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
}
