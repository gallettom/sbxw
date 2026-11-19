//! Intents: the only way a screen asks the core to do something. A screen's
//! action returns them as data (`intent("stop_sandbox", { sandbox = "x" })`);
//! the core checks each one against this closed list, with exact argument
//! names and types, and only then acts. A screen never reaches the daemon.
//!
//! An unknown intent, an unknown argument, a missing or mistyped one: the
//! whole batch the action returned is refused, nothing runs, and the error is
//! shown on the screen.
//!
//! The list is one table, [`REGISTRY`]: each intent's name, arguments with
//! their types, and its [`Risk`]. Adding an intent (waves 2 to 4) is a row
//! there, a variant in [`Intent`] built in [`parse_one`], the code that runs
//! it in `shell.rs` (`App::run_intent`), and its entry in `docs/screens.md`.
//! Screens written against the existing rows do not change.
//!
//! **Approvals.** An intent whose risk is [`Risk::Approval`] is not run when
//! a screen asks: the core queues it (`App::approvals`) and draws the dialog
//! itself (`approval.rs`), above everything, out of reach of Luau; only the
//! person's answer there runs it. `approve_proposal` is the one with that risk.

use serde_json::{Map, Value};

/// Where an opened terminal goes.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneTarget {
    /// The focused pane (the web UI's row click).
    Focused,
    /// A pane showing nothing, or a new pane if all are used and the layout has room.
    Spare,
    /// That pane, counted from 1.
    Index(usize),
}

/// A thread's first open (`open_terminal` with `thread`, agent mode): the
/// web UI's `connectPane` query (`assets/js/panes.js`). The daemon remembers
/// them, so a reconnection does not send them again.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadStart {
    /// The repository folder, relative to the workspace; `""` is the workspace itself.
    pub repo: Option<String>,
    /// A model id for the thread's agent.
    pub model: Option<String>,
    /// A session in the main checkout, no worktree.
    pub checkout: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Opens a terminal on a sandbox. `mode`: `agent` (default; starts a stopped
    /// sandbox), `bash`. `thread` for a thread's session.
    OpenTerminal { sandbox: String, mode: Mode, thread: Option<String>, pane: PaneTarget, start: ThreadStart },
    /// Starts a stopped sandbox: as the web UI does, by opening its agent terminal.
    StartSandbox { sandbox: String, pane: PaneTarget },
    StopSandbox { sandbox: String },
    /// Empties every pane showing that session: the sandbox's own (`thread`
    /// None) or that thread's (after it is closed, as `panes.js`
    /// `afterThreadClosed` does). A pane other than the last shown one closes.
    ClosePanes { sandbox: String, thread: Option<String> },
    /// Puts text on the clipboard (a notice says how much); up to 100 KB.
    CopyText { text: String },
    /// Marks a sandbox as the selected one (the sidebar's highlight).
    Select { sandbox: Option<String> },
    /// Shows or hides the sandboxes sidebar (`None`: the other way); kept in
    /// the state file. The pane area takes its width.
    Sidebar { show: Option<bool> },
    /// The team leads view on or off (`None`: the other way): the terminals
    /// show only the team leaders' panes, Sessions only their sessions
    /// (shell/leads.rs). One state for both base views.
    TeamLeads { show: Option<bool> },
    /// The app's theme by name; kept in the state file, applied at once
    /// (every surface, open terminals included). `None`: the next theme.
    SetTheme { theme: Option<crate::theme::Mode> },
    /// Pane count (1 to 4) and how two panes split.
    SetLayout { panes: usize, split: Split },
    /// Creates Jarvis's sandbox, or starts it, and opens its session.
    StartJarvis,
    /// Sends what the person typed in this screen's input `input` to Jarvis.
    /// The core reads the text from its own copy of the input, never from
    /// the screen, so a screen cannot put words in the person's mouth.
    SendToJarvis { input: String },
    /// Opens or closes the Jarvis panel (`open` absent: toggles).
    ToggleJarvisPanel { open: Option<bool> },
    /// Push-to-talk from a button: start recording, then stop and send.
    TalkStart,
    TalkStop,
    /// Reads one of the conversation's messages aloud (`data.jarvis.log[i].id`).
    ReadAloud { id: u64 },
    StopReading,
    /// Shows a proposal's dialog. Risk Approval: the screen only opens the
    /// core's dialog; approving takes the person's click there.
    ApproveProposal { id: String },
    RefuseProposal { id: String, reason: Option<String> },
    /// Opens an http(s) link in the browser (a link in a `markdown` element).
    OpenLink { url: String },
    /// Shows a feature's modal screen (`feat::Slot::Modal`), with an argument it reads as `data.app.modal.arg`.
    OpenModal { screen: String, arg: Option<String> },
    CloseModal,
    /// Opens a feature's docked panel (`feat::Slot::Panel`); `close_panel` without `screen` closes the shown one.
    OpenPanel { screen: String },
    ClosePanel { screen: Option<String> },
    /// One of a feature's own intents (`src/feat/`), checked against its row.
    Feature(crate::feat::Asked),
    /// Pages of terminals and where panes sit (shell/pages.rs, tiling.rs).
    /// Views only: nothing starts, stops or is created; a terminal keeps its
    /// socket wherever its pane goes.
    Pages(PageOp),
}

