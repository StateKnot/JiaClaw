// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `JiaClaw` 可执行宿主

use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json, Response,
    },
    routing::{get, post},
    Router,
};
use clap::{Parser, Subcommand};
use futures_util::{stream, Stream};
use governor::{
    clock::{Clock, DefaultClock},
    middleware::StateInformationMiddleware,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use hmac::{Hmac, Mac};
use jiaclaw::{
    compact_imported_session_messages, inspect_heartbeat_file, inspect_identity_file,
    inspect_memory_file, load_heartbeat_message, resolve_heartbeat_path, JiaClawAgent, Workspace,
};
use jiaclaw_core::{
    parse_log_format, AgentConfig, ChatMessage, ChatRequest, ChatResponse, HttpCorsConfig,
    LogFormat, LoggingConfig, MessageRole, ToolCall, DEFAULT_LOG_LEVEL, MAX_MAX_TOOL_ITERATIONS,
    MAX_SESSION_MESSAGES, MIN_MAX_TOOL_ITERATIONS,
};

#[cfg(test)]
use jiaclaw_core::DEFAULT_HTTP_MAX_BODY_BYTES;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;
use tower_http::cors::{AllowOrigin, CorsLayer};

mod channel_store;
mod channel_types;
mod channels;
mod jobs;
mod outbound;
mod schedule;
mod scheduler;
mod store;
mod ui;
use store::SessionStore;

mod metrics;
use metrics::{classify_http_path, Metrics, PROMETHEUS_CONTENT_TYPE};

/// 进程内全局（非按 IP）速率限制器，oneshot 测试无需 `ConnectInfo`。
type GlobalRateLimiter =
    RateLimiter<NotKeyed, InMemoryState, DefaultClock, StateInformationMiddleware>;

/// 请求追踪头。大小写不敏感，响应回写同名头。
const X_REQUEST_ID: &str = "x-request-id";

/// 限流配额上限（每分钟 burst）。仅在限流启用且路径受保护时发送。
const X_RATELIMIT_LIMIT: &str = "x-ratelimit-limit";

/// 本次判定后剩余可立即通过的请求数。仅在限流启用且路径受保护时发送。
const X_RATELIMIT_REMAINING: &str = "x-ratelimit-remaining";

/// 配额补满到 Limit 的 Unix 纪元秒（UTC）。仅在限流启用且路径受保护时发送。
const X_RATELIMIT_RESET: &str = "x-ratelimit-reset";

/// 会话 JSONL 导出的 `Content-Type`（NDJSON）。
const SESSION_EXPORT_NDJSON: &str = "application/x-ndjson";

/// Platform webhook timestamp freshness window.
const SLACK_MAX_TIMESTAMP_SKEW_SECS: u64 = 300;

/// HMAC-SHA256 用于 Slack v0 签名。
type HmacSha256 = Hmac<Sha256>;

/// 手写 `OpenAPI` 3 草图（不引入代码生成）。
const OPENAPI_JSON: &str = include_str!("openapi.json");

#[derive(Parser)]
#[command(name = "jiaclaw")]
#[command(about = "JiaClaw - 基于 StateKnot 的个人持久化智能体运行时", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 初始化工作空间和配置
    Init {
        /// 工作空间路径
        #[arg(short, long, value_name = "PATH")]
        path: Option<PathBuf>,

        /// 强制覆盖已存在的文件
        #[arg(short, long)]
        force: bool,
    },

    /// 启动 `JiaClaw` Agent 服务
    Serve {
        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,

        /// 绑定地址（覆盖配置文件）
        #[arg(short, long)]
        bind: Option<String>,
    },

    /// 运行聊天（支持单次消息或交互式 REPL）
    Chat {
        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,

        /// 用户消息（如果提供，则执行单次聊天；否则进入 REPL）
        #[arg(value_name = "MESSAGE")]
        message: Option<String>,

        /// 启用的技能列表（可重复使用）
        #[arg(short, long = "skill", value_name = "NAME")]
        skills: Vec<String>,

        /// 会话 ID（可选，用于恢复历史对话）
        #[arg(long, value_name = "ID")]
        session: Option<String>,

        /// 禁用技能自动激活
        #[arg(long)]
        no_auto_skill: bool,
    },

    /// 显示版本和构建信息
    Version,

    /// Inspect MCP descriptors for review; does not approve or call tools
    McpInspect {
        /// Fixed HTTPS or literal-loopback HTTP endpoint
        #[arg(long)]
        endpoint: String,
        /// Environment variable containing the bearer token
        #[arg(long)]
        bearer_token_env: Option<String>,
    },

    /// 检查配置和连接状态
    Doctor {
        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,
    },

    /// 列出或重载技能
    Skills {
        #[command(subcommand)]
        action: Option<SkillsCommands>,

        /// 配置文件路径
        #[arg(short, long, value_name = "FILE", global = true)]
        config: Option<PathBuf>,

        /// 显示详细信息
        #[arg(short, long)]
        verbose: bool,
    },

    /// 工作区长期记忆（MEMORY.md）
    Memory {
        #[command(subcommand)]
        action: MemoryCommands,
    },

    /// Agent 人格（SOUL.md）
    Soul {
        #[command(subcommand)]
        action: IdentityFileCommands,
    },

    /// 用户画像（USER.md）
    User {
        #[command(subcommand)]
        action: IdentityFileCommands,
    },

    /// 会话（导出 JSONL；导入 JSONL/JSON，不调用 LLM）
    Session {
        #[command(subcommand)]
        action: SessionCommands,
    },
}

/// 会话子命令
#[derive(Subcommand)]
enum SessionCommands {
    /// 导出会话历史为 JSONL（默认 stdout）
    Export {
        /// 会话 ID
        id: String,

        /// 输出文件（省略则写 stdout）
        #[arg(short = 'o', long = "output", value_name = "FILE")]
        output: Option<PathBuf>,

        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,
    },

    /// 从 JSONL 或 `{id?, messages}` JSON 文件导入会话（不调用 LLM）
    Import {
        /// 输入文件
        file: PathBuf,

        /// 会话 ID（省略则用文件内 id 或新建 UUID）
        #[arg(long, value_name = "ID")]
        id: Option<String>,

        /// 若会话已存在则替换消息
        #[arg(long)]
        overwrite: bool,

        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
}

/// 技能子命令
#[derive(Subcommand)]
enum SkillsCommands {
    /// 重新扫描工作区 `skills/` 并打印结果（不通知已运行的 serve）
    Reload,
}

/// 长期记忆子命令
#[derive(Subcommand)]
enum MemoryCommands {
    /// 显示约定 MEMORY 文件内容
    Show {
        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
}

/// 人格 / 用户画像子命令
#[derive(Subcommand)]
enum IdentityFileCommands {
    /// 显示约定文件内容
    Show {
        /// 配置文件路径
        #[arg(short, long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // serve 在加载配置后再初始化 subscriber，以便 `[logging] format=json` 生效。
    // 其它命令只读环境变量 / 默认 text，行为与原先 `fmt()` + EnvFilter 一致。
    if !matches!(cli.command, Commands::Serve { .. }) {
        init_tracing_from(&LoggingConfig::default());
    }

    match cli.command {
        Commands::Init { path, force } => {
            init_command(path, force)?;
        }
        Commands::Serve { config, bind } => {
            serve_command(config, bind).await?;
        }
        Commands::Chat {
            config,
            message,
            skills,
            session,
            no_auto_skill,
        } => {
            chat_command(config, message.as_deref(), skills, session, no_auto_skill).await?;
        }
        Commands::Version => {
            version_command();
        }
        Commands::McpInspect {
            endpoint,
            bearer_token_env,
        } => {
            let review = jiaclaw::inspect_mcp_server(&endpoint, bearer_token_env).await?;
            println!("{}", serde_json::to_string_pretty(&review)?);
        }
        Commands::Doctor { config } => {
            doctor_command(config).await?;
        }
        Commands::Skills {
            action,
            config,
            verbose,
        } => match action {
            Some(SkillsCommands::Reload) => {
                skills_reload_command(config)?;
            }
            None => {
                skills_command(config, verbose)?;
            }
        },
        Commands::Memory { action } => match action {
            MemoryCommands::Show { config } => {
                memory_show_command(config)?;
            }
        },
        Commands::Soul { action } => match action {
            IdentityFileCommands::Show { config } => {
                identity_show_command(config, IdentityShowKind::Soul)?;
            }
        },
        Commands::User { action } => match action {
            IdentityFileCommands::Show { config } => {
                identity_show_command(config, IdentityShowKind::User)?;
            }
        },
        Commands::Session { action } => match action {
            SessionCommands::Export { id, output, config } => {
                session_export_command(config, &id, output.as_deref())?;
            }
            SessionCommands::Import {
                file,
                id,
                overwrite,
                config,
            } => {
                session_import_command(config, &file, id.as_deref(), overwrite).await?;
            }
        },
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn init_command(path: Option<PathBuf>, force: bool) -> Result<()> {
    let workspace_path = path.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".jiaclaw")
            .join("workspace")
    });

    tracing::info!("初始化工作空间: {}", workspace_path.display());

    // 检查是否已存在
    if workspace_path.exists() && !force {
        tracing::warn!("工作空间已存在。使用 --force 强制覆盖。");
        println!("\n❌ 工作空间已存在: {}", workspace_path.display());
        println!("   使用 --force 标志强制覆盖现有文件。");
        return Ok(());
    }

    // 初始化工作空间
    Workspace::init(&workspace_path).context("初始化工作空间失败")?;

    println!("\n✅ 工作空间已初始化: {}", workspace_path.display());
    println!("\n📁 已创建文件:");
    println!("   • AGENTS.md  - Agent 配置和元数据");
    println!("   • SOUL.md    - Agent 性格和指令");
    println!("   • USER.md    - 用户信息和偏好");
    println!("   • MEMORY.md  - 长期记忆和上下文");
    println!("   • skills/    - 技能目录");
    println!("     ├── search/SKILL.md");
    println!("     └── calculator/SKILL.md");

    println!("\n📝 下一步:");
    println!("   1. 编辑工作空间文件以个性化你的 Agent");
    println!("   2. 配置 Brokerrouter 虚拟 API key:");
    println!("      export JIACLAW_API_KEY=your-key-here");
    println!("   3. 开始聊天:");
    println!("      jiaclaw chat \"你好\"");

    println!("\n💡 提示:");
    println!("   • 离线验收需显式配置 provider_type = stub");
    println!("   • 可选 HEARTBEAT.md：仅 serve 进程可按间隔自检（默认关闭，见 [heartbeat]）");
    println!("   • 参见 config/jiaclaw.toml.example 了解完整配置选项");

    Ok(())
}

/// 会话闲置 TTL 后台扫描间隔。
const SESSION_TTL_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// 内存中的一条会话：消息 + 最近触达时间。
#[derive(Clone)]
struct SessionRecord {
    messages: Vec<ChatMessage>,
    last_accessed: Instant,
}

impl SessionRecord {
    fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            last_accessed: Instant::now(),
        }
    }

    fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }

    fn is_expired(&self, ttl: Duration, now: Instant) -> bool {
        now.saturating_duration_since(self.last_accessed) >= ttl
    }
}

/// HTTP 服务的共享状态
#[derive(Clone)]
struct AppState {
    channel_runtime: Option<Arc<channels::ChannelRuntime>>,
    agent: Arc<JiaClawAgent>,
    sessions: Arc<Mutex<SessionStore>>,
    turn_locks: Arc<Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>>,
    api_token: Option<String>,
    webhook_secret: Option<String>,
    persist_enabled: bool,
    persist_path: Arc<PathBuf>,
    rate_limiter: Option<Arc<GlobalRateLimiter>>,
    session_ttl: Option<Duration>,
    metrics: Arc<Metrics>,
    metrics_require_auth: bool,
    scheduler_health: Arc<std::sync::atomic::AtomicU8>,
}

/// 健康检查响应
#[derive(Debug, Serialize, Deserialize)]
struct HealthResponse {
    status: String,
    agent_name: String,
    version: String,
}

/// Webhook 入站请求
#[derive(Debug, Serialize, Deserialize)]
struct InboundWebhookRequest {
    #[serde(default = "default_channel")]
    channel: String,
    chat_id: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
}

fn default_channel() -> String {
    "webhook".to_string()
}

fn telegram_token_config_source() -> &'static str {
    match std::env::var("JIACLAW_TELEGRAM_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => "环境变量 JIACLAW_TELEGRAM_BOT_TOKEN",
        _ => "配置文件",
    }
}

fn slack_signing_secret_config_source() -> &'static str {
    match std::env::var("JIACLAW_SLACK_SIGNING_SECRET") {
        Ok(value) if !value.trim().is_empty() => "环境变量 JIACLAW_SLACK_SIGNING_SECRET",
        _ => "配置文件",
    }
}

fn slack_token_config_source() -> &'static str {
    match std::env::var("JIACLAW_SLACK_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => "环境变量 JIACLAW_SLACK_BOT_TOKEN",
        _ => "配置文件",
    }
}

fn discord_public_key_config_source() -> &'static str {
    match std::env::var("JIACLAW_DISCORD_PUBLIC_KEY") {
        Ok(value) if !value.trim().is_empty() => "环境变量 JIACLAW_DISCORD_PUBLIC_KEY",
        _ => "配置文件",
    }
}

fn discord_token_config_source() -> &'static str {
    match std::env::var("JIACLAW_DISCORD_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => "环境变量 JIACLAW_DISCORD_BOT_TOKEN",
        _ => "配置文件",
    }
}

fn verify_discord_signature(
    public_key_hex: &str,
    timestamp: &str,
    body: &[u8],
    signature_hex: &str,
) -> bool {
    let Some(public_key) = decode_hex(public_key_hex) else {
        return false;
    };
    if public_key.len() != 32 {
        return false;
    }
    let Some(signature) = decode_hex(signature_hex) else {
        return false;
    };
    if signature.len() != 64 {
        return false;
    }
    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(&message, &signature)
        .is_ok()
}

fn verify_discord_request(
    public_key_hex: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
) -> bool {
    let Some(timestamp) = timestamp.filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(signature) = signature.filter(|s| !s.is_empty()) else {
        return false;
    };
    verify_discord_signature(public_key_hex, timestamp, body, signature)
}

#[cfg(test)]
fn encode_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(HEX[(byte >> 4) as usize]));
        out.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    out
}

fn from_hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let hi = from_hex_digit(chunk[0])?;
        let lo = from_hex_digit(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn slack_timestamp_fresh(timestamp: &str, now_secs: u64) -> bool {
    timestamp
        .parse::<u64>()
        .is_ok_and(|ts| now_secs.abs_diff(ts) <= SLACK_MAX_TIMESTAMP_SKEW_SECS)
}

/// 计算 Slack 官方 `v0=` HMAC-SHA256 签名（用于测试与对照已知向量）。
#[cfg(test)]
fn slack_v0_signature(secret: &str, timestamp: &str, body: &[u8]) -> Option<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    Some(format!(
        "v0={}",
        encode_hex_lower(&mac.finalize().into_bytes())
    ))
}

fn verify_slack_v0_signature(secret: &str, timestamp: &str, body: &[u8], signature: &str) -> bool {
    let Some(hex_sig) = signature.strip_prefix("v0=") else {
        return false;
    };
    let Some(sig_bytes) = decode_hex(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}

fn verify_slack_request(
    secret: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now_secs: u64,
) -> bool {
    let Some(timestamp) = timestamp.filter(|s| !s.is_empty()) else {
        return false;
    };
    if !slack_timestamp_fresh(timestamp, now_secs) {
        return false;
    }
    let Some(signature) = signature.filter(|s| !s.is_empty()) else {
        return false;
    };
    verify_slack_v0_signature(secret, timestamp, body, signature)
}

fn build_rate_limiter(per_minute: u32) -> Option<Arc<GlobalRateLimiter>> {
    NonZeroU32::new(per_minute).map(|nz| {
        Arc::new(
            RateLimiter::direct(Quota::per_minute(nz))
                .with_middleware::<StateInformationMiddleware>(),
        )
    })
}

fn is_rate_limited_path(path: &str) -> bool {
    path.starts_with("/api/")
        || path == "/hooks/inbound"
        || path == "/hooks/telegram"
        || path == "/hooks/slack"
        || path == "/hooks/discord"
}

/// 限流响应头快照。`retry_after_secs` 仅 429 设置。
#[derive(Clone, Copy)]
struct RateLimitInfo {
    limit: u32,
    remaining: u32,
    reset_unix_secs: u64,
    retry_after_secs: Option<u64>,
}

/// `X-RateLimit-Reset`：Unix 纪元秒（UTC），剩余配额补满到 `Limit` 的时刻。
///
/// 按每分钟 burst 的 replenish 间隔向上取整到秒；429 时 `remaining = 0`，因此约为当前窗口结束。
fn rate_limit_reset_unix(now: u64, remaining: u32, limit: u32, replenish: Duration) -> u64 {
    let missing = limit.saturating_sub(remaining);
    let wait = replenish.saturating_mul(missing);
    let ceil_secs = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0));
    now.saturating_add(ceil_secs)
}

fn insert_rate_limit_headers(headers: &mut HeaderMap, info: &RateLimitInfo) {
    if let Ok(value) = HeaderValue::from_str(&info.limit.to_string()) {
        headers.insert(X_RATELIMIT_LIMIT, value);
    }
    if let Ok(value) = HeaderValue::from_str(&info.remaining.to_string()) {
        headers.insert(X_RATELIMIT_REMAINING, value);
    }
    if let Ok(value) = HeaderValue::from_str(&info.reset_unix_secs.to_string()) {
        headers.insert(X_RATELIMIT_RESET, value);
    }
    if let Some(retry_after_secs) = info.retry_after_secs {
        if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
            headers.insert(header::RETRY_AFTER, value);
        }
    }
}

fn rate_limit_info_from_quota(
    now: u64,
    remaining: u32,
    quota: Quota,
    retry_after_secs: Option<u64>,
) -> RateLimitInfo {
    let limit = quota.burst_size().get();
    RateLimitInfo {
        limit,
        remaining,
        reset_unix_secs: rate_limit_reset_unix(now, remaining, limit, quota.replenish_interval()),
        retry_after_secs,
    }
}

/// 全局限流中间件：返回 `Response`，不依赖 `ConnectInfo`，也不使用 `Err(StatusCode)`。
///
/// 限流启用时，受保护路径附加 `X-RateLimit-*`；429 另带 `Retry-After`。关闭时不发送这些头。
async fn rate_limit_middleware(
    State(limiter): State<Option<Arc<GlobalRateLimiter>>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if !is_rate_limited_path(&path) {
        return next.run(request).await;
    }

    let Some(limiter) = limiter.as_ref() else {
        return next.run(request).await;
    };

    let now = unix_now_secs();
    match limiter.check() {
        Ok(snapshot) => {
            let info = rate_limit_info_from_quota(
                now,
                snapshot.remaining_burst_capacity(),
                snapshot.quota(),
                None,
            );
            let mut response = next.run(request).await;
            insert_rate_limit_headers(response.headers_mut(), &info);
            response
        }
        Err(not_until) => {
            let wait = not_until.wait_time_from(DefaultClock::default().now());
            let retry_after_secs = wait.as_secs().max(1);
            tracing::warn!(path, retry_after_secs, "HTTP 请求超过速率限制");
            rate_limited_response(rate_limit_info_from_quota(
                now,
                0,
                not_until.quota(),
                Some(retry_after_secs),
            ))
        }
    }
}

/// HTTP 请求计数：在 handler / 限流 429 之后按路由族累加。
async fn metrics_middleware(
    State(metrics): State<Arc<Metrics>>,
    request: Request,
    next: Next,
) -> Response {
    let family = classify_http_path(request.uri().path());
    let method = request.method().clone();
    let response = next.run(request).await;
    metrics.record_http(family, method.as_str(), response.status().as_u16());
    response
}

fn rate_limited_response(info: RateLimitInfo) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error": "rate_limit_exceeded"})),
    )
        .into_response();
    insert_rate_limit_headers(response.headers_mut(), &info);
    response
}

/// `GET /health` 与 `GET /metrics` 通常无 body，不检查请求体上限。
fn is_body_limit_exempt_path(path: &str) -> bool {
    path == "/health" || path == "/metrics"
}

fn request_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

fn payload_too_large_response() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(json!({"error": "payload_too_large"})),
    )
        .into_response()
}

fn max_body_bytes_usize(limit: u64) -> usize {
    usize::try_from(limit).unwrap_or(usize::MAX)
}

/// 超限时在读取 body 之前返回 413 JSON，避免把超大请求缓冲进内存。
async fn max_body_bytes_middleware(
    State(limit): State<usize>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if is_body_limit_exempt_path(&path) {
        return next.run(request).await;
    }

    if let Some(content_len) = request_content_length(request.headers()) {
        if content_len > u64::try_from(limit).unwrap_or(u64::MAX) {
            tracing::warn!(path, limit, "HTTP 请求体超过上限");
            return payload_too_large_response();
        }
    }

    next.run(request).await
}

/// 将 axum `DefaultBodyLimit` 产生的空 413 转成与限流一致的 JSON 错误体。
async fn jsonify_payload_too_large(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if is_json {
        response
    } else {
        payload_too_large_response()
    }
}

/// 从请求头读取 `X-Request-Id`；缺失或为空则生成 UUID。
fn resolve_request_id(headers: &HeaderMap) -> String {
    headers
        .get(X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), ToOwned::to_owned)
}

/// 供 tracing 日志使用的 `request_id`；中间件保证响应侧必有该头。
fn request_id_log_value(headers: &HeaderMap) -> &str {
    headers
        .get(X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("-")
}

/// 请求追踪中间件：缺省生成 UUID，响应回写 `X-Request-Id`（含 `/health` 与 429）。
async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let request_id = resolve_request_id(request.headers());
    let Ok(header_value) = HeaderValue::from_str(&request_id) else {
        let mut response = next.run(request).await;
        if let Ok(generated) = HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()) {
            response.headers_mut().insert(X_REQUEST_ID, generated);
        }
        return response;
    };

    request
        .headers_mut()
        .insert(X_REQUEST_ID, header_value.clone());
    let mut response = next.run(request).await;
    response.headers_mut().insert(X_REQUEST_ID, header_value);
    response
}

#[cfg(test)]
fn build_router(state: AppState) -> Router {
    build_router_with_cors(state, None)
}

#[cfg(test)]
fn build_router_with_cors(state: AppState, cors: Option<CorsLayer>) -> Router {
    build_router_with_body_limit(state, cors, DEFAULT_HTTP_MAX_BODY_BYTES)
}

fn build_router_with_body_limit(
    state: AppState,
    cors: Option<CorsLayer>,
    max_body_bytes: u64,
) -> Router {
    let limiter = state.rate_limiter.clone();
    let metrics = state.metrics.clone();
    let body_limit = max_body_bytes_usize(max_body_bytes);
    let mut router = Router::new()
        .route("/", get(ui::index))
        .route("/ui/app.js", get(ui::javascript))
        .route("/ui/app.css", get(ui::stylesheet))
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .route("/api/chat", post(chat_handler))
        .route("/api/channels/status", get(channels::status))
        .route("/api/channels/events", get(channels::events))
        .route(
            "/api/channels/events/:id",
            get(channels::event).delete(channels::purge),
        )
        .route("/api/channels/events/:id/cancel", post(channels::cancel))
        .route("/api/channels/deliveries", get(channels::deliveries))
        .route(
            "/api/channels/deliveries/:id/resolve",
            post(channels::resolve),
        )
        .route("/api/jobs", get(scheduler::list).post(scheduler::create))
        .route("/api/jobs/status", get(scheduler::status))
        .route(
            "/api/jobs/:id",
            get(scheduler::get).delete(scheduler::delete),
        )
        .route("/api/jobs/:id/runs", get(scheduler::runs))
        .route(
            "/api/jobs/:id/runs/:run_id/deliveries",
            get(channels::job_deliveries).delete(channels::purge_job_delivery),
        )
        .route(
            "/api/jobs/:id/runs/:run_id/deliveries/cancel",
            post(channels::cancel_job_delivery),
        )
        .route("/api/jobs/:id/pause", post(scheduler::pause))
        .route("/api/jobs/:id/resume", post(scheduler::resume))
        .route(
            "/api/sessions",
            get(list_sessions_handler).post(create_session_handler),
        )
        .route("/api/sessions/import", post(import_session_handler))
        .route(
            "/api/sessions/:id",
            get(get_session_handler).delete(delete_session_handler),
        )
        .route("/api/sessions/:id/export", get(export_session_handler))
        .route("/api/tools", get(tools_handler))
        .route("/api/skills", get(skills_handler))
        .route("/api/skills/reload", post(reload_skills_handler))
        .route("/api/openapi.json", get(openapi_handler))
        .route("/hooks/inbound", post(hooks_inbound_handler))
        .route("/hooks/telegram", post(channels::telegram))
        .route("/hooks/slack", post(channels::slack))
        .route("/hooks/discord", post(channels::discord))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(middleware::from_fn_with_state(
            body_limit,
            max_body_bytes_middleware,
        ))
        .layer(middleware::from_fn(jsonify_payload_too_large))
        .layer(middleware::from_fn_with_state(
            limiter,
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(metrics, metrics_middleware));
    if let Some(layer) = cors {
        router = router.layer(layer);
    }
    router
        // 外层：即使限流 429 / CORS preflight / 413 也回写 X-Request-Id
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
}

/// 由已 `resolved()` 的 CORS 配置构建中间件。`enabled = false` 时不挂层、不发送 CORS 头。
fn build_cors_layer(cors: &HttpCorsConfig) -> Option<CorsLayer> {
    if !cors.enabled {
        return None;
    }

    let methods = parse_cors_methods(&cors.allowed_methods);
    let allow_headers = parse_cors_header_names(&cors.allowed_headers);
    let expose_headers = parse_cors_header_names(&cors.expose_headers);
    let origins = cors.allowed_origins.clone();
    let allow_any = origins.iter().any(|origin| origin == "*");
    let allow_origin = if allow_any {
        AllowOrigin::any()
    } else {
        AllowOrigin::predicate(move |origin, _parts| {
            origin
                .to_str()
                .ok()
                .is_some_and(|candidate| origins.iter().any(|item| item == candidate))
        })
    };

    let mut layer = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(methods)
        .allow_headers(allow_headers)
        .expose_headers(expose_headers);
    if let Some(secs) = cors.max_age_secs.filter(|&value| value > 0) {
        layer = layer.max_age(Duration::from_secs(secs));
    }
    Some(layer)
}

fn parse_cors_methods(values: &[String]) -> Vec<Method> {
    let parsed: Vec<Method> = values.iter().filter_map(|item| item.parse().ok()).collect();
    if parsed.is_empty() {
        vec![Method::GET, Method::POST, Method::DELETE, Method::OPTIONS]
    } else {
        parsed
    }
}

fn parse_cors_header_names(values: &[String]) -> Vec<HeaderName> {
    let parsed: Vec<HeaderName> = values.iter().filter_map(|item| item.parse().ok()).collect();
    if parsed.is_empty() {
        vec![
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-request-id"),
            header::ACCEPT,
        ]
    } else {
        parsed
    }
}

fn cors_status_line(cors: &HttpCorsConfig) -> String {
    let resolved = cors.resolved();
    if !resolved.enabled {
        "未启用（默认无 CORS 头）".to_string()
    } else if resolved.allows_any_origin() {
        "已启用（允许所有来源 *）".to_string()
    } else if resolved.allowed_origins.is_empty() {
        "已启用（未配置 allowed_origins，不回声任何 Origin）".to_string()
    } else {
        format!(
            "已启用（允许来源: {}）",
            resolved.allowed_origins.join(", ")
        )
    }
}

fn cors_enabled_config_source() -> &'static str {
    if std::env::var("JIACLAW_CORS_ENABLED").is_ok() {
        "环境变量 JIACLAW_CORS_ENABLED"
    } else {
        "配置文件"
    }
}

fn cors_origins_config_source() -> &'static str {
    if std::env::var("JIACLAW_CORS_ORIGINS").is_ok() {
        "环境变量 JIACLAW_CORS_ORIGINS"
    } else {
        "配置文件"
    }
}

fn rate_limit_config_source() -> &'static str {
    if std::env::var("JIACLAW_RATE_LIMIT_PER_MINUTE").is_ok() {
        "环境变量 JIACLAW_RATE_LIMIT_PER_MINUTE"
    } else {
        "配置文件"
    }
}

fn max_body_bytes_config_source() -> &'static str {
    if std::env::var("JIACLAW_MAX_BODY_BYTES").is_ok() {
        "环境变量 JIACLAW_MAX_BODY_BYTES"
    } else {
        "配置文件"
    }
}

fn log_format_config_source() -> &'static str {
    match std::env::var("JIACLAW_LOG_FORMAT") {
        Ok(raw) if parse_log_format(&raw).is_some() => "环境变量 JIACLAW_LOG_FORMAT",
        _ => "配置文件",
    }
}

fn log_level_config_source() -> &'static str {
    match std::env::var("JIACLAW_LOG_LEVEL") {
        Ok(raw) if !raw.trim().is_empty() => "环境变量 JIACLAW_LOG_LEVEL",
        _ => match std::env::var("RUST_LOG") {
            Ok(raw) if !raw.trim().is_empty() => "环境变量 RUST_LOG",
            _ => "配置文件",
        },
    }
}

fn logging_status_line(logging: &LoggingConfig) -> String {
    format!(
        "{}（级别 {}，格式来自 {}，级别来自 {}）",
        logging.effective_format(),
        logging.effective_level_directive(),
        log_format_config_source(),
        log_level_config_source()
    )
}

/// 与 `init_tracing_from` 使用同一套 layer 选择，便于单测断言而不启动 HTTP。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TracingFmtKind {
    Text,
    Json,
}

fn tracing_fmt_kind(format: LogFormat) -> TracingFmtKind {
    match format {
        LogFormat::Json => TracingFmtKind::Json,
        LogFormat::Text => TracingFmtKind::Text,
    }
}

fn env_filter_from_logging(logging: &LoggingConfig) -> tracing_subscriber::EnvFilter {
    let directive = logging.effective_level_directive();
    tracing_subscriber::EnvFilter::try_new(&directive)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_LEVEL))
}

