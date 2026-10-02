# Brokerrouter 消费方状态

核对日期：2026-10-02；private 仓库 main：`e01ecb94919d992eb0b74b3db00d70742820b4cc`。以下内容基于有权限读取的 README、`docs/jiaclaw-consumer-guide.md`、`docs/tool-roundtrip-certification.md`。私有源码没有复制到 JiaClaw；上游链接仅有权限用户可访问。

| 能力 | 上游当前状态 | JiaClaw 状态 |
|---|---|---|
| 文本 Chat Completions | 已支持 | BrokerrouterProvider 接入非流式请求 |
| SSE | 已支持，受端点能力与护栏限制 | 尚未接通逐事件读取；现有 API SSE 是完成后分块 |
| embeddings | 已有网关契约 | 尚未实现 embedding 适配与向量索引 |
| 个人配置 `init-personal` | 已实现事务化初始化 | 可按上游消费者指南接入自己的网关 |
| 工具调用 | 端点能力控制；fixture 已认证 | 本地协议测试通过，缺真实供应商生产默认 |
| 多端点路由/降级 | 最多 3 个候选；仅已证明 not_sent 可换端点 | 不另做盲目供应商重试 |
| 多模态 chat content | 明确拒绝 | JiaClaw 当前文本契约 |
| 媒体任务 | 已有独立子系统，认证仍待完成 | 图片/语音尚未接线 |

## 上游议题

- [#28 消费方指南](https://github.com/StateKnot/Brokerrouter/issues/28)：已关闭。
- [#29 SSE](https://github.com/StateKnot/Brokerrouter/issues/29)：已关闭。不能再将 JiaClaw 自身流式接线列成上游不支持。
- [#30 个人配置](https://github.com/StateKnot/Brokerrouter/issues/30)：已关闭。
- [#31 真实工具闭环认证](https://github.com/StateKnot/Brokerrouter/issues/31)：仍开放。当前没有上游能标记为 JiaClaw 生产默认的真实供应商工具路径；继续沿用该议题，不重复提交。
- [#41 StateKnot durable 原生 JSON Schema 输出](https://github.com/StateKnot/Brokerrouter/issues/41)：本轮新增。现有 StateKnot `ProviderNativeAgentGraph` 要求模型原生 JSON Schema 最终输出，而网关消费者合同拒绝 `response_format` / Responses。需要有界、能力控制、保留治理/幂等/结算的原生 schema 路径；不能用提示词或工具模拟最终 JSON 冒充该合同。

## 消费合同与下一步

`provider.provider_type="brokerrouter"`，`base_url` 是自己运行的网关，`model` 为授权逻辑模型，`JIACLAW_API_KEY` 是有限预算的虚拟 Key。JiaClaw 每次请求生成 Idempotency-Key，不启用 SDK 自动重试。完整合同见 [上游消费者指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)。

目前每次模型调用的操作 ID 与 request ID 没有持久化。下游断开或 `submission_unknown` 后，不能以新键自动重发，也不能假设计费未发生。durable 集成需要保存原操作身份、使用网关状态/结果恢复，最终 `[DONE]` 才能确认 SSE 已持久结算。

接入 StateKnot 现成 durable graph 还需解决 #41 的原生输出合同。已核对固定版本代码和文档：发布版 OpenAI adapter 使用 Responses；另写应用层 Chat `Model` adapter 仍需要网关允许原生 schema。该议题的证据是合同检查与脱敏请求，没有声称执行过真实收费供应商请求。

工具生产默认需要在固定供应商/地域/模型版本/网关提交上，完成原样 assistant.tool_calls → tool message → 最终回答两轮调用，并核对 usage、人民币账本、预留归零、幂等重放。矩阵见 [上游认证证据](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/tool-roundtrip-certification.md)。没有真实供应商凭证与该证据时，不能以 mock 测试关闭 #31。

真正流式、语义记忆现在属于 JiaClaw 的待实现适配任务。遇到具体上游契约缺陷时，提交含固定版本、脱敏重现与验收要求的新 issue；不要重复提交已经完成的能力。
