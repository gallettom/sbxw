//! Its intents, compared with Setup's former tab tests (moved here as they
//! were: same intents, same arguments, same dialogs), and its panel.

use super::*;
use crate::feat::setup::tests::{asked, fixture, one_confirm, press, render_panel};
use crate::feat::Method;

fn run(data: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    act(&Ctx { data }, &asked(name, args))
}

const SRC: &str = include_str!("../../../screens/mcp.luau");

#[test]
fn every_approval_intent_answers_one_confirm() {
    let data = fixture("full");
    for (name, args) in [
        ("mcp_unload", json!({ "server": "linear", "sandbox": "web-app" })),
        ("mcp_rm", json!({ "server": "linear" })),
        ("mcp_save", json!({ "name": "pw", "kind": "stdio", "command": "npx", "args": "-y @playwright/mcp@latest" })),
    ] {
        let spec = INTENTS.iter().find(|s| s.name == name).unwrap();
        assert!(matches!(spec.risk, Risk::Approval(_)), "{name} must be risk Approval");
        one_confirm(run(&data, name, args));
    }
    // The new ones only open the view.
    for n in ["mcp_show", "mcp_pick"] {
        assert!(matches!(INTENTS.iter().find(|s| s.name == n).unwrap().risk, Risk::Direct));
    }
}

#[test]
fn calls_go_where_the_web_ui_sends_them() {
    let data = fixture("full");
    let v = run(&data, "mcp_load", json!({ "server": "linear", "sandbox": "web-app" })).unwrap();
    let Action::Call(c) = &v[0] else { panic!() };
    assert_eq!((c.method, c.path.as_str()), (Method::Post, "/api/sandboxes/web-app/mcp/load"));
    assert_eq!(c.body, json!({ "server": "linear" }));
    assert!(run(&data, "mcp_load", json!({ "server": "linear", "sandbox": "web-api" })).unwrap_err().contains("not running"));
    let c = one_confirm(run(&data, "mcp_unload", json!({ "server": "linear", "sandbox": "web-app" })));
    assert_eq!(c.call.path, "/api/sandboxes/web-app/mcp/unload");
    let v = run(&data, "mcp_auth", json!({ "server": "linear" })).unwrap();
    assert!(matches!(&v[1], Action::Call(c) if c.path == "/api/mcp/linear/auth"));
    let v = run(&data, "mcp_inspect", json!({ "server": "linear" })).unwrap();
    assert!(matches!(&v[1], Action::Call(c) if c.path == "/api/mcp/linear" && c.method == Method::Get));
    let c = one_confirm(run(&data, "mcp_rm", json!({ "server": "linear" })));
    assert!(c.danger && c.call.path == "/api/mcp/linear/rm");
}

#[test]
fn opened_for_a_sandbox_it_is_on_that_sandbox_and_remembers_it() {
    let data = fixture("full");
    let v = run(&data, "mcp_show", json!({ "sandbox": "docs" })).unwrap();
    assert!(v.contains(&Action::Set { key: "mcp_view".into(), value: json!({ "sandbox": "docs" }) }));
    assert!(v.contains(&Action::Remember { key: "sandbox".into(), value: json!("docs") }));
    assert!(v.contains(&Action::OpenPanel("mcp".into())) && v.contains(&Action::Refresh("mcp")));
    // From the rail: no view, the last sandbox used.
    let v = run(&data, "mcp_show", json!({})).unwrap();
    assert!(v.contains(&Action::Set { key: "mcp_view".into(), value: J::Null }));
    assert!(run(&data, "mcp_pick", json!({ "sandbox": "nope" })).unwrap_err().contains("no sandbox"));

    // The panel: on docs when its view says so, on web-app (the last used) when not.
    let load = |extra: J| press("mcp", SRC, "full", extra, "mcp-load-linear");
    assert_eq!(load(json!({ "mcp_view": { "sandbox": "docs" } })), vec![json!({ "name": "mcp_load", "args": { "server": "linear", "sandbox": "docs" } })]);
    assert_eq!(load(json!({ "kept": { "mcp": { "sandbox": "docs" } } })), vec![json!({ "name": "mcp_load", "args": { "server": "linear", "sandbox": "docs" } })]);
    assert_eq!(load(json!({})), vec![json!({ "name": "mcp_load", "args": { "server": "linear", "sandbox": "web-app" } })]);
    // A stopped sandbox: said, and the first running one is used.
    let (t, _) = render_panel("mcp", SRC, "full", json!({ "mcp_view": { "sandbox": "web-api" } }));
    assert!(t.iter().any(|x| x.starts_with("web-api is not running")), "{t:?}");
}

