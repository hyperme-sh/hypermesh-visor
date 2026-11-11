# Nightly CI

Full integration runs belong to the central [Infra nightly batch](https://github.com/FyberLabs/infra/blob/main/.github/workflows/nightly-integration.yml), scheduled daily at 06:23 UTC against main. Its serial matrix uses one cancellation group and reports regressions through Infra issues.

| Lane | Scope | Runner requirements |
| --- | --- | --- |
| `visor-container` | Two private bots, file grants, MCP, restart, real apps and fixture session verbs | Docker and Compose on Linux x64 |
| `visor-x11` | Existing X11 display scenarios | Xorg dummy driver, uinput, display tools, Python display modules and noninteractive harness sudo |
| `visor-wayland` | Existing Wayland scenarios and refusal behavior | Sway/Weston, XWayland and capture tools |
| `visor-user-isolation` | Socket permissions and authenticated loopback denial for another OS user | Existing `nobody` account and noninteractive sudo |

Infra owns setup, readiness and teardown. Missing runner requirements fail setup;
registration alone does not establish coverage. Native Wayland portal input,
Windows/macOS, ARM64 containers and installed-package nightly tests still need
platform adapters. See Infra's [Visor coverage](https://github.com/FyberLabs/infra/blob/main/docs/testing/nightly/visor.md).

PR checks run unit/contract tests, syntax/configuration checks, builds and package
creation. They do not launch full display or container suites. Installed-package
release checks retain their version-tag trigger. No workflow is dispatched by an
agent to substitute for the nightly schedule. Results do not add a human approval
prerequisite or change version/tag authority.
