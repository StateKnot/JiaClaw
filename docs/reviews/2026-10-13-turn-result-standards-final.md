# Standards — final fixed-source review

Independently reviewed `git diff 0a8b399406a4fbf49135c5189a4ce8af4bb5dc86...797194b0f72e1ad8cbec806a2036469c0623f309`: `15d7eb7` adds original-reply downloads; `797194b` restores revoked-key feedback after identity clearing. Nine paths changed. Cumulative `origin/main` remains `4b2357fcf01f47ba08d7724edbba7accfb60972c`; earlier source/runtime and PR134 qualification are inherited history, not an exhaustive fresh cumulative review.

**Documented-standard breaches: 0.** Reviewed user/global AGENTS instructions, README and the download, Web streaming and tenant original-request/privacy contracts. No repository AGENTS.md, CODING_STANDARDS.md, CONTRIBUTING.md or GLOSSARY.md was found outside generated directories.

The action reuses the original bounded `turnApi`/receipt validator/control owner. It downloads the verified retained reply, rather than preview DOM, and creates no new backend reader, store or execution path. The original UUID forms the filename. Browser standard UTF-8/Blob APIs preserve text; invalid surrogates are refused; temporary anchor/object URL ownership is closed. The UI describes a requested browser download without claiming final user persistence, and explicitly explains saved-file privacy.

The final `turnControl` hunk reports `ApiError(401)` only while disconnected. `api` checks the captured identity before clearing it on 401; the new catch displays authentication feedback without restoring the old attempt, private DOM or credentials. A newly connected identity is protected. The parent's actual initial nonJSON-401 feedback failure is separately preserved; the initial static zero-finding report is not substituted for runtime acceptance.

**Actionable Fowler heuristic findings: 0.** All twelve categories were considered as judgement calls, with repository conventions overriding them. Shared physical-file assertions avoid duplicated suite-specific save implementations; no speculative production abstraction or tooling-enforced style issue was raised.

Separate external-driver verification: the corrected `72bd7b9…` driver preserves all original 162 assert ASTs and adds one guard. A synthetic Actions envelope using the three final real browser logs accepted one positive and rejected 185 typed-field, UUID, exact byte/hash, command, ownership, duplicate/missing-marker and order countercases. This tests the driver, not official Actions or a product rerun. The earlier invalid temporary driver is retained.

I read no Spec report and changed no repository files. **Result: 0 documented breaches; 0 actionable heuristic findings.**
