# 飞书自建应用接入

飞书渠道复用 JiaClaw 的 SQLite 收件箱、后台 Agent、会话事务和持久发件箱。当前支持中国飞书企业自建应用、单租户安装、个人单聊和获准群聊的文本消息，以及显式授权的定时文本通知。商店应用的多租户授权、Lark 国际站、长连接接收、图片、语音和卡片不在本次实现范围内。

## 安装和配置

在飞书开发者后台创建并发布企业自建应用，启用机器人能力，配置应用可用范围及所需消息权限。订阅 `im.message.receive_v1`，将请求地址指向公开 HTTPS 反向代理后的 `POST /hooks/feishu`；反向代理必须保留原始 body 和 `X-Lark-*` 请求头。应用可用范围、机器人群成员资格及群发言权限都必须与实际用户一致。群聊接收范围由飞书审批后的权限决定，JiaClaw 的白名单不会代替平台授权。[官方接收消息文档](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/events/receive)、[官方发送消息文档](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/create)

```toml
[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"
shutdown_timeout_secs = 30

[[http.channels]]
channel = "feishu"
installation_id = "cli_your_app_id:your_tenant_key"
allowed_senders = ["ou_authorized_user"]
allowed_conversations = ["oc_authorized_private_chat", "oc_authorized_group"]
enabled_tools = ["datetime_now", "json_query"]
timeout_secs = 120
```

`installation_id` 是精确的 `app_id:tenant_key`，不是显示名称。飞书绑定不填写仅 Slack 使用的 `app_id` 配置字段。一个服务进程只接一个飞书安装；当前存储采用 SQLite 独占进程锁，不支持多个服务共享同一数据库文件。模型须使用 `brokerrouter`，离线验收可显式使用 `stub`；服务必须配置持久化和管理 API Token。

通过服务的密钥管理注入以下环境变量：

| 变量 | 内容 |
|---|---|
| `JIACLAW_API_TOKEN` | 管理 API 的 Bearer Token |
| `JIACLAW_FEISHU_APP_SECRET` | 对应自建应用的 App Secret |
| `JIACLAW_FEISHU_ENCRYPT_KEY` | 事件与回调加密策略中的 Encrypt Key |
| `JIACLAW_FEISHU_VERIFICATION_TOKEN` | 同一应用的 Verification Token |

平台密钥也可配置为 `http.feishu_app_secret`、`http.feishu_encrypt_key`、`http.feishu_verification_token`；环境变量优先。不要将真实密钥提交到仓库、放入消息内容或复制到故障报告。`local_test_api_base` 仅用于显式字面 loopback HTTP fixture，飞书测试路径可使用 `http://127.0.0.1:端口/open-apis`；正式部署应省略，使用固定 `https://open.feishu.cn/open-apis`。

## 接收、身份和会话

普通事件必须通过原始 body 的 SHA-256 签名校验、±5 分钟时间窗、Verification Token 和 app/tenant 绑定。签名算法为 `SHA256(timestamp + nonce + EncryptKey + raw_body)`，不是 HMAC。签名明文事件和加密事件都可接收；加密事件使用 SHA-256 派生的 AES-256-CBC 密钥、密文前 16 字节 IV 和严格 PKCS#7 校验。回调 body、解密结果和文本分别有大小限制，错误不会回显凭据或解密细节。[官方签名与解密说明](https://open.feishu.cn/document/ukTMukTMukTM/uYDNxYjL2QTM24iN0EjN/event-subscription-configure-/encrypt-key-encryption-configuration-case)

URL 验证按官方协议单独处理：验证解密后的 Verification Token，并原样返回 `{"challenge":"收到的值"}`，不要求普通事件签名头。平台要求 challenge 在 1 秒内返回、普通事件在 3 秒内返回 HTTP 200。正常消息先完成持久化准入再 ACK，Agent 在后台运行；数据库不可用时返回错误，不能先 ACK 再尝试保存。[官方回调 FAQ](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/event-subscription-guide/event-subscriptions/faq)

