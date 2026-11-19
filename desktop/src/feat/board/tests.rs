//! The Board: the team filter and its preselection, the conversation's
//! sides and kinds, commits folded, notes pinned, a commit opened with its
//! diff, filters and search, agent text that makes no control, one event
//! appended without the rest drawn again, and nothing outside the overlay.

use super::*;
use crate::shell::{view, App, Args};
use rune::Runtime;

fn fx() -> J {
    serde_json::from_str(include_str!("../../../fixtures/board/basic.json")).unwrap()
}

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/board/basic.json");

fn data() -> J {
    let f = fx();
    let mut d = f["daemon"].clone();
    d["app"] = json!({ "groups": { "web-app": "#e06c75", "docs": "#61afef" }, "overlay": { "screen": "board", "w": 1000 } });
    // The screenshot web-page asked for still waits in the Requests panel.
    d["relay"] = json!({ "requests": [{ "id": "r-13", "from": "web-app", "from_thread": "web-page", "kind": "screenshot", "state": "pending" }] });
    d
}

fn asked(name: &'static str, args: J) -> Asked {
    crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap_or_else(|e| panic!("{name}: {e}"));
    Asked { name, args: args.as_object().cloned().unwrap_or_default() }
}

fn ui(out: &[Action]) -> J {
    out.iter().rev().find_map(|a| match a {
        Action::Set { key, value } if key == UI => Some(value.clone()),
        _ => None,
    }).unwrap_or(J::Null)
}

/// Runs the actions as the core would: each call answered by the stand-in
/// (the fixture), its answer handed back. The last view.
fn drive(st: &mut State, d: &mut J, mut out: Vec<Action>) -> (J, Vec<String>) {
    let mut last = ui(&out);
    let mut paths = vec![];
    while let Some(i) = out.iter().position(|a| matches!(a, Action::Call(_))) {
        let Action::Call(c) = out.remove(i) else { unreachable!() };
        paths.push(c.path.clone());
        let r = crate::feat::answer_from(d, "GET", &c.path, &J::Null, &mut |_, _| {});
        let more = st.on_answer(&Ctx { data: d }, &c, Ok(r));
        let v = ui(&more);
        if !v.is_null() {
            last = v;
        }
        out.extend(more);
    }
    (last, paths)
}

fn show(st: &mut State, d: &mut J, args: J) -> J {
    let out = st.act(&Ctx { data: d }, &asked("board_show", args)).unwrap();
    assert!(out.iter().any(|a| matches!(a, Action::OpenPanel(s) if s == SCREEN)));
    drive(st, d, out).0
}

fn run(st: &mut State, d: &mut J, name: &'static str, args: J) -> (J, Vec<String>) {
    let out = st.act(&Ctx { data: d }, &asked(name, args)).unwrap();
    drive(st, d, out)
}

#[test]
fn opened_from_a_sandbox_it_shows_that_sandbox_s_team_and_the_picker_lists_every_team() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "sandbox": "web-app" }));
    assert_eq!(v["team"], "p-run");
    let ids: Vec<&str> = v["teams"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["p-run", "p-new", "p-old"], "most recent first, as the daemon lists them");
    assert_eq!(v["teams"][0]["color"], "#e06c75", "the sandbox's colour");
    assert_eq!(v["teams"][0]["state"], "running");
    // A merged team's sandbox: its latest plan.
    let v = show(&mut st, &mut d, json!({ "sandbox": "docs" }));
    assert_eq!(v["team"], "p-old");
    assert_eq!(v["side"]["state"], "merged");
    // Every team: each row says whose it is.
    let (v, paths) = run(&mut st, &mut d, "board_team", json!({ "id": "all" }));
    assert!(v["all"].as_bool().unwrap());
    assert_eq!(paths.len(), 3, "one read per team: {paths:?}");
    assert!(v["side"].is_null());
    let teams: BTreeSet<&str> = v["rows"].as_array().unwrap().iter().filter_map(|r| r["team"].as_str()).collect();
    assert_eq!(teams.len(), 3);
    // The rail's entry and a row's: both open it.
    let ex = extras(&Ctx { data: &d });
    assert!(ex.iter().any(|e| matches!(e.place, Place::Rail { zone: Zone::Activity, .. }) && e.icon.as_deref() == Some("board")));
    assert!(ex.iter().any(|e| e.place == Place::Sandbox("web-app".into()) && e.intent.as_ref().unwrap().1["sandbox"] == "web-app"));
}

