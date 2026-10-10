# Failed tool attempts and unknown effects

A failed or timed-out tool call does not prove that it did nothing. The native Brokerrouter loop and existing development text-tool loop stop the remaining batch and model rounds when an attempt has unknown effects. They retain the attempted tool record and return `RequiresHumanInput`, independently of the presence of a streaming progress transport. The JSON/CLI tool error includes `effect_status: "unknown"`; compatibility SSE retains its existing tool summary and final review status.

`Tool::failure_effect()` is an implementation guarantee. Its default is `Unknown`, including custom tools, file writes/copy/delete/move, controlled exec, HTTP requests, external MCP and semantic memory. A refusal before submission may also conservatively report unknown; the application does not infer no-effect from an error string, timeout, tool name, model claim or MCP annotation. This avoids allowing a later model request to duplicate a possibly completed write or billable external call.

Only trusted local tools whose entire implementation never writes workspace data or submits external requests opt into `NoEffect`: datetime/json query, workspace listing, memory reading, bounded file reads/listing/grep/glob/stat/tree, and their registered read/list aliases. Their errors include `effect_status: "no_effect"` and can be sent back to the model for correction. A custom tool registered under the same name retains the conservative default unless its implementation explicitly promises otherwise. Keyword/semantic memory search shares one implementation and therefore remains unknown even for a keyword-mode error.

Successful tools still return their existing results. The separate 256 KiB completed-result-loss boundary continues to stop dispatch. A normally returned nonzero exec exit code is a known process result and retains the existing API; an exec timeout, transport or cleanup error is unknown. This policy is not a cross-tool transaction and does not undo earlier writes, settle external billing, release real blocking-work capacity early, persist a resumable execution ledger or authorize replay. Operators must inspect the original workspace, model receipt and any external service before starting another request.

Failed error details exceeding 256 KiB are replaced by a bounded failed-error record retaining `effect_status`; they never become a claim of successful completion. Lost details require review even for a trusted `NoEffect` failure. An actual native-loop custom-tool regression writes once before returning a 300,000-byte error, checks the retained unknown marker and verifies that the pending tool/model round is not dispatched.

## Verification

The frozen PR108 binary wrote once before an actual non-root/network-disabled Docker command timeout, then continued the same batch or another model round. A repeated model call caused two writes and a `completed` result. The reproduction and original failure log are saved outside the build cache; this is an application dispatch defect, not a new framework issue.

```sh
python3 tests/native_result_limits.py target/debug/jiaclaw --tool-errors
python3 tests/native_result_limits.py target/debug/jiaclaw --tool-errors --legacy
JIACLAW_TEST_DOCKER=/absolute/path/to/docker \
JIACLAW_TEST_EXEC_IMAGE=repository@sha256:fixed_digest \
  python3 tests/native_result_limits.py target/debug/jiaclaw --exec --tool-errors
```

Each failure mode covers ordinary HTTP JSON, compatibility SSE, CLI, same-batch and next-round interruption, persisted history, an unchanged successful control and a recoverable local pure-error control. The Docker mode verifies a real write followed by timeout, one effect, no pending effect and one model submission; it neither pulls an unpinned image nor substitutes host execution. `--legacy` covers the existing development provider and does not make it the recommended production route. CI requires both local failure suites and Linux Docker versions; four optimized release candidates each run both local suites. Final fixed-head official evidence is recorded on the delivery PR/pin independently of parent qualification.

Independent Spec review found that the initial implementation mislabeled an oversized failed error as completed and discarded its unknown marker. The regression first failed on the unchanged implementation; the bounded replacement now preserves the failed origin and classification. This P2 correction is separate from the timeout dispatch defect.

An initial fixture assertion incorrectly expected the compatibility SSE summary to contain the full result's metadata. Its failure is kept separately; the assertion now checks the actual summary error and final status, with the production binary unchanged. The first local full Rust run could not bind localhost inside the restricted sandbox and is distinct from the explicitly authorized fixture-enabled run. Real supplier effects, durable recovery and real channel installations remain separately unqualified.

## File boundary fixture integration

PR109's first fixed head `fd4f98f` failed the Ubuntu and macOS CI jobs at `workspace_files.py`: the existing rejection helper still required a `completed` status and two model submissions after a failed mutation. The same frozen final binary reproduced this expectation failure in `workspace_files`, `workspace_mutations` and `workspace_copy`. This is a regression-fixture integration omission; their observed `RequiresHumanInput`/one-submission behavior matches the implemented conservative contract above.

The three fixtures now share a small wire-assertion function. Failed mutations require a retained error and `effect_status: "unknown"`, one model submission and `RequiresHumanInput`. Only the file-read/list fixture's four trusted read/list names expect `no_effect` and normal model feedback. Successful batches retain their complete records, two submissions and JSON-object results. Each formerly grouped rejected boundary receives its own explicit request so that stopping the first failure does not silently skip later link, traversal, inode, path, lock or capacity checks. The original disk-state assertions remain intact.

Independent [Standards](reviews/2026-10-10-tool-failure-fixtures-standards.md) and [Spec](reviews/2026-10-10-tool-failure-fixtures-spec.md) reviews have zero open incremental findings. The initial Standards P3 duplication heuristic is closed by the shared assertion function; initial reports remain separate. Their cumulative base stays fixed at `origin/main`, with earlier coverage inherited rather than claiming an exhaustive new audit.

The unchanged frozen production binary passed all 43 mandatory process-suite commands with the initial fixture correction, then passed the three affected suites again after the shared-helper revision; the other 40 commands' fixture bytes are unchanged. The complete matrix and subsequent three-suite results remain separate artifacts. All 93 compiler inputs remain byte-identical to final production source `316911bb`; no Rust rebuild or full Rust rerun was performed for this fixture-only correction. Old-head CI failures and partial candidate successes remain separate from the new head's required seven-job qualification. Final fixed-head CI, Chromium, real Docker and optimized candidate evidence belongs in the PR body/pin; no pure documentation commit is added merely to cite its own newly generated head.

The old head's macOS Intel optimized candidate was separately cancelled at the 40-minute job limit; GitHub's check annotation explicitly reports that limit. Unit tests took 20m50s and optimized compilation 16m49s, leaving approximately two minutes for all optimized process, archive and installation acceptance. Only that matrix entry now receives a 60-minute job budget; the other three retain 40 minutes. No acceptance command, runtime/fixture deadline or release approval is removed or relaxed. This budget correction requires a fresh complete Intel candidate; the cancelled run supplies no archive/installation qualification.
