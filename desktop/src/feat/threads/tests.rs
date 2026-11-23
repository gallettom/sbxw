use super::*;
use crate::feat::answer_from;

fn fixture(name: &str) -> J {
    let p = crate::fixture::dir().join("threads").join(format!("{name}.json"));
    let f: J = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let mut d = f["daemon"].clone();
    // What the core adds: `app` (two panes, the first on web-app's main session).
    d["app"] = json!({ "selected": "web-app", "layout": { "panes": 2, "split": "columns" }, "modal": null,
        "panes": [ { "index": 1, "sandbox": "web-app", "mode": "agent", "thread": null, "focused": true, "status": "connected" },
                   { "index": 2, "sandbox": null, "mode": "agent", "thread": null, "focused": false, "status": "empty" } ] });
    d
}

fn asked(name: &'static str, args: J) -> Asked {
    // Checked as the core checks it, against this feature's own row.
    crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap_or_else(|e| panic!("{name}: {e}"));
    Asked { name, args: args.as_object().cloned().unwrap_or_default() }
}

fn run(data: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    act(&Ctx { data }, &asked(name, args))
}

/// Applies the `Set`s of a batch of actions to `data`, as the core does.
fn apply(data: &mut J, acts: &[Action]) {
    for a in acts {
        if let Action::Set { key, value } = a {
            data[key.as_str()] = value.clone();
        }
    }
}

fn prepare(v: Vec<Action>) -> Prepare {
    assert_eq!(v.len(), 1, "{v:?}");
    match v.into_iter().next().unwrap() {
        Action::Prepare(p) => *p,
        a => panic!("not a prepare: {a:?}"),
    }
}

fn confirm(v: Vec<Action>) -> Confirm {
    assert_eq!(v.len(), 1, "{v:?}");
    match v.into_iter().next().unwrap() {
        Action::Confirm(c) => *c,
        a => panic!("not a confirm: {a:?}"),
    }
}

fn spec(name: &str) -> &'static Spec {
    INTENTS.iter().find(|s| s.name == name).unwrap()
}

#[test]
fn approving_merging_and_closing_only_ever_open_the_core_dialog() {
    let pending = fixture("plan-pending");
    let merge = fixture("merge-proposed");
    let basic = fixture("basic");
    for (data, name, args) in [
        (&pending, "plan_approve", json!({ "id": "p-1" })),
        (&merge, "plan_merge", json!({ "id": "p-1" })),
        (&basic, "thread_close", json!({ "sandbox": "web-app", "thread": "t1" })),
        (&basic, "remote_login", json!({ "sandbox": "web-app" })),
    ] {
        assert!(matches!(spec(name).risk, Risk::Approval(_)), "{name} must be risk Approval");
        let v = run(data, name, args).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(v.len(), 1, "{name}: {v:?}");
        assert!(matches!(v[0], Action::Confirm(_) | Action::Prepare(_)), "{name}: {v:?}");
    }
    // Nothing else reaches those routes: no Direct intent of this feature
    // calls approve or merge, whatever its arguments.
    for s in INTENTS.iter().filter(|s| matches!(s.risk, Risk::Direct)) {
        for args in [json!({ "id": "p-1", "sandbox": "web-app", "thread": "t1", "index": 1, "field": "goal", "value": "x", "variant": "split", "on": true, "name": "t9", "kind": "thread", "tab": "board" })] {
            let mut a = Map::new();
            for (k, _, _) in s.args {
                if let Some(v) = args.get(*k) {
                    a.insert(k.to_string(), v.clone());
                }
            }
            for x in act(&Ctx { data: &pending }, &Asked { name: s.name, args: a }).unwrap_or_default() {
                if let Action::Call(c) = x {
                    assert!(!c.path.contains("/approve") && !c.path.contains("/merge") && !c.path.ends_with("/close"), "{} calls {}", s.name, c.path);
                }
            }
        }
    }
}

