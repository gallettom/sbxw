//! Features: one area of the web UI ported to the app (network, create,
//! settings, threads, relay, files...), declared in ONE place.
//!
//! A feature is a module under `src/feat/` that exports one
//! `pub const FEATURE: Feature`, and one line in the [`features!`] list at
//! the bottom of this file. Nothing else in the core changes: the core reads
//! the declaration and does the rest.
//!
//! What a feature declares ([`Feature`]):
//!
//! * **data**: daemon routes the core reads for it ([`Data`]), under a key of
//!   the screens' `data` (and of its fixtures' `daemon` object), read again
//!   on the stream events it names, every N seconds, or when asked
//!   ([`Action::Refresh`], [`Action::Fetch`]).
//! * **events**: `/api/stream` events it wants ([`Feature::events`],
//!   handled by [`Feature::on_event`]).
//! * **screens**: its Luau files, compiled in, and the slot each shows in
//!   ([`Slot`]: a modal dialog or a docked right panel).
//! * **intents**: rows of the closed list ([`crate::intent::Spec`]) with their
//!   risk. A screen returns them like any intent; the core checks the
//!   arguments, then calls [`Feature::act`], which answers with
//!   [`Action`]s. An intent of risk `Approval` may only answer with ONE
//!   [`Action::Confirm`]: the core draws that dialog itself
//!   (`approval.rs`), out of reach of Luau, and runs its [`Call`] only on the
//!   person's click. Anything else is refused.
//! * **extras**: buttons and badges it adds to slots drawn by others
//!   ([`Extra`]: on a sandbox's row, in the top bar, on a pane's bar).
//! * **stand-in routes**: what `src/bin/standin.rs` answers for its routes
//!   ([`Feature::standin`]), so its screens run against the stand-in daemon.
//!
//! `docs/screens.md`, "Adding a feature", has a worked example.

use crate::intent::Spec;
use serde_json::{Map, Value as J};

/// One area of the app. All fields are plain data or plain functions, so a
/// feature is a `const`.
pub struct Feature {
    /// Short name (`network`, `create`, `setup`); the notice prefix and logs.
    pub name: &'static str,
    /// Daemon data the core reads for this feature.
    pub data: &'static [Data],
    /// `/api/stream` event names passed to [`Feature::on_event`].
    pub events: &'static [&'static str],
    /// Screens compiled in, and where each shows.
    pub screens: &'static [ScreenDecl],
    /// Rows of the closed list of intents (names unique across the app).
    pub intents: &'static [Spec],
    /// Runs one of this feature's intents, already checked. `Err`: a notice.
    pub act: fn(&Ctx, &Asked) -> Result<Vec<Action>, String>,
    /// A stream event this feature named in `events`: `(name, data as JSON)`.
    pub on_event: Option<fn(&Ctx, &str, &J) -> Vec<Action>>,
    /// One of this feature's [`Call`]s answered: the HTTP status and the JSON
    /// body, non-2xx bodies included (`Err`: no answer at all, a readable
    /// reason). When set, the core leaves the answer to it: no notice, no
    /// `store`; `then` keys are still read again.
    pub on_answer: Option<fn(&Ctx, &Call, Result<(u16, J), String>) -> Vec<Action>>,
    /// One of its data keys changed (a read brought something new): `(key,
    /// old value, new value)`; `old` is null on the first read. A new sbx
    /// request becomes a notice here.
    pub on_data: Option<fn(&Ctx, &str, &J, &J) -> Vec<Action>>,
    /// Buttons and badges in slots drawn by others, from the current data.
    pub extras: Option<fn(&Ctx) -> Vec<Extra>>,
    /// The stand-in daemon's answer to a request for this feature, if the
    /// route is its: `(status, JSON body)`.
    pub standin: Option<fn(&mut Standin, &str, &str, &J) -> Option<(u16, J)>>,
    /// Body fields only the core may set, and only after the person pressed
    /// Confirm on the core's dialog ([`Confirm::grant`]): a call whose body
    /// has one at the top level, sent any other way, is refused. (Create's
    /// `approved`: a screen or its arguments can never send it.)
    pub core_only: &'static [&'static str],
    /// Files or a picture the PERSON handed this feature: files dropped on
    /// its shown panel or dialog, a picture pasted (⌘V) while its panel is
    /// shown and no terminal has the keyboard, the answer to its
    /// [`Action::PickFiles`] or [`Action::ReadClipboard`]. These are the
    /// only ways a feature gets a path to read: never from a screen or a
    /// sandbox. Pictures go through [`crate::picture::prepare`].
    pub on_files: Option<fn(&Ctx, Incoming) -> Vec<Action>>,
    /// Global keyboard shortcuts: the window takes them before any pane.
    pub shortcuts: &'static [Shortcut],
    /// Picture surfaces it owns, by id (`crate::surface`): a screen places
    /// one with `surface{ id = "..." }`; its frames come from the feature's
    /// Rust ([`crate::surface::set_rgba`], [`crate::surface::set_encoded`]).
    pub surfaces: &'static [&'static str],
    /// The person's pointer and keys on one of its surfaces, in picture
    /// pixels, on the UI thread (keep it cheap: send and return). The app's
    /// reserved keys never come here ([`crate::surface::reserved`]).
    pub on_surface: Option<fn(&crate::surface::SurfaceEvent) -> Vec<Action>>,
    /// An event of one of its sockets ([`Action::OpenSocket`]): `(socket id,
    /// event)`, on that socket's own thread, never the UI's; the actions
    /// returned run on the UI thread at its next poll. Frames go straight
    /// to [`crate::surface`] from here.
    pub on_socket: Option<fn(&str, crate::socket::SocketEvent) -> Vec<Action>>,
    /// The stand-in daemon's side of a feature socket: called on the
    /// connection's own thread with the request target (`/ws/browser?sandbox=x`)
    /// once the upgrade is accepted; return `false` at once if the route is
    /// not this feature's. Push and read lines through `peer` until it closes.
    pub standin_ws: Option<fn(&str, &mut dyn WsPeer) -> bool>,
    /// One of its raw calls ([`Call::raw`]) answered: the status, the bytes
    /// (at most `max + 1`, so more than `max` means truncated) and the
    /// content type; `Err`: no answer at all.
    pub on_bytes: Option<fn(&Ctx, &Call, Result<(u16, Vec<u8>, String), String>) -> Vec<Action>>,
    /// Files the person dropped on a terminal pane (the first feature that
    /// declares this gets them). The paths are handed in: an upload may read them.
    pub on_pane_drop: Option<fn(&Ctx, &PaneDrop) -> Vec<Action>>,
    /// One of its core dialogs closed without its call being sent: Cancel,
    /// the close button, Escape, a press outside, a newer dialog replacing
    /// it, or a `Prepare` whose read failed and was closed. The argument is
    /// the dialog's call tag (the `Confirm`'s call, or the `Prepare`'s call
    /// while nothing was built yet), so the feature can clear what it kept
    /// for that dialog. Not called once the call went out (its answer comes
    /// to `on_answer`).
    pub on_dismiss: Option<fn(&Ctx, &str) -> Vec<Action>>,
}

