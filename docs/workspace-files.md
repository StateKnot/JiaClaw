# 工作区文件工具与权限边界

本页覆盖 `read_file`、`write_file`、`delete_file`、`str_replace`、`list_dir`、`grep`、`glob`、`mkdir`、`move`，以及四个兼容名称。九个主工具共用有界目录句柄 I/O，mutation 另受协作写入锁控制。本轮补齐 mkdir/move 的原子变更边界，验收状态见文末。独立的 `copy` 保留自己的原子复制合同，不因此继承本页所有资源/锁语义。可选 `stat` / `tree` 仍未新增。

## 配置与兼容名称

| 配置开关 | 主名称 | 兼容名称 |
|---|---|---|
| `tools.read_file.enabled` | `read_file` | `file_read` |
| `tools.write_file.enabled` | `write_file` | `file_write` |
| `tools.delete_file.enabled` | `delete_file` | `file_delete` |
| `tools.list_dir.enabled` | `list_dir` | `file_list` |
| `tools.str_replace.enabled` | `str_replace` | 无 |
| `tools.grep.enabled` | `grep` | 无 |
| `tools.glob.enabled` | `glob` | 无 |
| `tools.mkdir.enabled` | `mkdir` | 无 |
| `tools.move.enabled` | `move` | 无 |

这些开关默认 true；关闭开关会移除该主名称及存在的兼容名称，别名不能绕过配置。请求的 `enabled_tools` 仍按确切名称授权：允许 `read_file` 不隐式允许 `file_read`，反之亦然。普通聊天传空数组沿用“允许全部已注册工具”的行为，见[原生工具合同](native-tools.md)。

兼容名称现在直接使用主名称的参数 schema、描述和执行路径，不另行改写参数。已有 `path` / `content` 请求继续可用，别名也可使用主名称的行范围、追加模式等参数。**这是安全相关的兼容变化**：四个别名的成功结果由旧人类可读字符串改为同结构 JSON；缺失的读取、目录或删除目标返回错误，不能再将旧提示文本当作成功。调用方应解析 JSON 与错误状态，不匹配旧展示文字。

关闭文件写入可分别配置：

```toml
[tools.write_file]
enabled = false

[tools.delete_file]
enabled = false

[tools.str_replace]
enabled = false

[tools.mkdir]
enabled = false

[tools.move]
enabled = false
```

这只关闭上述配置对应的入口。完整只读工作区还需分别限制记忆/身份写入、`copy`、可写 exec、外部 MCP 及文件系统权限。通用文件工具获准后可以编辑 MEMORY 等工作文件，不受 `tools.memory_write.enabled` 代为限制。普通实例内不提供逐路径、逐用户 ACL；用户隔离使用[独立工作区与容器](gateway.md)。后台渠道和定时任务的工具白名单仍不接纳这些文件工具，本批不扩大后台授权。

## 路径与文件类型

路径相对于管理员配置的工作区，不展开 `~` 或环境变量。禁止绝对路径、`..` 和空文件路径，最多 1024 UTF-8 字节、64 个路径组件；`list_dir`、`grep`、`glob` 缺省目录为 `.`。工作区根及其上级目录必须由管理员控制。

实现逐级打开工作区内父目录，再通过所持目录句柄操作叶子，不在检查后重新用宿主绝对路径打开文件。父目录符号链接被拒绝；文件读写、替换和删除拒绝符号链接、硬链接、目录、FIFO、socket 等非常规目标。写入可创建缺失的普通父目录。目录列表的目标路径也不得经过链接；列表中的符号链接显示为 `type: "symlink"`，硬链接或特殊文件显示为 `"unsupported"`，均不会被跟随或读取正文。

这些约束不是宿主 OS 沙箱。管理员不得把工作区根或父目录的移动/替换权交给不可信进程；不遵守锁的编辑器也不参与后述串行化。跨网络文件系统、设备故障和恶意宿主进程需独立部署验证。

