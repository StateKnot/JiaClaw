# PDF 资料任务接线验收

本批以父 PR140 已完成的独立资格为历史，新增 PDF 资料入口，复用原 FileReader、可编辑草稿与显式发送。累计 origin/main 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`；本批 fresh 范围仅新增 PDF 接线及受影响入口，不称累计穷尽重审。

## 许可证补齐后的最终接线

最终生产源 `1173e7f0cb9d1bd8ac792eb5ceee8b53f6cbe1dd`，113编译输入 manifest `717e755fc84915553caebe1163be552f3908476c379a770e85cb248dfc1e5cad`，冻结 binary `58a5a58cdeff0310f29d6c34fd42393d045c4fa3e4275230d2b0d15944952891`。原完整10174字节 Mozilla LICENSE 随二进制嵌入，两端只读同源文本资源与明确许可证链接提供原摘要；三入口实际GET核验MIME、nosniff、全文SHA及长度。

许可证修订后重新运行完整1318Rust+1ignored、locked build、fmt/必需Clippy和原十个外层命令，共13实际Chromium（新PDF3/旧10）及parser13全部通过。下方e58与其112输入/89d二进制是此前加密/XFA修订阶段的历史记录，不作为最终1173编译或运行资格。

## 加密/XFA修订阶段的本机记录

此前源 `e58dec189096bafb9f85d82a7c82ad8f0518acf8`，112 完整编译输入 manifest `064fbf9fccb124b2b05d0878714d00bedd2804ae3e664b74831a3ae63be98a97`，冻结 binary `89d89b0be2aaf7ad015978a6c92f623de4663a8d3841c8b83d53029bfb9284aa`。固定 PDF.js 6.4.299 的完整官方 dist ZIP、三个上游原字节及 LICENSE 摘要见[功能合同](pdf-task-input.md)。SDK 字节来源绑定不等于逐行认证上游全部实现。

完整 Rust 为1318（472/123/723）通过，另1 ignored；locked build、fmt 和必需 correctness/suspicious Clippy 通过，209项非必需 host-test warning 保留。使用原同仓库/工具链/profile缓存，命令环境 incremental/dev-test debug 均为0，原断言与溢出检查保持。缓存超过20GB已评估复用保留，无整个 target 复制、新独立缓存或清理；实际回收0，APFS/其它进程磁盘变化不计释放。

三实际 Chromium 入口各运行两份真实 PDF→草稿→显式原发送→唯一模型 POST→物理回复下载，共六唯一任务；2/32页、完整1MiB文件、完整32KiB草稿、源/提取/模型输入/物理回复 SHA 对齐，未截取、未上传原PDF、工具记录0。另实际中文字体映射、16KiB精确文字与16385超限、空页/33页/损坏/空密码和需密码加密/XFA整体拒绝，以及真实 Worker GetDoc 暂留、原5秒截止/身份切换终止、迟到不恢复草稿、移动端/readonly/无storage通过。文字按 PDF.js 内容流与字体映射提取，不认证 OCR、图片、视觉全文或业务阅读质量。

受影响的原 standalone/租户JSON/预览、文件交付、反馈、修订草稿和 parser13 已实际回归；根执行共13次实际 Chromium，新3与旧10分开记录。独立 Spec 的自身 PDF/中文/预算/加密/XFA 和原文本资料回归另外留证，不冒称根重执行。Playwright1.62.1 使用现有 custom Chromium153.0.8010.12（1243），不是缺失的默认1234分发。未本机重跑全部67普通Python/全部26Chromium/优化候选安装打包/Docker，不能以本机补官方资格。

## 初始失败与修订边界

初始固定 EB19 空用户密码加密1105B及带经典文字的 XFA906B，独立 Spec 实际被导入、模型 POST0；Standards/Spec各两个P2原报告保留。最终同一解析 owner 先取 typed metadata，拒绝非空 encryption filter及XFA，异步后复核同一原截止；同原固定文件重新验证整体拒绝，预算/期限未变。

此前e58源/344交付记录未发现完整LICENSE只有源码文件、尚未嵌入二进制这一缺口；根在发布前发现，Standards独立核验原冻结文件缺全文/113th编译输入/两端资源。原两轴0记录明确只为该阶段历史，不借其关闭这个后续缺口。1173嵌入原LICENSE并实际三端GET核验，新源码/driver/交付两轴另行闭合；所有修改均在首次新PR发布前，不造发布后自引资格CI。

最早默认Chromium路径缺失为环境准备；首次拒绝观察超时未捕获原case/DOM，唯一历史原因未知，独立保存。后续真实380px页面横向溢出修订原资料控件共享CSS约束，最终三入口通过。初始与最终 binary、日志及报告分别留存；新成功不推断未捕获旧超时的唯一同因，不报上游。

## 新固定 head 官方门槛

本文件在发布前记录接线与本机范围，官方资格仍 pending。新批须自己的七actual+另draft条件skip，两平台67普通Python、Linux26实际Chromium（原23+新3），四候选各13完整唯一源码/归档安装/认证artifact API digest，继承171合同与新增PDF guard共172实际 sites。Mac不跑Chrome；候选不跑PDF/反馈API/model_routing/Chrome，不能借父140或本机补齐。

新 guard 核唯一完整三条PDF Actions命令、Run/ANSI一致、六20字段 typed 收据/六UUID、重建全文 prompt/reply SHA、各入口顺序唯一许可证/拒绝/元数据/owner/end标记。driver合成反例只验证审计器，不当产品执行。全绿后独立 fresh 官方 Standards/Spec 各自行认证 API/terminal archives/四artifact对象及172 trace，才仅正文/pin回填；不提交发布后自引资格文件或造新CI。

全部个人Agent里程碑尚未充分验收。正式供应商/publicTLS/计费代理/公开安装/经验质量/自动学习/Agent浏览器/审批与StateKnot durable未由本批认证。原文件、发送后模型消息/会话及备份隐私边界见[功能合同](pdf-task-input.md)。
