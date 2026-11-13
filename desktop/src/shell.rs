//! The window: the web UI's layout (top bar, sandbox side panel, a pane
//! area of terminals) around two Luau screens, `topbar` and `sandboxes`.
//! The core owns the panes, the daemon link and every intent; screens only
//! draw and ask.
//!
//! Layout, as `assets/app.css` draws it: a dark floor (`#080b0f`), and on it,
//! 6 pt apart, rounded tiles: the header (48 pt), the sidebar, and one tile
//! per pane, each with a 32 pt bar over its terminal.

use crate::approval;
use crate::client::{Daemon, FixtureDaemon, Link, LinkEvent, Sandbox, Target, TerminalRequest, TokenSource};
use crate::fixture::{AppView, Connection, Fixture, LayoutView, PaneView};
use crate::intent::{Intent, Mode, PaneTarget, Risk, Split};
use crate::screen::{self, Host, ScreenState};
use crate::terminal::{self, TerminalState, LINE_HEIGHT, PAD, STATUS_H};
use crate::theme::{self, c, layout};
use rune::{col, el, grid, row, text, Cursor, Cx, El, Key, KeyEvent, TextStyle, Tx, Waker};
use serde_json::Value as J;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The screens the shell shows, in the order they are loaded.
pub const SCREENS: &[&str] = &["topbar", "sandboxes"];

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
}

impl Pane {
    fn empty() -> Pane {
        Pane { sandbox: None, mode: Mode::Agent, thread: None, term: None, error: None }
    }

    fn status(&self) -> String {
        match (&self.sandbox, &self.term, &self.error) {
            (None, _, _) => "empty".into(),
            (_, _, Some(_)) => "error".into(),
            (_, Some(t), _) if t.status.starts_with("closed") => "closed".into(),
            (_, Some(t), _) => t.status.clone(),
            (Some(_), None, None) => "closed".into(),
        }
    }

