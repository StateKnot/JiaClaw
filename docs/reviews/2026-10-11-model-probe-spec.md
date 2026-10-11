# Spec 独立源码审查：显式模型验证

固定累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c` → `4616ab34d8562cba023d5a6f7e7dab7b40b43905`；继承此前累计报告，本次 fresh `4862de9..4616ab3` 新查 ModelProbe Module/CLI、共享 tracing 修订、真实夹具及 CI 接线。未读取 Standards，不声称穷尽累计重审；读取实际 pinned git log（含4616），旧提交清单文件暂未含该最后提交。

**新增开放 Spec 问题：0。** 规格第9行“验证在打开账本前完成”、第11行“两条固定诊断消息”的实现顺序成立：显式 config/计费 flag，Brokerrouter+账本启用门槛，路由/Key/endpoint/完整字节 prepare 后才打开独占 Store；Chat/温度0/min预算128，无 ambient 初始化。仅借现有账本的 prepare/complete 原身份/hash/预留、canonical UUID/精确模型/assistant-stop/无工具及2MiB预算；不另造重试或fallback。

第15行“响应在输出前已经持久保存”由 complete 完成后 status 精确 turn_id/completed/has_receipt 核验保障。错验证码与已完成收据分离。初真实 SIGTERM stdout 含86字节INFO日志的失败属于此输出合同：4616共用 stderr subscriber 与流式CLI原有行为一致；源码修正成立，最终执行资格尚待日志。元数据不包含原消息/Key，错误路径沿用脱敏收据错误。

第17—19行所有权/取消/一次尝试：verify 先 take 唯一 prepared；真正模型与保存 owner 在原独立任务中继续，CLI实际处理取消后只 settle 五秒，完成也中断退出；未知 hold不清除、不重发，重启仍用原GET-only恢复。Signal夹具现明确观察处理日志再放行响应/核验竞争者被拒，区别旧“只发信号”断言。

第21行真实二进制/七组回归已接入两平台CI及四优化候选；当前最终执行/全量/冻结身份和新官方资格尚待独立补核。初缺字段/base路径/预算夹具错误、初stdout真实失败分别保留，不借父或初GREEN；假网关不认证真实供应商、费用、TLS、安装或首用工具任务。没有新的应用或框架缺陷报告。

# Spec 独立增量补审：ambient 夹具与单位测试准备

固定累计4b2357fc → 0e8c03656d3a479eed9f6b1428d49f6eb476468e，继承已保存4616 source-spec.md；本次仅审4616→69f0868a8f3a87cabc23089e0dd748d96e79efce→0e8c036增量。未读取Standards，不声称累计穷尽重审。**新增开放Spec问题：0。**

规格model-probe.md第11行“不初始化它们的数据库”与“即使配置启用了这些能力”新增真实夹具覆盖成立：69启用semantic SQLite配置并核验文件不存在；配置有凭据的MCP服务器指向同一实际假网关，任何发现POST会被非Chat路径断言记入errors，并在每次snapshot中失败。原有FIFO技能正文、私有工作区内容与session数据库检查保留。此增量不调用真实供应商，也不改变生产授权/重试/预算。

现已读取model_probe-final.log和model_probe-ambient-final.log，两份各包含原七组实际GREEN：明确门槛、精确固定消息、错验证码完成收据、未知hold、准入回滚、实际处理SIGTERM后原owner结算、SIGKILL原GET-only恢复。此前stdout真实RED由源码修订解决后获得实际GREEN；早期协议/配置夹具失败仍独立保留。上述只是当前冻结二进制进程证据，不替代全量Rust或交付head官方资格。

0e8只给cfg(test)失败尝试测试创建所需工作区，随后才开账本；原初rust-final.log在open处失败没有证明“一次尝试不可重用”，不计为全量通过。该修正不触及生产prefix，也不改变零状态静态拒绝两项测试。doctor.md新入口明确计费、独占库服务停止、持久收据和仅诊断响应范围，符合规格第9/15/21行，未将doctor默认行为扩成模型调用。

新的全量rust-settled/最终构建、99输入及最终交付身份仍等待完整补核；不借父PR或初版失败/部分成功作为资格。
