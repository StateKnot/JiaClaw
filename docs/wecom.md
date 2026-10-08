# 企业微信自建应用接入

本渠道接入企业微信企业内部自建应用：接收成员发给应用的文本消息，后台运行 Agent，再通过应用消息接口回复同一成员。它支持显式授权的定时成员通知，复用 SQLite 收件箱、会话事务、持久发件箱和人工核查接口。

这是单实例渠道，使用该服务的共享工作区和数据库；独立用户网关尚无企业微信永久绑定、私有队列或专属后端入口。本批补齐实际 `serve` 路径的启动身份校验，不将它计为完整独立用户企业微信交付。

企业微信“智能机器人”使用另一套协议，包括 `aibotid`、群聊 `chatid`、临时 `response_url` 和流式刷新；这些字段不能放进自建应用配置。本次也不接群机器人 webhook、应用群聊会话、客户群、微信客服、第三方套件或个人微信会话。[自建应用接收消息](https://developer.work.weixin.qq.com/document/path/90238)、[智能机器人接收消息](https://developer.work.weixin.qq.com/document/path/100719)

## 部署条件

在企业微信管理后台创建应用，记录 CorpID、应用 AgentID 和该应用自己的 Secret。将获准成员加入应用可见范围，配置可信出口 IP，并在“接收消息 / 设置 API 接收”中配置公开 HTTPS URL、回调 Token 和 EncodingAESKey。回调路径为 `GET /hooks/wecom` 和 `POST /hooks/wecom`，当前采用 XML 消息格式。反向代理必须保留查询参数和请求体；管理 API 应只向可信网络开放并启用 API Token。

```toml
[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"
shutdown_timeout_secs = 30

[[http.channels]]
channel = "wecom"
installation_id = "ww_your_corp_id:1000002"
allowed_senders = ["alice", "bob@example.com"]
allowed_conversations = ["alice", "bob@example.com"]
enabled_tools = ["datetime_now", "json_query"]
timeout_secs = 120
```

`installation_id` 精确绑定 `CorpID:AgentID`，AgentID 必须是正整数。此渠道不填写 Slack 使用的 `app_id` 字段。每个进程最多配置一个企业微信安装；SQLite 使用独占进程锁，不能由多个 JiaClaw 实例共享。必须开启持久化、管理 API Token，并使用 `brokerrouter` 模型提供商；离线验收可显式使用 `stub`。

由服务的密钥管理系统注入环境变量：

| 变量 | 用途 |
|---|---|
| `JIACLAW_API_TOKEN` | 管理 API 的 Bearer Token |
| `JIACLAW_WECOM_APP_SECRET` | 对应自建应用的 Secret |
| `JIACLAW_WECOM_CALLBACK_TOKEN` | 1–32 个 ASCII 字母或数字组成的回调签名 Token |
| `JIACLAW_WECOM_ENCODING_AES_KEY` | 管理后台配置的 43 字符 EncodingAESKey |

平台凭据也可配置为 `http.wecom_app_secret`、`http.wecom_callback_token`、`http.wecom_encoding_aes_key`，对应环境变量优先。不要使用通讯录同步、客户联系或其它应用的 Secret 代替当前应用凭据。正式发送固定访问 `https://qyapi.weixin.qq.com/cgi-bin`；测试覆盖地址 `local_test_api_base` 仅允许显式字面 loopback HTTP，可使用 `http://127.0.0.1:端口/cgi-bin`。

成员 UserID 不区分大小写，JiaClaw 使用 ASCII 小写规范值保存白名单、会话身份和发送目标。ID 为 1–64 字节，首字符须为字母或数字，其余仅允许字母、数字、`_-.@`。`allowed_senders` 和 `allowed_conversations` 都是单个成员的精确列表，不接受 `@all`、`alice|bob`、部门、标签或通配符。`conversation_id` 等于入站 `FromUserName` 的规范值；`thread_id` 必须为空。[官方成员 ID 约束](https://developer.work.weixin.qq.com/document/path/90195)

应用的可见范围与可信 IP 仍需平台侧配置；本地白名单不会替代平台授权。必须为 JiaClaw 使用专用自建应用，并让 JiaClaw 独占该应用的发送权，保证其它系统不会消耗同一接收额度。

## 启动校验

每次 `serve` 先完成本地配置、API Token、渠道白名单及 gateway-driven 后端限制检查，再执行企业微信安装校验。它位于 Agent/MCP 连接、会话数据库打开、HTTP 监听及所有 worker 启动之前。未配置企业微信时不发起这些请求；校验失败则整个服务启动失败，不接受回调、运行模型、开始定时任务或发送消息。

校验按固定官方 HTTPS origin 依次读取两个接口，不发送测试消息，也不自动重发查询：

1. 使用配置的 CorpID 与该应用自己的 Secret 调用 `GET /cgi-bin/gettoken`。该响应不回显 CorpID 或 AgentID，不能仅凭拿到 token 判定应用身份。
2. 使用所得 token 与预期 AgentID 调用 `GET /cgi-bin/agent/get`。官方只允许取得凭证对应的应用；成功必须 `errcode=0`、返回整数 `agentid` 精确匹配，且 `close=0`。校验实际读取的 `allow_userinfos.user[].userid`，不要求接口回显不存在的 corp_id、bot_id 或 chat_id。[官方 access_token](https://developer.work.weixin.qq.com/document/path/91039)、[获取应用](https://developer.work.weixin.qq.com/document/path/90227)

所有 `allowed_senders`、`allowed_conversations` 和 `scheduled_destinations[].conversation_id` 的去重并集都必须出现在人员可见范围中；该并集限 1–300 个规范成员。配置使用 ASCII 小写，平台返回的 UserID 按同样规则归一；非法或归一后重复的人员条目拒绝启动。平台人员列表最多 4,096 条，响应正文仍受 64 KiB 总上限约束，不能用大列表绕过资源边界。

平台可以用人员、部门或标签配置可见范围。JiaClaw 的部署策略要求上述每个获准成员**显式列入人员可见范围**，不根据部门或标签猜测成员归属；仅通过部门/标签授权不能通过此启动门槛。额外部门/标签不代替本地精确白名单，这不是企业微信官方要求所有应用必须采用的授权方式。

整体启动校验最多 30 秒；启动 token 获取（含刷新锁等待、请求及正文）和应用查询分别最多 5 秒。三类平台响应（token、应用身份、发送回执）都要求唯一 `application/json` Content-Type、有界正文、原始 JSON 对象和关键字段无重复；不能先转成通用 JSON 值丢失重复字段，也不接受位置数组。请求 Secret/token、完整 URL、平台正文及解析错误不会写入诊断、数据库或管理 API，失败只返回固定脱敏信息。

启动证据仅证明该凭证当前对应启用应用，以及配置成员当时显式可见；发送前不重复查询动态可见范围。管理员须持续维护平台授权，后续许可或可见范围变更仍可能使发送失败。启动校验不证明成员基础接口许可、可信出口 IP 长期稳定、公开 HTTPS 回调、客户端展示或真实收发资格。

## 验签、加密与确认

GET 验证请求携带 `msg_signature`、`timestamp`、`nonce`、`echostr`。服务进行一次 URL 解码、校验签名与时间窗，解密并核对尾部 CorpID，在 1 秒内原样返回 challenge 明文；不添加 JSON 引号、BOM 或换行。

POST 请求也必须通过签名和时间窗校验。签名为 `SHA1` 对回调 Token、timestamp、nonce、密文这四个字符串排序后拼接的结果，不是 HMAC。EncodingAESKey 解码为 32 字节 AES 密钥，IV 为该密钥的前 16 字节；CBC 明文由随机 16 字节、4 字节网络序消息长度、XML、接收 CorpID 组成，采用 **32 字节边界**的 PKCS#7 填充。它与飞书的密文格式不同，不能直接复用飞书解密逻辑。[官方加解密说明](https://developer.work.weixin.qq.com/document/path/90968)

服务核对解密后的 `ToUserName`、`AgentID` 和尾部接收 CorpID，并检查外层标识的一致性；仅靠未加密的外层 AgentID 不能认证应用身份。XML 解析拒绝 DTD、外部实体、重复关键字段和畸形结构，并限制输入大小。只有通过授权的文本消息触发 Agent；非文本和事件不会触发新的工具执行。

合法文本先持久化，然后返回 **HTTP 200 空响应体**，后台执行模型和发送消息。官方要求 POST 在 5 秒内响应，否则会断开并最多重试三次；允许返回空体，再通过主动消息接口异步回复。重复消息按安装范围内的 `MsgId` 去重，即使重新加密或签名 nonce 改变，也不重复执行。相同 MsgId 的发送者、目标或文本变化会被拒绝。数据库不可用时不能先确认再尝试保存。[官方回调与重投说明](https://developer.work.weixin.qq.com/document/path/90238)

会话按平台、CorpID/AgentID、规范成员 ID 隔离，仍使用该服务的共享工作空间。后台工具白名单、执行取消边界、重启恢复和存储容量与[可靠渠道](channels.md)一致；这不等于每用户独立的操作系统沙箱。

## 主动回复与投递含义

服务使用 `GET /cgi-bin/gettoken?corpid=...&corpsecret=...` 获取应用 access_token，按 `expires_in` 缓存并预留刷新时间。并发刷新合并为一次请求，获取过程有超时和失败退避；token 只存在内存，重启后重新获取。消息通过 `POST /cgi-bin/message/send?access_token=...` 发出。查询参数中包含凭据，因此诊断和错误不能保存完整请求 URL；管理 API、会话和发件箱不会返回凭据。[官方 access_token 接口](https://developer.work.weixin.qq.com/document/path/91039)

每次只发送一个精确 `touser`，携带绑定的整数 `agentid`、`msgtype:text` 和文本内容，不发送 `toparty`、`totag` 或广播目标。禁用 `enable_id_trans` 和 `enable_duplicate_check`；后者按内容合并重复请求，不能区分两个独立业务事件中相同的回复，因此不能替代持久发件记录的身份。[官方发送应用消息](https://developer.work.weixin.qq.com/document/path/90236)

回复总量仍有 16 KiB UTF-8 上限。文本中的 ASCII `<`、`>` 在发送时替换成全角字符，避免被解释为内联链接或控制标签；按**渲染后的 UTF-8 字节数**拆分，每片不超过 2,048 字节，保留 Unicode 字符和完整内容，超限不静默截断。

企业微信的 `delivered` 表示接口返回 `errcode:0`、非空合法 `msgid`，且 `invaliduser`、`invalidparty`、`invalidtag`、`unlicenseduser` 等失败列表均为空。它是**平台接受记录，不是终端展示或已读证明**。回执带有失败成员、字段矛盾或缺少有效 msgid 时不能记录为完整成功。

官方对同一应用、同一成员规定每分钟 30 次、每小时 1,000 次接收上限，应用每天最多发送企业账号上限乘以 200 人次，超额消息会被丢弃、不下发。为限制额度消耗，JiaClaw 使用更保守的持久化安装级限额：每次已知发送完成后至少等待 4 秒，再领取下一片；每个安装最多保留 200 次有效发送预留。已知结果从完成时开始占用 24 小时，不能以较早的领取时间释放额度。多片回复逐片消耗预算，失败尝试也不返还；正在发送和结果未知的预留不会自动到期，重启或删除审计不会重置额度。达到本地额度后待发送记录继续保留，等待额度恢复，不通过删除不确定记录绕过限制。此保护只统计 JiaClaw 的发送；如果其它系统共用同一应用，无法据此保证平台剩余额度，因此专用应用和独占发送权是部署要求。[官方访问频率限制](https://developer.work.weixin.qq.com/document/path/90312)

额度不足时，尚未提交的平台请求保留为 `pending`、`attempts:0`，`error` 为 `wecom_daily_budget`，`next_attempt_ms` 表示已结算额度最早释放的时间，实际领取还须满足冷却和人工核查条件。独立额度账本最多保存 10,000 条有效预留，防止频繁更换安装耗尽存储；达到该容量时显示 `wecom_budget_capacity`，等待已结算记录满 24 小时释放。结果未知时，同一安装的其它成员投递也停止领取并显示 `wecom_delivery_review_required`；须先核实原请求的结果及执行终止，再人工处理，不能假设网络超时代表平台停止执行。管理员通过现有投递 API 查看这些字段，不需要删除消息或重启服务来恢复额度，其它平台仍可继续发送。

持久额度依赖可靠、同步的系统时钟，运行中不得人为前拨时钟来释放额度。恢复旧数据库备份会丢失备份后产生的预留，不能据旧快照推断平台没有收到消息：恢复期间保持服务停止，独立核实旧进程所有在途和未知请求已结束，再从最后一次可能被接受的发送起至少等待 24 小时，才重新启用发送。备份恢复不能撤销外部平台效果。

发送器不会隐藏重试或跟随重定向。连接超时 2 秒、发送单次 HTTP 总超时 10 秒、响应体上限 64 KiB，并遵守上述原始 JSON/MIME 合同。企业微信当前未核实出可直接复用的可信 429 重试等待契约，因而未知 HTTP 400、HTTP 429、5xx、连接中断、发送超时以及矛盾回执都进入 `unknown`，不自动重发。明确的权限或参数拒绝使用审核后的错误码白名单记录 `permanent_failed`。

`40014` / `42001` 等已确认的 token 错误只会使缓存对后续独立消息失效，当前消息不会刷新凭据后重新发送。失效或获取失败后的 30 秒退避期间，新投递可能明确失败为 `credential_unavailable`，不会自动排队等待后补发。普通发送路径的单次凭据获取含刷新锁等待的预算仍为 12 秒，与启动校验的 5 秒门槛分别计时。

## 恢复与定时通知

Agent 完成时，会话、事件终态和完整发件箱在同一个 SQLite 事务内提交。重启发现 `processing` 时转为 `needs_review`，不重跑可能有副作用的工具；发现 `submitting` 时转为 `unknown`。平台接口没有持久业务幂等键保障，不能因为 JiaClaw 没有收到回执就再次发送。

通过 `GET /api/channels/deliveries` 检查发件审计。核对平台接受记录后，管理员可调用 `POST /api/channels/deliveries/{id}/resolve`，提供 `{"action":"delivered","receipt":"msgid 和核对说明"}`；或以 `{"action":"cancel"}` 取消该事件/运行的剩余片段。企业微信在人工确认或取消前，还须核实原请求已结束、不会再被平台接受；缺少该证据时应保留 `unknown`。人工操作以核查时间开始新的 24 小时额度保留和 4 秒冷却。管理员操作不重发，也不撤回已经被平台接受的消息。完整的容量、墓碑、清理和失败健康状态见[可靠渠道](channels.md)。

若凭据失效、应用停用或成员可见范围不足，启动门槛会拒绝整个 `serve`，不能依靠坏 Secret 启动 HTTP 诊断。先停止原进程并保护原数据库及 sidecar；若需使用既有管理审计，维护配置须保留同一数据库和原管理 API 授权，显式移除渠道安装与**全部平台凭据**（包括环境变量），关闭 scheduler、HEARTBEAT 和入站 webhook，避免恢复发送或其它后台执行。既有无渠道路径会保留历史审计，将中断的 processing/submitting 分别转为 needs_review/unknown，并不启动渠道 worker；它不会清空额度、自动解除核对或重放模型/消息。本批真实进程已验证同库/原 API Token 下的无凭据、`http.channels=[]`、scheduler/HEARTBEAT 关闭配置：原 unknown 审计与完整额度账本不变，没有 token、应用查询、模型或发送请求；恢复原配置后重新校验安装，仍不重放 unknown。该本地证据不证明原平台请求已终止，人工核对仍须独立取得证据。

入站成员白名单不自动授予定时通知权限。启用 scheduler 后，在对应安装下增加精确目的地：

```toml
[[http.channels.scheduled_destinations]]
conversation_id = "alice"
```

JobSpec 的 `delivery` 示例：

```json
{
  "channel":"wecom",
  "installation_id":"ww_your_corp_id:1000002",
  "conversation_id":"alice",
  "thread_id":null
}
```

Job 的工具集合必须是安装 `enabled_tools` 的子集。创建、恢复、执行和发送时都重新校验授权；通知未知或永久失败会暂停任务，未解决结果禁止恢复和重叠执行。暂停或删除任务不会取消已入库通知，需要显式取消对应运行的投递。任务及运行审计 API 见[定时通知](scheduled-delivery.md)。

## 验收边界

```sh
cargo build --locked -p jiaclaw-host
python3 tests/wecom_startup.py target/debug/jiaclaw
python3 tests/wecom.py target/debug/jiaclaw
```

脚本仅使用临时数据库、本地模型/平台 HTTP fixture 和一次性密钥，OpenSSL 独立构造 AES 回调。覆盖 GET/POST 验签和时限、企业/应用身份、XML 实体拒绝、成员大小写归一、重投与冲突、原生工具闭环、相同独立消息、渲染字节分片、token 复用、发送间隔、未知结果与强杀恢复、精确定时接收人、环境变量凭据及错误脱敏。加密单元测试还使用官方 Java 示例中的固定向量；官方示例包可从[企业微信官网下载](https://open.work.weixin.qq.com/wwopen/downloadfile/java.zip)，本次读取的 SHA-256 为 `4a1644d08db8a2b489e79281925dedb2a4788a082fda7d311b17ee942f03192f`。

新增启动 fixture 的六组和 32 个原始 JSON/MIME/HTTP/身份/可见范围/大小负例已在同一最终二进制通过，验证实际 `serve` 失败时没有 HTTP 监听、Agent/MCP 请求、会话库或后台效果，并覆盖真实阻塞请求的 SIGTERM 与 token 头/app 部分正文的五秒截止。完整默认并行 Rust 1105 项、fmt/必需 Clippy/locked build 通过；同一冻结二进制的 WeCom、MCP、e2e、channels、scheduled_delivery 五套既有回归全部退出码 0，包含 WeCom 的加密、发送、恢复、定时和停发维护。两次测试观察/断言修订及精确证据见[验证记录](validation.md)。本提交准备时最终跨平台 CI pending，固定 head 结果以本批 draft PR 为准；不把本地 fixture 外推为真实安装认证。

真实上线还须验证企业认证、生产 API 与应用可见范围、成员许可、可信出口 IP、公开 HTTPS 回调、真实成员收发、应用 Secret 对应的 AgentID、接收额度及终端展示。正常运行期 token 到期刷新与容器内渠道 runtime 压力也尚未取得真实平台认证。本地测试不能替代真实安装认证，本次未使用真实凭据或发送真实消息。Brokerrouter 的真实模型供应商认证及 StateKnot durable 契约仍单独跟踪，见[上游能力差距](brokerrouter-gaps.md)。完整独立用户企业微信还需永久 registry/后端 owner、私有队列、跨渠道用户授权和撤销后停机核对接线。
