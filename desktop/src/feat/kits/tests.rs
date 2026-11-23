//! Its intents, compared with Setup's former tab tests (moved here as they
//! were: same intents, same arguments, same dialogs), and its panel.

use super::*;
use crate::feat::setup::tests::{asked, fixture, one_confirm, press, render_panel};

fn run(data: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    act(&Ctx { data }, &asked(name, args))
}

const SRC: &str = include_str!("../../../screens/kits.luau");

#[test]
fn opened_for_a_sandbox_it_is_on_the_file_that_sandbox_uses() {
    let data = fixture("full");
    let c = one_confirm(run(&data, "kits_save", json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:codemap\nsbxw:dev-ports" })));
    assert!(c.ack.is_some());
    assert!(matches!(INTENTS.iter().find(|s| s.name == "kits_save").unwrap().risk, Risk::Approval(_)));
    for n in ["kits_show", "kits_pick"] {
        assert!(matches!(INTENTS.iter().find(|s| s.name == n).unwrap().risk, Risk::Direct));
    }
    let v = run(&data, "kits_show", json!({ "sandbox": "docs" })).unwrap();
    assert!(v.contains(&Action::Set { key: "kits_view".into(), value: json!({ "sandbox": "docs", "path": "/Users/ana/dev/docs/sbxw.toml" }) }), "{v:?}");
    assert!(v.contains(&Action::Remember { key: "path".into(), value: json!("/Users/ana/dev/docs/sbxw.toml") }));
    assert!(v.contains(&Action::OpenPanel("kits".into())));
    assert!(run(&data, "kits_pick", json!({ "path": "/etc/passwd" })).unwrap_err().contains("not an sbxw.toml"));

    // The panel follows the view, then what was kept, then the daemon's file.
    let file = |extra: J| {
        let (_, tree) = render_panel("kits", SRC, "full", extra);
        fn sel(n: &crate::luau::Node) -> Option<J> {
            if n.id.as_deref() == Some("kits-file") {
                return Some(n.value.clone());
            }
            n.children.iter().find_map(sel)
        }
        sel(&tree).unwrap()
    };
    assert_eq!(file(json!({ "kits_view": { "sandbox": "docs" } })), json!("/Users/ana/dev/docs/sbxw.toml"));
    assert_eq!(file(json!({ "kept": { "kits": { "sandbox": "docs" } } })), json!("/Users/ana/dev/docs/sbxw.toml"));
    assert_eq!(file(json!({})), json!("/Users/ana/dev/web/sbxw.toml"));
}

#[test]
fn every_former_action_is_on_the_panel_with_the_same_intent() {
    let p = |key: &str| press("kits", SRC, "full", json!({}), key);
    assert_eq!(p("kit-answer-docs")[0], json!({ "name": "open_terminal", "args": { "sandbox": "docs", "mode": "kit-add", "pane": "spare" } }));
    assert_eq!(p("kits-defaults")[0], json!({ "name": "kits_save", "args": { "path": "/Users/ana/dev/web/sbxw.toml", "defaults": true } }));
    assert_eq!(p("kits-file")[0]["name"], "kits_pick");
    // Toggle then save, through the host: the same `kits_save` the tab sent.
    let fx = crate::fixture::Fixture::load(&crate::fixture::dir().join("setup/full.json")).unwrap();
    let data = fx.screen_data(&crate::fixture::AppView::default());
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load("kits", SRC).unwrap();
    h.render(&data).unwrap();
    let cur = json!(["sbxw:codemap", "/Users/ana/.sbxw/state/kits/jarvis-notes"]);
    h.act("toggle", &json!({ "ref": "sbxw:ssh-agent", "path": "/Users/ana/dev/web/sbxw.toml", "list": cur }), &J::Bool(true)).unwrap();
    let (t, _) = { let tree = h.render(&data).unwrap(); let mut o = vec![]; crate::feat::setup::tests::texts(&tree, &mut o); (o, tree) };
    assert!(t.iter().any(|x| x == "unsaved"));
    let asks = h.act("save", &J::Null, &J::Null).unwrap();
    assert_eq!(asks["name"], "kits_save");
    assert_eq!(asks["args"], json!({ "path": "/Users/ana/dev/web/sbxw.toml", "kits": "sbxw:codemap\n/Users/ana/.sbxw/state/kits/jarvis-notes\nsbxw:ssh-agent" }));
    let c = one_confirm(run(&fixture("full"), "kits_save", asks["args"].clone()));
    assert!(c.ack.as_deref().unwrap().contains("sbxw:ssh-agent"), "still acknowledged in the core's dialog");
}

#[test]
fn waiting_kits_are_the_rail_badge_and_the_sandbox_button() {
    let mut data = fixture("full");
    let ex = extras(&Ctx { data: &data });
    let r = &ex[0];
    assert_eq!((r.id.as_str(), r.icon.as_deref(), r.count, r.tone.as_str()), ("kits", Some("box"), Some(1), "amber"));
    assert_eq!(r.intent.as_ref().unwrap(), &("open_panel".to_string(), json!({ "screen": "kits" })));
    let k = ex.iter().find(|e| e.place == Place::Sandbox("docs".into())).unwrap();
    assert_eq!((k.id.as_str(), k.intent.as_ref().unwrap().1.clone()), ("kit-add", json!({ "sandbox": "docs", "mode": "kit-add", "pane": "spare" })));
    for e in &ex {
        let (n, a) = e.intent.as_ref().unwrap();
        crate::intent::parse(&json!({ "kind": "intent", "name": n, "args": a })).unwrap();
    }
    // Nothing waits: no badge, the calm tone.
    for s in data["sandboxes"].as_array_mut().unwrap() {
        s["kits_waiting"] = json!(0);
    }
    let r = rail_entry(&Ctx { data: &data });
    assert_eq!((r.count, r.tone.as_str()), (None, "neutral"));
    let (t, _) = render_panel("kits", SRC, "full", json!({}));
    assert!(t.iter().any(|x| x == "Waiting for you"));
}

#[test]
fn empty_and_failed_say_what_to_do() {
    let (t, _) = render_panel("kits", SRC, "empty", json!({}));
    assert!(t.iter().any(|x| x.starts_with("No sbxw.toml to show")), "{t:?}");
    let (t, _) = render_panel("kits", SRC, "errors", json!({}));
    assert!(t.iter().any(|x| x.starts_with("Cannot be edited")), "{t:?}");
    let (t, _) = render_panel("kits", SRC, "full", json!({ "kits": { "ok": false, "error": "sbxw.toml unreadable" } }));
    assert!(t.iter().any(|x| x == "kits-error") && t.iter().any(|x| x == "sbxw.toml unreadable"), "{t:?}");
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


/// The picker at its top: a sandbox puts the view on the file it uses.
#[test]
fn the_sandbox_picker_puts_it_on_that_sandbox_s_file() {
    let data = fixture("full");
    let v = run(&data, "kits_pick", json!({ "sandbox": "docs" })).unwrap();
    assert!(v.contains(&Action::Set { key: "kits_view".into(), value: json!({ "sandbox": "docs", "path": "/Users/ana/dev/docs/sbxw.toml" }) }), "{v:?}");
    assert!(run(&data, "kits_pick", json!({ "sandbox": "nope" })).unwrap_err().contains("no sandbox"));
    assert!(run(&data, "kits_pick", json!({})).is_err());
    assert_eq!(press("kits", SRC, "full", json!({}), "kits-sandbox").len(), 1);
}

// ---- sbx 0.54: kit installs that failed on hosts sbx denied ----

#[test]
fn a_failed_kit_install_lists_its_denied_hosts_with_allow_and_retry() {
    let failure = |scope: &str, allowed: bool| json!({ "create": { "denied": [{
        "sandbox": "web-app", "step": "kits", "kit": "sbxw:agent-skills", "source": "sbx", "allow_scope": scope, "can_retry": true, "left": if allowed { 1 } else { 2 },
        "message": "`sbx kit add web-app sbxw:agent-skills` exited with status 1",
        "hosts": [{ "host": "registry.npmjs.org", "allowed": allowed }, { "host": "github.com", "allowed": false }] }] } });
    let (t, _) = render_panel("kits", SRC, "full", failure("sandbox", false));
    let all = t.join("\n");
    assert!(all.contains("web-app: sbx denied 2 hosts") && all.contains("Installing sbxw:agent-skills failed"), "{all}");
    assert!(all.contains("`sbx kit add web-app sbxw:agent-skills` exited with status 1"), "sbx's words, as text: {all}");
    assert_eq!(t.iter().filter(|x| *x == "Allow for this sandbox").count(), 2, "{all}");
    assert!(!all.contains("Allow for all sandboxes"));
    // The allow is network's existing intent, scoped to the sandbox.
    let a = press("kits", SRC, "full", failure("sandbox", false), "denied-allow-web-app-github.com");
    assert_eq!(a, vec![json!({ "name": "network_allow", "args": { "sandbox": "web-app", "resources": "github.com", "global": false } })]);
    // One allowed: said so, the other still offered; Retry is create's.
    let (t, _) = render_panel("kits", SRC, "full", failure("sandbox", true));
    let all = t.join("\n");
    assert!(!all.contains("denied-allow-web-app-registry.npmjs.org") && all.contains("allowed") && all.contains("denied-allow-web-app-github.com"), "{all}");
    let a = press("kits", SRC, "full", failure("sandbox", true), "kits-denied-retry-web-app");
    assert_eq!(a, vec![json!({ "name": "create_retry", "args": { "sandbox": "web-app" } })]);
    let a = press("kits", SRC, "full", failure("sandbox", true), "kits-denied-dismiss-web-app");
    assert_eq!(a, vec![json!({ "name": "create_dismiss", "args": { "sandbox": "web-app" } })]);
    // A sandbox that does not exist: the rule can only be every sandbox's, and the button says so.
    let (t, _) = render_panel("kits", SRC, "full", failure("global", false));
    let all = t.join("\n");
    assert!(all.contains("Allow for all sandboxes") && !all.contains("Allow for this sandbox") && all.contains("every sandbox on this machine"), "{all}");
    let a = press("kits", SRC, "full", failure("global", false), "denied-allow-web-app-github.com");
    assert_eq!(a[0]["args"]["global"], true);
    // Nothing failed: no card.
    let (t, _) = render_panel("kits", SRC, "full", json!({}));
    assert!(!t.iter().any(|x| x.starts_with("kits-denied-")));
}
