use super::*;
use crate::feat::{Method, Standin};
use serde_json::Map;

fn fixture(name: &str) -> J {
    let p = crate::fixture::dir().join("setup").join(format!("{name}.json"));
    let f: J = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    f["daemon"].clone()
}

fn asked(name: &'static str, args: J) -> Asked {
    let args: Map<String, J> = args.as_object().cloned().unwrap_or_default();
    // Checked as the core checks it, against this feature's own row.
    let spec = INTENTS.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no row {name}"));
    let checked = crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(checked.len(), 1);
    let _ = spec;
    Asked { name, args }
}

fn one_confirm(r: Result<Vec<Action>, String>) -> Confirm {
    let mut v = r.unwrap();
    assert_eq!(v.len(), 1, "{v:?}");
    match v.remove(0) {
        Action::Confirm(c) => *c,
        a => panic!("not a confirm: {a:?}"),
    }
}

fn run(data: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    act(&Ctx { data }, &asked(name, args))
}

#[test]
fn every_approval_intent_answers_one_confirm_or_prepare() {
    let data = fixture("full");
    let cases: Vec<(&'static str, J)> = vec![
        ("github_token_set", json!({})),
        ("github_token_gh", json!({})),
        ("github_token_rm", json!({})),
        ("secret_custom_set", json!({ "hosts": "api.example.com, *.example.io", "env": "EXAMPLE_KEY" })),
        ("secret_rm", json!({ "name": "OPENAI_KEY" })),
        ("kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:codemap\nsbxw:dev-ports" })),
        ("mcp_unload", json!({ "server": "linear", "sandbox": "web-app" })),
        ("mcp_rm", json!({ "server": "linear" })),
        ("mcp_save", json!({ "name": "pw", "kind": "stdio", "command": "npx", "args": "-y @playwright/mcp@latest" })),
        ("skill_forget", json!({ "name": "release-notes" })),
        ("host_rm", json!({ "name": "build-box" })),
        ("host_save", json!({ "name": "gpu", "host": "10.0.0.9" })),
        ("voice_install", json!({})),
        ("voice_rm", json!({})),
    ];
    for (name, args) in cases {
        let spec = INTENTS.iter().find(|s| s.name == name).unwrap();
        assert!(matches!(spec.risk, Risk::Approval(_)), "{name} must be risk Approval");
        let v = run(&data, name, args).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(v.len(), 1, "{name}: {v:?}");
        assert!(matches!(v[0], Action::Confirm(_) | Action::Prepare(_)), "{name}: {v:?}");
    }
}

#[test]
fn every_intent_row_is_documented_and_unique() {
    for s in INTENTS {
        assert!(!s.doc.is_empty(), "{}", s.name);
        assert_eq!(INTENTS.iter().filter(|x| x.name == s.name).count(), 1, "{}", s.name);
        assert!(crate::intent::REGISTRY.iter().all(|x| x.name != s.name), "{} is a core intent", s.name);
    }
}

#[test]
fn a_secret_is_asked_by_the_core_and_never_in_the_request_built_from_the_screen() {
    let data = fixture("full");
    let c = one_confirm(run(&data, "github_token_set", json!({})));
    assert_eq!(c.secrets.len(), 1);
    assert_eq!(c.secrets[0].field, "token");
    assert_eq!(c.call.path, "/api/github-token");
    assert_eq!(c.call.body, json!({}), "the body holds no token until the core's field is confirmed");

    let c = one_confirm(run(&data, "secret_custom_set", json!({ "hosts": "api.example.com *.example.io", "env": "EXAMPLE_KEY" })));
    assert_eq!(c.secrets[0].field, "value");
    assert_eq!(c.call.body, json!({ "hosts": ["api.example.com", "*.example.io"], "env": "EXAMPLE_KEY" }));

    // A screen cannot slip a value in: the rows have no argument for one.
    let e = crate::intent::parse(&json!({ "kind": "intent", "name": "github_token_set", "args": { "token": "ghp_x" } })).unwrap_err();
    assert!(e.contains("unknown argument 'token'"), "{e}");
    let e = crate::intent::parse(&json!({ "kind": "intent", "name": "secret_custom_set", "args": { "hosts": "a.b", "env": "X", "value": "v" } })).unwrap_err();
    assert!(e.contains("unknown argument 'value'"), "{e}");
}

