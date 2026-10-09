# 模型调用收据与核对

显式启用后，JiaClaw 在每次 Brokerrouter 聊天补全及摘要提交前保存操作身份，在返回结果前持久化经过校验的响应。未知提交会阻止同一账本的新模型调用。该能力处理的是一次模型调用的收据；它不恢复工具执行、会话、渠道发送或整个 turn。

本批最终二进制的 7 组本机进程验收已通过；Linux/macOS/容器验收以最终 PR 当前 head 的 checks 为准，进度见[验收记录](validation.md)。所有 fixture 使用本机假网关，没有真实供应商计费认证。

## 启用与保存范围

```toml
[model_calls]
enabled = true
store_path = "../state/model-calls/index.sqlite3"
```

默认关闭，关闭时不创建账本。启用只接受 `brokerrouter` provider；`store_path` 相对于 `agent.workspace_path`，最终必须位于模型可访问的工作区外，与会话和语义索引数据库分开。新状态目录创建为 0700、数据库和锁文件为 0600；已有私有目录权限过宽、符号链接、硬链接、特殊文件和不匹配的数据库身份会拒绝。默认使用独立 `model-calls` 子目录，兼容已有 0755 的外层 `state` 目录。当前私有权限合同适用于 Linux/macOS POSIX 文件系统；祖先目录必须由管理员信任，不能允许不受信任用户替换目录或挂载点。不要把该文件加入工作区、共享下载目录或版本库。

账本记录本地操作 UUID、应用内部 turn UUID、用途、会话 ID 摘要、工具轮次、精确逻辑模型、原请求字节 SHA-256、端点/虚拟 Key 的绑定摘要、已知的远端 request UUID 和处理状态。这里的内部 turn UUID 不是 Brokerrouter 治理 turn，也不会建立上游审批身份。发送的 Idempotency-Key 使用已持久化的本地操作 UUID；单轮工具循环中的每次补全有独立调用身份，同一轮内可关联。

原始请求正文不落账本，响应正文会保存。响应可能包含敏感文本、工具参数或模型复述的输入；它是受文件权限保护的私有数据，不提供应用层加密。会话、渠道及其他日志仍按各自合同保存内容，不能将“本账本不保存 prompt”解释为整个应用不保存输入。

`status` 只显示有界操作/审计元数据和 pending；正文仅由显式 `result` 返回。保留最近 128 个操作元数据、8 份响应收据及 256 条审计事件；旧收据被淘汰后不可本地取回，模型请求不会因此重新发送。需要长期证据时，在保留窗口内按部署的数据管理要求归档。审计说明可能含敏感信息，避免写入 Key 或原始业务正文。

## 状态与取消

| 状态 | 含义 |
|---|---|
| `submitting` | 已持久准入，模型请求可能在进行中；不能推出已经发送或尚未发送 |
| `unknown` | 网络/协议/持久化失败，或重启发现未完成准入；同库保留 hold |
| `completed` | 已保存符合合同的模型响应；不代表工具已执行或用户会话已提交 |
| `cleared` | 管理员核对后解除本地 hold；不代表模型请求成功、费用为零或工具已完成 |

同库同时只允许一个模型调用。响应必须有精确逻辑模型、合规的 assistant/finish_reason 和有界 JSON；原 POST 必须返回唯一、规范的非空远端 UUID 头。失败保守进入 unknown，不自动重试收费 POST，不换模型或新幂等键重建请求。即使 HTTP 错误看似可重试，也先保留该次调用的核对状态。

调用方超时或取消等待后，独立的模型收据 worker 保留容量和数据库所有权，继续完成有界 HTTP 接收与收据写入。已经取消的工具循环不会因迟到响应继续执行工具。进程 SIGKILL 或主机故障会中断 worker；下次打开账本将遗留 `submitting` 变为 `unknown`。取消不能证明上游未受理或未计费。

