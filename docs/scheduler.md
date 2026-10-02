# 持久化定时任务

JiaClaw 可以将多个 cron 或固定间隔任务保存在 SQLite，在 `jiaclaw serve` 运行时调度。每个任务有独立会话和运行记录，使用配置好的 Agent、工作空间和模型提供商。结果保存在会话及运行历史中；当前任务不自动向 Telegram、Slack 等渠道发送通知。

## 启用

```toml
[scheduler]
enabled = true

[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"
```

还必须通过 `JIACLAW_API_TOKEN` 或 `http.api_token` 配置 API Token。后台执行只接受 `provider_type = "brokerrouter"`，或供离线验收使用的显式 `stub`；`openai_compatible` 旧路径不能用于调度。调度器和旧 `heartbeat.enabled` 不能同时开启；迁移时关闭旧 heartbeat、启用调度器，再将原提示词创建为定时任务。服务在配置不满足要求时启动失败。调度器默认关闭，数据库必须使用工作空间之外的 SQLite 路径；沿用会话存储的独占进程锁。

开启后可使用同源 REST API 管理任务。下面命令假定已在环境中设置 API Token：

```sh
curl -fsS http://127.0.0.1:8080/api/jobs \
  -H "Authorization: Bearer ${JIACLAW_API_TOKEN}" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "每五分钟记录时间",
    "prompt": "调用 datetime_now 并简短记录当前 UTC 时间。",
    "schedule": {"kind": "cron", "expression": "*/5 * * * *", "timezone": "Asia/Shanghai"},
    "enabled_tools": ["datetime_now"],
    "timeout_secs": 30
  }'
```

创建响应是一个 Job 对象，其中 `id` 是任务 ID，`session_id` 为 `job:<任务 UUID>`，`next_due_ms` 为下一次计划执行的 UTC Unix 毫秒时间。任务配置的 `enabled_tools` 必填、非空，表示这项后台任务允许使用的工具，不能继承前台请求“空数组允许全部工具”的语义。

## 时间语义

支持两种 `schedule`：

```json
{"kind":"cron","expression":"*/5 * * * *","timezone":"Asia/Shanghai"}
```

```json
{"kind":"interval","seconds":3600}
```

cron 固定为五字段“分钟 小时 日 月 星期”，时区必须是 IANA 名称，例如 `Asia/Shanghai` 或 `America/New_York`。同时限制日期和星期时采用传统 OR 语义；不支持六/七字段、`+` AND 扩展或以缩写代替完整五字段表达式。interval 范围为 1 至 31,536,000 秒，按 UTC 时间推进，不受夏令时影响。

夏令时和时钟变化使用明确政策：

- 春季跳时导致本地时间不存在时跳过，包括时区规则导致整日不存在的情况。
- 秋季重复的本地时间仅取较早的 UTC 瞬间一次，不在第二次重复时间再次触发。
- 下一次执行必须严格晚于计算基准的 UTC 时间；重新启用任务也不重访已经领取过的 UTC occurrence。
- 查找未来执行点最多检查 8 个日历年和 4,096 个候选；无法在边界内找到执行点的表达式被拒绝。

时间计算使用固定的 `croner 4.0.1`、`chrono 0.4.45` 和 `chrono-tz 0.10.4`；后者包含 tzdb 2025b。时区法规更新需要升级依赖并重新验证，运行中的系统时区不会替换该固定数据库。

服务停止期间错过的调度不追补。启动时将已过期的待执行点推进到未来；正常运行中晚于计划时间超过 5 秒的 occurrence 也跳过。容量占满或同一任务仍在运行时，不额外排队重叠执行；不会把积压按分钟或秒逐个补跑。被跳过的过期时间点推进后不会创建模型请求。

## 管理 API

所有接口都要求同一个 API Token。列表直接返回数组，单项返回 Job 或 JobRun 对象。`GET /api/jobs?include_deleted=true` 包含软删除的任务，便于检索 ID 后显式清理；默认列表隐藏它们。

| 方法与路径 | 行为 |
|---|---|
| `GET /api/jobs/status` | 查看调度器运行状态和 `max_concurrent_runs: 4` |
| `GET /api/jobs?limit=50&offset=0` | 分页列出未删除任务；`include_deleted=true` 时包含软删除记录；limit 为 1–100 |
| `POST /api/jobs` | 创建 JobSpec，创建后启用 |
| `GET /api/jobs/{id}` | 查看配置、启用状态、下一次时间及会话 ID |
| `POST /api/jobs/{id}/pause` | 暂停后续调度 |
| `POST /api/jobs/{id}/resume` | 从未来时间恢复调度，不重放历史运行 |
| `GET /api/jobs/{id}/runs?limit=50&offset=0` | 查看最新优先的运行记录，limit 为 1–100 |
| `DELETE /api/jobs/{id}` | 软删除并禁用，保留运行审计 |
| `DELETE /api/jobs/{id}?purge=true` | 显式删除已软删除、且没有 running 运行的任务及其审计历史 |

