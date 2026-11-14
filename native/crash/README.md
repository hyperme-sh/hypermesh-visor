# Native capture dependency

`build-sdk.sh` builds Sentry native 0.17.5 at the recorded source commit, with
the exact recursive Crashpad submodule revisions. It creates development build
artifacts, preserves dependency license files and installs the shared SDK plus
its matching handler. It does not change Visor's package version or create a
tag or release. These artifacts are not yet included in the app installers.

The SDK transport is compiled out. Crashpad also has its own sender, so the
native adapter must explicitly use an empty native DSN, require SDK upload
consent and never grant it. The provisioned GlitchTip DSN belongs only to the
separate sanitized-event sender. Per-crash approval must never call the native
SDK's global consent-grant function. Disable proxy inheritance and automatic
session collection before initialization.

Capture must start only under the original device-local crash consent and use
a new private run directory. The writer must own the capture lease until it
has stopped; cleanup must finish before withdrawal succeeds. On Windows and
macOS, `sentry_close` alone does not uninstall all handlers. The adapter must
handle their shutdown and retain any library they still reference. Runtime
capture, process ownership and installer wiring remain under #230.

CI builds on the four native target runners. The small C unit contract checks
SDK options without calling `sentry_init`, opening a desktop or forcing a crash.
The fake Sentry endpoint contract belongs to the Rust sender. Real crash
scenarios run only in the nightly batch after the runtime adapter is connected.

The guarded build links a small Rust consent owner into the stock Crashpad
handler entry point. It validates the private original ticket, signals readiness,
acquires the capture lock and validates again before entering Crashpad. A private
stop request terminates that writer; its operating-system lock is released only
when the process stops. The caller must reacquire the lock and purge the run
before recording withdrawal. Other category changes remain possible.

The checked SDK preparation patch disables automatic handler restart and WER
fallback, rejects any handler upload URL, and exports a policy-version marker.
This prevents a runtime from silently accepting an older unguarded SDK. Native
SDK initialization and bundle installation still need their application caller;
the guard contract tests do not start Crashpad or terminate a process.
