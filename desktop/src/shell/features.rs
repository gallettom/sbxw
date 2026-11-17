//! The core side of features (`src/feat/`): reading their data, running
//! their intents and actions, the generic confirmation dialog, and the
//! slots their screens show in (a modal, docked panels, extras on others'
//! slots).

use super::App;
use crate::approval::{self, ConfirmAnswers};
use crate::client::LinkEvent;
use crate::feat::{self, Action, Asked, Call, Confirm, Ctx, Extra, Feature, Place, Slot};
use crate::intent::{Intent, Risk};
use crate::theme::{c, layout, radius};
use rune::{col, el, row, text, Cursor, El, TextEdit};
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The confirmation showing.
pub struct Dialog {
    pub feature: &'static str,
    /// The screen (or `feature:<name>`) whose intent asked.
    pub from: String,
    pub title: String,
    pub confirm: Option<Confirm>,
    /// Waiting for the call a `Prepare` makes (its id).
    pub preparing: Option<u64>,
    pub prepare: Option<Box<feat::Prepare>>,
    /// The confirmed call is running (its id).
    pub running: Option<u64>,
    pub error: Option<String>,
    pub acked: bool,
    /// One secure edit per `confirm.secrets`.
    pub secrets: Vec<TextEdit>,
    /// The tag its feature hears if it closes unsent (`on_dismiss`).
    pub tag: String,
}

/// Features' state in the app.
#[derive(Default)]
pub struct Feats {
    /// Data read for features, by key (over the fixture's keys).
    pub data: Map<String, J>,
    /// Why a key's last read failed (the daemon's words), until the next
    /// read works: `data.app.errors[key]`. The key keeps its last good value.
    pub errors: Map<String, J>,
    /// The modal screen showing and its argument.
    pub modal: Option<(String, J)>,
    /// Open panels, in the order opened; `panel` the one shown.
    pub panels: Vec<String>,
    pub panel: Option<String>,
    /// Open panels shown side by side, each in its own column, instead of as tabs.
    pub split: bool,
    pub dialog: Option<Dialog>,
    /// Data keys to read again at that time (bursts of events merged).
    due: HashMap<String, Instant>,
    /// Data keys read every N seconds: next time.
    every: HashMap<String, Instant>,
    /// An intent offered on the notice showing.
    pub notice_then: Option<(String, J)>,
    /// Extras on pane bars, from the last data.
    pub pane_extras: Vec<Extra>,
    next_id: u64,
    /// The running sandboxes per-sandbox data was last read for.
    running: Vec<String>,
    /// Answers of the core's own operations ([`feat::Method::Core`]), handed on after the dialog marks the call running.
    core_done: Vec<LinkEvent>,
    /// Native requests a feature made, run at the next poll (they need Rune's `Tx`).
    pub asks: Vec<(&'static str, Ask)>,
}

/// A feature's request that needs the window: a file dialog, a clipboard read.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    PickFiles { title: String, extensions: Vec<String> },
    ReadClipboard,
    /// `copy_text`: onto the clipboard.
    Copy(String),
    /// Move Rune's keyboard focus to that element key.
    Focus(String),
    /// A surface's paste chord off macOS: the clipboard's text to that surface.
    PasteText(String),
    /// A download to a place the person picks: the save dialog, then the
    /// download runs as call `id` (answered to the feature's `on_answer`).
    SaveAs { id: u64, call: Call },
}

/// The whole rule for an intent of risk Approval: its feature may answer only
/// ONE `Confirm` or ONE `Prepare` (the core draws that dialog and runs the
/// call on the person's click). Anything else is refused, nothing runs.
pub fn admit(risk: Risk, name: &str, actions: Vec<Action>) -> Result<Vec<Action>, String> {
    match risk {
        Risk::Direct => Ok(actions),
        Risk::Approval(_) => match actions.as_slice() {
            [Action::Confirm(_)] | [Action::Prepare(_)] => Ok(actions),
            _ => Err(format!("{name} needs the person's approval, but its feature answered without the core's dialog: refused")),
        },
    }
}

impl App {
    fn feat_ctx_data(&self) -> J {
        self.screen_data()
    }

