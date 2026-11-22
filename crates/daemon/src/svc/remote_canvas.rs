//! The canvas on the phone (`.sbxw-artifacts/remote/API-canvas.md`): the
//! canvases of a sandbox the phone reaches, one canvas's shots and its
//! draft remarks, its pictures, a remark added, changed or removed, and a
//! round sent. Every call goes through the Mac's own canvas service
//! (`canvas_list`, `canvas_get`, `canvas_image`, `canvas_edit`,
//! `canvas_submit`); nothing here writes a manifest itself.
//!
//! * A canvas answers only while its sandbox is reachable
//!   ([`App::remote_target`]): a thread's, Jarvis's, a stopped sandbox's or
//!   an unknown canvas is the same not found.
//! * Agent text (titles, notes) is masked by the anonymise rules before it
//!   leaves; the phone's remark texts are masked before they are stored.
//! * A round is sent as the Mac sends it: the round recorded, then ONE fixed
//!   line typed into the main session, under the phone's message gate.
//! * Never logged, never journaled: a title, a remark's text, a file name.

use super::remote_access::{DeviceCtx, Reason, Refusal, HOOK_GRACE};
use crate::app::App;
use sbxw_core::canvas::{self, Canvas, Edit, Note, Pin};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// A remark's text, in characters, after trimming (the canvas's own cap).
pub const TEXT_MAX: usize = canvas::MAX_NOTE;

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Add,
    Edit,
    Remove,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemarkKind {
    Pin,
    Note,
}

/// `POST /r/canvas/:id/remarks`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemarkIn {
    pub op: Op,
    pub kind: RemarkKind,
    #[serde(default)]
    pub shot: Option<String>,
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub text: Option<String>,
}

/// Why a round was not sent from the phone.
#[derive(Debug, PartialEq)]
pub enum SubmitError {
    Refused(Refusal),
    /// The draft holds example pictures added on the Mac.
    Examples,
    /// The round IS recorded, but the line was not submitted.
    NotSubmitted(u32),
}

impl From<Refusal> for SubmitError {
    fn from(r: Refusal) -> Self {
        SubmitError::Refused(r)
    }
}

fn round1(v: f32) -> f64 {
    ((v as f64) * 1000.0).round() / 10.0
}

/// When the last round of remarks was sent (0: never).
fn last_review_ms(c: &Canvas) -> u64 {
    c.reviews.last().map_or(0, |r| r.at_ms)
}

fn unsent_examples(c: &Canvas) -> usize {
    c.assets.iter().filter(|a| a.sent.is_none()).count()
}

/// The draft pins on shots and the draft notes, as the phone shows them.
fn draft_view(c: &Canvas) -> (Vec<Value>, Vec<Value>) {
    let mut pins: Vec<&Pin> = c.pins.iter().filter(|p| p.sent.is_none() && c.shots.iter().any(|s| s.id == p.on)).collect();
    pins.sort_by_key(|p| p.n);
    let mut notes: Vec<&Note> = c.notes.iter().filter(|n| n.sent.is_none()).collect();
    notes.sort_by_key(|n| n.n);
    (
        pins.iter().map(|p| json!({ "n": p.n, "shot": p.on, "x": round1(p.fx), "y": round1(p.fy), "text": p.text })).collect(),
        notes.iter().map(|n| json!({ "n": n.n, "shot": c.note_shot(n), "text": n.text })).collect(),
    )
}

/// A remark's text from the phone: trimmed, 1 to [`TEXT_MAX`] characters,
/// no control character but a line break, none of the one invisible list
/// ([`sbxw_core::invisible::is_invisible`]).
fn checked_text(t: Option<&str>) -> Result<&str, Refusal> {
    let t = t.map(str::trim).unwrap_or("");
    if t.is_empty() {
        return Err(Refusal::Bad("a remark needs a text".into()));
    }
    if t.chars().count() > TEXT_MAX {
        return Err(Refusal::Bad(format!("a remark is at most {TEXT_MAX} characters")));
    }
    if t.chars().any(|c| (c.is_control() && c != '\n') || sbxw_core::invisible::is_invisible(c)) {
        return Err(Refusal::Reason(Reason::Control));
    }
    Ok(t)
}