#[test]
fn the_plan_screen_asks_and_never_approves_by_itself() {
    // Every action the plan screen has, pressed: what it returns is intents
    // the core checks; Approve and Merge are only the Approval rows.
    let mut data = fixture("plan-pending");
    data["app"]["modal"] = json!({ "screen": "plans", "arg": "p-1" });
    let dir = std::env::temp_dir().join("sbxw-threads-test-no-overrides");
    let mut s = crate::screen::ScreenState::new("plans", &dir, data);
    let got = s.act("approve", &json!("p-1"), &J::Null).expect("intents");
    assert_eq!(got.len(), 1);
    match &got[0] {
        crate::intent::Intent::Feature(a) => assert_eq!(a.name, "plan_approve"),
        i => panic!("{i:?}"),
    }
    let got = s.act("merge", &json!("p-1"), &J::Null).expect("intents");
    assert!(matches!(&got[0], crate::intent::Intent::Feature(a) if a.name == "plan_merge"));
}

#[test]
fn the_approval_dialog_lists_the_draft_and_the_sessions_it_starts() {
    let mut data = fixture("plan-pending");
    // The person removes web-orders, edits the docs goal, picks Haiku, shares.
    for (name, args) in [
        ("plan_remove", json!({ "id": "p-1", "index": 2 })),
        ("plan_edit", json!({ "id": "p-1", "index": 2, "field": "goal", "value": "Document the v2 orders API, with curl examples." })),
        ("plan_edit", json!({ "id": "p-1", "index": 2, "field": "model", "value": "claude-haiku-5-5" })),
        ("plan_variant", json!({ "id": "p-1", "variant": "shared" })),
    ] {
        let acts = run(&data, name, args).unwrap();
        apply(&mut data, &acts);
    }
    let p = prepare(run(&data, "plan_approve", json!({ "id": "p-1" })).unwrap());
    assert_eq!(p.call.path, "/api/sandboxes/web-app/threads", "the core reads the sessions itself");
    let view = json!({ "ok": true, "open": 2, "max": 5, "threads": [ { "name": "t1", "open": true } ] });
    let c = (p.build)(&Ctx { data: &data }, &p.asked, &view).unwrap();
    assert_eq!(c.call.path, "/api/plans/p-1/approve");
    assert_eq!(c.call.body["variant"], "shared");
    let names: Vec<&str> = c.call.body["threads"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["api-routes", "docs"]);
    assert_eq!(c.call.body["threads"][1]["model"], "claude-haiku-5-5");
    assert_eq!(c.call.body["threads"][1]["goal"], "Document the v2 orders API, with curl examples.");
    let preview = &c.preview.as_ref().unwrap().text;
    assert!(preview.contains("1. api-routes (Sonnet 5.5, in api)") && preview.contains("2. docs (Haiku 5.5)"), "{preview}");
    let all = c.lines.join("\n");
    assert!(all.contains("Removed: web-orders"), "{all}");
    assert!(all.contains("docs: goal edited, model claude-haiku-5-5"), "{all}");
    assert!(all.contains("Variant: shared instead of split"), "{all}");
    assert!(all.contains("Starts 2 new Claude sessions: web-app has 2 of 5 open now (main, t1), 3 free."), "{all}");
}

#[test]
fn over_the_ceiling_the_dialog_says_how_many_which_and_how_to_free_one() {
    let data = fixture("plan-pending");
    let p = prepare(run(&data, "plan_approve", json!({ "id": "p-1" })).unwrap());
    let view = json!({ "ok": true, "open": 4, "max": 5, "threads": [ { "name": "t1", "open": true }, { "name": "t2", "open": true }, { "name": "s1", "open": true } ] });
    let e = (p.build)(&Ctx { data: &data }, &p.asked, &view).unwrap_err();
    assert!(e.contains("needs 3 new Claude sessions"), "{e}");
    assert!(e.contains("4 sessions are open now (main, t1, t2, s1)"), "{e}");
    assert!(e.contains("1 place is free"), "{e}");
    assert!(e.contains("remove 2 threads from the plan"), "{e}");
    assert!(e.contains("max_agent_sessions"), "{e}");
}

