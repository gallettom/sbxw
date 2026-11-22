//! Files attached to what the person writes to a session from the Monitor.
//!
//! * **Where a path comes from.** Only the core: the file dialog
//!   (`monitor_attach`, an `Action::PickFiles` the core runs only from a
//!   press), files dropped on the Monitor, a picture pasted while it shows
//!   (`on_files`). No intent takes a path, and agent text never becomes one.
//! * **Whose files.** Each session's own: a file is kept under the card it
//!   was attached to, changing session shows that session's files, and a
//!   send only ever takes the files of the session it goes to.
//! * **The guardrails** are the Files view's (`feat::files::drops`): a file
//!   whose name looks like a secret, in a `.ssh`, `.aws` or `.gnupg` folder,
//!   or a send of more than 25 MB in all is marked "look" and the dialog
//!   needs its box ticked; an empty file or one over 128 MB cannot go.
//! * **Sending** is one core dialog for the words and every file (local
//!   path, size, total, target). On Send the core uploads the files one after
//!   the other (`core:uploads`, then a pasted picture's `paste-image`), and
//!   only when all are in, the words followed by the paths the daemon gave go
//!   as one message (`/api/chat/push`). A failure stops there: nothing is
//!   pushed, the words stay, a file already uploaded keeps its daemon path
//!   (the core cannot read a local path twice) and goes with the next Send.
//! * **Names** are the person's files' and may hold anything: in a chip
//!   they are cut and their control and direction characters drawn as `?`;
//!   in the dialog they are shown whole, those characters written out
//!   (`\u{202e}`). The log gets counts, never a name or a path.

use crate::feat::files::{drops, view as fview};
use serde_json::{json, Value as J};
use std::path::PathBuf;

/// The files one session may hold at a time.
pub const MAX: usize = 10;

/// A name in a chip: at most this many characters (its middle cut).
pub const CHIP_CHARS: usize = 36;

