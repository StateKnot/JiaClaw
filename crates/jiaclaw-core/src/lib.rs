// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `JiaClaw` 核心领域类型和契约
//!
//! 本模块定义了 `JiaClaw` 个人智能体运行时的核心类型。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod mcp;
pub use mcp::{McpConfig, McpServerConfig, McpToolConfig, McpToolEffect};

mod routing;
pub use routing::{ModelPurpose, ModelRoute, ModelRoutingConfig, ModelSelection};

mod model_calls;
pub use model_calls::ModelCallsConfig;

mod semantic;
pub use semantic::SemanticMemoryConfig;

/// 外部依赖
extern crate dirs;

/// `JiaClaw` 错误类型
#[derive(Debug, Error)]
pub enum JiaClawError {
    /// 配置错误
    #[error("配置错误: {0}")]
    Configuration(String),

    /// `StateKnot` 集成错误
    #[error("StateKnot 集成错误: {0}")]
    StateKnotIntegration(String),

    /// 无效请求
    #[error("无效请求: {0}")]
    InvalidRequest(String),

    /// 工具执行错误
    #[error("工具执行错误: {0}")]
    ToolExecution(String),
}

/// 聊天消息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage {
    /// 消息角色（user/assistant/system）
    pub role: MessageRole,

    /// 消息内容
    pub content: String,
}

/// 消息角色
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// 用户消息
    User,

    /// 助手消息
    Assistant,

    /// 系统消息
    System,
}

/// `JiaClaw` 聊天请求
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatRequest {
    /// 会话历史
    pub messages: Vec<ChatMessage>,

    /// 启用的工具列表（可选）
    #[serde(default)]
    pub enabled_tools: Vec<String>,

    /// 启用的技能列表（可选）
    #[serde(default)]
    pub enabled_skills: Vec<String>,

    /// 是否启用技能自动激活（默认 true）
    #[serde(default = "default_auto_skills")]
    pub auto_skills: bool,

    /// 会话 ID（可选，用于多轮对话）
    #[serde(default)]
    pub session_id: Option<String>,
}

fn default_auto_skills() -> bool {
    true
}

/// `JiaClaw` 聊天响应
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatResponse {
    /// 助手回复消息
    pub message: ChatMessage,

    /// 使用的工具调用（可选）
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,

    /// 运行状态
    pub status: RunStatus,

    /// 会话 ID（如果请求中提供）
    #[serde(default)]
    pub session_id: Option<String>,

    /// Operator-selected model and generation limits, without credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<ModelSelection>,
}

/// 工具调用记录
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    /// 工具名称
    pub tool_name: String,

    /// 工具参数
    pub arguments: serde_json::Value,

    /// 工具结果
    pub result: Option<serde_json::Value>,
}

/// 运行状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// 运行中
    Running,

    /// 已完成
    Completed,

    /// 失败
    Failed,

    /// 需要人工介入
    RequiresHumanInput,
}

/// Agent 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent 名称
    pub name: String,

    /// Agent 描述
    pub description: String,

    /// 系统指令
    pub system_instructions: String,

    /// 最大对话轮次
    pub max_turns: usize,

    /// 工作空间路径
    #[serde(default = "default_workspace_path")]
    pub workspace_path: std::path::PathBuf,

    /// 模型提供商配置
    #[serde(default)]
    pub provider: ProviderConfig,

    /// Optional operator-owned model selection by trusted execution purpose.
    #[serde(default)]
    pub routing: ModelRoutingConfig,

    /// Optional private model-call receipts and unknown-outcome admission hold.
    #[serde(default)]
    pub model_calls: ModelCallsConfig,

    /// HTTP 服务配置
    #[serde(default)]
    pub http: HttpConfig,

    /// 工作区长期记忆配置（缺省为 `{workspace}/MEMORY.md`）
    #[serde(default)]
    pub memory: MemoryConfig,

    /// 工作区人格 / 用户画像配置（缺省为 `{workspace}/SOUL.md` 与 `USER.md`）
    #[serde(default)]
    pub identity: IdentityConfig,

    /// 工作区心跳配置（缺省关闭；约定文件 `{workspace}/HEARTBEAT.md`）
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,

    /// Opt-in persistent background job scheduler (requires authenticated SQLite serve).
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// 会话历史溢出策略（缺省关闭摘要压缩，保持硬截断）
    #[serde(default)]
    pub session: SessionConfig,

    /// 单次工具调用超时（秒，可选，环境变量 `JIACLAW_TOOL_TIMEOUT_SECS` 优先）
    ///
    /// `None` 或非正整数表示不限制，保持现有行为。
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,

    /// 整轮 tool loop 最大迭代次数（环境变量 `JIACLAW_MAX_TOOL_ITERATIONS` 优先）
    ///
    /// 默认 [`DEFAULT_MAX_TOOL_ITERATIONS`]（与历史硬编码上限兼容）。
    /// 生效值经 [`AgentConfig::effective_max_tool_iterations`] 钳制到 1..=32。
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: usize,

    /// 本地工具配置（缺省本段不影响现有配置；`web_search` / `web_fetch` / `memory_search` / `memory_write` / `read_file` / `list_dir` / `write_file` / `delete_file` / `str_replace` / `grep` / `glob` / `mkdir` / `move` 默认启用）
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Explicitly reviewed remote MCP bindings (empty by default).
    #[serde(default)]
    pub mcp: McpConfig,

    /// 进程日志配置（缺省 `format = "text"`，与当前 tracing fmt 一致）
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Persistent background scheduling is disabled until explicitly configured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerConfig {
    /// Enable authenticated job APIs and the bounded background worker.
    #[serde(default)]
    pub enabled: bool,
    /// Only an authenticated gateway dispatch may claim a scheduled run.
    #[serde(default)]
    pub gateway_driven: bool,
}

impl SchedulerConfig {
    /// Validate the scheduler's administrator-selected execution mode.
    ///
    /// # Errors
    /// Gateway-driven execution cannot be selected while scheduling is disabled.
    pub fn validate(&self) -> Result<(), JiaClawError> {
        if self.gateway_driven && !self.enabled {
            return Err(JiaClawError::Configuration(
                "scheduler.gateway_driven requires scheduler.enabled".into(),
            ));
        }
        Ok(())
    }
}

fn default_workspace_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".jiaclaw")
        .join("workspace")
}

/// 默认工作区记忆文件名（相对于 `workspace_path`）
pub const DEFAULT_MEMORY_PATH: &str = "MEMORY.md";

/// 默认人格文件名（相对于 `workspace_path`）
pub const DEFAULT_SOUL_PATH: &str = "SOUL.md";

/// 默认用户画像文件名（相对于 `workspace_path`）
pub const DEFAULT_USER_PATH: &str = "USER.md";

/// 默认心跳文件名（相对于 `workspace_path`）
pub const DEFAULT_HEARTBEAT_PATH: &str = "HEARTBEAT.md";

/// 默认心跳会话 ID（固定会话，便于追踪）
pub const DEFAULT_HEARTBEAT_SESSION_ID: &str = "heartbeat";

/// 默认心跳间隔（秒）
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 3600;

/// `jiaclaw serve` 优雅退出时等待进行中请求的默认宽限期（秒）
pub const DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS: u64 = 15;

/// HTTP 请求体默认上限（1 MiB）。
///
/// axum 提取器另有约 2MiB 的隐式上限；JiaClaw 在中间件层默认收紧为 1MiB，
/// 并允许通过 `[http] max_body_bytes` / `JIACLAW_MAX_BODY_BYTES` 调整。
pub const DEFAULT_HTTP_MAX_BODY_BYTES: u64 = 1_048_576;

/// 默认日志级别（与当前 `EnvFilter::new("info")` 回退一致）
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// 默认工具循环上限（与历史硬编码 `MAX_ITERATIONS = 5` 保持兼容）
pub const DEFAULT_MAX_TOOL_ITERATIONS: usize = 5;

/// 工具循环上限的下限（防止配成 0 导致无法跑完一轮）
pub const MIN_MAX_TOOL_ITERATIONS: usize = 1;

/// 工具循环上限的上限（防止离谱配置引发工具风暴）
pub const MAX_MAX_TOOL_ITERATIONS: usize = 32;

fn default_max_tool_iterations() -> usize {
    DEFAULT_MAX_TOOL_ITERATIONS
}

/// 每个 session 保留的最大消息数（防止内存涨爆）
pub const MAX_SESSION_MESSAGES: usize = 50;

/// 摘要压缩时默认保留的最近消息条数
pub const DEFAULT_SESSION_KEEP_RECENT: usize = 10;

/// 注入系统提示时的最大字节数（32 KiB）；MEMORY / SOUL / USER 各自独立截断
pub const MEMORY_PROMPT_MAX_BYTES: usize = 32 * 1024;

fn default_memory_path() -> String {
    DEFAULT_MEMORY_PATH.to_string()
}

fn default_soul_path() -> String {
    DEFAULT_SOUL_PATH.to_string()
}

fn default_user_path() -> String {
    DEFAULT_USER_PATH.to_string()
}

/// 工作区长期记忆配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 记忆文件路径（相对于 `workspace_path`，默认 `MEMORY.md`）
    #[serde(default = "default_memory_path")]
    pub path: String,

    /// 默认关闭的语义检索与持久化向量索引。
    #[serde(default)]
    pub semantic: SemanticMemoryConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            path: default_memory_path(),
            semantic: SemanticMemoryConfig::default(),
        }
    }
}

/// 工作区人格与用户画像配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// 人格文件路径（相对于 `workspace_path`，默认 `SOUL.md`）
    #[serde(default = "default_soul_path")]
    pub soul_path: String,

    /// 用户画像文件路径（相对于 `workspace_path`，默认 `USER.md`）
    #[serde(default = "default_user_path")]
    pub user_path: String,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            soul_path: default_soul_path(),
            user_path: default_user_path(),
        }
    }
}

fn default_heartbeat_path() -> String {
    DEFAULT_HEARTBEAT_PATH.to_string()
}

fn default_heartbeat_session_id() -> String {
    DEFAULT_HEARTBEAT_SESSION_ID.to_string()
}

fn default_heartbeat_interval_secs() -> u64 {
    DEFAULT_HEARTBEAT_INTERVAL_SECS
}

/// 工作区心跳配置（仅 `jiaclaw serve` 进程内生效；默认关闭）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// 是否启用定时心跳（默认 `false`）
    #[serde(default)]
    pub enabled: bool,

    /// 心跳间隔（秒，默认 3600）；环境变量 `JIACLAW_HEARTBEAT_INTERVAL_SECS` 可覆盖（正整数）
    #[serde(default = "default_heartbeat_interval_secs")]
    pub interval_secs: u64,

    /// 心跳文件路径（相对于 `workspace_path`，默认 `HEARTBEAT.md`）
    #[serde(default = "default_heartbeat_path")]
    pub path: String,

    /// 固定会话 ID（默认 `heartbeat`），便于追踪
    #[serde(default = "default_heartbeat_session_id")]
    pub session_id: String,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_heartbeat_interval_secs(),
            path: default_heartbeat_path(),
            session_id: default_heartbeat_session_id(),
        }
    }
}

impl HeartbeatConfig {
    /// 解析生效的心跳间隔（秒）。
    ///
    /// 环境变量 `JIACLAW_HEARTBEAT_INTERVAL_SECS` 优先于配置文件；仅正整数生效。
    /// 未设置、`0` 或无法解析时回退到配置值；配置值非正则使用默认 3600。
    #[must_use]
    pub fn effective_interval_secs(&self) -> u64 {
        resolve_heartbeat_interval_secs(
            self.interval_secs,
            std::env::var("JIACLAW_HEARTBEAT_INTERVAL_SECS")
                .ok()
                .as_deref(),
        )
    }

    /// 生效的固定会话 ID；空白时回退到 [`DEFAULT_HEARTBEAT_SESSION_ID`]。
    #[must_use]
    pub fn effective_session_id(&self) -> &str {
        let trimmed = self.session_id.trim();
        if trimmed.is_empty() {
            DEFAULT_HEARTBEAT_SESSION_ID
        } else {
            trimmed
        }
    }
}

/// 解析正整数心跳间隔（秒）；`0` 或无法解析时返回 `None`。
#[must_use]
pub fn parse_positive_heartbeat_interval(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

/// 根据配置文件值与可选环境变量解析心跳间隔（秒）。
///
/// 环境变量优先（仅正整数）；否则使用正整数配置值；再否则默认 3600。
#[must_use]
pub fn resolve_heartbeat_interval_secs(configured: u64, env_value: Option<&str>) -> u64 {
    match env_value {
        Some(raw) => parse_positive_heartbeat_interval(raw)
            .unwrap_or_else(|| fallback_heartbeat_interval(configured)),
        None => fallback_heartbeat_interval(configured),
    }
}

fn fallback_heartbeat_interval(configured: u64) -> u64 {
    if configured > 0 {
        configured
    } else {
        DEFAULT_HEARTBEAT_INTERVAL_SECS
    }
}

fn default_session_keep_recent() -> usize {
    DEFAULT_SESSION_KEEP_RECENT
}

/// 会话历史溢出策略（硬截断 vs 可选摘要压缩）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// 接近消息上限时是否先做摘要压缩（默认 `false`，保持现有硬截断）
    ///
    /// 环境变量 `JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW=1/true` 可强制开启。
    #[serde(default)]
    pub summarize_on_overflow: bool,

    /// 摘要后保留的最近消息条数（默认 10）
    #[serde(default = "default_session_keep_recent")]
    pub keep_recent: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            summarize_on_overflow: false,
            keep_recent: default_session_keep_recent(),
        }
    }
}

impl SessionConfig {
    /// 解析是否启用摘要压缩。
    ///
    /// 环境变量 `JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW` 优先：`1`/`true`/`yes`/`on` 强制开启，
    /// `0`/`false`/`no`/`off` 强制关闭；未设置或无法解析时回退到配置文件。
    #[must_use]
    pub fn effective_summarize_on_overflow(&self) -> bool {
        resolve_session_summarize_on_overflow(
            self.summarize_on_overflow,
            std::env::var("JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW")
                .ok()
                .as_deref(),
        )
    }

    /// 摘要后保留的最近消息条数；`0` 回退默认值，并钳制到 `[1, MAX_SESSION_MESSAGES - 1]`。
    #[must_use]
    pub fn effective_keep_recent(&self) -> usize {
        resolve_session_keep_recent(self.keep_recent)
    }
}

/// 进程日志格式（`[logging] format`）。默认人类可读文本，与当前 tracing fmt 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// 人类可读文本（默认；`format=text` 时行为与当前完全一致）
    Text,
    /// 每行一条 JSON（`timestamp` / `level` / `target` / `fields` / `message`）
    Json,
}

impl Default for LogFormat {
    fn default() -> Self {
        Self::Text
    }
}

impl LogFormat {
    /// 配置与日志中使用的稳定名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for LogFormat {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LogFormat {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        parse_log_format(&raw)
            .ok_or_else(|| serde::de::Error::unknown_variant(raw.trim(), &["text", "json"]))
    }
}

