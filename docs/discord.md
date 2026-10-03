# Discord 授权频道定时文字

Discord 定时任务通过 Bot Token 向单独授权的 guild 普通文字频道发送结果。入站 Interactions 继续使用 Ed25519 验签、快速 ACK 和加密保存的短期 interaction token 回复；定时任务不保存或复用这些短期凭据。当前不支持定时 DM、线程、论坛帖子、公告频道、附件或媒体。

## 配置与权限

先配置 [持久化调度器](scheduler.md)、[可靠渠道](channels.md)、API Token、工作区外 SQLite，以及 Brokerrouter 提供商。以下字段加入现有完整配置；所有 ID 替换为目标安装的规范十进制 Snowflake。

```toml
[scheduler]
enabled = true

[http]
bind = "127.0.0.1:8080"
persist = true
persist_path = "../state/sessions.sqlite3"

[[http.channels]]
channel = "discord"
installation_id = "123456789012345678" # Application ID
# 仅在非空 scheduled_destinations 时配置；一个安装限定一个 guild。
discord_guild_id = "223456789012345678"
allowed_senders = ["623456789012345678"]
allowed_conversations = ["323456789012345678"]
enabled_tools = ["datetime_now", "json_query"]
timeout_secs = 120
scheduled_destinations = [
  { conversation_id = "323456789012345678" }
]
```

通过受保护的服务环境配置 `JIACLAW_API_TOKEN`、`JIACLAW_DISCORD_PUBLIC_KEY`、`JIACLAW_CHANNEL_STATE_KEY` 和 `JIACLAW_DISCORD_BOT_TOKEN`。公钥为 32 字节 Ed25519 公钥的 64 个十六进制字符；state key 为独立随机 32 字节密钥的 64 个十六进制字符，须保留以解密既有交互凭据。Bot Token 也可放在受保护配置的 `http.discord_bot_token`，环境变量优先。日志只显示配置来源；API、运行记录和发件箱不保存 Bot Token。

`scheduled_destinations` 默认空；只使用 Interactions 时不需要 Bot Token，也不配置 `discord_guild_id`。启用定时发送须同时提供非空目的地、guild ID 和 Bot Token；缺项、无定时目的地却配置 guild、非 Discord 安装配置该字段、非法 ID 或非空 thread_id 都会拒绝启动。入站会话白名单不会自动授予主动发送权限。当前每进程仅允许一个 Discord 安装。

