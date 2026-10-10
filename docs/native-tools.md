# Brokerrouter 原生工具往返

JiaClaw 的 `provider_type = "brokerrouter"` 使用 Chat Completions 原生函数工具合同：请求携带 `tools`，模型返回 `assistant.tool_calls`，应用执行获准工具后以 `role: "tool"` 和对应 `tool_call_id` 回传，再请求最终回答。MCP 工具经过 StateKnot 连接与本地审核后进入同一工具目录。

这一能力解决应用到网关之间的工具协议接线。它不等于 StateKnot durable Agent，也不构成真实供应商认证。供应商路径仍须完成 [Brokerrouter #31](https://github.com/StateKnot/Brokerrouter/issues/31) 的固定模型、区域、用量和结算验收。

## 工具授权与执行边界

`POST /api/chat` 的 `enabled_tools` 是当前调用的工具白名单：非空时只向模型发布所列已注册工具，并拒绝模型调用其余工具；未知名字在模型请求前失败。空数组保留已有 API 语义，允许当前配置已注册的全部工具。禁用的 exec 或未审核的 MCP 工具不会因此获得授权。

```json
{
  "session_id": "example",
  "messages": [{"role": "user", "content": "列出工作空间"}],
  "enabled_tools": ["workspace_list"]
}
```

工具执行前检查整个模型返回批次，包括工具名称与权限、唯一调用 ID、JSON 参数对象、注册工具的参数 schema 和调用数量。批次中任何一项不合法时，该批次不会执行任何工具。参数校验关闭外部 schema 获取。合法批次中的工具按顺序执行，多个调用的结果分别绑定原始 ID；这不是跨工具事务，已完成的外部副作用不能在后续工具失败时回滚。

原生定义的 Schema 编译与整个批次的参数校验使用与 [MCP](mcp.md#固定资源与失败合同) 共享的进程四槽阻塞 worker，包含尚在 Tokio 阻塞池等待的任务；满载立即失败。每次纯前置校验最多等待30秒，另受调用它的宿主/cron/渠道期限约束，不为整次工具循环新增恢复或总期限承诺。定义编译失败/到期在模型提交前拒绝；批次校验失败/到期在该批任何工具执行前拒绝。worker 只持有描述、参数、validators 和已有 ID，不持有工具/网络执行句柄；等待者取消后不能在后台派发效果，容量直到实际工作结束才释放。不能强制停止已运行 CPU 工作，结构/离线引用/正则预算与管理员 Schema 审查继续必要。已有工具记录的中断处理保持下述合同。

每轮最多接纳 32 个工具调用，单次对话请求最多 128 个，并受配置中的迭代次数限制。通用工具超时仅在配置 `tool_timeout_secs` 后生效，默认未设置；当前没有覆盖整次对话的总体 deadline，各具体工具另有自己的资源策略。最后一次模型迭代仍要求执行工具，或下一批次将超出总调用预算时，返回 `requireshumaninput` 状态及已完成记录，剩余批次不执行。模型消息必须来自 `assistant`，完整结束原因须为 `stop` 或与非空工具批次一致的 `tool_calls`。截断、畸形消息和不一致的结束原因会失败；不会猜测或执行不完整参数。

单个工具参数 JSON 不超过 16 KiB，参数和 schema 另有深度/节点上限。工具结果的 JSON wire 表示不超过 256 KiB（包含转义，不能仅按原始文件或 stdout 长度判断）。结果超限时保留该工具的已尝试记录及“操作已经完成、结果过大、禁止重放”说明，立即停止同批剩余工具和下一轮模型请求，返回 `requireshumaninput`。此边界适用于普通 CLI、兼容 `/api/chat` JSON/完成后 SSE、渠道/调度调用和持久流式入口，与是否存在进度传输对象无关；不能将输出失败当成副作用未发生。失败/超时另遵守[工具效果边界](tool-failure-effects.md)：未知效果停止剩余批次和模型轮次，只有受信本地纯工具的无效果错误可反馈模型；不引入通用外部效果确定性或 durable 恢复。原生模型请求和响应各不超过 2 MiB，每次模型请求最长 60 秒，连接阶段最长 10 秒。端点必须是 HTTPS 或字面量 loopback HTTP，禁止 URL 凭证、query 和 fragment；客户端不跟随重定向、不自动重试。

模型的文本内容作为文本返回。Brokerrouter 路径不会将正文中的 Markdown `tool` 代码块作为指令执行。`content: null` 的原生工具消息会保留在本轮模型上下文中，工具结果使用 `role: "tool"`，不会伪装成用户消息。

## 故障与恢复

应用不对失败的网关请求自动重试。上游 HTTP 错误正文不会回传给客户端或写入日志，避免把上游回显的凭证与请求内容泄漏出去。应用错误与真实供应商的费用状态是不同事实：连接断开或返回错误时，不得假定供应商没有执行。

首批工具尚未执行时，协议或授权失败返回 HTTP 错误。一旦本轮已有工具执行，后续网关失败或非法工具批次会停止循环，以 HTTP 200 返回 `requireshumaninput`、已执行工具记录和包含尝试次数、工具名的中断说明；不会派发剩余工具或掩盖此前已发生的副作用。提供 `session_id` 时，中断说明进入 SQLite 的用户可见历史，便于重新打开会话核对。该历史只保存消息，没有将工具记录转换为可恢复的执行日志。

SQLite 当前持久化用户可见的会话消息；它没有保存此工具循环的全部模型操作身份、外部副作用凭据与恢复状态。进程中断后，不承诺恢复未完成的工具循环或自动重放写操作。生产使用外部写入，需要先接通 StateKnot durable admission、driver、store 和网关结果核对合同；[Brokerrouter #41](https://github.com/StateKnot/Brokerrouter/issues/41) 的原生 JSON Schema 输出仍是该组合的缺口。

## 验收方法

构建真实二进制后运行：

```sh
cargo build --locked -p jiaclaw-host
python3 tests/native_tools.py target/debug/jiaclaw
python3 tests/native_result_limits.py target/debug/jiaclaw
python3 tests/mcp.py target/debug/jiaclaw
```

`native_tools.py` 启动本地 HTTP 网关 fixture 和 JiaClaw 服务，检查原生多工具往返、ID 对应、白名单、整批拒绝无副作用、畸形模型响应、文本不执行以及 HTTP 错误脱敏。它还覆盖写文件后下一次模型调用失败、后续非法批次、跨轮重复 ID、最后一轮预算耗尽，验证已完成记录和文件保留、剩余工具不执行、中断说明可从会话读取。`mcp.py` 同时经过真实 StateKnot HTTP MCP 客户端、原生工具模型往返、CLI/HTTP 入口与 SQLite 会话存储。两者不使用真实付费供应商凭证。

上游版本、未解决议题与生产验收界限见 [Brokerrouter 消费方状态](brokerrouter-gaps.md) 和 [StateKnot 消费方状态](stateknot-gaps.md)。

`native_result_limits.py` 通过真实 HTTP/普通 CLI 和文件工具验证小输出往返、结果转义后超限、同批停止、下一轮停止、兼容 SSE 及历史保留。Linux CI 另以显式 `JIACLAW_TEST_DOCKER`/`JIACLAW_TEST_EXEC_IMAGE` 运行 `--exec`，使用预拉取的固定摘要镜像和真实沙箱追加效果；缺少所选依赖即失败，不跳过或改用宿主 shell。四平台候选二进制均执行文件模式。这是应用结果丢失边界验收，不是供应商、通用工具错误确定性或恢复认证。