    fn closed(&self) -> bool {
        self.sandbox.is_some() && self.term.as_ref().is_none_or(|t| t.status.starts_with("closed"))
    }
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
    /// Sandboxes whose start is being watched: name, next recheck step.
    expect: Vec<(String, usize, Instant)>,
    marks: Vec<(f64, Mark)>,
    dump_text: Option<PathBuf>,
    solo: Option<String>,
    start: Option<(String, Mode)>,
    waker: Option<Waker>,
    /// The link's way to wake the UI, set once `setup` has the waker.
    wake_slot: Arc<std::sync::Mutex<Option<Waker>>>,
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
                    Ok(fx) => (Target::Fixture(Arc::new(FixtureDaemon::new(&fx.name, fx.daemon.clone()))), fx.raw.clone(), fx.app.clone()),
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
        let mut app = App {
            target,
            link: None,
            daemon: None,
            connection: Connection::default(),
            sandboxes: vec![],
            listed: false,
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
            expect: vec![],
            marks: vec![],
            dump_text: a.dump_text.clone(),
            solo: a.solo.clone(),
            start: a.sandbox.clone().map(|s| (s, if a.mode.as_deref() == Some("agent") { Mode::Agent } else { Mode::Bash })),
            waker: None,
            wake_slot: Arc::new(std::sync::Mutex::new(None)),
        };
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
            None => SCREENS.iter().map(|s| s.to_string()).collect(),
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
                .map(|(i, p)| PaneView { index: i + 1, sandbox: p.sandbox.clone(), mode: p.mode.name(), focused: i == self.focused, status: p.status() })
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
        crate::fixture::screen_data(&J::Object(daemon), &self.view_model())
    }

    fn push_data(&mut self) {
        let d = self.screen_data();
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
                self.daemon = Some(d);
                // The daemon restarted: panes whose socket closed open again (the
                // daemon keeps the PTY and replays its last 256 KB).
                for i in 0..self.panes.len() {
                    let had = self.panes[i].term.is_some();
                    if had && self.panes[i].closed() && self.panes[i].error.is_none() {
                        let p = &self.panes[i];
                        let (sb, mode, th) = (p.sandbox.clone().unwrap(), p.mode, p.thread.clone());
                        self.open(i, &sb, mode, th);
                    }
                }
            }
            LinkEvent::Down(why) => {
                self.connection = Connection { state: "down", text: why };
                self.daemon = None;
            }
            LinkEvent::Sandboxes(l) => {
                self.listed = true;
                if l == self.sandboxes {
                    return false;
                }
                let was: HashMap<String, String> = self.sandboxes.iter().map(|s| (s.name.clone(), s.status.clone())).collect();
                for sb in &l {
                    if was.get(&sb.name).is_some_and(|w| *w != sb.status) {
                        log::info!("list: {} is now {}", sb.name, sb.status);
                    }
                }
                self.sandboxes = l;
                // A watched start that now shows as running is done.
                let running: Vec<String> = self.sandboxes.iter().filter(|s| s.status == "running").map(|s| s.name.clone()).collect();
                self.expect.retain(|(n, _, _)| !running.contains(n));
            }
            LinkEvent::Bus(ev) => {
                log::debug!("event {} {}", ev.name, ev.data);
                return false;
            }
            LinkEvent::Done { what, result } => match result {
                Ok(()) => self.notify(format!("{what}: done"), false),
                Err(e) => self.notify(format!("{what}: {e}"), true),
            },
        }
        true
    }

    pub fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
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
        if self.notice.as_ref().is_some_and(|n| n.2 <= now) {
            self.notice = None;
            changed = true;
        } else if let Some(n) = &self.notice {
            tx.wake_after(n.2.saturating_duration_since(now));
        }
        let settle = headless().then_some(Duration::from_millis(1500));
        for (i, p) in self.panes.iter_mut().enumerate() {
            if let Some(t) = &mut p.term {
                let ch = t.poll(settle);
                if let (true, true, Some(path)) = (ch, i == self.focused, &self.dump_text) {
                    let _ = std::fs::write(path, format!("{}x{}\n{}\n", t.size.0, t.size.1, t.screen_text().trim_end()));
                }
                changed |= ch;
            }
        }
        if changed {
            self.push_data();
        }
        changed
    }

    pub fn on_key(&mut self, ev: &KeyEvent) -> bool {
        if ev.key == Key::Escape && self.screens.iter_mut().any(|s| s.close_menus()) {
            return true;
        }
        let f = self.focused;
        self.panes.get_mut(f).and_then(|p| p.term.as_mut()).is_some_and(|t| t.on_key(ev))
    }

    /// Opens a terminal in pane `i`.
    fn open(&mut self, i: usize, sandbox: &str, mode: Mode, thread: Option<String>) {
        self.ensure_panes(i + 1);
        let waker = self.waker.clone();
        let p = &mut self.panes[i];
        p.sandbox = Some(sandbox.to_string());
        p.mode = mode;
        p.thread = thread.clone();
        p.error = None;
        p.term = None;
        let req = TerminalRequest { sandbox: sandbox.to_string(), mode: mode.query(), thread };
        let r = match (&self.daemon, waker) {
            (Some(d), Some(w)) => d.open_terminal(&req, w).map_err(|e| format!("{e:#}")),
            (None, _) => Err(format!("not connected: {}", self.connection.text)),
            (_, None) => Err("not started".into()),
        };
        match r {
            Ok(conn) => self.panes[i].term = Some(TerminalState::new(conn)),
            Err(e) => self.panes[i].error = Some(e),
        }
        let running = self.sandboxes.iter().any(|s| s.name == sandbox && s.status == "running");
        if mode == Mode::Agent && !running && self.panes[i].error.is_none() {
            self.expect.retain(|(n, _, _)| n != sandbox);
            self.expect.push((sandbox.to_string(), 0, Instant::now() + Duration::from_millis(RUNNING_RECHECK[0])));
        }
    }

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
            PaneTarget::Spare => match (0..self.layout.0).find(|&i| self.panes.get(i).is_none_or(|p| p.sandbox.is_none())) {
                Some(i) => i,
                None if self.layout.0 < 4 => {
                    self.layout.0 += 1;
                    self.layout.0 - 1
                }
                None => self.focused,
            },
        }
    }

    /// Runs one checked intent.
    pub fn run_intent(&mut self, from: &str, intent: Intent) {
        log::info!("intent from {from}: {intent:?}");
        match intent {
            Intent::OpenTerminal { sandbox, mode, thread, pane } => {
                let i = self.pane_for(&pane);
                self.open(i, &sandbox, mode, thread);
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
            Intent::SetLayout { panes, split } => {
                self.layout = (panes, split);
                self.ensure_panes(panes);
                if self.focused >= panes {
                    self.focused = 0;
                }
            }
        }
    }
}

