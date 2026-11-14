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
existing Rust compiler floor. The existing Azure reporter remains separate and unchanged.

`crash_dialog::review_and_send` displays the exact sanitized JSON and destination,
then sends only on an explicit affirmative choice. Linux uses the packaged Zenity
text viewer with an initially unchecked send checkbox. Windows and macOS use
native dialogs whose default choice declines. Report text is data, never script
source or a shell argument. Dismissal or dialog failure consumes the report
without sending or retrying. Local consent is checked again after the dialog;
withdrawal while the dialog is open refuses the send. This entry point adds no
remote approval route. The daemon and embedded native engine call startup
recovery before accepting work. Set `HYPERMESH_CRASH_CONFIG` to the absolute path
of the private JSON configuration. The containing private directory also owns
`health-consent` (the shared consent store) and `crash-runs` (capture run roots).
Missing configuration leaves this path disabled and creates no state. Startup
purges completed raw runs before showing any preview. Review is synchronous; a
dismissal or failure never produces a background retry. After recovery and
review, startup initializes the packaged guarded handler only if the current
local crash decision is enabled. The native SDK receives an empty DSN and no
upload consent; only the separate sanitized sender can contact GlitchTip.

The Linux daemon accepts the standalone local command `--crash-consent enable`
or `--crash-consent disable`. Enable takes effect on the next startup and does
not approve any upload. Disable requests that the guarded writer stop, waits
for its capture lock, purges raw runs and then records withdrawal. A failed stop
or cleanup returns an error rather than claiming withdrawal succeeded. Windows and macOS expose Crash diagnostics in their local menus. The preview
shows the current choice and destination, and defaults to keeping that choice.
A changed configuration or consent revision invalidates the preview. These
controls are in-process only and add no remote approval route.

`crash_dump::consume` reduces a completed owner-private UUID-named minidump to
one fixed crash-code classification. It bounds file size and stream-directory
reads, excludes memory and strings, and removes the raw file before returning
either a classification or a parse/read error. Unsafe linked files are refused.
`PendingCrash::from_dump` uses the original capture ticket; it never acquires a
new decision for an old dump. Invalidated consent rejects the preview after raw
cleanup. The capture adapter must stop its writer before reduction and handle
startup cleanup and withdrawal. This reader does not install a crash handler or
implement a persistent crash queue.

The native writer must own a `Store::capture_lease` for its entire lifetime.
The lease requires current crash consent and prevents another writer, crash
decision changes or identity resets until the writer stops. Other categories
remain editable. The owner must stop and reap the writer and purge raw files
before completing lease withdrawal; releasing this lock is not itself a way to
stop a native handler. Startup recovery can restore only the exact original
ticket from private capture metadata, never a newly granted revision. Recovered
reports retain the minidump's capture time: stale or future-dated captures are
deleted and refused instead of receiving a fresh 24-hour window.

`crash_recovery::recover` reads stopped capture runs under an exclusive capture
lock. Each run is a private UUID directory with the original serialized ticket
in `ticket.json` and the SDK database in `database`. Completed dump locations
are `pending`, `completed` and Windows `reports`; in-progress `new` files are
never parsed. Invalidated consent discards the run without reading its dumps.
Recovery returns at most five unapproved sanitized reports, removes each entire
raw run before returning any preview, and never follows directory links into
other data. Unknown run names or cleanup failures refuse recovery. The native
adapter must still create these runs and stop its writer; the startup caller
now connects recovery to the per-crash dialogs and sanitized sender.
