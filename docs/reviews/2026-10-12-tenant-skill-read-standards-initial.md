# Standards — initial immutable review

Reviewed fixed source `75adca37a9c1f82a6a7dfa38cc33821c01591370`, tree `bdf3d4f8d1d36885e75629d717ccf7b5949219b9`, fresh diff `db4a4c9cf9e30677363ed5474c7e79cad9e1a4ec...75adca37a9c1f82a6a7dfa38cc33821c01591370` (22 paths). Cumulative `4b2357f` review is inherited, not exhaustively repeated. Later working-tree fixture logging is excluded.

Standards sources: `/Users/jiawy/.codex/AGENTS.md` (production quality and execution hygiene), `docs/architecture.md` (core/Agent/host ownership), `docs/skill-read.md` (same loaded registry, short lock, original budgets), `docs/native-tools.md` (original authorization/schema and failure boundaries), `docs/private-state-files.md` (owner lifecycle), plus repository conventions and all twelve Fowler heuristics. No tracked CODING_STANDARDS, CONTRIBUTING, GLOSSARY, ADR or nested AGENTS source was found. Tooling-enforced warnings were excluded.

**Hard violations: 0.**

**[P3] Possible Duplicated Code — `crates/jiaclaw/src/lib.rs:600–620`, `:920–934`.** The new tenant/default-off catalog selection is repeated in production chat and the test-only prompt helper. Hunk quotes: production `let selected = self.skills.tenant_snapshot(policy)?;`; helper `self.skills.tenant_snapshot(policy).unwrap_or_default()`. The helper additionally skips production's eager-skill rejection and live permission validation when `skill_read` is unselected. This can let prompt tests silently exercise a different selection path. Extract a small shared Result-returning selector and make the helper unwrap/assert its result. This is the skill's Duplicated Code heuristic, a maintainability judgment, not a documented hard breach or an observed production disclosure.

The implementation otherwise keeps selection/read validation on the original registry owner, adds bounded explicit domain policy types, and preserves backend/tool boundaries. No unsupported generic engine or extra persistence was introduced.

Evidence inspected: preserved root source identity (1306 Rust passes, one ignored; build/Clippy exit 0), all 108 frozen compiled-input hashes match the fixed source, and frozen binary SHA matches `b795f38a…266c9c`. Tests were not independently rerun by this axis. Root process/browser evidence is local fixture evidence, not official CI or supplier certification. Spec-agent findings were not consumed.

Standards findings: **1 P3 heuristic; 0 hard violations**.
