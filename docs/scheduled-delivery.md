# 定时任务结果投递

定时任务可以把成功结果发送到已配置的 Telegram、Slack、飞书、企业微信或钉钉安装。每次执行的会话、运行结果和待发送消息在一个 SQLite 事务中提交，随后由统一渠道发送器处理。没有 `delivery` 的任务继续只保存运行结果和会话。

## 配置目的地授权

先启用 [持久化调度器](scheduler.md) 和 [可靠渠道](channels.md)，配置 API Token、工作空间之外的 SQLite、平台密钥及 `brokerrouter` 提供商。离线验收可显式使用 `stub`。下面仅展示与定时投递有关的配置；平台凭证应通过环境变量或受保护的服务配置提供。

```toml
[scheduler]
enabled = true

[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"

[[http.channels]]
channel = "telegram"
installation_id = "123456789"
allowed_senders = ["987654321"]
allowed_conversations = ["-1001234567890"]
enabled_tools = ["datetime_now", "json_query"]
scheduled_destinations = [
  { conversation_id = "-1001234567890" },
  { conversation_id = "-1001234567890", thread_id = "42" }
]

[[http.channels]]
channel = "slack"
installation_id = "T0123456789"
app_id = "A0123456789"
allowed_senders = ["U0123456789"]
allowed_conversations = ["C0123456789"]
enabled_tools = ["datetime_now", "json_query"]
scheduled_destinations = [
  { conversation_id = "C0123456789", thread_id = "1760000000.123456" }
]
```

`scheduled_destinations` 默认是空数组。入站 `allowed_conversations` 不会自动授予主动发送权限；管理员必须单独列出目的地。conversation_id 与 thread_id 组成精确匹配对：缺省或 JSON null 表示不指定线程，不等于允许该会话的任意线程。Telegram thread_id 是 topic 的正整数 ID，Slack thread_id 是已有线程消息的 ts。

任务的 `enabled_tools` 还必须是该渠道安装 `enabled_tools` 的子集，并满足后台工具注册和取消要求。创建、恢复、执行和实际发送时都会检查安装、目的地和工具授权；重启后撤销授权会阻止旧队列继续使用它。入站发送者白名单仍用于平台 webhook，这些主动任务由管理 API Token 授权创建。

当前允许 Telegram、Slack、飞书、企业微信和钉钉。Discord 交互 token 有短期生命周期，不能作为持久定时任务的发送凭证；Discord、任意 URL、请求自带 token 或未声明字段都会被拒绝。

## 创建任务

下面示例向已授权的 Telegram topic 发送每小时执行结果：

```sh
curl -fsS http://127.0.0.1:8080/api/jobs \
  -H "Authorization: Bearer ${JIACLAW_API_TOKEN}" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "每小时记录时间",
    "prompt": "调用 datetime_now 并简短报告当前 UTC 时间。",
    "schedule": {"kind":"cron","expression":"0 * * * *","timezone":"Asia/Shanghai"},
    "enabled_tools": ["datetime_now"],
    "timeout_secs": 60,
    "delivery": {
      "channel": "telegram",
      "installation_id": "123456789",
      "conversation_id": "-1001234567890",
      "thread_id": "42"
    }
  }'
```

Slack 的 delivery 使用 `channel:"slack"`、对应的 team ID、channel ID 和 thread ts。Telegram installation_id 必须与 Bot Token 数字前缀一致。请求里只保存授权目标，不保存发送密钥；运行时由对应安装解析平台凭证。

只有完整成功且回复满足发送大小限制的运行才产生发件箱记录。回复最多 16 KiB UTF-8；Telegram、Slack、飞书、钉钉每片最多 2,000 个 UTF-16 单位，企业微信按渲染后的 UTF-8 字节拆分且每片最多 2,048 字节。超过总量或片数限制时整体转人工核对，不静默截断。消息复用各渠道的文字转义、回执校验、超时及已核实的限流政策。企业微信和钉钉的 429 没有已核实的安全重试等待合同，进入 unknown 并暂停任务；企业微信本地发送预算不足时消息保持待发送，详见[企业微信指南](wecom.md)与[钉钉指南](dingtalk.md)。

## 运行状态与发送状态

`JobRun.status = completed` 表示 Agent 结果、会话和发件箱已经原子提交；是否送达要看该运行的 deliveries。发件箱记录使用 `job_id`、`job_run_id` 标识任务来源，`event_id` 为 null。平台入站消息仍使用 event_id；数据库约束保证两种来源恰好存在一种，不能同时存在或全部为空。

同一任务有未解决的出站消息时，不再领取新的运行，也不能 resume。已知 429 可以按持久化冷却期重试，最多 5 次尝试；等待期间不新增一轮模型调用。unknown、明确拒绝或达到重试上限会暂停任务，要求管理员核对。暂停或软删除任务仅阻止下一次调度，已完成并入库的发送计划仍会执行；要停止剩余发送，必须调用下面的 cancel 接口。

发送超时、断线、5xx、损坏回执或重启发现 submitting 时，无法确认平台是否已经接收消息。系统记录 unknown 并暂停对应任务，不自动重发、不重新调用模型。管理员可以提供已送达证据，或明确取消余下消息；恢复调度后安排未来 occurrence，不重放旧运行。这里提供本地持久化与保守恢复，不承诺外部平台 exactly-once。

## 查询与人工恢复

以下接口均要求 API Bearer Token。列表直接返回数组，分页 limit 默认 50、范围 1–100；通过路径中的 job_id 和 run_id 校验记录归属。

