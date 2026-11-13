# Headless build

Build the Linux daemon with `--no-default-features --features headless`. This
selects a refusal backend for every desktop operation and omits X11, Wayland
D-Bus, OS keyring dependencies and the inverse desktop MCP binary. Session,
configured MCP tools, local sockets, vault and Conductor routing remain present.
The companion is a separate workspace package and is not part of this build.

```sh
cargo build --release --locked -p hypermesh-visor --no-default-features --features headless
```

`desktop` remains the default. Combining `desktop` and `headless` fails at compile
time, preventing an accidentally graphical build being labelled headless.
Capability discovery reports `build_mode: headless`, an unavailable desktop and
no installed inverse desktop tools. It does not expose new host abilities by
default or create a network listener. Existing loopback-only TCP opt-in and
socket authorization remain unchanged.

Without OS-keyring support, session credentials require an explicit
`HYPERMESH_SESSION_SECRET_FILE` absolute path. Its parent must already be owned
by the running user with mode 0700. The backend uses descriptor-relative file
operations, mode 0600, atomic replacement and bounded reads. It rejects symlinks,
hardlinks, wrong ownership and permissive modes. Missing configuration fails
closed. Desktop builds continue to use the OS keyring and do not silently fall
back to a file when that service fails.

The headless CI workflow performs unit tests and native x86_64/aarch64 builds for
glibc and musl. Alpine builds verify the kernel and Rust host architecture and
reject dynamic ELF interpreters. No QEMU or cross-architecture fallback is used.
CI artifacts expire after one day; they are not published releases.

Native musl CI also creates unsigned `.apk`, `.deb`, `.rpm` and tar bundles.
They install `/usr/bin/hypermesh-visor-headless` and a static Conductor under
`/usr/lib/hypermesh-visor-headless/`. The two package names and install paths are
separate, so installing headless does not replace the desktop companion.
Package versions retain the Cargo version. These short-lived CI artifacts are
not an update channel: `build.txt` identifies both source revisions and the
architecture, and `checksums.txt` identifies the exact downloadable bytes.
They do not create users, enable a daemon, or configure network services.

The APK includes an OpenRC service. Deployment configuration must first provision
a dedicated non-root account and set `visor_user` in
`/etc/conf.d/hypermesh-visor-headless`. The service refuses an empty account or
UID zero, creates private runtime/state directories, and keeps its PID file in
root-owned `/run`. Installation leaves it disabled. Its default transport is a
local socket; network access requires separate configuration.

The Alpine build and packaging work in #101 is implemented. Infra owns the
scheduled installed-package checks; they are not a prerequisite for closing the
implementation issue. On-request host delivery/removal, opt-in host capabilities
and AIuditor authorization remain separate work in #99.

The same Dockerfile has an optional `runtime` target built on digest-pinned
Alpine. It installs the APK, runs as UID 1000, and contains no Rust/Go toolchain,
desktop stack or D-Bus service. Its local sockets are under `/run/visor`; private
state is under `/var/lib/visor`. Deployment configuration owns any state mounts
and grants; the image does not expose a TCP port or mount host resources.

For a future owner-authorized version tag, the release workflow also builds the
native headless bundles on both architectures, verifies their source revisions
and checksums, and includes them beside the desktop assets. Architecture-specific
build metadata avoids filename collisions, and the combined checksum file covers
all published bytes. Current CI artifacts are still not a published release;
this wiring does not create a tag, authorize a version, or alter desktop packages.

## Alpine installation

Current native bundles are CI artifacts, not a published Alpine release. Download
the complete `headless-musl-x86_64` or `headless-musl-aarch64` artifact from the
same successful main build. Keep its `build.txt` and `checksums.txt`; the former
records both source revisions and the latter covers the bundle's bytes. Do not
combine files from different builds. The APK is unsigned; checksum comparison
does not provide a package-signing identity.

On a native Alpine machine, unpack that artifact into a new directory. From that
directory, verify the complete artifact and select the matching architecture:

```sh
visor_arch=$(uname -m)
case "$visor_arch" in x86_64|aarch64) ;; *) exit 1 ;; esac
test "$(apk --print-arch)" = "$visor_arch"
sha256sum -c checksums.txt
```

Install the matching file as root. This installs the binaries and service files;
it does not create an account, start a service or enable TCP:

```sh
apk add --no-network --allow-untrusted "./hypermesh-visor-headless-${visor_arch}.apk"
```

For OpenRC operation, use an Alpine installation that already has OpenRC and a
provisioned dedicated non-root service account. Set `visor_user` to that account
in `/etc/conf.d/hypermesh-visor-headless`, then start the service as root:

```sh
rc-service hypermesh-visor-headless start
rc-service hypermesh-visor-headless status
```

The service creates `/run/hypermesh-visor-headless` and
`/var/lib/hypermesh-visor-headless` with mode 0700 owned by the selected account.
It sets the private session-file backend, stores configuration under the state
directory and keeps TCP disabled. Desktop operations remain unavailable. To
start it at boot, explicitly add it with `rc-update add hypermesh-visor-headless
default`; installation itself does not do that.

For an upgrade, first verify the new complete artifact, stop the service, and
install its matching APK. Development artifacts can share a package version:
remove the old package with `apk del hypermesh-visor-headless` before adding a
same-version candidate. Preserve the state directory and service-account choice,
and restore `visor_user` if the package manager supplies a fresh configuration.
Start the service after installation. This is a manual development bundle path,
not an automatic update channel.

To uninstall, stop the service, remove its boot entry if one was added, and run
`apk del hypermesh-visor-headless`. The service account and private state remain
operator-owned; package removal does not erase stored credentials.

The static tarball contains the daemon, Conductor and licenses under `usr/`; it
does not contain OpenRC setup. Keep the daemon's `bin` directory and Conductor's
sibling `lib/hypermesh-visor-headless` layout together. A direct non-root launch
requires an explicitly configured private session-file backend as described
above. Neither installation mode silently grants host abilities or provisions a
Conductor lease.

The bundled Conductor now includes capability-note indexing and the signed
AIuditor capability-review client (Conductor PR55 and PR56). Its public source
archive is verified by SHA256 in both package builders, and `build.txt` records
the source revision. This makes the command available; it does not enable host
capabilities or configure an authority, listener, machine signer or controller.
