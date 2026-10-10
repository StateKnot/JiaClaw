# Doctor Capability — Spec Review

Fixed cumulative base: `4b2357fcf01f47ba08d7724edbba7accfb60972c`; final source head: `b4291089175f088229fd6619e78005608f80cd6b` (97 commits). Focused comparison: `48830471f135bb7486abfbd74184cfd0e396b95b...b4291089175f088229fd6619e78005608f80cd6b`. The final commit contains only the reviewed fixture correction in `tests/doctor.py`; production source is unchanged from `9e22620`. Spec: `docs/doctor.md`.

## Spec

**0 open focused findings:** missing/partial requirements: 0; unasked behavior/scope creep: 0; implemented requirements with wrong behavior: 0.

The focused source review confirms closure of the reported misleading capability statuses:

- Spec line 27: “Explicit stub mode does not require a model key.” `main.rs:5118` now reports a key as unnecessary for explicit stub. The corrected fixture covers the default SQLite configuration and a separate explicit `persist=false` memory case.
- Spec line 27: “An invalid endpoint cannot also display a successful endpoint check.” `main.rs:5128` conditions the nested Brokerrouter endpoint result on the same provider readiness result. Its heading is now a local configuration check. The frozen old-binary failure at `/tmp/jiaclaw-oct10-doctor-capability-initial.log` remains a real contradictory-success reproduction.
- Spec line 29: “its configured server count is shown separately from the discovery result.” `main.rs:5377` reports the actual MCP configuration count and points to the tool-system connectivity result. The durable driver is explicitly reported as unwired, matching the existing application execution path.
- Spec line 31: “Even `--connect` does not open, migrate, lock, or create the HTTP session database.” Doctor still calls only `JiaClawAgent::connect`, whose storage initialization is limited to enabled model-call and semantic stores. The session opener is absent from this path. `main.rs:5384` branches on `http.persist` and explicitly disclaims database availability/recovery verification.

The original fixture expected memory storage despite `HttpConfig` defaulting to `persist=true`; the final committed correction fixes that test assumption without changing production behavior. This was a fixture error, not a production defect.

## Scope and evidence

This is a read-only source/fixture review; no build or test was executed by this reviewer. The coordinator's final real-CLI run passed both diagnostic groups in `/tmp/jiaclaw-oct10-doctor-capability-final.log`. Inherited architecture and readiness reports remain carried forward; the cumulative inherited diff was not freshly exhaustively reviewed. Supplier qualification, durable recovery, and session availability remain separate acceptance work, as spec lines 23/31 require. No external credentials, paid requests, merge, or release were used.