/// What a page or pane intent rearranges. Pages and panes are counted from
/// 0 here (the intents' arguments from 1); `pane: None` is the focused pane.
#[derive(Debug, Clone, PartialEq)]
pub enum PageOp {
    Show(usize),
    /// The next page (`1`) or the previous one (`-1`), round.
    Step(i32),
    New,
    Close(Option<usize>),
    Per(usize),
    /// `true`: tabs (one per page); `false`: grid (the last count above one).
    View(bool),
    Gather,
    Move { pane: Option<usize>, to: String },
    Rotate(Option<usize>),
    ToPage { pane: Option<usize>, page: Option<usize> },
    Equalise,
}

impl PageOp {
    pub fn name(&self) -> &'static str {
        match self {
            PageOp::Show(_) => "page_show",
            PageOp::Step(_) => "page_step",
            PageOp::New => "page_new",
            PageOp::Close(_) => "page_close",
            PageOp::Per(_) => "pages_per",
            PageOp::View(_) => "pages_view",
            PageOp::Gather => "pages_gather",
            PageOp::Move { .. } => "pane_move",
            PageOp::Rotate(_) => "pane_rotate",
            PageOp::ToPage { .. } => "pane_to_page",
            PageOp::Equalise => "panes_equalise",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Agent,
    Bash,
    /// The web UI's "kit waiting" pane, where sbx's kit prompt is answered (`/ws?mode=kit-add`).
    KitAdd,
}

impl Mode {
    /// The `/ws` query's `mode`.
    pub fn query(self) -> Option<String> {
        match self {
            Mode::Agent => None,
            Mode::Bash => Some("bash".into()),
            Mode::KitAdd => Some("kit-add".into()),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Agent => "agent",
            Mode::Bash => "bash",
            Mode::KitAdd => "kit-add",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Split {
    /// Side by side.
    Columns,
    /// One above the other.
    Rows,
}

/// How much an intent can do, declared once per intent in [`REGISTRY`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Risk {
    /// What the web UI does on one click: runs when the screen asks.
    Direct,
    /// Held for a dialog drawn by the core; the person says yes or no.
    /// The text says what is asked (wave 4: Jarvis proposals, network rules,
    /// host commands).
    Approval(&'static str),
}

/// What an intent argument holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg {
    /// A non-empty string.
    Str,
    /// A string, possibly empty (a thread's `repo`: `""` is the workspace).
    Text,
    /// One of these strings.
    OneOf(&'static [&'static str]),
    /// A whole number in this range.
    Int(i64, i64),
    /// `"focused"`, `"spare"` or a pane number 1 to 4.
    Pane,
    Bool,
    /// An `http://` or `https://` URL.
    Url,
}

/// One entry of the closed list.
pub struct Spec {
    pub name: &'static str,
    pub risk: Risk,
    /// `(name, type, required)`.
    pub args: &'static [(&'static str, Arg, bool)],
    pub doc: &'static str,
}

