# Private desktop bots

Each bot has its own X11 desktop, audio server, daemon, companion, MCP configuration,
identity and persistent home. No host display, input device, Docker socket or network
is exposed. This example gives one bot write access and another read access to a
bounded local exchange volume.

```sh
export VISOR_IMAGE=ghcr.io/hyperme-sh/hypermesh-visor:test
docker compose -p visor -f container/compose.yaml pull
docker compose -p visor -f container/compose.yaml up -d

docker compose -p visor -f container/compose.yaml exec -T writer \
  visor-resource put share example.txt --expected missing <<'TEXT'
Hello from the writer.
TEXT
docker compose -p visor -f container/compose.yaml exec -T reader \
  visor-resource get share example.txt

docker compose -p visor -f container/compose.yaml down
```

The TEST channel follows successful main builds. Pull it again before recreating
a bot to pick up the current build; running containers are not replaced by a
registry tag update alone. Package visibility and registry authentication follow
the repository package settings. For local development, build
`docker build -f container/Dockerfile -t visor-container:local .` and set
`VISOR_IMAGE=visor-container:local`. The daemon's control socket is private to its
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
Archives expire after one day. Main CI additionally publishes the verified native
images to GHCR and combines their registry digests into the moving `:test` channel.
PR jobs cannot publish. No running container is started by these build jobs.

Verify the downloaded archive against the manifest before loading it. After
`docker image load`, compare the loaded image ID and platform with the manifest;
record that image ID when inspecting the archive. A local image ID identifies
image configuration and layers; it is not a registry manifest digest. Main CI
records both architecture manifest digests, the multi-architecture index digest,
source revision, image IDs and archive hashes in `container-test-channel.json`
(retained for 90 days). Source-specific `test-<revision>` tags aid lookup; the
recorded digest identifies exact downloadable bytes. TEST deployments use the
moving channel rather than requiring manual source-pin updates. Publication does
not create Git version tags, stable releases or change package visibility.

Container assignment remains local to the operator: each private home identifies
one bot, and access to `docker compose exec` grants that bot's local control.
There is no physical host-input pause in the private virtual desktop. Its own
companion pause/stop controls and session audit remain active. Shared-file grants
authorize only the mounted directory; they do not grant a remote machine
assignment. Cross-service enrollment, remote assignment and revocation remain
in #30; this image does not expose a local socket or token to the public site.

## Scheduled coverage

The central nightly batch owns the two-bot, shared-resource, restart, MCP and app
scenarios. Do not run those suites manually on hosts or shared resources. PRs
validate unit contracts, Compose configuration and native image builds.

See [the base decision](../docs/container-base.md). Host physical input and native
Wayland automation are separate from this private X11 container environment.