    pub(super) fn feature_of(name: &str) -> Option<&'static Feature> {
        feat::FEATURES.iter().copied().find(|f| f.name == name)
    }

    /// Slots and features' intents.
    pub(super) fn run_feature_intent(&mut self, from: &str, intent: Intent) {
        match intent {
            Intent::OpenModal { screen, arg } => self.open_modal(&screen, arg.map(J::String).unwrap_or(J::Null)),
            Intent::CloseModal => self.feats.modal = None,
            Intent::OpenPanel { screen } => self.open_panel(&screen),
            Intent::ClosePanel { screen } => {
                let s = screen.or_else(|| self.feats.panel.clone());
                if let Some(s) = s {
                    self.close_panel(&s);
                }
            }
            Intent::Feature(a) => self.feature_intent(from, a),
            other => self.notify(format!("{from}: {} is not a feature intent", other.name()), true),
        }
    }

    fn feature_intent(&mut self, from: &str, a: Asked) {
        let Some((f, spec)) = feat::intent(a.name) else {
            return self.notify(format!("{from}: unknown intent {}", a.name), true);
        };
        let data = self.feat_ctx_data();
        let answer = (f.act)(&Ctx { data: &data }, &a).and_then(|acts| admit(spec.risk, a.name, acts));
        match answer {
            Ok(acts) => self.feature_actions(f, from, acts),
            Err(e) => self.notify(format!("{}: {e}", f.name), true),
        }
    }

    /// Runs what a feature asked.
    pub(super) fn feature_actions(&mut self, f: &'static Feature, from: &str, acts: Vec<Action>) {
        for a in acts {
            match a {
                Action::Call(call) => {
                    self.feature_call(f, call, false);
                }
                Action::Confirm(c) => self.open_dialog(f, from, Some(*c), None),
                Action::Prepare(p) => self.open_dialog(f, from, None, Some(p)),
                Action::Refresh(key) => self.refresh_key(key),
                Action::Fetch { key, path } => {
                    if let (Some(d), Some(l)) = (self.daemon.clone(), &self.link) {
                        l.fetch(d, key, vec![(None, path)]);
                    }
                }
                Action::Set { key, value } => {
                    self.feats.data.insert(key, value);
                }
                Action::OpenModal { screen, arg } => self.open_modal(&screen, arg),
                Action::CloseModal => self.feats.modal = None,
                Action::OpenPanel(s) => self.open_panel(&s),
                Action::ClosePanel(s) => self.close_panel(&s),
                Action::Notice { text, error, then } => {
                    self.notify(text, error);
                    self.feats.notice_then = then;
                }
                Action::OpenLink(url) => self.run_intent(from, Intent::OpenLink { url }),
                // Both only from the person's own press on one of the
                // feature's screens: never from its data, events or answers
                // (`feature:<name>`), nor while typing.
                Action::PickFiles { .. } | Action::ReadClipboard if !self.pressed(from) => {
                    self.notify(format!("{}: opening a file or reading the clipboard runs only from your own press: refused", f.name), true);
                }
                // A file written to the person's machine, a file manager opened: from a press too.
                Action::Download(_) | Action::Reveal(_) if !self.pressed(from) => {
                    self.notify(format!("{}: a download or showing a file runs only from your own press: refused", f.name), true);
                }
                Action::PickFiles { title, extensions, multiple } => {
                    if multiple {
                        self.notify(format!("{}: the file dialog picks one file at a time; drop several files on the panel to hand them in at once", f.name), false);
                    }
                    self.feats.asks.push((f.name, Ask::PickFiles { title, extensions }));
                }
                Action::ReadClipboard => self.feats.asks.push((f.name, Ask::ReadClipboard)),
                Action::Image { key, bytes } => {
                    if bytes.is_empty() {
                        crate::picture::forget(&key);
                    } else {
                        match crate::picture::prepare(&bytes, crate::picture::Limits::default(), Some(2048), crate::picture::Encode::Pixels) {
                            Ok(p) => crate::picture::show(&key, &p),
                            Err(e) => self.notify(format!("{}: {e}", f.name), true),
                        }
                    }
                }
                Action::Remember { key, value } => {
                    let mut mine = self.store.get("features")[f.name].as_object().cloned().unwrap_or_default();
                    if value.is_null() {
                        mine.remove(&key);
                    } else {
                        mine.insert(key, value);
                    }
                    self.store.set_in("features", f.name, if mine.is_empty() { J::Null } else { J::Object(mine) });
                }
                Action::OpenSocket { id, path, opts } => self.open_socket(f, id, path, opts),
                Action::CloseSocket(id) => crate::socket::close(&id),
                Action::Focus(key) => self.feats.asks.push((f.name, Ask::Focus(key))),
                Action::Download(d) => {
                    if d.direct() {
                        let call = d.call(format!("download {}", crate::safe::file_name(&d.name)));
                        self.core_call(f, call, true);
                    } else {
                        self.notify(format!("{}: a download you choose the place of, or that opens, runs only from the app's own dialog: refused", f.name), true);
                    }
                }
                Action::ForgetFiles(p) => crate::files::forget(&p),
                Action::Reveal(p) => {
                    if let Err(e) = crate::files::reveal(&p) {
                        self.notify(format!("{}: {e}", f.name), true);
                    }
                }
                Action::Intent { name, args } => match crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })) {
                    Ok(list) => crate::screen::Host::intents(self, &format!("feature:{}", f.name), list),
                    Err(e) => self.notify(format!("{}: {e}", f.name), true),
                },
            }
        }
    }

    /// One of the core's own operations, behind its dialog: `(status, body)` as a daemon would answer.
    fn core_op(&mut self, path: &str) -> Result<(u16, J), String> {
        if let Some(name) = path.strip_prefix("core:screen-restore/") {
            let Some(sc) = self.screens.iter().find(|s| s.name == name) else { return Ok((404, json!({ "error": format!("no screen named {name}") }))) };
            let p = sc.override_file().to_path_buf();
            return match std::fs::remove_file(&p) {
                Ok(()) => {
                    log::info!("removed the override {}", p.display());
                    self.store.set_in("overrides", name, J::Null);
                    Ok((200, json!({ "ok": true, "removed": p })))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((200, json!({ "ok": true }))),
                Err(e) => Ok((500, json!({ "error": format!("could not remove {}: {e}", p.display()) }))),
            };
        }
        Err(format!("no core operation {path}"))
    }

    /// Whether an intent from `from` comes from the person's own press: a
    /// screen (not while an input's per-keystroke action runs), a pane bar,
    /// a notice or a shortcut; never a feature's own chain (`feature:<name>`).
    pub(super) fn pressed(&self, from: &str) -> bool {
        !from.starts_with("feature:") && !self.screens.iter().any(|s| s.name == from && s.ui.editing)
    }

    /// What the person handed a feature: its `on_files` runs (no `on_files`: said so).
    pub(super) fn feature_incoming(&mut self, name: &str, inc: feat::Incoming) {
        let Some(f) = Self::feature_of(name) else { return };
        let Some(on) = f.on_files else {
            return self.notify(format!("{name} takes no files"), true);
        };
        log::info!("{name}: {}", match &inc {
            feat::Incoming::Files(p) => format!("{} file(s) handed in", p.len()),
            feat::Incoming::Png(b) => format!("a picture of {} bytes handed in", b.len()),
        });
        if let feat::Incoming::Files(p) = &inc {
            crate::files::hand_in(p);
        }
        let data = self.feat_ctx_data();
        let acts = on(&Ctx { data: &data }, inc);
        self.feature_actions(f, &format!("feature:{name}"), acts);
        self.push_data();
    }

    /// Files dropped on pane `i`: handed in, to the first feature that takes pane drops.
    pub(super) fn pane_drop(&mut self, i: usize, paths: &[std::path::PathBuf]) {
        let Some(p) = self.panes.get(i) else { return };
        let Some(sandbox) = p.sandbox.clone() else { return };
        let thread = p.thread.clone();
        let Some((f, on)) = feat::FEATURES.iter().find_map(|f| f.on_pane_drop.map(|on| (*f, on))) else {
            return self.notify("Nothing here takes files dropped on a terminal", false);
        };
        crate::files::hand_in(paths);
        let data = self.feat_ctx_data();
        let acts = on(&Ctx { data: &data }, &feat::PaneDrop { paths: paths.to_vec(), sandbox, thread });
        self.feature_actions(f, &format!("feature:{}", f.name), acts);
        self.push_data();
    }

    /// A feature's global shortcut, if `ev` is one (see [`feat::Shortcut`]): runs its intent.
    pub(super) fn shortcut(&mut self, ev: &rune::KeyEvent, tx: &rune::Tx) -> bool {
        let m = ev.mods;
        let mac = cfg!(target_os = "macos");
        if !m.alt || m.shift || (mac && !m.ctrl) || (!mac && m.ctrl) || (mac && m.logo) {
            return false;
        }
        // The key's place first (Option changes what a key types on macOS), else what it typed.
        let typed = match (tx.physical_key(), &ev.key) {
            (Some(code), _) => format!("{code:?}").strip_prefix("Key").and_then(|k| k.chars().next()).map(|c| c.to_ascii_lowercase()),
            (None, rune::Key::Text(t)) => t.chars().next().map(|c| c.to_ascii_lowercase()),
            (None, rune::Key::Command(c)) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        let Some(k) = typed else { return false };
        let Some(sc) = feat::FEATURES.iter().flat_map(|f| f.shortcuts.iter()).find(|s| s.key == k) else { return false };
        let args: J = serde_json::from_str(sc.args).unwrap_or(json!({}));
        self.press_extra("shortcut", sc.intent, &args);
        true
    }

    /// The feature whose panel is shown, if it takes files.
    pub(super) fn files_target(&self) -> Option<&'static str> {
        let (f, _) = feat::screen(self.feats.panel.as_deref()?)?;
        f.on_files.map(|_| f.name)
    }

    /// Runs the window requests features made (file dialog, clipboard read).
    pub(super) fn run_asks(&mut self, tx: &mut rune::Tx) {
        for (name, ask) in std::mem::take(&mut self.feats.asks) {
            match ask {
                Ask::PickFiles { title, extensions } => {
                    let mut o = rune::FileDialogOptions { title, ..Default::default() };
                    if !extensions.is_empty() {
                        let ex: Vec<&str> = extensions.iter().map(String::as_str).collect();
                        o = o.filter("Files", &ex);
                    }
                    tx.open_file(o, move |s: &mut App, r, _tx| match r {
                        Ok(Some(p)) => s.feature_incoming(name, feat::Incoming::Files(vec![p])),
                        Ok(None) => {}
                        Err(e) => {
                            log::info!("{name}: file dialog: {e}");
                            s.notify("Drop the file on this panel, or paste an image.", false);
                            s.push_data();
                        }
                    });
                }
                Ask::Copy(text) => {
                    // What was copied is what the notice shows: its size and
                    // its first characters, controls made visible (as OSC 52's).
                    let line = format!("Copied {} to the clipboard: {}", approval::size_label(text.len()), approval::osc_preview(&text));
                    tx.copy(text);
                    self.notify(line, false);
                }
                Ask::Focus(key) => tx.focus(key, true),
                Ask::SaveAs { id, call } => {
                    let suggested = crate::safe::file_name(call.body["name"].as_str().unwrap_or(""));
                    let o = rune::FileDialogOptions { title: format!("Save {suggested}"), file_name: Some(suggested), directory: crate::files::downloads_dir().ok(), ..Default::default() };
                    tx.save_file(o, move |s: &mut App, r, _tx| match r {
                        Ok(Some(p)) => {
                            let dir = p.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
                            let n = p.file_name().and_then(|n| n.to_str()).map(str::to_string);
                            s.start_download(name, id, call, dir, n);
                        }
                        Ok(None) => s.feats_link(LinkEvent::Answer { id, feature: name, call, result: Ok((499, json!({ "cancelled": true, "error": "cancelled" }))) }).then_some(()).unwrap_or(()),
                        Err(e) => {
                            log::info!("{name}: save dialog: {e}");
                            s.notify("No save dialog here: the file goes to your Downloads folder instead.", false);
                            match crate::files::downloads_dir() {
                                Ok(d) => s.start_download(name, id, call, d, None),
                                Err(e) => s.notify(e, true),
                            }
                            s.push_data();
                        }
                    });
                }
                Ask::PasteText(id) => tx.read_clipboard(move |s: &mut App, c: &rune::ClipboardContent, _tx| {
                    s.surface_paste(&id, c.text());
                    s.push_data();
                }),
                Ask::ReadClipboard => tx.read_clipboard(move |s: &mut App, c: &rune::ClipboardContent, _tx| match c.image() {
                    Some(img) => s.feature_incoming(name, feat::Incoming::Png(img.png().to_vec())),
                    None => {
                        s.notify("The clipboard holds no picture", false);
                        s.push_data();
                    }
                }),
            }
        }
    }

    /// Opens a feature's socket ([`crate::socket`]): its events to its
    /// `on_socket` on the socket's thread, the actions back here.
    fn open_socket(&mut self, f: &'static Feature, id: String, path: String, opts: crate::socket::Opts) {
        let Some(on) = f.on_socket else {
            return self.notify(format!("{}: opens a socket but has no on_socket", f.name), true);
        };
        if !path.starts_with("/ws/") {
            return self.notify(format!("{}: {path} is not a /ws/ route", f.name), true);
        }
        let name = f.name;
        let sid = id.clone();
        crate::socket::open(self.current.clone(), &id, &path, opts, Box::new(move |ev| crate::socket::post(name, on(&sid, ev))));
    }

    /// Runs what features' socket threads asked, and surface focus and size changes.
    pub(super) fn surfaces_poll(&mut self, tx: &mut rune::Tx) -> bool {
        let mut changed = false;
        for (name, acts) in crate::socket::take_actions() {
            if let Some(f) = Self::feature_of(name) {
                self.feature_actions(f, &format!("feature:{name}"), acts);
                changed = true;
            }
        }
        let mut evs = crate::surface::focus_changed(tx.focused());
        let (sizes, next) = crate::surface::due_resizes(Instant::now());
        evs.extend(sizes);
        if let Some(n) = next {
            tx.wake_after(n.saturating_duration_since(Instant::now()));
        }
        for ev in evs {
            self.feats_surface(ev);
            changed = true;
        }
        changed
    }

    /// A surface event to the feature that owns the surface.
    pub(super) fn feats_surface(&mut self, ev: crate::surface::SurfaceEvent) {
        let Some(f) = feat::surface_owner(ev.surface()) else { return };
        let Some(on) = f.on_surface else { return };
        let acts = on(&ev);
        if !acts.is_empty() {
            self.feature_actions(f, &format!("feature:{}", f.name), acts);
            self.push_data();
        }
    }

    /// The person pasted on a surface: its text to the feature, and a
    /// notice saying how much left for it.
    pub(super) fn surface_paste(&mut self, id: &str, text: &str) {
        if text.is_empty() {
            return self.notify("The clipboard holds no text", false);
        }
        let owner = feat::surface_owner(id).map(|f| f.name).unwrap_or("the surface");
        self.notify(format!("Pasted {} into {owner}: {}", approval::size_label(text.len()), approval::osc_preview(text)), false);
        self.feats_surface(crate::surface::SurfaceEvent::Paste { surface: id.to_string(), text: text.to_string() });
    }

    /// The app's keys on a surface (down or up): true when the app takes it
    /// (it then never reaches the picture).
    pub(super) fn surface_key_app(&mut self, ev: &rune::KeyEvent, down: bool, tx: &mut rune::Tx) -> bool {
        let Some(id) = crate::surface::focused() else { return false };
        let ptt = self.wiring.reserved;
        if !down {
            return crate::surface::reserved(ev, ptt, false);
        }
        if crate::surface::is_release(ev) {
            tx.blur();
            return true;
        }
        if ev.key == rune::Key::Function(6) {
            self.feats_surface(crate::surface::SurfaceEvent::F6 { surface: id });
            return true;
        }
        if crate::surface::is_paste(ev) {
            self.feats.asks.push(("surface", Ask::PasteText(id)));
            return true;
        }
        if self.shortcut(ev, tx) {
            self.push_data();
            return true;
        }
        crate::surface::reserved(ev, ptt, false)
    }

    fn next_id(&mut self) -> u64 {
        self.feats.next_id += 1;
        self.feats.next_id
    }

    /// Sends a call; returns its id (0: not connected, said in a notice).
    /// `confirmed`: sent by the person's Confirm on the core's dialog; only
    /// then may the body carry the feature's `core_only` fields.
    fn feature_call(&mut self, f: &'static Feature, call: Call, confirmed: bool) -> u64 {
        if let Err(e) = core_only_check(f, &call, confirmed) {
            self.notify(format!("{}: {e}", f.name), true);
            return 0;
        }
        if call.method == feat::Method::Core && call.path.starts_with("core:") && !call.path.starts_with("core:screen-restore/") {
            return self.core_call(f, call, confirmed);
        }
        if call.method == feat::Method::Core {
            if !confirmed {
                self.notify(format!("{}: {} runs only from the app's own dialog: refused", f.name, call.what), true);
                return 0;
            }
            let id = self.next_id();
            let result = self.core_op(&call.path);
            // Answered once the dialog marks it running (`confirm_click`).
            self.feats.core_done.push(LinkEvent::Answer { id, feature: f.name, call, result });
            return id;
        }
        let (Some(d), Some(_)) = (self.daemon.clone(), &self.link) else {
            self.notify(format!("{}: not connected: {}", call.what, self.connection.text), true);
            return 0;
        };
        let id = self.next_id();
        log::info!("{}: {:?} {} ({})", f.name, call.method, call.path, call.what);
        self.link.as_ref().unwrap().call(d, id, f.name, call);
        id
    }

    /// The core's file operations (`src/files.rs`), off the UI thread; the
    /// rule of each (direct or only from the core's dialog) is checked here.
    fn core_call(&mut self, f: &'static Feature, call: Call, confirmed: bool) -> u64 {
        let op = call.path.strip_prefix("core:").unwrap_or("").to_string();
        let b = call.body.clone();
        let s = |k: &str| b[k].as_str().unwrap_or("").to_string();
        let direct = match op.as_str() {
            "inspect" | "reveal" => true,
            "download" => s("to") == "downloads" && s("after") != "open",
            "open" | "upload" => false,
            _ => {
                self.notify(format!("{}: no core operation {}", f.name, call.path), true);
                return 0;
            }
        };
        if !direct && !confirmed {
            self.notify(format!("{}: {} runs only from the app's own dialog: refused", f.name, call.what), true);
            return 0;
        }
        // The box of the dialog that confirmed it, ticked (a `warn` file opens only then).
        let acked = confirmed && self.feats.dialog.as_ref().is_some_and(|d| d.acked && d.confirm.as_ref().is_some_and(|c| c.ack.is_some()));
        if self.link.is_none() {
            self.notify(format!("{}: not connected", call.what), true);
            return 0;
        }
        let id = self.next_id();
        let link = self.link.as_ref().unwrap();
        let daemon = self.daemon.clone();
        let need = |d: Option<std::sync::Arc<dyn crate::client::Daemon>>| d.ok_or_else(|| "not connected to the daemon".to_string());
        log::info!("{}: core {op} ({})", f.name, call.what);
        match op.as_str() {
            "inspect" => {
                let p = std::path::PathBuf::from(s("path"));
                link.core(id, f.name, call, move || crate::files::inspect(&p).map(|v| (200, v)));
            }
            "reveal" => {
                let p = std::path::PathBuf::from(s("path"));
                link.core(id, f.name, call, move || crate::files::reveal(&p));
            }
            "open" => {
                let p = std::path::PathBuf::from(s("path"));
                link.core(id, f.name, call, move || crate::files::open(&p, acked));
            }
            "upload" => {
                let (route, file, ct, tag) = (s("route"), std::path::PathBuf::from(s("file")), s("content_type"), s("tag"));
                let ct = if ct.is_empty() { "application/octet-stream".to_string() } else { ct };
                let w = self.wake_slot.lock().unwrap().clone();
                let wake: Option<crate::client::Wake> = w.map(|w| std::sync::Arc::new(move || w()) as crate::client::Wake);
                link.core(id, f.name, call, move || crate::files::upload(need(daemon)?.as_ref(), &route, &file, &ct, &tag, wake));
            }
            _ if s("to") == "ask" => self.feats.asks.push((f.name, Ask::SaveAs { id, call })),
            _ => {
                let dir = match crate::files::downloads_dir() {
                    Ok(d) => d,
                    Err(e) => {
                        self.notify(format!("{}: {e}", f.name), true);
                        return 0;
                    }
                };
                self.start_download(f.name, id, call, dir, None);
            }
        }
        id
    }

    /// Runs a download into `dir` (under `name`, else the call's), then what comes after.
    fn start_download(&mut self, feature: &'static str, id: u64, call: Call, dir: std::path::PathBuf, name: Option<String>) {
        let Some(link) = &self.link else { return };
        let daemon = self.daemon.clone();
        let b = call.body.clone();
        let (get, after) = (b["get"].as_str().unwrap_or("").to_string(), b["after"].as_str().unwrap_or("nothing").to_string());
        let name = name.unwrap_or_else(|| b["name"].as_str().unwrap_or("").to_string());
        link.core(id, feature, call, move || {
            let d = daemon.ok_or_else(|| "not connected to the daemon".to_string())?;
            let (code, v) = crate::files::download(d.as_ref(), &get, &dir, &name)?;
            if (200..300).contains(&code) {
                let p = std::path::PathBuf::from(v["saved"].as_str().unwrap_or(""));
                let then = match after.as_str() {
                    "reveal" => crate::files::reveal(&p).map(|_| ()),
                    // Behind the core's dialog (core_call's rule); a `warn` file needs its box, which a download dialog has.
                    "open" => crate::files::open(&p, true).map(|_| ()),
                    _ => Ok(()),
                };
                if let Err(e) = then {
                    let mut v = v.clone();
                    v["after_error"] = J::String(e);
                    return Ok((code, v));
                }
            }
            Ok((code, v))
        });
    }

    pub(super) fn open_modal(&mut self, screen: &str, arg: J) {
        match feat::screen(screen) {
            Some((_, s)) if matches!(s.slot, Slot::Modal { .. }) => self.feats.modal = Some((screen.to_string(), arg)),
            _ => self.notify(format!("open_modal: no dialog screen named {screen}"), true),
        }
    }

    fn open_panel(&mut self, screen: &str) {
        match feat::screen(screen) {
            Some((_, s)) if matches!(s.slot, Slot::Panel { .. }) => {
                if !self.feats.panels.iter().any(|p| p == screen) {
                    self.feats.panels.push(screen.to_string());
                }
                self.feats.panel = Some(screen.to_string());
            }
            _ => self.notify(format!("open_panel: no panel screen named {screen}"), true),
        }
    }

    fn close_panel(&mut self, screen: &str) {
        self.feats.panels.retain(|p| p != screen);
        if self.feats.panel.as_deref() == Some(screen) {
            self.feats.panel = self.feats.panels.last().cloned();
        }
    }

    fn open_dialog(&mut self, f: &'static Feature, from: &str, confirm: Option<Confirm>, prepare: Option<Box<feat::Prepare>>) {
        let title = confirm.as_ref().map(|c| c.title.clone()).or_else(|| prepare.as_ref().map(|p| p.title.clone())).unwrap_or_default();
        let secrets = confirm.as_ref().map(secret_edits).unwrap_or_default();
        let tag = confirm.as_ref().map(|c| c.call.tag.clone()).or_else(|| prepare.as_ref().map(|p| p.call.tag.clone())).unwrap_or_default();
        let mut d = Dialog { feature: f.name, from: from.to_string(), title, confirm, preparing: None, prepare: None, running: None, error: None, acked: false, secrets, tag };
        if let Some(p) = prepare {
            let id = self.feature_call(f, p.call.clone().quiet(), false);
            d.preparing = Some(id);
            if id == 0 {
                d.error = Some(format!("Not connected: {}", self.connection.text));
            }
            d.prepare = Some(p);
        }
        // A newer one replaces the one showing (the person answers one at a time).
        self.dismiss_dialog();
        self.feats.dialog = Some(d);
    }

    /// Closes the core's dialog; if its call never went out, its feature
    /// hears it (`on_dismiss`, with the call's tag).
    pub(super) fn dismiss_dialog(&mut self) -> bool {
        let Some(d) = self.feats.dialog.take() else { return false };
        if d.running.is_none() {
            let tag = d.tag.clone();
            if let Some((f, on)) = Self::feature_of(d.feature).and_then(|f| f.on_dismiss.map(|on| (f, on))) {
                let data = self.feat_ctx_data();
                let acts = on(&Ctx { data: &data }, &tag);
                self.feature_actions(f, &format!("feature:{}", f.name), acts);
            }
        }
        true
    }

    // ---- data ----

    /// Reads a declared data key now (per sandbox: for every running sandbox).
    pub(super) fn refresh_key(&mut self, key: &str) {
        let Some(d) = feat::FEATURES.iter().flat_map(|f| f.data.iter()).find(|d| d.key == key) else {
            return log::warn!("refresh: no data key {key}");
        };
        let (Some(daemon), Some(l)) = (self.daemon.clone(), &self.link) else { return };
        let paths = if d.per_sandbox() {
            self.sandboxes.iter().filter(|s| s.status == "running").map(|s| (Some(s.name.clone()), d.path.replace("{sandbox}", &crate::client::enc(&s.name)))).collect::<Vec<_>>()
        } else {
            vec![(None, d.path.to_string())]
        };
        if paths.is_empty() {
            self.feats.data.insert(key.to_string(), json!({}));
            return;
        }
        l.fetch(daemon, key.to_string(), paths);
    }

    /// At (re)connection: every non-lazy key.
    pub(super) fn feats_connected(&mut self) {
        let now = Instant::now();
        for d in feat::FEATURES.iter().flat_map(|f| f.data.iter()) {
            if !d.lazy {
                self.feats.due.insert(d.key.to_string(), now);
            }
            if d.every > 0 {
                self.feats.every.insert(d.key.to_string(), now + Duration::from_secs(d.every));
            }
        }
    }

    /// The list changed: per-sandbox keys follow the running set.
    pub(super) fn feats_sandboxes(&mut self) {
        let running: Vec<String> = self.sandboxes.iter().filter(|s| s.status == "running").map(|s| s.name.clone()).collect();
        if running != self.feats.running {
            self.feats.running = running;
            let now = Instant::now();
            for d in feat::FEATURES.iter().flat_map(|f| f.data.iter()) {
                if d.per_sandbox() && !d.lazy {
                    self.feats.due.insert(d.key.to_string(), now);
                }
            }
        }
    }

    /// A stream event: data keys read again 300 ms later; features told.
    pub(super) fn feats_bus(&mut self, name: &str, raw: &str) -> bool {
        let soon = Instant::now() + Duration::from_millis(300);
        for d in feat::FEATURES.iter().flat_map(|f| f.data.iter()) {
            if d.on.contains(&name) {
                self.feats.due.entry(d.key.to_string()).or_insert(soon);
            }
        }
        let mut changed = false;
        let body: J = serde_json::from_str(raw).unwrap_or(J::String(raw.to_string()));
        for f in feat::FEATURES {
            if let (true, Some(on)) = (f.events.contains(&name), f.on_event) {
                let data = self.feat_ctx_data();
                let acts = on(&Ctx { data: &data }, name, &body);
                if !acts.is_empty() {
                    changed = true;
                    self.feature_actions(f, &format!("feature:{}", f.name), acts);
                }
            }
        }
        changed
    }

    /// Due reads; returns when to wake next.
    pub(super) fn feats_poll(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let due: Vec<String> = self.feats.due.iter().filter(|(_, t)| **t <= now).map(|(k, _)| k.clone()).collect();
        for k in &due {
            self.feats.due.remove(k);
        }
        let mut every = vec![];
        for (k, t) in self.feats.every.iter_mut() {
            if *t <= now {
                every.push(k.clone());
                let secs = feat::FEATURES.iter().flat_map(|f| f.data.iter()).find(|d| d.key == k).map_or(60, |d| d.every);
                *t = now + Duration::from_secs(secs);
            }
        }
        if self.daemon.is_some() {
            for k in due.iter().chain(every.iter()) {
                self.refresh_key(k);
            }
        }
        self.feats.due.values().chain(self.feats.every.values()).min().copied()
    }

    /// What the link answered for features.
    pub(super) fn feats_link(&mut self, ev: LinkEvent) -> bool {
        match ev {
            LinkEvent::DataPer { key, results } => {
                // Per sandbox: a failed read keeps that sandbox's last good
                // value, and its reason goes in `errors[key][sandbox]`.
                let old = self.feats.data.get(&key).cloned().unwrap_or(J::Null);
                let mut v = Map::new();
                let mut errs = Map::new();
                for (sb, r) in results {
                    match r {
                        Ok(x) => {
                            v.insert(sb, x);
                        }
                        Err(e) => {
                            log::warn!("{key} ({sb}): {e}");
                            if let Some(x) = old.get(&sb) {
                                v.insert(sb.clone(), x.clone());
                            }
                            errs.insert(sb, J::String(e));
                        }
                    }
                }
                let errs = (!errs.is_empty()).then_some(J::Object(errs));
                let errors_changed = self.feats.errors.get(&key) != errs.as_ref();
                match errs {
                    Some(e) => self.feats.errors.insert(key.clone(), e),
                    None => self.feats.errors.remove(&key),
                };
                let v = J::Object(v);
                if v == old {
                    return errors_changed;
                }
                self.feats.data.insert(key.clone(), v.clone());
                self.feats_data_changed(&key, &old, &v);
            }
            LinkEvent::Data { key, result } => match result {
                Ok(v) => {
                    let had_error = self.feats.errors.remove(&key).is_some();
                    if self.feats.data.get(&key) == Some(&v) && !had_error {
                        return false;
                    }
                    let old = self.feats.data.insert(key.clone(), v.clone()).unwrap_or(J::Null);
                    if old != v {
                        self.feats_data_changed(&key, &old, &v);
                    }
                }
                Err(e) => {
                    log::warn!("{key}: {e}");
                    if self.feats.errors.get(&key).and_then(J::as_str) == Some(e.as_str()) {
                        return false;
                    }
                    self.feats.errors.insert(key, J::String(e));
                }
            },
            LinkEvent::Bytes { id: _, feature, call, result } => {
                let Some(f) = Self::feature_of(feature) else { return false };
                for k in &call.then {
                    self.refresh_key(k);
                }
                let Some(on) = f.on_bytes else {
                    self.notify(format!("{feature}: a raw read answered but it has no on_bytes"), true);
                    return true;
                };
                let data = self.feat_ctx_data();
                let acts = on(&Ctx { data: &data }, &call, result);
                self.feature_actions(f, &format!("feature:{feature}"), acts);
            }
            LinkEvent::Answer { id, feature, call, result } => {
                let Some(f) = Self::feature_of(feature) else { return false };
                for k in &call.then {
                    self.refresh_key(k);
                }
                // A prepared dialog's read.
                if self.feats.dialog.as_ref().is_some_and(|d| d.preparing == Some(id)) {
                    let data = self.feat_ctx_data();
                    let d = self.feats.dialog.as_mut().unwrap();
                    d.preparing = None;
                    let p = d.prepare.take().expect("prepare");
                    let built = match result {
                        Ok((code, v)) if (200..300).contains(&code) => (p.build)(&Ctx { data: &data }, &p.asked, &v),
                        Ok((code, v)) => Err(format!("The daemon answered {code}: {}", v["error"].as_str().unwrap_or("no reason given"))),
                        Err(e) => Err(e),
                    };
                    match built {
                        Ok(c) => {
                            d.secrets = secret_edits(&c);
                            d.title = c.title.clone();
                            d.tag = c.call.tag.clone();
                            d.confirm = Some(c);
                        }
                        Err(e) => d.error = Some(e),
                    }
                    return true;
                }
                let from_dialog = self.feats.dialog.as_ref().is_some_and(|d| d.running == Some(id));
                if let Some(on) = f.on_answer {
                    if from_dialog {
                        self.feats.dialog = None;
                    }
                    let data = self.feat_ctx_data();
                    let acts = on(&Ctx { data: &data }, &call, result);
                    self.feature_actions(f, &format!("feature:{}", f.name), acts);
                    return true;
                }
                match result {
                    Ok((code, v)) if (200..300).contains(&code) => {
                        if from_dialog {
                            self.feats.dialog = None;
                        }
                        if let Some(k) = &call.store {
                            self.feats.data.insert(k.clone(), v);
                        }
                        if call.notify {
                            self.notify(format!("{}: done", call.what), false);
                        }
                    }
                    Ok((code, v)) => {
                        let why = v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"));
                        self.fail(from_dialog, &call.what, why);
                    }
                    Err(e) => self.fail(from_dialog, &call.what, e),
                }
            }
            _ => return false,
        }
        true
    }

    /// Tells the feature that declared `key` (its `on_data`).
    fn feats_data_changed(&mut self, key: &str, old: &J, new: &J) {
        for f in feat::FEATURES {
            if let (true, Some(on)) = (f.data.iter().any(|d| d.key == key), f.on_data) {
                let data = self.feat_ctx_data();
                let acts = on(&Ctx { data: &data }, key, old, new);
                self.feature_actions(f, &format!("feature:{}", f.name), acts);
            }
        }
    }

    fn fail(&mut self, from_dialog: bool, what: &str, why: String) {
        if from_dialog {
            if let Some(d) = self.feats.dialog.as_mut() {
                d.running = None;
                d.error = Some(why);
                return;
            }
        }
        self.notify(format!("{what}: {why}"), true);
    }

    // ---- what screens see ----

    /// `data.app.modal`, `data.app.panels`, `data.app.extras`, from `data`.
    pub(super) fn feats_app(&self, data: &J) -> (J, J, J, Vec<Extra>) {
        let modal = match &self.feats.modal {
            Some((s, a)) => json!({ "screen": s, "arg": a }),
            None => J::Null,
        };
        let panels = json!({ "open": self.feats.panels, "shown": self.feats.panel, "split": self.feats.split });
        let mut topbar = vec![];
        let mut sandboxes = Map::new();
        let mut panes = vec![];
        let mut rows = Map::new();
        for f in feat::FEATURES {
            let Some(x) = f.extras else { continue };
            for e in x(&Ctx { data }) {
                match &e.place {
                    Place::Topbar => topbar.push(e.to_json()),
                    Place::Sandbox(n) => sandboxes.entry(n.clone()).or_insert_with(|| J::Array(vec![])).as_array_mut().unwrap().push(e.to_json()),
                    Place::SandboxRow(n) => rows.entry(n.clone()).or_insert_with(|| J::Array(vec![])).as_array_mut().unwrap().push(e.to_json()),
                    Place::Pane(_) | Place::Session { .. } => panes.push(e),
                }
            }
        }
        let _ = &self.feats.errors;
        (modal, panels, json!({ "topbar": topbar, "sandboxes": sandboxes, "rows": rows }), panes)
    }

    /// A press on an extra the core draws (pane bars) or on a notice's offer.
    pub(super) fn press_extra(&mut self, from: &str, name: &str, args: &J) {
        match crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })) {
            Ok(list) => crate::screen::Host::intents(self, from, list),
            Err(e) => self.notify(format!("{from}: {e}"), true),
        }
    }

    // ---- drawing ----

    /// The confirmation, centred over the window.
    pub(super) fn confirm_view(&self) -> Option<El> {
        let d = self.feats.dialog.as_ref()?;
        let w = 520.0f32.min(self.window.0 - 32.0);
        let side = ((self.window.0 - w) / 2.0).max(16.0);
        let top = layout::GUTTER * 2.0 + layout::TOP_BAR_H + 24.0;
        let st = approval::ConfirmState {
            confirm: d.confirm.as_ref(),
            title: &d.title,
            loading: d.preparing.is_some() && d.error.is_none(),
            error: d.error.as_deref(),
            busy: d.running.is_some(),
            acked: d.acked,
            secrets: &d.secrets,
            left: side,
            top,
            right: side,
            max_h: self.window.1 - top - 24.0,
        };
        Some(approval::confirm_view::<App>(&st))
    }

    /// The modal screen over a dimmed window.
    pub(super) fn modal_view(&self, screen: El, width: f32) -> El {
        let w = width.min(self.window.0 - 32.0);
        let side = ((self.window.0 - w) / 2.0).max(16.0);
        let top = layout::GUTTER * 2.0 + layout::TOP_BAR_H + 16.0;
        let card = col()
            .key("modal")
            .role("dialog")
            // Keyboard focus stays inside; Escape closes it.
            .modal(false, |s: &mut App| {
                s.feats.modal = None;
                s.push_data();
            })
            .pin(Some(side), Some(top), Some(side), None)
            // A fixed height: the screen fills it and scrolls inside (its
            // own close button; Escape and a press outside close it too).
            .h((self.window.1 - top - 24.0).clamp(160.0, 720.0))
            .padding(16.0)
            .gap(12.0)
            .radius(radius("lg").unwrap())
            .fill(c("surface"))
            .border(1.0, c("border"))
            .shadow(8.0, 24.0, rune::color::hex("#00000073").unwrap())
            // A press inside stays inside (it does not reach the scrim).
            .on_press(|_s: &mut App| {})
            .on_drop_files(|s: &mut App, files: &[std::path::PathBuf], _tx| {
                if let Some(f) = s.feats.modal.as_ref().and_then(|(m, _)| feat::screen(m)).filter(|(f, _)| f.on_files.is_some()).map(|(f, _)| f.name) {
                    s.feature_incoming(f, feat::Incoming::Files(files.to_vec()));
                }
            })
            .child(col().w_full().min_h(0.0).grow(1.0).child(screen));
        el().key("modal-scrim")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(rune::color::hex("#00000080").unwrap())
            .on_press(|s: &mut App| {
                s.feats.modal = None;
                s.push_data();
            })
            .child(card)
    }

    /// The docked feature panel: tabs when more than one is open, the screen below.
    /// A docked panel: tabs of every open panel, or (side by side) only
    /// `only`'s; the switch between the two when more than one is open.
    pub(super) fn feat_panel_view(&self, screen: El, width: f32, only: Option<&str>) -> El {
        let mut tabs = row().key(match only {
            Some(n) => format!("panel-tabs-{n}"),
            None => "panel-tabs".into(),
        }).w_full().gap(4.0).items_center();
        for name in self.feats.panels.iter().filter(|n| only.is_none_or(|o| o == n.as_str())) {
            let Some((_, decl)) = feat::screen(name) else { continue };
            let Slot::Panel { title, icon, .. } = decl.slot else { continue };
            let shown = self.feats.panel.as_deref() == Some(name.as_str());
            let n1 = name.clone();
            let n2 = name.clone();
            let mut tab = row()
                .key(format!("panel-tab-{name}"))
                .role("tab")
                .label(title)
                .gap(8.0)
                .pl(8.0)
                .pr(2.0)
                .h(layout::CONTROL_H)
                .items_center()
                .radius(radius("sm").unwrap())
                .fill(c(if shown { "elevated" } else { "bg" }))
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut App| {
                    s.feats.panel = Some(n1.clone());
                    s.push_data();
                });
            if let Some(i) = crate::icons::get(icon) {
                tab = tab.child(rune::icon(i, 14.0).color(c(if shown { "text-bright" } else { "text-muted" })));
            }
            tab = tab.child(text(title).font(crate::screen::body(13.0, 500.0)).color(c(if shown { "text-bright" } else { "text-muted" }))).child(
                el().key(format!("panel-close-{name}"))
                    .role("button")
                    .label(format!("Close {title}"))
                    .help(format!("Close {title}"))
                    .w(layout::DENSE_H)
                    .h(layout::DENSE_H)
                    .items_center()
                    .justify_center()
                    .radius(radius("xs").unwrap())
                    .hover(c("border"))
                    .cursor(Cursor::Pointer)
                    .on_press(move |s: &mut App| {
                        s.close_panel(&n2);
                        s.push_data();
                    })
                    .child(rune::icon(crate::icons::get("close").unwrap(), 12.0).color(c("text-muted"))),
            );
            tabs = tabs.child(tab);
        }
        if self.feats.panels.len() > 1 && only.is_none_or(|o| self.feats.panels.first().map(String::as_str) == Some(o)) {
            let split = self.feats.split;
            tabs = tabs.child(el().grow(1.0)).child(
                el().key("panel-split")
                    .role("button")
                    .label(if split { "Show panels as tabs" } else { "Show panels side by side" })
                    .help(if split { "Show panels as tabs" } else { "Show panels side by side" })
                    .w(layout::DENSE_H)
                    .h(layout::DENSE_H)
                    .items_center()
                    .justify_center()
                    .radius(radius("xs").unwrap())
                    .hover(c("border"))
                    .cursor(Cursor::Pointer)
                    .on_press(|s: &mut App| {
                        s.feats.split = !s.feats.split;
                        s.push_data();
                    })
                    .child(rune::icon(crate::icons::get(if split { "single" } else { "columns" }).unwrap(), 14.0).color(c("text-muted"))),
            );
        }
        col()
            .key(match only {
                Some(n) => format!("feature-panel-{n}"),
                None => "feature-panel".into(),
            })
            .role("complementary")
            .w(self.panel_width(width))
            .h_full()
            .min_h(0.0)
            .padding(12.0)
            .gap(12.0)
            .fill(c("bg"))
            .border(1.0, c("elevated"))
            .radius(radius("md").unwrap())
            // Files dropped on the shown panel go to its feature (if it takes them).
            .on_drop_files(|s: &mut App, files: &[std::path::PathBuf], _tx| {
                if let Some(f) = s.files_target() {
                    s.feature_incoming(f, feat::Incoming::Files(files.to_vec()));
                }
            })
            .child(tabs)
            .child(col().w_full().grow(1.0).min_h(0.0).child(screen))
    }

    /// A docked panel's width: what it asks for, narrowed so the panes keep
    /// at least 280 pt (never under 260: a panel narrower than that is not
    /// usable; the window's minimum then squeezes the panes, not the panel).
    pub(super) fn panel_width(&self, asked: f32) -> f32 {
        let shown = if self.feats.split && self.feats.panels.len() > 1 { self.feats.panels.len() } else { 1 } as f32;
        let side = if self.sidebar_shown() { self.sidebar_w() + layout::GUTTER } else { 0.0 };
        let jarvis = if self.jarvis.open { self.jarvis.width + layout::GUTTER } else { 0.0 };
        let room = self.window.0 - side - jarvis - layout::GUTTER * 2.0 - 280.0 - layout::GUTTER * shown;
        if self.window.0 <= 0.0 {
            return asked;
        }
        asked.min(room / shown).max(260.0)
    }

    /// Feature buttons and badges on the bar of pane `i`.
    /// The extras of a pane bar showing that session (`Pane(sandbox)`, and
    /// `Session` of that thread), as the bar lays them out (bar.rs).
    pub(super) fn pane_items(&self, sandbox: &str, thread: Option<&str>) -> Vec<super::bar::Item> {
        self.feats
            .pane_extras
            .iter()
            .filter(|e| match &e.place {
                Place::Pane(s) => s == sandbox,
                Place::Session { sandbox: s, thread: t } => s == sandbox && t.as_deref() == thread,
                _ => false,
            })
            .map(super::bar::Item::from_extra)
            .collect()
    }
}

