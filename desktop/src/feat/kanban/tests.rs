//! The Kanban: five columns and their counts as the daemon decided them, the
//! ticket panel, the form (paths only from the core, the guardrails), delete
//! through one Confirm, "Send to the lead" as one Confirm and one fixed line
//! (refused under a dialog or a Jarvis proposal, held for a session asked
//! something), moves only inside Planned, hostile text that makes no
//! control, nothing outside the overlay at any width, the render budget.

use super::*;
use crate::shell::{view, App, Args};
use rune::Runtime;

const BASIC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/kanban/basic.json");
const BIG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/kanban/big.json");

fn fx(path: &str) -> J {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn data_of(path: &str, w: f64) -> J {
    let mut d = fx(path)["daemon"].clone();
    d["app"] = json!({ "groups": { "web-app": "#e06c75", "docs": "#61afef" }, "overlay": { "screen": "kanban", "w": w, "h": 760 }, "selected": "web-app", "covered_top": false });
    d["jarvis"] = json!({ "pending": [] });
    d
}

fn data() -> J {
    data_of(BASIC, 1260.0)
}

fn asked(name: &'static str, args: J) -> Asked {
    crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap_or_else(|e| panic!("{name}: {e}"));
    Asked { name, args: args.as_object().cloned().unwrap_or_default() }
}

fn last_set(out: &[Action], key: &str) -> J {
    out.iter().rev().find_map(|a| match a {
        Action::Set { key: k, value } if k == key => Some(value.clone()),
        _ => None,
    }).unwrap_or(J::Null)
}

/// Answers every call from the fixture's stand-in, as the core would.
fn drive(st: &mut State, d: &mut J, mut out: Vec<Action>) -> (J, Vec<Action>) {
    let mut last = last_set(&out, UI);
    let mut all = vec![];
    while let Some(i) = out.iter().position(|a| matches!(a, Action::Call(_))) {
        let Action::Call(c) = out.remove(i) else { unreachable!() };
        if c.raw.is_some() {
            continue;
        }
        let method = if c.method == crate::feat::Method::Post { "POST" } else { "GET" };
        let r = crate::feat::answer_from(d, method, &c.path, &c.body, &mut |_, _| {});
        let more = st.on_answer(&Ctx { data: d }, &c, Ok(r));
        let v = last_set(&more, UI);
        if !v.is_null() {
            last = v;
        }
        out.extend(more);
    }
    all.extend(out);
    (last, all)
}

fn run(st: &mut State, d: &mut J, name: &'static str, args: J) -> (J, Vec<Action>) {
    let out = st.act(&Ctx { data: d }, &asked(name, args)).unwrap();
    drive(st, d, out)
}

fn show(st: &mut State, d: &mut J, sb: &str) -> J {
    let out = st.act(&Ctx { data: d }, &asked("kanban_show", json!({ "sandbox": sb }))).unwrap();
    assert!(out.iter().any(|a| matches!(a, Action::OpenPanel(s) if s == SCREEN)));
    drive(st, d, out).0
}

fn col<'a>(v: &'a J, id: &str) -> &'a J {
    v["columns"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()
}

fn card_id(v: &J, num: &str) -> String {
    v["columns"].as_array().unwrap().iter().flat_map(|c| c["cards"].as_array().unwrap()).find(|c| c["num"] == num).unwrap_or_else(|| panic!("no card {num}"))["id"].as_str().unwrap().to_string()
}

fn column_of(v: &J, num: &str) -> String {
    v["columns"].as_array().unwrap().iter().find(|c| c["cards"].as_array().unwrap().iter().any(|x| x["num"] == num)).map(|c| c["id"].as_str().unwrap().to_string()).unwrap_or_default()
}