/// 解析 `text` / `json`（大小写不敏感）。无法识别时返回 `None`。
#[must_use]
pub fn parse_log_format(raw: &str) -> Option<LogFormat> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "text" => Some(LogFormat::Text),
        "json" => Some(LogFormat::Json),
        _ => None,
    }
}

/// 环境变量 `JIACLAW_LOG_FORMAT` 优先；无法识别时回退配置（默认 [`LogFormat::Text`]）。
#[must_use]
pub fn resolve_log_format(configured: LogFormat, env_value: Option<&str>) -> LogFormat {
    match env_value {
        Some(raw) => parse_log_format(raw).unwrap_or(configured),
        None => configured,
    }
}

fn nonempty_log_directive(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

/// 解析生效的日志级别指令。
///
/// 优先级：`JIACLAW_LOG_LEVEL` > `RUST_LOG` > `[logging] level` > [`DEFAULT_LOG_LEVEL`]。
/// 空白视为未设置。
#[must_use]
pub fn resolve_log_level(
    configured: Option<&str>,
    jiaclaw_env: Option<&str>,
    rust_log: Option<&str>,
) -> String {
    nonempty_log_directive(jiaclaw_env)
        .or_else(|| nonempty_log_directive(rust_log))
        .or_else(|| nonempty_log_directive(configured))
        .unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string())
}

/// 进程日志配置（`[logging]`）。
///
/// 默认 `format = "text"`，保持现有人类可读 tracing fmt。`format = "json"` 时
/// 每行一条 JSON，仍走同一套 tracing 事件（含可选 `request_id` 字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// 日志格式：`text`（默认）或 `json`。环境变量 `JIACLAW_LOG_FORMAT` 优先。
    #[serde(default)]
    pub format: LogFormat,

    /// 日志级别指令（可选，例如 `info` 或 `jiaclaw=debug`）。
    ///
    /// 环境变量 `JIACLAW_LOG_LEVEL` 优先于 `RUST_LOG`，再回退本字段，最后为
    /// [`DEFAULT_LOG_LEVEL`]。
    #[serde(default)]
    pub level: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            format: LogFormat::Text,
            level: None,
        }
    }
}

impl LoggingConfig {
    /// 解析生效的日志格式。环境变量 `JIACLAW_LOG_FORMAT` 优先。
    #[must_use]
    pub fn effective_format(&self) -> LogFormat {
        resolve_log_format(
            self.format,
            std::env::var("JIACLAW_LOG_FORMAT").ok().as_deref(),
        )
    }

    /// 解析生效的日志级别指令（供 `EnvFilter` 使用）。
    #[must_use]
    pub fn effective_level_directive(&self) -> String {
        resolve_log_level(
            self.level.as_deref(),
            std::env::var("JIACLAW_LOG_LEVEL").ok().as_deref(),
            std::env::var("RUST_LOG").ok().as_deref(),
        )
    }
}

/// 解析布尔开关（`1`/`true`/`yes`/`on` 与 `0`/`false`/`no`/`off`）；无法识别时返回 `None`。
#[must_use]
pub fn parse_boolish_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// 解析摘要压缩开关；无法识别时返回 `None`。
#[must_use]
pub fn parse_session_summarize_on_overflow(raw: &str) -> Option<bool> {
    parse_boolish_flag(raw)
}

/// 解析 `JIACLAW_METRICS_REQUIRE_AUTH`；无法识别时返回 `None`。
#[must_use]
pub fn parse_metrics_require_auth(raw: &str) -> Option<bool> {
    parse_boolish_flag(raw)
}

/// 解析 `JIACLAW_CORS_ENABLED`；无法识别时返回 `None`。
#[must_use]
pub fn parse_cors_enabled(raw: &str) -> Option<bool> {
    parse_boolish_flag(raw)
}

/// 解析逗号分隔的 CORS 来源列表；空白项丢弃。
#[must_use]
pub fn parse_cors_origins(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// 根据配置文件值与可选环境变量解析 CORS 开关。
///
/// 环境变量 `JIACLAW_CORS_ENABLED` 优先：可解析的 `1`/`true`/`yes`/`on` 与
/// `0`/`false`/`no`/`off` 覆盖配置；未设置或无法解析时回退配置值（默认关闭）。
#[must_use]
pub fn resolve_cors_enabled(configured: bool, env_value: Option<&str>) -> bool {
    match env_value {
        Some(raw) => parse_cors_enabled(raw).unwrap_or(configured),
        None => configured,
    }
}

/// 根据配置文件值与可选环境变量解析 CORS 允许来源。
///
/// 环境变量 `JIACLAW_CORS_ORIGINS`（逗号分隔）一旦设置即覆盖配置列表。
/// 空白项丢弃；`*` 只有出现在列表中才表示允许所有来源。
#[must_use]
pub fn resolve_cors_origins(configured: Vec<String>, env_value: Option<&str>) -> Vec<String> {
    match env_value {
        Some(raw) => parse_cors_origins(raw),
        None => configured
            .into_iter()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
    }
}

/// 根据配置文件 `metrics_public` 与可选环境变量解析 `/metrics` 是否公开。
///
/// 环境变量 `JIACLAW_METRICS_REQUIRE_AUTH` 优先：`1`/`true` 表示要求鉴权（不公开），
/// `0`/`false` 表示公开；未设置或无法解析时回退到配置值（默认公开）。
#[must_use]
pub fn resolve_metrics_public(configured_public: bool, env_require_auth: Option<&str>) -> bool {
    match env_require_auth {
        Some(raw) => {
            parse_metrics_require_auth(raw).map_or(configured_public, |require_auth| !require_auth)
        }
        None => configured_public,
    }
}

/// 根据配置文件值与可选环境变量解析摘要压缩开关。
///
/// 可解析的环境变量优先；否则使用配置值。
#[must_use]
pub fn resolve_session_summarize_on_overflow(configured: bool, env_value: Option<&str>) -> bool {
    match env_value {
        Some(raw) => parse_session_summarize_on_overflow(raw).unwrap_or(configured),
        None => configured,
    }
}

/// 钳制 `keep_recent`：`0` 使用默认 10，且不超过 `MAX_SESSION_MESSAGES - 1`。
#[must_use]
pub fn resolve_session_keep_recent(configured: usize) -> usize {
    let raw = if configured == 0 {
        DEFAULT_SESSION_KEEP_RECENT
    } else {
        configured
    };
    raw.clamp(1, MAX_SESSION_MESSAGES.saturating_sub(1))
}

fn default_web_search_enabled() -> bool {
    true
}

fn default_web_fetch_enabled() -> bool {
    true
}

fn default_memory_search_enabled() -> bool {
    true
}

fn default_memory_write_enabled() -> bool {
    true
}

fn default_read_file_enabled() -> bool {
    true
}

fn default_list_dir_enabled() -> bool {
    true
}

fn default_write_file_enabled() -> bool {
    true
}

fn default_delete_file_enabled() -> bool {
    true
}

fn default_str_replace_enabled() -> bool {
    true
}

fn default_grep_enabled() -> bool {
    true
}

fn default_glob_enabled() -> bool {
    true
}

fn default_mkdir_enabled() -> bool {
    true
}

fn default_move_enabled() -> bool {
    true
}

/// 本地工具总配置（缺省本段不影响现有 `[http]` / `[memory]` 等段）
///
/// 历史示例里的 `[tools] enabled = [...]` 列表仍可出现在文件中（未知字段忽略），
/// 当前真正生效的是嵌套表 `[tools.web_search]`、`[tools.web_fetch]`、
/// `[tools.memory_search]`、`[tools.memory_write]`、`[tools.read_file]`、`[tools.list_dir]`、`[tools.write_file]`、`[tools.delete_file]`、`[tools.str_replace]`、`[tools.grep]`、`[tools.glob]`、`[tools.mkdir]`、`[tools.move]`、`[tools.stat]` 与 `[tools.tree]`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// 原子复制文件，默认启用。
    #[serde(default)]
    pub copy: CopyToolConfig,

    /// 容器隔离的命令执行，默认关闭。
    #[serde(default)]
    pub exec: ExecToolConfig,

    /// `web_search` 工具配置
    #[serde(default)]
    pub web_search: WebSearchToolConfig,

    /// `web_fetch` 工具配置
    #[serde(default)]
    pub web_fetch: WebFetchToolConfig,

    /// `memory_search` 工具配置
    #[serde(default)]
    pub memory_search: MemorySearchToolConfig,

    /// `memory_write` 工具配置
    #[serde(default)]
    pub memory_write: MemoryWriteToolConfig,

    /// `read_file` 工具配置
    #[serde(default)]
    pub read_file: ReadFileToolConfig,

    /// `list_dir` 工具配置
    #[serde(default)]
    pub list_dir: ListDirToolConfig,

    /// `stat` 工作区元数据工具配置。
    #[serde(default)]
    pub stat: StatToolConfig,

    /// `tree` 有界工作区目录树工具配置。
    #[serde(default)]
    pub tree: TreeToolConfig,

    /// `write_file` 工具配置
    #[serde(default)]
    pub write_file: WriteFileToolConfig,

    /// `delete_file` 工具配置
    #[serde(default)]
    pub delete_file: DeleteFileToolConfig,

    /// `str_replace` 工具配置
    #[serde(default)]
    pub str_replace: StrReplaceToolConfig,

    /// `grep` 工具配置
    #[serde(default)]
    pub grep: GrepToolConfig,

    /// `glob` 工具配置
    #[serde(default)]
    pub glob: GlobToolConfig,

    /// `mkdir` 工具配置
    #[serde(default)]
    pub mkdir: MkdirToolConfig,

    /// `move` 工具配置（Rust 关键字，字段名为 `r#move`，序列化为 `move`）
    #[serde(default)]
    pub r#move: MoveToolConfig,
}

/// 文件复制开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyToolConfig {
    /// 是否注册 copy 工具及 `file_copy` 兼容名称。
    #[serde(default = "default_move_enabled")]
    pub enabled: bool,
}

impl Default for CopyToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Docker 沙箱执行配置。必须显式启用并指定不可变镜像。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecToolConfig {
    /// 是否注册 exec / `shell_exec`，默认 false。
    pub enabled: bool,
    /// 本机 Docker CLI 的绝对路径。
    pub docker_path: std::path::PathBuf,
    /// 预先拉取的 image@sha256:digest，不允许运行时拉取。
    pub image: String,
    /// 模型可用的命令名到容器内绝对可执行文件的映射。
    pub commands: std::collections::BTreeMap<String, String>,
    /// 包括创建容器的总期限，1..=300 秒。
    pub timeout_secs: u64,
    /// stdout/stderr 各自的最大保留字节数，1..=1048576。
    pub max_output_bytes: usize,
    /// 是否只读挂载工作区，默认 true。
    pub workspace_read_only: bool,
    /// 容器 UID:GID，必须为非 root 数字。
    pub user: String,
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            docker_path: "/usr/bin/docker".into(),
            image: String::new(),
            commands: std::collections::BTreeMap::new(),
            timeout_secs: 30,
            max_output_bytes: 65536,
            workspace_read_only: true,
            user: "65534:65534".into(),
        }
    }
}

/// 可选 `web_search` 联网检索配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchToolConfig {
    /// 是否注册 `web_search` 工具（默认 `true`）
    #[serde(default = "default_web_search_enabled")]
    pub enabled: bool,

    /// Brave Search API key（可选，环境变量 `JIACLAW_BRAVE_API_KEY` 优先）
    ///
    /// 日志与 `jiaclaw doctor` 只报告是否已配置，不打印明文。
    #[serde(default)]
    pub brave_api_key: Option<String>,
}

impl Default for WebSearchToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_search_enabled(),
            brave_api_key: None,
        }
    }
}

impl WebSearchToolConfig {
    /// 解析生效的 Brave API key。
    ///
    /// 环境变量 `JIACLAW_BRAVE_API_KEY` 优先于配置文件；空白视为未配置。
    #[must_use]
    pub fn effective_brave_api_key(&self) -> Option<String> {
        resolve_optional_secret(
            self.brave_api_key.clone(),
            std::env::var("JIACLAW_BRAVE_API_KEY").ok().as_deref(),
        )
    }
}

/// 可选 `web_fetch` 网页抓取配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchToolConfig {
    /// 是否注册 `web_fetch` 工具（默认 `true`）
    #[serde(default = "default_web_fetch_enabled")]
    pub enabled: bool,

    /// 是否允许抓取 localhost / 私网地址（默认 `false`）
    ///
    /// 默认拒绝 `127.0.0.0/8`、`::1`、`10/8`、`172.16/12`、`192.168/16` 以及
    /// 链路本地地址。测试或内网场景可设为 `true`。
    #[serde(default)]
    pub allow_private: bool,
}

impl Default for WebFetchToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_fetch_enabled(),
            allow_private: false,
        }
    }
}

/// 可选 `memory_search` 工作区记忆检索配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchToolConfig {
    /// 是否注册 `memory_search` 工具（默认 `true`）
    #[serde(default = "default_memory_search_enabled")]
    pub enabled: bool,
}

impl Default for MemorySearchToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_memory_search_enabled(),
        }
    }
}

/// 可选 `memory_write` 工作区记忆写入配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWriteToolConfig {
    /// 是否注册 `memory_write` 工具（默认 `true`）
    #[serde(default = "default_memory_write_enabled")]
    pub enabled: bool,
}

impl Default for MemoryWriteToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_memory_write_enabled(),
        }
    }
}

/// 可选 `read_file` 工作区只读文件配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileToolConfig {
    /// 是否注册 `read_file` 工具（默认 `true`）
    #[serde(default = "default_read_file_enabled")]
    pub enabled: bool,
}

impl Default for ReadFileToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_read_file_enabled(),
        }
    }
}

/// 可选 `list_dir` 工作区列目录配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirToolConfig {
    /// 是否注册 `list_dir` 工具（默认 `true`）
    #[serde(default = "default_list_dir_enabled")]
    pub enabled: bool,
}

impl Default for ListDirToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_list_dir_enabled(),
        }
    }
}

/// 工作区元数据检查工具开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatToolConfig {
    /// 是否注册 `stat` 工具，默认 true。
    pub enabled: bool,
}

impl Default for StatToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// 有界工作区目录树工具开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TreeToolConfig {
    /// 是否注册 `tree` 工具，默认 true。
    pub enabled: bool,
}

impl Default for TreeToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// 可选 `write_file` 工作区文件写入配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileToolConfig {
    /// 是否注册 `write_file` 工具（默认 `true`）
    #[serde(default = "default_write_file_enabled")]
    pub enabled: bool,
}

impl Default for WriteFileToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_write_file_enabled(),
        }
    }
}

/// 可选 `delete_file` 工作区文件删除配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileToolConfig {
    /// 是否注册 `delete_file` 工具（默认 `true`）
    #[serde(default = "default_delete_file_enabled")]
    pub enabled: bool,
}

impl Default for DeleteFileToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_delete_file_enabled(),
        }
    }
}

