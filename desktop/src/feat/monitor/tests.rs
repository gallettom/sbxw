use super::model::{Group, Model};
use super::*;
use crate::luau::{LuauHost, Limits, Node};
use serde_json::json;

/// The core's rule (`shell::features::admit`): an approval answers with its dialog only.
fn admit(risk: Risk, name: &str, a: Vec<Action>) -> Result<Vec<Action>, String> {
    match (risk, a.as_slice()) {
        (Risk::Direct, _) | (Risk::Approval(_), [Action::Confirm(_)] | [Action::Prepare(_)]) => Ok(a),
        _ => Err(format!("{name}: not the core's dialog")),
    }
}

const SRC: &str = include_str!("../../../screens/monitor.luau");

fn fixture(name: &str) -> J {
    let p = format!("{}/fixtures/monitor/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// What a feature sees: the fixture's daemon data, nothing over the view, the view shown.
fn data_of(name: &str) -> J {
    let mut d = fixture(name)["daemon"].clone();
    d["app"] = json!({ "covered": false, "overlay": { "screen": "monitor", "w": 1000 }, "panes": [], "pages": { "panes": [] }, "selected": null });
    d["jarvis"] = json!({ "pending": [] });
    d
}

fn run(d: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    let a = Asked { name, args: args.as_object().cloned().unwrap_or_default() };
    act(&Ctx { data: d }, &a)
}

fn view(acts: &[Action]) -> J {
    acts.iter().find_map(|a| match a {
        Action::Set { key, value } if key == KEY => Some(value.clone()),
        _ => None,
    }).expect("the view")
}

/// A fresh model for this test's thread, opened on the fixture.
fn opened(d: &mut J) -> J {
    *STATE().lock().unwrap() = Model::default();
    let v = view(&run(d, "monitor_open", json!({})).unwrap());
    d["monitor"] = v.clone();
    v
}

fn keys(v: &J) -> Vec<String> {
    v["rows"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap().to_string()).collect()
}

/// Anything that would reach an agent: a daemon call, a dialog, a pick.
fn sends(acts: &[Action]) -> bool {
    acts.iter().any(|a| matches!(a, Action::Call(_) | Action::Confirm(_) | Action::Prepare(_)) || matches!(a, Action::Intent { name, .. } if name == "agent_pick" || name == "approve_proposal"))
}

fn host() -> LuauHost {
    let mut h = LuauHost::new(Limits::default()).unwrap();
    // The rows built so far, drawn as a last text the test reads.
    let src = SRC.replace("return screen\n", "local r = screen.render\nscreen.render = function(s, d) return column{ r(s, d), text{ \"built \" .. built, key = \"test-built\" } } end\nreturn screen\n");
    h.load("monitor", &src).unwrap();
    h
}

fn walk<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    out.push(n);
    for c in &n.children {
        walk(c, out);
    }
}

fn find<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
    let mut all = vec![];
    walk(n, &mut all);
    all.into_iter().find(|x| x.key.as_deref() == Some(key) || x.id.as_deref() == Some(key))
}

fn built(n: &Node) -> u64 {
    find(n, "test-built").unwrap().text.trim_start_matches("built ").parse().unwrap()
}

/// Every control drawn: what it is, its key, its action.
fn controls(n: &Node) -> Vec<String> {
    let mut all = vec![];
    walk(n, &mut all);
    all.iter().filter(|x| x.on.is_some() || x.on_submit.is_some() || x.id.is_some()).map(|x| format!("{:?} {} {:?} {:?}", x.kind, x.key.clone().unwrap_or_default(), x.on, x.on_submit)).collect()
}

