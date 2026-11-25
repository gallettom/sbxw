//! How long each `sbx` call took, kept for the bring-up it belongs to.
//!
//! A caller that wants its calls counted runs its future inside
//! [`Recorder::scope`]; every call made from that task (a `join!` included,
//! a spawned task not) is noted there by subcommand and milliseconds. A call
//! outside any scope is logged at debug level only. Only the subcommand is
//! kept (`command_class`), never an argument: a name, a token, a value.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// One `sbx` call: what it was, when it started (milliseconds after the
/// recorder's start), how long it took, and whether it exited 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub class: String,
    pub at_ms: u64,
    pub ms: u64,
    pub ok: bool,
}

/// Collects the calls of one piece of work. Cheap to clone; clones share.
#[derive(Clone)]
pub struct Recorder {
    start: Instant,
    calls: Arc<Mutex<Vec<Call>>>,
}

tokio::task_local! {
    static CURRENT: Recorder;
}

impl Recorder {
    pub fn new(start: Instant) -> Self {
        Self { start, calls: Arc::default() }
    }

    /// Run `f` with every `sbx` call it makes noted here.
    pub async fn scope<F: Future>(&self, f: F) -> F::Output {
        CURRENT.scope(self.clone(), f).await
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// How many calls of `class` started at or after `since_ms`.
    pub fn count_since(&self, class: &str, since_ms: u64) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| c.class == class && c.at_ms >= since_ms).count()
    }
}

/// Note a finished call: in the current task's recorder, else in the debug log.
pub(crate) fn note(class: &str, started: Instant, ok: bool) {
    let ms = started.elapsed().as_millis() as u64;
    let kept = CURRENT.try_with(|r| {
        let at_ms = started.saturating_duration_since(r.start).as_millis() as u64;
        r.calls.lock().unwrap().push(Call { class: class.to_string(), at_ms, ms, ok });
    });
    if kept.is_err() {
        tracing::debug!(target: "sbx", "sbx {class} took {ms} ms");
    }
}
