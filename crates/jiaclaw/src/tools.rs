// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 工具系统（本地工具实现）

use async_trait::async_trait;
use jiaclaw_core::{JiaClawError, ToolCall};
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What a failed tool attempt can prove about its effects.
///
/// This is an implementation guarantee, not a model/MCP annotation or an error
/// message heuristic. Unknown includes timeouts and abandoned blocking work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureEffect {
    /// The implementation never performs workspace writes or external effects.
    NoEffect,
    /// An effect may have happened, or may still finish after cancellation.
    Unknown,
}

/// 工具 trait
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称
    fn name(&self) -> &str;

    /// 工具描述
    fn description(&self) -> &str;

    /// 工具参数 schema（JSON Schema 格式）
    fn parameters_schema(&self) -> Value;

    /// Classify errors and timeouts of this implementation conservatively.
    ///
    /// Only local implementations that never write workspace data, submit
    /// external requests or otherwise perform effects may return `NoEffect`.
    /// Custom tools and external MCP calls retain `Unknown` by default.
    fn failure_effect(&self) -> ToolFailureEffect {
        ToolFailureEffect::Unknown
    }

    /// 执行工具
    async fn execute(&self, args: Value) -> Result<String, JiaClawError>;
}

/// 工具注册表
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// 创建新的工具注册表
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// 注册工具
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        tracing::debug!("注册工具: {}", name);
        self.tools.insert(name, tool);
    }

    /// 获取工具
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|b| &**b)
    }

    /// 列出所有工具
    pub fn list(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// 执行工具调用
    ///
    /// # Errors
    ///
    /// 如果工具不存在或执行失败，返回错误。
    pub async fn execute(&self, tool_call: &ToolCall) -> Result<String, JiaClawError> {
        let tool = self.get(&tool_call.tool_name).ok_or_else(|| {
            JiaClawError::ToolExecution(format!("工具不存在: {}", tool_call.tool_name))
        })?;

        tool.execute(tool_call.arguments.clone()).await
    }

    /// 执行工具调用，可选超时。
    ///
    /// `timeout_secs` 为 `None` 时与 [`Self::execute`] 行为相同（不限制）。
    /// 超时时返回 `JiaClawError::ToolExecution("Tool timed out after Ns")`，不 panic。
    ///
    /// # 策略
    ///
    /// 对 `Tool::execute` 的 Future 使用 `tokio::time::timeout`。
    /// `shell_exec` / `http_get` 等同步工作已在工具内部 `spawn_blocking`，
    /// 超时后本调用立即把错误交还给 tool loop；后台阻塞任务可能仍会跑完，
    /// 但不会继续卡住本轮循环。
    ///
    /// # Errors
    ///
    /// 如果工具不存在、执行失败或超时，返回错误。
    pub async fn execute_with_timeout(
        &self,
        tool_call: &ToolCall,
        timeout_secs: Option<u64>,
    ) -> Result<String, JiaClawError> {
        match timeout_secs {
            Some(secs) => {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(secs),
                    self.execute(tool_call),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(JiaClawError::ToolExecution(format!(
                        "Tool timed out after {secs}s"
                    ))),
                }
            }
            None => self.execute(tool_call).await,
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Workspace List 工具（列出工作空间文件）
pub struct WorkspaceListTool {
    workspace_path: PathBuf,
}

impl WorkspaceListTool {
    /// 创建新的工作空间列表工具
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: workspace_path.to_path_buf(),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceListTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "workspace_list"
    }

    fn description(&self) -> &str {
        "列出工作空间中的文件和目录结构"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value) -> Result<String, JiaClawError> {
        if !self.workspace_path.exists() {
            return Ok(format!(
                "工作空间不存在: {}\n提示: 运行 'jiaclaw init' 创建工作空间",
                self.workspace_path.display()
            ));
        }

        let mut result = format!("工作空间: {}\n\n", self.workspace_path.display());

        // 列出工作空间根文件
        let root_files = vec!["AGENTS.md", "SOUL.md", "USER.md", "MEMORY.md"];
        result.push_str("文件:\n");

        for file in &root_files {
            let file_path = self.workspace_path.join(file);
            if file_path.exists() {
                let metadata = std::fs::metadata(&file_path)
                    .map_err(|e| JiaClawError::ToolExecution(format!("无法读取文件元数据: {e}")))?;
                result.push_str(&format!("  • {} ({} bytes)\n", file, metadata.len()));
            } else {
                result.push_str(&format!("  • {file} (不存在)\n"));
            }
        }

        // 列出技能目录
        let skills_dir = self.workspace_path.join("skills");
        if skills_dir.exists() {
            result.push_str("\n技能:\n");

            let entries = std::fs::read_dir(&skills_dir)
                .map_err(|e| JiaClawError::ToolExecution(format!("无法读取技能目录: {e}")))?;

            for entry in entries {
                let entry = entry
                    .map_err(|e| JiaClawError::ToolExecution(format!("无法读取目录条目: {e}")))?;

                if entry.path().is_dir() {
                    let skill_name = entry.file_name();
                    let skill_file = entry.path().join("SKILL.md");
                    if skill_file.exists() {
                        result.push_str(&format!("  • {}/\n", skill_name.to_string_lossy()));
                    }
                }
            }
        } else {
            result.push_str("\n技能目录不存在\n");
        }

        Ok(result)
    }
}

/// Memory Read 工具（通过逻辑名称读取配置的记忆文件）
pub struct MemoryReadTool {
    workspace_path: PathBuf,
    memory_rel_path: String,
    soul_rel_path: String,
    user_rel_path: String,
}

impl MemoryReadTool {
    /// 使用默认 MEMORY / SOUL / USER 路径。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self::with_paths(
            workspace_path,
            jiaclaw_core::DEFAULT_MEMORY_PATH,
            jiaclaw_core::DEFAULT_SOUL_PATH,
            jiaclaw_core::DEFAULT_USER_PATH,
        )
    }

    /// 工具参数仍使用逻辑文件名；实际读取管理员配置的相对路径。
    #[must_use]
    pub fn with_paths(
        workspace_path: &Path,
        memory_rel_path: impl Into<String>,
        soul_rel_path: impl Into<String>,
        user_rel_path: impl Into<String>,
    ) -> Self {
        Self {
            workspace_path: workspace_path.to_path_buf(),
            memory_rel_path: memory_rel_path.into(),
            soul_rel_path: soul_rel_path.into(),
            user_rel_path: user_rel_path.into(),
        }
    }
}

#[async_trait]
impl Tool for MemoryReadTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "memory_read"
    }

    fn description(&self) -> &str {
        "按逻辑文件名读取配置的 MEMORY / USER / SOUL 或固定 AGENTS.md；最多返回32KiB，超限明确标记截断。禁止链接和特殊文件。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "enum": ["MEMORY.md", "USER.md", "SOUL.md", "AGENTS.md"],
                    "description": "逻辑文件名；MEMORY / USER / SOUL 使用配置中的路径"
                }
            },
            "required": ["file"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let file_name = args
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'file'".to_string()))?;
        let configured = match file_name {
            "MEMORY.md" => self.memory_rel_path.clone(),
            "SOUL.md" => self.soul_rel_path.clone(),
            "USER.md" => self.user_rel_path.clone(),
            "AGENTS.md" => "AGENTS.md".to_string(),
            _ => {
                return Err(JiaClawError::ToolExecution(
                    "不允许的逻辑文件名；仅允许 MEMORY.md、USER.md、SOUL.md、AGENTS.md".into(),
                ));
            }
        };
        let workspace = self.workspace_path.clone();
        let relative = configured.clone();
        let file = crate::memory_io::run_blocking(move || {
            crate::memory_io::read_text(
                &workspace,
                &relative,
                jiaclaw_core::MEMORY_PROMPT_MAX_BYTES,
                true,
            )
        })
        .await?;
        let Some(file) = file else {
            return Ok(format!(
                "文件不存在: {configured}\n提示: 运行 'jiaclaw init' 创建工作空间"
            ));
        };
        let truncation = if file.truncated {
            "\n已截断：仅返回前32KiB内完整UTF-8字符。"
        } else {
            ""
        };
        Ok(format!(
            "文件: {file_name}\n约定路径: {configured}\n大小: {} 字节；返回: {} 字节{truncation}\n\n{}",
            file.size_bytes,
            file.text.len(),
            file.text
        ))
    }
}

