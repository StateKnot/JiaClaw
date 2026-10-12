# Explicit tenant document tasks

This opt-in extends only personal tracked HTTP turns. It consumes the existing
`file_read` tool, capability-root reader, eight nonqueueing blocking I/O slots,
native schema/batch preflight, model-call ledger, original turn owner and gateway
identity/hold/SSE contracts. It adds no file engine, model tool or storage.

Both administrators must configure exactly the same grant. Private backend:

```toml
[http.tenant_file_read]
paths = ["tenant-documents/notes.txt", "tenant-documents/资料.txt"]
```

Gateway's corresponding `backends` entry:

```json
{"id":"alice","url":"http://127.0.0.1:9001/","token_file":"/private/alice.token",
 "tenant_file_read":{"paths":["tenant-documents/notes.txt","tenant-documents/资料.txt"]}}
```

Omission is disabled. Requires original authenticated, persistent Brokerrouter
tracked tenant HTTP configuration, an enabled read_file tool, and gateway
tracked_turns. Startup rejects empty/invalid grants or mismatched/missing backend
capability. New admission repeats the exact authenticated handshake before the
registry identity/hold and any backend PUT. SSE protocol stays 2; separate file
permission protocol is exactly 1. Unconfigured legacy shape stays unchanged;
older clients fail closed on the extended capability, and older backends cannot
enable the new grant. No schema migration or automatic permission upgrade.

Grant 1..32 **sorted unique exact files**, each at most 256 UTF-8 bytes/16 path
components, only beneath `tenant-documents/`. Empty, hidden, control, backslash,
dot/parent, repeated slash and whitespace aliases are rejected. This is not a
directory or recursive grant. Administrators deliberately place reviewed input
there; they must not place credentials or private ledger copies there. These
permissions do not authenticate publishers or document truth. Missing files are
not created; unsafe parents/leaves, symbolic/hard links, FIFO/devices/directories,
invalid UTF-8/NUL and over-budget sources fail through the original reader.

The authenticated capability exposes complete paths, byte limit 262144 and
`policy_sha256 = SHA256(JSON([1, paths, 262144]))`. This hash identifies the path
permission, **not source bytes, a policy generation or ABA**. File content remains
mutable under the operator-owned root. Existing full-source byte checks happen
before optional offset/limit line selection. Original line slicing, newline
normalization and result-wire budgets remain; an over-budget encoded tool result
still requires review, and is not silently accepted as complete.

Select file_read explicitly with the exact current policy hash:

```json
{"session_id":"http:3b80c58b-e495-4fd8-a77e-b1579576471f",
 "prompt":"Read tenant-documents/notes.txt and summarize it",
 "enabled_tools":["file_read"],"enabled_skills":[],
 "file_read_policy_sha256":"<64 lowercase hex characters from capability>"}
```

Without file_read the field must be absent/null; omitted serialization preserves
all legacy body fingerprints. File requests' hash participates in original
permanent submission identity; backend context hash also captures immutable
process configuration. The restricted tool advertises exact enum paths and
rejects aliases independently at execution. Complete model-call batches are
preflighted before any call. read_file, file_write/exec/MCP/memory/skills remain
outside this tenant allowlist. Channels and cron retain datetime_now/json_query.

Policy changes require stopping/reconfiguring both processes. New requests with
old grants reject before model dispatch; old UUIDs remain GET-only facts with
their original hashes/results, even after permission withdrawal. Revoked/disabled
users and read-only Keys cannot create/cancel turns; original registered user,
backend, body, hold, budget and capacity rules remain. Existing facts do not
authorize new reads or resume a tool graph. Changing an operator-owned file or
config cannot retract bytes already sent to a model or saved in a session/result.

The workbench validates the complete grant and, on a secure page, verifies its
SHA using WebCrypto; displays the full path list with textContent; and offers an
unchecked file_read selection. Selection captures the current hash in the exact
body. No implicit directory probe or browser storage. Original identity clearing,
late-response rejection, finite SSE delivery, explicit original GET and retry
contracts apply. Capability reads remain bounded to 32 KiB/15 seconds in the page and 32 KiB/5 seconds in the private handshake. Historical file result validation does not require a currently
enabled grant, but a tracked attempt's result must match its captured tools.

Cancellation stops future dispatch; submitted model operations settle by original
remote ID. A timed-out/cancelled read waiter cannot release the blocking I/O slot
before the actual read ends. This is a bounded blocking owner, not OS read
preemption or a snapshot across administrator edits. The read has NoEffect;
there is no new workspace writer or document publication transaction.

Fixtures qualify actual local HTTP/model/tool/SQLite mechanics, isolation,
policy withdrawal, unsafe source refusal and read cancellation. They do not
certify paid suppliers, publisher provenance, business summary quality, real
release installation, Agent browser tools, physical power failure or StateKnot
durable. Those milestones remain independently open.

Ordinary known read errors retain the existing NoEffect model-correction path.
A successful fixture final reply echoing that error certifies the refusal, not a
successful document read. The original read tool also classifies its timer error
NoEffect: it cannot mutate the source, and its blocking slot remains owned. An
explicit cancellation stops future model/tool dispatch while that read settles.
Initial fixture assumptions and compile/driver failures are retained
separately from final production evidence.
