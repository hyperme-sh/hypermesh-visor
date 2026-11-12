# Remote bot polling

Visor can opt into the Plane bot registry from hypermesh-plane #112. It opens
outbound HTTPS requests only; the site never receives a local socket or local
Visor API token. Remote tasks use the existing bounded local task handlers.

Set `HYPERMESH_BOT_CONFIG` to an owner-only JSON file, with these fields:

```json
{
  "origin": "https://your-configured-plane.example",
  "tenant_id": "00000000-0000-4000-8000-000000000001",
  "bot_id": "00000000-0000-4000-8000-000000000002",
  "session_id": "00000000-0000-4000-8000-000000000003",
  "mode": "interactive",
  "credential_file": "/absolute/private/path/bot-token"
}
```

Both files must be regular files owned by the Visor user with mode `0600`.
Symlinks, shared permissions and files larger than 8 KiB are refused. The token
file contains the scoped `hm_bot_` credential returned once by the owner-only
Plane enrollment endpoint. It is different from the local API bearer. Keep it
outside model context, workspace attachments and forum messages.

Start with `session_id` set to `00000000-0000-0000-0000-000000000000` while no
local session is selected. After Visor starts, create a session through the local
companion and replace that field with its UUID, preserving mode `0600`. The
client rereads the selected session every poll, so no restart is needed. The
owner assigns that same session in the registry. Changing the bot identity,
origin, mode or credential path requires restart; changing sessions during an
active task stops the daemon instead of moving that task. Only advertised `desktop`
tasks are accepted, with the local queued-task `steps` payload. An assignment
for any other session is refused. A new process uses a new connection UUID;
lost completion acknowledgments are not replayed.

The native/local companion must grant consent. Remote resume never starts local
automation or lifts a physical-input pause. Container mode must be explicitly
selected at enrollment and in this config; it also requires local consent and
does not bypass the companion safety controls. The owner UI is available at
`/dashboard/hypermesh/bots` (Portal #114), through Edge #59.

Polling uses two-second request timeouts and does not follow redirects. Each
claim carries an absolute authorization deadline, capped at 15 seconds. The
local queue counts waiting time against that deadline and cancels work when its
consent epoch changes, including inside the input handlers. Heartbeats cannot extend an already-delivered task.
Results remain local; the service receives terminal status and correlation IDs,
not screenshots, typed text or local result contents.

A request failure, invalid response binding, expired authorization, or local
submission failure pauses automation and triggers the daemon's coordinated
shutdown. Restart and renew local consent to reconnect. There is no automatic
retry of desktop work. This behavior is opt-in; without the config variable no
bot service requests are made.

## Dedicated unattended containers

`container/compose.remote.yaml` opts a dedicated private desktop into one local
startup consent. It has no host devices, host sockets or published ports.
Registration alone never enables this mode. Set `VISOR_IMAGE` to the moving
`ghcr.io/hyperme-sh/hypermesh-visor:test` channel, or a verified downloaded image.
Provide `VISOR_BOT_CONFIG_FILE` and `VISOR_BOT_TOKEN_FILE` as absolute paths to
files owned by UID 1000 with mode `0600`. Enroll the bot with mode `container`;
set config `mode` to `container`, `credential_file` to `/run/secrets/bot-token`,
and the initial `session_id` to the all-zero UUID above.

On an explicit container launch, `HYPERMESH_CONTAINER_UNATTENDED=1` copies the
read-only config into private temporary storage, creates one local session and
requests start once through the local companion socket. The session UUID is
printed in the container log for assignment in Your bots. Old assignments are
not claimed after a new session is created. Keep the credential file private;
it is never printed or sent to the local session API.

The companion continues monitoring the desktop. A pause is never automatically
resumed. Remote pause/stop cannot be undone by another remote task. This Compose
configuration disables automatic restart: after failure, the owner must launch
the container again and assign the newly created session. With the flag absent
or `0`, startup retains the normal explicit local consent requirement. Any other
flag value or a non-container enrollment config is refused.
