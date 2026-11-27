//! Previews of the pictures the person sent to the shown session: a
//! prompt of theirs whose text holds the path of a picture sbxw uploaded
//! (`/tmp/sbxw-pastes/<name>`, `/tmp/sbxw-uploads/<n>/<name>`) shows it
//! small under the text.
//!
//! * **Whose text.** The person's prompts only (rows `t = "you"`). A path
//!   in a reply, a step, a result, a question or another session's message
//!   is never read: agent text is data and makes the app fetch nothing.
//! * **Which paths.** Only under those two folders, a plain name (no `/`,
//!   no `..`, not hidden, no control character), ending in `.png`, `.jpg`,
//!   `.jpeg`, `.gif` or `.webp`. A GIF is not decoded here
//!   ([`crate::picture`] reads PNG, JPEG and WebP): it says so, unread.
//! * **Bounds.** Only the session followed (`conv.sub`), only rows inside
//!   the drawn window, the newest [`CAP`] paths, [`INFLIGHT`] reads at a
//!   time, each read once through the Files view's route
//!   (`GET .../fs/file?path=..&max=..`, the daemon refuses past `max`),
//!   decoded within the Files view's cap and kept scaled to [`FIT`] px.
//!   Another session, or the view left: every preview is forgotten
//!   (`Action::Image` with no bytes).
//! * **Failures** are a muted line under the text, never a notice.
//! * **Opened large.** A press on a ready preview names it by its key
//!   ([`Pics::zoom_open`]); its path comes from [`Pics::map`] only. The
//!   file is read again, decoded within [`ZOOM_FIT`] px, for the dialog
//!   (`monitor-picture`); one such picture at a time, forgotten when the
//!   dialog closes, another opens, another session is shown or the view
//!   is left.

use super::conv::Sid;
use crate::feat::{Action, Call};
use serde_json::{json, Value as J};
use std::collections::BTreeMap;

/// Previews kept for the shown session (its newest paths).
pub const CAP: usize = 12;
/// Reads under way at a time.
pub const INFLIGHT: usize = 2;
/// A preview's longest side once decoded (twice the drawn 180 pt).
pub const FIT: u32 = 360;
/// A picture opened large: its longest side once decoded, the Files
/// view's own preview bound (`feat/files/mod.rs`, `got_preview`).
pub const ZOOM_FIT: u32 = 1600;
/// The largest picture read: the Files view's own cap.
pub const MAX_BYTES: u64 = crate::feat::files::view::IMAGE_MAX;

pub const GONE: &str = "Picture no longer in the sandbox";
pub const TOO_LARGE: &str = "Picture too large to preview";
pub const UNREADABLE: &str = "Picture cannot be previewed";
pub const NOT_READ: &str = "Picture could not be read";
pub const GIF: &str = "GIF pictures are not previewed";

const PASTES: &str = "/tmp/sbxw-pastes/";
const UPLOADS: &str = "/tmp/sbxw-uploads/";
const EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// A file name sbxw could have written: plain, not hidden, a picture's extension.
fn plain_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 200
        && !n.starts_with('.')
        && !n.contains("..")
        && !n.chars().any(|c| c == '/' || c == '\\' || c.is_control() || crate::safe::is_invisible(c))
        && n.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// The path, when it is one of the person's sent pictures.
pub fn picture_path(tok: &str) -> Option<&str> {
    if let Some(name) = tok.strip_prefix(PASTES) {
        return plain_name(name).then_some(tok);
    }
    let rest = tok.strip_prefix(UPLOADS)?;
    let (n, name) = rest.split_once('/')?;
    (!n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()) && plain_name(name)).then_some(tok)
}

/// The picture paths a prompt's text holds, in order, each once.
pub fn paths_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for tok in text.split_whitespace() {
        if let Some(p) = picture_path(tok) {
            if !out.iter().any(|x| x == p) {
                out.push(p.to_string());
            }
        }
    }
    out
}

fn is_gif(p: &str) -> bool {
    p.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case("gif"))
}

