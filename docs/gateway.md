# 多用户网关与独立后端

`jiaclaw gateway` 为个人 API Key 绑定一个专属 JiaClaw 后端。每个用户使用不同的进程、工作区、SQLite 会话、身份/记忆文件和 Brokerrouter 虚拟 Key。网关负责鉴权、固定后端映射和不确定写入暂停；隔离依赖本页的容器、网络、存储与运维配置，不能只给同一个后端换两个 Key。

首批支持同源 Web 聊天和会话管理。渠道、定时任务、HEARTBEAT、MCP、exec 和其他后台执行不在此多用户准入边界内，部署样例全部关闭。普通 `jiaclaw serve` 仍是单用户实例；本批不改变 StateKnot durable 或真实供应商认证状态。

## 请求与身份合同

客户端连接共享网关，在工作台输入管理员签发的个人 Key，或携带 `Authorization: Bearer <个人 Key>`。一个用户与一个 backend ID 永久绑定，客户端不能通过 URL、请求头、session ID 或模型参数指定后端。后端 API Token 只供网关使用，不发给用户；Brokerrouter 虚拟 Key 留在各自后端。

| 接口 | 网关行为 |
|---|---|
| `/`、`/ui/app.js`、`/ui/app.css` | 公共静态工作台，不包含凭证或会话；Token 保留在页面内存 |
| `GET /health` | 公共简要存活状态，不展示用户或后端列表 |
| `POST /api/chat` | 仅 JSON，必须明确 session_id；stream=true 和 SSE Accept 拒绝 |
| `GET/POST /api/sessions`、`GET/DELETE /api/sessions/{id}` | 仅操作当前 Key 所属的独立后端 |
| `/api/sessions/{id}/export`、`POST /api/sessions/import` | 支持受限 JSON/JSONL；只允许约定的 format/id/overwrite 查询参数 |
| 其他路径 | 不代理，返回 404；包括渠道、jobs、metrics、工具/技能管理和任意 URL |

会话 ID 限 1–128 个 ASCII 字母、数字、短横线、下划线、点；保留值 `.`、`..`、`import` 不可使用，不接受编码路径绕过。两个用户可以使用相同的 session ID，记录仍位于不同数据库。工作台切换或鉴权失败时清除上一身份的会话/消息/输入状态。

网关请求体最多 512 KiB，响应最多 2 MiB；请求体读取期限 10 秒。每个后端一次最多一个请求，全局 `max_in_flight` 默认为 16、范围 1–64；超过容量返回 429。后端转发期限默认 180 秒、可配 10–300 秒，另有前述请求体读取时间及数据库准入等待，不能当作端到端总期限。后端还会应用自己的鉴权、体积和速率限制。客户端不要自动重发失败的写请求。

## 生产部署的必要边界

参考 [`deploy/gateway/compose.yaml`](../deploy/gateway/compose.yaml)，初始 Alice/Bob 示例具有以下布局：

```text
用户 -- HTTPS/TLS代理 -- 主机loopback:8088 -- gateway
                                         /       \
                             alice_private       bob_private
                                   |                   |
                             alice-backend        bob-backend
                             Alice专属/data       Bob专属/data
```

- 网关有独立 registry 卷；两个后端各有独立、**已验证字节/ inode 限额**的卷，不得共享或复用相同外部卷名。Compose 使用 external 卷，故意不自动创建普通无限额 local 卷。CPU/内存限制不限制磁盘，`storage_opt` 也不能推断成命名卷限额。
- 三个容器均 UID:GID 10001:10001、只读根文件系统、cap_drop=ALL、no-new-privileges、PID 128、内存 512 MiB、CPU 1、16 MiB tmpfs。没有 Docker socket、host PID/network 或额外设备。日志最多 3 × 10 MiB/容器；仍须监控宿主的镜像、备份和总容量。
- 后端没有 published port，分别只接入自己的 internal bridge。网关接入两条内部网络与入口网络；backend A 不得到达 backend B 的私网地址。此配置限定 IPv4，并禁用容器 IPv6，不能自行增加未受审核的网络或地址族。
- **internal bridge 本身不等于宿主服务隔离。** 在容器启动前使用下述 network_guard，为两个精确桥接口设置宿主 INPUT 拒绝规则。不得向后端暴露宿主 Docker API、管理端口、云 metadata 或其他私网服务。
- 模型通道必须另行部署：将分别受治理的 Brokerrouter 接入对应 tenant 的内部网络，或使用只转发固定 Brokerrouter origin 的专属 egress 代理。不要给两个后端接共享 egress bridge，也不要直接取消 internal 属性以访问公网。专属 Brokerrouter 可以保留其独立、受控的供应商出站网络；各 tenant 的虚拟 Key/预算仍须隔离。这些供应商设施不由示例 Compose 创建或认证。
- HTTPS 证书、自动续期、请求体/连接/速率限制由可信 TLS 反向代理配置；只将该代理连接到主机 loopback 发布端口。`allow_remote_bind` 不开启 TLS，`allow_private_http` 是管理员对受保护内部链路的明确声明，不能用于公网明文后端。反向代理不得记录 Authorization、请求正文或用户 Key；保护管理入口与日志。

