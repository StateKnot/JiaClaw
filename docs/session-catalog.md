# 会话目录的摘要、分页与 TTL

会话目录从权威 Memory/SQLite 存储返回摘要，默认且最多 50 条；不复制/反序列化 Rust `ChatMessage` 历史。两种存储只保留一页加一条 lookahead。SQLite 用主键 ID 范围和 LIMIT 查询，JSON 数组计数在存储侧完成；这仍有逐条 SQLite JSON 解析成本，不声称零成本或 OOM 压测认证。

## 公开合同

鉴权成功后 `GET /api/sessions?limit=10&after=...` 返回 `{sessions:[{id,message_count}],limit,has_more,next_cursor}`。原 `sessions` 字段保留，但旧客户端必须处理分页，不能把第一页当全部。`limit` 为规范十进制 1..=50；缺省 50。`after` 是上一页 `next_cursor`，为最后 ID 精确 UTF-8 字节的小写十六进制；最多 2048 字符，必须解码为 UTF-8。未知、重复、编码参数、空查询、超长查询、非法游标和前导零拒绝；无权请求先返回 401。个人 Key 网关复用同一查询验证，只读 Key 可分页；非法目录查询及带查询的 POST 创建路由为 403，不准入写操作。其他未知路由/方法和绝对 URI 仍遵守原 404 范围。

按 ID 的 UTF-8 字节升序，游标是排他位置。删除上一页末条后仍可继续；新插入到游标之前的记录需刷新第一页查看。每页是读取时视图，不是持久快照。`has_more=true` 时使用 `next_cursor`；末页为 false/null。无需无界 offset 扫描或分页历史。

网关驱动的 `job:` 会话在 LIMIT 前过滤，运行结果继续使用独立 jobs/runs 入口。每个目录 ID 上限 1024 UTF-8 字节；历史单行 JSON 上限 32 MiB，须为数组。越界/非法存储拒绝整页并提示核对，不静默略过。SQLite 在复制 ID / JSON 解析前检查大小，不读取正文到 Rust；这是目录预算，不扩大历史导入、读取、模型或 tracked turn 的原预算。不迁移 schema：会话仍 v11，registry 仍 v8。

列表不刷新任何会话的 `last_accessed/accessed_ms`。单个 GET 会话仍触达该会话，正常执行/导入仍沿用原持久提交语义，export 仍不触达。开启 TTL 时列表仍清理过期项；SQLite 在事务内每批最多 256 条删除，不先收集全部过期 ID。实际活跃执行、未核对 HTTP/渠道效果和未完成投递在 LIMIT 前豁免，不能因目录刷新或 TTL 丢失。

工作台只保留当前页，最多 50 条目录项，加上必要的一个 standalone 待跟踪会话占位。下一页替换当前列表；刷新返回第一页。选中的聊天正文/草稿不因翻页切换，换 Key 时清空页/游标；旧身份延迟响应不能回填。无浏览器分页历史或密钥持久化。这不接通租户预览 UI，个人 Key 对话仍走原 JSON；已有租户 SSE API 资格独立。

## 验收范围

`python3 tests/session_catalog.py BINARY` 使用真实 Memory/SQLite/HTTP 和双租户网关，检查 53 条分页、删除锚点、严格鉴权/查询、无正文摘要、选中行 32 MiB 预算、列表不触达、单 GET 触达、多批 TTL 清理、job 过滤/只读隔离/无写准入审计；不使用付费模型。

`node tests/browser.cjs BINARY` 增加真实 Chromium 当前页/末页/刷新/选中会话保留及延迟旧身份响应；`node tests/read_only_browser.cjs BINARY` 增加真实 gateway/SQLite/只读 Chromium 分页。Tokio 受控时钟验证 Memory 列表不能延长闲置会话 TTL；既有实际取消/持久 unknown/恢复豁免回归继续执行。

旧固定 head `d3bb650` 真实进程默认返回 53 条，新增回归失败；新固定 head 的本地检查、双轴审查与官方 CI 分别记录在交付证据。本批当前 head 的跨平台与四候选资格需独立完成，不能借用 PR #102。

下一步：租户预览 UI 与真实供应商资格；两个维护启发式（渠道准入事务形状、私有账本文件能力）仍归历史 Standards 轴。durable、stdio、外部写入、通用工具错误效果确定性和恢复仍开放。