#[test]
fn five_columns_with_their_counts_and_each_card_where_the_daemon_put_it() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, "web-app");
    let ids: Vec<&str> = v["columns"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["planned", "progress", "test", "ready", "merged"]);
    assert_eq!(v["columns"].as_array().unwrap().iter().map(|c| c["count"].as_u64().unwrap()).collect::<Vec<_>>(), [4, 3, 1, 2, 3]);
    assert_eq!(column_of(&v, "T-1"), "planned", "a P1 with files, first");
    assert_eq!(col(&v, "planned")["cards"][0]["num"], "T-1");
    assert_eq!(column_of(&v, "T-5"), "progress");
    assert_eq!(column_of(&v, "T-6"), "test");
    assert_eq!(column_of(&v, "T-7"), "ready");
    assert_eq!(column_of(&v, "T-9"), "merged");
    // The blocked thread is marked; the failed thread with no ticket is a red card labelled "thread".
    let blocked = col(&v, "progress")["cards"].as_array().unwrap().iter().find(|c| c["num"] == "T-5").unwrap().clone();
    assert_eq!(blocked["marks"].as_array().unwrap().iter().map(|m| m["label"].as_str().unwrap()).collect::<Vec<_>>(), ["P1", "blocked", "2 files"]);
    let failed = col(&v, "progress")["cards"].as_array().unwrap().iter().find(|c| c["thread"] == "api-routes").unwrap().clone();
    assert_eq!((failed["num"].as_str(), failed["border"].as_str(), failed["thread_card"].as_bool()), (Some("thread"), Some("red-border"), Some(true)));
    let validated = col(&v, "merged")["cards"].as_array().unwrap().iter().find(|c| c["num"] == "T-9").unwrap().clone();
    assert!(validated["marks"].as_array().unwrap().iter().any(|m| m["label"] == "✓ validated"));
    // All sandboxes: each card with its sandbox's colour mark.
    let (v, _) = run(&mut st, &mut d, "kanban_scope", json!({ "sandbox": "" }));
    assert_eq!(v["scope"], "");
    assert_eq!(col(&v, "planned")["count"], 5);
    assert!(col(&v, "planned")["cards"].as_array().unwrap().iter().any(|c| c["team_color"] == "#61afef" && c["sandbox_label"] == "docs"));
}

#[test]
fn a_card_opens_its_ticket_as_a_panel_and_escape_closes_it_back_to_the_card() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, "web-app");
    let id = card_id(&v, "T-5");
    let (v, out) = run(&mut st, &mut d, "kanban_open", json!({ "id": id }));
    let t = &v["ticket"];
    assert_eq!((t["num"].as_str(), t["column_label"].as_str(), t["thread"]["name"].as_str()), (Some("T-5"), Some("In progress"), Some("members-page")));
    assert!(t["description"].as_str().unwrap().contains("3,000 members"));
    assert_eq!(t["files"].as_array().unwrap().len(), 2);
    assert_eq!(t["send_why"], "Already in a plan", "disabled, with its reason");
    assert!(!t["deletable"].as_bool().unwrap(), "its thread works");
    assert!(v["popup"].as_bool().unwrap());
    // Its picture is fetched for the preview (raw bytes, decoded by the core).
    let _ = out;
    let (v, out) = run(&mut st, &mut d, "kanban_key", json!({ "key": "escape" }));
    assert!(v["ticket"].is_null());
    assert!(out.iter().any(|a| matches!(a, Action::Focus(k) if *k == id)), "the keyboard back on its card");
    // A thread with no ticket: its card, no Send, no form.
    let tid = col(&v, "progress")["cards"].as_array().unwrap().iter().find(|c| c["thread"] == "api-routes").unwrap()["id"].as_str().unwrap().to_string();
    let (v, _) = run(&mut st, &mut d, "kanban_open", json!({ "id": tid }));
    assert!(v["ticket"]["thread_card"].as_bool().unwrap());
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_edit", json!({}))).is_err());
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_send", json!({}))).is_err());
}

