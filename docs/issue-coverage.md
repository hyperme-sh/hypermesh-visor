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
| [#18](https://github.com/hyperme-sh/hypermesh-visor/issues/18) Native companions | Complete native Windows/macOS companions: in-process desktop callbacks, visible Allow/Pause/Stop, physical-input pause and bounded idle resume. Windows landed in #186. Isolated native MCP stdio workers remain in #29. | Separate native Windows/macOS desktop adapters with physical-input pause and recovery. |
| [#25](https://github.com/hyperme-sh/hypermesh-visor/issues/25) Windows distribution | Complete Windows x64 development distribution: private current-user IPC, bundled CLI, owned Conductor/app launcher, unsigned per-user installer and native desktop/package CI contracts (#187, #190–#193). ARM64 is unclaimed; signing stays in #84. | Native installed Windows session and upgrade/removal checks. |
| [#26](https://github.com/hyperme-sh/hypermesh-visor/issues/26) macOS distribution | Complete Apple Silicon development distribution: native app, CLI, inverse MCP and Conductor bundle, private IPC, capture/input, permission recovery and keychain/package contracts. #182 adds automatic bundled startup; #199 preserves directory selectors. This does not claim login-item registration or Intel support. Signing stays in #84. | Apple Silicon permission refusal/recovery, installed application, interruption and keychain checks. |
| [#27](https://github.com/hyperme-sh/hypermesh-visor/issues/27) Container bots | Complete local container deployment: Debian desktop image, bundled CLI, private state, scoped shared resources, native amd64/arm64 archives and automatic verified GHCR TEST publication. Cross-service registration and assignment are implemented under #30. | `visor-container` repeats two-bot isolation, resource, restart, MCP and app checks on x64; add native ARM64. |
| [#28](https://github.com/hyperme-sh/hypermesh-visor/issues/28) Verb contract | Complete audit: HTTP/MCP inventory, limits, errors, coordinates, cancellation and task classification are documented. Missing native/service implementations stay in their own issues. | Browser/editor/terminal and display regression coverage stays in the scheduled batch; unsupported operations are explicitly classified in the verb contract. |
| [#29](https://github.com/hyperme-sh/hypermesh-visor/issues/29) MCP discovery | Discovery, native trust screens (#198/#200/#201), stable app identities, HTTP transport, schema checks and explicit catalog operations are implemented. #202 blocks automation during pending source reviews. Isolated Windows/macOS stdio workers remain unavailable; native connection-control UI is tracked in #203/#204. | Native platform application matching, permission/refusal, refresh and disconnect cases. |
| [#30](https://github.com/hyperme-sh/hypermesh-visor/issues/30) Bot assignment | Plane #110–#112 implement owner-scoped enrollment, durable assignment, heartbeat, task deduplication, cancellation and revocation. Opt-in Visor outbound polling binds one local session and enforces local consent and short deadlines. Portal #114 adds owner enrollment and controls; Edge #59 routes scoped agent requests. Dedicated containers opt into one local startup consent with automatic restart disabled. | Infra-owned assigned-machine/container fixtures with denied owner, expired assignment and duplicate dispatch. |
| [#31](https://github.com/hyperme-sh/hypermesh-visor/issues/31) Forum tasks | s3r.ch #387/#388 provide signed delegated task admission, durable replay protection, task cards and cancellation drafts. Visor #150 supplies consent-bound absolute task leases. s3r.ch #389 adds the opt-in worker, individual Gun subscriptions, durable reports and independently authorized private artifacts. s3r.ch #391 renders approval timing without publishing private approval handles. s3r.ch #392 and acl-service #78/#79/#82 supply complete primary history, atomic admission, quotas and explicit startup bindings to provisioned stores. Complete; opt-in worker configuration and private local approval files remain required. | One user/two bots, cross-thread refusal, replay, reconnect and cancellation. |
| [#32](https://github.com/hyperme-sh/hypermesh-visor/issues/32) Inverse MCP | Session-bound bounded task submit/status/cancel, result resources, approved verbs, initialization and in-flight RPC cancellation are implemented. | MCP client subprocess contracts cover HTTP/Unix parity, denied requests, cancellation and disconnect cleanup. |
| [#34](https://github.com/hyperme-sh/hypermesh-visor/issues/34) Delivery tracker | This map records the remaining scope; it does not replace the implementation issues. | Link each platform/service adapter and its scheduled results. |
| [#35](https://github.com/hyperme-sh/hypermesh-visor/issues/35) Desktop base | Complete: Debian 13, private Xvfb/Openbox and pinned app packages are selected in `container-base.md` and implemented in `container/Dockerfile`. Headless Alpine is a separate deployment mode. | The central `visor-container` lane retains regression coverage; native ARM64 coverage remains scheduled regression work. |
| [#36](https://github.com/hyperme-sh/hypermesh-visor/issues/36) Downloads | Complete for published v0.1.0: architecture/format selectors, checksum-verified install commands, removal guidance, first-capture instructions and docs links are merged. Source/CI artifacts are not published native/headless downloads. | Versioned install-to-first-task and website navigation checks for each published platform. |
| [#37](https://github.com/hyperme-sh/hypermesh-visor/issues/37) Shared vault | Cross-bot encrypted sharing, operation-specific grants, freshness and signer boundaries require coordinated SociACL/s3r.ch work. The headless session file store is not a shared vault. | Scoped user/two-bot sharing, denial, revocation, rotation, replay, offline expiry and secret containment. |
| [#84](https://github.com/hyperme-sh/hypermesh-visor/issues/84) Signing | Deferred by owner. Do not publish signed/notarized claims or add approval gates. | Signing checks only after the required identities and specific release authority exist. |
| [#88](https://github.com/hyperme-sh/hypermesh-visor/issues/88) Companion nest | One-default/up-to-four local companions, isolated app sessions, clone/restore and saved sets remain open. Squad control stays in the router. | Concurrent companion isolation, saved-set recovery and squad forum attachment. |
| [#89](https://github.com/hyperme-sh/hypermesh-visor/issues/89) Compact UI | Complete compact controls and reported usage across Linux, macOS and Windows. Windows #194/#196 add session-bound forum, VS Code and PowerShell/WSL launchers; #197 adds tray state and comparative token bars. Multi-companion nest behavior belongs to #88. | Native UI interactions and correct per-companion launch targets. |
| [#90](https://github.com/hyperme-sh/hypermesh-visor/issues/90) Bundled Conductor | Complete bundled Conductor routing and owned startup: private sockets and cancellation (#179), task accounting (#181), macOS launcher (#182/#199), Windows job-owned launcher (#191), and the existing Linux managed sidecar. Explicit endpoint/journal/lease configuration remains required; implementation does not activate a provider. | Installed sidecar startup/exit, limits, cancellation, selected recipe/model and measured host use. |
| [#99](https://github.com/hyperme-sh/hypermesh-visor/issues/99) Headless hosts | Complete headless delivery and owner-enabled capability API (#178): authenticated local machine bridge, guarded Conductor dispatcher, AIuditor review, duplicate-call protection and controller checks. Defaults remain off. The reference controller reads the clock; actuator drivers must supply their own immutable limits. See `host-capability-api.md`. | Installed headless sessions, on-request download verification/removal, network refusal and AIuditor denial. |
| [#100](https://github.com/hyperme-sh/hypermesh-visor/issues/100) Mobile | Rust core and generated Swift/Kotlin bindings build for iOS/Android. Apple Keychain and Android Keystore adapters are implemented (#173/#175/#176). Native apps, enrolled device auth, VPN, share actions, sensor permissions and local model execution remain open. | Native Android/iOS simulator/device adapters; no claim of desktop-style iOS automation. |
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

## Remaining delivery order

The original platform backlog remains after #31. The forum worker and concrete
ACL source adapter are implemented; the client refuses execution when its independent
authority source is absent. Operators must configure the source and provision
its private stores. Closing the implementation issue does not activate a worker
or a provider. Artifact handles in
#31 provide private task-result access only, not the shared credential vault in
#37 or delegated access to s3r.ch media.

The original remaining issues are #17, #29, #34, #37, #84, #88 and #100.
Native Windows/macOS companions and distributions, compact UI and bundled
Conductor startup are implemented. #17 still needs native Wayland input,
physical-input pause and overlay support. The portal refusal lane does not
implement those features. #29 retains isolated native stdio workers; HTTP
support and source-review screens do not provide that worker boundary.

#88 adds independently isolated companions and saved sets; shared desktop
input must not be presented as four isolated sessions. Squad run-control stays
with the router. #100 is a separate native mobile app/network/sensor effort,
not an extension of desktop input permissions. #37 requires the coordinated
encrypted-object and operation-specific grant contract before sharing secrets;
its draft design (#153) still awaits the wallet-signer choice. #34 remains the
umbrella tracker. #84 stays deferred by owner decision.

#99's guarded host route is implemented and default-off. Enabling it requires
explicit Host bridge, owner policy, machine identity and controller configuration.
Closing the implementation issue does not start those services or grant access.

Client unit tests run on bugtest. J4012 is reserved for the authorized Visor/host
CI lanes; do not update its pinned packages. The outstanding X11 startup failure is
tracked in Infra #1088; #174 adds stage-specific diagnostics for the next
scheduled batch. Do not run a
manual batch to advance an implementation issue.

## Additional design requests

[#169: external MCP server](mcp-server-design.md) and
[#170: SLOPCANNON client](slop-client-design.md) define transport and authorization
boundaries only. Neither authorizes implementation or provider activation.
#171 adds a separate opt-in health-agent requirement; the existing #115 crash
reporter does not implement that broader scope. #188 (Agent Client Protocol)
and #189 (external coding dispatch) are newer requests outside the original
remaining-issue set.