#[test]
fn the_daemon_refusing_the_plan_is_said_in_plain_words_in_the_dialog() {
    let data = fixture("plan-ceiling");
    let call = Call::post("/api/plans/p-1/approve", json!({}), "approve").tag("approve:p-1");
    let msg = "3 new thread(s) and 4 session(s) already open is more than max_agent_sessions = 5. Remove threads, or raise the ceiling in sbxw.toml.";
    let acts = on_answer(&Ctx { data: &data }, &call, Ok((400, json!({ "ok": false, "error": msg }))));
    let mut d = data.clone();
    apply(&mut d, &acts);
    let t = d[UI]["plan_error"]["p-1"].as_str().unwrap();
    assert!(t.contains("4 sessions are open now (main, t1, t2, s1)") && t.contains("1 place is free") && t.contains("close a thread"), "{t}");
    assert!(acts.iter().any(|a| matches!(a, Action::OpenModal { screen, .. } if screen == "plans")));
}

#[test]
fn a_draft_needs_a_named_thread_with_a_goal() {
    let mut data = fixture("plan-pending");
    for _ in 0..3 {
        let acts = run(&data, "plan_remove", json!({ "id": "p-1", "index": 1 })).unwrap();
        apply(&mut data, &acts);
    }
    assert!(run(&data, "plan_approve", json!({ "id": "p-1" })).unwrap_err().contains("at least one thread"));
    let acts = run(&data, "plan_add", json!({ "id": "p-1" })).unwrap();
    apply(&mut data, &acts);
    assert!(run(&data, "plan_approve", json!({ "id": "p-1" })).unwrap_err().contains("Unnamed thread"));
    let acts = run(&data, "plan_edit", json!({ "id": "p-1", "index": 1, "field": "name", "value": "Bad Name" })).unwrap();
    apply(&mut data, &acts);
    assert!(run(&data, "plan_approve", json!({ "id": "p-1" })).unwrap_err().contains("lower-case"));
    // Reset puts it back as proposed.
    let acts = run(&data, "plan_reset", json!({ "id": "p-1" })).unwrap();
    apply(&mut data, &acts);
    assert_eq!(data[UI]["drafts"]["p-1"]["threads"].as_array().unwrap().len(), 3);
}

