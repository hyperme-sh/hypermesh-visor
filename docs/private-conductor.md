# Private Conductor routing

The daemon and native macOS prompt configuration select the current user's
Conductor socket by default. Linux resolves it under
`$XDG_CACHE_HOME/hypermesh/conductor/run/conductor.sock`, falling back to
`$HOME/.cache`. macOS uses `$HOME/Library/Caches` with the same suffix.
`HYPERMESH_CONDUCTOR_SOCKET` can select another absolute private path.

Before sending a credential or prompt, Visor checks the immediate directory and
socket owner and permissions, then authenticates the connected kernel peer.
Missing, unsafe, or unavailable sockets return an error. No direct provider
fallback is selected. Explicit supervisor routes and the legacy Conductor
address setting remain opt-in; a socket cannot be combined with another route.

The Linux managed bundle passes a private Unix listener to its Conductor child.
The parent retains the descriptor, checks startup, observes child failure, and
reaps the child and removes its own socket on shutdown. It refuses an explicit
socket or address alongside managed configuration.

Requests preserve the selected model and lease task ID, generate a unique step,
and report measured usage. Request cancellation, session closure, and daemon
shutdown cancel socket work, including calls waiting for the prompt gate.
Cancelled calls are not retried. Recipe and budget enforcement remain in
Conductor. The production model-review adapter requires authenticated HTTPS and
its dedicated tenant credential; an unavailable authority refuses dispatch.

## Platform scope

| Package | Conductor connection | Child management |
| --- | --- | --- |
| Linux desktop and headless | Private Unix socket by default | Bundled managed child when configured |
| Native macOS development app | Same-user Unix socket by default | Independently started Conductor; app child isolation remains pending |
| Windows and mobile session clients | Remote session API | No local Conductor runtime |

The native macOS app is not yet a complete bundled Conductor distribution.
Issue #90 remains open for that packaging and isolated startup work.

Validation is covered by socket peer and permission contracts, measured-usage
and lease forwarding contracts, cancellation tests, native configuration tests,
and managed-socket cleanup tests. Native macOS kernel checks run in CI.