/// `file_read`：`read_file` 的兼容名称，共用配置、参数、输出与 I/O 边界。
pub struct FileReadTool {
    inner: crate::files::WorkspaceReadFileTool,
}

impl FileReadTool {
    /// 创建兼容名称工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            inner: crate::files::WorkspaceReadFileTool::new(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        self.inner.execute(args).await
    }
}

/// `file_write`：`write_file` 的兼容名称，共用配置、参数、输出与 I/O 边界。
pub struct FileWriteTool {
    inner: crate::files::WorkspaceWriteFileTool,
}

impl FileWriteTool {
    /// 创建兼容名称工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            inner: crate::files::WorkspaceWriteFileTool::new(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        self.inner.execute(args).await
    }
}

/// HTTP GET 工具
pub struct HttpGetTool;

impl HttpGetTool {
    /// 创建新的HTTP GET工具
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for HttpGetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpGetTool {
    fn name(&self) -> &str {
        "http_get"
    }

    fn description(&self) -> &str {
        "发送HTTP GET请求并返回响应内容"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "目标URL（必须是http或https）"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'url'".to_string()))?;

        // 验证URL格式
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(JiaClawError::ToolExecution(
                "URL必须以http://或https://开头".to_string(),
            ));
        }

        // 使用spawn_blocking执行同步HTTP请求
        let url_owned = url.to_string();
        tokio::task::spawn_blocking(move || {
            let response = minreq::get(&url_owned)
                .send()
                .map_err(|e| JiaClawError::ToolExecution(format!("HTTP请求失败: {e}")))?;

            let status = response.status_code;
            let body = response.as_str().unwrap_or_default().to_string();

            Ok(format!(
                "HTTP GET {}\n状态码: {}\n响应大小: {} 字节\n\n{}",
                url_owned,
                status,
                body.len(),
                if body.len() > 1000 {
                    format!(
                        "{}...\n\n[响应过长，已截断。完整大小: {} 字节]",
                        &body[..1000],
                        body.len()
                    )
                } else {
                    body
                }
            ))
        })
        .await
        .map_err(|e| JiaClawError::ToolExecution(format!("任务执行失败: {e}")))?
    }
}

/// Brave Search 默认端点
pub const DEFAULT_BRAVE_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// `web_search` HTTP 超时（秒）；仍遵守注册表级 `tool_timeout_secs`
pub const WEB_SEARCH_HTTP_TIMEOUT_SECS: u64 = 10;

/// `max_results` 缺省值
pub const WEB_SEARCH_DEFAULT_MAX_RESULTS: usize = 5;

/// `max_results` 上限（含）
pub const WEB_SEARCH_MAX_RESULTS: usize = 10;

/// 将 `max_results` 钳制到 `1..=10`。
#[must_use]
pub fn clamp_web_search_max_results(raw: u64) -> usize {
    usize::try_from(raw)
        .unwrap_or(WEB_SEARCH_MAX_RESULTS)
        .clamp(1, WEB_SEARCH_MAX_RESULTS)
}

/// 解析 `web_search` 参数：必填 `query`，可选 `max_results`（默认 5，钳制 1..=10）。
///
/// # Errors
///
/// `query` 缺失/空白，或 `max_results` 不是数字时返回错误。
pub fn parse_web_search_args(args: &Value) -> Result<(String, usize), JiaClawError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'query'（非空字符串）".to_string()))?;

    let max_results = match args.get("max_results") {
        None => WEB_SEARCH_DEFAULT_MAX_RESULTS,
        Some(value) => {
            let raw = value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
            match raw {
                Some(n) => clamp_web_search_max_results(n),
                None => {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'max_results' 必须是整数（将钳制到 1..=10）".to_string(),
                    ));
                }
            }
        }
    };

    Ok((query.to_string(), max_results))
}

fn percent_encode_query(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            b' ' => encoded.push_str("%20"),
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

const WEB_SEARCH_MISSING_KEY_HINT: &str = "web_search 需要 Brave Search API key。\
请设置环境变量 JIACLAW_BRAVE_API_KEY，或在配置中设置 [tools.web_search] brave_api_key。\
可在 https://brave.com/search/api/ 申请。若暂不使用该工具，设置 [tools.web_search] enabled = false。";

/// 联网检索工具（Brave Search；无 key 时返回友好错误，不访问网络）
pub struct WebSearchTool {
    api_key: Option<String>,
    endpoint: String,
}

impl WebSearchTool {
    /// 使用默认 Brave 端点创建工具
    #[must_use]
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            endpoint: DEFAULT_BRAVE_SEARCH_ENDPOINT.to_string(),
        }
    }

    /// 指定检索端点（测试可指向 mockito；生产默认 Brave）
    #[must_use]
    pub fn with_endpoint(api_key: Option<String>, endpoint: impl Into<String>) -> Self {
        Self {
            api_key,
            endpoint: endpoint.into(),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new(None)
    }
}

#[derive(serde::Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<BraveWebResults>,
}

#[derive(Default, serde::Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveSearchHit>,
}

#[derive(serde::Deserialize)]
struct BraveSearchHit {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default, alias = "snippet")]
    description: String,
}

#[derive(serde::Serialize)]
struct WebSearchResultItem {
    title: String,
    url: String,
    snippet: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "使用搜索引擎检索网页，返回若干条 {title, url, snippet}。默认 Brave Search（需 API key）；未配置 key 时返回友好错误。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索查询（必填）"
                },
                "max_results": {
                    "type": "integer",
                    "description": "返回条数（可选，默认 5，钳制到 1..=10）",
                    "minimum": 1,
                    "maximum": 10,
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let (query, max_results) = parse_web_search_args(&args)?;

        let Some(api_key) = self
            .api_key
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        else {
            return Err(JiaClawError::ToolExecution(
                WEB_SEARCH_MISSING_KEY_HINT.to_string(),
            ));
        };

        let endpoint = self.endpoint.clone();
        let api_key = api_key.to_string();

        tokio::task::spawn_blocking(move || {
            perform_brave_search(&endpoint, &api_key, &query, max_results)
        })
        .await
        .map_err(|e| JiaClawError::ToolExecution(format!("任务执行失败: {e}")))?
    }
}

fn perform_brave_search(
    endpoint: &str,
    api_key: &str,
    query: &str,
    max_results: usize,
) -> Result<String, JiaClawError> {
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!(
        "{endpoint}{separator}q={}&count={max_results}",
        percent_encode_query(query)
    );

    tracing::debug!("web_search 请求: {url}");

    let response = minreq::get(&url)
        .with_header("Accept", "application/json")
        .with_header("User-Agent", "JiaClaw/0.1 (web_search)")
        .with_header("X-Subscription-Token", api_key)
        .with_timeout(WEB_SEARCH_HTTP_TIMEOUT_SECS)
        .send()
        .map_err(|e| JiaClawError::ToolExecution(format!("web_search 请求失败: {e}")))?;

    let status = response.status_code;
    if status == 401 || status == 403 {
        return Err(JiaClawError::ToolExecution(
            format!(
                "Brave Search 拒绝了 API key（HTTP {status}）。请检查 JIACLAW_BRAVE_API_KEY 或 [tools.web_search] brave_api_key。"
            ),
        ));
    }
    if status == 429 {
        return Err(JiaClawError::ToolExecution(
            "Brave Search 触发限流（HTTP 429），请稍后重试。".to_string(),
        ));
    }
    if status != 200 {
        return Err(JiaClawError::ToolExecution(format!(
            "Brave Search 返回 HTTP {status}"
        )));
    }

    let parsed: BraveSearchResponse = response
        .json()
        .map_err(|e| JiaClawError::ToolExecution(format!("解析 Brave Search 响应失败: {e}")))?;

    let items: Vec<WebSearchResultItem> = parsed
        .web
        .unwrap_or_default()
        .results
        .into_iter()
        .take(max_results)
        .map(|hit| WebSearchResultItem {
            title: hit.title,
            url: hit.url,
            snippet: hit.description,
        })
        .collect();

    serde_json::to_string_pretty(&items)
        .map_err(|e| JiaClawError::ToolExecution(format!("序列化搜索结果失败: {e}")))
}

