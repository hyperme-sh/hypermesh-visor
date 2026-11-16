
## Phone Conductor transport

`MobilePhoneClient` uses a native-configured HTTPS Conductor origin and the
existing app-owned machine store. `prepare` retains the exact prompt bytes only
in bounded process memory and returns public operation/task IDs and their digest.
`owned_sessions` returns current owner-assigned interactive sessions. `send_owned`
uses the native owner session to grant this phone access and admit the exact
digest through Plane #181 before submitting through Conductor. Plane rechecks
the current host connection, heartbeat and consent; no second worker claim is
needed. The low-level `submit` method still requires prior owner admission.

`submit` removes the pending request before any network I/O. A failed submission
has an uncertain outcome and is never automatically replayed. Credentials stay
inside Rust and are sent only to the configured Conductor `/v1/phone/prompt-submit`
path; redirects are refused. Responses are bounded and only message text crosses
the binding. Local revocation or changed enrollment refuses a pending submission.
`discard` cancels a prepared request, not an already submitted operation.

The Capacitor conversation view selects these assignments and calls `send_owned`.
Prompt history remains in memory and is cleared on disconnect or selection
change; uncertain sends are not retried. This transport does not supply mesh
provisioning or SEA identity adapters. Those remain in #227/#228,
Infra #1747 and s3r.ch #592.