#[test]
fn custom_secret_rules_are_the_daemons() {
    let data = fixture("full");
    assert!(run(&data, "secret_custom_set", json!({ "hosts": "https://api.x.com/v1", "env": "K" })).unwrap_err().contains("not a host"));
    assert!(run(&data, "secret_custom_set", json!({ "hosts": "api.x.com", "env": "1BAD" })).unwrap_err().contains("variable name"));
    assert!(run(&data, "secret_rm", json!({ "name": "NOPE" })).unwrap_err().contains("no secret named"));
    assert!(run(&data, "secret_rm", json!({ "name": "SCOPED_KEY" })).unwrap_err().contains("one sandbox"));
}

#[test]
fn a_stdio_server_shows_its_host_command_and_needs_the_acknowledgement() {
    let data = fixture("full");
    let c = one_confirm(run(&data, "mcp_save", json!({ "name": "fs", "kind": "stdio", "command": "npx", "args": "-y @mcp/fs /srv/my data", "dir": "/srv" })));
    assert_eq!(c.preview.as_ref().unwrap().text, "cd /srv && npx -y @mcp/fs /srv/my data");
    assert_eq!(c.ack.as_deref(), Some("I have read this command"));
    assert_eq!(c.call.body["spec"]["kind"], "stdio");
    assert_eq!(c.call.body["spec"]["args"], json!(["-y", "@mcp/fs", "/srv/my", "data"]));
    assert_eq!(host_command("my tool", &["it's".into()], ""), "'my tool' 'it'\\''s'");
}

#[test]
fn a_remote_server_asks_one_secret_per_placeholder() {
    let data = fixture("full");
    let c = one_confirm(run(
        &data,
        "mcp_save",
        json!({ "name": "linear2", "kind": "remote", "url": "https://mcp.linear.app/mcp", "headers": "Authorization: Bearer ${TOKEN}; X-Org: ${ORG}; X-Again: ${TOKEN}" }),
    ));
    let fields: Vec<&str> = c.secrets.iter().map(|s| s.field.as_str()).collect();
    assert_eq!(fields, ["secrets.TOKEN", "secrets.ORG"]);
    assert!(c.secrets.iter().all(|s| !s.optional));
    assert_eq!(c.call.body["secrets"], json!({}));
    assert!(c.ack.is_none());
    // An edit: empty means unchanged, and the dialog says the token is dropped.
    let c = one_confirm(run(&data, "mcp_save", json!({ "name": "linear", "kind": "remote", "url": "https://mcp.linear.app/mcp", "headers": "Authorization: Bearer ${TOKEN}", "replace": "linear" })));
    assert!(c.secrets[0].optional);
    assert!(c.lines.iter().any(|l| l.contains("drops its OAuth token")));
    assert!(run(&data, "mcp_save", json!({ "name": "x", "kind": "remote", "url": "ftp://x" })).unwrap_err().contains("http"));
    assert!(run(&data, "mcp_save", json!({ "name": "-x", "kind": "stdio", "command": "a" })).unwrap_err().contains("server name"));
}

