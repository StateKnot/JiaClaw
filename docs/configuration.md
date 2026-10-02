# 配置说明

TOML 与 JSON 使用同一契约，顶层必须有 `agent`，其他段覆盖 `agent` 内的同名配置。完整可解析示例：[`jiaclaw.toml.example`](../config/jiaclaw.toml.example)、[`jiaclaw.json.example`](../config/jiaclaw.json.example)。CLI 不自动读取 `~/.jiaclaw/config.toml`，请明确传 `--config`。

路径不会展开 `~` 或 `$HOME`。`agent.workspace_path` 缺省为用户主目录下 `.jiaclaw/workspace`；指定相对路径时相对于进程工作目录。记忆/身份/心跳路径相对于工作区。状态路径虽然允许 `..`，必须最终位于工作区之外。

## Agent 与模型

| 字段 | 默认 / 行为 |
|---|---|
| `agent.name`, `description`, `system_instructions`, `max_turns` | 配置文件必填；`max_turns` 是保留元数据，实际消息历史上限为 50 条 |
| `agent.workspace_path` | `~/.jiaclaw/workspace` 的展开后的主目录路径 |
| `agent.tool_timeout_secs` | 缺省不限；正整数限制单次工具调用；建议 45 秒 |
| `agent.max_tool_iterations` | 5，限制整个工具循环，生效范围 1–32 |
| `provider.provider_type` | `brokerrouter`；另有 `openai_compatible` 兼容路径与明确的 `stub` 离线模式 |
| `provider.base_url` | 必须配置自己部署的网关；不要假定示例域名是公共服务 |
| `provider.api_key` | 网关虚拟 Key；环境变量 `JIACLAW_API_KEY` 优先 |
| `provider.model` | 网关授权的逻辑模型 ID；工具模型必须满足上游认证矩阵 |
| `provider.temperature`, `max_tokens` | 0.7 / 4096，仍受网关和模型上限约束 |

CLI 对话与 `serve` 对缺失 Key 或未知 provider 启动失败，不隐式切到 stub。Brokerrouter 收费请求带独立 Idempotency-Key，应用没有自动供应商重试或静默降级直连。当前请求操作 ID 尚未持久化；断电或未知提交状态不能自动重跑工具轮次。

## 按用途选择模型

顶层 `[routing]` 默认空，仅非空时要求 Brokerrouter。可分别配置 `chat`、`channel`、`scheduled`、`heartbeat`、`summary`；每项必填 `model`，可选 `temperature` / `max_tokens`。未知用途或字段启动失败；model 为 1–200 UTF-8 字节、无首尾空白和控制字符，温度为有限的 0–2，输出上限为正整数且不超过 `provider.max_tokens`。启用路由时 provider 输出上限为 1–1,000,000。未配置用途继承 provider。

摘要默认温度 0.2，始终限制为 `min(512, provider.max_tokens, summary 的有效 max_tokens)`，不携带工具。输出上限按每次补全计算，不是整个轮次的 token/人民币预算。用途由可信入口决定，HTTP/job 正文、提示词和工具结果不能改路由；一次工具循环固定选择，错误不会触发应用自动换模型。配置修改后重启。启用路由时 ChatResponse 可返回有效参数；持久审计范围和网关降级边界见[模型路由指南](model-routing.md)。

## HTTP、安全与存储

| 字段 | 默认 / 行为 |
|---|---|
| `http.bind` | `127.0.0.1:8080`；必须 IP:端口；`--bind` 覆盖；端口 0 可用于测试 |
| `http.api_token` | `JIACLAW_API_TOKEN` 优先；支持 `Authorization: Bearer` 或 `X-Api-Token` |
| `http.persist` | **true**；false 是明确的易失模式，不保存历史 |
| `http.persist_path` | `../state/sessions.sqlite3`；不得位于 Agent 工作区内 |
| `http.rate_limit_per_minute` | 缺省不限；正整数为全进程 API/渠道共享额度；超限 429 |
| `http.max_body_bytes` | 1048576；超限 413；不提供文件上传 |
| `http.session_ttl_secs` | 缺省不删除；正整数启用 30 秒扫描；活跃对话豁免 |
| `http.shutdown_timeout_secs` | 15；建议 30；SIGINT/SIGTERM 停止接入并等待正在处理的请求 |
| `http.metrics_public` | true；生产示例设置 false，避免公开进程信息 |
| `http.cors.enabled` | false；内置工作台同源，不需要 CORS |
| `http.cors.allowed_origins` | []；跨源使用精确来源；`*` 仅显式配置时生效 |
| `http.cors.allowed_methods`, `allowed_headers`, `expose_headers`, `max_age_secs` | 参见示例；允许的来源不会替代 API 鉴权 |

