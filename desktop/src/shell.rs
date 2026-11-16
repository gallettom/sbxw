//! The window: the web UI's layout (top bar, sandbox side panel, a pane
//! area of terminals) around two Luau screens, `topbar` and `sandboxes`.
//! The core owns the panes, the daemon link and every intent; screens only
//! draw and ask.
//!
//! Layout, as `assets/app.css` draws it: a dark floor (`#080b0f`), and on it,
//! 6 pt apart, rounded tiles: the header (48 pt), the sidebar, and one tile
//! per pane, each with a 32 pt bar over its terminal.

mod features;
mod jarvis;
pub mod store;

use crate::approval;
use crate::client::{Daemon, FixtureDaemon, Link, LinkEvent, Sandbox, Target, TerminalRequest, TokenSource};
use crate::fixture::{AppView, Connection, Fixture, LayoutView, PaneView};
use crate::intent::{Intent, Mode, PaneTarget, Risk, Split, ThreadStart};
use crate::screen::{self, Host, ScreenState};
use crate::terminal::{self, TerminalEvent, TerminalState};
use crate::theme::{self, c, layout};
use rune::{col, el, row, text, Cursor, Cx, El, Key, KeyEvent, TextStyle, Tx, Waker};
use serde_json::Value as J;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The screens the shell shows, in the order they are loaded.
pub const SCREENS: &[&str] = &["topbar", "sandboxes", "jarvis"];

/// Command-line options (what Rune did not take as its own flags).
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub daemon: Option<String>,
    pub token: Option<String>,
    pub token_file: Option<PathBuf>,
    pub fixture: Option<String>,
    /// Open a terminal on this sandbox at start (scripts).
    pub sandbox: Option<String>,
    pub mode: Option<String>,
    /// Draw only this screen, filling the window.
    pub solo: Option<String>,
    pub screens_dir: Option<PathBuf>,
    pub dump_text: Option<PathBuf>,
}

impl Args {
    pub fn parse(extra: &[String]) -> Args {
        let mut a = Args::default();
        let mut it = extra.iter();
        while let Some(k) = it.next() {
            let mut v = || it.next().cloned().unwrap_or_default();
            match k.as_str() {
                "--daemon" => a.daemon = Some(v()),
                "--token" => a.token = Some(v()),
                "--token-file" => a.token_file = Some(v().into()),
                "--fixture" => a.fixture = Some(v()),
                "--sandbox" => a.sandbox = Some(v()),
                "--mode" => a.mode = Some(v()),
                "--solo" => a.solo = Some(v()),
                "--screens-dir" => a.screens_dir = Some(v().into()),
                "--dump-text" => a.dump_text = Some(v().into()),
                _ => eprintln!("unknown argument {k}"),
            }
        }
        a
    }
}

pub struct Pane {
    pub sandbox: Option<String>,
    pub mode: Mode,
    pub thread: Option<String>,
    pub term: Option<TerminalState>,
    pub error: Option<String>,
    /// The program's title (OSC 0/2), shown on the pane bar.
    pub title: Option<String>,
    /// The daemon's "the session wants you" frame, until a key in this pane.
    pub attention: bool,
    /// Lost the socket; the terminal is reconnecting on its own.
    pub reconnecting: Option<String>,
    /// When it was last opened or focused (the app's own counter): the
    /// least recently used pane is the one a new sandbox may replace when no
    /// pane is free and the layout is full.
    pub used: u64,
}

impl Pane {
    fn empty() -> Pane {
        Pane { sandbox: None, mode: Mode::Agent, thread: None, term: None, error: None, title: None, attention: false, reconnecting: None, used: 0 }
    }

    fn status(&self) -> String {
        match (&self.sandbox, &self.term, &self.error) {
            (None, _, _) => "empty".into(),
            (_, _, Some(_)) => "error".into(),
            (_, Some(_), _) if self.reconnecting.is_some() => "reconnecting".into(),
            (_, Some(t), _) if t.status.starts_with("closed") => "closed".into(),
            (_, Some(t), _) => t.status.clone(),
            (Some(_), None, None) => "closed".into(),
        }
    }

}

/// Finds pane `i`'s terminal in the app (the terminal's handlers take one per pane).
pub fn lens(i: usize) -> impl Fn(&mut App) -> Option<&mut TerminalState> + Clone + 'static {
    move |s: &mut App| s.panes.get_mut(i).and_then(|p| p.term.as_mut())
}

/// Headless script marks the app acts on.
enum Mark {
    /// `hold:<ms>`: wait that long in real time, taking in what arrives (the
    /// virtual clock does not wait for the daemon or a file edit).
    Hold(u64),
    /// `screen:<name>=<path>`: load that file as that screen now.
    Screen(String, PathBuf),
}

/// After opening the agent of a sandbox that is not running, the list is read
/// again at these delays until it shows as running (`RUNNING_RECHECK_MS` in
/// `assets/js/sandboxes.js`).
const RUNNING_RECHECK: &[u64] = &[1500, 3000, 6000, 12000, 24000];

pub struct App {
    target: Target,
    pub link: Option<Link>,
    pub daemon: Option<Arc<dyn Daemon>>,
    pub connection: Connection,
    pub sandboxes: Vec<Sandbox>,
    /// A list arrived since the last connection.
    listed: bool,
    /// A list arrived at least once (panes of a sandbox that leaves it empty).
    listed_once: bool,
    /// The fixture set the connection state: the link does not change it.
    fixed_connection: bool,
    /// The fixture's daemon data (keys of later waves), when there is one.
    raw: J,
    pub screens: Vec<ScreenState>,
    pub panes: Vec<Pane>,
    pub focused: usize,
    pub layout: (usize, Split),
    /// The pane area's splits: `(ratio, length)` by id.
    pub splits: HashMap<String, (f32, f32)>,
    pub selected: Option<String>,
    /// A line at the bottom right: text, is an error, until.
    pub notice: Option<(String, bool, Instant)>,
    pub approvals: Vec<approval::Pending>,
    /// An OSC 52 copy too long to make unasked, waiting for a click (approval.rs).
    pub copy_offer: Option<approval::CopyOffer>,
    /// Sandboxes whose start is being watched: name, next recheck step.
    expect: Vec<(String, usize, Instant)>,
    marks: Vec<(f64, Mark)>,
    dump_text: Option<PathBuf>,
    solo: Option<String>,
    start: Option<(String, Mode)>,
    waker: Option<Waker>,
    /// The link's way to wake the UI, set once `setup` has the waker.
    wake_slot: Arc<std::sync::Mutex<Option<Waker>>>,
    /// The daemon now, for terminals reconnecting on their own (a restarted
    /// daemon has a new token, so an old `HttpDaemon` would be refused).
    current: Arc<std::sync::Mutex<Option<Arc<dyn Daemon>>>>,
    /// Whether the window has the keyboard (`on_window_focus`).
    pub window_focused: bool,
    /// The core moved the focused pane: move Rune's keyboard focus there next poll.
    refocus: bool,
    /// Jarvis: the conversation, its proposals, the panel (jarvis.rs).
    pub jarvis: crate::jarvis::Jarvis,
    /// Native audio (`audio::Audio` in a window; `NullAudio` headless and in fixtures).
    pub audio: Box<dyn crate::voice::AudioIo>,
    wiring: jarvis::Wiring,
    /// The window's size in points (the root's layout).
    pub window: (f32, f32),
    /// Features (`src/feat/`): their data, slots and dialog (shell/features.rs).
    pub feats: features::Feats,
    /// The counter behind [`Pane::used`].
    use_seq: u64,
    /// The app's state across runs (store.rs).
    pub store: store::Store,
}

/// The most panes the layout shows (the layout switch's largest choice).
pub const MAX_PANES: usize = 4;

/// Whether the window takes pastes at its root (see `view`).
pub fn window_paste() -> bool {
    cfg!(target_os = "macos") || headless()
}

pub fn headless() -> bool {
    rune::tooling::headless_run().is_some()
}