#[test]
fn a_kits_line_change_lists_what_it_adds_and_asks_for_kits_that_reach_out() {
    let data = fixture("full");
    // Only bundled, harmless: no acknowledgement.
    let c = one_confirm(run(&data, "kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:codemap\n/Users/ana/.sbxw/state/kits/jarvis-notes\nsbxw:agent-skills" })));
    assert!(c.ack.is_none(), "{c:?}");
    assert!(c.lines.iter().any(|l| l == "Adds sbxw:agent-skills (bundled)."));
    assert_eq!(c.call.body, json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": ["sbxw:codemap", "/Users/ana/.sbxw/state/kits/jarvis-notes", "sbxw:agent-skills"] }));
    // A local kit, and a bundled one that reaches this machine: acknowledged.
    let c = one_confirm(run(&data, "kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:codemap\n./kits/mine\nsbxw:ssh-agent" })));
    assert!(c.ack.as_deref().unwrap().contains("./kits/mine, sbxw:ssh-agent"), "{c:?}");
    assert!(c.lines.iter().any(|l| l.starts_with("Removes /Users/ana/.sbxw/state/kits/jarvis-notes")), "{c:?}");
    assert!(c.preview.unwrap().diff);
    // Defaults: the line goes, `kits: null`.
    let c = one_confirm(run(&data, "kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "defaults": true })));
    assert_eq!(c.call.body["kits"], J::Null);
    // Unchanged, unknown file, jarvis: refused.
    assert!(run(&data, "kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:codemap\n/Users/ana/.sbxw/state/kits/jarvis-notes" })).unwrap_err().contains("unchanged"));
    assert!(run(&data, "kits_save", json!({ "path": "/etc/passwd", "kits": "x" })).unwrap_err().contains("not an sbxw.toml"));
    assert!(run(&data, "kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:jarvis" })).unwrap_err().contains("Jarvis"));
}

#[test]
fn keeping_a_skill_uses_the_cores_listing_not_the_screens_words() {
    let mut data = fixture("full");
    assert!(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "deploy" })).unwrap_err().contains("look at"));
    let v = run(&data, "skills_look", json!({ "sandbox": "web-app" })).unwrap();
    assert!(matches!(&v[2], Action::Fetch { key, path } if key == "sandbox_skills" && path == "/api/sandboxes/web-app/skills"));
    data["skills_for"] = json!("web-app");
    data["sandbox_skills"] = data["sandbox_skills_of"]["web-app"].clone();
    let c = one_confirm(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "deploy" })));
    assert_eq!(c.call.body, json!({ "origin": "project", "name": "deploy", "replace": false }));
    let c = one_confirm(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "release-notes" })));
    assert_eq!(c.call.body["replace"], true);
    assert!(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "Bad Name" })).unwrap_err().contains("cannot be kept"));
}

#[test]
fn a_new_host_is_confirmed_on_fingerprints_the_core_read() {
    let data = fixture("full");
    let v = run(&data, "host_save", json!({ "name": "gpu", "host": "10.0.0.9", "port": 2222, "user": "ana", "trust_secrets": true })).unwrap();
    let Action::Prepare(p) = &v[0] else { panic!("{v:?}") };
    assert_eq!(p.call.path, "/api/remote-hosts/key");
    assert_eq!(p.call.body["port"], 2222);
    let key = json!({ "ok": true, "known_as": "[10.0.0.9]:2222", "known": false, "fingerprints": ["256 SHA256:abc 10.0.0.9 (ED25519)"] });
    let c = (p.build)(&Ctx { data: &data }, &p.asked, &key).unwrap();
    assert_eq!(c.call.path, "/api/remote-hosts/add");
    assert_eq!(c.call.body["fingerprints"], json!(["256 SHA256:abc 10.0.0.9 (ED25519)"]));
    assert_eq!(c.preview.unwrap().text, "256 SHA256:abc 10.0.0.9 (ED25519)");
    assert!(c.ack.is_some());
    assert!(c.danger, "trusting a host with secrets is shown in red");
    // Already known: no fingerprints sent, nothing to tick.
    let c = (p.build)(&Ctx { data: &data }, &p.asked, &json!({ "ok": true, "known_as": "x", "known": true, "fingerprints": [] })).unwrap();
    assert_eq!(c.call.body["fingerprints"], J::Null);
    assert!(c.ack.is_none());
    // keyscan failed.
    assert!((p.build)(&Ctx { data: &data }, &p.asked, &json!({ "ok": false, "error": "timed out" })).unwrap_err().contains("timed out"));
    // An edit that keeps the address asks no key.
    let c = one_confirm(run(&data, "host_save", json!({ "name": "build-box", "ssh": "build-box", "editing": "build-box" })));
    assert_eq!(c.call.path, "/api/remote-hosts/build-box/edit");
    assert!(c.lines.iter().any(|l| l.contains("No longer trusted")));
    assert!(run(&data, "host_save", json!({ "name": "build-box", "ssh": "x" })).unwrap_err().contains("already registered"));
}

