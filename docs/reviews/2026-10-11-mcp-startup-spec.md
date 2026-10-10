# Spec 独立源码审查：MCP 启动阶段结算

固定累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c` → `5de3e5e7fd5f8a808903b52b44524b235c6fe2d4`；继承此前累计审查，本次 fresh `6e338ad..5de3e5e` 仅独立审阅 mcp.rs 测试模块及七条诊断规格，未读取 Standards，不声称穷尽累计重审。

**新增开放 Spec 问题：0。** 第1条要求生产“byte-identical”，已逐字节核验 `cfg(test)` 前源码与父提交一致，原诊断快照匹配，临时探针移除。第2条要求实际 schema admission 后才到原一秒截止；独立 workers 实际许可减一才推进虚拟时钟，真实 blocking gate 仍持有唯一执行槽，超时后许可仍归原工作，实际 gate join/许可结算后新启动成功且零 tools/call。

第3—4条要求 held discovery、全许可/零调用/不重放及“observe settlement of the abandoned discovery response”。记录真实请求后才推进原截止；仅两种原配置错误合法。Fixture 单一顺序服务端且此时仅一请求，finished_responses 在完整响应写入尝试结束后发布（取消连接可写入失败）；新初始化在取得此唯一收据后才启动。因此它不会把新请求响应冒认旧响应，也不声称客户端接受过已取消响应。先后 registry 空及全许可核验、后续批准 descriptor 与零 tools/call 保留。

第5条的五秒真实 setup 与两秒观察仅为 harness guard；paused-time 持真实 localhost 阶段，advance 原一秒并 resume 结算，没有生产期限变更或物理冷启动认证。第6条三 held-discovery RED 证明旧夹具阶段混淆可复现，不证明历史 expected3/actual4 的具体阶段；初 paused-clock SIGTERM、两次严格错误断言失败分别保留，不是生产缺陷。

第7条最终 source 的重复阶段、全并行/原串行 Rust、build/fmt/Clippy及实际进程与冻结身份仍需最终日志独立补核；本源码审查不借中间 GREEN、父或本地为新官方资格。未额外编译或运行测试。机器补核 `mcp-startup-diagnosis/spec-source-verification.json`。