impl App {
    pub fn new(a: Args) -> App {
        let (target, raw, init) = match &a.fixture {
            Some(f) => {
                let path = if f.contains('/') && !f.ends_with(".json") {
                    let (scr, name) = f.split_once('/').unwrap();
                    Fixture::resolve(scr, name)
                } else {
                    PathBuf::from(f)
                };
                match Fixture::load(&path) {
                    Ok(fx) => (Target::Fixture(Arc::new(FixtureDaemon::new(&fx.name, fx.daemon.clone()).with_raw(fx.raw.clone()))), fx.raw.clone(), fx.app.clone()),
                    Err(e) => {
                        eprintln!("{e:#}");
                        std::process::exit(2);
                    }
                }
            }
            None => {
                let addr = a.daemon.clone().or_else(|| std::env::var("SBXW_DAEMON").ok().filter(|s| !s.is_empty())).unwrap_or_else(|| crate::client::DEFAULT_ADDR.into());
                let token = match (&a.token, &a.token_file) {
                    (Some(t), _) => TokenSource::Fixed(t.clone()),
                    (None, Some(p)) => TokenSource::File(p.clone()),
                    (None, None) => TokenSource::File(TokenSource::default_file()),
                };
                (Target::Http { addr, token }, J::Null, Default::default())
            }
        };
        let dir = a.screens_dir.clone().unwrap_or_else(screen::override_dir);
        log::info!("screen overrides: {}", dir.display());
        // Jarvis writes its screen changes to ~/.sbxw/desktop/screens: say when this run reads elsewhere.
        let elsewhere = a.screens_dir.is_some() || std::env::var_os("SBXW_DESKTOP_SCREENS").is_some_and(|d| !d.is_empty());
        let mut app = App {
            target,
            link: None,
            daemon: None,
            connection: Connection::default(),
            sandboxes: vec![],
            listed: false,
            listed_once: false,
            fixed_connection: false,
            raw,
            screens: vec![],
            panes: vec![Pane::empty()],
            focused: 0,
            layout: (1, Split::Columns),
            splits: HashMap::new(),
            selected: init.selected.clone(),
            notice: None,
            approvals: vec![],
            copy_offer: None,
            expect: vec![],
            marks: vec![],
            dump_text: a.dump_text.clone(),
            solo: a.solo.clone(),
            start: a.sandbox.clone().map(|s| (s, if a.mode.as_deref() == Some("agent") { Mode::Agent } else { Mode::Bash })),
            waker: None,
            wake_slot: Arc::new(std::sync::Mutex::new(None)),
            current: Arc::new(std::sync::Mutex::new(None)),
            window_focused: true,
            refocus: true,
            jarvis: crate::jarvis::Jarvis::default(),
            audio: Box::new(crate::voice::NullAudio::default()),
            wiring: jarvis::Wiring::new(),
            window: (1280.0, 800.0),
            feats: features::Feats::default(),
            use_seq: 0,
            store: store::Store::open(),
        };
        jarvis::load_prefs(&mut app.jarvis);
        if init.jarvis_open {
            app.jarvis.open = true;
        }
        if init.fake_audio || std::env::var_os("SBXW_DESKTOP_FAKE_AUDIO").is_some() {
            app.audio = Box::new(crate::voice::FakeAudio { capture: vec![0u8; 16_000 * 4], ..Default::default() });
        }
        for (who, text) in &init.jarvis_log {
            let w = match who.as_str() {
                "me" => crate::jarvis::Who::Me,
                "note" => crate::jarvis::Who::Note,
                "error" => crate::jarvis::Who::Error,
                _ => crate::jarvis::Who::Jarvis,
            };
            app.jarvis.say(w, text.clone());
        }
        // What the app has built in, for the daemon to show Jarvis (builtin.rs).
        let export = std::env::var_os("SBXW_DESKTOP_BUILTIN").map(PathBuf::from).or_else(|| (!headless() && a.fixture.is_none()).then(crate::builtin::default_dir));
        if let Some(dir) = export {
            match crate::builtin::export(&dir) {
                Ok(()) => log::info!("built-in screens written to {}", dir.display()),
                Err(e) => log::warn!("could not write {}: {e}", dir.display()),
            }
        }
        for p in &init.panels {
            app.feats.panels.push(p.clone());
            app.feats.panel = Some(p.clone());
        }
        app.feats.modal = init.modal.clone();
        if let Some(n) = init.panes {
            app.layout.0 = n.clamp(1, 4);
        }
        if init.split.as_deref() == Some("rows") {
            app.layout.1 = Split::Rows;
        }
        for (i, sb) in init.open.iter().enumerate().take(4) {
            app.ensure_panes(i + 1);
            app.panes[i].sandbox = Some(sb.clone());
        }
        if let Some(f) = init.focused {
            app.focused = f.saturating_sub(1).min(3);
        }
        app.ensure_panes(app.layout.0);
        if let Target::Fixture(f) = &app.target {
            app.sandboxes = f.sandboxes().unwrap_or_default();
            app.connection = Connection { state: "up", text: f.describe() };
            if let Some(ci) = &init.connection {
                let state = match ci.state.as_str() {
                    "down" => "down",
                    "connecting" => "connecting",
                    _ => "up",
                };
                app.connection = Connection { state, text: ci.text.clone() };
                app.fixed_connection = true;
            }
        }
        let names: Vec<String> = match &app.solo {
            Some(s) => vec![s.clone()],
            None => SCREENS.iter().map(|s| s.to_string()).chain(crate::feat::screens().map(|(n, _)| n.to_string())).collect(),
        };
        // The link starts now, not in `setup`: Rune checks a script's element
        // keys against the first frame, so in a headless run the first list
        // must be in before it. Its waker is filled in by `setup`.
        let slot = app.wake_slot.clone();
        app.link = Some(Link::start(app.target.clone(), Arc::new(move || {
            if let Some(w) = slot.lock().unwrap().as_ref() {
                w();
            }
        })));
        if headless() {
            app.drain(Some(Duration::from_millis(1500)), true);
        }
        let data = app.screen_data();
        app.screens = names.iter().map(|n| ScreenState::new(n, &dir, data.clone())).collect();
        app.marks = rune::tooling::headless_run()
            .map(|r| {
                r.marks()
                    .filter_map(|(n, t)| {
                        if let Some(ms) = n.strip_prefix("hold:") {
                            return ms.parse().ok().map(|ms| (t, Mark::Hold(ms)));
                        }
                        let (name, path) = n.strip_prefix("screen:")?.split_once('=')?;
                        Some((t, Mark::Screen(name.into(), path.into())))
                    })
                    .collect()
            })
            .unwrap_or_default();
        app.marks.sort_by(|a, b| a.0.total_cmp(&b.0));
        if elsewhere {
            let line = format!("Screens are read from {}: Jarvis's screen changes go to ~/.sbxw/desktop/screens instead", dir.display());
            log::info!("{line}");
            if !headless() {
                app.notify(line, false);
            }
        }
        // After that line: an old override matters more.
        app.check_overrides();
        if let Target::Fixture(f) = &app.target {
            // A fixture answers at once: no thread, so the first frame has it all.
            let d: Arc<dyn Daemon> = f.clone();
            app.daemon.get_or_insert(d.clone());
            let v = d.voice_status().ok();
            app.set_voice(v);
            if let Ok(st) = d.jarvis_status() {
                app.jarvis.status = st["state"].as_str().unwrap_or("unknown").to_string();
                app.jarvis.status_error = st["error"].as_str().map(str::to_string);
            }
            let list = d.jarvis_proposals().unwrap_or_default();
            app.jarvis.seed_proposals(list);
            app.jarvis.seed_sessions(&d.sessions().unwrap_or_default());
            let data = app.screen_data();
            for s in &mut app.screens {
                s.set_data(data.clone());
            }
        }
        app
    }

    fn ensure_panes(&mut self, n: usize) {
        while self.panes.len() < n {
            self.panes.push(Pane::empty());
        }
    }

    pub fn setup(&mut self, waker: Waker) {
        for s in &mut self.screens {
            s.watch(waker.clone());
        }
        *self.wake_slot.lock().unwrap() = Some(waker.clone());
        // Native audio (audio.rs) in a real window on a real daemon; headless
        // runs and fixtures keep NullAudio (or FakeAudio for scripts).
        let real = !headless() && !matches!(self.target, Target::Fixture(_)) && !self.audio.available() && std::env::var_os("SBXW_DESKTOP_NO_AUDIO").is_none();
        if real {
            let w = waker.clone();
            let factory: crate::voice::AudioFactory = crate::audio::Audio::boxed;
            self.audio = factory(Arc::new(move || w()));
            log::info!("audio: native");
        }
        self.waker = Some(waker);
        if let Some((sb, mode)) = self.start.take() {
            self.open(0, &sb, mode, None);
        }
    }

    /// The core's state for the screens.
    pub fn view_model(&self) -> AppView {
        AppView {
            connection: self.connection.clone(),
            selected: self.selected.clone(),
            focused: self.focused + 1,
            layout: LayoutView { panes: self.layout.0, split: if self.layout.1 == Split::Rows { "rows" } else { "columns" } },
            panes: self
                .panes
                .iter()
                .take(self.layout.0)
                .enumerate()
                .map(|(i, p)| PaneView { index: i + 1, sandbox: p.sandbox.clone(), mode: p.mode.name(), thread: p.thread.clone(), focused: i == self.focused, status: p.status() })
                .collect(),
            platform: std::env::consts::OS,
        }
    }

