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
    assert_eq!(ids, ["p-run", "p-new", "p-old", "p-long"], "as the daemon lists them");
    assert_eq!(v["teams"][0]["color"], "#e06c75", "the sandbox's colour");
    assert_eq!(v["teams"][0]["state"], "running");
    // A merged team's sandbox: its latest plan.
    let v = show(&mut st, &mut d, json!({ "sandbox": "docs" }));
    assert_eq!(v["team"], "p-old");
    assert_eq!(v["side"]["state"], "merged");
    // Every team: each row says whose it is.
    let (v, paths) = run(&mut st, &mut d, "board_team", json!({ "id": "all" }));
    assert!(v["all"].as_bool().unwrap());
    assert_eq!(paths.len(), 4, "one read per team: {paths:?}");
    assert!(v["side"].is_null());
    let teams: BTreeSet<&str> = v["rows"].as_array().unwrap().iter().filter_map(|r| r["team"].as_str()).collect();
    assert_eq!(teams.len(), 4);
    // Each row names its team by its threads, the summary in the tooltip: never a cut sentence.
    let r = v["rows"].as_array().unwrap().iter().find(|r| r["team"] == "p-run").unwrap();
    assert_eq!(r["team_label"], "api-routes + web-page + docs-update");
    assert_eq!(r["team_help"], "Orders v2: the API routes, the web page that uses them, and their docs");
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
    let dm = find("direct_message");
    assert_eq!((dm["type"].as_str(), dm["label"].as_str(), dm["side"].as_str(), dm["is_lead"].as_bool()), (Some("message"), Some("direct message"), Some("left"), Some(true)));
    let note = find("team_note");
    assert_eq!((note["type"].as_str(), note["label"].as_str(), note["tone"].as_str(), note["note_via"].as_str()), (Some("note"), Some("Info"), Some("blue"), Some("team note")));
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
    assert_eq!(notes.len(), 3, "the board's decision and warning, and the lead's decision sent as a team note");
    assert_eq!((notes[0]["label"].as_str(), notes[0]["tone"].as_str()), (Some("Decision"), Some("green")));
    assert_eq!(notes[2]["tone"], "amber");
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
    assert_eq!(kinds, ["board_decision", "board_warning", "team_note"].into_iter().collect());
    let (v, paths) = run(&mut st, &mut d, "board_filter", json!({ "group": "all", "thread": "web-page" }));
    assert!(paths[0].contains("thread=web-page") && paths[0].contains("group=conversation"), "{paths:?}");
    assert!(v["rows"].as_array().unwrap().iter().filter(|r| r["type"] != "day").all(|r| r["who"] == "web-page" || r["to"] == "web-page" || r["kind"] == "thread_status"));
    let (v, paths) = run(&mut st, &mut d, "board_filter", json!({ "thread": "", "text": "cents" }));
    assert!(paths[0].contains("q=cents"));
    assert_eq!(v["rows"].as_array().unwrap().iter().filter(|r| r["type"] != "day").count(), 4, "the warning, the two reports and the commit that mention it");
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
    for (w, h) in [(2000.0, 1000.0), (1284.0, 816.0), (760.0, 600.0), (500.0, 600.0), (420.0, 700.0)] {
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
    let mut dm = note_event("l992", hostile);
    dm["kind"] = "direct_message".into();
    dm["group"] = "messages".into();
    dm["to"] = "web-page".into();
    append(&mut rt, dm);
    // A report (its bubble tinted) and a bare change of state with only spaces.
    let mut st = note_event("l993", hostile);
    st["kind"] = "thread_status".into();
    st["to"] = "lead".into();
    st["data"] = json!({ "thread": "api-routes", "state": "done" });
    append(&mut rt, st.clone());
    st["id"] = "l994".into();
    st["text"] = "   ".into();
    append(&mut rt, st);
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

// ── The polish after the first Mac test ──────────────────────────────────

#[test]
fn the_picker_names_a_team_by_its_threads_with_its_summary_state_and_age() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    let t = &v["teams"][0];
    assert_eq!(t["names"], "api-routes + web-page + docs-update", "thread names first");
    assert_eq!(t["summary"], "Orders v2: the API routes, the web page that uses them, and their docs", "the summary whole (the screen wraps it on two lines)");
    assert_eq!((t["state"].as_str(), t["tone"].as_str()), (Some("running"), Some("blue")));
    assert!(t["when"].as_str().unwrap().ends_with("ago") || t["when"] == "just now", "{}", t["when"]);
    assert_eq!(v["side"]["names"], "api-routes + web-page + docs-update", "the closed picker's too");
    // A team with no thread: its sandbox. Past three: "+ n more".
    assert_eq!(model::team_names(&json!({ "sandbox": "api", "threads": [] })), "api");
    assert_eq!(model::team_names(&json!({ "threads": [{ "name": "a" }, { "name": "b" }, { "name": "c" }, { "name": "d" }] })), "a + b + 2 more");
    // Closed until asked; the popup flag follows it.
    assert_eq!((v["picker"]["open"].as_bool(), v["popup"].as_bool()), (Some(false), Some(false)));
}

