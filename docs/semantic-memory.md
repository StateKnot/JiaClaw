# 语义记忆

语义检索是显式启用的 Brokerrouter embeddings 消费方。工作区 Markdown 仍是内容来源；`memory_search` 缺省保持关键词检索。启用后可选择 `mode = "semantic"`，用受限本地 SQLite 索引返回来源路径、行号、摘录和余弦分数。它不自动提取聊天历史、不扩大系统提示的文件上限，也不自动在查询期间刷新索引。

本批 CLI、离线 fixture 和文档已接线；运行与跨平台验收状态见[验证记录](validation.md)。合成向量 fixture 不证明真实模型检索质量或真实网关计费行为。

## 配置和授权

在已有 Agent/Brokerrouter 配置中加入：

```toml
[memory]
path = "MEMORY.md"

[memory.semantic]
enabled = true
model = "your-authorized-embedding-model"
space_revision = "operator-frozen-model-revision-1"
dimensions = 1024
sources = ["MEMORY.md"]
index_path = "../state/semantic/index.sqlite3"
timeout_secs = 30
```

`model` 必须替换为本部署虚拟 Key 已授权的 embedding 逻辑模型。只允许 `provider_type = "brokerrouter"`，沿用已配置的网关地址和虚拟 Key（`JIACLAW_API_KEY` 环境变量优先）。开启该功能授权将 `sources` 中的文本以及显式 semantic 查询发送给该网关；关键词搜索不发送 embedding 请求。应用没有本地模型降级、供应商直连或自动重试。

| 字段 | 默认值与约束 |
|---|---|
| `enabled` | `false`；关闭时不创建索引、不调用 embeddings |
| `model` | 启用时必填，使用精确的授权逻辑模型 |
| `space_revision` | 启用时必填，由管理员固定底层模型版本；升级时显式修改 |
| `dimensions` | 启用时必须为 1–3072，返回向量必须精确匹配 |
| `sources` | `[]` 表示仅使用 `memory.path`；最多 3 个工作区相对路径，不自动扫描其他文件 |
| `index_path` | `../state/semantic/index.sqlite3`，相对于工作区；必须最终在工作区之外，并独立于会话/网关数据库；默认放入独立的 0700 子目录 |
| `timeout_secs` | 30，范围 1–120 秒；超时可能已提交，进入待核对状态 |

每个源最多 128 KiB，全部最多 384 KiB；文本按 UTF-8 边界分块，每块最多 1024 字节、不重叠，最多 512 块。向量以 binary f32 存储，在有界内存中做精确余弦比较，不依赖外部向量数据库。网关响应最多 2 MiB，搜索摘录合计最多 16 KiB、结果 JSON 最多 64 KiB。源文件必须是安全路径下的常规 UTF-8 文件，拒绝绝对路径、`..`、符号链接、硬链接和特殊文件。

文档块与查询使用相同原文预处理，不自动添加 query/passage 前缀，因此只适用于已认证的对称文本 embedding 模型。同一个逻辑模型的所有候选端点都必须认证为同一向量空间；维度相同不足以证明可比较。

`space_revision` 是管理员的版本承诺，不会自动探测同名网关逻辑模型背后的权重变化。底层 embedding 模型、维度或分块规则改变时，必须让应用识别为新空间并显式重新索引；不能继续比较不同空间的向量。修改配置后重启。

## 初始化、查询和更新

维护 CLI 直接操作本地索引，不通过聊天模型，也没有公开 HTTP 管理路由。索引由使用它的进程持有终身独占锁；执行这些命令前，先停止使用同一索引的 `serve` 或其他 CLI 进程。只读 `status` 也遵循这个所有权要求。

```sh
jiaclaw memory semantic status --config config.toml
jiaclaw memory semantic refresh --config config.toml
jiaclaw memory semantic search "用户喜欢什么热饮" --max-results 5 --config config.toml
```

`refresh` 显式读取允许的源文件并建立索引，每次 POST 最多 8 个文本块，可能产生 embedding 费用。相同请求内容和空间仍保留经验证的成功回执时可复用；失败不会自动再 POST。每次刷新使用 5 分钟的新请求入场预算：每批发送前为配置的 `timeout_secs` 留出时间，预算不足就停止，并保留已确认批次；显式再次 `refresh` 可以继续。这不是对阻塞操作系统 I/O 的硬截止。CLI 成功输出 JSON，出错返回非零退出码。

服务启动后，模型可在已授权的 `memory_search` 工具中使用：

```json
{"query":"用户喜欢什么热饮","mode":"semantic","max_results":5,"paths":["MEMORY.md"]}
```

查询为非空且最多 1024 UTF-8 字节，`max_results` 为 1–20。`paths` 只能缩小配置的源集合，不能加入新文件或扩大允许上传的范围。工具白名单和 `tools.memory_search.enabled` 仍生效。省略 `mode` 使用既有关键词路径，保持其[文件、查询和结果边界](memory-files.md)。

每次 semantic 搜索先读取并哈希整个允许的源集合。未建立索引、源内容或源集合改变、文件删除、空间不匹配时，返回 `stale_index`，在此之前不请求查询 embedding。相同文件大小和 mtime 不能掩盖内容变化。收到查询向量后还会重新核对源；中途变化时拒绝结果，不返回旧摘录。没有静默关键词降级。