/// Files dropped on a pane ([`Feature::on_pane_drop`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PaneDrop {
    pub paths: Vec<std::path::PathBuf>,
    /// The person chose "Do not ask again for files I drop" (the core's
    /// store): the feature may send a plain drop without its dialog. The
    /// core still allows that only for this very drop, and checks the
    /// guardrails again itself.
    pub skip_dialog: bool,
    /// The sandbox the pane shows, and its thread (nil: the main session).
    pub sandbox: String,
    pub thread: Option<String>,
}

/// Where a download goes ([`Action::Download`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dest {
    /// The person's Downloads folder.
    Downloads,
    /// The native save dialog, prefilled with the sanitised name.
    Ask,
}

/// What the core does once a download is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum After {
    Nothing,
    /// Shows it in the file manager.
    Reveal,
    /// Opens it with the system's default app, by its verdict (`safe.rs`).
    Open,
}

/// A download the core writes (see `src/files.rs`).
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    /// A daemon GET route answering the raw file.
    pub get: String,
    /// The suggested name (the core sanitises it).
    pub name: String,
    pub to: Dest,
    pub after: After,
    /// Handed back in `on_answer` (the call's `tag`).
    pub tag: String,
}

impl Download {
    /// As the call of a [`Confirm`] (needed for `Dest::Ask` and `After::Open`).
    pub fn call(&self, what: impl Into<String>) -> Call {
        let to = match self.to {
            Dest::Downloads => "downloads",
            Dest::Ask => "ask",
        };
        let after = match self.after {
            After::Nothing => "nothing",
            After::Reveal => "reveal",
            After::Open => "open",
        };
        Call { method: Method::Core, path: "core:download".into(), body: serde_json::json!({ "get": self.get, "name": self.name, "to": to, "after": after, "tag": self.tag }), what: what.into(), tag: self.tag.clone(), timeout: 900, ..Call::get("", "") }
    }
    /// Whether it may run without the core's dialog.
    pub fn direct(&self) -> bool {
        self.to == Dest::Downloads && self.after != After::Open
    }
}

/// The core's own calls on files (`src/files.rs`), as `Confirm` calls or direct where allowed.
pub fn core_call(op: &str, body: J, what: impl Into<String>, tag: &str) -> Call {
    Call { method: Method::Core, path: format!("core:{op}"), body, what: what.into(), tag: tag.into(), timeout: 900, ..Call::get("", "") }
}

/// The stand-in's end of a feature socket ([`Feature::standin_ws`]).
pub trait WsPeer {
    /// Sends a text message; false once closed.
    fn send(&mut self, text: &str) -> bool;
    fn send_binary(&mut self, bytes: &[u8]) -> bool;
    /// The next text message from the app, waiting at most `wait`:
    /// `Ok(None)` when nothing came, `Err` once closed.
    fn recv(&mut self, wait: std::time::Duration) -> Result<Option<String>, String>;
    /// Closes it (a lost browser process).
    fn close(&mut self);
}

/// A global shortcut: the web UI's rule (`assets/js/util.js`,
/// `isAppShortcut`): Alt+<key> off macOS; Ctrl+Option+<key> on macOS, where
/// Option alone types characters (⌥N is the `~` dead key). Shift and Cmd off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shortcut {
    /// The letter, by its place on the keyboard (`n` is the key labelled N on a US layout).
    pub key: char,
    /// The intent it runs (checked as any), with these arguments as JSON.
    pub intent: &'static str,
    pub args: &'static str,
    /// For Help: what it does.
    pub help: &'static str,
}

