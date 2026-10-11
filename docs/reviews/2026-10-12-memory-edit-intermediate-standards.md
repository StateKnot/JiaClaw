# Standards — revised fixed source 19737f6

Fresh review covers all 16 paths in `git diff 91b7b3c365c7806bd5d9d821e1677af7a714b3bc...19737f6df09e39f3605b26d7a5a11a3153a0e9d9` and both commits. Cumulative `4b2357fc` history supplies inherited interfaces only, not exhaustive cumulative re-review. Standards sources remain user AGENTS, configuration/gateway and memory correction/review/edit/file contracts; tooling-enforced Cargo rules were skipped. The initial report is preserved unchanged. No Spec report was read.

## Documented violation still open

- **[P2] Use the resolved global HTTP budget, including environment overrides.** Revised `crates/jiaclaw-host/src/memory_review.rs:92` reads raw `state.agent.config().http.max_body_bytes`, while the router/middleware consumes `effective_max_body_bytes()`. `jiaclaw-core/src/lib.rs:1708–1718` documents `JIACLAW_MAX_BODY_BYTES` priority and fallback; `docs/configuration.md:51` requires overages to return 413 and `docs/memory-edit.md:22` retains the global limit for actual chunked reads. Independent frozen `95f212...41b1c` probe: configured 1MiB plus environment 512 bytes, identical 1242-byte JSON returned 413 with Content-Length, but chunked returned 200 and published 1024 bytes. Expected-413 assertion actually exited 1; disposable server settled 0. Consume the same resolved budget, not the raw field. Evidence: `/tmp/jiaclaw-oct11-memory-edit-final-standards-env-evidence.json`, `.log`, `.py`.

## Closed earlier conditions

The original configured-512-byte reproduction was independently re-executed with unchanged request/expected-status assertions against the revised binary: both transports returned 413, original three bytes remained, probe/server exited 0. Evidence: `memory-edit-final-standards-chunked-evidence.json` in `/tmp`.

The initial **P3 possible Duplicated Code** is closed: all four OpenAPI operation response tables now match their distinct handler branches; capabilities no longer describe write uncertainty, GET lists 422/500/503, and POST lists its 400/408/413/415/422 outcomes. Static evidence: `/tmp/jiaclaw-oct11-memory-edit-final-standards-openapi-evidence.json`.

All twelve smell baselines were reconsidered with no additional actionable heuristic. Real blocking ownership, independent write gates, conservative unknown handling and test-only C injection remain intact. No build, target copy, supplier call or repository/Git/PR change; df was checked before both actual local probes.

Standards: **1 open documented P2, 0 open heuristics; worst P2**. Original P2 configuration-only case and P3 are closed; environment-budget coverage remains incomplete. Spec deliberately not assessed.