只处理获准 `open_id` 发送者、获准 `chat_id` 的用户文本消息。应用/机器人消息、其它事件和非文本消息不会触发 Agent。header 与 sender 的租户标识不能跨越安装边界。工具白名单须非空，沿用后台任务可取消工具集合；执行和发送前重新检查授权。完整的后台工具、容量和恢复约束见[可靠渠道说明](channels.md)。

业务去重使用安装范围内的 **`message.message_id`**。同一消息即使被飞书用不同 `header.event_id` 重投，也只运行一次 Agent；同一消息 ID 对应不同发送者、目标或文本时拒绝冲突。这个规则来自接收消息专页，不能只用通用事件投递 ID 去重。[官方消息去重要求](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/events/receive)

单聊在原 `chat_id` 发送普通消息。群聊回复到入站 `root_id`，没有 root 时使用当前 `message_id`。JiaClaw 的 `Destination.thread_id` 因而保存 **`om_` 根消息 ID**，用于 `/im/v1/messages/{message_id}/reply` 和 `reply_in_thread:true`；它不是飞书原生的 `omt_` 话题 ID。会话继续按平台、安装、会话、根消息和发送者隔离，同一线程不同用户不会共享全部历史。飞书返回的话题回复 `parent_id` 可指话题根，不能按普通嵌套消息假定其总是直接入站消息。[官方回复消息文档](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/reply)

## 发送凭据与结果

服务按需使用 App ID / App Secret 获取自建应用 `tenant_access_token`，在内存中缓存，并在平台返回的有效期前预留 60 秒刷新。并发刷新合并为一次请求；包括等待刷新锁在内的凭据获取预算为 12 秒，失败后退避 30 秒。token 不写入发件箱或管理 API，重启后重新获取。发送接口返回 HTTP 401 或凭据业务码 `99991663` / `99991665` 时，会清除缓存并退避 30 秒，之后的独立消息可以重新获取 token；当前消息不会因此刷新后重发。退避期间尝试投递的新消息会明确记录 `credential_unavailable` / `permanent_failed`，需管理员核查并取消未发送记录，不能期望它们自动等待后补发。此流程不覆盖商店应用的 app_ticket / 多租户 token 生命周期。

消息发送连接超时为 2 秒、单次 HTTP 总超时为 10 秒，响应体上限 64 KiB；不跟随重定向或隐藏重试。飞书安装使用持久化的 1.1 秒基础发送间隔，平台限流会延长它。Agent 最长时间由安装 `timeout_secs` 控制；工具超时、进程取消不能证明外部工具没有产生效果。

文本沿用总量 16 KiB UTF-8、每片 2,000 UTF-16 单位及最多 16 片限制。发件箱保存原始回复；发送时将 ASCII `<`、`>` 替换为全角字符，避免模型输出构造飞书内联 `at` / 样式标签。每片使用固定发件记录 UUID 作为平台 `uuid`；限流后的再次尝试保持相同 UUID，不重新运行模型。官方仅承诺相同 UUID 一小时内至多成功创建一条消息，因此该字段不能变成跨重启无限重试的依据。[官方消息 UUID 契约](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/create)

HTTP 2xx 不代表已送达。必须同时得到 `code:0`、合法 `om_` 消息 ID、匹配的 `chat_id` 和文本类型；线程回复还必须匹配绑定的根消息。保存有效回执后才记为 `delivered`。

| 结果 | 发件箱处理 |
|---|---|
| 有效平台限流 | `retry_wait`；保留 UUID、attempts 和安装冷却期，每片最多 5 次尝试 |
| 发送前凭据获取失败、明确参数或权限拒绝 | `permanent_failed`，不自动重跑 Agent 或重发 |
| HTTP 5xx、断线、发送超时、错误目标或矛盾线程回执、未知业务错误（包括未知 HTTP 400） | `unknown`，等待平台侧核对 |
| `230049` 或 `18121` 表示消息仍在处理中 | `unknown`，不将其当作可安全重发的失败 |
| 重启发现 `submitting` | `unknown`；不因本地没收到回执而假定未发送 |

