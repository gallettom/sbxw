//! Whether a sandbox is writing its code map — or a lens on it — right now.
//!
//! Both are written by the sandbox's own agent — nothing here reads a
//! repository or produces a line of markdown. What this module owns is the one
//! fact the daemon needs while that happens: *this sandbox has a codemap run
//! in flight*, of this kind, started at this moment, and it ended like so.
//!
//! It exists because the run is invisible otherwise. Writing a map takes
//! minutes on a real repository, it happens in a background session with no
//! pane to watch, and the only evidence it produces is a directory appearing
//! under `.sbxw-artifacts/` at the very end. Between the click and that
//! directory there was nothing to show and nothing to stop a second click from
//! starting a second run over the first.
//!
//! Two parties end a run, which is why finishing is a transition and not a
//! write:
//!
//!  - **The agent**, through `POST /api/sandboxes/:name/codemap/done` — it
//!    knows the document is written the moment it has written it, which is
//!    earlier than its process knows anything.
//!  - **The background session's exit**, which is the backstop: an agent that
//!    crashed, was refused permissions, or simply never reported still has to
//!    stop the UI saying "writing…" forever.
//!
//! First one wins, and the loser is ignored — `finish` moves a run out of
//! `Writing` and nothing moves it back, so the process exiting a second after
//! the agent reported cannot overwrite what the agent said.
//!
//! Like `relay`, this is state and nothing else: `web.rs` owns the endpoints,
//! `sbx.rs` owns the command that starts the session.

use serde::Serialize;
use std::{collections::HashMap, sync::Mutex};
use tokio::sync::broadcast;

/// Where a run stands.
///
/// Serialized as-is to the browser (the UI keys its button and its badge on
/// these), so they are a wire contract rather than internal labels.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Phase {
    /// A background session is writing the map right now.
    Writing,
    /// It finished. The map is on disk — `/artifacts` is what says what is in
    /// it; this only says nobody is still working on it.
    Done,
    /// It ended without a map: the session failed to start, the agent was
    /// refused, or it was still going when the cap ran out.
    Failed,
}

/// What a run is writing.
///
/// A sandbox holds one run at a time whatever the kind (see `begin`), so this
/// is not a key — it is what lets every message about a run, from the refusal
/// to the toast, name the thing being written instead of calling a lens a map.
/// Serialized to the browser, which labels its badge and its corner card from
/// it, so these spellings are a wire contract.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// `/codemap` — the map itself, under `.sbxw-artifacts/codemap/`.
    Map,
    /// `/codemap-lens` — the map retold for one reader, under
    /// `.sbxw-artifacts/codemap-lenses/<slug>/`.
    Lens,
}

impl Kind {
    /// What this run is writing, as it reads in a sentence about a sandbox:
    /// "'neos' is already writing *its code map*".
    pub(crate) fn what(self) -> &'static str {
        match self {
            Kind::Map => "its code map",
            Kind::Lens => "a lens",
        }
    }
}

/// Longest note kept with a run. Notes are the tail of a failed session's
/// output, shown in a tooltip and a toast — a paragraph, not a log.
const MAX_NOTE: usize = 300;

