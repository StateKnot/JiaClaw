# 管理员语义索引检查

本批让已审阅记忆的操作员在同一工作台显式核对允许来源和当前索引，消费既有 `SemanticMemory.check`；不另开SQLite owner、向量库、模型工具或索引刷新流程。

`http.semantic_review` 默认关闭，须同时显式开启 `http.memory_review`、配置非空管理员Token、处于 standalone，并已启用原语义记忆配置。启动前和每个请求均核对门槛，gateway_channel_chat/gateway_driven拒绝；旧匿名HTTP准入或模型 memory_search 权限不能授权管理员检查。

`GET /api/memory/semantic-capabilities` 仅返回版本、范围与原预算；`GET /api/memory/semantic` 通过原Agent持有的同一Arc/Store，消费原非排队semantic admission、persistence executor、workspace writer与完整来源/manifest/chunk/vector核验。原四槽HTTP控制许可进入实际worker至结算，断开等待者不提前释放。没有POST/PUT/DELETE、embedding/chat/GET远端结果、refresh/rebuild/recover/reconcile、账本/receipt/generation变更，也不重新初始化另一个Agent。HTTP进程仍使用原Agent启动，原Operational启动可能将崩溃pending规范化unknown；这不是CLI inspection入口的零初始化替代。

来源最多3×128KiB完整UTF8、总384KiB和512chunks；不靠mtime/size、不截断、不回传正文/片段/向量/凭据。unsafe/超限/无效UTF8/坏索引/busy不能提供ready快照。正常JSON沿用CLI protocol_version1，含 current/stale/missing/space_mismatch、capture时ready、原pending身份及下一维护动作；HTTP通用错误不回显底层路径或错误。成功/拒绝均no-store/nosniff/no-referrer；错误方法也复用门槛和私密响应。

工作台在独立协商能力后才显示按钮，只在显式“检查语义索引”时读取，完整核对generation空间、计数、字节预算与freshness一致性后，用textContent展示已保存源的元数据，不自动轮询、提交embedding或刷新索引。开始新检查、记忆更正派发、错误/重连/身份切换清旧结果；迟到旧身份不能恢复。没有浏览器storage/Token落盘。gateway/readOnly身份零索引探查。

结果仅本次完整捕获，不保证未来查询或非协作外部编辑；hash非代次/ABA。运行Agent持有索引的原生命周期锁，本机refresh/reconcile等命令须先停止该服务释放索引，完成原显式维护后再启动并重新检查。CLI检查与这个GET都不自动解除hold、更新索引、删除会话/备份或提升租户范围；不是自动学习、经验复用质量、供应商、Agent浏览器、物理断电或StateKnot durable认证。
