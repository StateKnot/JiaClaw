# 下载当前明确授权的文件

这项能力让个人 Key 用户取得工作区中的真实文件字节，例如 Agent 生成的 Markdown 或 CSV。它与保存模型回复、执行 file_read 和授予 file_write 分别独立；下载的是点击时捕获的当前文件，不认证它仍等于某次历史任务的产物。

两端 `http.tenant_file_download` / gateway backend `tenant_file_download` 默认关闭，启用须已有完整相同的 `tenant_file_read` 精确清单、tracked tenant HTTP 与 read_file。只消费原 FileReadTool / WorkspaceReadFileTool / 八槽 blocking I/O / 有界目录能力读取，不增加模型工具、第二读取引擎、存储或目录发现。完整 UTF-8 文本最多262144字节，NUL、非UTF-8、链接、unsafe parent、hardlink、FIFO、目录、缺失和超限均整次拒绝；BOM、CRLF、空白与零字节原样保留。普通 Agent 按行读取的既有语义保持。

独立协议1绑定当前完整 read policy。`GET /api/turns/files/{0..31 index}/{policy_sha256}` 不接收正文、query 或路径别名。原认证先于读取；gateway 每次核两端完整合同，后端身份只来自当前用户绑定，读取后再认证当前 Key。readonly Key 沿原 GET 权限，撤 Key / 跨用户 / 不同权限拒绝。原 registry 身份、hold、模型账本和会话均不改变；下载不解除 hold、不创建任务、不触发模型。

原 native 四槽控制许可随真实 blocking I/O owner 至结算，浏览器/HTTP超时或断开不证明线程停止；gateway 原 global/backend 控制限额保持。一次完整有界捕获后再响应，响应是 octet-stream、no-store/nosniff，严格单值协议/index/policy/bytes/SHA头。浏览器核完整原字节和SHA后，显式请求固定摘要名下载；不冒称保存成功。原文件可能被非协作编辑；应用权限不是独立OS ACL或原子多文件快照。下载文件可能含私人资料，不会随断开或服务清理删除。

工作台显示完整当前授权路径，默认未确认；明确选择与勾选绑定当前identity/policy/index。无自动下载、storage、轮询、模型或新 Agent 工具。迟到读取或SHA计算不能恢复旧用户DOM或触发下载；不把文件正文插进DOM或执行。模型回复下载仍保留原合同。

## 显式启用

原 native 配置中设置 `http.tenant_file_download=true`；对应 gateway backend 设置 `tenant_file_download=true`。两端都须已有一致的 `tenant_file_read.paths`（例如 `["tenant-documents/report.md"]`）、原 tracked tenant / read_file 配置。服务启动核完整两端协议；改授权须按原权限 epoch 停止并重启两端。禁用下载保持原 file_read 与工具选择行为。

新文件捕获 native 与 gateway 均用5秒完整异步预算，工作台从发起GET、完整接收、SHA核对到请求保存共15秒；超时整次不下载且不自动重试。底层实际读取可能继续持有原许可至结算，文件读取不会变成后台重派发。