#[test]
fn the_order_is_who_needs_you_first() {
    let mut d = data_of("basic");
    let v = opened(&mut d);
    let groups: Vec<&str> = v["rows"].as_array().unwrap().iter().map(|r| r["group"].as_str().unwrap()).collect();
    assert_eq!(groups, ["you", "you", "working", "working", "working", "idle", "idle", "idle"]);
    // Needs you: the longest waiting first; the others: the latest report first.
    assert_eq!(
        keys(&v),
        ["shop-web::checkout-copy", "shop-web:s-shop-main", "docs-site:s-docs-main", "shop-web::tax-rules", "billing-api::retry-queue", "billing-api:s-bill-main", "data-pipeline:s-pipe-main", "chat-research:s-chat"]
    );
    assert_eq!(v["counts"], json!({ "you": 2, "working": 3, "idle": 3, "ended": 0, "total": 8 }));
    // An ended session goes last; Jarvis's sandbox is not listed.
    let mut m = STATE().lock().unwrap();
    m.upsert(&json!({ "sandbox": "billing-api", "thread": "retry-queue", "state": "exited" }));
    m.upsert(&json!({ "sandbox": "sbxw-jarvis", "mode": "claude", "state": "attention", "started_ms": 1, "session_id": "j", "ts": 9 }));
    let o = m.order();
    assert_eq!(o.last().unwrap(), "billing-api::retry-queue");
    assert_eq!(o.len(), 8);
    assert_eq!(Group::of("exited"), Group::Ended);
}

#[test]
fn an_event_changes_one_card_and_the_screen_builds_one_row_again() {
    let mut d = data_of("basic");
    let v = opened(&mut d);
    let h = host();
    let n = h.render(&d).unwrap();
    assert_eq!(built(&n), 8, "every row built the first time");
    // The same data: nothing built again.
    assert_eq!(built(&h.render(&d).unwrap()), 8);
    // A working session moves on to its next step.
    let mut info = d["sessions"].as_array().unwrap().iter().find(|i| i["session_id"] == "s-docs-main").unwrap().clone();
    info["activity"] = "Edit guides/install.md".into();
    let acts = on_event(&Ctx { data: &d }, "session", &info);
    let w = view(&acts);
    let rev = |v: &J, k: &str| v["rows"].as_array().unwrap().iter().find(|r| r["key"] == k).unwrap()["rev"].clone();
    for k in keys(&v) {
        if k == "docs-site:s-docs-main" {
            assert_ne!(rev(&v, &k), rev(&w, &k), "the card that changed has a new rev");
        } else {
            assert_eq!(rev(&v, &k), rev(&w, &k), "{k} did not change");
            let (a, b) = (v["rows"].as_array().unwrap().iter().find(|r| r["key"] == k.as_str()), w["rows"].as_array().unwrap().iter().find(|r| r["key"] == k.as_str()));
            assert_eq!(a, b, "{k}'s row is the same JSON");
        }
    }
    d["monitor"] = w;
    let n = h.render(&d).unwrap();
    assert_eq!(built(&n), 9, "one row built again, not eight");
    let row = find(&n, "monitor-row-docs-site:s-docs-main").unwrap();
    let mut all = vec![];
    walk(row, &mut all);
    assert!(all.iter().any(|x| x.text == "Edit guides/install.md"));
    // The same report again changes nothing, and hands no view over.
    assert!(on_event(&Ctx { data: &d }, "session", &info).is_empty());
    // While the view is closed, the card changes but no view is pushed.
    d["app"]["overlay"] = J::Null;
    info["activity"] = "Edit guides/faq.md".into();
    assert!(on_event(&Ctx { data: &d }, "session", &info).is_empty());
    assert_eq!(STATE().lock().unwrap().cards["docs-site:s-docs-main"].info["activity"], "Edit guides/faq.md");
}

#[test]
fn each_state_says_what_it_is() {
    // No session at all.
    let mut d = data_of("empty");
    opened(&mut d);
    let h = host();
    let n = h.render(&d).unwrap();
    assert!(find(&n, "monitor-empty").is_some());
    assert!(find(&n, "monitor-load").is_none());
    // Not opened through the feature yet: a way to load it.
    d["monitor"] = J::Null;
    assert!(find(&h.render(&d).unwrap(), "monitor-load").is_some());
    // Everything idle: nothing needs you, and the composer is open.
    let mut d = data_of("idle");
    let v = opened(&mut d);
    assert_eq!(v["counts"]["you"], 0);
    let n = host().render(&d).unwrap();
    assert!(find(&n, "monitor-calm").is_some() && find(&n, "monitor-next").is_none());
    assert!(!find(&n, "monitor-compose").unwrap().disabled);
    // Many waiting, more than a page: the count, the next one, the rest paged.
    let mut d = data_of("busy");
    let v = opened(&mut d);
    assert_eq!(v["counts"]["you"], 9);
    assert_eq!(v["rows"].as_array().unwrap().len(), model::PAGE);
    assert_eq!(v["count"]["total"], 64);
    let n = host().render(&d).unwrap();
    assert_eq!(find(&n, "monitor-next").unwrap().text, "9 need you");
    assert!(find(&n, "monitor-more").is_some());
    let w = view(&run(&d, "monitor_more", json!({})).unwrap());
    assert_eq!(w["rows"].as_array().unwrap().len(), 64);
    // N goes through those who need you, round.
    let first = v["sel"].clone();
    let mut seen = vec![];
    for _ in 0..9 {
        let w = view(&run(&d, "monitor_key", json!({ "key": "type", "text": "n" })).unwrap());
        assert_eq!(w["detail"]["state"], "attention");
        seen.push(w["sel"].clone());
    }
    seen.sort_by_key(|k| k.to_string());
    seen.dedup();
    assert_eq!(seen.len(), 9, "each of the nine once");
    assert_eq!(first["sel"], J::Null);
    // A narrow overlay: the list alone.
    d["app"]["overlay"]["w"] = 600.into();
    let n = host().render(&d).unwrap();
    assert!(find(&n, "monitor-detail").is_none() && find(&n, "monitor-list").is_some());
}

