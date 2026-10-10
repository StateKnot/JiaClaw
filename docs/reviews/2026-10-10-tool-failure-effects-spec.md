# Tool failure effects — final independent Spec review

Fixed cumulative base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`; reviewed final source head: `316911bbf818fc6561b92fed339636fe22152530`. Focused implementation: `4434679...316911bb`; correction recheck: `357f86c...316911bb`. The architecture/doctor review conclusions remain inherited; this is not a fresh exhaustive review of all cumulative code. The initial independent report is preserved at `docs/reviews/2026-10-10-tool-failure-effects-spec-initial.md`.

## Spec

**0 open focused findings:** missing/partial requirements: 0; unrequested behavior: 0; incorrectly implemented requirements: 0. **Initial P2 closed in the reviewed source.**

Spec `docs/tool-failure-effects.md:3`: “The JSON/CLI tool error includes `effect_status: "unknown"`.” Line 11: “Failed error details exceeding 256 KiB are replaced by a bounded failed-error record retaining `effect_status`; they never become a claim of successful completion.” `native_agent.rs:225–236` now selects the replacement by `failure_effect`: failed results retain the original classification and say `tool failed`; successful oversized results retain the prior completed-result-loss message. Both paths stop dispatch and preserve the attempted record.

The new `lib.rs` regression runs the actual native loop with a custom tool registered as `file_read`, writes an append marker before returning a 300,000-byte error, and supplies a pending same-batch call. It checks `RequiresHumanInput`, exactly one write/record, bounded failed-error text and retained `unknown`, while the uniquely authenticated model mock expects one submission. The frozen pre-correction execution failed specifically with `Null` versus `"unknown"`, consistent with the original defect.

The earlier focused source inspection remains valid: native/text loops stop unknown effects independently of progress; trusted local pure implementations explicitly opt into `NoEffect`; custom/write/exec/HTTP/MCP/semantic defaults remain conservative; successful results and known nonzero exec returns retain existing semantics. No new durable/replay or supplier qualification claim is introduced.

## Evidence limits

This reviewer performed source, fixture and initial failure-log inspection only; no build or test execution. Final Rust and process-suite execution was still running when this source report was written, so the report does not certify its success. Fixed-head CI/release qualification and generic external effect certainty remain separate acceptance work under the roadmap.