/// A call's body may hold a feature's `core_only` fields only when the
/// person confirmed it on the core's dialog.
pub fn core_only_check(f: &Feature, call: &Call, confirmed: bool) -> Result<(), String> {
    if confirmed {
        return Ok(());
    }
    match f.core_only.iter().find(|k| call.body.get(**k).is_some()) {
        Some(k) => Err(format!("{}: '{k}' is set only by your Confirm on the app's own dialog: refused", call.what)),
        None => Ok(()),
    }
}

fn secret_edits(c: &Confirm) -> Vec<TextEdit> {
    c.secrets.iter().map(|_| TextEdit::new("").secure()).collect()
}

impl ConfirmAnswers for App {
    fn confirm_click(&mut self) {
        let Some(d) = self.feats.dialog.as_ref() else { return };
        let st = approval::ConfirmState { confirm: d.confirm.as_ref(), title: "", loading: false, error: None, busy: d.running.is_some(), acked: d.acked, secrets: &d.secrets, left: 0.0, top: 0.0, right: 0.0, max_h: 0.0 };
        if !st.can_confirm() {
            return;
        }
        let Some(f) = Self::feature_of(d.feature) else { return };
        let confirm = d.confirm.clone().expect("loaded");
        let mut call = confirm.call.clone();
        // The secrets go into the body here, once; the edits are wiped below.
        for (i, s) in confirm.secrets.iter().enumerate() {
            let v = d.secrets[i].text.clone();
            if v.is_empty() && s.optional {
                continue;
            }
            if let Err(e) = feat::put_at(&mut call.body, &s.field, J::String(v)) {
                if let Some(d) = self.feats.dialog.as_mut() {
                    d.error = Some(e);
                }
                return;
            }
        }
        for g in &confirm.grants {
            if let Err(e) = feat::put_at(&mut call.body, &g.field, g.value()) {
                if let Some(d) = self.feats.dialog.as_mut() {
                    d.error = Some(e);
                }
                return;
            }
        }
        log::info!("{}: confirmed '{}' ({:?} {})", f.name, confirm.title, call.method, call.path);
        let id = self.feature_call(f, call, true);
        if let Some(d) = self.feats.dialog.as_mut() {
            for e in &mut d.secrets {
                e.replace_text(String::new());
            }
            if id == 0 {
                d.error = d.error.take().or_else(|| Some(format!("Not connected: {}", self.connection.text)));
            } else {
                d.running = Some(id);
                d.error = None;
            }
        }
        for ev in std::mem::take(&mut self.feats.core_done) {
            self.feats_link(ev);
        }
        self.push_data();
    }

