// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `JiaClaw` - 基于 `StateKnot` 的个人持久化智能体运行时
//!
//! `JiaClaw` 是一个"爪式"（Claw-style）个人助手智能体运行时，提供：
//! - 聊天驱动的交互界面
//! - 工具使用能力
//! - 技能系统
//! - 持久化运行（支持重启后恢复）

pub use jiaclaw_core::{
    AgentConfig, ChatMessage, ChatRequest, ChatResponse, DeleteFileToolConfig, GlobToolConfig,
    GrepToolConfig, HeartbeatConfig, HttpConfig, IdentityConfig, JiaClawError, ListDirToolConfig,
    MemoryConfig, MemorySearchToolConfig, MemoryWriteToolConfig, MessageRole, MkdirToolConfig,
    ModelPurpose, ModelRoute, ModelRoutingConfig, ModelSelection, MoveToolConfig, ProviderConfig,
    ReadFileToolConfig, RunStatus, SessionConfig, StatToolConfig, StrReplaceToolConfig, ToolCall,
    ToolsConfig, TreeToolConfig, WebFetchToolConfig, WebSearchToolConfig, WriteFileToolConfig,
    DEFAULT_HEARTBEAT_INTERVAL_SECS, DEFAULT_HEARTBEAT_PATH, DEFAULT_HEARTBEAT_SESSION_ID,
    DEFAULT_HTTP_MAX_BODY_BYTES, DEFAULT_HTTP_SHUTDOWN_TIMEOUT_SECS, DEFAULT_MAX_TOOL_ITERATIONS,
    DEFAULT_MEMORY_PATH, DEFAULT_SESSION_KEEP_RECENT, DEFAULT_SOUL_PATH, DEFAULT_USER_PATH,
    MAX_MAX_TOOL_ITERATIONS, MAX_SESSION_MESSAGES, MEMORY_PROMPT_MAX_BYTES,
    MIN_MAX_TOOL_ITERATIONS,
};

mod copy;
mod exec;
mod files;
mod filesystem_info;
pub use filesystem_info::{StatOutput, TreeOutput, WorkspaceStatTool, WorkspaceTreeTool};
mod glob_pattern;
pub use copy::{copy_workspace, CopyOutput, WorkspaceCopyTool, COPY_MAX_BYTES};
pub use exec::{validate_exec_config, ControlledExecTool};
mod heartbeat;
mod identity;
mod mcp;
mod memory;
mod memory_io;
mod native_agent;
mod schema_work;
pub use mcp::inspect_mcp_server;
mod model_calls;
mod provider;
pub use model_calls::ModelCalls;
mod progress;
pub use progress::{ChatEvents, ChatProgress, ChatProgressEvent};

mod semantic;
mod session;
pub use semantic::SemanticMemory;
mod skills;
mod tools;
mod workspace;

pub use files::{
    clamp_glob_max_results, clamp_grep_max_matches, clamp_list_dir_max_entries,
    delete_workspace_regular_file, glob_matches, glob_workspace, grep_workspace,
    list_workspace_dir, mkdir_workspace, move_workspace, parse_delete_file_args, parse_glob_args,
    parse_grep_args, parse_list_dir_args, parse_mkdir_args, parse_move_args, parse_read_file_args,
    parse_str_replace_args, parse_write_file_args, read_workspace_file, str_replace_workspace_file,
    write_workspace_regular_file, DeleteFileArgs, DeleteFileOutput, DirEntryInfo, GlobArgs,
    GlobOutput, GrepArgs, GrepMatch, GrepOutput, ListDirArgs, ListDirOutput, MkdirArgs,
    MkdirOutput, MoveArgs, MoveKind, MoveOutput, ReadFileArgs, ReadFileOutput, StrReplaceArgs,
    StrReplaceOutput, WorkspaceDeleteFileTool, WorkspaceGlobTool, WorkspaceGrepTool,
    WorkspaceListDirTool, WorkspaceMkdirTool, WorkspaceMoveTool, WorkspaceReadFileTool,
    WorkspaceStrReplaceTool, WorkspaceWriteFileTool, WriteFileArgs, WriteFileMode, WriteFileOutput,
    GLOB_DEFAULT_MAX_RESULTS, GLOB_MAX_RESULTS, GREP_DEFAULT_MAX_MATCHES, GREP_FILE_MAX_BYTES,
    GREP_MAX_MATCHES, LIST_DIR_DEFAULT_MAX_ENTRIES, LIST_DIR_MAX_ENTRIES, READ_FILE_MAX_BYTES,
    STR_REPLACE_MAX_BYTES, WRITE_FILE_MAX_BYTES,
};
pub use heartbeat::{inspect_heartbeat_file, load_heartbeat_message, resolve_heartbeat_path};
pub use identity::{
    inspect_identity_file, load_identity_for_prompt, resolve_identity_path, write_identity,
    IdentityKind, IdentityWriteTool,
};
pub use memory::{
    clamp_memory_search_max_results, inspect_memory_file, inspect_workspace_file,
    load_memory_for_prompt, load_prompt_file, parse_memory_search_args, parse_memory_write_args,
    resolve_memory_path, resolve_workspace_relative_path, search_memory_windows, write_memory,
    write_memory_with_limit, write_workspace_file, write_workspace_file_with_limit,
    MemoryAppendTool, MemoryFileStatus, MemorySearchHit, MemorySearchTool, MemoryWriteMode,
    MemoryWriteTool, MEMORY_SEARCH_DEFAULT_MAX_RESULTS, MEMORY_SEARCH_FILE_MAX_BYTES,
    MEMORY_SEARCH_LINE_RADIUS, MEMORY_SEARCH_MAX_RESULTS, MEMORY_WRITE_MAX_BYTES,
};
use provider::{BrokerrouterProvider, OpenAICompatibleProvider};
pub use session::{
    compact_imported_session_messages, compact_session_history, compact_session_history_default,
    format_messages_for_summary, hard_truncate_session_messages, local_conversation_digest,
    ConversationSummarizer, SESSION_SUMMARY_MAX_TOKENS, SESSION_SUMMARY_PREFIX,
    SESSION_SUMMARY_PROMPT, SESSION_SUMMARY_TEMPERATURE,
};
pub use skills::{Skill, SkillDiscovery, SkillRegistry};
pub use tools::{
    clamp_web_fetch_max_chars, clamp_web_search_max_results, html_to_readable_text,
    parse_web_fetch_args, parse_web_search_args, validate_web_fetch_url, DateTimeTool,
    FileCopyTool, FileDeleteTool, FileListTool, FileReadTool, FileWriteTool, HttpGetTool,
    JsonQueryTool, MemoryReadTool, ShellExecTool, Tool, ToolRegistry, WebFetchTool, WebSearchTool,
    WorkspaceListTool, DEFAULT_BRAVE_SEARCH_ENDPOINT, WEB_FETCH_DEFAULT_MAX_CHARS,
    WEB_FETCH_HTTP_TIMEOUT_SECS, WEB_FETCH_MAX_CHARS, WEB_FETCH_MAX_REDIRECTS, WEB_FETCH_MIN_CHARS,
    WEB_SEARCH_DEFAULT_MAX_RESULTS, WEB_SEARCH_HTTP_TIMEOUT_SECS, WEB_SEARCH_MAX_RESULTS,
};
pub use workspace::Workspace;

// StateKnot runtime is not linked; see docs/stateknot-gaps.md.
// use stateknot_core::{AgentExecutionConfig, AgentInstructions, BudgetLimits};
// use stateknot_runtime::AgentBuilder;

/// 工具执行计数钩子（`true` = ok，`false` = error）。
pub type ToolMetricsHook = std::sync::Arc<dyn Fn(&str, bool) + Send + Sync>;

/// 按配置注册可选工作区文件工具（`read_file` / `list_dir` / `write_file` / `delete_file` / `str_replace` / `grep` / `glob` / `mkdir` / `move` / `stat` / `tree`）。
fn register_optional_workspace_file_tools(tools: &mut ToolRegistry, config: &AgentConfig) {
    let workspace = &config.workspace_path;
    if config.tools.read_file.enabled {
        tools.register(Box::new(WorkspaceReadFileTool::new(workspace)));
        tools.register(Box::new(FileReadTool::new(workspace)));
    }
    if config.tools.list_dir.enabled {
        tools.register(Box::new(WorkspaceListDirTool::new(workspace)));
        tools.register(Box::new(FileListTool::new(workspace)));
    }
    if config.tools.stat.enabled {
        tools.register(Box::new(WorkspaceStatTool::new(workspace)));
    }
    if config.tools.tree.enabled {
        tools.register(Box::new(WorkspaceTreeTool::new(workspace)));
    }
    if config.tools.write_file.enabled {
        tools.register(Box::new(WorkspaceWriteFileTool::new(workspace)));
        tools.register(Box::new(FileWriteTool::new(workspace)));
    }
    if config.tools.delete_file.enabled {
        tools.register(Box::new(WorkspaceDeleteFileTool::new(workspace)));
        tools.register(Box::new(FileDeleteTool::new(workspace)));
    }
    if config.tools.str_replace.enabled {
        tools.register(Box::new(WorkspaceStrReplaceTool::new(workspace)));
    }
    if config.tools.grep.enabled {
        tools.register(Box::new(WorkspaceGrepTool::new(workspace)));
    }
    if config.tools.glob.enabled {
        tools.register(Box::new(WorkspaceGlobTool::new(workspace)));
    }
    if config.tools.mkdir.enabled {
        tools.register(Box::new(WorkspaceMkdirTool::new(workspace)));
    }
    if config.tools.r#move.enabled {
        tools.register(Box::new(WorkspaceMoveTool::new(workspace)));
    }
    if config.tools.copy.enabled {
        tools.register(Box::new(WorkspaceCopyTool::new(workspace)));
        tools.register(Box::new(WorkspaceCopyTool::legacy(workspace)));
    }
}

/// `JiaClaw` Agent 包装器
///
/// 使用 `StateKnot` HTTP MCP 协议适配器；durable runtime 尚待独立接入和认证。
pub struct JiaClawAgent {
    config: AgentConfig,
    workspace: Workspace,
    skills: SkillRegistry,
    tools: ToolRegistry,
    /// 可选：每次真实 tool 执行后回调（serve 的 Prometheus 计数）。
    tool_metrics: Option<ToolMetricsHook>,
    model_calls: Option<std::sync::Arc<ModelCalls>>,
    /// 测试专用：stub 每次都请求该工具，用于验证 tool loop 上限。
    #[cfg(test)]
    test_repeat_tool: Option<String>,
    // TODO: 当 StateKnot 发布稳定 API 后，添加 TypedAgent 字段
    // typed_agent: TypedAgent<ChatRequest, ChatResponse>,
}

impl JiaClawAgent {
    /// 创建新的 `JiaClaw` Agent 实例
    ///
    /// # Errors
    ///
    /// 工作区加载失败或沙箱策略无效时返回错误。
    ///
    /// # 当前限制
    ///
    /// 当前对话循环不使用 `StateKnot` 的持久化运行时。
    /// 已发布的 alpha 版本仍待生产认证；会话、后台任务和专用操作账本的
    /// 持久化不等价于通用工具循环恢复。
    ///
    /// 参见 `docs/stateknot-gaps.md` 了解详情。
    pub fn new(config: AgentConfig) -> Result<Self, JiaClawError> {
        if !config.mcp.servers.is_empty()
            || config.memory.semantic.enabled
            || config.model_calls.enabled
        {
            return Err(JiaClawError::Configuration(
                "MCP/semantic memory/model calls requires JiaClawAgent::connect(...).await; synchronous construction does not omit configured tools".into(),
            ));
        }
        Self::new_local(config)
    }

