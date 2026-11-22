//! MCP servers: the servers registered on this machine (`sbx mcp`) and the
//! sandbox they are loaded into, as the web UI's MCP dialog (`assets/js/mcp.js`).
//! It was Setup's MCP tab; it is now an overlay over the pane area
//! (`screens/mcp.luau`, as Commits and Worktrees), opened from the rail, the palette, a sandbox's
//! Manage dialog and a pane's "⋯" menu. The intents, their arguments and
//! their dialogs are the tab's, unchanged.
//!
//! **Which sandbox.** Load and Unload act on one running sandbox. Opened for
//! a sandbox (`mcp_show {sandbox}`), the panel is on it (`mcp_view.sandbox`,
//! view state dropped when the panel closes, as Network's filter); opened
//! from the rail, on the last sandbox used (kept in the app's state file).
//!
//! **Secrets.** A header's `${KEY}` is a secret: the screen names the key,
//! the value is typed in the core's dialog ([`crate::feat::Secret`]) and put
//! in the request body on Confirm. Nothing here stores, logs or returns one.
//!
//! The stand-in answers these routes in `setup/standin.rs` (one fixture
//! object for every Setup route, as before the move).

use crate::feat::setup::{list, shell_word, strings};
use crate::feat::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, ScreenDecl, Slot};
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};

/// The panel's name and its icon, used everywhere it appears.
pub const ICON: &str = "plug";

pub const FEATURE: Feature = Feature {
    name: "mcp",
    data: &[Data::new("mcp", "/api/mcp")],
    screens: &[ScreenDecl { name: "mcp", src: include_str!("../../screens/mcp.luau"), slot: Slot::Overlay { title: "MCP servers", icon: ICON } }],
    intents: INTENTS,
    act,
    extras: Some(extras),
    ..Feature::EMPTY
};

const KINDS: &[&str] = &["remote", "stdio"];

use crate::intent::direct as d;
const fn a(name: &'static str, asks: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Approval(asks), args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("mcp_show", &[("sandbox", Arg::Str, false)], "Opens the MCP servers panel and reads the servers again; on that sandbox when given (Load and Unload act on it), else on the last one used. View state only."),
    d("mcp_pick", &[("sandbox", Arg::Str, true)], "Puts the MCP servers panel on that sandbox (the one Load and Unload act on). View state only: nothing is read or sent."),
    d("mcp_load", &[("server", Arg::Str, true), ("sandbox", Arg::Str, true)], "Loads a registered MCP server into a running sandbox."),
    a("mcp_unload", "Unload an MCP server", &[("server", Arg::Str, true), ("sandbox", Arg::Str, true)], "Unloads an MCP server from a sandbox."),
    d("mcp_auth", &[("server", Arg::Str, true)], "Signs in to a remote MCP server (`sbx mcp auth`) from this machine's browser."),
    d("mcp_inspect", &[("server", Arg::Str, true)], "Shows `sbx mcp inspect` for a server (its header templates and whether their secrets are set)."),
    d("mcp_inspect_close", &[], "Hides the inspect text."),
    a("mcp_rm", "Unregister an MCP server", &[("server", Arg::Str, true)], "Unregisters an MCP server from this machine."),
    a(
        "mcp_save",
        "Register an MCP server",
        &[
            ("name", Arg::Str, true),
            ("kind", Arg::OneOf(KINDS), true),
            ("url", Arg::Str, false),
            ("headers", Arg::Str, false),
            ("command", Arg::Str, false),
            ("args", Arg::Str, false),
            ("dir", Arg::Str, false),
            ("replace", Arg::Str, false),
        ],
        "Registers an MCP server, or replaces `replace`. `headers` are separated by `;` or new lines; each `${KEY}` in them is a secret asked in the core's dialog. A `stdio` server shows its host command and needs an acknowledgement.",
    ),
];

/// The secret fields of an MCP server: each `${KEY}` in its headers, once, in order.
pub fn placeholders(headers: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for h in headers {
        let mut rest = h.as_str();
        while let Some(i) = rest.find("${") {
            rest = &rest[i + 2..];
            let Some(end) = rest.find('}') else { break };
            let k = &rest[..end];
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !keys.iter().any(|x| x == k) {
                keys.push(k.to_string());
            }
            rest = &rest[end + 1..];
        }
    }
    keys
}

/// The line a stdio MCP server runs on this machine, as Jarvis's dialog shows it.
pub fn host_command(command: &str, args: &[String], dir: &str) -> String {
    let mut line = String::new();
    if !dir.is_empty() {
        line.push_str(&format!("cd {} && ", shell_word(dir)));
    }
    line.push_str(&shell_word(command));
    for a in args {
        line.push(' ');
        line.push_str(&shell_word(a));
    }
    line
}

