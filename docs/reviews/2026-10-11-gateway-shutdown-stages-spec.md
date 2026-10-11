# Independent Spec review — final 8ea34b0 fixture

固定累计差异 `4b2357fcf01f47ba08d7724edbba7accfb60972c...8ea34b023762afc224de4af90e7b655139326cfa`；refs、merge-base、非空及166个commit已核对。fresh仅`5135fa1..8ea34b0`的`tests/gateway_shutdown.py`；历史累计继承，不是穷尽重审；未读任何Standards报告。

**Spec findings：0。** 101生产输入SHA与原pin逐项相同，生产10+5s单一期限、其余三组原signal-relative截止和exact safety assertions保持；无生产instrumentation或付费请求。

初247 P1迟读锚点/P2旧marker，以及a286中间ANSI解析/5s慢读越界，均在独立e663真实原pin子进程回归中关闭：17.0032s事件相对拒绝、5.0089s旧marker拒绝、真实ANSI接受、6.2054s迟读过期拒绝；child全部结算零。最终helper与e663逐字节相同，UTC事件映射monotonic、读到时间仅诊断，拒绝旧收据与不一致clock；保持同hostwallclock假设，不认证clock adjustment。[四项独立证据](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-e663-probes.json)

对后续两次fixture差异独立补审：

- spec:9–11：第5组先真实stop/start留下旧marker，再原2.3s预处理暂停；必须验证本次新事件、原5s接受/15s排空及无hold/effect、零退出。
- spec:9：第6组本次closing后真实暂停child/迟读3s，18s恢复；原17s必须因`TimeoutExpired`拒绝，随后原child结算零。finally恢复reader、cancel Timer、SIGCONT自有child并join；受控OS暂停不代表生产失败。
- spec:9、11：第7组真实本次收据读到后迟6s，必须精确拒绝原5s观察期限；child零退出、无hold/effect，finally恢复reader。额外覆盖为明确授权的持久回归，未扩大生产行为。

root固定8ea本次执行 `env -u NO_COLOR python3 tests/gateway_shutdown.py /tmp/jiaclaw-oct11-update-delivery/jiaclaw-final`，session85791实际退出0；[七组日志](/tmp/jiaclaw-oct11-round28-shutdown-seven-final.log)与我独立四项probe分开记录。[最终身份](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-final-identity.json)

初247、a286/e663两次中间及240补审报告/RED原样保留。历史Intel根因未证明；spec:13仍要求最终交付固定tree的fresh官方资格，不能借用父head部分资格或把本机通过称为官方通过。
