# Required owner cleanup service

Raw capture is unavailable until the cleanup helper is installed as an OS
service, is running as the same account as Visor, and has completed its first
cleanup pass. Enabling consent does not install a service or change privileges.
The helper has no network client. Do not run it as root or LocalSystem and do
not grant access to other users' crash directories.

The packaged `hypermesh-crash-retention` executable accepts:

- `register`, run as the Visor account.
- `service`, invoked only by the OS service manager.

Registration creates the fixed OS-account profile directory
`.hypermesh-crash-retention/config` with private storage permissions. Place the
private crash configuration file there and point `HYPERMESH_CRASH_CONFIG` to its
absolute path. HOME/USERPROFILE overrides do not change the account location.
Registration does not accept an alternate directory: rewriting its account-owned
record cannot redirect the running service's cleanup scope or reset the quota.
Existing unsafe paths are refused. Never pass a DSN or secret on the command
line. Registration creates no consent grant.

Before moving an existing configuration, successfully disable capture and purge
its old raw runs using the old configuration. Keep its configuration contents
private during the move, then explicitly enable consent only after service
installation. Legacy directories remain readable for startup cleanup but cannot
start new raw capture.

Install the helper in a stable executable location before registering the OS
service. Keep the service executable and service definition administrator-owned.
Installations are local administrator operations; the application does not run
these commands automatically. Installation does not enable raw capture.

## Linux

As the Visor account, run the packaged helper's `register` command. As the local
administrator, install `hypermesh-crash-retention@.service` into
`/etc/systemd/system/`, reload systemd, and enable/start its instance for that
account's numeric UID. The supplied system service runs with `User=%i`, starts
at boot and survives logout; a user-session unit is not a substitute. Adjust the
absolute executable path if the desktop package is installed elsewhere. The
helper verifies that systemd identifies its process as that instance's MainPID.

## macOS

As the Visor account, register the directory. A local administrator installs a
LaunchDaemon plist in `/Library/LaunchDaemons/` with these properties:

- Label: `sh.hyperme.visor.crash-retention.UID`, with the numeric account UID.
- UserName: that account's login name, never root.
- ProgramArguments: the stable absolute helper path followed by `service`.
- RunAtLoad: true; KeepAlive: true; ThrottleInterval: 1; Umask: 63 (decimal 0077).

The plist is root-owned and not writable by ordinary users. Bootstrap it in the
`system` domain. Do not use a LaunchAgent, which ends at logout. The helper
checks that launchd reports its PID under the account-specific system label.
Store the helper outside an app bundle that may move during an update.

## Windows

As the Visor account, register the directory with the packaged `.exe`. A local
administrator creates an automatic-start, own-process Windows service whose
binary command is the quoted stable helper path followed by `service`. Use the
exact account-scoped name `HypermeshCrashRetention-ACCOUNT-SID`, substituting
that account's SID. In the
Services management console, configure Log On as the Visor account and enter
credentials there; never put passwords in a command, config file or issue.
Configure recovery to restart the service after failure with a one-second delay,
and enable recovery actions for stops with errors (the Windows
`SERVICE_CONFIG_FAILURE_ACTIONS_FLAG` setting).
Start it before enabling consent. The binary enters through the Service Control
Manager dispatcher and refuses interactive `service` execution. Its account must
have a loaded, available user profile. Protected files retain owner/System access.

## Operation and removal

OS service-manager identity, a fresh service heartbeat and a held OS lease are
all required. Admission verifies the manager's PID and its OS account; local
files alone cannot satisfy this prerequisite. Successful manager queries are
cached for at most one second while heartbeat and lease checks remain per-write. Losing either
refuses new raw writes and stops the handler. The helper coordinates deletion
with the same capture lock as startup and withdrawal. It expires each entire
run within seven days of its start, conservatively earlier than seven days from
any capture in that run. Sleep/restart checks run before capture or review;
malformed, missing or future start times cause stopped runs to be purged.

The single registered directory and one writer prevent independent per-run
budgets. Each handler reserves at most five reports and each report is bounded
at 32 MiB. Failed attempts consume reservations. Oversized or over-count stopped
storage is purged before the service publishes readiness. A cleanup failure
leaves capture unavailable. Expiry requests stop slightly before the deadline;
OS suspension or power loss can delay deletion until execution resumes.

Disable crash consent successfully before uninstalling the service or moving
its registration. Stop/remove the OS service only after raw cleanup succeeds.
Do not delete the registration merely to switch directories with old raw files
remaining. Forced service termination or a broken OS scheduler cannot promise
physical deletion while it is unable to execute; capture still fails closed.

Raw directories and the private registration directory are not application
backup or export inputs. Never add them to diagnostic bundles, sync roots or
application archives. Visor has no raw export command. An OS backup outside
Visor's control must be excluded separately by its owner. Ordinary deletion is
not a secure-erasure guarantee for SSDs, snapshots or backups.
