# 本机语义索引新鲜度核对

记忆纠正或清空后，原 `memory semantic status` 只显示账本和 generation 元数据。原 `search` 已在查询 embedding 前检查完整来源，但操作员不必为核对新鲜度发起一个查询。本机 `check` 直接消费同一个 SemanticMemory/Store、分块规则与内容核验，不另建索引或模型工具。

```sh
jiaclaw memory semantic check --config config.toml
```

必须显式给出配置，且原 `memory.semantic.enabled`、`memory_search` 和 Brokerrouter 配置有效。先停止使用同一索引的服务或其他 CLI：原索引进程终身独占锁保持。构造仍使用原打开流程，首次打开可初始化原私有 SQLite 状态；本命令不发布、刷新或删除 generation，不改操作账本、回执和 hold。

Module 的 CLI Interface 是 `SemanticMemory.check_config()`：检查专用打开流程复用原 Store 完整路径/权限、数据库身份、容量、锁和初始化核验，保留崩溃遗留的原 pending 行，不做原操作启动的 pending→unknown 规范化。既有 `open`、status/refresh/search/recover 的规范化行为保持。检查随后消费原 `SemanticMemory.check()`：原非排队索引 admission 持有至实际 worker 结束，原单 worker persistence executor 执行本地核对。实际 blocking worker 持有原 store owner 和非排队 workspace writer，贯穿完整源捕获、generation 核验和返回值构建；等待取消不提前释放 owner。其他协作记忆/文件写入会被原 workspace writer 拒绝，检查也拒绝其他 writer。文件从同一已打开 root capability 相对读取，不重开 ambient root 或已核验叶。

全部允许来源最多三个，每个最多128KiB完整原始UTF8、合计384KiB，最多512块；不截断，不按 mtime/大小代替内容哈希，不自动扫描其他文件。缺失来源按原 manifest 规则记录；空文件与缺失文件不同。链接、特殊文件、父路径、无效UTF8、超限、数据库损坏或匹配 manifest 下的块/向量损坏均返回非零错误，不输出可用结论。

成功输出 protocol_version 1 的 JSON，包含原路径集合、active_space、当前 source manifest/chunk 数、generation 元数据、原 pending 操作身份及下表状态；不含私人全文、摘录、向量、Key、请求体或账务结论。

| freshness | 含义 |
|---|---|
| `current` | 当前允许来源的完整 manifest、块路径/行号/原文/hash 和原向量维度/范数通过同一搜索核验 |
| `stale` | active-space generation 的 manifest 与本次来源捕获不符 |
| `missing` | 本地没有 generation |
| `space_mismatch` | 有其他空间的 generation，不能用于当前配置；不认证其他空间的完整性 |

`search_ready_at_capture` 只在 `current` 且没有 pending 操作时为 true。pending 存在时 `next_action` 为 `reconcile_pending`，必须沿原身份先 status/GET recover 或独立管理员核对；检查不清 hold、不联网核对账务。否则非 current 提示 `explicit_refresh`，当前可用提示 `none`。`local_only:true`、`runtime_applied:false` 不表示已执行任务、恢复提示或刷新索引。

整个 check 不调用 embedding、chat 或结果 GET，也不自动重试/refresh/rebuild/recover。后续 refresh 仍是独立明确操作，可能产生费用；先审阅待上传源与原权限。未使用原模型、MCP、技能、会话或 Agent 初始化入口。没有新增 HTTP 端点、模型工具或租户权限。

结论只描述本次捕获，不能保证之后的搜索、外部非协作编辑或管理员直接改库；hash 非代次/ABA、索引权限非独立 ACL。不认证来源真实性、学习/检索质量、真实供应商、业务经验复用、物理断电或 StateKnot durable。文件和数据库操作没有可抢占的 OS 硬截止。

本批验收需分别记录本机公共 Interface/真实 CLI、原检索回归、独立 Standards/Spec、新固定 head 官方七作业/两平台必需 Python/Linux九 Chromium及四优化候选各13安装归档源码/API摘要；不得借父128或本机结果当新官方资格。
