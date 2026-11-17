//! A screen: a Luau file drawn by the core with Rune elements, reloaded
//! when its file changes. A reload keeps the screen's state (it lives in the
//! VM); a file that fails to load or render keeps the last good view and
//! shows the error above it.
//!
//! **Where a screen comes from.** Built-in screens are compiled into the
//! binary ([`BUILTIN`]). A file with the same name in the override folder
//! (`~/.sbxw/desktop/screens/<name>.luau`, or `$SBXW_DESKTOP_SCREENS`) wins
//! and is hot reloaded; deleting it brings the built-in back. A watcher
//! thread stats the file every 150 ms (no file I/O on the UI thread during
//! frames, as Rune asks) and sends new sources to [`ScreenState::poll`].
//!
//! **What a screen may do.** Draw the frozen element set (`luau.rs`,
//! `docs/screens.md`) and return intents from its actions. The core holds
//! the parts of elements Luau must not drive itself (an input's text while
//! typed, a split's ratio, which menu is open) in [`Ui`].

use crate::intent::{self, Intent};
use crate::luau::{Align, Item, Justify, Kind, Limits, LuauHost, Node, Size, Style};
use crate::theme::{self, c, hex};
use rune::{col, el, row, text, text_input, Cursor, El, ScrollSpec, Side, TextEdit, TextStyle};
use serde_json::Value as J;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Screens compiled into the binary: `(name, source)`.
pub const BUILTIN: &[(&str, &str)] = &[
    ("sandboxes", include_str!("../screens/sandboxes.luau")),
    ("topbar", include_str!("../screens/topbar.luau")),
    ("jarvis", include_str!("../screens/jarvis.luau")),
];

pub fn builtin(name: &str) -> Option<&'static str> {
    all_builtin().find(|(n, _)| *n == name).map(|(_, s)| s)
}

/// The core's screens, then every feature's (`src/feat/`).
pub fn all_builtin() -> impl Iterator<Item = (&'static str, &'static str)> {
    BUILTIN.iter().copied().chain(crate::feat::screens())
}

/// The override folder: `$SBXW_DESKTOP_SCREENS`, else `~/.sbxw/desktop/screens`.
pub fn override_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("SBXW_DESKTOP_SCREENS").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".sbxw").join("desktop").join("screens")
}

/// What the screen is running now.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Builtin,
    File(PathBuf),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Source::Builtin => write!(f, "built-in"),
            Source::File(p) => write!(f, "{}", p.display()),
        }
    }
}

/// What the screen's app needs from it: its screens, and running intents.
pub trait Host: Sized + 'static {
    fn screen(&mut self, name: &str) -> &mut ScreenState;
    /// A screen's action returned these (already checked against the closed list).
    fn intents(&mut self, screen: &str, intents: Vec<Intent>);
    /// An event on a feature's picture surface (`surface{ id }`): to its owner.
    fn surface_event(&mut self, ev: crate::surface::SurfaceEvent, tx: &mut rune::Tx) {
        let _ = (ev, tx);
    }
    /// Whether a key on a surface is the app's (see [`crate::surface::view`]).
    fn surface_key(&mut self, ev: &rune::KeyEvent, down: bool, tx: &mut rune::Tx) -> bool {
        crate::surface::reserved(ev, &[], false) && { let _ = (down, tx); true }
    }
}

/// Hover zones (`hover_zone`): which one the pointer rests in, with intent
/// delays so a pointer sweeping a list opens nothing. A zone becomes active
/// [`ZONE_OPEN`] after the pointer MOVED inside it (an element that slides
/// under a still pointer, because something above it opened, does not
/// count), and stops [`ZONE_CLOSE`] after it left every element of the zone
/// (moving between two of them is no leave). One zone is active at a time.
#[derive(Default, Debug)]
pub struct Zones {
    /// Per zone, its elements the pointer is over.
    inside: HashMap<String, std::collections::HashSet<String>>,
    pub active: Option<String>,
    /// What becomes active (none: nothing), and when.
    pending: Option<(Option<String>, Instant)>,
}

pub const ZONE_OPEN: Duration = Duration::from_millis(180);
pub const ZONE_CLOSE: Duration = Duration::from_millis(280);

impl Zones {
    fn over(&self, zone: &str) -> bool {
        self.inside.get(zone).is_some_and(|s| !s.is_empty())
    }

    /// The pointer entered or left element `el` of `zone` (a layout shift
    /// included).
    pub fn hover(&mut self, zone: &str, el: &str, inside: bool, now: Instant) {
        let set = self.inside.entry(zone.to_string()).or_default();
        if inside {
            set.insert(el.to_string());
        } else {
            set.remove(el);
        }
        if self.over(zone) {
            if self.active.as_deref() == Some(zone) && matches!(self.pending, Some((None, _))) {
                self.pending = None;
            }
            return;
        }
        match &self.pending {
            // Left before it opened.
            Some((Some(z), _)) if z == zone => self.pending = self.active.is_some().then_some((None, now + ZONE_CLOSE)),
            _ if self.active.as_deref() == Some(zone) && self.pending.is_none() => self.pending = Some((None, now + ZONE_CLOSE)),
            _ => {}
        }
    }

    /// The pointer moved inside `zone`.
    pub fn moved(&mut self, zone: &str, now: Instant) {
        if self.active.as_deref() == Some(zone) {
            if matches!(self.pending, Some((None, _))) {
                self.pending = None;
            }
            return;
        }
        if !matches!(&self.pending, Some((Some(z), _)) if z == zone) {
            self.pending = Some((Some(zone.to_string()), now + ZONE_OPEN));
        }
    }

    /// Applies what is due; whether the active zone changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.pending.clone() {
            Some((to, at)) if at <= now => {
                self.pending = None;
                let changed = self.active != to;
                self.active = to;
                changed
            }
            _ => false,
        }
    }

    /// When [`Zones::tick`] has something to do.
    pub fn due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| p.1)
    }
}

/// The core-held part of elements.
#[derive(Default)]
pub struct Ui {
    /// `input` id: the edit Rune drives, and the last `value` the screen gave.
    pub edits: HashMap<String, (TextEdit, Option<String>)>,
    /// `split` id: ratio of the first child, and the container's length along the split.
    pub splits: HashMap<String, (f32, f32)>,
    /// The `menu` or `select` whose list is open.
    pub open: Option<String>,
    /// An input's per-keystroke `on` action is running: what it returns
    /// was not asked for by the person (no Enter, no press). Intents that
    /// act on what the person typed refuse to answer a proposal then.
    pub editing: bool,
    pub zones: Zones,
}

impl Ui {
    /// The text the person typed in input `id`, when it is theirs: the
    /// screen did not set it through `value`. `None` otherwise.
    pub fn typed(&self, id: &str) -> Option<String> {
        let (edit, given) = self.edits.get(id)?;
        (given.as_deref() != Some(edit.text.as_str())).then(|| edit.text.clone())
    }

    /// Empties input `id` (the core took its text).
    pub fn clear(&mut self, id: &str) {
        if let Some((edit, _)) = self.edits.get_mut(id) {
            edit.replace_text(String::new());
        }
    }
}

pub struct ScreenState {
    pub name: String,
    host: LuauHost,
    pub source: Source,
    override_path: PathBuf,
    /// What the core hands the screen, as JSON (see `docs/screens.md`, "Data").
    data: J,
    pub last_good: Option<Node>,
    pub error: Option<String>,
    pub loads: u32,
    pub ui: Ui,
    /// The override file's mtime when the screen was first read: the watcher
    /// starts from it, so an edit made before the watcher runs is not missed.
    seen: Option<SystemTime>,
    sources: Option<mpsc::Receiver<(Source, Result<String, String>)>>,
    /// The running override's hash (`shell/store.rs`, `hash`); `None` on the built-in.
    pub override_hash: Option<String>,
    /// Set by the core when this override was made for an older built-in:
    /// a marker is drawn over the screen, its press asks to restore.
    pub stale: bool,
    /// `loads` when the core last checked the override (shell.rs).
    pub checked: u32,
}

impl ScreenState {
    /// Loads screen `name`: the override file if there is one, else the built-in.
    pub fn new(name: &str, dir: &Path, data: J) -> Self {
        let override_path = dir.join(format!("{name}.luau"));
        let mut v = Self {
            name: name.to_string(),
            host: LuauHost::new(Limits::default()).expect("Luau VM"),
            source: Source::Builtin,
            override_path,
            data,
            last_good: None,
            error: None,
            loads: 0,
            ui: Ui::default(),
            seen: None,
            sources: None,
            override_hash: None,
            stale: false,
            checked: 0,
        };
        v.seen = std::fs::metadata(&v.override_path).and_then(|m| m.modified()).ok();
        let (src, text) = read_source(name, &v.override_path);
        v.load(src, text);
        v
    }

    /// Starts the watcher thread; `wake` asks the UI to call [`Self::poll`].
    pub fn watch(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        let (tx, rx) = mpsc::channel();
        self.sources = Some(rx);
        let (name, path, mut seen) = (self.name.clone(), self.override_path.clone(), self.seen);
        std::thread::Builder::new()
            .name(format!("watch-{name}"))
            .spawn(move || {
                let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                loop {
                    std::thread::sleep(Duration::from_millis(150));
                    let m = mtime(&path);
                    if m != seen {
                        seen = m;
                        if tx.send(read_source(&name, &path)).is_err() {
                            break;
                        }
                        wake();
                    }
                }
            })
            .expect("watcher thread");
    }

    /// Loads what the watcher sent. True when the view changed.
    pub fn poll(&mut self) -> bool {
        let mut latest = None;
        if let Some(rx) = &self.sources {
            while let Ok(src) = rx.try_recv() {
                latest = Some(src);
            }
        }
        match latest {
            Some((src, text)) => {
                self.load(src, text);
                true
            }
            None => false,
        }
    }

