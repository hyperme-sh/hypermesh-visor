# Install the published Linux release

[Downloads](https://hyperme.sh/visor) · [v0.1.0 release notes](https://github.com/hyperme-sh/hypermesh-visor/releases/tag/v0.1.0) · [Current source guide](../README.md)

This guide applies to **v0.1.0**, the published Linux daemon. It does not describe
the newer private-socket default, animated companion, MCP adapter, queued tasks,
Conductor bundle or container/headless packages in current source builds.
Windows, macOS and mobile downloads are not part of this release.

## Choose and verify a download

Use a logged-in Linux X11 desktop. The binaries were built on Ubuntu 24.04;
use that userspace generation or a compatible system. Keep the graphical
session's `DISPLAY` and authorization environment. Run `uname -m`: `x86_64`
means 64-bit PC, and `aarch64` means 64-bit ARM. Python 3 and curl are used in
the example below; `xdg-open` opens the resulting image in your desktop viewer.

The [download page](https://hyperme.sh/visor) offers copyable commands for each
architecture and format. Choose `.deb` for Debian/Ubuntu, `.rpm` for an RPM-based
system, or a raw binary if you do not want package-manager installation. It
checks the exact asset's SHA-256 before installation. Published checksums are
also available in [checksums.txt](https://github.com/hyperme-sh/hypermesh-visor/releases/download/v0.1.0/checksums.txt).
A checksum identifies the downloaded bytes; it is not a publisher signature.

For example, in a new download directory on an x86_64 machine:

```sh
curl --fail --location --proto '=https' --tlsv1.2 \
  --output hypermesh-visor-x86_64 \
  'https://github.com/hyperme-sh/hypermesh-visor/releases/download/v0.1.0/hypermesh-visor-x86_64' &&
printf '%s  %s\n' \
  'cde079b450d4a0af9561f4258ffa4a3ef39e3da2af3c56c095b5d52cfc95205d' \
  'hypermesh-visor-x86_64' | sha256sum --check --strict - &&
chmod +x hypermesh-visor-x86_64
```

Continue only after the checksum reports `OK`. An ARM64 download has a different
filename and checksum; use its selection on the download page. Do not bypass a
mismatch or substitute an artifact from a source build.

## Start and capture your desktop

A `.deb` or `.rpm` installs `/usr/bin/hypermesh-visor` and does not start a
service. Run `hypermesh-visor --listen 127.0.0.1:9847` as your desktop user.
For the raw x86_64 download above, run:

```sh
./hypermesh-visor-x86_64 --listen 127.0.0.1:9847
```

Keep that terminal open. The daemon prints `listening 127.0.0.1:9847` once it
has bound its listener. This release has no `/health` or `/capabilities` route.
It refuses root and non-loopback listening addresses. Loopback is a network
boundary, not isolation from other local users: use a trusted desktop and do not
forward this port. This version does not require a portal sign-in or bearer token.

In another terminal, open a session for a desktop image, save the image, and
open it in your viewer:

```sh
VISOR_SESSION=$(curl --fail --silent --show-error \
  -X POST http://127.0.0.1:9847/session \
  -H 'content-type: application/json' \
  -d '{"purpose":"capture my desktop","recipes":[],"agent":{"name":"local","instructions":""},"skills":[]}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])') &&
curl --fail --silent --show-error -X POST \
  "http://127.0.0.1:9847/session/$VISOR_SESSION/view" \
  --output visor-first-frame.png &&
xdg-open visor-first-frame.png
```

The PNG contains your desktop, so keep it private. No mouse or keyboard input is
needed for this first task. Close this session when finished:

```sh
curl --fail --silent --show-error -X DELETE \
  "http://127.0.0.1:9847/session/$VISOR_SESSION"
```

Session closure returns HTTP 204. **Ctrl+C in the daemon terminal stops Visor and
clears all sessions.** Do not assume this release has the newer companion pause
or physical-user interruption controls. Closing a session stops subsequent
calls; it does not undo input already sent. See the [versioned API source](https://github.com/hyperme-sh/hypermesh-visor/blob/v0.1.0/src/api.rs)
for this release's view/watch/listen/mouse/type request shapes.

## Upgrade or remove

Stop the daemon first. For a later published release, verify that release's own
checksum and compatibility notes before installing its package over this one.
The selected download page commands use the package manager for that operation.
For a raw binary, retain the old file if you need rollback, then replace only the
chosen daemon file with a verified download. Do not replace a running process.

Remove an installed Debian package with `sudo apt remove hypermesh-visor`, or
an RPM package with `sudo dnf remove hypermesh-visor`. A raw binary can be removed
by deleting that downloaded file after stopping it. This release installs no
background service or companion autostart entry. Images you saved are your files
and are not removed by package removal.

## Troubleshooting and newer features

- **Checksum mismatch:** discard that download and fetch the same version and
  architecture again. Do not install it while verification fails.
- **Exec format error:** check `uname -m` and the selected architecture.
- **Missing GLIBC version:** use a compatible Linux userspace; the raw download
  is not a static musl binary.
- **No display or capture failure:** start from your logged-in X11 session,
  preserve its environment, and check that you did not use sudo. Wayland capture
  depends on portal support and consent; native Wayland input is not supported.
- **Address already in use:** stop the previous daemon you started or select
  another loopback port and use that same port in every client command.
- **Unknown session:** open a new session after a restart or session closure.

For current development features, use the [verb reference](verb-contract.md),
[MCP setup](mcp-discovery.md), [queued tasks](queued-tasks.md),
[container guide](../container/README.md), and [headless guide](headless.md).
Those instructions do not upgrade the published v0.1.0 binary.
[Report an issue](https://github.com/hyperme-sh/hypermesh-visor/issues) with the
version, architecture and error text; omit tokens, private images and file content.
