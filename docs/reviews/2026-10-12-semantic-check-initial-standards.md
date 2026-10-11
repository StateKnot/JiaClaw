# Standards — semantic check source

Fixed source: `5d0e23c6b7f85bea02ae1905801fbfc28ad7a270`; fresh parent: `c13885c69d5055f752f839d74588c326e9d8ec66`. Both resolve. Captured commands: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...5d0e23c` and `git diff c13885c69d5055f752f839d74588c326e9d8ec66...5d0e23c`; the fresh commit list is `5d0e23c feat: check semantic source freshness without network requests`. Fresh review covers all ten changed paths. Historical PR107–128 conclusions are inherited; this is not an exhaustive cumulative re-review.

Sources: user AGENTS instructions and `/Users/jiawy/.codex/AGENTS.md`; `docs/architecture.md`, `docs/workspace-files.md`, `docs/memory-files.md`, `docs/semantic-memory.md`, `docs/semantic-check.md`; Matt Pocock code-review and codebase-design skills. Repository inventory found no AGENTS, CONTRIBUTING, CODING_STANDARDS, GLOSSARY or memory-io file.

## Documented violations

**0.** The directory-handle rule in `docs/workspace-files.md` (“不在检查后重新用宿主绝对路径打开文件”) is preserved by `memory_io.rs::read_text_at`: checked parents and leaves remain relative to the retained root capability. `semantic/mod.rs::check` retains the existing admission, persistence worker, Store owner and workspace writer through capture, validation and result construction. The hold-preservation and explicit-refresh rules in `docs/semantic-memory.md` remain intact. Host wiring adds only the explicit-config local command; the test interposer is confined to disposable fixture processes. CI and release verification consume the same public command.

## Possible baseline smells

**0 actionable judgement calls.** Applied all twelve: Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. Private `capture` has two actual read adapters; shared `generation_current` centralizes the search/check corruption rules. Existing JSON and ownership conventions override speculative type or executor layering suggestions. No unrelated production reasons or hypothetical public seam were added.

Tool-enforced matters are excluded. This axis is static source review; test/build qualification belongs to the root’s separately captured execution evidence. Spec-axis reports were not consulted.

Standards total: **0 documented violations / 0 actionable heuristics**; worst issue: **none**.
