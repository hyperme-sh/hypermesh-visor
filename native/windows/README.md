# Windows native engine

The Rust engine builds as `hypermesh_visor.dll` on Windows x64. This is a
component embedded by the native Windows Forms companion. The development
bundle is self-contained and unsigned; an installer is still pending. Windows ARM64 desktop packaging is not enabled yet.

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
| Capture/input | Visible native approval; PNG capture, SendInput, Unicode text and common key chords |
| Audio and Linux machine bridge | Unavailable |
| Native companion and physical-input pause | Low-level input hooks; manual continue or two-minute idle resume |
| Bundled session CLI | Private control pipe, same instance as native companion |
| Automatic app/Conductor startup | Bundled launcher owns both processes |
| Installer and user-login registration | Pending #25 |
| Signing | Deferred to #84 |

PR CI builds the DLL and checks local pipe metadata, external pause-authority
refusal, and shutdown without opening a desktop. This does not claim that the
Windows companion or installer is complete.

## Run the development bundle

Build on Windows x64 with Rust, Go 1.25 and the .NET 10 SDK:

```powershell
./native/windows/build-development.ps1
```

The output is `dist/windows-development`. It includes the .NET desktop runtime,
engine DLL, native companion, inverse MCP adapter, session CLI, Conductor and checksums.
Signing is deferred. Windows may show an unknown-publisher or SmartScreen prompt;
this bundle does not request elevation or `uiAccess`.

Set the existing Conductor endpoint configuration and journal paths, then start
the launcher as your normal user:

```powershell
$env:HYPERMESH_CONDUCTOR_CONFIG = 'C:\Hypermesh\endpoints.json'
$env:HYPERMESH_CONDUCTOR_JOURNAL = 'C:\Hypermesh\measurements.jsonl'
./hypermesh-visor.exe
```

The launcher starts its bundled Conductor, verifies that the private pipe belongs
to that child and returns a ready response, then opens the companion. Quit the
companion to stop both. If either child fails or the launcher exits, its Windows
job terminates the remaining processes. Assignment to that job happens during
process creation; child processes never inherit the job handle. The launcher
refuses elevation and a second copy of the same instance.

Conductor credentials and configuration stay in its environment. The companion
receives only the required user/system paths, model/lease hints, instance name,
and the private Conductor pipe. An assigned session must still be opened by the
client/bot before desktop access can be allowed. Installer and login registration
remain under #25; this launcher does not register an autorun entry.

The control window shows the selected session and purpose. **Allow desktop
access** grants that session screen capture and mouse/keyboard input. **Pause**
and **Stop** revoke local input immediately. Minimize to the bug in the notification
area; its hover text shows the latest response's input/output token counts, not a
budget or total. Double-click it to reopen the controls.

Real input and input from other applications pause automation. A click on Allow
continues it; otherwise it may resume after two idle minutes, only for the same
approved session and pause epoch, with no held input. Manual pause/stop, a changed
session, display/session changes, or loss of the input monitor never auto-resume.
A stalled monitor requires restarting the app. Elevated windows and secure
Windows desktops are refused, with a visible status message.

The bug cursor is a click-through overlay. The system pointer remains visible;
the app never replaces a system cursor. The overlay disappears on pause, exit,
and process termination. Held synthetic keys/buttons are released on normal
pause, stop and exit. Unsupported shortcut glyphs are refused; Unicode text and
named navigation/modifier/function keys are supported.

MCP discovery/protocol code is present, but the native trust-review editor and
isolated stdio worker remain tracked under #29. There is no silent import or
automatic server launch.

## Session CLI

The bundle includes `hypermesh.exe`. Run `hypermesh.exe prompt --session <UUID>`
with the prompt on stdin or in `--message`. By default it uses this user's private
Windows control pipe. `HYPERMESH_VISOR_INSTANCE` selects the same instance as the
native companion (default `default`). The client validates the connected pipe's
owner and access list before sending the request. It does not fall back to TCP.
Use the normal CLI renter authentication configuration; credentials stay out of
terminal arguments. Windows terminal launch actions are still tracked in #89.

The CLI source is Visor's existing `third_party/hypermesh-cli` snapshot, including
its local-session API changes; it is not replaced by the separate upstream CLI.
