# 部署、升级与恢复

先部署并认证 Brokerrouter，再部署 JiaClaw。上游 [`init-personal` 消费方指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)提供本地初始化路径，但不代替真实供应商、TLS、计费与生产认证。JiaClaw 的默认工具注册需要网关模型完成工具闭环；未认证模型可能返回 `unsupported_model_capability`，不能删工具字段或直连绕过。

## 单机与网络

个人本机实例使用 `127.0.0.1`，配置独立的随机 API Token 与有限预算网关虚拟 Key。除非显式离线验收，provider 使用 brokerrouter；模型失败返回错误，不自动用新键重发。状态和工作区必须分开，默认 `~/.jiaclaw/workspace` 与 `~/.jiaclaw/state`。

服务部署使用单进程、单 SQLite 数据库、本地可靠文件系统。不要把数据库放进网络共享盘、工作区或允许模型修改的目录。普通 `serve` 仍是单用户实例，没有多副本写入协议。多用户聊天与会话服务须使用[独立用户网关部署](gateway.md)：每个用户独立容器、受限持久卷、私有网络及供应商虚拟 Key；不能把多个 Key 直接配置到共享 Agent。受限[网关定时任务](tenant-cron.md)已独立验收；默认关闭的[独立用户 Telegram 私聊](tenant-telegram.md)本批接线，验收状态见专门指南。其他多用户渠道/后台能力和 durable 恢复仍待完成。

公网接入通过 TLS 反向代理，仅公开需要的端点。保持精确 CORS 来源；内置工作台不需 CORS。渠道必须同时配置专属鉴权及 `http.channels` 安装/身份/工具白名单；旧密钥配置缺少策略时启动失败，未启用的渠道返回 404。配置后仍需在渠道侧登记正确 HTTPS webhook 和验证签名。API Token 不替代 Telegram/Slack/Discord/飞书/企业微信/钉钉的鉴权。钉钉 HTTP 机器人签名不覆盖正文，必须从钉钉到可信入口使用 HTTPS，入口到 JiaClaw 的链路也须受保护，禁止代理、中间件或审计工具记录可重放的 timestamp/sign 头；不能让客户端绕过可信入口直连服务。

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


## 工作区目录与移动

工作区根及其祖先目录须由管理员控制，预设服务用户的所有权、权限和 umask。十二个主文件工具与五个兼容名称通过共享目录句柄操作，异步入口和记忆工具共用八槽阻塞 I/O 容量，协作 mutation（含 copy/file_copy）使用同一工作区 inode 锁；stat/tree 等只读工具不取该锁，不提供并发编辑下的一致快照。这不能阻止外部进程更换根目录、移动其祖先或绕过协作锁。exec/MCP 的文件权限需单独审核，不参与该协作串行化。

`move` 只允许同卷原子 rename。即使源与目标看起来都在 workspace 下，分别挂载的文件系统也可能返回 EXDEV；本版移除了旧的跨卷 copy/delete 回退，不会自动迁移字节后删除来源。默认不覆盖依赖 Linux/macOS 的原子独占 rename，文件系统不支持时明确失败；覆盖失败不预先删除目的地。上线前验证实际挂载的目录锁、rename 与目录 fsync 支持，网络文件系统不能仅凭本机测试视作已认证。

`copy` 保留最大 64 MiB 的流式字节复制合同，允许二进制，元数据检查后最多读取上限 + 1 字节，不套用文本工具的 256 KiB。源与目标父目录须存在，源和已有目标必须为单链接常规文件。暂存文件建立在目标目录；无覆盖需要 hard link 原子发布，覆盖需要 rename，同步文件并同步目标父目录后才返回成功。源/目标可在不同挂载卷上，但不是跨卷原子事务。部署前必须在实际服务用户和实际挂载上验证目录锁、hard link、rename、文件/目录 fsync 与磁盘空间，不能用开发机通过代替挂载资格。

mkdir 中途失败可能留下已创建目录；move/copy 报同步错误、超时或取消时可能已经提交。copy 的暂存链接清理失败也可能发生在目标已发布之后。先停止新写入，等待已受理任务结束或停止服务，再核对实际目录、源/目标内容和 `.jiaclaw-memory-*` / `.jiaclaw-copy-*` 暂存文件；暂存文件不自动当作成功内容恢复。不要自动重试或依赖恢复旧会话数据库撤销工作区变更。没有持久文件操作收据或 durable 重放，非协作编辑器也不受串行化保护。完整参数、竞争与故障边界见[工作区文件指南](workspace-files.md)。