/// A point on a shot, in percent: both or neither, finite, 0 to 100.
fn checked_point(x: Option<f64>, y: Option<f64>) -> Result<Option<(f32, f32)>, Refusal> {
    match (x, y) {
        (None, None) => Ok(None),
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() && (0.0..=100.0).contains(&x) && (0.0..=100.0).contains(&y) => Ok(Some(((x / 100.0) as f32, (y / 100.0) as f32))),
        _ => Err(Refusal::Bad("x and y: numbers from 0 to 100, both".into())),
    }
}

impl App {
    /// A canvas the phone may see: a canvas id, a canvas of a sandbox the
    /// phone reaches now. Every failure is the same not found.
    pub fn remote_canvas_of(&self, id: &str) -> Result<(Canvas, SandboxRef), Refusal> {
        if !canvas::valid_id(id) {
            return Err(Refusal::NotFound);
        }
        let c = self.canvas_get(id).ok_or(Refusal::NotFound)?;
        let sb = self.remote_target(&c.sandbox)?;
        Ok((c, sb))
    }

    fn remote_mask(&self) -> impl Fn(&str) -> String {
        let rules = self.anonymise_rules().unwrap_or_default();
        move |t: &str| sbxw_core::anonymise::mask(t, &rules).text
    }

    /// `GET /r/leaders/:sandbox/canvas`, for a sandbox already resolved.
    pub fn remote_canvas_list(&self, sb: &SandboxRef) -> Value {
        let m = self.remote_mask();
        let mine = sb.to_string();
        let (all, _) = self.canvas_list();
        let list: Vec<Value> = all
            .into_iter()
            .filter(|s| s.sandbox == mine)
            .filter_map(|s| self.canvas_get(&s.id))
            .map(|c| {
                let since = last_review_ms(&c);
                json!({
                    "id": c.id,
                    "title": m(&c.title),
                    "shots": c.shots.len(),
                    "rounds": c.rounds.len(),
                    "new_shots": c.shots.iter().filter(|s| s.added_ms > since).count(),
                    "last_round_ms": c.rounds.last().map(|r| r.at_ms),
                    "reviews": c.reviews.len(),
                    "unread": c.unread().len(),
                    "draft": c.draft(),
                    "draft_examples": unsent_examples(&c),
                    "updated_ms": c.updated_ms,
                })
            })
            .collect();
        json!({ "sandbox": mine, "canvases": list })
    }

    /// `GET /r/canvas/:id`: the manifest reduced to what the phone shows.
    pub fn remote_canvas_view(&self, d: &DeviceCtx, id: &str) -> Result<Value, Refusal> {
        let (c, sb) = self.remote_canvas_of(id)?;
        let m = self.remote_mask();
        let since = last_review_ms(&c);
        let mut shots: Vec<&canvas::Shot> = c.shots.iter().collect();
        // Round order, then the order the agent added them (a stable sort).
        shots.sort_by_key(|s| s.round);
        let shots: Vec<Value> = shots
            .iter()
            .map(|s| {
                json!({
                    "id": s.id, "round": s.round, "title": m(&s.title), "note": m(&s.note),
                    "width": s.width, "screen": s.screen, "file": s.file, "px_w": s.px_w, "px_h": s.px_h,
                    "new": s.added_ms > since,
                })
            })
            .collect();
        let (pins, notes) = draft_view(&c);
        let rounds: Vec<Value> = c.rounds.iter().map(|r| json!({ "n": r.n, "at_ms": r.at_ms, "shots": r.shots })).collect();
        let reviews: Vec<Value> = c.reviews.iter().map(|r| json!({ "n": r.n, "at_ms": r.at_ms, "pins": r.pins, "notes": r.notes, "examples": r.assets, "read": r.read_ms.is_some() })).collect();
        self.remote_record(d, Kind::CanvasRead);
        Ok(json!({
            "id": c.id,
            "sandbox": sb.to_string(),
            "title": m(&c.title),
            "updated_ms": c.updated_ms,
            "rounds": rounds,
            "shots": shots,
            "pins": pins,
            "notes": notes,
            "sent": { "pins": c.pins.iter().filter(|p| p.sent.is_some()).count(), "notes": c.notes.iter().filter(|n| n.sent.is_some()).count() },
            "reviews": reviews,
            "draft": c.draft(),
            "draft_examples": unsent_examples(&c),
            "limits": { "text": TEXT_MAX, "pins": canvas::MAX_PINS, "notes": canvas::MAX_NOTES },
        }))
    }

