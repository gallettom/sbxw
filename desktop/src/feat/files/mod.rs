//! The Project panel: a sandbox's deliverables (`.sbxw-artifacts`), its
//! files and what was written lately, uploads into it, and its code map
//! (`assets/js/files.js`, `upload.js`, `codemap.js`; `docs/web-ui.md`,
//! "The Project panel").
//!
//! The rules of this area:
//!
//! * **Everything read from a sandbox is untrusted.** Text and Markdown go
//!   to the screen as strings drawn by the core's `text` and `markdown`
//!   elements; pictures only through `crate::picture` (limits before
//!   decoding, one re-encode); HTML is shown as its source, never rendered;
//!   nothing is run.
//! * **Nothing a sandbox names is written on this computer.** A download
//!   is written by the core, under a name the core cleans itself, in the
//!   Downloads folder (direct) or where the person picks in the save dialog
//!   (approval). Opening a file with this computer's app is an approval whose
//!   dialog the core builds from what it wrote and sniffed (its open-by-type
//!   rule refuses programs, scripts, installers and shortcuts).
//! * **Deleting, uploading, opening, writing outside Downloads** are intents
//!   of risk Approval: the core's dialog shows the exact paths and sizes.
//! * **A local path only comes from the person** (a drop, a paste, the file
//!   dialog: `on_files`); no intent takes one. Sandbox paths are arguments
//!   (browsing), checked by the daemon as for the web UI.
//!
//! Parts that need the core's next operations (raw reads, downloads, opening
//! and revealing, streamed uploads, drops on a pane) are marked `CORE:` and
//! said in plain words until they land (see `FILES.md`).

pub mod codemap;
mod standin;
#[cfg(test)]
mod tests;
pub mod view;

use super::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Incoming, Method, Place, ScreenDecl, Slot};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use codemap::Doc;
use serde_json::{json, Value as J};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const SCREEN: &str = "files";
pub const MAP_SCREEN: &str = "codemap";
/// `data.files_ui`, `data.codemap_ui`: set by the core, what the screens draw.
pub const UI: &str = "files_ui";
pub const MAP_UI: &str = "codemap_ui";
/// The daemon's answers, each a route's whole answer (read for the sandbox shown).
pub const ARTIFACTS: &str = "artifacts";
pub const DIR: &str = "files_dir";
pub const RECENT: &str = "files_recent";
pub const RUNS: &str = "codemap";

/// CORE: the core hands a feature a route's raw bytes (`Feature::on_bytes`).
/// Until then text arrives through the JSON path (good for text, not for
/// pictures), and pictures say so.
pub const RAW_READS: bool = false;
/// The most map files read (a map is a few dozen).
const MAP_FILES_MAX: usize = 200;

const TABS: &[&str] = &["artifacts", "files", "recent"];

