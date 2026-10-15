# Initial Standards review — tenant file move

Reviewed `git diff 280932753003e33831fab9cbecce8a935a6263f2...f949dea5b6974adcac5c15b5e8d77b0f56548d5b` (one commit, 19 files), source tree `43ddc7f85501730eb7065a7f49418ed9d67377e1`.

**Documented-standard violations: 0.** The immutable source inventory contains no AGENTS.md, CONTRIBUTING.md, CODING_STANDARDS.md or CLAUDE.md. Applicable sources checked were README.md, Cargo.toml/.github/workflows/ci.yml, docs/architecture.md, docs/workspace-files.md, docs/native-tools.md, docs/tool-failure-effects.md, docs/private-state-files.md, docs/tenant-copy.md, docs/tenant-file-settlement.md, and relevant tenant HTTP/gateway contracts. Tool-enforced formatting/lint rules are excluded.

The added domain/configuration types remain in core; capability/tool policy stays in the library and gateway/UI wiring in host (architecture ownership rule). Restricted moves call the original memory_io rename engine under its existing blocking capacity and workspace writer (workspace-files.md, mkdir/move and settlement rules); no second mutation engine, weakened failure-effect classification, retry or storage was added. UI pair display uses textContent (architecture HTTP rule).

**Fowler heuristic findings: 0.** Considered all twelve required smells. The shared TenantFilePair validation and canonicalDocumentPairs remove duplicate path/pair checks. The small file-only engine adapter preserves the existing test seam and owner, so it is not a useful Middle Man finding. Explicit permission fields across trust boundaries are necessary contracts, rather than a demonstrated Shotgun Surgery violation. No actionable new name, ownership, clump, domain-type, switch, responsibility, speculative hook, navigation or inheritance smell was established.

Independent work was static review and identity verification only; no tests/builds/provider requests executed. Root validation results are not claimed as this reviewer's execution. Earlier cumulative origin/main coverage is inherited, not exhaustively re-reviewed. Initial report retained separately from any later closure.
