# Standards — reviewed lock creation

Fixed cumulative comparison: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...8138f10d8868110729ad30ba19cfbb90687c85b0`; commit list: `/tmp/jiaclaw-oct11-lock-create-commits.txt`. Historical architecture/code Standards conclusions are inherited. Fresh review covers the eight-file `ea992611b6623984eb65b8c097c747c1306ccb89...8138f10d8868110729ad30ba19cfbb90687c85b0` delta, not an exhaustive cumulative rereview.

Standards sources: user-provided AGENTS instructions; `docs/architecture.md` (filesystem capability/owner boundary); `docs/workspace-files.md` (nonqueueing writer, publication and unknown-result semantics); existing skill lock/edit/registration contracts. Repository search found no physical AGENTS, CONTRIBUTING or CODING_STANDARDS file. Tool-enforced formatting/lints are excluded.

**Hard violations: 0. Actionable heuristic findings: 0.**

`memory_io.rs::create_validated_text` retains the original writer through the preparation callback and shared `publish(..., false)`. Directory/leaf access, private staging, synchronization, atomic create-if-absent and post-publication uncertainty remain owned by the existing filesystem module. The new interface does not expose the lock or invent a second publication implementation. Similar setup to `edit_file` is bounded and expresses a different absence-only contract; extracting a speculative common mutation framework is not warranted by this delta.

`skills/lock.rs::create` keeps schema/hash, all-body byte accounting and candidate validation beside the existing source-policy implementation. Reusing `verify_source_body` and returning its actual byte count avoids another reader/hash path. `SkillDiscovery::create_lock` is the established public façade rather than a new ownership layer; the CLI does not expose model/HTTP write authority or instantiate unrelated runtime resources.

Both new Rust tests exercise the production discovery interface. The seven process groups use the actual CLI, OS lock/permission/file-size behavior and authenticated original service; they do not replace the publication logic with a test adapter. Workflow changes directly connect the same fixture to debug and optimized candidates.

All twelve Fowler smell heuristics were considered as judgment calls. No opposite-axis report was read. This review performed static/source-contract inspection; no additional Cargo build or test execution is claimed.

Standards total: 0 new open findings; no worst issue.
