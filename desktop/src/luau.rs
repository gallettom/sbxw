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
use std::cell::Cell;
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
}

/// One entry of a `menu` or `select`.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub disabled: bool,
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
    /// `button`: neutral, primary, danger, ghost. `badge`/`dot`/`progress`: a tone.
    pub variant: Option<String>,
    pub icon: Option<String>,
    pub disabled: bool,
    pub secure: bool,
    /// `split` and `divider`: true for side by side (a vertical divider).
    pub vertical: bool,
    pub stick_bottom: bool,
    pub style: Style,
    pub children: Vec<Node>,
}

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
            variant: None,
            icon: None,
            disabled: false,
            secure: false,
            vertical: false,
            stick_bottom: false,
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
pub const COMMON: &[&str] = &["key", "help", "w", "h", "min_w", "max_w", "min_h", "max_h", "grow", "pad", "px", "py", "fill", "border", "radius", "opacity"];
const TEXT_PROPS: &[&str] = &["color", "size", "weight", "mono", "wrap", "truncate"];
const BOX_PROPS: &[&str] = &["gap", "align", "justify", "hover", "on", "arg"];

/// The frozen element set: name, kind, positional entries, props beyond [`COMMON`].
pub const SPEC: &[(&str, Kind, Positional, &[&str])] = &[
    ("text", Kind::Text, Positional::Text, TEXT_PROPS),
    ("row", Kind::Row, Positional::Children, BOX_PROPS),
    ("column", Kind::Column, Positional::Children, BOX_PROPS),
    ("list", Kind::List, Positional::Children, &["gap"]),
    ("scroll", Kind::Scroll, Positional::Children, &["gap", "stick_bottom"]),
    ("button", Kind::Button, Positional::Text, &["on", "arg", "variant", "icon", "disabled", "color"]),
    ("input", Kind::Input, Positional::None, &["id", "value", "placeholder", "on", "on_submit", "arg", "secure", "disabled"]),
    ("icon", Kind::Icon, Positional::Text, &["color", "size"]),
    ("badge", Kind::Badge, Positional::Text, &["tone"]),
    ("dot", Kind::Dot, Positional::None, &["tone"]),
    ("split", Kind::Split, Positional::Two, &["id", "vertical", "ratio"]),
    ("spacer", Kind::Spacer, Positional::None, &[]),
    ("divider", Kind::Divider, Positional::None, &["vertical", "color"]),
    ("menu", Kind::Menu, Positional::Text, &["id", "items", "on", "arg", "icon", "variant", "disabled"]),
    ("select", Kind::Select, Positional::None, &["id", "items", "value", "on", "arg", "placeholder", "disabled"]),
    ("checkbox", Kind::Checkbox, Positional::Text, &["value", "on", "arg", "disabled"]),
    ("progress", Kind::Progress, Positional::None, &["value", "tone"]),
];

/// Tones of `badge`, `dot` and `progress`.
pub const TONES: &[&str] = &["neutral", "blue", "green", "amber", "red", "purple"];
/// Variants of `button` and `menu`.
pub const VARIANTS: &[&str] = &["neutral", "primary", "danger", "ghost"];

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    /// Interrupt callbacks allowed per call into the script.
    pub interrupts: u64,
    /// Wall clock allowed per call into the script.
    pub wall: Duration,
    /// Elements one render may return.
    pub nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { memory_bytes: 16 << 20, interrupts: 200_000, wall: Duration::from_millis(50), nodes: 5_000 }
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
                     "dot", "split", "spacer", "divider", "menu", "select", "checkbox", "progress" }) do
  _G[k] = mk(k)
end
intent = function(name, args)
  if type(name) ~= "string" then error("intent(name, args): name must be a string", 2) end
  return { kind = "intent", name = name, args = args or {} }
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
}

struct Budget {
    count: Cell<u64>,
    deadline: Cell<Option<Instant>>,
}