/// What the person handed a feature (see [`Feature::on_files`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// Paths dropped or chosen in the native dialog.
    Files(Vec<std::path::PathBuf>),
    /// A picture from the clipboard, as PNG bytes (Rune's).
    Png(Vec<u8>),
}

impl Feature {
    /// A feature with nothing declared, to fill with `..Feature::EMPTY`.
    pub const EMPTY: Feature = Feature { name: "", data: &[], events: &[], screens: &[], intents: &[], act: no_act, on_event: None, on_answer: None, on_data: None, extras: None, standin: None, core_only: &[], on_files: None, shortcuts: &[], surfaces: &[], on_surface: None, on_socket: None, standin_ws: None, on_bytes: None, on_pane_drop: None, on_dismiss: None };
}

fn no_act(_: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    Err(format!("{}: nothing runs it", a.name))
}

/// A daemon route read into `data.<key>`.
pub struct Data {
    /// The key in the screens' `data` and in a fixture's `daemon` object.
    pub key: &'static str,
    /// The route, `GET`. With `{sandbox}` in it: read once per running
    /// sandbox, and `data.<key>` is an object by sandbox name.
    pub path: &'static str,
    /// Stream events after which it is read again (300 ms later, bursts merged).
    pub on: &'static [&'static str],
    /// Also read again this often, in seconds, while connected (0: never).
    pub every: u64,
    /// Read only when asked ([`Action::Refresh`]), not at connection.
    pub lazy: bool,
}

impl Data {
    pub const fn new(key: &'static str, path: &'static str) -> Data {
        Data { key, path, on: &[], every: 0, lazy: false }
    }
    pub const fn per_sandbox(&self) -> bool {
        // `{sandbox}` anywhere in the path.
        let p = self.path.as_bytes();
        let mut i = 0;
        while i + 9 <= p.len() {
            if p[i] == b'{' && p[i + 1] == b's' && p[i + 8] == b'}' {
                return true;
            }
            i += 1;
        }
        false
    }
}

/// A screen of a feature.
pub struct ScreenDecl {
    /// Its name: `[a-z0-9-]`, unique across the app; the override file is `<name>.luau`.
    pub name: &'static str,
    /// The built-in source (`include_str!("../../screens/<name>.luau")`).
    pub src: &'static str,
    pub slot: Slot,
}

/// Where a feature's screen shows. Every slot is drawn by the core.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slot {
    /// A dialog over the window, centred, one at a time (`open_modal`,
    /// `close_modal`, Escape). The screen reads `data.app.modal.arg`.
    /// `light_dismiss`: a press outside closes it too. Only for a modal
    /// that holds nothing the person may have typed or changed (Help): a
    /// form keeps its cross, Cancel and Escape, so a stray click loses
    /// nothing. The safe default is false.
    Modal { width: f32, light_dismiss: bool },
    /// A panel docked on the right, beside Jarvis's (`open_panel`,
    /// `close_panel`). Several open show as tabs; one is shown at a time.
    Panel { title: &'static str, icon: &'static str, width: f32 },
    /// A full view drawn OVER the pane area (`open_panel` opens it,
    /// `close_panel`, Escape, its ×, or its rail entry pressed again close
    /// it), one at a time: opening one closes the other. The terminals keep
    /// running underneath and are not resized; the rail, the sidebar and the
    /// top and status bars stay; the pager is covered. No terminal gets a key
    /// or a press while it shows (`App::terminal_may_take_keys`).
    Overlay { title: &'static str, icon: &'static str },
    /// A base view of the pane area, equal to the terminals: the person
    /// switches between Terminals and it (the pager's view switch, its own
    /// header's, its rail entry, `open`'s shortcut), and the choice is kept
    /// across runs. Drawn where an overlay is; an overlay opened while it is
    /// the base goes OVER it, and closing that overlay comes back to it.
    /// Escape never leaves it; `close_panel` of it does (back to Terminals).
    /// No terminal gets a key, a paste, a drop or the focus while it is the
    /// base (`App::panes_covered`). `open`: the feature's intent that opens
    /// it (run by the switch and once at start when it was the last base).
    Base { title: &'static str, icon: &'static str, open: &'static str },
}

/// One of a feature's intents, checked against its [`Spec`].
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub name: &'static str,
    pub args: Map<String, J>,
}

impl Asked {
    /// A string argument (absent: `None`).
    pub fn s(&self, k: &str) -> Option<&str> {
        self.args.get(k).and_then(J::as_str)
    }
    /// A required string argument (the spec made sure it is there).
    pub fn req(&self, k: &str) -> &str {
        self.s(k).unwrap_or("")
    }
    pub fn b(&self, k: &str) -> Option<bool> {
        self.args.get(k).and_then(J::as_bool)
    }
    pub fn i(&self, k: &str) -> Option<i64> {
        self.args.get(k).and_then(J::as_f64).map(|f| f as i64)
    }
}

/// What a feature reads: the screens' data as they get it (its keys, the
/// sandboxes, `app`).
pub struct Ctx<'a> {
    pub data: &'a J,
}

