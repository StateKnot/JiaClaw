# Doctor Capability — Standards Review

Fixed cumulative base: `4b2357fcf01f47ba08d7724edbba7accfb60972c`; source head: `b4291089175f088229fd6619e78005608f80cd6b` (97 commits). Focused increment: `git diff 48830471f135bb7486abfbd74184cfd0e396b95b...b4291089175f088229fd6619e78005608f80cd6b` (three files, 50 insertions/6 deletions). Final follow-up reviewed separately: `9e22620b...b4291089`, seven insertions/one deletion in `tests/doctor.py` only.

## Standards

**0 new hard violations; 0 actionable baseline smell heuristics.**

- `crates/jiaclaw-host/src/main.rs:5118–5145`: the added `provider_type == "stub"` branch correctly reports that explicit stub mode needs no key. The Brokerrouter label now describes local configuration validation, and its endpoint success message is conditional on `provider_ready`. This follows `docs/doctor.md`'s truthful configured-path and invalid-endpoint reporting contract.
- `crates/jiaclaw-host/src/main.rs:5374–5389`: HTTP MCP integration, configured server count, unintegrated durable execution, and configured SQLite/in-memory sessions are distinct. The hunk explicitly states `doctor 不打开会话库` and that availability/recovery remain unverified. No new store-opening path is introduced; the surrounding `--connect` path still initializes Agent private stores only. This follows `docs/doctor.md`'s read-only default and session-storage boundary.
- `docs/doctor.md:25–33` and `tests/doctor.py`: the documented capability boundaries match the changed output; assertions cover contradictory endpoint success, stub key guidance, MCP count, durable status, and absence of the serve-only session database. The final fixture correction now checks default SQLite, consistent with the `HttpConfig.persist` field contract (`crates/jiaclaw-core/src/lib.rs:1484–1486`, default true), then performs another CLI invocation with explicit `persist = false` to check memory mode. No production code changed in that follow-up, and no actionable smell arose.

Inherited cumulative findings are carried from `docs/reviews/2026-10-10-architecture-code-product-review.md` and `docs/reviews/2026-10-10-doctor-readiness-standards.md`: session-catalog resource risk was closed in PR #103; channel admission and private-file duplication heuristics were closed in PR #105/#106; the doctor/MCP credential duplication heuristic was closed by shared `bearer_token()`. These remain closed and are not recounted.

Scope: complete focused delta and relevant call-path context, using the user's production-readiness requirement and `docs/doctor.md`; lint/format issues were excluded. No tests or compilation were executed by this reviewer; the supplied final doctor log was inspected and contains both PASS lines. Initial frozen old-binary RED evidence remains separate. No source edits or fresh exhaustive audit of the inherited >100k lines.
