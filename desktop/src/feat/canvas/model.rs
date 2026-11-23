//! Canvas: the manifest as the daemon sends it, the surface's scene, the
//! view's data, the texts of a send, and the stand-in's routes.

use super::State;
use crate::feat::monitor::model::{who, Group};
use crate::feat::files::view::{ago, now_secs, size_label};
use crate::feat::monitor::attach::{clipped, clipped_text};
use crate::feat::Standin;
use crate::surface_canvas::diagram::{self as dg, Diagram, History};
use crate::surface_canvas::{Body, Item, Pin, Scene, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Shot {
    pub id: String,
    pub file: String,
    pub round: u32,
    pub title: String,
    pub note: String,
    pub width: Option<u32>,
    pub screen: Option<String>,
    pub px_w: u32,
    pub px_h: u32,
    pub bytes: u64,
    pub added_ms: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Moved by the person (the daemon's flag): its round's band ignores it.
    pub moved: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Asset {
    pub id: String,
    pub n: u32,
    pub file: String,
    pub name: String,
    pub px_w: u32,
    pub px_h: u32,
    pub bytes: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub sent: Option<u32>,
    pub sandbox_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct PinM {
    pub n: u32,
    pub on: String,
    pub fx: f32,
    pub fy: f32,
    pub text: String,
    pub sent: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct NoteM {
    pub id: String,
    pub n: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub sent: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Round {
    pub n: u32,
    pub at_ms: u64,
    pub shots: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Review {
    pub n: u32,
    pub at_ms: u64,
    pub session: String,
    pub pins: u32,
    pub notes: u32,
    pub assets: u32,
    pub read_ms: Option<u64>,
}

/// A diagram on the canvas, as the manifest holds it: `x, y` is where the
/// diagram's own (0, 0) sits on the surface; `w, h` its content's far corner.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct DiagramItem {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub round: u32,
    pub added_ms: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub moved: bool,
    pub bytes: u64,
    pub diagram: Diagram,
}

/// The person's editor of one diagram.
#[derive(Debug, Clone, Default)]
pub struct DState {
    /// The version shown and edited.
    pub cur: Diagram,
    /// The revision the daemon holds: the next save is made from it.
    pub rev: u32,
    pub hist: History,
    /// One save in flight; a change meanwhile marks it dirty and the answer sends the rest.
    pub saving: bool,
    pub dirty: bool,
    /// "Lay out again" in flight: the daemon's answer (its layout) is taken whole.
    pub relayout: bool,
    /// The daemon's newer version, when a save was refused as stale (409):
    /// shown with Reload and Reapply my change; nothing is saved until one is chosen.
    pub conflict: Option<Diagram>,
    /// Why the last save failed (any other refusal).
    pub error: Option<String>,
}

/// `GET /api/canvas/:id`'s `canvas`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub sandbox: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub shots: Vec<Shot>,
    pub assets: Vec<Asset>,
    pub pins: Vec<PinM>,
    pub notes: Vec<NoteM>,
    pub rounds: Vec<Round>,
    pub reviews: Vec<Review>,
    pub diagrams: Vec<DiagramItem>,
}

/// An unsent pin (the person's draft), as the edit route takes it.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct PinD {
    pub n: u32,
    pub on: String,
    pub fx: f32,
    pub fy: f32,
    pub text: String,
}

impl From<&PinM> for PinD {
    fn from(p: &PinM) -> PinD {
        PinD { n: p.n, on: p.on.clone(), fx: p.fx, fy: p.fy, text: p.text.clone() }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct NoteD {
    pub id: String,
    pub n: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
}

impl From<&NoteM> for NoteD {
    fn from(n: &NoteM) -> NoteD {
        NoteD { id: n.id.clone(), n: n.n, x: n.x, y: n.y, w: n.w, h: n.h, text: n.text.clone() }
    }
}

/// Agent or person text drawn on one line: control and direction marks as `?`.
pub fn plain(s: &str) -> String {
    clipped_text(s, 200).replace('\n', " ")
}

/// A shot's or asset's rectangle, the person's move applied.
pub fn item_rect(c: &Manifest, id: &str, layout: &BTreeMap<String, [f32; 4]>) -> Option<[f32; 4]> {
    if let Some(r) = layout.get(id) {
        return Some(*r);
    }
    c.shots.iter().find(|s| s.id == id).map(|s| [s.x, s.y, s.w, s.h]).or_else(|| c.assets.iter().find(|a| a.id == id).map(|a| [a.x, a.y, a.w, a.h]))
}

/// A diagram's item on the surface: (the rectangle of its content, the
/// content box's corner in the diagram's points). The person's move applied,
/// and their edits (`local`) drawn.
pub fn diagram_rect(c: &Manifest, id: &str, layout: &BTreeMap<String, [f32; 4]>, local: &BTreeMap<String, DState>) -> Option<([f32; 4], [f32; 2])> {
    let it = c.diagrams.iter().find(|d| d.id == id)?;
    let d = local.get(id).map(|s| &s.cur).unwrap_or(&it.diagram);
    let (ox, oy) = layout.get(id).map(|r| (r[0], r[1])).unwrap_or((it.x, it.y));
    let b = dg::content_box(d);
    Some(([ox + b[0], oy + b[1], b[2], b[3]], [b[0], b[1]]))
}

/// Pins are numbered per canvas: after every pin, sent or not.
pub fn next_pin(c: &Manifest, draft: &[PinD]) -> u32 {
    c.pins.iter().map(|p| p.n).chain(draft.iter().map(|p| p.n)).max().unwrap_or(0) + 1
}

pub fn next_note(c: &Manifest, draft: &[NoteD]) -> u32 {
    c.notes.iter().map(|p| p.n).chain(draft.iter().map(|p| p.n)).max().unwrap_or(0) + 1
}

pub fn ext_of(file: &str) -> &str {
    match file.rsplit('.').next() {
        Some("jpg") => "jpg",
        Some("webp") => "webp",
        _ => "png",
    }
}

/// The caption under a shot: its title, then round, width and note.
fn caption(s: &Shot) -> Vec<String> {
    let mut meta = format!("Round {}", s.round.max(1));
    if let Some(w) = s.width {
        meta.push_str(&format!(" · {w} wide"));
    }
    if !s.note.trim().is_empty() {
        meta.push_str(&format!(" · {}", plain(&s.note)));
    }
    vec![plain(&s.title), meta]
}

/// The surface's scene: shots, a band above each round's row, assets,
/// notes (sent and draft), pins (sent ones muted).
pub fn scene(c: &Manifest, pins: &[PinD], notes: &[NoteD], layout: &BTreeMap<String, [f32; 4]>, local: &BTreeMap<String, DState>, selected: Option<&str>, tool: Tool) -> Scene {
    let mut items = vec![];
    // Diagrams under everything else (a shot moved onto one stays seen).
    for it in &c.diagrams {
        let Some((r, off)) = diagram_rect(c, &it.id, layout, local) else { continue };
        let d = local.get(&it.id).map(|s| s.cur.clone()).unwrap_or_else(|| it.diagram.clone());
        items.push(Item { id: it.id.clone(), x: r[0], y: r[1], w: r[2], h: r[3], body: Body::Diagram { diagram: std::sync::Arc::new(d), off, title: plain(&it.title) }, movable: true });
    }
    let mut rounds: BTreeMap<u32, [f32; 3]> = BTreeMap::new();
    for s in &c.shots {
        let r = item_rect(c, &s.id, layout).unwrap_or([s.x, s.y, s.w, s.h]);
        // Bands sit above each round's grid: a shot the person moved (now
        // or before) does not move or stretch it, unless all were moved.
        let round_moved = c.shots.iter().filter(|o| o.round == s.round).all(|o| o.moved || layout.contains_key(&o.id));
        if !(s.moved || layout.contains_key(&s.id)) || round_moved {
            let e = rounds.entry(s.round).or_insert([s.x, s.y, s.x + s.w]);
            e[0] = e[0].min(s.x);
            e[1] = e[1].min(s.y);
            e[2] = e[2].max(s.x + s.w);
        }
        items.push(Item { id: s.id.clone(), x: r[0], y: r[1], w: r[2], h: r[3], body: Body::Picture { image: s.file.clone(), caption: caption(s), asset: false }, movable: true });
    }
    for (n, [x0, y0, x1]) in &rounds {
        let round = c.rounds.iter().find(|r| r.n == *n);
        let when = round.map(|r| ago(r.at_ms / 1000, now_secs())).unwrap_or_default();
        let count = c.shots.iter().filter(|s| s.round == *n).count();
        let label = format!("Round {n} · {count} shot{} · {when}", if count == 1 { "" } else { "s" });
        items.insert(0, Item { id: format!("band-{n}"), x: *x0, y: *y0, w: (x1 - x0).max(400.0), h: 0.0, body: Body::Band { label }, movable: false });
    }
    for a in &c.assets {
        let r = item_rect(c, &a.id, layout).unwrap_or([a.x, a.y, a.w, a.h]);
        let sent = a.sent.map(|n| format!(" · sent in review {n}")).unwrap_or_default();
        items.push(Item { id: a.id.clone(), x: r[0], y: r[1], w: r[2], h: r[3], body: Body::Picture { image: a.file.clone(), caption: vec![format!("Example {}", a.n), format!("{}{sent}", clipped(&a.name))], asset: true }, movable: true });
    }
    for n in c.notes.iter().filter(|n| n.sent.is_some()) {
        let r = layout.get(&n.id).copied().unwrap_or([n.x, n.y, n.w, n.h]);
        items.push(Item { id: n.id.clone(), x: r[0], y: r[1], w: r[2], h: r[3], body: Body::Note { text: n.text.clone(), n: Some(n.n) }, movable: true });
    }
    for n in notes {
        items.push(Item { id: n.id.clone(), x: n.x, y: n.y, w: n.w, h: n.h, body: Body::Note { text: n.text.clone(), n: Some(n.n) }, movable: true });
    }
    let mut ps: Vec<Pin> = c.pins.iter().filter(|p| p.sent.is_some()).map(|p| Pin { n: p.n, item: p.on.clone(), fx: p.fx, fy: p.fy, sent: true }).collect();
    ps.extend(pins.iter().map(|p| Pin { n: p.n, item: p.on.clone(), fx: p.fx, fy: p.fy, sent: false }));
    Scene { items, pins: ps, selected: selected.map(str::to_string), tool, ..Scene::default() }
}

/// Whether the session surely has `canvas_read`: it started after a shot of
/// this canvas was added (the relay tools were installed by then).
pub fn session_has_tool(c: &Manifest, info: &J) -> bool {
    let first = c.shots.iter().map(|s| s.added_ms).filter(|t| *t > 0).min();
    match (first, info["started_ms"].as_u64()) {
        (Some(f), Some(s)) => s >= f,
        _ => false,
    }
}

fn on_label(c: &Manifest, on: &str) -> String {
    match c.shots.iter().find(|s| s.id == on) {
        Some(s) => format!("shot \"{}\"", plain(&s.title)),
        None => c.assets.iter().find(|a| a.id == on).map(|a| format!("example {}", a.n)).unwrap_or_else(|| "a removed item".into()),
    }
}

/// Every remark of a send, in full, for the dialog.
pub fn remarks_text(c: &Manifest, pins: &[PinD], notes: &[NoteD], assets: &[&Asset]) -> String {
    let mut out = vec![];
    for p in pins {
        out.push(format!("Pin {} on {}, {} ({:.0}% across, {:.0}% down):\n{}", p.n, on_label(c, &p.on), hint(p.fx, p.fy), p.fx * 100.0, p.fy * 100.0, if p.text.trim().is_empty() { "(no text)" } else { p.text.trim() }));
    }
    for n in notes {
        out.push(format!("Note {}:\n{}", n.n, if n.text.trim().is_empty() { "(no text)" } else { n.text.trim() }));
    }
    for a in assets {
        out.push(format!("Example {}: {} ({}x{} px)", a.n, crate::feat::monitor::attach::written_out(&a.name), a.px_w, a.px_h));
    }
    out.join("\n\n")
}

/// The numbered list typed for a session without `canvas_read`.
pub fn fallback_list(c: &Manifest, pins: &[PinD], notes: &[NoteD], max: usize) -> String {
    let mut out = vec![];
    let cut = |t: &str| clipped_text(t.trim(), max).replace('\n', " ");
    for p in pins {
        out.push(format!("{}. Pin {} on {} ({}): {}", out.len() + 1, p.n, on_label(c, &p.on), hint(p.fx, p.fy), cut(&p.text)));
    }
    for n in notes {
        out.push(format!("{}. Note {}: {}", out.len() + 1, n.n, cut(&n.text)));
    }
    out.join("\n")
}

pub fn hint(fx: f32, fy: f32) -> &'static str {
    let col = if fx < 1.0 / 3.0 { 0 } else if fx < 2.0 / 3.0 { 1 } else { 2 };
    let row = if fy < 1.0 / 3.0 { 0 } else if fy < 2.0 / 3.0 { 1 } else { 2 };
    [["top-left", "top", "top-right"], ["left", "middle", "right"], ["bottom-left", "bottom", "bottom-right"]][row][col]
}

/// A picture the person handed in, read here: a file of at most 20 MB whose
/// first bytes say PNG, JPEG or WebP and whose header fits 8192 px a side.
pub fn read_asset(p: &std::path::Path) -> Result<Vec<u8>, String> {
    let m = std::fs::metadata(p).map_err(|e| format!("a file could not be read ({e})"))?;
    if !m.is_file() {
        return Err("a folder: add a picture file instead".into());
    }
    if m.len() > crate::surface_canvas::limits().max_bytes as u64 {
        return Err(format!("the picture is {}, more than 20 MB", size_label(m.len())));
    }
    let b = std::fs::read(p).map_err(|e| format!("a file could not be read ({e})"))?;
    crate::picture::sniff(&b).ok_or("not a PNG, JPEG or WebP picture")?;
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&b)).with_guessed_format().map_err(|e| e.to_string())?.into_dimensions().map_err(|e| format!("the picture could not be read: {e}"))?;
    if w > 8192 || h > 8192 || w == 0 || h == 0 {
        return Err(format!("the picture is {w}x{h} pixels; 8192 a side at most"));
    }
    Ok(b)
}

// ---------------------------------------------------------------------------
// The view's data

fn when(ms: u64) -> String {
    ago(ms / 1000, now_secs())
}

/// `canvas_ui`: what the screen draws (it computes nothing of its own).
pub fn view(s: &State, snap: &J) -> J {
    let sandboxes: Vec<String> = snap["sandboxes"].as_array().into_iter().flatten().filter_map(|x| x["name"].as_str().map(str::to_string)).collect();
    let mut sb_items: Vec<J> = sandboxes.iter().map(|n| json!({ "id": n, "label": n })).collect();
    // Sandboxes removed since: their canvases stay, listed under their name.
    let mut gone: Vec<String> = s.list.iter().filter_map(|c| c["sandbox"].as_str()).filter(|n| !sandboxes.iter().any(|x| x == n)).map(str::to_string).collect();
    gone.sort();
    gone.dedup();
    for g in &gone {
        sb_items.push(json!({ "id": g, "label": format!("{g} (removed)") }));
    }
    let sb = s.sandbox.clone().unwrap_or_default();
    let mut canvases: Vec<J> = s
        .list
        .iter()
        .filter(|c| c["sandbox"] == sb.as_str())
        .map(|c| {
            let bytes = c["bytes"].as_u64().unwrap_or(0);
            json!({ "id": c["id"], "label": format!("{} · {} · {} shot(s) · {}", plain(c["title"].as_str().unwrap_or("")), when(c["updated_ms"].as_u64().unwrap_or(0)), c["shots"], size_label(bytes)),
                "help": format!("{} review round(s), {} remark(s) not sent", c["reviews"], c["draft"]) })
        })
        .collect();
    canvases.push(json!({ "divider": true }));
    canvases.push(json!({ "id": "new", "label": "New canvas", "icon": "plus" }));
    // The scope picker offers only the sandboxes that exist: a removed
    // one's canvases are reached from here.
    let elsewhere: Vec<&String> = gone.iter().filter(|g| **g != sb).collect();
    if !elsewhere.is_empty() {
        canvases.push(json!({ "divider": true }));
        for g in elsewhere {
            canvases.push(json!({ "id": format!("sb:{g}"), "label": format!("Canvases of {g} (removed)"), "icon": "canvas" }));
        }
    }
    let cur = s.list.iter().find(|c| Some(c["id"].as_str().unwrap_or("")) == s.current.as_deref());
    let storage = format!(
        "This canvas {} · all canvases {} ({})",
        size_label(cur.and_then(|c| c["bytes"].as_u64()).unwrap_or(0)),
        size_label(s.total),
        s.list.len()
    );
    // The storage line in the picker too (not pressable).
    canvases.push(json!({ "divider": true }));
    canvases.push(json!({ "id": "storage", "label": format!("Storage: {storage}"), "disabled": true, "help": "Delete a canvas, or every canvas older than a date, from Delete" }));
    let mut v = json!({
        "sandboxes": sb_items, "sandbox": sb, "removed": gone.contains(&sb), "canvases": canvases, "storage": storage, "count": s.list.len(),
        "tool": s.tool.name(), "selected": s.selected, "renaming": s.renaming, "loading": s.loading, "error": s.error,
        "tab": if s.tab.is_empty() { "canvas" } else { s.tab.as_str() },
        "ask": { "text": s.ask_text.clone().unwrap_or_else(|| super::ASK_PROMPT.to_string()) },
        // The bar's ground menu: the current choice carries the check.
        "ground": s.ground.name(), "ground_label": s.ground.label(),
        "grounds": crate::surface_canvas::Ground::ALL.iter().map(|g| if *g == s.ground { json!({ "id": g.name(), "label": g.label(), "icon": "check" }) } else { json!({ "id": g.name(), "label": g.label() }) }).collect::<Vec<_>>(),
        "popup": s.tool != Tool::Select, "zoom": format!("{:.0}%", crate::surface_canvas::camera(super::SURF).zoom * 100.0),
        // In its own window (crate::windows): its width lays the view out.
        "windowed": crate::windows::canvas_open(), "win_w": crate::windows::canvas_width(),
    });
    let Some(c) = &s.canvas else {
        v["empty"] = if s.current.is_none() { "none".into() } else { "loading".into() };
        v["send"] = send_view(s, snap, 0);
        return v;
    };
    v["current"] = json!({ "id": c.id, "title": plain(&c.title), "shots": c.shots.len(), "rounds": c.rounds.len(), "reviews": c.reviews.len() });
    if c.shots.is_empty() && c.diagrams.is_empty() {
        v["empty"] = "shots".into();
    }
    // The draft: pins, notes, assets not sent yet.
    let sel = s.selected.as_deref();
    let mut draft = vec![];
    for p in &s.pins {
        let key = format!("pin-{}", p.n);
        draft.push(json!({ "key": key, "kind": "pin", "n": p.n, "label": format!("Pin {} · {} · {}", p.n, on_label(c, &p.on), hint(p.fx, p.fy)), "text": p.text, "selected": sel == Some(key.as_str()) }));
    }
    for n in &s.notes {
        draft.push(json!({ "key": n.id, "kind": "note", "n": n.n, "label": format!("Note {}", n.n), "text": n.text, "selected": sel == Some(n.id.as_str()) }));
    }
    for a in c.assets.iter().filter(|a| a.sent.is_none() && !s.remove.contains(&a.id)) {
        let look = s.handed.get(&a.id).map(|h| !h.look.is_empty()).unwrap_or(false);
        draft.push(json!({ "key": a.id, "kind": "asset", "n": a.n, "label": format!("Example {} · {} · {}", a.n, clipped(&a.name), size_label(a.bytes)), "look": look, "selected": sel == Some(a.id.as_str()) }));
    }
    let new = draft.len();
    v["draft"] = J::Array(draft);
    // Past rounds, newest first, read only.
    let mut history = vec![];
    for r in c.reviews.iter().rev() {
        let mut items = vec![];
        for p in c.pins.iter().filter(|p| p.sent == Some(r.n)) {
            items.push(json!({ "key": format!("pin-{}", p.n), "label": format!("Pin {} · {}", p.n, on_label(c, &p.on)), "text": clipped_text(&p.text, 2000) }));
        }
        for n in c.notes.iter().filter(|n| n.sent == Some(r.n)) {
            items.push(json!({ "key": n.id, "label": format!("Note {}", n.n), "text": clipped_text(&n.text, 2000) }));
        }
        for a in c.assets.iter().filter(|a| a.sent == Some(r.n)) {
            items.push(json!({ "key": a.id, "label": format!("Example {} · {}", a.n, clipped(&a.name)), "text": a.sandbox_path.clone().unwrap_or_default() }));
        }
        let read = if r.read_ms.is_some() { "read by the agent" } else { "not read yet" };
        history.push(json!({ "n": r.n, "line": format!("Review {} · {} · to {} · {}", r.n, when(r.at_ms), plain(&r.session), read), "items": items }));
    }
    v["history"] = J::Array(history);
    v["send"] = send_view(s, snap, new);
    v
}

/// The sessions of `sb` a send or an ask may go to, one per place as the
/// Sessions view folds them (`monitor::model::Model`: the newest report of
/// a place wins, two live sessions in one main checkout are two places):
/// `(key, label, info)`, main first. Ended sessions are not offered.
pub fn places(snap: &J, sb: &str) -> Vec<(String, String, J)> {
    let mine: Vec<J> = snap["sessions"].as_array().into_iter().flatten().filter(|i| i["sandbox"] == sb).cloned().collect();
    let mut m = crate::feat::monitor::model::Model::default();
    m.seed(&J::Array(mine));
    let cards: Vec<(String, J)> = m.cards.iter().filter(|(_, c)| Group::of(c.info["state"].as_str().unwrap_or("")) != Group::Ended).map(|(k, c)| (k.clone(), c.info.clone())).collect();
    let mains = cards.iter().filter(|(_, i)| thread_of(i).is_none()).count();
    let mut v: Vec<(String, String, J)> = cards
        .into_iter()
        .map(|(k, i)| {
            let label = match thread_of(&i) {
                Some(t) => who(sb, Some(t)),
                None if mains > 1 => {
                    let sid: Vec<char> = i["session_id"].as_str().unwrap_or("").chars().collect();
                    format!("{} · {}", who(sb, None), sid[sid.len().saturating_sub(4)..].iter().collect::<String>())
                }
                None => who(sb, None),
            };
            (k, label, i)
        })
        .collect();
    v.sort_by_key(|(_, l, i)| (thread_of(i).is_some(), std::cmp::Reverse(i["ts"].as_u64().unwrap_or(0)), l.clone()));
    v
}

fn thread_of(i: &J) -> Option<&str> {
    i["thread"].as_str().filter(|t| !t.is_empty())
}

/// Why one session cannot take typed words, if it cannot.
fn session_refusal(label: &str, info: &J) -> Option<String> {
    match Group::of(info["state"].as_str().unwrap_or("")) {
        Group::You => return Some(format!("{label} waits on a question or a permission; answer that first")),
        Group::Ended => return Some(format!("{label} has ended")),
        _ => {}
    }
    // The daemon's reason, worded as the Sessions composer words it.
    crate::feat::monitor::model::unreachable_line(info).map(|w| format!("{label}: {w}"))
}

/// Where a send or an ask goes, or why it cannot, and the sessions to pick
/// from. ONE verdict for the button (the view) and the press (`target`), so
/// the two never disagree.
pub struct Verdict {
    pub places: Vec<(String, String, J)>,
    /// The place chosen: the one picked, else the main session that can be
    /// asked, else the first.
    pub key: Option<String>,
    pub label: Option<String>,
    pub info: J,
    /// Why not, in plain words that say what to do (no "Not sent" prefix).
    pub why: Option<String>,
}

/// `snap`: `{sessions, sandboxes, covered, jarvis}` (the app's data).
/// `held`: also refuse while something is over the canvas or a Jarvis
/// proposal waits (the button; a press checks those first itself).
pub fn verdict(snap: &J, sandbox: Option<&str>, picked: Option<&str>, held: bool) -> Verdict {
    let sb = sandbox.unwrap_or("");
    let places = if sb.is_empty() { vec![] } else { places(snap, sb) };
    let fine = |(_, l, i): &(String, String, J)| session_refusal(l, i).is_none();
    let key = picked
        .filter(|k| places.iter().any(|(x, _, _)| x == k))
        .map(str::to_string)
        // Not picked: the main checkout's session that can be asked (a stale
        // one waiting on an old question beside it is not chosen), else the
        // first. Never a thread on its own: that is the person's pick.
        .or_else(|| places.iter().find(|p| thread_of(&p.2).is_none() && fine(p)).or(places.first()).map(|(k, _, _)| k.clone()));
    let chosen = key.as_ref().and_then(|k| places.iter().find(|(x, _, _)| x == k));
    let label = chosen.map(|(_, l, _)| l.clone());
    let info = chosen.map(|(_, _, i)| i.clone()).unwrap_or(J::Null);
    let running = snap["sandboxes"].as_array().into_iter().flatten().any(|x| x["name"] == sb && x["status"] == "running");
    let why = if held && snap["covered"] != false && !snap["covered"].is_null() {
        Some("Something is open over the canvas (a dialog or a menu); close it first".to_string())
    } else if held && snap["jarvis"].as_array().is_some_and(|p| !p.is_empty()) {
        Some("A Jarvis proposal waits for you; approve or refuse it first".to_string())
    } else if sb.is_empty() {
        Some("No sandbox is shown; pick one first".to_string())
    } else if !running {
        Some(format!("{sb} is not running; start it first"))
    } else if let Some((_, l, i)) = chosen {
        session_refusal(l, i).map(|w| {
            let others: Vec<&str> = places.iter().filter(|p| p.0 != *key.as_deref().unwrap_or("") && fine(p)).map(|(_, l, _)| l.as_str()).collect();
            if others.is_empty() { w } else { format!("{w}. Or pick {} instead", others.join(" or ")) }
        })
    } else {
        Some(format!("{sb} has no open agent session; open one first"))
    };
    Verdict { places, key, label, info, why }
}

fn send_view(s: &State, snap: &J, new: usize) -> J {
    let sb = s.sandbox.clone().unwrap_or_default();
    let v = verdict(snap, s.sandbox.as_deref(), s.session.as_deref(), true);
    let busy = s.job.as_ref().is_some_and(|j| j.sent);
    let to = v.label.clone().unwrap_or_else(|| format!("{sb} · no session"));
    let why = v.why.as_ref().map(|w| format!("{w}."));
    json!({
        "label": format!("Send remarks to {to}"), "to": to, "new": new,
        "ok": new > 0 && why.is_none() && !busy, "ask_ok": why.is_none(),
        "why": if busy { Some("Sending…".to_string()) } else if new == 0 { Some("No new pin, note or example since the last round".to_string()) } else { why.clone() },
        // The Ask button's own reason: independent of what is new.
        "ask_why": why,
        "sessions": v.places.iter().map(|(k, l, _)| json!({ "id": k, "label": l })).collect::<Vec<_>>(), "session": v.key,
    })
}

// ---------------------------------------------------------------------------
// The stand-in daemon (demos, tests)

/// A drawn mock screen: a header bar, a side bar on a wide one, cards with
/// lines. Neutral, invented; no real project.
pub fn mock_png(w: u32, h: u32, seed: u32) -> Vec<u8> {
    mock_png_of(w, h, seed, false)
}

/// [`mock_png`], light or dark (an app in its dark theme, close to the surface's ground).
pub fn mock_png_of(w: u32, h: u32, seed: u32, dark: bool) -> Vec<u8> {
    let hue = [(52u8, 120u8, 200u8), (40, 160, 110), (180, 90, 60), (120, 80, 170)][(seed % 4) as usize];
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let bar = y < (h / 14).max(24);
        let side = w > 600 && x < w / 6;
        let (cx, cy) = ((x as i32 - if side { 0 } else { (w / 6) as i32 }).rem_euclid(300), (y as i32 - (h / 7) as i32).rem_euclid(220));
        let card = !side && !bar && (w <= 600 || x >= w / 6) && cx > 20 && cx < 280 && cy > 20 && cy < 200;
        let line = card && (cy - 40).rem_euclid(24) < 6 && cx < 220;
        let px = if bar {
            if dark { [hue.0 / 3, hue.1 / 3, hue.2 / 3] } else { [hue.0, hue.1, hue.2] }
        } else if side {
            if dark { [18, 20, 24] } else { [36, 40, 48] }
        } else if line {
            if dark { [70, 76, 86] } else { [150, 155, 165] }
        } else if card {
            if dark { [30, 34, 40] } else { [250, 250, 252] }
        } else if dark {
            [16, 19, 24]
        } else {
            [232, 234, 238]
        };
        image::Rgba([px[0], px[1], px[2], 255])
    });
    let mut b = vec![];
    let _ = image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png);
    b
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn summary(c: &J, changed: &J) -> J {
    let bytes: u64 = c["shots"].as_array().into_iter().flatten().chain(c["assets"].as_array().into_iter().flatten()).map(|x| x["bytes"].as_u64().unwrap_or(0)).sum();
    let draft = c["pins"].as_array().into_iter().flatten().filter(|p| p["sent"].is_null()).count() + c["notes"].as_array().into_iter().flatten().filter(|p| p["sent"].is_null()).count();
    let diagrams = c["diagrams"].as_array().map_or(0, Vec::len);
    let draft_diagrams = c["diagrams"].as_array().into_iter().flatten().filter(|d| changed[d["id"].as_str().unwrap_or("")] == true).count();
    json!({ "id": c["id"], "sandbox": c["sandbox"], "title": c["title"], "created_ms": c["created_ms"], "updated_ms": c["updated_ms"],
        "shots": c["shots"].as_array().map_or(0, Vec::len), "rounds": c["rounds"].as_array().map_or(0, Vec::len), "reviews": c["reviews"].as_array().map_or(0, Vec::len), "bytes": bytes, "draft": draft,
        "diagrams": diagrams, "draft_diagrams": draft_diagrams })
}

/// The stand-in's layout of the nodes without a place (the daemon runs the
/// core's layered layout; this is a plain one, for demos and tests):
/// lifelines left to right in a sequence; else ranks by longest path from
/// the nodes nothing leads to, one column a rank, lanes as rows.
pub fn standin_layout(d: &mut Diagram) {
    use dg::{Kind, Placed};
    if d.nodes.iter().all(|n| n.placed != Placed::None) {
        return;
    }
    if d.kind == Kind::Sequence {
        let mut x = 0.0;
        for n in &mut d.nodes {
            (n.x, n.y, n.placed) = (x, 0.0, Placed::Layout);
            x += n.w + dg::SEQ_GAP;
        }
        return;
    }
    let ids: Vec<String> = d.nodes.iter().map(|n| n.id.clone()).collect();
    let mut rank = vec![0usize; ids.len()];
    for _ in 0..ids.len() {
        for e in &d.edges {
            let (Some(a), Some(b)) = (ids.iter().position(|i| *i == e.from), ids.iter().position(|i| *i == e.to)) else { continue };
            if a != b && rank[b] < rank[a] + 1 && rank[a] + 1 < ids.len() {
                rank[b] = rank[a] + 1;
            }
        }
    }
    let lanes: Vec<String> = d.lanes.iter().map(|l| l.id.clone()).collect();
    let mut rows: BTreeMap<(usize, usize), f32> = BTreeMap::new();
    let lane_h = 140.0;
    for (k, n) in d.nodes.iter_mut().enumerate() {
        let lane = n.lane.as_ref().and_then(|l| lanes.iter().position(|x| x == l)).unwrap_or(0);
        let at = rows.entry((lane, rank[k])).or_insert(0.0);
        (n.x, n.y, n.placed) = (dg::LANE_TITLE + rank[k] as f32 * 240.0, lane as f32 * lane_h + dg::LANE_PAD + *at, Placed::Layout);
        *at += n.h + dg::NODE_GAP;
    }
    dg::fit_lanes(d);
}

/// The core's column rule (`sbxw_core::canvas::grid_cols`), for the stand-in:
/// the count whose block is nearest 1.6 wide, an empty cell costing a little.
pub fn grid_cols(cells: usize, cw: f64, ch: f64) -> usize {
    let mut best = (f64::MAX, 1);
    for c in 1..=cells.clamp(1, 8) {
        let rows = cells.div_ceil(c).max(1);
        let (w, h) = (c as f64 * cw + (c - 1) as f64 * 120.0, rows as f64 * ch + (rows - 1) as f64 * 200.0);
        let cost = (w / h / 1.6).ln().abs() + 0.05 * (rows * c - cells) as f64;
        if cost < best.0 - 1e-4 {
            best = (cost, c);
        }
    }
    best.1
}

/// The stand-in's canvas routes over the fixture's `canvas` object
/// (`{canvases: {id: manifest}, pictures: {file: {w, h, seed}}}`).
pub fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    use crate::feat::match_path;
    if !path.starts_with("/api/canvas") {
        return None;
    }
    let p = path.split('?').next().unwrap_or(path);
    let now = now_secs() * 1000;
    let data = &mut st.data["canvas"];
    if data.is_null() {
        *data = json!({ "canvases": {}, "pictures": {} });
    }
    if data["changed"].is_null() {
        data["changed"] = json!({});
    }
    if let Some(c) = match_path("/api/canvas/{id}/diagrams/{did}", p) {
        let (id, did) = (c[0].1.clone(), c[1].1.clone());
        let Some(k) = data["canvases"][id.as_str()]["diagrams"].as_array().and_then(|a| a.iter().position(|d| d["id"] == did.as_str())) else {
            return Some((404, json!({ "ok": false, "error": "no such diagram" })));
        };
        if method == "GET" {
            return Some((200, json!({ "ok": true, "diagram": data["canvases"][id.as_str()]["diagrams"][k] })));
        }
        // The daemon's save: the revision checked, the whole diagram checked
        // again, unplaced nodes laid out, the revision bumped.
        let item = &mut data["canvases"][id.as_str()]["diagrams"][k];
        let cur = item["diagram"]["rev"].as_u64().unwrap_or(0);
        if body["rev"].as_u64() != Some(cur) {
            return Some((409, json!({ "ok": false, "error": format!("the diagram changed since revision {} (now {cur}): read it again", body["rev"]), "rev": cur })));
        }
        let mut d: Diagram = match serde_json::from_value(body["diagram"].clone()) {
            Ok(d) => d,
            Err(e) => return Some((400, json!({ "ok": false, "error": format!("not a diagram: {e}") }))),
        };
        if let Err(e) = dg::check(&d) {
            return Some((400, json!({ "ok": false, "error": e })));
        }
        standin_layout(&mut d);
        (d.id, d.rev, d.author, d.updated_ms) = (did.clone(), cur as u32 + 1, "person".into(), now);
        let b = dg::content_box(&d);
        item["w"] = (b[0] + b[2]).ceil().into();
        item["h"] = (b[1] + b[3]).ceil().into();
        item["diagram"] = dg::wire(&d);
        // Laid-out nodes come back with their places.
        let saved = item.clone();
        data["changed"][did.as_str()] = true.into();
        data["canvases"][id.as_str()]["updated_ms"] = now.into();
        let sb = data["canvases"][id.as_str()]["sandbox"].clone();
        (st.publish)("canvas", json!({ "id": id, "sandbox": sb, "what": "edit" }));
        return Some((200, json!({ "ok": true, "diagram": saved, "rev": cur + 1 })));
    }
    if method == "GET" && p == "/api/canvas" {
        let changed = data["changed"].clone();
        let mut l: Vec<J> = data["canvases"].as_object().into_iter().flatten().map(|(_, c)| summary(c, &changed)).collect();
        l.sort_by_key(|c| std::cmp::Reverse(c["updated_ms"].as_u64().unwrap_or(0)));
        let total: u64 = l.iter().map(|c| c["bytes"].as_u64().unwrap_or(0)).sum();
        return Some((200, json!({ "ok": true, "canvases": l, "total_bytes": total })));
    }
    if method == "POST" && p == "/api/canvas" {
        let id = format!("c-{:016x}", data["canvases"].as_object().map_or(0, |m| m.len()) + 0xaa00);
        let c = json!({ "id": id, "sandbox": body["sandbox"], "title": body["title"].as_str().unwrap_or("Review"), "created_ms": now, "updated_ms": now, "shots": [], "assets": [], "pins": [], "notes": [], "rounds": [], "reviews": [] });
        data["canvases"][id.as_str()] = c.clone();
        (st.publish)("canvas", json!({ "id": id, "sandbox": body["sandbox"], "what": "created" }));
        return Some((200, json!({ "ok": true, "canvas": c })));
    }
    if method == "POST" && p == "/api/canvas/delete" {
        let mut n = 0;
        let mut bytes = 0;
        for id in body["ids"].as_array().into_iter().flatten().filter_map(|i| i.as_str()) {
            if let Some(c) = data["canvases"].as_object_mut().and_then(|m| m.remove(id)) {
                n += 1;
                bytes += summary(&c, &J::Null)["bytes"].as_u64().unwrap_or(0);
                (st.publish)("canvas", json!({ "id": id, "sandbox": c["sandbox"], "what": "deleted" }));
            }
        }
        return Some((200, json!({ "ok": true, "deleted": n, "bytes": bytes })));
    }
    if method == "GET" {
        if let Some(c) = match_path("/api/canvas/{id}/images/{file}", p) {
            let f = &data["pictures"][c[1].1.as_str()];
            if f.is_null() {
                return Some((404, json!({ "ok": false, "error": "no such picture" })));
            }
            let png = mock_png_of(f["w"].as_u64().unwrap_or(800) as u32, f["h"].as_u64().unwrap_or(600) as u32, f["seed"].as_u64().unwrap_or(0) as u32, f["dark"].as_bool().unwrap_or(false));
            return Some((200, json!({ "$raw_hex": hex(&png), "$type": "image/png" })));
        }
        if let Some(c) = match_path("/api/canvas/{id}", p) {
            let m = data["canvases"][c[0].1.as_str()].clone();
            if m.is_null() {
                return Some((404, json!({ "ok": false, "error": "no such canvas" })));
            }
            return Some((200, json!({ "ok": true, "canvas": m })));
        }
        return None;
    }
    let c = match_path("/api/canvas/{id}/{what}", p)?;
    let (id, what) = (c[0].1.clone(), c[1].1.clone());
    if data["canvases"][id.as_str()].is_null() {
        return Some((404, json!({ "ok": false, "error": "no such canvas" })));
    }
    let sb = data["canvases"][id.as_str()]["sandbox"].clone();
    let answer = match what.as_str() {
        "rename" => {
            data["canvases"][id.as_str()]["title"] = body["title"].clone();
            json!({ "ok": true, "title": body["title"] })
        }
        "edit" => {
            let cv = &mut data["canvases"][id.as_str()];
            let keep = |k: &str, cv: &J| -> Vec<J> { cv[k].as_array().into_iter().flatten().filter(|x| !x["sent"].is_null()).cloned().collect() };
            if body["pins"].is_array() {
                let mut v = keep("pins", cv);
                v.extend(body["pins"].as_array().cloned().unwrap_or_default());
                cv["pins"] = J::Array(v);
            }
            if body["notes"].is_array() {
                let mut v = keep("notes", cv);
                v.extend(body["notes"].as_array().cloned().unwrap_or_default());
                cv["notes"] = J::Array(v);
            }
            for r in body["remove_assets"].as_array().into_iter().flatten() {
                if let Some(a) = cv["assets"].as_array_mut() {
                    a.retain(|x| x["id"] != *r || !x["sent"].is_null());
                }
            }
            for l in body["layout"].as_array().into_iter().flatten() {
                for k in ["shots", "assets", "notes"] {
                    for x in cv[k].as_array_mut().into_iter().flatten().filter(|x| x["id"] == l["id"]) {
                        for f in ["x", "y", "w", "h"] {
                            x[f] = l[f].clone();
                        }
                    }
                }
                // A diagram moves by its origin only.
                for x in cv["diagrams"].as_array_mut().into_iter().flatten().filter(|x| x["id"] == l["id"]) {
                    (x["x"], x["y"], x["moved"]) = (l["x"].clone(), l["y"].clone(), true.into());
                }
            }
            cv["updated_ms"] = now.into();
            json!({ "ok": true })
        }
        "assets" => {
            let n = data["canvases"][id.as_str()]["assets"].as_array().map_or(0, Vec::len) + 1;
            let file = format!("a-{:012x}.png", 0xa000 + n);
            data["pictures"][file.as_str()] = json!({ "w": 640, "h": 400, "seed": n + 2 });
            let q = |k: &str| crate::feat::query(path, k);
            let a = json!({ "id": format!("a-{n}"), "n": n, "file": file, "name": q("name").unwrap_or_else(|| "picture".into()), "px_w": 640, "px_h": 400, "bytes": body["$bytes"].as_u64().unwrap_or(1000),
                "x": q("x").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0), "y": q("y").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0), "w": 480, "h": 300, "sent": null });
            data["canvases"][id.as_str()]["assets"].as_array_mut()?.push(a.clone());
            json!({ "ok": true, "asset": a })
        }
        "submit" => {
            let cv = &mut data["canvases"][id.as_str()];
            let n = cv["reviews"].as_array().map_or(0, Vec::len) + 1;
            let (mut pins, mut notes, mut assets) = (0, 0, 0);
            for p in cv["pins"].as_array_mut().into_iter().flatten().filter(|p| p["sent"].is_null()) {
                p["sent"] = n.into();
                pins += 1;
            }
            for p in cv["notes"].as_array_mut().into_iter().flatten().filter(|p| p["sent"].is_null()) {
                p["sent"] = n.into();
                notes += 1;
            }
            for a in cv["assets"].as_array_mut().into_iter().flatten().filter(|p| p["sent"].is_null()) {
                a["sent"] = n.into();
                a["sandbox_path"] = body["assets"][a["id"].as_str().unwrap_or("")].clone();
                assets += 1;
            }
            let ids: Vec<String> = cv["diagrams"].as_array().into_iter().flatten().filter_map(|d| d["id"].as_str().map(str::to_string)).collect();
            let diagrams = ids.iter().filter(|d| data["changed"][d.as_str()] == true).count();
            if pins + notes + assets + diagrams == 0 {
                return Some((400, json!({ "ok": false, "error": "there is no new remark to send" })));
            }
            for d in &ids {
                data["changed"][d.as_str()] = false.into();
            }
            let cv = &mut data["canvases"][id.as_str()];
            cv["reviews"].as_array_mut()?.push(json!({ "n": n, "at_ms": now, "session": body["session"], "pins": pins, "notes": notes, "assets": assets, "read_ms": null, "diagrams": diagrams }));
            json!({ "ok": true, "review": n, "line": format!("[sbxw canvas · {id}] review round {n} is ready: run canvas_read to read the person's remarks") })
        }
        // Stand-in only: the agent adds a round of shots (`{shots: [{title, w, h, width, screen}]}`).
        "standin-shots" => {
            let cv = &mut data["canvases"][id.as_str()];
            let round = cv["rounds"].as_array().map_or(0, Vec::len) as u64 + 1;
            let top = cv["shots"].as_array().into_iter().flatten().map(|s| s["y"].as_f64().unwrap_or(0.0) + s["h"].as_f64().unwrap_or(0.0)).fold(-480.0, f64::max) + 480.0;
            // A grid as the daemon lays one (crates/core/src/canvas.rs), for same-sized shots.
            let list: Vec<J> = body["shots"].as_array().cloned().unwrap_or_default();
            let size = |s: &J| {
                let (pw, ph) = (s["w"].as_u64().unwrap_or(1440), s["h"].as_u64().unwrap_or(900));
                let w = s["width"].as_f64().unwrap_or(pw as f64);
                (pw, ph, w, w * ph as f64 / pw as f64)
            };
            let (cw, ch) = list.iter().map(size).fold((1.0f64, 1.0f64), |m, s| (m.0.max(s.2), m.1.max(s.3)));
            let cols = grid_cols(list.len(), cw, ch);
            let mut files = vec![];
            for (k, s) in list.iter().enumerate() {
                let (pw, ph, w, h) = size(s);
                let (x, y) = ((k % cols) as f64 * (cw + 120.0), top + (k / cols) as f64 * (ch + 200.0));
                let file = format!("s-{:06x}{:06x}.png", round, k);
                files.push((file.clone(), json!({ "w": pw, "h": ph, "seed": round as usize + k, "dark": s["dark"] })));
                cv["shots"].as_array_mut()?.push(json!({ "id": format!("s-{round}-{k}"), "file": file, "round": round, "title": s["title"], "note": s["note"], "width": s["width"], "screen": s["screen"], "px_w": pw, "px_h": ph, "bytes": 200_000, "added_ms": now, "x": x, "y": y, "w": w, "h": h }));
            }
            cv["rounds"].as_array_mut()?.push(json!({ "n": round, "at_ms": now, "shots": files.len() }));
            cv["updated_ms"] = now.into();
            for (f, v) in files {
                data["pictures"][f.as_str()] = v;
            }
            json!({ "ok": true })
        }
        // Stand-in only: the agent adds or replaces a diagram (`{diagram: item}`, its id kept).
        "standin-diagram" => {
            let it = body["diagram"].clone();
            let did = it["id"].clone();
            data["changed"][did.as_str().unwrap_or("")] = false.into();
            let cv = &mut data["canvases"][id.as_str()];
            if cv["diagrams"].is_null() {
                cv["diagrams"] = json!([]);
            }
            let list = cv["diagrams"].as_array_mut()?;
            match list.iter().position(|d| d["id"] == did) {
                Some(k) => list[k] = it,
                None => list.push(it),
            }
            cv["updated_ms"] = now.into();
            json!({ "ok": true })
        }
        _ => return None,
    };
    let what_ev = match what.as_str() {
        "standin-diagram" => "diagram",
        "standin-shots" => "shots",
        "submit" => "review",
        w => if w == "assets" { "asset" } else { "edit" },
    };
    (st.publish)("canvas", json!({ "id": id, "sandbox": sb, "what": what_ev }));
    Some((200, answer))
}