impl Ctx<'_> {
    pub fn get(&self, key: &str) -> &J {
        &self.data[key]
    }
    /// The sandbox of that name, as `GET /api/sandboxes` lists it.
    pub fn sandbox(&self, name: &str) -> Option<&J> {
        self.data["sandboxes"].as_array()?.iter().find(|s| s["name"] == name)
    }
}

/// What a feature asks the core to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A daemon request, off the UI thread; a notice says how it went.
    Call(Call),
    /// The core's own dialog (`approval.rs`); its call runs on Confirm only.
    /// With [`Action::Prepare`], the only answer an intent of risk
    /// `Approval` may give.
    Confirm(Box<Confirm>),
    /// The dialog, after the core read something first: it shows the
    /// dialog loading, runs `call`, then `build` makes the [`Confirm`] from
    /// the answer (a host key's fingerprints read by the core itself, so a
    /// screen cannot forge what the dialog shows). A failure shows in the
    /// dialog, readable.
    Prepare(Box<Prepare>),
    /// Read one of the feature's declared [`Data`] again now.
    Refresh(&'static str),
    /// Read `path` (`GET`) into `data.<key>` now (data that depends on what
    /// the person picked: a folder listing, one sandbox's ports).
    Fetch { key: String, path: String },
    /// Put a value in `data.<key>` (nothing sent).
    Set { key: String, value: J },
    OpenModal { screen: String, arg: J },
    CloseModal,
    OpenPanel(String),
    ClosePanel(String),
    /// A line at the bottom right; `then` an intent offered on it (a click runs it, checked as any).
    Notice { text: String, error: bool, then: Option<(String, J)> },
    OpenLink(String),
    /// Runs an intent, checked exactly as one from a screen: an unknown name
    /// or argument is refused, and an intent of risk `Approval` still goes
    /// through the core's dialog.
    Intent { name: String, args: J },
    /// Keeps `value` across runs under this feature's `key` (the app's state
    /// file, `shell/store.rs`); null forgets it. Read back in
    /// `data.app.kept.<feature>.<key>`. Small values only (a host name, a tab).
    Remember { key: String, value: J },
    /// The native open dialog; the path chosen comes back to
    /// [`Feature::on_files`] as `Incoming::Files`. Rune's dialog picks one
    /// file (`multiple` is kept for when it can pick several); `extensions`
    /// filter it (empty: any file). A cancel says nothing.
    PickFiles { title: String, extensions: Vec<String>, multiple: bool },
    /// Reads the clipboard: a picture comes back to [`Feature::on_files`]
    /// as `Incoming::Png`; anything else is a notice.
    ReadClipboard,
    /// Shows a picture under `key` for screens' `image{ key = ... }`, decoded
    /// by the core ([`crate::picture`], its default limits, scaled to 2048 px
    /// at most). Empty `bytes`: forgets it. A picture refused is a notice.
    Image { key: String, bytes: Vec<u8> },
    /// Opens a WebSocket on the daemon (`path`: a `/ws/...` route and its
    /// query) under `id`, with the app's authentication ([`crate::socket`]);
    /// its events go to [`Feature::on_socket`]; send with
    /// [`crate::socket::send`]. An `id` already open is closed first.
    OpenSocket { id: String, path: String, opts: crate::socket::Opts },
    /// Closes socket `id` (its handler hears `Closed("closed")`).
    CloseSocket(String),
    /// Moves the keyboard focus to the element of that key (an input's
    /// `id`, a surface's `surface-<id>`).
    Focus(String),
    /// Scrolls the nearest viewport so the element of that key shows
    /// (Rune's `Tx::scroll_into_view`), once this change is drawn. Only a
    /// scroll: the keyboard focus stays where it is; a key not drawn does
    /// nothing. (The Board: a bubble folded from the end of its text.)
    ScrollIntoView(String),
    /// A download written by the core. Direct only to the Downloads folder
    /// and without opening ([`Download::direct`]); else refused: use
    /// [`Download::call`] as a [`Confirm`]'s call. The answer
    /// (`{saved, name, bytes, verdict, kind, reason}`) goes to `on_answer`.
    Download(Download),
    /// Shows a file the core wrote this run in the file manager.
    Reveal(std::path::PathBuf),
    /// The core forgets these handed-in paths: an upload can no longer read
    /// them (the feature cleared the queue they were in). A path is also
    /// forgotten once an upload of it is done.
    ForgetFiles(Vec<std::path::PathBuf>),
    /// Ask again before uploading files dropped on a pane: the core clears
    /// the person's "do not ask again" ([`Confirm::dont_ask`]). Only this
    /// direction exists as an action; setting it is the core dialog's box.
    UploadsAskAgain,
}

