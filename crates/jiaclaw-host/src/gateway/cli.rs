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
    /// Query bounded retained audit metadata for one user; notes require explicit opt-in.
    AuditList {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        /// Continue after this exact decimal sequence from a previous result.
        #[arg(long, default_value_t = 0, value_parser = audit_after_seq)]
        after_seq: u64,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Include private administrator notes in the JSON output.
        #[arg(long)]
        include_notes: bool,
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
    /// Permanently bind a dedicated Slack app installation and private DM to one user.
    SlackBind {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        #[arg(long)]
        team_id: String,
        #[arg(long)]
        app_id: String,
        #[arg(long)]
        bot_user_id: String,
        #[arg(long)]
        bot_id: String,
        #[arg(long)]
        sender_id: String,
        #[arg(long)]
        conversation_id: String,
    },
    /// List lifetime Slack bindings, including permanent revocations; no credentials.
    SlackBindings {
        #[arg(long)]
        config: PathBuf,
    },
    /// Permanently revoke a Slack installation without freeing its owner reservation.
    SlackRevoke {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
    },
    /// Inspect private Slack queues after stopping gateway; no credentials are printed.
    SlackInspect {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long, value_parser = ["events", "deliveries", "operations"])]
        kind: String,
        #[arg(long)]
        event: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=16000))]
        offset: u16,
    },
    /// Record a verified Slack receipt for an unknown send, without sending again.
    SlackResolve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        delivery: String,
        #[arg(long)]
        receipt: String,
    },
    /// Cancel remaining Slack sends after external review; requires stopped gateway.
    SlackCancel {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Purge only fully resolved Slack events, retaining dedup tombstones.
    SlackPurge {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Permanently bind one user-installed Discord application and private bot DM to one user.
    DiscordBind {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        #[arg(long)]
        application_id: String,
        #[arg(long)]
        /// Public Ed25519 verification pin (64 lowercase hexadecimal characters).
        verify_key: String,
        #[arg(long)]
        bot_user_id: String,
        #[arg(long)]
        sender_id: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        command_id: String,
    },
    /// List lifetime Discord bindings, including permanent revocations; no credentials.
    DiscordBindings {
        #[arg(long)]
        config: PathBuf,
    },
    /// Permanently revoke a Discord installation without freeing its owner reservation.
    DiscordRevoke {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
    },
    /// Inspect private Discord queues after stopping gateway; no credentials are printed.
    DiscordInspect {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long, value_parser = ["events", "deliveries", "operations"])]
        kind: String,
        #[arg(long)]
        event: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=16000))]
        offset: u16,
    },
    /// Record a verified Discord receipt for an unknown send, without sending again.
    DiscordResolve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        delivery: String,
        #[arg(long)]
        receipt: String,
    },
    /// Cancel remaining Discord sends after external review; requires stopped gateway.
    DiscordCancel {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Purge only fully resolved Discord events, retaining dedup tombstones.
    DiscordPurge {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Permanently bind a dedicated Feishu enterprise application and private chat to one user.
    FeishuBind {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user: Uuid,
        #[arg(long)]
        app_id: String,
        #[arg(long)]
        tenant_key: String,
        #[arg(long)]
        bot_open_id: String,
        #[arg(long)]
        human_open_id: String,
        #[arg(long)]
        chat_id: String,
    },
    /// List lifetime Feishu bindings, including permanent revocations; no credentials.
    FeishuBindings {
        #[arg(long)]
        config: PathBuf,
    },
    /// Permanently revoke a Feishu installation without freeing its owner reservation.
    FeishuRevoke {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
    },
    /// Inspect private Feishu queues after stopping gateway; no credentials are printed.
    FeishuInspect {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long, value_parser = ["events", "deliveries", "operations"])]
        kind: String,
        #[arg(long)]
        event: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=16000))]
        offset: u16,
    },
    /// Record a verified Feishu receipt for an unknown send, without sending again.
    FeishuResolve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        delivery: String,
        #[arg(long)]
        receipt: String,
    },
    /// Cancel remaining Feishu sends after external review; requires stopped gateway.
    FeishuCancel {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        binding: Uuid,
        #[arg(long)]
        event: String,
    },
    /// Purge only fully resolved Feishu events, retaining dedup tombstones.
    FeishuPurge {
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

fn audit_after_seq(raw: &str) -> std::result::Result<u64, String> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("audit cursor must contain decimal digits only".into());
    }
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| "audit cursor must be in 0..9223372036854775807".into())
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
        Commands::AuditList {
            config,
            user,
            after_seq,
            limit,
            include_notes,
        } => {
            let (_, registry) = registry(&config)?;
            let page = registry.audit_list(user, after_seq, usize::from(limit), include_notes)?;
            output(&serde_json::to_value(page).context("cannot serialize gateway audit result")?)
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
        Commands::SlackBind {
            config,
            user,
            team_id,
            app_id,
            bot_user_id,
            bot_id,
            sender_id,
            conversation_id,
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
            output(&serde_json::to_value(registry.add_slack_binding(
                user,
                &team_id,
                &app_id,
                &bot_user_id,
                &bot_id,
                &sender_id,
                &conversation_id,
            )?)?)
        }
        Commands::SlackBindings { config } => {
            let (_, registry) = registry(&config)?;
            output(&json!({"bindings":registry.list_slack_bindings()?}))
        }
        Commands::SlackRevoke { config, binding } => {
            let (_, registry) = registry(&config)?;
            registry.revoke_slack_binding(binding)?;
            output(&json!({"binding_id":binding.to_string(),"revoked":true}))
        }
        Commands::SlackInspect {
            config,
            binding,
            kind,
            event,
            limit,
            offset,
        } => output(&super::slack::admin(
            &Config::load(&config)?,
            binding,
            super::slack::AdminAction::Inspect {
                kind,
                event,
                limit: usize::from(limit),
                offset: usize::from(offset),
            },
        )?),
        Commands::SlackResolve {
            config,
            binding,
            delivery,
            receipt,
        } => output(&super::slack::admin(
            &Config::load(&config)?,
            binding,
            super::slack::AdminAction::Resolve { delivery, receipt },
        )?),
        Commands::SlackCancel {
            config,
            binding,
            event,
        } => output(&super::slack::admin(
            &Config::load(&config)?,
            binding,
            super::slack::AdminAction::Cancel { event },
        )?),
        Commands::SlackPurge {
            config,
            binding,
            event,
        } => output(&super::slack::admin(
            &Config::load(&config)?,
            binding,
            super::slack::AdminAction::Purge { event },
        )?),
        Commands::DiscordBind {
            config,
            user,
            application_id,
            verify_key,
            bot_user_id,
            sender_id,
            conversation_id,
            command_id,
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
            output(&serde_json::to_value(registry.add_discord_binding(
                user,
                &application_id,
                &verify_key,
                &bot_user_id,
                &sender_id,
                &conversation_id,
                &command_id,
            )?)?)
        }
        Commands::DiscordBindings { config } => {
            let (_, registry) = registry(&config)?;
            output(&json!({"bindings":registry.list_discord_bindings()?}))
        }
        Commands::DiscordRevoke { config, binding } => {
            let (_, registry) = registry(&config)?;
            registry.revoke_discord_binding(binding)?;
            output(&json!({"binding_id":binding.to_string(),"revoked":true}))
        }
        Commands::DiscordInspect {
            config,
            binding,
            kind,
            event,
            limit,
            offset,
        } => output(&super::discord::admin(
            &Config::load(&config)?,
            binding,
            super::discord::AdminAction::Inspect {
                kind,
                event,
                limit: usize::from(limit),
                offset: usize::from(offset),
            },
        )?),
        Commands::DiscordResolve {
            config,
            binding,
            delivery,
            receipt,
        } => output(&super::discord::admin(
            &Config::load(&config)?,
            binding,
            super::discord::AdminAction::Resolve { delivery, receipt },
        )?),
        Commands::DiscordCancel {
            config,
            binding,
            event,
        } => output(&super::discord::admin(
            &Config::load(&config)?,
            binding,
            super::discord::AdminAction::Cancel { event },
        )?),
        Commands::DiscordPurge {
            config,
            binding,
            event,
        } => output(&super::discord::admin(
            &Config::load(&config)?,
            binding,
            super::discord::AdminAction::Purge { event },
        )?),
        Commands::FeishuBind {
            config,
            user,
            app_id,
            tenant_key,
            bot_open_id,
            human_open_id,
            chat_id,
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
            output(&serde_json::to_value(registry.add_feishu_binding(
                user,
                &app_id,
                &tenant_key,
                &bot_open_id,
                &human_open_id,
                &chat_id,
            )?)?)
        }
        Commands::FeishuBindings { config } => {
            let (_, registry) = registry(&config)?;
            output(&json!({"bindings":registry.list_feishu_bindings()?}))
        }
        Commands::FeishuRevoke { config, binding } => {
            let (_, registry) = registry(&config)?;
            registry.revoke_feishu_binding(binding)?;
            output(&json!({"binding_id":binding.to_string(),"revoked":true}))
        }
        Commands::FeishuInspect {
            config,
            binding,
            kind,
            event,
            limit,
            offset,
        } => output(&super::feishu::admin(
            &Config::load(&config)?,
            binding,
            super::feishu::AdminAction::Inspect {
                kind,
                event,
                limit: usize::from(limit),
                offset: usize::from(offset),
            },
        )?),
        Commands::FeishuResolve {
            config,
            binding,
            delivery,
            receipt,
        } => output(&super::feishu::admin(
            &Config::load(&config)?,
            binding,
            super::feishu::AdminAction::Resolve { delivery, receipt },
        )?),
        Commands::FeishuCancel {
            config,
            binding,
            event,
        } => output(&super::feishu::admin(
            &Config::load(&config)?,
            binding,
            super::feishu::AdminAction::Cancel { event },
        )?),
        Commands::FeishuPurge {
            config,
            binding,
            event,
        } => output(&super::feishu::admin(
            &Config::load(&config)?,
            binding,
            super::feishu::AdminAction::Purge { event },
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
            let (current_config, registry) = registry(&config)?;
            let _channel_guard = super::channel_review_guard(&current_config, &registry, user)?;
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
    fn slack_commands_require_complete_explicit_ownership_and_never_accept_credentials() {
        let user = "12345678-1234-4234-9234-123456789012";
        let base = [
            "gateway",
            "slack-bind",
            "--config",
            "c.json",
            "--user",
            user,
            "--team-id",
            "T1",
            "--app-id",
            "A1",
            "--bot-user-id",
            "W10",
            "--bot-id",
            "B1",
            "--sender-id",
            "U11",
            "--conversation-id",
            "D1",
        ];
        let Commands::SlackBind {
            user: actual,
            team_id,
            app_id,
            bot_user_id,
            bot_id,
            sender_id,
            conversation_id,
            ..
        } = Args::try_parse_from(base).unwrap().command
        else {
            panic!("wrong command")
        };
        assert_eq!(actual.to_string(), user);
        assert_eq!(
            (
                team_id.as_str(),
                app_id.as_str(),
                bot_user_id.as_str(),
                bot_id.as_str(),
                sender_id.as_str(),
                conversation_id.as_str()
            ),
            ("T1", "A1", "W10", "B1", "U11", "D1")
        );
        for index in (2..base.len()).step_by(2) {
            let mut missing = base.to_vec();
            missing.drain(index..index + 2);
            assert!(Args::try_parse_from(missing).is_err());
        }
        for flag in ["--token", "--bot-token", "--signing-secret", "--backend"] {
            let mut supplied = base.to_vec();
            supplied.extend([flag, "provided-secret"]);
            assert!(Args::try_parse_from(supplied).is_err());
        }
        assert!(Args::try_parse_from(["gateway", "slack-bindings", "--config", "c.json"]).is_ok());
        for command in [
            "slack-revoke",
            "slack-inspect",
            "slack-resolve",
            "slack-cancel",
            "slack-purge",
        ] {
            assert!(Args::try_parse_from(["gateway", command, "--config", "c.json"]).is_err());
        }
        assert!(Args::try_parse_from([
            "gateway",
            "slack-revoke",
            "--config",
            "c.json",
            "--binding",
            user
        ])
        .is_ok());
        assert!(Args::try_parse_from([
            "gateway",
            "slack-resolve",
            "--config",
            "c.json",
            "--binding",
            user,
            "--delivery",
            user,
            "--receipt",
            "slack:1234567890.000001"
        ])
        .is_ok());
        for command in ["slack-cancel", "slack-purge"] {
            assert!(Args::try_parse_from([
                "gateway",
                command,
                "--config",
                "c.json",
                "--binding",
                user,
                "--event",
                user
            ])
            .is_ok());
        }
    }

    #[test]
    fn slack_inspection_pages_and_kinds_are_bounded_before_dispatch() {
        let binding = "12345678-1234-4234-9234-123456789012";
        let base = [
            "gateway",
            "slack-inspect",
            "--config",
            "c.json",
            "--binding",
            binding,
            "--kind",
            "operations",
        ];
        let Commands::SlackInspect { limit, offset, .. } =
            Args::try_parse_from(base).unwrap().command
        else {
            panic!("wrong command")
        };
        assert_eq!((limit, offset), (20, 0));
        for (limit, offset) in [("1", "0"), ("100", "16000")] {
            let mut supplied = base.to_vec();
            supplied.extend(["--limit", limit, "--offset", offset]);
            assert!(Args::try_parse_from(supplied).is_ok());
        }
        for (limit, offset) in [
            ("0", "0"),
            ("101", "0"),
            ("-1", "0"),
            ("1", "16001"),
            ("1", "-1"),
        ] {
            let mut supplied = base.to_vec();
            supplied.extend(["--limit", limit, "--offset", offset]);
            assert!(Args::try_parse_from(supplied).is_err());
        }
        assert!(Args::try_parse_from([
            "gateway",
            "slack-inspect",
            "--config",
            "c.json",
            "--binding",
            binding,
            "--kind",
            "secrets"
        ])
        .is_err());
    }

    #[test]
    fn discord_commands_require_complete_explicit_ownership_and_never_accept_credentials() {
        let user = "12345678-1234-4234-9234-123456789012";
        let base = [
            "gateway",
            "discord-bind",
            "--config",
            "c.json",
            "--user",
            user,
            "--application-id",
            "1",
            "--verify-key",
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "--bot-user-id",
            "10",
            "--command-id",
            "13",
            "--sender-id",
            "11",
            "--conversation-id",
            "12",
        ];
        let Commands::DiscordBind {
            user: actual,
            application_id,
            verify_key,
            bot_user_id,
            command_id,
            sender_id,
            conversation_id,
            ..
        } = Args::try_parse_from(base).unwrap().command
        else {
            panic!("wrong command")
        };
        assert_eq!(actual.to_string(), user);
        assert_eq!(
            (
                application_id.as_str(),
                verify_key.as_str(),
                bot_user_id.as_str(),
                command_id.as_str(),
                sender_id.as_str(),
                conversation_id.as_str()
            ),
            (
                "1",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "10",
                "13",
                "11",
                "12"
            )
        );
        for index in (2..base.len()).step_by(2) {
            let mut missing = base.to_vec();
            missing.drain(index..index + 2);
            assert!(Args::try_parse_from(missing).is_err());
        }
        for flag in ["--token", "--bot-token", "--state-key", "--backend"] {
            let mut supplied = base.to_vec();
            supplied.extend([flag, "provided-secret"]);
            assert!(Args::try_parse_from(supplied).is_err());
        }
        assert!(
            Args::try_parse_from(["gateway", "discord-bindings", "--config", "c.json"]).is_ok()
        );
        for command in [
            "discord-revoke",
            "discord-inspect",
            "discord-resolve",
            "discord-cancel",
            "discord-purge",
        ] {
            assert!(Args::try_parse_from(["gateway", command, "--config", "c.json"]).is_err());
        }
        assert!(Args::try_parse_from([
            "gateway",
            "discord-revoke",
            "--config",
            "c.json",
            "--binding",
            user
        ])
        .is_ok());
        assert!(Args::try_parse_from([
            "gateway",
            "discord-resolve",
            "--config",
            "c.json",
            "--binding",
            user,
            "--delivery",
            user,
            "--receipt",
            "discord:123456789012345678"
        ])
        .is_ok());
        for command in ["discord-cancel", "discord-purge"] {
            assert!(Args::try_parse_from([
                "gateway",
                command,
                "--config",
                "c.json",
                "--binding",
                user,
                "--event",
                user
            ])
            .is_ok());
        }
    }

    #[test]
    fn discord_inspection_pages_and_kinds_are_bounded_before_dispatch() {
        let binding = "12345678-1234-4234-9234-123456789012";
        let base = [
            "gateway",
            "discord-inspect",
            "--config",
            "c.json",
            "--binding",
            binding,
            "--kind",
            "operations",
        ];
        let Commands::DiscordInspect { limit, offset, .. } =
            Args::try_parse_from(base).unwrap().command
        else {
            panic!("wrong command")
        };
        assert_eq!((limit, offset), (20, 0));
        for (limit, offset) in [("1", "0"), ("100", "16000")] {
            let mut supplied = base.to_vec();
            supplied.extend(["--limit", limit, "--offset", offset]);
            assert!(Args::try_parse_from(supplied).is_ok());
        }
        for (limit, offset) in [
            ("0", "0"),
            ("101", "0"),
            ("-1", "0"),
            ("1", "16001"),
            ("1", "-1"),
        ] {
            let mut supplied = base.to_vec();
            supplied.extend(["--limit", limit, "--offset", offset]);
            assert!(Args::try_parse_from(supplied).is_err());
        }
        assert!(Args::try_parse_from([
            "gateway",
            "discord-inspect",
            "--config",
            "c.json",
            "--binding",
            binding,
            "--kind",
            "secrets"
        ])
        .is_err());
    }

    #[test]
    fn feishu_commands_require_full_explicit_ownership_and_no_credentials() {
        let user = "12345678-1234-4234-9234-123456789012";
        let base = [
            "gateway",
            "feishu-bind",
            "--config",
            "c.json",
            "--user",
            user,
            "--app-id",
            "cli_app",
            "--tenant-key",
            "tenant",
            "--bot-open-id",
            "ou_bot",
            "--human-open-id",
            "ou_human",
            "--chat-id",
            "oc_chat",
        ];
        let Commands::FeishuBind {
            app_id,
            tenant_key,
            bot_open_id,
            human_open_id,
            chat_id,
            ..
        } = Args::try_parse_from(base).unwrap().command
        else {
            panic!("wrong command")
        };
        assert_eq!(
            (
                app_id.as_str(),
                tenant_key.as_str(),
                bot_open_id.as_str(),
                human_open_id.as_str(),
                chat_id.as_str()
            ),
            ("cli_app", "tenant", "ou_bot", "ou_human", "oc_chat")
        );
        for index in [4, 6, 8, 10, 12, 14] {
            let mut missing = base.to_vec();
            missing.drain(index..index + 2);
            assert!(Args::try_parse_from(missing).is_err());
        }
        for forbidden in [
            "--app-secret",
            "--encrypt-key",
            "--verification-token",
            "--backend-id",
            "--state-key",
            "--token",
        ] {
            let mut extra = base.to_vec();
            extra.extend([forbidden, "provided-secret"]);
            assert!(Args::try_parse_from(extra).is_err());
        }
        for command in [
            "feishu-bindings",
            "feishu-revoke",
            "feishu-inspect",
            "feishu-resolve",
            "feishu-cancel",
            "feishu-purge",
        ] {
            assert!(Args::try_parse_from(["gateway", command]).is_err());
        }
        assert!(Args::try_parse_from(["gateway", "feishu-bindings", "--config", "c.json"]).is_ok());
        assert!(Args::try_parse_from([
            "gateway",
            "feishu-revoke",
            "--config",
            "c.json",
            "--binding",
            user
        ])
        .is_ok());
        assert!(Args::try_parse_from([
            "gateway",
            "feishu-resolve",
            "--config",
            "c.json",
            "--binding",
            user,
            "--delivery",
            user,
            "--receipt",
            "om_platformReceipt"
        ])
        .is_ok());
        for command in ["feishu-cancel", "feishu-purge"] {
            assert!(Args::try_parse_from([
                "gateway",
                command,
                "--config",
                "c.json",
                "--binding",
                user,
                "--event",
                user
            ])
            .is_ok());
        }
    }

    #[test]
    fn feishu_inspection_pages_and_kinds_are_bounded_before_dispatch() {
        let binding = "12345678-1234-4234-9234-123456789012";
        for kind in ["events", "deliveries", "operations"] {
            let Commands::FeishuInspect { limit, offset, .. } = Args::try_parse_from([
                "gateway",
                "feishu-inspect",
                "--config",
                "c.json",
                "--binding",
                binding,
                "--kind",
                kind,
            ])
            .unwrap()
            .command
            else {
                panic!("wrong command")
            };
            assert_eq!((limit, offset), (20, 0));
            assert!(Args::try_parse_from([
                "gateway",
                "feishu-inspect",
                "--config",
                "c.json",
                "--binding",
                binding,
                "--kind",
                kind,
                "--limit",
                "100",
                "--offset",
                "16000"
            ])
            .is_ok());
        }
        for (field, value) in [
            ("--limit", "0"),
            ("--limit", "101"),
            ("--limit", "-1"),
            ("--offset", "16001"),
            ("--offset", "-1"),
        ] {
            assert!(Args::try_parse_from([
                "gateway",
                "feishu-inspect",
                "--config",
                "c.json",
                "--binding",
                binding,
                "--kind",
                "events",
                field,
                value
            ])
            .is_err());
        }
        assert!(Args::try_parse_from([
            "gateway",
            "feishu-inspect",
            "--config",
            "c.json",
            "--binding",
            binding,
            "--kind",
            "credentials"
        ])
        .is_err());
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
    fn audit_queries_require_user_and_exact_bounded_decimal_cursor() {
        let user = "12345678-1234-4234-9234-123456789012";
        let base = [
            "gateway",
            "audit-list",
            "--config",
            "c.json",
            "--user",
            user,
        ];
        let Commands::AuditList {
            after_seq,
            limit,
            include_notes,
            ..
        } = Args::try_parse_from(base).unwrap().command
        else {
            panic!("wrong command")
        };
        assert_eq!((after_seq, limit, include_notes), (0, 20, false));
        for cursor in ["0", "1", "9007199254740993", "9223372036854775807"] {
            let mut args = base.to_vec();
            args.extend(["--after-seq", cursor, "--limit", "100", "--include-notes"]);
            let Commands::AuditList {
                after_seq,
                limit,
                include_notes,
                ..
            } = Args::try_parse_from(args).unwrap().command
            else {
                panic!("wrong command")
            };
            assert_eq!(after_seq.to_string(), cursor);
            assert_eq!((limit, include_notes), (100, true));
        }
        for cursor in [
            "",
            "+1",
            "-1",
            "1.0",
            "0x1",
            " 1",
            "1 ",
            "9223372036854775808",
            "18446744073709551616",
        ] {
            let mut args = base.to_vec();
            args.extend(["--after-seq", cursor]);
            assert!(Args::try_parse_from(args).is_err());
        }
        for limit in ["0", "101", "-1"] {
            let mut args = base.to_vec();
            args.extend(["--limit", limit]);
            assert!(Args::try_parse_from(args).is_err());
        }
        assert!(Args::try_parse_from(["gateway", "audit-list", "--config", "c.json"]).is_err());
        let mut args = base.to_vec();
        args.extend(["--token", "secret"]);
        assert!(Args::try_parse_from(args).is_err());
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
