# Optional Apple mesh tunnel adapter

This is the Apple library portion of Visor #226. The Swift package uses the
MIT-licensed official WireGuardKit at the source revision recorded in
`Package.swift`. It supports an app-owned NetworkExtension packet tunnel provider
on iOS 15 and macOS 12 or newer. It does not create an app, VPN profile,
entitlement, secure-store item, or startup job.

The provider owns an `AppleMeshTunnel` and passes it an in-memory
`TunnelConfiguration` from the enrolled-device secure-store owner. The adapter
copies the configuration before asynchronous work, accepts one keepalive peer,
rejects default routes and fixed listen ports, and opens no local service
listener. Stop must complete before replacing a configuration. Stop invalidates
pending start/status callbacks immediately. All completion callbacks should be
treated as background callbacks; app UI updates belong on the main queue.

A successful start means the native interface was installed. `status` reports
connected only for a recent handshake from the configured peer, never merely
because NetworkExtension says connected. Continuous elapsed time bounds freshness
through sleep and clock changes. Missing or malformed runtime data, stale
handshakes, and native failures refuse connected status. Callers must bound their
wait for callbacks and treat timeout as unreachable rather than reuse old status.

The adapter suppresses upstream diagnostics and extracts only the configured
peer's handshake time from bounded runtime data. It never returns raw WireGuard
configuration to the app, logs keys or topology, or writes credentials to files.
App/provider configuration exchange and secure-store provisioning remain part of
#225 and Plane #151. A current tunnel is not service authorization.

The consuming app must build/link WireGuardKit's native Go library according to
[its upstream instructions](https://git.zx2c4.com/wireguard-apple/about/), embed
its own packet tunnel extension, and obtain the OS VPN permission. No signing
identities or provisioning profiles are added here. CI compiles the actual Swift
target for macOS, iOS and the simulator and runs deterministic state/parser and
callback-ordering contracts; those checks do not link
or start a device VPN. A packaged extension, guarded Conductor/AIuditor requests,
app cancellation wiring and native nightly scenarios remain in #226/#228.
