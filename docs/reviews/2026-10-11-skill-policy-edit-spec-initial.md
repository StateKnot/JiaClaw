# Spec 初审：条件技能策略编辑

固定累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c...def298217bafc61ef219c416608fa7dcdcb22ec3`；fresh `3c4dde50..def29821` 八路径，完整提交清单 `/tmp/jiaclaw-oct11-policy-edit-commits.txt` 已读取。继承架构、发现/读取/来源锁和启停审查，不声称穷尽累计重审。主规格 `docs/skill-policy-edit.md`。

**新增待验证 P2 一项：规范化 v2 的无变化写入不消耗原 hash。** 规格说：“若其他写入者在修改前取得同一原hash，只有一个协作修改可以提交，另一个必须重新读取并审核。” `skills/lock.rs::set_enabled` 不区分目标 enabled 与现值相同；规范化 v2 已与 `next` 字节一致时，`memory_io::edit_file` 仍 rename，返回成功而原 hash 不变，后续协作写入仍可使用该 hash。该问题由固定源码可推出；独立 CLI 重现 `/tmp/jiaclaw-oct11-policy-edit-spec-noop.py` 已准备但二进制尚未冻结，因此不声称已执行。建议明确拒绝无变化，或明确不发布的 no-op 及其并发合同；不能只沿用“只有一个提交”的描述。

其余 fresh 要求未发现缺失或范围扩张：显式 bool/hash 与目录登记、v1→v2 来源/其他启停值保留，原 FD 读取与 parse/hash、候选完整扫描、0600暂存/sync/rename/目录sync 均在同一非排队 workspace writer 生命周期内。停用漂移仅验证候选启用表，坏 enable 和提交前错误不写 manifest；缺失锁不创建，特殊/链接/超限读取沿用受限 I/O。输出 runtime_applied:false，只有独立 reload 修改内存表；模型/HTTP 写端点未添加。post-rename fsync 错误保留可能已提交诊断，没有自动重试。

新全 Rust 正在运行，父 CLI 负例不是当前资格；尚未执行新的生产二进制矩阵，不借旧1246测试或 PR114 资格。
