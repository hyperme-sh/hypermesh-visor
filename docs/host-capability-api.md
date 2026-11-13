# Owner-enabled host tools

Set `HYPERMESH_HOST_BRIDGE_SOCKET` to the absolute socket path of the Host
machine bridge when starting Visor. Unset means disabled. Visor remains an
unprivileged process; it verifies the bridge's kernel peer UID is zero before
sending HTTP. Host must configure Visor's UID explicitly. No socket address,
policy, controller rule, identity key or executable can be changed through this API.

The existing control socket and optional authenticated loopback API serve:

- `GET /capabilities`: desktop metadata plus `host.capabilities`, the current
  owner-enabled verb list. `host.available` is false when disabled or unavailable.
- `GET /host/capabilities`: the same enabled catalog, or a refusal.
- `POST /host/call`: `{scope, capability_id, call_id, arguments}`. `call_id` is a
  canonical lowercase UUID supplied by the caller; `arguments` is an object.
- `POST /host/mcp`: stateless JSON-RPC MCP initialization, ping, `tools/list`
  and `tools/call`. Tool names are stable hashes of scope and capability ID.
  Each tool takes `{call_id, arguments}` with the capability's typed bounds.

No new listener is created. TCP remains loopback only and keeps its existing
local API authentication. That local transport credential is not sent to
AIuditor. Machine review uses the enrolled host identity and WireGuard binding.

The bridge returns the current catalog from a fixed root-owned Conductor
profile. Visor fills owner, revision and source/graph hashes from that catalog;
callers cannot supply replacement bindings. Arguments become compact JSON bytes
before machine review. Owner policy must authorize the digest of those exact
bytes. Neither catalog access nor MCP initialization grants execution authority.

Host runs the fixed Conductor dispatcher as root. Only that dispatcher can reach
machine review and the controller socket. It validates the graph and typed
arguments, reserves the call ID durably, checks the controller's current
conditions and limits, obtains AIuditor permission, and checks freshness again
before dispatch. Controllers enforce their own immutable rules and duplicate
call protection. The Host reference controller only reads the clock; it is not
an actuator driver. Other drivers must implement their own hard safety checks.

Calls are never retried automatically. A refusal or lost connection can leave
an uncertain outcome. Do not generate a replacement UUID to retry an uncertain
operation. The dispatcher retains consumed IDs across restarts. Four client
requests may be in flight; bodies, output and request duration are bounded.
Request arguments and controller outputs are not logged by these routes.

Unit and contract coverage checks kernel peer rejection before HTTP, default-off
routing, existing API authentication, exact catalog bindings, unlisted tools,
refusal without retries, and generated tool schemas. Host/controller and policy
contracts run in their owning repositories' CI.