| 方法与路径 | 用途 |
|---|---|
| `GET /api/jobs/{job_id}/runs/{run_id}/deliveries?limit=50&offset=0` | 查看该次运行的片段、状态、attempts、下一次尝试时间和回执 |
| `POST /api/jobs/{job_id}/runs/{run_id}/deliveries/cancel` | 人工取消该次运行尚未送达的全部片段；正在执行的 run 或 submitting 发送返回 409 |
| `DELETE /api/jobs/{job_id}/runs/{run_id}/deliveries` | 显式删除全部已 delivered/cancelled 的发件审计，保留运行结果和会话 |
| `POST /api/channels/deliveries/{delivery_id}/resolve` | 用 `{"action":"delivered","receipt":"核对的消息 ID 和证据"}` 将 unknown 标为送达；或 `{"action":"cancel"}` 取消所属运行剩余片段 |
| `POST /api/jobs/{job_id}/resume` | 重新验证当前授权后恢复未来调度；仍有未解决投递时返回 409 |

resolve、cancel 和投递审计 DELETE 成功返回 204。delivered 的人工证据必须非空；这项操作不调用平台。cancel 不会撤回已经送出的消息。恢复前应查看平台侧证据，判断消息是否已发送，再决定如何处理未知记录。

新投递审计接口在 scheduler 关闭时仍可通过 SQLite 和 API Token 使用，便于恢复处理，不必先重新开启任务执行。普通任务 API 仍按调度器配置启用。

不能删除仍有 pending、retry_wait、submitting、unknown、permanent_failed 或 expired 消息的运行来源。软删除任务之后，只有所有关联投递都已 delivered/cancelled 且没有 running 执行，`DELETE /api/jobs/{id}?purge=true` 才能在同一事务中删除任务、运行和关联发件审计。也可以先按单次运行清理投递记录，再清理任务。

每个任务最多保留 100 条运行，总运行上限 10,000，任务上限 100（含软删除）。超出单任务保留范围时，系统可在同一事务内淘汰最旧的 completed/failed/skipped 及其全部已 delivered/cancelled 投递；有未解决投递或 needs_review/interrupted 的记录不会自动清理。正常持续发送不因累计成功超过 100 次而暂停。需长期保留的平台回执和执行记录应在淘汰前自行归档。

发件箱最多 10,000 条，与入站回复共享。执行前每个带通知的运行保守预留 100 条分片空间；空间不足时不调用模型，按调度的迟到/跳过政策处理。核对并清理审计后可释放容量，不会通过删除不确定结果腾出空间。

## 存储故障与验收

SQLite schema v4 为统一发件箱增加任务运行来源，当前 v7 保留所有来源、回执和序列高水位，允许飞书/企业微信/钉钉通知并保存企业微信独立发送额度账本。升级前按现有部署流程停止服务并备份 SQLite；升级使用数据库事务，延续同一文件的独占进程锁。不要用多个 JiaClaw 进程共享同一个数据库。

如果会话、运行终态或发件箱写入失败，整个完成事务回滚。调度器报告 failed，拒绝创建和恢复任务；原 running 由恢复流程记为 interrupted 并暂停，避免重放可能已发生的工具副作用。修复存储后需要重启；仅删除故障条件不会自动重新启用调度器。发送结果提交失败也保留不确定性，由恢复流程核对，不能通过再次调用平台补偿。

```sh
cargo build --locked -p jiaclaw-host
python3 tests/scheduled_delivery.py target/debug/jiaclaw
```

验收使用真实 JiaClaw 二进制、临时 SQLite 与本地模型/平台 fixture，覆盖 Telegram/Slack 原生工具结果、线程、精确目的地和工具授权、401、运行与会话及发件箱原子性、429 防止重叠、unknown 暂停、强杀恢复、人工核查后仅恢复未来运行、撤销授权和审计清理。测试通过一次性 SQLite trigger 模拟完成事务失败，产品没有故障注入接口。

本地 fixture 不代替实际平台安装验收。部署者仍需验证目标 Bot/App 的安装归属、发送权限、真实 thread/topic、账户限流及运行环境的网络与持久存储。底层模型供应商认证和 StateKnot durable 接线的剩余边界仍见 [上游差距](brokerrouter-gaps.md)。

飞书使用同一任务 API 和恢复流程，`delivery.channel = "feishu"`，installation_id 为应用和租户复合身份。conversation_id 是 `oc_` chat ID；thread_id 为 `om_` 根消息 ID，省略时向会话顶层发送。配置和独立进程验收见[飞书指南](feishu.md)。

企业微信同样使用该任务 API，`delivery.channel = "wecom"`，installation_id 为 `CorpID:AgentID`，conversation_id 为单个小写成员 UserID，thread_id 必须为空。入站成员白名单不会授予定时发送权限。配置、额度与真实安装验收见[企业微信指南](wecom.md)。

钉钉使用 `delivery.channel = "dingtalk"`、`installation_id = "robotCode:corpId"` 和单个成员 UserID 作为 conversation_id，保留大小写且 thread_id 必须为空。Client ID 从该安装的 `app_id` 取得，Client Secret 从服务器配置取得；请求中不能指定它们。使用独立的 scheduled_destinations 白名单，既不继承入站成员权限，也不依赖临时 sessionWebhook。每片最多 2,000 个 UTF-16 单位，平台接收回执并不等于已读；详见[钉钉指南](dingtalk.md)。