/// 可选 `str_replace` 工作区精确字符串替换配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrReplaceToolConfig {
    /// 是否注册 `str_replace` 工具（默认 `true`）
    #[serde(default = "default_str_replace_enabled")]
    pub enabled: bool,
}

impl Default for StrReplaceToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_str_replace_enabled(),
        }
    }
}

/// 可选 `grep` 工作区文本搜索配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepToolConfig {
    /// 是否注册 `grep` 工具（默认 `true`）
    #[serde(default = "default_grep_enabled")]
    pub enabled: bool,
}

impl Default for GrepToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_grep_enabled(),
        }
    }
}

/// 可选 `glob` 工作区按模式找文件配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobToolConfig {
    /// 是否注册 `glob` 工具（默认 `true`）
    #[serde(default = "default_glob_enabled")]
    pub enabled: bool,
}

impl Default for GlobToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_glob_enabled(),
        }
    }
}

/// 可选 `mkdir` 工作区创建目录配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MkdirToolConfig {
    /// 是否注册 `mkdir` 工具（默认 `true`）
    #[serde(default = "default_mkdir_enabled")]
    pub enabled: bool,
}

impl Default for MkdirToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_mkdir_enabled(),
        }
    }
}

/// 可选 `move` 工作区移动 / 重命名配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveToolConfig {
    /// 是否注册 `move` 工具（默认 `true`）
    #[serde(default = "default_move_enabled")]
    pub enabled: bool,
}

impl Default for MoveToolConfig {
    fn default() -> Self {
        Self {
            enabled: default_move_enabled(),
        }
    }
}

/// 可选浏览器 CORS 配置（`[http.cors]`）。
///
/// 默认关闭：不发送任何 CORS 头，行为与未配置时一致。开启后才处理
/// `Origin` / OPTIONS preflight；`*` 仅在 `allowed_origins` 中显式写出时生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpCorsConfig {
    /// 是否启用 CORS（默认 `false`）。环境变量 `JIACLAW_CORS_ENABLED` 优先。
    #[serde(default)]
    pub enabled: bool,

    /// 允许的精确 Origin 列表。环境变量 `JIACLAW_CORS_ORIGINS`（逗号分隔）优先。
    ///
    /// 仅当列表中显式包含 `*` 时才允许所有来源；空列表不回声任何 Origin。
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// 允许的 HTTP 方法。空则回退默认：GET / POST / DELETE / OPTIONS。
    #[serde(default = "default_cors_allowed_methods")]
    pub allowed_methods: Vec<String>,

    /// 允许的请求头。空则回退默认：`Authorization`、`Content-Type`、`X-Request-Id`、`Accept`。
    #[serde(default = "default_cors_allowed_headers")]
    pub allowed_headers: Vec<String>,

    /// 暴露给浏览器的响应头。空则回退默认：`X-Request-Id` 与限流头
    /// （`X-RateLimit-Limit` / `X-RateLimit-Remaining` / `X-RateLimit-Reset` / `Retry-After`）。
    #[serde(default = "default_cors_expose_headers")]
    pub expose_headers: Vec<String>,

    /// Preflight 缓存秒数（可选）。`None` / `0` 表示不发送 `Access-Control-Max-Age`。
    #[serde(default)]
    pub max_age_secs: Option<u64>,
}

fn default_cors_allowed_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "DELETE".to_string(),
        "OPTIONS".to_string(),
    ]
}

fn default_cors_allowed_headers() -> Vec<String> {
    vec![
        "Authorization".to_string(),
        "Content-Type".to_string(),
        "X-Request-Id".to_string(),
        "Accept".to_string(),
    ]
}

fn default_cors_expose_headers() -> Vec<String> {
    vec![
        "X-Request-Id".to_string(),
        "X-RateLimit-Limit".to_string(),
        "X-RateLimit-Remaining".to_string(),
        "X-RateLimit-Reset".to_string(),
        "Retry-After".to_string(),
    ]
}

fn nonempty_or_default(values: &[String], fallback: Vec<String>) -> Vec<String> {
    let trimmed: Vec<String> = values
        .iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

impl Default for HttpCorsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_origins: Vec::new(),
            allowed_methods: default_cors_allowed_methods(),
            allowed_headers: default_cors_allowed_headers(),
            expose_headers: default_cors_expose_headers(),
            max_age_secs: None,
        }
    }
}

impl HttpCorsConfig {
    /// 解析生效的 CORS 开关。环境变量 `JIACLAW_CORS_ENABLED` 优先。
    #[must_use]
    pub fn effective_enabled(&self) -> bool {
        resolve_cors_enabled(
            self.enabled,
            std::env::var("JIACLAW_CORS_ENABLED").ok().as_deref(),
        )
    }

    /// 解析生效的允许来源。环境变量 `JIACLAW_CORS_ORIGINS` 优先。
    #[must_use]
    pub fn effective_allowed_origins(&self) -> Vec<String> {
        resolve_cors_origins(
            self.allowed_origins.clone(),
            std::env::var("JIACLAW_CORS_ORIGINS").ok().as_deref(),
        )
    }

    /// 解析生效的允许方法；空配置回退默认 GET/POST/DELETE/OPTIONS。
    #[must_use]
    pub fn effective_allowed_methods(&self) -> Vec<String> {
        nonempty_or_default(&self.allowed_methods, default_cors_allowed_methods())
    }

    /// 解析生效的允许请求头；空配置回退默认四项。
    #[must_use]
    pub fn effective_allowed_headers(&self) -> Vec<String> {
        nonempty_or_default(&self.allowed_headers, default_cors_allowed_headers())
    }

    /// 解析生效的暴露响应头；空配置回退 `X-Request-Id` 与限流头。
    #[must_use]
    pub fn effective_expose_headers(&self) -> Vec<String> {
        nonempty_or_default(&self.expose_headers, default_cors_expose_headers())
    }

    /// 解析生效的 preflight 缓存秒数；`0` 视为未设置。
    #[must_use]
    pub fn effective_max_age_secs(&self) -> Option<u64> {
        self.max_age_secs.filter(|&secs| secs > 0)
    }

    /// 应用环境变量覆盖后的 CORS 配置（serve / 中间件构建时调用）。
    #[must_use]
    pub fn resolved(&self) -> Self {
        Self {
            enabled: self.effective_enabled(),
            allowed_origins: self.effective_allowed_origins(),
            allowed_methods: self.effective_allowed_methods(),
            allowed_headers: self.effective_allowed_headers(),
            expose_headers: self.effective_expose_headers(),
            max_age_secs: self.effective_max_age_secs(),
        }
    }

    /// 当前来源列表是否显式包含 `*`（不读取环境变量；请先 `resolved()`）。
    #[must_use]
    pub fn allows_any_origin(&self) -> bool {
        self.allowed_origins.iter().any(|origin| origin == "*")
    }
}

/// An exact destination authorized for scheduled notifications, independently of ingress.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledChannelDestination {
    /// Exact platform chat/channel ID.
    pub conversation_id: String,
    /// Exact topic/thread, or None for a new top-level message.
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// An explicitly authorized installation for unattended channel work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBinding {
    /// Supported platform: telegram, slack, discord, feishu, wecom or dingtalk.
    pub channel: String,
    /// Telegram bot ID, Slack team ID, Discord app ID, Feishu app_id:tenant_key WeCom corp_id:agent_id or DingTalk robotCode:corpId.
    pub installation_id: String,
    #[serde(default)]
    /// Slack application ID, or DingTalk Client ID (separate from robotCode).
    pub app_id: Option<String>,
    /// Exact Discord guild authorized for Bot scheduled sends; absent disables them.
    #[serde(default)]
    pub discord_guild_id: Option<String>,
    /// Exact platform user IDs authorized to invoke this agent.
    pub allowed_senders: Vec<String>,
    /// Exact platform chat/channel IDs authorized for ingress and reply.
    pub allowed_conversations: Vec<String>,
    /// Exact targets for scheduled notifications; empty disables proactive delivery.
    #[serde(default)]
    pub scheduled_destinations: Vec<ScheduledChannelDestination>,
    /// Nonempty list of registered, qualified background tools.
    pub enabled_tools: Vec<String>,
    #[serde(default = "default_channel_timeout")]
    /// Total model run deadline, including session lock wait and compaction.
    pub timeout_secs: u64,
    /// Explicit local HTTP fixture endpoint; production uses the platform URL.
    #[serde(default)]
    pub local_test_api_base: Option<String>,
}

fn default_channel_timeout() -> u64 {
    120
}

/// HTTP 服务配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    /// Enable the private gateway channel chat protocol with fixed read-only tools.
    #[serde(default)]
    pub gateway_channel_chat: bool,
    /// Channels are closed until an explicit installation and identity policy exists.
    #[serde(default)]
    pub channels: Vec<ChannelBinding>,
    /// HTTP 服务绑定地址
    #[serde(default = "default_http_bind")]
    pub bind: String,

    /// API 鉴权 Token（可选，环境变量 `JIACLAW_API_TOKEN` 优先）
    #[serde(default)]
    pub api_token: Option<String>,

    /// Webhook 鉴权密钥（可选，环境变量 `JIACLAW_WEBHOOK_SECRET` 优先）
    #[serde(default)]
    pub webhook_secret: Option<String>,

    /// Telegram Bot webhook secret token（可选，环境变量 `JIACLAW_TELEGRAM_SECRET` 优先）
    ///
    /// 对应 Telegram `setWebhook` 的 `secret_token`，校验头 `X-Telegram-Bot-Api-Secret-Token`。
    #[serde(default)]
    pub telegram_secret: Option<String>,

    /// Telegram Bot API token（可选，环境变量 `JIACLAW_TELEGRAM_BOT_TOKEN` 优先）
    ///
    /// 须同时配置 channels 安装/身份/工具策略；回复通过持久 outbox 投递。
    #[serde(default)]
    pub telegram_bot_token: Option<String>,

    /// Slack Events API signing secret（可选，环境变量 `JIACLAW_SLACK_SIGNING_SECRET` 优先）
    ///
    /// 配置后，`POST /hooks/slack` 校验 `X-Slack-Signature` + `X-Slack-Request-Timestamp`
    ///（官方 v0 HMAC-SHA256，时间窗 ±5 分钟）。未配置则关闭。
    #[serde(default)]
    pub slack_signing_secret: Option<String>,

    /// Slack Bot token（可选，环境变量 `JIACLAW_SLACK_BOT_TOKEN` 优先）
    ///
    /// 须同时配置 channels 安装/身份/工具策略；回复通过持久 outbox 投递。
    #[serde(default)]
    pub slack_bot_token: Option<String>,

    /// Feishu self-built application secret (JIACLAW_FEISHU_APP_SECRET overrides).
    #[serde(default)]
    pub feishu_app_secret: Option<String>,
    /// Feishu callback Encrypt Key (JIACLAW_FEISHU_ENCRYPT_KEY overrides).
    #[serde(default)]
    pub feishu_encrypt_key: Option<String>,
    /// Feishu callback Verification Token (JIACLAW_FEISHU_VERIFICATION_TOKEN overrides).
    #[serde(default)]
    pub feishu_verification_token: Option<String>,

    /// WeCom application secret (JIACLAW_WECOM_APP_SECRET overrides).
    #[serde(default)]
    pub wecom_app_secret: Option<String>,
    /// DingTalk internal-app Client Secret; JIACLAW_DINGTALK_APP_SECRET overrides it.
    pub dingtalk_app_secret: Option<String>,
    /// WeCom callback token (JIACLAW_WECOM_CALLBACK_TOKEN overrides).
    #[serde(default)]
    pub wecom_callback_token: Option<String>,
    /// WeCom callback EncodingAESKey (JIACLAW_WECOM_ENCODING_AES_KEY overrides).
    #[serde(default)]
    pub wecom_encoding_aes_key: Option<String>,

    /// Discord Interactions 公钥（可选，环境变量 `JIACLAW_DISCORD_PUBLIC_KEY` 优先）
    ///
    /// 配置后，`POST /hooks/discord` 校验 `X-Signature-Ed25519` + `X-Signature-Timestamp`
    ///（官方 Ed25519，签名消息为 `timestamp + raw body`）。未配置则关闭。
    #[serde(default)]
    pub discord_public_key: Option<String>,

    /// Discord Bot token（可选，环境变量 `JIACLAW_DISCORD_BOT_TOKEN` 优先）
    ///
    /// 仅用于显式授权的定时 Bot 通知；interaction 回复继续使用加密保存的 interaction token。
    #[serde(default)]
    pub discord_bot_token: Option<String>,

    /// 可选浏览器 CORS（默认关闭）。详见 [`HttpCorsConfig`]。
    #[serde(default)]
    pub cors: HttpCorsConfig,

    /// 是否使用 SQLite 持久化 session（默认 true）
    #[serde(default = "default_http_persist")]
    pub persist: bool,

    /// Session 持久化文件路径（相对于 `workspace_path`）
    #[serde(default = "default_persist_path")]
    pub persist_path: String,

    /// 每分钟全局限流（可选，环境变量 `JIACLAW_RATE_LIMIT_PER_MINUTE` 优先）
    ///
    /// `None` 或非正整数表示不限流。
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,

    /// 会话闲置 TTL（秒，可选，环境变量 `JIACLAW_SESSION_TTL_SECS` 优先）
    ///
    /// `None` 或非正整数表示不启用过期清理。
    #[serde(default)]
    pub session_ttl_secs: Option<u64>,

    /// 优雅退出宽限期（秒，环境变量 `JIACLAW_SHUTDOWN_TIMEOUT_SECS` 优先）
    ///
    /// 收到 SIGINT/SIGTERM 后停止 accept，并等待进行中请求结束；超时则丢弃剩余连接。
    /// 正整数生效；未设置、`0` 或无法解析时回退 [`DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS`]。
    #[serde(default = "default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,

    /// HTTP 请求体上限（字节，环境变量 `JIACLAW_MAX_BODY_BYTES` 优先）
    ///
    /// 超限返回 413。正整数生效；未设置、`0` 或无法解析时回退 [`DEFAULT_HTTP_MAX_BODY_BYTES`]。
    /// `GET /health` 与 `GET /metrics` 不检查该上限。
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: u64,

    /// `GET /metrics` 是否无需 API Bearer（默认 `true`，便于 Prometheus scrape）。
    ///
    /// 设为 `false` 时与 `/api/*` 相同鉴权。环境变量 `JIACLAW_METRICS_REQUIRE_AUTH=1`
    /// 优先，表示要求鉴权（即不公开）。
    #[serde(default = "default_metrics_public")]
    pub metrics_public: bool,
}

fn default_http_bind() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_http_persist() -> bool {
    true
}

fn default_persist_path() -> String {
    "../state/sessions.sqlite3".to_string()
}

fn default_metrics_public() -> bool {
    true
}

fn default_shutdown_timeout_secs() -> u64 {
    DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
}

