# 已审阅本地技能更新

操作员 `skills update` 更新必需来源锁中已登记且明确停用的私有本地目录。先另行停用并核对运行服务；更新不自动改变原 registry 或取消已选择工作。完整 schema1 JSON 包与安装格式相同，须提供审阅原包、原策略、当前完整原始 SKILL.md 的摘要。旧正文、全部声明参考、完整目录和新包均验证原字节；目录0700、普通单链接文件0600，无额外文件。

```sh
jiaclaw skills update --config config.json --operation-id SAVED_UUID \
  --from reviewed-version.json --if-bundle-sha256 RAW_BUNDLE_SHA256 \
  --if-manifest-sha256 ORIGINAL_POLICY_SHA256 --if-current-skill-sha256 ORIGINAL_BODY_SHA256
jiaclaw skills update-status SAVED_UUID --config config.json
jiaclaw skills finish-update SAVED_UUID --if-intent-sha256 SAVED_INTENT_SHA256 --config config.json
```

记录在 `.jiaclaw-skill-updates/UUID/`。包/每文件/策略预算沿用安装的512KiB/128KiB/128KiB及完整256项候选扫描；一操作最多32项状态/160项目录扫描。v1停用不可能，故目标须为明确停用v2；其他来源原样保留。跨文件系统或平台不支持EXCHANGE时不降级为复制/删除，失败先核对原身份。

接口为公共 `update/inspect/finish`、真实 CLI、原 workspace writer、鉴权 reload/GET 与实际 OS sync/SIGKILL。一个原非排队 writer 贯穿原/新内容捕获、原 UUID/意图、完整私有新目录、停用的新来源策略、Linux/macOS EXCHANGE 交换和收据。原打开的只读目录能力可继续读旧版本。

原 UUID 最多启动一次；未结算安装或更新阻新操作。更新最多64份原身份记录，保存原/新包、原/候选策略、目录 dev/ino、意图与收据。相同内容也不能重复交换同一次操作。意图保存前中断为 preparation_incomplete，保留人工审阅；之后 prepared、policy_published、files_published、completed。显式 finish 只消费保存身份与字节，原意图摘要须吻合，不重读输入。

状态/内容/目录身份冲突或不安全文件拒绝并保留证据，不在冲突前删暂存内容。策略先发布、目录后交换，并非同时原子；交换后当前为新目录、私有 stage 为旧目录。相关目录同步后保存历史收据。发布后错误可能已提交，先查询原 UUID，不按退出码推断零效果或自动重发。

成功保持 disabled/runtime_applied:false，另行 enable 与认证 reload/GET 才加载新版本。历史收据不证明当前加载或当前完整性；旧版本保留不等于自动回滚接口。来源是管理员声明，hash 非单调代次/ABA，现有 workspace 权限仍可改变 journal，无独立不可变 ACL/签名。没有远端下载、模型/HTTP写入、租户扩权、自动启用/重载/启动恢复。真实本机崩溃矩阵不认证物理断电、发行安装、供应商、完整技能生态或 StateKnot durable。

## 验证边界

固定源码 `1bb4406` 实际1272 Rust（另1忽略）、21进程套、八更新CLI/OS组及四公共Rust边界通过。六真实sync→SIGKILL阶段、交换后EIO、相同字节目录身份、完整冲突证据和原writer下鉴权GET/reload留证。初版跨安装/更新复用UUID的两条实际RED及修订GREEN分别保留；同一writer内拒绝任一生命周期已使用原UUID。源码双轴新增开放0，累计基准4b2357fc历史继承且fresh范围明确。新head七官方作业、55必需Python/七Chromium和四优化候选独立待验收，不借父、本机或初版资格；详见[验收记录](validation.md#2026-10-11-本地技能更新)。
