//! One SSH ControlMaster per host, shared by every command sent there, plus
//! its tunnels: `-R` so sandboxes on the host reach this daemon's agent
//! listener, `-L` for ports published on the host.
//!
//! Empty for now: phase 2's SSH transport thread fills it.
