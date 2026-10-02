# 可靠渠道接入与消息恢复

Telegram、Slack、Discord、飞书、企业微信和钉钉共用 SQLite 收件箱、受监督的 Agent worker 及持久化发件箱。合法事件先入库再返回 webhook ACK；模型调用、会话提交和向平台发送消息在后台执行。在去重保留期内，平台重投同一事件不会再次运行 Agent。现有安装必须补齐下面的安装身份、发送者、会话和工具白名单；只配置旧平台密钥的服务会启动失败，需要先迁移配置。

## 部署与身份

渠道要求 `http.persist = true`、非空 API Token 和工作空间之外的 SQLite 路径。模型提供商必须是 `brokerrouter`，离线验收可显式使用 `stub`。一个进程每个平台最多配置一个安装，最多六个安装；SQLite 沿用独占进程锁，不支持多实例共享同一个文件。

下面是同时配置三个平台的结构。ID 必须替换为实际安装及获准使用人的平台 ID；只启用所需的 `[[http.channels]]` 项。

```toml
[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"
shutdown_timeout_secs = 30

[[http.channels]]
channel = "telegram"
installation_id = "123456789"
allowed_senders = ["987654321"]
allowed_conversations = ["-1001234567890"]
enabled_tools = ["datetime_now", "json_query"]
timeout_secs = 120

[[http.channels]]
channel = "slack"
installation_id = "T0123456789"
app_id = "A0123456789"
allowed_senders = ["U0123456789"]
allowed_conversations = ["C0123456789"]
enabled_tools = ["datetime_now", "json_query"]

[[http.channels]]
channel = "discord"
installation_id = "123456789012345678"
allowed_senders = ["234567890123456789"]
allowed_conversations = ["345678901234567890"]
enabled_tools = ["datetime_now", "json_query"]
```

通过服务的密钥管理配置环境变量：

| 环境变量 | 用途 |
|---|---|
| `JIACLAW_API_TOKEN` | 管理 API 的 Bearer Token |
| `JIACLAW_TELEGRAM_SECRET` | webhook 的 `X-Telegram-Bot-Api-Secret-Token` |
| `JIACLAW_TELEGRAM_BOT_TOKEN` | Bot API 发送凭证；数字前缀必须与 installation_id 相同 |
| `JIACLAW_SLACK_SIGNING_SECRET` | 入站请求原始 body 的 HMAC 验签 |
| `JIACLAW_SLACK_BOT_TOKEN` | 已安装到指定 Slack workspace 的 Bot Token |
| `JIACLAW_DISCORD_PUBLIC_KEY` | 应用的 32 字节 Ed25519 公钥，编码为 64 个十六进制字符 |
| `JIACLAW_CHANNEL_STATE_KEY` | Discord 持久化交互凭证的 AES-256-GCM 密钥，编码为 64 个十六进制字符 |

除加密密钥外，平台密钥也可沿用 `http.telegram_secret`、`telegram_bot_token`、`slack_signing_secret`、`slack_bot_token`、`discord_public_key` 配置；对应环境变量优先。Discord 使用交互 webhook token 回复，不需要 Bot Token。加密密钥应在首次部署时生成并通过受保护的密钥存储长期保存；重启时必须保持一致。数据库备份与密钥需分别保护，当前没有在线轮换或批量重加密接口。

`allowed_senders` 和 `allowed_conversations` 必须各有 1–100 个不重复的精确 ID，不支持通配符。`enabled_tools` 必须有 1–32 个已注册工具；当前允许 `datetime_now`、`json_query`、受控容器 `exec` / `shell_exec` 及经过审核的 `mcp_` 工具。旧文件和 HTTP 工具缺少可靠的取消边界，不能进入后台渠道任务。自动 skill 选择关闭。`timeout_secs` 为 1–600 秒，默认 120 秒，覆盖等待会话锁、准备消息和 Agent 调用；取消本地等待不能证明外部副作用没有发生。

入站文本最多 32 KiB。会话身份由平台、安装、会话、线程和发送者共同确定，不将同一群聊里所有人的历史混在一起。这是会话隔离和显式工具授权，底层工作空间仍由服务配置共享，不等同于每用户操作系统隔离或独立 API Key。

事件保存接收时的授权快照，执行 Agent 和实际发送前都会重新检查当前安装及白名单。重启时撤销的安装、发送者、会话或工具授权不会被旧队列绕过。

正式 webhook 应通过公开 HTTPS 反向代理接入下列路径，并保留签名头、查询参数与原始请求体。管理 API 只对可信网络开放并使用 API Token。`local_test_api_base` 仅供测试覆盖发送端点，必须是显式字面 loopback HTTP 地址；生产配置应省略。

## 平台接线

