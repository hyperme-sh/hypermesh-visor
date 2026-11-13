# Mobile session binding spike

For #100, `core` wraps the existing Rust `SessionStore` through UniFFI and
generates Swift and Kotlin interfaces. It is an isolated Cargo workspace so
desktop keyring features cannot be unified into the mobile build. The lockfile
pins the generator and libraries together. It builds static and dynamic libraries
with a stable Rust toolchain; the desktop workspace's minimum compiler claim does
not cover this spike.

Native apps implement `NativeSessionStore` using their secure storage owner and
pass it to `MobileSession`. The callback must be thread-safe and return a fixed
`Unavailable` error when storage is locked, denied or absent. Rust maps storage
failures to fixed messages, rejects empty/control-containing/oversized credentials,
and never caches the stored token. There is no file or desktop keyring fallback.
The native owner is trusted code: it must not log callback arguments or persist
them outside its secure store. UniFFI strings cross native/Rust memory; this spike
does not promise zeroization of copies made by generated language runtimes.

`hasStoredSession` reports storage presence only. It is not proof of device
enrollment or a service grant. `storeRefreshToken` is for the native auth owner
after authorized sign-in; it does not authenticate supplied strings.
`clearLocalSession` deletes locally and does not revoke a remote grant. The auth
owner must perform remote revocation separately. No credential getter is exported
on `MobileSession`.

Generate bindings from the host library, for example on Linux:

```sh
cargo build --locked --manifest-path mobile/core/Cargo.toml --features bindgen
mobile/core/target/debug/mobile-bindgen generate --no-format \
  --library mobile/core/target/debug/libhypermesh_visor_mobile.so \
  --language swift --language kotlin --out-dir /tmp/visor-mobile-bindings
```

Use the `.dylib` on macOS. The generator's `bindgen` feature is host-only and is
not enabled for iOS/Android builds. See [UniFFI's foreign trait
documentation](https://mozilla.github.io/uniffi-rs/next/proc_macro/traits.html).

CI runs Rust units and generated Swift/Kotlin callback contracts using memory-only
fixtures, builds the core for iOS ARM64 and Android ARM64/API 26, and generates
both language interfaces. The Kotlin contract runs on the hosted Linux JVM;
it is not an Android app test.
The iOS/Android libraries are build checks, not store packages or installed apps.

SociACL device enrollment and secure-store implementations, native UI, VPN,
Conductor/AIuditor routing, intents/sharing, sensors and model execution remain
unimplemented. This spike opens no listener, launches no browser, contacts no
service and requests no device permissions. It does not implement the shared
vault in #37. Signing and store distribution remain deferred.
