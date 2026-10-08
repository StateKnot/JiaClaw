# 独立用户 Discord 私聊

该渠道的私有库原子初始化支持 Linux 和 macOS。一个网关用户永久绑定一个专用 Discord App、公开 Ed25519 验证公钥、Bot 用户、获准人、固定 Bot DM 与全局 `/jiaclaw prompt` 命令 ID。入站、后端执行和回复都检查该身份；模型、工作区和会话仍在此人的独立后端。网关保存私有队列，与 Web、cron、Telegram 和 Slack 共用用户启用状态、hold 及执行容量。默认关闭。

本切片只接受 `USER_INSTALL` 的 `BOT_DM` 文本命令：安装人必须就是调用人和绑定的普通用户，拒绝 guild、其他人的 DM、群聊、Bot/system 用户、组件、自动补全、附件与任意命令。每个用户和 App 仅一个终身预留绑定，最多 32 个，撤销不释放身份。它不扩大已有[单实例 Discord](discord.md)或 Bot 定时通知权限。真实 App 安装、终端可用性、TLS 总延迟和供应商认证尚未完成，验证状态见[记录](validation.md#独立用户-discord-批次)。

## 部署和注册

先完成[网关部署](gateway.md)：独立租户容器、工作区、SQLite、私有网络和真实限额卷；后端自己的 Token 与 Brokerrouter Key。后端在已有 `[http]` 中启用 `gateway_channel_chat = true`，关闭 standalone channels、旧 inbound hook 和 HEARTBEAT；scheduler 关闭或 gateway_driven。内部渠道执行固定 channel 模型、`datetime_now` / `json_query`、关闭自动 skills，运行预算 120 秒；网关 `request_timeout_seconds` 至少 150。

可信管理员在 Discord Developer Portal 创建专用 App 和 Bot，**仅启用 User Install**，安装参数仅 `applications.commands`、permissions `"0"`。由获准人完成用户安装并建立与该 Bot 的 DM；管理员核对 App、公钥、Bot、人和实际 DM ID，不根据名字或未认证 HTTP 内容绑定。不启用 guild 安装、Gateway intents 或消息内容读取，也不自动创建 DM 或修改平台配置。

管理员先审阅以下全局命令正文，再通过获授权的管理流程提交到 `POST /applications/{application_id}/commands`。这是可审核的注册材料，JiaClaw 不自动执行平台写入；记录返回的 `id` 作为 `command_id`。应用安装上下文与命令上下文须同时正确。[官方命令合同](https://docs.discord.com/developers/interactions/application-commands)

```json
{
  "name": "jiaclaw",
  "type": 1,
  "description": "向你的个人 JiaClaw 提交文本任务",
  "integration_types": [1],
  "contexts": [1],
  "options": [{
    "name": "prompt",
    "type": 3,
    "description": "任务文本",
    "required": true,
    "min_length": 1,
    "max_length": 4096
  }]
}
```

将实际身份写入网关 registry。所有 snowflake 必须是非零、无前导零的十进制 u64 字符串；公钥必须是非零的 64 个小写十六进制字符。下面占位值须替换为实际 ID 和公开公钥，不填写 Token。

```sh
jiaclaw gateway discord-bind --config /etc/jiaclaw/gateway.json \
  --user YOUR_USER_UUID --application-id YOUR_APP_ID --verify-key PUBLIC_KEY_HEX \
  --bot-user-id YOUR_BOT_USER_ID --sender-id YOUR_HUMAN_ID \
  --conversation-id YOUR_BOT_DM_ID --command-id YOUR_GLOBAL_COMMAND_ID
jiaclaw gateway discord-bindings --config /etc/jiaclaw/gateway.json
```

在现有配置顶层加入 `discord` 数组，使用返回的 binding UUID；完整示例见 [gateway.discord.json.example](../deploy/gateway/gateway.discord.json.example)。`api_base` 默认 `https://discord.com/api/v10`，`allow_loopback` 默认 false。只在显式本机 fixture 中使用字面量 loopback `/api/v10` 与 `allow_loopback=true`，生产出口保持官方 HTTPS；无自动重试、重定向或代理。

```json
"discord": [{
  "binding_id": "YOUR_BINDING_UUID",
  "bot_token_file": "/run/secrets/alice_discord_bot_token",
  "state_key_file": "/run/secrets/alice_discord_state_key"
}]
```

Bot Token 和状态密钥分别保存为绝对路径、普通单硬链接文件，由实际网关运行 UID 持有（参考容器为 10001），设置 0600 并只读挂载；实现拒绝链接、特殊文件和世界权限。Bot Token 为 32–2048 字节有限 ASCII；状态密钥用密码学随机源生成 32 字节，以 64 个小写 hex 字符保存，拒绝全零。凭据不得与其他 Discord、Slack、Telegram 或后端凭据相同，也不放进 URL、日志或租户工作区。

状态密钥使用 AES-256-GCM 加密短期 interaction token，每条随机 96 位 nonce，AAD 固定 binding/App/interaction ID。主库和后端 owner 永久保存非秘密 SHA256 指纹；换密钥拒绝启动，不提供在线轮换或旧记录重新加密。备份必须保护原密钥并与对应状态库关联；密钥丢失无法恢复未过期凭据，不能用新 Key 或删除 owner 绕过。加密只保护 interaction 凭据，prompt、reply、审计和数据库备份仍须按私密业务数据保护。

启动先以 Bot Token 只读验证 `GET /applications/@me`、`GET /users/@me`、`GET /applications/{App}/commands/{command}`：App、公钥、User Install 仅 commands/零权限、Bot 身份和全局命令 ID/名称/文本 option、`integration_types=[1]`、`contexts=[1]` 精确匹配。平台总预算 20 秒，每请求 5 秒。随后打开并验证私有 owner，核对专属后端 protocol 3 并持久预留完整绑定及密钥指纹，后端握手总预算 10 秒。任意不符均拒绝启动，日志不回显平台正文；失败重试保持原身份，不重新绑定绕过已保存预留。

启动时**不要求 App 已保存 Interactions Endpoint URL**。先启动网关与公共 HTTPS 反代，再由管理员在 Portal 设置 URL，Discord 发送签名 PING、收到 PONG 后才保存该 URL，避免 bootstrap 循环。精确映射 `POST /hooks/discord/{binding UUID}`，不带 query；保留原始正文及唯一 `X-Signature-Timestamp` / `X-Signature-Ed25519`，禁止正文重写、自动转发重试和缓存。后端内部 API 不对公网开放；网关非 loopback 监听须显式 `allow_remote_bind`。

Discord 初始响应期限为 3 秒。JiaClaw 本地处理最多 2.8 秒、正文读取最多 2 秒；TLS、代理、排队和网络也属于平台总期限，生产须对实际链路测量余量，不能把本机 guard 通过作为公网延迟认证。[官方交互与响应合同](https://docs.discord.com/developers/interactions/receiving-and-responding)

## 准入和私密回复

| 边界 | JiaClaw 合同 |
|---|---|
| 认证 | 原始 bytes 的 Ed25519 签名、唯一规范头与五分钟时间窗口；认证后才读 registry，拒绝重复关键 JSON 字段 |
| 请求 | 64 KiB 正文、16 KiB prompt；全局 8 / 每绑定 1 入站槽；忙或队列满 429，超大 413、读截止 408、总截止或存储失败 503 |
| ACK | 签名 PING 返回 PONG，不写事件；命令仅在持久准入后返回 `type=5,data.flags=64`，重复相同 interaction 不重复执行，身份或 token/prompt 冲突 409 |
| 授权 | 绑定 App、公钥、USER_INSTALL owner、调用人、Bot DM、固定全局命令；每次 effect 准入重新检查用户/绑定启用、共享 hold、专属后端与容量 |
| 取消和 I/O | 全局 8 个阻塞 I/O 槽，调用者超时或取消仍保留执行中 I/O 的槽至结束；结算等槽至多 5 秒，不能以调用者离开推断写入未发生 |
| 会话 | 固定 `discord:{binding UUID}`；API Key 轮换/只读不撤销独立后台授权，disable 或 discord-revoke 才阻止下一次准入 |

模型结果总计最多 16 KiB、最多六片，每片最多 2000 UTF-16 单元；超过任何界限整条进入人工核对，不部分截断。第一片使用 `PATCH .../messages/@original`，继承已 deferred 的 ephemeral 属性，不尝试修改可见性；其余最多五片使用 followup POST，并逐条显式 `flags=64`。所有片均禁用 mentions，固定原 interaction，不发主动 Bot 消息或换 DM。六片上限适配 User Install 的五条 followup 平台合同。[官方 followup 合同](https://docs.discord.com/developers/interactions/receiving-and-responding#followup-messages)

平台 token 最长有效 15 分钟；应用按 interaction snowflake 的创建时间设保守 **14 分钟**截止，并在准入及调用模型前要求剩余 **165 秒**，覆盖 150 秒后端请求、10 秒发送及 5 秒结算预算。排队耗尽预算会进入 needs_review，不调用模型。模型返回后在结果提交时再次检查发送余量，提交期间耗尽余量也保留 hold。发送受剩余时间约束；到期 pending/retry 持久变 expired，保留 hold，不能因被前序未知片或冷却阻塞而永远跳过清理。[官方 token 期限](https://docs.discord.com/developers/interactions/receiving-and-responding#interaction-callback)

每次发送先原子记录 submitting、UUIDv7 operation 与 attempt；成功回执必须是 JSON 对象，message/channel 必须匹配且 flags 包含 EPHEMERAL。429 同样要求 JSON 对象、明确的 global 布尔值和合法冷却才重试，间隔至少约 300 ms、至多五次，并保留跨重启期限；冷却超出 interaction 余量改 expired。超时、断连、未知回执、终态拒绝、第五次限流或结算失败都保留人工 hold，后续片不越过未知片；没有模型或平台自动重放。

## 存储、备份和停机核对

registry schema 5 保留既有用户、Key 权限、审计、hold 和 Telegram/Slack 绑定。每绑定库在 registry 同级 `discord/{UUID}.sqlite3`：0700 目录、0600 单链接文件、排他生命周期锁。protocol 3 不可变 owner 校验全部永久身份和状态密钥指纹，再接 SessionStore schema 10；拒绝普通会话库、其他渠道/owner、未知版本或密钥替换。

首次 owner 在固定私有 `.sqlite3.initializing` 中以 FULL rollback-journal 事务提交，关闭 SQLite、文件 fsync、同目录 NOREPLACE 发布及目录 fsync。runtime 可继续有界空 stage 或完整相同 owner stage；不完整/WAL header、任何暂存 sidecar、final 缺失时的 final sidecar 均保留并拒绝。维护无配置/密钥时只能继续**已提交**的 owner stage，空或部分 stage 要求人工核对；既有未知/空 final 不收养，既有 final 的遗留 stage 留供离线检查。不承诺所有断电状态自动恢复。

| 资源 | 边界 |
|---|---|
| 主文件 | 64 MiB max_page_count；stage ≤128 KiB；启动 sidecar ≤128 MiB；FULL、250 ms busy、256 页 checkpoint、2 MiB WAL 回收目标，后者不是硬限额 |
| 队列 | 1000 事件、10000 投递、10000 去重 tombstone；至少七天去重，满额拒绝新增 |
| 网关操作 | 16000 条；与 claim 同事务，失败回滚；仅已核对事件 purge 级联清关联 |
| 后端身份 | 16000 个永久 request/event 记录；结果与会话同事务，completed 不回退，无自动删除或重新分配 CLI |

各绑定逻辑私有，但共享网关总限额卷；64 MiB 不包括 WAL、registry、日志或备份。部署须有实际字节/inode 配额、事务余量、监控及一致备份。停入口、停止并排空网关，确认后端无在途请求后，备份 registry、整个 Telegram/Slack/Discord 状态目录、各后端状态及原 Secrets；恢复旧快照须先核对恢复点后的撤销、权限和外部效果。

业务维护必须持网关停机锁。原配置中的 Discord 条目可以移除，撤销绑定仍保留在 registry；`inspect` 和 review-clear 从已保存 owner 的非秘密指纹检查原库，不读取 Bot Token/状态密钥，也不访问 Discord。无库、stage 或 sidecar 的绑定返回空历史，未知残留不得视为无状态。

```sh
jiaclaw gateway discord-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind events
jiaclaw gateway discord-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind deliveries --event YOUR_EVENT_UUID
jiaclaw gateway discord-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind operations
jiaclaw gateway discord-resolve --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID \
  --delivery YOUR_DELIVERY_UUID --receipt discord:YOUR_VERIFIED_MESSAGE_ID
jiaclaw gateway discord-cancel --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway review-clear --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID \
  --confirm-backend-idle --note '已核对原请求、平台回执及全部保留渠道队列'
jiaclaw gateway discord-purge --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway discord-revoke --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID
```

inspect 默认 20 条、limit 1–100；operations offset ≤16000，events/deliveries ≤10000。查询可能包含私密业务正文，但不返回 interaction 凭据。operation 的 event_id 是内部 UUID，events.spec.event_id 才是平台 interaction ID；原 request UUIDv7 可与 `gateway audit-list` 的 discord_execute/discord_send 对应。resolve 仅保存已核对的合法回执，不请求平台或重跑模型；cancel 放弃未发送部分，不能撤回外部效果；purge 仅清已解决事件，仍保留去重。

processing 崩溃变 needs_review，submitting 变 unknown，保留原 request ID。未知、permanent_failed、expired 和未核对事件必须人工停机处置，不能因 token 到期推断之前没有发送；对无已知副作用的到期项也须显式 cancel。review-clear 在同一停机锁下检查该人的所有保留 Telegram/Slack/Discord 队列，包括 revoked 绑定；删配置、换 App、移走库不能作为核对完成的依据。

后端可信管理员可用私有 Bearer 查询 `GET /internal/channels/discord/requests/{原 request UUIDv7}`；仅返回 protocol/backend/binding/request/event/session/status（admitted 或 completed），没有 prompt、hash、回复正文或 replay。后端与网关跨库不能原子提交，metadata 不证明 Discord 已送达，也不自动清 hold。这些应用队列与身份接线不等于 StateKnot durable driver 或工具运行恢复。
