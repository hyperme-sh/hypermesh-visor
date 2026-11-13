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

SociACL device enrollment, Android secure storage, native UI, VPN,
Conductor/AIuditor routing, intents/sharing, sensors and model execution remain
unimplemented. This spike opens no listener, launches no browser, contacts no
service and requests no device permissions. It does not implement the shared
vault in #37. Signing and store distribution remain deferred.

## Apple Keychain adapter

Compile `apple/KeychainSessionStore.swift` into the same native app module as the
generated Swift binding, then pass `try KeychainSessionStore(service: appService,
account: activeAccount)` to `MobileSession`. The app supplies a stable private
namespace for its active account; changing accounts must not reuse another
account's key. The adapter implements only token storage, not sign-in or account
selection. Do not expose its internal test backend as a runtime setting.

Items use the data-protection Keychain, disable synchronization, and set
`kSecAttrAccessibleWhenUnlockedThisDeviceOnly`. Apple documents this class as
accessible while unlocked and not migrated to another device; it does not require
a passcode. See [Apple's accessibility documentation](https://developer.apple.com/documentation/security/ksecattraccessiblewhenunlockedthisdeviceonly).
No shared access group is selected. An `LAContext` with interaction disabled
prevents callback-driven authentication prompts; the native auth UI owns any
interactive authentication. Locked, denied, unavailable, malformed or missing
entitlement responses fail closed with fixed errors. Only item-not-found means
an absent session.

Writes update in place, preserve the previous item on an OS refusal, and never
delete first. A duplicate-add race gets one bounded update retry. Calls are
serialized, reads are uncached, and deleting an absent item is idempotent.
No credential is stored in preferences, files, logs or a fallback keychain.
This adapter makes no Secure Enclave, biometric-presence or memory-zeroization
claim. Local deletion still does not revoke the service-side grant.

CI exercises the adapter through generated Swift/Rust bindings with a recording
Keychain backend and compiles it against the iOS SDK. Those are unit/contract and
build checks; they do not access a user's Keychain, sign an app, or establish
installed-device behavior. Device storage and entitlement validation remain part
of native app delivery. Signing stays deferred.
