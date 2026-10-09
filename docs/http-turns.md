# 持久 HTTP 请求与结果核对

本协议供单用户 `serve` 实例显式开启，使用已接线的 Brokerrouter 原生工具循环、SSE 收据与授权。JSON 提交返回持久准入记录，随后按同一个 UUID 查询结果；同一次执行也可通过[有界 HTTP SSE](http-streaming.md)接收临时事件与提交后的终态。启用时 capability 的 streaming=true，关闭时false。现有 `/api/chat` SSE 仍在完整回复后分块，Web 已接通本协议，见[工作台流式](web-streaming.md)。[独立用户网关](tenant-http-turns.md)另有显式 JSON 准入合同，租户 streaming=false，不能直接套用 standalone SSE。此协议不代表 StateKnot durable driver、工具自动恢复或真实供应商认证。

## 服务停机后的本机维护

可信本机管理员可直接指定**已经存在的 standalone 会话库**，不需要有效的模型配置或 API Token：

```sh
jiaclaw http-turns --database /srv/jiaclaw/state/sessions.sqlite3 list
jiaclaw http-turns --database /srv/jiaclaw/state/sessions.sqlite3 list --state all --limit 20 --offset 0
jiaclaw http-turns --database /srv/jiaclaw/state/sessions.sqlite3 get ORIGINAL_UUID
jiaclaw http-turns --database /srv/jiaclaw/state/sessions.sqlite3 review ORIGINAL_UUID --confirm-abandon --note '已独立核对原模型账本、工具效果和遗留容器，确认放弃此请求'
jiaclaw http-turns --database /srv/jiaclaw/state/sessions.sqlite3 purge-result ORIGINAL_UUID --confirm-purge
```

四个命令均要求同库 `serve` 已停止，以其相同的独占生命周期锁排斥服务及其他维护者。只接受 schema 11/WAL、Unix 下单链接且无组/其他用户权限的普通数据库与锁文件；拒绝数据库/锁叶子符号链接、FIFO、旧/未来版本、错误 HTTP 表/索引/附加触发器和网关渠道私库。不创建数据库、不自动迁移、不初始化 Agent/MCP、不开模型账本、不做 TTL 清理，也不把查询到的 `running` 自动改为中断。SQLite 读取可能维护 WAL/SHM 辅助文件，不能把只读 SQL 等同于文件系统零写入。数据库父目录和显式路径由本机管理员信任并管理，不提供针对同权限管理员替换文件的沙箱或 inode CAS 保证。

`list` 沿用 HTTP 目录的九字段投影和有限页（limit 1..50 / offset 0..10000）；`get` 返回原私密收据、保留结果和核对备注。输出有 `offline:true`，没有 `active`，持久状态不能证明实际模型、容器或其他外部资源已停止。特别是 SIGKILL 后的供应商请求或容器子进程，须先独立核对；独占数据库锁不证明退款、效果回滚或这些参与方停止。

`review` 需显式 `--confirm-abandon` 和非空、最多 1024 UTF-8 字节的备注；对已失去本机 owner 的 `running` 可记录放弃，对 `needs_review` 保留已有终态。原备注重复为幂等核对，更换已记录的备注拒绝，`completed` 不需核对且拒绝该命令。它不清除模型账本 hold，也不恢复或重放工具。`purge-result` 需 `--confirm-purge`，仅删除终态大结果，保留永久身份、核对和会话历史，仍拒绝 `running`。若输出管道关闭导致进程报错，已经提交的事务不会回滚，应按原 UUID 再 `get` 核对。

## 开启与升级

```toml
[http]
tracked_turns = true                 # 默认 false
tracked_turn_timeout_secs = 300      # 1..=300；包括准入与执行，不为后续轮次重置
api_token = "从私有配置或 JIACLAW_API_TOKEN 读取"
persist = true
persist_path = "../state/sessions.sqlite3"

[model_calls]
enabled = true
store_path = "../state/model-calls/index.sqlite3"
```

还须配置 Brokerrouter 提供商与有效 `tool_timeout_secs` 1..=30。本协议拒绝 gateway-driven scheduler 和 gateway_channel_chat 后端模式；外层网关白名单未开放任何 `/api/turns` 路径。没有认证、SQLite、模型账本或有限期限时启动拒绝，不能默默转为内存/无账本运行。

会话库从 schema 10 原子升级到 **11**，保留已有历史、任务、渠道及身份。新版本校验 HTTP 表/索引的完整声明形状；缺失、替换或附加触发器须先人工检查，不能自动丢弃身份。旧二进制拒绝 schema 11，需升级所有读取该库的 CLI/后端维护工具。升级前停服务并备份整个 SQLite 数据库及 WAL；不要只复制正在写入的主文件。五个私有渠道 store 接受原 schema 10 后迁移和当前 11，仍拒绝其他版本和错误归属。