impl Action {
    pub fn notice(text: impl Into<String>) -> Action {
        Action::Notice { text: text.into(), error: false, then: None }
    }
    pub fn error(text: impl Into<String>) -> Action {
        Action::Notice { text: text.into(), error: true, then: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Method {
    Get,
    Post,
    /// A removal. Only for what undoes nothing the person made: Sessions'
    /// own history subscription (`.../history/subscription`). Any other
    /// DELETE from a feature goes through the core's dialog, as every
    /// destructive call does (docs/screens.md).
    Delete,
    /// Run by the core itself, never sent: `path` names one of the core's
    /// own operations (`core:screen-restore/<screen>`: deletes that screen's
    /// override file). Only behind the core's dialog.
    Core,
}

/// One daemon request.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: Method,
    pub path: String,
    pub body: J,
    /// What it does, for the notice: `allow api.github.com for web-app`.
    pub what: String,
    /// Data keys read again once it answered.
    pub then: Vec<String>,
    /// Put the answer in `data.<key>`.
    pub store: Option<String>,
    /// Say "done" in a notice (errors are always said).
    pub notify: bool,
    /// Seconds before giving up (default 120; a bring-up that pulls an image takes minutes).
    pub timeout: u64,
    /// A tag of the feature's own, handed back in `on_answer` (which call this was).
    pub tag: String,
    /// `GET` read as raw bytes, at most this many (+1): answered to
    /// [`Feature::on_bytes`], not `on_answer`.
    pub raw: Option<u64>,
    /// `POST` with this raw body instead of JSON: `(content type, bytes)`.
    pub bytes: Option<(String, Vec<u8>)>,
}

impl Call {
    pub fn post(path: impl Into<String>, body: J, what: impl Into<String>) -> Call {
        Call { method: Method::Post, path: path.into(), body, what: what.into(), then: vec![], store: None, notify: true, timeout: 120, tag: String::new(), raw: None, bytes: None }
    }
    pub fn get(path: impl Into<String>, what: impl Into<String>) -> Call {
        Call { method: Method::Get, path: path.into(), body: J::Null, what: what.into(), then: vec![], store: None, notify: false, timeout: 30, tag: String::new(), raw: None, bytes: None }
    }
    /// A `DELETE` (see [`Method::Delete`]): quiet, short.
    pub fn delete(path: impl Into<String>, what: impl Into<String>) -> Call {
        Call { method: Method::Delete, timeout: 10, ..Call::get(path, what) }
    }
    /// Read these keys again after it answered.
    pub fn then(mut self, keys: &[&str]) -> Call {
        self.then.extend(keys.iter().map(|k| k.to_string()));
        self
    }
    pub fn store(mut self, key: &str) -> Call {
        self.store = Some(key.into());
        self
    }
    /// Up to `secs` seconds (a bring-up: 1800).
    pub fn timeout(mut self, secs: u64) -> Call {
        self.timeout = secs;
        self
    }
    pub fn tag(mut self, tag: &str) -> Call {
        self.tag = tag.into();
        self
    }
    /// Read the answer as raw bytes, at most `max` (+1, to tell truncation):
    /// answered to [`Feature::on_bytes`].
    pub fn raw(mut self, max: u64) -> Call {
        self.raw = Some(max);
        self
    }
    /// A `POST` whose body is these bytes (a pasted picture to `paste-image`).
    pub fn post_bytes(path: impl Into<String>, content_type: &str, bytes: Vec<u8>, what: impl Into<String>) -> Call {
        let mut c = Call::post(path, J::Null, what);
        c.bytes = Some((content_type.into(), bytes));
        c
    }
    pub fn quiet(mut self) -> Call {
        self.notify = false;
        self
    }
}

/// The core's confirmation dialog for an intent of risk `Approval`. Its text
/// comes from the daemon's data, built by the feature's Rust, never from a
/// screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub title: String,
    /// Plain lines, shown in order (what is allowed, for which sandbox or
    /// for all, for how long).
    pub lines: Vec<String>,
    /// A block of text shown as is (a command, a file), or a diff.
    pub preview: Option<Preview>,
    /// A checkbox the person must tick before Confirm works.
    pub ack: Option<String>,
    /// An optional checkbox, unticked: "Do not ask again for files I drop".
    /// Ticked and confirmed, the core (never the feature) records it, and
    /// only for a batch of dropped files (`core:uploads` of `files`).
    pub dont_ask: Option<String>,
    /// Fields drawn by the core whose values never reach Luau, fixtures,
    /// logs or a headless tree: each is put in the call's body at its
    /// `field` on Confirm (handed once, then dropped).
    pub secrets: Vec<Secret>,
    pub confirm: String,
    pub cancel: String,
    /// Confirm in red (removing, widening access).
    pub danger: bool,
    /// Body fields the core sets on Confirm, and only then (see
    /// [`Feature::core_only`]). The feature cannot put them in the body itself.
    pub grants: Vec<Grant>,
    pub call: Call,
}

impl Confirm {
    pub fn new(title: impl Into<String>, call: Call) -> Confirm {
        Confirm { title: title.into(), lines: vec![], preview: None, ack: None, dont_ask: None, secrets: vec![], confirm: "Confirm".into(), cancel: "Cancel".into(), danger: false, grants: vec![], call }
    }
    pub fn line(mut self, l: impl Into<String>) -> Confirm {
        self.lines.push(l.into());
        self
    }
    pub fn labels(mut self, confirm: &str, cancel: &str) -> Confirm {
        self.confirm = confirm.into();
        self.cancel = cancel.into();
        self
    }
    pub fn danger(mut self) -> Confirm {
        self.danger = true;
        self
    }
    pub fn dont_ask(mut self, text: &str) -> Confirm {
        self.dont_ask = Some(text.into());
        self
    }
    pub fn ack(mut self, text: &str) -> Confirm {
        self.ack = Some(text.into());
        self
    }
    pub fn preview(mut self, text: impl Into<String>, diff: bool) -> Confirm {
        self.preview = Some(Preview { text: text.into(), diff });
        self
    }
    /// A secret field the person must fill; `field` is a body path, dots
    /// between keys (`value`, `secrets.API_KEY`), see [`put_at`].
    pub fn secret(mut self, label: &str, field: &str) -> Confirm {
        self.secrets.push(Secret { label: label.into(), placeholder: String::new(), field: field.into(), multiline: false, optional: false });
        self
    }
    /// A secret field that may stay empty: then it is left out of the body (an edit keeping the old value).
    pub fn optional_secret(mut self, label: &str, field: &str, placeholder: &str) -> Confirm {
        self.secrets.push(Secret { label: label.into(), placeholder: placeholder.into(), field: field.into(), multiline: false, optional: true });
        self
    }
    /// `field` (one of the feature's `core_only`) is set to `true` in the body on Confirm.
    pub fn grant(mut self, field: &str) -> Confirm {
        self.grants.push(Grant { field: field.into(), lines: None });
        self
    }
    /// The dialog lists exactly these lines as what is granted, and on
    /// Confirm the core sends exactly the same strings, as a list, at
    /// `field` (one of the feature's `core_only`): what is shown is what is sent.
    pub fn grant_lines(mut self, field: &str, lines: Vec<String>) -> Confirm {
        self.grants.push(Grant { field: field.into(), lines: Some(lines) });
        self
    }
    pub fn into_action(self) -> Action {
        Action::Confirm(Box::new(self))
    }
}

/// A body field the core sets on Confirm only: `true`, or (`lines`) the
/// list of lines the dialog showed as granted.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    pub field: String,
    pub lines: Option<Vec<String>>,
}

