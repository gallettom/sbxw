//! One event bus for everything a client may want to hear about.
//!
//! The old daemon had nine broadcast channels and a hand-written multiplexer
//! listing them. Here every change is one `Event`, published once; each SSE
//! endpoint is a filter over the same stream, and adding a kind of event means
//! adding a variant — the multiplexer does not change.

use sbxw_core::agents::SessionInfo;
use sbxw_core::codemap::Run;
use sbxw_core::plans::Plan;
use sbxw_core::progress::Progress;
use sbxw_core::relay::RelayRequest;
use serde::Serialize;
use tokio::sync::{broadcast, watch};

/// A bring-up's progress, with the sandbox it is about, flattened on the wire.
#[derive(Clone, Serialize, Debug)]
pub struct ProvisionEvent {
    pub sandbox: String,
    #[serde(flatten)]
    pub progress: Progress,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// A hook moved an agent session.
    Session(SessionInfo),
    /// The island asks open tabs to show a sandbox.
    Focus(String),
    /// A tab is now reading a sandbox's terminal (for the island).
    Watching(String),
    Relay(RelayRequest),
    Provision(ProvisionEvent),
    Codemap(Run),
    Plan(Plan),
    Lead(String),
    LeadEnd(String),
    /// A sandbox's Team-mode board changed.
    Board(String),
}

impl Event {
    /// The SSE event name on `/api/stream`.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Session(_) => "session",
            Event::Focus(_) => "focus",
            Event::Watching(_) => "watching",
            Event::Relay(_) => "relay",
            Event::Provision(_) => "provision",
            Event::Codemap(_) => "codemap",
            Event::Plan(_) => "plan",
            Event::Lead(_) => "lead",
            Event::LeadEnd(_) => "lead-end",
            Event::Board(_) => "board",
        }
    }

    /// The SSE `data:` payload: JSON for structures, the bare name for names.
    pub fn data(&self) -> String {
        let json = |v: &dyn erased::Ser| v.to_json();
        match self {
            Event::Session(v) => json(v),
            Event::Relay(v) => json(v),
            Event::Provision(v) => json(v),
            Event::Codemap(v) => json(v),
            Event::Plan(v) => json(v),
            Event::Focus(s) | Event::Watching(s) | Event::Lead(s) | Event::LeadEnd(s) | Event::Board(s) => s.clone(),
        }
    }
}

mod erased {
    pub trait Ser {
        fn to_json(&self) -> String;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_json(&self) -> String {
            serde_json::to_string(self).unwrap_or_else(|_| "null".into())
        }
    }
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Event>,
    /// Open `/api/stream` connections: the UI warns when there is more than one tab.
    pub clients: watch::Sender<usize>,
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus {
    pub fn new() -> Self {
        // Deep: a bring-up's detail lines arrive several times a second, and a
        // slow tab should skip a frame of a progress line, not a step.
        let (tx, _) = broadcast::channel(1024);
        let (clients, _) = watch::channel(0);
        Self { tx, clients }
    }

    /// Publish. Nobody listening is fine: the state lives in the services,
    /// and a tab that connects later seeds itself from a snapshot.
    pub fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// How many are listening for a given kind right now — what `/api/focus`
    /// answers so the island knows whether a tab is open.
    pub fn listeners(&self) -> usize {
        self.tx.receiver_count()
    }
}