pause 和软删除阻止后续调度；已经领取的工作以该运行的最终状态为准。软删除任务不能 resume。当前没有原地编辑接口；需要改动计划或授权时，暂停/删除旧任务并创建新任务。`purge=true` 是管理员主动删除审计记录，调用前应完成必要核对或备份；它不会自动删除该任务的会话消息。

JobSpec 的 `name` 为 1–128 字节，`prompt` 非空且最多 32 KiB，`enabled_tools` 为 1–32 个不重复工具名，`timeout_secs` 为 1–600 秒，默认 120。后台工具必须已注册，且属于 `datetime_now`、`json_query`、受控 Docker `exec` / `shell_exec` 或经过审核的 `mcp_` 工具。当前不允许 `file_write`、`file_read`、`http_get` 等缺少可靠取消边界的旧工具进入后台任务；未知或不允许的工具在创建时拒绝。

`/api/jobs/status` 的 `state` 为 running、failed、stopping 或 disabled。后台存储/worker 致命错误会停止准入并报告 failed，创建和 resume 返回 HTTP 503；管理者应检查日志、修复故障并重启 serve，不要把 HTTP 服务仍存活理解为调度器仍工作。

## 领取、结果和中断

数据库事务先领取 occurrence 并记录 `running`，再调用 Agent。数据库约束保证同一任务不会同时有两条 running，同一任务与计划 UTC 时间不能重复领取；全局最多并发 4 条任务。运行记录保存领取时的完整 JobSpec 快照。

| 状态 | 含义 |
|---|---|
| `running` | 已领取，尚未提交终态 |
| `completed` | Agent 返回已完成；会话消息与运行结果在同一 SQLite 事务提交 |
| `failed` | 运行失败；任务暂停，需检查后再恢复 |
| `needs_review` | Agent 返回部分完成、工具错误或结果需要人工核对；任务暂停，不自动重试 |
| `interrupted` | 超时、关闭期间取消，或重启发现没有提交终态的运行；保留原运行 ID，暂停任务 |
| `skipped` | 运行领取后未执行的记录；普通过期时间点跳过只推进计划，不创建运行记录 |

任务 timeout 覆盖等待会话锁、准备上下文、模型与工具循环。超时取消本地等待不代表供应商或外部工具没有执行；执行超时、异常与不确定结果不会自动重放。正常关闭先停止新领取，在 `http.shutdown_timeout_secs` 内等待运行结束；超出宽限后取消本地工作并记录 interrupted。进程被 SIGKILL 或主机掉电后，下一次启动将残留 running 标记为 interrupted，并暂停对应任务。管理员必须查看工具/网关证据后决定是否 resume；resume 安排下一次未来执行，不是恢复旧请求。

这里持久化的是调度和结果记录。模型操作 ID、供应商账务、外部副作用仍需要 [StateKnot durable 接线与 Brokerrouter 合同](brokerrouter-gaps.md) 才能做跨系统恢复；不能将本地单次领取解释为外部效果 exactly-once。真实供应商默认认证仍受 [Brokerrouter #31](https://github.com/StateKnot/Brokerrouter/issues/31) 约束。

## 保留与验收

任务上限为 100，包含软删除记录；每个任务保留最多 100 条运行，总运行记录上限为 10,000。系统只自动淘汰较旧的 completed、failed、skipped；needs_review 和 interrupted 不自动清除。审计记录占满上限时暂停该任务，要求显式核对与清理，不删除未知效果的证据来继续执行。

构建后运行真实进程验收：

```sh
cargo build --locked -p jiaclaw-host
python3 tests/scheduler.py target/debug/jiaclaw
```

该脚本使用本地 HTTP 网关 fixture、真实 JiaClaw 二进制和 SQLite，覆盖配置与认证、cron/interval 创建、工具白名单、暂停恢复与审计删除、运行和会话结果、超时暂停，以及模型请求已提交后 SIGKILL/正常关闭的中断恢复和不自动重放。

它还在一次性测试数据库中注入完成事务失败：验证会话与完成记录共同回滚、调度器报告 failed、原运行转为 interrupted 并暂停、创建和恢复任务返回 503。移除故障后仍须重启调度器；重启不会自动重放原运行。故障仅通过测试数据库 trigger 构造，产品没有故障注入接口。

这些测试不消耗真实供应商凭证，也不能替代目标环境的供应商、容器清理与长期负载验收。