## 客户端合同

所有入口要求**唯一一个** `Authorization: Bearer ...`；不接受重复 Authorization 或 X-Api-Token。密钥按原有实例配置解析，比较使用固定时间。结果包含原文，只有实例管理员可读；这不是多用户权限协议。

| 方法和路径 | 行为 |
| --- | --- |
| `GET /api/turns/capabilities` | protocol 1、是否允许新增、是否可流式、listing、期限/字节容量与 stream_suffix |
| `GET /api/turns` | 认证后的有界持久身份目录，默认只列待核对请求 |
| `PUT /api/turns/{uuid-v4}` | create-only 准入；首次 202，同规范化请求重复 200；不重放模型/工具 |
| `PUT /api/turns/{uuid-v4}/stream` | 同一 create-only 身份；首次202 SSE，旧身份200 JSON，无重连/重放 |
| `GET /api/turns/{uuid-v4}` | 原准入/终态、active owner 与保留结果的权威快照 |
| `POST /api/turns/{uuid-v4}/cancel` | 先持久记录取消意图，再停止后续派发；当前模型仍按原期限结算 |
| `POST /api/turns/{uuid-v4}/review` | 无活动 owner/turn 写锁时，显式确认放弃未完成请求 |
| `DELETE /api/turns/{uuid-v4}/result` | 仅清理已结束的大结果，永久身份、核对状态和历史保留 |

UUID 必须是小写、标准 RFC4122 UUIDv4。`session_id` 为 `http:` 加另一个同格式 UUIDv4。这一命名空间拒绝旧 `/api/chat`、CLI chat/stream 的模型派发；未解决请求也阻断导入、删除和 TTL 清理。其他会话可继续使用原 API。

请求正文只有以下字段（未知字段拒绝，最多 64 KiB）：

```json
{
  "session_id": "http:95d48cda-4c49-48ae-8c5e-41c010bb7f72",
  "prompt": "读取当前时间",
  "enabled_tools": ["datetime_now"],
  "enabled_skills": []
}
```

prompt 非空且最多 32 KiB；最多 128 个不同、已注册的工具与 16 个不同、已存在的技能。鉴权先于正文读取，提交/核对正文读取共用四个控制槽，最多五秒；超时408、超过独立64 KiB/2 KiB及配置全局上限时413，均不进入持久准入或模型派发。工具必须显式列出，因为原生循环中的空数组含义是全部工具。自动技能关闭。数组顺序、默认技能空数组、session_id 和 prompt 均进入规范化身份摘要；同 UUID 的正文变化返回 409。已有身份的读取优先于当前配置校验，密钥轮换、功能关闭或工具配置变化不会重新执行旧请求。首次准入另存不含明文凭据的配置/认证 epoch 摘要；模型账本另存实际准备正文的精确身份，两者不是同一个合同。

客户端须在发送前保存 request UUID、session_id 和原正文。响应丢失时 GET 原 UUID；查到原记录即可核对。重复 PUT 只返回同一记录，不能以一个新 UUID 自动重试未知工作。未落盘/繁忙/冲突返回明确错误；新请求不得在资源槽中无限等待。

## 终态、人工核对与保留

返回形如 `{"protocol":1,"receipt":{...},"active":true}`。202 中是初始准入快照；随后 GET 判断实际终态。`running` 且 active=false 可能是终态 SQL 失败留下的孤儿，必须人工核对，不能根据旧历史判断“没执行”。

- `completed`：结果与新会话历史已在**同一个 FULL/WAL 事务**提交。`session_committed=true` 指这次历史提交，不保证历史随后没有被管理员删除或 TTL 清理。
- `needs_review`：失败、取消、总预算、进程中断或原生工具需要人工介入。已返回的工具核对回复可与历史一起提交，故此状态的 session_committed 可以为 true；没有最终可信回复时原历史保持原样。
- `result`：最终 reply、原运行 status、routing 和 tool_names；不重复保存工具参数/原结果。模型收据与工具工作区须分别核对。
- `cancel_requested` 是持久意图，不是供应商已撤销、未收费或原生副作用被回滚的证明。

核对模型账本、实际工具工作区和外部效果后，管理员可发送：

```json
{"decision":"abandon","note":"已核对模型收据与实际工作区，放弃未完成请求"}
```

note 为 1..1024 UTF-8 字节，首次记录保留，不允许覆盖成另一条。该动作只解除本会话的应用阻断，不会执行收据中的工具、追加恢复结果、重新发送模型或清除模型账本的未知 hold。当前工具超时可能仍有底层资源 owner；needs_review/abandon 不证明该副作用已停止，底层工作区/MCP 资源锁仍按自身合同保留。应在实际效果确认后再授权新工作。

