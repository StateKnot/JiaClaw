# Personal Key: create an exact directory

A user who explicitly selects `mkdir` for a new tracked HTTP task can create an administrator-approved directory, then use separately approved write/copy/move paths inside it. This uses the existing `WorkspaceMkdirTool`, directory handles, eight nonqueue blocking-I/O owners and cooperative workspace writer. No second filesystem engine, Store, model tool or dependency is added.

## Configuration and authority

Both the native backend `http.tenant_mkdir` and matching `gateway.backends[].tenant_mkdir` must explicitly contain the same `{"paths":["tenant-documents/deliveries"]}`. Both default to absent. Native `tools.mkdir.enabled`, tracked turns and tenant mode must also be enabled. Paths are 1..32 sorted unique exact workspace-relative names under `tenant-documents/`, each at most 256 UTF-8 bytes and 16 components, using the existing file-grant canonical rules: no hidden, empty, control, backslash, dot-parent, whitespace or BOM aliases. Permission does not assert that a directory or its parent exists, authenticate inode identity, source, ABA, independent OS ACLs or administrator/exec restrictions.

The permission descriptor is exactly `protocol:1`, full `paths`, `mode:"single_directory"`, and `policy_sha256`: lowercase SHA-256 of canonical UTF-8 JSON `[1,paths,"single_directory"]`. It is independent of copy, move, file read/write and SSE protocol 2. The gateway authenticates the complete native contract at startup and before every new admission, before recording identity/hold or dispatching the backend PUT. Reconfiguration requires stopping and restarting both ends. Old UUID GET and matching repeated PUT return the original facts, without redispatch. Unselected requests omit the new hash; their original submission hashes remain unchanged.

## Execution and settlement

The restricted schema accepts only required `path`, enumerating complete granted names. Extra `recursive`, `parents` or any other keys, aliases, child paths and another user's paths refuse. Execute rechecks exact membership, forces `recursive=false` and invokes the original mkdir engine once. Parent directories must already exist. It never implicitly creates an ungranted ancestor. An existing ordinary directory is idempotent success (`created:false`, `existed:true`); an absent leaf is created and synchronized with its parent. Existing files, symlinks, hardlinks, FIFOs/special leaves and unsafe ancestors refuse under the original handle-based no-follow rules. No file body is read or disclosed. Standalone mkdir retains its original recursive/parents behavior.

Failures, timeouts and cancellation retain the original `Unknown` effects contract: an error may follow directory creation or synchronization, so no automatic retry, rollback or hold clearance is introduced. The actual blocking owner retains the I/O permit and writer until settlement. Explicit cancel stops future dispatch; it is neither OS hard preemption nor deletion of directories already created. No atomic multi-task snapshot, crash durability beyond the original sync contract, or protection against noncooperative administrators is claimed.

## Workbench and privacy

The workbench strictly validates the complete descriptor, canonical path list and WebCrypto SHA in a secure context. The `mkdir` checkbox defaults unchecked; full paths are shown with `textContent`. Sending explicitly captures the hash. Identity changes clear private scope, results and selection; late old-identity Crypto cannot restore them or dispatch. Readonly keys can inspect their original facts but cannot create tasks; revoked/cross-user keys refuse. There is no browser storage, polling, automatic model request or new route. Channel and cron retain their original two tools.

Directory names and mkdir result metadata can enter this user's configured model and original persistent conversation after explicit Send. Ambient SOUL/USER/MEMORY remains independent. This workbench browser is not an Agent browser tool; fixtures do not certify supplier access, real business quality, learning or StateKnot durable execution.

## Required acceptance

`tenant_mkdir` Rust tests cover original engine reuse, nonrecursive authority, exact schema and canonical permission. `python3 tests/tenant_mkdir.py target/debug/jiaclaw` exercises real native/gateway/model/SQLite and OS effects for two users, creation plus inode-preserving idempotence, old UUID zero replay, missing parents, invalid schema, unsafe leaves/ancestors, writer ownership, Unknown holds, cancel, capability revocation and key boundaries. `--browser` adds a separate actual Chromium invocation covering explicit Send, readonly/identity, mobile layout, late Crypto and mismatched digest. Parser 19 validates the descriptor and saved-tool facts. Original deadlines and all inherited CI acceptance remain unchanged. This feature requires its own fixed-head official qualification; parent PR154 qualification is historical only.