    /// Construct an agent, validate reviewed MCP bindings, and open configured
    /// semantic storage before use. No embedding requests occur at startup.
    ///
    /// # Errors
    /// Local initialization, private storage, policy, discovery, pin or schema validation failures.
    pub async fn connect(config: AgentConfig) -> Result<Self, JiaClawError> {
        let model_calls = ModelCalls::open(&config).await?;
        let semantic = SemanticMemory::open(&config).await?;
        let mut agent = Self::new_local(config)?;
        agent.model_calls = model_calls;
        mcp::initialize(&agent.config.mcp, &mut agent.tools).await?;
        if let Some(semantic) = semantic {
            agent.tools.register(Box::new(
                MemorySearchTool::new(
                    &agent.config.workspace_path,
                    [
                        agent.config.memory.path.clone(),
                        agent.config.identity.soul_path.clone(),
                        agent.config.identity.user_path.clone(),
                    ],
                )
                .with_semantic(semantic),
            ));
        }
        Ok(agent)
    }

    fn new_local(config: AgentConfig) -> Result<Self, JiaClawError> {
        config.routing.validate(&config.provider)?;
        config.model_calls.validate(&config.provider)?;
        // 加载工作空间文件
        let workspace = Workspace::load(&config.workspace_path)?;

        // 发现技能
        let skill_discovery = SkillDiscovery::new(&config.workspace_path);
        let skills = skill_discovery.discover().unwrap_or_else(|e| {
            tracing::warn!("技能发现失败: {e}");
            Vec::new()
        });

        if !skills.is_empty() {
            tracing::info!("发现 {} 个技能", skills.len());
        }
        let skills = SkillRegistry::new(skills);

        // 初始化工具注册表
        let mut tools = ToolRegistry::new();

        // 工作空间和记忆工具
        tools.register(Box::new(WorkspaceListTool::new(&config.workspace_path)));
        tools.register(Box::new(MemoryReadTool::with_paths(
            &config.workspace_path,
            config.memory.path.clone(),
            config.identity.soul_path.clone(),
            config.identity.user_path.clone(),
        )));
        if config.tools.memory_search.enabled {
            tools.register(Box::new(MemorySearchTool::new(
                &config.workspace_path,
                [
                    config.memory.path.clone(),
                    config.identity.soul_path.clone(),
                    config.identity.user_path.clone(),
                ],
            )));
        }
        if config.tools.memory_write.enabled {
            tools.register(Box::new(MemoryAppendTool::new(
                &config.workspace_path,
                config.memory.path.clone(),
            )));
            tools.register(Box::new(MemoryWriteTool::new(
                &config.workspace_path,
                config.memory.path.clone(),
            )));
        }
        register_optional_workspace_file_tools(&mut tools, &config);
        tools.register(Box::new(IdentityWriteTool::soul(
            &config.workspace_path,
            config.identity.soul_path.clone(),
        )));
        tools.register(Box::new(IdentityWriteTool::user(
            &config.workspace_path,
            config.identity.user_path.clone(),
        )));

        // 网络和数据工具
        tools.register(Box::new(HttpGetTool::new()));
        tools.register(Box::new(JsonQueryTool::new()));
        if config.tools.web_search.enabled {
            tools.register(Box::new(WebSearchTool::new(
                config.tools.web_search.effective_brave_api_key(),
            )));
        }
        if config.tools.web_fetch.enabled {
            tools.register(Box::new(WebFetchTool::new(
                config.tools.web_fetch.allow_private,
            )));
        }

        // 系统工具
        tools.register(Box::new(DateTimeTool::new()));
        if config.tools.exec.enabled {
            tools.register(Box::new(ControlledExecTool::new(
                &config.workspace_path,
                config.tools.exec.clone(),
            )?));
            tools.register(Box::new(ControlledExecTool::legacy(
                &config.workspace_path,
                config.tools.exec.clone(),
            )?));
        }

        tracing::info!("注册了 {} 个本地工具", tools.list().len());

        Ok(Self {
            config,
            workspace,
            skills,
            tools,
            tool_metrics: None,
            model_calls: None,
            #[cfg(test)]
            test_repeat_tool: None,
        })
    }

    /// 挂接工具执行计数钩子（HTTP `/metrics` 使用）。
    #[must_use]
    pub fn with_tool_metrics_hook(mut self, hook: ToolMetricsHook) -> Self {
        self.tool_metrics = Some(hook);
        self
    }

    fn observe_tool_call(&self, tool_name: &str, ok: bool) {
        if let Some(hook) = &self.tool_metrics {
            hook(tool_name, ok);
        }
    }

    /// 获取 Agent 配置
    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// 获取工作空间
    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// 获取技能列表快照（不持锁）。
    #[must_use]
    pub fn skills(&self) -> Vec<Skill> {
        self.skills.snapshot()
    }

    /// 重新扫描工作区 `skills/` 并替换进程内注册表。
    ///
    /// 扫描在锁外完成；失败时保留旧表。进行中的 `chat` 使用各自开始时的快照，不会 panic。
    ///
    /// # Errors
    ///
    /// 目录无法读取，或任一现存 `SKILL.md` 无效。此时旧表保持不变。
    pub fn reload_skills(&self) -> Result<Vec<Skill>, JiaClawError> {
        match self.skills.reload(&self.config.workspace_path) {
            Ok(skills) => {
                tracing::info!(count = skills.len(), "技能已重载");
                Ok(skills)
            }
            Err(e) => {
                tracing::error!(error = %e, "技能重载失败，已保留旧表");
                Err(e)
            }
        }
    }

    /// 获取工具注册表
    #[must_use]
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    #[cfg(test)]
    fn register_tool_for_test(&mut self, tool: Box<dyn Tool>) {
        self.tools.register(tool);
    }

    #[cfg(test)]
    fn force_repeat_tool_for_test(&mut self, tool_name: &str) {
        self.test_repeat_tool = Some(tool_name.to_string());
    }

    /// 处理聊天请求
    ///
    /// # Errors
    ///
    /// 可能返回以下错误：
    /// - 请求验证失败
    /// - 模型调用失败
    /// - 网络错误
    ///
    /// # 实现说明
    ///
    /// 根据配置选择提供商：
    /// 1. `brokerrouter` - 推荐的生产路径（通过 Brokerrouter Gateway）
    /// 2. `openai_compatible` - 已废弃的直连模式（仅作开发逃生舱）
    /// 3. `stub` - 显式选择的离线存根模式
    ///
    /// 本方法会自动处理工具调用循环（上限见 [`AgentConfig::effective_max_tool_iterations`]）。
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, JiaClawError> {
        self.chat_for(request, ModelPurpose::Chat).await
    }

    /// Execute a turn using an administrator-configured model for its trusted origin.
    ///
    /// The host assigns the purpose; clients, prompts and tools cannot override it.
    /// The selected logical model and limits stay fixed for the complete tool loop.
    /// Endpoint fallback remains owned by Brokerrouter; no model call is replayed here.
    ///
    /// # Errors
    /// Invalid policy, unsupported purpose, authorization or provider failure.
    pub async fn chat_for(
        &self,
        request: &ChatRequest,
        purpose: ModelPurpose,
    ) -> Result<ChatResponse, JiaClawError> {
        self.chat_for_inner(request, purpose, None).await
    }

    /// Execute a tracked Brokerrouter turn with bounded provisional text delivery.
    /// Every complete tool batch still passes the same authorization/schema gate.
    /// Delivery cancellation stops further dispatch; submitted model receipts settle separately.
    ///
    /// # Errors
    /// Explicit Brokerrouter/model_calls configuration, authorization, transport or delivery failure.
    pub async fn chat_stream_for(
        &self,
        request: &ChatRequest,
        purpose: ModelPurpose,
        progress: &ChatProgress,
    ) -> Result<ChatResponse, JiaClawError> {
        if self.config.provider.provider_type != "brokerrouter" || self.model_calls.is_none() {
            return Err(JiaClawError::Configuration(
                "streaming requires Brokerrouter and an explicitly enabled model_calls ledger"
                    .into(),
            ));
        }
        if !self.tools.list().is_empty()
            && !self
                .config
                .effective_tool_timeout_secs()
                .is_some_and(|secs| (1..=30).contains(&secs))
        {
            return Err(JiaClawError::Configuration(
                "streaming with tools requires effective tool_timeout_secs in 1..=30".into(),
            ));
        }
        progress.ensure_open()?;
        self.chat_for_inner(request, purpose, Some(progress)).await
    }

