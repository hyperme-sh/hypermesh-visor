# Independent companion desktops

Decision for #220: implement one Linux container desktop per companion first.
Keep native desktop control limited to one companion per logged-in desktop.
The default remains one companion; #88 may offer up to four only after #224
supplies independent desktops and the controls described here.

## First mode by operating system

| Host | First concurrent mode | What the user sees | Limits |
| --- | --- | --- | --- |
| Linux | One container, X server, home and Visor session per companion | A separately named Linux desktop for each companion | Linux applications only; containers share the host kernel |
| Windows | The same Linux containers through an already provisioned Linux VM backend, or an assigned Linux machine | Linux desktops clearly labelled as such, alongside at most one native Windows desktop | Does not run Windows applications in the Linux guests; no automatic backend installation |
| macOS | The same Linux containers through an already provisioned Linux VM backend, or an assigned Linux machine | Linux desktops clearly labelled as such, alongside at most one native Mac desktop | Does not run Mac applications in the Linux guests; no automatic backend installation |

Docker Desktop runs Linux containers in a Linux VM; available VM managers vary
by platform. Visor must discover a supported, already configured backend and
refuse additional desktops when none is available. It must not change host
virtualization, packages or permissions to make the option work. See
[Docker's VM manager documentation](https://docs.docker.com/desktop/features/vmm/).

Count every companion managed by this machine toward its four-slot limit,
including the native desktop and any remotely assigned desktop. A remote
assignment must also pass its destination's capacity check. Four is a ceiling,
not a reservation: insufficient memory, CPU, storage or backend capacity must
produce a clear refusal. The UI must never silently reuse another companion's
desktop or browser profile.

## Alternatives and their boundaries

Browser contexts separate cookies and local/session storage, even within one
browser. They are useful for a future browser-only adapter. They do not give
Visor's generic screenshot, pointer and keyboard adapter an independent desktop
or isolate arbitrary installed applications. See
[Playwright's context model](https://playwright.dev/docs/browser-contexts).

Separate X servers on Linux can provide separate display and input targets.
Starting them under the same host account alone leaves files and processes
shared. The existing container base is a better starting point because it also
provides private homes and resource limits. A container still shares a kernel;
this mode is for companions in the operator's trust domain, not a promise of VM
isolation between hostile tenants. Docker documents the roles of namespaces,
cgroups and the privileged daemon in its
[security overview](https://docs.docker.com/engine/security/).

Win32 desktop objects are not a substitute for concurrent visible desktops:
only one desktop in the interactive window station receives user input at a
time. Switching desktops would move the shared input target. Keep native
Windows control single-session; do not use switching as a concurrency adapter.
See [Microsoft's desktop contract](https://learn.microsoft.com/en-us/windows/win32/winstation/desktops).

The Mac adapter controls the logged-in desktop. This design does not treat
Spaces or multiple windows as independent input sessions. Native Windows or Mac
applications requiring concurrent desktop control need separately provisioned
OS guests or separate machines, each with its own Visor session and permissions.
Guest provisioning, OS licensing, recovery and platform-specific input support
are not implemented by this decision and are not prerequisites for the Linux
container mode.

## Boundaries the implementation must preserve

Each companion needs a distinct identity, session, desktop target, browser
profile, private home, bounded temporary storage and control channel. A display
number may repeat inside separate containers; the routing identity must include
the owning container and session generation. An old request must not reach a
replacement container after restart.

Use the restrictions in `container/compose.yaml` as the starting point:
non-root execution, dropped capabilities, no new privileges, a read-only root,
bounded resources and private homes. Its two example services are not a dynamic
four-companion manager. Network access remains disabled until an explicit
policy supplies only the required destinations. Shared files require an
explicit per-companion grant, with read-only access where appropriate; never
mount another companion's home, a host display socket or a host device to
simulate independence.

A trusted manager outside the desktops owns container creation and destruction.
Bots must not receive the Docker socket or arbitrary container specifications.
Selecting a desktop in the UI changes the view, not the authority of an existing
request. Each viewer and input action must resolve the authorized companion,
session and generation before sending anything.

Stop first revokes new actions and cancels the active turn, then stops the
desktop and releases its allocation. If stopping fails, keep the slot occupied
and show the failure; do not reassign it while work may still be running.
Persisted homes need an explicit retention choice and must not be silently
reused for a different identity.

The container's virtual input cannot stand in for detecting physical activity
on the host. #224 and #88 must wire a trusted host pause signal and an always
available stop control before exposing concurrent control. Host physical input
pauses locally managed interactive companions; resuming follows the existing
explicit session authority. The UI must show which companion is paused and
which desktop it controls. A missing pause route must refuse interactive starts.

## Implementation owners and completion

- #224 builds the per-companion container manager, desktop/view routing,
  resource admission, pause/stop propagation and refusal paths.
- #88 adds the default-one, maximum-four UI and distinct identities, with
  assignment and state kept separate through start, pause, stop and restart.
- #221 handles cloning an identity into a new context; #222 handles saved
  preprompt sets. Neither may copy an active session's authority into a new one.
- #223 embeds the forum view without changing the desktop's control boundary.
- #17 retains the separate Wayland scope; this decision does not enable native
  Wayland input or claim platform parity.

Unit and contract coverage belongs in the implementation PRs: independent
profiles and routing, stale generations, refusal of a fifth allocation,
capacity failures, cancellation and pause/stop propagation. Display scenarios
belong only in the central nightly CI batch against main. A nightly failure
opens a new linked issue. This note closes the research slice when its PR merges
green; it does not claim that four companions can already control desktops.