/// sbx's server name rule (`crates/core/src/mcp.rs`, `check_name`).
pub fn mcp_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !n.starts_with(['-', '.'])
}

fn confirm(c: Confirm) -> Result<Vec<Action>, String> {
    Ok(vec![c.into_action()])
}

/// The sandbox the panel is on: `mcp_view`, else the last one used.
pub fn shown(ctx: &Ctx) -> Option<String> {
    ctx.get("mcp_view")["sandbox"].as_str().or_else(|| ctx.get("app")["kept"]["mcp"]["sandbox"].as_str()).map(str::to_string)
}

/// Puts the panel on a sandbox, and keeps it as the last one used.
fn view_on(sb: &str) -> Vec<Action> {
    vec![Action::Set { key: "mcp_view".into(), value: json!({ "sandbox": sb }) }, Action::Remember { key: "sandbox".into(), value: json!(sb) }]
}

pub fn act(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    match q.name {
        "mcp_show" => {
            let mut v = match q.s("sandbox") {
                Some(sb) => view_on(sb),
                None => vec![Action::Set { key: "mcp_view".into(), value: J::Null }],
            };
            v.push(Action::Refresh("mcp"));
            v.push(Action::OpenPanel("mcp".into()));
            Ok(v)
        }
        "mcp_pick" => {
            let sb = q.req("sandbox");
            if ctx.sandbox(sb).is_none() {
                return Err(format!("no sandbox named {sb}"));
            }
            Ok(view_on(sb))
        }
        "mcp_load" | "mcp_unload" => {
            let (server, sb) = (q.req("server"), q.req("sandbox"));
            if ctx.sandbox(sb).is_none_or(|s| s["status"] != "running") {
                return Err(format!("{sb} is not running: a server is loaded into a running sandbox"));
            }
            let load = q.name == "mcp_load";
            let verb = if load { "load" } else { "unload" };
            let call = Call::post(format!("/api/sandboxes/{}/mcp/{verb}", crate::client::enc(sb)), json!({ "server": server }), format!("{verb} {server} {} {sb}", if load { "into" } else { "from" }));
            if load {
                Ok(vec![Action::Call(call)])
            } else {
                confirm(Confirm::new(format!("Unload {server} from {sb}"), call).line("Its agent keeps running; the server's tools go away.").labels("Unload", "Cancel"))
            }
        }
        "mcp_auth" => {
            let server = q.req("server");
            Ok(vec![Action::notice(format!("Signing in to {server}: finish in the browser")), Action::Call(Call::post(format!("/api/mcp/{}/auth", crate::client::enc(server)), json!({}), format!("sign in to {server}")))])
        }
        "mcp_inspect" => {
            let server = q.req("server");
            Ok(vec![
                Action::Set { key: "mcp_inspect_for".into(), value: json!(server) },
                Action::Call(Call::get(format!("/api/mcp/{}", crate::client::enc(server)), format!("inspect {server}")).store("mcp_inspect")),
            ])
        }
        "mcp_inspect_close" => Ok(vec![Action::Set { key: "mcp_inspect_for".into(), value: J::Null }, Action::Set { key: "mcp_inspect".into(), value: J::Null }]),
        "mcp_rm" => {
            let server = q.req("server");
            confirm(
                Confirm::new(format!("Unregister {server}"), Call::post(format!("/api/mcp/{}/rm", crate::client::enc(server)), json!({}), format!("remove {server}")).then(&["mcp"]))
                    .line("It is removed from this machine with its OAuth token. Sandboxes that loaded it keep it until it is unloaded there.")
                    .labels("Remove", "Cancel")
                    .danger(),
            )
        }
        "mcp_save" => mcp_save(q),
        n => Err(format!("{n}: not an MCP intent")),
    }
}

/// The server spec the daemon takes (`crates/core/src/mcp.rs`, `McpSpec`).
pub fn mcp_spec(q: &Asked) -> Result<J, String> {
    let name = q.req("name").trim().to_string();
    if !mcp_name_ok(&name) {
        return Err(format!("{name:?} is not a server name: letters, digits, '-', '_' and '.', up to 64"));
    }
    let kind = q.req("kind");
    let split = |s: Option<&str>, sep: &[char]| -> Vec<String> { s.unwrap_or("").split(sep).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect() };
    if kind == "stdio" {
        let command = q.s("command").unwrap_or("").trim().to_string();
        if command.is_empty() {
            return Err("a local server needs its command".into());
        }
        Ok(json!({ "name": name, "kind": "stdio", "url": "", "headers": [], "command": command, "args": split(q.s("args"), &[' ', '\t', '\n']), "dir": q.s("dir").unwrap_or("").trim() }))
    } else {
        let url = q.s("url").unwrap_or("").trim().to_string();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("a remote server needs its http:// or https:// URL".into());
        }
        Ok(json!({ "name": name, "kind": "remote", "url": url, "headers": split(q.s("headers"), &[';', '\n']), "command": "", "args": [], "dir": "" }))
    }
}

