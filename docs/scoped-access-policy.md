# Personal and organization access policy

Permissions → Review or change access policy uses the existing native sign-in,
then Auth's organization-scoped session exchange over the fixed API HTTPS
origin. The scoped token stays in memory, redirects/proxies are disabled, and
User rechecks current organization authority before AIuditor exposes or edits
policy. Personal users may narrow their own rules; organization controls appear
only for current administrators. The UI supports denial, fresh verification on
each use, reduced limits and removal of that scope's restriction, with a reason.
Personal edits cannot relax organization rules. Stale concurrent edits require
a reload instead of silently overwriting another change.

A successful edit clears local remembered allows for the organization. Every
new action still obtains current review and committed owner admission; cached
metadata never authorizes dispatch. An unavailable verification discards the
matching optional allow. A changed revision can be freshly verified, but does
not count as reuse of an old grant. Per-use policy disables learning without
adding an automatic human prompt.

The policy owner's admission commit orders an action against revocation. A
revocation before that point refuses dispatch. A later change cannot undo an
already admitted action, and the UI says so. Consumed IDs survive process
restart independently of optional allow-cache eviction. This applies to reads,
HTTPS requests, project writes and named service control.

Related owner/consumer work: AIuditor #92/#100, Conductor #96 and Host #244.
Unit/contract checks use named owner responses and local disposable files.
