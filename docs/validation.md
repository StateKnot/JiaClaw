# 基础能力验收记录

2026-10-09 组织迁移、PR #89/#90 最终三项 CI 回填与安装接线见[组织迁移验收](organization-migration.md)，本批[候选发布](release-candidates.md)独立记录实际归档/原生四平台范围，以及操作 owner 显式释放工作区写锁的生产修复和取消/dup边界。以下各批历史结果仍保留对应固定提交和认证范围。

## MCP Schema worker 所有权与总期限

2026-10-09，本机 macOS arm64、Rust1.88.0、锁定依赖。实际 HTTP MCP 的摘要/编译/参数/输出 Schema 工作与 Brokerrouter 原生定义编译/整批预检接入同一进程四槽阻塞池。MCP 单次调用在输入、HTTP、输出全程使用原始截止；每次原生纯前置校验有30秒等待上限。取消/到期不释放仍在运行或已准入排队的 worker 容量；MCP CPU 阶段的服务器许可也由真实 worker 持有。迟到结果不接受，校验 worker 无工具/网络执行句柄，不会在等待者消失后后台派发。

- 最终生产源码默认并行 Rust **1179 passed**（387/123/669），0 failed；真实 Docker ignored 专项继续由独立 Linux CI 执行。
- MCP **19项**，含八个新增生命周期测试：实际运行 worker 与单 async 线程定时器、四槽已准入排队在取消后继续持有、输入 timeout/cancel 后零 POST、输出 timeout/cancel 后服务器容量保留且不重放、启动总期限/原子注册、五种阶段的 panic 释放与公开错误脱敏、HTTP 前后容量错误的不同结果，以及输入工作与 HTTP 共用原始1200ms预算。已有外部引用/正则预算、MRTR、JSON/分片 SSE、鉴权/redirect、输出 Schema 和传输取消保持通过。
- fmt、diff whitespace、必需 Clippy correctness/suspicious 与 locked build 通过；仍有既存 style/pedantic warnings，不声称全仓 warning-free。
- 同一冻结二进制 SHA256 `02a6b81218499c5d295df01830ba1837791cc02499cb14be435b617d8dc7043b`：`tests/mcp.py`、`tests/native_tools.py`、`tests/e2e.py` 与 `tests/model_calls.py` 七组全部通过，真实 inspect/CLI/HTTP/SQLite、Bearer、原生整批拒绝零副作用、既有副作用后的中断/历史、收据取消与持久未知 hold 均覆盖。

本批自查保留整个批次先验权限/ID/参数检查、单次 HTTP、完整描述 pin、只读 effect、离线引用与限额，不扩权/重放。不能强制终止运行中的 CPU job，也未将任意 Schema 变成 CPU 进程沙箱；资源所有权与有限等待合同见[MCP](mcp.md)和[原生工具](native-tools.md)。上述是本机最终源码与整机 fixture 证据；跨平台全量/容器/四原生候选仍须本批 draft PR 的最终固定 head checks 全部通过，PR #91 的已成功 tree 不能代替本批源码。未使用真实平台/付费凭据，stdio/外部写入、durable/委派、生产流式、钉钉完整安装/WhatsApp、供应商、语义质量及多模态仍开放。

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