非 loopback 监听以及启用 exec/MCP 均必须配置 API Token。公网部署仍需 TLS 反向代理。`/health` 公开，`/metrics` 可鉴权，工作台静态资源公开但不包含密钥或历史。API Token 是个人实例的一把共享钥匙，目前没有多用户权限隔离。

SQLite 使用 WAL、FULL 同步与单进程所有权锁。API 完成响应前提交历史；数据库写入失败返回 `session_storage_error`。同一会话的 chat/delete/import 通过同一锁串行执行。旧 `.jiaclaw/sessions.json` 默认自动迁移到工作区相邻的 state；指定其他 `.json` 路径时迁移到同名 `.sqlite3`。迁移在事务内完成、源文件保留、坏 JSON 中止启动；已导入源不会在重启后恢复已删除会话。

## 工具

`tools.copy.enabled` 默认 true，同时控制 `copy` 和 `file_copy`。参数为 `from/to` 或 `source/destination`，`overwrite` 默认 false；只复制不超过 64 MiB 的常规文件。父目录须存在，不复制目录，不保留权限/时间戳，目标文件在 Unix 创建为 0600。禁止绝对路径、`..`、符号链接和特殊文件；提交时原子发布，无覆盖模式能处理并发竞争。依赖文件系统支持 hard link / rename / fsync，错误会明确返回。

`tools.exec` 字段如下；缺省关闭，错误配置中止工具注册：

| 字段 | 约束 |
|---|---|
| `enabled` | 默认 false；启用需要预先验证 Docker daemon |
| `docker_path` | 已安装 CLI 的绝对路径；默认 `/usr/bin/docker` |
| `image` | 必须 `image@sha256:64位摘要`，预先拉取；运行时 `--pull=never` |
| `commands` | 1–32 个名字→容器内绝对可执行路径；名字仅字母数字/下划线/短横线 |
| `timeout_secs` | 30；1–300，覆盖创建与运行，清理另外有 10 秒期限 |
| `max_output_bytes` | 65536；stdout/stderr 分别限制；1–1048576；超额继续排空并标记截断 |
| `workspace_read_only` | true；显式 false 才允许沙箱写工作区 |
| `user` | `65534:65534`；非零数字 UID:GID；目录权限必须允许该用户读取/写入 |

调用格式：`{"command":"cat","args":["README.md"]}`。最多 128 个字符串参数，总长 32 KiB。参数逐个传递，模型不能指定镜像、Docker 参数、环境变量或宿主机路径。`shell_exec` 只是同一工具的兼容名字，不接受任意 shell 字符串。允许 `sh`、Python 等解释器等于授权容器内任意代码，仍受沙箱约束，请按实际任务配置白名单。

容器无网络、只读根文件系统、移除 capabilities、no-new-privileges、非 root、限制内存/CPU/PID，只有 `/workspace` 与受限 `/tmp`。超时和 future 取消会请求删除整个容器；daemon 不可达时不能声称清理成功，需按日志检查。宿主服务被 SIGKILL/断电时无法执行取消清理，这是容器 exec 生命周期的已知边界。服务宿主接入 Docker daemon 是高权限操作，容器部署示例因此禁用 exec。

其他 `tools.<name>.enabled` 默认 true：`read_file`, `list_dir`, `write_file`, `delete_file`, `str_replace`, `grep`, `glob`, `mkdir`, `move`, `memory_search`, `memory_write`, `web_search`, `web_fetch`。`web_search.brave_api_key` 可由 `JIACLAW_BRAVE_API_KEY` 覆盖；未配置调用报错。`web_fetch.allow_private` 默认 false。`tools.memory_write.enabled` 同时控制 `memory_write` 与 `memory_append`；通用文件写工具的权限需另外限制。记忆检索目前按关键词，不是向量检索，文件/查询/结果上限与提交边界见[记忆文件指南](memory-files.md)。

## MCP

`mcp.servers` 默认空，不访问任何远程服务器。使用 StateKnot `0.1.0-alpha.1` 的 HTTP client；完整字段、审查与指纹生成流程见 [MCP 配置](mcp.md)。每个工具必须显式分类 `effect = "read_only"` 并固定完整描述的 SHA-256。服务启动前验证本地策略；任何已批准工具缺失、描述变更或 schema 无效都会中止启动，避免静默丢失能力。更新配置/凭证/工具描述后重启；没有自动接受新版描述。