    pub fn screen_data(&self) -> J {
        let mut daemon = match &self.raw {
            J::Object(o) => o.clone(),
            _ => serde_json::Map::new(),
        };
        daemon.insert("sandboxes".into(), serde_json::to_value(&self.sandboxes).expect("sandboxes"));
        daemon.insert("jarvis".into(), self.jarvis.screen_data(self.audio.available()));
        for (k, v) in &self.feats.data {
            daemon.insert(k.clone(), v.clone());
        }
        let mut data = crate::fixture::screen_data(&J::Object(daemon), &self.view_model());
        let (modal, panels, extras, _) = self.feats_app(&data);
        data["app"]["modal"] = modal;
        data["app"]["panels"] = panels;
        data["app"]["extras"] = extras;
        data["app"]["errors"] = J::Object(self.feats.errors.clone());
        data["app"]["overrides"] = self.overrides_view();
        data["app"]["kept"] = self.store.get("features").clone();
        data
    }

    /// `data.app.overrides`: each screen running an override, `{path, stale}`.
    fn overrides_view(&self) -> J {
        let mut o = serde_json::Map::new();
        for sc in self.screens.iter().filter(|sc| sc.override_hash.is_some()) {
            o.insert(sc.name.clone(), serde_json::json!({ "path": sc.override_file(), "stale": sc.stale }));
        }
        J::Object(o)
    }

    /// Finds overrides made for an older built-in screen (phase 3 bug: an
    /// old `sandboxes.luau` copy hid the new row extras, with no word).
    ///
    /// The state file keeps, per override, the hash of the built-in that was
    /// current when the override appeared or last changed. An override is
    /// stale when the built-in compiled in now has another hash. An override
    /// with no record yet (made before this check existed) is stale when its
    /// file is older than the running app: it was made for an earlier build.
    /// The built-in is never read back from `builtin/` on disk.
    pub(crate) fn check_overrides(&mut self) {
        let exe = std::env::current_exe().and_then(std::fs::metadata).and_then(|m| m.modified()).ok();
        let mut newly = vec![];
        for i in 0..self.screens.len() {
            let sc = &self.screens[i];
            let name = sc.name.clone();
            let bh = store::hash(screen::builtin(&name).unwrap_or(""));
            let rec = self.store.get("overrides")[&name].clone();
            let stale = match sc.override_hash.clone() {
                None => {
                    if !rec.is_null() {
                        self.store.set_in("overrides", &name, J::Null);
                    }
                    false
                }
                Some(oh) if rec["override"] != oh.as_str() => {
                    let file = match &sc.source {
                        screen::Source::File(p) => p.clone(),
                        screen::Source::Builtin => sc.override_file().to_path_buf(),
                    };
                    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
                    let older = rec.is_null() && matches!((mtime, exe), (Some(m), Some(e)) if m < e);
                    let b = if older { "older than this app".to_string() } else { bh.clone() };
                    self.store.set_in("overrides", &name, serde_json::json!({ "builtin": b, "override": oh }));
                    older
                }
                Some(_) => rec["builtin"] != bh.as_str(),
            };
            if stale && !self.screens[i].stale {
                newly.push(name.clone());
            }
            self.screens[i].stale = stale;
        }
        if let Some(first) = newly.first() {
            let text = match newly.as_slice() {
                [one] => format!("Your {one} screen is a customised copy made for an older version; new features may be missing. Ask Jarvis to update it, or restore the built-in."),
                many => format!("Your {} screens are customised copies made for an older version; new features may be missing. Ask Jarvis to update them, or restore the built-ins.", many.join(", ")),
            };
            self.notify(text, false);
            self.feats.notice_then = Some(("screen_restore".into(), serde_json::json!({ "screen": first })));
        }
    }

