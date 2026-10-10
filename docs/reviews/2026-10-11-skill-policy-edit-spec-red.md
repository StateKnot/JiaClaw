# Spec 独立 RED 补核：v2 no-op 原 hash 可重复发布

初审固定源码 `def298217bafc61ef219c416608fa7dcdcb22ec3` 的初报告保持不变。作者随后用本审查准备的 `/tmp/jiaclaw-oct11-policy-edit-spec-noop.py` 运行冻结初版二进制；本人独立读取真实日志并复算二进制摘要。

`/tmp/jiaclaw-oct11-policy-edit-noop-initial.log` 显示同一内容 token 下两次命令均 exit 0、两次 runtime_applied:false、hash_unchanged:true；manifest inode `67454736 → 67454737 → 67454738`，证明不是仅返回旧 receipt，而是两次实际成功发布。冻结 initial SHA256 `b59a54df75840d1e29fd9e821628bcfff808a13c1e95e13f6a2953d67820fcea` 已独立核验。

初审待实证 P2 现已确认。修正源码 `d347a278b2dc8b520bb8abb63defdb358caa29d6` 在原 hash/目录核验后拒绝 v2 同值，仍允许 v1 同值迁移；规格同时准确说明 hash 是内容条件、不是操作代次，保留 ABA 边界。该修正静态符合，最终真实执行尚待冻结 binary；不先关闭 runtime 资格，不借父或初版 Rust。