基础批次的 Linux/macOS 与容器 CI 已通过，见 [PR #58](https://github.com/StateKnot/JiaClaw/pull/58)。StateKnot durable 尚未接入；上游工具认证仍跟踪 Brokerrouter #31。StateKnot stdio MCP 需求已提交 #140。当前尚未发布 tag、公开 Release 或推送镜像。

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
- 最终二进制既有 e2e、native_tools、memory_io、model_routing 回归全部通过。PR #70 的 head `dae57af27c77f553e6344f5391611b35df454bfe` 已通过 [CI 37082943062](https://github.com/StateKnot/JiaClaw/actions/runs/37082943062)，覆盖 Linux/macOS、Chromium 及真实容器。

CLI `memory semantic status/refresh/search/recover/review-clear/rebuild` 维护同库前须停止 `serve`，没有公开 HTTP 管理路由。测试只使用本机 embeddings/chat fixture、一次性凭据和合成向量，不发送付费模型请求。不能将本批协议验收解释为真实模型检索质量、真实供应商计费或 StateKnot durable Agent 认证。配置、资源与人工核对边界见[语义记忆指南](semantic-memory.md)。


## 独立用户定时任务批次

2026-10-03，macOS arm64，最终二进制本地验收。

- `tests/tenant_cron.py` 4 组通过：真实双后端/网关/本机模型验证受保护能力发现、任务 CRUD/204、输入和分页边界、租户结果隔离、普通 Key 轮换保留用户任务、禁用零派发、共享执行容量及运行期间状态读取、网关停机零自主执行。领取且模型提交后强杀后端与网关，重启保留同一 interrupted run、未知 hold 及 request/run ID 关联；更换 Key 不能解除 hold，另一租户继续运行；明确核对并恢复后只建立新的运行。
- 最终二进制内嵌工作台的 Chromium 验收通过：原有会话流程、任务创建/暂停/恢复/软删、204、聊天 200/任务 201 的待核对标志保留草稿并阻止成功提示、恶意 HTML 纯文本展示、结果截断、身份切换清空任务/结果/草稿和迟到响应隔离。浏览器任务接口使用 route fixture；真实接口由双后端脚本独立验收。
- 全量 Rust：797 passed、0 failed（library 317、core 121、host 359），1 项真实 Docker 专项保持独立 ignored。fmt、diff-check、Clippy correctness/suspicious 和锁定构建通过；既有 e2e、user_gateway、scheduler、scheduled_delivery、model_routing 回归通过。PR #71 最终 head `949aebb` 已通过 [CI 37089235842](https://github.com/StateKnot/JiaClaw/actions/runs/37089235842)，包含 Linux/macOS、Chromium、真实 Docker 沙箱及镜像。首轮 Linux 故障注入触发了非目标租户的合法未知 hold；fixture 已隔离非目标任务并保留全部目标 SIGKILL 断言，修复后完整 CI 通过。

本批使用一次性本机凭据、临时数据库与合成模型响应，没有付费模型调用。保留跨服务未知结果与人工核对边界，不能据此声称真实供应商计费或完整多用户后台能力已认证；见[独立用户定时任务](tenant-cron.md)。


## 模型调用收据批次

2026-10-03，macOS arm64，本批最终二进制本地验收完成；PR #72 最终 head `a841b026` 已通过 [CI 37093371689](https://github.com/StateKnot/JiaClaw/actions/runs/37093371689)，覆盖 Linux/macOS、Chromium 和真实容器。

- 最终 `tests/model_calls.py` 7 组全部通过：默认关闭零账本；POST UUID/精确字节摘要、原生工具分轮及摘要收据、进程重启读取；摘要响应错误后持久 hold 且不继续聊天 POST；已知远端 UUID 的 GET 状态/无 UUID 头结果恢复，不执行恢复工具或修改原会话；更换 Key/模型不能绕过；缺 UUID/断连的拒重试与显式核对；实际提交后 SIGKILL 重启保留原操作和摘要；scheduler 父运行超时后收据 worker 完成，真实已批准的本机 MCP probe 零 tools/call，运行保持 interrupted。
- 私有目录验收覆盖已有 0755 的外层 state：默认新 `model-calls` 子目录为 0700、数据库为 0600，外层目录保持原权限。发送正文标记不出现在账本文件；CLI status 不展示收据正文；存活服务持有独占所有权，维护 CLI 被拒绝。
- Rust 验证：修复后全量 815 项通过（library 334、core 122、host 359），无失败；真实 Docker 专项仍由独立 CI 执行。新增测试验证取消等待并丢弃调用方后，worker 仍持有数据库独占锁直至收据完成，再打开可见 completed。Clippy correctness/suspicious 与锁定构建通过。既有 e2e、native_tools、model_routing、semantic_memory、scheduler、tenant_cron、scheduled_delivery 七组整机回归通过。

fixture 的 SQLite 检查以 `mode=rw` 打开已有库并立即启用 `query_only`，允许 SQLite 在 WAL 已 checkpoint 后创建自身 sidecar；不写账本记录，也不对运行中的库使用忽略 WAL 的 immutable 模式。 查询连接显式关闭，不依赖 Python 垃圾回收。

PR #72 首轮 CI（head `93c653d`、run `37092094733`）在 Linux 的取消收据组暴露了存储层问题，macOS 同组通过：SQLite 建立连接后才关闭预检文件句柄，会在 POSIX 下释放同进程的主库锁；独立只读查询连接关闭时可能误删仍在使用的 WAL。离线 Linux 两进程最小对照验证，旧顺序下写入方看到 completed，而新读者仍看到 submitting；把预检句柄移到 SQLite 建连之前关闭后，两方都读到 completed。模型调用与语义索引两个同型 store 已修复该句柄顺序，fixture 增加有界脱敏状态/门控时间/服务器错误类别和宿主事件诊断，不增加超时或放松成功断言。修复后 PR #72 最终 head `a841b026` 的完整跨平台 CI 已通过；首轮失败及修复证据保留用于追踪。语义记忆新增真实 serve 与独立只读 observer 回归，验证 observer 关闭不移除或替换活跃 WAL/SHM、后续回执跨进程可见、重启查询复用回执不新增 POST；修复后二进制的语义记忆全部 10 组通过。

该文件句柄行为与 [SQLite 官方文件锁说明（2.2）](https://www.sqlite.org/howtocorrupt.html#_posix_advisory_locks_canceled_by_a_separate_thread_doing_close_) 一致。官方说明 3.51.0 增加了部分 WAL 多进程防御；本机观察者为 3.51.0、Linux 最小复现为 3.40.1，因此不将平台表现差异直接归为操作系统本身，也不依赖新版防御代替修复。

没有使用付费模型或真实供应商凭据。该账本只保存模型调用事实，不是供应商费用主账、工具执行日志或完整 turn 检查点；真实 Brokerrouter、供应商计费、StateKnot durable、SSE 与媒体身份链认证分别保留。配置与操作见[模型调用收据](model-calls.md)。

## Discord Bot 定时文字批次

2026-10-03，macOS arm64，最终二进制的 `tests/discord_scheduled.py` 六组真实进程验收全部通过。测试只使用本机模型/Discord HTTP fixture、临时 SQLite 和一次性凭据，覆盖配置与精确授权、应用/guild/type 前置核验零 POST、超过六片的 Unicode 结果与完整 UUID nonce、可信 429/成功耗尽预算的持久安装冷却、401 跨重启与环境新 Token、错回执安装阻断、实际 POST 后 SIGKILL 及人工核查后不重放。既有 `tests/channels.py` 独立回归交互凭据路径。

数据库迁移至 schema v9；全量 Rust 832 项通过（library 334、core 122、host 376），Clippy correctness/suspicious、fmt/diff-check 与锁定构建通过。最终二进制既有 channels、scheduled_delivery 与 model_calls（7 组）回归全部通过；Docker 专项保留独立 CI 验收。fixture 同时检查 API、日志及停服后数据库/保留 sidecar 不含一次性凭据与上游私有错误标记。Token 轮换验收保留原 permanent_failed，并通过人工取消同目标旧失败计划后才发送未来结果，未放宽目标 FIFO。PR #73 最终 head `b99e96690b6ec2ec0265fe68d2895a135cb9e8d5` 已通过 [CI 37095933677](https://github.com/StateKnot/JiaClaw/actions/runs/37095933677)：Ubuntu、macOS、container 均成功，覆盖 Chromium、真实 Docker 与镜像。没有真实 Discord 或付费供应商调用，正式 Bot 安装、guild/频道权限和限流认证仍需独立完成。

## Web 发件箱审计批次

2026-10-03，macOS arm64，最终二进制的本机验收完成。全量 Rust 834 项通过（library 334、core 122、host 378），fmt、Clippy correctness/suspicious 与锁定构建通过；真实 Docker 专项仍由独立 CI 执行。

- `tests/outbox_browser.cjs` 真实 Chromium 全流程通过、退出码 0：真实 webhook/调度与本地模型/Telegram fixture 生成入站及定时多片 outbox，不直接写入产品数据库伪造审计状态。实际平台 POST 后 SIGKILL，重启并禁用 channels/scheduler 后，原投递仍可审计和核对。
- 列表固定 5 条；offset=5 的断网失败保留原页，下一次仍请求 offset=5。UUID 直接定位、记录不存在的 404、submitting/delivered 状态禁写通过。空白、换行或超过 4096 UTF-8 字节的证据在浏览器拒绝，不弹确认、不 POST，并保留草稿。
- 409、提交前断网均触发 GET 核对且不自动重发 POST；GET 失败时禁止修改，直至成功刷新。真实 resolve 在服务端完成并返回 204 后丢失浏览器响应，页面通过 GET 发现 delivered，保留证据而不再次 POST。实际 204 的确认及整来源取消后，原有模型 12 次、平台 6 次请求均未增加。
- 恶意 HTML 纯文本显示，移动端列表行高至少 30 px 且无横向溢出；403/404 隐藏能力，身份切换、401 和迟到响应清空隔离，Token 仅保留在页面内存。既有 `tests/browser.cjs` 的聊天、会话与受限任务回归通过。单条详情 API 鉴权/持久化/UUID/不存在边界和独立用户网关拒绝访问由 Rust 回归验证。

本批没有真实渠道或付费模型请求，不更改平台发送合同，不增加未知消息自动重放，不将人工核对解释为工具或任务恢复。PR #74 最终 head `e22965dd9dc2488c9433d0a93f7b9e1bf59304a5` 已通过 [CI 37098122253](https://github.com/StateKnot/JiaClaw/actions/runs/37098122253)：Ubuntu、macOS 与 container 均成功，包含 Chromium 和真实容器。操作与权限说明见 [Web 发件箱指南](web-outbox.md)。

## 单实例定时任务工作台批次

2026-10-03，全量 Rust 845 项通过（library 334、core 122、host 389），1 项真实容器测试在本机保持 ignored，由独立 CI 执行。fmt、Clippy correctness/suspicious、Node 语法检查及锁定构建通过。

- 含最后筛选修复的最终二进制通过三套真实 Chromium：`tests/scheduler_browser.cjs`、`tests/browser.cjs`、`tests/outbox_browser.cjs`，均退出码 0。新脚本通过本机模型、实际 host 和 SQLite 验证创建/运行/暂停恢复/删除/purge/重启、原生 interval 工具循环、含时区的未来 cron，以及已有 600 秒和渠道目的地等高级授权字段的展示。
- 创建的实际 201 响应丢失后，GET 核对保留已暂停状态；创建后软删除且 GET 失败，再以同 ID/快照 PUT 返回已删除状态，不复活任务。真实 purge 后 PUT409，再 GET404，页面继续显示永久退役原因、保留 ID/草稿并禁用重试；查询不自动 PUT 或更换 ID，只有显式放弃才解除本页跟踪。
- 任务每页 5 项、运行每页 1 项；翻页失败不提交新 offset。更改“显示已删除”筛选的请求失败时，复选框、旧列表和分页保持原值，下一页继续使用原筛选。近 990 KiB 的响应可读，超过 2 MiB 的页面拒绝且不换页；gateway false、200/null、204 和旧 status 无协议均不触发 standalone 回退。
- 实际模型 503 产生的 failed run 同时显示保存响应和错误；浏览器 failed/stopping 健康状态、响应体积等边界使用协议注入，不能描述为真实数据库故障。XSS 纯文本、身份切换/401/迟到响应、移动布局及旧网关任务/发件箱流程通过。
- 同批 schema 10 后端的 `tests/scheduled_delivery.py` 和 `tests/discord_scheduled.py`（六组）进程回归均退出码 0；这两项使用最后 UI 筛选修复前的二进制，之后后端未改动。Rust 回归覆盖规范 UUIDv4、typed 指纹、异体与旧任务/遗留会话拒绝认领、同 ID 并发、创建事务回滚、重启及 purge 墓碑、永久容量和模式/权限边界。

本批只使用一次性本地凭据与协议 fixture，没有付费模型或真实平台请求。跨平台与真实容器由本批 draft PR 最终 head CI 验证；配置、备份和恢复边界见[单实例工作台](standalone-scheduler.md)。

PR #75 首轮 head `1a14cad55b9602ee6b7a71055671b3ef03c9ab3f` 的 [CI 37100634034](https://github.com/StateKnot/JiaClaw/actions/runs/37100634034) 在 Linux 的 `tests/tenant_cron.py:307` 遇到 `GET /api/sessions` 返回 429（`user request already in progress`）。运行状态属于控制请求，可先读到后端 completed；此时网关尚未完成 `finish_write` 收据提交，仍持有执行许可，会话查询使用执行容量，因此该 429 是正常的在途保护，不能以 completed 状态推定执行许可已经释放。

修正仅限 fixture 的两处会话列表断言：遇到精确匹配的 busy 429 时，对只读 GET 作有界轮询，其他错误仍直接失败；另通过暂停模型响应确定性验证在途期间仍返回该 429。不自动重放写入，不提前释放生产执行许可，也不放宽后端准入。生产代码无需修改；修正后的真实双租户进程验收四组通过、退出码 0，原 SIGKILL/持久 hold/Key 轮换/其他用户继续及零模型重发断言保留。修正后 PR #75 最终 head `0c9900a75a8f5a3a1c980c8ba98b18c3430b12d2` 的 [CI 37101387934](https://github.com/StateKnot/JiaClaw/actions/runs/37101387934) 已通过 Ubuntu、macOS 和 container 三项检查，覆盖 Chromium 与真实容器；首轮失败记录保留用于追踪。

## 工作区文件权限与 I/O 批次

最终本机全量 Rust 854 项通过（library 343、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI；fmt、Clippy correctness/suspicious、锁定构建与 diff-check 通过，保留既有 style/pedantic warnings。

- 新 `tests/workspace_files.py` 已接入 Linux/macOS CI，最终真实二进制四组全部通过：主名称/别名 schema 和 JSON、一致的 UTF-8 落盘字节、真实创建/追加/读取/替换/目录/删除；通过小参数追加达到精确 262144 字节，行范围读取和追加/替换超限拒绝保留原文件；内外部符号链接、损坏链接、硬链接、FIFO、路径穿越及缺失父目录外逸均拒绝，外部哨兵文件不变；四个配置开关同时从 HTTP/native 目录移除八个名称，禁用名称与伪造别名在整批执行前被拒绝。
- 文件增长的进程测试是在读取前向文件追加后再验证拒绝；读取过程最多消耗 limit+1 字节由 Rust 的受控 reader 测试验证。Rust 还验证最终 pretty JSON ≤64 KiB（含转义名称）、扫描 2000 条、递归 32 层、过期遍历预算、共享目录锁/并发追加及取消后的阻塞容量，不把这些证据归到 Python fixture。
- 最终二进制的 `tests/native_tools.py`、`tests/memory_io.py`（四组）和 `tests/e2e.py` 回归全部通过、退出码 0；记忆回归继续覆盖旧临时文件守护、锁竞争、配置路径与超大 HEARTBEAT 零模型调用。

测试只使用本机模型协议、一次性凭据和临时文件，没有真实平台或付费模型调用。跨平台与真实容器以本批 draft PR 最终 head CI 为准，不以单机协议验收代替断电硬件持久性或完整个人 Agent 生产认证。

PR #76 最终 head `277a89a5ade1e4ab84d7c17d696c004b7fd1e7ea` 已通过 [CI 37103576477](https://github.com/StateKnot/JiaClaw/actions/runs/37103576477)：Ubuntu、macOS、container 均成功，覆盖 Chromium 与真实容器。

该批目录句柄迁移限于 `read_file`、`write_file`、`delete_file`、`str_replace`、`list_dir` 及四个兼容名称。`grep`、`glob`、`mkdir`、`move` 未迁移，`copy` 保留独立合同；没有新增 `stat` / `tree` 或后台文件工具授权。操作与兼容边界见[工作区文件指南](workspace-files.md)。

## grep / glob 有界搜索批次

最终本机全量 Rust 864 项通过（library 353、core 122、host 389），1 项真实 Docker 专项本地 ignored 交 CI；fmt、Clippy correctness/suspicious、锁定构建与 diff-check 通过，保留既有 style/pedantic warnings。

- 最终二进制新增 `tests/file_search.py` 六组通过、退出码 0：字面量/Unicode 小写匹配、行号和路径、glob 排序/过滤及 `.git` 排除；精确 256 KiB、目录中超大/二进制跳过与显式拒绝、glob 不读取正文；内部/外部/父目录/损坏链接、硬链接与 FIFO 拒绝，外部哨兵不泄露；禁用搜索工具从 HTTP/native 目录移除，伪造批次不执行前面的获准工具，非法请求白名单零模型请求。
- 真实进程通过 2005 个空目录、34 层目录树和 65 个各 256 KiB 文件触发对应截断；glob 过滤后不读取正文，80 个小文件完整搜索。400 个带大量引号的路径及 Unicode snippet 保持最终 pretty JSON ≤64 KiB；20 个 `**` 对 22 层路径的失败匹配结束。上述结果验证实际链路，不用完成耗时代替精确资源计数。
- Rust 确定性测试另验证条目计数覆盖目录和被跳过项、深度/路径/过期 deadline、实际字节预算和不搜索半文件、metadata 检查后真实文件增长最多读取 limit+1 字节并计入总预算、父目录改名后仍使用所持目录能力、叶子替换拒绝。glob 最终全路径排序后才应用请求上限，grep 保留 DFS；动态规划有受限最大模式与独立小输入参考比对。
- 非 UTF-8 名称拒绝专项在 Linux 执行；本机 APFS 不允许构造该名称，因此未把 macOS 通过列为此专项证据。最终二进制的 `workspace_files.py` 四组、`native_tools.py`、`memory_io.py` 四组及 `e2e.py` 全部通过、退出码 0。

该批只使用本机模型协议、一次性凭据和临时文件，没有真实供应商或平台请求。PR #77 最终 head `bc7ef30467ad8c436585eeee4b1cfc99d16ef68f` 已通过 [CI 37105552392](https://github.com/StateKnot/JiaClaw/actions/runs/37105552392)，含 Linux/macOS、Chromium 与真实容器。

本轮仅迁移 grep/glob 的工作区访问和资源边界，不新增后台授权、跨编辑器快照或取消后回滚。`mkdir` / `move` 和可选 `stat` / `tree` 不记为完成，copy 保留原独立合同；详见[工作区文件指南](workspace-files.md)。

## mkdir / move 原子变更批次

最终本机全量 Rust 875 项通过（library 364、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI。fmt、Clippy correctness/suspicious 与锁定 host 构建通过，保留既有 style/pedantic warnings；`memory_io` 定向 24 项通过，含 10 项新增本机 mutation 测试。

- 最终生产二进制 `tests/workspace_mutations.py` 六组通过、退出码 0：mkdir 递归/parents 别名、普通目录和 `.` 幂等 inode、缺失父目录及冲突参数拒绝；move 的主名/别名四种组合、超过 1 MiB 的二进制和非空目录同卷 rename 保留 dev/inode/字节，源目录内链接与 FIFO 原样移动且不被跟随；默认不覆盖、同类型文件/空目录覆盖、非空目的地/类型不符/同源/自身子目录/缺失父目录失败保留两端；静态链接/硬链接/FIFO/越界拒绝；64 组件与 1024 字节边界、真实工作区锁争用及释放后继续；禁用配置从目录移除工具，伪造混合批次零工具效果、非法显式白名单零模型请求。
- Rust 本机测试覆盖真实内核 NO_REPLACE：在预检后创建目标，单次 rename 仍拒绝覆盖；EXDEV / ENOSYS / EACCES 受控错误注入不触发 copy/delete 回退并保留两端；已打开父目录在宿主路径替换后继续受句柄约束；叶子替换竞争不跟随链接目标；同 inode 和 macOS 大小写别名的自身子路径拒绝。追加的定向 mkdir 回归也通过：总路径合法但后续单个文件名超过文件系统上限时，先创建的 `partial` 父目录保留且为空；人工核对后显式继续创建可成功。这里验证的是后续创建失败，不是目录 fsync 故障注入。该证据不提供非协作编辑器的 inode compare-and-swap 保证。
- 进程 fixture 不挂载第二个文件系统，本机 macOS 没有真实跨卷验收。Linux Rust 新增真实 EXDEV 专项，要求 `/dev/shm` 可写且与临时目录为不同设备，CI 缺少前提直接失败，非 CI 仅允许明确报告跳过；其通过状态以本批最终 CI 为准。
- 同一最终生产二进制的 `workspace_files.py` 四组、`file_search.py` 六组、`native_tools.py` 与 `memory_io.py` 四组回归全部通过、退出码 0。本批未单独重跑通用 `e2e.py`，由最终 CI 执行，不沿用上批本机结果。

只使用本机模型协议、一次性凭据和临时文件，没有真实供应商或平台请求。PR #78 最终 head `9690692bbb208fe5bebac3eab69a73540e335c16` 已通过 [CI 37107782116](https://github.com/StateKnot/JiaClaw/actions/runs/37107782116)，Ubuntu、macOS 与 container 均成功，含 Chromium、真实容器和 Linux 真实 EXDEV 专项。

此批不增加后台文件工具授权、非协作编辑器快照、操作收据或取消后回滚；copy 保留独立实现，该批没有新增 stat/tree 或 durable 工作流。配置与恢复步骤见[工作区文件指南](workspace-files.md)。

## stat / tree 文件信息批次

本轮增加 WorkspaceStat/WorkspaceTree 与独立配置开关，并将 list_dir 共享 walker 的路径预算收紧到完整工作区相对路径。最终本机全量 Rust 884 项通过（library 372、core 123、host 389），1 项真实 Docker 专项本地 ignored 由 Linux CI 执行；fmt、Clippy correctness/suspicious 和锁定 host 构建通过，仍有 style/pedantic warnings。

- 最终含严格 JSON object 检查的二进制运行 `tests/filesystem_info.py`，六组全部通过、退出码 0：默认根/精确可选字段与固定毫秒 mtime、文本/二进制和 64 MiB+3 稀疏文件元数据；所选目录相对名称、每层排序 DFS、隐藏/`.git`、条数/深度及空目录边界保守截断；叶子符号链接/损坏链接/硬链接/FIFO 仅显示类型、链接父目录与非法根拒绝、不跟随外部目标，持有写锁时只读查询仍可完成。
- 同一新 fixture 验证严格参数类型/未知字段/范围、64 组件和 32 层边界；2005 个空目录及含大量转义名称的完整 pretty JSON ≤64 KiB；stat/tree 独立开关、HTTP/native 工具目录一致、显式白名单与伪造混合批次零先前写入效果，非法调用方授权零模型请求。它不以耗时观察代替精确 deadline 或计数证明。
- Rust 测试验证规范路径、epoch 前时间向下取整、父目录能力保留和叶子替换不跟随、完整工作区路径 1024 字节/64 组件在 lstat 前限制及 list_dir 同步收紧、tree DFS/深度和完整输出预算；既有共享 I/O 取消/许可与精确扫描/deadline 回归同样通过。非 UTF-8 文件名专项仅在 Linux 执行，本机 APFS 不允许构造该名称，不将其算作本机覆盖。
- 本批七套既有真实进程回归 `workspace_files.py`（四组）、`file_search.py`（六组）、`native_tools.py`、`mcp.py`、`memory_io.py`（四组）、`e2e.py`、`workspace_mutations.py`（六组）全部通过、退出码 0。

测试只使用本机模型协议、一次性凭据和临时文件，没有真实模型或平台请求。PR #79 最终 head `768cfba3041c7c863e14cbdcc17d6b13ef6470d5` 已通过 [CI 37110081396](https://github.com/StateKnot/JiaClaw/actions/runs/37110081396)，Ubuntu、macOS 与 container 三项均成功，包含 Chromium 与真实容器。

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

PR #80 最终 head `68b3a22867e65ed32154c4fc2292066da6f842b4` 已通过 [CI 37113957145](https://github.com/StateKnot/JiaClaw/actions/runs/37113957145)，含 Linux/macOS、Chromium 与真实容器。部署与人工核对见[指南](tenant-telegram.md)；真实 Telegram、TLS/egress、共享网关卷压力与供应商联调不由本机协议 fixture 认证。

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

本批新进程和浏览器 fixture 已加入 CI。PR #81 最终 head `00d013c205e9c92b6649b8738d9d7d39bca966e5` 已通过 [CI 37715609778](https://github.com/StateKnot/JiaClaw/actions/runs/37715609778)：Ubuntu、macOS 和 container 三项均为 completed/success，覆盖 Chromium、真实 Docker 沙箱及限额卷/私网网关容器的只读权限验收。协议 fixture 不替代真实供应商/渠道联调，也不将只读权限计为 StateKnot durable 或完整多用户后台认证。

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

首轮 [CI 37723633396](https://github.com/StateKnot/JiaClaw/actions/runs/37723633396) 对应 head `da1434374b5f1a660fe1f6513cd1891948753500`；该轮 container 作业 `113136703895` 已成功，实际覆盖限额卷审计分页、只读 Key 生命周期、SIGKILL 后原 request ID/hold 的核对、显式私密 notes、空尾页及 disabled 用户重启查询。真实 ENOSPC 在 58,675,200 字节文件系统写入 57,028,608 字节后出现，`df` 剩余 0；另一用户与 registry 仍可处理，撤销/禁用持久保留。这是首轮固定提交的容器证据，不代表修订后的最终 head 已通过。

macOS 的既有 semantic 取消生命周期测试暴露了测试同步竞争：`Weak::upgrade()==None` 只观察强引用归零，不能作为结构字段或 `spawn_blocking` 捕获的 Store 已完成析构、所有权锁已释放的同步点。测试已改为在原 5 秒预算内等待实际 Store 打开成功，仅重试明确的所有权忙错误，其他错误立即失败；在途 busy、取消后保留所有权和最终收据断言保持。修复后该严格用例连续 30/30、全量 936 项 Rust 均通过，没有修改生产锁或释放语义。

Ubuntu 的既有 tenant Telegram 基本流程在消息已 delivered 后，等待 hold 结算的原 20 秒预算内失败，原因尚未确定。修改诊断前，本机完整七组和基本流程 20/20 均通过，不能据此将 CI 失败归因于 SQLite、容量或声明问题已经修复。已补充有界诊断：生产 finish 失败只记录静态错误类别、request/user ID 和 known 状态；fixture 使用默认不包含 note 的管理员审计元数据核对。保留原 hold、锁、超时和成功断言，不重放未知工作。诊断修订后的最终二进制进程回归已完成，精确 head CI 仍待核对；真实供应商和完整多用户认证状态不变。

诊断修订后的本机检查：`cargo test --workspace --locked -- --test-threads=1` 全量串行 937 项通过（library 372、core 123、host 442），1 项真实 Docker 测试本机 ignored 留给 Linux CI；fmt、所需 Clippy correctness/suspicious、锁定 host 构建、Python 语法编译和 diff-check 均通过。937 相比初次 936 新增一项静态 finish 错误类别测试，生产诊断不会输出私密错误正文。

本机默认并行运行曾在既有 MCP fixture 的 1 秒初始化截止时间和请求头跟踪 fixture 的 1 秒 guard 失败，MCP 在四线程复验亦失败，而该 MCP 用例隔离运行通过（1.92 秒）。未修改 MCP 实现、生产截止时间或这些 fixture；串行通过不能证明默认并行问题已经解决。最终 CI 继续采用默认并行，须核对其真实结果。最终二进制的五套进程验收已全部通过、退出码 0：`gateway_audit.py` 四组、`user_gateway.py` 整套、`read_only_keys.py` 五组、`tenant_cron.py` 四组和 `tenant_telegram.py` 七组。最终提交的跨平台/容器 CI 仍待核对。

最终二进制定向 SQL 故障注入亦退出码 0：在临时 registry 中，仅对发件回执后的 hold 删除设置失败 trigger；本机平台 fixture 的 delivery 已为 delivered、尝试次数为 1，但 finish 收到 `SQLITE_ABORT` 后保留原 in_flight hold，原 20 秒结算 guard 按预期失败，没有释放锁或重发。诊断只记录静态 `registry_storage`、`known=true` 及匹配的 request/user ID，默认审计元数据保留对应 `telegram_send` 关联。该场景的 stdout 与 gateway 日志均未包含注入的私密错误正文/note、一次性 Key/模型/后端/Bot/webhook Secret 或基本场景 prompt 标记；显式 notes 查询仍遵循前述私密管理边界。

这项故障注入证明诊断可关联真实失败并保留保守停止语义，不能证明首轮 Ubuntu CI 的未知 hold 超时由同一原因引起或已修复。最终 CI 结果将在 PR 记录对应精确 head，并由下一批验证记录回填，避免为记录自身 SHA 反复改提交；完整多用户里程碑与真实供应商/渠道认证仍未完成。

## 独立用户 Slack 批次

2026-10-08 04:38 UTC 回访核对：附着 PR #58–#82 均仍为 OPEN draft，当前 head 检查成功，无 review/thread。PR #82 最终 head `32c5830cc1f6caf008b39149f884bc6b464aac24` 已通过 [CI 37726979505](https://github.com/StateKnot/JiaClaw/actions/runs/37726979505)：container job `113147283883`、macOS `113147284066`、Ubuntu `113147284088` 均 completed/success。默认并行 Rust、Telegram、审计、浏览器和真实 Docker/限额卷验收通过；这没有确定首轮 Ubuntu Telegram 超时原因，也不认证全部共享磁盘或供应商能力。

StateKnot main 更新至 `c9318368bbb70fbf6f9318deb961bd2c450227ee`，仅 #141 依赖/CI 管理修改，无 runtime/integration 合同变化；最新仍 0.1.0-alpha.1，#140 OPEN。Brokerrouter main `e01ecb94919d992eb0b74b3db00d70742820b4cc` 未变，无 release，#31/#41 OPEN，PR #40 仍 draft 未合并。没有新可消费的 durable 原生 Schema 合同，继续精确 HTTP MCP 依赖，不提交重复 issue。

本批实现默认关闭的独立用户 Slack；生产配置、范围、容量和人工恢复见[指南](tenant-slack.md)。registry schema 4 保留旧 Key 权限/hold/审计/Telegram，永久预留专用 App 与固定用户/后端/工作区/Bot/成员/DM。原始签名、四次平台身份握手、2.8 秒 ACK、私有队列、共享执行授权和离线复核接入实际路径；后端 protocol 2 永久预留同一身份，request UUIDv7/平台 event ID/固定会话受限，记录与结果原子提交。metadata 收据只是核对依据，没有自动重放或清 hold。

首版实现的本机验证已完成（下述二进制哈希对应首版，后续修订单独复验）：

- 全量 Rust 串行 975 项通过（library 372、core 123、host 480），1 项真实 Docker 专项本机 ignored，交给 Linux CI；最终谓词整理及测试锁作用域修订后，runtime 六项定向复验通过。fmt、所需 Clippy correctness/suspicious、锁定构建、Python 语法与 diff-check 通过；仍有 style/pedantic warnings。首次未提升权限的本机全量运行因 localhost/FIFO 被沙箱拒绝而失败；完整验证使用已授权的本地 fixture 权限，没有将权限失败归因于产品。
- 新 registry 十项、SlackStore 八项、runtime 六项、后端七项及 recorded claim 三项测试覆盖 schema 1/2/3 升级与失败回滚、专用 App 终身唯一、owner/OR REPLACE 防改绑、普通/Telegram/未来版本库拒绝、文件权限/链接/独占锁、真实 64 MiB SQLITE_FULL 和 claim 双向事务回滚；处理中的原 request、admitted/completed 收据跨重开保持，不重放模型。
- 最终二进制 `tenant_slack.py` 八组全部通过、退出码 0，运行前后 SHA256 `bd4c3f27ff5906c717d48e42a3869c9ffff2f058ef76c4b87120b7b1f62b5574` 一致。两个真实 JiaClaw 后端、localhost Brokerrouter/Slack 协议 fixture、实际 SQLite/CLI 验证四次平台身份握手、U/W ID、签名/重复头/过期/篡改/2 秒正文/64 KiB/2.8 秒 ACK、普通格式块只用 text、纯文本转义、固定收发与会话隔离。
- 真实 registry BEGIN IMMEDIATE 持锁期间，错误 MAC 仍直接 401，rollback 后 audit/hold/队列/模型/发送不变；同 App 跨工作区绑定也被真实 CLI 拒绝。仅剩只读 HTTP Key 时，独立 Slack 授权仍生效；用户禁用和共享 hold 阻止下一次准入。
- 未知第一片阻挡后续片，429 绝对冷却跨重启、五次上限，停机 inspect/resolve/cancel/purge 和 review-clear 保留原关联；真正 SIGKILL 网关而模型继续提交、真正 SIGKILL 已发 POST，两种恢复均保留原 request ID，无模型或平台自动重发。后端 metadata 从 admitted 到 completed 可核对但不自动解 hold。永久撤销、owner 错误交换、在线维护锁及日志隐私断言通过。
- 同一最终二进制的既有十套进程回归全通过、退出码 0：user_gateway、read_only_keys 五组、gateway_audit 四组、tenant_cron 四组、tenant_telegram 七组、mcp、native_tools、e2e、channels、scheduled_delivery。所有模型/平台调用为本机合成凭据，没有真实安装或付费供应商调用。

新 Slack fixture 已加入 Ubuntu/macOS CI；真实限额卷 container 验收增加 schema 4 App 预留/撤销/重启、默认关闭及私有路由拒绝，但没有配置 Slack runtime，不能据此认证其容器内收发或共享网关卷压力。本机未执行 Linux 容器组；本批最终精确提交的 CI 结果将在 PR 核对并由下一批文档回填。协议 fixture 不认证真实 Slack 安装或 StateKnot durable。

本批交付前 05:26 UTC 复核发现 StateKnot main 再推进到 `4e3c9e9194db524886ca795e5e2394be071ea202`；#145 只更新依赖 patch 与 Dependabot 分组，无运行合同源码变化，17 项 main 检查均成功，release/#140 未变。独立 review 修正 Slack review-clear 示例缺少必需的 `--confirm-backend-idle`；实际 fixture 已正确传此标志。

首版 head `349a5f41a3a6cb3aeddd6372d9925fd78e822814` 的 [CI 37732172100](https://github.com/StateKnot/JiaClaw/actions/runs/37732172100) 中，Ubuntu job `113163547361` 全部成功；macOS job `113163547668` 与 container job `113163547629` 失败，不能把首版本机通过记为最终 CI 通过。容器的新 Slack 预留/撤销/重启组已通过，旧审计组却在已有保留 Slack 绑定后在线清 hold，正确触发停机锁拒绝。fixture 现在先断言在线拒绝不改变 hold/审计，再停止网关，以相同非 root/只读根文件系统/限额卷的维护容器清 hold，并重启继续原断言；不取消生产锁。

macOS 的旧记忆并发追加测试在所有写线程 join 后立即取目录 flock，得到 EWOULDBLOCK。真实 fork/pipe 实验确认：带 CLOEXEC 的目录描述符仍能在子进程 exec/关闭前保留 flock 引用，父线程完成不是内核锁释放的充分条件。初始 CI 没有采集持锁进程，因此不认定具体 child 是该次失败原因。测试在原 200×2 ms 争抢预算内只重试固定 flock 上下文的 EWOULDBLOCK；其他错误立即失败，生产仍非阻塞。另以真实 dup 引用及释放同步验证等待内核锁，不增加 CI 截止时间或改为串行 CI。

独立 review 发现首次初始化直接创建 final 文件会在 owner 提交前崩溃时留下不可收养空库。修订改为完整 owner 暂存事务、关闭/文件 fsync、同目录 NOREPLACE 发布和目录 fsync。恢复只接受有界私有空暂存库或 exact owner-only schema；部分写入、热 journal、WAL header/sidecar、外来 owner 均保留并拒绝，不改动未知 final。真实 WAL 主文件无 sidecar 的只读打开会创建 WAL，故增加打开前文件头检查及“不改变文件/sidecar”回归。详细人工处置边界见[Slack 指南](tenant-slack.md)。

依赖复核确认首版锁定的 libsqlite3-sys 0.30.1 内含 SQLite 3.46.0，处于官方列出的 WAL-reset 并发 write/checkpoint 缺陷影响版本；本项目没有复现该罕见损坏。为移除已知风险，两处 rusqlite 精确升级为 0.39.0，锁定 libsqlite3-sys 0.37.0 / bundled SQLite 3.51.3，并显式保留 fallible_uint 的有检查整数转换。此缺陷是已有 SQLite 依赖风险，不是 StateKnot/Brokerrouter 合同缺陷。验证须在升级后重做，不能引用旧二进制哈希代替。rusqlite 上游只承诺发布时最新 stable，不保证 Rust 1.88；本项目的精确特性/依赖组合由实际 1.88.0 构建及同版本 CI 验收。[SQLite 官方缺陷和修复说明](https://sqlite.org/wal.html#the_wal_reset_bug)、[rusqlite v0.39.0](https://github.com/rusqlite/rusqlite/releases/tag/v0.39.0)

修订后的最终源码验证完成：

- 实际 Rust 1.88.0 全目标锁定 check、984 项完整串行 Rust（library 373、core 123、host 488；真实 Docker 一项本机 ignored）、fmt、所需 Clippy correctness/suspicious、锁定 build、Python 语法及 diff-check 均通过；仍有 style/pedantic warnings。SlackStore 原八项及新增七项全部通过；实际链接 SQLite 修复版本、负整数读取到 usize 拒绝、超 i64 的 u64 绑定拒绝和普通整数往返均验证。
- macOS 真实 fork/pipe 验证锁引用机制；原并发追加与真实 dup 锁释放测试分别连续 30/30，最终 memory_io 模块默认并行 30 项通过。只改测试同步，不改变生产锁、CI 并行方式或原争抢预算。
- 最终重建二进制 SHA256 `cb15ba29d69daf0c656193c5c94625a11ae0f703c7bef1104b08758b874f6c1e`：新 `tenant_slack.py` 八组完整通过，十五套既有进程回归全部通过（user_gateway、read_only_keys、gateway_audit、tenant_cron、tenant_telegram、mcp、native_tools、e2e、channels、scheduled_delivery、memory_io、scheduler、model_routing、semantic_memory、model_calls）。每套及整体运行前后哈希保持一致，未修改 fixture 或在验收中重建。所有调用仍为 localhost 合成凭据。
- 最终只读复核确认 owner 发布、未知文件保留、严格锁竞争错误、真实维护容器及 SQLite 整数转换边界无剩余实质问题；维护容器采用追踪后显式删除，失败由原清理路径处理。PR #83 最终 head `af9265ccc5e321ab98e0916f0deac2164b2780b3` 已通过 [CI 37735649553](https://github.com/StateKnot/JiaClaw/actions/runs/37735649553)：macOS job `113174436581`、container job `113174436806`、Ubuntu job `113174436901` 均 completed/success。覆盖默认并行 Rust、所有进程 fixture、Chromium 和真实 Docker/限额卷验收；该容器作业没有开启 Slack runtime，不据此认证真实平台收发。

## 独立用户 Discord 批次

2026-10-08 06:32 UTC 官方 GitHub API 复核：附着 PR #58–#83 均 OPEN draft，各自 head 检查 SUCCESS，无 review/thread。当前本批基于 PR #83 上述最终已验证提交；源文档回填了其精确 head、三个成功作业和首轮失败的实际修订，不将旧二进制或首轮 Ubuntu 成功记为本批证据。

StateKnot main 为 `9110ad71934e446d9fbb8ff21387cf14a7b7bdc6`，12 项检查全成功。新 #146 有实际源码合同：受限 RFC 9068 RS256 JWT/JWKS 身份、可信公钥 provision/CAS 轮换、共享过期租户策略与独立 resource 授权；#147 增加执行/凭据不可持久序列化的 compile-fail 测试及生产验收 ledger。公开 release 仍为 `0.1.0-alpha.1`，不包含新身份 profile，JiaClaw 当前未消费它。stdio #140 OPEN、无回复，runtime/integrations/native Schema 合同未变化。Brokerrouter main `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无 release、#31/#41 和 draft PR #40 均未变；再次读取 Rust check `107571112146` 注释确认六个 FAILURE 检查中的该作业因账户付款/额度未启动。没有新可消费的 durable 原生输出合同，不重复提交 issue。

本批实现默认关闭的[独立用户 Discord](tenant-discord.md)：registry schema 5 的永久 App/公钥/人/Bot DM/固定全局命令绑定、签名先行的持久 ephemeral 准入、protocol 3 专属后端收据、密钥指纹固定的加密队列和撤销后无 Secret 的停机核对。仅 USER_INSTALL/BOT_DM 文本，original PATCH 加最多五条显式 ephemeral followup；不自动注册平台命令或修改 App。真实 App 安装、授权字段/终端流程、公共 TLS 总延迟、平台限额和真实供应商认证尚未执行。

本批最终本地证据（跨平台及真实容器以本批 draft PR 最终 head 的 CI 记录为准）：

- `CARGO_INCREMENTAL=0 cargo test -p jiaclaw-host --locked gateway::discord_store::tests -- --test-threads=1`：21 项通过。真实 SQLite 覆盖完整 owner/密钥指纹、不可 UPDATE/DELETE/REPLACE、生命周期锁、原子 NOREPLACE 发布、空/完整 stage 恢复、SQLITE_FULL 回滚、foreign/partial/WAL header 与 sidecar 保留、0700/0600 和单链接拒绝、64 MiB page quota、recorded claim 双向回滚、未知工作原 operation 保留。
- 无配置 Secret 的 stopped maintenance 覆盖完全无状态时不创建目录/锁/库、完整已提交 stage 从非秘密指纹恢复、空/部分 stage 拒绝、孤立 sidecar 不误报 None、撤销后完整 owner 校验及非法 persisted fingerprint 拒绝。到期并被 FIFO/冷却/attempt 阻塞的片仍提示一次 claim，持久改为 expired，不记录 send operation。
- 全部冻结源码的默认并行 `cargo test --workspace --locked`：1042 项通过（library 373 / core 123 / host 546；真实 Docker 一项本机 ignored）。此前完整串行 1041 项及追加 429 对象测试后的 host 默认并行 546 项也通过；最终数以上述完整默认并行为准。Rust 1.88.0 的 fmt、必需 Clippy correctness/suspicious、locked 全目标 check/build、Python 语法及 diff check 通过，仍有 style/pedantic warnings。
- 最终重建二进制 SHA256 `e2f725408abce9eba8651ce6300e57c2ff2c122dc32eb5a415b23193703a3e06`：`tenant_discord.py` 九组完整通过；十六套旧进程回归全部通过（user_gateway、read_only_keys、gateway_audit、tenant_cron、tenant_telegram、tenant_slack、mcp、native_tools、e2e、channels、scheduled_delivery、memory_io、scheduler、model_routing、semantic_memory、model_calls）。每套和整体前后哈希相同，测试同步修订后再次 locked 构建仍产生相同二进制，没有使用旧 artifact 代替代码验收。
- 新整机使用两个真实 JiaClaw 后端、真实 Ed25519 原始签名、真实原生工具往返和 SQLite；覆盖签名前的实际 registry 写竞争拒绝、User Install 与 DM/命令 identity、加密凭据/原 key 指纹、六片 Unicode ephemeral 回复、unknown PATCH/POST 的后续阻断及人工核对、429 重启/五次上限/到期 hold、模型与 PATCH/POST 中实际 SIGKILL、1000 条真实签名持久准入与第 1001 条拒绝、停机分页和 revoked/no-runtime 全保留队列检查。仅 localhost 合成凭据；SQLite 文件无明文 interaction 凭据，日志不含 Secrets，inspect 不含明文或密文 token。
- 独立 review 修正了模型返回后、结果落盘时凭据期限已用尽却可能清 hold 的边界：获得结算槽和 store mutex 后重查发送余量，过期原子写 needs_review/无 outbox，慢提交后再保守保留 hold。真实私有库 recorded claim 回归覆盖已计算模型结果的到期提交。私有发送成功回执及 429 均要求 JSON 对象，拒绝 positional array，保留原字节重复字段拒绝。
- CI 增加 Linux/macOS 的九组 Discord 整机验收；真实容器组新增 schema 5 App 预留、撤销重启、不可重分配、默认关闭和私有路由拒绝，保留硬配额卷与原停机 hold-clear 流程。容器案例不启用 Discord runtime，不能代替真实平台/TLS 或共享磁盘 runtime 压力认证。最终 exact-head CI 已在本节下文回填，不为记录自身 SHA 循环替换已验证 head。

首轮本机默认并行全 workspace 出现两个旧短截止 fixture 失败：MCP `deadline_is_finite_and_not_retried` 的 initialize 到达 1 秒截止，Brokerrouter tracked_headers 的 POST 到达 1 秒截止。当时并发运行 outbound 等其他用例；后续独立默认并行再次只复现 MCP 的准备阶段超时。仅修正 MCP 测试：真实 connect/catalog、descriptor pin 与 schema 编译仍受默认 5 秒预算约束，测试实际 RemoteTool 的调用仍为 1 秒、elapsed<1800 ms、明确 remote outcome unknown 和实际 socket 请求仅一次。模块默认并行 11/11、独立连续 20/20 及最终完整默认并行 1042 项均通过。生产/SDK 超时和 CI 并行方式未改变；首次 tracked_headers 超时的具体调度原因未确定，未借该测试修订宣称其根因已解决。

新 process fixture 首轮的 Unicode 数据为 25.9 KiB，超过既定 16 KiB 整条上限；程序正确保持 needs_review、无 outbox，未发送分片。测试数据改为小于 16 KiB 的 6/7 片边界，准确覆盖最多 original+五条 followup；这不是生产 split 修复，最终九组全部通过。

首轮 [CI 37743578880](https://github.com/StateKnot/JiaClaw/actions/runs/37743578880) 对应 head `765965c69352dfe1d948bf7450e4e044cb91770d`。Ubuntu job `113199709450` 全部成功（1043 项默认并行 Rust，含 Linux 专属 EXDEV；九组 Discord、全部旧进程、Chromium 与独立真实 Docker 沙箱），container job `113199709161` 成功（限额卷 App 预留/撤销/重启、停机维护及真实 ENOSPC）。macOS job `113199709457` 的默认并行 Rust 及九组 Discord 等前序测试均成功，在钉钉阶段因整作业的 20 分钟上限被取消，installer 未执行；GitHub 注释明确为 `The job has exceeded the maximum execution time of 20m0s`，不是已识别的测试断言失败。编译与新增强制验收使原整作业预算不足，测试作业改为有界 30 分钟；保留默认并行、每个 fixture/生产期限、完整测试清单及 container 的 20 分钟预算。该修订只改变 CI 资源预算和记录，生产二进制及本地哈希不变；最终新 head 仍须重新完成三项 CI，不借首轮局部成功宣称全部通过。

2026-10-08 08:23 UTC 已回填终验：[PR #84](https://github.com/StateKnot/JiaClaw/pull/84) 最终 head `768da0eeff2219ec69d3ca36cd12fbcd8943e927` 的 [CI 37746206382](https://github.com/StateKnot/JiaClaw/actions/runs/37746206382) 完整成功；macOS job `113208183475`、Ubuntu job `113208183608`、container job `113208183208` 均 completed/success。包括默认并行 Rust、全部强制进程 fixture、Linux Chromium、真实 Docker 沙箱与限额卷验收；Mac 钉钉及 installer 也完成。容器仍未开启 Discord runtime，此成功不认证真实平台或全部共享磁盘压力。

剩余：真实 Discord 安装/反代延迟/终端收发与供应商联合认证；其他独立用户渠道；StateKnot 固定版本 durable admission/driver/store、stdio、子 Agent，及逐 token 输出/媒体身份接线。应用 hold、加密 inbox/outbox 和元数据收据都没有赋予工具重放或自动恢复能力。

## 独立用户飞书批次

2026-10-08 08:23 UTC 官方 GitHub API 核对：附着 PR #58–#84 均 OPEN draft，各自当前 head 检查 SUCCESS，无 review/thread；本批基于上述 PR #84 最终已验证提交。StateKnot `9110ad7`、release alpha.1、#140，以及 Brokerrouter `e01ecb9`、无 release、#31/#41/PR #40 均未变化；再次核对 PR #40 Rust annotation，仍是付款/额度使作业未开始，没有据此报告代码失败或重复 issue。

本批接入默认关闭的[独立用户飞书私聊](tenant-feishu.md)，registry schema 6 和后端 protocol 4 固定专用 App/tenant/Bot/人/p2p Chat；私有库 owner、录入/执行/发送与 shared hold、UUIDv7 原请求收据及撤销后无 Secret 的停机核对均采用明确边界。900 ms 本地回调总预算覆盖 URL challenge，普通事件仅持久接纳后 ACK。业务文本为明文，不把 Encrypt Key 入站加密外推为落盘加密。

合同证据直接读取飞书官方 `.md` 文档及固定官方 Go SDK `99927aa13e271ea9fe03591204aad7bc6a2d869c`：Bot GET 顶层 `bot`、activate_status=2、无额外 scope；Tenant GET 的 `data.tenant.tenant_key`；Chat GET 的 `data.chat_mode=p2p` 与单聊缺省群字段；`im.message.receive_v1` 的固定 human/chat 关联及 message_id 去重；原始 SHA-256 签名、AES-CBC/IV、challenge 1 秒和普通事件 3 秒平台预算。最小权限仅私聊读取、机器人发送、Chat 与企业信息读取，不用通讯录/成员列表证明。官方字段与完整部署步骤见该指南的对应链接。

本批已完成的本地源码证据：

- Rust 1.88.0 的完整默认并行 `CARGO_INCREMENTAL=0 cargo test --workspace --locked`：1097 项通过（library 373 / core 123 / host 601），既有真实 Docker 一项本机 ignored。未以串行结果代替默认并行验收。
- 独立 review 修正七处 parser/sender 边界和三处 backend/store 边界：关键 JSON 对象保留原字节并拒绝重复字段，平台身份/回执按真实响应解包；final SQLite 在只读打开前验证完整原始文件头，避免验证异库时创建 WAL sidecar；完整 owner 的不可替换/修改/删除 trigger schema 必须吻合，version 0 不收养外来 view 或其他 schema；私有入口和后端统一 16 KiB prompt。针对真实 SQLite 和协议对象的回归已包括在上述全量测试中。
- 已准入的模型 completion、投递 settle 和 registry hold 结算等待同一有界 I/O 容量，取得许可最多 5 秒；阻塞任务持有许可至实际结束。暂时忙不会直接丢弃结算，失败仍保守保留人工核对状态，不把网关/后端两次提交当作跨库事务。
- 首轮默认并行的旧 Brokerrouter tracked_headers 用例在外层 1 秒等待到期。仅把该测试的 fixture 等待改为有界 5 秒，保留 body barrier、durable 请求身份和实际单请求断言；锁定 artifact 独立连续 20/20 通过（总 13.14 秒），最终上述默认并行全量也通过。生产/SDK deadline 未改，具体调度原因未确定，不据此声称已修复生产问题。
- fmt、locked 全目标 check（10.82 秒）、build（20.22 秒）和全目标 Clippy 必需 correctness/suspicious 检查（52.04 秒）通过，仍有 style/pedantic warnings。最终二进制 SHA256 为 `43f7ec3bdacb6d2258eef37e77c75377822493de5e134283712b8149ada93db6`；新 `tenant_feishu.py` 11/11 组、21 套既有进程回归最终全部通过，每套及分批整体前后哈希一致，没有在验收中重建或使用旧二进制代替。
- 新整机使用两个真实 JiaClaw 后端、原生工具往返、真实 SQLite 和 OpenSSL 独立加密/原始签名。覆盖真实 Bot/tenant/p2p Chat 响应、challenge/正文期限、跨用户拒绝与异 owner、不同 event_id 的 message_id 去重、只读 HTTP Key 独立授权、用户 disable、unknown 片阻断、五次限流同 UUID/文本及持久冷却、模型/POST 中 SIGKILL、已知 token 失效、精确 chat 回执、不可逆 revoke、1000 条实际准入及第 1001 条拒绝、撤销/移除 runtime 后队列和停机 hold 核对。仅 localhost 合成凭据，不认证真实平台。
- 21 套回归为 user_gateway、read_only_keys、gateway_audit、tenant_cron、tenant_telegram、tenant_slack、mcp、native_tools、e2e、channels、scheduled_delivery、memory_io、scheduler、model_routing、semantic_memory、model_calls、feishu、wecom、dingtalk、discord_scheduled、tenant_discord；初批九套成功，后续十一套成功，修订后的 channels 正式一次运行成功（28.13 秒）。

既有 channels 首轮在 1 秒 Agent deadline 用例的“模型 HTTP 请求数必为 2”断言失败，当时没有记录实际次数。保留旧断言与同一 1 秒期限的诊断运行观察到 2 请求、0 outbox、0 send 并通过，不能据此声称已复现或确定首轮调度原因。仅修订测试的两处断言：deadline 到达时实际请求数可以为 0–2，记录该次数并验证重启不增加，仍要求 needs_review、零 outbox/send；既有 processing_crash 用例另行明确在第二个 HTTP 已提交后核对未知结果。生产实现、期限和 CI 并行方式未改，最终修订 fixture 上述正式运行通过。

token 验收分层保留：通过的进程 fixture 使用正常 `expire=7200` mint，覆盖已知 `99991663` 失效后的 terminal/no resend、离线核对，以及启动 token HTTP 的真实取消/截止。Sender 单元测试另行覆盖 cache 失效、退避、取消及未来独立投递，不用成功 mint 的 61 秒寿命、修改 clock 或人工改 hold 来构造运行期刷新。官方自建 token 接口在剩余有效期不足 30 分钟时签发新 token，运行期正常到期刷新仍未取得真实平台整机认证。[官方 token 生命周期](https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal)

首轮 [PR #85](https://github.com/StateKnot/JiaClaw/pull/85) 的 [CI 37759803423](https://github.com/StateKnot/JiaClaw/actions/runs/37759803423) 对应 exact head `ff4737a531f0a8ca70b3e903513e39b75164ddd8`，已 completed/failure。Ubuntu job `113253178887` 全部成功，包含新增飞书 11 组、全部强制进程回归、Chromium 与真实 Docker；container job `113253178575` 成功，覆盖 schema 6 永久 App 预留、撤销/重启、默认关闭及真实 ENOSPC，未据此认证渠道 runtime。

macOS job `113253179014` 的飞书前 10 组通过，第 11 组千条容量准入循环在 `tenant_feishu.py:1078` 预期 HTTP 200、实际收到 429 后失败，后续 channels 等步骤 skipped。首版 helper 的断言没有输出 response `body.status`，当前不能区分 transient busy 与 queue_full，更不能据此确定调度、队列或生产根因。首轮 Linux/容器成功不能代替修订后最终 head 的三项完整验证。

首轮后的第一次修订仅限 fixture：helper 输出白名单内的 `body.status`、HTTP 状态及 elapsed，不输出正文或 Secrets。测试当时要求真实 `BEGIN IMMEDIATE` 写锁期间只返回 `503 admission_failed`，断言队列/hold/model/send 未变；保持锁到网关停止并排空后再释放，重启后再次计数，避免把 HTTP 截止当作阻塞 SQL 已结束的证明。

千条准入每条复用同一原始正文/签名 header，仅明确的 `429 busy` 可在 2.5 秒内至多尝试八次，全组至多 32 次 busy；逐次检查精确队列计数和唯一消息身份、hold/model/send 不变，queue_full、503、未知状态立即失败。用真实未读完的 challenge 占用 body-reader 槽、收到 busy 后有界释放，明确执行新重试分支；最终 1000 条事件必须等于原 ID 集合与新增 ID 集合的精确并集，第 1001 条必须 `429 queue_full`。生产实现、每次 1 秒断言、900 ms 回调预算、SQL 250 ms 和各项限额均未改。这增加确定的忙碌合同验收，不解释首次未记录状态的 429；修订后的完整 11 组一次运行退出码 0、全部通过，前后仍为同一生产二进制 SHA256 `43f7ec3bdacb6d2258eef37e77c75377822493de5e134283712b8149ada93db6`。实际 reader-slot 分支记录一次 `busy`，队列为 12、该新消息匹配数为 0、hold/model/send 未变；释放 challenge 后同 raw/header 接纳一次，988 个新增身份加原 12 个精确达到 1000，第 1001 条明确 queue_full。Python 语法和 diff-check 通过，没有重建生产二进制。

第二轮 [CI 37763844564](https://github.com/StateKnot/JiaClaw/actions/runs/37763844564) 对应 exact head `890f47d53756faf5c3443ea4dd35812b57e136c6`。Ubuntu job `113266536157` 全部成功，含修订后的飞书 11 组、全部强制后续、Chromium 和真实 Docker；container job `113266535783` 成功。macOS job `113266536142` 的前 10 飞书组通过，第 11 组写锁用例预期 admission_failed，静态诊断实际为 HTTP 503、`ingress_deadline`、elapsed 908 ms，随后失败并跳过后续步骤。这确认到达整体 900 ms 回调截止；单次 SQLite 250 ms busy timeout 不能当作整体返回时限。具体排队、mutex 或 CPU 阶段未确定，更不证明首轮未知 429 的原因。

当前最小修订仍仅在 fixture：写锁用例只允许精确 HTTP 503 的 admission_failed 或 ingress_deadline，其他 503 立即失败；保留写锁直到网关进程确认退出并排空，再核对完整 hold、事件计数、消息 4001 不存在、model/send 全不变，才 rollback 释放锁，重启后重复核对。静态 `SQL_BUSY` 诊断只输出白名单 code、drained 和 unchanged。验收最终持久状态，不把 HTTP 回调截止当作在途 SQL 已取消的证明；所有 callback 1 秒断言、900 ms、SQL 250 ms、1000/1001、busy 重试八次/2.5 秒/全组 32 次及真实 reader-slot 分支保留，生产没有变更。本次完整 11 组一次运行退出码 0、全部通过，前后二进制仍为上述 SHA256。本机 SQL_BUSY 明确 admission_failed/process_exited，完整原队列、hold/model/send 在持锁退出和重启后不变；实际 reader-slot busy 一次，仍精确 12+988=1000、第 1001 条 queue_full。未强制本机产生 deadline，不把本机成功外推为已确定 macOS 耗时阶段。

2026-10-08 11:35 UTC 回填最终证据：[PR #85](https://github.com/StateKnot/JiaClaw/pull/85) 当前固定 head `80a8bb03d391cd27bde8f03034aa2c5081d441f4` 的 [CI 37767529821](https://github.com/StateKnot/JiaClaw/actions/runs/37767529821) 已 completed/success，三个作业全部成功，没有把前两轮局部成功作为最终验收。

| 最终作业 | 实际执行证据 |
|---|---|
| [macOS 113278716859](https://github.com/StateKnot/JiaClaw/actions/runs/37767529821/job/113278716859) | 默认并行 Rust 1097 项（373/123/601）、新增飞书 11 组与所有该平台强制后续步骤通过；SQL_BUSY 实际 ingress_deadline，持锁停机确认 process_exited、队列仍 12、完整效果不变，再释放/重启核对 |
| [Ubuntu 113278717605](https://github.com/StateKnot/JiaClaw/actions/runs/37767529821/job/113278717605) | 默认并行 Rust 1098 项（374/123/601，含 Linux 专项）、飞书 11 组及全部强制进程回归、Chromium、真实 Docker 通过；SQL_BUSY 实际 admission_failed，同样确认进程终止及持久效果不变 |
| [container 113278717131](https://github.com/StateKnot/JiaClaw/actions/runs/37767529821/job/113278717131) | 非 root/只读根镜像与限额卷/私网网关通过，含 schema 6 永久 App 预留、撤销/重启、默认关闭和真实 ENOSPC；未配置飞书渠道 runtime，不能据此认证容器内收发或共享卷压力 |

上述两种精确 503 的实际跨平台结果支持 fixture 的最终持久态合同，没有确定首轮未记录状态的 429 原因，也没有定位 macOS 整体截止的具体调度阶段。真实飞书自建安装/可用范围、事件字段、公开 TLS 总延迟、终端收发、正常运行期 token 到期刷新、平台限额、共享卷及容器渠道 runtime 压力、供应商联合认证仍需独立完成。

## 企业微信启动校验批次

2026-10-08 11:35 UTC 官方 GitHub API 核对：附着 PR #58–#85 均 OPEN draft，各自当前 head 检查 SUCCESS，无 review/thread；本批基于 PR #85 上述最终已验证提交。StateKnot main 已推进至 `a312b0c2d09cd6d695b37b8d4163cddb910bdf6a`，十二项检查成功，#148 typed Schema 方向修复已合并但未发布；当前精确 alpha.1 HTTP MCP 直接使用远端原始 Schema，不调用 typed registry，因此无需为该修复升级。release/#140 不变，Brokerrouter main/#31/#41/PR #40 不变；main 六项与 PR 六项 FAILURE 的全部 annotations 均明确付款/额度导致作业未启动，无已执行测试失败证据。固定源码与能力边界见 [StateKnot](stateknot-gaps.md) 和 [Brokerrouter](brokerrouter-gaps.md)，没有新增或重复 issue。

本批交付 standalone 企业微信的实际 `serve` 前置校验：本地渠道/API/gateway 授权完成后，在 Agent/MCP、会话数据库、HTTP 监听和所有 worker 前核对专用凭据、固定且启用的 AgentID 与人员可见范围。入站、会话和定时授权成员去重并集最多 300 个；不根据部门/标签推断成员，平台人员响应归一并拒绝重复。启动整体 30 秒、token 含锁等待与应用查询各 5 秒，64 KiB 原始 JSON/关键字段/MIME 合同也用于既有 token 和发送响应；凭据查询不会发送消息、自动重发或记录秘密。

本批最终本地证据：

- 完整默认并行 `CARGO_INCREMENTAL=0 cargo test --workspace --locked` 1105 项通过（library 373 / core 123 / host 609），既有真实 Docker 一项本机 ignored。新增八组真实 HTTP 单元测试包括正确应用/全部成员、原始身份对象/MIME/重复字段、并发 token singleflight、取消退避及前置请求期限；fmt、Clippy 必需 correctness/suspicious 检查与 locked build 通过。
- 最终二进制 SHA256 `a6a42a66d6439ba4fb2a29dcb93f93f1204330ad28a686504a8276ff5ad5a5fc` 的 `wecom_startup.py` 正式脚本六组/32 负例一次全部通过（26.52 秒）。成功用真实 GET token→应用与含 scheduled-only 成员的完整并集；未配置企业微信没有平台/模型请求。负例覆盖原始 JSON/MIME/HTTP/身份/可见范围/大小，并验证失败前无监听、SQLite、MEMORY、MCP/模型或发送效果，以及已有真实数据库/工作区不变。
- 真实阻塞 token/app 查询的 SIGTERM 进程退出通过，没有启动监听或后台效果。token 头等待和 agent 部分正文分别观察到请求至失败退出 5.016 / 5.021 秒，从进程启动至退出为 5.261 / 5.216 秒；均未由 fixture 释放请求，且满足整体 30 秒。该 wall-clock 观察不等于实时系统零调度延迟保证。
- 同一二进制旧 `wecom.py` 一次退出码 0、64.89 秒，包含既有 OpenSSL/XML、加密回调、成员大小写/去重、原生工具、token 复用、分片、独立相同消息、发送间隔、unknown/SIGKILL 恢复和精确定时通知。
- 新增停发维护路径使用原 SQLite/原管理 API Key、无 WeCom 凭据、`http.channels=[]`、scheduler/HEARTBEAT 关闭。HTTP 两次完整 unknown 审计相同；一秒观察无 token/agent/model/send，正常 SIGTERM 后确认实际进程退出，再核对全部额度账本及未知请求的空完成时间占额不变。恢复原配置重新执行 token/agent GET 校验，仍不重放原 unknown。该证据不确认原平台请求终止，也不授权自动清理未知状态。[维护边界](wecom.md#恢复与定时通知)
- 同一冻结二进制五套既有回归全部一次通过，最终 runner 与每套退出码均为 0：WeCom 64.89 秒、MCP 2.47 秒、e2e 0.46 秒、channels 33.98 秒、scheduled_delivery 37.92 秒。每套前后二进制哈希一致，不沿用上批结果或在验收中重建 artifact。

两次初始失败及测试修订保留：新 startup fixture 在已停止的真实 WAL 数据库、无 WAL/SHM sidecar 情况下用 Python `mode=ro` 重新打开失败。停机后同一文件 URI 的 A/B 观察中，原 mode=ro 与规范 as_uri 的 mode=ro 均失败，而非创建的 mode=rw 配合 query_only 返回 integrity_check=ok、完整 14 张表，数据库文件哈希不变。修订仅针对 fixture 的 VFS 观察路径：正式脚本只在实际进程退出后用非创建 mode=rw/query_only 读取并关闭；移除一次性 A/B 探针后六组正式脚本仍通过。生产 bundled SQLite 3.51.3 没有变更，没有把观察失败称为生产数据库修复。

旧 WeCom 新维护 helper 初次错误地要求 scheduler-disabled DTO 含 `state`，实际既有合同在 scheduler.enabled=false 时返回精确 HTTP 404，导致 KeyError。仅修正测试断言为 404，正式完整旧 WeCom 一次通过；没有新增 DTO、改变服务语义或延长生产/fixture 超时。

原启动门槛提交准备时跨平台 CI pending，未猜 PR 编号或以飞书 CI 替代；下方已回填 PR #86 的最终固定提交结果，避免为记录自身 SHA 循环提交。校验失败拒绝整个服务，坏 Secret 不会开放 HTTP 诊断；上述维护配置与请求终止/人工核对是不同层次。

本批不改变未知发送人工核对、200 个预留/24 小时/4 秒额度或独立重复消息语义；不增加独立用户企业微信 registry/后端 owner、私有队列与后台准入，不计为完整 tenant WeCom。真实生产 API、企业认证、动态可见范围/成员许可、可信出口 IP、公开 TLS/客户端收发、运行期 token 到期刷新、容器渠道 runtime 压力及供应商认证仍未完成。

2026-10-08 12:56 UTC 回填：[PR #86](https://github.com/StateKnot/JiaClaw/pull/86) 固定 head `db802c46b8726e2dfbaf9defb1eebddb043619c7` 的 [CI 37776140027](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027) completed/success，无 review/thread。

| 作业 | 最终固定提交结果 |
|---|---|
| [macOS 113307363149](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027/job/113307363149) | SUCCESS；新增 wecom_startup 与既有 wecom 及该平台全部强制步骤通过 |
| [Ubuntu 113307363413](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027/job/113307363413) | SUCCESS；新启动/旧渠道、全部强制回归、Chromium 与真实 Docker 步骤通过 |
| [container 113307363064](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027/job/113307363064) | SUCCESS；原镜像/私网限额卷范围通过，不含本批之后的独立用户 WeCom runtime |

## 独立用户企业微信批次

本批基于 PR #86 上述固定 head。12:56 UTC 官方 GitHub API 核对 PR #58–#86：均 OPEN draft，各自当前 head 检查 SUCCESS，无 review/thread。StateKnot main `04567c4db12553025b4d31330693f4958222c30f` 十二项成功，#152 已合并七种空 tagged execution wire 严格读取；合法 wire/schema pins 不变、未发布，当前精确 alpha.1 HTTP MCP 不消费这些读取器，不需要升级。release/#140 未变化。Brokerrouter main/#31/#41/PR #40 未变化，十二项 FAILURE 的全部 annotations 均为付款/额度导致未启动，无执行代码失败或新 durable 输出合同；没有新/重复 issue。

实际接线为 registry schema7 的 CorpID/AgentID/精确成员/用户终身绑定、protocol5 后端永久 owner、每绑定私有队列、XML/AES 原始回调持久准入、共享用户授权/hold/容量与原UUID账本。启动先打开/迁移普通 registry，官方安装证明通过后才打开 WeCom 私库/握手；不承诺启动前所有本地文件零写入。发送原请求与NULL额度保持未知结果、不自动重放，停机管理在缺失 runtime/Secret 或永久撤销后仍核对保留 owner/额度。

新 `tests/tenant_wecom.py` 使用同企业两个不同专用应用、真实双后端、原生 Brokerrouter native tools、本地官方形状 HTTP 和独立 OpenSSL，最终冻结artifact十组已全部通过；第一组实际验证partial-body超时/安装间槽隔离及原始后端DTO数组/重复字段/unknown负例、同一合法已绑定owner的单一MIME正例与JSON+text/JSON+JSON重复头负例。永久16000操作 headroom 为每消息预留一次模型及最多16次单尝试发送，空新库最多941条未执行消息；整机根据停止后的真实既有 metadata 计算剩余名额，不改时钟/hold/额度或SQL伪造满额。写锁用例只接受503 admission_failed/ingress_deadline，保持真实锁到网关实际退出排空，再核对完整状态、零新增效果及重启后原身份不变。

首轮通过安装组并完成真实 native basic 回复后，测试错误地读取 events 顶层 event_id，实际 DTO 位于 spec.event_id，导致 KeyError；仅修四处测试读取。第二轮通过前六组，在 actual POST 后 SIGKILL 用例错误要求 wecom-inspect 之后仍为 submitting；管理打开会恢复为 unknown。已改为**任何 inspect 前**先用非创建、WAL-aware、query_only 且立即关闭的停止进程数据库快照证明真实 submitting/in_flight/NULL额度，再核对维护转换 unknown/needs_review 且原UUID/额度不变。独立 review 同时修正未执行到的 bindings DTO 为顶层 `bindings` 数组。这些是观察/DTO 断言错误，不是生产语义或期限变更；被沙箱拒绝的 loopback 初始化没有执行任何验收组。

源码审查另发现实际永久操作 headroom 不足可能永久停滞，以及异 owner 的WAL打开可能在拒绝前生成sidecar；最终生产事务预留及immutable主文件 owner 前置读取已按完整源码测试通过；immutable只用于已固定主文件的application/schema/owner证明，不读取队列/额度或执行恢复，正式queue/quota/recovery仍使用完整WAL。测试停止进程观察也保持WAL-aware，不用immutable掩盖真实提交；私有后端共用body读取最初仅检查首个Content-Type，已收紧唯一JSON MIME，并用合法已绑定owner正例与重复头负例通过验证，不能以无效DTO掩盖头检查。新增 parser 测试编译初次把无serde feature的Uuid交给json!，已仅改测试为规范字符串，不改变依赖或生产。修订后 preliminary 二进制十组完整通过：按真实历史 metadata 计算准入940条新身份，下一条精确queue_full，满额原MsgId仍ACK。该artifact在最后owner/receipt补强前，不作为最终源码认证。首份默认并行Rust快照1153项（373/123/657）通过；追加owner/receipt测试后的第三份默认并行快照1157项（373/123/661）全部通过。中间第二份全量在旧Slack测试出现一次EAGAIN，随后目标单独执行及第三份完整默认并行执行通过；具体原因未确定，不把重跑成功写成生产缺陷已修复，没有修改生产/fixture期限。以上属于中间快照，最终整批数字如下，不以旧快照代替MIME等最后源码验收。

最终源码完成完整Rust/格式/静态检查/构建，整机使用同一冻结生产artifact，未延长生产/fixture期限：

| 验证 | 最终结果/范围 |
|---|---|
| 默认并行完整workspace Rust | 1158项（library373/core123/host662）全部通过，157.95秒；既有真实Docker一项本机ignored，由Linux CI另验 |
| fmt / required Clippy / locked build | 全部通过，分别1.06 / 21.86 / 24.59秒；Clippy必需correctness/suspicious通过，不将既有style warnings说成零告警 |
| 冻结生产二进制 | SHA256 `ac8da42d9c066b7ea9212c1e32733cde4c9459d7828a5acec444b1e6fcf5f58d`，fixture前后保持不变 |
| 新tenant_wecom整机 | 十组一次全部通过，exit0、143.05秒；fixture SHA256 `341726b8fa524d7d053aef6399cc1a6ace4506b2844ba8e845252dac1698430b`；日志SHA256 `85c2ca33cc056eed213153a73311a5fc656b89a7444e55b45855181abf11da2e` |
| 既有进程回归 / 最终CI | 提交准备时21套回归仍独立运行，尚不声称全部通过；本批draft PR保存最终固定head的完整结果，下一批回填 |

新十组的最终源码证据包含真实模型POST/平台POST后的SIGKILL原UUID核对、维护前submitting/in_flight/NULL快照及恢复后unknown/needs_review、物理移除全部安装凭据的离线核对、purge后metadata/额度相同、user-disable和不可逆撤销、完整Unicode渲染与4秒间隔，以及真实写锁保持到网关退出后核对零新增效果。最终库历史仅剩940条可预留新身份，全部实际加密准入、下一条精确queue_full、原MsgId重复仍200，未编辑时钟/额度/hold或假造满额。

真实生产安装/许可/动态可见范围、公开TLS与终端收发、正常token到期刷新、接收额度及共享卷/容器runtime压力、付费供应商联合认证仍未完成。公开指南见[独立用户企业微信](tenant-wecom.md)；本提交准备时最终CI pending，以本批draft PR固定head为准，后续批次回填，不用PR #86旧范围代替此批交付。

## 独立用户企业微信最终 CI 回填

2026-10-08 14:39 UTC 当前官方 PR/CI 核对：PR #87 OPEN/draft，head `ba989c30a75e4e6fe7eaf9e13c0704a90cc7695e`、base `db802c46b8726e2dfbaf9defb1eebddb043619c7`。[CI 37787484242](https://github.com/StateKnot/JiaClaw/actions/runs/37787484242) 第一次运行三项完成成功。CI实际checkout merge `b35104ed9bbde064b7bc85daed964ef75e3e6eb1` 的 tree `6a349f6f629fc15cd9c9ffd45d26a78466646e81` 与冻结本地源码完全一致，parents 精确为上述 head/base。本地工作区 clean、15个生产/fixture source SHA 和生产二进制前后未变。

同一二进制旧13套 standalone、八套 gateway/tenant实际进程全部一次通过，分别374.488/428.480秒；每套完整日志SHA与binary前后SHA复核，不改deadline、不重试、不在验收中构建。

| 最终作业 | 完整日志核对的实际结果 |
|---|---|
| [macOS 113345874473](https://github.com/StateKnot/JiaClaw/actions/runs/37787484242/job/113345874473) | library373/core123/host662，共1158；29项强制Python步骤（旧28+新1）成功；WeCom十组完整、实际940身份后精确queue_full、满额重复ACK保留。浏览器与真实Docker为Linux专项，明确skipped。 |
| [Ubuntu 113345874820](https://github.com/StateKnot/JiaClaw/actions/runs/37787484242/job/113345874820) | library374/core123/host662，共1159；同样29项/WeCom十组/940全部通过；四项真实Chromium PASS，另单独执行既有ignored Docker隔离/超时/输出/清理测试 PASS（10.44秒）。 |
| [container 113345874791](https://github.com/StateKnot/JiaClaw/actions/runs/37787484242/job/113345874791) | 非root/只读rootfs、私网/限额卷；schema7 Corp/Agent永久身份与撤销重启、默认关闭、在线维护锁拒绝、停机无Secret四种空inspect实际通过。真实ENOSPC写57,028,608字节、文件系统58,675,200字节、df可用0，其它租户仍可用。未启用WeCom平台runtime，不能外推容器收发压力。 |

最终三份完整日志SHA256（macOS/Ubuntu/container）：`df3d74314097c91e69a48a64b270b79d962623b59ba8b5014b0e7c0d2964d44c` / `412287fd6187e0df674c4fc4bff265984ba0533e3e4d51f8ba79184f9af28657` / `d764538f5733331528aad4fad356d16d9cac85de7d00b5f42588997ca1016d5d`。旧Slack staging lock测试两平台实际ok，先前本地EAGAIN原因仍未知，不把后续通过称为已定位或修复。

独立agent源码审查已完成；GitHub无人工/代码review或inline findings，唯一CodeRabbit评论说明draft不自动审，是信息提示，外部CodeRabbit审查未执行。保留draft、未合并/公开Release；真实安装、动态范围/许可、公开TLS/客户端、正常token到期刷新、容器启用WeCom runtime与供应商认证仍开放。

## 钉钉原始报文合同批次

2026-10-08 14:39 UTC 官方预检：PR #58–#87 均 OPEN/draft、各自当前head CI成功、无实质review/thread；CodeRabbit仅draftskip信息。StateKnot main `83802cb3202bf9cb860c6357a94abc80408b1f88`与13:51快照相同，release alpha.1/#140未变；最新HTTP MCP源与实际alpha.1 Cargo缓存字节相同，digest见[上游状态](stateknot-gaps.md)。Brokerrouter main/#31/#41/PR#40未变化，十二条失败annotation均付款/额度未启动；没有新的可消费durable原生输出或stdio合同，不提交重复issue。

本批修复现有standalone钉钉原始JSON/MIME与回执合同：标准Serde struct visitor可以接收完整位置数组，现要求root/text及token/send为原始对象、拒绝critical duplicates和唯一JSON MIME。关键重复包括null→值和Unicode转义同名，附加平台metadata/可选null保持兼容。code与任何字符串processQueryKey（包括空串）矛盾时unknown；原有三类失败名单和精确已知HTTP/code拒绝白名单保留。平台MAC仍只覆盖timestamp/ClientSecret，严格对象不替代受控HTTPS正文完整性；未新增token-only的安装证明或独立用户渠道。

最终本地默认并行`CARGO_INCREMENTAL=0 cargo test --workspace --locked`1163项（library373/core123/host667）全通过，原真实Docker一项本机ignored。fmt、必需correctness/suspicious Clippy与locked build全通过，既有style warnings保留。新真实HTTP unit三组含12个token和17个receipt负例，合法唯一MIME/metadata/缓存正例，以及callback原始root/text对象与Mime两组通过；token12秒、退避30秒、HTTP10秒与64KiB响应、单尝试/无跳转/无隐藏retry均未改。

正式整机使用冻结binary SHA256 `ea37d64a682f0599ce0148e43f366628a6967c77f057a8b10807963953a77635`；fixture SHA256 `30ba7fa10c436849d6f1ff63d9b02985cd4fb29574f2f3f7d2d4b5a064cf535e`。完整`tests/dingtalk.py`一次exit0、147.87秒，日志SHA256 `0f4b7ae2e527d3be4fd222bf59081990fb36e94857be150f9a4ceb7c2f27adda`，binary/三份source前后相同。

- 九个合法签名原始callback负例实际返回401，事件/model/token/send完整计数零增；有效单JSON charset正例走同一路径和获准真实成员。
- 六类token对象/重复字段/MIME/code矛盾负例各用真实新serve冷缓存、正常7200expiry；native tools完成后恰一次mint、消息POST零。原v4 delivery UUID/attempt1/permanent_failed、完成后至少4秒持久cooldown、重投无新执行、停机WAL观察及重启不重放均通过，没有改时钟或短expiry造结果。
- 七类send原始形状/重复键/双MIME/code矛盾（含空receipt）均实际抵达固定batchSend一次，保存原UUID/attempt1/unknown与空回执；同MsgID重复无SQL/model/platform增量，同成员后续事件等待人工核对。
- send_array在真实停止进程后用非创建mode=rw/query_only读取完整WAL，核对original unknown/cooldown，再物理移除DingTalk运行配置/Secret并关闭scheduler/HEARTBEAT；HTTP原对象与整份SQLite/cooldown前后相同，model/token/send零增；恢复原配置仍不重放，显式cancel才允许原待处理消息继续。无secret指钉钉平台凭据，不移除管理API或fixture provider鉴权。
- filteredStaffIdList三种合法absent/null/[]各真实送达一次；非空列表继续unknown。旧case-preserving成员、原生clock/json、独立相同消息、UTF16分片、4秒间隔、真实model/send POST后SIGKILL、精确定时成员与环境凭据路径全部保留并通过。

同一冻结binary的五套相关旧进程回归全一次通过，实际合计79.647秒：e2e0.465、MCP1.315、channels28.086、scheduled_delivery31.841、wecom_startup17.727（原六组/32负例）；每套前后binary/log/script SHA核对。临时`/tmp`runner最初Popen参数拼写在任何测试启动前失败，仅修runner并运行正式完整五套，不把该初始化失败计作已执行回归或生产修复。

两位独立agent分别审root callback/fixture和sender，未发现明确blocking finding；未把作者自审当成独立sender review。提交准备时本批固定head跨平台CI pending，最终结果以本批draft PR为准；当前30分钟完整test job和20分钟container预算、原生产/fixture期限不变。真实Docker/Chromium由Linux CI实际验证。本批不增加tenant安装/权限证明、正常到期refresh、真实私聊/平台额度、容器启用DingTalk runtime或供应商生产资格。[合同范围](dingtalk.md#本批协议修复与安装证明的区别)

## 钉钉原始报文最终 CI 回填

2026-10-08 15:36 UTC 官方复核：[PR #88](https://github.com/StateKnot/JiaClaw/pull/88) OPEN/draft，head `4f62ecf2ae471899aa5233e24796a71e57bd4a40`、base `ba989c30a75e4e6fe7eaf9e13c0704a90cc7695e`；[CI 37797041026](https://github.com/StateKnot/JiaClaw/actions/runs/37797041026) 首次运行三项 SUCCESS。实际 checkout merge `e3fa352454d2ecf839dfc89671c8d685b7f79924` 的 parents 精确为上述 head/base，tree `436af9a4488e76af595f33c026588d638cce9ac6` 与本地冻结源码一致；真实平台身份未因此认证。

| 最终作业 | 完整日志及官方步骤核对 |
|---|---|
| [macOS 113379096190](https://github.com/StateKnot/JiaClaw/actions/runs/37797041026/job/113379096190) | Rust 1163（373/123/667）、29项强制Python全部通过，新钉钉原始回调/冷缓存token/发送负例9/6/7及三类filtered正例完整；浏览器/真实Docker专项明确skipped。 |
| [Ubuntu 113379096092](https://github.com/StateKnot/JiaClaw/actions/runs/37797041026/job/113379096092) | Rust 1164（374/123/667）、同29项及完整钉钉组通过；四项真实Chromium PASS，单独既有ignored Docker隔离/超时/输出/清理测试1项PASS（7.79秒）。 |
| [container 113379095664](https://github.com/StateKnot/JiaClaw/actions/runs/37797041026/job/113379095664) | 非root、只读rootfs、私网/限额卷、schema7/default-off及维护实际通过；真实ENOSPC与其它租户继续可用。没有启用DingTalk平台runtime，不记成容器渠道压力认证。 |

三份完整日志SHA256（macOS/Ubuntu/container）为 `128e072051318bdc710fce5cdac4410e9ca288a3ab3bcf66443bb286de118b26` / `0d7e97a2e1ce415b2c7184234c55bb934eaf17ea7f88b7a49041ae04d2f59df8` / `6c0de1c5c78d7237c5347f31062851cad3fb74e1b95d3bb6412a228a8e33f501`。CLI watch观察器曾API i/o timeout退出，重新读取官方状态和完整日志确认全成功；没有重跑或把观察器网络错误记作CI代码失败。无人工/代码review、inline findings或review thread，唯一CodeRabbit评论仅表示draftskip；外部CodeRabbit审查未执行。保留draft，未合并或公开Release。

## 复制共享 IO 与写锁批次

2026-10-08 15:36 UTC 预检两框架最新main、release和相关议题：StateKnot `83802cb3202bf9cb860c6357a94abc80408b1f88`、Brokerrouter `e01ecb94919d992eb0b74b3db00d70742820b4cc` 未变化；当前raw HTTP MCP与alpha.1客户端字节相同，stdio/durable/native最终Schema合同仍分别跟踪原议题。PR #58–#88均OPEN/draft且当前head CI成功，只有CodeRabbit draftskip，无实质review。另取得正式CorpID-bound GetToken JSON/权限合同，但当前生产端点未替换、凭据→机器人完整安装证明仍缺正式read权限/适用性，见[钉钉边界](dingtalk.md#本批协议修复与安装证明的区别)。

改动前用PR88冻结二进制 `ea37d64a682f0599ce0148e43f366628a6967c77f057a8b10807963953a77635` 真实serve/native localhost重现：canonical/legacy正例均HTTP200及两轮模型；fixture所有的工作区外哨兵与工作区来源实际同dev/inode、nlink2，旧copy仍HTTP200并复制相同SHA字节；`from/destination`、`source/to`各被native Schema在第一轮后HTTP500拒绝且无目标。脱敏记录SHA256 `b29f27baaffb5c9f18039f81baba056b4b2e93de1efd6b481c7cc2de29d94f7c`，旧binary前后相同，临时文件真实清理。属于应用接线/Schema缺陷，不向两个框架重复报issue。

注册的copy/file_copy现实际走共享`run_blocking → copy_file`：与十二主工具/五compat及记忆共用八槽；持有工作区inode写锁，经单组件目录句柄与pre/post-open单链接普通文件检查，两端共用1024字节/64组件路径；64MiB仍独立流式字节合同，最多实际读取上限+1后拒绝源增长。随机0600目标卷暂存、文件sync、无覆盖hardlink或覆盖rename、明确清理与父目录sync。两个名称同Schema接受全部四种参数组合，冲突别名由运行期拒绝。没有复制正文到模型、跨卷事务、外部编辑器CAS或自动回滚/重放；取消后可能发布，实际worker继续持有许可/锁到结束。

六个新Rust测试实际覆盖：外部hardlink/FIFO及两端保留；创建可访问的1024/1025字节和64/65组件路径；metadata之后真实源增长、严格最多64MiB+1及目标保留；父目录真实替换后保留已打开能力和目标叶子替换拒绝；no-clobber真实竞争/部分暂存传输失败与清理；真实copy传输门上取消async调用、阻塞worker仍持有一槽/目录锁且下一mutation零效果，释放后实际发布及新显式copy成功。取消测试使用与生产同helper的隔离一槽，不将其记为真实serve八槽压力。新文件不保留源权限/时间戳；跨源/目标卷机制未单独实测认证。

| 初版提交前本地验证 | 实际结果 |
|---|---|
| copy定向 / 完整默认并行Rust | 定向10项通过；`CARGO_INCREMENTAL=0 cargo test --workspace --locked`1169项（379/123/667）全通过，本机既有Docker一项ignored。 |
| fmt / required Clippy / locked host build | 全通过，0.482/11.438/13.635秒；correctness/suspicious必需检查通过，保留既有style warnings。Cargo依赖/锁未升级。 |
| 最终生产二进制 | SHA256 `c25276180f625e8480da8e5b8836fbc62744a415f3f72b66d2bfd9bc50b0944d`，正式新/旧进程验收前后相同，没有测试中构建。 |
| `tests/workspace_copy.py`真实进程 | 七组一次全通过、exit0、9.095秒；fixture SHA256 `e316c4323d4ffc7e990fdc5fd3959cea0b4b388003145a306ff52a7d620c30ff`，日志SHA256 `677b1312451ceb764dc9fef79fb43ca4d7c9051cc50e76a99a863d07af1f207f`。 |
| 同一binary既有八套进程 | e2e/native_tools/workspace_files/workspace_mutations/file_search/filesystem_info/memory_io/MCP顺序全通过、exit0、总54.181秒，每套script/log/binary前后SHA独立核对。 |

七组真实serve/native证据包括两个名字全部四种Schema组合/准确JSON；非稀疏真实64MiB字节SHA及64MiB+1拒绝后目标inode/字节保留；内部/外部/悬空父目录及叶子链接、两端hardlink/FIFO/目录与路径越界零外部效果；实际1024/1025、64/65路径正负边界；对真实工作区目录flock保持期间copy/file_copy/write_file/file_write零效果、health响应，释放后仅新显式请求成功；disabled两名称HTTP/native均隐藏，伪造混合批次全预检失败/零工具效果，disabled/未知请求权限零模型调用。全部使用临时目录和localhost fixture，没有真实平台凭据/付费调用。

中间测试修正如实保留：Mac不提供使用的rustix mknodat API，FIFO构造改用既有mkfifo测试方式；绝对路径根前缀使1024边界setup触发Mac PATH_MAX，改为实际逐级目录句柄创建/读取，未减小路径边界或skip；首版名为copy.py阴影Python标准库，在host启动前0.071秒导入失败，重命名workspace_copy.py及CI引用，fixture内容SHA未变后正式七组通过。生产/fixture期限、整作业30/20分钟预算和所有既有fixture未改，未把测试setup问题记为生产bug。

独立非作者审查生产两文件和新fixture，无blocking finding；read-only review不当作执行测试。CI新增一个强制workspace_copy步骤，两平台现在各30项Python；提交准备时本批固定head CI待运行/核对，以本批draft PR最终完整日志为准，后续回填。真实Linux Docker/Chromium由CI单独验收，开发机/机制描述不代替实际挂载与资源部署资格。[使用/恢复合同](workspace-files.md#copy-原子字节复制合同)

## 复制批次首次 CI 与数据库所有权修订

[PR #89](https://github.com/StateKnot/JiaClaw/pull/89) 初版 head `292142118b85bf6166199a6c733ce58bd2c61a30` 的 [CI 37805809681](https://github.com/StateKnot/JiaClaw/actions/runs/37805809681) 在 macOS 既有 `gateway::telegram_store::tests::restart_preserves_claim_associations_unknown_fifo_and_atomic_purge_cascades` 失败：同步 drop/reopen 后，`telegram_store.rs:466` 收到 lifetime flock EWOULDBLOCK（errno35）。library379/core123和六项新copy测试均通过，host666通过/1失败；macOS后续Clippy/build/Python全部skipped，不能计为copy跨平台整机PASS。当前没有证明该次CI恰好与哪个子进程fork重叠，不能把后续通过写成已经证明了CI根因。

独立Mac实际fork/pipe gate证明：CLOEXEC文件描述仍由fork继承；只关闭父描述符、子进程尚活时，独立flock立即busy；显式解锁旧描述后，不等子进程退出即可取得新独立锁。另新增真实SessionStore retained-dup定向测试，在旧生产实现稳定失败，正是drop后即时重开busy。此证据确认close-only生命周期缺口，区别于对原CI具体重叠的推断。

修订引入不可克隆的 `DatabaseOwnership` guard：会话库和Telegram/Slack/Discord/飞书/企业微信私库在成功取得锁后立即guard，覆盖helper前的所有初始化错误；SessionStore字段按conn→ownership声明，正常先关闭SQLite再显式解锁，错误路径的局部conn/事务也先析构。没有在SessionStore自身Drop提前解锁，没有新增等待/retry、串行全局测试或放宽期限。析构释锁失败仍保留下一次打开的fail-closed行为；强杀不保证执行析构。两项定向回归实际验证活跃owner拒绝、提交会话重开、保留旧dup关闭不解除新owner、未来schema11打开失败释放旧锁且version不改；11项ownership定向全部通过。独立非作者复核六份生产文件与两新测试，没有可行动finding。

初版container作业113409655465实际SUCCESS：checkout merge `d8fbe554e721a860502a018af8c7af56e19be8ae` / tree `c86f9128bc8854b7b7e0212b50449333984ec10d` / 精确初版head与base4f62，完整日志90271字节，SHA256 `097541a14db80b2cd5bf83d98f186afc5735a6020946d829ed2a95e158675587`；实际新镜像、非root/只读rootfs、private network/限额卷、schema7/default-off/无平台凭据维护及真实ENOSPC全通过。两份container脚本没有执行workspace_copy，不能记为copy容器压力；该初版成功也不代替修订后最终head验收。修订后的最终证据如下，CI仍以本PR最终固定head为准。


初版Ubuntu作业113409655961亦已完成SUCCESS，完整日志SHA256 `f867bd4c58f4e4e34b1539615923d201c2cda3f078acb19bbe1dd0d1c823e22b`：Rust1170（380/123/667），30项Python及新copy七组全通过，四项真实Chromium及单独ignored Docker一项PASS。初版整个run仍因macOS失败而FAIL，不重跑相同提交；后续正常释锁修复需新head三作业重新认证。

最终修订源码默认并行完整Rust1171项（379/123/669）全部通过，62.919秒；fmt/必需Clippy/locked host build0.680/14.930/14.726秒全通过。真实新binary SHA256 `9b5af94818d9454ab8ac2d599246dc2521b32d3b0f11320a0ce5d06d919d0646` 冻结后重新执行copy七组PASS（9.969秒、原fixture内容SHA未变、日志SHA仍 `677b1312451ceb764dc9fef79fb43ca4d7c9051cc50e76a99a863d07af1f207f`），同binary既有文件/记忆/MCP八套全PASS（57.704秒），再执行用户网关/只读Key/审计/cron/Telegram/Slack/Discord/飞书/企业微信九套全部PASS（551.243秒）。全部17套原脚本/内部deadline不改、零重试，每套script/log/binary前后digest独立核对；既有Docker一项本机仍ignored，由最终Linux CI另验。没有把初版c252二进制或初版CI结果替代最终修订。

提交准备时修订后固定head完整CI待运行/核对，最终精确head/tree、三份完整日志与所有30项步骤以本draft PR最终说明为准，后续批次回填。16:27:50 UTC 官方再次复核两框架main/release/#140/#31/#41/PR40无新回复/合同，不重复issues。已逐文件核对现成网关SSE与PR40有界连接修复，可在既有provider/模型收据/native loop/HTTP接线；现JiaClaw仍是完成后分块，生产默认stream资格仍须固定资源修复和真实供应商验收。此只读研究不记成流式或durable完成。


## 复制批次跨连接验收屏障修订

数据库所有权修订 head `92a2d6d07b849bb9a7b5b61c669f399b9d59baa9` 的 [CI 37809741134](https://github.com/StateKnot/JiaClaw/actions/runs/37809741134) 仍因 macOS 整机失败而整体 FAIL，未重跑相同提交。实际 checkout merge `dc1ed1a1863d1f30301aa0027dc3d08746d902c2`、tree `f54e207f0d4c198058434e1998965e5ba559ce40`，Git API 核对 parents 为该 head 和 base4f62；以下结果属于此固定版本，不代替后续 fixture 修订。

| 修订作业 | 完整官方日志核对 |
|---|---|
| [macOS 113423181717](https://github.com/StateKnot/JiaClaw/actions/runs/37809741134/job/113423181717) | 19分26秒；Rust1171（379/123/669）、六copy新测、两ownership新测及原Telegram同步重开全部PASS，fmt/必需Clippy/build通过。30项Python为20成功、飞书1失败、其后9跳过；新copy七组通过，飞书前10组通过，第11组 `tenant_feishu.py:1190` 的占槽就绪断言失败。日志SHA256 `1811392aa2376c846ee09f3c9dd9e619f55cbd4d1fa11d84095d44c936f6446a`。 |
| [Ubuntu 113423181481](https://github.com/StateKnot/JiaClaw/actions/runs/37809741134/job/113423181481) | 25分11秒SUCCESS；Rust1172（380/123/669）、全部新单测/原Telegram重开、30强制Python/copy七组、四实际Chromium与单独实际Docker1项PASS（7.80秒）及清理通过。仅macOS专用OpenSSL步骤按条件skipped。日志SHA256 `e4370af77c51b82e79282f33ab4473d78c7d4f357935d6853e10987555648247`。 |
| [container 113423181801](https://github.com/StateKnot/JiaClaw/actions/runs/37809741134/job/113423181801) | 5分35秒SUCCESS；精确新镜像 `bbf0484e9dbe1a748afe05a741048a8d44b6b4a18bf0d97f527d6316abaaabec`，非root/只读rootfs、命名卷恢复、私网、schema7/default-off与维护、真实ENOSPC及其它租户可用全部实测。日志SHA256 `4881abbef08f459de11ec64fa8c4efb27131ae4c6f674ee7f4e07f73739174a0`；未执行copy专项容器压力或真实平台runtime。 |

飞书失败日志仅证明原partial请求与另一连接probe未在250ms就绪窗口内形成429占槽证据，不证明生产semaphore失效，也未记录足以判定具体网络/Nagle/调度原因的数据。之前的SQL_BUSY `ingress_deadline`、`process_exited=true` 指的是fixture主动停机并完成退出屏障的旧gateway；此前队列/hold/model/send保持不变，随后确实启动了新gateway，不能误读成在死进程上probe。

本次只修该验收脚本的真实同步方式：headers-only `Expect: 100-continue`，在原250ms总窗口内限8192字节读取精确100 interim，再要求另一完整challenge实际429 busy。固定Cargo.lock的Hyper1.11.1在body consumer首次poll后才允许Sender ready并发送100；飞书handler在该poll前已取得global及单binding许可，所以这条协议屏障确认实际占槽。100尚不证明callback认证完成。随后原queue_admission仍须观察真实busy、核对未入队和原hold/model/send零增，再补全第一条完整body并读取精确200释放许可；原签名、最多8次/2.5秒/总32次busy、1000精确集合/1001拒绝、SQL退出drain、撤销与离线保留断言都保留。生产650/900ms、fixture250ms及原整作业预算没有扩大。

同一冻结生产binary `9b5af94818d9454ab8ac2d599246dc2521b32d3b0f11320a0ce5d06d919d0646` 的修订后完整 `tests/tenant_feishu.py` 一次11组PASS、exit0、119.517秒；fixture SHA256 `8701f98030df3f09156facc870a88d5c6f8b7a746941579927ceacfabb5dff10`，日志SHA256 `be56ea9a671ba14c1946c0f5eb28183bd6a6260b59ef73e13f91d287936f4a09`。实际100屏障0.434ms、另一请求429、队列请求第一次busy时count12且matches0/hold/model/send不变，补全后released=true，随后准确满队列与全部原11组通过。fixture/binary前后hash相同，生产源码与依赖未再改或重建；先前17套是其当时原fixture的完整回归，本次另执行更新后的飞书全套，不伪称所有17套在该fixture修订后重新运行。独立非作者只读复核同步时序、预算与全部原断言，无阻断发现。

提交准备时最新fixture修订的三项固定head CI仍待运行/完整日志核对；最终head/tree/run及日志以本draft PR最后说明为准，下一批回填。Linux/macOS本地文件与泛容器部署证据不认证Windows、网络文件系统、copy容器压力或真实平台/供应商；stdio/外部写入、durable委派、生产流式与多模态仍保持开放。没有恢复自动回访、合并或公开发布。


## 复制批次完整 CI 作业容量修订

跨连接屏障修订 head `469ba6ae005eb9c303cc7059bc6234d92a2e6f14` 的 [CI 37814363011](https://github.com/StateKnot/JiaClaw/actions/runs/37814363011) 没有取得三作业整体 SUCCESS：Linux 和容器成功，macOS 整项作业在30分02秒被取消。官方步骤元数据显示全部四项Cargo、30项强制Python、新copy七组和更新后的飞书11组都通过，checkout后处理和Complete job亦成功；两个Linux专用浏览器/Docker步骤按条件跳过。官方failure annotation明确为 `The job has exceeded the maximum execution time of 30m0s`，原始annotation JSON SHA256 `5a1dac18985d23cf3b4fb37805f4231eadbbc6c46cdf88372be3111d856ce49d`。不能把所有测试步骤成功改写为这次完整作业成功，也不能把整体取消误报为框架或应用测试失败。

实际macOS冷构建及完整进程链占满原30分钟预算，末尾installer17:39:29 UTC、checkout清理17:39:30 UTC、Complete job17:39:32 UTC，而job从17:09:31 UTC开始；整任务取消于17:39:33 UTC。本次只把test矩阵的完整作业容量从30分钟改为有上限的40分钟，预留冷构建和runner结束开销。容器作业仍20分钟，四项Cargo、30项Python、Linux真实Chromium/Docker及清理、工具链/action固定版本、全部生产和fixture期限/资源/原断言不改。生产源码、依赖和二进制 `9b5af94818d9454ab8ac2d599246dc2521b32d3b0f11320a0ce5d06d919d0646` 未重建，飞书fixture仍 `8701f98030df3f09156facc870a88d5c6f8b7a746941579927ceacfabb5dff10`。

该固定head的Linux日志SHA256 `882684250ff9a83b58201383fea6480c5c7f675a7ff3515a1bf9e115785d0ad5`：Rust1172、30强制Python、copy七组、飞书11组（100屏障0.433ms/第二请求429/原队列count12及matches0/无效果/实际释放）、四Chromium和单独真实Docker全部通过；容器日志SHA256 `91e4b52354eb2dbc4fe1b9257959d150a7e84a1c7da3a8c1d8be23a36e576c35`，新镜像部署/所有权/卷恢复/限额压力通过，仍未认证copy容器专项或真实平台runtime。macOS完整日志SHA256 `9e1933ef9bb338313b5b0db910fa294cebb44470f9e565a22cfb157656d319aa`：Rust1171、全部新copy/ownership及旧Telegram重开、30强制Python与copy七组/飞书11组通过；实际100屏障0.77ms、第二请求429、busy第一次count12/event0/无效果并实际释放。此记录仍不改变整个macOS job cancelled。前两轮应用/fixture失败记录同样保留。预算修订需新提交重新执行三项完整CI，最终head/tree/run与完整日志以本draft PR最后说明为准，不使用旧head的成功替代。没有恢复自动回访、合并或公开发布。