    fn push_data(&mut self) {
        let d = self.screen_data();
        self.feats.pane_extras = self.feats_app(&d).3;
        for s in &mut self.screens {
            s.set_data(d.clone());
        }
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            log::warn!("{text}");
        } else {
            log::info!("{text}");
        }
        self.notice = Some((text, error, Instant::now() + Duration::from_secs(if error { 10 } else { 4 })));
    }

    /// Takes what the link sent. Headless (`wait`), first waits up to `wait`
    /// while the first connection or list is on its way, or (`pending`) while
    /// a background job runs: the virtual clock does not wait for the daemon.
    fn drain(&mut self, wait: Option<Duration>, pending: bool) -> bool {
        let mut changed = false;
        let deadline = wait.map(|w| Instant::now() + w);
        loop {
            let Some(l) = &self.link else { break };
            if let Some(ev) = l.try_recv() {
                changed |= self.on_link(ev);
                continue;
            }
            let busy = pending && l.pending.load(std::sync::atomic::Ordering::SeqCst) > 0;
            let waiting = busy || self.connection.state == "connecting" || (self.connection.state == "up" && !self.listed);
            match deadline {
                Some(d) if waiting && Instant::now() < d => {
                    if let Some(ev) = l.recv_timeout(Duration::from_millis(20)) {
                        changed |= self.on_link(ev);
                    }
                }
                _ => break,
            }
        }
        changed
    }

    fn on_link(&mut self, ev: LinkEvent) -> bool {
        match ev {
            LinkEvent::Up(d) => {
                self.listed = false;
                if !self.fixed_connection {
                    self.connection = Connection { state: "up", text: d.describe() };
                }
                *self.current.lock().unwrap() = Some(d.clone());
                self.daemon = Some(d);
                if !matches!(self.target, Target::Fixture(_)) {
                    self.jarvis_connected();
                }
                self.feats_connected();
                // Terminals whose socket closed reconnect on their own
                // (`TerminalState::with_reconnect`, through `current`), and the
                // daemon replays what they missed.
            }
            LinkEvent::Down(why) => {
                self.connection = Connection { state: "down", text: why };
                self.daemon = None;
                *self.current.lock().unwrap() = None;
            }
            LinkEvent::Sandboxes(l) => {
                self.listed = true;
                if l == self.sandboxes {
                    self.listed_once = true;
                    return false;
                }
                let was: HashMap<String, String> = self.sandboxes.iter().map(|s| (s.name.clone(), s.status.clone())).collect();
                for sb in &l {
                    if was.get(&sb.name).is_some_and(|w| *w != sb.status) {
                        log::info!("list: {} is now {}", sb.name, sb.status);
                    }
                }
                // A sandbox gone from the list (removed): its panes empty, as the web UI does.
                if self.listed_once {
                    for p in &mut self.panes {
                        if p.sandbox.as_ref().is_some_and(|n| !l.iter().any(|s| &s.name == n)) {
                            *p = Pane::empty();
                        }
                    }
                }
                self.listed_once = true;
                self.sandboxes = l;
                // A watched start that now shows as running is done.
                let running: Vec<String> = self.sandboxes.iter().filter(|s| s.status == "running").map(|s| s.name.clone()).collect();
                self.expect.retain(|(n, _, _)| !running.contains(n));
                self.feats_sandboxes();
            }
            LinkEvent::Bus(ev) => {
                log::debug!("event {} {}", ev.name, ev.data);
                let f = self.feats_bus(&ev.name, &ev.data);
                return self.jarvis_bus(&ev.name, &ev.data) || f;
            }
            LinkEvent::Done { what, result } => match result {
                Ok(()) => self.notify(format!("{what}: done"), false),
                Err(e) => self.notify(format!("{what}: {e}"), true),
            },
            ev @ (LinkEvent::Answer { .. } | LinkEvent::Data { .. } | LinkEvent::DataPer { .. }) => return self.feats_link(ev),
        }
        true
    }

    pub fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        if !self.feats.asks.is_empty() {
            self.run_asks(tx);
            changed = true;
        }
        while self.marks.first().is_some_and(|(t, _)| *t <= clock) {
            match self.marks.remove(0).1 {
                Mark::Hold(ms) => {
                    log::info!("hold {ms} ms at {clock:.2}s");
                    let until = Instant::now() + Duration::from_millis(ms);
                    while Instant::now() < until {
                        changed |= self.drain(None, false);
                        for s in &mut self.screens {
                            changed |= s.poll();
                        }
                        // Terminals too: output, and their own reconnect (real time).
                        for p in &mut self.panes {
                            if let Some(t) = &mut p.term {
                                changed |= t.poll(None);
                            }
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                Mark::Screen(name, path) => {
                    if let Some(s) = self.screens.iter_mut().find(|s| s.name == name) {
                        s.load_file(path);
                        changed = true;
                    }
                }
            }
        }
        changed |= self.drain(headless().then_some(Duration::from_millis(1500)), true);
        changed |= self.poll_jarvis(tx);
        if self.screens.iter().any(|s| s.loads != s.checked) {
            self.check_overrides();
            for s in &mut self.screens {
                s.checked = s.loads;
            }
        }
        if let Some(t) = self.feats_poll() {
            tx.wake_after(t.saturating_duration_since(Instant::now()));
        }
        for s in &mut self.screens {
            changed |= s.poll();
        }
        let now = Instant::now();
        let mut kick = false;
        for (_, step, due) in &mut self.expect {
            if *due <= now {
                kick = true;
                *step += 1;
                *due = now + Duration::from_millis(*RUNNING_RECHECK.get(*step).unwrap_or(&0));
            }
        }
        self.expect.retain(|(_, step, _)| *step < RUNNING_RECHECK.len());
        if kick {
            if let Some(l) = &self.link {
                l.refresh(Duration::ZERO);
            }
        }
        if let Some(next) = self.expect.iter().map(|e| e.2).min() {
            tx.wake_after(next.saturating_duration_since(now));
        }
        if self.copy_offer.as_ref().is_some_and(|o| o.until <= now) {
            self.copy_offer = None;
            changed = true;
        } else if let Some(o) = &self.copy_offer {
            tx.wake_after(o.until.saturating_duration_since(now));
        }
        if self.notice.as_ref().is_some_and(|n| n.2 <= now) {
            self.notice = None;
            changed = true;
        } else if let Some(n) = &self.notice {
            tx.wake_after(n.2.saturating_duration_since(now));
        }
        // The focused pane follows Rune's keyboard focus (a click in a
        // terminal focuses its grid, `grid-<id>`), and Rune's follows the core
        // when an intent moved it.
        let shown = self.layout.0.min(self.panes.len());
        if self.refocus {
            self.refocus = false;
            if let Some(t) = self.panes.get(self.focused).and_then(|p| p.term.as_ref()) {
                tx.focus(t.key("grid"), false);
            }
        } else if let Some(k) = tx.focused().map(str::to_string) {
            if let Some(i) = (0..shown).find(|&i| self.panes[i].term.as_ref().is_some_and(|t| t.key("grid") == k)) {
                if i != self.focused {
                    self.focused = i;
                    self.touch(i);
                    changed = true;
                }
            }
        }
        let settle = headless().then_some(Duration::from_millis(1500));
        let mut notes = vec![];
        let mut copies = vec![];
        for (i, p) in self.panes.iter_mut().enumerate() {
            let Some(t) = &mut p.term else { continue };
            // Only the pane with the keyboard shows a solid cursor and gets
            // the input method's caret (Rune wants one).
            t.set_focused(self.window_focused && i == self.focused && i < shown);
            let mut ch = t.poll_tx(settle, clock, tx);
            for ev in t.take_events() {
                ch = true;
                let name = p.sandbox.clone().unwrap_or_default();
                match ev {
                    TerminalEvent::Attention => {
                        p.attention = true;
                        notes.push((format!("{name} wants you (pane {})", i + 1), false));
                    }
                    TerminalEvent::Bell => log::info!("pane {}: bell", i + 1),
                    TerminalEvent::Title(title) => p.title = Some(title).filter(|t| !t.is_empty()),
                    // OSC 52: a program in the sandbox asking to write the
                    // person's clipboard (Claude Code's own copy). By size:
                    // copied, offered or dropped (approval.rs, `osc_tier`).
                    TerminalEvent::Clipboard(c) => copies.push((i, name, c)),
                    TerminalEvent::Disconnected(why) => p.reconnecting = Some(why),
                    TerminalEvent::Reconnected => p.reconnecting = None,
                }
            }
            if let (true, true, Some(path)) = (ch, i == self.focused, &self.dump_text) {
                let _ = std::fs::write(path, format!("{}x{} offset {} selection {:?}\n{}\n", t.size.0, t.size.1, t.display_offset(), t.selection_text(), t.visible_text().trim_end()));
            }
            changed |= ch;
        }
        for (n, e) in notes {
            self.notify(n, e);
        }
        for (i, name, text) in copies {
            self.osc_copy(i, &name, text, now, tx);
            changed = true;
        }
        if changed {
            self.push_data();
        }
        changed
    }

    /// The window's keys (nothing focused, or what the focused control passed on).
    pub fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.key == Key::Escape && self.screens.iter_mut().any(|s| s.close_menus()) {
            return true;
        }
        if ev.key == Key::Escape && (self.feats.dialog.is_some() || self.feats.modal.is_some()) {
            // The confirmation first, then the modal under it.
            if self.feats.dialog.take().is_none() {
                self.feats.modal = None;
            }
            self.push_data();
            return true;
        }
        if self.jarvis_key(ev, tx) {
            self.push_data();
            return true;
        }
        let f = self.focused;
        if let Some(p) = self.panes.get_mut(f) {
            p.attention = false;
        }
        let Some(t) = self.panes.get_mut(f).and_then(|p| p.term.as_mut()) else {
            if terminal::debug_keys() {
                eprintln!("sbxw keys: key {} pane {} focus {} -> no terminal in the focused pane", terminal::key_label(ev), f + 1, tx.focused().unwrap_or("none"));
            }
            return false;
        };
        t.on_key_tx(ev, tx, lens(f))
    }

    /// The input method (dead keys, CJK, emoji): to the focused pane.
    pub fn on_ime(&mut self, ime: &rune::Ime) -> bool {
        let f = self.focused;
        self.panes.get_mut(f).and_then(|p| p.term.as_mut()).is_some_and(|t| t.on_ime(ime))
    }

    /// The window's paste, to the focused pane. On macOS this is ⌘V (and
    /// Edit > Paste): Rune reads the clipboard itself and calls the root's
    /// `on_paste` instead of handing the key to anyone (rune
    /// `runtime/input/physical.rs`, `dispatch_physical`), so it never reaches
    /// `on_key`. Elsewhere Ctrl+Shift+V reads the clipboard from the key
    /// (`TerminalState::on_key_tx`), and plain Ctrl+V stays the program's.
    pub fn paste(&mut self, text: &str) {
        let f = self.focused;
        let t = self.panes.get_mut(f).and_then(|p| p.term.as_mut());
        if terminal::debug_keys() {
            let to = t.as_ref().map_or("no terminal".to_string(), |t| {
                let bracketed = t.mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
                format!("pane {}{}", t.id, if bracketed { " bracketed" } else { "" })
            });
            eprintln!("sbxw keys: paste (window on_paste) {} bytes -> {to}", text.len());
        }
        if let Some(t) = t {
            t.paste(text);
        }
    }

    /// An OSC 52 copy from pane `i`'s program: copied, offered or dropped by
    /// size, as the web UI does (approval.rs).
    fn osc_copy(&mut self, i: usize, sandbox: &str, text: String, now: Instant, tx: &mut Tx) {
        if text.is_empty() {
            return;
        }
        let bytes = text.len();
        let who = if sandbox.is_empty() { "the sandbox" } else { sandbox };
        let tier = approval::osc_tier(bytes);
        if terminal::debug_keys() {
            eprintln!("sbxw keys: osc52 copy pane {} {bytes} bytes -> {tier:?}", i + 1);
        }
        match tier {
            approval::OscTier::Copy => {
                tx.copy(text);
                self.notify(format!("Copied {} from {who}", approval::size_label(bytes)), false);
            }
            // A newer offer replaces the one showing.
            approval::OscTier::Offer => self.copy_offer = Some(approval::CopyOffer::new(sandbox, text, now)),
            approval::OscTier::Drop => self.notify(
                format!("{} tried to copy {} to the clipboard: more than {}, dropped", if sandbox.is_empty() { "A sandbox" } else { sandbox }, approval::size_label(bytes), approval::size_label(approval::OSC_COPY_MAX)),
                true,
            ),
        }
    }

    /// The person clicked Copy on the offer.
    pub fn accept_copy_offer(&mut self, tx: &mut Tx) {
        if let Some(o) = self.copy_offer.take() {
            if terminal::debug_keys() {
                eprintln!("sbxw keys: osc52 offer copied {} bytes", o.bytes);
            }
            let who = if o.sandbox.is_empty() { "the sandbox".to_string() } else { o.sandbox.clone() };
            tx.copy(o.text);
            self.notify(format!("Copied {} from {who}", approval::size_label(o.bytes)), false);
        }
    }

    /// Opens a terminal in pane `i`.
    fn open(&mut self, i: usize, sandbox: &str, mode: Mode, thread: Option<String>) {
        self.open_with(i, sandbox, mode, thread, ThreadStart::default());
    }

    fn open_with(&mut self, i: usize, sandbox: &str, mode: Mode, thread: Option<String>, start: ThreadStart) {
        self.ensure_panes(i + 1);
        let waker = self.waker.clone();
        self.touch(i);
        let p = &mut self.panes[i];
        p.sandbox = Some(sandbox.to_string());
        p.mode = mode;
        p.thread = thread.clone();
        p.error = None;
        p.term = None;
        p.title = None;
        p.attention = false;
        p.reconnecting = None;
        let req = TerminalRequest::new(sandbox, mode, thread, start);
        let again = req.again();
        let r = match (&self.daemon, waker) {
            (Some(d), Some(w)) => d.open_terminal(&req, w).map_err(|e| format!("{e:#}")),
            (None, _) => Err(format!("not connected: {}", self.connection.text)),
            (_, None) => Err("not started".into()),
        };
        match r {
            Ok(conn) => {
                let (current, wake) = (self.current.clone(), self.waker.clone().expect("setup ran"));
                let reopen: terminal::Reconnect = Arc::new(move || {
                    let d = current.lock().unwrap().clone().ok_or_else(|| anyhow::anyhow!("not connected to the daemon"))?;
                    d.open_terminal(&again, wake.clone())
                });
                let mut t = TerminalState::with_reconnect(conn, reopen);
                // Element keys `grid-N`, `terminal-N` (N the pane's number).
                t.id = (i + 1).to_string();
                // Push-to-talk and the panel's key are the shell's, even with a terminal focused.
                t.keys.reserved = self.wiring.reserved;
                // Headless shots: a steady cursor, so a PNG does not catch it off.
                t.cursor_blink = !headless();
                self.panes[i].term = Some(t);
                self.focused = i;
                self.refocus = true;
            }
            Err(e) => self.panes[i].error = Some(e),
        }
        let running = self.sandboxes.iter().any(|s| s.name == sandbox && s.status == "running");
        if mode == Mode::Agent && !running && self.panes[i].error.is_none() {
            self.expect.retain(|(n, _, _)| n != sandbox);
            self.expect.push((sandbox.to_string(), 0, Instant::now() + Duration::from_millis(RUNNING_RECHECK[0])));
        }
    }

    /// Marks pane `i` as just used (opened or focused).
    pub(crate) fn touch(&mut self, i: usize) {
        self.use_seq += 1;
        if let Some(p) = self.panes.get_mut(i) {
            p.used = self.use_seq;
        }
    }

    /// Where an `open_terminal` goes. `spare` is the web UI's rule
    /// (`assets/js/sandboxes.js`, `openInSparePane`), with one more step: a
    /// new terminal never takes the place of a live one while there is room.
    ///
    /// 1. the first empty pane on screen;
    /// 2. else the layout grows by one pane, up to [`MAX_PANES`]: the pane
    ///    revealed is used if it is empty; one hidden by a smaller layout that
    ///    still holds a terminal is swapped with an empty hidden one first, so
    ///    the hidden terminal is kept (not replaced, as it was in phase 3);
    /// 3. else (four panes, all in use) the least recently used pane is
    ///    replaced, never the focused one, and a notice names what it closed.
    fn pane_for(&mut self, target: &PaneTarget) -> usize {
        match target {
            PaneTarget::Focused => self.focused,
            PaneTarget::Index(n) => {
                let i = n - 1;
                if i >= self.layout.0 {
                    self.layout.0 = i + 1;
                }
                i
            }
            PaneTarget::Spare => self.spare_pane(),
        }
    }

    fn spare_pane(&mut self) -> usize {
        self.ensure_panes(self.layout.0);
        let free = |p: &Pane| p.sandbox.is_none();
        if let Some(i) = (0..self.layout.0).find(|&i| free(&self.panes[i])) {
            return i;
        }
        if self.layout.0 < MAX_PANES {
            let i = self.layout.0;
            self.ensure_panes(MAX_PANES);
            if !free(&self.panes[i]) {
                if let Some(j) = (i + 1..MAX_PANES).find(|&j| free(&self.panes[j])) {
                    self.swap_panes(i, j);
                }
            }
            if free(&self.panes[i]) {
                self.layout.0 = i + 1;
                return i;
            }
            // Every pane, hidden ones included, holds a terminal: show one
            // more and take the least recently used of all (below).
            self.layout.0 = i + 1;
        }
        let shown = self.layout.0;
        let lru = (0..shown).filter(|&i| i != self.focused || shown == 1).min_by_key(|&i| self.panes[i].used).unwrap_or(self.focused);
        if let Some(old) = self.panes[lru].sandbox.clone() {
            self.notify(format!("No free pane: pane {} ({old}, the least recently used) now shows the new terminal", lru + 1), false);
        }
        lru
    }

    /// Empties the panes showing that session; with more than one pane shown,
    /// such a pane goes and the layout shrinks (as `panes.js` closes it).
    fn close_panes(&mut self, sandbox: &str, thread: Option<&str>) {
        let mut i = self.layout.0.min(self.panes.len());
        while i > 0 {
            i -= 1;
            let p = &self.panes[i];
            if p.sandbox.as_deref() != Some(sandbox) || p.thread.as_deref() != thread {
                continue;
            }
            if self.layout.0 > 1 {
                self.panes.remove(i);
                self.panes.push(Pane::empty());
                self.layout.0 -= 1;
                for (k, p) in self.panes.iter_mut().enumerate() {
                    if let Some(t) = p.term.as_mut() {
                        t.id = (k + 1).to_string();
                    }
                }
                if self.focused >= self.layout.0 || self.focused > i {
                    self.focused = self.focused.saturating_sub(1).min(self.layout.0 - 1);
                }
            } else {
                self.panes[i] = Pane::empty();
            }
        }
        // Hidden panes of that session close too.
        for p in self.panes.iter_mut().skip(self.layout.0) {
            if p.sandbox.as_deref() == Some(sandbox) && p.thread.as_deref() == thread {
                *p = Pane::empty();
            }
        }
        self.refocus = true;
    }

    /// Swaps two panes, terminals included (their element keys follow the index).
    fn swap_panes(&mut self, a: usize, b: usize) {
        self.panes.swap(a, b);
        for i in [a, b] {
            if let Some(t) = self.panes[i].term.as_mut() {
                t.id = (i + 1).to_string();
            }
        }
    }

    /// Runs one checked intent.
    pub fn run_intent(&mut self, from: &str, intent: Intent) {
        log::info!("intent from {from}: {intent:?}");
        match intent {
            Intent::OpenTerminal { sandbox, mode, thread, pane, start } => {
                let i = self.pane_for(&pane);
                self.open_with(i, &sandbox, mode, thread, start);
                self.focused = i;
            }
            Intent::StartSandbox { sandbox, pane } => {
                let i = self.pane_for(&pane);
                self.open(i, &sandbox, Mode::Agent, None);
                self.focused = i;
            }
            Intent::StopSandbox { sandbox } => {
                let (Some(d), Some(l)) = (self.daemon.clone(), &self.link) else {
                    self.notify(format!("cannot stop {sandbox}: {}", self.connection.text), true);
                    return;
                };
                if let Some(s) = self.sandboxes.iter_mut().find(|s| s.name == sandbox) {
                    s.status = "stopping…".into();
                }
                // As the web UI does: close this sandbox's terminals first.
                for p in &mut self.panes {
                    if p.sandbox.as_deref() == Some(sandbox.as_str()) {
                        p.term = None;
                    }
                }
                self.expect.retain(|(n, _, _)| *n != sandbox);
                let name = sandbox.clone();
                l.run(d, format!("stop {sandbox}"), move |d| d.stop_sandbox(&name));
            }
            Intent::Select { sandbox } => self.selected = sandbox,
            Intent::ClosePanes { sandbox, thread } => self.close_panes(&sandbox, thread.as_deref()),
            Intent::SetLayout { panes, split } => {
                self.layout = (panes, split);
                self.ensure_panes(panes);
                if self.focused >= panes {
                    self.focused = 0;
                }
            }
            i @ (Intent::OpenModal { .. } | Intent::CloseModal | Intent::OpenPanel { .. } | Intent::ClosePanel { .. } | Intent::Feature(_)) => self.run_feature_intent(from, i),
            other => self.run_jarvis_intent(from, other),
        }
    }
}

