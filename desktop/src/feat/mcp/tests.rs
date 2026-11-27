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

// ── Header rows: a name picked (or Custom) and a value each ──────────────

/// The panel held across presses: each press runs the screen's action on
/// its state, as the core does, and the next render shows the result.
struct Form {
    h: crate::luau::LuauHost,
    data: J,
}

impl Form {
    fn new(extra: J) -> Form {
        let p = crate::fixture::dir().join("setup").join("full.json");
        let fx = crate::fixture::Fixture::load(&p).unwrap();
        let mut data = fx.screen_data(&crate::fixture::AppView::default());
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            data[k] = v;
        }
        let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
        h.load("mcp", SRC).unwrap();
        Form { h, data }
    }
    fn tree(&self) -> crate::luau::Node {
        self.h.render(&self.data).unwrap()
    }
    fn texts(&self) -> Vec<String> {
        let mut out = vec![];
        crate::feat::setup::tests::texts(&self.tree(), &mut out);
        out
    }
    fn find(&self, key: &str) -> Option<crate::luau::Node> {
        fn find(n: &crate::luau::Node, k: &str) -> Option<crate::luau::Node> {
            if crate::screen::key_of(n).as_deref() == Some(k) {
                return Some(n.clone());
            }
            n.children.iter().find_map(|c| find(c, k))
        }
        find(&self.tree(), key)
    }
    /// Presses (or types `value` into) the element of that key; the intents it asks.
    fn act(&mut self, key: &str, value: J) -> Vec<J> {
        let n = self.find(key).unwrap_or_else(|| panic!("no element {key}: {:?}", self.texts()));
        let on = n.on.clone().unwrap_or_else(|| panic!("{key} has no handler"));
        let v = if value.is_null() { n.value.clone() } else { value };
        let asks = self.h.act(&on, &n.arg, &v).unwrap();
        if asks.is_null() {
            return vec![];
        }
        crate::intent::parse(&asks).unwrap_or_else(|e| panic!("{key}: {e}"));
        let list = if asks.is_array() { asks.as_array().cloned().unwrap() } else { vec![asks] };
        list.into_iter().map(|a| json!({ "name": a["name"], "args": a["args"].clone() })).collect()
    }
    fn press(&mut self, key: &str) -> Vec<J> {
        self.act(key, J::Null)
    }
    fn typed(&mut self, key: &str, text: &str) {
        assert!(self.act(key, json!(text)).is_empty(), "{key}: typing asks nothing");
    }
}

/// A remote server named `name` at a URL, ready for its headers.
fn remote_form(name: &str) -> Form {
    let mut f = Form::new(json!({}));
    f.typed("mcp-name", name);
    f.typed("mcp-url", "https://mcp.example.com/mcp");
    f
}

#[test]
fn no_header_row_by_default_only_add_header() {
    let f = remote_form("x");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "Add header") && t.iter().any(|x| x == "mcp-h-add"), "{t:?}");
    assert!(!t.iter().any(|x| x.starts_with("mcp-h-name-") || x == "mcp-h-note"), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("one per ;")), "the old free-text field is gone");
    assert!(t.iter().any(|x| x.starts_with("OAuth servers are added unauthorized")), "{t:?}");
}

