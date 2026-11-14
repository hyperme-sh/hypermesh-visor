# Native capture dependency

`build-sdk.sh` builds Sentry native 0.17.5 at the recorded source commit, with
the exact recursive Crashpad submodule revisions. It creates development build
artifacts, preserves dependency license files and installs the SDK plus
its matching handler. macOS links the SDK statically; Linux and Windows load
the packaged shared library. It does not change Visor's package version or create a
tag or release. The desktop packages stage the matching handler and required runtime files.

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
handle their shutdown and retain any library they still reference. The runtime adapter retains the library for the process lifetime and stops
the guarded writer before withdrawal purges its private run.

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
This prevents a runtime from silently accepting an older unguarded SDK. Startup recovery and review run before the adapter initializes the SDK.
The guard contract tests do not start Crashpad or terminate a process.

## Building desktop bundles

CI calls the reusable SDK build for each target and stages only artifacts whose
`visor-source-revision` matches the application checkout. Set
`HYPERMESH_CRASH_SDK_PATH` to that artifact for Windows and macOS builds. Linux
packaging uses `HYPERMESH_CRASH_SDK_X86_64` and
`HYPERMESH_CRASH_SDK_AARCH64`. The container builds its SDK inside a separate build stage from the same
application source tree. When Git metadata is absent it records a SHA-256
digest of the capture build inputs in place of a Git revision.
No private runtime configuration or DSN belongs in these build artifacts.

The standalone Linux executable and musl headless bundles do not include a
native capture SDK. Capture remains disabled without local consent and a
matching installed desktop SDK. macOS retains the existing ad-hoc development
signing and hardened runtime settings; distribution signing remains deferred.