    /// `GET /r/canvas/:id/images/:file`: a SHOT's picture only (never an
    /// example picture), by the name the manifest gives.
    pub fn remote_canvas_image(&self, id: &str, file: &str) -> Result<(Vec<u8>, &'static str), Refusal> {
        if !canvas::valid_file(file) {
            return Err(Refusal::NotFound);
        }
        let (c, _) = self.remote_canvas_of(id)?;
        if !c.shots.iter().any(|s| s.file == file) {
            return Err(Refusal::NotFound);
        }
        self.canvas_image(id, file).map_err(|_| Refusal::NotFound)
    }

    /// `POST /r/canvas/:id/remarks`: one remark of the draft, through
    /// `canvas_edit`. The answer carries the draft as the manifest does.
    pub fn remote_canvas_remark(&self, d: &DeviceCtx, id: &str, r: RemarkIn) -> Result<Value, Refusal> {
        let (c, _) = self.remote_canvas_of(id)?;
        let point = checked_point(r.x, r.y)?;
        let text = match r.op {
            Op::Remove => None,
            _ => Some(checked_text(r.text.as_deref())?),
        };
        let masked = match text {
            Some(t) => Some(self.anonymise(t).map_err(Refusal::Unavailable)?),
            None => None,
        };
        let count = masked.as_ref().map_or(0, |m| m.count);
        let text = masked.map(|m| m.text).unwrap_or_default();
        let shot_of = |s: &Option<String>| -> Result<String, Refusal> {
            let s = s.as_deref().ok_or_else(|| Refusal::Bad("a new remark names its shot".into()))?;
            c.shots.iter().find(|x| x.id == s).map(|x| x.id.clone()).ok_or(Refusal::NotFound)
        };
        let mut edit = Edit::default();
        let (next_pin, next_note) = c.next_numbers();
        let n = match r.kind {
            RemarkKind::Pin => {
                let mut pins: Vec<Pin> = c.pins.iter().filter(|p| p.sent.is_none()).cloned().collect();
                let n = match r.op {
                    Op::Add => {
                        let on = shot_of(&r.shot)?;
                        let (fx, fy) = point.ok_or_else(|| Refusal::Bad("a new pin needs x and y".into()))?;
                        pins.push(Pin { n: next_pin, on, fx, fy, text, sent: None });
                        next_pin
                    }
                    op => {
                        let n = r.n.ok_or_else(|| Refusal::Bad("n: the remark's number".into()))?;
                        let i = pins.iter().position(|p| p.n == n && c.shots.iter().any(|s| s.id == p.on)).ok_or(Refusal::NotFound)?;
                        if op == Op::Remove {
                            pins.remove(i);
                        } else {
                            pins[i].text = text;
                            if let Some((fx, fy)) = point {
                                (pins[i].fx, pins[i].fy) = (fx, fy);
                            }
                        }
                        n
                    }
                };
                edit.pins = Some(pins);
                n
            }
            RemarkKind::Note => {
                if point.is_some() {
                    return Err(Refusal::Bad("a note has no x and y on the phone".into()));
                }
                let mut notes: Vec<Note> = c.notes.iter().filter(|n| n.sent.is_none()).cloned().collect();
                let n = match r.op {
                    Op::Add => {
                        let on = shot_of(&r.shot)?;
                        let (x, y, w, h) = c.note_spot(&on).ok_or(Refusal::NotFound)?;
                        notes.push(Note { id: String::new(), n: next_note, x, y, w, h, text, sent: None, on: Some(on) });
                        next_note
                    }
                    op => {
                        let n = r.n.ok_or_else(|| Refusal::Bad("n: the remark's number".into()))?;
                        let i = notes.iter().position(|x| x.n == n).ok_or(Refusal::NotFound)?;
                        if op == Op::Remove {
                            notes.remove(i);
                        } else {
                            notes[i].text = text;
                        }
                        n
                    }
                };
                edit.notes = Some(notes);
                n
            }
        };
        let c = self.canvas_edit(id, edit).map_err(|e| if e.contains("at most") || e.contains("already used") { Refusal::Conflict(e) } else { Refusal::Bad(e) })?;
        self.remote_record(d, Kind::CanvasRemark);
        tracing::info!("remote access: device {}: canvas remark", d.id);
        let (pins, notes) = draft_view(&c);
        Ok(json!({ "ok": true, "n": n, "masked": count, "pins": pins, "notes": notes, "draft": c.draft() }))
    }

