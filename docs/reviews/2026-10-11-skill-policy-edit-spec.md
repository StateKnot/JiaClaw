# Spec 源码终审：条件技能策略编辑

固定累计 `4b2357fcf01f47ba08d7724edbba7accfb60972c...d347a278b2dc8b520bb8abb63defdb358caa29d6`；fresh `3c4dde50..d347a278` 八路径，增量 `def29821..d347a278` 三路径。继承架构/启停及初审，未穷尽重审累计实现；交付文档后续须另审。

**新增未解决 0；初审生产 P2 已确认并关闭。** 旧规格“同一原hash，只有一个协作修改可以提交”与规范化 v2 no-op 两次实际发布冲突，初版 RED 两次 exit0、同 hash、三个 inode 已独立读回；原初审和 RED 补核保持不变。最终源码在原 hash/登记目录核验后明确拒绝 v2 同值，不 publish，v1 同值仍允许迁移。新规格准确限定 hash 是原始内容条件，不能证明没有中间修改（ABA），未把协作 writer 提升为权限 ACL。

本人在最终冻结 binary 实际执行七组 CLI 边界，全为 PASS：v1 同 true 迁移保留两个来源/明确启停/0600；v2 双向 no-op 与错 hash 保持字节和 inode；其他启用项漂移拒绝，目标漂移停用修复；外部链接叶启用拒绝；真实 workspace flock 覆盖编辑后正确释放；两个不同目录共享 hash 仅一方提交且来源保留；含凭据 origin 拒绝且诊断不泄露。没有供应商请求、网络或编译。

原能力 FD 读/解析/hash、候选全表扫描、暂存 sync/同目录 rename/目录 sync 在共享 writer 内；提交前错误保持 manifest，post-rename 同步失败明确可能已提交，没有自动重试。成功 receipt 仅 disk_policy/runtime_applied:false，没有模型或 HTTP 写入口、自动 reload、取消或安装/签名扩张。

94 个生产输入已核对在 d347 Git blob/当前文件逐字节一致。最终 binary SHA256 `43ef5e25…83fa61`；独立日志 `/tmp/jiaclaw-oct11-policy-edit-spec-boundaries-final.log`，SHA256 `d72e63c7…0495df`。测试前 df 324780520 KiB 可用，无新 cache 或生产编辑。作者最终全 Rust/主七组及官方资格仍在进行，不借旧测试或父资格。
