# Visor mobile shell

React/TypeScript UI in a Capacitor application. This directory is in development:
the UI build and controller unit tests work; native `Visor` plugin registration,
service provisioning and packaged application qualification are not complete.
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
