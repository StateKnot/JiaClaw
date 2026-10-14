# 个人 Key 按文件名找资料

管理员可在后端 `http.tenant_glob` 和网关 `backends[].tenant_glob` 同时配置完全相同的 `{"roots":["tenant-documents/project"]}`，启用原 `tools.glob.enabled` 和 tracked tenant HTTP。两端默认省略，目录 `tree`、文件正文读取及写入各自授权，不互相授予。工作台显示完整批准根，默认不勾选；只有显式选择 `glob` 后发送，模型才能按文件名模式寻找资料。channel/cron 原两工具合同保持。

目录 grant 复用原 tree 的路径规则：1..8 个按 UTF8 字节排序、唯一、互不包含的完整根，每根位于 `tenant-documents/` 下，最多256 UTF8字节/16组件，拒绝隐藏组件、控制字符、反斜线、空或首尾空白/BOM组件、点和父路径别名。授权覆盖整个子树中的普通单链接文件名称，**含隐藏文件名**；不认证目录存在、来源或独立 OS ACL。管理员应先核对完整子树范围。

受限实例仍是原 `WorkspaceGlobTool`，复用同一目录句柄 scanner、glob matcher 和原八槽非排队 blocking I/O。schema 只接受必填 `path`、`pattern` 和可选整数 `max_results` 1..500（默认100）。path 必须完整等于批准根，不能子路径或别名；根在同一次句柄遍历中必须是非链接目录，不能替换成普通文件。pattern 原始完整 UTF8 最多256字节，非空、无首尾空白/控制字符；先验 raw 字节，再沿原 parser。`*`、`?`、`**` 沿原 matcher：不含 `/` 时匹配文件名；含 `/` 时匹配**完整工作区相对路径**，例如 `tenant-documents/project/**/*.md`。不执行 shell 或外部 glob。

原 scanner 不打开正文，跳过 symlink、hardlink、FIFO 等特殊文件和 `.git`；普通二进制文件仍可按名称找到。缺失根、链接父目录、非法 UTF8 名称或访问失败整体报错。保留原相对下降32层、累计2000个扫描条目（包括不匹配项）、2秒协作 deadline、最多500个候选和64KiB完整 pretty JSON；耗尽时 `truncated=true`。结果按已收集路径排序，截断不保证是完整子树的前 N 项，没有自动补扫。原普通 standalone 的单文件根、参数默认和钳制行为保持。

权限 descriptor protocol1 包含 roots、max_pattern_bytes256、max_results500、max_depth32、max_scanned2000、scan_budget_ms2000、max_result_bytes65536，以及 canonical UTF8 JSON `[1,roots,256,500,32,2000,2000,65536]` 的 SHA256。这是权限摘要，不是正文摘要/来源签名/ABA防护。网关启动与每个新准入先完整核对两端 descriptor，再创建身份/hold或发送 backend PUT。只有本次选择 glob 才捕获 `glob_policy_sha256`；旧 None 序列化省略，原请求 fingerprint 保持。撤权或改根需停服务并重启两端；旧 UUID 仍只查询原事实，相同旧 PUT 不重新执行。

UI 使用 secure WebCrypto 核完整 descriptor 和 canonical SHA，迟到旧身份计算不能恢复私人 DOM 或 dispatch。readonly Key 只查旧任务，撤 Key/跨用户沿原拒绝。名称可随显式任务进入本用户配置模型与原会话，原 SOUL/USER/MEMORY ambient 提示仍独立保留。没有第二 reader/search/store/vector、模型工具、后台上传、自动模型请求、poll或浏览器存储。

扫描不是原子快照，也不能硬中断挂起的 OS I/O；取消或 async 超时后原实际 blocking owner 仍持许可到结算。受限工具执行错误为已知 NoEffect，不能清除 unknown hold 或证明其它工具效果。工作台浏览器不认证 Agent 浏览器、经验质量、自动学习或 durable。