/// `web_fetch` HTTP 总体超时（秒）；仍遵守注册表级 `tool_timeout_secs`
pub const WEB_FETCH_HTTP_TIMEOUT_SECS: u64 = 15;

/// 最多跟随的重定向次数
pub const WEB_FETCH_MAX_REDIRECTS: usize = 5;

/// `max_chars` 缺省值
pub const WEB_FETCH_DEFAULT_MAX_CHARS: usize = 8000;

/// `max_chars` 下限（含）
pub const WEB_FETCH_MIN_CHARS: usize = 500;

/// `max_chars` 上限（含）
pub const WEB_FETCH_MAX_CHARS: usize = 50_000;

/// 原始响应用的读取上限，避免超大页面占满内存
const WEB_FETCH_MAX_BODY_BYTES: usize = 1_048_576;

const WEB_FETCH_USER_AGENT: &str = "JiaClaw/0.1 (web_fetch)";

/// 将 `max_chars` 钳制到 `500..=50000`。
#[must_use]
pub fn clamp_web_fetch_max_chars(raw: u64) -> usize {
    usize::try_from(raw)
        .unwrap_or(WEB_FETCH_MAX_CHARS)
        .clamp(WEB_FETCH_MIN_CHARS, WEB_FETCH_MAX_CHARS)
}

/// 解析 `web_fetch` 参数：必填 `url`，可选 `max_chars`（默认 8000，钳制 500..=50000）。
///
/// # Errors
///
/// `url` 缺失/空白，或 `max_chars` 不是数字时返回错误。
pub fn parse_web_fetch_args(args: &Value) -> Result<(String, usize), JiaClawError> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JiaClawError::ToolExecution("缺少参数 'url'（非空 http/https URL）".to_string())
        })?;

    let max_chars = match args.get("max_chars") {
        None => WEB_FETCH_DEFAULT_MAX_CHARS,
        Some(value) => {
            let raw = value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
            match raw {
                Some(n) => clamp_web_fetch_max_chars(n),
                None => {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'max_chars' 必须是整数（将钳制到 500..=50000）".to_string(),
                    ));
                }
            }
        }
    };

    Ok((url.to_string(), max_chars))
}

/// 校验 `web_fetch` URL：仅 http(s)，默认拒绝 localhost / 私网 / 链路本地。
///
/// # Errors
///
/// 非 http(s)、无法解析、或目标落在被拒绝网段时返回错误。
pub fn validate_web_fetch_url(url: &str, allow_private: bool) -> Result<(), JiaClawError> {
    let parsed = parse_http_url(url)?;
    if allow_private {
        return Ok(());
    }
    if host_is_obviously_private_or_local(&parsed.host) {
        return Err(private_url_error(url));
    }
    if resolved_host_has_private_ip(&parsed.host)? {
        return Err(private_url_error(url));
    }
    Ok(())
}

fn private_url_error(url: &str) -> JiaClawError {
    JiaClawError::ToolExecution(format!(
        "拒绝抓取私网或本地地址: {url}。默认阻止 localhost、127.0.0.0/8、::1、10/8、172.16/12、192.168/16 与链路本地。如需内网访问，设置 [tools.web_fetch] allow_private = true。"
    ))
}

struct ParsedHttpUrl {
    scheme: String,
    host: String,
    port: Option<u16>,
    path_and_query: String,
}

fn parse_http_url(raw: &str) -> Result<ParsedHttpUrl, JiaClawError> {
    let raw = raw.trim();
    let (scheme, rest) = if let Some(rest) = raw.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = raw.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(scheme_end) = raw.find("://") {
        let scheme = &raw[..scheme_end];
        return Err(JiaClawError::ToolExecution(format!(
            "仅允许 http/https URL，收到协议: {scheme}"
        )));
    } else {
        return Err(JiaClawError::ToolExecution(
            "URL 必须以 http:// 或 https:// 开头".to_string(),
        ));
    };

    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].to_string()),
        None => (rest, "/".to_string()),
    };

    if authority.is_empty() {
        return Err(JiaClawError::ToolExecution("URL 缺少主机名".to_string()));
    }

    let authority = match authority.rfind('@') {
        Some(idx) => &authority[idx + 1..],
        None => authority,
    };

    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let end = inner
            .find(']')
            .ok_or_else(|| JiaClawError::ToolExecution("IPv6 URL 缺少闭合括号".to_string()))?;
        let host = inner[..end].to_string();
        let after = &inner[end + 1..];
        let port = if after.is_empty() {
            None
        } else if let Some(port_str) = after.strip_prefix(':') {
            Some(parse_port(port_str)?)
        } else {
            return Err(JiaClawError::ToolExecution(
                "IPv6 URL 端口格式无效".to_string(),
            ));
        };
        (host, port)
    } else if let Some((host, port_str)) = split_host_port(authority) {
        (host.to_string(), Some(parse_port(port_str)?))
    } else {
        (authority.to_string(), None)
    };

    if host.is_empty() {
        return Err(JiaClawError::ToolExecution("URL 缺少主机名".to_string()));
    }

    Ok(ParsedHttpUrl {
        scheme: scheme.to_string(),
        host,
        port,
        path_and_query,
    })
}

fn split_host_port(authority: &str) -> Option<(&str, &str)> {
    let (host, port) = authority.rsplit_once(':')?;
    if host.is_empty() || port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((host, port))
}

fn parse_port(raw: &str) -> Result<u16, JiaClawError> {
    raw.parse::<u16>()
        .map_err(|_| JiaClawError::ToolExecution(format!("无效端口: {raw}")))
}

fn format_origin(parsed: &ParsedHttpUrl) -> String {
    match parsed.port {
        Some(port) if parsed.host.contains(':') => {
            format!("{}://[{}]:{port}", parsed.scheme, parsed.host)
        }
        Some(port) => format!("{}://{}:{port}", parsed.scheme, parsed.host),
        None if parsed.host.contains(':') => {
            format!("{}://[{}]", parsed.scheme, parsed.host)
        }
        None => format!("{}://{}", parsed.scheme, parsed.host),
    }
}

fn resolve_redirect_location(current: &str, location: &str) -> Result<String, JiaClawError> {
    let location = location.trim();
    if location.is_empty() {
        return Err(JiaClawError::ToolExecution(
            "重定向响应缺少 Location".to_string(),
        ));
    }
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if let Some(rest) = location.strip_prefix("//") {
        let parsed = parse_http_url(current)?;
        return Ok(format!("{}://{rest}", parsed.scheme));
    }

    let parsed = parse_http_url(current)?;
    let origin = format_origin(&parsed);
    if location.starts_with('/') {
        return Ok(format!("{origin}{location}"));
    }

    let path = parsed
        .path_and_query
        .split('?')
        .next()
        .unwrap_or(&parsed.path_and_query);
    let dir = match path.rfind('/') {
        Some(idx) => &path[..=idx],
        None => "/",
    };
    Ok(format!("{origin}{dir}{location}"))
}

fn host_is_obviously_private_or_local(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip_is_private_or_local(ip);
    }
    false
}

fn resolved_host_has_private_ip(host: &str) -> Result<bool, JiaClawError> {
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host_is_obviously_private_or_local(host));
    }
    let addrs = (host, 0u16)
        .to_socket_addrs()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析主机 {host}: {e}")))?;
    let ips: Vec<IpAddr> = addrs.map(|addr| addr.ip()).collect();
    if ips.is_empty() {
        return Err(JiaClawError::ToolExecution(format!(
            "无法解析主机 {host}: 没有地址"
        )));
    }
    Ok(ips.into_iter().any(ip_is_private_or_local))
}

