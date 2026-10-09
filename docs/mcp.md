# StateKnot HTTP MCP

使用精确锁定的 `stateknot-integrations = 0.1.0-alpha.1`，协议 `2026-07-28`，Rust 1.88.0。实现的是无状态 Streamable HTTP Tool 客户端；服务器必须支持该契约。未声明兼容旧版 MCP、stdio 或所有 MCP 服务器。上游实现与生产支持边界见 [StateKnot](stateknot-gaps.md)。

默认无服务器。每个绑定由实例管理员审查，只开放明确无外部写入副作用的工具。远程 `readOnlyHint`、描述、instructions 不构成授权；`effect = "read_only"` 是管理员对已审查能力的分类，客户端无法证明服务器内部没有写操作。使用可审计的服务与最小权限凭证。外部写入须接入 StateKnot durable admission/reconciliation 后再开放；当前配置拒绝 `write` 等 effect。

## 审查并绑定工具

先使用只读发现命令导出完整描述与 RFC 8785 SHA-256，命令不会调用或批准工具。Bearer 只接受环境变量引用，不接受命令行明文 token；不要把导出结果直接转成自动批准。

```sh
jiaclaw mcp-inspect --endpoint https://mcp.example.com/mcp/ \
  --bearer-token-env INVENTORY_MCP_TOKEN > inventory-review.json
```

审查服务所有者、input/output schema、功能与副作用、权限范围、`x-mcp-header` 扩展以及完整描述中其他字段。留存审查 JSON 与对应软件版本，将已审查条目的 `descriptor_sha256` 放入配置。服务器在运行中改变实际行为无法靠摘要证明安全，管理员仍须控制其部署与凭证。已审查描述变更时需要重新审查并重启，不自动追随 discovery。

在 TOML 配置顶层加入：

```toml
[[mcp.servers]]
name = "inventory"
endpoint = "https://mcp.example.com/mcp/"
bearer_token_env = "INVENTORY_MCP_TOKEN"
timeout_secs = 30
max_response_bytes = 262144
max_concurrent_calls = 4

[[mcp.servers.tools]]
name = "lookup"
alias = "lookup"
descriptor_sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
effect = "read_only"
```

全零摘要仅为占位，必须换成实际审查值；否则启动失败。JSON 配置使用相同顶层 `mcp.servers` 数组。工具以 `mcp_inventory_lookup` 注册；remote name 与本地 alias 独立。`JiaClawAgent::connect(config).await` 为库调用入口；同步 `new` 拒绝有 MCP 的配置，避免配置被忽略。`serve`、`chat` 与 `doctor` 使用异步入口。

`serve` 启用 MCP 时，即使监听 loopback 也必须设置 API Token。它只保护实例 API；各渠道仍按自己的签名认证，共享同一实例工作区和获批工具。无多用户权限模型，不应将个人实例分享给不同权限的用户。

## 固定资源与失败合同

| 配置或边界 | 策略 |
|---|---|
| 服务器 / allowlist | 最多 8 个服务器，每个 1..32 个工具；发现不会增加权限 |
| name / alias | ASCII 字母、数字、下划线，各 1..24 字节；服务器名/别名/remote name 不可重复 |
| endpoint | HTTPS，或字面量 `127.0.0.1` / `[::1]` 的 HTTP；不接受 `localhost`、URL 凭证、query、fragment、redirect |
| Bearer | 可选；变量名为大写字母/数字/下划线，以字母开始；指定却缺失/无效则失败，不降级匿名 |
| `timeout_secs` | 默认 30，1..120；覆盖每个服务器的总启动过程；单次调用以一个原始截止时刻覆盖参数校验、HTTP 和结果校验/编码，各阶段不重置预算 |
| `max_response_bytes` | 默认 256 KiB，1 KiB..1 MiB；JSON body、SSE 行/事件/总字节均受同一上限 |
| `max_concurrent_calls` | 默认 4，1..16；同一服务器所有工具共享，满载立即返回未提交，不排无界队列 |
| Schema worker | 所有 Agent/服务器及原生工具前置校验共享进程内四槽；描述摘要、编译、参数/结果校验与结果编码转到阻塞池，满载立即拒绝；已准入但等待 Tokio 阻塞线程的任务也计入四槽 |
| 发现 | 最多 4 页、128 个 catalog entries，每个响应最多 32 个通知；失败不注册部分工具 |
| schema / 参数 | schema 最多 32 KiB；参数最多 16 KiB；深度 24、节点 2048；schema 本地编译，不读取外部文件/网络引用；正则回溯最多 10000，编译/DFA 各 256 KiB |
| 描述与输出 | 描述最多 4 KiB且由完整 pin 固定；只支持 text content 与可选 structuredContent；有 output schema 时必须匹配 |
| 调用 | StateKnot `call_tool_once`；单次 HTTP 提交，无协议/鉴权/网络自动重放 |

