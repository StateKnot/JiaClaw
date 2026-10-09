# CLI 真实流式与模型收据

单次命令 `chat --stream` 从 Brokerrouter 的 SSE 逐事件读取文本，输出 UTF-8 JSON-lines。完整工具参数只在内部聚合；完整模型收据提交后，仍经过既有整批权限、Schema、ID 和预算校验，再执行工具。HTTP `/api/chat`、Web、REPL、渠道和独立用户网关本批没有开启真实流式。

## 启用

使用已审核、支持 streaming 及 tools+streaming 的网关端点，显式启用私有模型账本与工具期限：

```toml
[agent]
tool_timeout_secs = 30

[model_calls]
enabled = true
store_path = "../state/model-calls/index.sqlite3"
```

保留完整配置的其他字段。有效工具期限（含环境覆盖）须为1–30秒；不支持的配置在 CLI 创建 Agent 前拒绝。工作区必须存在。stdout 必须是本命令独占的 Unix 管道或常规文件；直接终端可使用：

```sh
jiaclaw chat --config /private/config.toml --session personal --stream '你好' | cat
```

管道下游可能收到敏感模型文本。日志独立写 stderr，支持既有 text/JSON 日志格式。不要让另一个 writer 共享 stdout 管道。先停止使用同一数据库的服务；CLI 会话与模型账本继续使用各自独占锁。stdout 文件写入受正常文件系统可用性约束，不能强制中断内核 I/O；管道 readiness 与小块写入提供慢消费者的有限等待。

## 事件与权威

| 事件 | 含义 |
|---|---|
| `model_started` | 已持久保存本地 operation 与唯一远端 UUID，随后才允许预览；携带 turn/round/model |
| `preview` | 指定模型轮的临时文字，单段最多1024 UTF-8字节；各轮文字分别显示 |
| `model_completed` | 上游完整结束且本地模型收据已提交，尚不代表工具/会话完成 |
| `tool_completed` | 原始调用 ID 对应的工具尝试已返回；可能失败或超时，不是效果恢复检查点 |
| `done` | 最终 `reply/status` 权威；会话写入成功后才发，`persisted` 明示是否保存 |
| `error` | 没有最终交付；核对原始模型收据、会话和工作区，勿自动重发整个 turn |

预览不授予工具执行权限。`done.status=requireshumaninput` 要求核对已尝试工具；不是任务成功。出现 EOF、错误或取消时，不能把先前预览作为已保存的最终回答。没有自动重连、Last-Event-ID、POST 重试、模型降级或工具重放。

每个模型轮固定管理员选择的逻辑模型，精确序列化 `stream:true` 正文后持久保存 SHA-256，POST 的 Idempotency-Key 是该原始 operation UUID。应用 turn UUID 不作为 Brokerrouter 治理 turn，当前没有创建上游审批身份。完整接收并校验 role、稳定 id/created/model、单个 index=0 choice、结束原因、唯一精确 usage 和 `[DONE]`，再构造普通 Chat Completions JSON 收据。finish_reason 或 EOF 单独不能证明结算；本地还要求原 HTTP 正文有界结束且无后续数据事件。

## 有限资源与取消

进程同时最多四个流式 delivery/settlement owner，每个预览队列最多八个有限事件；满载立即拒绝新 stream。当前模型 worker 与消费者共同持有容量，等待者取消不提前释放实际结算占用。同库仍最多一个模型调用。输出 writer 只有一个阻塞任务，最多八段、每段4 KiB，逐次写入最多512字节且不修改共享 fd flags；一行的队列/管道 readiness 使用原始5秒期限。

预览交付等待100毫秒即关闭该 delivery，后续预览丢弃，继续读取当前已提交模型直到原60秒期限并保存 completed 或 unknown。SSE 原始正文最多4 MiB、100000个 JSON 事件；增量支持分割 UTF-8、CR/LF/CRLF、注释、多行 data 和大单事件重放。拒绝 event/id/retry、未知或重复权威字段、混合身份/模型、非法 usage、断流和越界。聚合收据最多2 MiB、32工具/轮、每工具16 KiB参数；不会执行不完整片段。

输出断开、满载或 SIGINT/SIGTERM 停止后续模型/工具；已提交模型仍由独立 owner 结算。已开始工具先等待有限的尝试结果，再停止后续派发；失败、超时或输出超限保留人工核对结果，不继续另一工具/模型。通用超时不能证明后台 I/O 已停止或效果为零。CLI 等待当前 turn/模型最多65秒的停止宽限，输出 drain 另受其5秒期限；超限或 SIGKILL 后核对持久 hold 与实际效果。已有工具的中断说明可以写入配置的会话，取消本身不补写未完成助手回答。

仅完整最终响应且会话保存成功时发送 done；模型 completed、会话 committed、用户收到完整 done 是三个事实。stdout 失败可能发生在数据库提交之后，应读已有会话/收据核对，不重放。工具历史仍只有用户可见消息，不提供 durable 执行恢复。

## 账本兼容与恢复

模型账本 schema2持久记录 streaming 模式。首次打开真实 schema1时，在既有身份、workspace、完整布局、完整性与容量校验后原子升级；旧 operation/receipt/hold 原样保留、模式标为 JSON。未知或改过的 schema1拒绝迁移。升级前按[模型账本备份要求](model-calls.md)一致备份；旧二进制拒绝 schema2，不能回滚旧账本绕过未决请求。

未知 stream 经显式 `model-calls recover` GET核对时，额外要求完整 completion envelope 与精确 usage；没有合法响应则保留 hold。恢复只保存收据并返回 `applied_to_turn:false`，不执行收据中的工具或恢复预览/会话。

## 验收与上游门槛

[PR #93](https://github.com/StateKnot/JiaClaw/pull/93) 最终 head `43cff1267ded9afeaf7a8dc1a028f82127e56eb4` 的七项必需 checks 均 SUCCESS：两平台完整普通 CI、实际容器和四平台优化候选/安装；公开 Release draft 步骤按 PR 条件 SKIPPED。[固定源码与资产回填](validation.md#单次-cli-真流式最终-ci-回填)保留首轮 Intel fixture 失败和最终证据。HTTP/Web 尚未增加真正流式入口。

```sh
cargo test --workspace --locked
cargo build --locked -p jiaclaw-host
python3 tests/cli_stream.py target/debug/jiaclaw
```

这是实际二进制+本机 HTTP/SQLite/管道/信号验收。固定合同取自 [Brokerrouter main 消费方指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)；上游慢消费者/连接容量修复 [PR #40](https://github.com/StateKnot/Brokerrouter/pull/40) 尚未合并且作业未获执行，真实供应商 stream+tools/用量/结算的 [#31](https://github.com/StateKnot/Brokerrouter/issues/31)仍开放。这批接线不代替完整生产栈认证，不宣称主线网关的资源修复已经交付。后续先接实际 Web/HTTP 生命周期与浏览器，再独立认证网关、端点和代理；StateKnot durable 原生输出仍依 [#41](https://github.com/StateKnot/Brokerrouter/issues/41)。
