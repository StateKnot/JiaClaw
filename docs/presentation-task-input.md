# PowerPoint 幻灯片文字资料

三个原工作台明确选择一个 `.pptx` 并点击提取，复用原 FileReader 完整捕获与 Office Worker ZIP 校验、原五秒绝对截止、身份/会话/选择代次/原草稿核对。完整结果加入可编辑消息，只有用户选择原工具权限并显式发送才创建原 UUID、调用配置模型、保存原历史/收据。没有新模型工具、读取引擎、存储、上传 API、依赖、浏览器存储或自动请求。

文件1..1048576字节，原文件名128 UTF-8字节与Cc/Bidi/slash限制。ZIP沿原128项、单项4MiB/总声明8MiB、stored/deflate、central/local/descriptor/CRC/实际大小和连续区域核验；拒绝加密、ZIP64、宏、重复/不安全名称。完整解压并核CRC的类型/根关系/主文档/主关系各64KiB、每张幻灯片及其关系各1MiB/64KiB，XML总50000元素；母版、布局、主题、打印设置等其他包项仅结构与声明预算，不读取或认证其内容。

仅标准非宏 PresentationML Transitional/Strict，按主文档 `sldIdLst` 的1..32张幻灯片顺序。完整输出为 `pptx-slide-text-v1` JSON，保存每张编号及形状/段落顺序的文字、制表和换行；完整资料限16384 UTF-8字节，完整消息限32768，exact允许，超限整次拒绝。每张须有非空文字。明确隐藏的幻灯片/形状、图像、图表/表格/嵌入对象、字段、动画/切换、外部关系和未知正文结构整份拒绝，不执行链接或文档代码。仅允许当前幻灯片关系指向布局/备注，不读取它们的文字；母版、备注、批注、样式、位置和视觉阅读顺序不在提取范围，不能用此结果代替原演示文稿。非文字装饰形状可无正文；这不认证可见性、裁剪或整份语义完整性。

readonly、busy、active、无会话拒绝；失败、超时、身份取消均终止真实 Worker并保留原草稿，迟到结果不能恢复旧私人DOM。五秒是协作应用预算，不是OS内存沙箱或主线程硬抢占。同源模块/Worker GET仍会发生，Worker的connect-src为none。

源文件留本机。显式发送后文件名、源SHA和提取文字进入本用户配置模型与原历史，可能增加输入费用；SHA不是来源签名、prompt injection隔离或业务质量认证。现有任务、模型账本、未知hold和所有原预算保持。

格式依据：[Microsoft PresentationML结构](https://learn.microsoft.com/en-us/office/open-xml/presentation/structure-of-a-presentationml-document)、[幻灯片文字与关系](https://learn.microsoft.com/en-us/office/open-xml/presentation/how-to-get-all-the-text-in-a-slide-in-a-presentation)。