fn ip_is_private_or_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_private_or_local(v4),
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ipv4_is_private_or_local(v4);
            }
            if let Some(v4) = v6.to_ipv4() {
                return ipv4_is_private_or_local(v4);
            }
            let segs = v6.segments();
            // fe80::/10 链路本地
            if segs[0] & 0xffc0 == 0xfe80 {
                return true;
            }
            // fc00::/7 unique local
            if segs[0] & 0xfe00 == 0xfc00 {
                return true;
            }
            false
        }
    }
}

fn ipv4_is_private_or_local(ip: Ipv4Addr) -> bool {
    ip.is_unspecified() || ip.is_loopback() || ip.is_private() || ip.is_link_local()
}

fn is_redirect_status(status: i32) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn header_value<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn content_type_is_html(content_type: &str, body: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("text/html") || ct.contains("application/xhtml") {
        return true;
    }
    if !ct.is_empty() {
        return false;
    }
    let trimmed = body.trim_start();
    let lower = trimmed.get(..32).unwrap_or(trimmed).to_ascii_lowercase();
    lower.starts_with("<!doctype html") || lower.starts_with("<html")
}

fn remaining_timeout_secs(deadline: Instant) -> Result<u64, JiaClawError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(JiaClawError::ToolExecution(format!(
            "web_fetch 超过 {WEB_FETCH_HTTP_TIMEOUT_SECS}s 超时"
        )));
    }
    Ok(remaining.as_secs().clamp(1, WEB_FETCH_HTTP_TIMEOUT_SECS))
}

/// 轻量 HTML → 可读文本：去掉 script/style，剥离标签，保留标题。
#[must_use]
pub fn html_to_readable_text(html: &str) -> (Option<String>, String) {
    let title = extract_html_title(html);
    let mut text = strip_elements_with_content(html, "script");
    text = strip_elements_with_content(&text, "style");
    text = strip_elements_with_content(&text, "noscript");
    text = strip_html_comments(&text);
    text = tags_to_text(&text);
    text = decode_basic_entities(&text);
    text = collapse_whitespace(&text);
    (title, text)
}

fn extract_html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start_tag = lower.find("<title")?;
    let start_inner = lower[start_tag..].find('>')? + start_tag + 1;
    let end_tag = lower[start_inner..].find("</title>")? + start_inner;
    let title = decode_basic_entities(&html[start_inner..end_tag]);
    let title = collapse_whitespace(&title);
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

fn strip_elements_with_content(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(rel) = lower[i..].find(&open) {
        let start = i + rel;
        let after = start + open.len();
        let boundary = lower.as_bytes().get(after).copied().unwrap_or(b'>');
        if !matches!(boundary, b'>' | b'/' | b' ' | b'\n' | b'\r' | b'\t') {
            out.push_str(&html[i..after]);
            i = after;
            continue;
        }
        out.push_str(&html[i..start]);
        if let Some(end_rel) = lower[after..].find(&close) {
            i = after + end_rel + close.len();
        } else {
            return out;
        }
    }
    out.push_str(&html[i..]);
    out
}

fn strip_html_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start + 4..].find("-->") {
            rest = &rest[start + 4 + end + 3..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn tags_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('>') {
            let tag = after[..end].trim();
            let name = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            let name = name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "p" | "div"
                    | "br"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "li"
                    | "tr"
                    | "section"
                    | "article"
                    | "header"
                    | "footer"
                    | "blockquote"
                    | "hr"
                    | "ul"
                    | "ol"
            ) {
                out.push('\n');
            } else {
                out.push(' ');
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

fn decode_basic_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find(';') {
            let entity = &after[..end];
            if let Some(ch) = decode_entity(entity) {
                out.push(ch);
            } else {
                out.push('&');
                out.push_str(entity);
                out.push(';');
            }
            rest = &after[end + 1..];
        } else {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        other => {
            if let Some(digits) = other
                .strip_prefix("#x")
                .or_else(|| other.strip_prefix("#X"))
            {
                u32::from_str_radix(digits, 16)
                    .ok()
                    .and_then(char::from_u32)
            } else if let Some(digits) = other.strip_prefix('#') {
                digits.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                None
            }
        }
    }
}

fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut newline_run = 0;
    let mut space_pending = false;
    for ch in input.chars() {
        if ch == '\n' || ch == '\r' {
            space_pending = false;
            newline_run += 1;
            if newline_run <= 2 {
                if ch == '\r' {
                    continue;
                }
                out.push('\n');
            }
            continue;
        }
        newline_run = 0;
        if ch.is_whitespace() {
            space_pending = !out.is_empty() && !out.ends_with('\n');
            continue;
        }
        if space_pending {
            out.push(' ');
            space_pending = false;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

fn format_web_fetch_output(
    final_url: &str,
    title: Option<&str>,
    body: &str,
    max_chars: usize,
) -> String {
    let mut truncated = false;
    let body = if body.chars().count() > max_chars {
        truncated = true;
        body.chars().take(max_chars).collect::<String>()
    } else {
        body.to_string()
    };

    let mut out = String::new();
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        out.push_str("Title: ");
        out.push_str(title);
        out.push('\n');
    }
    out.push_str("URL: ");
    out.push_str(final_url);
    out.push_str("\n\n");
    out.push_str(&body);
    if truncated {
        out.push_str("\n\n[truncated]");
    }
    out
}

/// 网页抓取工具：GET URL，HTML 去标签为可读文本
pub struct WebFetchTool {
    allow_private: bool,
}

impl WebFetchTool {
    /// 创建工具；`allow_private` 为 `true` 时允许 localhost / 私网。
    #[must_use]
    pub fn new(allow_private: bool) -> Self {
        Self { allow_private }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "抓取网页并返回可读纯文本（HTML 会去掉 script/style 与标签）。url 必填且仅限 http/https；可选 max_chars（默认 8000，钳制 500..=50000）。默认拒绝 localhost/私网。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要抓取的 URL（必填，仅 http/https）"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "返回正文最大字符数（可选，默认 8000，钳制到 500..=50000）",
                    "minimum": 500,
                    "maximum": 50000,
                    "default": 8000
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let (url, max_chars) = parse_web_fetch_args(&args)?;
        validate_web_fetch_url(&url, self.allow_private)?;
        let allow_private = self.allow_private;

        tokio::task::spawn_blocking(move || perform_web_fetch(&url, max_chars, allow_private))
            .await
            .map_err(|e| JiaClawError::ToolExecution(format!("任务执行失败: {e}")))?
    }
}

fn perform_web_fetch(
    start_url: &str,
    max_chars: usize,
    allow_private: bool,
) -> Result<String, JiaClawError> {
    let deadline = Instant::now() + Duration::from_secs(WEB_FETCH_HTTP_TIMEOUT_SECS);
    let mut current = start_url.to_string();

    for redirect_count in 0..=WEB_FETCH_MAX_REDIRECTS {
        validate_web_fetch_url(&current, allow_private)?;
        let timeout_secs = remaining_timeout_secs(deadline)?;
        tracing::debug!("web_fetch 请求: {current}");

        let response = minreq::get(&current)
            .with_header(
                "Accept",
                "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8",
            )
            .with_header("User-Agent", WEB_FETCH_USER_AGENT)
            .with_max_redirects(0)
            .with_timeout(timeout_secs)
            .send()
            .map_err(|e| JiaClawError::ToolExecution(format!("web_fetch 请求失败: {e}")))?;

        let status = response.status_code;
        if is_redirect_status(status) {
            if redirect_count == WEB_FETCH_MAX_REDIRECTS {
                return Err(JiaClawError::ToolExecution(format!(
                    "web_fetch 重定向超过 {WEB_FETCH_MAX_REDIRECTS} 次"
                )));
            }
            let location = header_value(&response.headers, "location").ok_or_else(|| {
                JiaClawError::ToolExecution(format!("web_fetch 收到 HTTP {status} 但缺少 Location"))
            })?;
            current = resolve_redirect_location(&current, location)?;
            continue;
        }

        if !(200..300).contains(&status) {
            return Err(JiaClawError::ToolExecution(format!(
                "web_fetch 返回 HTTP {status}（最终 URL: {current}）"
            )));
        }

        let content_type = header_value(&response.headers, "content-type")
            .unwrap_or("")
            .to_string();
        let raw = response.as_bytes();
        let truncated_download = raw.len() > WEB_FETCH_MAX_BODY_BYTES;
        let slice = if truncated_download {
            &raw[..WEB_FETCH_MAX_BODY_BYTES]
        } else {
            raw
        };
        let body = String::from_utf8_lossy(slice);

        let (title, text) = if content_type_is_html(&content_type, &body) {
            html_to_readable_text(&body)
        } else {
            (None, body.trim().to_string())
        };

        let mut output = format_web_fetch_output(&current, title.as_deref(), &text, max_chars);
        if truncated_download && !output.contains("[truncated]") {
            output.push_str("\n\n[truncated]");
        }
        return Ok(output);
    }

    Err(JiaClawError::ToolExecution(format!(
        "web_fetch 重定向超过 {WEB_FETCH_MAX_REDIRECTS} 次"
    )))
}

/// `DateTime` 工具
pub struct DateTimeTool;

impl DateTimeTool {
    /// 创建新的 `DateTime` 工具
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for DateTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DateTimeTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "datetime_now"
    }

    fn description(&self) -> &str {
        "获取当前日期和时间（UTC）"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value) -> Result<String, JiaClawError> {
        use std::time::SystemTime;

        let now = SystemTime::now();
        let duration_since_epoch = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| JiaClawError::ToolExecution(format!("系统时间错误: {e}")))?;

        let timestamp = duration_since_epoch.as_secs();

        // 简单的UTC时间格式化
        let days_since_epoch = timestamp / 86400;
        let seconds_today = timestamp % 86400;
        let hours = seconds_today / 3600;
        let minutes = (seconds_today % 3600) / 60;
        let seconds = seconds_today % 60;

        // 简单的日期计算（从1970-01-01开始）
        let year = 1970 + days_since_epoch / 365; // 简化计算

        Ok(format!(
            "当前时间 (UTC):\n\
             Unix时间戳: {timestamp}\n\
             大约时间: {year}-??-?? {hours:02}:{minutes:02}:{seconds:02}\n\n\
             注意: 这是简化的时间表示。完整的日期时间功能将在后续迭代中添加。"
        ))
    }
}

/// JSON Query 工具
pub struct JsonQueryTool;

impl JsonQueryTool {
    /// 创建新的JSON查询工具
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonQueryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for JsonQueryTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "json_query"
    }

    fn description(&self) -> &str {
        "解析JSON字符串并提取指定路径的值（使用点号表示法，如：user.name）"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "json": {
                    "type": "string",
                    "description": "JSON字符串"
                },
                "path": {
                    "type": "string",
                    "description": "查询路径（可选，留空返回整个JSON）"
                }
            },
            "required": ["json"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let json_str = args
            .get("json")
            .and_then(|v| v.as_str())
            .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'json'".to_string()))?;

        let path = args.get("path").and_then(|v| v.as_str());

        // 解析JSON
        let json_value: Value = serde_json::from_str(json_str)
            .map_err(|e| JiaClawError::ToolExecution(format!("JSON解析失败: {e}")))?;

        // 如果没有指定路径，返回格式化的整个JSON
        if path.is_none() || path == Some("") {
            let pretty = serde_json::to_string_pretty(&json_value)
                .unwrap_or_else(|_| json_value.to_string());
            return Ok(format!("JSON (格式化):\n\n{pretty}"));
        }

        // 处理路径查询
        let path = path.unwrap();
        let parts: Vec<&str> = path.split('.').collect();

        let mut current = &json_value;
        for part in &parts {
            current = current
                .get(part)
                .ok_or_else(|| JiaClawError::ToolExecution(format!("路径 '{path}' 不存在")))?;
        }

        let result = serde_json::to_string_pretty(current).unwrap_or_else(|_| current.to_string());

        Ok(format!("查询路径: {path}\n结果:\n\n{result}"))
    }
}

