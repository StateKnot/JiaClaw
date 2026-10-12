# Standards — revised live semantic review source

Fixed source: `56e61fc999e9a28ea55d91999623858d76961409`; fresh parent: `76ddacbb635ce4f86cdbcae96c2c0d65eb7036cc`. Reviewed the complete nineteen-path feature diff and six-path repair diff from `f5845fc`. Commits are `f5845fc feat(memory): inspect the live semantic index from the private workbench` and `56e61fc fix(memory): reject incomplete index snapshots and keep private method errors`. Cumulative `4b2357fc` and PR107–129 history are inherited, not exhaustively re-reviewed. Initial Standards raw report remains preserved and does not certify Spec correctness.

Sources remain user AGENTS, `/Users/jiawy/.codex/AGENTS.md`, repository architecture/workspace/memory/semantic/validation/release contracts, revised `docs/semantic-review.md`, and Matt Pocock code-review/codebase-design skills.

## Documented violations

**0.** `docs/architecture.md` requires textContent rendering and separate standalone/gateway authority. The UI still uses the existing identity guard, explicit action and RAM-only metadata, while its private generation validator checks required presence, space relation, finite bounded counts/bytes and current chunk consistency before rendering. No raw source text, browser storage, automatic embedding or tenant authority is introduced.

Both route method fallbacks now call the same authorization and private-response Module as GET, retaining nonempty administrator/standalone/default-off gates and no-store/nosniff/no-referrer. Unsupported methods remain refused with accurate GET/HEAD Allow metadata. They create neither a Store owner nor a control worker.

The actual check retains the original Agent Arc, semantic admission/persistence/workspace owner and four-slot control permit through settlement, consistent with `docs/workspace-files.md` and `docs/semantic-check.md`. Shared authorization/response helpers and original complete capture avoid competing Implementations. Fixture changes remove the 405 privacy exemption and add seven inconsistent-generation refusals; existing conditions and budgets are not weakened.

## Possible baseline smells

**0 actionable judgement calls.** Applied Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. The generation validator is a present protocol constraint; one method fallback serves both real routes. Existing JSON and ownership conventions override speculative type/executor layering. The Agent Interface hides optional feature ownership rather than exposing caller navigation.

Tool-enforced matters are excluded. This is static fixed-source review, with no compilation, product/browser acceptance or network rerun; no Spec findings were consulted. Root full qualification remains an independently captured pending execution and cannot be inferred from this report or initial evidence.

Standards: **0 documented violations / 0 actionable heuristics**; worst issue: **none**.