#[test]
fn the_keys_move_the_selection_and_enter_reaches_the_list() {
    let mut d = data_of("basic");
    let v = opened(&mut d);
    assert_eq!(v["sel"], "shop-web::checkout-copy");
    let w = view(&run(&d, "monitor_key", json!({ "key": "down" })).unwrap());
    assert_eq!(w["sel"], "shop-web:s-shop-main");
    let w = view(&run(&d, "monitor_key", json!({ "key": "up" })).unwrap());
    assert_eq!(w["sel"], "shop-web::checkout-copy");
    let w = view(&run(&d, "monitor_key", json!({ "key": "up" })).unwrap());
    assert_eq!(w["sel"], "shop-web::checkout-copy", "stops at the top");
    assert_eq!(run(&d, "monitor_key", json!({ "key": "enter" })).unwrap(), [Action::Focus("monitor-row-shop-web::checkout-copy".into())]);
    // The sidebar's sandbox picks its most urgent session when the view opens.
    *STATE().lock().unwrap() = Model::default();
    d["app"]["selected"] = "billing-api".into();
    let v = view(&run(&d, "monitor_open", json!({})).unwrap());
    assert_eq!(v["sel"], "billing-api::retry-queue");
}

#[test]
fn the_links_go_through_the_intents_that_own_them() {
    let mut d = data_of("basic");
    opened(&mut d);
    let i = |n: &str, a: J| Action::Intent { name: n.into(), args: a };
    // A thread on its worktree.
    let k = json!({ "key": "shop-web::tax-rules" });
    assert_eq!(run(&d, "monitor_commits", k.clone()).unwrap(), [i("commits_show", json!({ "sandbox": "shop-web", "branch": "sbxw/tax-rules" }))]);
    assert_eq!(run(&d, "monitor_files", k.clone()).unwrap(), [i("files_open", json!({ "sandbox": "shop-web", "worktree": "tax-rules" }))]);
    assert_eq!(run(&d, "monitor_worktrees", k.clone()).unwrap(), [i("worktrees_show", json!({})), i("worktrees_filter", json!({ "text": "tax-rules" }))]);
    assert_eq!(run(&d, "monitor_terminal", k).unwrap(), [Action::ClosePanel("monitor".into()), i("thread_open", json!({ "sandbox": "shop-web", "name": "tax-rules" }))]);
    // The main session: the sandbox's own.
    let k = json!({ "key": "billing-api:s-bill-main" });
    assert_eq!(run(&d, "monitor_commits", k.clone()).unwrap(), [i("commits_show", json!({ "sandbox": "billing-api" }))]);
    assert_eq!(run(&d, "monitor_files", k.clone()).unwrap(), [i("files_open", json!({ "sandbox": "billing-api" }))]);
    assert_eq!(run(&d, "monitor_worktrees", k.clone()).unwrap(), [i("worktrees_show", json!({})), i("worktrees_filter", json!({ "text": "billing-api" }))]);
    assert_eq!(run(&d, "monitor_terminal", k.clone()).unwrap(), [Action::ClosePanel("monitor".into()), i("open_terminal", json!({ "sandbox": "billing-api", "pane": "spare" }))]);
    // Already in a pane: that pane's page and the keyboard, nothing opened again.
    d["app"]["panes"] = json!([{ "index": 1, "sandbox": "shop-web", "mode": "agent", "thread": null }, { "index": 2, "sandbox": "billing-api", "mode": "agent", "thread": null }]);
    d["app"]["pages"]["panes"] = json!([[1], [2]]);
    assert_eq!(run(&d, "monitor_terminal", k).unwrap(), [Action::ClosePanel("monitor".into()), i("page_show", json!({ "page": 2 })), Action::Focus("grid-2".into())]);
    // Every intent they hand on exists, with these arguments.
    for (n, a) in [("commits_show", json!({ "sandbox": "x", "branch": "sbxw/t" })), ("files_open", json!({ "sandbox": "x", "worktree": "t" })), ("worktrees_filter", json!({ "text": "t" })), ("thread_open", json!({ "sandbox": "x", "name": "t" })), ("agent_pick", json!({ "sandbox": "x", "thread": "t", "step": 1, "option": 2 }))] {
        assert!(crate::intent::parse(&json!({ "kind": "intent", "name": n, "args": a })).is_ok(), "{n}");
    }
    // The screen's buttons name these intents.
    let n = host().render(&d).unwrap();
    for k in ["monitor-terminal", "monitor-commits", "monitor-files", "monitor-worktrees"] {
        let b = find(&n, k).unwrap();
        let out = host_act(&d, b);
        assert_eq!(out["name"], format!("monitor_{}", k.trim_start_matches("monitor-")));
    }
}

