# 网关监听前信号注册的修订验收

本次修订以本地交付 `4fb07ab3ba2b4b85da654521c7e7a77a3c5653c5` 为 fresh 基点，源码固定在 `575641d1b8367a1f1e0d29f14e9f0d9808bc0140`。源码范围仅 `gateway/mod.rs` 与 [信号所有权合同](gateway-signal-readiness.md)。PR149 原文件名任务及累计 `origin/main 4b2357fcf01f47ba08d7724edbba7accfb60972c` 的历史证据继承，不称累计穷尽 fresh 审查。

## 实际 RED 与同条件 GREEN

固定旧二进制 `95fb9a69a0342237e8dc216f44e4fe80ac9e2afcb1c66c4600ee0b600bf5f694` 通过真实 `listen()` 后的受控窗口，TCP-only readiness 已返回，启动主线程仅等待 0.2 秒释放。原 SIGTERM/五秒观察器两次得到默认信号终止 `-15`，约 5.019/5.016 秒被原 generic barrier 拒绝。普通对照约 25 毫秒退出零。

最终二进制 `c5175967c0a0faf15e19aa3e158951a2985d2d2f4abe87cf058048779fb0c643` 在同一库、同一原 Fixture/stop_signal/until AST 和原五秒窗口下，普通对照约 25 毫秒、两个受控对照约 216/210 毫秒均正常退出零。没有第二次信号、重启、slow-download socket、后端 POST 或 hold。实验只控制本机启动窗口，不认证 Linux/Hosted 等价或历史失败唯一原因。

初诊断库 `RTLD_NEXT` 调用产生递归堆栈溢出，没有合格 listen marker，是外部 harness 错误；原脚本/库/三日志保留。改用同一镜像原 `listen` 的 interpose 方式后才得到上述产品 RED，对照没有改生产源码或原期限。

独立 Spec 自有库另做八个真实服务对照：旧/新 binary、TERM/INT、route-ready baseline/listen-window，旧窗口分别默认退出 `-15/-2`，新窗口均正常退出零。该轴尝试注册失败注入时先遇到自有短 token、后遇到最小 backend `/health` 返回 501，未达到 sigaction hook；相关 composite/probe 退出 1 和初记录保留。注册失败的运行时注入仍未认证，仅静态核验 Result 在 bind 前传播，不把准备失败当产品 finding 或通过。

## 实际本机回归

完整 Rust 1326 项（477/123/726，另 1 ignored，四线程）、locked build、fmt、required correctness/suspicious Clippy 均退出零；211 个非必需 host-test warning 保留。新的隔离 Rust 测试对子进程实际发送 SIGTERM、SIGINT，要求在关闭 future 首次轮询前注册的接收器仍能在原五秒内完成。

原 `gateway_shutdown.py` 七组全部退出零，原准入、未知 hold、重启、四慢连接、迟到观察、三秒响应头、共享 grace 和五秒 observer 均不变。九个相关真实 process 套均退出零：tenant HTTP turns/stream、readonly、tree、glob、file-read/file-write、skill-read、memory-search。没有本机重跑完整 70 Python/42 Chromium、parser、优化打包安装或 Docker；零 paid/vendor 调用。

121 个完整 compiler inputs 只有 `gateway/mod.rs` 改变。源码/交付/当前 target 的输入和最终 binary 字节需在交付身份中逐一绑定；旧 root/delivery 冻结 `95fb9` 保留，当前 target 已是 `c517`，不能说父 target 不变。日志、报告和冻结 binary 位于 target 外；共享 Cargo cache 已按同 repo/toolchain/profile 评估复用，未复制或清理，回收零。

## 审查与官方资格

源码独立 [Standards](reviews/2026-10-15-gateway-signal-readiness-standards.md) / [Spec](reviews/2026-10-15-gateway-signal-readiness-spec.md) 分开保存。外部 driver 审查、交付和发布身份审查各自独立，不代官方完整资格。新增 driver 保留原 182 个 Assert AST（含重复）与规范化其余 AST，只加原六个 non-container Cargo test 成功步骤中的真实信号测试门槛，共 183 sites。外部 synthetic 正反例不算产品或官方执行。

修订 head 必须独立完成七 actual + 条件 draft skip、两平台各 70 ordinary Python、Ubuntu 42 actual Chromium、四优化候选各 13 source/install/archive 与 Currentfilecapture/认证 artifact API digest、全部 182 继承合同及新信号测试，再 root full183 与 fresh 官方 Matt 双轴。当前只允许声明新 head 资格 pending，不借旧六成功/四候选、本机或前缀；最终资格只回填正文/pin，不提交自引 head 资格文档。

PR149 原 Ubuntu Alpine pull 错误的实际 HTTP status/route/quota 和唯一原因、原 Intel 第五组重启后三秒 header timeout、新 Ubuntu 第五组初始 stop 五秒 marker failure，各完整原证据分开保留，唯一原因仍未知。新的局部生产缺陷修复不证明这些旧失败同因修复。R145 原拒绝 gate 与 R146 新范围两次单作业 POST 实际 201、额度用尽保持；本次是真实源码修订，不再执行旧 head retry。