## 参数、输出与资源上限

KiB 为 1024 字节。返回的 `path` 是工作区相对路径，目录条目名称相对于所列目录。

| 工具 | 参数与行为 | 成功 JSON |
|---|---|---|
| `read_file` | 必填 `path`；`offset` 为从 1 开始的行号，缺省 1；可选 `limit` 行数。先有界读取全文件，再取行范围；文件超过 256 KiB 时拒绝，不能靠小行范围绕过。拒绝无效 UTF-8 或 NUL 文本 | `path, offset, limit?, total_lines, returned_lines, truncated, size_bytes, content` |
| `write_file` | 必填 `path, content`；`mode` 为 `overwrite`（默认）或 `append`。最终文件最多 256 KiB，追加计入已有字节且不自动插入换行；覆盖可以把原大文件替换为合法小文件 | `path, mode, bytes_written`，字节数是最终文件大小 |
| `str_replace` | 必填 `path, old_str, new_str`；`old_str` 非空，`new_str` 可空。默认要求恰好一个匹配，`replace_all=true` 替换全部非重叠匹配；零匹配始终报错。输入和结果均最多 256 KiB，增长溢出在构造结果前拒绝 | `path, replacements, replace_all, bytes_written` |
| `delete_file` | 必填 `path`；只删除一个常规文件，不递归，缺失时报错；不读取文件正文，不以 256 KiB 限制被删文件大小 | `path, deleted, size_bytes` |
| `list_dir` | `path` 默认 `.`，`recursive` 默认 false；`max_entries` 默认 200、最多 1000 | `path, recursive, truncated, entries`，条目含 `name, type, size?` |

`read_file.truncated` 表示行范围没有包含全文，不表示超大文件被静默截断。读取所得字节和大小仅属于本次打开文件的观测，不保证与同时发生的外部编辑形成一致快照。

`list_dir` 最多扫描 2000 个条目、32 层子目录，并在遍历检查点执行 2 秒预算；达到深度边界的目录仍可能列出，但不会继续遍历。最终 pretty JSON 连同路径、外层字段、转义和缩进不得超过 64 KiB。达到条目、扫描、深度、路径长度、输出或时间预算时设置 `truncated=true`；截断结果不能当作完整清单。已收集条目按名称排序，但扫描被截断时不保证是全目录字典序的前 N 项。非 UTF-8 文件名无法无损表达时明确报错；遍历期间目录消失或访问失败也可返回错误。2 秒是合作检查预算，不能中止卡住的内核文件系统调用。

上述九个主工具、四个兼容名称与记忆工具的异步入口共用全进程 8 个阻塞 I/O 许可，忙时立即返回错误，没有无限等待队列。取消调用方等待不会提前释放仍在工作的许可，也不会强制终止阻塞任务。原生模型工具参数 JSON 另受 16 KiB 上限、工具结果另受 256 KiB 上限；因此文件总量可通过小次追加达到边界，读取大文本时应请求合适行范围。工具结果编码超限不表示操作未发生，见[原生工具合同](native-tools.md)。

## grep / glob 扫描合同

`grep` 按字面量子串搜索 UTF-8 文本，不执行正则或 shell；`glob` 只返回匹配的常规文件路径，不读取正文。两者的 `path` 都可指定一个工作区相对文件或目录，缺省为 `.`，目录递归搜索。参数 schema 与 JSON 字段保持现有合同：

| 工具 | 参数与默认值 | 成功 JSON |
|---|---|---|
| `grep` | `pattern` 必填、非空、最多 512 字节；可选 `glob` 过滤最多 128 字节；`case_insensitive=false`；`max_matches` 默认 50、最多 200 | `pattern, path, glob?, case_insensitive, max_matches, truncated, match_count, matches`；匹配含 `path, line, snippet` |
| `glob` | `pattern` 必填、非空、最多 256 字节；`max_results` 默认 100、最多 500 | `pattern, path, max_results, truncated, match_count, matches`；匹配为工作区相对路径 |