/// One run, past or present. One per sandbox — see `begin` for why a lens does
/// not get a slot of its own.
#[derive(Clone, Serialize, Debug)]
pub(crate) struct Run {
    pub(crate) sandbox: String,
    /// Map or lens. What the run is writing, not where it is written: the
    /// directory is `web.rs`'s business.
    pub(crate) kind: Kind,
    pub(crate) phase: Phase,
    /// Unix epoch ms the run started.
    pub(crate) started: u64,
    /// Unix epoch ms it ended; absent while it is still writing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ended: Option<u64>,
    /// What ended it, when that is worth saying: the tail of a failed session's
    /// output, or whatever the agent reported alongside its own "done".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// Clip a note to something a tooltip can hold, on a character boundary.
fn clip(note: &str) -> Option<String> {
    let note = note.trim();
    if note.is_empty() {
        return None;
    }
    Some(match note.char_indices().nth(MAX_NOTE) {
        Some((i, _)) => format!("{}…", &note[..i]),
        None => note.to_string(),
    })
}

/// Why a second run is refused — worded once, because two callers say it: the
/// transition below, and the endpoints that check early to save themselves two
/// `sbx` calls they would only throw away.
///
/// `kind` is the kind of the run *in flight*, not of the one being refused: the
/// useful half of "no" is what the sandbox is busy with, and a lens turned away
/// because the map is being rewritten under it has to say so.
pub(crate) fn already_writing(sandbox: &str, kind: Kind) -> String {
    format!("'{sandbox}' is already writing {}", kind.what())
}

/// Every sandbox's latest run, and a bus that reports each change.
///
/// The last *finished* run is kept rather than dropped, so a failure survives
/// long enough to be read: the browser tab that clicked may not be the one open
/// when the session dies, and "it silently went back to offering the button" is
/// not an explanation.
pub(crate) struct Runs {
    runs: Mutex<HashMap<String, Run>>,
    tx: broadcast::Sender<Run>,
}

impl Runs {
    pub(crate) fn new() -> Self {
        Runs {
            runs: Mutex::new(HashMap::new()),
            tx: broadcast::channel(64).0,
        }
    }

    /// Subscribe to every phase change (see `web::api_stream`'s `codemap`
    /// event).
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Run> {
        self.tx.subscribe()
    }

    fn publish(&self, run: &Run) {
        // Errors only when nobody is subscribed, which is the ordinary state of
        // a daemon with no tab open.
        let _ = self.tx.send(run.clone());
    }

    /// Mark `sandbox` as writing, unless it already is.
    ///
    /// Rejecting the second start is the whole point of holding this state: two
    /// agents writing the same directory at once do not produce a map twice as
    /// fast, they produce one map with half of each in it.
    ///
    /// One slot per sandbox, whatever the kind, so a lens is refused while a
    /// map is being written and the other way round. They are not the same
    /// directory, but a lens is written *from* the map — a lens started over a
    /// map still being rewritten reads half of the old account and half of the
    /// new one, and says so with a straight face.
    pub(crate) fn begin(&self, sandbox: &str, kind: Kind, now: u64) -> Result<Run, String> {
        let mut runs = self.runs.lock().unwrap();
        if let Some(run) = runs.get(sandbox) {
            if run.phase == Phase::Writing {
                return Err(already_writing(sandbox, run.kind));
            }
        }
        let run = Run {
            sandbox: sandbox.to_string(),
            kind,
            phase: Phase::Writing,
            started: now,
            ended: None,
            note: None,
        };
        runs.insert(sandbox.to_string(), run.clone());
        drop(runs);
        self.publish(&run);
        Ok(run)
    }

    /// End the run in flight for `sandbox`. Returns `None` when there is
    /// nothing to end — a stray report, or the second of the two parties to
    /// arrive — and publishes nothing in that case.
    pub(crate) fn finish(
        &self,
        sandbox: &str,
        ok: bool,
        note: Option<&str>,
        now: u64,
    ) -> Option<Run> {
        let mut runs = self.runs.lock().unwrap();
        let run = runs.get_mut(sandbox)?;
        if run.phase != Phase::Writing {
            return None;
        }
        run.phase = if ok { Phase::Done } else { Phase::Failed };
        run.ended = Some(now);
        run.note = note.and_then(clip);
        let run = run.clone();
        drop(runs);
        self.publish(&run);
        Some(run)
    }

    /// This sandbox's latest run, finished or not.
    pub(crate) fn get(&self, sandbox: &str) -> Option<Run> {
        self.runs.lock().unwrap().get(sandbox).cloned()
    }

    /// What `sandbox` is writing right now, if anything. The kind comes back
    /// with the answer because every caller that asks goes on to say no, and a
    /// refusal has to name what it is waiting for.
    pub(crate) fn writing(&self, sandbox: &str) -> Option<Kind> {
        self.get(sandbox)
            .filter(|r| r.phase == Phase::Writing)
            .map(|r| r.kind)
    }

