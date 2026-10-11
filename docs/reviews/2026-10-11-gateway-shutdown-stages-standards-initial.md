# Standards

硬规范违反：**0**。Fowler 启发式发现：**0**。

固定累计比较为 `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...2474c706b64188486ed17b727bf6f20e203e7327`。两端 commit 均解析；merge-base 等于固定点，累计 diff 非空。162 项 commit 清单与提供的 `/tmp/jiaclaw-oct11-round28-shutdown-stage-commits.txt` 一致。累计历史继承原审查，本次没有声称穷尽重审；fresh 仅 `5135fa1488c19fa4a87b47824b8111f8c2f92dfb..2474c706b64188486ed17b727bf6f20e203e7327` 的 `tests/gateway_shutdown.py`。

已搜索 AGENTS、CONTRIBUTING、CODING_STANDARDS、GLOSSARY、ADR/decisions，未发现其他文件。依据用户提供的生产可用、期限/安全断言和缓存规范，以及 `docs/architecture.md:55–59`、`docs/workspace-files.md:165–171`、`docs/memory-files.md:44–48`、`docs/gateway.md:172–174`。只读代码；没有运行测试/编译或读取 Spec review 报告。

`tests/gateway_shutdown.py:166–190` 保留既有 5 秒信号观察 barrier、17 秒排空 allowance 和退出码检查；新增时间诊断没有重设起点。`:217–224` 从实际关闭准入回执观察点计时。生产 deadline 仍在 `crates/jiaclaw-host/src/gateway/mod.rs:384` 一次建立，回执在 `:397`，并于 `:434` 使用该原 deadline。fresh diff 不改变生产源码、配置或 CI 期限，符合 `docs/gateway.md:172` 的共享期限规则；修订不再认证原 17 秒物理信号至退出期限。

`:153–164` 的四条慢连接、`:221–222` 的 grace exhaustion/无 hold/无后端效果，以及 `:227–305` 三组原始信号相对期限、原请求身份、needs_review、禁止重放和迟到正文拒绝均保留。`:307–325` 只暂停 fixture 子进程，恢复线程有界结算并复查相同安全边界。`read_running_log` 复用既有严格 UTF-8 helper（`tests/host_log.py:5–8`），没有忽略完整坏字节。

逐项考虑全部 12 项：Mysterious Name、Duplicated Code、Feature Envy、Data Clumps、Primitive Obsession、Repeated Switches、Shotgun Surgery、Divergent Change、Speculative Generality、Message Chains、Middle Man、Refused Bequest。未发现值得修改的新问题；两组慢响应试验复用 `slow_downloads()`，保留各自核心断言便于直接比较，无需新增抽象。工具已覆盖项不重复报告。

此结论是 fresh 静态 Standards 审查；原 Intel 失败、原四组 GREEN、受控旧 RED、新五组 GREEN及新官方资格必须分别保留，不能互相替代。