Bot 必须安装到指定 guild，并有目标频道的可见和发送权限；频道覆盖权限也须允许发送。每次发送前，JiaClaw 使用同一 Bot Token 读取 `/applications/@me` 核对 Application ID，再读取 `/channels/{id}` 核对频道 ID、guild ID 和 `type=0`。不匹配时不会发送消息；检查不缓存。正式端点为 `https://discord.com/api/v10`；`local_test_api_base` 仅用于显式本机协议 fixture。平台合同见 [当前 Application](https://docs.discord.com/developers/resources/application#get-current-application) 和 [读取频道](https://docs.discord.com/developers/resources/channel#get-channel)。

## 创建与发送

任务管理沿用[定时投递 API](scheduled-delivery.md)。请求的 delivery 只包含固定目的地，不允许 token、URL 或自定义额外字段：

```json
{
  "name": "每小时时间报告",
  "prompt": "调用 datetime_now，简短报告 UTC 时间。",
  "schedule": {"kind": "interval", "seconds": 3600},
  "enabled_tools": ["datetime_now"],
  "timeout_secs": 60,
  "delivery": {
    "channel": "discord",
    "installation_id": "123456789012345678",
    "conversation_id": "323456789012345678",
    "thread_id": null
  }
}
```

成功运行的会话、结果和发件箱在同一事务内提交；发送在提交后进行，重试不再次调用模型。回复最多 16 KiB UTF-8，每片最多 2,000 个 UTF-16 单位，定时发送最多 16 片，保留 Unicode 原文。交互回复仍限定 6 片。超出整体边界转人工核对，不静默截断。所有 Bot POST 关闭 mentions、TTS 和自动嵌入预览。

每片的 nonce 是持久 delivery UUID 的完整 16 字节 base64url 编码（无 padding，22 字符），并设置 `enforce_nonce=true`。有效成功回执须有规范消息 ID 和精确 channel_id；若返回 nonce，也必须与请求一致。nonce 的平台去重期限有限，不能代替持久 unknown 处理或提供跨系统 exactly-once 保证。见 [Create Message](https://docs.discord.com/developers/resources/message#create-message)。

每次尝试覆盖两个 GET 和一个 POST，总期限 30 秒；单次 HTTP 最多 10 秒，响应体最多 64 KiB，无隐式重试或重定向。成功响应的 `X-RateLimit-Remaining: 0` 与有效 `X-RateLimit-Reset-After` 会延长持久安装冷却。429 必须有 JSON 数值 retry_after 和布尔 global，提供的等待头也必须有效；取保守最大值，等待上限 24 小时。可信冷却不会因重启或新任务缩短；每片最多 5 次尝试。相互矛盾的发送回执保持 unknown，即使同时提供了有效冷却也不自动重发。见 [Discord Rate Limits](https://docs.discord.com/developers/topics/rate-limits)。

## 凭据失效与未知结果

HTTP 401 在 SQLite 持久阻断当前 Bot Token 的 SHA-256 指纹，原投递 permanent_failed 并暂停原任务；同 Token 重启后不再发出 Bot HTTP 请求。更换 Token 并重启后，首次新发送重新登记凭据并验证安装；旧失败投递不会自动复活，也不会自动清除既有目标的 FIFO 阻断；管理员须先核对并取消或解决该目标旧计划，再发送后续结果。状态输出、日志和 API 不展示 Token 或平台原始错误正文。

POST 已开始后的超时、断线、无法证实的 HTTP 错误、错位回执，以及重启发现 submitting，都保留 unknown，暂停对应任务。任一 scheduled Discord unknown 会阻断同安装所有频道的后续 Bot 发送；换 Token 不解除该阻断。已有 Interactions 回复使用原独立路径。

管理员核对平台证据后，可调用 `POST /api/channels/deliveries/{id}/resolve` 提交 `{"action":"delivered","receipt":"已核对的消息 ID 与证据"}`，或明确取消所属运行余下消息。`resolve` 的 cancel 以及运行 deliveries/cancel 都属于显式人工取消，均可结束 unknown 阻断；接口不会重发或撤回旧消息。解除后其他已提交待发结果可继续，原任务仍暂停，须显式 resume 才恢复未来调度。暂停/软删除任务本身不取消已经提交的发送计划。完整鉴权、204/409、归属校验与审计清理见[定时投递](scheduled-delivery.md)。

## 升级与验收

Discord Bot 批次引入 schema v9：保留既有入站/定时来源、回执、冷却和 v8 网关派发摘要，新增 Discord Bot 凭据阻断记录。当前会话库版本为 v10，另新增[任务创建身份收据](standalone-scheduler.md#数据库容量和备份)，不改变 Discord 凭据阻断合同。升级前停止服务并备份完整 SQLite 状态与交互加密密钥；旧二进制不能直接降级。不要删除阻断记录或恢复旧备份来绕过核对；恢复旧状态不提供拒重放保证。

```sh
python3 tests/discord_scheduled.py target/debug/jiaclaw
python3 tests/channels.py target/debug/jiaclaw
```

新增 fixture 使用真实二进制、临时数据库和本机模型/Discord HTTP 服务，覆盖独立授权、错误安装/guild/频道类型零 POST、完整 Unicode 分片、nonce、持久限流、401/换 Key、未知安装阻断、真实提交后 SIGKILL、人工核对后仅继续后续发送及凭据不泄漏。既有 channels fixture 独立验证 Interactions 路径。当前批次运行结果见[验收记录](validation.md)。没有使用真实 Discord 或付费模型凭据；正式安装、频道覆盖权限、真实限流和平台可见消息仍需部署者独立验收。
