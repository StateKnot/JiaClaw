# 当前架构

`jiaclaw-core` 保存配置与领域类型，`jiaclaw` 实现工作区/技能/工具/对话循环和 Provider，`jiaclaw-host` 承载 CLI、HTTP、渠道、SQLite 与内置 Web。

```text
CLI / Web / channel webhook / HEARTBEAT
             |
     per-session turn lock
             |
SQLite history -> JiaClawAgent -> BrokerrouterProvider -> Brokerrouter -> model
                       |
                 ToolRegistry
               /
       workspace tools      controlled exec
       cap-std copy         digest-pinned Docker sandbox
             |
   commit SQLite before completed response
```

模型网关适配是现有应用路径；StateKnot durable runtime 尚未链接。没有伪装成框架调用的替代 MCP、A2A 或子 Agent。未来 durable adapter 必须接管 admission、执行、存储和恢复语义。

## 会话边界

SQLite 是服务/CLI 的权威会话存储，支持 WAL/FULL 同步与独占进程锁。数据库放在 Agent 工作区外，工具不能把数据库当工作文件操作。同一会话的读取-模型调用-工具循环-提交通过异步锁串行化，跨会话可并发。SQLite 操作在线程池执行；TTL 清理不删除活跃轮次。状态写入失败对外返回错误，不继续声称成功。

会话存储不保存工具 attempt、幂等身份或审批决策。进程在工具副作用发生后、历史提交前崩溃，历史可能没有这一步；人工重新发送可能再次执行。StateKnot durable 集成必须处理这个语义，当前不提供自动恢复工具轮次。

## HTTP 契约

- `GET /` 与 `/ui/app.js`, `/ui/app.css`：同源工作台，CSP 禁止第三方脚本/嵌入；模型内容用 textContent 呈现。
- `/api/chat`：历史会话或一次性聊天；现有 `stream=true` 是完成后 SSE，不是真正 token streaming。
- `/api/sessions`：GET 列表 / POST 空会话；`/:id` GET/DELETE；`/:id/export` GET；`/import` POST。
- `/api/tools`, `/api/skills`, `/api/skills/reload`, `/api/openapi.json`：工具/技能管理与 API 草图。
- `/hooks/inbound`, `/hooks/telegram`, `/hooks/slack`, `/hooks/discord`：只有配置各自入站 secret/公钥才开放；现有渠道签名继续验证原始 body。
- `/health` 公开；`/metrics` 依配置鉴权。API 使用 Bearer 或 X-Api-Token。共享限流、body 上限、request ID 与优雅退出沿用现有中间件。

当前 API token 是实例级鉴权，不是用户身份。个人工作区与渠道共用资源，不支持多租户授权隔离。

## 文件与执行边界

copy 使用目录能力与相对路径，拒绝链接/特殊文件，有界读取，临时文件 fsync 后 atomic publish；无覆盖使用 hard link，覆盖使用 rename。其他现有文件工具仍由各自实现处理路径与大小限制，不能推断为完整 OS 沙箱。

exec 的可信配置固定 Docker CLI、镜像摘要、容器命令映射与上限；模型只能选择已配置命令和有限参数。沙箱不继承宿主凭证或网络，仅挂载工作区。取消清理由独立线程完成；daemon 不可达或宿主被硬终止时需运维确认残留。容器部署不暴露 Docker socket。

## 验收证据

单元与 HTTP fixture 测试覆盖本地工具、配置和渠道协议；`tests/e2e.py` 启动实际二进制，覆盖 API 鉴权、并发会话、关闭未配置渠道、SIGKILL 恢复与持久删除。`tests/browser.cjs` 在实际 Chromium 验收连接/聊天/删除、文本渲染、内存 Token 与移动布局；`tests/installer.py` 用本地 Release fixture 验证安装与失败保留旧版本；`tests/container.py` 使用实际镜像/命名卷验证非 root 与重启恢复。

真实供应商计费、真实渠道出站和 StateKnot durable 故障恢复不在上述 fixture 证据内。依赖的认证门槛见上游状态文档。
