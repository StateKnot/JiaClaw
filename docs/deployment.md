# 部署、升级与恢复

先部署并认证 Brokerrouter，再部署 JiaClaw。上游 [`init-personal` 消费方指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)提供本地初始化路径，但不代替真实供应商、TLS、计费与生产认证。JiaClaw 的默认工具注册需要网关模型完成工具闭环；未认证模型可能返回 `unsupported_model_capability`，不能删工具字段或直连绕过。

## 单机与网络

个人本机实例使用 `127.0.0.1`，配置独立的随机 API Token 与有限预算网关虚拟 Key。除非显式离线验收，provider 使用 brokerrouter；模型失败返回错误，不自动用新键重发。状态和工作区必须分开，默认 `~/.jiaclaw/workspace` 与 `~/.jiaclaw/state`。

服务部署使用单进程、单 SQLite 数据库、本地可靠文件系统。不要把数据库放进网络共享盘、工作区或允许模型修改的目录。当前没有多用户隔离，也没有多副本写入协议；要扩容应先完成 StateKnot durable 与鉴权里程碑。

公网接入通过 TLS 反向代理，仅公开需要的端点。保持精确 CORS 来源；内置工作台不需 CORS。渠道未配置专属鉴权时返回 404，配置后仍需在渠道侧登记正确 HTTPS webhook 和验证签名。API Token 不替代 Telegram/Slack/Discord 的鉴权。

## Linux systemd

[`deploy/jiaclaw.service`](../deploy/jiaclaw.service)作为专用非 root 系统用户运行，状态目录 `/var/lib/jiaclaw`。部署管理员需创建 jiaclaw 用户，将已验证二进制放入 `/usr/local/bin/jiaclaw`，准备 `/etc/jiaclaw/config.toml` 与 root 所有、0600 的 `/etc/jiaclaw/environment`。

配置使用实际路径：

```toml
[agent]
name = "JiaClaw"
description = "Personal assistant"
system_instructions = "You are JiaClaw, a helpful personal assistant."
max_turns = 10
workspace_path = "/var/lib/jiaclaw/workspace"
tool_timeout_secs = 45

[provider]
provider_type = "brokerrouter"
base_url = "http://127.0.0.1:8081"
model = "YOUR_CERTIFIED_LOGICAL_MODEL"

[http]
bind = "127.0.0.1:8080"
persist_path = "/var/lib/jiaclaw/state/sessions.sqlite3"
metrics_public = false
rate_limit_per_minute = 60
shutdown_timeout_secs = 30

[tools.exec]
enabled = false
```

environment 文件设置 `JIACLAW_API_KEY` 与 `JIACLAW_API_TOKEN`；用实际 Secret 注入，不把真实 Key 提交到仓库。安装 unit 后执行 `systemctl daemon-reload` 与 `systemctl enable --now jiaclaw`。确认 `/health`、鉴权后的会话 API、文件权限与 `journalctl -u jiaclaw`。unit 不提供交互配置向导；路径、网关地址与逻辑模型必须明确配置。

## Docker

[`Dockerfile`](../Dockerfile)执行 locked release build，运行层只含二进制、CA/curl，UID:GID=10001:10001。[`compose.yaml`](../compose.yaml)将根文件系统设为只读，移除 capabilities，限制 CPU/内存/PID，只有 `/data` 可持久写入。`deploy/container.toml` 需修改网关地址/模型；Compose 要求环境中已有两个 Key。命名卷初次初始化由镜像目录所有权赋给 10001，不需要 chmod 777。

```sh
docker compose up --build -d
docker compose logs --tail=100 jiaclaw
docker compose ps
```

健康检查访问实际 `/health`。SQLite 与工作区位于同一个持久卷的不同目录。不要执行 `docker compose down -v` 来升级，它会删除历史和记忆。容器默认不支持 exec：不要为启用它把宿主 Docker socket 挂入应用容器。

构建默认使用实际构建验证过的 Rust、Debian 和 Dockerfile frontend 摘要。生产发布升级基础镜像时应审核并用 `--build-arg RUST_IMAGE=...@sha256:... --build-arg RUNTIME_IMAGE=...@sha256:...` 更新基础镜像 digest，并复验所有支持的平台。本仓库工作流只构建/验收，不自动推送镜像仓库。

## 宿主机 exec

确实需要执行时，在独立个人宿主实例配置 Docker CLI 与摘要固定的沙箱镜像，并在管理员侧预先拉取。使用受控或 rootless Docker daemon，授予服务访问 daemon 的权限前审核该权限边界。Docker daemon 可控性意味着宿主高权限，不等于普通文件工具权限。

为沙箱指定非 root UID/GID，确认该 UID 可遍历和读取工作区；写权限只授予确需写入的目录，显式将 `workspace_read_only=false`。默认只读、无网络，不能下载包。不要把 Secret、宿主配置、会话数据库或 `.git` 凭证放入 Agent 工作区。

超时/future 取消会执行 `docker rm --force`，失败记录容器名。宿主 SIGKILL 或断电时清理代码无法运行；恢复后先停止 JiaClaw，检查 `docker ps -a --filter label=jiaclaw.sandbox=true`，确认属于该实例再删除残留。不要在服务运行时无差别删除其他实例的容器。需要跨进程自动恢复租约/执行清理时，应接 StateKnot durable 生命周期后再启用该承诺。

## 备份、迁移、回滚

最简单可靠的备份：先停止 JiaClaw，复制整个 state 目录（含 sqlite3 与可能的 WAL/SHM）和 workspace，再启动服务。在线文件复制单个 `.sqlite3` 会遗漏 WAL，不可靠；如需在线备份，使用 SQLite backup API/官方 `.backup` 路径，并额外备份工作区。备份包含聊天与记忆，按 Secret 数据保护。

旧 JSON 迁移源保留不删除，导入和标记在同一事务内；坏 JSON 或不支持的未来 schema 导致启动失败。不要修改 user_version 来强制降级。恢复演练应使用独立实例和复制的目录，验证已提交历史、空会话和删除状态。

升级时停止服务、备份、安装固定版本、启动并检查。同一数据库有排他锁，不允许旧/新进程同时打开。当前版本 schema=2，启动时从 schema=1 事务迁移并保留会话，新增 jobs/runs；schema=1 的旧二进制会拒绝新库。回滚必须恢复升级前的完整 state 备份，不能指望旧进程读取新库。已有 JSON 源也不是升级后的完整历史备份。

CLI `session export/import` 与 HTTP 导入导出提供会话级迁移。serve 正在运行时使用鉴权 HTTP API，避免第二个 CLI 进程竞争数据库。消息摘要/TTL 会删除或压缩历史，开启前确认保留策略。

## 发布审核

CI 对 Linux/macOS 运行完整测试、真实二进制 E2E、安装 fixture；Linux 额外做 Docker 沙箱与镜像启动。`v<workspace.version>` tag 才触发四平台 release build，验证版本后上传二进制、校验和并创建 draft。发布维护者审核 CI、平台 ABI、上游模型认证与恢复证据后再公开。安装脚本只使用显式版本，不自动执行下载内容的初始化或覆盖配置。
