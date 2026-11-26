//! Kanban: the Dashboard's second view (the feature `board`, shown to the
//! person as "Dashboard"), the tickets of a sandbox (or of every sandbox) in
//! five columns: Planned, In progress, In test, Ready to merge, Merged
//! (docs/tickets.md). An overlay titled "Dashboard", like the Dashboard's
//! conversation, with a "Conversation | Kanban" switch at the same place.
//!
//! * **Doors.** The top bar's Kanban button, ⌃⌥K / Alt+K and the palette's
//!   "Show the Kanban" run `kanban_toggle`: open on the sandbox in context
//!   ([`context_sandbox`]), or close when the Kanban is the screen drawn.
//!   The Dashboard's own switch runs `kanban_show` (its last scope).
//!
//! * **Data.** The daemon decides every card's column and writes every
//!   label (`GET /api/tickets?sandbox=`); this feature only lays the
//!   answer out for the screen (`model.rs`), once per answer or press. A
//!   ticket opens as a panel over the board (`GET /api/tickets/:sb/:n`); its
//!   pictures preview in place (their bytes fetched here, decoded by the core).
//! * **Paths.** Only the core hands this feature a path: the file dialog, a
//!   drop on the form, a pasted picture (`on_files`). No intent takes a
//!   path. Each file is checked by `drops::guardrails`; one that looks like
//!   a secret makes Save ask once, with the ack box.
//! * **Acts.** New, edit, attach, reorder inside Planned, priority,
//!   "validated on my machine": the person's own data on this computer,
//!   Direct. Delete and removing a file: ONE core Confirm naming what goes.
//!   "Send to the lead": risk Approval, ONE Confirm naming the target and
//!   the fixed line in full; the daemon types that line, written from the
//!   ticket's number alone, under a team note's delivery rule. Refused here
//!   in Rust under a dialog or a menu, and while a Jarvis proposal waits.
//! * **Text.** Ticket and agent words are data: text and pills, never a control.
//! * **Ticket source.** A daemon with ticket sources adds a Backlog column,
//!   first: the issues of a tracker (GitHub first), imported by the daemon.
//!   An imported card shows its issue's key and opens it in the browser
//!   (http and https only); it is read-only here. "Move to Planned" (only on
//!   a card with `can_plan`) is the one way out of Backlog, the person's.
//!   The header shows the sandbox's source (`GET /api/tickets/:sb/source`):
//!   its status and Connect, or Refresh. Its dialog's Save and Disconnect
//!   change what sbxw writes to the tracker: each is an Approval, ONE
//!   Confirm saying what is written outward. An older daemon (no Backlog,
//!   404 on the source) leaves the Kanban as it was.

pub mod model;
#[cfg(test)]
mod tests;

use super::{Action, Asked, Call, Confirm, Ctx, Feature, Incoming, ScreenDecl, Slot, Standin};
use crate::client::enc;
use crate::feat::{After, Dest, Download};
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub const SCREEN: &str = "kanban";
pub const FORM: &str = "kanban-form";
pub const UI: &str = "kanban_ui";
pub const FORM_UI: &str = "kanban_form";
pub const SOURCE: &str = "kanban-source";
pub const SOURCE_UI: &str = "kanban_source_form";
/// The trackers a source can be (GitHub only for now).
const TRACKERS: &[&str] = &["github"];
/// A filter label's default.
pub const DEFAULT_LABEL: &str = "sbxw";
/// What sbxw writes to the tracker, said in the dialog and in both Confirms.
pub fn writes_line(tracker: &str) -> String {
    format!("sbxw writes one comment and one state label per issue to {tracker}, under your own account.")
}

const KEYS: &[&str] = &["up", "down", "left", "right", "enter", "escape", "type"];
const MOVES: &[&str] = &["up", "down", "top", "bottom"];
const PRIOS: &[&str] = &["none", "p1", "p2", "p3"];
const LINKS: &[&str] = &["terminal", "monitor", "commits", "worktrees", "plan"];
/// Files one ticket holds, and one file's bytes (the daemon's limits, checked first here).
pub const MAX_FILES: usize = 20;
pub const MAX_FILE_BYTES: u64 = 20 << 20;
pub const MAX_TITLE: usize = 160;
pub const MAX_DESCRIPTION: usize = 8000;

pub const FEATURE: Feature = Feature {
    name: "kanban",
    screens: &[
        ScreenDecl { name: SCREEN, src: include_str!("../../../screens/kanban.luau"), slot: Slot::Overlay { title: TITLE, icon: "board" } },
        ScreenDecl { name: FORM, src: include_str!("../../../screens/kanban-form.luau"), slot: Slot::Modal { width: 720.0, light_dismiss: false } },
        ScreenDecl { name: SOURCE, src: include_str!("../../../screens/kanban-source.luau"), slot: Slot::Modal { width: 560.0, light_dismiss: false } },
    ],
    events: &["tickets", "plan"],
    intents: INTENTS,
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    on_bytes: Some(on_bytes),
    on_files: Some(on_files),
    on_dismiss: Some(on_dismiss),
    standin: Some(standin),
    shortcuts: &[crate::feat::Shortcut { key: 'k', intent: "kanban_toggle", args: "{}", help: SHOW }],
    ..Feature::EMPTY
};

/// The overlay's title, the same as the conversation's (`board::TITLE`).
pub const TITLE: &str = crate::feat::board::TITLE;
/// The palette's row and Help's line for the shortcut.
pub const SHOW: &str = "Show the Kanban";

use crate::intent::direct as d;

pub const INTENTS: &[Spec] = &[
    d("kanban_show", &[("sandbox", Arg::Text, false)], "Opens the Kanban over the panes: the tickets and the threads of a sandbox (empty: every sandbox) in five columns, Planned, In progress, In test, Ready to merge, Merged, each placed by sbxw from its thread's state."),
    d("kanban_toggle", &[], "Shows the Kanban, as the top bar's Kanban button does: on the sandbox in context (the focused pane's in the terminals, else the selected sandbox), else the last sandbox it showed, else every sandbox. Closes it when it is the screen shown."),
    d("kanban_scope", &[("sandbox", Arg::Text, true)], "Shows another sandbox's Kanban (empty: every sandbox)."),
    d("kanban_open", &[("id", Arg::Text, true)], "Opens a card's ticket as a panel over the board (its element key; empty: closes the panel)."),
    d("kanban_column", &[("column", Arg::OneOf(model::COLUMNS), true)], "Under 900 pt, the column shown."),
    d("kanban_more", &[("column", Arg::OneOf(model::COLUMNS), true)], "Draws more cards of a column."),
    d("kanban_older", &[], "Shows the older merged cards too."),
    d("kanban_key", &[("key", Arg::OneOf(KEYS), true), ("text", Arg::Text, false)], "The Kanban's keys while no control has the keyboard: arrows between cards and columns, Enter opens, N a new ticket, Escape closes the panel."),
    d("kanban_new", &[], "Opens the form for a new ticket (title, description, files) in the sandbox shown."),
    d("kanban_edit", &[], "Opens the form on the ticket open in the panel."),
    d("kanban_form", &[("title", Arg::Text, false), ("description", Arg::Text, false), ("priority", Arg::OneOf(PRIOS), false), ("sandbox", Arg::Text, false)], "The form's fields as the person types them."),
    d("kanban_attach", &[], "Opens the file dialog to attach files to the ticket in the form."),
    d("kanban_paste", &[], "Attaches the picture on the clipboard to the ticket in the form."),
    d("kanban_unattach", &[("index", Arg::Int(0, 40), true)], "Takes back a file attached in the form, not saved yet."),
    d("kanban_save", &[], "Saves the form: the ticket, then each file attached (asked first when one looks like a secret)."),
    d("kanban_cancel", &[], "Closes the form without saving."),
    Spec { name: "kanban_delete", risk: Risk::Approval("Deletes a ticket and its files from this computer"), args: &[], doc: "Deletes the ticket open in the panel and its files, after one confirmation naming them." },
    Spec { name: "kanban_remove_file", risk: Risk::Approval("Removes a file from a ticket"), args: &[("file", Arg::Str, true)], doc: "Removes one file of the open ticket, after one confirmation." },
    d("kanban_move", &[("to", Arg::OneOf(MOVES), true)], "Moves the open ticket up or down inside Planned (only a ticket no thread works on moves)."),
    d("kanban_priority", &[("priority", Arg::OneOf(PRIOS), true)], "Sets the open ticket's priority."),
    d("kanban_validate", &[("on", Arg::Bool, true)], "Marks a merged ticket validated on your machine, or not."),
    Spec { name: "kanban_send", risk: Risk::Approval("Types a line into the lead's session"), args: &[], doc: "Tells the sandbox's lead that the open ticket waits for a plan: one fixed line with its number, after one confirmation showing it. The ticket's words are never typed." },
    d("kanban_file", &[("file", Arg::Str, true), ("open", Arg::Bool, false)], "Saves a file of the open ticket to your Downloads folder and shows it there (`open`: opens it with the system, after the core's check)."),
    d("kanban_link", &[("what", Arg::OneOf(LINKS), true)], "Goes where the open card's thread is: its terminal, its session in Sessions, its commits or worktrees, the plan dialog."),
    d("kanban_refresh", &[], "Reads the Kanban again."),
    d("kanban_issue", &[("id", Arg::Text, false)], "Opens an imported card's issue in the browser (its element key; empty: the ticket open in the panel). Only an http or https link opens."),
    d("kanban_plan", &[("id", Arg::Text, false)], "Moves a Backlog ticket to Planned (its element key; empty: the ticket open in the panel). The one way out of Backlog."),
    d("kanban_source_open", &[], "Opens the ticket source dialog for the sandbox shown: the tracker, the repository, the filter label, whether merged tickets close their issue."),
    d("kanban_source_field", &[("tracker", Arg::OneOf(TRACKERS), false), ("repo", Arg::Text, false), ("label", Arg::Text, false), ("auto_close", Arg::Bool, false)], "The source dialog's fields as the person sets them."),
    d("kanban_source_cancel", &[], "Closes the source dialog without saving."),
    d("kanban_source_refresh", &[], "Syncs the sandbox shown with its ticket source now."),
    Spec { name: "kanban_source_save", risk: Risk::Approval("Changes what sbxw writes to the tracker"), args: &[], doc: "Saves the source dialog: sbxw imports the repository's issues with the label and writes each ticket's progress back to its issue, after one confirmation saying what is written." },
    Spec { name: "kanban_source_remove", risk: Risk::Approval("Stops sbxw writing to the tracker"), args: &[], doc: "Disconnects the sandbox's ticket source, after one confirmation: imported Backlog tickets are removed; nothing is written to the tracker any more." },
];

