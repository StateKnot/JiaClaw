# Independent Spec review — initial 2474c70

固定累计差异 `4b2357fcf01f47ba08d7724edbba7accfb60972c...2474c706b64188486ed17b727bf6f20e203e7327`；refs、merge-base、非空和完整162个commit均核对。历史累计继承；fresh仅 `5135fa1..2474c70` 的 `tests/gateway_shutdown.py`，不是穷尽重审；未读任何Standards报告。

- **[P1，新引入] 迟读日志重置了排空起点。** spec:9要求“same17s allowance for actual15s timer”；[docs/gateway.md:172](/Users/jiawy/Documents/jiaclaw/docs/gateway.md:172)要求所有连接和worker共用单一期限。`stop_signal`在读完标记后才赋`grace_observed`（tests/gateway_shutdown.py:172–173），第1/5组从该时刻起等待（220、318），把Python调度/读取延迟额外赠给实际排空。独立原pin真实TCP/子进程probe延后读收据3秒，并在排空末段暂停本任务child：实际日志事件至退出 **18.056s**，现实现测得 **15.046s并通过**。应锚定本次实际日志事件时间并映射monotonic，读到时间仅作诊断；保留17s及独立5s上限。证据：[probe.log](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-probe.log)、[late-read-host.log](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-late-read-host.log)。这是测试资格缺陷，非生产timer修改或历史Intel根因证明。
- **[P2，继承的相关缺口] 重启可复用旧阶段收据。** spec:9的“actual closing-admission/shared-grace receipt”及spec:10的“effect/hold/restart…exact original safety assertions”要求本次阶段证据。日志以append打开（111），172行搜索整个文件；第3组267行重启后275行再次调用会立即命中前次标记。独立probe使新child保持SIGSTOP，`stop_signal`仍在 **0.000103s** 返回，标记数量始终1，故本次尚未处理信号。应只接受信号前边界之后本次新增标记。原信号相对截止及hold/no-replay断言保留；此缺口在父版本已经存在。证据：[restart-host.log](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-restart-host.log)、[probes.json](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-probes.json)。

101生产输入逐项SHA与原pin一致，原10+5s期限未动；其余三组截止/安全断言未删除。原四组GREEN、受控旧RED、修订五组GREEN与官方失败独立保存；受控复现未证明历史根因。两项资格缺口修复后仍须固定新tree独立补审与fresh官方资格，不能借用原head结果。身份：[spec-identity.json](/tmp/jiaclaw-oct11-round28-shutdown-stage-spec-identity.json)。

Spec findings：2；最严重P1。