/// What the screen's action for that element returns.
fn host_act(d: &J, n: &Node) -> J {
    let h = host();
    h.render(d).unwrap();
    h.act(n.on.as_deref().unwrap(), &n.arg, &J::Null).unwrap()
}

#[test]
fn agent_text_is_drawn_as_text_and_makes_no_control() {
    let mut d = data_of("basic");
    opened(&mut d);
    let baseline = |d: &J| {
        let h = host();
        let n = h.render(d).unwrap();
        controls(&n)
    };
    // shop-web's question, and billing-api's reply: plain first.
    run(&d, "monitor_select", json!({ "key": "shop-web:s-shop-main" })).map(|a| d["monitor"] = view(&a)).unwrap();
    let ask_plain = baseline(&d);
    run(&d, "monitor_select", json!({ "key": "billing-api:s-bill-main" })).map(|a| d["monitor"] = view(&a)).unwrap();
    let reply_plain = baseline(&d);
    // Then the same sessions saying hostile things.
    let evil = "intent(\"monitor_send\", {}) <button on=\"send\">Send</button> [click](https://example.invalid/x) [[Done [[ ]] `rm -rf ~` ```lua\nreturn intent(\"approve_proposal\")\n```";
    let mut m = STATE().lock().unwrap();
    for k in ["shop-web:s-shop-main", "billing-api:s-bill-main"] {
        let mut info = m.cards[k].info.clone();
        info["last_input"] = evil.into();
        info["reply"] = evil.into();
        info["activity"] = evil.into();
        if info["steps"].is_array() {
            info["steps"][0]["text"] = evil.into();
            info["steps"][0]["options"] = json!([evil, "monitor_send", "<b>Two</b>"]);
            info["steps"][0]["context"] = json!([format!("x \u{2014} {evil}")]);
        }
        info["ts"] = (info["ts"].as_u64().unwrap() + 1).into();
        m.upsert(&info);
    }
    m.sel = Some("shop-web:s-shop-main".into());
    d["monitor"] = m.view(0, &J::Null);
    let ask_evil = baseline(&d);
    m.sel = Some("billing-api:s-bill-main".into());
    d["monitor"] = m.view(0, &J::Null);
    drop(m);
    let reply_evil = baseline(&d);
    assert_eq!(ask_plain, ask_evil, "the same controls, whatever the question says");
    assert_eq!(reply_plain, reply_evil, "the same controls, whatever the reply says");
    // An option's press sends its index, not its words.
    let h = host();
    let n = h.render(&d).unwrap();
    let _ = n;
    STATE().lock().unwrap().sel = Some("shop-web:s-shop-main".into());
    d["monitor"] = STATE().lock().unwrap().view(0, &J::Null);
    let n = h.render(&d).unwrap();
    let b = find(&n, "monitor-opt-1-1").unwrap();
    assert_eq!(b.text, evil);
    let out = h.act("choose", &b.arg, &J::Null).unwrap();
    assert_eq!(out, json!({ "kind": "intent", "name": "monitor_pick", "args": { "key": "shop-web:s-shop-main", "step": 1, "option": 1 } }));
}

