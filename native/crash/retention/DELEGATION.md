# Host container cleanup verification contract

Prerequisite: [Visor #293](https://github.com/hyperme-sh/visor/issues/293).
Consumer: [Host #232](https://github.com/hyperme-sh/host/issues/232).

The `delegation` module provides a verification contract only. No command,
listener, native guard branch, mount or container capture is enabled here.
Existing Visor consent and direct Host cleanup checks remain unchanged.

`HostSession::open` accepts a canonical run UUID and exact 64-character Docker
container ID only after the current Host Terms and OS-managed cleanup service
admit an empty run. The signer is ephemeral and has no serialization or key
export API. Public configuration binds protocol version, run, worker, random
session and Ed25519 public key. Existing retention remains the only owner of
the account-wide quota and original run age; a new signing key does not reset it.

A worker creates a random 32-byte challenge before transport begins. The fixed
101-byte request contains `HMCD` plus version byte 1, the 16-byte run UUID,
32-byte container ID, 16-byte session UUID and challenge. A successful response
contains exactly that request and a 64-byte Ed25519 signature. There is no
positive response without another current Host Terms and live cleanup check.
Failure permanently revokes the session. The verifier consumes the pending
challenge, checks the whole frame and signature, and refuses after 500 ms on a
monotonic clock. Each write needs a new challenge; permits must never be cached.

The Host consumer must supply all of these before enabling native writes:

- Derive the peer's container ID from authenticated OS credentials and a stable
  process/cgroup binding. Do not pass a container ID claimed in request bytes.
- Mount public configuration read-only from a protected Host location. A
  self-signed response does not establish trust in an arbitrary public key.
- Bound frame reads and total connection time before calling the contract.
- Keep the run's control metadata and signing authority outside worker-writable
  storage. Expose only the bounded raw database and required public data.
- Preserve the shared capture lock, quota, original expiry, stop handling and
  cleanup-before-reporting order. Account for process exit, Host restart,
  service restart, revocation, suspended workers and unavailable transport.
- Add the guarded SDK to supported container recipes and retain the current
  capability, PID, network and filesystem restrictions. Do not copy enrollment
  state or machine credentials into workers.

Host #232 stays open until those consumers and their CI contracts are complete.
Raw dumps stay local. Collection and digest activation remain disabled.
