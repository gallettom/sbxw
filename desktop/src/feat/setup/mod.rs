//! Setup: what every sandbox starts with and what sbxw holds for them, at
//! parity with the web UI's Setup dialog (`assets/js/setup.js`) and its tabs:
//! secrets and the GitHub token (`github.js`), kits (`kits.js`), kept skills
//! (`skills.js`), MCP servers (`mcp.js`), remote hosts (`hosts.js`), and the
//! voice models (the setup part of `concierge.js`).
//!
//! One modal screen, `screens/setup.luau`, opened from a gear in the top bar.
//! The daemon's answers are handed to it as they come (`data.secrets`,
//! `data.kits`, ...). The screen keeps its forms (a kits line being edited,
//! a server being typed) and returns intents; this file checks them and
//! builds what the core sends.
//!
//! **Secrets.** A screen never sees a secret value: the daemon only ever
//! answers names and whether a value is set, and a value is typed in the
//! core's own dialog ([`crate::feat::Secret`]), put in the request body on
//! Confirm and dropped. Nothing here stores, logs or returns one.
//!
//! **Approvals.** Everything that changes what sandboxes get, or what sbx
//! holds, is an intent of risk `Approval`: its dialog is built here from the
//! daemon's data and the intent's checked arguments, and drawn by the core.
//! The web UI asks a `confirm()` for the same things; here the question is
//! out of reach of the screen.

pub mod standin;

use crate::feat::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, Prepare, ScreenDecl, Slot};
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};

pub const FEATURE: Feature = Feature {
    name: "setup",
    data: DATA,
    events: &["voice"],
    screens: &[ScreenDecl { name: "setup", src: include_str!("../../../screens/setup.luau"), slot: Slot::Modal { width: 720.0 } }],
    intents: INTENTS,
    act,
    on_event: Some(on_event),
    extras: Some(extras),
    standin: Some(standin::answer),
    ..Feature::EMPTY
};

/// The routes read for the screen, each under its own key. `voice` also
/// follows the bus (`voice` events carry the same status).
const DATA: &[Data] = &[
    Data::new("secrets", "/api/secrets"),
    Data::new("github_token", "/api/github-token"),
    Data::new("kits", "/api/kits"),
    Data::new("my_skills", "/api/my-skills"),
    Data::new("mcp", "/api/mcp"),
    Data::new("remote_hosts", "/api/remote-hosts"),
    Data::new("voice", "/api/voice"),
];

/// The keys a tab reads, for `setup_load`.
fn section_keys(section: &str) -> &'static [&'static str] {
    match section {
        "secrets" => &["secrets", "github_token"],
        "kits" => &["kits"],
        "skills" => &["my_skills"],
        "mcp" => &["mcp"],
        "hosts" => &["remote_hosts"],
        "voice" => &["voice"],
        _ => &[],
    }
}

const SECTIONS: &[&str] = &["secrets", "kits", "skills", "mcp", "hosts", "voice"];
const KINDS: &[&str] = &["remote", "stdio"];
const PORT: Arg = Arg::Int(1, 65535);

