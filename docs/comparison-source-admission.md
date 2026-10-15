# 对照验收的原任务结算前置

`tests/task_result_compare.py` 为每种工作台建立两个真实源任务。native 收据的 `completed`、`active=false` 不代表原 gateway 的观测请求已经归还该用户的写入许可。夹具直接建立下一个源任务时，可能遇到原 429 `user request already in progress`；拒绝本身保留原授权与 owner。

gateway 两种模式在原有同一个 15 秒完成观察窗口内，先只读核对原用户 `write_holds` 为空，再读取原 UUID 收据并核对完整回复及空工具记录。两次观察均使用剩余时间，返回后重核绝对截止；不另加窗口，不重发 PUT，不删除或更改 hold，不跳过失败。standalone 沿用原完成观察。四份 `ACTUAL comparison source settlement` 收据标明模式、源编号、UUID、原期限内耗时和该任务实际模型 POST 数；浏览器原来的全部比较、身份、只读、撤 Key、迟到、15 秒拒绝、私密 DOM 与零写入断言保持。

同一原任务在自己的 gateway 观测响应被暂扣时，另一真实 GET 可以先读到已完成正文。本批冻结程序的最小受控案例保留原源任务循环 AST，实际得到完整 1900 KiB 回复后，下一原 PUT 返回同样的 429；释放原观测响应后自然结算，没有手动清 hold 或重发。它证明这个夹具的前置缺口，不能证明历史 Hosted 失败的唯一原因，也不认证业务质量、供应商、Agent 浏览器或 durable。
