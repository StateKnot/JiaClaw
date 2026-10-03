# JiaClaw 里程碑与验收

2026-10-03 按当前代码与上游仓库重新核对。此表以交付能力为准，不以文档里的设计或存根作为“完成”。

| 顺序 | 能力 | 状态 | 完成标准 / 当前证据 |
|---|---|---|---|
| 1 | copy | 实现并本地验收 | 二进制文件、64 MiB 上限、越界/链接拒绝、原子覆盖与并发不覆盖 |
| 2 | 受控 exec | 实现并真实 Docker 验收 | 默认禁用、白名单、固定镜像、非 root/无网络、超时/输出限制、清理；SIGKILL 边界见配置说明 |
| 3 | SQLite 会话 | 实现并进程级验收 | 创建/对话/删除持久化、一次性 JSON 迁移、独占锁、并发串行、SIGKILL 后恢复 |
| 4 | MCP 客户端 | HTTP 只读工具已接线并协议/整机 fixture 验收 | 精确 StateKnot 版本、工具白名单/描述 pin、离线 schema、鉴权/有界调用/取消；[使用边界](mcp.md)。外部服务器独立认证，写入需 durable；stdio 上游 [#140](https://github.com/StateKnot/StateKnot/issues/140) |
| 5 | Web 工作台 | 实现 | 内置同源静态资源，鉴权后创建/选择/聊天/删除会话，无模型 HTML 执行、无浏览器持久密钥 |
| 5a | Brokerrouter 原生工具往返 | 实现；按本批 fixture 验收 | 原生 tools/tool_calls/role:tool、调用 ID 关联、整批权限/参数预检、正文不执行、有限调用预算；[合同与验收方法](native-tools.md)。真实供应商 #31 与 durable #41 仍开放 |
| 6 | cron 多任务 | 实现；含 Telegram/Slack/飞书/企业微信/钉钉定时通知 | SQLite jobs/runs、鉴权增删查与暂停/恢复、明确时区/DST、原子领取/完成、配额与中断暂停；[运行边界](scheduler.md)。[定时通知](scheduled-delivery.md) 与运行/会话原子提交、目的地单独授权，无副作用自动重放 |
| 7 | 渠道统一出站 | Telegram/Slack/Discord/飞书/企业微信/钉钉已实现，真实渠道认证与新渠道待完成 | 持久 inbox 去重、授权白名单、统一有界发送、共享 outbox/回执、429 冷却、未知结果人工核对；[合同](channels.md)。定时 Telegram/Slack/飞书/企业微信/钉钉已接入；[飞书](feishu.md)限企业自建单租户文本，[企业微信](wecom.md)限专用自建应用与精确成员文本；[钉钉](dingtalk.md)限内部应用机器人、获准成员私聊；Discord 主动发送及 WhatsApp 仍待交付；进程 fixture 不能替代真实安装认证 |
| 8 | StateKnot durable + 委派 | 待认证/接线 | 原生输出合同缺口 [Brokerrouter #41](https://github.com/StateKnot/Brokerrouter/issues/41)；admission/driver/store、子任务身份、预算/并发/取消、恢复语义及上游生产门槛 |
| 9 | 模型路由与降级 | 按任务来源选逻辑模型已接线；网关端点降级待联合认证 | 管理员配置聊天/渠道/定时/心跳/摘要模型与有界输出策略，每轮工具循环固定选择；[路由合同](model-routing.md)。端点降级归 Brokerrouter，应用不在未知结果后换模型或重发；真实供应商 #31 仍开放 |
| 10 | 多用户与 Key 管理 | 独立用户聊天/会话入口已实现；完整里程碑未完成 | 一用户一容器/工作区/数据库/私有网络/限额卷，独立网关哈希 Key 轮换撤销、受限代理、未知写入持久核对；[部署和验收范围](gateway.md)。多用户后台任务/渠道身份绑定与真实供应商联合认证仍待完成 |
| 11 | 真正流式 | 上游已有协议支持；应用待实现，资源修复未验收 | 逐事件输出、tool delta 聚合、断流恢复/结算、取消与背压；上游 [PR #40](https://github.com/StateKnot/Brokerrouter/pull/40) 慢客户端/连接容量修复尚未合并，不能把完成后分块作为 token streaming |
| 12 | 语义记忆 | 显式 Brokerrouter/SQLite 已接线并本地验收；真实模型质量待认证 | [语义记忆](semantic-memory.md)：来源/空间版本、私有索引、精确余弦、源哈希新鲜度、持久未知 hold、GET 核对与显式 CLI 刷新/重建；fixture 与真实模型质量验收分别记录 |
| 13 | 多模态 | 待实现/上游认证 | 文本契约之外新增受限 media 输入/输出，大小/格式/权限校验，使用网关媒体任务契约与认证供应商 |
| 14 | 打包发布 | 文件与工作流已交付，首次公开 Release 待审核 | 四平台二进制/校验和、安装回滚、非 root 镜像验收、版本标签匹配、draft 审核；没有自动发布到公网 |
| 15 | 文档与 E2E | 本批覆盖；持续扩充 | 实际配置/备份部署、模型 fixture、SQLite 崩溃、容器和安装；真实渠道及供应商仍需独立联调 |

MCP 之后的功能依赖 durable 身份、授权或 outbox 的应先补底层契约，避免在当前内存执行循环上承诺恢复能力。MCP stdio 与 Brokerrouter 真实工具默认分别跟踪上游 #140 / #31；不复制框架内部实现绕过未通过的生产门槛。

本批修改改善单机应用的可运行性与安全边界；完整个人 Agent 生产认证仍未完成。最新固定版本、检查证据和障碍见 [StateKnot](stateknot-gaps.md) 和 [Brokerrouter](brokerrouter-gaps.md)。

下一批先复核 StateKnot #140、Brokerrouter #31/#41 与 PR #40 的固定版本变化；durable 合同未满足期间，继续独立可交付的渠道与应用接线。钉钉本批只覆盖 HTTP 模式内部机器人私聊及定时文本通知；回调重投与字段稳定性、真实安装、平台限额和终端收发需独立认证。WhatsApp 接入须先核实当前 Cloud API 通用 AI 服务资格、部署主体/地区、身份、客户服务窗口、模板授权与未知投递语义，不能将独立 3P Agents 条款外推为 Cloud API 许可。模型路由已完成应用策略接线，后续保持网关治理边界，独立用户聊天入口已接线，继续推进多用户后台权限、真实模型语义检索认证或满足合同后的 durable 接入。网关保留未知写入的人工核对状态，不表示能够恢复/重放工具运行。

本批先收紧既有 MEMORY / SOUL / USER / HEARTBEAT 文件边界：配置路径接线、有界读取、统一写入上限、目录句柄约束、原子发布、协作追加锁和初始化保留；新增整机 fixture 与 747 项 Rust 回归已在本机通过，跨平台/真实容器以本批 draft PR 最终 head CI 为准。这是语义记忆前置修复，不能据此将 embeddings 或向量检索标记完成。

语义记忆本批仅在显式启用时接入 Brokerrouter embeddings 与私有 SQLite。源变更先拒绝查询计费，未知提交保留 hold；维护 CLI 要求同库服务停机，不新增公开管理路由。最终二进制的 9 组离线整机验收、773 项 Rust 回归、fmt/Clippy/锁定构建以及 e2e/native_tools/memory_io/model_routing 已在本机通过。Linux/macOS 和真实容器仍待最终 head CI；合成向量不代表真实模型检索质量认证。
