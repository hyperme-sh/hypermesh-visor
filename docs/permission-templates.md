# Permission templates and previews

Open **Permissions** from the companion menu, or start the companion with
`--permissions`. Sign in before opening the screen. Select a project directory
and one of four editable presets: local inspection, project development, named
HTTPS access, or scoped administration. Enter explicit targets, one per line,
and byte/time limits. Preferences are private to the local user and scoped to
organization, device and project. Concurrent edits must be reloaded before saving.

The preview shows the current policy decision, normalized target, effective
limits, policy revision and eligibility for later learning. An advisory is still
permitted. A prohibition or extreme concern is displayed separately from a
verification outage. This screen never runs the action or creates an allow.

The host needs a configured permission-preview assignment and the Conductor
`permission-preview` command. Conductor and AIuditor independently check the
current Plane assignment. The host supplies its machine credential privately;
Visor cannot choose the authenticated principal, device or session. Missing or
expired verification returns unavailable. Saved preferences are not authority.

Local paths must be absolute and explicit. HTTPS targets use a canonical domain,
no custom port, query, fragment, credentials or percent escaping. Service targets
are exact names. Targets are matched exactly, not as directory or URL prefixes.
Later slices add execution, verified learned allows, policy changes and dreaming
review; this slice only stores preferences and previews current policy.
