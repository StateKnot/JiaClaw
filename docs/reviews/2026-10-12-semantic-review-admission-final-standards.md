# Standards — admission fixture final source

Fixed source `67cbe4b48fa7da02000252418d73af6fba068cd6`; fresh parent `941f226a88c5fd64cf6f1b462c7e3419702741e6`. Complete feature diff still changes exactly `tests/semantic_review.py` and `docs/semantic-review.md`. Separately reviewed `git diff da4702e...67cbe4b`: one final deadline assertion in the shared wire-header reader. The original da4702e Standards report remains unchanged and does not certify the Spec axis.

User AGENTS was re-read. Other standards remain the documented architecture/workspace/semantic/validation/release contracts and Matt Pocock code-review/codebase-design. Production source56, 106 inputs, frozen f9, Rust/browser qualification and cumulative `4b2357fc` history remain inherited; no exhaustive cumulative re-review or fresh execution is claimed.

## Documented violations

**0.** `read_control_header` now verifies `time.monotonic() < deadline` after the complete header is obtained, before returning. Its shared original two-second deadline is created before contenders connect/send; neither Continue nor fifth-request refusal gets a replacement budget. The added assertion closes the final-byte observation gap without widening the existing header budget, five-second Body capture or eight-second settlement limit.

The original owner/source/SQL and authenticated response assertions are retained. Three exact native Hyper1.11.1 Continue receipts precede exhausted-capacity inspection; the fifth identical request must receive503 without Continue. The existing permit-before-Body and middleware-no-Body contracts still support the documented admission meaning, with no production hook or new executor.

Documentation correctly describes the delayed-loopback experiment as one reproducible observation mechanism. It does not assert the unique historical Linux ARM cause, label a framework/product repair, or transfer old successful jobs into the repaired head’s official qualification.

## Possible baseline smells

**0 actionable judgement calls.** Applied Mysterious Name, Duplicated Code, Feature Envy, Data Clumps, Primitive Obsession, Repeated Switches, Shotgun Surgery, Divergent Change, Speculative Generality, Message Chains, Middle Man and Refused Bequest. One bounded reader serves the actual Continue/refusal consumers; the final assertion centralizes the same completion condition rather than scattering caller-specific budgets. Repository evidence conventions override speculative layering.

Tool-enforced matters are excluded. This is independent static Standards recheck; no Spec report was read and no product, compilation, browser or network acceptance was re-run. Revised qualification requires its own actual receipts.

Standards final: **0 documented violations / 0 actionable heuristics**; worst issue: **none**.
