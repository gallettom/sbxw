//! Setup (Settings in the rail): what sbxw holds for every sandbox, at
//! parity with the web UI's Setup dialog (`assets/js/setup.js`) and its tabs:
//! secrets and the GitHub token (`github.js`), remote hosts (`hosts.js`),
//! and the voice models (the setup part of `concierge.js`). MCP servers,
//! kits and kept skills were tabs here; each is now a docked panel of its
//! own with its own rail entry (`feat/mcp.rs`, `feat/kits.rs`,
//! `feat/skills.rs`). Their stand-in routes stay in `standin.rs`.
//!
//! One modal screen, `screens/setup.luau`, opened from Settings in the rail.
//! The daemon's answers are handed to it as they come (`data.secrets`,
//! `data.remote_hosts`, ...). The screen keeps its forms (a host being
//! typed) and returns intents; this file checks them and builds what the
//! core sends.
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
    screens: &[ScreenDecl { name: "setup", src: include_str!("../../../screens/setup.luau"), slot: Slot::Modal { width: 720.0, light_dismiss: false } }],
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
    Data::new("remote_hosts", "/api/remote-hosts"),
    Data::new("voice", "/api/voice"),
];

/// The keys a tab reads, for `setup_load`.
fn section_keys(section: &str) -> &'static [&'static str] {
    match section {
        "secrets" => &["secrets", "github_token"],
        "hosts" => &["remote_hosts"],
        "voice" => &["voice"],
        _ => &[],
    }
}

/// The tabs, in their order.
pub const SECTIONS: &[&str] = &["secrets", "hosts", "voice"];
const PORT: Arg = Arg::Int(1, 65535);

const fn d(name: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Direct, args, doc }
}
const fn a(name: &'static str, asks: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Approval(asks), args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("setup_load", &[("section", Arg::OneOf(SECTIONS), true)], "Reads a Setup tab's data from the daemon again (`secrets`, `hosts`, `voice`)."),
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

pub(crate) fn list<'a>(v: &'a J, key: &str) -> &'a [J] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub(crate) fn str_of<'a>(v: &'a J, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}

/// A word as a POSIX shell reads it (the provision command, the host command
/// shown before a stdio MCP server is added).
pub fn shell_word(w: &str) -> String {
    if !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./~-".contains(c)) {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
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

pub(crate) fn strings(v: &J) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

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
        "setup_load" => {
            // The tab shown last opens next time, across runs too.
            let mut v = refresh(section_keys(q.req("section")));
            v.push(Action::Remember { key: "tab".into(), value: J::String(q.req("section").to_string()) });
            Ok(v)
        }

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

/// Settings in the rail (Setup's own entry). The tab shown last opens next
/// time, if it is still one of Setup's (MCP, Kits and Skills left it).
fn extras(ctx: &Ctx) -> Vec<Extra> {
    vec![Extra {
        place: Place::Topbar,
        id: "setup".into(),
        label: String::new(),
        icon: Some("gear".into()),
        tone: "muted".into(),
        help: "Setup: secrets and sign-in, remote hosts, voice".into(),
        count: None,
        intent: Some(("open_modal".into(), match ctx.get("app")["kept"]["setup"]["tab"].as_str().filter(|t| SECTIONS.contains(t)) {
            Some(t) => json!({ "screen": "setup", "arg": t }),
            None => json!({ "screen": "setup" }),
        })),
    }]
}

#[cfg(test)]
pub(crate) mod tests;
