# Visor mobile shell

React/TypeScript UI in a Capacitor application. This directory is in development:
the UI build and controller unit tests work; the Android and Apple `Visor` plugins wire enrollment through the native core.
Native SDK qualification runs in PR CI. The phone obtains a short-lived mesh lease
from Plane and routes application requests through Conductor and AIuditor.
Infra deployment configuration and scheduled native qualification are separate.
Do not interpret a successful web build as a working enrolled phone.

`npm ci`, `npm test`, and `npm run build` qualify the UI. `npm run sync` copies the
bundled assets into the native projects. Native SDK builds belong in PR CI;
emulator/simulator integration belongs in the scheduled nightly batch. Signing
remains deferred.

## Native boundary

`src/native.ts` describes operation-specific calls and public state. OAuth refresh
tokens, machine credentials, signing keys and WireGuard keys stay in the Rust and
native secure-store layer. No generic HTTP, filesystem or credential-export bridge
is exposed. JavaScript does not choose service origins or supply native authority.
A browser-only build cannot claim native enrollment or connectivity.

## s3r.ch integration coordination

The user selected this shell for integration with phone-based s3r.ch components.
The s3r.ch identity owner is coordinating login, SEA custody, restoration and
account switching. Reusable React components should consume narrow public
interfaces; machine enrollment must not substitute for the person's SEA identity,
account or workspace authorization. No shared package API is final until the owner
confirms its existing contracts. AI/tool service calls use Conductor; this
shell must not introduce a direct Gateway client.

## Native enrollment configuration

Capacitor reads the five public build-time inputs `VISOR_MOBILE_ISSUER`,
`VISOR_MOBILE_AUTH_ORIGIN`, `VISOR_MOBILE_PLANE_ORIGIN`,
`VISOR_MOBILE_WORKSPACE_ID` and `VISOR_MOBILE_WORKSPACE_NAME`. Supply all five or
none. Unconfigured builds refuse enrollment; they never invent service origins.
These inputs must not contain credentials. The configured workspace is a candidate,
not a membership grant: native owner-session switching and Plane enrollment verify
current membership. Do not query the administrator-only tenant listing endpoint.

Both platforms restore their native identity from the OS secure store. Recreating
the Android activity stops its previous tunnel and requires an explicit reconnect.
Device authorization opens the validated issuer URL in the system browser; only the
short user code returns to React. Cancellation bypasses the polling worker.
Expired-owner revocation stays locally denied, and signing in again can retry remote
revocation before generating a replacement identity. `VISOR_MOBILE_AAR` selects the
locally built, verified library artifact for SDK compilation, not a running service.
Connect requests a Plane-authorized lease using native owner and machine credentials.
Android asks for OS VPN permission, then restricts the tunnel to this application.
iOS embeds a packet-tunnel extension and requests OS VPN permission. Its saved VPN
preferences contain no WireGuard keys; startup options and refresh messages carry
configuration only in memory. Both adapters refresh leases every 20 seconds and
stop on expiry, disconnect, owner change or revocation. Connected status requires a
fresh peer handshake. No tunnel starts at boot. Infra configuration is tracked in #1747.

`VISOR_TUNNEL_AAR` selects the matching Android adapter artifact. Apple CI builds
WireGuard's Go archive and embeds the unsigned extension. NetworkExtension
entitlements declare the required capability but do not supply signing identities
or authorize a signed release.

Apple SDK CI stages the checksum-verified XCFramework and Swift wrappers from the
same source revision, then compiles the unsigned simulator app without launching a
simulator. Android SDK CI builds an unsigned ARM64 release APK against the same
job's native AAR. Neither compile job provisions a peer or performs live sign-in.

## Native prompt operations

The lease supplies the native Conductor origin. The legacy optional
`VISOR_MOBILE_CONDUCTOR_ORIGIN` build input grants no routing authority. Both platform plugins expose `preparePrompt`, `submitPrompt` and
`discardPrompt`; no per-request origin, bearer token or arbitrary network method
crosses the JavaScript boundary. Preparing returns public task/operation IDs and
the exact digest. The owner/session task workflow must authorize that binding
before submission. A failed submission is uncertain and must not be retried.
Owned sessions can be listed before connecting. Sending requires a currently
authorized tunnel; the SEA identity adapter remains a separate owner component.

## Shared s3r.ch presentation

The phone conversation uses `MessageComposer` from the same private
`@s3rch/forum-ui` source package as s3r.ch room chat. Its checked-in source copy
is refreshed with `python3 mobile/app/sync-forum-ui.py /path/to/s3r.ch` from the
checkout's committed revision. `vendor/s3rch-forum-ui/UPSTREAM.json` records the
application version, revision and each source file digest. No network fetch or
floating source dependency is needed to build the phone.

The composer owns no credentials, navigation or transport. Native owner and
machine authorization stay in Rust. This presentation reuse does not connect
a SEA identity or create a Squad/forum membership.