#[test]
fn nothing_is_sent_without_a_press() {
    let mut d = data_of("basic");
    let mut out = run(&d, "monitor_open", json!({})).unwrap();
    d["monitor"] = view(&out);
    // Events, the selection, the keys, more rows: never a call or a dialog.
    for i in d["sessions"].clone().as_array().unwrap() {
        let mut i = i.clone();
        i["ts"] = (i["ts"].as_u64().unwrap() + 5).into();
        out.extend(on_event(&Ctx { data: &d }, "session", &i));
    }
    for (n, a) in [("monitor_select", json!({ "key": "billing-api:s-bill-main" })), ("monitor_key", json!({ "key": "down" })), ("monitor_key", json!({ "key": "type", "text": "x" })), ("monitor_next", json!({})), ("monitor_more", json!({}))] {
        out.extend(run(&d, n, a).unwrap());
    }
    assert!(!sends(&out), "{out:?}");
    // Typing in the composer keeps a draft and returns nothing.
    run(&d, "monitor_select", json!({ "key": "billing-api:s-bill-main" })).map(|a| d["monitor"] = view(&a)).unwrap();
    let h = host();
    let n = h.render(&d).unwrap();
    let ta = find(&n, "monitor-compose").unwrap();
    assert!(!ta.disabled);
    assert_eq!(h.act("typed", &ta.arg, &"Ship it".into()).unwrap(), J::Null);
    // Send is the press: the intent, then the core's dialog after masking.
    let n = h.render(&d).unwrap();
    let send_b = find(&n, "monitor-send").unwrap();
    assert!(!send_b.disabled);
    let it = h.act("send", &send_b.arg, &J::Null).unwrap();
    assert_eq!(it["name"], "monitor_send");
    assert_eq!(it["args"]["text"], "Ship it");
    let a = run(&d, "monitor_send", it["args"].clone()).unwrap();
    let a = admit(Risk::Approval("x"), "monitor_send", a).expect("only the core's dialog");
    let Action::Prepare(p) = &a[0] else { panic!("{a:?}") };
    assert_eq!(p.call.path, "/api/voice/anonymise");
    let c = (p.build)(&Ctx { data: &d }, &p.asked, &json!({ "ok": true, "text": "Ship it", "masked": 0 })).unwrap();
    assert_eq!(c.call.path, "/api/chat/push");
    assert_eq!(c.call.body, json!({ "text": "Ship it", "name": "billing-api" }), "the name is always given: no chat sandbox is made");
    assert!(c.title.contains("billing-api · main"));
    assert_eq!(c.preview.as_ref().unwrap().text, "Ship it");
    // A thread's words go to the thread.
    let a = run(&d, "monitor_send", json!({ "key": "shop-web::tax-rules", "text": "Go on" })).unwrap();
    let Action::Prepare(p) = &a[0] else { panic!() };
    let c = (p.build)(&Ctx { data: &d }, &p.asked, &json!({ "ok": true, "text": "Go on" })).unwrap();
    assert_eq!(c.call.body, json!({ "text": "Go on", "name": "shop-web", "thread": "tax-rules" }));
    // Its answer moves `sent`: the screen empties the draft.
    let back = on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "ok": true }))));
    let w = view(&back);
    assert_eq!(STATE().lock().unwrap().sent["shop-web::tax-rules"], 1);
    let _ = w;
}

