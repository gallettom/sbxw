//! The SSH [`HostExec`](crate::host::HostExec): runs a [`HostCmd`](crate::host::HostCmd)
//! on a remote host through `ssh` (the argv from `RemoteHost::ssh_argv()`),
//! quoting each argument for the remote shell, sending secrets on stdin, and
//! reporting a failed connection (ssh's exit 255) as a `HostUnreachable`.
//!
//! Empty for now: phase 2's SSH transport thread fills it.
