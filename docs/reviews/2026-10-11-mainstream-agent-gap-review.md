# JiaClaw 与个人 Agent／软件 Agent 的能力差距核对

核对日：2026-10-11，Asia/Shanghai。研究方法：按用户指定的 `mattpocock-skills:research`，只采用官方发布、文档、源码和 GitHub API；JiaClaw 代码与证据按当前开发分支逐项核对。

结论：JiaClaw 当前开发分支已具备许多基础工具和持久化能力。最显著的剩余差距是首用闭环、技能生命周期与经验积累、真实浏览器、审批后继续、子 Agent 与长任务、完整外部工具/媒体，以及租户功能范围和真实环境资格。下文分别给出竞品官方支持范围、JiaClaw 固定代码依据和验收建议；不将竞品文档或本项目 fixture 当作真实任务效果认证。

## 证据范围与版本

本次选择 OpenClaw、Hermes Agent 作为个人助理比较对象，OpenHands 作为软件任务执行与控制台的邻近参照；Claude Code 用于补充技能、委派、审批和记忆设计。这个选择沿用用户指定的产品和项目定位，不用 star 数量或宣传用语证明“主流”与质量。竞品部分是官方资料核对；本机没有安装、运行竞品、调用模型或供应商，更没有认证它们的真实任务成功率。

GitHub `releases/latest` 的实时结果与搜索缓存不同，以下采用实时 API。发布标签与当前 main 分开列出；官网文档是核对日的滚动版本，不能据此保证每一项都已进入相应稳定安装包。

