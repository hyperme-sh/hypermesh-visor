# Windows crash registration

The approved crash upload path supports the fixed local endpoint
`\\.\pipe\hypermesh-crash-registration-v1` provided by Host's dedicated
`HypermeshCrashRegistration` service. Set the private crash configuration's
`owner_socket` to that exact path (escape backslashes in JSON).

Host #227 owns the LocalSystem service, its machine-owned signing key and the
single allowed client SID. The raw-dump cleanup service continues under the
existing Windows user account; it is still required before raw capture.

Before sending any bytes, Visor checks Service Control Manager for the running
own-process LocalSystem service and matches its process to the named pipe's
server. It rechecks the service configuration, process and pipe identity while
holding the process handle. The client requests identification-only SQOS and
specific data rights without permission to create another pipe server.
Same-user local IPC elsewhere retains its existing identity checks.

Only crash registration uses this Windows connection. It carries the approved
event ID, original UTC timestamp and exact sanitized JSON hash. Every returned
owner/project/event binding is checked before the existing unchanged upload.
Raw dumps, general credentials and arbitrary paths are never transmitted.

Registration configuration, destination collection and digest activation remain
disabled. This transport does not remove destination refusal or grant consent.
Windows unit contracts use a disposable CI LocalSystem service with synthetic
metadata; they do not install or activate the production service.
