# Saved companion sets

The Linux Companions menu can save the current container companions as a named
set, load a saved set, or delete its definition. The native desktop is not part
of a set. Each member saves its memory and CPU limits, its instructions, and an
optional reference to an existing broker enrollment. It does not copy homes,
sessions, running tasks, credentials, or remote grants.

Use **Edit saved instructions** on a container to change its instructions. The
manager pauses that companion first, cancels outstanding work, and verifies the
guest applied the new text. Resume still requires a fresh local confirmation.
The guest reads only its own scoped profile from the read-only control mount.
An older image that cannot confirm nonempty instructions is refused.

Loading checks current capacity and available memory for each member. Enrolled
members also require owner sign-in and a fresh Plane authorization through the
host-side broker before a guest starts. A missing enrollment, revoked assignment,
or insufficient resource results in a refusal for that member. Other members
can load; the dialog reports each result. Successful members receive fresh
identities and start paused with network isolation intact. Saved references do
not revive deleted enrollments or inherit old approvals.

The owner-private `companion-sets.json` stores at most 32 sets of three members,
with instructions limited to 4096 bytes. Instructions are ordinary local text,
not a secret vault: do not paste credentials into them. The schema rejects
unknown credential fields and stores broker scope references only. Removing a
saved definition does not stop desktops, delete homes, or revoke enrollments.
Deleting an enrolled source can make saved references unusable; save a new set
from currently enrolled companions in that case.

Unit tests use temporary local files and a fake manager backend. They cover
private persistence, credential-field refusal, scope binding, capacity and grant
refusals before guest creation, and the private profile route. They do not start
Docker or contact Plane.