impl Grant {
    /// What goes in the body.
    pub fn value(&self) -> J {
        match &self.lines {
            Some(l) => J::Array(l.iter().cloned().map(J::String).collect()),
            None => J::Bool(true),
        }
    }
}

/// See [`Action::Prepare`].
#[derive(Debug, Clone)]
pub struct Prepare {
    /// The dialog's title while it loads.
    pub title: String,
    /// What the core reads first (its answer goes to `build`, never to a screen).
    pub call: Call,
    /// The intent that asked, handed back to `build`.
    pub asked: Asked,
    pub build: fn(&Ctx, &Asked, &J) -> Result<Confirm, String>,
}

impl PartialEq for Prepare {
    fn eq(&self, o: &Self) -> bool {
        // `build` is a function: two are told apart by what they are built from.
        (&self.title, &self.call, &self.asked) == (&o.title, &o.call, &o.asked)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub text: String,
    /// Lines starting with `+` green, `-` red.
    pub diff: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Secret {
    pub label: String,
    pub placeholder: String,
    /// Where it goes in the call's body: keys with dots between (`value`, `secrets.API_KEY`).
    pub field: String,
    pub multiline: bool,
    /// May stay empty (Confirm still works); empty, it is left out of the body.
    pub optional: bool,
}

/// Where an [`Extra`] shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// On that sandbox's row in the sidebar (`data.app.extras.sandboxes[name]`).
    Sandbox(String),
    /// In the top bar (`data.app.extras.topbar`).
    Topbar,
    /// On the bar of every pane showing that sandbox (drawn by the core).
    Pane(String),
    /// On the bar of every pane showing that session of that sandbox: its
    /// main agent (`thread: None`, any mode but a thread's) or that thread
    /// (drawn by the core).
    Session { sandbox: String, thread: Option<String> },
    /// A sub-row under that sandbox's row in the sidebar
    /// (`data.app.extras.rows[name]`, a list, drawn by `sandboxes.luau`):
    /// a dot of `tone`, the label, a right badge (`count`, else `help`),
    /// pressable with its `intent`; key `extra-row-<sandbox>-<id>`.
    SandboxRow(String),
    /// An entry of the left rail (`shell/rail.rs`), drawn zone by zone in
    /// [`Zone`] order, a hairline between two zones, then by `order` (lower
    /// first) within its zone. Key `rail-<id>`; `icon` is its mark (set it),
    /// `tone` its colour by meaning, `help` its tooltip (first line the
    /// name), `count` a badge, `intent` what a press asks. An entry whose
    /// intent is `open_panel {screen}` or `open_modal {screen}` shows as on
    /// while that screen is shown, and a second press closes a panel or an
    /// overlay it opened. Settings is the configure zone's last entry and
    /// Jarvis stands apart at the very bottom (both the core's own).
    Rail { zone: Zone, order: u16 },
    /// A status dot on the rail entry of that id (`Place::Rail`): `tone` is
    /// the dot's colour (`green`, `blue`, `amber`, `red`; `muted` a hollow
    /// grey ring; no such extra, no dot), drawn at the icon's lower right
    /// with a dark ring. Colour is never the only signal: say the state in
    /// the entry's own `help` too. Only `tone` is read.
    RailStatus(String),
}

/// The left rail's zones, top to bottom (`Place::Rail`).
///
/// * `Activity`: what happens now (Requests 10, Network 20, Team leads 30).
/// * `Browse`: views to read (Commits 10, Worktrees 20, Files 30).
/// * `Configure`: what is set up (MCP 10, Skills 20, Kits 30; Settings is
///   always its last, drawn by the core). Anchored at the bottom of the
///   rail, above Jarvis. When the rail is too short for every entry, the
///   configure zone's entries fold first into one "More" entry (a menu),
///   then the browse zone's; Settings, Jarvis and the activity zone never
///   fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Zone {
    Activity,
    Browse,
    Configure,
}