#[test]
fn closing_a_dirty_thread_needs_its_files_discarded_and_a_refusal_shows_the_daemons_reason() {
    let data = fixture("basic");
    let p = prepare(run(&data, "thread_close", json!({ "sandbox": "web-app", "thread": "t2" })).unwrap());
    assert_eq!(p.call.path, "/api/sandboxes/web-app/threads/t2");
    let st = json!({ "ok": true, "status": { "worktree": "/work/.sbxw-worktrees/t2", "branch": "sbxw/t2", "base": "main", "has_branch": true, "dirty": 2, "ahead": 3 } });
    let c = (p.build)(&Ctx { data: &data }, &p.asked, &st).unwrap();
    assert!(c.danger);
    assert_eq!(c.ack.as_deref(), Some("Discard 2 uncommitted files in it"));
    assert_eq!(c.call.body, json!({ "discard": true, "delete_branch": false }), "commits not in main keep the branch");
    assert!(c.lines.iter().any(|l| l.contains("Branch sbxw/t2 is kept: its 3 commits are not in main")));
    let clean = json!({ "ok": true, "status": { "worktree": "/w/t1", "branch": "sbxw/t1", "base": "main", "has_branch": true, "dirty": 0, "ahead": 0 } });
    let c = (p.build)(&Ctx { data: &data }, &p.asked, &clean).unwrap();
    assert!(c.ack.is_none());
    assert_eq!(c.call.body, json!({ "discard": false, "delete_branch": true }));
    let same = json!({ "ok": true, "status": { "worktree": "/work", "same_worktree": true } });
    assert_eq!((p.build)(&Ctx { data: &data }, &p.asked, &same).unwrap().title, "Close session t2?");
    // The stand-in refuses a dirty close without discard, in the daemon's words, and the notice says them.
    let mut d = fixture("basic");
    d["thread_status_of"] = json!({ "web-app": { "t2": { "dirty": 2 } } });
    let (code, body) = answer_from(&mut d, "POST", "/api/sandboxes/web-app/threads/t2/close", &json!({ "discard": false }), &mut |_, _| {});
    assert_eq!(code, 500);
    let call = Call::post("/x", json!({}), "close t2").tag("close:web-app\nt2");
    let acts = on_answer(&Ctx { data: &d }, &call, Ok((code, body)));
    assert!(matches!(&acts[0], Action::Notice { text, error: true, .. } if text == "t2 of web-app was not closed: /work/.sbxw-worktrees/t2 has 2 uncommitted file(s)"), "{acts:?}");
    let (code, body) = answer_from(&mut d, "POST", "/api/sandboxes/web-app/threads/t2/close", &json!({ "discard": true }), &mut |_, _| {});
    assert_eq!(code, 200);
    let acts = on_answer(&Ctx { data: &d }, &call, Ok((code, body)));
    assert!(acts.contains(&Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": "web-app", "thread": "t2" }) }), "its panes go: {acts:?}");
    assert!(!d["threads"]["web-app"]["threads"].as_array().unwrap().iter().any(|t| t["name"] == "t2"));
}

#[test]
fn a_new_thread_opens_with_its_repo_model_and_kind_and_no_place_is_said() {
    let data = fixture("basic");
    let v = run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "t3", "repo": "=api", "model": "claude-haiku-5-5" })).unwrap();
    assert!(v.contains(&Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": "web-app", "thread": "t3", "pane": "spare", "repo": "api", "model": "claude-haiku-5-5" }) }), "{v:?}");
    // The workspace itself, and the default model left unsaid.
    let v = run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "t3", "repo": "=", "model": "claude-opus-5-5" })).unwrap();
    assert!(v.contains(&Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": "web-app", "thread": "t3", "pane": "spare", "repo": "" }) }), "{v:?}");
    let v = run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "s2", "checkout": true })).unwrap();
    assert!(v.contains(&Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": "web-app", "thread": "s2", "pane": "spare", "checkout": true }) }), "{v:?}");
    // An existing one reopens without them; one of the other kind is refused.
    let v = run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "old-fix", "repo": "=api" })).unwrap();
    assert!(v.contains(&Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": "web-app", "thread": "old-fix", "pane": "spare" }) }), "{v:?}");
    assert!(run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "s1", "checkout": false })).unwrap_err().contains("session on the main checkout"));
    assert!(run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "Bad" })).is_err());
    let full = fixture("new-full");
    let e = run(&full, "thread_open", json!({ "sandbox": "web-app", "name": "t5" })).unwrap_err();
    assert!(e.contains("5 of its 5 Claude sessions open (main, t1, t2, t3, t4), no place is free"), "{e}");
}

#[test]
fn sessions_say_where_each_stands() {
    let data = fixture("basic");
    let s = model::sessions(&data, "web-app");
    let got: Vec<(Option<&str>, &str, &str)> = s.iter().map(|x| (x.thread.as_deref(), x.kind.as_str(), x.state)).collect();
    assert_eq!(got, [(None, "same", "working"), (Some("s1"), "same", "idle"), (Some("old-fix"), "own", "stopped"), (Some("t1"), "own", "working"), (Some("t2"), "own", "wants")]);
    assert_eq!(s[4].word(), "wants you");
    // Stopped sandbox: everything stopped.
    assert!(model::sessions(&data, "docs").iter().all(|x| x.state == "stopped"));
}

