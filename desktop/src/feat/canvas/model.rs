//! Canvas: the manifest as the daemon sends it, the surface's scene, the
//! view's data, the texts of a send, and the stand-in's routes.

use super::State;
use crate::feat::files::view::{ago, now_secs, size_label};
use crate::feat::monitor::attach::{clipped, clipped_text};
use crate::feat::Standin;
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
pub fn scene(c: &Manifest, pins: &[PinD], notes: &[NoteD], layout: &BTreeMap<String, [f32; 4]>, selected: Option<&str>, tool: Tool) -> Scene {
    let mut items = vec![];
    let mut rounds: BTreeMap<u32, [f32; 3]> = BTreeMap::new();
    for s in &c.shots {
        let r = item_rect(c, &s.id, layout).unwrap_or([s.x, s.y, s.w, s.h]);
        // Bands sit at each round's original row (a moved shot does not move its row).
        let e = rounds.entry(s.round).or_insert([s.x, s.y, s.x + s.w]);
        e[0] = e[0].min(s.x);
        e[1] = e[1].min(s.y);
        e[2] = e[2].max(s.x + s.w);
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
        items.push(Item { id: a.id.clone(), x: r[0], y: r[1], w: r[2], h: r[3], body: Body::Picture { image: a.file.clone(), caption: vec![format!("Asset {}", a.n), format!("{}{sent}", clipped(&a.name))], asset: true }, movable: true });
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
    Scene { items, pins: ps, selected: selected.map(str::to_string), tool }
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
        None => c.assets.iter().find(|a| a.id == on).map(|a| format!("asset {}", a.n)).unwrap_or_else(|| "a removed item".into()),
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
        out.push(format!("Asset {}: {} ({}x{} px)", a.n, crate::feat::monitor::attach::written_out(&a.name), a.px_w, a.px_h));
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
    let cur = s.list.iter().find(|c| Some(c["id"].as_str().unwrap_or("")) == s.current.as_deref());
    let storage = format!(
        "This canvas {} · all canvases {} ({})",
        size_label(cur.and_then(|c| c["bytes"].as_u64()).unwrap_or(0)),
        size_label(s.total),
        s.list.len()
    );
    let mut v = json!({
        "sandboxes": sb_items, "sandbox": sb, "removed": gone.contains(&sb), "canvases": canvases, "storage": storage, "count": s.list.len(),
        "tool": s.tool.name(), "selected": s.selected, "renaming": s.renaming, "loading": s.loading, "error": s.error,
        "tab": if s.tab.is_empty() { "canvas" } else { s.tab.as_str() },
        "ask": { "text": s.ask_text.clone().unwrap_or_else(|| super::ASK_PROMPT.to_string()) },
        "popup": s.tool != Tool::Select,
    });
    let Some(c) = &s.canvas else {
        v["empty"] = if s.current.is_none() { "none".into() } else { "loading".into() };
        v["send"] = send_view(s, snap, 0);
        return v;
    };
    v["current"] = json!({ "id": c.id, "title": plain(&c.title), "shots": c.shots.len(), "rounds": c.rounds.len(), "reviews": c.reviews.len() });
    if c.shots.is_empty() {
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
        draft.push(json!({ "key": a.id, "kind": "asset", "n": a.n, "label": format!("Asset {} · {} · {}", a.n, clipped(&a.name), size_label(a.bytes)), "look": look, "selected": sel == Some(a.id.as_str()) }));
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
            items.push(json!({ "key": a.id, "label": format!("Asset {} · {}", a.n, clipped(&a.name)), "text": a.sandbox_path.clone().unwrap_or_default() }));
        }
        let read = if r.read_ms.is_some() { "read by the agent" } else { "not read yet" };
        history.push(json!({ "n": r.n, "line": format!("Review {} · {} · to {} · {}", r.n, when(r.at_ms), plain(&r.session), read), "items": items }));
    }
    v["history"] = J::Array(history);
    v["send"] = send_view(s, snap, new);
    v
}

