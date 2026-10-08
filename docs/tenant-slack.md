# 独立用户 Slack 私聊

管理员把专用 Slack App 安装、工作区、Bot、一个人的成员 ID、一个 DM、网关用户及其既定后端永久绑定。每次入站和执行都核验该身份。模型使用这个用户的独立后端、工作区和 SQLite，网关保存私有 inbox/outbox，并与 Web、cron 和 Telegram 共用用户启用状态、hold 和执行容量。默认关闭。

这一切片接受普通成员发给专用 App 的根 DM 文本。普通 rich_text blocks 被忽略，只有已签名 text 进入模型；拒绝线程、群聊、子类型、Bot 发言、文件、附件、Slack Connect、企业组织安装及访客。每人使用专用 App 与独立 signing secret；每个用户仅一个永久绑定、每个 app_id 仅一个终身保留安装（跨工作区也不复用 App），registry 最多 32 个终身绑定。撤销不释放身份，不能改绑或收养旧队列。真实安装、TLS/egress、平台限额及用户终端收发需要单独验收，见[验证记录](validation.md)。

## 配置和部署

先完成[网关部署](gateway.md)，保持各租户独立容器、私有网络和实际限额卷。后端需要 SQLite、自己的 API Token、Brokerrouter（离线验证可明确选择 stub）。在已有 `[http]` 表中加入 `gateway_channel_chat = true`；关闭 standalone channels、旧 inbound hook、HEARTBEAT；scheduler 只能关闭或 gateway_driven。内部执行固定为 channel 模型、`datetime_now` / `json_query`、关闭自动 skills 和 120 秒预算。网关 `request_timeout_seconds` 至少 150。

使用 Slack 内部 workspace App 的 Bot Token，显式授予 `users:read`、`im:read`、`im:history`、`chat:write`，订阅 Bot 事件 `message.im`；不申请文件、管理员或任意频道写入权限。由获准成员与 App 建立 DM，可信管理员核对实际 ID。用户 ID 可以是 U 或 W；team/app/bot/DM 分别以 T/A/B/D 开头，长度 2–64，余下为大写 ASCII 字母或数字。不能使用名字代替 ID。通过官方管理面安装并取得 Secret，禁止将 Token 放到事件 URL、客户端 Key 或日志中。

```sh
jiaclaw gateway slack-bind --config /etc/jiaclaw/gateway.json \
  --user YOUR_USER_UUID --team-id T123 --app-id A123 \
  --bot-user-id U123 --bot-id B123 --sender-id W456 --conversation-id D456
jiaclaw gateway slack-bindings --config /etc/jiaclaw/gateway.json
```

将输出的 binding UUID 放入现有网关配置的顶层 `slack` 数组；示例 [gateway.slack.json.example](../deploy/gateway/gateway.slack.json.example) 仅示范一个已创建绑定，需要替换所有部署路径和身份。基础部署保持关闭，不自动安装 App 或开启后端。Secret 文件必须绝对路径、普通单硬链接文件、0600、32–4096 可见 ASCII，拒绝链接和特殊文件；Bot Token 另限 xoxb-、至多 2048 字节，signing secret 32–256。后端、Telegram 与每个 Slack 凭据必须不同。挂载仅给网关、只读，不能交给租户或由 HTTP 消息选择。

```json
"slack": [{
  "binding_id": "YOUR_BINDING_UUID",
  "bot_token_file": "/run/secrets/alice_slack_bot_token",
  "signing_secret_file": "/run/secrets/alice_slack_signing_secret"
}]
```

