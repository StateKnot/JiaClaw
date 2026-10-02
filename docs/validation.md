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
