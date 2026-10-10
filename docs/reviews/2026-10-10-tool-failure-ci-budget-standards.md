# Intel Release Budget — Independent Standards Review

Date: 2026-10-10. Fixed cumulative base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`; committed head: `ef0c04eae69947eaeca4b0e2ca4500a134cd2079` (104 cumulative commits). Comparison: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...ef0c04eae69947eaeca4b0e2ca4500a134cd2079`. Prior source, documentation and fixture Standards conclusions are inherited; no exhaustive re-audit of 114k cumulative lines is claimed.

Fresh focus: complete `git show ef0c04eae69947eaeca4b0e2ca4500a134cd2079`, parent `70acdcc`; only `.github/workflows/release.yml` changes. The committed blob `3e303f1e0a0c6524240ef79c62f2d749cf276b78` matches the initially inspected patch. Uncommitted documentation is outside this focused review.

## Documented standards

**0 hard violations; 0 open hard findings.** The user's “都需要生产级可用的方案” rule supports a bounded qualification budget grounded in observed execution. No build/test was run by this reviewer, so local cache instructions did not trigger. Cargo/tool-enforced lint and formatting rules are excluded.

The supplied official job `114234851224` is pinned to old head `fd4f98f186c98dd1b49eab98ab54fa3dabcac4e2`. Its annotation explicitly says “The job has exceeded the maximum execution time of 40m0s”; timestamps show unit tests took 20m50s and optimized compilation 16m49s. Qualification began near minute 38, was cancelled, and package/install steps were skipped. This is direct evidence for budget insufficiency, not successful qualification inherited from that old source.

`release.yml:52–61` adds `job_timeout: 60` only to `macos-15-intel` and uses `timeout-minutes: ${{ matrix.job_timeout || 40 }}`. The other three candidates retain 40 minutes. All build, optimized-process, archive/install and artifact commands remain present; no fixture deadline, cancellation policy, failure condition or production behavior changed. The comment's approximate 38-minute setup/test/build figure agrees with the evidence.

## Judgment heuristics

**0 actionable heuristics; 0 open Standards findings.** All twelve skill baselines were considered. One matrix override and a shared fallback express the actual platform-specific budget without repeated platform switches or speculative abstraction.

The 60-minute budget is justified but still requires current-head official execution. The ongoing local 43-suite matrix and candidate qualification are not certified here. No repository edits, builds or tests were performed; Spec compliance was not reviewed on this axis.