## 渠道、记忆、技能与心跳

Telegram、Slack、Discord、飞书、企业微信和钉钉必须配置 `http.channels` 安装/身份/工具白名单及平台鉴权后开放；未启用返回 404。旧密钥配置缺少策略时启动失败。API Token 不替代渠道鉴权。

| 端点 | 入站配置 / 环境变量 | 出站配置 / 环境变量 |
|---|---|---|
| `/hooks/inbound` | `http.webhook_secret` / `JIACLAW_WEBHOOK_SECRET`，头 `X-Webhook-Secret` | 同步 JSON 回复 |
| `/hooks/telegram` | `http.telegram_secret` / `JIACLAW_TELEGRAM_SECRET`，头 `X-Telegram-Bot-Api-Secret-Token` | `telegram_bot_token` / `JIACLAW_TELEGRAM_BOT_TOKEN` |
| `/hooks/slack` | `http.slack_signing_secret` / `JIACLAW_SLACK_SIGNING_SECRET`，v0 HMAC-SHA256 | `slack_bot_token` / `JIACLAW_SLACK_BOT_TOKEN` |
| `/hooks/discord` | `http.discord_public_key` / `JIACLAW_DISCORD_PUBLIC_KEY`，Ed25519 | interaction token；`JIACLAW_CHANNEL_STATE_KEY` 加密存储 |
| `POST /hooks/feishu` | `http.feishu_encrypt_key` / `JIACLAW_FEISHU_ENCRYPT_KEY` 和 `http.feishu_verification_token` / `JIACLAW_FEISHU_VERIFICATION_TOKEN`，签名与可选解密 | `http.feishu_app_secret` / `JIACLAW_FEISHU_APP_SECRET` |
| `GET/POST /hooks/wecom` | `http.wecom_callback_token` / `JIACLAW_WECOM_CALLBACK_TOKEN` 和 `http.wecom_encoding_aes_key` / `JIACLAW_WECOM_ENCODING_AES_KEY`，查询签名与 AES-CBC 解密 | `http.wecom_app_secret` / `JIACLAW_WECOM_APP_SECRET` |
| `POST /hooks/dingtalk` | `http.dingtalk_app_secret` / `JIACLAW_DINGTALK_APP_SECRET`，毫秒 timestamp 与 HMAC-SHA256 sign 请求头 | 同一 Client Secret；`http.channels[].app_id` 为独立 Client ID |

六个平台使用统一持久 inbox/outbox 与有界异步发送；fixture 测试覆盖协议，不等于真实渠道联调认证。会话按安装、会话、线程和发送者绑定；工作区仍是实例共享，没有多用户工作区隔离。

`memory.path` 默认 MEMORY.md；`identity.soul_path/user_path` 默认 SOUL.md/USER.md。`memory_read` 按逻辑文件名读取这些配置路径；提示注入、记忆读取和 CLI 展示最多读取 32 KiB，按 UTF-8 边界截断；`memory_write` / `memory_append` / `soul_write` / `user_write` 的最终文件均不得超过 32 KiB。父目录/目标链接与非常规文件被拒绝，提交与并发边界见[记忆文件指南](memory-files.md)。技能从 `workspace/skills/*/SKILL.md` 发现；HTTP `POST /api/skills/reload` 或 Unix SIGHUP 重新加载。

`heartbeat.enabled` 默认 false，`interval_secs` 默认 3600，`path` 默认 HEARTBEAT.md，`session_id` 默认 heartbeat。仅 serve 内运行；空或缺失文件跳过，超过 32 KiB 拒绝本轮且不调用模型；它不是持久化 cron 多任务调度器。

`session.summarize_on_overflow` 默认 false，历史超过 50 条时硬截断；启用则摘要并保留 `keep_recent`（默认 10）。摘要失败退回截断。`logging.format` 为 text/json；`logging.level` 默认 info。

## 环境变量覆盖

正整数类变量无效或为 0 时回退配置/默认值，具体生效范围以字段约束为准：

- `JIACLAW_TOOL_TIMEOUT_SECS`, `JIACLAW_MAX_TOOL_ITERATIONS`
- `JIACLAW_RATE_LIMIT_PER_MINUTE`, `JIACLAW_MAX_BODY_BYTES`, `JIACLAW_SESSION_TTL_SECS`, `JIACLAW_SHUTDOWN_TIMEOUT_SECS`
- `JIACLAW_HEARTBEAT_INTERVAL_SECS`, `JIACLAW_SESSION_SUMMARIZE_ON_OVERFLOW`
- `JIACLAW_METRICS_REQUIRE_AUTH`（true 表示不公开）、`JIACLAW_CORS_ENABLED`, `JIACLAW_CORS_ORIGINS`（逗号分隔）
- `JIACLAW_LOG_FORMAT`；级别优先级 `JIACLAW_LOG_LEVEL` > `RUST_LOG` > `logging.level` > info

