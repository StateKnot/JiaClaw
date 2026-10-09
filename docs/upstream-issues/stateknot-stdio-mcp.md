## Consumer and verified gap

JiaClaw (https://github.com/StateKnot/JiaClaw) is implementing personal-agent installation and MCP tool discovery using StateKnot for the agent/protocol layer and Brokerrouter for model access.

Verified against main revision `fbd629d73e50610dc8f4889b47e05f698cc110e7`, `docs/mcp-client.md`: `McpClient` implements stateless Streamable HTTP with an immutable HTTPS or literal-loopback endpoint. No child-process/stdio transport is exposed. Existing issues were searched for stdio before filing.

## Required behavior

A user configures an executable, literal arguments and an explicit environment allowlist. The host starts it without a shell, negotiates the supported protocol version, discovers allowed tools and calls them. Startup failure must not advertise tools. Timeout, cancellation and shutdown must release the entire process tree.

## Proposed scope

- A bounded stdio client or transport-neutral client contract alongside HTTP.
- Explicit lifecycle ownership; no inherited host credentials, automatic installation or shell strings.
- Bounded framing/output/catalog, correlation IDs and handshake deadlines.
- Preserve tool errors separately from transport failures; never automatically retry ambiguous writes.
- Consumer tool allowlist and local schema validation; descriptions and results remain untrusted.
- Document protocol versions, supported platforms, cancellation and orphan recovery.

## Acceptance evidence

Fixture-server contracts for discovery/call, protocol mismatch, malformed/oversized frames, interleaved notifications, stalled handshake, cancellation, child descendants and shutdown. Provide an external consumer example with an exact crate/revision pin.

This is an interoperability gap for local tools, not a claim that the implemented HTTP client is broken. The publication request #92 has been closed after the alpha release; production qualification is still explicitly open in the StateKnot README and roadmap.
