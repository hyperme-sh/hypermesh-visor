# Native feedback intake transport

`feedback_intake::IntakeClient` is disabled by default. Trusted application configuration must explicitly provide an HTTPS intake origin and a component name. This library does not configure a destination, enable a form, change consent automatically, or send reports in the background.

The caller obtains a short-lived `hm_feedback_` bearer from the native Host grant broker. There is no manual credential entry. Local claim checks bind confirmed report bytes to the issuer, user, tenant and device; they do not verify the signature. Intake remains responsible for signature verification, current membership and current consent.

`read_consent` reads the fixed human-report consent. `change_consent` uses its expected revision; enabling requires local opt-in, while withdrawal is allowed after local opt-in is cleared. A caller withdrawing consent must discard its confirmed report and stop any unsent work. A request already dispatched cannot be recalled.

After explicit preview confirmation, `confirm` captures the exact title/body, category, component and consent revision in an immutable in-memory object. `submit` requires local opt-in, the same intake origin and a fresh grant for the same identity. On an uncertain result, retain that same object for an explicit retry so both the request bytes and idempotency key remain unchanged. Editing, withdrawing or changing accounts requires discarding it. The library never retries automatically or persists the report.

All network methods block and must run on the caller's background worker. Requests use fixed consent/report paths, a 12-second deadline, no redirects, no environment proxy discovery and bounded responses. Errors contain fixed text; returned submission data contains only a server report ID and state. Confirmed report text is redacted from Debug output and its serialized buffer is cleared on drop.

Unit contracts cover disabled configuration, consent revision matching and withdrawal, grant expiry and identity, immutable retry bytes, malformed responses, response limits and redirect refusal. The desktop forms and grant-client composition remain separate work under #79.