/// The source dialog, open.
#[derive(Debug, Default, Clone)]
pub struct SourceForm {
    pub sandbox: String,
    pub tracker: String,
    pub repo: String,
    pub label: String,
    pub auto_close: bool,
    /// The repository was filled from the sandbox's own (`detected`).
    pub detected: bool,
    /// A source is set now (Disconnect is offered).
    pub configured: bool,
    pub saving: bool,
    pub error: Option<String>,
}

/// `owner/name`, each part made of letters, digits, `-`, `_`, `.`.
pub fn repo_ok(r: &str) -> bool {
    let ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    matches!(r.split_once('/'), Some((o, n)) if ok(o) && ok(n))
}

/// A file the person handed the form, not saved yet.
#[derive(Debug, Clone)]
pub struct Handed {
    /// The path the core handed (a drop, the dialog); `None` for a pasted picture.
    pub path: Option<PathBuf>,
    pub name: String,
    pub size: u64,
    /// Bytes of a pasted picture (a file is read when it is sent).
    pub bytes: Option<Vec<u8>>,
    /// What the guardrails found (empty: nothing).
    pub look: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Form {
    /// The ticket edited, or a new one.
    pub edit: Option<(String, u32)>,
    pub sandbox: String,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub files: Vec<Handed>,
    pub saving: bool,
    pub error: Option<String>,
    /// Uploads of the save under way: the ticket, the next file.
    pub job: Option<(String, u32, usize)>,
}

#[derive(Debug, Default)]
pub struct State {
    gen: u64,
    /// The sandbox shown (`None`: every sandbox).
    scope: Option<String>,
    board: J,
    loading: bool,
    error: Option<String>,
    older: bool,
    pages: BTreeMap<String, usize>,
    /// Under 900 pt: the column shown.
    col: String,
    /// The card the keys are on (its element key).
    focus: Option<String>,
    /// The card open as a panel (its element key), and its ticket's view.
    open: Option<String>,
    view: Option<J>,
    previews: BTreeSet<String>,
    pub form: Option<Form>,
    pick_armed: bool,
    /// The sandbox's ticket source as the daemon answered (`None`: not read,
    /// every sandbox shown, or an older daemon with no sources).
    source: Option<J>,
    pub source_form: Option<SourceForm>,
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
    if tag == "kb-src-save" || tag == "kb-src-rm" {
        if let Some(f) = s.source_form.as_mut() {
            f.saving = false;
        }
        return s.views(cx);
    }
    if tag == "kb-save" {
        if let Some(f) = s.form.as_mut() {
            f.saving = false;
        }
        return s.views(cx);
    }
    vec![]
}

/// The Kanban's previews still registered for `image` elements: their
/// pixels' bytes and how many (the memory report, shell/memory.rs). They are
/// counted in `picture::mem` too.
pub fn previews_mem() -> (u64, usize) {
    let s = STATE().lock().unwrap_or_else(|e| e.into_inner());
    let kept: Vec<(u32, u32)> = s.previews.iter().filter_map(|k| crate::picture::shown(k).map(|p| p.1)).collect();
    (kept.iter().map(|(w, h)| *w as u64 * *h as u64 * 4).sum(), kept.len())
}

pub fn shown(cx: &Ctx) -> bool {
    cx.get("app")["overlay"]["screen"] == SCREEN
}

/// The sandbox the top bar's Kanban button opens on: in the terminals, the
/// focused pane's sandbox; else (Sessions, an empty pane) the selected one.
/// `None`: no sandbox in context (the Kanban's last scope, else every sandbox).
pub fn context_sandbox(cx: &Ctx) -> Option<String> {
    let app = cx.get("app");
    let gui = app["base"].is_string();
    let pane = app["panes"].as_array().into_iter().flatten().find(|p| p["focused"] == true).and_then(|p| p["sandbox"].as_str());
    let sel = app["selected"].as_str();
    let known = |s: &&str| !s.is_empty() && cx.sandbox(s).is_some();
    (if gui { None } else { pane.filter(known) }).or(sel.filter(known)).map(str::to_string)
}

fn form_shown(cx: &Ctx) -> bool {
    cx.get("app")["modal"]["screen"] == FORM
}

/// Why "Send to the lead" cannot go now: something over the view, or a
/// Jarvis proposal waiting.
pub fn held(cx: &Ctx) -> Option<String> {
    if cx.data["app"]["covered_top"] != false && !cx.data["app"]["covered_top"].is_null() {
        return Some("Something is open over the Kanban (a dialog or a menu): close it first".into());
    }
    if cx.data["jarvis"]["pending"].as_array().is_some_and(|p| !p.is_empty()) {
        return Some("A Jarvis proposal waits for you: approve or refuse it first".into());
    }
    None
}

/// The line the daemon types (`sbxw_core::tickets::send_line`), shown in the Confirm.
pub fn send_line(n: u32) -> String {
    format!("[sbxw] ticket T-{n} waits for a plan: read it with ticket_read")
}

impl State {
    fn read_board(&mut self) -> Action {
        self.loading = true;
        let mut p = format!("/api/tickets?sandbox={}", enc(self.scope.as_deref().unwrap_or("all")));
        if self.older {
            p.push_str("&older=1");
        }
        Action::Call(Call::get(p, "read the Kanban").tag(&format!("kb-board:{}", self.gen)).quiet())
    }

    /// The sandbox's ticket source (one sandbox shown only).
    fn read_source(&self) -> Option<Action> {
        let sb = self.scope.as_deref()?;
        Some(Action::Call(Call::get(format!("/api/tickets/{}/source", enc(sb)), "read the ticket source").tag(&format!("kb-source:{}:{sb}", self.gen)).quiet()))
    }

    /// The board, and the source with it.
    fn read_all(&mut self) -> Vec<Action> {
        let mut v = vec![self.read_board()];
        v.extend(self.read_source());
        v
    }

    /// A card named by its element key, or the one open in the panel.
    fn card_or_open(&self, id: Option<&str>) -> Result<&J, String> {
        match id.filter(|i| !i.is_empty()) {
            Some(i) => self.card(i).ok_or_else(|| format!("{i} is not a card on the board")),
            None => self.open_card(),
        }
    }

    fn cards(&self) -> Vec<&J> {
        self.board["columns"].as_array().into_iter().flatten().flat_map(|c| c["cards"].as_array().into_iter().flatten()).collect()
    }

    /// The daemon's card of an element key.
    fn card(&self, id: &str) -> Option<&J> {
        self.cards().into_iter().find(|c| model::el(c["key"].as_str().unwrap_or("")) == id)
    }

    fn open_card(&self) -> Result<&J, String> {
        self.open.as_deref().and_then(|id| self.card(id)).ok_or_else(|| "no ticket is open".to_string())
    }

    fn open_ticket(&self) -> Result<(String, u32, &J), String> {
        let c = self.open_card()?;
        let (sb, n) = model::ticket_of(c).ok_or("a thread card: it has no ticket")?;
        Ok((sb, n, c))
    }

    fn read_ticket(&self, sb: &str, n: u32) -> Action {
        Action::Call(Call::get(format!("/api/tickets/{}/{n}", enc(sb)), "read the ticket").tag(&format!("kb-ticket:{sb}/{n}")).quiet())
    }

    fn post(&self, sb: &str, n: u32, what_path: &str, body: J, what: &str, tag: &str) -> Action {
        Action::Call(Call::post(format!("/api/tickets/{}/{n}/{what_path}", enc(sb)), body, what).tag(tag).quiet())
    }

