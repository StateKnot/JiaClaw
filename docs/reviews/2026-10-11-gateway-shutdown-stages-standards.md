# Standards — final

最终硬规范违反：**0**；Fowler 启发式发现：**0**；未决测试建议：**0**。

固定 head：`8ea34b023762afc224de4af90e7b655139326cfa`。累计命令 `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...8ea34b023762afc224de4af90e7b655139326cfa`；ref 可解析，merge-base 等于固定点，累计 diff 非空，共166项 commit。累计历史继承已有审查，未声称穷尽重审。fresh 为 `5135fa1488c19fa4a87b47824b8111f8c2f92dfb..8ea34b023762afc224de4af90e7b655139326cfa`，仅 `tests/gateway_shutdown.py`。初版、中间版、e663及240报告分别保留，240已原样另存 `standards-240.md`。本结论只适用固定8ea34b0，不包括未提交交付文档。

依据用户的生产可用、期限/安全断言和缓存规范，以及 `docs/architecture.md:55–59`、`docs/workspace-files.md:165–171`、`docs/memory-files.md:44–48`、`docs/gateway.md:172–174`。标准搜索未发现其他 AGENTS、CONTRIBUTING、CODING_STANDARDS、GLOSSARY、ADR/decisions。没有运行测试/编译、编辑仓库或读取 Spec review 报告。

`tests/gateway_shutdown.py:168–198` 保留本次日志偏移、读取返回后原5秒截止、ANSI CSI清理后严格UTC解析及配对单调时间映射；`:245/345/374` 继续使用事件 `grace_started`。原三组信号相对期限和身份/needs_review/禁止重放/迟到正文拒绝（`:252–330`）、四条慢连接、exhaustion/无 hold/无 backend effect 均保留。fresh不改生产源码、配置、CI；生产一次建立并复用 deadline（`crates/jiaclaw-host/src/gateway/mod.rs:384/434`），符合 `docs/gateway.md:172`。

第五组 `:333–344` 保留真实停机/重启旧marker及处理前暂停；第六组 `:354–394` 保护3秒迟读不重发17秒drain预算，验证真正 `TimeoutExpired` 后另行恢复/零退出/零hold效果。第七组 `:396–420` 在本次真实关闭回执出现后迟读6秒，要求精确的原5秒观察拒绝文本并确认延迟注入实际发生，再单独结算child、复查零hold/效果、finally恢复读取helper。它补齐此前未决覆盖建议；后续settlement窗口没有替代原deadline。

全部12项已考虑：Mysterious Name、Duplicated Code、Feature Envy、Data Clumps、Primitive Obsession、Repeated Switches、Shotgun Surgery、Divergent Change、Speculative Generality、Message Chains、Middle Man、Refused Bequest。无值得修改的新问题。两个迟读wrapper服务不同期限和真实阶段，显式保留各自断言且finally恢复，无需增加通用注入抽象；工具覆盖项跳过。

此为最终fresh静态 Standards 结论；不认证原17秒物理信号至退出或历史Intel根因，不借用旧本地/官方资格，也不宣称本审查执行了七组验证。