const fn d(name: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Direct, args, doc }
}
const fn a(name: &'static str, asks: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Approval(asks), args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("setup_load", &[("section", Arg::OneOf(SECTIONS), true)], "Reads a Setup tab's data from the daemon again (`secrets`, `kits`, `skills`, `mcp`, `hosts`, `voice`)."),
    a("github_token_set", "Store the GitHub token", &[], "Asks for the GitHub token in the core's dialog (with a secret field) and stores it as sbx's global `github` secret."),
    a("github_token_gh", "Use the gh login for GitHub", &[], "Makes sbx ask this machine's `gh auth token` for the GitHub token, and refresh it."),
    a("github_token_rm", "Remove the GitHub token", &[], "Removes sbx's global `github` secret."),
    a(
        "secret_custom_set",
        "Store a secret for other services",
        &[("hosts", Arg::Str, true), ("env", Arg::Str, true)],
        "Asks for a custom secret's value in the core's dialog: the sandbox sees a placeholder in `env`, the proxy puts the value in requests to `hosts` (comma or space separated).",
    ),
    a("secret_rm", "Remove a secret", &[("name", Arg::Str, true)], "Removes a global secret sbx lists under that name."),
    a(
        "kits_save",
        "Change a kits line",
        &[("path", Arg::Str, true), ("kits", Arg::Str, false), ("defaults", Arg::Bool, false)],
        "Writes the `kits` line of one of the sbxw.toml files sbxw uses: `kits` one reference per line, or `defaults = true` to remove the line.",
    ),
    d("skills_look", &[("sandbox", Arg::Str, true)], "Lists the skills a sandbox has that could be kept (starts a stopped sandbox)."),
    a("skill_keep", "Keep a skill for every new sandbox", &[("sandbox", Arg::Str, true), ("name", Arg::Str, true)], "Keeps one of the skills `skills_look` found: every sandbox created from now on gets it."),
    a("skill_forget", "Stop keeping a skill", &[("name", Arg::Str, true)], "Stops giving a kept skill to new sandboxes (existing ones keep their copy)."),
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
    a(
        "host_save",
        "Add or change a remote host",
        &[
            ("name", Arg::Str, true),
            ("ssh", Arg::Str, false),
            ("host", Arg::Str, false),
            ("user", Arg::Str, false),
            ("port", PORT, false),
            ("key", Arg::Str, false),
            ("agent_port", PORT, false),
            ("sbx_path", Arg::Str, false),
            ("trust_secrets", Arg::Bool, false),
            ("editing", Arg::Str, false),
        ],
        "Adds a remote host, or edits `editing`. For a new address the core first reads its SSH host key, then shows the fingerprints in its dialog; they are trusted on Confirm.",
    ),
    a("host_rm", "Remove a remote host", &[("name", Arg::Str, true)], "Unregisters a remote host from hosts.toml (nothing on the host is touched)."),
    d("host_test", &[("name", Arg::Str, true)], "Runs `sbx version` over a remote host's link and times a round trip."),
    d("host_provision", &[("name", Arg::Str, true)], "Shows the command that installs sbx on a remote host (`data.provision`), to run in a terminal."),
    d("host_provision_close", &[], "Hides the provision command."),
    d("hosts_reload", &[], "Reads hosts.toml again."),
    a("voice_install", "Download the voice models", &[], "Downloads the voice models (about 2 GB), with progress in `data.voice.job`."),
    d("voice_cancel", &[], "Stops a voice model download (it resumes where it stopped)."),
    a("voice_rm", "Remove the voice models", &[], "Deletes the downloaded voice models."),
    d("voice_warm", &[], "Loads the voice models now, before the first word."),
];

// ---------------------------------------------------------------------------
// Reading the daemon's answers