全库最多永久保留 **10,000 个身份**，不自动驱逐；最多 **32 个保留结果/在途结果预留**，每个结果最多 2 MiB+4096 字节。明确 DELETE result 释放结果预留，旧 UUID 仍永久不可复用。每轮历史最多 4 MiB、最终消息数不超过既有 `MAX_SESSION_MESSAGES`；本协议不自动触发有费用的历史摘要或丢弃旧消息，超限需显式导出/压缩后导入，或建立新 session。容量满返回 409，不能通过 purge 身份绕过副作用去重。会话历史、模型账本、渠道与任务各有自己的存储预算，结果预算不是全实例磁盘配额。

## 生命周期与验收范围

实例只允许 **1 个**活动 HTTP turn（匹配当前单模型账本准入），共享进程流式 owner 也必须有余量；控制操作最多四个真实存储 owner，满即拒绝。实际 blocking SQLite 工作同时持有 turn/容量所有权，取消 HTTP waiter 不提前释放。JSON 客户端失联不会取消已授权的异步请求，取消使用显式入口。SSE 的实际正文 drop 或有界交付失败会取消未来派发；当前模型/存储仍保留真实 owner，原 UUID 的落盘与人工核对规则不变。正文存在时仍占用共享 delivery 槽，不能把客户端 FIN 当成实际 Hyper 正文结束。

取消通知由实际控制 worker 在成功写入原身份的意图后发出，不依赖 HTTP 等待者继续存活。停机先关闭准入，再检查同一个 active-owner 锁；已取得许可、稍后才完成登记的 owner 也会观察关闭状态并停止未来派发。这两个边界有真实 handler/SQLite 阻塞取消和已占用许可/登记时序回归，不以 TCP 写出或简单 sleep 代替实际责任验证。

总期限和服务停机先停止新模型/工具派发，当前已提交模型仍按原 60 秒合同结算；不为结算、后续轮次或 shutdown 重置预算。HTTP owner 保持到当前原生尝试返回及会话事务结束。停机共享原 configured grace，超限后原 UUID 保留；下一次启动在数据库生命周期锁下把 running 标记 process_interrupted，**不恢复模型/工具循环**。额外 checkpoint 不延长宽限期；真正已经派发的 blocking 存储仍持有底层资源到结束。SQLite 模型账本与会话库分离，不宣称跨库 exactly-once。

`tests/http_turns.py` 以实际二进制/HTTP/SSE/SQLite/进程信号验收七组：启动认证与期限，原身份两轮原生工具与重复/冲突/重启清理，外部 SQLite writer 阻断准入及丢失回复，显式取消/总期限后的结算与无预览副作用，失败工具停止后续批次，终态 SQL 回滚/人工核对，SIGKILL/同宽限期停机与 GET-only 模型恢复。另有存储/控制 owner 单元回归。fixture 凭据仅访问脱敏本机协议；真实 Brokerrouter slow-consumer 修复、供应商、HTTP 流式的新增应用验收见[合同](http-streaming.md)，Web/租户/代理与供应商仍分别待接线或认证。

## 遗失编号时查找持久请求

使用同一实例管理员Bearer认证 `GET /api/turns?state=unresolved&limit=20&offset=0`。所有参数可省略，state仅为unresolved/all/running/completed/needs_review，limit为1–50（默认20），offset为0–10000（默认0）；raw query最多128字节，拒绝未知/重复参数、编码、正负号和非规范前导零，鉴权先于检查。响应Cache-Control:no-store，结构为protocol/state/limit/offset/has_more/turns。每项只有id/session_id/created_ms/finished_ms/state/session_committed/cancel_requested/result_purged/reviewed_ms九字段，不包含正文、请求hash、工具参数或review_note。

默认unresolved为running及reviewed_ms为空的needs_review；其他筛选可找到已核对或已清理正文的永久身份。按created_ms降序再id降序，单页来自一次SQLite读取快照；新增准入或筛选状态变化可能改变后续页，刷新第一页再核对，不能把offset分页当永久游标或完整导出。没有总数扫描，最多读取limit+1条状态投影，共享原四槽storage owner，满时409而不等待无限队列；数据库永久容量仍10000，不扩容或清理身份。

目录不包含active，也不证明模型/工具已经停止；选中后必须 `GET /api/turns/{id}` 获取原收据与实际owner。关闭tracked_turns仍可认证读目录和原收据，不开放新提交；网关全权限及只读Key都没有此路由。进程重启后running转needs_review仍能查找，不能由列表自动执行、取消、review或恢复unknown模型账本。
