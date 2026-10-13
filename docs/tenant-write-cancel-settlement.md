# 取消文件任务后的原 hold 结算前置

PR140 初固定 head `a383e22cda0e9fe9f0124dccb9ac99078fbf6673` 的 Linux x64/ARM 优化候选在 `Verify optimized binary` 内、新 `tenant_file_write.py` 第六取消组的 `App.review → review-clear` 失败：原注册表拒绝清除仍为 in_flight 的 write hold。两份正式job/check-run/annotations与完整认证日志保留；Rust/前五组与writer-busy严格hold收据先通过，取消组后续及安装归档未完成。Mac ARM同head独立完整通过不补两失败资格，不证明失败唯一环境原因。

网关原后台观察 owner 负责原任务结算，客户端原GET看到 native needs_review 并不同时认证网关 hold 已转换。原 `review-clear` 正确拒绝 in_flight；本批不修改它、不清未知hold或重派发原请求，也不报 StateKnot/Brokerrouter 缺陷。

外部最小反馈环使用相同冻结 `2075429b036bd6475e9da9eb7ab420f905c5547bc8b577847f9dac5f386a3cee`、原native/gateway/registry/model与一个实际取消任务。loopback relay仅延迟第一份原后台观察GET响应1500ms，其原running正文完整捕获；客户端后续GET正常。原顺序真实RED：取消POST200/intent保存、原needs_review、一次model/零文件效果，实际SQL hold in_flight，review-clear原错误。只在同一原15秒绝对截止内核对exact user/request_id needs_review后再review，1676ms真实GREEN，一model/零效果、原期限保持。这证明受控观察前置缺口机制，不认证Hosted延迟、具体调度/网络或历史唯一同因；日志认证采用connector完整decoded UTF8，未独立认证原Azure物理raw object，后续终结workflow archive须另核。

源码修订只在新文件夹具的取消POST前建立原15秒绝对截止，共用writer-busy的纯观察helper；SQL返回和最终返回均复核截止，只等剩余预算内的原精确hold，之后原App.review各原鉴权/idle/receipt断言保持。取消原POST和model release不重复；文件内容、cancel_requested、原needs_review、一model、readonly/撤Key/跨用户与所有原安全断言不减。新五key `ACTUAL tenant write cancel hold observation` 收据完整记录UUID/state/精确int elapsed_ms与两个true期限字段；须落在PASS5与PASS6之间，与writer-busy/users/browser UUID独立。

109compiler inputs与生产2075字节不变；此前1318Rust/相关进程/browser/source审查只继承，修订夹具与controlled loop/原七HTTP_OS和新四工作台Chrome必须各自实际执行。没有Rust/build/优化package/Docker本机重跑、paid调用或新target。新外部审计器保留原171 Assert AST，只增强现有helper核验取消收据；countercases为synthetic，不当产品或官方171执行。旧head完整失败证据和partial不覆盖；正常向前更新同draft PR，新的固定head须自己的七actual/两平台67Python/Linux23Chrome/四候选13/API摘要/full171及fresh官方两轴，不能借旧两个成功、父139或本机，不提交纯自引资格docs制造CI。
