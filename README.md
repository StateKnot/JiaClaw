# JiaClaw

JiaClaw 是用 Rust 实现的个人 Agent：CLI 对话、工作区文件和记忆工具、渠道 webhook、SQLite 会话历史，以及内置聊天工作台。模型请求推荐统一经过 [Brokerrouter](https://github.com/StateKnot/Brokerrouter)。[StateKnot](https://github.com/StateKnot/StateKnot) 的 HTTP MCP 已接入，持久化执行框架仍待接线与独立认证。

**当前边界**：SQLite 保存会话，不提供工具调用的持久化执行、断点恢复或 exactly-once 保证。StateKnot alpha 与 Brokerrouter 的真实供应商工具闭环仍有生产认证门槛，详见[上游状态](docs/roadmap.md)。不能把本仓库的单机测试通过解释为完整个人 Agent 栈已经生产认证。

## 从源码安装

需要 Rust 1.88.0、C 编译器与 Git。SQLite 随二进制编译，无须单独安装数据库。

```sh
git clone https://github.com/jiawenyao401/JiaClaw.git
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

打开 [本机工作台](http://127.0.0.1:8080)，输入 API Token，创建会话并聊天。Token 仅留在页面内存中。服务缺少模型 Key 时启动失败；离线验收需明确把 `provider_type` 改为 `stub`。

CLI 也支持持久化多轮对话：

```sh
jiaclaw chat --config "$HOME/.jiaclaw/config.toml" --session personal "你好"
```

同一数据库只允许一个进程使用；`serve` 运行时通过 Web/API 操作会话，先停止服务再使用 CLI 会话命令。

## Release 与容器安装

版本 tag 触发四个平台的构建：Linux/macOS × x86_64/arm64，并生成含 `SHA256SUMS` 的 **draft Release**。维护者审核并公开 Release 后，可使用固定版本安装脚本：

```sh
./scripts/install.sh v0.1.0
```

脚本验证校验和、确认二进制可运行后原子替换 `~/.local/bin/jiaclaw`，不调用 sudo、不改配置。首次 Release 未公开时请使用源码安装。Linux Release 在 Ubuntu 24.04 构建，需要 glibc 2.39 或更新版本；较旧发行版可使用容器或在本机编译。macOS 构建在 macOS 14 arm64 / 15 Intel 验证。

```sh
# 先修改 deploy/container.toml 的网关地址和逻辑模型，再注入两个环境变量。
docker compose up --build -d
```

容器以非 root 运行，根文件系统只读，持久化数据保存在命名卷。容器部署默认关闭 exec，不挂载 Docker socket。容器中的 `127.0.0.1` 指向容器本身；网关必须通过实际可达的容器网络地址访问。[部署与升级](docs/deployment.md)包含服务管理、备份、回滚和沙箱说明。

## 已实现的能力

- 工作区文件读写、目录、grep/glob、mkdir/move，以及原子 `copy` / `file_copy`。
- 显式启用的 `exec` / `shell_exec`：白名单映射、固定镜像、无网络容器、非 root、时间与输出限制。默认关闭；没有宿主机 shell 回退。
- SQLite WAL 会话存储、旧 JSON 一次性迁移、导入导出、TTL、同一会话并发串行提交。
- StateKnot HTTP MCP：显式批准的只读工具、描述摘要固定、离线 schema 校验、有界 JSON/SSE 与取消；[配置与验收边界](docs/mcp.md)。
- Brokerrouter 原生工具调用、请求级工具白名单、整批参数校验与部分完成故障说明；[契约与限制](docs/native-tools.md)。
- 内置 Web 聊天、会话创建/选择/删除；同源 API，无前端构建依赖。
- 持久化 cron/interval 多任务：鉴权管理、独立会话、运行记录、超时/重启中断暂停与显式恢复；[定时任务指南](docs/scheduler.md)。
- MEMORY / SOUL / USER、工作区技能与 HEARTBEAT；Telegram、Slack、Discord 的授权入站去重、持久 outbox、回执核对与重启恢复，见[渠道配置](docs/channels.md)。
- Bearer 鉴权、请求体上限、限流、指标、结构化日志与优雅退出。渠道未配置鉴权时关闭。

MCP stdio、外部写工具的 durable admission、真正逐 token 流式、子 Agent、多用户隔离、新渠道、语义记忆和多模态尚未完成。详细验收要求见[里程碑](docs/roadmap.md)，实际配置字段见[配置说明](docs/configuration.md)，API 与执行边界见[架构说明](docs/architecture.md)。

## 验证

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D clippy::correctness -D clippy::suspicious
cargo build --locked -p jiaclaw-host
python3 tests/e2e.py target/debug/jiaclaw
python3 tests/installer.py
```

真实 Chromium、Docker 沙箱和镜像验收由 CI 执行。全套单元/HTTP fixture 测试不需要真实供应商 Key；真实模型、网关计费和渠道服务仍需独立联调。许可证：Apache-2.0 OR MIT。
