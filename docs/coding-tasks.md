# Queued coding work

The existing outbound bot worker may consume an owner-admitted coding request
through the configured Host machine bridge. Plane #154 owns task admission and
private claim delivery; Conductor performs the exact machine-ingress submission
through its protected socket and AIuditor-reviewed routing. Host configuration
selects the workspace and model route. Neither repository text nor model output
can choose a different host, workspace, credential or executable.

The coding consumer runs alongside the existing heartbeat worker. It requires
current local consent for the bound session, cancels its Host request when that
consent changes, discards late output, and does not replay an uncertain claim.
Plane retains task status across restarts; no local queue is created.

`GET /coding/status` on the private companion socket shows only the last scoped
task ID, output hash and observation state. `output-observed` means Conductor
returned a bounded response; Plane remains the authority for durable completion.
No prompt or generated code is retained in this local view. A changed owner,
organization, machine, assignment, session or workspace cannot read the prior
view. Native shared checks remain CI-only.
