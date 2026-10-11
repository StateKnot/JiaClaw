# ModelProbe source — independent Standards review

Open actionable findings: **0 hard violations; 0 heuristic findings**.

Fixed cumulative baseline: `4b2357fcf01f47ba08d7724edbba7accfb60972c...4616ab34d8562cba023d5a6f7e7dab7b40b43905`. Read the complete cumulative commit history. Fresh review: `4862de9e1619be5ed604946a083a5e7ad1e43512..4616ab34d8562cba023d5a6f7e7dab7b40b43905`, nine production, fixture, workflow, README and specification paths. Earlier cumulative architecture and MCP/skill reviews are inherited; this is not an exhaustive cumulative rereview. Immutable Git objects were used, not the working tree. The Spec review was not consulted.

Normative sources: user AGENTS production, authorization, actual-owner cancellation/persistence and disk/cache requirements; `docs/model-probe.md` and the existing model-call/provider contracts. No physical AGENTS, CONTRIBUTING, CODING_STANDARDS, GLOSSARY or ADR was found. Tool-enforced formatting/Clippy rules were excluded.

The CLI requires explicit configuration and billing confirmation; preparation rejects invalid route/key/endpoint before private state opens. The dedicated library constructs only two bounded challenge messages and no tools, reuses the existing exclusive receipt owner, consumes its prepared attempt before dispatch, and emits identities only after persisted completion. Cancellation settles the actual owner without replay. `main.rs` now shares stderr tracing with structured-output commands, closing the demonstrated stdout pollution. Existing bounded status metadata is read fail-closed within the one-shot module; no new persistence/recovery state machine is duplicated.

All twelve Fowler heuristics were considered as judgments: Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. No actionable smell was established: the probe owns challenge/request/report semantics, while ledger settlement remains with ModelCalls.

The real fixture preserves billing gates, exact wire/receipt assertions, durable unknown holds, admission rollback, observed SIGTERM settlement and GET-only SIGKILL recovery, and is wired into CI/release qualification. Historical configuration/budget/signal-fixture failures and stdout RED remain separate. Final full qualification is still author-run; this source review grants no official CI, supplier, physical-deadline or billing certification. Reviewer ran no builds, tests, network requests or cache cleanup.

# ModelProbe fixture supplement — independent Standards review

Open actionable findings: **0 hard violations; 0 heuristic findings**.

Fixed incremental scope: `4616ab34d8562cba023d5a6f7e7dab7b40b43905..69f0868a8f3a87cabc23089e0dd748d96e79efce`, only `tests/model_probe.py`. Cumulative base remains `4b2357fcf01f47ba08d7724edbba7accfb60972c`; the earlier nine-path source review is inherited unchanged. This is not an exhaustive cumulative rereview. Immutable Git objects and the incremental commit history were read. Spec review was not consulted.

The specification promises that ambient MCP/embedding configuration cannot cause discovery, network submission or semantic state initialization. The fixture now provides an enabled, authenticated MCP endpoint on the local gateway and enabled semantic memory configuration. An accidental MCP request reaches the existing exact-path assertion and enters the shared fixture error channel; `snapshot()` rejects it. The successful probe additionally checks that the configured semantic SQLite file is absent. The original two-message, no-tools, identity/hash, session absence, durable hold, real signal and recovery assertions remain intact. The random token remains disposable, and private content/token output checks still apply.

All twelve Fowler heuristics and the user production/auth/ownership/cache standards were reconsidered for this test-only hunk. The extension reuses `configure`, gateway error collection and original assertions rather than creating a second request/ownership harness; no actionable repetition, hypothetical hook or new authority boundary was introduced.

Readback of `model_probe-final.log` and `model_probe-ambient-final.log` shows seven actual GREEN groups each, including observed SIGTERM settlement and GET-only SIGKILL recovery. They are separate from the retained stdout pollution RED and other initial fixture failures. Production inputs were not changed by this fixture commit; full Rust/official qualification is not granted by this supplement. Local fake-gateway evidence does not certify supplier billing, real credentials, physical deadlines or installation. Reviewer performed no builds, tests, network requests or cache cleanup.

# ModelProbe test settlement — independent Standards supplement

Open actionable findings: **0 hard violations; 0 heuristic findings**.

Fixed scope: `69f0868a8f3a87cabc23089e0dd748d96e79efce..0e8c03656d3a479eed9f6b1428d49f6eb476468e`, two paths. Cumulative baseline remains `4b2357fcf01f47ba08d7724edbba7accfb60972c`; earlier ModelProbe source/fixture and historical cumulative reviews are inherited, not exhaustively repeated. Immutable Git diff and blobs were read; the Spec report was not consulted.

`crates/jiaclaw/src/model_probe.rs` adds `std::fs::create_dir(&config.workspace_path).unwrap()` only inside `cfg(test)`. It establishes the existing private-store workspace precondition before the transport-failure test opens ModelProbe. The assertions still require an unknown original hold, refusal to reuse the consumed request, and unchanged ledger metadata. There is no retry, assertion relaxation or production initialization change. Independent production-prefix SHA-256 comparison across the two revisions is identical.

`docs/doctor.md` links the separate `model-probe --config ... --confirm-billing` command, requires stopping the same-ledger service, and limits success to its diagnostic response. The paragraph preserves doctor’s no-model boundary and explicitly avoids tools, channels or supplier-deployment certification.

User production/authorization/actual-owner/cache standards and all twelve Fowler judgments were reconsidered. This test precondition and adjacent diagnostic contract introduce no actionable naming, duplication, data ownership, primitive/type, switch, scattering, mixed responsibility, hypothetical abstraction, navigation, delegation or inheritance issue. Tool-enforced rules remain outside this axis.

Historical `rust-final.log` actually failed: 422 passed, one failed, one ignored, with the new test reporting a missing workspace. That RED is a fixture-precondition failure, not a repaired production defect or complete Rust qualification. At review, `rust-settled.log` contained compilation progress only; no final Rust pass is claimed. Earlier seven-group CLI GREEN remains separate, and no parent/official qualification is borrowed. Reviewer ran no builds, tests, network requests or cache cleanup.