容器共享宿主内核，防火墙和 Docker daemon 属于可信管理面；上述边界不提供抵抗宿主管理员、内核漏洞或错误的额外挂载的保证。网关本身是能访问所有后端 Token 的可信组件，禁止将它的 registry 或 Secrets 挂入任何用户后端。

## 准备与启动

1. 在独立 Linux 主机上准备当前 Docker Engine/Compose 与 iptables，确认本地 rootful daemon。升级 Docker、防火墙或网络配置后重新做隔离验收；不把 Docker Desktop/远程 daemon 当作本 fixture 的宿主防火墙证明。
2. 由存储管理员预置三个独立的有限额块文件系统或可靠 quota 卷，分别限制容量和 inode，并留有宿主容量余量。验证耗尽其中一个时其他卷与 registry 仍可写。卷根目录必须归 10001:10001，只有该运行身份可写；建议 0700。禁止把示例中的占位卷名替换成同一卷。持久文件系统须支持 SQLite WAL/FULL 同步，不能用 tmpfs 假装持久存储。
3. 复制 [环境模板](../deploy/gateway/.env.example) 为部署目录的 `.env`；设置已审核镜像 digest、三个外部卷名和 Secret 目录绝对路径。修改 [Alice](../deploy/gateway/alice.toml)、[Bob](../deploy/gateway/bob.toml) 的逻辑模型与各自网关地址。backend.agent.name 必须与 [gateway.json](../deploy/gateway/gateway.json) 的 id 精确一致，地址、后端 Token 和数据卷须分别独立。
4. 从 Secret 管理准备四个文件：`alice-backend-token`、`bob-backend-token`、`alice-brokerrouter-key`、`bob-brokerrouter-key`。前两个是互不相同的 32–4096 字节可见 ASCII 随机令牌，后两个是各自授权/限额的 Brokerrouter 虚拟 Key。文件只读挂载且各后端只看到自己的两个文件；文件属组 10001、权限 0440，父目录只允许可信管理员访问。Compose 的本地 Secret 文件挂载沿用宿主权限，不应依赖 uid/gid 字段替你修正所有权。不要将 Secret 放进工作区或仓库。

使用固定项目名称以便防火墙核验网络归属：

```sh
docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env \
  -f deploy/gateway/compose.yaml create
python3 deploy/gateway/network_guard.py apply --project jiaclaw-users
```

`network_guard.py` 只接受本机 Linux Docker；先检查两个网络属于该 Compose 项目、是 IPv4 internal bridge，再对精确桥接口加入带项目标记的 INPUT 拒绝规则。它不修改默认策略或其它项目，失败会回滚本次新增规则。 apply 还检查实际 INPUT 顺序：本项目两条精确 REJECT 之前只允许已知终止 DROP/REJECT；任何 ACCEPT、跳转/goto 或无法判断的规则在前都会失败，不能仅因规则已经存在就报告成功。若旧规则被放行规则遮挡，先停止所有相关容器，再执行本项目 remove → apply 并复验；工具不会按不稳定的规则编号删除或改动无关规则。需要 root 或无交互 sudo 权限。防火墙规则不是持久配置管理的替代物：重启/防火墙 reload 后必须在启用服务前恢复并核验规则。专用 Compose 默认 `restart: "no"`，避免 Docker 自动启动早于防火墙恢复；由管理员或已审查的 supervisor 按 create → guard → up 的顺序启动。若环境不能保证此顺序，保持服务停止；不能只修改 restart 策略就宣称具备安全自动重启。

