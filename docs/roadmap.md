# JiaClaw 里程碑与验收

2026-10-11 继续[主流Agent差距](reviews/2026-10-11-mainstream-agent-gap-review.md)，接入[已声明技能参考资源按需读取](skill-resources.md)：默认关闭独立开关，正文版本和声明资源原始字节双hash核验，实际worker在配置工作区能力内读取，原白名单/只读/共享I/O所有权不扩大。真实140/256字节路径、声明撤销/磁盘改版、父/叶链接/硬链接/FIFO、UTF-8/原始和JSON预算均有进程证据；初版生产路径P2和两项夹具P2已分别修正，独立[Standards](reviews/2026-10-11-skill-resources-standards.md)/[Spec](reviews/2026-10-11-skill-resources-spec.md)聚焦开放0。新批固定head仍需46必需Python、七Chromium、四优化候选的独立官方验收，不借父或本地资格。来源锁定、安装/更新/撤销、经验记忆、真实供应商/浏览器、durable与租户范围继续开放。

父批最新状态：PR110 head `5ace6d9` 已独立七作业/四候选合格，结果固定在PR正文/pin，不再自引文档触发CI。PR111旧head `5fcdf2b` 的Ubuntu在活跃日志UTF-8尾部读取失败；启动和最终扫描均经真实片段RED→GREEN修正，最终严格扫描等待退出并包含shutdown日志。新head `4bdeac5` 的CI `38075103858` /候选 `38075103866` 正独立验收，不能借旧head三个候选。下方各批的“待验收”保留为当时交付快照。

2026-10-11 接入[模型按需技能正文读取](skill-read.md)：显式开关默认关闭；启用后目录仅含摘要与正文 SHA-256，模型在原请求白名单内读取指定的已加载版本，不因关键词提前注入正文。名称不是路径，热加载替换/移除后拒绝旧版本，读取与版本核验共享短锁，正文和 JSON 转义后的输出分别有界，失败为本地只读 no_effect。显式 enabled_skills 与缺省配置保持兼容，技能不能授予工具权限，租户范围未扩大。冻结 PR110 二进制的缺少工具负例、最终源码1238 Rust（另1忽略）及本批真实进程验收分开保存；[Standards](reviews/2026-10-11-skill-read-standards.md)/[Spec](reviews/2026-10-11-skill-read-spec.md)聚焦开放均0，历史累计审查继承。新固定 head 的45必需Python、七Chromium和四优化候选官方资格独立待验收；父PR110资格不代替本批。参考资源、来源治理、安装生命周期、经验记忆、真实供应商与浏览器仍分别开放。

2026-10-11 根据[主流 Agent 差距研究](reviews/2026-10-11-mainstream-agent-gap-review.md)继续技能生态的可落地前置工作：冻结旧二进制两次真实复现工作区外技能链接读取与 FIFO 阻塞，现有发现改用[有界技能读取合同](skills.md)。完整目录/原始文本/元数据/重复名称边界、认证严格重载和每注册表同步/异步容量已实际接线；等待取消不释放实际 worker 的许可。独立审查初版发现普通空目录兼容 P2 与全局槽造成并行测试干扰 P2，均经真实 CLI/并行 Rust 复现修正，初版报告保留。最终源码 `0291084` 的1235 Rust（另1忽略）、必需 Clippy/fmt、原始四场景复现、真实技能 CLI/HTTP 套件、七项并行技能测试和四套既有进程回归通过；[Standards](reviews/2026-10-11-bounded-skills-standards.md)/[Spec](reviews/2026-10-11-bounded-skills-spec.md)聚焦开放均0，继承累计审查不声称穷尽重审。新 draft PR 的固定 head 官方 CI/四优化候选独立待验收，不借 PR109资格。按需原生技能工具、来源/版本治理、经验记忆和浏览器仍开放，本批不标记完整技能生态。

PR109 最终 `f5319902` 的七实际作业、两平台各43必需Python、七Chromium及四优化候选已独立通过，最终证据保存在 PR正文和 `/tmp/jiaclaw-oct10-tool-failure-delivery/pin.json`，没有为自引head另交纯文档触发CI；下方“待核对/仍需”记录是之前的批次状态。上游本轮 StateKnot `aa11b4f`/唯一alpha.1/#140未变；Brokerrouter连接器404由本机已授权gh身份补充核对，main仍`e01ecb9`，无发布，#31/#41开放且#40仍为未合并草稿；#49在两种入口仍不可见，只记实际404。

PR109 初次固定 head `fd4f98f` 的 Ubuntu/macOS 官方 CI 均在旧 `workspace_files` 拒绝断言失败；冻结最终二进制也复现三套文件测试仍期待失败后 `completed`/二次模型请求的接线遗漏。本轮按既有 unknown 停止合同修正三套 fixture，拒绝输入拆成逐个明确请求以保留全部边界，共享 wire 断言关闭新发现的 Standards P3 重复启发式，生产实现未变。完整43套实际进程命令已通过；共享版本三套随后重验通过，其余40套fixture字节未变，原矩阵与追加证据分开保留。新增 [Standards](reviews/2026-10-10-tool-failure-fixtures-standards.md) / [Spec](reviews/2026-10-10-tool-failure-fixtures-spec.md) 独立增量审查开放均0，继承范围与初版分别记录。旧 head 的局部候选通过不代替修正后 head 的七官方作业、43 Python、七 Chromium 和四优化候选资格；全部资格仍按固定 head 在PR正文/pin核对。

旧 Intel 优化候选另因40分钟作业上限取消，官方 annotation 与单测20分50秒/优化编译16分49秒证据已保存；仅 Intel 矩阵项增加到60分钟，其他三项维持40分钟，所有验收命令和应用/fixture截止不变。新版本需完整通过该候选后才能计资格，取消不能冒充修复后成功。

本轮已独立核对 PR107/108 最终固定 head 的全部七实际作业、每平台41必需Python、七Chromium和四优化候选各13安装/归档/源码检查；最终证据仅回填各PR正文/pin，未为自引head提交文档。随后冻结 PR108 二进制真实复现命令写入后超时仍继续派发：下一模型轮重复写入两次并报告完成。本批接线[失败工具效果边界](tool-failure-effects.md)，未知错误/超时停止后续批次和模型请求，受信本地纯错误保留反馈，custom/MCP/HTTP/semantic默认unknown；独立 Spec 初版另发现超大失败错误被标成完成并丢效果标记的 P2，真实 native-loop 负例复现后保留失败来源与 marker 修正；最终 [Standards](reviews/2026-10-10-tool-failure-effects-standards.md) / [Spec](reviews/2026-10-10-tool-failure-effects-spec.md) 当前开放均0，初版报告独立保留。本机最终源码1231 Rust通过（另1忽略），必需Clippy与fmt/parser通过；最终二进制的四组失败矩阵共24实际用例、文件/Docker成功结果丢失控制独立验收。初版源码12套相关进程验收与最终修正源码证据分别记录。本批新draft PR固定head的43必需Python/七Chromium/四优化候选资格另计，不能借用PR108。通用外部效果确定性、durable恢复、独立钉钉/WhatsApp、多模态及真实供应商/安装继续开放。

