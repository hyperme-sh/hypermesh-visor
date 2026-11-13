# Private local storage

MCP trust data and profile edits use this shared file interface. Unix uses owned
regular files, modes 0600/0700, no-follow opens, single-link checks, flock and
atomic rename with directory sync. Windows uses owned disk-file handles,
protected owner/System DACLs on creation, reparse-point and hard-link refusal,
nonblocking byte-range locks, and write-through replacement after flushing data.

Existing paths are checked rather than having their permissions changed. Private
Windows paths reject a missing DACL and unknown ACE forms; access may be granted
only to the owner, Local System or built-in Administrators. Profile source reads
can permit other readers while still requiring the current owner and a regular,
single-link file. Lock guards explicitly unlock even if a duplicated handle remains.

Linux and macOS use this interface in the existing MCP implementation. Native
Windows CI tests the storage implementation independently. Windows desktop IPC,
input/capture, app controls and installer work remain in Visor #25 and #18; this
crate alone does not make the daemon run on Windows. Signing remains deferred.