#[derive(Debug, Clone, PartialEq)]
pub enum St {
    Queued,
    /// Read `n` under way.
    Loading(u64),
    Ready { key: String, w: u32, h: u32 },
    Note(&'static str),
}

/// The previews of the session followed.
#[derive(Debug, Default)]
pub struct Pics {
    /// Whose previews these are.
    pub sid: Option<Sid>,
    /// By path; at most [`CAP`].
    pub map: BTreeMap<String, St>,
    /// The paths wanted, newest first (the order reads start in).
    order: Vec<String>,
    seq: u64,
    /// The one picture opened large, if any.
    pub zoom: Option<Zoom>,
}

/// A picture opened large in the dialog: its path (from [`Pics::map`]),
/// its preview (drawn while the large copy loads) and the large copy.
#[derive(Debug)]
pub struct Zoom {
    pub path: String,
    pub small: (String, u32, u32),
    pub st: St,
}

/// A read's answer decoded within `fit` px, or the muted line it gets.
fn decode(r: Result<(u16, Vec<u8>, String), String>, fit: u32) -> Result<crate::picture::Picture, &'static str> {
    match r {
        Ok((200, bytes, _)) if bytes.len() as u64 > MAX_BYTES => Err(TOO_LARGE),
        Ok((200, bytes, _)) => {
            let limits = crate::picture::Limits { max_bytes: MAX_BYTES as usize, ..crate::picture::Limits::default() };
            crate::picture::prepare(&bytes, limits, Some(fit), crate::picture::Encode::Png).map_err(|e| {
                // `picture`'s limits say "more than" (bytes, sides, pixels).
                if e.contains("more than") { TOO_LARGE } else { UNREADABLE }
            })
        }
        Ok((code, body, _)) => {
            let why = String::from_utf8_lossy(&body[..body.len().min(4096)]).to_ascii_lowercase();
            Err(if code == 413 || why.contains("too large") || why.contains("larger than") { TOO_LARGE } else { GONE })
        }
        Err(_) => Err(NOT_READ),
    }
}

fn forget(st: &St) -> Option<Action> {
    match st {
        St::Ready { key, .. } => Some(Action::Image { key: key.clone(), bytes: vec![] }),
        _ => None,
    }
}

