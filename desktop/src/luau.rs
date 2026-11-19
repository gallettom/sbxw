//! The Luau host: runs one screen file in a locked-down VM and turns what it
//! returns into a [`Node`] tree the core knows how to draw.
//!
//! The VM gets no file, network or process access: only the `string`,
//! `table`, `math`, `utf8` and `bit32` libraries are opened, `require`,
//! `loadstring`, `getfenv` and `setfenv` are removed, then mlua's Luau
//! sandbox mode makes the globals read-only. Every call into the script runs
//! under a memory cap and an instruction budget enforced from Luau's
//! interrupt hook (fired on calls and loop back-edges). A screen can only
//! describe elements and return intents (data); the core decides what runs.
//!
//! A screen file returns a table:
//!
//! ```lua
//! return {
//!   init = function() return { selected = nil } end,       -- first load only
//!   render = function(state, data) return column{ ... } end,
//!   actions = { pick = function(state, arg) state.selected = arg end },
//! }
//! ```
//!
//! `state` lives in the VM across reloads: a new version of the file gets the
//! same table, so editing the file keeps what the person was doing.
//!
//! The element set is frozen: `docs/screens.md` describes every element,
//! prop and event, and [`SPEC`] is the same list in code (a test checks the
//! two agree).

use mlua::{Function, Lua, LuaOptions, RegistryKey, StdLib, Table, Value, VmState};
use serde_json::Value as J;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Row,
    Column,
    List,
    Scroll,
    Button,
    Input,
    Icon,
    Badge,
    Dot,
    Split,
    Spacer,
    Divider,
    Menu,
    Select,
    Checkbox,
    Progress,
    Markdown,
    /// A multi-line `input`: Return is a new line, Cmd/Ctrl+Return submits.
    Textarea,
    /// A picture a feature registered with the core (`Action::Image`), by its key.
    Image,
    /// A feature's live picture surface (`crate::surface`), by its `id`:
    /// drawn and fed by the core, it takes the space left.
    Surface,
    /// A feature's canvas (`crate::surface_canvas`), by its `id`: pictures,
    /// pins and notes under a camera the core pans and zooms.
    Canvas,
}

/// A width or height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    Pt(f32),
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Justify {
    Start,
    Center,
    End,
    Between,
}

/// Layout and look, shared by every element (see `docs/screens.md`, "Common props").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub w: Option<Size>,
    pub h: Option<Size>,
    pub min_w: Option<f32>,
    pub max_w: Option<f32>,
    pub min_h: Option<f32>,
    pub max_h: Option<f32>,
    pub grow: Option<f32>,
    pub pad: Option<f32>,
    pub px: Option<f32>,
    pub py: Option<f32>,
    pub gap: Option<f32>,
    pub fill: Option<u32>,
    pub border: Option<u32>,
    pub hover: Option<u32>,
    pub radius: Option<f32>,
    pub opacity: Option<f32>,
    pub align: Option<Align>,
    pub justify: Option<Justify>,
    // Text.
    pub color: Option<u32>,
    pub size: Option<f32>,
    pub weight: Option<f32>,
    pub mono: bool,
    pub wrap: bool,
    pub truncate: bool,
    /// `pills`: draw `[[Word]]` markers as keyword pills (emphasis.rs):
    /// `Some("")` by the daemon's global switch, `Some(sandbox)` by that
    /// sandbox's (and the app's own preference for it).
    pub pills: Option<String>,
    /// Out of the parent's flow, pinned to some of its edges (left, top, right, bottom).
    pub pin: Option<[Option<f32>; 4]>,
}

/// One entry of a `menu` or `select`.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub disabled: bool,
    /// The item's tooltip: the long explanation a short label leaves out.
    pub help: Option<String>,
    /// Its colour by meaning (`green`, `red`, `amber`, `blue`, `purple`):
    /// the icon in that colour, and a `red` item's label too (destructive).
    pub tone: Option<String>,
    /// The shortcut's keycaps, at the item's right end.
    pub keys: Vec<String>,
    /// A line between two groups of items (`{ divider = true }`, no id).
    pub divider: bool,
}

impl Item {
    pub fn plain(id: &str, label: &str, icon: Option<&str>) -> Item {
        Item { id: id.into(), label: label.into(), icon: icon.map(str::to_string), disabled: false, help: None, tone: None, keys: vec![], divider: false }
    }
}

/// One element, checked: every prop known, every value of the right type.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    /// The text of `text`/`badge`, the label of `button`/`menu`/`checkbox`, the name of `icon`.
    pub text: String,
    pub key: Option<String>,
    pub help: Option<String>,
    /// The action an event calls, and the value it passes.
    pub on: Option<String>,
    pub arg: J,
    pub on_submit: Option<String>,
    /// `input`, `split`, `menu`, `select`: names the core-held part of the element.
    pub id: Option<String>,
    /// `input` text, `select` value, `checkbox` checked, `progress` fraction, `split` ratio.
    pub value: J,
    pub placeholder: Option<String>,
    pub items: Vec<Item>,
    /// `select`: a scope picker (scope.rs): `"sandbox"`, or a table
    /// `{ kind = "sandbox", narrow = ..., narrow_icon = ..., clear = ... }`.
    pub scope: J,
    /// `select` with exactly two items: a two-position switch (switch.rs).
    pub switch: bool,
    /// `button`: neutral, primary, danger, ghost. `badge`/`dot`/`progress`: a tone.
    pub variant: Option<String>,
    pub icon: Option<String>,
    pub disabled: bool,
    pub secure: bool,
    /// `split` and `divider`: true for side by side (a vertical divider).
    pub vertical: bool,
    pub stick_bottom: bool,
    /// `scroll`: its edges fade over this many points where content is hidden under them.
    pub fade: Option<f32>,
    /// `textarea`: visible lines (default 4).
    pub rows: u32,
    /// `image`: `fit` (letterboxed, default) or `fill` (cropped).
    pub fill_image: bool,
    /// `row`/`column`: children with `reveal` show while it is hovered or
    /// holds keyboard focus (Rune's hover group).
    pub hover_group: bool,
    /// `row`/`column`: one hover zone with every element of the same name;
    /// the core tells the screen which zone the pointer rests in
    /// (`data.app.hover_zone`, screen.rs `Zones`).
    pub hover_zone: Option<String>,
    /// Any element: drawn only while the nearest `hover_group` is hovered or
    /// focused (still pressable and reachable with Tab, which lights it).
    pub reveal: bool,
    /// `row`/`column`: `"list"` makes its focusable children one keyboard
    /// group (arrows move between them, Tab enters at the selected one).
    pub group: Option<String>,
    /// `row`/`column` with `on`: the selected item of its group.
    pub selected: bool,
    /// `row`/`column` with `on`: called when Space is pressed while it has
    /// the keyboard (Enter calls `on`).
    pub on_space: Option<String>,
    /// `row`/`column` with `on`: called with the same `arg` when Left or
    /// Right is pressed while it has the keyboard (a tree's fold and open).
    pub on_left: Option<String>,
    pub on_right: Option<String>,
    /// `row`/`column`: what overflows it is cut at its edges.
    pub clip: bool,
    /// `button`, `menu`: the dense size (24 pt, for bars) instead of 28.
    pub dense: bool,
    /// `button`, `menu`: `tab_stop = false` leaves it out of the Tab order
    /// (it stays pressable): a row's secondary actions, reached by its keys.
    pub skip_tab: bool,
    /// `button`, `menu`: the shortcut's keycaps shown in its tooltip.
    pub keys: Vec<String>,
    /// `button`, `menu`: its colour by meaning (the action colour code,
    /// docs/screens.md "Colour by meaning"): the icon in it, a tint under it
    /// on hover and focus. `input`, `textarea`: `compose` for a chat input.
    pub tone: Option<String>,
    /// `button`: one text line tall (20 pt drawn, a 24 pt target).
    pub compact: bool,
    /// `badge`: a round bubble (a count alone).
    pub pill: bool,
    /// `column`: its children fold open and shut smoothly (a growth stack
    /// whose rows glide to their new height and fade what they gain or lose).
    pub folds: bool,
    /// `row`/`column`: lines under its `help` in the tooltip, each what a
    /// key does and its keycaps (`{ { "Open", "Enter" }, { "Start or stop", "Space" } }`).
    pub shortcuts: Vec<(String, Vec<String>)>,
    /// `row`/`column`: vector drawing under its children, in its own points
    /// from its top-left (Rune's `Shape`): the Worktrees view's branch graph.
    pub paint: Option<Paint>,
    pub style: Style,
    pub children: Vec<Node>,
}