pub const FEATURE: Feature = Feature {
    name: "files",
    data: &[
        // `{name}`, not `{sandbox}`: read for the sandbox the panel shows, on
        // its own (`Action::Fetch`), never for every running sandbox.
        Data { key: ARTIFACTS, path: "/api/sandboxes/{name}/artifacts", on: &[], every: 0, lazy: true },
        Data { key: DIR, path: "/api/sandboxes/{name}/fs", on: &[], every: 0, lazy: true },
        Data { key: RECENT, path: "/api/sandboxes/{name}/fs/recent", on: &[], every: 0, lazy: true },
        Data { key: RUNS, path: "/api/codemap", on: &["codemap"], every: 0, lazy: false },
    ],
    screens: &[
        ScreenDecl { name: SCREEN, src: include_str!("../../../screens/files.luau"), slot: Slot::Panel { title: "Project", icon: "folder", width: 460.0 } },
        ScreenDecl { name: MAP_SCREEN, src: include_str!("../../../screens/codemap.luau"), slot: Slot::Panel { title: "Code map", icon: "sparkle", width: 520.0 } },
    ],
    intents: INTENTS,
    events: &["codemap"],
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    on_data: Some(on_data),
    extras: Some(extras),
    standin: Some(standin::route),
    on_files: Some(on_files),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "files_open", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("tab", Arg::OneOf(TABS), false)], doc: "Opens the Project panel on that sandbox (its deliverables first, or `tab`)." },
    Spec { name: "files_tab", risk: Risk::Direct, args: &[("tab", Arg::OneOf(TABS), true)], doc: "Shows the Artifacts, Files or Recent tab." },
    Spec { name: "files_place", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Goes to one of the places (`recent`, or a place's path as listed in `files_ui.places`)." },
    Spec { name: "files_browse", risk: Risk::Direct, args: &[("path", Arg::Str, true)], doc: "Lists that folder of the sandbox (`~` is the agent's home). The daemon reads it as the agent sees it." },
    Spec { name: "files_up", risk: Risk::Direct, args: &[], doc: "Lists the parent folder." },
    Spec { name: "files_back", risk: Risk::Direct, args: &[], doc: "Back to the folder listed before." },
    Spec { name: "files_filter", risk: Risk::Direct, args: &[("text", Arg::Text, false)], doc: "Filters the rows by name; give it on every edit." },
    Spec { name: "files_hidden", risk: Risk::Direct, args: &[("on", Arg::Bool, true)], doc: "Shows dotfiles, or hides them." },
    Spec { name: "files_select", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Picks a row by its `id`: a folder is listed, a file previewed (text, Markdown, pictures; HTML as its source)." },
    Spec { name: "files_show_in_folder", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Recent: lists the folder a file was written to, on that file." },
    Spec { name: "files_copy_path", risk: Risk::Direct, args: &[], doc: "Puts the selected file's path, as the agent sees it, on the clipboard." },
    Spec { name: "files_download", risk: Risk::Direct, args: &[], doc: "Downloads the selected file into this computer's Downloads folder, under a name the core cleans (never over a file there)." },
    Spec { name: "files_reveal", risk: Risk::Direct, args: &[], doc: "Downloads the selected file into Downloads and shows it in the file manager." },
    Spec { name: "files_save_as", risk: Risk::Approval("Save a sandbox file outside Downloads"), args: &[], doc: "The save dialog, for the selected file; the core's dialog names the file and its size first." },
    Spec { name: "files_open_with", risk: Risk::Approval("Open a sandbox file with this computer's app"), args: &[], doc: "Downloads the selected file, then the core's dialog says what it is (its type sniffed) and what would open it; programs, scripts, installers and shortcuts are refused." },
    Spec { name: "files_delete", risk: Risk::Approval("Delete a file in a sandbox"), args: &[], doc: "Deletes the selected file or folder (under the workspace, a read-write mount, home or /tmp only), after the core's dialog shows its path and size." },
    Spec { name: "files_star", risk: Risk::Direct, args: &[("path", Arg::Str, true)], doc: "Pins a folder among the places of this sandbox, or unpins it (kept across runs)." },
    Spec { name: "files_refresh", risk: Risk::Direct, args: &[], doc: "Reads the panel's lists again." },
    Spec { name: "files_upload_pick", risk: Risk::Direct, args: &[], doc: "The system's file dialog, to pick a file to upload." },
    Spec { name: "files_upload_paste", risk: Risk::Direct, args: &[], doc: "Takes the picture on the clipboard, to upload." },
    Spec { name: "files_upload_remove", risk: Risk::Direct, args: &[("index", Arg::Int(1, 10), false)], doc: "Drops file `index` from the files waiting to be uploaded (none: all)." },
    Spec {
        name: "files_upload_send",
        risk: Risk::Approval("Upload files into a sandbox"),
        args: &[("sandbox", Arg::Str, false)],
        doc: "Copies the files waiting into the sandbox (the panel's, or `sandbox`), then types their paths into its agent's prompt, not submitted; the core's dialog lists each local path and size first. 128 MB at most each.",
    },
    Spec { name: "codemap_open", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("map", Arg::Str, false)], doc: "Opens the Code map panel on that sandbox's map (or `map`: a lens's folder, as listed in `codemap_ui.maps`)." },
    Spec { name: "codemap_doc", risk: Risk::Direct, args: &[("doc", Arg::Str, true), ("anchor", Arg::Str, false)], doc: "Shows a map file, or one of its sections (`anchor`: a section id)." },
    Spec { name: "codemap_link", risk: Risk::Direct, args: &[("url", Arg::Url, true)], doc: "A link pressed in a map file: a `[[wiki link]]` is followed in the panel, any other opens in the browser." },
    Spec { name: "codemap_filter", risk: Risk::Direct, args: &[("text", Arg::Text, false)], doc: "Searches the map: files whose name, title or text hold the words." },
    Spec { name: "codemap_generate", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Asks the sandbox's agent to write its code map (`/codemap`), in a background session." },
    Spec { name: "codemap_lens_set", risk: Risk::Direct, args: &[("text", Arg::Text, false)], doc: "The reader a lens is for, as typed; give it on every edit." },
    Spec { name: "codemap_lens", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Asks the sandbox's agent to write a lens of its map for the reader typed (`/codemap-lens`), in a background session." },
    Spec { name: "codemap_diagram", risk: Risk::Approval("Open a page an agent wrote in your browser"), args: &[], doc: "The map's Archify diagram is an HTML page the app cannot draw: saved into Downloads, then opened in the browser, after the core's dialog says so." },
];

// ---------------------------------------------------------------------------
// State

#[derive(Debug, Clone, PartialEq)]
enum Src {
    /// Under `.sbxw-artifacts`: `path` is relative to it.
    Artifact,
    /// Anywhere in the sandbox: `path` is absolute.
    Fs,
}

#[derive(Debug, Clone)]
struct Sel {
    src: Src,
    path: String,
    name: String,
    size: u64,
    modified: u64,
    kind: String,
}

#[derive(Debug, Clone)]
enum Payload {
    Path(PathBuf),
    /// A picture from the clipboard (PNG). CORE: streamed by `core:upload`.
    #[allow(dead_code)]
    Png(Vec<u8>),
}

#[derive(Debug, Clone)]
struct Staged {
    name: String,
    size: u64,
    payload: Payload,
}

impl Staged {
    fn problem(&self) -> Option<String> {
        if self.size == 0 {
            Some("empty: the daemon takes no empty file".into())
        } else if self.size > view::UPLOAD_MAX {
            Some(format!("{}: more than the {} the daemon takes", view::size_label(self.size), view::size_label(view::UPLOAD_MAX)))
        } else {
            None
        }
    }
    fn source(&self) -> String {
        match &self.payload {
            Payload::Path(p) => p.display().to_string(),
            Payload::Png(_) => "a picture from the clipboard".into(),
        }
    }
}

#[derive(Debug, Default)]
struct MapState {
    sandbox: Option<String>,
    /// The map's folder under `.sbxw-artifacts` (`codemap`, `codemap-lenses/po`).
    root: Option<String>,
    /// The map asked for by the person (`codemap_open { map }`).
    want: Option<String>,
    docs: BTreeMap<String, Doc>,
    /// Map files still being read, and the read generation they belong to.
    waiting: usize,
    gen: u64,
    loading: bool,
    current: Option<String>,
    anchor: Option<String>,
    filter: String,
    lens: String,
    diagram: Option<(String, u64)>,
    error: Option<String>,
}

#[derive(Debug, Default)]
pub struct State {
    sandbox: Option<String>,
    tab: String,
    /// The folder asked for (empty: the daemon's default, the workspace).
    path: String,
    history: Vec<String>,
    /// The first listing goes on to `.sbxw-artifacts` when there is one.
    first: bool,
    /// A row to select once its folder is listed (Kit logs, Show in folder).
    select_next: Option<String>,
    filter: String,
    hidden: bool,
    sel: Option<Sel>,
    preview: J,
    seq: u64,
    staged: Vec<Staged>,
    paste_seq: u64,
    /// The sandbox each daemon answer was asked for.
    asked_for: BTreeMap<&'static str, String>,
    /// Sandboxes whose run ended before the answer to its start came back
    /// (then "is writing" would be stale).
    finished: std::collections::HashSet<String>,
    map: MapState,
}

crate::feat::relay::per_thread!(STATE, State);

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    STATE().lock().unwrap().act(cx, a)
}
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_answer(cx, call, r)
}
fn on_data(cx: &Ctx, key: &str, old: &J, new: &J) -> Vec<Action> {
    STATE().lock().unwrap().on_data(cx, key, old, new)
}
fn on_event(cx: &Ctx, name: &str, data: &J) -> Vec<Action> {
    STATE().lock().unwrap().on_event(cx, name, data)
}
fn on_files(cx: &Ctx, inc: Incoming) -> Vec<Action> {
    STATE().lock().unwrap().on_files(cx, inc)
}

fn sb_route(sandbox: &str, rest: &str) -> String {
    format!("/api/sandboxes/{}{rest}", enc(sandbox))
}

/// CORE: a download written by the core (`Action::Download`). Until it
/// lands, said in plain words.
fn download(_get: String, name: &str, reveal: bool) -> Action {
    Action::error(format!(
        "Saving {name}{} needs the app's download, which is not in this build yet: use the web UI's Download meanwhile.",
        if reveal { " and showing it in its folder" } else { "" }
    ))
}

/// CORE: the core's own operation behind a Confirm (`core:download`,
/// `core:upload`); its final shape is the core's.
fn core_call(op: &str, body: J, what: String) -> Call {
    let mut c = Call::post(format!("core:{op}"), body, what);
    c.method = Method::Core;
    c.tag(op)
}

/// The bytes of an answer read through the JSON path (see [`RAW_READS`]):
/// a text body comes back as a string, or as `{"error": text}`.
fn interim_bytes(v: &J) -> Vec<u8> {
    match v {
        J::Null => vec![],
        J::String(s) => s.clone().into_bytes(),
        J::Object(o) if o.len() == 1 && o.get("error").is_some_and(J::is_string) => o["error"].as_str().unwrap_or("").as_bytes().to_vec(),
        other => serde_json::to_vec_pretty(other).unwrap_or_default(),
    }
}

fn why(status: u16, body: &J) -> String {
    body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {status}"))
}

impl State {
    fn sandbox(&self) -> Result<String, String> {
        self.sandbox.clone().ok_or_else(|| "open the Project panel on a sandbox first".to_string())
    }

    fn tab(&self) -> &str {
        if self.tab.is_empty() {
            "artifacts"
        } else {
            &self.tab
        }
    }

    pub fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let mut out = vec![];
        match a.name {
            "files_open" => {
                let sb = a.req("sandbox").to_string();
                cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
                if self.sandbox.as_deref() != Some(&sb) {
                    let staged = std::mem::take(&mut self.staged);
                    *self = State { map: std::mem::take(&mut self.map), staged, paste_seq: self.paste_seq, seq: self.seq, ..State::default() };
                    self.sandbox = Some(sb.clone());
                    self.first = true;
                    for k in [ARTIFACTS, DIR, RECENT] {
                        out.push(Action::Set { key: k.into(), value: J::Null });
                    }
                    out.extend(self.fetch_all(&sb));
                }
                match a.s("tab") {
                    Some(t) => self.tab = t.into(),
                    None if self.tab.is_empty() => self.tab = cx.get("app")["kept"]["files"]["tab"].as_str().filter(|t| TABS.contains(t)).unwrap_or("artifacts").into(),
                    None => {}
                }
                out.push(Action::OpenPanel(SCREEN.into()));
            }
            "files_tab" => {
                self.tab = a.req("tab").into();
                self.sel = None;
                self.preview = J::Null;
                out.push(Action::Remember { key: "tab".into(), value: self.tab.clone().into() });
            }
            "files_place" => {
                let id = a.req("id");
                if id == "recent" {
                    self.tab = "recent".into();
                } else {
                    let places = view::places(cx.get(DIR), &self.starred(cx));
                    let p = places.iter().find(|p| p["id"] == id).ok_or_else(|| format!("{id} is not one of the places"))?;
                    self.select_next = p["select"].as_str().map(str::to_string);
                    let path = p["path"].as_str().unwrap_or("/").to_string();
                    out.extend(self.browse(&path)?);
                }
            }
            "files_browse" => {
                let mut path = a.req("path").trim().to_string();
                let home = cx.get(DIR)["home"].as_str().unwrap_or("/home/agent").to_string();
                if path == "~" {
                    path = home;
                } else if let Some(r) = path.strip_prefix("~/") {
                    path = view::join(&home, r);
                }
                if !path.starts_with('/') {
                    return Err(format!("{path}: give a path from / (or ~)"));
                }
                out.extend(self.browse(&path)?);
            }
            "files_up" => {
                let d = cx.get(DIR);
                let parent = d["parent"].as_str().map(str::to_string).or_else(|| d["path"].as_str().filter(|p| *p != "/").map(view::parent)).ok_or("this is the top: there is no folder above")?;
                out.extend(self.browse(&parent)?);
            }
            "files_back" => {
                let prev = self.history.pop().ok_or("nothing to go back to")?;
                let sb = self.sandbox()?;
                self.path = prev.clone();
                self.sel = None;
                self.preview = J::Null;
                self.tab = "files".into();
                out.push(self.fetch_dir(&sb));
            }
            "files_filter" => self.filter = a.s("text").unwrap_or("").into(),
            "files_hidden" => self.hidden = a.b("on").unwrap_or(false),
            "files_select" => out.extend(self.select(cx, a.req("id"))?),
            "files_show_in_folder" => {
                let id = a.req("id").to_string();
                if !self.recent_rows(cx).iter().any(|r| r["id"] == id.as_str()) {
                    return Err(format!("{id} is not in Recent"));
                }
                self.select_next = Some(id.rsplit('/').next().unwrap_or("").to_string());
                out.extend(self.browse(&view::parent(&id))?);
            }
            "files_copy_path" => {
                let s = self.sel.clone().ok_or("pick a file first")?;
                out.push(Action::Intent { name: "copy_text".into(), args: json!({ "text": self.abs_path(cx, &s) }) });
            }
            "files_download" | "files_reveal" => {
                let s = self.file_sel()?;
                out.push(download(self.download_route(&s)?, &s.name, a.name == "files_reveal"));
            }
            "files_save_as" => {
                let s = self.file_sel()?;
                let c = Confirm::new(format!("Save {} where you choose?", s.name), core_call("download", json!({ "get": self.download_route(&s)?, "name": s.name, "to": "ask", "after": "nothing" }), format!("save {}", s.name)))
                    .line(format!("From {}: {}", self.sandbox()?, self.abs_path(cx, &s)))
                    .line(format!("{} · {}", view::size_label(s.size), view::kind_label(&s.name)))
                    .line("Next, the system's save dialog: the name is cleaned by the app, and nothing is written over a file without the dialog asking.")
                    .labels("Choose where", "Cancel");
                return Ok(vec![c.into_action()]);
            }
            "files_open_with" => {
                let s = self.file_sel()?;
                // CORE: a Prepare whose read is the core's download and sniff; its
                // verdict (refuse, warn, plain) builds the dialog. Until then the
                // dialog says what will happen and the core refuses the operation.
                let c = Confirm::new(format!("Open {} with this computer's app?", s.name), core_call("download", json!({ "get": self.download_route(&s)?, "name": s.name, "to": "downloads", "after": "open" }), format!("open {}", s.name)))
                    .line(format!("From {}: {}", self.sandbox()?, self.abs_path(cx, &s)))
                    .line(format!("{} · {} (by its name; the app checks its first bytes before opening it)", view::size_label(s.size), view::kind_label(&s.name)))
                    .line("A copy is saved in Downloads, then opened with the app this computer uses for its type. Programs, scripts, installers and shortcuts are never opened.")
                    .labels("Save and open", "Cancel");
                return Ok(vec![c.into_action()]);
            }
            "files_delete" => return Ok(vec![self.delete(cx)?]),
            "files_star" => {
                let sb = self.sandbox()?;
                let p = a.req("path").to_string();
                let mut all = cx.get("app")["kept"]["files"]["starred"].as_object().cloned().unwrap_or_default();
                let mut mine: Vec<String> = self.starred(cx);
                if mine.contains(&p) {
                    mine.retain(|x| *x != p);
                } else {
                    mine.push(p);
                }
                all.insert(sb, json!(mine));
                out.push(Action::Remember { key: "starred".into(), value: J::Object(all) });
            }
            "files_refresh" => {
                let sb = self.sandbox()?;
                out.extend(self.fetch_all(&sb));
            }
            "files_upload_pick" => {
                let sb = self.sandbox.clone().unwrap_or_else(|| "a sandbox".into());
                out.push(Action::PickFiles { title: format!("Upload a file into {sb}"), extensions: vec![], multiple: true });
            }
            "files_upload_paste" => out.push(Action::ReadClipboard),
            "files_upload_remove" => match a.i("index") {
                Some(i) if (i as usize) <= self.staged.len() => {
                    self.staged.remove(i as usize - 1);
                }
                Some(i) => return Err(format!("there is no file {i} waiting")),
                None => self.staged.clear(),
            },
            "files_upload_send" => return Ok(vec![self.upload(cx, a.s("sandbox"))?]),
            "codemap_open" => {
                let sb = a.req("sandbox").to_string();
                cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
                let lens = std::mem::take(&mut self.map.lens);
                self.map = MapState { sandbox: Some(sb.clone()), want: a.s("map").map(str::to_string), loading: true, gen: self.map.gen + 1, lens, ..MapState::default() };
                out.push(Action::OpenPanel(MAP_SCREEN.into()));
                if self.asked_for.get(ARTIFACTS) == Some(&sb) && cx.get(ARTIFACTS).is_object() && self.map.want.is_some() {
                    out.extend(self.load_map(cx));
                } else {
                    out.push(self.fetch(ARTIFACTS, &sb, "/artifacts"));
                }
            }
            "codemap_doc" => {
                let d = a.req("doc").to_string();
                let doc = self.map.docs.get(&d).ok_or_else(|| format!("{d} is not a file of this map"))?;
                let anchor = a.s("anchor").filter(|x| !x.is_empty()).map(str::to_string);
                if let Some(x) = &anchor {
                    if !doc.sections.iter().any(|s| &s.id == x) {
                        return Err(format!("{d} has no section {x}"));
                    }
                }
                self.map.current = Some(d);
                self.map.anchor = anchor;
            }
            "codemap_link" => {
                let url = a.req("url");
                match codemap::parse_panel_url(url) {
                    Some((d, anchor)) => {
                        if !self.map.docs.contains_key(&d) {
                            return Err(format!("{d} is not a file of this map"));
                        }
                        self.map.current = Some(d);
                        self.map.anchor = anchor;
                    }
                    None => out.push(Action::OpenLink(url.to_string())),
                }
            }
            "codemap_filter" => self.map.filter = a.s("text").unwrap_or("").into(),
            "codemap_generate" => {
                let sb = a.req("sandbox");
                cx.sandbox(sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
                self.finished.remove(sb);
                out.push(Action::Call(Call::post(sb_route(sb, "/codemap"), json!({}), format!("write {sb}'s code map")).then(&[RUNS]).tag(&format!("generate:{sb}"))));
            }
            "codemap_lens_set" => self.map.lens = a.s("text").unwrap_or("").into(),
            "codemap_lens" => {
                let sb = a.req("sandbox");
                cx.sandbox(sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
                let objective = self.map.lens.trim().to_string();
                if objective.is_empty() {
                    return Err("say who the lens is for first (a product owner: what each part delivers...)".into());
                }
                if objective.chars().count() > 400 {
                    return Err(format!("a lens is for one reader, in a sentence: {} characters, 400 at most", objective.chars().count()));
                }
                self.finished.remove(sb);
                out.push(Action::Call(Call::post(sb_route(sb, "/codemap/lens"), json!({ "objective": objective }), format!("write a lens of {sb}'s map")).then(&[RUNS]).tag(&format!("lens:{sb}"))));
            }
            "codemap_diagram" => {
                let sb = self.map.sandbox.clone().ok_or("open a code map first")?;
                let (path, size) = self.map.diagram.clone().ok_or("this map has no diagram")?;
                let name = path.rsplit('/').next().unwrap_or("diagram.html").to_string();
                let get = format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(&path));
                let c = Confirm::new(format!("Open {sb}'s map diagram in your browser?"), core_call("download", json!({ "get": get, "name": name, "to": "downloads", "after": "open" }), format!("open {name}")))
                    .line(format!("{sb}: .sbxw-artifacts/{path} · {}", view::size_label(size)))
                    .line("It is an HTML page an agent wrote. The app does not draw HTML: a copy is saved in Downloads and your browser opens it from there, running its scripts as a local file.")
                    .ack("I understand it runs in my browser")
                    .labels("Save and open", "Cancel");
                return Ok(vec![c.into_action()]);
            }
            other => return Err(format!("{other}: not a files intent")),
        }
        out.push(self.ui_action(cx));
        out.push(self.map_ui_action(cx));
        Ok(out)
    }

    fn starred(&self, cx: &Ctx) -> Vec<String> {
        let sb = self.sandbox.clone().unwrap_or_default();
        cx.get("app")["kept"]["files"]["starred"][sb.as_str()].as_array().map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    fn fetch(&mut self, key: &'static str, sb: &str, rest: &str) -> Action {
        self.asked_for.insert(key, sb.to_string());
        Action::Fetch { key: key.into(), path: sb_route(sb, rest) }
    }

    fn fetch_dir(&mut self, sb: &str) -> Action {
        let q = if self.path.is_empty() { String::new() } else { format!("?path={}", enc(&self.path)) };
        self.fetch(DIR, sb, &format!("/fs{q}"))
    }

    fn fetch_all(&mut self, sb: &str) -> Vec<Action> {
        vec![self.fetch(ARTIFACTS, sb, "/artifacts"), self.fetch_dir(sb), self.fetch(RECENT, sb, "/fs/recent")]
    }

    fn browse(&mut self, path: &str) -> Result<Vec<Action>, String> {
        let sb = self.sandbox()?;
        if !self.path.is_empty() && self.path != path {
            self.history.push(self.path.clone());
        }
        self.path = path.to_string();
        self.tab = "files".into();
        self.filter.clear();
        self.sel = None;
        self.preview = J::Null;
        Ok(vec![self.fetch_dir(&sb)])
    }

    fn file_sel(&self) -> Result<Sel, String> {
        let s = self.sel.clone().ok_or("pick a file first")?;
        if s.kind != "file" {
            return Err(format!("{} is not a file", s.name));
        }
        Ok(s)
    }

    /// The path as the agent sees it (the workspace is mounted at its own path).
    fn abs_path(&self, cx: &Ctx, s: &Sel) -> String {
        match s.src {
            Src::Fs => s.path.clone(),
            Src::Artifact => {
                let dir = cx.get(ARTIFACTS)["dir"].as_str().unwrap_or(".sbxw-artifacts");
                view::join(dir, &s.path)
            }
        }
    }

    fn download_route(&self, s: &Sel) -> Result<String, String> {
        let sb = self.sandbox()?;
        Ok(match s.src {
            Src::Artifact => format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(&s.path)),
            Src::Fs => format!("{}?path={}&download=1", sb_route(&sb, "/fs/file"), enc(&s.path)),
        })
    }

    fn rows(&self, cx: &Ctx) -> Vec<J> {
        match self.tab() {
            "files" => self.dir_rows(cx),
            "recent" => self.recent_rows(cx),
            _ => self.artifact_rows(cx),
        }
    }

    fn keep(&self, name: &str) -> bool {
        let f = self.filter.trim().to_lowercase();
        f.is_empty() || name.to_lowercase().contains(&f)
    }

    fn artifact_rows(&self, cx: &Ctx) -> Vec<J> {
        let now = view::now_secs();
        cx.get(ARTIFACTS)["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| self.keep(e["name"].as_str().unwrap_or("")))
            .map(|e| {
                let path = e["path"].as_str().unwrap_or("");
                let m = e["modified"].as_u64().unwrap_or(0);
                let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                json!({ "id": path, "name": e["name"], "kind": "file", "icon": icon_of(e["name"].as_str().unwrap_or("")), "dir": dir,
                    "size": view::size_label(e["size"].as_u64().unwrap_or(0)), "when": view::ago(m, now), "fresh": now.saturating_sub(m) < view::FRESH_SECS,
                    "selected": self.sel.as_ref().is_some_and(|s| s.src == Src::Artifact && s.path == path) })
            })
            .collect()
    }

    fn dir_rows(&self, cx: &Ctx) -> Vec<J> {
        let d = cx.get(DIR);
        let base = d["path"].as_str().unwrap_or("/");
        let now = view::now_secs();
        let mut v: Vec<&J> = d["entries"].as_array().into_iter().flatten().filter(|e| {
            let n = e["name"].as_str().unwrap_or("");
            (self.hidden || !n.starts_with('.')) && self.keep(n)
        }).collect();
        v.sort_by_key(|e| (e["kind"] != "dir", e["name"].as_str().unwrap_or("").to_lowercase()));
        v.into_iter()
            .map(|e| {
                let name = e["name"].as_str().unwrap_or("");
                let id = view::join(base, name);
                let kind = e["kind"].as_str().unwrap_or("other");
                let m = e["modified"].as_u64().unwrap_or(0);
                json!({ "id": id, "name": name, "kind": kind, "link": e["link"] == true, "icon": if kind == "dir" { "folder" } else { icon_of(name) },
                    "size": if kind == "dir" { String::new() } else { view::size_label(e["size"].as_u64().unwrap_or(0)) },
                    "when": view::ago(m, now), "fresh": kind == "file" && now.saturating_sub(m) < view::FRESH_SECS,
                    "can_delete": view::can_delete(&id, d) && !(kind == "other" && e["link"] != true),
                    "selected": self.sel.as_ref().is_some_and(|s| s.src == Src::Fs && s.path == id) })
            })
            .collect()
    }

    fn recent_rows(&self, cx: &Ctx) -> Vec<J> {
        let r = cx.get(RECENT);
        let home = r["home"].as_str().unwrap_or("/home/agent");
        let ws = r["workspace"].as_str();
        let now = view::now_secs();
        r["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                let path = f["path"].as_str()?;
                let name = path.rsplit('/').next().unwrap_or(path);
                if !self.keep(name) {
                    return None;
                }
                let m = f["modified"].as_u64().unwrap_or(0);
                let age = now.saturating_sub(m);
                let group = if age < view::FRESH_SECS { "Last hour" } else if age < 86_400 { "Today" } else { "This week" };
                Some(json!({ "id": path, "name": name, "kind": "file", "icon": icon_of(name), "dir": view::short_dir(path, ws, home),
                    "size": view::size_label(f["size"].as_u64().unwrap_or(0)), "when": view::ago(m, now), "group": group, "fresh": age < view::FRESH_SECS,
                    "can_delete": view::can_delete(path, cx.get(DIR)), "selected": self.sel.as_ref().is_some_and(|s| s.src == Src::Fs && s.path == path) }))
            })
            .collect()
    }

    fn select(&mut self, cx: &Ctx, id: &str) -> Result<Vec<Action>, String> {
        let sb = self.sandbox()?;
        let (src, name, size, modified, kind) = match self.tab() {
            "artifacts" => {
                let e = cx.get(ARTIFACTS)["entries"].as_array().into_iter().flatten().find(|e| e["path"] == id).ok_or_else(|| format!("{id} is not in .sbxw-artifacts"))?;
                (Src::Artifact, e["name"].as_str().unwrap_or("").to_string(), e["size"].as_u64().unwrap_or(0), e["modified"].as_u64().unwrap_or(0), "file".to_string())
            }
            "recent" => {
                let f = cx.get(RECENT)["files"].as_array().into_iter().flatten().find(|f| f["path"] == id).ok_or_else(|| format!("{id} is not in Recent"))?;
                (Src::Fs, id.rsplit('/').next().unwrap_or(id).to_string(), f["size"].as_u64().unwrap_or(0), f["modified"].as_u64().unwrap_or(0), "file".to_string())
            }
            _ => {
                let d = cx.get(DIR);
                let base = d["path"].as_str().unwrap_or("/");
                let e = d["entries"].as_array().into_iter().flatten().find(|e| view::join(base, e["name"].as_str().unwrap_or("")) == id).ok_or_else(|| format!("{id} is not in this folder"))?;
                let kind = e["kind"].as_str().unwrap_or("other").to_string();
                if kind == "dir" {
                    return self.browse(id);
                }
                (Src::Fs, e["name"].as_str().unwrap_or("").to_string(), e["size"].as_u64().unwrap_or(0), e["modified"].as_u64().unwrap_or(0), kind)
            }
        };
        let mut out = vec![];
        if let Some(k) = self.preview["key"].as_str() {
            out.push(Action::Image { key: k.to_string(), bytes: vec![] });
        }
        let s = Sel { src, path: id.to_string(), name: name.clone(), size, modified, kind: kind.clone() };
        self.seq += 1;
        if kind != "file" {
            self.preview = json!({ "state": "note", "text": "Not a regular file." });
        } else if let Some(p) = view::preview_without_reading(&name, size) {
            self.preview = p;
        } else if view::kind(&name) == view::Kind::Image && !RAW_READS {
            self.preview = json!({ "state": "note", "text": "Pictures preview once the app reads files byte for byte, which is not in this build yet: download it meanwhile." });
        } else {
            self.preview = json!({ "state": "loading" });
            let route = match (&s.src, view::kind(&name)) {
                (Src::Artifact, _) => format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(&s.path)),
                (Src::Fs, view::Kind::Image) => format!("{}?path={}", sb_route(&sb, "/fs/file"), enc(&s.path)),
                (Src::Fs, _) => format!("{}?path={}&max={}{}", sb_route(&sb, "/fs/file"), enc(&s.path), view::TEXT_MAX, if size > view::TEXT_MAX { "&head=1" } else { "" }),
            };
            out.push(Action::Call(Call::get(route, format!("read {name}")).tag(&format!("preview:{}", self.seq)).quiet()));
        }
        self.sel = Some(s);
        Ok(out)
    }

    /// A preview's bytes arrived (the raw read; for now the JSON path).
    pub fn got_preview(&mut self, seq: u64, bytes: &[u8]) -> Vec<Action> {
        let Some(s) = self.sel.clone().filter(|_| seq == self.seq) else { return vec![] };
        if view::kind(&s.name) == view::Kind::Image {
            let limits = crate::picture::Limits { max_bytes: view::IMAGE_MAX as usize, ..crate::picture::Limits::default() };
            return match crate::picture::prepare(bytes, limits, Some(1600), crate::picture::Encode::Png) {
                Ok(p) => {
                    let key = format!("files-preview-{seq}");
                    self.preview = json!({ "state": "image", "key": key, "w": p.width, "h": p.height });
                    vec![Action::Image { key, bytes: p.bytes }]
                }
                Err(e) => {
                    self.preview = json!({ "state": "note", "text": format!("This picture cannot be shown: {e}.") });
                    vec![]
                }
            };
        }
        self.preview = view::text_preview(&s.name, bytes, s.size);
        vec![]
    }

    fn delete(&self, cx: &Ctx) -> Result<Action, String> {
        let sb = self.sandbox()?;
        let s = self.sel.clone().ok_or("pick a file first")?;
        let path = self.abs_path(cx, &s);
        let listing = cx.get(DIR);
        if !listing.is_object() {
            return Err("the sandbox's places are not read yet: try again in a moment".into());
        }
        if !view::can_delete(&path, listing) {
            return Err(format!("{path}: only files under the workspace, a read-write mount, the agent's home or /tmp can be deleted, never those folders themselves, .sbxw-artifacts or .git"));
        }
        let what = if s.kind == "dir" { "folder" } else if s.kind == "file" { "file" } else { "link" };
        let mut c = Confirm::new(format!("Delete the {what} {}?", s.name), Call::post(sb_route(&sb, "/fs/delete"), json!({ "path": path }), format!("delete {}", s.name)).tag("delete"))
            .line(format!("In {sb}: {path}"));
        c = if s.kind == "dir" {
            c.line("Everything in it goes too.")
        } else {
            c.line(format!("{} · modified {}", view::size_label(s.size), view::date(s.modified)))
        };
        Ok(c.line("This cannot be undone.").labels("Delete", "Cancel").danger().into_action())
    }

    fn upload(&self, cx: &Ctx, to: Option<&str>) -> Result<Action, String> {
        let sb = match to {
            Some(s) => s.to_string(),
            None => self.sandbox()?,
        };
        let info = cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
        if info["status"] != "running" {
            return Err(format!("{sb} is not running: start it first, then upload"));
        }
        if self.staged.is_empty() {
            return Err("nothing to upload: drop a file on the panel, paste a picture, or choose a file".into());
        }
        if let Some((s, p)) = self.staged.iter().find_map(|s| s.problem().map(|p| (s, p))) {
            return Err(format!("{} cannot go: {p}", s.name));
        }
        let n = self.staged.len();
        let total: u64 = self.staged.iter().map(|s| s.size).sum();
        let files: Vec<J> = self.staged.iter().enumerate().map(|(i, s)| match &s.payload {
            Payload::Path(p) => json!({ "path": p, "name": s.name, "size": s.size }),
            Payload::Png(_) => json!({ "picture": i + 1, "name": s.name, "size": s.size }),
        }).collect();
        let mut c = Confirm::new(
            format!("Upload {} into {sb}?", if n == 1 { self.staged[0].name.clone() } else { format!("{n} files") }),
            core_call("upload", json!({ "sandbox": sb, "files": files, "type_into_prompt": true }), format!("upload into {sb}")),
        );
        for s in &self.staged {
            c = c.line(format!("{} · {}", s.source(), view::size_label(s.size)));
        }
        if n > 1 {
            c = c.line(format!("{} in all", view::size_label(total)));
        }
        Ok(c.line(format!("Copied into {sb} under /tmp/sbxw-uploads/, a new folder each time; the daemon cleans the names."))
            .line(format!("Their paths are then typed into {sb}'s agent prompt, not submitted: say what they are for, then press Return."))
            .labels("Upload", "Cancel")
            .into_action())
    }

    fn ui_action(&self, cx: &Ctx) -> Action {
        Action::Set { key: UI.into(), value: self.ui(cx) }
    }

    fn map_ui_action(&self, cx: &Ctx) -> Action {
        Action::Set { key: MAP_UI.into(), value: self.map_ui(cx) }
    }

    /// `data.files_ui`, what the Project panel draws.
    pub fn ui(&self, cx: &Ctx) -> J {
        let sb = self.sandbox.clone();
        let mine = |k: &str| self.asked_for.get(k) == sb.as_ref() && sb.is_some();
        let d = cx.get(DIR);
        let errors = &cx.get("app")["errors"];
        let staged: Vec<J> = self.staged.iter().enumerate().map(|(i, s)| json!({ "index": i + 1, "name": s.name, "size": view::size_label(s.size), "from": s.source(), "problem": s.problem() })).collect();
        let sel = self.sel.as_ref().map(|s| json!({
            "id": s.path, "name": s.name, "kind": s.kind, "size": view::size_label(s.size), "type": view::kind_label(&s.name),
            "modified": view::date(s.modified), "path": self.abs_path(cx, s),
            "can_delete": d.is_object() && view::can_delete(&self.abs_path(cx, s), d),
        }));
        let recent_fresh = cx.get(RECENT)["files"].as_array().map(|v| v.iter().filter(|f| view::now_secs().saturating_sub(f["modified"].as_u64().unwrap_or(0)) < view::FRESH_SECS).count()).unwrap_or(0);
        let a = cx.get(ARTIFACTS);
        let run = run_for(cx, sb.as_deref().unwrap_or(""));
        json!({
            "sandbox": sb, "tab": self.tab(), "filter": self.filter, "hidden": self.hidden,
            "path": d["path"], "can_up": d["path"].as_str().is_some_and(|p| p != "/"), "can_back": !self.history.is_empty(),
            "offline": d["offline"] == true,
            "starred": d["path"].as_str().is_some_and(|p| self.starred(cx).iter().any(|x| x == p)),
            "places": if mine(DIR) { json!(view::places(d, &self.starred(cx))) } else { json!([]) },
            "rows": self.rows(cx),
            "loading": { "artifacts": !mine(ARTIFACTS) || a.is_null(), "files": d.is_null(), "recent": cx.get(RECENT).is_null() },
            "errors": { "artifacts": errors[ARTIFACTS], "files": errors[DIR], "recent": errors[RECENT] },
            "artifacts_dir": a["dir"], "has_map": a["codemap"].is_object() || a["lenses"].as_array().is_some_and(|l| !l.is_empty()),
            "lenses": a["lenses"].as_array().map(|l| l.len()).unwrap_or(0),
            "run": run,
            "recent_fresh": recent_fresh, "recent_days": cx.get(RECENT)["days"],
            "selected": sel, "preview": self.preview,
            "staged": staged, "upload_max": view::size_label(view::UPLOAD_MAX),
        })
    }

    pub fn map_ui(&self, cx: &Ctx) -> J {
        let m = &self.map;
        let a = cx.get(ARTIFACTS);
        let sb = m.sandbox.clone().unwrap_or_default();
        let maps = maps_of(a);
        let run = run_for(cx, &sb);
        let writing = run["phase"] == "writing";
        json!({
            "sandbox": m.sandbox, "root": m.root, "loading": m.loading, "error": m.error.clone().or_else(|| cx.get("app")["errors"][ARTIFACTS].as_str().map(str::to_string)),
            "maps": maps, "map": codemap::view(&m.docs, m.current.as_deref(), m.anchor.as_deref(), &m.filter),
            "filter": m.filter, "lens": m.lens, "lens_left": 400i64 - m.lens.chars().count() as i64,
            "diagram": m.diagram.as_ref().map(|(p, s)| json!({ "path": p, "size": view::size_label(*s) })),
            "run": run, "writing": writing,
            "running": cx.sandbox(&sb).is_some_and(|s| s["status"] == "running"),
        })
    }

    /// The artifacts listing arrived for the map's sandbox: read its files.
    fn load_map(&mut self, cx: &Ctx) -> Vec<Action> {
        let Some(sb) = self.map.sandbox.clone() else { return vec![] };
        let a = cx.get(ARTIFACTS);
        let maps = maps_of(a);
        let chosen = self.map.want.as_deref().and_then(|w| maps.iter().find(|m| m["dir"] == w)).or(maps.first()).cloned();
        self.map.gen += 1;
        self.map.docs.clear();
        self.map.current = None;
        self.map.anchor = None;
        self.map.diagram = None;
        self.map.error = None;
        let Some(map) = chosen else {
            self.map.root = None;
            self.map.loading = false;
            return vec![];
        };
        let root = map["dir"].as_str().unwrap_or("codemap").to_string();
        let entries = map["entries"].as_array().cloned().unwrap_or_default();
        let prefix = format!("{root}/diagram/");
        let html: Vec<&J> = entries.iter().filter(|e| e["path"].as_str().is_some_and(|p| p.starts_with(&prefix) && p.to_lowercase().ends_with(".html"))).collect();
        self.map.diagram = html.iter().find(|e| e["path"] == format!("{prefix}architecture.html")).or(html.first()).map(|e| (e["path"].as_str().unwrap_or("").to_string(), e["size"].as_u64().unwrap_or(0)));
        let md: Vec<String> = entries.iter().filter_map(|e| e["path"].as_str()).filter(|p| { let l = p.to_lowercase(); l.ends_with(".md") || l.ends_with(".markdown") }).take(MAP_FILES_MAX).map(str::to_string).collect();
        self.map.root = Some(root);
        self.map.waiting = md.len();
        self.map.loading = !md.is_empty();
        let gen = self.map.gen;
        md.iter()
            .map(|p| Action::Call(Call::get(format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(p)), format!("read {p}")).tag(&format!("doc:{gen}:{p}")).quiet()))
            .collect()
    }

    fn got_doc(&mut self, gen: u64, path: &str, bytes: &[u8]) {
        if gen != self.map.gen {
            return;
        }
        let root = self.map.root.clone().unwrap_or_default();
        let rel = path.strip_prefix(&format!("{root}/")).unwrap_or(path);
        let key = rel.strip_suffix(".md").or_else(|| rel.strip_suffix(".markdown")).unwrap_or(rel).to_string();
        let text = String::from_utf8_lossy(bytes);
        self.map.docs.insert(key.clone(), codemap::parse_doc(&key, path, &text));
        self.doc_done();
    }

    fn doc_done(&mut self) {
        self.map.waiting = self.map.waiting.saturating_sub(1);
        if self.map.waiting == 0 {
            self.map.loading = false;
            // The index is the map's front door: the file named as its folder.
            let stem = self.map.root.as_deref().and_then(|r| r.rsplit('/').next()).unwrap_or("codemap").to_string();
            self.map.current = if self.map.docs.contains_key(&stem) { Some(stem) } else { self.map.docs.keys().next().cloned() };
        }
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let mut out = vec![];
        let tag = call.tag.as_str();
        if let Some(seq) = tag.strip_prefix("preview:").and_then(|s| s.parse::<u64>().ok()) {
            match r {
                Ok((c, body)) if (200..300).contains(&c) => out.extend(self.got_preview(seq, &interim_bytes(&body))),
                Ok((c, body)) if seq == self.seq => self.preview = json!({ "state": "error", "text": why(c, &body) }),
                Err(e) if seq == self.seq => self.preview = json!({ "state": "error", "text": e }),
                _ => {}
            }
        } else if let Some(rest) = tag.strip_prefix("doc:") {
            let (gen, path) = rest.split_once(':').unwrap_or(("0", rest));
            let gen = gen.parse().unwrap_or(0);
            match r {
                Ok((c, body)) if (200..300).contains(&c) => self.got_doc(gen, path, &interim_bytes(&body)),
                Ok((c, body)) if gen == self.map.gen => {
                    self.map.error = Some(format!("{path}: {}", why(c, &body)));
                    self.doc_done();
                }
                Err(e) if gen == self.map.gen => {
                    self.map.error = Some(format!("{path}: {e}"));
                    self.doc_done();
                }
                _ => {}
            }
        } else if tag == "delete" {
            match r {
                Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
                    out.push(Action::notice(format!("Deleted {}", body["path"].as_str().or(call.body["path"].as_str()).unwrap_or("it"))));
                    self.sel = None;
                    self.preview = J::Null;
                    if let Some(sb) = self.sandbox.clone() {
                        out.extend(self.fetch_all(&sb));
                    }
                }
                Ok((c, body)) => out.push(Action::error(format!("Could not delete it: {}", why(c, &body)))),
                Err(e) => out.push(Action::error(format!("Could not delete it: {e}"))),
            }
        } else if let Some((what, sb)) = tag.split_once(':').filter(|(w, _)| matches!(*w, "generate" | "lens")) {
            match r {
                Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
                    if what == "lens" {
                        self.map.lens.clear();
                    }
                    if !self.finished.remove(sb) {
                        out.push(Action::notice(if what == "lens" { format!("{sb} is writing a lens of its map: a few minutes") } else { format!("{sb} is writing its code map: a few minutes") }));
                    }
                }
                // The daemon's refusal says why (stopped, no codemap kit, one at a time).
                Ok((c, body)) => out.push(Action::error(format!("{sb}: {}", why(c, &body)))),
                Err(e) => out.push(Action::error(format!("{sb}: {e}"))),
            }
        } else {
            // The core's own operations (downloads, uploads): their answer, or why not.
            match r {
                Ok((c, body)) if (200..300).contains(&c) => {
                    if let Some(p) = body["saved"].as_str().or(body["path"].as_str()) {
                        out.push(Action::notice(format!("{}: {p}", call.what)));
                    }
                    if call.tag == "upload" {
                        self.staged.clear();
                    }
                }
                Ok((c, body)) => out.push(Action::error(format!("{}: {}", call.what, why(c, &body)))),
                Err(e) => out.push(Action::error(format!("{}: {e}", call.what))),
            }
        }
        out.push(self.ui_action(cx));
        out.push(self.map_ui_action(cx));
        out
    }

    pub fn on_data(&mut self, cx: &Ctx, key: &str, _old: &J, new: &J) -> Vec<Action> {
        let mut out = vec![];
        if key == DIR && self.sandbox.is_some() {
            // The first listing (the workspace) goes on to its deliverables
            // folder, when it has one, as the web UI's panel opens.
            if std::mem::take(&mut self.first) && self.path.is_empty() {
                if let Some(a) = new["artifacts"].as_str().filter(|_| new["entries"].as_array().is_some_and(|e| e.iter().any(|x| x["name"] == ".sbxw-artifacts" && x["kind"] == "dir"))) {
                    self.path = a.to_string();
                    let sb = self.sandbox.clone().unwrap();
                    out.push(self.fetch_dir(&sb));
                    out.push(self.ui_action(cx));
                    return out;
                }
            }
            // A row asked for (Kit logs, Show in folder), else a README.
            let want = self.select_next.take();
            if self.tab() == "files" && self.sel.is_none() {
                let names: Vec<&str> = new["entries"].as_array().into_iter().flatten().filter(|e| e["kind"] == "file").filter_map(|e| e["name"].as_str()).collect();
                let pick = want.as_deref().filter(|w| names.contains(w)).or_else(|| names.iter().copied().find(|n| n.eq_ignore_ascii_case("readme.md")));
                if let (Some(p), Some(base)) = (pick, new["path"].as_str()) {
                    if let Ok(a) = self.select(cx, &view::join(base, p)) {
                        out.extend(a);
                    }
                }
            }
        }
        if key == ARTIFACTS && self.map.loading && self.map.sandbox.is_some() && self.asked_for.get(ARTIFACTS) == self.map.sandbox.as_ref() {
            out.extend(self.load_map(cx));
        }
        out.push(self.ui_action(cx));
        out.push(self.map_ui_action(cx));
        out
    }

    pub fn on_event(&mut self, cx: &Ctx, name: &str, run: &J) -> Vec<Action> {
        if name != "codemap" {
            return vec![];
        }
        let sb = run["sandbox"].as_str().unwrap_or("").to_string();
        let mut out = vec![];
        let lens = run["kind"] == "lens";
        match run["phase"].as_str() {
            Some("done") => {
                self.finished.insert(sb.clone());
                out.push(Action::Notice {
                    text: format!("{sb} {}", if lens { "wrote a lens of its map" } else { "wrote its code map" }),
                    error: false,
                    then: Some(("codemap_open".into(), json!({ "sandbox": sb }))),
                });
                // What it has changed: the panels showing it read again.
                if self.sandbox.as_deref() == Some(&sb) || self.map.sandbox.as_deref() == Some(&sb) {
                    if self.map.sandbox.as_deref() == Some(&sb) {
                        self.map.loading = true;
                    }
                    out.push(self.fetch(ARTIFACTS, &sb, "/artifacts"));
                }
            }
            Some("failed") => out.push(Action::error(format!("{sb}: {} ended without one{}", if lens { "the lens" } else { "the code map" }, run["note"].as_str().map(|n| format!(": {n}")).unwrap_or_default()))),
            _ => {}
        }
        out.push(self.ui_action(cx));
        out.push(self.map_ui_action(cx));
        out
    }

    pub fn on_files(&mut self, cx: &Ctx, inc: Incoming) -> Vec<Action> {
        let mut notes = vec![];
        match inc {
            Incoming::Files(paths) => {
                for p in paths {
                    if self.staged.len() >= view::STAGED_MAX {
                        notes.push(format!("{} files at a time at most: the rest were left out", view::STAGED_MAX));
                        break;
                    }
                    match std::fs::metadata(&p) {
                        Ok(m) if m.is_file() => {
                            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "upload".into());
                            self.staged.push(Staged { name, size: m.len(), payload: Payload::Path(p) });
                        }
                        Ok(_) => notes.push(format!("{}: a folder; drop the files in it instead", p.display())),
                        Err(e) => notes.push(format!("{}: {e}", p.display())),
                    }
                }
            }
            Incoming::Png(bytes) => {
                if self.staged.len() >= view::STAGED_MAX {
                    notes.push(format!("{} files at a time at most", view::STAGED_MAX));
                } else {
                    self.paste_seq += 1;
                    self.staged.push(Staged { name: format!("pasted-{}.png", self.paste_seq), size: bytes.len() as u64, payload: Payload::Png(bytes) });
                }
            }
        }
        let mut out: Vec<Action> = notes.into_iter().map(Action::error).collect();
        out.push(Action::OpenPanel(SCREEN.into()));
        out.push(self.ui_action(cx));
        out
    }
}