    /// `POST /r/canvas/:id/submit`: the round recorded by `canvas_submit`,
    /// then its fixed line typed into the main session as ONE paste and one
    /// Return, under the gate of a phone message
    /// ([`App::remote_can_type_message`], checked before the round is
    /// recorded and again before the Return).
    pub async fn remote_canvas_submit(self: &Arc<Self>, d: &DeviceCtx, id: &str) -> Result<u32, SubmitError> {
        let (c, sb) = self.remote_canvas_of(id)?;
        if c.draft() == 0 {
            return Err(Refusal::Bad("there is no new remark to send".into()).into());
        }
        if unsent_examples(&c) > 0 {
            return Err(SubmitError::Examples);
        }
        let refused = |r: Refusal| {
            self.remote_record(d, Kind::Refused);
            SubmitError::Refused(r)
        };
        if let Some(why) = self.remote_read_only(&sb) {
            return Err(refused(Refusal::Conflict(format!("'{sb}' is read only from the phone ({why})"))));
        }
        let Some(live) = self.sessions.live(&SessionKey::agent(&sb)) else {
            return Err(refused(Refusal::Reason(Reason::NeedsMac)));
        };
        let _one_at_a_time = live.typing.lock().await;
        let session = match self.remote_can_type_message(&sb) {
            Ok(s) if Arc::ptr_eq(&s, &live) => s,
            Ok(_) => return Err(refused(Refusal::Reason(Reason::NeedsMac))),
            Err(r) => return Err(refused(Refusal::Reason(r))),
        };
        // The Mac's own call: records the round and notes the line as sbxw's.
        let (n, c) = self.canvas_submit(id, &format!("{sb} · main"), &[]).map_err(|e| SubmitError::Refused(Refusal::Conflict(e)))?;
        let line = canvas::ready_line(&c.id, n);
        let mut rx = session.subscribe();
        session.write(&[sbxw_core::terminal::PASTE_START, line.as_bytes(), sbxw_core::terminal::PASTE_END]).await;
        crate::typing::settle(&mut rx, Some(crate::typing::ECHO_TIMEOUT), crate::typing::ECHO_QUIET).await;
        tokio::time::sleep(HOOK_GRACE).await;
        match self.remote_can_type_message(&sb) {
            Ok(again) if Arc::ptr_eq(&again, &session) => {}
            _ => {
                tracing::info!("remote access: device {}: canvas round recorded, its line not submitted", d.id);
                self.remote_record(d, Kind::Refused);
                return Err(SubmitError::NotSubmitted(n));
            }
        }
        session.write(&[b"\r"]).await;
        tracing::info!("remote access: device {}: canvas submit", d.id);
        self.remote_record(d, Kind::CanvasSubmit);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remark from the phone is refused for every character of the one
    /// invisible list, and kept for the characters around each range.
    #[test]
    fn remarks_refuse_every_invisible_character() {
        for c in sbxw_core::invisible::all() {
            let s = format!("a{c}b");
            assert!(matches!(checked_text(Some(&s)), Err(Refusal::Reason(Reason::Control))), "{c:?}");
        }
        for c in sbxw_core::invisible::neighbours() {
            let s = format!("a{c}b");
            assert_eq!(checked_text(Some(&s)).ok(), Some(s.as_str()), "{c:?}");
        }
        // The emoji and text selectors are accepted.
        for s in ["\u{2764}\u{fe0f} thanks", "\u{2764}\u{fe0e}"] {
            assert_eq!(checked_text(Some(s)).ok(), Some(s), "{s:?}");
        }
    }
}