/// A button or a badge a feature adds to a slot drawn by someone else.
#[derive(Debug, Clone, PartialEq)]
pub struct Extra {
    pub place: Place,
    /// Unique within the place: the element key's suffix.
    pub id: String,
    /// Short text (may be empty when there is an icon).
    pub label: String,
    /// An icon name (`icons.rs`).
    pub icon: Option<String>,
    /// A theme colour name: `amber`, `red`, `green`, `blue`, `muted`.
    pub tone: String,
    pub help: String,
    /// A number shown as a badge (pending approvals...).
    pub count: Option<u64>,
    /// What a press asks: an intent `(name, args)`, checked as any.
    pub intent: Option<(String, J)>,
}

impl Extra {
    /// As screens see it.
    pub fn to_json(&self) -> J {
        serde_json::json!({
            "id": self.id, "label": self.label, "icon": self.icon, "tone": self.tone, "help": self.help, "count": self.count,
            "intent": self.intent.as_ref().map(|(n, a)| serde_json::json!({ "name": n, "args": a })),
        })
    }
}

/// What a feature's stand-in routes can touch: the fixture's `daemon` object
/// (its keys are the routes' answers) and the event stream.
pub struct Standin<'a> {
    pub data: &'a mut J,
    pub publish: &'a mut dyn FnMut(&str, J),
}

thread_local! {
    static LATER: std::cell::RefCell<Vec<(u64, String, J)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Standin<'_> {
    /// An event published after the data lock is released, `ms` after the
    /// previous one, before the answer is written (a bring-up's `provision`
    /// lines). In a fixture daemon they go out at once.
    pub fn publish_after(&mut self, ms: u64, name: &str, data: J) {
        LATER.with(|l| l.borrow_mut().push((ms, name.to_string(), data)));
    }
}

// ---------------------------------------------------------------------------
// The list

/// Declares the features: one line each, `module` (a file or folder under
/// `src/feat/`). The order is the order of their extras.
macro_rules! features {
    ($($m:ident),* $(,)?) => {
        $(pub mod $m;)*
        /// Every feature of the app.
        pub const FEATURES: &[&Feature] = &[$(&$m::FEATURE),*];
    };
}

features! {
    browser,
    canvas,
    commits,
    board,
    create,
    emphasis,
    envfile,
    files,
    network,
    relay,
    setup,
    // Out of Setup, each its own panel and rail entry (configure zone).
    mcp,
    skills,
    kits,
    shell_extras,
    mac,
    monitor,
    templates,
    threads,
    worktrees,
}

// ---------------------------------------------------------------------------
// Lookups the core uses

/// Puts `value` in `body` at a dotted path (`secrets.API_KEY`), making
/// objects on the way. Refused: an empty or odd key (only `[A-Za-z0-9_-]`),
/// more than 4 levels, and a path through or onto a field already there (a
/// secret never replaces what the feature put in the body).
pub fn put_at(body: &mut J, path: &str, value: J) -> Result<(), String> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.len() > 4 || parts.iter().any(|k| k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        return Err(format!("secret field '{path}': keys of letters, digits, _ and -, 4 levels at most"));
    }
    if body.is_null() {
        *body = J::Object(Map::new());
    }
    let mut cur = body;
    for (i, k) in parts.iter().enumerate() {
        let Some(o) = cur.as_object_mut() else { return Err(format!("secret field '{path}': goes through a field that is not an object")) };
        if i + 1 == parts.len() {
            if o.contains_key(*k) {
                return Err(format!("secret field '{path}': the body already has it"));
            }
            o.insert(k.to_string(), value);
            return Ok(());
        }
        cur = o.entry(k.to_string()).or_insert_with(|| J::Object(Map::new()));
    }
    Ok(())
}

/// Matches a route pattern with `{name}` segments against a path (its
/// query string ignored): the captured values, decoded, or `None`.
pub fn match_path(pattern: &str, path: &str) -> Option<Vec<(String, String)>> {
    let path = path.split('?').next().unwrap_or("");
    let (a, b): (Vec<&str>, Vec<&str>) = (pattern.split('/').collect(), path.split('/').collect());
    if a.len() != b.len() {
        return None;
    }
    let mut out = vec![];
    for (p, v) in a.iter().zip(b.iter()) {
        if let Some(name) = p.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
            if v.is_empty() {
                return None;
            }
            out.push((name.to_string(), decode(v)));
        } else if p != v {
            return None;
        }
    }
    Some(out)
}

/// A query string's value (`?a=b&c=d`), decoded.
pub fn query(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| decode(v)))
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The stand-in's feature sockets: the first feature whose `standin_ws`
/// takes `target`. False: no feature serves that route.
pub fn standin_socket(target: &str, peer: &mut dyn WsPeer) -> bool {
    FEATURES.iter().filter_map(|f| f.standin_ws).any(|h| h(target, peer))
}

/// The feature that owns surface `id`.
pub fn surface_owner(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().copied().find(|f| f.surfaces.contains(&id))
}