impl LuauHost {
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
            if n > max || late {
                return Err(mlua::Error::runtime(format!("instruction budget exceeded ({n} interrupts)")));
            }
            Ok(VmState::Continue)
        });
        Ok(Self { lua, limits, budget, module: None, state: None })
    }

    fn arm(&self) {
        self.budget.count.set(0);
        self.budget.deadline.set(Some(Instant::now() + self.limits.wall));
    }

    fn disarm(&self) {
        self.budget.deadline.set(None);
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
        let (module, state) = self.parts()?;
        let tree: Value = self.guarded(|lua| {
            let render: Function = module.get("render")?;
            let data = json_to_lua(lua, data)?;
            render.call((state, data))
        })?;
        let mut count = 0;
        to_node(&tree, 0, &mut count, self.limits.nodes)
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
    let needs_id = matches!(n.kind, Kind::Input | Kind::Split | Kind::Menu | Kind::Select);
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
        "on" => n.on = Some(string_of(v)?),
        "on_submit" => n.on_submit = Some(string_of(v)?),
        "arg" => n.arg = lua_to_json(v, 0)?,
        "id" => n.id = Some(string_of(v)?),
        "value" => {
            n.value = lua_to_json(v, 0)?;
            match (n.kind, &n.value) {
                (Kind::Input | Kind::Select, J::String(_) | J::Null) => {}
                (Kind::Input | Kind::Select, J::Number(x)) => n.value = J::String(x.to_string()),
                (Kind::Checkbox, J::Bool(_) | J::Null) => {}
                (Kind::Progress, J::Number(_)) => {}
                _ => return Err("wrong type for this element (see docs/screens.md)".into()),
            }
        }
        "ratio" => n.value = J::from(num_of(v)?.clamp(0.1, 0.9) as f64),
        "placeholder" => n.placeholder = Some(string_of(v)?),
        "variant" => n.variant = Some(one_of(v, VARIANTS)?),
        "tone" => n.variant = Some(one_of(v, TONES)?),
        "icon" => n.icon = Some(string_of(v)?),
        "disabled" => n.disabled = bool_of(v)?,
        "secure" => n.secure = bool_of(v)?,
        "vertical" => n.vertical = bool_of(v)?,
        "stick_bottom" => n.stick_bottom = bool_of(v)?,
        "items" => {
            let Value::Table(t) = v else { return Err("expected a list of { id = ..., label = ... }".into()) };
            for (i, x) in t.sequence_values::<Value>().enumerate() {
                let x = x.map_err(|e| e.to_string())?;
                let item = match &x {
                    Value::String(s) => Item { id: s.to_string_lossy(), label: s.to_string_lossy(), icon: None, disabled: false },
                    Value::Table(it) => {
                        let get = |k: &str| -> Result<Option<String>, String> { it.get::<Option<String>>(k).map_err(|_| format!("item {}: '{k}' must be a string", i + 1)) };
                        for pair in it.pairs::<String, Value>() {
                            let (k, _) = pair.map_err(|_| format!("item {}: keys must be names", i + 1))?;
                            if !["id", "label", "icon", "disabled"].contains(&k.as_str()) {
                                return Err(format!("item {}: unknown key '{k}' (id, label, icon, disabled)", i + 1));
                            }
                        }
                        let id = get("id")?.ok_or_else(|| format!("item {} has no id", i + 1))?;
                        let icon = get("icon")?;
                        if let Some(ic) = &icon {
                            if !crate::icons::exists(ic) {
                                return Err(format!("item {}: unknown icon '{ic}'", i + 1));
                            }
                        }
                        Item { label: get("label")?.unwrap_or_else(|| id.clone()), id, icon, disabled: it.get::<Option<bool>>("disabled").ok().flatten().unwrap_or(false) }
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
    fn globals_and_theme_are_read_only() {
        let h = host();
        assert!(h.eval("string.rep = nil").is_err());
        assert!(h.eval("theme.color.text = '#ff0000'").is_err());
        assert_eq!(h.eval("return theme.color['text-muted']").unwrap().to_string().unwrap(), "#8b949e");
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
        assert_eq!(n.style.gap, Some(6.0));
        assert_eq!(n.style.radius, Some(12.0));
        assert_eq!(n.children.len(), 5, "false is skipped");
        assert_eq!(n.children[0].style.size, Some(11.0));
        assert_eq!(n.children[0].style.weight, Some(700.0));
        assert_eq!(n.children[1].arg, serde_json::json!({ "name": "x", "n": 2 }));
        assert_eq!(n.children[3].items[1], Item { id: "b".into(), label: "Bee".into(), icon: Some("check".into()), disabled: false });
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