impl Host for App {
    fn screen(&mut self, name: &str) -> &mut ScreenState {
        self.screens.iter_mut().find(|s| s.name == name).expect("screen exists")
    }

    fn intents(&mut self, screen: &str, intents: Vec<Intent>) {
        // "Start Jarvis and send": one press returns both; only then is the
        // input sent once Jarvis runs (never by itself later).
        self.wiring.start_and_send = intents.iter().any(|i| matches!(i, Intent::StartJarvis)) && intents.iter().any(|i| matches!(i, Intent::SendToJarvis { .. }));
        for i in intents {
            match i.risk() {
                Risk::Direct => self.run_intent(screen, i),
                Risk::Approval(asks) => match i {
                    // The screen only opens the core's dialog; the click there approves.
                    Intent::ApproveProposal { id } => self.show_proposal(&id),
                    // A feature's: its answer may only be the core's dialog (features::admit).
                    i @ Intent::Feature(_) => self.run_intent(screen, i),
                    i => {
                        // No other intent has this risk; if one is added without its dialog, refuse.
                        self.approvals.push(approval::Pending { screen: screen.into(), intent: i, asks });
                        self.approvals.clear();
                        self.notify(format!("{screen}: '{asks}' needs an approval dialog, which is not built: refused"), true);
                    }
                },
            }
        }
        self.wiring.start_and_send = false;
        self.push_data();
    }
}

// ---------------------------------------------------------------------------
// Drawing

fn small(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}