const MODES: &[&str] = &["agent", "bash", "kit-add"];

/// The closed list. Widening it later is a row here plus the variant and its
/// code; screens that use the existing rows do not change.
pub const REGISTRY: &[Spec] = &[
    Spec {
        name: "open_terminal",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("mode", Arg::OneOf(MODES), false), ("thread", Arg::Str, false), ("pane", Arg::Pane, false), ("repo", Arg::Text, false), ("model", Arg::Str, false), ("checkout", Arg::Bool, false)],
        doc: "Opens a terminal on a sandbox, in the focused pane by default. The agent mode starts a stopped sandbox. With `thread`: that thread's agent (bash: its shell); `repo` (\"\" is the workspace), `model` and `checkout` (no worktree) are for the thread's first open, agent mode only. `pane: \"spare\"` takes an empty pane, else adds one (up to 4), and only then replaces the least recently used.",
    },
    Spec {
        name: "start_sandbox",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("pane", Arg::Pane, false)],
        doc: "Starts a stopped sandbox by opening its agent terminal, as the web UI's Connect does.",
    },
    Spec { name: "stop_sandbox", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Stops a sandbox (`POST /api/sandboxes/:name/stop`)." },
    Spec {
        name: "close_panes",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("thread", Arg::Str, false)],
        doc: "Empties the panes showing that sandbox's session (`thread`: that thread's; none: the sandbox's own), its terminals closed.",
    },
    Spec { name: "copy_text", risk: Risk::Direct, args: &[("text", Arg::Str, true)], doc: "Writes the text (up to 100 KB) to the person's clipboard, replacing what it held; only from the person's own press (never from typing, data or a feature), and a notice shows the size and the first characters, controls made visible. Screens can never read the clipboard." },
    Spec { name: "sidebar", risk: Risk::Direct, args: &[("show", Arg::OneOf(&["show", "hide", "toggle"]), true)], doc: "Shows or hides the sandboxes sidebar (`toggle`: the other way); remembered across launches. The pane area, terminals or Sessions, takes its width. Cmd/Ctrl+B does the same." },
    Spec { name: "team_leads", risk: Risk::Direct, args: &[("show", Arg::OneOf(&["show", "hide", "toggle"]), true)], doc: "The team leads view on or off (`toggle`: the other way), as the rail's Team leads: the terminals show only the team leaders' panes (the layout before comes back when it goes off), Sessions only the leaders' sessions and their threads'. One state for both views. Nothing happens while no team is running." },
    Spec { name: "set_theme", risk: Risk::Direct, args: &[("theme", Arg::OneOf(&["dark", "graphite", "toggle"]), true)], doc: "The app's theme: `dark` (the blue-black, the default), `graphite` (neutral greys, an orange accent), or `toggle` (the next one). Remembered across launches; every surface switches at once, open terminals included." },
    Spec { name: "select", risk: Risk::Direct, args: &[("sandbox", Arg::Str, false)], doc: "Marks a sandbox as selected (no `sandbox`: none)." },
    Spec {
        name: "set_layout",
        risk: Risk::Direct,
        args: &[("panes", Arg::Int(1, 4), false), ("split", Arg::OneOf(&["columns", "rows"]), false)],
        doc: "Sets how many terminal panes show (1 to 4) and whether two split side by side or stacked.",
    },
    Spec { name: "page_show", risk: Risk::Direct, args: &[("page", Arg::Int(1, 99), true)], doc: "Shows that page of terminals (from 1)." },
    Spec { name: "page_step", risk: Risk::Direct, args: &[("back", Arg::Bool, false)], doc: "Shows the next page of terminals (`back`: the previous one), round." },
    Spec { name: "page_new", risk: Risk::Direct, args: &[], doc: "Adds an empty page of terminals at the end and shows it." },
    Spec { name: "page_close", risk: Risk::Direct, args: &[("page", Arg::Int(1, 99), false)], doc: "Closes a page (none: the current one): its panes' views close, the sessions go on. The only page is emptied instead." },
    Spec { name: "pages_per", risk: Risk::Direct, args: &[("per", Arg::Int(1, 6), true)], doc: "Sets how many terminals a page holds (1 is the tabbed view). Lowering it moves the overflowing terminals onto the following pages, in order; raising it moves nothing." },
    Spec { name: "pages_view", risk: Risk::Direct, args: &[("view", Arg::OneOf(&["tabs", "grid"]), true)], doc: "Tabs: one terminal per page. Grid: the last count per page above one." },
    Spec { name: "pages_gather", risk: Risk::Direct, args: &[], doc: "Puts the open terminals back on as few pages as the count per page allows, in order; empty panes go." },
    Spec {
        name: "pane_move",
        risk: Risk::Direct,
        args: &[("pane", Arg::Int(1, 99), false), ("to", Arg::OneOf(&["left", "right", "up", "down"]), true)],
        doc: "Moves a pane (none: the focused one) on its page: it trades places with the pane on that side, or, at the edge, docks along that whole side.",
    },
    Spec { name: "pane_rotate", risk: Risk::Direct, args: &[("pane", Arg::Int(1, 99), false)], doc: "The pane (none: the focused one) and its sibling switch between side by side and stacked: portrait to landscape and back." },
    Spec { name: "pane_to_page", risk: Risk::Direct, args: &[("pane", Arg::Int(1, 99), false), ("page", Arg::Int(1, 99), false)], doc: "Moves a pane (none: the focused one) to that page (none: a new page), which then shows; refused when that page is full." },
    Spec { name: "panes_equalise", risk: Risk::Direct, args: &[], doc: "Gives every pane of the current page an equal share along its splits." },
    Spec { name: "start_jarvis", risk: Risk::Direct, args: &[], doc: "Creates Jarvis's sandbox (the first time) or starts it, and opens its session (`POST /api/jarvis/start`)." },
    Spec {
        name: "send_to_jarvis",
        risk: Risk::Direct,
        args: &[("input", Arg::Str, true)],
        doc: "Sends what the person typed in this screen's input `input` to Jarvis, masked first. The core reads the text itself; the input is emptied once sent and kept when sending fails. A typed yes or no answers a waiting proposal (a yes only a low-risk one).",
    },
    Spec { name: "toggle_jarvis_panel", risk: Risk::Direct, args: &[("open", Arg::Bool, false)], doc: "Opens or closes the Jarvis panel docked on the right (no `open`: toggles)." },
    Spec { name: "talk_start", risk: Risk::Direct, args: &[], doc: "Starts recording for Jarvis (push-to-talk from a button)." },
    Spec { name: "talk_stop", risk: Risk::Direct, args: &[], doc: "Stops recording; the words are transcribed and sent to Jarvis." },
    Spec { name: "read_aloud", risk: Risk::Direct, args: &[("id", Arg::Int(1, i64::MAX), true)], doc: "Reads one message of the conversation aloud (`data.jarvis.log[i].id`)." },
    Spec { name: "stop_reading", risk: Risk::Direct, args: &[], doc: "Stops reading aloud." },
    Spec {
        name: "approve_proposal",
        risk: Risk::Approval("Approve a Jarvis proposal"),
        args: &[("id", Arg::Str, true)],
        doc: "Opens the core's dialog for that proposal. Never approves by itself: only the person's click on the dialog does.",
    },
    Spec { name: "refuse_proposal", risk: Risk::Direct, args: &[("id", Arg::Str, true), ("reason", Arg::Str, false)], doc: "Refuses a Jarvis proposal, with an optional reason Jarvis is told." },
    Spec { name: "open_link", risk: Risk::Direct, args: &[("url", Arg::Url, true)], doc: "Opens an `http://` or `https://` link in the browser." },
    Spec {
        name: "open_modal",
        risk: Risk::Direct,
        args: &[("screen", Arg::Str, true), ("arg", Arg::Str, false)],
        doc: "Shows a feature's dialog screen over the window (one at a time; Escape closes it). The screen reads `data.app.modal.arg`.",
    },
    Spec { name: "close_modal", risk: Risk::Direct, args: &[], doc: "Closes the dialog screen showing." },
    Spec { name: "open_panel", risk: Risk::Direct, args: &[("screen", Arg::Str, true)], doc: "Opens a feature's panel docked on the right (several open show as tabs)." },
    Spec { name: "close_panel", risk: Risk::Direct, args: &[("screen", Arg::Str, false)], doc: "Closes that panel (no `screen`: the one shown)." },
];