/// What a fixture daemon (and the stand-in) answers for a feature route:
/// the first feature whose `standin` takes it, else a `GET` of a declared
/// [`Data`] path answers that key of `data` (per sandbox: its entry).
pub fn answer_from(data: &mut J, method: &str, path: &str, body: &J, publish: &mut dyn FnMut(&str, J)) -> (u16, J) {
    let (r, later) = answer_later(data, method, path, body, publish);
    // No one to wait for here (a fixture): the delayed events go out now.
    for (_, n, d) in later {
        publish(&n, d);
    }
    r
}

/// [`answer_from`], with the events a route asked to publish later (the
/// stand-in plays them with the lock released).
pub fn answer_later(data: &mut J, method: &str, path: &str, body: &J, publish: &mut dyn FnMut(&str, J)) -> ((u16, J), Vec<(u64, String, J)>) {
    for f in FEATURES {
        if let Some(sf) = f.standin {
            LATER.with(|l| l.borrow_mut().clear());
            let mut st = Standin { data: &mut *data, publish: &mut *publish };
            if let Some(r) = sf(&mut st, method, path, body) {
                return (r, LATER.with(|l| std::mem::take(&mut *l.borrow_mut())));
            }
        }
    }
    (answer_data(data, method, path), vec![])
}

fn answer_data(data: &J, method: &str, path: &str) -> (u16, J) {
    if method == "GET" {
        for d in FEATURES.iter().flat_map(|f| f.data.iter()) {
            if let Some(caps) = match_path(d.path.split('?').next().unwrap_or(d.path), path) {
                let v = &data[d.key];
                let v = match caps.iter().find(|(k, _)| k == "sandbox") {
                    Some((_, sb)) => v[sb.as_str()].clone(),
                    None => v.clone(),
                };
                return (200, v);
            }
        }
    }
    (404, serde_json::json!({ "error": format!("no route {method} {}", path.split('?').next().unwrap_or("")) }))
}

/// The feature that owns intent `name`, and its row.
pub fn intent(name: &str) -> Option<(&'static Feature, &'static Spec)> {
    FEATURES.iter().find_map(|f| f.intents.iter().find(|s| s.name == name).map(|s| (*f, s)))
}

/// A feature's screen by name.
pub fn screen(name: &str) -> Option<(&'static Feature, &'static ScreenDecl)> {
    FEATURES.iter().find_map(|f| f.screens.iter().find(|s| s.name == name).map(|s| (*f, s)))
}

/// Every feature screen, `(name, source)`.
pub fn screens() -> impl Iterator<Item = (&'static str, &'static str)> {
    FEATURES.iter().flat_map(|f| f.screens.iter().map(|s| (s.name, s.src)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_well_formed() {
        let mut intents: Vec<&str> = crate::intent::REGISTRY.iter().map(|s| s.name).collect();
        let mut screens: Vec<&str> = crate::screen::BUILTIN.iter().map(|s| s.0).collect();
        let mut keys: Vec<&str> = vec!["sandboxes", "jarvis", "app"];
        for f in FEATURES {
            assert!(!f.name.is_empty());
            intents.extend(f.intents.iter().map(|s| s.name));
            for s in f.screens {
                assert!(s.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{}: [a-z0-9-] only", s.name);
                screens.push(s.name);
            }
            keys.extend(f.data.iter().map(|d| d.key));
        }
        for v in [&mut intents, &mut screens, &mut keys] {
            let n = v.len();
            v.sort();
            v.dedup();
            assert_eq!(n, v.len(), "a name is declared twice: {v:?}");
        }
    }

    #[test]
    fn secrets_go_at_their_path_and_never_over_a_field() {
        let mut b = serde_json::json!({ "spec": { "name": "x" } });
        put_at(&mut b, "secrets.API_KEY", "s1".into()).unwrap();
        put_at(&mut b, "secrets.OTHER", "s2".into()).unwrap();
        put_at(&mut b, "value", "v".into()).unwrap();
        assert_eq!(b, serde_json::json!({ "spec": { "name": "x" }, "secrets": { "API_KEY": "s1", "OTHER": "s2" }, "value": "v" }));
        for bad in ["spec.name", "spec.name.x", "a..b", "a.0.b c", "a.b.c.d.e", "", "secrets.API_KEY"] {
            assert!(put_at(&mut b, bad, "z".into()).is_err(), "{bad}");
        }
        assert_eq!((b["spec"]["name"].as_str(), b["secrets"]["API_KEY"].as_str()), (Some("x"), Some("s1")));
    }

    #[test]
    fn paths_match_their_patterns() {
        assert_eq!(match_path("/api/sandboxes/{sandbox}/ports", "/api/sandboxes/web%20app/ports?x=1"), Some(vec![("sandbox".into(), "web app".into())]));
        assert_eq!(match_path("/api/sandboxes/{sandbox}/ports", "/api/sandboxes//ports"), None);
        assert_eq!(match_path("/api/approvals", "/api/approvals/respond"), None);
        assert_eq!(query("/api/fs?path=%2Fhome&x=2", "path").as_deref(), Some("/home"));
    }

    #[test]
    fn per_sandbox_paths_are_seen() {
        assert!(Data::new("ports", "/api/sandboxes/{sandbox}/ports").per_sandbox());
        assert!(!Data::new("approvals", "/api/approvals").per_sandbox());
    }
}
