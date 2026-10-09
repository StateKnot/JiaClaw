# Brokerrouter 消费方状态

2026-10-09 16:10 UTC 官方API再次固定 StateKnot main `aa11b4f44a948aaf2e2baba4c30a297dc828ce6d`（比较identical、ahead0）、唯一alpha.1与#140 OPEN/无回复；Brokerrouter main `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无release、#31 OPEN/一回复、#41 OPEN/无回复、PR40仍未合并/head `7a7afea0244828851118ba32d1cf37d906a3f388`。未出现新消费合同，不重复建issue或浮动升级HTTP MCP。PR97先前6469最终七项CI/四优化候选已[回填](validation.md#web-原请求流式最终-ci-回填)；本批持久原请求目录/遗失编号恢复属于应用接线，不加载正文、不派发，不替代StateKnot durable、stdio/外部写入、原生Schema或供应商认证。

2026-10-09 13:48 UTC 再次从官方 API 固定 StateKnot main `aa11b4f44a948aaf2e2baba4c30a297dc828ce6d`、alpha.1/#140（OPEN/无回复），Brokerrouter main `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无 release/#31（OPEN/一回复）/#41（OPEN/无回复）/未合并 draft PR40 head `7a7afea0244828851118ba32d1cf37d906a3f388`，均未变化。PR96最终七项CI及四平台候选已逐项回填；本批[Web流式](web-streaming.md)继续实际接线原身份、授权、取消和核对，属于应用能力，不重复提交框架缺陷。租户、真实供应商/代理、stdio/外部写入和 durable 仍保留独立门槛。

2026-10-09 12:10 UTC 本轮再次读取当前main/release/issues：StateKnot `aa11b4f44a948aaf2e2baba4c30a297dc828ce6d`、alpha.1/#140和Brokerrouter `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无release/#31/#41/未合并PR40均未变化。新[HTTP正文流式](http-streaming.md)复用既有Brokerrouter SSE，属于应用真实接线，不增加框架资格或另报重复issue；Web/租户/代理/供应商与durable门槛分别保留。

2026-10-09 本批 main `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无 release、#31/#41 与未合并 draft #40 的 head `7a7afea0244828851118ba32d1cf37d906a3f388` 未变。实际新增[HTTP 请求身份/核对](http-turns.md)，调用现成 SSE 收据和同一原生授权循环，已准入 UUID 贯穿模型账本本地 turn_id；不伪造 governed turn header、不重发未知模型。当前 HTTP 出站是 JSON，Web/token delivery 和租户协议仍未接通；上游 slow-consumer、真实供应商与 durable 原生输出资格分别保留，不重复提交 issues。

2026-10-09 本轮再次核对 main `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无 release、#31（OPEN/一回复）、#41（OPEN/无回复）与未合并 draft #40（head `7a7afea0244828851118ba32d1cf37d906a3f388`），均没有新消费合同。main/PR40 十二项 FAILURE 的官方 annotations 仍明确因付款/额度未启动；同批 runner 标签升级警告不是代码失败。没有把应用 HTTP 会话提前释放锁归因于网关。单次 CLI SSE 的 PR #93 已通过最终七项 CI 和四平台候选安装，见[回填](validation.md#单次-cli-真流式最终-ci-回填)；HTTP/Web 仍待持久请求身份、有限发送及完整取消/停机接线，本批只修实际数据库取消边界。

2026-10-09 06:25 UTC 官方 main/no-release/#31/#41/PR #40 均未变化；本批实际接线[CLI SSE](cli-streaming.md)，使用原始流式正文/模式账本、有界逐事件预览、完整收据和取消后单次结算，不复制上游内部实现。HTTP/Web/租户尚未流式，上游 PR #40 与真实端点认证仍开放，不重复报已有议题。完整候选与进程验收见[验证记录](validation.md)。

2026-10-09 05:17 UTC 本批官方 main/no-release/#31/#41/PR #40 提交与检查未变；已有付款/额度未启动的失败证据没有新变化。MCP Schema worker 修复属于 JiaClaw 应用生命周期，单次 HTTP、只读授权、原生工具循环和网关未知 hold 不扩权；不重复提交 issue，生产流式、durable 原生输出与真实供应商资格仍开放。

2026-10-09 再核对 main/no-release/#31/#41/PR #40 固定合同未变，不重复提交 issue。[组织迁移与最终 CI 回填](organization-migration.md)记录实际安装接线；未知请求、流式资源与真实供应商认证边界仍保留。

核对时间：2026-10-08 15:36 UTC；private 仓库 main：`e01ecb94919d992eb0b74b3db00d70742820b4cc`。以下内容基于有权限读取的 README、`docs/jiaclaw-consumer-guide.md`、`docs/tool-roundtrip-certification.md`。私有源码没有复制到 JiaClaw；上游链接仅有权限用户可访问。

