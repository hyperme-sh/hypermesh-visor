# Visor issue and CI coverage

This map connects the open delivery issues to implementation work and the central
nightly batch. A registered lane, unit test or successful build does not complete
an issue whose remaining scope includes another platform or service. Keep those
issues open until their stated scope is implemented.

The owner confirmed two decisions on 2026-11-11: signing remains deferred (#84),
and the inverse MCP adapter supports queued bot tasks as well as direct control
of an existing assigned session (#32). Queued jobs remain bound to that session;
they do not create a new bot identity or grant assignment authority.

| Issue | Current work and remaining scope | Nightly coverage needed |
| --- | --- | --- |
| [#17](https://github.com/hyperme-sh/hypermesh-visor/issues/17) Wayland | Native portal/libei input and real-user pause remain unimplemented; refusal stays explicit. | GNOME and KDE permission, input, interruption and overlay scenarios. Existing Wayland refusal tests do not cover this. |
| [#18](https://github.com/hyperme-sh/hypermesh-visor/issues/18) Native companions | The macOS development app embeds the daemon with visible Allow/Pause/Stop controls and an in-process desktop callback. Windows support, native animated companion parity and isolated stdio workers remain open. | Separate native Windows/macOS desktop adapters with physical-input pause and recovery. |
| [#25](https://github.com/hyperme-sh/hypermesh-visor/issues/25) Windows distribution | Native IPC, workspace support and unsigned installer remain open. Portable discovery tests are only one part. | Native installed Windows session and upgrade/removal checks. |
| [#26](https://github.com/hyperme-sh/hypermesh-visor/issues/26) macOS distribution | Native Apple Silicon development ZIPs include the app, CLI and inverse MCP adapter with file checksums and a source revision. Capture and input share a bounded multi-display layout. Broader native permission, keychain and application coverage remain open; signing is deferred in #84. | Apple Silicon permission refusal/recovery, installed application, interruption and keychain checks. |
| [#27](https://github.com/hyperme-sh/hypermesh-visor/issues/27) Container bots | Debian desktop image, bundled CLI, scoped resource prototype and native amd64/arm64 CI archives exist. Registry distribution and the full assignment contract remain open. | `visor-container` repeats two-bot isolation, resource, restart, MCP and app checks on x64; add native ARM64. |
| [#28](https://github.com/hyperme-sh/hypermesh-visor/issues/28) Verb contract | Complete audit: HTTP/MCP inventory, limits, errors, coordinates, cancellation and task classification are documented. Missing native/service implementations stay in their own issues. | Browser/editor/terminal and display regression coverage stays in the scheduled batch; unsupported operations are explicitly classified in the verb contract. |
| [#29](https://github.com/hyperme-sh/hypermesh-visor/issues/29) MCP discovery | Shared discovery has native filesystem unit checks. Tool I/O no longer holds the global session lock, and audit outcomes survive session close. Explicit resource access and offline argument/result schema validation are implemented. Additional integrations, native app matching and reconnect policy remain open. | Native platform application matching, permission/refusal, refresh and disconnect cases. |
| [#30](https://github.com/hyperme-sh/hypermesh-visor/issues/30) Bot assignment | Cross-service enrollment, ownership, heartbeat, assignment and revocation remain open. Local session IDs are not remote assignments. | Infra-owned assigned-machine/container fixtures with denied owner, expired assignment and duplicate dispatch. |
| [#31](https://github.com/hyperme-sh/hypermesh-visor/issues/31) Forum tasks | Durable s3r.ch task delivery, membership, deduplication and authorized artifact references remain open. | One user/two bots, cross-thread refusal, replay, reconnect and cancellation. |
| [#32](https://github.com/hyperme-sh/hypermesh-visor/issues/32) Inverse MCP | Session-bound bounded task submit/status/cancel, result resources, approved verbs, initialization and in-flight RPC cancellation are implemented. | MCP client subprocess contracts cover HTTP/Unix parity, denied requests, cancellation and disconnect cleanup. |
| [#34](https://github.com/hyperme-sh/hypermesh-visor/issues/34) Delivery tracker | This map records the remaining scope; it does not replace the implementation issues. | Link each platform/service adapter and its scheduled results. |
| [#35](https://github.com/hyperme-sh/hypermesh-visor/issues/35) Desktop base | Complete: Debian 13, private Xvfb/Openbox and pinned app packages are selected in `container-base.md` and implemented in `container/Dockerfile`. Headless Alpine is a separate deployment mode. | The central `visor-container` lane retains regression coverage; native ARM64 deployment remains tracked by #27. |
| [#36](https://github.com/hyperme-sh/hypermesh-visor/issues/36) Downloads | Complete for published v0.1.0: architecture/format selectors, checksum-verified install commands, removal guidance, first-capture instructions and docs links are merged. Source/CI artifacts are not published native/headless downloads. | Versioned install-to-first-task and website navigation checks for each published platform. |
| [#37](https://github.com/hyperme-sh/hypermesh-visor/issues/37) Shared vault | Cross-bot encrypted sharing, operation-specific grants, freshness and signer boundaries require coordinated SociACL/s3r.ch work. The headless session file store is not a shared vault. | Scoped user/two-bot sharing, denial, revocation, rotation, replay, offline expiry and secret containment. |
| [#84](https://github.com/hyperme-sh/hypermesh-visor/issues/84) Signing | Deferred by owner. Do not publish signed/notarized claims or add approval gates. | Signing checks only after the required identities and specific release authority exist. |
| [#88](https://github.com/hyperme-sh/hypermesh-visor/issues/88) Companion nest | One-default/up-to-four local companions, isolated app sessions, clone/restore and saved sets remain open. Squad control stays in the router. | Concurrent companion isolation, saved-set recovery and squad forum attachment. |
| [#89](https://github.com/hyperme-sh/hypermesh-visor/issues/89) Compact UI | Persistent compact mode, an anchored hover card and a session-bound Linux CLI are merged. Measured usage, forum/editor destinations, native launchers and nest support remain open. | Native UI interactions and correct per-companion launch targets. |
| [#90](https://github.com/hyperme-sh/hypermesh-visor/issues/90) Bundled Conductor | Linux packaging has a verified source bundle and managed sidecar. Default routing, full native packaging and leased-host coding scope remain open. | Installed sidecar startup/exit, limits, cancellation, selected recipe/model and measured host use. |
| [#99](https://github.com/hyperme-sh/hypermesh-visor/issues/99) Headless hosts | Headless build and explicit private credential storage are implemented. Package work is separate from host delivery, opt-in capability schema and Conductor/AIuditor authorization. | Installed headless sessions, on-request download verification/removal, network refusal and AIuditor denial. |
| [#100](https://github.com/hyperme-sh/hypermesh-visor/issues/100) Mobile | Native app/core binding spike, enrolled device auth, VPN, share actions, sensor permissions and local model routing remain open. | Native Android/iOS simulator/device adapters; no claim of desktop-style iOS automation. |
| [#101](https://github.com/hyperme-sh/hypermesh-visor/issues/101) Alpine | Complete implementation: native x86_64/ARM64 musl builds, APK/OpenRC and static bundles, Conductor, runtime image, install guide and verified release-asset wiring. Actual version publication still requires owner authorization. | Installed APK/native architecture, private sockets, non-display session and OpenRC remain scheduled regression coverage, not an issue-closing gate. |

Crash reporting [#115](https://github.com/hyperme-sh/hypermesh-visor/issues/115)
is implemented as an opt-in, bounded Application Insights reporter with fixed
error categories and request-scoped upstream attribution. See
[configuration and privacy](crash-reporting.md); unit and mocked-collector
contracts cover the reporting boundary.

Infra owns all fixture setup, readiness and teardown. The registered Linux lanes
are listed in [nightly CI](nightly-ci.md); missing display prerequisites fail
setup. Dedicated native desktop and ARM64 nightly runners must be provisioned
through Infra before these gaps can be exercised. Do not substitute a developer's
interactive desktop or manually dispatch a full suite.
