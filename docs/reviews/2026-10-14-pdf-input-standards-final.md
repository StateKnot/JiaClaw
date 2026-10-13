## Standards

**PASS — 0 open documented-standard violations, 0 actionable smell findings. Both initial P2s are closed.** Fresh scope is exactly `git diff abffbf3e…...e58dec18…`, tree `eb507777…`, across21 paths and two commits. The new fixture JSON accounts for the additional path. User AGENTS and native-tool/failure-effect/MCP/text-input/PDF-input policies govern; Fowler smells remain repository-overridden heuristics, excluding tooling-enforced checks.

`ui/pdf-input.js:39–44` now awaits `doc.getMetadata()` and immediately checks the same original deadline/abort owner. Metadata must provide boolean `IsXFAPresent` and null/string `EncryptFilterName`; malformed shape fails closed. Every nonnull encryption filter and every XFA flag is rejected before page extraction, independently of password challenge or `enableXfa:false`. This satisfies the whole-refusal rule in `docs/pdf-task-input.md:8` and closes both findings without modifying upstream SDK bytes.

All original controls remain: one original FileReader capture, strict text UTF-8/control/16KiB behavior, explicit original Send/tool authorization, identity/session/draft generation guards, one native Worker port with no fake-worker fallback, real termination and late-result refusal,1MiB/32-page/16KiB extraction/32KiB draft budgets and one5-second absolute deadline. Fixed local assets load only after explicit import; Worker CSP blocks external data. No backend upload, second reading engine, Store, model tool or replay path was introduced.

Six hash-bound real PDF fixtures are present and wired into all three original wrappers: empty-password and password-required encryption, hybrid XFA, Chinese text, exact16384-byte extraction and16385-byte refusal. Earlier assertions and limits remain intact; the release path filter includes the fixture asset.

SDK/worker/LICENSE still exactly match the three official ZIP members authenticated in my initial independent Mozilla metadata acquisition, archive SHA `f2245eb7…`. This verifies provenance, not exhaustive upstream correctness. I independently matched all112 compiler-input hashes at the final pinned source and frozen binary SHA `89d89b0b…`; those bindings do not substitute for runtime execution.

Initial two-finding report remains unchanged. Parent reports final three PDF wrappers passed; I performed no product tests/builds or mutations. Fresh official new-head qualification remains separate. Historical PR140, cumulative `4b2357fc` and local1318 conclusions are inherited only; no unique initial timeout cause is claimed.
