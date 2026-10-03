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

2026-10-03，macOS arm64，本批最终二进制本地验收完成；Linux/macOS/容器验收以最终 PR 当前 head 的 checks 为准。

- 最终 `tests/model_calls.py` 7 组全部通过：默认关闭零账本；POST UUID/精确字节摘要、原生工具分轮及摘要收据、进程重启读取；摘要响应错误后持久 hold 且不继续聊天 POST；已知远端 UUID 的 GET 状态/无 UUID 头结果恢复，不执行恢复工具或修改原会话；更换 Key/模型不能绕过；缺 UUID/断连的拒重试与显式核对；实际提交后 SIGKILL 重启保留原操作和摘要；scheduler 父运行超时后收据 worker 完成，真实已批准的本机 MCP probe 零 tools/call，运行保持 interrupted。
- 私有目录验收覆盖已有 0755 的外层 state：默认新 `model-calls` 子目录为 0700、数据库为 0600，外层目录保持原权限。发送正文标记不出现在账本文件；CLI status 不展示收据正文；存活服务持有独占所有权，维护 CLI 被拒绝。
- Rust 验证：原全量 814 项通过，另新增 worker 生命周期测试通过，合计 815 项（library 334、core 122、host 359），无失败；真实 Docker 专项仍由独立 CI 执行。新增测试验证取消等待并丢弃调用方后，worker 仍持有数据库独占锁直至收据完成，再打开可见 completed。Clippy correctness/suspicious 与锁定构建通过。既有 e2e、native_tools、model_routing、semantic_memory、scheduler、tenant_cron、scheduled_delivery 七组整机回归通过。

fixture 的 SQLite 检查以 `mode=rw` 打开已有库并立即启用 `query_only`，允许 SQLite 在 WAL 已 checkpoint 后创建自身 sidecar；不写账本记录，也不对运行中的库使用忽略 WAL 的 immutable 模式。

没有使用付费模型或真实供应商凭据。该账本只保存模型调用事实，不是供应商费用主账、工具执行日志或完整 turn 检查点；真实 Brokerrouter、供应商计费、StateKnot durable、SSE 与媒体身份链认证分别保留。配置与操作见[模型调用收据](model-calls.md)。
