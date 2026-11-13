//! Intents: the only way a screen asks the core to do something. A screen's
//! action returns them as data (`intent("stop_sandbox", { sandbox = "x" })`);
//! the core checks each one against this closed list, with exact argument
//! names and types, and only then acts. A screen never reaches the daemon.
//!
//! An unknown intent, an unknown argument, a missing or mistyped one: the
//! whole batch the action returned is refused, nothing runs, and the error is
//! shown on the screen.
//!
//! The list is one table, [`REGISTRY`]: each intent's name, arguments with
//! their types, and its [`Risk`]. Adding an intent (waves 2 to 4) is a row
//! there, a variant in [`Intent`] built in [`parse_one`], the code that runs
//! it in `shell.rs` (`App::run_intent`), and its entry in `docs/screens.md`.
//! Screens written against the existing rows do not change.
//!
//! **Approvals.** An intent whose risk is [`Risk::Approval`] is not run when
//! a screen asks: the core queues it (`App::approvals`) and draws the dialog
//! itself (`approval.rs`), above everything, out of reach of Luau; only the
//! person's answer there runs it. No wave-1 intent has that risk.

use serde_json::{Map, Value};

/// Where an opened terminal goes.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneTarget {
    /// The focused pane (the web UI's row click).
    Focused,
    /// A pane showing nothing, or a new pane if all are used and the layout has room.
    Spare,
    /// That pane, counted from 1.
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Opens a terminal on a sandbox. `mode`: `agent` (default; starts a stopped
    /// sandbox), `bash`. `thread` for a thread's session.
    OpenTerminal { sandbox: String, mode: Mode, thread: Option<String>, pane: PaneTarget },
    /// Starts a stopped sandbox: as the web UI does, by opening its agent terminal.
    StartSandbox { sandbox: String, pane: PaneTarget },
    StopSandbox { sandbox: String },
    /// Marks a sandbox as the selected one (the sidebar's highlight).
    Select { sandbox: Option<String> },
    /// Pane count (1 to 4) and how two panes split.
    SetLayout { panes: usize, split: Split },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Agent,
    Bash,
}

impl Mode {
    /// The `/ws` query's `mode`.
    pub fn query(self) -> Option<String> {
        match self {
            Mode::Agent => None,
            Mode::Bash => Some("bash".into()),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Agent => "agent",
            Mode::Bash => "bash",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Split {
    /// Side by side.
    Columns,
    /// One above the other.
    Rows,
}

/// How much an intent can do, declared once per intent in [`REGISTRY`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Risk {
    /// What the web UI does on one click: runs when the screen asks.
    Direct,
    /// Held for a dialog drawn by the core; the person says yes or no.
    /// The text says what is asked (wave 4: Jarvis proposals, network rules,
    /// host commands).
    Approval(&'static str),
}

/// What an intent argument holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg {
    /// A non-empty string.
    Str,
    /// One of these strings.
    OneOf(&'static [&'static str]),
    /// A whole number in this range.
    Int(i64, i64),
    /// `"focused"`, `"spare"` or a pane number 1 to 4.
    Pane,
}

/// One entry of the closed list.
pub struct Spec {
    pub name: &'static str,
    pub risk: Risk,
    /// `(name, type, required)`.
    pub args: &'static [(&'static str, Arg, bool)],
    pub doc: &'static str,
}

const MODES: &[&str] = &["agent", "bash"];

/// The closed list. Widening it later is a row here plus the variant and its
/// code; screens that use the existing rows do not change.
pub const REGISTRY: &[Spec] = &[
    Spec {
        name: "open_terminal",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("mode", Arg::OneOf(MODES), false), ("thread", Arg::Str, false), ("pane", Arg::Pane, false)],
        doc: "Opens a terminal on a sandbox, in the focused pane by default. The agent mode starts a stopped sandbox.",
    },
    Spec {
        name: "start_sandbox",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("pane", Arg::Pane, false)],
        doc: "Starts a stopped sandbox by opening its agent terminal, as the web UI's Connect does.",
    },
    Spec { name: "stop_sandbox", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Stops a sandbox (`POST /api/sandboxes/:name/stop`)." },
    Spec { name: "select", risk: Risk::Direct, args: &[("sandbox", Arg::Str, false)], doc: "Marks a sandbox as selected (no `sandbox`: none)." },
    Spec {
        name: "set_layout",
        risk: Risk::Direct,
        args: &[("panes", Arg::Int(1, 4), false), ("split", Arg::OneOf(&["columns", "rows"]), false)],
        doc: "Sets how many terminal panes show (1 to 4) and whether two split side by side or stacked.",
    },
];