#[test]
fn removing_a_host_with_sandboxes_needs_the_tick_and_forces() {
    let data = fixture("full");
    let c = one_confirm(run(&data, "host_rm", json!({ "name": "build-box" })));
    assert_eq!(c.call.body, json!({ "force": true }));
    assert!(c.ack.is_some());
    let c = one_confirm(run(&data, "host_rm", json!({ "name": "spare" })));
    assert_eq!(c.call.body, json!({ "force": false }));
    assert!(c.ack.is_none());
}

#[test]
fn voice_events_replace_the_status_and_a_kit_waiting_gets_its_button() {
    let data = fixture("full");
    let v = on_event(&Ctx { data: &data }, "voice", &json!({ "ready": false, "job": { "state": "running" } }));
    assert!(matches!(&v[0], Action::Set { key, .. } if key == "voice"));
    let ex = extras(&Ctx { data: &data });
    assert!(ex.iter().any(|e| e.place == Place::Topbar && e.icon.as_deref() == Some("gear")));
    let k = ex.iter().find(|e| e.place == Place::Sandbox("docs".into())).unwrap();
    assert_eq!(k.intent.as_ref().unwrap().1, json!({ "sandbox": "docs", "mode": "kit-add", "pane": "spare" }));
    // The intent it asks is one the core knows.
    crate::intent::parse(&json!({ "kind": "intent", "name": "open_terminal", "args": k.intent.as_ref().unwrap().1 })).unwrap();
}

#[test]
fn calls_go_where_the_web_ui_sends_them() {
    let data = fixture("full");
    let v = run(&data, "mcp_load", json!({ "server": "linear", "sandbox": "web-app" })).unwrap();
    let Action::Call(c) = &v[0] else { panic!() };
    assert_eq!((c.method, c.path.as_str()), (Method::Post, "/api/sandboxes/web-app/mcp/load"));
    assert!(run(&data, "mcp_load", json!({ "server": "linear", "sandbox": "web-api" })).unwrap_err().contains("not running"));
    let v = run(&data, "setup_load", json!({ "section": "secrets" })).unwrap();
    assert_eq!(v, vec![Action::Refresh("secrets"), Action::Refresh("github_token")]);
    let v = run(&data, "host_provision", json!({ "name": "build-box" })).unwrap();
    assert!(matches!(&v[0], Action::Set { value, .. } if value["command"] == "/usr/local/bin/sbxw host provision build-box"));
}

// ---------------------------------------------------------------------------
// The stand-in

fn standin(data: &mut J, method: &str, path: &str, body: J) -> (u16, J, Vec<(String, J)>) {
    let mut events = Vec::new();
    let mut publish = |n: &str, v: J| events.push((n.to_string(), v));
    let mut st = Standin { data, publish: &mut publish };
    let (code, answer) = standin::answer(&mut st, method, path, &body).unwrap_or_else(|| panic!("{method} {path} not answered"));
    (code, answer, events)
}

