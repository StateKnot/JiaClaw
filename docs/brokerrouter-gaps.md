# Brokerrouter 消费方状态

核对日期：2026-10-03；private 仓库 main：`e01ecb94919d992eb0b74b3db00d70742820b4cc`。以下内容基于有权限读取的 README、`docs/jiaclaw-consumer-guide.md`、`docs/tool-roundtrip-certification.md`。私有源码没有复制到 JiaClaw；上游链接仅有权限用户可访问。

| 能力 | 上游当前状态 | JiaClaw 状态 |
|---|---|---|
| 文本 Chat Completions | 已支持 | BrokerrouterProvider 接入有界异步非流式请求，无重定向/自动重试 |
| SSE | 已有协议支持，受端点能力与护栏限制；资源修复在未合并 PR #40 | 尚未接通逐事件读取；现有 API SSE 是完成后分块 |
| embeddings | 已有网关契约 | 已接入显式刷新、私有 SQLite 索引、内容新鲜度校验与持久化调用账本；供应商检索质量待授权验收，见[语义记忆](semantic-memory.md) |
| 个人配置 `init-personal` | 已实现事务化初始化 | 可按上游消费者指南接入自己的网关 |
| 工具调用 | 端点能力控制；fixture 已认证 | 已接原生 tools/tool_calls/role:tool 和调用 ID 关联，见[合同与验收](native-tools.md)；缺真实供应商生产默认 |
| 多端点路由/降级 | 最多 3 个候选；仅已证明 not_sent 可换端点 | 已接管理员任务来源→逻辑模型策略，整轮固定；端点降级归网关，真实联合认证待完成，见[模型路由](model-routing.md) |
| 多模态 chat content | 明确拒绝 | JiaClaw 当前文本契约 |
| 媒体任务 | 已有独立子系统，认证仍待完成 | 图片/语音尚未接线 |

## 上游议题

- [#28 消费方指南](https://github.com/StateKnot/Brokerrouter/issues/28)：已关闭。
- [#29 SSE](https://github.com/StateKnot/Brokerrouter/issues/29)：已关闭。不能再将 JiaClaw 自身流式接线列成上游不支持。
- [#30 个人配置](https://github.com/StateKnot/Brokerrouter/issues/30)：已关闭。
- [#31 真实工具闭环认证](https://github.com/StateKnot/Brokerrouter/issues/31)：仍开放。当前没有上游能标记为 JiaClaw 生产默认的真实供应商工具路径；继续沿用该议题，不重复提交。
- [#41 StateKnot durable 原生 JSON Schema 输出](https://github.com/StateKnot/Brokerrouter/issues/41)：已提交。现有 StateKnot `ProviderNativeAgentGraph` 要求模型原生 JSON Schema 最终输出，而网关消费者合同拒绝 `response_format` / Responses。需要有界、能力控制、保留治理/幂等/结算的原生 schema 路径；不能用提示词或工具模拟最终 JSON 冒充该合同。
- [PR #40 MCP 治理恢复与 SSE 资源限制](https://github.com/StateKnot/Brokerrouter/pull/40)：draft、尚未合并，核对的 head 为 `7a7afea0244828851118ba32d1cf37d906a3f388`，base 为本文 main。修复完成的 MCP 结果重新授权、semantic worker、发现刷新后的恢复，以及 SSE 慢客户端缓冲和连接结束前提前释放容量。不能将修复描述为主线已交付，也不重复报已有 PR 覆盖的问题。

2026-10-02 已通过 GitHub API 重新读取 main、issues、PR 和检查状态；main 仍为上述 SHA，#31/#41 仍 OPEN，#41 无回复。PR #40 的 6 个 CI 状态为 FAILURE；抽查 Rust 检查注释明确为 GitHub 账户付款/额度导致作业没有启动，不是测试执行后失败。当前不把 SSE 资源边界、MCP 治理恢复或真实供应商默认标记为生产验收完成。此次核对尚无 Brokerrouter GitHub release。

2026-10-03 回访核对：main、#31/#41、PR #40 的提交和失败状态未变化，仍无 Release。本次未重跑上游或重复提交 issue；上述 CI 注释原因保留为前次检查证据。

## 消费合同与下一步

`provider.provider_type="brokerrouter"`，`base_url` 是自己运行的网关，`model` 为授权逻辑模型，`JIACLAW_API_KEY` 是有限预算的虚拟 Key。JiaClaw 每次请求生成 Idempotency-Key，不启用 SDK 自动重试。完整合同见 [上游消费者指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)。

目前聊天模型调用的操作 ID 与 request ID 没有持久化；新接入的 embeddings 独立持久化操作身份与已知请求 ID，不能据此推断聊天或工具循环已具备同样的恢复能力。下游断开或 `submission_unknown` 后，不能以新键自动重发，也不能假设计费未发生。durable 集成需要保存原操作身份、使用网关状态/结果恢复，最终 `[DONE]` 才能确认 SSE 已持久结算。

接入 StateKnot 现成 durable graph 还需解决 #41 的原生输出合同。已核对固定版本代码和文档：发布版 OpenAI adapter 使用 Responses；另写应用层 Chat `Model` adapter 仍需要网关允许原生 schema。该议题的证据是合同检查与脱敏请求，没有声称执行过真实收费供应商请求。

工具生产默认需要在固定供应商/地域/模型版本/网关提交上，完成原样 assistant.tool_calls → tool message → 最终回答两轮调用，并核对 usage、人民币账本、预留归零、幂等重放。矩阵见 [上游认证证据](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/tool-roundtrip-certification.md)。没有真实供应商凭证与该证据时，不能以 mock 测试关闭 #31。

真正流式仍属于 JiaClaw 的待实现适配任务；语义记忆已接入 embeddings 契约，已完成本机应用与进程恢复验收，跨平台以本批最终提交 CI 为准，真实供应商检索质量另行验收。遇到具体上游契约缺陷时，提交含固定版本、脱敏重现与验收要求的新 issue；不要重复提交已经完成的能力。

任务路由仅改变提交前选定的逻辑 `model` 与允许的采样/输出上限，不增加 `route`、`provider`、`endpoint` 或 `fallback` 请求字段。网关内部 attempt 的 `not_sent` 和消费方返回的 `not_submitted` 不是同一个状态；后者的允许重试仍要求原正文和原幂等键，换模型改变正文。JiaClaw 当前不自动重发任何模型请求，不能将 fixture 中的错误停止解释为网关端点降级或真实供应商认证。
