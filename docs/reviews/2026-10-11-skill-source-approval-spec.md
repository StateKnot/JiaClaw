# Spec 初审：停用技能来源确认

固定累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c...5bc56d718191fc7169956de17a078bc5b390ae1d`，完整 git log 已读；fresh `d726e910..5bc56d7` 七文件。继承此前架构、正文/资源、来源锁、条件启停及已关闭问题，不声称穷尽累计重审。主规格 `docs/skill-source-approval.md`，未读取 Standards 报告。

**新增未解决 0。** 规格“只修改已登记、已停用技能的来源声明”“成功仅返回 disk_policy 和 runtime_applied:false，目标仍停用”均落实：必需锁、目录/显式字段/来源验证、原 hash 条件、当前停用、不同来源、原始正文解析/完整 hash，最终保留目标 false 和其他声明。改源与启停共用同一实际 workspace writer 的原锁读取、候选全表扫描、暂存 sync/rename/目录 sync，拒绝不发布；未新增模型/HTTP 写入口或自动运行应用。

规格“目标正文额外至多读取128KiB”“参考文件原始字节在实际按需读取时另行核验”没有被扩大为整包认证。来源仅管理员声明；HTTPS/无凭据或 query/fragment、固定对象 ID、内容 hash/ABA、post-rename 可能已提交/先检查均准确，签名、下载、安装、ACL 与取消承诺没有增加。

本人在冻结新 binary 实际执行七组 CLI 补边界全部 PASS：完整原 hash 包含 frontmatter、64位对象 ID/其他来源保持；128KiB 精确界与+1拒绝；声明缺失参考文件仍仅解析、不认证其字节；另一启用项漂移拒绝；64停用声明含63缺失目录、缺失目标不创建；原 manifest 空白也参与条件；两个来源/启停命令共享 hash 与 writer 只一方提交。预检 df 307276848KiB，无网络、编译、生产或共享夹具修改。

94输入在固定5bc Git blob/当前文件字节一致；binary SHA256 `053d307a…74ad2`，日志 `/tmp/jiaclaw-oct11-source-approval-spec-boundaries-initial.log` SHA256 `e4bb1d4b…b2010`。本报告未核对作者完整 Rust/整套进程或新官方资格，不借父1249测试或 PR115 合格。
