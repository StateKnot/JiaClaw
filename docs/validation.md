# 基础能力验收记录

2026-10-02，本机 macOS arm64，Rust 1.85.0，锁定 Cargo.lock。

| 验收 | 结果 | 证据范围 |
|---|---|---|
| `cargo test --workspace --locked` | 560 passed；1 个 Docker 测试默认忽略 | 工具、配置、Provider/渠道 HTTP fixture、SQLite、迁移、并发与 UI HTTP |
| `cargo clippy ... -D clippy::correctness -D clippy::suspicious` | 通过 | 仍有已有 pedantic/style warnings，不声称 warning-free |
| 格式、diff whitespace、TOML/JSON 示例等价解析 | 通过 | 示例真实驱动配置，已移除无效 runtime/server/limits 表 |
| `tests/e2e.py` | 通过 | 实际二进制、API/渠道鉴权、禁用未配置渠道、并发、空会话、SIGKILL 恢复、持久删除 |
| `tests/browser.cjs` / Chromium | 通过 | 连接/创建/聊天/删除、text-only 渲染、无存储 Token、390px 移动布局 |
| 真实 Docker exec ignored test | 独立通过 | 固定 Alpine 摘要；宿主外文件不可见、只读拒写、输出截断、超时、明确授权写入、future 取消清理 |
| `docker build` 与 `tests/container.py` | 通过 | 实际 Linux arm64 镜像、UID 10001、只读 rootfs、会话、命名卷重启恢复 |
| `tests/installer.py` | 通过 | 本地 Release fixture：校验和、原子替换；非法版本/缺校验/损坏保留旧二进制 |

沙箱镜像：`alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507`。验收使用一次性容器/卷，没有真实渠道消息、真实供应商请求或费用。

