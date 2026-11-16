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

## Manager core

`companion_manager::Manager` is the local allocation and revocation core. It
adopts one already-paused native session, counts it toward the four-slot cap,
and allocates fresh local companion, session and generation IDs for additional
container desktops. These IDs are routing scopes, not enrolled bot principals
or SociACL grants. A trusted controller must still verify assignment before
issuing work. Each allocation charges an explicit memory/CPU budget; both failed
starts and incomplete stops retain their allocation until cleanup succeeds.

Starting creates a paused desktop. A trusted local confirmation consumes a
`ResumeOffer` tied to that exact pause revision. Physical input and observer loss
invalidate outstanding offers, including offers for an already-paused desktop.
Resuming creates a new in-memory permit rather than reviving an old one. Every
backend action and cancellation checkpoint must check the permit against its
exact scope. Selecting a different view cannot change a permit's target.

The trusted host observer has a one-second lease. A permit refuses an expired
lease even before the controller's watchdog tick. A late poll revokes the old
routes before renewing the lease; it never resumes them automatically. Physical
input revokes all running routes before any backend pause/cancel call, and one
backend failure does not suppress cancellation of the other companions. Stop
revokes its route, asks the backend to cancel active work, then stops its desktop.
Both acknowledgements must succeed before the slot becomes reusable. A failed
resume or cleanup leaves the allocation quarantined, available only for stop.

## Linux runtime and native controls

The native X11 menu's **+ Companions** control opens the manager. It lists the
native desktop and up to three private Linux desktops, with explicit add,
resume, stop, home-retention and view-only snapshot operations. Native session
selection continues to use the existing native controls. Additional desktops
require an already installed local rootful Docker engine and the installed
`ghcr.io/hyperme-sh/hypermesh-visor:test` image. The manager never installs a
backend, pulls an image or changes permissions to obtain Docker access.
Windows, macOS, Wayland, remote Docker and rootless Docker are refused by this
first runtime adapter.

Every guest receives 2 GiB RAM, one CPU, 512 process slots, bounded temporary
storage, a private named home volume and a read-only control mount. Network
access is disabled. The fixed specification has no host display, input device,
Docker socket or shared home. Resource flags follow the official
[Docker create reference](https://docs.docker.com/reference/cli/docker/container/create/)
and [resource constraints](https://docs.docker.com/engine/containers/resource_constraints/).
Homes consume Docker storage and are retained only by explicit choice; retained
homes appear separately for deletion and are never mounted for another bot.

The owner-only `~/.hypermesh-companions` directory holds an exclusive manager
lock and an atomically replaced allocation journal. A fixed Docker claim also
prevents a second owner/configuration from claiming the supported engine.
Allocations are committed before Docker effects. Startup attempts cleanup of
previous allocations, preserving their homes. Failed or uncertain cleanup
refuses new admission. Cleanup checks the owner and complete scope labels and
removes containers by their full inspected ID. It never prunes unrelated Docker
resources. Deleting the journal is not a supported reset or recovery operation.

Only the peer-checked native companion socket exposes manager operations. TCP
callers and managed guests cannot create or control the host manager. Physical
input invalidates permits through an observer handle independent of Docker
operations. Polls are considered healthy only while the X11 event loop continues
to report input availability; a background HTTP poll alone cannot keep the
observer alive. Lease publication and Docker cleanup run on separate workers.

The guest starts with one fixed, empty local session and no copied MCP servers,
secrets, remote assignment or authority. Its daemon checks the short host lease
at admission and input cancellation points. Pausing cancels queued local tasks;
a new run invalidates prior prompt cancellation tokens. Its supervisor exits
when host control expires. A local guest resume cannot override a paused host.
The host verifies the guest protocol and exact scope before exposing a newly
started desktop. Normal remote assignment/authentication rules still apply;
these local routing IDs are not enrolled bot identities or SociACL grants.

Unit contracts use an in-memory Docker command runner and temporary local
journals, not a running Docker engine. Native display scenarios are owned by
the central scheduled batch. Clone, resurrect, preprompt sets and squad forum
embedding retain their separate issue ownership.