#[test]
fn add_header_draws_a_row_with_its_name_and_value_and_remove_takes_it_away() {
    let mut f = remote_form("x");
    f.press("mcp-h-add");
    f.press("mcp-h-add");
    let name = f.find("mcp-h-name-1").expect("the first row's name");
    assert_eq!(name.value, json!("Authorization"));
    let value = f.find("mcp-h-value-1").expect("the first row's value");
    assert_eq!(value.placeholder.as_deref(), Some("Bearer ${TOKEN}"));
    assert!(f.find("mcp-h-name-2").is_some() && f.find("mcp-h-rm-2").is_some());
    assert!(f.texts().iter().any(|x| x == "mcp-h-note"), "the note under the rows");
    // The placeholder follows the name.
    f.act("mcp-h-name-2", json!("X-API-Key"));
    assert_eq!(f.find("mcp-h-value-2").unwrap().placeholder.as_deref(), Some("${API_KEY}"));
    f.act("mcp-h-name-2", json!("Cookie"));
    assert_eq!(f.find("mcp-h-value-2").unwrap().placeholder.as_deref(), Some("value, or ${SECRET}"));
    // Values typed in each row stay in that row.
    f.typed("mcp-h-value-1", "Bearer ${TOKEN}");
    f.typed("mcp-h-value-2", "session=${COOKIE}");
    f.press("mcp-h-rm-1");
    assert!(f.find("mcp-h-name-1").is_none() && f.find("mcp-h-value-1").is_none());
    assert_eq!(f.find("mcp-h-value-2").unwrap().value, json!("session=${COOKIE}"));
    f.press("mcp-h-rm-2");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "mcp-h-add") && !t.iter().any(|x| x.starts_with("mcp-h-name-")), "{t:?}");
}

#[test]
fn two_rows_one_custom_are_sent_as_two_name_value_lines_and_their_secrets_are_asked() {
    let mut f = remote_form("linear2");
    f.press("mcp-h-add");
    f.typed("mcp-h-value-1", "Bearer ${TOKEN}");
    f.press("mcp-h-add");
    f.act("mcp-h-name-2", json!("custom"));
    assert!(f.find("mcp-h-custom-2").is_some(), "Custom shows a name input");
    assert!(f.find("mcp-h-custom-1").is_none());
    f.typed("mcp-h-custom-2", "X-Org");
    f.typed("mcp-h-value-2", "${ORG}");
    // A third row left empty is skipped.
    f.press("mcp-h-add");
    let hint = f.texts().into_iter().find(|x| x.starts_with("Secrets asked in the next dialog")).expect("the hint");
    assert_eq!(hint, "Secrets asked in the next dialog: TOKEN, ORG");
    let sent = f.press("mcp-save");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["name"], "mcp_save");
    assert_eq!(sent[0]["args"], json!({ "name": "linear2", "kind": "remote", "url": "https://mcp.example.com/mcp", "headers": "Authorization: Bearer ${TOKEN}\nX-Org: ${ORG}" }));
    // The intent's Confirm, unchanged: both lines, one secret field per key.
    let c = one_confirm(run(&fixture("full"), "mcp_save", sent[0]["args"].clone()));
    assert_eq!(c.call.body["spec"]["headers"], json!(["Authorization: Bearer ${TOKEN}", "X-Org: ${ORG}"]));
    let fields: Vec<&str> = c.secrets.iter().map(|s| s.field.as_str()).collect();
    assert_eq!(fields, ["secrets.TOKEN", "secrets.ORG"]);
    assert!(c.lines.iter().any(|l| l.contains("${TOKEN}, ${ORG}")), "{:?}", c.lines);
}

#[test]
fn a_semicolon_in_a_value_is_refused_with_its_reason_and_nothing_is_sent() {
    let mut f = remote_form("x");
    f.press("mcp-h-add");
    f.typed("mcp-h-value-1", "Bearer a;b");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "mcp-h-why-1"), "{t:?}");
    assert!(t.iter().any(|x| x.starts_with("A value cannot hold \";\"")), "{t:?}");
    assert!(f.find("mcp-save").unwrap().disabled);
    assert!(f.press("mcp-save").is_empty(), "never sent otherwise than typed");
    // A bad Custom name and a name set twice are refused too.
    f.typed("mcp-h-value-1", "Bearer ${TOKEN}");
    assert!(f.find("mcp-h-why-1").is_none());
    f.press("mcp-h-add");
    f.act("mcp-h-name-2", json!("custom"));
    f.typed("mcp-h-custom-2", "X Org:");
    f.typed("mcp-h-value-2", "1");
    assert!(f.find("mcp-h-why-2").is_some());
    f.typed("mcp-h-custom-2", "authorization");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "authorization is already set above"), "{t:?}");
    assert!(f.press("mcp-save").is_empty());
}