#[test]
fn the_picker_opens_moves_picks_and_closes_by_key_press_outside_and_reopen() {
    let mut d = data();
    let mut st = State::default();
    show(&mut st, &mut d, json!({ "team": "p-run" }));
    let (v, _) = run(&mut st, &mut d, "board_picker", json!({}));
    assert_eq!((v["popup"].as_bool(), v["picker"]["hl"].as_u64()), (Some(true), Some(1)), "open, the team shown marked (0 is All teams)");
    let (v, _) = run(&mut st, &mut d, "board_key", json!({ "key": "down" }));
    assert_eq!(v["picker"]["hl"], 2);
    let (v, _) = run(&mut st, &mut d, "board_key", json!({ "key": "up" }));
    let (v2, _) = run(&mut st, &mut d, "board_key", json!({ "key": "up" }));
    assert_eq!((v["picker"]["hl"].as_u64(), v2["picker"]["hl"].as_u64()), (Some(1), Some(0)));
    let (v, _) = run(&mut st, &mut d, "board_key", json!({ "key": "up" }));
    assert_eq!(v["picker"]["hl"], 0, "stays on the first row");
    // Escape closes the list only.
    let (v, _) = run(&mut st, &mut d, "board_key", json!({ "key": "escape" }));
    assert_eq!((v["popup"].as_bool(), v["team"].as_str()), (Some(false), Some("p-run")));
    // Enter picks the marked row.
    run(&mut st, &mut d, "board_picker", json!({ "open": true }));
    run(&mut st, &mut d, "board_key", json!({ "key": "down" }));
    let (v, _) = run(&mut st, &mut d, "board_key", json!({ "key": "enter" }));
    assert_eq!((v["team"].as_str(), v["popup"].as_bool()), (Some("p-new"), Some(false)));
    // A press outside (the dismiss layer's intent), a pick with the pointer.
    run(&mut st, &mut d, "board_picker", json!({}));
    let (v, _) = run(&mut st, &mut d, "board_picker", json!({ "open": false }));
    assert_eq!(v["popup"], false);
    run(&mut st, &mut d, "board_picker", json!({}));
    let (v, _) = run(&mut st, &mut d, "board_team", json!({ "id": "p-old" }));
    assert_eq!((v["team"].as_str(), v["popup"].as_bool()), (Some("p-old"), Some(false)));
    // Closed with the overlay, it opens closed again.
    run(&mut st, &mut d, "board_picker", json!({}));
    let v = show(&mut st, &mut d, json!({}));
    assert_eq!(v["popup"], false);
    // Keys while closed do nothing; a team sbxw does not list is refused.
    assert!(st.act(&Ctx { data: &d }, &asked("board_key", json!({ "key": "enter" }))).unwrap().is_empty());
    assert!(st.act(&Ctx { data: &d }, &asked("board_team", json!({ "id": "p-nope" }))).is_err());
}

#[test]
fn many_teams_page_in_the_picker() {
    let mut d = data();
    for i in 0..45 {
        d["board_teams"].as_array_mut().unwrap().push(json!({ "id": format!("p-x{i}"), "sandbox": "api", "summary": format!("Team {i}"), "state": "done", "threads": [{ "name": format!("t{i}") }] }));
    }
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    assert_eq!(v["teams"].as_array().unwrap().len(), PICKER_PAGE);
    assert_eq!(v["picker"]["more"], 49 - PICKER_PAGE);
    let (v, _) = run(&mut st, &mut d, "board_picker", json!({ "more": true }));
    assert_eq!(v["teams"].as_array().unwrap().len(), 2 * PICKER_PAGE);
}

fn rows_of(v: &J) -> Vec<J> {
    v["rows"].as_array().unwrap().clone()
}