`grep` 的行号从 1 开始，每条 snippet 最多取 200 个 Unicode 标量，超出时附省略号。大小写不敏感模式使用 Unicode 小写后比较，不保证语言相关的完整字符等价。glob 的 `*` / `?` 匹配单段，`**` 匹配跨目录；无 `/` 的模式只匹配文件名。多段 `**` 采用多项式动态规划，避免重复递归产生指数分支；这不是完整的 shell glob 语法。

两者通过工作区目录句柄逐级访问。显式路径经过符号链接父目录，或目标是符号链接、硬链接、FIFO 等特殊文件时，直接拒绝；目录扫描跳过这些条目，不跟随它们。扫描跳过 `.git`，但该目录条目仍消耗扫描额度。非 UTF-8 文件名、条目访问失败、扫描时文件消失或变成链接而打开失败，会使本次搜索明确报错，不再把这些 I/O 故障静默当作未匹配；正常扫描中明确识别的链接/特殊文件及 grep 的二进制/超大文件按前述规则跳过。它们不取得 mutation 锁，也不声称搜索期间外部编辑形成一致快照。

| 预算 | 行为 |
|---|---|
| 扫描条目 | 单次最多 2000 项，包含目录、链接、`.git`、特殊文件和未命中 glob 过滤的条目；不是只统计已打开的普通文件 |
| 路径与深度 | 路径最多 1024 UTF-8 字节、64 个组件；最多遍历 32 层子目录，不能借助很深的空目录树绕过计数 |
| 合作时间预算 | 2 秒，在遍历、读取块和逐行匹配处检查；不能强行终止挂起的内核文件系统调用 |
| grep 单文件 | 最多 256 KiB；目录扫描跳过二进制、无效 UTF-8/NUL 和超大文件，显式指定此类文件时返回错误 |
| grep 正文读取总量 | 单次实际读取最多 16 MiB，含后来被判定为二进制的字节及读取中增长的文件字节；过滤后未打开的文件不消耗正文额度；剩余额度不足以确认读完整个文件时停止，不搜索半份正文 |
| glob 正文 | 不读取，因此可以返回超大或二进制普通文件路径；路径与条目预算仍生效 |
| 输出 | 最终 pretty JSON 连同原始查询/路径、结构、转义和缩进最多 64 KiB；不把 JSON 截成无法解析的字节片段 |

搜索因结果数、扫描、深度、路径、字节、输出或合作时间预算提前结束时返回 `truncated=true`。grep 在恰好达到 `max_matches` 时也保守标记截断；glob 若完整扫描且恰好达到 `max_results`，可以保持 false。该标记说明结果完整性受限，不证明未处理部分一定还有匹配。grep 保留目录深度优先顺序，每层只对已收集的候选排序；glob 最多保留 500 个候选，再按完整相对路径排序并应用请求条数和输出预算。在扫描中止时，glob 也不保证是整个目录的字典序前 N 项；两工具的空结果或少量结果都不能当作完整无匹配结论。收窄 `path` 或过滤条件后可发起新的显式查询，应用不会自动遍历剩余树。

## mkdir / move 原子变更合同

`mkdir` 与 `move` 使用工作区目录句柄，和记忆写入、`write_file`、`str_replace`、`delete_file` 共用工作区 inode 协作锁。路径同样最多 1024 UTF-8 字节、64 个组件；检查时拒绝符号链接父目录、显式叶子的链接或特殊文件。`mkdir` 接受普通目录，`move` 接受单链接普通文件或普通目录。两者不读取文件正文，move 不遍历源目录内容，因此不套用读写文本的 256 KiB 上限。

