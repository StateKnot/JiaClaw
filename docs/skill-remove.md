# 审阅本地技能卸载

`SkillRemover` 是本机操作员 Module：在一个原非排队 workspace writer 内，捕获已登记、明确停用的私有目录的完整正文和全部声明参考、原来源策略、原目录 dev/ino、调用者 UUID 与意图。先用 NOREPLACE 将原目录移入该操作的私有 `retired`，同步原目录和两侧父目录，再发布仅删除该停用声明的策略并保存历史收据。文件保留，不提供删除保留区或自动回滚命令。

```sh
jiaclaw skills uninstall reviewed --operation-id ORIGINAL_UUID \
  --if-manifest-sha256 REVIEWED_RAW_POLICY_SHA256 \
  --if-current-skill-sha256 REVIEWED_RAW_SKILL_SHA256 --config config.json
jiaclaw skills remove-status ORIGINAL_UUID --config config.json
jiaclaw skills finish-remove ORIGINAL_UUID \
  --if-intent-sha256 OBSERVED_ORIGINAL_INTENT_SHA256 --config config.json
```

配置必须 `agent.skill_lock_required=true`。先独立停用并认证 reload/GET 原服务；磁盘停用不能证明原服务已重载。CLI 只读取配置，不初始化模型、MCP、索引或会话，也不提供模型/HTTP 写入口。目标必须现存普通0700目录，叶为普通单链接0600文件；完整文件集合、原字节和声明摘要须一致，额外文件、链接、硬链接、FIFO、漂移、陈旧策略及启用项均在保存意图前拒绝。原有包512KiB、单文件128KiB、最多9文件、目录160entry和技能目录256entry预算保持。

一次新 UUID 只启动一次：跨安装、更新、卸载账本拒绝复用，三个账本的任何未结算/未完整捕获记录阻止新操作，每种账本最多64记录。失败或未知结果先只读 `remove-status`，不使用新 UUID 重发。准备期中断缺完整意图时保留原记录人工审阅，不自动清理或结算。完整意图的显式 `finish-remove` 只消费已保存字节/身份，不重读调用者输入。

状态为 `preparation_incomplete`、`prepared`、`files_retired`、`policy_published`、`completed`。原目标与退休路径、策略、捕获和全部保存状态先只读完整核验；冲突保留证据，不覆盖、不删除暂存叶、不重复退休。已有完成收据是历史：后来操作员在同名路径创建的新目录不归原操作，重复 finish 不动新目录。保存的原捕获和退休目录仍须通过身份/完整内容检查。

目录和政策有序发布，不是同时原子。退休目录后原 disabled 声明仍可严格加载，其他技能保持；政策删除后也可加载。并发 GET 返回旧表，只有独立认证 reload 成功才替换运行表；本命令始终 `runtime_applied:false`，不取消已选择只读工作或抹除历史，不撤销通用文件权限。文件同步失败、信号终止可能已经提交退休/政策/收据，必须核对原身份。跨文件系统无复制删除降级。

来源是管理员声明，hash 是内容条件而非操作代次/ABA 检测，不认证 Git 远端、发布者或签名。现有 workspace 写权限可以改变日志，没有独立不可变 ACL。此功能不提供下载、自动启用/重载/启动恢复或租户扩权，也不等于完整技能生态。

验收通过公共 CLI/Module：原 inode/完整正文和参考保留、旧打开能力、完整拒绝边界、六实际 sync→SIGKILL 阶段、恢复冲突与原暂存叶保留、三个 post-sync EIO、真实根 flock/竞争请求、跨账本身份，以及正常/400ms实际停止收据与鉴权 GET/reload。故障注入只在测试动态库，生产没有开关；本机进程崩溃不证明物理断电、内核随机故障、供应商、发行安装或 StateKnot durable。