#[test]
fn the_author_line_names_once_and_groups_one_author_s_messages() {
    let mut d = data();
    let mut st = State::default();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    let rows = rows_of(&v);
    // The lead named otherwise: its name and the role once.
    let dms: Vec<&J> = rows.iter().filter(|r| r["kind"] == "direct_message" && r["is_lead"] == true && r["to"] == "web-page").collect();
    assert_eq!(dms.len(), 2);
    assert_eq!((dms[0]["who"].as_str(), dms[0]["role"].as_str(), dms[0]["to"].as_str()), (Some("orders-planner"), Some("lead"), Some("web-page")));
    // Two messages in a row, same author, same addressee, a minute apart: one author line.
    assert!(dms[0]["cont"].is_null() && dms[1]["cont"] == true, "{:?}", dms.iter().map(|r| &r["cont"]).collect::<Vec<_>>());
    // The reply goes to the lead by its name; the team note to the whole team.
    let reply = rows.iter().find(|r| r["kind"] == "direct_message" && r["who"] == "web-page").unwrap();
    assert_eq!((reply["to"].as_str(), reply["cont"].is_null()), (Some("orders-planner"), true));
    let to_team = rows.iter().find(|r| r["kind"] == "team_note" && r["to"] == "the team").unwrap();
    assert_eq!(to_team["label"], "Decision");
    // No "lead lead": a lead without a name is "lead" with no role chip; one named "... lead" neither.
    assert_eq!(model::author("lead", None), ("lead".into(), "purple", None));
    assert_eq!(model::author("lead", Some("Orders lead")).2, None);
    assert_eq!(model::author("lead", Some("  ")).0, "lead");
    let v = show(&mut st, &mut d, json!({ "team": "p-old" }));
    for r in rows_of(&v).iter().filter(|r| r["is_lead"] == true) {
        assert_eq!((r["who"].as_str(), r["role"].is_null()), (Some("lead"), true));
    }
    // Not grouped: another addressee, another side, more than five minutes, a day apart.
    let ev = |id: &str, at: u64, from: &str, to: &str| json!({ "id": id, "at": at, "kind": "direct_message", "from": from, "to": to, "text": "x", "data": {} });
    let o = model::Opts::default();
    let t = 1_795_000_000_000u64;
    let r = model::rows(&[ev("a", t, "t1", "lead"), ev("b", t + 60_000, "t1", "lead"), ev("c", t + 120_000, "t1", "t2"), ev("d", t + 900_000, "t1", "t2")], &o);
    let cont: Vec<bool> = r.iter().filter(|x| x["type"] != "day").map(|x| x["cont"] == true).collect();
    assert_eq!(cont, [false, true, false, false]);
}

#[test]
fn a_report_is_tinted_by_its_state_and_a_bare_change_of_state_is_one_line() {
    let ev = |id: &str, min: u64, from: &str, state: &str, text: Option<&str>| json!({ "id": id, "at": 1_795_000_000_000u64 + min * 60_000, "kind": "thread_status", "from": from, "to": "lead", "text": text, "data": { "thread": from, "state": state } });
    let o = model::Opts::default();
    let r = model::rows(
        &[
            ev("a", 0, "t1", "done", Some("All in.")),
            ev("b", 1, "t1", "blocked", Some("Waiting on a key.")),
            ev("c", 2, "t1", "failed", Some("The build broke.")),
            ev("d", 3, "t1", "working", Some("Going on.")),
            ev("e", 4, "t2", "working", None),
            ev("f", 5, "t2", "blocked", Some("   ")),
            ev("g", 6, "t2", "working", Some("")),
            ev("h", 7, "t1", "starting", None),
        ],
        &o,
    );
    let r: Vec<&J> = r.iter().filter(|x| x["type"] != "day").collect();
    let tints: Vec<&str> = r[..4].iter().map(|x| x["tint"].as_str().unwrap()).collect();
    assert_eq!(tints, ["green", "amber", "red", ""]);
    assert!(r[..4].iter().all(|x| x["type"] == "status" && x["state"]["word"].is_string()), "the state chip stays");
    // t2's three bare changes: one line, the last state, a count and the trail.
    assert_eq!((r[4]["type"].as_str(), r[4]["count"].as_u64(), r[4]["state"]["word"].as_str()), (Some("transition"), Some(3), Some("working")));
    assert_eq!(r[4]["trail"], "working → blocked → working");
    assert_eq!((r[5]["type"].as_str(), r[5]["count"].as_u64()), (Some("transition"), Some(1)), "another agent's: its own line");
    assert_eq!(r.len(), 6);
}