/// `file_list`：`list_dir` 的兼容名称，共用配置、参数、输出与 I/O 边界。
pub struct FileListTool {
    inner: crate::files::WorkspaceListDirTool,
}

impl FileListTool {
    /// 创建兼容名称工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            inner: crate::files::WorkspaceListDirTool::new(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for FileListTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "file_list"
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        self.inner.execute(args).await
    }
}

/// `file_delete`：`delete_file` 的兼容名称，共用配置、参数、输出与 I/O 边界。
pub struct FileDeleteTool {
    inner: crate::files::WorkspaceDeleteFileTool,
}

impl FileDeleteTool {
    /// 创建兼容名称工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            inner: crate::files::WorkspaceDeleteFileTool::new(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for FileDeleteTool {
    fn name(&self) -> &str {
        "file_delete"
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        self.inner.execute(args).await
    }
}

/// File Copy 工具（复制文件）
pub struct FileCopyTool {
    workspace_path: PathBuf,
}

impl FileCopyTool {
    /// 创建新的文件复制工具
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        // 规范化工作空间路径，确保沙箱检查在所有平台上一致
        let canonical_workspace = workspace_path
            .canonicalize()
            .unwrap_or_else(|_| workspace_path.to_path_buf());

        Self {
            workspace_path: canonical_workspace,
        }
    }
}

#[async_trait]
impl Tool for FileCopyTool {
    fn name(&self) -> &str {
        "file_copy"
    }

    fn description(&self) -> &str {
        "在工作空间内复制文件"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "源文件相对路径"
                },
                "destination": {
                    "type": "string",
                    "description": "目标文件相对路径"
                }
            },
            "required": ["source", "destination"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let tool = crate::WorkspaceCopyTool::legacy(&self.workspace_path);
        let output = tool.execute(args).await?;
        Ok(format!("✅ 文件已复制: {output}"))
    }
}

/// Shell Exec 工具（白名单模式）
pub struct ShellExecTool {
    workspace_path: PathBuf,
}

impl ShellExecTool {
    /// 创建新的Shell执行工具
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: workspace_path.to_path_buf(),
        }
    }
}