| 工具 | 参数与行为 | 成功 JSON |
|---|---|---|
| `mkdir` | 必填 `path`；`recursive` / `parents` 为别名，默认 true，同时给出必须一致。普通目录已存在则幂等成功；`path="."` 同样保持幂等。false 时父目录必须已存在 | `path, created, existed, recursive` |
| `move` | `from` / `source` 与 `to` / `destination` 两组参数别名，每组至少提供一个，同时提供必须一致；原生工具 schema 接受四种组合。`overwrite=false` 默认不覆盖，目标父目录必须已存在 | `from, to, overwrite, overwritten, kind`，kind 为 `file` 或 `dir` |

mkdir 递归创建时，每次建立普通父目录后同步其父目录。它不是整个路径的事务：后续目录创建或同步失败，已创建的目录不会自动删除。返回 `created=false, existed=true` 只表示本次检查到已存在的普通目录，不是新写入的收据。

move 当前仅支持 Linux/macOS，其他平台明确拒绝；只执行一次、相对于源和目标目录句柄的同卷原子 rename。Linux 的默认不覆盖使用 `renameat2(RENAME_NOREPLACE)`，macOS 使用独占 rename；底层文件系统或平台不支持该语义时明确报错，不退回“先检查目标不存在，再普通 rename”的竞争窗口。工作区根、同一 inode，以及将目录移入自身子路径的请求被拒绝；move 不创建任何缺失父目录。

`overwrite=true` 也不会先删除目标。仅允许文件替换文件，或目录替换空目录；目标非空目录、类型不符和权限错误均报错，失败时不预先清空目的地。非空源目录可以整体 rename，内部链接随目录条目一起移动，不遍历、不跟随，也不表示这些链接通过了读取授权。成功后同步源和目标父目录。

**跨卷行为已收紧**：删除旧的文件/空目录 copy 后 delete 回退。跨文件系统返回 EXDEV 或其他不支持错误时保留失败语义，没有自动复制、删除或重放。需要跨卷迁移时，由管理员采用独立迁移和核对流程，不能把 move 当作两个卷之间的原子事务。

`overwritten` 表示持有协作锁时检查到目的地已存在；它不提供对非协作编辑器的快照保证。父目录句柄不会跟随后来同路径替换的链接，但 rename/unlink 操作的是那个父目录中的当前名称，不具有对叶子 inode 的 compare-and-swap 保证。非协作进程在最终检查之后更换叶子时，不承诺捕获每一种竞争；单次 rename 不会跟随该叶子的符号链接去操作链接目标。工作区根及其祖先仍必须由管理员控制。