/// Every element drawn inside a bubble stays inside it, and nothing leaves
/// the overlay, on the team with long words and on the team at work.
#[test]
fn no_text_leaves_its_bubble_or_the_overlay_with_long_unbroken_words() {
    let long = format!("{} {} {}", "y".repeat(200), "src/".repeat(30) + "file.rs", "0123456789abcdef".repeat(5));
    let mut bad = vec![];
    let mut inside = 0;
    for (w, h) in [(2000.0, 1000.0), (1284.0, 816.0), (760.0, 600.0), (500.0, 600.0), (420.0, 700.0)] {
        for team in ["p-long", "p-run"] {
            let mut rt = window(w, h);
            if w < 700.0 {
                rt.update(|s, _| s.toggle_sidebar());
                settle(&mut rt);
            }
            open(&mut rt, json!({ "team": team }));
            if team == "p-run" {
                append(&mut rt, note_event("l995", &long));
                let mut st = note_event("l996", &long);
                st["kind"] = "thread_status".into();
                st["group"] = "status".into();
                st["to"] = "lead".into();
                st["data"] = json!({ "thread": "api-routes", "state": "done" });
                append(&mut rt, st);
            }
            let Some((p, _)) = rt.presented("overlay-board") else { panic!("no overlay at {w}") };
            let els = rt.elements();
            for e in els.iter() {
                let r = e.rect;
                if r[2] <= 0.0 || r[3] <= 0.0 || e.visible[2] <= 0.0 || e.visible[3] <= 0.0 {
                    continue;
                }
                // Its bubble, if it is in one.
                let mut up = e.parent;
                let mut bubble = None;
                while let Some(j) = up {
                    if els[j].key.as_deref().is_some_and(|k| k.starts_with("board-bubble-")) {
                        bubble = Some(j);
                        break;
                    }
                    up = els[j].parent;
                }
                let in_overlay = e.path.contains("overlay") || bubble.is_some() || e.key.as_deref().is_some_and(|k| k.starts_with("board-"));
                if let Some(b) = bubble {
                    // A text's box is laid out to its parent; its ink is not. Its
                    // widest unbreakable run (in the prose face and in mono, the
                    // wider) must fit the box it was given (8 pt of slack: a chip's
                    // word is measured here at 13 pt, drawn at 12).
                    if let Some(t) = e.text.as_deref().filter(|_| e.role == "text") {
                        let st = crate::screen::body(13.0, 400.0);
                        let run = rune::text::content_widths(t, &[], st).0.max(rune::text::content_widths(t, &[], st.mono()).0);
                        if run > r[2] + 8.0 {
                            bad.push(format!("{team} at {w}: a run of {run} pt in a text {} wide ({:?})", r[2], t.chars().take(30).collect::<String>()));
                        }
                    }
                    inside += 1;
                    let br = els[b].rect;
                    if r[0] < br[0] - 0.5 || r[0] + r[2] > br[0] + br[2] + 0.5 {
                        bad.push(format!("{team} at {w}: {:?} {:?} ({:?}) at {r:?} leaves {:?} at {br:?}", e.key, e.role, e.text.as_deref().map(|t| t.chars().take(30).collect::<String>()), els[b].key));
                    }
                }
                if in_overlay && (r[1] + r[3] > p[1] && r[1] < p[1] + p[3]) && (r[0] < p[0] - 0.5 || r[0] + r[2] > p[0] + p[2] + 0.5) {
                    bad.push(format!("{team} at {w}: {:?} at {r:?} leaves the overlay {p:?}", e.key));
                }
            }
        }
    }
    assert!(inside > 100, "{inside} elements in bubbles checked");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn drawn_the_author_line_sits_above_its_bubble_and_a_bare_change_has_none() {
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    let node = rt.state.screens.iter().find(|s| s.name == SCREEN).and_then(|s| s.last_good.clone()).unwrap();
    let mut ks = vec![];
    keys(&node, &mut ks);
    let ks: BTreeSet<String> = ks.into_iter().collect();
    let mut st = State::default();
    let mut d = data();
    let v = show(&mut st, &mut d, json!({ "team": "p-run" }));
    let mut seen = 0;
    for r in rows_of(&v) {
        let id = r["id"].as_str().unwrap_or("");
        match r["type"].as_str().unwrap_or("") {
            "transition" => {
                assert!(!ks.contains(&format!("board-bubble-{id}")), "a bare change of state draws no bubble");
                assert!(ks.contains(&format!("board-msg-{id}")));
            }
            "status" | "note" | "message" | "question" | "card" | "merge" => {
                assert!(ks.contains(&format!("board-bubble-{id}")), "{id}");
                let author = ks.contains(&format!("board-author-{id}"));
                assert_eq!(author, r["cont"].is_null(), "{id}: an author line unless it follows the same author's");
                if let (true, Some((a, _)), Some((b, _))) = (author, rt.presented(&format!("board-author-{id}")), rt.presented(&format!("board-bubble-{id}"))) {
                    assert!(a[1] + a[3] <= b[1] + 0.5, "{id}: the author line above the bubble");
                    seen += 1;
                }
            }
            _ => {}
        }
    }
    assert!(seen >= 3, "{seen} author lines measured");
    // Bubbles hug their content: a short one is narrower than the column.
    let short = rows_of(&v).into_iter().find(|r| r["kind"] == "direct_message" && r["who"] == "web-page").unwrap();
    let (b, _) = rt.presented(&format!("board-bubble-{}", short["id"].as_str().unwrap())).unwrap_or(([0.0; 4], 0.0));
    let (c, _) = rt.presented("board-convo").unwrap();
    assert!(b[2] > 0.0 && b[2] < c[2] * 0.8, "{b:?} in {c:?}");
}

#[test]
fn the_done_blocked_and_failed_tints_are_drawn_in_both_themes_with_readable_text() {
    use crate::theme::{color_in, contrast, Mode};
    for m in Mode::ALL {
        for (fill, border) in [("green-tint", "green"), ("amber-tint", "amber"), ("red-tint", "red-border")] {
            let (f, t) = (color_in(m, fill).unwrap(), color_in(m, "text").unwrap());
            assert!(contrast(t, f) >= 4.5, "{m:?}: text on {fill} {}", contrast(t, f));
            assert!(color_in(m, border).is_some());
        }
    }
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    let els = rt.elements();
    let green = crate::theme::hex(crate::theme::color_value("green-tint").unwrap());
    let near = |a: f32, b: f32| (a - b).abs() < 0.002;
    let done = els.iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("board-bubble-"))).filter(|e| near(e.fill.r, green.r) && near(e.fill.g, green.g) && near(e.fill.b, green.b)).count();
    assert!(done >= 2, "the done reports (and the decision) on the green tint: {done}");
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("{key} not drawn"));
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn shown_team(rt: &Runtime<App>) -> String {
    rt.state.feats.data.get(UI).and_then(|v| v["team"].as_str().map(str::to_string)).unwrap_or_default()
}

