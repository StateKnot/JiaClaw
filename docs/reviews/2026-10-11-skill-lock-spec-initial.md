# Spec 初审：管理员技能来源锁

当前转录校正：原始初审以下文字的“七组”为记数笔误；对应 settled-final 日志实际只有六个 PASS 组。原始外部初审文件保持原样，这不增加任何验收证据。

固定累计基准 `4b2357fcf01f47ba08d7724edbba7accfb60972c...8e14f484b494fa8895d3e95eb2f77528eba64508`，fresh 范围 `68554879..8e14f484`；commit list 为 `/tmp/jiaclaw-oct11-lock-review-commits.txt`。继承架构/竞品、正文/资源与 reload 容量审查，仅重新审阅本批接线，不声称穷尽累计代码。规格为 pinned `docs/skill-lock.md`。

**生产行为新增未解决 0；初版测试夹具 P2 一项。** 规格要求“来源记录不进入模型指令，不增加原请求工具白名单”。Pinned `tests/skill_lock.py:272` 用不存在的 `user_message/allowed_tools` 发送聊天，且读取不存在的 `response`；因此原生三轮验收在进入模型前失败，CI/候选新增步骤也不能合格。已读初版失败及工作树 `messages/enabled_tools/message.content` 修正，settled-final 七组绿色证据；修正尚未进入 pinned head，不能把工作树通过记为 8e14 夹具资格。此项是夹具错误，不是生产/框架缺陷。

源码符合严格必需锁、同一读取字节的原文件 hash、目录一一对应、默认兼容、只读 doctor 非零、认证 GET 元数据及固定注册表策略；HTTP/SIGHUP 完整扫描成功后替换，失败保留旧表，实际 worker 所有权不变。来源不进入摘要指令，正文/参考工具权限不扩大。

独立运行冻结二进制（SHA256 `54a2547b…bdc7d6`）九项边界：缺根必须有锁、显式空锁、64 个技能与 64 位 Git ID、128 KiB/超一字节、65 记录、两层未知字段及重复 revision。最终日志 `/tmp/jiaclaw-oct11-lock-spec-boundaries-final.log` 全绿。我的初版配置夹具缺 agent 元数据失败另存，不冒称生产 RED。未自行执行全 Rust 或官方资格。

管理员锁与工作区写入共享权限；不是不可变审批边界、下载来源或签名证明，交付文档应明确此范围。安装/更新/撤销和完整技能生态仍未认证。
