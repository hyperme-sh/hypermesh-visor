# Native macOS development app

The development app embeds the Rust daemon and Swift desktop backend in one
process. It provides a visible permission window and menu-bar Allow, Pause, Stop
and Quit controls. It is a development build for #18/#26; distribution signing
and notarization remain deferred in #84. The existing development bundle version
and identifier are unchanged.

CI builds the app on its native macOS runner, checks event mappings without
posting input, verifies that command-line bridge requests are refused, and runs
workspace unit and contract tests. A separate CI-only credential-store contract
uses a temporary keychain for the bundled CLI, checks write/replace/read/delete
and empty-token refusal, then restores the runner keychain settings. It uses
fixture values and does not perform sign-in or contact an auth service. The uploaded development ZIP contains the app, CLI and inverse MCP adapter,
with per-file SHA256 checksums and a source revision. See
[distribution instructions](DISTRIBUTION.md) for verification, use and removal. It targets the runner's macOS version and
architecture; it is not a universal or backwards-compatible release bundle.

## Development installation

An operator can build and install with:

```sh
bash native/macos/install-development.sh
```

This installs `~/Applications/Hypermesh Visor Development.app` and opens its
window. The installer refuses to replace an existing app. Local ad-hoc hardened
signing verifies bundle integrity; it is not Developer ID signing or notarization.
No permissions are granted by installation. Replacing the executable can require
permissions to be granted again. Quit and remove the app to uninstall it, and
remove its permission entries in System Settings if desired.

Use the window's explicit permission buttons for Screen Recording, Accessibility
and Input Monitoring. A local client must open a session using the control socket
shown in the window. Then the user must select Allow in the visible native UI.
Desktop access starts off. Pause, Stop, physical input, lost permissions, a lost
input monitor or an unavailable engine disable access. Closing the window quits
the app and releases held input. Resume requires another visible user action. If permission or input monitoring
is lost, both window and menu disable Allow/Resume and show a restart instruction.
Restore the permissions, quit and reopen the app, reopen a session, then select
Allow. Ordinary user pauses and display-layout changes remain resumable.

## Permission boundary and limitations

Only the embedded app can register the desktop callback. There is no external
bridge invocation, environment switch or callback-registration socket. External
control clients can pause but cannot lift the native pause. Same-user socket
access does not confer the app's internal companion authority.

The permission-bearing app refuses stdio MCP child processes until an isolated
worker is implemented. HTTP MCP adapters remain available. Capture and input
use the active displays in one logical-pixel rectangle, including displays left
of or above the primary monitor. Coordinates start at the rectangle's top-left;
gaps are black in captures and refuse pointer moves. Display layout changes pause
access and require Allow again. Captures are sequential across displays, not an
atomic snapshot. Layouts are bounded to 16 displays, 16,384 pixels per axis and
16,777,216 total logical pixels. Modifier shortcuts use
explicit US ANSI key identities; unsupported Unicode shortcuts fail. Plain text
uses Unicode events. Audio capture is unavailable.

The existing X11 companion is separate. Native animated companion parity,
isolated stdio workers and broader distribution support remain
follow-up work. No native desktop run is implied by the unit and contract results.

## Conductor prompt route

The native app reads `HYPERMESH_CONDUCTOR_ADDR`, `HYPERMESH_CONDUCTOR_LEASE_ID`
and `HYPERMESH_DEFAULT_MODEL` at startup. The address must be a numeric loopback
address with a nonzero port. A lease requires that route. Prompts use the selected
model and caller's renter credential through the existing Conductor contract;
invalid settings stop startup with a configuration error. Without an address,
prompts report that their route is unconfigured.

Start Conductor independently before launching the native app with these settings.
Managed `HYPERMESH_CONDUCTOR_CONFIG` and supervisor URL settings are refused by
the permission-bearing app. Native bundled-child startup still needs an isolated
worker; explicit external routing does not complete default bundled delivery in #90.

The native menu-bar companion uses the bundled animated mascot for idle,
automation and disconnected states. Reduce Motion selects a still frame. Hover
shows session/engine state and the latest reported input/output token counts
when available; missing counts stay absent. Its menu retains explicit local
Allow/Resume, Pause and Stop controls. While the current session is running with desktop access allowed, a 48-point
animated bug follows the pointer in a nonactivating, click-through panel. It
uses AppKit screen coordinates across displays and is excluded from Visor screen
captures. Reduce Motion freezes its animation. Pause, Stop, session loss,
permission/monitor loss and Quit hide it; a crash removes the process's windows.
The system cursor stays visible and is never replaced or hidden. A missing sprite
bundle leaves the overlay absent without changing desktop permissions. Windows,
floating companion controls and isolated worker processes remain tracked in #18.