#[derive(Debug, Clone)]
pub enum Payload {
    /// A file the person handed in (the dialog, a drop).
    Path(PathBuf),
    /// A picture from the clipboard (PNG), sent to `paste-image`.
    Png(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Att {
    /// Its id: chips, removal and its upload's tag (`monitor-up-<id>`).
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub payload: Payload,
    /// Why the dialog asks its box to be ticked for it ([`drops::guardrails`]).
    pub look: Vec<String>,
    /// Uploaded by a send that stopped later: the daemon's path, kept for the next one.
    pub uploaded: Option<String>,
}

impl Att {
    /// A file from the person's machine, read now (`Err`: why it is not attached).
    pub fn of_path(id: u64, p: PathBuf) -> Result<Att, String> {
        let m = std::fs::metadata(&p).map_err(|e| format!("a file could not be read ({e})"))?;
        if !m.is_file() {
            return Err("a folder: attach the files in it instead".into());
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "upload".into());
        let look = drops::guardrails(std::slice::from_ref(&p), 1);
        Ok(Att { id, name, size: m.len(), payload: Payload::Path(p), look, uploaded: None })
    }

    pub fn of_png(id: u64, n: u64, bytes: Vec<u8>) -> Att {
        Att { id, name: format!("pasted-{n}.png"), size: bytes.len() as u64, payload: Payload::Png(bytes), look: vec![], uploaded: None }
    }

    /// Why it cannot be sent at all, if so (the Files view's rule).
    pub fn problem(&self) -> Option<String> {
        if self.uploaded.is_some() {
            None
        } else if self.size == 0 {
            Some("empty: the daemon takes no empty file".into())
        } else if self.size > fview::UPLOAD_MAX {
            Some(format!("too large ({}): the daemon takes {} at most", fview::size_label(self.size), fview::size_label(fview::UPLOAD_MAX)))
        } else {
            None
        }
    }

    /// Where it comes from, whole, for the dialog.
    pub fn source(&self) -> String {
        match &self.payload {
            Payload::Path(p) => written_out(&p.display().to_string()),
            Payload::Png(_) => format!("{} (a picture from the clipboard)", self.name),
        }
    }

    pub fn tag(&self) -> String {
        format!("monitor-up-{}", self.id)
    }
}

/// Characters a name may hold that would change how the line around it
/// reads: controls, and the one list of invisible characters
/// ([`crate::safe::is_invisible`]).
fn hidden(c: char) -> bool {
    c.is_control() || crate::safe::is_invisible(c)
}

/// Whole, each hidden character written out (`\u{202e}`): the dialog's.
pub fn written_out(s: &str) -> String {
    s.chars().map(|c| if hidden(c) { format!("\\u{{{:x}}}", c as u32) } else { c.to_string() }).collect()
}

/// For a chip: hidden characters as `?`, at most [`CHIP_CHARS`], its middle cut.
pub fn clipped(s: &str) -> String {
    let v: Vec<char> = s.chars().map(|c| if hidden(c) { '?' } else { c }).collect();
    if v.len() <= CHIP_CHARS {
        return v.into_iter().collect();
    }
    let head = (CHIP_CHARS - 1) * 2 / 3;
    let tail = CHIP_CHARS - 1 - head;
    let mut out: String = v[..head].iter().collect();
    out.push('\u{2026}');
    out.extend(v[v.len() - tail..].iter());
    out
}

/// Agent text cut to `max` characters, controls but new lines and tabs
/// dropped (drawn as plain text).
pub fn clipped_text(s: &str, max: usize) -> String {
    let mut out: String = s.chars().filter(|c| !hidden(*c) || *c == '\n' || *c == '\t').take(max).collect();
    if s.chars().count() > max {
        out.push('\u{2026}');
    }
    out
}

/// The look reasons of a set of files: each file's, then their total.
pub fn look_all(files: &[Att]) -> Vec<String> {
    let mut why: Vec<String> = files.iter().filter(|a| a.uploaded.is_none()).flat_map(|a| a.look.iter().cloned()).map(|w| written_out(&w)).collect();
    let total: u64 = files.iter().filter(|a| a.uploaded.is_none()).map(|a| a.size).sum();
    if total > drops::ASK_ABOVE_BYTES {
        why.push(format!("{} in all (more than {})", fview::size_label(total), fview::size_label(drops::ASK_ABOVE_BYTES)));
    }
    why
}

/// The chips the screen draws for a session's files (`detail.files`), with
/// each upload's progress while a send runs (`data.app.uploads`).
pub fn chips(files: &[Att], uploads: &J, sending: bool) -> J {
    J::Array(files.iter().map(|a| {
        let (state, why) = match (a.problem(), &a.uploaded, a.look.is_empty()) {
            (Some(p), _, _) => ("refused", Some(p)),
            (None, Some(_), _) => ("uploaded", Some("Already in the sandbox: it goes with your next Send".to_string())),
            (None, None, false) => ("look", Some(format!("Look before you send: {}", a.look.iter().map(|w| written_out(w)).collect::<Vec<_>>().join("; ")))),
            (None, None, true) => ("ok", None),
        };
        let u = &uploads[a.tag()];
        let progress = match (u["sent"].as_f64(), u["total"].as_f64()) {
            (Some(s), Some(t)) if t > 0.0 => s / t,
            _ => 0.0,
        };
        json!({ "id": a.id, "name": clipped(&a.name), "size": fview::size_label(a.size), "state": state, "why": why,
            "sending": (sending || u["state"] == "sending") && a.uploaded.is_none(), "progress": progress })
    }).collect())
}

/// A send with files, from its dialog's Send to the push: which files of
/// which session, the words as the dialog showed them, and what is being
/// waited for.
#[derive(Debug, Clone)]
pub struct Job {
    pub key: String,
    /// The masked words (may be empty: the message is then the paths).
    pub text: String,
    /// The files the dialog listed, by id, in order.
    pub ids: Vec<u64>,
    /// The answer waited for: the batch of local files, or one pasted picture.
    pub waiting: Waiting,
    /// An answer came: the dialog's Send was pressed (before, its Cancel drops the job).
    pub sent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Waiting {
    Batch,
    Png(u64),
}
