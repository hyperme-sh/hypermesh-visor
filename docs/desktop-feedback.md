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

Sending is unavailable: this slice has no intake transport, identity conversion,
credential prompt, persistence or direct GitHub action. Native issuer/device
authentication and intake authorization remain in Hypermesh #79. The phone form
is independent. The native desktop forms below are independent local surfaces.

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
closing. There is no transport, persistence, attachment or automatic collection.
Sending remains unavailable. Native macOS CI unit contracts cover consent,
preview invalidation, reset, byte boundaries and invalid text without showing
the form or opening a shared desktop session.
