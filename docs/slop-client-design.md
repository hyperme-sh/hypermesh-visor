# Optional SLOPCANNON MCP client

Design for [#170](https://github.com/hyperme-sh/hypermesh-visor/issues/170).
This note does not authorize implementation, account creation, terms acceptance,
uploads, spending or provider activation. Visor would be a client of SLOP, LLC's
hosted service. [#169's server design](mcp-server-design.md) is the opposite
direction; #29 remains responsible for discovering third-party applications.

## Account and credentials

Use the user's own slop.cc account. Prefer a SLOP, LLC-issued Visor OAuth client
with authorization code and PKCE, followed by the documented short-lived session
credential. Do not reuse the first-party `hermes-agent` client ID. Until Visor has
its own issued client, a human may create a scoped bearer token in the account UI
and save it through Visor's private secret-store interface. An unavailable secure
store blocks setup; plaintext configuration is not a fallback.

The proposed connector sends the bearer on every MCP request, including
`initialize`, to `https://api.slop.cc/mcp`. OAuth-capable clients use human browser
authorization. Never put tokens in chat, prompts, tool arguments, frontend bundles,
committed files, logs or process arguments. A provider/OpenRouter key is not a
SLOPCANNON credential. Do not forward credentials across redirects or to URLs
advertised by an untrusted host. Expiration or revocation stops further calls.
Official setup: [agent guide](https://slop.cc/for-agents) and
[API documentation](https://api.slop.cc/docs).

## Approval before uploads and spending

Every tool call requires AIuditor RBAC and a current account-bound grant, including
read operations. The connector must fail closed if the decision adapter is absent
or unavailable. Policy authorization does not substitute for either human decision
below.

Before any media upload, fetch the current
[`/v1/rights-attestation`](https://api.slop.cc/v1/rights-attestation), display the
platform statement and Terms/Acceptable Use links verbatim, and wait for explicit
human assent. Bind that decision to the authenticated human, account, session,
selected media and statement version/digest. Changed media or statement requires
a new decision. Do not invent or edit the statement, accept it on the person's
behalf, or infer assent from silence or a previous session.

Before any spending call, including at least `generations:create` and
`billing:purchase`, show the exact quoted Cannon Credit total and production
ceiling in the thread and require affirmative human approval. Bind the decision
to account, project, operation, arguments, quote identifier/expiry, ceiling and
human identity. Recheck the current quote and remaining authorized budget at
dispatch, atomically reserve budget, and consume the operation approval once.
Changes, expiry or ambiguous submission status block retries until reconciled.
Unknown tools with possible spending effects remain denied until classified.

The owning inline decision contract is
[s3r.ch #335](https://github.com/FyberLabs/s3r.ch/issues/335). It must independently
authenticate the human approver and prevent replay. Existing bot-signed forum
cards report private operator grants; their text/signatures are not human spending
authority. This design does not invent an implemented SEA decision endpoint.
Paid calls remain unavailable until that contract and its Visor consumer exist.

No automatic top-ups. The account holder opens SLOP's Stripe checkout to buy
credits; generation approval cannot authorize a purchase. No x402 or other
non-Stripe funding, even if advertised by the service. No Organizations reseller
API or resale of Cannon Credits; any future reseller arrangement requires a
written agreement with SLOP, LLC and separately authorized scope. A
[paid-agent contract](https://github.com/FyberLabs/s3r.ch/issues/331) uses only the
contracting party's own account and preserves all approval and rights rules.

## Live capabilities and data handling

Before each production run, read
[`/v1/capabilities`](https://api.slop.cc/v1/capabilities) and its advertised contract
URLs. Validate HTTPS origins against configured service boundaries before fetching
or attaching credentials. Discover current schemas rather than freezing tool
lists, prices or model names. Tool presence does not imply readiness: require
`submissionReady` and each operation's related flags. Missing flags, incompatible
contracts or changed prices invalidate the plan/quote approval and stop paid work.
Track accepted job IDs to reconcile uncertain submissions without duplicate spend.

Setup must explicitly disclose that SLOP, LLC receives project names, prompts,
scripts, lyrics, songs, reference images, takes and exports. Generation goes to
OpenRouter and selected model providers. Hypermesh does not log prompt bodies.
Keep media, tokens and signed download URLs out of diagnostics; retain only bounded
operation/status identifiers needed for recovery. AIuditor sees Visor tool calls,
not provider traffic that never passes through Conductor. Link the current
[privacy policy](https://slop.cc/privacy), [terms](https://slop.cc/terms),
[acceptable use](https://slop.cc/acceptable-use) and
[credit information](https://slop.cc/wiki/cannon-credits) during setup.

Bundling/forking SLOPCANNON and routing its video generation to a Conductor or
Hypermesh endpoint are outside scope; no self-service BYO generation base URL is
assumed. App signing deferral does not authorize wallet signing or payments.

## Later implementation checks

Separate implementation authorization must precede any connector or activation.
Unit/contract coverage must exercise credential redaction and redirect refusal,
policy denial, absent inline authority, forged bot approvals, quote changes,
concurrent budget reservations, replay, current rights assent, capability changes,
service unavailability and ambiguous job recovery. Fixtures must not upload media,
accept terms or spend credits. No service call was enabled by this design.
