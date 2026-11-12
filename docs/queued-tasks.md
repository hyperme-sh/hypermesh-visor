# Queued desktop tasks

A conventional AI client can queue bounded desktop work for its existing assigned
session. The client chooses the steps; Visor executes them through the same verb
handlers used by direct calls. This API does not interpret a natural-language
prompt or create a new model executor.

`POST /session/{session}/tasks` accepts `steps`, an array of 1–16 objects with
`tool` and `arguments`. Supported tools are `view`, `mouse_move`, `mouse_click`,
`mouse_scroll`, `mouse_drag` and `type_text`, with their inverse-MCP argument schemas. Unknown
operations, extra fields and invalid arguments fail before queue insertion.
The response is HTTP 202 with a task ID and `queued` status.

```json
{"steps":[{"tool":"type_text","arguments":{"text":"hello"}},{"tool":"view","arguments":{}}]}
```

Read `GET /session/{session}/tasks/{task}` for status, completed/total step counts,
results and an optional error. `DELETE` on that path requests cancellation.
A queued job becomes `cancelled`; a running job keeps `running` with
`cancel_requested: true` until its current handler returns. Input cancellation is
checked between input chunks and releases held keys/buttons through the existing
handler. An already-running capture can finish; cancellation stops later steps.
Completed steps are never undone or replayed.

The queue admits at most 16 active/waiting workers and executes one job at a time.
It retains at most 16 task records, evicting the oldest terminal record when a new
submission needs space. Results are bounded to 1 MiB per task; an individual PNG
must be at most 512 KiB. A 120-second deadline requests cancellation, including
queue wait time. It does not claim to kill an unresponsive native backend before
that backend returns. A full queue returns 409. Treat POST as non-idempotent:
inspect its returned task ID instead of blindly replaying a submission.

Normal transport authentication is required. IDs are checked against the session
in the URL; another live session cannot inspect or cancel them. Closing a session
cancels its jobs and removes its stored results. Jobs do not survive daemon
restart. Physical-user pause, companion freshness, backend restrictions and
input validation still apply. Tasks cannot start/resume automation, fetch
secrets, invoke a shell or select another session.

The inverse MCP adapter exposes `task_submit`, `task_status` and `task_cancel`.
All bind to `HYPERMESH_SESSION_ID`. Status includes images as MCP image blocks;
the JSON result summary identifies each image's `content_index`. Normal stdio
disconnect requests cancellation only for tasks created by that connection.
Abrupt process loss still relies on the daemon deadline or session closure.
The adapter tracks at most 16 tasks whose terminal status it has not observed.
At that limit, `task_submit` returns a tool error without submitting work or
forgetting an older task. Poll `task_status` or use `task_cancel` and confirm a
terminal status to free a slot; requesting cancellation of a running task alone
does not free its slot. A daemon-confirmed missing record also frees its slot:
another client can cause completed records to be evicted before they are polled.
Denial, proxy errors and transport failures retain tracking. This limit is
separate from the daemon's active queue.
`capabilities` returns the authenticated daemon contract without requiring a
session or granting desktop access.

The stdio adapter negotiates MCP `2024-11-05`. Clients send a valid `initialize`
request followed by `notifications/initialized` before listing or calling tools.
For another requested version, the server returns its supported version; clients
that cannot use it should disconnect. Ping requests work during setup. Invalid
JSON-RPC envelopes and malformed initialization requests receive protocol errors;
notifications never produce responses or execute tools.

`notifications/cancelled` does not interrupt a synchronous desktop HTTP call.
Such calls retain their transport timeout and existing user-pause controls.
Use queued jobs and `task_cancel` for interruptible multi-step work. A cancellation
notification for an already-completed RPC does not cancel the task that RPC
created. General in-flight RPC interruption remains a gap in #32.

Each stdio request is limited to 512 KiB. An oversized line ends the connection
after a protocol error and requests cleanup of its queued jobs.

`mouse_drag` requires `x`, `y`, `to_x` and `to_y` in root-window pixels,
all integers from 0 to 32767. Its optional `button` is `left`, `right` or
`middle` and defaults to `left`. Queued drags use the same mouse handler,
physical-user pause and held-button release behavior as direct HTTP drags.
