Initial external-driver finding: P3, missing JSON-suite lower bound.

`docs/tenant-json-restart-admission.md:9` requires “全部继承合同与新的实际准入收据”, preserving `docs/task-activity-validation.md:27`'s “不能借父日志或其他段”. Driver SHA256 `ac05b699249bb20d11467458dee465ecfb04f4661ca6137ebad205c7d425b058` checks PASS5 < receipt < PASS6 < JSON completion, but has no owned JSON-start lower bound.

An independent synthetic counterexample moved the actual restart receipt and adjacent PASS5/PASS6 lines before the standalone Web completion, keeping the JSON activity marker/completion in their correct later segment. All three inherited activity guards and the new restart guard still accepted. Require the globally unique JSON start before PASS5/receipt/PASS6/JSON completion.

Source fixture/document review has no finding. This is an external driver flaw, not product execution failure or the unique cause of the original CI failures. The 160 old AST assertion sites remain unchanged; one new site gives 161. Counterexample and acceptance evidence are preserved in `/tmp/jiaclaw-oct12-round77-restart-spec-driver-counterexample.log` and `/tmp/jiaclaw-oct12-round77-restart-spec-driver-initial-probe.json`; old driver snapshot in `/tmp/jiaclaw-oct12-round77-restart-spec-driver-ac05.py`.
