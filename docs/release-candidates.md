# 四平台候选发布验收

`Release qualification` 在修改 Cargo、工具链、crates、发布工作流或安装/E2E/MCP 验收的 PR 上运行。四个原生 runner 分别构建 Linux x86_64/arm64 和 macOS x86_64/arm64；完整锁定 Rust 测试后，针对目标运行优化构建、实际二进制 E2E/SQLite/MCP、最终归档安装与失败回滚。普通 CI 的全套渠道、浏览器和容器验收仍独立执行。runner 标签与支持范围见 [GitHub 官方说明](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)。

PR 的版本来自 Cargo metadata，不把 `refs/pull/.../merge` 当成版本。构建实际测试的 merge checkout，并记录 commit/tree/parents；审核时核对与该 PR head/base 的关系和 tree。原生 ELF64/Mach-O64 的 CPU 必须匹配宿主；仅在模拟器下可运行不算对应架构通过。打包脚本直接生成单文件 USTAR/gzip，清除宿主 owner、时间、扩展属性及 AppleDouble，仅允许最多256MiB的常规输入，独占创建输出、失败移除本次部分归档。最终 tar 只能有一个名为 `jiaclaw` 的普通文件，模式0755且全部字节必须等于测试过的二进制。链接、越界路径、重复成员、错误二进制和错误架构均有实际负例。本机旧 `tar` 路径实际夹带 `._jiaclaw` 与 provenance 属性并被新门槛拒绝；新打包保留二进制签名字节而去除归档外部元数据。

安装 fixture 将**该最终归档的原字节**作为固定版本下载响应，执行生产 `scripts/install.sh`，再运行新安装的二进制。校验和错误、可运行但版本不匹配、校验文件缺失和非法版本须保留旧字节且没有暂存遗留。还实际调用生产 packager 验证拒绝覆盖已有资产、拒绝链接输入，以及 RLIMIT_FSIZE 触发真实写失败后移除本次部分文件。进程被强制杀死可能留下部分文件，该次作业失败、不得上传或发布；没有跨进程清理承诺。这里不使用公开下载或供应商凭据，也不能证明 GitHub/CDN 真实下载资格。

每个平台上传一个七天保留的 Actions artifact，包含原归档与 JSON：版本/平台、实际 checkout commit/tree/parents、干净的 tracked source、二进制和归档 SHA256、已通过的安装项目。证据仅在完整安装/回滚成功后写出。维护者在保留期内保存对应资产、JSON 和完整 job log；不同提交、旧归档或仅成功的 build 步骤不能代替最终安装验收。没有声明跨构建机器的字节可复现、签名/attestation 或所有发行版兼容。

父引用直接读取 commit 对象原始 header，不能使用浅克隆边界下会隐藏父提交的 `git show %P`。安装验收另外创建真实两提交仓库和 depth=1 的本地 clone，证明图遍历返回空列表时仍保存真实父 SHA；不要求为记录元数据下载父对象。本批中间 head `e009f809` 的 Linux arm64 安装虽通过，JSON 曾因该问题漏记 parents，审核拒绝将其作为完整证据；修订后所有平台须用最终 head 重新验收。

## PR #91 最终固定提交验收

