# 按任务用途选择逻辑模型

JiaClaw 可在模型调用前，为聊天、渠道、定时任务、HEARTBEAT 和会话摘要选择不同的 Brokerrouter 逻辑模型。映射来自管理员配置，入口决定用途；请求正文、任务提示词、工具结果和模型输出不能选择路由。默认关闭，未配置的用途继承 `[provider]`。

这项能力不需要 StateKnot durable 才能使用，但也不提供模型操作的持久化恢复。端点降级由 Brokerrouter 处理，JiaClaw 不增加应用层自动重试。

## 配置

只支持 `provider.provider_type = "brokerrouter"`。下面的模型 ID 必须替换成自己网关中已授权并具备所需工具能力的逻辑模型；同一网关 URL 和虚拟 Key 用于所有用途。

```toml
[provider]
provider_type = "brokerrouter"
base_url = "https://YOUR_GATEWAY"
model = "YOUR_DEFAULT_LOGICAL_MODEL"
temperature = 0.7
max_tokens = 4096

[routing.chat]
model = "YOUR_CHAT_LOGICAL_MODEL"
temperature = 0.7
max_tokens = 4096

[routing.channel]
model = "YOUR_CHANNEL_LOGICAL_MODEL"
max_tokens = 2048

[routing.scheduled]
model = "YOUR_SCHEDULED_LOGICAL_MODEL"
temperature = 0.2
max_tokens = 1024

[routing.heartbeat]
model = "YOUR_HEARTBEAT_LOGICAL_MODEL"
max_tokens = 512

[routing.summary]
model = "YOUR_SUMMARY_LOGICAL_MODEL"
max_tokens = 256
```

也支持嵌套的 `[agent.routing]`；如果同时提供顶层 `[routing]`，顶层配置会整体替换嵌套部分，不逐用途合并。

JSON 使用同样的顶层 `routing` 对象；`"routing": {}` 与未配置等价。不能把它放到 `provider` 或 HTTP 请求中。省略不需要覆盖的用途即可；没有对应 `routing.<purpose>` 时，使用默认逻辑模型和参数。

| 字段 | 约束 |
|---|---|
| 用途 | 只接受 `chat`、`channel`、`scheduled`、`heartbeat`、`summary`；未知用途或字段启动失败 |
| `model` | 每个非空用途配置必填；1–200 UTF-8 字节，无首尾空白或控制字符；模型的真实可用性与授权由网关判定 |
| `temperature` | 可选、有限数字，范围 0–2；普通用途省略时继承 provider |
| `max_tokens` | 可选正整数，不得超过 `provider.max_tokens`；省略时继承 provider |
| `provider.max_tokens` | 启用路由时必须为 1–1,000,000；网关仍检查真实模型输出上限 |

摘要特殊处理：默认温度固定为 0.2，显式 `routing.summary.temperature` 可覆盖；输出上限始终取 **512、provider.max_tokens 和摘要用途上限的最小值**，包括未启用路由的配置。摘要调用不携带工具。摘要失败仍按既有会话政策硬截断，不换模型重试。

`max_tokens` 是**每次模型补全的输出上限**，不是整个工具循环的总 token 或花费预算。每个工具循环可能有多个模型请求，还可能有独立摘要请求；`agent.max_tool_iterations`、工具授权和网关虚拟 Key 的人民币预算继续生效。不能以低 `max_tokens` 代替网关预算。

## 哪个入口使用哪个用途

| 入口 | 用途 |
|---|---|
| CLI `chat`、`POST /api/chat`、内置 Web 聊天 | `chat` |
| 通用 `/hooks/inbound` 及 Telegram/Slack/Discord/飞书/企业微信/钉钉消息 worker | `channel` |
| cron/interval 调度任务，包括带渠道通知的任务 | `scheduled` |
| legacy HEARTBEAT 文件轮询 | `heartbeat` |
| 各入口的会话溢出摘要 | `summary` |