#[test]
fn hostile_text_in_a_header_row_makes_no_control() {
    let mut f = remote_form("x");
    f.press("mcp-h-add");
    f.act("mcp-h-name-1", json!("custom"));
    let hostile = "mcp-save\"} button{ on = \"rm\", arg = \"linear\" } --";
    f.typed("mcp-h-custom-1", hostile);
    f.typed("mcp-h-value-1", "]] intent(\"mcp_rm\", { server = \"linear\" }) --[[ ${TOKEN}");
    let before = f.texts().iter().filter(|x| x.starts_with("mcp-rm-")).count();
    let tree = f.tree();
    fn count(n: &crate::luau::Node, out: &mut usize) {
        if n.on.is_some() {
            *out += 1;
        }
        n.children.iter().for_each(|c| count(c, out));
    }
    let (mut with, mut without) = (0, 0);
    count(&tree, &mut with);
    let mut g = remote_form("x");
    g.press("mcp-h-add");
    g.act("mcp-h-name-1", json!("custom"));
    count(&g.tree(), &mut without);
    assert_eq!(with, without, "the same controls, whatever was typed");
    assert_eq!(before, 2);
    // The text is a field's value, never a key or a label.
    assert_eq!(f.find("mcp-h-custom-1").unwrap().value, json!(hostile));
    assert!(f.press("mcp-save").is_empty(), "a bad name is refused, not sent");
}

