//! The window: the web UI's layout (top bar, sandbox side panel, a pane
//! area of terminals) around two Luau screens, `topbar` and `sandboxes`.
//! The core owns the panes, the daemon link and every intent; screens only
//! draw and ask.
//!
//! Layout, as `assets/app.css` draws it: a dark floor (`#080b0f`), and on it,
//! 6 pt apart, rounded tiles: the header (48 pt), the sidebar, and one tile
//! per pane, each with a 32 pt bar over its terminal.

mod bar;
mod features;
mod jarvis;
mod palette;
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
    /// `wait:<key>[:<ms>]` (`gone`: `wait-gone:<key>[:<ms>]`): hold in real
    /// time, taking in what arrives as `hold` does, until an element with
    /// that key is drawn (gone: no longer drawn), at most `ms` (10 s by
    /// default); then the script goes on either way, so a tap on a key that
    /// never came fails with Rune's own message.
    Wait { key: String, ms: u64, gone: bool },
}

/// Parses `wait:<key>[:<ms>]` and `wait-gone:<key>[:<ms>]`.
fn wait_mark(n: &str) -> Option<Mark> {
    let (rest, gone) = match (n.strip_prefix("wait-gone:"), n.strip_prefix("wait:")) {
        (Some(r), _) => (r, true),
        (None, Some(r)) => (r, false),
        _ => return None,
    };
    let (key, ms) = match rest.rsplit_once(':') {
        Some((k, ms)) if !k.is_empty() && ms.parse::<u64>().is_ok() => (k, ms.parse().unwrap()),
        _ => (rest, 10_000),
    };
    (!key.is_empty()).then(|| Mark::Wait { key: key.to_string(), ms, gone })
}

/// The item ids of the menu or select whose id is `id`.
fn menu_items(n: &crate::luau::Node, id: &str) -> Vec<String> {
    if n.id.as_deref() == Some(id) && !n.items.is_empty() {
        return n.items.iter().map(|i| i.id.clone()).collect();
    }
    n.children.iter().map(|c| menu_items(c, id)).find(|v| !v.is_empty()).unwrap_or_default()
}