仅接受 HTTP 429 / 400 对应 `99991400`，或 HTTP 400 对应 `230020` 的已知未发送限流；还必须存在单一有效 `x-ogw-ratelimit-reset` 整数秒值（1–3600），且 `data` 缺省、为 null 或空对象。它不是 Slack 的 `Retry-After`。缺少、重复或畸形等待头、矛盾的状态/业务码及携带成功数据的限流响应都不会自动重试。业务错误只保存本地错误代码，不保存远端原始 body。[官方频控策略](https://open.feishu.cn/document/ukTMukTMukTM/uUzN04SN3QjL1cDN)、[官方业务错误码](https://open.feishu.cn/document/ukTMukTMukTM/ugjM14COyUjL4ITN)

`unknown` 会阻塞同一目的地的后续发送。使用受 API Token 保护的 `GET /api/channels/deliveries` 检查记录；在飞书侧确认已发送后，通过 `POST /api/channels/deliveries/{id}/resolve` 提交 `{"action":"delivered","receipt":"平台消息 ID 和核对说明"}`，或提交 `{"action":"cancel"}` 取消该事件/运行的剩余分片。操作不会再次调用平台，也不会撤回已经发出的消息。

收件箱、会话和完整发件箱仍通过 SQLite 事务提交。重启发现 `processing` 时转为 `needs_review`，不重放工具；业务队列和去重记录按[渠道保留与清理规则](channels.md)管理。业务文本在数据库中为明文，需要保护数据库目录与备份。应用的真实平台密钥不存入这些记录。

## 定时通知

入站 `allowed_conversations` 不授予主动发送权限。启用 scheduler 后，必须在同一安装下逐个配置精确目的地；未配置时默认禁止。

```toml
[[http.channels.scheduled_destinations]]
conversation_id = "oc_authorized_private_chat"

[[http.channels.scheduled_destinations]]
conversation_id = "oc_authorized_group"
thread_id = "om_existing_authorized_root_message"
```

配置项放在对应 `[[http.channels]]` 后面。管理 API 的 JobSpec 使用同一精确目标：

```json
{
  "name": "每日检查",
  "prompt": "汇总本次已授权检查的结果。",
  "schedule": {"kind":"cron","expression":"0 9 * * 1-5","timezone":"Asia/Shanghai"},
  "enabled_tools": ["datetime_now"],
  "timeout_secs": 120,
  "delivery": {
    "channel":"feishu",
    "installation_id":"cli_your_app_id:your_tenant_key",
    "conversation_id":"oc_authorized_group",
    "thread_id":"om_existing_authorized_root_message"
  }
}
```

Job 的工具集合必须是安装工具白名单的子集。创建、恢复、执行和发送前都会重新检查目标授权。通知 `unknown` 或永久失败会暂停任务；未解决投递禁止恢复和重叠执行。暂停或删除任务不会取消已入库结果，需要显式取消对应 run 的投递。运行审计、取消和清理接口见[定时通知](scheduled-delivery.md)。

## 验收

```sh
cargo build --locked -p jiaclaw-host
python3 tests/feishu.py target/debug/jiaclaw
```

脚本使用真实 JiaClaw 二进制、临时数据库、Python 本地 HTTP 平台与模型 fixture，并用 OpenSSL 独立生成加密回调。包含官方发布的 AES 已知密文样本，以及 challenge、签名、租户/发送者拒绝、不同 event_id 的消息去重、原生工具闭环、群根回复、token 复用、UUID、限流与重启冷却、未知发送结果和强杀恢复、精确定时目标及环境变量凭据验收。

这些测试验证本地协议与恢复行为。真实安装仍须验证企业审批、事件权限、公开 HTTPS 回调、真实用户/群身份、消息实际出现的线程位置、平台 token 生命周期和限流响应。当前没有使用真实飞书凭据或发送真实消息；Brokerrouter 的真实供应商默认认证和 StateKnot durable 契约也仍是独立未完成项，见[上游能力差距](brokerrouter-gaps.md)。
