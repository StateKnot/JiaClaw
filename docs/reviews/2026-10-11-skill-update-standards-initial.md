## Standards

Fixed cumulative comparison: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...d5864985480f03fe0d1ff2ad1086705b9bac4bf8`; `git log 4b2357fcf01f47ba08d7724edbba7accfb60972c..d5864985480f03fe0d1ff2ad1086705b9bac4bf8 --oneline` read completely. The base resolves, is the merge-base, and the diff is nonempty. Prior cumulative Standards reports through installation are inherited within their original scopes. Fresh independent review covers all ten paths in `048151a2733d3423d0e4e3a736258a40c16bb3d6..d5864985480f03fe0d1ff2ad1086705b9bac4bf8`; this is not an exhaustive cumulative rereview. No Spec report was read.

Standards sources: user AGENTS production/cache requirements; `docs/architecture.md:55–59`, `docs/workspace-files.md:165–171`, `docs/memory-files.md:44–48`, and existing skill source/policy/install contracts. Repository searches found no AGENTS/CONTRIBUTING/CODING_STANDARDS/GLOSSARY/ADR. All twelve Fowler heuristics were considered as judgments, repository rules override them, and tooling-enforced issues are excluded.

**Documented-standard breaches: 0. Actionable heuristic findings: 0.**

`skills/install/update.rs:117–192` retains the original nonqueueing workspace writer across reviewed old/new content capture, candidate validation, private UUID evidence and publication. Existing capability readers, private-file checks, package budgets, staging and sync helpers remain with the storage/package owner. `DirectoryIdentity` bundles dev/ino; typed `SkillUpdateState` and `Bundle::file_text` avoid primitive state and duplicated package comparison logic. Distinct install/update ledgers reflect different publication and recovery semantics without a hypothetical shared framework.

`update.rs:380–455,493–526` validates saved hashes, complete trees and original directory identities before finish mutates anything. `finish_owned:250–299` performs one EXCHANGE, recognizes an already exchanged original operation, retains the old directory, synchronizes actual parents and writes the receipt. Historical completion intentionally does not certify the current runtime. CLI branches initialize only configuration and the updater; no model/HTTP write authority, automatic activation/reload or replacement lifecycle owner is added.

The actual CLI fixture exercises six OS sync/SIGKILL stages, held-owner refusal, post-exchange EIO, equal-byte versions, conflict evidence, cross-ledger blocking and authenticated reload/GET. The interposer stays test-only; CI and optimized candidate workflows invoke this fixture directly. Independently read seven update PASS groups and the installation regression log; reviewer did not execute tests/builds or edit production source. Source identity, ongoing whole-batch checks and new-head official qualification remain separate evidence.

Standards total: **0 unresolved breaches; 0 actionable heuristics; no worst issue.**
