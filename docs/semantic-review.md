# 管理员语义索引检查

本批让已审阅记忆的操作员在同一工作台显式核对允许来源和当前索引，消费既有 `SemanticMemory.check`；不另开SQLite owner、向量库、模型工具或索引刷新流程。

`http.semantic_review` 默认关闭，须同时显式开启 `http.memory_review`、配置非空管理员Token、处于 standalone，并已启用原语义记忆配置。启动前和每个请求均核对门槛，gateway_channel_chat/gateway_driven拒绝；旧匿名HTTP准入或模型 memory_search 权限不能授权管理员检查。

`GET /api/memory/semantic-capabilities` 仅返回版本、范围与原预算；`GET /api/memory/semantic` 通过原Agent持有的同一Arc/Store，消费原非排队semantic admission、persistence executor、workspace writer与完整来源/manifest/chunk/vector核验。原四槽HTTP控制许可进入实际worker至结算，断开等待者不提前释放。没有POST/PUT/DELETE、embedding/chat/GET远端结果、refresh/rebuild/recover/reconcile、账本/receipt/generation变更，也不重新初始化另一个Agent。HTTP进程仍使用原Agent启动，原Operational启动可能将崩溃pending规范化unknown；这不是CLI inspection入口的零初始化替代。

来源最多3×128KiB完整UTF8、总384KiB和512chunks；不靠mtime/size、不截断、不回传正文/片段/向量/凭据。unsafe/超限/无效UTF8/坏索引/busy不能提供ready快照。正常JSON沿用CLI protocol_version1，含 current/stale/missing/space_mismatch、capture时ready、原pending身份及下一维护动作；HTTP通用错误不回显底层路径或错误。成功/拒绝均no-store/nosniff/no-referrer；错误方法也复用门槛和私密响应。

工作台在独立协商能力后才显示按钮，只在显式“检查语义索引”时读取，完整核对generation空间、计数、字节预算与freshness一致性后，用textContent展示已保存源的元数据，不自动轮询、提交embedding或刷新索引。开始新检查、记忆更正派发、错误/重连/身份切换清旧结果；迟到旧身份不能恢复。没有浏览器storage/Token落盘。gateway/readOnly身份零索引探查。

结果仅本次完整捕获，不保证未来查询或非协作外部编辑；hash非代次/ABA。运行Agent持有索引的原生命周期锁，本机refresh/reconcile等命令须先停止该服务释放索引，完成原显式维护后再启动并重新检查。CLI检查与这个GET都不自动解除hold、更新索引、删除会话/备份或提升租户范围；不是自动学习、经验复用质量、供应商、Agent浏览器、物理断电或StateKnot durable认证。


固定 Hyper 1.11.1 的验收以原生 `Expect: 100-continue` 响应核对三个竞争请求已经开始消费 Body；原 memory replace handler 在 Body poll 前先取得控制许可，全局 middleware 仅核 Content-Length/附加启动解析的预算，不消费 Body。三个 Continue 在同一个原2秒观察截止内读取，正常/400ms受控送达分别执行release0/SIGKILL-9；四槽已满的第五个同类请求必须直接503而无Continue，反证该回执不是TCP连接或header发送成功。仍保留原5秒body和8秒结算预算，未引入生产hook。[固定Hyper源码](https://github.com/hyperium/hyper/blob/v1.11.1/src/proto/h1/conn.rs#L405)。

共享回执读取在每次recv前及完整header返回前核验同一绝对截止。独立审查曾暂停仅本任务观察进程，在末次recv后超时仍返回的初版完整七组实际误过；最终修订同条件预期拒绝，正常/400ms四种结算与七组另行实际通过。暂停服务或延长期限不能代替这些回执。

原bcb1929远端Linux ARM候选在第4组观察503失败，官方日志没有三个竞争请求的body-poll回执，历史唯一根因未证明。受控loopback代理保留三请求，在原2秒内实际复现旧夹具误报owner释放；随后放行，实际503且原读暂停/SQL保持，证明旧观察缺少准入前提的一种机制。该受控实验不证明历史ARM一定同因，不称产品或框架修复，不借旧成功作业作新head资格。