| 平台 | 路径与必要字段 | 回复位置 |
|---|---|---|
| Telegram | `POST /hooks/telegram`；update_id、message.from.id、message.chat.id、message.text | 原 chat；携带 message_thread_id 时回原 topic |
| Slack | `POST /hooks/slack`；event_id、team_id、api_app_id、event.user、channel、text、ts | thread_ts 指定的线程；没有时回复到原消息 ts 的线程 |
| Discord | `POST /hooks/discord`；interaction id、application_id、channel_id、member.user.id 或 user.id；应用命令带一个字符串 prompt option | 首片编辑交互原始回复，后片发送 follow-up |
| 飞书 | `POST /hooks/feishu`；schema 2.0、app_id、tenant_key、message_id、open_id、chat_id | 原会话；群组回复使用 `om_` 根消息 ID |
| 企业微信 | `GET/POST /hooks/wecom`；查询签名、加密 XML 内 CorpID、AgentID、FromUserName、MsgId | 同一成员的应用私聊，无线程 |
| 钉钉 | `POST /hooks/dingtalk`；timestamp/sign、robotCode、chatbotCorpId、senderCorpId、senderStaffId、msgId；仅 conversationType=1 的 text.content | 精确成员 UserID 的机器人单聊，无线程 |

Telegram 使用 webhook secret 校验，并忽略 bot 消息和无文本事件。Slack 校验原始 body 的签名及 ±5 分钟时间窗，同时将 team_id、api_app_id 与配置绑定；bot、subtype 和非 message 事件不进入 Agent。URL verification 的 challenge 也必须通过签名校验。

