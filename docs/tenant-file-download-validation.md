# 当前授权文件交付接线验收

本批以 PR141 已完成资格为历史，新增个人用户取得工作区真实文件字节的能力。它消费原 FileReadTool / WorkspaceReadFileTool / memory_io，原模型回复文本下载保持。累计 origin/main 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`，本批 fresh 范围为新下载接线及受影响的原文件读取/工作台，未累计穷尽重审。

## 固定源码与实际本机范围

最终源 `16f95f0aab3e51c3b616c19a2997f1bcc487e236`，生产源 `52c75153767819102d351da13f81cfa77147a991`；两者仅一行夹具加强区别：原文件先设不同旧字节，再由真实模型的原 file_write 生成新字节。114完整嵌入/编译输入 manifest `e07240585a0b747588b162c3c4b3153b4814046f9867a98c96050b55d7e9e5b8`，冻结与target二进制均 `6ae340f1edf81300b106814fb5bbff2c89c3952e85cc32b8f41f6cc4a0fadaff`。初110路径启发式清单遗漏四个既有配置/OpenAPI输入，原清单与校正记录留证；最终按父113完整清单加入唯一新模块重建，不改变二进制。

完整本机1320Rust（473/123/724）另1ignored、4线程，以及 locked build、fmt、必需 correctness/suspicious Clippy 全0；209项非必需host-test warning保留。初编译漏四个测试 Backend 字面量的新字段，原错误保留后显式补false；不是原产品运行失败。命令环境incremental/dev-test debug均0，原断言与溢出检查保持。

新七HTTP/OS组与一个实际Chromium四组通过。实际双用户原任务各2模型POST、原file_write单工具，把不同旧内容变为新文件；下载另0模型/0准入身份/hold不变。原33/31字节BOM、CRLF、TAB、完整中文emoji与物理文件SHA对应；零字节/精确262144接受，262145/invalidUTF8/NUL/链接/hardlink/FIFO/目录/缺失整次拒绝。十种真实backend响应头/正文篡改与读取中撤Key拒绝，原GET恢复不重写/重放。

四个真实受控原读取线程在客户端断开/5秒等待超时后继续持四个native控制许可；第五拒绝，独立Bob可用，原线程实际释放后恢复。不是八读取并发证明。真实Chromium从原显式file_write任务，经新GET把完整35字节保存到物理下载文件；readonly重载再取同字节。默认未选/未确认、三种篡改、真实迟到GET及WebCrypto身份切换、375px、零storage/page errors通过。浏览器只报告请求下载，不认证用户最终保存位置或历史任务产物一致性。

另十二外层命令实际回归原workspace_files、file_read/write（各一Chrome）、feedback（API8+三Chrome）、三PDF入口、http_turns/tenantHTTP/stream/readonly及parser13，全0。最终选定的本机验收矩阵共9实际Chromium：新1、旧8；此前准备/初始阶段的实际执行另保留，不合并为最终矩阵重跑；独立Spec自身执行单独留证，不能借为根重执行。Playwright1.62.1使用已有custom Chromium153.0.8010.12（1243），不是缺失的默认1234。未本机重跑完整68普通Python/完整27Chromium/优化候选安装打包/Docker，不补官方资格。

新夹具首次错误查询http_requests（实际表http_turn_requests）/完成七组后临时目录清理异常/将原文件授权降为不支持的gateway协议1/Playwright APIResponse接口误用各原日志与校正保留。原协议2、生产字节、任何原断言/期限不减；不报上游。父58a不支持新开关，首次新夹具在CLI配置解析前置拒绝，模型0，只说明功能不存在，不是父资格回归。

## 新固定head资格门槛

本文件是发布前接线记录，新的官方资格pending。新head须自己的七actual加draft条件skip、两platform68普通Python（旧Run前缀67仅继承subset）、Ubuntu27真正Chromium（原26+新1）、四候选原13完整唯一源码/安装归档/认证artifact API digest，加独立Current file capture acceptance与Ubuntu Current file download browser acceptance，原172合同及新增两guard共174。新capture步骤与原40 Verify命令/16 Browser命令分开，未改父断言。Mac不跑Chrome，候选不跑PDF/feedback API/model_routing/Chrome。

新guard核唯一完整exact Actions命令/Run与ANSI一致、七顺序唯一HTTP/OS marker、严格九字段主收据/每用户九字段及全局唯一UUID/完整物理SHA、四浏览器marker/十五字段收据及结尾。初driver824a对普通Mac/候选段额外出现完整浏览器证据仍接受，独立Spec P3反例保留；最终06b5213拒绝所有非浏览器mode的额外浏览器前缀，原172 Assert AST与产品/夹具/期限不变。初Standards0只是该阶段记录，最终两轴另闭合。审计器合成反例只验证driver，不算产品/官方执行。官方全绿后独立fresh Matt Standards/Spec各自认证API/terminal archives/四artifact对象/alias若有/174 trace，才正文与pin回填；不提交发布后自引资格文档制造CI。

共享target超过20GB已核realpath/CargoTAG/无Git跟踪/打开锁后按同仓库工具链profile复用保留，无整个target复制/新专用缓存/清理；实际回收0。df变化与APFS du不充当释放量。所有日志与冻结binary在target外的Git本地排除持久目录。

本批不认证供应商/publicTLS/paidproxy/实际公开安装/经验质量/自动学习/Agent浏览器/审批与StateKnot durable。个人Agent全部里程碑尚未充分，继续30分钟ACTIVE；隐私、当前版本与权限边界见[功能合同](tenant-file-download.md)。