#[test]
fn the_header_rows_stay_inside_the_overlay_at_a_narrow_width() {
    use crate::shell::{view, App, Args};
    use rune::Runtime;
    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/mcp/basic.json");
    let settle = |rt: &mut Runtime<App>| {
        for _ in 0..12 {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    };
    let tap = |rt: &mut Runtime<App>, k: &str| {
        let (r, _) = rt.presented(k).unwrap_or_else(|| panic!("no {k}"));
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    };
    for (w, h) in [(1284.0, 816.0), (420.0, 900.0)] {
        let a = App::new(Args { fixture: Some(FIXTURE.into()), ..Args::default() });
        let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        settle(&mut rt);
        if w < 700.0 {
            rt.update(|s, _| s.toggle_sidebar());
            settle(&mut rt);
        }
        for i in crate::intent::parse(&json!({ "kind": "intent", "name": "mcp_show", "args": {} })).unwrap() {
            rt.update(|s, _| s.run_intent("test", i));
        }
        settle(&mut rt);
        tap(&mut rt, "mcp-h-add");
        settle(&mut rt);
        tap(&mut rt, "mcp-h-add");
        settle(&mut rt);
        tap(&mut rt, "mcp-h-name-2");
        settle(&mut rt);
        tap(&mut rt, "mcp-h-name-2-custom");
        settle(&mut rt);
        assert!(rt.presented("mcp-h-custom-2").is_some(), "at {w}: the Custom name input");
        let (o, _) = rt.presented("overlay-mcp").unwrap();
        let mut bad = vec![];
        for e in rt.elements() {
            let Some(k) = e.key.clone() else { continue };
            if !k.starts_with("mcp-h") {
                continue;
            }
            let r = e.rect;
            if r[0] < o[0] - 0.5 || r[0] + r[2] > o[0] + o[2] + 0.5 {
                bad.push(format!("{k} {r:?}"));
            }
        }
        assert!(bad.is_empty(), "at {w}, outside {o:?}:\n{}", bad.join("\n"));
        for k in ["mcp-h-name-1", "mcp-h-value-1", "mcp-h-rm-1", "mcp-h-value-2", "mcp-h-rm-2", "mcp-h-add", "mcp-save"] {
            assert!(rt.presented(k).is_some(), "at {w}: {k}");
        }
    }
}

// ── Edit: a listed server's settings in the form, saved with `replace` ───

fn listed(servers: J) -> J {
    json!({ "mcp": { "ok": true, "servers": servers } })
}

fn linear() -> J {
    json!({ "name": "linear", "type": "remote", "detail": { "name": "linear", "type": "remote", "url": "https://mcp.linear.app/mcp",
        "headers": ["Authorization: Bearer ${TOKEN}", "X-Org: acme:eu", "api-key: ****"] } })
}

#[test]
fn edit_prefills_every_field_and_the_header_rows() {
    let mut f = Form::new(listed(json!([linear()])));
    f.press("mcp-edit-linear");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "Edit linear") && t.iter().any(|x| x == "Save…"), "{t:?}");
    assert_eq!(f.find("mcp-name").unwrap().value, json!("linear"));
    assert_eq!(f.find("mcp-kind").unwrap().value, json!("remote"));
    assert_eq!(f.find("mcp-url").unwrap().value, json!("https://mcp.linear.app/mcp"));
    // Known names are picked; another is a Custom row; split at the first colon.
    assert_eq!(f.find("mcp-h-name-1").unwrap().value, json!("Authorization"));
    assert_eq!(f.find("mcp-h-value-1").unwrap().value, json!("Bearer ${TOKEN}"));
    assert_eq!(f.find("mcp-h-name-2").unwrap().value, json!("custom"));
    assert_eq!(f.find("mcp-h-custom-2").unwrap().value, json!("X-Org"));
    assert_eq!(f.find("mcp-h-value-2").unwrap().value, json!("acme:eu"));
    // A masked value is not a value: left empty, not invented.
    assert_eq!(f.find("mcp-h-name-3").unwrap().value, json!("Api-Key"));
    assert_eq!(f.find("mcp-h-value-3").unwrap().value, json!(""));
    assert!(t.iter().any(|x| x == "Secrets asked in the next dialog: TOKEN (left empty: unchanged)"), "{t:?}");
    // ...and never dropped unsaid: its reason shows and Save is off until it is typed again or removed.
    assert!(t.iter().any(|x| x == "sbx did not show this header's value: type it again, or remove the row"), "{t:?}");
    assert!(t.iter().any(|x| x == "mcp-h-why-3") && f.find("mcp-save").unwrap().disabled);
    assert!(f.press("mcp-save").is_empty(), "not sent without the masked header");

    // Saved: the same mcp_save, with replace; the row left empty is skipped.
    f.typed("mcp-h-value-3", "${API_KEY}");
    assert!(!f.find("mcp-save").unwrap().disabled && f.find("mcp-h-why-3").is_none(), "typed again: Save is on");
    let sent = f.press("mcp-save");
    assert_eq!(sent[0]["args"], json!({ "name": "linear", "kind": "remote", "url": "https://mcp.linear.app/mcp", "replace": "linear",
        "headers": "Authorization: Bearer ${TOKEN}\nX-Org: acme:eu\nApi-Key: ${API_KEY}" }));
    let c = one_confirm(run(&fixture("full"), "mcp_save", sent[0]["args"].clone()));
    assert!(c.lines.iter().any(|l| l.contains("is removed and added again, which drops its OAuth token")), "{:?}", c.lines);
    assert_eq!(c.title, "Replace MCP server linear");
    assert!(c.secrets.iter().all(|s| s.optional), "an edit keeps a secret left empty");

    // Clear leaves edit mode.
    f.press("mcp-clear");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "Add a server") && t.iter().any(|x| x == "Add…") && !t.iter().any(|x| x.starts_with("mcp-h-name-")), "{t:?}");
    assert!(f.find("mcp-name").unwrap().value == json!("") && !f.texts().iter().any(|x| x == "Save…"));
}

