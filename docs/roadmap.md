# JiaClaw 里程碑与验收

2026-10-08 按当前代码与上游仓库重新核对。此表以交付能力为准，不以文档里的设计或存根作为“完成”。

| 顺序 | 能力 | 状态 | 完成标准 / 当前证据 |
|---|---|---|---|
| 1 | copy | 实现并本地验收 | 二进制文件、64 MiB 上限、越界/链接拒绝、原子覆盖与并发不覆盖 |
| 1a | stat / tree | 实现并本机验收 | [只读元数据与目录树](workspace-files.md#stat--tree-元数据与目录树合同)：独立配置开关、目录句柄、叶子链接不跟随、严格参数及完整路径/扫描/输出预算；本机 884 项 Rust、新工具六组与七套既有进程回归通过，PR #79 最终 head 的 Linux/macOS 与真实容器 CI 已通过 |
| 2 | 受控 exec | 实现并真实 Docker 验收 | 默认禁用、白名单、固定镜像、非 root/无网络、超时/输出限制、清理；SIGKILL 边界见配置说明 |
| 3 | SQLite 会话 | 实现并进程级验收 | 创建/对话/删除持久化、一次性 JSON 迁移、独占锁、并发串行、SIGKILL 后恢复 |
| 4 | MCP 客户端 | HTTP 只读工具已接线并协议/整机 fixture 验收 | 精确 StateKnot 版本、工具白名单/描述 pin、离线 schema、鉴权/有界调用/取消；[使用边界](mcp.md)。外部服务器独立认证，写入需 durable；stdio 上游 [#140](https://github.com/StateKnot/StateKnot/issues/140) |
| 5 | Web 工作台 | 聊天/会话、受限任务管理及管理员发件箱已实现并本地验收 | 内置同源静态资源；管理员发件箱复用现有授权和持久 outbox，支持分页、详情、未知核对与整来源取消；[权限和恢复边界](web-outbox.md)。无模型 HTML 执行、无浏览器持久密钥 |
| 5a | Brokerrouter 原生工具往返 | 实现；按本批 fixture 验收 | 原生 tools/tool_calls/role:tool、调用 ID 关联、整批权限/参数预检、正文不执行、有限调用预算；[合同与验收方法](native-tools.md)。真实供应商 #31 与 durable #41 仍开放 |
| 6 | cron 多任务 | 实现；含 Telegram/Slack/Discord/飞书/企业微信/钉钉定时通知 | SQLite jobs/runs、鉴权增删查与暂停/恢复、明确时区/DST、原子领取/完成、配额与中断暂停；[运行边界](scheduler.md)。[定时通知](scheduled-delivery.md) 与运行/会话原子提交、目的地单独授权，无副作用自动重放 |
| 7 | 渠道统一出站 | Telegram/Slack/Discord/飞书/企业微信/钉钉已实现，真实渠道认证与新渠道待完成 | 持久 inbox 去重、授权白名单、统一有界发送、共享 outbox/回执、429 冷却、未知结果人工核对；[合同](channels.md)。定时 Telegram/Slack/Discord/飞书/企业微信/钉钉已接入；[飞书](feishu.md)限企业自建单租户文本，[企业微信](wecom.md)限专用自建应用与精确成员文本；[钉钉](dingtalk.md)限内部应用机器人、获准成员私聊；[Discord](discord.md) Bot 定时文字已接线（单 guild 普通频道，六组本机整机验收通过）；WhatsApp 仍待交付；进程 fixture 不能替代真实安装认证 |
| 8 | StateKnot durable + 委派 | 待认证/接线 | 原生输出合同缺口 [Brokerrouter #41](https://github.com/StateKnot/Brokerrouter/issues/41)；admission/driver/store、子任务身份、预算/并发/取消、恢复语义及上游生产门槛 |
| 9 | 模型路由与降级 | 按任务来源选逻辑模型已接线；网关端点降级待联合认证 | 管理员配置聊天/渠道/定时/心跳/摘要模型与有界输出策略，每轮工具循环固定选择；[路由合同](model-routing.md)。端点降级归 Brokerrouter，应用不在未知结果后换模型或重发；真实供应商 #31 仍开放 |
| 9a | 模型调用收据 | 已接线并通过 PR #72 跨平台 CI | 显式 Brokerrouter 私有账本，持久提交身份/收据、未知 hold、已知远端 UUID 的 GET 核对与管理员解除；[恢复边界](model-calls.md)。不恢复工具循环、会话或 StateKnot durable turn |
| 10 | 多用户与 Key 管理 | 独立用户聊天/会话入口已实现；完整里程碑未完成 | 一用户一容器/工作区/数据库/私有网络/限额卷，独立网关哈希 Key 轮换撤销及只读权限、受限代理、未知写入持久核对；[部署和验收范围](gateway.md)。受限用户定时任务已通过 PR #71 CI；[独立用户 Telegram 私聊](tenant-telegram.md)已通过 PR #80 最终 CI；只读 Key 的服务端权限、迁移、双用户进程与工作台已通过 PR #81 最终 CI；本批可信管理员审计查询已本地验证快照/游标/私密 notes，最终跨平台与真实容器 CI 待核对。其他渠道后台身份及真实供应商联合认证仍待完成 |
| 11 | 真正流式 | 上游已有协议支持；应用待实现，资源修复未验收 | 逐事件输出、tool delta 聚合、断流恢复/结算、取消与背压；上游 [PR #40](https://github.com/StateKnot/Brokerrouter/pull/40) 慢客户端/连接容量修复尚未合并，不能把完成后分块作为 token streaming |
| 12 | 语义记忆 | 显式 Brokerrouter/SQLite 已接线并本地验收；真实模型质量待认证 | [语义记忆](semantic-memory.md)：来源/空间版本、私有索引、精确余弦、源哈希新鲜度、持久未知 hold、GET 核对与显式 CLI 刷新/重建；fixture 与真实模型质量验收分别记录 |
| 13 | 多模态 | 待实现/上游认证 | 文本契约之外新增受限 media 输入/输出，大小/格式/权限校验，使用网关媒体任务契约与认证供应商 |
| 14 | 打包发布 | 文件与工作流已交付，首次公开 Release 待审核 | 四平台二进制/校验和、安装回滚、非 root 镜像验收、版本标签匹配、draft 审核；没有自动发布到公网 |
| 15 | 文档与 E2E | 本批覆盖；持续扩充 | 实际配置/备份部署、模型 fixture、SQLite 崩溃、容器和安装；真实渠道及供应商仍需独立联调 |

MCP 之后的功能依赖 durable 身份、授权或 outbox 的应先补底层契约，避免在当前内存执行循环上承诺恢复能力。MCP stdio 与 Brokerrouter 真实工具默认分别跟踪上游 #140 / #31；不复制框架内部实现绕过未通过的生产门槛。

本批修改改善单机应用的可运行性与安全边界；完整个人 Agent 生产认证仍未完成。最新固定版本、检查证据和障碍见 [StateKnot](stateknot-gaps.md) 和 [Brokerrouter](brokerrouter-gaps.md)。

下一批先复核 StateKnot #140、Brokerrouter #31/#41 与 PR #40 的固定版本变化；durable 合同未满足期间，继续独立可交付的渠道与应用接线。钉钉本批只覆盖 HTTP 模式内部机器人私聊及定时文本通知；回调重投与字段稳定性、真实安装、平台限额和终端收发需独立认证。WhatsApp 接入须先核实当前 Cloud API 通用 AI 服务资格、部署主体/地区、身份、客户服务窗口、模板授权与未知投递语义，不能将独立 3P Agents 条款外推为 Cloud API 许可。模型路由已完成应用策略接线，后续保持网关治理边界，独立用户聊天入口已接线，继续推进多用户后台权限、真实模型语义检索认证或满足合同后的 durable 接入。网关保留未知写入的人工核对状态，不表示能够恢复/重放工具运行。

本批先收紧既有 MEMORY / SOUL / USER / HEARTBEAT 文件边界：配置路径接线、有界读取、统一写入上限、目录句柄约束、原子发布、协作追加锁和初始化保留；新增整机 fixture 与 747 项 Rust 回归已在本机通过，跨平台/真实容器以本批 draft PR 最终 head CI 为准。这是语义记忆前置修复，不能据此将 embeddings 或向量检索标记完成。

语义记忆本批仅在显式启用时接入 Brokerrouter embeddings 与私有 SQLite。源变更先拒绝查询计费，未知提交保留 hold；维护 CLI 要求同库服务停机，不新增公开管理路由。最终二进制的 9 组离线整机验收、773 项 Rust 回归、fmt/Clippy/锁定构建以及 e2e/native_tools/memory_io/model_routing 已在本机通过。该批 PR #70 的 head `dae57af27c77f553e6344f5391611b35df454bfe` 已通过 [CI 37082943062](https://github.com/jiawenyao401/JiaClaw/actions/runs/37082943062)，含 Linux/macOS 与真实容器；合成向量不代表真实模型检索质量认证。

独立用户定时任务已接线并完成本地进程与浏览器验收：以网关 enabled/hold 和共享执行容量准入，后端停止自主 tick；任务及结果保存在每租户数据库，工作台提供受能力探测控制的管理入口。最终二进制双租户 4 组及 Chromium 验收通过；PR #71 的最终 head `949aebb` 已通过 [CI 37089235842](https://github.com/jiawenyao401/JiaClaw/actions/runs/37089235842)，包含 Linux/macOS 与真实容器；见[范围及生产边界](tenant-cron.md)。

模型调用收据批次只补模型请求的身份、收据和人工核对入口；最终二进制 7 组整机验收与 815 项 Rust 测试已在本机通过，PR #72 最终 head `a841b026` 已通过 [CI 37093371689](https://github.com/jiawenyao401/JiaClaw/actions/runs/37093371689) 的 Linux/macOS 与容器验收；不能据此将真正流式、工具运行恢复或多模态标记完成。媒体上游已支持文生视频作业和受控 MP4 交付，但 JiaClaw 尚缺受信 turn、独立审批/审阅及下载身份链，不能将 `output_pending` 当作可交付视频；固定合同见[Brokerrouter 状态](brokerrouter-gaps.md)。

Discord Bot 定时文字补齐独立目的地/guild 授权、每次发送前验证、持久冷却、401 凭据阻断和安装范围 unknown 核对；PR #73 最终 head `b99e96690b6ec2ec0265fe68d2895a135cb9e8d5` 已通过 [CI 37095933677](https://github.com/jiawenyao401/JiaClaw/actions/runs/37095933677)，含 Linux/macOS、Chromium 与真实容器。真实 Discord 安装认证仍独立。

本批补齐 [Web 发件箱审计](web-outbox.md)：单实例管理员通过既有 status 接口探测能力，渠道停用后仍可查看历史；独立用户网关不开放渠道权限。单条详情与人工核对保留服务端状态竞争、未知结果和整来源取消边界，浏览器超时不等于服务端停止。最终二进制的真实 Chromium 发件箱验收、旧工作台回归及 834 项 Rust 测试通过；fmt/Clippy/锁定构建通过。PR #74 最终 head `e22965dd9dc2488c9433d0a93f7b9e1bf59304a5` 已通过 [CI 37098122253](https://github.com/jiawenyao401/JiaClaw/actions/runs/37098122253)，含 Linux/macOS、Chromium 与真实容器。不将该 UI 扩展记为多用户渠道授权或自动恢复。

本批为 [standalone 定时任务工作台](standalone-scheduler.md)补齐管理员能力探测、有限分页与完整授权展示，并用 UUIDv4 create-only PUT/SQLite 收据解决响应丢失后的创建身份核对。schema 10 保留 purge 后创建 tombstone，不自动遗忘旧 ID；网关继续禁止该 PUT。845 项 Rust、fmt/Clippy/Node 语法检查和锁定构建通过；最终二进制的三套 Chromium 验收及同批 schema 10 后端两套进程回归通过。PR #75 最终 head `0c9900a75a8f5a3a1c980c8ba98b18c3430b12d2` 已通过 [CI 37101387934](https://github.com/jiawenyao401/JiaClaw/actions/runs/37101387934)，含 Linux/macOS、Chromium 与真实容器；不扩大 cron 的执行恢复保证。

本批优先修复既有[工作区文件权限与 I/O](workspace-files.md)：四个兼容名称遵守对应配置开关，五个主工具共用目录句柄、有界数据、协作 mutation 锁与受控阻塞容量；兼容名称返回统一 JSON。最终本机 854 项 Rust、fmt/Clippy/锁定构建通过；真实二进制新增四组文件验收及 native_tools、memory_io、e2e 回归通过。PR #76 最终 head `277a89a5ade1e4ab84d7c17d696c004b7fd1e7ea` 已通过 [CI 37103576477](https://github.com/jiawenyao401/JiaClaw/actions/runs/37103576477)，含 Linux/macOS 和真实容器；该批范围不扩大到 `grep` / `glob` / `mkdir` / `move`，不改变 `copy` 的独立合同，也不将可选 `stat` / `tree` 标记完成。

本轮继续收紧 [grep/glob](workspace-files.md#grep--glob-扫描合同)：目录句柄访问、所有目录条目计数、深度与合作时间限制、grep 累计实际读取及完整 JSON 预算，并用多项式匹配消除 `**` 指数递归。两工具与其余五个文件工具共用八个阻塞 I/O 许可；授权、后台工具范围和写入恢复保证均不扩大。最终本机 864 项 Rust、fmt/Clippy/锁定构建通过；新搜索六组及 workspace_files、native_tools、memory_io、e2e 真实进程回归通过。PR #77 最终 head `bc7ef30467ad8c436585eeee4b1cfc99d16ef68f` 已通过 [CI 37105552392](https://github.com/jiawenyao401/JiaClaw/actions/runs/37105552392)，含 Linux/macOS、Chromium 与真实容器；该批未迁移 `mkdir` / `move`，也未新增可选 `stat` / `tree`。

本轮迁移 [mkdir/move](workspace-files.md#mkdir--move-原子变更合同)：九个主文件工具共用八槽阻塞 I/O，目录变更与记忆写入共用协作锁。move 使用 descriptor-relative 同卷原子 rename，默认原子不覆盖、覆盖时不预删目标，移除跨卷 copy/delete 回退；mkdir 逐级同步但不回滚先前创建的目录。最终本机 875 项 Rust、fmt/Clippy/锁定构建通过；新变更六组与 workspace_files、file_search、native_tools、memory_io 真实进程回归通过。PR #78 最终 head `9690692bbb208fe5bebac3eab69a73540e335c16` 已通过 [CI 37107782116](https://github.com/jiawenyao401/JiaClaw/actions/runs/37107782116)，含 Linux/macOS、Chromium、真实容器及 Linux 真实 EXDEV 专项；跨卷实测不计入本机 macOS 证据。不声称对非协作编辑器的叶子 inode CAS、自动回滚或 durable 恢复；该批没有新增 stat/tree。

本轮接入 [stat/tree](workspace-files.md#stat--tree-元数据与目录树合同)，与既有九个主文件工具共享八槽只读/写入 I/O 容量，但元数据查询和目录树不取 mutation 锁。新增严格参数、叶子类型最小元数据、可调整深度和完整 JSON 预算；list_dir 同步按完整工作区相对路径收紧限制，保留既有排序和深度语义。持久渠道及 cron/interval 白名单不扩大；管理员启用的独立 HEARTBEAT 与兼容 `/hooks/inbound` 依既有全部已注册工具策略使用配置中启用的工具。最终本机 884 项 Rust、fmt/Clippy/锁定构建通过；最终二进制的新工具六组通过；本批七套既有进程回归也均通过、退出码 0。PR #79 最终 head `768cfba3041c7c863e14cbdcc17d6b13ef6470d5` 已通过 [CI 37110081396](https://github.com/jiawenyao401/JiaClaw/actions/runs/37110081396)，含 Linux/macOS、Chromium 与真实容器；这些证据不构成 OS 沙箱或 durable 运行认证。

本轮接入默认关闭的[独立用户 Telegram 私聊](tenant-telegram.md)：registry 固定 Bot/人/专属后端、后台准入复用用户 hold 与共享容量；每绑定私有 inbox/outbox 和操作关联，固定 channel 路由及 clock/json 工具。后端会话与网关队列不能跨库原子提交，未知结果须离线核对而不重放；网关队列盘仍为共享有限额卷。本机 918 项 Rust、fmt/Clippy/锁定构建通过；最终二进制新整机七组及本批七套既有进程回归通过。PR #80 最终 head `68b3a22867e65ed32154c4fc2292066da6f842b4` 已通过 [CI 37113957145](https://github.com/jiawenyao401/JiaClaw/actions/runs/37113957145)，含 Linux/macOS、Chromium 与真实容器；不标记完整多用户渠道或真实安装认证。

本轮增加管理员签发的[只读 API Key](gateway.md#只读-key)：权限保存在 registry schema 3 并在轮换时继承，旧 Key 迁移保持完整权限；仅允许既有本用户 GET，不执行模型或用户内容修改。它不会脱敏历史，不改变 cron/Telegram 的独立授权，也不承诺 GET 触发的后端维护零写入。最终二进制双后端五组及真实 Chromium 只读验收通过；928 项 Rust、fmt/Clippy/锁定构建和现有三套工作台与三套用户后台进程回归通过。PR #81 最终 head `00d013c205e9c92b6649b8738d9d7d39bca966e5` 已通过 [CI 37715609778](https://github.com/jiawenyao401/JiaClaw/actions/runs/37715609778)，包含 Ubuntu、macOS、Chromium 与真实限额卷/私网容器只读权限组。

本轮增加[可信管理员按用户审计查询](gateway.md#按用户查询管理审计)：同一读快照、有界页、精确字符串游标和全局保留水位，默认不提取私密 notes；不开放公共路由、不改变 schema 3 或用户权限。仅提供当前保留历史，恢复旧备份与长期归档仍需管理员核对。本机 936 项 Rust、fmt/Clippy/锁定构建及双后端真实进程四组通过；既有网关/只读 Key/用户定时任务/用户 Telegram 四套回归通过。最终跨平台与新增真实容器审计组 CI 仍待核对。