/// A `paint` prop: polylines and dots, drawn in order (strokes, then dots).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Paint {
    /// Points, width (points), colour.
    pub strokes: Vec<(Vec<[f32; 2]>, f32, u32)>,
    /// Centre, radius, colour, hollow (a ring).
    pub dots: Vec<([f32; 2], f32, u32, bool)>,
}

/// What one `paint` may hold: a branch graph has at most a few hundred.
pub const PAINT_STROKES: usize = 400;
pub const PAINT_DOTS: usize = 600;
pub const PAINT_POINTS: usize = 8000;

impl Node {
    fn new(kind: Kind) -> Self {
        Node {
            kind,
            text: String::new(),
            key: None,
            help: None,
            on: None,
            arg: J::Null,
            on_submit: None,
            id: None,
            value: J::Null,
            placeholder: None,
            items: vec![],
            scope: J::Null,
            switch: false,
            variant: None,
            icon: None,
            disabled: false,
            secure: false,
            vertical: false,
            stick_bottom: false,
            fade: None,
            rows: 4,
            fill_image: false,
            hover_group: false,
            hover_zone: None,
            reveal: false,
            group: None,
            selected: false,
            on_space: None,
            on_left: None,
            on_right: None,
            clip: false,
            skip_tab: false,
            dense: false,
            keys: vec![],
            tone: None,
            compact: false,
            pill: false,
            folds: false,
            shortcuts: vec![],
            paint: None,
            style: Style::default(),
            children: vec![],
        }
    }

    pub fn text(s: &str) -> Self {
        Node { text: s.into(), ..Node::new(Kind::Text) }
    }
}

/// What positional entries of an element's table are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Positional {
    None,
    /// `[1]` is a string: the text, label or icon name.
    Text,
    /// Children.
    Children,
    /// Exactly two children.
    Two,
}

/// Props every element takes.
pub const COMMON: &[&str] = &["key", "help", "w", "h", "min_w", "max_w", "min_h", "max_h", "grow", "pad", "px", "py", "fill", "border", "radius", "opacity", "reveal", "pin"];
const TEXT_PROPS: &[&str] = &["color", "size", "weight", "mono", "wrap", "truncate", "pills"];
const BOX_PROPS: &[&str] = &["gap", "align", "justify", "hover", "on", "arg", "hover_group", "hover_zone", "group", "selected", "on_space", "on_left", "on_right", "clip", "folds", "shortcuts", "paint", "wrap"];

/// The frozen element set: name, kind, positional entries, props beyond [`COMMON`].
pub const SPEC: &[(&str, Kind, Positional, &[&str])] = &[
    ("text", Kind::Text, Positional::Text, TEXT_PROPS),
    ("row", Kind::Row, Positional::Children, BOX_PROPS),
    ("column", Kind::Column, Positional::Children, BOX_PROPS),
    ("list", Kind::List, Positional::Children, &["gap"]),
    ("scroll", Kind::Scroll, Positional::Children, &["gap", "stick_bottom", "fade"]),
    ("button", Kind::Button, Positional::Text, &["on", "arg", "variant", "icon", "disabled", "color", "dense", "keys", "tab_stop", "hover", "tone", "compact"]),
    ("input", Kind::Input, Positional::None, &["id", "value", "placeholder", "on", "on_submit", "arg", "secure", "disabled", "tone"]),
    ("icon", Kind::Icon, Positional::Text, &["color", "size"]),
    ("badge", Kind::Badge, Positional::Text, &["tone", "pill"]),
    ("dot", Kind::Dot, Positional::None, &["tone"]),
    ("split", Kind::Split, Positional::Two, &["id", "vertical", "ratio"]),
    ("spacer", Kind::Spacer, Positional::None, &[]),
    ("divider", Kind::Divider, Positional::None, &["vertical", "color"]),
    ("menu", Kind::Menu, Positional::Text, &["id", "items", "on", "arg", "icon", "variant", "disabled", "dense", "keys", "tab_stop", "tone"]),
    ("select", Kind::Select, Positional::None, &["id", "items", "value", "on", "arg", "placeholder", "disabled", "scope", "switch"]),
    ("checkbox", Kind::Checkbox, Positional::Text, &["value", "on", "arg", "disabled"]),
    ("progress", Kind::Progress, Positional::None, &["value", "tone"]),
    ("markdown", Kind::Markdown, Positional::Text, &["color", "size", "on", "arg", "pills"]),
    ("textarea", Kind::Textarea, Positional::None, &["id", "value", "placeholder", "on", "on_submit", "arg", "disabled", "rows", "tone"]),
    ("image", Kind::Image, Positional::None, &["fit"]),
    ("surface", Kind::Surface, Positional::None, &["id"]),
    ("canvas", Kind::Canvas, Positional::None, &["id"]),
];

/// Tones of `input` and `textarea`: `compose` is where a person writes to
/// an agent (its own ground, and dictation goes there); `plain` the rest.
pub const FIELD_TONES: &[&str] = &["plain", "compose"];

/// Tones of `badge`, `dot` and `progress`.
pub const TONES: &[&str] = &["neutral", "blue", "green", "amber", "red", "purple"];
/// Variants of `button` and `menu`.
pub const VARIANTS: &[&str] = &["neutral", "primary", "danger", "ghost"];

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    /// Interrupt callbacks allowed per call into the script.
    pub interrupts: u64,
    /// Wall clock allowed per call into the script: generous (1 s), it only
    /// catches a call that blocks. The interrupt count is the real limit (it
    /// is the same on a busy machine), so load never breaks a screen.
    pub wall: Duration,
    /// Elements one render may return.
    pub nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { memory_bytes: 16 << 20, interrupts: 200_000, wall: Duration::from_secs(1), nodes: 5_000 }
    }
}

/// Element constructors, defined in Luau so a screen reads like markup. Each
/// tags its table with a `kind`; the core checks the rest.
const PRELUDE: &str = r#"
local function mk(kind) return function(t)
  if t == nil then t = {} elseif type(t) ~= "table" then t = { t } end
  t.kind = kind
  return t
end end
for _, k in ipairs({ "text", "row", "column", "list", "scroll", "button", "input", "icon", "badge",
                     "dot", "split", "spacer", "divider", "menu", "select", "checkbox", "progress", "markdown", "textarea", "image", "surface", "canvas" }) do
  _G[k] = mk(k)
end
intent = function(name, args)
  if type(name) ~= "string" then error("intent(name, args): name must be a string", 2) end
  return { kind = "intent", name = name, args = args or {} }
end
-- Long lists (docs/screens.md, "Long lists"): a render is capped at 5000
-- elements, so a list draws its first rows and a "Show N more" block.
-- `more_rows(count, opts)` draws that block from a count the core made
-- (`{total, shown, more, label}`, `screen::paged`), or false when nothing
-- is left; `paged(items, opts)` cuts a list the screen filtered itself and
-- returns the rows to draw and that block. opts: `page` (rows a page, 200),
-- `pages` (pages shown, 1), `key` (the button's key; its box is
-- `<key>-box`), `on` and `arg` (the press), `what` ("rows"), `hint`.
more_rows = function(c, o)
  o = o or {}
  if type(c) ~= "table" or (c.more or 0) <= 0 then return false end
  local key = o.key or "more"
  local label = c.label
  if label == nil or label == "" then label = tostring(c.total) .. " " .. (o.what or "rows") .. ", showing " .. tostring(c.shown) end
  if o.hint then label = label .. ". " .. o.hint end
  return column{ gap = 4, w = "full", py = 4, key = key .. "-box",
    text{ label, size = "sm", color = "text-muted", wrap = true },
    button{ "Show " .. tostring(c.more) .. " more", key = key, on = o.on or "more", arg = o.arg, variant = "ghost", tone = "blue",
      help = tostring(c.total - c.shown) .. " more not drawn" } }