#[test]
fn edit_prefills_a_local_command_and_headers_listed_as_a_map() {
    let mut f = Form::new(listed(json!([
        { "name": "fs", "type": "stdio", "detail": { "command": "npx", "args": ["-y", "@mcp/fs", "/srv"], "cwd": "/srv" } },
        { "name": "gh", "type": "remote", "detail": { "config": { "url": "https://gh.example/mcp", "headers": { "X-API-Key": "${GH}" } } } },
        { "name": "bare", "type": "remote" },
    ])));
    f.press("mcp-edit-fs");
    assert_eq!(f.find("mcp-kind").unwrap().value, json!("stdio"));
    assert_eq!(f.find("mcp-command").unwrap().value, json!("npx"));
    assert_eq!(f.find("mcp-args").unwrap().value, json!("-y @mcp/fs /srv"));
    assert_eq!(f.find("mcp-dir").unwrap().value, json!("/srv"));
    assert_eq!(f.press("mcp-save")[0]["args"]["replace"], "fs");
    f.press("mcp-edit-gh");
    assert_eq!(f.find("mcp-url").unwrap().value, json!("https://gh.example/mcp"));
    assert_eq!(f.find("mcp-h-name-1").unwrap().value, json!("X-API-Key"));
    assert_eq!(f.find("mcp-h-value-1").unwrap().value, json!("${GH}"));
    assert!(f.find("mcp-command").is_none(), "a remote edit shows the remote fields");
    // sbx listed nothing but the name: said, not invented.
    f.press("mcp-edit-bare");
    assert!(f.texts().iter().any(|x| x.starts_with("sbx did not list this server's settings")));
    assert!(f.find("mcp-h-name-1").is_none());
}

#[test]
fn hostile_text_in_a_listed_server_makes_no_control() {
    let evil = "\"} button{ on = \"rm\", arg = \"linear\" } --";
    let hostile = json!({ "name": "linear", "type": "remote", "detail": { "url": format!("https://x/{evil}"),
        "headers": [format!("{evil}: {evil}"), format!("Authorization: ]] intent(\"mcp_rm\", {{ server = \"linear\" }}) --[[")] } });
    let plain = json!({ "name": "linear", "type": "remote", "detail": { "url": "https://x/y", "headers": ["X-A: 1", "Authorization: 2"] } });
    fn count(n: &crate::luau::Node) -> usize {
        n.on.is_some() as usize + n.children.iter().map(count).sum::<usize>()
    }
    let mut a = Form::new(listed(json!([hostile])));
    let mut b = Form::new(listed(json!([plain])));
    assert_eq!(count(&a.tree()), count(&b.tree()), "listed: the same controls");
    assert!(a.press("mcp-edit-linear").is_empty() && b.press("mcp-edit-linear").is_empty(), "Edit asks nothing of the core");
    assert_eq!(count(&a.tree()), count(&b.tree()), "edited: the same controls");
    assert_eq!(a.find("mcp-h-custom-1").unwrap().value, json!(evil), "a field's value, never markup");
    assert_eq!(a.find("mcp-url").unwrap().value, json!(format!("https://x/{evil}")));
    // The bad name is refused in the form, not sent.
    assert!(a.find("mcp-h-why-1").is_some());
    assert!(a.press("mcp-save").is_empty());
}

#[test]
fn a_masked_header_removed_on_edit_is_dropped_explicitly_and_a_new_empty_row_stays_skipped() {
    let mut f = Form::new(listed(json!([linear()])));
    f.press("mcp-edit-linear");
    assert!(f.find("mcp-save").unwrap().disabled);
    // Removing the row is the person's way to drop that header.
    f.press("mcp-h-rm-3");
    assert!(!f.find("mcp-save").unwrap().disabled && f.find("mcp-h-why-3").is_none());
    // A row the person adds and leaves empty is skipped, as before.
    f.press("mcp-h-add");
    assert!(f.find("mcp-h-why-4").is_none() && !f.find("mcp-save").unwrap().disabled);
    let sent = f.press("mcp-save");
    assert_eq!(sent[0]["args"]["headers"], "Authorization: Bearer ${TOKEN}\nX-Org: acme:eu");
    // A listed header with no value at all is held the same way.
    let mut g = Form::new(listed(json!([{ "name": "a", "type": "remote", "detail": { "url": "https://a", "headers": ["X-Empty:"] } }])));
    g.press("mcp-edit-a");
    assert!(g.find("mcp-h-why-1").is_some() && g.press("mcp-save").is_empty());
    g.typed("mcp-h-value-1", "1");
    assert_eq!(g.press("mcp-save")[0]["args"]["headers"], "X-Empty: 1");
}

