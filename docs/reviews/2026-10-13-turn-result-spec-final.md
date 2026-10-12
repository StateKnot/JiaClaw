# Spec — final fixed source 797194b

Fresh review: `git diff 0a8b399406a4fbf49135c5189a4ce8af4bb5dc86...797194b0f72e1ad8cbec806a2036469c0623f309`. Spec: `docs/turn-result-download.md`. The fixed cumulative `4b2357fcf01f47ba08d7724edbba7accfb60972c` baseline and PR134's qualification remain inherited history, not fresh exhaustive cumulative review. No Standards prose was read.

**0 remaining product findings.** Initial P2 authentication feedback is closed: the final handler reports `ApiError(401)` only while disconnected, preserving original identity clearing and refusing the file without restoring private state. The spec's “若正文已清理或 Key 已撤销则拒绝下载” and “身份切换…迟到 GET 不能启动下载” hold in the independently executed public browser seam.

Independent actual probe59850 exited0 against exact final binary `28e8be5deb15d52ceecd01862271d0f660aab4acefc92def212016d266f1bbfb`. It completed two real native/model/SQLite requests, then downloaded exact BOM/CRLF/NUL/Unicode/whitespace bytes (24) and an empty reply (0), each via one original GET. An injected unauthorized `exec` tool receipt was refused; a gated original GET arriving after real reconnection/identity change created no file and restored no private DOM. Non-JSON401 cleared private DOM and now displayed 鉴权失败; only the two prior files existed. Two actual modelPOSTs, zero file-tool effects, zero page errors and fixture faults; browser storage remained empty. Browser Chromium153.0.8010.12 used the existing custom distribution. Contract-negative responses are not supplier or business certification.

Static review confirms the original bounded GET/receipt validator, current identity, completed/no-active/purge gates, final-GET freshness requirement, UTF-8 encoding, UUID-only filename, bounded original reply, Blob retirement, and read-only query path. No new execution/tool/storage/history/polling route or scope creep was found. This review does not borrow root full Rust/browser results or certify future official CI.

Separate temporary driver P3: strict new audit receipt parsing initially accepted duplicate `exact` keys. Its raw counterexample is preserved separately, and the corrected parser rejects it plus all 36 independent negatives while preserving the positive envelope and all original 162 assert ASTs. This is driver validation, not product or official CI execution. Initial P2 report/proof and temporary setup errors remain independently preserved.

Spec product findings: 0.
