# Standards — tenant file reads, initial source

Fixed source `93849d8e5134cd817c1c1324910810925a122fc2`. Fresh command: `git diff 0b9fa0410a2734873b3898b3735ff197deb87e9d...93849d8e5134cd817c1c1324910810925a122fc2`; commit `93849d8 feat(tenant): grant explicit scoped document reads to tracked HTTP turns`;22 paths. Cumulative baseline `4b2357fcf01f47ba08d7724edbba7accfb60972c` resolves; previous source/PR130 reviews are inherited, not exhaustively repeated.

Standards sources: user AGENTS, `/Users/jiawy/.codex/AGENTS.md`, README, Cargo documented lints, architecture, workspace-files, native-tools, tool-failure-effects, gateway and tenant HTTP/file contracts. Inventory finds no AGENTS/CODING_STANDARDS/CONTRIBUTING/GLOSSARY file in this repository. Matt code-review/codebase-design applied; tooling-enforced concerns excluded.

**Documented violations:1 (P3).** `docs/tenant-file-read.md:95–96` claims “tool timeout/abandonment is classified unknown and stops future dispatch”; `tests/tenant_file_read.py:222` likewise expects the held pure read to end needs_review after one model POST. This contradicts `docs/tool-failure-effects.md:7` and `docs/native-tools.md`’s effect contract: trusted local bounded reads retain NoEffect, so their errors may be returned for model correction. At this exact source, `FileReadTool::failure_effect` returns NoEffect (`crates/jiaclaw/src/tools.rs:381–383`), and `execute_and_record` uses that implementation classification for timeout errors (`crates/jiaclaw/src/lib.rs:1021–1030`). Correct the documentation and fixture expectation to distinguish pure-read timeout feedback from outer turn cancellation/unknown submitted model settlement; retain the actual blocking I/O owner and slot assertions. This is documentation/acceptance mismatch, not a production timeout or upstream defect.

**Actionable heuristics:0.** Canonical policy/capability types centralize grant validation and hashing; the restricted alias adds permission checks around the existing reader instead of another file engine. Gateway, backend and page enforce distinct admission/wire responsibilities. Explicit opt-in and historical lookup-only identities are maintained; no supplier, business-summary quality, durable or Agent-browser qualification is claimed.

All12 smells considered: Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man, Refused Bequest. Repo rules override these heuristics.

Static review only; no build, product execution or repository mutation. Root’s initial Rust/process evidence is not independently rerun or converted to official qualification. Spec findings were not read.

Standards total:1; worst:P3 documented effect-contract wording/fixture inconsistency.
