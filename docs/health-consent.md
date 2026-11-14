# Local category consent

`health_consent::Store` is the shared local decision store for #230 and #171.
It is a library boundary, not an enabled collector, uploader, consent screen or
server synchronization client. The existing Azure reporter is unchanged. Native
SDK integration and the crash payload path remain in #230.

The caller supplies a dedicated owner-controlled directory and trusted subject,
tenant and device IDs, plus notice and purpose versions. Before calling `decide`,
the UI must display the category purpose, allowed fields, destination, retention
and current choice. Enrollment and configuration are not opt-in. Five independent
categories default off: OS health, sensors, service health, crashes and human
reports. An identity or notice change requires fresh decisions.

Private `consent.json` contains version 1, the bound scope and five decisions in
the above order. Each explicit decision receives a new UUID revision, even when
its boolean is unchanged. Atomic replacement and the existing owner-bound local
storage checks protect writes; a private lock serializes processes. Corrupt or
unsafe state is refused. Missing state defaults off. No event payloads, tokens,
names or diagnostic text belong in this store.

Capture requires an enabled current category and returns a scope/revision ticket.
Off-category captures must be discarded by the caller without queueing. In v0,
crash consent stays on the device: no consent synchronization service is needed.
The crash sender checks the original ticket against the current local decision
under the same lock used by withdrawal. The older `synchronized` / `upload`
interface is separate from this v0 crash path and does not impose a server gate
on it. Any future server-consent contract belongs to a separate owner slice.

The upload callback must perform one synchronous, bounded send with a short
timeout, without deferred work or retries. It runs under the cross-process lock.
A concurrent withdrawal must wait or return an error and retry; it cannot report
success while the upload is active. After successful withdrawal, old tickets are
refused, including after a later opt-in. The controller must stop sampling and
purge that category's queued payloads before reporting withdrawal success. The
store contains decisions only and cannot purge an external queue. On logout or
scope changes, consumers must stop collection and dispose their old store and
tickets; they must not keep an old identity active in a background sender.

These primitives do not complete either delivery issue. Consent UI, capture and
queue ownership remain required. The owner approved private local-only minidumps for sanitization on
November 14, 2026, with a separate request before each sanitized upload. Raw dumps
must never be uploaded. This exception does not enable capture by itself: the
native adapter still needs private capture storage, sanitization and immediate
raw-dump cleanup on success, denial, withdrawal or failure.

`crash_report::PendingCrash` implements the sanitized-event and approval boundary.
It builds a fresh minimal event with a fixed error-code mapping, compiled release,
event ID, time and tenant tag. SDK messages, stacks, contexts, attachments and
breadcrumbs are excluded. Only sanitized JSON is exposed for a preview. The
operator must explicitly approve its exact event ID, payload hash and destination;
category opt-in is not report approval. Approval expires after five minutes and
reports expire after 24 hours. A send consumes the report even on failure or an
unknown outcome. Withdrawal and the current consent revision are checked at send.
Approvals are not serializable or restored after restart.

`crash_upload::Config` reads owner-private JSON containing `dsn` and the `scope`
fields above. Hypermesh #80 provisions the project-scoped, ingest-only GlitchTip
DSN; do not put its value in code, issues, logs or the preview. The preview shows
only the credential-free envelope endpoint. There is no custom ingestion API or
manually issued JWT. Missing config disables this path; invalid config fails shut.

The sender uses the Sentry core SDK with a synchronous single-attempt transport.
It sends the exact approved JSON in a standard Sentry event envelope, without SDK
enrichment or attachments. It authenticates with the DSN via `X-Sentry-Auth`,
rejects hosted Sentry domains, disables redirects and environment proxies, and
uses a five-second timeout. TLS is required outside numeric loopback; a private
IP alone does not establish WireGuard routing. The SDK version retains the
existing Rust compiler floor. No native dialog or capture adapter is installed
by this module. The existing Azure reporter remains separate and unchanged.

`crash_dump::consume` reduces a completed owner-private UUID-named minidump to
one fixed crash-code classification. It bounds file size and stream-directory
reads, excludes memory and strings, and removes the raw file before returning
either a classification or a parse/read error. Unsafe linked files are refused.
`PendingCrash::from_dump` uses the original capture ticket; it never acquires a
new decision for an old dump. Invalidated consent rejects the preview after raw
cleanup. The capture adapter must stop its writer before reduction and handle
startup cleanup and withdrawal. This reader does not install a crash handler or
implement a persistent crash queue.