fn init_tracing_from(logging: &LoggingConfig) {
    let filter = env_filter_from_logging(logging);
    match tracing_fmt_kind(logging.effective_format()) {
        TracingFmtKind::Text => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
        TracingFmtKind::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
    }
}

fn metrics_auth_config_source() -> &'static str {
    if std::env::var("JIACLAW_METRICS_REQUIRE_AUTH").is_ok() {
        "环境变量 JIACLAW_METRICS_REQUIRE_AUTH"
    } else {
        "配置文件"
    }
}

fn session_ttl_config_source() -> &'static str {
    if std::env::var("JIACLAW_SESSION_TTL_SECS").is_ok() {
        "环境变量 JIACLAW_SESSION_TTL_SECS"
    } else {
        "配置文件"
    }
}

fn shutdown_timeout_config_source() -> &'static str {
    if std::env::var("JIACLAW_SHUTDOWN_TIMEOUT_SECS").is_ok() {
        "环境变量 JIACLAW_SHUTDOWN_TIMEOUT_SECS"
    } else {
        "配置文件"
    }
}

fn tool_timeout_config_source() -> &'static str {
    if std::env::var("JIACLAW_TOOL_TIMEOUT_SECS").is_ok() {
        "环境变量 JIACLAW_TOOL_TIMEOUT_SECS"
    } else {
        "配置文件"
    }
}

fn max_tool_iterations_config_source() -> &'static str {
    if std::env::var("JIACLAW_MAX_TOOL_ITERATIONS").is_ok() {
        "环境变量 JIACLAW_MAX_TOOL_ITERATIONS"
    } else {
        "配置文件"
    }
}

fn max_tool_iterations_status_line(config: &AgentConfig) -> String {
    format!(
        "{}（通过 {}）",
        config.effective_max_tool_iterations(),
        max_tool_iterations_config_source()
    )
}

fn heartbeat_interval_config_source() -> &'static str {
    if std::env::var("JIACLAW_HEARTBEAT_INTERVAL_SECS").is_ok() {
        "环境变量 JIACLAW_HEARTBEAT_INTERVAL_SECS"
    } else {
        "配置文件"
    }
}

fn session_summarize_config_source() -> &'static str {
    if std::env::var("JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW").is_ok() {
        "环境变量 JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW"
    } else {
        "配置文件"
    }
}

fn brave_api_key_config_source() -> &'static str {
    match std::env::var("JIACLAW_BRAVE_API_KEY") {
        Ok(value) if !value.trim().is_empty() => "环境变量 JIACLAW_BRAVE_API_KEY",
        _ => "配置文件",
    }
}

fn web_search_status_lines(config: &AgentConfig) -> Vec<String> {
    if !config.tools.web_search.enabled {
        return vec!["已关闭（[tools.web_search] enabled = false，未注册）".to_string()];
    }
    if config.tools.web_search.effective_brave_api_key().is_some() {
        vec![format!(
            "已启用（Brave API key 已配置，通过 {}，明文不打印）",
            brave_api_key_config_source()
        )]
    } else {
        vec![
            "已启用但未配置 Brave API key（调用将返回友好错误）".to_string(),
            "💡 设置环境变量: export JIACLAW_BRAVE_API_KEY=your-key".to_string(),
            "💡 或配置 [tools.web_search] brave_api_key（不要把 key 提交到仓库）".to_string(),
        ]
    }
}

fn web_fetch_status_line(config: &AgentConfig) -> String {
    if !config.tools.web_fetch.enabled {
        return "已关闭（[tools.web_fetch] enabled = false，未注册）".to_string();
    }
    if config.tools.web_fetch.allow_private {
        "已启用（allow_private = true，允许 localhost/私网）".to_string()
    } else {
        "已启用（默认拒绝 localhost/私网）".to_string()
    }
}

fn memory_search_status_line(config: &AgentConfig) -> String {
    if !config.tools.memory_search.enabled {
        return "已关闭（[tools.memory_search] enabled = false，未注册）".to_string();
    }
    "已启用（默认扫描 MEMORY / SOUL / USER；子串检索，无向量库）".to_string()
}

fn memory_write_status_line(config: &AgentConfig) -> String {
    if !config.tools.memory_write.enabled {
        return "已关闭（[tools.memory_write] enabled = false，未注册）".to_string();
    }
    "已启用（只写配置的 MEMORY.md；append/overwrite，上限 32KiB）".to_string()
}

fn read_file_status_line(config: &AgentConfig) -> String {
    if !config.tools.read_file.enabled {
        return "已关闭（[tools.read_file] enabled = false，未注册）".to_string();
    }
    "已启用（工作区相对路径只读；禁穿越/symlink 逃逸；上限 256KiB；按行 offset/limit）".to_string()
}

fn list_dir_status_line(config: &AgentConfig) -> String {
    if !config.tools.list_dir.enabled {
        return "已关闭（[tools.list_dir] enabled = false，未注册）".to_string();
    }
    "已启用（工作区列目录；默认不递归；禁穿越/symlink 逃逸）".to_string()
}

fn write_file_status_line(config: &AgentConfig) -> String {
    if !config.tools.write_file.enabled {
        return "已关闭（[tools.write_file] enabled = false，未注册）".to_string();
    }
    "已启用（工作区相对路径写入；overwrite/append；禁穿越/symlink 逃逸；上限 256KiB；原子写）"
        .to_string()
}

fn delete_file_status_line(config: &AgentConfig) -> String {
    if !config.tools.delete_file.enabled {
        return "已关闭（[tools.delete_file] enabled = false，未注册）".to_string();
    }
    "已启用（工作区相对路径删除常规文件；拒绝目录；缺文件报错；禁穿越/symlink 逃逸）".to_string()
}

fn str_replace_status_line(config: &AgentConfig) -> String {
    if !config.tools.str_replace.enabled {
        return "已关闭（[tools.str_replace] enabled = false，未注册）".to_string();
    }
    "已启用（工作区相对路径精确替换；默认恰好 1 次；上限 256KiB；原子写；禁穿越/symlink 逃逸）"
        .to_string()
}

fn grep_status_line(config: &AgentConfig) -> String {
    if !config.tools.grep.enabled {
        return "已关闭（[tools.grep] enabled = false，未注册）".to_string();
    }
    "已启用（工作区字面量文本搜索，非正则；默认最多 50 条；禁穿越/symlink 逃逸）".to_string()
}

fn glob_status_line(config: &AgentConfig) -> String {
    if !config.tools.glob.enabled {
        return "已关闭（[tools.glob] enabled = false，未注册）".to_string();
    }
    "已启用（工作区 glob 按模式找文件；默认最多 100 条；禁穿越/symlink 逃逸）".to_string()
}

fn mkdir_status_line(config: &AgentConfig) -> String {
    if !config.tools.mkdir.enabled {
        return "已关闭（[tools.mkdir] enabled = false，未注册）".to_string();
    }
    "已启用（工作区相对路径创建目录；默认 mkdir -p；禁穿越/symlink 逃逸）".to_string()
}

fn move_status_line(config: &AgentConfig) -> String {
    if !config.tools.r#move.enabled {
        return "已关闭（[tools.move] enabled = false，未注册）".to_string();
    }
    "已启用（工作区相对路径移动/重命名；默认不覆盖；禁穿越/symlink 逃逸）".to_string()
}

fn session_summarize_status_line(config: &AgentConfig) -> String {
    if config.session.effective_summarize_on_overflow() {
        format!(
            "已启用（keep_recent={}, 通过 {}）",
            config.session.effective_keep_recent(),
            session_summarize_config_source()
        )
    } else {
        "未启用（超过上限硬截断）".to_string()
    }
}

fn heartbeat_file_status_label(config: &AgentConfig) -> String {
    match inspect_heartbeat_file(&config.workspace_path, &config.heartbeat.path) {
        Ok(status) if status.exists => format!("存在 ({} bytes)", status.size_bytes),
        Ok(_) => "不存在".to_string(),
        Err(e) => format!("路径无效 ({e})"),
    }
}

/// 心跳一轮的结果（供测试断言；serve 循环忽略具体值）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeartbeatTickOutcome {
    SkippedEmptyOrMissing,
    Completed {
        user_chars: usize,
        reply_chars: usize,
    },
    Failed(String),
}

type HeartbeatTickHook = Arc<dyn Fn() + Send + Sync>;

fn maybe_spawn_heartbeat(
    state: AppState,
    config: &AgentConfig,
    on_tick: Option<HeartbeatTickHook>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.heartbeat.enabled {
        return None;
    }

    let rel_path = config.heartbeat.path.clone();
    if let Err(e) = resolve_heartbeat_path(&config.workspace_path, &rel_path) {
        tracing::warn!("Heartbeat 已启用但路径无效，不启动后台任务: {e}");
        return None;
    }

    let workspace = config.workspace_path.clone();
    let session_id = config.heartbeat.effective_session_id().to_string();
    let interval = Duration::from_secs(config.heartbeat.effective_interval_secs());

    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let _ = run_heartbeat_tick(&state, &workspace, &rel_path, &session_id).await;
            if let Some(hook) = &on_tick {
                hook();
            }
        }
    }))
}

async fn run_heartbeat_tick(
    state: &AppState,
    workspace: &std::path::Path,
    rel_path: &str,
    session_id: &str,
) -> HeartbeatTickOutcome {
    let content = match load_heartbeat_message(workspace, rel_path) {
        Ok(Some(text)) => text,
        Ok(None) => {
            tracing::debug!(
                session_id,
                path = rel_path,
                "Heartbeat 跳过本轮：文件缺失或为空"
            );
            return HeartbeatTickOutcome::SkippedEmptyOrMissing;
        }
        Err(e) => {
            tracing::warn!(session_id, "Heartbeat 本轮跳过：无法读取文件: {e}");
            return HeartbeatTickOutcome::Failed(e.to_string());
        }
    };

    let user_chars = content.chars().count();
    match run_session_user_chat(state, session_id, &content, "heartbeat", "heartbeat").await {
        Ok(reply) => {
            let reply_chars = reply.chars().count();
            tracing::info!(session_id, user_chars, reply_chars, "Heartbeat 完成一轮");
            HeartbeatTickOutcome::Completed {
                user_chars,
                reply_chars,
            }
        }
        Err(e) => {
            let message = match e {
                AppError::ChannelConflict => "channel_state_conflict".to_string(),
                AppError::ChannelUnavailable => "channel_unavailable".to_string(),
                AppError::Internal(msg) | AppError::BadRequest(msg) => msg,
                AppError::Unauthorized => "Unauthorized".to_string(),
                AppError::NotFound => "NotFound".to_string(),
                AppError::Conflict => "Conflict".to_string(),
                AppError::ServiceUnavailable => "ServiceUnavailable".to_string(),
                AppError::JobConflict(message) => message,
            };
            tracing::warn!(session_id, "Heartbeat 本轮 chat 失败: {message}");
            HeartbeatTickOutcome::Failed(message)
        }
    }
}

fn persist_session_map(state: &AppState, sessions: &SessionStore) -> Result<()> {
    if !state.persist_enabled {
        return Ok(());
    }
    match sessions {
        SessionStore::Sqlite { .. } => sessions.flush(),
        SessionStore::Memory(map) => {
            let raw = map
                .iter()
                .map(|(id, rec)| (id.clone(), rec.messages.clone()))
                .collect();
            save_sessions(&state.persist_path, &raw)
        }
    }
}

async fn with_sessions<T: Send + 'static>(
    state: &AppState,
    operation: impl FnOnce(&mut SessionStore) -> Result<T> + Send + 'static,
) -> Result<T, AppError> {
    let sessions = state.sessions.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("session store lock poisoned"))?;
        operation(&mut store)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| {
        tracing::error!(error = %e, "会话存储失败");
        AppError::Internal("session_storage_error".into())
    })
}

async fn session_turn_lock(state: &AppState, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = state.turn_locks.lock().unwrap();
        locks.retain(|_, value| value.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(std::sync::Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(id.into(), Arc::downgrade(&lock));
            lock
        }
    };
    lock.lock_owned().await
}

/// 合并 session 历史与本轮入站消息，并在超过上限时压缩（共享写入前的唯一裁剪点）。
///
/// HTTP `/api/chat`、webhook / Telegram / Slack / heartbeat 都走这里，避免通道分叉。
async fn prepare_session_chat_messages(
    state: &AppState,
    session_id: &str,
    incoming: Vec<ChatMessage>,
    request_id: &str,
    channel_label: &str,
) -> Result<Vec<ChatMessage>, AppError> {
    let id = session_id.to_string();
    let history = with_sessions(state, move |sessions| {
        Ok(sessions.get(&id)?.map(|rec| rec.messages))
    })
    .await?;

    let Some(history) = history else {
        tracing::info!(
            request_id = %request_id,
            "创建新 {channel_label} session: {session_id}"
        );
        return Ok(incoming);
    };

    let mut all_messages = history;
    all_messages.extend(incoming);
    let before = all_messages.len();
    if before > MAX_SESSION_MESSAGES {
        tracing::info!(
            request_id = %request_id,
            "{channel_label} session {session_id} 消息数 {before} 超过上限 {MAX_SESSION_MESSAGES}，开始压缩"
        );
    }

    let compacted = state.agent.compact_session_messages(all_messages).await;
    tracing::info!(
        request_id = %request_id,
        "使用 {channel_label} session {session_id}, 合并后消息数: {} (压缩前 {before})",
        compacted.len()
    );
    Ok(compacted)
}

/// 将压缩后的完整历史写回共享 session store（含可选落盘）。
async fn commit_session_messages(
    state: &AppState,
    session_id: &str,
    messages: Vec<ChatMessage>,
    request_id: &str,
    channel_label: &str,
) -> Result<(), AppError> {
    let id = session_id.to_string();
    let state_copy = state.clone();
    with_sessions(state, move |sessions| {
        sessions.insert(id, SessionRecord::new(messages))?;
        persist_session_map(&state_copy, sessions)
    })
    .await?;
    tracing::info!(request_id, channel_label, session_id, "会话提交成功");
    Ok(())
}

async fn purge_expired_sessions(state: &AppState) -> usize {
    let Some(ttl) = state.session_ttl else {
        return 0;
    };
    let active = state
        .turn_locks
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, lock)| lock.strong_count() > 0)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let state_copy = state.clone();
    match with_sessions(state, move |sessions| {
        let removed = sessions.purge(ttl, &active)?;
        if removed > 0 {
            persist_session_map(&state_copy, sessions)?;
        }
        Ok(removed)
    })
    .await
    {
        Ok(n) => n,
        Err(_) => {
            tracing::error!("TTL 清理失败");
            0
        }
    }
}

fn spawn_session_ttl_sweeper(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SESSION_TTL_SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let removed = purge_expired_sessions(&state).await;
            if removed > 0 {
                tracing::info!("Session TTL 扫描移除 {removed} 个过期会话");
            }
        }
    })
}

/// serve 进程内后台任务，优雅退出时 abort，避免进程退出后仍跑。
struct BackgroundTasks {
    channels: Option<channels::ChannelWorkers>,
    scheduler: Option<scheduler::Scheduler>,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
    ttl_sweeper: Option<tokio::task::JoinHandle<()>>,
    skill_reloader: Option<tokio::task::JoinHandle<()>>,
}

impl BackgroundTasks {
    fn abort(&self) {
        if let Some(handle) = &self.heartbeat {
            tracing::info!("正在停止 Heartbeat 后台任务");
            handle.abort();
        }
        if let Some(handle) = &self.ttl_sweeper {
            tracing::info!("正在停止 Session TTL 扫描任务");
            handle.abort();
        }
        if let Some(handle) = &self.skill_reloader {
            tracing::info!("正在停止 SIGHUP 技能重载监听");
            handle.abort();
        }
    }
}

/// 关闭路径刷盘：与正常 persist 相同的原子 `save_sessions`。
///
/// 失败打 error 并返回 `false`，调用方仍继续退出。
fn flush_sessions_on_shutdown(state: &AppState) -> bool {
    if !state.persist_enabled {
        tracing::info!("Session 持久化未启用，跳过关闭刷盘");
        return true;
    }

    let sessions = match state.sessions.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("关闭时 sessions 锁已毒化，无法刷盘: {poisoned}");
            return false;
        }
    };

    match persist_session_map(state, &sessions) {
        Ok(()) => {
            tracing::info!(
                count = sessions.len().unwrap_or_default(),
                path = %state.persist_path.display(),
                "关闭时已刷盘 sessions"
            );
            true
        }
        Err(e) => {
            tracing::error!("关闭时保存 sessions 失败: {e}");
            false
        }
    }
}

/// 用 axum `with_graceful_shutdown` 停止 accept，并给进行中请求一个宽限期。
async fn serve_with_graceful_shutdown<F>(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    state: AppState,
    shutdown: F,
    shutdown_timeout: Duration,
    background: BackgroundTasks,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    let server_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    shutdown.await;
    tracing::info!("收到关闭信号，停止接受新连接，等待进行中请求结束");
    let shutdown_deadline = Instant::now() + shutdown_timeout;
    let _ = shutdown_tx.send(());
    background.abort();
    if let Some(channels) = &background.channels {
        channels.stop(&state);
    }
    tokio::join!(
        async {
            if let Some(scheduler) = background.scheduler {
                scheduler.shutdown(&state, shutdown_timeout).await;
            }
        },
        async {
            if let Some(channels) = background.channels {
                channels.shutdown(&state, shutdown_timeout).await;
            }
        }
    );

    match tokio::time::timeout_at(shutdown_deadline, server_task).await {
        Ok(Ok(Ok(()))) => {
            tracing::info!("进行中请求已完成");
        }
        Ok(Ok(Err(e))) => {
            return Err(e).context("服务器运行失败");
        }
        Ok(Err(join_err)) => {
            if join_err.is_cancelled() {
                tracing::warn!("HTTP 服务任务已取消");
            } else {
                return Err(join_err).context("HTTP 服务任务失败");
            }
        }
        Err(_) => {
            tracing::warn!(
                secs = shutdown_timeout.as_secs(),
                "优雅退出宽限期已到，结束剩余连接"
            );
        }
    }

    if !flush_sessions_on_shutdown(&state) {
        tracing::warn!("关闭时 sessions 刷盘失败，仍继续退出");
    }
    tracing::info!("服务器已关闭");
    Ok(())
}

/// Webhook 入站响应
#[derive(Debug, Serialize, Deserialize)]
struct InboundWebhookResponse {
    ok: bool,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn validate_host_provider(config: &AgentConfig) -> Result<()> {
    match config.provider.provider_type.as_str() {
        "stub" => Ok(()),
        "brokerrouter" | "openai_compatible" => {
            let key = std::env::var("JIACLAW_API_KEY")
                .ok()
                .or_else(|| config.provider.api_key.clone());
            if key.is_none_or(|key| key.trim().is_empty()) {
                anyhow::bail!("模型提供商缺少 API Key；不会自动回退到 stub。离线验收请显式配置 provider_type='stub'");
            }
            Ok(())
        }
        other => anyhow::bail!("未知 provider_type: {other}"),
    }
}

#[allow(clippy::too_many_lines)]
async fn serve_command(config_path: Option<PathBuf>, bind: Option<String>) -> Result<()> {
    // 加载配置
    let mut config = if let Some(path) = config_path {
        let path_str = path.to_string_lossy();
        if path_str.ends_with(".toml") {
            AgentConfig::from_toml_file(&path)?
        } else if path_str.ends_with(".json") {
            AgentConfig::from_json_file(&path)?
        } else {
            AgentConfig::from_toml_file(&path).or_else(|_| AgentConfig::from_json_file(&path))?
        }
    } else {
        AgentConfig::default()
    };

    // 命令行参数覆盖配置文件（如果提供）
    if let Some(bind_addr) = bind {
        config.http.bind = bind_addr;
    }

    init_tracing_from(&config.logging);
    tracing::info!("正在启动 JiaClaw Agent 服务");
    tracing::info!("使用 Agent 配置: {}", config.name);
    tracing::info!(
        format = %config.logging.effective_format(),
        source = log_format_config_source(),
        "日志格式"
    );

    // 读取 API token（环境变量优先于配置文件）
    let api_token = std::env::var("JIACLAW_API_TOKEN")
        .ok()
        .or(config.http.api_token.clone());

    let api_token = api_token.filter(|token| !token.trim().is_empty());
    validate_host_provider(&config)?;
    let bind_address: std::net::SocketAddr = config
        .http
        .bind
        .parse()
        .context("bind 必须为 IP:端口，例如 127.0.0.1:8080")?;
    if !bind_address.ip().is_loopback() && api_token.is_none() {
        anyhow::bail!("非本机监听必须设置 JIACLAW_API_TOKEN 或 http.api_token");
    }
    if config.tools.exec.enabled && api_token.is_none() {
        anyhow::bail!("启用 exec 必须设置 API Token");
    }
    if !config.mcp.servers.is_empty() && api_token.is_none() {
        anyhow::bail!("启用 MCP 必须设置 API Token");
    }

    if config.scheduler.enabled {
        if !config.http.persist || api_token.is_none() {
            anyhow::bail!("scheduler requires SQLite persistence and an API Token");
        }
        if !matches!(
            config.provider.provider_type.as_str(),
            "brokerrouter" | "stub"
        ) {
            anyhow::bail!("scheduler requires brokerrouter or explicit stub provider");
        }
        if config.heartbeat.enabled {
            anyhow::bail!("scheduler and legacy HEARTBEAT cannot both be enabled; register the heartbeat prompt as an explicit job");
        }
    }

    let channel_runtime = channels::ChannelRuntime::configure(&config, api_token.as_deref())?;

    // Local authorization is checked before outbound MCP discovery.
    let metrics = Arc::new(Metrics::default());
    let agent = attach_tool_metrics(
        JiaClawAgent::connect(config.clone())
            .await
            .context("创建 JiaClawAgent 失败")?,
        &metrics,
    );

    // 读取 webhook secret（环境变量优先于配置文件）
    let webhook_secret = std::env::var("JIACLAW_WEBHOOK_SECRET")
        .ok()
        .or(config.http.webhook_secret.clone())
        .filter(|value| !value.trim().is_empty());

    // 读取 Telegram secret token（环境变量优先于配置文件）
    let telegram_secret = std::env::var("JIACLAW_TELEGRAM_SECRET")
        .ok()
        .or(config.http.telegram_secret.clone())
        .filter(|value| !value.trim().is_empty());

    // 读取 Telegram Bot API token（环境变量优先于配置文件）
    let telegram_bot_token = config.http.effective_telegram_bot_token();

    // 读取 Slack signing secret / Bot token（环境变量优先于配置文件）
    let slack_signing_secret = config.http.effective_slack_signing_secret();
    let slack_bot_token = config.http.effective_slack_bot_token();
    let discord_public_key = config.http.effective_discord_public_key();
    let discord_bot_token = config.http.effective_discord_bot_token();

    // 读取限流配置（环境变量优先于配置文件）
    let rate_limit_per_minute = config.http.effective_rate_limit_per_minute();
    let rate_limiter = rate_limit_per_minute.and_then(build_rate_limiter);

    // 读取会话闲置 TTL（环境变量优先于配置文件）
    let session_ttl_secs = config.http.effective_session_ttl_secs();
    let session_ttl = session_ttl_secs.map(Duration::from_secs);
    let shutdown_timeout_secs = config.http.effective_shutdown_timeout_secs();
    let shutdown_timeout = Duration::from_secs(shutdown_timeout_secs);
    let max_body_bytes = config.http.effective_max_body_bytes();
    let metrics_public = config.http.effective_metrics_public();
    let metrics_require_auth = !metrics_public;

    // 解析持久化路径
    let persist_path = persist_path_from_config(&config);

    let sessions = if config.http.persist {
        open_configured_session_store(&config)?
    } else {
        SessionStore::memory()
    };

    let state = AppState {
        agent: Arc::new(agent),
        sessions: Arc::new(Mutex::new(sessions)),
        turn_locks: Arc::new(Mutex::new(HashMap::new())),
        api_token: api_token.clone(),
        webhook_secret: webhook_secret.clone(),
        persist_enabled: config.http.persist,
        persist_path: Arc::new(persist_path),
        rate_limiter,
        session_ttl,
        metrics,
        metrics_require_auth,
        channel_runtime,
        scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
    };

    let mut ttl_sweeper = None;
    if state.session_ttl.is_some() {
        ttl_sweeper = Some(spawn_session_ttl_sweeper(state.clone()));
    }

    let heartbeat_interval_secs = config.heartbeat.effective_interval_secs();
    let heartbeat_handle = maybe_spawn_heartbeat(state.clone(), &config, None);
    if heartbeat_handle.is_some() {
        tracing::info!(
            interval_secs = heartbeat_interval_secs,
            session_id = config.heartbeat.effective_session_id(),
            path = %config.heartbeat.path,
            file = %heartbeat_file_status_label(&config),
            "Heartbeat 已启用"
        );
    } else if config.heartbeat.enabled {
        tracing::warn!("Heartbeat 配置为启用，但未能启动后台任务");
    } else {
        tracing::info!("Heartbeat 未启用");
    }

    // 配置 CORS（默认关闭；request_id 仍在外层，preflight 也会回写 X-Request-Id）
    let cors = config.http.cors.resolved();
    let cors_layer = build_cors_layer(&cors);

    // 构建路由（限流中间件不依赖 ConnectInfo，oneshot 测试不会 500）
    let shutdown_state = state.clone();
    let app = build_router_with_body_limit(state.clone(), cors_layer, max_body_bytes)
        .layer(middleware::from_fn_with_state(state, configured_hooks_only));

    // 绑定地址
    let listener = tokio::net::TcpListener::bind(&config.http.bind)
        .await
        .with_context(|| format!("无法绑定到地址: {}", config.http.bind))?;

    config.http.bind = listener.local_addr()?.to_string();

    // 启动日志
    tracing::info!("✅ HTTP 服务已启动于 http://{}", config.http.bind);
    tracing::info!("   • 日志格式: {}", logging_status_line(&config.logging));
    tracing::info!("   • GET    /health              - 健康检查");
    tracing::info!("   • GET    /metrics             - Prometheus 文本指标");
    tracing::info!("   • POST   /api/chat            - 聊天端点（可选 SSE）");
    tracing::info!("   • GET    /api/sessions        - 列出会话");
    tracing::info!("   • POST   /api/sessions        - 创建会话");
    tracing::info!("   • GET    /api/sessions/:id    - 读取会话历史");
    tracing::info!("   • GET    /api/sessions/:id/export - 导出会话（JSONL / JSON）");
    tracing::info!("   • POST   /api/sessions/import - 导入会话（JSONL / JSON）");
    tracing::info!("   • DELETE /api/sessions/:id    - 删除会话");
    tracing::info!("   • GET    /api/tools           - 列出已注册工具");
    tracing::info!("   • GET    /api/skills          - 列出已加载技能（进程内注册表）");
    tracing::info!("   • POST   /api/skills/reload   - 热加载技能（不重启 serve）");
    tracing::info!("   • GET    /api/openapi.json    - OpenAPI 3 草图");
    tracing::info!("   • POST   /hooks/inbound       - Webhook 入站端点");
    tracing::info!("   • POST   /hooks/telegram      - Telegram Bot 入站端点");
    tracing::info!("   • POST   /hooks/slack         - Slack Events API 入站端点");
    tracing::info!("   • POST   /hooks/discord       - Discord Interactions 入站端点");
    if telegram_bot_token.is_some() {
        tracing::info!("   • Telegram 出站: 已配置 Bot Token（成功回复后调用 sendMessage）");
    } else {
        tracing::info!("   • Telegram 出站: 未配置；启用渠道还需显式安装/身份/工具策略");
    }
    if slack_bot_token.is_some() {
        tracing::info!("   • Slack 出站: 已配置 Bot Token（成功回复后调用 chat.postMessage）");
    } else {
        tracing::info!("   • Slack 出站: 未配置；启用渠道还需显式安装/身份/工具策略");
    }
    if discord_bot_token.is_some() {
        tracing::info!(
            "   • Discord 出站: interaction 回复通过持久 outbox 投递（不发送 Bot Authorization）"
        );
    } else {
        tracing::info!("   • Discord 出站: interaction 回复使用加密的短期令牌；无需 Bot Token");
    }
    tracing::info!("   • X-Request-Id                - 请求无该头则生成 UUID 并回写");
    tracing::info!(
        "   • CORS                        - {}",
        cors_status_line(&config.http.cors)
    );
    if let Some(limit) = rate_limit_per_minute {
        tracing::info!(
            "   • HTTP 限流: {limit} 次/分钟（/api/* 与 /hooks/* 带 X-RateLimit-*；429 另带 Retry-After；GET /health 与 GET /metrics 不限流）"
        );
    } else {
        tracing::info!("   • HTTP 限流: 未启用");
    }
    tracing::info!("   • HTTP 请求体上限: {max_body_bytes} 字节（超限 413；GET /health 与 GET /metrics 不检查）");
    if metrics_public {
        tracing::info!("   • Metrics: 公开（GET /metrics 无需 API Bearer，便于 scrape）");
    } else {
        tracing::info!(
            "   • Metrics: 需 API 鉴权（与 /api/* 相同，通过 {}）",
            metrics_auth_config_source()
        );
    }
    if let Some(ttl) = session_ttl_secs {
        tracing::info!("   • Session TTL: 已启用（闲置 {ttl} 秒后过期）");
    } else {
        tracing::info!("   • Session TTL: 未启用");
    }
    tracing::info!("   • 优雅退出: 支持 SIGINT/SIGTERM（宽限期 {shutdown_timeout_secs} 秒）");
    tracing::info!(
        "   • 技能热加载: POST /api/skills/reload；Unix 上 SIGHUP 触发同样路径（Windows 仅 HTTP）"
    );
    tracing::info!(
        "   • Session 摘要压缩: {}",
        session_summarize_status_line(&config)
    );
    if let Some(secs) = config.effective_tool_timeout_secs() {
        tracing::info!("   • 工具超时: 已启用（每调用 {secs} 秒）");
    } else {
        tracing::info!("   • 工具超时: 未启用（不限制）");
    }
    tracing::info!(
        "   • 工具循环上限: {}",
        max_tool_iterations_status_line(&config)
    );
    if config.tools.web_search.enabled {
        if config.tools.web_search.effective_brave_api_key().is_some() {
            tracing::info!("   • web_search: 已启用（Brave API key 已配置，明文不打印）");
        } else {
            tracing::info!("   • web_search: 已启用但未配置 Brave API key（调用将返回友好错误）");
        }
    } else {
        tracing::info!("   • web_search: 已关闭（未注册）");
    }
    if config.tools.web_fetch.enabled {
        if config.tools.web_fetch.allow_private {
            tracing::info!("   • web_fetch: 已启用（allow_private = true，允许 localhost/私网）");
        } else {
            tracing::info!("   • web_fetch: 已启用（默认拒绝 localhost/私网）");
        }
    } else {
        tracing::info!("   • web_fetch: 已关闭（未注册）");
    }
    if config.tools.memory_search.enabled {
        tracing::info!("   • memory_search: 已启用（默认扫描 MEMORY / SOUL / USER）");
    } else {
        tracing::info!("   • memory_search: 已关闭（未注册）");
    }
    if config.tools.memory_write.enabled {
        tracing::info!("   • memory_write: 已启用（只写配置的 MEMORY.md，上限 32KiB）");
    } else {
        tracing::info!("   • memory_write: 已关闭（未注册）");
    }
    if config.tools.read_file.enabled {
        tracing::info!("   • read_file: 已启用（工作区相对路径只读，上限 256KiB）");
    } else {
        tracing::info!("   • read_file: 已关闭（未注册）");
    }
    if config.tools.list_dir.enabled {
        tracing::info!("   • list_dir: 已启用（工作区列目录，默认不递归）");
    } else {
        tracing::info!("   • list_dir: 已关闭（未注册）");
    }
    if config.tools.write_file.enabled {
        tracing::info!("   • write_file: 已启用（工作区相对路径写入，上限 256KiB，原子写）");
    } else {
        tracing::info!("   • write_file: 已关闭（未注册）");
    }
    if config.tools.delete_file.enabled {
        tracing::info!("   • delete_file: 已启用（工作区相对路径删除常规文件，拒绝目录）");
    } else {
        tracing::info!("   • delete_file: 已关闭（未注册）");
    }
    if config.tools.str_replace.enabled {
        tracing::info!("   • str_replace: 已启用（工作区相对路径精确替换，上限 256KiB，原子写）");
    } else {
        tracing::info!("   • str_replace: 已关闭（未注册）");
    }
    if config.tools.grep.enabled {
        tracing::info!("   • grep: 已启用（工作区字面量文本搜索，非正则，默认最多 50 条）");
    } else {
        tracing::info!("   • grep: 已关闭（未注册）");
    }
    if config.tools.glob.enabled {
        tracing::info!("   • glob: 已启用（工作区 glob 按模式找文件，默认最多 100 条）");
    } else {
        tracing::info!("   • glob: 已关闭（未注册）");
    }
    if config.tools.mkdir.enabled {
        tracing::info!("   • mkdir: 已启用（工作区相对路径创建目录，默认 mkdir -p）");
    } else {
        tracing::info!("   • mkdir: 已关闭（未注册）");
    }
    if config.tools.r#move.enabled {
        tracing::info!("   • move: 已启用（工作区相对路径移动/重命名，默认不覆盖）");
    } else {
        tracing::info!("   • move: 已关闭（未注册）");
    }
    if config.heartbeat.enabled {
        tracing::info!(
            "   • Heartbeat: 已启用（间隔 {heartbeat_interval_secs} 秒，session={}, 文件 {}）",
            config.heartbeat.effective_session_id(),
            heartbeat_file_status_label(&config)
        );
    } else {
        tracing::info!("   • Heartbeat: 未启用");
    }

