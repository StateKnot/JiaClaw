# Standards — initial tenant settlement fixture repair

Fixed source `d4901c4fe6d8f1fbbd831ff33751d6b45e1de915`, tree `8fa075c47b24b44aff284dcab504bfb0ac24072c`. Fresh `git diff f19732dc...d4901c4`:five paths; captured commits479cc19 persistent old-order RED and d4901c4 ordered observation. Cumulative4b, production/source/delivery/publication conclusions are inherited, not exhaustively re-reviewed.

Applied user/global AGENTS, README/Cargo documented lints, architecture, native-tool/IO/tenant/model-call/release contracts and Matt code-review/codebase-design. Repo rules override the complete12-smell heuristic baseline; tooling-enforced issues excluded. Spec findings were not read.

**Documented violations:1 (P3 release qualification wiring).** The new `tests/tenant_file_gateway.py` is a real dependency of optimized acceptance via `tests/tenant_file_read.py:17`, but `.github/workflows/release.yml`’s explicit pull_request.paths omits it while enumerating the consuming file and existing shared helpers. `docs/release-candidates.md:3` documents qualification for acceptance changes. A later helper-only change could therefore skip all four candidates. Register this actual helper path; CI itself has no such filter. The present multi-file patch does trigger qualification, so this is a dependency-wiring omission, not evidence that current jobs were skipped or production broke.

**Other actionable heuristics:0.** ReceiptRelay contains actual loopback/observation behavior behind a small test Interface. The original hold query,15s deadline, complete results/tool scope/two POSTs, cross-user/readOnly and eight-reader checks remain; the successful observation is reordered before adding a client GET, and an absolute return guard makes the deadline stricter. Relay retirement precedes other/browser matrices. No production hook, hold clearing, retry or provider capability is introduced.

The original ARM wire/hold evidence is absent; controlled overlapping503 RED and ordered GREEN demonstrate one mechanism without proving its historical uniqueness. Unchanged frozen/local success and failed original head remain separate. No Rust/build/optimized/provider or product execution occurred in this review.

All12 smells considered: Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man, Refused Bequest.

Standards total:1; worst:P3 actual helper missing from release trigger filter. Initial raw retained for independent closure.