#[test]
fn the_conversation_puts_each_act_on_its_side_with_its_kind() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    let rows = v["rows"].as_array().unwrap();
    let find = |kind: &str| rows.iter().find(|r| r["kind"] == kind).unwrap_or_else(|| panic!("no {kind}")).clone();
    let plan = find("plan_proposed");
    assert_eq!((plan["type"].as_str(), plan["side"].as_str(), plan["is_lead"].as_bool()), (Some("card"), Some("left"), Some(true)));
    assert_eq!(plan["threads"].as_array().unwrap().len(), 3);
    assert_eq!(find("plan_approved")["side"], "right");
    let failed = find("thread_failed");
    assert_eq!((failed["type"].as_str(), failed["side"].as_str()), (Some("system"), Some("center")));
    let warn = find("board_warning");
    assert_eq!((warn["type"].as_str(), warn["label"].as_str(), warn["tone"].as_str()), (Some("note"), Some("Warning"), Some("amber")));
    assert_eq!(find("board_decision")["tone"], "green");
    let answer = find("user_answer");
    assert_eq!(answer["side"], "right");
    assert_eq!(answer["questions"][1]["answer"], "50, people scroll anyway.");
    let status = find("thread_status");
    assert_eq!(status["type"], "status");
    assert!(status["state"]["word"].is_string());
    // The screenshot still waits: a link to the Requests panel, not an answer here.
    let shot = find("screenshot_asked");
    assert_eq!(shot["link"]["what"], "requests");
    // Days: oldest first, a separator before the first message.
    assert_eq!(rows[0]["type"], "day");
    // Commits: api-routes' four in one row, folded; web-page's two; docs-update's one, open.
    let groups: Vec<(String, u64, bool)> = rows.iter().filter(|r| r["type"] == "commits").map(|r| (r["who"].as_str().unwrap().into(), r["count"].as_u64().unwrap(), r["open"].as_bool().unwrap())).collect();
    assert_eq!(groups, [("api-routes".into(), 4, false), ("web-page".into(), 2, false), ("docs-update".into(), 1, true)]);
    let key = rows.iter().find(|r| r["type"] == "commits").unwrap()["key"].as_str().unwrap().to_string();
    let (v, _) = run(&mut st, &mut d, "board_expand", json!({ "id": key }));
    assert!(v["rows"].as_array().unwrap().iter().any(|r| r["key"] == key.as_str() && r["open"] == true));
}

#[test]
fn notes_stand_pinned_and_a_commit_opens_with_its_diff() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    let notes = v["side"]["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0]["label"].as_str(), notes[0]["tone"].as_str()), (Some("Decision"), Some("green")));
    assert_eq!(notes[1]["tone"], "amber");
    let per = v["side"]["commits"].as_array().unwrap();
    let api = per.iter().find(|p| p["thread"] == "api-routes").unwrap();
    assert_eq!(api["totals"]["commits"], 4);
    assert_eq!(v["side"]["lanes"]["wide"]["labels"].as_array().unwrap().len(), 4, "main and three lanes");
    let hash = api["commits"][0]["hash"].as_str().unwrap().to_string();
    let (v, paths) = run(&mut st, &mut d, "board_commit", json!({ "hash": hash }));
    assert_eq!(paths, [format!("/api/sandboxes/web-app/commits/{hash}")], "the Commits view's route, on the team's sandbox");
    let open = v["side"]["commits"].as_array().unwrap().iter().flat_map(|p| p["commits"].as_array().unwrap().clone()).find(|c| c["open"] == true).unwrap();
    let file = open["detail"]["files"][0]["path"].as_str().unwrap().to_string();
    let (v, _) = run(&mut st, &mut d, "board_file", json!({ "path": file }));
    let open = v["side"]["commits"].as_array().unwrap().iter().flat_map(|p| p["commits"].as_array().unwrap().clone()).find(|c| c["open"] == true).unwrap();
    let groups = open["detail"]["files"][0]["groups"].as_array().unwrap();
    assert!(groups.iter().any(|g| g["kind"] == "add") && groups.iter().any(|g| g["kind"] == "del"), "{groups:?}");
    // Only the team's commits and the open commit's files.
    assert!(st.act(&Ctx { data: &d }, &asked("board_commit", json!({ "hash": "0123456789abcdef0123456789abcdef01234567" }))).is_err());
    assert!(st.act(&Ctx { data: &d }, &asked("board_file", json!({ "path": "/etc/passwd" }))).is_err());
}

