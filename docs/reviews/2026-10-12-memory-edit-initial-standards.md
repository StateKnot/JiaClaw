# Standards — fixed source 410c35a

Fresh scope: all 16 paths in `git diff 91b7b3c365c7806bd5d9d821e1677af7a714b3bc...410c35aa5559118d898a93027a176e3455fd68e6`; the corresponding log contains one feature commit. The cumulative `4b2357fc...410c35a` diff/log were used only to locate inherited interfaces, not to exhaustively re-review historical changes. I did not read the Spec report. Sources: user `/Users/jiawy/.codex/AGENTS.md`, memory correction/review/edit/file contracts, configuration/gateway permissions and Cargo lints. No repository coding-standard files exist; tooling-enforced lint/style rules were skipped.

## Documented rule violation

- **[P2] Respect configured body limits for chunked replacement requests.** `crates/jiaclaw-host/src/memory_review.rs:91` captures raw `Request` with `to_bytes(request.into_body(), 256 * 1024)`. This bypasses the extractor-only global `DefaultBodyLimit`; the existing middleware checks only Content-Length. `docs/configuration.md:51` requires `http.max_body_bytes` overages to return 413; `docs/memory-edit.md:22` expressly retains that global limit. Independent actual frozen-binary probe configured 512 bytes: identical 1242-byte JSON returned 413 and preserved the original with Content-Length, but returned 200 and published 1024 bytes when chunked. Apply the minimum of configured/global and endpoint budgets to the actual stream, with a no-Content-Length regression. Evidence: `/tmp/jiaclaw-oct11-memory-edit-standards-chunked-evidence.json` and `.log`; expected-413 assertion exited 1, disposable server settled exit 0.

## Baseline heuristic

- **[P3] Possible Duplicated Code in public response contracts.** Four new OpenAPI response blocks repeat `"409": {"description":"Replacement unconfirmed; X-JiaClaw-Write-Review required..."}` at `openapi.json:3294/3321/3348/3375`, including read-only capability GETs that never write. A missing-memory GET actually returned 422, absent from its block; POST also omits its distinct 400/408/413/415/422 outcomes. Describe responses per operation and share only genuinely identical components, preventing future contract drift.

The other eleven baseline smells were judged against these hunks without actionable findings. Writer/control permits move into the real blocking closure, write authorization requires independent opt-in, conservative unknown results prohibit replay, and C injection remains test-only. These observations do not certify providers, physical durability or complete milestones. No repository/Git/PR mutation or build/cache creation.

Standards: **1 documented violation, 1 heuristic; worst P2**. Spec: deliberately not assessed here.
