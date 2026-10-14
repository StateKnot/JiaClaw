# Spec — CI dependency-order closure

No remaining findings in the fixed `6cee97a...cd228717` closure. Its only path is `.github/workflows/ci.yml`: the exact Linux-only tenant directory browser step is moved after the existing Browser acceptance block, which installs Playwright 1.62.1 and Chromium. Removing that single identical step from both revisions produces byte-identical workflow text. Existing commands, conditions, assertions and budgets are unchanged.

The root identified the earlier dependency-order gap before publication. My initial 0-finding Spec report did not identify it; that initial report and identity remain unchanged. This closure confirms the concrete correction without representing a hosted Actions run.

All 120 compiler inputs match both final source and current files. The frozen binary remains SHA256 `5c7ceb4d09a519e075f5750d7b4503447ebc3666883da16965dd6ac753fd9962`. My independent native/gateway/model countercases from the initial source remain applicable; no product execution, broad matrix, old qualification or CI mutation was repeated for this workflow-only change. The source review does not certify the future PR's own fixed-head official qualification, suppliers, business quality, Agent browsing or durable execution.
