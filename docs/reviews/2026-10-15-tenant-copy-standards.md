# Standards — exact tenant copy source

0 hard documented violations and0 actionable baseline smells in the fresh18-path diff `d8334145bb073ac3db707667c373ddc5b2626eb7...2c902b70482353ad75855b3946389cc74393f764` (tree327b1f2e29a9e6cb5f7a75751d2e1070c9cea5a6).

Reviewed user AGENTS, `docs/tenant-copy.md`, native-tool/failure-effects, tenant-file-write and architecture contracts, repository lint policy and the complete production/fixture/workflow diff. Mandatory repository searches found no AGENTS/CONTRIBUTING/CODING_STANDARDS file. Tooling-enforced issues were excluded; smell heuristics were evaluated as judgement calls.

The change uses a small explicit pair type, canonical sorted complete grants and the original copy engine/I/O owner/writer rather than a parallel implementation. It preserves create-if-absent and Unknown effects, denies recombination/aliases/overwrite, restricts the registered alias surface, compares complete paired capabilities before admission, hashes only selected authority, and clears private UI state across identities. Cross-boundary validators and existing capability plumbing are justified rather than actionable duplicated-code/shotgun-surgery smells.

My131 independent checks reconstructed all124 compiler inputs and matched source/current/frozen bindings. Frozen binary SHA256 is `113620173e7e8a2a4ce5a950498175bfda59da484ba47645df6c60a7053380c1`; manifest is `bba57a89db04af737f01f0deeebf9eb167adefef89da45bcb6762e63e86f3a76`.

Own ordinary tenant-copy acceptance exited0/all six groups: two actual isolated model/tool/physical-binary copies, source retention/no replay, forbidden combinations/schema, readonly/revocation, original unsafe-file guards, complete64MiB acceptance/oversize refusal, writer contention, cancellation and retained Unknown holds. Own Node parser17 also exited0. Fresh df readings preceded both; the shared cache was assessed/retained, with no build/cache copy/cleanup or paid call. Raw logs, commands, exits and hashes are preserved in this review directory.

This is source Standards only. Parent152's qualified official evidence, local root1335 tests and cumulative4b are inherited scope rather than fresh exhaustive review. This does not certify full187 official qualification, supplier integration, business quality, Agent browser or durable execution. External-driver checks/findings are separate.
