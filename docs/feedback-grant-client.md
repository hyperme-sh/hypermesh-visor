# Unix feedback grant client

The native library exposes `feedback_grant::FeedbackGrantClient` for the
Host broker contract introduced in Host #235. Its default has no socket and
cannot connect. An explicit configured absolute socket path and a true local
human-reports opt-in assertion are required for a request.

The client authenticates the Unix peer as root before writing HTTP. It sends
only version 1 and the opt-in assertion to the fixed `/feedback-grant` path.
It supplies no tenant, device, intake destination or general machine credential.
Replies are limited to 4096 bytes, require status 200 and the exact version-1
envelope, and must contain a bounded compact `hm_feedback_` credential with
the Host contract's 60-second lifetime field. Host checks the actual grant
claims; intake independently verifies the signature and expiry.

The credential's Debug representation is redacted and its owned token buffer is
cleared on drop. This is not guaranteed erasure of transport buffers or copies
made by callers. Use it immediately with the configured intake, never as a
cached credential or general authority. Errors expose a fixed refusal, and the
client does not retry, persist, log, collect text or submit reports.

This is a callable library prerequisite, not a connected form or a new local
HTTP/MCP route. Existing phone and desktop forms still cannot send. OS-level
Windows feedback transport, form consent/confirmation, intake composition and
configured membership/destinations remain Hypermesh #79 work. No configuration,
provisioning or collection is enabled.

CI unit contracts substitute a local Unix broker and the expected peer UID.
They cover the fixed wire request, opt-in refusal before connection, body limits,
HTTP/redirect/malformed replies and no retry. The shared bridge's peer-refusal
contract covers refusal before HTTP is written. These tests do not qualify an
installed Host, real Plane grant or deployed intake.
