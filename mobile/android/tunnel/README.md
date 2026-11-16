# Optional Android mesh tunnel adapter

This is the Android library portion of Visor #226. It is separate from the
session AAR and does not add VPN permissions to existing consumers. It wraps the
official Apache-2.0 `com.wireguard.android:tunnel:1.0.20260102` library and its
`GoBackend.VpnService`. Gradle merges that dependency's non-exported VPN service;
this module adds the Internet permission. No boot receiver or automatic startup
is installed.

An Android app explicitly includes this module, shows `permissionRequest()` from
its foreground UI, and waits for approval. Only then may its enrollment owner
supply a WireGuard `Config` from secure storage to `start`, off the UI thread.
The adapter accepts one peer with keepalive, a dynamically assigned listen port,
and an included-app list containing exactly this app. It never reads a config
file, logs configuration, or falls back to another network/backend. Replacement
first stops the previous tunnel; a failed stop refuses replacement.

`start` confirms native installation only. `status()` reports connected only
with a recent handshake for the configured peer from the current start. Missing,
stale, future, or pre-start handshakes cannot establish reachability. Both wall
and monotonic time bound freshness; a backward clock change requires a new
start. Native failure reports unreachable. A stopped tunnel reports disconnected
only after the native stop succeeds. Reconnection does not confer any service
authority, and the app must continue polling status rather than cache it.

No Conductor or AIuditor requests are implemented in this library. The future
mobile request owner must refuse requests unless the tunnel is currently
connected and the machine/session authorization is current, and cancel work on
disconnect. Phone identity and server-issued peer configuration depend on
Plane #151 and Visor #225. Apple NetworkExtension, the service request gate and
native app wiring remain in #226; this library alone does not complete it.
Signing, deployment and native device runs remain deferred. PR CI compiles the
real adapter and runs deterministic JVM state contracts without opening a VPN.

Upstream API: https://github.com/WireGuard/wireguard-android/tree/master/tunnel
