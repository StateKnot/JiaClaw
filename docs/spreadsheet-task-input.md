# Excel 单元格资料进入任务草稿

用户在原 standalone、个人 Key JSON 或预览工作台明确选择 `.xlsx`，点击提取，将完整受支持的原始单元格 JSON 加入可编辑草稿。只有原显式 Send 才创建任务并发送给本用户配置的模型。文件名、源 SHA 和提取资料随原消息保存，可能增加输入费用；源摘要不是来源认证或 prompt injection 隔离。文件留在本机；模块 GET 是同源网络请求，提取不上传文件，不自动创建 API/model 请求，不增加工具权限、Store、读取引擎、依赖、浏览器存储或轮询。

复用原 FileReader/资料身份、草稿和选择 epoch、同一五秒绝对截止、WebCrypto 与 Word ZIP/实际 Worker owner。所有结束路径终止 Worker；迟到读取、Crypto、Worker、身份或草稿变化均不能赋值。readonly、busy、active 或无会话拒绝。不是操作系统沙箱或主线程硬抢占。

支持 Transitional/Strict SpreadsheetML，标准非宏主工作簿、唯一内部关系、1–8 个可见工作表、最多2048个单元格。ZIP 沿原单磁盘/128项/完整 central-local-name-flags-size-CRC-descriptor/连续区域合同，每项声明4MiB、总8MiB，源1MiB。只完整解包 types/rootrels/workbook/workbookrels/sharedStrings/worksheet XML，前三类与 workbookrels各64KiB，其余每份1MiB；实际流每块核声明与预算。不解析 styles/theme 或其他非数据包内容，其 ZIP 元数据仍核验。每份 XML 拒绝 DTD/实体、无效 UTF-8、外来 namespace，解析合计50000元素。工作表包名只接受 `xl/worksheets/[A-Za-z0-9_-]+.xml`。不认证其他包项内容。

按工作簿与单元格原顺序输出表名、坐标、类型、原始值和 date1904。shared/inline rich text 展平，ST_Xstring 单次解码（包括CR、转义下划线和UTF16代理对），解码后的禁止控制字符和孤立代理整次拒绝；数字保留原字符串，避免 JS 精度损失。不应用样式、日期、货币或百分比格式，不认证 Excel 显示/视觉/语义完整性。公式、隐藏行列/表（包括 zeroHeight 默认隐藏）、合并、绘图、外部关系、宏与未支持结构整次拒绝，不做部分导入；不读批注、打印信息。完整 JSON 含 framing 前限16384 UTF-8字节，完整草稿32768字节，exact允许，超限原草稿保持。正文 ASCII 控制字符除 TAB/LF/CR 外拒绝，JSON 转义保持单元格字节语义。

验收须真实 FileReader→Worker→原授权/UUID/model wire→物理回复下载，并独立核对源、提取JSON、prompt、model和reply完整SHA。fixture 不认证真实供应商、业务质量、自动学习、Agent浏览器或 StateKnot durable。父PR144完整资格仅历史，新head须自己的完整CI与独立Standards/Spec，不能借本机或父job。

原始单元格与 shared string 语义参考 [Microsoft Open XML](https://learn.microsoft.com/en-us/office/open-xml/spreadsheet/working-with-the-shared-string-table)，转义语义参考 [MS-OE376 ST_Xstring](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oe376/bd0aa042-434a-4ca7-b25f-4e1fd25a954d)。这些参考不认证当前实现；实际验收和独立审查单列。
