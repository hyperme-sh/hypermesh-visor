# Shared vault design for #37

Status: implementation proposal. Cross-bot credential sharing is not enabled.
The wallet signer choice is pending owner input. This document defines the
credential/file boundary independently of that choice; it does not authorize
wallet transactions, custody, export, a release, or a service deployment.

The owning roadmap is [SociACL #155](https://github.com/FyberLabs/SociACL/issues/155)
and its proposed [multi-slot design](https://github.com/FyberLabs/SociACL/blob/811f887a705112b4b7d9a7c856e3fed22f171e72/docs/design/secret-store-vault-design.md).
That design is still under review in SociACL #181, not implemented by this PR.
SociACL owns file/slot formats, sync, sharing, recovery, and lease contracts;
Visor supplies the consuming use/execute/sign broker. v1 agents receive brokered
use only. Plaintext release to agents is deferred to its slice H. The full #37
secret-file/export scope therefore spans later work and is not a v1 completion
claim. Current item-epoch and confidential wrap delivery additionally depend on
[ACL #81](https://github.com/hyperme-sh/acl-service/issues/81).

## Existing components

Visor's [`Vault`](../src/vault.rs) materializes local values and short-lived MCP
results in zeroizing session memory. `SecretSource::Url`, `Chain`, and `Ipfs`
remain unimplemented. `Vault::get` copies bytes to a zeroizing buffer; this is a
local session mechanism, not a synchronized encrypted object store or a grant
verifier. `Vault::redact` skips short and non-UTF-8 values, so it cannot provide
the confidentiality boundary for shared credentials.

[`fetch_secret_bytes`](../src/mcp/secret_fetch.rs) invokes a configured MCP tool
and accepts successful text results. The native macOS app currently refuses
permission-inheriting stdio children in [`attach_one_server`](../src/mcp/stdio.rs).
Shared-vault work must preserve that refusal until an isolated worker exists.

[`container/resource.py`](../container/resource.py) provides bounded flat-file
publication, no-follow opens, and compare-and-swap replacement. Its own module
contract identifies mount permissions as the boundary. A writer can bypass this
cooperative helper; `/resources` must never become a decrypted shared vault.

s3r.ch's [worker artifacts](https://github.com/FyberLabs/s3r.ch/blob/a98a208a6c60fe94d22c788dff5d994de1a6aaea/lib/forum-bot-artifacts.ts)
bind private task results to a bot scope, current reader, local approval, and
expiry. They do not implement credential-use or delegation grants. Its
[SEA recovery envelope](https://github.com/FyberLabs/s3r.ch/blob/a98a208a6c60fe94d22c788dff5d994de1a6aaea/lib/identity/wrap.ts)
is browser-local account recovery, not permission to share an account key.

[SociACL encrypted discovery](https://github.com/FyberLabs/SociACL/blob/d623c10bfe011ce5cc73353c450242841fe74b92/docs/encrypted-discovery.md)
specifies item encryption, signed content/manifests, reader wraps, epoch
selection, cancellation ordering, account history, and nonce/audience-bound
presentation. Those library checks need durable application storage and current
history. A Gun relay, wrap, cached successful check, or browser session cannot
assert that history is complete.

## Object and operation contract

Each object has an opaque stable ID, tenant, owner principal, resource kind,
encrypted revision, signed provenance, and explicit recipient grants. Sensitive
names, descriptions, hostnames, and filenames belong inside the encrypted body.
Visible references contain only IDs necessary for routing and authorization.
The immutable revision identifies the content that was granted and executed;
the running services continue to use their normal moving TEST/minor channels.

The resource kinds are login, API credential, SSH credential, secret file,
prompt, skill, automation, and wallet signer reference. A kind never grants an
operation implicitly. An operation grant additionally binds the object revision,
recipient principal, assigned bot/device/session, purpose, expiry, and allowed
destination/tool. Missing dimensions deny; wildcards and transitive delegation
are absent from the first implementation.

| Operation | Required effect | Does not imply |
| --- | --- | --- |
| Discover | Reveal the minimal object label to this recipient | Read or use |
| Read | Decrypt an explicitly readable value | Execute, share, or modify |
| Use | Ask a broker to apply a credential to an allowed destination | Reveal bytes to the caller/model |
| Execute | Run one approved immutable asset with declared tools/resources | Read unrelated credentials or install changed privileges |
| Modify | Create a new signed revision with a compare-and-swap precondition | Rewrite prior signed history or expand grants |
| Delegate/share | Create a narrower grant only when explicitly permitted | Change tenant, purpose, resource revision, or deadline |
| Export | Reveal an explicitly exportable non-wallet value | Export wallet custody/recovery material |
| Revoke | Cancel a grant and advance required key state | Erase plaintext already received |

SociACL read/post decisions alone do not implement this operation table. Extend
the owning SociACL agent-delegation/lease contract and its use-to-execute mapping
with shared Rust/TypeScript vectors before enabling it; do not introduce a second
Visor grant or encryption format. Unknown operations and
versions deny. Each policy revision is monotonic; cancellation wins an equal
revision. Persist the verifier's revision floor independently of presented data.
The implementation must use the library's verified identity and grant result,
not reimplement cryptographic decisions in a route or model prompt.

## Storage and synchronization

The owner publishes ciphertext, signed immutable content/manifests, per-reader
key wraps, and signed grant/cancellation history. Preserve all records required
to verify rotation, cancellation, and the current epoch; do not prune based only
on a relay's visible rows. The independent authority service supplies a bounded
complete snapshot and fence. Append operations compare that fence atomically
with identity, ACL, policy, and retained-capacity state.

Use the owner-selected container v2 profile and mandatory recovery-key slot,
per-device daily slots, and shared Spanopticon storage namespace from #155.
An account SEA key copied to enrolled devices is not a revocable owner-device
slot. Login gates ciphertext access; it is never a decryption key or a mechanism
for server-side account-key escrow. The proposed v2 formats need their owner
implementation and vectors before Visor consumes them.

Concurrent edits use an expected revision and return a conflict. Clients must
read, display, and explicitly reconcile; they cannot silently overwrite or
restamp a stale policy. Retry only identical signed payloads. Per-owner and
per-recipient retained quotas prevent one grantee from consuming everyone else's
capacity. A rejected write leaves both the revision floor and stored body intact.

Default offline behavior is deny for use, execute, export, and signing. Any later
offline mode needs an explicit owner-issued bounded lease, a monotonic deadline,
and a persisted revision floor. A clock rollback or unavailable current history
cannot renew a lease. Cached ciphertext is not cached permission.

Unattended online use follows #155's signed lease: default/maximum 24 hours,
renewal every two minutes, token life at most five minutes, and five minutes of
grace after token expiry. Renewal never moves the signed hard deadline; extension
requires a new human-signed lease. Check current grants at every use and renewal.
The broker drops the vault key when the human session ends and retains only the
granted item keys under an in-memory lease key. Default placement is the user
device or user-controlled host. Managed-host custody stays off by default and
requires the owner's per-lease opt-in; storage services never gain a general
decryption path. This lease is separate from Visor's short desktop task lease.

## Local broker

Extend the session vault through a broker adapter instead of adding a parallel
plaintext store. The broker authenticates the local peer and independently
checks enrolled bot/device assignment, exact session, current authority, selected
operation, immutable resource revision, purpose, destination, and local approval.
Forum messages and model-generated tool arguments are untrusted request data.
They cannot select a different tenant, grant, credential destination, or tool.

Use operations keep bytes in the broker or an isolated destination adapter.
API injection fixes the approved origin, method class, and credential placement;
redirects cannot forward credentials. SSH operations bind the selected host key
and permitted account/operation. Login injection fixes the approved application
identity and origin. An arbitrary URL, shell fragment, or MCP server name from a
forum post is never a destination selector.

Future slice-H secret files would use one private runtime directory per authenticated workload, with
no-follow directory-relative opens, exclusive creation, one-link regular files,
0600 modes, bounded bytes, and explicit cleanup. Materialization is an explicit
read/export-like disclosure to that workload; it cannot promise non-extractable
use. Never put plaintext in shared `/resources`, repository workspaces, image
layers, process arguments, environment dumps, or persistent container volumes.
These file/mount/environment release adapters are not enabled in v1. First
implement the owner contract's brokered HTTP/git use path and explicit operation
limits; a browser-readable grant does not authorize handing plaintext to an agent.

Recheck permission before each operation and at bounded checkpoints during use.
Revocation stops new operations, cancels active broker work, removes temporary
files, and closes handles where possible. Restart begins unavailable, reconciles
unfinished operations, and removes orphaned private material before accepting
requests. Uncertain signing/remote side effects are not automatically repeated.
Destroying a file cannot retract bytes already copied by an authorized workload.

## Wallet boundary

The wallet object identifies a signer, not a seed phrase or private key. The
provider/OS/hardware choice is pending; there is no generic raw-key backend.
The eventual request contract must bind account, chain, operation, destination,
value ceiling, nonce, deadline, and an exact displayed operation. User approval
must cover those bytes, not a mutable summary. An address-read grant never
authorizes signing or spending. Recovery stays with the provider/user.

## User controls and output handling

s3r.ch must show current recipient, operation, purpose, immutable version,
deadline, delegation limits, and revocation state before a grant is confirmed.
Changed skill privileges require a new explicit execution decision. Shared
prompts and skills remain data until an execution grant and tool/resource policy
both allow them. No forum card or generated instruction can approve itself.

Logs and forum posts contain operation IDs and fixed outcome categories, not
credential values, private paths, object names, key wraps, or signed request
payloads. Keep credential-bearing surfaces out of model captures/transcripts by
construction; value replacement is only a secondary precaution. Retain a bounded
owner-visible audit of policy changes and operation outcomes without secret data.
Treat screenshots and crash artifacts as private until their own grants permit
access. A successful bot report never substitutes for a broker authorization.

## Recovery and implementation order

Encrypted backup must include signed manifests, reader wraps, cancellation/key
history, and monotonic verifier floors. Losing current history denies access;
restoring an old backup cannot restore a revoked grant. Lost-device handling
revokes that device, advances the appropriate key state, and reconciles pending
operations. Root/admin control of a live endpoint remains outside this encrypted
storage boundary.

1. Finalize the threat model and signer choice; publish canonical operation-policy
   vectors in SociACL. No sharing is enabled by this design document.
2. Implement durable encrypted objects/history, atomic revisions, quotas, and
   grant/revoke UI in s3r.ch/ACL. Preserve complete independent authority.
3. Add the authenticated Visor broker, isolated use adapters, owner-defined
   unattended leases, cancellation, and restart recovery. Agent plaintext
   materialization remains in deferred slice H.
4. Add signer-specific handling after the owner-selected boundary is known.
5. Run unit/contract tests in PR CI and the authorized central nightly batch.

Required contracts cover one user/two allowed bots plus a denied bot; each
operation independently; rotated/revoked identities; stale/missing history;
same-revision cancellation; cross-tenant/session/purpose substitution; replay;
concurrent edits; offline expiry/clock rollback; symlink/hardlink/path escape;
restart cleanup; revoked active use; and refusal to place test secrets in logs,
posts, model inputs, screenshots, packages, or shared directories. The nightly
batch covers the assembled services; no manual host suite is introduced.