#[async_trait]
impl Tool for ShellExecTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn description(&self) -> &str {
        "已停用的旧构造器；通过 tools.exec 显式配置 Docker 沙箱"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的命令（仅白名单命令）"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "命令参数列表（可选）"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let _ = args;
        let _ = &self.workspace_path;
        Err(JiaClawError::ToolExecution("shell_exec 的宿主机执行已移除；执行权限默认关闭，使用 tools.exec 配置 Docker 沙箱和安全白名单".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_workspace_list_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_tools_workspace");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        // 创建测试文件
        fs::write(temp_workspace.join("MEMORY.md"), "test memory").unwrap();
        fs::write(temp_workspace.join("USER.md"), "test user").unwrap();

        let tool = WorkspaceListTool::new(&temp_workspace);

        assert_eq!(tool.name(), "workspace_list");
        assert!(!tool.description().is_empty());

        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert!(result.contains("MEMORY.md"));
        assert!(result.contains("USER.md"));

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[tokio::test]
    async fn test_memory_read_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_tools_memory");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        let test_content = "# Test Memory\n\nThis is test content.";
        fs::write(temp_workspace.join("MEMORY.md"), test_content).unwrap();

        let tool = MemoryReadTool::new(&temp_workspace);

        assert_eq!(tool.name(), "memory_read");

        let result = tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .unwrap();

        assert!(result.contains(test_content));

        // 测试无效文件
        let result = tool
            .execute(serde_json::json!({"file": "invalid.txt"}))
            .await;
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[tokio::test]
    async fn memory_read_resolves_configured_names_without_root_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("notes")).unwrap();
        for name in ["MEMORY.md", "SOUL.md", "USER.md"] {
            fs::write(root.join(name), "root-decoy").unwrap();
        }
        for (path, content) in [
            ("notes/memory.txt", "configured-memory"),
            ("notes/soul.txt", "configured-soul"),
            ("notes/user.txt", "configured-user"),
            ("AGENTS.md", "fixed-agents"),
        ] {
            fs::write(root.join(path), content).unwrap();
        }
        let tool = MemoryReadTool::with_paths(
            root,
            "notes/memory.txt",
            "notes/soul.txt",
            "notes/user.txt",
        );
        for (name, path, expected) in [
            ("MEMORY.md", "notes/memory.txt", "configured-memory"),
            ("SOUL.md", "notes/soul.txt", "configured-soul"),
            ("USER.md", "notes/user.txt", "configured-user"),
            ("AGENTS.md", "AGENTS.md", "fixed-agents"),
        ] {
            let result = tool
                .execute(serde_json::json!({"file": name}))
                .await
                .unwrap();
            assert!(result.contains(expected), "{name}: {result}");
            assert!(result.contains(path), "{name}: {result}");
            assert!(!result.contains("root-decoy"));
        }
        fs::remove_file(root.join("notes/memory.txt")).unwrap();
        let missing = tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .unwrap();
        assert!(missing.contains("文件不存在"));
        assert!(!missing.contains("root-decoy"));
        for file in ["../MEMORY.md", "/MEMORY.md", "notes/memory.txt"] {
            assert!(tool
                .execute(serde_json::json!({"file": file}))
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn memory_read_bounds_unicode_and_reports_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let limit = jiaclaw_core::MEMORY_PROMPT_MAX_BYTES;
        let body = format!("{}中tail-must-be-omitted", "x".repeat(limit - 1));
        fs::write(temp.path().join("MEMORY.md"), body).unwrap();
        let tool = MemoryReadTool::new(temp.path());
        let result = tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .unwrap();
        assert!(result.contains("已截断"));
        let (_, returned) = result.split_once("\n\n").unwrap();
        assert_eq!(returned.len(), limit - 1);
        assert!(!result.contains("tail-must-be-omitted"));
        fs::write(temp.path().join("MEMORY.md"), "").unwrap();
        let empty = tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .unwrap();
        assert!(empty.contains("返回: 0 字节"));
        assert!(!empty.contains("文件不存在"));
        fs::write(temp.path().join("MEMORY.md"), [b'x', 0xff, b'y']).unwrap();
        assert!(tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn memory_read_rejects_symlink_hardlink_and_directory() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("private.txt");
        fs::write(&outside, "private-outside").unwrap();
        let target = root.join("MEMORY.md");
        let tool = MemoryReadTool::new(&root);
        symlink(&outside, &target).unwrap();
        assert!(tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .is_err());
        fs::remove_file(&target).unwrap();
        fs::hard_link(&outside, &target).unwrap();
        assert!(tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .is_err());
        fs::remove_file(&target).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(tool
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .is_err());
        fs::remove_dir(&target).unwrap();
        let linked_parent = root.join("notes");
        symlink(temp.path(), &linked_parent).unwrap();
        let configured =
            MemoryReadTool::with_paths(&root, "notes/private.txt", "SOUL.md", "USER.md");
        assert!(configured
            .execute(serde_json::json!({"file": "MEMORY.md"}))
            .await
            .is_err());
    }

    #[test]
    fn test_tool_registry() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_registry");

        let mut registry = ToolRegistry::new();

        let tool = Box::new(WorkspaceListTool::new(&temp_workspace));
        registry.register(tool);

        assert!(registry.get("workspace_list").is_some());
        assert!(registry.get("nonexistent").is_none());

        let tools = registry.list();
        assert!(tools.contains(&"workspace_list"));
    }

    struct SlowSleepTool {
        delay: std::time::Duration,
    }

    #[async_trait]
    impl Tool for SlowSleepTool {
        fn name(&self) -> &str {
            "slow_sleep"
        }

        fn description(&self) -> &str {
            "test-only slow tool"
        }

        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        async fn execute(&self, _args: Value) -> Result<String, JiaClawError> {
            tokio::time::sleep(self.delay).await;
            Ok("slept".to_string())
        }
    }

    fn slow_sleep_call() -> ToolCall {
        ToolCall {
            tool_name: "slow_sleep".to_string(),
            arguments: serde_json::json!({}),
            result: None,
        }
    }

    #[tokio::test]
    async fn execute_without_timeout_completes_slow_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(SlowSleepTool {
            delay: std::time::Duration::from_millis(50),
        }));
        let result = registry
            .execute_with_timeout(&slow_sleep_call(), None)
            .await
            .unwrap();
        assert_eq!(result, "slept");
    }

