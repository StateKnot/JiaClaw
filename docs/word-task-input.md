# Word 正文加入任务草稿

用户在 standalone、个人 Key JSON 和预览工作台明确选择一个本机 `.docx`，点击“提取并加入草稿”，审阅编辑后再按原发送按钮。选择文件本身不读取；导入不调用 API、模型或工具，不保存浏览器存储。显式发送沿用原 Key、会话、工具选择、新 UUID、模型账本、收据和回复下载，导入资料不获得工具权限。

复用原 `readMaterialFile`/FileReader，一次完整捕获；与 PDF 共用捕获、SHA、身份和清理 owner。Word 解包在一个真实同源 Worker 中完成，使用浏览器原生 `DecompressionStream('deflate-raw')`，不增加解析依赖、模型工具或后端存储。正文 XML 使用浏览器 DOMParser，不进行 HTML 渲染。新静态模块和 Worker 通过同源公开 GET 加载；Worker CSP 禁止网络，不能说零网络。

## 完整拒绝与预算

- 源文件 1..1048576 字节；文件名沿原完整 Unicode Cc/Bidi/slash/backslash 拒绝和128 UTF-8字节限制。
- 普通单磁盘 ZIP，stored 或 deflate，最多128个唯一项。中央目录、local header、名字、压缩方式、CRC/长度及可选 data descriptor 相符，文件区连续无重叠，中央目录紧邻完整 EOCD；拒绝加密、分卷、ZIP64、未知 flags/压缩方式、不安全路径、重复项和宏项。所有声明解压尺寸每项最多4 MiB、总共8 MiB；不是信任声明来分配解压内存。
- 仅完整解压并 CRC 核对 `[Content_Types].xml`、`_rels/.rels`（各64 KiB）和 `word/document.xml`（1 MiB）。实际流式输出每块先计数，超过声明或该项预算即拒绝，不保留部分文本。其他包项只核 ZIP结构/声明预算，不解压或认证内容。源 SHA 是完整原文件摘要，不是签名或来源真实性证明。
- 必须有标准非宏 Word 主文档 content type、唯一内部根 officeDocument relationship 指向 `word/document.xml`；不解析链接、加载外部模板/字体/图片或执行文档代码。三个 XML 必须完整 UTF-8、无 DTD/实体声明、无解析错误；正文接受 Transitional/Strict WordprocessingML namespace。
- 仅主正文中的段落文字、制表符、显式换行，按文档 XML 顺序连接，每段加 LF。表格单元格内段落可读，但不保留表格结构、自动编号或排版。字段只读保存的文字结果，不计算字段。正文中的图片、嵌入对象、altChunk、文本框、修订及不支持 namespace 整次拒绝。页眉页脚、批注、脚注/尾注不读取，不认证整份 Word 的视觉/语义完整性。正文为空、非法 Unicode 或不支持 ASCII 控制拒绝。
- 完整提取正文最多16384 UTF-8字节，含每段 LF；完整组装消息最多32768字节。精确上限可用，超过整次拒绝，不截取。
- 捕获、SHA、模块加载、解包及 XML/文字提取共用原5秒绝对截止；每次异步返回、提取循环和赋值前核对。身份、会话、选择 epoch 或原草稿改变拒绝迟到内容。所有退出路径终止 Worker，无 fake-worker fallback。计时不是 OS 内存沙箱或主线程硬抢占；有界 XML 的同步 DOMParser 返回后仍核绝对截止。

只读 Key、未选会话、active/busy 保持原禁止。清文件选择不移除已加入草稿；断开清本页私人内容，不删除已有会话、模型方数据、备份或下载。资料中的提示不构成隔离或可信指令。

## 隐私和资格

原 DOCX 留本机。显式发送后，文件名、完整源 SHA 和提取正文作为消息交给本用户配置的模型并按原会话保存，可能增加输入费用。不是 `.doc`、加密/宏 Word、OCR、图片理解、完整排版/批注阅读、自动学习、真实 Agent 浏览器或 durable。浏览器工作台验收和本地模型 fixture 不认证真实业务质量、供应商或公开安装。

实现依据：[Microsoft WordprocessingML 主文档结构](https://learn.microsoft.com/en-us/office/open-xml/word/structure-of-a-wordprocessingml-document)、[PKWARE ZIP 6.3.10 APPNOTE](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT)、[浏览器 DecompressionStream](https://developer.mozilla.org/en-US/docs/Web/API/DecompressionStream)。采用平台解压/XML 能力，不复制第三方引擎。

父 PR142 已独立完整合格，仅历史。新批源码、本机、交付、发布和自身固定 head 官方资格分别验收；累计基准 `4b2357fcf01f47ba08d7724edbba7accfb60972c` 历史继承，不称穷尽 fresh 累计审查。

正文修订拒绝包括插入/删除文字、属性 `*Change`、单元格插入/删除/合并、移动和 customXml 修订范围标记，覆盖 Transitional 与 Strict 主文档命名空间。已有普通 `rsid` 属性不代表未接受修订，不据此拒绝。初次独立两轴发现属性与单元格修订缺口，原始 RED 和报告保留；最终同文件必须整次拒绝并保留原草稿。格式依据：[Microsoft 修订标记说明](https://learn.microsoft.com/en-us/office/open-xml/word/how-to-accept-all-revisions-in-a-word-processing-document)。
