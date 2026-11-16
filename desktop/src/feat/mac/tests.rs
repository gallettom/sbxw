use super::*;
use crate::feat::Standin;
use serde_json::Map;

fn fixture(name: &str) -> J {
    let raw = match name {
        "basic" => include_str!("../../../fixtures/mac/basic.json"),
        "missing" => include_str!("../../../fixtures/mac/missing.json"),
        "not-installed" => include_str!("../../../fixtures/mac/not-installed.json"),
        "unsupported" => include_str!("../../../fixtures/mac/unsupported.json"),
        "stopping" => include_str!("../../../fixtures/mac/stopping.json"),
        "setup" => include_str!("../../../fixtures/mac/setup.json"),
        _ => include_str!("../../../fixtures/mac/errors.json"),
    };
    let v: J = serde_json::from_str(raw).unwrap();
    v["daemon"].clone()
}

fn ask(name: &'static str) -> Asked {
    Asked { name, args: Map::new() }
}

fn run(d: &J, name: &'static str) -> Result<Vec<Action>, String> {
    act(&Ctx { data: d }, &ask(name))
}

fn spec(name: &str) -> &'static Spec {
    INTENTS.iter().find(|s| s.name == name).unwrap()
}

/// The core's rule (`shell::features::admit`): an approval answers with its dialog only.
fn admitted(name: &str, a: &[Action]) -> bool {
    match spec(name).risk {
        Risk::Direct => true,
        Risk::Approval(_) => matches!(a, [Action::Confirm(_)] | [Action::Prepare(_)]),
    }
}

fn confirm(a: &[Action]) -> &Confirm {
    match a {
        [Action::Confirm(c)] => c,
        other => panic!("not one Confirm: {other:?}"),
    }
}

#[test]
fn what_downloads_loses_work_or_takes_the_mac_is_an_approval() {
    for n in ["mac_install", "mac_stop", "mac_force_off", "mac_take_back"] {
        assert!(matches!(spec(n).risk, Risk::Approval(_)), "{n}");
    }
    for n in ["mac_open", "mac_refresh", "mac_up", "mac_pause", "mac_window"] {
        assert!(matches!(spec(n).risk, Risk::Direct), "{n}");
    }
}

