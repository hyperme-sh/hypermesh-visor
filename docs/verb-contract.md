# Desktop verb and capability contract

This audit describes the Linux and native macOS implementations. `GET /capabilities` returns
schema version 1 without opening a session, taking a screenshot, recording audio,
or connecting an MCP server. It uses the same protected Unix sockets and opt-in
TCP bearer authentication as the other routes.

`conditional` means implemented but dependent on device access, services, consent,
and the automation gate. `disabled` means deliberately refused. `unavailable`
means the operation is not implemented by the selected backend. `fixture` means test data, never live desktop access.
Discovery is neither a readiness probe nor an authorization grant. Clients must
handle operation errors even after discovery. Ignore unknown additive fields;
reject unsupported schema versions. There is no capability cache lifetime promise.

```sh
curl --unix-socket "$HYPERMESH_RUNTIME_DIR/control.sock" \
  http://localhost/capabilities
```

Set `HYPERMESH_RUNTIME_DIR` to the daemon's private runtime directory for this
example. Do not make its socket or bearer token available to other users.

## HTTP inventory

Paths below use `/session/{id}` for an existing session. JSON shapes are summarized;
the deserializers in `src/api.rs`, `src/input.rs`, `src/harness.rs`, and
`src/vault.rs` remain the implementation reference.

| Method and path | Input and result | Preconditions or limits |
| --- | --- | --- |
| `GET /capabilities` | Versioned JSON implementation description | No session; normal transport authorization |
| `POST /session` | `purpose`, `recipes`, `agent`, `skills`; optional `secrets`, `mcp_profile`; returns session ID and attached MCP profile/servers | Valid harness and secret sources |
| `DELETE /session/{id}` | Close session | Ends its work and clears session secrets |
| `POST …/view` | No body; PNG bytes | Live session and capture backend |
| `GET …/watch` | SSE capture updates; optional `mode` | Live session; disconnect/close ends streaming |
| `GET …/listen` | PCM audio stream | Live session; PulseAudio-compatible service; s16le, 48 kHz, mono |
| `POST …/mouse` | `action`: move/click/drag/scroll; `x`,`y`; drag adds `to_x`,`to_y`; optional left/right/middle `button`; move accepts `clickable` | Desktop automation gate; scroll adds required `delta_x`,`delta_y` in wheel ticks |
| `POST …/type` | `text`, `keys:[{key,action}]`, or secret handle `secret`; actions tap/down/up | At most 4096 planned keystrokes; Desktop automation gate |
| `POST …/tasks` | 1–16 validated desktop steps; returns task ID and queued status | At most 16 active/waiting jobs; same session and automation gate |
| `GET …/tasks/{task}` | Status, step counts, bounded results and error | Task must belong to the URL session; 1 MiB results per task |
| `DELETE …/tasks/{task}` | Requests cancellation; returns current state | Does not roll back completed steps; held input releases through the normal handler |
| `GET …/feed` | Activity and latest snapshot handle | Does not return secret values or file bytes |
| `GET …/feed/snapshot` | Cached PNG | Does not capture a new frame |
| `POST …/stream` | Newline-delimited prompt/file/secret items; acknowledgement lines | `X-Api-Key`; at most 1 MiB body; prompt items enter prompt routing |
| `GET …/inbox` | Held prompts, files including base64 content, secret handles | Sensitive file content; not a public forum feed |
| `GET …/mcp` | Profile, server health, tool names and schemas | Session tool discovery |
| `POST …/mcp/call` | `server`, `tool`, `decision` approve/deny, optional `arguments` | Explicit tool decision; returns outcome/result |
| `POST …/mcp/resources` | `server`, `action` list/templates/read, `decision` approve/deny, optional `params` | Explicit resource operation; requires advertised resource capability; returns outcome/result |
| `POST …/mcp/prompts` | `server`, `action` list/get, `decision` approve/deny, optional `params` | Explicit template retrieval; requires advertised prompts capability; returns untrusted content without inference |
| `GET /companion` | Desktop/session state, focus and preferred MCP binding | Does not grant execution permission |
| `POST /companion/automation` | `action` | Trusted companion link required to lift a user pause |
| `POST /companion/interrupt`, `/companion/notice` | Companion control messages | See API request types; not task submission APIs |
| `POST /prompt` | JSON `prompt`, `api_key`, and optional `model` | API key is in the JSON body (not `X-Api-Key`); configured routing and model validation |
| `GET /audit`, `/mcp-audit` | Operation metadata | Not secret storage |

