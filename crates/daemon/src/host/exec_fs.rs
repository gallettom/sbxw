//! [`HostFs`](super::HostFs) over any [`HostExec`](sbxw_sbx::host::HostExec):
//! each method is one POSIX `sh` script run on the host (no bash, no GNU-only
//! flags), paths passed as arguments rather than spliced into the script, and
//! file contents on stdin.
//!
//! Empty for now: phase 2's git/sandbox_fs-over-a-host thread fills it.
