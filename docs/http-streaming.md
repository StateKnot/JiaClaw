# 持久 HTTP 的有界事件流

单用户管理员 `serve` 显式开启 `http.tracked_turns` 后，可以用 `PUT /api/turns/{原始UUIDv4}/stream` 接收同一次执行的 SSE。鉴权、正文、原会话命名空间、准入/结果预算、模型账本、取消和人工核对均沿用[持久 HTTP 合同](http-turns.md)。不增加上游治理 turn、StateKnot durable、自动恢复或外部写入授权。现有 `/api/chat` 保留兼容分块 SSE；[单用户工作台](web-streaming.md)仅在认证能力允许的新 `http:` 会话使用此入口。租户网关按独立的[个人 Key SSE 合同](tenant-http-turns.md#个人-key-sse-api)开放同一路径，使用配对 protocol 2、固定工具授权和原用户审核占用；个人 Key 工作台目前继续使用 JSON，租户预览 UI 尚未接线。

## 提交和响应

客户端在发送前保存自己的 UUID、`http:`会话 UUID、原 JSON 和显式工具/技能选择；不在 URL、日志或浏览器持久存储放密钥。发送一次与 JSON PUT 相同的正文到 `/stream`，唯一 Bearer Header 先于有界正文读取校验。`GET /api/turns/capabilities` 的 streaming=true仅表示当前实例允许此入口，不是供应商或代理认证。

- 首次202：持久准入已提交，Content-Type为 `text/event-stream; charset=utf-8`。返回 `Cache-Control: no-store`、`X-Accel-Buffering: no`；代理是否遵从这些头须另行验收。
- 已有身份200：application/json原记录，无新订阅、历史预览或派发。JSON与stream共用同一规范化正文摘要；切换路径不创建另一执行。功能关闭后旧身份仍只lookup。
- 其他状态：沿用JSON错误和原认证/64KiB/五秒正文/资源准入合同。不要按事件流解析JSON重复/错误响应。

使用支持自定义Header的 `fetch`/HTTP客户端，不能以无认证的EventSource GET代替这个create-only PUT。没有自动重连、Last-Event-ID、重试指令或新UUID重发；丢失202、EOF、error、超时或不完整done时，只查询原UUID。查不到也须先核对在途准入，不用新身份掩盖未知结果。

## 事件合同

每个SSE事件只有固定event名称和一行JSON data，JSON内event字段与名称一致；UTF-8可跨4KiB正文片段，须增量解码和有界解析。十秒注释keepalive没有业务权威。

| 事件 | 数据与含义 |
| --- | --- |
| admitted | protocol1与原初始receipt快照；不是终态 |
| model_started | 原turn/operation/remote UUID、轮次和固定model；本地已持久化远端身份 |
| preview | 指定round的临时文本，每段最多1024 UTF-8字节；各轮分开显示 |
| model_completed | 原模型收据已结算并落盘；不代表会话/工具/用户交付完成 |
| tool_completed | 原call ID和授权工具名称的尝试已返回；可能失败/超时，不表示效果已成功或停止 |
| done | protocol1与实际原子事务提交后的terminal receipt；必须完整解析再使用 |
| error | 固定http_stream_requires_review；查询原身份，无自动执行恢复 |

`done.receipt.state=needs_review`仍要求人工核对；session_committed可以为true或false。成功收到完整done、模型收据completed、会话事务committed是不同事实。部分大done写入失败时不会在残缺JSON中追加error；EOF须GET原记录。输出失败也可能发生在会话已提交后，不能以“没有读到done”断言没有写入，更不能重放工具。原SQL终态事务失败只给error，原记录仍running/inactive并需要核对。

## 所有权、预算与停止

没有另起原生模型循环：JSON由执行任务排空原八事件队列，SSE由唯一transport task消费它，再通过八个最多4KiB的片段交给实际HTTP Body。实际Body和当前模型共同保留进程四个delivery槽；只允许一个HTTP执行owner，transport task在交付结束前也保留它，blocking存储/当前模型仍保留各自实际owner。客户端FIN不等于Hyper已drop正文，不根据客户端写出或断开时间提前释放槽。

单模型轮预览最多2MiB，整turn最多8MiB，整个HTTP事件正文最多12MiB（包括JSON转义、metadata、keepalive及终态）。metadata最多16KiB、单progress事件8KiB、终态为既有结果预算加16KiB；最终JSON有界序列化，传输分片不要求UTF-8字符边界。片段队列总计最多32KiB，不增加无界转发队列。原native队列发送超过100毫秒会取消该delivery；一个完整SSE帧的排队等待使用同一个五秒期限，不为每片重置。诊断只在完整帧边界有容量时try-send，不再延长宽限。

实际Body drop同步取消未来派发；producer看到关闭、慢读或字节超限也取消，不能因此打断已提交的模型结算或提前释放当前SQL工作。网络内已经写出的预览不能撤回。显式cancel先落盘intent，body断开产生的内部停止不伪造cancel_requested；最终错误/状态由原身份核对。已开始工具按原尝试期限/底层锁等待，超时不证明效果为零。

原turn预算在准入前固定，执行和交付不重置它。transport观察终态最多到原deadline+65秒，包括当前模型的原60秒结算和有限收尾；超过只停止观察，实际owner/存储不伪造完成。服务停机仍共用原configured grace，超限后启动将running标为process_interrupted，无重放。五秒是生产队列的写入期限，不保证内核TCP缓冲已清空、远端用户已读、实际正文已销毁或反向代理已经释放连接；正文槽由实际Body生命周期持有。

## 验收和下一步

`python3 tests/http_stream.py <固定binary>`真实访问二进制HTTP/SSE、原生两轮文件工具、SQLite事务故障、TCP小接收窗/实际backpressure日志、消费者关闭及进程信号。`tests/http_turns.py`继续单独运行原七组身份/恢复。两个Rust边界实际drop未poll的Axum Body和填满有界输出队列，验证执行责任与部分帧语义；全回归/候选资格按本批PR固定源码。

慢TCP可能在会话提交之前或之后填满平台缓冲。前者必须保留needs_review和未改历史，后者保留completed/已提交历史；两者都用原身份查证，不能把缓冲差异改成模型重试。真实代理、浏览器Web、租户扩权、供应商stream+tools和账单未在这些fixture中认证。继续跟踪 [Brokerrouter #31](https://github.com/StateKnot/Brokerrouter/issues/31)、未合并的[#40](https://github.com/StateKnot/Brokerrouter/pull/40)、durable原生输出[#41](https://github.com/StateKnot/Brokerrouter/issues/41)与[StateKnot #140](https://github.com/StateKnot/StateKnot/issues/140)。下一批Web使用显式工具授权、新session命名空间、内存身份、消费者取消和原UUID核对；保留网关/只读/关闭模式的独立授权，不从404推断完整权限。