按已审查的模型部署连接对应专属 Brokerrouter，例如：

```sh
docker network connect --alias brokerrouter-alice jiaclaw-users_alice_private YOUR_ALICE_BROKERROUTER_CONTAINER
docker network connect --alias brokerrouter-bob jiaclaw-users_bob_private YOUR_BOB_BROKERROUTER_CONTAINER

docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env \
  -f deploy/gateway/compose.yaml up -d
```

这些容器必须是管理员已部署并认证的对应模型网关，不能将任意现有公共服务或另一个用户后端接进去。示例不是带凭证的供应商一键部署；没有这些受限通道时，真实模型请求不可用。网关启动逐个检查后端 `/health.agent_name`，身份不匹配或后端不可达时拒绝启动；该检查不等于真实模型调用验收。

在公开入口前实际验证：宿主 metadata/管理服务和另一 tenant IP 不可达；只有网关能持有各后台 Token；同名会话跨用户不可见；单用户磁盘耗尽不拖垮另一用户及 registry。`network_guard.py` 处理 INPUT 边界，Docker 的跨 bridge FORWARD 隔离和专属 egress origin 还必须分别验收。

## 用户与 Key 管理

管理命令只在可信主机/容器内执行，没有公开 HTTP 管理 API；用户不能自己指定 backend 或签发 Key。将下述 `YOUR_*_UUID` 替换为 CLI 返回的公开 ID，不是 Token：

```sh
docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env \
  -f deploy/gateway/compose.yaml exec -T gateway \
  jiaclaw gateway user-add --config /etc/jiaclaw/gateway.json --backend alice
```

首次签发输出一份 JSON：`user_id`、`key_id`、`token`。Token 只显示一次，通过受保护渠道交付给对应用户；不要留在终端录屏、工单或 shell 历史里。registry 只保存随机密钥的 verifier，不可恢复明文。每个 backend 只能绑定一个用户，不能利用新增用户绕过暂停状态。

```sh
jiaclaw gateway user-list --config /etc/jiaclaw/gateway.json
jiaclaw gateway key-add --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID
jiaclaw gateway key-rotate --config /etc/jiaclaw/gateway.json --key YOUR_KEY_UUID
jiaclaw gateway key-revoke --config /etc/jiaclaw/gateway.json --key YOUR_KEY_UUID
jiaclaw gateway user-disable --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID
jiaclaw gateway user-enable --config /etc/jiaclaw/gateway.json --user YOUR_USER_UUID
```

以上简写均在 gateway 容器内执行，可使用前例的 `docker compose exec -T gateway` 前缀。Key rotation 在同一事务签发新 Key 并撤销旧 Key。运行中的网关每次准入读取最新用户/Key 状态，管理 CLI 可同时使用 registry；旧 Key 不须等待服务重启才失效。禁用用户阻止未来准入，但不撤回已提交的后端动作；重新启用不复活已撤销 Key。最多 32 用户、每用户 8 把 active Key、总 Key 历史 1024 条，较旧 revoked Key 可被清理；管理/写入审计保留最近 4096 条，长期记录需另行安全归档。

网关 registry 放在独立 `/data/gateway/registry.sqlite3`，目录须为当前 UID 私有 0700、数据库 0600；首次创建会设置这些权限。`/data` 卷本身必须可由 10001 创建该子目录。网关进程有独立锁，禁止第二个 serve 同时打开同一 registry；CLI 管理使用短 SQLite 事务，不持有该服务锁。

## 不确定写入、停机与恢复

每次写入在转发前先持久化 hold。网关不自动重试，不跟随后端重定向，也不把用户的 Authorization/cookie/代理头原样交给后端。只有完整且通过验证的完成响应才解除 hold；例如聊天必须 completed、会话 ID 匹配且无工具错误。HTTP 200 但仍需人工处理的响应带 `x-jiaclaw-write-review: required`，保留原结果并暂停后续写入。