    pub fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let mut out = vec![];
        match a.name {
            "kanban_show" => {
                let sb = a.s("sandbox").map(str::to_string).or_else(|| self.scope.clone()).or_else(|| cx.get("app")["selected"].as_str().map(str::to_string));
                out.extend(self.show(sb));
            }
            "kanban_toggle" => {
                if shown(cx) {
                    // Closed as Escape or its cross close it: back to the
                    // base view, or the focused terminal gets the keyboard.
                    return Ok(vec![Action::ClosePanel(SCREEN.into())]);
                }
                let sb = context_sandbox(cx).or_else(|| self.scope.clone());
                out.extend(self.show(sb));
            }
            "kanban_scope" => {
                let sb = a.req("sandbox");
                if !sb.is_empty() && cx.sandbox(sb).is_none() {
                    return Err(format!("{sb} is not a sandbox sbxw lists"));
                }
                self.scope = (!sb.is_empty()).then(|| sb.to_string());
                self.reset();
                out.extend(self.read_all());
            }
            "kanban_open" => {
                let id = a.req("id");
                if id.is_empty() {
                    let back = self.open.take();
                    self.view = None;
                    out.extend(self.views(cx));
                    if let Some(k) = back {
                        out.push(Action::Focus(k));
                    }
                    return Ok(out);
                }
                let c = self.card(id).cloned().ok_or_else(|| format!("{id} is not a card on the board"))?;
                self.open = Some(id.to_string());
                self.focus = Some(id.to_string());
                self.view = None;
                if let Some((sb, n)) = model::ticket_of(&c) {
                    out.push(self.read_ticket(&sb, n));
                }
            }
            "kanban_column" => self.col = a.req("column").to_string(),
            "kanban_more" => {
                let c = a.req("column").to_string();
                let p = self.pages.get(&c).copied().unwrap_or(1);
                self.pages.insert(c, (p + 1).min(model::MAX_PAGES));
            }
            "kanban_older" => {
                self.older = true;
                out.push(self.read_board());
            }
            "kanban_refresh" => out.extend(self.read_all()),
            "kanban_issue" => {
                let c = self.card_or_open(a.s("id"))?;
                if !model::imported(c) {
                    return Err("not an imported ticket: it has no issue".into());
                }
                let url = model::issue_url(c).ok_or("its link is not an http or https address: not opened")?;
                return Ok(vec![Action::OpenLink(url.to_string())]);
            }
            "kanban_plan" => {
                let c = self.card_or_open(a.s("id"))?;
                if !model::can_plan(c) {
                    return Err(if c["column"] == "backlog" { "this ticket does not move to Planned now".into() } else { "only a Backlog ticket moves to Planned".into() });
                }
                let (sb, n) = model::ticket_of(c).ok_or("a card with no ticket")?;
                return Ok(vec![self.post(&sb, n, "plan", json!({}), "move the ticket to Planned", "kb-plan")]);
            }
            "kanban_source_open" => return self.source_open(cx),
            "kanban_source_field" => {
                let f = self.source_form.as_mut().ok_or("the source dialog is not open")?;
                if let Some(t) = a.s("tracker") {
                    f.tracker = t.to_string();
                }
                if let Some(r) = a.s("repo") {
                    f.repo = r.chars().take(200).collect();
                    f.detected = false;
                }
                if let Some(l) = a.s("label") {
                    f.label = l.chars().take(60).collect();
                }
                if let Some(b) = a.b("auto_close") {
                    f.auto_close = b;
                }
                f.error = None;
                return Ok(vec![self.source_ui()]);
            }
            "kanban_source_cancel" => {
                self.source_form = None;
                return Ok(vec![Action::CloseModal, Action::Set { key: SOURCE_UI.into(), value: J::Null }]);
            }
            "kanban_source_refresh" => {
                let sb = self.scope.clone().ok_or("pick a sandbox: a source is one sandbox's")?;
                if self.source.as_ref().is_none_or(|s| s["configured"] != true) {
                    return Err(format!("{sb} has no ticket source"));
                }
                return Ok(vec![Action::Call(Call::post(format!("/api/tickets/{}/source/refresh", enc(&sb)), json!({}), "sync the ticket source").tag("kb-src-refresh").quiet())]);
            }
            "kanban_source_save" => return self.source_save(),
            "kanban_source_remove" => return self.source_remove(),
            "kanban_key" => return self.key(cx, a.req("key"), a.s("text")),
            "kanban_new" => return Ok(self.new_form(cx, None)),
            "kanban_edit" => {
                let (sb, n, c) = self.open_ticket()?;
                if model::imported(c) {
                    return Err(format!("an imported ticket is edited in {}", model::tracker_label(c["origin"]["tracker"].as_str().unwrap_or(""))));
                }
                if c["can"]["edit"] != true {
                    return Err("this card is recorded from a thread: nothing to edit".into());
                }
                let v = self.view.clone().ok_or("the ticket is still being read")?;
                return Ok(self.new_form(cx, Some((sb, n, v))));
            }
            "kanban_form" => {
                let f = self.form.as_mut().ok_or("no form is open")?;
                if let Some(t) = a.s("title") {
                    f.title = t.chars().take(MAX_TITLE + 40).collect();
                }
                if let Some(t) = a.s("description") {
                    f.description = t.chars().take(MAX_DESCRIPTION + 200).collect();
                }
                if let Some(p) = a.s("priority") {
                    f.priority = p.to_string();
                }
                if let Some(s) = a.s("sandbox") {
                    if f.edit.is_none() {
                        if !s.is_empty() && cx.sandbox(s).is_none() {
                            return Err(format!("{s} is not a sandbox sbxw lists"));
                        }
                        f.sandbox = s.to_string();
                    }
                }
                f.error = None;
                return Ok(vec![self.form_ui()]);
            }
            "kanban_attach" => {
                let f = self.form.as_ref().ok_or("open the form first")?;
                if f.files.len() >= MAX_FILES {
                    return Err(format!("a ticket holds {MAX_FILES} files at most"));
                }
                self.pick_armed = true;
                return Ok(vec![Action::PickFiles { title: "Attach to the ticket".into(), extensions: vec![], multiple: true }]);
            }
            "kanban_paste" => {
                self.form.as_ref().ok_or("open the form first")?;
                self.pick_armed = true;
                return Ok(vec![Action::ReadClipboard]);
            }
            "kanban_unattach" => {
                let f = self.form.as_mut().ok_or("no form is open")?;
                let i = a.i("index").unwrap_or(-1);
                if i < 0 || i as usize >= f.files.len() {
                    return Err("no such file in the form".into());
                }
                let gone = f.files.remove(i as usize);
                if let Some(p) = gone.path {
                    out.push(Action::ForgetFiles(vec![p]));
                }
                out.push(self.form_ui());
                return Ok(out);
            }
            "kanban_save" => return self.save(cx),
            "kanban_cancel" => {
                if let Some(f) = self.form.take() {
                    let paths: Vec<PathBuf> = f.files.into_iter().filter_map(|h| h.path).collect();
                    if !paths.is_empty() {
                        out.push(Action::ForgetFiles(paths));
                    }
                }
                out.push(Action::CloseModal);
                out.push(Action::Set { key: FORM_UI.into(), value: J::Null });
                return Ok(out);
            }
            "kanban_delete" => {
                let (sb, n, c) = self.open_ticket()?;
                if model::imported(c) {
                    return Err("an imported ticket goes when its source is disconnected".into());
                }
                if c["can"]["delete"] != true {
                    return Err(c["can"]["delete_why"].as_str().unwrap_or("not deleted").to_string());
                }
                let files = self.view.as_ref().and_then(|v| v["files"].as_array().map(|f| f.len())).unwrap_or(c["files"].as_u64().unwrap_or(0) as usize);
                let bytes = c["bytes"].as_u64().unwrap_or(0);
                let call = Call::post(format!("/api/tickets/{}/{n}/delete", enc(&sb)), json!({}), format!("delete T-{n}")).tag("kb-del");
                let mut conf = Confirm::new(format!("Delete T-{n}?"), call).danger();
                conf = conf.line(format!("T-{n} of {sb}, \"{}\", is deleted from this computer.", model::clip(c["title"].as_str().unwrap_or(""), 80)));
                conf = conf.line(match files {
                    0 => "It has no file.".to_string(),
                    1 => format!("Its file goes with it ({}).", crate::feat::files::view::size_label(bytes)),
                    k => format!("Its {k} files go with it ({} in all).", crate::feat::files::view::size_label(bytes)),
                });
                conf = conf.line("Nothing in a sandbox changes; a copy an agent read stays where it was copied.");
                return Ok(vec![Action::Confirm(Box::new(conf.labels("Delete", "Cancel")))]);
            }
            "kanban_remove_file" => {
                let (sb, n, c) = self.open_ticket()?;
                if model::imported(c) {
                    return Err("an imported ticket is read-only here".into());
                }
                let fid = a.req("file");
                let v = self.view.as_ref().ok_or("the ticket is still being read")?;
                let f = v["files"].as_array().into_iter().flatten().find(|f| f["id"] == fid).ok_or("no such file on this ticket")?;
                let call = Call::post(format!("/api/tickets/{}/{n}/files/{}/remove", enc(&sb), enc(fid)), json!({}), "remove a file").tag("kb-rmf");
                let conf = Confirm::new(format!("Remove a file from T-{n}?"), call)
                    .danger()
                    .line(format!("{} ({}) is removed from T-{n} and from this computer.", f["name"].as_str().unwrap_or("The file"), f["size"].as_str().unwrap_or("")))
                    .labels("Remove", "Cancel");
                return Ok(vec![Action::Confirm(Box::new(conf))]);
            }
            "kanban_move" => {
                let (sb, n, c) = self.open_ticket()?;
                if c["can"]["reorder"] != true {
                    return Err(c["can"]["reorder_why"].as_str().unwrap_or("it does not move").to_string());
                }
                out.push(self.post(&sb, n, "move", json!({ "to": a.req("to") }), "move the ticket", "kb-move"));
                return Ok(out);
            }
            "kanban_priority" => {
                let (sb, n, c) = self.open_ticket()?;
                if !model::editable(c) {
                    return Err(if model::imported(c) { "an imported ticket is read-only here".into() } else { "a card recorded from a thread has no priority".into() });
                }
                out.push(self.post(&sb, n, "edit", json!({ "priority": a.req("priority") }), "set the priority", "kb-edit"));
                return Ok(out);
            }
            "kanban_validate" => {
                let (sb, n, c) = self.open_ticket()?;
                if c["column"] != "merged" {
                    return Err("only a merged ticket is validated on your machine".into());
                }
                out.push(self.post(&sb, n, "edit", json!({ "validated": a.b("on").unwrap_or(true) }), "mark it validated", "kb-edit"));
                return Ok(out);
            }
            "kanban_send" => return self.send(cx),
            "kanban_file" => {
                let (sb, n, _) = self.open_ticket()?;
                let fid = a.req("file");
                let v = self.view.as_ref().ok_or("the ticket is still being read")?;
                let f = v["files"].as_array().into_iter().flatten().find(|f| f["id"] == fid).ok_or("no such file on this ticket")?;
                let name = f["name"].as_str().unwrap_or("file").to_string();
                let get = format!("/api/tickets/{}/{n}/files/{}", enc(&sb), enc(fid));
                if a.b("open") == Some(true) {
                    let dl = Download { get, name: name.clone(), to: Dest::Downloads, after: After::Open, tag: "kb-dl".into() };
                    let conf = Confirm::new(format!("Open {name}?"), dl.call(format!("open {name}"))).line("It is saved to your Downloads folder, then opened with the app your system picks, after its check.").labels("Open", "Cancel");
                    return Ok(vec![Action::Confirm(Box::new(conf))]);
                }
                return Ok(vec![Action::Download(Download { get, name, to: Dest::Downloads, after: After::Reveal, tag: "kb-dl".into() })]);
            }
            "kanban_link" => return self.link(a.req("what")),
            other => return Err(format!("{other}: not a kanban intent")),
        }
        out.extend(self.views(cx));
        Ok(out)
    }

