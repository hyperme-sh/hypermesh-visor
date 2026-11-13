# Authenticated local transport

The inverse MCP client uses this transport before sending local HTTP requests.
Unix requires a same-user private directory and socket, refuses socket symlinks,
and checks the kernel's cached peer UID. The listener authenticates accepted peers
before handing streams to a protocol handler.

Windows endpoints are local named pipes scoped to the process user's SID and an
explicit instance/channel. Servers use a protected owner/System DACL, refuse
remote clients and reserve the first pipe instance. Clients verify the opened
object's owner and ACL, and use identification-only security quality of service.
The name is routing information, not authentication. A busy pipe can be acquired
for up to two seconds before any request is sent; requests are never replayed.

The inverse client can use this route now. Windows daemon startup, native desktop
controls and package installation remain in #25/#18; the server-side transport is
covered by native unit fixtures, not claimed as a complete desktop runtime.