fn send_view(s: &State, snap: &J, new: usize) -> J {
    let sb = s.sandbox.clone().unwrap_or_default();
    let sessions: Vec<(String, String, J)> = {
        let mut l: Vec<(String, String, J)> = snap["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["sandbox"] == sb.as_str() && i["state"] != "exited")
            .map(|i| {
                let t = i["thread"].as_str().filter(|t| !t.is_empty());
                (t.map_or_else(|| sb.clone(), |t| format!("{sb}::{t}")), crate::feat::monitor::model::who(&sb, t), i.clone())
            })
            .collect();
        l.sort_by_key(|(k, _, _)| (k.contains("::"), k.clone()));
        l
    };
    let key = s.session.clone().filter(|k| sessions.iter().any(|(x, _, _)| x == k)).or_else(|| sessions.first().map(|(k, _, _)| k.clone()));
    let label = key.as_ref().and_then(|k| sessions.iter().find(|(x, _, _)| x == k)).map(|(_, l, _)| l.clone());
    let running = snap["sandboxes"].as_array().into_iter().flatten().any(|x| x["name"] == sb.as_str() && x["status"] == "running");
    let info = key.as_ref().and_then(|k| sessions.iter().find(|(x, _, _)| x == k)).map(|(_, _, i)| i.clone()).unwrap_or(J::Null);
    let why = if snap["covered"] != false && !snap["covered"].is_null() {
        Some("Close what is open over the canvas first")
    } else if snap["jarvis"].as_array().is_some_and(|p| !p.is_empty()) {
        Some("A Jarvis proposal waits for you")
    } else if !running {
        Some("The sandbox is not running")
    } else if label.is_none() {
        Some("No agent session is open in this sandbox")
    } else if info["state"] == "attention" {
        Some("The session waits on a question or a permission")
    } else {
        None
    };
    let busy = s.job.as_ref().is_some_and(|j| j.sent);
    let to = label.clone().unwrap_or_else(|| format!("{sb} · no session"));
    json!({
        "label": format!("Send remarks to {to}"), "to": to, "new": new,
        "ok": new > 0 && why.is_none() && !busy, "ask_ok": why.is_none(),
        "why": if busy { Some("Sending…") } else if new == 0 { Some("No new pin, note or picture since the last round") } else { why },
        "sessions": sessions.iter().map(|(k, l, _)| json!({ "id": k, "label": l })).collect::<Vec<_>>(), "session": key,
    })
}

// ---------------------------------------------------------------------------
// The stand-in daemon (demos, tests)

/// A drawn mock screen: a header bar, a side bar on a wide one, cards with
/// lines. Neutral, invented; no real project.
pub fn mock_png(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let hue = [(52u8, 120u8, 200u8), (40, 160, 110), (180, 90, 60), (120, 80, 170)][(seed % 4) as usize];
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let bar = y < (h / 14).max(24);
        let side = w > 600 && x < w / 6;
        let (cx, cy) = ((x as i32 - if side { 0 } else { (w / 6) as i32 }).rem_euclid(300), (y as i32 - (h / 7) as i32).rem_euclid(220));
        let card = !side && !bar && (w <= 600 || x >= w / 6) && cx > 20 && cx < 280 && cy > 20 && cy < 200;
        let line = card && (cy - 40).rem_euclid(24) < 6 && cx < 220;
        let px = if bar {
            [hue.0, hue.1, hue.2]
        } else if side {
            [36, 40, 48]
        } else if line {
            [150, 155, 165]
        } else if card {
            [250, 250, 252]
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

fn summary(c: &J) -> J {
    let bytes: u64 = c["shots"].as_array().into_iter().flatten().chain(c["assets"].as_array().into_iter().flatten()).map(|x| x["bytes"].as_u64().unwrap_or(0)).sum();
    let draft = c["pins"].as_array().into_iter().flatten().filter(|p| p["sent"].is_null()).count() + c["notes"].as_array().into_iter().flatten().filter(|p| p["sent"].is_null()).count();
    json!({ "id": c["id"], "sandbox": c["sandbox"], "title": c["title"], "created_ms": c["created_ms"], "updated_ms": c["updated_ms"],
        "shots": c["shots"].as_array().map_or(0, Vec::len), "rounds": c["rounds"].as_array().map_or(0, Vec::len), "reviews": c["reviews"].as_array().map_or(0, Vec::len), "bytes": bytes, "draft": draft })
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
    if method == "GET" && p == "/api/canvas" {
        let mut l: Vec<J> = data["canvases"].as_object().into_iter().flatten().map(|(_, c)| summary(c)).collect();
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
                bytes += summary(&c)["bytes"].as_u64().unwrap_or(0);
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
            let png = mock_png(f["w"].as_u64().unwrap_or(800) as u32, f["h"].as_u64().unwrap_or(600) as u32, f["seed"].as_u64().unwrap_or(0) as u32);
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
            if pins + notes + assets == 0 {
                return Some((400, json!({ "ok": false, "error": "there is no new remark to send" })));
            }
            cv["reviews"].as_array_mut()?.push(json!({ "n": n, "at_ms": now, "session": body["session"], "pins": pins, "notes": notes, "assets": assets, "read_ms": null }));
            json!({ "ok": true, "review": n, "line": format!("[sbxw canvas · {id}] review round {n} is ready: run canvas_read to read the person's remarks") })
        }
        // Stand-in only: the agent adds a round of shots (`{shots: [{title, w, h, width, screen}]}`).
        "standin-shots" => {
            let cv = &mut data["canvases"][id.as_str()];
            let round = cv["rounds"].as_array().map_or(0, Vec::len) as u64 + 1;
            let top = cv["shots"].as_array().into_iter().flatten().map(|s| s["y"].as_f64().unwrap_or(0.0) + s["h"].as_f64().unwrap_or(0.0)).fold(-240.0, f64::max) + 240.0;
            let mut x = 0.0;
            let mut files = vec![];
            for (k, s) in body["shots"].as_array().into_iter().flatten().enumerate() {
                let (pw, ph) = (s["w"].as_u64().unwrap_or(1440), s["h"].as_u64().unwrap_or(900));
                let w = s["width"].as_f64().unwrap_or(pw as f64);
                let h = w * ph as f64 / pw as f64;
                let file = format!("s-{:06x}{:06x}.png", round, k);
                files.push((file.clone(), json!({ "w": pw, "h": ph, "seed": round as usize + k })));
                cv["shots"].as_array_mut()?.push(json!({ "id": format!("s-{round}-{k}"), "file": file, "round": round, "title": s["title"], "note": s["note"], "width": s["width"], "screen": s["screen"], "px_w": pw, "px_h": ph, "bytes": 200_000, "added_ms": now, "x": x, "y": top, "w": w, "h": h }));
                x += w + 120.0;
            }
            cv["rounds"].as_array_mut()?.push(json!({ "n": round, "at_ms": now, "shots": files.len() }));
            cv["updated_ms"] = now.into();
            for (f, v) in files {
                data["pictures"][f.as_str()] = v;
            }
            json!({ "ok": true })
        }
        _ => return None,
    };
    let what_ev = match what.as_str() {
        "standin-shots" => "shots",
        "submit" => "review",
        w => if w == "assets" { "asset" } else { "edit" },
    };
    (st.publish)("canvas", json!({ "id": id, "sandbox": sb, "what": what_ev }));
    Some((200, answer))
}