2026-10-10 PR #105 固定 head `ac680464` 的[五渠道准入最终资格](validation.md#pr-105-五渠道准入最终-ci-回填)已独立核对七官方作业、40必需Python/七浏览器与四实际优化归档/安装/源码证明，Standards渠道事务P2重复启发式关闭；租户预览UI的PR104资格保持独立。本轮接线[私有状态文件Module](private-state-files.md)，模型账本和语义索引仅共享叶子检查/安全打开，原目录、锁、schema及恢复分别保留。本批1227 Rust与16套实际进程/三Chromium/parser全部通过，独立Standards/Spec新增未解决均0，历史私有文件P3重复启发式关闭。原渠道P2保持关闭。PR #106 源码 head `f0f12ec8` 的七官方作业/四优化候选也已[独立通过](validation.md#pr-106-私有状态文件最终-ci-回填)；后续文档回填提交仍按自身 head 验收。

本批修复 doctor 的真实首用假成功：缺少模型 key、无效 provider endpoint、缺失/无效 MCP bearer 均以非零退出，stub 只在显式配置时报告。默认检查离线且不打开私有存储；显式 `--connect` 验证实际 MCP discovery 和启用存储，但不执行模型、embedding 或远程工具调用。新增冻结 CLI 真实进程覆盖与 Standards/Spec 双轴增量审查；完整批次固定 head 的 CI 与四候选资格单独核对。