fn default_max_body_bytes() -> u64 {
    DEFAULT_HTTP_MAX_BODY_BYTES
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            gateway_channel_chat: false,
            channels: vec![],
            bind: default_http_bind(),
            api_token: None,
            webhook_secret: None,
            telegram_secret: None,
            telegram_bot_token: None,
            slack_signing_secret: None,
            slack_bot_token: None,
            feishu_app_secret: None,
            feishu_encrypt_key: None,
            feishu_verification_token: None,
            wecom_app_secret: None,
            dingtalk_app_secret: None,
            wecom_callback_token: None,
            wecom_encoding_aes_key: None,
            discord_public_key: None,
            discord_bot_token: None,
            cors: HttpCorsConfig::default(),
            persist: true,
            persist_path: default_persist_path(),
            rate_limit_per_minute: None,
            session_ttl_secs: None,
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
            max_body_bytes: default_max_body_bytes(),
            metrics_public: default_metrics_public(),
        }
    }
}

/// 解析可选密钥：环境变量优先；空白视为未设置并回退到配置文件。
#[must_use]
pub fn resolve_optional_secret(
    configured: Option<String>,
    env_value: Option<&str>,
) -> Option<String> {
    env_value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            configured
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

/// 解析正整数限流值；`0` 或无法解析时视为不限流。
#[must_use]
pub fn parse_positive_rate_limit(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok().filter(|&n| n > 0)
}

/// 根据配置文件值与可选环境变量解析每分钟限流。
///
/// 环境变量优先；仅正整数生效。
#[must_use]
pub fn resolve_rate_limit_per_minute(
    configured: Option<u32>,
    env_value: Option<&str>,
) -> Option<u32> {
    match env_value {
        Some(raw) => parse_positive_rate_limit(raw),
        None => configured.filter(|&n| n > 0),
    }
}

/// 解析正整数会话 TTL（秒）；`0` 或无法解析时视为不启用。
#[must_use]
pub fn parse_positive_session_ttl(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

/// 根据配置文件值与可选环境变量解析会话闲置 TTL（秒）。
///
/// 环境变量优先；仅正整数生效。
#[must_use]
pub fn resolve_session_ttl_secs(configured: Option<u64>, env_value: Option<&str>) -> Option<u64> {
    match env_value {
        Some(raw) => parse_positive_session_ttl(raw),
        None => configured.filter(|&n| n > 0),
    }
}

/// 解析正整数优雅退出宽限期（秒）；`0` 或无法解析时返回 `None`。
#[must_use]
pub fn parse_positive_shutdown_timeout(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

fn fallback_shutdown_timeout(configured: u64) -> u64 {
    if configured > 0 {
        configured
    } else {
        DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
    }
}

/// 根据配置文件值与可选环境变量解析优雅退出宽限期（秒）。
///
/// 环境变量 `JIACLAW_SHUTDOWN_TIMEOUT_SECS` 优先（仅正整数）；非法 / `0` 回退配置，
/// 配置亦非正整数时回退 [`DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS`]。
#[must_use]
pub fn resolve_shutdown_timeout_secs(configured: u64, env_value: Option<&str>) -> u64 {
    match env_value {
        Some(raw) => parse_positive_shutdown_timeout(raw)
            .unwrap_or_else(|| fallback_shutdown_timeout(configured)),
        None => fallback_shutdown_timeout(configured),
    }
}

/// 解析正整数请求体上限（字节）；`0` 或无法解析时返回 `None`。
#[must_use]
pub fn parse_positive_max_body_bytes(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

fn fallback_max_body_bytes(configured: u64) -> u64 {
    if configured > 0 {
        configured
    } else {
        DEFAULT_HTTP_MAX_BODY_BYTES
    }
}

/// 根据配置文件值与可选环境变量解析 HTTP 请求体上限（字节）。
///
/// 环境变量 `JIACLAW_MAX_BODY_BYTES` 优先（仅正整数）；非法 / `0` 回退配置，
/// 配置亦非正整数时回退 [`DEFAULT_HTTP_MAX_BODY_BYTES`]。
#[must_use]
pub fn resolve_max_body_bytes(configured: u64, env_value: Option<&str>) -> u64 {
    match env_value {
        Some(raw) => parse_positive_max_body_bytes(raw)
            .unwrap_or_else(|| fallback_max_body_bytes(configured)),
        None => fallback_max_body_bytes(configured),
    }
}

/// 解析正整数工具超时（秒）；`0` 或无法解析时视为不启用。
#[must_use]
pub fn parse_positive_tool_timeout(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

/// 根据配置文件值与可选环境变量解析单次工具调用超时（秒）。
///
/// 环境变量优先；仅正整数生效。
#[must_use]
pub fn resolve_tool_timeout_secs(configured: Option<u64>, env_value: Option<&str>) -> Option<u64> {
    match env_value {
        Some(raw) => parse_positive_tool_timeout(raw),
        None => configured.filter(|&n| n > 0),
    }
}

/// 解析正整数工具循环上限；`0` 或无法解析时返回 `None`。
#[must_use]
pub fn parse_positive_max_tool_iterations(raw: &str) -> Option<usize> {
    raw.trim().parse::<usize>().ok().filter(|&n| n > 0)
}

fn fallback_max_tool_iterations(configured: usize) -> usize {
    if configured == 0 {
        DEFAULT_MAX_TOOL_ITERATIONS
    } else {
        configured
    }
}

/// 根据配置文件值与可选环境变量解析工具循环上限。
///
/// 环境变量 `JIACLAW_MAX_TOOL_ITERATIONS` 优先（仅正整数；`0`/非法忽略）。
/// 配置值为 `0` 时回退默认 5。最终钳制到 `[1, 32]`。
#[must_use]
pub fn resolve_max_tool_iterations(configured: usize, env_value: Option<&str>) -> usize {
    let raw = match env_value {
        Some(raw) => parse_positive_max_tool_iterations(raw)
            .unwrap_or_else(|| fallback_max_tool_iterations(configured)),
        None => fallback_max_tool_iterations(configured),
    };
    raw.clamp(MIN_MAX_TOOL_ITERATIONS, MAX_MAX_TOOL_ITERATIONS)
}

impl HttpConfig {
    /// 解析生效的每分钟请求上限。
    ///
    /// 环境变量 `JIACLAW_RATE_LIMIT_PER_MINUTE` 优先于配置文件。
    /// 仅正整数生效；未设置、`0` 或无法解析表示不限流。
    #[must_use]
    pub fn effective_rate_limit_per_minute(&self) -> Option<u32> {
        resolve_rate_limit_per_minute(
            self.rate_limit_per_minute,
            std::env::var("JIACLAW_RATE_LIMIT_PER_MINUTE")
                .ok()
                .as_deref(),
        )
    }

    /// 解析生效的会话闲置 TTL（秒）。
    ///
    /// 环境变量 `JIACLAW_SESSION_TTL_SECS` 优先于配置文件。
    /// 仅正整数生效；未设置、`0` 或无法解析表示不启用。
    #[must_use]
    pub fn effective_session_ttl_secs(&self) -> Option<u64> {
        resolve_session_ttl_secs(
            self.session_ttl_secs,
            std::env::var("JIACLAW_SESSION_TTL_SECS").ok().as_deref(),
        )
    }

    /// 解析生效的优雅退出宽限期（秒）。
    ///
    /// 环境变量 `JIACLAW_SHUTDOWN_TIMEOUT_SECS` 优先于配置文件。
    /// 仅正整数生效；非法 / `0` 回退配置或默认 15 秒。
    #[must_use]
    pub fn effective_shutdown_timeout_secs(&self) -> u64 {
        resolve_shutdown_timeout_secs(
            self.shutdown_timeout_secs,
            std::env::var("JIACLAW_SHUTDOWN_TIMEOUT_SECS")
                .ok()
                .as_deref(),
        )
    }

    /// 解析生效的 HTTP 请求体上限（字节）。
    ///
    /// 环境变量 `JIACLAW_MAX_BODY_BYTES` 优先于配置文件。
    /// 仅正整数生效；非法 / `0` 回退配置或默认 1MiB。
    #[must_use]
    pub fn effective_max_body_bytes(&self) -> u64 {
        resolve_max_body_bytes(
            self.max_body_bytes,
            std::env::var("JIACLAW_MAX_BODY_BYTES").ok().as_deref(),
        )
    }

    /// 解析 `GET /metrics` 是否公开（无需 API Bearer）。
    ///
    /// 默认公开。`[http] metrics_public = false` 或环境变量
    /// `JIACLAW_METRICS_REQUIRE_AUTH=1/true` 时与 `/api/*` 相同鉴权。
    #[must_use]
    pub fn effective_metrics_public(&self) -> bool {
        resolve_metrics_public(
            self.metrics_public,
            std::env::var("JIACLAW_METRICS_REQUIRE_AUTH")
                .ok()
                .as_deref(),
        )
    }

    /// 解析生效的 Telegram Bot API token。
    ///
    /// 环境变量 `JIACLAW_TELEGRAM_BOT_TOKEN` 优先于配置文件；空白视为未配置。
    #[must_use]
    pub fn effective_telegram_bot_token(&self) -> Option<String> {
        resolve_optional_secret(
            self.telegram_bot_token.clone(),
            std::env::var("JIACLAW_TELEGRAM_BOT_TOKEN").ok().as_deref(),
        )
    }

    /// 解析生效的 Slack signing secret。
    ///
    /// 环境变量 `JIACLAW_SLACK_SIGNING_SECRET` 优先于配置文件；空白视为未配置。
    #[must_use]
    pub fn effective_slack_signing_secret(&self) -> Option<String> {
        resolve_optional_secret(
            self.slack_signing_secret.clone(),
            std::env::var("JIACLAW_SLACK_SIGNING_SECRET")
                .ok()
                .as_deref(),
        )
    }

    /// 解析生效的 Slack Bot token。
    ///
    /// 环境变量 `JIACLAW_SLACK_BOT_TOKEN` 优先于配置文件；空白视为未配置。
    #[must_use]
    pub fn effective_slack_bot_token(&self) -> Option<String> {
        resolve_optional_secret(
            self.slack_bot_token.clone(),
            std::env::var("JIACLAW_SLACK_BOT_TOKEN").ok().as_deref(),
        )
    }

    /// Resolve the Feishu application secret, preferring the environment.
    #[must_use]
    pub fn effective_feishu_app_secret(&self) -> Option<String> {
        resolve_optional_secret(
            self.feishu_app_secret.clone(),
            std::env::var("JIACLAW_FEISHU_APP_SECRET").ok().as_deref(),
        )
    }

    /// Resolve the Feishu callback encryption/signature key, preferring the environment.
    #[must_use]
    pub fn effective_feishu_encrypt_key(&self) -> Option<String> {
        resolve_optional_secret(
            self.feishu_encrypt_key.clone(),
            std::env::var("JIACLAW_FEISHU_ENCRYPT_KEY").ok().as_deref(),
        )
    }

    /// Resolve the Feishu callback verification token, preferring the environment.
    #[must_use]
    pub fn effective_feishu_verification_token(&self) -> Option<String> {
        resolve_optional_secret(
            self.feishu_verification_token.clone(),
            std::env::var("JIACLAW_FEISHU_VERIFICATION_TOKEN")
                .ok()
                .as_deref(),
        )
    }

    /// Resolve the DingTalk Client Secret, preferring the environment.
    #[must_use]
    pub fn effective_dingtalk_app_secret(&self) -> Option<String> {
        resolve_optional_secret(
            self.dingtalk_app_secret.clone(),
            std::env::var("JIACLAW_DINGTALK_APP_SECRET").ok().as_deref(),
        )
    }

    /// Resolve the WeCom app secret, preferring the environment.
    #[must_use]
    pub fn effective_wecom_app_secret(&self) -> Option<String> {
        resolve_optional_secret(
            self.wecom_app_secret.clone(),
            std::env::var("JIACLAW_WECOM_APP_SECRET").ok().as_deref(),
        )
    }

    /// Resolve the WeCom callback token, preferring the environment.
    #[must_use]
    pub fn effective_wecom_callback_token(&self) -> Option<String> {
        resolve_optional_secret(
            self.wecom_callback_token.clone(),
            std::env::var("JIACLAW_WECOM_CALLBACK_TOKEN")
                .ok()
                .as_deref(),
        )
    }

    /// Resolve the WeCom encoding aes key, preferring the environment.
    #[must_use]
    pub fn effective_wecom_encoding_aes_key(&self) -> Option<String> {
        resolve_optional_secret(
            self.wecom_encoding_aes_key.clone(),
            std::env::var("JIACLAW_WECOM_ENCODING_AES_KEY")
                .ok()
                .as_deref(),
        )
    }

    /// 解析生效的 Discord Interactions 公钥。
    ///
    /// 环境变量 `JIACLAW_DISCORD_PUBLIC_KEY` 优先于配置文件；空白视为未配置。
    #[must_use]
    pub fn effective_discord_public_key(&self) -> Option<String> {
        resolve_optional_secret(
            self.discord_public_key.clone(),
            std::env::var("JIACLAW_DISCORD_PUBLIC_KEY").ok().as_deref(),
        )
    }

    /// 解析生效的 Discord Bot token。
    ///
    /// 环境变量 `JIACLAW_DISCORD_BOT_TOKEN` 优先于配置文件；空白视为未配置。
    #[must_use]
    pub fn effective_discord_bot_token(&self) -> Option<String> {
        resolve_optional_secret(
            self.discord_bot_token.clone(),
            std::env::var("JIACLAW_DISCORD_BOT_TOKEN").ok().as_deref(),
        )
    }
}

/// 模型提供商配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// 提供商类型 (`brokerrouter` 推荐, `openai_compatible` 已废弃)
    #[serde(default = "default_provider_type")]
    pub provider_type: String,

    /// API 基础 URL（支持 OpenAI、Ollama、LM Studio 等）
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// API 密钥（可选，无密钥时回退到存根）
    #[serde(default)]
    pub api_key: Option<String>,

    /// 模型名称
    #[serde(default = "default_model")]
    pub model: String,

    /// 温度参数
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// 最大 tokens
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_provider_type() -> String {
    "brokerrouter".to_string()
}

fn default_base_url() -> String {
    "https://api.brokerrouter.dev".to_string()
}

fn default_model() -> String {
    "gpt-4o-mini".to_string()
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: default_provider_type(),
            base_url: default_base_url(),
            api_key: None,
            model: default_model(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "JiaClaw".to_string(),
            description: "Personal durable agent runtime".to_string(),
            system_instructions: "You are JiaClaw, a helpful personal assistant.".to_string(),
            max_turns: 10,
            workspace_path: default_workspace_path(),
            provider: ProviderConfig::default(),
            routing: ModelRoutingConfig::default(),
            model_calls: ModelCallsConfig::default(),
            http: HttpConfig::default(),
            memory: MemoryConfig::default(),
            identity: IdentityConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            scheduler: SchedulerConfig::default(),
            session: SessionConfig::default(),
            tool_timeout_secs: None,
            max_tool_iterations: default_max_tool_iterations(),
            tools: ToolsConfig::default(),
            mcp: McpConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl AgentConfig {
    /// 解析生效的单次工具调用超时（秒）。
    ///
    /// 环境变量 `JIACLAW_TOOL_TIMEOUT_SECS` 优先于配置文件。
    /// 仅正整数生效；未设置、`0` 或无法解析表示不限制。
    #[must_use]
    pub fn effective_tool_timeout_secs(&self) -> Option<u64> {
        resolve_tool_timeout_secs(
            self.tool_timeout_secs,
            std::env::var("JIACLAW_TOOL_TIMEOUT_SECS").ok().as_deref(),
        )
    }

    /// 解析生效的工具循环上限。
    ///
    /// 环境变量 `JIACLAW_MAX_TOOL_ITERATIONS` 优先于配置文件。
    /// 仅正整数生效；未设置、`0` 或无法解析时回退配置值（`0` 再回退默认 5）。
    /// 最终钳制到 `[MIN_MAX_TOOL_ITERATIONS, MAX_MAX_TOOL_ITERATIONS]`。
    #[must_use]
    pub fn effective_max_tool_iterations(&self) -> usize {
        resolve_max_tool_iterations(
            self.max_tool_iterations,
            std::env::var("JIACLAW_MAX_TOOL_ITERATIONS").ok().as_deref(),
        )
    }

    /// 从 TOML 文件加载配置
    ///
    /// # Errors
    ///
    /// 如果文件读取失败或解析失败，返回错误。
    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, JiaClawError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| JiaClawError::Configuration(format!("无法读取配置文件: {e}")))?;
        Self::from_toml_str(&content)
    }

    /// 从 TOML 字符串解析配置
    ///
    /// # Errors
    ///
    /// 如果解析失败，返回错误。
    pub fn from_toml_str(content: &str) -> Result<Self, JiaClawError> {
        #[derive(Deserialize)]
        struct ConfigFile {
            agent: AgentConfig,
            #[serde(default)]
            provider: Option<ProviderConfig>,
            #[serde(default)]
            routing: Option<ModelRoutingConfig>,
            #[serde(default)]
            model_calls: Option<ModelCallsConfig>,
            #[serde(default)]
            http: Option<HttpConfig>,
            #[serde(default)]
            memory: Option<MemoryConfig>,
            #[serde(default)]
            identity: Option<IdentityConfig>,
            #[serde(default)]
            heartbeat: Option<HeartbeatConfig>,
            #[serde(default)]
            scheduler: Option<SchedulerConfig>,
            #[serde(default)]
            session: Option<SessionConfig>,
            #[serde(default)]
            tools: Option<ToolsConfig>,
            #[serde(default)]
            mcp: Option<McpConfig>,
            #[serde(default)]
            logging: Option<LoggingConfig>,
        }

        let mut config_file: ConfigFile = toml::from_str(content)
            .map_err(|e| JiaClawError::Configuration(format!("无法解析 TOML 配置: {e}")))?;

        // 顶层配置覆盖 agent 中的对应配置。
        if let Some(provider) = config_file.provider {
            config_file.agent.provider = provider;
        }
        if let Some(routing) = config_file.routing {
            config_file.agent.routing = routing;
        }
        if let Some(model_calls) = config_file.model_calls {
            config_file.agent.model_calls = model_calls;
        }
        if let Some(http) = config_file.http {
            config_file.agent.http = http;
        }
        if let Some(memory) = config_file.memory {
            config_file.agent.memory = memory;
        }
        if let Some(identity) = config_file.identity {
            config_file.agent.identity = identity;
        }
        if let Some(heartbeat) = config_file.heartbeat {
            config_file.agent.heartbeat = heartbeat;
        }
        if let Some(scheduler) = config_file.scheduler {
            config_file.agent.scheduler = scheduler;
        }
        if let Some(session) = config_file.session {
            config_file.agent.session = session;
        }
        if let Some(tools) = config_file.tools {
            config_file.agent.tools = tools;
        }
        if let Some(mcp) = config_file.mcp {
            config_file.agent.mcp = mcp;
        }
        if let Some(logging) = config_file.logging {
            config_file.agent.logging = logging;
        }

        Ok(config_file.agent)
    }

    /// 从 JSON 文件加载配置
    ///
    /// # Errors
    ///
    /// 如果文件读取失败或解析失败，返回错误。
    pub fn from_json_file(path: impl AsRef<std::path::Path>) -> Result<Self, JiaClawError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| JiaClawError::Configuration(format!("无法读取配置文件: {e}")))?;
        Self::from_json_str(&content)
    }

    /// 从 JSON 字符串解析配置
    ///
    /// # Errors
    ///
    /// 如果解析失败，返回错误。
    pub fn from_json_str(content: &str) -> Result<Self, JiaClawError> {
        #[derive(Deserialize)]
        struct ConfigFile {
            agent: AgentConfig,
            #[serde(default)]
            provider: Option<ProviderConfig>,
            #[serde(default)]
            routing: Option<ModelRoutingConfig>,
            #[serde(default)]
            model_calls: Option<ModelCallsConfig>,
            #[serde(default)]
            http: Option<HttpConfig>,
            #[serde(default)]
            memory: Option<MemoryConfig>,
            #[serde(default)]
            identity: Option<IdentityConfig>,
            #[serde(default)]
            heartbeat: Option<HeartbeatConfig>,
            #[serde(default)]
            scheduler: Option<SchedulerConfig>,
            #[serde(default)]
            session: Option<SessionConfig>,
            #[serde(default)]
            tools: Option<ToolsConfig>,
            #[serde(default)]
            mcp: Option<McpConfig>,
            #[serde(default)]
            logging: Option<LoggingConfig>,
        }

        let mut config_file: ConfigFile = serde_json::from_str(content)
            .map_err(|e| JiaClawError::Configuration(format!("无法解析 JSON 配置: {e}")))?;

        // 顶层配置覆盖 agent 中的对应配置。
        if let Some(provider) = config_file.provider {
            config_file.agent.provider = provider;
        }
        if let Some(routing) = config_file.routing {
            config_file.agent.routing = routing;
        }
        if let Some(model_calls) = config_file.model_calls {
            config_file.agent.model_calls = model_calls;
        }
        if let Some(http) = config_file.http {
            config_file.agent.http = http;
        }
        if let Some(memory) = config_file.memory {
            config_file.agent.memory = memory;
        }
        if let Some(identity) = config_file.identity {
            config_file.agent.identity = identity;
        }
        if let Some(heartbeat) = config_file.heartbeat {
            config_file.agent.heartbeat = heartbeat;
        }
        if let Some(scheduler) = config_file.scheduler {
            config_file.agent.scheduler = scheduler;
        }
        if let Some(session) = config_file.session {
            config_file.agent.session = session;
        }
        if let Some(tools) = config_file.tools {
            config_file.agent.tools = tools;
        }
        if let Some(mcp) = config_file.mcp {
            config_file.agent.mcp = mcp;
        }
        if let Some(logging) = config_file.logging {
            config_file.agent.logging = logging;
        }

        Ok(config_file.agent)
    }
}

#[cfg(test)]
mod example_tests {
    use super::AgentConfig;

    #[test]
    fn shipped_toml_and_json_examples_parse_to_the_same_effective_config() {
        let toml = AgentConfig::from_toml_str(include_str!("../../../config/jiaclaw.toml.example"))
            .unwrap();
        let json = AgentConfig::from_json_str(include_str!("../../../config/jiaclaw.json.example"))
            .unwrap();
        assert_eq!(
            serde_json::to_value(&toml).unwrap(),
            serde_json::to_value(&json).unwrap()
        );
        assert!(toml.http.persist);
        assert!(toml.tools.copy.enabled);
        assert!(!toml.tools.exec.enabled);
        assert_eq!(toml.provider.provider_type, "brokerrouter");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_boolish_flag, parse_cors_enabled, parse_cors_origins, parse_log_format,
        parse_metrics_require_auth, parse_positive_heartbeat_interval,
        parse_positive_max_body_bytes, parse_positive_max_tool_iterations,
        parse_positive_rate_limit, parse_positive_session_ttl, parse_positive_shutdown_timeout,
        parse_positive_tool_timeout, parse_session_summarize_on_overflow, resolve_cors_enabled,
        resolve_cors_origins, resolve_heartbeat_interval_secs, resolve_log_format,
        resolve_log_level, resolve_max_body_bytes, resolve_max_tool_iterations,
        resolve_metrics_public, resolve_optional_secret, resolve_rate_limit_per_minute,
        resolve_session_keep_recent, resolve_session_summarize_on_overflow,
        resolve_session_ttl_secs, resolve_shutdown_timeout_secs, resolve_tool_timeout_secs,
        AgentConfig, DeleteFileToolConfig, GlobToolConfig, GrepToolConfig, HeartbeatConfig,
        HttpConfig, HttpCorsConfig, ListDirToolConfig, LogFormat, LoggingConfig,
        MemorySearchToolConfig, MemoryWriteToolConfig, MkdirToolConfig, MoveToolConfig,
        ReadFileToolConfig, SessionConfig, StrReplaceToolConfig, ToolsConfig, WebFetchToolConfig,
        WebSearchToolConfig, WriteFileToolConfig, DEFAULT_HEARTBEAT_INTERVAL_SECS,
        DEFAULT_HEARTBEAT_PATH, DEFAULT_HEARTBEAT_SESSION_ID, DEFAULT_HTTP_MAX_BODY_BYTES,
        DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS, DEFAULT_LOG_LEVEL, DEFAULT_MAX_TOOL_ITERATIONS,
        DEFAULT_MEMORY_PATH, DEFAULT_SESSION_KEEP_RECENT, DEFAULT_SOUL_PATH, DEFAULT_USER_PATH,
        MAX_MAX_TOOL_ITERATIONS, MAX_SESSION_MESSAGES, MIN_MAX_TOOL_ITERATIONS,
    };

    #[test]
    fn http_config_rate_limit_defaults_to_none() {
        assert_eq!(HttpConfig::default().rate_limit_per_minute, None);
        assert_eq!(
            HttpConfig::default().effective_rate_limit_per_minute(),
            None
        );
    }

    #[test]
    fn http_config_session_ttl_defaults_to_none() {
        assert_eq!(HttpConfig::default().session_ttl_secs, None);
        assert_eq!(HttpConfig::default().effective_session_ttl_secs(), None);
    }

    #[test]
    fn http_config_shutdown_timeout_defaults_to_15() {
        assert_eq!(DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS, 15);
        assert_eq!(
            HttpConfig::default().shutdown_timeout_secs,
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_shutdown_timeout_secs(0, None),
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_shutdown_timeout_secs(DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS, None),
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
    }

    #[test]
    fn http_config_max_body_bytes_defaults_to_1mib() {
        assert_eq!(DEFAULT_HTTP_MAX_BODY_BYTES, 1_048_576);
        assert_eq!(
            HttpConfig::default().max_body_bytes,
            DEFAULT_HTTP_MAX_BODY_BYTES
        );
        assert_eq!(
            HttpConfig::default().effective_max_body_bytes(),
            DEFAULT_HTTP_MAX_BODY_BYTES
        );
        assert_eq!(resolve_max_body_bytes(0, None), DEFAULT_HTTP_MAX_BODY_BYTES);
        assert_eq!(
            resolve_max_body_bytes(DEFAULT_HTTP_MAX_BODY_BYTES, None),
            DEFAULT_HTTP_MAX_BODY_BYTES
        );
    }

    #[test]
    fn http_config_metrics_public_defaults_to_true() {
        assert!(HttpConfig::default().metrics_public);
        assert!(HttpConfig::default().effective_metrics_public());
    }

    #[test]
    fn logging_config_defaults_to_text() {
        let logging = LoggingConfig::default();
        assert_eq!(logging.format, LogFormat::Text);
        assert_eq!(logging.level, None);
        assert_eq!(AgentConfig::default().logging.format, LogFormat::Text);
        assert_eq!(DEFAULT_LOG_LEVEL, "info");
        assert_eq!(resolve_log_level(None, None, None), "info");
    }

    #[test]
    fn parse_log_format_accepts_text_and_json() {
        assert_eq!(parse_log_format("text"), Some(LogFormat::Text));
        assert_eq!(parse_log_format(" JSON "), Some(LogFormat::Json));
        assert_eq!(parse_log_format("Text"), Some(LogFormat::Text));
        assert_eq!(parse_log_format(""), None);
        assert_eq!(parse_log_format("pretty"), None);
        assert_eq!(parse_log_format("compact"), None);
    }

    #[test]
    fn resolve_log_format_env_overrides_config() {
        assert_eq!(
            resolve_log_format(LogFormat::Text, Some("json")),
            LogFormat::Json
        );
        assert_eq!(
            resolve_log_format(LogFormat::Json, Some("text")),
            LogFormat::Text
        );
        assert_eq!(
            resolve_log_format(LogFormat::Json, Some("nope")),
            LogFormat::Json
        );
        assert_eq!(
            resolve_log_format(LogFormat::Text, Some("")),
            LogFormat::Text
        );
        assert_eq!(resolve_log_format(LogFormat::Json, None), LogFormat::Json);
    }

    #[test]
    fn resolve_log_level_prefers_jiaclaw_env_then_rust_log() {
        assert_eq!(
            resolve_log_level(Some("warn"), Some("debug"), Some("error")),
            "debug"
        );
        assert_eq!(
            resolve_log_level(Some("warn"), None, Some("error")),
            "error"
        );
        assert_eq!(resolve_log_level(Some("warn"), None, None), "warn");
        assert_eq!(resolve_log_level(None, Some("  "), Some("trace")), "trace");
        assert_eq!(resolve_log_level(Some("  "), None, None), "info");
    }

    #[test]
    fn logging_config_parses_default_text_when_section_missing() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.logging.format, LogFormat::Text);
        assert_eq!(config.logging.level, None);
    }

    #[test]
    fn logging_config_parses_json_format_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[logging]
format = "json"
level = "debug"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.logging.format, LogFormat::Json);
        assert_eq!(config.logging.level.as_deref(), Some("debug"));
        assert_eq!(
            resolve_log_format(config.logging.format, Some("json")),
            LogFormat::Json
        );
    }

    #[test]
    fn logging_config_parses_missing_logging_from_json_as_text() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.logging.format, LogFormat::Text);
        assert_eq!(config.logging.level, None);
    }

    #[test]
    fn logging_config_parses_json_format_from_json_file() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "logging": {
                "format": "json",
                "level": "warn"
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.logging.format, LogFormat::Json);
        assert_eq!(config.logging.level.as_deref(), Some("warn"));
    }

    #[test]
    fn http_cors_defaults_to_disabled_without_wildcard() {
        let cors = HttpConfig::default().cors;
        assert!(!cors.enabled);
        assert!(!cors.effective_enabled());
        assert!(cors.allowed_origins.is_empty());
        assert!(cors.effective_allowed_origins().is_empty());
        assert!(!cors.allows_any_origin());
        assert_eq!(
            cors.effective_allowed_methods(),
            vec!["GET", "POST", "DELETE", "OPTIONS"]
        );
        assert_eq!(
            cors.effective_allowed_headers(),
            vec!["Authorization", "Content-Type", "X-Request-Id", "Accept"]
        );
        assert_eq!(
            cors.effective_expose_headers(),
            vec![
                "X-Request-Id",
                "X-RateLimit-Limit",
                "X-RateLimit-Remaining",
                "X-RateLimit-Reset",
                "Retry-After"
            ]
        );
        assert_eq!(cors.effective_max_age_secs(), None);
    }

    #[test]
    fn parse_cors_origins_splits_comma_and_trims() {
        assert_eq!(
            parse_cors_origins(" http://localhost:5173 , https://app.example "),
            vec!["http://localhost:5173", "https://app.example"]
        );
        assert_eq!(parse_cors_origins("*"), vec!["*"]);
        assert!(parse_cors_origins(" ,  , ").is_empty());
        assert!(parse_cors_origins("").is_empty());
    }

    #[test]
    fn parse_cors_enabled_accepts_boolish_values() {
        assert_eq!(parse_cors_enabled("1"), Some(true));
        assert_eq!(parse_cors_enabled("true"), Some(true));
        assert_eq!(parse_cors_enabled(" YES "), Some(true));
        assert_eq!(parse_cors_enabled("0"), Some(false));
        assert_eq!(parse_cors_enabled("off"), Some(false));
        assert_eq!(parse_cors_enabled(""), None);
        assert_eq!(parse_cors_enabled("maybe"), None);
    }

    #[test]
    fn resolve_cors_env_overrides_config() {
        assert!(resolve_cors_enabled(false, Some("1")));
        assert!(resolve_cors_enabled(false, Some("true")));
        assert!(!resolve_cors_enabled(true, Some("0")));
        assert!(!resolve_cors_enabled(false, Some("nope")));
        assert!(!resolve_cors_enabled(false, None));
        assert!(resolve_cors_enabled(true, None));
        assert_eq!(
            resolve_cors_origins(
                vec!["https://from-file.example".into()],
                Some("http://localhost:5173, https://ui.example")
            ),
            vec!["http://localhost:5173", "https://ui.example"]
        );
        assert_eq!(
            resolve_cors_origins(vec!["https://from-file.example".into()], Some("*")),
            vec!["*"]
        );
        assert!(
            resolve_cors_origins(vec!["https://from-file.example".into()], Some("")).is_empty()
        );
        assert_eq!(
            resolve_cors_origins(vec![" https://from-file.example ".into()], None),
            vec!["https://from-file.example"]
        );
    }

    #[test]
    fn http_config_parses_cors_section_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:9090"

