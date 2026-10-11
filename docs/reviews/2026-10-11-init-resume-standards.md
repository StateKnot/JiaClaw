# Resumable init — independent Standards source review

Open actionable findings: **0 hard violations; 0 heuristic findings**.

Fixed cumulative command: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...1e357f30cde9aef69e95f23e208fac66e9de9db3`. Complete cumulative commit history was read. Fresh scope: `d5b71735e83019b27d8d9f48c871804533f9728b...1e357f30cde9aef69e95f23e208fac66e9de9db3`, five production, test and documentation paths. Prior cumulative/ModelProbe reviews are inherited; this is not an exhaustive cumulative rereview. Immutable Git objects were inspected, and the counterpart Spec report was not consulted.

Normative sources are user AGENTS production and df/cache requirements, `docs/memory-files.md`, `docs/init-resume.md`, and the existing Workspace/file contracts. No physical AGENTS, CODING_STANDARDS, CONTRIBUTING, GLOSSARY or ADR was found. Tool-enforced formatting/Clippy checks were excluded.

Hard standards: no breach found. `main.rs` removes the path-exists success shortcut and always delegates to `Workspace::init_with_overwrite(&workspace_path, force)`. That existing owner retains bounded create-if-absent, ordinary-file/link checks, directory capabilities and nonblocking writer locking; the CLI does not reproduce file publication logic or silently enable overwrite. Success is printed only after the initializer returns. The default path and explicit force behavior remain unchanged.

README, memory-file guidance and the new contract consistently distinguish preserving existing regular files from restoring missing defaults. They disclose single-file partial commits, require fixing the error before resuming, and state that `--path` does not update model configuration. The printed doctor/chat commands use the same explicit configuration; supplier, full first-use and durable execution remain unqualified.

The expanded existing fixture checks missing root/nested defaults, byte-exact preservation of other operator edits, invalid-root preservation, both modes’ leaf/skill-parent link refusal, existing force replacement and 0600 checks. Parent binary logs show the three original false-success cases; final repro logs show missing defaults restored and invalid targets exiting nonzero without outside changes. These are local real-CLI evidence, not parent PR118 or new-head official qualification; full related validation was still author-run at review.

All twelve Fowler heuristics were considered as possible judgments. Reusing the existing Workspace owner removes duplicated admission policy; no actionable naming, data ownership/type, switch, scattering, mixed responsibility, speculative abstraction, navigation, delegation or inheritance issue was established. Reviewer ran no builds, tests, network calls or cache cleanup.

# Resumable init candidate wiring — independent Standards supplement

Open actionable findings: **0 hard violations; 0 heuristic findings**.

Fixed incremental scope: `1e357f30cde9aef69e95f23e208fac66e9de9db3..9438cd65a55539e25cff7209731ea9166c1157f3`, exactly two hunks in `.github/workflows/release.yml`. Cumulative base remains `4b2357fcf01f47ba08d7724edbba7accfb60972c`; the five-path init source review and historical cumulative reviews are inherited, not exhaustively repeated. Immutable workflow and diff were read; the counterpart Spec report was not consulted.

Hard standards: no breach found against user production, actual verification, permission and cache standards or the init contract. The first hunk adds `tests/memory_io.py` to the pull-request qualification path filter. The second invokes that exact fixture with `target/$TARGET/release/jiaclaw` in the existing optimized-binary step. This step runs after locked release compilation for each of the four Linux/macOS native matrix targets, so candidate validation exercises the actual candidate rather than a debug binary or inherited parent evidence. Existing fail-fast commands, application/fixture deadlines, archive/install checks, read-only PR permissions and tag-only draft-release condition remain unchanged.

All twelve Fowler heuristics were considered as possible judgments. Adding the test through the existing single matrix and qualification step avoids scattered per-platform copies; the path filter and command serve distinct trigger/execution roles, not duplicated logic. No actionable naming, data ownership/type, switch, divergent responsibility, speculative hook, message-chain, middle-man or inheritance issue was established. Tool-enforced checks were excluded.

Local `process-checks.json` records memory_io/e2e/doctor/model_probe runs against the same frozen binary with all four exits zero; `checks-checks.json` records format and required Clippy exits zero. Those records are local debug-process verification only. This supplemental review does not claim a fresh full-Rust run, local optimized qualification, any official candidate completion, parent PR118 qualification, supplier certification or installation success. Production compiler inputs are unaffected by this workflow-only change. Reviewer performed no builds, tests, network calls or cache cleanup.