#[test]
fn the_standin_never_keeps_a_secret_value() {
    let mut data = fixture("full");
    let token = "ghp_SECRETVALUE0123456789";
    let (code, _, _) = standin(&mut data, "POST", "/api/github-token", json!({ "token": token }));
    assert_eq!(code, 200);
    let (code, _, _) = standin(&mut data, "POST", "/api/secrets/custom", json!({ "hosts": ["api.x.com"], "env": "X_KEY", "value": "sk-OTHERSECRET" }));
    assert_eq!(code, 200);
    let (code, _, _) = standin(&mut data, "POST", "/api/mcp", json!({ "spec": { "name": "n", "kind": "remote", "url": "https://x" }, "replace": null, "secrets": { "TOKEN": "tok-THIRDSECRET" } }));
    assert_eq!(code, 200);
    let all = data.to_string();
    for v in [token, "sk-OTHERSECRET", "tok-THIRDSECRET"] {
        assert!(!all.contains(v), "the stand-in kept {v}");
    }
    assert_eq!(data["github_token"]["set"], true);
    assert!(data["secrets"]["secrets"].as_array().unwrap().iter().any(|x| x["name"] == "X_KEY"));
}

#[test]
fn the_standin_downloads_the_voice_in_steps() {
    let mut data = fixture("full");
    let (_, _, ev) = standin(&mut data, "POST", "/api/voice/install", json!({}));
    assert_eq!(ev[0].1["job"]["state"], "running");
    for _ in 0..4 {
        standin(&mut data, "GET", "/api/voice", J::Null);
    }
    assert_eq!(data["voice"]["ready"], true);
    assert_eq!(data["voice"]["job"]["state"], "done");
}

/// Each element's texts, keys and helps, depth first.
fn texts(n: &crate::luau::Node, out: &mut Vec<String>) {
    out.push(n.text.clone());
    out.extend(n.key.clone());
    out.extend(n.help.clone());
    for c in &n.children {
        texts(c, out);
    }
}

pub fn render_tab(fixture: &str, tab: &str) -> Vec<String> {
    let p = crate::fixture::dir().join("setup").join(format!("{fixture}.json"));
    let fx = crate::fixture::Fixture::load(&p).unwrap();
    let data = fx.screen_data(&crate::fixture::AppView::default());
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load("setup", include_str!("../../../screens/setup.luau")).unwrap();
    let asks = h.act("tab", &json!(tab), &J::Null).unwrap();
    assert_eq!(crate::intent::parse(&asks).unwrap().len(), 1);
    let tree = h.render(&data).unwrap_or_else(|e| panic!("{fixture}: {tab}: {e}"));
    let mut out = Vec::new();
    texts(&tree, &mut out);
    out
}

#[test]
fn every_fixture_renders_every_tab_without_a_value() {
    let dir = crate::fixture::dir().join("setup");
    for f in std::fs::read_dir(&dir).unwrap() {
        let name = f.unwrap().path().file_stem().unwrap().to_string_lossy().into_owned();
        for tab in SECTIONS {
            let all = render_tab(&name, tab).join("\n");
            assert!(!all.contains("ghp_") && !all.contains("sk-"), "{name}/{tab}: a value reached the screen");
        }
    }
}

#[test]
fn the_tabs_show_what_the_daemon_said() {
    let s = render_tab("full", "secrets").join("\n");
    assert!(s.contains("anthropic") && s.contains("OPENAI_KEY") && s.contains("token set"), "{s}");
    let k = render_tab("full", "kits").join("\n");
    assert!(k.contains("sbxw:codemap") && k.contains("Jarvis kit") && k.contains("Answer in a pane"), "{k}");
    let m = render_tab("full", "mcp").join("\n");
    assert!(m.contains("linear") && m.contains("Sign in"), "{m}");
    let h = render_tab("full", "hosts").join("\n");
    assert!(h.contains("build-box") && h.contains("trusted with secrets"), "{h}");
    let v = render_tab("voice-downloading", "voice").join("\n");
    assert!(v.contains("Downloading") && v.contains("Cancel"), "{v}");
}
