# Android mobile library development bundle

This bundle contains `hypermesh-visor-mobile.aar`: generated UniFFI Kotlin
bindings, the native ARM64 Rust session core, and the Android Keystore adapter.
It targets Android API 28 or newer. The core alone can build for API 26; the
adapter requires API 28's unlocked-device key policy. Other Android ABIs are
not included. Rust load segments are checked for 16 KiB page compatibility.

The library adds no manifest permissions, activities, services or receivers.
It does not enroll a device, configure a VPN, make authorized agent calls,
request sensor access or run a model. It is an app dependency, not an APK.
Signing and store distribution remain deferred. Package version stays 0.1.0.

CI also compiles a separate build-only consumer against this AAR and the declared
JNA/Kotlin dependencies. The unsigned APK must contain exactly the ARM64 Visor
and JNA native libraries, with 16 KiB ELF load alignment and uncompressed,
16 KiB-aligned ZIP offsets. Visor's library bytes must match the input AAR.
The consumer calls the public adapter and generated API at compile time; it has
no entry points or permissions and is never installed or run. This checks
consumption and packaging, not Android device behavior or enrollment. The APK
is a temporary CI build output, not a distributed Visor app.

## Consume it from a native app

Verify `checksums.txt` and the download's adjacent SHA256 file. `build.json`
records the exact source revision, native binary and generated binding hashes.
The AAR includes no JNA or Kotlin runtime bytecode; declare both dependencies
in the app alongside the local AAR:

```kotlin
repositories { mavenCentral() }
android {
    defaultConfig {
        minSdk = 28
        ndk { abiFilters += "arm64-v8a" }
    }
}
dependencies {
    implementation(files("libs/hypermesh-visor-mobile.aar"))
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("org.jetbrains.kotlin:kotlin-stdlib:2.1.20")
}
```

Use `MobileSession(androidSessionStore(context, accountNamespace))` from
`uniffi.hypermesh_visor_mobile` and `sh.hyperme.visor.mobile`. The native auth
owner supplies a complete account namespace including issuer and tenant where
applicable. Never reuse one account's namespace for another account. Only the
auth owner writes a refresh token after authorized sign-in. Storage presence
is not authentication, device enrollment or an execution grant. There is no
exported credential getter. Close the `MobileSession` when its native owner
finishes using it. Calls may block; use a worker thread rather than the UI thread.

The AAR carries consumer rules preserving generated UniFFI/JNA classes and
members used through native callbacks and reflection. The library is unminified.
Verify the consumer's own minified build.
The app owns remote revocation; clearing local storage only deletes locally.
See the repository's `mobile/README.md` for storage refusal and recovery rules.

`licenses/` contains the Rust dependency inventory and license/notice files
available in the source packages. JNA and Kotlin are separate app dependencies
with their own notices. No Maven registry or app store publication is performed.
