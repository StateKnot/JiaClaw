# Spec — initial fixed source 15d7eb7

Fresh review: `git diff 0a8b399406a4fbf49135c5189a4ce8af4bb5dc86...15d7eb748c682a5d64eace0971d00695cd895b21`, one feature commit. Spec: `docs/turn-result-download.md`. The cumulative `4b2357fcf01f47ba08d7724edbba7accfb60972c` baseline and PR134 qualification are inherited history; this is not a fresh exhaustive cumulative review. No Standards prose was read.

**[P2] Revoked-Key refusal loses its authentication feedback and the new acceptance cannot finish.** The spec requires “若正文已清理或 Key 已撤销则拒绝下载” and explicitly includes “撤销/身份迟到…分别验收”. The added download handler invokes the existing `turnControl`; on the real GET returning non-JSON HTTP401, `api` clears identity and throws `ApiError(401)`, but `turnControl` ignores the error because its captured identity is now old. The actual page keeps “回复已保存 · 当前无活跃执行”, giving no explanation or reconnect instruction. The new `revokedDownload` acceptance therefore waits indefinitely for its required 鉴权失败 status. Keep the identity/DOM clearing and refusal; surface the 401 message while disconnected without restoring the old attempt.

Independent actual Chromium153.0.8010.12 against immutable binary `d3cd8c216b71964cf8ba311cbd098561839599f653f13498ae89066ec65de5e2` reproduced this failure (session6236, exit1). It first verified real downloaded files for BOM/CRLF/NUL/Unicode/whitespace (24 exact UTF-8 bytes) and empty reply (0 bytes), each one original GET; the unauthorized tool-name response was rejected. The revoked GET made no third download and cleared private DOM, so this finding is feedback/acceptance correctness, not data disclosure or replay. The modified response is a contract-negative fixture, not supplier certification.

No other missing/partial requirement or scope creep found. Temporary driver setup errors (sandbox loopback restriction, missing explicit tool selection, frozen binary mode, and corrected initial/final identity selection) are preserved separately and are not product findings. Repository/source objects were not modified by this reviewer.

Spec findings: 1 P2.
