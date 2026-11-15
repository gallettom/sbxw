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
    /// Buttons and badges in slots drawn by others, from the current data.
    pub extras: Option<fn(&Ctx) -> Vec<Extra>>,
    /// The stand-in daemon's answer to a request for this feature, if the
    /// route is its: `(status, JSON body)`.
    pub standin: Option<fn(&mut Standin, &str, &str, &J) -> Option<(u16, J)>>,
}

impl Feature {
    /// A feature with nothing declared, to fill with `..Feature::EMPTY`.
    pub const EMPTY: Feature = Feature { name: "", data: &[], events: &[], screens: &[], intents: &[], act: no_act, on_event: None, on_answer: None, extras: None, standin: None };
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
    Modal { width: f32 },
    /// A panel docked on the right, beside Jarvis's (`open_panel`,
    /// `close_panel`). Several open show as tabs; one is shown at a time.
    Panel { title: &'static str, icon: &'static str, width: f32 },
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
}

impl Call {
    pub fn post(path: impl Into<String>, body: J, what: impl Into<String>) -> Call {
        Call { method: Method::Post, path: path.into(), body, what: what.into(), then: vec![], store: None, notify: true, timeout: 120, tag: String::new() }
    }
    pub fn get(path: impl Into<String>, what: impl Into<String>) -> Call {
        Call { method: Method::Get, path: path.into(), body: J::Null, what: what.into(), then: vec![], store: None, notify: false, timeout: 30, tag: String::new() }
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
    /// Fields drawn by the core whose values never reach Luau, fixtures,
    /// logs or a headless tree: each is put in the call's body at its
    /// `field` on Confirm (handed once, then dropped).
    pub secrets: Vec<Secret>,
    pub confirm: String,
    pub cancel: String,
    /// Confirm in red (removing, widening access).
    pub danger: bool,
    pub call: Call,
}

impl Confirm {
    pub fn new(title: impl Into<String>, call: Call) -> Confirm {
        Confirm { title: title.into(), lines: vec![], preview: None, ack: None, secrets: vec![], confirm: "Confirm".into(), cancel: "Cancel".into(), danger: false, call }
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
    pub fn into_action(self) -> Action {
        Action::Confirm(Box::new(self))
    }
}

/// See [`Action::Prepare`].
#[derive(Debug, Clone, PartialEq)]
pub struct Prepare {
    /// The dialog's title while it loads.
    pub title: String,
    /// What the core reads first (its answer goes to `build`, never to a screen).
    pub call: Call,
    /// The intent that asked, handed back to `build`.
    pub asked: Asked,
    pub build: fn(&Ctx, &Asked, &J) -> Result<Confirm, String>,
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
    fn per_sandbox_paths_are_seen() {
        assert!(Data::new("ports", "/api/sandboxes/{sandbox}/ports").per_sandbox());
        assert!(!Data::new("approvals", "/api/approvals").per_sandbox());
    }
}
