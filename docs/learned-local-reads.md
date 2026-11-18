# Learned local reads

Permissions can read one exact file selected in the saved project template. The
assigned Host privately supplies machine credentials to Conductor; Conductor and
AIuditor verify the active assignment and current organization policy. Visor opens
the file as the desktop user, refuses symlinks in every path component and binds
the review to the opened object's metadata and read limits.

A successful verified read may retain a scoped allow for up to one day. Reuse
still checks current identity, policy revision, consent, expiry and OS access.
Advisories permit otherwise authorized reads and mark the allow for later review.
Prohibitions refuse reads; no-always-allow rules prevent remembering permission.
The allow list shows origin, scope, policy revision, last use and pending review.
Stored metadata never includes file contents or machine credentials. The optional
allow cache evicts least recently used entries to stay within 256 entries and
512 KiB; a full cache does not turn into a quota on authorized reads.

AIuditor #92 adds committed owner admission immediately before reading. Its
retained consumed IDs survive local retry-cache eviction and restarts. The local
last-1,024 list is only an early duplicate check; it cannot authorize reuse.
A current policy revision change invalidates old allow metadata and requires a
fresh review. Revocation before admission refuses the read. A later revocation
cannot undo an already admitted read. Reads remain capped at 1 MiB and 30 seconds.

This consumer supports Conductor #68 with Plane #177, AIuditor #95, Conductor #80
and Host #224. The initial assigned Host bridge supports Linux. Unit and contract
checks use owned temporary files and named review substitutes. No version or
release change is included.
