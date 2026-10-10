# Tool Failure Effects — Standards Review

Fixed cumulative comparison: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...357f86c4ec17220df74d8972ef60363c1edf1d39`; 99 commits, 221 files, 114,326 insertions/10,501 deletions. Commit list: `/tmp/jiaclaw-oct10-tool-failure-review-commits.txt`. Fresh focus: complete `git show 357f86c4ec17220df74d8972ef60363c1edf1d39` (parent `4434679`) and relevant tool/ownership call paths. Historical coverage is carried from the architecture, doctor-readiness and doctor-capability reports; this is not a fresh exhaustive audit of the cumulative 114k lines.

## Standards

**0 new documented-rule violations; 0 actionable baseline-smell heuristics; 0 unresolved Standards findings.**

The user's rule is “都需要生产级可用的方案”. No tracked AGENTS/CONTRIBUTING/CODING_STANDARDS/GLOSSARY document was found. `Cargo.toml` declares `unsafe_code = "forbid"`, documentation and Clippy rules; tooling-enforced formatting/lints are excluded from this review. All twelve skill smell baselines were considered as judgment calls, subordinate to repository contracts.

- `tools.rs:14–46`, `lib.rs:921–950`: `ToolFailureEffect` gives the effect concept a type and places its guarantee on the actual `Tool` implementation. The shared error path uses `.map_or(...NoEffect, Tool::failure_effect)` only when registry lookup finds no implementation to dispatch. Registered custom, write, exec, HTTP, MCP and semantic tools inherit `Unknown`. No name-based capability assumption or new ownership release was introduced.
- `lib.rs:1130–1146`, `native_agent.rs:219–250`: both loops consume `failure_effect == Some(...Unknown)` after retaining the attempted record. Their distinct protocol framing remains local; the new classification is shared. The branch is independent of the progress transport. The existing completed-result size boundary remains separate.
- `files.rs:1312/1384/1676/1750`, `filesystem_info.rs:124/166`, and pure implementations/aliases in `tools.rs`: the `NoEffect` opt-ins were checked against read/list/search/stat implementations and their shared bounded I/O paths. The blocking closure still owns its permit through actual completion (`memory_io.rs:27–51`). Explicit implementation promises do not justify a new generic wrapper or tool-name map.
- `tests/native_result_limits.py` and CI/release hunks add the existing fixture's failure modes without changing resource ownership. The focused custom-tool test protects conservative defaults under a familiar builtin name.

Inherited session-catalog resource risk and channel/private-file/doctor-MCP duplication findings remain closed per the three prior reports and are not recounted. No builds, tests, runtime probes, source edits or external writes were performed by this reviewer. Spec compliance was not reviewed on this axis.