[http.cors]
enabled = true
allowed_origins = ["http://localhost:5173", "https://app.example"]
allowed_methods = ["GET", "POST"]
allowed_headers = ["Authorization", "Content-Type"]
expose_headers = ["X-Request-Id"]
max_age_secs = 600
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(config.http.cors.enabled);
        assert_eq!(
            config.http.cors.allowed_origins,
            vec!["http://localhost:5173", "https://app.example"]
        );
        assert_eq!(config.http.cors.allowed_methods, vec!["GET", "POST"]);
        assert_eq!(
            config.http.cors.allowed_headers,
            vec!["Authorization", "Content-Type"]
        );
        assert_eq!(config.http.cors.expose_headers, vec!["X-Request-Id"]);
        assert_eq!(config.http.cors.max_age_secs, Some(600));
        let resolved = config.http.cors.resolved();
        assert!(resolved.enabled);
        assert!(!resolved.allows_any_origin());
    }

    #[test]
    fn http_config_parses_missing_cors_from_json_as_disabled() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "http": {
                "bind": "127.0.0.1:8080"
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.http.cors.enabled);
        assert!(config.http.cors.allowed_origins.is_empty());
        assert!(!HttpCorsConfig::default().enabled);
    }

    #[test]
    fn agent_config_tool_timeout_defaults_to_none() {
        assert_eq!(AgentConfig::default().tool_timeout_secs, None);
        assert_eq!(AgentConfig::default().effective_tool_timeout_secs(), None);
    }

    #[test]
    fn agent_config_max_tool_iterations_defaults_to_legacy_constant() {
        assert_eq!(DEFAULT_MAX_TOOL_ITERATIONS, 5);
        assert_eq!(
            AgentConfig::default().max_tool_iterations,
            DEFAULT_MAX_TOOL_ITERATIONS
        );
        assert_eq!(
            AgentConfig::default().effective_max_tool_iterations(),
            DEFAULT_MAX_TOOL_ITERATIONS
        );
    }

    #[test]
    fn parse_positive_rate_limit_accepts_only_positive_integers() {
        assert_eq!(parse_positive_rate_limit("60"), Some(60));
        assert_eq!(parse_positive_rate_limit(" 1 "), Some(1));
        assert_eq!(parse_positive_rate_limit("0"), None);
        assert_eq!(parse_positive_rate_limit(""), None);
        assert_eq!(parse_positive_rate_limit("abc"), None);
        assert_eq!(parse_positive_rate_limit("-1"), None);
    }

    #[test]
    fn parse_positive_session_ttl_accepts_only_positive_integers() {
        assert_eq!(parse_positive_session_ttl("3600"), Some(3600));
        assert_eq!(parse_positive_session_ttl(" 1 "), Some(1));
        assert_eq!(parse_positive_session_ttl("0"), None);
        assert_eq!(parse_positive_session_ttl(""), None);
        assert_eq!(parse_positive_session_ttl("abc"), None);
        assert_eq!(parse_positive_session_ttl("-1"), None);
    }

    #[test]
    fn resolve_rate_limit_env_overrides_config() {
        assert_eq!(
            resolve_rate_limit_per_minute(Some(30), Some("120")),
            Some(120)
        );
        assert_eq!(resolve_rate_limit_per_minute(Some(30), Some("0")), None);
        assert_eq!(resolve_rate_limit_per_minute(Some(30), Some("nope")), None);
        assert_eq!(resolve_rate_limit_per_minute(Some(30), None), Some(30));
        assert_eq!(resolve_rate_limit_per_minute(Some(0), None), None);
        assert_eq!(resolve_rate_limit_per_minute(None, None), None);
    }

    #[test]
    fn resolve_session_ttl_env_overrides_config() {
        assert_eq!(resolve_session_ttl_secs(Some(30), Some("120")), Some(120));
        assert_eq!(resolve_session_ttl_secs(Some(30), Some("0")), None);
        assert_eq!(resolve_session_ttl_secs(Some(30), Some("nope")), None);
        assert_eq!(resolve_session_ttl_secs(Some(30), None), Some(30));
        assert_eq!(resolve_session_ttl_secs(Some(0), None), None);
        assert_eq!(resolve_session_ttl_secs(None, None), None);
    }

    #[test]
    fn parse_positive_shutdown_timeout_accepts_only_positive_integers() {
        assert_eq!(parse_positive_shutdown_timeout("15"), Some(15));
        assert_eq!(parse_positive_shutdown_timeout(" 1 "), Some(1));
        assert_eq!(parse_positive_shutdown_timeout("0"), None);
        assert_eq!(parse_positive_shutdown_timeout(""), None);
        assert_eq!(parse_positive_shutdown_timeout("abc"), None);
        assert_eq!(parse_positive_shutdown_timeout("-1"), None);
    }

    #[test]
    fn resolve_shutdown_timeout_env_overrides_and_falls_back() {
        assert_eq!(resolve_shutdown_timeout_secs(15, Some("30")), 30);
        assert_eq!(
            resolve_shutdown_timeout_secs(20, Some("0")),
            20,
            "env 0 应忽略并回退配置"
        );
        assert_eq!(
            resolve_shutdown_timeout_secs(20, Some("nope")),
            20,
            "非法 env 应忽略并回退配置"
        );
        assert_eq!(resolve_shutdown_timeout_secs(8, None), 8);
        assert_eq!(
            resolve_shutdown_timeout_secs(0, None),
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_shutdown_timeout_secs(0, Some("bad")),
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
    }

    #[test]
    fn parse_positive_max_body_bytes_accepts_only_positive_integers() {
        assert_eq!(parse_positive_max_body_bytes("1048576"), Some(1_048_576));
        assert_eq!(parse_positive_max_body_bytes(" 64 "), Some(64));
        assert_eq!(parse_positive_max_body_bytes("0"), None);
        assert_eq!(parse_positive_max_body_bytes(""), None);
        assert_eq!(parse_positive_max_body_bytes("abc"), None);
        assert_eq!(parse_positive_max_body_bytes("-1"), None);
    }

    #[test]
    fn resolve_max_body_bytes_env_overrides_and_falls_back() {
        assert_eq!(resolve_max_body_bytes(1_048_576, Some("2048")), 2048);
        assert_eq!(
            resolve_max_body_bytes(4096, Some("0")),
            4096,
            "env 0 应忽略并回退配置"
        );
        assert_eq!(
            resolve_max_body_bytes(4096, Some("nope")),
            4096,
            "非法 env 应忽略并回退配置"
        );
        assert_eq!(resolve_max_body_bytes(8192, None), 8192);
        assert_eq!(resolve_max_body_bytes(0, None), DEFAULT_HTTP_MAX_BODY_BYTES);
        assert_eq!(
            resolve_max_body_bytes(0, Some("bad")),
            DEFAULT_HTTP_MAX_BODY_BYTES
        );
    }

    #[test]
    fn parse_positive_tool_timeout_accepts_only_positive_integers() {
        assert_eq!(parse_positive_tool_timeout("30"), Some(30));
        assert_eq!(parse_positive_tool_timeout(" 1 "), Some(1));
        assert_eq!(parse_positive_tool_timeout("0"), None);
        assert_eq!(parse_positive_tool_timeout(""), None);
        assert_eq!(parse_positive_tool_timeout("abc"), None);
        assert_eq!(parse_positive_tool_timeout("-1"), None);
    }

    #[test]
    fn resolve_tool_timeout_env_overrides_config() {
        assert_eq!(resolve_tool_timeout_secs(Some(30), Some("120")), Some(120));
        assert_eq!(resolve_tool_timeout_secs(Some(30), Some("0")), None);
        assert_eq!(resolve_tool_timeout_secs(Some(30), Some("nope")), None);
        assert_eq!(resolve_tool_timeout_secs(Some(30), None), Some(30));
        assert_eq!(resolve_tool_timeout_secs(Some(0), None), None);
        assert_eq!(resolve_tool_timeout_secs(None, None), None);
    }

    #[test]
    fn parse_positive_max_tool_iterations_accepts_only_positive_integers() {
        assert_eq!(parse_positive_max_tool_iterations("8"), Some(8));
        assert_eq!(parse_positive_max_tool_iterations(" 1 "), Some(1));
        assert_eq!(parse_positive_max_tool_iterations("0"), None);
        assert_eq!(parse_positive_max_tool_iterations(""), None);
        assert_eq!(parse_positive_max_tool_iterations("abc"), None);
        assert_eq!(parse_positive_max_tool_iterations("-1"), None);
    }

    #[test]
    fn resolve_max_tool_iterations_env_overrides_and_clamps() {
        assert_eq!(resolve_max_tool_iterations(5, Some("12")), 12);
        assert_eq!(
            resolve_max_tool_iterations(8, Some("0")),
            8,
            "env 0 应忽略并回退配置"
        );
        assert_eq!(
            resolve_max_tool_iterations(8, Some("nope")),
            8,
            "非法 env 应忽略并回退配置"
        );
        assert_eq!(resolve_max_tool_iterations(8, None), 8);
        assert_eq!(
            resolve_max_tool_iterations(0, None),
            DEFAULT_MAX_TOOL_ITERATIONS
        );
        assert_eq!(
            resolve_max_tool_iterations(5, Some("1")),
            MIN_MAX_TOOL_ITERATIONS
        );
        assert_eq!(
            resolve_max_tool_iterations(5, Some("999")),
            MAX_MAX_TOOL_ITERATIONS
        );
        assert_eq!(
            resolve_max_tool_iterations(100, None),
            MAX_MAX_TOOL_ITERATIONS
        );
    }

    #[test]
    fn agent_config_parses_max_tool_iterations_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10
max_tool_iterations = 12
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.max_tool_iterations, 12);
        assert_eq!(config.effective_max_tool_iterations(), 12);
        assert_eq!(config.tool_timeout_secs, None);
    }

    #[test]
    fn agent_config_parses_max_tool_iterations_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10,
                "max_tool_iterations": 3
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.max_tool_iterations, 3);
        assert_eq!(config.effective_max_tool_iterations(), 3);
    }

    #[test]
    fn http_config_parses_rate_limit_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:9090"