#[test]
fn drawn_the_picker_list_is_as_wide_as_its_button_or_more_and_fits_the_overlay() {
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let mut rt = window(w, h);
        open(&mut rt, json!({ "team": "p-run" }));
        tap(&mut rt, "board-row-picker");
        let (o, _) = rt.presented("overlay-board").unwrap();
        let (b, _) = rt.presented("board-row-picker").unwrap();
        let (m, _) = rt.presented("board-picker").unwrap_or_else(|| panic!("the list is not open at {w}"));
        assert!(m[2] + 0.5 >= b[2], "at {w}: the list {m:?} narrower than its button {b:?}");
        assert!(m[2] <= 560.5 && m[0] >= o[0] - 0.5 && m[0] + m[2] <= o[0] + o[2] + 0.5, "at {w}: {m:?} in {o:?}");
        if w < 1000.0 {
            assert!(m[2] >= o[2] - 64.0, "at {w}: the list takes the room there is ({m:?} in {o:?})");
        } else {
            assert!(m[2] >= 540.0, "at {w}: wide enough for two lines of summary ({m:?})");
        }
        // Each row: the thread names first, the summary under them.
        let els = rt.elements();
        let row = els.iter().position(|e| e.key.as_deref() == Some("board-row-team-p-run")).unwrap();
        let texts: Vec<(String, f32)> = els.iter().filter(|e| e.text.is_some() && {
            let mut up = e.parent;
            let mut found = false;
            while let Some(j) = up {
                if j == row { found = true; break; }
                up = els[j].parent;
            }
            found
        }).map(|e| (e.text.clone().unwrap(), e.rect[1])).collect();
        assert_eq!(texts[0].0, "api-routes + web-page + docs-update", "{texts:?}");
        let summary = texts.iter().find(|t| t.0.starts_with("Orders v2")).unwrap();
        assert!(summary.1 > texts[0].1, "the summary under the names");
        // A pick closes it and shows that team.
        tap(&mut rt, "board-row-team-p-old");
        assert!(rt.presented("board-picker").is_none());
        assert_eq!(shown_team(&rt), "p-old");
        // A press outside closes it and does nothing else.
        tap(&mut rt, "board-row-picker");
        assert!(rt.presented("board-picker").is_some());
        let (d, _) = rt.presented("board-picker-dismiss").unwrap();
        rt.pointer_down(d[0] + d[2] - 20.0, d[1] + d[3] - 20.0);
        rt.pointer_up(d[0] + d[2] - 20.0, d[1] + d[3] - 20.0);
        settle(&mut rt);
        assert!(rt.presented("board-picker").is_none(), "light dismiss at {w}");
        assert_eq!(shown_team(&rt), "p-old");
    }
}

