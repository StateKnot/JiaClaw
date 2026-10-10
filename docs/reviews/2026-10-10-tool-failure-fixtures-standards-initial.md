# Tool Failure CI Fixture Fix — Independent Standards Review

Date: 2026-10-10. Fixed base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`; head: `44f3c37cd35ad1ac23318dd9fcae4a97a9f0ca58`. Cumulative command: `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...44f3c37cd35ad1ac23318dd9fcae4a97a9f0ca58`; 102 commits/225 files, with commit list `/tmp/jiaclaw-oct10-tool-failure-ci-fix-review-commits.txt`. Fresh focus: the complete `git show 44f3c37` and relevant fixture callers, three Python files only. Prior source/document Standards conclusions are inherited; no exhaustive re-audit of 114k cumulative lines is claimed.

## Documented standards

**0 new hard violations.** The user's rule is “都需要生产级可用的方案”; cache instructions apply to builds/tests, neither performed here. No tracked CONTRIBUTING/CODING_STANDARDS/GLOSSARY was found. Cargo lint/format rules are tooling-enforced and excluded. The fixtures retain actual workspace-byte/inode checks and use fresh session IDs for explicitly separated rejection cases. Production implementation and resource ownership are unchanged.

## Judgment heuristics

**1 open finding — [P3] possible Duplicated Code.** `tests/workspace_files.py:179–196`, `tests/workspace_mutations.py:175–192`, and `tests/workspace_copy.py:181–198` repeat the new policy shape: `return [self.tool(..., rejected=True) ...]`, `expected_status = 'requireshumaninput' if needs_review else 'completed'`, model count `1 if needs_review else 2`, and the `effect_status` assertion. This single error-policy change already required synchronized changes to three actual consumers. Further contract changes can leave their acceptance helpers inconsistent.

Extract only the shared rejection/request and response-assertion shape into a small fixture helper, passing each suite's expected pure/unknown effect distinction. Keep their separate localhost models, authority catalogs and filesystem invariants. This is a maintenance judgment under the skill's Duplicated Code baseline, not a demonstrated production defect or a hard repository-rule violation; merging the entire Host implementations is unnecessary.

All twelve baseline smells were considered; no other actionable heuristic was identified. The supplied three-suite success and ongoing mandatory-suite collection were not independently certified. No builds, tests or source edits were executed, and Spec compliance was not reviewed on this axis.

Standards: **1 unresolved heuristic, 0 unresolved hard violations; worst issue within this axis P3.**
