# Native session library

The CLI and companion share PKCE and device login, refresh-token storage and
loopback callback handling here. PKCE verifier and state bytes come from the OS
randomness source; failure stops login rather than falling back to a weak source.

`open_system_browser` accepts only bounded HTTP(S) URLs without embedded
credentials or control characters. Linux uses `xdg-open`, macOS uses
`/usr/bin/open`, and Windows uses `ShellExecuteW` on a dedicated COM STA thread.
The URL is passed as data, never interpolated into a shell command. Browser
launch errors do not include the authorization URL or its state value.

Native macOS and Windows try browser login without requiring X11 environment
variables. A reported launch failure selects the existing device flow. An
operator in an unattended or noninteractive session should explicitly select
device login; a successful OS launch does not guarantee a visible browser or a
completed sign-in. The callback timeout and OAuth state checks still apply.

CI runs session unit contracts natively on Linux, macOS and Windows. Tests use
mock token endpoints and in-memory credentials; they do not open a browser,
sign into a provider or qualify a native daemon, installer or credential store.