    #[tokio::test]
    async fn execute_with_short_timeout_returns_timeout_error() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(SlowSleepTool {
            delay: std::time::Duration::from_secs(10),
        }));
        let err = registry
            .execute_with_timeout(&slow_sleep_call(), Some(1))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Tool timed out after 1s"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_file_read_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_file_read");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        let test_content = "Test file content";
        fs::write(temp_workspace.join("test.txt"), test_content).unwrap();

        let tool = FileReadTool::new(&temp_workspace);

        assert_eq!(tool.name(), "file_read");

        let result = tool
            .execute(serde_json::json!({"path": "test.txt"}))
            .await
            .unwrap();

        assert!(result.contains(test_content));
        assert!(result.contains("test.txt"));

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[tokio::test]
    async fn test_file_write_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_file_write");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        let tool = FileWriteTool::new(&temp_workspace);

        assert_eq!(tool.name(), "file_write");

        let test_content = "Written content";
        let result = tool
            .execute(serde_json::json!({
                "path": "output.txt",
                "content": test_content
            }))
            .await
            .unwrap();

        let output: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(output["path"], "output.txt");
        assert_eq!(output["mode"], "overwrite");
        assert_eq!(output["bytes_written"], test_content.len());

        // 验证文件确实被写入
        let file_path = temp_workspace.join("output.txt");
        assert!(file_path.exists());
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, test_content);

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[test]
    fn file_aliases_share_canonical_schemas_and_descriptions() {
        let workspace = tempfile::tempdir().unwrap();
        let pairs: Vec<(Box<dyn Tool>, Box<dyn Tool>)> = vec![
            (
                Box::new(FileReadTool::new(workspace.path())),
                Box::new(crate::WorkspaceReadFileTool::new(workspace.path())),
            ),
            (
                Box::new(FileWriteTool::new(workspace.path())),
                Box::new(crate::WorkspaceWriteFileTool::new(workspace.path())),
            ),
            (
                Box::new(FileListTool::new(workspace.path())),
                Box::new(crate::WorkspaceListDirTool::new(workspace.path())),
            ),
            (
                Box::new(FileDeleteTool::new(workspace.path())),
                Box::new(crate::WorkspaceDeleteFileTool::new(workspace.path())),
            ),
        ];
        for (alias, canonical) in pairs {
            assert_ne!(alias.name(), canonical.name());
            assert_eq!(alias.parameters_schema(), canonical.parameters_schema());
            assert_eq!(alias.description(), canonical.description());
        }
    }

    #[tokio::test]
    async fn file_aliases_share_bounded_outputs_and_failure_semantics() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("note.txt"), "first\nsecond\nthird").unwrap();
        let args = serde_json::json!({"path":"note.txt", "offset":2, "limit":1});
        let legacy = FileReadTool::new(workspace.path())
            .execute(args.clone())
            .await
            .unwrap();
        let canonical = crate::WorkspaceReadFileTool::new(workspace.path())
            .execute(args)
            .await
            .unwrap();
        assert_eq!(legacy, canonical);
        let output: Value = serde_json::from_str(&legacy).unwrap();
        assert_eq!(output["content"], "second");
        assert_eq!(output["returned_lines"], 1);

        let oversized = "x".repeat(crate::WRITE_FILE_MAX_BYTES + 1);
        assert!(FileWriteTool::new(workspace.path())
            .execute(serde_json::json!({
                "path":"note.txt", "content":oversized,
            }))
            .await
            .is_err());
        assert_eq!(
            fs::read_to_string(workspace.path().join("note.txt")).unwrap(),
            "first\nsecond\nthird"
        );
        assert!(FileReadTool::new(workspace.path())
            .execute(serde_json::json!({"path":"missing"}))
            .await
            .is_err());
        assert!(FileListTool::new(workspace.path())
            .execute(serde_json::json!({"path":"missing"}))
            .await
            .is_err());
        assert!(FileDeleteTool::new(workspace.path())
            .execute(serde_json::json!({"path":"missing"}))
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_write_alias_never_follows_leaf_or_missing_parent_symlinks() {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("sentinel.txt");
        fs::write(&sentinel, "outside unchanged").unwrap();
        symlink(&sentinel, workspace.path().join("leaf.txt")).unwrap();
        symlink(outside.path(), workspace.path().join("directory")).unwrap();
        let tool = FileWriteTool::new(workspace.path());
        for path in ["leaf.txt", "directory/missing/escape.txt"] {
            assert!(
                tool.execute(serde_json::json!({"path":path, "content":"escaped"}))
                    .await
                    .is_err(),
                "{path}"
            );
        }
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "outside unchanged");
        assert!(!outside.path().join("missing").exists());
        assert!(fs::symlink_metadata(workspace.path().join("leaf.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[tokio::test]
    async fn test_http_get_tool() {
        let tool = HttpGetTool::new();

        assert_eq!(tool.name(), "http_get");

        // 测试无效URL
        let result = tool
            .execute(serde_json::json!({"url": "invalid-url"}))
            .await;
        assert!(result.is_err());

        // 注意: 实际的HTTP测试需要mock服务器，这里仅测试工具结构
    }

    #[tokio::test]
    async fn test_datetime_tool() {
        let tool = DateTimeTool::new();

        assert_eq!(tool.name(), "datetime_now");

        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert!(result.contains("Unix时间戳"));
        assert!(result.contains("UTC"));
    }

    #[tokio::test]
    async fn test_json_query_tool() {
        let tool = JsonQueryTool::new();

        assert_eq!(tool.name(), "json_query");

        let test_json = r#"{"user": {"name": "Alice", "age": 30}}"#;

        // 测试完整JSON
        let result = tool
            .execute(serde_json::json!({"json": test_json}))
            .await
            .unwrap();
        assert!(result.contains("Alice"));

        // 测试路径查询
        let result = tool
            .execute(serde_json::json!({
                "json": test_json,
                "path": "user.name"
            }))
            .await
            .unwrap();
        assert!(result.contains("Alice"));

        // 测试无效路径
        let result = tool
            .execute(serde_json::json!({
                "json": test_json,
                "path": "invalid.path"
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_list_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_file_list");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        // 创建测试文件和目录
        fs::write(temp_workspace.join("file1.txt"), "content1").unwrap();
        fs::write(temp_workspace.join("file2.txt"), "content2").unwrap();
        fs::create_dir_all(temp_workspace.join("subdir")).unwrap();

        let tool = FileListTool::new(&temp_workspace);

        assert_eq!(tool.name(), "file_list");

        let result = tool.execute(serde_json::json!({})).await.unwrap();

        assert!(result.contains("file1.txt"));
        assert!(result.contains("file2.txt"));
        let output: Value = serde_json::from_str(&result).unwrap();
        assert!(output["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "subdir" && entry["type"] == "dir"));

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[tokio::test]
    async fn test_file_delete_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_file_delete");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        let test_file = temp_workspace.join("to_delete.txt");
        fs::write(&test_file, "delete me").unwrap();

        let tool = FileDeleteTool::new(&temp_workspace);

        assert_eq!(tool.name(), "file_delete");

        assert!(test_file.exists());

        let result = tool
            .execute(serde_json::json!({"path": "to_delete.txt"}))
            .await
            .unwrap();

        let output: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(output["path"], "to_delete.txt");
        assert_eq!(output["deleted"], true);
        assert!(!test_file.exists());

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[tokio::test]
    async fn test_file_copy_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_file_copy");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        let test_content = "copy this content";
        fs::write(temp_workspace.join("source.txt"), test_content).unwrap();

        let tool = FileCopyTool::new(&temp_workspace);

        assert_eq!(tool.name(), "file_copy");

        let result = tool
            .execute(serde_json::json!({
                "source": "source.txt",
                "destination": "dest.txt"
            }))
            .await
            .unwrap();

        assert!(result.contains("已复制"));

        let dest_path = temp_workspace.join("dest.txt");
        assert!(dest_path.exists());
        let content = fs::read_to_string(&dest_path).unwrap();
        assert_eq!(content, test_content);

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[tokio::test]
    async fn test_shell_exec_tool() {
        let temp_workspace = std::env::temp_dir().join("jiaclaw_test_shell");
        let _ = fs::remove_dir_all(&temp_workspace);
        fs::create_dir_all(&temp_workspace).unwrap();

        let tool = ShellExecTool::new(&temp_workspace);

        assert_eq!(tool.name(), "shell_exec");

        let result = tool
            .execute(serde_json::json!({"command":"echo","args":["hello"]}))
            .await;
        assert!(result.unwrap_err().to_string().contains("默认关闭"));

        let _ = fs::remove_dir_all(&temp_workspace);
    }

    #[test]
    fn parse_web_search_args_requires_non_empty_query() {
        let err = parse_web_search_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("query"), "{err}");

        let err = parse_web_search_args(&serde_json::json!({"query": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("query"), "{err}");
    }

    #[test]
    fn parse_web_search_args_defaults_and_clamps_max_results() {
        let (query, max_results) =
            parse_web_search_args(&serde_json::json!({"query": " rust "})).unwrap();
        assert_eq!(query, "rust");
        assert_eq!(max_results, WEB_SEARCH_DEFAULT_MAX_RESULTS);

        let (_, max_results) =
            parse_web_search_args(&serde_json::json!({"query": "q", "max_results": 1})).unwrap();
        assert_eq!(max_results, 1);

        let (_, max_results) =
            parse_web_search_args(&serde_json::json!({"query": "q", "max_results": 0})).unwrap();
        assert_eq!(max_results, 1);

        let (_, max_results) =
            parse_web_search_args(&serde_json::json!({"query": "q", "max_results": 99})).unwrap();
        assert_eq!(max_results, WEB_SEARCH_MAX_RESULTS);

        let err = parse_web_search_args(&serde_json::json!({
            "query": "q",
            "max_results": "nope"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("max_results"), "{err}");
    }

    #[test]
    fn clamp_web_search_max_results_bounds() {
        assert_eq!(clamp_web_search_max_results(0), 1);
        assert_eq!(clamp_web_search_max_results(5), 5);
        assert_eq!(clamp_web_search_max_results(10), 10);
        assert_eq!(clamp_web_search_max_results(11), 10);
    }

    #[tokio::test]
    async fn web_search_without_key_returns_friendly_error() {
        let tool = WebSearchTool::new(None);
        assert_eq!(tool.name(), "web_search");
        let err = tool
            .execute(serde_json::json!({"query": "hello"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("JIACLAW_BRAVE_API_KEY"), "{err}");
        assert!(err.contains("tools.web_search"), "{err}");
        assert!(
            !err.to_lowercase().contains("bsa-"),
            "must not leak api key: {err}"
        );
    }

    #[tokio::test]
    async fn web_search_blank_key_returns_friendly_error() {
        let tool = WebSearchTool::new(Some("   ".to_string()));
        let err = tool
            .execute(serde_json::json!({"query": "hello"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Brave Search API key"), "{err}");
    }

    #[tokio::test]
    async fn web_search_with_key_hits_brave_endpoint_and_parses_results() {
        let path = "/web-search-parse";
        let _mock = mockito::mock("GET", path)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("q".into(), "hello world".into()),
                mockito::Matcher::UrlEncoded("count".into(), "2".into()),
            ]))
            .match_header("X-Subscription-Token", "test-brave-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"web":{"results":[
                    {"title":"Hello","url":"https://example.com/hello","description":"A greeting"},
                    {"title":"World","url":"https://example.com/world","snippet":"The planet"}
                ]}}"#,
            )
            .create();

        let tool = WebSearchTool::with_endpoint(
            Some("test-brave-key".to_string()),
            format!("{}{path}", mockito::server_url()),
        );
        let result = tool
            .execute(serde_json::json!({"query": "hello world", "max_results": 2}))
            .await
            .unwrap();

        assert!(result.contains("Hello"), "{result}");
        assert!(result.contains("https://example.com/hello"), "{result}");
        assert!(result.contains("A greeting"), "{result}");
        assert!(result.contains("The planet"), "{result}");
        assert!(
            !result.contains("test-brave-key"),
            "must not echo api key: {result}"
        );
    }

    #[tokio::test]
    async fn web_search_unauthorized_does_not_echo_key() {
        let path = "/web-search-401";
        let _mock = mockito::mock("GET", path)
            .match_query(mockito::Matcher::Any)
            .match_header("X-Subscription-Token", "secret-key-value")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create();

        let tool = WebSearchTool::with_endpoint(
            Some("secret-key-value".to_string()),
            format!("{}{path}", mockito::server_url()),
        );
        let err = tool
            .execute(serde_json::json!({"query": "hello"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("401"), "{err}");
        assert!(
            !err.contains("secret-key-value"),
            "must not leak api key: {err}"
        );
    }

    #[test]
    fn parse_web_fetch_args_requires_non_empty_url() {
        let err = parse_web_fetch_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("url"), "{err}");

        let err = parse_web_fetch_args(&serde_json::json!({"url": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("url"), "{err}");
    }

    #[test]
    fn parse_web_fetch_args_defaults_and_clamps_max_chars() {
        let (url, max_chars) =
            parse_web_fetch_args(&serde_json::json!({"url": " https://example.com "})).unwrap();
        assert_eq!(url, "https://example.com");
        assert_eq!(max_chars, WEB_FETCH_DEFAULT_MAX_CHARS);

        let (_, max_chars) = parse_web_fetch_args(&serde_json::json!({
            "url": "https://example.com",
            "max_chars": 500
        }))
        .unwrap();
        assert_eq!(max_chars, WEB_FETCH_MIN_CHARS);

        let (_, max_chars) = parse_web_fetch_args(&serde_json::json!({
            "url": "https://example.com",
            "max_chars": 10
        }))
        .unwrap();
        assert_eq!(max_chars, WEB_FETCH_MIN_CHARS);

        let (_, max_chars) = parse_web_fetch_args(&serde_json::json!({
            "url": "https://example.com",
            "max_chars": 99_999
        }))
        .unwrap();
        assert_eq!(max_chars, WEB_FETCH_MAX_CHARS);

        let err = parse_web_fetch_args(&serde_json::json!({
            "url": "https://example.com",
            "max_chars": "nope"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("max_chars"), "{err}");
    }

    #[test]
    fn clamp_web_fetch_max_chars_bounds() {
        assert_eq!(clamp_web_fetch_max_chars(0), WEB_FETCH_MIN_CHARS);
        assert_eq!(clamp_web_fetch_max_chars(8000), 8000);
        assert_eq!(clamp_web_fetch_max_chars(50_000), WEB_FETCH_MAX_CHARS);
        assert_eq!(clamp_web_fetch_max_chars(50_001), WEB_FETCH_MAX_CHARS);
    }

    #[test]
    fn validate_web_fetch_url_rejects_non_http_and_private() {
        for url in [
            "ftp://example.com/file",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "example.com",
        ] {
            let err = validate_web_fetch_url(url, false).unwrap_err().to_string();
            assert!(
                err.contains("http") || err.contains("https") || err.contains("协议"),
                "unexpected error for {url}: {err}"
            );
        }

        for url in [
            "http://127.0.0.1/",
            "http://127.0.0.1:8080/foo",
            "http://localhost/secret",
            "http://LOCALHOST/secret",
            "http://[::1]/",
            "http://10.0.0.1/",
            "http://172.16.0.1/",
            "http://172.31.255.1/",
            "http://192.168.1.1/",
            "http://169.254.1.1/",
        ] {
            let err = validate_web_fetch_url(url, false).unwrap_err().to_string();
            assert!(err.contains("私网") || err.contains("本地"), "{url}: {err}");
        }

        validate_web_fetch_url("https://1.1.1.1/path", false).unwrap();
        validate_web_fetch_url("http://172.15.0.1/", false).unwrap();
        validate_web_fetch_url("http://127.0.0.1/", true).unwrap();
        validate_web_fetch_url("http://192.168.0.5/internal", true).unwrap();
    }

    #[test]
    fn html_to_readable_text_strips_script_and_keeps_body() {
        let html = r#"
            <html>
              <head>
                <title>Demo Title</title>
                <script>alert('xss')</script>
                <style>body { color: red; }</style>
              </head>
              <body>
                <p>Hello visible</p>
                <script>secret_token</script>
              </body>
            </html>
        "#;
        let (title, text) = html_to_readable_text(html);
        assert_eq!(title.as_deref(), Some("Demo Title"));
        assert!(text.contains("Hello visible"), "{text}");
        assert!(!text.contains("alert"), "{text}");
        assert!(!text.contains("xss"), "{text}");
        assert!(!text.contains("secret_token"), "{text}");
        assert!(!text.contains("color: red"), "{text}");
    }

    #[tokio::test]
    async fn web_fetch_rejects_private_url_without_network() {
        let tool = WebFetchTool::new(false);
        assert_eq!(tool.name(), "web_fetch");
        let err = tool
            .execute(serde_json::json!({"url": "http://127.0.0.1/"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("私网") || err.contains("本地"), "{err}");
    }

    #[tokio::test]
    async fn web_fetch_html_becomes_text_without_script() {
        let path = "/web-fetch-html";
        let _mock = mockito::mock("GET", path)
            .match_header("User-Agent", WEB_FETCH_USER_AGENT)
            .with_status(200)
            .with_header("content-type", "text/html; charset=utf-8")
            .with_body(
                r#"<html><head><title>Demo Title</title>
                <script>alert('xss')</script></head>
                <body><p>Hello visible</p></body></html>"#,
            )
            .create();

        let tool = WebFetchTool::new(true);
        let url = format!("{}{path}", mockito::server_url());
        let result = tool.execute(serde_json::json!({"url": url})).await.unwrap();

        assert!(result.contains("Demo Title"), "{result}");
        assert!(result.contains("Hello visible"), "{result}");
        assert!(result.contains("URL: "), "{result}");
        assert!(!result.contains("alert"), "{result}");
        assert!(!result.contains("xss"), "{result}");
        assert!(!result.contains("<p>"), "{result}");
    }

    #[tokio::test]
    async fn web_fetch_truncates_long_text() {
        let path = "/web-fetch-long";
        let long_body = "A".repeat(20_000);
        let html = format!("<html><body><p>{long_body}</p></body></html>");
        let _mock = mockito::mock("GET", path)
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body(html)
            .create();

        let tool = WebFetchTool::new(true);
        let url = format!("{}{path}", mockito::server_url());
        let result = tool
            .execute(serde_json::json!({"url": url, "max_chars": 500}))
            .await
            .unwrap();

        assert!(result.contains("[truncated]"), "{result}");
        let body = result.split("\n\n").nth(1).unwrap_or(&result);
        let body = body.replace("[truncated]", "");
        assert!(
            body.chars().count() <= WEB_FETCH_MIN_CHARS + 20,
            "body too long: {}",
            body.chars().count()
        );
        assert!(result.contains('A'), "{result}");
    }
}