#[test]
fn extras_put_rows_under_the_sandbox_and_controls_on_each_sessions_pane() {
    let data = fixture("basic");
    let x = extras(&Ctx { data: &data });
    let rows: Vec<&str> = x.iter().filter(|e| e.place == Place::SandboxRow("web-app".into())).map(|e| e.label.as_str()).collect();
    assert_eq!(rows, ["✦ main · working", "✦ s1", "⑂ old-fix · stopped", "⑂ t1 · working", "⑂ t2 · wants you"]);
    let t2 = Place::Session { sandbox: "web-app".into(), thread: Some("t2".into()) };
    let ids: Vec<&str> = x.iter().filter(|e| e.place == t2).map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["state", "interrupt", "remote", "close"]);
    let main = Place::Session { sandbox: "web-app".into(), thread: None };
    assert!(x.iter().any(|e| e.place == main && e.id == "lead"));
    // No rows for a sandbox with only its main session.
    assert!(!x.iter().any(|e| e.place == Place::SandboxRow("api".into())));
    let x = extras(&Ctx { data: &fixture("plan-pending") });
    assert!(x.iter().any(|e| e.place == Place::Topbar && e.id == "plan" && e.tone == "amber"));
    assert!(x.iter().any(|e| e.place == main && e.id == "plan"));
}

#[test]
fn a_plan_arriving_opens_its_dialog_and_an_approval_opens_no_pane() {
    let data = fixture("basic");
    let old = json!([]);
    let p = fixture("plan-pending")["plans"].clone();
    let acts = on_data(&Ctx { data: &data }, "plans", &old, &p);
    assert!(acts.iter().any(|a| matches!(a, Action::OpenModal { screen, arg } if screen == "plans" && arg == "p-1")), "{acts:?}");
    let mut d = data.clone();
    apply(&mut d, &acts);
    assert!(d[UI]["drafts"]["p-1"].is_object());
    // Approved elsewhere: no pane opens, no page shows, the focus stays.
    let mut approved = p.clone();
    approved[0]["state"] = json!("approved");
    let acts = on_data(&Ctx { data: &d }, "plans", &p, &approved);
    assert!(!acts.iter().any(|a| matches!(a, Action::Intent { .. })), "{acts:?}");
    // Approved here: the dialog closes, a notice says the threads start, no pane.
    let call = Call::post("/api/plans/p-1/approve", json!({}), "approve").tag("approve:p-1");
    let acts = on_answer(&Ctx { data: &d }, &call, Ok((200, json!({ "ok": true, "plan": approved[0] }))));
    assert!(acts.contains(&Action::CloseModal), "{acts:?}");
    assert!(acts.iter().any(|a| matches!(a, Action::Notice { text, .. } if text == "Plan approved: 3 threads starting")), "{acts:?}");
    assert!(!acts.iter().any(|a| matches!(a, Action::Intent { .. })), "{acts:?}");
}

#[test]
fn the_merge_dialog_shows_the_leads_order_and_note_and_a_claimed_file_holds_it() {
    let data = fixture("merge-proposed");
    let c = confirm(run(&data, "plan_merge", json!({ "id": "p-1" })).unwrap());
    assert_eq!(c.preview.unwrap().text, "1. api-routes\n2. web-orders\n3. docs");
    assert!(c.lines.iter().any(|l| l == "The lead's note: api first: the web page depends on its routes."));
    assert!(c.lines.iter().any(|l| l.starts_with("Then each merged thread is closed")));
    assert_eq!(c.call.body, json!({ "close_threads": true }));
    let mut d = data.clone();
    apply(&mut d, &run(&data, "merge_close_set", json!({ "id": "p-1", "on": false })).unwrap());
    assert_eq!(confirm(run(&d, "plan_merge", json!({ "id": "p-1" })).unwrap()).call.body, json!({ "close_threads": false }));
    let e = run(&fixture("merge-shared"), "plan_merge", json!({ "id": "p-2" })).unwrap_err();
    assert!(e.contains("2 files are still claimed") && e.contains("web/src/api/orders.ts (web-orders)"), "{e}");
    // A failed merge is retried; a merged one is over.
    assert_eq!(confirm(run(&fixture("merge-failed"), "plan_merge", json!({ "id": "p-1" })).unwrap()).confirm, "Retry merge");
    assert!(run(&fixture("merge-merged"), "plan_merge", json!({ "id": "p-1" })).is_err());
}

