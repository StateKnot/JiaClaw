# 个人 Key 文件移动：源码验收范围

本批在已合格 PR153 之上，接通用户明确选择的原 `move`：管理员双端精确源→目标授权、单链接普通文件、同卷原子不覆盖，成功移除源名称。使用及风险见[合同](tenant-move.md)。不新增工具或文件读取引擎、Store、模型调用路径。

源码固定点为 local parent `280932753003e33831fab9cbecce8a935a6263f2`；本批源码 `f949dea5b6974adcac5c15b5e8d77b0f56548d5b`，tree `43ddc7f85501730eb7065a7f49418ed9d67377e1`。19条源码路径包括新权限、原 rename 的受限分支、双端握手/请求身份、工作台及真实验收。累计 origin/main `4b2357fcf01f47ba08d7724edbba7accfb60972c` 仅历史继承，不称穷尽累计 fresh 重审。

125个完整 compiler inputs 的规范清单 SHA-256 为 `a0f270bcfc6ce247bed5aac3f405dc400067e322f2ec061c29455c3f5a614f60`；冻结源码二进制与 target/debug/jiaclaw 均为 `ed2e974e9f5863d72dbe44aaf16779c2eca9c283fe731fa084d5b55061809305`。源码之后的交付仅文档，逐输入与冻结二进制再核对。

实际本机完整 Rust 1337项通过（488/123/726）、1项忽略、4线程；locked build、fmt、required correctness+suspicious Clippy 均通过。215项非必需 host-test Clippy warnings 原样保留，不称 warning-free。冻结父二进制实际运行新增用例，在 gateway user-add 阶段拒绝新配置；原脱敏 generic 错误与退出1保留，尚未派发模型/工具，不冒称父 native 配置已识别新权限。首次完整编译发现四处旧测试 Backend 构造缺新增字段，日志与失败退出单独保留，补齐 None 后完整通过；不是运行期失败或上游缺陷。

11条实际相关命令均通过：新普通 move API、新 move Chromium、原 copy Chromium、原 workspace_mutations、workspace_copy、HTTP turns、tenant HTTP turns、tenant HTTP stream、只读 Key、tenant file write、18组纯 parser。共两次真正 Chromium invocation，Playwright1.62.1 使用本机已有 custom Chromium153.0.8010.12（1243），不称使用缺失的 default1234 分发。没有在本机重跑完整74条 ordinary Python、46次 Chromium、优化/安装/打包或 Docker；新固定 head 官方资格独立。

新普通/浏览器用例各自两个实际用户，以原模型→原工具→原收据链移动完整二进制文件；分别核 source 名称消失、目标全字节 SHA 与 inode 保持，每原任务两次模型 POST、一个 move 工具名。另以67108865字节稀疏普通文件核原 inode rename，不套用 copy 的正文预算。已有目标、错组合、别名、overwrite、缺失源、目录、symlink、hardlink、FIFO均真实拒绝，受保护文件不变。writer竞争与取消保留原 Unknown hold 至明确 fixture 审阅；双端改权先于新身份/hold/模型请求拒绝，旧 UUID 事实零重派发，readonly、跨用户与撤 Key 原权限保持。

实际页面默认不勾选、完整组合换行与375px布局、原 SSE/GET、只读原事实、换身份清私人 DOM/选择、真实迟到 Alice Crypto 切 Bob、错误 descriptor 摘要、浏览器 storage0/pageerrors0分别通过。工作台测试不是 Agent 浏览器工具或真实供应商/业务质量认证。所有模型/文件属于本任务一次性本地 fixture，未使用付费凭据。

外部审计器在原189个 Assert AST（含重复计数）与其余 AST 合同之上新增一个 move guard=190；八个旧 RunPython 计数只排除新增命令而保留原历史子集，新 guard 另要求当前74普通套（73 RunPython+named Currentfilecapture）及 Ubuntu 原45+新1实际 Chromium。新6个 noncontainer Rust/18组 parser；两主 test 自己完整六组 API、Ubuntu独立三组 browser、严格 typed 双用户全物理 SHA/权限 SHA/五 UUID、原命令/API成功步骤/UTC/行尾/顺序/全局计数均须由自身新 fixed head 的实际官方日志核对；当前仅完成审计器静态及隔离反例检查，官方资格仍待执行。candidate只执行自己 Rust/parser及原13源码/安装归档/Currentfilecapture，不冒称执行 move API 或浏览器。

Standards 与 Spec 以固定源码独立审查，原始报告分别保留：[Standards](reviews/2026-10-16-tenant-move-standards.md)、[Spec](reviews/2026-10-16-tenant-move-spec.md)。Spec自有12项冻结二进制检查另计，包含原操作系统拒绝读取的 chmod-000源仍能rename，Unix socket和链接祖先拒绝、精确权限边界与默认关闭；两次自有fixture准备错误分别保留后关闭，不算产品 finding 或根运行。源码、本机、审计器、交付、发布身份与 fixed head 官方资格分开；不会借 PR153 或本机前缀补官方资格，不提交自引 head 资格文档制造 CI 循环。

共享 target 超20 decimal GB，已核真实路径、Cargo CACHEDIR.TAG、Git不跟踪及 metadata lsof，按同仓库/工具链/profile复用保留；后者不证明全树无活跃构建。逐命令先 df，Rust incremental/dev-test debug均0且保留原断言/溢出检查。没有复制/清理 target，实际回收0；APFS du或其他进程 df 变化不算本任务释放。日志、typed receipts与冻结二进制在 target 外的 `.jiaclaw-evidence/round177-next/`。

全部个人 Agent 里程碑仍未充分验收；whole physical release artifact ZIP、正式供应商/public TLS/付费代理、实际公开安装、真实业务质量/学习/Agent浏览器/StateKnot durable 均未由本批认证。原失败的未知历史根因各留原证据，不由新 GREEN 外推同因修复。
