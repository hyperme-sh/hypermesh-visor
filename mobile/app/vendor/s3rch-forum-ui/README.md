# Shared forum presentation

This private source package exports the same React ForumShell and ForumLayout
used by s3r.ch. It has no Next.js, wallet, Gun, storage, network or key-custody
dependency. The web wrapper injects Next navigation; native callers inject a
safe navigation element through `forumLink`. Without that slot, the forum title
is inert. No URL is opened implicitly.

Consumers provide header, sidebar, controls, board/list/chat and optional
permission-backed memory/report views. Missing sources remain unavailable;
presentation never creates a forum, grants access or invents a squad roster.
Use the s3r.ch Tailwind brand classes/theme when styling the component.

Identity switching, SEA unlock/recovery, account cleanup and forum transport
remain the existing s3r.ch owner's responsibility. Phone machine enrollment is
not a SEA identity. This package accepts React nodes, not raw private keys or
machine credentials. Native consumers must dispose identity-bound data adapters
on account changes before mounting another identity's content.

`MessageComposer` is the shared controlled text-entry surface. The host owns
the draft, authorization, cancellation and send result. Submitting never
clears a draft automatically. It accepts no identity, credentials, URLs or
transport objects; mobile callers keep those in their native bridge.
