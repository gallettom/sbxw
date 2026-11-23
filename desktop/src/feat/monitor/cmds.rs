//! The composer's completion list: the skills and slash commands of the
//! shown session's sandbox, as `GET /api/sandboxes/:name/commands` lists
//! them (the daemon reads them; a stopped sandbox is never started, its
//! answer is then the project's part only).
//!
//! * **When.** Read once, when the composer first needs it (the screen asks
//!   with `monitor_commands` when the draft starts with `/` and it has no
//!   list for that sandbox), and again when the shown session is in another
//!   sandbox. Never on a timer. One sandbox's list is held at a time.
//! * **Data.** Names and descriptions are written in the sandbox: names
//!   are checked (the daemon's pattern), descriptions cleaned as the
//!   conversation's text is (controls and invisible characters dropped,
//!   one line, cut). A daemon without the route (404), or any failure,
//!   shows no list at all, and says nothing.

use serde_json::{json, Value as J};

/// The longest description drawn, in characters.
pub const DESC_MAX: usize = 100;
/// The most entries kept from one answer.
pub const MAX: usize = 400;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Loading,
    Ready(Vec<(String, String)>),
    /// Read and failed, or a daemon without the route: no list.
    None,
}

/// The list held, and the sandbox it is for.
#[derive(Debug, Clone, PartialEq)]
pub struct Cmds {
    pub sandbox: String,
    pub state: State,
}

pub fn path(sb: &str) -> String {
    format!("/api/sandboxes/{}/commands", crate::client::enc(sb))
}

pub fn tag(sb: &str) -> String {
    format!("cmds:{sb}")
}

/// A name the daemon may hand over: `^[a-z0-9][a-z0-9:_-]{0,63}$`.
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty() && b.len() <= 64 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b':' | b'_' | b'-'))
}

/// A description on one line: hidden characters dropped, any run of
/// spaces, tabs and line breaks one space, cut to [`DESC_MAX`].
pub fn clean_desc(s: &str) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\t' || c == '\r' { ' ' } else { c }).collect();
    let flat = flat.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    super::attach::clipped_text(&flat, DESC_MAX).replace(['\n', '\t'], " ")
}

/// The entries of an answer, checked: a bad name is left out, a name once.
pub fn parse(b: &J) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for c in b["commands"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(n) = c["name"].as_str().filter(|n| valid_name(n)) else { continue };
        if out.iter().any(|(x, _)| x == n) {
            continue;
        }
        out.push((n.to_string(), clean_desc(c["description"].as_str().unwrap_or(""))));
        if out.len() >= MAX {
            break;
        }
    }
    out
}

/// `detail.commands`: what the screen draws for that sandbox, `null` when
/// nothing was asked for it yet.
pub fn view(c: Option<&Cmds>, sb: &str) -> J {
    match c.filter(|c| c.sandbox == sb).map(|c| &c.state) {
        None => J::Null,
        Some(State::Loading) => json!({ "state": "loading", "items": [] }),
        Some(State::None) => json!({ "state": "none", "items": [] }),
        Some(State::Ready(v)) => json!({ "state": "ready", "items": v.iter().map(|(n, d)| json!({ "name": n, "desc": d })).collect::<Vec<_>>() }),
    }
}