    // 打印配置摘要（不打印 secret 明文）
    println!("\n📋 HTTP 配置摘要:");
    println!("   • 绑定地址: {}", config.http.bind);
    println!("   • 日志格式: {}", logging_status_line(&config.logging));

    if api_token.is_some() {
        println!(
            "   • API 鉴权: ✅ 已启用（通过 {}）",
            if std::env::var("JIACLAW_API_TOKEN").is_ok() {
                "环境变量 JIACLAW_API_TOKEN"
            } else {
                "配置文件"
            }
        );
    } else {
        println!("   • API 鉴权: ⚠️  未启用（API 端点无需鉴权，本地开发友好）");
    }

    if webhook_secret.is_some() {
        println!(
            "   • Webhook 鉴权: ✅ 已启用（通过 {}）",
            if std::env::var("JIACLAW_WEBHOOK_SECRET").is_ok() {
                "环境变量 JIACLAW_WEBHOOK_SECRET"
            } else {
                "配置文件"
            }
        );
    } else {
        println!("   • Webhook 鉴权: ⚠️  未配置（/hooks/inbound 已关闭）");
    }

    if telegram_secret.is_some() {
        println!(
            "   • Telegram 鉴权: ✅ 已启用（通过 {}）",
            if std::env::var("JIACLAW_TELEGRAM_SECRET").is_ok() {
                "环境变量 JIACLAW_TELEGRAM_SECRET"
            } else {
                "配置文件"
            }
        );
    } else {
        println!("   • Telegram 鉴权: ⚠️  未配置（/hooks/telegram 已关闭）");
    }

    if telegram_bot_token.is_some() {
        println!(
            "   • Telegram Bot Token: ✅ 已配置（通过 {}，明文不打印；将 sendMessage 出站）",
            telegram_token_config_source()
        );
    } else {
        println!("   • Telegram Bot Token: ⚠️  未配置；Telegram 渠道需要 Token 和授权策略");
    }

    if slack_signing_secret.is_some() {
        println!(
            "   • Slack 签名校验: ✅ 已启用（通过 {}）",
            slack_signing_secret_config_source()
        );
    } else {
        println!("   • Slack 签名校验: ⚠️  未配置（/hooks/slack 已关闭）");
    }

    if slack_bot_token.is_some() {
        println!(
            "   • Slack Bot Token: ✅ 已配置（通过 {}，明文不打印；将 chat.postMessage 出站）",
            slack_token_config_source()
        );
    } else {
        println!("   • Slack Bot Token: ⚠️  未配置；Slack 渠道需要 Token 和授权策略");
    }

    if discord_public_key.is_some() {
        println!(
            "   • Discord 签名校验: ✅ 已启用（通过 {}）",
            discord_public_key_config_source()
        );
    } else {
        println!("   • Discord 签名校验: ⚠️  未配置（/hooks/discord 已关闭）");
    }

    if discord_bot_token.is_some() {
        println!(
            "   • Discord 旧 Bot Token: 已配置（通过 {}，回复不会发送 Bot Authorization）",
            discord_token_config_source()
        );
    } else {
        println!(
            "   • Discord Bot Token: ⚠️  不需要 Bot Token；interaction 回复使用加密的短期令牌"
        );
    }

    if let Some(limit) = rate_limit_per_minute {
        println!(
            "   • HTTP 限流: ✅ 已启用（{limit} 次/分钟，通过 {}）",
            rate_limit_config_source()
        );
    } else {
        println!(
            "   • HTTP 限流: ⚠️  未启用（/api/* 与 /hooks/inbound、/hooks/telegram、/hooks/slack、/hooks/discord 不限流）"
        );
    }

    println!(
        "   • HTTP 请求体上限: {max_body_bytes} 字节（超限 413，通过 {}；GET /health 与 GET /metrics 不检查）",
        max_body_bytes_config_source()
    );

    if metrics_public {
        println!("   • Metrics: ✅ 公开（GET /metrics 无需鉴权，不计入限流）");
    } else {
        println!(
            "   • Metrics: 🔒 需 API 鉴权（与 /api/* 相同，通过 {}，不计入限流）",
            metrics_auth_config_source()
        );
    }

    if let Some(ttl) = session_ttl_secs {
        println!(
            "   • Session TTL: ✅ 已启用（闲置 {ttl} 秒，通过 {}）",
            session_ttl_config_source()
        );
    } else {
        println!("   • Session TTL: ⚠️  未启用（会话不会因闲置过期）");
    }

    println!(
        "   • 优雅退出: ✅ 支持 SIGINT/SIGTERM（宽限期 {shutdown_timeout_secs} 秒，通过 {}）",
        shutdown_timeout_config_source()
    );
    println!(
        "   • 技能热加载: ✅ POST /api/skills/reload；Unix SIGHUP 同步 reload（Windows 仅 HTTP）"
    );

    if config.session.effective_summarize_on_overflow() {
        println!(
            "   • Session 摘要压缩: ✅ {}",
            session_summarize_status_line(&config)
        );
    } else {
        println!(
            "   • Session 摘要压缩: ⚠️  {}；可设置 [session] summarize_on_overflow = true 或 JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW=1",
            session_summarize_status_line(&config)
        );
    }

    if let Some(secs) = config.effective_tool_timeout_secs() {
        println!(
            "   • 工具超时: ✅ 已启用（每调用 {secs} 秒，通过 {}）",
            tool_timeout_config_source()
        );
    } else {
        println!("   • 工具超时: ⚠️  未启用（不限制单次工具执行时间）");
    }

    println!(
        "   • 工具循环上限: {}",
        max_tool_iterations_status_line(&config)
    );

    if config.tools.web_search.enabled {
        if config.tools.web_search.effective_brave_api_key().is_some() {
            println!(
                "   • web_search: ✅ 已启用（Brave API key 已配置，通过 {}，明文不打印）",
                brave_api_key_config_source()
            );
        } else {
            println!("   • web_search: ⚠️  已启用但未配置 Brave API key（调用将返回友好错误）");
        }
    } else {
        println!("   • web_search: ⚠️  已关闭（[tools.web_search] enabled = false）");
    }

    if config.tools.web_fetch.enabled {
        if config.tools.web_fetch.allow_private {
            println!("   • web_fetch: ✅ 已启用（allow_private = true，允许 localhost/私网）");
        } else {
            println!("   • web_fetch: ✅ 已启用（默认拒绝 localhost/私网）");
        }
    } else {
        println!("   • web_fetch: ⚠️  已关闭（[tools.web_fetch] enabled = false）");
    }

    if config.tools.memory_search.enabled {
        println!(
            "   • memory_search: ✅ {}",
            memory_search_status_line(&config)
        );
    } else {
        println!(
            "   • memory_search: ⚠️  {}",
            memory_search_status_line(&config)
        );
    }

    if config.tools.memory_write.enabled {
        println!(
            "   • memory_write: ✅ {}",
            memory_write_status_line(&config)
        );
    } else {
        println!(
            "   • memory_write: ⚠️  {}",
            memory_write_status_line(&config)
        );
    }

    if config.tools.read_file.enabled {
        println!("   • read_file: ✅ {}", read_file_status_line(&config));
    } else {
        println!("   • read_file: ⚠️  {}", read_file_status_line(&config));
    }

    if config.tools.list_dir.enabled {
        println!("   • list_dir: ✅ {}", list_dir_status_line(&config));
    } else {
        println!("   • list_dir: ⚠️  {}", list_dir_status_line(&config));
    }

    if config.tools.write_file.enabled {
        println!("   • write_file: ✅ {}", write_file_status_line(&config));
    } else {
        println!("   • write_file: ⚠️  {}", write_file_status_line(&config));
    }

    if config.tools.delete_file.enabled {
        println!("   • delete_file: ✅ {}", delete_file_status_line(&config));
    } else {
        println!("   • delete_file: ⚠️  {}", delete_file_status_line(&config));
    }

    if config.tools.str_replace.enabled {
        println!("   • str_replace: ✅ {}", str_replace_status_line(&config));
    } else {
        println!("   • str_replace: ⚠️  {}", str_replace_status_line(&config));
    }

    if config.tools.grep.enabled {
        println!("   • grep: ✅ {}", grep_status_line(&config));
    } else {
        println!("   • grep: ⚠️  {}", grep_status_line(&config));
    }

    if config.tools.glob.enabled {
        println!("   • glob: ✅ {}", glob_status_line(&config));
    } else {
        println!("   • glob: ⚠️  {}", glob_status_line(&config));
    }

    if config.tools.mkdir.enabled {
        println!("   • mkdir: ✅ {}", mkdir_status_line(&config));
    } else {
        println!("   • mkdir: ⚠️  {}", mkdir_status_line(&config));
    }

    if config.tools.r#move.enabled {
        println!("   • move: ✅ {}", move_status_line(&config));
    } else {
        println!("   • move: ⚠️  {}", move_status_line(&config));
    }

    if config.heartbeat.enabled {
        println!(
            "   • Heartbeat: ✅ 已启用（间隔 {heartbeat_interval_secs} 秒，session={}, 文件 {}，间隔来自 {}）",
            config.heartbeat.effective_session_id(),
            heartbeat_file_status_label(&config),
            heartbeat_interval_config_source()
        );
    } else {
        println!("   • Heartbeat: ⚠️  未启用（仅 serve 进程可挂后台任务；CLI chat 不跑心跳）");
    }

    if config.http.cors.resolved().enabled {
        println!(
            "   • CORS: ✅ {}（开关来自 {}，来源来自 {}）",
            cors_status_line(&config.http.cors),
            cors_enabled_config_source(),
            cors_origins_config_source()
        );
    } else {
        println!(
            "   • CORS: ⚠️  {}（开关来自 {}）",
            cors_status_line(&config.http.cors),
            cors_enabled_config_source()
        );
    }

    println!("\n💡 试试：curl http://{}/health", config.http.bind);
    println!("         curl http://{}/metrics", config.http.bind);
    println!("支持 SIGINT/SIGTERM 优雅退出（宽限期 {shutdown_timeout_secs} 秒）");
    println!("Unix 上可发送 SIGHUP 热加载技能；Windows 请用 POST /api/skills/reload");
    println!("按 Ctrl+C 或发送 SIGTERM 停止服务\n");

    let channels = channels::start(shutdown_state.clone()).await?;
    let scheduler = scheduler::start(shutdown_state.clone()).await?;
    let skill_reloader = spawn_skill_reload_on_sighup(shutdown_state.agent.clone());
    serve_with_graceful_shutdown(
        listener,
        app,
        shutdown_state,
        shutdown_signal(),
        shutdown_timeout,
        BackgroundTasks {
            channels,
            scheduler,
            heartbeat: heartbeat_handle,
            ttl_sweeper,
            skill_reloader,
        },
    )
    .await
}

// Production entry point closes each channel until its own authentication is configured.
// A browser/API token does not authenticate provider webhooks.
async fn configured_hooks_only(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let configured = match request.uri().path() {
        "/hooks/inbound" => state.webhook_secret.is_some(),
        "/hooks/telegram" => state
            .channel_runtime
            .as_ref()
            .is_some_and(|r| r.installation(channel_types::Channel::Telegram).is_some()),
        "/hooks/slack" => state
            .channel_runtime
            .as_ref()
            .is_some_and(|r| r.installation(channel_types::Channel::Slack).is_some()),
        "/hooks/discord" => state
            .channel_runtime
            .as_ref()
            .is_some_and(|r| r.installation(channel_types::Channel::Discord).is_some()),
        _ => true,
    };
    if !configured {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"channel_disabled"})),
        )
            .into_response();
    }
    next.run(request).await
}

/// 检查 API Token 鉴权
fn check_api_auth(state: &AppState, headers: &HeaderMap) -> bool {
    // 如果未配置 token，则不需要鉴权
    let Some(expected_token) = state.api_token.as_deref() else {
        return true;
    };

    // 检查 Authorization: Bearer <token>
    if let Some(auth_header) = headers.get("Authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return token == expected_token;
            }
        }
    }

    // 检查 X-Api-Token: <token>
    if let Some(token_header) = headers.get("X-Api-Token") {
        if let Ok(token) = token_header.to_str() {
            return token == expected_token;
        }
    }

    false
}

fn attach_tool_metrics(agent: JiaClawAgent, metrics: &Arc<Metrics>) -> JiaClawAgent {
    let metrics = Arc::clone(metrics);
    agent.with_tool_metrics_hook(Arc::new(move |tool, ok| {
        metrics.record_tool_call(tool, ok);
    }))
}

/// 健康检查处理器
async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let response = HealthResponse {
        status: "ok".to_string(),
        agent_name: state.agent.config().name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    Json(response)
}

/// Prometheus 文本指标。默认公开；`metrics_public = false` 或 `JIACLAW_METRICS_REQUIRE_AUTH=1` 时与 `/api/*` 相同鉴权。
async fn metrics_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if state.metrics_require_auth && !check_api_auth(&state, &headers) {
        tracing::warn!(
            request_id = request_id_log_value(&headers),
            "Metrics 鉴权失败: token 不匹配或缺失"
        );
        return Err(AppError::Unauthorized);
    }

    let sessions_active = state
        .sessions
        .lock()
        .map(|guard| u64::try_from(guard.len().unwrap_or_default()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let body = state
        .metrics
        .render(sessions_active, env!("CARGO_PKG_VERSION"));
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE),
        )],
        body,
    ))
}

/// `OpenAPI` 3 草图。鉴权与 `/api/tools` 一致：有 token 则需要鉴权。
async fn openapi_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if !check_api_auth(&state, &headers) {
        tracing::warn!(
            request_id = request_id_log_value(&headers),
            "API 鉴权失败: token 不匹配或缺失"
        );
        return Err(AppError::Unauthorized);
    }

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        OPENAPI_JSON,
    ))
}

/// HTTP 聊天请求体：核心 `ChatRequest` + 可选 `stream`。
#[derive(Debug, Deserialize)]
struct ChatHttpBody {
    #[serde(flatten)]
    request: ChatRequest,
    /// 为 true 时返回 SSE；也可通过 `Accept: text/event-stream` 开启。
    #[serde(default)]
    stream: bool,
}

/// `Accept` 是否显式包含 `text/event-stream`（不含 `*/*`）。
fn accept_includes_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| {
            accept.split(',').any(|part| {
                part.split(';')
                    .next()
                    .map(str::trim)
                    .is_some_and(|media| media.eq_ignore_ascii_case("text/event-stream"))
            })
        })
}

fn wants_event_stream(headers: &HeaderMap, stream_field: bool) -> bool {
    stream_field || accept_includes_event_stream(headers)
}

fn sse_json_event(name: &str, data: &serde_json::Value) -> Event {
    Event::default().event(name).data(data.to_string())
}

fn tool_sse_payload(call: &ToolCall) -> serde_json::Value {
    let error = call
        .result
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(serde_json::Value::as_str);
    match error {
        Some(error) => json!({
            "name": call.tool_name,
            "ok": false,
            "error": error,
        }),
        None => json!({
            "name": call.tool_name,
            "ok": true,
        }),
    }
}

/// 将助手文本按句/按块切开，供 SSE `token` 事件使用。
///
/// TODO(true-streaming): `Brokerrouter` / `StateKnot` 提供 token stream API 后改为真流式。
fn chunk_assistant_text(text: &str) -> Vec<String> {
    const MAX_CHARS: usize = 80;
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    for ch in text.chars() {
        current.push(ch);
        current_chars += 1;
        let at_sentence = matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '\n');
        if at_sentence || current_chars >= MAX_CHARS {
            chunks.push(std::mem::take(&mut current));
            current_chars = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn build_chat_sse_events(
    request_id: &str,
    session_id: Option<&str>,
    outcome: Result<&ChatResponse, &str>,
) -> Vec<Event> {
    let mut events = vec![sse_json_event(
        "meta",
        &json!({
            "session_id": session_id,
            "request_id": request_id,
        }),
    )];

    match outcome {
        Ok(response) => {
            for call in &response.tool_calls {
                events.push(sse_json_event("tool", &tool_sse_payload(call)));
            }
            // TODO(true-streaming): Brokerrouter 已支持 SSE；应用 provider 尚未接线，当前完成后分块。
            for chunk in chunk_assistant_text(&response.message.content) {
                events.push(sse_json_event("token", &json!({ "text": chunk })));
            }
            let status =
                serde_json::to_value(&response.status).unwrap_or_else(|_| json!("unknown"));
            events.push(sse_json_event(
                "done",
                &json!({
                    "reply": response.message.content,
                    "status": status,
                    "session_id": response.session_id,
                }),
            ));
        }
        Err(message) => {
            events.push(sse_json_event("error", &json!({ "error": message })));
        }
    }

    events
}

fn chat_sse_response(events: Vec<Event>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(stream::iter(events.into_iter().map(Ok)))
}

/// 聊天处理器
#[allow(clippy::too_many_lines)]
async fn chat_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChatHttpBody>,
) -> Result<Response, AppError> {
    let request_id = request_id_log_value(&headers).to_string();
    let stream = wants_event_stream(&headers, body.stream);
    let mut request = body.request;

    // API Token 鉴权检查（失败仍返回 JSON 401，即使客户端请求了 SSE）
    if !check_api_auth(&state, &headers) {
        tracing::warn!(request_id = %request_id, "API 鉴权失败: token 不匹配或缺失");
        return Err(AppError::Unauthorized);
    }

    tracing::info!(
        request_id = %request_id,
        "收到聊天请求，消息数: {}, session_id: {:?}, stream: {stream}",
        request.messages.len(),
        request.session_id
    );

    let session_id = request.session_id.clone();

    let _turn_guard = if let Some(ref sid) = session_id {
        Some(session_turn_lock(&state, sid).await)
    } else {
        None
    };
    purge_expired_sessions(&state).await;

    if let Some(ref sid) = session_id {
        request.messages = prepare_session_chat_messages(
            &state,
            sid,
            request.messages.clone(),
            &request_id,
            "http",
        )
        .await?;
    }

    let response = match state.agent.chat(&request).await {
        Ok(response) => response,
        Err(e) => {
            let message = e.to_string();
            if stream {
                tracing::error!(request_id = %request_id, "聊天失败（SSE error 事件）: {message}");
                let events = build_chat_sse_events(
                    &request_id,
                    session_id.as_deref(),
                    Err(message.as_str()),
                );
                return Ok(chat_sse_response(events).into_response());
            }
            return Err(AppError::Internal(message));
        }
    };

    // 如果提供了 session_id，更新 session 历史
    if let Some(ref sid) = session_id {
        let mut messages = request.messages.clone();
        messages.push(response.message.clone());
        commit_session_messages(&state, sid, messages, &request_id, "http").await?;
    }

    tracing::info!(
        request_id = %request_id,
        "聊天响应生成，状态: {:?}, 工具调用数: {}",
        response.status,
        response.tool_calls.len()
    );

    // 将 session_id 添加到响应中
    let mut response = response;
    response.session_id = session_id;

    if stream {
        let events =
            build_chat_sse_events(&request_id, response.session_id.as_deref(), Ok(&response));
        return Ok(chat_sse_response(events).into_response());
    }

    Ok(Json(response).into_response())
}

/// 会话列表项
#[derive(Debug, Serialize, Deserialize)]
struct SessionSummary {
    id: String,
    message_count: usize,
}

/// 列出会话响应
#[derive(Debug, Serialize, Deserialize)]
struct ListSessionsResponse {
    sessions: Vec<SessionSummary>,
}

/// 读取会话响应
#[derive(Debug, Serialize, Deserialize)]
struct GetSessionResponse {
    id: String,
    messages: Vec<ChatMessage>,
}

/// 创建会话响应
#[derive(Debug, Serialize, Deserialize)]
struct CreateSessionResponse {
    session_id: String,
}

/// 从权威会话存储读取列表，与 chat/delete 使用相同后端。
async fn list_sessions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListSessionsResponse>, AppError> {
    if !check_api_auth(&state, &headers) {
        tracing::warn!(
            request_id = request_id_log_value(&headers),
            "API 鉴权失败: token 不匹配或缺失"
        );
        return Err(AppError::Unauthorized);
    }

    purge_expired_sessions(&state).await;

    let mut sessions: Vec<SessionSummary> = with_sessions(&state, move |map| {
        let entries = map.list()?;
        for (id, _) in &entries {
            map.touch(id)?;
        }
        Ok(entries
            .into_iter()
            .map(|(id, rec)| SessionSummary {
                id,
                message_count: rec.messages.len(),
            })
            .collect())
    })
    .await?;
    sessions.sort_by(|a, b| a.id.cmp(&b.id));

    tracing::info!(
        request_id = request_id_log_value(&headers),
        "列出 sessions: {} 个",
        sessions.len()
    );
    Ok(Json(ListSessionsResponse { sessions }))
}

/// 从权威会话存储读取历史；不存在返回 404。
async fn get_session_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<GetSessionResponse>, AppError> {
    if !check_api_auth(&state, &headers) {
        tracing::warn!(
            request_id = request_id_log_value(&headers),
            "API 鉴权失败: token 不匹配或缺失"
        );
        return Err(AppError::Unauthorized);
    }

    purge_expired_sessions(&state).await;

    let id = session_id.clone();
    let messages = with_sessions(&state, move |map| {
        let record = map.get(&id)?;
        if record.is_some() {
            map.touch(&id)?;
        }
        Ok(record.map(|rec| rec.messages))
    })
    .await?;

    if let Some(messages) = messages {
        tracing::info!(
            request_id = request_id_log_value(&headers),
            "读取 session {}，消息数: {}",
            session_id,
            messages.len()
        );
        Ok(Json(GetSessionResponse {
            id: session_id,
            messages,
        }))
    } else {
        tracing::info!(
            request_id = request_id_log_value(&headers),
            "session 不存在: {}",
            session_id
        );
        Err(AppError::NotFound)
    }
}

/// 导出会话查询参数。默认 JSONL；`format=json` 返回整包。
#[derive(Debug, Default, Deserialize)]
struct ExportSessionQuery {
    #[serde(default)]
    format: Option<String>,
}

fn wants_json_export(format: Option<&str>) -> bool {
    format.is_some_and(|value| value.eq_ignore_ascii_case("json"))
}

/// 将消息编码为 NDJSON（每行一条，沿用现有 `ChatMessage` 字段）。
fn encode_messages_jsonl(messages: &[ChatMessage]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for message in messages {
        out.push_str(&serde_json::to_string(message)?);
        out.push('\n');
    }
    Ok(out)
}

/// 只读取出会话消息：过期先按 TTL 清理；不 touch、不摘要、不因导出而改写消息。
async fn load_session_messages_for_export(
    state: &AppState,
    session_id: &str,
) -> Result<Option<Vec<ChatMessage>>, AppError> {
    purge_expired_sessions(state).await;
    let id = session_id.to_string();
    with_sessions(state, move |store| {
        Ok(store.get(&id)?.map(|rec| rec.messages))
    })
    .await
}

/// 导出会话历史；不存在或已过期返回 404。只读，不触发摘要、不刷新 TTL。
async fn export_session_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<ExportSessionQuery>,
) -> Result<Response, AppError> {
    if !check_api_auth(&state, &headers) {
        tracing::warn!(
            request_id = request_id_log_value(&headers),
            "API 鉴权失败: token 不匹配或缺失"
        );
        return Err(AppError::Unauthorized);
    }

    let Some(messages) = load_session_messages_for_export(&state, &session_id).await? else {
        tracing::info!(
            request_id = request_id_log_value(&headers),
            "导出 session 不存在: {}",
            session_id
        );
        return Err(AppError::NotFound);
    };

    tracing::info!(
        request_id = request_id_log_value(&headers),
        "导出会话 {}，消息数: {}",
        session_id,
        messages.len()
    );

    if wants_json_export(query.format.as_deref()) {
        return Ok(Json(GetSessionResponse {
            id: session_id,
            messages,
        })
        .into_response());
    }

    let body = encode_messages_jsonl(&messages)
        .map_err(|e| AppError::Internal(format!("序列化会话导出失败: {e}")))?;
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(SESSION_EXPORT_NDJSON),
        )],
        body,
    )
        .into_response())
}