    async fn chat_for_inner(
        &self,
        request: &ChatRequest,
        purpose: ModelPurpose,
        progress: Option<&ChatProgress>,
    ) -> Result<ChatResponse, JiaClawError> {
        if purpose == ModelPurpose::Summary {
            return Err(JiaClawError::InvalidRequest(
                "summary purpose requires the no-tools summarization path".into(),
            ));
        }
        let allowed_tools = self.allowed_tool_names(request)?;
        let env_key = std::env::var("JIACLAW_API_KEY").ok();
        let api_key = Self::resolve_provider_api_key(&self.config.provider, env_key.as_deref())?;
        let selection = self.config.routing.select(&self.config.provider, purpose);
        let routing_enabled = !self.config.routing.is_empty();
        if routing_enabled {
            tracing::info!(purpose = ?selection.purpose, requested_model = %selection.model,
                max_tokens = selection.max_tokens, "selected model policy for turn");
        }
        // 短锁快照：reload 期间进行中的 chat 继续使用本轮技能表，不持锁。
        let skills = self.skills.snapshot();
        // 检查是否有技能应该被自动触发（仅在 auto_skills 为 true 时）
        let mut enabled_skills = request.enabled_skills.clone();

        if request.auto_skills {
            if let Some(last_user_msg) = request
                .messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, MessageRole::User))
            {
                let discovery = SkillDiscovery::new(&self.config.workspace_path);
                let auto_triggered = discovery.auto_trigger_skills(&last_user_msg.content, &skills);

                for skill_name in auto_triggered {
                    if !enabled_skills.contains(&skill_name) {
                        tracing::info!("自动激活技能: {}", skill_name);
                        enabled_skills.push(skill_name);
                    }
                }
            }
        } else {
            tracing::info!("技能自动激活已禁用");
        }

        let request_with_skills = ChatRequest {
            messages: request.messages.clone(),
            enabled_tools: request.enabled_tools.clone(),
            enabled_skills,
            auto_skills: request.auto_skills,
            session_id: request.session_id.clone(),
        };

        // 构建完整的系统提示（包含工作空间内容）
        let system_prompt = self.build_system_prompt_with_skills(&request_with_skills, &skills);

        // 根据提供商类型执行工具循环
        let provider_type = self.config.provider.provider_type.as_str();
        let mut response = match (api_key, provider_type) {
            (Some(key), "brokerrouter") => {
                self.execute_brokerrouter_loop(
                    &request_with_skills,
                    &system_prompt,
                    &key,
                    &selection,
                    progress,
                )
                .await
            }
            (Some(key), "openai_compatible") => {
                // 使用工具执行循环
                self.execute_tool_loop(
                    request_with_skills.messages.clone(),
                    &system_prompt,
                    provider_type,
                    Some(&key),
                    &allowed_tools,
                )
                .await
            }
            (None, "stub") => {
                self.execute_tool_loop(
                    request_with_skills.messages.clone(),
                    &system_prompt,
                    "stub",
                    None,
                    &allowed_tools,
                )
                .await
            }
            _ => Err(JiaClawError::Configuration(format!(
                "无效的模型提供商配置: {provider_type}"
            ))),
        }?;
        if routing_enabled {
            response.routing = Some(selection);
        }
        Ok(response)
    }

    /// Wait for the current bounded model receipt worker during graceful caller shutdown.
    /// This sends no requests and does not resume tools or a turn.
    ///
    /// # Errors
    /// The supplied grace expires; pending state must be inspected after restart.
    pub async fn settle_model_calls(&self, grace: std::time::Duration) -> Result<(), JiaClawError> {
        if let Some(ledger) = &self.model_calls {
            ledger.settle(grace).await?;
        }
        Ok(())
    }

    /// A request may narrow the configured registry, never expand it. The empty
    /// list retains the API's existing meaning: all registered tools are allowed.
    fn allowed_tool_names(
        &self,
        request: &ChatRequest,
    ) -> Result<std::collections::HashSet<String>, JiaClawError> {
        if request.enabled_tools.is_empty() {
            return Ok(self.tools.list().into_iter().map(str::to_owned).collect());
        }
        for name in &request.enabled_tools {
            if self.tools.get(name).is_none() {
                return Err(JiaClawError::InvalidRequest(format!(
                    "请求启用了未注册的工具: {name}"
                )));
            }
        }
        Ok(request.enabled_tools.iter().cloned().collect())
    }

    fn resolve_provider_api_key(
        provider: &ProviderConfig,
        env_key: Option<&str>,
    ) -> Result<Option<String>, JiaClawError> {
        match provider.provider_type.as_str() {
            "stub" => Ok(None),
            "brokerrouter" | "openai_compatible" => {
                let key = env_key.or(provider.api_key.as_deref());
                match key {
                    Some(key) if !key.trim().is_empty() => Ok(Some(key.to_owned())),
                    _ => Err(JiaClawError::Configuration(
                        "模型提供商缺少 API Key；离线模式需显式配置 provider_type='stub'".into(),
                    )),
                }
            }
            other => Err(JiaClawError::Configuration(format!(
                "未知的提供商类型: {other}"
            ))),
        }
    }

    /// Compact history; an uncertain paid summary must preserve the caller's session.
    /// # Errors
    /// An unresolved model call blocks compaction instead of silently truncating history.
    pub async fn compact_session_messages(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<Vec<ChatMessage>, JiaClawError> {
        if let Some(ledger) = &self.model_calls {
            ledger.ensure_clear().await?;
        }
        let compacted = compact_session_history(
            messages,
            MAX_SESSION_MESSAGES,
            self.config.session.effective_summarize_on_overflow(),
            self.config.session.effective_keep_recent(),
            self,
        )
        .await;
        if let Some(ledger) = &self.model_calls {
            ledger.ensure_clear().await?;
        }
        Ok(compacted)
    }

    /// 使用当前 LLM provider 生成会话摘要（无工具、限制 `max_tokens`）。
    ///
    /// 显式 stub 模式返回确定性本地摘要，不走 tool loop。
    ///
    /// # Errors
    ///
    /// 提供商调用失败或返回空文本时返回错误，由调用方回退硬截断。
    pub async fn summarize_conversation(
        &self,
        messages: &[ChatMessage],
    ) -> Result<String, JiaClawError> {
        self.summarize_messages_for_session(messages).await
    }

    async fn summarize_messages_for_session(
        &self,
        messages: &[ChatMessage],
    ) -> Result<String, JiaClawError> {
        if messages.is_empty() {
            return Err(JiaClawError::InvalidRequest("没有可摘要的消息".to_string()));
        }

        let transcript = format_messages_for_summary(messages);
        let env_key = std::env::var("JIACLAW_API_KEY").ok();
        let api_key = Self::resolve_provider_api_key(&self.config.provider, env_key.as_deref())?;

        let Some(key) = api_key else {
            return Ok(local_conversation_digest(messages));
        };

        let prompt_messages = [ChatMessage {
            role: MessageRole::User,
            content: transcript,
        }];

        let selection = self
            .config
            .routing
            .select(&self.config.provider, ModelPurpose::Summary);
        if !self.config.routing.is_empty() {
            tracing::info!(purpose = ?selection.purpose, requested_model = %selection.model,
                max_tokens = selection.max_tokens, "selected model policy for summary");
        }
        let response = self
            .complete_without_tools(
                SESSION_SUMMARY_PROMPT,
                &prompt_messages,
                self.config.provider.provider_type.as_str(),
                &key,
                &selection,
            )
            .await?;

        let text = response.message.content.trim();
        if text.is_empty() {
            return Err(JiaClawError::InvalidRequest("摘要为空".to_string()));
        }
        Ok(text.to_string())
    }

    async fn brokerrouter_text(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        key: &str,
        selection: &ModelSelection,
    ) -> Result<ChatResponse, JiaClawError> {
        if let Some(ledger) = &self.model_calls {
            let wire = provider::brokerrouter::WireMessage::history(system_prompt, messages);
            let prepared = ledger.prepare(
                &selection.model,
                &wire,
                selection.temperature,
                selection.max_tokens,
                &[],
            )?;
            let message = ledger
                .complete(
                    prepared,
                    uuid::Uuid::new_v4().to_string(),
                    selection.purpose,
                    None,
                    0,
                )
                .await?;
            Ok(ChatResponse {
                message: ChatMessage {
                    role: MessageRole::Assistant,
                    content: message.content.unwrap_or_default(),
                },
                tool_calls: vec![],
                status: RunStatus::Completed,
                routing: None,
                session_id: None,
            })
        } else {
            BrokerrouterProvider::new(&self.config.provider.base_url, key)
                .chat(
                    &selection.model,
                    system_prompt,
                    messages,
                    selection.temperature,
                    selection.max_tokens,
                )
                .await
        }
    }

    /// 单次补全，不进入 tool loop（供摘要压缩使用，避免递归工具/心跳爆炸）。
    async fn complete_without_tools(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        provider_type: &str,
        api_key: &str,
        selection: &ModelSelection,
    ) -> Result<ChatResponse, JiaClawError> {
        match provider_type {
            "brokerrouter" => {
                self.brokerrouter_text(system_prompt, messages, api_key, selection)
                    .await
            }
            "openai_compatible" => {
                let provider =
                    OpenAICompatibleProvider::new(&self.config.provider.base_url, api_key);
                provider
                    .chat(
                        &selection.model,
                        system_prompt,
                        messages,
                        selection.temperature,
                        selection.max_tokens,
                    )
                    .await
            }
            other => Err(JiaClawError::Configuration(format!(
                "未知的提供商类型: {other}"
            ))),
        }
    }

    /// 构建系统提示（包含工作空间内容；每次调用重读 SOUL / USER / MEMORY）
    #[cfg(test)]
    fn build_system_prompt(&self, request: &ChatRequest) -> String {
        let skills = self.skills.snapshot();
        self.build_system_prompt_with_skills(request, &skills)
    }

    fn build_system_prompt_with_skills(&self, request: &ChatRequest, skills: &[Skill]) -> String {
        let mut prompt = self.config.system_instructions.clone();

        // 每次对话开始时重读约定身份与记忆路径（工具写入对后续 chat 可见）
        Self::inject_prompt_section(
            &mut prompt,
            IdentityKind::Soul.prompt_heading(),
            load_identity_for_prompt(
                &self.config.workspace_path,
                &self.config.identity.soul_path,
                IdentityKind::Soul,
            ),
            "人格",
        );
        Self::inject_prompt_section(
            &mut prompt,
            IdentityKind::User.prompt_heading(),
            load_identity_for_prompt(
                &self.config.workspace_path,
                &self.config.identity.user_path,
                IdentityKind::User,
            ),
            "用户画像",
        );
        Self::inject_prompt_section(
            &mut prompt,
            "Long-term Memory（长期记忆）",
            load_memory_for_prompt(&self.config.workspace_path, &self.config.memory.path),
            "长期记忆",
        );

        // 添加技能摘要
        if !skills.is_empty() {
            prompt.push_str("\n\n## Available Skills\n\n");
            for skill in skills {
                prompt.push_str(&format!("- {}\n", skill.summary()));
            }

            // 如果用户请求了特定技能，添加详细信息
            if !request.enabled_skills.is_empty() {
                prompt.push_str("\n### Enabled Skills (详细)\n");
                for skill_name in &request.enabled_skills {
                    if let Some(skill) = skills.iter().find(|s| &s.name == skill_name) {
                        prompt.push_str(&format!("\n#### {}\n", skill.name));
                        prompt.push_str(&skill.content);
                        prompt.push('\n');
                    }
                }
            }
        }

        // 添加可用工具列表
        let mut tool_list = self.tools.list();
        tool_list.retain(|name| {
            request.enabled_tools.is_empty()
                || request.enabled_tools.iter().any(|allowed| allowed == name)
        });
        tool_list.sort_unstable();
        if !tool_list.is_empty() {
            prompt.push_str("\n\n## Available Tools\n\n");
            prompt.push_str("你可以使用以下工具来完成任务。每个工具的详细信息如下：\n\n");
            for tool_name in tool_list {
                if let Some(tool) = self.tools.get(tool_name) {
                    prompt.push_str(&format!(
                        "### {}\n{}\n\n参数 schema:\n```json\n{}\n```\n\n",
                        tool.name(),
                        tool.description(),
                        serde_json::to_string_pretty(&tool.parameters_schema()).unwrap_or_default()
                    ));
                }
            }
            if self.config.provider.provider_type != "brokerrouter" {
                prompt.push_str(
                    "## Tool Calling Format\n\n\
                 要调用工具，请在你的响应中使用以下 JSON 代码块格式：\n\n\
                 ```tool\n\
                 {\n\
                   \"tool_name\": \"工具名称\",\n\
                   \"arguments\": {\"参数名\": \"参数值\"}\n\
                 }\n\
                 ```\n\n\
                 你可以在一条消息中调用多个工具，每个工具调用使用一个单独的 ```tool 代码块。\n\
                 我会执行这些工具并将结果返回给你，然后你可以继续处理。\n\n",
                );
            }
        }

        prompt
    }

    fn inject_prompt_section(
        prompt: &mut String,
        heading: &str,
        loaded: Result<Option<String>, JiaClawError>,
        warn_label: &str,
    ) {
        match loaded {
            Ok(Some(content)) => {
                prompt.push_str("\n\n## ");
                prompt.push_str(heading);
                prompt.push('\n');
                prompt.push_str(&content);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("读取{warn_label}失败，跳过注入: {e}");
            }
        }
    }

    /// 执行一次工具调用，并把成功/失败结果写入 `ToolCall.result`。
    ///
    /// 超时走现有错误路径：写入错误结果并返回，不 panic。
    async fn execute_and_record(&self, mut tool_call: ToolCall) -> (ToolCall, String) {
        match self
            .tools
            .execute_with_timeout(&tool_call, self.config.effective_tool_timeout_secs())
            .await
        {
            Ok(result) => {
                tool_call.result = Some(serde_json::json!(result.clone()));
                self.observe_tool_call(&tool_call.tool_name, true);
                let message = format!("工具 {} 执行成功:\n{}", tool_call.tool_name, result);
                (tool_call, message)
            }
            Err(e) => {
                let error_msg = format!("工具 {} 执行失败: {}", tool_call.tool_name, e);
                tool_call.result = Some(serde_json::json!({"error": error_msg.clone()}));
                self.observe_tool_call(&tool_call.tool_name, false);
                (tool_call, error_msg)
            }
        }
    }

    /// 解析 assistant 消息中的工具调用
    fn parse_tool_calls(content: &str) -> Vec<ToolCall> {
        let mut tool_calls = Vec::new();

        // 查找所有 ```tool ... ``` 代码块
        let mut start_idx = 0;
        while let Some(block_start) = content[start_idx..].find("```tool") {
            let block_start = start_idx + block_start;
            if let Some(block_end) = content[block_start + 7..].find("```") {
                let block_end = block_start + 7 + block_end;
                let json_str = content[block_start + 7..block_end].trim();

                // 尝试解析 JSON
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if let (Some(tool_name), Some(arguments)) = (
                        parsed.get("tool_name").and_then(|v| v.as_str()),
                        parsed.get("arguments"),
                    ) {
                        tool_calls.push(ToolCall {
                            tool_name: tool_name.to_string(),
                            arguments: arguments.clone(),
                            result: None,
                        });
                    }
                }

                start_idx = block_end + 3;
            } else {
                break;
            }
        }

        tool_calls
    }

    /// 达到工具循环上限时写入 assistant / 未执行 tool 的提示。
    fn max_tool_iterations_stop_hint(limit: usize) -> String {
        format!("已达到最大工具迭代次数（{limit}）。本轮停止，未执行剩余工具调用。")
    }

    /// 单次 LLM / stub 补全（不进入 tool loop）。
    async fn complete_turn(
        &self,
        messages: &[ChatMessage],
        system_prompt: &str,
        provider_type: &str,
        api_key: Option<&str>,
    ) -> Result<ChatResponse, JiaClawError> {
        if let Some(key) = api_key {
            match provider_type {
                "brokerrouter" => {
                    let selection = self
                        .config
                        .routing
                        .select(&self.config.provider, ModelPurpose::Chat);
                    self.brokerrouter_text(system_prompt, messages, key, &selection)
                        .await
                }
                "openai_compatible" => {
                    let provider =
                        OpenAICompatibleProvider::new(&self.config.provider.base_url, key);
                    provider
                        .chat(
                            &self.config.provider.model,
                            system_prompt,
                            messages,
                            self.config.provider.temperature,
                            self.config.provider.max_tokens,
                        )
                        .await
                }
                _ => Err(JiaClawError::Configuration(format!(
                    "未知的提供商类型: {provider_type}"
                ))),
            }
        } else if provider_type == "stub" {
            let request = ChatRequest {
                messages: messages.to_vec(),
                enabled_tools: vec![],
                enabled_skills: vec![],
                auto_skills: true,
                session_id: None,
            };
            Ok(self.stub_chat(&request, system_prompt))
        } else {
            Err(JiaClawError::Configuration(
                "模型提供商缺少 API Key；离线模式需显式配置 provider_type='stub'".into(),
            ))
        }
    }

    fn finish_at_tool_iteration_limit(
        response: ChatResponse,
        pending_tool_calls: Vec<ToolCall>,
        mut all_tool_calls: Vec<ToolCall>,
        max_iterations: usize,
    ) -> ChatResponse {
        let hint = Self::max_tool_iterations_stop_hint(max_iterations);
        tracing::warn!("{hint}");
        for mut tool_call in pending_tool_calls {
            tool_call.result = Some(serde_json::json!({ "error": hint.as_str() }));
            all_tool_calls.push(tool_call);
        }
        let mut message = response.message;
        message.content = format!("{}\n\n{hint}", message.content.trim_end());
        ChatResponse {
            message,
            tool_calls: all_tool_calls,
            status: response.status,
            session_id: None,
            routing: response.routing,
        }
    }

    /// 执行工具调用循环
    async fn execute_tool_loop(
        &self,
        mut messages: Vec<ChatMessage>,
        system_prompt: &str,
        provider_type: &str,
        api_key: Option<&str>,
        allowed_tools: &std::collections::HashSet<String>,
    ) -> Result<ChatResponse, JiaClawError> {
        let max_iterations = self.config.effective_max_tool_iterations();
        let mut iteration = 0;
        let mut all_tool_calls = Vec::new();

        loop {
            iteration += 1;

            let response = self
                .complete_turn(&messages, system_prompt, provider_type, api_key)
                .await?;
            let tool_calls = Self::parse_tool_calls(&response.message.content);

            if tool_calls.is_empty() {
                return Ok(ChatResponse {
                    message: response.message,
                    tool_calls: all_tool_calls,
                    status: response.status,
                    session_id: None,
                    routing: response.routing,
                });
            }

            // Validate the whole model response before executing any side effect.
            // Tool descriptions and prompts alone are not an authorization boundary.
            if let Some(denied) = tool_calls
                .iter()
                .find(|call| !allowed_tools.contains(&call.tool_name))
            {
                return Err(JiaClawError::InvalidRequest(format!(
                    "模型请求了本轮未授权的工具: {}",
                    denied.tool_name
                )));
            }

            if iteration >= max_iterations {
                return Ok(Self::finish_at_tool_iteration_limit(
                    response,
                    tool_calls,
                    all_tool_calls,
                    max_iterations,
                ));
            }

            let mut executed_tool_calls = Vec::new();
            let mut tool_results = Vec::new();

            for tool_call in tool_calls {
                tracing::info!(
                    "执行工具: {} (迭代 {}/{})",
                    tool_call.tool_name,
                    iteration,
                    max_iterations
                );

                let (recorded, message) = self.execute_and_record(tool_call).await;
                executed_tool_calls.push(recorded);
                tool_results.push(message);
            }

            all_tool_calls.extend(executed_tool_calls);
            messages.push(response.message.clone());
            messages.push(ChatMessage {
                role: MessageRole::User,
                content: format!(
                    "工具执行结果 (迭代 {}/{}):\n\n{}",
                    iteration,
                    max_iterations,
                    tool_results.join("\n\n")
                ),
            });
        }
    }

    /// 显式选择 stub 提供商时的离线实现。
    fn stub_chat(&self, request: &ChatRequest, system_prompt: &str) -> ChatResponse {
        // 获取最后一条用户消息
        let last_user_message = request
            .messages
            .iter()
            .rev()
            .find(|msg| matches!(msg.role, MessageRole::User))
            .map_or("", |msg| msg.content.as_str());

        // 简单的响应生成逻辑（演示用）
        let response_content =
            self.generate_stub_response(last_user_message, request, system_prompt);

        ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: response_content,
            },
            tool_calls: vec![],
            status: RunStatus::Completed,
            session_id: None,
            routing: None,
        }
    }

    /// 生成存根响应（演示用）
    #[allow(clippy::too_many_lines)]
    fn generate_stub_response(
        &self,
        user_message: &str,
        request: &ChatRequest,
        _system_prompt: &str,
    ) -> String {
        let user_lower = user_message.to_lowercase();
        let skills = self.skills.snapshot();

        #[cfg(test)]
        if let Some(tool_name) = &self.test_repeat_tool {
            return format!(
                "继续调用工具。\n\n\
                 ```tool\n\
                 {{\n\
                   \"tool_name\": \"{tool_name}\",\n\
                   \"arguments\": {{}}\n\
                 }}\n\
                 ```"
            );
        }

        // 检查是否是工具执行结果反馈
        if user_lower.contains("工具执行结果") || user_lower.contains("执行成功") {
            // 工具已经执行完成，返回一个总结
            return "根据工具执行结果，操作已完成。\n\n\
                 如需了解更多信息，请查看上面的工具输出结果。"
                .to_string();
        }

        // 检查是否应该触发工具调用（演示）
        if user_lower.contains("列出工作空间")
            || user_lower.contains("list workspace")
            || user_lower.contains("workspace files")
        {
            return "好的,让我列出工作空间文件。\n\n\
                 ```tool\n\
                 {\n\
                   \"tool_name\": \"workspace_list\",\n\
                   \"arguments\": {}\n\
                 }\n\
                 ```"
            .to_string();
        }

        if user_lower.contains("读取记忆")
            || user_lower.contains("read memory")
            || (user_lower.contains("memory") && user_lower.contains("read"))
        {
            return "让我读取记忆文件。\n\n\
                 ```tool\n\
                 {\n\
                   \"tool_name\": \"memory_read\",\n\
                   \"arguments\": {\"file\": \"MEMORY\"}\n\
                 }\n\
                 ```"
            .to_string();
        }

        if user_lower.contains("当前时间")
            || user_lower.contains("current time")
            || user_lower.contains("what time")
        {
            return "让我获取当前时间。\n\n\
                 ```tool\n\
                 {\n\
                   \"tool_name\": \"datetime_now\",\n\
                   \"arguments\": {}\n\
                 }\n\
                 ```"
            .to_string();
        }

        if (user_lower.contains("列出") || user_lower.contains("list"))
            && user_lower.contains("文件")
        {
            return "让我列出当前目录的文件。\n\n\
                 ```tool\n\
                 {\n\
                   \"tool_name\": \"file_list\",\n\
                   \"arguments\": {\"path\": \".\"}\n\
                 }\n\
                 ```"
            .to_string();
        }

        // 简单的关键词匹配响应
        if user_lower.contains("你好") || user_lower.contains("hello") || user_lower.contains("hi")
        {
            format!(
                "你好！我是 {}。{}\n\n\
                 我目前运行在存根模式下，等待 StateKnot 框架集成。\n\
                 当前配置的最大对话轮次为 {} 轮。\n\n\
                 💡 可用功能（演示）：\n\
                 • 工作空间已加载（{} 个文件）\n\
                 • 发现了 {} 个技能\n\
                 • 注册了 {} 个工具\n\n\
                 试试问我：\"列出工作空间\" 或 \"读取记忆\" 来测试工具执行！",
                self.config.name,
                self.config.description,
                self.config.max_turns,
                self.count_workspace_files(),
                skills.len(),
                self.tools.list().len()
            )
        } else if user_lower.contains("工具") || user_lower.contains("tool") {
            let tool_list = self.tools.list();
            if tool_list.is_empty() {
                "目前没有注册任何工具。".to_string()
            } else {
                let mut response = format!("🔧 已注册 {} 个本地工具：\n\n", tool_list.len());
                for tool_name in tool_list {
                    if let Some(tool) = self.tools.get(tool_name) {
                        response.push_str(&format!(
                            "• **{}**\n  {}\n\n",
                            tool.name(),
                            tool.description()
                        ));
                    }
                }
                response.push_str("💡 试试说：\n");
                response.push_str("• \"列出工作空间\" - 调用 workspace_list 工具\n");
                response.push_str("• \"读取记忆\" - 调用 memory_read 工具\n");
                response.push_str("• \"当前时间\" - 调用 datetime_now 工具\n");
                response
            }
        } else if user_lower.contains("技能") || user_lower.contains("skill") {
            if skills.is_empty() {
                "目前没有发现任何技能。\n\n运行 'jiaclaw init' 会创建示例技能。".to_string()
            } else {
                let mut response = format!("🎯 发现 {} 个技能：\n\n", skills.len());
                for skill in &skills {
                    response.push_str(&format!("• {}\n\n", skill.summary()));
                }
                response.push_str("完整的技能系统将在 M3 实现。");
                response
            }
        } else if user_lower.contains("功能")
            || user_lower.contains("能力")
            || user_lower.contains("what can you do")
        {
            let tools_str = if request.enabled_tools.is_empty() {
                format!("{} 个本地工具", self.tools.list().len())
            } else {
                request.enabled_tools.join(", ")
            };
            let skills_str = if skills.is_empty() {
                "无".to_string()
            } else {
                format!("{} 个技能", skills.len())
            };

            format!(
                "{} 设计用于提供以下能力：\n\n\
                 ✨ 核心功能（计划中）：\n\
                 • 💬 聊天驱动交互 - 自然语言对话\n\
                 • 🔧 工具使用 - 集成外部工具和服务\n\
                 • 🎯 技能系统 - 可扩展的技能模块\n\
                 • 💾 持久化运行 - 支持重启后恢复\n\n\
                 ⚙️ 当前状态：\n\
                 • 存根实现，等待 StateKnot 集成\n\
                 • 已注册工具：{}\n\
                 • 已发现技能：{}\n\
                 • 工作空间文件：{} 个已加载\n\n\
                 查看文档了解更多：docs/architecture.md",
                self.config.name,
                tools_str,
                skills_str,
                self.count_workspace_files()
            )
        } else if user_lower.contains("帮助") || user_lower.contains("help") {
            "📖 JiaClaw 帮助\n\n\
             命令:\n\
             • `jiaclaw init` - 初始化工作空间\n\
             • `jiaclaw version` - 显示版本信息\n\
             • `jiaclaw chat <消息>` - 发送单次聊天消息\n\
             • `jiaclaw doctor` - 检查配置和连接\n\
             • `jiaclaw memory show` - 显示长期记忆\n\
             • `jiaclaw soul show` - 显示人格\n\
             • `jiaclaw user show` - 显示用户画像\n\
             • `jiaclaw serve` - 启动 HTTP 服务（计划中）\n\n\
             配置:\n\
             • 使用 `--config <文件>` 指定配置文件\n\
             • 支持 TOML 和 JSON 格式\n\
             • 参见 config/jiaclaw.toml.example\n\n\
             文档:\n\
             • README.md - 项目概览\n\
             • docs/architecture.md - 架构设计\n\
             • docs/roadmap.md - 开发路线图"
                .to_string()
        } else if user_lower.contains("状态") || user_lower.contains("status") {
            format!(
                "🔍 {} 状态报告\n\n\
                 Agent 配置:\n\
                 • 名称: {}\n\
                 • 描述: {}\n\
                 • 最大轮次: {}\n\n\
                 会话信息:\n\
                 • 历史消息数: {}\n\
                 • 工作空间文件: {} 个\n\
                 • 发现技能: {} 个\n\
                 • 注册工具: {} 个\n\n\
                 系统状态:\n\
                 • 运行模式: 存根（Stub）\n\
                 • StateKnot 集成: 等待中\n\
                 • 持久化: 未启用\n\n\
                 💡 提示：运行 'jiaclaw doctor' 检查配置",
                self.config.name,
                self.config.name,
                self.config.description,
                self.config.max_turns,
                request.messages.len(),
                self.count_workspace_files(),
                skills.len(),
                self.tools.list().len()
            )
        } else if user_message.trim().is_empty() {
            "请发送一条消息开始对话。你可以说\"你好\"或询问\"你有什么功能\"。".to_string()
        } else {
            // 默认响应
            let workspace_hint = if self.count_workspace_files() > 0 {
                format!(
                    "\n\n💡 我已加载了你的工作空间配置（{} 个文件），包括你的偏好和记忆。",
                    self.count_workspace_files()
                )
            } else {
                "\n\n💡 运行 'jiaclaw init' 创建工作空间以个性化我的行为。".to_string()
            };

            format!(
                "我收到了你的消息：\"{user_message}\"\n\n\
                 目前我运行在存根模式下，无法进行实际的自然语言理解或生成。\
                 当 StateKnot 框架集成完成后，我将能够：\n\
                 • 理解复杂的自然语言输入\n\
                 • 使用模型生成智能回复\n\
                 • 调用工具完成实际任务\n\
                 • 保持持久化的对话上下文{workspace_hint}\n\n\
                 试试说\"帮助\"了解更多命令。"
            )
        }
    }

    /// 统计工作空间文件数量
    fn count_workspace_files(&self) -> usize {
        let mut count = 0;
        if self.workspace.agents.is_some() {
            count += 1;
        }
        if self.workspace.soul.is_some() {
            count += 1;
        }
        if self.workspace.user.is_some() {
            count += 1;
        }
        if self.workspace.memory.is_some() {
            count += 1;
        }
        count
    }
}

