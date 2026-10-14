# PowerPoint 文字任务验收范围

最终源码 `d472d3ef8aac5ba8e40f8014adf060c3adc9279e`，119项完整编译输入 manifest `30b1bc72f5407050a4917a7045a59e673b49e34bc398c22e4b0eece540744a44`，冻结本机 binary `f9920b2e54c746cbfbe81a8340c8a78aaba378aa66f1b921791f67cb5a9c82a3`。源码、交付和新远端固定head分别绑定；累计 `origin/main=4b2357fcf01f47ba08d7724edbba7accfb60972c` 与父PR145完整资格仅历史继承，不称累计fresh穷尽审查。

最终实际本机：1321 Rust（473/123/725）加1 ignored、四线程，locked build/fmt/required correctness+suspicious Clippy各0；209项非必需host-test警告保留。三个新入口与原XLSX/Word/PDF/Text各三个、会话迁移一个，共16次真实Chromium命令和parser13均0。Playwright1.62.1使用现有custom Chromium153.0.8010.12（1243），未使用缺失default1234。没有本机完整68普通Python、37个全部历史Chromium、优化打包/安装或Docker重跑。

六个实际任务分别完整捕获2411与1048576源字节、127与16384提取字节、350与32768消息字节；每次FileReader1、真实Worker1且terminate1、import API0、外部请求0、原model POST1、tool records0。六个UUID及完整source/text/prompt/model/reply SHA独立核对，原回复真实物理保存；未勾原权限先拒绝，取消、身份/草稿/选择迟到、五秒、readonly、375px、零storage/page errors分别验收。另实际python-pptx1.0.2生成的28297字节普通文稿、嵌套group独立文稿、32页边界、Strict页序与原ZIP边界分别核验；它们不认证PowerPoint视觉或业务质量。

初版两轴独立发现原始非元素文字/未知属性与结构被忽略，以及标准producer打印元数据被拒绝，两项P2均以同文件真实UI/Worker反例关闭。后续Spec发现重复break属性P3，最终整次拒绝并保留单属性正例。初、中、最终报告独立保存：[Standards初版](reviews/2026-10-14-presentation-input-standards-initial.md)、[修订](reviews/2026-10-14-presentation-input-standards-final.md)、[闭合](reviews/2026-10-14-presentation-input-standards-closure.md)；[Spec初版](reviews/2026-10-14-presentation-input-spec-initial.md)、[中间](reviews/2026-10-14-presentation-input-spec-intermediate.md)、[最终](reviews/2026-10-14-presentation-input-spec-final.md)。最终各0开放。root最初互通失败的单入口完整日志被后续运行误覆盖，仅原exit1过程收据保留，不伪造恢复；独立两轴原RED完整日志不受影响，后续阶段另存。

新官方审计器只新增一个Presentation guard，原178 Assert（含重复）和其余normalized AST完整保留。必须新fixedhead自己的七actual（另draftskip）、两平台各68普通Python、Ubuntu37实际Chromium、四候选各13完整唯一源码/安装归档与认证artifact API digest及Current file capture独立步骤，以及完整179合同，再由独立fresh官方Matt Standards/Spec各自认证读取和完整执行。Mac不跑Chromium，候选不跑PPTX/Word/PDF/feedback API/model routing/Chromium；本机、源码/交付/发布审查不替代官方资格。发布前此段是资格要求，不是已完成声明；后续只回填PR正文/pin，不为自引head再造CI。

原60分钟、三秒header、native10秒、10+5共15秒关闭、250ms、PDF/Word/XLSX/PPTX五秒、会话导出15秒与所有fixture预算保持。df与真实CargoTAG/realpath/无Git跟踪/打开文件核对后，共享target按同repo/toolchain/profile复用保留；无target复制、专用新缓存或清理，回收0。物理release artifact ZIP全本机hash、正式供应商/public TLS/paid proxy/实际公开安装/业务质量/学习/Agent浏览器/StateKnot durable仍未认证；无paid调用、force、merge或公开release。
