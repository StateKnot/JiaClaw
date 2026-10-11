# 本机审阅技能包安装

`SkillInstaller` 是本机操作员接口，安装一个已审阅的 UTF8 JSON 包到新的、尚未登记且不存在的技能目录。必须显式配置 `agent.skill_lock_required=true`，并已有有效来源锁。不访问远端、不替换已有技能、不自动启用或重载，不初始化模型、MCP、语义库或会话。下载、更新、卸载、签名、独立 ACL、完整技能生态与 StateKnot durable 恢复继续开放。

```sh
jiaclaw skills install --config config.json --operation-id UUID \
  --from reviewed-bundle.json --if-bundle-sha256 BUNDLE_SHA256 \
  --if-manifest-sha256 ORIGINAL_POLICY_SHA256
jiaclaw skills install-status UUID --config config.json
jiaclaw skills finish-install UUID --if-intent-sha256 CAPTURED_INTENT_SHA256 --config config.json
```

UUID 必须在启动前生成并保存，使用规范小写、非零 UUID。输入相对工作区，普通单链接、有界 UTF8，拒绝链接、硬链接、FIFO 与路径穿越。包原始 JSON 最多512KiB，严格 schema1，最多9份文件，每份最多128KiB；必须恰好是完整原始 `SKILL.md` 和它声明的至多8份 `references/` 资源，无额外脚本、二进制或压缩包。正文解析复用发现器的同一规则，正文与每个资源均核对原字节摘要。

```json
{"version":1,"directory":"reviewed","source":{"repository":"https://github.com/example/skills","revision":"固定40或64位小写Git对象ID","skill_sha256":"完整原始SKILL.md的SHA256"},"files":[{"path":"SKILL.md","text":"完整原始正文"}]}
```

`source` 是管理员声明：要求无凭据、无 query/fragment 的 HTTPS 来源、固定对象ID和完整正文摘要，不验证发布者签名、Git 远端或远端对象。hash 是内容条件，不是操作代次或 ABA 防护。包摘要覆盖包中的全部文件，也不构成发布者认证。

## 发布与所有权

同一个原 workspace 非排队 writer 持有输入捕获、原策略 hash 核对、完整候选扫描、持久意图、私有暂存、策略与目录发布、完成收据。其他协作写入立即拒绝，取消/杀进程前不释放 owner；不拼接独立 register/copy。Linux/macOS 使用目录原子 NOREPLACE，拒绝现存目标，不降级成检查后覆盖。

原策略 v1 迁移 v2 时保留所有原启用状态和来源；新增声明明确停用。快照、暂存文件与各相关父目录在发布阶段执行实际 sync。先发布停用策略，再发布已核验目录，最后完成收据；这不是两个命名空间的同时原子可见。读取者可能观察停用声明而目录尚缺，加载仍保留旧技能；跨阶段读取可能失败，失败 reload 原表不变。目录出现或 reload 成功不代替安装内容核验。

成功返回 `state:completed, runtime_applied:false`。必须另行 `set-enabled`（新的策略 hash 条件）以及认证的服务 reload/GET 或新进程启动才能应用。安装不撤销通用文件权限、不取消已经选择的只读工作、不抹除会话历史、不增加租户或模型工具权限。

## 原身份核对

私有 `.jiaclaw-skill-installs/UUID/` 保存原包、原策略、候选策略、严格版本1意图和完成收据；目录0700、普通单链接文件0600，读取仍使用能力句柄。最多64份记录，包含未结算记录，单包/策略/扫描预算固定，达到容量则停止，须停服并由管理员审阅归档；程序不自动遗忘原身份。现有 workspace 写权限仍可改变这些文件，没有独立不可变 ACL；摘要/身份不一致或不安全文件一律拒绝，不宣称防管理员篡改。

status 只读，使用原 writer 获得稳定观察，活跃 owner 存在时立即拒绝。不存在的身份不创建状态。`preparation_incomplete` 表示意图尚未完整落盘，程序未开始外部策略/目录发布；保留记录，须管理员审阅，不能自动重发或用不完整快照完成。成功落盘意图后的观察依次为 `prepared`、`policy_published`、`files_published`；`completed` 是历史收据，不认证现在仍未被操作员修改、仍启用或已加载。

收到非零、信号中断或丢失 stdout 时，先查询保存的 UUID。`finish-install` 必须提供查询到的原意图摘要，只消费已捕获的同一原包/策略，不重读输入、不分配新ID、不在启动时自动恢复。它只补齐原操作剩余阶段；当前策略、目录、捕获内容与原意图冲突时停止，保留证据，不覆盖、不自动回滚。已完成操作 finish 不重新安装；新 install 即使使用相同包，也拒绝复用旧UUID。未结算/不可核对记录阻止新的安装操作。

发布后的 sync/收据失败可以已经提交，不能按退出码推断零效果。实际 SIGKILL 与受控 fsync 错误验证本地原身份核对与显式结算；不认证物理断电、文件系统/网络卷耐久性、供应商、真实远端安装或 StateKnot durable。非协作编辑器/进程不被这个文件锁序列化，操作者须尊重工作区维护边界。

## 验收接口

公共 `install/inspect/finish`、真实 CLI、原 workspace writer、鉴权服务 reload/GET 与实际 OS sync/SIGKILL 为验收边界。生产不包含测试暂停或故障注入开关；仅测试构建的 C 动态库在临时工作区拦截真实 fsync，已执行阶段有实际收据。官方 CI 与四个平台优化二进制必须在本批固定 head 独立执行新回归，不能借父PR或本机资格。