#[async_trait::async_trait]
impl ConversationSummarizer for JiaClawAgent {
    async fn summarize_conversation(
        &self,
        messages: &[ChatMessage],
    ) -> Result<String, JiaClawError> {
        self.summarize_messages_for_session(messages).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_config() -> AgentConfig {
        AgentConfig {
            provider: ProviderConfig {
                provider_type: "stub".into(),
                ..ProviderConfig::default()
            },
            ..AgentConfig::default()
        }
    }

    #[tokio::test]
    async fn summary_purpose_cannot_enter_the_tool_execution_path() {
        let agent = JiaClawAgent::new(stub_config()).unwrap();
        let error = agent
            .chat_for(&sample_request(), ModelPurpose::Summary)
            .await
            .unwrap_err();
        assert!(matches!(error, JiaClawError::InvalidRequest(_)));
    }

    #[test]
    fn provider_configuration_requires_explicit_stub_and_nonempty_key() {
        for provider_type in ["brokerrouter", "openai_compatible"] {
            for missing_key in [None, Some(""), Some(" \t\n")] {
                let provider = ProviderConfig {
                    provider_type: provider_type.into(),
                    api_key: missing_key.map(str::to_owned),
                    ..ProviderConfig::default()
                };
                let error = JiaClawAgent::resolve_provider_api_key(&provider, None).unwrap_err();
                assert!(matches!(error, JiaClawError::Configuration(_)));
                assert!(error.to_string().contains("API Key"));
            }
        }
        let unknown = ProviderConfig {
            provider_type: "unsupported-provider".into(),
            api_key: Some("configured-secret".into()),
            ..ProviderConfig::default()
        };
        let error =
            JiaClawAgent::resolve_provider_api_key(&unknown, Some("env-secret")).unwrap_err();
        assert!(matches!(error, JiaClawError::Configuration(_)));
        assert!(!error.to_string().contains("secret"));
        assert_eq!(
            JiaClawAgent::resolve_provider_api_key(&stub_config().provider, Some("env-secret"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn environment_key_overrides_configuration_including_empty_values() {
        let provider = ProviderConfig {
            api_key: Some("configured-secret".into()),
            ..ProviderConfig::default()
        };
        assert_eq!(
            JiaClawAgent::resolve_provider_api_key(&provider, Some("env-secret")).unwrap(),
            Some("env-secret".into())
        );
        assert_eq!(
            JiaClawAgent::resolve_provider_api_key(&provider, None).unwrap(),
            Some("configured-secret".into())
        );
        assert!(JiaClawAgent::resolve_provider_api_key(&provider, Some("")).is_err());
    }

    #[tokio::test]
    async fn invalid_provider_never_returns_a_successful_stub_response() {
        let agent = JiaClawAgent::new(AgentConfig {
            provider: ProviderConfig {
                provider_type: "misspelled-provider".into(),
                api_key: None,
                ..ProviderConfig::default()
            },
            ..AgentConfig::default()
        })
        .unwrap();
        assert!(matches!(
            agent.chat(&sample_request()).await,
            Err(JiaClawError::Configuration(_))
        ));
        assert!(matches!(
            agent
                .summarize_conversation(&sample_request().messages)
                .await,
            Err(JiaClawError::Configuration(_))
        ));
        assert!(matches!(
            agent
                .complete_turn(&sample_request().messages, "", "brokerrouter", None)
                .await,
            Err(JiaClawError::Configuration(_))
        ));
    }

    #[test]
    fn request_tool_allowlist_filters_prompt_and_empty_list_keeps_registered_tools() {
        let agent = JiaClawAgent::new(stub_config()).unwrap();
        let mut request = sample_request();
        assert_eq!(
            agent.allowed_tool_names(&request).unwrap().len(),
            agent.tools.list().len()
        );
        request.enabled_tools = vec!["datetime_now".into(), "datetime_now".into()];
        assert_eq!(agent.allowed_tool_names(&request).unwrap().len(), 1);
        let prompt = agent.build_system_prompt(&request);
        assert!(prompt.contains("### datetime_now\n"));
        assert!(!prompt.contains("### file_write\n"));
        assert!(!prompt.contains("### workspace_list\n"));
    }

    #[tokio::test]
    async fn unknown_requested_tools_are_rejected_before_model_execution() {
        let agent = JiaClawAgent::new(stub_config()).unwrap();
        let mut request = sample_request();
        request.enabled_tools = vec!["unknown-tool".into()];
        let error = agent.chat(&request).await.unwrap_err();
        assert!(matches!(error, JiaClawError::InvalidRequest(_)));
        assert!(error.to_string().contains("unknown-tool"));
    }

    struct CountExecutionTool(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl Tool for CountExecutionTool {
        fn name(&self) -> &str {
            "count_execution"
        }
        fn description(&self) -> &str {
            "test-only side effect counter"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, _args: serde_json::Value) -> Result<String, JiaClawError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("executed".into())
        }
    }

    #[tokio::test]
    async fn model_cannot_execute_a_registered_tool_outside_request_allowlist() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut agent = JiaClawAgent::new(stub_config()).unwrap();
        agent.register_tool_for_test(Box::new(CountExecutionTool(counter.clone())));
        agent.force_repeat_tool_for_test("count_execution");
        let mut request = sample_request();
        request.enabled_tools = vec!["datetime_now".into()];
        let error = agent.chat(&request).await.unwrap_err();
        assert!(matches!(error, JiaClawError::InvalidRequest(_)));
        assert!(error.to_string().contains("count_execution"));
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn model_can_execute_a_tool_in_request_allowlist() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut agent = JiaClawAgent::new(AgentConfig {
            max_tool_iterations: 2,
            ..stub_config()
        })
        .unwrap();
        agent.register_tool_for_test(Box::new(CountExecutionTool(counter.clone())));
        agent.force_repeat_tool_for_test("count_execution");
        let mut request = sample_request();
        request.enabled_tools = vec!["count_execution".into()];
        let response = agent.chat(&request).await.unwrap();
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            response.tool_calls[0].result,
            Some(serde_json::json!("executed"))
        );
    }

    #[tokio::test]
    async fn legacy_model_batch_is_authorized_before_any_tool_executes() {
        let content = "```tool\n{\"tool_name\":\"count_execution\",\"arguments\":{}}\n```\n\
                       ```tool\n{\"tool_name\":\"file_write\",\"arguments\":{}}\n```";
        let mock = mockito::mock("POST", "/chat/completions")
            .match_header("Authorization", "Bearer permission-test")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": content}}]
                })
                .to_string(),
            )
            .expect(1)
            .create();
        let mut agent = JiaClawAgent::new(AgentConfig {
            provider: ProviderConfig {
                provider_type: "openai_compatible".into(),
                base_url: mockito::server_url(),
                api_key: Some("permission-test".into()),
                ..ProviderConfig::default()
            },
            ..AgentConfig::default()
        })
        .unwrap();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        agent.register_tool_for_test(Box::new(CountExecutionTool(counter.clone())));
        let mut request = sample_request();
        request.enabled_tools = vec!["count_execution".into()];
        let error = agent.chat(&request).await.unwrap_err();
        assert!(matches!(error, JiaClawError::InvalidRequest(_)));
        assert!(error.to_string().contains("file_write"));
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
        mock.assert();
    }

