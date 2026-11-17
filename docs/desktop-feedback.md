# Desktop feedback drafts

The Linux companion menu offers **Human feedback** using the existing Zenity
desktop dialog dependency. Opt in before entering a problem or idea, type up to
4096 UTF-8 bytes, and choose **Preview locally**. The preview displays plain text.
**Edit draft** returns to the editor; the next preview uses the replacement text.
**Discard draft**, closing the window, or the 15-minute per-dialog timeout ends
the form. Opening it again starts with fresh consent and an empty draft.

This form keeps text in process memory and dialog pipes. It does not place draft
contents in command arguments, diagnostics or files. Invalid UTF-8, unsupported
control characters, oversized output and blank drafts are refused. User text is
not interpreted as HTML or Markdown. No prompt, document, clipboard, attachment
or diagnostic is read automatically.

Sending stays disabled by default. The Linux dialog has an optional connection
to the native Host grant broker and intake client; the phone and native
Windows forms below remain independent local surfaces.

## Optional Linux submission

The worker reads trusted application settings: `HYPERMESH_FEEDBACK_ENABLED`
must be exactly `true` to select submission mode; absent or `false` preserves
local drafts. No checked-in setting enables it. The existing
`HYPERMESH_HOST_BRIDGE_SOCKET` must identify the root-owned Host broker, and
`HYPERMESH_FEEDBACK_INTAKE_ORIGIN` must be an HTTPS origin without credentials,
path, query or fragment. There is no manual token or destination entry in the form.

Opt in before the worker obtains a grant or reads server consent. Enabling saved
human-reports-v1/product-feedback consent requires a separate confirmation.
Choose a Problem or Idea, enter a title (4–160 characters) and body (10–4000
characters, at most 4096 UTF-8 bytes), review plain text, then explicitly choose
**Send this report**. Editing returns to the editors and requires another preview.

The worker uses the existing observer activity checkpoint before authorized
changes and sending. Network work never runs on the X11 event loop. Fresh grants
must match the initially authenticated issuer, tenant, user and device, and
consent must still have the same enabled revision before confirmation.
No general machine credential enters the form.

An unknown result offers **Retry same report**: only that explicit action resends
the immutable confirmed bytes and idempotency key. No automatic retry occurs.
Closing discards the local copy; it cannot recall an already dispatched request.
**Withdraw saved consent** or **Withdraw consent and discard** explicitly updates
server consent and ends the form. A failed update is reported as unconfirmed,
never as successful withdrawal. Ordinary cancel clears local work without changing
saved server consent. No draft or token is persisted; process-local clearing is
not guaranteed secure erasure of all operating-system memory.

No runtime destination, membership, provisioning or collection is enabled by this
implementation. Installed owner composition and shared journeys remain scheduled
CI work under Hypermesh #79.

Validation uses offline companion unit contracts with injected dialog responses.
No desktop session or shared service is opened by those tests.

## Windows

The native tray menu offers **Human feedback**. Check the explicit opt-in box
before entering a Problem or Idea. **Preview locally** shows plain text; editing
the draft or category clears that preview. The same 4096 UTF-8 byte limit applies.
Unchecking opt-in clears the editor, its undo buffer, and the preview. Closing the
form discards the draft; reopening requires fresh consent. This is process-local
clearing, not a promise of secure erasure from operating-system memory.

Opening this form pauses desktop access and blocks new desktop approval while
it is open. It has no engine calls, file writes, transport, attachments or automatic
collection. Sending remains unavailable. Windows self-test contracts check the
form's consent/reset/edit controls and strict UTF-8 draft validation in CI.

## macOS

The native companion menu offers **Human feedback**. Explicit opt-in enables a
Problem/Idea editor and **Preview locally**. Drafts are limited to 4096 UTF-8
bytes; oversized edits and unsupported controls are refused. The preview is
plain text and any edit or category change clears it. Withdrawal clears the
editor and preview; closing discards the form. Reopening starts with fresh
consent. Undo, rich text, spell-checking and automatic replacement are disabled.
Clearing process-local text does not guarantee secure operating-system memory erasure.

Desktop access pauses while the form is open and requires fresh approval after
closing. With the same explicit settings as Linux, the form can acquire a grant
from the root-owned Unix Host broker and submit to intake. Without those settings
it remains local-only. No destination, credential or membership is provisioned.

Opting in reads saved consent; enabling it requires a separate confirmation.
Enter a report title and body, preview them as plain text, and confirm sending.
The native engine retains confirmed bytes, consent revision and identity. An
unknown response allows only an explicit retry of that same report. Edits discard
the pending confirmation. Withdrawal clears the draft and explicitly updates saved
consent, reporting a failed update as unconfirmed. Closing clears local work; it
cannot recall a request already dispatched. Grant refresh must preserve identity.

Requests run on a dedicated background worker through the in-process engine
interface, not the desktop event loop or an external API. Tokens never enter Swift
form state. There is no automatic retry, attachment, persistence or collection.
Native macOS unit contracts cover consent, preview invalidation, reset, byte
boundaries and invalid text. Rust unit contracts cover the form/session binding,
confirmation, withdrawal and refusal/unknown-result handling with a fake backend.