#[test]
fn filters_and_search_ask_the_daemon_and_the_rows_follow() {
    let mut d = data();
    let mut st = State::default();
    show(&mut st, &mut d, json!({ "team": "p-run" }));
    let (v, paths) = run(&mut st, &mut d, "board_filter", json!({ "group": "notes" }));
    assert!(paths[0].contains("group=notes"), "{paths:?}");
    let kinds: BTreeSet<&str> = v["rows"].as_array().unwrap().iter().filter(|r| r["type"] != "day").filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(kinds, ["board_decision", "board_warning"].into_iter().collect());
    let (v, paths) = run(&mut st, &mut d, "board_filter", json!({ "group": "all", "thread": "web-page" }));
    assert!(paths[0].contains("thread=web-page") && paths[0].contains("group=conversation"), "{paths:?}");
    assert!(v["rows"].as_array().unwrap().iter().filter(|r| r["type"] != "day").all(|r| r["who"] == "web-page" || r["to"] == "web-page" || r["kind"] == "thread_status"));
    let (v, paths) = run(&mut st, &mut d, "board_filter", json!({ "thread": "", "text": "cents" }));
    assert!(paths[0].contains("q=cents"));
    assert_eq!(v["rows"].as_array().unwrap().iter().filter(|r| r["type"] != "day").count(), 3, "the warning, the status and the commit that mention it");
    assert!(st.act(&Ctx { data: &d }, &asked("board_filter", json!({ "thread": "not-a-thread" }))).is_err());
}

#[test]
fn links_go_where_one_acts_and_never_act_here() {
    let mut d = data();
    let mut st = State::default();
    show(&mut st, &mut d, json!({ "team": "p-run" }));
    let go = |st: &mut State, what: &str, thread: Option<&str>| -> Vec<(String, J)> {
        let mut args = json!({ "what": what });
        if let Some(t) = thread {
            args["thread"] = t.into();
        }
        st.act(&Ctx { data: &d }, &asked("board_open", args)).unwrap().into_iter().filter_map(|a| match a {
            Action::Intent { name, args } => Some((name, args)),
            _ => None,
        }).collect()
    };
    assert_eq!(go(&mut st, "terminal", Some("web-page")), [("open_terminal".to_string(), json!({ "sandbox": "web-app", "thread": "web-page", "pane": "spare" }))]);
    assert_eq!(go(&mut st, "plan", None)[0].0, "plan_review");
    assert_eq!(go(&mut st, "requests", None)[0].0, "relay_open");
    assert_eq!(go(&mut st, "commits", Some("api-routes"))[0].1["branch"], "sbxw/api-routes");
    // Every intent of the Board is Direct and asks nothing of the daemon but reads.
    for s in INTENTS {
        assert!(matches!(s.risk, Risk::Direct), "{}", s.name);
    }
}

