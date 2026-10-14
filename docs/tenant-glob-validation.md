# 个人 Key 文件名任务：固定源码验收

本批源 `0b223d1e4e937c502794f866a3b4b5a33719ff0a` / tree `71a634c4fea5c06ad958d6ec62b10891aec5a119`，fresh 基准本地 PR148 delivery `dce13153deac8833754b50531a0ce4e7fa084072`，19个 source 路径。远端父 PR148 `416ddca5af74adf180c768a547b38c4c3a0337bd` 与该本地提交身份不同但完整树相同，未覆盖父ref。累计 origin/main 固定 `4b2357fcf01f47ba08d7724edbba7accfb60972c`、PR148及更早资格与历史 runtime 只继承，不称 fresh 穷尽累计重审。

完整121个编译输入由原集合加新 tenant_glob.rs 导出，source/current逐字节一致。manifest SHA256 `5a4300ae1447a1ad36ba522219cdb52551891de7862514d9d081c6034c75a104`；本批冻结 source 与 target binary SHA256 `95fb9a69a0342237e8dc216f44e4fe80ac9e2afcb1c66c4600ee0b600bf5f694`。实际本机1325Rust（477/123/725）通过，另1项真实Docker专项 ignored；locked host build、fmt check、Clippy correctness/suspicious均退出0，保留211项非必需host-test warnings。

十二个相关明确进程命令各退出0：新 tenant_glob --browser、原 tenant_tree --browser、file_search、filesystem_info、http_turns、tenant_http_turns、read_only_keys、tenant_file_read、tenant_file_write、tenant_skill_read、tenant_memory_search和原turn_stream parser。两次实际 Chromium invocation 分别为新 glob 与原 tree，使用 Playwright1.62.1 / 现有 custom Chromium153.0.8010.12(1243)，不是缺失的default1234分发；parser保留原13组并新增第14组。没有本机重跑全70 ordinary Python/42Chromium/优化打包安装/Docker或其它全历史套。

新六API组用真实临时 native/gateway/model/SQLite、Alice/Bob各原2modelPOST/一次原glob结果，结果完整permission/result SHA、3个普通文件匹配（隐藏项、子目录文字文件和二进制名称），零正文或链接目标。真实symlink/hardlink/FIFO/目录替换文件、原截断、exact256模式与258多字节拒绝、整份raw先验、schema拒绝、readonly/cross-user/撤Key、cap变更先于identity/hold、显式取消未来dispatch均覆盖。一个真实Chromium invocation验证默认unchecked、原SSE/GET、mobile375、readonly旧事实、身份清理、真实迟到Crypto及损坏摘要拒绝，storage/pageerrors均0。普通 standalone 原glob和tree回归独立通过。

独立 Matt [Standards](reviews/2026-10-15-tenant-glob-standards.md) 与 [Spec](reviews/2026-10-15-tenant-glob-spec.md) 各0。Standards自己普通六组，Spec自己34完整cap字段/type变异、相邻目录根、raw边界与5NoEffect/9schema负例、根替换文件、双端撤权后原4GET+2旧PUT零新模型调用，以及自己的实际Chromium11非法descriptor/晚Alice Crypto切Bob，全部通过。两轴自行导出121输入并绑定固定binary；这些独立执行不是root矩阵重执行。

初脚本HTTP注释锚点计数拒绝与常量可见性误拼导致未编译，纠正后最终完整验收；sandbox首次431pass/46既有OS-loopback拒绝/1ignored和授权完整1325分别保留。初新夹具将257字符schema拒绝误期望为NoEffect，实际原preflight needs_review/1POST，修正fixture断言后同产品二进制完整通过；不放宽原schema/期限，不报上游。Before/after独立控制只加tenant_glob配置：冻结父binary无该字段成功、加字段配置拒绝、最终binary同配置成功；具体任务资格由上述真实任务认证，配置正例不代替执行。

新外部审计器保留原181个Assert AST（含重复）和除新helper/接线/路径字面量/新RunPython历史子集排除外全部normalized AST，只加本批1guard为182。新guard要求本head六noncontainer自己的两Rust边界、两test各完整成功的glob命令、Ubuntu独立browser命令、严格11field双用户完整SHA与6field浏览器收据、全局计数/顺序/唯一UUID/UTC/版本/typed JSON/原预算。Root1positive/285negative仅为synthetic driver校验，不能算产品或官方执行。独立driver两轴结果在发布前的publication gate/pin另行绑定。

自身官方资格须独立七actual（另条件draftskip）、两平台各70 ordinary Python（69 RunPython加named Currentfilecapture）、Ubuntu42实际Chromium（原41加glob1）、四候选各13唯一source/install/archive/认证artifact API digest与Currentfilecapture，再full182及fresh独立官方两轴0，才仅正文/pin回填。candidate只跑自己的Rust/原13/Currentfilecapture和parser，不跑globAPI/Chromium；Mac不跑Chromium。原40minUbuntu/60minIntel/3sheader/native10s/关闭10+5秒与15sgrace/250ms/资料5s/会话导出与对照15s/FileReader5s全部保持，不能借父head、本机、源码/交付/发布审查或前缀补资格。

上游StateKnot/Brokerrouter正式发布/合同本轮无新可消费MCP/durable。未使用paid凭据，不认证完整physical release artifact ZIP、本机实际公开安装、供应商/publicTLS/计费代理、真实业务质量/学习/Agentbrowser或StateKnotdurable。所有日志、报告和frozen binary在target外；共享target超过20decimalGB已按同repo/toolchain/profile评估复用保留，df/真实路径/CargoTAG/noGittracking已核，无target复制或清理，回收0，APFSdu和其它进程df变化不算释放。
