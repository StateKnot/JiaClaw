# StateKnot 集成状态

2026-10-09 11:31 UTC 再固定 main `aa11b4f44a948aaf2e2baba4c30a297dc828ce6d`，官方十三项 checks 全成功。[#160](https://github.com/StateKnot/StateKnot/pull/160) 增加 graph state/checkpoint 属性与参考模型证据；完整增量没有 crates 生产源码、runtime/integrations 或 lockfile 变化，alpha.1/release/#140 未变。这些属性模型不替代 JiaClaw durable driver 接线或真实恢复资格，继续保留精确 MCP pin 和 Brokerrouter #41 原生输出门槛。

2026-10-09 10:35 UTC 固定 main `1699837c2885c38a686e90c27202610911dc153e`，官方十三项 checks 全成功；[#158](https://github.com/StateKnot/StateKnot/pull/158) 增加 71 个枚举/298 个分支的封闭向量，[#159](https://github.com/StateKnot/StateKnot/pull/159) 增加七个复合预算属性模型与边界控制。相对以下 e58db449 的完整文件差异没有 crates 生产源码、runtime/integrations 或 Cargo.lock 变化；release 仍 alpha.1、#140 OPEN/无回复。保留精确 HTTP MCP pin，不把测试证据增量记为可消费的新稳定合同，不新增重复 issue。

2026-10-09 本批再次固定 main `e58db449939b79b215660704400bf5aa91a39296`：[#157](https://github.com/StateKnot/StateKnot/pull/157) 在 Core 中强制声明对象使用 map reader，拒绝 JSON array 的位置解码；runtime/tool_registration 仅增测试，没有 integrations 生产接线变化。此前最终读取的十三项 main checks 全成功；release 仍 alpha.1、#140 OPEN/无回复。该源码增量不等于稳定生产发布。JiaClaw 保留原 HTTP MCP 精确 pin，不消费新的 typed object reader，不另建重复议题。本批[持久 HTTP 请求](http-turns.md)由应用准入/会话事务与已接线 Brokerrouter 原生循环实现，仍不是 StateKnot durable runtime。

2026-10-09 本轮固定 main 已推进到 `288cfc634574cc314e748f0ebeaa48ca418435ed`，十三项 checks 全部成功；release 仍为 alpha.1，#140 仍 OPEN/无回复。[#156](https://github.com/StateKnot/StateKnot/pull/156) 将时间戳十进制解析的 eager `then_some` 改为 digit 校验后的 lazy `then`，并增加嵌套 JSON/规范时间资源与 fuzz 证据；相对上一 main 的完整文件差异没有 runtime/integrations 生产合同变更。现有精确 HTTP MCP 依赖不消费该时间戳 reader，不据此升级或另建重复 issue。本批[会话取消修复](session-cancellation.md)属于 JiaClaw 的实际 SQLite worker 所有权，不是 durable driver 接线。

2026-10-09 06:25 UTC 已读取 main `9c52b9cd4a69ec4a9537a6f70b5f41cf7e966d3f` 的十四项成功检查、alpha.1发布和 #140（OPEN/无回复），均与前批固定合同相同。本批 CLI 流式只使用 Brokerrouter 现成 SSE，保留原 HTTP MCP精确依赖与整批工具授权，不将预览/模型收据作为 StateKnot durable 或外部写入恢复。后续 durable 原生输出仍跟踪 Brokerrouter #41。

2026-10-09 05:17 UTC 本批 main、alpha.1 与 #140 未变，十四项 main checks 仍成功。HTTP MCP 的编译/校验调度和取消所有权是 JiaClaw 应用接线缺口，已按进程四槽 worker 与原始总期限修订，不升级精确 pin、不新增重复框架 issue；固定提交验收见[验证记录](validation.md#mcp-schema-worker-所有权与总期限)。durable、stdio 与真实外部服务器资格仍分别保留。

2026-10-09 03:16 UTC 再核对 main 为 `9c52b9cd4a69ec4a9537a6f70b5f41cf7e966d3f`，alpha.1/release/#140 不变。[#155](https://github.com/StateKnot/StateKnot/pull/155) 修正 Failure/ToolError/CapabilityLifecycle 六个可选字段的序列化 Schema，并补齐有界 fuzz 与输出 Schema 清单；十四项 main checks 成功。既有输入 pins 与 wire bytes 不变，旧输出 pin 须启动拒绝，后续 typed registry 升级需新 Schema/Tool 版本并保留已准入旧合同，见[固定 RFC-0020](https://github.com/StateKnot/StateKnot/blob/9c52b9cd4a69ec4a9537a6f70b5f41cf7e966d3f/docs/rfcs/0020-core-optional-output-schemas.md)。仍为未发布源码能力；HTTP MCP 客户端与现有 alpha.1 字节相同，83407字节/SHA256 `0d5887abaeba040784707193c5b99106c8b9d197d8239462f688216afcd07974`。应用不调用 typed registry，保留精确 MCP pin，不将此变化误报为当前适配缺陷。[组织迁移与最终 CI 回填](organization-migration.md)保留实际认证范围。

核对时间：2026-10-08 15:36 UTC；上游 main：`83802cb3202bf9cb860c6357a94abc80408b1f88`。证据来自 [Cargo.toml](https://github.com/StateKnot/StateKnot/blob/83802cb3202bf9cb860c6357a94abc80408b1f88/Cargo.toml) 与 [README](https://github.com/StateKnot/StateKnot/blob/83802cb3202bf9cb860c6357a94abc80408b1f88/README.md)。公开 release 仍为 [v0.1.0-alpha.1](https://github.com/StateKnot/StateKnot/releases/tag/v0.1.0-alpha.1)，对应提交 `9f697735b8a0164197bd06697d07d8c63d169b68`；main 的增量不等于已发布依赖。

旧结论“edition 2024 不稳定、crates 没发布”已经过时：上游已发布 `0.1.0-alpha.1`，要求 Rust 1.88+；发布追踪 [#92](https://github.com/StateKnot/StateKnot/issues/92) 已关闭。上游仍声明处于 pre-alpha/evaluation 阶段，没有生产支持承诺。

JiaClaw 已精确锁定 `stateknot-integrations = 0.1.0-alpha.1` 并使用其 HTTP MCP 客户端，工具链固定为 Rust 1.88.0。对话循环仍是已有的应用实现，不能声称由 StateKnot Graph Driver、TypedAgent 或 durable admission 驱动。SQLite 保存聊天、应用级调度和渠道收发状态；这些事务不等于 StateKnot 运行检查点或可恢复工具执行。

2026-10-03 回访核对：main、v0.1.0-alpha.1 和 #140 状态未变化；优先保持已接入 HTTP MCP 的回归，durable 继续受 Brokerrouter #41 原生输出合同阻挡。

2026-10-08 04:38 UTC 核对 #141 的依赖/CI 更新；05:26 UTC 的 main `4e3c9e9194db524886ca795e5e2394be071ea202` 中，#145 仅更新依赖 patch 与 Dependabot 分组。该次没有 runtime/integrations 合同变化。

06:32 UTC 再核对 main 推进至 `9110ad71934e446d9fbb8ff21387cf14a7b7bdc6`：[#146](https://github.com/StateKnot/StateKnot/pull/146) 已合并本地 RFC 9068 RS256 JWT/JWKS 身份 profile，使用可信运营方 provision 的有界公钥集、CAS 轮换、过期租户策略和独立资源授权；真实 PostgreSQL 16/17 与 TLS Keycloak 验收覆盖声明的范围。可信公钥分发/刷新、多副本拓扑及框架生产门槛仍需部署验收。[固定接线合同](https://github.com/StateKnot/StateKnot/blob/9110ad71934e446d9fbb8ff21387cf14a7b7bdc6/docs/agent-jwt-jwks.md)。[#147](https://github.com/StateKnot/StateKnot/pull/147) 增加执行/凭据不得序列化的 compile-fail 合同及生产验收 ledger；该快照 main 的 12 项检查全部 SUCCESS。

这些是实际源码增量，不能再说本轮只有依赖变化；但公开 release 仍为 `0.1.0-alpha.1`，不包含新 JWT/JWKS profile。JiaClaw 未认证或接入该身份 profile，当前永久渠道映射与租户容器隔离不能据此改称框架身份接线。此 delta 没有修改现成 durable graph/native schema 或 MCP transport；#140 仍 OPEN、无回复，Brokerrouter #41 仍阻止当前原生输出合同。保留精确 HTTP MCP 依赖，不自动切换浮动 main。

08:23 UTC 飞书批次通过官方 GitHub API 复核：当时 main `9110ad7`、release、#140 与十二项成功检查均未变化；没有新增可消费 runtime/native Schema/stdio 合同。本批私聊身份与后端 protocol 4 属于应用接线，不新增上游缺陷议题。

11:35 UTC 企业微信启动校验批次复核：当时 main 为 `a312b0c2d09cd6d695b37b8d4163cddb910bdf6a`，[CI 37765235278](https://github.com/StateKnot/StateKnot/actions/runs/37765235278) 十二项检查全部 SUCCESS。已合并的 [#148](https://github.com/StateKnot/StateKnot/pull/148) 是实际工具合同修复：[typed Tool input](https://github.com/StateKnot/StateKnot/blob/a312b0c2d09cd6d695b37b8d4163cddb910bdf6a/crates/stateknot-core/src/tool_runtime.rs#L3466) 按 Serde Deserialize 方向生成 draft 2020-12 Schema，[output](https://github.com/StateKnot/StateKnot/blob/a312b0c2d09cd6d695b37b8d4163cddb910bdf6a/crates/stateknot-core/src/tool_runtime.rs#L3480) 按 Serialize 方向生成，并新增 [register_rust_output_type](https://github.com/StateKnot/StateKnot/blob/a312b0c2d09cd6d695b37b8d4163cddb910bdf6a/crates/stateknot-runtime/src/schema.rs#L321)。此修复尚未发布到 alpha.1；后续采用 typed runtime 时须固定新版本、复核 schema/version 与描述 pin，并保留已准入操作的旧合同，不能原地覆盖。[方向与升级合同](https://github.com/StateKnot/StateKnot/blob/a312b0c2d09cd6d695b37b8d4163cddb910bdf6a/docs/rfcs/0019-typed-tool-schema-directions.md)

已独立核对 JiaClaw 当前精确依赖及调用路径：HTTP MCP 直接保存服务端原始 input/output Schema 与完整描述 digest，经离线验证后单次调用；没有 typed Tool 或 runtime 类型注册调用。因此 #148 不要求当前 HTTP MCP 升级，也不表示其现有接线存在框架缺陷。最新 PostgreSQL 并发 journal 身份/投影测试覆盖真实 16/17，不能代替尚未完成的容量、fencing、failover 或 soak 闸门。release 和 #140 仍未变化；Brokerrouter #41 的原生输出准入缺口仍在。此次企业微信启动 token/AgentID/可见范围校验是应用接线，不属于 durable driver 或新身份 profile 消费。

本批应用验收已有默认并行 Rust 1105 项、企业微信启动六组/32 负例，以及同一冻结二进制的 WeCom/MCP/e2e/channels/scheduled_delivery 五套回归全部通过，原 HTTP MCP pin 未变；两次 fixture 观察/断言修订没有改动生产数据库或框架，见[验证记录](validation.md#企业微信启动校验批次)。后续 12:56 UTC 已核对 [PR #86](https://github.com/StateKnot/JiaClaw/pull/86) 固定 head `db802c46b8726e2dfbaf9defb1eebddb043619c7` 的 [CI 37776140027](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027)，Ubuntu/macOS/container 三项成功；不认证真实企业安装或 StateKnot durable。

12:56 UTC 独立用户企业微信批次复核：[PR #152](https://github.com/StateKnot/StateKnot/pull/152) 已于 11:56 UTC 合并，main 推进至 `04567c4db12553025b4d31330693f4958222c30f`， [CI 37773442993](https://github.com/StateKnot/StateKnot/actions/runs/37773442993) 十二项 SUCCESS。实际源码新增[严格空对象读取](https://github.com/StateKnot/StateKnot/blob/04567c4db12553025b4d31330693f4958222c30f/crates/stateknot-core/src/json.rs)，关闭七种内部 tagged 空变体对序列或额外字段的宽松接纳；合法 Rust 变体、规范 wire bytes 与七个 Schema pins 不变。新增 67 typed readers、104 正例、32 digest 变异，以及既有八类完整 wire 对照；[R1 qualification ledger](https://github.com/StateKnot/StateKnot/blob/04567c4db12553025b4d31330693f4958222c30f/docs/r1-contract-gap-ledger.zh-CN.md) 仍保留类型/属性/fuzz、namespace 与版本兼容/生产门槛。

#148/#152 均是未发布 main 能力；release/#140 仍未变化。当前 alpha.1 HTTP MCP 不读取这些 execution wire/tagged runtime 变体，也不调用 typed registry，不因这次修复升级或提交框架缺陷。新企业微信 protocol 5、私有队列/原请求/持久额度属于应用接线：最终默认并行Rust1158项与同一冻结二进制真实双后端十组已通过；提交准备时旧21套进程回归仍独立运行、最终固定head CI pending，不替代 durable graph 的原生输出合同或认证。

## MCP 现状与新议题

上游 integrations 已有受限的无状态 Streamable HTTP MCP 客户端（协议 `2026-07-28`、有界 discovery/list/call、HTTPS/字面量 loopback 约束），目前没有本地 stdio 客户端。

已提交 [#140：bounded stdio MCP client](https://github.com/StateKnot/StateKnot/issues/140)，需求正文保存在 [本地副本](upstream-issues/stateknot-stdio-mcp.md)。要求明确的可执行文件/参数、有限环境、进程组终止、超时和输出边界、生命周期与发现/调用测试。不能为了补齐 JiaClaw 的勾选项绕过框架另造一套不受治理的 stdio 进程。

HTTP MCP 已完成应用接线：名字空间、配置前置检查、逐工具白名单与描述 pin、离线 input/output schema、Bearer 环境变量、有限并发/截止时间/JSON/SSE、单次调用无重放与 MRTR 拒绝。网络 fixture 与真实二进制的 CLI/HTTP/SQLite 链路验收见 [MCP](mcp.md)。具体外部服务器的生产资格须分别审查；本批只开放管理员审查为只读的工具，未开放外部写入/OAuth 交互/multimodal/stdio。stdio 要等 #140 的有界客户端契约。

## 生产集成门槛

1. 在独立适配层固定已认证 StateKnot 版本，升级工具链到其要求版本；不追踪浮动 main。
2. 接入 durable admission / driver / store，明确请求、工具副作用与重试语义，不能把现有 chat loop 继续包装成 durable。
3. 接通 Brokerrouter 受治理 turn 身份与持久化 operation ID；未知提交状态进入恢复/核对，不换新键自动重发。
4. 覆盖进程崩溃、租约交接、取消、工具失败和恢复；具备备份/迁移、容量与运行监控证据。
5. 完成上游 README 所列生产 qualification，再将默认运行路径切换到框架。尚未达到这些门槛时，文档不得标记整个运行栈 production-ready。

现成 `ProviderNativeAgentGraph` 只接受模型原生 JSON Schema 最终输出；Brokerrouter 当前拒绝该合同，新增跟踪 [Brokerrouter #41](https://github.com/StateKnot/Brokerrouter/issues/41)。先补齐受治理的模型合同，再接 admission/driver/store；不能把旧文本 tool loop 包装成该 durable graph。

独立用户 Telegram/Slack/Discord/飞书及本批企业微信的 registry 准入、私有 inbox/outbox、原请求收据与人工 hold 属于 JiaClaw 应用接线；Discord 单独加密短期凭据，飞书业务文本仍为明文。没有切换到 StateKnot durable driver，也未扩大 HTTP MCP 工具权限。新源码 JWT/JWKS 身份是后续固定版本、可信身份部署和授权接线的可评估能力，不认证当前个人 Agent 恢复语义。

14:39 UTC 钉钉批次复核：StateKnot main相对13:51已核实的`83802cb3` compare identical/ahead0，release/#140未变化。#153仅公开类型/schema/constructor测试盘点与fixtures/CI/docs，没有当前HTTP MCP生产合同变化。重新读取最新main的`integrations/src/mcp_client.rs`与实际Cargo alpha.1缓存，字节完全相同，SHA256均`0d5887abaeba040784707193c5b99106c8b9d197d8239462f688216afcd07974`；JiaClaw仍消费原始descriptor/input/output Schema，未使用typed registry。没有必要升级精确pin或新重复issue；typed schema方向和durable/gap ledger门槛仍分开计。

此前企业微信固定head PR #87 三项CI及旧21套回归已最终通过，见[最终回填](validation.md#独立用户企业微信最终-ci-回填)。本批钉钉原始JSON/MIME修复属于现有应用协议路径，不新增框架原生durable/stdio身份，亦不将钉钉的未知安装权限映射归因于StateKnot。

15:36 UTC 复制批次重新核对官方 main/release/#140，与14:39快照 compare identical/ahead0；四份 runtime/integrations 合同源码字节未变。当前 HTTP MCP 与实际 alpha.1 缓存仍为83407字节、SHA256 `0d5887abaeba040784707193c5b99106c8b9d197d8239462f688216afcd07974`，现有 raw descriptor/schema 路径不消费尚未发布的 typed registry。PR #58–#88 均 OPEN/draft、当前head CI成功；[PR #88最终三项验收](validation.md#钉钉原始报文最终-ci-回填)已补证。copy 的共享资源、锁和 Schema 参数修复属于实际应用接线缺陷，不新增框架 issue、不升级pin，也不替代 durable/stdio/外部写入认证。
