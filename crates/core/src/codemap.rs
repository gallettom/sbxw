//! Background code map runs: which sandbox is writing its map (or a lens), and
//! how the last run of each ended.
//!
//! One slot per sandbox, map or lens alike — a lens is written *from* the map
//! and must not read one moving under it. A run ends from whichever side
//! reports first: the agent (`$SBXW_CODEMAP_DONE`, earlier and better
//! informed) or the session's exit (the backstop). The second report is ignored.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Writing,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Map,
    Lens,
}

impl Kind {
    pub fn what(self) -> &'static str {
        match self {
            Kind::Map => "its code map",
            Kind::Lens => "a lens",
        }
    }
}

/// Where the map lives under `.sbxw-artifacts`, and where lenses go.
pub const CODEMAP_DIR: &str = "codemap";
pub const LENS_DIR: &str = "codemap-lenses";
/// Longest brief a lens may be given: a sentence naming a reader.
pub const LENS_OBJECTIVE_MAX: usize = 400;
const MAX_NOTE: usize = 300;

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct Run {
    pub sandbox: String,
    pub kind: Kind,
    pub phase: Phase,
    pub started: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

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

pub fn already_writing(sandbox: &str, kind: Kind) -> String {
    format!("'{sandbox}' is already writing {}", kind.what())
}

/// A lens brief, as one line: collapsed rather than refused, since a textarea
/// wraps and Enter mid-thought is no error.
pub fn lens_objective(raw: &str) -> Result<String, String> {
    let one = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return Err("say what the lens is for — who is reading, and what they need from it".into());
    }
    let n = one.chars().count();
    if n > LENS_OBJECTIVE_MAX {
        return Err(format!(
            "that brief is {n} characters; keep it under {LENS_OBJECTIVE_MAX} — a sentence naming \
             the reader and what they need"
        ));
    }
    Ok(one)
}

#[derive(Default, Debug)]
pub struct RunBook {
    runs: HashMap<String, Run>,
}

impl RunBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self, sandbox: &str, kind: Kind, now: u64) -> Result<Run, String> {
        if let Some(run) = self.runs.get(sandbox) {
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
        self.runs.insert(sandbox.to_string(), run.clone());
        Ok(run)
    }

    /// The first party to report wins; a report with nothing running is `None`.
    pub fn finish(&mut self, sandbox: &str, ok: bool, note: Option<&str>, now: u64) -> Option<Run> {
        let run = self.runs.get_mut(sandbox)?;
        if run.phase != Phase::Writing {
            return None;
        }
        run.phase = if ok { Phase::Done } else { Phase::Failed };
        run.ended = Some(now);
        run.note = note.and_then(clip);
        Some(run.clone())
    }

    pub fn get(&self, sandbox: &str) -> Option<&Run> {
        self.runs.get(sandbox)
    }

    pub fn writing(&self, sandbox: &str) -> Option<Kind> {
        self.get(sandbox).filter(|r| r.phase == Phase::Writing).map(|r| r.kind)
    }

    /// Newest first.
    pub fn all(&self) -> Vec<Run> {
        let mut out: Vec<Run> = self.runs.values().cloned().collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.started));
        out
    }
}

/// The last `n` non-blank lines of a session's output, as one line — what a
/// headless agent prints last is its own account of what it did.
pub fn output_tail(raw: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(raw);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_slot_per_sandbox_map_or_lens() {
        let mut b = RunBook::new();
        b.begin("n", Kind::Map, 1).unwrap();
        assert!(b.begin("n", Kind::Lens, 2).is_err());
        assert_eq!(b.writing("n"), Some(Kind::Map));
        b.begin("other", Kind::Lens, 2).unwrap();
    }

    #[test]
    fn the_first_to_finish_wins() {
        let mut b = RunBook::new();
        b.begin("n", Kind::Map, 1).unwrap();
        assert_eq!(b.finish("n", true, Some(" written "), 2).unwrap().note.as_deref(), Some("written"));
        assert!(b.finish("n", false, Some("exit 1"), 3).is_none());
        assert_eq!(b.get("n").unwrap().phase, Phase::Done);
        b.begin("n", Kind::Lens, 4).unwrap();
        assert!(b.finish("nothing", true, None, 5).is_none());
    }

    #[test]
    fn notes_are_clipped_and_runs_listed_newest_first() {
        let mut b = RunBook::new();
        b.begin("a", Kind::Map, 1).unwrap();
        b.begin("b", Kind::Map, 2).unwrap();
        let long = "x".repeat(400);
        assert!(b.finish("a", true, Some(&long), 3).unwrap().note.unwrap().ends_with('…'));
        assert_eq!(b.all()[0].sandbox, "b");
        assert!(b.finish("b", true, Some("  "), 4).unwrap().note.is_none());
    }

    #[test]
    fn a_lens_brief_is_one_bounded_line() {
        assert_eq!(lens_objective("a product\n owner").unwrap(), "a product owner");
        assert!(lens_objective("   ").is_err());
        assert!(lens_objective(&"x ".repeat(300)).is_err());
    }

    #[test]
    fn the_tail_is_the_last_lines() {
        assert_eq!(output_tail(b"a\n\nb\nc\nd\n", 3), "b c d");
        assert_eq!(output_tail(b"", 3), "");
    }
}
