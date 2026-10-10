# Spec 源码终审：管理员技能来源锁与响应合同

固定累计基准 `4b2357fcf01f47ba08d7724edbba7accfb60972c...2a5e4c327ddd563215882c13b6982f726eaaa798`；完整 fresh 范围 `68554879..2a5e4c3`，本次新增聚焦 `a952c96..2a5e4c3`。继承架构/竞品、正文/资源/reload 与本批初审、a952 终审；不声称穷尽累计重审。规格为 pinned `docs/skill-lock.md`。

**本次新增未解决 0。** 规格要求“认证的 `/api/skills` 在已锁技能上提供 `source` 字段”。此前实际返回有该字段，嵌入 OpenAPI 却未声明：冻结旧二进制 `/api/openapi.json` 真实负例报 `KeyError: source`（`/tmp/jiaclaw-oct11-lock-openapi-negative.log`）。这与已关闭的原生聊天夹具字段 P2 是两项不同事实，不归为框架缺陷。

新源码中 `SkillInfo.source` 是可选 `$ref`，与 serde 省略未锁目录的实际响应一致；`SkillSourcePin` 是封闭对象，三个 required 字段匹配实际已锁元数据。Git object ID/完整原文件 hash 的格式、无凭据 HTTPS 来源和“声明不等于发布者认证”范围描述符合实际验证。API GET 与 reload 共用 SkillInfo，因此响应引用接线均覆盖。字段注释未改变生产策略，新增真实进程断言核对嵌入响应而非只读取磁盘 JSON。

初版聊天夹具 P2 已在 a952 提交的 `messages/enabled_tools/message.content` 修正关闭，原六组 settled 证据和独立九项锁边界保留；它们不是新 OpenAPI 二进制资格。锁读/原始字节 hash/严格启动诊断/固定 reload 策略和工作区写权限合同本次未改变。

**响应合同遗漏已完成源码修正，最终二进制实测待核验。** OpenAPI 是编译输入，不能借旧冻结二进制或 a952 绿色日志。我未执行重复构建；需最终二进制进程日志与新的固定输入身份再回填 runtime GREEN。官方 CI/四候选资格同样另计，安装、签名、审批及完整生态不扩大认证。


---

# Spec 终审：管理员技能来源锁

固定累计基准 `4b2357fcf01f47ba08d7724edbba7accfb60972c...a952c96e31b052545a81afc3fb7bec6d1470a602`；完整 fresh 范围 `68554879..a952c96`，终审新增 `8e14f484..a952c96`。commit list 为 `/tmp/jiaclaw-oct11-lock-review-final-commits.txt`。继承架构/竞品、技能正文/资源/reload 容量报告及本批独立初审，不声称穷尽累计重审。规格为最终 pinned `docs/skill-lock.md`。

**新增未解决 0；初版夹具 P2 关闭。** 初版 `/api/chat` 请求与响应字段错误现已提交为实际 `messages/enabled_tools/message.content` 合同；读六组 settled-final 实际进程日志，原生三轮正确读取已锁正文和参考，来源声明不进入模型目录，原工具白名单不扩大。原版失败日志保留，修正是夹具接线，不是生产缺陷。

生产源码与初审固定字节一致：必需锁严格接入启动、CLI、只读 doctor、认证 HTTP/SIGHUP；同次能力读取的完整原始 UTF-8 字节参与 hash，包括 frontmatter；锁/实际目录一一对应，空目录须显式空锁；失败保留旧表和来源，注册表固定策略及实际 reload worker 所有权不变；默认关闭忽略锁并保留旧行为。

独立九项冻结二进制边界全部通过，包括真实缺少技能根、64 条记录/64 位 Git ID、128 KiB 精确上限和超一字节、65 条记录、两层未知字段及重复 revision。证据 `/tmp/jiaclaw-oct11-lock-spec-boundaries-final.log`；独立初版配置夹具解析失败另存，不能当生产 RED。六组主进程日志由作者执行，我独立读取，未自行重跑全 Rust/官方候选。

最终文档明确“已获授权的通用文件工具或本机进程可以改写锁”，与既有工作区写权限合同一致；只读挂载是部署限制，不宣称新增 ACL、不可变审批或来源认证。声明、签名、安装/更新/撤销及完整生态范围未混同。官方固定 head 与四候选资格仍须独立验收。