#[test]
fn the_picker_answers_up_down_and_enter() {
    use rune::{Key, KeyEvent, Mods};
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    tap(&mut rt, "board-row-picker");
    let focused = rt.elements().iter().find(|e| e.focused).and_then(|e| e.key.clone());
    assert_eq!(focused.as_deref(), Some("board-row-team-p-run"), "the keyboard on the team shown");
    for k in [Key::Down, Key::Down, Key::Up, Key::Down, Key::Enter] {
        rt.key(KeyEvent { key: k, mods: Mods::default() });
        settle(&mut rt);
    }
    assert!(rt.presented("board-picker").is_none(), "Enter picks and closes");
    assert_eq!(shown_team(&rt), "p-old", "All teams, p-run, p-new, p-old: two down from p-run");
}

#[test]
fn escape_closes_the_picker_list_first_and_the_board_on_the_second_press() {
    use rune::{Key, KeyEvent, Mods};
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    tap(&mut rt, "board-row-picker");
    assert!(rt.presented("board-picker").is_some(), "the list is open");
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    settle(&mut rt);
    assert!(rt.presented("board-picker").is_none(), "Escape closes the list");
    assert!(rt.presented("overlay-board").is_some(), "the Board stays");
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    settle(&mut rt);
    assert!(rt.presented("overlay-board").is_none(), "a second Escape closes the Board");
}

/// The widest of the conversation's bubbles, drawn, follows the column.
#[test]
fn bubbles_are_as_wide_as_the_column_allows_and_never_leave_it() {
    let prose = "The route handlers are split from the validation layer, so each endpoint reads its input once and returns a typed error the page can show. ".repeat(12);
    for (w, h, team) in [(2000.0, 1000.0, "all"), (2000.0, 1000.0, "p-run"), (1284.0, 816.0, "p-run"), (760.0, 600.0, "p-run"), (420.0, 700.0, "p-run")] {
        let mut rt = window(w, h);
        if w < 700.0 {
            rt.update(|s, _| s.toggle_sidebar());
            settle(&mut rt);
        }
        open(&mut rt, json!({ "team": team }));
        append(&mut rt, note_event("l990", &prose));
        let (p, _) = rt.presented("overlay-board").unwrap();
        let (c, _) = rt.presented("board-convo").unwrap();
        let conv = model::conv_w(p[2], team != "all");
        let max = model::bubble_max(conv, p[2]);
        let widest = rt.elements().iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("board-bubble-"))).map(|e| e.rect[2]).fold(0.0, f32::max);
        assert!((widest - max).abs() <= 1.0, "{team} at {w}: widest bubble {widest}, rule {max} (overlay {}, column {})", p[2], c[2]);
        assert!(widest <= c[2] + 0.5, "{team} at {w}: a bubble of {widest} in a column of {}", c[2]);
        if p[2] >= 760.0 {
            assert!(max >= model::BUBBLE_MIN.min(conv - 24.0) && max <= model::BUBBLE_CAP);
        } else {
            assert!(max <= model::BUBBLE_MIN);
        }
    }
}

// ── The fold holds the page (A6) ─────────────────────────────────────────