#[test]
fn a_live_event_reads_the_newest_page_and_appends() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    let before = v["total"].as_u64().unwrap();
    // The daemon's timeline grows by one note.
    let ev = json!({ "id": "l999", "at": 1_795_100_000_000u64, "kind": "board_handoff", "group": "notes", "from": "api-routes", "to": "web-page", "text": "The totals helper is in api/src/money.rs.", "data": { "files": [] } });
    d["board_timelines"]["p-run"]["events"].as_array_mut().unwrap().insert(0, ev);
    let out = st.on_event(&Ctx { data: &d }, "team", &json!({ "plan": "p-run" }));
    let (v, paths) = drive(&mut st, &mut d, out);
    assert_eq!(paths.len(), 1);
    // The note, and a day separator before it (another day).
    assert!(v["total"].as_u64().unwrap() >= before + 1);
    assert_eq!(v["rows"].as_array().unwrap().last().unwrap()["id"], "l999", "newest at the bottom");
    // Another team's event: the list only. Closed: nothing at all.
    assert!(st.on_event(&Ctx { data: &d }, "team", &json!({ "plan": "p-old" })).iter().all(|a| matches!(a, Action::Call(c) if c.path == "/api/teams")));
    d["app"]["overlay"] = J::Null;
    assert!(st.on_event(&Ctx { data: &d }, "team", &json!({ "plan": "p-run" })).is_empty());
}

/// A team of 8 threads and 500 events, built here (no file).
fn big(d: &mut J) {
    let threads: Vec<J> = (1..=8).map(|i| json!({ "name": format!("t{i}"), "state": "working" })).collect();
    let mut events = vec![];
    for i in 0..500u64 {
        let t = format!("t{}", i % 8 + 1);
        let at = 1_795_000_000_000u64 + i * 60_000;
        let e = match i % 5 {
            0 => json!({ "id": format!("l{i}"), "at": at, "kind": "thread_status", "group": "status", "from": t, "to": "lead", "text": format!("Step {i} of the work, going well."), "data": { "thread": t, "state": "working" } }),
            1 => json!({ "id": format!("b{i}"), "at": at, "kind": "board_warning", "group": "notes", "from": t, "text": format!("Note {i}: check the shared helper before you change it."), "data": { "files": [] } }),
            _ => json!({ "id": format!("c{i:040}"), "at": at, "kind": "commit", "group": "commits", "from": t, "text": format!("commit {i}"), "data": { "hash": format!("{i:040}"), "short": format!("{i:07}"), "added": 3, "removed": 1, "files": 1, "thread": t } }),
        };
        events.push(e);
    }
    events.reverse();
    let team = json!({ "id": "p-big", "sandbox": "web-app", "summary": "Eight threads at once", "state": "running", "variant": "split", "threads": threads });
    d["board_teams"].as_array_mut().unwrap().insert(0, team.clone());
    d["board_timelines"]["p-big"] = json!({ "team": team, "events": events, "standing": [], "commits": { "available": true, "list": [], "totals": [] } });
}

#[test]
fn a_big_team_draws_a_page_of_rows_and_shows_earlier_on_demand() {
    let mut d = data();
    big(&mut d);
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-big" }));
    assert!(v["rows"].as_array().unwrap().len() <= model::ROWS_PAGE);
    assert_eq!(v["older"], true);
    let (v, paths) = run(&mut st, &mut d, "board_more", json!({}));
    assert!(v["rows"].as_array().unwrap().len() > model::ROWS_PAGE);
    let _ = paths;
}

// ── In the whole window ───────────────────────────────────────────────────

fn window(w: f32, h: f32) -> Runtime<App> {
    let a = App::new(Args { fixture: Some(FIXTURE.into()), ..Args::default() });
    let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
    }
}

fn open(rt: &mut Runtime<App>, args: J) {
    let v = json!({ "kind": "intent", "name": "board_show", "args": args });
    for i in crate::intent::parse(&v).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(rt);
}

fn keys(n: &crate::luau::Node, out: &mut Vec<String>) {
    if let Some(k) = crate::screen::key_of(n) {
        out.push(k);
    }
    n.children.iter().for_each(|c| keys(c, out));
}

