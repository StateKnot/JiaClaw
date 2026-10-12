# Standards — semantic review admission fixture

Fixed source: `da4702eee5b09a800236c7893899a78657a4c49b`; fresh parent: `941f226a88c5fd64cf6f1b462c7e3419702741e6`. Captured `git diff 941f226a88c5fd64cf6f1b462c7e3419702741e6...da4702e`; commit: `da4702e test(memory): prove contender body admission before checking owner capacity`. Fresh scope is exactly `tests/semantic_review.py` and `docs/semantic-review.md`. Production source56, 106 inputs, frozen f9, Rust/browser and cumulative `4b2357fc` history remain inherited, not newly executed or exhaustively re-reviewed.

Sources: user AGENTS, `/Users/jiawy/.codex/AGENTS.md`, repository owner/workspace/semantic/validation contracts, and Matt Pocock code-review/codebase-design skills.

## Documented violations

**0.** The fixture now establishes actual contender admission before asserting exhausted control capacity. Its shared bounded wire-header reader uses the original absolute deadline; that deadline starts before connecting/sending instead of receiving a fresh budget afterward. The three exact native Continue responses and fifth 503 without Continue distinguish Body polling from TCP/header delivery. Original source-owner, SQL, authorization, body and settlement assertions remain; the five-second Body and eight-second settlement limits are unchanged.

The locked Hyper1.11.1 local source sends Continue from `poll_read_body` while writing is still Initial; the existing replacement handler acquires its control permit before `to_bytes`, and the middleware only validates length/adds the resolved budget. Documentation states this concrete contract rather than attributing the official failure to an unproven scheduler or framework defect.

Controlled proxy evidence records zero delivered contenders during the old observation, subsequent actual503, held source read and unchanged tables. It remains a demonstrated mechanism, not the unique historical ARM cause. Root’s normal/400ms × release/SIGKILL receipts and original local success are distinctly scoped; neither qualifies the repaired official head.

## Possible baseline smells

**0 actionable heuristics.** Applied Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. One header reader serves real Continue/refusal consumers; the admission helper adds a concrete protocol assertion. No hypothetical production hook, executor or duplicate owner is introduced. Repository evidence conventions override speculative layering.

Tool-enforced matters are excluded. This is static fixture/documentation review; no compilation, product/browser or network acceptance was re-run, and no Spec findings were read.

Standards: **0 documented violations / 0 actionable heuristics**; worst issue: **none**.
