# Linux desktop feedback draft

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
is independent. Windows and native macOS desktop controls are separate surfaces;
this change adds the Linux companion menu entry only.

Validation uses offline companion unit contracts with injected dialog responses.
No desktop session or shared service is opened by those tests.