/// Where the pressed bubble and every row above it are drawn, in the window.
fn rows_at(rt: &Runtime<App>, id: &str) -> (Option<[f32; 4]>, Vec<(String, f32)>) {
    let b = rt.presented(&format!("board-bubble-{id}")).map(|(r, _)| r);
    let (c, _) = rt.presented("board-convo").unwrap();
    let above: Vec<(String, f32)> = rt
        .elements()
        .into_iter()
        .filter_map(|e| e.key.clone().map(|k| (k, e.rect)))
        .filter(|(k, _)| k.starts_with("board-msg-") || k.starts_with("board-author-"))
        .filter(|(_, r)| b.is_some_and(|b| r[1] + r[3] <= b[1] + 0.5) && r[1] + r[3] > c[1] && r[1] < c[1] + c[3])
        .map(|(k, r)| (k, r[1]))
        .collect();
    (b, above)
}

/// Presses `key` and checks the fold rule: the bubble's top edge and every
/// row above it stay where they were (within 1 pt) on every frame, its
/// width is the same. Returns what moved.
/// Scrolls with the wheel until `key` is wholly in the conversation's view,
/// as a person reaches a control before pressing it.
fn reach(rt: &mut Runtime<App>, key: &str) {
    let (c, _) = rt.presented("board-convo").unwrap();
    for _ in 0..80 {
        let Some((r, _)) = rt.presented(key) else { return };
        let dy = if r[1] < c[1] { 30.0 } else if r[1] + r[3] > c[1] + c[3] { -30.0 } else { return };
        rt.wheel(c[0] + c[2] / 2.0, c[1] + c[3] / 2.0, dy);
        settle(rt);
    }
}

fn press_holds(rt: &mut Runtime<App>, key: &str, id: &str, folding: bool) -> Vec<String> {
    reach(rt, key);
    let (b0, above0) = rows_at(rt, id);
    let b0 = b0.unwrap_or_else(|| panic!("{id} not drawn"));
    let (c, _) = rt.presented("board-convo").unwrap();
    if folding && b0[1] < c[1] - 0.5 {
        // Its top above the view: folding brings it back into the view
        // (Action::ScrollIntoView) instead of holding it out of sight.
        tap(rt, key);
        let (b, _) = rt.presented(&format!("board-bubble-{id}")).unwrap();
        return if b[1] >= c[1] - 0.5 && b[1] < c[1] + c[3] { vec![] } else { vec![format!("{key}: folded from above the view, its top {b:?} is not in {c:?}")] };
    }
    let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("{key} not drawn"));
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    let mut bad = vec![];
    for f in 0..24 {
        rt.frame(0.025);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        let (b, above) = rows_at(rt, id);
        let Some(b) = b else { bad.push(format!("frame {f}: the bubble is gone")); continue };
        if (b[1] - b0[1]).abs() > 1.0 {
            bad.push(format!("frame {f}: {key}: the bubble's top moved {} -> {}", b0[1], b[1]));
        }
        if (b[2] - b0[2]).abs() > 1.0 {
            bad.push(format!("frame {f}: {key}: the bubble's width changed {} -> {}", b0[2], b[2]));
        }
        for (k, y0) in &above0 {
            if let Some((_, y1)) = above.iter().find(|(k1, _)| k1 == k) {
                if (y1 - y0).abs() > 1.0 {
                    bad.push(format!("frame {f}: {key}: {k} above moved {y0} -> {y1}"));
                }
            }
        }
    }
    bad.dedup();
    bad
}

fn long_note(id: &str, at: u64) -> J {
    let text = "The handler reads its input once, checks it against the schema, and returns a typed error the page can show without parsing a message. ".repeat(14);
    let mut e = note_event(id, &text);
    e["at"] = at.into();
    e
}

