//! The sbxw daemon: one process, two listeners, every service explicit.
//!
//! * `store` — everything kept on the host, one record per sandbox;
//! * `bus` — one event stream, filtered per SSE endpoint;
//! * `pty`, `sessions`, `typing` — terminals and typing into agents;
//! * `provision` — the bring-up pipeline; `bundled` and `cache` — the kits
//!   sbxw ships and the read-only download cache they share;
//! * `svc` — the services, as `impl App` blocks;
//! * `http` — the UI listener (browser, island, extension) and the agent
//!   listener (the only one sandboxes can reach, token-authenticated).

pub mod app;
pub mod bundled;
pub mod cache;
pub mod bus;
pub mod git;
pub mod http;
pub mod hostsys;
pub mod provision;
pub mod pty;
pub mod sandbox_fs;
pub mod sessions;
pub mod store;
pub mod svc;
pub mod typing;

#[cfg(test)]
pub(crate) mod testing;

pub use app::{now_ms, App, AppBuilder};
