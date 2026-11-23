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
    assert!(t.iter().any(|x| x.starts_with("Opened for web-api, which is not running: Load acts on web-app")), "{t:?}");
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


/// The picker, the notice and every "Load into X" come from one sandbox: the
/// one picked (or, opened for a stopped one, the first running one).
#[test]
fn the_picker_the_notice_and_load_name_the_same_sandbox() {
    for (view, want) in [(json!({ "sandbox": "docs" }), "docs"), (json!({ "sandbox": "web-api" }), "web-app"), (json!({ "sandbox": "web-app" }), "web-app")] {
        let extra = json!({ "mcp_view": view, "mcp_inspect_for": "linear", "mcp_inspect": { "text": "x" } });
        let (t, tree) = render_panel("mcp", SRC, "full", extra.clone());
        fn find<'a>(n: &'a crate::luau::Node, id: &str) -> Option<&'a crate::luau::Node> {
            if n.id.as_deref() == Some(id) {
                return Some(n);
            }
            n.children.iter().find_map(|c| find(c, id))
        }
        assert_eq!(find(&tree, "mcp-sandbox").unwrap().value, json!(want), "{view}: the picker");
        assert!(t.iter().any(|x| x == &format!("Load into {want}")), "{view}: the details' Load: {t:?}");
        assert_eq!(press("mcp", SRC, "full", extra.clone(), "mcp-load-linear")[0]["args"]["sandbox"], want, "{view}: a row's Load");
        assert_eq!(press("mcp", SRC, "full", extra, "mcp-detail-load")[0]["args"]["sandbox"], want, "{view}: the details' Load");
        for line in t.iter().filter(|x| x.starts_with("Opened for")) {
            assert!(line.contains(&format!("Load acts on {want}")), "{line}");
        }
    }
}

// ── One column (the form on top, the servers under it) ───────────────────

mod window {
    use crate::shell::{view, App, Args};
    use rune::Runtime;
    use serde_json::json;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/basic.json");

