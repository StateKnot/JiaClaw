# 定时投递的当前会话 schema 验收

原任务用户反馈通过原会话 Store 将 SQLite 从 schema11 事务升级为 schema12，定时投递也使用同一会话数据库。Telegram/Slack 与 Discord 定时投递夹具必须精确核对当前 schema12；不能以允许任意版本或移除版本检查代替验收。

PR138 初始远端 `abc8ca8f8b9f69e9d4e88221ffac9fc5baf698de` 的 Linux/macOS 主验收都在 `tests/scheduled_delivery.py` 原 schema11 断言失败。固定生产二进制 `b44c8780ce6bcecd7815dc63e82f860d3b2fa9b4aae4eda0e80c391c87200d6e` 在本机原夹具重现同一失败；仅加只读诊断的临时副本实际读到 schema12。原 Discord 定时投递夹具也在旧 schema11 断言重现失败。这是应用夹具没有同步当前迁移契约，不是上游框架缺陷，也不是运行超时。

本次修改只将两个夹具的预期版本从11改为12。原模型/平台请求、权限、投递顺序、分片、重启、取消、未知效果、持久身份、人工核对和所有等待期限及断言保持。生产 Store、迁移、模型工具、网关和工作台不变。原反馈升级的停服务备份要求与旧二进制拒绝 schema12 的边界仍适用。

初始官方两个失败 job/check-run/annotations/完整日志、五个成功实际作业和四候选归档/API digest分别保留。失败主验收尚未执行后续完整 Chromium/Docker 验收；前缀和旧成功不能转移为修订 head 的完整资格。修订必须重新完成自己的七个实际作业、两平台66个必需 Python/Linux19套真实 Chromium、四候选各13项安装归档源码/API摘要和完整168审计，再由独立 Standards/Spec 核对官方证据。

本次 fresh 范围为上述两处夹具和本说明；既有反馈源码、本机1316Rust/19Chromium及累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c` 审查只历史继承，不称穷尽累计重审。不使用旧已合格PR137及更早批次审计器，不放宽原60分钟/三秒header/十五秒grace/250ms或任何夹具预算。
