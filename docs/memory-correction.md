# 本机记忆审阅与条件纠错

操作员可以纠正或删除已过时的长期记忆，而不让较早的审阅覆盖后来的协作写入。先使用显式配置读取完整快照：

```sh
jiaclaw memory inspect --config /private/config.toml
```

JSON 包含配置的相对 `path`、完整 UTF-8 `content`、`bytes` 和原字节 `content_sha256`。只读取现存普通单链接文件，最多32KiB，不截断、不回退到默认 MEMORY。输出含完整私人记忆，应按原文件权限保管。该快照描述本次读取的字节，不是文件版本号。

将审阅后的完整替换文本保存为工作区内普通文件，然后明确提交两个完整小写 SHA256：

```sh
jiaclaw memory replace --from notes/reviewed-memory.md \
  --if-content-sha256 OLD_SHA256 --if-replacement-sha256 NEW_SHA256 \
  --config /private/config.toml
```

一个 `MemoryEditor` 复用原非排队 workspace writer，持有原目标读取/摘要、候选相对能力读取/摘要及原私有 sync+rename 发布；不拼接独立读后无条件写。目标和候选均限32KiB完整 UTF-8；路径遵循原父目录/链接/特殊文件拒绝合同，不创建父目录或缺失目标。原摘要陈旧、候选摘要漂移、无效摘要和同字节 no-op 均非零退出，不发布、不改变原字节/inode，也不残留本次暂存。空候选是明确清空全文的方式，仍须审阅原/空字节的两个摘要。

成功输出仅含 `path`、`previous_sha256`、新 `content_sha256` 和 `bytes_written`，不回显候选正文。这是本机操作员的磁盘维护入口；不受模型 `tools.memory_write.enabled` 开关控制，不向模型或 HTTP 增加写权限。读取配置后直接进入记忆 Module，不初始化模型、MCP、技能、语义索引或会话存储，不发送请求。

已有 Agent 每轮重读配置的 MEMORY，因此之后开始的轮次可读取新内容；已构建的当前轮提示不被改写。语义索引不会自动刷新或删除已索引内容，须使用原显式 refresh/核对流程。清空 MEMORY 不删除历史会话、模型收据、备份或索引，也不提供全面的隐私删除保证。

协作锁只约束遵守它的 JiaClaw 写入者；不保证外部编辑器、exec 或管理员修改的线性一致性。摘要是内容条件，不检测 ABA 或单调代次。rename 后目录同步错误或进程中断可能已提交，必须先重新 inspect，不能把非零退出当作未写入并自动重试。没有多文件回滚、操作账本、自动学习、来源真实性或真实任务复用质量认证；原文件 I/O、取消与同步边界见[记忆文件](memory-files.md)。

本批验收通过公共 `MemoryEditor` 和真实 CLI/鉴权服务：完整配置快照、双摘要纠错/清空、陈旧/no-op保留、32KiB UTF-8、链接/FIFO/缺失拒绝、OS写入失败、实际持锁和两个竞争写入者、同一服务后续轮提示与独立会话的实际字节观察。模型协议采用本机 fixture，只认证接线，不认证学习正确性或供应商质量。固定新 head 官方 CI 和四优化候选资格须另行完成，不借父 PR125 或本机结果。
