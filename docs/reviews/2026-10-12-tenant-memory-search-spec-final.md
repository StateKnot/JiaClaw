# Spec review: tenant keyword memory search (final)

Base `7fbfe68779ce4f32da94e7f2b010ff23363237ba`; fixed head `4179a8d52d4f58a9fcf047b64e30f6f6d71f8e50`, tree `5fc95ecd47cd33ed388c9e0735a787ce3f0051da`. Fresh review covers all 24 feature paths against `docs/tenant-memory-search.md` and the originating heartbeat requirements. Cumulative origin/main 4b2357f and parent qualification remain inherited, without an exhaustive cumulative rereview.

**0 findings.** The initial P2, violating the advertised "1024 query bytes" after trimming, is closed: restricted execution validates raw UTF8 length before parsing or IO. My unchanged four-case public driver independently reran on the final frozen binary (SHA256 `afca14f0ea4c114637fda55fc4287ff6e6bb0f712ef05d1074e1ae091d3651fc`, session98569, exit0). The same 1206-byte padded query now refuses with known NoEffect and model correction; exact 1024-byte padded and multibyte queries succeed, and 1026-byte unpadded UTF8 refuses. Four committed actual native tasks each preserve captured permission and memory_search authority with exactly two model POSTs. Initial d3 RED and source report remain separate and immutable.

Complete paired default-off permission, configured source membership, old absent hashes/UUID facts, restricted schema and semantic refusal, all-source capture under the original IO/root/writer, unsafe/trailing-source failure, original standalone behavior, cancellation ownership, UI hash/late identity handling and ambient per-turn memory boundaries match the spec. The separately flagged NoEffect-result fixture wrapper now decodes either actual JSON representation correctly; its prior/root reproduction is fixture evidence, not product failure.

I independently checked all 109 compiled inputs against fixed Git and current bytes, plus frozen/target identity. My runtime evidence is the four query tasks; root Rust, other processes/browser suites and external audit-driver checks are not represented as my reruns. Official new-head qualification remains separate. No provider quality, automatic learning, independent ACL, durable execution, public installation or Agent browser certification is inferred.

Spec: 0 findings.
