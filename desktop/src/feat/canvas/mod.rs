//! Canvas: reviewing an agent's screens (docs/relay.md, "Canvas reviews").
//!
//! The agent adds shots with `canvas_add_shots`; the person pans and zooms
//! over them on a surface the core draws (`crate::surface_canvas`), places
//! numbered pins, notes and example pictures (assets), and sends a round of
//! remarks that the agent reads with `canvas_read`.
//!
//! * **Data.** The daemon keeps every canvas (`/api/canvas...`); this
//!   feature reads a canvas's manifest, fetches its pictures as bytes and
//!   hands them to the surface (decoded off the UI thread, size-checked
//!   before decoding). The person's unsent remarks live here and are saved
//!   as they change: one save in flight, the next carries everything newer.
//! * **Paths.** Only the core hands this feature a path (the file dialog, a
//!   drop, a pasted picture: `on_files`). No intent takes a path; nothing
//!   from an agent or a screen becomes one.
//! * **Sending** is an intent of risk Approval: ONE core Confirm with the
//!   target, every remark in full and each asset (local path when known,
//!   size, the guardrails' verdict and its box). On Send: each asset is
//!   copied into the sandbox (the upload route, one after the other), the
//!   daemon records the round, then ONE fixed line written by sbxw is typed
//!   into the session (`/api/chat/push`). A failure stops there; nothing is typed.
//! * **Refusals** are checked here in Rust, not only greyed: something over
//!   the view (`data.app.covered_top`: Canvas is an overlay, so a base view
//!   under it does not hold it), a Jarvis proposal, no session, a session
//!   asked something, ended, a sandbox not running.
//! * **Memory.** The surface's pictures are dropped when the overlay closes
//!   (`surface_canvas::close`, from `extras`, which runs on every data push).

pub mod model;
#[cfg(test)]
mod tests;

use crate::feat::monitor::model::who;
use crate::feat::{Action, Asked, Call, Confirm, Ctx, Extra, Feature, Incoming, Place, ScreenDecl, Slot, Zone};
use crate::intent::{Arg, Risk, Spec};
use crate::surface_canvas::{self as sc, CanvasEvent, Ground, Tool, ZoomCmd};
use model::{DState, Manifest, NoteD, PinD};
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const SCREEN: &str = "canvas";
/// The surface's id (`canvas{ id = "canvas" }`).
pub const SURF: &str = "canvas";
/// The view's data key.
pub const UI: &str = "canvas_ui";
/// Pictures fetched at once.
const FETCH_AT_ONCE: usize = 3;
/// A picture's bytes read at most (the daemon's own limit is 20 MB).
const IMAGE_READ_MAX: u64 = 21 << 20;
/// Characters of a remark typed into a session that has no `canvas_read`.
const FALLBACK_CHARS: usize = 300;
/// The default prompt of "Ask the agent for screenshots".
pub const ASK_PROMPT: &str = "Please take screenshots of the main screens of the app you are working on (one state per shot, at a fixed viewport width, no secrets on screen, about 12) and add them to a canvas with the canvas_add_shots tool, each with a short title, a one-line note and its width.";

