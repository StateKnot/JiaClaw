# 租户 JSON 重启用例的实际准入

成功的原 JSON 收据与网关观察者结算是两个事实。测试下一项真实重启前，必须确认上一项成功请求的原用户 hold 已为空，并确认下一请求实际提交了唯一模型 POST。两项核对共享原 restart gate 的 15 秒绝对截止；每次返回后再次核对截止，不增加等待预算、重发请求或清除 hold。随后仍执行原网关 SIGKILL、原 UUID GET 恢复、持久审核 hold、零模型重放和另一用户可用性矩阵。

PR134 原 `c767a17b554eecba9e15cb010c2bddbb43712c5a` 的 Ubuntu attempt1 在 `tenant_cron.py` 管理员 `user-list` 上返回 SQLite locked；仅一次已接受的 attempt2 通过原 cron 后，在租户 JSON 浏览器的 `restart gate` 超时，尚未执行预期 SIGKILL。两个失败及各实际日志、job/check-run/annotations 分别保留；原因不能合并为同一个问题。

原冻结产品和原完整 JSON Chromium 用例在本机通过。另一个受控真实 loopback 用例只延迟上一请求的原网关观察 GET：原收据已完成时原 hold 仍为 in_flight，立即新请求实际返回 429、模型 POST 为 0，原 15 秒 restart gate 超时。按上述顺序，在相同产品、相同延迟及原 15 秒总截止内，上一 hold 正常结算、下一模型 POST 为 1，随后真实重启保留该请求的 needs_review hold。这个机制证明夹具缺少成功用例的结算前置条件；官方历史失败的唯一原因仍未证明。

修订只影响夹具与验收证据，不改变产品 UI、native/gateway/registry、数据库、权限、模型工具或运行期限。109 编译输入与冻结产品摘要独立核对；原本机 Rust/其他历史用例仅继承。修订的新远端 head 仍须独立七官方作业、两平台各65必需 Python、Linux 十三 Chromium、四候选各13完整安装归档及认证 artifact API digest、全部继承合同与新的实际准入收据，再进行独立 Standards/Spec。旧 head 六成功/四候选不能转移为修订资格；不认证供应商、经验质量、Agent 浏览器或 durable。
