## Standards

Fixed cumulative baseline: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...2ef1c951646688ac6543aa3cc3b76c23be5ae4f6`; corresponding cumulative commit list inspected. Historical Standards conclusions through PR107–121 are inherited at their qualified heads. Fresh detailed review covers only `git diff 6a8da1fb62eacc07f925760a8a6c4f84847442ec...2ef1c951646688ac6543aa3cc3b76c23be5ae4f6`, commit `2ef1c95`, across eleven changed paths and their existing filesystem/discovery callees. This is not an exhaustive cumulative rereview. No Spec-axis report was read.

Sources: user-provided AGENTS production/cache instructions; `docs/architecture.md:55–59`; `docs/workspace-files.md:165–171`; `docs/memory-files.md:44–48`; established skill policy contracts. No tracked AGENTS, CONTRIBUTING, CODING_STANDARDS, GLOSSARY or ADR was found. `Cargo.toml:55` forbids unsafe Rust and CI runs `cargo fmt`; tooling-enforced matters are excluded.

**Documented-standard breaches: 0. Actionable Fowler heuristic findings: 0.**

`memory_io.rs:541–554` keeps the original nonqueueing workspace writer around the complete operation. `skills/install.rs:178–238,298–356` retains that owner through captured inputs, complete candidate validation, intent, stage, policy, atomic directory publication and receipt. Capability-relative parents/readers and the existing safe NOREPLACE wrapper remain shared with the filesystem owner; publication never falls back to check-and-overwrite. Snapshot leaves use rename-without-replacement to avoid crash-surviving hard-link aliases. Related parents are synchronized and post-publication failure remains an error requiring original-identity inspection.

`Bundle::parse`, shared `Skill::parse_raw`, capture integrity validation and exact/subset tree verification keep the reviewed domain checks together. The new module has one installation/recovery reason to change, without a second lock owner or speculative install framework. String observation phases follow existing receipt conventions; their bounded construction and identity comparison do not justify a new actionable Primitive Obsession finding. All twelve Fowler heuristics were considered as judgments, not hard rules.

Rust tests and the CLI fixture exercise production interfaces, real filesystem ownership, explicit finish and actual OS sync/SIGKILL boundaries. The C interposer stays outside production and CI/release invoke the same fixture on their candidate binaries.

Static source review only: no builds/tests, source edits, network publication, cache copying or cleanup. Initial report retained unchanged.

Standards total: **0 unresolved documented breaches; 0 unresolved heuristic findings; no worst issue.**