通用 webhook 的 `channel` 标签只描述入站来源，不能把用途改成 heartbeat。HTTP 请求中额外的 `model` / `routing` 字段没有选择模型的能力；JobSpec 的未知字段会被拒绝。带通知的定时任务仍使用 `scheduled`，目的地渠道不会改变生成内容的模型。工具目录与授权保持原来的入口限制。 chat/channel/scheduled/heartbeat 仍会携带该入口获授权的原生工具目录；前台空工具列表沿用允许全部已注册工具的语义。因此所选逻辑模型需要具备相应工具能力，即使本轮提示词没有要求调用工具。只有摘要明确不携带工具；不会为了适配模型而删除工具目录。

一次 Agent 轮次在进入工具循环前固定用途、模型、温度和上限；两轮原生工具往返使用相同策略。服务运行中不热加载路由配置，修改后重启。重新启动后尚未执行的队列和未来定时任务使用当前配置；这不是领取前已持久化的路由快照。已在处理但没有完成的工作继续遵守既有中断核对规则，不因改模型自动重跑。

## 响应和可追溯范围

只要配置了至少一个用途，HTTP `ChatResponse` 和现有 SSE 的最终 `done` 数据包含可选字段：

```json
{"routing":{"purpose":"chat","model":"YOUR_CHAT_LOGICAL_MODEL","temperature":0.7,"max_tokens":4096}}
```

它记录本次请求选择的**逻辑模型和参数**，不是供应商实际端点、实际 token 数、账单或逐 token 流式的证明。即使当前用途继承 provider 默认值，也会给出有效值。未配置路由时完全省略该字段，保留旧响应形状。

调度运行中已保存的 ChatResponse 会连同该字段写入 SQLite，可通过 `GET /api/jobs/{id}/runs` 查看并在重启后保留。失败或中断且没有 ChatResponse 的运行不具备这条证据。通用 webhook 的简化回复与具名渠道事件没有新增持久路由审计字段；会话消息本身也不记录路由。当前模型操作 ID、原始网关逻辑请求 ID 与供应商提交结果仍没有形成 durable 恢复链，不能据此声称已解决恢复缺口。

## 降级、权限和失败

依据固定 Brokerrouter `e01ecb94919d992eb0b74b3db00d70742820b4cc` 的[消费者指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)和 [M2 合同](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/m2-text-gateway.md)：

- JiaClaw 只把选定的逻辑 `model` 及已有支持字段发给网关，不发送自造的端点、供应商或 fallback 请求字段。
- 网关筛选授权与能力后，按 priority、端点 ID 排序，固定最多三个候选及价格快照。只有已证明未发送的 `not_sent` attempt 可以自动切到下一个候选。
- 已发送后的超时、断线、非 200（包括 429）、无效或不完整回执都可能是 `submission_unknown`，不能换模型或端点重发，也不能把费用显示为零。
- 消费方的 `not_submitted` 与内部 `not_sent` 不同。前者允许在原操作身份、原正文下恢复；JiaClaw 当前仍不自动重试。改 `model` 就改变正文，不能复用该幂等键。
- 预算不足、鉴权失败、审批要求和模型能力不足不会触发直连、删除 tools、降低审核或自动换模型。更改配置必须先完成授权和故障核对。

真实供应商工具认证仍跟踪 [Brokerrouter #31](https://github.com/StateKnot/Brokerrouter/issues/31)。路由配置不把未认证模型升级为生产默认；不同逻辑模型的能力、区域、隐私与成本需要独立审查。

## 验证

```sh
cargo build --locked -p jiaclaw-host
python3 tests/model_routing.py target/debug/jiaclaw
```

2026-10-03 已使用本批构建的真实 debug 二进制执行上述脚本，两组验收均通过；跨平台结果以本批 PR 的最终提交 CI 为准。

脚本通过实际二进制和本地 HTTP 网关 fixture 检查配置启动拒绝、CLI/HTTP/通用 webhook/调度/HEARTBEAT 的用途选择、参数继承、摘要上限、原生工具循环固定策略及工具执行后模型失败保留原路由和工具记录、请求/提示词不可覆盖配置、未认证请求零模型调用、失败无替代请求、旧响应形状和定时结果重启保留。它不使用真实供应商凭据，不认证供应商端点、网关计费或 StateKnot durable。实际执行结果见[验收记录](validation.md)。
