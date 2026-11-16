
## Phone Conductor transport

`MobilePhoneClient` uses a native-configured HTTPS Conductor origin and the
existing app-owned machine store. `prepare` retains the exact prompt bytes only
in bounded process memory and returns public operation/task IDs and their digest.
The owner must grant the phone access to the current remote session and dispatch
that exact digest through Plane; the remote host must accept the task before
`submit` can be admitted. This object does not grant or claim tasks itself.

`submit` removes the pending request before any network I/O. A failed submission
has an uncertain outcome and is never automatically replayed. Credentials stay
inside Rust and are sent only to the configured Conductor `/v1/phone/prompt-submit`
path; redirects are refused. Responses are bounded and only message text crosses
the binding. Local revocation or changed enrollment refuses a pending submission.
`discard` cancels a prepared request, not an already submitted operation.

This transport does not supply mesh provisioning, remote task selection, SEA
identity adapters, or the complete conversation UI. Those remain in #227/#228,
Infra #1747 and s3r.ch #592.
