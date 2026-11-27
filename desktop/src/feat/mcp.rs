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
//! A remote server's optional OAuth client ID (`client_id`, for a server with
//! no dynamic client registration, such as GitHub) adds one optional secret,
//! the client secret, asked in the same dialog (`secrets.client_secret`; the
//! daemon stores it as `mcp:<name>:client_secret` before the add). A header
//! that carries a credential (`Authorization`, `Cookie`, `X-API-Key`,
//! `X-Auth-Token`...) without a `${KEY}` is refused before the Confirm is built, in
//! the core's words ([`header_refusal`]; the screen says it under the row).
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
    // `mcp_mode`: each running sandbox's MCP mode and connected servers
    // (sbx 0.54, one `sbx inspect` each), read when the panel opens and
    // when it moves to a sandbox not read yet; never on a timer.
    data: &[Data::new("mcp", "/api/mcp"), Data { key: "mcp_mode", path: "/api/sandboxes/{sandbox}/mcp", on: &[], every: 0, lazy: true }],
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
    d("mcp_show", &[("sandbox", Arg::Str, false)], "Opens the MCP servers panel and reads the servers again, and each running sandbox's MCP mode (static or dynamic) and connected servers (sbx 0.54); on that sandbox when given (Load and Unload act on it), else on the last one used."),
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
            ("client_id", Arg::Str, false),
        ],
        "Registers an MCP server, or replaces `replace`. `headers` are separated by `;` or new lines; each `${KEY}` in them is a secret asked in the core's dialog. `client_id`: a remote server's pre-registered OAuth client ID (a server with no dynamic client registration, such as GitHub); its optional client secret is asked in the same dialog. A `stdio` server shows its host command and needs an acknowledgement.",
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

/// The key a pre-registered OAuth client's secret is stored under
/// (`crates/core/src/mcp.rs`, `CLIENT_SECRET`): no header placeholder takes it.
pub const CLIENT_SECRET: &str = "client_secret";

/// The core's OAuth client ID rule (`crates/core/src/mcp.rs`, `check_client_id`).
pub fn client_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || "-._~:@/+=".contains(c))
}

/// The core's credential header words (`crates/core/src/mcp.rs`,
/// `CREDENTIAL_WORDS` and `KEY_AFTER`; `tests/js/mcp-header-cases.json`).
pub const CREDENTIAL_WORDS: &[&str] = &["authorization", "auth", "authentication", "token", "secret", "password", "passwd", "credential", "credentials", "cookie", "apikey"];
pub const KEY_AFTER: &[&str] = &["api", "access", "secret", "private", "auth", "subscription", "license", "signing", "session"];

/// The core's credential header rule (`crates/core/src/mcp.rs`,
/// `credential_header`): on whole words, a credential word, or `key` next to
/// a word of [`KEY_AFTER`].
pub fn credential_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty()).collect();
    words.iter().enumerate().any(|(i, w)| {
        CREDENTIAL_WORDS.contains(w) || (*w == "key" && [i.checked_sub(1), Some(i + 1)].into_iter().flatten().filter_map(|j| words.get(j)).any(|n| KEY_AFTER.contains(n)))
    })
}

/// The core's refusal of a credential header without a `${KEY}`
/// (`crates/core/src/mcp.rs`, `header_refusal`): the name only, never the value.
pub fn header_refusal(header: &str) -> Option<String> {
    let (name, value) = header.split_once(':')?;
    let name = name.trim();
    if !credential_header(name) || !placeholders(&[value.to_string()]).is_empty() {
        return None;
    }
    let example = if matches!(name.to_ascii_lowercase().as_str(), "authorization" | "proxy-authorization") { "Bearer ${token}" } else { "${token}" };
    Some(format!("{name} carries a credential: write its value with a placeholder, such as `{example}`, and sbxw asks for the secret separately"))
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
            v.push(Action::Refresh("mcp_mode"));
            v.push(Action::OpenPanel("mcp".into()));
            Ok(v)
        }
        "mcp_pick" => {
            let sb = q.req("sandbox");
            if ctx.sandbox(sb).is_none() {
                return Err(format!("no sandbox named {sb}"));
            }
            let mut v = view_on(sb);
            if ctx.get("mcp_mode")[sb].is_null() {
                v.push(Action::Refresh("mcp_mode"));
            }
            Ok(v)
        }
        "mcp_load" | "mcp_unload" => {
            let (server, sb) = (q.req("server"), q.req("sandbox"));
            if ctx.sandbox(sb).is_none_or(|s| s["status"] != "running") {
                return Err(format!("{sb} is not running: a server is loaded into a running sandbox"));
            }
            let load = q.name == "mcp_load";
            let verb = if load { "load" } else { "unload" };
            let call = Call::post(format!("/api/sandboxes/{}/mcp/{verb}", crate::client::enc(sb)), json!({ "server": server }), format!("{verb} {server} {} {sb}", if load { "into" } else { "from" }));
            // What is connected follows a load or an unload.
            let call = call.then(&["mcp_mode"]);
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
    let client_id = q.s("client_id").unwrap_or("").trim().to_string();
    let split = |s: Option<&str>, sep: &[char]| -> Vec<String> { s.unwrap_or("").split(sep).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect() };
    if kind == "stdio" {
        let command = q.s("command").unwrap_or("").trim().to_string();
        if command.is_empty() {
            return Err("a local server needs its command".into());
        }
        if !client_id.is_empty() {
            return Err("an OAuth client ID is for a remote server, not a local command".into());
        }
        Ok(json!({ "name": name, "kind": "stdio", "url": "", "headers": [], "command": command, "args": split(q.s("args"), &[' ', '\t', '\n']), "dir": q.s("dir").unwrap_or("").trim() }))
    } else {
        let url = q.s("url").unwrap_or("").trim().to_string();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("a remote server needs its http:// or https:// URL".into());
        }
        if !client_id.is_empty() && !client_id_ok(&client_id) {
            return Err(format!("{client_id:?} is not an OAuth client ID: letters, digits and - . _ ~ : @ / + =, up to 256, not starting with '-'"));
        }
        let headers = split(q.s("headers"), &[';', '\n']);
        // Before any Confirm: a literal credential is never drawn in it.
        if let Some(why) = headers.iter().find_map(|h| header_refusal(h)) {
            return Err(why);
        }
        if placeholders(&headers).iter().any(|k| k == CLIENT_SECRET) {
            return Err(format!("${{{CLIENT_SECRET}}} is the OAuth client secret's own key: name the header's secret otherwise"));
        }
        Ok(json!({ "name": name, "kind": "remote", "url": url, "headers": headers, "command": "", "args": [], "dir": "", "client_id": client_id }))
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
        if let Some(id) = spec["client_id"].as_str().filter(|id| !id.is_empty()) {
            c = c.line(format!("OAuth client ID: {id} (a pre-registered client). Its client secret, if it has one, is stored by sbx; leave it empty for a public client."));
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
    if spec["client_id"].as_str().is_some_and(|id| !id.is_empty()) {
        let hint = if replace.is_some() { "unchanged if left empty" } else { "only for a confidential client" };
        c = c.optional_secret("OAuth client secret", &format!("secrets.{CLIENT_SECRET}"), hint);
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