后续[诊断能力说明](doctor.md#capability-status-and-session-storage)修正同一输出中的矛盾：无效端点不再显示成功，显式 stub 不要求 Key，HTTP MCP 接线、SQLite/内存会话配置和未接线的 StateKnot durable 分别报告。`--connect` 仍不打开会话库，诊断不声称验证了该库可用性。冻结旧二进制真实负例与修正后的 CLI/e2e 证据分别保留；本批 fmt、锁定构建及必需 Clippy 通过，独立 [Standards](reviews/2026-10-10-doctor-capability-standards.md) / [Spec](reviews/2026-10-10-doctor-capability-spec.md) 新增开放均0。本批固定 head 的官方资格独立核对，不借用父 PR 的结果。

PR #103 固定 head `0e1d28fe` 的七官方作业/四候选已[最终回填](validation.md#pr-103-会话目录最终-ci-回填)；首次Mac Discord503根因未确认，同head重跑成功与根因修复保持区别。PR104初版200/final GET工具选择缺口经真实负例复现，最终与done共享receipt选择核验后关闭，初版失败证据独立保留。

2026-10-10 会话目录批次先完成 PR #102 的七官方作业与四候选实际日志核对，见[最终回填](validation.md#pr-102-执行结果丢失修复最终-ci-回填)。随后按审查修复[目录资源边界](session-catalog.md)：Memory/SQLite 存储侧摘要、最多50条游标页、网关只读查询接线、TTL不触达与SQL分批清理，工作台只保留当前页。新 head 的本地/双轴/官方资格分开记录；租户预览 UI 随后推进，已有 SSE API 资格不代替 UI。

2026-10-09 已核对组织迁移、PR #58–#91 当前 head CI 与两框架固定合同，见[迁移与最终验收](organization-migration.md)。StateKnot #155 的未发布 Core 输出 Schema/fuzz 变更不改变当前 alpha.1 HTTP MCP 字节。现有回访按用户“恢复”指令保持启用，仍在每批完成后 30 分钟继续；下文旧暂停记录属于当时状态。此表以交付能力为准，不以文档里的设计或存根作为“完成”。

| 顺序 | 能力 | 状态 | 完成标准 / 当前证据 |
|---|---|---|---|
| 1 | copy | 已迁移共享 I/O 与写锁；PR #89 三项最终 CI 通过 | [复制合同](workspace-files.md)：十二个主文件工具共用八槽；copy/file_copy 共用目录句柄、1024字节/64组件路径与单链接普通文件检查，最多实际读取64 MiB+1拦截源增长；原子不覆盖/覆盖和四种参数组合。历史见[验证记录](validation.md#复制共享-io-与写锁批次)，最终三作业成功见[回填](organization-migration.md#上一批-copy-最终验收) |
| 1a | stat / tree | 实现并本机验收 | [只读元数据与目录树](workspace-files.md#stat--tree-元数据与目录树合同)：独立配置开关、目录句柄、叶子链接不跟随、严格参数及完整路径/扫描/输出预算；本机 884 项 Rust、新工具六组与七套既有进程回归通过，PR #79 最终 head 的 Linux/macOS 与真实容器 CI 已通过 |
| 2 | 受控 exec | 实现并真实 Docker 验收 | 默认禁用、白名单、固定镜像、非 root/无网络、超时/输出限制、清理；SIGKILL 边界见配置说明 |
| 3 | SQLite 会话 | 实现并进程级验收；本批修复取消时的存储所有权 | 创建/对话/删除持久化、一次性 JSON 迁移、独占锁、并发串行、SIGKILL 后恢复；[取消合同](session-cancellation.md)覆盖排队、写锁争用、导入/删除和失败后释放 |
| 4 | MCP 客户端 | HTTP 只读工具已接线并协议/整机 fixture 验收 | 精确 StateKnot 版本、工具白名单/描述 pin、离线 schema、鉴权/有界调用/取消；本批补齐进程四槽 Schema worker 与原始总期限，超时后容量归实际工作持有，固定提交 CI 见本批交付；[使用边界](mcp.md)。外部服务器独立认证，写入需 durable；stdio 上游 [#140](https://github.com/StateKnot/StateKnot/issues/140) |
| 5 | Web 工作台 | standalone 流式/目录已通过 PR #97；个人 Key JSON 工作台已通过 PR #99；租户预览 UI 已通过 PR #104 固定head独立官方资格 | 内置同源静态资源；管理员发件箱复用现有授权和持久 outbox，支持分页、详情、未知核对与整来源取消；[权限和恢复边界](web-outbox.md)。无模型 HTML 执行、无浏览器持久密钥；[Web 流式](web-streaming.md)显式工具授权、原编号/取消/刷新核对与有界预览，固定 head 资格按各批交付记录 |
| 5a | Brokerrouter 原生工具往返 | 实现；按本批 fixture 验收 | 原生 tools/tool_calls/role:tool、调用 ID 关联、整批权限/参数预检、正文不执行、有限调用预算；[合同与验收方法](native-tools.md)。真实供应商 #31 与 durable #41 仍开放 |
| 6 | cron 多任务 | 实现；含 Telegram/Slack/Discord/飞书/企业微信/钉钉定时通知 | SQLite jobs/runs、鉴权增删查与暂停/恢复、明确时区/DST、原子领取/完成、配额与中断暂停；[运行边界](scheduler.md)。[定时通知](scheduled-delivery.md) 与运行/会话原子提交、目的地单独授权，无副作用自动重放 |
| 7 | 渠道统一出站 | Telegram/Slack/Discord/飞书/企业微信/钉钉已实现；企业微信启动门槛已通过 PR #86 CI | 持久 inbox 去重、授权白名单、统一有界发送、共享 outbox/回执、429 冷却、未知结果人工核对；[合同](channels.md)。定时 Telegram/Slack/Discord/飞书/企业微信/钉钉已接入；[飞书](feishu.md)限企业自建单租户文本，[企业微信](wecom.md)限专用自建应用与精确成员文本，本批实际 serve 在 Agent/MCP/数据库/监听/worker 前校验 token、AgentID、启用状态和三类白名单的显式人员可见范围，新增六组/32 负例及无凭据停发维护通过，PR #86 最终三项 CI 已通过；[钉钉](dingtalk.md)限内部应用机器人、获准成员私聊；[Discord](discord.md) Bot 定时文字已接线；WhatsApp 及真实安装认证仍待完成 |
| 8 | StateKnot durable + 委派 | 待认证/接线 | 原生输出合同缺口 [Brokerrouter #41](https://github.com/StateKnot/Brokerrouter/issues/41)；admission/driver/store、子任务身份、预算/并发/取消、恢复语义及上游生产门槛 |
| 9 | 模型路由与降级 | 按任务来源选逻辑模型已接线；网关端点降级待联合认证 | 管理员配置聊天/渠道/定时/心跳/摘要模型与有界输出策略，每轮工具循环固定选择；[路由合同](model-routing.md)。端点降级归 Brokerrouter，应用不在未知结果后换模型或重发；真实供应商 #31 仍开放 |
| 9a | 模型调用收据 | 已接线并通过 PR #72 跨平台 CI | 显式 Brokerrouter 私有账本，持久提交身份/收据、未知 hold、已知远端 UUID 的 GET 核对与管理员解除；[恢复边界](model-calls.md)。不恢复工具循环、会话或 StateKnot durable turn |
| 10 | 多用户与 Key 管理 | 独立用户聊天/会话入口已实现；完整里程碑未完成 | 一用户一容器/工作区/数据库/私有网络/限额卷、哈希 Key/只读权限、受限代理与持久 hold；[部署范围](gateway.md)。用户任务、[Telegram](tenant-telegram.md)、只读 Key、管理员审计、[Slack](tenant-slack.md)、[Discord](tenant-discord.md)、[飞书](tenant-feishu.md)已分别通过 PR #71/#80/#81/#82/#83/#84/#85 最终 CI。[独立用户企业微信](tenant-wecom.md) 已通过 PR #87 固定 head 的本地十组、旧21套与三项最终 CI；独立用户钉钉、WhatsApp 及真实供应商联合认证待完成；standalone 企业微信启动门槛的 PR #86 CI 不代替新私有队列/后端验收 |
| 11 | 真正流式 | CLI/持久 HTTP/Web 已分别验收；租户 SSE API 已通过 PR #101；租户预览 UI 已通过 PR #104，真实上游认证待完成 | [CLI 实际逐事件合同](cli-streaming.md)与[四平台回填](validation.md#单次-cli-真流式最终-ci-回填)：原始身份与模式/收据、整批工具权限、有限队列、取消后结算及重启 hold；上游 [PR #40](https://github.com/StateKnot/Brokerrouter/pull/40) 资源修复尚未合并，原 `/api/chat` 兼容分块仍非 token streaming；租户 API 的[最终回填](validation.md#pr-101-租户-sse-api-最终资格回填)不代替预览 UI 的独立资格与供应商认证 |
| 12 | 语义记忆 | 显式 Brokerrouter/SQLite 已接线并本地验收；真实模型质量待认证 | [语义记忆](semantic-memory.md)：来源/空间版本、私有索引、精确余弦、源哈希新鲜度、持久未知 hold、GET 核对与显式 CLI 刷新/重建；fixture 与真实模型质量验收分别记录 |
| 13 | 多模态 | 待实现/上游认证 | 文本契约之外新增受限 media 输入/输出，大小/格式/权限校验，使用网关媒体任务契约与认证供应商 |
| 14 | 打包发布 | PR #91 固定 head 四平台候选验收已通过；公开发布待审核 | [候选发布合同](release-candidates.md)：PR 与 tag 复用优化构建/真实归档安装、原生架构与完整字节比对、失败回滚、源码与资产摘要；打包去除宿主扩展属性/AppleDouble。head `4edc04c868b07096a4ebe367d5b0083988822d72` 的七项 checks 全部成功，四份实际归档/源码证据见候选合同；公开资产、ABI/真实供应商及恢复资格仍独立审核 |
| 15 | 文档与 E2E | 本批覆盖；持续扩充 | 实际配置/备份部署、模型 fixture、SQLite 崩溃、容器和安装；真实渠道及供应商仍需独立联调 |

MCP 之后的功能依赖 durable 身份、授权或 outbox 的应先补底层契约，避免在当前内存执行循环上承诺恢复能力。MCP stdio 与 Brokerrouter 真实工具默认分别跟踪上游 #140 / #31；不复制框架内部实现绕过未通过的生产门槛。

本批修改改善单机应用的可运行性与安全边界；完整个人 Agent 生产认证仍未完成。最新固定版本、检查证据和障碍见 [StateKnot](stateknot-gaps.md) 和 [Brokerrouter](brokerrouter-gaps.md)。

当前 standalone 企业微信实际启动身份门槛已通过 PR #86 最终跨平台 CI；本批实际接入独立用户企业微信永久 registry/后端 owner、私有队列、原请求身份、持久额度与停机核对，最终冻结二进制十组与1158项Rust、旧21套及 PR #87 的三项最终CI均已通过。飞书已完成最终 head 的整机、跨平台和容器范围验收，真实 Telegram/Slack/Discord/飞书/企业微信安装、TLS 延迟与终端收发仍分别认证。持续复核 StateKnot #140、Brokerrouter #31/#41 与 PR #40 的固定合同；源码 JWT/JWKS 和 typed Schema 方向能力另计固定版本接线，不代替当前租户隔离或 HTTP MCP 升级理由。钉钉当前限内部机器人 HTTP 私聊/定时文本，真实安装、字段稳定性及限额仍须认证。WhatsApp 须先核实 Cloud API 通用 AI 资格、主体/地区、客户服务窗口、模板授权与未知投递语义，不能外推 3P Agents 条款。外部写工具、子 Agent 和运行恢复仍须满足 durable/治理身份；用户 hold 不表示能恢复或重放工具执行。

本批先收紧既有 MEMORY / SOUL / USER / HEARTBEAT 文件边界：配置路径接线、有界读取、统一写入上限、目录句柄约束、原子发布、协作追加锁和初始化保留；新增整机 fixture 与 747 项 Rust 回归已在本机通过，跨平台/真实容器以本批 draft PR 最终 head CI 为准。这是语义记忆前置修复，不能据此将 embeddings 或向量检索标记完成。

语义记忆本批仅在显式启用时接入 Brokerrouter embeddings 与私有 SQLite。源变更先拒绝查询计费，未知提交保留 hold；维护 CLI 要求同库服务停机，不新增公开管理路由。最终二进制的 9 组离线整机验收、773 项 Rust 回归、fmt/Clippy/锁定构建以及 e2e/native_tools/memory_io/model_routing 已在本机通过。该批 PR #70 的 head `dae57af27c77f553e6344f5391611b35df454bfe` 已通过 [CI 37082943062](https://github.com/StateKnot/JiaClaw/actions/runs/37082943062)，含 Linux/macOS 与真实容器；合成向量不代表真实模型检索质量认证。

独立用户定时任务已接线并完成本地进程与浏览器验收：以网关 enabled/hold 和共享执行容量准入，后端停止自主 tick；任务及结果保存在每租户数据库，工作台提供受能力探测控制的管理入口。最终二进制双租户 4 组及 Chromium 验收通过；PR #71 的最终 head `949aebb` 已通过 [CI 37089235842](https://github.com/StateKnot/JiaClaw/actions/runs/37089235842)，包含 Linux/macOS 与真实容器；见[范围及生产边界](tenant-cron.md)。

模型调用收据批次只补模型请求的身份、收据和人工核对入口；最终二进制 7 组整机验收与 815 项 Rust 测试已在本机通过，PR #72 最终 head `a841b026` 已通过 [CI 37093371689](https://github.com/StateKnot/JiaClaw/actions/runs/37093371689) 的 Linux/macOS 与容器验收；不能据此将真正流式、工具运行恢复或多模态标记完成。媒体上游已支持文生视频作业和受控 MP4 交付，但 JiaClaw 尚缺受信 turn、独立审批/审阅及下载身份链，不能将 `output_pending` 当作可交付视频；固定合同见[Brokerrouter 状态](brokerrouter-gaps.md)。

Discord Bot 定时文字补齐独立目的地/guild 授权、每次发送前验证、持久冷却、401 凭据阻断和安装范围 unknown 核对；PR #73 最终 head `b99e96690b6ec2ec0265fe68d2895a135cb9e8d5` 已通过 [CI 37095933677](https://github.com/StateKnot/JiaClaw/actions/runs/37095933677)，含 Linux/macOS、Chromium 与真实容器。真实 Discord 安装认证仍独立。

本批补齐 [Web 发件箱审计](web-outbox.md)：单实例管理员通过既有 status 接口探测能力，渠道停用后仍可查看历史；独立用户网关不开放渠道权限。单条详情与人工核对保留服务端状态竞争、未知结果和整来源取消边界，浏览器超时不等于服务端停止。最终二进制的真实 Chromium 发件箱验收、旧工作台回归及 834 项 Rust 测试通过；fmt/Clippy/锁定构建通过。PR #74 最终 head `e22965dd9dc2488c9433d0a93f7b9e1bf59304a5` 已通过 [CI 37098122253](https://github.com/StateKnot/JiaClaw/actions/runs/37098122253)，含 Linux/macOS、Chromium 与真实容器。不将该 UI 扩展记为多用户渠道授权或自动恢复。

本批为 [standalone 定时任务工作台](standalone-scheduler.md)补齐管理员能力探测、有限分页与完整授权展示，并用 UUIDv4 create-only PUT/SQLite 收据解决响应丢失后的创建身份核对。schema 10 保留 purge 后创建 tombstone，不自动遗忘旧 ID；网关继续禁止该 PUT。845 项 Rust、fmt/Clippy/Node 语法检查和锁定构建通过；最终二进制的三套 Chromium 验收及同批 schema 10 后端两套进程回归通过。PR #75 最终 head `0c9900a75a8f5a3a1c980c8ba98b18c3430b12d2` 已通过 [CI 37101387934](https://github.com/StateKnot/JiaClaw/actions/runs/37101387934)，含 Linux/macOS、Chromium 与真实容器；不扩大 cron 的执行恢复保证。

本批优先修复既有[工作区文件权限与 I/O](workspace-files.md)：四个兼容名称遵守对应配置开关，五个主工具共用目录句柄、有界数据、协作 mutation 锁与受控阻塞容量；兼容名称返回统一 JSON。最终本机 854 项 Rust、fmt/Clippy/锁定构建通过；真实二进制新增四组文件验收及 native_tools、memory_io、e2e 回归通过。PR #76 最终 head `277a89a5ade1e4ab84d7c17d696c004b7fd1e7ea` 已通过 [CI 37103576477](https://github.com/StateKnot/JiaClaw/actions/runs/37103576477)，含 Linux/macOS 和真实容器；该批范围不扩大到 `grep` / `glob` / `mkdir` / `move`，不改变 `copy` 的独立合同，也不将可选 `stat` / `tree` 标记完成。

本轮继续收紧 [grep/glob](workspace-files.md#grep--glob-扫描合同)：目录句柄访问、所有目录条目计数、深度与合作时间限制、grep 累计实际读取及完整 JSON 预算，并用多项式匹配消除 `**` 指数递归。两工具与其余五个文件工具共用八个阻塞 I/O 许可；授权、后台工具范围和写入恢复保证均不扩大。最终本机 864 项 Rust、fmt/Clippy/锁定构建通过；新搜索六组及 workspace_files、native_tools、memory_io、e2e 真实进程回归通过。PR #77 最终 head `bc7ef30467ad8c436585eeee4b1cfc99d16ef68f` 已通过 [CI 37105552392](https://github.com/StateKnot/JiaClaw/actions/runs/37105552392)，含 Linux/macOS、Chromium 与真实容器；该批未迁移 `mkdir` / `move`，也未新增可选 `stat` / `tree`。

本轮迁移 [mkdir/move](workspace-files.md#mkdir--move-原子变更合同)：九个主文件工具共用八槽阻塞 I/O，目录变更与记忆写入共用协作锁。move 使用 descriptor-relative 同卷原子 rename，默认原子不覆盖、覆盖时不预删目标，移除跨卷 copy/delete 回退；mkdir 逐级同步但不回滚先前创建的目录。最终本机 875 项 Rust、fmt/Clippy/锁定构建通过；新变更六组与 workspace_files、file_search、native_tools、memory_io 真实进程回归通过。PR #78 最终 head `9690692bbb208fe5bebac3eab69a73540e335c16` 已通过 [CI 37107782116](https://github.com/StateKnot/JiaClaw/actions/runs/37107782116)，含 Linux/macOS、Chromium、真实容器及 Linux 真实 EXDEV 专项；跨卷实测不计入本机 macOS 证据。不声称对非协作编辑器的叶子 inode CAS、自动回滚或 durable 恢复；该批没有新增 stat/tree。

本轮接入 [stat/tree](workspace-files.md#stat--tree-元数据与目录树合同)，与既有九个主文件工具共享八槽只读/写入 I/O 容量，但元数据查询和目录树不取 mutation 锁。新增严格参数、叶子类型最小元数据、可调整深度和完整 JSON 预算；list_dir 同步按完整工作区相对路径收紧限制，保留既有排序和深度语义。持久渠道及 cron/interval 白名单不扩大；管理员启用的独立 HEARTBEAT 与兼容 `/hooks/inbound` 依既有全部已注册工具策略使用配置中启用的工具。最终本机 884 项 Rust、fmt/Clippy/锁定构建通过；最终二进制的新工具六组通过；本批七套既有进程回归也均通过、退出码 0。PR #79 最终 head `768cfba3041c7c863e14cbdcc17d6b13ef6470d5` 已通过 [CI 37110081396](https://github.com/StateKnot/JiaClaw/actions/runs/37110081396)，含 Linux/macOS、Chromium 与真实容器；这些证据不构成 OS 沙箱或 durable 运行认证。

本轮接入默认关闭的[独立用户 Telegram 私聊](tenant-telegram.md)：registry 固定 Bot/人/专属后端、后台准入复用用户 hold 与共享容量；每绑定私有 inbox/outbox 和操作关联，固定 channel 路由及 clock/json 工具。后端会话与网关队列不能跨库原子提交，未知结果须离线核对而不重放；网关队列盘仍为共享有限额卷。本机 918 项 Rust、fmt/Clippy/锁定构建通过；最终二进制新整机七组及本批七套既有进程回归通过。PR #80 最终 head `68b3a22867e65ed32154c4fc2292066da6f842b4` 已通过 [CI 37113957145](https://github.com/StateKnot/JiaClaw/actions/runs/37113957145)，含 Linux/macOS、Chromium 与真实容器；不标记完整多用户渠道或真实安装认证。

本轮增加管理员签发的[只读 API Key](gateway.md#只读-key)：权限保存在 registry schema 3 并在轮换时继承，旧 Key 迁移保持完整权限；仅允许既有本用户 GET，不执行模型或用户内容修改。它不会脱敏历史，不改变 cron/Telegram 的独立授权，也不承诺 GET 触发的后端维护零写入。最终二进制双后端五组及真实 Chromium 只读验收通过；928 项 Rust、fmt/Clippy/锁定构建和现有三套工作台与三套用户后台进程回归通过。PR #81 最终 head `00d013c205e9c92b6649b8738d9d7d39bca966e5` 已通过 [CI 37715609778](https://github.com/StateKnot/JiaClaw/actions/runs/37715609778)，包含 Ubuntu、macOS、Chromium 与真实限额卷/私网容器只读权限组。

本轮增加[可信管理员按用户审计查询](gateway.md#按用户查询管理审计)：同一读快照、有界页、精确字符串游标和全局保留水位，默认不提取私密 notes；不开放公共路由、不改变 schema 3 或用户权限。仅提供当前保留历史，恢复旧备份与长期归档仍需管理员核对。初次提交本机 936 项 Rust、双后端真实进程四组及四套用户回归通过；诊断修订后全量 Rust 串行 937 项及 fmt/Clippy/锁定构建通过，最终二进制新审计与四套用户回归共五套均通过。本机默认并行曾触发旧 fixture 的短截止时间失败，最终 CI 仍以默认并行验证。首轮固定提交的真实容器审计及 ENOSPC 已通过；macOS 的旧 semantic 测试同步竞争已修正，Ubuntu 的旧 Telegram hold 结算等待失败原因仍未确定；定向 SQL 故障注入已验证脱敏诊断和保守 hold，不据此声称该 CI 原因已解决。PR #82 最终 head `32c5830cc1f6caf008b39149f884bc6b464aac24` 已通过 [CI 37726979505](https://github.com/StateKnot/JiaClaw/actions/runs/37726979505) 的 Ubuntu/macOS/container，见[验证记录](validation.md#ci-暴露问题与诊断)。

Slack 批次基于 PR #82 已验证 head，交付默认关闭的[独立用户 Slack](tenant-slack.md)：专用 App、固定工作区/成员/DM、四次平台身份握手、原始签名和 2.8 秒 ACK、私有 inbox/outbox、共享用户 hold 与 UUIDv7 操作账本；后端请求记录与会话结果同事务提交，未知结果不自动重发。独立复核后补齐 owner 暂存事务与原子不覆盖发布；继承的 SQLite WAL-reset 风险通过精确升级 bundled 3.51.3 移除。修订后 984 项 Rust、fmt/Clippy/锁定构建、同一最终二进制新八组及十五套旧进程回归通过。PR #83 最终 head `af9265ccc5e321ab98e0916f0deac2164b2780b3` 已通过 [CI 37735649553](https://github.com/StateKnot/JiaClaw/actions/runs/37735649553) 的 Ubuntu/macOS/container；真实 Slack 安装与容器内 Slack runtime 压力认证仍另计。人工清 hold 已检查全部保留 Telegram/Slack 队列。

2026-10-08 Discord 批次基于上述固定 head，接入默认关闭的[独立用户 Bot DM 命令](tenant-discord.md)：完整永久身份、签名持久准入、ephemeral original/followup、原密钥加密队列、protocol 3 元数据核对与撤销后无 Secret 的停机检查。私有库 21 项、本机默认并行 1042 项 Rust、新整机九组与旧进程十六套全部通过，最后重建二进制哈希保持一致；fmt/必需 Clippy/locked 全目标检查及构建通过。PR #84 最终 head `768da0eeff2219ec69d3ca36cd12fbcd8943e927` 已通过 [CI 37746206382](https://github.com/StateKnot/JiaClaw/actions/runs/37746206382) 的 Ubuntu/macOS/container。首轮 Mac 因整作业 20 分钟预算被取消，修订为 30 分钟后完整通过；没有修改生产/fixture 截止时间或省略测试。模型后到期提交保留人工 hold；真实平台、Discord 容器 runtime 压力和供应商认证仍未完成。上游 StateKnot main `9110ad71934e446d9fbb8ff21387cf14a7b7bdc6` 的源码级 JWT/JWKS profile 未发布到 alpha.1；Brokerrouter 原生 Schema 缺口和 StateKnot stdio 议题未变化，应用入站/队列接线不等于 durable driver。

飞书批次基于 PR #84 已验证 head，扩展 registry schema 6 与后端 protocol 4：固定 App/tenant/Bot/人/p2p Chat，900 ms 本地回调预算、共享准入、原请求收据、私有队列及撤销后的无 Secret 停机检查。官方 Bot Info 顶层响应和单聊缺省群字段已逐项核对，权限限于私聊接收、Bot 发送、Chat 与企业读取。review 后收紧原始 JSON、只读打开前 SQLite 文件头、完整 immutable owner schema、16 KiB 私有 prompt 和已准入结算的有界 I/O 等待；本机默认并行 1097 项 Rust、fmt、必需 Clippy 及 locked 全目标 check/build 已通过。首版同一生产二进制的新整机 11 组与旧进程 21 套通过；Brokerrouter fixture 等待与 channels deadline 次数断言仅在测试内修订，未确定首轮调度根因。

PR #85 首轮 CI 的 Ubuntu/容器成功，macOS 第 11 组千条准入意外收到 429 后失败且后续 skipped；原断言没记录 body.status，原因仍未知。第一次 fixture 修订增加 SQL 锁/停机排空、明确 `429 busy` 的有界同签名重试及精确 1000/1001 身份断言，本地 11 组一次通过且生产二进制哈希不变。第二轮 Ubuntu/容器仍成功，macOS 写锁用例收到 `503 ingress_deadline`（908 ms）而未满足测试只要求 admission_failed 的断言；整体回调截止不能等同单次 SQL 250 ms 截止，具体调度段未确定，也不解释第一次 429。

最终仅把 fixture 写锁结果限定为 admission_failed 或 ingress_deadline 两种精确 503，持锁直到进程退出/排空、核对完整持久状态和零新增效果后再释放并重启核对；其他 503 仍失败。每次 1 秒、900 ms/SQL 250 ms、精确 1000/1001 和 busy 重试边界均保留。本地完整 11 组通过且生产二进制哈希不变；[PR #85](https://github.com/StateKnot/JiaClaw/pull/85) 最终 head `80a8bb03d391cd27bde8f03034aa2c5081d441f4` 已通过 [CI 37767529821](https://github.com/StateKnot/JiaClaw/actions/runs/37767529821) 的 Ubuntu/macOS/container 三项。macOS 实际触发 ingress_deadline、Linux 触发 admission_failed，两者都完成持锁停机、完整持久态不变及精确容量验收；不据此推断首轮 429 或具体调度根因。token 整机仅验正常 mint 的启动取消/截止和已知失效后的 terminal/no resend 核对，运行期正常到期刷新、真实平台和容器渠道 runtime 压力仍待认证；证据分层见[验证记录](validation.md)。

2026-10-08 11:35 UTC 复核：StateKnot main `a312b0c2d09cd6d695b37b8d4163cddb910bdf6a` 十二项 CI 成功，#148 修正 typed Tool input/output Schema 的 Serde 方向并新增 output 类型注册；release 仍为 alpha.1，当前 HTTP MCP 不调用 typed registry，无需为此升级。Brokerrouter main 和 #31/#41/PR #40 不变，main 与 PR 共十二项 FAILURE 全部因账户付款/额度未启动；没有新可消费 durable 原生输出合同或代码测试失败证据。

企业微信启动批次基于 PR #85 上述已验证 head：对入站成员、会话成员和定时目的地的最多 300 个规范成员并集，核对官方 token、匹配且启用的 AgentID 及显式人员可见范围。前置读取采用 30 秒总预算、启动 token/agent 各 5 秒、64 KiB 原始 JSON 与关键字段/MIME 校验，保留已有发送、未知核对和 200 次/24 小时/4 秒额度语义。默认并行 Rust 1105 项及 fmt/必需 Clippy/locked build 通过；同一最终二进制的新 `wecom_startup.py` 六组/32 负例与 WeCom（含无凭据同库维护）、MCP、e2e、channels、scheduled_delivery 五套既有回归全部通过、退出码 0。初次 WAL 观察与 scheduler-disabled DTO 断言只修 fixture，未改生产数据库版本、平台行为或超时；详见[验证记录](validation.md)。后续 12:56 UTC 已核对 [PR #86](https://github.com/StateKnot/JiaClaw/pull/86) 固定 head `db802c46b8726e2dfbaf9defb1eebddb043619c7` 的 [CI 37776140027](https://github.com/StateKnot/JiaClaw/actions/runs/37776140027)，Ubuntu/macOS/container 三项均成功。该证据属于 standalone 启动门槛，独立用户企业微信本批另行验收，真实平台/许可/TLS/客户端/正常 token 到期刷新/容器渠道 runtime 认证仍未完成。

12:56 UTC 新预检：StateKnot main `04567c4db12553025b4d31330693f4958222c30f` 十二项成功，#152 收紧七个空 tagged execution wire 读取，合法 wire/Schema pins 不变，未发布；当前精确 alpha.1 HTTP MCP 不消费该读取器或 typed registry，无需为此升级。Brokerrouter main/#31/#41/PR #40 未变化，十二个失败 annotation 全部为付款/额度未启动，没有新 durable 输出合同或代码失败证据。

新[独立用户企业微信](tenant-wecom.md) 基于 PR #86 固定 head：registry schema7、backend protocol5、专用 CorpID/AgentID/成员与用户终身绑定，官方安装身份校验通过后才打开私库/握手，XML/AES 900 ms 持久准入、原 UUID 元数据和永久16000操作账本、NULL未知额度、撤销后无Secret停机核对已实际接线。准入为每事件预留一次模型与最多16片单尝试发送；空库最多941个未执行消息，历史操作减少名额，purge保留UUID/额度。最终默认并行Rust1158项（373/123/662）及fmt/必需Clippy/locked build通过；冻结二进制SHA256 `ac8da42d9c066b7ea9212c1e32733cde4c9459d7828a5acec444b1e6fcf5f58d` 的真实双后端十组一次全部通过（143.05秒），按历史metadata实测940个新身份后精确queue_full且重复ACK保留。首轮events DTO/维护恢复观察错误只修fixture；中间旧Slack一次EAGAIN的具体原因未确定，目标及全量最终复验通过，没有改生产/fixture期限。提交准备时既有21套进程回归仍独立运行，本批draft PR固定head最终CI待核对，详见[验证记录](validation.md#独立用户企业微信批次)。下一步完成旧回归与固定head CI验收，再按依赖推进独立用户钉钉、WhatsApp准入资格与流式/durable剩余项。

2026-10-08 14:39 UTC 钉钉批次预检：PR #58–#87 各自当前 head CI 成功，均 OPEN draft，无代码 review/thread；PR #87 唯一 CodeRabbit 信息评论说明 draft 不自动审，未将它当成外部审查完成。PR #87 head `ba989c30a75e4e6fe7eaf9e13c0704a90cc7695e` 的 [CI 37787484242](https://github.com/StateKnot/JiaClaw/actions/runs/37787484242) 第一次运行全部成功，macOS1158/Ubuntu1159 Rust、每平台29进程步骤、新WeCom十组与实际940身份 headroom、Linux Chromium/真实Docker和container registry/维护/限额卷均有完整日志证据；容器未启用WeCom收发 runtime，真实企业认证仍开放。StateKnot main `83802cb3202bf9cb860c6357a94abc80408b1f88` 的 #153 仅现有类型测试盘点/fixtures/CI/docs，未改当前消费的原始 HTTP MCP Schema；alpha.1/#140不变。Brokerrouter main/#31/#41/PR#40不变，十二项失败 annotation 均付款/额度未启动。

本轮钉钉已实际加严standalone原始回调/token/send对象与唯一JSON MIME，含糊code+字符串receipt进入unknown，保留单次请求与原UUID/cooldown/无钉钉凭据维护。最终本地1163 Rust、fmt/必需Clippy/locked build、新完整147.87秒真实进程与五套旧回归全通过；[PR #88](https://github.com/StateKnot/JiaClaw/pull/88) 固定 head `4f62ecf2ae471899aa5233e24796a71e57bd4a40` 的三项最终 CI 已通过，详见[最终回填](validation.md#钉钉原始报文最终-ci-回填)。这不记为独立用户钉钉完成：15:39 UTC 已取得正式 CorpID-bound GetToken 授权合同，但尚未接线/真实认证；app/detail 的正式最小权限、独立机器人适用性及凭据→机器人完整证明仍待核实。[安装边界](dingtalk.md#本批协议修复与安装证明的区别)

15:36 UTC 当前两框架固定 main/release/议题未出现可消费的新合同。本批修复实测的应用 copy 缺陷：旧二进制可复制 fixture 所有的外部 hardlink 哨兵，混合参数被 native Schema 错拒；现在注册的两个名称实际进入共享八槽与工作区写锁，保持二进制/64 MiB/原子发布合同，取消后锁和许可由真实阻塞工作持有直到结束。初版已通过本机1169项 Rust和新七组，PR #89首CI暴露旧Telegram同步重开锁失败；已实际fork/dup与旧SessionStore定向重现close-only生命周期缺口，改为SQLite连接先关、所有权guard后显式解锁并覆盖六入口错误路径。最终本机1171项Rust、必需检查、同一新冻结binary的七组copy与17套旧进程回归均通过；原CI具体fork重叠未证明，新head三作业仍按本批最终证据验收。下一步完成此批交付后继续核实钉钉完整安装身份链，以及已有网关流式合同的应用接线与治理身份；HTTP MCP pin 保留，stdio/外部写入、子 Agent/durable、真实供应商、WhatsApp资格与多模态仍开放。没有恢复用户要求保持暂停的自动回访。

数据库所有权修订CI的Linux/容器通过，macOS Rust1171与copy七组通过，但旧飞书第11组跨连接占槽就绪断言失败（其后9个Python跳过）。已把TCP写出观察改为固定Hyper1.11.1的真实100 Continue屏障，原250/650/900ms预算及busy/1000队列/无副作用断言全部保留；同一生产binary的修订后飞书完整11组本地一次PASS（119.517秒），生产源码/依赖未再改。先前17套完整回归及两次CI失败证据保留，最新fixture提交三作业待最终验证；详见[跨连接屏障修订](validation.md#复制批次跨连接验收屏障修订)。剩余合同/真实认证与下一步依上段，不将旧head的成功代替新head验收。


跨连接屏障提交的两平台全部Cargo/30进程测试（含copy七组、飞书11组）均已通过，但macOS整项作业在所有步骤及清理结束时碰到30分钟总预算，官方明确timeout、整体取消。只修CI完整test作业容量为有上限40分钟，全部生产/fixture截止和30强制步骤不改；生产binary/依赖/源码及飞书fixture保持冻结。新提交的三项CI仍须完整通过，不能把旧head的步骤成功当作整体资格；见[作业容量修订](validation.md#复制批次完整-ci-作业容量修订)。剩余能力/真实认证和用户的自动回访暂停约束不变。

2026-10-09 PR #91 补齐四平台优化候选、实际归档安装及源码/资产摘要；首轮 macOS 打包实测的 AppleDouble 已由生产单文件打包移除。普通 CI 暴露的 Discord 跨连接观察改为真实100 Continue，原期限和精确429/408/401保留，修订后三项旧 head CI通过。随后 Linux 候选并发追加和本机 mkdir 的 busy 揭示 close-only 工作区锁生命周期缺口；不可克隆操作 owner guard 现于真正结束时显式解锁，取消 worker 仍持锁到发布。默认并行1171项Rust、fmt及必需Clippy已通过；同一优化二进制的文件/记忆/MCP/E2E及实际归档11套通过，四平台固定 head 仍需独立最终验收，不借用旧 head 成功。剩余 stdio/外部写入、durable委派、生产流式、独立用户钉钉/WhatsApp、真实供应商和多模态仍依上述合同推进。


2026-10-09 本轮回填 PR #93 最终七项 CI、四平台优化候选实际归档安装与真实旧账本迁移。StateKnot main 已推进到 `288cfc634574cc314e748f0ebeaa48ca418435ed`，#156 时间戳解析修复及嵌套 JSON 测试没有改变现有 HTTP MCP 合同；十三项 checks 成功，alpha.1/#140 不变。Brokerrouter main/#31/#41/PR40 未变化，十二项 failure 仍因付款/额度未启动。

HTTP/Web 流式准备发现并实际修复[存储取消竞态](session-cancellation.md)：原聊天提交、导入、删除及拒绝写入的四项回归均复现提前释放 turn 锁，现将所有权移交实际 blocking storage。最终六项边界测试、本地1194 Rust/必需检查、同一冻结 binary 七套相关进程回归通过；本批固定head跨平台/候选CI仍须完整验收。没有增加HTTP/Web真流式或durable恢复能力。下一步在此已序列化存储基础上接通持久HTTP请求身份、有限事件交付/核对、取消/停机与真实浏览器，再分别认证上游资源修复和供应商。现有回访ACTIVE，每批结束后30分钟继续，不合并或公开发布。

2026-10-09 PR #94 最终 head `e3986d6d3bb10532504de4d5d334a7447343cd19` 的七项 CI 全成功；四平台候选在合并 tree `a01a92187fbf1dc670505d2da76c1422e33002d6` 上运行六项真实存储取消边界、CLI 七组及归档安装十三组，详见[最终回填](validation.md#http-session-取消所有权最终-ci-回填)。此前本批“待最终验收”已解决。

本轮继续接线[持久 HTTP 请求与结果](http-turns.md)：显式启用的 UUIDv4 create-only 准入、单活动 owner/四个真实控制 owner、有限总期限、相同会话库内原子结果/历史、取消意图、重启人工核对与永久身份/结果清理均已实际实现。协议使用已资格化 Brokerrouter SSE 原生循环，但对 HTTP 输出 JSON；不将其标为 Web/token delivery 或 StateKnot durable 完成。新增真实进程七组初验通过，固定最终源码的完整 Rust/回归与本批 draft PR CI 仍须验收。下一步在该身份和状态基础上接有界 HTTP SSE/body consumer 与 Web，随后分别认证租户、上游资源修复和供应商；其他剩余 stdio/外部写入、durable 委派、独立用户钉钉/WhatsApp、多模态保持开放。回访继续 ACTIVE，不自动合并或发布。

该 HTTP 初版 head `aa2c02fed9d42f40181722a2579746ccf2d79e1e` 已通过完整本机1203 Rust/32套及七项CI/四候选，详见[验收记录](validation.md#持久-http-请求身份批次)。最后复核修正准入事务中已经存在身份的响应码，保留 created 标志，仅新记录202/旧记录200；需要重新固定最终源码及完整跨平台证据，不能借用上述初版成功。StateKnot main 的 #158/#159 测试证据增量已核对，不改变现有 integrations 消费合同。

再增加两项实际责任回归，旧行为均失败：取消等待者消失后仍须在成功落盘后通知 owner，停机之前占用许可但稍后登记的 owner 也须观察关闭状态。生产修订将通知与真实 control worker、同一 active 登记锁接线，保持原结算/总期限及底层工具资源边界；最终十一项 HTTP 边界与完整固定 head 资格以本批 PR 记录为准。

PR #95最终head `01034bd2a3152869ecdd9f6aa58d62e04656d7c7` 的1205本机Rust/32套、七项CI/四平台候选全部成功，[最终证据](validation.md#持久-http-身份最终-ci-回填)已回填。本轮继续[有界HTTP正文流式](http-streaming.md)：实际共享原身份/原生队列/结算与会话事务，新增七组真实HTTP含TCP慢读及两个Body责任边界初验通过，完整固定head资格以本批PR为准。Web仍使用旧聊天API，下一批接明确工具授权/原session命名空间、内存身份与有界预览/未知原ID核对，不自动重连重发。stdio/外部写入、durable/委派、独立用户钉钉/WhatsApp、真实供应商和多模态继续开放。回访ACTIVE，不自动合并或发布。

2026-10-09 PR #97 Web原请求接线先前head `6469e26a83f247aa186520155dd4e0ca81048968` 最终七CI/四平台优化候选、完整1207 Rust/33进程/五浏览器通过，已[逐项回填](validation.md#web-原请求流式最终-ci-回填)。当前复用该draft分支补齐[认证持久请求目录](http-turns.md#遗失编号时查找持久请求)和浏览器遗失编号恢复，SQLite直接投影状态而不读正文、不重放，关闭准入后仍可读；新增三Rust/四HTTP组/十一浏览器组初验通过，固定最终head完整34套与七CI/四候选需重新核对，不能沿用6469资格。下一项继续按上游原生Schema与资源合同推进durable/stdio；没有新上游合同可用时独立完善渠道/租户验收，不把目录当作恢复模型或外部效果。DingTalk安装权限证据、WhatsApp、多模态、供应商/代理认证仍开放。回访保持ACTIVE/每批结束后30分钟，不自动合并或公开发布。

目录复核发现历史TTL/删除后的永久完成身份会在Web错误hold，新增真实删除/清理的第十二浏览器组先复现再修订历史404与fetch/body退出。最终提交重新固定并重跑全部检查，不把初版2709的候选或本机成功当作最终资格。原收据仍是完成/active权威，缺失历史不恢复会话、不重放。

历史缺失的最后协议复核补上明确404标记，修正204/null混淆；冻结3a2 binary的前端故障fixture先复现错误结束hold，第十二组验证204/500均保持原身份、正常原GET404才解除。最终提交重新资格化，不扩大框架或供应商完成范围。

2026-10-10 已回填 PR #97 最终1072719的完整1210 Rust/34进程/五浏览器、七项CI与四平台优化归档资格，见[目录最终回填](validation.md#web-原请求目录最终-ci-回填)。本轮基于该head补齐[停机HTTP收据维护](http-turns.md#服务停机后的本机维护)：直接打开已存在私密v11/WAL库，四命令共用真实独占锁，不初始化模型或迁移，不擅改running；显式人工放弃与终态结果清理保留原身份/历史/模型hold。新增真实进程六组初验通过，固定最终源码及本批draft PR完整CI仍待验收。stdio/外部写入、StateKnot durable/委派、租户流式、钉钉安装证明、WhatsApp、多模态和真实供应商/代理认证仍分别开放；下一步在不绕过这些门槛的范围继续接线和验收，30分钟回访保持ACTIVE。

2026-10-10 已逐项回填 PR #98 最终c639255的七项CI、35套跨平台进程与四平台优化归档，见[停机维护最终回填](validation.md#停机-http-维护最终-ci-回填)。本轮继续[独立用户持久 HTTP JSON 请求](tenant-http-turns.md)：原 UUID 跨网关/后端/模型账本、个人只读 Key、每用户永久目录、共享写入锁、取消和重启不重放；registry v8 保留原状态并修正跨用户同 UUID 的锁约束。双实际后端六组初验通过，固定源码完整回归及本批draft PR/七CI/四候选仍需资格化。租户 SSE/工作台、stdio/外部写入、StateKnot durable/委派、钉钉安装证明、WhatsApp、多模态和真实供应商仍开放。每批结束后30分钟回访继续ACTIVE，不自动合并或发布。


2026-10-10 本轮复用draft PR #99，先[回填初版364b274资格](validation.md#租户-http-json-初版最终-ci-回填)，再补齐[个人Key JSON工作台](tenant-http-turns.md#个人-key-工作台)：真实旧binary连接失败已复现，严格scope/固定授权、原收据显示、只读目录/刷新找回、30秒原观察期限和身份清理已接线。新十组真实Chromium在最终冻结binary上通过，包括真实后端停机时手机提示/原目录仍可读；最终b023f3f8字节13套相关回归/1218 Rust/fmt/必需Clippy与parser八组也已通过；新七CI/四候选仍待独立资格；不沿用初版成功。两框架最新固定main/release/#140/#31/#41/PR40不变，没有新原生Schema/stdio合同可消费或重复缺陷议题。下一步继续优先核对该合同，随后接通租户有界SSE与工作台；独立钉钉/WhatsApp、durable委派、多模态及真实供应商仍保持开放，30分钟回访ACTIVE。

首版a0f8c2e官方CI出现Docker Hub匿名拉取429和新浏览器第九组等待失败，未标完整完成。当前补实际网关hold结算屏障、首次PUT与GET同一故障视图/诊断并在原生产字节上十组通过；CI镜像来源改为实际验证同摘要的Docker官方公开ECR，全部步骤/截止保持不变。源码和新固定head官方七作业/四候选继续资格化，不用首次部分成功代替。

后续d61c354容器在读取原固定Dockerfile前端的Docker Hub令牌时504，未开始ECR基础镜像或源码构建。已核对Google公开缓存的同一前端manifest内容摘要，并按官方支持的daemon配置接入临时CI runner，保留原配置/driver/Dockerfile固定值和全部隔离验收；新head七项CI/四优化候选继续独立资格化，manifest验证不计镜像或生产供应商认证。

24b331a容器的实际构建/恢复/每租户ENOSPC已通过；Linux十组的新增诊断定位最后一次GET命中原观察预算却显示一般RPC超时的边界。原b023字节上已用真实回执延迟固定复现，当前区分单次RPC与原观察预算到期，撤销过期fresh状态并保留原身份/草稿/占用，原GET核对之前不标完成。新增早期与最后RPC两条验收仍在同十组内，所有15/30/40/240秒截止不变；新冻结二进制完整资格另计。旧Intel12写线程锁测试首次失败及原设置一次完整复验保留，具体根因未证明，不报告框架缺陷或已修复。回访保持ACTIVE，租户SSE/上游durable等剩余项仍开放。

1efef199 Linux旧Slack/前台双准入测试发现0成功，初版没有具体错误；新增诊断与本机200次隔离复现均不改变原竞争者/250ms预算/成功与Held断言，具体根因未证明。后续CI单worker隔离独立Rust用例的夹具IO，各用例内部并发与全部原期限保持不变，固定源码和新验收配置重新认证；不将该配置或重复成功报告为生产缺陷已修复、负载认证或上游阻塞。
## 2026-10-10 停机边界接线

PR #99 最终273004a及官方七作业/四候选完整资格已回填[validation](validation.md#pr-99-工作台最终资格回填)，PR仍draft/未合并。上游StateKnot aa11b4f / alpha.1、Brokerrouter e01ecb9无release与 #140/#31/#41未出现新可消费合同。准备租户SSE时真实慢读连接复现网关SIGTERM被HTTP排空无限阻塞；本批先修复共享停机期限、晚到正文拒绝与实际DB owner保留进程锁，新增实际TCP/重启验收。租户SSE和durable仍待完整接线，stdio/外部写入、多模态/供应商、正式发布等剩余项保持开放；旧并行锁/准入失败根因未认证。

## 2026-10-10 租户 SSE API 接线

PR #100 最终699adf6的1220 Rust、15相关套件、七CI/四优化归档已[回填](validation.md#pr-100-网关停机最终资格回填)。本轮基于该固定head接通[个人Key SSE API](tenant-http-turns.md#个人-key-sse-api)：配对protocol2握手、原UUID/hold、有限实际Body投递槽位、逐帧校验、断线cancel、原GET终态结算和重启不重放。双真实backend六组初验及三owner/parser Rust通过；完整冻结源码、本批draft PR与七CI/四候选另计，不能沿用PR100资格。已有租户JSON工作台继续使用原路径，租户预览UI仍开放。上游contract/release/#140/#31/#41/PR40均不变，stdio/外部写入、StateKnot durable委派、钉钉安装/WhatsApp、多模态、供应商/代理及公开发布仍分别待验收；下一轮先完成本批CI/审查并接线租户预览UI，30分钟回访保持ACTIVE，不自动合并或公开发布。

用户指定 mattpocock-skills 后，本批双轴审查发现原生流式合同被拒绝却被后续成功 GET 清除审核 hold，以及 standalone 文档仍否认租户入口。冻结 22bb 字节的真实后端、网关和协议故障代理两次复现：原模型恰执行一次、已完成且 active=false，但 GET 前 hold 已消失。修订要求原响应有效且原 GET 成功才清审核，实际 idle 容量仍独立释放；身份和工具授权两类真实故障、原 GET-only 重复及有效 200 JSON lookup 反馈环已通过。新第七组纳入同一 mandatory 套件；旧 22bb 本机和候选资格不计修订最终资格，完整新 head 与七 CI、四候选另行认证。规范轴仅记录测试 setup 重复的非阻塞维护启发，不把它作为生产缺陷或擅改已验证生命周期。

## 2026-10-10 审查后的执行结果边界修复

PR #101 最终 d714a831 的1223本机Rust/22相关套件、七官方作业/四优化候选已[回填](validation.md#pr-101-租户-sse-api-最终资格回填)，PR仍draft/未合并。用户指定的[架构、累计代码与业务审查](reviews/2026-10-10-architecture-code-product-review.md)已完成；本轮先修复确认的 P1：已完成工具的结果超过256KiB后，普通入口仍继续同批/下一轮派发。冻结旧binary的真实文件与非root/无网络Docker回归均失败；新binary十组通过，小输出对照仍完成，超限只执行一次、保留记录/历史并立即要求人工核查。当前 schema11、standalone/gateway 和租户 JSON/SSE API/UI 文档已同步；完整Rust/相关回归与本批draft PR最终CI资格另计，不能沿用PR101。

下一步先完成本批CI/审查，再将会话列表改为存储侧摘要与有限分页，明确TTL触达；随后接线租户预览UI。通用工具错误/超时的效果确定性需独立合同和真实证据，不由本次确定结果丢失修复代替。StateKnot aa11b4f/alpha.1、Brokerrouter e01ecb9/无release、#140/#31/#41与未合并PR40无变化；stdio/外部写入、durable/委派、独立钉钉/WhatsApp、多模态/真实供应商与公开发布仍开放。30分钟回访保持ACTIVE，不自动合并或发布。
