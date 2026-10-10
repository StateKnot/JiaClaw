# Doctor Readiness — Spec Review

- Fixed cumulative base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`.
- Cumulative source head: `2418d8b57b391ce0f149191b62e55c5593e9eb08`; 94 commits, 216 files in the three-dot comparison. Inherited findings remain tracked in [the prior architecture/code/product review](2026-10-10-architecture-code-product-review.md); this report is the focused Spec result for `docs/doctor.md` and the new readiness behavior.
- Spec: [`docs/doctor.md`](../doctor.md), plus the confirmed false-success reproduction in `/tmp/jiaclaw-oct10-doctor-repro.py` and `/tmp/jiaclaw-oct10-doctor-initial.log`.

## Spec

Final focused result: **0 open findings**.

The review found and verified closure of three cases against the stated contract:

1. A syntactically invalid or disallowed provider URL could display an invalid summary while still exiting successfully. Doctor now uses the runtime’s HTTPS/literal-loopback HTTP endpoint policy and treats rejection as provider-not-ready.
2. The original `--connect` coverage did not prove enabled private stores were opened or that initialization avoided model, embedding, and tool calls. The real-binary fixture now verifies both SQLite stores are created, no provider requests occur during startup, and an opted-in MCP discovery failure sends only the expected discovery request without `tools/call`.
3. An empty MCP bearer environment variable passed the offline readiness check even though connection authorization rejected it. Doctor now checks the same `ApiKey` contract through a shared credential helper; the real-binary fixture verifies a nonzero exit and no credential/variable disclosure.

Explicit `stub` remains the only path that reports stub mode. Default checks remain offline and do not open private stores. `--connect` is an explicit opt-in and does not send model/embedding requests or invoke tools.

## Verification scope

This is the independent Spec axis, not the Standards report. Findings above were resolved and rechecked; inherited cumulative code is not represented as freshly re-reviewed. No paid supplier, production credential, or live external MCP service was used.
