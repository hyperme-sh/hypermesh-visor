# Headless host health worker

The headless package includes `hypermesh-host-health`, a separate Linux-only
executable. It uses the same bounded OS sampler and OTLP projection as Visor.
Visor still requires device-local category opt-in; no Visor option enables host
policy. The host executable runs as root and accepts only root-owned private
configuration, credentials and the root-owned host owner socket. It requests no
interactive consent. The host's accepted agreement and current machine status
are checked through Host #215 before collection and again before upload.

The root-owned mode-0600 `/etc/hypermesh/health.json` contains these fields:

- `device_id` and `tenant_id`: the enrolled machine scope.
- `owner_socket` and `owner_token_file`: absolute paths to the host's existing
  private owner socket and automatically provisioned owner credential.
- `collector_url`: provisioned OTLP HTTP endpoint ending in `/v1/metrics`.
- `collector_authorization_file`: absolute path to a private file containing the
  complete provisioned HTTP Authorization value. Never manually mint a token.

HTTPS is required except for a literal loopback address, which supports a local
collector or an isolated fixture. No redirects, query credentials or persistent
payload queue are supported. Remote errors are reduced to a fixed message;
credential values and response bodies are never printed. Infra #1435 still owns
provisioning and the receiving collector contract; a configured endpoint does
not establish collector readiness. This worker's initial transport accepts a
provisioned Authorization value; mesh-only transport without that header remains
a follow-up until Infra publishes its exact contract.

Install the headless package, provision this configuration through Infra, and
manage `hypermesh-host-health` with its included systemd or OpenRC service.
Package installation alone does not enable the service or grant monitoring
authority. `hypermesh-host-health --once /etc/hypermesh/health.json` performs one
cycle for registered CI scenarios; agents do not run it on shared hosts by hand.
Normal service operation samples every 60 seconds. Failures discard the sample;
the next cycle obtains fresh authorization and collects again. Shutdown leaves
no persistent payload. An authorization check failure or changed machine/terms
prevents upload. Numeric OS aggregates are the initial sources; sensors and disk
health remain #218. No host selling or package-update action belongs here.

The host authority is a point-in-time check, not a transferable grant. The worker
compares both checks to the configured machine and to each other and rejects a
cycle older than 30 seconds before sending. Collection and network operations
are bounded. The collector still authenticates its ingest credential and assigns
tenant identity independently; payloads carry no client tenant or machine tags.