fn icon_of(name: &str) -> &'static str {
    match view::kind(name) {
        view::Kind::Markdown | view::Kind::Text => "file",
        view::Kind::Image => "sparkle",
        view::Kind::Html => "link",
        view::Kind::Other => "file",
    }
}

/// Every map of the listing, the code map first, then its lenses (newest first, as listed).
fn maps_of(a: &J) -> Vec<J> {
    let now = view::now_secs();
    let mut v = vec![];
    if a["codemap"].is_object() {
        let m = &a["codemap"];
        v.push(json!({ "dir": m["dir"], "label": "Code map", "lens": false, "files": m["files"], "when": view::ago(m["modified"].as_u64().unwrap_or(0), now), "entries": m["entries"] }));
    }
    for l in a["lenses"].as_array().into_iter().flatten() {
        v.push(json!({ "dir": l["dir"], "label": format!("Lens: {}", l["slug"].as_str().unwrap_or("").replace('-', " ")), "lens": true, "files": l["files"], "when": view::ago(l["modified"].as_u64().unwrap_or(0), now), "entries": l["entries"] }));
    }
    v
}

/// The sandbox's code map run, from `GET /api/codemap` (null: none).
fn run_for(cx: &Ctx, sb: &str) -> J {
    let r = cx.get(RUNS).as_array().and_then(|v| v.iter().find(|r| r["sandbox"] == sb)).cloned().unwrap_or(J::Null);
    if r.is_null() {
        return r;
    }
    let lens = r["kind"] == "lens";
    let label = match r["phase"].as_str() {
        Some("writing") => if lens { "Writing a lens…" } else { "Writing the map…" }.to_string(),
        Some("failed") => format!("The last {} ended without one{}", if lens { "lens" } else { "map" }, r["note"].as_str().map(|n| format!(": {n}")).unwrap_or_default()),
        _ => String::new(),
    };
    json!({ "phase": r["phase"], "kind": r["kind"], "note": r["note"], "label": label })
}

fn extras(cx: &Ctx) -> Vec<Extra> {
    let mut v = vec![];
    for sb in cx.get("sandboxes").as_array().into_iter().flatten() {
        let Some(name) = sb["name"].as_str() else { continue };
        v.push(Extra {
            place: Place::Pane(name.into()),
            id: "project".into(),
            label: "Project".into(),
            icon: Some("folder".into()),
            tone: "muted".into(),
            help: "Its deliverables, files, uploads and code map".into(),
            count: None,
            intent: Some(("files_open".into(), json!({ "sandbox": name }))),
        });
        if run_for(cx, name)["phase"] == "writing" {
            v.push(Extra {
                place: Place::Sandbox(name.into()),
                id: "codemap".into(),
                label: "map…".into(),
                icon: None,
                tone: "amber".into(),
                help: "Writing its code map in a background session".into(),
                count: None,
                intent: Some(("files_open".into(), json!({ "sandbox": name }))),
            });
        }
    }
    v
}