旧示例中的 `[runtime]`, `[server]`, `[limits]`, `tools.enabled`, `tools.mcp_servers`, `skills.enabled` 不驱动运行时，已移除。请使用本文实际字段，不依赖被 serde 忽略的配置。

## 持久调度

`[scheduler] enabled = true` 启用 cron/interval 多任务与鉴权管理 API，默认关闭。要求 SQLite、API Token、Brokerrouter 或显式 stub；与 legacy heartbeat 互斥。工具范围、时区、中断处理和配额见[定时任务指南](scheduler.md)。数据库自动事务迁移至 schema v7（保留入站事件和定时运行两种发件来源，支持飞书、企业微信、钉钉通知及独立的企业微信发送额度账本），旧二进制拒绝降级；升级前应按部署指南停机备份。

### 渠道授权与持久消息

`http.channels` 默认为空，渠道关闭。启用 Telegram/Slack/Discord/飞书/企业微信/钉钉时须同时设置安装身份、发送者/会话/后台工具精确白名单，SQLite 和 API Token。旧版本只有平台密钥的配置须按[渠道指南](channels.md)显式迁移，不能依赖同步 JSON reply 或空工具列表放行所有工具。Discord 另需环境变量 `JIACLAW_CHANNEL_STATE_KEY`（32 字节密钥的 64 位十六进制编码）。

Telegram/Slack/飞书/企业微信/钉钉每个安装可额外配置 `scheduled_destinations = [{ conversation_id = "...", thread_id = "..." }]`；thread_id 省略表示只授权会话顶层。该列表默认空，最多 100 个精确且不重复的目的地，独立于入站会话白名单。有通知的任务工具必须同时获得该安装授权；任务 `delivery` 不允许携带凭证或服务端点。详见[定时通知指南](scheduled-delivery.md)。

飞书企业自建应用使用 `feishu_app_secret`、`feishu_encrypt_key`、`feishu_verification_token`（对应 `JIACLAW_FEISHU_APP_SECRET`、`JIACLAW_FEISHU_ENCRYPT_KEY`、`JIACLAW_FEISHU_VERIFICATION_TOKEN` 环境变量优先）。安装身份为 `cli_<app>:<tenant_key>`，不另填 app_id；群组 thread_id 指 `om_` 根消息 ID。配置、签名、token 生命周期和验收范围见[飞书指南](feishu.md)。

企业微信企业自建应用使用 `wecom_app_secret`、`wecom_callback_token`、`wecom_encoding_aes_key`，对应 `JIACLAW_WECOM_APP_SECRET`、`JIACLAW_WECOM_CALLBACK_TOKEN`、`JIACLAW_WECOM_ENCODING_AES_KEY` 环境变量优先；缺省不配置。安装身份为 `CorpID:AgentID`，不另填 app_id。发送者与 conversation_id 都使用小写成员 UserID，thread_id 必须为空。仅接成员与应用的私聊文本及精确定时成员通知；须使用由 JiaClaw 独占发送权的专用应用。回调加密、平台额度、持久预算及真实安装验收见[企业微信指南](wecom.md)。

钉钉仅接企业内部应用机器人的成员私聊文本与精确定时成员通知；`dingtalk_app_secret` 默认未配置，`JIACLAW_DINGTALK_APP_SECRET` 优先。安装身份为 `robotCode:corpId`，`app_id` 必须另填 Client ID；不能假定它与 robotCode 相同。发送者和 conversation_id 都是保留大小写的单个成员 UserID，应用支持 1–64 个 ASCII 字符：首位字母或数字，其余可含 `_-.@`；thread_id 必须为空。启用并发布应用机器人，明确应用可见范围和消息权限。回调身份、可信 HTTPS 边界及真实安装验收见[钉钉指南](dingtalk.md)。

## 独立用户网关

`jiaclaw gateway` 使用独立、严格校验的 JSON 配置与私有身份 SQLite，不读取上述 Agent 配置，也不创建共享 Agent。具体 `serve`、用户/Key 管理命令、限额、TLS/容器/磁盘隔离、未知写入恢复见[网关指南](gateway.md)。现有普通 `serve` 配置仍是一用户一实例。