impl Pics {
    /// Follows `sid` and its drawn `rows`: previews no longer wanted are
    /// forgotten, new ones queued, reads started up to [`INFLIGHT`].
    /// Returns the actions and whether what is drawn changed.
    pub fn sync(&mut self, sid: Option<&Sid>, rows: &[J]) -> (Vec<Action>, bool) {
        let mut out = vec![];
        let mut changed = false;
        if self.sid.as_ref() != sid {
            out.extend(self.map.values().filter_map(forget));
            out.extend(self.zoom_close());
            changed |= !self.map.is_empty();
            self.map.clear();
            self.order.clear();
            self.sid = sid.cloned();
        }
        let Some(sid) = sid else { return (out, changed) };
        let mut wanted: Vec<String> = vec![];
        'rows: for r in rows.iter().rev().filter(|r| r["t"] == "you") {
            for p in paths_in(r["text"].as_str().unwrap_or("")) {
                if wanted.len() >= CAP {
                    break 'rows;
                }
                if !wanted.contains(&p) {
                    wanted.push(p);
                }
            }
        }
        let before = self.map.len();
        let gone: Vec<String> = self.map.keys().filter(|k| !wanted.contains(k)).cloned().collect();
        for k in gone {
            if let Some(st) = self.map.remove(&k) {
                out.extend(forget(&st));
            }
        }
        changed |= self.map.len() != before;
        for p in &wanted {
            if !self.map.contains_key(p) {
                let st = if is_gif(p) { St::Note(GIF) } else { St::Queued };
                changed |= st != St::Queued;
                self.map.insert(p.clone(), st);
            }
        }
        self.order = wanted;
        out.extend(self.start(&sid.0));
        (out, changed)
    }

    /// Reads started for the queued paths, newest first, up to [`INFLIGHT`].
    fn start(&mut self, sandbox: &str) -> Vec<Action> {
        let mut out = vec![];
        let mut busy = self.map.values().filter(|s| matches!(s, St::Loading(_))).count();
        for p in self.order.clone() {
            if busy >= INFLIGHT {
                break;
            }
            if self.map.get(&p) != Some(&St::Queued) {
                continue;
            }
            self.seq += 1;
            let n = self.seq;
            self.map.insert(p.clone(), St::Loading(n));
            busy += 1;
            let path = format!("/api/sandboxes/{}/fs/file?path={}&max={MAX_BYTES}", crate::client::enc(sandbox), crate::client::enc(&p));
            out.push(Action::Call(Call::get(path, "read a picture you sent").raw(MAX_BYTES).tag(&format!("pic:{n}")).quiet()));
        }
        out
    }

    /// Read `n` answered: decoded into a preview, or its line. `None`: a
    /// read no longer waited for (another session, the path dropped).
    pub fn got(&mut self, n: u64, r: Result<(u16, Vec<u8>, String), String>) -> Option<Vec<Action>> {
        let path = self.map.iter().find(|(_, s)| **s == St::Loading(n)).map(|(p, _)| p.clone())?;
        let mut out = vec![];
        let st = match decode(r, FIT) {
            Ok(p) => {
                let key = format!("monitor-pic-{n}");
                out.push(Action::Image { key: key.clone(), bytes: p.bytes });
                St::Ready { key, w: p.width, h: p.height }
            }
            Err(t) => St::Note(t),
        };
        self.map.insert(path, st);
        if let Some(sid) = self.sid.clone() {
            out.extend(self.start(&sid.0));
        }
        Some(out)
    }

    /// The preview drawn under `key`, and its path: only a ready preview of
    /// the session shown (the map holds nothing else).
    pub fn ready(&self, key: &str) -> Option<(String, String, u32, u32)> {
        self.map.iter().find_map(|(p, s)| match s {
            St::Ready { key: k, w, h } if k == key => Some((p.clone(), k.clone(), *w, *h)),
            _ => None,
        })
    }

    /// The picture of preview `key` opened large: the large picture held
    /// before is forgotten, the file read again ([`ZOOM_FIT`]). `None`: no
    /// ready preview of that key (nothing opens).
    pub fn zoom_open(&mut self, key: &str) -> Option<Vec<Action>> {
        let (path, small, w, h) = self.ready(key)?;
        let sid = self.sid.clone()?;
        let mut out = self.zoom_close();
        self.seq += 1;
        let n = self.seq;
        let route = format!("/api/sandboxes/{}/fs/file?path={}&max={MAX_BYTES}", crate::client::enc(&sid.0), crate::client::enc(&path));
        out.push(Action::Call(Call::get(route, "read a picture you sent").raw(MAX_BYTES).tag(&format!("zoom:{n}")).quiet()));
        self.zoom = Some(Zoom { path, small: (small, w, h), st: St::Loading(n) });
        Some(out)
    }

    /// The large read `n` answered. `None`: not the one waited for (closed,
    /// another opened): nothing is shown.
    pub fn zoom_got(&mut self, n: u64, r: Result<(u16, Vec<u8>, String), String>) -> Option<Vec<Action>> {
        let z = self.zoom.as_mut().filter(|z| z.st == St::Loading(n))?;
        Some(match decode(r, ZOOM_FIT) {
            Ok(p) => {
                let key = format!("monitor-zoom-{n}");
                z.st = St::Ready { key: key.clone(), w: p.width, h: p.height };
                vec![Action::Image { key, bytes: p.bytes }]
            }
            Err(t) => {
                z.st = St::Note(t);
                vec![]
            }
        })
    }

    /// The large picture forgotten (the dialog closed).
    pub fn zoom_close(&mut self) -> Vec<Action> {
        self.zoom.take().and_then(|z| forget(&z.st)).into_iter().collect()
    }

    /// `data.monitor_picture`: what the dialog draws, or null.
    pub fn zoom_view(&self) -> J {
        let Some(z) = &self.zoom else { return J::Null };
        let name = z.path.rsplit('/').next().unwrap_or("");
        let mut v = json!({ "name": super::attach::clipped(name), "small": { "key": z.small.0, "w": z.small.1, "h": z.small.2 } });
        match &z.st {
            St::Ready { key, w, h } => {
                v["state"] = "ready".into();
                v["key"] = key.as_str().into();
                v["w"] = (*w).into();
                v["h"] = (*h).into();
            }
            St::Note(t) => {
                v["state"] = "note".into();
                v["note"] = (*t).into();
            }
            _ => v["state"] = "loading".into(),
        }
        v
    }

    /// The rows as drawn, each prompt of the person given its previews
    /// (`pics`: `{key, w, h}` or `{note}`; a read under way draws nothing
    /// yet). Only for `sid`, the session these previews belong to.
    pub fn annotate(&self, sid: &Sid, rows: &mut J) {
        if self.sid.as_ref() != Some(sid) || self.map.is_empty() {
            return;
        }
        for r in rows.as_array_mut().into_iter().flatten().filter(|r| r["t"] == "you") {
            let pics: Vec<J> = paths_in(r["text"].as_str().unwrap_or("")).iter().filter_map(|p| match self.map.get(p)? {
                St::Ready { key, w, h } => Some(json!({ "key": key, "w": w, "h": h })),
                St::Note(t) => Some(json!({ "note": t })),
                _ => None,
            }).collect();
            if pics.is_empty() {
                continue;
            }
            // The screen's row memo follows the signature: it moves with the previews.
            let sig = format!("{}~{}", r["sig"].as_str().unwrap_or(""), pics.iter().map(|p| p["key"].as_str().or(p["note"].as_str()).unwrap_or("")).collect::<Vec<_>>().join(","));
            r["sig"] = sig.into();
            r["pics"] = J::Array(pics);
        }
    }

    /// Every preview forgotten (the view left).
    pub fn clear(&mut self) -> Vec<Action> {
        self.sync(None, &[]).0
    }
}
