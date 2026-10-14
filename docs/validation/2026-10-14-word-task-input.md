# Word 正文任务输入验收

本批在原 standalone、个人 Key JSON、预览工作台增加本机 `.docx` 正文材料。规格见 [Word 资料输入](../word-task-input.md)。复用原 FileReader、PDF 捕获/身份/取消 owner、消息编辑和显式 Send；真实 Worker 使用浏览器原生解压，浏览器 DOMParser 读取有界 XML，不新增依赖、上传/模型工具、存储或检索引擎。

## 固定源码和实际执行

- 父 local `6c5657cf0c55eeb63e151fae67dc6497f1654632` 与 PR142 完成树一致；父142资格仅历史。最终 source `1bbd9cd2621c82538dd3810202d60fe44db75077`，production `e0c2a1b247acbc1bb6819d47033c85764d8e4524`；116完整 compiler inputs manifest `a98adca9992db540be32a9abf0f7f8b288f802648044ee8f63ff3b75ab3bcf19`。最终 frozen/target binary `5095f54d187ca44b8f493cfc9880b38152e5c08627ce392caf968446525ef932`。
- 本机最终 1320 Rust（473/123/724）+1 ignored、4线程；locked build、fmt、required correctness/suspicious Clippy 均0；209 host-test 非必需 warnings 保留。每次 Rust 命令 incremental/dev-testdebug 为0，原断言与溢出检查保持。
- 最终新3 Word + 原3 PDF + 原3 Text 共9实际 Chromium；parser13，各0。Playwright1.62.1 使用既有 custom Chromium153.0.8010.12(1243)，不是缺失 default1234。没有本机完整68普通Python/30Chrome、optimized/package/install/Docker 验收。
- 新三入口各2个真实任务：完整源 SHA/提取/完整 prompt/真实模型 wire/物理回复 SHA，原1FileReader/1真实Worker/terminate1、0importAPI/外部请求、显式权限前拒绝、每任务1modelPOST/0toolrecords；六原UUID唯一。精确1MiB源、16KiB正文、32KiB草稿通过，超限整次拒绝。源SHA不是来源真实性。
- 原5秒绝对截止、身份/草稿/selection变更和真实Worker迟到回调保持拒绝并终止；只读（两租户入口）、375px、零storage/pageerrors。ZIP/CRC/重复/DTD/外部根关系/宏/不支持结构/空正文/实际deflate超声明/129项与unsafe path拒绝。每入口60个两命名空间修订标记原生Worker拒绝，4个真实UI属性/单元格修订拒绝。

## RED 与独立审查

初 `f9a1b895c2ef207ac402b88b4a4f028856ac6276` binary2074 两轴独立发现 P2：属性修订和表格单元格删除仍可导入。原文件、真实UI/Worker RED、初报告封存；`e0c` 拒绝所有正文属性/文字/单元格/移动/customXml修订标记，同文件 GREEN；普通rsid属性仍可读。两轴分别独立复验，source初/最终与后续仅fixture差异报告分开，不把初判断改写为最终通过。

初 standalone 用例在移动宽度发现溢出，扩展已有资料控件共享CSS后通过；最初失败的具体元素未捕获，不猜唯一原因。受限环境 OS/loopback 拒绝导致一次 Rust/三入口执行失败，授权相同原断言后完整通过，原日志保留，仅环境限制。外部 driver 准备括号语法错误、zlib890/889压缩字节差异和提交批准前 metadata 身份错误只属collector/fixture准备，原记录保留，不报框架缺陷。

`1bbd` 只固定真实 data-descriptor 验收包的标准 raw-DEFLATE stored blocks，独立Python重建逐字一致，不依赖zlib压缩版本；其他原生动态deflate正例/超限反例保持，生产inputs/binary不变。新外部175sites审计器逐字保留原174Assert AST及重复计数，仅加Word独立guard；root1positive/420negative只 syntheticdriver，不算产品或官方执行。独立driver初Standards P3标记行尾矛盾、Spec另P3重复namedAPIstep矛盾均留原反例；最终检查完整标记行和唯一实际成功步骤，175原断言不减；最终driver两轴另存。

## 官方资格边界

本批需自己的七actual+draftskip、两平台各68普通Python、Ubuntu原27+新Word3=30实际Chromium、四candidate各原13完整源码/安装归档/API digest及当前文件capturestep，所有继承174合同和新Word唯一exact3commands/六strict20field收据/完整独立SHA/命名APIstep/顺序全局唯一markers，完整175后再fresh独立官方Standards/Spec。Mac不跑Chromium，candidate不跑Word/PDF/反馈API/modelrouting/Chromium。本机或父142/发布身份审查不能补官方资格。

本地日志、运行收据、输入清单和冻结二进制在 `.jiaclaw-evidence/round121-next/`，位于target之外。共享target按同repo/toolchain/profile评估复用保留；无whole target复制/专用缓存/清理，实际回收0。每次执行前df，APFS du或其他进程变化不算本任务释放。

累计origin/main固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`，历史继承，不称穷尽fresh累计重审。Word原文件留本机；显式Send后文件名/源SHA/正文进入配置模型和原会话，费用可能增加。正文有界提取不是整份Word视觉/语义/OCR/批注/业务质量认证；未认证供应商/publicTLS/paidproxy/公开安装/学习/Agent浏览器/StateKnot durable，无paid调用/merge/公开release。
