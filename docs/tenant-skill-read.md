# 明确授权的租户技能按需读取

本批合同：个人 tracked HTTP 的 `http.tenant_skill_read` 与 gateway backend 同名配置都默认关闭。显式 policy 含1..16个按名称排序且唯一的 `{name, content_sha256}`；正文版本是现有 skill_read 返回的完整 loaded body SHA，不是来源签名、操作代次或 ABA 检测。启用需原 tracked tenant、必需来源锁和 skill_read；资源读取不在本批范围。复用原 SkillRegistry、只读锁、版本核验和原始/JSON 输出预算，不创建另一套目录、读取引擎、存储或模型工具。

独立 skill permission protocol1 返回完整 skills、128KiB source/body 上限、256KiB原native JSON结果上限及规范摘要。规范输入为 `[1, [[name, content_sha256], ...], 131072, 262144]` 的完整UTF8 JSON。两端在启动和每个新准入认证完整描述相等，原 SSE protocol2不变，旧backend不能opt-in。新请求只在明确选择 skill_read 时捕获 `skill_read_policy_sha256`；缺省None不改变历史 fingerprint。enabled_skills保持空，不允许 eager body 或自动技能选择。

配置epoch内权限固定。无 grant 时租户目录为空；提示目录只包含已授权的精确name/body版本，而且只有请求选择skill_read时才加入，不注入description/source/resources/trigger或任何技能正文。模型工具schema只列明确名称与摘要；execute重新核精确pair并走原loaded表读取。未授权、缺失、改版、未知参数和原raw/JSON预算失败维持已知只读NoEffect，不扩大工具权限。成功正文可以发送给该用户已配置模型；技能文本不授权其提及的exec/文件/资源/MCP。

加载失败保留原registry；成功reload使授权版本不可用时，native capability及后续新准入的live核验拒绝。gateway capability GET仅展示配置epoch的权限描述，不认证当前可用性；实际新请求仍在保存身份/hold/PUT前重新检查完整native描述，旧已构建提示不修改，旧版本工具调用不会读到新版本正文。撤权/重配须停机并重启双端，旧UUID仍只查原捕获事实，不PUT重派发或自动解除unknown hold。readonly/撤Key/跨用户拒绝、模型/hold原预算、取消和真实worker所有权保持。渠道/cron仍原两工具，独立OS ACL、管理员exec、来源真实性、自动学习/经验质量和StateKnot durable不在资格内。

工作台验证完整描述/规范SHA与secure WebCrypto，显示所有name和版本，skill_read默认不勾选；明确选择后捕获同一权限摘要。identity变化清旧目录/选择/私人DOM；无私有skills/catalog额外探查、浏览器storage、自动请求或模型请求。历史结果按原请求授权处理，不借当前能力恢复权限。管理页面Chromium不认证Agent浏览器工具。

本批验收Interface已由用户任务授权：真实native/gateway HTTP准入/能力/原历史收据、现有Agent模型/原skill_read与reload Interface、实际工作台选择及迟到identity。测试从这些公共Interface观察完整提示/模型工具调用和保存结果，覆盖两隔离用户、默认关闭/双端不符/旧backend负例、原hash兼容、提示最小目录、工具pair/未知参数、完整预算、已提交模型中的reload/取消、readOnly/撤Key/跨用户、权限重配及不重派发。冻结父缺能力负例与新源码GREEN、独立Standards/Spec以及新fixed head官方资格分别保存；fixture不认证供应商或业务效果。


## 本批固定源码与资格边界

最终源码 `1593e6cb3d6b8e5ca1daad220b3a0c6ffbb0d7c6`，生产输入修订 `e90e8612a8e97bf3be5625908ed1c08d3125fc5a`；108完整编译输入与冻结binary SHA `ce88fd9ca7c1678feb5d181b7c89f1caf3d9d7edf131691cf6792121fb57f77d` exact。真实完整本机1306Rust（463/123/720）及1ignored、lockedbuild/fmt/required correctness+suspicious Clippy均exit0。十三套冻结binary进程/parser验收逐套exit0：skills/read/resources/lock/activation、tenant file/HTTP/stream、read_only_keys/doctor/e2e、parser11及新七HTTP/OS+四实际Chromium；普通新七组另行exit0。Playwright1.62.1使用现有custom Chromium153.0.8010.12(1243)，不是缺失默认1234分发；未本机optimized/发行包/Docker/其余全部历史process或Chromium重跑。

独立source两轴最终各0开放：[Standards](reviews/2026-10-12-tenant-skill-read-standards-final.md)、[Spec](reviews/2026-10-12-tenant-skill-read-spec-final.md)。初Standards P3重复选择路径与初Spec P3静态权限/实时可用性措辞分别保留原报告；修订共用同一个可失败选择函数并准确限定native/new准入live检查。Spec原gateway200断言RED基于错误的实时ready预期，不认证授权绕过；原日志与关闭后实际native503/gateway200静态/newPUT503/UUID404/零POST/零hold并存。Spec另独立最终七HTTP+四Chromium、六请求/十实际modelPOST/four exact-pair组、迟到WebCrypto身份切换第五Chromium均exit0，108input/identity errors[]；不把根13套当该agent重跑。

冻结父PR131 ed64支持的旧contract不能打开新gateway opt-in，valid配置负例实际exit1；早期不可执行冻结file/缺必需description及路径/driver字段准备错误分开，未冒称产品RED。源码初75adca3/b795的完整1306及七组/四browser为初资格，后修订e90/1593/ce88才是最终资格；初及最终日志、raw两轴与身份均在 `/tmp/jiaclaw-oct12-tenant-skill-source` 及独立agent目录，最终交付pin在 `/tmp/jiaclaw-oct12-tenant-skill-delivery/pin.json`。共享target按repo/toolchain/config复用保留，无target复制或专用cache/清理，实际回收0；其它进程改变df不归功于本批回收。

固定累计4b2357及父PR131资格仅继承，本批fresh源码22路径，不称穷尽累计重审。交付raw copy/发布身份与新head官方七actual/两平台64必需Python/Linux十二真实Chromium/四候选各13安装归档源码/API digest及154audit guards分别认证；新head官方完整证据尚未取得时保持unqualified，不借父/源码或本机补齐。新功能不是自动学习、完整生态、供应商/远端来源/公开发行安装/业务效果、独立OS ACL、物理断电或StateKnot durable资格。