    /// Loads `path` as this screen now (headless script marks `screen:<name>=<path>`).
    pub fn load_file(&mut self, path: PathBuf) {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        self.load(Source::File(path), text);
    }

    fn load(&mut self, source: Source, src: Result<String, String>) {
        let file = match &source {
            Source::Builtin => format!("{}.luau", self.name),
            Source::File(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let hash = match (&source, &src) {
            (Source::File(_), Ok(t)) => Some(crate::shell::store::hash(t)),
            _ => None,
        };
        let loaded = src.map_err(|e| format!("{file}: {e}")).and_then(|src| self.host.load(&file, &src));
        self.loads += 1;
        match loaded {
            Ok(()) => {
                log::info!("screen {} loaded from {source} (load #{})", self.name, self.loads);
                self.override_hash = hash;
                self.source = source;
                self.error = None;
                self.redraw();
            }
            Err(e) => {
                log::warn!("screen {}: {e}", self.name);
                self.error = Some(e);
            }
        }
    }

    pub fn data(&self) -> &J {
        &self.data
    }

    /// Where this screen's override is read from (it may not exist).
    pub fn override_file(&self) -> &Path {
        &self.override_path
    }

    pub fn set_data(&mut self, data: J) {
        if data != self.data {
            self.data = data;
            self.redraw();
        }
    }

    /// Hover zones: what became due. Whether the screen drew again (the
    /// active zone changed; it reads it in `data.app.hover_zone`).
    pub fn tick_zones(&mut self, now: Instant) -> bool {
        if self.ui.zones.tick(now) {
            self.redraw();
            return true;
        }
        false
    }

    fn redraw(&mut self) {
        let with_zone;
        let data = match &self.ui.zones.active {
            Some(z) => {
                let mut d = self.data.clone();
                d["app"]["hover_zone"] = J::String(z.clone());
                with_zone = d;
                &with_zone
            }
            None => &self.data,
        };
        match self.host.render(data) {
            Ok(n) => {
                sync_ui(&mut self.ui, &n);
                self.last_good = Some(n);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("render: {e}")),
        }
    }

    /// Runs an action; returns the intents it asked for, checked. On any
    /// error (in the action, or an intent not in the closed list) the error
    /// is shown and nothing is returned.
    pub fn act(&mut self, action: &str, arg: &J, value: &J) -> Option<Vec<Intent>> {
        let out = self.host.act(action, arg, value).and_then(|v| intent::parse(&v).map_err(|e| format!("intent refused, nothing done: {e}")));
        let r = match out {
            Ok(i) => Some(i),
            Err(e) => {
                log::warn!("screen {}: {e}", self.name);
                self.redraw();
                self.error = Some(e);
                return None;
            }
        };
        self.redraw();
        r
    }

    /// Closes an open menu (Escape). True when one was open.
    pub fn close_menus(&mut self) -> bool {
        self.ui.open.take().is_some()
    }
}

fn read_source(name: &str, path: &Path) -> (Source, Result<String, String>) {
    match std::fs::read_to_string(path) {
        Ok(s) => (Source::File(path.to_path_buf()), Ok(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match builtin(name) {
            Some(s) => (Source::Builtin, Ok(s.to_string())),
            None => (Source::Builtin, Err(format!("no built-in screen '{name}' and no {}", path.display()))),
        },
        Err(e) => (Source::File(path.to_path_buf()), Err(e.to_string())),
    }
}

/// Keeps the core-held parts in step with what the screen rendered.
fn sync_ui(ui: &mut Ui, n: &Node) {
    match n.kind {
        Kind::Input | Kind::Textarea => {
            let id = n.id.clone().unwrap_or_default();
            let want = n.value.as_str().map(str::to_string);
            let e = ui.edits.entry(id).or_insert_with(|| (TextEdit::new(want.clone().unwrap_or_default()), want.clone()));
            // The screen changed the value itself (cleared it after a submit, say): follow it.
            if want != e.1 {
                if let Some(w) = &want {
                    if *w != e.0.text {
                        e.0.replace_text(w.clone());
                    }
                }
                e.1 = want;
            }
        }
        Kind::Split => {
            let id = n.id.clone().unwrap_or_default();
            let r = n.value.as_f64().unwrap_or(0.5) as f32;
            ui.splits.entry(id).or_insert((r, 0.0));
        }
        _ => {}
    }
    for k in &n.children {
        sync_ui(ui, k);
    }
}

// ---------------------------------------------------------------------------
// Drawing

// ---------------------------------------------------------------------------
// Menus: one look for every list that drops from a button (a screen's menu
// and select, the pane bars' overflow, the layout picker, the row's ⋯).

/// A menu's padding all round, its rows' height and padding, the icon's
/// column and the gap after it, the widest it grows.
pub const MENU_PAD: f32 = 6.0;
pub const MENU_ROW_H: f32 = 28.0;
pub const MENU_ROW_PX: f32 = 8.0;
pub const MENU_ICON: f32 = 14.0;
pub const MENU_ICON_GAP: f32 = 12.0;
pub const MENU_MIN_W: f32 = 160.0;
pub const MENU_MAX_W: f32 = 320.0;

/// One row of a menu as the shared look draws it.
pub struct MenuRow<'a> {
    pub key: String,
    pub label: &'a str,
    pub icon: Option<&'a str>,
    /// `red` is destructive: its icon and label in red. Any other tone colours the icon.
    pub tone: Option<&'a str>,
    pub help: Option<&'a str>,
    pub keys: &'a [String],
    pub count: Option<u64>,
    pub chosen: bool,
    pub disabled: bool,
}

/// About how wide a menu's text is: the UI is monospace, 0.6 em a character.
fn chars_w(t: &str, size: f32) -> f32 {
    t.chars().count() as f32 * size * 0.6
}

/// The width a menu needs for its longest row (label, keycaps, count,
/// check), between [`MENU_MIN_W`] and [`MENU_MAX_W`]. A row longer than
/// that truncates, and its tooltip has the whole label.
pub fn menu_width(rows: &[MenuRow]) -> f32 {
    let w = rows.iter().map(|r| {
        let keys: f32 = r.keys.iter().map(|k| chars_w(k, 12.0).max(10.0) + 10.0 + 4.0).sum::<f32>() + if r.keys.is_empty() { 0.0 } else { 12.0 };
        let count = r.count.map_or(0.0, |c| chars_w(&c.to_string(), 12.0) + 12.0 + 8.0);
        MENU_ROW_PX * 2.0 + MENU_ICON + MENU_ICON_GAP + chars_w(r.label, 13.0) + keys + count + if r.chosen { 20.0 } else { 0.0 }
    }).fold(0.0, f32::max);
    (w + MENU_PAD * 2.0 + 2.0).ceil().clamp(MENU_MIN_W, MENU_MAX_W)
}

/// The list's surface, anchored under (or beside) `anchor`, `w` wide; it
/// fades in over 0.1 s (at once under reduced motion).
pub fn menu_surface(key: String, anchor: String, align: rune::Align, w: f32) -> El {
    let e = col()
        .key(key)
        .role("menu")
        .focus_group()
        .anchored(anchor, Side::Bottom, align, 4.0)
        .w(w)
        .padding(MENU_PAD)
        .gap(0.0)
        .fill(c("surface"))
        .border(1.0, c("border"))
        .radius(theme::radius("md").unwrap())
        .shadow(8.0, 24.0, rune::color::hex("#00000073").unwrap());
    if reduced_motion() {
        e
    } else {
        e.transition(quick_fade())
    }
}

/// Menus and the palette: in over 0.1 s, out over 0.08 s.
pub fn quick_fade() -> rune::Transition {
    rune::Transition::opacity().springs(rune::SpringPair::new(rune::Spring::new(0.1, 0.0), rune::Spring::new(0.08, 0.0)))
}

/// A line between two groups of a menu's rows.
pub fn menu_divider() -> El {
    el().w_full().py(4.0).child(el().w_full().h(1.0).fill(c("elevated")))
}

/// One row: the icon in one aligned column (an empty slot when it has none),
/// 12 pt to the label, keycaps or a count at the right end; a red row is
/// destructive. It is a menu item for screen readers, highlighted on hover
/// and keyboard focus (with the focus ring), and its tooltip is its help,
/// else its whole label (in case it truncates).
pub fn menu_row(r: &MenuRow) -> El {
    let danger = r.tone == Some("red");
    let icon_c = match r.tone {
        Some(t) if t != "neutral" => hex(action_tone(t).0),
        _ => c("text-muted"),
    };
    let label_c = if danger { c("red-text") } else { c("text-bright") };
    let mut row_ = row().key(r.key.clone()).w_full().h(MENU_ROW_H).px(MENU_ROW_PX).gap(0.0).items_center().radius(theme::radius("xs").unwrap()).role("menuitem").label(r.label.to_string()).focusable(!r.disabled);
    let slot = el().w(MENU_ICON).h(MENU_ICON).items_center().justify_center();
    row_ = row_.child(match r.icon.and_then(crate::icons::get) {
        Some(i) => slot.child(rune::icon(i, MENU_ICON).color(icon_c)),
        None => slot,
    });
    row_ = row_.child(el().w(MENU_ICON_GAP));
    row_ = row_.child(text(r.label.to_string()).font(body(13.0, if r.chosen { 600.0 } else { 400.0 })).color(label_c).truncate().min_w(0.0).grow(1.0));
    if let Some(n) = r.count {
        row_ = row_.child(el().w(8.0)).child(el().px(6.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(c("elevated")).child(text(n.to_string()).font(body(12.0, 500.0)).color(c("text"))));
    }
    if !r.keys.is_empty() {
        let mut caps = row().gap(4.0).items_center().pl(12.0);
        for k in r.keys {
            caps = caps.child(el().h(18.0).min_w(18.0).px(5.0).items_center().justify_center().radius(4.0).fill(c("bg")).border(1.0, c("border")).child(text(k.clone()).font(body(12.0, 500.0)).color(c("text-muted"))));
        }
        row_ = row_.child(caps);
    }
    if r.chosen {
        row_ = row_.child(el().w(8.0)).child(rune::icon(crate::icons::get("check").unwrap(), 12.0).color(c("blue")));
    }
    // Its help, else its whole label when the label may be cut.
    let cut = chars_w(r.label, 13.0) > MENU_MAX_W - MENU_PAD * 2.0 - MENU_ROW_PX * 2.0 - MENU_ICON - MENU_ICON_GAP - 24.0;
    if let Some(h) = r.help.map(str::to_string).or_else(|| cut.then(|| r.label.to_string())) {
        row_ = row_.help(h);
    }
    if r.disabled {
        return row_.opacity(theme::DISABLED_OPACITY).disabled(true);
    }
    row_.hover(c(if danger { "red-tint" } else { "elevated" })).focus_highlight(c(if danger { "red-tint" } else { "elevated" })).focus_ring(2.0, -2.0, c("focus")).cursor(Cursor::Pointer)
}

/// What sits under an open menu: nothing to see, the whole window, so a
/// press anywhere outside the menu closes it and does nothing else (as a
/// native menu does): it never reaches a terminal, a button or a row.
pub fn menu_scrim<S: 'static>(key: String, close: impl Fn(&mut S) + 'static) -> El {
    el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).on_press(close)
}

/// An action's colour by meaning (docs/screens.md, "Colour by meaning"):
/// its icon or label colour and the tint under it on hover and focus.
/// Neutral is the plain text colour on the quiet hover.
pub fn action_tone(name: &str) -> (u32, u32) {
    let v = |n| theme::color_value(n).unwrap();
    match name {
        "green" => (v("green"), v("green-tint")),
        "red" => (v("red"), v("red-tint")),
        "amber" => (v("amber"), v("amber-tint")),
        "blue" => (v("blue"), v("blue-tint")),
        "purple" => (v("purple"), v("purple-tint")),
        _ => (v("text"), v("border")),
    }
}

/// The app's one motion curve for things that open and shut (a sandbox's
/// sessions, a group, a menu's fade): 0.18 s, no bounce, settling like an
/// ease-out. Only a press or a key starts it (Rune plays nothing for data
/// that arrives); never a loop.
pub const FOLD: rune::Spring = rune::Spring::new(0.18, 0.0);

thread_local! {
    static REDUCED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The system asks for reduced motion (`Cx::is_reduced_motion`, set by the
/// shell's view each frame): folds land at once, menus show without a fade.
pub fn set_reduced_motion(on: bool) {
    REDUCED.with(|r| r.set(on));
}
pub fn reduced_motion() -> bool {
    REDUCED.with(|r| r.get())
}

pub fn tone(name: &str) -> (u32, u32) {
    // (foreground, tint)
    let v = |n| theme::color_value(n).unwrap();
    match name {
        "blue" => (v("blue"), v("blue-tint")),
        "green" => (v("green"), v("green-tint")),
        "amber" => (v("amber"), v("amber-tint")),
        "red" => (v("red"), v("red-tint")),
        "purple" => (v("purple"), v("purple-tint")),
        _ => (v("text-muted"), v("elevated")),
    }
}

/// The app's text: `size` on the type scale, `weight` 400, 500 or 600
/// (700 is drawn 600), the theme's line height.
pub fn body(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight.min(600.0)).lh((size * theme::LINE_HEIGHT).round())
}

/// The panel for screen `name`, drawn from its last good view, with its
/// open menus inside it (a screen drawn alone).
pub fn view<S: Host>(s: &ScreenState) -> El {
    let (mut panel, overlays) = view_parts::<S>(s);
    for o in overlays {
        panel = panel.child(o);
    }
    panel
}

/// The panel, and its open menus apart: the shell puts those last in the
/// window so they paint over every other tile.
pub fn view_parts<S: Host>(s: &ScreenState) -> (El, Vec<El>) {
    let name: Rc<str> = s.name.as_str().into();
    let mut b = Build { name: name.clone(), ui: &s.ui, overlays: vec![], nav: HashMap::new() };
    // The top bar is one line: what the core adds goes beside it, not under it.
    let mut panel = if s.name == "topbar" { row().items_center() } else { col() }.key(format!("screen-{}", s.name)).w_full().h_full().min_h(0.0).gap(8.0);
    if let Some(e) = &s.error {
        panel = panel.child(
            el().key(format!("screen-error-{}", s.name)).w_full().padding(12.0).radius(theme::radius("sm").unwrap()).fill(c("red-tint")).border(1.0, c("red")).child(
                text(format!("Screen error, showing the last good view:\n{e}")).font(body(12.0, 400.0)).color(c("text-bright")).wrap().w_full(),
            ),
        );
    }
    if s.stale {
        panel = panel.child(stale_marker::<S>(&s.name));
    }
    if let Some(n) = &s.last_good {
        panel = panel.child(b.node::<S>(n));
    }
    let missing = s.undrawn_extras();
    if !missing.is_empty() {
        panel = panel.child(fallback_extras::<S>(&s.name, &missing));
    }
    (panel, b.overlays)
}

/// The press of a core-drawn element: an intent, checked as any.
fn press_intent<S: Host>(s: &mut S, screen: &str, name: &str, args: &J) {
    match intent::parse(&serde_json::json!({ "kind": "intent", "name": name, "args": args })) {
        Ok(list) => s.intents(screen, list),
        Err(e) => log::warn!("{screen}: {e}"),
    }
}

/// The persistent marker on an override made for an older built-in: its
/// press opens the core's dialog to restore the built-in (`screen_restore`).
fn stale_marker<S: Host>(name: &str) -> El {
    let screen: Rc<str> = name.into();
    let line = if name == "topbar" { "Old copy".to_string() } else { format!("Your {name} screen is a customised copy made for an older version. Restore…") };
    row()
        .key(format!("screen-stale-{name}"))
        .gap(5.0)
        .px(6.0)
        .py(2.0)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .fill(c("amber-tint"))
        .border(1.0, c("amber"))
        .role("button")
        .cursor(Cursor::Pointer)
        .help(format!("Your {name} screen is a customised copy made for an older version; new features may be missing. Ask Jarvis to update it, or restore the built-in."))
        .child(rune::icon(crate::icons::get("warning").unwrap(), 11.0).color(c("amber")))
        .w_full()
        .child(text(line).font(body(12.0, 500.0)).color(c("text-bright")).wrap().grow(1.0).min_w(0.0))
        .on_press(move |s: &mut S| press_intent(s, &screen, "screen_restore", &serde_json::json!({ "screen": &*screen })))
}

/// Extras this screen did not draw, drawn by the core under it (an old
/// override cannot hide a feature): `(group, extra)`.
fn fallback_extras<S: Host>(name: &str, missing: &[(String, J)]) -> El {
    let screen: Rc<str> = name.into();
    let mut strip = row().key(format!("screen-extras-{name}")).gap(4.0).flex_wrap().items_center().w_full();
    let mut group = None;
    for (g, e) in missing {
        if group.as_deref() != Some(g.as_str()) && !g.is_empty() {
            strip = strip.child(text(format!("{g}:")).font(body(12.0, 500.0)).color(c("text-muted")));
        }
        group = Some(g.clone());
        let (fg, tint) = tone(e["tone"].as_str().unwrap_or(""));
        let mut label = e["label"].as_str().unwrap_or("").to_string();
        if let Some(n) = e["count"].as_u64() {
            label = if label.is_empty() { n.to_string() } else { format!("{label} {n}") };
        }
        let mut b = row()
            .key(format!("screen-extras-{name}-{g}-{}", e["id"].as_str().unwrap_or("")))
            .gap(4.0)
            .px(5.0)
            .py(1.0)
            .items_center()
            .radius(theme::radius("xs").unwrap())
            .fill(hex(tint))
            .border(1.0, hex(fg));
        if let Some(h) = e["help"].as_str().filter(|h| !h.is_empty()) {
            b = b.help(h.to_string());
        }
        if let Some(i) = e["icon"].as_str().and_then(crate::icons::get) {
            b = b.child(rune::icon(i, 11.0).color(hex(fg)));
        }
        if !label.is_empty() {
            b = b.child(text(label).font(body(12.0, 500.0)).color(hex(fg)));
        }
        if let (Some(n), args) = (e["intent"]["name"].as_str().map(str::to_string), e["intent"]["args"].clone()) {
            let screen = screen.clone();
            b = b.role("button").cursor(Cursor::Pointer).on_press(move |s: &mut S| press_intent(s, &screen, &n, &args));
        }
        strip = strip.child(b);
    }
    strip
}

impl ScreenState {
    /// The extras of this screen's place (`data.app.extras`) its last view
    /// did not draw (no element with the extra's key): `(group, extra)`.
    /// Only the top bar and the sandbox list host extras; a sandbox's are
    /// looked for only when its row is drawn (a filter hides rows).
    pub fn undrawn_extras(&self) -> Vec<(String, J)> {
        let Some(tree) = &self.last_good else { return vec![] };
        let mut keys = std::collections::HashSet::new();
        let mut texts = std::collections::HashSet::new();
        fn walk<'a>(n: &'a Node, keys: &mut std::collections::HashSet<&'a str>, texts: &mut std::collections::HashSet<&'a str>) {
            if let Some(k) = &n.key {
                keys.insert(k.as_str());
            }
            texts.insert(n.text.as_str());
            for c in &n.children {
                walk(c, keys, texts);
            }
        }
        walk(tree, &mut keys, &mut texts);
        // A menu's items count as drawn (`<id>-<item>`): the top bar puts the
        // extras that do not fit into one (topbar.luau, "extra-topbar").
        let mut items = std::collections::HashSet::new();
        fn menus(n: &Node, items: &mut std::collections::HashSet<String>) {
            if let Some(id) = &n.id {
                for it in &n.items {
                    items.insert(format!("{id}-{}", it.id));
                }
            }
            for c in &n.children {
                menus(c, items);
            }
        }
        menus(tree, &mut items);
        keys.extend(items.iter().map(String::as_str));
        let x = &self.data["app"]["extras"];
        let mut out = vec![];
        match self.name.as_str() {
            "topbar" => {
                for e in x["topbar"].as_array().into_iter().flatten() {
                    // Drawn elsewhere by the core's own rule: New in the
                    // sidebar's header, Requests/Network/Setup in the left rail.
                    if e["in_sidebar"] == true || e["in_rail"] == true {
                        continue;
                    }
                    if !keys.contains(format!("extra-topbar-{}", e["id"].as_str().unwrap_or("")).as_str()) {
                        out.push((String::new(), e.clone()));
                    }
                }
            }
            "sandboxes" => {
                for sb in self.data["sandboxes"].as_array().into_iter().flatten() {
                    let n = sb["name"].as_str().unwrap_or("");
                    if !keys.contains(format!("row-{n}").as_str()) && !texts.contains(n) {
                        continue;
                    }
                    // A sessions disclosure (`sessions-<name>`) accounts for the
                    // rows it holds, open or shut.
                    let folded = keys.contains(format!("sessions-{n}").as_str());
                    for (place, prefix) in [("sandboxes", "extra-sandbox"), ("rows", "extra-row")] {
                        if folded && place == "rows" {
                            continue;
                        }
                        for e in x[place][n].as_array().into_iter().flatten() {
                            if !keys.contains(format!("{prefix}-{n}-{}", e["id"].as_str().unwrap_or("")).as_str()) {
                                out.push((n.to_string(), e.clone()));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }
}

struct Build<'a> {
    name: Rc<str>,
    ui: &'a Ui,
    overlays: Vec<El>,
    /// Filled by a `group = "list"` container for its items (the pressable
    /// rows inside it, at any depth, not inside another item): the keys of
    /// the items before and after each (Up and Down move there).
    nav: HashMap<String, (Option<String>, Option<String>)>,
}

/// The items of a list group, in reading order: rows and columns with `on`,
/// at any depth (a group's rows may sit inside blocks), not inside another.
fn list_items(n: &Node, out: &mut Vec<String>) {
    for k in &n.children {
        if matches!(k.kind, Kind::Row | Kind::Column) && k.on.is_some() {
            if let Some(key) = key_of(k) {
                out.push(key);
            }
        } else {
            list_items(k, out);
        }
    }
}

fn arg_key(v: &J) -> String {
    match v {
        J::String(s) => s.clone(),
        J::Number(n) => n.to_string(),
        J::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// The element's Rune key: its `key`, else `<on>-<arg>` for anything that
/// reacts, else none.
fn key_of(n: &Node) -> Option<String> {
    n.key.clone().or_else(|| match n.kind {
        Kind::Input | Kind::Textarea | Kind::Menu | Kind::Select | Kind::Split => n.id.clone(),
        _ => n.on.as_ref().map(|on| {
            let a = arg_key(&n.arg);
            if a.is_empty() {
                on.clone()
            } else {
                format!("{on}-{a}")
            }
        }),
    })
}

fn fire<S: Host>(s: &mut S, screen: &str, action: &str, arg: &J, value: &J) {
    if let Some(intents) = s.screen(screen).act(action, arg, value) {
        if !intents.is_empty() {
            s.intents(screen, intents);
        }
    }
}

fn apply(mut e: El, st: &Style, help: &Option<String>) -> El {
    match st.w {
        Some(Size::Pt(w)) => e = e.w(w),
        Some(Size::Full) => e = e.w_full(),
        None => {}
    }
    match st.h {
        Some(Size::Pt(h)) => e = e.h(h),
        Some(Size::Full) => e = e.h_full(),
        None => {}
    }
    if let Some(v) = st.min_w {
        e = e.min_w(v);
    }
    if let Some(v) = st.max_w {
        e = e.max_w(v);
    }
    if let Some(v) = st.min_h {
        e = e.min_h(v);
    }
    if let Some(v) = st.max_h {
        e = e.max_h(v);
    }
    if let Some(g) = st.grow {
        e = e.grow(g).min_w(st.min_w.unwrap_or(0.0));
    }
    if let Some(p) = st.pad {
        e = e.padding(p);
    }
    if let Some(p) = st.px {
        e = e.px(p);
    }
    if let Some(p) = st.py {
        e = e.py(p);
    }
    if let Some(f) = st.fill {
        e = e.fill(hex(f));
    }
    if let Some(b) = st.border {
        e = e.border(1.0, hex(b));
    }
    if let Some(r) = st.radius {
        e = e.radius(r);
    }
    if let Some(o) = st.opacity {
        e = e.opacity(o);
    }
    if let Some([l, t, r, b]) = st.pin {
        e = e.pin(l, t, r, b);
    }
    if let Some(h) = help {
        e = e.help(h.clone());
    }
    e
}

fn boxed(e: El, st: &Style) -> El {
    let mut e = e;
    if let Some(g) = st.gap {
        e = e.gap(g);
    }
    e = match st.align {
        Some(Align::Start) => e.items_start(),
        Some(Align::Center) => e.items_center(),
        Some(Align::End) => e.items_end(),
        Some(Align::Stretch) => e.items_stretch(),
        None => e,
    };
    match st.justify {
        Some(Justify::Start) => e.justify_start(),
        Some(Justify::Center) => e.justify_center(),
        Some(Justify::End) => e.justify_end(),
        Some(Justify::Between) => e.justify_between(),
        None => e,
    }
}

impl Build<'_> {
    fn node<S: Host>(&mut self, n: &Node) -> El {
        let st = &n.style;
        let key = key_of(n);
        let name = self.name.clone();
        let e = match n.kind {
            Kind::Text => {
                let mut t = text(n.text.clone()).font(text_style(st, 13.0, 400.0)).color(hex(st.color.unwrap_or(theme::color_value("text").unwrap())));
                if st.wrap {
                    t = t.wrap();
                }
                // A text that truncates also gives way when its row is short
                // (it shrinks before its siblings are pushed out).
                if st.truncate {
                    t = t.truncate().shrink();
                }
                t
            }
            Kind::Row | Kind::Column => {
                let nav = if n.on.is_some() { key.as_ref().and_then(|k| self.nav.get(k).cloned()) } else { None };
                let folds = n.folds && n.kind == Kind::Column && !reduced_motion();
                let mut b = boxed(if n.kind == Kind::Row { row().items_center() } else { col() }, st);
                // A list group: each pressable item learns its neighbours' keys.
                if n.group.is_some() {
                    let mut items = vec![];
                    list_items(n, &mut items);
                    for (i, k) in items.iter().enumerate() {
                        let prev = i.checked_sub(1).map(|j| items[j].clone());
                        self.nav.insert(k.clone(), (prev, items.get(i + 1).cloned()));
                    }
                }
                for k in n.children.iter() {
                    let child = self.node::<S>(k);
                    // A folding column: each child glides to its new height.
                    let child = if folds { child.fold_glide(FOLD) } else { child };
                    b = b.child(child);
                }
                if folds {
                    b = b.growth(rune::GrowthSpec { spring: FOLD, ..Default::default() });
                }
                if let Some(on) = &n.on {
                    let (on, arg) = (on.clone(), n.arg.clone());
                    b = b.role("button").cursor(Cursor::Pointer).hover(hex(st.hover.unwrap_or(theme::color_value("surface").unwrap()))).on_press(move |s: &mut S| fire(s, &name, &on, &arg, &J::Null));
                    if let Some(h) = &n.help {
                        b = b.label(h.clone());
                    }
                    if n.selected {
                        // Screen readers hear it as the current item.
                        b = b.focus_selected(true).label(format!("{} (selected)", n.help.clone().unwrap_or_default()));
                    }
                    if n.on_space.is_some() || nav.is_some() {
                        let (name, arg, sp) = (self.name.clone(), n.arg.clone(), n.on_space.clone());
                        let (prev, next) = nav.unwrap_or((None, None));
                        let me = key.clone();
                        b = b.on_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx| {
                            let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo || ev.mods.shift);
                            if !plain {
                                return false;
                            }
                            // Only when the row itself has the keyboard, not a button inside it.
                            if tx.focused() != me.as_deref() {
                                return false;
                            }
                            match &ev.key {
                                rune::Key::Text(t) if t == " " => {
                                    if let Some(sp) = &sp {
                                        fire(s, &name, sp, &arg, &J::Null);
                                        return true;
                                    }
                                    false
                                }
                                rune::Key::Up | rune::Key::Down => {
                                    let to = if ev.key == rune::Key::Up { &prev } else { &next };
                                    if let Some(k) = to {
                                        tx.focus(k.clone(), true);
                                    }
                                    true
                                }
                                _ => false,
                            }
                        });
                    }
                } else if let Some(h) = st.hover {
                    b = b.hover(hex(h));
                }
                if n.hover_group {
                    b = b.hover_group();
                }
                if let Some(z) = n.hover_zone.clone() {
                    // The pointer over this element and moving in it, for the zone (`Zones`).
                    let el_key = key.clone().unwrap_or_else(|| format!("{z}#{}", n.children.len()));
                    let (nm, nm2, z2) = (self.name.clone(), self.name.clone(), z.clone());
                    b = b
                        .on_hover_change(move |s: &mut S, inside: bool, tx: &mut rune::Tx| {
                            s.screen(&nm).ui.zones.hover(&z, &el_key, inside, Instant::now());
                            tx.wake_after(ZONE_CLOSE + Duration::from_millis(10));
                        })
                        .on_pointer_move_local(move |s: &mut S, _p: rune::PointerEvent, tx: &mut rune::Tx| {
                            s.screen(&nm2).ui.zones.moved(&z2, Instant::now());
                            tx.wake_after(ZONE_OPEN + Duration::from_millis(10));
                        });
                }
                if n.clip {
                    b = b.clip_rounded(st.radius.unwrap_or(0.0));
                }
                if n.group.is_some() {
                    // Up and Down are the items' own (above), so Rune's roving stays out.
                    b = b.role("list");
                }
                b
            }
            Kind::List => {
                let mut l = col().w_full().border(1.0, c("border")).radius(theme::radius("sm").unwrap());
                for (i, k) in n.children.iter().enumerate() {
                    if i > 0 {
                        l = l.child(el().w_full().h(1.0).fill(c("elevated")));
                    }
                    l = l.child(el().w_full().px(12.0).py(st.gap.unwrap_or(8.0)).child(self.node::<S>(k)));
                }
                l
            }
            Kind::Scroll => {
                let mut content = col().w_full().gap(st.gap.unwrap_or(0.0));
                for k in &n.children {
                    content = content.child(self.node::<S>(k));
                }
                let base = col().scroll(ScrollSpec { stick_bottom: n.stick_bottom }).w_full().min_h(0.0);
                let base = if st.h.is_none() && st.grow.is_none() { base.grow(1.0) } else { base };
                base.child(content)
            }
            Kind::Button => self.button::<S>(n, key.clone()),
            Kind::Input => {
                let id = n.id.clone().unwrap_or_default();
                let edit = self.ui.edits.get(&id).map(|e| e.0.clone()).unwrap_or_else(|| TextEdit::new(""));
                let (id2, id3) = (id.clone(), id.clone());
                let mut t = text_input(&edit, body(st.size.unwrap_or(13.0), 400.0))
                    .single_line()
                    .key(key.clone().unwrap_or_default())
                    .color(c("text-bright"))
                    .placeholder(n.placeholder.clone().unwrap_or_default(), c("text-dim"))
                    .w_full()
                    .on_edit(move |s: &mut S| &mut s.screen(&name).ui.edits.get_mut(&id2).expect("input synced").0);
                if n.secure {
                    t = t.secure();
                }
                if let Some(on) = n.on.clone() {
                    let (name, arg) = (self.name.clone(), n.arg.clone());
                    t = t.on_input_change(move |s: &mut S, _| {
                        let v = s.screen(&name).ui.edits.get(&id).map(|e| e.0.text.clone()).unwrap_or_default();
                        s.screen(&name).ui.editing = true;
                        fire(s, &name, &on, &arg, &J::String(v));
                        s.screen(&name).ui.editing = false;
                    });
                }
                if let Some(on) = n.on_submit.clone() {
                    let (name, arg) = (self.name.clone(), n.arg.clone());
                    t = t.on_submit(move |s: &mut S, _| {
                        let v = s.screen(&name).ui.edits.get(&id3).map(|e| e.0.text.clone()).unwrap_or_default();
                        fire(s, &name, &on, &arg, &J::String(v));
                    });
                }
                if n.disabled {
                    t = t.read_only(true);
                }
                el().w_full().h(theme::layout::CONTROL_H).px(8.0).items_center().fill(c("bg")).border(1.0, c("control-border")).radius(theme::radius("sm").unwrap()).focus_within_ring(2.0, 1.0, c("focus")).child(t)
            }
            Kind::Icon => {
                let i = crate::icons::get(&n.text).expect("checked icon");
                rune::icon(i, st.size.unwrap_or(14.0)).color(hex(st.color.unwrap_or(theme::color_value("text").unwrap())))
            }
            Kind::Badge if n.pill => {
                // A count alone in a round bubble: the tone's colour filled,
                // the number in the page's dark (a mark and its label at once).
                let (fg, _) = tone(n.variant.as_deref().unwrap_or("blue"));
                // 16 pt high, 11 pt figures (tabular in the monospace face), a
                // pill for two digits rather than a stretched circle.
                el().h(16.0).min_w(16.0).px(5.0).items_center().justify_center().radius(8.0).fill(hex(fg)).child(text(n.text.clone()).font(TextStyle::new(11.0, 600.0).lh(16.0)).color(c("bg")))
            }
            Kind::Badge => {
                let (fg, tint) = tone(n.variant.as_deref().unwrap_or("neutral"));
                // A quiet label: the tone's tint, its colour as text, no frame.
                el().px(6.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(hex(tint)).child(text(n.text.clone()).font(body(12.0, 500.0)).color(hex(fg)))
            }
            Kind::Dot => {
                let (fg, _) = tone(n.variant.as_deref().unwrap_or("neutral"));
                el().w(8.0).h(8.0).radius(4.0).fill(hex(fg))
            }
            Kind::Split => self.split::<S>(n),
            Kind::Spacer => {
                if st.w.is_none() && st.h.is_none() && st.grow.is_none() {
                    el().grow(1.0)
                } else {
                    el()
                }
            }
            Kind::Divider => {
                let col_ = hex(st.color.unwrap_or(theme::color_value("elevated").unwrap()));
                if n.vertical {
                    el().w(1.0).h_full().fill(col_)
                } else {
                    el().h(1.0).w_full().fill(col_)
                }
            }
            Kind::Menu | Kind::Select => self.menu::<S>(n, key.clone()),
            Kind::Checkbox => {
                let on = n.value.as_bool().unwrap_or(false);
                let mut b = el().w(16.0).h(16.0).radius(4.0).border(1.0, c(if on { "blue" } else { "control-border" })).items_center().justify_center();
                if on {
                    b = b.fill(c("blue")).child(rune::icon(crate::icons::get("check").unwrap(), 12.0).color(c("bg")));
                }
                let mut r = row().gap(8.0).min_h(theme::layout::DENSE_H).items_center().child(b).child(text(n.text.clone()).font(body(13.0, 400.0)).color(c("text"))).role("checkbox").label(n.text.clone()).checked(on);
                if !n.disabled {
                    if let Some(act) = n.on.clone() {
                        let arg = n.arg.clone();
                        r = r.cursor(Cursor::Pointer).on_press(move |s: &mut S| fire(s, &name, &act, &arg, &J::Bool(!on)));
                    }
                } else {
                    r = r.opacity(theme::DISABLED_OPACITY);
                }
                r
            }
            Kind::Markdown => {
                let look = crate::markdown::Look { color: st.color.unwrap_or(theme::color_value("text").unwrap()), size: st.size.unwrap_or(13.0) };
                let on_link: Option<Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>> = n.on.clone().map(|on| {
                    let (name, arg) = (name.clone(), n.arg.clone());
                    Rc::new(move |s: &mut S, url: &str, _tx: &mut rune::Tx| fire(s, &name, &on, &arg, &J::String(url.to_string()))) as Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>
                });
                let k = key.clone().unwrap_or_else(|| "md".into());
                crate::markdown::view::<S>(&n.text, &look, &k, on_link)
            }
            Kind::Textarea => {
                let id = n.id.clone().unwrap_or_default();
                let edit = self.ui.edits.get(&id).map(|e| e.0.clone()).unwrap_or_else(|| TextEdit::new(""));
                let (id2, id3) = (id.clone(), id.clone());
                let size = st.size.unwrap_or(13.0);
                let mut t = text_input(&edit, body(size, 400.0))
                    .key(key.clone().unwrap_or_default())
                    .color(c("text-bright"))
                    .placeholder(n.placeholder.clone().unwrap_or_default(), c("text-dim"))
                    .w_full()
                    .on_edit(move |s: &mut S| &mut s.screen(&name).ui.edits.get_mut(&id2).expect("textarea synced").0);
                if let Some(on) = n.on.clone() {
                    let (name, arg, id) = (self.name.clone(), n.arg.clone(), id.clone());
                    t = t.on_input_change(move |s: &mut S, _| {
                        let v = s.screen(&name).ui.edits.get(&id).map(|e| e.0.text.clone()).unwrap_or_default();
                        s.screen(&name).ui.editing = true;
                        fire(s, &name, &on, &arg, &J::String(v));
                        s.screen(&name).ui.editing = false;
                    });
                }
                // Rune submits on a plain Return in a paragraph: here that is a new
                // line, and only Cmd (Ctrl elsewhere) + Return submits.
                let (name2, arg, on_submit) = (self.name.clone(), n.arg.clone(), n.on_submit.clone());
                t = t.on_submit(move |s: &mut S, tx: &mut rune::Tx| {
                    if tx.mods().cmd {
                        if let Some(on) = &on_submit {
                            let v = s.screen(&name2).ui.edits.get(&id3).map(|e| e.0.text.clone()).unwrap_or_default();
                            fire(s, &name2, on, &arg, &J::String(v));
                        }
                    } else if let Some(e) = s.screen(&name2).ui.edits.get_mut(&id3) {
                        let _ = e.0.paste("\n");
                    }
                });
                if n.disabled {
                    t = t.read_only(true);
                }
                let h = n.rows as f32 * (size * theme::LINE_HEIGHT).round() + 16.0;
                col().w_full().h(h).px(8.0).py(8.0).fill(c("bg")).border(1.0, c("control-border")).radius(theme::radius("sm").unwrap()).focus_within_ring(2.0, 1.0, c("focus")).child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(t))
            }
            Kind::Image => match crate::picture::shown(key.as_deref().unwrap_or("")) {
                Some((px, (w, h))) => {
                    let (mw, mh) = (st.max_w.unwrap_or(w as f32), st.max_h.unwrap_or(h as f32));
                    let scale = (mw / w as f32).min(mh / h as f32).min(1.0);
                    let e = el().w((w as f32 * scale).max(1.0)).h((h as f32 * scale).max(1.0)).radius(theme::radius("xs").unwrap());
                    e.pixels_fit(&px, if n.fill_image { rune::SurfaceFit::Fill } else { rune::SurfaceFit::Fit })
                }
                None => el().px(8.0).py(4.0).radius(theme::radius("xs").unwrap()).fill(c("elevated")).child(text("picture not loaded").font(body(12.0, 400.0)).color(c("text-muted"))),
            },
            Kind::Surface if crate::feat::surface_owner(n.id.as_deref().unwrap_or("")).is_none() => {
                el().grow(1.0).w_full().padding(8.0).fill(c("elevated")).child(text(format!("no feature owns a surface '{}'", n.id.as_deref().unwrap_or(""))).font(body(12.0, 400.0)).color(c("text-muted")))
            }
            Kind::Surface => crate::surface::view::<S>(n.id.as_deref().unwrap_or(""), |s: &mut S, ev, tx| s.surface_event(ev, tx), |s: &mut S, ev, down, tx| s.surface_key(ev, down, tx)),
            Kind::Progress => {
                let v = n.value.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                let (fg, _) = tone(n.variant.as_deref().unwrap_or("blue"));
                el().w_full().h(4.0).radius(2.0).fill(c("elevated")).child(el().w_frac(v).h_full().radius(2.0).fill(hex(fg)))
            }
        };
        // Shown only while the nearest hover group is hovered or focused.
        let e = if n.reveal { e.group_opacity(0.0, 1.0) } else { e };
        let e = match (&key, n.kind) {
            // Inputs and menus key their inner part.
            (Some(k), k2) if !matches!(k2, Kind::Input | Kind::Textarea | Kind::Menu | Kind::Select | Kind::Button | Kind::Markdown) => e.key(k.clone()),
            _ => e,
        };
        // Buttons and menus set their own tooltip, with the shortcut's keycaps.
        let help = if matches!(n.kind, Kind::Button | Kind::Menu | Kind::Select) { &None } else { &n.help };
        let mut e = apply(e, st, help);
        for (what, keys) in &n.shortcuts {
            e = e.help_row(what.clone()).help_keys(keys.clone());
        }
        e
    }

    fn button<S: Host>(&mut self, n: &Node, key: Option<String>) -> El {
        if n.compact {
            return self.compact_button::<S>(n, key);
        }
        let variant = n.variant.as_deref().unwrap_or("neutral");
        let (fill, border, fg, hover) = match variant {
            "primary" => ("green-deep", "green-deep", "white", "primary-hover"),
            "danger" => ("elevated", "border", "red", "red-tint"),
            "ghost" => ("", "", "text-muted", "elevated"),
            _ => ("elevated", "border", "text", "border"),
        };
        // A tone (colour by meaning) colours the icon and the label, and
        // tints the button under the pointer and the keyboard focus.
        let toned = n.tone.as_deref().filter(|t| *t != "neutral" && variant != "primary").map(action_tone);
        let fg = n.style.color.map(hex).or(toned.map(|t| hex(t.0))).unwrap_or_else(|| c(fg));
        // At least 28 pt square (24 dense), the icon centred; a label pads 10 (8 dense).
        let (h, size, icon) = if n.dense { (theme::layout::DENSE_H, 12.0, 14.0) } else { (theme::layout::CONTROL_H, 13.0, 16.0) };
        let only_icon = n.text.is_empty();
        let name = if only_icon { n.help.clone().unwrap_or_else(|| n.icon.clone().unwrap_or_default()) } else { n.text.clone() };
        let mut b = row().gap(6.0).items_center().justify_center().h(h).min_w(h).px(if only_icon { 0.0 } else if n.dense { 8.0 } else { 10.0 }).radius(theme::radius("xs").unwrap()).role("button").label(name);
        if !fill.is_empty() {
            b = b.fill(c(fill)).border(1.0, c(border));
        }
        if let Some(k) = key {
            b = b.key(k);
        }
        if let Some(i) = n.icon.as_deref().and_then(crate::icons::get) {
            b = b.child(rune::icon(i, icon).color(fg));
        }
        if !only_icon {
            b = b.child(text(n.text.clone()).font(text_style(&n.style, size, 500.0)).color(fg).truncate().min_w(0.0));
        }
        if let Some(h) = &n.help {
            // The tooltip names the action and its shortcut (apply() sets the help itself).
            b = b.help(h.clone()).help_keys(n.keys.clone());
        }
        if n.disabled {
            return b.opacity(theme::DISABLED_OPACITY).disabled(true);
        }
        if n.skip_tab {
            b = b.tab_index(-1);
        }
        if let Some((_, tint)) = toned {
            b = b.focus_highlight(hex(tint));
        }
        let (name, on, arg) = (self.name.clone(), n.on.clone().unwrap_or_default(), n.arg.clone());
        let hover = n.style.hover.map(hex).or(toned.map(|t| hex(t.1))).unwrap_or_else(|| c(hover));
        b.hover(hover).cursor(Cursor::Pointer).on_press(move |s: &mut S| fire(s, &name, &on, &arg, &J::Null))
    }

    /// A button one text line tall: a 20 pt framed square (or a label
    /// padded 6), inside a 24 pt target with no fill of its own. Its tone
    /// colours the icon and tints the target on hover.
    fn compact_button<S: Host>(&mut self, n: &Node, key: Option<String>) -> El {
        let tone = n.tone.as_deref().unwrap_or("neutral");
        let (fg, tint) = action_tone(tone);
        let only_icon = n.text.is_empty();
        let name = if only_icon { n.help.clone().unwrap_or_else(|| n.icon.clone().unwrap_or_default()) } else { n.text.clone() };
        let mut face = row().gap(4.0).items_center().justify_center().h(20.0).min_w(20.0).px(if only_icon { 0.0 } else { 6.0 }).radius(theme::radius("xs").unwrap()).fill(c("elevated")).border(1.0, c("border"));
        if let Some(i) = n.icon.as_deref().and_then(crate::icons::get) {
            face = face.child(rune::icon(i, 12.0).color(hex(fg)));
        }
        if !only_icon {
            face = face.child(text(n.text.clone()).font(body(12.0, 500.0)).color(hex(fg)));
        }
        let mut b = row().items_center().justify_center().h(theme::layout::DENSE_H).min_w(theme::layout::DENSE_H).radius(theme::radius("xs").unwrap()).role("button").label(name).child(face);
        if let Some(k) = key {
            b = b.key(k);
        }
        if let Some(h) = &n.help {
            b = b.help(h.clone()).help_keys(n.keys.clone());
        }
        if n.disabled {
            return b.opacity(theme::DISABLED_OPACITY).disabled(true);
        }
        if n.skip_tab {
            b = b.tab_index(-1);
        }
        let (name, on, arg) = (self.name.clone(), n.on.clone().unwrap_or_default(), n.arg.clone());
        b.hover(hex(tint)).cursor(Cursor::Pointer).on_press(move |s: &mut S| fire(s, &name, &on, &arg, &J::Null))
    }

    fn menu<S: Host>(&mut self, n: &Node, key: Option<String>) -> El {
        let id = n.id.clone().unwrap_or_default();
        let key = key.unwrap_or_else(|| id.clone());
        let open = self.ui.open.as_deref() == Some(id.as_str());
        let label = match n.kind {
            Kind::Select => n.items.iter().find(|i| Some(i.id.as_str()) == n.value.as_str()).map(|i| i.label.clone()).or_else(|| n.placeholder.clone()).unwrap_or_default(),
            _ => n.text.clone(),
        };
        let (h, size) = if n.dense { (theme::layout::DENSE_H, 12.0) } else { (theme::layout::CONTROL_H, 13.0) };
        let name = if label.is_empty() { n.help.clone().unwrap_or_else(|| id.clone()) } else { label.clone() };
        let mut t = row().key(key.clone()).gap(6.0).items_center().justify_center().px(8.0).h(h).min_w(h).radius(theme::radius("sm").unwrap()).role(if n.kind == Kind::Select { "combobox" } else { "button" }).label(name).expanded(open);
        t = match (n.kind, n.variant.as_deref()) {
            (Kind::Menu, Some("ghost")) => t.hover(c("elevated")),
            _ => t.fill(c("bg")).border(1.0, c(if open { "blue" } else { "control-border" })).hover(c("elevated")),
        };
        if let Some(h) = &n.help {
            t = t.help(h.clone()).help_keys(n.keys.clone());
        }
        if let Some(i) = n.icon.as_deref().and_then(crate::icons::get) {
            t = t.child(rune::icon(i, if n.dense { 14.0 } else { 16.0 }).color(c("text")));
        }
        if !label.is_empty() {
            t = t.child(text(label.clone()).font(body(size, 400.0)).color(c("text-bright")).truncate().min_w(0.0));
        }
        // A ghost menu that shows only its icon (an "actions" menu) needs no chevron.
        if !(n.variant.as_deref() == Some("ghost") && n.kind == Kind::Menu && label.is_empty()) {
            t = t.child(rune::icon(crate::icons::get("chevron-down").unwrap(), 12.0).color(c("text-muted")));
        }
        if n.disabled {
            return t.opacity(theme::DISABLED_OPACITY).disabled(true);
        }
        if n.skip_tab {
            t = t.tab_index(-1);
        }
        let (name, id2) = (self.name.clone(), id.clone());
        t = t.cursor(Cursor::Pointer).on_press(move |s: &mut S| {
            let ui = &mut s.screen(&name).ui;
            ui.open = if ui.open.as_deref() == Some(id2.as_str()) { None } else { Some(id2.clone()) };
        });
        if open {
            let rows: Vec<MenuRow> = n.items.iter().filter(|i| !i.divider).map(|it| self.menu_row_of(n, &id, it)).collect();
            let mut list = menu_surface(format!("{id}-list"), key.clone(), rune::Align::Start, menu_width(&rows));
            for it in &n.items {
                list = list.child(if it.divider { menu_divider() } else { self.item::<S>(n, &id, it) });
            }
            // A press outside closes the menu, and only that (light dismiss).
            let (name, id3) = (self.name.clone(), id.clone());
            self.overlays.push(menu_scrim(format!("{id}-scrim"), move |s: &mut S| {
                let ui = &mut s.screen(&name).ui;
                if ui.open.as_deref() == Some(id3.as_str()) {
                    ui.open = None;
                }
            }));
            self.overlays.push(list);
        }
        t
    }

    fn menu_row_of<'a>(&self, n: &Node, id: &str, it: &'a Item) -> MenuRow<'a> {
        MenuRow {
            key: format!("{id}-{}", it.id),
            label: &it.label,
            icon: it.icon.as_deref(),
            tone: it.tone.as_deref(),
            help: it.help.as_deref(),
            keys: &it.keys,
            count: None,
            chosen: n.kind == Kind::Select && n.value.as_str() == Some(it.id.as_str()),
            disabled: it.disabled,
        }
    }

    fn item<S: Host>(&self, n: &Node, id: &str, it: &Item) -> El {
        let r = menu_row(&self.menu_row_of(n, id, it));
        if it.disabled {
            return r;
        }
        let (name, on, arg, value) = (self.name.clone(), n.on.clone().unwrap_or_default(), n.arg.clone(), J::String(it.id.clone()));
        r.on_press(move |s: &mut S| {
            s.screen(&name).ui.open = None;
            fire(s, &name, &on, &arg, &value);
        })
    }

    fn split<S: Host>(&mut self, n: &Node) -> El {
        let id = n.id.clone().unwrap_or_default();
        let (ratio, _) = self.ui.splits.get(&id).copied().unwrap_or((0.5, 0.0));
        let a = self.node::<S>(&n.children[0]);
        let b = self.node::<S>(&n.children[1]);
        let screen = self.name.clone();
        split_view::<S, _>(n.vertical, ratio, a, b, &id, move |s: &mut S, id: &str| s.screen(&screen).ui.splits.entry(id.to_string()).or_insert((0.5, 0.0)))
    }
}

/// Two children side by side (`vertical`) or stacked, the first taking
/// `ratio` of the length, with a 6 pt handle between them that drags the ratio.
/// `get` finds the `(ratio, length)` pair in the app's state. Used by the
/// `split` element and by the core's pane area.
pub fn split_view<S: 'static, F>(vertical: bool, ratio: f32, a: El, b: El, id: &str, get: F) -> El
where
    F: for<'a> Fn(&'a mut S, &str) -> &'a mut (f32, f32) + 'static,
{
    let get = Rc::new(get);
    let (g1, g2) = (get.clone(), get);
    let (id1, id2): (Rc<str>, Rc<str>) = (id.into(), id.into());
    let handle_key = format!("{id}-handle");
    let handle = if vertical { el().w(theme::layout::GUTTER).h_full().cursor(Cursor::ResizeColumn) } else { el().h(theme::layout::GUTTER).w_full().cursor(Cursor::ResizeRow) };
    let handle = handle.key(handle_key).hover(c("border")).on_drag(move |s: &mut S, d: &mut rune::Drag, _tx| {
        let r = g1(s, &id1);
        if matches!(d.phase, rune::Phase::Down) {
            d.data = r.0;
        }
        let len = r.1.max(1.0);
        let moved = if vertical { d.dx } else { d.dy };
        r.0 = (d.data + moved / len).clamp(0.1, 0.9);
    });
    // Flex basis 0, so the two take `ratio` and `1 - ratio` of what the handle leaves.
    let (first, second) = (a.grow(ratio).min_w(0.0).min_h(0.0), b.grow(1.0 - ratio).min_w(0.0).min_h(0.0));
    let base = if vertical { row() } else { col() };
    base.w_full().h_full().min_w(0.0).min_h(0.0).items_stretch().child(first).child(handle).child(second).on_layout(move |s: &mut S, r: [f32; 4]| {
        let e = g2(s, &id2);
        e.1 = if vertical { r[2] } else { r[3] };
    })
}

fn text_style(st: &Style, size: f32, weight: f32) -> TextStyle {
    // Nothing a person reads is under 12 pt, whatever a screen asks.
    let t = body(st.size.unwrap_or(size).max(12.0), st.weight.unwrap_or(weight));
    if st.mono {
        t.mono()
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SCREEN: &str = r#"
return {
  init = function() return { n = 0 } end,
  render = function(state, data) return text{ "TITLE " .. (state.n or 0) } end,
  actions = { bump = function(state) state.n = (state.n or 0) + 1 end,
              go = function(state, arg) return intent("stop_sandbox", { sandbox = arg }) end,
              bad = function() return intent("rm_everything", {}) end },
}"#;

    fn title(s: &ScreenState) -> String {
        s.last_good.as_ref().map(|n| n.text.clone()).unwrap_or_default()
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sbxw-screen-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn wait_wake(wakes: &AtomicUsize, n: usize) {
        for _ in 0..100 {
            if wakes.load(Ordering::SeqCst) >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no wake");
    }

    /// The watcher sees a real override file appear, change, break and go
    /// away: it wins over the built-in, state is kept, a broken edit keeps
    /// the last good view, and deleting it restores the built-in.
    #[test]
    fn override_file_wins_reloads_and_deleting_it_restores_the_builtin() {
        let dir = tmp("ovr");
        let path = dir.join("sandboxes.luau");
        let mut s = ScreenState::new("sandboxes", &dir, crate::fixture::screen_data(&J::Null, &crate::fixture::AppView::default()));
        assert_eq!(s.source, Source::Builtin);
        assert!(s.error.is_none(), "{:?}", s.error);
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        s.watch(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));

        std::fs::write(&path, SCREEN).unwrap();
        wait_wake(&wakes, 1);
        assert!(s.poll());
        assert_eq!(s.source, Source::File(path.clone()));
        s.act("bump", &J::Null, &J::Null);
        assert_eq!(title(&s), "TITLE 1");

        // Some file systems keep whole-second mtimes: wait past the second.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&path, SCREEN.replace("TITLE", "EDITED")).unwrap();
        wait_wake(&wakes, 2);
        assert!(s.poll());
        assert_eq!(title(&s), "EDITED 1", "state kept across the reload");

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&path, "return {{{").unwrap();
        wait_wake(&wakes, 3);
        assert!(s.poll());
        assert_eq!(title(&s), "EDITED 1", "last good view kept");
        assert!(s.error.as_deref().unwrap_or("").contains("sandboxes.luau"), "error shown: {:?}", s.error);

        std::fs::remove_file(&path).unwrap();
        wait_wake(&wakes, 4);
        assert!(s.poll());
        assert_eq!(s.source, Source::Builtin);
        assert!(s.error.is_none(), "{:?}", s.error);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_intents_are_shown_and_not_returned() {
        let dir = tmp("int");
        std::fs::write(dir.join("x.luau"), SCREEN).unwrap();
        let mut s = ScreenState::new("x", &dir, J::Null);
        assert_eq!(s.act("go", &J::from("web"), &J::Null), Some(vec![Intent::StopSandbox { sandbox: "web".into() }]));
        assert_eq!(s.act("bad", &J::Null, &J::Null), None);
        assert!(s.error.as_deref().unwrap().contains("unknown intent 'rm_everything'"), "{:?}", s.error);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `textarea` holds its text like `input`; `image` names a picture the core holds.
    #[test]
    fn textarea_and_image_elements() {
        let dir = std::env::temp_dir().join(format!("sbxw-ta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.luau"), r#"return { render = function(state, data)
  return column{ textarea{ id = "answer", value = "a\nb", rows = 3, on_submit = "send", placeholder = "Your answer" },
                 image{ key = "shot-1", max_w = 100, max_h = 100 } }
end, actions = { send = function(_, _, v) end } }"#).unwrap();
        let mut s = ScreenState::new("x", &dir, J::Null);
        assert!(s.error.is_none(), "{:?}", s.error);
        assert_eq!(s.ui.edits["answer"].0.text, "a\nb");
        // An image needs its key; an unknown fit is refused.
        std::fs::write(dir.join("x.luau"), r#"return { render = function() return image{ max_w = 10 } end }"#).unwrap();
        s.load_file(dir.join("x.luau"));
        assert!(s.error.as_deref().unwrap_or("").contains("needs a 'key'"), "{:?}", s.error);
        let mut png = vec![];
        image::DynamicImage::new_rgba8(40, 20).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = crate::picture::prepare(&png, crate::picture::Limits::default(), None, crate::picture::Encode::Pixels).unwrap();
        crate::picture::show("shot-1", &p);
        assert_eq!(crate::picture::shown("shot-1").map(|x| x.1), Some((40, 20)));
        crate::picture::forget("shot-1");
        assert!(crate::picture::shown("shot-1").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Session sub-rows (`Place::SandboxRow`) are drawn under their sandbox.
    #[test]
    fn the_sandbox_list_draws_session_rows() {
        let dir = std::env::temp_dir().join("sbxw-no-overrides");
        let fx = crate::fixture::Fixture::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sandboxes/basic.json")).unwrap();
        let mut data = fx.screen_data(&crate::fixture::AppView::default());
        let name = data["sandboxes"][0]["name"].as_str().unwrap().to_string();
        data["app"]["extras"] = serde_json::json!({ "rows": { name.clone(): [
            { "id": "main", "label": "main agent", "tone": "green", "help": "working", "count": null, "icon": null, "intent": { "name": "open_terminal", "args": { "sandbox": name } } },
            { "id": "t1", "label": "t1: fix the login", "tone": "amber", "help": "", "count": 2, "icon": null, "intent": null }
        ] } });
        let s = ScreenState::new("sandboxes", &dir, data);
        assert!(s.error.is_none(), "{:?}", s.error);
        let tree = format!("{:?}", s.last_good.unwrap());
        for k in [format!("extra-row-{name}-main"), format!("extra-row-{name}-t1"), "t1: fix the login".into()] {
            assert!(tree.contains(&k), "{k} not drawn");
        }
    }

    /// The status rail: each row says its state in a word and a rail shape;
    /// the name has its own line; start or stop is always there; every
    /// other action is in the row's menu, keyed as the inline extras were.
    #[test]
    fn sandbox_rows_carry_their_state_and_keep_every_action_in_their_menu() {
        let dir = std::env::temp_dir().join("sbxw-no-overrides");
        let fx = crate::fixture::Fixture::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/shell/design-1.json")).unwrap();
        let mut data = fx.screen_data(&crate::fixture::AppView::default());
        let more = |sb: &str| serde_json::json!({ "id": "more", "label": "", "icon": "more", "tone": "muted", "help": "Remove, duplicate, recreate…", "count": null, "intent": { "name": "open_modal", "args": { "screen": "sandbox-actions", "arg": sb } } });
        let net = |sb: &str| serde_json::json!({ "id": "network", "label": "", "icon": "ports", "tone": "muted", "help": "Network: policy, blocked hosts, ports", "count": null, "intent": { "name": "network_show", "args": { "sandbox": sb } } });
        let asks = serde_json::json!({ "id": "relay", "label": "?", "icon": null, "tone": "amber", "help": "waits for your answer", "count": 1, "intent": { "name": "relay_open", "args": {} } });
        data["app"]["extras"] = serde_json::json!({ "sandboxes": {
            "test-sbx": [net("test-sbx"), more("test-sbx")],
            "sbxw-desktop-design": [asks, more("sbxw-desktop-design")],
            "chat-release-notes": [more("chat-release-notes")],
        } });
        let s = ScreenState::new("sandboxes", &dir, data);
        assert!(s.error.is_none(), "{:?}", s.error);
        let n = s.last_good.clone().unwrap();
        let tree = format!("{n:?}");
        // State words: running, working, wants you, stopped, starting…, unreachable.
        for w in ["\"running\"", "\"working\"", "\"wants you\"", "\"stopped\"", "\"starting…\"", "\"unreachable\""] {
            assert!(tree.contains(w), "no row says {w}");
        }
        // The amber request says itself in the state word's place, pressable.
        assert!(tree.contains("asks you 1") && node_has(&n, "extra-sandbox-sbxw-desktop-design-relay"), "the amber count shows as a text badge");
        // The model in place of the agent's name, short and lower case.
        assert!(tree.contains("\"opus 5.5\"") && tree.contains("\"sonnet 5.5\"") && tree.contains("\"haiku 5.5\""), "models shown");
        for k in ["add-test-sbx", "add-chat-release-notes"] {
            assert!(node_has(&n, k), "{k} not drawn");
        }
        for k in ["row-test-sbx", "stop-test-sbx", "start-chat-release-notes", "extra-sandbox-test-sbx"] {
            assert!(node_has(&n, k), "{k} not drawn");
        }
        // The menu holds the extras under their old keys (extra-sandbox-<name>-<id>).
        let menu = find(&n, "extra-sandbox-test-sbx").unwrap();
        let ids: Vec<&str> = menu.items.iter().filter(|i| !i.divider).map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["bash", "spare", "network", "more", "stop"]);
        // In groups, a line between: open, go to, manage, then stop (red).
        assert_eq!(menu.items.iter().filter(|i| i.divider).count(), 3);
        assert!(menu.items.iter().any(|i| i.id == "stop" && i.tone.as_deref() == Some("red")));
        // Every row is one item of the list: Up and Down move between them.
        let list = find_where(&n, &|x| x.group.is_some()).expect("the list group");
        // Rows sit in their group's block; every one is an item of the list.
        let mut items = vec![];
        list_items(list, &mut items);
        assert!(items.iter().filter(|k| k.starts_with("row-")).count() >= 10, "{items:?}");
        // Each group is its own tinted block; ungrouped rows are on the plain ground.
        // (The tint needs data.app.groups, which the shell fills: see the shots.)
        assert!(find(&n, "block-Chats").is_some(), "the Chats block");
        assert!(find(&n, "block-ungrouped").is_some_and(|b| b.style.fill.is_none()), "ungrouped rows are not");
    }

    fn node_has(n: &Node, key: &str) -> bool {
        find(n, key).is_some()
    }

    fn find<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
        find_where(n, &|x| key_of(x).as_deref() == Some(key))
    }

    fn find_where<'a>(n: &'a Node, f: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
        if f(n) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find_where(c, f))
    }

    #[test]
    fn builtins_load_against_their_fixtures() {
        for (name, _) in BUILTIN {
            let dir = std::env::temp_dir().join("sbxw-no-overrides");
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name);
            let mut n = 0;
            for f in std::fs::read_dir(&fixtures).unwrap_or_else(|_| panic!("no fixtures/{name}/")) {
                let f = f.unwrap().path();
                let fx = crate::fixture::Fixture::load(&f).unwrap();
                let s = ScreenState::new(name, &dir, fx.screen_data(&crate::fixture::AppView::default()));
                assert!(s.error.is_none(), "{name} with {}: {:?}", f.display(), s.error);
                assert!(s.last_good.is_some());
                n += 1;
            }
            assert!(n > 0, "fixtures/{name}/ is empty");
        }
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    /// On a virtual clock: a zone opens 180 ms after the pointer moved in
    /// it, not before; moving from one of its elements to another is no
    /// leave; leaving all of them shuts it 280 ms later.
    #[test]
    fn a_zone_opens_after_a_rest_and_shuts_after_leaving() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut z = Zones::default();
        z.hover("sb:a", "row-a", true, ms(0));
        z.moved("sb:a", ms(0));
        assert!(!z.tick(ms(170)) && z.active.is_none(), "not yet");
        assert!(z.tick(ms(180)) && z.active.as_deref() == Some("sb:a"));
        // Down from the row into its block: one zone, no leave.
        z.hover("sb:a", "row-a", false, ms(300));
        z.hover("sb:a", "sessions-block-a", true, ms(301));
        z.moved("sb:a", ms(305));
        assert!(!z.tick(ms(900)) && z.active.as_deref() == Some("sb:a"), "still open");
        // Out of the zone: shut 280 ms later.
        z.hover("sb:a", "sessions-block-a", false, ms(1000));
        assert!(!z.tick(ms(1270)));
        assert_eq!(z.due(), Some(ms(1280)));
        assert!(z.tick(ms(1280)) && z.active.is_none());
    }

    /// Sweeping the pointer down a list opens nothing; resting on one row
    /// opens that one only, and the one open before shuts.
    #[test]
    fn a_sweep_opens_nothing_and_one_zone_is_open_at_a_time() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut z = Zones::default();
        for (i, row) in ["a", "b", "c", "d"].iter().enumerate() {
            let at = i as u64 * 60;
            z.hover(&format!("sb:{row}"), "row", true, ms(at));
            z.moved(&format!("sb:{row}"), ms(at));
            assert!(!z.tick(ms(at + 50)));
            z.hover(&format!("sb:{row}"), "row", false, ms(at + 59));
        }
        assert!(!z.tick(ms(2000)) && z.active.is_none(), "a sweep opened {:?}", z.active);
        // Rest on b: b opens. Then rest on c: c opens and b is no longer open.
        z.hover("sb:b", "row", true, ms(3000));
        z.moved("sb:b", ms(3000));
        assert!(z.tick(ms(3200)) && z.active.as_deref() == Some("sb:b"));
        z.hover("sb:b", "row", false, ms(3300));
        z.hover("sb:c", "row", true, ms(3300));
        z.moved("sb:c", ms(3301));
        assert!(z.tick(ms(3500)) && z.active.as_deref() == Some("sb:c"));
    }

    /// A row that slides under a still pointer (a block above it opened)
    /// gets an enter but no move: it does not open, so blocks never open
    /// each other in a chain.
    #[test]
    fn a_layout_shift_alone_does_not_open_a_zone() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut z = Zones::default();
        z.hover("sb:a", "row", true, ms(0));
        z.moved("sb:a", ms(0));
        assert!(z.tick(ms(200)));
        // a's block opened: b's row moved down... and c's row slid under the pointer.
        z.hover("sb:a", "row", false, ms(250));
        z.hover("sb:c", "row", true, ms(250));
        assert!(z.tick(ms(600)) && z.active.is_none(), "a shut (the pointer is not over it)");
        assert!(!z.tick(ms(2000)) && z.active.is_none(), "c did not open without a move");
        z.moved("sb:c", ms(2100));
        assert!(z.tick(ms(2300)) && z.active.as_deref() == Some("sb:c"), "the next move opens it");
    }
}