/// Every row: the core's [`REGISTRY`], then each feature's (`src/feat/`).
pub fn all() -> impl Iterator<Item = &'static Spec> {
    REGISTRY.iter().chain(crate::feat::FEATURES.iter().flat_map(|f| f.intents.iter()))
}

impl Intent {
    pub fn name(&self) -> &'static str {
        match self {
            Intent::OpenTerminal { .. } => "open_terminal",
            Intent::StartSandbox { .. } => "start_sandbox",
            Intent::StopSandbox { .. } => "stop_sandbox",
            Intent::Select { .. } => "select",
            Intent::SetTheme { .. } => "set_theme",
            Intent::Sidebar { .. } => "sidebar",
            Intent::TeamLeads { .. } => "team_leads",
            Intent::SetLayout { .. } => "set_layout",
            Intent::ClosePanes { .. } => "close_panes",
            Intent::CopyText { .. } => "copy_text",
            Intent::StartJarvis => "start_jarvis",
            Intent::SendToJarvis { .. } => "send_to_jarvis",
            Intent::ToggleJarvisPanel { .. } => "toggle_jarvis_panel",
            Intent::TalkStart => "talk_start",
            Intent::TalkStop => "talk_stop",
            Intent::ReadAloud { .. } => "read_aloud",
            Intent::StopReading => "stop_reading",
            Intent::ApproveProposal { .. } => "approve_proposal",
            Intent::RefuseProposal { .. } => "refuse_proposal",
            Intent::OpenLink { .. } => "open_link",
            Intent::OpenModal { .. } => "open_modal",
            Intent::CloseModal => "close_modal",
            Intent::OpenPanel { .. } => "open_panel",
            Intent::ClosePanel { .. } => "close_panel",
            Intent::Feature(a) => a.name,
            Intent::Pages(op) => op.name(),
        }
    }

    pub fn spec(&self) -> &'static Spec {
        spec(self.name()).expect("every intent is registered")
    }

    pub fn risk(&self) -> Risk {
        self.spec().risk
    }
}

