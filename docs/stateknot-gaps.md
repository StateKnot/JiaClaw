# StateKnot 集成状态

核对日期：2026-10-02；上游 main：`fbd629d73e50610dc8f4889b47e05f698cc110e7`。证据来自 [Cargo.toml](https://github.com/StateKnot/StateKnot/blob/fbd629d73e50610dc8f4889b47e05f698cc110e7/Cargo.toml) 与 [README](https://github.com/StateKnot/StateKnot/blob/fbd629d73e50610dc8f4889b47e05f698cc110e7/README.md)。

旧结论“edition 2024 不稳定、crates 没发布”已经过时：上游已发布 `0.1.0-alpha.1`，要求 Rust 1.88+；发布追踪 [#92](https://github.com/StateKnot/StateKnot/issues/92) 已关闭。上游仍声明处于 pre-alpha/evaluation 阶段，没有生产支持承诺。

JiaClaw 已精确锁定 `stateknot-integrations = 0.1.0-alpha.1` 并使用其 HTTP MCP 客户端，工具链固定为 Rust 1.88.0。对话循环仍是已有的应用实现，不能声称由 StateKnot Graph Driver、TypedAgent 或 durable admission 驱动。SQLite 保存聊天、应用级调度和渠道收发状态；这些事务不等于 StateKnot 运行检查点或可恢复工具执行。

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