#[test]
fn the_arrows_move_between_cards_and_columns_and_n_opens_the_form() {
    let mut d = data();
    let mut st = State::default();
    show(&mut st, &mut d, "web-app");
    let (v, _) = run(&mut st, &mut d, "kanban_key", json!({ "key": "down" }));
    let first = col(&v, "planned")["cards"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(st.focus.as_deref(), Some(first.as_str()));
    run(&mut st, &mut d, "kanban_key", json!({ "key": "down" }));
    assert_eq!(st.focus.as_deref(), col(&v, "planned")["cards"][1]["id"].as_str());
    run(&mut st, &mut d, "kanban_key", json!({ "key": "right" }));
    assert!(col(&v, "progress")["cards"].as_array().unwrap().iter().any(|c| Some(c["id"].as_str().unwrap()) == st.focus.as_deref()));
    let (v, _) = run(&mut st, &mut d, "kanban_key", json!({ "key": "enter" }));
    assert!(!v["ticket"].is_null());
    let (_, out) = run(&mut st, &mut d, "kanban_key", json!({ "key": "type", "text": "n" }));
    assert!(out.iter().any(|a| matches!(a, Action::OpenModal { screen, .. } if screen == FORM)));
}

#[test]
fn the_form_takes_paths_only_from_the_core_and_asks_once_for_a_file_that_looks_like_a_secret() {
    let mut d = data();
    let mut st = State::default();
    show(&mut st, &mut d, "web-app");
    let (_, out) = run(&mut st, &mut d, "kanban_new", json!({}));
    assert!(out.iter().any(|a| matches!(a, Action::OpenModal { screen, .. } if screen == FORM)));
    // No intent takes a path.
    for s in INTENTS {
        assert!(s.args.iter().all(|(n, _, _)| !["path", "paths", "file_path"].contains(n)), "{}", s.name);
    }
    // Files handed in by the core (a drop on the form): one plain, one that looks like a secret.
    let dir = std::env::temp_dir().join(format!("sbxw-kanban-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (plain, secret) = (dir.join("notes.txt"), dir.join("id_rsa"));
    std::fs::write(&plain, "invented notes").unwrap();
    std::fs::write(&secret, "not a key").unwrap();
    d["app"]["modal"] = json!({ "screen": FORM });
    let out = st.on_files(&Ctx { data: &d }, Incoming::Files(vec![plain.clone(), secret.clone()]));
    let f = last_set(&out, FORM_UI);
    assert_eq!(f["files"].as_array().unwrap().len(), 2);
    assert!(f["files"][0]["look"].is_null() && f["files"][1]["look"].as_str().unwrap().contains("secret"), "{f}");
    // A picture pasted.
    st.on_files(&Ctx { data: &d }, Incoming::Png(b"\x89PNG\r\n\x1a\nxx".to_vec()));
    // Nothing handed while the form is not shown and nothing was asked for.
    d["app"]["modal"] = J::Null;
    assert!(st.on_files(&Ctx { data: &d }, Incoming::Files(vec![plain.clone()])).is_empty());
    d["app"]["modal"] = json!({ "screen": FORM });
    // Save without a title: said in the form.
    let out = st.act(&Ctx { data: &d }, &asked("kanban_save", json!({}))).unwrap();
    assert!(last_set(&out, FORM_UI)["error"].as_str().unwrap().contains("title"));
    st.act(&Ctx { data: &d }, &asked("kanban_form", json!({ "title": "Export as PDF", "description": "With the logo.", "priority": "p1" }))).unwrap();
    // The secret-looking file: ONE Confirm with the ack box, its call the save.
    let out = st.act(&Ctx { data: &d }, &asked("kanban_save", json!({}))).unwrap();
    let Some(Action::Confirm(c)) = out.iter().find(|a| matches!(a, Action::Confirm(_))) else { panic!("no confirm: {out:?}") };
    assert!(c.ack.as_deref() == Some("I have looked at these files and want to attach them"));
    assert!(c.lines.iter().any(|l| l.contains("id_rsa")));
    assert_eq!(c.call.path, "/api/tickets/web-app");
    // Confirmed: the ticket, then each file in turn, then the panel on it.
    let call = c.call.clone();
    let r = crate::feat::answer_from(&mut d, "POST", &call.path, &call.body, &mut |_, _| {});
    let more = st.on_answer(&Ctx { data: &d }, &call, Ok(r));
    let (_, rest) = drive(&mut st, &mut d, more);
    let ups: Vec<&Action> = rest.iter().filter(|a| matches!(a, Action::CloseModal)).collect();
    assert_eq!(ups.len(), 1, "closed once saved: {rest:?}");
    assert!(st.form.is_none());
    assert!(st.open.as_deref().is_some_and(|o| o.starts_with("kanban-row-t-web-app-")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_and_file_removal_go_through_one_confirm_naming_what_goes() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, "web-app");
    run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-1") }));
    let out = st.act(&Ctx { data: &d }, &asked("kanban_delete", json!({}))).unwrap();
    let [Action::Confirm(c)] = out.as_slice() else { panic!("{out:?}") };
    assert!(c.danger);
    assert_eq!(c.title, "Delete T-1?");
    assert!(c.lines.iter().any(|l| l.contains("Its 2 files go with it")), "{:?}", c.lines);
    assert_eq!(c.call.path, "/api/tickets/web-app/1/delete");
    let out = st.act(&Ctx { data: &d }, &asked("kanban_remove_file", json!({ "file": "a-00000001" }))).unwrap();
    let [Action::Confirm(c)] = out.as_slice() else { panic!("{out:?}") };
    assert!(c.lines[0].contains("report-today.png"));
    // A ticket at work: refused, with why.
    run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-5") }));
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_delete", json!({}))).unwrap_err().contains("at work"));
}

#[test]
fn send_to_the_lead_is_one_confirm_with_one_fixed_line_and_is_refused_under_a_dialog_or_a_proposal() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, "web-app");
    run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-1") }));
    let out = st.act(&Ctx { data: &d }, &asked("kanban_send", json!({}))).unwrap();
    let [Action::Confirm(c)] = out.as_slice() else { panic!("{out:?}") };
    assert_eq!(c.title, "Send T-1 to web-app's lead?");
    let p = c.preview.as_ref().unwrap();
    assert_eq!(p.text, "[sbxw] ticket T-1 waits for a plan: read it with ticket_read");
    assert!(!p.text.contains("PDF"), "never the ticket's words");
    assert_eq!(c.call.path, "/api/tickets/web-app/1/send");
    assert_eq!(c.call.body, json!({}), "the daemon writes the line");
    assert!(spec("kanban_send").is_some_and(|s| matches!(s.risk, Risk::Approval(_))));
    // A session waiting on a question: held, said in the Confirm.
    d["sessions"][0]["state"] = "attention".into();
    let out = st.act(&Ctx { data: &d }, &asked("kanban_send", json!({}))).unwrap();
    let [Action::Confirm(c)] = out.as_slice() else { panic!() };
    assert!(c.lines.iter().any(|l| l.contains("waits on a question now")));
    d["sessions"][0]["state"] = "idle".into();
    // Under a dialog or a menu, a Jarvis proposal, no session: refused.
    d["app"]["covered_top"] = true.into();
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_send", json!({}))).unwrap_err().contains("close it first"));
    d["app"]["covered_top"] = false.into();
    d["jarvis"]["pending"] = json!([{ "id": "j1" }]);
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_send", json!({}))).unwrap_err().contains("Jarvis"));
    d["jarvis"]["pending"] = json!([]);
    d["sessions"] = json!([]);
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_send", json!({}))).unwrap_err().contains("no open agent session"));
}

fn spec(name: &str) -> Option<&'static Spec> {
    INTENTS.iter().find(|s| s.name == name)
}

#[test]
fn only_a_planned_ticket_with_no_thread_moves() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, "web-app");
    run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-2") }));
    let out = st.act(&Ctx { data: &d }, &asked("kanban_move", json!({ "to": "up" }))).unwrap();
    assert!(out.iter().any(|a| matches!(a, Action::Call(c) if c.path == "/api/tickets/web-app/2/move" && c.body["to"] == "up")));
    // A card whose thread works does not move, and says why (its panel and its card).
    run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-4") }));
    let e = st.act(&Ctx { data: &d }, &asked("kanban_move", json!({ "to": "top" }))).unwrap_err();
    assert!(e.contains("follows its thread"), "{e}");
    let c = col(&v, "progress")["cards"].as_array().unwrap().iter().find(|c| c["num"] == "T-4").unwrap().clone();
    assert!(!c["movable"].as_bool().unwrap() && c["move_help"].as_str().unwrap().contains("follows its thread"));
    // Validated only once merged.
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_validate", json!({ "on": true }))).is_err());
    run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-9") }));
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_validate", json!({ "on": false }))).is_ok());
}