pub fn spec(name: &str) -> Option<&'static Spec> {
    all().find(|s| s.name == name)
}

/// What an action returned: `nil`, one intent, or a list of them. All or nothing.
pub fn parse(v: &Value) -> Result<Vec<Intent>, String> {
    let is_intent = |o: &Map<String, Value>| o.get("kind").and_then(Value::as_str) == Some("intent");
    match v {
        Value::Null => Ok(vec![]),
        Value::Object(o) if is_intent(o) => Ok(vec![parse_one(o)?]),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, x)| match x {
                Value::Object(o) if is_intent(o) => parse_one(o),
                _ => Err(format!("item {} of the returned list is not an intent (make it with intent(name, args))", i + 1)),
            })
            .collect(),
        // `{}` from Luau is an empty table: no intents.
        Value::Object(o) if o.is_empty() => Ok(vec![]),
        _ => Err("an action must return nil, an intent, or a list of intents (make them with intent(name, args))".into()),
    }
}

/// Checks the arguments against the registry, then builds the variant.
fn parse_one(o: &Map<String, Value>) -> Result<Intent, String> {
    let name = o.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return Err("an intent without a name".into());
    }
    let spec = spec(name).ok_or_else(|| format!("unknown intent '{name}' (known: {})", all().map(|s| s.name).collect::<Vec<_>>().join(", ")))?;
    let empty = Map::new();
    let m = match o.get("args") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        // An empty Luau table converts to `[]`.
        Some(Value::Array(a)) if a.is_empty() => &empty,
        Some(_) => return Err(format!("{name}: arguments must be a table of named values")),
    };
    for k in m.keys() {
        if !spec.args.iter().any(|(a, _, _)| a == k) {
            let takes: Vec<_> = spec.args.iter().map(|(a, _, _)| *a).collect();
            return Err(format!("{name}: unknown argument '{k}' (takes: {})", takes.join(", ")));
        }
    }
    for (a, ty, required) in spec.args {
        match m.get(*a) {
            None | Some(Value::Null) if *required => return Err(format!("{name}: '{a}' is required")),
            None | Some(Value::Null) => {}
            Some(v) => check(name, a, *ty, v)?,
        }
    }
    let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
    let pane = || match m.get("pane") {
        Some(Value::String(p)) if p == "spare" => PaneTarget::Spare,
        Some(Value::Number(n)) => PaneTarget::Index(n.as_u64().unwrap_or(1) as usize),
        _ => PaneTarget::Focused,
    };
    Ok(match name {
        "open_terminal" => Intent::OpenTerminal {
            sandbox: s("sandbox").unwrap(),
            mode: match s("mode").as_deref() {
                Some("bash") => Mode::Bash,
                Some("kit-add") => Mode::KitAdd,
                _ => Mode::Agent,
            },
            thread: s("thread"),
            pane: pane(),
            start: ThreadStart { repo: s("repo"), model: s("model"), checkout: m.get("checkout").and_then(Value::as_bool).unwrap_or(false) },
        },
        "start_sandbox" => Intent::StartSandbox { sandbox: s("sandbox").unwrap(), pane: pane() },
        "stop_sandbox" => Intent::StopSandbox { sandbox: s("sandbox").unwrap() },
        "select" => Intent::Select { sandbox: s("sandbox") },
        "sidebar" => Intent::Sidebar { show: match s("show").as_deref() {
            Some("show") => Some(true),
            Some("hide") => Some(false),
            _ => None,
        } },
        "team_leads" => Intent::TeamLeads { show: match s("show").as_deref() {
            Some("show") => Some(true),
            Some("hide") => Some(false),
            _ => None,
        } },
        "set_theme" => Intent::SetTheme { theme: s("theme").as_deref().and_then(crate::theme::Mode::parse) },
        "set_layout" => Intent::SetLayout {
            panes: m.get("panes").and_then(Value::as_u64).unwrap_or(1) as usize,
            split: if s("split").as_deref() == Some("rows") { Split::Rows } else { Split::Columns },
        },
        "copy_text" => Intent::CopyText { text: s("text").unwrap() },
        "close_panes" => Intent::ClosePanes { sandbox: s("sandbox").unwrap(), thread: s("thread") },
        "start_jarvis" => Intent::StartJarvis,
        "send_to_jarvis" => Intent::SendToJarvis { input: s("input").unwrap() },
        "toggle_jarvis_panel" => Intent::ToggleJarvisPanel { open: m.get("open").and_then(Value::as_bool) },
        "talk_start" => Intent::TalkStart,
        "talk_stop" => Intent::TalkStop,
        "read_aloud" => Intent::ReadAloud { id: m.get("id").and_then(Value::as_f64).unwrap_or(0.0) as u64 },
        "stop_reading" => Intent::StopReading,
        "approve_proposal" => Intent::ApproveProposal { id: s("id").unwrap() },
        "refuse_proposal" => Intent::RefuseProposal { id: s("id").unwrap(), reason: s("reason") },
        "open_link" => Intent::OpenLink { url: s("url").unwrap() },
        "open_modal" => Intent::OpenModal { screen: s("screen").unwrap(), arg: s("arg") },
        "close_modal" => Intent::CloseModal,
        "open_panel" => Intent::OpenPanel { screen: s("screen").unwrap() },
        "close_panel" => Intent::ClosePanel { screen: s("screen") },
        "page_show" | "page_step" | "page_new" | "page_close" | "pages_per" | "pages_view" | "pages_gather" | "pane_move" | "pane_rotate" | "pane_to_page" | "panes_equalise" => {
            let n = |k: &str| m.get(k).and_then(Value::as_u64).map(|v| v as usize - 1);
            Intent::Pages(match name {
                "page_show" => PageOp::Show(n("page").unwrap()),
                "page_step" => PageOp::Step(if m.get("back").and_then(Value::as_bool) == Some(true) { -1 } else { 1 }),
                "page_new" => PageOp::New,
                "page_close" => PageOp::Close(n("page")),
                "pages_per" => PageOp::Per(n("per").unwrap() + 1),
                "pages_view" => PageOp::View(s("view").as_deref() == Some("tabs")),
                "pages_gather" => PageOp::Gather,
                "pane_move" => PageOp::Move { pane: n("pane"), to: s("to").unwrap() },
                "pane_rotate" => PageOp::Rotate(n("pane")),
                "pane_to_page" => PageOp::ToPage { pane: n("pane"), page: n("page") },
                _ => PageOp::Equalise,
            })
        }
        _ if crate::feat::intent(name).is_some() => Intent::Feature(crate::feat::Asked { name: spec.name, args: m.clone() }),
        _ => unreachable!("{name} is registered but not built"),
    })
}

