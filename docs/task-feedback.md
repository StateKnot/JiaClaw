# 用户评价原任务结果

用户在核对已完成的原 tracked HTTP 任务后，可明确标记“可用”或“需要修改”，附最多1024 UTF-8字节说明，也可明确清除当前评价。该记录用于保留用户的实际判断，不把模型完成、浏览器下载或 fixture 通过当成业务成功率；不自动写入记忆、训练模型、继续任务或重发工具。

评价位于原会话 SQLite 的原永久任务身份行；schema12以同一事务从原schema11升级，原收据、历史、权限摘要和未知hold不变。不建立第二存储。原停止服务后的维护入口可只读schema11或12，不迁移。升级前应在停止服务后保留原数据库备份；旧版本拒绝schema12，停止使用反馈也不会将数据库降级。每个身份最多一份当前评价和递增revision，说明是私人资料；清除会移除当前说明，原任务与历史保留，备份不自动删除。原正文purge同时清除当前评价，仍保留revision以拒绝陈旧提交。

`PUT /api/turns/{原UUID}/feedback`只接受`expected_revision`精确整数、`decision=usable|needs_changes|clear`、完整`note`。clear要求空note，其他说明可空；拒绝未知或重复字段、无效编码/控制字符、超限和陈旧revision。只允许原`completed`、非active、正文仍保留且status=completed的任务；unknown/needs_review/running/purged均拒绝，不解除任何会话或网关hold。反馈不会替换原reply、工具记录、请求hash或完成时间。相同内容不再修改；响应丢失后仅按原UUID GET核对，不自动重试写请求。

standalone沿原Token/四个非排队控制owner和五秒正文期限；gateway需当前完整个人Key、自己的原注册身份和精确后端收据，readonly/撤Key/跨用户不能写。网关不新建准入、执行占用或POST模型，不结算任何旧hold；取消等待者仍由实际存储worker持有owner直到结束。每端保留原限额和期限。

网关只有在原completed正文仍包含完整reply、completed status及原五种允许工具名称列表时，才接受反馈成功回执；字段缺失、错误类型、超限或非允许工具均按未知响应处理。实际后端可能已经保存评价，此时只按原UUID GET核对，不自动再次PUT。

工作台仅在当前身份下已核对的原completed收据显示反馈；新任务/断开/身份切换/结束跟踪清私人评价DOM和编辑草稿。保存前按原UUID再核对，revision发生变化则要求重新审阅；用户明确按钮只提交一次。任何反馈写入的未知响应保留原UUID，并将评价视图设为未核对；核对成功前不可继续写。服务重启后评价可由原GET找回，不新增poll、浏览器storage、模型工具或隐式模型请求。

验收要求：真实native/gateway/模型/SQLite/Chromium任务→人工评价→GET→实际重启→原事实恢复；同UUID并发CAS最多一份、失联后GET不重复写、readonly/撤Key/跨用户/迟到identity/未知工具效果/原purge拒绝、完整1024边界及文本XSS、原模型POST/工具效果数量和全部hold不变。真实fixture证明接线与边界，不认证实际模型业务质量、学习、供应商、publicTLS、发行安装或StateKnot durable。

源验收、初始失败与资格边界见[task-feedback-validation.md](task-feedback-validation.md)；新fixedhead官方资格另独立核验。
