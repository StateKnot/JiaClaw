Spec review: e5db71f…443578f, tree be6e7f9d. Fresh feedback feature and review fixes, independently of Standards.

No open Spec findings. The initial P2 is closed: spec lines7/9 require “status=completed” and “精确后端收据”. Both original malformed result acknowledgements that returned200 on e4 return503 on final443; actual backend intent is reconciled only through original GET. Initial RED receipts/report/identity and d45 intermediate evidence remain preserved.

Own final execution: 16 malformed UTF-8/surrogate/type/duplicate/field/control/length requests refuse; wrong namespace/content type/private gateway marker refuse. Actual5.005-second body408, four native/two gateway nonqueued controls, disconnected SQLite worker ownership and key revocation during incomplete body pass. All14 malformed acknowledgement modes reject503, including missing/nonarray/nonstring/forbidden tools, reply above2MiB and names above2048. Original facts/model ledgers/admissions/holds remain unchanged; one model POST per original task.

Actual native/gateway/both-backend SIGKILL/restarts recover exact facts. Disabled tracked backend explicitly advertises feedback_protocol0; gateway restart still reads original GET200, while fresh task/feedback refuse503 without changes. Omitting feedback_protocol even when disabled rejects gateway startup with exit1. Chromium 153.0.8010.12 receives a delayed Alice original GET after switching to Bob: private saved DOM/draft/review/UUID stay cleared, zero feedback PUTs/storage entries/page errors.

All109 compiled inputs independently match443. identity.json binds immutable final executable, source manifest, drivers and23 real check groups; errors=[]. Evidence: /tmp/jiaclaw-oct13-round96-feedback-spec/independent-api-443-final.log, independent-api-evidence-443-final.json, late-identity-browser-443-final.log, missing-disabled-feedback-capability.log. No missing requirement or scope creep found in the fixed fresh diff. Historical cumulative claims were not requalified.

Separately, CI-driver SHAccb269d7 passes4 explicitly synthetic positives and57 negatives; all166 parent assertion ASTs remain exact, plus2 guards=168. ci-driver-identity.json keeps this synthetic validation separate from product/official-CI qualification. One initial stale-driver-hash harness refusal is preserved; it executed no parser or product.

Spec: 0 open findings; initial1 P2 resolved. Production execution errors: none.