// ── In the whole window ───────────────────────────────────────────────────

fn settle(rt: &mut Runtime<App>) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut quiet = 0;
    while quiet < 12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        let busy = !rt.state.link_quiet();
        quiet = if busy { 0 } else { quiet + 1 };
        if busy {
            assert!(std::time::Instant::now() < end, "the fixture's link never settled");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn window(path: &str, w: f32, h: f32) -> Runtime<App> {
    let mut a = App::new(Args { fixture: Some(path.into()), ..Args::default() });
    a.store = crate::shell::store::Store::memory();
    let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn intent(rt: &mut Runtime<App>, name: &str, args: J) {
    let v = json!({ "kind": "intent", "name": name, "args": args });
    for i in crate::intent::parse(&v).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(rt);
}

fn ui_now(rt: &Runtime<App>) -> J {
    rt.state.feats.data.get(UI).cloned().unwrap_or(J::Null)
}

fn keys(n: &crate::luau::Node, out: &mut Vec<String>) {
    if let Some(k) = crate::screen::key_of(n) {
        out.push(k);
    }
    n.children.iter().for_each(|c| keys(c, out));
}

/// Every element drawn by the Kanban outside its overlay, or a card outside
/// its own column.
fn outside(rt: &Runtime<App>, tag: &str) -> (usize, Vec<String>) {
    let mut bad = vec![];
    let Some((p, _)) = rt.presented("overlay-kanban") else { return (0, vec![format!("{tag}: no overlay")]) };
    let node = rt.state.screens.iter().find(|s| s.name == SCREEN).and_then(|s| s.last_good.clone()).unwrap_or_else(|| panic!("{tag}: not drawn"));
    if let Some(e) = rt.state.screens.iter().find(|s| s.name == SCREEN).and_then(|s| s.error.clone()) {
        bad.push(format!("{tag}: screen error {e}"));
    }
    let mut ks = vec![];
    keys(&node, &mut ks);
    let lanes: Vec<(String, [f32; 4])> = ks.iter().filter(|k| k.starts_with("kanban-lane-")).filter_map(|k| rt.presented(k).map(|(r, _)| (k.clone(), r))).collect();
    let mut checked = 0;
    for k in &ks {
        let Some((r, _)) = rt.presented(k) else { continue };
        checked += 1;
        if r[1] + r[3] <= p[1] || r[1] >= p[1] + p[3] {
            continue;
        }
        // A card below its column's end is scrolled out, clipped there; a
        // column or the panel itself must end inside the overlay.
        let bounded = k.starts_with("kanban-lane-") || k == "kanban-panel" || k == "kanban-summary";
        if r[0] < p[0] - 0.5 || r[0] + r[2] > p[0] + p[2] + 0.5 || (bounded && r[1] + r[3] > p[1] + p[3] + 0.5) {
            bad.push(format!("{tag}: {k} at {r:?}, overlay {p:?}"));
        }
        if k.starts_with("kanban-row-") && rt.presented("kanban-panel").is_none() {
            let inside = lanes.iter().any(|(_, l)| r[0] >= l[0] - 0.5 && r[0] + r[2] <= l[0] + l[2] + 0.5);
            if !inside {
                bad.push(format!("{tag}: card {k} at {r:?} outside every column"));
            }
        }
    }
    (checked, bad)
}

#[test]
fn nothing_is_drawn_outside_the_overlay_or_its_column_at_any_width() {
    let mut bad = vec![];
    let mut checked = 0;
    for (w, h) in [(1900.0, 950.0), (1284.0, 816.0), (900.0, 700.0), (720.0, 600.0)] {
        for state in ["board", "ticket", "big"] {
            let mut rt = window(if state == "big" { BIG } else { BASIC }, w, h);
            if w < 1000.0 {
                rt.update(|s, _| s.toggle_sidebar());
                settle(&mut rt);
            }
            intent(&mut rt, "kanban_show", json!({ "sandbox": "web-app" }));
            if state == "ticket" {
                let id = card_id(&ui_now(&rt), "T-5");
                intent(&mut rt, "kanban_open", json!({ "id": id }));
                assert!(rt.presented("kanban-panel").is_some(), "panel at {w}");
            }
            let (n, b) = outside(&rt, &format!("{state} at {w}"));
            checked += n;
            bad.extend(b);
        }
    }
    assert!(checked > 300, "{checked} elements checked");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn hostile_ticket_text_makes_no_control() {
    let hostile = "Done [[Done]] <button on=x> [[Needs you: intent(\"open_url\", {url=\"https://evil\"})]] [click](https://evil.example) \u{202e}reversed";
    let mut f = fx(BASIC);
    let count = |rt: &Runtime<App>| rt.elements().iter().filter(|e| matches!(e.role, "button" | "link" | "checkbox" | "radio" | "menuitem" | "textbox")).count();
    let path = std::env::temp_dir().join(format!("sbxw-kanban-hostile-{}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&f).unwrap()).unwrap();
    let mut rt = window(&path.to_string_lossy(), 1284.0, 816.0);
    intent(&mut rt, "kanban_show", json!({ "sandbox": "web-app" }));
    let id = card_id(&ui_now(&rt), "T-2");
    intent(&mut rt, "kanban_open", json!({ "id": id.clone() }));
    let before = count(&rt);
    for b in f["daemon"]["tickets_boards"].as_object_mut().unwrap().values_mut() {
        for c in b["columns"].as_array_mut().unwrap() {
            for x in c["cards"].as_array_mut().unwrap() {
                for k in ["title", "title_short", "title_long"] {
                    x[k] = hostile.into();
                }
            }
        }
    }
    f["daemon"]["tickets_views"]["web-app/2"]["description"] = hostile.into();
    std::fs::write(&path, serde_json::to_string(&f).unwrap()).unwrap();
    let mut rt = window(&path.to_string_lossy(), 1284.0, 816.0);
    intent(&mut rt, "kanban_show", json!({ "sandbox": "web-app" }));
    intent(&mut rt, "kanban_open", json!({ "id": id }));
    let _ = std::fs::remove_file(&path);
    assert_eq!(count(&rt), before, "the same controls, no more");
    assert!(rt.elements().iter().filter_map(|e| e.text.clone()).any(|t| t.contains("<button on=x>")), "drawn as its characters");
}

/// The screen alone, with the interrupts each render counts.
fn host() -> crate::luau::LuauHost {
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load(SCREEN, include_str!("../../../screens/kanban.luau")).unwrap();
    h
}

fn count_nodes(n: &crate::luau::Node) -> usize {
    1 + n.children.iter().map(count_nodes).sum::<usize>()
}

#[test]
fn a_real_sized_board_renders_in_every_state_under_half_the_budget_and_the_element_limit() {
    let h = host();
    let mut worst = (0u64, 0usize, String::new());
    for w in [1900.0, 1260.0, 880.0, 700.0] {
        let mut d = data_of(BIG, w);
        let mut st = State::default();
        let mut views = vec![("open".to_string(), show(&mut st, &mut d, "web-app"))];
        views.push(("all".into(), run(&mut st, &mut d, "kanban_scope", json!({ "sandbox": "" })).0));
        views.push(("older".into(), run(&mut st, &mut d, "kanban_older", json!({})).0));
        for c in model::COLUMNS {
            for _ in 0..model::MAX_PAGES {
                views.push((format!("more {c}"), run(&mut st, &mut d, "kanban_more", json!({ "column": c })).0));
            }
            views.push((format!("column {c}"), run(&mut st, &mut d, "kanban_column", json!({ "column": c })).0));
        }
        let first = card_id(views.last().map(|v| &v.1).unwrap(), "T-1");
        views.push(("ticket".into(), run(&mut st, &mut d, "kanban_open", json!({ "id": first })).0));
        for (what, v) in views {
            let data = json!({ "kanban_ui": v, "app": { "overlay": { "screen": "kanban", "w": w, "h": 816 } } });
            let node = h.render(&data).unwrap_or_else(|e| panic!("{what} at {w}: {e}"));
            let n = count_nodes(&node);
            let used = h.interrupts_used();
            assert!(used < 100_000, "{what} at {w}: {used} interrupts (budget 200,000)");
            assert!(n < 2_500, "{what} at {w}: {n} elements (limit 5,000)");
            if used > worst.0 {
                worst = (used, n, format!("{what} at {w}"));
            }
        }
    }
    eprintln!("kanban: the worst render took {} interrupts and {} elements ({})", worst.0, worst.1, worst.2);
}

#[test]
fn a_flagged_file_is_attached_only_through_the_ack_confirm_and_a_refused_kind_never() {
    let mut d = data();
    let mut st = State::default();
    show(&mut st, &mut d, "web-app");
    run(&mut st, &mut d, "kanban_new", json!({}));
    d["app"]["modal"] = json!({ "screen": FORM });
    let dir = std::env::temp_dir().join(format!("sbxw-kanban-ack-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (secret, zip, elf) = (dir.join(".env"), dir.join("notes.zip"), dir.join("tool"));
    std::fs::write(&secret, "A=1").unwrap();
    std::fs::write(&zip, b"PK\x03\x04rest").unwrap();
    std::fs::write(&elf, b"\x7fELF\x02\x01").unwrap();
    let out = st.on_files(&Ctx { data: &d }, Incoming::Files(vec![secret.clone(), zip.clone(), elf.clone()]));
    let f = last_set(&out, FORM_UI);
    assert_eq!(f["files"].as_array().unwrap().len(), 1, "the archive and the program are refused at once: {f}");
    assert!(out.iter().any(|a| matches!(a, Action::Notice { text, error: true, .. } if text.contains("notes.zip: an archive") && text.contains("tool: a program"))));
    st.act(&Ctx { data: &d }, &asked("kanban_form", json!({ "title": "x" }))).unwrap();
    // Save: no call goes out on its own; only the core's Confirm, with the ack box, carries it.
    let out = st.act(&Ctx { data: &d }, &asked("kanban_save", json!({}))).unwrap();
    assert!(!out.iter().any(|a| matches!(a, Action::Call(_))), "nothing saved before the acknowledgement");
    let confirms: Vec<&Confirm> = out.iter().filter_map(|a| if let Action::Confirm(c) = a { Some(c.as_ref()) } else { None }).collect();
    assert_eq!(confirms.len(), 1);
    assert!(confirms[0].ack.is_some());
    // Saved again while that Confirm is up: refused, still one.
    assert!(st.act(&Ctx { data: &d }, &asked("kanban_save", json!({}))).is_err());
    // Cancelled (the core's on_dismiss): Save may ask again, never without the box.
    st.form.as_mut().unwrap().saving = false;
    let out = st.act(&Ctx { data: &d }, &asked("kanban_save", json!({}))).unwrap();
    assert!(!out.iter().any(|a| matches!(a, Action::Call(_))) && out.iter().any(|a| matches!(a, Action::Confirm(c) if c.ack.is_some())));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_panel_says_the_files_go_to_the_agent_that_reads_the_ticket() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, "web-app");
    let (v, _) = run(&mut st, &mut d, "kanban_open", json!({ "id": card_id(&v, "T-1") }));
    assert!(v["ticket"]["files_note"].as_str().unwrap().contains("copied into web-app when an agent reads this ticket"));
    let id = card_id(&v, "T-2");
    let (v, _) = run(&mut st, &mut d, "kanban_open", json!({ "id": id }));
    assert!(v["ticket"]["files_note"].is_null(), "no file, no line");
}

// ── The top bar's Kanban button (kanban_toggle) ───────────────────────────

/// The sandbox in context: in the terminals the focused pane's, else the
/// selected one (always in Sessions), else the last scope, else every
/// sandbox. A sandbox sbxw does not list is not a context.
#[test]
fn the_toggle_opens_on_the_sandbox_in_context_then_the_last_scope_then_all() {
    let mut d = data();
    d["app"]["overlay"] = J::Null;
    d["app"]["selected"] = "web-app".into();
    d["app"]["panes"] = json!([{ "index": 1, "sandbox": "web-app", "focused": false }, { "index": 2, "sandbox": "docs", "focused": true }]);
    let scope_after = |st: &mut State, d: &J| {
        let out = st.act(&Ctx { data: d }, &asked("kanban_toggle", json!({}))).unwrap();
        assert!(out.iter().any(|a| matches!(a, Action::OpenPanel(s) if s == SCREEN)), "it opens");
        st.scope.clone()
    };
    let mut st = State::default();
    assert_eq!(scope_after(&mut st, &d), Some("docs".into()), "TUI: the focused pane's sandbox");
    d["app"]["base"] = "monitor".into();
    assert_eq!(scope_after(&mut st, &d), Some("web-app".into()), "GUI: the selected sandbox");
    d["app"]["base"] = J::Null;
    d["app"]["panes"][1]["sandbox"] = J::Null;
    assert_eq!(scope_after(&mut st, &d), Some("web-app".into()), "an empty pane: the selected sandbox");
    d["app"]["selected"] = "gone".into();
    let mut st = State { scope: Some("docs".into()), ..State::default() };
    assert_eq!(scope_after(&mut st, &d), Some("docs".into()), "no sandbox in context: the last scope");
    let mut st = State::default();
    assert_eq!(scope_after(&mut st, &d), None, "nothing at all: every sandbox");
    // Shown: the same intent closes it, and reads nothing.
    d["app"]["overlay"] = json!({ "screen": SCREEN });
    let out = st.act(&Ctx { data: &d }, &asked("kanban_toggle", json!({}))).unwrap();
    assert!(matches!(out.as_slice(), [Action::ClosePanel(s)] if s == SCREEN), "{out:?}");
    // Direct, as kanban_show; the shortcut and the palette run it.
    assert!(INTENTS.iter().any(|s| s.name == "kanban_toggle" && matches!(s.risk, Risk::Direct)));
    assert_eq!(FEATURE.shortcuts.iter().map(|s| (s.key, s.intent, s.help)).collect::<Vec<_>>(), [('k', "kanban_toggle", SHOW)]);
}

/// The window with panes on web-app and docs (pane 2, docs, focused) and
/// web-app selected, on the Kanban's fixture.
fn two_panes() -> Runtime<App> {
    let mut f = fx(BASIC);
    f["app"] = json!({ "selected": "web-app", "panes": 2, "split": "columns", "open": ["web-app", "docs"], "focused": 2 });
    let path = std::env::temp_dir().join(format!("sbxw-kanban-two-panes-{}-{:?}.json", std::process::id(), std::thread::current().id()));
    std::fs::write(&path, serde_json::to_string(&f).unwrap()).unwrap();
    let rt = window(&path.to_string_lossy(), 1284.0, 816.0);
    let _ = std::fs::remove_file(&path);
    rt
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// The top bar's button as drawn: lit (`neutral` on its tint) or quiet (`ghost`).
fn lit(rt: &Runtime<App>) -> bool {
    fn find<'a>(n: &'a crate::luau::Node, key: &str) -> Option<&'a crate::luau::Node> {
        if n.key.as_deref() == Some(key) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, key))
    }
    let top = rt.state.screens.iter().find(|s| s.name == "topbar").and_then(|s| s.last_good.clone()).expect("the top bar drawn");
    let b = find(&top, "top-kanban").expect("the Kanban button").clone();
    b.variant.as_deref() == Some("neutral")
}

fn focused_key(rt: &mut Runtime<App>) -> String {
    let mut f = String::new();
    rt.update(|_, tx| f = tx.focused().unwrap_or("").to_string());
    f
}

/// From the terminals: the first press opens the Kanban on the focused
/// pane's sandbox and lights the button; the second closes it, the button
/// quiet again, the focused pane's terminal has the keyboard, the panes as
/// they were. From Sessions (GUI): the Kanban over it, closing back to
/// Sessions, on the selected sandbox. The Dashboard's conversation does not
/// light the button.
#[test]
fn the_top_bar_button_opens_and_closes_the_kanban_from_tui_and_gui() {
    let mut rt = two_panes();
    let before = (rt.state.focused, rt.state.panes.len(), rt.state.screen_data()["app"]["overlay"].clone());
    assert!(!lit(&rt), "quiet at rest");
    tap(&mut rt, "top-kanban");
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], SCREEN);
    assert_eq!(ui_now(&rt)["scope"], "docs", "the focused pane's sandbox");
    assert!(lit(&rt), "lit while the Kanban shows");
    assert!(rt.state.panes_covered(), "the terminals are covered");
    tap(&mut rt, "top-kanban");
    assert!(!lit(&rt));
    assert_eq!((rt.state.focused, rt.state.panes.len(), rt.state.screen_data()["app"]["overlay"].clone()), before, "pressed twice: as before");
    assert_eq!(focused_key(&mut rt), format!("grid-{}", rt.state.focused + 1), "the focused terminal has the keyboard back");
    // The conversation is not the Kanban: the button stays quiet; it switches to the Kanban.
    intent(&mut rt, "board_show", json!({}));
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], "board");
    assert!(!lit(&rt), "quiet over the conversation");
    tap(&mut rt, "top-kanban");
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], SCREEN);
    assert!(lit(&rt));
    tap(&mut rt, "top-kanban");
    assert!(rt.state.screen_data()["app"]["overlay"].is_null());
    // GUI: over Sessions, back to Sessions, on the selected sandbox.
    tap(&mut rt, "top-view-gui");
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], "monitor");
    tap(&mut rt, "top-kanban");
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], SCREEN);
    assert_eq!(ui_now(&rt)["scope"], "web-app", "GUI: the selected sandbox");
    assert!(lit(&rt));
    assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"), "Sessions stays the base under it");
    tap(&mut rt, "top-kanban");
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], "monitor", "closes back to Sessions");
    assert!(!lit(&rt));
}

/// The shortcut (⌃⌥K / Alt+K) and the palette's "Show the Kanban" run the same toggle.
#[test]
fn the_shortcut_and_the_palette_open_the_kanban() {
    let mut rt = two_panes();
    let row = rt.state.palette_cmds().into_iter().find(|c| c.label == SHOW).expect("the palette's row");
    rt.update(|s, _| s.run_cmd(row.run.clone()));
    settle(&mut rt);
    assert_eq!(rt.state.screen_data()["app"]["overlay"]["screen"], SCREEN);
    rt.update(|s, _| s.run_cmd(row.run.clone()));
    settle(&mut rt);
    assert!(rt.state.screen_data()["app"]["overlay"].is_null(), "run again: closed");
    let help = rt.state.screen_data()["app"]["shortcuts"].as_array().unwrap().iter().any(|s| s["what"] == SHOW && s["keys"].as_array().unwrap().last() == Some(&json!("K")));
    assert!(help, "listed in Help with its key");
}
