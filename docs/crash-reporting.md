# First-party crash reporting

Set `APPLICATIONINSIGHTS_CONNECTION_STRING` to the existing operations workspace's
Application Insights connection string to enable reports in the daemon, desktop
companion and inverse MCP process. The reporter uses the workspace's HTTPS
`IngestionEndpoint` and instrumentation key. It creates no Azure resource and
uses no third-party crash vendor. Unset configuration sends nothing. Invalid
configuration or a missing/unsupported build version disables reporting with a
generic diagnostic; configuration values are never printed.

Reports use Application Insights `ExceptionData` (`AppExceptions` in the shared
workspace), independently of domain events. The reporter covers Rust panic hooks,
daemon server/startup failures, companion window failures, and upstream 5xx or
invalid prompt responses. It cannot report SIGKILL, process abort, power loss or
failures before initialization. Delivery is best effort, not a guarantee that
absence of reports means a healthy canary.

## Fields and privacy

Every report includes the compiled package version in `ai.application.ver` and
`visor_version`, plus a fixed component and error category. No panic payload,
error message, stack, path, URL, prompt, model response, user/session/device ID,
request body, authentication value or hostname is collected. The IP context tag
is `0.0.0.0`. The collector still necessarily receives a network connection;
there is no claim of anonymous network transport.

Prompt requests capture only these upstream response headers:

- `X-Hypermesh-Version`: numeric `major.minor.patch`, optional numeric
  `alpha`, `beta` or `rc` suffix, or a 7–40 character hexadecimal revision.
- `X-Hypermesh-Release-Color`: exactly `blue` or `green`.
- `X-Hypermesh-Canary`: exactly `true` or `false`.

Other header values and response bodies are never used in reports. A known
version or color sets `upstream_attribution=complete`. A request without usable
metadata sets it to `incomplete`; consumers must not count that as a clean canary
window. `upstream_canary` is included only when explicitly advertised. Unknown
version formats are discarded instead of forwarding free-form strings. The
upstream service name is a fixed enum, not a URL or caller-provided name.

Request attribution is thread-scoped, restored after nested calls and cleared on
return. It is never copied from another request's last successful response. Local
Visor IPC in the companion/inverse adapter currently has no release headers and
is reported as incomplete when a panic occurs within that request. Failures
outside a request have `upstream_attribution=not_applicable`.

## Bounds and operation

Reporting uses a 16-item queue and at most eight reports per minute per process.
Callers never wait for network delivery. The sender makes one attempt per report,
with a two-second timeout, no redirect following, no proxy inherited from the
environment and no persistent spool. Shutdown waits at most three seconds for a
flush. A full queue, unavailable collector or early process termination can lose
reports. Telemetry failure never retries a user operation or changes its result.

Use the workspace's standard connection string with an HTTPS Azure ingestion
origin. Authentication modes other than the instrumentation-key connection string
are rejected; resources configured to prohibit that mode need an authenticated
collector implementation before enabling this reporter. Do not weaken workspace
authentication to accommodate it.

Unit and mocked-collector contracts cover version rejection, field allowlists,
request isolation, missing upstream metadata, redirect refusal, queue/rate bounds
and the Application Insights envelope. They send no production reports. The
reporter does not change production promotion rules or activate a canary.
