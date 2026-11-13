# Apple Silicon development distribution

This archive contains the native development app, the Hypermesh CLI and the
inverse MCP adapter. It is a CI development artifact, not a versioned release.
The app has a local ad-hoc signature only: no Developer ID identity or Apple
notarization. macOS may refuse to open it under its security policy. Inspect the
source and artifact before explicitly allowing a development app; do not disable
system security settings. Signing remains deferred in issue #84.

Only Apple Silicon is currently distributed. Intel is not claimed as supported
until a native build and unit-contract lane exists. The app's Info.plist records
the minimum macOS version used by the CI builder. The native app provides visible
Allow, Pause, Stop and Quit controls; it does not yet have the animated companion's
full feature set. Multi-display capture and pointer coordinates share the active
displays' logical rectangle; display changes pause access until Allow is selected
again. The app shows access state in its window and menu bar; it does not hide or
replace the global cursor in other apps. A click-through animated bug follows
the pointer during allowed automation and disappears on pause. Remaining native
companion controls and Windows support stay in #18. Audio and embedded stdio MCP workers remain unavailable.

## Verify and use

Verify the ZIP against its sibling `.sha256` file, extract it into a new directory,
and verify its contents before use:

```sh
shasum -a 256 -c hypermesh-visor-macos-arm64.zip.sha256
unzip hypermesh-visor-macos-arm64.zip
cd hypermesh-visor-macos-arm64
shasum -a 256 -c checksums.sha256
open 'Hypermesh Visor Development.app'
```

The checksums identify the downloaded bytes; they do not authenticate the publisher.
`source-revision` identifies the source built by CI. Keep the directory together.
Run `bin/hypermesh` for the CLI, or configure an external MCP client to execute the
absolute path to `bin/hypermesh-desktop-mcp`. These launchers default to the native
app's local control socket directory. An explicit `HYPERMESH_RUNTIME_DIR` remains
available for another local instance. Existing CLI endpoint overrides still apply.

The app starts with desktop access off. Grant the OS permissions explicitly and
select Allow in the visible app after a client opens a session. The CLI and inverse
MCP programs cannot lift a native user pause. Permission or input-monitor loss
disables Allow/Resume until Visor is restarted; restore permissions first, reopen
the app and a session, then select Allow. Renter credentials remain in the
existing user credential store; the archive contains no credentials.

## Upgrade and remove

Quit the old app before upgrading. Extract and verify the new archive into a
separate directory, update external MCP client paths to that directory, and open
the new app. Keep the prior directory if rollback is needed. Do not run both app
versions at once: they use the same local socket directory. Replacing a development
app may require granting its OS permissions again.

To remove this distribution, quit the app, remove external MCP client entries and
delete its extracted directory. This archive installs no system service, startup
entry, global command or privileged package. Existing user credentials and config
are deliberately preserved; remove them separately only if you intend to do so.