启动依次核对 `auth.test` 的工作区/Bot 用户/Bot ID、`bots.info` 的 App ID、`users.info` 的未删除普通成员和同工作区身份、`conversations.info` 的未共享双方 DM 与对应成员，再核对后端 protocol 2 并持久预留同一完整绑定。任何不符、缺字段或错误都拒绝启动，日志不回显平台错误正文；需要排查时由可信管理员在 Slack 管理面核对权限和身份。平台四次只读握手总预算 20 秒，后端握手总预算 10 秒，各 HTTP 请求至多 5 秒。全部成功后才打开本地队列。后端预留可能先于后续启动失败而持久保存，重试须保留同一绑定，不生成新身份绕过预留。[Slack auth.test](https://docs.slack.dev/reference/methods/auth.test/)、[bots.info](https://docs.slack.dev/reference/methods/bots.info/)、[users.info](https://docs.slack.dev/reference/methods/users.info/)、[conversations.info](https://docs.slack.dev/reference/methods/conversations.info/)

生产平台 API 固定 `https://slack.com/api`；不使用代理、重定向或 HTTP 自动重试。`allow_loopback=true` 与字面量 loopback `/api` 仅用于显式本机 fixture，不能作为生产平台出口。HTTPS Events Request URL 必须精确映射至 `POST /hooks/slack/{binding UUID}`，保留原始正文和唯一 `X-Slack-Request-Timestamp` / `X-Slack-Signature`。网关可在 loopback 后由 TLS 反向代理转发；非 loopback bind 还需 `allow_remote_bind`。按[网关指南](gateway.md)限制入口/出口、网络与卷，不把后端 API 暴露给公网。

## 入站、共享权限和持久准入

| 边界 | 合同 |
|---|---|
| 签名 | 原始 bytes 的 v0 HMAC、恒定时间比较、5 分钟时间窗口；拒绝重复头、非规范时间戳及签名，不读取未认证请求的 registry；签名通过后才检查用户和绑定启用 |
| 正文和 ACK | 64 KiB、读取最多 2 秒、text 最多 16 KiB；整体 2.8 秒预算，超时 408、过大 413、总截止 503；只有持久接纳后才成功 ACK |
| 并发 | 入站全局 8 / 每安装 1、阻塞 I/O 全局 8；超时仍在执行的 I/O 保留其槽直到结束，重试同一事件去重；忙或容量满 429，存储失败 503 |
| 身份 | envelope 工作区/App 和唯一 authorization Bot 与绑定相符，消息的人/根 DM/时间戳符合固定合同；同 event_id 内容冲突 409，不覆盖原事件 |
| 模型/发送 | 同用户共享 registry hold 和后端单执行槽；全局槽取配置，每次持久准入重新检查启用、撤销和既定后端；结算等待 I/O 槽至多 5 秒，阻塞任务保留槽至结束；单次 POST 不自动重放 |

Slack 要求 3 秒内确认事件；平台有限重试不能替代持续队列容量和磁盘监控。成功 ACK 不代表模型完成或用户收到。签名 URL verification 仅返回有界 challenge，不创建事件。[Events API](https://docs.slack.dev/apis/events-api/)、[签名验证](https://docs.slack.dev/authentication/verifying-requests-from-slack/)

API Key 轮换、撤销或只读权限与已绑定的 Slack 后台授权独立；只有用户 disable 或 slack-revoke 阻止后续后台准入。撤销不能撤回已发出的模型/平台请求；enable 不清 hold，不复活永久撤销的安装。消息不能选择其他用户、模型、工具、定时通知目的地或文件。

## 状态、恢复与容量

registry 事务升级至 schema 4，保留 schema 1/2/3 的用户、Key 权限、hold、审计及 Telegram 绑定。升级前停机备份，旧二进制不能打开 schema 4。每绑定库位于 registry 同级 `slack/{UUID}.sqlite3`：0700 目录、0600 单链接文件、生命周期排他锁；application ID 与 protocol 2 owner 全身份校验后才打开 SessionStore schema 10。拒绝普通会话库、Telegram 库、其他 owner 和未知版本；owner 不可修改、删除或替换。

| 容量 | 行为与维护 |
|---|---|
| 队列 | 1000 事件、10000 投递、10000 去重 tombstones，最短去重保留 7 天；满额拒绝新增 |
| 网关操作账本 | 16000 条，不自动裁剪；同一事务关联 claim 与运行时 UUIDv7、内部事件/投递 UUID、attempt，插入失败撤销 claim；满额停止执行/发送；已核对事件 purge 级联删除关联，保留去重 |
| 后端请求账本 | 每独立后端 16000 个永久身份记录；request/event 不可再用、completed 不可回退，满额拒绝新执行。当前无删除/重新分配 CLI；接近上限须维护窗口及受审查的容量迁移，不能删除未知记录继续运行 |
| 数据库与 WAL | 主文件约 64 MiB max_page_count；FULL 同步、250 ms busy、256 页 checkpoint、2 MiB journal_size_limit 回收目标；启动拒绝已有超过 128 MiB 的 sidecar。WAL 目标不是硬限额 |
| 出站 | 固定 DM、纯文本、关闭 mrkdwn/链接展开、特殊字符转义；总回复 16 KiB、至多 16 片、每片原始文本至多 2000 UTF-16 单元，再按 Slack 合同转义；每片发送前持久 submitting，成功 channel/ts 回执必须匹配 |

明确 429 加合法 Retry-After 才进入持久冷却，至多 5 次；冷却与约 1.1 秒发送间隔跨重启。未知回执、断连、终态拒绝、第五次限流或结算存储失败都保留 hold；后续片不先越过未知片，不自动重发。[chat.postMessage](https://docs.slack.dev/reference/methods/chat.postMessage/)

后端与网关不能跨库原子提交：模型执行已完成而响应丢失，事件仍需人工核对。进程崩溃时 processing 变 needs_review，submitting 变 unknown，保留原 operation ID。后端 metadata 仅证明其会话事务状态，不能据此推断 Slack 已送达、工具效果可撤销或自动清除 hold。

每绑定数据库逻辑私有，但共享网关总限额卷，**没有逐绑定物理磁盘隔离**。主文件限制不含 WAL、registry、日志或备份，生产要设真实总字节/inode 配额、预留事务余量、监控并离线核对。满盘不删未知记录来恢复运行。停网关并确认后端工作停止后，一致备份 registry、整个 telegram/slack 目录、每个后端状态与 Secrets；恢复旧快照必须核对恢复点后的权限、撤销及外部效果，不能直接放开入口。

## 离线核对和停止重放

所有业务维护命令持 gateway 停机锁，不能在线与 worker 并用。先关闭平台入口、停止并排空网关，再确认后端没有仍在继续的请求，使用原挂载和可信部署身份：

```sh
jiaclaw gateway slack-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind events
jiaclaw gateway slack-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind deliveries --event YOUR_EVENT_UUID
jiaclaw gateway slack-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind operations
jiaclaw gateway slack-resolve --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID \
  --delivery YOUR_DELIVERY_UUID --receipt slack:1700000000.123456
jiaclaw gateway slack-cancel --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway review-clear --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID --note '已核对原请求、平台回执和全部保留队列'
jiaclaw gateway slack-purge --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway slack-revoke --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID
```

inspect 默认 20、limit 1–100；operations offset ≤16000，events/deliveries offset ≤10000。可信管理员查询内容可能包含私密 prompt/reply。operations 使用内部事件 UUID，对应 events 的 `spec.event_id` 才是 Slack Ev ID；原 request UUIDv7 可与 `gateway audit-list` 的 slack_execute/slack_send 关联。resolve 只记录人工核对的合法回执，不发请求、不重跑模型；cancel 放弃未发送结果但不能撤回外部已接受效果；purge 仅清已解决业务内容并保留去重边界。

后端可信管理员可以用其私有 Bearer Token 查询 `GET /internal/channels/slack/requests/{原 request UUIDv7}`。返回 protocol/backend/binding/request/event/session/status，status 为 admitted 或 completed；不返回 prompt、hash、回执正文。此接口和 binding/execute/status 均不在用户网关公开白名单。它只是核对依据，没有 replay 或自动解锁协议。

review-clear 在同一停机锁下检查该用户所有保留的 Telegram/Slack 库，包括已撤销绑定。未核对事件和 submitting/unknown/失败投递阻止清 hold。撤销后移除运行配置，但保留原 Secret/owner/业务库供核对；不能以删配置或换 App 绕过未知工作。应用 hold、操作记录和回复队列不等于 StateKnot durable driver；完整多用户与供应商认证仍待完成。
