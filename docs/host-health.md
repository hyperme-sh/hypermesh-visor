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
- `collector_authorization_file`: optional absolute path to a private file containing the
  complete provisioned HTTP Authorization value. Never manually mint a token.

Credential-bearing uploads require HTTPS, including on loopback. A literal
loopback HTTP endpoint is allowed only with no authorization file, for isolated
fixtures; it cannot authenticate its listener and is not a credentialed collector. No redirects, query credentials or persistent
payload queue are supported. Remote errors are reduced to a fixed message;
credential values and response bodies are never printed. Infra #1435 still owns
provisioning and the receiving collector contract; a configured endpoint does
not establish collector readiness. This worker accepts a
provisioned Authorization value over HTTPS, or no header where the receiving
contract supplies machine authentication independently.

Install the headless package, provision this configuration through Infra, and
manage `hypermesh-host-health` with its included systemd or OpenRC service.
Package installation alone does not enable the service or grant monitoring
authority. `hypermesh-host-health --once /etc/hypermesh/health.json` performs one
cycle for registered CI scenarios; agents do not run it on shared hosts by hand.
Normal service operation samples every 60 seconds. Failures discard the sample;
the next cycle obtains fresh authorization and collects again. Shutdown leaves
no persistent payload. An authorization check failure or changed machine/terms
prevents upload. Numeric OS aggregates are the initial sources; sensors and disk
health sources are described below. No host selling or package-update action belongs here.

The host authority is a point-in-time check, not a transferable grant. The worker
compares both checks to the configured machine and to each other and rejects a
cycle older than 30 seconds before sending. Collection and network operations
are bounded. The collector still authenticates its ingest credential and assigns
tenant identity independently; payloads carry no client tenant or machine tags.

For bounded CI checks, exit 0 means accepted upload, 3 means explicit local
authorization denial, 4 means the host reported its upstream authorization
unavailable, and 1 means another failure. Fixed stderr distinguishes these
outcomes without including upstream details.

## Hardware sources

After host authorization, the worker also reads installed hardware tools. It
never installs packages or starts a monitoring daemon. Each source emits a fixed
`host.<source>.available` gauge. Zero on that gauge means unavailable; missing
measurements are omitted, never represented as zero temperature, power or health.
GPU/board temperatures and GPU power are maxima across the observed devices.
Disk health is true only when every configured disk supplied a passing result.
No device names, UUIDs, serials, free-form labels or raw tool output are uploaded.

- Jetson: `/usr/bin/tegrastats --interval 100`; one line, then stop only that
  invocation. Known temperature tokens and input power are read. No jetson-stats
  dependency is linked or vendored.
- NVIDIA: `/usr/bin/dcgmi dmon -e 150,155 -c 1`; the existing local DCGM host
  engine supplies temperature and power. Unsupported fields remain absent.
- AMD: `/opt/rocm/bin/amd-smi metric --temperature --power --json`; fixed numeric
  temperature and socket-power fields, including explicit JSON units.
- Board: temperatures from at most 32 kernel thermal-zone sysfs entries.
- Disks: optional `smart_devices` in the private health config, for example
  `["/dev/sda", "/dev/nvme0n1"]`, selects at most eight root-owned block devices.
  The external `/usr/sbin/smartctl --json=c --health --attributes` is never linked
  into the agent. It does not request identify information, self-tests, logs or
  serial fields. Only `smart_status.passed` and `temperature.current` enter the
  typed projection; extra fields are discarded. No configured disks means the
  disk source is unavailable. Names are local configuration, never metric labels.

Each command has a two-second deadline and a 64 KiB memory limit, within a
20-second hardware budget. Killed processes stuck in a driver are reaped on
later cycles without blocking; at most eight outstanding invocations are allowed.
Standard error is discarded; raw output is neither
logged nor persisted. Executables and their resolved ancestors must be owned by
root and not writable by group or others. Command options are fixed and no shell
is used. Missing commands, permissions, unsupported formats and timeouts report
unavailable without suppressing other sources. The existing second agreement
check still precedes upload.

Source formats: [NVIDIA tegrastats](https://docs.nvidia.com/jetson/archives/r36.4.4/DeveloperGuide/AT/JetsonLinuxDevelopmentTools/TegrastatsUtility.html),
[DCGM dmon](https://docs.nvidia.com/datacenter/dcgm/latest/reference/command-line-reference/dcgmi/dcgmi-dmon.html),
[AMD SMI](https://rocm.docs.amd.com/projects/amdsmi/en/docs-7.14.1/how-to/amdsmi-cli-tool.html),
and [smartctl](https://github.com/smartmontools/smartmontools/blob/main/src/smartctl.8.in).
Fixture-output unit contracts cover all sources; they do not claim physical
hardware qualification. Installed/native scenarios remain CI-owned.