fn check(name: &str, a: &str, ty: Arg, v: &Value) -> Result<(), String> {
    let ok = match (ty, v) {
        (Arg::Str, Value::String(s)) => !s.is_empty(),
        (Arg::Text, Value::String(s)) => s.len() <= 4096 && !s.chars().any(char::is_control),
        (Arg::OneOf(set), Value::String(s)) => set.contains(&s.as_str()),
        (Arg::Int(lo, hi), Value::Number(n)) => n.as_f64().is_some_and(|f| f.fract() == 0.0 && f >= lo as f64 && f <= hi as f64),
        (Arg::Pane, Value::String(s)) => s == "focused" || s == "spare",
        (Arg::Pane, Value::Number(n)) => n.as_f64().is_some_and(|f| f.fract() == 0.0 && (1.0..=4.0).contains(&f)),
        (Arg::Bool, Value::Bool(_)) => true,
        (Arg::Url, Value::String(u)) => crate::markdown::safe_url(u),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(match ty {
        Arg::Str => format!("{name}: '{a}' must be a non-empty string"),
        Arg::Text => format!("{name}: '{a}' must be a string without control characters"),
        Arg::OneOf(set) => format!("{name}: '{a}' must be one of {}", set.join(", ")),
        Arg::Int(lo, hi) => format!("{name}: '{a}' must be a whole number from {lo} to {hi}"),
        Arg::Pane => format!("{name}: '{a}' must be \"focused\", \"spare\" or a pane number 1 to 4"),
        Arg::Bool => format!("{name}: '{a}' must be true or false"),
        Arg::Url => format!("{name}: '{a}' must be an http:// or https:// URL"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn i(name: &str, args: Value) -> Value {
        json!({ "kind": "intent", "name": name, "args": args })
    }

    #[test]
    fn known_intents_parse() {
        assert_eq!(parse(&i("stop_sandbox", json!({ "sandbox": "a" }))).unwrap(), vec![Intent::StopSandbox { sandbox: "a".into() }]);
        assert_eq!(
            parse(&json!([i("select", json!({ "sandbox": "a" })), i("open_terminal", json!({ "sandbox": "a", "mode": "bash", "pane": 2 }))])).unwrap(),
            vec![
                Intent::Select { sandbox: Some("a".into()) },
                Intent::OpenTerminal { sandbox: "a".into(), mode: Mode::Bash, thread: None, pane: PaneTarget::Index(2), start: ThreadStart::default() }
            ]
        );
        assert_eq!(parse(&i("set_layout", json!({ "panes": 2, "split": "rows" }))).unwrap(), vec![Intent::SetLayout { panes: 2, split: Split::Rows }]);
        for spec in REGISTRY {
            let approval = matches!(spec.risk, Risk::Approval(_));
            assert_eq!(approval, spec.name == "approve_proposal", "{}: only approving a proposal needs the core's dialog", spec.name);
        }
        assert_eq!(parse(&i("send_to_jarvis", json!({ "input": "jarvis-input" }))).unwrap(), vec![Intent::SendToJarvis { input: "jarvis-input".into() }]);
        assert_eq!(parse(&i("toggle_jarvis_panel", json!({}))).unwrap(), vec![Intent::ToggleJarvisPanel { open: None }]);
        assert_eq!(parse(&i("read_aloud", json!({ "id": 3 }))).unwrap(), vec![Intent::ReadAloud { id: 3 }]);
        assert_eq!(parse(&i("approve_proposal", json!({ "id": "p1" }))).unwrap()[0].risk(), Risk::Approval("Approve a Jarvis proposal"));
        assert_eq!(parse(&i("select", json!([]))).unwrap(), vec![Intent::Select { sandbox: None }]);
        assert_eq!(parse(&Value::Null).unwrap(), vec![]);
        assert_eq!(parse(&i("page_show", json!({ "page": 2 }))).unwrap(), vec![Intent::Pages(PageOp::Show(1))]);
        assert_eq!(parse(&i("pages_per", json!({ "per": 1 }))).unwrap(), vec![Intent::Pages(PageOp::Per(1))]);
        assert_eq!(parse(&i("pane_to_page", json!({}))).unwrap(), vec![Intent::Pages(PageOp::ToPage { pane: None, page: None })]);
        assert_eq!(parse(&i("pane_move", json!({ "pane": 3, "to": "up" }))).unwrap(), vec![Intent::Pages(PageOp::Move { pane: Some(2), to: "up".into() })]);
        assert_eq!(parse(&i("page_step", json!({ "back": true }))).unwrap()[0].risk(), Risk::Direct);
    }

    #[test]
    fn anything_else_is_refused_whole() {
        for (v, want) in [
            (i("rm_sandbox", json!({ "sandbox": "a" })), "unknown intent 'rm_sandbox'"),
            (i("stop_sandbox", json!({})), "'sandbox' is required"),
            (i("stop_sandbox", json!({ "sandbox": 3 })), "must be a non-empty string"),
            (i("stop_sandbox", json!({ "sandbox": "a", "force": true })), "unknown argument 'force'"),
            (i("set_layout", json!({ "panes": 9 })), "from 1 to 4"),
            (i("pages_per", json!({ "per": 7 })), "from 1 to 6"),
            (i("pane_move", json!({ "to": "away" })), "must be one of left, right, up, down"),
            (i("open_terminal", json!({ "sandbox": "a", "mode": "root" })), "must be one of agent, bash, kit-add"),
            (json!([i("select", json!({})), { "name": "stop_sandbox" }]), "item 2"),
            (json!("stop everything"), "must return nil"),
            (i("send_to_jarvis", json!({ "text": "yes" })), "unknown argument 'text'"),
            (i("open_link", json!({ "url": "javascript:alert(1)" })), "http:// or https://"),
            (i("open_link", json!({ "url": "file:///etc/passwd" })), "http:// or https://"),
            (i("toggle_jarvis_panel", json!({ "open": "yes" })), "true or false"),
        ] {
            let e = parse(&v).unwrap_err();
            assert!(e.contains(want), "{v}: {e}");
        }
    }

    #[test]
    fn every_name_is_documented() {
        let doc = include_str!("../docs/screens.md");
        for s in all().filter(|s| !REGISTRY.iter().any(|r| r.name == s.name)) {
            assert!(!s.doc.is_empty(), "feature intent {} has no doc (it goes to intents.json)", s.name);
        }
        for n in REGISTRY.iter().map(|s| s.name) {
            assert!(doc.contains(&format!("`{n}`")), "docs/screens.md does not describe the intent {n}");
        }
    }
}
