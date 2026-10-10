# Shared workspace assertions — final independent Spec review

Fixed cumulative base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`; reviewed head: `70acdcc3e4eeb93d89e9438665a752167ad39fbb`. Cumulative comparison remains `4b2357fc...70acdcc`; fresh focus is complete `git show 70acdcc` against parent `44f3c37`. Production source remains `316911bb`. Earlier source/document conclusions and the closed oversized-error P2 are inherited, without claiming exhaustive cumulative review. The initial fixture report remains preserved at `docs/reviews/2026-10-10-tool-failure-fixtures-spec-initial.md`.

**0 open incremental Spec findings:** missing/partial requirements: 0; unrequested scope: 0; incorrect implementation: 0.

The shared `tests/tool_batches.py:5–28` preserves the previously reviewed wire assertions: each rejected mutation requires HTTP 200, `RequiresHumanInput`, one tool record and one model submission, an error and `effect_status: "unknown"`. This matches `docs/tool-failure-effects.md:3`: unknown attempts “stop the remaining batch and model rounds.” Multiple rejected inputs still invoke distinct explicit `Host.chat` requests, ensuring every boundary executes instead of being skipped after the first failure.

Only `workspace_files` supplies the same four actual read/list implementations and aliases as `pure_errors`. Their rejected calls still require completed/two-submission behavior and `no_effect`, matching spec line 7: pure errors “can be sent back to the model for correction.” Mutation/copy hosts pass no pure list. No production tool-name heuristic or no-effect inference was introduced.

The three `Host.batch` methods only delegate. All fixture model behavior, boundary inputs, disk/inode/sentinel assertions, exact capacities, long-path checks, lock/release controls, staging cleanup and disabled-tool direct-chat authorization tests remain unchanged. `docs/workspace-files.md:165` still requires “锁忙立即报错，不自动重试”; both classification and explicit post-release requests remain checked. Successful batches retain completed status, full record counts, two model submissions and JSON-object results.

The three shared-fixture final logs were independently read and contain all original 4/6/7 passing groups. This reviewer ran no builds or tests and modified no project files. The outstanding full 43-suite/current-head and official matrices are not certified here; unaffected production qualification remains separate from this fixture-only change.
