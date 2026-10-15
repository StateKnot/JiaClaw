# 网页正文读取预算验收范围

本批基于 PR150 已完成资格的本地交付 `91fc1999632d42c94a78c37689ec7b27ec08b967`，修复原 `web_fetch` 在完整下载之后才检查 1 MiB，以及客户端在原逐跳校验前拒绝 302 的应用接线缺陷。累计 `origin/main` 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`，此前源码、运行和资格结论为历史继承，不称累计穷尽 fresh 重审。

新增 Rust 回归使用真实本机 TCP 服务和原 `WebFetchTool`：发送完整响应头与 1 MiB + 1 字节前缀后保持连接，只有客户端完成观察后才允许清理关闭。旧实现同一长度响应在 3 秒观察内未完成，实际 RED；修订消费原 minreq `send_lazy` 和标准库 `Read::take`，不添加客户端或解析器。固定长度、chunked、关闭定界、204、非成功状态及原 HTML／参数／私网案例分别验收。3 秒是新反例的观察窗口，不修改原 15 秒 HTTP 或任何既有工具、关闭、CI、文件、浏览器预算。

最终本机完整 Rust 为 1331 成功（482／123／726）加 1 原有 ignored；locked build、fmt、必要 correctness／suspicious Clippy 及原 e2e、native_tools、http_turns、model_calls 四个真实进程套件均退出 0。非必需 warning 保留（host test Clippy 212），不称 warning-free。没有本机重跑全部 71 Python、43 Chromium、parser、优化打包安装或 Docker。

冻结源码 `898bcf37b68c4a899b2d57d419cf567cdb5b0dd2` 的 122 完整编译输入与交付逐字一致；二进制 SHA256 为 `2531980f972debdf291f70b444c7383aeb7be4ccd449bd0722be4169fa808bdb`。独立 Spec 用该二进制完成八个真实 localhost model／HTTP 场景：held length、未结束的大 chunk、close、exact 1 MiB keep-open、204、503、相对 302 和损坏 chunk；旧父二进制的相同 held-length 反例另存。该轴自己的执行不冒称 root 重执行。独立[Standards](reviews/2026-10-15-web-fetch-body-standards.md)和[Spec](reviews/2026-10-15-web-fetch-body-spec.md)报告保留各自范围。

实际退出、日志和初始错误分别保存在 Git 本地排除的 `.jiaclaw-evidence/round155-next/`。首次 `take` 歧义编译失败后明确调用 `Read::take`；新 chunk 夹具初次误扣非零 chunk 的 CRLF，校正为完整该 chunk 定界、仍扣零终止块；首项进程验收在异步 freeze 完成前找不到二进制，未启动产品，原记录保留后才执行正式四套。三者均未放宽产品或原期限，不报 minreq／上游缺陷。

新的固定 head 需要自己的七个正式作业、两平台各 71 个普通 Python 套件、Ubuntu 43 次真实 Chromium、四候选各 13 项独立安装归档源码／artifact API 摘要和 Currentfile capture；原 184 审计合同和本批六个非容器作业各自的三条新 Rust 边界必须完整通过 185 项。审计器正反例检查与 AST 继承证明只是 synthetic driver 验证。源码／本机结果和父 PR150 不补新 head 资格。发布前的此记录只描述本批交付范围，不认证尚未取得的 CI 结果；最终资格仅回填 PR 正文和本地 pin，不生成自引 head 的提交／CI 循环。

不认证完整物理发布归档、真实付费模型、供应商、公共 TLS、公开安装、业务质量、自动学习、Agent 浏览器或 StateKnot durable。没有 paid 调用、自动合并或公开 release。
