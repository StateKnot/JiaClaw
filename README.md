# JiaClaw

JiaClaw 是用 Rust 实现的个人 Agent：CLI 对话、工作区文件和记忆工具、渠道 webhook、SQLite 会话历史，以及内置聊天工作台。模型请求推荐统一经过 [Brokerrouter](https://github.com/StateKnot/Brokerrouter)。[StateKnot](https://github.com/StateKnot/StateKnot) 的 HTTP MCP 已接入，持久化执行框架仍待接线与独立认证。

**当前边界**：SQLite 保存会话，不提供工具调用的持久化执行、断点恢复或 exactly-once 保证。StateKnot alpha 与 Brokerrouter 的真实供应商工具闭环仍有生产认证门槛，详见[上游状态](docs/roadmap.md)。不能把本仓库的单机测试通过解释为完整个人 Agent 栈已经生产认证。

## 从源码安装

需要 Rust 1.88.0、C 编译器与 Git。SQLite 随二进制编译，无须单独安装数据库。

```sh
git clone https://github.com/StateKnot/JiaClaw.git
cd JiaClaw
cargo build --release --locked -p jiaclaw-host
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/jiaclaw "$HOME/.local/bin/jiaclaw"
jiaclaw init
mkdir -p "$HOME/.jiaclaw"
cp config/jiaclaw.toml.example "$HOME/.jiaclaw/config.toml"
chmod 600 "$HOME/.jiaclaw/config.toml"
```

将 `~/.local/bin` 加入 PATH。修改配置的网关 URL、逻辑模型 ID，再通过 Secret 管理或终端环境注入 `JIACLAW_API_KEY`（网关虚拟 Key）与 `JIACLAW_API_TOKEN`（JiaClaw 自身 API Token）。两者用途不同。

```sh
jiaclaw doctor --config "$HOME/.jiaclaw/config.toml"
jiaclaw serve --config "$HOME/.jiaclaw/config.toml"
```

`doctor` 默认只读本地配置，不会连接 MCP 服务器或打开私有持久化存储；缺少当前模型提供商必需的 Key 时会以非零状态退出。需要验证远端 MCP 连通性时，显式使用 `jiaclaw doctor --connect --config ...`；该模式可能初始化私有 SQLite 状态，但不会发送模型请求或调用工具。离线验收需明确配置 `provider_type = "stub"`。详见 [`doctor` 检查合同](docs/doctor.md)。打开 [本机工作台](http://127.0.0.1:8080)，输入 API Token，创建会话并聊天。Token 仅留在页面内存中。服务缺少模型 Key 时启动失败。

CLI 也支持持久化多轮对话：

```sh
jiaclaw chat --config "$HOME/.jiaclaw/config.toml" --session personal "你好"
```

同一数据库只允许一个进程使用；`serve` 运行时通过 Web/API 操作会话，先停止服务再使用 CLI 会话命令。

单次 CLI 可用 `chat --stream` 输出真实逐事件 JSON-lines，要求显式模型账本、有效工具期限和独占 Unix stdout 管道/文件。模型收据、预览取消与 Web/上游认证边界见[CLI 流式合同](docs/cli-streaming.md)。

## Release 与容器安装

仓库已转入 [StateKnot 组织](https://github.com/StateKnot/JiaClaw)，现有检出与安装/发布状态见[迁移说明](docs/organization-migration.md)。

源码或发布链路变更的 PR 会先在 Linux/macOS × x86_64/arm64 四个平台原生构建 Release 二进制，实际安装最终归档并保留版本、源码和资产摘要证据，见[候选发布验收](docs/release-candidates.md)。版本 tag 使用相同构建/打包/安装步骤，之后生成含 `SHA256SUMS` 的 **draft Release**。维护者审核并公开 Release 后，可使用固定版本安装脚本：

```sh
./scripts/install.sh v0.1.0
```

脚本验证校验和、确认二进制可运行且版本与指定 Release 一致后原子替换 `~/.local/bin/jiaclaw`，不调用 sudo、不改配置。首次 Release 未公开时请使用源码安装。Linux Release 在 Ubuntu 24.04 构建，需要 glibc 2.39 或更新版本；较旧发行版可使用容器或在本机编译。macOS 构建在 macOS 14 arm64 / 15 Intel 验证。

```sh
# 先修改 deploy/container.toml 的网关地址和逻辑模型，再注入两个环境变量。
docker compose up --build -d
```

容器以非 root 运行，根文件系统只读，持久化数据保存在命名卷。容器部署默认关闭 exec，不挂载 Docker socket。容器中的 `127.0.0.1` 指向容器本身；网关必须通过实际可达的容器网络地址访问。[部署与升级](docs/deployment.md)包含服务管理、备份、回滚和沙箱说明。

## 已实现的能力

- 十二个主文件工具与五个兼容名称：文件读写、目录、grep/glob、mkdir/move、stat/tree，以及二进制 `copy` / `file_copy`。copy 与其他文件/记忆工具共用目录句柄、八槽 I/O 容量和协作写锁，保留 64 MiB 流式复制与原子发布合同；[权限、取消和核对边界](docs/workspace-files.md)分别说明。
- 显式启用的 `exec` / `shell_exec`：白名单映射、固定镜像、无网络容器、非 root、时间与输出限制。默认关闭；没有宿主机 shell 回退。
- SQLite WAL 会话存储、旧 JSON 一次性迁移、导入导出、TTL、同一会话并发串行提交。
- StateKnot HTTP MCP：显式批准的只读工具、描述摘要固定、离线 schema 校验、有界 JSON/SSE 与取消；[配置与验收边界](docs/mcp.md)。
- Brokerrouter 原生工具调用、请求级工具白名单、整批参数校验与部分完成故障说明；[契约与限制](docs/native-tools.md)。
- 按可信任务用途选择 Brokerrouter 逻辑模型：聊天、渠道、定时任务、HEARTBEAT、摘要；固定工具循环策略、输出上限及有效配置回执，见[模型路由](docs/model-routing.md)。
- 内置 Web 聊天、会话创建/选择/删除，以及单实例管理员的 [发件箱审计与人工核对](docs/web-outbox.md)；同源 API，无前端构建依赖。
- 持久化 cron/interval 多任务：鉴权管理、独立会话、运行记录、超时/重启中断暂停与显式恢复；[定时任务指南](docs/scheduler.md)。[单实例工作台](docs/standalone-scheduler.md)提供任务管理与持久创建身份，[独立用户任务](docs/tenant-cron.md)保留网关准入。
- [MEMORY / SOUL / USER](docs/memory-files.md) 的受限文件读写、工作区技能与有界 HEARTBEAT；Telegram、Slack、Discord、[飞书](docs/feishu.md)、[企业微信自建应用](docs/wecom.md)和[钉钉企业内部机器人](docs/dingtalk.md)的授权入站去重、持久 outbox、回执核对与重启恢复，见[渠道配置](docs/channels.md)。
- 多用户聊天入口：独立容器/卷/私有网络、哈希 Key 轮换撤销、管理员签发只读 Key、有界代理与未知写入核对；[部署与验收边界](docs/gateway.md)。默认关闭的[独立用户 Telegram 私聊](docs/tenant-telegram.md)、[Slack](docs/tenant-slack.md)、[Discord Bot DM 命令](docs/tenant-discord.md)分别通过 PR #80/#83/#84 最终跨平台 CI；本批[独立用户飞书](docs/tenant-feishu.md)限定专用企业自建 App、固定人的 p2p 文本和停机核对，验收进行中。只读 Key 已通过 PR #81 最终 CI；可信管理员可[按用户查询保留审计](docs/gateway.md#按用户查询管理审计)，当前批次验证状态见[验证记录](docs/validation.md)。
- Bearer 鉴权、请求体上限、限流、指标、结构化日志与优雅退出。渠道未配置鉴权时关闭。

显式 [Brokerrouter 语义记忆](docs/semantic-memory.md)、[模型调用收据](docs/model-calls.md)和受限独立用户定时任务已接线。MCP stdio、外部写工具的 durable admission、真正逐 token 流式、子 Agent、其他多用户后台/渠道身份绑定、WhatsApp 与多模态仍待完成；真实模型检索质量、供应商和渠道认证另行验收。详细验收要求见[里程碑](docs/roadmap.md)，实际配置字段见[配置说明](docs/configuration.md)，API 与执行边界见[架构说明](docs/architecture.md)。

## 验证

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D clippy::correctness -D clippy::suspicious
cargo build --locked -p jiaclaw-host
python3 tests/e2e.py target/debug/jiaclaw
python3 tests/memory_io.py target/debug/jiaclaw
python3 tests/workspace_files.py target/debug/jiaclaw
python3 tests/file_search.py target/debug/jiaclaw
python3 tests/workspace_mutations.py target/debug/jiaclaw
python3 tests/filesystem_info.py target/debug/jiaclaw
python3 tests/workspace_copy.py target/debug/jiaclaw
python3 tests/tenant_telegram.py target/debug/jiaclaw
python3 tests/installer.py
```

真实 Chromium、Docker 沙箱和镜像验收由 CI 执行。全套单元/HTTP fixture 测试不需要真实供应商 Key；真实模型、网关计费和渠道服务仍需独立联调。许可证：Apache-2.0 OR MIT。