请求与响应各最多 2 MiB；每个 HTTP 请求最多 60 秒、连接最多 10 秒。POST 头/正文与其间的远端 ID 保存使用同一请求期限，恢复最多执行状态与结果两个 GET。账本在发请求前为响应持久分配 2 MiB 空间，数据库页数上限 32 MiB；打开前拒绝超过 64 MiB 的 WAL/rollback journal 或超过 4 MiB 的 SHM。SQLite 同步与预留不能保证磁盘、硬件或管理员操作永不失败；部署仍须设置磁盘硬配额、监控和一致备份。

## 管理命令

所有管理命令要求先停止使用同库的 `serve`，等待其进程退出。CLI 与服务共用独占所有权，不提供公开 HTTP 管理路由。

```sh
jiaclaw model-calls --config /private/config.toml status
jiaclaw model-calls --config /private/config.toml result OPERATION_ID
jiaclaw model-calls --config /private/config.toml recover OPERATION_ID
jiaclaw model-calls --config /private/config.toml review-clear OPERATION_ID \
  --note '已核对上游请求和账单；记录工单编号及处理结论' \
  --confirm-reconciled
```

`status.recent` 从新到旧列出操作，可从中选择本地 operation ID；`pending` 是必须先处理的未知调用。`result` 只读本地保留的完整 Chat Completions 响应，可能含敏感内容；不会访问网络、执行工具或把响应补写回历史。

`recover` 要求原端点和原虚拟 Key，并且本地已保存远端 request UUID。它先 GET `/v1/requests/{id}`，校验精确 `id`、`model`、`purpose="model"`，只有远端 `status="succeeded"` 才 GET `/v1/requests/{id}/result`。上游 GET 结果不保证重复 POST 的 UUID 响应头；如提供则必须匹配已知 ID，正文仍须通过同一响应验证。恢复成功只保存收据并解除 hold，返回 `applied_to_turn:false`。恢复结果里的工具调用没有执行授权，不会追加会话或继续已中断的工作。

远端未知/未完成、结果过期或不可用、权限撤销、审批需求、身份冲突、错误响应都保留 hold。GET 可能发现新策略；应用不会转为 POST `check-result`，因为后者可能启动新的收费检测。不能通过新请求重新生成丢失的旧结果，也不根据响应正文中的某个 `id` 猜测远端 request UUID。

如果 POST 没收到远端 UUID，无法通过本入口自动核对；管理员需要在 Brokerrouter 及供应商证据中独立核实。`review-clear` 只有显式确认与非空核对说明才解除指定 unknown；该操作不访问网络、不修改上游账务，也不重跑原任务。解除后，用户可以明确发起一个新的任务，该任务可能产生新的费用和副作用。

更换模型或 Key 不清除同库 hold，原绑定不符时不能恢复。管理员不得通过禁用功能、换路径、删除账本或回滚旧副本绕过未决调用；应用无法防止拥有磁盘控制权的人抹去事实。备份应停机一致保存整个私有状态并保留未决记录；恢复旧副本后，先核对备份点之后的上游费用及外部副作用，再恢复流量。

## 认证边界

单次 CLI 的[真实流式](cli-streaming.md)使用同一原始操作与收据 owner；schema2保存 streaming 模式，并对 schema1进行保留原始身份/收据/hold 的原子升级。流式原始正文最多4 MiB、完整收据仍最多2 MiB；GET 恢复额外核对完整 envelope 与精确 usage。HTTP/Web 流式、上游资源修复与真实供应商认证仍独立，不由该接线推定完成。

本能力兼容固定 Brokerrouter main 的文本非流式 GET 状态/结果合同，不依赖未合并的 SSE PR #40 或原生 JSON Schema 议题 #41。[上游消费者指南](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/jiaclaw-consumer-guide.md)、[文本网关合同](https://github.com/StateKnot/Brokerrouter/blob/e01ecb94919d992eb0b74b3db00d70742820b4cc/docs/m2-text-gateway.md)。

本地 `completed` 不是费用主账或工具运行检查点，GET 恢复也不认证真实供应商账单、治理 turn 或完整 durable graph。真正流式、StateKnot durable 和媒体消费者的受信 turn/审批/下载链仍待各自交付与验收；当前差距见[Brokerrouter 状态](brokerrouter-gaps.md)。
