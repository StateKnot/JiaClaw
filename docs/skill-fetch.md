# 已审阅原始技能包下载

`skills fetch HTTPS_URL --operation-id UUID --if-bundle-sha256 SHA256 --config FILE`
要求明确必需来源锁和已有有效来源锁 JSON。操作员先从受信独立渠道取得完整原始安装包摘要；下载复用512KiB严格 version1 JSON Bundle 的完整正文和声明参考核验。它只保存已审阅原字节，不安装、修改技能策略、启用或重载运行表，也不初始化模型/MCP/索引/会话。

一个 `SkillFetcher` Module 持有原非排队 workspace writer，贯穿 DNS、持久原 UUID/意图、一次 HTTPS GET、完整有界捕获与私有收据。只接受最多2048字节无凭据/query/fragment的 HTTPS URL；仅200成功，不跟随重定向、不使用环境代理、不自动重试或解压，传输仍验证正常原生 TLS 根和主机名。一次 DNS 解析最多16个地址全部核验并固定给同一请求；默认保守拒绝本地、私有、文档及指定保留地址（IPv6仅全球单播2000::/3并额外拒绝指定特殊前缀；IPv4映射地址沿IPv4规则）。内网 registry 必须操作员显式 `--allow-private`，TLS验证仍在。HTTP连接10秒、完整HTTP请求30秒；OS DNS、原生证书与磁盘同步不宣称可抢占硬截止。取消等待不释放实际 blocking owner；进程被杀则按原记录核对。

下载前发布 `.jiaclaw-skill-fetches/UUID/intent.json`。完成后原 bundle 和 receipt 都是普通单链接0600文件、目录0700；保留完整原包，拒绝原UUID重用和安装/更新/卸载跨账本UUID重用。每账本最多64份，四种账本内任一未结算/不完整记录阻止新的fetch/install/update/remove operation，须先人工审阅；现有workspace写权限仍可修改账本，没有独立不可变ACL。

`skills fetch-status UUID --config FILE` 只读观察，不发HTTP。`skills finish-fetch UUID --if-intent-sha256 SHA256 --config FILE` 只结算已经完整保存且通过原摘要/Bundle核验的包；缺包为未知，需要人工核对，不能用finish重下载。提交/同步错误可已保存原包或收据，先status检查，不自动换UUID重发。普通HTTP/预算/内容失败保存稳定失败收据并非零退出；同UUID仍不可再次GET，另一次明确CLI调用是新逻辑请求，不保证远端日志/费用只发生一次。

成功输出 `bundle_path` 是工作区相对原捕获，可继续独立审阅后用已有 `skills install --from PATH --if-bundle-sha256 SHA256 --if-manifest-sha256 SHA256 --operation-id NEW_UUID` 安装。这是独立、显式本地安装；不是一个下载并安装事务，下载receipt不证明Git远端/发布者签名/当前已加载技能。安装后还须独立启用及认证reload/GET。原始包SHA256是内容条件，不是操作代次/ABA检测；来源字段是管理员声明。

验收必须真实TLS HTTP/CLI、原地址准入与固定、完整字节/预算/schema拒绝、无代理/重定向/重试、原writer/取消/竞争、SIGKILL与已捕获finish不再GET、状态冲突证据保留，以及四优化候选接线。测试本机信任的临时CA/内网registry不认证公网供应商、发行安装、物理断电、StateKnot durable或完整技能生态。

地址策略参考[IANA IPv4注册表](https://www.iana.org/assignments/iana-ipv4-special-registry/iana-ipv4-special-registry)与[IANA IPv6注册表](https://www.iana.org/assignments/iana-ipv6-special-registry/iana-ipv6-special-registry)；这是保守固定准入规则，非对所有特殊用途或未来地址分配的自动分类。
