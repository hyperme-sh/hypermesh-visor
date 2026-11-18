# Continue tasks

Open `hypermesh-companion --continue-workspace /absolute/project` for the current
assigned workspace. The task form captures the goal/central issue, completion
criteria, declared sequential chunks and explicit time/token limits. Continue is
on by default for the new task; choosing off stores it paused in Plane.

The task view reads the authoritative selected task after reconnect. It displays
state and stop reason, current/next chunk, counters and deadline. Refresh reads
new state; toggles, pause, resume and stop carry the displayed revision and only
show acknowledged owner state. A stale mutation refuses and asks for refresh.
Scope and limits are fixed per task: stop it using the current revision before
starting a replacement. A cancelled uncertain chunk is never resubmitted.

This initial contract runs bounded coding model turns through Conductor and
AIuditor and checks for a complete nonempty text response. Generated output is
not a shell script or authority to expand tools. Continue does not bypass local
consent, current user/org policy or publication authorization.

The local `/continue/start`, `/continue/view` and `/continue/control` routes are
private companion-socket APIs. They bind workspace/session to the current Host
context and use the existing owner session for Plane controls. Neither native
UI nor IDE carries owner or machine credentials. Task IDs derive stable chunk
and operation IDs; refresh after an uncertain submission before starting again.

Typing in the form may pause local automation. Preparing the task never resumes
that pause. Status and cancellation stay available through scope-only context.
When ready, choose **Resume local execution for this session**: it uses the
existing selected-session and current-epoch check, then closes the dialog.
An intervening pause or session change refuses the resume. Execution still waits
for the existing current Plane assignment and local consent heartbeat.
