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
//! The core's file operations (`src/files.rs`) do the writing, opening,
//! revealing and streaming; their names and verdicts are the core's
//! (`src/safe.rs`), never this feature's.

pub mod codemap;
pub mod drops;
pub mod explorer;
mod standin;
#[cfg(test)]
mod tests;
pub mod view;

use super::{core_call, Action, After, Asked, Call, Confirm, Ctx, Data, Dest, Download, Extra, Feature, Incoming, PaneDrop, Place, Prepare, ScreenDecl, Slot};
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
        ScreenDecl { name: SCREEN, src: include_str!("../../../screens/files.luau"), slot: Slot::Overlay { title: "Files", icon: "folder" } },
        ScreenDecl { name: MAP_SCREEN, src: include_str!("../../../screens/codemap.luau"), slot: Slot::Panel { title: "Code map", icon: "file", width: 520.0 } },
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
    on_bytes: Some(on_bytes),
    on_pane_drop: Some(on_pane_drop),
    on_dismiss: Some(on_dismiss),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "files_open", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("tab", Arg::OneOf(TABS), false), ("worktree", Arg::Str, false)], doc: "Opens the file explorer over the panes on that sandbox (its deliverables first, or `tab`; `worktree`: at that thread's worktree, as the daemon lists it)." },
    Spec { name: "files_show", risk: Risk::Direct, args: &[], doc: "Opens the file explorer on the sandbox used last, else the focused pane's, else the selected one." },
    Spec { name: "files_link", risk: Risk::Direct, args: &[("url", Arg::Url, true)], doc: "A link pressed in a file read in the explorer: a relative link or a `[[wiki link]]` is followed in the explorer (its folder listed, the file selected), any other opens in the browser." },
    Spec { name: "files_outline", risk: Risk::Direct, args: &[("index", Arg::Int(0, 10_000), false)], doc: "Shows one heading's section of the Markdown file read (`index` in its outline), or the whole file (none)." },
    Spec { name: "files_tab", risk: Risk::Direct, args: &[("tab", Arg::OneOf(TABS), true)], doc: "Shows the Artifacts, Files or Recent tab." },
    Spec { name: "files_place", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Goes to one of the places (`recent`, or a place's path as listed in `files_ui.places`)." },
    Spec { name: "files_browse", risk: Risk::Direct, args: &[("path", Arg::Str, true)], doc: "Lists that folder of the sandbox (`~` is the agent's home). The daemon reads it as the agent sees it." },
    Spec { name: "files_up", risk: Risk::Direct, args: &[], doc: "Lists the parent folder." },
    Spec { name: "files_back", risk: Risk::Direct, args: &[], doc: "Back to the folder listed before." },
    Spec { name: "files_filter", risk: Risk::Direct, args: &[("text", Arg::Text, false)], doc: "Filters the rows by name; give it on every edit." },
    Spec { name: "files_more", risk: Risk::Direct, args: &[], doc: "Draws the next rows of a long list (the panel draws 200 at first, 200 more each time)." },
    Spec { name: "files_fold", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Artifacts: opens a folder's rows, or folds them back (`id`: a folder row's id). Folders start folded, with their file count." },
    Spec { name: "files_hidden", risk: Risk::Direct, args: &[("on", Arg::Bool, true)], doc: "Shows dotfiles, or hides them." },
    Spec { name: "files_expand", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Opens a folder of the list in place (its entries under it, read from the sandbox), or folds it back. Folders start folded. Artifacts: as files_fold." },
    Spec { name: "files_sort", risk: Risk::Direct, args: &[("by", Arg::OneOf(SORTS), true)], doc: "Sorts a folder's rows by name or by date (newest first); folders first either way." },
    Spec { name: "files_key", risk: Risk::Direct, args: &[("key", Arg::OneOf(KEYS), true), ("text", Arg::Text, false)], doc: "The explorer's keys when none of its controls has the keyboard: Up and Down move in the list, Right opens a folder in place, Left folds it (or goes to its folder's row), Enter opens a folder or reads a file; typing (`type` with `text`, `backspace`) edits the filter." },
    Spec { name: "files_select", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Picks a row by its `id`: a folder is listed, a file previewed (text, Markdown, pictures; HTML as its source)." },
    Spec { name: "files_show_in_folder", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Recent: lists the folder a file was written to, on that file." },
    Spec { name: "files_copy_path", risk: Risk::Direct, args: &[], doc: "Puts the selected file's path, as the agent sees it, on the clipboard." },
    Spec { name: "files_download", risk: Risk::Direct, args: &[], doc: "Downloads the selected file into this computer's Downloads folder, under a name the core cleans (never over a file there)." },
    Spec { name: "files_reveal", risk: Risk::Direct, args: &[], doc: "Downloads the selected file into Downloads and shows it in the file manager." },
    Spec { name: "files_show_saved", risk: Risk::Direct, args: &[], doc: "Shows the last file the panel saved in the file manager." },
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
        risk: Risk::Approval("Upload a file into a sandbox"),
        args: &[("index", Arg::Int(1, 10), false)],
        doc: "Copies waiting file `index` (default the first) into its sandbox (the panel's, or the pane's it was dropped on), then types its path into that agent's prompt, not submitted; the core's dialog shows the local path and size first. 128 MB at most.",
    },
    Spec {
        name: "files_upload_dropped",
        risk: Risk::Approval("Upload a file into a sandbox"),
        args: &[],
        doc: "The files dropped together on a terminal pane: ONE core dialog lists them (local paths, sizes, total, the sandbox and session), then they are copied in one after the other (a failure stops the rest) and their paths typed into that prompt, not submitted. Its box \"Do not ask again for files I drop\" is the only way to skip it; a drop that looks like secrets, comes from ~/.ssh, ~/.aws or ~/.gnupg, holds more than 10 files or more than 25 MB is asked all the same.",
    },
    Spec { name: "files_upload_ask_again", risk: Risk::Direct, args: &[], doc: "Asks again before uploading files dropped on a terminal (undoes \"Do not ask again\"). Nothing can turn the dialog off but its own box." },
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
    /// A picture from the clipboard (PNG), sent to `paste-image`.
    Png(Vec<u8>),
}

#[derive(Debug, Clone)]
struct Staged {
    /// Its upload's tag (`upload-<id>`; progress in `data.app.uploads`).
    id: u64,
    name: String,
    size: u64,
    payload: Payload,
    /// Dropped on a pane: that sandbox and thread (else the panel's sandbox, the main session).
    to: Option<(String, Option<String>)>,
}

impl Staged {
    fn problem(&self) -> Option<String> {
        if self.size == 0 {
            Some("empty: the daemon takes no empty file".into())
        } else if self.size > view::UPLOAD_MAX {
            Some(format!("too large: {} bytes, the daemon takes {} at most", self.size, view::size_label(view::UPLOAD_MAX)))
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
    /// The next artifacts answer for this sandbox is read as the map again.
    reload: bool,
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
    /// Pages of rows drawn beyond the first ([`PAGE`] each): reset by a new
    /// tab, filter or folder.
    more: usize,
    /// Artifact folders opened (folded by default).
    open: std::collections::BTreeSet<String>,
    sel: Option<Sel>,
    preview: J,
    seq: u64,
    staged: Vec<Staged>,
    paste_seq: u64,
    staged_seq: u64,
    drop_seq: u64,
    /// Files dropped on a terminal pane, asked one after the other (never
    /// shown as waiting: a Cancel leaves nothing behind once the next drop
    /// replaces them, and nothing goes without its dialog).
    dropped: Vec<Staged>,
    /// That drop's batch: its tag (`drop-<n>`), and why its dialog asks even
    /// when the person chose not to be asked ([`drops::guardrails`]).
    drop_tag: String,
    drop_why: Vec<String>,
    /// The person chose not to be asked (as the core said with the drop).
    drop_skip: bool,
    /// The last file the panel saved (Show in folder).
    saved: Option<(String, String)>,
    /// The sandbox each daemon answer was asked for.
    asked_for: BTreeMap<&'static str, String>,
    /// Sandboxes whose run ended before the answer to its start came back
    /// (then "is writing" would be stale).
    finished: std::collections::HashSet<String>,
    map: MapState,
    /// A thread's worktree to list once the sandbox's places are read.
    want_worktree: Option<String>,
    /// The Markdown section shown (`files_outline`), by heading index.
    section: Option<usize>,
    /// Folders of the list opened in place (absolute paths), and what the
    /// daemon answered for each (its listing, or `{error}`).
    expanded: std::collections::BTreeSet<String>,
    children: BTreeMap<String, J>,
    /// `name` (default) or `date`.
    sort: String,
    /// The keyboard's row, by id.
    cursor: Option<String>,
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
fn on_bytes(cx: &Ctx, call: &Call, r: Result<(u16, Vec<u8>, String), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_bytes(cx, call, r)
}
fn on_pane_drop(cx: &Ctx, d: &PaneDrop) -> Vec<Action> {
    STATE().lock().unwrap().on_pane_drop(cx, d)
}
fn on_dismiss(cx: &Ctx, tag: &str) -> Vec<Action> {
    STATE().lock().unwrap().on_dismiss(cx, tag)
}

/// The local paths among these files (a pasted picture has none).
fn paths_of<'a>(v: impl IntoIterator<Item = &'a Staged>) -> Vec<PathBuf> {
    v.into_iter().filter_map(|s| match &s.payload {
        Payload::Path(p) => Some(p.clone()),
        Payload::Png(_) => None,
    }).collect()
}

/// The core forgets these handed-in paths (no longer uploadable); nothing when none.
fn forget(paths: Vec<PathBuf>) -> Option<Action> {
    (!paths.is_empty()).then_some(Action::ForgetFiles(paths))
}

/// The core's own reading of a file it wrote (`{saved, name, bytes, verdict,
/// kind, reason}`, from `safe.rs`) makes the Open dialog: a file the core
/// refuses is said, never offered; one it warns about needs the box ticked.
pub fn build_open(_cx: &Ctx, asked: &Asked, v: &J) -> Result<Confirm, String> {
    let name = v["name"].as_str().unwrap_or("the file");
    let saved = v["saved"].as_str().ok_or("the app did not say where it saved the file")?;
    let kind = v["kind"].as_str().unwrap_or("a file");
    let reason = v["reason"].as_str().unwrap_or("");
    let size = view::size_label(v["bytes"].as_u64().unwrap_or(0));
    let from = asked.s("from").unwrap_or("");
    match v["verdict"].as_str() {
        Some("plain") | Some("warn") => {}
        Some("refuse") => {
            let why = reason.trim().trim_end_matches('.');
            let why = if why.is_empty() { format!("it is {kind}") } else { why.to_string() };
            return Err(format!("{name}: {why}. The app never opens it. A copy is in Downloads, to look at: {saved}"));
        }
        _ => return Err(format!("the app could not tell what {name} is: it is not opened")),
    }
    let mut c = Confirm::new(format!("Open {name} with this computer's app?"), core_call("open", json!({ "path": saved }), format!("open {name}"), "open"))
        .line(format!("From {from}"))
        .line(format!("Saved in Downloads: {saved}"))
        .line(format!("{size} · {kind}"));
    if !reason.is_empty() {
        c = c.line(reason.to_string());
    }
    if v["verdict"] == "warn" {
        c = c.ack(if asked.s("what") == Some("diagram") { "I understand my browser runs this page's scripts" } else { "I understand what opening it can do" });
    }
    Ok(c.labels("Open", "Keep it closed"))
}

/// Where an upload into `sandbox` goes, up to its name (the core checks a
/// dialog-less batch against it).
pub fn upload_route_prefix(sandbox: &str) -> String {
    format!("{}?name=", sb_route(sandbox, "/upload"))
}

fn sb_route(sandbox: &str, rest: &str) -> String {
    format!("/api/sandboxes/{}{rest}", enc(sandbox))
}

fn why(status: u16, body: &J) -> String {
    body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {status}"))
}

impl State {
    /// No preview any more; a picture it showed is forgotten (its decoded
    /// pixels went on taking memory otherwise, up to `picture::MAX_SHOWN` of
    /// them: MEMORY.md).
    fn drop_preview(&mut self) -> Option<Action> {
        let gone = self.preview["key"].as_str().map(|k| Action::Image { key: k.to_string(), bytes: vec![] });
        self.preview = J::Null;
        gone
    }

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
                    *self = State { map: std::mem::take(&mut self.map), staged, dropped: std::mem::take(&mut self.dropped), paste_seq: self.paste_seq, staged_seq: self.staged_seq, seq: self.seq, drop_seq: self.drop_seq, drop_tag: std::mem::take(&mut self.drop_tag), drop_why: std::mem::take(&mut self.drop_why), drop_skip: self.drop_skip, saved: self.saved.take(), ..State::default() };
                    self.sandbox = Some(sb.clone());
                    self.first = true;
                    for k in [ARTIFACTS, DIR, RECENT] {
                        out.push(Action::Set { key: k.into(), value: J::Null });
                    }
                    out.extend(self.fetch_all(&sb));
                }
                if self.sort.is_empty() {
                    self.sort = cx.get("app")["kept"]["files"]["sort"].as_str().filter(|x| SORTS.contains(x)).unwrap_or("name").into();
                }
                match a.s("tab") {
                    Some(t) => self.tab = t.into(),
                    None if self.tab.is_empty() => self.tab = cx.get("app")["kept"]["files"]["tab"].as_str().filter(|t| TABS.contains(t)).unwrap_or("artifacts").into(),
                    None => {}
                }
                if let Some(t) = a.s("worktree") {
                    // Its path as the daemon lists it (never a path from the screen).
                    match worktree_path(cx.get(DIR), t).filter(|_| self.asked_for.get(DIR) == Some(&sb)) {
                        Some(p) => out.extend(self.browse(&p)?),
                        None => {
                            self.want_worktree = Some(t.to_string());
                            self.first = false;
                        }
                    }
                }
                out.push(Action::OpenPanel(SCREEN.into()));
                out.push(Action::Remember { key: "last".into(), value: sb.into() });
            }
            "files_show" => {
                let app = cx.get("app");
                let focused = app["focused"].as_u64().and_then(|f| app["panes"].as_array()?.iter().find(|p| p["index"].as_u64() == Some(f)).and_then(|p| p["sandbox"].as_str()));
                let known = |s: &&str| cx.sandbox(s).is_some();
                let sb = self.sandbox.as_deref().filter(known).or(app["kept"]["files"]["last"].as_str().filter(known)).or(focused.filter(known)).or(app["selected"].as_str().filter(known)).or(cx.get("sandboxes")[0]["name"].as_str())
                    .ok_or("no sandbox to explore yet")?.to_string();
                return self.act(cx, &Asked { name: "files_open", args: json!({ "sandbox": sb }).as_object().cloned().unwrap_or_default() });
            }
            "files_link" => {
                let url = a.req("url");
                match explorer::parse_url(url) {
                    Some(path) => {
                        self.select_next = Some(path.rsplit('/').next().unwrap_or("").to_string());
                        out.extend(self.browse(&view::parent(&path))?);
                        self.select_next = Some(path.rsplit('/').next().unwrap_or("").to_string());
                    }
                    None => out.push(Action::Intent { name: "open_link".into(), args: json!({ "url": url }) }),
                }
            }
            "files_outline" => {
                let n = a.i("index").map(|i| i as usize);
                let text = self.preview["text"].as_str().unwrap_or("");
                if let Some(i) = n {
                    if explorer::section(text, i).is_none() {
                        return Err(format!("the file has no heading {i}"));
                    }
                }
                self.section = n;
            }
            "files_tab" => {
                self.tab = a.req("tab").into();
                self.more = 0;
                self.sel = None;
                out.extend(self.drop_preview());
                out.push(Action::Remember { key: "tab".into(), value: self.tab.clone().into() });
            }
            "files_place" => {
                let id = a.req("id");
                if id == "recent" || id == "artifacts" {
                    // A shared location: its list (as its tab was).
                    self.tab = id.into();
                    self.more = 0;
                    self.sel = None;
                    out.extend(self.drop_preview());
                    self.cursor = None;
                } else {
                    // A location (views2) or a former place: the same ids (paths).
                    let sb = self.sandbox.clone().unwrap_or_default();
                    let mut places = view::places(cx.get(DIR), &self.starred(cx));
                    places.extend(view::locations(cx.get(DIR), &self.starred(cx), &sb).into_iter().flat_map(|g| g["items"].as_array().cloned().unwrap_or_default()).filter(|x| x["path"].is_string()));
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
                out.extend(self.drop_preview());
                self.tab = "files".into();
                self.expanded.clear();
                self.children.clear();
                self.cursor = None;
                out.push(self.fetch_dir(&sb));
            }
            "files_filter" => {
                self.filter = a.s("text").unwrap_or("").into();
                self.more = 0;
            }
            "files_more" => self.more += 1,
            "files_fold" => {
                let id = a.req("id").to_string();
                if !artifact_dirs(cx).contains_key(&id) {
                    return Err(format!("{id} is not a folder of .sbxw-artifacts"));
                }
                if !self.open.remove(&id) {
                    self.open.insert(id);
                }
            }
            "files_hidden" => self.hidden = a.b("on").unwrap_or(false),
            "files_expand" => out.extend(self.expand(cx, a.req("id"))?),
            "files_sort" => {
                self.sort = a.req("by").to_string();
                self.more = 0;
                out.push(Action::Remember { key: "sort".into(), value: self.sort.clone().into() });
            }
            "files_key" => out.extend(self.key(cx, a.req("key"), a.s("text").unwrap_or(""))?),
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
                let after = if a.name == "files_reveal" { After::Reveal } else { After::Nothing };
                out.push(Action::Download(Download { get: self.download_route(&s)?, name: s.name.clone(), to: Dest::Downloads, after, tag: "download".into() }));
            }
            "files_show_saved" => {
                let (_, path) = self.saved.clone().ok_or("nothing saved yet")?;
                out.push(Action::Reveal(PathBuf::from(path)));
            }
            "files_save_as" => {
                let s = self.file_sel()?;
                let d = Download { get: self.download_route(&s)?, name: s.name.clone(), to: Dest::Ask, after: After::Nothing, tag: "download".into() };
                let c = Confirm::new(format!("Save {} where you choose?", s.name), d.call(format!("save {}", s.name)))
                    .line(format!("From {}: {}", self.sandbox()?, self.abs_path(cx, &s)))
                    .line(format!("{} · {}", view::size_label(s.size), view::kind_label(&s.name)))
                    .line("Next, the system's save dialog: the name is cleaned by the app, and nothing is written over a file without the dialog asking.")
                    .labels("Choose where", "Cancel");
                return Ok(vec![c.into_action()]);
            }
            "files_open_with" => {
                let s = self.file_sel()?;
                let sb = self.sandbox()?;
                let d = Download { get: self.download_route(&s)?, name: s.name.clone(), to: Dest::Downloads, after: After::Nothing, tag: "open-read".into() };
                // The core downloads it, then says what it is (its name and first
                // bytes): the dialog is built from that, never from this panel's guess.
                let asked = Asked { name: "files_open_with", args: json!({ "from": format!("{sb}: {}", self.abs_path(cx, &s)), "what": "file" }).as_object().cloned().unwrap_or_default() };
                return Ok(vec![Action::Prepare(Box::new(Prepare { title: format!("Open {}?", s.name), call: d.call(format!("save {}", s.name)), asked, build: build_open }))]);
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
                    let s = self.staged.remove(i as usize - 1);
                    out.extend(forget(paths_of([&s])));
                }
                Some(i) => return Err(format!("there is no file {i} waiting")),
                None => out.extend(forget(paths_of(std::mem::take(&mut self.staged).iter()))),
            },
            "files_upload_send" => {
                let s = self.staged.get((a.i("index").unwrap_or(1) as usize).wrapping_sub(1)).ok_or_else(|| if self.staged.is_empty() { "nothing to upload: drop a file on the panel or a pane, paste a picture, or choose a file".to_string() } else { format!("there is no file {} waiting", a.i("index").unwrap_or(1)) })?;
                return Ok(vec![self.upload(cx, s, None)?]);
            }
            "files_upload_dropped" => return Ok(vec![self.drop_dialog(cx)?]),
            "files_upload_ask_again" => out.push(Action::UploadsAskAgain),
            "codemap_open" => {
                let sb = a.req("sandbox").to_string();
                cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
                let lens = std::mem::take(&mut self.map.lens);
                self.map = MapState { sandbox: Some(sb.clone()), want: a.s("map").map(str::to_string), loading: true, gen: self.map.gen + 1, lens, ..MapState::default() };
                out.push(Action::OpenPanel(MAP_SCREEN.into()));
                // Already read for this sandbox: the map is read from it now (a read
                // bringing the same answer would not come back as new data).
                if self.asked_for.get(ARTIFACTS) == Some(&sb) && cx.get(ARTIFACTS).is_object() {
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
                let (path, _) = self.map.diagram.clone().ok_or("this map has no diagram")?;
                let name = path.rsplit('/').next().unwrap_or("diagram.html").to_string();
                let get = format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(&path));
                let d = Download { get, name: name.clone(), to: Dest::Downloads, after: After::Nothing, tag: "open-read".into() };
                let asked = Asked { name: "codemap_diagram", args: json!({ "from": format!("{sb}: .sbxw-artifacts/{path}"), "what": "diagram" }).as_object().cloned().unwrap_or_default() };
                return Ok(vec![Action::Prepare(Box::new(Prepare { title: format!("Open {sb}'s map diagram?"), call: d.call(format!("save {name}")), asked, build: build_open }))]);
            }
            other => return Err(format!("{other}: not a files intent")),
        }
        out.extend(self.ui_action(cx));
        out.extend(self.map_ui_action(cx));
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
        self.more = 0;
        self.sel = None;
        let mut out: Vec<Action> = self.drop_preview().into_iter().collect();
        self.expanded.clear();
        self.children.clear();
        self.cursor = None;
        out.push(self.fetch_dir(&sb));
        Ok(out)
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

    /// The rows drawn: the first [`PAGE`] (more on "Show more"), and the
    /// selected one, so a screen never builds more than it can
    /// show (the core refuses a render past its element cap). The filter
    /// has already run over every entry. With the count line's numbers.
    fn page(&self, cx: &Ctx) -> (Vec<J>, J) {
        let mut rows = self.rows(cx);
        let total = rows.len();
        let files = rows.iter().map(|r| if r["kind"] == "folder" { r["count"].as_u64().unwrap_or(0) as usize } else { 1 }).sum::<usize>();
        let shown = total.min(PAGE * (1 + self.more));
        // The selected row past the page is drawn after it, alone (never
        // the rows between: a selection must not lift the cap).
        let late = rows.iter().skip(shown).find(|r| r["selected"] == true).cloned();
        rows.truncate(shown);
        rows.extend(late);
        for r in rows.iter_mut() {
            r["cursor"] = (self.cursor.is_some() && r["id"].as_str() == self.cursor.as_deref()).into();
        }
        let what = if self.tab() == "files" { "entries" } else { "files" };
        let count = json!({
            "total": total, "shown": shown, "more": PAGE.min(total - shown),
            "label": if shown < total { format!("{} {what}, showing {}", view::thousands(files), view::thousands(shown)) } else { String::new() },
        });
        (rows, count)
    }

    fn keep(&self, name: &str) -> bool {
        let f = self.filter.trim().to_lowercase();
        f.is_empty() || name.to_lowercase().contains(&f)
    }

    /// Artifacts: with a filter, every matching file, flat, its folder said;
    /// without, a tree in the daemon's order (newest first), each folder one
    /// row with its file count, folded until opened.
    fn artifact_rows(&self, cx: &Ctx) -> Vec<J> {
        let now = view::now_secs();
        let entries: Vec<&J> = cx.get(ARTIFACTS)["entries"].as_array().into_iter().flatten().collect();
        let file = |e: &J, depth: usize, with_dir: bool| {
            let path = e["path"].as_str().unwrap_or("");
            let m = e["modified"].as_u64().unwrap_or(0);
            let dir = if with_dir { path.rsplit_once('/').map(|(d, _)| d).unwrap_or("") } else { "" };
            json!({ "id": path, "name": e["name"], "kind": "file", "icon": icon_of(e["name"].as_str().unwrap_or("")), "dir": dir, "depth": depth,
                "size": view::size_label(e["size"].as_u64().unwrap_or(0)), "when": view::ago(m, now), "fresh": now.saturating_sub(m) < view::FRESH_SECS,
                "selected": self.sel.as_ref().is_some_and(|s| s.src == Src::Artifact && s.path == path) })
        };
        if !self.filter.trim().is_empty() {
            return entries.into_iter().filter(|e| self.keep(e["name"].as_str().unwrap_or(""))).map(|e| file(e, 0, true)).collect();
        }
        let counts = artifact_dirs(cx);
        let is_open = |d: &str| self.open.contains(d);
        let mut out = vec![];
        let mut seen = std::collections::HashSet::new();
        for e in &entries {
            let path = e["path"].as_str().unwrap_or("");
            // The rows this entry calls for, from the top: each folder on its
            // way once (at its newest file), then the file if all are open.
            let mut prefix = String::new();
            let mut visible = true;
            for (depth, seg) in path.split('/').collect::<Vec<_>>().split_last().map(|(_, dirs)| dirs.to_vec()).unwrap_or_default().into_iter().enumerate() {
                let dir = if prefix.is_empty() { seg.to_string() } else { format!("{prefix}/{seg}") };
                if seen.insert(dir.clone()) {
                    out.push(json!({ "id": dir, "name": seg, "kind": "folder", "icon": "folder", "depth": depth,
                        "count": counts.get(&dir).copied().unwrap_or(0), "open": is_open(&dir) }));
                }
                if !is_open(&dir) {
                    visible = false;
                    break;
                }
                prefix = dir;
            }
            if visible {
                out.push(file(e, path.matches('/').count(), false));
            }
        }
        out
    }

    fn dir_rows(&self, cx: &Ctx) -> Vec<J> {
        let d = cx.get(DIR);
        let mut out = vec![];
        self.dir_level(d, d, 0, &mut out);
        out
    }

    /// One folder's rows (`listing`, the daemon's answer for it), folders
    /// first, sorted by name or date; an opened folder's rows under it.
    fn dir_level(&self, top: &J, listing: &J, depth: usize, out: &mut Vec<J>) {
        let base = listing["path"].as_str().unwrap_or("/");
        let now = view::now_secs();
        let visible = |e: &&J| {
            let n = e["name"].as_str().unwrap_or("");
            self.hidden || !n.starts_with('.')
        };
        let mut v: Vec<&J> = listing["entries"].as_array().into_iter().flatten().filter(visible).collect();
        if self.sort == "date" {
            v.sort_by_key(|e| (e["kind"] != "dir", std::cmp::Reverse(e["modified"].as_u64().unwrap_or(0)), e["name"].as_str().unwrap_or("").to_lowercase()));
        } else {
            v.sort_by_key(|e| (e["kind"] != "dir", e["name"].as_str().unwrap_or("").to_lowercase()));
        }
        for e in v {
            let name = e["name"].as_str().unwrap_or("");
            let id = view::join(base, name);
            let kind = e["kind"].as_str().unwrap_or("other");
            let open = kind == "dir" && self.expanded.contains(&id);
            let kids = if open { self.children.get(&id) } else { None };
            let mut sub = vec![];
            if let Some(k) = kids.filter(|k| k["entries"].is_array()) {
                self.dir_level(top, k, depth + 1, &mut sub);
            }
            // The filter keeps a row whose name matches, or an opened folder holding one.
            if !self.keep(name) && sub.is_empty() {
                continue;
            }
            let m = e["modified"].as_u64().unwrap_or(0);
            let count = kids.and_then(|k| k["entries"].as_array()).map(|l| l.iter().filter(|x| self.hidden || !x["name"].as_str().unwrap_or("").starts_with('.')).count());
            out.push(json!({ "id": id, "name": name, "kind": kind, "link": e["link"] == true, "icon": if kind == "dir" { "folder" } else { icon_of(name) },
                "size": if kind == "dir" { String::new() } else { view::size_label(e["size"].as_u64().unwrap_or(0)) },
                "when": view::ago(m, now), "fresh": kind == "file" && now.saturating_sub(m) < view::FRESH_SECS,
                "can_delete": view::can_delete(&id, top) && !(kind == "other" && e["link"] != true),
                "depth": depth, "open": open, "count": count, "loading": open && kids.is_none(),
                "error": kids.and_then(|k| k["error"].as_str()),
                "cursor": self.cursor.as_deref() == Some(id.as_str()),
                "selected": self.sel.as_ref().is_some_and(|s| s.src == Src::Fs && s.path == id) }));
            out.extend(sub);
        }
    }

    /// The listing a row of the Files list belongs to: the folder listed, or
    /// a folder opened in place.
    fn entry_of<'a>(&'a self, cx: &'a Ctx, id: &str) -> Option<&'a J> {
        let parent = view::parent(id);
        let d = cx.get(DIR);
        let l = if d["path"].as_str() == Some(parent.as_str()) { d } else { self.children.get(&parent)? };
        let name = id.rsplit('/').next().unwrap_or(id);
        l["entries"].as_array()?.iter().find(|e| e["name"] == name)
    }

    /// Opens a folder in place, or folds it (`files_expand`).
    fn expand(&mut self, cx: &Ctx, id: &str) -> Result<Vec<Action>, String> {
        if self.tab() == "artifacts" {
            if !artifact_dirs(cx).contains_key(id) {
                return Err(format!("{id} is not a folder of .sbxw-artifacts"));
            }
            if !self.open.remove(id) {
                self.open.insert(id.to_string());
            }
            return Ok(vec![]);
        }
        let e = self.entry_of(cx, id).ok_or_else(|| format!("{id} is not in this folder"))?;
        if e["kind"] != "dir" {
            return Err(format!("{id} is not a folder"));
        }
        if self.expanded.remove(id) {
            // Folded: what was opened inside it folds too.
            let inside = format!("{}/", id.trim_end_matches('/'));
            self.expanded.retain(|p| !p.starts_with(&inside));
            return Ok(vec![]);
        }
        self.expanded.insert(id.to_string());
        if self.children.contains_key(id) {
            return Ok(vec![]);
        }
        let sb = self.sandbox()?;
        Ok(vec![Action::Call(Call::get(format!("{}?path={}", sb_route(&sb, "/fs"), enc(id)), format!("list {id}")).tag(&format!("expand:{sb}:{id}")).quiet())])
    }

    /// The explorer's keys (`files_key`), over the rows drawn.
    fn key(&mut self, cx: &Ctx, key: &str, text: &str) -> Result<Vec<Action>, String> {
        match key {
            "type" => {
                self.filter.push_str(text);
                self.more = 0;
                return Ok(vec![]);
            }
            "backspace" => {
                self.filter.pop();
                self.more = 0;
                return Ok(vec![]);
            }
            _ => {}
        }
        let (rows, _) = self.page(cx);
        let ids: Vec<String> = rows.iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let at = self.cursor.as_ref().and_then(|c| ids.iter().position(|i| i == c));
        let row = |id: &str| rows.iter().find(|r| r["id"] == id).cloned().unwrap_or(J::Null);
        match key {
            "down" => self.cursor = ids.get(at.map_or(0, |i| (i + 1).min(ids.len() - 1))).cloned(),
            "up" => self.cursor = ids.get(at.map_or(0, |i| i.saturating_sub(1))).cloned(),
            "right" | "left" | "enter" => {
                let Some(c) = self.cursor.clone().or_else(|| ids.first().cloned()) else { return Ok(vec![]) };
                self.cursor = Some(c.clone());
                let r = row(&c);
                let folder = r["kind"] == "dir" || r["kind"] == "folder";
                match key {
                    "right" if folder && r["open"] != true => return self.expand(cx, &c),
                    "left" if folder && r["open"] == true => return self.expand(cx, &c),
                    "left" => {
                        // To the row of the folder it is in, when that is drawn.
                        let up = if self.tab() == "artifacts" { c.rsplit_once('/').map(|(d, _)| d.to_string()) } else { Some(view::parent(&c)) };
                        if let Some(u) = up.filter(|u| ids.contains(u)) {
                            self.cursor = Some(u);
                        }
                    }
                    "enter" if r["kind"] == "folder" => return self.expand(cx, &c),
                    "enter" => return self.select(cx, &c),
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(vec![])
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
                // Selected inside folded folders: they open, so its row shows.
                self.open.extend(ancestors(id));
                (Src::Artifact, e["name"].as_str().unwrap_or("").to_string(), e["size"].as_u64().unwrap_or(0), e["modified"].as_u64().unwrap_or(0), "file".to_string())
            }
            "recent" => {
                let f = cx.get(RECENT)["files"].as_array().into_iter().flatten().find(|f| f["path"] == id).ok_or_else(|| format!("{id} is not in Recent"))?;
                (Src::Fs, id.rsplit('/').next().unwrap_or(id).to_string(), f["size"].as_u64().unwrap_or(0), f["modified"].as_u64().unwrap_or(0), "file".to_string())
            }
            _ => {
                let e = self.entry_of(cx, id).ok_or_else(|| format!("{id} is not in this folder"))?.clone();
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
        self.section = None;
        if kind != "file" {
            self.preview = json!({ "state": "note", "text": "Not a regular file." });
        } else if let Some(p) = view::preview_without_reading(&name, size) {
            self.preview = p;
        } else {
            self.preview = json!({ "state": "loading" });
            let route = match (&s.src, view::kind(&name)) {
                (Src::Artifact, _) => format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(&s.path)),
                (Src::Fs, view::Kind::Image) => format!("{}?path={}", sb_route(&sb, "/fs/file"), enc(&s.path)),
                (Src::Fs, _) => format!("{}?path={}&max={}{}", sb_route(&sb, "/fs/file"), enc(&s.path), view::TEXT_MAX, if size > view::TEXT_MAX { "&head=1" } else { "" }),
            };
            let max = if view::kind(&name) == view::Kind::Image { view::IMAGE_MAX } else { view::TEXT_MAX };
            out.push(Action::Call(Call::get(route, format!("read {name}")).raw(max).tag(&format!("preview:{}", self.seq)).quiet()));
        }
        self.sel = Some(s);
        Ok(out)
    }

    /// A preview's bytes arrived (the raw read; for now the JSON path).
    pub fn got_preview(&mut self, seq: u64, bytes: &[u8]) -> Vec<Action> {
        let Some(s) = self.sel.clone().filter(|_| seq == self.seq) else { return vec![] };
        // More than asked (`max + 1`): the head only.
        let bytes = &bytes[..bytes.len().min(if view::kind(&s.name) == view::Kind::Image { view::IMAGE_MAX + 1 } else { view::TEXT_MAX } as usize)];
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
        // Cut short only when the read hit its cap (the listing's size may be older).
        let size = if bytes.len() as u64 >= view::TEXT_MAX { s.size.max(bytes.len() as u64 + 1) } else { bytes.len() as u64 };
        self.preview = view::text_preview(&s.name, bytes, size);
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

    /// Where waiting file `s` goes: the pane's sandbox and thread it was dropped on, else the panel's.
    fn target(&self, s: &Staged) -> Result<(String, Option<String>), String> {
        match &s.to {
            Some(t) => Ok(t.clone()),
            None => Ok((self.sandbox()?, None)),
        }
    }

    /// The core's dialog for one file; `more`: files dropped with it, asked next.
    fn upload(&self, cx: &Ctx, s: &Staged, more: Option<usize>) -> Result<Action, String> {
        if let Some(p) = s.problem() {
            return Err(format!("{} cannot go: {p}", s.name));
        }
        let (sb, thread) = self.target(s)?;
        let info = cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
        if info["status"] != "running" {
            return Err(format!("{sb} is not running: start it first, then upload"));
        }
        let tag = format!("upload-{}", s.id);
        let (call, dest) = match &s.payload {
            Payload::Path(p) => (
                core_call("upload", json!({ "route": format!("{}?name={}", sb_route(&sb, "/upload"), enc(&s.name)), "file": p, "content_type": "application/octet-stream", "tag": tag }), format!("upload {} into {sb}", s.name), &tag),
                "/tmp/sbxw-uploads/, in a new folder, under its own name (cleaned by the daemon)",
            ),
            Payload::Png(b) => (Call::post_bytes(sb_route(&sb, "/paste-image"), "image/png", b.clone(), format!("upload {} into {sb}", s.name)).tag(&tag), "/tmp/sbxw-pastes/, as a new PNG"),
        };
        let session = match &thread {
            Some(t) => format!("{sb}'s thread {t}"),
            None => format!("{sb}'s agent"),
        };
        let mut c = Confirm::new(format!("Upload {} into {sb}?", s.name), call)
            .line(format!("{} · {}", s.source(), view::size_label(s.size)))
            .line(format!("Copied into {sb} under {dest}."))
            .line(format!("Its path is then typed into {session}'s prompt, not submitted: say what it is for, then press Return."));
        if let Some(n) = more {
            c = c.line(format!("{n} more file{} dropped with it: asked next, one by one. Cancel stops here.", if n == 1 { "" } else { "s" }));
        }
        Ok(c.labels("Upload", "Cancel").into_action())
    }

    /// The dropped files' names, for a notice: one, or "N files (a, b, …)".
    fn drop_names(&self) -> String {
        match self.dropped.as_slice() {
            [one] => one.name.clone(),
            many => format!("{} files ({})", many.len(), many.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")),
        }
    }

    /// One `core:uploads` for every file of the drop, in order. Refused, for
    /// all of them, when one cannot go (empty, too large) or the sandbox is
    /// not running.
    fn drop_call(&self, cx: &Ctx) -> Result<Call, String> {
        let first = self.dropped.first().ok_or("no file dropped on a pane is waiting")?;
        let (sb, _) = self.target(first)?;
        let bad: Vec<String> = self.dropped.iter().filter_map(|s| s.problem().map(|p| format!("{}: {p}", s.name))).collect();
        if !bad.is_empty() {
            return Err(format!("Nothing was uploaded: {}", bad.join("; ")));
        }
        let info = cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw knows"))?;
        if info["status"] != "running" {
            return Err(format!("{sb} is not running: start it first, then upload"));
        }
        let mut items = vec![];
        for s in &self.dropped {
            let Payload::Path(p) = &s.payload else { return Err("only files dropped on a pane go this way".into()) };
            items.push(json!({ "route": format!("{}{}", upload_route_prefix(&sb), enc(&s.name)), "file": p, "content_type": "application/octet-stream", "tag": format!("upload-{}", s.id) }));
        }
        Ok(core_call("uploads", json!({ "sandbox": sb, "items": items }), format!("upload {} into {sb}", self.drop_names()), &self.drop_tag))
    }

    /// ONE dialog for the whole drop: every file, the total, where they go,
    /// what happens; the first time (not yet chosen) the box to stop asking;
    /// when a guardrail asks all the same, why.
    fn drop_dialog(&self, cx: &Ctx) -> Result<Action, String> {
        let call = self.drop_call(cx)?;
        let first = &self.dropped[0];
        let (sb, thread) = self.target(first)?;
        let session = match &thread {
            Some(t) => format!("{sb}'s thread {t}"),
            None => format!("{sb}'s agent"),
        };
        let n = self.dropped.len();
        let title = if n == 1 { format!("Upload {} into {sb}?", first.name) } else { format!("Upload {n} files into {sb}?") };
        let mut c = Confirm::new(title, call);
        for s in &self.dropped {
            c = c.line(format!("{} · {}", s.source(), view::size_label(s.size)));
        }
        if n > 1 {
            c = c.line(format!("{n} files, {} in all.", view::size_label(self.dropped.iter().map(|s| s.size).sum())));
        }
        let (it, its) = if n == 1 { ("It is", "Its path is") } else { ("They are", "Their paths are") };
        c = c
            .line(format!("{it} copied into {sb} under /tmp/sbxw-uploads/, where its agent can read {}.", if n == 1 { "it" } else { "them" }))
            .line(format!("{its} then typed into {session}'s prompt, not sent: say what {} for, then press Return.", if n == 1 { "it is" } else { "they are" }));
        if !self.drop_why.is_empty() {
            let lead = if self.drop_skip { "Asked even though you chose not to be asked, because" } else { "Look before you upload" };
            for w in &self.drop_why {
                c = c.line(format!("{lead}: {w}."));
            }
        }
        if !self.drop_skip {
            c = c.dont_ask("Do not ask again for files I drop");
        }
        Ok(c.labels("Upload", "Cancel").into_action())
    }

    /// A drop's batch answered: each file in order; the paths the daemon made
    /// typed together (not submitted); a failure says what was sent and what
    /// was not. Every path of the drop is forgotten afterwards.
    fn drop_uploaded(&mut self, tag: &str, r: Result<(u16, J), String>) -> Vec<Action> {
        if tag != self.drop_tag {
            return vec![];
        }
        let dropped = std::mem::take(&mut self.dropped);
        let mut out: Vec<Action> = forget(paths_of(dropped.iter())).into_iter().collect();
        let Some(first) = dropped.first() else { return out };
        let Ok((sb, thread)) = self.target(first) else { return out };
        let results = match r {
            Ok((_, body)) => body["results"].as_array().cloned().unwrap_or_default(),
            Err(e) => {
                out.push(Action::error(format!("Nothing was uploaded: {e}")));
                return out;
            }
        };
        let mut sent: Vec<&str> = vec![];
        let mut paths: Vec<String> = vec![];
        let mut failure = None;
        for s in &dropped {
            let Some(res) = results.iter().find(|x| x["tag"] == format!("upload-{}", s.id).as_str()) else { break };
            let (code, body) = (res["code"].as_u64().unwrap_or(0) as u16, &res["body"]);
            if let Some(e) = res["error"].as_str() {
                failure = Some(format!("{} was not uploaded: {e}", s.name));
                break;
            }
            if !(200..300).contains(&code) || body["ok"] == false {
                failure = Some(format!("{} was not uploaded: {}", s.name, why(code, body)));
                break;
            }
            sent.push(&s.name);
            match body["path"].as_str() {
                Some(p) if typable(p) => paths.push(p.to_string()),
                Some(p) => out.push(Action::error(format!("{} was uploaded, but its path {p:?} is not one the app types into a prompt", s.name))),
                None => out.push(Action::error(format!("{}: uploaded, but the daemon gave no path", s.name))),
            }
        }
        if !paths.is_empty() {
            let mut at = json!({ "sandbox": sb, "data": format!("{} ", paths.join(" ")) });
            if let Some(t) = &thread {
                at["mode"] = format!("thread:{t}").into();
            }
            out.push(Action::Call(Call::post("/api/input", at, format!("type {}", paths.join(" "))).tag("input:drop").quiet()));
        }
        let not_sent: Vec<&str> = dropped.iter().map(|s| s.name.as_str()).filter(|n| !sent.contains(n)).collect();
        match failure {
            Some(f) => {
                let mut m = f;
                if !sent.is_empty() {
                    m.push_str(&format!(". Sent: {}", sent.join(", ")));
                }
                let rest: Vec<&str> = not_sent.iter().skip(1).copied().collect();
                if !rest.is_empty() {
                    m.push_str(&format!(". Not sent: {}", rest.join(", ")));
                }
                out.push(Action::error(m));
            }
            None if !sent.is_empty() => out.push(Action::notice(format!("Uploaded {} to {sb}", if sent.len() == 1 { sent[0].to_string() } else { format!("{} files ({})", sent.len(), sent.join(", ")) }))),
            None => {}
        }
        out
    }

    /// `data.files_ui`, once the panel was opened (before, nothing is set:
    /// a fixture's own view stays).
    fn ui_action(&self, cx: &Ctx) -> Option<Action> {
        self.sandbox.as_ref().map(|_| Action::Set { key: UI.into(), value: self.ui(cx) })
    }

    fn map_ui_action(&self, cx: &Ctx) -> Option<Action> {
        self.map.sandbox.as_ref().map(|_| Action::Set { key: MAP_UI.into(), value: self.map_ui(cx) })
    }

    /// `data.files_ui`, what the Project panel draws.
    pub fn ui(&self, cx: &Ctx) -> J {
        let sb = self.sandbox.clone();
        let mine = |k: &str| self.asked_for.get(k) == sb.as_ref() && sb.is_some();
        let d = cx.get(DIR);
        let errors = &cx.get("app")["errors"];
        let ups = &cx.get("app")["uploads"];
        let staged: Vec<J> = self.staged.iter().enumerate().map(|(i, s)| {
            let u = &ups[format!("upload-{}", s.id)];
            let to = self.target(s).ok().map(|(sb, t)| match t { Some(t) => format!("{sb} · {t}"), None => sb });
            json!({ "index": i + 1, "name": s.name, "size": view::size_label(s.size), "from": s.source(), "problem": s.problem(), "to": to,
                "state": u["state"], "progress": match (u["sent"].as_f64(), u["total"].as_f64()) { (Some(a), Some(t)) if t > 0.0 => a / t, _ => 0.0 } })
        }).collect();
        let sel = self.sel.as_ref().map(|s| json!({
            "id": s.path, "name": s.name, "kind": s.kind, "size": view::size_label(s.size), "type": view::kind_label(&s.name),
            "modified": view::date(s.modified), "path": self.abs_path(cx, s),
            "can_delete": d.is_object() && view::can_delete(&self.abs_path(cx, s), d),
        }));
        let recent_fresh = cx.get(RECENT)["files"].as_array().map(|v| v.iter().filter(|f| view::now_secs().saturating_sub(f["modified"].as_u64().unwrap_or(0)) < view::FRESH_SECS).count()).unwrap_or(0);
        let a = cx.get(ARTIFACTS);
        let run = run_for(cx, sb.as_deref().unwrap_or(""));
        let (rows, count) = self.page(cx);
        let locations = if mine(DIR) { view::locations(d, &self.starred(cx), sb.as_deref().unwrap_or("the sandbox")) } else { vec![] };
        let location = match self.tab() {
            "files" => d["path"].as_str().and_then(|p| view::location_of(&locations, p)),
            t => Some(t.to_string()),
        };
        json!({
            "sandbox": sb, "tab": self.tab(), "filter": self.filter, "hidden": self.hidden,
            "path": d["path"], "can_up": d["path"].as_str().is_some_and(|p| p != "/"), "can_back": !self.history.is_empty(),
            "offline": d["offline"] == true,
            "starred": d["path"].as_str().is_some_and(|p| self.starred(cx).iter().any(|x| x == p)),
            "places": if mine(DIR) { json!(view::places(d, &self.starred(cx))) } else { json!([]) },
            "locations": locations, "location": location,
            "crumbs": match self.tab() {
                "files" => json!(d["path"].as_str().map(|p| view::crumbs(p, d["home"].as_str().unwrap_or("/home/agent"))).unwrap_or_default()),
                "recent" => json!([{ "label": "Recent", "path": J::Null }]),
                _ => json!([{ "label": "Artifacts", "path": J::Null }]),
            },
            "home": d["home"].as_str().unwrap_or("/home/agent"), "sort": if self.sort.is_empty() { "name" } else { self.sort.as_str() },
            "rows": rows, "count": count,
            "loading": { "artifacts": !mine(ARTIFACTS) || a.is_null(), "files": d.is_null(), "recent": cx.get(RECENT).is_null() },
            "errors": { "artifacts": errors[ARTIFACTS], "files": errors[DIR], "recent": errors[RECENT] },
            "artifacts_dir": a["dir"], "has_map": a["codemap"].is_object() || a["lenses"].as_array().is_some_and(|l| !l.is_empty()),
            "lenses": a["lenses"].as_array().map(|l| l.len()).unwrap_or(0),
            "run": run,
            "recent_fresh": recent_fresh, "recent_days": cx.get(RECENT)["days"],
            "selected": sel, "preview": self.reader(cx),
            "sandboxes": cx.get("sandboxes").as_array().into_iter().flatten().filter_map(|x| x["name"].as_str()).map(|n| json!({ "id": n, "label": n })).collect::<Vec<_>>(),
            "color": cx.get("app")["groups"][sb.as_deref().unwrap_or("")],
            "worktrees": if mine(DIR) { d["worktrees"].as_array().into_iter().flatten().filter_map(|w| Some(json!({ "thread": w["thread"].as_str()?, "path": w["path"].as_str()?, "here": d["path"].as_str().is_some_and(|p| w["path"].as_str().is_some_and(|wp| p.starts_with(wp))) }))).collect::<Vec<_>>() } else { vec![] },
            "staged": staged, "upload_max": view::size_label(view::UPLOAD_MAX),
            "saved": self.saved.as_ref().map(|(n, p)| json!({ "name": n, "path": p })),
        })
    }

    /// The preview as the explorer's reader draws it: a Markdown file with
    /// its links made the explorer's own, its outline and the section shown;
    /// a text file with line numbers.
    fn reader(&self, cx: &Ctx) -> J {
        let mut p = self.preview.clone();
        let text = p["text"].as_str().unwrap_or("").to_string();
        match p["state"].as_str() {
            Some("markdown") => {
                let dir = self.sel.as_ref().map(|s| view::parent(&self.abs_path(cx, s))).unwrap_or_else(|| "/".into());
                let outline = explorer::outline(&text);
                let shown = self.section.and_then(|i| explorer::section(&text, i)).unwrap_or(text);
                p["text"] = explorer::rewrite(&shown, &dir).into();
                p["outline"] = J::Array(outline.into_iter().map(|mut h| {
                    h["on"] = (self.section == h["index"].as_u64().map(|i| i as usize)).into();
                    h
                }).collect());
                p["section"] = self.section.map_or(J::Null, |i| json!(i));
            }
            Some("text") => p["numbered"] = explorer::numbered(&text).into(),
            _ => {}
        }
        p
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
            .map(|p| Action::Call(Call::get(format!("{}?path={}", sb_route(&sb, "/artifacts/download"), enc(p)), format!("read {p}")).raw(view::TEXT_MAX).tag(&format!("doc:{gen}:{p}")).quiet()))
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

    pub fn on_bytes(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, Vec<u8>, String), String>) -> Vec<Action> {
        let mut out = vec![];
        let tag = call.tag.as_str();
        let body_err = |c: u16, b: &[u8]| {
            let v: J = serde_json::from_slice(b).unwrap_or_else(|_| json!({ "error": String::from_utf8_lossy(b).trim().to_string() }));
            why(c, &v)
        };
        if let Some(seq) = tag.strip_prefix("preview:").and_then(|s| s.parse::<u64>().ok()) {
            match r {
                Ok((c, b, _)) if (200..300).contains(&c) => out.extend(self.got_preview(seq, &b)),
                Ok((c, b, _)) if seq == self.seq => self.preview = json!({ "state": "error", "text": body_err(c, &b) }),
                Err(e) if seq == self.seq => self.preview = json!({ "state": "error", "text": e }),
                _ => {}
            }
        } else if let Some(rest) = tag.strip_prefix("doc:") {
            let (gen, path) = rest.split_once(':').unwrap_or(("0", rest));
            let gen = gen.parse().unwrap_or(0);
            match r {
                Ok((c, b, _)) if (200..300).contains(&c) => self.got_doc(gen, path, &b[..b.len().min(view::TEXT_MAX as usize)]),
                Ok((c, b, _)) if gen == self.map.gen => {
                    self.map.error = Some(format!("{path}: {}", body_err(c, &b)));
                    self.doc_done();
                }
                Err(e) if gen == self.map.gen => {
                    self.map.error = Some(format!("{path}: {e}"));
                    self.doc_done();
                }
                _ => {}
            }
        }
        out.extend(self.ui_action(cx));
        out.extend(self.map_ui_action(cx));
        out
    }

    /// An upload answered: its path typed into the prompt it was meant for.
    fn uploaded(&mut self, id: u64, r: Result<(u16, J), String>) -> Vec<Action> {
        let (s, from_drop) = if let Some(i) = self.staged.iter().position(|s| s.id == id) {
            (self.staged[i].clone(), false)
        } else if let Some(i) = self.dropped.iter().position(|s| s.id == id) {
            (self.dropped[i].clone(), true)
        } else {
            return vec![];
        };
        let name = s.name.clone();
        let mut out = match r {
            Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
                self.staged.retain(|x| x.id != id);
                self.dropped.retain(|x| x.id != id);
                match (body["path"].as_str(), self.target(&s)) {
                    (Some(path), Ok((sb, thread))) if typable(path) => {
                        let mut at = json!({ "sandbox": sb, "data": format!("{path} ") });
                        if let Some(t) = thread {
                            at["mode"] = format!("thread:{t}").into();
                        }
                        vec![Action::Call(Call::post("/api/input", at, format!("type {path}")).tag(&format!("input:{path}")).quiet())]
                    }
                    // Not a path the daemon makes: never typed into a prompt.
                    (Some(path), _) => vec![Action::error(format!("{name} was uploaded, but its path {path:?} is not one the app types into a prompt"))],
                    (None, _) => vec![Action::error(format!("{name}: uploaded, but the daemon gave no path"))],
                }
            }
            Ok((c, body)) => {
                let mut v = vec![Action::error(format!("{name} was not uploaded: {}", why(c, &body)))];
                if from_drop {
                    v.extend(forget(paths_of(std::mem::take(&mut self.dropped).iter())));
                }
                v
            }
            Err(e) => {
                let mut v = vec![Action::error(format!("{name} was not uploaded: {e}"))];
                if from_drop {
                    v.extend(forget(paths_of(std::mem::take(&mut self.dropped).iter())));
                }
                v
            }
        };
        // The next file dropped with it: its own dialog.
        if from_drop && !self.dropped.is_empty() {
            out.push(Action::Intent { name: "files_upload_dropped".into(), args: json!({}) });
        }
        out
    }

    fn stage(&mut self, name: String, size: u64, payload: Payload, to: Option<(String, Option<String>)>) {
        self.staged_seq += 1;
        self.staged.push(Staged { id: self.staged_seq, name, size, payload, to });
    }

    /// Files dropped on a terminal pane: waiting for that sandbox's agent,
    /// and the core's dialog for the first at once.
    pub fn on_pane_drop(&mut self, cx: &Ctx, d: &PaneDrop) -> Vec<Action> {
        // A new drop replaces what an earlier one left (normally nothing: a
        // Cancel empties it), and those paths are forgotten.
        let mut out: Vec<Action> = forget(paths_of(std::mem::take(&mut self.dropped).iter())).into_iter().collect();
        let mut notes = vec![];
        for p in &d.paths {
            if self.dropped.len() >= view::STAGED_MAX {
                notes.push(format!("{} files at a time at most: the rest were left out", view::STAGED_MAX));
                break;
            }
            match std::fs::metadata(p) {
                Ok(m) if m.is_file() => {
                    self.staged_seq += 1;
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "upload".into());
                    self.dropped.push(Staged { id: self.staged_seq, name, size: m.len(), payload: Payload::Path(p.clone()), to: Some((d.sandbox.clone(), d.thread.clone())) });
                }
                Ok(_) => notes.push(format!("{}: a folder; drop the files in it instead", p.display())),
                Err(e) => notes.push(format!("{}: {e}", p.display())),
            }
        }
        self.drop_seq += 1;
        self.drop_tag = format!("drop-{}", self.drop_seq);
        self.drop_skip = d.skip_dialog;
        self.drop_why = drops::guardrails(&paths_of(self.dropped.iter()), d.paths.len());
        let plain = notes.is_empty() && self.drop_why.is_empty();
        out.extend(notes.into_iter().map(Action::error));
        if self.dropped.is_empty() {
            return out;
        }
        // Chosen not to be asked, and nothing asks all the same: the batch
        // goes now (the core checks it is this drop's, and the guardrails,
        // again), and a notice says it at once.
        if d.skip_dialog && plain {
            match self.drop_call(cx) {
                Ok(call) => {
                    out.push(Action::notice(format!("Uploading {} to {}…", self.drop_names(), d.sandbox)));
                    out.push(Action::Call(call));
                }
                Err(e) => {
                    out.push(Action::error(e));
                    out.extend(forget(paths_of(std::mem::take(&mut self.dropped).iter())));
                }
            }
            return out;
        }
        out.push(Action::Intent { name: "files_upload_dropped".into(), args: json!({}) });
        out
    }

    /// One of this feature's dialogs closed without its call going out
    /// (Cancel, Escape, replaced): a drop's upload ends there, and every
    /// file left of that drop stops being uploadable.
    pub fn on_dismiss(&mut self, cx: &Ctx, tag: &str) -> Vec<Action> {
        if tag.starts_with("drop-") {
            if tag != self.drop_tag {
                return vec![];
            }
            let mut out: Vec<Action> = forget(paths_of(std::mem::take(&mut self.dropped).iter())).into_iter().collect();
            out.extend(self.ui_action(cx));
            return out;
        }
        let Some(id) = tag.strip_prefix("upload-").and_then(|s| s.parse::<u64>().ok()) else { return vec![] };
        let mut out = vec![];
        if self.dropped.iter().any(|s| s.id == id) {
            out.extend(forget(paths_of(std::mem::take(&mut self.dropped).iter())));
        }
        // A file waiting on the panel stays there (its row has Upload and ×).
        out.extend(self.ui_action(cx));
        out
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let mut out = vec![];
        let tag = call.tag.as_str();
        if let Some((sb, path)) = tag.strip_prefix("expand:").and_then(|t| t.split_once(':')) {
            // A folder opened in place: its listing, or why not (said on its row).
            if self.sandbox.as_deref() == Some(sb) && self.expanded.contains(path) {
                let v = match r {
                    Ok((200, b)) if b["entries"].is_array() => b,
                    Ok((c, b)) => json!({ "error": why(c, &b) }),
                    Err(e) => json!({ "error": e }),
                };
                self.children.insert(path.to_string(), v);
            }
        } else if tag.starts_with("drop-") {
            out.extend(self.drop_uploaded(tag, r));
        } else if let Some(id) = tag.strip_prefix("upload-").and_then(|s| s.parse::<u64>().ok()) {
            out.extend(self.uploaded(id, r));
        } else if tag == "delete" {
            match r {
                Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
                    out.push(Action::notice(format!("Deleted {}", body["path"].as_str().or(call.body["path"].as_str()).unwrap_or("it"))));
                    self.sel = None;
                    out.extend(self.drop_preview());
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
        } else if let Some(path) = tag.strip_prefix("input:") {
            // The uploaded file's path typed into the prompt, or why not.
            let sb = call.body["sandbox"].as_str().unwrap_or("");
            match r {
                Ok((c, _)) if (200..300).contains(&c) => out.push(Action::notice(format!("📎 {path} typed into {sb}'s prompt"))),
                _ => out.push(Action::Notice {
                    text: format!("Uploaded to {path}; no agent session of {sb} took the path: open it, then paste the path (click to copy)"),
                    error: true,
                    then: Some(("copy_text".into(), json!({ "text": path }))),
                }),
            }
        } else if tag == "download" || tag == "open" {
            match r {
                Ok((c, body)) if (200..300).contains(&c) => {
                    if let Some(p) = body["saved"].as_str() {
                        let name = body["name"].as_str().unwrap_or("the file").to_string();
                        self.saved = Some((name.clone(), p.to_string()));
                        out.push(Action::notice(format!("Saved {name}: {p}")));
                    }
                    if let Some(e) = body["after_error"].as_str() {
                        out.push(Action::error(format!("{}: {e}", call.what)));
                    }
                }
                Ok((499, _)) => {}
                Ok((c, body)) => out.push(Action::error(format!("{}: {}", call.what, why(c, &body)))),
                Err(e) => out.push(Action::error(format!("{}: {e}", call.what))),
            }
        } else {
            match r {
                Ok((c, _)) if (200..300).contains(&c) => {}
                Ok((c, body)) => out.push(Action::error(format!("{}: {}", call.what, why(c, &body)))),
                Err(e) => out.push(Action::error(format!("{}: {e}", call.what))),
            }
        }
        out.extend(self.ui_action(cx));
        out.extend(self.map_ui_action(cx));
        out
    }

    pub fn on_data(&mut self, cx: &Ctx, key: &str, _old: &J, new: &J) -> Vec<Action> {
        let mut out = vec![];
        if key == DIR && self.sandbox.is_some() {
            // The explorer was opened at a thread's worktree before its places were read.
            if let Some(t) = self.want_worktree.take() {
                if let Some(p) = worktree_path(new, &t) {
                    if let Ok(a) = self.browse(&p) {
                        out.extend(a);
                        out.extend(self.ui_action(cx));
                        return out;
                    }
                }
            }
            // The first listing (the workspace) goes on to its deliverables
            // folder, when it has one, as the web UI's panel opens.
            if std::mem::take(&mut self.first) && self.path.is_empty() {
                if let Some(a) = new["artifacts"].as_str().filter(|_| new["entries"].as_array().is_some_and(|e| e.iter().any(|x| x["name"] == ".sbxw-artifacts" && x["kind"] == "dir"))) {
                    self.path = a.to_string();
                    let sb = self.sandbox.clone().unwrap();
                    out.push(self.fetch_dir(&sb));
                    out.extend(self.ui_action(cx));
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
        if key == ARTIFACTS && (self.map.loading || self.map.reload) && self.map.sandbox.is_some() && self.asked_for.get(ARTIFACTS) == self.map.sandbox.as_ref() {
            self.map.reload = false;
            out.extend(self.load_map(cx));
        }
        out.extend(self.ui_action(cx));
        out.extend(self.map_ui_action(cx));
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
                        if self.asked_for.get(ARTIFACTS) == Some(&sb) && cx.get(ARTIFACTS).is_object() {
                            out.extend(self.load_map(cx));
                        }
                        self.map.reload = true;
                    }
                    out.push(self.fetch(ARTIFACTS, &sb, "/artifacts"));
                }
            }
            Some("failed") => out.push(Action::error(format!("{sb}: {} ended without one{}", if lens { "the lens" } else { "the code map" }, run["note"].as_str().map(|n| format!(": {n}")).unwrap_or_default()))),
            _ => {}
        }
        out.extend(self.ui_action(cx));
        out.extend(self.map_ui_action(cx));
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
                            self.stage(name, m.len(), Payload::Path(p), None);
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
                    let n = bytes.len() as u64;
                    self.stage(format!("pasted-{}.png", self.paste_seq), n, Payload::Png(bytes), None);
                }
            }
        }
        let mut out: Vec<Action> = notes.into_iter().map(Action::error).collect();
        out.push(Action::OpenPanel(SCREEN.into()));
        out.extend(self.ui_action(cx));
        out
    }
}