    fn cancel_click(&mut self) {
        self.dismiss_dialog();
        self.push_data();
    }

    fn toggle_confirm_ack(&mut self) {
        if let Some(d) = self.feats.dialog.as_mut() {
            d.acked = !d.acked;
        }
    }

    fn secret_edit(&mut self, i: usize) -> &mut TextEdit {
        let d = self.feats.dialog.as_mut().expect("dialog open");
        &mut d.secrets[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core's file operations keep their rule whatever a feature asks:
    /// open and upload only from the core's dialog, a download that picks
    /// its place or opens likewise; dialogs and clipboard only from a press.
    #[test]
    fn file_operations_and_dialogs_keep_the_cores_rules() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/network/basic.json");
        let mut a = App::new(super::super::Args { fixture: Some(fixture.into()), ..Default::default() });
        a.drain(Some(Duration::from_secs(3)), true);
        let f = App::feature_of("network").unwrap();
        let last = |a: &App| a.notice.as_ref().map(|n| n.0.clone()).unwrap_or_default();
        for (op, body) in [("open", json!({ "path": "/etc/passwd" })), ("upload", json!({ "route": "/api/sandboxes/web-app/upload?name=x", "file": "/etc/passwd", "tag": "u" }))] {
            assert_eq!(a.feature_call(f, feat::core_call(op, body, op, ""), false), 0, "{op}");
            assert!(last(&a).contains("runs only from the app's own dialog"), "{op}: {}", last(&a));
        }
        let dl = |to, after| feat::Download { get: "/api/x".into(), name: "../r.md".into(), to, after, tag: "t".into() };
        a.feature_actions(f, "network", vec![Action::Download(dl(feat::Dest::Ask, feat::After::Nothing))]);
        assert!(last(&a).contains("refused"), "{}", last(&a));
        a.feature_actions(f, "network", vec![Action::Download(dl(feat::Dest::Downloads, feat::After::Open))]);
        assert!(last(&a).contains("refused"));
        assert!(dl(feat::Dest::Downloads, feat::After::Reveal).direct());
        // A save dialog, as the call of a confirmed dialog, waits for the window.
        let id = a.feature_call(f, dl(feat::Dest::Ask, feat::After::Nothing).call("save"), true);
        assert!(id > 0 && a.feats.asks.iter().any(|(_, k)| matches!(k, Ask::SaveAs { .. })));
        a.feats.asks.clear();
        // The open dialog and the clipboard: from a press only, never from the feature's own chain.
        a.feature_actions(f, "feature:network", vec![Action::PickFiles { title: "x".into(), extensions: vec![], multiple: false }, Action::ReadClipboard]);
        assert!(a.feats.asks.is_empty() && last(&a).contains("only from your own press"));
        a.feature_actions(f, "feature:network", vec![Action::Download(dl(feat::Dest::Downloads, feat::After::Nothing))]);
        assert!(last(&a).contains("only from your own press"), "{}", last(&a));
        a.feature_actions(f, "network", vec![Action::PickFiles { title: "x".into(), extensions: vec![], multiple: true }]);
        assert!(matches!(a.feats.asks.as_slice(), [(_, Ask::PickFiles { .. })]) && last(&a).contains("one file at a time"));
        // An upload of a path the person never handed in: refused by the core, even confirmed.
        let r = crate::files::upload(&crate::client::FixtureDaemon::new("t", Default::default()), "/api/sandboxes/web-app/upload", std::path::Path::new("/etc/hostname"), "text/plain", "u", None);
        assert!(r.unwrap_err().contains("not a file you handed in"));
        // A dialog keeps its call's tag for `on_dismiss`; a newer one replaces it.
        let c = |t: &str| Confirm::new("t", Call::post("/api/x", json!({}), "x").tag(t)).into_action();
        a.feature_actions(f, "network", vec![c("first")]);
        a.feature_actions(f, "network", vec![c("second")]);
        assert_eq!(a.feats.dialog.as_ref().map(|d| d.tag.as_str()), Some("second"));
        assert!(a.dismiss_dialog() && a.feats.dialog.is_none() && !a.dismiss_dialog());
    }

    #[test]
    fn an_approval_intent_only_ever_opens_the_core_dialog() {
        let call = Call::post("/api/x", json!({}), "x");
        let risk = Risk::Approval("Allow");
        // Straight to the daemon: refused.
        assert!(admit(risk, "allow", vec![Action::Call(call.clone())]).is_err());
        // A dialog plus a call beside it: refused.
        assert!(admit(risk, "allow", vec![Confirm::new("t", call.clone()).into_action(), Action::Call(call.clone())]).is_err());
        // An intent run on its behalf: refused (it could be anything).
        assert!(admit(risk, "allow", vec![Action::Intent { name: "x".into(), args: json!({}) }]).is_err());
        assert!(admit(risk, "allow", vec![]).is_err());
        assert!(admit(risk, "allow", vec![Confirm::new("t", call.clone()).into_action()]).is_ok());
        assert!(admit(Risk::Direct, "deny", vec![Action::Call(call)]).is_ok());
    }

    /// The rule end to end, through the app: an allow a screen emits opens
    /// the core's dialog and sends nothing; only Confirm there sends it.
    #[test]
    fn a_screen_emitted_allow_never_runs_without_the_core_dialog() {
        use crate::screen::Host;
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/network/basic.json");
        let mut a = App::new(super::super::Args { fixture: Some(fixture.into()), ..Default::default() });
        let raw = |a: &App| match &a.target {
            crate::client::Target::Fixture(f) => f.raw(),
            _ => unreachable!(),
        };
        let pending = |a: &App| raw(a)["approvals"]["approvals"].as_array().map(Vec::len).unwrap_or(0);
        a.drain(Some(Duration::from_secs(3)), true);
        assert_eq!(pending(&a), 1);
        // The screen asks to allow, three ways: the request, a blocked host, a typed rule.
        let ask = |a: &mut App, v: J| {
            let list = crate::intent::parse(&v).unwrap();
            a.intents("network", list);
            a.drain(Some(Duration::from_secs(3)), true);
        };
        ask(&mut a, json!({ "kind": "intent", "name": "approval_answer", "args": { "id": "network:abc", "option": "allow" } }));
        assert_eq!(pending(&a), 1, "nothing sent before the person's click");
        let d = a.feats.dialog.as_ref().expect("the core's dialog shows");
        assert!(d.confirm.as_ref().unwrap().lines.iter().any(|l| l == "Asks for: api.github.com:443"));
        let rules = |a: &App| raw(a)["policy"]["web-app"]["rules"]["rows"].as_array().map(Vec::len).unwrap_or(0);
        let before = rules(&a);
        a.cancel_click();
        ask(&mut a, json!({ "kind": "intent", "name": "network_allow", "args": { "sandbox": "web-app", "resources": "evil.com", "global": true } }));
        assert!(a.feats.dialog.as_ref().unwrap().confirm.as_ref().unwrap().danger);
        assert_eq!(rules(&a), before, "a global allow waits for the dialog");
        a.cancel_click();
        assert_eq!((rules(&a), pending(&a)), (before, 1), "cancel sends nothing");
        // Only the click on Confirm answers.
        ask(&mut a, json!({ "kind": "intent", "name": "approval_answer", "args": { "id": "network:abc", "option": "allow" } }));
        a.confirm_click();
        a.drain(Some(Duration::from_secs(3)), true);
        assert_eq!(pending(&a), 0);
        assert!(a.feats.dialog.is_none(), "closed once answered");
    }

    #[test]
    fn core_only_fields_go_out_only_after_a_confirm() {
        const F: Feature = Feature { name: "t", core_only: &["approved"], ..Feature::EMPTY };
        let sneaky = Call::post("/api/sandboxes/create", json!({ "name": "x", "approved": true }), "create x");
        assert!(core_only_check(&F, &sneaky, false).is_err(), "a feature (or a screen's args copied into the body) cannot send it");
        assert!(core_only_check(&F, &Call::post("/api/sandboxes/create", json!({ "name": "x" }), "create x"), false).is_ok());
        // On Confirm the core adds it itself; the feature cannot preset it.
        let c = Confirm::new("Create x?", Call::post("/api/sandboxes/create", json!({ "name": "x" }), "create x")).grant("approved");
        let mut body = c.call.body.clone();
        for g in &c.grants {
            feat::put_at(&mut body, &g.field, g.value()).unwrap();
        }
        assert_eq!(body["approved"], true);
        // Lines: what the dialog lists is exactly what is sent.
        let lines = vec!["network: allow api.github.com".to_string(), "mount: ~/data (read only)".to_string()];
        let c = Confirm::new("Create x?", Call::post("/api/sandboxes/create", json!({ "name": "x" }), "create x")).grant_lines("approved", lines.clone());
        let shown = approval::granted_lines(&c);
        assert_eq!(shown, lines);
        let mut body = c.call.body.clone();
        for g in &c.grants {
            feat::put_at(&mut body, &g.field, g.value()).unwrap();
        }
        assert_eq!(body["approved"], json!(shown));
        let preset = Confirm::new("t", sneaky.clone()).grant("approved");
        let mut b2 = preset.call.body.clone();
        assert!(feat::put_at(&mut b2, "approved", J::Bool(true)).is_err());
    }
}
