# 两次任务完整回复对照

用户在 standalone、个人 Key JSON 或预览工作台打开“对照两次任务的完整回复”，输入两个不同的规范 UUIDv4 原请求编号，勾选本次私人资料读取确认，再点击核对。它帮助用户对照修改前后实际保存的文字，自行判断修改效果；不自动评分、学习、生成修订或发送新任务。

## 读取合同

复用原 `turnApi`、`Turn.snapshot` / `Turn.gatewaySnapshot` 和原授权 GET `/api/turns/{id}`。每次显式点击按 A、B 顺序各读一次，完整响应仍受原 `2 MiB + 20 KiB` wire 限额，回复各受原 2 MiB UTF-8 限额、工具记录仍受原 2048 条校验。原状态必须为 completed、active=false、正文未 purge 且存在，拒绝非法 Unicode。空回复允许；BOM、CRLF、TAB、NUL 和完整字符串保留，使用 textContent，HTML 不执行。完整正文来自服务端收据，不取截断消息 DOM 或临时预览。两份校验完成后一起显示；不展示部分成功读取。

两次读取共用一个 15 秒 absolute deadline，原 GET 使用剩余时间和同一 AbortController。每次异步返回、开始下一读及最终显示前复核 deadline、identity、epoch、两个编号和确认。编号/确认变化、显式清空、身份切换、新原任务、结束跟踪、页面离开清除私人 DOM 并 abort 本次读取；刷新不恢复。一次失败清空两侧，没有自动重试、poll 或自动重连。两次读取是各自读取时的事实，不构成原子双任务快照，也不证明显示后服务端仍保留正文。

## 权限和边界

standalone 沿原管理员 Token 范围；gateway 只读取当前个人 Key 自己的原 UUID，原 readonly Key 可查看，跨用户、撤 Key 仍拒绝。关闭新任务准入不放宽原 GET 合同。不会修改原 pendingTurn、原收据、反馈、会话历史、模型账本、hold 或工具权限；没有 PUT/POST、额外模型请求、第二 Store/搜索/读取引擎、工具或后端入口。它不比较实际文件/外部效果，不解除 unknown hold，不恢复执行，不认证业务质量、Agent 浏览器、供应商或 durable。

私人回复只在当前页面内存和 DOM。清空页面不会删除服务端任务、备份或已经保存到本机的文件；原 GET 本身有真实浏览器→本地服务网络请求，不能称零网络。

## 验收规格

`tests/task_result_compare.py` 为三种模式建立独立真实 native / gateway / registry / SQLite / 本地模型实例。每种模式两个实际任务、各一个模型 POST、零工具，模型输出含 1900 KiB 完整回复与 BOM/CRLF/TAB/NUL/HTML 字面量，以及 0 字节回复。浏览器完整原文和 SHA 与原收据一致，显式读取只两次 GET、无浏览器写请求。真实第二 GET 迟到、编号/确认变化、身份切换、跨用户、readonly、撤 Key、原 15 秒截止和清空/刷新私密 DOM；active/purge/错误 UUID/非法 Unicode/超限/tool type 是独立标明的响应故障夹具，不冒称已执行的任务。375px 页面不横向溢出、无 storage 和 page errors。

父 PR146 及更早资格只历史继承；本批须自身固定 head 官方七作业和新增 Ubuntu `Task result comparison acceptance` 完整执行，不能用本机、父作业或 source/delivery/publication review 补官方资格。
