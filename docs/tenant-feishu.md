# 独立用户飞书私聊

管理员把中国飞书企业自建应用、精确租户、机器人、一个人的 `open_id`、既有单聊 `chat_id`、网关用户及其独立后端永久绑定。默认关闭；模型使用该用户的独立工作区和 SQLite，网关保留私有 inbox/outbox、原请求操作身份，并与 Web、cron 和其他独立渠道共用用户启用状态、hold 和执行容量。

每人使用专用 App，每个用户和 App 最多一个终身绑定，registry 最多 32 个终身绑定；撤销不释放身份，也不允许换 owner 收养旧队列。入口仅接收绑定用户的 p2p 文本，不开放群聊、其他机器人、Lark 国际站、商店应用多租户、长连接、卡片、媒体或主动定时通知。真实自建应用发布/可用范围、公开 TLS 回调总延迟、终端收发、平台限额及真实供应商认证尚未完成；本地协议测试不能代替这些验收。

## 平台权限与身份核对

可信企业管理员在[开发者后台](https://open.feishu.cn/app)创建专用企业自建应用，启用并发布机器人能力，使绑定人在应用可用范围内；由该人与机器人建立单聊，再核对真实 ID。只订阅 `im.message.receive_v1`。这条入口需要以下最小应用权限，不需要通讯录、员工 `user_id`、企业域名或群成员权限。

| 用途 | 最小应用权限 | 官方合同 |
|---|---|---|
| 接收用户发给机器人的单聊 | `im:message.p2p_msg:readonly` | [接收消息](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/events/receive) |
| 以机器人身份回复 | `im:message:send_as_bot` | [发送消息](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/create) |
| 读取绑定聊天类型 | `im:chat:read` | [获取群信息](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/chat/get) |
| 核对 token 所属企业 | `tenant:tenant:readonly` | [获取企业信息](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/tenant-v2/tenant/query) |

`GET /bot/v3/info` 本身无需 scope，但要求机器人能力已开启并发布。响应使用顶层 `bot`，`activate_status=2` 表示租户已启用，`open_id` 是机器人自己的身份；不能按 `data.bot` 或人的 ID 解包。[机器人信息](https://open.feishu.cn/document/client-docs/bot-v3/obtain-bot-info)

启动先以绑定 App ID/App Secret 获取自建应用 `tenant_access_token`，再依次核对 Bot 身份与启用状态、`data.tenant.tenant_key`、绑定请求路径的 Chat。Chat 必须返回 `data.chat_mode=p2p`；若返回 `tenant_key` 或 `external`，必须不与固定内部租户冲突。这两个字段允许缺省。该接口不回显 `chat_id`，单聊不返回群专用的 `chat_type`、`owner_id` 等字段，也不能通过群成员接口构造“两个人且包含 Bot”的证明。固定人和聊天的关联由每次通过签名、Verification Token 与安装校验的消息事件精确匹配 `sender_id.open_id` 和 `message.chat_id`，不假设通讯录状态。

token 加三次身份读取总预算 30 秒，每个身份 GET 至多 5 秒；随后后端握手总预算 10 秒，各请求至多 5 秒，核对 protocol 4、既定 backend、工具与完整永久绑定。失败拒绝启动，日志不回显远端原始 body 或凭据。后端或本地 owner 预留可能已提交，重试必须保持同一绑定，不创建新身份绕过它。应用管理员修改平台权限和可用范围后仍须发布并核对；JiaClaw 白名单不授予飞书平台权限。

## 配置和部署

先完成[独立用户网关部署](gateway.md)，保留一用户一容器/工作区/数据库/私有网络及实际限额卷。后端使用自己的 API Token、SQLite、Brokerrouter（离线验收可显式选择 stub）。在已有 `[http]` 表中启用 `gateway_channel_chat = true`，关闭 standalone channels、旧 inbound hook、HEARTBEAT；scheduler 只能关闭或 gateway_driven。内部模型固定走 channel 路由、`datetime_now`/`json_query`、120 秒、关闭自动 skills。网关 `request_timeout_seconds` 至少 150。

```sh
jiaclaw gateway feishu-bind --config /etc/jiaclaw/gateway.json \
  --user YOUR_USER_UUID --app-id cli_your_app --tenant-key your_tenant \
  --bot-open-id ou_your_bot --human-open-id ou_authorized_human --chat-id oc_existing_p2p
jiaclaw gateway feishu-bindings --config /etc/jiaclaw/gateway.json
```

ID 是平台真实值，不是显示名称；App/Bot/人/chat 分别以 `cli_`/`ou_`/`ou_`/`oc_` 开头，长度有界，剩余字符为 ASCII 字母、数字、下划线或连字符。人的 open_id 随 App 变化，不能从另一个 App 的记录复制。将输出的 binding UUID 放入网关顶层 `feishu` 数组，见[完整配置示例](../deploy/gateway/gateway.feishu.json.example)；示例不自动创建 App、修改平台权限或启用基础部署。

```json
"feishu": [{
  "binding_id": "YOUR_BINDING_UUID",
  "app_secret_file": "/run/secrets/alice_feishu_app_secret",
  "encrypt_key_file": "/run/secrets/alice_feishu_encrypt_key",
  "verification_token_file": "/run/secrets/alice_feishu_verification_token"
}]
```

三个 Secret 文件只挂载给网关、只读、绝对路径、当前网关 UID 所有的普通单硬链接文件、0600、16–1024 个可见 ASCII 字节；文件含可选结尾换行至多 1026 字节，拒绝链接并核对已打开的文件身份。与其他平台、后端 Token 及彼此不同，不交给租户或写入 URL/消息/日志。此分支没有 Discord 的 state_key 配置，App Secret 与短期 token 不存入业务队列；**prompt/reply 在库和备份中是明文**，必须保护存储及可信维护访问。

生产 API 固定 `https://open.feishu.cn/open-apis`，不使用代理、重定向或 HTTP 自动重试；只有显式 `allow_loopback=true` 才允许字面 loopback HTTP `/open-apis` fixture。公共 HTTPS 精确转发 `POST /hooks/feishu/{binding UUID}`，不带 query，保持原始正文与唯一 `X-Lark-Request-Timestamp`、`X-Lark-Request-Nonce`、`X-Lark-Signature`。非 loopback 监听须显式 `allow_remote_bind`，后端内部 API 不对公网开放。先启动服务和反代，再由管理员设置平台回调 URL，收到正确 challenge 后保存并完成事件订阅，避免先要求 URL 已验证的启动循环。

## 回调、共享授权与准入

普通事件签名为 `SHA256(timestamp + nonce + EncryptKey + raw_body)`，不是 HMAC；签名覆盖收到的原始 bytes，而非解密或重新编码后的 JSON。时间窗为 ±5 分钟。加密包用 AES-256-CBC，key 为 EncryptKey 的 SHA-256，base64 解码后的前 16 字节为 IV，之后为密文；严格检查 padding。原始 typed JSON 解码拒绝关键字段重复与非对象形状，签名/Token 通过后才访问 registry。[官方签名与解密](https://open.feishu.cn/document/ukTMukTMukTM/uYDNxYjL2QTM24iN0EjN/event-subscription-configure-/encrypt-key-encryption-configuration-case)

URL verification 按官方协议豁免普通事件签名，仍须解密、核对 Verification Token 和当前绑定启用状态，原样返回有界 challenge，不创建事件。飞书要求 challenge 在 1 秒内返回，普通事件在 3 秒内返回 HTTP 200；本地入口整体预算 900 ms，正文读取最多 650 ms，公共 TLS/反代/网络必须另计并实测。[官方回调时限](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/event-subscription-guide/event-subscriptions/faq)

| 边界 | 行为 |
|---|---|
| 正文 | 唯一 application/json，128 KiB 原始 body、64 KiB 解密正文、16 KiB 用户文本（与独立后端合同一致，超限不持久准入）；过大 413、正文超时 408、总截止或存储失败 503 |
| 入站容量 | 全局 8、每安装 1、阻塞 I/O 全局 8；忙或队列满 429，超时仍在执行的 I/O 保留槽至结束 |
| 身份 | schema 2.0、header App/tenant/Token、固定 sender user/open_id 和 chat_id、消息 chat_type=p2p/text；群聊或未授权人不准入 |
| 去重 | 使用安装范围内 `message.message_id`；同一消息换 event_id 重投不再执行，原消息 ID 的人/chat/text 冲突 409，不覆盖旧数据 |
| ACK | 只在持久接纳后 HTTP 200；成功 ACK 不代表模型完成或平台送达 |
| 执行与发送 | 每次准入复核用户 enabled、不可撤销回退的绑定及既定后端；同用户共享 hold、后端单执行槽和配置的全局执行容量 |

消息专页明确要求 `message_id` 去重，不能用通用事件 FAQ 的 event_id 建议替代。[消息身份合同](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/events/receive)

HTTP API Key 的轮换、撤销或只读权限不取消另行授权的飞书后台工作；用户 disable 或 feishu-revoke 才阻止以后准入。enable 不清 hold、不复活已撤销 App；已提交模型或平台请求仍需核对。入站内容不能选择用户、后端、模型、工具或通知目标。

## 状态、发送与容量

registry 事务升级至 schema 6，保留 schema 1–5 的用户、Key 权限、hold、审计和原渠道身份。每绑定库位于 registry 同级 `feishu/{UUID}.sqlite3`，0700 目录、0600 单链接文件、生命周期排他锁；独立 application ID 和 protocol 4 完整 owner 校验后才打开 SessionStore schema 10。owner 固定 binding/user/backend/App/tenant/Bot/人/chat，不包含 enabled；不可 UPDATE/DELETE/REPLACE。拒绝普通会话库、其他渠道库、异 owner 和未来版本。

首次初始化用同目录 `.sqlite3.initializing` 的 FULL rollback-journal 事务提交完整 owner，关闭 SQLite/同步文件，再原子不覆盖发布并同步目录。仅受限空 stage 或精确 owner-only 已提交 stage 可供正常初始化恢复；停机维护不会把未知空 stage 当作已有已认证 owner。部分文件、热 journal、孤立 sidecar、外来 owner 保留并拒绝，需要可信管理员停机一致备份、核对；不能删除残留或修改 owner 自动继续。Linux/macOS 的原子发布合同独立于断电后全部状态自动恢复。

| 资源 | 限制与维护 |
|---|---|
| inbox/outbox | 1000 事件、10000 投递、10000 去重 tombstones，最短去重保留 7 天；满额拒绝新增 |
| 网关操作账本 | 16000 条，不自动裁剪；UUIDv7 request 与事件/投递/attempt 同事务关联，插入失败回滚 claim；已解决事件 purge 级联清理关联，保留去重 |
| 后端请求账本 | 每独立后端 16000 个永久身份，admitted/completed 不回退、不复用；当前没有删除/重新分配 CLI，接近满额需要受审查的容量迁移 |
| 数据库与 WAL | 主文件约 64 MiB page quota、FULL、250 ms busy、256 页 checkpoint、2 MiB journal 回收目标；启动拒绝超过 128 MiB 的 sidecar，WAL 目标不是硬配额 |
| 文本 | 固定原单聊、总 16 KiB UTF-8、每片 2000 UTF-16 单元、最多 16 片；发送前把 `<`/`>` 转成全角，禁止文本构造 at/样式标签 |
| 发送 | 连接 2 秒、token 获取含锁等待最多 12 秒、单次消息 HTTP 10 秒、总发送 25 秒、响应 64 KiB；约 1.1 秒发送间隔与有效 429 冷却持久保留 |

每片先持久 submitting，以固定投递 UUID 作飞书 `uuid`；平台只保证同 UUID 在一小时内至多成功创建一条消息，不授予无限恢复重试。成功必须 HTTP/JSON 对象/code=0、合法消息 ID、匹配 chat/text 回执；对象边界保留原字节重复字段拒绝。明确已知未发送限流和唯一有效 `x-ogw-ratelimit-reset` 才持久重试，最多五次。凭据、权限等明确终态拒绝保留 permanent_failed；5xx、断线、超时、矛盾回执或未知业务错误保留 unknown。它们及结算失败都保持人工 hold，后续片不越过未知片。详细飞书出站判定沿用[飞书渠道发送合同](feishu.md#发送凭据与结果)。

模型侧最多 120 秒，网关等后端完整结果至多 150 秒；结算等 I/O 槽最多 5 秒，取消仍在执行的阻塞任务保留槽。网关和后端不能跨库原子提交：崩溃中的 processing 转 needs_review、submitting 转 unknown，保留原 request，不重新调用模型/工具或平台。后端 metadata 只证明原会话事务状态，不证明平台送达、工具可撤销或 hold 可自动解除。

逻辑私有库仍共享网关总限额卷，**没有逐绑定物理磁盘隔离**。主文件 quota 不含 registry、WAL、日志或备份；须设置总字节/inode 配额、预留事务空间并监控。停机一致备份 registry、全部 telegram/slack/discord/feishu 目录及残留、各后端状态与 Secrets；升级前备份，旧二进制拒绝 schema 6。旧快照会回退权限、撤销、hold 或丢失外部收据，必须核对恢复点后效果，不直接恢复公网服务，不删除未知记录释放容量。

## 停机维护与核对

先关闭平台入口、停止并排空网关、确认后端没有继续请求。以下业务维护取得同一 gateway 停机锁，在线拒绝；普通绑定查询/撤销使用 registry 短事务。

```sh
jiaclaw gateway feishu-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind events
jiaclaw gateway feishu-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind deliveries --event YOUR_EVENT_UUID
jiaclaw gateway feishu-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind operations
jiaclaw gateway feishu-resolve --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID \
  --delivery YOUR_DELIVERY_UUID --receipt om_verified_platform_message
jiaclaw gateway feishu-cancel --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway review-clear --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID \
  --confirm-backend-idle --note '已核对原请求、平台回执及全部保留渠道队列'
jiaclaw gateway feishu-purge --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway feishu-revoke --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID
```

inspect 默认 limit=20、1–100；operations offset≤16000，events/deliveries≤10000。内容含私密 prompt/reply，仅供可信管理员；operations 的 event_id 是内部事件 UUID，平台 message_id 位于 events 的 spec.event_id，原 request UUIDv7 与 audit 的 feishu_execute/feishu_send 关联。resolve 只记录人工核对回执，不发网络请求；cancel 取消尚未发送片，不能撤销外部效果；purge 只清已解决业务内容并保留 tombstone。

后端可信管理员用其私有 Bearer Token 查询 `GET /internal/channels/feishu/requests/{原 request UUIDv7}`。返回 protocol/backend/binding/request/event/session/status，status 为 admitted/completed，不返回正文或完整结果；binding/status/execute/requests 均不在公开用户网关白名单。核对接口没有 replay 协议。

review-clear 必须检查该用户全部保留渠道，包括撤销或已移除 runtime 的绑定。Feishu 维护不依赖 App Secret、EncryptKey 或 Verification Token；完全无状态时不创建库，残留 stage/sidecar 与已有 owner 都必须核对，不能误报无状态。未知事件和 submitting/unknown/失败投递阻止清 hold；已核对取消后可以保留未执行 received 队列，但清 hold 不保证撤销的绑定会再执行。删配置、换 App 或丢弃业务库不是完成核对。应用 hold/收据/outbox 不等于 StateKnot durable driver。

## 验收状态

本机完整默认并行 1097 项 Rust、fmt/必需 Clippy/locked check/build、新整机 11 组与既有进程回归 21 套最终通过，使用同一最终二进制；初始失败、实际修订和证据分层见[验证记录](validation.md)。当前 draft PR 的最终 exact-head CI 仍待核对 Linux/macOS 与真实容器，不据本机结果标记完整跨平台验收。

协议 fixture 使用一次性凭据，不触发真实平台消息或付费供应商。正常 7200 秒 token mint 的启动 HTTP 取消/截止、明确失效后的 terminal/no resend 与离线核对已作进程验证；Sender 单元覆盖 cache 失效/退避/取消，不能外推运行期正常到期刷新认证。真实飞书安装、权限可用范围、事件字段、终端消息、TLS 总预算、平台 token/限额、共享网关卷及容器渠道 runtime 压力、供应商联合认证仍另行完成。
