# 独立用户持久 HTTP 请求

个人 API Key 可显式提交、发现、查询和取消自己的请求。每个用户仍须部署专属后端、工作区、会话库与 Brokerrouter 虚拟 Key，遵守[网关生产部署边界](gateway.md)。本功能默认关闭；工作台在协议2使用有界租户 SSE 预览，在协议1使用 JSON。租户预览 UI 已完成真实接线，PR #104 固定 head 的[独立官方资格](validation.md#pr-104-租户预览-ui-最终-ci-回填)已通过。

## 配置与迁移

网关 `gateway.json` 增加 `"tracked_turns": true`。各专属后端设置：

```toml
[http]
gateway_channel_chat = true
tracked_turns = true
tracked_turn_timeout_secs = 300
persist = true
persist_path = "../state/sessions.sqlite3"
# API Token 使用专属私密配置或 JIACLAW_API_TOKEN，不交给用户。

[model_calls]
enabled = true
store_path = "../state/model-calls/index.sqlite3"
```

同时配置 `provider_type = "brokerrouter"`、各自模型/虚拟 Key，以及 `agent.tool_timeout_secs` 1..30。关闭独立渠道、HEARTBEAT 和兼容 webhook；若启用 cron，必须是 `scheduler.gateway_driven = true`。保持 Compose 的私网、卷限额、资源和 Secret 边界；现有部署样例不自动打开新功能。

启动时和每次新准入前都检查私密后端的协议、精确 agent 名称、租户模式、单活动 owner、结果和流式预算与 gateway_protocol=2 模式。SSE升级须网关与全部启用 tracked_turns 的后端使用同一新版本；旧protocol=1后端拒绝启动/新准入，不能混装后假定支持新合同。无数据库schema增量，registry仍v8、后端仍v11。后端需要网关添加的单一 `x-jiaclaw-gateway-turns: 1` 标记；标记只是模式合同，不能代替 Bearer Token 或网络隔离。独立模式后端拒绝此标记，租户后端拒绝缺失/重复/错误标记。后端关闭 tracked_turns 后仍可读取既有结果；新准入返回 503，不保留新身份或派发模型。

registry 自动事务迁移 schema 1..7 → **8**，保留原用户、Key/撤销、渠道绑定、审计和 write hold。新增每用户永久请求索引，并移除 write_holds 的全局 request_id 唯一约束；一个用户仍最多一个 hold，两个隔离用户可以使用同一 UUID。迁移失败整体回滚；版本和新表/索引/触发器不符时拒绝启动。升级前停机并备份，旧二进制拒绝 v8；不能恢复旧快照来遗忘准入身份、撤销或未知结果。这是不可直接降级的持久迁移。

## 个人 Key 接口

| 接口 | 合同 |
|---|---|
| `GET /api/turns/capabilities` | gateway_protocol=2、streaming=true、固定工具/会话及有限流式预算 |
| `GET /api/turns?limit=20&offset=0` | 当前用户在网关永久保留的准入目录；limit 1..50、offset 0..10000 |
| `PUT /api/turns/{UUIDv4}` | 首次原请求准入；相同正文的旧身份仅 GET 原后端结果 |
| `PUT /api/turns/{UUIDv4}/stream` | 显式新建原请求的有限SSE；旧身份返回200 JSON原GET，不重建流或执行 |
| `GET /api/turns/{UUIDv4}` | 已属于当前用户的原后端私密收据和结果 |
| `POST /api/turns/{UUIDv4}/cancel` | 对自己的原请求持久记录取消意图，再通知实际 owner |

必须携带唯一的 `Authorization: Bearer <个人 Key>`。只读 Key 只允许 GET；PUT/cancel 在读取正文或持久准入前返回 403，数据库操作还重查当前权限、撤销和用户状态。所有响应 no-store/nosniff；路径仅接受规范小写 UUIDv4，拒绝编码路径、后台选择、未知/重复分页参数以及GET stream/review/purge 路由；stream只接受明确PUT。