    /// Opens the overlay on `sb` (`None` or empty: every sandbox) and reads it.
    fn show(&mut self, sb: Option<String>) -> Vec<Action> {
        self.scope = sb.filter(|s| !s.is_empty());
        self.reset();
        let mut v = vec![Action::OpenPanel(SCREEN.into())];
        v.extend(self.read_all());
        v
    }

    fn reset(&mut self) {
        self.gen += 1;
        self.board = J::Null;
        self.error = None;
        self.older = false;
        self.pages.clear();
        self.open = None;
        self.view = None;
        self.focus = None;
        self.source = None;
    }

    fn source_open(&mut self, cx: &Ctx) -> Result<Vec<Action>, String> {
        let sb = self.scope.clone().ok_or("pick a sandbox: a ticket source is one sandbox's")?;
        cx.sandbox(&sb).ok_or_else(|| format!("{sb} is not a sandbox sbxw lists"))?;
        let src = self.source.clone().ok_or("this daemon has no ticket sources")?;
        let configured = src["configured"] == true;
        let det = src["detected"]["repo"].as_str().filter(|r| !r.is_empty());
        let (repo, detected) = match src["repo"].as_str().filter(|r| configured && !r.is_empty()) {
            Some(r) => (r.to_string(), false),
            None => (det.unwrap_or("").to_string(), det.is_some()),
        };
        let tracker = src["tracker"].as_str().filter(|t| configured && TRACKERS.contains(t)).or_else(|| src["detected"]["tracker"].as_str().filter(|t| TRACKERS.contains(t))).unwrap_or("github");
        let label = if configured { src["label"].as_str().unwrap_or("").to_string() } else { DEFAULT_LABEL.to_string() };
        self.source_form = Some(SourceForm { sandbox: sb, tracker: tracker.into(), repo, label, auto_close: configured && src["auto_close"] == true, detected, configured, saving: false, error: None });
        Ok(vec![self.source_ui(), Action::OpenModal { screen: SOURCE.into(), arg: J::Null }, Action::Focus("kanban-source-repo".into())])
    }

    fn source_save(&mut self) -> Result<Vec<Action>, String> {
        let f = self.source_form.as_mut().ok_or("the source dialog is not open")?;
        if f.saving {
            return Err("the source is being saved".into());
        }
        let repo = f.repo.trim().to_string();
        let label = f.label.trim().to_string();
        if !repo_ok(&repo) {
            f.error = Some("Write the repository as owner/name, for example acme/web-app.".into());
            return Ok(vec![self.source_ui()]);
        }
        if label.chars().any(|c| c.is_control()) {
            f.error = Some("A label is one line of text.".into());
            return Ok(vec![self.source_ui()]);
        }
        let tracker = model::tracker_label(&f.tracker);
        let body = json!({ "tracker": f.tracker, "repo": repo, "label": label, "auto_close": f.auto_close });
        let call = Call::post(format!("/api/tickets/{}/source", enc(&f.sandbox)), body, format!("set {}'s ticket source", f.sandbox)).tag("kb-src-save").quiet();
        f.saving = true;
        f.error = None;
        let mut conf = Confirm::new(format!("Connect {} to {tracker} {repo}?", f.sandbox), call)
            .line(if label.is_empty() { format!("The open issues of {repo} come in as Backlog tickets of {}.", f.sandbox) } else { format!("The open issues of {repo} labelled \"{label}\" come in as Backlog tickets of {}.", f.sandbox) })
            .line(writes_line(&tracker));
        conf = conf.line(if f.auto_close { "When a ticket is merged, sbxw closes its issue.".to_string() } else { "sbxw never closes an issue: you close it.".to_string() });
        let label_btn = if f.configured { "Save" } else { "Connect" };
        Ok(vec![Action::Confirm(Box::new(conf.labels(label_btn, "Cancel"))), self.source_ui()])
    }

    fn source_remove(&mut self) -> Result<Vec<Action>, String> {
        let f = self.source_form.as_mut().ok_or("the source dialog is not open")?;
        if !f.configured {
            return Err(format!("{} has no ticket source", f.sandbox));
        }
        let tracker = model::tracker_label(&f.tracker);
        let call = Call::post(format!("/api/tickets/{}/source/remove", enc(&f.sandbox)), json!({}), format!("disconnect {}'s ticket source", f.sandbox)).tag("kb-src-rm").quiet();
        f.saving = true;
        let conf = Confirm::new(format!("Disconnect {} from {tracker}?", f.sandbox), call)
            .danger()
            .line(format!("The imported tickets still in Backlog are removed from {}.", f.sandbox))
            .line("Imported tickets already planned or worked on stay, as your own.")
            .line(format!("sbxw writes nothing more to {tracker}; what it wrote there stays."))
            .labels("Disconnect", "Cancel");
        Ok(vec![Action::Confirm(Box::new(conf)), self.source_ui()])
    }

    fn new_form(&mut self, cx: &Ctx, edit: Option<(String, u32, J)>) -> Vec<Action> {
        let f = match edit {
            Some((sb, n, v)) => Form { edit: Some((sb.clone(), n)), sandbox: sb, title: v["title"].as_str().unwrap_or("").into(), description: v["description"].as_str().unwrap_or("").into(), priority: v["priority"].as_str().unwrap_or("none").into(), ..Form::default() },
            None => {
                let sb = self.scope.clone().or_else(|| cx.get("app")["selected"].as_str().map(str::to_string)).unwrap_or_default();
                Form { sandbox: sb, priority: "none".into(), ..Form::default() }
            }
        };
        self.form = Some(f);
        vec![self.form_ui(), Action::OpenModal { screen: FORM.into(), arg: J::Null }, Action::Focus("kanban-form-title".into())]
    }

