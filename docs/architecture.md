# 当前架构

`jiaclaw-core` 保存配置与领域类型，`jiaclaw` 实现工作区/技能/工具/对话循环和 Provider，`jiaclaw-host` 承载 CLI、HTTP、渠道、SQLite 与内置 Web。

```text
CLI / Web / channel webhook / HEARTBEAT / scheduler
             |
     per-session turn lock
             |
SQLite history -> JiaClawAgent -> BrokerrouterProvider -> Brokerrouter -> model
                       |
                 ToolRegistry
               /
       workspace tools      controlled exec
       directory handles    digest-pinned Docker sandbox
             |
   commit SQLite before completed response
```

Brokerrouter 使用原生 `tools` / `assistant.tool_calls` / `role:tool`；请求白名单控制工具目录与执行权限，完整批次预检后按 ID 回传结果。后续模型失败时保留已经执行的工具记录并返回需人工核查状态。正文代码块不触发工具，详情见 [原生工具合同](native-tools.md)。

模型网关适配是现有应用路径；StateKnot durable runtime 尚未链接。MCP 使用 StateKnot 的发布版 HTTP client，启动时完成固定 endpoint、白名单、descriptor pin 和 schema 校验，再一次性注册到 ToolRegistry。未批准的工具和 server instructions 不进入模型提示。未来 durable adapter 必须接管 admission、执行、存储和恢复语义。

## 模型用途边界

可信 CLI/HTTP、渠道 worker、调度 worker 和 HEARTBEAT 入口分别选择 chat、channel、scheduled、heartbeat；摘要单独使用 summary。管理员的顶层 routing 配置映射到同一 Brokerrouter 的授权逻辑 model 和有限参数，在工具循环前固定。入口内容不能指定用途，渠道通知不改变 scheduled 选择；具体继承与上限见[模型路由](model-routing.md)。

路由启用时 ChatResponse 包含有效用途/逻辑模型/温度/输出上限，定时运行保存的 response JSON 保留这份回执。渠道 inbox/outbox 和普通会话没有新增持久路由记录。它不提供持久模型操作身份或未知提交恢复；网关仅对已证明未发送的端点调用降级，应用不增加失败自动重试。

## 会话边界

SQLite 是服务/CLI 的权威会话存储，支持 WAL/FULL 同步与独占进程锁。数据库放在 Agent 工作区外，工具不能把数据库当工作文件操作。同一会话的读取-模型调用-工具循环-提交通过异步锁串行化，跨会话可并发。SQLite 操作在线程池执行；TTL 清理不删除活跃轮次。状态写入失败对外返回错误，不继续声称成功。

会话存储不保存工具 attempt、幂等身份或审批决策。进程在工具副作用发生后、历史提交前崩溃，历史可能没有这一步；人工重新发送可能再次执行。StateKnot durable 集成必须处理这个语义，当前不提供自动恢复工具轮次。

## HTTP 契约

- `GET /` 与 `/ui/app.js`, `/ui/app.css`：同源工作台，CSP 禁止第三方脚本/嵌入；模型内容用 textContent 呈现。
- `/api/chat`：历史会话或一次性聊天；现有 `stream=true` 是完成后 SSE，不是真正 token streaming。
- `/api/sessions`：GET 列表 / POST 空会话；`/:id` GET/DELETE；`/:id/export` GET；`/import` POST。
- `/api/tools`, `/api/skills`, `/api/skills/reload`, `/api/openapi.json`：工具/技能管理与 API 草图。
- `/hooks/inbound`, `/hooks/telegram`, `/hooks/slack`, `/hooks/discord`, `/hooks/feishu`, `/hooks/wecom`, `/hooks/dingtalk`：按各平台合同验证 secret、公钥、签名或加密消息；具名渠道还要求显式安装及身份策略。企业微信同一路径支持 GET 验证和 POST 加密 XML 回调。钉钉为企业内部机器人 HTTP 私聊，timestamp/sign 不涵盖正文，依赖可信 HTTPS 入口并独立校验安装与企业成员；不消费 sessionWebhook。
- `/health` 公开；`/metrics` 依配置鉴权。API 使用 Bearer 或 X-Api-Token。共享限流、body 上限、request ID 与优雅退出沿用现有中间件。

当前 API token 是实例级鉴权，不是用户身份。个人工作区与渠道共用资源，不支持多租户授权隔离。

## 文件与执行边界

`read_file`、`write_file`、`delete_file`、`str_replace`、`list_dir`、`grep`、`glob`、`mkdir`、`move`、`copy`、`stat`、`tree` 十二个主工具及对应五个兼容名称通过共享 `memory_io` 目录句柄访问工作区。文件正文读写和复制拒绝符号/硬链接目标及特殊文件；元数据/目录工具可以报告这些条目的类型但不跟随目标。路径最多 1024 UTF-8 字节、64 个组件，执行有界读取、写入和遍历。异步入口与记忆工具共用 8 个阻塞 I/O 许可；协作 mutation 共用工作区 inode 锁。取消等待不会提前释放仍在执行的许可或锁，也不会终止已受理写入。配置、兼容变化和准确范围见[工作区文件指南](workspace-files.md)。