/// A path the daemon's upload routes return (`/tmp/sbxw-uploads/<ms>/<name>`,
/// `/tmp/sbxw-pastes/paste-<ms>.png`, names of `[A-Za-z0-9._-]`): only such
/// a path is typed into a prompt, so nothing in it can read as more words,
/// a quote, a new line or an option.
pub fn typable(path: &str) -> bool {
    (path.starts_with("/tmp/sbxw-uploads/") || path.starts_with("/tmp/sbxw-pastes/"))
        && path.len() < 400
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        && !path.split('/').any(|c| c == ".." || c.starts_with('-'))
}

/// A thread's worktree folder, as the daemon's listing names it.
fn worktree_path(listing: &J, thread: &str) -> Option<String> {
    listing["worktrees"].as_array()?.iter().find(|w| w["thread"] == thread)?["path"].as_str().filter(|p| p.starts_with('/')).map(str::to_string)
}

/// Rows drawn at first, and added by each "Show more".
pub const PAGE: usize = 200;
/// `files_sort`'s orders.
pub const SORTS: &[&str] = &["name", "date"];
/// `files_key`'s keys.
pub const KEYS: &[&str] = &["up", "down", "left", "right", "enter", "type", "backspace"];

/// Every folder of `.sbxw-artifacts` that holds a listed file, with the
/// number of files under it.
fn artifact_dirs(cx: &Ctx) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for e in cx.get(ARTIFACTS)["entries"].as_array().into_iter().flatten() {
        for d in ancestors(e["path"].as_str().unwrap_or("")) {
            *out.entry(d).or_insert(0) += 1;
        }
    }
    out
}