#[test]
fn a_client_id_is_sent_named_in_the_confirm_and_its_secret_is_asked_optional() {
    let mut f = remote_form("github");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "Only for a server that needs a pre-registered OAuth client, such as GitHub."), "{t:?}");
    assert_eq!(f.find("mcp-client-id").unwrap().value, json!(""));
    f.typed("mcp-client-id", " Iv1.abc ");
    assert!(f.texts().iter().any(|x| x == "Secrets asked in the next dialog: the OAuth client secret (optional)"), "{:?}", f.texts());
    let sent = f.press("mcp-save");
    assert_eq!(sent[0]["args"], json!({ "name": "github", "kind": "remote", "url": "https://mcp.example.com/mcp", "client_id": "Iv1.abc" }));
    let c = one_confirm(run(&fixture("full"), "mcp_save", sent[0]["args"].clone()));
    assert_eq!(c.call.body["spec"]["client_id"], "Iv1.abc");
    assert!(c.lines.iter().any(|l| l.starts_with("OAuth client ID: Iv1.abc")), "{:?}", c.lines);
    let s: Vec<(&str, bool)> = c.secrets.iter().map(|s| (s.field.as_str(), s.optional)).collect();
    assert_eq!(s, [("secrets.client_secret", true)]);
    assert_eq!(c.call.body["secrets"], json!({}), "a secret is the dialog's, never the intent's");
    // With a header secret: both, the header's first.
    let c = one_confirm(run(&fixture("full"), "mcp_save", json!({ "name": "g", "kind": "remote", "url": "https://x", "headers": "X-K: ${KEY}", "client_id": "abc" })));
    let fields: Vec<&str> = c.secrets.iter().map(|s| s.field.as_str()).collect();
    assert_eq!(fields, ["secrets.KEY", "secrets.client_secret"]);
    // Without one, no client secret and an empty client_id.
    let c = one_confirm(run(&fixture("full"), "mcp_save", json!({ "name": "g", "kind": "remote", "url": "https://x" })));
    assert!(c.secrets.is_empty() && c.call.body["spec"]["client_id"] == "");
    assert!(!c.lines.iter().any(|l| l.contains("client ID")));
}

#[test]
fn a_bad_client_id_is_refused_and_a_local_command_takes_none() {
    let data = fixture("full");
    for bad in ["-x", "a b", "a;b", "é"] {
        assert!(run(&data, "mcp_save", json!({ "name": "g", "kind": "remote", "url": "https://x", "client_id": bad })).unwrap_err().contains("client ID"), "{bad}");
    }
    assert!(run(&data, "mcp_save", json!({ "name": "s", "kind": "stdio", "command": "npx", "client_id": "abc" })).unwrap_err().contains("remote"));
    let e = run(&data, "mcp_save", json!({ "name": "g", "kind": "remote", "url": "https://x", "headers": "X-K: ${client_secret}" })).unwrap_err();
    assert!(e.contains("client secret"), "{e}");
}

