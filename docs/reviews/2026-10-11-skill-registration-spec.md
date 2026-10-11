# Spec

Fixed cumulative baseline: `4b2357fcf01f47ba08d7724edbba7accfb60972c`. Immutable source: `b1c7c901c91929a5ce52ff2a5cc6ffb9de3e41a7`; parent: `5c9f93f68050fa92563c61ffcb0af39af63c4177`. Canonical comparison is `git diff baseline...source`; commit list: `/tmp/jiaclaw-oct11-registration-cumulative-commits.txt`. Inherited cumulative conclusions remain outside this fresh review. I reviewed only `git diff parent...source -- .github/workflows/ci.yml .github/workflows/release.yml crates/jiaclaw-host/src/main.rs crates/jiaclaw/src/skills.rs crates/jiaclaw/src/skills/lock.rs docs/skill-registration.md tests/skill_registration.py`, with inherited skills/source-approval/memory-I/O contracts. No Standards reports were consulted.

**0 findings:** no missing/partial requirement, unrequested behavior, or incorrectly implemented requirement found in this delta.

- `docs/skill-registration.md:3` requires an existing required lock, an unregistered local directory and explicit reviewed source/hash. The CLI requires all fields; registration validates the existing captured lock and target through the existing parser and capability reader.
- Line 5 requires the shared writer, disabled insertion, v1→v2 preservation and complete candidate validation. The edit stays inside `update_existing_text`; only the new declaration is added, disabled, and the serialized candidate is parsed and strictly scanned before private sync/rename publication.
- Line 7 requires refusal before publication to retain original bytes/inode and staging cleanup. Independent real CLI checks confirmed these effects for rogue unregistered content, directory links and capacity violations.
- Lines 9 and 11 require disk-only output without activation, reload, installation or widened write authority. The new command returns `runtime_applied:false` and introduces no runtime/HTTP/model registration entry.

Independent frozen-binary evidence: `/tmp/jiaclaw-oct11-registration-spec-boundaries.py` and `.log`, seven groups passed: mixed v2 states/origins, another unregistered body, target/root directory links, 63→64/65 declarations, 256/257 entries, exact 128KiB/+1 target, exact 2MiB/excess enabled catalog. Binary SHA256: `cea8a99ae3fc64185623977224cf5bd769967066d5b6ab3214c5ace986784187`. `df` preceded tests (about 259GiB available); no compilation occurred. Two earlier fixture errors were preserved separately: omitted required config field and oversized fallback description, corrected only in `/tmp`.

This is local source/CLI evidence. Official source qualification remains pending official CI after the eventual new PR; it establishes no supplier or installation certification.
