# Native session library

The CLI and companion share PKCE and device login, refresh-token storage and
loopback callback handling here. PKCE verifier and state bytes come from the OS
randomness source; failure stops login rather than falling back to a weak source.

`open_system_browser` accepts only bounded HTTP(S) URLs without embedded
credentials or control characters. Linux uses `xdg-open`, macOS uses
`/usr/bin/open`, and Windows uses `ShellExecuteW` on a dedicated COM STA thread.
The URL is passed as data, never interpolated into a shell command. Browser
launch errors do not include the authorization URL or its state value.

Native macOS and Windows try browser login without requiring X11 environment
variables. A reported launch failure selects the existing device flow. An
operator in an unattended or noninteractive session should explicitly select
device login; a successful OS launch does not guarantee a visible browser or a
completed sign-in. The callback timeout and OAuth state checks still apply.

CI runs session unit contracts natively on Linux, macOS and Windows. Tests use
mock token endpoints and synthetic tokens in memory or private temporary files.
They do not open a browser, access an operator keychain, sign into a provider or
qualify a native daemon, installer or credential store.

On Windows, the shared entry is `hypermesh:session` with a UTF-8 credential blob,
matching the bundled Go CLI. The Rust keyring library's default
`session.hypermesh`/UTF-16 mapping is not used. A session stored by the former
Rust-only mapping requires a fresh sign-in; that old entry is not read or
silently migrated. Disposable Windows CI checks both Go-to-Rust and Rust-to-Go
reads plus deletion using a unique synthetic service name.

## App-supplied session stores

Embedders can build with `--no-default-features --features external-store` and
supply the `SessionStore` trait implementation. This mode excludes the desktop
keyring dependency and the private-file backend; the default `KeyringStore`
refuses every operation. It cannot be combined with `os-keyring`. Existing
headless private-file behavior is unchanged when `external-store` is absent.

CI builds this session crate for `aarch64-apple-ios` and
`aarch64-linux-android` (API 26) and runs its unit contracts on desktop runners.
These are library builds, not phone-app or device validation. The mobile app
must still provide SociACL-backed enrollment/storage, native browser callbacks,
UniFFI bindings, VPN, Conductor/AIuditor routing and permission-controlled sensors.
No mobile filesystem credential fallback or implicit machine identity is added.
