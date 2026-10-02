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
