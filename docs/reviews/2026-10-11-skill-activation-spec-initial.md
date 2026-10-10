# Spec 初审：持久技能启停

固定累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c...dc8dc2029917a6ff96b79d2bdf282825d474fcdb`；fresh `9b0b2d91..dc8dc202` 八文件，完整 commit list `/tmp/jiaclaw-oct11-activation-commit-list.txt` 已读。继承架构/来源锁/正文/参考与容量审查，仅重新审阅本批实际策略与接线，不声称穷尽累计代码。主规格 `docs/skill-activation.md`。

**生产新增未解决 0；初版验收夹具 P2 一项。** 已保留的 `docs/skill-read.md` 要求“显式 enabled_skills 仍代表调用方直接选定并注入技能”。新 `tests/skill_activation.py` 的 `chat()` 对按需读取和 legacy 两种场景都发送 `enabled_skills=['reviewed']`；Model 的 enabled/race 首轮却要求正文不在提示中。这与仍正确保留的生产显式正文注入冲突，会使原生第5组验收失败，不能据此回退生产合同。应仅 legacy-disabled 发送显式选择，按需场景留空；实际初始失败尚待作者执行保存，不能冒称已运行 RED 或六组资格。

v2 自定义布尔反序列化拒绝 null/非布尔，v1 明确拒绝 enabled，重复/未知字段与版本受封闭 schema 限制；整个锁先解析验证，来源/hash 和64条/128KiB预算沿用。已停用普通目录在打开叶文件前跳过，可完全删除；一级链接与技能根仍经严格扫描拒绝。启用项逐文件绑定原字节并按启用数量核对完整覆盖，名称重复只对实际表检查。

`inspect_policy` 一次读取完整锁，clone 的有界排序输出与随后严格扫描共享同一 captured lock；关闭必需锁时拒绝。注册表现有原子成功发布、固定策略及实际 worker 容量不变。停用项不进入 snapshot，因而不进入后续摘要/关键词/显式注入，正文/资源未来选择拒绝；先前已选只读工作和原许可可结算，不夸大为取消、ACL 或抹除历史。

父版本不支持v2的实际负例只证明能力缺口；原诊断文字断言错误单独保留，不是上游缺陷。新全Rust、六组实际流程与官方候选仍待独立核验。
