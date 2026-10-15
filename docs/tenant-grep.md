# 个人 Key：按字面量查找资料正文

管理员在 native 与 gateway 的同一 backend 分别设置相同的 `tenant_grep`，并启用原 `tools.grep`。两端都需 tracked tenant HTTP。默认关闭；配置中的根是完整工作区相对目录，不是文件或任意子路径：

```json
{"tenant_grep":{"roots":["tenant-documents/reports"]}}
```

native 配置位置为 `http.tenant_grep`，gateway 为 `backends[].tenant_grep`。沿用 tree 的 1..8 排序、唯一、非重叠根与 256 UTF8 字节/16 组件规则；隐藏组件、控制字符、反斜线、首尾空白、BOM、点父路径与重复斜线别名拒绝。配置不探查目录存在性。它独立授权整个目录子树中的普通单链接文件**正文，包含隐藏文件**。撤权或改根需停并重启两端。

个人 Key 工作台完整显示目录，默认不勾选 `grep`。用户选择后发送，原模型才能调用原工具：`path` 必须为某个完整批准根，`pattern` 为非空字面量，不是正则；原始 UTF8 最多512字节，拒绝控制字符，保留首尾空格。可选 `glob` 最多128原始UTF8字节、非空且无首尾空白/控制字符；不含 `/` 匹配文件名，有 `/` 匹配完整工作区相对路径。可选 `case_insensitive` 必须布尔，`max_matches` 必须整数1..200（默认50）。额外字段、null、浮点数、子路径和根别名拒绝。

复用原 WorkspaceGrepTool、原目录句柄 walker、原 literal matcher 与八个非排队 blocking I/O 名额。原 walker 在同一次打开中要求实际目录，无另一次 stat/正文读取/复查。每文件256KiB、实际总读16MiB、2000所有条目、32相对下降层、2秒协作时间、64KiB完整JSON；摘录每行最多200 Unicode字符再加省略号。达到返回条数或扫描/读/时间/输出预算时 `truncated=true`，不自动补扫。跳过 `.git`、链接、硬链接、FIFO等特殊项、二进制/NUL、无效UTF8、超大文件；这些跳过不单独设置 truncated，故 `false` **不代表所有文件正文已搜索**。缺根、不安全父路径、无效UTF8名称或访问错误整次拒绝。无原子目录快照、ABA/来源真实性或独立OS ACL保证；协作超时/取消不硬抢占线程，实际 owner 持原I/O permit与目录句柄至结算；原搜索不获取协作workspace writer锁，unknown hold不清除。

独立 permission protocol1 完整 descriptor 是 protocol/roots/max_pattern_bytes512/max_glob_bytes128/max_file_bytes262144/max_read_bytes16777216/max_matches200/max_snippet_chars200/max_depth32/max_scanned2000/scan_budget_ms2000/max_result_bytes65536/policy_sha256。SHA256 输入为 canonical UTF8 JSON `[1,roots,512,128,262144,16777216,200,200,32,2000,2000,65536]`，仅表示权限，不是正文哈希、代次或 ready。gateway 启动与每次新准入在 identity/hold/backend PUT 前核完整双端cap；仅显式选 grep 时原 submission 捕获 `grep_policy_sha256`，旧None哈希不变。旧UUID只读原事实，不重派发；readonly不能创建任务，撤Key/跨用户拒绝。

UI先严格核descriptor与安全页面WebCrypto哈希；完整根用textContent，迟到旧身份Crypto不能恢复scope或派发。无浏览器存储、自动poll/模型请求、第二读/搜索/Store/vector、额外模型工具或新网络入口。channel/cron仍原两工具。选择后匹配摘录与文件名可能发往该用户配置模型并保存原会话；文件名查找、tree、精确文件读、记忆检索各自授权，原SOUL/USER/MEMORY每轮ambient提示独立保留。已保存会话/供应商/备份不因断开或撤权自动删除。此工作台浏览器验收不认证Agent浏览器、真实业务质量、自动学习或durable。