    fn key(&mut self, cx: &Ctx, key: &str, text: Option<&str>) -> Result<Vec<Action>, String> {
        if key == "escape" {
            if self.open.is_some() {
                return self.act(cx, &Asked { name: "kanban_open", args: json!({ "id": "" }).as_object().cloned().unwrap_or_default() });
            }
            return Ok(vec![]);
        }
        if key == "type" {
            return Ok(if matches!(text, Some("n") | Some("N")) { self.new_form(cx, None) } else { vec![] });
        }
        let narrow = cx.get("app")["overlay"]["w"].as_f64().is_some_and(|w| w < 900.0);
        let cols = model::columns(&self.board, &cx.get("app")["groups"], &self.pages, None);
        let order = model::order(&cols);
        let ids: Vec<String> = cols.iter().filter_map(|c| c["id"].as_str().map(str::to_string)).collect();
        let at = self.focus.as_ref().and_then(|f| order.iter().enumerate().find_map(|(ci, col)| col.iter().position(|x| x == f).map(|ri| (ci, ri))));
        let shown_col = if narrow { ids.iter().position(|c| *c == self.col).unwrap_or(0) } else { 0 };
        let (mut ci, mut ri) = at.unwrap_or((shown_col, 0));
        match key {
            "enter" => {
                let Some(f) = self.focus.clone() else { return Ok(vec![]) };
                return self.act(cx, &Asked { name: "kanban_open", args: json!({ "id": f }).as_object().cloned().unwrap_or_default() });
            }
            "up" if at.is_some() => ri = ri.saturating_sub(1),
            "down" if at.is_some() => ri += 1,
            "left" | "right" => {
                let step = |c: usize| if key == "left" { c.checked_sub(1) } else { (c + 1 < order.len()).then_some(c + 1) };
                let mut c = ci;
                // The next column that has a card.
                loop {
                    match step(c) {
                        Some(n) if order[n].is_empty() => c = n,
                        Some(n) => {
                            ci = n;
                            break;
                        }
                        None => break,
                    }
                }
                if narrow {
                    self.col = ids.get(ci).cloned().unwrap_or_default();
                }
            }
            _ => {}
        }
        let Some(col) = order.get(ci).filter(|c| !c.is_empty()) else { return Ok(self.views(cx)) };
        let id = col[ri.min(col.len() - 1)].clone();
        self.focus = Some(id.clone());
        let mut out = self.views(cx);
        out.push(Action::Focus(id.clone()));
        out.push(Action::ScrollIntoView(id));
        Ok(out)
    }

    fn save(&mut self, cx: &Ctx) -> Result<Vec<Action>, String> {
        let f = self.form.as_mut().ok_or("no form is open")?;
        if f.saving {
            return Err("the ticket is being saved".into());
        }
        let title = f.title.trim();
        if title.is_empty() {
            f.error = Some("A ticket needs a title.".into());
            return Ok(vec![self.form_ui()]);
        }
        if title.chars().count() > MAX_TITLE {
            f.error = Some(format!("A title is at most {MAX_TITLE} characters ({} now).", title.chars().count()));
            return Ok(vec![self.form_ui()]);
        }
        if f.description.chars().count() > MAX_DESCRIPTION {
            f.error = Some(format!("A description is at most {MAX_DESCRIPTION} characters ({} now).", f.description.chars().count()));
            return Ok(vec![self.form_ui()]);
        }
        if f.sandbox.is_empty() || cx.sandbox(&f.sandbox).is_none() {
            f.error = Some("Pick the sandbox the ticket is for.".into());
            return Ok(vec![self.form_ui()]);
        }
        let body = json!({ "title": title, "description": f.description, "priority": f.priority });
        let call = match &f.edit {
            Some((sb, n)) => Call::post(format!("/api/tickets/{}/{n}/edit", enc(sb)), body, format!("save T-{n}")),
            None => Call::post(format!("/api/tickets/{}", enc(&f.sandbox)), body, "save the ticket"),
        }
        .tag("kb-save")
        .quiet();
        f.saving = true;
        f.error = None;
        let look: Vec<String> = f.files.iter().flat_map(|h| h.look.iter().cloned()).collect();
        if look.is_empty() {
            return Ok(vec![Action::Call(call), self.form_ui()]);
        }
        // A file that looks like a secret: one question, with the ack box.
        let total: u64 = f.files.iter().map(|h| h.size).sum();
        let mut conf = Confirm::new("Attach these files to the ticket?", call).line(format!(
            "{} file{}, {} in all, kept on this computer with the ticket; an agent that reads the ticket gets a copy in its sandbox.",
            f.files.len(),
            if f.files.len() == 1 { "" } else { "s" },
            crate::feat::files::view::size_label(total)
        ));
        for w in &look {
            conf = conf.line(format!("Look first: {}", crate::feat::monitor::attach::written_out(w)));
        }
        conf = conf.ack("I have looked at these files and want to attach them").labels("Save", "Cancel");
        Ok(vec![Action::Confirm(Box::new(conf)), self.form_ui()])
    }

    /// The next file of a save, or the end of it.
    fn upload_next(&mut self, cx: &Ctx) -> Vec<Action> {
        let Some(f) = self.form.as_mut() else { return vec![] };
        let Some((sb, n, i)) = f.job.clone() else { return vec![] };
        if i >= f.files.len() {
            let paths: Vec<PathBuf> = f.files.iter().filter_map(|h| h.path.clone()).collect();
            let k = f.files.len();
            self.form = None;
            let mut out = vec![Action::CloseModal, Action::Set { key: FORM_UI.into(), value: J::Null }];
            if !paths.is_empty() {
                out.push(Action::ForgetFiles(paths));
            }
            out.push(Action::notice(if k == 0 { format!("T-{n} saved") } else { format!("T-{n} saved with {k} file{}", if k == 1 { "" } else { "s" }) }));
            self.open = Some(model::el(&format!("t:{sb}:{n}")));
            self.focus = self.open.clone();
            self.view = None;
            out.push(self.read_board());
            out.push(self.read_ticket(&sb, n));
            out.extend(self.views(cx));
            return out;
        }
        let h = &f.files[i];
        let bytes = match (&h.bytes, &h.path) {
            (Some(b), _) => Ok(b.clone()),
            (None, Some(p)) => read_handed(p),
            _ => Err("nothing to read".to_string()),
        };
        match bytes {
            Ok(b) => {
                let path = format!("/api/tickets/{}/{n}/files?name={}", enc(&sb), enc(&h.name));
                vec![Action::Call(Call::post_bytes(path, "application/octet-stream", b, format!("attach {}", h.name)).tag(&format!("kb-up:{i}")).quiet()), self.form_ui()]
            }
            Err(e) => {
                f.saving = false;
                f.job = None;
                f.error = Some(format!("T-{n} is saved, but {} could not be read ({e}); the files before it are attached. Remove it and save again.", h.name));
                f.edit = Some((sb.clone(), n));
                f.files.drain(..i);
                vec![self.form_ui(), self.read_board()]
            }
        }
    }

    fn send(&mut self, cx: &Ctx) -> Result<Vec<Action>, String> {
        if let Some(why) = held(cx) {
            return Err(format!("Not sent: {why}."));
        }
        let (sb, n, c) = self.open_ticket()?;
        if c["can"]["send"] != true {
            return Err(format!("Not sent: {}.", c["can"]["send_why"].as_str().unwrap_or("not a planned ticket").to_lowercase()));
        }
        // The sandbox's main session, as the app sees it now.
        let mains: Vec<&J> = cx.get("sessions").as_array().into_iter().flatten().filter(|i| i["sandbox"] == sb.as_str() && i["thread"].as_str().is_none_or(str::is_empty)).collect();
        let live: Vec<&&J> = mains.iter().filter(|i| crate::feat::monitor::model::Group::of(i["state"].as_str().unwrap_or("")) != crate::feat::monitor::model::Group::Ended).collect();
        if live.is_empty() {
            return Err(format!("Not sent: {sb} has no open agent session; open its terminal first."));
        }
        let waits = live.iter().any(|i| crate::feat::monitor::model::Group::of(i["state"].as_str().unwrap_or("")) == crate::feat::monitor::model::Group::You);
        let line = send_line(n);
        let call = Call::post(format!("/api/tickets/{}/{n}/send", enc(&sb)), json!({}), format!("tell {sb}'s lead about T-{n}")).tag("kb-send");
        let mut conf = Confirm::new(format!("Send T-{n} to {sb}'s lead?"), call)
            .line(format!("To: {sb}'s main session (its lead), typed as one line, shown in full below."))
            .line("Only the ticket's number is typed: the lead reads its words and files with ticket_read, as the person's request.".to_string());
        if waits {
            conf = conf.line(format!("{sb}'s session waits on a question now: the line is typed once it is answered (a team note's rule)."));
        }
        conf = conf.preview(line, false).labels(&format!("Send to {sb}"), "Cancel");
        Ok(vec![Action::Confirm(Box::new(conf))])
    }

