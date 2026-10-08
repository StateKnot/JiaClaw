# 钉钉企业内部应用机器人

JiaClaw 接收企业内部应用机器人的 HTTP 私聊文本回调，先将事件写入 SQLite，再在后台执行 Agent，并通过固定的机器人单聊发送接口回复同一成员。显式授权的定时成员通知复用同一发件箱与人工核查接口。当前范围是企业内部应用机器人；群聊、Stream 接入、自定义群机器人 webhook、工作通知、第三方应用和卡片不在此接入范围内。

## 应用与部署

在钉钉开发者后台创建企业内部应用并启用机器人，配置 HTTP 消息接收地址 `https://你的域名/hooks/dingtalk`，授予企业内机器人发送消息权限并发布到获准成员可见范围。分别记录企业 CorpID、机器人 robotCode、应用 Client ID 和 Client Secret。robotCode 与 Client ID 是不同字段，不能假设两者相同。[机器人接收消息](https://open.dingtalk.com/document/dingstart/robot-receive-message.md)、[机器人单聊发送接口及权限](https://open.dingtalk.com/document/development/chatbots-send-one-on-one-chat-messages-in-batches.md)

```toml
[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"
shutdown_timeout_secs = 30

[[http.channels]]
channel = "dingtalk"
installation_id = "dingRobotCode:dingCorpID"
app_id = "dingApplicationClientID"
allowed_senders = ["Alice.User", "employee123"]
allowed_conversations = ["Alice.User", "employee123"]
enabled_tools = ["datetime_now", "json_query"]
timeout_secs = 120
```

服务要求 SQLite、非空管理 API Token 和工作空间之外的数据库文件。模型提供商为 `brokerrouter`；离线验收可显式使用 `stub`。每个进程最多一个钉钉安装，数据库由单进程独占。

由服务密钥管理系统注入 `JIACLAW_API_TOKEN` 和 `JIACLAW_DINGTALK_APP_SECRET`。后者对应应用的 Client Secret，也可使用 `http.dingtalk_app_secret` 配置；环境变量优先。`app_id` 是该 Secret 对应的 Client ID，不能填 robotCode 来代替。平台凭据与回调签名头不进入会话、发件箱或管理 API。

`installation_id` 使用 `robotCode:CorpID`。每个组成部分采用 JiaClaw 支持的 ASCII 字母、数字、`_-.` 子集，长度不超过 64 字节，完整身份不超过 128 字节。成员 ID 保留大小写；`Alice.User` 和 `alice.user` 不会被应用归并。官方 UserID 是企业范围内 1–64 字符的唯一标识；JiaClaw 当前支持其中的 ASCII 子集：首字符为字母或数字，其余字符允许字母、数字、`_-.@`。这是应用配置约束，不是对钉钉全部合法 ID 的重新定义。[官方成员 ID 约束](https://open.dingtalk.com/document/development/user-information-creation.md)

`allowed_senders` 和 `allowed_conversations` 都填写精确的成员 UserID，禁止广播、部门、多个拼接成员或通配符。入站 `senderStaffId` 同时作为会话发送者和主动回复目标；回调里的 `conversationId` 会校验，但不会被当成成员 ID 或主动发送目标。`thread_id` 必须为空。会话按平台、安装与成员隔离，工作空间仍由实例共享，并不构成多用户操作系统隔离。

## HTTP 回调与信任边界

`POST /hooks/dingtalk` 校验两个请求头：`timestamp` 是毫秒 Unix 时间，`sign` 是 Base64 编码的 HMAC-SHA256。HMAC 的 key 为 Client Secret，消息为 `timestamp + "\n" + ClientSecret`。签名须在服务时钟的正负一小时范围内；重复签名头、非法编码和畸形时间被拒绝。宿主时钟必须可靠并保持同步。[官方 HTTP 接收签名](https://open.dingtalk.com/document/development/receive-message.md)

**该平台签名不覆盖请求体。** 必须通过可信 HTTPS 入口接入，并确保 TLS 终止点至 JiaClaw 的转发链路受控；代理、APM 与请求追踪不得记录或公开 `timestamp` / `sign`。获得仍在有效期内签名头的人可能构造不同请求体，因此不能把验签通过解释成请求体本身具有独立的密码学完整性。不要将不可信代理放在该信任边界内。

回调中的 `robotCode`、`chatbotCorpId` 必须匹配配置；触发 Agent 的私聊文本还要求 `senderCorpId` 匹配同一企业，`conversationType = "1"`、`msgtype = "text"`，并通过成员与工具白名单。群聊、非文本和已识别的配额通知不会执行工具。原始 JSON 最多 128 KiB，文本最多 32 KiB，嵌套深度有界。请求须且仅有一个 `Content-Type: application/json`（可带 charset 参数）；根与非 null 的 `text` 必须是原始 JSON 对象，位置数组、重复关键字段或含混 MIME 均在持久准入前返回统一 401。直接读取原始字节，不先转为会消除重复键的 `Value`；缺失/null 的可选 text 与附加平台元数据保持既有语义。

合法文本在 SQLite 提交后返回 **HTTP 200 空响应体**，随后由后台 worker 调用模型与发送消息。接收接口不等待模型完成，也不会把模型正文作为回调响应。官方 HTTP 示例使用空返回值；当前核对的文档没有可据以承诺的固定 ACK 截止时间或重试次数，整机测试中的快速 ACK 是本地回归要求。[HTTP 接收示例](https://open.dingtalk.com/document/dingstart/robot-receive-message.md)

同一安装的相同 `msgId` 只接收一次；`msgId` 作为有界不透明字符串处理，允许官方示例中的 Base64 风格字符。相同 ID 的发送者、目标或正文变化返回冲突。平台在实际重投时是否始终保持该 ID、重投时序和超时行为仍须真实安装验证；本地去重合同不能替代平台重投保证。

回调 `sessionWebhook`、过期时间及其它 URL 字段不用于发送、不写入事件快照，也不进入诊断。所有主动回复只使用本地配置的安装和获准成员，通过固定平台端点发送。

## 主动消息与回执

服务调用 `POST https://api.dingtalk.com/v1.0/oauth2/accessToken`，JSON 请求中的 `appKey` 使用配置 `app_id`，`appSecret` 使用 Client Secret。有效响应中的 `accessToken` 按 `expireIn` 在内存缓存并预留刷新时间；并发刷新合并，含锁等待的获取预算为 12 秒，失败后退避 30 秒；缓存最长采用 7,200 秒并提前 60 秒失效。token 不写数据库，重启后重新获取。token 响应要求唯一 JSON MIME、原始根对象和唯一关键字段；形状/MIME 含混或 code 与凭据矛盾时不缓存、不调用消息接口，现有失败退避继续生效。[获取企业内部应用 accessToken](https://open.dingtalk.com/document/development/obtain-the-access-token-of-an-internal-app.md)

发送固定调用 `POST https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend`，凭据放在 `x-acs-dingtalk-access-token` 请求头。每次 `userIds` 只包含一个精确成员，`robotCode` 使用绑定机器人，`msgKey = "sampleText"`，`msgParam` 是包含 `content` 的 JSON 字符串。不会使用回调 URL 或接受模型提供的服务端点。[单聊发送接口](https://open.dingtalk.com/document/development/chatbots-send-one-on-one-chat-messages-in-batches.md)、[文本模板](https://open.dingtalk.com/document/development/robot-message-type.md)

JiaClaw 的本地回复上限为 16 KiB UTF-8，总共最多 16 片，每片最多 2,000 个 UTF-16 单位，按 Unicode 字符边界拆分且不静默截断。该数值是应用限制：当前官方文本模板文档未给出可据以认证的数字上限，平台仍可能拒绝 `msgParam.tooLong`。部署验收必须实际验证目标机器人、语言及字符组合的接受范围，不能把本地拆分测试解释为平台长度认证。

`delivered` 要求 HTTP 成功、有效且有界的 `processQueryKey`，以及 `invalidStaffIdList`、`flowControlledStaffIdList`、`filteredStaffIdList` 全部为空、null 或不存在。非空失败列表、错误与成功字段矛盾、缺失回执均不能算完整成功。发送响应同样要求唯一 JSON MIME 与原始对象/关键字段；已提交请求的形状或 MIME 含混进入 unknown，保持单次请求、不自动重发。明确 code 同时出现任何字符串 processQueryKey（包括空字符串）属于矛盾回执；仅不存在/null 的回执允许继续按原有精确 HTTP/code 拒绝白名单分类。官方 SDK 同时定义了这些失败列表；回执保存的是 **API 接受证据，不是终端展示或已读证明**。[固定版本官方 SDK 合同](https://github.com/alibabacloud-go/dingtalk/blob/1986c966942afc67b8ccae59d29d57989a45fe76/robot_1_0/client.go)

每个安装同时最多一条发送请求；发送完成后保守等待至少 4 秒，冷却记录跨重启保存。当前没有将其它钉钉产品的每日额度或自定义 webhook 速率套用到这个接口，也没有声称本地间隔保证平台剩余额度。企业权限、应用额度和真实接收效果仍需在目标安装确认。

请求不跟随重定向，也没有隐藏重试。未知 HTTP 400、429、5xx、超时、断线或矛盾回执进入 `unknown`，不自动重发；只有已核实的明确参数/权限拒绝归入 `permanent_failed`。当前没有已核实的可信 429 等待合同，不能凭状态码或猜测的冷却时间重发。token 失效只影响后续独立发送的缓存；不会刷新 token 后重放当前消息。

正式发送主机固定为 `https://api.dingtalk.com`。独立测试可配置 `local_test_api_base = "http://127.0.0.1:端口"`，只允许显式字面 loopback HTTP 根地址，生产配置应省略。

## 恢复与定时通知

Agent 结果、会话和全部待发分片一起提交。进程重启发现 `processing` 时转为 `needs_review`，不重跑可能已经执行的工具；`submitting` 转为 `unknown`。相同目的地后续发送不会越过不确定结果，其它获准成员仍可在安装冷却后处理。未知发送没有自动重试操作。

管理员通过 `GET /api/channels/deliveries` 查看投递，核对平台接受记录后，可用 `POST /api/channels/deliveries/{id}/resolve` 提交 `{"action":"delivered","receipt":"processQueryKey 与核对说明"}`，或用 `{"action":"cancel"}` 取消来源事件/运行的剩余片段。人工操作不再调用平台、不撤回已发送消息。完整状态、容量与审计清理规则见[可靠渠道](channels.md)。

主动通知使用单独白名单，入站成员授权不会自动转为定时发送权限。启用 scheduler 后，在安装中配置：

```toml
[[http.channels.scheduled_destinations]]
conversation_id = "Alice.User"
```

JobSpec 的 `delivery`：

```json
{
  "channel": "dingtalk",
  "installation_id": "dingRobotCode:dingCorpID",
  "conversation_id": "Alice.User",
  "thread_id": null
}
```

任务工具必须是安装工具白名单的子集；创建、恢复、执行与发送时重新校验权限。未知或永久失败会暂停任务，未解决投递阻止下一轮执行。暂停/删除任务不取消已经提交的通知，需要显式取消运行的剩余投递。见[定时通知](scheduled-delivery.md)。

## 验收边界

```sh
cargo build --locked -p jiaclaw-host
python3 tests/dingtalk.py target/debug/jiaclaw
```

脚本只使用 Python 标准库、真实 JiaClaw 二进制、临时 SQLite、本地模型/平台 HTTP fixture 和一次性测试凭据。覆盖签名头与时间窗、企业/机器人身份、大小写保留的成员授权、持久 ACK/去重冲突、原生工具、固定 token/发送端点、同文独立事件、Unicode 分片、发送间隔、未知回执与强杀恢复、精确定时成员、环境变量以及 callback URL 与平台错误脱敏。

2026-10-03 使用本批构建的实际二进制执行上述脚本，退出码为 0，三组整机验收全部通过。额外断言覆盖解析前 128 KiB 请求体上限（HTTP 413）、32 KiB 文本上限、延迟回执后仍保持完整 4 秒冷却，以及临时数据库和日志中不含 callback URL、签名头或平台凭据。该证据使用本地 fixture，不涉及真实钉钉服务。

真实上线仍须验证应用归属与发布、机器人可见范围、成员身份、发送权限、受控 HTTPS 转发、回调重投、长文本边界、额度和终端展示。本地 fixture 不代替真实安装认证；本次没有使用真实平台凭据或发送真实消息。模型供应商认证和 StateKnot durable 合同另见[上游状态](brokerrouter-gaps.md)。

## 本批协议修复与安装证明的区别

原始根/text对象、唯一JSON MIME和含混回执是已部署standalone接收/发送路径的实际合同加严，不是独立用户 registry/私库/后端接线。`filteredStaffIdList`此前已经处理，本批保留并扩充原始HTTP验证，不能记成新增支持。没有引入未知结果重发、新额度或缩短token寿命来触发刷新；正常到期刷新、真实平台权限/客户端和容器内启用渠道压力仍须认证。

2026-10-08 14:39 UTC 正式目录与固定SDK核对：内部token仅返回accessToken/expireIn；全企业`allInnerApps`和新版`scopes`不能独自证明当前凭据对应配置robotCode/CorpID。固定SDK存在`GET /v1.0/microApp/app/detail`（[原始定义](https://github.com/alibabacloud-go/dingtalk/blob/1986c966942afc67b8ccae59d29d57989a45fe76/micro_app_1_0/client.go#L8826)），但完整官方目录未取得它的最小权限及企业内部独立机器人适用性证据；这不是平台“不支持”的证明，也不能猜权限或仅mint token来宣称完整启动校验。本批不添加未经核实的安装gate、不将独立用户钉钉标完成。[新版应用范围](https://open.dingtalk.com/document/development/obtains-the-application-visible-range.md)

完整本地与固定提交验收见[验证记录](validation.md#钉钉原始报文合同批次)。可信TLS入口仍必需：严格JSON形状不改变平台MAC只覆盖timestamp/ClientSecret、不覆盖正文的原始事实。