rate_limit_per_minute = 60
session_ttl_secs = 3600
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.http.bind, "127.0.0.1:9090");
        assert_eq!(config.http.rate_limit_per_minute, Some(60));
        assert_eq!(config.http.session_ttl_secs, Some(3600));
        assert_eq!(
            config.http.shutdown_timeout_secs,
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
        assert_eq!(config.http.max_body_bytes, DEFAULT_HTTP_MAX_BODY_BYTES);
        assert!(config.http.metrics_public);
        assert_eq!(config.http.api_token, None);
        assert_eq!(config.http.webhook_secret, None);
        assert_eq!(config.http.telegram_secret, None);
        assert_eq!(config.http.telegram_bot_token, None);
        assert_eq!(config.http.slack_signing_secret, None);
        assert_eq!(config.http.slack_bot_token, None);
        assert_eq!(config.http.discord_public_key, None);
        assert_eq!(config.http.discord_bot_token, None);
        assert_eq!(config.tool_timeout_secs, None);
        assert_eq!(config.max_tool_iterations, DEFAULT_MAX_TOOL_ITERATIONS);
    }

    #[test]
    fn http_config_parses_shutdown_timeout_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
shutdown_timeout_secs = 5
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.http.shutdown_timeout_secs, 5);
    }

    #[test]
    fn http_config_parses_max_body_bytes_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
max_body_bytes = 2048
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.http.max_body_bytes, 2048);
        assert_eq!(config.http.effective_max_body_bytes(), 2048);
    }

    #[test]
    fn http_config_telegram_secret_defaults_to_none() {
        assert_eq!(HttpConfig::default().telegram_secret, None);
        assert_eq!(HttpConfig::default().telegram_bot_token, None);
        assert_eq!(HttpConfig::default().slack_signing_secret, None);
        assert_eq!(HttpConfig::default().slack_bot_token, None);
        assert_eq!(HttpConfig::default().discord_public_key, None);
        assert_eq!(HttpConfig::default().discord_bot_token, None);
    }

    #[test]
    fn http_config_parses_telegram_secret_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
telegram_secret = "tg-secret-token"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(
            config.http.telegram_secret.as_deref(),
            Some("tg-secret-token")
        );
        assert_eq!(config.http.webhook_secret, None);
        assert_eq!(config.http.telegram_bot_token, None);
    }

    #[test]
    fn http_config_parses_telegram_bot_token_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