#[test]
fn show_more_and_show_less_keep_the_bubble_and_the_rows_above_in_place() {
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let mut rt = window(w, h);
        if w < 760.0 {
            rt.update(|s, _| s.toggle_sidebar());
            settle(&mut rt);
        }
        open(&mut rt, json!({ "team": "p-run" }));
        append(&mut rt, long_note("l980", 1_795_100_000_000));
        // At the end of a conversation longer than the window (the view
        // follows its end): the newest bubble folded.
        let mut bad = press_holds(&mut rt, "board-fold-l980", "l980", false);
        settle(&mut rt);
        bad.extend(press_holds(&mut rt, "board-fold-l980", "l980", true));
        settle(&mut rt);
        // In the middle: rows after it, the view scrolled so it shows.
        for i in 0..6 {
            append(&mut rt, note_event(&format!("l98{}", i + 1), "A short note after the long one."));
        }
        let (c, _) = rt.presented("board-convo").unwrap();
        for _ in 0..40 {
            let Some((b, _)) = rt.presented("board-fold-l980") else { break };
            if b[1] > c[1] + c[3] * 0.35 {
                break;
            }
            rt.wheel(c[0] + c[2] / 2.0, c[1] + c[3] / 2.0, 40.0);
            settle(&mut rt);
        }
        for _ in 0..40 {
            let Some((b, _)) = rt.presented("board-fold-l980") else { break };
            if b[1] < c[1] + c[3] * 0.7 {
                break;
            }
            rt.wheel(c[0] + c[2] / 2.0, c[1] + c[3] / 2.0, -40.0);
            settle(&mut rt);
        }
        bad.extend(press_holds(&mut rt, "board-fold-l980", "l980", false));
        settle(&mut rt);
        // Show less: the bubble's top is in view after. An open bubble whose
        // top is above the view comes back into it (Action::ScrollIntoView); one in
        // view holds.
        bad.extend(press_holds(&mut rt, "board-fold-l980", "l980", true));
        settle(&mut rt);
        let (b, _) = rt.presented("board-bubble-l980").unwrap();
        assert!(b[1] >= c[1] - 0.5 && b[1] < c[1] + c[3], "at {w}: after Show less the bubble's top is in view: {b:?} in {c:?}");
        assert!(bad.is_empty(), "at {w}:\n{}", bad.join("\n"));
    }
}

#[test]
fn show_less_from_the_end_of_a_tall_bubble_brings_its_top_into_view() {
    let mut rt = window(720.0, 480.0);
    rt.update(|s, _| s.toggle_sidebar());
    settle(&mut rt);
    open(&mut rt, json!({ "team": "p-run" }));
    append(&mut rt, long_note("l980", 1_795_100_000_000));
    tap(&mut rt, "board-fold-l980");
    let (c, _) = rt.presented("board-convo").unwrap();
    // Down to its end: its Show less in view, its top above the view.
    for _ in 0..60 {
        if rt.presented("board-fold-l980").is_some_and(|(f, _)| f[1] + f[3] <= c[1] + c[3] && f[1] >= c[1]) && rt.presented("board-bubble-l980").is_some_and(|(b, _)| b[1] < c[1]) {
            break;
        }
        rt.wheel(c[0] + c[2] / 2.0, c[1] + c[3] / 2.0, -30.0);
        settle(&mut rt);
    }
    let (b, _) = rt.presented("board-bubble-l980").unwrap();
    assert!(b[1] < c[1], "the open bubble's top is above the view: {b:?} in {c:?}");
    tap(&mut rt, "board-fold-l980");
    let (b, _) = rt.presented("board-bubble-l980").unwrap();
    assert!(b[1] >= c[1] - 0.5 && b[1] < c[1] + c[3], "after Show less its top is in view: {b:?} in {c:?}");
}

#[test]
fn scrolling_to_a_key_not_drawn_does_nothing() {
    let mut rt = window(1284.0, 816.0);
    open(&mut rt, json!({ "team": "p-run" }));
    let (c0, _) = rt.presented("board-convo").unwrap();
    let before: Vec<(Option<String>, [f32; 4])> = rt.elements().into_iter().map(|e| (e.key, e.rect)).collect();
    let focus0 = rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    // Opened then folded: a fold key with no bubble drawn asks to scroll to "board-bubble-nowhere".
    for _ in 0..2 {
        let v = json!({ "kind": "intent", "name": "board_expand", "args": { "id": "nowhere" } });
        for i in crate::intent::parse(&v).unwrap() {
            rt.update(|s, _| s.run_intent("test", i));
        }
        settle(&mut rt);
    }
    let after: Vec<(Option<String>, [f32; 4])> = rt.elements().into_iter().map(|e| (e.key, e.rect)).collect();
    assert_eq!(before.len(), after.len());
    for ((ka, a), (kb, b)) in before.iter().zip(&after) {
        assert!(ka == kb && (0..4).all(|i| (a[i] - b[i]).abs() < 0.5), "nothing moved: {ka:?} {a:?} -> {kb:?} {b:?}");
    }
    assert_eq!(rt.presented("board-convo").unwrap().0, c0);
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key), focus0, "the keyboard focus stays");
}