fn tile(e: El) -> El {
    e.fill(c("bg")).border(1.0, c("elevated")).radius(theme::radius("md").unwrap())
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let root = col()
        .w_full()
        .h_full()
        .fill(c("floor"))
        .on_layout(|s: &mut App, r: [f32; 4]| s.window = (r[2], r[3]))
        .on_key_up(|s: &mut App, ev: &KeyEvent, _tx: &mut Tx| {
            let took = s.jarvis_key_up(ev);
            if took {
                s.push_data();
            }
            took
        })
        .on_window_focus(|s: &mut App, focused: bool, _tx: &mut Tx| {
            s.window_focused = focused;
            // A key held while the window loses focus never comes up here.
            if !focused {
                s.talk_stop();
            }
        });
    // A root `on_paste` makes Rune take the primary modifier + V as a paste
    // before any key handler: ⌘V on macOS, as wanted, but plain Ctrl+V
    // elsewhere, which a terminal must send (^V: Bash's quoted insert, Vim's
    // block selection). So off macOS there is none, and Ctrl+Shift+V pastes
    // through the key. Headless runs keep it for scripted pastes.
    // A picture pasted while a feature's panel that takes files is shown,
    // and no terminal has the keyboard, goes to that feature; text pastes as before.
    let root = if window_paste() {
        root.on_paste_content(|s: &mut App, content: &rune::ClipboardContent, tx: &mut Tx| {
            let in_terminal = tx.focused().is_some_and(|k| k.starts_with("grid-"));
            if let (Some(img), Some(f), false) = (content.image(), s.files_target(), in_terminal) {
                s.feature_incoming(f, crate::feat::Incoming::Png(img.png().to_vec()));
                return;
            }
            if !content.text().is_empty() {
                s.paste(content.text());
            }
        })
    } else {
        root
    };
    if let Some(name) = &s.solo {
        let sc = s.screens.iter().find(|x| &x.name == name).expect("solo screen");
        return root.padding(layout::GUTTER).child(tile(col().w_full().h_full().min_h(0.0).padding(10.0)).child(screen::view::<App>(sc)));
    }
    let inset = cx.titlebar_inset();
    let top = s.screens.iter().find(|x| x.name == "topbar").unwrap();
    let side = s.screens.iter().find(|x| x.name == "sandboxes").unwrap();
    let (top_el, mut overlays) = screen::view_parts::<App>(top);
    let (side_el, more) = screen::view_parts::<App>(side);
    overlays.extend(more);
    // A feature's panel, docked left of Jarvis's.
    // Features' docked panels: the shown one with tabs, or (side by side)
    // every open one in its own column.
    let shown: Vec<String> = if s.feats.split && s.feats.panels.len() > 1 { s.feats.panels.clone() } else { s.feats.panel.iter().cloned().collect() };
    let split = shown.len() > 1;
    let mut fpanels = vec![];
    for name in &shown {
        let Some((_, decl)) = crate::feat::screen(name) else { continue };
        let crate::feat::Slot::Panel { width, .. } = decl.slot else { continue };
        let Some(sc) = s.screens.iter().find(|x| &x.name == name) else { continue };
        let (el_, more) = screen::view_parts::<App>(sc);
        overlays.extend(more);
        fpanels.push(s.feat_panel_view(el_, width, split.then_some(name.as_str())));
    }
    let modal = s.feats.modal.as_ref().and_then(|(name, _)| {
        let (_, decl) = crate::feat::screen(name)?;
        let crate::feat::Slot::Modal { width } = decl.slot else { return None };
        let sc = s.screens.iter().find(|x| &x.name == name)?;
        Some(s.modal_view(screen::view::<App>(sc), width))
    });
    let panel = match s.screens.iter().find(|x| x.name == "jarvis") {
        Some(j) if s.jarvis.open => {
            let (el_, more) = screen::view_parts::<App>(j);
            overlays.extend(more);
            Some(s.panel_view(el_))
        }
        _ => None,
    };
    let bar = row()
        .key("top-bar")
        .h(layout::TOP_BAR_H)
        .mx(layout::GUTTER)
        .mt(layout::GUTTER)
        .fill(c("surface"))
        .border(1.0, c("elevated"))
        .radius(theme::radius("md").unwrap())
        .items_center()
        .window_drag()
        // The traffic lights (macOS) sit over this band: nothing goes under them.
        .child(el().key("traffic-lights-room").w(inset + 4.0).h_full())
        .child(col().grow(1.0).min_w(0.0).h_full().justify_center().pr(12.0).child(top_el));
    let sidebar = tile(col().key("sidebar").w(layout::SIDEBAR_W).h_full().min_h(0.0).padding(8.0)).child(side_el);
    let mut main = row().grow(1.0).min_h(0.0).w_full().padding(layout::GUTTER).gap(layout::GUTTER).child(sidebar).child(panes_view(s));
    for p in fpanels {
        main = main.child(p);
    }
    // Docked on the right: it narrows the panes instead of covering them.
    if let Some(p) = panel {
        main = main.child(p);
    }
    let mut root = root.child(bar).child(main);
    for o in overlays {
        root = root.child(o);
    }
    if let Some(m) = modal {
        root = root.child(m);
    }
    if let Some((t, err, _)) = &s.notice {
        let offer = s.feats.notice_then.is_some();
        let mut note = col().gap(3.0).w_full().child(text(t.clone()).font(small(12.0, 400.0)).color(c("text-bright")).wrap().w_full());
        if offer {
            let what = match s.feats.notice_then.as_ref().map(|t| t.0.as_str()) {
                Some("screen_restore") => "Click to see the file and restore the built-in",
                _ => "Click to open",
            };
            note = note.child(text(what).font(small(11.0, 500.0)).color(c("blue")));
        }
        let mut ne = el().key("notice").pin(None, None, Some(16.0), Some(16.0));
        // A long line wraps in a fixed width (an auto width measures it unwrapped).
        if t.chars().count() > 56 {
            ne = ne.w(420.0f32.min(s.window.0 - 32.0));
        }
        root = root.child(
            ne
                .max_w(420.0)
                .px(12.0)
                .py(8.0)
                .radius(theme::radius("sm").unwrap())
                .fill(c(if *err { "red-tint" } else { "surface" }))
                .border(1.0, c(if *err { "red" } else { "border" }))
                .cursor(Cursor::Pointer)
                .on_press(|s: &mut App| {
                    s.notice = None;
                    // The notice's offer: a click runs it, checked as any intent.
                    if let Some((name, args)) = s.feats.notice_then.take() {
                        s.press_extra("notice", &name, &args);
                    }
                    s.push_data();
                })
                .child(note),
        );
    }
    if let Some(o) = &s.copy_offer {
        root = root.child(approval::copy_offer_view::<App>(o, |s: &mut App, tx: &mut Tx| s.accept_copy_offer(tx), |s: &mut App| s.copy_offer = None));
    }
    if let Some(dialog) = s.dialog_view() {
        root = root.child(dialog);
    }
    if let Some(dialog) = s.confirm_view() {
        root = root.child(dialog);
    }
    root
}

fn panes_view(s: &App) -> El {
    let n = s.layout.0.min(4);
    let vertical = s.layout.1 == Split::Columns;
    let p = |i: usize| pane_view(s, i);
    let get = |id: &str| s.splits.get(id).map(|x| x.0).unwrap_or(0.5);
    let sp = |vertical: bool, id: &'static str, a: El, b: El| {
        screen::split_view::<App, _>(vertical, get(id), a, b, id, |s: &mut App, id: &str| s.splits.entry(id.to_string()).or_insert((0.5, 0.0)))
    };
    let body = match n {
        1 => p(0),
        2 => sp(vertical, "panes", p(0), p(1)),
        3 => sp(true, "panes", p(0), sp(false, "panes-right", p(1), p(2))),
        _ => sp(true, "panes", sp(false, "panes-left", p(0), p(2)), sp(false, "panes-right", p(1), p(3))),
    };
    col().key("panes").grow(1.0).h_full().min_w(0.0).min_h(0.0).child(body)
}