fn list<'a>(v: &'a J, key: &str) -> &'a [J] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn str_of<'a>(v: &'a J, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}

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

/// A word as a POSIX shell reads it (the host command shown before a stdio server is added).
pub fn shell_word(w: &str) -> String {
    if !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./~-".contains(c)) {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
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

/// The daemon's rule for a custom secret's variable.
pub fn env_ok(env: &str) -> bool {
    env.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && env.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Hosts as the daemon takes them: split on commas and spaces, no scheme or path.
pub fn split_hosts(raw: &str) -> Result<Vec<String>, String> {
    let hosts: Vec<String> = raw.split([',', ' ', '\t', '\n']).map(str::trim).filter(|h| !h.is_empty()).map(str::to_string).collect();
    if hosts.is_empty() {
        return Err("name at least one host the secret is for (api.example.com, *.example.com)".into());
    }
    if let Some(h) = hosts.iter().find(|h| h.starts_with('-') || h.contains('/')) {
        return Err(format!("{h:?} is not a host: one name or pattern per entry, no scheme or path"));
    }
    Ok(hosts)
}

/// `kits = [...]` as the file will read; `None` is no line.
fn kits_line(list: Option<&[String]>) -> String {
    match list {
        None => "(no kits line: the default kits apply)".into(),
        Some(l) => format!("kits = [{}]", l.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", ")),
    }
}

fn strings(v: &J) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

/// What a kit reference is, for the dialog.
fn kit_kind(r: &str) -> &'static str {
    if r.starts_with("sbxw:") {
        "bundled"
    } else if r.contains("/kits/jarvis-") {
        "Jarvis kit"
    } else if r.starts_with(['.', '/', '~']) {
        "local kit"
    } else {
        "registry kit"
    }
}

/// Bundled kits that reach outside the sandbox (Jarvis's rule for a kits line,
/// `crates/core/src/jarvis.rs`): the person's SSH agent, git identity, ports, the Mac.
const BUNDLED_REACH: &[&str] = &["sbxw:ssh-agent", "sbxw:git-identity", "sbxw:dev-ports", "sbxw:terminal-mac"];

// ---------------------------------------------------------------------------
// The intents

fn confirm(c: Confirm) -> Result<Vec<Action>, String> {
    Ok(vec![c.into_action()])
}

fn refresh(keys: &[&'static str]) -> Vec<Action> {
    keys.iter().map(|k| Action::Refresh(k)).collect()
}

pub fn act(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    match q.name {
        "setup_load" => Ok(refresh(section_keys(q.req("section")))),

        // ── Secrets ─────────────────────────────────────────────────────
        "github_token_set" => {
            let set = ctx.get("github_token")["set"] == true;
            confirm(
                Confirm::new(if set { "Replace the GitHub token" } else { "Store a GitHub token" }, Call::post("/api/github-token", json!({}), "store the GitHub token").then(&["github_token", "secrets"]))
                    .line("sbx's proxy presents it to github.com and api.github.com for every sandbox's git and gh. It never enters a sandbox.")
                    .line("Stored by sbx (`sbx secret set github`); sbxw never reads it back.")
                    .secret("GitHub token (ghp_… or github_pat_…)", "token")
                    .labels("Save", "Cancel"),
            )
        }
        "github_token_gh" => {
            let gh = str_of(ctx.get("github_token"), "gh").ok_or("no GitHub CLI found on this machine: install gh and run gh auth login")?;
            confirm(
                Confirm::new("Use your gh login for GitHub", Call::post("/api/github-token/gh", json!({}), "use the gh login for GitHub").then(&["github_token", "secrets"]))
                    .line(format!("sbx runs `{gh} auth token` whenever it needs the token, and keeps it fresh."))
                    .line("Every sandbox's git and gh then act as your gh account (the token itself never enters a sandbox).")
                    .labels("Use gh", "Cancel"),
            )
        }
        "github_token_rm" => confirm(
            Confirm::new("Remove the GitHub token", Call::post("/api/github-token/rm", json!({}), "remove the GitHub token").then(&["github_token", "secrets"]))
                .line("Sandboxes lose git push over HTTPS, and gh, until you set another.")
                .labels("Remove", "Cancel")
                .danger(),
        ),
        "secret_custom_set" => {
            let hosts = split_hosts(q.req("hosts"))?;
            let env = q.req("env").trim().to_string();
            if !env_ok(&env) {
                return Err("the variable name is letters, digits and _, not starting with a digit".into());
            }
            confirm(
                Confirm::new(format!("Store a secret as ${env}"), Call::post("/api/secrets/custom", json!({ "hosts": hosts, "env": env }), format!("store the secret ${env}")).then(&["secrets"]))
                    .line(format!("Every sandbox sees a placeholder in ${env}."))
                    .line(format!("sbx's proxy swaps in the value on requests to {} only.", hosts.join(", ")))
                    .secret("Value", "value")
                    .labels("Save", "Cancel"),
            )
        }
        "secret_rm" => {
            let name = q.req("name");
            let row = list(ctx.get("secrets"), "secrets").iter().find(|x| x["name"] == name).ok_or_else(|| format!("sbx lists no secret named {name}"))?;
            if row["scope"] != "global" {
                return Err(format!("{name} belongs to one sandbox; remove it there with sbx"));
            }
            confirm(
                Confirm::new(format!("Remove the secret {name}"), Call::post("/api/secrets/rm", json!({ "name": name }), format!("remove the secret {name}")).then(&["secrets", "github_token"]))
                    .line("Sandboxes then send its placeholder unchanged.")
                    .labels("Remove", "Cancel")
                    .danger(),
            )
        }

        // ── Kits ────────────────────────────────────────────────────────
        "kits_save" => kits_save(ctx, q),

        // ── Skills ──────────────────────────────────────────────────────
        "skills_look" => {
            let sb = q.req("sandbox");
            if ctx.sandbox(sb).is_none() {
                return Err(format!("no sandbox named {sb}"));
            }
            Ok(vec![
                Action::Set { key: "skills_for".into(), value: json!(sb) },
                Action::Set { key: "sandbox_skills".into(), value: J::Null },
                Action::Fetch { key: "sandbox_skills".into(), path: format!("/api/sandboxes/{}/skills", crate::client::enc(sb)) },
            ])
        }
        "skill_keep" => {
            let (sb, name) = (q.req("sandbox"), q.req("name"));
            if ctx.get("skills_for") != sb {
                return Err(format!("look at {sb}'s skills first"));
            }
            let s = list(ctx.get("sandbox_skills"), "skills").iter().find(|s| s["name"] == name).ok_or_else(|| format!("{sb} has no skill named {name}"))?;
            if s["keepable"] != true {
                return Err(format!("/{name} cannot be kept: lowercase letters, digits, - and _ only"));
            }
            let replace = list(ctx.get("my_skills"), "skills").iter().any(|k| k["name"] == name);
            let origin = s["origin"].clone();
            let mut c = Confirm::new(
                if replace { format!("Replace your kept /{name}") } else { format!("Keep /{name} for every new sandbox") },
                Call::post(format!("/api/sandboxes/{}/skills/keep", crate::client::enc(sb)), json!({ "origin": origin, "name": name, "replace": replace }), format!("keep /{name}")).then(&["my_skills"]),
            )
            .line(format!("From {sb}: {}", s["path"].as_str().unwrap_or("?")))
            .line(format!("{} files, {} bytes.", s["files"].as_u64().unwrap_or(0), s["bytes"].as_u64().unwrap_or(0)))
            .line(if replace { "Sandboxes created from now on get this version instead of the one you kept." } else { "Every sandbox created from now on gets it. Read its SKILL.md first: a skill is instructions the agent follows." })
            .labels(if replace { "Replace" } else { "Keep" }, "Cancel");
            if let Some(desc) = str_of(s, "description") {
                c = c.preview(desc, false);
            }
            confirm(c)
        }
        "skill_forget" => {
            let name = q.req("name");
            if !list(ctx.get("my_skills"), "skills").iter().any(|k| k["name"] == name) {
                return Err(format!("no kept skill named {name}"));
            }
            confirm(
                Confirm::new(format!("Stop keeping /{name}"), Call::post("/api/my-skills/rm", json!({ "name": name }), format!("forget /{name}")).then(&["my_skills"]))
                    .line("Sandboxes created from now on will not have it; existing ones keep their copy.")
                    .labels("Remove", "Cancel")
                    .danger(),
            )
        }

        // ── MCP ─────────────────────────────────────────────────────────
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

        // ── Hosts ───────────────────────────────────────────────────────
        "host_save" => host_save(ctx, q),
        "host_rm" => {
            let name = q.req("name");
            let h = list(ctx.get("remote_hosts"), "hosts").iter().find(|h| h["name"] == name).ok_or_else(|| format!("no remote host named {name}"))?;
            let held = h["sandboxes"].as_u64().unwrap_or(0);
            let mut c = Confirm::new(format!("Remove host {name}"), Call::post(format!("/api/remote-hosts/{}/rm", crate::client::enc(name)), json!({ "force": held > 0 }), format!("remove host {name}")).then(&["remote_hosts"]))
                .line("It leaves ~/.sbxw/hosts.toml. Nothing on the host is touched; its known_hosts line stays.")
                .labels("Remove", "Cancel")
                .danger();
            if held > 0 {
                c = c.line(format!("sbxw holds {held} sandbox record{} there: kept on disk, no longer listed.", if held == 1 { "" } else { "s" })).ack("Remove it anyway");
            }
            confirm(c)
        }
        "host_test" => {
            let name = q.req("name");
            Ok(vec![Action::Call(Call::post(format!("/api/remote-hosts/{}/test", crate::client::enc(name)), json!({}), format!("test {name}")).then(&["remote_hosts"]))])
        }
        "host_provision" => {
            let name = q.req("name");
            let bin = str_of(ctx.get("remote_hosts"), "sbxw").unwrap_or("sbxw");
            Ok(vec![Action::Set { key: "provision".into(), value: json!({ "host": name, "command": format!("{} host provision {}", shell_word(bin), shell_word(name)) }) }])
        }
        "host_provision_close" => Ok(vec![Action::Set { key: "provision".into(), value: J::Null }]),
        "hosts_reload" => Ok(vec![Action::Call(Call::post("/api/remote-hosts/reload", json!({}), "read hosts.toml again").then(&["remote_hosts"]))]),

        // ── Voice ───────────────────────────────────────────────────────
        "voice_install" => {
            let v = ctx.get("voice");
            if v["compiled"] == false {
                return Err("this sbxw was built without voice: cargo build --release --features voice".into());
            }
            let mb = v["approx_mb"].as_f64().unwrap_or(0.0);
            confirm(
                Confirm::new("Download the voice models", Call::post("/api/voice/install", json!({}), "start the voice download").then(&["voice"]).quiet())
                    .line(format!("About {:.1} GB, into {}.", mb / 1000.0, v["dir"].as_str().unwrap_or("the models folder")))
                    .line("It can be cancelled and resumed. Speech is then turned into text on this machine.")
                    .labels("Download", "Cancel"),
            )
        }
        "voice_cancel" => Ok(vec![Action::Call(Call::post("/api/voice/cancel", json!({}), "cancel the voice download").then(&["voice"]))]),
        "voice_rm" => confirm(
            Confirm::new("Remove the voice models", Call::post("/api/voice/rm", json!({}), "remove the voice models").then(&["voice"]))
                .line(format!("Deletes the downloaded models from {}. Talking is off until they are installed again.", ctx.get("voice")["dir"].as_str().unwrap_or("the models folder")))
                .labels("Remove", "Cancel")
                .danger(),
        ),
        "voice_warm" => {
            let speech = ctx.get("voice")["speech"]["supported"] == true;
            Ok(vec![Action::Call(Call::post("/api/voice/warm", json!({ "speech": speech }), "load the voice models").quiet())])
        }
        n => Err(format!("{n}: not a setup intent")),
    }
}

fn kits_save(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let path = q.req("path");
    let panel = ctx.get("kits");
    let file = list(panel, "files").iter().find(|f| f["path"] == path).ok_or_else(|| format!("{path} is not an sbxw.toml sbxw uses"))?;
    if let Some(e) = str_of(file, "error") {
        return Err(format!("{path} cannot be edited: {e}"));
    }
    let old = strings(&file["kits"]);
    let new: Option<Vec<String>> = if q.b("defaults") == Some(true) {
        None
    } else {
        let mut seen: Vec<String> = Vec::new();
        for k in q.s("kits").unwrap_or("").lines().map(str::trim).filter(|k| !k.is_empty()) {
            if !seen.iter().any(|x| x == k) {
                seen.push(k.to_string());
            }
        }
        Some(seen)
    };
    if new == old {
        return Err("nothing to save: the line is unchanged".into());
    }
    if new.as_deref().is_some_and(|l| l.iter().any(|k| k == "sbxw:jarvis")) {
        return Err("sbxw:jarvis is the kit of Jarvis's own sandbox: a project's kits cannot list it".into());
    }
    let defaults = strings(&panel["defaults"]).unwrap_or_default();
    let before = old.clone().unwrap_or_else(|| defaults.clone());
    let after = new.clone().unwrap_or_else(|| defaults.clone());
    let added: Vec<&String> = after.iter().filter(|k| !before.contains(k)).collect();
    let removed: Vec<&String> = before.iter().filter(|k| !after.contains(k)).collect();

    let short = path.rsplit('/').next().unwrap_or(path);
    let mut c = Confirm::new(
        if new.is_none() { format!("Use the default kits in {short}") } else { format!("Change the kits of {short}") },
        Call::post("/api/kits", json!({ "path": path, "kits": new }), format!("save the kits of {short}")).then(&["kits"]),
    )
    .line(path.to_string());
    for k in &added {
        c = c.line(format!("Adds {k} ({}).", kit_kind(k)));
    }
    for k in &removed {
        c = c.line(format!("Removes {k}."));
    }
    let users: Vec<String> = file["daemon"].as_bool().filter(|d| *d).map(|_| "new sandboxes from this app".to_string()).into_iter().chain(strings(&file["sandboxes"]).unwrap_or_default()).collect();
    if !users.is_empty() {
        c = c.line(format!("Used by {}; a running sandbox gets the change at its next up.", users.join(", ")));
    }
    let reach: Vec<&&String> = added.iter().filter(|k| !k.starts_with("sbxw:") || BUNDLED_REACH.contains(&k.as_str())).collect();
    if !reach.is_empty() {
        c = c.line("A kit can bring scripts that run in the sandbox, network rules, and access to this machine (an SSH agent, ports, credentials). Read what it declares before saving.").ack(&format!(
            "I know what {} brings",
            reach.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let preview = format!("- {}\n+ {}", kits_line(old.as_deref()), kits_line(new.as_deref()));
    confirm(c.preview(preview, true).labels("Save", "Cancel"))
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

/// The host form as the daemon reads it (`svc/hosts.rs`, `HostForm`), with the name.
fn host_form(q: &Asked) -> J {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), json!(q.req("name").trim()));
    for k in ["ssh", "host", "user", "key", "sbx_path"] {
        if let Some(v) = q.s(k).map(str::trim).filter(|v| !v.is_empty()) {
            m.insert(k.into(), json!(v));
        }
    }
    for k in ["port", "agent_port"] {
        if let Some(v) = q.i(k) {
            m.insert(k.into(), json!(v));
        }
    }
    m.insert("trust_secrets".into(), json!(q.b("trust_secrets").unwrap_or(false)));
    J::Object(m)
}

/// What decides which host key is asked about: the address part of the form.
pub fn target_of(form: &J) -> J {
    json!({ "ssh": form["ssh"], "host": form["host"], "user": form["user"], "port": form["port"], "key": form["key"] })
}

fn host_save(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let (_, old) = host_checked(ctx, q)?;
    let form = host_form(q);
    if old.as_ref().is_some_and(|h| target_of(h) == target_of(&form)) {
        // An edit that keeps the address: its key is already trusted.
        return confirm(host_confirm(ctx, q, None)?);
    }
    // A new address: the core reads the host's key, then shows its
    // fingerprints in the dialog. A screen never names them.
    Ok(vec![Action::Prepare(Box::new(Prepare {
        title: format!("Reading {}'s SSH host key", q.req("name").trim()),
        call: Call::post("/api/remote-hosts/key", form, format!("read {}'s host key", q.req("name").trim())).quiet().timeout(30),
        asked: q.clone(),
        build: host_build,
    }))])
}

/// The form's rules, and the host being edited.
fn host_checked(ctx: &Ctx, q: &Asked) -> Result<(String, Option<J>), String> {
    let name = q.req("name").trim().to_string();
    if q.s("ssh").is_none() && q.s("host").is_none() {
        return Err("give an SSH alias, or an address".into());
    }
    let hosts = list(ctx.get("remote_hosts"), "hosts");
    let old = match q.s("editing") {
        Some(e) => {
            if e != name {
                return Err("a host's name cannot change: remove it and add it again".into());
            }
            Some(hosts.iter().find(|h| h["name"] == e).ok_or_else(|| format!("no remote host named {e}"))?.clone())
        }
        None => {
            if name == "local" {
                return Err("'local' is this machine, not a remote host".into());
            }
            if hosts.iter().any(|h| h["name"] == name.as_str()) {
                return Err(format!("{name} is already registered: edit it"));
            }
            None
        }
    };
    Ok((name, old))
}

/// The dialog once the key is read (`POST /api/remote-hosts/key`'s answer).
pub fn host_build(ctx: &Ctx, q: &Asked, key: &J) -> Result<Confirm, String> {
    if key["ok"] == false {
        return Err(format!("the host key could not be read: {}", key["error"].as_str().unwrap_or("unknown error")));
    }
    host_confirm(ctx, q, Some(key))
}

fn host_confirm(ctx: &Ctx, q: &Asked, key: Option<&J>) -> Result<Confirm, String> {
    let (name, old) = host_checked(ctx, q)?;
    let form = host_form(q);
    let editing = q.s("editing");
    let fingerprints: Option<Vec<String>> = key.filter(|k| k["known"] != true).map(|k| strings(&k["fingerprints"]).unwrap_or_default());
    if fingerprints.as_ref().is_some_and(Vec::is_empty) {
        return Err("ssh-keyscan found no key for this host: is the address right, and is SSH up there?".into());
    }
    let mut body = form.clone();
    body["fingerprints"] = json!(fingerprints);
    let path = match editing {
        Some(e) => format!("/api/remote-hosts/{}/edit", crate::client::enc(e)),
        None => "/api/remote-hosts/add".into(),
    };
    let target = match form["ssh"].as_str() {
        Some(a) => format!("ssh {a}"),
        None => format!(
            "{}{}{}",
            form["user"].as_str().map(|u| format!("{u}@")).unwrap_or_default(),
            form["host"].as_str().unwrap_or(""),
            form["port"].as_i64().map(|p| format!(":{p}")).unwrap_or_default()
        ),
    };
    let mut c = Confirm::new(
        if editing.is_some() { format!("Change host {name}") } else { format!("Add host {name}") },
        Call::post(path, body, if editing.is_some() { format!("save host {name}") } else { format!("add host {name}") }).then(&["remote_hosts"]),
    )
    .line(format!("sbxw drives sbx on it over SSH: {target}."))
    .labels(if editing.is_some() { "Save" } else { "Add host" }, "Cancel");
    if let Some(p) = form["agent_port"].as_i64() {
        c = c.line(format!("Agent port {p}."));
    }
    match (key, &fingerprints) {
        (Some(k), Some(fps)) => {
            c = c
                .line(format!("Its SSH host key is new to this machine ({}). Trust it only if these fingerprints are the host's:", k["known_as"].as_str().unwrap_or("?")))
                .preview(fps.join("\n"), false)
                .ack("These fingerprints are the host's");
        }
        (Some(k), None) => c = c.line(format!("Its SSH host key is already in known_hosts ({}).", k["known_as"].as_str().unwrap_or("?"))),
        _ => {}
    }
    let trust = form["trust_secrets"] == true;
    if trust && old.as_ref().is_none_or(|h| h["trust_secrets"] != true) {
        c = c.line("Trusted with secrets: sbxw sends it your Anthropic token (CLAUDE_CODE_OAUTH_TOKEN or API key), the OAuth kit (your Claude login) and your skills. Anyone with root on the host can read them.").danger();
    } else if !trust && old.as_ref().is_some_and(|h| h["trust_secrets"] == true) {
        c = c.line("No longer trusted with secrets: sbxw stops sending them.");
    }
    Ok(c)
}

fn on_event(_: &Ctx, name: &str, data: &J) -> Vec<Action> {
    if name == "voice" && data.is_object() {
        return vec![Action::Set { key: "voice".into(), value: data.clone() }];
    }
    vec![]
}

fn extras(ctx: &Ctx) -> Vec<Extra> {
    let mut out = vec![Extra {
        place: Place::Topbar,
        id: "setup".into(),
        label: String::new(),
        icon: Some("gear".into()),
        tone: "muted".into(),
        help: "Setup: secrets, kits, skills, MCP, hosts, voice".into(),
        count: None,
        intent: Some(("open_modal".into(), json!({ "screen": "setup" }))),
    }];
    for sb in ctx.get("sandboxes").as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let n = sb["kits_waiting"].as_u64().unwrap_or(0);
        let Some(name) = sb["name"].as_str() else { continue };
        if n == 0 {
            continue;
        }
        out.push(Extra {
            place: Place::Sandbox(name.to_string()),
            id: "kit-add".into(),
            label: String::new(),
            icon: Some("check".into()),
            tone: "amber".into(),
            help: format!("{n} kit{} waiting for your approval: answer sbx's prompt in a pane", if n == 1 { "" } else { "s" }),
            count: Some(n),
            intent: Some(("open_terminal".into(), json!({ "sandbox": name, "mode": "kit-add", "pane": "spare" }))),
        });
    }
    out
}

#[cfg(test)]
mod tests;
