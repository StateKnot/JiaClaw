// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Administrator-owned, bounded gateway configuration; request data never selects a backend.
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    pub registry_path: PathBuf,
    pub backends: Vec<BackendConfig>,
    #[serde(default = "default_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_concurrency")]
    pub max_in_flight: usize,
    /// Administrator assertion that HTTP backends are confined to a trusted private network.
    #[serde(default)]
    pub allow_private_http: bool,
    /// Non-loopback listeners require an independently configured TLS reverse proxy.
    #[serde(default)]
    pub allow_remote_bind: bool,
    /// Opt in to user-owned cron on backends configured in gateway-driven mode.
    #[serde(default)]
    pub scheduled_jobs: bool,
    /// Dedicated Telegram private-chat installations, authorized by immutable registry bindings.
    #[serde(default)]
    pub telegram: Vec<TelegramConfig>,
    /// Dedicated Slack app installations, each bound to one user and private DM.
    #[serde(default)]
    pub slack: Vec<SlackConfig>,
    /// Dedicated user-installable Discord applications, each bound to one private bot DM.
    #[serde(default)]
    pub discord: Vec<DiscordConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub id: String,
    pub url: String,
    pub token_file: PathBuf,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    pub binding_id: String,
    pub bot_token_file: PathBuf,
    pub webhook_secret_file: PathBuf,
    #[serde(default = "telegram_api")]
    pub api_base: String,
    #[serde(default)]
    pub allow_loopback: bool,
}
fn telegram_api() -> String {
    "https://api.telegram.org".into()
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackConfig {
    pub binding_id: String,
    pub bot_token_file: PathBuf,
    pub signing_secret_file: PathBuf,
    #[serde(default = "slack_api")]
    pub api_base: String,
    #[serde(default)]
    pub allow_loopback: bool,
}
fn slack_api() -> String {
    "https://slack.com/api".into()
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordConfig {
    pub binding_id: String,
    pub bot_token_file: PathBuf,
    pub state_key_file: PathBuf,
    #[serde(default = "discord_api")]
    pub api_base: String,
    #[serde(default)]
    pub allow_loopback: bool,
}
fn discord_api() -> String {
    "https://discord.com/api/v10".into()
}

fn default_bind() -> String {
    "127.0.0.1:8081".into()
}
fn default_timeout() -> u64 {
    180
}
fn default_concurrency() -> usize {
    16
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        ensure!(
            std::fs::metadata(path)
                .context("cannot inspect gateway configuration")?
                .is_file(),
            "gateway configuration must be a regular file"
        );
        let mut file = File::open(path).context("cannot open gateway configuration")?;
        let metadata = file
            .metadata()
            .context("cannot inspect gateway configuration")?;
        ensure!(
            metadata.is_file(),
            "gateway configuration must be a regular file"
        );
        ensure!(
            metadata.len() <= MAX_CONFIG_BYTES,
            "gateway configuration exceeds 64 KiB"
        );
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("cannot read gateway configuration")?;
        ensure!(
            bytes.len() as u64 <= MAX_CONFIG_BYTES,
            "gateway configuration exceeds 64 KiB"
        );
        // Parser diagnostics can quote input values. Configuration may name secret files.
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid gateway configuration JSON or fields"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let bind: SocketAddr = self
            .bind
            .parse()
            .context("gateway bind must be a literal IP:port")?;
        ensure!(
            bind.ip().is_loopback() || self.allow_remote_bind,
            "non-loopback gateway bind requires allow_remote_bind and a TLS reverse proxy"
        );
        ensure!(
            self.registry_path.is_absolute(),
            "gateway registry_path must be absolute"
        );
        ensure!(
            (1..=32).contains(&self.backends.len()),
            "gateway requires 1..32 backends"
        );
        ensure!(
            (10..=300).contains(&self.request_timeout_seconds),
            "gateway request_timeout_seconds must be 10..300"
        );
        ensure!(
            (1..=64).contains(&self.max_in_flight),
            "gateway max_in_flight must be 1..64"
        );
        ensure!(
            !self.scheduled_jobs || self.request_timeout_seconds >= 150,
            "scheduled_jobs requires request_timeout_seconds >= 150 for bounded execution and commit"
        );
        ensure!(
            self.telegram.len() <= 32,
            "at most 32 Telegram bindings are supported"
        );
        ensure!(
            self.telegram.is_empty() || self.request_timeout_seconds >= 150,
            "Telegram requires request_timeout_seconds >= 150"
        );
        let mut telegram_ids = HashSet::new();
        for entry in &self.telegram {
            ensure!(
                uuid::Uuid::parse_str(&entry.binding_id)
                    .is_ok_and(|id| !id.is_nil() && id.to_string() == entry.binding_id),
                "Telegram binding_id must be a canonical non-nil UUID"
            );
            ensure!(
                telegram_ids.insert(&entry.binding_id),
                "duplicate Telegram binding_id"
            );
            ensure!(
                entry.bot_token_file.is_absolute() && entry.webhook_secret_file.is_absolute(),
                "Telegram secret paths must be absolute"
            );
            crate::outbound::validate_api_base(
                crate::channel_types::Channel::Telegram,
                &entry.api_base,
                entry.allow_loopback,
            )?;
        }
        ensure!(
            self.slack.len() <= 32,
            "at most 32 Slack bindings are supported"
        );
        ensure!(
            self.slack.is_empty() || self.request_timeout_seconds >= 150,
            "Slack requires request_timeout_seconds >= 150"
        );
        let mut slack_ids = HashSet::new();
        for entry in &self.slack {
            ensure!(
                uuid::Uuid::parse_str(&entry.binding_id).is_ok_and(|id| !id.is_nil()
                    && id.get_variant() == uuid::Variant::RFC4122
                    && id.to_string() == entry.binding_id),
                "Slack binding_id must be a canonical non-nil RFC4122 UUID"
            );
            ensure!(
                slack_ids.insert(&entry.binding_id),
                "duplicate Slack binding_id"
            );
            ensure!(
                [&entry.bot_token_file, &entry.signing_secret_file]
                    .into_iter()
                    .all(|path| path.is_absolute()
                        && path.file_name().is_some()
                        && !path
                            .components()
                            .any(|part| matches!(part, Component::ParentDir))),
                "Slack secret paths must be absolute without parent traversal"
            );
            ensure!(
                entry.api_base.len() <= 2048,
                "Slack API base exceeds its size limit"
            );
            crate::outbound::validate_api_base(
                crate::channel_types::Channel::Slack,
                &entry.api_base,
                entry.allow_loopback,
            )?;
        }
        ensure!(
            self.discord.len() <= 32,
            "at most 32 Discord bindings are supported"
        );
        ensure!(
            self.discord.is_empty() || self.request_timeout_seconds >= 150,
            "Discord requires request_timeout_seconds >= 150"
        );
        let mut discord_ids = HashSet::new();
        for entry in &self.discord {
            ensure!(
                uuid::Uuid::parse_str(&entry.binding_id).is_ok_and(|id| !id.is_nil()
                    && id.get_variant() == uuid::Variant::RFC4122
                    && id.to_string() == entry.binding_id),
                "Discord binding_id must be a canonical non-nil RFC4122 UUID"
            );
            ensure!(
                discord_ids.insert(&entry.binding_id),
                "duplicate Discord binding_id"
            );
            ensure!(
                [&entry.bot_token_file, &entry.state_key_file]
                    .into_iter()
                    .all(|path| path.is_absolute()
                        && path.file_name().is_some()
                        && !path
                            .components()
                            .any(|part| matches!(part, Component::ParentDir))),
                "Discord secret paths must be absolute without parent traversal"
            );
            ensure!(
                entry.api_base.len() <= 2048,
                "Discord API base exceeds its size limit"
            );
            crate::outbound::validate_api_base(
                crate::channel_types::Channel::Discord,
                &entry.api_base,
                entry.allow_loopback,
            )?;
        }
        let mut ids = HashSet::new();
        let mut origins = HashSet::new();
        for backend in &self.backends {
            ensure!(
                !backend.id.is_empty()
                    && backend.id.len() <= 64
                    && backend
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
                "backend id must contain 1..64 ASCII letters, digits, underscores or hyphens"
            );
            ensure!(ids.insert(&backend.id), "duplicate gateway backend id");
            ensure!(
                backend.token_file.is_absolute(),
                "backend token_file must be absolute"
            );
            ensure!(
                backend.url.len() <= 2048
                    && !backend.url.contains('\\')
                    && backend.url.trim() == backend.url
                    && !backend.url.chars().any(char::is_control),
                "invalid backend URL"
            );
            let url = reqwest::Url::parse(&backend.url)
                .map_err(|_| anyhow::anyhow!("invalid backend URL"))?;
            let raw_origin = backend.url.split_once("://").map(|(_, rest)| rest);
            let authority = raw_origin.map(|rest| rest.split('/').next().unwrap_or(rest));
            let root_only = raw_origin
                .is_some_and(|rest| rest.split_once('/').is_none_or(|(_, path)| path.is_empty()));
            ensure!(
                matches!(url.scheme(), "https" | "http")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && !authority.is_some_and(|value| value.contains('@'))
                    && root_only
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "backend URL must be an HTTP(S) root origin without userinfo, query or fragment"
            );
            if url.scheme() == "http" && !self.allow_private_http {
                let host = authority.and_then(|value| {
                    if let Some(bracketed) = value.strip_prefix('[') {
                        bracketed.split_once(']').map(|(host, _)| host)
                    } else {
                        Some(value.split(':').next().unwrap_or(value))
                    }
                });
                let loopback = host
                    .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                    .is_some_and(|ip| ip.is_loopback());
                ensure!(loopback, "HTTP backend requires a literal loopback address or allow_private_http on a trusted network");
            }
            ensure!(
                origins.insert(url.origin().ascii_serialization()),
                "duplicate gateway backend origin"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn value() -> serde_json::Value {
        json!({"registry_path":"/tmp/gateway-registry.sqlite3", "backends":[{"id":"alice-1", "url":"http://127.0.0.1:9001", "token_file":"/tmp/backend.token"}]})
    }
    fn config(value: serde_json::Value) -> Config {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn defaults_and_explicit_network_boundaries() {
        let mut c = config(value());
        c.validate().unwrap();
        assert_eq!(c.bind, "127.0.0.1:8081");
        assert_eq!(c.request_timeout_seconds, 180);
        assert_eq!(c.max_in_flight, 16);
        assert!(!c.scheduled_jobs);
        assert!(c.slack.is_empty());
        assert!(c.discord.is_empty());
        c.scheduled_jobs = true;
        c.request_timeout_seconds = 149;
        assert!(c.validate().is_err());
        c.request_timeout_seconds = 150;
        c.validate().unwrap();
        c.request_timeout_seconds = 180;
        for url in [
            "http://[::1]:9001/",
            "http://127.0.0.2:9001",
            "https://backend.example:9443/",
        ] {
            c.backends[0].url = url.into();
            c.validate().unwrap();
        }
        c.backends[0].url = "http://tenant-alice:8080".into();
        assert!(c.validate().is_err());
        c.allow_private_http = true;
        c.validate().unwrap();
        c.bind = "0.0.0.0:8081".into();
        assert!(c.validate().is_err());
        c.allow_remote_bind = true;
        c.validate().unwrap();
    }

    #[test]
    fn rejects_ambiguous_or_credential_bearing_backend_urls() {
        for url in [
            "ftp://example.com/",
            "file:///tmp/a",
            "https://example.com/path",
            "https://example.com/?x=1",
            "https://example.com/#x",
            "https://secret@example.com",
            "https://:secret@example.com",
            "https://@example.com",
            "http://localhost:9001",
            "http://2130706433",
            "https://example.com/../",
            "https://example.com\\",
            " https://example.com",
            "https://example.com\n",
        ] {
            let mut c = config(value());
            c.backends[0].url = url.into();
            let error = c.validate().expect_err(url).to_string();
            assert!(!error.contains("secret"));
        }
        let mut c = config(value());
        c.backends.push(BackendConfig {
            id: "bob".into(),
            url: "http://127.0.0.1:9001/".into(),
            token_file: "/tmp/bob.token".into(),
        });
        assert!(c.validate().is_err());
        c.backends[1].url = "http://127.0.0.1:9002".into();
        c.validate().unwrap();
        c.backends[1].id = "alice-1".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn bounds_ids_paths_counts_and_limits() {
        for id in ["", "alice/bob", " alice", "alice.bob", "用户"] {
            let mut c = config(value());
            c.backends[0].id = id.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(value());
        c.backends[0].id = "x".repeat(65);
        assert!(c.validate().is_err());
        for timeout in [0, 9, 301, u64::MAX] {
            let mut c = config(value());
            c.request_timeout_seconds = timeout;
            assert!(c.validate().is_err());
        }
        for limit in [0, 65, usize::MAX] {
            let mut c = config(value());
            c.max_in_flight = limit;
            assert!(c.validate().is_err());
        }
        let mut c = config(value());
        c.registry_path = "relative.sqlite3".into();
        assert!(c.validate().is_err());
        let mut c = config(value());
        c.backends[0].token_file = "relative.token".into();
        assert!(c.validate().is_err());
        let mut c = config(value());
        c.backends.clear();
        assert!(c.validate().is_err());
        let mut c = config(value());
        c.backends = vec![c.backends[0].clone(); 33];
        assert!(c.validate().is_err());
    }

    #[test]
    fn strict_json_and_bounded_load_do_not_echo_input() {
        let dir =
            std::env::temp_dir().join(format!("jiaclaw-gateway-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("gateway.json");
        for invalid in [
            json!({"unexpected":"sensitive-config-value"}),
            json!({"registry_path":"/tmp/a","backends":[{"id":"a","url":"https://example.com","token_file":"/tmp/t","token":"sensitive-config-value"}]}),
        ] {
            std::fs::write(&path, invalid.to_string()).unwrap();
            let error = Config::load(&path).err().unwrap().to_string();
            assert!(!error.contains("sensitive-config-value"));
        }
        std::fs::write(
            &path,
            vec![b' '; usize::try_from(MAX_CONFIG_BYTES).unwrap() + 1],
        )
        .unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(&path, value().to_string()).unwrap();
        Config::load(&path).unwrap();
        assert!(Config::load(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn telegram_value() -> serde_json::Value {
        let mut input = value();
        input["telegram"] = json!([{"binding_id":"12345678-1234-4234-9234-123456789012","bot_token_file":"/tmp/bot.token","webhook_secret_file":"/tmp/webhook.secret"}]);
        input
    }

    #[test]
    fn telegram_configuration_requires_bounded_canonical_bindings_and_explicit_loopback() {
        let c = config(telegram_value());
        c.validate().unwrap();
        assert_eq!(c.telegram[0].api_base, "https://api.telegram.org");
        assert!(!c.telegram[0].allow_loopback);
        for id in [
            "",
            "00000000-0000-0000-0000-000000000000",
            "ABCDEF01-2345-4678-9ABC-DEF012345678",
            "not-uuid",
        ] {
            let mut c = config(telegram_value());
            c.telegram[0].binding_id = id.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(telegram_value());
        c.request_timeout_seconds = 149;
        assert!(c.validate().is_err());
        c.request_timeout_seconds = 150;
        c.validate().unwrap();
        c.telegram.push(c.telegram[0].clone());
        assert!(c.validate().is_err());
        let mut c = config(telegram_value());
        c.telegram = vec![c.telegram[0].clone(); 33];
        assert!(c.validate().is_err());
        for url in [
            "https://other.example",
            "http://localhost:8080",
            "https://api.telegram.org/path",
            "https://secret@api.telegram.org",
        ] {
            let mut c = config(telegram_value());
            c.telegram[0].api_base = url.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(telegram_value());
        c.telegram[0].api_base = "http://127.0.0.1:8080".into();
        assert!(c.validate().is_err());
        c.telegram[0].allow_loopback = true;
        c.validate().unwrap();
        c.telegram[0].bot_token_file = "relative.token".into();
        assert!(c.validate().is_err());
        let mut c = config(telegram_value());
        c.telegram[0].webhook_secret_file = "relative.secret".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn telegram_configuration_rejects_inline_credentials_and_client_chosen_identity_fields() {
        for (field, value) in [
            ("bot_token", json!("secret")),
            ("sender_id", json!("201")),
            ("backend_id", json!("other")),
            ("user_id", json!("other")),
            ("enabled", json!(true)),
        ] {
            let mut input = telegram_value();
            input["telegram"][0][field] = value;
            assert!(serde_json::from_value::<Config>(input).is_err(), "{field}");
        }
        for value in [json!(null), json!(1), json!("true")] {
            let mut input = telegram_value();
            input["telegram"][0]["allow_loopback"] = value;
            assert!(serde_json::from_value::<Config>(input).is_err());
        }
    }

    fn slack_value() -> serde_json::Value {
        let mut input = value();
        input["slack"] = json!([{"binding_id":"12345678-1234-4234-9234-123456789012","bot_token_file":"/tmp/bot.token","signing_secret_file":"/tmp/signing.secret"}]);
        input
    }

    #[test]
    fn slack_configuration_requires_bounded_canonical_bindings_paths_and_explicit_loopback() {
        let c = config(slack_value());
        c.validate().unwrap();
        assert_eq!(c.slack[0].api_base, "https://slack.com/api");
        assert!(!c.slack[0].allow_loopback);
        for id in [
            "",
            "00000000-0000-0000-0000-000000000000",
            "ABCDEF01-2345-4678-9ABC-DEF012345678",
            "abcdef01-2345-4678-1abc-def012345678",
            "not-uuid",
        ] {
            let mut c = config(slack_value());
            c.slack[0].binding_id = id.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(slack_value());
        c.request_timeout_seconds = 149;
        assert!(c.validate().is_err());
        c.request_timeout_seconds = 150;
        c.validate().unwrap();
        c.slack.push(c.slack[0].clone());
        assert!(c.validate().is_err());
        let mut c = config(slack_value());
        c.slack = (0..32)
            .map(|_| {
                let mut entry = c.slack[0].clone();
                entry.binding_id = uuid::Uuid::new_v4().to_string();
                entry
            })
            .collect();
        c.validate().unwrap();
        c.slack.push(c.slack[0].clone());
        assert!(c.validate().is_err());
        for url in [
            "https://other.example/api",
            "http://localhost:8080/api",
            "https://slack.com/other",
            "https://secret@slack.com/api",
            "https://slack.com/api?secret=token",
            "https://slack.com/api#secret",
            "http://127.0.0.1:8080/other",
        ] {
            let mut c = config(slack_value());
            c.slack[0].api_base = url.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(slack_value());
        c.slack[0].api_base = "http://127.0.0.1:8080/api".into();
        assert!(c.validate().is_err());
        c.slack[0].allow_loopback = true;
        c.validate().unwrap();
        for path in ["relative.secret", "/tmp/../secret", "/"] {
            for signing in [false, true] {
                let mut c = config(slack_value());
                if signing {
                    c.slack[0].signing_secret_file = path.into();
                } else {
                    c.slack[0].bot_token_file = path.into();
                }
                assert!(c.validate().is_err());
            }
        }
    }

    #[test]
    fn slack_configuration_rejects_inline_credentials_and_caller_owned_identity() {
        for (field, value) in [
            ("bot_token", json!("secret")),
            ("signing_secret", json!("secret")),
            ("team_id", json!("T1")),
            ("app_id", json!("A1")),
            ("bot_user_id", json!("U1")),
            ("bot_id", json!("B1")),
            ("sender_id", json!("U2")),
            ("conversation_id", json!("D1")),
            ("backend_id", json!("other")),
            ("user_id", json!("other")),
            ("enabled", json!(true)),
        ] {
            let mut input = slack_value();
            input["slack"][0][field] = value;
            assert!(serde_json::from_value::<Config>(input).is_err(), "{field}");
        }
        for value in [json!(null), json!(1), json!("true")] {
            let mut input = slack_value();
            input["slack"][0]["allow_loopback"] = value;
            assert!(serde_json::from_value::<Config>(input).is_err());
        }
    }
    fn discord_value() -> serde_json::Value {
        let mut input = value();
        input["discord"] = json!([{"binding_id":"12345678-1234-4234-9234-123456789012","bot_token_file":"/tmp/bot.token","state_key_file":"/tmp/signing.secret"}]);
        input
    }

    #[test]
    fn discord_configuration_requires_bounded_canonical_bindings_paths_and_explicit_loopback() {
        let c = config(discord_value());
        c.validate().unwrap();
        assert_eq!(c.discord[0].api_base, "https://discord.com/api/v10");
        assert!(!c.discord[0].allow_loopback);
        for id in [
            "",
            "00000000-0000-0000-0000-000000000000",
            "ABCDEF01-2345-4678-9ABC-DEF012345678",
            "abcdef01-2345-4678-1abc-def012345678",
            "not-uuid",
        ] {
            let mut c = config(discord_value());
            c.discord[0].binding_id = id.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(discord_value());
        c.request_timeout_seconds = 149;
        assert!(c.validate().is_err());
        c.request_timeout_seconds = 150;
        c.validate().unwrap();
        c.discord.push(c.discord[0].clone());
        assert!(c.validate().is_err());
        let mut c = config(discord_value());
        c.discord = (0..32)
            .map(|_| {
                let mut entry = c.discord[0].clone();
                entry.binding_id = uuid::Uuid::new_v4().to_string();
                entry
            })
            .collect();
        c.validate().unwrap();
        c.discord.push(c.discord[0].clone());
        assert!(c.validate().is_err());
        for url in [
            "https://other.example/api",
            "http://localhost:8080/api/v10",
            "https://discord.com/other",
            "https://secret@discord.com/api",
            "https://discord.com/api/v10?secret=token",
            "https://discord.com/api/v10#secret",
            "http://127.0.0.1:8080/other",
        ] {
            let mut c = config(discord_value());
            c.discord[0].api_base = url.into();
            assert!(c.validate().is_err());
        }
        let mut c = config(discord_value());
        c.discord[0].api_base = "http://127.0.0.1:8080/api/v10".into();
        assert!(c.validate().is_err());
        c.discord[0].allow_loopback = true;
        c.validate().unwrap();
        for path in ["relative.secret", "/tmp/../secret", "/"] {
            for signing in [false, true] {
                let mut c = config(discord_value());
                if signing {
                    c.discord[0].state_key_file = path.into();
                } else {
                    c.discord[0].bot_token_file = path.into();
                }
                assert!(c.validate().is_err());
            }
        }
    }

    #[test]
    fn discord_configuration_rejects_inline_credentials_and_caller_owned_identity() {
        for (field, value) in [
            ("bot_token", json!("secret")),
            ("state_key", json!("secret")),
            ("application_id", json!("1")),
            ("verify_key", json!("public-pin")),
            ("bot_user_id", json!("U1")),
            ("command_id", json!("13")),
            ("sender_id", json!("U2")),
            ("conversation_id", json!("D1")),
            ("backend_id", json!("other")),
            ("user_id", json!("other")),
            ("enabled", json!(true)),
        ] {
            let mut input = discord_value();
            input["discord"][0][field] = value;
            assert!(serde_json::from_value::<Config>(input).is_err(), "{field}");
        }
        for value in [json!(null), json!(1), json!("true")] {
            let mut input = discord_value();
            input["discord"][0]["allow_loopback"] = value;
            assert!(serde_json::from_value::<Config>(input).is_err());
        }
    }
}
