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

/// The actions without Sessions' history reads and its subscription's delete
/// (they reach the daemon's history store, never an agent).
fn no_hist(acts: Vec<Action>) -> Vec<Action> {
    acts.into_iter().filter(|a| !matches!(a, Action::Call(c) if c.tag.starts_with("hist:"))).collect()
}

/// Anything that would reach an agent: a daemon call (not a history read), a dialog, a pick.
fn sends(acts: &[Action]) -> bool {
    acts.iter().any(|a| matches!(a, Action::Call(c) if !c.tag.starts_with("hist:")) || matches!(a, Action::Confirm(_) | Action::Prepare(_)) || matches!(a, Action::Intent { name, .. } if name == "agent_pick" || name == "approve_proposal"))
}

fn host() -> LuauHost {
    let mut h = LuauHost::new(Limits::default()).unwrap();
    // The rows built so far, drawn as a last text the test reads.
    let src = SRC.replace("return screen\n", "local r = screen.render\nscreen.render = function(s, d) return column{ r(s, d), text{ \"built \" .. built, key = \"test-built\" }, text{ \"conv \" .. cbuilt, key = \"test-conv-built\" } } end\nreturn screen\n");
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
    // One line a row: the activity is the row's tooltip (and the detail's "Now").
    assert_eq!(row.help.as_deref(), Some("Edit guides/install.md"));
    // The same report again changes nothing, and hands no view over.
    assert!(no_hist(on_event(&Ctx { data: &d }, "session", &info)).is_empty());
    // While the view is closed, the card changes but no view is pushed.
    d["app"]["overlay"] = J::Null;
    info["activity"] = "Edit guides/faq.md".into();
    assert!(no_hist(on_event(&Ctx { data: &d }, "session", &info)).is_empty());
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
    assert_eq!(run(&d, "monitor_worktrees", k.clone()).unwrap(), [i("worktrees_show", json!({ "sandbox": "shop-web" })), i("worktrees_filter", json!({ "text": "tax-rules" }))]);
    assert_eq!(no_hist(run(&d, "monitor_terminal", k).unwrap()), [Action::ClosePanel("monitor".into()), i("thread_open", json!({ "sandbox": "shop-web", "name": "tax-rules" }))]);
    // The main session: the sandbox's own.
    let k = json!({ "key": "billing-api:s-bill-main" });
    assert_eq!(run(&d, "monitor_commits", k.clone()).unwrap(), [i("commits_show", json!({ "sandbox": "billing-api" }))]);
    assert_eq!(run(&d, "monitor_files", k.clone()).unwrap(), [i("files_open", json!({ "sandbox": "billing-api" }))]);
    assert_eq!(run(&d, "monitor_worktrees", k.clone()).unwrap(), [i("worktrees_show", json!({ "sandbox": "billing-api" })), i("worktrees_filter", json!({ "text": "" }))]);
    assert_eq!(no_hist(run(&d, "monitor_terminal", k.clone()).unwrap()), [Action::ClosePanel("monitor".into()), i("open_terminal", json!({ "sandbox": "billing-api", "pane": "spare" }))]);
    // Already in a pane: that pane's page and the keyboard, nothing opened again.
    d["app"]["panes"] = json!([{ "index": 1, "sandbox": "shop-web", "mode": "agent", "thread": null }, { "index": 2, "sandbox": "billing-api", "mode": "agent", "thread": null }]);
    d["app"]["pages"]["panes"] = json!([[1], [2]]);
    assert_eq!(no_hist(run(&d, "monitor_terminal", k).unwrap()), [Action::ClosePanel("monitor".into()), i("page_show", json!({ "page": 2 })), Action::Focus("grid-2".into())]);
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
    tap(&mut rt, "top-view-gui");
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
    let (x, y) = rt.find("top-view-gui").unwrap();
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
    assert!(rt.state.feats.overlay.is_none() && rt.state.feats.base.is_none(), "the terminal opened: back to the terminals");
    let last = sel(&rt);
    let (sb, th) = last.split_once("::").map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or_else(|| (last.split(':').next().unwrap().to_string(), None));
    assert!(rt.state.panes.iter().any(|p| p.sandbox.as_deref() == Some(sb.as_str()) && p.thread == th), "its terminal is in a pane: {last}");
    // Back by GUI: the same session; Escape does not leave it (a base
    // view), TUI does.
    let (x, y) = rt.find("top-view-gui").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 6);
    assert_eq!(sel(&rt), last, "the view comes back on the same session");
    key(&mut rt, Key::Escape);
    frames(&mut rt, 2);
    assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"), "Escape does not leave the Monitor");
    let (x, y) = rt.find("top-view-tui").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 2);
    assert!(rt.state.feats.overlay.is_none() && rt.state.feats.base.is_none(), "TUI: the terminals");
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

/// The Monitor under another view (Files, opened from its links): it is
/// still the base, but covered: words and picks refuse in Rust; the view
/// closed, the Monitor is back with the same session and the same draft,
/// and they work again.
#[test]
fn under_files_the_monitor_sends_nothing_and_comes_back_as_it_was() {
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
    let tap = |rt: &mut rune::Runtime<App>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.016);
    };
    let sel = |rt: &rune::Runtime<App>| rt.state.screen_data()["monitor"]["sel"].as_str().unwrap_or("").to_string();
    let typed = |rt: &rune::Runtime<App>| rt.state.screens.iter().find(|x| x.name == "monitor").and_then(|sc| sc.ui.edits.get("monitor-compose").map(|e| e.0.text.clone()));
    frames(&mut rt, 4);
    tap(&mut rt, "top-view-gui");
    frames(&mut rt, 6);
    tap(&mut rt, "monitor-row-billing-api:s-bill-main");
    frames(&mut rt, 4);
    rt.update(|_, tx| tx.focus("monitor-compose", false));
    for c in "draft".chars() {
        rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: rune::Mods::default() });
        rt.frame(0.02);
    }
    assert_eq!(typed(&rt).as_deref(), Some("draft"));
    let send = |rt: &mut rune::Runtime<App>| {
        rt.update(|s, _| {
            s.notice = None;
            let l = crate::intent::parse(&json!({ "kind": "intent", "name": "monitor_send", "args": { "key": "billing-api:s-bill-main", "text": "draft" } })).unwrap();
            crate::screen::Host::intents(s, "monitor", l);
        });
    };
    let pick = |rt: &mut rune::Runtime<App>| {
        rt.update(|s, _| {
            s.notice = None;
            let l = crate::intent::parse(&json!({ "kind": "intent", "name": "monitor_pick", "args": { "key": "shop-web:s-shop-main", "step": 1, "option": 1 } })).unwrap();
            crate::screen::Host::intents(s, "monitor", l);
        });
    };
    let refused = |rt: &rune::Runtime<App>| rt.state.notice.as_ref().is_some_and(|n| n.0.contains("Not sent"));
    // Files over the Monitor, from its link.
    tap(&mut rt, "monitor-files");
    frames(&mut rt, 4);
    assert_eq!(rt.state.feats.overlay.as_ref().map(|o| o.0.as_str()), Some("files"));
    assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"), "still the base");
    assert_eq!(rt.state.screen_data()["app"]["covered"], true);
    send(&mut rt);
    frames(&mut rt, 1);
    assert!(refused(&rt) && rt.state.feats.dialog.is_none(), "words under Files: {:?}", rt.state.notice);
    pick(&mut rt);
    frames(&mut rt, 1);
    assert!(refused(&rt) && rt.state.feats.dialog.is_none(), "a pick under Files: {:?}", rt.state.notice);
    // Files closed: the Monitor as it was, and both work again.
    rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
    frames(&mut rt, 4);
    assert_eq!(rt.state.feats.overlay.as_ref().map(|o| o.0.as_str()), Some("monitor"), "back to the Monitor");
    assert_eq!(sel(&rt), "billing-api:s-bill-main", "the same session");
    assert_eq!(typed(&rt).as_deref(), Some("draft"), "the same draft");
    assert_eq!(rt.state.screen_data()["app"]["covered"], false);
    pick(&mut rt);
    frames(&mut rt, 1);
    assert!(!refused(&rt), "the pick goes again: {:?}", rt.state.notice);
    send(&mut rt);
    frames(&mut rt, 1);
    assert!(rt.state.feats.dialog.is_some(), "the words go to the core's dialog again: {:?}", rt.state.notice);
}

// ---------------------------------------------------------------------------
// Attach