impl Intent {
    pub fn name(&self) -> &'static str {
        match self {
            Intent::OpenTerminal { .. } => "open_terminal",
            Intent::StartSandbox { .. } => "start_sandbox",
            Intent::StopSandbox { .. } => "stop_sandbox",
            Intent::Select { .. } => "select",
            Intent::SetLayout { .. } => "set_layout",
        }
    }

    pub fn spec(&self) -> &'static Spec {
        spec(self.name()).expect("every intent is registered")
    }

    pub fn risk(&self) -> Risk {
        self.spec().risk
    }
}

pub fn spec(name: &str) -> Option<&'static Spec> {
    REGISTRY.iter().find(|s| s.name == name)
}

/// What an action returned: `nil`, one intent, or a list of them. All or nothing.
pub fn parse(v: &Value) -> Result<Vec<Intent>, String> {
    let is_intent = |o: &Map<String, Value>| o.get("kind").and_then(Value::as_str) == Some("intent");
    match v {
        Value::Null => Ok(vec![]),
        Value::Object(o) if is_intent(o) => Ok(vec![parse_one(o)?]),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, x)| match x {
                Value::Object(o) if is_intent(o) => parse_one(o),
                _ => Err(format!("item {} of the returned list is not an intent (make it with intent(name, args))", i + 1)),
            })
            .collect(),
        // `{}` from Luau is an empty table: no intents.
        Value::Object(o) if o.is_empty() => Ok(vec![]),
        _ => Err("an action must return nil, an intent, or a list of intents (make them with intent(name, args))".into()),
    }
}

/// Checks the arguments against the registry, then builds the variant.
fn parse_one(o: &Map<String, Value>) -> Result<Intent, String> {
    let name = o.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return Err("an intent without a name".into());
    }
    let spec = spec(name).ok_or_else(|| format!("unknown intent '{name}' (known: {})", REGISTRY.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")))?;
    let empty = Map::new();
    let m = match o.get("args") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        // An empty Luau table converts to `[]`.
        Some(Value::Array(a)) if a.is_empty() => &empty,
        Some(_) => return Err(format!("{name}: arguments must be a table of named values")),
    };
    for k in m.keys() {
        if !spec.args.iter().any(|(a, _, _)| a == k) {
            let takes: Vec<_> = spec.args.iter().map(|(a, _, _)| *a).collect();
            return Err(format!("{name}: unknown argument '{k}' (takes: {})", takes.join(", ")));
        }
    }
    for (a, ty, required) in spec.args {
        match m.get(*a) {
            None | Some(Value::Null) if *required => return Err(format!("{name}: '{a}' is required")),
            None | Some(Value::Null) => {}
            Some(v) => check(name, a, *ty, v)?,
        }
    }
    let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
    let pane = || match m.get("pane") {
        Some(Value::String(p)) if p == "spare" => PaneTarget::Spare,
        Some(Value::Number(n)) => PaneTarget::Index(n.as_u64().unwrap_or(1) as usize),
        _ => PaneTarget::Focused,
    };
    Ok(match name {
        "open_terminal" => Intent::OpenTerminal {
            sandbox: s("sandbox").unwrap(),
            mode: if s("mode").as_deref() == Some("bash") { Mode::Bash } else { Mode::Agent },
            thread: s("thread"),
            pane: pane(),
        },
        "start_sandbox" => Intent::StartSandbox { sandbox: s("sandbox").unwrap(), pane: pane() },
        "stop_sandbox" => Intent::StopSandbox { sandbox: s("sandbox").unwrap() },
        "select" => Intent::Select { sandbox: s("sandbox") },
        "set_layout" => Intent::SetLayout {
            panes: m.get("panes").and_then(Value::as_u64).unwrap_or(1) as usize,
            split: if s("split").as_deref() == Some("rows") { Split::Rows } else { Split::Columns },
        },
        _ => unreachable!("{name} is registered but not built"),
    })
}

