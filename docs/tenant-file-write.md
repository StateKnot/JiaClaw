# 个人 Key 的明确文件交付

默认关闭。实例管理员分别在原 private backend 的 `[http.tenant_file_write]` 和 gateway 对应 backend 的 `tenant_file_write` 配置同一 `paths`。清单为 1..32 个按 UTF-8 字节排序、唯一、完整的 `tenant-documents/` 相对文件名，每项最多256 UTF-8字节/16组件，沿用原文件读取 policy 的 hidden/control/backslash/dot-parent/重复slash/首尾空白/BOM别名拒绝规则。权限是完整覆盖指定文件，包括新建；不是目录、递归或追加授权。已有文件正文会被替换。

```toml
[http.tenant_file_write]
paths = ["tenant-documents/report.md"]

[tools.write_file]
enabled = true
```

原 tracked tenant HTTP、Brokerrouter、SQLite会话/model_calls与原鉴权必须启用；单独开放 `write_file` 工具配置不会授予租户权限。双方启动和每次新准入核对完整 `file_write` descriptor，protocol1 与 SSE protocol2 独立。descriptor 为 `{protocol:1,paths,mode:"overwrite",max_bytes:262144,policy_sha256}`，SHA 输入是完整规范 UTF-8 JSON `[1,paths,"overwrite",262144]`。它是静态权限摘要，不是原文件内容、代次、ABA、来源真实性或文件准备状态。

工作台安全页面用 WebCrypto 核对完整descriptor/hash，以 textContent 显示所有目标；默认不勾选。用户本次明确勾选 `file_write` 才捕获 `file_write_policy_sha256` 并向原 native loop 授予同名已有工具。它只收精确 `path` 和完整 `content`；mode、其它参数或未获准路径拒绝。与原 file_write 共享同一个原八非排队 blocking I/O owner、工作区 writer、路径能力操作、私有暂存/sync/rename与256KiB上限。原工具批次16KiB arguments、原请求/模型/收据/历史期限仍适用，因此模型单次可提交正文实际也受原JSON参数预算限制；不截断超限内容。

读取授权独立，写入不隐含 file_read、skills、memory、exec/MCP、目录、移动或删除权限。standalone 原 file_write/write_file 的覆盖/追加行为保留，私有管理员/OS权限也不由租户清单认证。channel/cron 保持原两工具范围。选中写入可能产生原模型费用，生成正文可能含私人资料或错误；模型答复/工具返回不是用户业务验收，实际文件应由用户核对。工作台回复下载仍只保存回复，不把工具正文伪装成附件。

全 Key/原用户/原 UUID/原 hold/admission 路径不变；readonly、撤 Key、跨用户、错误/缺失/陈旧/额外 write hash拒绝。未选写入时新字段省略，原 None 指纹保持；已存在 UUID 只读原事实，不因撤权重派发。撤权须停止并重启两端以更新固定epoch权限，不能在旧epoch原地改清单并声称已撤权。

写入 errors、timeout、断开和取消继续是保守 Unknown。显式停止后不再派发未来工具，已开始的原 blocking writer 持有真实 I/O permit 与 writer 至结算，可能已写入或稍后完成。未知结果须核对原文件/原任务/model ledger；不自动重试、重放、回滚或解除 hold。此接线不是 StateKnot durable/审批后继续、物理断电/任意外部 exactly-once、非协作OS编辑原子快照或独立管理员exec限制的认证。