    fn settle(rt: &mut Runtime<App>) {
        for _ in 0..12 {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    }

    fn window(w: f32, h: f32) -> Runtime<App> {
        let a = App::new(Args { fixture: Some(FIXTURE.into()), ..Args::default() });
        let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        settle(&mut rt);
        if w < 700.0 {
            rt.update(|s, _| s.toggle_sidebar());
            settle(&mut rt);
        }
        let v = json!({ "kind": "intent", "name": "mcp_show", "args": {} });
        for i in crate::intent::parse(&v).unwrap() {
            rt.update(|s, _| s.run_intent("test", i));
        }
        settle(&mut rt);
        rt
    }

    #[test]
    fn the_form_is_above_the_servers_and_nothing_leaves_the_overlay() {
        for (w, h) in [(2000.0, 1000.0), (1284.0, 816.0), (720.0, 480.0), (500.0, 700.0), (420.0, 700.0)] {
            let mut rt = window(w, h);
            let (o, _) = rt.presented("overlay-mcp").unwrap_or_else(|| panic!("no overlay at {w}"));
            let (f, _) = rt.presented("mcp-form").unwrap_or_else(|| panic!("no form at {w}"));
            let (l, _) = rt.presented("mcp-linear").unwrap_or_else(|| panic!("no server row at {w}"));
            assert!(f[1] + f[3] <= l[1] + 0.5, "at {w}: the form {f:?} above the list {l:?}");
            assert!(f[2] >= o[2] - 64.0 && l[2] >= o[2] - 64.0, "at {w}: full width: form {f:?}, row {l:?} in {o:?}");
            let mut bad = vec![];
            for e in rt.elements() {
                let Some(k) = e.key.clone() else { continue };
                if !k.starts_with("mcp-") {
                    continue;
                }
                let r = e.rect;
                if r[1] + r[3] <= o[1] || r[1] >= o[1] + o[3] || r[2] <= 0.0 {
                    continue;
                }
                if r[0] < o[0] - 0.5 || r[0] + r[2] > o[0] + o[2] + 0.5 {
                    bad.push(format!("{k} {r:?}"));
                }
            }
            assert!(bad.is_empty(), "at {w}, outside {o:?}:\n{}", bad.join("\n"));
            // Every action of a row is drawn (and reaches its intent: the
            // tree tests above press them).
            for k in ["mcp-load-linear", "mcp-unload-linear", "mcp-auth-linear", "mcp-inspect-linear", "mcp-rm-linear", "mcp-save"] {
                assert!(rt.presented(k).is_some(), "at {w}: {k}");
            }
        }
    }

    #[test]
    fn inspect_opens_under_its_row() {
        let mut rt = window(1284.0, 816.0);
        let (r, _) = rt.presented("mcp-sel-linear").unwrap();
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        let (d, _) = rt.presented("mcp-inspect").expect("the details");
        let (row, _) = rt.presented("mcp-sel-linear").unwrap();
        assert!(d[1] >= row[1] + row[3] - 0.5, "under its row: {d:?} under {row:?}");
        let (p, _) = rt.presented("mcp-playwright").unwrap();
        assert!(p[1] >= d[1] + d[3] - 0.5, "the next server under the details");
    }
}

// ---- sbx 0.54: each sandbox's MCP mode and the servers connected now ----

/// The stand-in's answer to `GET /api/sandboxes/:name/mcp`.
fn mode_of(data: &mut J, sb: &str) -> J {
    let mut publish = |_: &str, _: J| {};
    let mut st = crate::feat::Standin { data, publish: &mut publish };
    crate::feat::setup::standin::answer(&mut st, "GET", &format!("/api/sandboxes/{sb}/mcp"), &J::Null).unwrap().1
}

#[test]
fn the_panel_shows_static_dynamic_and_unknown_and_what_is_connected() {
    let mut d = fixture("full");
    d["mcp_mode_of"] = json!({ "docs": { "ok": true, "supported": true, "mode": "dynamic", "servers": [] } });
    // The stand-in: static by default, the fixture's dynamic, an older sbx unknown.
    let web = mode_of(&mut d, "web-app");
    assert_eq!((web["mode"].clone(), web["supported"].clone()), (json!("static"), json!(true)));
    // Loading into a dynamic sandbox connects the server; the read follows the load.
    let v = run(&d, "mcp_load", json!({ "server": "linear", "sandbox": "docs" })).unwrap();
    let Action::Call(c) = &v[0] else { panic!() };
    assert!(c.then.iter().any(|k| k == "mcp_mode"), "{:?}", c.then);
    let mut publish = |_: &str, _: J| {};
    let mut st = crate::feat::Standin { data: &mut d, publish: &mut publish };
    crate::feat::setup::standin::answer(&mut st, "POST", &c.path, &c.body).unwrap();
    let docs = mode_of(&mut d, "docs");
    assert_eq!(docs["servers"], json!(["linear"]));
    let mut old = d.clone();
    old["standin_sbx_old"] = true.into();
    let unknown = mode_of(&mut old, "web-app");
    assert_eq!((unknown["supported"].clone(), unknown["mode"].clone()), (json!(false), json!("unknown")));

    // Opening reads every running sandbox's mode; picking one not read yet reads again.
    let v = run(&d, "mcp_show", json!({})).unwrap();
    assert!(v.contains(&Action::Refresh("mcp_mode")));
    assert!(run(&d, "mcp_pick", json!({ "sandbox": "docs" })).unwrap().contains(&Action::Refresh("mcp_mode")));
    let mut read = d.clone();
    read["mcp_mode"] = json!({ "docs": docs.clone() });
    assert!(!run(&read, "mcp_pick", json!({ "sandbox": "docs" })).unwrap().contains(&Action::Refresh("mcp_mode")));

    // The panel: one row per running sandbox, its mode and what is connected.
    let (t, _) = render_panel("mcp", SRC, "full", json!({ "mcp_view": { "sandbox": "docs" }, "mcp_mode": { "web-app": web, "docs": docs } }));
    let all = t.join("\n");
    assert!(all.contains("mcp-mode-web-app") && all.contains("mcp-mode-docs"), "{all}");
    assert!(t.iter().any(|s| s == "static") && t.iter().any(|s| s == "dynamic"), "{all}");
    assert!(all.contains("no server connected") && all.contains("mcp-on-linear") && all.contains("connected in docs") && !all.contains("mcp-on-playwright"), "{all}");
    // Unknown: an older sbx's note, and a failed read, muted, never red.
    let (t, tree) = render_panel("mcp", SRC, "full", json!({ "mcp_mode": { "web-app": unknown, "docs": { "ok": true, "supported": true, "mode": "unknown", "servers": [] } } }));
    let all = t.join("\n");
    assert!(all.contains("needs sbx 0.54") && t.iter().filter(|s| *s == "unknown").count() == 2, "{all}");
    fn find<'a>(n: &'a crate::luau::Node, k: &str) -> Option<&'a crate::luau::Node> {
        if n.key.as_deref() == Some(k) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, k))
    }
    for sb in ["web-app", "docs"] {
        assert_eq!(find(&tree, &format!("mcp-mode-word-{sb}")).and_then(|b| b.variant.clone()).as_deref(), Some("neutral"), "{sb}: unknown is quiet, not an error");
    }
}