impl Host for App {
    fn screen(&mut self, name: &str) -> &mut ScreenState {
        self.screens.iter_mut().find(|s| s.name == name).expect("screen exists")
    }

    fn intents(&mut self, screen: &str, intents: Vec<Intent>) {
        for i in intents {
            match i.risk() {
                Risk::Direct => self.run_intent(screen, i),
                Risk::Approval(asks) => {
                    // Wave 4 draws the dialog (approval.rs); until then, refuse.
                    self.approvals.push(approval::Pending { screen: screen.into(), intent: i, asks });
                    self.approvals.clear();
                    self.notify(format!("{screen}: '{asks}' needs an approval dialog, which is not built yet: refused"), true);
                }
            }
        }
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
    let root = col().w_full().h_full().fill(c("floor"));
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
    let main = row().grow(1.0).min_h(0.0).w_full().padding(layout::GUTTER).gap(layout::GUTTER).child(sidebar).child(panes_view(s));
    let mut root = root.child(bar).child(main);
    for o in overlays {
        root = root.child(o);
    }
    if let Some((t, err, _)) = &s.notice {
        root = root.child(
            el().key("notice")
                .pin(None, None, Some(16.0), Some(16.0))
                .max_w(420.0)
                .px(12.0)
                .py(8.0)
                .radius(theme::radius("sm").unwrap())
                .fill(c(if *err { "red-tint" } else { "surface" }))
                .border(1.0, c(if *err { "red" } else { "border" }))
                .cursor(Cursor::Pointer)
                .on_press(|s: &mut App| s.notice = None)
                .child(text(t.clone()).font(small(12.0, 400.0)).color(c("text-bright")).wrap()),
        );
    }
    if let Some(dialog) = approval::view::<App>(&s.approvals) {
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
        .child(el().w(7.0).h(7.0).radius(3.5).fill(c(if connected { "blue" } else { "dim" })))
        .child(text(label).font(small(12.0, 500.0)).color(c("text-bright")).truncate().min_w(0.0));
    if let Some(p) = pane.filter(|p| p.sandbox.is_some()) {
        bar = bar.child(el().px(5.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(c("elevated")).child(text(p.mode.name()).font(small(10.5, 400.0)).color(c("text-muted"))));
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
        Some(Pane { term: Some(t), .. }) => terminal_view(t, i),
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
                s.push_data();
            }
        })
        .child(bar)
        .child(el().h(1.0).w_full().fill(c(if focused { "blue" } else { "elevated" })))
        .child(body)
}

fn terminal_view(t: &TerminalState, i: usize) -> El {
    let (cols, rows) = t.size;
    let cw = terminal::cell_width();
    let mut g = grid(cols as u16, cw, LINE_HEIGHT, t.rows.clone()).key(format!("grid-{}", i + 1)).w(cols as f32 * cw).h(rows as f32 * LINE_HEIGHT);
    if let Some((cc, r)) = t.cursor {
        g = g.grid_caret(cc, r);
    }
    col()
        .key(format!("terminal-{}", i + 1))
        .grow(1.0)
        .w_full()
        .min_w(0.0)
        .min_h(0.0)
        .padding(PAD)
        .fill(terminal::rgb(terminal::BG))
        .focusable(true)
        .on_key(move |s: &mut App, ev: &KeyEvent, _tx: &mut Tx| s.panes.get_mut(i).and_then(|p| p.term.as_mut()).is_some_and(|t| t.on_key(ev)))
        .on_layout(move |s: &mut App, r: [f32; 4]| {
            if let Some(t) = s.panes.get_mut(i).and_then(|p| p.term.as_mut()) {
                t.fit(r[2], r[3]);
            }
        })
        .child(el().h(STATUS_H).child(text(format!("terminal {cols}x{rows}  {}", t.status)).font(small(11.0, 400.0)).color(c("text-dim"))))
        .child(g)
}
