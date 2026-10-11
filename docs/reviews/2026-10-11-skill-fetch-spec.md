# Independent Spec axis — initial final report

Pinned source: `31237b787749724fbaa21192b11de4adf12f7edb`; qualified fresh parent: `954c010a545539643260359f622f3fd045d21b44`; cumulative origin/main: `4b2357fcf01f47ba08d7724edbba7accfb60972c`.

Captured `git diff 4b2357fc...31237b7`, `git log 4b2357fc..31237b7 --oneline`, and `git diff 954c010...31237b7` in `/tmp/jiaclaw-oct11-fetch-spec-{cumulative.diff,commits.log,fresh.diff}`. Reviewed the explicit fresh 12 paths and shared contracts they invoke; qualified historical reviews inherited, without exhaustive cumulative rereview.

## Findings

Open findings: **0**. Severity: none. Missing/partial requirements: none identified. Unrequested scope: none identified. Implemented-but-wrong requirements: none identified.

Spec `docs/skill-fetch.md:4` requires “完整正文和声明参考核验”; `fetch.rs:131–136` reuses the existing strict Bundle parser, including body/resource hashes and limits (`install.rs:83–119`). Spec line 6 requires “原非排队 workspace writer” and one pinned validated DNS set; `fetch.rs:104–156,364–394,433–481` spans resolution, the one GET, byte validation and publication under the original writer, with TLS verification and explicit proxy/redirect/retry/decompression refusal. Lines 8–10 require stable UUID/intent/private capture/receipt, shared unsettled gates, original conditional finish and no repeated GET; `fetch.rs:110–125,199–343` and install/update/remove admission wiring match those constraints. Lines 4 and 12 require independent local installation: fetch only writes its private journal and returns `runtime_applied=false`.

A source-only wording ambiguity about IPv4-mapped IPv6 was clarified in root's delivery documentation; code correctly applies the IPv4 policy. No production source modification was reviewed or needed.

## Independent execution

`df` showed 237GiB free; no Rust build/cache copy/cleanup. Frozen binary `/tmp/jiaclaw-oct11-fetch-delivery/jiaclaw-final`, SHA256 `5a6ebe9e426c76ff367034a3f19eeacc456a5e4797eab55d6e01a878917a724e`.

Own `/tmp/jiaclaw-oct11-fetch-spec-probes.py` exit **0**: 22 public CLI executions, four actual locally trusted TLS GETs. Verified exact 512KiB byte capture, GET-free finish, preserved conflicting receipt, body/reference rejection, 307 refusal, no failed-UUID resend, each incomplete ledger blocking fetch, and incomplete fetch blocking valid install/update/uninstall without policy/file changes. Raw results: `/tmp/jiaclaw-oct11-fetch-spec-probes.log` and `.json`. Initial sandbox loopback-bind failure (exit 1) retained separately in `…-probes-sandbox-bind-failure.log`; authorized local-network rerun passed. Root's tests are not claimed as own execution; no public supplier/release/power-loss certification.
