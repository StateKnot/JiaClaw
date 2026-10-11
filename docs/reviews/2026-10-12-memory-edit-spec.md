# Spec 独立源码正确性审查

结论：本批 fresh 源码未发现开放的缺失、部分实现、错误实现或范围扩张问题（0 项）。审查比较 `git diff 91b7b3c365c7806bd5d9d821e1677af7a714b3bc...36084fd586d6581ca17eeeb2e0ccd786bff63331`，覆盖初版 `410c35a`、修订 `19737f6` 与 `36084fd`；依据 `docs/memory-edit.md`、`docs/roadmap.md` 首条及用户生产级要求。累计基准 `4b2357fcf01f47ba08d7724edbba7accfb60972c` 的历史结论继承，未声称穷尽累计重审；未读取 Standards 报告。

按 Spec 核对独立默认关闭、审阅/管理员/standalone 门槛、严格 JSON、完整配置路径及原文/候选双摘要、现存安全目标、同值/陈旧拒绝、原非排队 writer、私有发布、无正文收据，以及浏览器勾选后单次提交、未知结果先手动读取和身份清理。未新增模型、索引、历史删除或新存储授权。

对第24行“上限取256KiB与配置 HTTP 全局上限的较小值”及“原四槽非排队控制许可贯穿有界正文捕获到实际 blocking writer 结算”，最终 handler 消费原路由启动时解析的 `RequestBodyBudget`。实际验收配置512、环境覆盖512、正常1MiB及配置0回退：两种正文 framing 的有效上限减1/等于/加1共24条件全部通过；合法正文完整发布，超限保持原字节/inode。另四个真实 stalled chunked 请求占满原池，第五次 inspect 503，四请求约5秒分别408，随后 inspect 200，目标未改变。

同一最终冻结二进制独立重跑原30条 HTTP断言（不是31），以及原两条512预算断言全部通过，进程均正常退出。此前410的 chunked 绕过 RED 记录和197的局部 GREEN保留，不转移到最终身份。`fetch.rs` 修订仅在 `cfg(test)` 下让已 accept 的 socket 实际等待 ClientHello，原10秒截止不变。

最终源 `36084fd` / tree `5269a84bf98392723a50a4fd73e15729f980a766`，binary SHA256 `b6edb73af50a16ae7d3f51271f87d19f937fbb880a4f4ea340159b3f350235e0`；独立核对105/105输入与固定提交/当前文件相符。构建本身由父代理执行。本审查未运行 Chromium、全 Rust 或官方作业；第30行要求的新固定官方 head 七作业、两平台60 Python/Linux九 Chromium及四候选各13项资格仍须独立验收，不能由本报告替代。

证据：`/tmp/jiaclaw-oct11-memory-edit-resolved-spec-identity.json`；三个 `resolved-spec-http`、`resolved-spec-global-body`、`resolved-spec-body-boundary` 目录的 `evidence.json`，脚本分别为同名 `.py`。均为可丢弃本机 stub 服务，无模型/供应商调用；测试前df可用233GiB，无共享target复制或清理。