#[test]
fn a_session_waiting_on_a_menu_takes_no_typed_words() {
    let mut d = data_of("basic");
    opened(&mut d);
    for k in ["shop-web:s-shop-main", "shop-web::checkout-copy"] {
        let e = run(&d, "monitor_send", json!({ "key": k, "text": "1" })).unwrap_err();
        assert!(e.contains("answer that first"), "{e}");
        run(&d, "monitor_select", json!({ "key": k })).map(|a| d["monitor"] = view(&a)).unwrap();
        let n = host().render(&d).unwrap();
        assert!(find(&n, "monitor-compose").unwrap().disabled);
        assert!(find(&n, "monitor-send").unwrap().disabled);
        assert!(find(&n, "monitor-target").unwrap().text.starts_with("shop-web · "), "the target stays shown");
    }
    // The permission prompt: no button allows or refuses; its terminal does.
    let n = host().render(&d).unwrap();
    let card = find(&n, "monitor-permission").unwrap();
    let mut all = vec![];
    walk(card, &mut all);
    let ctl: Vec<_> = all.iter().filter(|x| x.on.is_some()).map(|x| x.key.clone().unwrap_or_default()).collect();
    assert_eq!(ctl, ["monitor-permission-terminal"]);
    // A session the daemon cannot answer: no option button, its terminal instead.
    let e = run(&d, "monitor_pick", json!({ "key": "shop-web::checkout-copy", "step": 1, "option": 1 })).unwrap_err();
    assert!(e.contains("not waiting") || e.contains("terminal"), "{e}");
    // A pick for the answerable one is the relay's own path.
    let a = run(&d, "monitor_pick", json!({ "key": "shop-web:s-shop-main", "step": 1, "option": 2 })).unwrap();
    assert_eq!(a, [Action::Intent { name: "agent_pick".into(), args: json!({ "sandbox": "shop-web", "step": 1, "option": 2 }) }]);
}

#[test]
fn nothing_is_sent_under_a_dialog_or_a_proposal() {
    let mut d = data_of("basic");
    opened(&mut d);
    let pick = json!({ "key": "shop-web:s-shop-main", "step": 1, "option": 1 });
    let send = json!({ "key": "billing-api:s-bill-main", "text": "hello" });
    assert!(run(&d, "monitor_pick", pick.clone()).is_ok());
    assert!(run(&d, "monitor_send", send.clone()).is_ok());
    for (what, f) in [
        ("the core's dialog, a modal, the palette or a shown proposal", (|d: &mut J| d["app"]["covered"] = true.into()) as fn(&mut J)),
        ("a Jarvis proposal waiting", |d: &mut J| d["jarvis"]["pending"] = json!([{ "id": "p-1", "summary": "x", "risk": "high" }])),
        ("no word from the core on what covers the view", |d: &mut J| d["app"]["covered"] = J::Null),
    ] {
        let mut d2 = d.clone();
        f(&mut d2);
        let e = run(&d2, "monitor_pick", pick.clone()).unwrap_err();
        assert!(e.starts_with("Not sent"), "{what}: {e}");
        let e = run(&d2, "monitor_send", send.clone()).unwrap_err();
        assert!(e.starts_with("Not sent"), "{what}: {e}");
    }
    // The screen's look follows `covered` (the rule is the Rust's, above).
    let mut d2 = d.clone();
    run(&d2, "monitor_select", json!({ "key": "billing-api:s-bill-main" })).map(|a| d2["monitor"] = view(&a)).unwrap();
    d2["app"]["covered"] = true.into();
    let h = host();
    let ta = find(&h.render(&d2).unwrap(), "monitor-compose").unwrap().clone();
    h.act("typed", &ta.arg, &"x".into()).unwrap();
    assert!(find(&h.render(&d2).unwrap(), "monitor-send").unwrap().disabled);
}

#[test]
fn the_rail_entry_says_who_needs_you() {
    let d = data_of("basic");
    let x = extras(&Ctx { data: &d });
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].place, Place::Rail { zone: Zone::Activity, order: 5 });
    assert_eq!(x[0].help, "Every session in one view. 2 need you, 3 working");
    assert_eq!(x[0].count, None, "the Requests entry counts what waits");
    assert!(crate::icons::exists("inbox"));
    let d = data_of("empty");
    assert!(extras(&Ctx { data: &d })[0].help.contains("None is running"));
}