mkdir/move 的同步错误、超时或取消可能发生在变更已经提交之后。阻塞任务保留 I/O 许可及协作锁直到实际结束；不会因 HTTP/模型等待取消而回滚。停止新的写入、核对目录或移动的源/目标两端后，再决定下一步；不按失败文本盲目重新执行。部署前提见[工作区目录与移动](deployment.md#工作区目录与移动)。

## 提交、并发和恢复

Linux/macOS 上，`write_file`、`str_replace`、`delete_file`、`mkdir`、`move` 与记忆写入共用工作区目录 inode 的非阻塞独占锁，覆盖整个读取、修改及提交过程。锁忙立即报错，不自动重试；它只串行化使用该锁的协作写入者，不约束外部编辑器、`copy` 或不使用该锁的外部工具。

写入和替换在目标目录排他创建随机暂存文件，Unix 权限 0600；同步文件后原子 rename，再同步父目录。覆盖不保留原权限、时间戳等元数据。追加同样发布一个完整新文件，旧固定 `<name>.tmp` 不会复用。超出文件内容边界时保留原目标；多级父目录创建不构成整体事务，失败后可能留下已创建目录。删除通过父目录句柄 unlink 目标条目后同步该目录。

返回同步错误、请求超时、调用方取消或进程中断，不能证明写入/删除没有发生。已受理的阻塞 mutation 可能在请求取消后提交；应用没有自动回滚，也没有跨模型轮次的持久文件操作收据。先核对实际文件，再决定是否再次追加、替换或删除。进程异常结束可能遗留 `.jiaclaw-memory-*` 暂存文件；停机核对后清理，不自动把它们视为已提交内容。生产备份应在停止写入后取得一致快照。

## 验收状态

上一批 PR #76 的最终本机二进制 `tests/workspace_files.py` 四组通过：主名称/别名 schema 与 JSON、真实增改查删、精确 256 KiB 追加与超限保留、链接/特殊文件和缺失父目录外逸拒绝、四个配置开关同时关闭八个名称，以及伪造别名时整批零执行。该进程测试的文件增长场景是在读取前由外部增长；读取过程中最多消耗 limit+1 字节由 Rust 测试单独验证。

Rust 层另覆盖最终 pretty JSON 的 64 KiB 预算（含转义文件名）、2000 条扫描、32 层深度、遍历预算过期、共享锁/并发追加和取消后的 I/O 许可。全量 854 项 Rust 通过（library 343、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI；fmt、Clippy correctness/suspicious 和锁定构建通过，既有 style/pedantic warnings 保留。最终二进制的 `native_tools.py`、`memory_io.py` 四组与 `e2e.py` 回归全部通过。

PR #76 最终 head `277a89a5ade1e4ab84d7c17d696c004b7fd1e7ea` 已通过 [CI 37103576477](https://github.com/jiawenyao401/JiaClaw/actions/runs/37103576477) 的 Linux/macOS 与真实容器检查。上述证据属于上一批五工具与别名修复。

上一批 PR #77 的 grep/glob 最终本机全量 Rust 864 项通过（library 353、core 122、host 389），1 项真实 Docker 专项本地 ignored 由 CI 执行；fmt、Clippy correctness/suspicious、锁定构建与 diff-check 通过，保留既有 style/pedantic warnings。真实二进制 `tests/file_search.py` 六组通过，覆盖匹配/权限、文件类型、扫描与读取预算、输出及多 `**` 模式；`workspace_files.py` 四组、`native_tools.py`、`memory_io.py` 四组和 `e2e.py` 回归全部通过。

Rust 单独验证精确条目/深度/期限计数、metadata 检查后文件增长时最多读取 limit+1 字节并计入总预算、父目录改名后保留目录能力、叶子替换拒绝和动态规划匹配。非 UTF-8 文件名专项在 Linux 执行，APFS 不允许构造该测试名称，不能把本机通过当作该专项通过。PR #77 最终 head `bc7ef30467ad8c436585eeee4b1cfc99d16ef68f` 已通过 [CI 37105552392](https://github.com/jiawenyao401/JiaClaw/actions/runs/37105552392)，含 Linux/macOS、Chromium 与真实容器；详见[验证记录](validation.md)。该批只使用本机模型协议、一次性凭据和临时文件，没有真实供应商请求，不以协议 fixture 代替断电硬件持久性认证。

本轮 mkdir/move 最终本机 875 项 Rust 通过（library 364、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI；fmt、Clippy correctness/suspicious 与锁定 host 构建通过，保留既有 style/pedantic warnings。`memory_io` 定向 24 项通过，含 10 项新增本机 mutation 测试。最终生产二进制 `tests/workspace_mutations.py` 六组、`workspace_files.py` 四组、`file_search.py` 六组、`native_tools.py` 和 `memory_io.py` 四组全部通过。

本机 macOS 证据包含真实内核原子不覆盖竞争，以及 EXDEV / ENOSYS / EACCES 注入后不回退且保留两端；没有把它记为真实跨卷实测。Linux 另设 `/dev/shm` 与临时目录不同设备的真实 EXDEV 专项，CI 缺少该前提会失败，非 CI 仅允许显式跳过。Linux/macOS、Chromium 与真实容器以本轮 draft PR 最终 head CI 为准，不沿用 PR #77 结果。完整证据见[验证记录](validation.md)。
