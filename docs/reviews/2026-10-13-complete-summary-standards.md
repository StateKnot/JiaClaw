# Standards — complete summary input (initial)

Fixed source `52f57c4c69efb23981e7808ef30ecdf5cb4b5cb2`, tree `dc5be063bc44e351121c144bd6942c96ff12d16f`. Fresh review uses `git diff 640f5a4ce4492ed565b61f4a6f8ca87434783594...52f57c4c69efb23981e7808ef30ecdf5cb4b5cb2` and the corresponding commit list: six paths, two production compiler inputs. Cumulative `origin/main` `4b2357fcf01f47ba08d7724edbba7accfb60972c` and PR136 qualification are inherited history; this is not an exhaustive cumulative review.

Standards sources: user AGENTS production/disk requirements, README, Cargo workspace lint policy, configuration/model-routing guides, and complete-summary-input contract. An independent hidden-file search found no repository AGENTS, CODING_STANDARDS or CONTRIBUTING file. All twelve Fowler smells were considered as heuristics; tooling-enforced formatting/lints were excluded.

No documented-standard violations or actionable Fowler smell findings. The complete-input policy lives in the existing session formatter; one caller handles refusal through the existing error path. Checked UTF-8 byte accounting includes role labels and separators before output allocation, and the error contains no private transcript. Stub/import behavior remains on the original local digest. No additional reader, store, tool engine or retry abstraction was introduced. Documentation distinguishes complete outbound material from summary quality, hard-truncation fallback and unknown-submission holds.

Independent synthetic audit checks: one positive and 24 structural/type/hash negatives, all rejected; both transcript fingerprints were reconstructed independently. This checks only the external validator, not product execution or official CI. The 109 compiler inputs and frozen binary SHA256 `27f49af85b1dd3c0e8ca2b7d0bf6e2b83db502135e9d363ae317ea3e7b4ec289` were independently bound to this source. Root test/process executions remain inherited observations, not my executions.

Findings: 0. Identity and check evidence: `/tmp/jiaclaw-oct13-round92-summary-standards/identity.json`, `driver-checks.json`.
