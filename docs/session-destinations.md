# Session destinations

The Linux companion menu offers **Session CLI**, **Session forum**, and
**Session in VS Code** while a session is open. These actions are also available
from the menu on the compact bug icon. The CLI sends prompts to the existing
session. The other destinations are explicitly configured by the local user in
`companion.toml` inside the Hypermesh config directory:

```toml
[sessions."00000000-0000-4000-8000-000000000001"]
forum_url = "https://your-forum.example/t/your-thread"
workspace = "/home/you/projects/your-workspace"
```

Use the UUID of the session shown by Visor. Missing configuration produces a
setup message; an action never falls back to another session. Forum URLs must
use HTTPS without embedded credentials, query strings or fragments. Opening a
forum uses the desktop browser's own login; Visor does not append credentials.
The workspace must be an existing absolute directory, and VS Code's `code`
command must be available. Both actions pass one literal argument without a
shell or command template. Session purpose, task payload and model output do
not configure these destinations.

These actions open an existing forum thread or workspace. They do not enroll a
forum bot or deliver tasks; that transport remains tracked in #31. Native
PowerShell/WSL launch and the shared nest destination remain in #18/#25/#88.
