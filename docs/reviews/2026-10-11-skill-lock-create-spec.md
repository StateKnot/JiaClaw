# Spec review — reviewed initial skill source lock

Fixed cumulative baseline: `4b2357fcf01f47ba08d7724edbba7accfb60972c`. Fixed source: `8138f10d8868110729ad30ba19cfbb90687c85b0`.

Canonical comparison: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...8138f10d8868110729ad30ba19cfbb90687c85b0`; full commit list: `/tmp/jiaclaw-oct11-lock-create-commits.txt`. Historical architecture/Spec conclusions are inherited. Fresh review covers `git diff ea992611b6623984eb65b8c097c747c1306ccb89...8138f10d8868110729ad30ba19cfbb90687c85b0`, one commit/eight files; this is not an exhaustive cumulative rereview. Spec: `docs/skill-lock-create.md`, with existing lock/activation/registration contracts.

**New unresolved findings: 0.** No missing/partial requirement, unasked behavior or incorrect implementation found within this fresh scope.

- “输入是工作区相对普通单链接 UTF-8 文件，最多128KiB” and “本次实际捕获的完整原字节”: existing capability reads reject unsafe parents/leaves; parsed/hash-checked captured bytes are preserved at publication. Strict v2 parsing requires explicit false and validated source declarations.
- “原非排队 workspace writer 实际持有到结算”: `create_validated_text` acquires the original owner before target checks/prepare and retains it across source/body/catalog validation and existing private create-if-absent publication. Existing target, including unsafe target, is rejected. Postpublication uncertainty retains the existing inspect-before-new-request contract.
- “所有声明的正文均需现存且可安全读取；正文合计最多2MiB”: every disabled declaration is independently body-parsed/hash-checked before the complete catalog scan; unchanged metadata/reference declaration and entry budgets remain enforced.
- “runtime_applied:false” and “不自动加载技能…发起收费请求”: the CLI only loads configuration and invokes the discovery operation; no host/model/MCP/index initialization or registry mutation is introduced.

Independent real CLI evidence: `/tmp/jiaclaw-oct11-lock-create-spec-boundaries.py` and `.log`, four groups passed. Verified exact 128KiB input/+1; duplicate JSON/invalid UTF-8/null/unknown fields; 64 declarations at exactly2MiB/+1; 256/257 directory entries; missing body/root and skill-directory links; empty v2 policy. Frozen initial binary SHA256: `337f45547ec592afac7781d378afa7369e9e36503731b33142deae1de3185fd8`. Pretest df: `/tmp/jiaclaw-oct11-lock-create-spec-df.txt`. No Cargo/cache, full Rust, physical deadline, supplier, installer, durable or new-head official qualification claim.