| 能力 | 上游当前状态 | JiaClaw 状态 |
|---|---|---|
| 文本 Chat Completions | 已支持 | BrokerrouterProvider 接入有界异步非流式请求，无重定向/自动重试 |
| SSE | 已有协议支持，受端点能力与护栏限制；资源修复在未合并 PR #40 | 单次 CLI 和持久 HTTP 已实际接通逐事件读取与收据/取消边界，PR #93/#96 最终七项 CI 通过；旧 `/api/chat` 仍保留兼容分块，Web 本批接线，租户与联合认证另计 |
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

2026-10-08 再次通过 GitHub 官方 API 核对：main、#31/#41 和未合并 draft PR #40 的提交均未变化；#31 只有原有一条认证进度回复，#41 仍无回复，仍无 Release。main 与 PR #40 当前均有六个 FAILURE 检查；重新读取 PR #40 Rust 检查注释，作业仍因账户付款/额度未启动，不能解释为已执行的代码测试失败。本批不重复报 issue、不把未认证合同标为完成。

2026-10-08 06:32 UTC Discord 批次再次核对，上述 main、release、issues 和 draft PR head 均未变化，PR #40 Rust check `107571112146` 注释仍明确作业未启动。StateKnot 新源码 JWT/JWKS 身份没有改变这里的模型消费合同；当前没有新增可消费的 durable 原生 JSON Schema 路径。Discord 复用受限 channel 逻辑模型，不新增供应商认证或媒体身份能力。

08:23 UTC 飞书批次再次读取官方 API：本文 main、无 release、#31/#41 与 PR #40 head 均未变化；PR #40 check `107571112146` 的 failure annotation 仍明确账户付款/额度导致作业未开始。没有新的 durable 原生输出合同；飞书复用独立后端 channel 模型，不认证供应商、路由降级或媒体身份，不重复提交 issue。

11:35 UTC 企业微信启动校验批次复核上述 main、无 release、#31/#41 与 PR #40 head，均未变化。此次逐项读取 main 六项和 PR #40 六项 FAILURE 的全部 annotations：十二项均明确因账户付款/额度未启动，没有未分类失败或已执行测试失败的证据。不能因此宣称 SSE 修复已验收，也不能误报为代码测试失败。StateKnot #148 的 typed Tool Schema 方向修复已合并但未发布，不改变网关拒绝原生最终输出 Schema 的合同；#41 仍 OPEN、无回复。JiaClaw 本批复用既有 standalone channel 模型，只新增企业微信官方 token/应用身份/显式成员可见范围的启动校验，不新增模型、媒体或真实供应商资格。

