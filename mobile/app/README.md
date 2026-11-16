# Visor mobile shell

React/TypeScript UI in a Capacitor application. This directory is in development:
the UI build and controller unit tests work; the Android and Apple `Visor` plugins wire enrollment through the native core.
Native SDK qualification is in progress; mesh provisioning, service-call wiring
and native nightly application acceptance are not complete.
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
confirms its existing contracts. Application service calls use Conductor; this
shell must not introduce a direct Gateway client.

## Native enrollment configuration

Capacitor reads the five public build-time inputs `VISOR_MOBILE_ISSUER`,
`VISOR_MOBILE_AUTH_ORIGIN`, `VISOR_MOBILE_PLANE_ORIGIN`,
`VISOR_MOBILE_WORKSPACE_ID` and `VISOR_MOBILE_WORKSPACE_NAME`. Supply all five or
none. Unconfigured builds refuse enrollment; they never invent service origins.
These inputs must not contain credentials. The configured workspace is a candidate,
not a membership grant: native owner-session switching and Plane enrollment verify
current membership. Do not query the administrator-only tenant listing endpoint.

Both platforms use one process-owned native identity/store pair across activity/view recreation.
Device authorization opens the validated issuer URL in the system browser; only the
short user code returns to React. Cancellation bypasses the polling worker.
Expired-owner revocation stays locally denied, and signing in again can retry remote
revocation before generating a replacement identity. `VISOR_MOBILE_AAR` selects the
locally built, verified library artifact for SDK compilation, not a running service.
Phone mesh provisioning is tracked in Infra #1747; connect remains unavailable.

Apple SDK CI stages the checksum-verified XCFramework and Swift wrappers from the
same source revision, then compiles the unsigned simulator app without launching a
simulator. Android SDK CI builds an unsigned ARM64 release APK against the same
job's native AAR. Neither compile job provisions a peer or performs live sign-in.
