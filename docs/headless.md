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

This implements the build/storage dependency portion of #99/#101. Separate
packages and OpenRC service installation, bundled musl Conductor, on-request host
delivery/removal, the opt-in host capability schema and AIuditor integration
remain required. Nightly integration coverage must be added for those paths;
unit/build results do not establish installed-service behavior.