/// A file of invented content under this test's own folder.
fn file_in(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sbxw-monitor-attach-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The view after an action list (the last `Set` of `data.monitor`).
fn last_view(acts: &[Action]) -> J {
    acts.iter().rev().find_map(|a| match a {
        Action::Set { key, value } if key == KEY => Some(value.clone()),
        _ => None,
    }).expect("the view")
}

fn shown_on(d: &mut J, key: &str) {
    run(d, "monitor_select", json!({ "key": key })).map(|a| d["monitor"] = view(&a)).unwrap();
}

/// Attach's press asks the core for its file dialog, and adds nothing: a
/// chip comes only from what the core hands in (`on_files`). A chip is the
/// shown session's: another session has none, and keeps its own.
#[test]
fn a_chip_comes_only_from_the_core_and_belongs_to_its_session() {
    let dir = tmp_dir("chip");
    let brief = file_in(&dir, "brief.md", b"# Brief");
    let mut d = data_of("basic");
    opened(&mut d);
    shown_on(&mut d, "billing-api:s-bill-main");
    let a = run(&d, "monitor_attach", json!({ "key": "billing-api:s-bill-main" })).unwrap();
    assert!(matches!(a.as_slice(), [Action::PickFiles { .. }]), "{a:?}");
    assert!(STATE().lock().unwrap().att.is_empty(), "the press itself attaches nothing");
    // Not the session shown: refused.
    assert!(run(&d, "monitor_attach", json!({ "key": "docs-site:s-docs-main" })).is_err());
    // The dialog's answer, from the core.
    let out = on_files(&Ctx { data: &d }, Incoming::Files(vec![brief.clone()]));
    let v = last_view(&out);
    let files = v["detail"]["files"].as_array().unwrap().clone();
    assert_eq!(files.len(), 1);
    assert_eq!((files[0]["name"].as_str(), files[0]["state"].as_str(), files[0]["size"].as_str()), (Some("brief.md"), Some("ok"), Some("7 B")));
    // Its chip is drawn with its size and a remove cross; Send works with no words.
    d["monitor"] = v;
    let n = host().render(&d).unwrap();
    let id = files[0]["id"].as_u64().unwrap();
    assert!(find(&n, &format!("monitor-file-{id}")).is_some());
    assert!(find(&n, &format!("monitor-file-remove-{id}")).is_some());
    assert!(!find(&n, "monitor-send").unwrap().disabled, "files alone may be sent");
    assert!(!find(&n, "monitor-attach").unwrap().disabled);
    // Another session: none of it.
    shown_on(&mut d, "docs-site:s-docs-main");
    assert_eq!(d["monitor"]["detail"]["files"], json!([]));
    let n = host().render(&d).unwrap();
    assert!(find(&n, &format!("monitor-file-{id}")).is_none());
    // A send to it takes no file of billing-api's.
    let a = run(&d, "monitor_send", json!({ "key": "docs-site:s-docs-main", "text": "go" })).unwrap();
    let Action::Prepare(p) = &a[0] else { panic!("{a:?}") };
    let c = (p.build)(&Ctx { data: &d }, &p.asked, &json!({ "ok": true, "text": "go" })).unwrap();
    assert_eq!(c.call.path, "/api/chat/push", "no upload for another session's file");
    // Back on billing-api: its file waits; remove takes it away and the core forgets its path.
    shown_on(&mut d, "billing-api:s-bill-main");
    assert_eq!(d["monitor"]["detail"]["files"].as_array().unwrap().len(), 1);
    let out = run(&d, "monitor_attach_remove", json!({ "key": "billing-api:s-bill-main", "id": id })).unwrap();
    assert_eq!(last_view(&out)["detail"]["files"], json!([]));
    assert!(out.iter().any(|a| matches!(a, Action::ForgetFiles(p) if p == &vec![brief.clone()])), "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Files with words: ONE core dialog (never a call before it), listing the
/// words, each file with its local path and size, the total, the target;
/// its call is the core's batch upload. Once every file is in, the words
/// and the daemon's paths (never the local ones) go as one message.
#[test]
fn one_dialog_then_the_uploads_then_the_words_with_the_daemons_paths() {
    let dir = tmp_dir("send");
    let a1 = file_in(&dir, "notes.txt", b"some notes");
    let a2 = file_in(&dir, "chart.csv", b"a,b\n1,2\n");
    let mut d = data_of("basic");
    opened(&mut d);
    shown_on(&mut d, "billing-api:s-bill-main");
    let key = "billing-api:s-bill-main";
    on_files(&Ctx { data: &d }, Incoming::Files(vec![a1.clone(), a2.clone()]));
    let a = run(&d, "monitor_send", json!({ "key": key, "text": "Read these" })).unwrap();
    let a = admit(Risk::Approval("x"), "monitor_send", a).expect("only the core's dialog");
    assert!(!a.iter().any(|x| matches!(x, Action::Call(_))));
    let Action::Prepare(p) = &a[0] else { panic!("{a:?}") };
    let c = (p.build)(&Ctx { data: &d }, &p.asked, &json!({ "ok": true, "text": "Read these", "masked": 0 })).unwrap();
    assert_eq!(c.call.path, "core:uploads");
    assert!(c.dont_ask.is_none(), "no \"Do not ask again\" on a Monitor send");
    assert_eq!(c.preview.as_ref().unwrap().text, "Read these");
    let lines = c.lines.join("\n");
    assert!(lines.contains(&a1.display().to_string()) && lines.contains(&a2.display().to_string()), "{lines}");
    assert!(lines.contains("2 files, 18 B in all"), "{lines}");
    assert!(c.title.contains("billing-api · main"));
    let items = c.call.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0]["route"].as_str().unwrap().starts_with("/api/sandboxes/billing-api/upload?name="));
    // Send there: the core runs the batch and answers.
    let up = |i: usize| items[i]["tag"].as_str().unwrap().to_string();
    let body = json!({ "results": [
        { "tag": up(0), "code": 200, "body": { "ok": true, "path": "/tmp/sbxw-uploads/1700000000001/notes.txt" } },
        { "tag": up(1), "code": 200, "body": { "ok": true, "path": "/tmp/sbxw-uploads/1700000000002/chart.csv" } } ] });
    let out = on_answer(&Ctx { data: &d }, &c.call, Ok((200, body)));
    let push = out.iter().find_map(|a| match a {
        Action::Call(c) if c.path == "/api/chat/push" => Some(c.clone()),
        _ => None,
    }).expect("the push");
    let text = push.body["text"].as_str().unwrap();
    assert_eq!(text, "Read these /tmp/sbxw-uploads/1700000000001/notes.txt /tmp/sbxw-uploads/1700000000002/chart.csv");
    assert!(!text.contains(&dir.display().to_string()), "no local path is typed");
    assert_eq!(push.body["name"], "billing-api");
    // The push answered: the chips go with the draft.
    let back = on_answer(&Ctx { data: &d }, &push, Ok((200, json!({ "ok": true }))));
    assert_eq!(last_view(&back)["detail"]["files"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The second file fails: nothing is pushed, the notice says what went and
/// what did not, the draft stays (no `sent` move) and so do the chips; the
/// first file keeps its daemon path and goes with the next Send, which
/// uploads only the rest.
#[test]
fn a_failed_upload_pushes_nothing_and_keeps_the_draft() {
    let dir = tmp_dir("fail");
    let a1 = file_in(&dir, "one.txt", b"1");
    let a2 = file_in(&dir, "two.txt", b"22");
    let mut d = data_of("basic");
    opened(&mut d);
    let key = "billing-api:s-bill-main";
    shown_on(&mut d, key);
    on_files(&Ctx { data: &d }, Incoming::Files(vec![a1, a2]));
    let a = run(&d, "monitor_send", json!({ "key": key })).unwrap();
    let Action::Confirm(c) = &a[0] else { panic!("files alone: the dialog at once, {a:?}") };
    let items = c.call.body["items"].as_array().unwrap().clone();
    let body = json!({ "results": [
        { "tag": items[0]["tag"], "code": 200, "body": { "ok": true, "path": "/tmp/sbxw-uploads/1/one.txt" } },
        { "tag": items[1]["tag"], "code": 507, "body": { "ok": false, "error": "no space left" } } ] });
    let sent_before = STATE().lock().unwrap().sent.get(key).copied();
    let out = on_answer(&Ctx { data: &d }, &c.call, Ok((200, body)));
    assert!(!out.iter().any(|a| matches!(a, Action::Call(_))), "nothing pushed: {out:?}");
    let note = out.iter().find_map(|a| match a {
        Action::Notice { text, error: true, .. } => Some(text.clone()),
        _ => None,
    }).unwrap();
    assert!(note.contains("two.txt was not uploaded: no space left") && note.contains("In the sandbox already") && note.contains("one.txt"), "{note}");
    assert_eq!(STATE().lock().unwrap().sent.get(key).copied(), sent_before, "the draft stays");
    let v = last_view(&out);
    let st: Vec<&str> = v["detail"]["files"].as_array().unwrap().iter().map(|f| f["state"].as_str().unwrap()).collect();
    assert_eq!(st, ["uploaded", "ok"]);
    // The next Send uploads only the second, then pushes both paths.
    let a = run(&d, "monitor_send", json!({ "key": key })).unwrap();
    let Action::Confirm(c) = &a[0] else { panic!() };
    let items = c.call.body["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    let out = on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "results": [{ "tag": items[0]["tag"], "code": 200, "body": { "ok": true, "path": "/tmp/sbxw-uploads/2/two.txt" } }] }))));
    let push = out.iter().find_map(|a| match a { Action::Call(c) => Some(c.clone()), _ => None }).unwrap();
    assert_eq!(push.body["text"], "/tmp/sbxw-uploads/1/one.txt /tmp/sbxw-uploads/2/two.txt", "no words: the message is the paths");
    // A daemon path that is not one of its upload folders is never typed.
    on_answer(&Ctx { data: &d }, &push, Ok((200, json!({ "ok": true }))));
    let a3 = file_in(&dir, "three.txt", b"3");
    on_files(&Ctx { data: &d }, Incoming::Files(vec![a3]));
    let a = run(&d, "monitor_send", json!({ "key": key })).unwrap();
    let Action::Confirm(c) = &a[0] else { panic!() };
    let items = c.call.body["items"].as_array().unwrap().clone();
    let out = on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "results": [{ "tag": items[0]["tag"], "code": 200, "body": { "ok": true, "path": "/etc/passwd; rm -rf ~" } }] }))));
    assert!(!out.iter().any(|a| matches!(a, Action::Call(_))), "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An empty file, or one the daemon cannot take, cannot be sent: the Send
/// is refused before any dialog, the button is off. A file whose name looks
/// like a secret needs the dialog's box ticked.
#[test]
fn a_refused_file_blocks_the_send_and_a_secret_looking_one_needs_the_box() {
    let dir = tmp_dir("refused");
    let empty = file_in(&dir, "empty.txt", b"");
    let env = file_in(&dir, ".env.local", b"NAME=value");
    let mut d = data_of("basic");
    opened(&mut d);
    let key = "billing-api:s-bill-main";
    shown_on(&mut d, key);
    let out = on_files(&Ctx { data: &d }, Incoming::Files(vec![empty]));
    d["monitor"] = last_view(&out);
    assert_eq!(d["monitor"]["detail"]["files"][0]["state"], "refused");
    let e = run(&d, "monitor_send", json!({ "key": key, "text": "hi" })).unwrap_err();
    assert!(e.starts_with("Not sent: empty.txt cannot go"), "{e}");
    let h = host();
    let ta = find(&h.render(&d).unwrap(), "monitor-compose").unwrap().clone();
    h.act("typed", &ta.arg, &"hi".into()).unwrap();
    assert!(find(&h.render(&d).unwrap(), "monitor-send").unwrap().disabled, "a refused file greys Send");
    let id = d["monitor"]["detail"]["files"][0]["id"].as_u64().unwrap();
    run(&d, "monitor_attach_remove", json!({ "key": key, "id": id })).unwrap();
    on_files(&Ctx { data: &d }, Incoming::Files(vec![env]));
    let a = run(&d, "monitor_send", json!({ "key": key })).unwrap();
    let Action::Confirm(c) = &a[0] else { panic!() };
    assert!(c.ack.is_some(), "a secret-looking name: the box must be ticked");
    assert!(c.lines.iter().any(|l| l.contains("looks like a secret")), "{:?}", c.lines);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Nothing is attached, picked or sent while something covers the Monitor
/// or a Jarvis proposal waits, nor for a session that waits on a menu.
#[test]
fn nothing_is_attached_under_a_dialog_a_proposal_or_a_menu() {
    let dir = tmp_dir("held");
    let f = file_in(&dir, "a.txt", b"a");
    let mut d = data_of("basic");
    opened(&mut d);
    shown_on(&mut d, "billing-api:s-bill-main");
    for g in [(|d: &mut J| d["app"]["covered"] = true.into()) as fn(&mut J), |d: &mut J| d["jarvis"]["pending"] = json!([{ "id": "p-1" }])] {
        let mut d2 = d.clone();
        g(&mut d2);
        assert!(run(&d2, "monitor_attach", json!({ "key": "billing-api:s-bill-main" })).unwrap_err().starts_with("Nothing attached"));
        let out = on_files(&Ctx { data: &d2 }, Incoming::Files(vec![f.clone()]));
        assert!(out.iter().any(|a| matches!(a, Action::ForgetFiles(_))) && !out.iter().any(|a| matches!(a, Action::Set { .. })), "{out:?}");
        let out = on_files(&Ctx { data: &d2 }, Incoming::Png(vec![1, 2, 3]));
        assert!(!out.iter().any(|a| matches!(a, Action::Set { .. })));
    }
    assert!(STATE().lock().unwrap().att.is_empty());
    shown_on(&mut d, "shop-web::checkout-copy");
    assert!(run(&d, "monitor_attach", json!({ "key": "shop-web::checkout-copy" })).unwrap_err().contains("answer that first"));
    let n = host().render(&d).unwrap();
    assert!(find(&n, "monitor-attach").unwrap().disabled);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hostile names (control characters, direction marks, very long, a
/// backslash) make no control and no key: the chip shows them cut, the
/// hidden characters as `?`; the dialog shows the whole path, those
/// characters written out; the upload route encodes the name.
#[test]
fn hostile_file_names_are_cut_in_the_chip_and_whole_in_the_dialog() {
    let dir = tmp_dir("names");
    let long = format!("{}.txt", "x".repeat(240));
    let names = ["evil\nname\u{1b}[31m.txt".to_string(), "invoice\u{202e}txt.exe".to_string(), long.clone(), "a\\b \"c\".md".to_string()];
    let paths: Vec<_> = names.iter().map(|n| file_in(&dir, n, b"x")).collect();
    let mut d = data_of("basic");
    opened(&mut d);
    let key = "billing-api:s-bill-main";
    shown_on(&mut d, key);
    let out = on_files(&Ctx { data: &d }, Incoming::Files(paths.clone()));
    d["monitor"] = last_view(&out);
    let chips = d["monitor"]["detail"]["files"].as_array().unwrap().clone();
    assert_eq!(chips.len(), 4);
    for c in &chips {
        let n = c["name"].as_str().unwrap();
        assert!(n.chars().count() <= attach::CHIP_CHARS && !n.chars().any(|ch| ch.is_control() || ch == '\u{202e}'), "{n:?}");
    }
    assert_eq!(chips[0]["name"], "evil?name?[31m.txt");
    // Drawn: each chip makes one control, its cross, keyed by id only.
    let n = host().render(&d).unwrap();
    let ctl = controls(find(&n, "monitor-attached").unwrap());
    assert_eq!(ctl.len(), 4, "{ctl:?}");
    assert!(ctl.iter().all(|k| k.split(' ').nth(1).is_some_and(|key| key.strip_prefix("monitor-file-remove-").is_some_and(|id| id.parse::<u64>().is_ok()))), "{ctl:?}");
    // The dialog: the whole path of each, hidden characters written out.
    let a = run(&d, "monitor_send", json!({ "key": key })).unwrap();
    let Action::Confirm(c) = &a[0] else { panic!() };
    let lines = c.lines.join("\n");
    assert!(lines.contains("evil\\u{a}name\\u{1b}[31m.txt") && lines.contains("invoice\\u{202e}txt.exe") && lines.contains(&long), "{lines}");
    assert!(!lines.contains('\u{1b}') && !lines.contains('\u{202e}'));
    for it in c.call.body["items"].as_array().unwrap() {
        let r = it["route"].as_str().unwrap();
        assert!(!r.contains('\n') && !r.contains(' ') && !r.contains('\\'), "{r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The whole window: the person chose "Do not ask again for files I drop"
/// (the core's store), and still a Monitor send with files opens the
/// core's dialog; under Jarvis's proposal the Attach press asks no file
/// dialog and a drop on the Monitor attaches nothing.
#[test]
fn the_skip_dialog_choice_never_skips_a_monitor_send_and_the_gate_holds() {
    use crate::shell::{view as shell_view, App};
    use rune::KeyEvent;
    let dir = tmp_dir("window");
    let f = file_in(&dir, "plan.md", b"# Plan");
    let fx = format!("{}/fixtures/monitor/gate.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(crate::shell::Args { fixture: Some(fx), ..Default::default() });
    a.store.set_in("uploads", "skip_drop_dialog", J::Bool(true));
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
    rt.update(|s, _| s.jarvis.shown = None);
    frames(&mut rt, 4);
    tap(&mut rt, "top-view-gui");
    frames(&mut rt, 6);
    tap(&mut rt, "monitor-row-billing-api:s-bill-main");
    frames(&mut rt, 4);
    // Jarvis's proposal up: Attach asks for no file dialog, a drop attaches nothing.
    rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
    frames(&mut rt, 2);
    rt.update(|s, _| {
        let l = crate::intent::parse(&json!({ "kind": "intent", "name": "monitor_attach", "args": { "key": "billing-api:s-bill-main" } })).unwrap();
        crate::screen::Host::intents(s, "monitor", l);
    });
    assert!(rt.state.feats.asks.is_empty(), "a file dialog was asked under the proposal");
    let det = rt.presented("monitor-detail").map(|r| r.0);
    if let Some(r) = det {
        rt.drop_files(vec![f.clone()], Some((r[0] + 20.0, r[1] + 20.0)));
        frames(&mut rt, 2);
    }
    assert!(rt.state.screen_data()["monitor"]["detail"]["files"].as_array().is_none_or(|v| v.is_empty()), "a drop attached a file under the proposal");
    // Nothing waits for the person (another window, the basic fixture): a
    // drop on the Monitor attaches it to the session shown.
    let fx = format!("{}/fixtures/monitor/basic.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(crate::shell::Args { fixture: Some(fx), ..Default::default() });
    a.store.set_in("uploads", "skip_drop_dialog", J::Bool(true));
    let mut rt = rune::Runtime::new(a, shell_view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    frames(&mut rt, 4);
    tap(&mut rt, "top-view-gui");
    frames(&mut rt, 6);
    tap(&mut rt, "monitor-row-billing-api:s-bill-main");
    frames(&mut rt, 4);
    let r = rt.presented("monitor-detail").unwrap().0;
    rt.drop_files(vec![f.clone()], Some((r[0] + 20.0, r[1] + 20.0)));
    frames(&mut rt, 3);
    assert_eq!(rt.state.screen_data()["monitor"]["detail"]["files"].as_array().map(Vec::len), Some(1), "the drop is the shown session's");
    assert!(rt.state.feats.dialog.is_none(), "a drop on the Monitor uploads nothing by itself");
    // Send: the core's dialog, even with "Do not ask again" chosen.
    rt.update(|s, _| {
        let l = crate::intent::parse(&json!({ "kind": "intent", "name": "monitor_send", "args": { "key": "billing-api:s-bill-main" } })).unwrap();
        crate::screen::Host::intents(s, "monitor", l);
    });
    frames(&mut rt, 2);
    let d = rt.state.feats.dialog.as_ref().expect("the core's dialog");
    assert_eq!(d.confirm.as_ref().map(|c| c.call.path.as_str()), Some("core:uploads"));
    assert!(d.confirm.as_ref().is_some_and(|c| c.dont_ask.is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// One row per session (place)

/// The daemon lists a session per hook `session_id`; one terminal that was
/// restarted or resumed leaves several, the old ones idle for ever (no
/// `SessionEnd`). The list folds them by place, as the sidebar does: one
/// row per thread, one per main checkout, the latest report; two live
/// sessions in one main checkout are two rows. The header counts the rows.
#[test]
fn one_row_per_place_and_a_new_session_takes_its_place() {
    let mut d = data_of("folding");
    let v = opened(&mut d);
    let ks = keys(&v);
    let studio_main: Vec<&String> = ks.iter().filter(|k| k.starts_with("studio:") && !k.contains("::")).collect();
    assert_eq!(studio_main.len(), 1, "studio's main checkout once: {ks:?}");
    assert_eq!(ks.iter().filter(|k| k.starts_with("studio::paint")).count(), 1, "{ks:?}");
    assert_eq!(ks.iter().filter(|k| k.starts_with("lab:")).count(), 2, "two live terminals on lab's checkout: {ks:?}");
    assert_eq!(v["counts"]["total"], ks.len(), "the header counts the rows");
    let paint = v["rows"].as_array().unwrap().iter().find(|r| r["key"] == "studio::paint").unwrap().clone();
    assert_eq!(paint["group"], "working", "the thread's latest session: {paint}");
    // The selected main row: the latest report, its last prompt an agent's message.
    let main = studio_main[0].clone();
    shown_on(&mut d, &main);
    let det = d["monitor"]["detail"].clone();
    assert_eq!(det["reply"], "Noted: I merge the palette thread next.");
    assert!(det["last_input"].is_null(), "a cross-session message is not You: {det}");
    assert_eq!(det["from"]["name"], "paint-thread");
    assert_eq!(det["from"]["text"], "The palette step is done; tests pass.");
    let row = d["monitor"]["rows"].as_array().unwrap().iter().find(|r| r["key"] == main.as_str()).unwrap().clone();
    assert!(!row["line"].as_str().unwrap().contains('<'), "{row}");
    // Drawn: no raw tag anywhere, and the message makes no control.
    let n = host().render(&d).unwrap();
    let mut all = vec![];
    walk(&n, &mut all);
    assert!(all.iter().all(|x| !x.text.contains("cross-session-message")), "a raw tag is drawn");
    let from = find(&n, "monitor-from").expect("the message, as data");
    assert!(controls(from).is_empty());
    // A new session in the same place (a resume): the same row, its draft and files with it.
    let dir = tmp_dir("fold");
    let f = file_in(&dir, "note.txt", b"n");
    on_files(&Ctx { data: &d }, Incoming::Files(vec![f]));
    let mut next = d["sessions"].as_array().unwrap().iter().find(|i| i["session_id"] == "s-studio-3").unwrap().clone();
    next["session_id"] = "s-studio-4".into();
    next["started_ms"] = (next["ts"].as_u64().unwrap() + 1000).into();
    next["ts"] = (next["ts"].as_u64().unwrap() + 2000).into();
    next["last_input"] = "Go on with the export.".into();
    let out = on_event(&Ctx { data: &d }, "session", &next);
    let v = last_view(&out);
    assert_eq!(keys(&v).iter().filter(|k| k.starts_with("studio:") && !k.contains("::")).count(), 1, "still one row");
    assert_eq!(v["sel"], main.as_str(), "the selection stays on the place");
    assert_eq!(v["detail"]["last_input"], "Go on with the export.");
    assert_eq!(v["detail"]["files"].as_array().map(Vec::len), Some(1), "its attached file stays");
    // An old session's report coming late changes nothing.
    let mut late = d["sessions"][0].clone();
    late["ts"] = (late["ts"].as_u64().unwrap() + 1).into();
    assert!(on_event(&Ctx { data: &d }, "session", &late).is_empty());
    // lab's two rows say which session each is.
    let names: Vec<String> = d["monitor"]["rows"].as_array().unwrap().iter().filter(|r| r["sandbox"] == "lab").map(|r| r["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names.len(), 2);
    assert!(names[0] != names[1] && names.iter().all(|n| n.starts_with("main \u{b7} ")), "{names:?}");
    // The end of paint's older session (the stand-in's event) leaves paint as it is.
    let ended_old = d["session_events"][0]["session"].clone();
    assert!(on_event(&Ctx { data: &d }, "session", &ended_old).is_empty(), "an older session's end changes nothing");
    // One of lab's two ends: its row leaves; the other stays.
    let mut gone = d["sessions"].as_array().unwrap().iter().find(|i| i["session_id"] == "s-lab-a").unwrap().clone();
    gone["state"] = "exited".into();
    gone["ts"] = (gone["ts"].as_u64().unwrap() + 5).into();
    let v = last_view(&on_event(&Ctx { data: &d }, "session", &gone));
    assert_eq!(keys(&v).iter().filter(|k| k.starts_with("lab:")).count(), 1, "{:?}", keys(&v));
    assert_eq!(v["counts"]["total"], keys(&v).len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn harness_text_is_told_from_the_person_s_prompt() {
    use super::model::injected;
    assert_eq!(injected("Fix the build"), None);
    assert_eq!(injected("<b>bold</b> is mine"), None);
    let (from, body) = injected("<\\cross-session-message from=\"uds:/x\" from-name=\"lead\" from-mode=\"bypass\">Hi there</cross-session-message>").unwrap();
    assert_eq!((from.as_deref(), body.as_str()), (Some("lead"), "Hi there"));
    let (from, _) = injected("  <task-notification>\n<task-id>b1</task-id></task-notification>").unwrap();
    assert_eq!(from.as_deref(), Some("sbxw"));
    // The daemon's word, when it gives it, comes first.
    use super::model::from_of;
    let r = |extra: J| { let mut i = json!({ "last_input": "<cross-session-message from-name=\"x\">hi</cross-session-message>" }); for (k, v) in extra.as_object().unwrap() { i[k] = v.clone(); } i };
    assert_eq!(from_of(&r(json!({}))), Some(("x".into(), "hi".into())));
    assert_eq!(from_of(&r(json!({ "last_input_from": "person" }))), None, "the person typed it, tags and all");
    assert_eq!(from_of(&r(json!({ "last_input_from": "injected", "last_input_sender": "lead" }))), Some(("lead".into(), "hi".into())));
    assert_eq!(from_of(&json!({ "last_input": "plain words", "last_input_from": "injected", "last_input_sender": "task" })), Some(("task".into(), "plain words".into())));
}

/// When the daemon says so (`continues`, `ended`: fields it may serve), the
/// list follows it: a session that continues another takes its card, even
/// in another place's order; `ended: true` is an end. Absent, nothing changes.
#[test]
fn the_daemon_s_continues_and_ended_are_followed_when_served() {
    let mut m = Model::default();
    let base = |sid: &str, ts: u64, state: &str| json!({ "sandbox": "lab", "mode": "claude", "state": state, "started_ms": 1, "ts": ts, "session_id": sid });
    m.upsert(&base("a", 10, "working"));
    m.upsert(&base("b", 20, "attention"));
    assert_eq!(m.cards.len(), 2, "two live sessions: two cards");
    let mut c = base("c", 30, "working");
    c["continues"] = "a".into();
    m.upsert(&c);
    assert_eq!(m.cards.len(), 2, "c continues a: a's card");
    assert!(m.cards.values().any(|x| x.info["session_id"] == "c") && !m.cards.values().any(|x| x.info["session_id"] == "a"));
    let mut e = base("b", 40, "idle");
    e["ended"] = true.into();
    m.upsert(&e);
    assert_eq!(m.cards.len(), 1, "b ended: its card leaves (another holds the place)");
}

/// Part 1 of SESSIONS.md: every name the person reads says "Sessions" (the
/// view's title, the pager's switch, the rail entry, the palette, Help);
/// the names code and the state file use stay `monitor`, so a `base_view`
/// stored by an older build still brings the view back.
#[test]
fn the_view_is_called_sessions_and_an_older_stored_base_still_restores() {
    use crate::shell::{App, Args};
    let fx = format!("{}/fixtures/monitor/basic.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(Args { fixture: Some(fx), ..Default::default() });
    assert!(matches!(crate::feat::screen("monitor").map(|(_, d)| &d.slot), Some(Slot::Base { title: "Sessions", icon: "inbox", .. })), "the view's title and its switch segment");
    // GUI | TUI and Lead Mode are the top bar's: the rail has neither (it shows more, it does not switch views).
    let (top, bottom) = a.rail_entries();
    let keys: Vec<String> = top.iter().chain(&bottom).map(|e| e.key.clone()).collect();
    assert!(!keys.iter().any(|k| k == "rail-monitor" || k == "rail-leads"), "no Sessions or Team leads entry on the rail: {keys:?}");
    assert!(FEATURE.extras.is_none(), "Sessions puts nothing on the rail");
    let cmds = a.palette_cmds();
    assert!(cmds.iter().any(|c| c.label == "Switch between GUI and TUI"), "the palette's shortcut");
    assert!(a.screen_data()["app"]["shortcuts"].as_array().unwrap().iter().any(|k| k["what"] == "Switch between GUI and TUI"), "Help lists the shortcut under its new name");
    let all: String = cmds.iter().map(|c| format!("{} {}\n", c.label, c.hint)).collect();
    assert!(!all.contains("Monitor"), "no palette text says Monitor: {all}");
    // The state file of an older build: `base_view: "monitor"`.
    let p = std::env::temp_dir().join(format!("sbxw-sessions-base-{}.json", std::process::id()));
    std::fs::write(&p, r#"{"base_view":"monitor"}"#).unwrap();
    a.store = crate::shell::store::Store::at(Some(p.clone()));
    a.kept_base();
    assert_eq!((a.feats.base.as_deref(), a.feats.base_restore), (Some("monitor"), true), "the stored value still restores the view");
    let _ = std::fs::remove_file(&p);
}

// ---- Part 3: the conversation (conv.rs) ----

/// The stand-in answers every history call in `acts` (from `d`'s
/// `history`), each answer handed to `on_answer`; what that returns, with
/// the answers to the reads it asks in turn.
fn serve(d: &mut J, acts: Vec<Action>) -> Vec<Action> {
    let mut out = vec![];
    let mut todo = acts;
    for _ in 0..8 {
        let mut next = vec![];
        for a in todo {
            match a {
                Action::Call(c) if c.tag.starts_with("hist:") => {
                    let m = match c.method {
                        crate::feat::Method::Get => "GET",
                        crate::feat::Method::Delete => "DELETE",
                        _ => "POST",
                    };
                    let (code, body) = crate::feat::answer_from(d, m, &c.path, &J::Null, &mut |_, _| {});
                    next.extend(on_answer(&Ctx { data: d }, &c, Ok((code, body))));
                }
                Action::Set { key, value } if key == KEY => {
                    d["monitor"] = value.clone();
                    out.push(Action::Set { key, value });
                }
                other => out.push(other),
            }
        }
        if next.is_empty() {
            break;
        }
        todo = next;
    }
    out
}

/// The history calls the stand-in saw, and forgets them.
fn calls(d: &mut J) -> Vec<String> {
    let v = d["history_calls"].as_array().cloned().unwrap_or_default();
    d["history_calls"] = json!([]);
    v.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()
}

/// Opened on the fixture with `key` selected and its conversation read.
fn on_session(key: &str) -> (J, J) {
    let mut d = data_of("basic");
    *STATE().lock().unwrap() = Model::default();
    let a = run(&d, "monitor_open", json!({})).unwrap();
    serve(&mut d, a);
    let a = run(&d, "monitor_select", json!({ "key": key })).unwrap();
    serve(&mut d, a);
    let v = d["monitor"].clone();
    (d, v)
}

fn conv_rows(v: &J) -> Vec<J> {
    v["detail"]["conv"]["rows"].as_array().cloned().unwrap_or_default()
}

fn it(seq: u64, kind: &str, extra: J) -> J {
    let mut i = json!({ "seq": seq, "at": 1763462000000u64 + seq * 1000, "kind": kind });
    for (k, v) in extra.as_object().unwrap() {
        i[k] = v.clone();
    }
    i
}

fn put_log(d: &mut J, sb: &str, sid: &str, log: J) {
    d["history"][format!("{sb}/{sid}")] = log;
}

#[test]
fn the_conversation_reads_in_the_terminal_s_order_and_injected_is_never_you() {
    let (d, v) = on_session("billing-api:s-bill-main");
    assert_eq!(v["detail"]["conv"]["state"], "ready");
    let rows = conv_rows(&v);
    let kinds: Vec<&str> = rows.iter().map(|r| r["t"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"day"), "a day separator first");
    assert!(kinds.contains(&"you") && kinds.contains(&"claude") && kinds.contains(&"group") && kinds.contains(&"system") && kinds.contains(&"from"), "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == "day").count(), 2, "two days: yesterday and today");
    // The newest last: the reply.
    assert_eq!(rows.last().unwrap()["t"], "claude");
    let from = rows.iter().find(|r| r["t"] == "from").unwrap();
    assert_eq!(from["name"], "lead");
    assert_eq!(from["long"], true, "a long message is folded");
    assert!(from["text"].as_str().unwrap().len() < 400);
    // Drawn: "From lead", never "You", for it.
    let h = host();
    let n = h.render(&d).unwrap();
    let f = find(&n, &format!("monitor-from-{}", from["id"].as_str().unwrap())).expect("the injected message");
    let mut all = vec![];
    walk(f, &mut all);
    let texts: Vec<&str> = all.iter().map(|x| x.text.as_str()).collect();
    assert!(texts.contains(&"From lead") && !texts.contains(&"You"), "{texts:?}");
    // A raw harness tag in an injected item is not drawn either.
    let mut m = model::Model::default();
    let _ = &mut m;
    let rows = conv::build([it(1, "injected", json!({ "text": "<cross-session-message from-name=\"lead\">Rebase on main</cross-session-message>", "from": "lead" }))].iter(), &Default::default(), 1763462000);
    assert_eq!(rows[1]["text"], "Rebase on main");
    assert!(!rows[1]["text"].as_str().unwrap().contains('<'));
}

#[test]
fn steps_fold_into_a_group_that_opens_and_a_step_opens_to_its_output() {
    let (mut d, v) = on_session("billing-api:s-bill-main");
    let g = conv_rows(&v).into_iter().filter(|r| r["t"] == "group").last().unwrap();
    assert_eq!(g["n"], 12);
    assert_eq!(g["summary"], "CHANGELOG.md", "the last step's summary");
    assert!(g.get("steps").is_none(), "folded: its steps are not sent");
    let id = g["id"].as_str().unwrap().to_string();
    let h = host();
    let before = h.render(&d).unwrap();
    assert!(find(&before, &format!("monitor-steps-{id}")).is_some());
    let a = run(&d, "monitor_expand", json!({ "key": "billing-api:s-bill-main", "id": id })).unwrap();
    assert!(!sends(&a), "unfolding sends nothing");
    serve(&mut d, a);
    let g = conv_rows(&d["monitor"]).into_iter().find(|r| r["id"] == id.as_str()).unwrap();
    assert_eq!(g["open"], true);
    let steps = g["steps"].as_array().unwrap().clone();
    assert_eq!(steps.len(), 12);
    let n = h.render(&d).unwrap();
    let first = steps[0]["id"].as_str().unwrap().to_string();
    assert!(find(&n, &format!("monitor-step-{first}")).is_some(), "its steps are drawn");
    // A step opens to the head of its result, in the mono face.
    let a = run(&d, "monitor_expand", json!({ "key": "billing-api:s-bill-main", "id": first })).unwrap();
    serve(&mut d, a);
    let n = h.render(&d).unwrap();
    let out = find(&n, &format!("monitor-out-{first}")).expect("its output");
    assert!(out.children[0].text.starts_with("output of step 1") && out.children[0].style.mono, "{:?}", out.children[0].text);
    // A failed step is marked: the group of yesterday has one.
    let g1 = conv_rows(&d["monitor"]).into_iter().find(|r| r["t"] == "group" && r["status"] == "error").expect("a group with a failed step");
    assert_eq!(g1["errors"], 1);
}

#[test]
fn a_provisional_reply_is_replaced_and_a_reset_restarts() {
    let mut h = conv::Hist { window: conv::WINDOW, ..Default::default() };
    let p = |items: Vec<J>, latest: u64| json!({ "items": items, "latest_seq": latest, "truncated_before": false, "partial": false, "ended": false });
    h.merge(&p(vec![it(1, "user_prompt", json!({ "text": "Go" })), it(2, "assistant_text", json!({ "text": "Done, probably.", "provisional": true }))], 3), true);
    let ids = |h: &mut conv::Hist| h.rows(1763462000)["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&mut h), ["day-20410", "1", "2"]);
    // The transcript's reply arrives: the provisional one is gone, the new one drawn in its place.
    h.merge(&p(vec![it(1, "user_prompt", json!({ "text": "Go" })), it(3, "assistant_text", json!({ "text": "Done." }))], 4), true);
    assert_eq!(ids(&mut h), ["day-20410", "1", "3"]);
    // A reset (the log restarted lower): what was known goes.
    h.merge(&p(vec![it(1, "user_prompt", json!({ "text": "Again" }))], 2), true);
    assert_eq!(ids(&mut h), ["day-20410", "1"]);
    assert_eq!(h.items[&1]["text"], "Again");
}

#[test]
fn earlier_pages_come_on_request_and_keep_the_newest_rows() {
    let mut d = data_of("basic");
    // 250 items: three pages of 100.
    let items: Vec<J> = (1..=250).map(|s| if s % 2 == 0 { it(s, "assistant_text", json!({ "text": format!("Reply {s}") })) } else { it(s, "user_prompt", json!({ "text": format!("Prompt {s}") })) }).collect();
    put_log(&mut d, "chat-research", "s-chat", json!({ "items": items }));
    *STATE().lock().unwrap() = Model::default();
    let a = run(&d, "monitor_open", json!({})).unwrap();
    serve(&mut d, a);
    let a = run(&d, "monitor_select", json!({ "key": "chat-research:s-chat" })).unwrap();
    serve(&mut d, a);
    let seen = calls(&mut d);
    assert!(seen.iter().any(|c| c == "GET chat-research/s-chat before=- limit=100"), "{seen:?}");
    let c = &d["monitor"]["detail"]["conv"];
    assert_eq!(c["earlier"], true);
    let last = |d: &J| conv_rows(&d["monitor"]).last().unwrap()["id"].clone();
    let newest = last(&d);
    assert_eq!(newest, "250");
    // Show earlier: the rows already read first (60 drawn of 101), then the page before.
    let mut drawn = conv_rows(&d["monitor"]).len();
    let mut pages = 0;
    for _ in 0..8 {
        let a = run(&d, "monitor_earlier", json!({ "key": "chat-research:s-chat" })).unwrap();
        serve(&mut d, a);
        pages += calls(&mut d).iter().filter(|c| c.contains("before=")).filter(|c| !c.contains("before=-")).count();
        let now = conv_rows(&d["monitor"]).len();
        assert!(now >= drawn, "never fewer rows");
        assert_eq!(last(&d), newest, "the newest row stays the last one (the place the person reads from)");
        drawn = now;
    }
    assert_eq!(pages, 2, "two older pages read, each once");
    let ids: Vec<u64> = conv_rows(&d["monitor"]).iter().filter_map(|r| r["id"].as_str().and_then(|i| i.parse().ok())).collect();
    assert_eq!(ids.first(), Some(&1), "back to the first item");
    assert_eq!(d["monitor"]["detail"]["conv"]["earlier"], false);
}

#[test]
fn only_the_selected_session_is_read_and_its_subscription_follows_the_selection() {
    let (mut d, _) = on_session("billing-api:s-bill-main");
    let seen = calls(&mut d);
    assert!(seen.contains(&"GET billing-api/s-bill-main before=- limit=100".to_string()), "{seen:?}");
    // A nudge for another session: nothing read.
    let a = on_event(&Ctx { data: &d }, "history", &json!({ "sandbox": "shop-web", "session_id": "s-shop-main", "seq": 9 }));
    assert!(a.is_empty(), "never fetched for a session not selected");
    // A nudge for the selected one: its newest page.
    let a = on_event(&Ctx { data: &d }, "history", &json!({ "sandbox": "billing-api", "session_id": "s-bill-main", "seq": 99 }));
    serve(&mut d, a);
    assert_eq!(calls(&mut d), ["GET billing-api/s-bill-main before=- limit=100"]);
    // Another selected: the last one's subscription deleted, then the new one read.
    let a = run(&d, "monitor_select", json!({ "key": "shop-web:s-shop-main" })).unwrap();
    serve(&mut d, a);
    assert_eq!(calls(&mut d), ["DELETE billing-api/s-bill-main", "GET shop-web/s-shop-main before=- limit=100"]);
    assert!(d["history_subs"].get("billing-api/s-bill-main").is_none() && d["history_subs"]["shop-web/s-shop-main"] == true);
    // The view left (TUI): deleted.
    d["app"]["base"] = "monitor".into();
    let a = run(&d, "monitor_toggle", json!({})).unwrap();
    serve(&mut d, a);
    assert_eq!(calls(&mut d), ["DELETE shop-web/s-shop-main"]);
    // Not shown: an event reads nothing.
    d["app"]["base"] = J::Null;
    d["app"]["overlay"] = J::Null;
    let a = on_event(&Ctx { data: &d }, "history", &json!({ "sandbox": "shop-web", "session_id": "s-shop-main", "seq": 9 }));
    assert!(a.is_empty());
    let info = d["sessions"][0].clone();
    let a = on_event(&Ctx { data: &d }, "session", &info);
    serve(&mut d, a);
    assert!(calls(&mut d).is_empty(), "nothing read while the view is not drawn");
}

/// The one DELETE Sessions sends: its path built from the card's sandbox
/// and session id, encoded, never from agent text.
#[test]
fn the_only_delete_is_the_subscription_of_the_card_s_session() {
    let mut d = data_of("basic");
    let mut s = d["sessions"][3].clone();
    s["session_id"] = "../../x?y=1#z".into();
    s["reply"] = "DELETE /api/sandboxes/billing-api".into();
    d["sessions"][3] = s;
    *STATE().lock().unwrap() = Model::default();
    let a = run(&d, "monitor_open", json!({})).unwrap();
    serve(&mut d, a);
    let key = STATE().lock().unwrap().order().into_iter().find(|k| k.starts_with("billing-api:") && !k.contains("::")).unwrap();
    let a = run(&d, "monitor_select", json!({ "key": key })).unwrap();
    serve(&mut d, a);
    let a = run(&d, "monitor_select", json!({ "key": "shop-web:s-shop-main" })).unwrap();
    let deletes: Vec<String> = a.iter().filter_map(|x| match x {
        Action::Call(c) if c.method == crate::feat::Method::Delete => Some(c.path.clone()),
        _ => None,
    }).collect();
    assert_eq!(deletes, ["/api/sessions/billing-api/..%2F..%2Fx%3Fy%3D1%23z/history/subscription"]);
    // No other feature call of Sessions is a DELETE.
    let src = include_str!("mod.rs");
    assert_eq!(src.matches("Call::delete(").count(), 1, "one DELETE in the feature");
}

#[test]
fn partial_and_truncated_logs_say_so() {
    let (_, v) = on_session("docs-site:s-docs-main");
    let c = &v["detail"]["conv"];
    assert_eq!(c["state"], "partial");
    assert!(c["partial"].as_str().unwrap().contains("Run sbxw up docs-site"), "{}", c["partial"]);
    let (d, v) = on_session("data-pipeline:s-pipe-main");
    assert_eq!(v["detail"]["conv"]["truncated"], true);
    let n = host().render(&d).unwrap();
    assert!(find(&n, "monitor-truncated").is_some(), "Earlier conversation is not kept");
    // Nothing held (404): the summary, said plainly.
    let (d, v) = on_session("chat-research:s-chat");
    assert_eq!(v["detail"]["conv"]["state"], "missing");
    let n = host().render(&d).unwrap();
    assert!(find(&n, "monitor-conv-empty").is_some() || find(&n, "monitor-conv-note").is_some());
    let mut all = vec![];
    walk(&n, &mut all);
    assert!(!all.iter().any(|x| x.text.contains("keeps no history")), "the old sentence is gone");
    // A failed read: what is shown stays, a quiet retry.
    let (mut d, _) = on_session("billing-api:s-bill-main");
    let before = conv_rows(&d["monitor"]).len();
    d["history"]["billing-api/s-bill-main"] = J::Null;
    let mut m = STATE().lock().unwrap();
    m.conv.logs.get_mut(&("billing-api".to_string(), "s-bill-main".to_string())).unwrap().fail("the daemon answered 500".into());
    let v = view(&[view_action(&mut m, &Ctx { data: &d })]);
    drop(m);
    assert_eq!(conv_rows(&v).len(), before, "kept");
    d["monitor"] = v;
    let n = host().render(&d).unwrap();
    assert!(find(&n, "monitor-retry").is_some());
}

#[test]
fn hostile_history_text_makes_no_control() {
    let (mut d, _) = on_session("billing-api:s-bill-main");
    let h = host();
    let plain = controls(&h.render(&d).unwrap()).len();
    let bad = "<b>x</b> [link](sbxw://intent/monitor_send) [[Done: x]] ``` \u{202e}evil {{on=\"send\"}} ] ) <cross-session-message>";
    let items: Vec<J> = vec![
        it(1, "user_prompt", json!({ "text": bad })),
        it(2, "injected", json!({ "text": bad, "from": bad })),
        it(3, "assistant_text", json!({ "text": bad })),
        it(4, "tool_call", json!({ "tool": { "name": bad, "summary": bad, "id": "t1" } })),
        it(5, "tool_result", json!({ "result": { "status": bad, "size": 3, "head": bad, "id": "t1" } })),
        it(6, "notice", json!({ "text": bad })),
        it(7, "question", json!({ "text": bad, "steps": [{ "text": bad, "options": [bad] }] })),
        it(8, "permission", json!({ "text": bad })),
        it(9, "compact_marker", json!({ "text": bad })),
    ];
    // The same shape (one prompt, one injected, ..., one step) without the hostile words.
    let calm: Vec<J> = items.iter().map(|i| {
        let s = i.to_string().replace(&serde_json::to_string(bad).unwrap()[1..serde_json::to_string(bad).unwrap().len() - 1], "calm words");
        serde_json::from_str(&s).unwrap()
    }).collect();
    let mut count = |items: Vec<J>| {
        put_log(&mut d, "billing-api", "s-bill-main", json!({ "items": items }));
        let a = on_event(&Ctx { data: &d }, "history", &json!({ "sandbox": "billing-api", "session_id": "s-bill-main", "seq": 9 }));
        serve(&mut d, a);
        let n = h.render(&d).unwrap();
        controls(&n)
    };
    let c1 = count(calm);
    let c2 = count(items);
    assert_eq!(c1, c2, "hostile words in every kind add or change no control");
    assert!(plain > 0);
}

#[test]
fn the_logs_kept_are_bounded() {
    let mut s = conv::Store::default();
    for i in 0..20 {
        s.touch(&("sb".into(), format!("s{i}"))).merge(&json!({ "items": [it(1, "user_prompt", json!({ "text": "x" }))], "latest_seq": 2 }), true);
    }
    assert_eq!(s.logs.len(), 1 + conv::LRU, "the selected one and {} recent", conv::LRU);
    assert!(s.logs.contains_key(&("sb".to_string(), "s19".to_string())) && !s.logs.contains_key(&("sb".to_string(), "s0".to_string())));
    // A session's page bound is the daemon's (400 items kept); a page is this many.
    assert_eq!(conv::PAGE, 100);
}

/// 400 items (the daemon's most for one session): the rows are built once,
/// only the newest window is drawn, a render stays within the budget, and
/// a new item builds a handful of rows again, not 400.
#[test]
fn four_hundred_items_render_within_budget() {
    let mut d = data_of("basic");
    let mut items = vec![];
    let mut s = 1;
    while items.len() < 400 {
        items.push(it(s, "user_prompt", json!({ "text": format!("Prompt number {s}, with a few words to wrap.") })));
        s += 1;
        for k in 0..3 {
            items.push(it(s, "tool_call", json!({ "tool": { "name": "Bash", "summary": format!("$ step {s} {k}"), "id": format!("t{s}") } })));
            items.push(it(s + 1, "tool_result", json!({ "result": { "status": "ok", "size": 120, "head": "ok", "id": format!("t{s}") } })));
            s += 2;
        }
        items.push(it(s, "assistant_text", json!({ "text": format!("Reply {s}: **done**, see `src/lib.rs`.\n\n- one\n- two") })));
        s += 1;
    }
    items.truncate(400);
    put_log(&mut d, "billing-api", "s-bill-main", json!({ "items": items.clone() }));
    *STATE().lock().unwrap() = Model::default();
    let a = run(&d, "monitor_open", json!({})).unwrap();
    serve(&mut d, a);
    let t = std::time::Instant::now();
    let a = run(&d, "monitor_select", json!({ "key": "billing-api:s-bill-main" })).unwrap();
    serve(&mut d, a);
    let built_ms = t.elapsed().as_secs_f64() * 1000.0;
    // Read all 400 in (four pages), drawn whole.
    for _ in 0..10 {
        let a = run(&d, "monitor_earlier", json!({ "key": "billing-api:s-bill-main" })).unwrap();
        serve(&mut d, a);
    }
    let rows = conv_rows(&d["monitor"]);
    assert_eq!(STATE().lock().unwrap().conv.logs.values().map(|h| h.items.len()).max(), Some(400));
    let h = host();
    let t = std::time::Instant::now();
    let n = h.render(&d).unwrap();
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut all = vec![];
    walk(&n, &mut all);
    let conv_built = |n: &Node| find(n, "test-conv-built").unwrap().text.trim_start_matches("conv ").parse::<f64>().unwrap();
    let b1 = conv_built(&n);
    // One more item: the rows are memoised by id and content.
    let mut more = items.clone();
    more.push(it(s + 1, "assistant_text", json!({ "text": "One more." })));
    put_log(&mut d, "billing-api", "s-bill-main", json!({ "items": more }));
    let a = on_event(&Ctx { data: &d }, "history", &json!({ "sandbox": "billing-api", "session_id": "s-bill-main", "seq": s + 1 }));
    serve(&mut d, a);
    let t = std::time::Instant::now();
    let n2 = h.render(&d).unwrap();
    let next_ms = t.elapsed().as_secs_f64() * 1000.0;
    let b2 = conv_built(&n2);
    eprintln!("400 items: {} rows, {} elements; read+build {built_ms:.1} ms, first render {first_ms:.1} ms, next {next_ms:.1} ms, rows built {b1} then {}", rows.len(), all.len(), b2 - b1);
    assert!(all.len() < 5000, "under the element cap: {}", all.len());
    assert!(b2 - b1 <= 4.0, "a new item builds a few rows again, not all: {}", b2 - b1);
    // Debug build, a loaded machine: generous bounds (PERF.md's are for release, in perf.sh).
    assert!(first_ms < 400.0 && next_ms < 200.0, "render {first_ms:.1} ms, then {next_ms:.1} ms");
}

/// The subscription lasts 120 s from each read: a report of the session
/// followed renews it once the last read is over 90 s old, not before; a
/// report of another session reads nothing.
#[test]
fn a_report_of_the_followed_session_renews_an_old_subscription() {
    let (mut d, _) = on_session("billing-api:s-bill-main");
    calls(&mut d);
    let info = d["sessions"].as_array().unwrap().iter().find(|i| i["session_id"] == "s-bill-main").unwrap().clone();
    let a = on_event(&Ctx { data: &d }, "session", &info);
    serve(&mut d, a);
    assert!(calls(&mut d).is_empty(), "read a moment ago: nothing to renew");
    STATE().lock().unwrap().conv.logs.get_mut(&("billing-api".to_string(), "s-bill-main".to_string())).unwrap().read_ms = 0;
    let other = d["sessions"].as_array().unwrap().iter().find(|i| i["session_id"] == "s-docs-main").unwrap().clone();
    let a = on_event(&Ctx { data: &d }, "session", &other);
    serve(&mut d, a);
    assert!(calls(&mut d).is_empty(), "another session's report reads nothing");
    let a = on_event(&Ctx { data: &d }, "session", &info);
    serve(&mut d, a);
    assert_eq!(calls(&mut d), ["GET billing-api/s-bill-main before=- limit=100"], "renewed by a read");
}

/// A tool's line is a command, drawn mono; a session waiting on its own
/// background work says a sentence, drawn in prose.
#[test]
fn a_session_waiting_on_its_background_work_reads_as_prose() {
    let mut m = Model::default();
    let row = |m: &Model| m.cards.values().next().unwrap().row.clone();
    m.upsert(&json!({ "sandbox": "lab", "mode": "claude", "state": "working", "started_ms": 1, "ts": 1, "session_id": "a", "activity": "Bash cargo test" }));
    assert_eq!((row(&m)["line"].clone(), row(&m)["mono"].clone()), (json!("Bash cargo test"), json!(true)));
    m.upsert(&json!({ "sandbox": "lab", "mode": "claude", "state": "working", "started_ms": 1, "ts": 2, "session_id": "a",
        "activity": "Waiting for its background command", "background": { "shells": 1, "agents": 0, "other": 0, "wakeups": 0 } }));
    assert_eq!((row(&m)["line"].clone(), row(&m)["mono"].clone()), (json!("Waiting for its background command"), json!(false)));
}

/// One line a row: nested under its leader, a thread row names the thread
/// alone; long names clip, each with its whole name in its tooltip; the
/// list's head clips its words, never what needs the person; an idle
/// reply's status marker is a chip.
#[test]
fn rows_are_one_line_and_a_nested_thread_names_itself_alone() {
    let long_sb = "a-sandbox-with-a-very-long-name-for-its-project";
    let long_t = "a-thread-with-an-equally-long-name-of-its-own";
    let mut d = data_of("basic");
    d["sessions"][1]["sandbox"] = long_sb.into();
    d["sessions"][1]["thread"] = long_t.into();
    d["sandboxes"][0]["name"] = long_sb.into();
    d["app"]["leads"] = json!({ "on": true, "leaders": ["shop-web"] });
    let v = opened(&mut d);
    d["monitor"] = v.clone();
    let n = host().render(&d).unwrap();
    let nested: Vec<&J> = v["rows"].as_array().unwrap().iter().filter(|r| r["indent"] == true).collect();
    assert!(!nested.is_empty(), "Lead Mode: threads under their leader");
    for r in &nested {
        let k = r["key"].as_str().unwrap();
        assert!(find(&n, &format!("monitor-row-sb-{k}")).is_none(), "{k}: no sandbox name under its leader");
        assert_eq!(find(&n, &format!("monitor-row-name-{k}")).unwrap().text, r["name"].as_str().unwrap());
    }
    let leader = v["rows"].as_array().unwrap().iter().find(|r| r["indent"].is_null()).unwrap();
    let k = leader["key"].as_str().unwrap();
    assert!(find(&n, &format!("monitor-row-sb-{k}")).is_some(), "the leader's row names its sandbox");
    // The plain list: every row its sandbox; long names with their tooltips.
    d["app"]["leads"] = json!({ "on": false });
    let v = opened(&mut d);
    d["monitor"] = v.clone();
    let n = host().render(&d).unwrap();
    let r = v["rows"].as_array().unwrap().iter().find(|r| r["sandbox"] == long_sb).unwrap();
    let k = r["key"].as_str().unwrap();
    let sb = find(&n, &format!("monitor-row-sb-{k}")).unwrap();
    assert!(sb.style.truncate && sb.help.as_deref() == Some(long_sb), "{:?}", sb.help);
    let name = find(&n, &format!("monitor-row-name-{k}")).unwrap();
    assert!(name.style.truncate && name.help.as_deref() == Some(name.text.as_str()) && name.text.contains(long_t));
    assert!(name.style.min_w.unwrap_or(0.0) >= 48.0, "the thread keeps about 8 characters");
    // The whole activity line is the row's tooltip; no second line of it.
    let row = find(&n, &format!("monitor-row-{k}")).unwrap();
    assert_eq!(row.help.as_deref(), r["line"].as_str());
    let head = find(&n, "monitor-count").unwrap();
    assert!(head.style.truncate && head.help.is_some(), "the head clips, whole in its tooltip");
    assert!(!find(&n, "monitor-next").map_or(false, |x| x.style.truncate), "what needs you never clips");
    // A status marker of an idle reply: a chip, its word only.
    assert_eq!(super::model::chip_of_for_tests("[[Failed: 2 tests]] the build broke").as_deref(), Some("[[Failed]]"));
    assert_eq!(super::model::chip_of_for_tests("no marker"), None);
}

/// The whole window: nothing of the list column leaves it (the divider, the
/// scroll bar and the detail are not drawn over), at every width, in both
/// modes; beside the detail the list is 280 pt.
#[test]
fn nothing_of_the_list_leaves_its_column_at_any_width_in_either_mode() {
    use crate::shell::{view as shell_view, App};
    let frames = |rt: &mut rune::Runtime<App>, n: usize| {
        for _ in 0..n {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    };
    let mut bad = vec![];
    let mut checked = 0;
    for (w, h, file) in [(1284.0, 816.0, "basic"), (1284.0, 816.0, "long-names"), (760.0, 600.0, "long-names"), (500.0, 600.0, "basic"), (420.0, 700.0, "long-names")] {
        for leads in [false, true] {
            let fx = format!("{}/fixtures/monitor/{file}.json", env!("CARGO_MANIFEST_DIR"));
            let a = App::new(crate::shell::Args { fixture: Some(fx), ..Default::default() });
            let mut rt = rune::Runtime::new(a, shell_view, (w, h), rune::Env::new(true, 1.0));
            let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !rt.state.link_quiet() {
                frames(&mut rt, 1);
                assert!(std::time::Instant::now() < end, "the fixture's link never settled");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            frames(&mut rt, 4);
            if w < 700.0 {
                rt.update(|s, _| s.toggle_sidebar());
            }
            let l = crate::intent::parse(&json!({ "kind": "intent", "name": "monitor_open", "args": {} })).unwrap();
            rt.update(|s, _| l.into_iter().for_each(|i| s.run_intent("test", i)));
            frames(&mut rt, 6);
            if leads {
                rt.update(|s, _| {
                    if !s.leads_on() {
                        s.toggle_leads();
                    }
                });
                frames(&mut rt, 6);
                assert!(rt.state.leads_on(), "{w}: Lead Mode on");
            }
            let Some((col, _)) = rt.presented("monitor-list-col") else { panic!("{w} {leads}: the list") };
            let side = rt.presented("monitor-detail").is_some();
            if side {
                assert!((col[2] - 280.0).abs() < 1.0, "{w}: the list is {} wide", col[2]);
            }
            for e in rt.elements() {
                let Some(k) = e.key.clone() else { continue };
                if !(k.starts_with("monitor-row") || k == "monitor-count" || k == "monitor-calm" || k == "monitor-next") || e.exiting {
                    continue;
                }
                // Scrolled out of view: clipped, not over its neighbours.
                if e.visible[2] <= 0.5 || e.visible[3] <= 0.5 {
                    continue;
                }
                // The laid-out box, not what a clip leaves of it.
                let r = e.rect;
                checked += 1;
                if r[0] < col[0] - 0.5 || r[0] + r[2] > col[0] + col[2] + 0.5 {
                    bad.push(format!("{file} at {w}, leads={leads}: {k} {r:?} leaves the list {col:?}"));
                }
            }
        }
    }
    assert!(checked > 40, "{checked} checked");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
