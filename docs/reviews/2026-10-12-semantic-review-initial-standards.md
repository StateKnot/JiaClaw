# Standards — live semantic review source

Fixed source: `f5845fc917d7ab95bd8f9736bb79d3ab56b99b46`; fresh parent: `76ddacbb635ce4f86cdbcae96c2c0d65eb7036cc`. Captured `git diff 76ddacbb635ce4f86cdbcae96c2c0d65eb7036cc...f5845fc917d7ab95bd8f9736bb79d3ab56b99b46`; commit list is `f5845fc feat(memory): inspect the live semantic index from the private workbench`. All nineteen fresh paths were examined. Cumulative `4b2357fc` and PR107–129 source/delivery/publication/official reviews are inherited, not exhaustively re-reviewed.

Sources: user AGENTS instructions, `/Users/jiawy/.codex/AGENTS.md`, documented architecture/workspace/memory/semantic/validation/release contracts, `docs/semantic-review.md`, and Matt Pocock code-review/codebase-design. No repository AGENTS, CONTRIBUTING, CODING_STANDARDS or GLOSSARY file was found.

## Documented violations

**0.** `docs/architecture.md` requires textContent rendering and distinct standalone/gateway authority. The new UI uses textContent, the existing identity guard and RAM-only state; capability negotiation excludes gateway/read-only identities, and stale/error/reconnect paths clear metadata. The CSS change wraps the complete existing SHA state rather than truncating it.

Backend `semantic_review.rs` reuses `memory_review` authorization/private-response helpers and independently requires the default-off semantic flag plus enabled semantic memory. It retains the existing four-slot control permit in actual admitted work. `JiaClawAgent::check_semantic_memory` accesses the already-connected original Arc; it does not construct another Store, model tool or executor. Original semantic admission, directory capability and complete source validation remain behind that Interface, consistent with `docs/workspace-files.md` and `docs/semantic-check.md`. GET-only wiring adds no tenant or model authority.

Documentation distinguishes preliminary focused qualification, outstanding full/new-head gates, ordinary Agent startup normalization and custom browser provenance; it does not claim supplier, learning-quality, Agent-browser-tool or durable certification.

## Possible baseline smells

**0 actionable judgement calls.** Applied Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. The Agent Interface hides optional-feature ownership; shared authorization/response helpers serve two actual callers. This earns locality without a hypothetical store abstraction or duplicated capture Implementation. Existing JSON/protocol conventions override speculative cross-language type layering.

Tool-enforced matters are excluded. This is static source review; no product, browser, Rust or network acceptance was re-run, and no Spec findings were read. Initial targeted evidence is not promoted to complete or official qualification.

Standards: **0 documented violations / 0 actionable heuristics**; worst issue: **none**.
