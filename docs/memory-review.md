# 私密记忆审阅工作台

Standalone 管理员可显式启用完整记忆审阅：

```toml
[http]
memory_review = true
api_token = "REPLACE_WITH_PRIVATE_ADMIN_TOKEN"
```

启动前要求非空管理 Token，并拒绝 gateway_channel_chat 或 gateway_driven 后端范围。默认关闭，即使旧本机服务允许匿名会话，也不能读取记忆。网关、个人 Key、只读 Key 不因此获得新的后端能力；工作台观察到网关合同后不会探查此入口。远程监听应通过受信任 TLS 入口，Token 和文本按原管理权限保管。

连接 standalone 工作台后选择“记忆审阅”，按显式动作读取配置的 `memory.path`。显示完整原始 UTF8 文本、配置路径、原字节 SHA256 与字节数；32KiB 是完整源预算，无截断或默认 MEMORY.md 回退。空源明确显示为空；缺失、父/叶链接、硬链接、特殊文件、不合法UTF8、超限拒绝。本次读取失败先清除旧文本，不把旧快照显示成当前事实。

`GET /api/memory/capabilities` 提供 protocol1、standalone、read_only 和 max_bytes32768；`GET /api/memory` 直接消费已合格 MemoryEditor.inspect。两个入口都要求显式配置与管理鉴权；响应 no-store/nosniff，页面只用 textContent，不保存内容/Token 到浏览器存储。重新连接或鉴权失败清除私人 DOM，晚到旧身份响应不能恢复它。

读工作消费原四槽控制 pool 的非排队许可，在 blocking owner 中持有到实际读取结算；取消 HTTP 等待不提前释放许可。文件预算不保证阻塞 OS 读可抢占的硬期限。此入口只读，无 POST/PUT/DELETE、自动轮询或模型请求，不新增记忆存储/工具/写入权限。

更正或清空仍使用[已实现的本机记忆维护](memory-correction.md)，完整保留原审阅文本，再通过原/候选双摘要条件提交。本页不编辑、清空或执行模型；读取时快照不证明索引、历史、备份、当前已构建提示、自动学习或真实任务复用质量。浏览器验收认证管理页面行为，不认证 Agent 浏览器工具、供应商或完整个人 Agent 里程碑。

以上为单独启用审阅时的原只读合同。管理员可另外显式开启默认关闭的 `http.memory_edit`，通过[独立纠错合同](memory-edit.md)审阅候选全文并条件提交；读权限本身不提供写入授权。原两个 GET 的 protocol1/read_only 合同保持。