/// 导入会话查询参数。默认 NDJSON；`format=json` 或 `Content-Type: application/json` 接受整包。
#[derive(Debug, Default, Deserialize)]
struct ImportSessionQuery {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

/// JSON 导入整包：与 GET/export `format=json` 对称。
#[derive(Debug, Deserialize)]
struct ImportSessionJsonBody {
    #[serde(default)]
    id: Option<String>,
    messages: Vec<ChatMessage>,
}

fn content_type_is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

fn wants_json_import(format: Option<&str>, headers: &HeaderMap) -> bool {
    wants_json_export(format) || content_type_is_json(headers)
}

fn first_nonempty_id(id: Option<&str>) -> Option<String> {
    id.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_import_session_id(query_id: Option<&str>, body_id: Option<&str>) -> String {
    first_nonempty_id(query_id)
        .or_else(|| first_nonempty_id(body_id))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn parse_session_import_jsonl(body: &str) -> Result<Vec<ChatMessage>, String> {
    let mut messages = Vec::new();
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: ChatMessage = serde_json::from_str(trimmed).map_err(|e| {
            format!(
                "invalid_session_import: line {}: {e}",
                idx.saturating_add(1)
            )
        })?;
        messages.push(message);
    }
    Ok(messages)
}

fn parse_session_import_json(body: &str) -> Result<(Option<String>, Vec<ChatMessage>), String> {
    let parsed: ImportSessionJsonBody =
        serde_json::from_str(body.trim()).map_err(|e| format!("invalid_session_import: {e}"))?;
    Ok((parsed.id, parsed.messages))
}

fn parse_session_import_file(body: &str) -> Result<(Option<String>, Vec<ChatMessage>), String> {
    if let Ok((id, messages)) = parse_session_import_json(body) {
        return Ok((id, messages));
    }
    Ok((None, parse_session_import_jsonl(body)?))
}

/// 导入会话历史。不存在则新建；已存在默认 409，`overwrite=true` 替换消息并刷新 `last_accessed`。
/// 不调用 LLM；超长走现有硬截断/本地摘要策略。
async fn import_session_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ImportSessionQuery>,
    body: Bytes,
) -> Result<Json<GetSessionResponse>, AppError> {
    if !check_api_auth(&state, &headers) {
        tracing::warn!(
            request_id = request_id_log_value(&headers),
            "API 鉴权失败: token 不匹配或缺失"
        );
        return Err(AppError::Unauthorized);
    }

    let body_text = std::str::from_utf8(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid_session_import: {e}")))?;

    let (body_id, messages) = if wants_json_import(query.format.as_deref(), &headers) {
        parse_session_import_json(body_text).map_err(AppError::BadRequest)?
    } else {
        (
            None,
            parse_session_import_jsonl(body_text).map_err(AppError::BadRequest)?,
        )
    };

    let session_id = resolve_import_session_id(query.id.as_deref(), body_id.as_deref());
    let request_id = request_id_log_value(&headers).to_string();

    purge_expired_sessions(&state).await;

    let compacted = compact_imported_session_messages(
        messages,
        state
            .agent
            .config()
            .session
            .effective_summarize_on_overflow(),
        state.agent.config().session.effective_keep_recent(),
    )
    .await;

    let _turn_guard = session_turn_lock(&state, &session_id).await;
    let id = session_id.clone();
    let messages = compacted.clone();
    let state_copy = state.clone();
    let imported = with_sessions(&state, move |sessions| {
        let imported = sessions.import(&id, SessionRecord::new(messages), query.overwrite)?;
        if imported {
            persist_session_map(&state_copy, sessions)?;
        }
        Ok(imported)
    })
    .await?;
    if !imported {
        return Err(AppError::Conflict);
    }
    tracing::info!(request_id, session_id, "导入会话成功");

    Ok(Json(GetSessionResponse {
        id: session_id,
        messages: compacted,
    }))
}

/// 创建会话处理器
async fn create_session_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CreateSessionResponse>, AppError> {
    // API Token 鉴权检查
    if !check_api_auth(&state, &headers) {
        tracing::warn!("API 鉴权失败: token 不匹配或缺失");
        return Err(AppError::Unauthorized);
    }
    let session_id = uuid::Uuid::new_v4().to_string();

    purge_expired_sessions(&state).await;

    let id = session_id.clone();
    let state_copy = state.clone();
    with_sessions(&state, move |sessions| {
        sessions.insert(id, SessionRecord::new(Vec::new()))?;
        persist_session_map(&state_copy, sessions)
    })
    .await?;

    tracing::info!("创建新 session: {}", session_id);
    Ok(Json(CreateSessionResponse { session_id }))
}

/// 删除会话响应
#[derive(Debug, Serialize, Deserialize)]
struct DeleteSessionResponse {
    success: bool,
    message: String,
}

/// 工具信息
#[derive(Debug, Serialize, Deserialize)]
struct ToolInfo {
    name: String,
    description: String,
}

/// 工具列表响应
#[derive(Debug, Serialize, Deserialize)]
struct ToolsResponse {
    tools: Vec<ToolInfo>,
}

/// 技能信息
#[derive(Debug, Serialize, Deserialize)]
struct SkillInfo {
    name: String,
    description: String,
    path: String,
}

/// 技能列表响应
#[derive(Debug, Serialize, Deserialize)]
struct SkillsResponse {
    skills: Vec<SkillInfo>,
}

/// 技能热加载响应
#[derive(Debug, Serialize, Deserialize)]
struct SkillsReloadResponse {
    reloaded: usize,
    skills: Vec<SkillInfo>,
}

fn skill_infos(skills: &[jiaclaw::Skill]) -> Vec<SkillInfo> {
    skills
        .iter()
        .map(|skill| SkillInfo {
            name: skill.name.clone(),
            description: skill.description.clone(),
            path: skill.path.to_string_lossy().to_string(),
        })
        .collect()
}

/// 删除会话处理器
async fn delete_session_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<DeleteSessionResponse>, AppError> {
    // API Token 鉴权检查
    if !check_api_auth(&state, &headers) {
        tracing::warn!("API 鉴权失败: token 不匹配或缺失");
        return Err(AppError::Unauthorized);
    }
    purge_expired_sessions(&state).await;

    let _turn_guard = session_turn_lock(&state, &session_id).await;
    let id = session_id.clone();
    let state_copy = state.clone();
    let existed = with_sessions(&state, move |sessions| {
        let existed = sessions.remove(&id)?;
        if existed {
            persist_session_map(&state_copy, sessions)?;
        }
        Ok(existed)
    })
    .await?;

    if existed {
        tracing::info!("删除 session: {}", session_id);

        Ok(Json(DeleteSessionResponse {
            success: true,
            message: format!("会话 {session_id} 已删除"),
        }))
    } else {
        tracing::warn!("尝试删除不存在的 session: {}", session_id);
        Ok(Json(DeleteSessionResponse {
            success: false,
            message: format!("会话 {session_id} 不存在"),
        }))
    }
}

/// 工具列表处理器
async fn tools_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ToolsResponse>, AppError> {
    // API Token 鉴权检查
    if !check_api_auth(&state, &headers) {
        tracing::warn!("API 鉴权失败: token 不匹配或缺失");
        return Err(AppError::Unauthorized);
    }
    let tool_names = state.agent.tools().list();
    let tools: Vec<ToolInfo> = tool_names
        .iter()
        .filter_map(|name| {
            state.agent.tools().get(name).map(|tool| ToolInfo {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
            })
        })
        .collect();

    tracing::info!("列出工具: {} 个已注册", tools.len());
    Ok(Json(ToolsResponse { tools }))
}

/// 技能列表处理器（进程内注册表，需 reload 后才看到磁盘新技能）
async fn skills_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SkillsResponse>, AppError> {
    // API Token 鉴权检查
    if !check_api_auth(&state, &headers) {
        tracing::warn!("API 鉴权失败: token 不匹配或缺失");
        return Err(AppError::Unauthorized);
    }
    let skills = skill_infos(&state.agent.skills());
    tracing::info!("列出技能: {} 个（进程内注册表）", skills.len());
    Ok(Json(SkillsResponse { skills }))
}

/// 热加载技能：重扫工作区 `skills/` 并替换进程内注册表。失败保留旧表。
async fn reload_skills_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SkillsReloadResponse>, AppError> {
    if !check_api_auth(&state, &headers) {
        tracing::warn!("API 鉴权失败: token 不匹配或缺失");
        return Err(AppError::Unauthorized);
    }

    match state.agent.reload_skills() {
        Ok(discovered) => {
            let skills = skill_infos(&discovered);
            let reloaded = skills.len();
            tracing::info!(reloaded, "技能重载成功");
            Ok(Json(SkillsReloadResponse { reloaded, skills }))
        }
        Err(e) => {
            tracing::error!("技能重载失败，已保留旧表: {e}");
            Err(AppError::BadRequest(format!(
                "技能重载失败，已保留旧表: {e}"
            )))
        }
    }
}

fn unauthorized_hook_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"ok": false, "error": "unauthorized"})),
    )
        .into_response()
}

/// 将用户文本写入指定 session 并跑一轮 agent chat（仅管理员配置的 `/hooks/inbound` 与旧 HEARTBEAT 使用）。
async fn run_session_user_chat(
    state: &AppState,
    session_id: &str,
    user_text: &str,
    request_id: &str,
    channel_label: &str,
) -> Result<String, AppError> {
    tracing::info!(
        request_id = %request_id,
        "使用 {channel_label} session_id: {session_id}"
    );

    let _turn_guard = session_turn_lock(state, session_id).await;
    purge_expired_sessions(state).await;

    let incoming = vec![ChatMessage {
        role: MessageRole::User,
        content: user_text.to_string(),
    }];
    let request = ChatRequest {
        messages: prepare_session_chat_messages(
            state,
            session_id,
            incoming,
            request_id,
            channel_label,
        )
        .await?,
        enabled_tools: vec![],
        enabled_skills: vec![],
        auto_skills: true,
        session_id: Some(session_id.to_string()),
    };

    let response = state
        .agent
        .chat(&request)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    {
        let mut messages = request.messages.clone();
        messages.push(response.message.clone());
        commit_session_messages(state, session_id, messages, request_id, channel_label).await?;
    }

    tracing::info!(
        request_id = %request_id,
        "{channel_label} 聊天响应生成，状态: {:?}, 工具调用数: {}",
        response.status,
        response.tool_calls.len()
    );

    Ok(response.message.content)
}

/// Webhook 入站处理器
async fn hooks_inbound_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<InboundWebhookRequest>,
) -> Result<impl IntoResponse, AppError> {
    let request_id = request_id_log_value(&headers).to_string();

    tracing::info!(
        request_id = %request_id,
        "收到 webhook 入站请求: channel={}, chat_id={}, username={:?}",
        body.channel,
        body.chat_id,
        body.username
    );

    if let Some(expected) = state.webhook_secret.as_deref() {
        let provided = headers
            .get("X-Webhook-Secret")
            .and_then(|v| v.to_str().ok());
        if provided != Some(expected) {
            tracing::warn!(request_id = %request_id, "Webhook 鉴权失败: secret 不匹配");
            return Ok(unauthorized_hook_response());
        }
    }

    let session_id = format!("webhook:{}", body.chat_id);
    let reply =
        run_session_user_chat(&state, &session_id, &body.text, &request_id, "webhook").await?;

    let webhook_response = InboundWebhookResponse {
        ok: true,
        session_id,
        reply: Some(reply.clone()),
        message: Some(reply),
        error: None,
    };

    Ok((StatusCode::OK, Json(webhook_response)).into_response())
}

/// 应用错误类型
#[derive(Debug)]
enum AppError {
    ChannelConflict,
    ChannelUnavailable,
    ServiceUnavailable,
    JobConflict(String),
    Internal(String),
    Unauthorized,
    NotFound,
    BadRequest(String),
    Conflict,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            Self::ChannelConflict => (StatusCode::CONFLICT, "channel_state_conflict".to_string()),
            Self::ChannelUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "channel_unavailable".to_string(),
            ),
            Self::ServiceUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "scheduler_unavailable".to_string(),
            ),
            Self::JobConflict(message) => (StatusCode::CONFLICT, message),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()),
            Self::NotFound => (StatusCode::NOT_FOUND, "session_not_found".to_string()),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Conflict => (StatusCode::CONFLICT, "session_already_exists".to_string()),
        };

        let body = serde_json::json!({
            "error": message,
        });

        (status, Json(body)).into_response()
    }
}

/// 优雅关闭信号（SIGINT / Ctrl+C 与 Unix SIGTERM）
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("等待 Ctrl+C 信号失败");
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                tracing::error!("安装 SIGTERM 处理器失败: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("收到 SIGINT/SIGTERM，开始优雅退出");
}

/// Unix `SIGHUP` 触发与 `POST /api/skills/reload` 相同的重载路径。Windows 无此信号。
fn spawn_skill_reload_on_sighup(agent: Arc<JiaClawAgent>) -> Option<tokio::task::JoinHandle<()>> {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
            Ok(mut hangup) => Some(tokio::spawn(async move {
                loop {
                    if hangup.recv().await.is_none() {
                        tracing::info!("SIGHUP 监听结束");
                        break;
                    }
                    tracing::info!("收到 SIGHUP，开始重载技能");
                    match agent.reload_skills() {
                        Ok(skills) => {
                            tracing::info!(count = skills.len(), "SIGHUP 技能重载成功");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "SIGHUP 技能重载失败，已保留旧表");
                        }
                    }
                }
            })),
            Err(e) => {
                tracing::error!("安装 SIGHUP 处理器失败: {e}");
                None
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = agent;
        tracing::info!("当前平台不支持 SIGHUP 技能重载，请使用 POST /api/skills/reload");
        None
    }
}

fn persist_path_from_config(config: &AgentConfig) -> PathBuf {
    let configured = PathBuf::from(&config.http.persist_path);
    // Legacy defaults migrate to a sibling state directory, outside model tools.
    if config.http.persist_path == ".jiaclaw/sessions.json"
        || config.http.persist_path == ".jiaclaw/sessions.sqlite3"
    {
        return config.workspace_path.join("../state/sessions.sqlite3");
    }
    let path = if configured.is_absolute() {
        configured
    } else {
        config.workspace_path.join(configured)
    };
    if path.extension().is_some_and(|ext| ext == "json") {
        path.with_extension("sqlite3")
    } else {
        path
    }
}

fn open_configured_session_store(config: &AgentConfig) -> Result<SessionStore> {
    let path = persist_path_from_config(config);
    let workspace = config.workspace_path.canonicalize()?;
    let parent = path
        .parent()
        .context("session database requires a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let db = if path.exists() {
        path.canonicalize()?
    } else {
        parent.canonicalize()?.join(
            path.file_name()
                .context("session database requires a filename")?,
        )
    };
    if db.starts_with(&workspace) {
        anyhow::bail!("session SQLite 数据库必须放在 Agent 工作区之外，建议 persist_path = '../state/sessions.sqlite3'");
    }
    let mut store = SessionStore::open(&db)?;
    let legacy = if config.http.persist_path.ends_with(".json") {
        PathBuf::from(&config.http.persist_path)
    } else {
        PathBuf::from(".jiaclaw/sessions.json")
    };
    let legacy = if legacy.is_absolute() {
        legacy
    } else {
        config.workspace_path.join(legacy)
    };
    store.migrate_json(&legacy)?;
    Ok(store)
}

/// 从磁盘加载 sessions
#[cfg(test)]
fn load_sessions(path: &std::path::Path) -> HashMap<String, Vec<ChatMessage>> {
    if !path.exists() {
        tracing::info!("Session 文件不存在，从空 map 开始");
        return HashMap::new();
    }

    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<HashMap<String, Vec<ChatMessage>>>(&content) {
            Ok(sessions) => {
                tracing::info!("成功加载 {} 个 sessions", sessions.len());
                sessions
            }
            Err(e) => {
                tracing::warn!("Session 文件损坏，无法解析: {}。从空 map 开始", e);
                HashMap::new()
            }
        },
        Err(e) => {
            tracing::warn!("无法读取 session 文件: {}。从空 map 开始", e);
            HashMap::new()
        }
    }
}

/// 保存 sessions 到磁盘（原子写入）
fn save_sessions(path: &PathBuf, sessions: &HashMap<String, Vec<ChatMessage>>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(sessions).context("序列化 sessions 失败")?;

    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, json)
        .with_context(|| format!("无法写入临时文件: {}", temp_path.display()))?;

    std::fs::rename(&temp_path, path).with_context(|| {
        format!(
            "无法重命名文件: {} -> {}",
            temp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

async fn chat_command(
    config_path: Option<PathBuf>,
    message: Option<&str>,
    skills: Vec<String>,
    session_id: Option<String>,
    no_auto_skill: bool,
) -> Result<()> {
    let config = if let Some(path) = config_path {
        let path_str = path.to_string_lossy();
        if path_str.ends_with(".toml") {
            AgentConfig::from_toml_file(&path)?
        } else if path_str.ends_with(".json") {
            AgentConfig::from_json_file(&path)?
        } else {
            AgentConfig::from_toml_file(&path).or_else(|_| AgentConfig::from_json_file(&path))?
        }
    } else {
        AgentConfig::default()
    };

    tracing::info!("使用 Agent 配置: {}", config.name);

    // 检查工作空间是否存在
    if !config.workspace_path.exists() {
        tracing::warn!("工作空间不存在，使用默认配置");
        println!("\n💡 提示: 运行 'jiaclaw init' 创建工作空间");
    }

    // 创建并使用 agent
    validate_host_provider(&config)?;
    let agent = JiaClawAgent::connect(config)
        .await
        .context("创建 JiaClawAgent 失败")?;

    // 如果提供了消息，执行单次聊天
    if let Some(msg) = message {
        tracing::info!("运行单次聊天");
        single_chat(&agent, msg, &skills, session_id, no_auto_skill).await?;
    } else {
        // 否则进入 REPL 模式
        tracing::info!("进入 REPL 模式");
        repl_chat(&agent, &skills, session_id, no_auto_skill).await?;
    }

    Ok(())
}

/// 单次聊天模式
async fn single_chat(
    agent: &JiaClawAgent,
    message: &str,
    skills: &[String],
    session_id: Option<String>,
    no_auto_skill: bool,
) -> Result<()> {
    let mut store = if session_id.is_some() && agent.config().http.persist {
        Some(open_configured_session_store(agent.config())?)
    } else {
        None
    };
    let mut messages = match (&store, &session_id) {
        (Some(store), Some(id)) => store.get(id)?.map_or_else(Vec::new, |rec| rec.messages),
        _ => Vec::new(),
    };
    messages.push(ChatMessage {
        role: MessageRole::User,
        content: message.to_string(),
    });
    let request = ChatRequest {
        messages: agent.compact_session_messages(messages).await,
        enabled_tools: vec![],
        enabled_skills: skills.to_vec(),
        auto_skills: !no_auto_skill,
        session_id: session_id.clone(),
    };

    tracing::info!("用户消息: {}", message);

    let response = agent.chat(&request).await.context("聊天请求失败")?;

    if let (Some(store), Some(id)) = (&mut store, &session_id) {
        let mut history = request.messages.clone();
        history.push(response.message.clone());
        store.insert(id.clone(), SessionRecord::new(history))?;
    }

    // 显示工具调用（如果有）
    if !response.tool_calls.is_empty() {
        println!("\n🔧 工具调用:");
        for tool_call in &response.tool_calls {
            println!("   • {}", tool_call.tool_name);
            if let Some(ref result) = tool_call.result {
                if let Some(result_str) = result.as_str() {
                    // 结果是字符串，直接显示
                    println!(
                        "     结果: {}",
                        result_str
                            .lines()
                            .take(3)
                            .collect::<Vec<_>>()
                            .join("\n     ")
                    );
                    if result_str.lines().count() > 3 {
                        println!("     ...");
                    }
                } else {
                    // 结果是其他 JSON，格式化显示
                    println!(
                        "     结果: {}",
                        serde_json::to_string_pretty(result).unwrap_or_default()
                    );
                }
            }
        }
        println!();
    }

    println!("\n助手回复:");
    println!("{}", response.message.content);
    println!("\n状态: {:?}", response.status);

    if let Some(sid) = session_id {
        println!("会话 ID: {sid}");
    }

    Ok(())
}

/// REPL 多轮对话模式
async fn repl_chat(
    agent: &JiaClawAgent,
    skills: &[String],
    session_id: Option<String>,
    no_auto_skill: bool,
) -> Result<()> {
    use std::io::{self, Write};

    println!("\n🦀 JiaClaw REPL 模式");
    println!("输入消息开始对话，输入 'exit'、'quit' 或按 Ctrl+D 退出\n");

    if !skills.is_empty() {
        println!("✨ 启用的技能: {}", skills.join(", "));
    }
    if no_auto_skill {
        println!("⚠️  技能自动激活已禁用");
    }
    if let Some(ref sid) = session_id {
        println!("📝 会话 ID: {sid}");
    }
    println!();

    // 维护会话历史
    let mut store = if session_id.is_some() && agent.config().http.persist {
        Some(open_configured_session_store(agent.config())?)
    } else {
        None
    };
    let mut history: Vec<ChatMessage> = match (&store, &session_id) {
        (Some(store), Some(id)) => store.get(id)?.map_or_else(Vec::new, |rec| rec.messages),
        _ => Vec::new(),
    };
    let enabled_skills = skills.to_vec();

    loop {
        // 显示提示符
        print!("👤 > ");
        io::stdout().flush()?;

        // 读取用户输入
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(0) => {
                // EOF (Ctrl+D)
                println!("\n👋 再见！");
                break;
            }
            Ok(_) => {
                let trimmed = input.trim();

                // 检查退出命令
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "exit" || trimmed == "quit" {
                    println!("👋 再见！");
                    break;
                }

                // 添加用户消息到历史
                let user_message = ChatMessage {
                    role: MessageRole::User,
                    content: trimmed.to_string(),
                };
                history.push(user_message);

                history = agent.compact_session_messages(history).await;

                // 构建请求
                let request = ChatRequest {
                    messages: history.clone(),
                    enabled_tools: vec![],
                    enabled_skills: enabled_skills.clone(),
                    auto_skills: !no_auto_skill,
                    session_id: session_id.clone(),
                };

                // 调用 agent
                match agent.chat(&request).await {
                    Ok(response) => {
                        history.push(response.message.clone());
                        if let (Some(store), Some(id)) = (&mut store, &session_id) {
                            store.insert(id.clone(), SessionRecord::new(history.clone()))?;
                        }
                        // 显示工具调用（如果有）
                        if !response.tool_calls.is_empty() {
                            println!("\n🔧 工具调用:");
                            for tool_call in &response.tool_calls {
                                println!("   • {}", tool_call.tool_name);
                            }
                            println!();
                        }

                        // 显示助手回复
                        println!("🤖 {}\n", response.message.content);

                        // 添加助手消息到历史
                    }
                    Err(e) => {
                        eprintln!("❌ 错误: {e}\n");
                        // 失败时从历史中移除刚才的用户消息
                        history.pop();
                    }
                }
            }
            Err(e) => {
                eprintln!("❌ 读取输入失败: {e}");
                break;
            }
        }
    }

    Ok(())
}

fn version_command() {
    println!("JiaClaw v{}", env!("CARGO_PKG_VERSION"));
    println!("基于 StateKnot 框架构建");
    println!("许可证: Apache-2.0 OR MIT");
    println!("仓库: https://github.com/jiawenyao401/JiaClaw");
}

#[allow(clippy::too_many_lines)]
fn skills_command(config_path: Option<PathBuf>, verbose: bool) -> Result<()> {
    let config = if let Some(path) = config_path {
        let path_str = path.to_string_lossy();
        if path_str.ends_with(".toml") {
            AgentConfig::from_toml_file(&path)?
        } else if path_str.ends_with(".json") {
            AgentConfig::from_json_file(&path)?
        } else {
            AgentConfig::from_toml_file(&path).or_else(|_| AgentConfig::from_json_file(&path))?
        }
    } else {
        AgentConfig::default()
    };

    println!("🎯 JiaClaw 技能列表\n");
    println!("📁 工作空间: {}\n", config.workspace_path.display());

    let skills_dir = config.workspace_path.join("skills");

    if !skills_dir.exists() {
        println!("❌ 技能目录不存在: {}", skills_dir.display());
        println!("\n💡 运行 'jiaclaw init' 创建工作空间和示例技能");
        return Ok(());
    }

    let discovery = jiaclaw::SkillDiscovery::new(&config.workspace_path);

    match discovery.discover() {
        Ok(skills) => {
            if skills.is_empty() {
                println!("⚠️  未发现任何技能");
                println!(
                    "\n💡 在 {} 中创建技能目录和 SKILL.md 文件",
                    skills_dir.display()
                );
                println!("   每个技能应包含:");
                println!("   • YAML frontmatter（name, description, triggers）");
                println!("   • Markdown 内容（技能说明）");
            } else {
                println!("✅ 发现 {} 个技能:\n", skills.len());

                for (i, skill) in skills.iter().enumerate() {
                    println!("{}. {}", i + 1, skill.name);
                    println!("   描述: {}", skill.description);

                    if skill.triggers.is_empty() {
                        println!("   触发词: 无");
                    } else {
                        println!("   触发词: {}", skill.triggers.join(", "));
                    }

                    println!("   路径: {}", skill.path.display());

                    if verbose {
                        println!("\n   内容预览:");
                        let preview = skill
                            .content
                            .lines()
                            .take(5)
                            .collect::<Vec<_>>()
                            .join("\n   ");
                        println!("   {preview}");
                        if skill.content.lines().count() > 5 {
                            println!("   ...");
                        }
                    }

                    println!();
                }

                println!("💡 使用提示:");
                println!("   • 显式启用: 在 ChatRequest 的 enabled_skills 字段中指定");
                println!("   • 自动激活: 当用户消息包含触发词时自动启用");
                println!("   • 详细模式: 使用 --verbose 查看技能完整内容");
                println!("   • 热加载运行中的 serve: POST /api/skills/reload 或 Unix SIGHUP");
            }
        }
        Err(e) => {
            println!("❌ 技能发现失败: {e}");
            return Err(e.into());
        }
    }

    Ok(())
}

fn skills_reload_command(config_path: Option<PathBuf>) -> Result<()> {
    let config = load_agent_config(config_path)?;
    let skills_dir = config.workspace_path.join("skills");

    println!("🔄 JiaClaw 技能扫描（CLI）\n");
    println!("📁 工作空间: {}\n", config.workspace_path.display());

    if !skills_dir.exists() {
        println!("⚠️  技能目录不存在: {}", skills_dir.display());
        println!("   扫描结果：0 个技能（与空目录相同）");
        print_skills_reload_serve_hint();
        return Ok(());
    }

    match jiaclaw::SkillDiscovery::new(&config.workspace_path).discover_strict() {
        Ok(skills) => {
            println!("✅ 扫描成功，将加载 {} 个技能:\n", skills.len());
            for skill in &skills {
                println!("   • {}", skill.name);
            }
            if skills.is_empty() {
                println!("   （目录为空或没有 SKILL.md）");
            }
            println!();
            print_skills_reload_serve_hint();
            Ok(())
        }
        Err(e) => {
            println!("❌ 扫描失败: {e}");
            println!("   若对运行中的 serve 执行重载，进程内旧表会保留。");
            print_skills_reload_serve_hint();
            Err(e.into())
        }
    }
}

fn print_skills_reload_serve_hint() {
    println!("ℹ️  本命令只扫描当前工作区，不会通知已运行的 `jiaclaw serve`。");
    println!("   运行中的服务请使用: POST /api/skills/reload");
    println!("   Unix 也可向 serve 进程发送 SIGHUP；Windows 仅支持 HTTP。");
}

fn load_agent_config(config_path: Option<PathBuf>) -> Result<AgentConfig> {
    if let Some(path) = config_path {
        let path_str = path.to_string_lossy();
        if path_str.ends_with(".toml") {
            Ok(AgentConfig::from_toml_file(&path)?)
        } else if path_str.ends_with(".json") {
            Ok(AgentConfig::from_json_file(&path)?)
        } else {
            Ok(AgentConfig::from_toml_file(&path)
                .or_else(|_| AgentConfig::from_json_file(&path))?)
        }
    } else {
        Ok(AgentConfig::default())
    }
}

/// 从落盘 session store 只读导出 JSONL。不触发摘要、不改写文件。
fn session_export_command(
    config_path: Option<PathBuf>,
    session_id: &str,
    output: Option<&std::path::Path>,
) -> Result<()> {
    let config = load_agent_config(config_path)?;
    let store = open_configured_session_store(&config)?;
    let messages = store
        .get(session_id)?
        .ok_or_else(|| anyhow::anyhow!("session_not_found"))?
        .messages;
    write_session_jsonl(&messages, output)
}

#[cfg(test)]
fn export_session_from_persist_file(
    persist_path: &std::path::Path,
    session_id: &str,
    output: Option<&std::path::Path>,
) -> Result<()> {
    let sessions = load_sessions(persist_path);
    let Some(messages) = sessions.get(session_id) else {
        anyhow::bail!("session_not_found");
    };
    write_session_jsonl(messages, output)
}

fn write_session_jsonl(messages: &[ChatMessage], output: Option<&std::path::Path>) -> Result<()> {
    let body = encode_messages_jsonl(messages).context("序列化会话导出失败")?;
    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("无法创建目录: {}", parent.display()))?;
            }
        }
        std::fs::write(path, body).with_context(|| format!("无法写入 {}", path.display()))?;
    } else {
        print!("{body}");
    }
    Ok(())
}

/// 从 JSONL/JSON 文件导入到落盘 session store。不调用 LLM。
async fn session_import_command(
    config_path: Option<PathBuf>,
    file: &std::path::Path,
    id: Option<&str>,
    overwrite: bool,
) -> Result<()> {
    let config = load_agent_config(config_path)?;
    let body = std::fs::read_to_string(file)?;
    let (body_id, messages) = parse_session_import_file(&body).map_err(anyhow::Error::msg)?;
    let session_id = resolve_import_session_id(id, body_id.as_deref());
    let messages = compact_imported_session_messages(
        messages,
        config.session.effective_summarize_on_overflow(),
        config.session.effective_keep_recent(),
    )
    .await;
    let mut store = open_configured_session_store(&config)?;
    if !store.import(&session_id, SessionRecord::new(messages), overwrite)? {
        anyhow::bail!("session_already_exists");
    }
    println!("✅ 已导入 session {session_id}");
    Ok(())
}

#[cfg(test)]
async fn import_session_into_persist_file(
    persist_path: &std::path::Path,
    file: &std::path::Path,
    id: Option<&str>,
    overwrite: bool,
    summarize_on_overflow: bool,
    keep_recent: usize,
) -> Result<String> {
    let body =
        std::fs::read_to_string(file).with_context(|| format!("无法读取 {}", file.display()))?;
    let (body_id, messages) = parse_session_import_file(&body).map_err(anyhow::Error::msg)?;
    let session_id = resolve_import_session_id(id, body_id.as_deref());
    let compacted =
        compact_imported_session_messages(messages, summarize_on_overflow, keep_recent).await;

    let mut sessions = load_sessions(persist_path);
    if sessions.contains_key(&session_id) && !overwrite {
        anyhow::bail!("session_already_exists");
    }
    sessions.insert(session_id.clone(), compacted);
    save_sessions(&persist_path.to_path_buf(), &sessions)?;
    Ok(session_id)
}

fn memory_show_command(config_path: Option<PathBuf>) -> Result<()> {
    let config = load_agent_config(config_path)?;
    let status = inspect_memory_file(&config.workspace_path, &config.memory.path)
        .context("无法解析 MEMORY 路径")?;

    println!("🧠 长期记忆\n");
    println!("   工作空间: {}", config.workspace_path.display());
    println!("   约定路径: {}", config.memory.path);
    println!("   文件:     {}", status.path.display());

    if !status.exists {
        println!("   状态:     ⚠️  不存在 (0 bytes)");
        println!("\n💡 文件缺失时对话仍可进行，不会报错。");
        println!("   可手动创建该文件，或让 Agent 调用 memory_write / memory_append。");
        return Ok(());
    }

    println!("   状态:     ✅ 存在 ({} bytes)", status.size_bytes);

    let content = std::fs::read_to_string(&status.path)
        .with_context(|| format!("无法读取 {}", status.path.display()))?;

    if content.trim().is_empty() {
        println!("\n（文件为空，不会注入系统提示）");
        return Ok(());
    }

    println!("\n----- 内容 -----\n{content}");
    if !content.ends_with('\n') {
        println!();
    }
    println!("----- 结束 -----");
    Ok(())
}

#[derive(Clone, Copy)]
enum IdentityShowKind {
    Soul,
    User,
}