/// `a/b/c.md` → `a`, `a/b`.
fn ancestors(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    (1..parts.len()).map(|n| parts[..n].join("/")).collect()
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
    // The rail's Files (browse zone, last): the sandbox used last, else the focused pane's.
    let mut v = vec![Extra {
        place: Place::Rail { zone: crate::feat::Zone::Browse, order: 30 },
        id: SCREEN.into(),
        label: "Files".into(),
        icon: Some("folder".into()),
        tone: "blue".into(),
        help: "Explore a sandbox's files and deliverables, read Markdown, upload".into(),
        count: None,
        intent: Some(("files_show".into(), json!({}))),
    }];
    for sb in cx.get("sandboxes").as_array().into_iter().flatten() {
        let Some(name) = sb["name"].as_str() else { continue };
        // A pane's Project opens the explorer on its sandbox, and a thread's
        // pane at that thread's worktree.
        for se in crate::feat::threads::model::sessions(cx.data, name) {
            let own = se.thread.is_some() && se.kind == "own";
            let mut args = json!({ "sandbox": name });
            if own {
                args["worktree"] = se.thread.clone().into();
            }
            v.push(Extra {
                place: Place::Session { sandbox: name.into(), thread: se.thread.clone() },
                id: "project".into(),
                label: "Project".into(),
                icon: Some("folder".into()),
                tone: "muted".into(),
                help: if own { "The file explorer at this thread's worktree: files, deliverables, uploads, code map".into() } else { "The file explorer on this sandbox: files, deliverables, uploads, code map".into() },
                count: None,
                intent: Some(("files_open".into(), args)),
            });
        }
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