更新 Markdown 后，停止服务并显式 `refresh`，成功后再启动服务。无法验证源文件时应修复文件或配置后再刷新；不要依赖旧索引继续回答。

## 未知提交与恢复

源块 embedding 和查询 embedding 都有持久操作身份。发送 POST 前先写入本地操作账本；提交后断连、超时、响应不完整或进程中断，无法确定结果的操作保持 hold。单个索引只允许一个操作运行；取消工具等待不释放已准入 worker 的执行许可和存储所有权，也不证明请求未提交。该 hold 跨进程重启，并阻止后续 semantic 搜索、刷新和重建。更换同一数据库所用的模型、版本或 Key 不会解除 hold。

先停止服务，查看状态：

```sh
jiaclaw memory semantic status --config config.toml
```

JSON 中 `pending.id` 是本地操作 ID，`pending.remote_id` 是已观察到的网关请求 ID（可能缺失）。有可用网关请求身份时：

```sh
jiaclaw memory semantic recover OPERATION_ID --config config.toml
```

`recover` 只 GET `/v1/requests/{uuid}/result`，验证返回的原始 embedding 回执；不重新 POST。成功只保存已验证回执并解除 hold，不自动发布索引。随后显式执行 `refresh`；相同批次可复用该回执。GET 未能确认完成或返回无效回执时，继续保持 hold。

若没有足够的远端身份或网关无法给出有效回执，管理员必须独立核对请求和账务，再记录结论：

```sh
jiaclaw memory semantic review-clear OPERATION_ID \
  --note "已核对网关请求及账务；记录核对依据和后续处理结论" \
  --confirm-reconciled --config config.toml
```

该命令记录管理员声明并解除 hold，不代替账务核对，不退款，也不证明此前请求没有执行。缺少确认标志或说明时拒绝；解除后新的显式请求可能产生费用。

## 存储、重建和隔离

`index.sqlite3` 使用私有状态目录和文件、独占所有权锁及 workspace/数据库身份校验；拒绝其他用途的数据库、其他工作区的索引及链接别名。不同用户必须使用不同工作区和独立索引路径，不能仅凭 session ID 隔离同一工作区内容。与会话 DB 位于同一私有父目录可以，但不能共用同一个数据库文件。默认 `state/semantic/` 子目录由应用以 0700 创建；已有目录不会被静默改权限。

```sh
jiaclaw memory semantic rebuild --config config.toml
jiaclaw memory semantic refresh --config config.toml
```

`rebuild` 仅清理派生索引，保留成功回执、操作账本和审计记录；存在 hold 时拒绝执行。重建后查询仍返回 `stale_index`，直到显式刷新成功。向量可以重建，操作账本属于防止不确定请求重新计费的持久依据，不能当普通缓存删除。已完成或已核对的历史按容量策略有界保留，未决操作不会被该回收删除；历史成功回执被回收后，再次显式刷新可能需要新的计费请求。

当前 semantic schema 为 2，不兼容或用途不明的数据库会被拒绝。数据库本体上限 32 MiB；打开时还检查 WAL/rollback journal 各最多 64 MiB、SHM 最多 4 MiB。部署的磁盘硬配额需单独设置，这些检查不等于整个状态卷的硬配额。最大容量回归中，512 块 × 3072 维的 generation 为 7,383,192 字节，单个 8 文本回执为 98,320 字节；65 个回执与原子替换时的新旧 generation 共占 21,157,184 字节，低于数据库上限。连续六次原子替换通过，后四次数据库页数保持稳定。

不得通过更换、删除或回滚 `index_path` 绕过 hold。应用能够验证现存数据库和进程所有权，不能阻止拥有磁盘管理权的管理员抹除或回滚持久证据。备份/恢复应停止所有 owner，保留 SQLite 一致快照和操作账本；恢复历史备份可能丢失更新的提交证据，须先完成外部核对。

## 验收范围

`tests/semantic_memory.py` 使用真实 JiaClaw 二进制和本机 embeddings/chat 网关，覆盖 CLI、新进程恢复、原生工具模式、源变化、已提交后 SIGKILL 与未知 hold、GET 恢复、显式管理员核对和私有索引边界。2026-10-03，macOS arm64 最终二进制的 9 组验收全部通过；全量 Rust 773 项、fmt、Clippy correctness/suspicious、锁定构建及既有 e2e/native_tools/memory_io/model_routing 回归通过。传输单元测试另覆盖向量/响应边界，存储 12 项测试覆盖最大容量、原子替换和账本恢复。Linux/macOS 与真实容器 CI 仍待最终提交验证，证据以 draft PR 最终 head 检查为准。

上线前还需在固定网关提交、授权虚拟 Key、模型版本和地域下验证：语义检索质量、实际向量维度、usage/账务、请求 ID 和结果 GET、限额及未知状态核对。测试不调用真实供应商，不关闭 Brokerrouter 真实供应商认证议题，也不代表完成 StateKnot durable Agent 或工具副作用恢复。