| 组件 | 当前最新非预发布与固定提交 | 当前 main 快照 | 发布／分发边界 |
| --- | --- | --- | --- |
| OpenClaw | [v2026.9.9](https://github.com/openclaw/openclaw/releases/tag/v2026.9.9)，2026-10-08；标签解析为 [bcfc88812a35243893585dbeca87ca41b48272ca](https://github.com/openclaw/openclaw/commit/bcfc88812a35243893585dbeca87ca41b48272ca) | [379da8e456922f1abd32b6241a1c99e3dea5f755](https://github.com/openclaw/openclaw/commit/379da8e456922f1abd32b6241a1c99e3dea5f755) | 比较稳定版，不把 beta 能力自动计入。版本来自 [官方 latest API](https://api.github.com/repos/openclaw/openclaw/releases/latest)。 |
| Hermes Agent | [v0.21.6](https://github.com/NousResearch/hermes-agent/releases/tag/v0.21.6)，2026-10-08；[818c13be1dc4fd28987e1e881a9408224afd4535](https://github.com/NousResearch/hermes-agent/commit/818c13be1dc4fd28987e1e881a9408224afd4535) | [a62979dc3601c9eae455bff5b35ba8f9bed0df30](https://github.com/NousResearch/hermes-agent/commit/a62979dc3601c9eae455bff5b35ba8f9bed0df30) | 此发布交付 tag、GitHub Release、Docker；发布说明明确桌面、Termux、Microsoft Store 保持各自现有构建，不能混算资格。[官方发布](https://github.com/NousResearch/hermes-agent/releases/tag/v0.21.6) |
| OpenHands Agent Canvas | [v1.26.0](https://github.com/OpenHands/OpenHands/releases/tag/v1.26.0)，2026-10-08；[3500d9e5c7cbdfb5a2cbfe9499ce4448eb10c0a2](https://github.com/OpenHands/OpenHands/commit/3500d9e5c7cbdfb5a2cbfe9499ce4448eb10c0a2) | [e0a26af82f0b2016423c0d53cc9ae32a357cef2d](https://github.com/OpenHands/OpenHands/commit/e0a26af82f0b2016423c0d53cc9ae32a357cef2d) | 当前 `OpenHands/OpenHands` 是 Canvas 浏览器控制台；不与旧单仓库 GUI 或 SDK 同版本处理。[官方组件地图](https://docs.openhands.dev/overview/introduction) |
| OpenHands Software Agent SDK／Agent Server | [v1.54.0](https://github.com/OpenHands/software-agent-sdk/releases/tag/v1.54.0)，2026-10-09；[1d471742ea7be017d013eaaa3080f5d28d5490cb](https://github.com/OpenHands/software-agent-sdk/commit/1d471742ea7be017d013eaaa3080f5d28d5490cb) | [e2bac66be26e94f571f27b71f507e7192720f825](https://github.com/OpenHands/software-agent-sdk/commit/e2bac66be26e94f571f27b71f507e7192720f825) | SDK／Server 与 Canvas 分仓、分版本；文档仓库本次快照为 [bb02b4eae847d1cc6bc831428b41fdff5617c74d](https://github.com/OpenHands/docs/commit/bb02b4eae847d1cc6bc831428b41fdff5617c74d)。 |

## OpenClaw：首用、浏览器和渠道的官方能力

| 维度 | 官方资料中可核实的能力 | 限制与比较口径 |
| --- | --- | --- |
| 安装与第一次成功任务 | 安装脚本、引导、Gateway 后台服务和 Web Dashboard 形成连续路径；Quick start 会尝试复用检测到的模型登录／Key，并用真实 completion 验证后保存配置。[Getting started](https://docs.openclaw.ai/start/getting-started) | “约五分钟”是官方目标，本次未计时实测。Node 与平台服务条件需满足，凭据仍受模型方条款和账户权限约束。 |
| 真实浏览器操作 | 可控制独立 Chromium 系 profile 的标签、点击、输入、拖动、截图和页面；另有显式连接现有已登录 Chrome 的路径。[Browser](https://docs.openclaw.ai/tools/browser) | 网页提取与真实浏览器操作是两种能力；默认独立 profile 与连接个人登录态的授权边界要分别核验。 |
| 渠道／平台 | 官方有 WhatsApp、Telegram、Slack、Discord、Feishu、WeCom 等独立设置文档；WhatsApp 提供设备链接、准入、会话和消息路径。[渠道索引](https://docs.openclaw.ai/channels)、[WhatsApp](https://docs.openclaw.ai/channels/whatsapp) | 文档存在证明适配路径，不能证明用户账号、供应商网络或全部媒体类型已在本机认证。 |
| Skills 生命周期 | 有安装、更新、启用配置、注册来源和锁元数据；提供有界目录、`skills_search` 与完整 `skills_read`，模型按需读取正文。[Skills](https://docs.openclaw.ai/tools/skills) | 第三方 skill 是不可信内容；扫描不等于安全保证。Git／本地安装的更新方式与 ClawHub 跟踪更新有区别。 |
| MCP／外部工具 | 官方配置支持 Stdio、SSE、Streamable HTTP、工具过滤和 HTTP OAuth 登录；接入 MCP App UI 是另一个显式开关。[Connect MCP servers](https://docs.openclaw.ai/tools/mcp) | 发现工具、授权调用、写入业务系统与允许 UI 执行不能互相替代。 |
| 审批与继续 | 请求包含固定执行计划；操作员审批后在原工具调用返回结果，取消／过期使授权失效。Control UI、macOS 和渠道可展示审批。[Exec approvals](https://docs.openclaw.ai/tools/exec-approvals) | 不能只比较“出现确认按钮”，还要比较命令／cwd／会话绑定、一次授权重用和取消后迟到批准。 |
| 可复用工作流 | Lobster 有 `needs_approval`／`needs_input`、resume token、保存在状态目录的检查点和继续操作。[Lobster](https://docs.openclaw.ai/tools/lobster) | token 不绑定 OpenClaw 用户／会话；调用权限与 token 保管仍需处理。此能力不能推出任意外部写入 exactly-once。 |
| 记忆与上下文 | 内建 SQLite 记忆可关键词搜索，配置 embedding 后做向量＋关键词混合检索；Honcho、LanceDB 是独立插件路径。[Memory overview](https://docs.openclaw.ai/concepts/memory) | 语义索引依赖 embedding 配置，不能把关键词检索与自动语义记忆混算。 |
| 子 Agent 与恢复 | 有并发／嵌套子任务、权限与取消控制、结果投递和恢复记录。[Sub-agents](https://docs.openclaw.ai/tools/subagents) | 重启后中断子任务通过完成路径结算，父 Agent 决定剩余工作；文档明确不自动重启所有中断执行，恢复上下文不等于重放命令。[恢复与停止](https://docs.openclaw.ai/tools/subagents/operations) |
| 模型／媒体 | 有凭据轮换和模型 failover；媒体理解支持按 provider／CLI 配置输入图片、音频、视频。[Model failover](https://docs.openclaw.ai/concepts/model-failover)、[Media understanding](https://docs.openclaw.ai/nodes/media-understanding) | 依赖 provider 能力、认证和配置；这次没有真实付费请求或媒体回路验收。 |

## Hermes Agent：技能学习与跨会话记忆的官方能力

| 维度 | 官方资料中可核实的能力 | 限制与比较口径 |
| --- | --- | --- |
| 首用与平台 | 提供 Linux／macOS／WSL2、原生 Windows 安装，以及 desktop 和 Termux 的独立分发；`hermes setup --portal` 是模型和工具凭据的联合设置路径。[官方入口](https://hermes-agent.nousresearch.com/docs/) | Tool Gateway 是付费 Nous Portal 能力；不同发布构建的覆盖范围须逐项确认。 |
| Skills 与学习循环 | Skills 按列表摘要→正文→单个 references 文件渐进读取；`/learn` 可从已完成任务、文档或材料生成／更新 skill；Hub 维护来源、hash、扫描、隔离和审计元数据。[Skills System](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills) | 自动生成／更新是工具与提示提供的工作流，不能据此断言模型每次都会学对。外部 skills 目录可写时会被原位修改，需权限治理。 |
| 长期记忆 | MEMORY.md／USER.md 作为有界跨会话资料，另有 `session_search` 检索历史；新会话加载记忆快照，外部深层记忆是插件路径。[Persistent Memory](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory) | 同一个 gateway 会话可跨重启延续；官方建议自然边界 `/new`。模型口头说“记住”不能代替实际调用 memory 工具和文件落盘。 |
| 浏览器与 Web | local Chromium／CDP 和多个 cloud backend 均有页面导航、输入、截图与视觉分析路径。[Browser Automation](https://hermes-agent.nousresearch.com/docs/user-guide/features/browser) | 云端成本、浏览器依赖、登录／2FA和无人值守可用性分别处理；文档列出某些后端操作／下载限制，不是每个 backend 功能等价。 |
| MCP | 支持本地进程与 HTTP、OAuth 2.1／PKCE／refresh，工具过滤、超时、可选懒连接和 stdio 进程回收。[MCP](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp) | 某些供应商不支持动态客户端注册，需预注册 OAuth client；成功列工具不能证明实际授权调用已成功。 |
| 交互审批 | smart／manual／off 模式、超时、渠道审批和无人值守策略均有合同；过期一次授权不能直接恢复旧调用。[Security](https://hermes-agent.nousresearch.com/docs/user-guide/security) | 安全默认与平台可回答能力要比较；API 对可回答审批的 run 与普通无人值守请求作区别。 |
| 子 Agent | 独立上下文、终端、继承受限工具，后台并发、嵌套控制、取消和完成投递。[Subagent Delegation](https://hermes-agent.nousresearch.com/docs/user-guide/features/delegation) | 官方明确：进程重启不继续正在运行的 child，效果变 unknown；完成但未投递的结果可恢复。这是持久结果投递，不能称为任意任务无缝 durable execution。 |
| 多模态 | CLI 图像粘贴／vision 与 CLI、Telegram、Discord 的语音、TTS、Discord VC 均有配置路径。[Vision](https://hermes-agent.nousresearch.com/docs/user-guide/features/vision)、[Voice Mode](https://hermes-agent.nousresearch.com/docs/user-guide/features/voice-mode) | STT/TTS、硬件／系统包和渠道支持分别依赖；Intel macOS 与 Windows ARM64 的本地 Faster-Whisper 有平台限制。 |

## OpenHands：软件 Agent 控制台与 SDK 的官方能力

当前官网将 Agent Canvas、Software Agent SDK／Agent Server、Automation Server 和 Sandbox Server 分开说明。Cloud 的托管执行、组织协作、预算管理与 Enterprise 能力是商业服务范围；旧 Local GUI 已 deprecated。比较本地可实现能力时，采用相应 OSS 组件的具体合同。[官方组件地图](https://docs.openhands.dev/overview/introduction)

| 维度 | 官方资料中可核实的能力 | 限制与比较口径 |
| --- | --- | --- |
| 控制台与部署 | Canvas 管理对话、文件、终端、模型和后端；可以用本机、Docker、VM 或远程后端，模型可为 provider Key、兼容 endpoint 或 ACP agent。[Agent Canvas](https://docs.openhands.dev/openhands/usage/agent-canvas/overview) | “browser client” 是用户控制台，不自动等于 Agent 具备网页操作；npm 本地后端可访问主机文件，Docker 边界取决于挂载。 |
| 网页操作 | SDK `BrowserToolSet` 基于 browser-use，支持导航、点击、表单和提取，可与终端／文件工具组合。[Browser Use](https://docs.openhands.dev/sdk/guides/agent-browser-use) | 这是已文档化的 SDK 集成与示例，本次没有运行它。 |
| Skills | 支持 AGENTS.md、关键词／路径触发，以及 AgentSkills SKILL.md 的摘要目录／模型按需读取；生命周期 API 有 install、update、enable、disable、uninstall。[Agent Skills & Context](https://docs.openhands.dev/sdk/guides/skill) | 旧 `trigger=None` 格式正文留在上下文，不能按渐进加载计算成本；路径规则对 local 与 ACP conversation 的行为不同。 |
| MCP | 本地 command/args 示例、工具过滤、HTTP OAuth 与 token refresh 有官方 SDK 接线。[MCP guide](https://docs.openhands.dev/sdk/guides/mcp) | 文档明确首次 OAuth 要人类浏览器交互，不适用于完全无人值守初始化；需区分凭据准备与后续任务。 |
| 审批后继续 | AlwaysConfirm／NeverConfirm／ConfirmRisky 策略，WAITING_FOR_CONFIRMATION 状态、拒绝反馈和原 conversation 再运行有明确接口。[Security & Action Confirmation](https://docs.openhands.dev/sdk/guides/security) | 风险分析器是配置项；提供一个拒绝状态与提供可继续的批准状态机是两种资格。 |
| 子任务 | `TaskToolSet` 可创建／按 task ID 恢复子 Agent，对话保存后可再次继续。[Task Tool Set](https://docs.openhands.dev/sdk/guides/task-tool-set) | 该工具被明确定位为同步阻塞、顺序委派；不能把它自动当成全套后台并行调度。 |
| 对话持久化 | 用固定 conversation ID 与 persistence_dir 保存事件、执行状态、工具结果、配置，并在之后恢复再发消息。[Persistence](https://docs.openhands.dev/sdk/guides/convo-persistence) | 对话状态恢复不证明任意工具中断的事务恢复、效果判定或 exactly-once；这些要求需额外故障验收。 |
| 记忆维护 | SDK 有显式启用的用户／项目两层 Markdown 记忆，默认关闭；新对话读索引，提示 Agent 去重、删陈旧事实。[Persistent Memory](https://docs.openhands.dev/sdk/guides/persistent-memory) | 记忆维护仍由模型实际写入承担；不能用提示定义替代成功率／错误记忆率实测。 |

## 用于 JiaClaw 核对的验收问题

以下是从官方合同归纳的比较问题，属于研究推论，不是对 JiaClaw 代码的缺陷认定。

1. 普通用户从空机器安装、获得明确可用模型到第一次完成文件／研究任务，是否只有一条连续且可诊断的路径？真实失败和缺 Key 能否及时报告？
2. Skills 是否只有读取和关键词触发，还是包含来源、版本、安装更新、启停、资源按需读取、生成／修改后的验证与权限撤销？
3. 网页任务能否在真实浏览器观察后点击／填表／验证结果，登录态、下载、取消、截图与授权是否有清楚的所有者？
4. 人类审核是否能在绑定原任务、动作、参数和效果的条件下继续？工具 outcome unknown 与尚未执行的 approval pending 是否分开？
5. MCP 的 transport、OAuth、子进程生命周期、外部写入授权和超时后效果判定是否都已真实接线，而不是只有发现工具或 mock 成功？
6. 子任务是否有资源预算、取消、父子权限、结果保管和重启后结算？恢复对话、恢复结果投递和恢复执行必须分别标注。
7. 长期记忆有没有真实的保存、召回、纠错／删除及新会话使用证据？用户业务任务成功率、干预率、成本与恢复率是否已量化？

## JiaClaw 当前能力与真正的缺口

本节固定当前开发分支 `codex/tool-failure-effects`：本地 head `cc80e85c10bf181b22f0f1dd90343d1fbf21a661`、tree `2df603eb91ec711c6df03005e41c3739dc503c4a`。对应 PR #109 远端 head 为 `f53199024d86d934c2daae511daaed2966cfbd54`，交付记录固定相同 tree。这里比较开发分支已实现的能力，并非宣称这些累计 draft PR 已合并进公开 main 或完成全部生产认证。`origin/main` 本地引用为 `4b2357fcf01f47ba08d7724edbba7accfb60972c`；本次是产品能力研究，不代替原 Standards/Spec 累计代码审查，不声称重新穷尽审查全部实现。[本地路线图](../roadmap.md)、[PR #109](https://github.com/StateKnot/JiaClaw/pull/109)

之前清单中的 copy、stat/tree、受控 Docker exec、SQLite 会话、cron 多任务、Web 工作台、多个文本渠道及统一 outbox 均已有实现；HTTP 只读 MCP、按任务来源模型路由、部分入口真流式和显式语义索引也已接线。剩余工作应区分“缺实现”“应用范围受限”和“已有实现缺真实环境认证”，避免重复建设。各项既有测试与资格只采用路线图指向的固定提交证据，本次没有重新运行这些验收。[路线图能力表](../roadmap.md)

| 用户能力 | 当前事实与证据 | 尚缺的可交付能力 |
|---|---|---|
| 安装后完成第一项真实任务 | 有 init、doctor、源码安装、Docker、四平台候选和校验和安装脚本；README 仍要求手工配置网关、逻辑模型与两类不同 Key。本轮 GitHub Releases API 返回空列表。 | 经审核的公开安装资产、模型/渠道连接引导、凭据与端到端首任务检查、可验证的更新/备份/回滚路径。候选归档 fixture 不能代替真实下载与供应商任务。[README](../../README.md)、[发布候选](../release-candidates.md)、[公开 releases API](https://api.github.com/repos/StateKnot/JiaClaw/releases) |
| 技能生态与经验复用 | 已发现 SKILL.md、列出摘要、手动启用、关键词触发和热重载；启用后的正文加入提示。当前 CLI 技能管理仅列表/重载，仓库示例为 calculator、web_search。 | 模型按需读取技能与参考文件、明确的兼容格式和调用控制、安装/更新/移除/版本固定/回滚、来源与权限审查，以及真实任务产生的可复用技能流程。已有摘要机制不等于完整渐进加载。原始依据：[触发实现](https://github.com/StateKnot/JiaClaw/blob/f53199024d86d934c2daae511daaed2966cfbd54/crates/jiaclaw/src/skills.rs#L296)、[摘要与正文](https://github.com/StateKnot/JiaClaw/blob/f53199024d86d934c2daae511daaed2966cfbd54/crates/jiaclaw/src/lib.rs#L841)、[CLI](https://github.com/StateKnot/JiaClaw/blob/f53199024d86d934c2daae511daaed2966cfbd54/crates/jiaclaw-host/src/main.rs#L201) |
| 浏览器办事 | Brave web_search 与 web_fetch 已注册，不能再归类为没有联网搜索。[实际工具注册](https://github.com/StateKnot/JiaClaw/blob/f53199024d86d934c2daae511daaed2966cfbd54/crates/jiaclaw/src/lib.rs#L312) | Agent 侧浏览器会话、动态页面读取、点击/输入、登录态隔离、下载和截图；最终提交、发送等动作与审批联动。CI 的 Chromium 是 Web 页面测试工具，不能算 Agent 浏览器能力。 |
| 持续记忆与长上下文 | 有 MEMORY/SOUL/USER、关键词检索、显式 embedding + SQLite 余弦检索；语义索引不自动提取聊天或刷新。会话有按消息条数的硬截断/可选摘要，摘要默认关闭。[语义记忆](../semantic-memory.md)、[会话压缩](https://github.com/StateKnot/JiaClaw/blob/f53199024d86d934c2daae511daaed2966cfbd54/crates/jiaclaw/src/session.rs#L113)、[默认配置](https://github.com/StateKnot/JiaClaw/blob/f53199024d86d934c2daae511daaed2966cfbd54/crates/jiaclaw-core/src/lib.rs#L473) | 用户可控的偏好/经验积累、出处/纠错/删除、索引新鲜度维护、更多文档来源和真实检索质量评估；按模型上下文窗口管理 token、工具输出与任务摘要，并验证长任务不丢目标/授权/待办。 |
| 子 Agent 与长任务 | SQLite 历史、模型调用收据和持久 hold 已有；StateKnot durable driver/admission/store、子任务仍待接线/认证。[路线图](../roadmap.md)、[模型收据边界](../model-calls.md) | 真实子任务身份、独立工具权限/预算/并发、取消传播、结果汇总与任务树；进程重启后恢复原任务而不重复未知写入。保存聊天或模型收据不等于恢复工具图。 |
| 审批后继续原任务 | 有逐次工具选择、白名单、整批预检、未知效果停止与人工核对；这些是必要的现有边界。[工具失败合同](../tool-failure-effects.md)、[Web 工具选择](../../crates/jiaclaw-host/ui/index.html) | 执行前展示具体动作与目标，允许/拒绝后继续原任务；跨断线/重启保留审批身份，撤销可生效。未知效果的核对流程与尚未执行动作的审批必须分别建模，不能把“点继续”实现为盲目重放。 |
| 外部工具与业务系统连接 | 已锁定 StateKnot alpha.1 HTTP 只读 MCP；不支持所有传输或服务器，管理员明确批准描述与工具。[MCP 当前范围](../mcp.md) | stdio、有界进程生命周期、OAuth/每用户凭据、更多协议交互和经审批的外部写工具；邮件/日历/文档等用户工作流的真实集成。应用接线缺失不能上报成框架缺陷。 |
| 多模态与文档工作流 | 当前主合同为文本；多模态仍开放。[路线图](../roadmap.md) | 图片/PDF/语音的受限导入、结构化提取与模型输入；语音/图片输出、私有媒体生命周期和授权。能够在 exec 中运行某个脚本不等于已有面向用户的文档能力。 |
| 多用户功能一致性 | 一用户一后端/工作区/数据库与 Key 管理已有；独立用户 HTTP 明确只准入 datetime_now、json_query，技能为空；standalone 的文件/exec/MCP 不能外推到租户。[用户任务合同](../tenant-http-turns.md)、[网关部署范围](../gateway.md) | 在原隔离与额度合同下逐项开放文件、技能、检索等真实任务。独立用户钉钉仍是应用缺口，WhatsApp 资格与实现待核实；已有渠道的真实安装与端点联调也需独立通过。 |
| 可观察的任务工作台与运行质量 | Web 已有聊天、会话、调度和发件箱；模型有来源路由、输出限制及网关额度。不能说没有管理 UI 或预算。[工作台](../../crates/jiaclaw-host/ui/index.html)、[模型路由](../model-routing.md) | 用户可读的步骤/子任务/审批时间线、可打开的交付物、技能/记忆管理，以及跨模型/工具/子任务的任务级耗时和成本归因；真实任务成功率、人工介入次数与用户试用证据。 |

### Claude Code 提供的补充参照

Claude Code 官方文档提供三个具体参照：技能正文仅在使用时加载，可控制调用并带参考文件；子 Agent 有各自上下文、工具与权限；权限规则支持 allow/ask/deny。这些可作为 JiaClaw 技能加载、委派和审批体验的目标，并不要求复制全部产品形态。[Skills](https://code.claude.com/docs/en/skills)、[Subagents](https://code.claude.com/docs/en/sub-agents)、[Permissions](https://code.claude.com/docs/en/permissions)

其自动记忆会记录偏好与纠正，因此“有向量库”本身不是记忆产品完成标准；文件型记忆也能形成用户可理解的积累与编辑流程。另需保留竞品边界：Claude Code checkpoint 主要跟踪直接文件编辑，不恢复 Bash 修改及多数子 Agent 编辑，更不是外部业务写入的通用事务回滚。[Memory](https://code.claude.com/docs/en/memory)、[Checkpointing 的限制](https://code.claude.com/docs/en/checkpointing#limitations)

### 生产级推进顺序与验收

以下是根据用户收益与当前依赖提出的优先级，不是已执行的功能改动，也没有擅自重写回访任务。

1. **先形成真实首用闭环。** 完成当前交付自身资格，选一个代表性真实模型与一个已有国内渠道，核对从干净环境安装、诊断、配置、执行到查看结果。公开发布仍由用户审核；付费模型和真实渠道只使用明确授权凭据。验证升级失败保留旧版本、备份恢复保持身份与存储边界。[当前首用入口](../../README.md)、[候选与安装资格边界](../release-candidates.md)
2. **补强已有技能与记忆。** 先支持来源固定、权限明确的技能按需读取和少量经过实际任务验证的技能，再做受控的经验沉淀；保留原始来源、用户编辑/忘记入口和恢复旧版本的能力。技能内容与记忆内容不获得隐含执行权限。参考上面的 Claude Code/Hermes 官方材料与 JiaClaw 实际加载路径。
3. **增加浏览器与一个完整外部工作流。** 在独立浏览器身份、网络/下载边界、资源预算、取消和动作审批具备后，完成动态网页采集或登录后只读检索；有外部写入的最终提交须满足审批与持久核对合同。基于已认证 SDK/工具集成，优先消费框架现成能力，避免另造协议和 runtime。[现有 HTTP MCP 边界](../mcp.md)，参照前文 OpenClaw 官方 Browser 合同
4. **接 durable，再交付审批恢复与子 Agent。** 持久记录原任务/子任务/技能版本/审批/预算/结果身份；真实 SIGKILL、取消、额度不足及未知写入矩阵通过后再开放自动恢复。上游若出现可消费的固定合同应立即提高此项优先级；不要将本项生产门槛降成内存中的多 Future 并发。[StateKnot 集成记录](../stateknot-gaps.md)、[Brokerrouter 合同记录](../brokerrouter-gaps.md)
5. **按目标场景扩展租户、多模态和渠道。** 若目标用户以小团队为主，优先租户工具范围；若以个人为主，优先图片/PDF/语音输入。先认证已实现国内渠道，再评估新增平台的主体、地区和权限资格。[租户范围](../tenant-http-turns.md)、[多模态与渠道剩余项](../roadmap.md)

### 业务方向与真实验收任务

建议延续“个人和小团队自托管、国内渠道可用、任务有据可查且异常可接管”的定位假设。现有进程隔离、持久收据、统一 outbox 与国内渠道方向与该假设相符；它尚没有真实试用、付费意愿或留存数据，不能据此宣布已经形成市场优势。[原架构与业务审查](2026-10-10-architecture-code-product-review.md)

| 代表任务 | 用户得到什么 | 应实际记录的证据 |
|---|---|---|
| 本地资料整理成可交付文档 | 找到指定来源、去重/归纳、产出可打开文件 | 来源准确率、成品是否完成、权限越界拒绝、模型/工具成本、人工纠正次数 |
| 定时收集公开信息并发送到已授权飞书/企业微信 | 定时获得有来源的摘要，而非仅有一次模型回复 | 原任务与投递回执、真实终端收发、断网/重启/取消不重复投递、每次运行成本 |
| 用户重复委托同类任务 | 复用经过确认的技能与偏好，减少重复配置 | 首次/后续完成步骤、技能版本与加载记录、记忆来源/编辑/删除、成功率和实际耗时 |

先用同样输入、同样授权范围与同类模型预算跑这些任务，再比较产品体验。完成率、费用、耗时与介入次数在本次均未测量；本报告没有给出虚构分数或对竞品稳定性的排名。没有当前证据要求为此引入微服务、分布式数据库或把所有终端/渠道一次性补齐。[原规模与架构判断](2026-10-10-architecture-code-product-review.md)

### 本轮证据限制

本次执行源码/文档检查和官方网页/API 研究，没有调用付费模型、发送真实渠道消息、安装或运行竞品，也没有重新构建/测试 JiaClaw。官方文档的支持声明与真实供应商质量、所有部署平台兼容及任务效果是不同证据等级。

2026-10-10 16:05 UTC GitHub API 复核：JiaClaw releases 返回 `[]`；StateKnot #140 仍 open、0 comments。Brokerrouter #31/#41/PR #40 在本轮连接返回 404，仅能记录当前不可见，不能推断删除、关闭、合并或合同已经解决；仓库已有记录作为历史依赖证据单独保留。本次未创建重复上游 issue。[Releases](https://api.github.com/repos/StateKnot/JiaClaw/releases)、[StateKnot #140](https://github.com/StateKnot/StateKnot/issues/140)、[Brokerrouter 历史证据](../brokerrouter-gaps.md)
