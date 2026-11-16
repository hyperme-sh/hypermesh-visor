# Isolated companion bot broker

The native Linux companion can enroll a managed desktop, resume it, stop it,
and clone or restore its configuration through the companion menu. The signed-in
owner authorizes management through Plane. Plane #149 checks the current source
grant atomically when creating a fresh paused bot for clone or restoration.
Revoked or expired source grants refuse the operation.

Bot credentials live only in the private manager directory's broker.json file.
Owner access tokens are transient and never persisted. The Docker guest keeps
network=none and receives no owner token, bot credential or inherited session.
Every recreation gets a new local session, container, home, remote bot and remote
assignment. Saved homes are never attached to another identity.

Enrollment saves the new paused credential before requesting an assignment.
An interrupted assignment is not retried automatically. It cannot run; the owner
can inspect or revoke that paused bot through Plane and stop its local desktop.
An uncertain recreation is likewise never retried automatically. Plane remembers
the operation and refuses duplicate requests. An unacknowledged remote identity
remains paused and never has local input authority.

Local resume requires a fresh physical-input-bound confirmation and a current
Plane grant. An independent lease publisher stops granting guest input when the
remote check expires, even if Docker or an owner request blocks the manager.
Physical host input still revokes all local permits immediately. Reconnection
uses a fresh connection ID so delivered tasks become unknown rather than replayed.

The broker claims only desktop tasks for the exact bot, tenant, owner, session,
assignment and connection. Tasks retain Plane's absolute authorization deadline.
They pass through anonymous private stdin to fixed bridge code and the existing
leased task endpoint; no task text enters Docker arguments. Status and completion
are relayed to Plane. Local stop cleanup remains available when the guest, login
session or Plane is unavailable; restoration still requires Plane to confirm a
stopped source with a current grant. No arbitrary guest command or network endpoint is accepted.

Unit contracts cover private persistence, reconnection identity, scope and grant
refusals, independent lease expiry, and stdin-only task transport. The existing
Docker specification tests preserve network isolation and exact owned cleanup.
Display and installed-container scenarios belong to the central nightly batch;
these unit checks do not claim deployed Plane or physical-host qualification.