#[test]
fn every_former_action_is_on_the_panel_with_the_same_intent() {
    let p = |key: &str| press("mcp", SRC, "full", json!({}), key);
    assert_eq!(p("mcp-unload-linear")[0], json!({ "name": "mcp_unload", "args": { "server": "linear", "sandbox": "web-app" } }));
    assert_eq!(p("mcp-auth-linear")[0], json!({ "name": "mcp_auth", "args": { "server": "linear" } }));
    assert_eq!(p("mcp-inspect-linear")[0], json!({ "name": "mcp_inspect", "args": { "server": "linear" } }));
    assert_eq!(p("mcp-rm-playwright")[0], json!({ "name": "mcp_rm", "args": { "server": "playwright" } }));
    assert_eq!(p("mcp-sandbox").len(), 1);
    let (t, _) = render_panel("mcp", SRC, "full", json!({}));
    assert!(!t.iter().any(|x| x == "mcp-auth-playwright"), "no Sign in for a local server");
    assert!(t.iter().any(|x| x == "Add…") && t.iter().any(|x| x == "local command"));
}

#[test]
fn empty_and_failed_say_what_to_do() {
    let (t, _) = render_panel("mcp", SRC, "empty", json!({}));
    assert!(t.iter().any(|x| x.starts_with("No server registered yet")), "{t:?}");
    let (t, _) = render_panel("mcp", SRC, "errors", json!({}));
    assert!(t.iter().any(|x| x == "mcp-error"), "{t:?}");
}

#[test]
fn a_long_list_is_paged() {
    let servers: Vec<J> = (0..60).map(|i| json!({ "name": format!("s{i}"), "type": "remote" })).collect();
    let (t, _) = render_panel("mcp", SRC, "full", json!({ "mcp": { "ok": true, "servers": servers } }));
    assert!(t.iter().any(|x| x == "mcp-s24") && !t.iter().any(|x| x == "mcp-s25"));
    assert!(t.iter().any(|x| x == "Show 25 more"));
}

#[test]
fn its_rail_entry_and_pane_entries() {
    let data = fixture("full");
    let ex = extras(&Ctx { data: &data });
    let r = &ex[0];
    assert_eq!((r.id.as_str(), r.icon.as_deref(), r.count), ("mcp", Some("plug"), None));
    assert_eq!(r.intent.as_ref().unwrap().0, "open_panel");
    let panes: Vec<&Extra> = ex.iter().filter(|e| matches!(e.place, Place::Pane(_))).collect();
    assert_eq!(panes.len(), 2, "running sandboxes only");
    for e in &ex {
        let (n, a) = e.intent.as_ref().unwrap();
        crate::intent::parse(&json!({ "kind": "intent", "name": n, "args": a })).unwrap();
    }
}

#[test]
fn a_header_secret_is_named_never_valued() {
    // The form names ${TOKEN}; the value is the core dialog's. The rows take no value.
    let e = crate::intent::parse(&json!({ "kind": "intent", "name": "mcp_save", "args": { "name": "x", "kind": "remote", "url": "https://x", "secrets": { "TOKEN": "v" } } })).unwrap_err();
    assert!(e.contains("unknown argument"), "{e}");
    for f in ["full", "empty", "errors", "looked"] {
        let (t, _) = render_panel("mcp", SRC, f, json!({}));
        let all = t.join("\n");
        assert!(!all.contains("ghp_") && !all.contains("sk-") && !all.contains("tok-"), "{f}: a value reached the screen");
    }
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