## 备份、迁移、回滚

最简单可靠的备份：先停止 JiaClaw，复制整个 state 目录（含 sqlite3 与可能的 WAL/SHM）和 workspace，再启动服务。在线文件复制单个 `.sqlite3` 会遗漏 WAL，不可靠；如需在线备份，使用 SQLite backup API/官方 `.backup` 路径，并额外备份工作区。备份包含聊天与记忆，按 Secret 数据保护。

旧 JSON 迁移源保留不删除，导入和标记在同一事务内；坏 JSON 或不支持的未来 schema 导致启动失败。不要修改 user_version 来强制降级。恢复演练应使用独立实例和复制的目录，验证已提交历史、空会话和删除状态。

升级时停止服务、备份、安装固定版本、启动并检查。同一数据库有排他锁，不允许旧/新进程同时打开。当前版本 schema=10，启动时事务迁移受支持的旧版本，保留会话、jobs/runs、渠道事件与投递回执。v7 引入钉钉定时投递，v8 增加网关派发身份与时间高水位，v9 增加 Discord Bot 凭据持久阻断，v10 增加独立于任务删除的创建身份收据；发件箱的来源与外键约束继续保留。完整备份必须包括创建收据，删除它们或恢复陈旧快照不能继续保证拒绝旧创建 ID 的重放，详见[创建身份与备份](standalone-scheduler.md#数据库容量和备份)。企业微信独立发送额度账本随数据库保存，清理业务审计不会返还发送预算。旧二进制会拒绝新库。Discord 的 JIACLAW_CHANNEL_STATE_KEY 必须与 state 分别保护并共同备份，移除或替换密钥前先排空待发送交互。回滚必须恢复升级前的完整 state 备份，不能指望旧进程读取新库。已有 JSON 源也不是升级后的完整历史备份。

企业微信额度依赖持久时间记录，宿主时钟必须可靠并保持同步。恢复旧 state 快照会丢失快照之后的发送预约，因此恢复后应保持服务停止，独立核对旧进程所有在途或未知请求，并至少等到最后一条可能被平台接受的消息之后满 24 小时再启用企业微信。如需先恢复其它功能，必须移除企业微信 `http.channels` 安装项及其三个配置/环境凭据后再启动；当前没有单独暂停企业微信出站的开关。数据库回滚不会撤回平台消息，也不能恢复平台剩余额度；不要把旧备份当作发送重试或额度重置手段。

CLI `session export/import` 与 HTTP 导入导出提供会话级迁移。serve 正在运行时使用鉴权 HTTP API，避免第二个 CLI 进程竞争数据库。消息摘要/TTL 会删除或压缩历史，开启前确认保留策略。

会话库与独立用户 Telegram/Slack/Discord/飞书/企业微信队列库正常关闭时，先关闭 SQLite 再显式释放 lifetime flock；初始化失败也释放取得的所有权。不要通过删除 `.sqlite3.lock` 文件解除占用，这会让两个描述符锁住不同 inode。正常关闭的显式解锁消除 fork/dup 保留旧描述符造成的锁余存，但强杀/断电没有析构保证；先核对原实例与子进程均已停止再打开。若仍报告 busy，保留拒绝并检查真实占用和挂载锁语义，不自动重试写入或绕过独占。

## 发布审核

CI 对 Linux/macOS 运行完整测试、真实二进制 E2E、安装 fixture；Linux 额外做 Docker 沙箱与镜像启动。[候选发布工作流](release-candidates.md)在源码或发布链路 PR 上执行四平台原生 Release build，验证实际归档、安装、回滚及源码/资产摘要，只保留 Actions artifact。`v<workspace.version>` tag 复用同一构建步骤，四项均成功才创建带校验和的 draft。发布维护者审核固定源码与各平台证据、ABI、上游模型认证与恢复证据后再公开；不同提交的旧成功不能替代。安装脚本只使用显式版本，不自动执行下载内容的初始化或覆盖配置。

钉钉部署仅支持已发布的企业内部应用机器人，HTTP 回调登记为 `/hooks/dingtalk`。安装身份 `robotCode:corpId` 与 `app_id`（Client ID）分别配置；secret 默认关闭，使用 `JIACLAW_DINGTALK_APP_SECRET` 提供 Client Secret。入站成员、会话与主动发送目的地须分别列入精确白名单。`delivered` 表示平台返回有效接收回执，不代表终端显示或已读；平台配额、回调重投和真实消息展示仍需在目标安装验收，见[钉钉指南](dingtalk.md)。
