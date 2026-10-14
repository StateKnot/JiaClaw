# 完整会话资料迁移：发布前验收

固定源码 `1e5f4ee8176987f6943760473346dd2205278f00`，生产边界修订 `9196eb7`；相对父批本机 `fd15fa9` 新增十条路径。累计 `origin/main` 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c` 的旧结论和 PR143 完整资格仅历史继承，不称累计穷尽重审。合同见 [session-transfer.md](../session-transfer.md)。

复用原 export/import、SQLite Store 和 FileReader，在 standalone 工作台明确下载完整 JSON 或导入全新 UUID。新 `preserve=true` 只让最多51条消息完整保留，原缺省压缩和网关入口范围保持。没有新读引擎、存储、模型工具或自动模型调用；资料迁移不恢复任务、工具效果、账本、反馈、配置或 durable 执行。

116 编译输入逐字绑定最终源码和冻结 binary，manifest `6084a630fe298751c0894e54b8c70f7ef838d6ee14dd80f44ce2063a6457c723`；最终 binary SHA256 `f9efa2e42384efe2db50e9b58de50686f1318c0830cb573e0b1534797889b5fe`。最终本机1321 Rust（473/123/725）和1忽略、四线程、locked build、fmt、必需 correctness/suspicious Clippy 均通过；209条非必需 host-test warning 保留，不称零 warning。

最终新资料迁移一条真实 Chromium 命令、原 standalone stream 和原 tenant preview 各一条，以及 parser13 全部通过。Playwright1.62.1 使用现有 custom Chromium153.0.8010.12（1243），不是缺失的默认1234。没有本机完整68 Python、31 Chromium、优化打包安装或 Docker 验收，不能借本机补新 head 官方资格。

新实际原 Store/HTTP/SQLite/Chromium 验收：完整70218字节物理 JSON，正文超过 DOM 显示上限；BOM/CRLF/TAB/NUL/Unicode 原样；显式预览确认前零 POST，原 source 不变、全新 UUID、真实 SIGKILL 后原 GET 核对。完整51条和 exact262144字节 JSON 均无压缩，52条/262145字节/非法UTF8/额外字段/重复JSON/错角色/surrogate 整次拒绝。共四次明确导入，其中一次实际已提交响应丢失后只原目标 GET 恢复，零重写、零模型调用；身份迟到不恢复私人 DOM 或下载，网关 full/readonly 不显示入口并沿原404拒绝新参数，mobile375、零storage/pageErrors。

初618源码两轴分别确认 P3：真实原 GET 延迟和同步 Blob URL 准备使保存请求发生在原15秒后（15512.8/15218.5ms）。最终在 `link.click()` 前再次核同一绝对截止；各轴原条件独立复验均拒绝保存，零 Blob URL/anchor 泄漏，普通下载仍通过。原报告和物理反例保留。中间234新回归夹具等待了错误状态控件，原 task catch 实际写入 `#status`；初实际 timeout 独立保留，1e5仅纠正夹具观察，无期限或生产字节变化。最终 root 原13.5秒 GET +2秒同步准备在15513ms观察到明确拒绝和零下载。不是 OS 主线程硬抢占认证。

源码 Standards/Spec 初、最终和中间夹具报告原样保存在本目录邻接的 reviews，最终各零开放问题。源码、交付、发布身份与新固定 head 官方资格分别认证；本记录是发布前 checkpoint。新 head 仍需自己的七 actual、四候选各13归档安装源码/API digest、两平台68普通Python/Ubuntu31真实Chromium和原175合同及新两 guard 共177，再独立 fresh 官方两轴，才回填正文/pin；父143或本机结果均不转移。

构建前读取 df；共享 target 按同仓库/工具链/配置评估保留，不复制或清理，实际回收0。Rust 每命令关闭增量及 dev/test 调试符号，原断言和溢出检查保持。日志、冻结二进制、各阶段错误及审查身份存于 Git 排除的 `.jiaclaw-evidence/round125-next/`，不从 APFS du 或其它进程 df 变化声称回收。