end
-- A sandbox picker, as the explorer draws it: the sandbox's colour square
-- (data.app.groups), then the select. opts: `id`, `value`, `items`
-- ({id, label}), `on`, `placeholder`, `disabled`.
sandbox_picker = function(data, o)
  o = o or {}
  local groups = ((data or {}).app or {}).groups or {}
  local id = o.id or "sandbox"
  return row{ gap = 8, key = id .. "-row",
    column{ w = 8, h = 8, radius = 2, fill = (o.value and groups[o.value]) or "text-dim", key = id .. "-color" },
    select{ id = id, value = o.value or "", items = o.items or {}, on = o.on, placeholder = o.placeholder, disabled = o.disabled } }
end
-- An overlay over the pane area wide enough for two columns (760 pt and
-- more, data.app.overlay.w; a screen drawn alone counts as wide).
overlay_wide = function(data)
  local o = ((data or {}).app or {}).overlay or {}
  return (o.w or 1000) >= 760
end
paged = function(items, o)
  o = o or {}
  local page = o.page or 200
  local n = page * (o.pages or 1)
  local total = #items
  if total <= n then return items, false end
  local shown = {}
  for i = 1, n do shown[i] = items[i] end
  return shown, more_rows({ total = total, shown = n, more = math.min(page, total - n) }, o)
end
-- `data` and `data.app` fill their fields on first read (luau.rs,
-- `lazy_data`): `pairs` and `next` over one fill it all first.
do
  local raw_pairs, raw_next = pairs, next
  local function fill(t)
    local mt = type(t) == "table" and getmetatable(t)
    if type(mt) == "table" and rawget(mt, "__lazy") then mt.__iter(t) end
  end
  pairs = function(t) fill(t) return raw_pairs(t) end
  next = function(t, k) if k == nil then fill(t) end return raw_next(t, k) end
end
"#;

pub struct LuauHost {
    lua: Lua,
    limits: Limits,
    budget: Rc<Budget>,
    /// The last screen table that loaded cleanly.
    module: Option<RegistryKey>,
    /// The screen's state, kept across reloads.
    state: Option<RegistryKey>,
    /// `theme::generation()` when the `theme` global was last filled: a
    /// switch of theme refills its colours before the next render.
    theme_gen: Cell<u64>,
}

struct Budget {
    count: Cell<u64>,
    deadline: Cell<Option<Instant>>,
}

impl LuauHost {
    /// Bytes the Luau VM holds now (its allocator's count: tables, strings,
    /// functions, garbage not yet collected).
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// A full collection now (MEMORY.md): garbage the incremental collector
    /// has not reached yet goes. True when it ran.
    pub fn collect(&self) -> bool {
        self.lua.gc_collect().is_ok()
    }

    pub fn new(limits: Limits) -> mlua::Result<Self> {
        let libs = StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::BIT;
        let lua = Lua::new_with(libs, LuaOptions::default())?;
        {
            let g = lua.globals();
            for name in ["require", "loadstring", "getfenv", "setfenv", "collectgarbage", "load", "dofile", "loadfile"] {
                g.raw_set(name, Value::Nil)?;
            }
            let theme = json_to_lua(&lua, &crate::theme::to_json())?;
            if let Value::Table(t) = &theme {
                readonly(t);
            }
            g.raw_set("theme", theme)?;
        }
        lua.load(PRELUDE).set_name("=prelude").exec()?;
        lua.sandbox(true)?;
        lua.set_memory_limit(limits.memory_bytes)?;
        let budget = Rc::new(Budget { count: Cell::new(0), deadline: Cell::new(None) });
        let b = budget.clone();
        let max = limits.interrupts;
        // `Lua` is not `Send` here (no "send" feature), so the callback may hold an `Rc`.
        lua.set_interrupt(move |_| {
            let n = b.count.get() + 1;
            b.count.set(n);
            let late = b.deadline.get().is_some_and(|d| Instant::now() > d);
            if n > max {
                return Err(mlua::Error::runtime(format!("instruction budget exceeded ({n} interrupts)")));
            }
            if late {
                return Err(mlua::Error::runtime(format!("time budget exceeded (still running after {n} interrupts)")));
            }
            Ok(VmState::Continue)
        });
        Ok(Self { lua, limits, budget, module: None, state: None, theme_gen: Cell::new(crate::theme::generation()) })
    }

    fn arm(&self) {
        self.budget.count.set(0);
        self.budget.deadline.set(Some(Instant::now() + self.limits.wall));
    }

    fn disarm(&self) {
        self.budget.deadline.set(None);
    }

    /// Tests only: the interrupts the last render, act or load counted (the
    /// count the instruction budget is checked against; kept after the call).
    #[cfg(test)]
    pub fn interrupts_used(&self) -> u64 {
        self.budget.count.get()
    }

    fn guarded<R>(&self, f: impl FnOnce(&Lua) -> mlua::Result<R>) -> Result<R, String> {
        self.arm();
        let r = f(&self.lua).map_err(|e| e.to_string());
        self.disarm();
        r
    }

    /// Load (or reload) a screen's source. On error the previous version stays.
    pub fn load(&mut self, name: &str, source: &str) -> Result<(), String> {
        let module: Table = self.guarded(|lua| {
            let v: Value = lua.load(source).set_name(format!("={name}")).call(())?;
            match v {
                Value::Table(t) => {
                    let _: Function = t.get("render").map_err(|_| mlua::Error::runtime("screen has no render function"))?;
                    Ok(t)
                }
                _ => Err(mlua::Error::runtime("a screen file must return a table")),
            }
        })?;
        if self.state.is_none() {
            let init: Option<Function> = module.get("init").map_err(|e| e.to_string())?;
            let state: Table = match init {
                Some(f) => self.guarded(|_| f.call(()))?,
                None => self.lua.create_table().map_err(|e| e.to_string())?,
            };
            self.state = Some(self.lua.create_registry_value(state).map_err(|e| e.to_string())?);
        }
        self.module = Some(self.lua.create_registry_value(module).map_err(|e| e.to_string())?);
        Ok(())
    }

    /// Render with the current module and state; `data` is what the core hands in.
    pub fn render(&self, data: &J) -> Result<Node, String> {
        self.render_tracked(data).map(|(n, _)| n)
    }

    /// [`Self::render`], and what of `data` the render read. `data` reaches
    /// the screen as a table whose top-level fields (and `app`'s) are turned
    /// into Lua only when the screen reads them: converting the whole data
    /// for each render was most of its time. The paths read (`sessions`,
    /// `app.pages`; `*` or `app.*` when a level was iterated) let the core
    /// skip a render whose inputs did not change (screen.rs).
    pub fn render_tracked(&self, data: &J) -> Result<(Node, Reads), String> {
        let (module, state) = self.parts()?;
        let reads = Rc::new(RefCell::new(Reads::default()));
        let root = Rc::new(data.clone());
        let tree: Value = self.guarded(|lua| {
            let render: Function = module.get("render")?;
            let data = lazy_data(lua, &root, &[], &reads)?;
            render.call((state, data))
        })?;
        let mut count = 0;
        let node = to_node(&tree, 0, &mut count, self.limits.nodes)?;
        let reads = std::mem::take(&mut *reads.borrow_mut());
        Ok((node, reads))
    }

    /// Refills the read-only `theme` table's colours and mode from the active
    /// palette when it changed since the table was filled (the table stays
    /// the same object, read-only to screens). The screen then loads its
    /// chunk again (screen.rs, `redraw`): Luau resolved its reads at load.
    pub fn sync_theme(&self) {
        let now = crate::theme::generation();
        if self.theme_gen.replace(now) == now {
            return;
        }
        let fresh = crate::theme::to_json();
        let refill = || -> mlua::Result<()> {
            // `get`, not `raw_get`: once sandboxed, the globals a script sees are
            // a proxy over the table the theme was put in.
            let theme: Table = self.lua.globals().get("theme")?;
            let colors: Table = theme.raw_get("color")?;
            theme.set_readonly(false);
            colors.set_readonly(false);
            if let Some(m) = fresh["color"].as_object() {
                for (k, v) in m {
                    colors.raw_set(k.as_str(), v.as_str().unwrap_or_default())?;
                }
            }
            theme.raw_set("mode", fresh["mode"].as_str().unwrap_or_default())?;
            colors.set_readonly(true);
            theme.set_readonly(true);
            Ok(())
        };
        if let Err(e) = refill() {
            log::warn!("screen theme: {e}");
        }
    }