#[test]
fn merge_rows_say_merged_stopped_and_not_merged() {
    let (rows, out) = model::merge_rows(&fixture("merge-failed")["plans"][0]);
    let st: Vec<&str> = rows.iter().map(|r| r["state"].as_str().unwrap()).collect();
    assert_eq!(st, ["ok", "failed", "skipped"]);
    assert!(out.starts_with("Stopped at web-orders: CONFLICT") && out.ends_with("Not merged: docs."), "{out}");
    let (_, out) = model::merge_rows(&fixture("merge-merged")["plans"][0]);
    assert_eq!(out, "Merged into main.");
}

/// The scripted run against the stand-in's routes: Team mode, a plan
/// arrives, is refused over the ceiling then approved, its threads start and
/// report, the lead proposes the merge, it stops at a conflict, a retry
/// merges, and a thread the merge could not close is said.
#[test]
fn a_whole_plan_against_the_standin() {
    let mut d = fixture("standin");
    d["standin_plan_step_ms"] = json!(0);
    let mut events: Vec<(String, J)> = vec![];
    let r = answer_from(&mut d, "POST", "/api/sandboxes/web-app/lead", &json!({}), &mut |n, v| events.push((n.to_string(), v)));
    assert_eq!(r.0, 200);
    assert!(events.iter().any(|(n, v)| n == "plan" && v["state"] == "pending"));
    let call = |d: &mut J, m: &str, p: &str, b: J| answer_from(d, m, p, &b, &mut |_, _| {});
    let id = d["plans"][0]["id"].as_str().unwrap().to_string();
    // Over the ceiling: refused in the daemon's words.
    d["threads"]["web-app"]["open"] = json!(4);
    let body = model::approve_body(&model::draft_of(&d["plans"][0]));
    let (code, v) = call(&mut d, "POST", &format!("/api/plans/{id}/approve"), body.clone());
    assert_eq!(code, 400);
    assert_eq!(model::parse_ceiling(v["error"].as_str().unwrap()), Some((3, 4, 5)));
    d["threads"]["web-app"]["open"] = json!(1);
    let (code, v) = call(&mut d, "POST", &format!("/api/plans/{id}/approve"), body);
    assert_eq!((code, v["plan"]["state"].as_str()), (200, Some("approved")));
    assert_eq!(d["threads"]["web-app"]["open"], 4);
    // Each read moves it on: working, done, then the merge proposed.
    for _ in 0..3 {
        call(&mut d, "GET", "/api/plans", J::Null);
    }
    let p = &d["plans"][0];
    assert!(p["threads"].as_array().unwrap().iter().all(|t| t["state"] == "done"));
    assert_eq!(p["merge"]["state"], "proposed");
    // The merge stops at web-orders, then a retry merges the rest.
    let (_, v) = call(&mut d, "POST", &format!("/api/plans/{id}/merge"), json!({ "close_threads": true }));
    assert_eq!(v["merge"]["state"], "failed");
    let (_, out) = model::merge_rows(&d["plans"][0]);
    assert!(out.contains("Stopped at web-orders") && out.contains("Not merged: docs"), "{out}");
    d["standin_close_refuse"] = json!(["docs"]);
    let (_, v) = call(&mut d, "POST", &format!("/api/plans/{id}/merge"), json!({ "close_threads": true }));
    assert_eq!(v["merge"]["state"], "merged");
    let left: Vec<&str> = d["threads"]["web-app"]["threads"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(left, ["docs"]);
    // The feature checks who was not closed and says so.
    let mut ui = json!({ "merge_check": { &id: { "sandbox": "web-app", "threads": ["api-routes", "web-orders", "docs"] } } });
    let mut data = d.clone();
    data[UI] = ui.take();
    let acts = on_answer(&Ctx { data: &data }, &Call::get("/x", "x").tag(&format!("merge-check:{id}")), Ok((200, d["threads"]["web-app"].clone())));
    assert!(acts.contains(&Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": "web-app", "thread": "api-routes" }) }), "closed threads leave the screen");
    assert!(!acts.contains(&Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": "web-app", "thread": "docs" }) }), "one still there keeps its pane");
    apply(&mut data, &acts);
    assert!(data[UI]["merge_note"][&id].as_str().unwrap().starts_with("Not closed: docs."));
}

#[test]
fn a_thread_opened_on_the_standins_ws_is_recorded_as_the_daemon_does() {
    let mut d = fixture("basic");
    let mut ev = vec![];
    answer_from(&mut d, "OPEN", "/ws?sandbox=web-app&mode=thread&thread=t9&repo=api&model=claude-haiku-5-5", &J::Null, &mut |n, v| ev.push((n.to_string(), v)));
    let t = d["threads"]["web-app"]["threads"].as_array().unwrap().iter().find(|t| t["name"] == "t9").cloned().unwrap();
    assert_eq!((t["open"].clone(), t["repo"].clone(), t["model"].clone(), t["worktree"].clone()), (json!(true), json!("api"), json!("claude-haiku-5-5"), json!("own")));
    assert_eq!(d["threads"]["web-app"]["open"], 5);
    assert!(ev.iter().any(|(n, v)| n == "session" && v["thread"] == "t9"));
    answer_from(&mut d, "OPEN", "/ws?sandbox=web-app&mode=thread&thread=s9&checkout=1", &J::Null, &mut |_, _| {});
    assert!(model::sessions(&d, "web-app").iter().any(|s| s.thread.as_deref() == Some("s9") && s.kind == "same"));
}

/// Pages (the desktop's layout): a five-thread plan at three per page opens
/// every thread (no cap of four), spilling onto following pages through the
/// core's spare rule, then shows the page of the first one.
#[test]
fn the_worktrees_tab_is_kept_while_it_reads_the_worktrees() {
    let mut data = fixture("basic");
    let acts = run(&data, "threads_show", json!({ "sandbox": "web-app", "tab": "worktrees" })).unwrap();
    let sets = acts.iter().filter(|a| matches!(a, Action::Set { key, .. } if key == UI)).count();
    assert_eq!(sets, 1, "{acts:?}");
    apply(&mut data, &acts);
    assert_eq!(data[UI]["tab"], "worktrees");
    assert_eq!(data[UI]["worktrees_for"], "web-app");
    assert!(acts.iter().any(|a| matches!(a, Action::Fetch { key, .. } if key == "worktrees")));
}

#[test]
fn the_dialog_normalizes_its_name_and_every_other_caller_stays_strict() {
    let data = fixture("basic");
    // The dialog (`normalize`): a fixable name opens, as its normalized self.
    let v = run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "  Fix Login/Bug ", "normalize": true })).unwrap();
    let open = v.iter().find_map(|a| if let Action::Intent { name, args } = a { (name == "open_terminal").then(|| args.clone()) } else { None }).unwrap();
    assert_eq!(open["thread"], "fix-login-bug");
    // Nothing left after the rule: asked for a name, not refused for its letters.
    let e = run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "!!!", "normalize": true })).unwrap_err();
    assert!(e.contains("Give it a name"), "{e}");
    // A name taken by the other kind is still said.
    assert!(run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "S1", "checkout": false, "normalize": true })).unwrap_err().contains("session on the main checkout"));
    // Without `normalize` (chips, scripts, plans) the strict rule holds.
    assert!(run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "Fix Login" })).unwrap_err().contains("inner hyphens"));
    assert!(run(&data, "thread_open", json!({ "sandbox": "web-app", "name": "fix-" })).is_err());
}

