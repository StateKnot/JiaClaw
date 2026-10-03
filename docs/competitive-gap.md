# 个人 Agent 能力差距

本项目以 [OpenClaw](https://github.com/openclaw/openclaw) 和 [Hermes Agent](https://github.com/NousResearch/hermes-agent) 为使用体验参考：清晰的安装/初始化入口，显式配置模型与凭证，再启动个人实例。不会因为竞品列出某项功能就把设计中的功能标记为 JiaClaw 已实现。

已补 copy、容器 exec、SQLite 会话、内置 Web 和 StateKnot HTTP MCP 的已审查只读工具，并提供可审核的版本安装/镜像/发布路径。持久 cron/interval 与 Telegram/Slack/Discord/飞书/企业微信/钉钉授权 inbox/outbox 已接线；Discord 本批增加单 guild 普通文字频道的 Bot 定时通知，验收见 [Discord](discord.md)。独立用户聊天/会话及受限定时任务、显式 Brokerrouter 语义检索、模型调用收据也已接线。完整多用户后台、真实模型检索质量/渠道/供应商认证仍未完成；MCP stdio/外部写入、更多渠道、durable 子 Agent、多模态与真正逐 token streaming 仍待交付。

本批增加单实例管理员的 [Web 发件箱审计](web-outbox.md)，把已有投递列表、单条详情和人工核对接入工作台；最终二进制的真实 Chromium 验收与本机回归已通过，跨平台以本批 draft PR 最终 head CI 为准。确认送达和整来源取消保留原持久状态边界，普通网关用户不能获得渠道管理权限。可选独立 `stat` / `tree` 工具仍未新增，已有 `list_dir` 提供有界递归列表。PR #76 已修复五个主文件工具和四个兼容名称的[权限与 I/O 边界](workspace-files.md)，并通过跨平台 CI；PR #77 继续迁移 grep/glob，完整条目计数、正文读取与输出预算已通过跨平台 CI。本轮 mkdir/move 目录句柄、协作锁和同卷原子 rename 已通过本机 875 项 Rust 与六组真实变更进程验收，跨平台以本轮 draft PR 最终 head CI 为准；copy 保留独立合同，stat/tree 与 durable 文件操作恢复未因此交付。

能力、框架支持状态与可落地验收条件统一维护在 [roadmap](roadmap.md)，不再在多份文档中复制相互矛盾的完成勾选。框架生产资格与实际应用集成是两个独立门槛，见 [StateKnot](stateknot-gaps.md)、[Brokerrouter](brokerrouter-gaps.md)。