    /// Run an action, `action(state, arg, value)`: `arg` is the element's `arg`
    /// prop, `value` what the event carries. Returns what the action returned,
    /// as JSON (intents, checked by the caller with `intent::parse`).
    pub fn act(&self, action: &str, arg: &J, value: &J) -> Result<J, String> {
        let (module, state) = self.parts()?;
        let out: Value = self.guarded(|lua| {
            let actions: Option<Table> = module.get("actions")?;
            let f: Option<Function> = actions.map(|a| a.get(action)).transpose()?.flatten();
            match f {
                Some(f) => f.call((state, json_to_lua(lua, arg)?, json_to_lua(lua, value)?)),
                None => Err(mlua::Error::runtime(format!("no action '{action}'"))),
            }
        })?;
        lua_to_json(&out, 0).map_err(|e| format!("action '{action}' returned {e}"))
    }

    /// Run arbitrary source under the same limits (used by the tests).
    pub fn eval(&self, source: &str) -> Result<Value, String> {
        self.guarded(|lua| lua.load(source).set_name("=eval").call(()))
    }

    fn parts(&self) -> Result<(Table, Table), String> {
        let (Some(m), Some(s)) = (&self.module, &self.state) else { return Err("no screen loaded".into()) };
        let m: Table = self.lua.registry_value(m).map_err(|e| e.to_string())?;
        let s: Table = self.lua.registry_value(s).map_err(|e| e.to_string())?;
        Ok((m, s))
    }
}

fn readonly(t: &Table) {
    for (_, v) in t.pairs::<Value, Value>().flatten() {
        if let Value::Table(c) = v {
            readonly(&c);
        }
    }
    t.set_readonly(true);
}

/// What of the data a render read: field paths from the top (`["sessions"]`,
/// `["app", "pages"]`); a level iterated with `pairs` reads as its path
/// with `*` (`["*"]`, `["app", "*"]`); a level itself read lazily as its
/// path with `?` (only whether it is still an object counts).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reads(pub std::collections::BTreeSet<Vec<String>>);

impl Reads {
    /// Whether the parts read differ between `old` and `new`.
    pub fn changed(&self, old: &J, new: &J) -> bool {
        self.0.iter().any(|path| {
            let (mut a, mut b) = (old, new);
            for (i, k) in path.iter().enumerate() {
                if k == "*" {
                    return a != b;
                }
                if k == "?" {
                    return a.is_object() != b.is_object();
                }
                a = &a[k.as_str()];
                b = &b[k.as_str()];
                if i + 1 == path.len() {
                    return a != b;
                }
            }
            a != b
        })
    }
}

/// The levels converted lazily: the top, and `app`'s fields (the core's own
/// state, many small parts of which change on their own).
const LAZY_DEPTH: usize = 2;

/// Whether the value at `path` of `root` is turned into a lazy table.
fn lazy_at(root: &J, path: &[String]) -> bool {
    let mut v: &J = root;
    for k in path {
        v = &v[k.as_str()];
    }
    matches!(v, J::Object(_)) && (path.is_empty() || (path.len() < LAZY_DEPTH && path[0] == "app"))
}

/// The JSON object at `path` of `root`, as a Lua table whose fields are
/// converted on first read (and kept, so a second read is a plain lookup),
/// each read noted in `reads`. Anything else, or deeper, converts at once.
fn lazy_data(lua: &Lua, root: &Rc<J>, path: &[String], reads: &Rc<RefCell<Reads>>) -> mlua::Result<Value> {
    let mut v: &J = root;
    for k in path {
        v = &v[k.as_str()];
    }
    if !lazy_at(root, path) {
        return json_to_lua(lua, v);
    }
    let t = lua.create_table()?;
    let mt = lua.create_table()?;
    let (r, rd, p) = (root.clone(), reads.clone(), path.to_vec());
    mt.raw_set(
        "__index",
        lua.create_function(move |lua, (t, k): (Table, Value)| {
            let Value::String(k) = k else { return Ok(Value::Nil) };
            let k = k.to_string_lossy();
            let mut sub = p.clone();
            sub.push(k);
            let v = lazy_data(lua, &r, &sub, &rd)?;
            // A level read lazily in turn is noted by what is read inside
            // it; of itself, only that it is an object (`?`).
            let mut noted = sub.clone();
            if lazy_at(&r, &sub) {
                noted.push("?".into());
            }
            rd.borrow_mut().0.insert(noted);
            if !v.is_nil() {
                t.raw_set(sub.last().unwrap().as_str(), v.clone())?;
            }
            Ok(v)
        })?,
    )?;
    let (r, rd, p) = (root.clone(), reads.clone(), path.to_vec());
    mt.raw_set(
        "__iter",
        lua.create_function(move |lua, t: Table| {
            let mut all = p.clone();
            all.push("*".into());
            rd.borrow_mut().0.insert(all);
            let mut v: &J = &r;
            for k in &p {
                v = &v[k.as_str()];
            }
            if let J::Object(o) = v {
                for k in o.keys() {
                    if t.raw_get::<Value>(k.as_str())?.is_nil() {
                        let mut sub = p.clone();
                        sub.push(k.clone());
                        let x = lazy_data(lua, &r, &sub, &rd)?;
                        t.raw_set(k.as_str(), x)?;
                    }
                }
            }
            let next: Function = lua.globals().get("next")?;
            Ok((next, t))
        })?,
    )?;
    mt.raw_set("__lazy", true)?;
    t.set_metatable(Some(mt))?;
    Ok(Value::Table(t))
}

