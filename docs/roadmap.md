# JiaClaw 里程碑与验收

2026-10-02 按当前代码与上游仓库重新核对。此表以交付能力为准，不以文档里的设计或存根作为“完成”。

| 顺序 | 能力 | 状态 | 完成标准 / 当前证据 |
|---|---|---|---|
| 1 | copy | 实现并本地验收 | 二进制文件、64 MiB 上限、越界/链接拒绝、原子覆盖与并发不覆盖 |
| 2 | 受控 exec | 实现并真实 Docker 验收 | 默认禁用、白名单、固定镜像、非 root/无网络、超时/输出限制、清理；SIGKILL 边界见配置说明 |
| 3 | SQLite 会话 | 实现并进程级验收 | 创建/对话/删除持久化、一次性 JSON 迁移、独占锁、并发串行、SIGKILL 后恢复 |
| 4 | MCP 客户端 | HTTP 只读工具已接线并协议/整机 fixture 验收 | 精确 StateKnot 版本、工具白名单/描述 pin、离线 schema、鉴权/有界调用/取消；[使用边界](mcp.md)。外部服务器独立认证，写入需 durable；stdio 上游 [#140](https://github.com/StateKnot/StateKnot/issues/140) |
| 5 | Web 工作台 | 实现 | 内置同源静态资源，鉴权后创建/选择/聊天/删除会话，无模型 HTML 执行、无浏览器持久密钥 |
| 5a | Brokerrouter 原生工具往返 | 实现；按本批 fixture 验收 | 原生 tools/tool_calls/role:tool、调用 ID 关联、整批权限/参数预检、正文不执行、有限调用预算；[合同与验收方法](native-tools.md)。真实供应商 #31 与 durable #41 仍开放 |
| 6 | cron 多任务 | 实现；本批执行持久化与进程验收 | SQLite v2 jobs/runs、鉴权增删查与暂停/恢复、明确时区/DST、原子领取/完成、配额与中断暂停；[运行边界](scheduler.md)。没有副作用自动重放或渠道通知 |
| 7 | 渠道统一出站 | 待实现 | Telegram/Slack/Discord 统一消息与错误合同，持久 outbox/幂等、重试和速率限制，再接飞书/企微/钉钉/WhatsApp |
| 8 | StateKnot durable + 委派 | 待认证/接线 | 原生输出合同缺口 [Brokerrouter #41](https://github.com/StateKnot/Brokerrouter/issues/41)；admission/driver/store、子任务身份、预算/并发/取消、恢复语义及上游生产门槛 |
| 9 | 模型路由与降级 | 上游有支持；应用待策略接线 | 由 Brokerrouter 按能力/模型策略路由，仅明确未提交可安全重试；预算/审批不绕过 |
| 10 | 多用户与 Key 管理 | 待实现 | 鉴权主体、授权检查、workspace/记忆/会话隔离、Key 哈希/轮换/撤销、渠道身份绑定与越权测试 |
| 11 | 真正流式 | 上游已有协议支持；应用待实现，资源修复未验收 | 逐事件输出、tool delta 聚合、断流恢复/结算、取消与背压；上游 [PR #40](https://github.com/StateKnot/Brokerrouter/pull/40) 慢客户端/连接容量修复尚未合并，不能把完成后分块作为 token streaming |
| 12 | 语义记忆 | 上游已有 embeddings；应用待实现 | 模型/维度版本、SQLite 元数据与索引一致性、去重、隔离、删除/重建与检索质量验收 |
| 13 | 多模态 | 待实现/上游认证 | 文本契约之外新增受限 media 输入/输出，大小/格式/权限校验，使用网关媒体任务契约与认证供应商 |
| 14 | 打包发布 | 文件与工作流已交付，首次公开 Release 待审核 | 四平台二进制/校验和、安装回滚、非 root 镜像验收、版本标签匹配、draft 审核；没有自动发布到公网 |
| 15 | 文档与 E2E | 本批覆盖；持续扩充 | 实际配置/备份部署、模型 fixture、SQLite 崩溃、容器和安装；真实渠道及供应商仍需独立联调 |

MCP 之后的功能依赖 durable 身份、授权或 outbox 的应先补底层契约，避免在当前内存执行循环上承诺恢复能力。MCP stdio 与 Brokerrouter 真实工具默认分别跟踪上游 #140 / #31；不复制框架内部实现绕过未通过的生产门槛。

本批修改改善单机应用的可运行性与安全边界；完整个人 Agent 生产认证仍未完成。最新固定版本、检查证据和障碍见 [StateKnot](stateknot-gaps.md) 和 [Brokerrouter](brokerrouter-gaps.md)。
