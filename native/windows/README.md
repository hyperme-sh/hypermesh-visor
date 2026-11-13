# Windows native engine

The Rust engine builds as `hypermesh_visor.dll` on Windows x64. This is a
component for the native companion, not a standalone desktop application or
installer. Windows ARM64 desktop packaging is not enabled yet.

The native app must register its callback in-process. Callback output buffers
use `hypermesh_native_alloc` and `hypermesh_native_free` from this DLL; never
mix its allocator with the app runtime. The engine rejects elevated startup.
Only the in-process UI can lift a user pause. Both external pipe routes retain
ordinary client authority, including the pipe named `companion`.

Local control uses protected current-user named pipes. `HYPERMESH_VISOR_INSTANCE`
is an ASCII letter/digit/hyphen name, default `default`, at most 64 characters.
Private runtime files default to `%LOCALAPPDATA%\HypermeshVisor\run-<instance>`.
MCP settings default to `%LOCALAPPDATA%\HypermeshVisor`. Explicit config/runtime
directory selections remain supported; existing unsafe runtime paths are refused.

Prompts use the independently started Conductor `serve --socket` service.
Its current-user pipe defaults to instance `default`; select another with
`HYPERMESH_CONDUCTOR_INSTANCE`, or provide a validated full local pipe name in
`HYPERMESH_CONDUCTOR_SOCKET`. Conductor must include its Windows pipe support
from hypermesh-conductor PR #62. There is no requirement to run matching builds.

| Function | Current Windows engine support |
| --- | --- |
| Control and inverse MCP transport | Private current-user pipes; no native TCP listener |
| Session credential store | Windows Credential Manager |
| MCP config, profiles and trust storage | Native file checks, owner/DACL checks for private files |
| MCP HTTP transport | Existing bounded HTTP protocol implementation |
| MCP stdio children | Refused before spawn; isolated worker remains pending |
| Focused executable | Same-user live process handle, never a window title |
| Capture/input callbacks | In-process bridge available; native app implementation remains pending |
| Audio and Linux machine bridge | Unavailable |
| Native companion, physical-input pause monitor, installer, automatic startup | Pending #18/#25 |
| Signing | Deferred to #84 |

PR CI builds the DLL and checks local pipe metadata, external pause-authority
refusal, and shutdown without opening a desktop. This does not claim that the
Windows companion or installer is complete.
