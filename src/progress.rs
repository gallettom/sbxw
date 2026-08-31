//! What a sandbox bring-up is doing, while it is doing it.
//!
//! `provision_sandbox` is one call from the outside and eight steps from the
//! inside, and one of them — `sbx create` on a host that has never pulled the
//! image — can hold the other seven up for minutes. From a terminal that is
//! visible: sbx prints as it goes and you can see the layers arrive. From the
//! web UI it was a single "Creating…" row that eventually turned into a
//! sandbox, with no way to tell a slow download from a wedged one.
//!
//! So the bring-up says what it is doing and whoever started it decides who
//! hears it. The reporting side (`plan`, `step`, `detail`) names steps and
//! never learns who is listening; the listening side installs a sink for the
//! duration of the call. The web UI installs one that broadcasts to the browser
//! (see `/api/stream`'s `provision` event); the CLI installs nothing, every
//! call here is then a no-op, and `sbx` keeps the terminal it already had.
//!
//! The sink is **per thread**, because that is the shape of the thing being
//! reported: one bring-up runs start to finish on one thread (the web UI's
//! `spawn_blocking` task), and two of them running at once are two threads with
//! a sandbox name each. A global would have to be keyed by that name, and every
//! reporting call would then have to carry it — including the ones several
//! layers down in `sbx.rs`, which know they are running a command and nothing
//! else about it.

use serde::Serialize;
use std::cell::RefCell;
use std::sync::Arc;

/// Ids of the steps a bring-up takes. Shared with the browser as-is (the UI
/// keys its rows on them), so they are a wire contract and not just labels.
pub const WORKSPACE: &str = "workspace";
pub const CREATE: &str = "create";
pub const BOOT: &str = "boot";
pub const TOOLING: &str = "tooling";
pub const POLICY: &str = "policy";
pub const KITS: &str = "kits";
pub const PORTS: &str = "ports";

/// Longest detail line forwarded to a listener. These are single lines of `sbx`
/// output shown under a step in a sidebar column a few hundred pixels wide;
/// anything past this is a wrapped paragraph nobody reads, and on a chatty pull
/// it is also a lot of JSON on a socket.
const MAX_DETAIL: usize = 160;

/// One step of a bring-up, as announced up front by `plan`.
#[derive(Clone, Serialize)]
pub struct Step {
    /// Stable id — one of the constants above.
    pub id: &'static str,
    /// What the UI shows for it.
    pub label: String,
}

impl Step {
    pub fn new(id: &'static str, label: impl Into<String>) -> Self {
        Step {
            id,
            label: label.into(),
        }
    }
}

/// A single report from a bring-up in flight.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Progress {
    /// Every step this bring-up intends to take, in order, sent before the
    /// first one starts. Announced whole rather than grown a row at a time so
    /// that a step taking minutes reads as *one of seven, five still to come*
    /// — a list that has stopped moving, but visibly not stopped early.
    Plan { steps: Vec<Step> },
    /// This step is now running, which also means every step before it is done.
    /// Ending a step explicitly would be a second event per step saying what
    /// the next one already implies, and would leave the last one needing a
    /// third (`Done` closes it).
    Started { id: &'static str },
    /// A line `sbx` printed under the step currently running — image pull
    /// progress, mostly. Latest wins: this is a status line, not a log.
    Detail { text: String },
    /// The bring-up ended. `error` is absent when it worked.
    Done { error: Option<String> },
}

/// A listener. `Send + Sync` because the reader draining a child's stderr in
/// `sbx::run_inherit` runs on a thread of its own and reports from there.
pub type Sink = Arc<dyn Fn(Progress) + Send + Sync>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Restores whatever sink was installed before, on the way out of `with_sink`
/// — including out of a panic. Blocking threads are pooled and reused, so a
/// sink left behind by a panicking bring-up would report the *next* one's steps
/// to the browser tab that started this one.
struct Installed(Option<Sink>);

