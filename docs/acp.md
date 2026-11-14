# ACP adapter boundary

The `hypermesh-visor-client::acp` library implements the ACP v1 admission and
stdio turn state for #188. The `hypermesh-acp` executable exposes it on stdio. No production
backend or network listener is enabled. The existing inverse MCP executable
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
512 KiB, the output queue to 16 messages, text/output to 64 KiB and backend work
and stalled output to 30 seconds. Transport and backend futures own cancellable
I/O; dropping them releases their handles without detached threads. Embedding callers must supply cancellable I/O handles. The standalone
executable owns its portable stdio workers and exits the whole process after
the transport drops active work, rather than waiting for blocked stdio workers.
It must not be embedded as an in-process shell.
A backend must bound its own I/O and honor cancellation; closing
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
added. Production routing belongs to #219 and Conductor #64. #188 explicitly uses
a fixture backend and does not depend on that production route.

References: [ACP initialization](https://agentclientprotocol.com/protocol/v1/initialization),
[session setup](https://agentclientprotocol.com/protocol/v1/session-setup),
[prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn),
[cancellation](https://agentclientprotocol.com/protocol/v1/cancellation) and
[stdio transport](https://agentclientprotocol.com/protocol/v1/transports).

## Runnable fixture adapter

Desktop Linux packages, native Windows/macOS bundles and the desktop container
include `hypermesh-acp`. An ACP editor may launch it with
`--fixture-assignment /absolute/private/assignment.json`. The owner-private JSON
contains `machine` and `session` UUIDs, plus boolean `assigned` and `paused`
fields. It is a test fixture, never an enrollment credential or a production
assignment grant. With no explicit fixture, the executable refuses startup.

Fixture turns return only a fixed response and perform no model, tool or desktop
actions. Admission and every turn check assignment, machine/session identity
and pause; changes are reread during a turn. Cancellation suppresses late output.
The executable contracts exercise initialize, admission, a turn, cancellation,
wrong-session and pause refusal, and bounded EOF exit without logging prompts.
These are unit/contract fixtures. Installed-package execution belongs only to
the central nightly owner registration.
