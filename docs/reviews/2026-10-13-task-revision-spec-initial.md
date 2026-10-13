## Spec

No missing/partial requirement, incorrect behavior or scope creep found in the fresh `7fb67832… → worktree` source snapshot (UI, workflow, new spec and two fixtures). This is an initial source review while the new build/browser verification is still being handled separately; PR138's completed official qualification and cumulative `4b2357…` conclusions are historical only.

`docs/task-revision-draft.md:5` requires a verified completed original task, no active execution, retained result, saved nonblank needs_changes note, writable identity, selected original session and empty draft. The new UI gates preparation on those conditions and starts unchecked. Editing feedback cancels its review binding.

Line 7 requires confirmation bound to “原 UUID、会话、请求摘要、评价 revision、完整回复和已保存说明”. The click handler captures those exact values, invokes the existing bounded 15-second authenticated GET/receipt validator, and rejects changed identity, attempt, selected session, nonempty draft or changed bound content before preparing. It adds no retry, polling or model request.

Line 9 requires “整份含 framing 最多32768 UTF-8字节，exact允许…整次拒绝，不截取”. The complete receipt reply and saved note are LF-normalized without trimming, retain BOM/TAB/whitespace, include original UUID/revision, and are rejected wholesale on malformed Unicode, disallowed ASCII controls or excess total bytes. The displayed clipped reply and extra history are not draft sources.

Line 11 requires clearing tool/skill/source selections and an explicit new send. Preparation clears all permission inputs and private feedback/confirmation, ends only page tracking, and leaves the editable draft on the original send path. It issues no feedback write, cancellation, review/hold settlement or original-UUID submission; the normal sender still creates the new UUID/admission.

The three-mode fixture declares changed-feedback, synthetic exact/overflow/invalid-character cases, delayed actual GET/draft/identity, readonly/reload, actual full draft/new submission and purge refusal checks. Its synthetic metadata is labelled explicitly and does not claim model execution or business quality. Runtime pass/fail and official qualification for this new source snapshot remain independent evidence obligations.

Initial source Spec findings: 0.
