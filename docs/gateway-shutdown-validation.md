# 网关关闭测试的实际阶段

本页规定 `tests/gateway_shutdown.py` 的验收边界；生产共享期限仍是 `request_timeout_seconds + 5`，只建立一次。测试没有改变生产代码、请求预算、CI 作业上限或停机资源规则。

## 信号处理与共享排空

发送 SIGTERM 之前捕获当前日志长度，并配对本机 UTC 与单调时钟。五秒观察窗口只接受该偏移后本次关闭准入回执；重启前的 marker 不算本次处理。每次读取返回后仍核对原五秒观察期限。完整 UTF-8 必须有效，ANSI CSI 仅用于剥除日志颜色，回执时间戳必须含时区且顺序一致。

四条真实 TCP 慢响应的排空 allowance 仍为17秒，锚点是本次回执的实际 UTC 事件映射到单调时钟；Python 迟读日志不重新领取预算。读取时刻只用于诊断。该 fixture 假设同一主机 UTC 一致，异常时间戳拒绝；不认证壁钟调整、物理信号到退出的17秒硬截止或阻塞 OS 的可取消性。生产 deadline 在回执之前建立，测试不重建生产计时器。

原三组已准入效果、未知 hold 重启审核、迟到正文拒绝仍保留信号发送相对的原等待边界、原身份、禁止重放和无新准入断言。第五组真实重启保留旧marker，再用子进程 SIGSTOP/SIGCONT 在处理信号前暂停2.3秒，随后按本次事件结算原15秒排空。第六组在真实closing后暂停自有子进程、迟读3秒，原17秒必须拒绝；第18秒恢复后原child退出零。第七组读取实际closing迟6秒后必须按原五秒观察期限拒绝，原child退出零。两种迟读与旧marker是仓库内独立回归；四慢连接、grace exhaustion、无 hold/无后端效果均核对。

## 失败与受控证据

PR123 原 head `287201751a64b5e57120a0979ee446fd02ae921c` 的 Intel 候选 `114408290347` 在原 case1 等待时超出信号发送相对17秒；日志有15秒共享 grace 耗尽，但没有信号发送时刻，历史具体触发原因未证明。作业并未撞上60分钟上限；优化编译、新八更新组已通过，打包/安装/归档被跳过。六其他作业、三候选只属于原 head。

冻结原二进制下，未改四组 GREEN 与受控2.3秒信号处理前暂停触发旧17秒 RED 分开保存。初修订 `2474c70` 独立 Spec 又实际复现迟读掩盖18.056秒事件至退出（新P1）和重启复用旧 marker（继承P2）；`a286bdd` 关闭两项后，真实彩色 UTC 解析错误及慢读取越过五秒观察窗口仍分别复现。初版、中间与最终报告/负例不互相替代；受控 OS 暂停不证明原 Intel 调度根因。

源码固定审查继承 `origin/main 4b2357fcf01f47ba08d7724edbba7accfb60972c` 的累计历史，fresh 只从本地原交付 `5135fa1` 起的关闭 fixture 与本页等文档；Standards/Spec 分开报告，不称穷尽累计重审。101编译输入与冻结 SHA256 `0a3a4547977ecd690ac5af9139658cdb763da8078cf8503f085e6c1ecfb538f2` 不变。此前1272 Rust、21进程、build/fmt/Clippy属于原生产源码的继承证据，本次没有重跑或重新编译。

修订 head 必须独立完成七实际官方作业、两平台55必需Python/七实际Chromium、四优化候选各13安装归档源码与八更新/四Rust边界及全部继承验收，并核对新增第五/六/七关闭场景和 artifact 认证API摘要。原 head、父PR122、初修订和本机资格均不转移；本机没有原始 artifact ZIP 完整物理摘要。最终资格只回填 PR正文/pin，不提交自引head文档制造CI循环。

源码独立最终 [Standards](reviews/2026-10-11-gateway-shutdown-stages-standards.md) / [Spec](reviews/2026-10-11-gateway-shutdown-stages-spec.md) 聚焦开放各0；[初Standards](reviews/2026-10-11-gateway-shutdown-stages-standards-initial.md)与[初Spec](reviews/2026-10-11-gateway-shutdown-stages-spec-initial.md)原样保留。最终 `8ea34b0` 的七组实际彩色日志回归退出0；独立e663四项受控负例与root最终七组分别记录。中间报告和完整日志/原始官方失败在 `/tmp/jiaclaw-oct11-update-shutdown-delivery/pin.json` 所列路径保存，未重新执行原全Rust或其他21套进程。