本批默认并行 Rust 1105 项、企业微信启动六组/32 负例，以及同一冻结二进制的 WeCom/MCP/e2e/channels/scheduled_delivery 五套本地回归全部通过，保留未知发送和持久额度语义；没有真实供应商请求或上游模型合同变更，详见[验证记录](validation.md#企业微信启动校验批次)。后续已核对 [PR #86](https://github.com/StateKnot/JiaClaw/pull/86) 固定 head `db802c46b8726e2dfbaf9defb1eebddb043619c7` 的 [CI 37776140027](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027)，Ubuntu/macOS/container 全部成功；不据此关闭 #31/#41 或认证运行期 token 刷新、真实平台和容器渠道 runtime。

## 消费合同与下一步

`provider.provider_type="brokerrouter"`，`base_url` 是自己运行的网关，`model` 为授权逻辑模型，`JIACLAW_API_KEY` 是有限预算的虚拟 Key。JiaClaw 每次请求生成 Idempotency-Key，不启用 SDK 自动重试。完整合同见 [上游消费者指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)。

本批已为显式启用的聊天补全/摘要接入独立模型调用账本：发送前持久操作身份、保存经过校验的收据，并对已知远端 UUID 提供仅 GET 核对；最终二进制 7 组本机验收通过，跨平台及容器验收以最终 PR 当前 head 的 checks 为准，见[模型调用收据](model-calls.md)。embeddings 继续使用独立账本，两者都不能恢复工具循环。下游断开或 `submission_unknown` 后，不能以新键自动重发，也不能假设计费未发生。durable 集成需要保存原操作身份、使用网关状态/结果恢复，最终 `[DONE]` 才能确认 SSE 已持久结算。

接入 StateKnot 现成 durable graph 还需解决 #41 的原生输出合同。已核对固定版本代码和文档：发布版 OpenAI adapter 使用 Responses；另写应用层 Chat `Model` adapter 仍需要网关允许原生 schema。该议题的证据是合同检查与脱敏请求，没有声称执行过真实收费供应商请求。

工具生产默认需要在固定供应商/地域/模型版本/网关提交上，完成原样 assistant.tool_calls → tool message → 最终回答两轮调用，并核对 usage、人民币账本、预留归零、幂等重放。矩阵见 [上游认证证据](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/tool-roundtrip-certification.md)。没有真实供应商凭证与该证据时，不能以 mock 测试关闭 #31。

真正流式仍属于 JiaClaw 的待实现适配任务；语义记忆已接入 embeddings 契约，已完成本机应用与进程恢复验收，该批 #70 的最终 CI 已通过，真实供应商检索质量另行验收。遇到具体上游契约缺陷时，提交含固定版本、脱敏重现与验收要求的新 issue；不要重复提交已经完成的能力。

任务路由仅改变提交前选定的逻辑 `model` 与允许的采样/输出上限，不增加 `route`、`provider`、`endpoint` 或 `fallback` 请求字段。网关内部 attempt 的 `not_sent` 和消费方返回的 `not_submitted` 不是同一个状态；后者的允许重试仍要求原正文和原幂等键，换模型改变正文。JiaClaw 当前不自动重发任何模型请求，不能将 fixture 中的错误停止解释为网关端点降级或真实供应商认证。

## 媒体消费者边界

在上述固定 main，`media-jobs-v1` 仅支持 `video.generate` 文生视频；输入为文本 prompt/negative_prompt，实际分辨率、宽高比和 2–15 秒时长组合由授权 capabilities 决定。私有资产上传虽然已经交付，目前没有图片、音频或视频生成输入接线，多模态 chat content 仍拒绝。详情见[媒体作业](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/m4-media-jobs.md)和[私有输入资产](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/m4-assets-pricing.md)。

后续 M4/M5 包已交付扫描后受控 MP4 下载，不能再把历史 M4.2 的“无下载接口”当作当前能力。但生成终态 `output_pending` 不代表可交付：作业创建时必须绑定独立受信控制面建立的 turn，最长一小时、不可补绑或续期；产物经当前扫描/策略及必要真人审批后，逐块重新授权并验证整文件 SHA-256。JiaClaw 尚无这一身份链，媒体接线不在本批范围。[交付合同](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/m4-output-delivery.md)、[隔离 MP4 审阅](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/m5-isolated-media-review.md)。

媒体创建须持久保存原幂等键、正文和 turn 绑定；已提交后的取消只是意图，不保证供应商停止或退款。未知提交不能重新生成，模型费用、检测费用及存储/流量费用分别核对。固定连接器、真实费用与产物源、S3、IdP 和检测质量仍在上游 LIVE-10/11/12/13/20 发布闸门内；离线 fixture 只能证明消费者协议和恢复行为，不能代替这些认证。[上游验收清单](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/acceptance-backlog.md)。

本轮独立用户 Telegram 复用既有受限 channel 逻辑模型和专属后端虚拟 Key，不新增网关模型合同，也不把应用的持久 inbox/outbox 解释为工具循环恢复。main/#31/#41/PR #40 本次核对未变化；没有据此新增上游 issue。范围见[独立用户 Telegram](tenant-telegram.md)。

12:56 UTC 独立用户企业微信预检重新读取固定 main、无 release、#31/#41 与 draft PR #40 及全部十二项失败 annotations，状态没有变化：十二项均因账户付款/额度未启动，没有已执行代码失败或未分类 annotation。StateKnot #152 的执行状态读取加严已合并但未发布，不改变 Brokerrouter 原生最终输出 Schema 合同；#41 仍 OPEN、无回复。新企业微信 protocol 5 继续使用现有原生 Brokerrouter channel 工具循环及应用原请求 metadata，不增加供应商资格、durable graph、自动换模型或未知重放。本批固定artifact的默认并行Rust1158项与真实双后端十组全部通过；提交准备时旧21套回归仍独立运行、最终CI pending，以本批draft PR固定head为准，不能用PR #86启动门槛或新本地fixture替代真实供应商/完整跨平台交付。

14:39 UTC 钉钉批次固定官方观察：main/no-release/#31/#41/PR#40未变化；重新读取main与PR40全部十二项失败annotations，十二项均付款/额度导致未启动，非执行代码失败。StateKnot #153没有改变Brokerrouter原生输出Schema准入。PR #87固定head三项CI已成功，完整结果见[验证记录](validation.md#独立用户企业微信最终-ci-回填)；本批standalone钉钉JSON/MIME与含糊回执修复仍用现有原生工具循环，不替代真实供应商资格、durable driver、流式或多模态验收，不新增重复issue。

15:36 UTC 复制批次重新读取官方固定 main、no-release、#31/#41 与 PR #40；均未变化，PR40 head仍为 `7a7afea0244828851118ba32d1cf37d906a3f388`。main与PR40十二条失败 annotation 全部仍为付款/额度导致未启动，不是执行失败；没有新增或重复 issue。PR #58–#88当前head CI均成功，见[PR88最终回填](validation.md#钉钉原始报文最终-ci-回填)。本批修复 copy 自身 native Schema 与共享文件 I/O 的接线；网关既有原生工具往返、有限调用和单次未知 hold 合同未扩大，真实供应商/原生最终Schema/durable/SSE资源认证仍分别开放。