#[test]
fn edit_prefills_the_listed_client_id_and_its_secret_stays_unchanged_if_empty() {
    let gh = json!({ "name": "github", "type": "remote", "detail": { "url": "https://api.githubcopilot.com/mcp", "oauth": { "client_id": "Iv1.abc", "client_secret": "never-shown" } } });
    let mut f = Form::new(listed(json!([gh])));
    f.press("mcp-edit-github");
    assert_eq!(f.find("mcp-client-id").unwrap().value, json!("Iv1.abc"));
    assert!(!f.texts().iter().any(|x| x.contains("never-shown")), "a secret never reaches the form");
    let sent = f.press("mcp-save");
    assert_eq!(sent[0]["args"]["client_id"], "Iv1.abc");
    let c = one_confirm(run(&fixture("full"), "mcp_save", sent[0]["args"].clone()));
    assert!(c.secrets.iter().any(|s| s.field == "secrets.client_secret" && s.optional && s.placeholder == "unchanged if left empty"));
    // A local command's form has no client ID field.
    f.press("mcp-clear");
    f.act("mcp-kind", json!("stdio"));
    assert!(f.find("mcp-client-id").is_none());
}

// ── A credential header takes a ${KEY} (the core's rule, on its table) ───

/// The table the core and the web UI are tested on.
const HEADER_CASES: &str = include_str!("../../../../tests/js/mcp-header-cases.json");

fn header_cases() -> J {
    serde_json::from_str(HEADER_CASES).unwrap()
}

fn strs(v: &J) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

fn refusal_of(c: &J, name: &str) -> String {
    let example = c["example"].get(name.to_ascii_lowercase().as_str()).unwrap_or(&c["example"]["other"]).as_str().unwrap();
    c["message"].as_str().unwrap().replace("NAME", name).replace("EXAMPLE", example)
}

#[test]
fn the_shared_table_is_the_intents_rule_and_the_screens() {
    let c = header_cases();
    assert_eq!(strs(&c["words"]), CREDENTIAL_WORDS);
    let data = fixture("full");
    let save = |h: &str| run(&data, "mcp_save", json!({ "name": "acme", "kind": "remote", "url": "https://x", "headers": h }));
    for name in strs(&c["credential"]) {
        assert!(credential_header(&name), "{name}");
        for v in strs(&c["values"]["ok"]) {
            assert!(save(&format!("{name}: {v}")).is_ok(), "{name}: {v}");
        }
        for v in strs(&c["values"]["refused"]) {
            assert_eq!(save(&format!("{name}: {v}")).unwrap_err(), refusal_of(&c, &name), "{name}: {v}");
        }
    }
    for name in strs(&c["free"]) {
        assert!(!credential_header(&name), "{name}");
        for v in strs(&c["values"]["refused"]) {
            assert!(save(&format!("{name}: {v}")).is_ok(), "{name}: {v}");
        }
    }
    // The screen's copy: a Custom row of each name, a refused value and a placeholder.
    let mut f = remote_form("acme");
    f.press("mcp-h-add");
    f.act("mcp-h-name-1", json!("custom"));
    for name in strs(&c["credential"]).into_iter().chain(strs(&c["free"])) {
        f.typed("mcp-h-custom-1", &name);
        f.typed("mcp-h-value-1", "Bearer abc123");
        let t = f.texts();
        let refused = credential_header(&name);
        assert_eq!(t.contains(&refusal_of(&c, &name)), refused, "{name}: {t:?}");
        assert_eq!(f.find("mcp-save").unwrap().disabled, refused, "{name}");
        f.typed("mcp-h-value-1", "Bearer ${token}");
        assert!(f.find("mcp-h-why-1").is_none() && !f.find("mcp-save").unwrap().disabled, "{name}");
    }
}

