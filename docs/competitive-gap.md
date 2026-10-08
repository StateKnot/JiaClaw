# 个人 Agent 能力差距

本项目以 [OpenClaw](https://github.com/openclaw/openclaw) 和 [Hermes Agent](https://github.com/NousResearch/hermes-agent) 为使用体验参考：清晰的安装/初始化入口，显式配置模型与凭证，再启动个人实例。不会因为竞品列出某项功能就把设计中的功能标记为 JiaClaw 已实现。

已补 copy、容器 exec、SQLite 会话、内置 Web 和 StateKnot HTTP MCP 的已审查只读工具，并提供可审核的版本安装/镜像/发布路径。持久 cron/interval 与 Telegram/Slack/Discord/飞书/企业微信/钉钉授权 inbox/outbox 已接线；Discord 本批增加单 guild 普通文字频道的 Bot 定时通知，验收见 [Discord](discord.md)。独立用户聊天/会话及受限定时任务、显式 Brokerrouter 语义检索、模型调用收据也已接线。完整多用户后台、真实模型检索质量/渠道/供应商认证仍未完成；MCP stdio/外部写入、更多渠道、durable 子 Agent、多模态与真正逐 token streaming 仍待交付。

单实例管理员的 [Web 发件箱审计](web-outbox.md)已通过 PR #74 的 Chromium 与跨平台验收；确认送达和整来源取消保留持久状态边界，普通网关用户不能获得渠道管理权限。PR #76–78 依次修复五个主文件工具/四个兼容名称、grep/glob 有界搜索及 mkdir/move 的同卷原子变更，均已通过各自最终 head 的跨平台 CI；这些历史批次不包含本次 copy 治理修复。

当前十二个主文件工具与五个兼容名称共用目录句柄及八槽阻塞 I/O，copy/file_copy 也纳入与记忆及其他 mutation 相同的工作区 inode 协作写锁，并拒绝源/目标硬链接。copy 保留 64 MiB 二进制流式上限、增长时上限 + 1 字节检测，以及 hard link 无覆盖 / rename 覆盖的原子发布合同。取消后任务可能完成，提交后同步或暂存链接清理错误需要停写核对，不自动重放；这不是 durable 文件操作恢复或非协作编辑的一致快照。此批本地、跨平台及真实文件系统验收结果由[验证记录](validation.md)分别记录，不用历史 CI 替代。

本轮新增可选的 [stat/tree](workspace-files.md#stat--tree-元数据与目录树合同)：叶子自身元数据查询、可调深度的有界 DFS 目录树、严格参数与独立配置开关。本机 884 项 Rust、fmt/Clippy/锁定构建、新工具六组与七套既有进程回归通过，PR #79 最终 head 已通过 Linux/macOS 与真实容器 CI；持久渠道/cron 白名单不扩大，管理员启用的独立 HEARTBEAT 与兼容 `/hooks/inbound` 仍遵循现有注册工具策略。

本轮进一步接入默认关闭的[独立用户 Telegram 私聊](tenant-telegram.md)：一个 Bot/人绑定一个独立后端，持久准入与业务关联、受限工具和离线未知核对。本机 918 项 Rust、七组新整机与七套既有回归通过，跨平台 CI 以本批最终 head 为准；共享网关队列盘、其他渠道、定时外发及真实安装仍有各自范围，不能据此将完整多用户后台能力标记完成。

能力、框架支持状态与可落地验收条件统一维护在 [roadmap](roadmap.md)，不再在多份文档中复制相互矛盾的完成勾选。框架生产资格与实际应用集成是两个独立门槛，见 [StateKnot](stateknot-gaps.md)、[Brokerrouter](brokerrouter-gaps.md)。