telegram_secret = "tg-secret-token"
telegram_bot_token = "123456:ABC-bot-token"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(
            config.http.telegram_secret.as_deref(),
            Some("tg-secret-token")
        );
        assert_eq!(
            config.http.telegram_bot_token.as_deref(),
            Some("123456:ABC-bot-token")
        );
        assert_eq!(config.http.slack_signing_secret, None);
        assert_eq!(config.http.slack_bot_token, None);
    }

    #[test]
    fn http_config_parses_slack_fields_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
slack_signing_secret = "slack-signing-secret"
slack_bot_token = "xoxb-test-token"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(
            config.http.slack_signing_secret.as_deref(),
            Some("slack-signing-secret")
        );
        assert_eq!(
            config.http.slack_bot_token.as_deref(),
            Some("xoxb-test-token")
        );
        assert_eq!(config.http.telegram_bot_token, None);
        assert_eq!(config.http.telegram_secret, None);
        assert_eq!(config.http.discord_public_key, None);
        assert_eq!(config.http.discord_bot_token, None);
    }

    #[test]
    fn http_config_parses_discord_fields_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
discord_public_key = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
discord_bot_token = "discord-bot-token"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(
            config.http.discord_public_key.as_deref(),
            Some("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        );
        assert_eq!(
            config.http.discord_bot_token.as_deref(),
            Some("discord-bot-token")
        );
        assert_eq!(config.http.slack_bot_token, None);
        assert_eq!(config.http.telegram_secret, None);
    }

    #[test]
    fn resolve_optional_secret_env_overrides_config() {
        assert_eq!(
            resolve_optional_secret(Some("from-file".into()), Some("from-env")),
            Some("from-env".into())
        );
        assert_eq!(
            resolve_optional_secret(Some("from-file".into()), Some("  ")),
            Some("from-file".into())
        );
        assert_eq!(
            resolve_optional_secret(Some("  file-token  ".into()), None),
            Some("file-token".into())
        );
        assert_eq!(resolve_optional_secret(Some(String::new()), None), None);
        assert_eq!(resolve_optional_secret(None, None), None);
        assert_eq!(
            resolve_optional_secret(None, Some(" env-token ")),
            Some("env-token".into())
        );
    }

    #[test]
    fn agent_config_parses_tool_timeout_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10
tool_timeout_secs = 30
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.tool_timeout_secs, Some(30));
    }

    #[test]
    fn agent_config_parses_tool_timeout_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10,
                "tool_timeout_secs": 15
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.tool_timeout_secs, Some(15));
    }

    #[test]
    fn http_config_parses_missing_rate_limit_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "http": {
                "bind": "0.0.0.0:8080"
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.http.bind, "0.0.0.0:8080");
        assert_eq!(config.http.rate_limit_per_minute, None);
        assert_eq!(config.http.session_ttl_secs, None);
        assert_eq!(
            config.http.shutdown_timeout_secs,
            DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS
        );
        assert_eq!(config.http.max_body_bytes, DEFAULT_HTTP_MAX_BODY_BYTES);
        assert_eq!(config.memory.path, DEFAULT_MEMORY_PATH);
        assert_eq!(config.identity.soul_path, DEFAULT_SOUL_PATH);
        assert_eq!(config.identity.user_path, DEFAULT_USER_PATH);
        assert_eq!(config.heartbeat.path, DEFAULT_HEARTBEAT_PATH);
        assert!(!config.heartbeat.enabled);
        assert_eq!(
            config.heartbeat.interval_secs,
            DEFAULT_HEARTBEAT_INTERVAL_SECS
        );
        assert_eq!(config.heartbeat.session_id, DEFAULT_HEARTBEAT_SESSION_ID);
        assert_eq!(config.tool_timeout_secs, None);
        assert_eq!(config.max_tool_iterations, DEFAULT_MAX_TOOL_ITERATIONS);
    }

    #[test]
    fn memory_config_defaults_without_section() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.memory.path, DEFAULT_MEMORY_PATH);
        assert_eq!(config.identity.soul_path, DEFAULT_SOUL_PATH);
        assert_eq!(config.identity.user_path, DEFAULT_USER_PATH);
        assert!(!config.heartbeat.enabled);
        assert_eq!(config.heartbeat.path, DEFAULT_HEARTBEAT_PATH);
        assert_eq!(
            config.heartbeat.interval_secs,
            DEFAULT_HEARTBEAT_INTERVAL_SECS
        );
        assert_eq!(config.heartbeat.session_id, DEFAULT_HEARTBEAT_SESSION_ID);
    }

    #[test]
    fn memory_config_parses_top_level_section() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[memory]
path = "notes/MEMORY.md"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.memory.path, "notes/MEMORY.md");
    }

    #[test]
    fn memory_config_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "memory": {
                "path": "custom.md"
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.memory.path, "custom.md");
    }

    #[test]
    fn identity_config_defaults_without_section() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.identity.soul_path, DEFAULT_SOUL_PATH);
        assert_eq!(config.identity.user_path, DEFAULT_USER_PATH);
    }

    #[test]
    fn identity_config_parses_top_level_section() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[identity]
soul_path = "persona/SOUL.md"
user_path = "persona/USER.md"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert_eq!(config.identity.soul_path, "persona/SOUL.md");
        assert_eq!(config.identity.user_path, "persona/USER.md");
    }

    #[test]
    fn identity_config_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "identity": {
                "soul_path": "soul.md",
                "user_path": "user.md"
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert_eq!(config.identity.soul_path, "soul.md");
        assert_eq!(config.identity.user_path, "user.md");
        assert_eq!(config.memory.path, DEFAULT_MEMORY_PATH);
        assert!(!config.heartbeat.enabled);
        assert_eq!(config.heartbeat.path, DEFAULT_HEARTBEAT_PATH);
    }

    #[test]
    fn heartbeat_config_defaults_are_disabled() {
        let cfg = HeartbeatConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.interval_secs, DEFAULT_HEARTBEAT_INTERVAL_SECS);
        assert_eq!(cfg.path, DEFAULT_HEARTBEAT_PATH);
        assert_eq!(cfg.session_id, DEFAULT_HEARTBEAT_SESSION_ID);
        assert_eq!(cfg.effective_session_id(), DEFAULT_HEARTBEAT_SESSION_ID);
    }

    #[test]
    fn parse_positive_heartbeat_interval_accepts_only_positive_integers() {
        assert_eq!(parse_positive_heartbeat_interval("3600"), Some(3600));
        assert_eq!(parse_positive_heartbeat_interval(" 1 "), Some(1));
        assert_eq!(parse_positive_heartbeat_interval("0"), None);
        assert_eq!(parse_positive_heartbeat_interval(""), None);
        assert_eq!(parse_positive_heartbeat_interval("abc"), None);
        assert_eq!(parse_positive_heartbeat_interval("-1"), None);
    }

    #[test]
    fn resolve_heartbeat_interval_env_overrides_config() {
        assert_eq!(resolve_heartbeat_interval_secs(30, Some("120")), 120);
        assert_eq!(resolve_heartbeat_interval_secs(30, Some("0")), 30);
        assert_eq!(resolve_heartbeat_interval_secs(30, Some("nope")), 30);
        assert_eq!(resolve_heartbeat_interval_secs(30, None), 30);
        assert_eq!(
            resolve_heartbeat_interval_secs(0, None),
            DEFAULT_HEARTBEAT_INTERVAL_SECS
        );
        assert_eq!(
            resolve_heartbeat_interval_secs(0, Some("abc")),
            DEFAULT_HEARTBEAT_INTERVAL_SECS
        );
    }

    #[test]
    fn heartbeat_config_parses_top_level_section() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[heartbeat]
enabled = true
interval_secs = 15
path = "ops/HEARTBEAT.md"
session_id = "nightly"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(config.heartbeat.enabled);
        assert_eq!(config.heartbeat.interval_secs, 15);
        assert_eq!(config.heartbeat.path, "ops/HEARTBEAT.md");
        assert_eq!(config.heartbeat.session_id, "nightly");
        assert_eq!(config.heartbeat.effective_session_id(), "nightly");
        assert_eq!(config.memory.path, DEFAULT_MEMORY_PATH);
    }

    #[test]
    fn heartbeat_config_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "heartbeat": {
                "enabled": true,
                "interval_secs": 90,
                "path": "pulse.md",
                "session_id": "pulse"
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(config.heartbeat.enabled);
        assert_eq!(config.heartbeat.interval_secs, 90);
        assert_eq!(config.heartbeat.path, "pulse.md");
        assert_eq!(config.heartbeat.session_id, "pulse");
        assert_eq!(config.identity.soul_path, DEFAULT_SOUL_PATH);
    }

    #[test]
    fn heartbeat_blank_session_id_falls_back() {
        let cfg = HeartbeatConfig {
            session_id: "  ".to_string(),
            ..HeartbeatConfig::default()
        };
        assert_eq!(cfg.effective_session_id(), DEFAULT_HEARTBEAT_SESSION_ID);
    }

    #[test]
    fn session_config_defaults_disable_summarize() {
        let cfg = SessionConfig::default();
        assert!(!cfg.summarize_on_overflow);
        assert_eq!(cfg.keep_recent, DEFAULT_SESSION_KEEP_RECENT);
        assert_eq!(cfg.effective_keep_recent(), DEFAULT_SESSION_KEEP_RECENT);
        assert!(!AgentConfig::default().session.summarize_on_overflow);
        assert_eq!(
            AgentConfig::default().session.keep_recent,
            DEFAULT_SESSION_KEEP_RECENT
        );
    }

    #[test]
    fn parse_session_summarize_on_overflow_accepts_boolish_values() {
        assert_eq!(parse_session_summarize_on_overflow("1"), Some(true));
        assert_eq!(parse_session_summarize_on_overflow("true"), Some(true));
        assert_eq!(parse_session_summarize_on_overflow(" TRUE "), Some(true));
        assert_eq!(parse_session_summarize_on_overflow("yes"), Some(true));
        assert_eq!(parse_session_summarize_on_overflow("on"), Some(true));
        assert_eq!(parse_session_summarize_on_overflow("0"), Some(false));
        assert_eq!(parse_session_summarize_on_overflow("false"), Some(false));
        assert_eq!(parse_session_summarize_on_overflow("off"), Some(false));
        assert_eq!(parse_session_summarize_on_overflow(""), None);
        assert_eq!(parse_session_summarize_on_overflow("maybe"), None);
    }

    #[test]
    fn resolve_session_summarize_env_can_force_enable() {
        assert!(resolve_session_summarize_on_overflow(false, Some("1")));
        assert!(resolve_session_summarize_on_overflow(false, Some("true")));
        assert!(!resolve_session_summarize_on_overflow(true, Some("0")));
        assert!(!resolve_session_summarize_on_overflow(false, Some("nope")));
        assert!(!resolve_session_summarize_on_overflow(false, None));
        assert!(resolve_session_summarize_on_overflow(true, None));
        assert!(resolve_session_summarize_on_overflow(true, Some("bogus")));
    }

    #[test]
    fn parse_metrics_require_auth_accepts_boolish_values() {
        assert_eq!(parse_metrics_require_auth("1"), Some(true));
        assert_eq!(parse_metrics_require_auth("true"), Some(true));
        assert_eq!(parse_metrics_require_auth(" YES "), Some(true));
        assert_eq!(parse_metrics_require_auth("0"), Some(false));
        assert_eq!(parse_metrics_require_auth("off"), Some(false));
        assert_eq!(parse_metrics_require_auth(""), None);
        assert_eq!(parse_boolish_flag("on"), Some(true));
    }

    #[test]
    fn resolve_metrics_public_env_can_force_auth() {
        assert!(!resolve_metrics_public(true, Some("1")));
        assert!(!resolve_metrics_public(true, Some("true")));
        assert!(resolve_metrics_public(false, Some("0")));
        assert!(resolve_metrics_public(true, Some("nope")));
        assert!(resolve_metrics_public(true, None));
        assert!(!resolve_metrics_public(false, None));
        assert!(!resolve_metrics_public(false, Some("bogus")));
    }

    #[test]
    fn http_config_parses_metrics_public_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:9090"
metrics_public = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.http.metrics_public);
        assert!(!config.http.effective_metrics_public());
    }

    #[test]
    fn resolve_session_keep_recent_clamps_and_defaults() {
        assert_eq!(resolve_session_keep_recent(0), DEFAULT_SESSION_KEEP_RECENT);
        assert_eq!(resolve_session_keep_recent(10), 10);
        assert_eq!(resolve_session_keep_recent(1), 1);
        assert_eq!(
            resolve_session_keep_recent(10_000),
            MAX_SESSION_MESSAGES - 1
        );
    }

    #[test]
    fn session_config_parses_top_level_section() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[session]
summarize_on_overflow = true
keep_recent = 8
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(config.session.summarize_on_overflow);
        assert_eq!(config.session.keep_recent, 8);
        assert_eq!(config.session.effective_keep_recent(), 8);
        assert!(!config.heartbeat.enabled);
    }

    #[test]
    fn session_config_parses_from_json_and_omitted_keep_recent_defaults() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "session": {
                "summarize_on_overflow": true
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(config.session.summarize_on_overflow);
        assert_eq!(config.session.keep_recent, DEFAULT_SESSION_KEEP_RECENT);
    }

    #[test]
    fn omitted_session_section_keeps_hard_truncate_defaults() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.session.summarize_on_overflow);
        assert_eq!(config.session.keep_recent, DEFAULT_SESSION_KEEP_RECENT);
    }

    #[test]
    fn web_search_config_defaults_to_enabled_without_key() {
        let config = WebSearchToolConfig::default();
        assert!(config.enabled);
        assert_eq!(config.brave_api_key, None);
        assert!(ToolsConfig::default().web_search.enabled);
        assert!(AgentConfig::default().tools.web_search.enabled);
    }

    #[test]
    fn web_fetch_config_defaults_to_enabled_without_private() {
        let config = WebFetchToolConfig::default();
        assert!(config.enabled);
        assert!(!config.allow_private);
        assert!(ToolsConfig::default().web_fetch.enabled);
        assert!(!ToolsConfig::default().web_fetch.allow_private);
        assert!(AgentConfig::default().tools.web_fetch.enabled);
        assert!(!AgentConfig::default().tools.web_fetch.allow_private);
    }

    #[test]
    fn omitted_tools_section_keeps_web_search_defaults() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[http]
bind = "127.0.0.1:8080"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(config.tools.web_search.enabled);
        assert_eq!(config.tools.web_search.brave_api_key, None);
        assert!(config.tools.web_fetch.enabled);
        assert!(!config.tools.web_fetch.allow_private);
        assert!(config.tools.memory_search.enabled);
        assert!(config.tools.memory_write.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.glob.enabled);
        assert!(config.tools.mkdir.enabled);
        assert!(config.tools.r#move.enabled);
        assert_eq!(config.http.bind, "127.0.0.1:8080");
    }

    #[test]
    fn existing_tools_enabled_list_does_not_break_parsing() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools]
enabled = ["search", "calculator"]

[tools.web_search]
enabled = false
brave_api_key = "listed-key"

[http]
bind = "127.0.0.1:9090"

