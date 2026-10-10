# Intel candidate job budget — independent Spec review

Fixed cumulative base: `origin/main` at `4b2357fcf01f47ba08d7724edbba7accfb60972c`; reviewed committed head: `ef0c04eae69947eaeca4b0e2ca4500a134cd2079`, parent `70acdcc3e4eeb93d89e9438665a752167ad39fbb`. Fresh scope: complete `git show ef0c04e`, limited to `.github/workflows/release.yml`; the commit matches the previously inspected working-tree diff. Previous source/fixture/document findings remain inherited; the oversized-error P2 remains closed. This is not an exhaustive re-review of the cumulative branch or pending documentation, which is excluded from this budget review.

**0 open incremental Spec findings:** missing/partial requirements: 0; unrequested scope: 0; incorrect implementation: 0.

The actual job evidence identifies Intel candidate job `114234851224`, old head `fd4f98f186c98dd1b49eab98ab54fa3dabcac4e2`. Its annotation says “The job has exceeded the maximum execution time of 40m0s.” Unit tests succeeded in 20m50s and optimized compilation in 16m49s. Optimized-binary verification was then cancelled; archive installation and artifact upload were skipped. Those earlier successes do not establish a qualified candidate.

The diff sets `job_timeout: 60` only on `macos-15-intel / x86_64-apple-darwin` and uses `${{ matrix.job_timeout || 40 }}`. The three other candidate platforms retain 40 minutes. No step, command, test assertion, tool/application/fixture deadline, runner target, upload condition or permission changes. The finite extra job capacity directly addresses the demonstrated inability to finish required acceptance after cold compilation.

`docs/release-candidates.md:3` requires complete locked tests followed by optimized binaries and actual archive installation; all remain mandatory. Line 39: “PR 只有 read token，`draft` 作业条件明确排除 PR” and “公开发布仍须单独人工审核.” The diff preserves `contents: read`, tag-only draft conditions, `needs: build`, checksummed draft creation and separate public-release review. It neither authorizes publication nor reclassifies the cancelled old candidate as successful.

Read-only source and saved official-evidence inspection only; no builds/tests executed. The complete 43-process-suite run was still collecting, and no new-head official qualification is certified here. Only this report was written outside the repository.
