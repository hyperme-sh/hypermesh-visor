# Native inverse MCP client

`hypermesh-desktop-mcp-client` exposes the existing Visor MCP tools, resources,
queued-task controls and cancellation protocol over stdio. It builds independently
of the desktop daemon on Linux, macOS and Windows. Existing Linux/macOS packages
keep their `hypermesh-desktop-mcp` entry point and call this same implementation.

The default Windows route is a local named pipe scoped to the current user's SID
and `HYPERMESH_VISOR_INSTANCE` (default `default`; 1–64 ASCII letters, digits or
hyphens). The client verifies the opened object's owner and private ACL before
sending HTTP data, and uses identification-only security quality of service.
The Windows daemon is still being implemented in #25; until it is available,
configure `HYPERMESH_VISOR_URL`, `HYPERMESH_VISOR_TOKEN`, and the assigned
`HYPERMESH_SESSION_ID` for an explicitly enabled Visor HTTP(S) listener.
Use HTTPS outside a trusted tunnel or loopback. No token is put in process
arguments, and no local token-file fallback is attempted on Windows. An explicit
HTTP(S) endpoint still requires an explicit token. An unavailable or untrusted
local pipe fails without falling back to HTTP.
The client does not open sessions or grant desktop consent; server-side session,
local approval and task authorization still apply.

Linux/macOS preserve the local control socket default, runtime-directory
precedence and opt-in HTTP(S) behavior. The daemon and client share path-selection
code. Local clients now also require private socket paths and verify the kernel
peer identity before sending requests. The client shares input validation and bounded opt-in telemetry code with
the daemon; it does not link X11, D-Bus, keyring or desktop capture libraries.

Build with `cargo build --locked -p hypermesh-visor-client`. PR CI runs the client
unit contracts and builds the executable on all three desktop operating systems.
The Windows CI artifact is an unsigned development executable, not an installer
or a Windows desktop daemon. Windows daemon IPC wiring, input and installer work remain in
#25; signing remains deferred.