    /// Every run this daemon knows about, newest first — what a tab that
    /// reloaded mid-run reads to rebuild what it was showing.
    pub(crate) fn all(&self) -> Vec<Run> {
        let mut runs: Vec<Run> = self.runs.lock().unwrap().values().cloned().collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.started));
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_start_is_refused_while_the_first_runs() {
        let runs = Runs::new();
        assert!(runs.begin("neos", Kind::Map, 1).is_ok());
        assert!(runs.begin("neos", Kind::Map, 2).is_err());
        // Another sandbox is another map, and is never in the way.
        assert!(runs.begin("other", Kind::Map, 3).is_ok());
    }

    /// One slot per sandbox: a lens and a map are different documents, but the
    /// lens is written from the map, so neither waits for a second slot — and
    /// the refusal names the kind that is *running*, not the one turned away.
    #[test]
    fn a_lens_and_a_map_share_the_one_slot() {
        let runs = Runs::new();
        runs.begin("neos", Kind::Map, 1).unwrap();
        let refused = runs.begin("neos", Kind::Lens, 2).unwrap_err();
        assert!(refused.contains("its code map"), "{refused}");

        let runs = Runs::new();
        runs.begin("neos", Kind::Lens, 1).unwrap();
        let refused = runs.begin("neos", Kind::Map, 2).unwrap_err();
        assert!(refused.contains("a lens"), "{refused}");
    }

    #[test]
    fn a_finished_run_makes_way_for_the_next() {
        let runs = Runs::new();
        runs.begin("neos", Kind::Map, 1).unwrap();
        runs.finish("neos", true, None, 2).unwrap();
        assert!(runs.writing("neos").is_none());
        let again = runs.begin("neos", Kind::Lens, 3).unwrap();
        assert_eq!(again.phase, Phase::Writing);
        assert_eq!(again.kind, Kind::Lens);
        assert_eq!(again.started, 3);
        assert!(again.ended.is_none(), "a restart is not still finished");
    }

    /// The agent reports the moment the map is written; its session exits a
    /// little later and reports again. The second report must not turn a
    /// finished run back into a running one, nor rewrite what the first said.
    #[test]
    fn the_first_party_to_finish_wins() {
        let runs = Runs::new();
        runs.begin("neos", Kind::Map, 1).unwrap();
        let done = runs.finish("neos", true, Some("12 files"), 2).unwrap();
        assert_eq!(done.phase, Phase::Done);
        assert!(runs.finish("neos", false, Some("exit 1"), 3).is_none());
        let after = runs.get("neos").unwrap();
        assert_eq!(after.phase, Phase::Done);
        assert_eq!(after.note.as_deref(), Some("12 files"));
        assert_eq!(after.ended, Some(2));
    }

    #[test]
    fn finishing_what_never_started_reports_nothing() {
        let runs = Runs::new();
        assert!(runs.finish("neos", true, None, 1).is_none());
        assert!(runs.get("neos").is_none());
    }

    #[test]
    fn notes_are_trimmed_dropped_when_empty_and_clipped_when_long() {
        let runs = Runs::new();
        runs.begin("a", Kind::Map, 1).unwrap();
        assert!(runs
            .finish("a", false, Some("  \n "), 2)
            .unwrap()
            .note
            .is_none());

        runs.begin("b", Kind::Lens, 1).unwrap();
        let long = "x".repeat(MAX_NOTE * 2);
        let note = runs
            .finish("b", false, Some(&long), 2)
            .unwrap()
            .note
            .unwrap();
        assert_eq!(note.chars().count(), MAX_NOTE + 1); // clipped, plus the ellipsis
    }

    /// The browser labels its badge, its corner card and its toast from
    /// `kind`, and keys its button on `phase`: both spellings are a wire
    /// contract, not internal names.
    #[test]
    fn a_run_reaches_the_browser_with_its_kind() {
        let runs = Runs::new();
        let run = runs.begin("neos", Kind::Lens, 1).unwrap();
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json["kind"], serde_json::json!("lens"));
        assert_eq!(json["phase"], serde_json::json!("writing"));
        assert_eq!(json["sandbox"], serde_json::json!("neos"));
    }

    #[test]
    fn every_run_is_listed_newest_first() {
        let runs = Runs::new();
        runs.begin("old", Kind::Map, 10).unwrap();
        runs.begin("new", Kind::Lens, 20).unwrap();
        assert_eq!(
            runs.all()
                .iter()
                .map(|r| r.sandbox.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
    }
}