#[test]
fn nothing_of_the_board_is_drawn_outside_its_overlay_at_any_width() {
    let mut bad = vec![];
    let mut checked = 0;
    for (w, h) in [(1284.0, 816.0), (760.0, 600.0), (500.0, 600.0), (420.0, 700.0)] {
        for tab in ["conversation", "notes", "commits"] {
            let mut rt = window(w, h);
            // A narrow window is used with the sidebar hidden (at 500 pt it
            // would leave the pane area 224 pt).
            if w < 700.0 {
                rt.update(|s, _| s.toggle_sidebar());
                settle(&mut rt);
            }
            open(&mut rt, json!({ "team": "p-run" }));
            let v = json!({ "kind": "intent", "name": "board_tab", "args": { "tab": tab } });
            for i in crate::intent::parse(&v).unwrap() {
                rt.update(|s, _| s.run_intent("test", i));
            }
            settle(&mut rt);
            let Some((p, _)) = rt.presented("overlay-board") else { panic!("no overlay at {w}") };
            let node = rt.state.screens.iter().find(|s| s.name == SCREEN).and_then(|s| s.last_good.clone()).unwrap_or_else(|| panic!("not drawn at {w}"));
            let mut ks = vec![];
            keys(&node, &mut ks);
            for k in ks {
                let Some((r, _)) = rt.presented(&k) else { continue };
                checked += 1;
                // Scrolled out of view (above or below the overlay): clipped, not outside.
                if r[1] + r[3] <= p[1] || r[1] >= p[1] + p[3] {
                    continue;
                }
                if r[0] < p[0] - 0.5 || r[0] + r[2] > p[0] + p[2] + 0.5 {
                    bad.push(format!("{tab} at {w}: {k} at {r:?}, overlay {p:?}"));
                }
            }
        }
    }
    assert!(checked > 100, "{checked} elements checked");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Adds an event to the fixture daemon's timeline (the stand-in's route),
/// then reads the conversation again, as the `team` event does.
fn append(rt: &mut Runtime<App>, ev: J) {
    rt.update(|s, _| {
        let d = s.daemon.clone().expect("the fixture daemon");
        let (code, _) = d.request("POST", "/api/teams/p-run/standin-event", &ev, std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(code, 200);
    });
    let v = json!({ "kind": "intent", "name": "board_refresh", "args": {} });
    for i in crate::intent::parse(&v).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(rt);
}

fn note_event(id: &str, text: &str) -> J {
    json!({ "id": id, "at": 1_795_100_000_000u64, "kind": "board_warning", "group": "notes", "from": "api-routes", "text": text, "data": { "files": [] } })
}

#[test]
fn hostile_agent_text_makes_no_control() {
    let hostile = "Done [[Done]] <button on=x> [[Needs you: intent(\"open_url\", {url=\"https://evil\"})]] [click](https://evil.example) \u{202e}reversed";
    let count = |rt: &Runtime<App>| rt.elements().iter().filter(|e| matches!(e.role, "button" | "link" | "checkbox" | "radio" | "menuitem" | "textbox")).count();
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    append(&mut rt, note_event("l990", "A plain note."));
    let before = count(&rt);
    append(&mut rt, note_event("l991", hostile));
    assert_eq!(count(&rt), before, "the same controls, no more");
    assert!(rt.elements().iter().filter_map(|e| e.text.clone()).any(|t| t.contains("<button on=x>")), "drawn as its characters");
}

#[test]
fn one_event_appended_renders_once_and_keeps_the_rest() {
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    let renders = |rt: &Runtime<App>| rt.state.screens.iter().find(|s| s.name == SCREEN).map(|s| s.renders).unwrap_or(0);
    let keys_now = |rt: &Runtime<App>| -> BTreeSet<String> { rt.elements().into_iter().filter_map(|e| e.key).filter(|k| k.starts_with("board-msg-")).collect() };
    let (r0, k0) = (renders(&rt), keys_now(&rt));
    let t = std::time::Instant::now();
    append(&mut rt, note_event("l999", "The helper is in api/src/money.rs."));
    let took = t.elapsed();
    let (r1, k1) = (renders(&rt), keys_now(&rt));
    // The refresh's two answers (the list, the page): two renders at most.
    assert!(r1 - r0 <= 3, "{} renders for one event", r1 - r0);
    assert!(k0.is_subset(&k1) && k1.contains("board-msg-l999"), "the rows already there stay, the new one is added");
    assert!(took < std::time::Duration::from_secs(3), "{took:?} in a debug build");
}