pub fn json_to_lua(lua: &Lua, v: &J) -> mlua::Result<Value> {
    Ok(match v {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(a) => {
            let t = lua.create_table()?;
            for (i, x) in a.iter().enumerate() {
                t.raw_set(i + 1, json_to_lua(lua, x)?)?;
            }
            Value::Table(t)
        }
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, x) in o {
                t.raw_set(k.as_str(), json_to_lua(lua, x)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A Luau value as JSON: a table with keys 1..n only is an array (an empty
/// table is `[]`), any other table an object with string keys.
pub fn lua_to_json(v: &Value, depth: usize) -> Result<J, String> {
    if depth > 32 {
        return Err("a value nested too deep".into());
    }
    Ok(match v {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => J::from(*i),
        Value::Number(n) => serde_json::Number::from_f64(*n).map(J::Number).ok_or("a number that is not finite")?,
        Value::String(s) => J::String(s.to_string_lossy()),
        Value::Table(t) => {
            let mut ints = vec![];
            let mut obj = serde_json::Map::new();
            for pair in t.pairs::<Value, Value>() {
                let (k, x) = pair.map_err(|e| e.to_string())?;
                match k {
                    Value::Integer(i) if i >= 1 => ints.push((i as usize, lua_to_json(&x, depth + 1)?)),
                    Value::Number(n) if n >= 1.0 && n.fract() == 0.0 => ints.push((n as usize, lua_to_json(&x, depth + 1)?)),
                    Value::String(s) => {
                        obj.insert(s.to_string_lossy(), lua_to_json(&x, depth + 1)?);
                    }
                    other => return Err(format!("a table key of type {}", other.type_name())),
                }
            }
            ints.sort_by_key(|(i, _)| *i);
            if obj.is_empty() && ints.iter().enumerate().all(|(n, (i, _))| *i == n + 1) {
                J::Array(ints.into_iter().map(|(_, x)| x).collect())
            } else {
                for (i, x) in ints {
                    obj.insert(i.to_string(), x);
                }
                J::Object(obj)
            }
        }
        other => return Err(format!("a {} (only nil, booleans, numbers, strings and tables)", other.type_name())),
    })
}

const MAX_DEPTH: usize = 32;

fn to_node(v: &Value, depth: usize, count: &mut usize, max: usize) -> Result<Node, String> {
    if depth > MAX_DEPTH {
        return Err("element tree too deep".into());
    }
    *count += 1;
    if *count > max {
        return Err(format!("more than {max} elements in one render"));
    }
    let t = match v {
        Value::Table(t) => t,
        Value::String(s) => return Ok(Node::text(&s.to_string_lossy())),
        other => return Err(format!("expected an element, got {}", other.type_name())),
    };
    let kind_name: Option<String> = t.raw_get("kind").map_err(|e| e.to_string())?;
    let kind_name = kind_name.ok_or("a table that is not an element (make it with text{...}, row{...}, ...)")?;
    let &(name, kind, positional, props) = SPEC.iter().find(|s| s.0 == kind_name).ok_or_else(|| format!("unknown element '{kind_name}'"))?;
    let mut n = Node::new(kind);
    let mut pos: Vec<(usize, Value)> = vec![];
    for pair in t.pairs::<Value, Value>() {
        let (k, x) = pair.map_err(|e| e.to_string())?;
        match k {
            Value::Integer(i) if i >= 1 => pos.push((i as usize, x)),
            Value::Number(f) if f >= 1.0 && f.fract() == 0.0 => pos.push((f as usize, x)),
            Value::String(s) => {
                let k = s.to_string_lossy();
                if k == "kind" {
                    continue;
                }
                if !COMMON.contains(&k.as_str()) && !props.contains(&k.as_str()) {
                    return Err(format!("{name}: unknown prop '{k}'"));
                }
                prop(&mut n, name, &k, &x).map_err(|e| format!("{name}.{k}: {e}"))?;
            }
            other => return Err(format!("{name}: a key of type {}", other.type_name())),
        }
    }
    pos.sort_by_key(|(i, _)| *i);
    // `false` in a child list means "nothing here" (`cond and x or false`).
    let pos: Vec<Value> = pos.into_iter().map(|(_, v)| v).filter(|v| !matches!(v, Value::Boolean(false))).collect();
    match positional {
        Positional::None if !pos.is_empty() => return Err(format!("{name} takes no positional values")),
        Positional::None => {}
        Positional::Text => match pos.as_slice() {
            [] => {}
            [Value::String(s)] => n.text = s.to_string_lossy(),
            [Value::Integer(i)] => n.text = i.to_string(),
            [Value::Number(f)] => n.text = f.to_string(),
            _ => return Err(format!("{name} takes one string, as {name}{{ \"...\" }}")),
        },
        Positional::Children | Positional::Two => {
            for c in &pos {
                n.children.push(to_node(c, depth + 1, count, max).map_err(|e| format!("{name} > {e}"))?);
            }
            if positional == Positional::Two && n.children.len() != 2 {
                return Err(format!("{name} takes exactly two children, got {}", n.children.len()));
            }
        }
    }
    check(&n, name)?;
    Ok(n)
}

fn check(n: &Node, name: &str) -> Result<(), String> {
    let needs_id = matches!(n.kind, Kind::Input | Kind::Textarea | Kind::Split | Kind::Menu | Kind::Select | Kind::Surface | Kind::Canvas);

    if n.kind == Kind::Image && n.key.is_none() {
        return Err(format!("{name} needs a 'key' (the picture's, as the feature registered it)"));
    }
    if needs_id && n.id.is_none() {
        return Err(format!("{name} needs an 'id'"));
    }
    if matches!(n.kind, Kind::Button | Kind::Menu | Kind::Select | Kind::Checkbox) && n.on.is_none() && !n.disabled {
        return Err(format!("{name} needs 'on' (the action it calls) unless disabled"));
    }
    if n.kind == Kind::Icon && !crate::icons::exists(&n.text) {
        return Err(format!("unknown icon '{}' (known: {})", n.text, crate::icons::names().join(", ")));
    }
    if let Some(i) = &n.icon {
        if !crate::icons::exists(i) {
            return Err(format!("{name}: unknown icon '{i}'"));
        }
    }
    Ok(())
}

fn f32_of(v: &Value) -> Option<f32> {
    match v {
        Value::Integer(i) => Some(*i as f32),
        Value::Number(f) => Some(*f as f32),
        _ => None,
    }
}

fn string_of(v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.to_string_lossy()),
        _ => Err(format!("expected a string, got {}", v.type_name())),
    }
}

fn bool_of(v: &Value) -> Result<bool, String> {
    match v {
        Value::Boolean(b) => Ok(*b),
        Value::Nil => Ok(false),
        _ => Err(format!("expected true or false, got {}", v.type_name())),
    }
}

fn num_of(v: &Value) -> Result<f32, String> {
    f32_of(v).ok_or_else(|| format!("expected a number, got {}", v.type_name()))
}

/// A number, or a name in a theme table.
fn num_or(v: &Value, named: fn(&str) -> Result<f32, String>) -> Result<f32, String> {
    match v {
        Value::String(s) => named(&s.to_string_lossy()),
        _ => num_of(v),
    }
}

fn size_of(v: &Value) -> Result<Size, String> {
    match v {
        Value::String(s) if s.to_string_lossy() == "full" => Ok(Size::Full),
        _ => num_of(v).map(Size::Pt).map_err(|_| "expected a number of points or \"full\"".into()),
    }
}

/// `paint = { strokes = { { points = { x1, y1, x2, y2, ... }, width = 2, color = "blue" } },
/// dots = { { x = 4, y = 4, r = 4, color = "green", hollow = false } } }`.
fn paint_of(v: &Value) -> Result<Paint, String> {
    let Value::Table(t) = v else { return Err("paint: expected { strokes = {...}, dots = {...} }".into()) };
    let num = |x: &Value, what: &str| -> Result<f32, String> { num_of(x).map_err(|e| format!("paint: {what}: {e}")).and_then(|n| if n.is_finite() { Ok(n) } else { Err(format!("paint: {what}: not a number")) }) };
    let mut p = Paint::default();
    let mut points = 0usize;
    for pair in t.pairs::<String, Value>() {
        let (k, list) = pair.map_err(|_| "paint: keys are strokes and dots".to_string())?;
        let Value::Table(list) = list else { return Err(format!("paint.{k}: expected a list")) };
        match k.as_str() {
            "strokes" => {
                for s in list.sequence_values::<Value>() {
                    let Value::Table(s) = s.map_err(|e| e.to_string())? else { return Err("paint.strokes: each is { points = {...}, width =, color = }".into()) };
                    let mut pts = vec![];
                    let mut xy: Vec<f32> = vec![];
                    if let Ok(Value::Table(ps)) = s.get::<Value>("points") {
                        for x in ps.sequence_values::<Value>() {
                            xy.push(num(&x.map_err(|e| e.to_string())?, "points")?);
                        }
                    }
                    if xy.len() % 2 != 0 || xy.len() < 4 {
                        return Err("paint.strokes: points are x, y pairs, two at least".into());
                    }
                    for c in xy.chunks(2) {
                        pts.push([c[0], c[1]]);
                    }
                    points += pts.len();
                    let width = match s.get::<Value>("width").map_err(|e| e.to_string())? { Value::Nil => 1.5, x => num(&x, "width")?.clamp(0.5, 8.0) };
                    let color = match s.get::<Value>("color").map_err(|e| e.to_string())? { Value::Nil => crate::theme::color_value("text-muted").unwrap(), x => color_of(&x)? };
                    p.strokes.push((pts, width, color));
                }
            }
            "dots" => {
                for d in list.sequence_values::<Value>() {
                    let Value::Table(d) = d.map_err(|e| e.to_string())? else { return Err("paint.dots: each is { x =, y =, r =, color = }".into()) };
                    let g = |k: &str| d.get::<Value>(k).map_err(|e| e.to_string());
                    let (x, y) = (num(&g("x")?, "x")?, num(&g("y")?, "y")?);
                    let r = match g("r")? { Value::Nil => 4.0, v => num(&v, "r")?.clamp(1.0, 12.0) };
                    let color = match g("color")? { Value::Nil => crate::theme::color_value("text-muted").unwrap(), v => color_of(&v)? };
                    let hollow = match g("hollow")? { Value::Nil => false, v => bool_of(&v)? };
                    p.dots.push(([x, y], r, color, hollow));
                }
            }
            other => return Err(format!("paint: unknown key '{other}' (strokes, dots)")),
        }
    }
    if p.strokes.len() > PAINT_STROKES || p.dots.len() > PAINT_DOTS || points > PAINT_POINTS {
        return Err(format!("paint: at most {PAINT_STROKES} strokes, {PAINT_DOTS} dots and {PAINT_POINTS} points"));
    }
    Ok(p)
}

fn color_of(v: &Value) -> Result<u32, String> {
    crate::theme::parse_color(&string_of(v)?)
}

fn one_of(v: &Value, allowed: &[&str]) -> Result<String, String> {
    let s = string_of(v)?;
    if allowed.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(format!("'{s}' is not one of {}", allowed.join(", ")))
    }
}

fn prop(n: &mut Node, name: &str, k: &str, v: &Value) -> Result<(), String> {
    let st = &mut n.style;
    match k {
        "key" => n.key = Some(string_of(v)?),
        "help" => n.help = Some(string_of(v)?),
        "w" => st.w = Some(size_of(v)?),
        "h" => st.h = Some(size_of(v)?),
        "min_w" => st.min_w = Some(num_of(v)?),
        "max_w" => st.max_w = Some(num_of(v)?),
        "min_h" => st.min_h = Some(num_of(v)?),
        "max_h" => st.max_h = Some(num_of(v)?),
        "grow" => st.grow = Some(num_of(v)?),
        "pad" => st.pad = Some(num_or(v, crate::theme::space)?),
        "px" => st.px = Some(num_or(v, crate::theme::space)?),
        "py" => st.py = Some(num_or(v, crate::theme::space)?),
        "gap" => st.gap = Some(num_or(v, crate::theme::space)?),
        "fill" => st.fill = Some(color_of(v)?),
        "border" => st.border = Some(color_of(v)?),
        "hover" => st.hover = Some(color_of(v)?),
        "radius" => st.radius = Some(num_or(v, crate::theme::radius)?),
        "opacity" => st.opacity = Some(num_of(v)?.clamp(0.0, 1.0)),
        "align" => {
            st.align = Some(match one_of(v, &["start", "center", "end", "stretch"])?.as_str() {
                "start" => Align::Start,
                "center" => Align::Center,
                "end" => Align::End,
                _ => Align::Stretch,
            })
        }
        "justify" => {
            st.justify = Some(match one_of(v, &["start", "center", "end", "between"])?.as_str() {
                "start" => Justify::Start,
                "center" => Justify::Center,
                "end" => Justify::End,
                _ => Justify::Between,
            })
        }
        "color" => st.color = Some(color_of(v)?),
        "size" => st.size = Some(num_or(v, crate::theme::text_size)?),
        "weight" => {
            st.weight = Some(match v {
                Value::String(_) => match one_of(v, &["normal", "medium", "semibold", "bold"])?.as_str() {
                    "normal" => 400.0,
                    "medium" => 500.0,
                    "semibold" => 600.0,
                    _ => 700.0,
                },
                _ => num_of(v)?,
            })
        }
        "mono" => st.mono = bool_of(v)?,
        "wrap" => st.wrap = bool_of(v)?,
        "truncate" => st.truncate = bool_of(v)?,
        // true: the global switch; a sandbox's name: its switch.
        "pills" => {
            st.pills = match v {
                Value::String(s) => Some(s.to_string_lossy()),
                Value::Boolean(true) => Some(String::new()),
                Value::Boolean(false) | Value::Nil => None,
                _ => return Err(format!("expected true or a sandbox name, got {}", v.type_name())),
            }
        }
        "on" => n.on = Some(string_of(v)?),
        "on_submit" => n.on_submit = Some(string_of(v)?),
        "arg" => n.arg = lua_to_json(v, 0)?,
        "id" => n.id = Some(string_of(v)?),
        "value" => {
            n.value = lua_to_json(v, 0)?;
            match (n.kind, &n.value) {
                (Kind::Input | Kind::Textarea | Kind::Select, J::String(_) | J::Null) => {}
                (Kind::Input | Kind::Textarea | Kind::Select, J::Number(x)) => n.value = J::String(x.to_string()),
                (Kind::Checkbox, J::Bool(_) | J::Null) => {}
                (Kind::Progress, J::Number(_)) => {}
                _ => return Err("wrong type for this element (see docs/screens.md)".into()),
            }
        }
        "ratio" => n.value = J::from(num_of(v)?.clamp(0.1, 0.9) as f64),
        "placeholder" => n.placeholder = Some(string_of(v)?),
        "scope" => n.scope = lua_to_json(v, 0)?,
        "switch" => n.switch = bool_of(v)?,
        "variant" => n.variant = Some(one_of(v, VARIANTS)?),
        "tone" if matches!(n.kind, Kind::Button | Kind::Menu) => n.tone = Some(one_of(v, TONES)?),
        "tone" if matches!(n.kind, Kind::Input | Kind::Textarea) => n.tone = Some(one_of(v, FIELD_TONES)?),
        "tone" => n.variant = Some(one_of(v, TONES)?),
        "compact" => n.compact = bool_of(v)?,
        "pill" => n.pill = bool_of(v)?,
        "folds" => n.folds = bool_of(v)?,
        "shortcuts" => {
            let Value::Table(t) = v else { return Err("expected a list of { \"what\", \"key\", ... }".into()) };
            for line in t.sequence_values::<Value>() {
                let Value::Table(l) = line.map_err(|e| e.to_string())? else { return Err("each shortcut is { \"what\", \"key\", ... }".into()) };
                let mut parts = vec![];
                for x in l.sequence_values::<Value>() {
                    parts.push(string_of(&x.map_err(|e| e.to_string())?)?);
                }
                if parts.len() < 2 {
                    return Err("each shortcut is { \"what\", \"key\", ... }".into());
                }
                let what = parts.remove(0);
                n.shortcuts.push((what, parts));
            }
        }
        "paint" => n.paint = Some(paint_of(v)?),
        "icon" => n.icon = Some(string_of(v)?),
        "disabled" => n.disabled = bool_of(v)?,
        "secure" => n.secure = bool_of(v)?,
        "vertical" => n.vertical = bool_of(v)?,
        "stick_bottom" => n.stick_bottom = bool_of(v)?,
        "fade" => n.fade = Some(num_of(v)?.clamp(0.0, 64.0)),
        "rows" => n.rows = num_of(v)?.clamp(1.0, 40.0) as u32,
        "fit" => n.fill_image = one_of(v, &["fit", "fill"])? == "fill",
        "pin" => {
            let Value::Table(t) = v else { return Err("expected { left = n, top = n, right = n, bottom = n } (any of them)".into()) };
            let mut e = [None; 4];
            for pair in t.pairs::<String, Value>() {
                let (k, x) = pair.map_err(|_| "pin: keys must be left, top, right or bottom".to_string())?;
                let i = ["left", "top", "right", "bottom"].iter().position(|n| *n == k).ok_or_else(|| format!("pin: unknown edge '{k}' (left, top, right, bottom)"))?;
                e[i] = Some(num_or(&x, crate::theme::space)?);
            }
            st.pin = Some(e);
        }
        "hover_group" => n.hover_group = bool_of(v)?,
        "hover_zone" => n.hover_zone = Some(string_of(v)?).filter(|z| !z.is_empty() && z.len() <= 200),
        "reveal" => n.reveal = bool_of(v)?,
        "group" => n.group = Some(one_of(v, &["list"])?),
        "selected" => n.selected = bool_of(v)?,
        "on_space" => n.on_space = Some(string_of(v)?),
        "on_left" => n.on_left = Some(string_of(v)?),
        "on_right" => n.on_right = Some(string_of(v)?),
        "dense" => n.dense = bool_of(v)?,
        "clip" => n.clip = bool_of(v)?,
        "tab_stop" => n.skip_tab = !bool_of(v)?,
        "keys" => {
            let Value::Table(t) = v else { return Err("expected a list of key names, { \"⌘\", \"K\" }".into()) };
            for k in t.sequence_values::<Value>() {
                n.keys.push(string_of(&k.map_err(|e| e.to_string())?)?);
            }
        }
        "items" => {
            let Value::Table(t) = v else { return Err("expected a list of { id = ..., label = ... }".into()) };
            for (i, x) in t.sequence_values::<Value>().enumerate() {
                let x = x.map_err(|e| e.to_string())?;
                let item = match &x {
                    Value::String(s) => Item::plain(&s.to_string_lossy(), &s.to_string_lossy(), None),
                    Value::Table(it) => {
                        let get = |k: &str| -> Result<Option<String>, String> { it.get::<Option<String>>(k).map_err(|_| format!("item {}: '{k}' must be a string", i + 1)) };
                        for pair in it.pairs::<String, Value>() {
                            let (k, _) = pair.map_err(|_| format!("item {}: keys must be names", i + 1))?;
                            if !["id", "label", "icon", "disabled", "help", "tone", "keys", "divider"].contains(&k.as_str()) {
                                return Err(format!("item {}: unknown key '{k}' (id, label, icon, disabled, help, tone, keys, divider)", i + 1));
                            }
                        }
                        if it.get::<Option<bool>>("divider").ok().flatten().unwrap_or(false) {
                            n.items.push(Item { divider: true, ..Item::plain(&format!("divider-{}", i + 1), "", None) });
                            continue;
                        }
                        let id = get("id")?.ok_or_else(|| format!("item {} has no id", i + 1))?;
                        let tone = get("tone")?;
                        if let Some(t) = &tone {
                            if !TONES.contains(&t.as_str()) {
                                return Err(format!("item {}: unknown tone '{t}' ({})", i + 1, TONES.join(", ")));
                            }
                        }
                        let mut keys = vec![];
                        if let Some(Value::Table(kt)) = it.get::<Option<Value>>("keys").ok().flatten() {
                            for k in kt.sequence_values::<String>() {
                                keys.push(k.map_err(|_| format!("item {}: keys must be strings", i + 1))?);
                            }
                        }
                        let icon = get("icon")?;
                        if let Some(ic) = &icon {
                            if !crate::icons::exists(ic) {
                                return Err(format!("item {}: unknown icon '{ic}'", i + 1));
                            }
                        }
                        Item { label: get("label")?.unwrap_or_else(|| id.clone()), id, icon, disabled: it.get::<Option<bool>>("disabled").ok().flatten().unwrap_or(false), help: get("help")?, tone, keys, divider: false }
                    }
                    _ => return Err(format!("item {} must be a string or a table", i + 1)),
                };
                n.items.push(item);
            }
        }
        _ => unreachable!("{name}.{k} is listed but not handled"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: &str = r#"
        return {
          init = function() return { clicks = 0 } end,
          render = function(state, data)
            return column{ text{ "clicks " .. state.clicks }, text(data.title) }
          end,
          actions = { click = function(state) state.clicks = state.clicks + 1 end },
        }
    "#;

    fn host() -> LuauHost {
        LuauHost::new(Limits::default()).unwrap()
    }

    fn render(src: &str) -> Result<Node, String> {
        let mut h = host();
        h.load("s", &format!("return {{ render = function(state, data) return {src} end }}"))?;
        h.render(&J::Null)
    }

    #[test]
    fn data_is_read_lazily_and_what_was_read_is_known() {
        let mut h = host();
        h.load("s", r#"return { render = function(state, data)
            local n = 0
            if data.flag then for _ in pairs(data.app.list or {}) do n = n + 1 end end
            return column{ text(data.title .. " " .. tostring(data.app.pages.current) .. " " .. n) }
          end }"#).unwrap();
        let data = serde_json::json!({ "title": "t", "flag": false, "big": { "x": 1 }, "app": { "pages": { "current": 2 }, "list": [1, 2], "hover": "a" } });
        let (n, reads) = h.render_tracked(&data).unwrap();
        assert_eq!(n.children[0].text, "t 2 0");
        let paths: Vec<String> = reads.0.iter().map(|p| p.join(".")).collect();
        assert_eq!(paths, ["app.?", "app.pages", "flag", "title"], "big and app.list never read, so never converted");
        // A change only where it did not read leaves the view as it is.
        let mut other = data.clone();
        other["big"]["x"] = 2.into();
        other["app"]["hover"] = "b".into();
        assert!(!reads.changed(&data, &other));
        let mut flag = data.clone();
        flag["flag"] = true.into();
        assert!(reads.changed(&data, &flag));
        // Iterating a level reads all of it.
        let (n, reads) = h.render_tracked(&flag).unwrap();
        assert_eq!(n.children[0].text, "t 2 2");
        assert!(reads.0.contains(&vec!["app".to_string(), "list".to_string()]));
        let mut list = flag.clone();
        list["app"]["list"] = serde_json::json!([1]);
        assert!(reads.changed(&flag, &list));
        h.load("s", r#"return { render = function(state, data)
            local keys = {}
            for k in pairs(data) do keys[#keys + 1] = k end
            table.sort(keys)
            return column{ text(table.concat(keys, ",")) }
          end }"#).unwrap();
        let (n, reads) = h.render_tracked(&data).unwrap();
        assert_eq!(n.children[0].text, "app,big,flag,title");
        assert!(reads.changed(&data, &other), "pairs over the top reads everything");
    }

    #[test]
    fn renders_and_keeps_state_across_reload() {
        let mut h = host();
        h.load("s", SCREEN).unwrap();
        h.act("click", &J::Null, &J::Null).unwrap();
        h.act("click", &J::Null, &J::Null).unwrap();
        let data = serde_json::json!({ "title": "t" });
        let edited = SCREEN.replace("\"clicks \"", "\"count: \"");
        h.load("s", &edited).unwrap();
        let n = h.render(&data).unwrap();
        assert_eq!(n.kind, Kind::Column);
        assert_eq!(n.children[0].text, "count: 2");
    }

    #[test]
    fn a_broken_file_keeps_the_last_good_version() {
        let mut h = host();
        h.load("s", SCREEN).unwrap();
        let e = h.load("s", "return { render = ").unwrap_err();
        assert!(e.contains("s:"), "{e}");
        assert!(h.render(&serde_json::json!({ "title": "t" })).is_ok());
    }

    #[test]
    fn an_endless_loop_is_stopped() {
        let h = host();
        let t = Instant::now();
        let e = h.eval("while true do end").unwrap_err();
        eprintln!("while true do end\n  -> {} (after {:?})", e.lines().next().unwrap_or(""), t.elapsed());
        assert!(e.contains("instruction budget exceeded"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(1));
        // The VM is still usable after the stop.
        assert!(h.eval("return 1 + 1").is_ok());
    }

    #[test]
    fn an_endless_render_is_stopped_and_the_host_survives() {
        let mut h = host();
        h.load("s", "return { render = function() while true do end end }").unwrap();
        assert!(h.render(&J::Null).unwrap_err().contains("budget"));
    }

    #[test]
    fn an_endless_action_is_stopped() {
        let mut h = host();
        h.load("s", "return { render = function() return text'x' end, actions = { spin = function() while true do end end } }").unwrap();
        assert!(h.act("spin", &J::Null, &J::Null).unwrap_err().contains("budget"));
        assert!(h.render(&J::Null).is_ok());
    }

    #[test]
    fn no_file_process_or_module_access() {
        let h = host();
        for src in [
            "return io.open('/etc/passwd')",
            "return os.execute('id')",
            "return os.getenv('HOME')",
            "return require('x')",
            "return loadstring('return 1')",
            "return getfenv(1)",
            "return dofile('/etc/passwd')",
        ] {
            let e = h.eval(src).unwrap_err();
            eprintln!("{src}\n  -> {}", e.lines().next().unwrap_or(""));
            assert!(e.contains("attempt to") || e.contains("nil"), "{src}: {e}");
        }
    }

    #[test]
    fn the_theme_table_follows_a_switch() {
        // Luau resolves `theme.color.bg` once, when the chunk loads (an
        // import of a sandboxed global): the table is refilled on a switch,
        // and the screen is loaded again (screen.rs, `redraw`) to read it.
        let src = r#"return { render = function() return text{ theme.color.bg } end }"#;
        let mut h = host();
        h.load("t.luau", src).unwrap();
        assert_eq!(h.render(&J::Null).unwrap().text, "#0d1117");
        crate::theme::with_mode(crate::theme::Mode::Graphite, || {
            h.sync_theme();
            assert_eq!(h.eval("return theme.color.bg").unwrap().to_string().unwrap(), "#202124", "the table holds the new palette");
            assert_eq!(h.eval("return theme.mode").unwrap().to_string().unwrap(), "graphite");
            h.load("t.luau", src).unwrap();
            assert_eq!(h.render(&J::Null).unwrap().text, "#202124", "loaded again, it reads it");
        });
    }

    #[test]
    fn globals_and_theme_are_read_only() {
        let h = host();
        assert!(h.eval("string.rep = nil").is_err());
        assert!(h.eval("theme.color.text = '#ff0000'").is_err());
        assert_eq!(h.eval("return theme.color['text-muted']").unwrap().to_string().unwrap(), "#9198a1");
    }

    #[test]
    fn memory_is_capped() {
        let h = LuauHost::new(Limits { memory_bytes: 4 << 20, ..Limits::default() }).unwrap();
        let e = h.eval("return string.rep('x', 8 * 1024 * 1024)").unwrap_err();
        eprintln!("string.rep 8 MB under a 4 MB cap\n  -> {}", e.lines().next().unwrap_or(""));
        assert!(e.to_lowercase().contains("memory"), "{e}");
    }

    #[test]
    fn too_many_elements_are_refused() {
        let e = render("(function() local t = {} for i = 1, 6000 do t[i] = text'x' end return column(t) end)()").unwrap_err();
        assert!(e.contains("more than 5000"), "{e}");
    }

    #[test]
    fn props_are_checked() {
        let n = render(r#"row{ gap = "sm", fill = "surface", radius = "md", w = "full", key = "r",
            text{ "a", color = "text-muted", size = "sm", weight = "bold", truncate = true },
            false,
            button{ "Stop", on = "stop", arg = { name = "x", n = 2 }, variant = "danger", icon = "stop", help = "Stop it" },
            input{ id = "q", value = "abc", placeholder = "Search", on = "typed", on_submit = "go" },
            select{ id = "s", items = { "a", { id = "b", label = "Bee", icon = "check" } }, value = "b", on = "pick" },
            split{ id = "sp", ratio = 0.3, vertical = true, text"l", text"r" },
        }"#)
        .unwrap();
        assert_eq!(n.kind, Kind::Row);
        assert_eq!(n.style.gap, Some(8.0));
        assert_eq!(n.style.radius, Some(10.0));
        assert_eq!(n.children.len(), 5, "false is skipped");
        assert_eq!(n.children[0].style.size, Some(12.0));
        assert_eq!(n.children[0].style.weight, Some(700.0));
        assert_eq!(n.children[1].arg, serde_json::json!({ "name": "x", "n": 2 }));
        assert_eq!(n.children[3].items[1], Item::plain("b", "Bee", Some("check")));
        assert_eq!(n.children[4].value, serde_json::json!(0.30000001192092896));

        for (src, want) in [
            ("text{ 'a', colour = 'red' }", "text: unknown prop 'colour'"),
            ("text{ 'a', color = 'pink' }", "unknown colour 'pink'"),
            ("frame{}", "attempt to call a nil value"),
            ("{ kind = 'frame' }", "unknown element 'frame'"),
            ("button{ 'x' }", "needs 'on'"),
            ("input{}", "needs an 'id'"),
            ("icon{ 'rocket' }", "unknown icon 'rocket'"),
            ("badge{ 'x', tone = 'pink' }", "not one of"),
            ("split{ id = 'a', text'x' }", "exactly two"),
            ("row{ gap = 'huge' }", "unknown space"),
            ("{ 1, 2 }", "not an element"),
            ("row{ 42 }", "expected an element, got integer"),
        ] {
            let e = render(src).unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
    }

    #[test]
    fn actions_return_intents_as_data() {
        let mut h = host();
        h.load("s", r#"return { render = function() return text'x' end, actions = {
            one = function(state, arg) return intent("stop_sandbox", { sandbox = arg }) end,
            many = function() return { intent("select", { sandbox = "a" }), intent("set_layout", { panes = 2 }) } end,
            none = function() end,
            bad = function() return print end,
        } }"#)
            .unwrap();
        let one = h.act("one", &J::from("web"), &J::Null).unwrap();
        assert_eq!(crate::intent::parse(&one).unwrap(), vec![crate::intent::Intent::StopSandbox { sandbox: "web".into() }]);
        assert_eq!(crate::intent::parse(&h.act("many", &J::Null, &J::Null).unwrap()).unwrap().len(), 2);
        assert_eq!(h.act("none", &J::Null, &J::Null).unwrap(), J::Null);
        assert!(h.act("bad", &J::Null, &J::Null).unwrap_err().contains("function"));
        assert!(h.act("nope", &J::Null, &J::Null).unwrap_err().contains("no action 'nope'"));
    }

    /// Each element's example in docs/screens.md renders as written.
    #[test]
    fn every_example_in_the_doc_renders() {
        let doc = include_str!("../docs/screens.md");
        let mut n = 0;
        for section in doc.split("\n### `").skip(1) {
            let name = section.split('`').next().unwrap();
            let Some(block) = section.split("```lua\n").nth(1).and_then(|b| b.split("\n```").next()) else { panic!("{name}: no example") };
            let code: Vec<&str> = block.lines().filter(|l| !l.trim_start().starts_with("--")).collect();
            let src = format!(
                "local sb = {{ name = 'docs', status = 'running' }} local state = {{ filter = '', ro = false }} local step, steps = 1, 2\nreturn {{ render = function() return {} end }}",
                code.join("\n")
            );
            let mut h = host();
            h.load("doc", &src).unwrap_or_else(|e| panic!("{name}: {e}"));
            let node = h.render(&J::Null).unwrap_or_else(|e| panic!("{name}: {e}\n{block}"));
            assert!(SPEC.iter().any(|s| s.0 == name), "{name}");
            let _ = node;
            n += 1;
        }
        assert_eq!(n, SPEC.len(), "one example per element");
    }

    /// `paint`: strokes and dots parsed with theme colours; bad shapes and
    /// too much drawing are errors that name the prop.
    #[test]
    fn paint_draws_strokes_and_dots_within_its_caps() {
        let n = render(r#"column{ w = 100, h = 40, paint = { strokes = { { points = { 0, 1, 50, 1, 50, 30 }, width = 2, color = "blue" } }, dots = { { x = 4, y = 4, r = 3, color = "green" }, { x = 9, y = 9, hollow = true } } } }"#).unwrap();
        let p = n.paint.expect("paint");
        assert_eq!(p.strokes[0].0, vec![[0.0, 1.0], [50.0, 1.0], [50.0, 30.0]]);
        assert_eq!(p.strokes[0].1, 2.0);
        assert_eq!(p.dots.len(), 2);
        assert!(p.dots[1].3, "hollow");
        assert!(render("column{ paint = { strokes = { { points = { 1, 2, 3 } } } } }").unwrap_err().contains("x, y pairs"));
        assert!(render("column{ paint = { lines = {} } }").unwrap_err().contains("unknown key 'lines'"));
        assert!(render("text{ 'a', paint = {} }").unwrap_err().contains("unknown prop 'paint'"));
        let many = "local d = {} for i = 1, 700 do d[i] = { x = i, y = 1 } end return { render = function() return column{ paint = { dots = d } } end }";
        let mut h = LuauHost::new(Limits::default()).unwrap();
        h.load("x", many).unwrap();
        assert!(h.render(&serde_json::json!({})).unwrap_err().to_string().contains("at most"));
    }

    #[test]
    fn markdown_is_an_element_with_text_only() {
        let n = render(r#"markdown{ "**run `ls`**", color = "text", size = 13, on = "link", key = "m" }"#).unwrap();
        assert_eq!((n.kind, n.text.as_str(), n.on.as_deref()), (Kind::Markdown, "**run `ls`**", Some("link")));
        assert!(render("markdown{ text'x' }").unwrap_err().contains("takes one string"));
        assert!(render("markdown{ 'x', wrap = true }").unwrap_err().contains("unknown prop 'wrap'"));
    }

    #[test]
    fn every_element_and_prop_is_documented() {
        let doc = include_str!("../docs/screens.md");
        for (name, _, _, props) in SPEC {
            assert!(doc.contains(&format!("### `{name}`")), "docs/screens.md has no section for {name}");
            for p in *props {
                assert!(doc.contains(&format!("`{p}`")), "docs/screens.md never names the prop {p} (of {name})");
            }
        }
        for p in COMMON {
            assert!(doc.contains(&format!("`{p}`")), "docs/screens.md never names the common prop {p}");
        }
    }
}
