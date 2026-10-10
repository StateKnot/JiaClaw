# 私有状态文件处理

模型调用账本和语义索引实际使用同一个私有 `private_state_file` Module。它的 Interface 只有 `inspect(path)` 和 `open_or_create(path)`，返回标准元数据或文件句柄；各存储 Adapter 将 I/O 错误映射回原有 `model-call store:` / `semantic store:` 错误，不新增公开方法。

本批保持原有文件合同：不跟随叶子链接，只接受普通文件；Unix要求一个硬链接与精确的0600低九位权限。创建使用 `create_new`、0600及 `O_NOFOLLOW | O_NONBLOCK`；已存在文件先验证再打开，不截断或修改权限。打开后重新检查命名文件，Unix核对句柄与路径的dev/ino及单链接身份，成功才返回句柄。目录由管理员拥有；不保证在敌对管理员不断替换祖先或原文件时安全。

目录规范化、祖先链接拒绝、0700父目录及workspace隔离由各Adapter负责。锁获取与持有、数据库身份/header、schema迁移、WAL/SHM/journal恢复大小、容量预留、收据与未知hold继续由各存储负责。模型调用与embedding状态机没有合并，不增加自动重发或工具恢复。

文件预检和用于SQLite header的临时句柄仍在SQLite打开之前结束。Module不缓存、复制或持有额外句柄，不在SQLite取得POSIX锁后再关闭同inode的预检句柄；所有权句柄保留到实际存储生命周期结束。

验收使用真实文件系统：合法私有字节重开不截断；软链接、悬空链接、多硬链接、0644文件、目录、Unix socket和FIFO拒绝后字节/权限不变。两个实际Store保留原错误前缀；既有sidecar、迁移、重启、独占锁、SQL回滚和未知hold继续验收。macOS不会从 `F_GETFL` 回显 `O_NOFOLLOW`，不将回显作为跨平台断言，生产标志未改变。首轮测试错误使用unsafe libc，被仓库规则阻止，日志单独保留；最终使用既有安全测试方式。

固定head的本地与官方资格分别记录在[验证文档](validation.md)，不能借用父PR #105。这是历史Standards P3私有文件重复启发式的结构改进，不宣传为既有越权漏洞修复。真实供应商/渠道安装、StateKnot durable、stdio/外部写入、多模态及公开发布仍分别开放。