fn identity_show_command(config_path: Option<PathBuf>, kind: IdentityShowKind) -> Result<()> {
    let config = load_agent_config(config_path)?;
    let (title, rel_path, tool_hint) = match kind {
        IdentityShowKind::Soul => (
            "✨ 人格（SOUL）",
            config.identity.soul_path.as_str(),
            "soul_write",
        ),
        IdentityShowKind::User => (
            "👤 用户画像（USER）",
            config.identity.user_path.as_str(),
            "user_write",
        ),
    };
    let status =
        inspect_identity_file(&config.workspace_path, rel_path).context("无法解析身份文件路径")?;

    println!("{title}\n");
    println!("   工作空间: {}", config.workspace_path.display());
    println!("   约定路径: {rel_path}");
    println!("   文件:     {}", status.path.display());

    if !status.exists {
        println!("   状态:     ⚠️  不存在 (0 bytes)");
        println!("\n💡 文件缺失时对话仍可进行，不会报错。");
        println!("   可手动创建该文件，或让 Agent 调用 {tool_hint}。");
        return Ok(());
    }

    println!("   状态:     ✅ 存在 ({} bytes)", status.size_bytes);

    let content = std::fs::read_to_string(&status.path)
        .with_context(|| format!("无法读取 {}", status.path.display()))?;

    if content.trim().is_empty() {
        println!("\n（文件为空，不会注入系统提示）");
        return Ok(());
    }

    println!("\n----- 内容 -----\n{content}");
    if !content.ends_with('\n') {
        println!();
    }
    println!("----- 结束 -----");
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn doctor_command(config_path: Option<PathBuf>) -> Result<()> {
    println!("🔍 JiaClaw 配置检查\n");

    let config = if let Some(path) = config_path {
        let path_str = path.to_string_lossy();
        if path_str.ends_with(".toml") {
            AgentConfig::from_toml_file(&path)?
        } else if path_str.ends_with(".json") {
            AgentConfig::from_json_file(&path)?
        } else {
            AgentConfig::from_toml_file(&path).or_else(|_| AgentConfig::from_json_file(&path))?
        }
    } else {
        AgentConfig::default()
    };

    // 1. 检查工作空间
    println!("📁 工作空间检查");
    println!("   路径: {}", config.workspace_path.display());

    let mut workspace_ok = false;
    let mut workspace_file_count = 0;
    let mut skills_count = 0;

    if config.workspace_path.exists() {
        println!("   状态: ✅ 存在");

        match Workspace::load(&config.workspace_path) {
            Ok(workspace) => {
                let mut files = Vec::new();
                if workspace.agents.is_some() {
                    files.push("AGENTS.md");
                    workspace_file_count += 1;
                }
                if workspace.soul.is_some() {
                    files.push("SOUL.md");
                    workspace_file_count += 1;
                }
                if workspace.user.is_some() {
                    files.push("USER.md");
                    workspace_file_count += 1;
                }
                if workspace.memory.is_some() {
                    files.push("MEMORY.md");
                    workspace_file_count += 1;
                }

                if files.is_empty() {
                    println!("   ⚠️  没有找到工作空间文件");
                    println!("   💡 运行 'jiaclaw init' 创建默认文件");
                } else {
                    println!("   文件: {} 个已加载 ({})", files.len(), files.join(", "));
                    workspace_ok = true;
                }

                match inspect_memory_file(&config.workspace_path, &config.memory.path) {
                    Ok(status) => {
                        println!(
                            "   MEMORY: {} （配置路径: {}）",
                            status.path.display(),
                            config.memory.path
                        );
                        if status.exists {
                            println!("           ✅ 存在 ({} bytes)", status.size_bytes);
                        } else {
                            println!("           ⚠️  不存在");
                            println!("           💡 可手动创建，或在对话中使用 memory_write / memory_append 写入");
                        }
                    }
                    Err(e) => {
                        println!("   MEMORY: ❌ 无法解析约定路径: {e}");
                    }
                }

                match inspect_identity_file(&config.workspace_path, &config.identity.soul_path) {
                    Ok(status) => {
                        println!(
                            "   SOUL:   {} （配置路径: {}）",
                            status.path.display(),
                            config.identity.soul_path
                        );
                        if status.exists {
                            println!("           ✅ 存在 ({} bytes)", status.size_bytes);
                        } else {
                            println!("           ⚠️  不存在");
                            println!("           💡 可手动创建，或在对话中使用 soul_write 写入");
                        }
                    }
                    Err(e) => {
                        println!("   SOUL:   ❌ 无法解析约定路径: {e}");
                    }
                }

                match inspect_identity_file(&config.workspace_path, &config.identity.user_path) {
                    Ok(status) => {
                        println!(
                            "   USER:   {} （配置路径: {}）",
                            status.path.display(),
                            config.identity.user_path
                        );
                        if status.exists {
                            println!("           ✅ 存在 ({} bytes)", status.size_bytes);
                        } else {
                            println!("           ⚠️  不存在");
                            println!("           💡 可手动创建，或在对话中使用 user_write 写入");
                        }
                    }
                    Err(e) => {
                        println!("   USER:   ❌ 无法解析约定路径: {e}");
                    }
                }

                match inspect_heartbeat_file(&config.workspace_path, &config.heartbeat.path) {
                    Ok(status) => {
                        println!(
                            "   HEARTBEAT: {} （配置路径: {}）",
                            status.path.display(),
                            config.heartbeat.path
                        );
                        if status.exists {
                            println!("           ✅ 存在 ({} bytes)", status.size_bytes);
                        } else {
                            println!("           ⚠️  不存在");
                            println!(
                                "           💡 启用 [heartbeat] 后，缺失或空文件会跳过本轮（不退出）"
                            );
                        }
                    }
                    Err(e) => {
                        println!("   HEARTBEAT: ❌ 无法解析约定路径: {e}");
                    }
                }

                // 检查技能
                let skills_dir = config.workspace_path.join("skills");
                if skills_dir.exists() {
                    let discovery = jiaclaw::SkillDiscovery::new(&config.workspace_path);
                    match discovery.discover() {
                        Ok(skills) => {
                            skills_count = skills.len();
                            if skills.is_empty() {
                                println!("   技能: 0 个");
                            } else {
                                println!("   技能: {} 个发现", skills.len());
                                for skill in &skills {
                                    println!("         • {}", skill.name);
                                }
                            }
                        }
                        Err(e) => {
                            println!("   ⚠️  技能发现失败: {e}");
                        }
                    }
                } else {
                    println!("   技能: 目录不存在");
                }
            }
            Err(e) => {
                println!("   ⚠️  加载工作空间失败: {e}");
            }
        }
    } else {
        println!("   状态: ❌ 不存在");
        println!("   💡 运行 'jiaclaw init' 创建工作空间");
    }

    // 2. 检查工具系统
    println!("\n🔧 工具系统");
    let tools_count = match JiaClawAgent::connect(config.clone()).await {
        Ok(agent) => {
            let tool_list = agent.tools().list();
            let count = tool_list.len();
            println!("   已注册工具: {count} 个");
            for tool_name in tool_list {
                if let Some(tool) = agent.tools().get(tool_name) {
                    println!("      • {}: {}", tool.name(), tool.description());
                }
            }
            count
        }
        Err(e) => {
            println!("   ⚠️  无法初始化 Agent: {e}");
            0
        }
    };

    let tool_timeout_secs = config.effective_tool_timeout_secs();
    if let Some(secs) = tool_timeout_secs {
        println!(
            "   工具超时: ✅ 已启用（每调用 {secs} 秒，通过 {}）",
            tool_timeout_config_source()
        );
    } else {
        println!("   工具超时: ⚠️  未启用（不限制）");
        println!("   💡 设置环境变量: export JIACLAW_TOOL_TIMEOUT_SECS=30");
    }

    println!(
        "   工具循环上限: {}（范围 {}–{}；可用 JIACLAW_MAX_TOOL_ITERATIONS 覆盖）",
        max_tool_iterations_status_line(&config),
        MIN_MAX_TOOL_ITERATIONS,
        MAX_MAX_TOOL_ITERATIONS
    );

    let web_search_lines = web_search_status_lines(&config);
    if config.tools.web_search.enabled
        && config.tools.web_search.effective_brave_api_key().is_some()
    {
        println!("   web_search: ✅ {}", web_search_lines[0]);
    } else {
        println!("   web_search: ⚠️  {}", web_search_lines[0]);
        for hint in web_search_lines.iter().skip(1) {
            println!("   {hint}");
        }
    }

    if config.tools.web_fetch.enabled {
        println!("   web_fetch: ✅ {}", web_fetch_status_line(&config));
    } else {
        println!("   web_fetch: ⚠️  {}", web_fetch_status_line(&config));
    }

    if config.tools.memory_search.enabled {
        println!(
            "   memory_search: ✅ {}",
            memory_search_status_line(&config)
        );
    } else {
        println!(
            "   memory_search: ⚠️  {}",
            memory_search_status_line(&config)
        );
    }

    if config.tools.memory_write.enabled {
        println!("   memory_write: ✅ {}", memory_write_status_line(&config));
    } else {
        println!("   memory_write: ⚠️  {}", memory_write_status_line(&config));
    }

    if config.tools.read_file.enabled {
        println!("   read_file: ✅ {}", read_file_status_line(&config));
    } else {
        println!("   read_file: ⚠️  {}", read_file_status_line(&config));
    }

    if config.tools.list_dir.enabled {
        println!("   list_dir: ✅ {}", list_dir_status_line(&config));
    } else {
        println!("   list_dir: ⚠️  {}", list_dir_status_line(&config));
    }

    if config.tools.write_file.enabled {
        println!("   write_file: ✅ {}", write_file_status_line(&config));
    } else {
        println!("   write_file: ⚠️  {}", write_file_status_line(&config));
    }

    if config.tools.delete_file.enabled {
        println!("   delete_file: ✅ {}", delete_file_status_line(&config));
    } else {
        println!("   delete_file: ⚠️  {}", delete_file_status_line(&config));
    }

    if config.tools.str_replace.enabled {
        println!("   str_replace: ✅ {}", str_replace_status_line(&config));
    } else {
        println!("   str_replace: ⚠️  {}", str_replace_status_line(&config));
    }

    if config.tools.grep.enabled {
        println!("   grep: ✅ {}", grep_status_line(&config));
    } else {
        println!("   grep: ⚠️  {}", grep_status_line(&config));
    }

    if config.tools.glob.enabled {
        println!("   glob: ✅ {}", glob_status_line(&config));
    } else {
        println!("   glob: ⚠️  {}", glob_status_line(&config));
    }

    if config.tools.mkdir.enabled {
        println!("   mkdir: ✅ {}", mkdir_status_line(&config));
    } else {
        println!("   mkdir: ⚠️  {}", mkdir_status_line(&config));
    }

    if config.tools.r#move.enabled {
        println!("   move: ✅ {}", move_status_line(&config));
    } else {
        println!("   move: ⚠️  {}", move_status_line(&config));
    }

    // 3. 检查提供商配置
    println!("\n🔌 提供商配置");
    println!("   类型: {}", config.provider.provider_type);
    println!("   模型: {}", config.provider.model);
    println!("   Base URL: {}", config.provider.base_url);

    // 检查 API key
    let env_key = std::env::var("JIACLAW_API_KEY").ok();
    let has_key = config.provider.api_key.is_some() || env_key.is_some();

    if has_key {
        println!("   API Key: ✅ 已配置");

        if config.provider.provider_type == "brokerrouter" {
            println!("\n   🔍 Brokerrouter 连接测试");
            println!("      注意: 完整的连接测试需要有效的虚拟密钥");
            println!("      当前仅进行配置验证");

            // 简单的 URL 格式检查
            if config.provider.base_url.starts_with("http://")
                || config.provider.base_url.starts_with("https://")
            {
                println!("      Base URL: ✅ 格式有效");
            } else {
                println!("      Base URL: ⚠️  格式可能无效（应以 http:// 或 https:// 开头）");
            }

            // 检查虚拟密钥格式
            let key = config.provider.api_key.as_deref().or(env_key.as_deref());
            if let Some(k) = key {
                if k.starts_with("brk_") {
                    println!("      Virtual Key: ✅ 格式正确（brk_ 前缀）");
                } else {
                    println!("      Virtual Key: ⚠️  格式可能不正确（应以 brk_ 开头）");
                }
            }
        }
    } else {
        println!("   API Key: ⚠️  未配置");
        println!("   💡 将使用存根模式（演示功能）");
        println!("   💡 设置环境变量: export JIACLAW_API_KEY=your-key");
    }

    // 4. HTTP 配置检查
    println!("\n🌐 HTTP 配置");
    println!("   绑定地址: {}", config.http.bind);
    println!(
        "   日志格式: {}（不打印日志正文或 secret）",
        logging_status_line(&config.logging)
    );

    // 检查 API token（环境变量优先）
    let api_token = std::env::var("JIACLAW_API_TOKEN")
        .ok()
        .or(config.http.api_token.clone());

    if api_token.is_some() {
        println!(
            "   API 鉴权: ✅ 已启用（通过 {}）",
            if std::env::var("JIACLAW_API_TOKEN").is_ok() {
                "环境变量 JIACLAW_API_TOKEN"
            } else {
                "配置文件"
            }
        );
    } else {
        println!("   API 鉴权: ⚠️  未启用（本地开发友好）");
        println!("   💡 生产环境建议设置: export JIACLAW_API_TOKEN=your-token");
    }

    // 检查 webhook secret（环境变量优先）
    let webhook_secret = std::env::var("JIACLAW_WEBHOOK_SECRET")
        .ok()
        .or(config.http.webhook_secret.clone())
        .filter(|value| !value.trim().is_empty());

    if webhook_secret.is_some() {
        println!(
            "   Webhook 鉴权: ✅ 已启用（通过 {}）",
            if std::env::var("JIACLAW_WEBHOOK_SECRET").is_ok() {
                "环境变量 JIACLAW_WEBHOOK_SECRET"
            } else {
                "配置文件"
            }
        );
    } else {
        println!("   Webhook 鉴权: ⚠️  未启用");
        println!("   💡 设置环境变量: export JIACLAW_WEBHOOK_SECRET=your-secret");
    }

    // 检查 Telegram secret token（环境变量优先）
    let telegram_secret = std::env::var("JIACLAW_TELEGRAM_SECRET")
        .ok()
        .or(config.http.telegram_secret.clone())
        .filter(|value| !value.trim().is_empty());

    if telegram_secret.is_some() {
        println!(
            "   Telegram 鉴权: ✅ 已启用（通过 {}）",
            if std::env::var("JIACLAW_TELEGRAM_SECRET").is_ok() {
                "环境变量 JIACLAW_TELEGRAM_SECRET"
            } else {
                "配置文件"
            }
        );
    } else {
        println!("   Telegram 鉴权: ⚠️  未启用");
        println!("   💡 设置环境变量: export JIACLAW_TELEGRAM_SECRET=your-secret-token");
    }

    // 检查 Telegram Bot API token（环境变量优先，不打印明文）
    let telegram_bot_token = config.http.effective_telegram_bot_token();
    if telegram_bot_token.is_some() {
        println!(
            "   Telegram Bot Token: ✅ 已配置（通过 {}，明文不打印）",
            telegram_token_config_source()
        );
    } else {
        println!("   Telegram Bot Token: ⚠️  未配置（启用 Telegram 还需显式授权策略）");
        println!("   💡 设置环境变量: export JIACLAW_TELEGRAM_BOT_TOKEN=your-bot-token");
    }

    let slack_signing_secret = config.http.effective_slack_signing_secret();
    if slack_signing_secret.is_some() {
        println!(
            "   Slack 签名校验: ✅ 已启用（通过 {}）",
            slack_signing_secret_config_source()
        );
    } else {
        println!("   Slack 签名校验: ⚠️  未启用");
        println!("   💡 设置环境变量: export JIACLAW_SLACK_SIGNING_SECRET=your-signing-secret");
    }

    let slack_bot_token = config.http.effective_slack_bot_token();
    if slack_bot_token.is_some() {
        println!(
            "   Slack Bot Token: ✅ 已配置（通过 {}，明文不打印）",
            slack_token_config_source()
        );
    } else {
        println!("   Slack Bot Token: ⚠️  未配置（启用 Slack 还需显式授权策略）");
        println!("   💡 设置环境变量: export JIACLAW_SLACK_BOT_TOKEN=xoxb-your-bot-token");
    }

    let discord_public_key = config.http.effective_discord_public_key();
    if discord_public_key.is_some() {
        println!(
            "   Discord 签名校验: ✅ 已启用（通过 {}）",
            discord_public_key_config_source()
        );
    } else {
        println!("   Discord 签名校验: ⚠️  未启用");
        println!("   💡 设置环境变量: export JIACLAW_DISCORD_PUBLIC_KEY=your-hex-public-key");
    }

    let discord_bot_token = config.http.effective_discord_bot_token();
    if discord_bot_token.is_some() {
        println!(
            "   Discord Bot Token: ✅ 已配置（通过 {}，明文不打印）",
            discord_token_config_source()
        );
    } else {
        println!("   Discord Bot Token: 无需配置；interaction 回复使用加密短期令牌");
        println!("   💡 设置环境变量: export JIACLAW_DISCORD_BOT_TOKEN=your-bot-token");
    }

    let rate_limit_per_minute = config.http.effective_rate_limit_per_minute();
    if let Some(limit) = rate_limit_per_minute {
        println!(
            "   HTTP 限流: ✅ 已启用（{limit} 次/分钟，通过 {}；受保护路径带 X-RateLimit-*，429 另带 Retry-After）",
            rate_limit_config_source()
        );
    } else {
        println!("   HTTP 限流: ⚠️  未启用");
        println!("   💡 设置环境变量: export JIACLAW_RATE_LIMIT_PER_MINUTE=60");
    }

    let max_body_bytes = config.http.effective_max_body_bytes();
    println!(
        "   HTTP 请求体上限: {max_body_bytes} 字节（超限 413，通过 {}；GET /health 与 GET /metrics 不检查）",
        max_body_bytes_config_source()
    );

    let metrics_public = config.http.effective_metrics_public();
    if metrics_public {
        println!("   Metrics: ✅ 公开（GET /metrics 无需鉴权，不计入限流）");
    } else {
        println!(
            "   Metrics: 🔒 需 API 鉴权（与 /api/* 相同，通过 {}，不计入限流）",
            metrics_auth_config_source()
        );
        println!(
            "   💡 默认公开以便 scrape；生产若暴露公网可设 [http] metrics_public = false 或 JIACLAW_METRICS_REQUIRE_AUTH=1"
        );
    }

    let session_ttl_secs = config.http.effective_session_ttl_secs();
    if let Some(ttl) = session_ttl_secs {
        println!(
            "   Session TTL: ✅ 已启用（闲置 {ttl} 秒，通过 {}）",
            session_ttl_config_source()
        );
    } else {
        println!("   Session TTL: ⚠️  未启用");
        println!("   💡 设置环境变量: export JIACLAW_SESSION_TTL_SECS=3600");
    }

    let shutdown_timeout_secs = config.http.effective_shutdown_timeout_secs();
    println!(
        "   优雅退出宽限期: {shutdown_timeout_secs} 秒（SIGINT/SIGTERM，通过 {}）",
        shutdown_timeout_config_source()
    );
    println!("   技能热加载: POST /api/skills/reload；Unix SIGHUP（Windows 仅 HTTP）");

    if config.session.effective_summarize_on_overflow() {
        println!(
            "   Session 摘要压缩: ✅ {}",
            session_summarize_status_line(&config)
        );
    } else {
        println!(
            "   Session 摘要压缩: ⚠️  {}",
            session_summarize_status_line(&config)
        );
        println!(
            "   💡 设置 [session] summarize_on_overflow = true，或 export JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW=1"
        );
    }

    let heartbeat_interval_secs = config.heartbeat.effective_interval_secs();
    if config.heartbeat.enabled {
        println!(
            "   Heartbeat: ✅ 已启用（间隔 {heartbeat_interval_secs} 秒，session={}, 文件 {}，间隔来自 {}）",
            config.heartbeat.effective_session_id(),
            heartbeat_file_status_label(&config),
            heartbeat_interval_config_source()
        );
    } else {
        println!("   Heartbeat: ⚠️  未启用（默认关闭；仅 jiaclaw serve 会挂后台任务）");
        println!(
            "   💡 在配置中设置 [heartbeat] enabled = true，可选 JIACLAW_HEARTBEAT_INTERVAL_SECS"
        );
    }

    // CORS 配置
    if config.http.cors.resolved().enabled {
        println!(
            "   CORS: ✅ {}（开关来自 {}，来源来自 {}）",
            cors_status_line(&config.http.cors),
            cors_enabled_config_source(),
            cors_origins_config_source()
        );
    } else {
        println!(
            "   CORS: ⚠️  {}（默认收紧，无 CORS 头；开关来自 {}）",
            cors_status_line(&config.http.cors),
            cors_enabled_config_source()
        );
        println!(
            "   💡 浏览器前端可设 [http.cors] enabled = true，或 export JIACLAW_CORS_ENABLED=1"
        );
        println!(
            "   💡 允许来源：allowed_origins 或 JIACLAW_CORS_ORIGINS（逗号分隔；* 仅在显式配置时）"
        );
    }

    // 5. StateKnot 集成状态
    println!("\n⚙️  StateKnot 集成");
    println!("   状态: ⏳ 等待稳定 API 发布");
    println!("   持久化: ❌ 未启用");
    println!("   PostgreSQL: ❌ 未配置");
    println!("   💡 参见 docs/stateknot-gaps.md 了解详情");

    // 6. 总结
    println!("\n📊 总结");
    println!("   • 工作空间文件: {workspace_file_count} 个");
    println!("   • 已发现技能: {skills_count} 个");
    println!("   • 已注册工具: {tools_count} 个");
    println!("   • HTTP 绑定: {}", config.http.bind);
    println!("   • 日志格式: {}", config.logging.effective_format());
    println!(
        "   • API 鉴权: {}",
        if api_token.is_some() {
            "已启用"
        } else {
            "未启用"
        }
    );
    println!(
        "   • Webhook 鉴权: {}",
        if webhook_secret.is_some() {
            "已启用"
        } else {
            "未启用"
        }
    );
    println!(
        "   • Telegram 鉴权: {}",
        if telegram_secret.is_some() {
            "已启用"
        } else {
            "未启用"
        }
    );
    println!(
        "   • Telegram Bot Token: {}",
        if telegram_bot_token.is_some() {
            "已配置"
        } else {
            "未配置"
        }
    );
    println!(
        "   • Slack 签名校验: {}",
        if slack_signing_secret.is_some() {
            "已启用"
        } else {
            "未启用"
        }
    );
    println!(
        "   • Slack Bot Token: {}",
        if slack_bot_token.is_some() {
            "已配置"
        } else {
            "未配置"
        }
    );
    println!(
        "   • Discord 签名校验: {}",
        if discord_public_key.is_some() {
            "已启用"
        } else {
            "未启用"
        }
    );
    println!(
        "   • Discord Bot Token: {}",
        if discord_bot_token.is_some() {
            "已配置"
        } else {
            "未配置"
        }
    );
    println!(
        "   • HTTP 限流: {}",
        if let Some(limit) = rate_limit_per_minute {
            format!("已启用（{limit} 次/分钟）")
        } else {
            "未启用".to_string()
        }
    );
    println!("   • HTTP 请求体上限: {max_body_bytes} 字节");
    println!("   • CORS: {}", cors_status_line(&config.http.cors));
    println!(
        "   • Metrics: {}",
        if metrics_public {
            "公开（无需鉴权，不限流）"
        } else {
            "需 API 鉴权（不限流）"
        }
    );
    println!(
        "   • Session TTL: {}",
        if let Some(ttl) = session_ttl_secs {
            format!("已启用（闲置 {ttl} 秒）")
        } else {
            "未启用".to_string()
        }
    );
    println!("   • 优雅退出宽限期: {shutdown_timeout_secs} 秒（SIGINT/SIGTERM）");
    println!("   • 技能热加载: POST /api/skills/reload；Unix SIGHUP（Windows 仅 HTTP）");
    println!(
        "   • Session 摘要压缩: {}",
        session_summarize_status_line(&config)
    );
    println!(
        "   • 工具超时: {}",
        if let Some(secs) = tool_timeout_secs {
            format!("已启用（每调用 {secs} 秒）")
        } else {
            "未启用".to_string()
        }
    );
    println!(
        "   • 工具循环上限: {}",
        max_tool_iterations_status_line(&config)
    );
    println!(
        "   • Heartbeat: {}",
        if config.heartbeat.enabled {
            format!(
                "已启用（间隔 {heartbeat_interval_secs} 秒，文件 {}）",
                heartbeat_file_status_label(&config)
            )
        } else {
            "未启用".to_string()
        }
    );

    if !has_key {
        println!("\n   ⚠️  运行模式: Stub（存根模式）");
        println!("   💡 未检测到 Brokerrouter/API key，将使用演示模式");
        println!("   💡 配置 JIACLAW_API_KEY 环境变量以启用真实模型调用");
    }

    if workspace_ok {
        println!("\n   ✅ 配置良好，可以开始使用");
        println!("   💡 试试: jiaclaw serve");
    } else {
        println!("\n   ⚠️  需要初始化工作空间");
        println!("   💡 运行: jiaclaw init");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use jiaclaw::SESSION_SUMMARY_PREFIX;
    use jiaclaw_core::{
        resolve_log_format, ChatMessage, ChatRequest, DeleteFileToolConfig, GlobToolConfig,
        GrepToolConfig, HeartbeatConfig, ListDirToolConfig, LogFormat, LoggingConfig,
        MemorySearchToolConfig, MemoryWriteToolConfig, MessageRole, MkdirToolConfig,
        MoveToolConfig, ReadFileToolConfig, SessionConfig, StrReplaceToolConfig, ToolsConfig,
        WebFetchToolConfig, WebSearchToolConfig, WriteFileToolConfig,
    };
    use tower::ServiceExt;

    // HTTP, channel and storage fixtures run offline deliberately. Production
    // provider defaults remain unchanged and are validated by integration tests.
    fn offline_test_config() -> AgentConfig {
        AgentConfig {
            provider: jiaclaw_core::ProviderConfig {
                provider_type: "stub".to_string(),
                ..jiaclaw_core::ProviderConfig::default()
            },
            ..AgentConfig::default()
        }
    }

    fn create_test_app() -> Router {
        create_test_app_with_auth(None, None)
    }

    fn create_test_app_with_secret(webhook_secret: Option<String>) -> Router {
        create_test_app_with_auth(None, webhook_secret)
    }

    fn create_test_app_with_auth(
        api_token: Option<String>,
        webhook_secret: Option<String>,
    ) -> Router {
        create_test_app_with_options(api_token, webhook_secret, None)
    }

    fn create_test_app_with_rate_limit(per_minute: u32) -> Router {
        create_test_app_with_options(None, None, Some(per_minute))
    }

    fn create_test_app_with_options(
        api_token: Option<String>,
        webhook_secret: Option<String>,
        rate_limit_per_minute: Option<u32>,
    ) -> Router {
        create_test_app_with_full(api_token, webhook_secret, rate_limit_per_minute, None)
    }

    fn create_test_app_with_session_ttl(session_ttl_secs: Option<u64>) -> Router {
        create_test_app_with_full(None, None, None, session_ttl_secs)
    }

    fn create_test_app_with_max_body_bytes(max_body_bytes: u64) -> Router {
        create_test_app_with_max_body_options(None, None, max_body_bytes)
    }

    fn create_test_app_with_max_body_options(
        api_token: Option<String>,
        rate_limit_per_minute: Option<u32>,
        max_body_bytes: u64,
    ) -> Router {
        let config = offline_test_config();
        let metrics = Arc::new(Metrics::default());
        let agent = attach_tool_metrics(
            JiaClawAgent::new(config).expect("创建测试 agent 失败"),
            &metrics,
        );
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: rate_limit_per_minute.and_then(build_rate_limiter),
            session_ttl: None,
            metrics,
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        build_router_with_body_limit(state, None, max_body_bytes)
    }

    fn create_test_app_with_full(
        api_token: Option<String>,
        webhook_secret: Option<String>,
        rate_limit_per_minute: Option<u32>,
        session_ttl_secs: Option<u64>,
    ) -> Router {
        create_test_app_with_metrics(
            api_token,
            webhook_secret,
            rate_limit_per_minute,
            session_ttl_secs,
            false,
        )
    }

    fn create_test_app_with_metrics_auth(
        api_token: Option<String>,
        metrics_require_auth: bool,
    ) -> Router {
        create_test_app_with_metrics(api_token, None, None, None, metrics_require_auth)
    }

    fn create_test_app_with_metrics(
        api_token: Option<String>,
        webhook_secret: Option<String>,
        rate_limit_per_minute: Option<u32>,
        session_ttl_secs: Option<u64>,
        metrics_require_auth: bool,
    ) -> Router {
        let config = offline_test_config();
        let metrics = Arc::new(Metrics::default());
        let agent = attach_tool_metrics(
            JiaClawAgent::new(config).expect("创建测试 agent 失败"),
            &metrics,
        );
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token,
            webhook_secret,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: rate_limit_per_minute.and_then(build_rate_limiter),
            session_ttl: session_ttl_secs.map(Duration::from_secs),
            metrics,
            metrics_require_auth,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };

        build_router(state)
    }

    fn create_test_app_with_cors(cors: &HttpCorsConfig) -> Router {
        create_test_app_with_cors_and_token(cors, None)
    }

    fn create_test_app_with_cors_and_token(
        cors: &HttpCorsConfig,
        api_token: Option<String>,
    ) -> Router {
        let config = offline_test_config();
        let metrics = Arc::new(Metrics::default());
        let agent = attach_tool_metrics(
            JiaClawAgent::new(config).expect("创建测试 agent 失败"),
            &metrics,
        );
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics,
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        build_router_with_cors(state, build_cors_layer(cors))
    }

    fn matching_browser_cors() -> HttpCorsConfig {
        HttpCorsConfig {
            enabled: true,
            allowed_origins: vec!["http://localhost:5173".to_string()],
            max_age_secs: Some(600),
            ..HttpCorsConfig::default()
        }
    }

    fn cors_header<'a>(response: &'a axum::http::Response<Body>, name: &str) -> Option<&'a str> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn header_list_contains(value: &str, needle: &str) -> bool {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(needle))
    }

    fn create_test_state_from_config(config: AgentConfig) -> AppState {
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        }
    }

    fn overflow_history(user_count: usize) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage {
            role: MessageRole::System,
            content: "你是一个助手".to_string(),
        }];
        for i in 0..user_count {
            messages.push(ChatMessage {
                role: MessageRole::User,
                content: format!("消息 {i}"),
            });
        }
        messages
    }

    async fn http_create_session(app: &Router) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: CreateSessionResponse = serde_json::from_slice(&body).unwrap();
        created.session_id
    }

    async fn http_get_session_status(app: &Router, session_id: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    async fn http_list_session_ids(app: &Router) -> Vec<String> {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: ListSessionsResponse = serde_json::from_slice(&body).unwrap();
        list.sessions.into_iter().map(|s| s.id).collect()
    }

    async fn http_delete_session(app: &Router, session_id: &str) -> DeleteSessionResponse {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn http_chat_with_session(app: &Router, session_id: &str) {
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "ttl-touch".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.to_string()),
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(health.status, "ok");
        assert_eq!(health.agent_name, "JiaClaw");
        assert!(!health.version.is_empty());
    }

    #[tokio::test]
    async fn test_chat_endpoint() {
        let app = create_test_app();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let request_body = serde_json::to_string(&request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_response: ChatResponse = serde_json::from_slice(&body).unwrap();

        assert!(!chat_response.message.content.is_empty());
    }

    #[tokio::test]
    async fn test_chat_endpoint_with_tool_call() {
        let app = create_test_app();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "列出工作空间".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let request_body = serde_json::to_string(&request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_response: ChatResponse = serde_json::from_slice(&body).unwrap();

        // 在存根模式下，应该触发工具调用
        assert!(!chat_response.tool_calls.is_empty(), "应该有工具调用");
        assert_eq!(chat_response.tool_calls[0].tool_name, "workspace_list");
    }

    #[tokio::test]
    async fn test_create_session() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let create_response: CreateSessionResponse = serde_json::from_slice(&body).unwrap();

        assert!(!create_response.session_id.is_empty());
    }

    #[tokio::test]
    async fn test_chat_with_session() {
        let app = create_test_app();

        // 第一次请求，创建 session
        let session_id = "test-session-123".to_string();
        let request1 = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };

        let request_body1 = serde_json::to_string(&request1).unwrap();

        let response1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body1))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response1.status(), StatusCode::OK);

        let body1 = axum::body::to_bytes(response1.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_response1: ChatResponse = serde_json::from_slice(&body1).unwrap();

        assert_eq!(chat_response1.session_id, Some(session_id.clone()));
        assert!(!chat_response1.message.content.is_empty());

        // 第二次请求，使用同一个 session
        let request2 = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "我是谁？".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };

        let request_body2 = serde_json::to_string(&request2).unwrap();

        let response2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body2))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response2.status(), StatusCode::OK);

        let body2 = axum::body::to_bytes(response2.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_response2: ChatResponse = serde_json::from_slice(&body2).unwrap();

        assert_eq!(chat_response2.session_id, Some(session_id));
        assert!(!chat_response2.message.content.is_empty());
    }

    #[tokio::test]
    async fn test_delete_session() {
        let app = create_test_app();

        let session_id = "test-session-to-delete".to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let delete_response: DeleteSessionResponse = serde_json::from_slice(&body).unwrap();

        // Session 不存在时也应该返回 200，但 success 为 false
        assert!(!delete_response.success);
    }

    #[tokio::test]
    async fn test_create_then_delete_session_success() {
        let app = create_test_app();

        // 创建 session
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(create_response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let create_result: CreateSessionResponse = serde_json::from_slice(&body).unwrap();
        let session_id = create_result.session_id;

        // 删除刚创建的 session
        let delete_response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{}", session_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(delete_response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(delete_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let delete_result: DeleteSessionResponse = serde_json::from_slice(&body).unwrap();

        // 应该成功删除
        assert!(delete_result.success, "刚创建的 session 应该能成功删除");
        assert!(delete_result.message.contains(&session_id));
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(X_REQUEST_ID).is_some());

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: ListSessionsResponse = serde_json::from_slice(&body).unwrap();
        assert!(list.sessions.is_empty());
    }

    #[tokio::test]
    async fn test_create_then_list_and_get_session() {
        let app = create_test_app();

        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: CreateSessionResponse = serde_json::from_slice(&body).unwrap();
        let session_id = created.session_id;

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: ListSessionsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(list.sessions.len(), 1);
        assert_eq!(list.sessions[0].id, session_id);
        assert_eq!(list.sessions[0].message_count, 0);

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let got: GetSessionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(got.id, session_id);
        assert!(got.messages.is_empty());
    }

    #[tokio::test]
    async fn test_get_session_returns_messages_after_chat() {
        let app = create_test_app();
        let session_id = "query-session-chat".to_string();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };

        let chat_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chat_response.status(), StatusCode::OK);

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let got: GetSessionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(got.id, session_id);
        assert!(
            got.messages.len() >= 2,
            "chat 后应包含用户消息与助手回复，实际: {}",
            got.messages.len()
        );
        assert_eq!(got.messages[0].role, MessageRole::User);
        assert_eq!(got.messages[0].content, "你好");
        assert_eq!(got.messages.last().unwrap().role, MessageRole::Assistant);

        let list_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: ListSessionsResponse = serde_json::from_slice(&body).unwrap();
        let summary = list
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .expect("list 应包含刚聊过的 session");
        assert_eq!(summary.message_count, got.messages.len());
    }

    fn parse_jsonl_messages(body: &str) -> Vec<ChatMessage> {
        body.lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("每行应为 ChatMessage JSON"))
            .collect()
    }

    async fn http_export_session(
        app: &Router,
        session_id: &str,
        format: Option<&str>,
    ) -> axum::http::Response<Body> {
        let uri = match format {
            Some(fmt) => format!("/api/sessions/{session_id}/export?format={fmt}"),
            None => format!("/api/sessions/{session_id}/export"),
        };
        app.clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_export_session_jsonl_line_count_matches_chat() {
        let app = create_test_app();
        let session_id = "export-session-chat".to_string();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };

        let chat_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chat_response.status(), StatusCode::OK);

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let got: GetSessionResponse = serde_json::from_slice(&body).unwrap();

        let export_response = http_export_session(&app, &session_id, None).await;
        assert_eq!(export_response.status(), StatusCode::OK);
        assert!(!request_id_header(&export_response).is_empty());
        assert_eq!(
            response_content_type(&export_response),
            SESSION_EXPORT_NDJSON
        );
        let export_body = body_text(export_response).await;
        let exported = parse_jsonl_messages(&export_body);
        assert_eq!(exported.len(), got.messages.len());
        assert_eq!(exported, got.messages);
    }

    #[tokio::test]
    async fn test_export_session_json_format() {
        let app = create_test_app();
        let session_id = "export-session-json".to_string();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "导出 JSON".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };
        let chat_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chat_response.status(), StatusCode::OK);

        let export_response = http_export_session(&app, &session_id, Some("json")).await;
        assert_eq!(export_response.status(), StatusCode::OK);
        assert!(response_content_type(&export_response).starts_with("application/json"));
        let body = axum::body::to_bytes(export_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let exported: GetSessionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(exported.id, session_id);
        assert!(
            exported.messages.len() >= 2,
            "json 导出应包含用户与助手消息，实际: {}",
            exported.messages.len()
        );
        assert_eq!(exported.messages[0].role, MessageRole::User);
        assert_eq!(exported.messages[0].content, "导出 JSON");
    }

    #[tokio::test]
    async fn test_export_session_not_found() {
        let app = create_test_app();
        let response = http_export_session(&app, "does-not-exist", None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.headers().get(X_REQUEST_ID).is_some());
        let payload: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["error"], "session_not_found");
    }

    #[tokio::test]
    async fn test_export_session_does_not_compact_or_rewrite() {
        let state = create_test_state_from_config(offline_test_config());
        let session_id = "export-no-compact".to_string();
        let history = overflow_history(60);
        let original_len = history.len();
        {
            let mut sessions = state.sessions.lock().unwrap();
            sessions
                .insert(session_id.clone(), SessionRecord::new(history))
                .unwrap();
        }
        let app = build_router(state.clone());

        let export_response = http_export_session(&app, &session_id, None).await;
        assert_eq!(export_response.status(), StatusCode::OK);
        let exported = parse_jsonl_messages(&body_text(export_response).await);
        assert_eq!(exported.len(), original_len);

        let stored_len = {
            let sessions = state.sessions.lock().unwrap();
            sessions
                .get(&session_id)
                .unwrap()
                .expect("export 不应删除 session")
                .messages
                .len()
        };
        assert_eq!(stored_len, original_len, "导出不应触发摘要或改写 store");
    }

    #[tokio::test(start_paused = true)]
    async fn test_export_session_ttl_expired_is_not_found() {
        let app = create_test_app_with_session_ttl(Some(1));
        let session_id = http_create_session(&app).await;

        let ok = http_export_session(&app, &session_id, None).await;
        assert_eq!(ok.status(), StatusCode::OK);

        tokio::time::advance(Duration::from_secs(1)).await;

        let expired = http_export_session(&app, &session_id, None).await;
        assert_eq!(expired.status(), StatusCode::NOT_FOUND);
        let payload: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(expired.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["error"], "session_not_found");
    }

    #[tokio::test(start_paused = true)]
    async fn test_export_session_does_not_refresh_ttl() {
        let app = create_test_app_with_session_ttl(Some(1));
        let session_id = http_create_session(&app).await;

        tokio::time::advance(Duration::from_millis(700)).await;
        let export_response = http_export_session(&app, &session_id, None).await;
        assert_eq!(export_response.status(), StatusCode::OK);

        tokio::time::advance(Duration::from_millis(700)).await;
        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::NOT_FOUND,
            "export 不应刷新 last_accessed"
        );
    }

    fn sample_import_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: "你好！我是 JiaClaw。".to_string(),
            },
        ]
    }

    fn sample_import_jsonl() -> String {
        encode_messages_jsonl(&sample_import_messages()).expect("编码 jsonl")
    }

    async fn http_import_session(
        app: &Router,
        uri: &str,
        content_type: Option<&str>,
        body: impl Into<Body>,
        auth: Option<&str>,
    ) -> axum::http::Response<Body> {
        let mut builder = Request::builder().method("POST").uri(uri);
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        if let Some(token) = auth {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(builder.body(body.into()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_import_session_jsonl_roundtrip_matches_get_and_export() {
        let app = create_test_app();
        let session_id = "import-roundtrip";
        let jsonl = sample_import_jsonl();

        let import_response = http_import_session(
            &app,
            &format!("/api/sessions/import?id={session_id}"),
            Some(SESSION_EXPORT_NDJSON),
            jsonl.clone(),
            None,
        )
        .await;
        assert_eq!(import_response.status(), StatusCode::OK);
        assert!(!request_id_header(&import_response).is_empty());
        let imported: GetSessionResponse = serde_json::from_slice(
            &axum::body::to_bytes(import_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(imported.id, session_id);
        assert_eq!(imported.messages, sample_import_messages());

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let got: GetSessionResponse = serde_json::from_slice(
            &axum::body::to_bytes(get_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got.messages, imported.messages);

        let export_response = http_export_session(&app, session_id, None).await;
        assert_eq!(export_response.status(), StatusCode::OK);
        let exported = parse_jsonl_messages(&body_text(export_response).await);
        assert_eq!(exported, imported.messages);
    }

    #[tokio::test]
    async fn test_import_session_conflict_without_overwrite() {
        let app = create_test_app();
        let session_id = "import-conflict";
        let first = http_import_session(
            &app,
            &format!("/api/sessions/import?id={session_id}"),
            Some(SESSION_EXPORT_NDJSON),
            sample_import_jsonl(),
            None,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);

        let second = http_import_session(
            &app,
            &format!("/api/sessions/import?id={session_id}"),
            Some(SESSION_EXPORT_NDJSON),
            sample_import_jsonl(),
            None,
        )
        .await;
        assert_eq!(second.status(), StatusCode::CONFLICT);
        assert!(!request_id_header(&second).is_empty());
        let payload: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(second.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["error"], "session_already_exists");
    }

    #[tokio::test]
    async fn test_import_session_overwrite_replaces_messages() {
        let app = create_test_app();
        let session_id = "import-overwrite";
        let first = http_import_session(
            &app,
            &format!("/api/sessions/import?id={session_id}"),
            Some(SESSION_EXPORT_NDJSON),
            sample_import_jsonl(),
            None,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);

        let replacement = ChatMessage {
            role: MessageRole::User,
            content: "覆盖后的消息".to_string(),
        };
        let jsonl = encode_messages_jsonl(std::slice::from_ref(&replacement)).unwrap();
        let overwrite = http_import_session(
            &app,
            &format!("/api/sessions/import?id={session_id}&overwrite=true"),
            Some(SESSION_EXPORT_NDJSON),
            jsonl,
            None,
        )
        .await;
        assert_eq!(overwrite.status(), StatusCode::OK);
        let imported: GetSessionResponse = serde_json::from_slice(
            &axum::body::to_bytes(overwrite.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(imported.messages, vec![replacement.clone()]);

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let got: GetSessionResponse = serde_json::from_slice(
            &axum::body::to_bytes(get_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got.messages, vec![replacement]);
    }

    #[tokio::test]
    async fn test_import_session_bad_line_is_400() {
        let app = create_test_app();
        let body = "{\"role\":\"user\",\"content\":\"ok\"}\n{not-json}\n";
        let response = http_import_session(
            &app,
            "/api/sessions/import?id=bad-line",
            Some(SESSION_EXPORT_NDJSON),
            body,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!request_id_header(&response).is_empty());
        let payload: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let error = payload["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("invalid_session_import"),
            "400 应标明非法导入，实际: {error}"
        );
        assert!(error.contains("line 2"), "应指出坏行号，实际: {error}");
    }

    #[tokio::test]
    async fn test_import_session_json_format_and_body_id() {
        let app = create_test_app();
        let body = serde_json::json!({
            "id": "import-json-id",
            "messages": sample_import_messages(),
        });
        let response = http_import_session(
            &app,
            "/api/sessions/import?format=json",
            Some("application/json"),
            body.to_string(),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let imported: GetSessionResponse = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(imported.id, "import-json-id");
        assert_eq!(imported.messages, sample_import_messages());
    }

    #[tokio::test]
    async fn test_import_session_truncates_overflow_without_llm() {
        let app = create_test_app();
        let messages: Vec<_> = (0..60)
            .map(|n| ChatMessage {
                role: MessageRole::User,
                content: format!("消息 {n}"),
            })
            .collect();
        let jsonl = encode_messages_jsonl(&messages).unwrap();
        let response = http_import_session(
            &app,
            "/api/sessions/import?id=import-overflow",
            Some(SESSION_EXPORT_NDJSON),
            jsonl,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let imported: GetSessionResponse = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(imported.messages.len(), MAX_SESSION_MESSAGES);
        assert_eq!(imported.messages.last().unwrap().content, "消息 59");
        assert!(imported.messages.iter().all(|m| m.content != "消息 0"));
    }

    #[tokio::test]
    async fn test_import_session_persists_when_enabled() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-import-{}.json", uuid::Uuid::new_v4()));
        let config = offline_test_config();
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: true,
            persist_path: Arc::new(persist_path.clone()),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);
        let response = http_import_session(
            &app,
            "/api/sessions/import?id=persist-import",
            Some(SESSION_EXPORT_NDJSON),
            sample_import_jsonl(),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let loaded = load_sessions(&persist_path);
        assert_eq!(
            loaded.get("persist-import").map(Vec::as_slice),
            Some(sample_import_messages().as_slice())
        );
        std::fs::remove_file(&persist_path).ok();
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.headers().get(X_REQUEST_ID).is_some());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["error"], "session_not_found");
    }

    #[tokio::test]
    async fn test_get_session_404_after_delete() {
        let app = create_test_app();

        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: CreateSessionResponse = serde_json::from_slice(&body).unwrap();
        let session_id = created.session_id;

        let delete_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_response.status(), StatusCode::OK);

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::NOT_FOUND);

        let list_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: ListSessionsResponse = serde_json::from_slice(&body).unwrap();
        assert!(!list.sessions.iter().any(|s| s.id == session_id));
    }

    #[tokio::test(start_paused = true)]
    async fn test_session_ttl_expires_after_idle() {
        let app = create_test_app_with_session_ttl(Some(1));
        let session_id = http_create_session(&app).await;

        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::OK
        );

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::NOT_FOUND
        );
        assert!(!http_list_session_ids(&app).await.contains(&session_id));

        let deleted = http_delete_session(&app, &session_id).await;
        assert!(!deleted.success, "过期 session 的 DELETE 应与不存在一致");
    }

    #[tokio::test(start_paused = true)]
    async fn test_session_ttl_disabled_does_not_expire() {
        let app = create_test_app_with_session_ttl(None);
        let session_id = http_create_session(&app).await;

        tokio::time::advance(Duration::from_secs(10)).await;

        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::OK
        );
        assert!(http_list_session_ids(&app).await.contains(&session_id));
    }

    #[tokio::test(start_paused = true)]
    async fn test_session_ttl_get_touch_refreshes() {
        let app = create_test_app_with_session_ttl(Some(1));
        let session_id = http_create_session(&app).await;

        tokio::time::advance(Duration::from_millis(700)).await;
        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::OK
        );

        tokio::time::advance(Duration::from_millis(700)).await;
        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::OK,
            "GET 触达应刷新 last_accessed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_session_ttl_chat_touch_refreshes() {
        let app = create_test_app_with_session_ttl(Some(1));
        let session_id = http_create_session(&app).await;

        tokio::time::advance(Duration::from_millis(700)).await;
        http_chat_with_session(&app, &session_id).await;

        tokio::time::advance(Duration::from_millis(700)).await;
        assert_eq!(
            http_get_session_status(&app, &session_id).await,
            StatusCode::OK,
            "chat 触达应刷新 last_accessed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_session_ttl_purge_saves_when_persist_enabled() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let session_id = "ttl-persist".to_string();
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "将过期".to_string(),
        }];

        let config = offline_test_config();
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: true,
            persist_path: Arc::new(persist_path.clone()),
            rate_limiter: None,
            session_ttl: Some(Duration::from_secs(1)),
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };

        {
            let mut map = state.sessions.lock().unwrap();
            map.insert(session_id.clone(), SessionRecord::new(messages))
                .unwrap();
            persist_session_map(&state, &map).unwrap();
        }
        assert_eq!(load_sessions(&persist_path).len(), 1);

        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(purge_expired_sessions(&state).await, 1);
        assert!(
            load_sessions(&persist_path).is_empty(),
            "过期清理后落盘应同步删除"
        );

        std::fs::remove_file(&persist_path).ok();
    }

    fn unique_workspace(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(super) fn test_state_for_workspace(workspace: PathBuf) -> AppState {
        let config = AgentConfig {
            workspace_path: workspace,
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        }
    }

    fn heartbeat_config(enabled: bool, interval_secs: u64, path: &str) -> AgentConfig {
        AgentConfig {
            workspace_path: unique_workspace("jiaclaw-hb-cfg"),
            heartbeat: HeartbeatConfig {
                enabled,
                interval_secs,
                path: path.to_string(),
                ..HeartbeatConfig::default()
            },
            ..offline_test_config()
        }
    }

    struct CountdownLatch {
        remaining: std::sync::atomic::AtomicUsize,
        notify: tokio::sync::Notify,
    }

    impl CountdownLatch {
        fn new(count: usize) -> Arc<Self> {
            Arc::new(Self {
                remaining: std::sync::atomic::AtomicUsize::new(count),
                notify: tokio::sync::Notify::new(),
            })
        }

        fn count_down(&self) {
            let prev = self
                .remaining
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            if prev == 1 {
                self.notify.notify_waiters();
            }
        }

        async fn wait(&self) {
            loop {
                if self.remaining.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    return;
                }
                let notified = self.notify.notified();
                if self.remaining.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    return;
                }
                notified.await;
            }
        }
    }

    #[test]
    fn heartbeat_disabled_does_not_spawn() {
        let config = heartbeat_config(false, 1, "HEARTBEAT.md");
        let state = test_state_for_workspace(config.workspace_path.clone());
        assert!(
            maybe_spawn_heartbeat(state, &config, None).is_none(),
            "enabled=false 不得启动心跳任务"
        );
        let _ = std::fs::remove_dir_all(&config.workspace_path);
    }

    #[tokio::test]
    async fn heartbeat_empty_file_skips_without_session() {
        let ws = unique_workspace("jiaclaw-hb-empty");
        std::fs::write(ws.join("HEARTBEAT.md"), "  \n").unwrap();
        let state = test_state_for_workspace(ws.clone());
        let outcome = run_heartbeat_tick(&state, &ws, "HEARTBEAT.md", "heartbeat").await;
        assert_eq!(outcome, HeartbeatTickOutcome::SkippedEmptyOrMissing);
        assert!(
            state.sessions.lock().unwrap().is_empty(),
            "空文件跳过时不应写入 session"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn heartbeat_missing_file_skips_without_session() {
        let ws = unique_workspace("jiaclaw-hb-missing");
        let state = test_state_for_workspace(ws.clone());
        let outcome = run_heartbeat_tick(&state, &ws, "HEARTBEAT.md", "heartbeat").await;
        assert_eq!(outcome, HeartbeatTickOutcome::SkippedEmptyOrMissing);
        assert!(state.sessions.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_short_interval_fires_at_least_once() {
        let ws = unique_workspace("jiaclaw-hb-fire");
        std::fs::write(ws.join("HEARTBEAT.md"), "periodic self-check: stretch").unwrap();
        let config = AgentConfig {
            workspace_path: ws.clone(),
            heartbeat: HeartbeatConfig {
                enabled: true,
                interval_secs: 1,
                path: "HEARTBEAT.md".to_string(),
                session_id: "heartbeat".to_string(),
            },
            ..offline_test_config()
        };

        let state = test_state_for_workspace(ws.clone());
        let latch = CountdownLatch::new(1);
        let latch_hook = latch.clone();
        let handle = maybe_spawn_heartbeat(
            state.clone(),
            &config,
            Some(Arc::new(move || latch_hook.count_down())),
        )
        .expect("enabled=true 应启动心跳任务");

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        latch.wait().await;

        let sessions = state.sessions.lock().unwrap();
        let rec = sessions
            .get("heartbeat")
            .unwrap()
            .expect("心跳应写入固定 session");
        assert!(rec
            .messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content.contains("periodic self-check")));
        assert!(rec
            .messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant));
        drop(sessions);

        handle.abort();
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn heartbeat_handle_abort_is_cancelled() {
        let config = heartbeat_config(true, 3600, "HEARTBEAT.md");
        let state = test_state_for_workspace(config.workspace_path.clone());
        let handle = maybe_spawn_heartbeat(state, &config, None).expect("enabled=true 应启动任务");
        assert!(!handle.is_finished(), "未 abort 前心跳任务应仍在运行");
        let tasks = BackgroundTasks {
            channels: None,
            scheduler: None,
            heartbeat: Some(handle),
            ttl_sweeper: None,
            skill_reloader: None,
        };
        tasks.abort();
        let joined = tasks.heartbeat.expect("handle").await.unwrap_err();
        assert!(joined.is_cancelled(), "heartbeat handle 应被取消");
        let _ = std::fs::remove_dir_all(&config.workspace_path);
    }

    #[tokio::test]
    async fn graceful_shutdown_flushes_sessions_when_persist_enabled() {
        let persist_path = std::env::temp_dir().join(format!(
            "jiaclaw-shutdown-persist-{}.json",
            uuid::Uuid::new_v4()
        ));
        let session_id = "shutdown-save".to_string();
        let mut sessions = HashMap::new();
        sessions.insert(
            session_id.clone(),
            SessionRecord::new(vec![ChatMessage {
                role: MessageRole::User,
                content: "persist-on-shutdown".to_string(),
            }]),
        );

        let mut state = test_state_for_workspace(unique_workspace("jiaclaw-shutdown"));
        state.persist_enabled = true;
        state.persist_path = Arc::new(persist_path.clone());
        state.sessions = Arc::new(Mutex::new(SessionStore::from_memory(sessions)));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let app = build_router(state.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();

        let serve = tokio::spawn(async move {
            serve_with_graceful_shutdown(
                listener,
                app,
                state,
                async {
                    let _ = rx.await;
                },
                Duration::from_secs(1),
                BackgroundTasks {
                    channels: None,
                    scheduler: None,
                    heartbeat: None,
                    ttl_sweeper: None,
                    skill_reloader: None,
                },
            )
            .await
        });

        tokio::task::yield_now().await;
        let _ = tx.send(());
        serve
            .await
            .expect("join serve task")
            .expect("graceful shutdown");

        let loaded = load_sessions(&persist_path);
        let rec = loaded.get(&session_id).expect("shutdown 应刷盘 session");
        assert_eq!(rec[0].content, "persist-on-shutdown");
        std::fs::remove_file(&persist_path).ok();
    }

    #[tokio::test]
    async fn graceful_shutdown_skips_flush_when_persist_disabled() {
        let persist_path = std::env::temp_dir().join(format!(
            "jiaclaw-shutdown-nopersist-{}.json",
            uuid::Uuid::new_v4()
        ));
        let mut sessions = HashMap::new();
        sessions.insert(
            "s".to_string(),
            SessionRecord::new(vec![ChatMessage {
                role: MessageRole::User,
                content: "should-not-write".to_string(),
            }]),
        );
        let mut state = test_state_for_workspace(unique_workspace("jiaclaw-shutdown-off"));
        state.persist_enabled = false;
        state.persist_path = Arc::new(persist_path.clone());
        state.sessions = Arc::new(Mutex::new(SessionStore::from_memory(sessions)));

        assert!(flush_sessions_on_shutdown(&state));
        assert!(!persist_path.exists(), "persist=false 时 shutdown 不得写盘");
    }

    #[test]
    fn default_shutdown_timeout_parse_is_15() {
        assert_eq!(
            jiaclaw_core::resolve_shutdown_timeout_secs(0, None),
            jiaclaw_core::DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
        assert_eq!(jiaclaw_core::resolve_shutdown_timeout_secs(15, None), 15);
        assert_eq!(
            jiaclaw_core::parse_positive_shutdown_timeout("15"),
            Some(15)
        );
        assert_eq!(jiaclaw_core::parse_positive_shutdown_timeout("0"), None);
        assert_eq!(jiaclaw_core::parse_positive_shutdown_timeout("abc"), None);
    }

    #[test]
    fn default_max_body_bytes_parse_is_1mib() {
        assert_eq!(
            jiaclaw_core::resolve_max_body_bytes(0, None),
            jiaclaw_core::DEFAULT_HTTP_MAX_BODY_BYTES
        );
        assert_eq!(
            jiaclaw_core::resolve_max_body_bytes(1_048_576, None),
            1_048_576
        );
        assert_eq!(
            jiaclaw_core::parse_positive_max_body_bytes("1048576"),
            Some(1_048_576)
        );
        assert_eq!(jiaclaw_core::parse_positive_max_body_bytes("0"), None);
        assert_eq!(jiaclaw_core::parse_positive_max_body_bytes("abc"), None);
        assert_eq!(DEFAULT_HTTP_MAX_BODY_BYTES, 1_048_576);
    }

    #[tokio::test]
    async fn heartbeat_tick_writes_user_and_assistant_to_session() {
        let ws = unique_workspace("jiaclaw-hb-once");
        std::fs::write(ws.join("HEARTBEAT.md"), "remind me to drink water").unwrap();
        let state = test_state_for_workspace(ws.clone());
        let outcome = run_heartbeat_tick(&state, &ws, "HEARTBEAT.md", "heartbeat").await;
        match outcome {
            HeartbeatTickOutcome::Completed { user_chars, .. } => {
                assert!(user_chars > 0);
            }
            other => panic!("expected completed tick, got {other:?}"),
        }
        let sessions = state.sessions.lock().unwrap();
        let rec = sessions.get("heartbeat").unwrap().expect("session");
        assert_eq!(rec.messages[0].role, MessageRole::User);
        assert!(rec.messages[0].content.contains("drink water"));
        assert!(rec
            .messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant));
        drop(sessions);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn test_get_session_reads_memory_not_disk() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let session_id = "mem-session".to_string();
        let mem_messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "内存中的消息".to_string(),
        }];
        let disk_messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "磁盘上的旧消息".to_string(),
        }];

        let mut disk_map = HashMap::new();
        disk_map.insert(session_id.clone(), disk_messages);
        save_sessions(&persist_path, &disk_map).expect("保存失败");

        let config = offline_test_config();
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let mut mem_map = HashMap::new();
        mem_map.insert(session_id.clone(), SessionRecord::new(mem_messages));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::from_memory(mem_map))),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: true,
            persist_path: Arc::new(persist_path.clone()),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let got: GetSessionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(got.messages.len(), 1);
        assert_eq!(got.messages[0].content, "内存中的消息");

        std::fs::remove_file(&persist_path).ok();
    }

    #[tokio::test]
    async fn test_session_message_limit() {
        let state = create_test_state_from_config(offline_test_config());
        let session_id = "test-limit-session".to_string();

        {
            let mut sessions = state.sessions.lock().unwrap();
            sessions
                .insert(session_id.clone(), SessionRecord::new(overflow_history(60)))
                .unwrap();
        }

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "新消息".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };

        let app = Router::new()
            .route("/api/chat", post(chat_handler))
            .with_state(state.clone());

        let request_body = serde_json::to_string(&request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_response: ChatResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(chat_response.session_id, Some(session_id.clone()));

        let stored = {
            let sessions = state.sessions.lock().unwrap();
            sessions
                .get(&session_id)
                .unwrap()
                .expect("session 应存在")
                .messages
                .clone()
        };

        // 硬截断后写入 assistant，可能略超上限
        assert!(
            stored.len() <= MAX_SESSION_MESSAGES + 1,
            "截断后应接近上限，实际 {}",
            stored.len()
        );
        assert_eq!(stored[0].role, MessageRole::System);
        assert_eq!(stored[0].content, "你是一个助手");
        assert!(stored.iter().any(|m| m.content == "新消息"));
        assert!(
            !stored
                .iter()
                .any(|m| m.role == MessageRole::User && m.content == "消息 0"),
            "最旧用户消息应被丢弃"
        );
        assert!(!stored[0].content.contains(SESSION_SUMMARY_PREFIX));
    }

    #[tokio::test]
    async fn test_session_summarize_on_overflow_replaces_old_messages() {
        let config = AgentConfig {
            session: SessionConfig {
                summarize_on_overflow: true,
                keep_recent: 10,
            },
            ..offline_test_config()
        };
        let state = create_test_state_from_config(config);
        let session_id = "test-summarize-session".to_string();

        {
            let mut sessions = state.sessions.lock().unwrap();
            sessions
                .insert(session_id.clone(), SessionRecord::new(overflow_history(60)))
                .unwrap();
        }

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "新消息".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: Some(session_id.clone()),
        };

        let app = Router::new()
            .route("/api/chat", post(chat_handler))
            .with_state(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let stored = {
            let sessions = state.sessions.lock().unwrap();
            sessions
                .get(&session_id)
                .unwrap()
                .expect("session 应存在")
                .messages
                .clone()
        };

        assert_eq!(stored[0].role, MessageRole::System);
        assert!(
            stored[0].content.contains(SESSION_SUMMARY_PREFIX),
            "旧消息应折叠为带标记的摘要: {}",
            stored[0].content
        );
        assert!(stored.iter().any(|m| m.content == "新消息"));
        assert!(stored
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == "消息 59"));
        assert!(
            !stored
                .iter()
                .any(|m| m.role == MessageRole::User && m.content == "消息 0"),
            "被摘要的旧用户消息不应再作为独立条目"
        );
        assert!(
            stored.len() < 20,
            "摘要压缩后历史应远小于硬截断上限，实际 {}",
            stored.len()
        );
    }

    #[tokio::test]
    async fn test_webhook_session_summarize_on_overflow() {
        let config = AgentConfig {
            session: SessionConfig {
                summarize_on_overflow: true,
                keep_recent: 10,
            },
            ..offline_test_config()
        };
        let state = create_test_state_from_config(config);
        let session_id = "webhook:overflow-chat".to_string();
        {
            let mut sessions = state.sessions.lock().unwrap();
            sessions
                .insert(session_id.clone(), SessionRecord::new(overflow_history(60)))
                .unwrap();
        }

        let app = build_router(state.clone());
        let webhook_request = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "overflow-chat".to_string(),
            text: "新消息".to_string(),
            username: None,
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&webhook_request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let stored = {
            let sessions = state.sessions.lock().unwrap();
            sessions
                .get(&session_id)
                .unwrap()
                .expect("webhook session 应存在")
                .messages
                .clone()
        };
        assert!(stored[0].content.contains(SESSION_SUMMARY_PREFIX));
        assert!(stored.iter().any(|m| m.content == "新消息"));
        assert!(!stored
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == "消息 0"));
    }

    #[tokio::test]
    async fn test_tools_endpoint() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();

        // 应该有一些已注册的工具
        assert!(!tools_response.tools.is_empty(), "应该有已注册的工具");
        assert!(
            tools_response
                .tools
                .iter()
                .any(|t| t.name == "memory_append"),
            "应注册 memory_append 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "web_search"),
            "应注册默认 web_search 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "web_fetch"),
            "应注册默认 web_fetch 工具"
        );
        assert!(
            tools_response
                .tools
                .iter()
                .any(|t| t.name == "memory_search"),
            "应注册默认 memory_search 工具"
        );
        assert!(
            tools_response
                .tools
                .iter()
                .any(|t| t.name == "memory_write"),
            "应注册默认 memory_write 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "应注册默认 read_file 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "list_dir"),
            "应注册默认 list_dir 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "write_file"),
            "应注册默认 write_file 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "delete_file"),
            "应注册默认 delete_file 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "str_replace"),
            "应注册默认 str_replace 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "grep"),
            "应注册默认 grep 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "glob"),
            "应注册默认 glob 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "mkdir"),
            "应注册默认 mkdir 工具"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "move"),
            "应注册默认 move 工具"
        );

        // 验证工具信息包含名称和描述
        for tool in &tools_response.tools {
            assert!(!tool.name.is_empty(), "工具名称不应为空");
            assert!(!tool.description.is_empty(), "工具描述不应为空");
        }
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_web_search_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-web-search-off"),
            tools: ToolsConfig {
                web_search: WebSearchToolConfig {
                    enabled: false,
                    brave_api_key: None,
                },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "web_search"),
            "enabled=false 时不应出现 web_search"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_web_fetch_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-web-fetch-off"),
            tools: ToolsConfig {
                web_fetch: WebFetchToolConfig {
                    enabled: false,
                    allow_private: false,
                },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "web_fetch"),
            "enabled=false 时不应出现 web_fetch"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "web_search"),
            "关闭 web_fetch 不应影响 web_search"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_memory_search_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-memory-search-off"),
            tools: ToolsConfig {
                memory_search: MemorySearchToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response
                .tools
                .iter()
                .all(|t| t.name != "memory_search"),
            "enabled=false 时不应出现 memory_search"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "web_search"),
            "关闭 memory_search 不应影响 web_search"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "web_fetch"),
            "关闭 memory_search 不应影响 web_fetch"
        );
        assert!(
            tools_response
                .tools
                .iter()
                .any(|t| t.name == "memory_write"),
            "关闭 memory_search 不应影响 memory_write"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_memory_write_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-memory-write-off"),
            tools: ToolsConfig {
                memory_write: MemoryWriteToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response
                .tools
                .iter()
                .all(|t| t.name != "memory_write"),
            "enabled=false 时不应出现 memory_write"
        );
        assert!(
            tools_response
                .tools
                .iter()
                .any(|t| t.name == "memory_search"),
            "关闭 memory_write 不应影响 memory_search"
        );
        assert!(
            tools_response
                .tools
                .iter()
                .any(|t| t.name == "memory_append"),
            "关闭 memory_write 不应影响 memory_append"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_read_file_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-read-file-off"),
            tools: ToolsConfig {
                read_file: ReadFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "read_file"),
            "enabled=false 时不应出现 read_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "list_dir"),
            "关闭 read_file 不应影响 list_dir"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_list_dir_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-list-dir-off"),
            tools: ToolsConfig {
                list_dir: ListDirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "list_dir"),
            "enabled=false 时不应出现 list_dir"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 list_dir 不应影响 read_file"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_write_file_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-write-file-off"),
            tools: ToolsConfig {
                write_file: WriteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "write_file"),
            "enabled=false 时不应出现 write_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 write_file 不应影响 read_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "list_dir"),
            "关闭 write_file 不应影响 list_dir"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_delete_file_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-delete-file-off"),
            tools: ToolsConfig {
                delete_file: DeleteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "delete_file"),
            "enabled=false 时不应出现 delete_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "write_file"),
            "关闭 delete_file 不应影响 write_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 delete_file 不应影响 read_file"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_str_replace_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-str-replace-off"),
            tools: ToolsConfig {
                str_replace: StrReplaceToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "str_replace"),
            "enabled=false 时不应出现 str_replace"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "write_file"),
            "关闭 str_replace 不应影响 write_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "delete_file"),
            "关闭 str_replace 不应影响 delete_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 str_replace 不应影响 read_file"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_grep_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-grep-off"),
            tools: ToolsConfig {
                grep: GrepToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "grep"),
            "enabled=false 时不应出现 grep"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "str_replace"),
            "关闭 grep 不应影响 str_replace"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 grep 不应影响 read_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "glob"),
            "关闭 grep 不应影响 glob"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "mkdir"),
            "关闭 grep 不应影响 mkdir"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_glob_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-glob-off"),
            tools: ToolsConfig {
                glob: GlobToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "glob"),
            "enabled=false 时不应出现 glob"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "grep"),
            "关闭 glob 不应影响 grep"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 glob 不应影响 read_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "mkdir"),
            "关闭 glob 不应影响 mkdir"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "move"),
            "关闭 glob 不应影响 move"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_mkdir_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-mkdir-off"),
            tools: ToolsConfig {
                mkdir: MkdirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "mkdir"),
            "enabled=false 时不应出现 mkdir"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "glob"),
            "关闭 mkdir 不应影响 glob"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 mkdir 不应影响 read_file"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "move"),
            "关闭 mkdir 不应影响 move"
        );
    }

    #[tokio::test]
    async fn test_tools_endpoint_omits_move_when_disabled() {
        let config = AgentConfig {
            workspace_path: unique_workspace("jiaclaw-move-off"),
            tools: ToolsConfig {
                r#move: MoveToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..offline_test_config()
        };
        let agent = JiaClawAgent::new(config).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));
        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tools_response: ToolsResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            tools_response.tools.iter().all(|t| t.name != "move"),
            "enabled=false 时不应出现 move"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "mkdir"),
            "关闭 move 不应影响 mkdir"
        );
        assert!(
            tools_response.tools.iter().any(|t| t.name == "read_file"),
            "关闭 move 不应影响 read_file"
        );
    }

    #[test]
    fn web_search_status_lines_do_not_print_api_key() {
        let config = AgentConfig {
            tools: ToolsConfig {
                web_search: WebSearchToolConfig {
                    enabled: true,
                    brave_api_key: Some("BSA-super-secret-key".to_string()),
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let joined = web_search_status_lines(&config).join("\n");
        assert!(
            !joined.contains("BSA-super-secret-key"),
            "doctor must not print api key: {joined}"
        );
        assert!(joined.contains("已配置"), "{joined}");
    }

    #[test]
    fn tracing_fmt_kind_selects_text_by_default_and_json_when_configured() {
        assert_eq!(
            tracing_fmt_kind(LoggingConfig::default().format),
            TracingFmtKind::Text
        );
        assert_eq!(tracing_fmt_kind(LogFormat::Text), TracingFmtKind::Text);
        assert_eq!(tracing_fmt_kind(LogFormat::Json), TracingFmtKind::Json);
        let json = LoggingConfig {
            format: LogFormat::Json,
            level: None,
        };
        assert_eq!(
            tracing_fmt_kind(json.effective_format()),
            TracingFmtKind::Json
        );
        let env_overridden = resolve_log_format(LogFormat::Text, Some("json"));
        assert_eq!(tracing_fmt_kind(env_overridden), TracingFmtKind::Json);
    }

    #[test]
    fn logging_status_line_prints_format_not_secrets() {
        let logging = LoggingConfig {
            format: LogFormat::Json,
            level: Some("info".to_string()),
        };
        let line = logging_status_line(&logging);
        assert!(line.contains("json"), "{line}");
        assert!(!line.contains("token"), "{line}");
        assert!(!line.contains("secret"), "{line}");
    }

    #[test]
    fn json_tracing_layer_emits_one_parseable_line_with_request_id() {
        use std::io::{self, Write};
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct BufferWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for BufferWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for BufferWriter {
            type Writer = Self;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(BufferWriter(buf.clone()))
            .with_env_filter("info")
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(request_id = "req-1", "hello json");
        });

        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let line = output.lines().next().expect("one json line");
        let value: serde_json::Value = serde_json::from_str(line).expect("parse json log line");
        assert_eq!(value["level"], "INFO");
        assert!(value.get("timestamp").is_some(), "{value}");
        assert!(value.get("target").is_some(), "{value}");
        assert_eq!(value["fields"]["message"], "hello json");
        assert_eq!(value["fields"]["request_id"], "req-1");
    }

    #[test]
    fn web_fetch_status_line_reports_enabled_and_private_policy() {
        let enabled = AgentConfig::default();
        assert!(web_fetch_status_line(&enabled).contains("已启用"));
        assert!(web_fetch_status_line(&enabled).contains("拒绝"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                web_fetch: WebFetchToolConfig {
                    enabled: false,
                    allow_private: false,
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(web_fetch_status_line(&disabled).contains("已关闭"));

        let allow_private = AgentConfig {
            tools: ToolsConfig {
                web_fetch: WebFetchToolConfig {
                    enabled: true,
                    allow_private: true,
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(web_fetch_status_line(&allow_private).contains("allow_private"));
    }

    #[test]
    fn memory_search_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(memory_search_status_line(&enabled).contains("已启用"));
        assert!(memory_search_status_line(&enabled).contains("MEMORY"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                memory_search: MemorySearchToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(memory_search_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn memory_write_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(memory_write_status_line(&enabled).contains("已启用"));
        assert!(memory_write_status_line(&enabled).contains("MEMORY"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                memory_write: MemoryWriteToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(memory_write_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn read_file_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(read_file_status_line(&enabled).contains("已启用"));
        assert!(read_file_status_line(&enabled).contains("256KiB"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                read_file: ReadFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(read_file_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn list_dir_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(list_dir_status_line(&enabled).contains("已启用"));
        assert!(list_dir_status_line(&enabled).contains("不递归"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                list_dir: ListDirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(list_dir_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn write_file_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(write_file_status_line(&enabled).contains("已启用"));
        assert!(write_file_status_line(&enabled).contains("256KiB"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                write_file: WriteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(write_file_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn delete_file_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(delete_file_status_line(&enabled).contains("已启用"));
        assert!(delete_file_status_line(&enabled).contains("拒绝目录"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                delete_file: DeleteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(delete_file_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn str_replace_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(str_replace_status_line(&enabled).contains("已启用"));
        assert!(str_replace_status_line(&enabled).contains("256KiB"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                str_replace: StrReplaceToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(str_replace_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn grep_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(grep_status_line(&enabled).contains("已启用"));
        assert!(grep_status_line(&enabled).contains("字面量"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                grep: GrepToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(grep_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn glob_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(glob_status_line(&enabled).contains("已启用"));
        assert!(glob_status_line(&enabled).contains("glob"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                glob: GlobToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(glob_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn mkdir_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(mkdir_status_line(&enabled).contains("已启用"));
        assert!(mkdir_status_line(&enabled).contains("mkdir"));

        let disabled = AgentConfig {
            tools: ToolsConfig {
                mkdir: MkdirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(mkdir_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn move_status_line_reports_enabled_and_disabled() {
        let enabled = AgentConfig::default();
        assert!(move_status_line(&enabled).contains("已启用"));
        assert!(
            move_status_line(&enabled).contains("移动")
                || move_status_line(&enabled).contains("重命名")
        );

        let disabled = AgentConfig {
            tools: ToolsConfig {
                r#move: MoveToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        assert!(move_status_line(&disabled).contains("已关闭"));
    }

    #[test]
    fn max_tool_iterations_status_line_shows_effective_default() {
        let config = AgentConfig::default();
        let line = max_tool_iterations_status_line(&config);
        assert!(
            line.contains(&config.effective_max_tool_iterations().to_string()),
            "{line}"
        );
        assert!(
            line.contains("配置文件") || line.contains("环境变量"),
            "{line}"
        );
    }

    #[tokio::test]
    async fn test_skills_endpoint() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let skills_response: SkillsResponse = serde_json::from_slice(&body).unwrap();

        // 技能可能为空（如果工作空间不存在或没有技能）
        // 只验证响应格式正确
        for skill in &skills_response.skills {
            assert!(!skill.name.is_empty(), "技能名称不应为空");
            assert!(!skill.description.is_empty(), "技能描述不应为空");
            assert!(!skill.path.is_empty(), "技能路径不应为空");
        }
    }

    fn write_test_skill(workspace: &std::path::Path, name: &str, body: &str) {
        let dir = workspace.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    async fn get_skills_json(app: Router) -> SkillsResponse {
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn test_skills_reload_picks_up_new_skill() {
        let workspace = unique_workspace("jiaclaw-skills-reload");
        write_test_skill(
            &workspace,
            "alpha",
            "# Alpha\n\n## Description\n\nFirst skill\n",
        );
        let app = build_router(test_state_for_workspace(workspace.clone()));

        let listed = get_skills_json(app.clone()).await;
        assert!(listed.skills.iter().any(|s| s.name == "alpha"));
        assert!(!listed.skills.iter().any(|s| s.name == "beta"));

        write_test_skill(
            &workspace,
            "beta",
            "# Beta\n\n## Description\n\nSecond skill\n",
        );

        let before = get_skills_json(app.clone()).await;
        assert!(
            !before.skills.iter().any(|s| s.name == "beta"),
            "未 reload 前 GET 不应看到磁盘新技能"
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/skills/reload")
                    .header("X-Request-Id", "reload-ok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(request_id_header(&response), "reload-ok");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let reload: SkillsReloadResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(reload.reloaded, 2);
        assert!(reload.skills.iter().any(|s| s.name == "beta"));

        let after = get_skills_json(app).await;
        assert!(after.skills.iter().any(|s| s.name == "beta"));

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[tokio::test]
    async fn test_skills_reload_keeps_old_table_on_bad_file() {
        let workspace = unique_workspace("jiaclaw-skills-reload-bad");
        write_test_skill(
            &workspace,
            "alpha",
            "# Alpha\n\n## Description\n\nFirst skill\n",
        );
        let app = build_router(test_state_for_workspace(workspace.clone()));

        write_test_skill(
            &workspace,
            "broken",
            "---\nname: [not yaml\n---\n# Broken\n",
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/skills/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let error = json["error"].as_str().unwrap_or("");
        assert!(
            error.contains("已保留旧表"),
            "错误应说明保留旧表，实际: {error}"
        );

        let listed = get_skills_json(app).await;
        assert!(listed.skills.iter().any(|s| s.name == "alpha"));
        assert!(!listed.skills.iter().any(|s| s.name == "broken"));

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[tokio::test]
    async fn test_webhook_inbound_without_secret() {
        let app = create_test_app();

        let webhook_request = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "test-chat-123".to_string(),
            text: "你好，这是一个测试消息".to_string(),
            username: Some("test_user".to_string()),
        };

        let request_body = serde_json::to_string(&webhook_request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let webhook_response: InboundWebhookResponse = serde_json::from_slice(&body).unwrap();

        assert!(webhook_response.ok);
        assert_eq!(webhook_response.session_id, "webhook:test-chat-123");
        assert!(webhook_response.reply.is_some());
        assert!(webhook_response.message.is_some());
    }

    #[tokio::test]
    async fn test_webhook_inbound_with_secret_missing_header() {
        let app = create_test_app_with_secret(Some("test-secret-123".to_string()));

        let webhook_request = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "test-chat-456".to_string(),
            text: "测试消息".to_string(),
            username: None,
        };

        let request_body = serde_json::to_string(&webhook_request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(error_response["ok"], false);
        assert_eq!(error_response["error"], "unauthorized");
    }

    #[tokio::test]
    async fn test_webhook_inbound_with_secret_wrong_secret() {
        let app = create_test_app_with_secret(Some("correct-secret".to_string()));

        let webhook_request = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "test-chat-789".to_string(),
            text: "测试消息".to_string(),
            username: None,
        };

        let request_body = serde_json::to_string(&webhook_request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .header("X-Webhook-Secret", "wrong-secret")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let error_response: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(error_response["ok"], false);
        assert_eq!(error_response["error"], "unauthorized");
    }

    #[tokio::test]
    async fn test_webhook_inbound_with_secret_correct() {
        let app = create_test_app_with_secret(Some("correct-secret".to_string()));

        let webhook_request = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "test-chat-correct".to_string(),
            text: "认证成功的消息".to_string(),
            username: Some("authenticated_user".to_string()),
        };

        let request_body = serde_json::to_string(&webhook_request).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .header("X-Webhook-Secret", "correct-secret")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let webhook_response: InboundWebhookResponse = serde_json::from_slice(&body).unwrap();

        assert!(webhook_response.ok);
        assert_eq!(webhook_response.session_id, "webhook:test-chat-correct");
        assert!(webhook_response.reply.is_some());
        assert!(webhook_response.message.is_some());
    }

    #[tokio::test]
    async fn test_webhook_inbound_session_persistence() {
        let app = create_test_app();

        // 第一次调用
        let webhook_request1 = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "persistent-chat".to_string(),
            text: "第一条消息".to_string(),
            username: None,
        };

        let request_body1 = serde_json::to_string(&webhook_request1).unwrap();

        let response1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body1))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response1.status(), StatusCode::OK);

        // 第二次调用，使用相同的 chat_id
        let webhook_request2 = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "persistent-chat".to_string(),
            text: "第二条消息".to_string(),
            username: None,
        };

        let request_body2 = serde_json::to_string(&webhook_request2).unwrap();

        let response2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body2))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response2.status(), StatusCode::OK);

        let body2 = axum::body::to_bytes(response2.into_body(), usize::MAX)
            .await
            .unwrap();
        let webhook_response2: InboundWebhookResponse = serde_json::from_slice(&body2).unwrap();

        assert!(webhook_response2.ok);
        assert_eq!(webhook_response2.session_id, "webhook:persistent-chat");
    }

    #[tokio::test]
    async fn test_session_persistence_disabled() {
        let config = offline_test_config();
        let agent = JiaClawAgent::new(config.clone()).expect("创建测试 agent 失败");
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));

        let state = AppState {
            agent: Arc::new(agent),
            sessions: Arc::new(Mutex::new(SessionStore::memory())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            api_token: None,
            webhook_secret: None,
            persist_enabled: false,
            persist_path: Arc::new(persist_path.clone()),
            rate_limiter: None,
            session_ttl: None,
            metrics: Arc::new(Metrics::default()),
            metrics_require_auth: false,
            channel_runtime: None,
            scheduler_health: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };

        let session_id = "test-session".to_string();
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "测试消息".to_string(),
        }];

        {
            let mut sessions = state.sessions.lock().unwrap();
            sessions
                .insert(session_id.clone(), SessionRecord::new(messages.clone()))
                .unwrap();
        }

        assert!(!persist_path.exists(), "persist=false 时不应该创建文件");
    }

    #[tokio::test]
    async fn test_session_persistence_enabled() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));

        let session_id = "test-session".to_string();
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "测试消息".to_string(),
        }];

        {
            let mut sessions_map = HashMap::new();
            sessions_map.insert(session_id.clone(), messages.clone());
            save_sessions(&persist_path, &sessions_map).expect("保存失败");
        }

        assert!(persist_path.exists(), "persist=true 时应该创建文件");

        let loaded = load_sessions(&persist_path);
        assert_eq!(loaded.len(), 1, "应该加载 1 个 session");
        assert_eq!(loaded.get(&session_id).unwrap().len(), 1, "应该有 1 条消息");
        assert_eq!(loaded.get(&session_id).unwrap()[0].content, "测试消息");

        std::fs::remove_file(&persist_path).ok();
    }

    #[tokio::test]
    async fn test_session_persistence_corrupted_file() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));

        std::fs::write(&persist_path, "{ invalid json ").expect("写入失败");

        let loaded = load_sessions(&persist_path);
        assert_eq!(loaded.len(), 0, "损坏的文件应该返回空 map");

        std::fs::remove_file(&persist_path).ok();
    }

    #[tokio::test]
    async fn test_session_persistence_nonexistent_file() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));

        let loaded = load_sessions(&persist_path);
        assert_eq!(loaded.len(), 0, "不存在的文件应该返回空 map");
    }

    #[tokio::test]
    async fn test_session_persistence_delete() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-test-{}.json", uuid::Uuid::new_v4()));

        let session_id1 = "test-session-1".to_string();
        let session_id2 = "test-session-2".to_string();
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: "测试消息".to_string(),
        }];

        {
            let mut sessions_map = HashMap::new();
            sessions_map.insert(session_id1.clone(), messages.clone());
            sessions_map.insert(session_id2.clone(), messages.clone());
            save_sessions(&persist_path, &sessions_map).expect("保存失败");
        }

        let mut loaded = load_sessions(&persist_path);
        assert_eq!(loaded.len(), 2, "应该加载 2 个 sessions");

        loaded.remove(&session_id1);
        save_sessions(&persist_path, &loaded).expect("保存失败");

        let loaded_after_delete = load_sessions(&persist_path);
        assert_eq!(loaded_after_delete.len(), 1, "删除后应该剩 1 个 session");
        assert!(loaded_after_delete.contains_key(&session_id2));
        assert!(!loaded_after_delete.contains_key(&session_id1));

        std::fs::remove_file(&persist_path).ok();
    }

    #[tokio::test]
    async fn test_api_auth_not_required_when_no_token() {
        let app = create_test_app();

        // 不带 token 的请求应该成功
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "测试".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_auth_required_with_token() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // 不带 token 的请求应该返回 401
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "测试".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_api_auth_bearer_token_success() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // 带正确 Bearer token 的请求应该成功
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "测试".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_auth_x_api_token_success() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // 带正确 X-Api-Token 的请求应该成功
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "测试".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("x-api-token", "test-token-123")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_auth_wrong_token() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // 带错误 token 的请求应该返回 401
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "测试".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_health_endpoint_no_auth() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // health 端点不需要鉴权
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn test_sessions_api_requires_auth() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // POST /api/sessions 应该需要鉴权
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // GET /api/sessions 无 token 应 401
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // GET /api/sessions/:id 无 token 应 401（即使不存在也不应先 404）
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/any-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // GET /api/sessions/:id/export 无 token 应 401（即使不存在也不应先 404）
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/any-id/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // POST /api/sessions/import 无 token 应 401
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/import")
                    .header("content-type", SESSION_EXPORT_NDJSON)
                    .body(Body::from(sample_import_jsonl()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // 带正确 token 应该成功
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: CreateSessionResponse = serde_json::from_slice(&body).unwrap();
        let session_id = created.session_id;

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);

        let export_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}/export"))
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(export_response.status(), StatusCode::OK);
        assert_eq!(
            response_content_type(&export_response),
            SESSION_EXPORT_NDJSON
        );

        let import_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/import?id=auth-import")
                    .header("authorization", "Bearer test-token-123")
                    .header("content-type", SESSION_EXPORT_NDJSON)
                    .body(Body::from(sample_import_jsonl()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(import_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_sessions_query_allows_anonymous_when_no_token() {
        let app = create_test_app();

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_tools_api_requires_auth() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // GET /api/tools 应该需要鉴权
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // 带正确 token 应该成功
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/tools")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_skills_api_requires_auth() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        // GET /api/skills 应该需要鉴权
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // 带正确 token 应该成功
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/skills")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_skills_reload_api_requires_auth() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/skills/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/skills/reload")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_disabled_allows_many_requests() {
        let app = create_test_app();

        for _ in 0..5 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/tools")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_no_rate_limit_headers(&response);
        }
    }

    #[tokio::test]
    async fn test_rate_limit_enabled_success_has_quota_headers() {
        let app = create_test_app_with_rate_limit(5);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_rate_limit_headers(&response, 5, Some(4));
        assert!(
            response.headers().get(header::RETRY_AFTER).is_none(),
            "成功响应不应带 Retry-After"
        );
        assert!(!request_id_header(&response).is_empty());
    }

    #[tokio::test]
    async fn test_rate_limit_remaining_decreases() {
        let app = create_test_app_with_rate_limit(4);
        for expected_remaining in [3, 2, 1] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/tools")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_rate_limit_headers(&response, 4, Some(expected_remaining));
        }
    }

    #[tokio::test]
    async fn test_rate_limit_headers_on_unauthorized_api() {
        let app = create_test_app_with_options(Some("secret-token".to_string()), None, Some(8));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_rate_limit_headers(&response, 8, Some(7));
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
        assert!(!request_id_header(&response).is_empty());
    }

    #[tokio::test]
    async fn test_rate_limit_returns_429_with_retry_after() {
        let app = create_test_app_with_rate_limit(2);

        let mut statuses = Vec::new();
        for _ in 0..3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/tools")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            statuses.push(response.status());
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                assert_rate_limit_headers(&response, 2, Some(0));
                let retry_after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .expect("429 必须包含 Retry-After")
                    .to_str()
                    .unwrap();
                let secs: u64 = retry_after.parse().expect("Retry-After 应为秒数");
                assert!(secs >= 1, "Retry-After 至少为 1 秒");

                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(payload["error"], "rate_limit_exceeded");
            } else {
                assert_rate_limit_headers(&response, 2, None);
                assert!(response.headers().get(header::RETRY_AFTER).is_none());
            }
        }

        assert_eq!(statuses[0], StatusCode::OK);
        assert_eq!(statuses[1], StatusCode::OK);
        assert_eq!(statuses[2], StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_health_is_not_rate_limited() {
        let app = create_test_app_with_rate_limit(1);

        // 先打满 /api/* 配额
        let limited = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::OK);

        let limited = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

        // GET /health 仍应 200，且 oneshot 不依赖 ConnectInfo，也不带限流头
        for _ in 0..3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_no_rate_limit_headers(&response);
        }
    }

    #[tokio::test]
    async fn test_hooks_inbound_is_rate_limited() {
        let app = create_test_app_with_rate_limit(1);

        let webhook_request = InboundWebhookRequest {
            channel: "webhook".to_string(),
            chat_id: "rate-limit-chat".to_string(),
            text: "限流测试".to_string(),
            username: None,
        };
        let request_body = serde_json::to_string(&webhook_request).unwrap();

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_rate_limit_headers(&first, 1, Some(0));

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/inbound")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(second.headers().get(header::RETRY_AFTER).is_some());
        assert_rate_limit_headers(&second, 1, Some(0));
    }

    #[tokio::test]
    async fn test_disabled_channels_remain_rate_limited() {
        for path in ["/hooks/telegram", "/hooks/slack", "/hooks/discord"] {
            let app = create_test_app_with_rate_limit(1);
            for expected in [StatusCode::NOT_FOUND, StatusCode::TOO_MANY_REQUESTS] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(path)
                            .header("content-type", "application/json")
                            .body(Body::from("{}"))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), expected);
                assert_rate_limit_headers(&response, 1, Some(0));
            }
        }
    }

    #[test]
    fn test_is_rate_limited_path() {
        assert!(is_rate_limited_path("/api/chat"));
        assert!(is_rate_limited_path("/api/tools"));
        assert!(is_rate_limited_path("/api/sessions"));
        assert!(is_rate_limited_path("/api/sessions/abc"));
        assert!(is_rate_limited_path("/api/sessions/abc/export"));
        assert!(is_rate_limited_path("/api/sessions/import"));
        assert!(is_rate_limited_path("/api/openapi.json"));
        assert!(is_rate_limited_path("/hooks/inbound"));
        assert!(is_rate_limited_path("/hooks/telegram"));
        assert!(is_rate_limited_path("/hooks/slack"));
        assert!(is_rate_limited_path("/hooks/discord"));
        assert!(!is_rate_limited_path("/health"));
        assert!(!is_rate_limited_path("/metrics"));
        assert!(!is_rate_limited_path("/"));
        assert!(!is_rate_limited_path("/api"));
    }

    async fn body_text(response: axum::http::Response<Body>) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).expect("response body utf-8")
    }

    async fn get_metrics(app: &Router) -> (StatusCode, String, Option<String>) {
        get_metrics_with_headers(app, &[]).await
    }

    async fn get_metrics_with_headers(
        app: &Router,
        headers: &[(&str, &str)],
    ) -> (StatusCode, String, Option<String>) {
        let mut builder = Request::builder().uri("/metrics");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned);
        let body = body_text(response).await;
        (status, body, content_type)
    }

    #[tokio::test]
    async fn test_metrics_counts_http_requests_and_sessions() {
        let app = create_test_app();

        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);

        let (status, body, content_type) = get_metrics(&app).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some(PROMETHEUS_CONTENT_TYPE));
        assert!(body.contains(
            "jiaclaw_http_requests_total{path=\"health\",method=\"GET\",status=\"200\"} 2"
        ));
        assert!(body.contains(
            "jiaclaw_http_requests_total{path=\"other\",method=\"POST\",status=\"200\"} 1"
        ));
        assert!(body.contains("jiaclaw_sessions_active 1"));
        assert!(body.contains(&format!(
            "jiaclaw_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(
            !body.contains("path=\"metrics\""),
            "当次 scrape 不应计入自身"
        );
    }

    #[tokio::test]
    async fn test_metrics_records_tool_calls() {
        let app = create_test_app();
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "列出工作空间".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let (status, body, _) = get_metrics(&app).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(
            "jiaclaw_http_requests_total{path=\"api_chat\",method=\"POST\",status=\"200\"} 1"
        ));
        assert!(body.contains("jiaclaw_tool_calls_total{tool=\"workspace_list\",result=\"ok\"} 1"));
    }

    #[tokio::test]
    async fn test_metrics_public_even_when_api_token_set() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);
        let (status, body, content_type) = get_metrics(&app).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some(PROMETHEUS_CONTENT_TYPE));
        assert!(body.contains("jiaclaw_build_info"));
    }

    #[tokio::test]
    async fn test_metrics_requires_auth_when_configured() {
        let app = create_test_app_with_metrics_auth(Some("test-token-123".to_string()), true);

        let (status, body, _) = get_metrics(&app).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("Unauthorized"));

        let (status, body, content_type) =
            get_metrics_with_headers(&app, &[("authorization", "Bearer test-token-123")]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some(PROMETHEUS_CONTENT_TYPE));
        assert!(body.contains("jiaclaw_build_info"));

        let health = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_metrics_is_not_rate_limited() {
        let app = create_test_app_with_rate_limit(1);

        let limited = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::OK);

        let limited = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

        for _ in 0..3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/metrics")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_no_rate_limit_headers(&response);
        }

        let health = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert_no_rate_limit_headers(&health);
    }

    fn request_id_header(response: &axum::http::Response<Body>) -> String {
        response
            .headers()
            .get(X_REQUEST_ID)
            .expect("响应必须回写 X-Request-Id")
            .to_str()
            .expect("X-Request-Id 应为 UTF-8")
            .to_string()
    }

    fn header_number(response: &axum::http::Response<Body>, name: &str) -> u64 {
        response
            .headers()
            .get(name)
            .unwrap_or_else(|| panic!("缺少响应头 {name}"))
            .to_str()
            .unwrap_or_else(|_| panic!("{name} 应为 UTF-8"))
            .parse()
            .unwrap_or_else(|_| panic!("{name} 应为整数"))
    }

    fn assert_no_rate_limit_headers(response: &axum::http::Response<Body>) {
        for name in [
            X_RATELIMIT_LIMIT,
            X_RATELIMIT_REMAINING,
            X_RATELIMIT_RESET,
            header::RETRY_AFTER.as_str(),
        ] {
            assert!(
                response.headers().get(name).is_none(),
                "限流关闭或豁免路径不应发送 {name}"
            );
        }
    }

    fn assert_rate_limit_headers(
        response: &axum::http::Response<Body>,
        limit: u32,
        expected_remaining: Option<u32>,
    ) {
        assert_eq!(header_number(response, X_RATELIMIT_LIMIT), u64::from(limit));
        let remaining = header_number(response, X_RATELIMIT_REMAINING);
        if let Some(expected) = expected_remaining {
            assert_eq!(remaining, u64::from(expected));
        } else {
            assert!(remaining <= u64::from(limit));
        }
        let reset = header_number(response, X_RATELIMIT_RESET);
        let now = unix_now_secs();
        assert!(
            reset >= now.saturating_sub(1),
            "X-RateLimit-Reset 应为当前或未来的 Unix 秒，reset={reset} now={now}"
        );
        assert!(
            reset <= now.saturating_add(120),
            "X-RateLimit-Reset 应在约一分钟窗口内，reset={reset} now={now}"
        );
    }

    #[test]
    fn test_rate_limit_reset_unix_is_window_end() {
        let replenish = Duration::from_secs(1);
        assert_eq!(
            rate_limit_reset_unix(1_700_000_000, 60, 60, replenish),
            1_700_000_000
        );
        assert_eq!(
            rate_limit_reset_unix(1_700_000_000, 59, 60, replenish),
            1_700_000_001
        );
        assert_eq!(
            rate_limit_reset_unix(1_700_000_000, 0, 60, replenish),
            1_700_000_060
        );
        let subsecond = Duration::from_millis(1500);
        assert_eq!(rate_limit_reset_unix(100, 0, 1, subsecond), 102);
    }

    #[tokio::test]
    async fn test_health_generates_request_id() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let request_id = request_id_header(&response);
        assert!(
            uuid::Uuid::parse_str(&request_id).is_ok(),
            "未提供 X-Request-Id 时应生成 UUID，实际: {request_id}"
        );
    }

    #[tokio::test]
    async fn test_request_id_echoed_when_provided() {
        let app = create_test_app();
        let client_id = "client-trace-abc-123";

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("X-Request-Id", client_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(request_id_header(&response), client_id);
    }

    #[tokio::test]
    async fn test_chat_echoes_request_id() {
        let app = create_test_app();
        let client_id = "chat-req-001";
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("X-Request-Id", client_id)
                    .body(Body::from(serde_json::to_string(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(request_id_header(&response), client_id);
    }

    #[tokio::test]
    async fn test_rate_limited_response_has_request_id() {
        let app = create_test_app_with_rate_limit(1);
        let client_id = "limited-req-9";

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .header("X-Request-Id", client_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/api/tools")
                    .header("X-Request-Id", client_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(request_id_header(&second), client_id);
        assert_rate_limit_headers(&second, 1, Some(0));
        assert!(second.headers().get(header::RETRY_AFTER).is_some());
    }

    fn small_chat_json() -> String {
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "hi".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };
        serde_json::to_string(&request).unwrap()
    }

    #[tokio::test]
    async fn test_oversize_body_returns_413_json_and_request_id() {
        let app = create_test_app_with_max_body_bytes(32);
        let client_id = "too-large-1";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("X-Request-Id", client_id)
                    .body(Body::from("x".repeat(33)))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(request_id_header(&response), client_id);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["error"], "payload_too_large");
    }

    #[tokio::test]
    async fn test_normal_body_within_limit_passes() {
        let body = small_chat_json();
        assert!(body.len() < 256);
        let app = create_test_app_with_max_body_bytes(256);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("X-Request-Id", "within-limit")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(request_id_header(&response), "within-limit");
    }

    #[tokio::test]
    async fn test_health_and_metrics_ignore_body_limit() {
        let app = create_test_app_with_max_body_bytes(8);
        for uri in ["/health", "/metrics"] {
            let get = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("X-Request-Id", "probe-get")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(get.status(), StatusCode::OK, "{uri} GET 应保持 200");
            assert_eq!(request_id_header(&get), "probe-get");

            let oversized = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("content-type", "application/json")
                        .header("X-Request-Id", "probe-body")
                        .body(Body::from("x".repeat(64)))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                oversized.status(),
                StatusCode::PAYLOAD_TOO_LARGE,
                "{uri} 不应因请求体上限返回 413"
            );
            assert_eq!(request_id_header(&oversized), "probe-body");
        }
    }

    #[tokio::test]
    async fn test_oversize_body_returns_413_before_auth() {
        let app = create_test_app_with_max_body_options(Some("secret-token".to_string()), None, 16);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("X-Request-Id", "auth-too-large")
                    .body(Body::from("x".repeat(64)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(request_id_header(&response), "auth-too-large");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["error"], "payload_too_large");
    }

    #[tokio::test]
    async fn test_oversize_body_keeps_rate_limit_headers() {
        let app = create_test_app_with_max_body_options(None, Some(30), 16);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .header("X-Request-Id", "rl-too-large")
                    .body(Body::from("x".repeat(64)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(request_id_header(&response), "rl-too-large");
        assert_rate_limit_headers(&response, 30, Some(29));
    }

    #[tokio::test]
    async fn test_cors_disabled_has_no_allow_origin() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("Origin", "http://localhost:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            cors_header(&response, "access-control-allow-origin").is_none(),
            "enabled=false 时不应发送 Access-Control-Allow-Origin"
        );
        assert!(!request_id_header(&response).is_empty());
    }

    #[tokio::test]
    async fn test_cors_enabled_echoes_matching_origin() {
        let app = create_test_app_with_cors(&matching_browser_cors());
        let origin = "http://localhost:5173";
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("Origin", origin)
                    .header("X-Request-Id", "cors-match-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            cors_header(&response, "access-control-allow-origin"),
            Some(origin)
        );
        let expose = cors_header(&response, "access-control-expose-headers").unwrap_or("");
        assert!(
            header_list_contains(expose, "x-request-id"),
            "应暴露 X-Request-Id，实际: {expose}"
        );
        for header_name in [
            "x-ratelimit-limit",
            "x-ratelimit-remaining",
            "x-ratelimit-reset",
            "retry-after",
        ] {
            assert!(
                header_list_contains(expose, header_name),
                "应暴露 {header_name}，实际: {expose}"
            );
        }
        assert_eq!(request_id_header(&response), "cors-match-1");
    }

    #[tokio::test]
    async fn test_cors_enabled_does_not_echo_unmatched_origin() {
        let app = create_test_app_with_cors(&matching_browser_cors());
        let evil = "https://evil.example";
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("Origin", evil)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let acao = cors_header(&response, "access-control-allow-origin");
        assert_ne!(acao, Some(evil), "不得回声未允许的 Origin");
        assert!(acao.is_none(), "不匹配来源时不应发送 ACAO，实际: {acao:?}");
    }

    #[tokio::test]
    async fn test_cors_wildcard_uses_star_not_request_origin() {
        let cors = HttpCorsConfig {
            enabled: true,
            allowed_origins: vec!["*".to_string()],
            ..HttpCorsConfig::default()
        };
        let app = create_test_app_with_cors(&cors);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("Origin", "https://unlisted.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            cors_header(&response, "access-control-allow-origin"),
            Some("*")
        );
    }

    #[tokio::test]
    async fn test_cors_preflight_options_for_api_chat() {
        let app = create_test_app_with_cors(&matching_browser_cors());
        let origin = "http://localhost:5173";
        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/chat")
                    .header("Origin", origin)
                    .header("Access-Control-Request-Method", "POST")
                    .header(
                        "Access-Control-Request-Headers",
                        "authorization,content-type,x-request-id,accept",
                    )
                    .header("X-Request-Id", "cors-preflight-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            cors_header(&response, "access-control-allow-origin"),
            Some(origin)
        );
        let methods = cors_header(&response, "access-control-allow-methods").unwrap_or("");
        for required in ["GET", "POST", "DELETE", "OPTIONS"] {
            assert!(
                header_list_contains(methods, required),
                "Allow-Methods 应含 {required}，实际: {methods}"
            );
        }
        let headers = cors_header(&response, "access-control-allow-headers").unwrap_or("");
        for required in ["authorization", "content-type", "x-request-id", "accept"] {
            assert!(
                header_list_contains(headers, required),
                "Allow-Headers 应含 {required}，实际: {headers}"
            );
        }
        assert_eq!(
            cors_header(&response, "access-control-max-age"),
            Some("600")
        );
        assert_eq!(request_id_header(&response), "cors-preflight-1");
    }

    #[tokio::test]
    async fn test_cors_preflight_does_not_require_api_token() {
        let app = create_test_app_with_cors_and_token(
            &matching_browser_cors(),
            Some("secret-token".to_string()),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/chat")
                    .header("Origin", "http://localhost:5173")
                    .header("Access-Control-Request-Method", "POST")
                    .header(
                        "Access-Control-Request-Headers",
                        "authorization,content-type",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            cors_header(&response, "access-control-allow-origin"),
            Some("http://localhost:5173")
        );
        assert!(!request_id_header(&response).is_empty());
    }

    #[tokio::test]
    async fn test_cors_preflight_unmatched_origin_does_not_echo() {
        let app = create_test_app_with_cors(&matching_browser_cors());
        let evil = "https://evil.example";
        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/chat")
                    .header("Origin", evil)
                    .header("Access-Control-Request-Method", "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let acao = cors_header(&response, "access-control-allow-origin");
        assert_ne!(acao, Some(evil), "preflight 不得回声未允许 Origin");
        assert!(
            acao.is_none(),
            "不匹配 preflight 不应发送 ACAO，实际: {acao:?}"
        );
    }

    #[tokio::test]
    async fn test_openapi_endpoint_returns_paths() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(request_id_header(&response)
            .chars()
            .any(|c| c.is_ascii_hexdigit() || c == '-'));

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(spec["openapi"], "3.0.3");
        let paths = spec["paths"].as_object().expect("应包含 paths");
        for required in [
            "/health",
            "/metrics",
            "/api/chat",
            "/api/sessions",
            "/api/sessions/{id}",
            "/api/sessions/{id}/export",
            "/api/sessions/import",
            "/api/tools",
            "/api/skills",
            "/api/skills/reload",
            "/api/jobs/{id}/runs/{run_id}/deliveries",
            "/api/jobs/{id}/runs/{run_id}/deliveries/cancel",
            "/hooks/inbound",
            "/hooks/telegram",
            "/hooks/slack",
            "/hooks/discord",
        ] {
            assert!(
                paths.contains_key(required),
                "OpenAPI paths 缺少 {required}"
            );
        }
        assert!(paths["/api/sessions"].get("get").is_some());
        assert!(paths["/api/sessions"].get("post").is_some());
        assert!(paths["/api/sessions/{id}"].get("get").is_some());
        assert!(paths["/api/sessions/{id}"].get("delete").is_some());
        assert!(paths["/api/sessions/{id}/export"].get("get").is_some());
        assert!(paths["/api/sessions/import"].get("post").is_some());
        assert!(paths["/api/skills/reload"].get("post").is_some());
        for header_name in [
            "XRateLimitLimit",
            "XRateLimitRemaining",
            "XRateLimitReset",
            "RetryAfter",
        ] {
            assert!(
                spec["components"]["headers"].get(header_name).is_some(),
                "OpenAPI components.headers 缺少 {header_name}"
            );
        }
        let limited = &spec["components"]["responses"]["RateLimited"]["headers"];
        assert!(limited.get("X-RateLimit-Limit").is_some());
        assert!(limited.get("X-RateLimit-Remaining").is_some());
        assert!(limited.get("X-RateLimit-Reset").is_some());
        assert!(limited.get("Retry-After").is_some());
        let too_large = &spec["components"]["responses"]["PayloadTooLarge"];
        assert!(too_large["headers"].get("X-Request-Id").is_some());
        assert_eq!(
            too_large["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/ErrorBody"
        );
        let description = spec["info"]["description"].as_str().unwrap_or("");
        assert!(
            description.contains("X-RateLimit-Limit"),
            "OpenAPI 描述应说明 X-RateLimit-* 头"
        );
        assert!(
            description.contains("Unix"),
            "OpenAPI 描述应说明 X-RateLimit-Reset 为 Unix 秒"
        );
        assert!(
            description.contains("max_body_bytes"),
            "OpenAPI 描述应说明请求体上限"
        );
        assert!(
            description.contains("413"),
            "OpenAPI 描述应说明超限返回 413"
        );
        assert_eq!(
            spec["paths"]["/api/chat"]["post"]["responses"]["413"]["$ref"],
            "#/components/responses/PayloadTooLarge"
        );
    }

    #[tokio::test]
    async fn test_openapi_requires_auth_when_token_configured() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);

        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                Request::builder()
                    .uri("/api/openapi.json")
                    .header("authorization", "Bearer test-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);

        let body = axum::body::to_bytes(authorized.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(spec["paths"].is_object());
        assert_eq!(
            spec["components"]["schemas"]["ChatRequest"]["properties"]["stream"]["type"],
            "boolean"
        );
        assert!(
            spec["paths"]["/api/chat"]["post"]["responses"]["200"]["content"]
                .get("text/event-stream")
                .is_some(),
            "OpenAPI 应描述可选 SSE"
        );
    }

    fn response_content_type(response: &axum::http::Response<Body>) -> String {
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// 解析 SSE 文本为 `(event, data JSON)`；忽略无 `data:` 的块。
    fn parse_event_stream(body: &str) -> Vec<(String, serde_json::Value)> {
        let mut events = Vec::new();
        for block in body.split("\n\n") {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }
            let mut event_name = "message".to_string();
            let mut data_lines = Vec::new();
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix("event:") {
                    event_name = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    data_lines.push(rest.trim_start().to_string());
                }
            }
            if data_lines.is_empty() {
                continue;
            }
            let raw = data_lines.join("\n");
            let data = serde_json::from_str(&raw).unwrap_or(json!({ "raw": raw }));
            events.push((event_name, data));
        }
        events
    }

    fn chat_json_body(content: &str, session_id: Option<&str>, stream: bool) -> String {
        let mut value = json!({
            "messages": [{ "role": "user", "content": content }],
            "enabled_tools": [],
            "enabled_skills": [],
            "auto_skills": true,
            "session_id": session_id,
        });
        if stream {
            value["stream"] = json!(true);
        }
        value.to_string()
    }

    #[test]
    fn test_chunk_assistant_text_reconstructs() {
        let text = "你好！我是 JiaClaw。\n这是第二段，用来验证按句分块。";
        let chunks = chunk_assistant_text(text);
        assert!(chunks.len() > 1, "应至少按句切开，实际: {chunks:?}");
        assert_eq!(chunks.concat(), text);
        assert!(chunk_assistant_text("").is_empty());
    }

    #[test]
    fn test_accept_includes_event_stream() {
        let mut headers = HeaderMap::new();
        assert!(!accept_includes_event_stream(&headers));

        headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
        assert!(!accept_includes_event_stream(&headers));

        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream;q=0.9"),
        );
        assert!(accept_includes_event_stream(&headers));
    }

    #[tokio::test]
    async fn test_chat_json_path_unchanged_without_stream() {
        let app = create_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(chat_json_body("你好", None, false)))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response_content_type(&response).starts_with("application/json"),
            "未请求流式时应保持 JSON: {}",
            response_content_type(&response)
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat_response: ChatResponse = serde_json::from_slice(&body).unwrap();
        assert!(!chat_response.message.content.is_empty());
        assert!(chat_response.session_id.is_none());
    }

    async fn post_chat_sse(
        app: Router,
        body: String,
        extra_headers: &[(&str, &str)],
    ) -> (StatusCode, String, String, String) {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/chat")
            .header("content-type", "application/json");
        for (name, value) in extra_headers {
            builder = builder.header(*name, *value);
        }
        let response = app
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let content_type = response_content_type(&response);
        let request_id = request_id_header(&response);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).expect("SSE 应为 UTF-8");
        (status, content_type, request_id, body)
    }

    #[tokio::test]
    async fn test_chat_sse_accept_header_reaches_done() {
        let app = create_test_app();
        let (status, content_type, request_id, body) = post_chat_sse(
            app,
            chat_json_body("你好", Some("sse-accept"), false),
            &[
                ("accept", "text/event-stream"),
                ("x-request-id", "sse-trace-1"),
            ],
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(
            content_type.starts_with("text/event-stream"),
            "Content-Type 应为 SSE，实际: {content_type}"
        );
        assert_eq!(request_id, "sse-trace-1");

        let events = parse_event_stream(&body);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"meta") && names.contains(&"token") && names.contains(&"done"),
            "应包含 meta/token/done，实际: {names:?}\n{body}"
        );
        assert!(
            !names.contains(&"error"),
            "成功路径不应有 error 事件: {body}"
        );

        let meta = events
            .iter()
            .find(|(n, _)| n == "meta")
            .map(|(_, d)| d)
            .expect("缺少 meta");
        assert_eq!(meta["request_id"], "sse-trace-1");
        assert_eq!(meta["session_id"], "sse-accept");

        let done = events
            .iter()
            .find(|(n, _)| n == "done")
            .map(|(_, d)| d)
            .expect("缺少 done");
        assert!(!done["reply"].as_str().unwrap_or("").is_empty());
        assert_eq!(done["session_id"], "sse-accept");
    }

    #[tokio::test]
    async fn test_chat_sse_stream_field_reaches_done_with_tool() {
        let app = create_test_app();
        let (status, content_type, _, body) =
            post_chat_sse(app, chat_json_body("列出工作空间", None, true), &[]).await;

        assert_eq!(status, StatusCode::OK);
        assert!(content_type.starts_with("text/event-stream"));

        let events = parse_event_stream(&body);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"done"),
            "应读到 done 事件，实际: {names:?}\n{body}"
        );
        assert!(names.contains(&"tool"), "工具调用应有 tool 事件: {names:?}");

        let tool = events
            .iter()
            .find(|(n, _)| n == "tool")
            .map(|(_, d)| d)
            .expect("缺少 tool");
        assert_eq!(tool["name"], "workspace_list");
        assert_eq!(tool["ok"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn test_chat_sse_auth_failure_stays_json() {
        let app = create_test_app_with_auth(Some("test-token-123".to_string()), None);
        let (status, content_type, request_id, body) = post_chat_sse(
            app,
            chat_json_body("你好", None, true),
            &[
                ("accept", "text/event-stream"),
                ("x-request-id", "sse-unauth"),
            ],
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            content_type.contains("application/json"),
            "鉴权失败应仍为 JSON，实际: {content_type}"
        );
        assert_eq!(request_id, "sse-unauth");
        assert!(
            !body.contains("event:"),
            "鉴权失败不应返回 SSE 事件流: {body}"
        );
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["error"], "Unauthorized");
    }

    #[test]
    fn test_cli_session_export_parses() {
        let cli =
            Cli::try_parse_from(["jiaclaw", "session", "export", "abc-id", "-o", "out.jsonl"])
                .expect("应解析 session export");
        match cli.command {
            Commands::Session {
                action: SessionCommands::Export { id, output, config },
            } => {
                assert_eq!(id, "abc-id");
                assert_eq!(output.as_deref(), Some(std::path::Path::new("out.jsonl")));
                assert!(config.is_none());
            }
            _ => panic!("应为 session export 子命令"),
        }
    }

    #[test]
    fn test_cli_session_export_writes_jsonl_and_404() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-export-{}.json", uuid::Uuid::new_v4()));
        let out_path =
            std::env::temp_dir().join(format!("jiaclaw-export-{}.jsonl", uuid::Uuid::new_v4()));
        let session_id = "cli-export-id";
        let mut map = HashMap::new();
        map.insert(
            session_id.to_string(),
            vec![
                ChatMessage {
                    role: MessageRole::User,
                    content: "hi".to_string(),
                },
                ChatMessage {
                    role: MessageRole::Assistant,
                    content: "hello".to_string(),
                },
            ],
        );
        save_sessions(&persist_path, &map).expect("保存 persist 失败");

        export_session_from_persist_file(&persist_path, session_id, Some(&out_path))
            .expect("导出应成功");
        let body = std::fs::read_to_string(&out_path).expect("应写入 jsonl");
        let exported = parse_jsonl_messages(&body);
        assert_eq!(exported.len(), 2);
        assert_eq!(exported[0].content, "hi");
        assert_eq!(exported[1].content, "hello");

        let err = export_session_from_persist_file(&persist_path, "missing", Some(&out_path))
            .expect_err("缺失 session 应失败");
        assert!(
            err.to_string().contains("session_not_found"),
            "错误应为 session_not_found，实际: {err}"
        );

        let before = std::fs::read_to_string(&persist_path).unwrap();
        export_session_from_persist_file(&persist_path, session_id, Some(&out_path)).unwrap();
        let after = std::fs::read_to_string(&persist_path).unwrap();
        assert_eq!(before, after, "CLI 导出不应改写 persist store");

        std::fs::remove_file(&persist_path).ok();
        std::fs::remove_file(&out_path).ok();
    }

    #[test]
    fn test_cli_session_import_parses() {
        let cli = Cli::try_parse_from([
            "jiaclaw",
            "session",
            "import",
            "in.jsonl",
            "--id",
            "abc-id",
            "--overwrite",
        ])
        .expect("应解析 session import");
        match cli.command {
            Commands::Session {
                action:
                    SessionCommands::Import {
                        file,
                        id,
                        overwrite,
                        config,
                    },
            } => {
                assert_eq!(file, std::path::PathBuf::from("in.jsonl"));
                assert_eq!(id.as_deref(), Some("abc-id"));
                assert!(overwrite);
                assert!(config.is_none());
            }
            _ => panic!("应为 session import 子命令"),
        }
    }

    #[tokio::test]
    async fn test_cli_session_import_writes_and_conflict() {
        let persist_path =
            std::env::temp_dir().join(format!("jiaclaw-import-{}.json", uuid::Uuid::new_v4()));
        let in_path =
            std::env::temp_dir().join(format!("jiaclaw-import-{}.jsonl", uuid::Uuid::new_v4()));
        std::fs::write(&in_path, sample_import_jsonl()).expect("写入 jsonl");

        let session_id = import_session_into_persist_file(
            &persist_path,
            &in_path,
            Some("cli-import-id"),
            false,
            false,
            10,
        )
        .await
        .expect("导入应成功");
        assert_eq!(session_id, "cli-import-id");
        let loaded = load_sessions(&persist_path);
        assert_eq!(
            loaded.get("cli-import-id").map(Vec::as_slice),
            Some(sample_import_messages().as_slice())
        );

        let err = import_session_into_persist_file(
            &persist_path,
            &in_path,
            Some("cli-import-id"),
            false,
            false,
            10,
        )
        .await
        .expect_err("重复导入应冲突");
        assert!(
            err.to_string().contains("session_already_exists"),
            "错误应为 session_already_exists，实际: {err}"
        );

        let replacement = "{\"role\":\"user\",\"content\":\"replaced\"}\n";
        std::fs::write(&in_path, replacement).unwrap();
        import_session_into_persist_file(
            &persist_path,
            &in_path,
            Some("cli-import-id"),
            true,
            false,
            10,
        )
        .await
        .expect("overwrite 应成功");
        let loaded = load_sessions(&persist_path);
        assert_eq!(loaded["cli-import-id"].len(), 1);
        assert_eq!(loaded["cli-import-id"][0].content, "replaced");

        std::fs::remove_file(&persist_path).ok();
        std::fs::remove_file(&in_path).ok();
    }

    #[test]
    fn test_encode_messages_jsonl_empty() {
        assert_eq!(encode_messages_jsonl(&[]).unwrap(), "");
    }

    #[test]
    fn test_cli_skills_list_still_parses() {
        let cli = Cli::try_parse_from(["jiaclaw", "skills", "--verbose"]).expect("应解析 skills");
        match cli.command {
            Commands::Skills {
                action: None,
                verbose: true,
                ..
            } => {}
            _ => panic!("应为 skills 列表"),
        }
    }

    #[test]
    fn test_cli_skills_reload_parses() {
        let cli = Cli::try_parse_from(["jiaclaw", "skills", "reload", "-c", "cfg.toml"])
            .expect("应解析 skills reload");
        match cli.command {
            Commands::Skills {
                action: Some(SkillsCommands::Reload),
                config,
                ..
            } => {
                assert_eq!(config.as_deref(), Some(std::path::Path::new("cfg.toml")));
            }
            _ => panic!("应为 skills reload 子命令"),
        }
    }
    #[tokio::test]
    async fn sqlite_http_empty_chat_delete_and_restart() {
        let root = unique_workspace("jiaclaw-http-db");
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database = root.join("state.sqlite3");
        let mut state = test_state_for_workspace(workspace.clone());
        state.persist_enabled = true;
        state.persist_path = Arc::new(database.clone());
        state.sessions = Arc::new(Mutex::new(SessionStore::open(&database).unwrap()));
        let app = build_router(state.clone());
        let id = http_create_session(&app).await;
        // The empty session is committed immediately, without waiting for a chat.
        assert!(state.sessions.lock().unwrap().get(&id).unwrap().is_some());
        http_chat_with_session(&app, &id).await;
        assert_eq!(
            state
                .sessions
                .lock()
                .unwrap()
                .get(&id)
                .unwrap()
                .unwrap()
                .messages
                .len(),
            2
        );
        drop(app);
        drop(state);
        let mut restored = test_state_for_workspace(workspace.clone());
        restored.persist_enabled = true;
        restored.persist_path = Arc::new(database.clone());
        restored.sessions = Arc::new(Mutex::new(SessionStore::open(&database).unwrap()));
        let app = build_router(restored.clone());
        assert_eq!(http_get_session_status(&app, &id).await, StatusCode::OK);
        assert!(http_delete_session(&app, &id).await.success);
        drop(app);
        drop(restored);
        assert!(SessionStore::open(&database)
            .unwrap()
            .get(&id)
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn concurrent_turns_preserve_both_histories() {
        let state = test_state_for_workspace(unique_workspace("jiaclaw-concurrent"));
        let app = build_router(state.clone());
        let id = http_create_session(&app).await;
        tokio::join!(
            http_chat_with_session(&app, &id),
            http_chat_with_session(&app, &id)
        );
        let record = state.sessions.lock().unwrap().get(&id).unwrap().unwrap();
        assert_eq!(
            record.messages.len(),
            4,
            "concurrent messages must not overwrite an earlier turn"
        );
    }

    #[tokio::test]
    async fn sqlite_write_failure_returns_error_before_success() {
        let mut state = test_state_for_workspace(unique_workspace("jiaclaw-readonly"));
        let mut store = SessionStore::open(std::path::Path::new(":memory:")).unwrap();
        if let SessionStore::Sqlite { conn, .. } = &mut store {
            conn.execute_batch("PRAGMA query_only=ON").unwrap();
        }
        state.sessions = Arc::new(Mutex::new(store));
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .method("POST")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body_text(response).await).unwrap()["error"],
            "session_storage_error"
        );
    }

    #[tokio::test]
    async fn embedded_ui_headers_and_api_auth_are_enforced() {
        let mut state = test_state_for_workspace(unique_workspace("jiaclaw-ui"));
        state.api_token = Some("test-token".into());
        let app = build_router(state);
        let page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert!(page.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'"));
        assert_eq!(page.headers()["x-content-type-options"], "nosniff");
        assert!(body_text(page).await.contains("JiaClaw"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