fn pane_view(s: &App, i: usize) -> El {
    let pane = s.panes.get(i);
    let focused = i == s.focused;
    let label = pane.and_then(|p| p.sandbox.clone()).unwrap_or_else(|| "no sandbox".into());
    let connected = pane.is_some_and(|p| p.status() == "connected");
    let mut bar = row()
        .key(format!("pane-{}-bar", i + 1))
        .h(layout::PANE_BAR_H)
        .w_full()
        .px(12.0)
        .gap(8.0)
        .items_center()
        .fill(c("pane-bar"))
        .child(el().w(7.0).h(7.0).radius(3.5).fill(c(if pane.is_some_and(|p| p.attention) { "amber" } else if connected { "blue" } else { "dim" })))
        .child(text(label).font(small(12.0, 500.0)).color(c("text-bright")).truncate().min_w(0.0));
    if let Some(t) = pane.and_then(|p| p.title.clone()) {
        bar = bar.child(text(t).font(small(11.0, 400.0)).color(c("text-muted")).truncate().min_w(0.0).max_w(320.0));
    }
    if pane.is_some_and(|p| p.attention) {
        bar = bar.child(el().key(format!("pane-{}-attention", i + 1)).px(5.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(c("amber-tint")).border(1.0, c("amber")).child(text("wants you").font(small(10.5, 500.0)).color(c("amber"))));
    }
    if let Some(p) = pane.filter(|p| p.sandbox.is_some()) {
        bar = bar.child(el().px(5.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(c("elevated")).child(text(p.mode.name()).font(small(10.5, 400.0)).color(c("text-muted"))));
        for x in s.pane_extras(i, p.sandbox.as_deref().unwrap_or(""), p.thread.as_deref()) {
            bar = bar.child(x);
        }
        if !connected {
            bar = bar.child(text(p.status()).font(small(11.0, 400.0)).color(c("text-muted")));
        }
        bar = bar.child(el().grow(1.0)).child(
            el().key(format!("pane-{}-close", i + 1))
                .role("button")
                .label("Close pane")
                .help("Close this terminal")
                .padding(3.0)
                .radius(theme::radius("xs").unwrap())
                .hover(c("border"))
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut App| {
                    if let Some(p) = s.panes.get_mut(i) {
                        *p = Pane::empty();
                    }
                    s.push_data();
                })
                .child(rune::icon(crate::icons::get("close").unwrap(), 12.0).color(c("text-muted"))),
        );
    }
    let body = match pane {
        // The pane fills a row (its width grows, its height is the row's).
        Some(Pane { term: Some(t), .. }) => row().w_full().grow(1.0).min_h(0.0).child(terminal::pane(t, lens(i))),
        Some(p) => {
            let msg = match (&p.sandbox, &p.error) {
                (_, Some(e)) => e.clone(),
                (Some(sb), None) if matches!(s.target, Target::Fixture(_)) => format!("{sb}: no terminal in a fixture (scripts/shot.sh runs one against the stand-in daemon)"),
                (Some(sb), None) => format!("{sb}: {}", p.status()),
                (None, None) => if s.sandboxes.is_empty() { "No sandbox here. Create one with `sbxw up`.".into() } else { "Select a sandbox in the list to open it here.".into() },
            };
            col().grow(1.0).w_full().padding(16.0).child(text(msg).font(small(12.0, 400.0)).color(c(if p.error.is_some() { "red" } else { "text-muted" })).wrap())
        }
        None => el().grow(1.0),
    };
    let border = if focused { rune::color::hex("#36618f").unwrap() } else { c("elevated") };
    col()
        .key(format!("pane-{}", i + 1))
        .w_full()
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(c("bg"))
        .border(1.0, border)
        .radius(theme::radius("md").unwrap())
        .clip_rounded(theme::radius("md").unwrap())
        .on_press(move |s: &mut App| {
            if s.focused != i {
                s.focused = i;
                s.touch(i);
                s.push_data();
            }
        })
        .child(bar)
        .child(el().h(1.0).w_full().fill(c(if focused { "blue" } else { "elevated" })))
        .child(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ProposalAnswers;

    fn app() -> App {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        App::new(Args { fixture: Some(fixture.into()), ..Args::default() })
    }

    /// A yes the screen put in the input itself is a message, never an
    /// approval; the same word typed by the person approves a low-risk one.
    #[test]
    fn only_the_persons_own_yes_approves_and_only_low_risk() {
        let open = |f: &str| App::new(Args { fixture: Some(format!("{}/fixtures/jarvis/{f}.json", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
        let wait = |a: &mut App| {
            let until = Instant::now() + Duration::from_secs(3);
            while a.wiring.pending.load(std::sync::atomic::Ordering::SeqCst) > 0 && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            while let Ok(ev) = a.wiring.rx.try_recv() {
                a.on_jevent(ev);
            }
        };
        let put = |a: &mut App, text: &str, by_screen: bool| {
            let given = by_screen.then(|| text.to_string());
            a.screen_mut("jarvis").unwrap().ui.edits.insert("jarvis-input".into(), (rune::TextEdit::new(text), given));
            a.run_intent("jarvis", Intent::SendToJarvis { input: "jarvis-input".into() });
        };
        let mut a = open("proposal-low");
        assert_eq!(a.jarvis.armed.as_deref(), Some("p-low-1"));
        put(&mut a, "yes", true);
        wait(&mut a);
        assert_eq!(a.jarvis.proposal("p-low-1").unwrap()["state"], "pending", "the screen's own yes approves nothing");
        let mut a = open("proposal-low");
        put(&mut a, "yes", false);
        wait(&mut a);
        assert_eq!(a.jarvis.proposal("p-low-1").unwrap()["state"], "applied");
        assert_eq!(a.jarvis.proposal("p-low-1").unwrap()["via"], "voice");
        assert_eq!(a.screen_mut("jarvis").unwrap().ui.edits["jarvis-input"].0.text, "", "taken, so emptied");
        let mut a = open("proposal-high");
        put(&mut a, "oui", false);
        wait(&mut a);
        assert_eq!(a.jarvis.proposal("p-high-1").unwrap()["state"], "pending", "high risk: a click only");
        assert!(a.jarvis.log.last().unwrap().text.starts_with("High risk: approve it on screen"));
        // Even a voice approval sent past the core is refused by the daemon's rule.
        a.answer("p-high-1", "voice");
        wait(&mut a);
        assert_eq!(a.jarvis.proposal("p-high-1").unwrap()["state"], "pending");
        // The screen's `approve_proposal` only shows the dialog.
        a.jarvis.shown = None;
        a.intents("jarvis", vec![Intent::ApproveProposal { id: "p-high-1".into() }]);
        wait(&mut a);
        assert_eq!((a.jarvis.shown.as_deref(), a.jarvis.proposal("p-high-1").unwrap()["state"].as_str()), (Some("p-high-1"), Some("pending")));
        // A host command: the click is refused until the box is ticked.
        let mut a = open("proposal-ack");
        a.approve_click();
        wait(&mut a);
        assert_eq!(a.jarvis.proposal("p-ack-1").unwrap()["state"], "pending");
        a.toggle_ack();
        a.approve_click();
        wait(&mut a);
        assert_eq!(a.jarvis.proposal("p-ack-1").unwrap()["state"], "applied");
        // A 409: the daemon's reason beside the buttons.
        let mut a = open("proposal-refused");
        a.approve_click();
        wait(&mut a);
        assert!(a.jarvis.dialog_error.as_ref().unwrap().1.contains("changed since the proposal"), "{:?}", a.jarvis.dialog_error);
    }

    /// Phase 3 bug (user, macOS): a new sandbox opened in a "spare" pane took
    /// the place of a live terminal. A spare pane is an empty one on screen,
    /// else one more pane; a terminal hidden by a smaller layout is kept; only
    /// with four busy panes is one replaced: the least recently used, not the focused.
    #[test]
    fn a_spare_pane_never_replaces_a_live_terminal_while_there_is_room() {
        let open = |a: &mut App, i: usize, sb: &str| {
            a.ensure_panes(i + 1);
            a.panes[i].sandbox = Some(sb.into());
            a.touch(i);
        };
        let spare = |a: &mut App, sb: &str| a.run_intent("test", Intent::OpenTerminal { sandbox: sb.into(), mode: Mode::Agent, thread: None, pane: PaneTarget::Spare, start: ThreadStart::default() });
        let shown = |a: &App| a.panes.iter().take(a.layout.0).map(|p| p.sandbox.clone().unwrap_or_default()).collect::<Vec<_>>();
        let app = || {
            let mut a = app();
            a.panes = vec![Pane::empty()];
            (a.layout.0, a.focused) = (2, 0);
            a
        };
        // One pane in use, layout "1 terminal": a second pane is added.
        let mut a = app();
        a.layout.0 = 1;
        open(&mut a, 0, "web-app");
        spare(&mut a, "new-1");
        assert_eq!(shown(&a), ["web-app", "new-1"]);
        assert_eq!(a.focused, 1);
        // Two terminals, then the layout set back to 1: the second is hidden,
        // still live. The next spare open must not replace it.
        let mut a = app();
        open(&mut a, 0, "web-app");
        open(&mut a, 1, "web-api");
        a.run_intent("test", Intent::SetLayout { panes: 1, split: Split::Columns });
        spare(&mut a, "new-2");
        assert_eq!(a.layout.0, 2);
        assert_eq!(shown(&a), ["web-app", "new-2"]);
        assert!(a.panes.iter().any(|p| p.sandbox.as_deref() == Some("web-api")), "the hidden terminal is kept: {:?}", a.panes.iter().map(|p| &p.sandbox).collect::<Vec<_>>());
        // An empty pane on screen is taken first.
        let mut a = app();
        a.layout.0 = 3;
        open(&mut a, 0, "a");
        open(&mut a, 2, "c");
        spare(&mut a, "new-3");
        assert_eq!(shown(&a), ["a", "new-3", "c"]);
        // Four busy panes: the least recently used one, never the focused one.
        let mut a = app();
        a.layout.0 = 4;
        for (i, sb) in ["a", "b", "c", "d"].iter().enumerate() {
            open(&mut a, i, sb);
        }
        a.touch(0);
        a.focused = 3;
        spare(&mut a, "new-4");
        assert_eq!(shown(&a), ["a", "new-4", "c", "d"], "b was the least recently used");
        assert!(a.notice.as_ref().unwrap().0.contains("pane 2 (b, the least recently used)"));
    }

    /// Phase 3 bug (user, macOS): an old override of `sandboxes.luau` hid the
    /// new row extras with no word. Now: a notice and a marker on the screen,
    /// the extras it does not draw drawn by the core under it, and Restore
    /// (the core's dialog, naming the file) deletes it.
    #[test]
    fn an_override_older_than_its_builtin_is_flagged_and_restorable() {
        let dir = std::env::temp_dir().join(format!("sbxw-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sandboxes.luau");
        // Jarvis's old copy: names only, no extras.
        std::fs::write(&file, r#"return { render = function(_, data)
  local t = { gap = 4 }
  for _, sb in ipairs(data.sandboxes) do table.insert(t, text{ sb.name, key = "row-" .. sb.name }) end
  return column(t)
end, actions = {} }"#).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/network/machine-wide.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), screens_dir: Some(dir.clone()), ..Args::default() });
        a.push_data();
        let sc = |a: &App| a.screens.iter().find(|s| s.name == "sandboxes").unwrap().stale;
        assert!(sc(&a), "made before this app: stale");
        assert!(a.notice.as_ref().unwrap().0.starts_with("Your sandboxes screen is a customised copy made for an older version"), "{:?}", a.notice);
        assert_eq!(a.feats.notice_then, Some(("screen_restore".into(), serde_json::json!({ "screen": "sandboxes" }))));
        assert_eq!(a.screen_data()["app"]["overrides"]["sandboxes"]["stale"], true);
        // The extras the copy does not draw are listed for the core to draw (no double drawing).
        let s = a.screens.iter().find(|s| s.name == "sandboxes").unwrap();
        let missing: Vec<String> = s.undrawn_extras().iter().map(|(g, e)| format!("{g}/{}", e["id"].as_str().unwrap())).collect();
        assert!(missing.contains(&"web-app/approvals".to_string()) && missing.contains(&"web-app/network".to_string()), "{missing:?}");
        let builtin_drawn = ScreenState::new("sandboxes", &std::env::temp_dir().join("sbxw-no-overrides"), a.screen_data());
        assert!(builtin_drawn.undrawn_extras().is_empty(), "{:?}", builtin_drawn.undrawn_extras());
        // Updated (by Jarvis, for this version): no longer stale.
        std::fs::write(&file, std::fs::read_to_string(&file).unwrap() + "\n-- updated\n").unwrap();
        let i = a.screens.iter().position(|s| s.name == "sandboxes").unwrap();
        a.screens[i].load_file(file.clone());
        a.check_overrides();
        assert!(!sc(&a));
        // The built-in changes later (another release): stale again.
        let mut rec = a.store.get("overrides")["sandboxes"].clone();
        rec["builtin"] = "0000000000000000".into();
        a.store.set_in("overrides", "sandboxes", rec);
        a.check_overrides();
        assert!(sc(&a));
        // Restore: the core's dialog names the file; Confirm deletes it.
        a.intents("test", crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": "screen_restore", "args": { "screen": "sandboxes" } })).unwrap());
        let c = a.feats.dialog.as_ref().unwrap().confirm.clone().unwrap();
        assert!(c.lines[0].contains(&file.display().to_string()), "{:?}", c.lines);
        assert!(file.exists(), "nothing deleted before Confirm");
        crate::approval::ConfirmAnswers::confirm_click(&mut a);
        assert!(!file.exists());
        assert!(a.feats.dialog.is_none(), "closed once done");
        assert!(a.store.get("overrides")["sandboxes"].is_null());
        // A core operation is never run without the dialog.
        let mut call = crate::feat::Call::post("core:screen-restore/topbar", J::Null, "x");
        call.method = crate::feat::Method::Core;
        a.feature_actions(App::feature_of("shell").unwrap(), "test", vec![crate::feat::Action::Call(call)]);
        assert!(a.notice.as_ref().unwrap().0.contains("runs only from the app's own dialog"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn closing_a_threads_panes_empties_only_that_session() {
        let mut a = app();
        a.panes = vec![Pane::empty(), Pane::empty(), Pane::empty()];
        a.layout.0 = 3;
        for (i, t) in [None, Some("t1"), Some("t2")].iter().enumerate() {
            a.panes[i].sandbox = Some("web-app".into());
            a.panes[i].thread = t.map(str::to_string);
        }
        a.focused = 2;
        a.run_intent("test", Intent::ClosePanes { sandbox: "web-app".into(), thread: Some("t1".into()) });
        assert_eq!(a.layout.0, 2);
        let shown: Vec<_> = a.panes.iter().take(2).map(|p| p.thread.clone()).collect();
        assert_eq!(shown, [None, Some("t2".to_string())]);
        assert_eq!(a.focused, 1, "the focus follows its pane");
        a.run_intent("test", Intent::ClosePanes { sandbox: "web-app".into(), thread: None });
        a.run_intent("test", Intent::ClosePanes { sandbox: "web-app".into(), thread: Some("t2".into()) });
        assert_eq!(a.layout.0, 1);
        assert!(a.panes[0].sandbox.is_none(), "the last pane empties, it does not go");
    }

    /// A per-sandbox key's failed read: `errors[key][sandbox]`, the last good value kept.
    #[test]
    fn a_failed_per_sandbox_read_says_why_for_that_sandbox() {
        let mut a = app();
        let ev = |r: Vec<(&str, Result<J, String>)>| LinkEvent::DataPer { key: "network_ports".into(), results: r.into_iter().map(|(s, r)| (s.to_string(), r)).collect() };
        a.feats_link(ev(vec![("web-app", Ok(serde_json::json!([1]))), ("docs", Ok(serde_json::json!([2])))]));
        assert!(a.screen_data()["app"]["errors"].get("network_ports").is_none());
        a.feats_link(ev(vec![("web-app", Ok(serde_json::json!([3]))), ("docs", Err("sbx: no such sandbox".into()))]));
        let d = a.screen_data();
        assert_eq!(d["app"]["errors"]["network_ports"], serde_json::json!({ "docs": "sbx: no such sandbox" }));
        assert_eq!(d["network_ports"], serde_json::json!({ "web-app": [3], "docs": [2] }), "docs keeps its last good value");
        a.feats_link(ev(vec![("web-app", Ok(serde_json::json!([3]))), ("docs", Ok(serde_json::json!([4])))]));
        assert!(a.screen_data()["app"]["errors"].get("network_ports").is_none(), "cleared once it reads again");
        // A plain key: a string.
        a.feats_link(LinkEvent::Data { key: "usage".into(), result: Err("down".into()) });
        assert_eq!(a.screen_data()["app"]["errors"]["usage"], "down");
    }

    #[test]
    fn osc52_copies_offers_or_drops_by_size_and_a_newer_offer_replaces() {
        let mut a = app();
        let mut tx = Tx::default();
        let now = Instant::now();
        // Up to 4 KB: copied at once, and said so.
        a.osc_copy(0, "mus", "x".repeat(40), now, &mut tx);
        assert_eq!(a.notice.as_ref().map(|n| (n.0.as_str(), n.1)), Some(("Copied 40 B from mus", false)));
        assert!(a.copy_offer.is_none());
        // Up to 100 KB: offered, nothing copied yet; a newer offer replaces it.
        a.notice = None;
        a.osc_copy(0, "mus", "y".repeat(6 * 1024), now, &mut tx);
        assert_eq!(a.copy_offer.as_ref().map(|o| o.bytes), Some(6 * 1024));
        assert!(a.notice.is_none());
        a.osc_copy(1, "web-app", "z".repeat(5000), now, &mut tx);
        let o = a.copy_offer.as_ref().unwrap();
        assert_eq!((o.sandbox.as_str(), o.bytes), ("web-app", 5000));
        assert_eq!(o.headline(), "web-app wants to copy 4.9 KB to your clipboard:");
        // A click on Copy copies and closes it.
        a.accept_copy_offer(&mut tx);
        assert!(a.copy_offer.is_none());
        assert_eq!(a.notice.as_ref().map(|n| n.0.as_str()), Some("Copied 4.9 KB from web-app"));
        // Above: dropped, with an error notice; an offer showing stays.
        a.osc_copy(0, "mus", "y".repeat(6 * 1024), now, &mut tx);
        a.osc_copy(0, "mus", "w".repeat(100 * 1024 + 1), now, &mut tx);
        assert_eq!(a.notice.as_ref().map(|n| n.1), Some(true));
        assert!(a.notice.as_ref().unwrap().0.contains("more than 100.0 KB, dropped"), "{:?}", a.notice);
        assert_eq!(a.copy_offer.as_ref().map(|o| o.bytes), Some(6 * 1024));
    }
}