#[test]
fn the_name_intent_keeps_what_the_field_becomes() {
    let data = fixture("basic");
    let mut d = data.clone();
    apply(&mut d, &run(&data, "thread_name", json!({ "name": "Fix_Login " })).unwrap());
    let draft = &d[UI]["draft"];
    assert_eq!((draft["raw"].as_str(), draft["typing"].as_str(), draft["full"].as_str()), (Some("Fix_Login "), Some("fix-login-"), Some("fix-login")));
}

#[test]
fn a_thread_closed_elsewhere_drops_its_panes_and_says_so_once_and_one_never_shown_is_ignored() {
    let mut data = fixture("basic");
    data["app"]["panes"][1] = json!({ "index": 2, "sandbox": "web-app", "mode": "agent", "thread": "t2", "focused": false, "status": "connected" });
    let ev = json!({ "sandbox": "web-app", "thread": "t2" });
    let acts = on_event(&Ctx { data: &data }, "thread-closed", &ev);
    assert!(acts.contains(&Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": "web-app", "thread": "t2" }) }), "{acts:?}");
    assert!(acts.contains(&Action::Refresh("threads")) && acts.contains(&Action::Refresh("sessions")), "the rows leave: {acts:?}");
    assert!(acts.iter().any(|a| matches!(a, Action::Notice { text, error: false, .. } if text == "Thread t2 of web-app was closed")), "quiet notice: {acts:?}");
    // Not a thread this app showed (no pane, not listed): nothing, and not an event of another name.
    assert!(on_event(&Ctx { data: &data }, "thread-closed", &json!({ "sandbox": "web-app", "thread": "ghost" })).is_empty());
    assert!(on_event(&Ctx { data: &data }, "thread-closed", &json!({ "sandbox": "other", "thread": "t2" })).is_empty());
    assert!(on_event(&Ctx { data: &data }, "thread-closed", &json!({ "sandbox": "web-app" })).is_empty());
    assert!(on_event(&Ctx { data: &data }, "thread-closed", &json!({ "sandbox": "web-app", "thread": "../x" })).is_empty());
    assert!(on_event(&Ctx { data: &data }, "plan", &ev).is_empty());
    // An event only drops views and reads lists: it never asks the daemon to close anything,
    // so a client never closes a thread because another one closed a pane.
    assert!(acts.iter().all(|a| !matches!(a, Action::Call(_) | Action::Confirm(_) | Action::Prepare(_))), "{acts:?}");
}

#[test]
fn the_close_this_app_asked_for_hears_the_event_without_a_notice_and_its_answer_still_closes_once() {
    let data = fixture("basic");
    let p = prepare(run(&data, "thread_close", json!({ "sandbox": "web-app", "thread": "t1" })).unwrap());
    let st = json!({ "ok": true, "status": { "worktree": "/w/t1", "branch": "sbxw/t1", "base": "main", "has_branch": true, "dirty": 0, "ahead": 0 } });
    (p.build)(&Ctx { data: &data }, &p.asked, &st).unwrap();
    // The daemon's event comes before the answer: panes go, no "closed elsewhere".
    let acts = on_event(&Ctx { data: &data }, "thread-closed", &json!({ "sandbox": "web-app", "thread": "t1" }));
    assert!(acts.contains(&Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": "web-app", "thread": "t1" }) }));
    assert!(!acts.iter().any(|a| matches!(a, Action::Notice { .. })), "{acts:?}");
    // A second event of that close is not ours any more, and nobody showed it: ignored.
    let mut d = data.clone();
    d["threads"]["web-app"]["threads"] = json!([]);
    d["sessions"] = json!([]);
    assert!(on_event(&Ctx { data: &d }, "thread-closed", &json!({ "sandbox": "web-app", "thread": "t1" })).is_empty());
}