Discord 校验 Ed25519 签名与 ±5 分钟时间窗，校验 application_id、用户与会话后才接受应用命令；PING 返回 type 1，已入库命令立即返回 type 5。当前只接受一个 type 3 字符串选项，不提供任意嵌套命令解析。平台要求在 3 秒内响应交互，并将交互 token 的使用期限制为 15 分钟；JiaClaw 从 interaction snowflake ID 解出实际创建时间，保守在创建后 14 分钟停止新发送并清理凭证字段。Agent 的执行期限同时受此到期时间限制，并预留 10 秒发送时间，重新投递不会延长旧 token 的寿命。[Discord 官方交互文档](https://docs.discord.com/developers/interactions/receiving-and-responding)

Telegram、Slack 和飞书成功接收返回：

```json
{"ok":true,"event_id":"本地事件 UUID","duplicate":false}
```

企业微信合法文本在持久入库后返回 HTTP 200 空响应体；GET 验证请求返回解密后的原始 challenge。企业微信不会把内部事件 UUID 或模型结果写入回调 ACK，详细合同见[企业微信指南](wecom.md)。钉钉也在持久入库后返回 HTTP 200 空响应体；数据库操作的 2 秒期限是本地保护，不代表已获证实的平台 ACK 时限或重试承诺。

上述 JSON ACK 渠道的重复投递返回相同本地 UUID 和 `duplicate:true`。同一平台事件 ID 对应的发送者、会话或文本发生变化时返回冲突，不把修改后的内容再次执行。Slack 要求快速确认 Events API 请求；将入库与后台执行分离可以在模型慢请求时仍及时 ACK。[Slack 官方 Events API 文档](https://docs.slack.dev/apis/events-api/)

## 执行、发送和中断

全局最多处理 4 个 Agent 事件，每个会话同一时刻只处理一个。完成一次 Agent 调用时，会话消息、事件终态和所有待发送片段在一个 SQLite 事务中提交；提交失败不会留下只有会话或只有发件箱的半个结果。

发送器按目的地顺序领取片段，同一安装最多一条发送请求在途。为限制突发，安装级最短发送间隔分别为 Telegram 3.1 秒、Slack 1.1 秒、Discord 0.3 秒、飞书 1.1 秒，并持久化到数据库。企业微信使用更保守的 4 秒安装间隔与持久发送预算，详见[企业微信指南](wecom.md)。钉钉同样使用本地 4 秒安装间隔，并在请求完成后延长间隔；这不是平台配额保证。对于有已核实限流合同的渠道，平台返回的有效 429 冷却期会进一步延长基础间隔。SDK 不隐式重试或跟随重定向；每次 HTTP 发送连接超时 2 秒、总超时 10 秒，响应体上限 64 KiB。

回复总量最多 16 KiB UTF-8、总共最多 16 片。Telegram、Slack、Discord、飞书和钉钉每片最多 2,000 个 UTF-16 单位；企业微信按渲染后的 UTF-8 字节拆分，每片最多 2,048 字节，并将 ASCII `<`、`>` 转为全角字符。Discord 进一步限定最多 6 片，即编辑原始回复加 5 条 follow-up，兼容 user-installed 应用的 follow-up 上限。拆分保留原始 Unicode 文本，不静默截断；超出总量或片数时整批不入发件箱，事件转为 needs_review。Telegram 关闭链接预览且不设置 parse_mode；Slack 关闭 mrkdwn、名称展开和链接/媒体预览，并转义 `&<>` 控制字符；Discord 明确禁止自动 mentions。

平台返回 HTTP 2xx 还不够：Telegram 必须提供正确 chat 的正整数 message_id，Slack 必须提供对应 channel 和合法 ts，Discord 必须提供对应 channel_id 和合法消息 id；飞书核对成功码、消息 ID 与 chat_id，企业微信核对成功码、msgid 和空的失败成员列表；钉钉要求非空 processQueryKey，且失败、过滤和限流成员列表均为空。有效回执落库后才记录 delivered，表示平台接受而非终端已读。相应平台协议见 [Telegram Bot API](https://core.telegram.org/bots/api) 和 [Slack chat.postMessage](https://docs.slack.dev/reference/methods/chat.postMessage/)。

| 记录 | 状态 | 含义及操作 |
|---|---|---|
| 事件 | received | 已持久化且未领取；可在重启后继续处理 |
| 事件 | processing | 已领取，可能已经调用模型或工具 |
| 事件 | completed | 会话与完整发件箱已原子提交；仍须检查投递状态 |
| 事件 | needs_review | 超时、取消、恢复到未完成的 processing、工具/模型错误或授权变化；不自动重跑 |
| 投递 | pending | 已入库但尚未发送 |
| 投递 | submitting | 已保存发送意图，正在等待平台或提交回执 |
| 投递 | delivered | 已校验平台回执，或管理员提供已送达证据 |
| 投递 | retry_wait | 收到有效平台限流拒绝，等待持久化冷却期 |
| 投递 | unknown | 发送是否生效未知；需要平台侧核对，不自动重发 |
| 投递 | permanent_failed | 明确拒绝或达到有限重试上限；需人工处理 |
| 投递 | expired | Discord 凭证过期，停止未发送片段 |
| 投递 | cancelled | 管理员核对后取消余下投递；并不撤回平台已经收到的消息 |

只有可验证的平台限流响应会自动重试，每片最多 5 次尝试；重启不会清除 attempts 或安装冷却期。Slack 使用 Retry-After，Telegram 使用 parameters.retry_after，Discord 使用 retry_after；飞书需要匹配限流码及有效 x-ogw-ratelimit-reset；企业微信和钉钉的 429 当前不自动重试；钉钉的配额/发送频率错误也进入 unknown，不能将错误码猜测为安全重试窗口。无效或缺失的可信冷却期归入 unknown。平台 429 的限流范围不同，当前采取安装范围的保守冷却。[Slack 限流文档](https://docs.slack.dev/apis/web-api/rate-limits/)、[Discord 限流文档](https://docs.discord.com/developers/topics/rate-limits)

断线、发送超时、5xx、重定向、损坏/错目标回执以及重启发现的 submitting 都进入 unknown。同一目的地后续片段不会越过未解决的发送结果。进程被强杀后，processing 进入 needs_review，submitting 进入 unknown；正常关闭先停止领取、等待配置的退出宽限期，然后记录未完成工作。管理员不能把“JiaClaw 没有收到成功响应”解释成“平台没有发送”。本地去重和回执日志不提供跨系统 exactly-once；unknown 没有自动“重试发送”按钮。

SQLite 或后台 worker 的致命错误使渠道健康状态变为 failed，停止领取并拒绝新的 webhook，返回 503。先修复故障、核对已有事件和平台记录，再重启服务；HTTP `/health` 仍可用并不代表后台渠道仍在执行。

## 管理和恢复 API

所有 `/api/channels/*` 接口都要求 `Authorization: Bearer <JIACLAW_API_TOKEN>`。列表直接返回数组，默认 limit=50，范围 1–100，offset 范围 0–10000。

| 方法与路径 | 行为 |
|---|---|
| `GET /api/channels/status` | 查看 running、failed、stopping 或 disabled，以及 max_processing=4 |
| `GET /api/channels/events?limit=50&offset=0` | 最新优先列出入站事件和状态 |
| `GET /api/channels/events/{id}` | 查看事件、目的地、授权快照和会话 ID |
| `GET /api/channels/deliveries?event_id={id}&limit=50&offset=0` | 查看片段、attempts、next_attempt_ms、回执及错误代码；event_id 可省略 |
| `POST /api/channels/deliveries/{id}/resolve` | 提交 `{"action":"delivered","receipt":"平台消息 ID 和核对说明"}`，仅将 unknown 标记为已送达；或 `{"action":"cancel"}` 取消所属事件余下片段 |
| `POST /api/channels/events/{id}/cancel` | 显式核对事件并取消余下投递；不接受仍在执行或发送中的事件 |
| `DELETE /api/channels/events/{id}` | 删除已完成或已核对、且所有投递均已 delivered/cancelled 的事件和投递审计；保留去重墓碑 |

手工 delivered 要求 1–4096 字节证据，操作只修改审计状态，不再调用平台。cancel 不会撤销先前的外部效果。成功的 resolve、cancel 和 DELETE 返回 204。取消或删除事件不会删除该事件的会话历史。TTL 清理会保留仍有待处理、未核查事件或未解决出站消息的会话；管理员显式删除会话也不会取消已持久化的出站计划，停止发送应使用事件 cancel。

当前最多保留 1,000 条事件、10,000 条投递和 10,000 条去重记录。没有自动删除不确定结果来腾出容量的策略；达到上限会拒绝新事件，要求管理员核对后显式清理。领取事件前，存储层为每个执行中的事件保守预留 100 条投递空间，与定时通知共享同一容量预算：四个入站事件和四个带通知的定时运行最多预留 800 条；容量不足时事件留在 received，不运行 Agent，完成事务释放未使用的预留。删除审计时去重墓碑至少再保留 7 天；墓碑过期之后的旧平台重投可能被当作新事件。数据库中的会话、提示词和回复是持久化明文业务数据；仅 Discord 交互 token 单独使用 AES-256-GCM 密文，API 和日志不会返回该密文或原始 token，过期时清除当前行的凭证字段。

## 验收范围

```sh
cargo build --locked -p jiaclaw-host
python3 tests/channels.py target/debug/jiaclaw
```

脚本只使用本地 HTTP fixture、临时 SQLite、一次性平台凭证，以及支持 Ed25519 的 OpenSSL CLI。它覆盖真实二进制入站签名、白名单、快速 ACK 与重投、原生工具闭环、Unicode 分片、Discord follow-up 和整批上限、回执、429 有限重试与重启冷却、错误不泄密、processing/submitting 强杀恢复和无自动重放。它还验证 unknown 第一片会阻塞后续片段，以及数据库写锁延迟下合法的 1 毫秒 Discord 重试间隔不会使 worker 失败。SQLite trigger 只在临时测试库中模拟完成事务故障，以验证会话和发件箱一起回滚、worker failed 和拒绝新任务；产品没有故障注入端点。

这些测试验证本地契约。正式上线仍须在目标安装验证平台回调配置、Bot/App 权限与安装归属、真实消息的线程位置、账号级限流、Discord 交互时限，以及反向代理的延迟和 body 保真。本轮没有使用真实渠道凭证，也没有宣称真实渠道认证完成。模型供应商默认认证、StateKnot durable 恢复与 Brokerrouter 原生输出契约的剩余边界见 [上游能力差距](brokerrouter-gaps.md)。

## 飞书企业自建应用

`POST /hooks/feishu` 已接入同一持久收件箱、发件箱与管理接口，使用应用和租户复合身份、message_id 去重、SHA-256 签名和可选 AES-CBC 回调解密。定时通知通过独立目的地白名单授权。飞书已知限流包含 HTTP 429 和旧版 400，须有匹配错误码及有效 `x-ogw-ratelimit-reset`；其他不确定响应保持 unknown。完整配置、线程语义、token 缓存和真实安装验收见[飞书指南](feishu.md)。

## 企业微信企业自建应用

`GET/POST /hooks/wecom` 使用企业与应用复合身份、查询签名和 AES-CBC 加密 XML，MsgId 去重后持久入库，再异步主动回复同一成员。白名单和定时目的地使用规范小写 UserID，thread_id 必须为空。token 仅在内存缓存；2048 字节分片按发送渲染结果计数；未知结果保留人工核查边界。部署必须使用 JiaClaw 独占发送权的专用应用，持久发送预算与平台接收额度、配置和真实安装验收见[企业微信指南](wecom.md)。

## 钉钉企业内部应用机器人

`POST /hooks/dingtalk` 接入同一持久收件箱、发件箱与管理接口。安装身份为 `robotCode:corpId`，Client ID 单独配置到 `app_id`，Client Secret 通过 `JIACLAW_DINGTALK_APP_SECRET` 或受保护配置提供。白名单和定时目的地使用保留大小写的精确成员 UserID，thread_id 为空；后台主动文本使用固定 OpenAPI 端点，忽略且不保存回调中的 sessionWebhook。

钉钉的 HMAC-SHA256 签名只涵盖毫秒 timestamp 与 Client Secret，不涵盖 JSON 正文；时间窗为 ±1 小时。因此 HTTPS、可信反向代理和回调头保密属于认证边界；不要在代理日志记录 timestamp/sign。仅允许已发布的内部应用、匹配 robotCode 和企业身份、提供成员 UserID 的私聊文本。群消息和其它消息类型不运行 Agent。完整配置、平台额度及需使用真实安装核对的行为见[钉钉指南](dingtalk.md)。
