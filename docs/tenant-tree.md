# 个人 Key 的明确目录查看

`http.tenant_tree` 与 `gateway.backends[].tenant_tree` 默认省略。两端同时配置相同 `{"roots":["tenant-documents/project"]}`，后端启用原 `tools.tree.enabled`、tracked tenant HTTP，才能在个人 Key 任务中选择原 `tree`。不扩大 channel、cron、普通聊天或其它工具权限。配置变更需停服务并重启两端；旧 UUID 仍只查原事实，不重派发。

`roots` 是 1..8 个按 UTF-8 字节排序、唯一且不互相包含的完整目录根，沿用原文档路径规则：位于 `tenant-documents/` 下，每根最多256 UTF-8字节、16组件，拒绝隐藏组件、控制字符、反斜线、空/首尾空白/BOM组件及点/父路径别名。授权覆盖该根下**整个目录子树的名称、类型与普通单链接文件大小，包括隐藏项**。授权不认证目录存在、来源真实性、文件正文或独立 OS ACL。管理员必须先核对授权子树；工作台完整显示根清单并默认不勾选。

工具仍叫 `tree`，复用原 `WorkspaceTreeTool`、原八槽非排队 blocking I/O 和目录句柄 walker。受限 schema 要求显式 `path` 且只能等于某个批准根；不能改成工作区根、子路径、父路径或别名。只接受 `path`、整数 `max_depth` 1..32 和 `max_entries` 1..1000；默认深度3/200项。普通 standalone 原 schema/行为保持。链接不跟随，硬链接/特殊项按原 walker 返回 unsupported 且不暴露大小，父目录链接拒绝，文件正文从不打开。

保留原累计扫描2000项、2秒协作扫描预算、64KiB完整JSON、路径长度/深度和输出限制；任何扫描/深度/数量预算使 `truncated=true`，不能当完整目录快照。并发非协作修改可导致拒绝或不同观察，不是原子快照或 OS 硬抢占。取消后原实际阻塞 owner 仍持 I/O 许可直到结算；只读工具错误是 NoEffect，不解除原 unknown hold。

完整 capability 为 protocol1、roots、max_depth32、max_entries1000、max_scanned2000、scan_budget_ms2000、max_result_bytes65536 和 canonical UTF-8 JSON `[1,roots,32,1000,2000,2000,65536]` 的 SHA256。它是权限摘要，不是正文身份或 ABA 防护。网关启动和每个新准入完整核对两端 descriptor；新请求只有选择 tree 才捕获 `tree_policy_sha256`。原未选请求省略新字段，原请求 hash 不变。校验必须先于身份创建、hold和backend PUT。

浏览器原 WebCrypto 核对 descriptor 与摘要后才显示权限。身份切换和迟到 Crypto 沿原 identity 检查，不能恢复旧私人 DOM 或提交。只读 Key 可查原任务，不能新建；跨用户/撤 Key 保持原拒绝。只有原显式发送才调用模型，目录名称与大小会进入本用户配置模型及原任务结果；本批不增加模型工具、读引擎、存储、自动扫描、poll或浏览器存储。原 ambient SOUL/USER/MEMORY 提示仍独立读取。页面是工作台，不认证 Agent 浏览器、业务质量、学习或 durable。
