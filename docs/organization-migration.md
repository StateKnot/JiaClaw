# StateKnot 组织迁移与安装

2026-10-09 已通过 GitHub 官方 API 确认仓库归属为 [StateKnot/JiaClaw](https://github.com/StateKnot/JiaClaw)。原提交、PR 和 Actions run 身份保留；附着的 PR #58–#89 全部 OPEN/draft，各自当前 head 的最新 CI 均成功。

## 现有检出与产品接线

现有工作区和未提交改动无需重建。核对 origin 后只改地址：

```sh
git remote -v
git remote set-url origin https://github.com/StateKnot/JiaClaw.git
git remote -v
```

当前聊天的本地 origin 已更新，没有显式 push URL。若其他检出另有 push URL，须独立核对并更新，上述命令不会删除它。[GitHub 迁移说明](https://docs.github.com/en/repositories/creating-and-managing-repositories/transferring-a-repository)说明旧地址会重定向，但旧位置重新出现仓库会取消重定向。

源码安装、固定版本 Release 下载、三个 Cargo 包继承的 repository/homepage 和文档证据链接现已直接使用新地址。CLI `version` 与三个 Discord 出站请求路径从 `CARGO_PKG_REPOSITORY` 取得产品地址。现有 30 分钟回访保持 ACTIVE，保存的目标改为新地址，没有新增重复任务；用户“恢复”指令是当前状态，旧文档的暂停记录属于当时状态。

Release draft 工作流已经使用运行时 `github.repository`，无需硬编码新组织。容器仍从源码本地构建，没有公开镜像或 GHCR 名称迁移。没有更改组织权限、Secret、账单、分支保护或发布状态。官方 API 当前返回默认分支 main、已有 CI workflow active、默认 Actions token 权限 read、releases 列表为空。Pages API 返回 404，仅表示当前身份未能读取，不能证明没有 Pages。

## 安装与验收

目前没有公开 Release，使用 README 的源码安装步骤。固定版本安装脚本需要维护者批准并公开对应资产之后才能用于正式下载。本批修复“校验和正确但二进制版本错误仍替换旧安装”的缺口：可运行检查后，第一行版本必须与明确指定的 Release 一致，失败保留旧安装。CI 和四平台 Release 构建均实际归档并安装该平台候选二进制，校验新组织下载路径、校验和、原子替换，以及版本不匹配、错误版本参数、缺失校验文件和损坏归档时保留旧二进制。下载由本地夹具提供，没有下载或公开真实 Release。HTTP sender 单元测试实际读取 Discord 请求头；本批二进制的 `version` 也须显示新地址。

本批的本机结果和固定提交 CI 记录在交付 PR 正文与 checks 中；组织策略下的新 PR 必须实际执行 CI，旧提交成功不代替本批验收。没有触发 tag、合并 PR 或公开发布；这些证据不构成四平台公开资产、真实供应商、渠道、流式资源或 StateKnot durable 认证。

本机 macOS arm64 已通过 74 项现有 outbound HTTP 单元测试、fmt、必需 Clippy 和 locked build。同一冻结二进制 SHA256 `2851942a166929f2dbcfa27c39c6ecea856e27413b5f395779bed1e76b5feac8` 的真实归档安装、E2E/SQLite 与 Discord 定时六组均通过。另对旧 head `e95a396` 的安装脚本仅替换下载 URL 以复用本地夹具，实际重现匹配校验和的错误版本会覆盖旧安装；新脚本则拒绝且原字节不变。该定向重现没有替换旧脚本的版本行为；该批四平台正式 Release build 未触发，后续候选验收单列于[发布记录](release-candidates.md)。

[PR #90](https://github.com/StateKnot/JiaClaw/pull/90) 最终 head `08bff050ef3d7d1c77b2b95fe91880318de65cb2` 的 [CI 37876111419](https://github.com/StateKnot/JiaClaw/actions/runs/37876111419) 三项成功。实际 merge `4fd7aafc5e18eda37504029d707c7f5b4c468c9c` 的 tree `706f43e32144decb060a0a8ae7fb2bbbae350f8a` 与该 head 相同。Ubuntu 1172/macOS 1171 项 Rust、两平台全部30套 Python（实际候选安装含有效但版本错误的保留旧字节）、Ubuntu 四套 Chromium 与额外真实 Docker，以及独立 container 非 root/只读根/私网/限额卷/ENOSPC 全部通过。container 未运行渠道 runtime。03:16 UTC 再读取仍 OPEN/draft、成功且无代码 review/inline comment；不称外部审查完成。

## 上一批 copy 最终验收

[PR #89](https://github.com/StateKnot/JiaClaw/pull/89) 最终 head `e95a396ab75f4fcd2e1093405dd12377a9d0aca1`、tree `e96e4b6830c04192b63d9f3f2b2fc70d6786cd90` 的 [CI 37819362928](https://github.com/StateKnot/JiaClaw/actions/runs/37819362928) 三项均成功。三作业实际 checkout merge `4f005a60ec23be322d34e0e54ad0e6349189e43b` 的 parents 为该 head 与 base `4f62ecf2ae471899aa5233e24796a71e57bd4a40`，tree 与 head 相同。

| 作业 | 实际验收 | 完整日志 SHA256 |
|---|---|---|
| macOS `113456114788` | 成功，25m30s；1171 Rust、fmt/必需 Clippy/locked build、全部 30 Python 步骤，copy 七组及飞书十一组 | `b8b11a24c682f121ed6c67e48f4f6d872e5cb16e795b0e8d32a104bd4eee5139` |
| Ubuntu `113456114510` | 成功，25m16s；1172 Rust、相同 30 Python 步骤、四套 Chromium、额外真实 Docker 沙箱与清理 | `a998809caceac82e34396a2a231d17269a93190bbd7d95cb89b29bcd80fe29c9` |
| container `113456114630` | 成功，6m29s；非 root、只读根、私网/限额卷、registry 与默认关闭渠道的维护/ENOSPC 隔离 | `e2b30c507334847fe7f9c487392dc953296701739b2858eed3427877f6306bdf` |

两平台飞书实际 `100 Continue` 屏障分别为 0.421/0.577 ms；第二连接精确 429，队列 busy 无新增匹配或副作用，并验证释放及精确 1000/1001 身份。copy 六项实际文件/I/O 单元测试、两个新所有权测试及原 Telegram 重启测试通过。container 没有运行 copy fixture、copy 压力或渠道 runtime，不能从启动成功外推。

先前 `2921421` 的 Telegram 锁失败、`92a2d6d` 的旧飞书就绪断言失败及 `469ba6a` 全步骤成功但整作业 30m 超时取消，仍保留各自正式结论。40 分钟作业上限没有改变任何生产或 fixture 期限；最终成功不证明首轮具体 fork 重叠或调度根因。2026-10-09 再读取同一 PR/run，仍 OPEN/draft、SUCCESS；代码 review 与 inline comment 均为空，不能称外部代码审查已完成。

## 上游与剩余工作

组织迁移批次固定合同：StateKnot main `83802cb3202bf9cb860c6357a94abc80408b1f88`、alpha.1、#140；Brokerrouter main `e01ecb94919d992eb0b74b3db00d70742820b4cc`、无 release、#31/#41 和 PR #40 head `7a7afea0244828851118ba32d1cf37d906a3f388`。当时 StateKnot 十二项主 CI 与三个 Dependabot 检查成功；Brokerrouter main/PR 的十二项既有失败检查身份未变，保留先前付款/额度导致未启动的分类。后续 StateKnot #155 的实际 main 增量及十四项成功检查见[最新状态](stateknot-gaps.md)，没有升级当前 MCP pin。

精确 HTTP MCP pin 保留，不重复提交相同 issue。下一批继续核实钉钉凭据到机器人的完整安装身份链，以及流式实际接线的治理和资源合同；stdio、外部写入、durable/子 Agent、WhatsApp、真实供应商、语义检索质量及多模态仍分别开放。组织归属不替代任何框架运行身份或认证。