Invalid/closed sessions return 404; malformed semantic input generally returns
400. TCP authentication fails with 401 before routing. Root refusal and forbidden
control return 403. Paused/interrupted input returns 409, possibly with an error
string describing the number of keystrokes/pointer steps sent. Disabled Wayland
input and unimplemented secret sources return 501. Backend/MCP failures generally
return 502; unavailable automation conditions can return 503. Framework parsing
errors can have different status/body formats from the API's `{"error":…}`.

## Geometry, cancellation, and retries

X11 input uses root-window pixel coordinates, checked against known root geometry
and X11 coordinate bounds. There is no monitor ID, display enumeration, DPI
transform, screenshot crop transform, or normalized coordinate API. Clients must
not assume CSS pixels equal input pixels. The contract deliberately publishes no
Wayland coordinate promise while injection is disabled (#17).

The checked-in native macOS helper deliberately refuses automation until it has
an authenticated daemon channel. Discovery reports capture and input as
`disabled`, with that restriction, and audio as `unavailable`. It publishes no
usable coordinate contract for that disabled channel. OS permission grants alone
do not enable it; discovery never prompts for permissions or probes the helper.

Input runs in batches under a shared automation gate. Physical input, pause/stop,
session close, and watchdog events can interrupt an operation. Held keys/buttons
are released during unwind. A client HTTP timeout does not cancel an operation.
Queued jobs provide task IDs and cancellation through the [task API](queued-tasks.md);
direct calls have no general request ID or idempotency key.
Do not automatically retry a click, type, drag, or MCP tool call after a timeout or
partial-progress error: observe state and reconcile first. Read-only discovery
can be retried. Watch/listen clients should bound their own consumption time.
MCP transports have a 30-second timeout; this is not a global API execution
deadline. MCP request cancellation closes the transport and suppresses its
response; queued task cancellation uses the task API. See the
[request cancellation contract](queued-tasks.md) for submission cleanup and limits.

## Scrolling

HTTP example: `POST /session/{id}/mouse` with
`{"action":"scroll","x":650,"y":120,"delta_x":0,"delta_y":3}`.
Inverse MCP exposes `mouse_scroll` with the same fields except `action`.
Coordinates target the root-window pixel under the pointer; scrolling does not
activate or focus a window. Deltas are discrete wheel ticks, not pixel distances.
Positive X scrolls right; positive Y scrolls down. Horizontal ticks precede
vertical ticks. All four fields are required integers, and the sum of the absolute
deltas must be 1–128. Application settings determine the distance per tick.

The automation gate, session ownership, coordinate checks, physical-input pause,
and Wayland refusal apply exactly as for other pointer operations. Each tick
finishes its press/release pair before the next batch cancellation check.
Interrupted requests report completed pointer steps: one initial target move plus
one step per wheel tick. Do not replay a partial scroll automatically. A physical
pointer move during a request can affect where already in-flight events land;
pause takes effect at a batch boundary, not retroactively.

## MCP preference and inverse adapter

`GET /companion` reports a preferred healthy configured binding for the focused
app. This is advisory; it does not route an HTTP desktop verb through MCP.
Clients should inspect `GET …/mcp`, choose an appropriate tool, and retain the
same authorization/approval boundary. A failed non-idempotent MCP call must not
silently trigger desktop replay. Discovery and stale-binding work is #29.

The inverse stdio adapter advertises `capabilities`, `view`, `mouse_move`, `mouse_click`, `mouse_scroll`,
`mouse_drag`, `key_chord` and `type_text`, plus `task_submit`, `task_status` and `task_cancel`.
See [queued desktop tasks](queued-tasks.md) for bounds and cancellation.
Desktop operations need an existing session. The read-only
`capabilities` tool requires transport authentication but no session, and returns
the JSON object from `GET /capabilities`. It grants no desktop permission.
Watch, listen and secret handles remain absent from the inverse adapter. `view` returns a base64 PNG MCP image block, preserving the captured bytes over
both Unix and TCP transports (including chunked HTTP). Responses are bounded to
32 MiB; transport operations time out after 30 seconds. TCP redirects are refused.

The eleven tool schemas reject extra arguments. Coordinates are required integers
from 0 to 32767; the visor still checks live root geometry and the automation
gate. Click buttons are exactly `left`, `right`, or `middle` (omission means
`left`). Text is a required nonempty string of at most 4096 Unicode characters;
the backend additionally validates supported characters and planned keystrokes.
Session IDs must parse as UUIDs. Invalid arguments return JSON-RPC `-32602`
before transport access. Backend/transport failures return `isError: true`,
retaining the HTTP status and JSON error detail, including partial progress.
A tool-call notification without a request ID never executes input.

Bounded tasks, result resources, request cancellation and approved verb parity
are described in the [MCP task contract](queued-tasks.md).

## Task classification

“Supported primitive” means the API implements the operation under its documented
backend and authorization conditions. It does not promise that an arbitrary app,
layout or device will accept it. “Excluded” means there is deliberately no public
verb for that operation in the current contract. Platform and service gaps below
remain tracked by their implementation issues; they do not keep this audit open.

| User task | Classification and usable contract | Remaining scope |
| --- | --- | --- |
| Browser, editor or terminal interaction | Supported X11 capture, pointer and typing primitives; app MCP is preferred when an approved matching tool is available | App behavior and compatibility are covered by the scheduled Linux lanes; no generic shell-execution shortcut |
| Scroll a document | Supported bounded wheel ticks on both axes, direct HTTP and inverse MCP | Application scroll interpretation remains app-specific |
| Drag and keyboard shortcuts | Supported bounded drag and named-key chords; held keys/buttons release on interruption | Layout-specific shortcuts require the client's chosen key names |
| Multiple windows | Supported root-coordinate input and best-effort focused-app matching | Semantic window enumeration/activation is excluded from the current API |
| Multiple displays and DPI | Root-window pixels only; geometry and numeric limits are checked | Display IDs, monitor selection, normalized coordinates and DPI transforms are excluded |
| Clipboard | Excluded from the public verb set | No implicit clipboard read/write through another verb |
| File handoff to an assigned session | Supported authenticated stream/inbox bytes and scoped container resource helper | Cross-bot shared-vault grants and materialization: #27/#37 |
| Audio capture | Conditional PCM stream with the documented format | Capture depends on a PulseAudio-compatible service; playback and audio quality guarantees are excluded |
| Native Wayland input | Disabled with an explicit refusal | Portal/libei input and physical-user interruption: #17 |
| Windows/macOS automation | Native automation is unavailable or explicitly disabled | Authenticated native backends and distribution: #18/#25/#26 |
| Container desktop bots | Private X11 image and scoped file-resource prototype | Distribution and remote assignment: #27/#30 |
| Conventional AI client | Eleven inverse MCP tools, queued jobs and session-bound result resources | Protocol/transport behavior is covered by the MCP client contracts; see #32 |
| Shared secrets and executable skills | Local session secret handles are scoped; they are not shared-file grants | Shared read/use/execute policy: #37 |

The audit in #28 is complete when this contract change merges with green CI.
Unit and contract coverage checks capability refusal, argument bounds, structured
errors, HTTP/MCP parity, queued progress, cancellation and resource ownership.
Scheduled application and display scenarios remain regression coverage in the
[central nightly batch](nightly-ci.md), not an issue-closing prerequisite. A
nightly regression is filed as a new issue linked to the relevant closed issue.

## MCP preference and desktop fallback

Focused-app matching uses configured stable identity selectors and deterministic
binding precedence. A matching server must be attached, healthy and have directly
callable tools. The companion reports the winning binding and why a match failed;
see [match diagnostics](mcp-discovery.md#match-diagnostics).

Preference does not authorize execution. App MCP calls still require their
explicit decision, and desktop fallback still requires an active session and the
automation gate. An absent, unhealthy or task-only MCP server permits clients to
choose the normal desktop primitives; Visor does not translate arbitrary tool
calls into shell commands or replay a failed non-idempotent tool through input.
Clients must observe and reconcile partial progress before choosing another path.

MCP catalog refresh: `POST /session/{id}/mcp/refresh` accepts `server` and `action` (`refresh` or `reconnect`). It returns 204 on success, 409 for a concurrent refresh, 404 for a closed session, and 502 on discovery/transport failure. See [MCP discovery](mcp-discovery.md#pagination-refresh-and-reconnect) for bounds, catalog invalidation, and no-replay semantics.

MCP source review is available through the companion menu. `/mcp/discovery` and `/mcp/trust` require the same-user companion socket and cannot grant trust through control or bearer-authenticated TCP. See [MCP discovery and trust](mcp-discovery.md#review-discovery-sources-in-the-companion) for snapshot approval, revocation, and scope.