copy 与其他 mutation 共用目录能力、容量与写锁，字节及发布合同单独保留：最大 64 MiB 二进制流式复制，元数据检查后最多读取上限 + 1 字节以检测增长；同步目标目录中的随机暂存文件（Unix 0600），无覆盖使用 hard link 原子 create-if-absent，覆盖使用 rename 且不预删目标，再同步目标父目录。提交后同步或暂存链接清理错误属于需核对的未知结果，不能按失败自动复制；不提供非协作编辑的一致快照、持久操作收据或 durable 恢复。

grep/glob 限制所有扫描条目、深度、合作时间和最终 JSON，grep 另限制累计实际正文读取；glob 的跨目录模式使用动态规划避免指数递归。mkdir 通过父目录句柄逐级创建并同步；move 只作同卷原子 rename，默认使用原子不覆盖，显式覆盖也不预删目的地，跨卷或不支持操作时报错。它们持有与记忆写入相同的工作区协作锁；取消与同步失败可能发生于变更之后，需要核对目录或源/目标两端。stat/tree 由独立的 filesystem_info 模块提供，只读元数据与有界目录树，不读取正文或取得 mutation 锁；tree 每层排序 DFS，list_dir 保留最终完整名称排序，两者都按完整工作区路径限制长度与组件数。持久渠道和 cron/interval 固定工具白名单不加入它们；管理员启用的独立 HEARTBEAT 与兼容 `/hooks/inbound` 沿用全部已注册工具，受各自配置开关控制。工作区根及祖先须由管理员控制，生产挂载须支持实际锁、hard link、rename 与 fsync；这些应用工具不构成完整 OS 沙箱。

exec 的可信配置固定 Docker CLI、镜像摘要、容器命令映射与上限；模型只能选择已配置命令和有限参数。沙箱不继承宿主凭证或网络，仅挂载工作区。取消清理由独立线程完成；daemon 不可达或宿主被硬终止时需运维确认残留。容器部署不暴露 Docker socket。

## 验收证据

单元与 HTTP fixture 测试覆盖本地工具、配置和渠道协议；`tests/e2e.py` 启动实际二进制，覆盖 API 鉴权、并发会话、关闭未配置渠道、SIGKILL 恢复与持久删除。`tests/browser.cjs` 在实际 Chromium 验收连接/聊天/删除、文本渲染、内存 Token 与移动布局；`tests/installer.py` 用本地 Release fixture 验证安装与失败保留旧版本；`tests/container.py` 使用实际镜像/命名卷验证非 root 与重启恢复。

真实供应商计费、真实渠道出站和 StateKnot durable 故障恢复不在上述 fixture 证据内。依赖的认证门槛见上游状态文档。

## 持久调度

启用调度时，SQLite v2 的 jobs/runs 保存任务、UTC occurrence 和输入快照；领取事务后才调用 Agent。同一任务至多一个运行，全局上限 4。最终会话与 run 状态同事务提交，会话锁随 blocking 提交保留，即使调用 Future 被取消也不提前释放。进程中断、期限耗尽或未知结果会暂停任务，由管理员核查后恢复未来调度；没有自动重放工具。详细时间、权限和容量合同见[调度指南](scheduler.md)。

## 独立用户 Telegram

显式网关模式将 Bot/私聊发送者固定到 registry 用户及专属后端，用户启用/hold 与 Web/cron 共用；backend 只接收受 Token 保护的受限 channel 请求。网关先持久准入，并在私有绑定库的同一 claim 事务关联内部 UUIDv7 与 event/delivery attempt。后端会话和网关回复队列分属不同 SQLite；跨进程断连不能按成功事务恢复工具循环，遗留 processing/submitting 分别进入 needs_review/unknown，只有离线人工核对后才继续。队列私有文件共享网关有限额盘，不等于逐用户块设备隔离。配置与恢复见[独立用户 Telegram](tenant-telegram.md)。

## 可靠渠道

Telegram、Slack、Discord、飞书、企业微信和钉钉共享经过授权的持久入站与统一异步出站。平台事件 ID 与内容摘要用于去重；验签、安装/发送者/会话校验和 SQLite 提交完成后才 ACK，模型执行由受监督 worker 承担。在单实例渠道模式中，会话、事件终态与完整回复分片在同一事务写入；出站领取先持久化 submitting，再提交 HTTP 请求。发送结果不明进入 unknown，禁止自动重发及后续分片；429 冷却和基础发送间隔跨重启保存。SQLite 保存 inbox/outbox、去重记录及投递回执（v7 引入钉钉，当前 schema 10），并以外键和互斥约束区分入站事件与定时运行来源；企业微信另有跨审计清理保留的发送额度账本；详见[渠道运行合同](channels.md)、[企业微信指南](wecom.md)和[钉钉指南](dingtalk.md)。

定时 Telegram/Slack/飞书/企业微信/钉钉通知使用独立的目的地与工具授权，复用同一发送器、容量预留和目的地顺序。任务运行、会话和所有消息分片一起提交；有未解决投递的任务不会产生下一轮执行，发送结果未知或永久失败会暂停任务。清理历史必须保留被发件箱引用的来源，显式核对后才可删除；详见[定时通知](scheduled-delivery.md)。