/// The whole window: with Jarvis's high-risk proposal up, Enter in the
/// composer and a press on a choice send nothing.
#[test]
fn with_a_jarvis_proposal_up_the_composer_and_the_choices_send_nothing() {
    use crate::shell::{view as shell_view, App};
    use rune::{Key, KeyEvent};
    let fx = format!("{}/fixtures/monitor/gate.json", env!("CARGO_MANIFEST_DIR"));
    let a = App::new(crate::shell::Args { fixture: Some(fx), ..Default::default() });
    let mut rt = rune::Runtime::new(a, shell_view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut rune::Tx| s.on_key(ev, tx));
    rt.frame(0.0);
    let frames = |rt: &mut rune::Runtime<App>, n: usize| {
        for _ in 0..n {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    };
    let tap = |rt: &mut rune::Runtime<App>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.016);
    };
    // No proposal shown yet: the view opens, billing-api's main session selected, words typed.
    rt.update(|s, _| s.jarvis.shown = None);
    frames(&mut rt, 4);
    tap(&mut rt, "rail-monitor");
    frames(&mut rt, 6);
    tap(&mut rt, "monitor-row-billing-api:s-bill-main");
    frames(&mut rt, 4);
    rt.update(|_, tx| tx.focus("monitor-compose", false));
    for c in "hello".chars() {
        rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: rune::Mods::default() });
        rt.frame(0.02);
    }
    let typed = |rt: &rune::Runtime<App>| rt.state.screens.iter().find(|x| x.name == "monitor").and_then(|sc| sc.ui.edits.get("monitor-compose").map(|e| e.0.text.clone()));
    assert_eq!(typed(&rt).as_deref(), Some("hello"));
    frames(&mut rt, 2);
    // Jarvis's proposal comes up; the composer keeps the keyboard.
    rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
    frames(&mut rt, 2);
    rt.update(|_, tx| assert_eq!(tx.focused(), Some("monitor-compose"), "the composer kept the keyboard"));
    // Cmd+Return (Ctrl+Return off macOS): the composer's submit, held as the OS reports it.
    let cmd = rune::Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), logo: cfg!(target_os = "macos"), ..Default::default() };
    rt.physical_input(rune::PhysicalInput::Modifiers(cmd));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: cmd });
    frames(&mut rt, 2);
    rt.physical_input(rune::PhysicalInput::Modifiers(rune::Mods::default()));
    frames(&mut rt, 2);
    assert_eq!(typed(&rt).as_deref(), Some("hello"), "a submit, not a new line");
    assert!(rt.state.feats.dialog.is_none(), "no dialog opened to send words");
    assert!(rt.state.notice.as_ref().is_some_and(|n| n.0.contains("Not sent")), "{:?}", rt.state.notice);
    // A choice, by the keyboard, with the proposal still up.
    rt.update(|s, _| s.jarvis.shown = None);
    frames(&mut rt, 2);
    tap(&mut rt, "monitor-row-shop-web:s-shop-main");
    frames(&mut rt, 4);
    assert!(rt.find("monitor-opt-1-1").is_some(), "the question's options are drawn");
    // The option has the keyboard. With the proposal waiting (not shown),
    // Space presses it and the monitor's Rust refuses: the press is real.
    rt.update(|_, tx| tx.focus("monitor-opt-1-1", false));
    frames(&mut rt, 1);
    let space = KeyEvent { key: Key::Text(" ".into()), mods: rune::Mods::default() };
    rt.key(space.clone());
    frames(&mut rt, 2);
    assert!(rt.state.notice.as_ref().is_some_and(|n| n.0.contains("Not sent")), "the press reached the monitor and was refused: {:?}", rt.state.notice);
    // Then the proposal shows: the option keeps the keyboard; Enter and Space send nothing.
    rt.update(|s, _| {
        s.jarvis.shown = Some("p-high-1".into());
        s.notice = None;
    });
    frames(&mut rt, 2);
    rt.update(|_, tx| assert_eq!(tx.focused(), Some("monitor-opt-1-1")));
    rt.key(KeyEvent { key: Key::Enter, mods: rune::Mods::default() });
    frames(&mut rt, 2);
    rt.key(space);
    frames(&mut rt, 2);
    assert!(rt.state.notice.as_ref().is_none_or(|n| n.0.contains("Not sent")), "{:?}", rt.state.notice);
    // The pointer meets the proposal's scrim.
    if let Some((x, y)) = rt.find("monitor-opt-1-2") {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }
    frames(&mut rt, 4);
    let sending = rt.state.screen_data()["relay_ui"]["sending"].clone();
    assert!(sending.as_object().is_none_or(|o| o.is_empty()), "an answer went out: {sending}");
    let picks = rt.state.screen_data()["relay_ui"]["picks"].clone();
    assert!(picks.as_object().is_none_or(|o| o.is_empty()), "a pick was made: {picks}");
    assert!(rt.state.feats.dialog.is_none());
}