fn mcp_save(q: &Asked) -> Result<Vec<Action>, String> {
    let spec = mcp_spec(q)?;
    let name = spec["name"].as_str().unwrap_or("").to_string();
    let replace = q.s("replace").map(str::to_string);
    if let Some(r) = &replace {
        if !mcp_name_ok(r) {
            return Err(format!("{r:?} is not a server name"));
        }
    }
    let headers = strings(&spec["headers"]).unwrap_or_default();
    let keys = placeholders(&headers);
    let body = json!({ "spec": spec, "replace": replace, "secrets": {} });
    let what = if replace.is_some() { format!("save {name}") } else { format!("add {name}") };
    let mut c = Confirm::new(
        match &replace {
            Some(r) => format!("Replace MCP server {r}"),
            None => format!("Register MCP server {name}"),
        },
        Call::post("/api/mcp", body, what).then(&["mcp"]),
    )
    .labels(if replace.is_some() { "Save" } else { "Add" }, "Cancel");
    if spec["kind"] == "stdio" {
        let line = host_command(spec["command"].as_str().unwrap_or(""), &strings(&spec["args"]).unwrap_or_default(), spec["dir"].as_str().unwrap_or(""));
        c = c
            .line("A local server runs this command on this machine, outside any sandbox, with your user's rights, each time a sandbox loads it:")
            .preview(line, false)
            .ack("I have read this command");
    } else {
        c = c.line(format!("Remote server at {}", spec["url"].as_str().unwrap_or("")));
        if !headers.is_empty() {
            c = c.line(format!("Headers: {}", headers.join("; ")));
        }
        c = c.line("OAuth servers are added unauthorized: use Sign in once added.");
    }
    if let Some(r) = &replace {
        c = c.line(format!("sbx has no edit: {r} is removed and added again, which drops its OAuth token."));
    }
    if !keys.is_empty() {
        c = c.line(format!("{} stored by sbx: the server's requests carry {}, no sandbox sees {}.", keys.iter().map(|k| format!("${{{k}}}")).collect::<Vec<_>>().join(", "), if keys.len() == 1 { "it" } else { "them" }, if keys.len() == 1 { "it" } else { "them" }));
    }
    for k in &keys {
        let label = format!("${{{k}}}");
        let field = format!("secrets.{k}");
        c = if replace.is_some() { c.optional_secret(&label, &field, "unchanged if left empty") } else { c.secret(&label, &field) };
    }
    confirm(c)
}

/// Its rail entry: ONE declaration (a top bar extra the rail draws in its
/// configure zone; the palette lists it from the same extra).
pub fn rail_entry(ctx: &Ctx) -> Extra {
    let n = list(ctx.get("mcp"), "servers").len();
    Extra {
        place: Place::Rail { zone: crate::feat::Zone::Configure, order: 10 },
        id: "mcp".into(),
        label: "MCP servers".into(),
        icon: Some(ICON.into()),
        tone: "neutral".into(),
        help: format!("Registered on this machine, loaded into a sandbox ({n} registered)"),
        count: None,
        // The rail marks it on while the panel shows, and a second press
        // closes it; the panel then opens on the last sandbox used.
        intent: Some(("open_panel".into(), json!({ "screen": "mcp" }))),
    }
}

fn extras(ctx: &Ctx) -> Vec<Extra> {
    let mut v = vec![rail_entry(ctx)];
    // A pane's "⋯" menu: the panel on that sandbox.
    for sb in ctx.get("sandboxes").as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(name) = sb["name"].as_str() else { continue };
        if sb["status"] != "running" {
            continue;
        }
        v.push(Extra {
            place: Place::Pane(name.to_string()),
            id: "mcp-view".into(),
            label: "MCP servers…".into(),
            icon: Some(ICON.into()),
            tone: "muted".into(),
            help: format!("The MCP servers panel, on {name}: load or unload a server"),
            count: None,
            intent: Some(("mcp_show".into(), json!({ "sandbox": name }))),
        });
    }
    v
}

#[cfg(test)]
mod tests;
