# 工作台中的管理员记忆纠错

这项功能独立默认关闭。管理员需同时配置私密审阅与纠错，并使用非空管理 Token、standalone 后端：

```toml
[http]
memory_review = true
memory_edit = true
api_token = "REPLACE_WITH_PRIVATE_ADMIN_TOKEN"
```

只开启 memory_review 仍只读；legacy 匿名访问、gateway_channel_chat、gateway_driven、租户和只读 Key 不获得写权限。启动检查与每次请求均检查范围和鉴权。浏览器仅在 standalone 的审阅合同与独立纠错合同成立后展示表单；需要 WebCrypto 的安全上下文，例如 localhost 或受信任 HTTPS。远程监听沿原部署合同使用受信任 TLS。Token、原文与草稿不保存到浏览器 storage。

先显式读取配置记忆完整快照，然后编辑候选全文并勾选已审阅，再提交一次。服务核对配置路径、原始文本 SHA256 和候选 SHA256；每项均为完整原始 UTF8 字节，最多32KiB，无截断或默认文件回退。文本框遵循浏览器换行规范；候选摘要针对实际提交的文本，包括规范化后的换行。空候选明确清空当前文件；同值、陈旧条件、错误摘要、缺失父/目标、不安全身份和写入锁忙拒绝。外部管理员改动不在协作 flock 保证内；摘要是内容条件，不是代次或 ABA 历史检测。

`GET /api/memory/edit-capabilities` 返回 protocol1、standalone、replace、max_bytes32768、conditions path-and-two-raw-sha256。原审阅 protocol1/read_only 合同保持，独立入口不能由它推导授权。

`POST /api/memory/replace` 接受严格 JSON（未知/重复字段拒绝）：

```json
{"path":"MEMORY.md","content":"Reviewed complete replacement\n","if_content_sha256":"64 lowercase hex digits","if_replacement_sha256":"64 lowercase hex digits"}
```

路径必须等于当前配置及原快照。原四槽非排队控制许可贯穿有界正文捕获到实际 blocking writer 结算；正文捕获5秒，上限取256KiB与配置 HTTP 全局上限的较小值，Content-Length和chunked均受实际读取上限约束。这个预算不是阻塞 OS 读取或 fsync 的可抢占硬截止。MemoryEditor 的文本和已有 CLI 文件输入共用同一条件核验/原非排队 workspace writer/私有 sync+rename 发布，不另造存储、模型工具或新账本。目标和父目录必须现存。

成功返回磁盘 path/previous_sha256/content_sha256/bytes_written 收据，不回显候选正文；页面核对收据后清空旧原文和草稿，要求显式再读取核对当前文件。请求失败、断开、超时或 rename 后 fsync 失败可能已经提交；409 与 `X-Jiaclaw-Write-Review: required` 要求先 inspect/重新读取，不能自动重试或把未知结果当零效果。页面在首次实际提交后撤销可提交快照，不自动 GET 或重复 POST；未确认草稿只留在当前身份页面内存，重新读取/重新连接会清除。身份变化或401清除所有私人 DOM，旧响应不能恢复内容。它没有操作 UUID/持久回执或重启后的效果确定性保证。

发布只替换配置记忆文件。原 Agent 的后续轮重新读取，当前已构建提示不修改；原 memory_write 模型开关不授权此管理员入口。语义索引不自动刷新/清除，仍独立显式维护；清空不删除历史、模型收据、索引或备份。没有自动学习、出处真实性或真实任务经验质量认证。浏览器验收证明管理员页面行为，不认证 Agent 浏览器工具、真实供应商、物理断电或 StateKnot durable。已有双摘要 CLI 继续可用。

验收要求：冻结父实际端点不存在；新七组真实 HTTP/OS 含双摘要、明确清空/完整32KiB、能力隔离、竞写、不安全文件、实际 blocking owner 的断开等待者和 post-rename EIO。真实 Chromium 覆盖显式提交/勾选、stale 拒绝/手动读取、未知响应不重发、身份清理、默认关闭和网关零探查。新增 Rust 公共文本 Interface 与 HTTP 准入回归，原记忆纠错/审阅/工作台/只读合同保留。新固定官方 head 必须独立七作业、两平台60必需Python/Linux九Chromium、四优化候选各13安装归档源码及继承合同，不能借父127或本机结果。Standards/Spec 独立，累计4b2357fc历史继承与本批 fresh 分开。