/// The whole window: the rail opens the view with the keyboard on the list;
/// Up and Down move along it, Enter shows the session, N goes to the next
/// one that needs you, its terminal closes the view, Escape too.
#[test]
fn the_keyboard_walks_the_list() {
    use crate::shell::{view as shell_view, App};
    use rune::{Key, KeyEvent};
    let fx = format!("{}/fixtures/monitor/basic.json", env!("CARGO_MANIFEST_DIR"));
    let a = App::new(crate::shell::Args { fixture: Some(fx), ..Default::default() });
    let mut rt = rune::Runtime::new(a, shell_view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut rune::Tx| s.on_key(ev, tx));
    rt.frame(0.0);
    let frames = |rt: &mut rune::Runtime<App>, n: usize| {
        for _ in 0..n {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    };
    let key = |rt: &mut rune::Runtime<App>, k: Key| {
        rt.key(KeyEvent { key: k, mods: rune::Mods::default() });
        rt.frame(0.02);
    };
    let sel = |rt: &rune::Runtime<App>| rt.state.screen_data()["monitor"]["sel"].as_str().unwrap_or("").to_string();
    frames(&mut rt, 4);
    let (x, y) = rt.find("rail-monitor").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 6);
    assert_eq!(sel(&rt), "shop-web::checkout-copy");
    rt.update(|_, tx| assert_eq!(tx.focused(), Some("monitor-row-shop-web::checkout-copy"), "the keyboard is on the list"));
    let focused = |rt: &mut rune::Runtime<App>| {
        let mut f = String::new();
        rt.update(|_, tx| f = tx.focused().unwrap_or("").to_string());
        f
    };
    // Up and Down move the keyboard along the rows; Enter shows that session.
    key(&mut rt, Key::Down);
    frames(&mut rt, 2);
    key(&mut rt, Key::Down);
    frames(&mut rt, 2);
    assert_eq!(focused(&mut rt), "monitor-row-docs-site:s-docs-main", "two rows down");
    assert_eq!(sel(&rt), "shop-web::checkout-copy", "moving the keyboard shows nothing yet");
    key(&mut rt, Key::Enter);
    frames(&mut rt, 3);
    assert_eq!(sel(&rt), "docs-site:s-docs-main", "Enter shows it");
    key(&mut rt, Key::Up);
    frames(&mut rt, 2);
    assert_eq!(focused(&mut rt), "monitor-row-shop-web:s-shop-main");
    // N: the next session that needs you, from anywhere in the list.
    key(&mut rt, Key::Text("n".into()));
    frames(&mut rt, 3);
    assert_eq!(rt.state.screen_data()["monitor"]["detail"]["state"], "attention", "N: a session that needs you");
    let n1 = sel(&rt);
    key(&mut rt, Key::Text("n".into()));
    frames(&mut rt, 3);
    assert_ne!(sel(&rt), n1, "N again: the other one");
    // Its terminal: the view gives way.
    let (x, y) = rt.find("monitor-terminal").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 3);
    assert!(rt.state.feats.overlay.is_none(), "the terminal opened and the view closed");
    let last = sel(&rt);
    let (sb, th) = last.split_once("::").map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or_else(|| (last.split(':').next().unwrap().to_string(), None));
    assert!(rt.state.panes.iter().any(|p| p.sandbox.as_deref() == Some(sb.as_str()) && p.thread == th), "its terminal is in a pane: {last}");
    // Back from the rail: the same session; Escape closes.
    let (x, y) = rt.find("rail-monitor").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 6);
    assert_eq!(sel(&rt), last, "the view comes back on the same session");
    key(&mut rt, Key::Escape);
    frames(&mut rt, 2);
    assert!(rt.state.feats.overlay.is_none(), "Escape closes the view");
}

#[test]
fn the_intent_log_never_carries_the_words() {
    let marker = "MARKER-7f3a secret words";
    let a = Asked { name: "monitor_send", args: json!({ "key": "billing-api:s-bill-main", "text": marker }).as_object().cloned().unwrap() };
    let line = crate::shell::intent_log_line("monitor", &crate::intent::Intent::Feature(a));
    assert!(!line.contains("MARKER"), "{line}");
    assert!(!line.contains("billing-api"), "no value at all: {line}");
    assert_eq!(line, "intent from monitor: monitor_send(key, text)");
    // A core intent: its name only.
    let core = crate::intent::parse(&json!({ "kind": "intent", "name": "copy_text", "args": { "text": marker } })).unwrap();
    let line = crate::shell::intent_log_line("x", &core[0]);
    assert_eq!(line, "intent from x: copy_text");
}
