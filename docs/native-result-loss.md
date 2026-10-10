# 已完成工具的结果丢失边界

本批处理[累计审查](reviews/2026-10-10-architecture-code-product-review.md)中确认的 Spec P1，以及当前部署/HTTP说明漂移的 P2。会话目录资源风险和两项维护启发仍开放，不能将本批修复当作整个审查或个人 Agent 里程碑全部完成。

## 执行合同

原生 Brokerrouter 工具循环完成一次调用后，若该结果的序列化 JSON 超过既有256KiB预算，立即停止同批剩余工具和下一轮模型请求。返回 `requireshumaninput`，工具记录保留调用名称、参数和“tool completed but result exceeds 256 KiB; do not replay this operation”。若提供会话编号，HTTP/普通 CLI 仍提交用户可见的中断说明；这不是持久工具执行日志。

该边界在共享执行循环内成立，不依赖 `ChatProgress` 或传输模式，覆盖普通 `chat_for`/CLI、兼容 `/api/chat` JSON及完成后SSE、渠道/调度和已有持久流式入口。已执行的调用不回滚，原始大结果按预算丢弃；核查工作区、原工具/平台效果和模型账本后才能决定下一步。工具的源文件读取或stdout上限不等于序列化结果上限，JSON转义和工具自身的JSON包装也计入预算。

只调整确定的完成结果丢失。普通入口对其他工具错误的既有处理保持不变，尚未由本批提供通用效果确定性认证；流式入口原有的错误/超时停止行为也保留。没有新的结果截断策略、自动重试、语义重复调用检测、状态迁移或 StateKnot durable 接线。已完成但未知的外部写入和恢复语义仍需独立生产合同。

## 固定版本红绿反馈

审查已用真实非root/无网络Docker工具定位普通循环继续执行；本批复用该反馈与已确认的原因，未重复增加临时生产日志或推测框架缺陷。最小回归使用真实文件工具：合法UTF-8输入仅100000字节，JSON转义后的完成结果超限；模型先请求一次追加，再读结果，并在同批或下一轮请求额外写入。`--exec` 复核原始的实际命令追加与300000字节输出场景，不将文件模拟当作沙箱认证。

旧二进制来自head `d714a831a363a871c435976cb239899a871e47aa`，SHA256 `264c56b486f0610370445cfae236aecc9aee5d4c9cd00f6dc1f219c1800d040d`。两个模式的公开 HTTP/普通 CLI 回归均失败，观察如下：

| 场景（文件和exec各运行一次） | 旧行为 | 修订行为 |
|---|---|---|
| 小输出对照 | 1次效果、2次模型请求、completed | 相同 |
| 超限后同批写入 | 额外写入已发生、completed | 剩余写入未发生、1次模型请求、requireshumaninput |
| 超限后下一轮提出相同效果 | 2次效果、3次模型请求、completed | 1次效果、1次模型请求、requireshumaninput |
| 兼容完成后SSE | 剩余写入已发生、completed | 剩余写入未发生、requireshumaninput |
| 普通CLI | 剩余写入已发生、Completed | 剩余写入未发生、RequiresHumanInput |

新本机冻结binary SHA256 `377f7529ecb9043c001afa7f8428063a07cb781f95d8f0e7c87e738b10d6067e`，十组均通过。文件模式每次记录原追加与读取两项已完成调用；exec模式每次保留一项实际命令记录。HTTP历史核对不新增模型调用；CLI使用真实会话存储。日志与固定字节保存在本机 `/tmp/jiaclaw-oct10-result-loss-delivery/`，不是发布资产。源码最终head的完整检查和官方CI分别记录，旧PR101成功不能替代。

同一冻结binary的19个相关真实进程套件（原生工具、文件/复制/记忆、MCP、路由/模型账本、CLI/HTTP/租户协议、调度/渠道/投递、E2E）通过；同一生产源码的1223 Rust、fmt、必要Clippy及parser9通过。本机使用任务专用空 `CARGO_TARGET_DIR`，关闭增量和dev/test调试符号，保留测试断言；未复制已有target。跨平台七作业/四候选及新十组在官方CI中的固定head资格仍需单独核对。

## 自动验收接线

```sh
python3 tests/native_result_limits.py PATH_TO_JIACLAW
# Requires explicitly selected Docker and an already pulled digest-pinned image:
JIACLAW_TEST_DOCKER=/usr/bin/docker \
JIACLAW_TEST_EXEC_IMAGE=IMAGE_AT_SHA256 \
python3 tests/native_result_limits.py PATH_TO_JIACLAW --exec
```

普通CI在Linux/macOS强制运行文件模式；Linux真实沙箱步骤强制运行exec模式；四平台候选工作流在真实优化binary上强制运行文件模式。所选exec依赖缺失即失败，不静默跳过、不自动拉取、不使用宿主shell。Docker的命令映射只接受管理员配置；测试只挂载一次性工作区，以65534:65534、无网络、只读rootfs和既有沙箱资源限额执行，结束后只清理带本次精确工作区标签的容器。

## 当前文档与剩余项

部署/架构统一注明会话schema11（网关registry仍v8）；HTTP文档按standalone管理员与个人Key网关分别说明授权、配对gateway_protocol2和JSON/SSE原身份接口。个人Key目录允许全权限及只读查询；只读Key不提交或探测原生工具/技能管理目录。PR101的租户SSE API资格已回填，个人Key工作台当前仍使用JSON，预览UI尚未接线。

下一项为会话目录存储侧摘要/有限分页/TTL触达，随后租户预览UI；通用工具超时/同步错误的效果确定性、StateKnot durable/委派、stdio/外部写MCP、真实供应商/安装、多模态与公开发布仍分别开放。本批为应用执行边界修复，不向两个框架重复建issue。