    #[tokio::test]
    async fn disabled_file_operations_cannot_be_restored_through_legacy_aliases() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = stub_config();
        config.workspace_path = workspace.path().to_owned();
        config.tools.read_file.enabled = false;
        config.tools.write_file.enabled = false;
        config.tools.list_dir.enabled = false;
        config.tools.delete_file.enabled = false;
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        for name in [
            "read_file",
            "file_read",
            "write_file",
            "file_write",
            "list_dir",
            "file_list",
            "delete_file",
            "file_delete",
        ] {
            assert!(agent.tools().get(name).is_none(), "{name}");
            assert!(!prompt.contains(&format!("### {name}\n")), "{name}");
            let call = ToolCall {
                tool_name: name.into(),
                arguments: serde_json::json!({"path":"blocked.txt", "content":"must not write"}),
                result: None,
            };
            assert!(agent
                .tools()
                .execute(&call)
                .await
                .unwrap_err()
                .to_string()
                .contains("工具不存在"));
            let mut request = sample_request();
            request.enabled_tools = vec![name.into()];
            let error = agent.chat(&request).await.unwrap_err();
            assert!(
                matches!(error, JiaClawError::InvalidRequest(_)),
                "{name}: {error}"
            );
        }
        assert!(!workspace.path().join("blocked.txt").exists());
    }

    #[tokio::test]
    async fn native_model_cannot_forge_a_disabled_or_unlisted_file_alias() {
        for disabled in [false, true] {
            let workspace = tempfile::tempdir().unwrap();
            let key = format!("file-alias-permission-{}", uuid::Uuid::new_v4());
            let mock = mockito::mock("POST", "/v1/chat/completions")
                .match_header("Authorization", format!("Bearer {key}").as_str())
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_header("x-brokerrouter-request-id", &uuid::Uuid::new_v4().to_string())
                .with_body(serde_json::json!({
                    "choices":[{"index":0,"finish_reason":"tool_calls","message":{
                        "role":"assistant", "content":null, "tool_calls":[
                            {"id":"first", "type":"function", "function":{"name":"count_execution", "arguments":"{}"}},
                            {"id":"forged", "type":"function", "function":{"name":"file_write", "arguments":"{\"path\":\"forged.txt\",\"content\":\"must not write\"}"}}
                        ]
                    }}]
                }).to_string())
                .expect(1)
                .create();
            let mut config = AgentConfig {
                workspace_path: workspace.path().to_owned(),
                provider: ProviderConfig {
                    provider_type: "brokerrouter".into(),
                    base_url: mockito::server_url(),
                    api_key: Some(key.clone()),
                    ..ProviderConfig::default()
                },
                ..AgentConfig::default()
            };
            config.tools.write_file.enabled = !disabled;
            let mut agent = JiaClawAgent::new(config).unwrap();
            let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            agent.register_tool_for_test(Box::new(CountExecutionTool(counter.clone())));
            let mut request = sample_request();
            if !disabled {
                // The canonical name's permission does not grant its alias.
                request.enabled_tools = vec!["count_execution".into(), "write_file".into()];
            }
            let selection = agent
                .config
                .routing
                .select(&agent.config.provider, ModelPurpose::Chat);
            let error = agent
                .execute_brokerrouter_loop(&request, "fixture", &key, &selection, None)
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("outside this request's allowlist"),
                "{error}"
            );
            assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert!(!workspace.path().join("forged.txt").exists());
            mock.assert();
        }
    }

    #[test]
    fn test_agent_creation() {
        let config = AgentConfig::default();
        let agent = JiaClawAgent::new(config);
        assert!(agent.is_ok());
    }

    #[test]
    fn web_search_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("web_search").is_some(),
            "web_search should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("web_search"),
            "system prompt should describe web_search"
        );
        assert!(prompt.contains("max_results"), "{prompt}");
    }

    #[test]
    fn web_search_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                web_search: WebSearchToolConfig {
                    enabled: false,
                    brave_api_key: None,
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("web_search").is_none());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### web_search"),
            "disabled web_search must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn web_search_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                web_search: WebSearchToolConfig {
                    enabled: false,
                    brave_api_key: None,
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "hello"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn web_fetch_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("web_fetch").is_some(),
            "web_fetch should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("web_fetch"),
            "system prompt should describe web_fetch"
        );
        assert!(prompt.contains("max_chars"), "{prompt}");
    }

    #[test]
    fn web_fetch_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                web_fetch: WebFetchToolConfig {
                    enabled: false,
                    allow_private: false,
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("web_fetch").is_none());
        assert!(agent.tools().get("web_search").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### web_fetch"),
            "disabled web_fetch must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn web_fetch_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                web_fetch: WebFetchToolConfig {
                    enabled: false,
                    allow_private: false,
                },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "web_fetch".to_string(),
                arguments: serde_json::json!({"url": "https://example.com"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn memory_read_registration_uses_configured_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("notes")).unwrap();
        std::fs::write(temp.path().join("MEMORY.md"), "default-decoy").unwrap();
        std::fs::write(temp.path().join("notes/facts.md"), "configured-fact").unwrap();
        let config = AgentConfig {
            workspace_path: temp.path().to_path_buf(),
            memory: MemoryConfig {
                path: "notes/facts.md".into(),
                ..MemoryConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let result = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "memory_read".into(),
                arguments: serde_json::json!({"file": "MEMORY.md"}),
                result: None,
            })
            .await
            .unwrap();
        assert!(result.contains("configured-fact"));
        assert!(!result.contains("default-decoy"));
    }

    #[test]
    fn memory_search_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("memory_search").is_some(),
            "memory_search should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("memory_search"),
            "system prompt should describe memory_search"
        );
        assert!(prompt.contains("max_results"), "{prompt}");
    }

    #[test]
    fn memory_search_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                memory_search: MemorySearchToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("memory_search").is_none());
        assert!(agent.tools().get("web_search").is_some());
        assert!(agent.tools().get("web_fetch").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### memory_search"),
            "disabled memory_search must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn memory_search_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                memory_search: MemorySearchToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "memory_search".to_string(),
                arguments: serde_json::json!({"query": "hello"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn memory_write_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("memory_write").is_some(),
            "memory_write should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("memory_write"),
            "system prompt should describe memory_write"
        );
        assert!(prompt.contains("overwrite"), "{prompt}");
    }

    #[test]
    fn memory_write_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                memory_write: MemoryWriteToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("memory_write").is_none());
        assert!(agent.tools().get("memory_search").is_some());
        assert!(agent.tools().get("memory_append").is_none());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### memory_write") && !prompt.contains("### memory_append"),
            "disabled memory writers must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn memory_write_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                memory_write: MemoryWriteToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        for name in ["memory_write", "memory_append"] {
            let err = agent
                .tools()
                .execute(&ToolCall {
                    tool_name: name.to_string(),
                    arguments: serde_json::json!({"content": "hello"}),
                    result: None,
                })
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("工具不存在"),
                "unexpected error for {name}: {err}"
            );
        }
    }

    #[test]
    fn read_file_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("read_file").is_some(),
            "read_file should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("read_file"),
            "system prompt should describe read_file"
        );
        assert!(prompt.contains("offset"), "{prompt}");
    }

    #[test]
    fn read_file_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                read_file: ReadFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("read_file").is_none());
        assert!(agent.tools().get("file_read").is_none());
        assert!(agent.tools().get("list_dir").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### read_file"),
            "disabled read_file must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn read_file_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                read_file: ReadFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "MEMORY.md"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn list_dir_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("list_dir").is_some(),
            "list_dir should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("list_dir"),
            "system prompt should describe list_dir"
        );
        assert!(prompt.contains("max_entries"), "{prompt}");
    }

    #[test]
    fn list_dir_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                list_dir: ListDirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("list_dir").is_none());
        assert!(agent.tools().get("file_list").is_none());
        assert!(agent.tools().get("read_file").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### list_dir"),
            "disabled list_dir must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn list_dir_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                list_dir: ListDirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "list_dir".to_string(),
                arguments: serde_json::json!({"path": "."}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn write_file_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("write_file").is_some(),
            "write_file should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("write_file"),
            "system prompt should describe write_file"
        );
        assert!(prompt.contains("overwrite"), "{prompt}");
    }

    #[test]
    fn write_file_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                write_file: WriteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("write_file").is_none());
        assert!(agent.tools().get("file_write").is_none());
        assert!(agent.tools().get("read_file").is_some());
        assert!(agent.tools().get("list_dir").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### write_file"),
            "disabled write_file must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn write_file_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                write_file: WriteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "write_file".to_string(),
                arguments: serde_json::json!({"path": "notes.md", "content": "x"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn delete_file_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("delete_file").is_some(),
            "delete_file should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("delete_file"),
            "system prompt should describe delete_file"
        );
        assert!(
            prompt.contains("拒绝删除目录") || prompt.contains("常规文件"),
            "{prompt}"
        );
    }

    #[test]
    fn delete_file_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                delete_file: DeleteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("delete_file").is_none());
        assert!(agent.tools().get("file_delete").is_none());
        assert!(agent.tools().get("write_file").is_some());
        assert!(agent.tools().get("read_file").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### delete_file"),
            "disabled delete_file must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn delete_file_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                delete_file: DeleteFileToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "delete_file".to_string(),
                arguments: serde_json::json!({"path": "notes.md"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn str_replace_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("str_replace").is_some(),
            "str_replace should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("str_replace"),
            "system prompt should describe str_replace"
        );
        assert!(
            prompt.contains("精确字符串替换") || prompt.contains("replace_all"),
            "{prompt}"
        );
    }

    #[test]
    fn str_replace_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                str_replace: StrReplaceToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("str_replace").is_none());
        assert!(agent.tools().get("write_file").is_some());
        assert!(agent.tools().get("read_file").is_some());
        assert!(agent.tools().get("delete_file").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### str_replace"),
            "disabled str_replace must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn str_replace_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                str_replace: StrReplaceToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "str_replace".to_string(),
                arguments: serde_json::json!({
                    "path": "notes.md",
                    "old_str": "a",
                    "new_str": "b"
                }),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn grep_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("grep").is_some(),
            "grep should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("grep"),
            "system prompt should describe grep"
        );
        assert!(
            prompt.contains("字面量") || prompt.contains("非正则"),
            "{prompt}"
        );
    }

    #[test]
    fn grep_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                grep: GrepToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("grep").is_none());
        assert!(agent.tools().get("str_replace").is_some());
        assert!(agent.tools().get("read_file").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### grep"),
            "disabled grep must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn grep_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                grep: GrepToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "foo"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn filesystem_info_configuration_controls_registry_and_prompt() {
        for (stat, tree) in [(true, true), (false, true), (true, false), (false, false)] {
            let workspace = tempfile::tempdir().unwrap();
            let agent = JiaClawAgent::new(AgentConfig {
                workspace_path: workspace.path().to_path_buf(),
                tools: ToolsConfig {
                    stat: StatToolConfig { enabled: stat },
                    tree: TreeToolConfig { enabled: tree },
                    ..ToolsConfig::default()
                },
                ..AgentConfig::default()
            })
            .unwrap();
            let prompt = agent.build_system_prompt(&ChatRequest {
                messages: vec![],
                enabled_tools: vec![],
                enabled_skills: vec![],
                auto_skills: true,
                session_id: None,
            });
            for (name, enabled) in [("stat", stat), ("tree", tree)] {
                assert_eq!(agent.tools().get(name).is_some(), enabled);
                assert_eq!(prompt.contains(&format!("### {name}")), enabled);
                if !enabled {
                    let error = agent
                        .tools()
                        .execute(&ToolCall {
                            tool_name: name.into(),
                            arguments: serde_json::json!({}),
                            result: None,
                        })
                        .await
                        .unwrap_err();
                    assert!(error.to_string().contains("工具不存在"));
                }
            }
            assert!(agent.tools().get("list_dir").is_some());
        }
    }

    #[test]
    fn glob_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("glob").is_some(),
            "glob should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("glob"),
            "system prompt should describe glob"
        );
        assert!(
            prompt.contains("glob 模式") || prompt.contains("**/*"),
            "{prompt}"
        );
    }

    #[test]
    fn glob_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                glob: GlobToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("glob").is_none());
        assert!(agent.tools().get("grep").is_some());
        assert!(agent.tools().get("read_file").is_some());
        assert!(agent.tools().get("mkdir").is_some());
        assert!(agent.tools().get("move").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### glob"),
            "disabled glob must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn glob_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                glob: GlobToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "glob".to_string(),
                arguments: serde_json::json!({"pattern": "**/*.rs"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mkdir_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("mkdir").is_some(),
            "mkdir should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("mkdir"),
            "system prompt should describe mkdir"
        );
        assert!(
            prompt.contains("mkdir -p") || prompt.contains("recursive"),
            "{prompt}"
        );
    }

    #[test]
    fn mkdir_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                mkdir: MkdirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("mkdir").is_none());
        assert!(agent.tools().get("glob").is_some());
        assert!(agent.tools().get("read_file").is_some());
        assert!(agent.tools().get("move").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### mkdir"),
            "disabled mkdir must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn mkdir_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                mkdir: MkdirToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "mkdir".to_string(),
                arguments: serde_json::json!({"path": "notes"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn move_is_registered_by_default() {
        let agent = JiaClawAgent::new(AgentConfig::default()).unwrap();
        assert!(
            agent.tools().get("move").is_some(),
            "move should be in the default tool list"
        );
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            prompt.contains("move"),
            "system prompt should describe move"
        );
        assert!(
            prompt.contains("from") && (prompt.contains("overwrite") || prompt.contains("to")),
            "{prompt}"
        );
    }

    #[test]
    fn move_is_not_registered_when_disabled() {
        let config = AgentConfig {
            tools: ToolsConfig {
                r#move: MoveToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        assert!(agent.tools().get("move").is_none());
        assert!(agent.tools().get("mkdir").is_some());
        assert!(agent.tools().get("read_file").is_some());
        let prompt = agent.build_system_prompt(&ChatRequest {
            messages: vec![],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        });
        assert!(
            !prompt.contains("### move"),
            "disabled move must not appear in tool docs"
        );
    }

    #[tokio::test]
    async fn move_execute_when_unregistered_returns_missing_tool() {
        let config = AgentConfig {
            tools: ToolsConfig {
                r#move: MoveToolConfig { enabled: false },
                ..ToolsConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let err = agent
            .tools()
            .execute(&ToolCall {
                tool_name: "move".to_string(),
                arguments: serde_json::json!({"from": "a.md", "to": "b.md"}),
                result: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("工具不存在"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_chat_stub() {
        let config = stub_config();
        let agent = JiaClawAgent::new(config).unwrap();

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

        let response = agent.chat(&request).await;
        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_tool_execution_in_stub_mode() {
        let config = stub_config();
        let agent = JiaClawAgent::new(config).unwrap();

        // 测试工具调用触发
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

        let response = agent.chat(&request).await;
        assert!(response.is_ok());
        let response = response.unwrap();

        // 验证工具被执行了
        assert!(
            !response.tool_calls.is_empty(),
            "工具应该被执行，tool_calls 不应为空"
        );
        assert_eq!(response.tool_calls[0].tool_name, "workspace_list");
        assert!(
            response.tool_calls[0].result.is_some(),
            "工具应该有执行结果"
        );
    }

    struct SlowSleepTool {
        delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl Tool for SlowSleepTool {
        fn name(&self) -> &str {
            "slow_sleep"
        }

        fn description(&self) -> &str {
            "test-only slow tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        async fn execute(&self, _args: serde_json::Value) -> Result<String, JiaClawError> {
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
    async fn unconfigured_tool_timeout_does_not_change_behavior() {
        let config = AgentConfig::default();
        assert_eq!(config.tool_timeout_secs, None);
        assert_eq!(config.effective_tool_timeout_secs(), None);

        let mut agent = JiaClawAgent::new(config).unwrap();
        agent.register_tool_for_test(Box::new(SlowSleepTool {
            delay: std::time::Duration::from_millis(50),
        }));

        let (recorded, message) = agent.execute_and_record(slow_sleep_call()).await;
        let result = recorded.result.expect("tool result");
        assert!(!result.to_string().contains("timed out"), "{result}");
        assert!(message.contains("执行成功"), "{message}");
        assert_eq!(result, serde_json::json!("slept"));
    }

    #[tokio::test]
    async fn short_tool_timeout_records_error_in_tool_result() {
        let config = AgentConfig {
            tool_timeout_secs: Some(1),
            ..AgentConfig::default()
        };
        let mut agent = JiaClawAgent::new(config).unwrap();
        agent.register_tool_for_test(Box::new(SlowSleepTool {
            delay: std::time::Duration::from_secs(10),
        }));

        let (recorded, message) = agent.execute_and_record(slow_sleep_call()).await;
        let result = recorded.result.expect("tool result");
        let result_text = result.to_string();
        assert!(
            result_text.contains("Tool timed out after 1s"),
            "timeout should be in tool result, got: {result_text}"
        );
        assert!(message.contains("Tool timed out after 1s"), "{message}");
    }

    #[tokio::test]
    async fn configured_timeout_does_not_fail_fast_tools() {
        let config = AgentConfig {
            tool_timeout_secs: Some(30),
            ..stub_config()
        };
        let agent = JiaClawAgent::new(config).unwrap();

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

        let response = agent.chat(&request).await.unwrap();
        assert_eq!(response.tool_calls[0].tool_name, "workspace_list");
        let result = response.tool_calls[0]
            .result
            .as_ref()
            .expect("tool result")
            .to_string();
        assert!(!result.contains("timed out"), "{result}");
    }

    #[tokio::test]
    async fn test_tool_call_parsing() {
        // 测试工具调用解析
        let content = r#"
让我列出工作空间文件。

```tool
{
  "tool_name": "workspace_list",
  "arguments": {}
}
```
"#;

        let tool_calls = JiaClawAgent::parse_tool_calls(content);
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].tool_name, "workspace_list");
    }

    #[tokio::test]
    async fn test_multiple_tool_calls_parsing() {
        // 测试多个工具调用解析
        let content = r#"
让我先获取时间，然后列出文件。

```tool
{
  "tool_name": "datetime_now",
  "arguments": {}
}
```

然后

```tool
{
  "tool_name": "workspace_list",
  "arguments": {}
}
```
"#;

        let tool_calls = JiaClawAgent::parse_tool_calls(content);
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].tool_name, "datetime_now");
        assert_eq!(tool_calls[1].tool_name, "workspace_list");
    }

    #[tokio::test]
    async fn tool_loop_stops_at_configured_max_tool_iterations() {
        let dir = unique_workspace("jiaclaw_max_tool_iter");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            max_tool_iterations: 2,
            ..stub_config()
        };
        assert_eq!(config.effective_max_tool_iterations(), 2);

        let mut agent = JiaClawAgent::new(config).unwrap();
        agent.force_repeat_tool_for_test("datetime_now");

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "反复调用工具".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = agent.chat(&request).await.unwrap();
        // max=2：第 1 轮执行，第 2 轮达上限停止（不执行，写入 tool/assistant 提示）
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].tool_name, "datetime_now");
        let first = response.tool_calls[0]
            .result
            .as_ref()
            .expect("executed tool result")
            .to_string();
        assert!(
            !first.contains("最大工具迭代次数"),
            "first call should execute, got: {first}"
        );

        assert_eq!(response.tool_calls[1].tool_name, "datetime_now");
        let skipped = response.tool_calls[1]
            .result
            .as_ref()
            .expect("skipped tool result")
            .to_string();
        assert!(
            skipped.contains("最大工具迭代次数（2）"),
            "limit hint should be in skipped tool result, got: {skipped}"
        );
        assert!(
            response.message.content.contains("最大工具迭代次数（2）"),
            "limit hint should be in assistant message: {}",
            response.message.content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tool_loop_default_limit_matches_legacy_constant() {
        let dir = unique_workspace("jiaclaw_max_tool_iter_default");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..stub_config()
        };
        assert_eq!(
            config.effective_max_tool_iterations(),
            DEFAULT_MAX_TOOL_ITERATIONS
        );

        let mut agent = JiaClawAgent::new(config).unwrap();
        agent.force_repeat_tool_for_test("datetime_now");

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "反复调用工具".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = agent.chat(&request).await.unwrap();
        // 默认 5：前 4 轮执行，第 5 轮达上限停止
        assert_eq!(response.tool_calls.len(), DEFAULT_MAX_TOOL_ITERATIONS);
        for call in response
            .tool_calls
            .iter()
            .take(DEFAULT_MAX_TOOL_ITERATIONS - 1)
        {
            let result = call.result.as_ref().expect("tool result").to_string();
            assert!(
                !result.contains("最大工具迭代次数"),
                "executed calls should not carry the stop hint: {result}"
            );
        }
        let last = response.tool_calls[DEFAULT_MAX_TOOL_ITERATIONS - 1]
            .result
            .as_ref()
            .expect("limit tool result")
            .to_string();
        assert!(
            last.contains(&format!(
                "最大工具迭代次数（{DEFAULT_MAX_TOOL_ITERATIONS}）"
            )),
            "{last}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_auto_skills_disabled() {
        // 测试禁用自动技能激活
        let config = stub_config();
        let agent = JiaClawAgent::new(config).unwrap();

        // 假设有一个技能会被 "搜索" 触发
        // 但我们设置 auto_skills = false
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "搜索 OpenAI".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: false,
            session_id: None,
        };

        let response = agent.chat(&request).await;
        assert!(response.is_ok());

        // 即使内容可能触发技能，也不应该自动启用
        // （因为默认工作空间没有技能，这个测试主要验证不会崩溃）
    }

    #[tokio::test]
    async fn test_auto_skills_enabled() {
        // 测试启用自动技能激活
        let config = stub_config();
        let agent = JiaClawAgent::new(config).unwrap();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "搜索信息".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        };

        let response = agent.chat(&request).await;
        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_explicit_skills_with_auto_disabled() {
        // 测试显式指定技能 + 禁用自动激活
        // 显式技能应该仍然生效
        let config = stub_config();
        let agent = JiaClawAgent::new(config).unwrap();

        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec!["calculator".to_string()],
            auto_skills: false,
            session_id: None,
        };

        let response = agent.chat(&request).await;
        assert!(response.is_ok());
        // 显式指定的技能应该在系统提示中
    }

    fn sample_request() -> ChatRequest {
        ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "你好".to_string(),
            }],
            enabled_tools: vec![],
            enabled_skills: vec![],
            auto_skills: true,
            session_id: None,
        }
    }

    fn unique_workspace(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn chat_without_memory_file_does_not_error() {
        let dir = unique_workspace("jiaclaw_agent_no_mem");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(
            !prompt.contains("Long-term Memory"),
            "无 MEMORY 文件时不应注入记忆区块: {prompt}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_prompt_includes_memory_file_content() {
        let dir = unique_workspace("jiaclaw_agent_with_mem");
        std::fs::write(
            dir.join("MEMORY.md"),
            "UNIQUE_MEMORY_TOKEN_prefer_dark_mode",
        )
        .unwrap();
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(prompt.contains("Long-term Memory"));
        assert!(prompt.contains("UNIQUE_MEMORY_TOKEN_prefer_dark_mode"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_append_then_prompt_sees_file() {
        let dir = unique_workspace("jiaclaw_agent_append_mem");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();

        let prompt_before = agent.build_system_prompt(&sample_request());
        assert!(!prompt_before.contains("learned-fact-xyz"));

        let call = ToolCall {
            tool_name: "memory_append".to_string(),
            arguments: serde_json::json!({"content": "- learned-fact-xyz"}),
            result: None,
        };
        let result = agent.tools().execute(&call).await.unwrap();
        assert!(result.contains("追加") || result.contains("记忆"));

        let on_disk = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        assert!(on_disk.contains("learned-fact-xyz"));

        let prompt_after = agent.build_system_prompt(&sample_request());
        assert!(prompt_after.contains("learned-fact-xyz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_write_then_prompt_sees_file() {
        let dir = unique_workspace("jiaclaw_agent_write_mem");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();

        let prompt_before = agent.build_system_prompt(&sample_request());
        assert!(!prompt_before.contains("written-fact-abc"));

        let call = ToolCall {
            tool_name: "memory_write".to_string(),
            arguments: serde_json::json!({"content": "- written-fact-abc"}),
            result: None,
        };
        let result = agent.tools().execute(&call).await.unwrap();
        assert!(result.contains("append"), "{result}");
        assert!(result.contains("bytes_written"), "{result}");
        assert!(result.contains("MEMORY.md"), "{result}");

        let on_disk = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        assert!(on_disk.contains("written-fact-abc"));

        let prompt_after = agent.build_system_prompt(&sample_request());
        assert!(prompt_after.contains("written-fact-abc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_memory_path_injected_into_prompt() {
        let dir = unique_workspace("jiaclaw_agent_custom_mem");
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("notes/keep.md"), "custom-rel-path-token").unwrap();
        let config = AgentConfig {
            workspace_path: dir.clone(),
            memory: MemoryConfig {
                path: "notes/keep.md".to_string(),
                ..MemoryConfig::default()
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(prompt.contains("custom-rel-path-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_without_soul_or_user_file_does_not_error() {
        let dir = unique_workspace("jiaclaw_agent_no_id");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(
            !prompt.contains("## Soul（人格）"),
            "无 SOUL 文件时不应注入人格区块: {prompt}"
        );
        assert!(
            !prompt.contains("## User（用户画像）"),
            "无 USER 文件时不应注入用户画像区块: {prompt}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_prompt_includes_soul_and_user_file_content() {
        let dir = unique_workspace("jiaclaw_agent_with_id");
        std::fs::write(dir.join("SOUL.md"), "UNIQUE_SOUL_TOKEN_be_concise").unwrap();
        std::fs::write(dir.join("USER.md"), "UNIQUE_USER_TOKEN_likes_rust").unwrap();
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(prompt.contains("## Soul（人格）"));
        assert!(prompt.contains("UNIQUE_SOUL_TOKEN_be_concise"));
        assert!(prompt.contains("## User（用户画像）"));
        assert!(prompt.contains("UNIQUE_USER_TOKEN_likes_rust"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn soul_user_and_memory_coexist_in_prompt() {
        let dir = unique_workspace("jiaclaw_agent_id_mem");
        std::fs::write(dir.join("SOUL.md"), "SOUL_TOKEN_AAA").unwrap();
        std::fs::write(dir.join("USER.md"), "USER_TOKEN_BBB").unwrap();
        std::fs::write(dir.join("MEMORY.md"), "MEMORY_TOKEN_CCC").unwrap();
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(prompt.contains("SOUL_TOKEN_AAA"));
        assert!(prompt.contains("USER_TOKEN_BBB"));
        assert!(prompt.contains("MEMORY_TOKEN_CCC"));
        assert!(prompt.contains("## Soul（人格）"));
        assert!(prompt.contains("## User（用户画像）"));
        assert!(prompt.contains("Long-term Memory"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn soul_write_then_prompt_sees_file_without_clobbering_memory() {
        let dir = unique_workspace("jiaclaw_agent_soul_write");
        std::fs::write(dir.join("MEMORY.md"), "keep-memory-xyz").unwrap();
        std::fs::write(dir.join("USER.md"), "keep-user-xyz").unwrap();
        let config = AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();

        let call = ToolCall {
            tool_name: "soul_write".to_string(),
            arguments: serde_json::json!({"content": "new-soul-xyz"}),
            result: None,
        };
        let result = agent.tools().execute(&call).await.unwrap();
        assert!(result.contains("覆盖") || result.contains("SOUL"));

        assert_eq!(
            std::fs::read_to_string(dir.join("SOUL.md")).unwrap(),
            "new-soul-xyz"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("MEMORY.md")).unwrap(),
            "keep-memory-xyz"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("USER.md")).unwrap(),
            "keep-user-xyz"
        );

        let prompt = agent.build_system_prompt(&sample_request());
        assert!(prompt.contains("new-soul-xyz"));
        assert!(prompt.contains("keep-memory-xyz"));
        assert!(prompt.contains("keep-user-xyz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_identity_paths_injected_into_prompt() {
        let dir = unique_workspace("jiaclaw_agent_custom_id");
        std::fs::create_dir_all(dir.join("persona")).unwrap();
        std::fs::write(dir.join("persona/soul.md"), "custom-soul-rel").unwrap();
        std::fs::write(dir.join("persona/user.md"), "custom-user-rel").unwrap();
        let config = AgentConfig {
            workspace_path: dir.clone(),
            identity: IdentityConfig {
                soul_path: "persona/soul.md".to_string(),
                user_path: "persona/user.md".to_string(),
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(prompt.contains("custom-soul-rel"));
        assert!(prompt.contains("custom-user-rel"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn identity_path_traversal_is_skipped_not_injected() {
        let dir = unique_workspace("jiaclaw_agent_id_trav");
        let config = AgentConfig {
            workspace_path: dir.clone(),
            identity: IdentityConfig {
                soul_path: "../evil.md".to_string(),
                user_path: "/etc/passwd".to_string(),
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let prompt = agent.build_system_prompt(&sample_request());
        assert!(!prompt.contains("## Soul（人格）"));
        assert!(!prompt.contains("## User（用户画像）"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn overflow_messages(count: usize) -> Vec<ChatMessage> {
        (0..count)
            .map(|i| ChatMessage {
                role: MessageRole::User,
                content: format!("消息 {i}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn compact_session_messages_off_hard_truncates() {
        let config = AgentConfig::default();
        assert!(!config.session.effective_summarize_on_overflow());
        let agent = JiaClawAgent::new(config).unwrap();
        let compacted = agent
            .compact_session_messages(overflow_messages(60))
            .await
            .unwrap();
        let expected = hard_truncate_session_messages(overflow_messages(60), MAX_SESSION_MESSAGES);
        assert_eq!(compacted, expected);
        assert_eq!(compacted.len(), MAX_SESSION_MESSAGES);
        assert_eq!(compacted.last().unwrap().content, "消息 59");
        assert!(compacted.iter().all(|m| m.content != "消息 0"));
    }

    #[tokio::test]
    async fn compact_session_messages_on_uses_local_digest_in_explicit_stub_mode() {
        let config = AgentConfig {
            session: SessionConfig {
                summarize_on_overflow: true,
                keep_recent: 10,
            },
            ..stub_config()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let compacted = agent
            .compact_session_messages(overflow_messages(51))
            .await
            .unwrap();
        assert_eq!(compacted.len(), 11);
        assert_eq!(compacted[0].role, MessageRole::System);
        assert!(compacted[0].content.contains(SESSION_SUMMARY_PREFIX));
        assert!(compacted[0].content.contains("消息 0"));
        assert_eq!(compacted[1].content, "消息 41");
        assert_eq!(compacted.last().unwrap().content, "消息 50");
    }

    #[tokio::test]
    async fn summarize_conversation_uses_mock_provider_without_tools() {
        let response_body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Summary: user asked about 0-9."
                },
                "finish_reason": "stop"
            }]
        });
        let _mock = mockito::mock("POST", "/v1/chat/completions")
            .match_header("Authorization", "Bearer brk_summary_key")
            .with_status(200)
            .with_body(response_body.to_string())
            .expect(2)
            .create();

        let config = AgentConfig {
            provider: ProviderConfig {
                provider_type: "brokerrouter".to_string(),
                base_url: mockito::server_url(),
                api_key: Some("brk_summary_key".to_string()),
                model: "gpt-4o-mini".to_string(),
                temperature: 0.7,
                max_tokens: 4096,
            },
            session: SessionConfig {
                summarize_on_overflow: true,
                keep_recent: 10,
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let summary = agent
            .summarize_conversation(&overflow_messages(12))
            .await
            .expect("mock summary");
        assert_eq!(summary, "Summary: user asked about 0-9.");

        let compacted = agent
            .compact_session_messages(overflow_messages(51))
            .await
            .unwrap();
        assert_eq!(compacted.len(), 11);
        assert!(compacted[0].content.contains(SESSION_SUMMARY_PREFIX));
        assert!(compacted[0]
            .content
            .contains("Summary: user asked about 0-9."));
        assert_eq!(compacted.last().unwrap().content, "消息 50");
    }

    #[tokio::test]
    async fn summarize_provider_error_compacts_by_hard_truncate() {
        let _mock = mockito::mock("POST", "/v1/chat/completions")
            .match_header("Authorization", "Bearer brk_fail_key")
            .with_status(500)
            .with_body("upstream down")
            .create();

        let config = AgentConfig {
            provider: ProviderConfig {
                provider_type: "brokerrouter".to_string(),
                base_url: mockito::server_url(),
                api_key: Some("brk_fail_key".to_string()),
                model: "gpt-4o-mini".to_string(),
                temperature: 0.7,
                max_tokens: 4096,
            },
            session: SessionConfig {
                summarize_on_overflow: true,
                keep_recent: 10,
            },
            ..AgentConfig::default()
        };
        let agent = JiaClawAgent::new(config).unwrap();
        let compacted = agent
            .compact_session_messages(overflow_messages(60))
            .await
            .unwrap();
        let expected = hard_truncate_session_messages(overflow_messages(60), MAX_SESSION_MESSAGES);
        assert_eq!(compacted, expected);
    }

    fn write_skill(workspace: &std::path::Path, name: &str, body: &str) {
        let dir = workspace.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn reload_skills_picks_up_new_skill_file() {
        let dir = unique_workspace("jiaclaw_agent_reload_ok");
        write_skill(&dir, "alpha", "# Alpha\n\n## Description\n\nFirst skill\n");
        let agent = JiaClawAgent::new(AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        })
        .unwrap();
        assert!(agent.skills().iter().any(|s| s.name == "alpha"));
        assert!(!agent.skills().iter().any(|s| s.name == "beta"));

        write_skill(&dir, "beta", "# Beta\n\n## Description\n\nSecond skill\n");
        let reloaded = agent.reload_skills().expect("重载应成功");
        assert!(reloaded.iter().any(|s| s.name == "beta"));
        assert!(agent.skills().iter().any(|s| s.name == "beta"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_skills_keeps_old_table_on_bad_file() {
        let dir = unique_workspace("jiaclaw_agent_reload_bad");
        write_skill(&dir, "alpha", "# Alpha\n\n## Description\n\nFirst skill\n");
        let agent = JiaClawAgent::new(AgentConfig {
            workspace_path: dir.clone(),
            ..AgentConfig::default()
        })
        .unwrap();

        write_skill(&dir, "broken", "---\nname: [not yaml\n---\n# Broken\n");
        let err = agent.reload_skills().expect_err("坏文件应使重载失败");
        assert!(
            err.to_string().contains("无效技能"),
            "错误应说明无效技能，实际: {err}"
        );
        let names: Vec<_> = agent.skills().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["alpha".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reload_skills_does_not_panic_during_concurrent_chat() {
        let dir = unique_workspace("jiaclaw_agent_reload_conc");
        write_skill(&dir, "alpha", "# Alpha\n\n## Description\n\nFirst skill\n");
        let agent = std::sync::Arc::new(
            JiaClawAgent::new(AgentConfig {
                workspace_path: dir.clone(),
                ..stub_config()
            })
            .unwrap(),
        );

        let chatting = agent.clone();
        let chat = tokio::spawn(async move { chatting.chat(&sample_request()).await });
        let reloading = agent.clone();
        let reload = tokio::task::spawn_blocking(move || reloading.reload_skills());
        let (chat_result, reload_result) = tokio::join!(chat, reload);
        assert!(chat_result.expect("chat join").is_ok());
        assert!(reload_result.expect("reload join").is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
