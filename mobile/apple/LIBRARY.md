# Apple mobile library development bundle

`HypermeshVisorMobile.xcframework` contains static ARM64 Rust libraries for iOS
and the iOS Simulator as separate platform slices. `Sources/` contains the
matching generated Swift binding and the Apple Keychain adapter. The consumer
build checks target iOS 15 or newer. Intel simulators and Mac Catalyst are not
included. Version remains 0.1.0; this bundle is unsigned and store distribution
remains deferred.

Verify the download's adjacent SHA256 and every file in `checksums.txt` before
using it. `build.json` identifies the source revision and both native libraries.
Use the Swift sources from this same bundle so the generated interface and
native symbols stay compatible.

Add the XCFramework to the app target's linked libraries (static linkage; do not
embed it as a dynamic framework). Compile both Swift files from `Sources/` in
the app's own module. The generated binding imports `hypermesh_visor_mobileFFI`;
the selected XCFramework slice supplies its header and module map. CI compiles
and links these packaged files separately against the device and simulator SDKs.
It does not launch an application or access a user's Keychain.

Create `MobileSession(store: try KeychainSessionStore(service: appService,
account: activeAccount))` from the native auth owner. The service/account scope
must distinguish the selected account, issuer and tenant where applicable.
Only the auth owner writes a refresh token after authorized sign-in. Stored
session presence is not device enrollment or authority to execute a request.
The adapter uses unlocked-device-only, non-synchronizing Keychain storage and
has no file fallback or token cache. Local deletion does not revoke remotely.
The application owns remote revocation and account changes. See the repository's
`mobile/README.md` for the complete storage behavior and limitations.

The bundle adds no app, entitlement, provisioning profile, VPN, sensor grant,
model runner or network service. Native enrollment and authorized mobile routing
remain implementation work. The consuming app owns its signing and distribution.
`licenses/` includes the Rust dependency inventory and available source-package
license/notice files. No package registry or App Store publication is performed.