服务初始化以客户端执行身份使用同一凭证进行 discovery/list/call；单租户固定绑定，不实现 OAuth 自动授权、凭证按用户切换或 MRTR 人工审批。遇到 `input_required` 返回明确错误，不提供 roots、sampling、elicitation 或 requestState 恢复。遇到图像/resource 内容拒绝，不隐式抓取外部资源。

工具错误标记为失败，错误消息不包含远程错误 body、token 或未知 requestState；worker panic 不将其 payload 拼入公开错误。工具结果仍为不可信数据。Schema 编译和校验在阻塞 worker 中执行，async 线程可以继续处理定时器与 I/O。超时/取消只停止等待，不能强制终止已经运行的 CPU 工作；worker 到实际结束才释放进程四槽。参数/结果 worker 同时持有服务器并发许可，等待者取消不提前释放，也不为同一服务器制造额外校验容量。此处提供有界准入和等待期限，未宣称 CPU 进程沙箱或任意 Schema 的硬执行时限；管理员仍须审查 Schema，结构/大小/离线引用与正则预算继续生效。

参数校验满载或到期明确返回“未提交”；取消后的纯校验 worker 没有 HTTP 提交能力。HTTP 阶段到期返回“远程结果未知”，取消丢弃 HTTP future 并释放该阶段的本地服务器许可，已收到请求的远程计算无法撤销。完整响应到达后的结果校验满载/到期返回“远程调用已完成，但输出未接受”。任何阶段均不自动重试或恢复请求；这是当前只开放只读工具的原因。启动 worker 到期或取消不发布部分注册表，迟到的编译结果不会注册工具。实际 Brokerrouter 聊天路径的定义编译/整批预检也接入同一容量，合同见[原生工具](native-tools.md#工具授权与执行边界)。

## 验收与升级

`cargo test -p jiaclaw mcp::tests --locked` 通过 StateKnot 实际 HTTP 传输测试 JSON、分片 SSE、descriptor drift、schema 违规、外部 `$ref`、未批准工具、结果超限、401/redirect/id 不匹配、MRTR、期限、满载和取消。单 async 线程/单阻塞线程 fixture 还验证真实运行及已准入排队 worker 的超时/取消所有权、四槽耗尽拒绝、输入取消后零 POST、输出取消后不重放、初始化不部分发布、panic 释放与原始总期限。`tests/mcp.py` 启动独立 HTTP MCP 和网关 fixture，实际运行二进制的 inspect、CLI chat、serve、模型工具循环和 SQLite 历史，校验 Bearer 与实例鉴权前置检查。Linux/macOS CI 与 Release 构建都执行该链路；本批固定提交证据见[验证记录](validation.md#mcp-schema-worker-所有权与总期限)。

这些测试证明应用适配合同，不代替每个外部服务器、真实模型供应商以及 durable 工具写入的独立生产验收。stdio 缺口跟踪 [StateKnot #140](https://github.com/StateKnot/StateKnot/issues/140)；网关真实工具供应商认证跟踪 [Brokerrouter #31](https://github.com/StateKnot/Brokerrouter/issues/31)。升级 StateKnot 需显式改版本/锁文件并重复协议验收，不能追踪浮动 main。
