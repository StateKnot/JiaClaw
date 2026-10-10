# Skill source lock — independent Standards source follow-up

Fixed cumulative base `4b2357fcf01f47ba08d7724edbba7accfb60972c`; source head `2a5e4c327ddd563215882c13b6982f726eaaa798`. Fresh whole batch is `68554879f365698b0e0f816f76cd0a52d7a5b43b..2a5e4c327ddd563215882c13b6982f726eaaa798`, twelve files. This follow-up independently reviews `a952c96e31b052545a81afc3fb7bec6d1470a602..2a5e4c327ddd563215882c13b6982f726eaaa798`, three files; initial and preceding final reports are inherited and preserved. No exhaustive cumulative rereview is claimed.

**Zero open hard Standards violations and zero actionable Fowler findings.** User production, authority and evidence standards plus documented module boundaries apply. All twelve Fowler heuristics considered as judgments; tooling-enforced rules excluded.

The embedded OpenAPI now describes the actual optional `SkillInfo.source` field with a named `SkillSourcePin` schema. Required source fields and lower-case Git/digest shapes are explicit; origin byte/credential limits are described without inventing publisher authentication. Optional rather than nullable matches omission on unlocked catalogs. The shared schema avoids duplicating the nested record across response shapes.

Public field comments clarify the source declaration, immutable Git object ID and complete original-file hash, rather than exposing internal parser details. Existing lock enforcement, caller permissions and registry lifecycle code remain unchanged.

The real authenticated HTTP fixture asserts the served schema reference and required fields before checking live source metadata. The preserved negative log `/tmp/jiaclaw-oct11-lock-openapi-negative.log` shows the frozen earlier binary failed with absent `source`; this is actual contract-discovery evidence, not an unrelated network or timing error.

Because OpenAPI is embedded compiler input, the earlier frozen binary and prior six-group result do not qualify this new source head. Rebuild and final process/official qualification remain separately pending at review time. Reviewer ran no compilation, tests or network calls and made no production edits.

Final Standards count: **0 open findings; no worst issue**. Spec and final source-identity/CI verification remain separate axes of evidence.


---

# Skill source lock — final independent Standards review

Fixed cumulative base `4b2357fcf01f47ba08d7724edbba7accfb60972c`; final source head `a952c96e31b052545a81afc3fb7bec6d1470a602`. Fresh full batch `68554879f365698b0e0f816f76cd0a52d7a5b43b..a952c96e31b052545a81afc3fb7bec6d1470a602` covers eleven files. Final focus is `8e14f484b494fa8895d3e95eb2f77528eba64508..a952c96e31b052545a81afc3fb7bec6d1470a602`, four fixture/documentation files. Initial report and inherited architecture/skill reviews remain distinct; this is not an exhaustive cumulative rereview.

**Zero open hard Standards violations and zero actionable Fowler findings.** User production, authorization, resource/cache and evidence requirements apply. All twelve Fowler heuristics were considered as judgments; tooling-enforced matters excluded.

The fixture repair uses actual `ChatRequest.messages`/`enabled_tools` and `ChatResponse.message.content`, preserving the three native model rounds and exact original tool set. It does not weaken the authority, loaded-byte or response assertions to accommodate a mismatch. Required-startup, diagnostic, exact catalog, safe file, reload publication and unlocked-compatibility groups remain unchanged.

Documentation connects the opt-in flag from configuration and the existing skills guide to the concrete lock contract. It explicitly limits the lock to administrator declarations and exact content, with existing workspace write authority and no independent approval/ACL guarantee. Read-only deployment is specified where lock immutability is required; hash is not promoted to publisher authentication. This matches the production design boundary rather than adding a hypothetical security mechanism.

The private lock module and registry retain the initial acceptable boundaries: bounded capability reads, parsing without credential echo, hashes from parsed raw bytes, immutable reload policy and atomic prior-table retention. No new production changes occur in the final focus.

Independently read `/tmp/jiaclaw-oct11-lock-process-settled-final.log`: six expected groups pass, including actual native body/reference reads. Reviewer did not execute them or compile. Initial wrong-field logs, local Rust evidence and eventual official fixed-head qualification remain separate; review is not supplier or release certification.

Final axis count: **0 open Standards findings; no worst issue**. Spec remains an independent axis.