    fn link(&self, what: &str) -> Result<Vec<Action>, String> {
        let c = self.open_card()?;
        let sb = c["sandbox"].as_str().unwrap_or("").to_string();
        let thread = c["thread"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        let go = |name: &str, args: J| Action::Intent { name: name.into(), args };
        let close = Action::ClosePanel(SCREEN.into());
        Ok(match what {
            "terminal" => {
                let t = thread.ok_or("no thread works on this ticket yet")?;
                vec![close, go("open_terminal", json!({ "sandbox": sb, "thread": t, "pane": "spare" }))]
            }
            "monitor" => {
                if crate::feat::intent("monitor_open").is_none() {
                    return Err("Sessions is not in this build".into());
                }
                let t = thread.ok_or("no thread works on this ticket yet")?;
                vec![go("monitor_open", json!({})), go("monitor_select", json!({ "key": format!("{sb}::{t}") }))]
            }
            "commits" => vec![go("commits_show", json!({ "sandbox": sb, "branch": thread.map(|t| format!("sbxw/{t}")) }))],
            "worktrees" => vec![go("worktrees_show", json!({ "sandbox": sb }))],
            "plan" => {
                let id = c["plan"].as_str().ok_or("no plan holds this ticket yet")?;
                vec![go("plan_review", json!({ "id": id }))]
            }
            w => return Err(format!("{w}: not a place the Kanban links to")),
        })
    }

    pub fn on_event(&mut self, cx: &Ctx, name: &str, data: &J) -> Vec<Action> {
        if !shown(cx) {
            return vec![];
        }
        if name == "tickets" {
            if let (Some(s), Some(sb)) = (&self.scope, data["sandbox"].as_str()) {
                if s != sb {
                    return vec![];
                }
            }
        }
        let mut out = vec![self.read_board()];
        // The source changed or synced: its line is read again (only from a
        // daemon that has sources: an older one never sends these).
        if name == "tickets" && matches!(data["what"].as_str(), Some("source" | "synced")) {
            out.extend(self.read_source());
        }
        if let Ok((sb, n, _)) = self.open_ticket() {
            out.push(self.read_ticket(&sb, n));
        }
        out
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let body = match r {
            Ok((200, b)) => Ok(b),
            Ok((code, b)) => Err(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"))),
            Err(e) => Err(e),
        };
        let tag = call.tag.as_str();
        if let Some(rest) = tag.strip_prefix("kb-source:") {
            let mine = format!("{}:{}", self.gen, self.scope.as_deref().unwrap_or(""));
            if rest != mine {
                return vec![];
            }
            // A 404 (an older daemon) or any failure: no source line, the Kanban as before.
            self.source = match body.as_ref().ok() {
                Some(b) if b.is_object() && !b["status"].is_null() => Some(b.clone()),
                _ => None,
            };
            return self.views(cx);
        }
        if tag == "kb-src-save" || tag == "kb-src-rm" {
            let Some(f) = self.source_form.as_mut() else { return vec![] };
            return match body {
                Ok(_) => {
                    let removed = tag == "kb-src-rm";
                    let sb = f.sandbox.clone();
                    self.source_form = None;
                    let mut out = vec![Action::CloseModal, Action::Set { key: SOURCE_UI.into(), value: J::Null }];
                    out.push(Action::notice(if removed { format!("{sb}'s ticket source is disconnected") } else { format!("{sb}'s ticket source is saved: its issues come in as Backlog tickets") }));
                    out.extend(self.read_all());
                    out
                }
                Err(e) => {
                    f.saving = false;
                    f.error = Some(format!("Not saved: {e}."));
                    vec![self.source_ui()]
                }
            };
        }
        if let Some(g) = tag.strip_prefix("kb-board:") {
            if g != self.gen.to_string() {
                return vec![];
            }
            self.loading = false;
            match body {
                Ok(b) => {
                    self.board = b["board"].clone();
                    self.error = None;
                    // A card gone (deleted, its plan pruned): its panel closes.
                    if self.open.as_deref().is_some_and(|id| self.card(id).is_none()) {
                        self.open = None;
                        self.view = None;
                    }
                }
                Err(e) => self.error = Some(e),
            }
            let mut out = vec![];
            // A ticket opened before its card was on the board (just saved): read it now.
            if self.view.is_none() {
                if let Ok((sb, n, _)) = self.open_ticket() {
                    out.push(self.read_ticket(&sb, n));
                }
            }
            out.extend(self.views(cx));
            return out;
        }
        if let Some(which) = tag.strip_prefix("kb-ticket:") {
            let mine = self.open_ticket().ok().map(|(sb, n, _)| format!("{sb}/{n}"));
            if mine.as_deref() != Some(which) {
                return vec![];
            }
            let mut out = vec![];
            match body {
                Ok(b) => {
                    let v = b["ticket"].clone();
                    let (sb, n) = which.split_once('/').map(|(s, n)| (s.to_string(), n.parse::<u32>().unwrap_or(0))).unwrap_or_default();
                    for f in v["files"].as_array().into_iter().flatten().filter(|f| f["picture"] == true).take(model::PREVIEWS) {
                        let id = f["id"].as_str().unwrap_or("");
                        let key = model::image_key(&sb, n, id);
                        if !self.previews.contains(&key) {
                            out.push(Action::Call(Call::get(format!("/api/tickets/{}/{n}/files/{}", enc(&sb), enc(id)), "read a picture").tag(&format!("kb-img:{key}")).raw(model::PREVIEW_MAX).quiet()));
                        }
                    }
                    self.view = Some(v);
                }
                Err(e) => out.push(Action::error(format!("The ticket could not be read: {e}"))),
            }
            out.extend(self.views(cx));
            return out;
        }
        if tag == "kb-save" {
            let Some(f) = self.form.as_mut() else { return vec![] };
            match body {
                Ok(b) => {
                    let n = b["ticket"]["n"].as_u64().unwrap_or(0) as u32;
                    let sb = f.edit.as_ref().map(|(s, _)| s.clone()).unwrap_or_else(|| f.sandbox.clone());
                    f.job = Some((sb, n, 0));
                    return self.upload_next(cx);
                }
                Err(e) => {
                    f.saving = false;
                    f.error = Some(format!("Not saved: {e}."));
                    return vec![self.form_ui()];
                }
            }
        }
        if let Some(i) = tag.strip_prefix("kb-up:") {
            let Some(f) = self.form.as_mut() else { return vec![] };
            let i: usize = i.parse().unwrap_or(0);
            match body {
                Ok(_) => {
                    if let Some(j) = f.job.as_mut() {
                        j.2 = i + 1;
                    }
                    return self.upload_next(cx);
                }
                Err(e) => {
                    let (sb, n, _) = f.job.take().unwrap_or_default();
                    let name = f.files.get(i).map(|h| h.name.clone()).unwrap_or_default();
                    f.saving = false;
                    f.edit = Some((sb, n));
                    f.files.drain(..i);
                    f.error = Some(format!("T-{n} is saved, but {name} was refused: {e}. Remove it and save again."));
                    return vec![self.form_ui(), self.read_board()];
                }
            }
        }
        let mut out = vec![];
        match (tag, body) {
            ("kb-del", Ok(b)) => {
                out.push(Action::notice(format!("Deleted, with {} file{}", b["files"], if b["files"] == 1 { "" } else { "s" })));
                let back = self.open.take();
                self.view = None;
                if back == self.focus {
                    self.focus = None;
                }
            }
            ("kb-send", Ok(b)) => out.push(Action::notice(if b["sent"] == "held" { "The lead waits on a question: the line is typed once it is answered" } else { "Sent to the lead: it reads the ticket with ticket_read" })),
            ("kb-dl", _) => return vec![],
            (_, Ok(_)) => {}
            (_, Err(e)) => out.push(Action::error(e)),
        }
        out.push(self.read_board());
        if tag == "kb-src-refresh" {
            out.extend(self.read_source());
        }
        if let Ok((sb, n, _)) = self.open_ticket() {
            out.push(self.read_ticket(&sb, n));
        }
        out
    }

    pub fn on_bytes(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, Vec<u8>, String), String>) -> Vec<Action> {
        let Some(key) = call.tag.strip_prefix("kb-img:") else { return vec![] };
        match r {
            Ok((200, bytes, _)) if (bytes.len() as u64) <= model::PREVIEW_MAX && crate::picture::sniff(&bytes).is_some() => {
                self.previews.insert(key.to_string());
                let mut out = vec![Action::Image { key: key.to_string(), bytes }];
                out.extend(self.views(cx));
                out
            }
            _ => vec![],
        }
    }

    pub fn on_files(&mut self, cx: &Ctx, inc: Incoming) -> Vec<Action> {
        let armed = std::mem::take(&mut self.pick_armed);
        if !form_shown(cx) && !armed {
            return vec![];
        }
        let Some(f) = self.form.as_mut() else { return vec![Action::error("Nothing attached: open a ticket's form first")] };
        let room = MAX_FILES.saturating_sub(f.files.len());
        let mut refused = vec![];
        match inc {
            Incoming::Png(b) => {
                if room == 0 {
                    refused.push(format!("a ticket holds {MAX_FILES} files at most"));
                } else {
                    let k = f.files.iter().filter(|h| h.path.is_none()).count() + 1;
                    f.files.push(Handed { path: None, name: format!("pasted-{k}.png"), size: b.len() as u64, bytes: Some(b), look: vec![] });
                }
            }
            Incoming::Files(paths) => {
                let many = paths.len();
                for (i, p) in paths.into_iter().enumerate() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                    if i >= room {
                        refused.push(format!("{name}: a ticket holds {MAX_FILES} files at most"));
                        continue;
                    }
                    match std::fs::metadata(&p) {
                        Ok(m) if !m.is_file() => refused.push(format!("{name}: a folder, not a file")),
                        Ok(m) if m.len() > MAX_FILE_BYTES => refused.push(format!("{name}: {}, more than 20 MB", crate::feat::files::view::size_label(m.len()))),
                        Ok(_) if refused_kind(&p).is_some() => refused.push(format!("{name}: {}", refused_kind(&p).unwrap_or_default())),
                        Ok(m) => {
                            let look = crate::feat::files::drops::guardrails(std::slice::from_ref(&p), many);
                            f.files.push(Handed { path: Some(p), name, size: m.len(), bytes: None, look });
                        }
                        Err(e) => refused.push(format!("{name}: could not be read ({e})")),
                    }
                }
            }
        }
        let mut out = vec![self.form_ui()];
        if !refused.is_empty() {
            out.push(Action::error(format!("Not attached: {}", refused.join("; "))));
        }
        out
    }