fn node_has_key(n: &crate::luau::Node, key: &str) -> bool {
    n.key.as_deref() == Some(key) || n.id.as_deref() == Some(key) || n.children.iter().any(|c| node_has_key(c, key))
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
    /// Each pane bar's width as last laid out (0: not yet), for what fits (bar.rs).
    bar_w: [f32; MAX_PANES],
    /// The top bar screen's column width as last laid out (0: not yet).
    top_w: f32,
    /// The pane whose bar's overflow menu is open.
    bar_menu: Option<usize>,
    /// The sidebar's width while its edge is dragged (written to the store on release).
    sidebar_live: Option<f32>,
    /// The command palette, while open (palette.rs).
    pub palette: Option<palette::Palette>,
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
            bar_w: [0.0; MAX_PANES],
            top_w: 0.0,
            bar_menu: None,
            sidebar_live: None,
            palette: None,
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
                        if let Some(m) = wait_mark(n) {
                            return Some((t, m));
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
        {
            // Features' sockets hand their actions to this thread (socket.rs).
            let w = waker.clone();
            crate::socket::set_wake(Arc::new(move || w()));
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
        data["app"]["topbar"] = self.top_layout(&mut data);
        data["app"]["errors"] = J::Object(self.feats.errors.clone());
        // Each sandbox's group colour (the web UI's), for rails, swatches and pane accents.
        data["app"]["groups"] = self.sandboxes.iter().filter_map(|sb| theme::group_color(sb.workspace.as_deref(), sb.chat).map(|c| (sb.name.clone(), J::String(format!("#{c:06x}"))))).collect::<serde_json::Map<_, _>>().into();
        data["app"]["sidebar"] = serde_json::json!({ "w": self.sidebar_w(), "shown": self.sidebar_shown() });
        // Every shortcut, for Help: `{keys, what}`.
        data["app"]["shortcuts"] = palette::shortcuts(&self.jarvis.key_name).into_iter().map(|(k, w)| serde_json::json!({ "keys": k, "what": w })).collect::<Vec<_>>().into();
        data["app"]["overrides"] = self.overrides_view();
        data["app"]["kept"] = self.store.get("features").clone();
        // Picture surfaces: the key that gives the keyboard back, and which one has it.
        data["app"]["uploads"] = crate::files::uploads();
        data["app"]["surface"] = serde_json::json!({ "release": crate::surface::RELEASE_KEY, "focused": crate::surface::focused() });
        data
    }

    /// `data.app.topbar`: how the top bar fits its width (bar.rs): `w` (its
    /// column, points), `compact` (no title, the layout switch as an icon
    /// menu), `more` (how many extras go into its menu). The extras that do
    /// not fit, by priority, get `more = true` in `data.app.extras.topbar`.
    fn top_layout(&self, data: &mut J) -> J {
        let w = if self.top_w > 0.0 { self.top_w } else { self.window.0 - 140.0 };
        let pending = data["jarvis"]["pending"].as_array().map_or(0, Vec::len);
        let jarvis = if pending > 0 { format!("Jarvis · {pending} to review") } else { "Jarvis".into() };
        let Some(list) = data["app"]["extras"]["topbar"].as_array_mut() else {
            return serde_json::json!({ "w": w, "compact": w < bar::TOP_COMPACT, "more": 0 });
        };
        let mut order: Vec<usize> = (0..list.len()).collect();
        let rank = |e: &J| bar::rank(e["tone"].as_str().unwrap_or(""), e["count"].as_u64());
        order.sort_by_key(|&i| rank(&list[i]));
        let label = |e: &J| {
            let l = e["label"].as_str().unwrap_or("");
            match e["count"].as_u64() {
                Some(n) if l.is_empty() => n.to_string(),
                Some(n) => format!("{l} {n}"),
                None => l.to_string(),
            }
        };
        // As topbar.luau draws them: a button when it has an intent, else a
        // badge; Setup, Help, the Mac and usage in the right group.
        let measured: Vec<bar::TopExtra> = order
            .iter()
            .map(|&i| bar::TopExtra { label: label(&list[i]), icon: !list[i]["icon"].is_null(), button: !list[i]["intent"].is_null(), tool: matches!(list[i]["id"].as_str(), Some("usage" | "setup" | "help" | "mac")) })
            .collect();
        // The layout switch shows the current choice (topbar.luau's labels).
        let layout = match (self.layout.0, self.layout.1) {
            (1, _) => "1 terminal",
            (2, Split::Rows) => "2 stacked",
            (2, _) => "2 side by side",
            (3, _) => "3 terminals",
            _ => "4 terminals",
        };
        let word = match self.connection.state {
            "up" => "connected",
            "connecting" => "connecting",
            "down" => "offline",
            other => other,
        };
        let (compact, k) = bar::top_fit(w, &jarvis, layout, word, &measured);
        for &i in &order[k..] {
            list[i]["more"] = J::Bool(true);
        }
        serde_json::json!({ "w": w, "compact": compact, "more": order.len() - k })
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
            ev @ (LinkEvent::Answer { .. } | LinkEvent::Bytes { .. } | LinkEvent::Data { .. } | LinkEvent::DataPer { .. }) => return self.feats_link(ev),
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
                Mark::Wait { key, ms, gone } => {
                    let t0 = Instant::now();
                    let until = t0 + Duration::from_millis(ms);
                    loop {
                        if self.drain(None, false) {
                            self.push_data();
                            changed = true;
                        }
                        for s in &mut self.screens {
                            changed |= s.poll();
                        }
                        for p in &mut self.panes {
                            if let Some(t) = &mut p.term {
                                changed |= t.poll(None);
                            }
                        }
                        if self.drawn(&key) != gone {
                            log::info!("waited {} ms for {}{key}", t0.elapsed().as_millis(), if gone { "the end of " } else { "" });
                            break;
                        }
                        if Instant::now() >= until {
                            log::warn!("gave up waiting for {}{key} after {} ms", if gone { "the end of " } else { "" }, t0.elapsed().as_millis());
                            break;
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
        changed |= self.surfaces_poll(tx);
        let settle = headless().then_some(Duration::from_millis(1500));
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
                    // No notice: the pane bar's amber "wants you" and the
                    // list's state already say it (the user found it noise).
                    TerminalEvent::Attention => p.attention = true,
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
    /// Escape closes the top-most thing: an open menu, a pane bar's menu,
    /// the core's dialog, then the modal under it. False when nothing was
    /// open (the key goes on: a field, a terminal).
    pub fn escape(&mut self) -> bool {
        if self.palette.is_some() {
            self.palette_close();
            self.push_data();
            return true;
        }
        if self.screens.iter_mut().any(|s| s.close_menus()) {
            return true;
        }
        if self.bar_menu.take().is_some() {
            return true;
        }
        if self.feats.dialog.is_some() || self.feats.modal.is_some() {
            // The confirmation first, then the modal under it.
            if !self.dismiss_dialog() {
                self.feats.modal = None;
            }
            self.push_data();
            return true;
        }
        false
    }

    pub fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.key == Key::Escape && self.escape() {
            return true;
        }
        // The app's chords (palette.rs): never a terminal's.
        if self.app_key(ev, tx.focused()) {
            return true;
        }
        if self.shortcut(ev, tx) {
            self.push_data();
            return true;
        }
        if self.jarvis_key(ev, tx) {
            self.push_data();
            return true;
        }
        // A terminal gets keys only when its grid has the keyboard (or
        // nothing has it): never while an input, a textarea, a secret field,
        // a button, a modal, a core dialog or a picture surface has it,
        // whatever the modifiers (Ctrl+C typed in a field must never reach
        // an agent's shell).
        if !self.terminal_may_take_keys(tx.focused()) {
            return false;
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

    /// Whether an element of that key is in what the app draws now (for
    /// `wait:` marks): a screen's last render (keys and input ids), the
    /// core's dialog and its parts, pane bars and their extras, terminals.
    fn drawn(&self, key: &str) -> bool {
        let shown_panes = self.layout.0.min(self.panes.len());
        if self.screens.iter().any(|s| s.last_good.as_ref().is_some_and(|n| node_has_key(n, key))) {
            return true;
        }
        // An open menu's or select's list and its items (`<id>-list`, `<id>-<item>`).
        for sc in &self.screens {
            let (Some(open), Some(n)) = (sc.ui.open.as_deref(), sc.last_good.as_ref()) else { continue };
            if key == format!("{open}-list") || menu_items(n, open).iter().any(|i| key == format!("{open}-{i}")) {
                return true;
            }
        }
        if let Some(d) = &self.feats.dialog {
            let built = d.confirm.is_some();
            let ack = d.confirm.as_ref().is_some_and(|c| c.ack.is_some());
            let secrets = d.confirm.as_ref().map_or(0, |c| c.secrets.len());
            let fixed = matches!(key, "cf-dialog" | "cf-cancel" | "cf-close" | "cf-title") || (built && key == "cf-confirm") || (ack && key == "cf-ack") || (!built && key == "cf-loading") || (d.error.is_some() && key == "cf-error");
            let secret = key.strip_prefix("cf-secret-").and_then(|i| i.parse::<usize>().ok()).is_some_and(|i| i < secrets);
            if fixed || secret {
                return true;
            }
        }
        if self.jarvis.shown.is_some() && key.starts_with("jv-") {
            return true;
        }
        if self.palette.is_some() && (key == "palette" || key == "palette-input" || key == "palette-list" || self.palette_shown().iter().any(|c| key == format!("palette-{}", c.id))) {
            return true;
        }
        for i in 0..shown_panes {
            let p = &self.panes[i];
            let n = i + 1;
            if key == format!("pane-{n}") || key == format!("pane-{n}-bar") || (p.sandbox.is_some() && key == format!("pane-{n}-close")) {
                return true;
            }
            if p.term.as_ref().is_some_and(|t| key == t.key("grid") || key == t.key("terminal")) {
                return true;
            }
            // A pane bar's extras: inline, or in its overflow menu while that is open.
            if let Some(f) = self.pane_fit(i) {
                let open = self.bar_menu == Some(i);
                if f.shown.iter().any(|x| x.key(n) == key) || (open && f.hidden.iter().any(|x| x.key(n) == key)) {
                    return true;
                }
                if !f.hidden.is_empty() && (key == format!("pane-{n}-more") || (open && key == format!("pane-{n}-more-list"))) {
                    return true;
                }
            }
        }
        false
    }

    /// Whether a key or a paste the window did not route elsewhere may go to
    /// the focused pane's terminal: only with no dialog, modal or surface
    /// up, and Rune's focus on a terminal grid (the core moves it there when
    /// it opens or focuses a pane).
    pub fn terminal_may_take_keys(&self, rune_focus: Option<&str>) -> bool {
        let overlay = self.feats.dialog.is_some() || self.feats.modal.is_some() || self.jarvis.shown.is_some() || self.palette.is_some() || crate::surface::focused().is_some();
        !overlay && rune_focus.is_some_and(|k| k.starts_with("grid"))
    }

    /// The input method (dead keys, CJK, emoji): to the focused pane.
    pub fn on_ime(&mut self, ime: &rune::Ime, rune_focus: Option<&str>) -> bool {
        if let Some(id) = crate::surface::focused() {
            if let rune::Ime::Commit(t) = ime {
                if !t.is_empty() {
                    self.feats_surface(crate::surface::SurfaceEvent::Text { surface: id, text: t.clone() });
                }
            }
            return true;
        }
        if !self.terminal_may_take_keys(rune_focus) {
            return false;
        }
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
        if let Some(id) = crate::surface::focused() {
            return self.surface_paste(&id, text);
        }
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
            self.remove_pane(i);
        }
        // Hidden panes of that session close too.
        for p in self.panes.iter_mut().skip(self.layout.0) {
            if p.sandbox.as_deref() == Some(sandbox) && p.thread.as_deref() == thread {
                *p = Pane::empty();
            }
        }
        self.refocus = true;
    }

    /// Closes pane `i`: the layout shrinks to the panes that remain, in their
    /// order, and they take the room; the last pane is not removed, it
    /// becomes empty. The focus goes to the pane that takes the freed place
    /// (the next one, else the one before). Only the view goes: the sandbox
    /// and its session go on (a terminal's socket closes as when it is
    /// replaced).
    pub fn remove_pane(&mut self, i: usize) {
        if i >= self.panes.len() {
            return;
        }
        if self.bar_menu.is_some() {
            self.bar_menu = None;
        }
        if self.layout.0 > 1 && i < self.layout.0 {
            self.panes.remove(i);
            self.panes.push(Pane::empty());
            self.layout.0 -= 1;
            if self.layout.0 < 2 {
                self.layout.1 = Split::Columns;
            }
            for (k, p) in self.panes.iter_mut().enumerate() {
                if let Some(t) = p.term.as_mut() {
                    t.id = (k + 1).to_string();
                }
            }
            if self.focused >= self.layout.0 || self.focused > i {
                self.focused = self.focused.saturating_sub(1).min(self.layout.0 - 1);
            }
            // The bars' widths change with the layout: measured again.
            self.bar_w = [0.0; MAX_PANES];
        } else {
            self.panes[i] = Pane::empty();
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
            Intent::CopyText { text } => {
                // Only the person's own press copies: not a feature's chain of
                // actions (`feature:<name>`: data, events, answers), not an
                // input's per-keystroke action.
                if !self.pressed(from) {
                    self.notify(format!("{from}: a copy runs only from your own press: refused"), true);
                } else if text.len() > 100 * 1024 {
                    self.notify(format!("{from}: more than 100 KB to copy, refused"), true);
                } else {
                    self.feats.asks.push(("copy", features::Ask::Copy(text)));
                }
            }
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

    fn surface_event(&mut self, ev: crate::surface::SurfaceEvent, _tx: &mut Tx) {
        self.feats_surface(ev);
    }

    fn surface_key(&mut self, ev: &KeyEvent, down: bool, tx: &mut Tx) -> bool {
        self.surface_key_app(ev, down, tx)
    }
}

// ---------------------------------------------------------------------------
// Drawing

fn small(size: f32, weight: f32) -> TextStyle {
    screen::body(size, weight)
}

fn tile(e: El) -> El {
    e.fill(c("bg")).border(1.0, c("elevated")).radius(theme::radius("md").unwrap())
}

/// The window's frame: on macOS the close, minimise and zoom buttons sit
/// on the top bar's content line, not AppKit's default (7, 6), which put
/// their centre 17 pt above the bar's text. Rune places them at the close
/// button's top-left (`Chrome::traffic_lights`, Electron's
/// `trafficLightPosition`): 12 pt into the bar, and centred on it
/// (gutter + half the bar, less half of AppKit's 14 pt button). Only the
/// `.window_drag()` regions move the window (the top bar), so the taller
/// titlebar container this makes never takes a press meant for the bar.
/// Ignored on Linux and Windows (their native frame stays).
pub const LIGHTS: (f32, f32) = (layout::GUTTER + 12.0, layout::GUTTER + layout::TOP_BAR_H / 2.0 - rune::Chrome::TRAFFIC_LIGHT / 2.0);
pub const fn window_chrome() -> rune::Chrome {
    rune::Chrome { full_size_content: true, traffic_lights: Some(LIGHTS), drag_regions: true }
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    // Keyboard focus as a web page has it: every pressable control is a Tab
    // stop in reading order, and shows a 2 pt ring 2 pt outside it when the
    // keyboard put it there (never for a click).
    cx.web_focus();
    cx.focus_ring(2.0, 2.0, c("focus"));
    // The app's zoom (Cmd/Ctrl with + − 0 outside a terminal, or the palette).
    cx.zoom(s.zoom());
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
        // Escape closes the top-most thing even while a control inside it
        // has the keyboard (Rune would only take the focus off it).
        .on_key(|s: &mut App, ev: &KeyEvent, _tx: &mut Tx| ev.key == Key::Escape && ev.mods == rune::Mods::default() && s.escape())
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
            // A paste reaches a terminal only as a key would (a field that
            // has the keyboard takes its own paste inside Rune).
            if !content.text().is_empty() && (crate::surface::focused().is_some() || s.terminal_may_take_keys(tx.focused())) {
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
        // The traffic lights (macOS) sit over this band, centred on its
        // line (`LIGHTS`): nothing goes under them, 12 pt clear after the
        // zoom button (`inset` is from the window's edge, the bar starts a
        // gutter in).
        .child(el().key("traffic-lights-room").w(if inset > 0.0 { inset - layout::GUTTER + 12.0 } else { 4.0 }).h_full())
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .justify_center()
                .pr(8.0)
                .clip_rounded(0.0)
                .on_layout(|s: &mut App, r: [f32; 4]| {
                    // What fits in the top bar follows its width (bar.rs, top_layout).
                    let w = r[2] - 8.0;
                    if (s.top_w - w).abs() > 1.0 {
                        s.top_w = w;
                        s.push_data();
                    }
                })
                .child(top_el),
        );
    // The notice lives IN the top bar, at its right end, part of its layout:
    // it covers nothing (phase 3 pinned it over the bottom right, where the
    // Jarvis input and its buttons are, and took their clicks), and it sits
    // under any modal or core dialog drawn later.
    let bar = match notice_view(s) {
        Some(n) => bar.child(n),
        None => bar,
    };
    // The sidebar: its width remembered, its right edge dragged (200 to 420 pt).
    let edge = el()
        .key("sidebar-edge")
        .pin(None, Some(0.0), Some(-layout::GUTTER), Some(0.0))
        .w(layout::GUTTER)
        .cursor(Cursor::ResizeColumn)
        .hover(c("border"))
        .help("Drag to resize the sidebar")
        .on_drag(|s: &mut App, d: &mut rune::Drag, _tx| {
            if matches!(d.phase, rune::Phase::Down) {
                d.data = s.sidebar_w();
            }
            let w = (d.data + d.dx).clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX);
            if matches!(d.phase, rune::Phase::Up) {
                s.sidebar_live = None;
                s.store.set_in("sidebar", "w", serde_json::json!(w.round()));
            } else {
                s.sidebar_live = Some(w);
            }
            s.push_data();
        });
    let sidebar = tile(col().key("sidebar").w(s.sidebar_w()).h_full().min_h(0.0).padding(layout::SIDEBAR_PAD)).child(side_el).child(edge);
    let mut main = row().grow(1.0).min_h(0.0).w_full().padding(layout::GUTTER).gap(layout::GUTTER);
    if s.sidebar_shown() {
        main = main.child(sidebar);
    }
    main = main.child(panes_view(s));
    for p in fpanels {
        main = main.child(p);
    }
    // Docked on the right: it narrows the panes instead of covering them.
    if let Some(p) = panel {
        main = main.child(p);
    }
    if let Some(m) = s.bar_menu_view() {
        overlays.push(m);
    }
    let mut root = root.child(bar).child(main);
    for o in overlays {
        root = root.child(o);
    }
    if let Some(m) = modal {
        root = root.child(m);
    }
    if let Some(o) = &s.copy_offer {
        root = root.child(approval::copy_offer_view::<App>(o, |s: &mut App, tx: &mut Tx| s.accept_copy_offer(tx), |s: &mut App| s.copy_offer = None));
    }
    if let Some(p) = palette::view(s) {
        root = root.child(p);
    }
    // Jarvis's proposal has no scrim: it stands beside the Jarvis panel,
    // whose input (a typed yes) and push-to-talk must stay usable while it shows.
    if let Some(dialog) = s.dialog_view() {
        root = root.child(dialog);
    }
    // The core's confirmation: a press outside cancels, as its × and Escape do.
    if let Some(dialog) = s.confirm_view() {
        root = root.child(scrim("cf-scrim", dialog, |s: &mut App| approval::ConfirmAnswers::cancel_click(s)));
    }
    root
}

/// What sits under a dialog: the window's floor at 60%, so what is behind
/// reads as inactive but stays recognisable. It is modal for the keyboard
/// and the pointer (Tab stays inside, nothing behind takes a press or a
/// key), Escape and a press on it run `outside`.
pub fn scrim(key: &str, dialog: El, outside: impl Fn(&mut App) + Clone + 'static) -> El {
    let o2 = outside.clone();
    el().key(key.to_string())
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(c("floor").alpha(0.6))
        .modal(false, move |s: &mut App| o2(s))
        .on_press(move |s: &mut App| outside(s))
        .child(dialog)
}

/// The notice (key `notice`): one line of text, its full text as a tooltip,
/// and what a click on it does; clickable only on itself. A × dismisses it.
fn notice_view(s: &App) -> Option<El> {
    let (t, err, _) = s.notice.as_ref()?;
    let what = s.feats.notice_then.as_ref().map(|t| match t.0.as_str() {
        "screen_restore" => "Click to see the file and restore the built-in",
        _ => "Click to open",
    });
    let mut body = col().gap(1.0).grow(1.0).min_w(0.0).child(text(t.clone()).font(small(13.0, 400.0)).color(c("text-bright")).truncate().min_w(0.0));
    if let Some(w) = what {
        body = body.child(text(w).font(small(12.0, 500.0)).color(c("blue")).truncate().min_w(0.0));
    }
    let close = el()
        .key("notice-close")
        .role("button")
        .label("Dismiss")
        .help("Dismiss")
        .w(layout::DENSE_H)
        .h(layout::DENSE_H)
        .items_center()
        .justify_center()
        .radius(theme::radius("xs").unwrap())
        .hover(c("border"))
        .cursor(Cursor::Pointer)
        .on_press(|s: &mut App| {
            s.notice = None;
            s.feats.notice_then = None;
            s.push_data();
        })
        .child(rune::icon(crate::icons::get("close").unwrap(), 12.0).color(c("text-muted")));
    Some(
        row()
            .key("notice")
            .role("status")
            .help(t.clone())
            .gap(6.0)
            .items_center()
            .max_w(380.0f32.min(s.window.0 * 0.35))
            .min_w(0.0)
            .mr(8.0)
            .px(8.0)
            .py(4.0)
            .radius(theme::radius("sm").unwrap())
            .fill(c(if *err { "red-tint" } else { "bg" }))
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
            .child(body)
            .child(close),
    )
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

/// The name a pane bar shows: the sandbox, and its thread when it shows one.
fn pane_name(p: &Pane) -> String {
    match (&p.sandbox, &p.thread) {
        (Some(sb), Some(t)) => format!("{sb} · {t}"),
        (Some(sb), None) => sb.clone(),
        (None, _) => "Empty pane".into(),
    }
}

impl App {
    /// The sidebar's width: dragged, else remembered, else the default.
    pub fn sidebar_w(&self) -> f32 {
        let w = self.sidebar_live.unwrap_or_else(|| self.store.get("sidebar")["w"].as_f64().map_or(layout::SIDEBAR_W, |w| w as f32));
        // A narrow window keeps room for the panes: at most 30% of it, never under the minimum.
        let room = if self.window.0 > 0.0 { (self.window.0 * 0.3).max(layout::SIDEBAR_MIN) } else { layout::SIDEBAR_MAX };
        w.min(room).clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX)
    }

    /// Whether the sidebar shows (Cmd/Ctrl+B hides it; remembered).
    pub fn sidebar_shown(&self) -> bool {
        self.store.get("sidebar")["hidden"] != J::Bool(true)
    }

    pub fn toggle_sidebar(&mut self) {
        let hidden = self.sidebar_shown();
        self.store.set_in("sidebar", "hidden", if hidden { J::Bool(true) } else { J::Null });
        self.push_data();
    }

    /// Pane `i`'s bar width: as last laid out, else a guess from the window
    /// (the first frame; the layout's report corrects it at once).
    fn bar_width(&self, i: usize) -> f32 {
        let w = self.bar_w.get(i).copied().unwrap_or(0.0);
        if w > 0.0 {
            return w;
        }
        let across = match (self.layout.0, self.layout.1) {
            (1, _) | (2, Split::Rows) => 1.0,
            _ => 2.0,
        };
        let jarvis = if self.jarvis.open { self.jarvis.width } else { 0.0 };
        let side = if self.sidebar_shown() { self.sidebar_w() + layout::GUTTER } else { 0.0 };
        ((self.window.0 - side - jarvis - layout::GUTTER * 3.0) / across).max(160.0)
    }

    /// What pane `i`'s bar shows inline and what goes into its overflow
    /// menu (bar.rs): the core's "wants you" and state word with the
    /// features' extras, by priority. None for a pane with no sandbox.
    fn pane_fit(&self, i: usize) -> Option<bar::PaneFit> {
        let p = self.panes.get(i).filter(|p| p.sandbox.is_some())?;
        let core = |id: &str, label: &str, tone: &str, help: &str| bar::Item { id: id.into(), label: label.into(), icon: None, tone: tone.into(), help: help.into(), count: None, intent: None, core: true };
        let mut items = vec![];
        if p.attention {
            items.push(core("attention", "wants you", "amber", "The agent in this pane waits for you"));
        }
        items.extend(self.pane_items(p.sandbox.as_deref().unwrap_or(""), p.thread.as_deref()));
        if p.status() != "connected" {
            items.push(core("status", &p.status(), "muted", "The terminal's connection"));
        }
        Some(bar::pane_fit(self.bar_width(i), &pane_name(p), p.mode.name(), items))
    }

    /// The overflow menu of pane `i`'s bar, anchored under its button.
    fn bar_menu_view(&self) -> Option<El> {
        let i = self.bar_menu?;
        let f = self.pane_fit(i)?;
        if f.hidden.is_empty() {
            return None;
        }
        let n = i + 1;
        let mut list = col()
            .key(format!("pane-{n}-more-list"))
            .role("menu")
            .anchored(format!("pane-{n}-more"), rune::Side::Bottom, rune::Align::End, 4.0)
            .min_w(160.0)
            .max_w(320.0)
            .padding(4.0)
            .gap(2.0)
            .fill(c("surface"))
            .border(1.0, c("border"))
            .radius(theme::radius("md").unwrap())
            .shadow(8.0, 24.0, rune::color::hex("#00000073").unwrap());
        for it in &f.hidden {
            list = list.child(bar_menu_item(it, n));
        }
        Some(list)
    }
}

/// A pane bar chip: an extra's (or the core's) label, icon, tone and count,
/// pressable when it has an intent.
fn bar_chip(it: &bar::Item, n: usize) -> El {
    // Quiet: a muted chip is text on the bar; a tone is its colour on its tint.
    let quiet = it.tone == "muted" || it.tone.is_empty();
    let (fg, tint) = crate::screen::tone(&it.tone);
    let fg = if quiet { theme::color_value("text").unwrap() } else { fg };
    let mut b = row().key(it.key(n)).gap(bar::CHIP_GAP).px(bar::CHIP_PX).h(layout::DENSE_H).min_w(layout::DENSE_H).justify_center().items_center().radius(theme::radius("xs").unwrap());
    if !quiet {
        b = b.fill(theme::hex(tint));
    }
    if !it.help.is_empty() {
        b = b.help(it.help.clone());
    }
    if let Some(ic) = it.icon.as_deref().and_then(crate::icons::get) {
        b = b.child(rune::icon(ic, bar::CHIP_ICON).color(theme::hex(fg)));
    }
    let label = it.text();
    if !label.is_empty() {
        b = b.child(text(label).font(small(bar::CHIP_TEXT, 500.0)).color(theme::hex(fg)));
    }
    if let Some((name, args)) = it.intent.clone() {
        b = b.role("button").label(if it.text().is_empty() { it.help.clone() } else { it.text() }).hover(c(if quiet { "elevated" } else { "active-bg" })).cursor(Cursor::Pointer).on_press(move |s: &mut App| {
            s.press_extra("pane", &name, &args);
            s.push_data();
        });
    }
    b
}

/// One row of a pane bar's overflow menu: the same key, label, icon, tone,
/// count and intent as the chip it stands for.
fn bar_menu_item(it: &bar::Item, n: usize) -> El {
    let (fg, tint) = crate::screen::tone(&it.tone);
    let fg = theme::hex(fg);
    let mut r = row().key(it.key(n)).gap(8.0).items_center().px(8.0).h(layout::CONTROL_H).radius(theme::radius("xs").unwrap()).role("menuitem").label(it.text());
    if !it.help.is_empty() {
        r = r.help(it.help.clone());
    }
    r = match it.icon.as_deref().and_then(crate::icons::get) {
        Some(ic) => r.child(rune::icon(ic, 14.0).color(fg)),
        None => r.child(el().w(8.0).h(8.0).radius(4.0).fill(fg)),
    };
    let name = if it.label.is_empty() { it.help.clone() } else { it.label.clone() };
    r = r.child(text(name).font(small(13.0, 400.0)).color(if it.tone == "muted" || it.tone.is_empty() { c("text-bright") } else { fg }).truncate().grow(1.0).min_w(0.0));
    if let Some(k) = it.count {
        r = r.child(el().px(6.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(theme::hex(tint)).child(text(k.to_string()).font(small(12.0, 500.0)).color(fg)));
    }
    match it.intent.clone() {
        Some((name, args)) => r.focus_highlight(c("elevated")).cursor(Cursor::Pointer).on_press(move |s: &mut App| {
            s.bar_menu = None;
            s.press_extra("pane", &name, &args);
            s.push_data();
        }),
        None => r,
    }
}

/// The overflow button of pane `n`'s bar: "…", how many it holds, and a dot
/// in the tone of the most urgent of them (bar::more_tone).
fn bar_more(f: &bar::PaneFit, i: usize, open: bool) -> El {
    let n = i + 1;
    let count = f.hidden.len();
    let mut b = row()
        .key(format!("pane-{n}-more"))
        .role("button")
        .label(format!("{count} more"))
        .expanded(open)
        .help(format!("{count} more: what does not fit in this bar"))
        .gap(bar::MORE_GAP)
        .px(bar::CHIP_PX)
        .h(layout::DENSE_H)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .fill(c(if open { "elevated" } else { "surface" }))
        .hover(c("elevated"))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut App| {
            s.bar_menu = if s.bar_menu == Some(i) { None } else { Some(i) };
        })
        .child(rune::icon(crate::icons::get("more").unwrap(), bar::CHIP_ICON).color(c("text")))
        .child(text(count.to_string()).font(small(bar::CHIP_TEXT, 500.0)).color(c("text")));
    if let Some(t) = bar::more_tone(&f.hidden) {
        b = b.child(el().w(bar::MORE_DOT).h(bar::MORE_DOT).radius(bar::MORE_DOT / 2.0).fill(theme::hex(crate::screen::tone(t).0)));
    }
    b
}

fn pane_view(s: &App, i: usize) -> El {
    let pane = s.panes.get(i);
    let focused = i == s.focused;
    let label = pane.map(pane_name).unwrap_or_else(|| "Empty pane".into());
    let connected = pane.is_some_and(|p| p.status() == "connected");
    let (dot_tone, dot_word) = if pane.is_some_and(|p| p.attention) { ("amber", "wants you") } else if connected { ("blue", "connected") } else { ("text-muted", "not connected") };
    let dot = el().w(bar::DOT_W).h(bar::DOT_W).radius(bar::DOT_W / 2.0).fill(c(dot_tone)).help(format!("The terminal: {dot_word}"));
    // dot | name, kind, extras…, "…" | close. The middle part is clipped and
    // shrinks; the dot and the close button never move out of view.
    let mut bar = row()
        .key(format!("pane-{}-bar", i + 1))
        .h(layout::PANE_BAR_H)
        .w_full()
        .px(bar::PANE_PAD)
        .gap(bar::PANE_GAP)
        .items_center()
        .fill(c(if focused { "surface" } else { "surface-alt" }))
        .on_layout(move |s: &mut App, r: [f32; 4]| {
            if let Some(w) = s.bar_w.get_mut(i) {
                if (*w - r[2]).abs() > 0.5 {
                    *w = r[2];
                }
            }
        })
        .child(dot);
    let fit = s.pane_fit(i);
    let name_min = fit.as_ref().map_or(0.0, |f| f.name_min);
    let mut mid = row().grow(1.0).min_w(0.0).h_full().gap(bar::PANE_GAP).items_center().clip_rounded(0.0).child(
        text(label.clone()).key(format!("pane-{}-name", i + 1)).help(label).font(small(bar::NAME_TEXT, 500.0)).color(c(if focused { "text-bright" } else { "text" })).truncate().min_w(name_min),
    );
    if let (Some(p), Some(f)) = (pane.filter(|p| p.sandbox.is_some()), fit.as_ref()) {
        mid = mid.child(text(p.mode.name()).font(small(bar::CHIP_TEXT, 400.0)).color(c("text-muted")));
        for it in &f.shown {
            mid = mid.child(bar_chip(it, i + 1));
        }
        if !f.hidden.is_empty() {
            mid = mid.child(bar_more(f, i, s.bar_menu == Some(i)));
        }
    }
    bar = bar.child(mid);
    if pane.is_some_and(|p| p.sandbox.is_some()) {
        bar = bar.child(
            el().key(format!("pane-{}-close", i + 1))
                .role("button")
                .label("Close pane")
                .help("Close this pane (the sandbox and its session go on)")
                .help_keys(palette::close_keys())
                .w(bar::CLOSE_W)
                .h(bar::CLOSE_W)
                .items_center()
                .justify_center()
                .radius(theme::radius("xs").unwrap())
                .hover(c("elevated"))
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut App| {
                    s.remove_pane(i);
                    s.push_data();
                })
                .child(rune::icon(crate::icons::get("close").unwrap(), 14.0).color(c("text-muted"))),
        );
    }
    let body = match pane {
        // The pane fills a row (its width grows, its height is the row's).
        Some(Pane { term: Some(t), .. }) => row().w_full().grow(1.0).min_h(0.0).child(terminal::pane(t, lens(i))),
        Some(Pane { sandbox: None, error: None, .. }) => empty_pane(s, i),
        Some(p) => {
            let msg = match (&p.sandbox, &p.error) {
                (_, Some(e)) => e.clone(),
                (Some(sb), None) if matches!(s.target, Target::Fixture(_)) => format!("{sb}: no terminal in a fixture (scripts/shot.sh runs one against the stand-in daemon)"),
                (Some(sb), None) => format!("{sb}: {}", p.status()),
                (None, None) => unreachable!("the empty pane"),
            };
            let mut body = row().gap(8.0).items_start().w_full().child(text(msg).font(small(13.0, 400.0)).color(c(if p.error.is_some() { "red" } else { "text-muted" })).wrap().grow(1.0).min_w(0.0));
            if p.error.is_some() {
                body = row().gap(8.0).items_start().w_full().child(rune::icon(crate::icons::get("warning").unwrap(), 16.0).color(c("red"))).child(body);
            }
            col().grow(1.0).w_full().padding(16.0).child(body)
        }
        None => el().grow(1.0),
    };
    // The focused pane: a clear 1.5 pt blue border; the others a quiet one.
    // A 2 pt line on top in the sandbox's group colour (the web UI's), so
    // panes of one piece of work read together.
    let accent = pane
        .and_then(|p| p.sandbox.as_deref())
        .and_then(|n| s.sandboxes.iter().find(|x| x.name == n))
        .and_then(|sb| theme::group_color(sb.workspace.as_deref(), sb.chat))
        .map(theme::hex)
        .unwrap_or_else(|| c("border"));
    let (bw, border) = if focused { (1.5, c("blue")) } else { (1.0, c("elevated")) };
    col()
        .key(format!("pane-{}", i + 1))
        .w_full()
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(c("bg"))
        .border(bw, border)
        .radius(theme::radius("md").unwrap())
        .clip_rounded(theme::radius("md").unwrap())
        .on_press(move |s: &mut App| {
            // A press anywhere on a pane gives its terminal the keyboard
            // (keys reach a terminal only through its grid's focus).
            s.refocus = true;
            if s.focused != i {
                s.focused = i;
                s.touch(i);
                s.push_data();
            }
        })
        // Files dropped on a pane: to the feature that takes them (files: the upload's dialog).
        .on_drop_files(move |s: &mut App, files: &[std::path::PathBuf], _tx| s.pane_drop(i, files))
        .child(el().key(format!("pane-{}-accent", i + 1)).h(2.0).w_full().fill(accent))
        .child(bar)
        .child(el().h(1.0).w_full().fill(c("elevated")))
        .child(body)
}

/// A core-drawn button, as the `button` element draws one: 28 pt, a label
/// 13/500, its shortcut in its tooltip.
fn core_button(key: &str, label: &str, icon: &str, keys: Vec<String>, primary: bool, on: impl Fn(&mut App) + 'static) -> El {
    let (fill, fg, hover) = if primary { ("green-deep", "white", "primary-hover") } else { ("elevated", "text", "border") };
    let mut b = row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .help(label.to_string())
        .help_keys(keys)
        .gap(6.0)
        .h(layout::CONTROL_H)
        .px(10.0)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .fill(c(fill))
        .border(1.0, c(if primary { "green-deep" } else { "border" }))
        .hover(c(hover))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut App| {
            on(s);
            s.push_data();
        });
    if let Some(ic) = crate::icons::get(icon) {
        b = b.child(rune::icon(ic, 16.0).color(c(fg)));
    }
    b.child(text(label.to_string()).font(small(13.0, 500.0)).color(c(fg)))
}

/// An empty pane says what to do, centred, and offers it: open a sandbox
/// from the list, a new sandbox, the command palette.
fn empty_pane(s: &App, i: usize) -> El {
    let none = s.sandboxes.is_empty();
    let (title, line) = if none {
        ("No sandboxes yet", "Create one to run an agent in it; its terminal opens here.")
    } else {
        ("Nothing open in this pane", "Pick a sandbox in the list to open its terminal here, or find it in the palette.")
    };
    let mut actions = row().gap(8.0).items_center().justify_center();
    // The create feature's "New" (its top bar extra), the same intent.
    let new = s.screens.iter().find(|x| x.name == "topbar").and_then(|t| {
        let e = t.data()["app"]["extras"]["topbar"].as_array()?.iter().find(|e| e["id"] == "create")?.clone();
        Some((e["intent"]["name"].as_str()?.to_string(), e["intent"]["args"].clone()))
    });
    if let Some((name, args)) = new {
        let keys = if cfg!(target_os = "macos") { vec!["⌃".to_string(), "⌥".into(), "N".into()] } else { vec!["Alt".to_string(), "N".into()] };
        actions = actions.child(core_button(&format!("pane-{}-new", i + 1), "New sandbox", "plus", keys, none, move |s: &mut App| s.press_extra("pane", &name, &args)));
    }
    actions = actions.child(core_button(&format!("pane-{}-palette", i + 1), "Command palette", "search", palette::chord_keys("K"), false, |s: &mut App| s.palette_open()));
    col()
        .key(format!("pane-{}-empty", i + 1))
        .grow(1.0)
        .w_full()
        .padding(24.0)
        .gap(12.0)
        .items_center()
        .justify_center()
        .child(rune::icon(crate::icons::get("terminal").unwrap(), 24.0).color(c("text-muted")))
        .child(text(title).font(small(16.0, 600.0)).color(c("text-bright")))
        .child(text(line).font(small(13.0, 400.0)).color(c("text-muted")).wrap().max_w(360.0))
        .child(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ProposalAnswers;

    fn app() -> App {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        App::new(Args { fixture: Some(fixture.into()), ..Args::default() })
    }

    /// The app in Rune's own runtime, its window `on_key` wired as in
    /// main.rs, a live terminal in pane 1 whose input we read.
    fn window_with_terminal() -> (rune::Runtime<App>, std::sync::mpsc::Receiver<crate::client::TermInput>, rune::clipboard::MemoryClipboard) {
        let mut a = app();
        let (_ev_tx, ev_rx) = std::sync::mpsc::channel();
        let (in_tx, in_rx) = std::sync::mpsc::channel();
        std::mem::forget(_ev_tx);
        let mut t = TerminalState::new(crate::client::TerminalConn { events: ev_rx, input: in_tx });
        t.resize(40, 5);
        t.id = "1".into();
        a.ensure_panes(1);
        a.panes[0].sandbox = Some("web-app".into());
        a.panes[0].term = Some(t);
        a.focused = 0;
        let mut rt = rune::Runtime::new(a, view, (1280.0, 800.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        let clip = rune::clipboard::memory();
        rt.use_clipboard(&clip);
        rt.frame(0.0);
        (rt, in_rx, clip)
    }

    fn sent(rx: &std::sync::mpsc::Receiver<crate::client::TermInput>) -> Vec<u8> {
        let mut out = vec![];
        while let Ok(i) = rx.try_recv() {
            if let crate::client::TermInput::Bytes(b) = i {
                out.extend(b);
            }
        }
        out
    }

    fn tap(rt: &mut rune::Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.016);
    }

    fn chords() -> Vec<KeyEvent> {
        let m = |cmd: bool, ctrl: bool, alt: bool, logo: bool, shift: bool| rune::Mods { cmd, ctrl, alt, logo, shift };
        let mut v = vec![];
        for c in ['a', 'c', 'd', 'u', 'v', 'z'] {
            v.push(KeyEvent { key: Key::Command(c), mods: m(true, true, false, false, false) });
            v.push(KeyEvent { key: Key::Command(c), mods: m(true, false, false, true, false) });
            v.push(KeyEvent { key: Key::Text(c.to_string()), mods: m(false, true, true, false, false) });
            v.push(KeyEvent { key: Key::Text(c.to_string()), mods: m(false, false, true, false, false) });
        }
        for k in [Key::Escape, Key::Tab, Key::Enter, Key::Up, Key::Function(5), Key::Backspace, Key::Text("x".into())] {
            v.push(KeyEvent { key: k.clone(), mods: rune::Mods::default() });
            v.push(KeyEvent { key: k, mods: m(true, true, false, false, true) });
        }
        v
    }

    #[test]
    fn wait_marks_parse_and_see_what_is_drawn() {
        let w = |n: &str| match wait_mark(n) {
            Some(Mark::Wait { key, ms, gone }) => Some((key, ms, gone)),
            _ => None,
        };
        assert_eq!(w("wait:files-row-report.md"), Some(("files-row-report.md".into(), 10_000, false)));
        assert_eq!(w("wait:cf-confirm:2500"), Some(("cf-confirm".into(), 2500, false)));
        assert_eq!(w("wait-gone:cf-dialog"), Some(("cf-dialog".into(), 10_000, true)));
        assert_eq!(w("wait:a:b"), Some(("a:b".into(), 10_000, false)), "a key may hold a colon");
        assert_eq!(w("wait:"), None);
        assert_eq!(w("hold:20"), None);
        let (mut rt, _rx, _clip) = window_with_terminal();
        assert!(rt.state.drawn("filter") && rt.state.drawn("pane-1-bar") && rt.state.drawn("grid-1"));
        assert!(!rt.state.drawn("cf-confirm") && !rt.state.drawn("nothing-of-that-key"));
        // A menu's items count once it is open (the layout switch of the top bar).
        let ids = rt.state.screens.iter().find_map(|sc| sc.last_good.as_ref().and_then(|n| {
            fn first_menu(n: &crate::luau::Node) -> Option<(String, String)> {
                if matches!(n.kind, crate::luau::Kind::Menu | crate::luau::Kind::Select) && !n.items.is_empty() {
                    return Some((n.id.clone()?, n.items[0].id.clone()));
                }
                n.children.iter().find_map(first_menu)
            }
            first_menu(n).map(|m| (sc.name.clone(), m))
        }));
        if let Some((screen, (id, item))) = ids {
            assert!(!rt.state.drawn(&format!("{id}-{item}")), "closed: not drawn");
            rt.update(|s, _| crate::screen::Host::screen(s, &screen).ui.open = Some(id.clone()));
            assert!(rt.state.drawn(&format!("{id}-{item}")) && rt.state.drawn(&format!("{id}-list")), "open: drawn");
            rt.update(|s, _| crate::screen::Host::screen(s, &screen).ui.open = None);
        }
        rt.update(|s, _| {
            let f = App::feature_of("network").unwrap();
            let c = crate::feat::Confirm::new("t", crate::feat::Call::post("/api/x", serde_json::json!({}), "x")).secret("Token", "value");
            s.feature_actions(f, "network", vec![c.into_action()]);
        });
        rt.frame(0.02);
        assert!(rt.state.drawn("cf-confirm") && rt.state.drawn("cf-secret-0") && !rt.state.drawn("cf-secret-1"));
        rt.update(|s, _| {
            s.dismiss_dialog();
        });
        assert!(!rt.state.drawn("cf-dialog"));
    }

    /// The user's screenshot (phase 4, macOS): two panes, each bar with
    /// many extras, the name squeezed to "…" and the close button pushed out.
    /// At the smallest window with 4 panes and at 1400 x 900: every bar's
    /// name and close button are inside the pane, the extras that do not fit
    /// are in one menu whose items keep their keys and run their intents,
    /// and the top bar's parts stay inside the window.
    #[test]
    fn pane_bars_keep_their_name_and_close_button_and_overflow_into_a_menu() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/panebar-4.json");
        for (w, h) in [(720.0, 480.0), (1400.0, 900.0)] {
            let a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
            let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
            rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
            for _ in 0..6 {
                rt.frame(0.016);
            }
            // The plan dialog the fixture opens, then the Requests panel.
            rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
            rt.update(|s, _| {
                s.feats.panel = None;
                s.feats.panels.clear();
            });
            for _ in 0..6 {
                rt.frame(0.016);
            }
            let inside = |rt: &rune::Runtime<App>, key: &str, outer: &str| {
                let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("{w}: {key} drawn"));
                let (o, _) = rt.presented(outer).unwrap();
                assert!(r[0] >= o[0] - 0.5 && r[0] + r[2] <= o[0] + o[2] + 0.5, "{w}: {key} {r:?} out of {outer} {o:?}");
                r
            };
            let mut hidden_total = 0;
            for n in 1..=4 {
                let bar = format!("pane-{n}-bar");
                inside(&rt, &format!("pane-{n}-close"), &bar);
                let name = inside(&rt, &format!("pane-{n}-name"), &bar);
                let full = bar::measure(&pane_name(&rt.state.panes[n - 1]), bar::NAME_TEXT, 500.0);
                assert!(name[2] + 1.0 >= full.min(bar::NAME_MIN), "{w}: pane {n}'s name {name:?}, {full} wide whole");
                let f = rt.state.pane_fit(n - 1).unwrap();
                hidden_total += f.hidden.len();
                for it in &f.shown {
                    inside(&rt, &it.key(n), &bar);
                }
                if !f.hidden.is_empty() {
                    inside(&rt, &format!("pane-{n}-more"), &bar);
                    assert!(rt.state.drawn(&format!("pane-{n}-more")) && !rt.state.drawn(&f.hidden[0].key(n)), "closed: the item is not drawn");
                }
            }
            if w < 1000.0 {
                assert!(hidden_total > 0);
                // Pane 1 (test-sbx): what needs the person is first in its menu.
                let f = rt.state.pane_fit(0).unwrap();
                assert!(f.hidden.iter().chain(&f.shown).take(3).any(|i| i.tone == "amber"), "{:?}", f.hidden);
                tap(&mut rt, "pane-1-more");
                assert!(rt.state.drawn("pane-1-more-list") && rt.state.drawn("pane-1-x-approvals"), "open: drawn, wait: marks see it");
                let (r, _) = rt.presented("pane-1-more-list").unwrap();
                assert!(r[0] >= 0.0 && r[0] + r[2] <= w + 0.5 && r[1] >= 0.0 && r[1] + r[3] <= h + 0.5, "menu {r:?} inside the window");
                // Its item runs the same intent as the chip: the Network panel on test-sbx.
                tap(&mut rt, "pane-1-x-approvals");
                assert_eq!(rt.state.bar_menu, None);
                assert_eq!(rt.state.feats.data.get("network_for").and_then(J::as_str), Some("test-sbx"));
                // Escape closes it too.
                tap(&mut rt, "pane-1-more");
                rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
                rt.frame(0.016);
                assert!(!rt.state.drawn("pane-1-more-list"));
            }
            // The top bar: nothing leaves the window.
            for key in ["jarvis-toggle", "layout"] {
                let (r, _) = rt.presented(key).unwrap();
                assert!(r[0] + r[2] <= w + 0.5, "{w}: {key} {r:?}");
            }
            let top = rt.state.screen_data()["app"]["topbar"].clone();
            assert_eq!(top["compact"], w < 1000.0, "{w}: {top}");
        }
    }

    fn runtime(fixture: &str, w: f32, h: f32) -> rune::Runtime<App> {
        let a = App::new(Args { fixture: Some(format!("{}/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR")).into()), ..Args::default() });
        let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        for _ in 0..6 {
            rt.frame(0.016);
        }
        rt
    }

    /// The top bar's fit is exact: each extra the core measured is as wide
    /// as the screen draws it, and at every width what shows fits (nothing
    /// clipped, nothing past the bar) while the first extra left out would not.
    #[test]
    fn the_top_bar_fit_measures_what_the_screen_draws() {
        for w in [720.0, 900.0, 1100.0, 1284.0, 1400.0, 1800.0] {
            let mut rt = runtime("shell/design-2.json", w, 816.0);
            rt.update(|s, _| {
                s.notice = None;
                s.feats.panels.clear();
                s.feats.panel = None;
            });
            for _ in 0..6 {
                rt.frame(0.016);
            }
            rt.update(|s, _| s.push_data());
            for _ in 0..6 {
                rt.frame(0.016);
            }
            // What the top bar screen last drew from.
            let data = rt.state.screens.iter().find(|x| x.name == "topbar").unwrap().data().clone();
            let list = data["app"]["extras"]["topbar"].as_array().unwrap().clone();
            let (bar_r, _) = rt.presented("top-bar").unwrap();
            for e in list.iter().filter(|e| e["more"] != true) {
                let id = e["id"].as_str().unwrap();
                let (r, _) = rt.presented(&format!("extra-topbar-{id}")).unwrap_or_else(|| panic!("{w}: {id} inline but not drawn"));
                let label = match e["count"].as_u64() {
                    Some(n) if e["label"] == "" => n.to_string(),
                    Some(n) => format!("{} {n}", e["label"].as_str().unwrap()),
                    None => e["label"].as_str().unwrap().to_string(),
                };
                let x = bar::TopExtra { label, icon: !e["icon"].is_null(), button: !e["intent"].is_null(), tool: false };
                assert!((r[2] - x.width()).abs() < 1.0, "{w}: {id} drawn {} wide, measured {}", r[2], x.width());
                assert!(r[0] + r[2] <= bar_r[0] + bar_r[2] + 0.5, "{w}: {id} {r:?} past the bar {bar_r:?}");
            }
            for key in ["jarvis-toggle", "daemon-state", "layout"] {
                let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("{w}: {key}"));
                assert!(r[0] + r[2] <= bar_r[0] + bar_r[2] + 0.5, "{w}: {key} {r:?} past the bar");
            }
            let more = data["app"]["topbar"]["more"].as_u64().unwrap();
            eprintln!("top bar at {w}: {} inline, {more} in the menu, compact {}", list.len() as u64 - more, data["app"]["topbar"]["compact"]);
            if w >= 1284.0 {
                assert_eq!(more, 0, "{w}: everything fits");
            }
        }
    }

    /// The palette opens with the app's chord, filters, runs the same
    /// intents as the list (Enter opens the sandbox's terminal), and closes
    /// on Escape; Cmd/Ctrl+B hides the sidebar, Cmd/Ctrl+2 focuses pane 2.
    /// A terminal keeps plain Ctrl chords off macOS.
    #[test]
    fn the_palette_and_the_chords() {
        let mac = cfg!(target_os = "macos");
        let chord = |c: char, shift: bool| KeyEvent { key: Key::Command(c), mods: rune::Mods { cmd: true, ctrl: !mac, logo: mac, shift, alt: false } };
        let mut rt = runtime("shell/design-2.json", 1284.0, 816.0);
        rt.update(|s, tx| {
            s.notice = None;
            tx.blur();
        });
        rt.key(chord('k', false));
        rt.frame(0.016);
        assert!(rt.state.palette.is_some(), "the chord opens the palette");
        assert!(rt.presented("palette-input").is_some());
        rt.update(|_, tx| assert_eq!(tx.focused(), Some("palette-input"), "its field has the keyboard"));
        for ch in "open docs".chars() {
            rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: rune::Mods::default() });
        }
        rt.frame(0.016);
        let shown = rt.state.palette_shown();
        assert_eq!(shown.first().map(|c| c.id.as_str()), Some("open-docs"), "{:?}", shown.iter().map(|c| &c.id).collect::<Vec<_>>());
        rt.key(KeyEvent { key: Key::Enter, mods: rune::Mods::default() });
        rt.frame(0.016);
        assert!(rt.state.palette.is_none(), "a pick closes it");
        assert!(rt.state.panes.iter().take(rt.state.layout.0).any(|p| p.sandbox.as_deref() == Some("docs")), "Enter ran open_terminal on docs");
        // Escape closes it.
        rt.key(chord('k', false));
        rt.frame(0.016);
        rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
        rt.frame(0.016);
        assert!(rt.state.palette.is_none());
        // The sidebar and the panes.
        rt.update(|_, tx| tx.blur());
        rt.key(chord('b', false));
        rt.frame(0.016);
        assert!(!rt.state.sidebar_shown());
        rt.key(chord('b', false));
        rt.frame(0.016);
        assert!(rt.state.sidebar_shown());
        rt.key(chord('2', false));
        rt.frame(0.016);
        assert_eq!(rt.state.focused, 1);
        // In a terminal (its grid has the keyboard): plain Ctrl+K is the shell's off macOS.
        if !mac {
            let taken = rt.state.app_key(&chord('k', false), Some("grid-1"));
            assert!(!taken, "Ctrl+K in a terminal is readline's");
            assert!(rt.state.app_key(&chord('k', true), Some("grid-1")), "Ctrl+Shift+K is the app's");
        }
    }

    /// Closing a pane removes it: the layout shrinks to the panes that
    /// remain, in their order, the focus goes to the one that takes the
    /// freed place; the last pane becomes the empty pane.
    #[test]
    fn closing_a_pane_removes_it_and_the_others_take_the_room() {
        let mut rt = runtime("shell/design-4.json", 1284.0, 816.0);
        let names = |rt: &rune::Runtime<App>| rt.state.panes.iter().take(rt.state.layout.0).map(|p| p.sandbox.clone().unwrap_or_default()).collect::<Vec<_>>();
        assert_eq!(names(&rt), ["better-checkout", "sbxw-desktop-design", "test-sbx", "docs"]);
        rt.update(|s, _| s.focused = 1);
        rt.frame(0.016);
        tap(&mut rt, "pane-2-close");
        assert_eq!(names(&rt), ["better-checkout", "test-sbx", "docs"]);
        assert_eq!(rt.state.layout.0, 3);
        assert_eq!(rt.state.focused, 1, "the pane that took the freed place has the focus");
        assert_eq!(rt.state.screen_data()["app"]["layout"]["panes"], 3, "the layout switch follows");
        // The sandbox is not stopped and no session ends: only the view went.
        assert!(rt.state.sandboxes.iter().any(|s| s.name == "sbxw-desktop-design" && s.status == "running"));
        tap(&mut rt, "pane-3-close");
        assert_eq!(names(&rt), ["better-checkout", "test-sbx"]);
        assert_eq!(rt.state.focused, 1, "the last pane closed: the one before it");
        tap(&mut rt, "pane-1-close");
        tap(&mut rt, "pane-1-close");
        assert_eq!(rt.state.layout.0, 1);
        assert!(rt.state.panes[0].sandbox.is_none(), "the last pane is the empty pane, not removed");
        assert!(rt.presented("pane-1").is_some());
    }

    /// The browser could not start (phase 4, macOS: no Chromium in the
    /// sandbox): the daemon's words stay; no picture, no "Click the page"
    /// hint; the next steps are buttons to what the app already has.
    #[test]
    fn a_browser_that_cannot_start_offers_the_next_step() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/browser/no-chromium.json");
        let a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        let mut rt = rune::Runtime::new(a, view, (1280.0, 800.0), rune::Env::new(true, 1.0));
        for _ in 0..4 {
            rt.frame(0.016);
        }
        assert!(rt.state.drawn("browser-error") && rt.state.drawn("browser-kits") && rt.state.drawn("browser-recreate"));
        assert!(!rt.state.drawn("browser-hint"), "no hint under a picture that is not there");
        let shown = rt.state.screens.iter().find(|s| s.name == "browser").and_then(|s| s.last_good.clone()).unwrap();
        fn find<'a>(n: &'a crate::luau::Node, f: &dyn Fn(&crate::luau::Node) -> bool) -> Option<&'a crate::luau::Node> {
            if f(n) {
                return Some(n);
            }
            n.children.iter().find_map(|c| find(c, f))
        }
        assert!(find(&shown, &|n| n.kind == crate::luau::Kind::Surface).is_none(), "no surface");
        assert!(find(&shown, &|n| n.key.as_deref() == Some("browser-back")).is_some_and(|n| n.disabled));
        tap(&mut rt, "browser-kits");
        assert_eq!(rt.state.feats.modal.as_ref().map(|(n, a)| (n.as_str(), a.clone())), Some(("setup", J::from("kits"))));
        rt.update(|s, _| s.feats.modal = None);
        rt.frame(0.016);
        tap(&mut rt, "browser-recreate");
        assert_eq!(rt.state.feats.modal.as_ref().map(|(n, a)| (n.as_str(), a.clone())), Some(("sandbox-actions", J::from("web-app"))));
    }

    /// No key and no paste reaches a terminal while anything else has the
    /// keyboard: a screen input, a core dialog and its secret field, a modal.
    #[test]
    fn keys_reach_a_terminal_only_when_its_grid_has_the_keyboard() {
        let (mut rt, rx, clip) = window_with_terminal();
        // Nothing has the keyboard: nothing goes to the terminal either.
        rt.key(KeyEvent { key: Key::Text("q".into()), mods: rune::Mods::default() });
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"", "with nothing focused, no terminal takes a key");
        // The control: its grid has the keyboard, it types.
        tap(&mut rt, "grid-1");
        rt.key(KeyEvent { key: Key::Text("q".into()), mods: rune::Mods::default() });
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"q", "the grid with the keyboard takes it");
        // A screen input (the sidebar's filter) has the keyboard.
        tap(&mut rt, "filter");
        rt.update(|_, tx| assert_eq!(tx.focused(), Some("filter")));
        for ev in chords() {
            rt.key(ev.clone());
            rt.frame(0.02);
            assert_eq!(sent(&rx), b"", "{ev:?} typed in a screen input (or after Escape left it) reached the terminal");
        }
        rt.paste("rm -rf ~");
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"", "a paste into an input reached the terminal");
        // A core dialog with a secret field.
        rt.update(|s, _| {
            let f = App::feature_of("network").unwrap();
            let c = crate::feat::Confirm::new("Save a secret", crate::feat::Call::post("/api/x", serde_json::json!({}), "x")).secret("Token", "value");
            s.feature_actions(f, "network", vec![c.into_action()]);
        });
        rt.frame(0.02);
        rt.update(|_, tx| tx.blur());
        rt.frame(0.02);
        for ev in chords() {
            // Escape closes the dialog (tried last); Enter presses the
            // control Tab reached inside it (its ×): the person's own press.
            if ev.key == Key::Escape || ev.key == Key::Enter {
                continue;
            }
            rt.key(ev.clone());
            rt.frame(0.02);
            assert_eq!(sent(&rx), b"", "{ev:?} with the core dialog up reached the terminal");
            // Tab stays inside the dialog: it is modal for the keyboard.
            rt.update(|_, tx| assert!(tx.focused().is_none_or(|f| f.starts_with("cf-")), "{ev:?} moved the focus out of the dialog to {:?}", tx.focused()));
        }
        rt.paste("hello");
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"", "a paste with the dialog up reached the terminal");
        // In the secret field: Ctrl+V / Cmd+V paste into it, nowhere else.
        tap(&mut rt, "cf-secret-0");
        clip.set_text("s3cret");
        for mods in [rune::Mods { cmd: true, ctrl: true, ..Default::default() }, rune::Mods { cmd: true, logo: true, ..Default::default() }] {
            rt.key(KeyEvent { key: Key::Command('v'), mods });
            rt.frame(0.02);
            assert_eq!(sent(&rx), b"", "a paste in the secret field reached the terminal");
        }
        rt.update(|_, tx| assert_eq!(tx.focused(), Some("cf-secret-0")));
        // The window's paste (what ⌘V becomes on macOS) with the field focused.
        rt.paste("s3cret");
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"", "a window paste in the secret field reached the terminal");
        let got = rt.state.feats.dialog.as_ref().unwrap().secrets[0].text.clone();
        eprintln!("secret field after the pastes: {} characters", got.chars().count());
        // (Keys typed while Tab had put the keyboard in the field went there too.)
        assert!(got.ends_with("s3cret"), "the paste went to the field: {got}");
        rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"");
        // A modal: Escape closes it and sends nothing.
        rt.update(|s, _| s.open_modal("templates", J::Null));
        rt.frame(0.02);
        rt.update(|_, tx| tx.blur());
        rt.key(KeyEvent { key: Key::Command('c'), mods: rune::Mods { cmd: true, ctrl: true, ..Default::default() } });
        // (Escape first leaves a field the modal focused, then closes it.)
        for _ in 0..2 {
            rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
            rt.frame(0.02);
        }
        assert!(rt.state.feats.modal.is_none(), "Escape closed the modal");
        assert_eq!(sent(&rx), b"", "keys with the modal up reached the terminal");
        // Back to the grid: it types again.
        tap(&mut rt, "grid-1");
        rt.key(KeyEvent { key: Key::Text("w".into()), mods: rune::Mods::default() });
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"w");
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

    /// Alt+N (Ctrl+Option+N on macOS) opens New; the host chosen is kept across runs.
    #[test]
    fn alt_n_opens_new_and_the_host_is_remembered() {
        let mut a = app();
        let tx = Tx::default();
        let mut ev = KeyEvent { key: Key::Text("n".into()), mods: Default::default() };
        ev.mods.alt = true;
        ev.mods.ctrl = cfg!(target_os = "macos");
        assert!(a.shortcut(&ev, &tx));
        assert_eq!(a.feats.modal.as_ref().map(|m| m.0.as_str()), Some("create"));
        ev.mods.shift = true;
        assert!(!a.shortcut(&ev, &tx), "Shift: not ours");
        ev.mods.shift = false;
        ev.mods.alt = false;
        assert!(!a.shortcut(&ev, &tx), "no Alt: the terminal's");
        a.intents("test", crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": "create_host", "args": { "host": "build-box" } })).unwrap());
        // No remote host is listed in this fixture: the form falls back to
        // `local`, and what it uses is what is kept.
        assert_eq!(a.store.get("features")["create"]["host"], "local");
        assert_eq!(a.screen_data()["app"]["kept"]["create"]["host"], "local");
    }

    /// `copy_text` writes the clipboard only from the person's press, and says what it copied.
    #[test]
    fn copy_text_runs_only_from_a_press_and_shows_what_it_copied() {
        let mut a = app();
        let copy = |t: &str| Intent::CopyText { text: t.into() };
        // A feature's chain (data, events, answers): refused.
        a.run_intent("feature:relay", copy("rm -rf ~"));
        assert!(a.feats.asks.is_empty());
        assert!(a.notice.as_ref().unwrap().0.contains("only from your own press"));
        // An input's per-keystroke action: refused.
        a.screens.iter_mut().find(|s| s.name == "sandboxes").unwrap().ui.editing = true;
        a.run_intent("sandboxes", copy("x"));
        assert!(a.feats.asks.is_empty());
        a.screens.iter_mut().find(|s| s.name == "sandboxes").unwrap().ui.editing = false;
        // A press: copied at the next poll, the notice shows it, controls visible.
        a.run_intent("sandboxes", copy("echo hi\n\u{1b}[31m"));
        let mut tx = Tx::default();
        a.run_asks(&mut tx);
        assert_eq!(a.notice.as_ref().unwrap().0, "Copied 13 B to the clipboard: echo hi↵␛[31m");
        // Over 100 KB: refused.
        a.run_intent("sandboxes", copy(&"x".repeat(100 * 1024 + 1)));
        assert!(a.feats.asks.is_empty());
        // Reading the clipboard (for a feature's Rust, never a screen) also
        // needs a press: a feature's own chain is refused.
        a.feature_actions(App::feature_of("relay").unwrap(), "feature:relay", vec![crate::feat::Action::ReadClipboard]);
        assert!(a.feats.asks.is_empty());
        a.feature_actions(App::feature_of("relay").unwrap(), "relay", vec![crate::feat::Action::ReadClipboard]);
        assert_eq!(a.feats.asks.len(), 1);
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
