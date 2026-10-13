# 修订任务草稿：源码与本机验收

本批 fresh 源为 `7fb67832e470412d58b104ed17cf6f31d2b8d443` 到 `ced431817d09786ee04ef45297e81ddcaab1dd74`，六路径（UI两文件、规格、两夹具及独立 CI 浏览器步骤）；source tree `d5580fc709a5257ff8369775ce82df7c5db8af77`。生产变化只有工作台 UI，消费原任务反馈、receipt GET/validator、草稿、授权和发送流程，无新后端端点、原 Store/schema/模型/hold 改动。累计 `origin/main` 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c` 及已合格 PR138 仅历史继承；不称穷尽累计 fresh 重审。

109 个编译输入中仅 app.js/index.html 改变，最终 source/cache外单冻结/target 二进制同 SHA256 `a6dfcf213e6127f3b4235662759e0732c80234d2fb41e90d03e6dd9710953fb9`。locked build、fmt、required correctness/suspicious Clippy 和原 parser12 均退出0；非必需 Clippy 警告保持，不称 warning-free。没有本批完整 Rust、旧全部 Python/浏览器、optimized/package/安装/Docker 本机重跑；这些旧结论只历史。

新夹具实际启动原 native、gateway、每用户 SQLite 和本地模型 HTTP：standalone、tenant JSON、tenant preview 三个真正 Chromium，各五组。每种实际旧任务及新任务 UUID 不同，原1次 model POST保留、新1次 model POST完整接收可编辑草稿；准备阶段只有1次原GET、0写入，原 receipt 全字段保持，所有权限取消。完整回复/说明、当前revision变化拒绝、真实迟到GET/较新草稿/身份保护、刷新确认丢弃、两租户只读、真实管理员purge拒绝、私DOM清理、storage0/pageerrors0均覆盖。三 typed19field 收据、六唯一UUID、逐 model input/draft SHA在持久外部日志封存。元数据故障 section 的 exact32768/32769、NUL和surrogate是明确 synthetic receipt，不冒称这些字节由模型执行。

实际运行 Playwright1.62.1，用已有 custom Chromium153.0.8010.12（1243），不从缺失默认分发推断成功。root 新三套 session70531 退出0；独立 Spec 使用同 source/冻结 binary 自己新执行一次，session49738 退出0、15.238秒，亦完整三套。旧反馈 API8与原三反馈 Chromium 在新 binary 回归退出0；其余旧全矩阵未重跑。fixture/模型完成不证明真实付费模型修订质量或自动经验学习。

初四次夹具退出1独立留证：拒绝后的非fresh原收据未先GET就点击Finish（两处）、不存在的disconnect选择器、刷新后未展开权限details。修订改为原GET+保留新草稿断言、实际reconnect控件及summary展开；原断言、15秒GET、30秒UI等待和150秒进程预算不减，生产a6df字节不变，不报上游。Standards初finding绑定原第二fixture copy，最终关闭；[初Standards](reviews/2026-10-13-task-revision-standards-initial.md)/[最终Standards](reviews/2026-10-13-task-revision-standards-final.md)、[初Spec](reviews/2026-10-13-task-revision-spec-initial.md)/[最终Spec](reviews/2026-10-13-task-revision-spec-final.md)各独立保留，最终两轴0。

官方新 head 须独立七实际作业（另draftskip）、两平台66 Python、Linux原19+本批3共22真实 Chromium、四候选各13完整唯一源码/归档安装及认证API digest、全部继承168与新修订草稿guard=169完整通过；然后 fresh 官方 Standards/Spec 和正文/pin精确回填。候选不执行新三浏览器，Mac不执行Chromium；原 browser_suites12、feedback total19只是继承字段，不能作为本批总数。任何 prefix、本机、父head或 source review都不替代自己的官方资格。

外部证据优先持久 `.jiaclaw-evidence/round99-revision/`（Git本地排除、target外）；旧/tmp已知缺失不伪造恢复。缓存先df并核tag/真实路径/Git/锁，按同repo/toolchain/profile复用共享target，Rust命令incremental0/dev-testdebug0保留原断言溢出，无target复制或清理，回收0。physical releaseZIP全本机hash、供应商/publicTLS/计费代理/公开安装/业务质量/Agent浏览器/审批继续/StateKnot durable仍未认证。