    /// `data.kanban_ui` (and the form's, when open).
    pub fn views(&self, cx: &Ctx) -> Vec<Action> {
        let mut v = vec![self.ui(cx)];
        if self.form.is_some() {
            v.push(self.form_ui());
        }
        if self.source_form.is_some() {
            v.push(self.source_ui());
        }
        v
    }

    fn source_ui(&self) -> Action {
        let Some(f) = &self.source_form else { return Action::Set { key: SOURCE_UI.into(), value: J::Null } };
        let tracker = model::tracker_label(&f.tracker);
        let v = json!({
            "sandbox": f.sandbox,
            "heading": if f.configured { "Ticket source" } else { "Connect a ticket source" },
            "trackers": TRACKERS.iter().map(|t| json!({ "id": t, "label": model::tracker_label(t) })).collect::<Vec<_>>(),
            "tracker": f.tracker,
            "tracker_note": "GitHub only for now.",
            "repo": f.repo,
            "repo_note": if f.detected { J::from(format!("Filled from {}'s own repository.", f.sandbox)) } else { J::Null },
            "label": f.label,
            "label_note": if f.label.trim().is_empty() { "Every open issue is imported." } else { "Only open issues with this label are imported." },
            "auto_close": f.auto_close,
            "writes": writes_line(&tracker),
            "configured": f.configured,
            "saving": f.saving,
            "error": f.error,
            "save_label": if f.configured { "Save" } else { "Connect" },
            "save_why": if f.saving { J::from("Saving…") } else if f.repo.trim().is_empty() { J::from("Write the repository first") } else { J::Null },
        });
        Action::Set { key: SOURCE_UI.into(), value: v }
    }

    fn ui(&self, cx: &Ctx) -> Action {
        let groups = cx.get("app")["groups"].clone();
        let cols = model::columns(&self.board, &groups, &self.pages, self.focus.as_deref());
        let col = if cols.iter().any(|c| c["id"] == self.col.as_str()) { self.col.clone() } else { cols.iter().find(|c| c["count"].as_u64().unwrap_or(0) > 0).and_then(|c| c["id"].as_str()).unwrap_or("planned").to_string() };
        let panel = self.open.as_deref().and_then(|id| self.card(id)).map(|c| model::panel(c, self.view.as_ref(), &self.previews, held(cx).as_deref()));
        let mut sandboxes = vec![json!({ "id": "", "label": "All sandboxes" })];
        sandboxes.extend(cx.get("sandboxes").as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).map(|n| json!({ "id": n })));
        let now = self.board["now"].as_u64().unwrap_or_else(crate::feat::now_ms);
        let source = match (&self.scope, &self.source) {
            (Some(_), Some(s)) => model::source_line(s, now),
            _ => J::Null,
        };
        let v = json!({
            "source": source,
            "scope": self.scope.clone().unwrap_or_default(),
            "sandboxes": sandboxes,
            "columns": cols,
            "col": col,
            "counts_line": self.board["counts_line"],
            "loading": self.loading && self.board.is_null(),
            "error": self.error,
            "ticket": panel,
            // Escape closes the panel before the overlay.
            "popup": self.open.is_some(),
            "new_why": if cx.get("sandboxes").as_array().is_none_or(|a| a.is_empty()) { J::from("No sandbox yet: a ticket is for one sandbox's repository") } else { J::Null },
        });
        Action::Set { key: UI.into(), value: v }
    }

    fn form_ui(&self) -> Action {
        let Some(f) = &self.form else { return Action::Set { key: FORM_UI.into(), value: J::Null } };
        let files: Vec<J> = f
            .files
            .iter()
            .enumerate()
            .map(|(i, h)| {
                json!({ "i": i, "name": h.name, "size": crate::feat::files::view::size_label(h.size), "look": if h.look.is_empty() { J::Null } else { h.look.iter().map(|w| crate::feat::monitor::attach::written_out(w)).collect::<Vec<_>>().join("; ").into() }, "el": format!("kanban-form-file-{i}") })
            })
            .collect();
        let total: u64 = f.files.iter().map(|h| h.size).sum();
        let tc = f.title.chars().count();
        let dc = f.description.chars().count();
        let v = json!({
            "title_text": f.title,
            "description": f.description,
            "priority": f.priority,
            "sandbox": f.sandbox,
            "edit": f.edit.as_ref().map(|(_, n)| format!("T-{n}")),
            "heading": match &f.edit { Some((_, n)) => format!("Edit T-{n}"), None => "New ticket".into() },
            "files": files,
            "files_label": if f.files.is_empty() { String::new() } else { format!("{} file{} · {}", f.files.len(), if f.files.len() == 1 { "" } else { "s" }, crate::feat::files::view::size_label(total)) },
            "title_count": if tc > MAX_TITLE - 20 { format!("{tc} / {MAX_TITLE}") } else { String::new() },
            "title_over": tc > MAX_TITLE,
            "description_count": if dc > MAX_DESCRIPTION - 500 { format!("{dc} / {MAX_DESCRIPTION}") } else { String::new() },
            "description_over": dc > MAX_DESCRIPTION,
            "saving": f.saving,
            "error": f.error,
            "attach_why": if f.files.len() >= MAX_FILES { J::from(format!("A ticket holds {MAX_FILES} files at most")) } else { J::Null },
            "save_why": if f.saving { J::from("Saving…") } else if f.title.trim().is_empty() { J::from("A ticket needs a title") } else { J::Null },
        });
        Action::Set { key: FORM_UI.into(), value: v }
    }
}

/// What the daemon refuses by content (`sbxw_core::tickets::sniff`), said
/// at once when the file is handed in: archives and native programs.
pub fn refused_kind(p: &std::path::Path) -> Option<&'static str> {
    use std::io::Read;
    let mut b = [0u8; 264];
    let n = std::fs::File::open(p).and_then(|mut f| f.read(&mut b)).unwrap_or(0);
    let b = &b[..n];
    let starts = |m: &[u8]| b.len() >= m.len() && &b[..m.len()] == m;
    let archive = starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"\x1f\x8b") || starts(b"7z\xbc\xaf\x27\x1c") || starts(b"Rar!\x1a\x07") || starts(b"\xfd7zXZ\x00") || starts(b"BZh") || starts(b"\x28\xb5\x2f\xfd") || (b.len() > 262 && &b[257..262] == b"ustar");
    if archive {
        return Some("an archive: attach the files themselves");
    }
    if starts(b"\x7fELF") || starts(b"MZ") || starts(b"\xcf\xfa\xed\xfe") || starts(b"\xce\xfa\xed\xfe") || starts(b"\xca\xfe\xba\xbe") {
        return Some("a program: a ticket's files are reference material");
    }
    None
}

/// A handed file's bytes, at most 20 MB.
fn read_handed(p: &std::path::Path) -> Result<Vec<u8>, String> {
    let m = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if m.len() > MAX_FILE_BYTES {
        return Err("more than 20 MB".into());
    }
    std::fs::read(p).map_err(|e| e.to_string())
}

