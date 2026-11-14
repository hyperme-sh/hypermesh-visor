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
Off-category captures must be discarded by the caller without queueing. Before
each upload, the authenticated consent service must confirm that exact ticket;
only its trusted adapter may call `synchronized`. The confirmation expires after
five seconds, permits one upload attempt and is never persisted. Restart requires
fresh synchronization. The ingestion service must independently check the current
revision. This library cannot authenticate a server response on its own.

The upload callback must perform one synchronous, bounded send with a short
timeout, without deferred work or retries. It runs under the cross-process lock.
A concurrent withdrawal must wait or return an error and retry; it cannot report
success while the upload is active. After successful withdrawal, old tickets are
refused, including after a later opt-in. The controller must stop sampling and
purge that category's queued payloads before reporting withdrawal success. The
store contains decisions only and cannot purge an external queue. On logout or
scope changes, consumers must stop collection and dispose their old store and
tickets; they must not keep an old identity active in a background sender.

These primitives do not complete either delivery issue. Consent UI, authenticated
synchronization, queue ownership, payload allowlists and transport wiring remain
required. Crashpad capture stays disabled pending resolution of the no-memory-dump
boundary in the approved privacy design.