#[test]
fn each_intent_answers_as_the_core_admits_it() {
    let cases: &[(&str, &[&'static str])] = &[
        ("not-installed", &["mac_install"]),
        ("missing", &["mac_up"]),
        ("basic", &["mac_pause", "mac_window", "mac_stop", "mac_take_back", "mac_force_off"]),
        ("stopping", &["mac_force_off", "mac_window"]),
    ];
    for (fx, names) in cases {
        let d = fixture(fx);
        for n in *names {
            let a = run(&d, n).unwrap_or_else(|e| panic!("{fx} {n}: {e}"));
            assert!(admitted(n, &a), "{fx} {n}: {a:?}");
        }
    }
}

#[test]
fn install_says_what_is_downloaded_from_the_daemons_data() {
    let d = fixture("not-installed");
    let a = run(&d, "mac_install").unwrap();
    let c = confirm(&a);
    assert_eq!(c.call.path, "/api/mac/install");
    assert!(c.lines.iter().any(|l| l.contains("v0.3.0-rc.2") && l.contains("SHA-256")), "{:?}", c.lines);
    assert!(c.lines.iter().any(|l| l.contains("not notarised")));
    // Already installed: refused, nothing asked.
    assert!(run(&fixture("basic"), "mac_install").unwrap_err().contains("already"));
}

#[test]
fn creating_the_vm_is_confirmed_starting_an_existing_one_is_not() {
    let a = run(&fixture("missing"), "mac_up").unwrap();
    let c = confirm(&a);
    assert!(c.lines.iter().any(|l| l.contains("several GB")));
    assert_eq!(c.call.path, "/api/mac/up");
    let mut d = fixture("missing");
    d["mac"]["mac"]["state"] = "stopped".into();
    let a = run(&d, "mac_up").unwrap();
    assert!(matches!(a.as_slice(), [Action::Call(c)] if c.path == "/api/mac/up"), "{a:?}");
    d["mac"]["mac"]["state"] = "paused".into();
    let a = run(&d, "mac_up").unwrap();
    assert!(matches!(a.as_slice(), [Action::Call(c)] if c.what.contains("resume")), "{a:?}");
}

#[test]
fn shutting_down_and_forcing_off_are_red_and_say_what_is_lost() {
    let d = fixture("basic");
    let c = run(&d, "mac_stop").unwrap();
    let c = confirm(&c);
    assert!(c.danger && c.lines.iter().any(|l| l.contains("held by web-app")));
    let f = run(&fixture("stopping"), "mac_force_off").unwrap();
    let f = confirm(&f);
    assert!(f.danger && f.lines.iter().any(|l| l.contains("unsaved")));
    assert!(f.lines.iter().any(|l| l.contains("asked it to shut down")), "{:?}", f.lines);
    let t = run(&d, "mac_take_back").unwrap();
    assert!(confirm(&t).title.contains("web-app"));
}

#[test]
fn nothing_is_asked_of_a_mac_that_cannot_do_it() {
    assert!(run(&fixture("unsupported"), "mac_up").unwrap_err().contains("Apple silicon"));
    assert!(run(&fixture("unsupported"), "mac_install").unwrap_err().contains("Apple silicon"));
    assert!(run(&fixture("errors"), "mac_up").unwrap_err().contains("not known"));
    // Busy: every button waits.
    let e = run(&fixture("setup"), "mac_stop").unwrap_err();
    assert!(e.contains("busy"), "{e}");
    assert!(run(&fixture("not-installed"), "mac_up").unwrap_err().contains("not installed"));
    assert!(run(&fixture("basic"), "mac_up").unwrap_err().contains("already running"));
    assert!(run(&fixture("missing"), "mac_take_back").unwrap_err().contains("nobody"));
    assert!(run(&fixture("missing"), "mac_window").unwrap_err().contains("not running"));
}

#[test]
fn the_top_bar_button_shows_the_state_and_waiting_requests() {
    let e = extras(&Ctx { data: &fixture("basic") });
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].tone.as_str(), e[0].count), ("green", None));
    assert!(e[0].help.contains("web-app"));
    let e = extras(&Ctx { data: &fixture("setup") });
    assert_eq!((e[0].tone.as_str(), e[0].count), ("amber", Some(2)));
    assert!(extras(&Ctx { data: &fixture("unsupported") }).is_empty());
    assert!(extras(&Ctx { data: &fixture("errors") }).is_empty());
}

#[test]
fn a_daemon_answer_becomes_its_notice() {
    let cx = Ctx { data: &J::Null };
    let call = Call::post("/api/mac/stop", json!({}), "shut the virtual Mac down");
    let a = on_answer(&cx, &call, Ok((200, json!({ "ok": true, "text": "VM\nstopped" }))));
    assert_eq!(a, vec![Action::notice("VM stopped")]);
    let a = on_answer(&cx, &call, Ok((500, json!({ "ok": false, "error": "no VM" }))));
    assert_eq!(a, vec![Action::error("Could not shut the virtual Mac down: no VM")]);
    let a = on_answer(&cx, &call, Err("timed out".into()));
    assert!(matches!(&a[0], Action::Notice { error: true, .. }));
}

#[test]
fn the_standin_moves_the_mac_as_the_daemon_would() {
    let mut d = fixture("not-installed");
    let mut events = vec![];
    let mut publish = |n: &str, _: J| events.push(n.to_string());
    let mut st = Standin { data: &mut d, publish: &mut publish };
    let (c, b) = standin(&mut st, "GET", "/api/mac", &J::Null).unwrap();
    assert_eq!((c, b["mac"]["installed"].clone()), (200, J::Bool(false)));
    assert_eq!(standin(&mut st, "POST", "/api/mac/install", &json!({})).unwrap().0, 200);
    assert_eq!(standin(&mut st, "POST", "/api/mac/up", &json!({})).unwrap().0, 200);
    assert_eq!(st.data["mac"]["mac"]["state"], "running");
    assert_eq!(standin(&mut st, "POST", "/api/mac/pause", &json!({})).unwrap().1["text"], "VM paused");
    assert_eq!(standin(&mut st, "POST", "/api/mac/stop", &json!({})).unwrap().0, 200);
    assert_eq!(standin(&mut st, "POST", "/api/mac/pause", &json!({})).unwrap().0, 500);
    assert!(standin(&mut st, "POST", "/api/mac/nope", &json!({})).is_none());
    drop(st);
    assert!(events.iter().all(|e| e == "mac") && events.len() >= 4);
}