/// The stand-in's tickets: the fixture's `tickets_boards` (by sandbox, `all`,
/// and `<scope>|older`), `tickets_views` (`<sandbox>/<n>`), `tickets_files`
/// (`<sandbox>/<n>/<file>`, raw), `tickets_sources` (by sandbox: a missing
/// one answers 404, as an older daemon). A new ticket joins Planned (not the
/// first column: Backlog comes before it); `/plan` moves a Backlog ticket
/// there; every write sends the `tickets` event, as the daemon does.
fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    if !path.starts_with("/api/tickets") {
        return None;
    }
    let ok = |v: J| Some((200, v));
    if method == "GET" {
        if crate::feat::match_path("/api/tickets", path).is_some() {
            let sb = crate::feat::query(path, "sandbox").filter(|s| !s.is_empty()).unwrap_or_else(|| "all".into());
            let older = crate::feat::query(path, "older").is_some();
            let key = if older { format!("{sb}|older") } else { sb.clone() };
            let b = st.data["tickets_boards"][key.as_str()].clone();
            let b = if b.is_null() { st.data["tickets_boards"][sb.as_str()].clone() } else { b };
            if b.is_null() {
                return ok(json!({ "ok": true, "board": empty_board(&sb) }));
            }
            return ok(json!({ "ok": true, "board": b }));
        }
        if let Some(c) = crate::feat::match_path("/api/tickets/{sandbox}/source", path) {
            let v = st.data["tickets_sources"][c[0].1.as_str()].clone();
            return Some(if v.is_null() { (404, json!({ "ok": false, "error": "not found" })) } else { (200, v) });
        }
        if let Some(c) = crate::feat::match_path("/api/tickets/{sandbox}/{n}/files/{file}", path) {
            let k = format!("{}/{}/{}", c[0].1, c[1].1, c[2].1);
            let v = st.data["tickets_files"][k.as_str()].clone();
            return Some(if v.is_null() { (404, json!({ "ok": false, "error": "no such file" })) } else { (200, v) });
        }
        if let Some(c) = crate::feat::match_path("/api/tickets/{sandbox}/{n}", path) {
            let k = format!("{}/{}", c[0].1, c[1].1);
            let v = st.data["tickets_views"][k.as_str()].clone();
            return Some(if v.is_null() { (404, json!({ "ok": false, "error": "no such ticket" })) } else { (200, json!({ "ok": true, "ticket": v })) });
        }
        return None;
    }
    if method != "POST" {
        return None;
    }
    if let Some(c) = crate::feat::match_path("/api/tickets/{sandbox}/source/{what}", path) {
        let (sb, what) = (c[0].1.clone(), c[1].1.clone());
        if st.data["tickets_sources"][sb.as_str()].is_null() {
            return Some((404, json!({ "ok": false, "error": "not found" })));
        }
        match what.as_str() {
            "remove" => {
                let src = &mut st.data["tickets_sources"][sb.as_str()];
                src["configured"] = false.into();
                src["status"] = json!({ "state": "off", "last_ok_ms": null, "error": null, "hint": null });
                for b in st.data["tickets_boards"].as_object_mut().into_iter().flat_map(|m| m.values_mut()) {
                    if let Some(col) = column_mut(b, "backlog") {
                        if let Some(cards) = col["cards"].as_array_mut() {
                            cards.retain(|x| x["sandbox"] != sb.as_str() || x["origin"].is_null());
                            let n = cards.len();
                            col["count"] = n.into();
                        }
                    }
                }
            }
            "refresh" => {}
            _ => return Some((404, json!({ "ok": false, "error": "not found" }))),
        }
        (st.publish)("tickets", json!({ "sandbox": sb, "what": if what == "refresh" { "synced" } else { "source" } }));
        return ok(json!({ "ok": true }));
    }
    if let Some(c) = crate::feat::match_path("/api/tickets/{sandbox}/source", path) {
        let sb = c[0].1.clone();
        if st.data["tickets_sources"][sb.as_str()].is_null() {
            return Some((404, json!({ "ok": false, "error": "not found" })));
        }
        let src = &mut st.data["tickets_sources"][sb.as_str()];
        for k in ["tracker", "repo", "label", "auto_close"] {
            src[k] = body[k].clone();
        }
        src["configured"] = true.into();
        src["status"] = json!({ "state": "syncing", "last_ok_ms": null, "error": null, "hint": null });
        (st.publish)("tickets", json!({ "sandbox": sb, "what": "source" }));
        return ok(json!({ "ok": true }));
    }
    if let Some(c) = crate::feat::match_path("/api/tickets/{sandbox}", path) {
        let sb = c[0].1.clone();
        let n = 1 + st.data["tickets_views"].as_object().map_or(0, |m| m.keys().filter(|k| k.starts_with(&format!("{sb}/"))).count()) as u64 + 100;
        let title = body["title"].as_str().unwrap_or("").to_string();
        let card = json!({ "key": format!("t:{sb}:{n}"), "sandbox": sb, "n": n, "num": format!("T-{n}"), "column": "planned", "title": title, "title_short": title, "title_long": title, "source": "person", "priority": body["priority"], "thread": null, "thread_state": null, "thread_tone": "neutral", "plan": null, "marks": [], "files": 0, "bytes": 0, "validated": false, "created_ms": 0, "updated_ms": 0, "age": "now", "can": { "reorder": true, "validate": false, "edit": true, "delete": true, "send": true, "reorder_why": "", "delete_why": "", "send_why": "" } });
        for k in [sb.clone(), "all".into()] {
            if let Some(col) = column_mut(&mut st.data["tickets_boards"][k.as_str()], "planned") {
                if let Some(cards) = col["cards"].as_array_mut() {
                    cards.insert(0, card.clone());
                }
                col["count"] = (col["count"].as_u64().unwrap_or(0) + 1).into();
            }
        }
        let view = json!({ "card": card, "n": n, "num": format!("T-{n}"), "title": title, "description": body["description"], "priority": body["priority"], "files": [], "files_label": "", "history": [{ "at": 0, "when": "now", "text": "Created by you", "tone": "neutral" }] });
        st.data["tickets_views"][format!("{sb}/{n}")] = view.clone();
        (st.publish)("tickets", json!({ "sandbox": sb, "n": n, "what": "created" }));
        return ok(json!({ "ok": true, "ticket": view }));
    }
    let c = crate::feat::match_path("/api/tickets/{sandbox}/{n}/{what}", path).or_else(|| crate::feat::match_path("/api/tickets/{sandbox}/{n}/files/{file}/remove", path))?;
    let (sb, n) = (c[0].1.clone(), c[1].1.clone());
    let k = format!("{sb}/{n}");
    let what = c.get(2).map(|x| x.1.clone()).unwrap_or_default();
    let r = match what.as_str() {
        "delete" => {
            for b in st.data["tickets_boards"].as_object_mut().into_iter().flat_map(|m| m.values_mut()) {
                for col in b["columns"].as_array_mut().into_iter().flatten() {
                    if let Some(cards) = col["cards"].as_array_mut() {
                        let before = cards.len();
                        cards.retain(|x| x["key"] != format!("t:{sb}:{n}"));
                        if cards.len() < before {
                            col["count"] = (col["count"].as_u64().unwrap_or(1) - 1).into();
                        }
                    }
                }
            }
            let files = st.data["tickets_views"][k.as_str()]["files"].as_array().map_or(0, |f| f.len());
            json!({ "ok": true, "deleted": 1, "files": files, "bytes": 0 })
        }
        "send" => json!({ "ok": true, "sent": "typed", "line": send_line(n.parse().unwrap_or(0)) }),
        "plan" => {
            let key = format!("t:{sb}:{n}");
            for b in st.data["tickets_boards"].as_object_mut().into_iter().flat_map(|m| m.values_mut()) {
                let Some(back) = column_mut(b, "backlog") else { continue };
                let Some(cards) = back["cards"].as_array_mut() else { continue };
                let Some(i) = cards.iter().position(|x| x["key"] == key.as_str()) else { continue };
                let mut card = cards.remove(i);
                back["count"] = (back["count"].as_u64().unwrap_or(1).saturating_sub(1)).into();
                card["column"] = "planned".into();
                card["can_plan"] = false.into();
                if let Some(p) = column_mut(b, "planned") {
                    if let Some(cs) = p["cards"].as_array_mut() {
                        cs.push(card);
                    }
                    p["count"] = (p["count"].as_u64().unwrap_or(0) + 1).into();
                }
            }
            if let Some(c) = st.data["tickets_views"][k.as_str()]["card"].as_object_mut() {
                c.insert("column".into(), "planned".into());
                c.insert("can_plan".into(), false.into());
            }
            json!({ "ok": true, "ticket": st.data["tickets_views"][k.as_str()] })
        }
        "files" => json!({ "ok": true, "ticket": st.data["tickets_views"][k.as_str()] }),
        _ => json!({ "ok": true, "ticket": st.data["tickets_views"][k.as_str()] }),
    };
    (st.publish)("tickets", json!({ "sandbox": sb, "what": what }));
    ok(r)
}

/// A board's column by its id.
fn column_mut<'a>(board: &'a mut J, id: &str) -> Option<&'a mut J> {
    board["columns"].as_array_mut()?.iter_mut().find(|c| c["id"] == id)
}

fn empty_board(scope: &str) -> J {
    let cols: Vec<J> = model::LOCAL_COLUMNS
        .iter()
        .map(|id| {
            let (t, tone) = model::column_label(id);
            json!({ "id": id, "title": t, "tone": tone, "help": "", "empty": if *id == "planned" { "No ticket planned: add one." } else { "Nothing here." }, "count": 0, "cards": [], "older": 0 })
        })
        .collect();
    json!({ "scope": scope, "columns": cols, "counts_line": "0 planned · 0 in progress · 0 in test · 0 ready to merge · 0 merged", "tickets": 0, "bytes": 0 })
}