pub const FEATURE: Feature = Feature {
    name: "canvas",
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../../screens/canvas.luau"), slot: Slot::Overlay { title: "Canvas", icon: "canvas" } }],
    events: &["canvas"],
    intents: INTENTS,
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    on_bytes: Some(on_bytes),
    on_files: Some(on_files),
    on_dismiss: Some(on_dismiss),
    extras: Some(extras),
    standin: Some(model::standin),
    surfaces: &[SURF],
    on_surface: Some(on_surface),
    shortcuts: &[crate::feat::Shortcut { key: 'o', intent: "canvas_window", args: r#"{"to":"window"}"#, help: "Open the Canvas in its own window" }],
    ..Feature::EMPTY
};

const TOOLS: &[&str] = &["select", "pin", "note", "asset"];
const ZOOMS: &[&str] = &["in", "out", "fit", "actual"];
const KEYS: &[&str] = &["type", "backspace", "escape", "up", "down", "enter", "left", "right"];
const OLDER: &[&str] = &["7", "30", "90"];
const TABS: &[&str] = &["canvas", "remarks"];
const GROUNDS: &[&str] = &["white", "dark", "follow"];
const WINDOW_TO: &[&str] = &["window", "main"];
const DIAGRAM_OPS: &[&str] = &["add", "relayout", "undo", "redo", "rename", "delete", "reload", "reapply"];

use crate::intent::direct as d;

pub const INTENTS: &[Spec] = &[
    d("canvas_show", &[("sandbox", Arg::Text, false), ("canvas", Arg::Str, false)], "Opens the Canvas over the panes: the shots an agent added of the screens it works on, to review with pins, notes and example pictures. On that sandbox's latest canvas, or that canvas."),
    d("canvas_sandbox", &[("name", Arg::Str, true)], "Shows that sandbox's latest canvas (the sandbox picker)."),
    d("canvas_pick", &[("id", Arg::Str, true)], "Shows that canvas (the canvas picker; `new` makes a new canvas for the sandbox shown)."),
    d("canvas_rename", &[("title", Arg::Text, false), ("open", Arg::Bool, false)], "Renames the canvas shown (`open`: shows or hides the title field)."),
    d("canvas_tool", &[("tool", Arg::OneOf(TOOLS), true)], "What a press on the surface does: select and move, place a numbered pin on a shot, place a note, place an example picture."),
    d("canvas_zoom", &[("cmd", Arg::OneOf(ZOOMS), true)], "Zooms in or out about the middle, fits every shot in view, or shows 100 %."),
    d("canvas_select", &[("target", Arg::Text, true)], "Selects a pin (`pin-<n>`), a note, an asset or a shot and brings it into view (empty: nothing)."),
    d("canvas_text", &[("target", Arg::Str, true), ("text", Arg::Text, true)], "Sets the text of an unsent pin or note; give the whole text on every edit."),
    d("canvas_remove", &[("target", Arg::Text, false)], "Removes an unsent pin, note or asset (none: the one selected). Sent ones stay."),
    d("canvas_asset", &[], "Opens the file dialog to add an example picture (PNG, JPEG or WebP)."),
    d("canvas_session", &[("key", Arg::Str, true)], "The session the remarks go to (`<sandbox>` for its main session, `<sandbox>::<thread>`)."),
    d("canvas_ask_text", &[("text", Arg::Text, true)], "Edits the prompt of Ask the agent for screenshots."),
    d("canvas_ground", &[("ground", Arg::OneOf(GROUNDS), true)], "The canvas's ground: `white` (the default), `dark`, or `follow` (the theme's: dark for the dark themes). Remembered across launches."),
    d("canvas_window", &[("to", Arg::OneOf(WINDOW_TO), true)], "Shows the Canvas in its own window (`window`: opened, or brought to the front when it is open; full screen there with ⌃⌘F, F11 elsewhere), or back over the panes of the main window (`main`: its window closes). One place at a time: while it has its window, the main window's Canvas says so."),
    d("canvas_tab", &[("tab", Arg::OneOf(TABS), true)], "On a narrow window: the canvas or the remarks."),
    d("canvas_diagram", &[("op", Arg::OneOf(DIAGRAM_OPS), true), ("id", Arg::Str, false)], "Edits the diagram selected (or `id`): add a node, lay it out again (the daemon's layout; one undo step), undo, redo, rename or delete the part selected; after a save was refused because the diagram changed elsewhere, reload the newer version or reapply your change over it."),
    d("canvas_key", &[("key", Arg::OneOf(KEYS), true), ("text", Arg::Text, false)], "The Canvas's keys: V, P, N, A pick a tool, Escape leaves it, Backspace removes the unsent remark selected."),
    Spec { name: "canvas_send", risk: Risk::Approval("Sends your remarks to an agent's session"), args: &[], doc: "Sends the new pins, notes and example pictures to the session shown, after one confirmation listing them all: the pictures are copied into the sandbox, the round recorded, and a fixed line asks the agent to read it with canvas_read." },
    Spec { name: "canvas_ask", risk: Risk::Approval("Types a prompt into an agent's session"), args: &[], doc: "Asks the session shown for screenshots of its main screens, after one confirmation showing the prompt in full; the canvas shown (or a new one) waits for them." },
    d("canvas_ask_cancel", &[], "Stops waiting for the screenshots asked for from the canvas shown: the agent's shots go where they used to."),
    Spec { name: "canvas_delete", risk: Risk::Approval("Deletes canvases and their pictures from this computer"), args: &[("older", Arg::OneOf(OLDER), false)], doc: "Deletes the canvas shown, or (`older`: days) every canvas not changed for that many days, after one confirmation naming what goes." },
];

/// A send of remarks under way, from its Confirm to the typed line.
#[derive(Debug, Clone)]
pub struct Job {
    pub n: u64,
    pub canvas: String,
    pub sb: String,
    pub thread: Option<String>,
    /// Assets still to copy, in order: (id, upload name).
    pub todo: Vec<(String, String)>,
    /// Copied: asset id → its path in the sandbox.
    pub paths: BTreeMap<String, String>,
    /// The plain numbered list typed after the line when the session may lack `canvas_read`.
    pub fallback: Option<String>,
    pub sent: bool,
}

/// What the person handed in for an asset, kept for the dialog.
#[derive(Debug, Clone, Default)]
pub struct Handed {
    pub local: Option<String>,
    pub look: Vec<String>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct State {
    pub list: Vec<J>,
    pub total: u64,
    pub sandbox: Option<String>,
    pub current: Option<String>,
    pub canvas: Option<Manifest>,
    pub loading: bool,
    pub error: Option<String>,
    pub pins: Vec<PinD>,
    pub notes: Vec<NoteD>,
    pub layout: BTreeMap<String, [f32; 4]>,
    /// The diagrams' editors, by diagram id (`surface_canvas::diagram`).
    pub diagrams: BTreeMap<String, DState>,
    /// The diagram part renamed inline (its selection key).
    pub dg_rename: Option<String>,
    pub remove: Vec<String>,
    pub saving: bool,
    pub dirty: bool,
    pub tool: Tool,
    pub selected: Option<String>,
    /// Pictures asked for (file, full), fetched FETCH_AT_ONCE at a time.
    pub queue: VecDeque<(String, bool)>,
    pub asked: BTreeSet<(String, bool)>,
    pub inflight: usize,
    pub handed: BTreeMap<String, Handed>,
    /// Bytes of assets handed in, waiting for the daemon's answer, by tag.
    pub adding: BTreeMap<String, Handed>,
    pub asset_at: Option<(f32, f32)>,
    pub pick_armed: bool,
    pub session: Option<String>,
    pub ask_text: Option<String>,
    pub job: Option<Job>,
    pub job_seq: u64,
    pub renaming: bool,
    pub tab: String,
    /// The ground the person chose (remembered: `features.canvas.ground`).
    pub ground: Ground,
    /// The surface holds this canvas's pictures (dropped on close).
    pub open: bool,
    pub seq: u64,
    /// What the view needs of the app's data when it is built without a Ctx.
    pub snap: J,
}

crate::feat::relay::per_thread!(STATE, State);

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    STATE().lock().unwrap().act(cx, a)
}
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_answer(cx, call, r)
}
fn on_event(cx: &Ctx, name: &str, data: &J) -> Vec<Action> {
    STATE().lock().unwrap().on_event(cx, name, data)
}
fn on_bytes(cx: &Ctx, call: &Call, r: Result<(u16, Vec<u8>, String), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_bytes(cx, call, r)
}
fn on_files(cx: &Ctx, inc: Incoming) -> Vec<Action> {
    STATE().lock().unwrap().on_files(cx, inc)
}
fn on_dismiss(cx: &Ctx, tag: &str) -> Vec<Action> {
    let mut s = STATE().lock().unwrap();
    if tag.starts_with("cv-up:") || tag.starts_with("cv-submit:") {
        if s.job.as_ref().is_some_and(|j| !j.sent) {
            s.job = None;
        }
    }
    vec![s.ui(Some(cx))]
}
fn on_surface(ev: &crate::surface::SurfaceEvent) -> Vec<Action> {
    let crate::surface::SurfaceEvent::Canvas { surface, ev } = ev else { return vec![] };
    if surface != SURF {
        return vec![];
    }
    STATE().lock().unwrap().on_canvas(ev.clone())
}

/// Shown: over the panes, or in its own window (`crate::windows`).
pub fn shown(cx: &Ctx) -> bool {
    cx.get("app")["overlay"]["screen"] == SCREEN || crate::windows::canvas_open()
}

/// The ground remembered across launches (absent, or a name no ground has: White).
pub fn kept_ground(cx: &Ctx) -> Ground {
    cx.get("app")["kept"]["canvas"]["ground"].as_str().and_then(Ground::parse).unwrap_or_default()
}

/// Why nothing may be sent now: something over the view, or a Jarvis proposal.
pub fn held(cx: &Ctx) -> Option<String> {
    if cx.data["app"]["covered_top"] != false {
        return Some("Not sent: something is open over the canvas (a dialog or a menu). Close it first.".into());
    }
    if cx.data["jarvis"]["pending"].as_array().is_some_and(|p| !p.is_empty()) {
        return Some("Not sent: a Jarvis proposal waits for you. Approve or refuse it first.".into());
    }
    None
}

/// What the view and the verdict read of the app's data.
fn snap_of(cx: &Ctx) -> J {
    json!({
        "sessions": cx.get("sessions"), "sandboxes": cx.get("sandboxes"), "groups": cx.get("app")["groups"],
        "covered": cx.get("app")["covered_top"], "jarvis": cx.get("jarvis")["pending"],
    })
}

fn wake() {
    // A decode landed: the UI redraws (a data push), off the decoding thread.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    crate::socket::post("canvas", vec![Action::Set { key: "canvas_px".into(), value: n.into() }]);
}

impl State {
    fn snap(&mut self, cx: &Ctx) {
        self.snap = snap_of(cx);
    }

    fn read_list(&self) -> Action {
        Action::Call(Call::get("/api/canvas", "read the canvases").tag("cv-list").quiet())
    }

    fn read_canvas(&mut self, id: &str) -> Action {
        self.loading = true;
        Action::Call(Call::get(format!("/api/canvas/{}", crate::client::enc(id)), "read the canvas").tag(&format!("cv-get:{id}")).quiet())
    }

    /// Shows canvas `id` (a fresh surface).
    fn show_canvas(&mut self, id: Option<String>) -> Vec<Action> {
        if self.current != id {
            self.forget_surface();
            self.canvas = None;
            self.pins.clear();
            self.notes.clear();
            self.layout.clear();
            self.diagrams.clear();
            self.dg_rename = None;
            self.remove.clear();
            self.selected = None;
            self.tool = Tool::Select;
            self.handed.clear();
            self.renaming = false;
            sc::refit(SURF);
        }
        self.current = id.clone();
        match id {
            Some(id) => vec![self.read_canvas(&id)],
            None => vec![],
        }
    }

    /// Canvas opens (over the panes, or in its own window): its sandbox,
    /// its ground, the list, and the canvas asked for or the last one.
    fn start(&mut self, cx: &Ctx, a: &Asked) -> Vec<Action> {
        sc::set_wake(std::sync::Arc::new(wake));
        if let Some(sb) = a.s("sandbox").filter(|s| !s.is_empty()) {
            self.sandbox = Some(sb.to_string());
        }
        if self.sandbox.is_none() {
            self.sandbox = cx.get("app")["selected"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        }
        self.ground = kept_ground(cx);
        let want = a.s("canvas").map(str::to_string).or_else(|| self.current.clone());
        // Opened again: the pictures are read again (they were dropped on close).
        self.forget_surface();
        self.current = None;
        let mut out = vec![self.read_list()];
        out.extend(self.show_canvas(want));
        out
    }

    fn forget_surface(&mut self) {
        sc::close(SURF);
        self.queue.clear();
        self.asked.clear();
        self.inflight = 0;
        self.open = false;
    }

    fn latest_of(&self, sb: &str) -> Option<String> {
        self.list.iter().filter(|c| c["sandbox"] == sb).max_by_key(|c| c["updated_ms"].as_u64().unwrap_or(0)).and_then(|c| c["id"].as_str().map(str::to_string))
    }

    pub fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        self.snap(cx);
        let mut out = vec![];
        match a.name {
            // Shown in its own window (crate::windows): the overlay says where
            // it is; a canvas asked for shows there, nothing is read again.
            "canvas_show" if crate::windows::canvas_open() => {
                out.push(Action::OpenPanel(SCREEN.into()));
                if let Some(id) = a.s("canvas").filter(|id| Some(*id) != self.current.as_deref()) {
                    out.push(self.read_list());
                    out.extend(self.show_canvas(Some(id.to_string())));
                }
            }
            "canvas_show" => {
                out.extend(self.start(cx, a));
                out.push(Action::OpenPanel(SCREEN.into()));
            }
            "canvas_window" if a.req("to") == "main" => {
                // The window closes (a confirmation it showed is cancelled);
                // the Canvas shows over the panes again, as it was.
                crate::windows::close_canvas();
                out.push(Action::OpenPanel(SCREEN.into()));
            }
            "canvas_window" => {
                // Not shown anywhere yet: read as Canvas opens.
                if !self.open && self.current.is_none() && !crate::windows::canvas_open() {
                    out.extend(self.start(cx, a));
                }
                crate::windows::open_canvas();
            }
            "canvas_sandbox" => {
                let sb = a.req("name").to_string();
                self.sandbox = Some(sb.clone());
                // A new scope: the list read again (another sandbox's canvases
                // may have come since).
                out.push(self.read_list());
                let latest = self.latest_of(&sb);
                out.extend(self.show_canvas(latest));
            }
            "canvas_pick" => {
                let id = a.req("id");
                if id == "new" {
                    let sb = self.sandbox.clone().ok_or("Pick a sandbox first")?;
                    out.push(Action::Call(Call::post("/api/canvas", json!({ "sandbox": sb, "title": "Review" }), "make a new canvas").tag("cv-new")));
                } else {
                    let c = self.list.iter().find(|c| c["id"] == id).ok_or_else(|| format!("{id} is not a canvas sbxw lists"))?;
                    self.sandbox = c["sandbox"].as_str().map(str::to_string);
                    out.extend(self.show_canvas(Some(id.to_string())));
                }
            }
            "canvas_rename" => {
                if let Some(open) = a.b("open") {
                    self.renaming = open;
                } else {
                    let id = self.current.clone().ok_or("No canvas is shown")?;
                    let t = a.s("title").unwrap_or("").trim().chars().take(120).collect::<String>();
                    if t.is_empty() {
                        return Err("A canvas needs a title".into());
                    }
                    self.renaming = false;
                    out.push(Action::Call(Call::post(format!("/api/canvas/{}/rename", crate::client::enc(&id)), json!({ "title": t }), "rename the canvas").tag("cv-rename")));
                }
            }
            "canvas_tool" => {
                self.tool = Tool::parse(a.req("tool")).unwrap_or_default();
                self.push_scene();
            }
            "canvas_zoom" => {
                let cmd = match a.req("cmd") {
                    "in" => ZoomCmd::In,
                    "out" => ZoomCmd::Out,
                    "actual" => ZoomCmd::Actual,
                    _ => ZoomCmd::Fit,
                };
                sc::zoom(SURF, cmd);
                out.extend(self.levels());
            }
            "canvas_select" => {
                let t = a.req("target");
                self.selected = (!t.is_empty()).then(|| t.to_string());
                if let Some(r) = self.rect_of(t) {
                    sc::show(SURF, r, 1.0);
                }
                self.push_scene();
                out.extend(self.levels());
            }
            "canvas_text" => {
                let t = a.req("target");
                let text: String = a.s("text").unwrap_or("").chars().take(2000).collect();
                if let Some(n) = t.strip_prefix("pin-").and_then(|n| n.parse::<u32>().ok()) {
                    let p = self.pins.iter_mut().find(|p| p.n == n).ok_or("That pin was sent already: a sent remark does not change")?;
                    p.text = text;
                } else {
                    let x = self.notes.iter_mut().find(|x| x.id == t).ok_or("That note was sent already: a sent remark does not change")?;
                    x.text = text;
                }
                self.push_scene();
                out.extend(self.save());
            }
            "canvas_remove" => {
                let t = a.s("target").filter(|t| !t.is_empty()).map(str::to_string).or_else(|| self.selected.clone()).ok_or("Select an unsent pin, note or example first")?;
                out.extend(self.remove(&t)?);
            }
            "canvas_asset" => {
                if let Some(why) = held(cx) {
                    return Err(why.replacen("Not sent", "Nothing added", 1));
                }
                self.current.clone().ok_or("Make or pick a canvas first")?;
                self.pick_armed = true;
                out.push(Action::PickFiles { title: "Add an example picture to the canvas".into(), extensions: vec!["png".into(), "jpg".into(), "jpeg".into(), "webp".into()], multiple: false });
            }
            "canvas_session" => {
                let k = a.req("key");
                let sb = self.sandbox.clone().unwrap_or_default();
                if !model::places(&snap_of(cx), &sb).iter().any(|(key, _, _)| key == k) {
                    return Err(format!("{k} is not a live session of {sb}"));
                }
                self.session = Some(k.to_string());
            }
            "canvas_ask_text" => self.ask_text = Some(a.s("text").unwrap_or("").chars().take(2000).collect()),
            "canvas_tab" => self.tab = a.req("tab").to_string(),
            "canvas_ground" => {
                self.ground = Ground::parse(a.req("ground")).unwrap_or_default();
                self.push_scene();
                out.push(Action::Remember { key: "ground".into(), value: self.ground.name().into() });
            }
            "canvas_key" => return self.key(cx, a),
            "canvas_diagram" => {
                out.extend(self.diagram_intent(a.req("op"), a.s("id"))?);
            }
            "canvas_send" => return self.send(cx),
            "canvas_ask" => return self.ask(cx),
            "canvas_ask_cancel" => {
                let id = self.current.clone().ok_or("No canvas is shown")?;
                return Ok(vec![Action::Call(Call::post(format!("/api/canvas/{}/ask/cancel", crate::client::enc(&id)), json!({}), "stop waiting for screenshots").tag("cv-ask-cancel"))]);
            }
            "canvas_delete" => return self.delete(a.s("older")),
            other => return Err(format!("{other}: not a canvas intent")),
        }
        out.push(self.ui(Some(cx)));
        Ok(out)
    }

    fn key(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let mut out = self.canvas_key(a.req("key"), a.s("text"))?;
        out.push(self.ui(Some(cx)));
        Ok(out)
    }

    /// The Canvas's keys, from the overlay (`canvas_key`) or from the surface
    /// when it has the keyboard: V, P, N, A pick a tool; Escape closes a
    /// rename, leaves a tool, or clears the selection; Backspace removes the
    /// unsent remark selected. A diagram's part selected takes its own keys first.
    fn canvas_key(&mut self, key: &str, text: Option<&str>) -> Result<Vec<Action>, String> {
        let mut out = vec![];
        if let Some((did, part)) = self.selected.as_deref().and_then(sc::diagram::parse_sel).map(|(d, p)| (d.to_string(), p)) {
            match key {
                "enter" if self.dg_rename.is_none() => {
                    self.dg_rename = Some(part.key(&did));
                    self.push_scene();
                    return Ok(out);
                }
                "tab" => {
                    if let sc::diagram::Part::Node(n) = part {
                        out.extend(self.diagram_op(&did, sc::diagram::Op::Successor { from: n }));
                    }
                    return Ok(out);
                }
                "delete" | "backspace" if part != sc::diagram::Part::Whole => {
                    out.extend(self.diagram_op(&did, sc::diagram::Op::Delete { part }));
                    return Ok(out);
                }
                "undo" | "redo" => {
                    out.extend(self.history(&did, key == "redo"));
                    return Ok(out);
                }
                _ => {}
            }
        }
        match key {
            "escape" => {
                if self.dg_rename.take().is_some() {
                } else if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                } else {
                    self.selected = None;
                }
                self.push_scene();
            }
            "backspace" | "delete" => {
                if let Some(t) = self.selected.clone().filter(|t| sc::diagram::parse_sel(t).is_none()) {
                    out.extend(self.remove(&t)?);
                }
            }
            "type" => {
                let tool = match text.unwrap_or("").to_lowercase().as_str() {
                    "v" => Some(Tool::Select),
                    "p" => Some(Tool::Pin),
                    "n" => Some(Tool::Note),
                    "a" => Some(Tool::Asset),
                    _ => None,
                };
                if let Some(t) = tool {
                    self.tool = t;
                    self.push_scene();
                }
            }
            _ => {}
        }
        Ok(out)
    }

    /// A key from the surface (it had the keyboard): as `canvas_key`; a
    /// refusal is a notice.
    fn surface_key(&mut self, key: &str, text: Option<&str>) -> Vec<Action> {
        match self.canvas_key(key, text) {
            Ok(v) => v,
            Err(e) => vec![Action::error(e)],
        }
    }

    // -----------------------------------------------------------------------
    // Diagrams

    /// The text a part's inline rename starts with.
    fn part_text(&self, key: &str) -> Option<String> {
        let (did, part) = sc::diagram::parse_sel(key)?;
        let d = &self.diagrams.get(did)?.cur;
        Some(match part {
            sc::diagram::Part::Node(n) => d.node(&n)?.label.clone(),
            sc::diagram::Part::Edge(e) => d.edge(&e)?.label.clone(),
            sc::diagram::Part::Whole => d.title.clone(),
        })
    }

    /// The diagram an intent acts on: `id`, else the one selected.
    fn diagram_target(&self, id: Option<&str>) -> Result<(String, sc::diagram::Part), String> {
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            if !self.diagrams.contains_key(id) {
                return Err(format!("{id} is not a diagram of this canvas"));
            }
            let part = self.selected.as_deref().and_then(sc::diagram::parse_sel).filter(|(d, _)| *d == id).map(|(_, p)| p).unwrap_or(sc::diagram::Part::Whole);
            return Ok((id.to_string(), part));
        }
        self.selected.as_deref().and_then(sc::diagram::parse_sel).map(|(d, p)| (d.to_string(), p)).ok_or_else(|| "Select a diagram first (press it on the canvas)".to_string())
    }

    fn diagram_intent(&mut self, op: &str, id: Option<&str>) -> Result<Vec<Action>, String> {
        let (did, part) = self.diagram_target(id)?;
        Ok(match op {
            "add" => {
                // Right of what is there (below in `tb`), on its first lane's row.
                let d = &self.diagrams[&did].cur;
                let b = sc::diagram::content_box(d);
                let (w, h) = sc::diagram::estimate(sc::diagram::NEW_LABEL, "", sc::diagram::Shape::Process);
                let (x, y) = if d.lr() { (b[0] + b[2] + sc::diagram::RANK_GAP + w / 2.0, d.nodes.first().map_or(h / 2.0, |n| n.y + n.h / 2.0)) } else { (d.nodes.first().map_or(w / 2.0, |n| n.x + n.w / 2.0), b[1] + b[3] + sc::diagram::RANK_GAP + h / 2.0) };
                self.diagram_op(&did, sc::diagram::Op::AddAt { x: x.round(), y: y.round() })
            }
            "relayout" => {
                let st = &self.diagrams[&did];
                if st.saving || st.dirty {
                    return Err("Not laid out: your last change is still being saved. Try again in a moment.".into());
                }
                if st.conflict.is_some() {
                    return Err("Not laid out: reload the diagram or reapply your change first.".into());
                }
                self.diagram_op(&did, sc::diagram::Op::Relayout)
            }
            "undo" => self.history(&did, false),
            "redo" => self.history(&did, true),
            "rename" => {
                if part == sc::diagram::Part::Whole && self.selected.as_deref() != Some(did.as_str()) {
                    self.selected = Some(did.clone());
                }
                self.dg_rename = Some(part.key(&did));
                self.push_scene();
                vec![]
            }
            "delete" => self.diagram_op(&did, sc::diagram::Op::Delete { part }),
            "reload" | "reapply" => {
                let st = self.diagrams.get_mut(&did).ok_or("That diagram is gone")?;
                let server = st.conflict.take().ok_or("This diagram has no refused change to settle")?;
                if op == "reload" {
                    // The newer version, and mine one undo step back.
                    let mine = std::mem::replace(&mut st.cur, server.clone());
                    st.hist.push(mine);
                    st.rev = server.rev;
                    st.error = None;
                    if let Some(c) = &mut self.canvas {
                        for it in c.diagrams.iter_mut().filter(|it| it.id == did) {
                            it.diagram = server.clone();
                        }
                    }
                    self.fix_selection(&did);
                    self.push_scene();
                    vec![Action::notice("Reloaded the diagram's newer version: undo brings yours back")]
                } else {
                    // Mine, whole, over the newer revision.
                    st.rev = server.rev;
                    st.error = None;
                    self.save_diagram(&did)
                }
            }
            other => return Err(format!("{other}: not a diagram operation")),
        })
    }

    /// One edit of diagram `did`: applied to a copy, checked whole by the
    /// core's limits, kept as one undo step, saved.
    fn diagram_op(&mut self, did: &str, op: sc::diagram::Op) -> Vec<Action> {
        let Some(st) = self.diagrams.get_mut(did) else { return vec![Action::error("That diagram is gone from the canvas")] };
        if st.relayout {
            return vec![Action::error("Not changed: the diagram is being laid out again. Try again in a moment.")];
        }
        let relayout = op == sc::diagram::Op::Relayout;
        let rename = matches!(op, sc::diagram::Op::Rename { .. });
        match sc::diagram::apply(&st.cur, &op) {
            Err(e) => {
                // A refused rename closes (the keyboard goes back to the
                // surface); Enter opens it again.
                if rename {
                    self.dg_rename = None;
                    self.push_scene();
                }
                vec![Action::error(format!("Not changed: {e}."))]
            }
            Ok(done) => {
                let before = std::mem::replace(&mut st.cur, done.d);
                st.hist.push(before);
                st.relayout = relayout;
                if let Some(p) = &done.select {
                    self.selected = Some(p.key(did));
                }
                if rename {
                    self.dg_rename = None;
                }
                if done.rename {
                    self.dg_rename = self.selected.clone();
                }
                self.fix_selection(did);
                self.push_scene();
                self.save_diagram(did)
            }
        }
    }

    /// Undo (or redo) one step of diagram `did`; saved as any edit.
    fn history(&mut self, did: &str, redo: bool) -> Vec<Action> {
        let Some(st) = self.diagrams.get_mut(did) else { return vec![] };
        if st.relayout {
            return vec![Action::error("Not changed: the diagram is being laid out again. Try again in a moment.")];
        }
        let step = if redo { st.hist.redo(&st.cur) } else { st.hist.undo(&st.cur) };
        let Some(d) = step else { return vec![Action::notice(if redo { "Nothing to redo" } else { "Nothing to undo" })] };
        st.cur = d;
        self.dg_rename = None;
        self.fix_selection(did);
        self.push_scene();
        self.save_diagram(did)
    }

    /// A selected part that is gone selects its whole diagram.
    fn fix_selection(&mut self, did: &str) {
        let Some(st) = self.diagrams.get(did) else { return };
        if let Some((d, part)) = self.selected.as_deref().and_then(sc::diagram::parse_sel) {
            let gone = d == did
                && match &part {
                    sc::diagram::Part::Node(n) => st.cur.node(n).is_none(),
                    sc::diagram::Part::Edge(e) => st.cur.edge(e).is_none(),
                    sc::diagram::Part::Whole => false,
                };
            if gone {
                self.selected = Some(did.to_string());
                self.dg_rename = None;
            }
        }
    }

    /// Diagram `did` saved whole (`POST /api/canvas/:id/diagrams/:did`):
    /// one save in flight; edits made meanwhile go in the next one, as one.
    /// Nothing is saved while a refused change waits for Reload or Reapply.
    fn save_diagram(&mut self, did: &str) -> Vec<Action> {
        let Some(cid) = self.current.clone() else { return vec![] };
        let Some(st) = self.diagrams.get_mut(did) else { return vec![] };
        if st.conflict.is_some() {
            return vec![];
        }
        if st.saving {
            st.dirty = true;
            return vec![];
        }
        st.saving = true;
        st.dirty = false;
        let body = json!({ "rev": st.rev, "diagram": sc::diagram::wire(&st.cur) });
        let path = format!("/api/canvas/{}/diagrams/{}", crate::client::enc(&cid), crate::client::enc(did));
        vec![Action::Call(Call::post(path, body, "save the diagram").tag(&format!("cv-dsave:{cid}:{did}")).quiet())]
    }

    /// The save's answer: the new revision (and, after "Lay out again", the
    /// daemon's places); a 409 reads the newer version and offers Reload or
    /// Reapply; any other refusal is said, the change kept here.
    fn diagram_saved(&mut self, cid: &str, did: &str, r: &Result<(u16, J), String>) -> Vec<Action> {
        let mut out = vec![];
        if self.current.as_deref() != Some(cid) {
            return out;
        }
        let Some(st) = self.diagrams.get_mut(did) else { return out };
        st.saving = false;
        match r {
            Ok((200..=299, b)) if b["ok"] != false => {
                st.rev = b["rev"].as_u64().map(|r| r as u32).unwrap_or(st.rev + 1);
                st.error = None;
                st.cur.rev = st.rev;
                let saved: Option<sc::diagram::Diagram> = serde_json::from_value(b["diagram"]["diagram"].clone()).ok().map(|mut d| {
                    sc::diagram::tidy(&mut d);
                    d
                });
                if std::mem::take(&mut st.relayout) {
                    // The daemon laid it out: its places are the diagram now (the step before is in undo).
                    if let Some(d) = saved.clone() {
                        st.cur = d;
                    }
                }
                let dirty = st.dirty;
                if let Some(c) = &mut self.canvas {
                    for it in c.diagrams.iter_mut().filter(|it| it.id == did) {
                        if let Some(d) = &saved {
                            it.diagram = d.clone();
                        }
                        it.w = b["diagram"]["w"].as_f64().map_or(it.w, |v| v as f32);
                        it.h = b["diagram"]["h"].as_f64().map_or(it.h, |v| v as f32);
                    }
                }
                self.push_scene();
                // The Send button counts changed diagrams as the daemon does.
                out.push(self.read_list());
                if dirty {
                    out.extend(self.save_diagram(did));
                }
            }
            Ok((409, _)) => {
                st.relayout = false;
                st.dirty = false;
                let path = format!("/api/canvas/{}/diagrams/{}", crate::client::enc(cid), crate::client::enc(did));
                out.push(Action::Call(Call::get(path, "read the diagram again").tag(&format!("cv-dget:{cid}:{did}")).quiet()));
            }
            Ok((code, b)) => {
                st.relayout = false;
                let why = b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"));
                st.error = Some(why.clone());
                out.push(Action::error(format!("The diagram could not be saved: {why}. Your change is kept here; the next change saves it again.")));
            }
            Err(e) => {
                st.relayout = false;
                st.error = Some(e.clone());
                out.push(Action::error(format!("The diagram could not be saved: {e}. Your change is kept here; the next change saves it again.")));
            }
        }
        out
    }

    /// After a 409: the daemon's newer version, held for Reload or Reapply.
    fn diagram_reread(&mut self, cid: &str, did: &str, r: &Result<(u16, J), String>) -> Vec<Action> {
        if self.current.as_deref() != Some(cid) {
            return vec![];
        }
        let Some(st) = self.diagrams.get_mut(did) else { return vec![] };
        match r {
            Ok((200, b)) => match serde_json::from_value::<sc::diagram::Diagram>(b["diagram"]["diagram"].clone()) {
                Ok(mut d) => {
                    sc::diagram::tidy(&mut d);
                    st.conflict = Some(d);
                    vec![]
                }
                Err(e) => {
                    st.error = Some(format!("the newer version could not be read ({e})"));
                    vec![]
                }
            },
            Ok((code, _)) => {
                st.error = Some(format!("the newer version could not be read (the daemon answered {code})"));
                vec![]
            }
            Err(e) => {
                st.error = Some(format!("the newer version could not be read ({e})"));
                vec![]
            }
        }
    }

    fn remove(&mut self, t: &str) -> Result<Vec<Action>, String> {
        if let Some(n) = t.strip_prefix("pin-").and_then(|n| n.parse::<u32>().ok()) {
            let before = self.pins.len();
            self.pins.retain(|p| p.n != n);
            if before == self.pins.len() {
                return Err("A sent pin stays: it belongs to a past round".into());
            }
        } else if self.notes.iter().any(|x| x.id == t) {
            self.notes.retain(|x| x.id != t);
        } else if let Some(c) = &self.canvas {
            if c.assets.iter().any(|x| x.id == t && x.sent.is_none()) {
                self.remove.push(t.to_string());
                self.pins.retain(|p| p.on != t);
                self.handed.remove(t);
            } else {
                return Err("Only an unsent pin, note or example can be removed".into());
            }
        } else {
            return Err("Nothing to remove".into());
        }
        if self.selected.as_deref() == Some(t) {
            self.selected = None;
        }
        self.push_scene();
        Ok(self.save())
    }

    /// The person's draft saved: one request in flight; a change meanwhile
    /// marks it dirty and the answer sends the rest.
    fn save(&mut self) -> Vec<Action> {
        let Some(id) = self.current.clone() else { return vec![] };
        if self.saving {
            self.dirty = true;
            return vec![];
        }
        self.saving = true;
        self.dirty = false;
        let moved = std::mem::take(&mut self.layout);
        // The places sent are kept in the manifest held here: the daemon does
        // not send our own edit back, and the scene falls back to it.
        if let Some(c) = &mut self.canvas {
            for (id, r) in &moved {
                for s in c.shots.iter_mut().filter(|s| &s.id == id) {
                    (s.x, s.y, s.w, s.h, s.moved) = (r[0], r[1], r[2], r[3], true);
                }
                for a in c.assets.iter_mut().filter(|a| &a.id == id) {
                    (a.x, a.y, a.w, a.h) = (r[0], r[1], r[2], r[3]);
                }
                for n in c.notes.iter_mut().filter(|n| &n.id == id) {
                    (n.x, n.y, n.w, n.h) = (r[0], r[1], r[2], r[3]);
                }
                for d in c.diagrams.iter_mut().filter(|d| &d.id == id) {
                    (d.x, d.y, d.moved) = (r[0], r[1], true);
                }
            }
        }
        let layout: Vec<J> = moved.into_iter().map(|(id, r)| json!({ "id": id, "x": r[0], "y": r[1], "w": r[2], "h": r[3] })).collect();
        let remove = std::mem::take(&mut self.remove);
        let body = json!({ "pins": self.pins, "notes": self.notes, "remove_assets": remove, "layout": layout });
        vec![Action::Call(Call::post(format!("/api/canvas/{}/edit", crate::client::enc(&id)), body, "save the canvas").tag(&format!("cv-save:{id}")).quiet())]
    }

    fn rect_of(&self, t: &str) -> Option<[f32; 4]> {
        let c = self.canvas.as_ref()?;
        if let Some(n) = t.strip_prefix("pin-").and_then(|n| n.parse::<u32>().ok()) {
            let on = self.pins.iter().find(|p| p.n == n).map(|p| p.on.clone()).or_else(|| c.pins.iter().find(|p| p.n == n).map(|p| p.on.clone()))?;
            // The pin's whole shot, the pin on it.
            return model::item_rect(c, &on, &self.layout);
        }
        if let Some(x) = self.notes.iter().find(|x| x.id == t) {
            return Some([x.x, x.y, x.w, x.h]);
        }
        if let Some((did, _)) = sc::diagram::parse_sel(t) {
            return model::diagram_rect(c, did, &self.layout, &self.diagrams).map(|(r, _)| r);
        }
        model::item_rect(c, t, &self.layout)
    }

    /// The diagrams' editors after a manifest was read: a new diagram gets
    /// one; a diagram the agent replaced is taken when the person has nothing
    /// unsaved on it (else their next save meets the daemon's 409 and offers
    /// Reload or Reapply); one gone is forgotten.
    fn sync_diagrams(&mut self, m: &mut Manifest) {
        for it in &mut m.diagrams {
            sc::diagram::tidy(&mut it.diagram);
            it.title = sc::diagram::clean(&it.title).chars().take(sc::diagram::MAX_TITLE).collect();
        }
        self.diagrams.retain(|id, _| m.diagrams.iter().any(|d| &d.id == id));
        for it in &m.diagrams {
            match self.diagrams.get_mut(&it.id) {
                None => {
                    self.diagrams.insert(it.id.clone(), DState { cur: it.diagram.clone(), rev: it.diagram.rev, ..DState::default() });
                }
                Some(st) if it.diagram.rev > st.rev && !st.saving && !st.dirty && st.conflict.is_none() => {
                    *st = DState { cur: it.diagram.clone(), rev: it.diagram.rev, ..DState::default() };
                }
                Some(_) => {}
            }
        }
    }

    /// The surface's scene from the manifest and the draft.
    pub fn push_scene(&mut self) {
        if let Some(c) = &self.canvas {
            let mut scene = model::scene(c, &self.pins, &self.notes, &self.layout, &self.diagrams, self.selected.as_deref(), self.tool);
            scene.ground = self.ground;
            scene.screen = SCREEN.into();
            scene.rename = self.dg_rename.as_deref().and_then(|k| self.part_text(k).map(|t| (k.to_string(), t)));
            sc::set_scene(SURF, scene);
            self.open = true;
        }
    }

    /// Asks for pictures the surface wants, and fetches the queue.
    fn levels(&mut self) -> Vec<Action> {
        for ev in sc::take_pending() {
            if let CanvasEvent::NeedFull { image } = ev {
                self.want(&image, true);
            }
        }
        self.pump()
    }

    fn want(&mut self, file: &str, full: bool) {
        if self.asked.insert((file.to_string(), full)) {
            self.queue.push_back((file.to_string(), full));
        }
    }

    fn pump(&mut self) -> Vec<Action> {
        let Some(id) = self.current.clone() else { return vec![] };
        let mut out = vec![];
        while self.inflight < FETCH_AT_ONCE {
            let Some((file, full)) = self.queue.pop_front() else { break };
            self.inflight += 1;
            let path = format!("/api/canvas/{}/images/{}", crate::client::enc(&id), crate::client::enc(&file));
            out.push(Action::Call(Call::get(path, "read a picture of the canvas").raw(IMAGE_READ_MAX).tag(&format!("cv-img:{id}:{file}:{}", u8::from(full))).quiet()));
        }
        out
    }

    pub fn on_canvas(&mut self, ev: CanvasEvent) -> Vec<Action> {
        let mut out = vec![];
        match ev {
            CanvasEvent::Select { target } => self.selected = target,
            CanvasEvent::Moved { item, x, y } => {
                if let Some((r, off)) = self.canvas.as_ref().and_then(|c| model::diagram_rect(c, &item, &self.layout, &self.diagrams)) {
                    // A diagram moves by its origin (where its (0, 0) sits): the edit route's `layout`.
                    self.layout.insert(item, [x - off[0], y - off[1], r[2], r[3]]);
                } else if let Some(n) = self.notes.iter_mut().find(|n| n.id == item) {
                    (n.x, n.y) = (x, y);
                } else if let Some(r) = self.canvas.as_ref().and_then(|c| model::item_rect(c, &item, &self.layout)) {
                    self.layout.insert(item, [x, y, r[2], r[3]]);
                }
                out.extend(self.save());
            }
            CanvasEvent::Resized { item, w, h } => {
                if let Some(n) = self.notes.iter_mut().find(|n| n.id == item) {
                    (n.w, n.h) = (w.max(120.0), h.max(60.0));
                } else if let Some(r) = self.canvas.as_ref().and_then(|c| model::item_rect(c, &item, &self.layout)) {
                    self.layout.insert(item, [r[0], r[1], w, h]);
                }
                out.extend(self.save());
            }
            CanvasEvent::PinAt { item, fx, fy } => {
                let Some(c) = &self.canvas else { return vec![] };
                let n = model::next_pin(c, &self.pins);
                self.pins.push(PinD { n, on: item, fx, fy, text: String::new() });
                self.selected = Some(format!("pin-{n}"));
                out.extend(self.save());
                out.push(Action::Focus(format!("canvas-text-pin-{n}")));
            }
            CanvasEvent::PinMoved { n, item, fx, fy } => {
                if let Some(p) = self.pins.iter_mut().find(|p| p.n == n) {
                    (p.on, p.fx, p.fy) = (item, fx, fy);
                    out.extend(self.save());
                }
            }
            CanvasEvent::NoteAt { x, y } => {
                let Some(c) = &self.canvas else { return vec![] };
                let n = model::next_note(c, &self.notes);
                let id = format!("n-{:08x}", rand_u32());
                self.notes.push(NoteD { id: id.clone(), n, x, y, w: 240.0, h: 140.0, text: String::new() });
                self.selected = Some(id.clone());
                self.tool = Tool::Select;
                out.extend(self.save());
                out.push(Action::Focus(format!("canvas-text-{id}")));
            }
            CanvasEvent::AssetAt { x, y } => {
                self.asset_at = Some((x, y));
                self.pick_armed = true;
                self.tool = Tool::Select;
                out.push(Action::PickFiles { title: "Add an example picture to the canvas".into(), extensions: vec!["png".into(), "jpg".into(), "jpeg".into(), "webp".into()], multiple: false });
            }
            CanvasEvent::NeedFull { image } => self.want(&image, true),
            // Only the readout changes (the screen's, beside the zoom buttons).
            CanvasEvent::Zoomed { .. } => {}
            CanvasEvent::Diagram { item, op } => out.extend(self.diagram_op(&item, op)),
            CanvasEvent::Rename { item, part } => {
                if self.diagrams.contains_key(&item) {
                    let k = part.key(&item);
                    self.selected = Some(k.clone());
                    self.dg_rename = Some(k);
                }
            }
            CanvasEvent::RenameCancel => self.dg_rename = None,
            CanvasEvent::Key { key, text } => out.extend(self.surface_key(&key, text.as_deref())),
        }
        self.push_scene();
        out.extend(self.levels());
        out.push(self.ui(None));
        out
    }

    pub fn on_event(&mut self, cx: &Ctx, name: &str, data: &J) -> Vec<Action> {
        if name != "canvas" || !shown(cx) {
            return vec![];
        }
        self.snap(cx);
        let mut out = vec![self.read_list()];
        let id = data["id"].as_str().unwrap_or("");
        if self.current.as_deref() == Some(id) {
            if data["what"] == "deleted" {
                out.extend(self.show_canvas(None));
                out.push(self.ui(Some(cx)));
            } else if data["what"] != "edit" {
                // Shots added by the agent while it is open appear live; our own saves are not read back.
                out.push(self.read_canvas(id));
            }
        }
        out
    }

    pub fn on_bytes(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, Vec<u8>, String), String>) -> Vec<Action> {
        let Some(rest) = call.tag.strip_prefix("cv-img:") else { return vec![] };
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        let (id, file, full) = (parts.first().copied().unwrap_or(""), parts.get(1).copied().unwrap_or(""), parts.get(2) == Some(&"1"));
        self.inflight = self.inflight.saturating_sub(1);
        if self.current.as_deref() != Some(id) {
            return self.pump();
        }
        match r {
            Ok((200, bytes, _)) if bytes.len() as u64 <= IMAGE_READ_MAX => {
                // An unsent asset's bytes are kept: a send copies them into the sandbox.
                if let Some(a) = self.canvas.as_ref().and_then(|c| c.assets.iter().find(|a| a.file == file && a.sent.is_none())) {
                    if !full {
                        self.handed.entry(a.id.clone()).or_default().bytes = bytes.clone();
                    }
                }
                sc::load(SURF, file, bytes, full);
            }
            Ok((code, _, _)) => log::warn!("canvas: a picture could not be read (HTTP {code})"),
            Err(e) => log::warn!("canvas: a picture could not be read: {e}"),
        }
        let mut out = self.pump();
        out.push(self.ui(Some(cx)));
        out
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        self.snap(cx);
        let ok = matches!(&r, Ok((c, b)) if (200..300).contains(c) && b["ok"] != false);
        let why = match &r {
            Ok((c, b)) => b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}")),
            Err(e) => e.clone(),
        };
        let body = r.as_ref().map(|(_, b)| b.clone()).unwrap_or(J::Null);
        let tag = call.tag.as_str();
        let mut out = vec![];
        if tag == "cv-list" {
            if ok {
                self.list = body["canvases"].as_array().cloned().unwrap_or_default();
                self.total = body["total_bytes"].as_u64().unwrap_or(0);
                if self.current.is_none() {
                    if let Some(sb) = self.sandbox.clone() {
                        let latest = self.latest_of(&sb);
                        out.extend(self.show_canvas(latest));
                    }
                }
            } else {
                self.error = Some(format!("The canvases could not be read: {why}"));
            }
        } else if let Some(id) = tag.strip_prefix("cv-get:") {
            self.loading = false;
            if self.current.as_deref() != Some(id) {
                return vec![];
            }
            if !ok {
                self.error = Some(format!("The canvas could not be read: {why}"));
            } else {
                self.error = None;
                match serde_json::from_value::<Manifest>(body["canvas"].clone()) {
                    Ok(mut m) => {
                        let first = self.canvas.is_none();
                        // Unsent remarks are ours: kept from the manifest the first time, then from here.
                        if first {
                            self.pins = m.pins.iter().filter(|p| p.sent.is_none()).map(PinD::from).collect();
                            self.notes = m.notes.iter().filter(|n| n.sent.is_none()).map(NoteD::from).collect();
                        }
                        for f in m.shots.iter().map(|s| s.file.clone()).chain(m.assets.iter().map(|a| a.file.clone())) {
                            self.want(&f, false);
                        }
                        self.sync_diagrams(&mut m);
                        self.canvas = Some(m);
                        self.push_scene();
                        out.extend(self.pump());
                    }
                    Err(e) => self.error = Some(format!("The canvas could not be read: {e}")),
                }
            }
        } else if tag == "cv-new" {
            if ok {
                let id = body["canvas"]["id"].as_str().unwrap_or("").to_string();
                out.push(self.read_list());
                out.extend(self.show_canvas(Some(id)));
            } else {
                out.push(Action::error(format!("No canvas made: {why}")));
            }
        } else if tag == "cv-rename" {
            if ok {
                if let Some(c) = &mut self.canvas {
                    c.title = body["title"].as_str().unwrap_or("").to_string();
                }
                out.push(self.read_list());
            } else {
                out.push(Action::error(format!("Not renamed: {why}")));
            }
        } else if let Some((cid, did)) = tag.strip_prefix("cv-dsave:").and_then(|t| t.split_once(':')) {
            let (cid, did) = (cid.to_string(), did.to_string());
            out.extend(self.diagram_saved(&cid, &did, &r));
        } else if let Some((cid, did)) = tag.strip_prefix("cv-dget:").and_then(|t| t.split_once(':')) {
            let (cid, did) = (cid.to_string(), did.to_string());
            out.extend(self.diagram_reread(&cid, &did, &r));
        } else if tag.starts_with("cv-save:") {
            self.saving = false;
            if !ok {
                self.dirty = true;
                out.push(Action::error(format!("The canvas could not be saved: {why}. Your remarks are kept here; the next change saves them again.")));
            } else if self.dirty {
                out.extend(self.save());
            }
        } else if let Some(t) = tag.strip_prefix("cv-asset:") {
            let h = self.adding.remove(t).unwrap_or_default();
            if ok {
                let aid = body["asset"]["id"].as_str().unwrap_or("").to_string();
                self.handed.insert(aid.clone(), h);
                if let Some(id) = self.current.clone() {
                    out.push(self.read_canvas(&id));
                }
                self.selected = Some(aid);
            } else {
                out.push(Action::error(format!("The picture was not added: {why}")));
            }
        } else if tag.starts_with("cv-up:") || tag.starts_with("cv-submit:") || tag.starts_with("cv-push:") {
            out.extend(self.send_step(cx, tag, ok, &why, &body));
        } else if tag == "cv-ask" {
            if ok {
                // The canvas asked from (or the one made for it) is shown, waiting.
                if let Some(id) = body["canvas"].as_str() {
                    self.current = Some(id.to_string());
                    out.push(self.read_canvas(id));
                }
                out.push(self.read_list());
                out.push(Action::notice("Asked the agent for screenshots: this canvas waits for them"));
            } else {
                out.push(Action::error(format!("Not asked: {why}")));
            }
        } else if tag == "cv-ask-cancel" {
            if ok {
                if let Some(id) = self.current.clone() {
                    out.push(self.read_canvas(&id));
                }
                out.push(self.read_list());
            } else {
                out.push(Action::error(format!("The request still waits: {why}")));
            }
        } else if tag == "cv-del" {
            if ok {
                out.push(Action::notice(format!("Deleted {} canvas(es), {} freed", body["deleted"], crate::feat::files::view::size_label(body["bytes"].as_u64().unwrap_or(0)))));
                out.push(self.read_list());
            } else {
                out.push(Action::error(format!("Nothing deleted: {why}")));
            }
        }
        out.push(self.ui(Some(cx)));
        out
    }

    pub fn on_files(&mut self, cx: &Ctx, inc: Incoming) -> Vec<Action> {
        self.snap(cx);
        let armed = std::mem::take(&mut self.pick_armed);
        if !shown(cx) && !armed {
            return vec![];
        }
        if let Some(why) = held(cx) {
            return vec![Action::error(why.replacen("Not sent", "Nothing added", 1))];
        }
        let Some(id) = self.current.clone() else { return vec![Action::error("Nothing added: make or pick a canvas first")] };
        let at = self.asset_at.take().unwrap_or_else(|| {
            let cam = sc::camera(SURF);
            let p = cam.world([cam.view[0] / 2.0, cam.view[1] / 2.0]);
            (p[0], p[1])
        });
        let items: Vec<(String, Handed)> = match inc {
            Incoming::Png(b) => vec![(format!("pasted-{}.png", self.seq + 1), Handed { local: None, look: vec![], bytes: b })],
            Incoming::Files(paths) => {
                let mut v = vec![];
                for p in paths.into_iter().take(10) {
                    match model::read_asset(&p) {
                        Ok(bytes) => v.push((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "picture".into()), Handed { local: Some(crate::feat::monitor::attach::written_out(&p.display().to_string())), look: crate::feat::files::drops::guardrails(std::slice::from_ref(&p), 1), bytes })),
                        Err(e) => return vec![Action::error(format!("Nothing added: {e}"))],
                    }
                }
                v
            }
        };
        let mut out = vec![];
        for (k, (name, h)) in items.into_iter().enumerate() {
            self.seq += 1;
            let tag = format!("{}", self.seq);
            let path = format!("/api/canvas/{}/assets?name={}&x={}&y={}", crate::client::enc(&id), crate::client::enc(&name), at.0 + 40.0 * k as f32, at.1 + 40.0 * k as f32);
            out.push(Action::Call(Call::post_bytes(path, "application/octet-stream", h.bytes.clone(), "add an example picture to the canvas").tag(&format!("cv-asset:{tag}"))));
            self.adding.insert(tag, h);
        }
        log::info!("canvas: {} picture(s) handed in", out.len());
        out.push(self.ui(Some(cx)));
        out
    }

    /// The session a send goes to, checked now: `(sandbox, thread, its info)`.
    pub fn target(&self, cx: &Ctx) -> Result<(String, Option<String>, J), String> {
        let sb = self.sandbox.clone().ok_or("Not sent: no sandbox is shown")?;
        // The view's own verdict (`model::verdict`), on the data as it is now.
        let v = model::verdict(&snap_of(cx), Some(&sb), self.session.as_deref(), false);
        if let Some(why) = v.why {
            return Err(format!("Not sent: {why}."));
        }
        let thread = v.info["thread"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        Ok((sb, thread, v.info))
    }

    fn send(&mut self, cx: &Ctx) -> Result<Vec<Action>, String> {
        if let Some(why) = held(cx) {
            return Err(why);
        }
        let c = self.canvas.clone().ok_or("Not sent: no canvas is shown")?;
        if self.saving || self.dirty {
            return Err("Not sent: your last remarks are still being saved. Try again in a moment.".into());
        }
        if self.job.as_ref().is_some_and(|j| j.sent) {
            return Err("Not sent: the last send is still under way.".into());
        }
        if self.diagrams.values().any(|d| d.saving || d.dirty || d.relayout) {
            return Err("Not sent: your last diagram change is still being saved. Try again in a moment.".into());
        }
        if self.diagrams.values().any(|d| d.conflict.is_some()) {
            return Err("Not sent: a diagram change was refused because the diagram changed elsewhere. Reload it or reapply your change first.".into());
        }
        let assets: Vec<&model::Asset> = c.assets.iter().filter(|a| a.sent.is_none() && !self.remove.contains(&a.id)).collect();
        let diagrams = model::draft_diagrams(self);
        if self.pins.is_empty() && self.notes.is_empty() && assets.is_empty() && diagrams == 0 {
            return Err("Not sent: there is no new pin, note, example picture or diagram change since the last round.".into());
        }
        let (sb, thread, info) = self.target(cx)?;
        let to = who(&sb, thread.as_deref());
        for a in &assets {
            if self.handed.get(&a.id).is_none_or(|h| h.bytes.is_empty()) {
                return Err(format!("Not sent: example {} is still loading. Try again in a moment.", a.n));
            }
        }
        let has_tool = model::session_has_tool(&c, &info);
        self.job_seq += 1;
        let n = self.job_seq;
        let todo: Vec<(String, String)> = assets.iter().map(|a| (a.id.clone(), format!("canvas-asset-{}.{}", a.n, model::ext_of(&a.file)))).collect();
        let fallback = (!has_tool).then(|| model::fallback_list(&c, &self.pins, &self.notes, FALLBACK_CHARS));
        let job = Job { n, canvas: c.id.clone(), sb: sb.clone(), thread: thread.clone(), todo: todo.clone(), paths: BTreeMap::new(), fallback, sent: false };
        let call = match todo.first() {
            Some((aid, name)) => self.upload_call(&job, aid, name)?,
            None => self.submit_call(&job, &to),
        };
        let total = self.pins.len() + self.notes.len() + assets.len() + diagrams;
        let mut conf = Confirm::new(format!("Send {total} remark{} to {to}?", if total == 1 { "" } else { "s" }), call)
            .line(format!("To: {to}'s agent, about canvas \"{}\".", model::plain(&c.title)))
            .line(format!("{} pin(s), {} note(s), {} example picture(s): every text in full below.", self.pins.len(), self.notes.len(), assets.len()));
        if diagrams > 0 {
            conf = conf.line(format!("{diagrams} changed diagram(s): the agent reads what changed, and each whole diagram, with canvas_read."));
        }
        let mut look = vec![];
        for a in &assets {
            let h = self.handed.get(&a.id).cloned().unwrap_or_default();
            let src = h.local.clone().unwrap_or_else(|| format!("{} (added earlier; kept by sbxw)", crate::feat::monitor::attach::written_out(&a.name)));
            let verdict = if h.look.is_empty() { String::new() } else { " · look before you send".into() };
            conf = conf.line(format!("Example {}: {src} · {}{verdict}", a.n, crate::feat::files::view::size_label(a.bytes)));
            look.extend(h.look.iter().map(|w| crate::feat::monitor::attach::written_out(w)));
        }
        if !assets.is_empty() {
            conf = conf.line(format!("The pictures are copied into {sb} first (under /tmp/sbxw-uploads/), one after the other; a failure stops there and sends nothing."));
        }
        conf = conf.line("Then sbxw records the round and types one fixed line into the session asking the agent to read it with canvas_read. Sent remarks stay on the canvas, read only.".to_string());
        if !has_tool {
            conf = conf.line("This session may have started before the canvas tools were installed (sbxw up for this sandbox): sbxw also types the remarks as a plain numbered list, each cut to 300 characters.".to_string());
        }
        for w in &look {
            conf = conf.line(format!("Look before you send: {w}."));
        }
        if !look.is_empty() {
            conf = conf.ack("I have looked at these pictures and want to send them");
        }
        let preview = model::remarks_text(&c, &self.pins, &self.notes, &assets);
        self.job = Some(job);
        log::info!("canvas: a send of {total} remark(s) asked");
        Ok(vec![Action::Confirm(Box::new(conf.preview(preview, false).labels(&format!("Send to {sb}"), "Cancel")))])
    }

    fn upload_call(&self, job: &Job, aid: &str, name: &str) -> Result<Call, String> {
        let bytes = self.handed.get(aid).map(|h| h.bytes.clone()).filter(|b| !b.is_empty()).ok_or("an example picture is not loaded")?;
        let route = format!("{}{}", crate::feat::files::upload_route_prefix(&job.sb), crate::client::enc(name));
        Ok(Call::post_bytes(route, "application/octet-stream", bytes, format!("copy an example picture into {}", job.sb)).tag(&format!("cv-up:{}:{aid}", job.n)))
    }

    fn submit_call(&self, job: &Job, to: &str) -> Call {
        let assets: serde_json::Map<String, J> = job.paths.iter().map(|(k, v)| (k.clone(), J::String(v.clone()))).collect();
        Call::post(format!("/api/canvas/{}/submit", crate::client::enc(&job.canvas)), json!({ "session": to, "assets": assets }), "record the round of remarks").tag(&format!("cv-submit:{}", job.n))
    }

    fn send_step(&mut self, cx: &Ctx, tag: &str, ok: bool, why: &str, body: &J) -> Vec<Action> {
        let Some(mut job) = self.job.clone() else { return vec![] };
        job.sent = true;
        let to = who(&job.sb, job.thread.as_deref());
        if let Some(rest) = tag.strip_prefix("cv-up:") {
            let aid = rest.split_once(':').map(|(_, a)| a).unwrap_or("");
            let path = body["path"].as_str().unwrap_or("");
            if !ok || !crate::feat::files::typable(path) {
                self.job = None;
                let done = job.paths.len();
                let why = if ok { "the daemon's path is not one of sbxw's upload folders".to_string() } else { why.to_string() };
                return vec![Action::error(format!("Not sent: an example picture was not copied into {} ({why}). {done} picture(s) were copied before it; nothing was recorded or typed. Your remarks are still here.", job.sb))];
            }
            job.paths.insert(aid.to_string(), path.to_string());
            job.todo.retain(|(a, _)| a != aid);
            let next = job.todo.first().cloned();
            self.job = Some(job.clone());
            return match next {
                Some((a, name)) => match self.upload_call(&job, &a, &name) {
                    Ok(c) => vec![Action::Call(c)],
                    Err(e) => {
                        self.job = None;
                        vec![Action::error(format!("Not sent: {e}. Nothing was recorded or typed."))]
                    }
                },
                None => vec![Action::Call(self.submit_call(&job, &to))],
            };
        }
        if tag.starts_with("cv-submit:") {
            if !ok {
                self.job = None;
                return vec![Action::error(format!("Not sent: the round could not be recorded ({why}). Nothing was typed."))];
            }
            let line = body["line"].as_str().unwrap_or("").to_string();
            if !line.starts_with("[sbxw canvas · ") {
                self.job = None;
                return vec![Action::error("Not sent: the daemon gave no line to type.")];
            }
            // The round is recorded: what was the draft is now read only.
            self.pins.clear();
            self.notes.clear();
            self.selected = None;
            let mut out = vec![];
            if let Some(id) = self.current.clone() {
                self.canvas = None;
                out.push(self.read_canvas(&id));
            }
            // The session as it is now must still take typed words.
            match self.target(cx) {
                Err(e) => {
                    self.job = None;
                    out.push(Action::error(format!("{} The round is recorded: the agent can read it with canvas_read.", e.replacen("Not sent", "Recorded, not typed", 1))));
                }
                Ok((sb, thread, _)) => {
                    let text = match &job.fallback {
                        Some(list) => format!("{line}\n{list}"),
                        None => line,
                    };
                    let mut b = json!({ "text": text, "name": sb });
                    if let Some(t) = &thread {
                        b["thread"] = t.clone().into();
                    }
                    out.push(Action::Call(Call::post("/api/chat/push", b, format!("tell {to} the round is ready")).tag(&format!("cv-push:{}", job.n))));
                    self.job = Some(job);
                }
            }
            return out;
        }
        // cv-push
        self.job = None;
        if ok {
            log::info!("canvas: a round of remarks sent");
            vec![Action::notice(format!("Sent to {to}: the agent reads your remarks with canvas_read"))]
        } else {
            vec![Action::error(format!("The round is recorded, but the line could not be typed into {to} ({why}). The agent can still read it with canvas_read."))]
        }
    }

    fn ask(&mut self, cx: &Ctx) -> Result<Vec<Action>, String> {
        if let Some(why) = held(cx) {
            return Err(why.replacen("Not sent", "Not asked", 1));
        }
        let (sb, thread, _) = self.target(cx).map_err(|e| e.replacen("Not sent", "Not asked", 1))?;
        let to = who(&sb, thread.as_deref());
        let text = self.ask_text.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| ASK_PROMPT.to_string());
        let mut b = json!({ "text": text.trim(), "sandbox": sb });
        if let Some(t) = &thread {
            b["thread"] = t.clone().into();
        }
        // The canvas shown, when it is this sandbox's: the shots come back to it.
        let here = self.canvas.as_ref().filter(|c| c.sandbox == sb).map(|c| (c.id.clone(), model::plain(&c.title)));
        let lands = match &here {
            Some((id, title)) => {
                b["canvas"] = id.clone().into();
                format!("Its shots come back to this canvas, \"{title}\", which shows it waits for them.")
            }
            None => "Its shots come back to a new canvas, \"Screenshots\", which shows it waits for them.".to_string(),
        };
        let call = Call::post("/api/canvas/ask", b, format!("ask {to} for screenshots")).tag("cv-ask");
        let c = Confirm::new(format!("Ask {to} for screenshots?"), call)
            .line(format!("To: {to}'s agent, typed into its session as one message, shown in full below, with a line naming the canvas."))
            .line(lands)
            .preview(text.trim().to_string(), false)
            .labels(&format!("Send to {sb}"), "Cancel");
        Ok(vec![Action::Confirm(Box::new(c))])
    }

    fn delete(&mut self, older: Option<&str>) -> Result<Vec<Action>, String> {
        let (ids, title, lines): (Vec<String>, String, Vec<String>) = match older {
            Some(days) => {
                let d: u64 = days.parse().unwrap_or(30);
                let before = crate::feat::files::view::now_secs().saturating_sub(d * 86_400) * 1000;
                let old: Vec<&J> = self.list.iter().filter(|c| c["updated_ms"].as_u64().unwrap_or(0) < before).collect();
                if old.is_empty() {
                    return Err(format!("No canvas is older than {d} days"));
                }
                let bytes: u64 = old.iter().map(|c| c["bytes"].as_u64().unwrap_or(0)).sum();
                let mut lines = vec![format!("{} canvas(es) not changed for {d} days, {} of pictures, from this computer.", old.len(), crate::feat::files::view::size_label(bytes))];
                for c in old.iter().take(12) {
                    lines.push(format!("{} · {} · {} shot(s) · {}", model::plain(c["title"].as_str().unwrap_or("")), c["sandbox"].as_str().unwrap_or(""), c["shots"], crate::feat::files::view::size_label(c["bytes"].as_u64().unwrap_or(0))));
                }
                if old.len() > 12 {
                    lines.push(format!("and {} more", old.len() - 12));
                }
                (old.iter().filter_map(|c| c["id"].as_str().map(str::to_string)).collect(), format!("Delete {} canvas(es) older than {d} days?", old.len()), lines)
            }
            None => {
                let id = self.current.clone().ok_or("No canvas is shown")?;
                let c = self.list.iter().find(|c| c["id"] == id.as_str()).cloned().unwrap_or(J::Null);
                let title = model::plain(c["title"].as_str().unwrap_or("this canvas"));
                (vec![id], format!("Delete the canvas \"{title}\"?"), vec![format!(
                    "Its {} shot(s), {} review round(s) and every remark, {} of pictures, from this computer. The agent's own files are not touched.",
                    c["shots"], c["reviews"], crate::feat::files::view::size_label(c["bytes"].as_u64().unwrap_or(0))
                )])
            }
        };
        let mut conf = Confirm::new(title, Call::post("/api/canvas/delete", json!({ "ids": ids }), "delete canvases").tag("cv-del")).danger();
        for l in lines {
            conf = conf.line(l);
        }
        conf = conf.line("This cannot be undone.".to_string());
        Ok(vec![Action::Confirm(Box::new(conf.labels("Delete", "Cancel")))])
    }

    /// The view's data (`canvas_ui`). Without a Ctx (a surface event) it uses
    /// what the last action saw of the app.
    pub fn ui(&self, cx: Option<&Ctx>) -> Action {
        let snap = match cx {
            Some(cx) => snap_of(cx),
            None => self.snap.clone(),
        };
        Action::Set { key: UI.into(), value: model::view(self, &snap) }
    }
}

fn rand_u32() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_le_bytes(b)
}

/// The rail entry; and, since this runs on every data push, the moment the
/// overlay is seen closed: its pictures are dropped (MEMORY.md).
fn extras(cx: &Ctx) -> Vec<Extra> {
    // `extras` also runs once while the app's data is half built (before
    // `app.overlay` and `app.covered` are set): only the whole data says closed.
    if !shown(cx) && cx.get("app").get("covered").is_some() {
        if let Ok(mut s) = STATE().try_lock() {
            if s.open {
                s.forget_surface();
            }
        }
    }
    vec![Extra {
        place: Place::Rail { zone: Zone::Browse, order: 25 },
        id: SCREEN.into(),
        label: "Canvas".into(),
        icon: Some("canvas".into()),
        tone: "blue".into(),
        help: "Canvas: review the screenshots an agent added, pin remarks and send them back".into(),
        count: None,
        intent: Some(("canvas_show".into(), json!({}))),
    }]
}
