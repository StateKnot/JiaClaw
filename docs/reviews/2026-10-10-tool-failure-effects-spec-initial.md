# Tool failure effects — independent Spec review

Fixed cumulative base: `4b2357fcf01f47ba08d7724edbba7accfb60972c`; reviewed head: `357f86c4ec17220df74d8972ef60363c1edf1d39`. Focus: `4434679...357f86c`, with the cumulative commit/file map and inherited architecture/doctor Spec reports retained. This does not claim a fresh exhaustive review of the inherited implementation.

## Spec

**1 open focused finding:** missing/partial requirements: 0; unrequested behavior: 0; incorrectly implemented requirements: 1.

- **[P2] Preserve unknown-effect metadata when a failed error exceeds the result limit.** `crates/jiaclaw/src/native_agent.rs:225–232` replaces every oversized serialized record with `"tool completed but result exceeds 256 KiB"`, including an `Err` from a custom tool with `failure_effect = Unknown`. The loop correctly stops dispatch and returns `RequiresHumanInput`, but the retained record falsely asserts completion and loses `effect_status: "unknown"`. Spec `docs/tool-failure-effects.md:3`: “The JSON/CLI tool error includes `effect_status: "unknown"`”; line 9: “The separate 256 KiB completed-result-loss boundary continues to stop dispatch.” A public custom `Tool` can return an oversized error after an effect. Use a bounded failed-error replacement carrying the original effect classification, retaining the existing replacement for successful completed results; add a custom oversized-error regression.

Other inspected requirements match the contract: both native and existing text loops stop unknown failures before pending dispatch or another model request; records are retained; trusted local read/query implementations explicitly opt into `NoEffect`; write/exec/HTTP/MCP/semantic/custom implementations retain the default `Unknown`; same-name custom registration is classified by its implementation; successful results and known nonzero exec returns keep their existing semantics.

## Evidence and scope

Read-only source and fixture inspection only; no build or test run by this reviewer. The four committed file/Docker/native/legacy matrices assert actual effect counts, pending-effect absence, model-submission counts, HTTP/SSE/CLI states and persisted history; their success claims are not independently certified here. The frozen original timeout log remains separate from current qualification. Generic external effect certainty, supplier/channel qualification and durable/replay recovery remain open milestones, as the roadmap requires.
