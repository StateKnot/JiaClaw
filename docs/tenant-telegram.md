# 独立用户 Telegram 私聊

这是默认关闭的独立用户渠道入口：管理员将一个 Telegram Bot、一个人的数字 ID、一个网关用户及其既定后端永久绑定。该人的私聊由专属后端执行，使用自己的工作区、记忆、会话 SQLite 和 Brokerrouter Key。网关保存私有收件箱、回复队列与操作关联，并与 Web 聊天、网关定时任务共用用户启用状态、未知写入 hold 和执行容量。它不开放群聊、线程、多成员 Bot、任意工具或定时 Telegram 通知。

基础隔离与运维必须先满足[网关部署](gateway.md)。本轮本机 918 项 Rust、fmt/Clippy/锁定构建及最终二进制七组整机验收已通过，具体证据与 CI 范围见[验证记录](validation.md)。协议 fixture 不代表真实 Telegram 安装、供应商工具闭环或 StateKnot durable 认证。

## 身份和执行权限

- 每个 registry 用户最多绑定一个 Bot；同一 Bot 不能绑定其他用户。backend 来自既有用户映射，不能通过消息或绑定配置指定。绑定 UUID、user/backend/bot/sender 不可修改；撤销也不释放用户/Bot 预留，不支持换用户重新收养旧队列。
- bot_id 与 sender_id 使用无前导零的正整数数字 ID，不接受用户名。私聊的 `from.id` 和 `chat.id` 必须都等于绑定的 sender_id，`from.is_bot=false`，不接受线程或群聊。只处理 `message.text`，其他更新类型忽略。
- 后端内部接口仅接受自己的 Bearer Token，并固定会话 `tg-<binding UUID 去连字符>`、`ModelPurpose::Channel`、工具 `datetime_now` / `json_query` 和关闭自动 skills。消息不能扩大工具权限、选择其他租户或改变模型路由。该会话仍属于同一用户，可由其既有会话管理入口查看；它不隔离这个用户自己的 Web 操作。
- 用户 Key 轮换、撤销或签发只读 Key 不会撤销 Bot 身份；Key 的只读权限只约束持该 Key 的 HTTP 请求，不改变独立绑定的后台授权。要阻止未来准入，禁用用户或撤销绑定；撤销不能撤回已提交的模型/平台请求。用户重新启用不清 hold、不恢复撤销绑定。
- 单个后端最多一个共享执行请求；全局容量取网关配置。网关持久准入后只发一次后端 POST，客户端断开不会提前释放后台工作许可。后端执行预算 120 秒，网关内部请求期限 150 秒；配置 `request_timeout_seconds` 因而必须至少 150。

## 显式启用

保留原有示例默认关闭。在每个要接入的独立后端现有 `[http]` 中加入下列键，不要重复创建 TOML 表：

```toml
gateway_channel_chat = true
```

后端必须启用 SQLite、API Token，并使用 brokerrouter（离线 fixture 可用 stub）。必须关闭 `http.channels`、旧 `/hooks/inbound` 的 webhook secret 和 HEARTBEAT；scheduler 要么关闭，要么已经配置为 gateway_driven。两个所需工具必须仍已注册。生产样例继续禁用 exec/MCP；本渠道不会获得它们的调用权限。网关启动检查后端身份和内部 channel 协议，不匹配即拒绝启动。