impl Drop for Installed {
    fn drop(&mut self) {
        SINK.with(|s| *s.borrow_mut() = self.0.take());
    }
}

/// Run `f` with `sink` receiving everything it reports.
pub fn with_sink<T>(sink: Sink, f: impl FnOnce() -> T) -> T {
    let _restore = Installed(SINK.with(|s| s.borrow_mut().replace(sink)));
    f()
}

/// The sink for this thread, if one is listening. Cloned out rather than
/// borrowed: a reporter can then outlive the borrow, which is what lets the
/// output readers in `sbx.rs` carry one onto a thread of their own.
pub fn sink() -> Option<Sink> {
    SINK.with(|s| s.borrow().clone())
}

fn report(p: Progress) {
    if let Some(sink) = sink() {
        sink(p);
    }
}

/// Announce the whole list of steps, before the first one starts.
pub fn plan(steps: Vec<Step>) {
    report(Progress::Plan { steps });
}

/// Start a step (and finish the one before it).
pub fn step(id: &'static str) {
    report(Progress::Started { id });
}

/// Report a line under the running step. Empty lines are dropped — they are
/// spacing in a terminal and a blanked status line here.
///
/// Takes the sink rather than reaching for the thread's, because the only
/// caller is an output reader on a thread of its own (see `sbx::run_reporting`)
/// — details are what a running command prints, and draining two pipes at once
/// takes a second thread whatever else happens.
pub fn detail(sink: &Sink, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let text = match text.char_indices().nth(MAX_DETAIL) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    };
    sink(Progress::Detail { text });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Collect everything reported inside `f`, in order, as `kind` strings.
    fn kinds_of(f: impl FnOnce()) -> Vec<String> {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        with_sink(
            Arc::new(move |p: Progress| {
                let value = serde_json::to_value(&p).unwrap();
                sink_seen
                    .lock()
                    .unwrap()
                    .push(value["kind"].as_str().unwrap().to_string());
            }),
            f,
        );
        let out = seen.lock().unwrap().clone();
        out
    }

    #[test]
    fn reports_nothing_without_a_sink() {
        // The CLI path: every call is a no-op rather than an error.
        assert!(sink().is_none());
        plan(vec![Step::new(CREATE, "Creating the sandbox")]);
        step(CREATE);
    }

    #[test]
    fn a_sink_sees_the_calls_in_order() {
        let kinds = kinds_of(|| {
            plan(vec![Step::new(CREATE, "Creating the sandbox")]);
            step(CREATE);
            detail(&sink().unwrap(), "pulling image");
        });
        assert_eq!(kinds, ["plan", "started", "detail"]);
    }

    #[test]
    fn blank_details_are_dropped() {
        assert!(kinds_of(|| detail(&sink().unwrap(), "   \n")).is_empty());
    }

    #[test]
    fn long_details_are_clipped() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        with_sink(
            Arc::new(move |p: Progress| {
                if let Progress::Detail { text } = p {
                    sink_seen.lock().unwrap().push(text);
                }
            }),
            || detail(&sink().unwrap(), &"x".repeat(MAX_DETAIL * 2)),
        );
        let line = seen.lock().unwrap()[0].clone();
        assert_eq!(line.chars().count(), MAX_DETAIL + 1); // clipped, plus the ellipsis
    }

    #[test]
    fn the_sink_does_not_outlive_the_call() {
        with_sink(Arc::new(|_| {}), || assert!(sink().is_some()));
        assert!(sink().is_none());
    }

    #[test]
    fn a_panicking_bring_up_leaves_no_sink_behind() {
        // Blocking threads are pooled: a sink left behind would report the next
        // sandbox's steps to the tab that created this one.
        let _ = std::panic::catch_unwind(|| {
            with_sink(Arc::new(|_| {}), || panic!("bring-up failed"));
        });
        assert!(sink().is_none());
    }
}
