# ACP adapter boundary

The `hypermesh-visor-client::acp` library implements the ACP v1 admission and
stdio turn state for #188. It is not yet a runnable editor adapter: no production
backend, executable or listener is enabled. The existing inverse MCP executable
is unchanged. Native session units exercise the new code on Linux, Windows and
macOS in CI.

The protocol requires initialization, then one conversation attached to an
independently authorized machine/session backend. `session/new` does not create
a Visor desktop session or approve desktop input. The supplied working directory
is checked but never opened; nonempty client MCP server lists are rejected.
Client capabilities, annotations and metadata do not grant filesystem, terminal,
network or tool access. Text and resource-link prompts are supported; links
become untrusted references in the prompt and are never fetched. Image, audio and
embedded content are refused. Session history loading is not advertised.

Only one prompt can be active. Cancellation is scoped to the bound session or
active request ID, reaches the backend, and suppresses a late response body.
EOF, invalid or oversized framing, queue overflow, timeout and output failure
close the connection and signal cancellation. Input frames are limited to
512 KiB, the pending queue to 16 frames, text/output to 64 KiB and backend work
to 30 seconds. A backend must bound its own I/O and honor cancellation; closing
the connection cannot retract an operation already accepted by another service.
Unknown outcomes are not retried. Errors contain fixed text, not backend
diagnostics or prompt bodies. The backend must likewise avoid logging sensitive
inputs, including in panic messages.

## Required production backend

The backend must verify the enrolled machine identity, current assignment and
local session on admission and every prompt. It must preserve physical-user
pause, bind requests to that session and an operation ID, and route through
Conductor, AIuditor and the destination. Cancellation, expiry, revocation and
disconnect must reach active upstream work. A successful protocol handshake or
an earlier successful admission is not a standing grant.

The existing Visor `/prompt` and session stream routes use the renter API-key
path. Conductor's current `/v1/peer/prompt-submit` takes an operator-configured
bearer grant and destination/class; it does not establish the machine proof or
assigned Visor session required by #188. Neither is a substitute for the missing
machine/session ingress contract. No manual JWT or renter-token fallback is
added. #188 remains open until that contract and the executable adapter are wired,
packaged and tested. The protocol library alone does not complete it.

References: [ACP initialization](https://agentclientprotocol.com/protocol/v1/initialization),
[session setup](https://agentclientprotocol.com/protocol/v1/session-setup),
[prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn),
[cancellation](https://agentclientprotocol.com/protocol/v1/cancellation) and
[stdio transport](https://agentclientprotocol.com/protocol/v1/transports).