提交唯一 JSON Content-Type，普通JSON PUT不接受 SSE Accept；显式/stream PUT可带 text/event-stream Accept。正文最多64 KiB、读取5秒；session_id 必须为 `http:<规范 UUIDv4>`，prompt 非空且最多32 KiB，enabled_tools 必须显式选择非重复的 `datetime_now`、`json_query`（至少一个），enabled_skills 必须为空或省略，未知字段拒绝。例如：

```json
{
  "session_id": "http:3b80c58b-e495-4fd8-a77e-b1579576471f",
  "prompt": "告诉我当前日期",
  "enabled_tools": ["datetime_now"]
}
```

客户端先生成并保存请求 UUID 和确切正文，然后 PUT。新准入通常202，旧身份查询200；返回原协议 `{protocol:1, receipt, active}`。`X-Request-Id` 是本次网关传输编号，原执行身份是 `receipt.id`。工具列表顺序和正文参与规范化哈希，省略 enabled_skills 等同空列表；旧 ID 改正文返回409。原 UUID 贯穿网关索引、用户 hold、后端 HTTP 收据和模型账本 turn_id。

目录的 `scope:"gateway"` 表示**网关准入元数据**，每行只含 id/session_id/created_ms；不伪造后端执行状态，也不证明模型已经提交。分页不是跨页快照，新增身份可能移动后续页。即使网关在转发前断电、后端数据库丢失或管理员已清 hold，目录仍能发现原编号。每用户最多10000永久身份，含未完成和已清理结果；满容量拒绝新请求，不删除旧身份或借换 Key 绕过限额。

## 执行、取消与恢复

首次准入在同一 registry 事务中重查授权、保留永久身份并取得用户唯一 hold，与聊天、cron 和已接线渠道共享互斥。随后只向固定后端发送一次 PUT。重复身份**永远不再 PUT**，包括重启、未知结果、后端404和人工清 hold 后；缺失原收据返回409和原编号，保留索引，不据此推断零效果。后端收据的身份、会话、哈希、协议及状态都须匹配原记录。

网关全局执行容量与每后端一个执行许可由独立实际 worker 持有。JSON HTTP 客户端断开不撤销已经准入的异步执行；取消使用独立 POST。原 `request_timeout_seconds`（10..300）从网关入口开始，不因准入或轮询重置；请求体和每次私密后端I/O另有5秒上限，网关SQLite busy上限仍250毫秒。排队的实际 blocking DB 工作不会因 HTTP 等待者离开释放许可。控制容量全局8、每后端2，超过返回429；后端自身的四控制 owner 也保持有限。

worker 只有在原收据 completed、session_committed=true、error=null、active=false 时，才清除匹配的 in_flight hold。取消、超时、失联、异常合同及结算失败均保留/转为 needs_review；观察截止不等于模型或外部资源已经停止。若尚未得到原收据终态且active=false的本机owner停止证据，网关同时保留全局及该后端执行容量；控制GET/cancel继续可用，其他写入可能429。关闭网关tracked_turns后重启也按HTTP审核hold保留容量，不能用功能开关绕过。网关 SIGKILL 后，in_flight 转为 gateway_restarted 的审核 hold，重启不恢复原观察 worker、不重新派发。tracked_turns仍开启时，用户 GET/cancel 可用；GET 即使看到 completed 也不会清除重启审核hold或容量保留。新网关在绑定入口前按所有未核对hold重建容量，含已禁用用户或未配置的旧后端；其数量达到全局上限时暂停新写入。功能关闭时至少保留已有HTTP hold的容量，原收据由可信管理员直接在私有后端核对。

可信管理员须独立检查原后端 active、HTTP 收据、模型账本和外部资源，再按[网关审核流程](gateway.md)执行 `review-clear --confirm-backend-idle --note ...`。若后端原会话也阻断，分别在后端执行既有显式收据维护；两个数据库不构成跨库原子事务。清hold不移除永久请求、不授权重放原UUID；已经保留的进程内容量不随CLI修改自动释放。管理员确认全部相关后端空闲、清hold后，**重启网关**才能重新领取容量。正常观察到终态/本机owner停止的请求可直接释放执行容量，但失败hold仍须显式审核。普通会话管理 API 不开放 `http:` 命名空间；JSON 客户端以原收据获取结果，工作台仅展示该原请求收据中的回复，完整租户 HTTP 会话历史接线仍开放。