fn check(name: &str, a: &str, ty: Arg, v: &Value) -> Result<(), String> {
    let ok = match (ty, v) {
        (Arg::Str, Value::String(s)) => !s.is_empty(),
        (Arg::OneOf(set), Value::String(s)) => set.contains(&s.as_str()),
        (Arg::Int(lo, hi), Value::Number(n)) => n.as_f64().is_some_and(|f| f.fract() == 0.0 && f >= lo as f64 && f <= hi as f64),
        (Arg::Pane, Value::String(s)) => s == "focused" || s == "spare",
        (Arg::Pane, Value::Number(n)) => n.as_f64().is_some_and(|f| f.fract() == 0.0 && (1.0..=4.0).contains(&f)),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(match ty {
        Arg::Str => format!("{name}: '{a}' must be a non-empty string"),
        Arg::OneOf(set) => format!("{name}: '{a}' must be one of {}", set.join(", ")),
        Arg::Int(lo, hi) => format!("{name}: '{a}' must be a whole number from {lo} to {hi}"),
        Arg::Pane => format!("{name}: '{a}' must be \"focused\", \"spare\" or a pane number 1 to 4"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn i(name: &str, args: Value) -> Value {
        json!({ "kind": "intent", "name": name, "args": args })
    }

    #[test]
    fn known_intents_parse() {
        assert_eq!(parse(&i("stop_sandbox", json!({ "sandbox": "a" }))).unwrap(), vec![Intent::StopSandbox { sandbox: "a".into() }]);
        assert_eq!(
            parse(&json!([i("select", json!({ "sandbox": "a" })), i("open_terminal", json!({ "sandbox": "a", "mode": "bash", "pane": 2 }))])).unwrap(),
            vec![
                Intent::Select { sandbox: Some("a".into()) },
                Intent::OpenTerminal { sandbox: "a".into(), mode: Mode::Bash, thread: None, pane: PaneTarget::Index(2) }
            ]
        );
        assert_eq!(parse(&i("set_layout", json!({ "panes": 2, "split": "rows" }))).unwrap(), vec![Intent::SetLayout { panes: 2, split: Split::Rows }]);
        for spec in REGISTRY {
            assert_eq!(spec.risk, Risk::Direct, "no wave-1 intent needs an approval");
        }
        assert_eq!(parse(&i("select", json!([]))).unwrap(), vec![Intent::Select { sandbox: None }]);
        assert_eq!(parse(&Value::Null).unwrap(), vec![]);
    }

    #[test]
    fn anything_else_is_refused_whole() {
        for (v, want) in [
            (i("rm_sandbox", json!({ "sandbox": "a" })), "unknown intent 'rm_sandbox'"),
            (i("stop_sandbox", json!({})), "'sandbox' is required"),
            (i("stop_sandbox", json!({ "sandbox": 3 })), "must be a non-empty string"),
            (i("stop_sandbox", json!({ "sandbox": "a", "force": true })), "unknown argument 'force'"),
            (i("set_layout", json!({ "panes": 9 })), "from 1 to 4"),
            (i("open_terminal", json!({ "sandbox": "a", "mode": "root" })), "must be one of agent, bash"),
            (json!([i("select", json!({})), { "name": "stop_sandbox" }]), "item 2"),
            (json!("stop everything"), "must return nil"),
        ] {
            let e = parse(&v).unwrap_err();
            assert!(e.contains(want), "{v}: {e}");
        }
    }

    #[test]
    fn every_name_is_documented() {
        let doc = include_str!("../docs/screens.md");
        for n in REGISTRY.iter().map(|s| s.name) {
            assert!(doc.contains(&format!("`{n}`")), "docs/screens.md does not describe the intent {n}");
        }
    }
}
