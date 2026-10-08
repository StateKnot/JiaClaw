# 独立用户企业微信私聊

可信管理员将专用企业微信自建应用、一个成员及网关用户的独立后端永久绑定。默认关闭；用户工作区和会话由独立后端保存，网关每绑定保留私有 inbox/outbox、原请求 UUID 与安装发送额度，并与 Web、cron 和其他渠道共享当前用户授权、hold 和执行容量。独立后端、私有网络和限额卷的基础部署见[网关部署](gateway.md)。已通过最终冻结二进制的本地十组、完整 Rust、旧21套回归及 [PR #87](https://github.com/jiawenyao401/JiaClaw/pull/87) 固定提交三项 CI；真实企业与启用渠道 runtime 的容器认证仍独立。

每个用户最多一个终身企业微信绑定，每个 `CorpID:AgentID` 也只能绑定一次；撤销不释放身份，registry 最多 32 个终身绑定。同企业下两个不同自建应用可以分别绑定两个用户。仅接收绑定人的应用文本私聊；不接应用群聊、智能机器人、群 webhook、第三方套件、客户群、微信客服、个人微信、媒体或独立用户定时通知。standalone 多成员及定时通知另见[企业微信渠道](wecom.md)。

## 安装和启动身份

企业管理员在管理后台创建**专用自建应用**，记录真实 CorpID、正整数 AgentID、该应用 Secret，并将绑定成员显式放入人员可见范围，配置可信出口 IP。JiaClaw 必须独占这个应用的发送权；其它系统共用应用会消耗 JiaClaw 无法观测的额度。应用可见范围、成员许可和企业认证由平台管理，不由网关绑定授予。

启动先完成网关配置及 registry 的正常打开/迁移，逐安装读取本地私密文件，再执行已有官方安装校验：`GET /cgi-bin/gettoken` 获取当前应用 token，随后 `GET /cgi-bin/agent/get` 必须返回整数 AgentID 精确匹配、`close=0`、`errcode=0`，且 `allow_userinfos.user[].userid` 包含规范绑定成员。token 接口不回显 CorpID/AgentID，不能只凭 token 证明应用身份。[官方 token](https://developer.work.weixin.qq.com/document/path/91039)、[获取应用](https://developer.work.weixin.qq.com/document/path/90227)

要求显式人员可见范围是 JiaClaw 的部署策略；额外部门/标签允许存在，但不能以它们推断成员授权。这不是官方要求所有自建应用只能授权人员。平台人员列表最多 4096 条、总响应 64 KiB，归一后重复或非法 UserID 拒绝启动。启动整体最多 30 秒，token（含刷新锁等待）与应用查询分别最多 5 秒，不发送测试消息或自动重发。

**官方身份校验通过后**才打开该安装私有队列并与后端进行 protocol 5 握手；它不意味着启动前所有本地文件零写入，因为 gateway registry 已经打开。随后固定后端 status、工具/时限及完整永久 owner。后端握手整体 10 秒、各请求 5 秒，失败不开 HTTP 监听或后台 worker。已有 owner 预留可能已提交，重试须保持原绑定，不能换 UUID 规避。

启动证明当前应用启用且绑定人当时显式可见；发送前不重新查询动态可见范围，不证明长期许可、可信 IP、公开 TLS、终端展示或已读。正常运行 token 刷新、真实生产 API/安装及容器内 runtime 压力仍须独立认证。

## 配置与公开入口

后端 `[http]` 开启 `gateway_channel_chat = true`，使用独立 SQLite/API Token，关闭 standalone channels、旧 inbound hook 和 HEARTBEAT；scheduler 关闭或 gateway_driven。使用 Brokerrouter（离线验收可显式 stub），固定 channel 路由、120 秒及 `datetime_now`/`json_query`，关闭自动 skills。网关 `request_timeout_seconds` 至少 150。

```sh
jiaclaw gateway wecom-bind --config /etc/jiaclaw/gateway.json \
  --user YOUR_USER_UUID --corp-id ww_your_corp --agent-id 1000002 \
  --human-user-id alice.member@example.com
jiaclaw gateway wecom-bindings --config /etc/jiaclaw/gateway.json
```

UserID 按 ASCII 小写规范化：1–64 字节、首字符为字母或数字，后续仅字母、数字、`_-.@`。绑定从 registry 取固定 backend，不允许回调选择用户、后端、模型、工具或发送目标。将输出的 binding UUID 放入网关顶层数组：

```json
"wecom": [{
  "binding_id": "YOUR_BINDING_UUID",
  "app_secret_file": "/run/secrets/alice_wecom_app_secret",
  "callback_token_file": "/run/secrets/alice_wecom_callback_token",
  "encoding_aes_key_file": "/run/secrets/alice_wecom_encoding_aes_key",
  "api_base": "https://qyapi.weixin.qq.com/cgi-bin",
  "allow_loopback": false
}]
```

三个文件为绝对路径、当前网关 UID 所有、0600、普通单硬链接文件，不跟随 symlink，打开前后核对 inode/权限。通用读取为 1–4096 可见 ASCII 字节及有限末尾换行；Callback Token 进一步要求 **1–32 个 ASCII 字母/数字**，EncodingAESKey 为官方 43 字符并解码成 32 字节。凭据须彼此不同，且与后端 Token 和其它渠道凭据不同。短合法 Callback Token 不能被错误要求至少 16 字节。凭据只挂载给网关，不交给租户；业务 prompt/reply 在私有库及备份中是明文，此入口没有 Discord state_key。

生产 API 使用固定官方 HTTPS origin，不使用代理、重定向或隐藏重试。`allow_loopback=true` 仅用于明确的字面 loopback HTTP `/cgi-bin` fixture。反代精确转发 `GET /hooks/wecom/{binding UUID}` 和 `POST /hooks/wecom/{binding UUID}`，保留查询参数与原始 XML body；不用 JSON 回调。非 loopback 监听须显式 allow_remote_bind，私有后端 API 不对公网开放。先启动网关与反代，再由管理员配置平台回调，完成 GET challenge 验证，避免把尚未验证的 URL 预设成启动条件。

## 回调与持久准入

GET 携带 `msg_signature/timestamp/nonce/echostr`，一次 URL 解码后验签和解密，核对尾部 CorpID、当前 binding 与用户启用状态，原样返回 challenge 明文。POST 签名为四字符串 Token、timestamp、nonce、密文排序后拼接的 SHA1；AES-256-CBC 使用密钥前 16 字节为 IV，明文由随机 16 字节、网络序长度、XML、CorpID 构成，采用 **32 字节边界** PKCS#7。时间窗为 ±5 分钟；核对内外 CorpID/AgentID，拒绝 DTD/外部实体、重复关键 XML/query 字段或畸形结构。[官方加解密](https://developer.work.weixin.qq.com/document/path/90968)

| 边界 | 实际合同 |
|---|---|
| 本地时限 | 整条入口 900 ms、body 650 ms；GET challenge 应在平台 1 秒预算内，POST 官方 5 秒预算另计公网 TLS/反代延迟 |
| 输入 | query ≤8192 字节、原始 POST ≤128 KiB、用户文本 ≤16 KiB；过大 413、正文超时 408，签名/XML 身份失败 401，未授权人或失效绑定 403 |
| 容量 | 全局入站 8、每安装 1、阻塞 I/O 8；busy/队列满 429，存储或整体截止 503，阻塞任务未结束仍保留资源槽 |
| 去重 | 安装范围内的 canonical `MsgId`；换密文/nonce 的同一消息 ACK，不重复执行；同 MsgId 的规范人/文本变化 409 |
| ACK | 合法文本仅在持久准入后 **200 空响应体**；不等待模型或发送。忽略的非文本/事件不创建模型任务 |
| 用户授权 | 后台准入、模型领取和发送重新核对用户 enabled、永久 binding、既定 backend 与共享 hold/执行容量 |

平台重投应保持原 MsgId；成功 ACK 不证明模型完成或终端收发。[官方回调与重投](https://developer.work.weixin.qq.com/document/path/90238) `503 ingress_deadline` 或连接丢失不能证明未接纳，在途 SQL 可能在 HTTP 截止后完成，250 ms SQLite busy timeout 也不是整条回调时限。未知消息必须停机排空、按原 MsgId/操作 UUID 核对，不换新 ID 重跑模型/工具或放弃原未知操作。

API Key 轮换、撤销或只读权限不撤销另行授权的渠道任务；user-disable 或 wecom-revoke 才阻止未来准入/发送。enable 不清 hold、不恢复已撤销绑定。已提交模型/平台请求仍可能完成，不能把 disable/revoke 当作取消外部效果。

## 私有状态、原请求和发送

registry 升级至 schema 7，保留 schema 1–6 的用户、Key、授权、审计、hold 与既有渠道身份。每绑定队列位于 registry 同级 `wecom/{UUID}.sqlite3`，目录0700、文件0600且单硬链接、生命周期排他锁。独立 application ID、完整 protocol 5 owner schema/触发器及 binding/user/backend/CorpID/AgentID/人一致性均校验；enabled 不属于 immutable owner。WAL打开前的immutable预检仅证明固定主文件application/schema/owner；实际queue/quota/recovery继续读取完整WAL，不能将旧主文件快照当成实时队列。原子初始化使用同目录 stage、FULL rollback-journal owner 事务、关闭/同步后原子不覆盖发布并同步目录；部分 stage、热 journal、孤立 sidecar 或异 owner 保留并拒绝，不能删除残留/改 owner 收养。

后端永久 identity 用 `{protocol:5,binding_id,user_id,backend_id,corp_id,agent_id,human_user_id}`，内部 `agent_id` 是 canonical 正十进制 **字符串**；registry summary 和官方 agent/get 的 AgentID 仍是整数。后端启用精确工具/status 后拒绝更换 owner。执行六字段 `{protocol:5,binding_id,request_id,session_id,event_id,prompt}`；`session_id=wecom:{binding UUID}`，同一 request UUIDv7 先在 registry 事务关联用户 hold/audit，随后 private 事务原子领取并关联 operation；发送领取与额度预留也在 private 同一事务。两个库之间不原子：registry 已准入而 private 尚未领取时中断仍保留 hold，须核对原请求；private 插入失败回滚领取，不生成替代请求。

| 状态与资源 | 边界 |
|---|---|
| 事件/投递/去重 | 通用 1000 事件、10000 投递/墓碑，最短去重保留7天；另受永久操作 headroom 限制 |
| 永久操作 | 16000 条模型/发送 metadata，不自动裁剪，业务 purge 保留原 UUID、内部 event/delivery、attempt、结果/回执/核对时间；无 prompt/reply/secret |
| 准入 headroom | 事务保证 `现有操作数 + received×17 + processing×16 + pending片数 +17 ≤16000`；每消息预留一次模型及最多16次单尝试发送，空新库最多941条 received，第942条拒绝，历史操作会减少实际名额；重复 ACK 不因满额被禁止 |
| 后端请求 | 每后端16000个永久 admitted/completed 身份，不删除/复用；网关与后端不能跨库原子提交 |
| SQLite | 主文件约64 MiB page quota、FULL、busy250ms、256页 checkpoint、2 MiB journal 回收目标；启动 sidecar 上限128 MiB，目标不是硬磁盘限额 |
| 模型/结算 | 后端120秒，网关完整结果等待150秒；结算 I/O 等待有界，任务取消不提前释放尚在执行的阻塞槽 |
| 文本/发送 | 回复总16 KiB UTF-8、渲染 `< >` 为全角后每片≤2048字节、最多16片；token含锁获取12秒、单次消息HTTP10秒、总发送25秒、64 KiB原始JSON/唯一MIME/重复关键字段拒绝 |

每次精确发送绑定人 `touser` 和整数 `agentid`，禁用 id 转换、内容重复合并，不发送部门/标签/广播。独立相同回复仍为两个真实请求。接口成功须 errcode0、有效非空 msgid 且所有失败成员列表为空；delivered 是平台接受，不是终端展示/已读。[发送应用消息](https://developer.work.weixin.qq.com/document/path/90236)

每安装最多200个有效发送预留；已知结果从完成或人工核对时间起占用24小时，下一片至少等待4秒。NULL `settled_ms` 的在途/未知预留**永不过期**，重启、撤销、删配置、业务 purge 均不释放。失败尝试也占额。当前消息不自动重试；429/5xx/超时/断线/矛盾回执保留 unknown，已知拒绝白名单保留 permanent_failed，均持有人工核查。40014/42001仅使后续独立请求的 token 缓存失效，当前消息不 refresh 后重发。正常额度不足保留待发送，不制造终态；其它用户/专用应用可独立运行。官方额度另见[访问频率](https://developer.work.weixin.qq.com/document/path/90312)，部署不得人工前拨时钟释放额度。

网关 processing 重启转 needs_review、submitting 转 unknown，保留原 UUID，不自动模型/工具/消息重放。后端会话和原请求 completed 同事务提交，metadata 只证明该事务状态，不能用于合成丢失回复或证明平台收到。网关 private 队列不存后端 session 历史。逻辑私有库仍共享网关限额卷，没有逐绑定物理磁盘隔离；总字节/inode/WAL/日志/备份配额、事务余量和监控由部署提供。

## 停机维护与撤销

先关闭公网回调、停机并排空网关，再确认原后端/平台请求终止。以下队列维护取得同一停机锁，在线拒绝；绑定查询/不可逆撤销使用 registry 短事务。保持 registry 和队列路径，不需要配置安装或任何 WeCom Secret；完全无状态返回空页且不创建私有库，残留 stage/sidecar 不能误报无状态，已撤销 binding 仍核对 retained owner/未知额度。

```sh
jiaclaw gateway wecom-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind events
jiaclaw gateway wecom-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind deliveries --event YOUR_EVENT_UUID
jiaclaw gateway wecom-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind operations
jiaclaw gateway wecom-inspect --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --kind reservations
jiaclaw gateway wecom-resolve --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID \
  --delivery YOUR_DELIVERY_UUID --receipt '已独立核对原平台 msgid 和执行终止'
jiaclaw gateway wecom-cancel --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway review-clear --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID \
  --confirm-backend-idle --note '已核对原请求、额度及全部保留渠道队列'
jiaclaw gateway wecom-purge --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway wecom-revoke --config /etc/jiaclaw/gateway.json --binding YOUR_BINDING_UUID
```

inspect 默认limit20、1–100；operations offset≤16000，其余≤10000，返回 `result[kind]`。events 平台 MsgId 位于 **`spec.event_id`**，operations/deliveries 的 event_id 是内部事件 UUID。operations 为原 request/kind/event/delivery/attempt/claimed_ms/state/receipt/reviewed_ms；reservations 为 installation_id/delivery_id/attempt/reserved_ms/settled_ms。业务详情仍含私密文本，只供可信维护管理员，不把 metadata 称为脱敏完整历史。

可信后端管理员按原 private Bearer Token 查询 `GET /internal/channels/wecom/requests/{原 UUIDv7}`，仅返回 protocol/backend/binding/request/event/session/status，无 prompt/reply 或 replay 方法；status 是 admitted/completed。status、binding、execute、requests 均不在公开用户 API 白名单。

resolve 仅记录核对回执，不发网络请求；cancel 取消未提交部分并显式核对未知额度，不能撤销外部效果；purge 仅清已解决业务内容，保留去重墓碑、永久操作和全部有效额度。review-clear 核对该用户**全部保留渠道**，包括撤销/无runtime的 WeCom；未知事件、submitting/unknown/permanent_failed 或 NULL额度均阻止清 hold。确认原执行终止并处理全部未知后才清 hold，不能通过改配置、换应用、删除业务库或清 ledger 获得重试。

停机一致备份 registry、所有渠道目录与残留、后端状态和凭据，保护明文文本。旧快照会回退授权/撤销/hold或丢失外部收据；恢复时保持入口和 worker 停止，独立核对恢复点之后的外部效果，不直接恢复公网发送。WeCom 配额恢复还须从最后可能接受的发送起保守等待至少24小时；不能据旧快照推断NULL请求已结束。旧二进制拒绝schema7。

## 验收范围

```sh
cargo build --locked -p jiaclaw-host
python3 tests/tenant_wecom.py target/debug/jiaclaw
```

新脚本使用两个真实独立后端、实际网关/native Brokerrouter工具往返、独立OpenSSL加密、临时SQLite及一次性本地平台凭据。十组覆盖安装门槛、原MsgId重加密去重、跨用户/只读Key、模型后禁用、未知多片hold、重复独立消息/4秒间隔、实际模型和发送POST后SIGKILL、原UUIDmetadata、无Secret停机核对、永久撤销、NULL额度及真实headroom/写锁持锁停机。包含合法原始后端DTO的唯一MIME正例/重复头负例、实际partial-body 408与安装间429 busy隔离。最终冻结二进制十组一次全部通过（143.05秒），默认并行Rust 1158项、fmt/必需Clippy/locked build通过。首轮DTO层级和维护恢复观察次序错误仅修测试，生产headroom、owner/WAL预检及私有请求/响应合同修复均已按最终源码验证；证据见[验证记录](validation.md#独立用户企业微信批次)。本提交准备时既有21套进程回归仍独立执行、最终CI pending，以本批draft PR固定head的完整结果为准，不把先前PR #86范围替代本批。

不使用付费或真实企业微信凭据，不发送真实消息。公开HTTPS总延迟、真实安装可见范围/许可与客户端收发、正常token到期刷新、接收额度、共享卷/容器渠道runtime压力和真实供应商联合认证仍未完成；本地协议证据不外推生产资格，也不等于StateKnot durable或可恢复工具执行。

2026-10-08 14:39 UTC 已回填 PR #87 的最终固定提交结果，见[验证记录](validation.md#独立用户企业微信最终-ci-回填)。提交准备时的 pending 记录保留为历史，不替代已经完成的固定源码验收，也不代表真实企业/客户端认证完成。
