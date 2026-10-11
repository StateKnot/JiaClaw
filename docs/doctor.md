# `jiaclaw doctor`

`doctor` reports configuration readiness without guessing that JiaClaw will fall back to a demo provider.

## Default: local, read-only checks

```sh
jiaclaw doctor --config "$HOME/.jiaclaw/config.toml"
```

The default command reads the selected config and workspace, validates local tool construction, MCP endpoint/allowlist policy, and required credential environment-variable references. It does not contact MCP servers, send model or embedding requests, open private SQLite stores, execute tools, or write to the workspace. Provider credentials are only checked for presence; values are never printed. Provider URLs are summarized as scheme/host/explicit port, omitting user information, path, query, and fragment.

Missing required provider credentials, an invalid provider endpoint, invalid local tool configuration, an invalid MCP policy, a missing or invalid configured MCP bearer value, or an uninitialized workspace makes the command exit nonzero. Provider URLs must satisfy the same HTTPS or literal-loopback HTTP contract used at runtime; endpoint credentials, query strings, and fragments are rejected. MCP bearer values are checked against the header credential contract without printing them. Optional channel credentials and hardening recommendations remain warnings. To run offline, explicitly set `provider_type = "stub"`; `doctor` never chooses stub mode on the operator's behalf.

## Explicit network and storage initialization

```sh
jiaclaw doctor --connect --config "$HOME/.jiaclaw/config.toml"
```

`--connect` opts into MCP discovery and initialization of enabled private model-call and semantic-memory stores. It does not submit model or embedding requests and does not invoke discovered tools. It can create private SQLite files and send requests to the configured MCP endpoints, so review the config and endpoint permissions first. The process returns nonzero when initialization or an opted-in MCP connection fails.

The MCP check only establishes that the configured discovery contract is reachable and that the reviewed allowlist can be installed. It does not certify a production supplier, provider credential, durable execution/recovery behavior, or the effect of a remote tool implementation.

## Capability status and session storage

Provider and integration status describe the actual configured path. Explicit stub mode does not require a model key. An invalid endpoint cannot also display a successful endpoint check, and the Brokerrouter section is labeled a local configuration check, not a network connection test.

StateKnot HTTP MCP is integrated; its configured server count is shown separately from the discovery result above. StateKnot durable graph execution is not integrated. The application can persist session history in SQLite when `[http] persist = true`; when disabled, sessions are in memory and do not survive a restart. Neither session history nor model-call receipts provide resumable tool execution.

`doctor` reports session storage configuration only. Even `--connect` does not open, migrate, lock, or create the HTTP session database: it initializes only the enabled Agent private stores described above. Session database availability and recovery require their separate serve/deployment checks. A successful diagnostic does not certify that session storage or a supplier is operational.

For an explicitly billed, no-tools model response check, stop the service using the same private model-call ledger and run [`model-probe --config ... --confirm-billing`](model-probe.md). This separate command persists the original request/receipt and retains uncertain outcomes for inspection. Its success verifies that diagnostic response only; it does not certify Agent tools, channels or a supplier deployment.
