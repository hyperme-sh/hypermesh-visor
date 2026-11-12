# Private desktop bots

Each bot has its own X11 desktop, audio server, daemon, companion, MCP configuration,
identity and persistent home. No host display, input device, Docker socket or network
is exposed. This example gives one bot write access and another read access to a
bounded local exchange volume.

```sh
docker build -f container/Dockerfile -t visor-container:local .
export VISOR_IMAGE=$(docker image inspect visor-container:local --format '{{.Id}}')
docker compose -p visor -f container/compose.yaml up -d

docker compose -p visor -f container/compose.yaml exec -T writer \
  visor-resource put share example.txt --expected missing <<'TEXT'
Hello from the writer.
TEXT
docker compose -p visor -f container/compose.yaml exec -T reader \
  visor-resource get share example.txt

docker compose -p visor -f container/compose.yaml down
```

Use an immutable registry digest when distributing an image. The local build above
uses its immutable local image ID. The daemon's control socket is private to its
container; run clients using `docker compose exec -T`. The inverse MCP executable
is `/usr/local/bin/hypermesh-desktop-mcp`. The bundled Go CLI is
`/usr/local/bin/hypermesh`; it uses the container's private runtime directory and
control socket by default. For example, `docker compose -p visor -f
container/compose.yaml exec -T writer hypermesh --help` lists its commands.
Run a bot's CLI in that bot's container, never by sharing its socket or home.
The CLI is built from the vendored source recorded in
`/usr/share/doc/hypermesh-cli/VENDOR_REV`, with a read-only module lock and no CGO.
Native image builds run its unit/contract tests and executable help check.

Homes survive container recreation. The shared exchange is **ephemeral**, limited
to 256 MiB, and may disappear when unmounted. For durable sharing, provision a
separate quota-enforced external volume. Never share a bot's home, credential
store or control socket. Do not use `down -v` unless intentionally deleting state.

`visor-resource` accepts flat filenames, refuses symlinks and hard links, and
publishes atomically with `--expected missing` or the previous SHA-256 for conflict
detection. Files are limited to 16 MiB; cooperative publishers share a lock and a
240 MiB budget. The mount's 256 MiB quota remains the enforcement boundary even
when a writer bypasses the helper. Read-only grants are enforced by the mount.
An authorized writer can replace or delete shared files. Share only with trusted
writers. Interrupted temporary publications count against quota; inspect and
remove them explicitly rather than deleting unrelated resources.

Networking is off by default. Enabling network access is an operator decision;
apply an egress policy appropriate to the apps and MCP services in use. Local
HTTP/SSE fixtures work over each container's own loopback interface.

## Native CI builds

PR and main CI build the image on native amd64 and arm64 runners. Each
`visor-container-<architecture>` artifact contains a compressed Docker image
archive and `manifest.json` recording its SHA-256, byte size, source revision,
platform and local image ID. CI checks the unprivileged user, entrypoint,
readiness command and absence of implicit ports or volumes before upload.
Artifacts expire after one day and are development downloads, not registry
releases. No running container is started by these build jobs.

Verify the downloaded archive against the manifest before loading it. After
`docker image load`, compare the loaded image ID and platform with the manifest;
use that image ID as `VISOR_IMAGE`. A local image ID identifies image configuration
and layers; it is not a published registry manifest digest. Registry distribution
and the remote assignment contract remain tracked in #27.

## Scheduled coverage

The central nightly batch owns the two-bot, shared-resource, restart, MCP and app
scenarios. Do not run those suites manually on hosts or shared resources. PRs
validate unit contracts, Compose configuration and native image builds.

See [the base decision](../docs/container-base.md). Host physical input and native
Wayland automation are separate from this private X11 container environment.
