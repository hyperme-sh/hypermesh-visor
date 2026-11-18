# Verified typed actions

Permissions → Run a verified typed action executes an exact configured HTTPS,
project-write or named user-service action. The active assignment supplies the
project and identity; the saved template must still match. User and organization
policy is checked for each action. An eligible current review learns a scoped
allow automatically, with advisory cases marked for dreaming review. The learned
allow list displays the operation, exact target, typed arguments and limits.

Visor asks Conductor and AIuditor for a current review, then requests fresh
admission against that policy revision. Conductor reserves the request ID in
its existing durable journal before execution. A consumed ID cannot be admitted
again after restart. Side effects do not use the bounded local read retry list.
If a response is lost, the result is unknown and is not automatically retried.

* HTTPS supports GET and POST to an exact canonical public HTTPS URL. The
  reviewed scope binds method, path, resolved addresses, payload digest and
  size. TLS verification remains enabled, redirects and environment proxies
  are disabled, and the resolver returns only the reviewed addresses. No caller
  headers, stored account credentials, URL credentials or query strings are
  forwarded. Request and response size are bounded by the selected limit.
* Linux project writes create a new direct child of the selected project.
  Existing files and symlinks are refused. Every project path component is
  opened without following links, and the reviewed parent identity is checked
  before an atomic create-only publication. Content is written as the Visor
  user with private file permissions; no privileged file adapter is used.
* Named service restart uses the Host's optional configured non-root user
  service driver. The reviewed scope binds the controller executable hash,
  configuration hash, unit and UID. It does not expose a shell or arbitrary
  flags. User-service definitions remain owned by that user.

The local companion socket is required. Work is bounded to four concurrent
workers. Session changes, physical pause and disconnect invalidate ongoing
work; cancellation cannot undo a side effect already accepted by its target.
Completed actions remain reported as completed if saving optional allow
metadata fails. File or HTTP payload contents are never stored in learned
allows. Current-policy edits and revocations are checked again on reuse.

Unit and contract checks cover current admission, policy revision binding,
create-only writes, changed parent identity, symlink collisions, private network
addresses, payload hashes and metadata-only storage. Unsupported file-write
platforms refuse the operation. Full-system checks remain in CI.

Related: Conductor #69/#95, AIuditor #99, Host #243.