本功能未扩大 MCP/文件/exec/技能或外部发送权限，不提供 StateKnot durable 图、工具自动恢复、供应商/代理认证。真实进程与跨平台证据见[验收记录](validation.md#独立用户-http-原请求批次)。


## 个人 Key 工作台

在根页面连接自己的完整或只读 Key。工作台分别校验网关能力和 standalone 流式能力；协议2采用有界预览，协议1采用下述JSON观察。开启tracked_turns的网关不需要也不探测私密工具/技能目录。网关请求能力被拒绝403、暂不可用503或合同异常时拒绝连接，不退回旧发送路径；只有该功能明确不存在404才保持已有普通会话方式。完整Key新建原请求会话后，显式选择时间/JSON查询工具并发送。原编号只保存在当前地址fragment，密钥、提示词、授权与草稿留在内存；不使用local/sessionStorage。

协议1的JSON流程从首次提交前开始最多观察30秒，后续只GET同一原编号，每次最多15秒，收据读取2MiB+20KiB、目录32KiB、能力16KiB。观察截止、刷新或关闭页面不取消已经准入的异步JSON执行。按“核对结果”查询原收据；取消按钮独立记录原停止意图。响应丢失时保留原编号/草稿，只有明确操作才以确切相同ID/正文再提交，由网关既有永久身份保证GET-only；页面不自动重连或重发。

最后一次读取因剩余观察期限到期而中断时，页面显示“观察已结束”，该收据不再视为最新核对，不据此标记完成或解除跟踪；原编号、草稿与服务端占用仍保留。先命中的独立15秒RPC超时、HTTP拒绝和畸形响应仍报告未确认的请求失败。两种超时依据本页实际定时器及原预算区分，不匹配错误文案、不续期、不增加请求。

刷新后重输Key，再核对fragment中的原编号。遗失编号可读取本用户每页五项的永久准入目录，再选择原GET；目录的准入时间不代表执行状态。只读Key允许目录/原结果查询，不能提交、取消或审核。切换Key清空显示和迟到响应；不将前一身份的原编号带到另一身份。普通会话列表因占用429或后端不可用502/503而暂不可读时，页面明确提示，原编号控制入口仍可使用。

只在已校验原收据完成且有结果时展示回复；该显示不是全会话历史。结果清理或原收据不可用时不恢复历史、不伪造完成、不换编号执行。个人工作台不开放网关或后端的review/purge；未知结果由可信管理员按上文核对。结束跟踪仅结束本页显示，不清用户hold或执行容量，后续准入仍由服务端当前权限和持久状态决定。真实浏览器、故障注入与最终固定head证据见[本批记录](validation.md#租户-json-工作台接线批次)，不据此标记租户SSE、代理/供应商或StateKnot durable完成。

## 个人 Key SSE API

`PUT /api/turns/<原UUIDv4>/stream` 与JSON使用确切同一正文/规范化哈希、永久身份和原用户write hold。新身份只发送一次私密后端stream PUT；重复身份仅GET原收据并返回200 JSON，包括已完成、当前运行、失联、重启及管理员清hold后。没有流续接、自动重连或新编号重放。普通PUT和协议1的Web JSON语义继续保留；协议2的工作台接线见下文。

新SSE响应202，唯一Content-Type为 `text/event-stream; charset=utf-8`，no-store/nosniff/X-Accel-Buffering:no。事件同[持久HTTP SSE](http-streaming.md)：admitted、model_started、preview、model_completed、tool_completed、done；仅在原模型remote ID已存账本后可发预览。预览不表示保存或授权。网关逐帧校验原UUID/会话/请求hash、上下文hash/准入时间、规范model操作/remote UUID、轮次、tool名称与本次显式授权；未知字段/非法顺序/UTF-8/framing/超预算停止交付并核对原编号。最终done使用再次GET获得的原终态收据，只在后端active=false和实际gateway settlement完成后发送；原始后端done不能自行解除网关hold。

网关额外投递容量全局四、每后端二，满时在新持久准入或backend PUT前429；与原执行许可和控制许可分别计数。实际未轮询/慢读HTTP Body和投递actor共同保留槽位；HTTP等待者消失或执行完成不能提前认定Hyper已释放Body。单队列8片、每片最多4KiB；frame最多原receipt上限+64，进/出wire各12MiB、preview每轮2MiB/累计8MiB，最多33个模型轮次。逐帧发送共用该帧原5秒grace与网关入口原request_timeout_seconds截止，分片/heartbeat/新事件均不续期。合法原后端keepalive保持转发并计wire预算；任一不完整已发帧之后只EOF，不拼接伪完整错误。完整error提示或没有完整done都须原GET核对。

投递断线/慢读/畸形合同停止继续读取私密SSE，使native停止未来dispatch；原观察期限尚有余量时，额外以同UUID持久记录一次cancel意图，再仅GET原收据。原模型和已提交操作继续按原remote ID结算，直至原终态/active=false才允许释放执行许可。取消/失败保留审核hold；原截止到期时未能确认停止则保留全局/后端执行容量，人工核对原后端/账本、清hold且重启网关后才能回收。到期已无预算时不保证新增cancel意图已写入，更不声明实际模型立即停止。SSE的HTTP投递槽位只在实际owner消失后释放，即便执行/审核已经完成。

原响应完整校验之前的取消、交付失败或异常合同保留审核hold，即使后续原GET显示completed且active=false。该停止证据允许释放实际执行容量，但不能自动免除协议异常的人工核对。有效原SSE终态或有效200 JSON lookup，再经原GET停止证明与实际gateway成功结算后，终态帧丢失不重新制造hold；客户端仍须以原ID核对结果，不创建替代执行。

授权固定于原准入：撤销Key/禁用用户阻止新请求和后续GET/cancel，不撤销已提交模型或原授权工具的既有事实。客户端保留原ID及确切正文；断线后显式GET，必要时由可信管理员核对。SIGKILL/restart保留原永久索引和审核hold，不恢复预览/图/工具执行；协议升级不提供跨库原子性。真实TLS反向代理、供应商、渠道安装和完整个人Agent生产认证仍独立开放。

## 个人 Key 预览工作台

工作台在认证能力严格声明 gateway_protocol=2、streaming=true 与固定预算时，使用现有增量解析/显示模块发送原UUID的 `/stream` PUT。只选择本次允许的时间/JSON工具；不自动增加授权，不探测管理员工具目录。每轮临时文字独立、textContent显示，保留有限wire/帧/UTF-8/DOM预算；完整done和EOF还须原GET确认，最后的GET与交付共用原期限/取消signal。200 JSON仅表示原身份重复查证，不恢复订阅或派发。具体预算见[Web合同](web-streaming.md)。

断流、刷新、pagehide或换Key关闭实际浏览器delivery，服务器仍按原owner/hold结算；与JSON的断开不取消异步执行语义分别保留。显式取消先持久原停止意图再abort交付。原编号、草稿、只有内存的权限与密钥、只读/跨用户查证和管理员独立核对沿用上文；仅GET的只读Key不会打开流式PUT，完成查看也不清管理员审核hold。

本地验收使用真实嵌入资源、Chromium、两个私有后端/实际网关/SQLite/模型fixture，九组覆盖实际准入后的预结算预览、reload/只读GET、持久取消与真实owner、前端终态授权故障、真实响应丢失后的显式同ID200/GET、换Key的迟到结算和独立用户。第九组保持真实合法done/EOF，只篡改最终GET或重复200的工具名；已在初版复现错误清草稿，最终done、200、GET共享receipt的本次选择核验后全部拒绝，正确原GET才解除跟踪。两项期限用明确前端故障fixture缩短原长timer并延迟headers/final GET；真实服务器/模型/control期限不变，不宣称物理85秒、真实代理或供应商认证。原十组JSON工作台另以严格协议1能力形状的前端fixture，在当前真实JSON后端路径验证兼容，不冒充另一历史版本的实机资格。PR #104 固定 head `7fcdc5f3` 的七官方作业与四实际候选已独立核对通过；后续新提交仍须取得自己的资格。