先按[用户管理](gateway.md#用户与-key-管理)创建两个后端各自的用户，再在可信管理面运行：

```sh
jiaclaw gateway telegram-bind --config /etc/jiaclaw/gateway.json \
  --user YOUR_ALICE_USER_UUID --bot-id YOUR_ALICE_BOT_ID --sender-id YOUR_ALICE_TELEGRAM_USER_ID
jiaclaw gateway telegram-bind --config /etc/jiaclaw/gateway.json \
  --user YOUR_BOB_USER_UUID --bot-id YOUR_BOB_BOT_ID --sender-id YOUR_BOB_TELEGRAM_USER_ID
jiaclaw gateway telegram-bindings --config /etc/jiaclaw/gateway.json
```

在线身份管理命令使用现有 `docker compose exec -T gateway` 前缀；将 `YOUR_*` 替换为核实的 ID。CLI 返回绑定 UUID，不接收 Bot Token。准备互不相同的两个 Bot Token 与两个 webhook secret，均与后端 Token 分离，保存为 Secret 文件；后端容器不能挂载这些文件。Bot Token 的数字前缀须匹配 bot_id。webhook secret 应为随机 32–256 个 `A–Z a–z 0–9 _ -` 字符，不能使用 Bot Token 代替。

停止网关，修改后端配置并重启后端。将 [gateway.telegram.json.example](../deploy/gateway/gateway.telegram.json.example) 的 `telegram` 数组合入部署用 gateway.json，替换两个绑定 UUID，保留原 registry/backend 映射。默认 `api_base=https://api.telegram.org`，生产不能改为其他 origin；`allow_loopback` 只供离线 fixture，必须保持 false。Secret 必须是绝对路径、普通单链接文件，无 world 权限，建议沿用 UID/GID 10001 可读的 0440 私有挂载。

[compose.telegram.yaml](../deploy/gateway/compose.telegram.yaml) 仅向网关增加四个 Secret 挂载，不启用后端开关或替你创建绑定。启用时所有 create/up/run 命令都要同时传入两个 Compose 文件；沿用原有有限额卷、网络 guard 和镜像 digest：

```sh
docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env \
  -f deploy/gateway/compose.yaml -f deploy/gateway/compose.telegram.yaml create
python3 deploy/gateway/network_guard.py apply --project jiaclaw-users
docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env \
  -f deploy/gateway/compose.yaml -f deploy/gateway/compose.telegram.yaml up -d
```

四个文件名见 overlay；只启用一个用户时删去第二条配置和对应 Secret。不要让 Compose 自动创建缺失的 Secret 文件，不提交真实密钥。网关需受治理的 HTTPS 出站访问 api.telegram.org；租户后端仍保持各自内部网络。已有网络 guard 不是公网 egress 白名单，必须独立配置并验证宿主防火墙/出站策略。

## HTTPS webhook 与 ACK

由管理员在可信 TLS 入口暴露 `POST /hooks/telegram/<binding UUID>`。这一网关路由与单实例 `/hooks/telegram` 不同；没有匹配配置时不处理消息。每个请求必须恰好有一个 `X-Telegram-Bot-Api-Secret-Token`，与该绑定 Secret 匹配；浏览器用户 Key 不能替代它。代理保护此头、Bot API URL 中的 Token 和消息正文，不记录它们，也不允许绕过 TLS 入口直连。

使用 Telegram 官方 `setWebhook` 手动登记精确 HTTPS URL、上述 secret_token、`allowed_updates=["message"]`，建议 `max_connections=1` 匹配每绑定单个入站槽。先让获准用户与自己的 Bot 建立私聊，再核验终端接收。不要为安装方便丢弃待处理更新或把未知历史发送当作未发生。Telegram 文档允许 secret_token 长度 1–256，应用主动收紧到 32–256；设置 allowed_updates 不会立即排除此前已产生的旧更新。非 2xx webhook 会被平台有限重试，具体次数/时序不由应用保证。[官方 setWebhook 合同](https://core.telegram.org/bots/api#setwebhook)

JSON 入站最多 64 KiB，正文读取 10 秒，text 最多 16 KiB UTF-8；入站并发全局 8、每绑定 1。health 不是队列积压或剩余容量报告。验证身份和 SQLite 持久写入后才 ACK，ACK 不表示模型已执行或消息已送达。在本库去重保留期内相同 update_id 不重复运行；同 ID 内容冲突不会用新内容覆盖旧事件。队列满或持久化失败不以成功 ACK 吞消息。

## 持久状态、容量与未知结果

每个绑定的文件固定为 registry 同级 `telegram/<binding UUID>.sqlite3`，目录 0700、文件 0600、拒绝符号/硬链接及特殊文件，服务持生命周期排他锁。owner protocol 1 校验不可变身份；内部 SessionStore 仍为 schema 10，registry 当前为 schema 3（只读 Key 迁移保留本批 schema 2 的绑定），不能把普通 session DB 或别人的库搬进来收养。

| 边界 | 行为 |
|---|---|
| 每绑定业务队列 | 最多 1000 个事件、10000 个投递；去重记录最多 10000 条，最短保留 7 天；满额拒绝新增 |
| 操作关联 | 最多 16000 条，不自动裁剪；满额停止执行/发送，入站仍可在事件限额内持久排队，管理员须离线清理已解决事件；同一事务写入 event processing / delivery submitting 与运行时 UUIDv7、对象、attempt；插入失败撤回整个 claim |
| SQLite 主文件 | max_page_count 限约 64 MiB；已有超限库拒绝打开，条数满不禁止离线审计；写入超限报错，purge 仍需可完成事务的磁盘余量 |
| WAL | FULL 同步、busy 250 ms、每 256 页自动 checkpoint、journal_size_limit 2 MiB；最后一项是回收目标，不是 WAL 硬磁盘配额；启动拒绝已有超过 128 MiB 的 WAL/SHM/journal 文件，须由管理员离线处理 |
| 发送 | 总回复最多 16 KiB、最多 16 片，每片最多 2000 UTF-16；纯文本，无 parse_mode，关闭链接预览；固定 Bot API，单次请求无重定向/代理/自动重试 |

多个绑定虽各有私有数据库，仍共享网关进程与网关有限额盘，**没有逐绑定物理磁盘隔离**。主文件限制不包含 WAL、日志、registry 或备份；必须给网关卷设置真实总字节/inode 限额、留余量并监控/checkpoint。目录权限保护后端用户，不能防拥有网关运行身份的管理员。物理满盘时核对后由管理员增加容量或释放无关安全文件，不能删未知操作记录来恢复执行。

发送以已持久 submitting 为前提，成功回执须匹配 chat/message ID。明确 429 且有合法 retry_after 才持久排队等待，最多 5 次；发送基本间隔及冷却跨重启保留，retry_after 以秒解释。[官方 ResponseParameters](https://core.telegram.org/bots/api#responseparameters) 终态错误、第五次限流、断连或未知回执均保留用户 hold；未知发送不自动重发，后续同用户执行也暂停。平台 API 接受不是用户已读证明。

后端会话提交与网关事件/回复事务位于不同数据库。后端已完成但响应丢失时，网关不能自动补跑：事件进入 needs_review，待发送请求遗留 submitting 则在重启后变为 unknown。registry 重启恢复同样保留 needs_review。内存任务取消、换 Key、重启或启用用户均不能清除 hold。新入站可继续有界排队，但有 hold 不开始下一轮执行/发送。

操作 UUID 由内部运行时生成，不是公开幂等/恢复 API。关联记录仅覆盖保留期，purge 删除内容及关联；registry 保留必要的有限管理审计，不能用它恢复被删除的业务结果。更换/回滚 registry、队列库或绑定文件都不是撤回外部效果。备份须先停网关、确认并停止后端工作，再一致保留 registry、整个 telegram 目录、各租户状态与 Secrets；恢复陈旧快照须逐项核对之后的消息、撤销和未知请求，禁止直接开放入口。

## 离线核对、取消与清理

这些维护命令要求网关停机，进程锁拒绝与运行中的服务并用；先保持 HTTPS 入口关闭或不可成功 ACK，排空网关，再确认后端已空闲。停止服务不证明此前平台请求未被接受。

下列命令在同一可信部署身份和挂载下执行。容器停止后可用 `docker compose ... run --rm --no-deps gateway` 前缀（保留两个 `-f`、env 文件和固定 project），替换下面的 `jiaclaw`；不要用会启动新的后台 gateway serve 的命令：

```sh
jiaclaw gateway telegram-inspect --config /etc/jiaclaw/gateway.json \
  --binding YOUR_BINDING_UUID --kind events --limit 20 --offset 0
jiaclaw gateway telegram-inspect --config /etc/jiaclaw/gateway.json \
  --binding YOUR_BINDING_UUID --kind deliveries --event YOUR_EVENT_UUID
jiaclaw gateway telegram-inspect --config /etc/jiaclaw/gateway.json \
  --binding YOUR_BINDING_UUID --kind operations
```

events/deliveries 可能含私人正文，保护终端输出和归档；operations 仅含关联元数据。每页 1–100 条，events/deliveries offset 最多 10000，operations 最多 16000。用 request_id 将 registry hold 关联到实际 event/delivery/attempt，核对后端的 tg 会话、模型账务与 Telegram 消息，不能仅凭 HTTP 超时判定未送达。

只在已独立确认 unknown 投递送达后记录证据：

```sh
jiaclaw gateway telegram-resolve --config /etc/jiaclaw/gateway.json \
  --binding YOUR_BINDING_UUID --delivery YOUR_DELIVERY_UUID --receipt '受保护证据引用与核对结论'
```

证据为 1–4096 UTF-8 字节且不含控制字符。此命令只改本地回执为 delivered，不再发送；可能解除后续分片的队列阻挡，但用户 hold 仍需明确核对解除。若决定放弃该来源剩余工作：

```sh
jiaclaw gateway telegram-cancel --config /etc/jiaclaw/gateway.json \
  --binding YOUR_BINDING_UUID --event YOUR_EVENT_UUID
jiaclaw gateway review-clear --config /etc/jiaclaw/gateway.json \
  --user YOUR_USER_UUID --confirm-backend-idle --note '核对的对象、证据与处理结论'
```

cancel 作用于整事件的未完成投递并标记人工审查，不删除已送达回执，也不撤回外部效果。review-clear 会拒绝尚未审查的事件、submitting/unknown/失败终态投递；先处理这些状态。清 hold 后，保留的 received/pending/retry_wait 可继续执行，必须先确认这些后续工作仍获授权。

只有完整结束或已核对取消的事件才能 `telegram-purge --binding ... --event ...`（同样传 `--config`），原子删除业务内容和操作关联，同时保留既有去重保留规则。绑定撤销后仍可离线审计/清理，但不能重新启用绑定。要永久撤销用 `telegram-revoke --config ... --binding ...`，并在下一次启动前从 gateway.json 去掉该条；不要删除 registry 行或库文件绕过预留/hold。

## 验证

```sh
python3 tests/tenant_telegram.py target/debug/jiaclaw
```

本批存储定向七项和原有 channel_store 35 项已通过；覆盖 owner/锁/链接拒绝、claim 回滚及 SQLITE_FULL、重复关联 ID、容量满后审计/清理、重启恢复、purge 外键级联和发送提示。最终二进制整机七组通过：真实双后端及 native 工具、channel 路由、私有会话/队列、用户禁用与永久绑定撤销、未知发送离线核对、跨重启 429 冷却及第五次停止、SIGKILL 后保守 hold 与后端继续完成，均不自动重放；本批七套既有进程回归通过。PR #80 最终 head `68b3a22867e65ed32154c4fc2292066da6f842b4` 已通过 [CI 37113957145](https://github.com/jiawenyao401/JiaClaw/actions/runs/37113957145)，含 Linux/macOS、Chromium 与真实容器。真实 Telegram TLS/webhook、用户终端收发、代理配置及供应商计费需要单独授权验收。
