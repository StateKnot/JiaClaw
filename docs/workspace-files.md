# 工作区文件工具与权限边界

本页覆盖 `read_file`、`write_file`、`delete_file`、`str_replace`、`list_dir`，以及四个兼容名称。它们共用有界目录句柄 I/O 与协作写入锁。独立的 `copy` 已有自己的原子复制合同；`grep`、`glob`、`mkdir`、`move` 本批未迁移到这里的实现，不能据此推断它们具有相同边界。可选 `stat` / `tree` 仍未新增。

## 配置与兼容名称

| 配置开关 | 主名称 | 兼容名称 |
|---|---|---|
| `tools.read_file.enabled` | `read_file` | `file_read` |
| `tools.write_file.enabled` | `write_file` | `file_write` |
| `tools.delete_file.enabled` | `delete_file` | `file_delete` |
| `tools.list_dir.enabled` | `list_dir` | `file_list` |
| `tools.str_replace.enabled` | `str_replace` | 无 |

这些开关默认 true；关闭一个开关会同时移除对应两个名称，别名不能绕过配置。请求的 `enabled_tools` 仍按确切名称授权：允许 `read_file` 不隐式允许 `file_read`，反之亦然。普通聊天传空数组沿用“允许全部已注册工具”的行为，见[原生工具合同](native-tools.md)。

兼容名称现在直接使用主名称的参数 schema、描述和执行路径，不另行改写参数。已有 `path` / `content` 请求继续可用，别名也可使用主名称的行范围、追加模式等参数。**这是安全相关的兼容变化**：四个别名的成功结果由旧人类可读字符串改为同结构 JSON；缺失的读取、目录或删除目标返回错误，不能再将旧提示文本当作成功。调用方应解析 JSON 与错误状态，不匹配旧展示文字。

关闭文件写入可分别配置：

```toml
[tools.write_file]
enabled = false

[tools.delete_file]
enabled = false

[tools.str_replace]
enabled = false
```

这只关闭表内相应入口。完整只读工作区还需分别限制记忆/身份写入、`copy`、`mkdir`、`move`、可写 exec、外部 MCP 及文件系统权限。通用文件工具获准后可以编辑 MEMORY 等工作文件，不受 `tools.memory_write.enabled` 代为限制。普通实例内不提供逐路径、逐用户 ACL；用户隔离使用[独立工作区与容器](gateway.md)。后台渠道和定时任务的工具白名单仍不接纳这些文件工具，本批不扩大后台授权。

## 路径与文件类型

路径相对于管理员配置的工作区，不展开 `~` 或环境变量。禁止绝对路径、`..` 和空文件路径，最多 1024 UTF-8 字节、64 个路径组件；`list_dir` 缺省目录为 `.`。工作区根及其上级目录必须由管理员控制。

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

异步入口与记忆工具共用全进程 8 个阻塞 I/O 许可，忙时立即返回错误，没有无限等待队列。取消调用方等待不会提前释放仍在工作的许可，也不会强制终止阻塞任务。原生模型工具参数 JSON 另受 16 KiB 上限、工具结果另受 256 KiB 上限；因此文件总量可通过小次追加达到边界，读取大文本时应请求合适行范围。工具结果编码超限不表示操作未发生，见[原生工具合同](native-tools.md)。

## 提交、并发和恢复

Linux/macOS 上，`write_file`、`str_replace`、`delete_file` 与记忆写入共用工作区目录 inode 的非阻塞独占锁，覆盖整个读取、修改及提交过程。锁忙立即报错，不自动重试；它只串行化使用该锁的协作写入者，不约束外部编辑器、`copy` 或尚未迁移的其他工具。

写入和替换在目标目录排他创建随机暂存文件，Unix 权限 0600；同步文件后原子 rename，再同步父目录。覆盖不保留原权限、时间戳等元数据。追加同样发布一个完整新文件，旧固定 `<name>.tmp` 不会复用。超出文件内容边界时保留原目标；多级父目录创建不构成整体事务，失败后可能留下已创建目录。删除通过父目录句柄 unlink 目标条目后同步该目录。

返回同步错误、请求超时、调用方取消或进程中断，不能证明写入/删除没有发生。已受理的阻塞 mutation 可能在请求取消后提交；应用没有自动回滚，也没有跨模型轮次的持久文件操作收据。先核对实际文件，再决定是否再次追加、替换或删除。进程异常结束可能遗留 `.jiaclaw-memory-*` 暂存文件；停机核对后清理，不自动把它们视为已提交内容。生产备份应在停止写入后取得一致快照。

## 验收状态

最终本机二进制的 `tests/workspace_files.py` 四组通过：主名称/别名 schema 与 JSON、真实增改查删、精确 256 KiB 追加与超限保留、链接/特殊文件和缺失父目录外逸拒绝、四个配置开关同时关闭八个名称，以及伪造别名时整批零执行。该进程测试的文件增长场景是在读取前由外部增长；读取过程中最多消耗 limit+1 字节由 Rust 测试单独验证。

Rust 层另覆盖最终 pretty JSON 的 64 KiB 预算（含转义文件名）、2000 条扫描、32 层深度、遍历预算过期、共享锁/并发追加和取消后的 I/O 许可。全量 854 项 Rust 通过（library 343、core 122、host 389），1 项真实 Docker 专项本地 ignored 留待 CI；fmt、Clippy correctness/suspicious 和锁定构建通过，既有 style/pedantic warnings 保留。最终二进制的 `native_tools.py`、`memory_io.py` 四组与 `e2e.py` 回归全部通过。

跨平台与真实容器以本批 draft PR 最终 head CI 为准；详见[验证记录](validation.md)。测试只使用本机模型协议、一次性凭据和临时文件，没有真实模型/平台调用，也不代替断电硬件持久性认证。
