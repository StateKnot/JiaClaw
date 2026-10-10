# Tool Failure CI Fixture Fix — Final Independent Standards Review

Date: 2026-10-10. Fixed cumulative base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`; final head: `70acdcc3e4eeb93d89e9438665a752167ad39fbb`. Comparison: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...70acdcc3e4eeb93d89e9438665a752167ad39fbb`, 103 commits/226 files. Fresh focus: complete `git show 70acdcc` (parent `44f3c37`) and all three actual fixture consumers. Earlier source/document conclusions are inherited; this is not an exhaustive re-audit of 114k cumulative lines. The initial P3 report remains preserved at `docs/reviews/2026-10-10-tool-failure-fixtures-standards-initial.md`.

## Documented standards

**0 new hard violations; 0 unresolved hard violations.** The user's “都需要生产级可用的方案” rule applies; no new build/test execution invokes its cache requirements. Cargo/tool-enforced lint and formatting rules are excluded. Production code, host/model ownership and actual filesystem boundaries are unchanged.

## Judgment heuristics

**Initial P3 possible Duplicated Code closed; 0 new actionable heuristics; 0 unresolved Standards findings.**

`tests/tool_batches.py:5–28` now owns the actual shared shape: `assert_tool_batch(chat, calls, rejected=False, pure_errors=())` admits multiple rejected cases as separate calls, then asserts status, model count, record names and typed wire effect metadata in one place. It retains successful-result parsing and returns each case's output. The callback is the existing bound `Host.chat`, so each case still receives its fixture's fresh session and real model observations.

`workspace_files.py:180–182` explicitly passes the four trusted read/list names; `workspace_mutations.py:176–177` and `workspace_copy.py:182–183` use the conservative empty default. These are fixture expectations, not a production name-based capability classifier. Their retained `Host.batch` interfaces preserve existing domain calls; no new hierarchy or whole-Host abstraction was added. Model validation, disabled-authority checks, disk bytes/inodes, locks and staging checks remain in the individual suites.

All twelve baseline smells were considered as judgment calls; the minimal shared function resolves the demonstrated three-consumer synchronization issue without speculative generality. The three supplied `shared-fixture-final-*` logs contain their expected PASS lines and no displayed failure; this reviewer did not execute them or certify the still-running complete 43-suite/official candidate qualification. No builds, tests or project edits were performed; Spec implementation was not reviewed on this axis.
