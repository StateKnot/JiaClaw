# Standards — revised semantic check source

Fixed source: `3f265cb739d6cf65ba614ac9f002e376a1de18c9`; fresh parent: `c13885c69d5055f752f839d74588c326e9d8ec66`. Reviewed `git diff c13885c69d5055f752f839d74588c326e9d8ec66...3f265cb` (all eleven paths) and `git diff 5d0e23c...3f265cb` (six repair paths). Commits: `5d0e23c feat: check semantic source freshness without network requests`; `3f265cb fix: preserve interrupted semantic ledger during inspection startup`. Cumulative baseline remains `4b2357fcf01f47ba08d7724edbba7accfb60972c`; PR107–128 conclusions are inherited, not exhaustively re-reviewed. The initial Standards report remains preserved and does not certify Spec-axis correctness.

Sources remain user AGENTS instructions, `/Users/jiawy/.codex/AGENTS.md`, `docs/architecture.md`, `docs/workspace-files.md`, `docs/memory-files.md`, `docs/semantic-memory.md`, revised `docs/semantic-check.md`, and Matt Pocock code-review/codebase-design skills. Inventory contains no AGENTS, CONTRIBUTING, CODING_STANDARDS, GLOSSARY or memory-io file.

## Documented violations

**0.** The original relative directory-handle rule remains satisfied by `memory_io.rs::read_text_at`. In `semantic/store.rs`, private `OpenPurpose` keeps one complete path, ownership, identity, capacity and schema validation Implementation; only operational startup performs pending-to-unknown normalization. The public `SemanticMemory::check_config` Interface hides that required selection from host callers, while existing `open` callers retain their operational behavior. The same admission/persistence/Store/workspace owners and shared generation validation remain concentrated in the existing Module. The revised CLI does not introduce model, HTTP or tenant authority. Documentation explicitly distinguishes inspection from ordinary startup and retains the original hold and separate-refresh requirements.

## Possible baseline smells

**0 actionable judgement calls.** All twelve were applied: Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. The private purpose has two present behaviors; the operational wrapper preserves the established Interface. Inspection selection, source capture and validation are shared, without a second Store Implementation, hypothetical executor or inherited class hierarchy. Repository JSON and ownership conventions take precedence over speculative layering.

Tool-enforced matters are excluded. This is independent static Standards review; root execution supplies revised test/build evidence. No Spec report was consulted.

Standards total: **0 documented violations / 0 actionable heuristics**; worst issue: **none**.