超时、异常、非成功后端响应、部分工具完成或重启遗留 in_flight 均须核对。后续写入返回 409；`user-list` 可查看 request ID、hold 状态与原因，读取历史仍可用于核查。客户端断开不释放已准入工作的占用；换 Key、用户 enable 或网关重启不能清除 hold。网关不具备供应商操作身份或跨系统 exactly-once 恢复能力。

管理员先确认对应后端已停止执行、查看会话/工具/模型账务并处理外部效果；必要时先停机终止旧进程，确认没有后台工具或供应商工作仍在进行。单纯取消浏览器请求或重启网关不能证明后端空闲。完成核对后才能执行：

```sh
jiaclaw gateway review-clear --config /etc/jiaclaw/gateway.json \
  --user YOUR_USER_UUID --confirm-backend-idle \
  --note '写入本次核对的证据引用和处理结论，不含正文或密钥'
```

该命令只解除 needs_review，拒绝清除正在执行的 in_flight；声明不是自动探测。它不会重跑旧操作，也不能恢复丢失的模型结果。用户后续动作是新的显式操作，仍应避免重复外部效果。

生产 Compose 给网关 320 秒正常停止宽限，为最大 300 秒后端转发期限、请求体读取和排空保留余量；后端 35 秒与其 30 秒本地优雅退出相配。更新时先排空并停止网关，再停止后端；不要反过来让已转发请求失去接收方。强制停止会保守留下 hold。

备份前停止网关和后端，分别备份 registry（含 WAL/SHM）、各自完整 `/data` 和所需 Secret，保持所有权/私有权限。三个库不是一个跨服务事务。恢复旧 registry 会回退 Key 撤销、用户禁用和 hold 记录，不能直接恢复对外服务；必须重新核对恢复点之后的 Key/准入/外部效果，执行必要撤销并确认后端空闲。恢复旧后端库也不能撤回模型/工具效果。限额满、权限错误或迁移失败须保持拒绝准入并修复，不删除审计来继续执行。

卸载或重建网络时先停止服务，再移除本项目规则，最后删除网络；不要在运行期间解除保护：

```sh
docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env -f deploy/gateway/compose.yaml stop
python3 deploy/gateway/network_guard.py remove --project jiaclaw-users
docker compose --project-name jiaclaw-users --env-file deploy/gateway/.env -f deploy/gateway/compose.yaml down
```

不使用 `down -v` 清理生产状态。若重新创建网络，其桥 ID 会变化，必须重新 apply 和验收；不能把旧规则存在误认为新桥已经保护。

## 容器验收

```sh
python3 tests/gateway_container.py YOUR_BUILT_JIACLAW_IMAGE
```

该脚本要求**原生 Linux、本机 rootful Docker、mkfs.ext4、losetup、iptables、root 或非交互 sudo**。它在新建临时目录中创建三个 64 MiB 稀疏普通文件，只对这些精确文件格式化 ext4，再关联空闲 loop device 并核验 backing-file，交给 Docker local driver 挂载。不会对已有块设备 mkfs，也不使用无限额普通卷或临时内存盘代替持久容量验证。流程依据 [Docker 官方块设备卷示例](https://docs.docker.com/engine/storage/volumes/)；Docker local driver 不替代 `losetup`。

脚本启动真实镜像网关和两个 stub 后端，验证同名会话、独立 workspace/凭证挂载/网络/PID、宿主 HTTP 哨兵与 metadata 不可达、在线轮换/撤销/禁用、后端重启、SIGKILL 后持久 hold 与明确核对，以及将 Alice 的专属 ext4 填至实际 ENOSPC 后 Bob 和 registry 仍可写。会精确删除本次容器、卷、防火墙规则和核验过的 loop；清理失败则保留镜像文件并明确报错，避免删除仍挂载的文件。

缺少文件系统/loop/firewall 能力会失败，不是 skip/pass。该脚本不使用真实供应商 Key；本地语法/静态配置检查不能替代 Linux 实际运行证据。最终执行与跨平台证据见[验收记录](validation.md)；它不认证内核安全、真实 TLS/egress、供应商计费或 StateKnot durable。
