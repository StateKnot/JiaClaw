# 个人 Key 的精确文件移动

管理员在原后端 `http.tenant_move` 与网关对应 `backends[].tenant_move` 配置完全相同的 `pairs`；缺省关闭。原 `tools.move.enabled`、tracked tenant HTTP 与个人完整 Key 均为必要条件。示例：

```json
{"pairs":[{"from":"tenant-documents/inbox/report.bin","to":"tenant-documents/archive/report.bin"}]}
```

1..8 组按 UTF-8 源、再目标排序、唯一且源目标不同。每条完整路径沿用 `tenant-documents/` 文档规则：最多256 UTF-8字节、16组件，拒绝隐藏组件、控制字符、反斜线、父路径、空组件、首尾空白与 BOM 别名。组合不能重配成源集合与目标集合的笛卡尔积。授权不证明文件存在、来源真实性、内容版本或独立操作系统 ACL。

用户必须在本次任务明确勾选原 `move`；默认不选。原工具的受限实例仅接受完整批准的 `from`、`to` 两个字符串，拒绝额外参数、别名及 overwrite。它复用原目录句柄移动、八槽阻塞 I/O 与同一工作区 writer：仅单链接普通文件，同卷原子不覆盖 rename，目标父目录须已存在。目录、符号/硬链接、FIFO、特殊源、已有目标、跨卷均拒绝，无复制后删除回退。不会读取文件正文，不新增 Agent 工具或存储。

移动成功会移除源名称并在目标名称保留原文件内容；任意大小的普通文件使用原元数据操作，不套用 copy 的64 MiB正文预算。原子性仅指同卷名称变更。非协作程序仍可能竞争叶子，授权不是 inode CAS 或多文件快照。提交后父目录同步失败可能已产生效果；错误、超时、停止均沿原 Unknown 效果与 hold 规则，不自动重试、不回滚、不清未知占用。明确取消只停止后续派发，实际线程仍持原 I/O/writer 至结算。

独立 protocol 1 descriptor 为 `protocol,pairs,mode,policy_sha256`；mode 固定 `file_only_no_replace`，SHA-256 输入为完整 UTF-8 JSON `[1,[[from,to],...],"file_only_no_replace"]`。两端启动与每次新准入完整认证能力，先于身份/hold/backend PUT；仅选 move 时提交 `move_policy_sha256`。旧 None 请求哈希不变。改权需停止并重启两端；原 UUID 查询与相同旧 PUT 只返回原事实，不重新执行。SSE protocol 2 不变。

工作台以安全页面 WebCrypto 核对完整 descriptor 与规范 SHA，完整组合通过 textContent 展示。身份切换、迟到 Crypto、只读 Key、撤 Key、跨用户和原预算保持原边界，无浏览器存储、自动轮询、隐式模型请求。路径与移动结果可能进入本用户配置模型及原会话；不授予正文读取。渠道与 cron 仍仅原两工具。这是资料整理能力，不认证 Agent 浏览器、自动学习、业务质量或 durable。

本批固定点为父 local `280932753003e33831fab9cbecce8a935a6263f2`；累计 origin/main `4b2357fcf01f47ba08d7724edbba7accfb60972c` 的既有资格只历史继承。新源码、交付、发布和新 fixed head 官方资格分别验收，不借已合格 PR153，也不重新运行其审计器。
