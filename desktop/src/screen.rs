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
use std::time::{Duration, SystemTime};

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
        let loaded = src.map_err(|e| format!("{file}: {e}")).and_then(|src| self.host.load(&file, &src));
        self.loads += 1;
        match loaded {
            Ok(()) => {
                log::info!("screen {} loaded from {source} (load #{})", self.name, self.loads);
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

    pub fn set_data(&mut self, data: J) {
        if data != self.data {
            self.data = data;
            self.redraw();
        }
    }

    fn redraw(&mut self) {
        match self.host.render(&self.data) {
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
        Kind::Input => {
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

fn body(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
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
    let mut b = Build { name: name.clone(), ui: &s.ui, overlays: vec![] };
    let mut panel = col().key(format!("screen-{}", s.name)).w_full().h_full().min_h(0.0).gap(8.0);
    if let Some(e) = &s.error {
        panel = panel.child(
            el().key(format!("screen-error-{}", s.name)).w_full().padding(8.0).radius(theme::radius("sm").unwrap()).fill(c("red-tint")).border(1.0, c("red")).child(
                text(format!("screen error, showing the last good view:\n{e}")).font(body(11.0, 400.0)).color(c("text-bright")).wrap().w_full(),
            ),
        );
    }
    if let Some(n) = &s.last_good {
        panel = panel.child(b.node::<S>(n));
    }
    (panel, b.overlays)
}

struct Build<'a> {
    name: Rc<str>,
    ui: &'a Ui,
    overlays: Vec<El>,
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
        Kind::Input | Kind::Menu | Kind::Select | Kind::Split => n.id.clone(),
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
                if st.truncate {
                    t = t.truncate().min_w(0.0);
                }
                t
            }
            Kind::Row | Kind::Column => {
                let mut b = boxed(if n.kind == Kind::Row { row().items_center() } else { col() }, st);
                for k in &n.children {
                    b = b.child(self.node::<S>(k));
                }
                if let Some(on) = &n.on {
                    let (on, arg) = (on.clone(), n.arg.clone());
                    b = b.role("button").cursor(Cursor::Pointer).hover(hex(st.hover.unwrap_or(theme::color_value("surface").unwrap()))).on_press(move |s: &mut S| fire(s, &name, &on, &arg, &J::Null));
                } else if let Some(h) = st.hover {
                    b = b.hover(hex(h));
                }
                b
            }
            Kind::List => {
                let mut l = col().w_full().border(1.0, c("border")).radius(theme::radius("sm").unwrap());
                for (i, k) in n.children.iter().enumerate() {
                    if i > 0 {
                        l = l.child(el().w_full().h(1.0).fill(c("elevated")));
                    }
                    l = l.child(el().w_full().px(8.0).py(st.gap.unwrap_or(4.0)).child(self.node::<S>(k)));
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
                let mut t = text_input(&edit, body(st.size.unwrap_or(12.0), 400.0))
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
                el().w_full().h(28.0).px(8.0).items_center().fill(c("bg")).border(1.0, c("border")).radius(theme::radius("sm").unwrap()).child(t)
            }
            Kind::Icon => {
                let i = crate::icons::get(&n.text).expect("checked icon");
                rune::icon(i, st.size.unwrap_or(14.0)).color(hex(st.color.unwrap_or(theme::color_value("text").unwrap())))
            }
            Kind::Badge => {
                let (fg, tint) = tone(n.variant.as_deref().unwrap_or("neutral"));
                el().px(5.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(hex(tint)).border(1.0, hex(fg)).child(text(n.text.clone()).font(body(10.0, 500.0)).color(hex(fg)))
            }
            Kind::Dot => {
                let (fg, _) = tone(n.variant.as_deref().unwrap_or("neutral"));
                let fg = if n.variant.as_deref().unwrap_or("neutral") == "neutral" { theme::color_value("dim").unwrap() } else { fg };
                el().w(7.0).h(7.0).radius(3.5).fill(hex(fg))
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
                let mut b = el().w(14.0).h(14.0).radius(4.0).border(1.0, c(if on { "blue" } else { "dim" })).items_center().justify_center();
                if on {
                    b = b.fill(c("blue")).child(rune::icon(crate::icons::get("check").unwrap(), 12.0).color(c("white")));
                }
                let mut r = row().gap(6.0).items_center().child(b).child(text(n.text.clone()).font(body(12.0, 400.0)).color(c("text"))).role("checkbox").checked(on);
                if !n.disabled {
                    if let Some(act) = n.on.clone() {
                        let arg = n.arg.clone();
                        r = r.cursor(Cursor::Pointer).on_press(move |s: &mut S| fire(s, &name, &act, &arg, &J::Bool(!on)));
                    }
                } else {
                    r = r.opacity(0.5);
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
            Kind::Progress => {
                let v = n.value.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                let (fg, _) = tone(n.variant.as_deref().unwrap_or("blue"));
                el().w_full().h(4.0).radius(2.0).fill(c("elevated")).child(el().w_frac(v).h_full().radius(2.0).fill(hex(fg)))
            }
        };
        let e = match (&key, n.kind) {
            // Inputs and menus key their inner part.
            (Some(k), k2) if !matches!(k2, Kind::Input | Kind::Menu | Kind::Select | Kind::Button | Kind::Markdown) => e.key(k.clone()),
            _ => e,
        };
        apply(e, st, &n.help)
    }

    fn button<S: Host>(&mut self, n: &Node, key: Option<String>) -> El {
        let variant = n.variant.as_deref().unwrap_or("neutral");
        let (fill, border, fg, hover) = match variant {
            "primary" => ("green-deep", "green-deep", "white", "green-strong"),
            "danger" => ("elevated", "border", "red", "red-tint"),
            "ghost" => ("", "", "text-muted", "elevated"),
            _ => ("elevated", "border", "text", "border"),
        };
        let fg = n.style.color.map(hex).unwrap_or_else(|| c(fg));
        let mut b = row().gap(4.0).items_center().px(if n.text.is_empty() { 4.0 } else { 7.0 }).py(3.0).radius(theme::radius("xs").unwrap()).role("button").label(if n.text.is_empty() { n.help.clone().unwrap_or_else(|| n.icon.clone().unwrap_or_default()) } else { n.text.clone() });
        if !fill.is_empty() {
            b = b.fill(c(fill)).border(1.0, c(border));
        }
        if let Some(k) = key {
            b = b.key(k);
        }
        if let Some(i) = n.icon.as_deref().and_then(crate::icons::get) {
            b = b.child(rune::icon(i, 12.0).color(fg));
        }
        if !n.text.is_empty() {
            b = b.child(text(n.text.clone()).font(text_style(&n.style, 11.0, 400.0)).color(fg));
        }
        if n.disabled {
            return b.opacity(0.5).disabled(true);
        }
        let (name, on, arg) = (self.name.clone(), n.on.clone().unwrap_or_default(), n.arg.clone());
        b.hover(c(hover)).cursor(Cursor::Pointer).on_press(move |s: &mut S| fire(s, &name, &on, &arg, &J::Null))
    }

    fn menu<S: Host>(&mut self, n: &Node, key: Option<String>) -> El {
        let id = n.id.clone().unwrap_or_default();
        let key = key.unwrap_or_else(|| id.clone());
        let open = self.ui.open.as_deref() == Some(id.as_str());
        let label = match n.kind {
            Kind::Select => n.items.iter().find(|i| Some(i.id.as_str()) == n.value.as_str()).map(|i| i.label.clone()).or_else(|| n.placeholder.clone()).unwrap_or_default(),
            _ => n.text.clone(),
        };
        let mut t = row().key(key.clone()).gap(6.0).items_center().px(8.0).h(26.0).radius(theme::radius("sm").unwrap()).role(if n.kind == Kind::Select { "combobox" } else { "button" }).label(label.clone()).expanded(open);
        t = match (n.kind, n.variant.as_deref()) {
            (Kind::Menu, Some("ghost")) => t.hover(c("elevated")),
            _ => t.fill(c("bg")).border(1.0, c(if open { "blue" } else { "border" })).hover(c("border")),
        };
        if let Some(i) = n.icon.as_deref().and_then(crate::icons::get) {
            t = t.child(rune::icon(i, 12.0).color(c("text")));
        }
        if !label.is_empty() {
            t = t.child(text(label).font(body(12.0, 400.0)).color(c("text-bright")));
        }
        t = t.child(rune::icon(crate::icons::get("chevron-down").unwrap(), 10.0).color(c("text-muted")));
        if n.disabled {
            return t.opacity(0.5).disabled(true);
        }
        let (name, id2) = (self.name.clone(), id.clone());
        t = t.cursor(Cursor::Pointer).on_press(move |s: &mut S| {
            let ui = &mut s.screen(&name).ui;
            ui.open = if ui.open.as_deref() == Some(id2.as_str()) { None } else { Some(id2.clone()) };
        });
        if open {
            let mut list = col().key(format!("{id}-list")).role("menu").anchored(key.clone(), Side::Bottom, rune::Align::Start, 4.0).min_w(140.0).padding(4.0).gap(2.0).fill(c("surface")).border(1.0, c("border")).radius(theme::radius("md").unwrap()).shadow(8.0, 24.0, rune::color::hex("#00000073").unwrap());
            for it in &n.items {
                list = list.child(self.item::<S>(n, &id, it));
            }
            self.overlays.push(list);
        }
        t
    }

    fn item<S: Host>(&self, n: &Node, id: &str, it: &Item) -> El {
        let chosen = n.kind == Kind::Select && n.value.as_str() == Some(it.id.as_str());
        let mut r = row().key(format!("{id}-{}", it.id)).gap(6.0).items_center().px(8.0).h(24.0).radius(theme::radius("xs").unwrap()).role("menuitem").label(it.label.clone());
        if let Some(i) = it.icon.as_deref().and_then(crate::icons::get) {
            r = r.child(rune::icon(i, 12.0).color(c("text")));
        }
        r = r.child(text(it.label.clone()).font(body(12.0, if chosen { 600.0 } else { 400.0 })).color(c("text-bright")).grow(1.0));
        if chosen {
            r = r.child(rune::icon(crate::icons::get("check").unwrap(), 12.0).color(c("blue")));
        }
        if it.disabled {
            return r.opacity(0.5).disabled(true);
        }
        let (name, on, arg, value) = (self.name.clone(), n.on.clone().unwrap_or_default(), n.arg.clone(), J::String(it.id.clone()));
        r.hover(c("elevated")).cursor(Cursor::Pointer).on_press(move |s: &mut S| {
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
    let t = body(st.size.unwrap_or(size), st.weight.unwrap_or(weight));
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