2026-10-09 已完成 [PR #91](https://github.com/StateKnot/JiaClaw/pull/91) 最终 head `4edc04c868b07096a4ebe367d5b0083988822d72` 的证据核对。[普通 CI 37885182415](https://github.com/StateKnot/JiaClaw/actions/runs/37885182415) 的 Linux/macOS/真实容器三作业与 [Release qualification 37885182482](https://github.com/StateKnot/JiaClaw/actions/runs/37885182482) 的四个原生平台均成功；PR 的发布作业明确 skipped。实际测试 merge `35d4802a49ba189501d9d42597bd5aab4e2df9ef` 的 raw parents 为 base `08bff050ef3d7d1c77b2b95fe91880318de65cb2` 与该 head，tree 与 head 均为 `8760e3555772ff01671d38e9724defdc754b6ca7`。

每个平台完整 Rust、优化构建、实际二进制 E2E/MCP 与最终归档的13项安装验收通过；原生二进制架构匹配，七天资产 JSON 保存准确 raw parents。普通两平台另通过30套进程验收，Linux 的四套 Chromium、真实 Docker exec、镜像隔离/重启/真实 ENOSPC 也成功。没有将中间失败/取消作业作为最终资格。

| 原生资产 | 最终归档 SHA256 |
|---|---|
| Linux arm64 | `82eaf4dedc88257b9052bd8aba347ade74863891388794e4c48d5a5d9be4b16b` |
| Linux x86_64 | `dfcebfeb5442760c72c04757528c366b34c5b461b53d968b171d70bc70d664de` |
| macOS arm64 | `b2b72b308e8b599cec7f696555e7e2a08470aeb11d06be0eabd16f37d390d212` |
| macOS x86_64 | `2404ec27dca53b3ecfb822af1de4e81d55cf30fe10b68daf7c47545011a7b4da` |

归档摘要取自各原生 CI 的实际打包/安装 JSON 与完整 job log，Actions 官方 artifact ZIP 摘要与各上传日志匹配。本机下载这些 CI ZIP 的存储连接不可用，未声称独立本机逐字节复核 CI ZIP；前述本机冻结优化二进制与归档是独立实测资产。PR 仍为 OPEN/draft，无自动合并、tag、公开 Release 或推送镜像。该证据只认证上述固定 tree，后续 MCP 修改另跑最终提交的完整门槛。

本机复现（示例为 macOS arm64，使用当前 Cargo 版本生成资产名）：

```sh
cargo build --release --locked --target aarch64-apple-darwin -p jiaclaw-host
mkdir -p /tmp/jiaclaw-candidate
python3 scripts/package_release.py target/aarch64-apple-darwin/release/jiaclaw /tmp/jiaclaw-candidate/jiaclaw-v0.1.0-aarch64-apple-darwin.tar.gz
python3 tests/installer.py target/aarch64-apple-darwin/release/jiaclaw --archive /tmp/jiaclaw-candidate/jiaclaw-v0.1.0-aarch64-apple-darwin.tar.gz --evidence /tmp/jiaclaw-candidate/aarch64-apple-darwin.json
```

保存 evidence 要求 tracked source 无改动。只测试本地未提交修改时省略 `--evidence`，控制台会明确记录 dirty。Linux 归档构建基线为 Ubuntu24.04/glibc2.39，macOS 为14 arm64/15 Intel；较老平台仍需自行编译或独立 ABI 验收。

PR 只有 read token，`draft` 作业条件明确排除 PR；不会创建 tag、Release 或写入发布资产。维护者推送匹配 `v<workspace.version>` 的 tag 后复用相同四平台链路，全部成功才创建带 `SHA256SUMS` 的 draft；公开发布仍须单独人工审核。应用、StateKnot durable、真实平台/供应商及恢复资格继续以各自认证范围为准。

2026-10-09 工作区锁所有权修订后，本机 macOS arm64 默认并行1171项Rust、fmt、必需Clippy与锁定优化构建通过；同一冻结二进制的 E2E/SQLite、MCP、原生工具、文件、搜索、目录移动、copy、stat/tree、记忆、语义记忆和最终归档安装共11套全部通过。二进制 SHA256 `806a6a1bdeec958361f285df2958fbb6bfc33912b90fac00a9dba277ade284be`、归档 `f1ce61f34cf6785367d09f05104fcd65551221453725bb5406cf8af70d7301a0`。最终固定源码的四平台结果见交付 PR checks 与对应 JSON artifact。尚无公开 Release，不把该工作流接线本身记为四平台通过。

首轮普通 CI `37879904141` 的 macOS Discord fixture 在两个独立连接间用 TCP 写入推断 body slot 已取得，未观察到429而失败；不能据此确定具体调度根因。修订 fixture 以实际 Hyper `100 Continue` 确认 handler 已轮询 body（生产代码先取每绑定许可），随后要求第二请求精确429、原请求精确408，再以401确认超时后许可已释放；保留原1.2秒观察和3秒完成预算，不改生产2秒 body 超时或任何授权。本机同一优化二进制的最终双租户九组全部通过，屏障分别0.313/0.266ms。最终验收以修订后固定 head 为准，首轮失败保留。

第二轮候选 `37881740889` 的 Linux x86_64 Rust 并发追加测试在原200×2ms contention 预算后仍观察到 EWOULDBLOCK；本机默认并行也在 mkdir 观察到 busy。真实 dup 回归证明，仅关闭操作描述符不能释放仍被其他引用保留的 flock；本次 CI 的具体 fork/调度重叠未获证明。生产 mutation 现由不可克隆的所有权 guard 持锁，完成时由取得锁的进程显式解锁再关闭描述符；异 PID 的 guard 析构不提前解除原 owner。没有新增等待、自动重试、全局串行测试或放宽期限。实际 dup 测试覆盖操作完成后旧引用仍打开时新 owner 可准入、旧引用关闭不解除新 owner，以及忙时零写入；既有取消测试要求阻塞 worker 到实际发布才释放锁和容量。强杀不保证析构，不把本修复当作崩溃恢复资格。最终四平台固定 head 必须重新验收；旧失败保留。
