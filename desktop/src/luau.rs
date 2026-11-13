//! The Luau host: runs one screen file in a locked-down VM and turns what it
//! returns into a [`Node`] tree the core knows how to draw.
//!
//! The VM gets no file, network or process access: only the `string`,
//! `table`, `math`, `utf8` and `bit32` libraries are opened, `require`,
//! `loadstring`, `getfenv` and `setfenv` are removed, then mlua's Luau
//! sandbox mode makes the globals read-only. Every call into the script runs
//! under a memory cap and an instruction budget enforced from Luau's
//! interrupt hook (fired on calls and loop back-edges).
//!
//! A screen file returns a table:
//!
//! ```lua
//! return {
//!   init = function() return { selected = nil } end,       -- first load only
//!   render = function(state, data) return column{ ... } end,
//!   actions = { select = function(state, arg) state.selected = arg end },
//! }
//! ```
//!
//! `state` lives in the VM across reloads: a new version of the file gets the
//! same table, so editing the file keeps what the person was doing.

use mlua::{Function, Lua, LuaOptions, RegistryKey, StdLib, Table, Value, VmState};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The fixed set of elements a screen may use.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text { text: String, color: Option<u32>, bold: bool },
    Row(Vec<Node>),
    Column(Vec<Node>),
    Button { label: String, action: String, arg: Option<String> },
    List(Vec<Node>),
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    /// Interrupt callbacks allowed per call into the script.
    pub interrupts: u64,
    /// Wall clock allowed per call into the script.
    pub wall: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self { memory_bytes: 16 << 20, interrupts: 200_000, wall: Duration::from_millis(50) }
    }
}

/// Element constructors, defined in Luau so a screen reads like markup.
const PRELUDE: &str = r#"
local function kids(t) local k = {} for i, v in ipairs(t) do k[i] = v end return k end
text = function(t) if type(t) == "string" then t = { t } end return { kind = "text", text = t[1], color = t.color, bold = t.bold } end
row = function(t) return { kind = "row", children = kids(t) } end
column = function(t) return { kind = "column", children = kids(t) } end
list = function(t) return { kind = "list", children = kids(t) } end
button = function(t) return { kind = "button", label = t[1] or t.label, on = t.on, arg = t.arg } end
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
    pub fn render(&self, data: &serde_json::Value) -> Result<Node, String> {
        let (module, state) = self.parts()?;
        let tree: Value = self.guarded(|lua| {
            let render: Function = module.get("render")?;
            let data = json_to_lua(lua, data)?;
            render.call((state, data))
        })?;
        to_node(&tree, 0)
    }

    /// Run a button's action against the state.
    pub fn act(&self, action: &str, arg: Option<&str>) -> Result<(), String> {
        let (module, state) = self.parts()?;
        self.guarded(|_| {
            let actions: Option<Table> = module.get("actions")?;
            let f: Option<Function> = actions.map(|a| a.get(action)).transpose()?.flatten();
            match f {
                Some(f) => f.call::<()>((state, arg)),
                None => Err(mlua::Error::runtime(format!("no action '{action}'"))),
            }
        })
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

fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    use serde_json::Value as J;
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

const MAX_DEPTH: usize = 32;

fn to_node(v: &Value, depth: usize) -> Result<Node, String> {
    if depth > MAX_DEPTH {
        return Err("element tree too deep".into());
    }
    let t = match v {
        Value::Table(t) => t,
        Value::String(s) => return Ok(Node::Text { text: s.to_string_lossy(), color: None, bold: false }),
        other => return Err(format!("expected an element, got {}", other.type_name())),
    };
    let s = |k: &str| -> Option<String> { t.get::<Option<String>>(k).ok().flatten() };
    let kids = || -> Result<Vec<Node>, String> {
        let c: Table = t.get("children").map_err(|e| e.to_string())?;
        c.sequence_values::<Value>().map(|x| to_node(&x.map_err(|e| e.to_string())?, depth + 1)).collect()
    };
    match s("kind").as_deref() {
        Some("text") => Ok(Node::Text {
            text: s("text").unwrap_or_default(),
            color: s("color").and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok()),
            bold: t.get::<Option<bool>>("bold").ok().flatten().unwrap_or(false),
        }),
        Some("row") => Ok(Node::Row(kids()?)),
        Some("column") => Ok(Node::Column(kids()?)),
        Some("list") => Ok(Node::List(kids()?)),
        Some("button") => Ok(Node::Button {
            label: s("label").unwrap_or_default(),
            action: s("on").ok_or("button without 'on'")?,
            arg: s("arg"),
        }),
        Some(k) => Err(format!("unknown element '{k}'")),
        None => Err("element without a kind".into()),
    }
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

    #[test]
    fn renders_and_keeps_state_across_reload() {
        let mut h = host();
        h.load("s", SCREEN).unwrap();
        h.act("click", None).unwrap();
        h.act("click", None).unwrap();
        let data = serde_json::json!({ "title": "t" });
        let edited = SCREEN.replace("\"clicks \"", "\"count: \"");
        h.load("s", &edited).unwrap();
        let Node::Column(k) = h.render(&data).unwrap() else { panic!() };
        assert_eq!(k[0], Node::Text { text: "count: 2".into(), color: None, bold: false });
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
        assert!(h.render(&serde_json::Value::Null).unwrap_err().contains("budget"));
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
    fn globals_are_read_only() {
        let h = host();
        assert!(h.eval("string.rep = nil").is_err());
    }

    #[test]
    fn memory_is_capped() {
        let h = LuauHost::new(Limits { memory_bytes: 4 << 20, ..Limits::default() }).unwrap();
        let e = h.eval("return string.rep('x', 8 * 1024 * 1024)").unwrap_err();
        eprintln!("string.rep 8 MB under a 4 MB cap\n  -> {}", e.lines().next().unwrap_or(""));
        assert!(e.to_lowercase().contains("memory"), "{e}");
    }
}