#[test]
fn a_literal_credential_shows_its_reason_under_the_row_and_save_is_off() {
    let c = header_cases();
    let mut f = remote_form("acme");
    f.press("mcp-h-add");
    // A picked name.
    f.typed("mcp-h-value-1", "Bearer ghp_Sup3rS3cret");
    let t = f.texts();
    assert!(t.iter().any(|x| x == "mcp-h-why-1") && t.contains(&refusal_of(&c, "Authorization")), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("Sup3r") && x != "Bearer ghp_Sup3rS3cret"), "the reason holds no part of the value: {t:?}");
    assert!(f.find("mcp-save").unwrap().disabled && f.press("mcp-save").is_empty(), "never sent");
    for pick in ["X-API-Key", "Api-Key", "Cookie"] {
        f.act("mcp-h-name-1", json!(pick));
        assert!(f.texts().contains(&refusal_of(&c, pick)), "{pick}");
    }
    // A placeholder lifts it.
    f.act("mcp-h-name-1", json!("Authorization"));
    f.typed("mcp-h-value-1", "Bearer ${TOKEN}");
    assert!(f.find("mcp-h-why-1").is_none());
    assert_eq!(f.press("mcp-save")[0]["args"]["headers"], "Authorization: Bearer ${TOKEN}");
    // A harmless Custom header stays free.
    f.press("mcp-h-add");
    f.act("mcp-h-name-2", json!("custom"));
    f.typed("mcp-h-custom-2", "X-MCP-Toolsets");
    f.typed("mcp-h-value-2", "repos");
    assert!(f.find("mcp-h-why-2").is_none() && !f.find("mcp-save").unwrap().disabled);
}

#[test]
fn the_confirm_is_never_opened_with_a_literal_credential() {
    let data = fixture("full");
    let e = run(&data, "mcp_save", json!({ "name": "acme", "kind": "remote", "url": "https://x", "headers": "Accept: application/json\nAuthorization: Bearer ghp_Sup3rS3cret" })).unwrap_err();
    assert_eq!(e, "Authorization carries a credential: write its value with a placeholder, such as `Bearer ${token}`, and sbxw asks for the secret separately");
    for h in ["x-api-key: abc", "Cookie: sid=abc; Accept: x", "X-Auth-Token: ${"] {
        let r = run(&data, "mcp_save", json!({ "name": "acme", "kind": "remote", "url": "https://x", "headers": h, "replace": "acme" }));
        assert!(r.as_ref().is_err_and(|e| e.contains("carries a credential") && !e.contains("abc")), "{h}: {r:?}");
    }
    // A placeholder: the one Confirm, as before.
    let c = one_confirm(run(&data, "mcp_save", json!({ "name": "acme", "kind": "remote", "url": "https://x", "headers": "Authorization: Bearer ${TOKEN}" })));
    assert!(c.lines.iter().any(|l| l == "Headers: Authorization: Bearer ${TOKEN}"), "{:?}", c.lines);
}

#[test]
fn an_edit_of_a_server_with_a_literal_credential_shows_it_refused_and_never_sends_it_back() {
    let c = header_cases();
    let old = json!({ "name": "old", "type": "remote", "detail": { "url": "https://old.example/mcp",
        "headers": ["Authorization: Bearer ghp_registeredBefore", "X-MCP-Toolsets: repos"] } });
    let mut f = Form::new(listed(json!([old])));
    f.press("mcp-edit-old");
    // Pre-filled as listed, the reason under it, Save off.
    assert_eq!(f.find("mcp-h-value-1").unwrap().value, json!("Bearer ghp_registeredBefore"));
    let t = f.texts();
    assert!(t.iter().any(|x| x == "mcp-h-why-1") && t.contains(&refusal_of(&c, "Authorization")), "{t:?}");
    assert!(f.find("mcp-h-why-2").is_none());
    assert!(f.find("mcp-save").unwrap().disabled && f.press("mcp-save").is_empty(), "the literal is never sent back");
    // Replaced by a placeholder: saved with replace, and the token is asked in the Confirm.
    f.typed("mcp-h-value-1", "Bearer ${TOKEN}");
    let sent = f.press("mcp-save");
    assert_eq!(sent[0]["args"]["headers"], "Authorization: Bearer ${TOKEN}\nX-MCP-Toolsets: repos");
    assert_eq!(sent[0]["args"]["replace"], "old");
    let confirm = one_confirm(run(&fixture("full"), "mcp_save", sent[0]["args"].clone()));
    assert!(confirm.secrets.iter().any(|s| s.field == "secrets.TOKEN"));
    assert!(!confirm.lines.iter().any(|l| l.contains("ghp_")), "{:?}", confirm.lines);
}
