# 租户目录任务验收范围（2026-10-14）

本批从 PR147 的本地交付 `b4a47c2e9a0ad4ee4e112dbf0e9e6709162319fc` 继续；父 fixed head 的 full180 资格只作历史。累计 `origin/main` 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`，本批未声称穷尽累计重审。新增接线消费已有 `WorkspaceTreeTool` 和原目录 capability walker、八槽 I/O、原 native 整批预检、HTTP 身份、网关 hold 和工作台；不另建工具、读取引擎、Store 或网络入口。[合同](tenant-tree.md)。

## 源码与实际执行

源码初始 `6cee97a82946f855a8691e6658cb6982ad1a4a6b` 的17条 fresh 路径独立审查，随后 `cd2287173ddeb8827d58010c1f22cb7f2d6f8034` 只移动 CI 浏览器步骤到原 Playwright/Chromium 安装之后。初始两轴未识别该依赖顺序缺口，root 在发布前发现并修订；初报告和后续单路径独立闭合报告分别保留。完整120 compiler inputs 在这次 CI 修订前后逐字相同，冻结二进制 SHA256 `5c7ceb4d09a519e075f5750d7b4503447ebc3666883da16965dd6ac753fd9962`。

最终本机1323 Rust（475/123/725，另1忽略、4线程）、locked build、fmt、必需 correctness/suspicious Clippy 均为实际 exit0；210项非必需 host-test warning 留存。初次完整编译因三个旧测试 Backend 构造器缺少新增 `tree: None` 而失败，修订后完整通过；不把初失败当产品运行或上游缺陷。初新 API 夹具误把原取消200预期为202，只有断言预期修正，原产品/期限不变。

新六组实际 Python 验收覆盖：完整双端权限与拒绝启动；Alice/Bob 原模型→原 tree→原收据往返；隐藏名称、非法 UTF-8/NUL 二进制文件元数据、真实 symlink/hardlink/FIFO、不读取正文；stale/missing/duplicate/unselected hash、只读及跨用户；不安全根 NoEffect 和原 truncation；重配 backend 在创建身份/hold/PUT 前拒绝；真实 in-flight 模型取消不再 dispatch tree；撤 Key 后拒绝原事实。两份 strict11-field 收据独立重建完整清单与权限 SHA，原每任务2模型 POST。原模型/服务全部为隔离本机 fixture，没有供应商调用。

另一个实际 Chromium invocation 的三组覆盖默认未勾选、完整路径/移动375宽度、原 tree/SSE/GET 与捕获 policy hash；只读旧事实和身份清私有 DOM/选择；真正延迟 WebCrypto 后换身份，旧摘要不能恢复 Alice 范围，篡改摘要失败关闭；零 storage/page errors/自动 PUT。Playwright1.62.1 消费本机现有 custom Chromium153.0.8010.12（1243），不是缺失的默认1234分发。

同一冻结 binary 的原 filesystem_info、HTTP turns、tenant HTTP、只读 Key、tenant file read/write、skill read、memory search 八套进程和 parser13 另实际 exit0。没有本机重跑全部69 ordinary Python/41 Chromium、优化、打包安装或 Docker。冻结父功能前置负例在 gateway 配置解析拒绝新权限，保留 exit1；不是重造父资格。

独立 Spec 另用双前缀相邻批准根、真实 capability relay 的遗漏/预算变化/多字段、非整数 depth/越界/子路径拒绝；两端停服撤回权限后旧 GET/相同旧 PUT 只返回原事实且模型不加，新的 tree 准入拒绝。Standards 自身执行新六组普通 API。两轴各完整120 inputs/冻结 binary 绑定与新增范围审查最终0；报告按初始和单路径闭合分开，不冒称 root 重执行。

独立报告：[Standards 初始](reviews/2026-10-14-tenant-tree-standards-initial.md)、[Spec 初始](reviews/2026-10-14-tenant-tree-spec-initial.md)、[Standards CI 顺序闭合](reviews/2026-10-14-tenant-tree-standards-final.md)、[Spec CI 顺序闭合](reviews/2026-10-14-tenant-tree-spec-final.md)。

## 自身官方资格门槛

本文件是发布前交付快照。新 PR 的固定 head 仍须自己的七 actual 作业（另条件 draft skip）、两平台各69 ordinary Python、Ubuntu41 actual Chromium（原40+新1）、四优化候选各13完整唯一 source/install/archive 和认证 artifact API digest、独立 Currentfilecapture step，完整181检查后再独立官方 Standards/Spec。Mac 不跑 Chromium，候选不跑新目录 API/Chromium 或材料/反馈/modelrouting process；旧65/66/67/68与 browser_suites12/40 Chromium 只是历史子集，不补本批资格。父、本机、源码/交付/发布两轴及日志前缀不能转移该资格。

新审计 guard 绑定每个非 container 自己两个 tree Rust marker；两主 test 的原始 completed-successful API step、唯一 exact Actions 命令/真实 UTC/ANSI、六个完整有序 PASS、两用户 strict11-field/完整清单与权限 SHA 和唯一 UUID；Ubuntu 独立 browser step 的三 PASS、strict6-field 回执/唯一端点与全局顺序。原180 assert AST（含重复计数）保留，只给旧 Python 子集显式排除新增 step，另增强本批总数；产品/夹具预算保持。初外部审计器被独立 Standards 证明接受别块额外 browser 回执，原反例和初 driver 保留；最终新增全局回执/marker计数与原 workflow 顺序。此类 synthetic driver 检查不是产品或官方执行，不报上游。

任何实际 CI 失败先留 job/check-run/annotations/完整日志和阶段，再定位 owner；不盲重跑或放宽原40分钟 Ubuntu/60分钟 Intel、3秒 header、native10秒、10+5关闭15秒、15秒grace、250ms、资料5秒、session export/comparison15秒等期限。完整后只在正文/pin回填，不提交自引 head 资格文档造成新 CI。

不认证管理员目录声明真实性、独立 OS ACL、原子目录快照、物理断电、Agent 浏览器、经验/业务质量、学习、StateKnot durable、真实供应商/公共 TLS/计费代理或公开发行安装。目录名称/大小可能进入本用户配置模型与原会话；原 ambient memory 保持。共享 target 约20GiB已按同仓库/工具链/profile评估保留，所有 Rust 命令关闭增量与 dev/test debug；不复制 target，无 task target 清理，实际回收0，APFS du和其它进程 df变化不作释放量。
