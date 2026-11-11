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

This implements the build/storage and package portions of #99/#101. Installed
OpenRC behavior, on-request host delivery/removal, the opt-in host capability
schema and AIuditor integration remain required. Nightly integration coverage must be added for those paths;
unit/build results do not establish installed-service behavior.

The same Dockerfile has an optional `runtime` target built on digest-pinned
Alpine. It installs the APK, runs as UID 1000, and contains no Rust/Go toolchain,
desktop stack or D-Bus service. Its local sockets are under `/run/visor`; private
state is under `/var/lib/visor`. Deployment configuration owns any state mounts
and grants; the image does not expose a TCP port or mount host resources.
