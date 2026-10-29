//! sbxw's domain rules.
//!
//! Everything here is decided from data: names, the config, the three
//! human-in-the-loop state machines (relay, plans, code map runs), the hook
//! state of agent sessions, every text typed into an agent, and the parsers
//! for what `sbx` prints. No processes, no sockets — the daemon and the CLI
//! bring the world; this crate says what to do with it.

pub mod agents;
pub mod board;
pub mod artifacts;
pub mod codemap;
pub mod config;
pub mod embed;
pub mod envexport;
pub mod envfile;
pub mod favourites;
pub mod hosts;
pub mod ids;
pub mod kits;
pub mod mcp;
pub mod messages;
pub mod my_skills;
pub mod names;
pub mod plans;
pub mod policy;
pub mod power;
pub mod ports;
pub mod progress;
pub mod relay;
pub mod remote;
pub mod session;
pub mod sysbin;
pub mod table;
pub mod templates;
pub mod terminal;
pub mod tooling;
pub mod usage;

pub use names::{ModelId, SandboxName, ThreadName};
