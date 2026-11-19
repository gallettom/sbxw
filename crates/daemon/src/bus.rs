//! One event bus for everything clients may hear about: each change is one
//! `Event`, published once, and each SSE endpoint filters the same stream. A
//! new kind of event is a new variant, nothing else.

use sbxw_core::agents::SessionInfo;
use sbxw_core::codemap::Run;
use sbxw_core::plans::Plan;
use sbxw_core::progress::Progress;
use sbxw_core::relay::RelayRequest;
use sbxw_core::SandboxRef;
use serde::Serialize;
use tokio::sync::{broadcast, watch};

/// A bring-up's progress, with the sandbox it is about, flattened on the wire.
#[derive(Clone, Serialize, Debug)]
pub struct ProvisionEvent {
    pub sandbox: SandboxRef,
    #[serde(flatten)]
    pub progress: Progress,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// A hook moved an agent session.
    Session(SessionInfo),
    /// The island asks open tabs to show a sandbox.
    Focus(SandboxRef),
    /// A tab is now reading a sandbox's terminal (for the island).
    Watching(SandboxRef),
    Relay(RelayRequest),
    Provision(ProvisionEvent),
    Codemap(Run),
    Plan(Plan),
    Lead(SandboxRef),
    LeadEnd(SandboxRef),
    /// A sandbox's Team-mode board changed.
    Board(SandboxRef),
    /// A team's timeline grew (its plan's id): re-read
    /// `/api/teams/:id/timeline`. Sent only while a UI listens.
    Team(String),
    /// The virtual Mac's lease or state changed: re-read `/api/mac`.
    Mac,
    /// The voice models or their download moved: `/api/voice`'s answer.
    Voice(serde_json::Value),
    /// A sandbox's agent started or stopped using its browser:
    /// `{ "sandbox", "active" }`.
    Browser(crate::svc::browser::BrowserState),
    /// A Jarvis proposal was made or settled: re-read `/api/jarvis`.
    Jarvis(sbxw_core::jarvis::Proposal),
    /// CPU and memory, every few seconds while a `/api/stream` is open:
    /// `GET /api/stats`'s answer.
    Stats(sbxw_core::stats::Stats),
    /// The status pills' vocabulary or a switch changed: `GET
    /// /api/emphasis`'s answer.
    Emphasis(serde_json::Value),
    /// A session's history grew: re-read `GET
    /// /api/sessions/:sandbox/:session_id/history`. A nudge, no text; sent
    /// only while a UI reads that session (see `svc::history`).
    History(HistoryNudge),
    /// A canvas changed (shots added, an edit, a review, deleted): re-read
    /// `GET /api/canvas/:id` (see `svc::canvas`).
    Canvas(crate::svc::canvas::CanvasNudge),
}

/// What the `history` event says: whose log, and its newest seq.
#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct HistoryNudge {
    pub sandbox: String,
    pub session_id: String,
    pub seq: u64,
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
            Event::Team(_) => "team",
            Event::Mac => "mac",
            Event::Voice(_) => "voice",
            Event::Browser(_) => "browser",
            Event::Jarvis(_) => "jarvis",
            Event::Stats(_) => "stats",
            Event::Emphasis(_) => "emphasis",
            Event::History(_) => "history",
            Event::Canvas(_) => "canvas",
        }
    }

    /// The SSE `data:` payload: JSON for structures, a ref's text form
    /// (`myapp`, `buildbox~myapp`) for sandboxes.
    pub fn data(&self) -> String {
        let json = |v: &dyn erased::Ser| v.to_json();
        match self {
            Event::Session(v) => json(v),
            Event::Relay(v) => json(v),
            Event::Provision(v) => json(v),
            Event::Codemap(v) => json(v),
            Event::Plan(v) => json(v),
            Event::Focus(s) | Event::Watching(s) | Event::Lead(s) | Event::LeadEnd(s) | Event::Board(s) => s.to_string(),
            Event::Team(id) => serde_json::json!({ "plan": id }).to_string(),
            Event::Mac => String::new(),
            Event::Voice(v) | Event::Emphasis(v) => v.to_string(),
            Event::Browser(v) => json(v),
            Event::Jarvis(v) => v.ui_view().to_string(),
            Event::Stats(v) => json(v),
            Event::History(v) => json(v),
            Event::Canvas(v) => json(v),
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
        // Deep: bring-up detail lines arrive several times a second; a slow tab
        // should skip a progress frame, not a step.
        let (tx, _) = broadcast::channel(1024);
        let (clients, _) = watch::channel(0);
        Self { tx, clients }
    }

    /// Publish. Nobody listening is fine: state lives in the services, and a
    /// tab that connects later seeds itself from a snapshot.
    pub fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Subscribers right now: what `/api/focus` answers so the island knows
    /// whether a tab is open.
    pub fn listeners(&self) -> usize {
        self.tx.receiver_count()
    }
}