[memory]
path = "MEMORY.md"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.web_search.enabled);
        assert_eq!(
            config.tools.web_search.brave_api_key.as_deref(),
            Some("listed-key")
        );
        assert!(
            config.tools.web_fetch.enabled,
            "omitted [tools.web_fetch] should keep default enabled"
        );
        assert!(!config.tools.web_fetch.allow_private);
        assert!(
            config.tools.memory_search.enabled,
            "omitted [tools.memory_search] should keep default enabled"
        );
        assert!(
            config.tools.memory_write.enabled,
            "omitted [tools.memory_write] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.list_dir.enabled,
            "omitted [tools.list_dir] should keep default enabled"
        );
        assert!(
            config.tools.write_file.enabled,
            "omitted [tools.write_file] should keep default enabled"
        );
        assert!(
            config.tools.delete_file.enabled,
            "omitted [tools.delete_file] should keep default enabled"
        );
        assert!(
            config.tools.str_replace.enabled,
            "omitted [tools.str_replace] should keep default enabled"
        );
        assert!(
            config.tools.grep.enabled,
            "omitted [tools.grep] should keep default enabled"
        );
        assert!(
            config.tools.glob.enabled,
            "omitted [tools.glob] should keep default enabled"
        );
        assert!(
            config.tools.mkdir.enabled,
            "omitted [tools.mkdir] should keep default enabled"
        );
        assert!(
            config.tools.r#move.enabled,
            "omitted [tools.move] should keep default enabled"
        );
        assert_eq!(config.http.bind, "127.0.0.1:9090");
        assert_eq!(config.memory.path, "MEMORY.md");
    }

    #[test]
    fn tools_web_search_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.web_search]
enabled = false
brave_api_key = "BSA-test-key"
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.web_search.enabled);
        assert_eq!(
            config.tools.web_search.brave_api_key.as_deref(),
            Some("BSA-test-key")
        );
    }

    #[test]
    fn tools_web_search_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "web_search": {
                    "enabled": true,
                    "brave_api_key": "json-key"
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(config.tools.web_search.enabled);
        assert_eq!(
            config.tools.web_search.brave_api_key.as_deref(),
            Some("json-key")
        );
        assert!(config.tools.web_fetch.enabled);
        assert!(!config.tools.web_fetch.allow_private);
        assert!(config.tools.memory_search.enabled);
        assert!(config.tools.memory_write.enabled);
    }

    #[test]
    fn tools_web_fetch_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.web_fetch]
enabled = false
allow_private = true
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.web_fetch.enabled);
        assert!(config.tools.web_fetch.allow_private);
        assert!(
            config.tools.web_search.enabled,
            "omitted [tools.web_search] should keep default enabled"
        );
    }

    #[test]
    fn tools_web_fetch_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "web_fetch": {
                    "enabled": false,
                    "allow_private": true
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.web_fetch.enabled);
        assert!(config.tools.web_fetch.allow_private);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.memory_search.enabled);
        assert!(config.tools.memory_write.enabled);
    }

    #[test]
    fn memory_search_config_defaults_to_enabled() {
        let config = MemorySearchToolConfig::default();
        assert!(config.enabled);
        assert!(ToolsConfig::default().memory_search.enabled);
        assert!(AgentConfig::default().tools.memory_search.enabled);
    }

    #[test]
    fn memory_write_config_defaults_to_enabled() {
        let config = MemoryWriteToolConfig::default();
        assert!(config.enabled);
        assert!(ToolsConfig::default().memory_write.enabled);
        assert!(AgentConfig::default().tools.memory_write.enabled);
    }

    #[test]
    fn tools_memory_search_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.memory_search]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.memory_search.enabled);
        assert!(
            config.tools.web_search.enabled,
            "omitted [tools.web_search] should keep default enabled"
        );
        assert!(
            config.tools.web_fetch.enabled,
            "omitted [tools.web_fetch] should keep default enabled"
        );
        assert!(
            config.tools.memory_write.enabled,
            "omitted [tools.memory_write] should keep default enabled"
        );
    }

    #[test]
    fn tools_memory_search_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "memory_search": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.memory_search.enabled);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.web_fetch.enabled);
        assert!(config.tools.memory_write.enabled);
    }

    #[test]
    fn tools_memory_write_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.memory_write]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.memory_write.enabled);
        assert!(
            config.tools.memory_search.enabled,
            "omitted [tools.memory_search] should keep default enabled"
        );
        assert!(
            config.tools.web_search.enabled,
            "omitted [tools.web_search] should keep default enabled"
        );
        assert!(
            config.tools.web_fetch.enabled,
            "omitted [tools.web_fetch] should keep default enabled"
        );
    }

    #[test]
    fn tools_memory_write_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "memory_write": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.memory_write.enabled);
        assert!(config.tools.memory_search.enabled);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.web_fetch.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.glob.enabled);
        assert!(config.tools.mkdir.enabled);
        assert!(config.tools.r#move.enabled);
    }

    #[test]
    fn filesystem_info_switches_parse_independently_and_reject_typos() {
        let default: ToolsConfig = toml::from_str("").unwrap();
        assert!(default.stat.enabled && default.tree.enabled);
        for (stat, tree) in [(false, true), (true, false), (false, false)] {
            let text = format!("[stat]\nenabled={stat}\n[tree]\nenabled={tree}\n");
            let configured: ToolsConfig = toml::from_str(&text).unwrap();
            assert_eq!(configured.stat.enabled, stat);
            assert_eq!(configured.tree.enabled, tree);
            assert!(configured.list_dir.enabled && configured.read_file.enabled);
            let restored: ToolsConfig =
                serde_json::from_value(serde_json::to_value(configured).unwrap()).unwrap();
            assert_eq!((restored.stat.enabled, restored.tree.enabled), (stat, tree));
        }
        assert!(toml::from_str::<ToolsConfig>("[stat]\nenabld=false").is_err());
        assert!(serde_json::from_str::<ToolsConfig>(r#"{"tree":{"enabled":"false"}}"#).is_err());
    }

    #[test]
    fn read_file_and_list_dir_config_defaults_to_enabled() {
        assert!(ReadFileToolConfig::default().enabled);
        assert!(ListDirToolConfig::default().enabled);
        assert!(WriteFileToolConfig::default().enabled);
        assert!(DeleteFileToolConfig::default().enabled);
        assert!(StrReplaceToolConfig::default().enabled);
        assert!(GrepToolConfig::default().enabled);
        assert!(GlobToolConfig::default().enabled);
        assert!(MkdirToolConfig::default().enabled);
        assert!(MoveToolConfig::default().enabled);
        assert!(ToolsConfig::default().read_file.enabled);
        assert!(ToolsConfig::default().list_dir.enabled);
        assert!(ToolsConfig::default().write_file.enabled);
        assert!(ToolsConfig::default().delete_file.enabled);
        assert!(ToolsConfig::default().str_replace.enabled);
        assert!(ToolsConfig::default().grep.enabled);
        assert!(ToolsConfig::default().glob.enabled);
        assert!(ToolsConfig::default().mkdir.enabled);
        assert!(ToolsConfig::default().r#move.enabled);
        assert!(AgentConfig::default().tools.read_file.enabled);
        assert!(AgentConfig::default().tools.list_dir.enabled);
        assert!(AgentConfig::default().tools.write_file.enabled);
        assert!(AgentConfig::default().tools.delete_file.enabled);
        assert!(AgentConfig::default().tools.str_replace.enabled);
        assert!(AgentConfig::default().tools.grep.enabled);
        assert!(AgentConfig::default().tools.glob.enabled);
        assert!(AgentConfig::default().tools.mkdir.enabled);
        assert!(AgentConfig::default().tools.r#move.enabled);
    }

    #[test]
    fn tools_read_file_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.read_file]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.read_file.enabled);
        assert!(
            config.tools.list_dir.enabled,
            "omitted [tools.list_dir] should keep default enabled"
        );
        assert!(config.tools.memory_write.enabled);
    }

    #[test]
    fn tools_read_file_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "read_file": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.memory_search.enabled);
    }

    #[test]
    fn tools_list_dir_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.list_dir]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.list_dir.enabled);
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
    }

    #[test]
    fn tools_list_dir_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "list_dir": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.list_dir.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.web_search.enabled);
    }

    #[test]
    fn tools_write_file_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.write_file]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.write_file.enabled);
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.list_dir.enabled,
            "omitted [tools.list_dir] should keep default enabled"
        );
    }

    #[test]
    fn tools_write_file_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "write_file": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.write_file.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.web_search.enabled);
    }

    #[test]
    fn tools_delete_file_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.delete_file]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.delete_file.enabled);
        assert!(
            config.tools.write_file.enabled,
            "omitted [tools.write_file] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.list_dir.enabled,
            "omitted [tools.list_dir] should keep default enabled"
        );
        assert!(
            config.tools.str_replace.enabled,
            "omitted [tools.str_replace] should keep default enabled"
        );
        assert!(
            config.tools.grep.enabled,
            "omitted [tools.grep] should keep default enabled"
        );
    }

    #[test]
    fn tools_delete_file_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "delete_file": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.delete_file.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.web_search.enabled);
    }

    #[test]
    fn tools_str_replace_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.str_replace]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.str_replace.enabled);
        assert!(
            config.tools.write_file.enabled,
            "omitted [tools.write_file] should keep default enabled"
        );
        assert!(
            config.tools.delete_file.enabled,
            "omitted [tools.delete_file] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.grep.enabled,
            "omitted [tools.grep] should keep default enabled"
        );
    }

    #[test]
    fn tools_str_replace_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "str_replace": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.str_replace.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.grep.enabled);
    }

    #[test]
    fn tools_grep_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.grep]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.grep.enabled);
        assert!(
            config.tools.str_replace.enabled,
            "omitted [tools.str_replace] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.glob.enabled,
            "omitted [tools.glob] should keep default enabled"
        );
        assert!(
            config.tools.mkdir.enabled,
            "omitted [tools.mkdir] should keep default enabled"
        );
        assert!(
            config.tools.r#move.enabled,
            "omitted [tools.move] should keep default enabled"
        );
    }

    #[test]
    fn tools_grep_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "grep": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.grep.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.glob.enabled);
        assert!(config.tools.mkdir.enabled);
        assert!(config.tools.r#move.enabled);
    }

    #[test]
    fn tools_glob_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.glob]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.glob.enabled);
        assert!(
            config.tools.grep.enabled,
            "omitted [tools.grep] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.mkdir.enabled,
            "omitted [tools.mkdir] should keep default enabled"
        );
        assert!(
            config.tools.r#move.enabled,
            "omitted [tools.move] should keep default enabled"
        );
    }

    #[test]
    fn tools_glob_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "glob": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.glob.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.mkdir.enabled);
        assert!(config.tools.r#move.enabled);
    }

    #[test]
    fn tools_mkdir_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.mkdir]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.mkdir.enabled);
        assert!(
            config.tools.glob.enabled,
            "omitted [tools.glob] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
        assert!(
            config.tools.r#move.enabled,
            "omitted [tools.move] should keep default enabled"
        );
    }

    #[test]
    fn tools_mkdir_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "mkdir": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.mkdir.enabled);
        assert!(config.tools.glob.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.str_replace.enabled);
        assert!(config.tools.write_file.enabled);
        assert!(config.tools.delete_file.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.list_dir.enabled);
        assert!(config.tools.web_search.enabled);
        assert!(config.tools.r#move.enabled);
    }

    #[test]
    fn tools_move_parses_from_toml() {
        let toml = r#"
[agent]
name = "JiaClaw"
description = "test"
system_instructions = "be helpful"
max_turns = 10

[tools.move]
enabled = false
"#;
        let config = AgentConfig::from_toml_str(toml).expect("parse toml");
        assert!(!config.tools.r#move.enabled);
        assert!(
            config.tools.mkdir.enabled,
            "omitted [tools.mkdir] should keep default enabled"
        );
        assert!(
            config.tools.read_file.enabled,
            "omitted [tools.read_file] should keep default enabled"
        );
    }

    #[test]
    fn tools_move_parses_from_json() {
        let json = r#"{
            "agent": {
                "name": "JiaClaw",
                "description": "test",
                "system_instructions": "be helpful",
                "max_turns": 10
            },
            "tools": {
                "move": {
                    "enabled": false
                }
            }
        }"#;
        let config = AgentConfig::from_json_str(json).expect("parse json");
        assert!(!config.tools.r#move.enabled);
        assert!(config.tools.mkdir.enabled);
        assert!(config.tools.glob.enabled);
        assert!(config.tools.grep.enabled);
        assert!(config.tools.read_file.enabled);
        assert!(config.tools.web_search.enabled);
    }

    #[test]
    fn resolve_optional_secret_prefers_env_for_brave_key() {
        assert_eq!(
            resolve_optional_secret(Some("cfg-key".to_string()), Some("env-key")),
            Some("env-key".to_string())
        );
        assert_eq!(
            resolve_optional_secret(Some("cfg-key".to_string()), Some("")),
            Some("cfg-key".to_string())
        );
        assert_eq!(resolve_optional_secret(None, Some("  ")), None);
        assert_eq!(
            resolve_optional_secret(Some("cfg-key".to_string()), None),
            Some("cfg-key".to_string())
        );
    }
}

#[cfg(test)]
mod scheduler_configuration_tests {
    use super::AgentConfig;

    #[test]
    fn scheduler_is_opt_in_and_top_level_json_overrides_nested_default() {
        let config = AgentConfig::default();
        assert!(!config.scheduler.enabled);
        let value = serde_json::json!({"agent": config, "scheduler": {"enabled":true}});
        assert!(
            AgentConfig::from_json_str(&value.to_string())
                .unwrap()
                .scheduler
                .enabled
        );
        let mut unknown = value;
        unknown["scheduler"]["enable"] = serde_json::json!(true);
        assert!(AgentConfig::from_json_str(&unknown.to_string()).is_err());
    }

    #[test]
    fn scheduler_toml_enables_explicitly_and_rejects_misspelled_policy() {
        let base = "[agent]\nname='test'\ndescription='test'\nsystem_instructions='test'\nmax_turns=1\n[scheduler]\nenabled=true\n";
        assert!(AgentConfig::from_toml_str(base).unwrap().scheduler.enabled);
        assert!(AgentConfig::from_toml_str(&format!("{base}enable=true\n")).is_err());
    }
}

#[cfg(test)]
mod scheduler_config_tests {
    use super::*;
    #[test]
    fn gateway_mode_requires_explicit_scheduler_and_rejects_typos() {
        let legacy: SchedulerConfig = toml::from_str("enabled=true").unwrap();
        assert!(!legacy.gateway_driven);
        assert!(legacy.validate().is_ok());
        let enabled: SchedulerConfig = toml::from_str("enabled=true\ngateway_driven=true").unwrap();
        assert!(enabled.validate().is_ok());
        let invalid: SchedulerConfig = toml::from_str("gateway_driven=true").unwrap();
        assert!(invalid.validate().is_err());
        assert!(toml::from_str::<SchedulerConfig>("enabled=true\ngateway_drivn=true").is_err());
    }
}
