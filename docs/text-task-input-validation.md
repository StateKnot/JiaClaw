# 本机文本资料输入：发布前验收快照

本文件记录首次发布前的本机验收 checkpoint，不宣称未来远端 head 的 CI 已完成。固定本地父 `788b094e182a2488e86fd4dfc83f89acf5052c0e` 对应 PR135 已认证的完整 tree；其源码、交付和官方结论只历史继承。累计 `origin/main` 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`，本批 fresh 范围是父至新源码的 14 路径和后续交付文档，非穷尽累计重审。

初版源码 `5285a20`／binary `a56544…` 在原空账本断言前先执行两项新任务，产生 `2 != 0` 的测试编排失败。新增真实文件与守卫已执行，但此阶段不标完整回归。候选 `0484818b72865b5c82f3567104fac66856432a72`／`4fdd479f…` 改用独立原 fixture 实例，完整 1310 Rust 和 16 Chromium 通过；独立 Spec actual6989 仍以真实文件名复现 U+061C／U+200E／U+200F／U+0085 被接受的 P2。初版日志、二进制、报告和反例单独保留，不转移到修订资格。

最终源码 `19dbec7a883b07f28353f9e039db4f34e8774fe9`／tree `34dd0f6e948b555b0ec8d20efc4379c36fa3b2e2` 仅补齐文件名 Unicode Cc／Bidi_Control、公开负例及独立 fixture 完成标记。109 个完整编译输入逐字节绑定源码、当前工作区及冻结 binary；最终 SHA256 `013bf86509c049a012fe8d69b0f5f1a6d7389183af2536823228cfc957c12f92`。body 的 ASCII 控制字符合同、文件 16 KiB／整稿 32 KiB、原五秒绝对截止及原发送预算保持。

最终实际本机执行：locked build16161、完整 Rust30215（467／123／720，共1310，另1ignored，4线程）、必需 correctness／suspicious Clippy、fmt，各 exit0。非必需 warning 保留（含 host test206 等），不称 warning-free。原13 Chromium 加独立文本资料3 Chromium，actual50709 的16命令各 exit0；原 parser12 actual25dc99 通过。没有本机 optimized/package/Docker 或全部65历史 Python套重跑。

六个真实本机文件任务覆盖 standalone、租户 JSON、预览，各两项：81原UTF-8字节（BOM／CRLF／CR／Unicode／HTML外观文本）和完整16384字节；整稿分别203／32768字节。每项浏览器只读所选文件一次、加入阶段零新增 API、原工具仍默认未选；未授权发送拒绝，随后明确选择原工具才有一次真实本地模型 POST，模型收到的完整消息 SHA 与草稿完全一致，保存工具记录零，实际下载完整回复的文件字节与原收据一致。

真实非法文件负例涵盖扩展名、空／超限、非法／截断UTF-8、NUL、换行／长／Cc／bidi 文件名和整稿超限，草稿保持。真实文件读取完成通知的测试控制分别延迟到身份切换和原五秒定时取消之后；旧完成不追加。此控制证明浏览器状态机守卫，不冒称 OS 慢读或硬抢占。只读、移动端、无浏览器存储和文本渲染实际验证。独立 Spec 原六 filename case actual65012 在最终冻结 binary 同条件通过；另 actual26234 验证草稿／选择代次／真实重连、主线程阻塞至5108ms后的绝对截止、刷新清除、128字节名称与正文 BOM／LF／TAB 及允许的 C1／bidi 文本保持，各零导入 API／模型请求。

外部官方审计保留父163 assert AST逐字等价，仅新增一项=164。新三条 exact Actions 命令在独立 step 核六个唯一 typed 实际任务／物理文件收据、严格类型／完整输入-模型-回复 SHA、六新 marker／原结束标记全局唯一和内部顺序；两平台65必需Python和原13 Chromium不减，Linux总16。root新guard synthetic 1positive／67negative拒绝；这是审计器验证，不是官方或新增产品执行。初9474审计器接受 JSON receipt 尾接额外文字的反例保留；修订只在新函数匹配行末。原错误 parser 路径等临时 driver 错误单列，不报产品／上游。

独立 source Standards／Spec、交付和发布身份各自留报告；新的远端 head 仍须自己的七实际作业、四候选各13源码／安装／归档和 authenticated artifact API digest，以及完整164和 fresh官方两轴0，不能借父135、本机或前缀。全物理 ZIP 本机摘要、供应商／公网 TLS／计费代理／实际公开安装／业务质量／学习／Agent浏览器／StateKnot durable 未认证。Playwright1.62.1 实际使用现有 custom Chromium153.0.8010.12（1243），非缺失 default1234；官方实际版本另核。

每轮本机验证前保存 df，约20.9百万KiB共享 target 按同仓库／工具链／profile复用评估保留；incremental0、dev/test debug0，原断言和溢出要求保持。冻结二进制／日志／报告在 target 外，无整个target复制或新专用cache，实际回收0；其他进程／APFS变化不算本批回收。所有请求只指向隔离本地 fixture，未使用未授权付费凭据；不自动合并、公开发布或 force push。
