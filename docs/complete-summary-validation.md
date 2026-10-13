# 完整摘要输入验收记录

本记录是发布前的源码／本机证据。累计基准固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`，PR107–136 的完整资格和历史审查继承，不称本批穷尽累计重审。fresh 源为父本地 `640f5a4ce4492ed565b61f4a6f8ca87434783594` 到 `52f57c4c69efb23981e7808ef30ecdf5cb4b5cb2` 的六路径；运行输入仅 `crates/jiaclaw/src/lib.rs`／`session.rs` 两项改变。

冻结父 `013bf865…` 在同一真实本地 HTTP／模型端点用例只发送4373字节摘要输入，预期262144字节，实际 SHA `eadc6b4c9f6871a0891b22b0226ab6d71c277b37ba18981e4aabda41b0fcce61` 与预期 `456fd577beaa3cec7e2d6f3cc4e8b78c20d97f8b71ebda9c48a16ee0b02648ac` 不同；断言真实 RED。初次 sandbox loopback bind 拒绝独立保留，属于环境限制，不算产品失败或上游缺陷。

最终新冻结二进制 SHA `27f49af85b1dd3c0e8ca2b7d0bf6e2b83db502135e9d363ae317ea3e7b4ec289` 与 source／current 的109编译输入逐字节绑定。恰好262144字节完整输入与出站 SHA 一致，包括原800字符之后的尾部约定、Unicode、BOM、TAB和CRLF；原 Summary 模型、0.2温度、512上限、一次无工具请求不变。262145字节零摘要 POST、精确原50条硬截断（写入回复后51条）。两个案例共四原聊天POST／一摘要POST；原 SQLite 重启后逐消息相同、零模型重放。真实本机日志 `/tmp/jiaclaw-oct13-summary-source/model-routing-settled.log`；日志含精确字段收据，完整输入与拒绝输入 SHA 分别 `456fd577…`／`7d714fcf…`。

完整本机 Rust 1313（470／123／720）通过、1忽略、4测试线程；fmt、锁定构建和 required correctness/suspicious Clippy 均通过，206项非必需 host-test warning 保留。三项新增Rust覆盖完整长文本、角色／分隔符和多字节边界、总预算／错误脱敏。最终六相关真实进程套 e2e、model_calls、cli_stream、http_turns、tenant_http_turns、read_only_keys 全通过。初次运行及最终明确 stub 路径兼容后的运行分别保留，最终结论只绑定最终源码。

生产UI／parser未改，本机未重跑16Chromium、优化候选、打包、安装、Docker或其余全部65Python；新head官方检查必须独立完成。既有完整164审计断言 AST 原样继承，新增加三Rust边界及两平台原model_routing唯一完整命令块的全文／超限／重启精确收据合同，共166审计位点。审计器 synthetic 反例只认证driver，不计产品执行或官方资格；原Python65／Linux16Chromium／四候选各13归档安装要求不减少。

共享 target 按同仓库／工具链／配置复用，已核真实路径、CACHEDIR.TAG及无Git跟踪；容量超过20decimalGB经评估保留，不复制target或另造cache，实际回收0。每次Rust命令 incremental=0、dev/test debug=0，保留断言和溢出检查；df、日志、binary、报告在target外留存。APFS du和其他进程df变化不算本批释放。

本地fixture不能认证供应商、公网TLS、计费代理、实际公开发行安装、真实业务摘要质量、经验学习、Agent浏览器、审批/子Agent或StateKnot durable。tracked任务仍原历史预算，不因本批产生隐式模型请求。默认关闭、stub/import digest、未知提交hold和原权限/取消/期限分别保持。
