use super::*;
use crate::feat::Standin;
use serde_json::Map;

pub(crate) fn fixture(name: &str) -> J {
    let p = crate::fixture::dir().join("setup").join(format!("{name}.json"));
    let f: J = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    f["daemon"].clone()
}

pub(crate) fn asked(name: &'static str, args: J) -> Asked {
    let args: Map<String, J> = args.as_object().cloned().unwrap_or_default();
    // Checked as the core checks it, against the row of the feature it belongs to.
    let spec = crate::feat::intent(name).unwrap_or_else(|| panic!("no row {name}")).1;
    let checked = crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(checked.len(), 1);
    let _ = spec;
    Asked { name, args }
}

pub(crate) fn one_confirm(r: Result<Vec<Action>, String>) -> Confirm {
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
fn voice_events_replace_the_status_and_settings_keeps_only_its_tabs() {
    let mut data = fixture("full");
    let v = on_event(&Ctx { data: &data }, "voice", &json!({ "ready": false, "job": { "state": "running" } }));
    assert!(matches!(&v[0], Action::Set { key, .. } if key == "voice"));
    let ex = extras(&Ctx { data: &data });
    assert_eq!(ex.len(), 1, "Settings' entry only: the kit-add badge is the Kits feature's now");
    assert!(ex[0].place == Place::Topbar && ex[0].icon.as_deref() == Some("gear"));
    assert!(!ex[0].help.contains("kits") && !ex[0].help.contains("MCP") && !ex[0].help.contains("skills"), "{}", ex[0].help);
    // A tab kept from before the move (kits) opens Setup on its first tab.
    data["app"] = json!({ "kept": { "setup": { "tab": "kits" } } });
    assert_eq!(extras(&Ctx { data: &data })[0].intent.as_ref().unwrap().1, json!({ "screen": "setup" }));
    data["app"] = json!({ "kept": { "setup": { "tab": "hosts" } } });
    assert_eq!(extras(&Ctx { data: &data })[0].intent.as_ref().unwrap().1, json!({ "screen": "setup", "arg": "hosts" }));
}

#[test]
fn setup_no_longer_has_the_tabs_that_moved() {
    assert_eq!(SECTIONS, &["secrets", "hosts", "voice"]);
    for gone in ["kits", "skills", "mcp"] {
        assert!(crate::intent::parse(&json!({ "kind": "intent", "name": "setup_load", "args": { "section": gone } })).is_err(), "{gone}");
    }
    let src = include_str!("../../../screens/setup.luau");
    for gone in ["id = \"kits\"", "id = \"skills\"", "id = \"mcp\"", "kits_save", "mcp_save", "skill_keep"] {
        assert!(!src.contains(gone), "setup.luau still has {gone}");
    }
    // Nothing points at the old tabs any more.
    for (name, src) in crate::feat::FEATURES.iter().flat_map(|f| f.screens.iter()).map(|s| (s.name, s.src)) {
        for old in ["arg = \"kits\"", "arg = \"mcp\"", "arg = \"skills\"", "Kits in Setup", "Setup > Kits"] {
            assert!(!src.contains(old), "{name}.luau still points at Setup's old tab: {old}");
        }
    }
}

#[test]
fn calls_go_where_the_web_ui_sends_them() {
    let data = fixture("full");
    let v = run(&data, "setup_load", json!({ "section": "secrets" })).unwrap();
    assert_eq!(v, vec![Action::Refresh("secrets"), Action::Refresh("github_token"), Action::Remember { key: "tab".into(), value: J::String("secrets".into()) }], "the tab is kept for next time");
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
pub(crate) fn texts(n: &crate::luau::Node, out: &mut Vec<String>) {
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
    let h = render_tab("full", "hosts").join("\n");
    assert!(h.contains("build-box") && h.contains("trusted with secrets"), "{h}");
    let v = render_tab("voice-downloading", "voice").join("\n");
    assert!(v.contains("Downloading") && v.contains("Cancel"), "{v}");
}

/// Labels are sentence case (docs/screens.md, "No capitals"): the voice
/// tab's runtime row says "Runtime", not "RUNTIME".
#[test]
fn the_runtime_badge_is_sentence_case() {
    let v = render_tab("full", "voice");
    assert!(v.iter().any(|t| t == "Runtime"), "{v:?}");
    assert!(!v.iter().any(|t| t == "RUNTIME"), "{v:?}");
}

#[test]
fn every_setup_intent_is_described_in_the_screen_doc() {
    let doc = include_str!("../../../docs/screens.md");
    let section = &doc[doc.find("### setup").expect("a ### setup section")..];
    for s in INTENTS {
        assert!(section.contains(&format!("`{}`", s.name)), "docs/screens.md, ### setup, does not describe {}", s.name);
    }
}

/// A panel screen moved out of Setup (mcp, skills, kits), rendered from one
/// of Setup's fixtures, with extra keys set in `data` (a view, the kept
/// state): its texts, keys and helps, and the tree.
pub(crate) fn render_panel(screen: &str, src: &str, fixture: &str, extra: J) -> (Vec<String>, crate::luau::Node) {
    let p = crate::fixture::dir().join("setup").join(format!("{fixture}.json"));
    let fx = crate::fixture::Fixture::load(&p).unwrap();
    let mut data = fx.screen_data(&crate::fixture::AppView::default());
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        if k == "kept" {
            data["app"]["kept"] = v;
        } else {
            data[k] = v;
        }
    }
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load(screen, src).unwrap();
    let tree = h.render(&data).unwrap_or_else(|e| panic!("{screen} {fixture}: {e}"));
    let mut out = Vec::new();
    texts(&tree, &mut out);
    (out, tree)
}

/// What a press on the element of that key returns, each `{name, args}`
/// (checked as the core checks a screen's intents).
pub(crate) fn press(screen: &str, src: &str, fixture: &str, extra: J, key: &str) -> Vec<J> {
    let p = crate::fixture::dir().join("setup").join(format!("{fixture}.json"));
    let fx = crate::fixture::Fixture::load(&p).unwrap();
    let mut data = fx.screen_data(&crate::fixture::AppView::default());
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        if k == "kept" {
            data["app"]["kept"] = v;
        } else {
            data[k] = v;
        }
    }
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load(screen, src).unwrap();
    let tree = h.render(&data).unwrap();
    fn find<'a>(n: &'a crate::luau::Node, k: &str) -> Option<&'a crate::luau::Node> {
        if crate::screen::key_of(n).as_deref() == Some(k) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, k))
    }
    let n = find(&tree, key).unwrap_or_else(|| panic!("{screen}: no element {key}"));
    let on = n.on.clone().unwrap_or_else(|| panic!("{key} has no handler"));
    let asks = h.act(&on, &n.arg, &n.value).unwrap();
    crate::intent::parse(&asks).unwrap_or_else(|e| panic!("{key}: {e}"));
    let list = if asks.is_array() { asks.as_array().cloned().unwrap() } else { vec![asks] };
    list.into_iter().map(|a| json!({ "name": a["name"], "args": if a["args"].is_null() { json!({}) } else { a["args"].clone() } })).collect()
}

#[test]
fn every_intent_of_the_three_panels_is_described_in_the_screen_doc() {
    let doc = include_str!("../../../docs/screens.md");
    let section = &doc[doc.find("### mcp, skills, kits").expect("a ### mcp, skills, kits section")..];
    for s in crate::feat::mcp::INTENTS.iter().chain(crate::feat::skills::INTENTS).chain(crate::feat::kits::INTENTS) {
        assert!(section.contains(&format!("`{}`", s.name)), "docs/screens.md does not describe {}", s.name);
        assert!(!s.doc.is_empty());
    }
}
