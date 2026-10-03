# 个人 Agent 能力差距

本项目以 [OpenClaw](https://github.com/openclaw/openclaw) 和 [Hermes Agent](https://github.com/NousResearch/hermes-agent) 为使用体验参考：清晰的安装/初始化入口，显式配置模型与凭证，再启动个人实例。不会因为竞品列出某项功能就把设计中的功能标记为 JiaClaw 已实现。

已补 copy、容器 exec、SQLite 会话、内置 Web 和 StateKnot HTTP MCP 的已审查只读工具，并提供可审核的版本安装/镜像/发布路径。持久 cron/interval 与 Telegram/Slack/Discord/飞书/企业微信/钉钉授权 inbox/outbox 已接线；Discord 本批增加单 guild 普通文字频道的 Bot 定时通知，验收见 [Discord](discord.md)。独立用户聊天/会话及受限定时任务、显式 Brokerrouter 语义检索、模型调用收据也已接线。完整多用户后台、真实模型检索质量/渠道/供应商认证仍未完成；MCP stdio/外部写入、更多渠道、durable 子 Agent、多模态与真正逐 token streaming 仍待交付。

能力、框架支持状态与可落地验收条件统一维护在 [roadmap](roadmap.md)，不再在多份文档中复制相互矛盾的完成勾选。框架生产资格与实际应用集成是两个独立门槛，见 [StateKnot](stateknot-gaps.md)、[Brokerrouter](brokerrouter-gaps.md)。