基础批次的 Linux/macOS 与容器 CI 已通过，见 [PR #58](https://github.com/jiawenyao401/JiaClaw/pull/58)。StateKnot durable 尚未接入；上游工具认证仍跟踪 Brokerrouter #31。StateKnot stdio MCP 需求已提交 #140。当前尚未发布 tag、公开 Release 或推送镜像。

## StateKnot HTTP MCP 批次

同日，本机 macOS arm64，Rust 1.88.0，精确发布版 `stateknot-integrations = 0.1.0-alpha.1` 和锁文件。

| 验收 | 结果 / 证据 |
|---|---|
| 全量 Rust 回归 | 572 passed；1 个 Docker 测试仍为独立 ignored acceptance |
| MCP HTTP/SSE | 10 个实际 StateKnot 客户端网络/资源测试，包含 MRTR、401/redirect、不重放、取消与正则边界 |
| TOML/JSON | 顶层 MCP 解析、显式 effect、未知字段/写 effect 拒绝 |
| `tests/mcp.py` | 二进制 inspect/CLI/HTTP、真实 HTTP fixture、独立 Bearer、鉴权前置、descriptor pin、工具 loop 与 SQLite 历史 |
| `tests/e2e.py` / installer | 原有进程级与安装验收通过 |
| Docker | Rust 1.88 fixed-digest locked release image 构建、非 root/只读 rootfs/命名卷恢复通过 |

Rust 镜像摘要取自 [Docker 官方 repo-info 历史](https://github.com/docker-library/repo-info/blob/18449209c1668dd35029f25f3525a63e11cd6508/repos/rust/remote/1.88.0-bookworm.md)，实际构建按内容摘要验证。HTTP MCP 使用的是网络协议 fixture，不能代替具体外部服务器、真实模型供应商或 StateKnot durable 资格认证；边界见 [MCP](mcp.md)。本批新增独立 PR 的 Linux/macOS CI 与容器 CI 会核对最终提交。

## Brokerrouter 原生工具往返批次

2026-10-02，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：579 passed，0 failed；Docker 专项仍单独标为 ignored。
- `tests/native_tools.py`：实际服务 + 网关 HTTP fixture，验证多工具 ID 关联、请求白名单、整批拒绝零副作用、坏参数/截断/重复 ID、正文不执行、HTTP 错误脱敏；执行后 502/非法批次/重复 ID 与迭代预算停止均保留记录，SQLite 会话保留中断说明。
- `tests/mcp.py`：已切换为原生工具协议；真实 StateKnot HTTP MCP 客户端的 inspect、CLI、HTTP 与 SQLite 闭环通过。
- `tests/e2e.py` 和 `tests/installer.py`：进程恢复、鉴权、并发、安装与失败保留旧版本通过。
- MCP 定向 11 项通过，新增跨服务器生成工具名碰撞在网络请求前拒绝。
- fmt、diff whitespace 与 clippy correctness/suspicious 门槛通过；保留 style/pedantic warnings，不声称全仓库 warning-free。

跨平台、浏览器与镜像验收由本批 PR 的 CI 针对提交执行。协议 fixture 不代替真实供应商、账务或 durable 认证。后续模型失败的部分完成处理只保护存活进程已知的结果，不提供崩溃后的工具恢复或操作重放。

## 持久化调度批次

2026-10-02，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：608 passed、0 failed；Docker 专项仍单独 ignored。
- 9 项时间域测试：严格未来 UTC、上海时区、DOM/DOW OR、纽约 DST gap/fold、Samoa 整日跳变、八年闰年间隔、非法表达式与溢出。
- 16 项持久任务测试：v1→v2 迁移、并发领取、4 并发上限、原子会话/运行完成、事务失败回滚、未知状态恢复、配额/保留和显式 purge。
- 2 项真实取消竞争测试：阻塞数据库提交后取消等待任务，会话锁继续保留至提交结束；恢复先标中断时，陈旧提交不能覆盖历史。
- `tests/scheduler.py`：真实二进制/HTTP/SQLite，配置及鉴权、CRUD/分页/软删除检索、任务权限、结果持久化、超时、SIGKILL 与 SIGTERM、停机跳过、不重放；SQLite 事务故障使调度停止领取并返回 503，修复重启后保持原任务暂停。
- `tests/e2e.py`、native tools、MCP 与 installer 均通过。fmt、diff whitespace、clippy correctness/suspicious 门槛通过；仍有 style/pedantic warnings。

数据库 schema v2 的回滚需要升级前备份。调度提供持久领取及中断审计，不提供外部效果 exactly-once、自动恢复工具轮次或渠道通知；具体合同见 [定时任务指南](scheduler.md)。跨平台、浏览器、真实 Docker 与镜像启动由本批 PR 的 CI 验证最终提交。

## 持久渠道与统一出站批次

2026-10-02，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：605 passed、0 failed；Docker 专项仍单独 ignored。旧同步 webhook/发送实现及其测试已移除，改由持久渠道测试覆盖，因此总数不代表在上一批测试数上简单累加。
- 18 项渠道存储测试：schema v1/v2→v3 迁移、事件去重与冲突、并发领取、会话/发件箱原子完成、CAS、严格顺序、冷却与恢复、审计清理/墓碑、会话 TTL、容量与并发预留。
- 11 项真实出站 HTTP 测试：三个平台的 payload/回执、Unicode、禁用 mentions、严格目标/凭证验证、响应体限制、超时/取消、重定向拒绝及有限 429。
- 8 项运行生命周期测试及 4 项鉴权/加密测试：错误密钥在模型调用前拒绝、队列过期、会话锁等待计入预算、禁用渠道仍执行恢复、持有数据库锁后的取消/领取竞争。cron 同类领取路径也增加锁内运行状态检查。
- `tests/channels.py`：最终二进制配合本地网关/平台 HTTP fixture，验证签名和授权、快速 ACK、重投去重、原生工具、分片顺序/回执、持久 429、断线未知结果与人工核对、SIGKILL/SIGTERM 恢复、凭证加密、事务故障原子回滚和停止准入。Discord Snowflake 有效期、最多 6 片及数据库写锁等待后的 1ms 重试回归通过。
- 最终二进制 `tests/scheduler.py` 通过；本批 e2e、native tools、MCP、installer 回归均通过。fmt、diff whitespace 与 clippy correctness/suspicious 门槛通过；仍有 style/pedantic warnings。

所有平台请求均指向本地 fixture，没有使用真实渠道/模型凭证，也没有发出真实平台消息。跨平台、浏览器、Docker 沙箱和镜像启动由本批 draft PR 的 CI 核对最终提交。升级需迁移旧渠道配置并备份 schema v3 与 Discord 加密密钥；恢复提供持久审计和保守停止，不承诺跨系统 exactly-once 或 StateKnot durable 工具恢复。正式安装与供应商认证仍待完成，见[渠道指南](channels.md)。

## 定时任务渠道通知批次

2026-10-02，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：625 passed、0 failed；Docker 专项继续作为独立 ignored acceptance。fmt、Clippy correctness/suspicious 和锁定构建通过；仍有 style/pedantic warnings。
- SQLite v4 验证 v3 数据、回执、加密凭证、冷却和序列高水位保留，双来源外键与互斥约束有效；任务运行、会话和全部分片原子提交，事务故障后整体回滚，陈旧完成不能重复入队。
- cron 与入站共享执行前容量预留；未解决投递阻止新一轮执行及恢复，失败和强杀后的未知结果暂停任务。连续 125 次通知保持运行并只保留最近 100 条；未知/未核查结果受到保护，清理故障整体回滚，清理已解决记录后可恢复容量。
- 7 组运行期 HTTP 测试验证 Telegram/Slack 正向链路、精确目的地/线程/安装与工具授权、撤权后零发送、健康状态准入、禁用调度后的恢复，以及停止/故障发生在模型调用之后时成功结果仍能持久排队。
- `tests/scheduled_delivery.py` 使用真实二进制、本地网关和平台 fixture，验证原生工具结果、线程、鉴权、限流防重叠、未知结果暂停与人工恢复、重启不重放、源记录归属、撤销权限、禁用调度时审计 API 可用，以及 SQLite 写入故障原子回滚并停止准入。
- 原有 e2e、MCP、native tools、scheduler、三渠道和 installer 进程回归通过。无通知任务继续省略 delivery 字段，保留旧响应形状。

本批没有调用真实平台或付费模型；实际安装、供应商及 StateKnot durable 认证仍未完成。升级与回滚须按 schema v4 备份要求执行；最近 100 条运行以外的长期审计需要另行归档。跨平台、Chromium、真实 Docker 沙箱及镜像启动由本批 draft PR 的 CI 验证最终提交。

## 飞书渠道批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：650 passed、0 failed；真实 Docker 专项仍单独 ignored。最终冻结后 host 270 项再次通过。fmt、Clippy correctness/suspicious、锁定二进制构建通过；工作区仍有 style/pedantic warnings。
- 10 项入站协议测试覆盖固定官方 SDK 的 AES 已知密文、严格 PKCS#7、原始 body 签名、时间窗和重复头、app/tenant/token 身份、message_id、群根路由及各级资源上限。
- 13 项飞书出站测试覆盖 token 单次并发刷新、内存过期、取消及失败退避、旧请求不会清新 token、消息不隐式重发、精确根消息回执、限流响应与 UUID、消息体边界、超时和重定向；原有 11 项出站测试继续通过。
- SQLite schema v5 验证从 v3/v4 迁移时保留回执、密文、冷却、两种来源和空表序列高水位；定时投递外键/单一来源/禁止凭据约束保持有效。
- 最终 `tests/feishu.py` 全部通过：真实二进制与独立 OpenSSL/Python 平台 fixture 覆盖 challenge/ACK 时限、签名/加密、身份拒绝、跨 event_id 消息去重、原生工具、单聊与群根、token 复用和失效退避后新消息刷新、三种有效限流和未知码、持久 UUID/冷却、processing/submitting 强杀及人工核对、精确定时目的地、环境密钥及错误配置启动拒绝。
- 既有真实二进制 e2e、MCP、native tools、scheduler、三渠道、scheduled delivery 和 installer 回归通过。

未使用真实平台或付费模型凭据。跨平台、Chromium、真实 Docker 沙箱与镜像验收由 draft PR CI 验证最终提交；正式飞书安装、真实供应商与 StateKnot durable 仍需独立验收。升级需先备份完整 state；schema v5 不支持旧二进制直接降级，详见[部署](deployment.md)和[飞书配置](feishu.md)。


## 企业微信自建应用批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：677 passed、0 failed；真实 Docker 专项仍独立 ignored。fmt、Clippy correctness/suspicious 和锁定构建通过；仍有 style/pedantic warnings。
- 加密回调使用独立官方固定向量及 OpenSSL fixture，覆盖 32 字节填充、CorpID/AgentID、MsgId、查询签名与时间窗、成员大小写规范、XML/资源边界。
- 10 项专用出站测试验证内存 token 单次刷新、期限与退避、固定单成员请求、渲染后的 2048 字节限制、完整回执、矛盾成功证据和未知结果不重发。
- SQLite schema v6 保留 v3/v4/v5 的两类来源、回执、密文、冷却、空表序列高水位与外键约束。独立发送额度账本通过延迟完成、两天后仍在途、未知重启、陈旧 CAS、结算失败原子回滚、人工核查、定时取消、审计删除与全局 10,000 条未结算容量边界测试。
- 最终 `tests/wecom.py` 通过：真实二进制 + 一次性加密密钥 + 本地网关/平台，验证 GET 1 秒内原样 challenge、POST 5 秒内持久空 ACK、授权/去重/冲突、原生工具、相同独立消息、文本无截断、发送间隔、未知响应及 processing/submitting 强杀、环境密钥与精确定时目的地。重启后 unknown 会拦住同安装其它成员，人工核查后才继续。

- 原有 e2e、MCP、native tools、scheduler、Telegram/Slack/Discord channels、scheduled delivery、飞书和 installer 的最终二进制回归全部通过。跨平台、Chromium、真实 Docker 沙箱和镜像启动由本批 draft PR CI 验证最终提交。

未使用真实企业微信或付费供应商凭据，没有真实平台消息。企业微信仅接专用自建应用的成员私聊与定时文本通知；平台接受不等于终端展示或已读。正式安装、可信出口 IP、应用可见范围与真实接收额度仍需独立认证。schema v6 升级及旧备份恢复须遵守[部署边界](deployment.md)；StateKnot durable 与真实供应商认证继续保持未完成。


## 钉钉内部应用机器人批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：695 passed、0 failed；真实 Docker 专项仍独立 ignored。fmt、Clippy correctness/suspicious 和锁定构建通过；仍有 style/pedantic warnings。
- 7 项入站测试覆盖独立 HMAC 向量、重复签名头、正负一小时时间窗、robotCode/企业身份、精确大小写成员、重复字段及 JSON 深度边界。签名只覆盖时间和 Secret，不覆盖请求体，可信 HTTPS 入口属于部署必需条件。
- 9 项专用出站测试覆盖固定 token/发送端点、独立 Client ID 与 robotCode、内存单次刷新/期限/退避/取消、单成员文本、三个失败列表、矛盾成功证据、未知结果不重放。
- SQLite schema v7 验证从 v3/v4/v5/v6 升级，保留历史回执、密文、冷却、空表序列高水位、双来源外键与企业微信未结算/已结算额度账本。新增钉钉测试验证领取与完成后的冷却、陈旧 CAS、重启冷却、同成员 FIFO 和其它成员继续处理。
- 最终 `tests/dingtalk.py` 三组整机验收全部通过：真实二进制、本地模型/平台和独立 Python 签名，覆盖持久空 ACK、精确身份与成员、去重冲突、同文独立事件、原生工具、Unicode 分片、延迟响应后的完整 4 秒冷却、未知/失败回执、processing/submitting 强杀、人工核查、精确定时授权、环境密钥和错误配置拒绝。
- 128 KiB 请求体在解析前返回 413，超过 32 KiB 文本被拒绝；恶意 sessionWebhook 不被访问，回调 URL、签名、Client Secret、token 和平台原始错误不进入数据库、日志或管理响应。

未使用真实钉钉或付费模型凭据，没有发出真实平台消息。该批仅覆盖 HTTP 模式内部应用机器人的成员私聊和定时文本通知；processQueryKey 表示平台 API 接受，不表示终端展示或已读。真实发布、权限、HTTPS 链路、回调重投、长文本和额度仍需安装验收。schema v7 升级前须备份，不能直接用旧二进制降级；见[钉钉指南](dingtalk.md)与[部署](deployment.md)。StateKnot durable、stdio MCP 和真实供应商认证仍未完成。

既有最终二进制 e2e、MCP、native tools、scheduler、Telegram/Slack/Discord channels、scheduled delivery、飞书、企业微信及 installer 九组回归全部通过。TOML/JSON 示例等价解析复核通过。Linux/macOS、Chromium、真实 Docker 沙箱与镜像启动由本批 draft PR CI 验证最终提交。


## 按任务用途选择模型批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：706 passed、0 failed；真实 Docker 专项仍独立 ignored。fmt、Clippy correctness/suspicious 和锁定构建通过；保留已有 style/pedantic warnings。
- 9 项核心策略测试验证五种可信用途、精确模型名及 200 UTF-8 字节边界、有限温度、provider/用途/摘要三层输出上限、Brokerrouter 限定、TOML/JSON 配置覆盖和错误拒绝、旧响应兼容。无工具摘要用途不能通过完整聊天 API 执行工具。
- 宿主测试通过真实渠道 worker 验证 channel 模型与参数、原生工具白名单保持不变；SSE done 保留可选模型信息，OpenAPI 不开放请求侧模型覆盖。
- 最终 `tests/model_routing.py` 两组整机验收通过：实际二进制 + 本地网关验证 CLI/HTTP/通用 webhook/cron/HEARTBEAT/摘要的可信选路，默认继承、参数边界、原生工具两轮固定模型、工具已执行后第二轮失败保留记录与路由、恶意输入无法覆盖配置、未认证请求零模型调用、未知提交/429/502/断连/预算/审批/能力错误均不切模型或重发。
- 定时任务完成响应的路由参数重启后按同一运行 ID 保留；摘要无工具且始终不超过 512 和全局/用途上限。未配置路由的响应继续省略 routing 字段，旧数据可读；本批不新增 SQLite schema。

这项验收只证明应用提交前选择逻辑模型并保留网关治理边界，不等于真实供应商认证或已执行网关内部端点降级。所有请求使用本地 fixture 和一次性凭据。渠道事件与会话没有新增持久模型审计；模型操作身份、未知提交恢复及 StateKnot durable 仍未完成。合同、预算含义及上线前提见[模型路由](model-routing.md)。跨平台、Chromium、真实 Docker 沙箱与镜像由本批 draft PR CI 验证最终提交。

原有 e2e、MCP、native tools、scheduler、channels、scheduled delivery、飞书、企业微信、钉钉和 installer 十组回归全部通过。收尾修正 OpenAPI 的人工核查状态拼写以匹配既有 `requireshumaninput` 回执，契约测试和 TOML/JSON 示例等价复核通过；最终重建后二次执行模型路由 fixture 仍通过。

## 独立用户网关批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：726 passed、0 failed；1 项真实 Docker 专项仍独立 ignored。Clippy correctness/suspicious、格式和锁定构建通过，保留 style/pedantic warnings。网关新增 19 项测试，另增加空持久卷启动且重启保留现有文件的回归；覆盖严格配置、密钥绑定/配额、事务回滚、并发准入、旧回执、重启核对、取消中的阻塞鉴权容量。
- `tests/user_gateway.py` 使用真实二进制与两个本机协议后端，验证同名用户请求隔离、单实例恢复锁、在线轮换/撤销/禁用、慢正文期间再次检查撤销、禁止客户端身份头和编码路径、体积/时限、重定向不跟随、取消不释放在途许可、SIGKILL 后只核对不重放、未完成或畸形 200 回执不解锁。
- Chromium 验收通过：除原有工作台流程，增加失败身份切换和非 JSON 401 后清空 Token、会话、消息与输入草稿；Key 不进入浏览器持久存储。
- 全部十二项本机进程/协议脚本通过：e2e、MCP、native_tools、model_routing、user_gateway、scheduler、channels、scheduled_delivery、feishu、wecom、dingtalk、installer。
- 网关专用 Compose、Secret 入口、JSON/TOML、Python/Shell 语法已检查。`tests/gateway_container.py` 的真实 Linux 容器/限额/防火墙验证由本批 draft PR CI 执行；macOS 静态检查不冒充 loop device/宿主网络验收。

仅交付独立后端的 HTTP/工作台聊天、会话与本地管理 Key。多用户渠道和后台任务绑定、逐 token 流式、模型操作身份恢复、真实供应商、TLS/egress 部署、StateKnot durable 均未据此认证。生产拓扑、管理员核对和备份回退边界见[网关指南](gateway.md)。


## 记忆文件 I/O 批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：747 passed、0 failed（library 294、core 117、host 336）；1 项真实 Docker 专项保持独立 ignored，由 CI 执行。
- fmt、Clippy correctness/suspicious 和锁定构建通过，保留已有 style/pedantic warnings。
- 真实进程 e2e、native_tools 与新增 memory_io 均通过；memory_io 完成四组验收，已接入 Linux/macOS CI。跨平台/真实容器以本批 draft PR 最终 head CI 为准。

已通过的真实进程行为：CLI init 默认保留与显式 force 安全覆盖（含示例技能）；MEMORY/SOUL/USER 配置路径读取及 CLI 展示；禁用 memory_write 同时阻止原生 memory_append；小参数追加验证最终文件的 32 KiB 上限（原生工具参数另受 16 KiB 预检约束）、UTF-8 读取与 HEARTBEAT 超限不调用模型；检索 512 KiB/16 路径/1024 字节查询和摘录边界；symlink 父目录与叶子、hardlink、FIFO 拒绝；旧固定临时文件不触碰外部目标；工作区目录锁竞争时立即失败，显式后续追加保留已有内容。

只使用本机模型协议 fixture、一次性凭证和临时文件；没有真实供应商调用。该批不新增 embeddings、向量索引或语义检索，也不声称外部编辑器受锁约束、取消后写操作必然未发生，或已认证断电硬件持久性。操作和备份边界见[记忆文件指南](memory-files.md)。


## 语义记忆批次

2026-10-03，macOS arm64，Rust 1.88.0，锁定依赖。

- 全量 Rust 回归：773 passed、0 failed（library 317、core 120、host 336）；1 项真实 Docker 专项保持独立 ignored。fmt、Clippy correctness/suspicious 和锁定构建通过，保留已有 style/pedantic warnings。
- 最终 `tests/semantic_memory.py` 的 9 组真实二进制验收全部通过：默认关键词及关闭零缓存/embedding 请求、刷新/查询/重启、原生 semantic 工具、paths 只能收窄、同大小/mtime 编辑与删除的零请求拒绝、响应期间源变化、错误回执/断连/已提交后 SIGKILL 的持久 hold、同库换 Key/版本与重建不可绕过、GET 恢复与显式管理员解除、继承源路径控制字符/边界空白启动拒绝、私有路径/独占所有权/工作区与数据库身份隔离。禁用 semantic 时，工具 schema 在分派前拒绝该模式。
- 存储 12 项测试全部通过：binary f32 的最大 generation 为 7,383,192 字节，单个回执为 98,320 字节，65 个回执加新旧 generation 为 21,157,184 字节，在 32 MiB 上限内；六次原子替换通过，后四次页数稳定。schema 2 拒绝旧 schema，不清除账本。传输测试另覆盖向量、响应身份与资源边界。
- 最终二进制既有 e2e、native_tools、memory_io、model_routing 回归全部通过。PR #70 的 head `dae57af27c77f553e6344f5391611b35df454bfe` 已通过 [CI 37082943062](https://github.com/jiawenyao401/JiaClaw/actions/runs/37082943062)，覆盖 Linux/macOS、Chromium 及真实容器。

CLI `memory semantic status/refresh/search/recover/review-clear/rebuild` 维护同库前须停止 `serve`，没有公开 HTTP 管理路由。测试只使用本机 embeddings/chat fixture、一次性凭据和合成向量，不发送付费模型请求。不能将本批协议验收解释为真实模型检索质量、真实供应商计费或 StateKnot durable Agent 认证。配置、资源与人工核对边界见[语义记忆指南](semantic-memory.md)。


## 独立用户定时任务批次

2026-10-03，macOS arm64，最终二进制本地验收。

- `tests/tenant_cron.py` 4 组通过：真实双后端/网关/本机模型验证受保护能力发现、任务 CRUD/204、输入和分页边界、租户结果隔离、普通 Key 轮换保留用户任务、禁用零派发、共享执行容量及运行期间状态读取、网关停机零自主执行。领取且模型提交后强杀后端与网关，重启保留同一 interrupted run、未知 hold 及 request/run ID 关联；更换 Key 不能解除 hold，另一租户继续运行；明确核对并恢复后只建立新的运行。
- 最终二进制内嵌工作台的 Chromium 验收通过：原有会话流程、任务创建/暂停/恢复/软删、204、聊天 200/任务 201 的待核对标志保留草稿并阻止成功提示、恶意 HTML 纯文本展示、结果截断、身份切换清空任务/结果/草稿和迟到响应隔离。浏览器任务接口使用 route fixture；真实接口由双后端脚本独立验收。
- 全量 Rust：797 passed、0 failed（library 317、core 121、host 359），1 项真实 Docker 专项保持独立 ignored。fmt、diff-check、Clippy correctness/suspicious 和锁定构建通过；既有 e2e、user_gateway、scheduler、scheduled_delivery、model_routing 回归通过。PR #71 最终 head `949aebb` 已通过 [CI 37089235842](https://github.com/jiawenyao401/JiaClaw/actions/runs/37089235842)，包含 Linux/macOS、Chromium、真实 Docker 沙箱及镜像。首轮 Linux 故障注入触发了非目标租户的合法未知 hold；fixture 已隔离非目标任务并保留全部目标 SIGKILL 断言，修复后完整 CI 通过。

本批使用一次性本机凭据、临时数据库与合成模型响应，没有付费模型调用。保留跨服务未知结果与人工核对边界，不能据此声称真实供应商计费或完整多用户后台能力已认证；见[独立用户定时任务](tenant-cron.md)。


## 模型调用收据批次

2026-10-03，macOS arm64，本批最终二进制本地验收完成；PR #72 最终 head `a841b026` 已通过 [CI 37093371689](https://github.com/jiawenyao401/JiaClaw/actions/runs/37093371689)，覆盖 Linux/macOS、Chromium 和真实容器。

- 最终 `tests/model_calls.py` 7 组全部通过：默认关闭零账本；POST UUID/精确字节摘要、原生工具分轮及摘要收据、进程重启读取；摘要响应错误后持久 hold 且不继续聊天 POST；已知远端 UUID 的 GET 状态/无 UUID 头结果恢复，不执行恢复工具或修改原会话；更换 Key/模型不能绕过；缺 UUID/断连的拒重试与显式核对；实际提交后 SIGKILL 重启保留原操作和摘要；scheduler 父运行超时后收据 worker 完成，真实已批准的本机 MCP probe 零 tools/call，运行保持 interrupted。
- 私有目录验收覆盖已有 0755 的外层 state：默认新 `model-calls` 子目录为 0700、数据库为 0600，外层目录保持原权限。发送正文标记不出现在账本文件；CLI status 不展示收据正文；存活服务持有独占所有权，维护 CLI 被拒绝。
- Rust 验证：修复后全量 815 项通过（library 334、core 122、host 359），无失败；真实 Docker 专项仍由独立 CI 执行。新增测试验证取消等待并丢弃调用方后，worker 仍持有数据库独占锁直至收据完成，再打开可见 completed。Clippy correctness/suspicious 与锁定构建通过。既有 e2e、native_tools、model_routing、semantic_memory、scheduler、tenant_cron、scheduled_delivery 七组整机回归通过。

fixture 的 SQLite 检查以 `mode=rw` 打开已有库并立即启用 `query_only`，允许 SQLite 在 WAL 已 checkpoint 后创建自身 sidecar；不写账本记录，也不对运行中的库使用忽略 WAL 的 immutable 模式。 查询连接显式关闭，不依赖 Python 垃圾回收。

PR #72 首轮 CI（head `93c653d`、run `37092094733`）在 Linux 的取消收据组暴露了存储层问题，macOS 同组通过：SQLite 建立连接后才关闭预检文件句柄，会在 POSIX 下释放同进程的主库锁；独立只读查询连接关闭时可能误删仍在使用的 WAL。离线 Linux 两进程最小对照验证，旧顺序下写入方看到 completed，而新读者仍看到 submitting；把预检句柄移到 SQLite 建连之前关闭后，两方都读到 completed。模型调用与语义索引两个同型 store 已修复该句柄顺序，fixture 增加有界脱敏状态/门控时间/服务器错误类别和宿主事件诊断，不增加超时或放松成功断言。修复后 PR #72 最终 head `a841b026` 的完整跨平台 CI 已通过；首轮失败及修复证据保留用于追踪。语义记忆新增真实 serve 与独立只读 observer 回归，验证 observer 关闭不移除或替换活跃 WAL/SHM、后续回执跨进程可见、重启查询复用回执不新增 POST；修复后二进制的语义记忆全部 10 组通过。

该文件句柄行为与 [SQLite 官方文件锁说明（2.2）](https://www.sqlite.org/howtocorrupt.html#_posix_advisory_locks_canceled_by_a_separate_thread_doing_close_) 一致。官方说明 3.51.0 增加了部分 WAL 多进程防御；本机观察者为 3.51.0、Linux 最小复现为 3.40.1，因此不将平台表现差异直接归为操作系统本身，也不依赖新版防御代替修复。

没有使用付费模型或真实供应商凭据。该账本只保存模型调用事实，不是供应商费用主账、工具执行日志或完整 turn 检查点；真实 Brokerrouter、供应商计费、StateKnot durable、SSE 与媒体身份链认证分别保留。配置与操作见[模型调用收据](model-calls.md)。

## Discord Bot 定时文字批次

2026-10-03，macOS arm64，最终二进制的 `tests/discord_scheduled.py` 六组真实进程验收全部通过。测试只使用本机模型/Discord HTTP fixture、临时 SQLite 和一次性凭据，覆盖配置与精确授权、应用/guild/type 前置核验零 POST、超过六片的 Unicode 结果与完整 UUID nonce、可信 429/成功耗尽预算的持久安装冷却、401 跨重启与环境新 Token、错回执安装阻断、实际 POST 后 SIGKILL 及人工核查后不重放。既有 `tests/channels.py` 独立回归交互凭据路径。

数据库迁移至 schema v9；全量 Rust 832 项通过（library 334、core 122、host 376），Clippy correctness/suspicious、fmt/diff-check 与锁定构建通过。最终二进制既有 channels、scheduled_delivery 与 model_calls（7 组）回归全部通过；Docker 专项保留独立 CI 验收。fixture 同时检查 API、日志及停服后数据库/保留 sidecar 不含一次性凭据与上游私有错误标记。Token 轮换验收保留原 permanent_failed，并通过人工取消同目标旧失败计划后才发送未来结果，未放宽目标 FIFO。PR #73 最终 head `b99e96690b6ec2ec0265fe68d2895a135cb9e8d5` 已通过 [CI 37095933677](https://github.com/jiawenyao401/JiaClaw/actions/runs/37095933677)：Ubuntu、macOS、container 均成功，覆盖 Chromium、真实 Docker 与镜像。没有真实 Discord 或付费供应商调用，正式 Bot 安装、guild/频道权限和限流认证仍需独立完成。

## Web 发件箱审计批次

2026-10-03，macOS arm64，最终二进制的本机验收完成。全量 Rust 834 项通过（library 334、core 122、host 378），fmt、Clippy correctness/suspicious 与锁定构建通过；真实 Docker 专项仍由独立 CI 执行。

- `tests/outbox_browser.cjs` 真实 Chromium 全流程通过、退出码 0：真实 webhook/调度与本地模型/Telegram fixture 生成入站及定时多片 outbox，不直接写入产品数据库伪造审计状态。实际平台 POST 后 SIGKILL，重启并禁用 channels/scheduler 后，原投递仍可审计和核对。
- 列表固定 5 条；offset=5 的断网失败保留原页，下一次仍请求 offset=5。UUID 直接定位、记录不存在的 404、submitting/delivered 状态禁写通过。空白、换行或超过 4096 UTF-8 字节的证据在浏览器拒绝，不弹确认、不 POST，并保留草稿。
- 409、提交前断网均触发 GET 核对且不自动重发 POST；GET 失败时禁止修改，直至成功刷新。真实 resolve 在服务端完成并返回 204 后丢失浏览器响应，页面通过 GET 发现 delivered，保留证据而不再次 POST。实际 204 的确认及整来源取消后，原有模型 12 次、平台 6 次请求均未增加。
- 恶意 HTML 纯文本显示，移动端列表行高至少 30 px 且无横向溢出；403/404 隐藏能力，身份切换、401 和迟到响应清空隔离，Token 仅保留在页面内存。既有 `tests/browser.cjs` 的聊天、会话与受限任务回归通过。单条详情 API 鉴权/持久化/UUID/不存在边界和独立用户网关拒绝访问由 Rust 回归验证。

本批没有真实渠道或付费模型请求，不更改平台发送合同，不增加未知消息自动重放，不将人工核对解释为工具或任务恢复。PR #74 最终 head `e22965dd9dc2488c9433d0a93f7b9e1bf59304a5` 已通过 [CI 37098122253](https://github.com/jiawenyao401/JiaClaw/actions/runs/37098122253)：Ubuntu、macOS 与 container 均成功，包含 Chromium 和真实容器。操作与权限说明见 [Web 发件箱指南](web-outbox.md)。

## 单实例定时任务工作台批次

2026-10-03，全量 Rust 845 项通过（library 334、core 122、host 389），1 项真实容器测试在本机保持 ignored，由独立 CI 执行。fmt、Clippy correctness/suspicious、Node 语法检查及锁定构建通过。

- 含最后筛选修复的最终二进制通过三套真实 Chromium：`tests/scheduler_browser.cjs`、`tests/browser.cjs`、`tests/outbox_browser.cjs`，均退出码 0。新脚本通过本机模型、实际 host 和 SQLite 验证创建/运行/暂停恢复/删除/purge/重启、原生 interval 工具循环、含时区的未来 cron，以及已有 600 秒和渠道目的地等高级授权字段的展示。
- 创建的实际 201 响应丢失后，GET 核对保留已暂停状态；创建后软删除且 GET 失败，再以同 ID/快照 PUT 返回已删除状态，不复活任务。真实 purge 后 PUT409，再 GET404，页面继续显示永久退役原因、保留 ID/草稿并禁用重试；查询不自动 PUT 或更换 ID，只有显式放弃才解除本页跟踪。
- 任务每页 5 项、运行每页 1 项；翻页失败不提交新 offset。更改“显示已删除”筛选的请求失败时，复选框、旧列表和分页保持原值，下一页继续使用原筛选。近 990 KiB 的响应可读，超过 2 MiB 的页面拒绝且不换页；gateway false、200/null、204 和旧 status 无协议均不触发 standalone 回退。
- 实际模型 503 产生的 failed run 同时显示保存响应和错误；浏览器 failed/stopping 健康状态、响应体积等边界使用协议注入，不能描述为真实数据库故障。XSS 纯文本、身份切换/401/迟到响应、移动布局及旧网关任务/发件箱流程通过。
- 同批 schema 10 后端的 `tests/scheduled_delivery.py` 和 `tests/discord_scheduled.py`（六组）进程回归均退出码 0；这两项使用最后 UI 筛选修复前的二进制，之后后端未改动。Rust 回归覆盖规范 UUIDv4、typed 指纹、异体与旧任务/遗留会话拒绝认领、同 ID 并发、创建事务回滚、重启及 purge 墓碑、永久容量和模式/权限边界。

本批只使用一次性本地凭据与协议 fixture，没有付费模型或真实平台请求。跨平台与真实容器由本批 draft PR 最终 head CI 验证；配置、备份和恢复边界见[单实例工作台](standalone-scheduler.md)。

PR #75 首轮 head `1a14cad55b9602ee6b7a71055671b3ef03c9ab3f` 的 [CI 37100634034](https://github.com/jiawenyao401/JiaClaw/actions/runs/37100634034) 在 Linux 的 `tests/tenant_cron.py:307` 遇到 `GET /api/sessions` 返回 429（`user request already in progress`）。运行状态属于控制请求，可先读到后端 completed；此时网关尚未完成 `finish_write` 收据提交，仍持有执行许可，会话查询使用执行容量，因此该 429 是正常的在途保护，不能以 completed 状态推定执行许可已经释放。

修正仅限 fixture 的两处会话列表断言：遇到精确匹配的 busy 429 时，对只读 GET 作有界轮询，其他错误仍直接失败；另通过暂停模型响应确定性验证在途期间仍返回该 429。不自动重放写入，不提前释放生产执行许可，也不放宽后端准入。生产代码无需修改；修正后的真实双租户进程验收四组通过、退出码 0，原 SIGKILL/持久 hold/Key 轮换/其他用户继续及零模型重发断言保留。修正后 PR #75 最终 head `0c9900a75a8f5a3a1c980c8ba98b18c3430b12d2` 的 [CI 37101387934](https://github.com/jiawenyao401/JiaClaw/actions/runs/37101387934) 已通过 Ubuntu、macOS 和 container 三项检查，覆盖 Chromium 与真实容器；首轮失败记录保留用于追踪。

## 工作区文件权限与 I/O 批次

最终本机全量 Rust 854 项通过（library 343、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI；fmt、Clippy correctness/suspicious、锁定构建与 diff-check 通过，保留既有 style/pedantic warnings。

- 新 `tests/workspace_files.py` 已接入 Linux/macOS CI，最终真实二进制四组全部通过：主名称/别名 schema 和 JSON、一致的 UTF-8 落盘字节、真实创建/追加/读取/替换/目录/删除；通过小参数追加达到精确 262144 字节，行范围读取和追加/替换超限拒绝保留原文件；内外部符号链接、损坏链接、硬链接、FIFO、路径穿越及缺失父目录外逸均拒绝，外部哨兵文件不变；四个配置开关同时从 HTTP/native 目录移除八个名称，禁用名称与伪造别名在整批执行前被拒绝。
- 文件增长的进程测试是在读取前向文件追加后再验证拒绝；读取过程最多消耗 limit+1 字节由 Rust 的受控 reader 测试验证。Rust 还验证最终 pretty JSON ≤64 KiB（含转义名称）、扫描 2000 条、递归 32 层、过期遍历预算、共享目录锁/并发追加及取消后的阻塞容量，不把这些证据归到 Python fixture。
- 最终二进制的 `tests/native_tools.py`、`tests/memory_io.py`（四组）和 `tests/e2e.py` 回归全部通过、退出码 0；记忆回归继续覆盖旧临时文件守护、锁竞争、配置路径与超大 HEARTBEAT 零模型调用。

测试只使用本机模型协议、一次性凭据和临时文件，没有真实平台或付费模型调用。跨平台与真实容器以本批 draft PR 最终 head CI 为准，不以单机协议验收代替断电硬件持久性或完整个人 Agent 生产认证。

PR #76 最终 head `277a89a5ade1e4ab84d7c17d696c004b7fd1e7ea` 已通过 [CI 37103576477](https://github.com/jiawenyao401/JiaClaw/actions/runs/37103576477)：Ubuntu、macOS、container 均成功，覆盖 Chromium 与真实容器。

该批目录句柄迁移限于 `read_file`、`write_file`、`delete_file`、`str_replace`、`list_dir` 及四个兼容名称。`grep`、`glob`、`mkdir`、`move` 未迁移，`copy` 保留独立合同；没有新增 `stat` / `tree` 或后台文件工具授权。操作与兼容边界见[工作区文件指南](workspace-files.md)。

## grep / glob 有界搜索批次

最终本机全量 Rust 864 项通过（library 353、core 122、host 389），1 项真实 Docker 专项本地 ignored 交 CI；fmt、Clippy correctness/suspicious、锁定构建与 diff-check 通过，保留既有 style/pedantic warnings。

- 最终二进制新增 `tests/file_search.py` 六组通过、退出码 0：字面量/Unicode 小写匹配、行号和路径、glob 排序/过滤及 `.git` 排除；精确 256 KiB、目录中超大/二进制跳过与显式拒绝、glob 不读取正文；内部/外部/父目录/损坏链接、硬链接与 FIFO 拒绝，外部哨兵不泄露；禁用搜索工具从 HTTP/native 目录移除，伪造批次不执行前面的获准工具，非法请求白名单零模型请求。
- 真实进程通过 2005 个空目录、34 层目录树和 65 个各 256 KiB 文件触发对应截断；glob 过滤后不读取正文，80 个小文件完整搜索。400 个带大量引号的路径及 Unicode snippet 保持最终 pretty JSON ≤64 KiB；20 个 `**` 对 22 层路径的失败匹配结束。上述结果验证实际链路，不用完成耗时代替精确资源计数。
- Rust 确定性测试另验证条目计数覆盖目录和被跳过项、深度/路径/过期 deadline、实际字节预算和不搜索半文件、metadata 检查后真实文件增长最多读取 limit+1 字节并计入总预算、父目录改名后仍使用所持目录能力、叶子替换拒绝。glob 最终全路径排序后才应用请求上限，grep 保留 DFS；动态规划有受限最大模式与独立小输入参考比对。
- 非 UTF-8 名称拒绝专项在 Linux 执行；本机 APFS 不允许构造该名称，因此未把 macOS 通过列为此专项证据。最终二进制的 `workspace_files.py` 四组、`native_tools.py`、`memory_io.py` 四组及 `e2e.py` 全部通过、退出码 0。

该批只使用本机模型协议、一次性凭据和临时文件，没有真实供应商或平台请求。PR #77 最终 head `bc7ef30467ad8c436585eeee4b1cfc99d16ef68f` 已通过 [CI 37105552392](https://github.com/jiawenyao401/JiaClaw/actions/runs/37105552392)，含 Linux/macOS、Chromium 与真实容器。

本轮仅迁移 grep/glob 的工作区访问和资源边界，不新增后台授权、跨编辑器快照或取消后回滚。`mkdir` / `move` 和可选 `stat` / `tree` 不记为完成，copy 保留原独立合同；详见[工作区文件指南](workspace-files.md)。

## mkdir / move 原子变更批次

最终本机全量 Rust 875 项通过（library 364、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI。fmt、Clippy correctness/suspicious 与锁定 host 构建通过，保留既有 style/pedantic warnings；`memory_io` 定向 24 项通过，含 10 项新增本机 mutation 测试。

- 最终生产二进制 `tests/workspace_mutations.py` 六组通过、退出码 0：mkdir 递归/parents 别名、普通目录和 `.` 幂等 inode、缺失父目录及冲突参数拒绝；move 的主名/别名四种组合、超过 1 MiB 的二进制和非空目录同卷 rename 保留 dev/inode/字节，源目录内链接与 FIFO 原样移动且不被跟随；默认不覆盖、同类型文件/空目录覆盖、非空目的地/类型不符/同源/自身子目录/缺失父目录失败保留两端；静态链接/硬链接/FIFO/越界拒绝；64 组件与 1024 字节边界、真实工作区锁争用及释放后继续；禁用配置从目录移除工具，伪造混合批次零工具效果、非法显式白名单零模型请求。
- Rust 本机测试覆盖真实内核 NO_REPLACE：在预检后创建目标，单次 rename 仍拒绝覆盖；EXDEV / ENOSYS / EACCES 受控错误注入不触发 copy/delete 回退并保留两端；已打开父目录在宿主路径替换后继续受句柄约束；叶子替换竞争不跟随链接目标；同 inode 和 macOS 大小写别名的自身子路径拒绝。追加的定向 mkdir 回归也通过：总路径合法但后续单个文件名超过文件系统上限时，先创建的 `partial` 父目录保留且为空；人工核对后显式继续创建可成功。这里验证的是后续创建失败，不是目录 fsync 故障注入。该证据不提供非协作编辑器的 inode compare-and-swap 保证。
- 进程 fixture 不挂载第二个文件系统，本机 macOS 没有真实跨卷验收。Linux Rust 新增真实 EXDEV 专项，要求 `/dev/shm` 可写且与临时目录为不同设备，CI 缺少前提直接失败，非 CI 仅允许明确报告跳过；其通过状态以本批最终 CI 为准。
- 同一最终生产二进制的 `workspace_files.py` 四组、`file_search.py` 六组、`native_tools.py` 与 `memory_io.py` 四组回归全部通过、退出码 0。本批未单独重跑通用 `e2e.py`，由最终 CI 执行，不沿用上批本机结果。

只使用本机模型协议、一次性凭据和临时文件，没有真实供应商或平台请求。PR #78 最终 head `9690692bbb208fe5bebac3eab69a73540e335c16` 已通过 [CI 37107782116](https://github.com/jiawenyao401/JiaClaw/actions/runs/37107782116)，Ubuntu、macOS 与 container 均成功，含 Chromium、真实容器和 Linux 真实 EXDEV 专项。

此批不增加后台文件工具授权、非协作编辑器快照、操作收据或取消后回滚；copy 保留独立实现，该批没有新增 stat/tree 或 durable 工作流。配置与恢复步骤见[工作区文件指南](workspace-files.md)。

## stat / tree 文件信息批次

本轮增加 WorkspaceStat/WorkspaceTree 与独立配置开关，并将 list_dir 共享 walker 的路径预算收紧到完整工作区相对路径。最终本机全量 Rust 884 项通过（library 372、core 123、host 389），1 项真实 Docker 专项本地 ignored 由 Linux CI 执行；fmt、Clippy correctness/suspicious 和锁定 host 构建通过，仍有 style/pedantic warnings。

- 最终含严格 JSON object 检查的二进制运行 `tests/filesystem_info.py`，六组全部通过、退出码 0：默认根/精确可选字段与固定毫秒 mtime、文本/二进制和 64 MiB+3 稀疏文件元数据；所选目录相对名称、每层排序 DFS、隐藏/`.git`、条数/深度及空目录边界保守截断；叶子符号链接/损坏链接/硬链接/FIFO 仅显示类型、链接父目录与非法根拒绝、不跟随外部目标，持有写锁时只读查询仍可完成。
- 同一新 fixture 验证严格参数类型/未知字段/范围、64 组件和 32 层边界；2005 个空目录及含大量转义名称的完整 pretty JSON ≤64 KiB；stat/tree 独立开关、HTTP/native 工具目录一致、显式白名单与伪造混合批次零先前写入效果，非法调用方授权零模型请求。它不以耗时观察代替精确 deadline 或计数证明。
- Rust 测试验证规范路径、epoch 前时间向下取整、父目录能力保留和叶子替换不跟随、完整工作区路径 1024 字节/64 组件在 lstat 前限制及 list_dir 同步收紧、tree DFS/深度和完整输出预算；既有共享 I/O 取消/许可与精确扫描/deadline 回归同样通过。非 UTF-8 文件名专项仅在 Linux 执行，本机 APFS 不允许构造该名称，不将其算作本机覆盖。
- 本批七套既有真实进程回归 `workspace_files.py`（四组）、`file_search.py`（六组）、`native_tools.py`、`mcp.py`、`memory_io.py`（四组）、`e2e.py`、`workspace_mutations.py`（六组）全部通过、退出码 0。

测试只使用本机模型协议、一次性凭据和临时文件，没有真实模型或平台请求。PR #79 最终 head `768cfba3041c7c863e14cbdcc17d6b13ef6470d5` 已通过 [CI 37110081396](https://github.com/jiawenyao401/JiaClaw/actions/runs/37110081396)，Ubuntu、macOS 与 container 三项均成功，包含 Chromium 与真实容器。

验收范围为本机文件元数据、目录扫描、参数/配置授权及资源边界；持久渠道和 cron/interval 继续拒绝这些工具，管理员启用的独立 HEARTBEAT 与兼容 `/hooks/inbound` 则受现有注册工具与配置开关控制。不据此承诺一致快照、持久文件操作收据、工具循环恢复或完整 OS 沙箱。

## 独立用户 Telegram 私聊批次

本批先完成私有队列与持久 claim 关联：新 TelegramStore 七项定向测试通过，既有 channel_store 35 项回归通过。覆盖不可变 owner/独占锁、符号链接/硬链接/特殊文件及错误权限拒绝、普通 session DB 拒绝收养、重复请求 ID、claim 插入失败及 SQLITE_FULL 原子回滚、16000 条容量满后审计与清理、重启 processing/submitting 恢复、purge 外键级联和 FIFO/冷却提示。操作关联仅在保留期防止重复关联，不提供公开重放协议或 purge 后永久幂等。

最终本机全量 Rust 918 项通过（library 372、core 123、host 423），1 项真实 Docker 专项本地 ignored 留给 Linux CI；fmt、Clippy correctness/suspicious 和锁定 host 构建通过，仍有既有及 style/pedantic warnings。

- 最终二进制 `tests/tenant_telegram.py` 七组全部通过、退出码 0：默认关闭/后端握手、严格私聊身份；两个真实 JiaClaw 后端及本机 Brokerrouter/Telegram 协议 fixture 完成原生工具闭环，验证 channel 模型路由、会话与私有队列不跨用户。
- 共享执行容量和用户禁用覆盖模型已准入但尚未发送的窗口；重新启用才继续已排队回复。未知第一片阻挡后续，重启不重放模型/发送；离线 inspect 关联 hold 与实际 delivery，resolve 仅记回执，核对后只继续剩余分片。
- 429 绝对冷却跨重启保留，第五次限流成为失败 hold，拒绝第六次发送；取消/purge 后保留去重。真实 SIGKILL 网关而后端仍继续工具循环并提交会话，重启保留原 request ID 和 needs_review，另一用户仍可处理；人工取消及 review-clear 后只执行新的显式事件。
- 永久撤销绑定阻止未来入站与排队发送，用户 enable 不复活绑定，不能重新分配已预留用户/Bot；运行期间离线维护被锁拒绝。日志校验不含 fixture 密钥。
- 本批既有 e2e、mcp、native_tools、user_gateway、tenant_cron、channels、scheduled_delivery 七套真实进程回归均通过、退出码 0。
- JSON/YAML 示例解析与 docker compose 合并配置检查通过：六个 Secret 仅挂到网关，两个后端不含 Telegram Secret，原有限额卷与私网不变；没有据此声称新增 overlay 的容器启动或平台实测。

PR #80 最终 head `68b3a22867e65ed32154c4fc2292066da6f842b4` 已通过 [CI 37113957145](https://github.com/jiawenyao401/JiaClaw/actions/runs/37113957145)，含 Linux/macOS、Chromium 与真实容器。部署与人工核对见[指南](tenant-telegram.md)；真实 Telegram、TLS/egress、共享网关卷压力与供应商联调不由本机协议 fixture 认证。

## 管理员签发只读 API Key 批次

2026-10-08 复核附着 PR #58–#80：均为未合并 draft，当前 head 检查全部 SUCCESS，未有 review 或 review thread。PR #80 上述最终 head 与 CI 37113957145 均未变化；container、Ubuntu 和 macOS 三项仍为 completed/success。上游固定版本及现有议题本次未变化，见 [StateKnot](stateknot-gaps.md) 和 [Brokerrouter](brokerrouter-gaps.md)。

本批基于已通过最终 CI 的 PR #80，实现 registry schema 3 的逐 Key 只读权限、管理员签发/轮换/分页列表、网关服务端拒绝修改及工作台只读状态。旧 schema 1/2 的 Key 默认保持完整权限；只读不脱敏已有内容，GET 的 TTL/访问维护和独立授权的 cron/Telegram 后台工作不受该 Key 的内容修改限制影响。

最终本机验证：

- `cargo test --workspace --locked`：928 项通过（library 372、core 123、host 433），1 项真实 Docker 测试在本机忽略，交给 Linux CI 实测；fmt、所需 Clippy correctness/suspicious 检查与锁定构建通过。
- registry/CLI/proxy 测试覆盖 schema 1/2 升级保留、失败原子回滚、未知 schema 拒绝、权限不可变、轮换继承、伪造 Principal 拒绝及无 hold/audit 副作用、生命周期与有界无秘密列表；权限拒绝优先于请求正文与后端容量。
- 最终二进制 `tests/read_only_keys.py` 五组通过：两个实际后端、私有 SQLite 与 localhost 模型；本人历史/JSON/JSONL 导出和任务读取，跨用户及 Header 伪造拒绝，八条修改路由 403、正文未提交也立即拒绝、用户内容/模型/hold/admission 未变；轮换/撤销/禁用、仅剩只读 Key 时独立授权 cron 继续、重启和停用能力保持。
- 最终二进制 `tests/read_only_browser.cjs` 真实 Chromium 通过：实际网关/后端/已保存任务结果，只读提示和只读浏览、强制 DOM 操作零修改/零模型/零 hold，完整权限切换、过期能力响应不升级新身份、权限响应格式异常和撤销清除身份、内存密钥与移动布局。
- 既有 `browser.cjs`、`outbox_browser.cjs`、`scheduler_browser.cjs` 通过。空或 204 权限响应现应清除身份并停用操作，旧 scheduler fixture 已按这一明确合同补充断言，仍仅真实 capability 404 允许 standalone 探测。
- 既有 user_gateway、tenant_cron 四组及 tenant_telegram 七组真实进程回归通过。新增 `gateway_container.py` 验收在真实限额卷/私网容器中签发只读 Key、读取本用户数据、拒绝全部公开修改路由且无 hold、在线轮换继承及撤销；本机 macOS 未执行该 Linux 容器组；PR #81 最终 CI 的 container 作业已完成该组实测。

本批新进程和浏览器 fixture 已加入 CI。PR #81 最终 head `00d013c205e9c92b6649b8738d9d7d39bca966e5` 已通过 [CI 37715609778](https://github.com/jiawenyao401/JiaClaw/actions/runs/37715609778)：Ubuntu、macOS 和 container 三项均为 completed/success，覆盖 Chromium、真实 Docker 沙箱及限额卷/私网网关容器的只读权限验收。协议 fixture 不替代真实供应商/渠道联调，也不将只读权限计为 StateKnot durable 或完整多用户后台认证。

## 可信管理员审计查询批次

本批基于 PR #81 的最终已验证提交，增加 `gateway audit-list`：按用户过滤的同一 SQLite 读快照、有限页、十进制字符串序号/游标/全局水位、保留缺口标记及默认不提取私密 notes。当前仍为 registry schema 3，没有新增公共 HTTP 或 UI 权限；disabled 用户可由可信管理员查询，读取不改变授权、hold 或后台工作。

初次提交的本机验证（936 项，后续修订结果见本节末尾）：

- `cargo test --workspace --locked`：936 项通过（library 372、core 123、host 441），1 项真实 Docker 测试在本机忽略，留给 Linux CI 实测；fmt、所需 Clippy correctness/suspicious 检查和锁定 host 构建通过。
- 新 `tests/gateway_audit.py` 使用两个实际后端、私有 SQLite 与 localhost 模型，四组全部通过、退出码 0。在线路径验证在途写入、Key 轮换与完成事件共享原 request ID；本机供应商 fixture 的 503 保留未知 hold、人工核对 note 的默认隐藏与显式 JSON 转义返回、disabled 用户管理员可查及公共 HTTP 404；按用户过滤的升序分页、尾页全局水位、空页、非法参数/未来游标和未知用户拒绝。
- 第四组离线场景覆盖全局保留剪裁与 potential gap、删除所有事件仍保留高水位、超过 `2^53` 的精确字符串游标、超大私密 note 默认不提取但显式查询拒绝、含控制字符的畸形 action 拒绝。没有用离线注入代替实际写入/模型/人工核对链路。
- registry/CLI 测试覆盖同一读快照、并发写入、分页及全局水位、默认 notes 不提取、显式 notes 有界、畸形查询数据和未来游标拒绝；异常不截断或跳过。
- 既有 `user_gateway.py`、`read_only_keys.py` 五组、`tenant_cron.py` 四组及 `tenant_telegram.py` 七组真实进程回归全部通过、退出码 0。
- `gateway_container.py` 新增管理员审计元数据分页、只读 Key 生命周期、SIGKILL 后原 request ID 恢复、私密 notes 的 JSON 返回、空尾页及 disabled 用户重启后查询断言，`py_compile` 通过。本机 macOS 未执行该 Linux 限额卷/私网容器组；实际验收以本批最终 head CI 为准。

新进程 fixture 已加入跨平台 CI。最终提交的 Ubuntu、macOS 与真实容器检查仍待核对，将在 PR 记录精确 head 和运行结果；不将单页查询计为完整、防篡改或可自动恢复的审计历史。未调用付费供应商或真实渠道。

### CI 暴露问题与诊断

首轮 [CI 37723633396](https://github.com/jiawenyao401/JiaClaw/actions/runs/37723633396) 对应 head `da1434374b5f1a660fe1f6513cd1891948753500`；该轮 container 作业 `113136703895` 已成功，实际覆盖限额卷审计分页、只读 Key 生命周期、SIGKILL 后原 request ID/hold 的核对、显式私密 notes、空尾页及 disabled 用户重启查询。真实 ENOSPC 在 58,675,200 字节文件系统写入 57,028,608 字节后出现，`df` 剩余 0；另一用户与 registry 仍可处理，撤销/禁用持久保留。这是首轮固定提交的容器证据，不代表修订后的最终 head 已通过。

macOS 的既有 semantic 取消生命周期测试暴露了测试同步竞争：`Weak::upgrade()==None` 只观察强引用归零，不能作为结构字段或 `spawn_blocking` 捕获的 Store 已完成析构、所有权锁已释放的同步点。测试已改为在原 5 秒预算内等待实际 Store 打开成功，仅重试明确的所有权忙错误，其他错误立即失败；在途 busy、取消后保留所有权和最终收据断言保持。修复后该严格用例连续 30/30、全量 936 项 Rust 均通过，没有修改生产锁或释放语义。

Ubuntu 的既有 tenant Telegram 基本流程在消息已 delivered 后，等待 hold 结算的原 20 秒预算内失败，原因尚未确定。修改诊断前，本机完整七组和基本流程 20/20 均通过，不能据此将 CI 失败归因于 SQLite、容量或声明问题已经修复。已补充有界诊断：生产 finish 失败只记录静态错误类别、request/user ID 和 known 状态；fixture 使用默认不包含 note 的管理员审计元数据核对。保留原 hold、锁、超时和成功断言，不重放未知工作。诊断修订后的最终二进制进程回归已完成，精确 head CI 仍待核对；真实供应商和完整多用户认证状态不变。

诊断修订后的本机检查：`cargo test --workspace --locked -- --test-threads=1` 全量串行 937 项通过（library 372、core 123、host 442），1 项真实 Docker 测试本机 ignored 留给 Linux CI；fmt、所需 Clippy correctness/suspicious、锁定 host 构建、Python 语法编译和 diff-check 均通过。937 相比初次 936 新增一项静态 finish 错误类别测试，生产诊断不会输出私密错误正文。

本机默认并行运行曾在既有 MCP fixture 的 1 秒初始化截止时间和请求头跟踪 fixture 的 1 秒 guard 失败，MCP 在四线程复验亦失败，而该 MCP 用例隔离运行通过（1.92 秒）。未修改 MCP 实现、生产截止时间或这些 fixture；串行通过不能证明默认并行问题已经解决。最终 CI 继续采用默认并行，须核对其真实结果。最终二进制的五套进程验收已全部通过、退出码 0：`gateway_audit.py` 四组、`user_gateway.py` 整套、`read_only_keys.py` 五组、`tenant_cron.py` 四组和 `tenant_telegram.py` 七组。最终提交的跨平台/容器 CI 仍待核对。

最终二进制定向 SQL 故障注入亦退出码 0：在临时 registry 中，仅对发件回执后的 hold 删除设置失败 trigger；本机平台 fixture 的 delivery 已为 delivered、尝试次数为 1，但 finish 收到 `SQLITE_ABORT` 后保留原 in_flight hold，原 20 秒结算 guard 按预期失败，没有释放锁或重发。诊断只记录静态 `registry_storage`、`known=true` 及匹配的 request/user ID，默认审计元数据保留对应 `telegram_send` 关联。该场景的 stdout 与 gateway 日志均未包含注入的私密错误正文/note、一次性 Key/模型/后端/Bot/webhook Secret 或基本场景 prompt 标记；显式 notes 查询仍遵循前述私密管理边界。

这项故障注入证明诊断可关联真实失败并保留保守停止语义，不能证明首轮 Ubuntu CI 的未知 hold 超时由同一原因引起或已修复。最终 CI 结果将在 PR 记录对应精确 head，并由下一批验证记录回填，避免为记录自身 SHA 反复改提交；完整多用户里程碑与真实供应商/渠道认证仍未完成。

## 独立用户 Slack 批次

2026-10-08 04:38 UTC 回访核对：附着 PR #58–#82 均仍为 OPEN draft，当前 head 检查成功，无 review/thread。PR #82 最终 head `32c5830cc1f6caf008b39149f884bc6b464aac24` 已通过 [CI 37726979505](https://github.com/jiawenyao401/JiaClaw/actions/runs/37726979505)：container job `113147283883`、macOS `113147284066`、Ubuntu `113147284088` 均 completed/success。默认并行 Rust、Telegram、审计、浏览器和真实 Docker/限额卷验收通过；这没有确定首轮 Ubuntu Telegram 超时原因，也不认证全部共享磁盘或供应商能力。

StateKnot main 更新至 `c9318368bbb70fbf6f9318deb961bd2c450227ee`，仅 #141 依赖/CI 管理修改，无 runtime/integration 合同变化；最新仍 0.1.0-alpha.1，#140 OPEN。Brokerrouter main `e01ecb94919d992eb0b74b3db00d70742820b4cc` 未变，无 release，#31/#41 OPEN，PR #40 仍 draft 未合并。没有新可消费的 durable 原生 Schema 合同，继续精确 HTTP MCP 依赖，不提交重复 issue。

本批实现默认关闭的独立用户 Slack；生产配置、范围、容量和人工恢复见[指南](tenant-slack.md)。registry schema 4 保留旧 Key 权限/hold/审计/Telegram，永久预留专用 App 与固定用户/后端/工作区/Bot/成员/DM。原始签名、四次平台身份握手、2.8 秒 ACK、私有队列、共享执行授权和离线复核接入实际路径；后端 protocol 2 永久预留同一身份，request UUIDv7/平台 event ID/固定会话受限，记录与结果原子提交。metadata 收据只是核对依据，没有自动重放或清 hold。

本机验证已完成：

- 全量 Rust 串行 975 项通过（library 372、core 123、host 480），1 项真实 Docker 专项本机 ignored，交给 Linux CI；最终谓词整理及测试锁作用域修订后，runtime 六项定向复验通过。fmt、所需 Clippy correctness/suspicious、锁定构建、Python 语法与 diff-check 通过；仍有 style/pedantic warnings。首次未提升权限的本机全量运行因 localhost/FIFO 被沙箱拒绝而失败；完整验证使用已授权的本地 fixture 权限，没有将权限失败归因于产品。
- 新 registry 十项、SlackStore 八项、runtime 六项、后端七项及 recorded claim 三项测试覆盖 schema 1/2/3 升级与失败回滚、专用 App 终身唯一、owner/OR REPLACE 防改绑、普通/Telegram/未来版本库拒绝、文件权限/链接/独占锁、真实 64 MiB SQLITE_FULL 和 claim 双向事务回滚；处理中的原 request、admitted/completed 收据跨重开保持，不重放模型。
- 最终二进制 `tenant_slack.py` 八组全部通过、退出码 0，运行前后 SHA256 `bd4c3f27ff5906c717d48e42a3869c9ffff2f058ef76c4b87120b7b1f62b5574` 一致。两个真实 JiaClaw 后端、localhost Brokerrouter/Slack 协议 fixture、实际 SQLite/CLI 验证四次平台身份握手、U/W ID、签名/重复头/过期/篡改/2 秒正文/64 KiB/2.8 秒 ACK、普通格式块只用 text、纯文本转义、固定收发与会话隔离。
- 真实 registry BEGIN IMMEDIATE 持锁期间，错误 MAC 仍直接 401，rollback 后 audit/hold/队列/模型/发送不变；同 App 跨工作区绑定也被真实 CLI 拒绝。仅剩只读 HTTP Key 时，独立 Slack 授权仍生效；用户禁用和共享 hold 阻止下一次准入。
- 未知第一片阻挡后续片，429 绝对冷却跨重启、五次上限，停机 inspect/resolve/cancel/purge 和 review-clear 保留原关联；真正 SIGKILL 网关而模型继续提交、真正 SIGKILL 已发 POST，两种恢复均保留原 request ID，无模型或平台自动重发。后端 metadata 从 admitted 到 completed 可核对但不自动解 hold。永久撤销、owner 错误交换、在线维护锁及日志隐私断言通过。
- 同一最终二进制的既有十套进程回归全通过、退出码 0：user_gateway、read_only_keys 五组、gateway_audit 四组、tenant_cron 四组、tenant_telegram 七组、mcp、native_tools、e2e、channels、scheduled_delivery。所有模型/平台调用为本机合成凭据，没有真实安装或付费供应商调用。

新 Slack fixture 已加入 Ubuntu/macOS CI；真实限额卷 container 验收增加 schema 4 App 预留/撤销/重启、默认关闭及私有路由拒绝，但没有配置 Slack runtime，不能据此认证其容器内收发或共享网关卷压力。本机未执行 Linux 容器组；本批最终精确提交的 CI 结果将在 PR 核对并由下一批文档回填。协议 fixture 不认证真实 Slack 安装或 StateKnot durable。
