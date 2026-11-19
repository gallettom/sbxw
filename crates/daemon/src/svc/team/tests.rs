//! A team's timeline from a scripted plan life, its paging and filters,
//! plans from before the log, commits attributed after the merge, no
//! emails, and the live event only while a UI listens.

use crate::bus::Event;
use crate::testing::{app, app_in, signed_in};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sbxw_core::board::Kind;
use sbxw_core::plans::{MergeResult, PlanThread, ThreadState, Variant};
use sbxw_core::relay::RelayKind;
use sbxw_core::{SandboxRef, ThreadName};
use serde_json::Value;
use std::path::Path;
use tower::ServiceExt;

fn t(name: &str) -> PlanThread {
    PlanThread {
        name: name.into(),
        goal: format!("Build the {name} part."),
        scope: String::new(),
        repo: None,
        model: None,
        state: ThreadState::Starting,
        summary: String::new(),
        updated_ms: 0,
        set_by: None,
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=someone@example.invalid", "-c", "user.name=Ada", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, msg: &str) {
    std::fs::write(dir.join(file), format!("{msg}\nline two\n")).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", msg]);
}

async fn get(app: std::sync::Arc<crate::app::App>, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder().uri(uri).header("host", "127.0.0.1:7681").body(Body::empty()).unwrap();
    let res = signed_in(app).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Steps a millisecond apart, as a real life's are: events of one
/// millisecond from the log and the board have no order between them.
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(3));
}

fn kinds(v: &Value) -> Vec<String> {
    // Newest first on the wire; oldest first is how a conversation reads.
    let mut k: Vec<String> = v["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
    k.reverse();
    k
}

#[tokio::test]
async fn a_plan_life_reads_as_one_ordered_timeline_with_its_commits() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    // A repo with two thread branches: `a` merged with a merge commit and
    // its branch deleted, `b` not merged.
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    git(w, &["init", "-q"]);
    commit(w, "base.txt", "base");
    git(w, &["checkout", "-q", "-b", "sbxw/a"]);
    commit(w, "a1.txt", "a: first step");
    commit(w, "a2.txt", "a: second step");
    git(w, &["checkout", "-q", "main"]);
    git(w, &["checkout", "-q", "-b", "sbxw/b"]);
    commit(w, "b1.txt", "b: only step");
    git(w, &["checkout", "-q", "main"]);
    app.store.update(&sb, |r| r.workspace = Some(w.to_path_buf())).unwrap();

    let p = app.plan_propose(&sb, "Two parts", vec![t("a"), t("b")], Variant::Split, "independent", Some("Lead session")).await.unwrap();
    tick();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a"), t("b")], None, None, crate::app::now_ms()).unwrap();
    tick();
    let (a, b) = (ThreadName::parse("a").unwrap(), ThreadName::parse("b").unwrap());
    app.plan_report(&sb, &a, ThreadState::Working, "on it").unwrap();
    tick();
    app.board_note("myapp", Some(&p.id), "a", Kind::Warning, "the schema moves", None, &[]).unwrap();
    tick();
    app.board_note("myapp", Some(&p.id), "lead", Kind::Decision, "keep the old route", None, &[]).unwrap();
    tick();
    // A round of questions to the person, answered.
    let q = app.relay_open(&sb, Some(&a), RelayKind::Ask, "❓ **Q1 - Port** : which port?\n\n➡️ 8080").unwrap();
    tick();
    app.relay_approve(&q.id, Some("Q1 : 9090"), &[]).unwrap();
    tick();
    // A question to another sandbox, routed, answered and released.
    let other = SandboxRef::parse("docs").unwrap();
    let r = app.relay_open(&sb, Some(&b), RelayKind::Question, "where is the style guide?").unwrap();
    tick();
    app.relay_route(&r.id, &other).unwrap();
    tick();
    app.relay_reply(&r.id, &other, "in docs/style.md").unwrap();
    tick();
    app.relay_approve(&r.id, None, &[]).unwrap();
    tick();
    app.thread_start_failed(&sb, &b, "no worktree");
    tick();
    app.plan_report(&sb, &a, ThreadState::Done, "both steps").unwrap();
    tick();
    app.plan_mark_done(&p.id, &sb, "b", "reviewed sbxw/b").unwrap();
    tick();
    app.plan_propose_merge(&p.id, &sb, vec!["a".into()], "a first", vec![]).unwrap();
    tick();
    // The merge, as the daemon records it: a's branch merged, then deleted.
    git(w, &["merge", "-q", "--no-ff", "-m", "Merge branch 'sbxw/a'", "sbxw/a"]);
    let merge = git(w, &["rev-parse", "HEAD"]);
    git(w, &["branch", "-q", "-D", "sbxw/a"]);
    {
        let mut book = app.plans.lock().unwrap();
        book.start_merge(&p.id, Some("main".into()), None, crate::app::now_ms()).unwrap();
        tick();
        book.merge_result(&p.id, MergeResult { thread: "a".into(), ok: true, commit: Some(merge.clone()), error: None, note: None }, crate::app::now_ms()).unwrap();
        tick();
        book.finish_merge(&p.id, crate::app::now_ms()).unwrap();
    }
    app.persist_plans("myapp");

    let (s, teams) = get(app.clone(), "/api/teams").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(teams["teams"][0]["id"], p.id.as_str());
    assert_eq!(teams["teams"][0]["state"], "merged");

    let (s, v) = get(app.clone(), &format!("/api/teams/{}/timeline", p.id)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let k = kinds(&v);
    let without_commits: Vec<&str> = k.iter().map(String::as_str).filter(|k| *k != "commit").collect();
    assert_eq!(
        without_commits,
        [
            "plan_proposed", "plan_approved", "thread_status", "board_warning", "board_decision", "user_question", "user_answer",
            "relay_question", "relay_routed", "relay_drafted", "relay_answer", "thread_failed", "thread_status", "marked_done",
            "merge_proposed", "merge_started", "merge_result", "merge_finished",
        ],
        "{k:?}"
    );
    let ev = |kind: &str| v["events"].as_array().unwrap().iter().find(|e| e["kind"] == kind).unwrap().clone();
    assert_eq!((ev("plan_approved")["from"].as_str(), ev("plan_approved")["to"].as_str()), (Some("user"), Some("lead")));
    assert_eq!(ev("board_warning")["from"], "a");
    assert_eq!(ev("thread_failed")["from"], "sbxw");
    assert_eq!(ev("thread_failed")["text"], "no worktree");
    assert_eq!(ev("marked_done")["to"], "b");
    // The round's questions and the person's answer, while the relay holds it.
    let ua = ev("user_answer");
    assert_eq!(ua["data"]["questions"][0]["title"], "Port");
    assert_eq!(ua["data"]["questions"][0]["answer"], "9090");
    assert_eq!(ev("relay_routed")["to"], "docs");
    assert_eq!(ev("relay_answer")["text"], "in docs/style.md");
    // Commits: a's two, attributed and merged although its branch is gone; b's one, not merged.
    let commits: Vec<(String, String, bool)> = v["commits"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["subject"].as_str().unwrap().into(), c["thread"].as_str().unwrap_or("").into(), c["merged"].as_bool().unwrap()))
        .collect();
    assert!(commits.contains(&("a: first step".into(), "a".into(), true)), "{commits:?}");
    assert!(commits.contains(&("a: second step".into(), "a".into(), true)), "{commits:?}");
    assert!(commits.contains(&("b: only step".into(), "b".into(), false)), "{commits:?}");
    assert!(!commits.iter().any(|c| c.0 == "base" || c.0.starts_with("Merge")), "only the threads' own: {commits:?}");
    let first = v["commits"]["list"].as_array().unwrap().iter().find(|c| c["subject"] == "a: first step").unwrap().clone();
    assert_eq!((first["files"].as_u64(), first["added"].as_u64(), first["author"].as_str()), (Some(1), Some(2), Some("Ada")));
    assert_eq!(k.iter().filter(|k| *k == "commit").count(), 3);
    let totals = v["commits"]["totals"].as_array().unwrap();
    assert!(totals.iter().any(|t| t["thread"] == "a" && t["commits"] == 2));
    // Standing notes: the decision and the agent's warning, newest first.
    assert_eq!(v["standing"].as_array().unwrap().len(), 2);
    assert_eq!(v["standing"][0]["kind"], "decision");
    // No email anywhere.
    let raw = v.to_string();
    assert!(!raw.contains("example.invalid") && !raw.contains('@'), "an email leaked");

    // Paging: newest first, `before` continues; a filter applies first.
    let (_, p1) = get(app.clone(), &format!("/api/teams/{}/timeline?limit=5", p.id)).await;
    assert_eq!(p1["events"].as_array().unwrap().len(), 5);
    assert_eq!(p1["more"], true);
    assert!(p1.get("commits").is_some());
    let last = p1["events"][4]["id"].as_str().unwrap().to_string();
    let (_, p2) = get(app.clone(), &format!("/api/teams/{}/timeline?limit=5&before={last}", p.id)).await;
    assert!(p2.get("commits").is_none(), "commits only on the first page");
    let ids1: Vec<&Value> = p1["events"].as_array().unwrap().iter().map(|e| &e["id"]).collect();
    assert!(p2["events"].as_array().unwrap().iter().all(|e| !ids1.contains(&&e["id"])));
    let (_, notes) = get(app.clone(), &format!("/api/teams/{}/timeline?group=notes", p.id)).await;
    assert_eq!(kinds(&notes), ["board_warning", "board_decision"]);
    let (_, only_b) = get(app.clone(), &format!("/api/teams/{}/timeline?thread=b", p.id)).await;
    assert!(only_b["events"].as_array().unwrap().iter().all(|e| e["from"] == "b" || e["to"] == "b" || e["data"]["thread"] == "b"), "{only_b}");
    let (_, found) = get(app.clone(), &format!("/api/teams/{}/timeline?q=SCHEMA", p.id)).await;
    assert_eq!(kinds(&found), ["board_warning"]);
    assert_eq!(get(app.clone(), &format!("/api/teams/{}/timeline?group=nope", p.id)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get(app.clone(), "/api/teams/p-nope/timeline").await.0, StatusCode::NOT_FOUND);

    // After a restart: the log is read back, the relay's texts with it (the
    // person's choice, KEEP_RELAY_TEXT), clipped; never an image.
    let again = app_in(_d.path());
    again.store.update(&sb, |r| r.workspace = Some(w.to_path_buf())).unwrap();
    let (_, after) = get(again.clone(), &format!("/api/teams/{}/timeline", p.id)).await;
    assert_eq!(kinds(&after).iter().filter(|k| *k != "commit").count(), without_commits.len());
    let ra = after["events"].as_array().unwrap().iter().find(|e| e["kind"] == "relay_answer").unwrap().clone();
    assert_eq!(ra["text"], "in docs/style.md", "kept with the plan");
    let rq = after["events"].as_array().unwrap().iter().find(|e| e["kind"] == "relay_question").unwrap().clone();
    assert_eq!(rq["text"], "where is the style guide?");
    let file = std::fs::read_to_string(again.store.paths.plans().join("team").join("myapp.json")).unwrap();
    assert!(file.contains("docs/style.md") && !file.contains("b64"), "the text is kept, no image");
}

#[tokio::test]
async fn a_team_note_is_on_the_timeline_and_read_once_by_its_addressees() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    let p = app.plan_propose(&sb, "Notes", vec![t("a"), t("b")], Variant::Split, "x", Some("Lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a"), t("b")], None, None, crate::app::now_ms()).unwrap();
    let a = ThreadName::parse("a").unwrap();
    // The lead to a; a to the whole team.
    let (_, told) = app.team_note(&sb, None, "a", "info", "The fixtures moved to tests/data.").unwrap();
    assert!(told.is_empty(), "no session open: recorded, nobody typed to");
    tick();
    app.team_note(&sb, Some(&a), "team", "warning", "Do not touch the money helper today.").unwrap();
    // Refused: an unknown kind, an addressee outside the plan, oneself, no team.
    assert!(app.team_note(&sb, None, "a", "order", "x").is_err());
    assert!(app.team_note(&sb, None, "zz", "info", "x").is_err());
    assert!(app.team_note(&sb, Some(&a), "a", "info", "x").is_err());
    assert!(app.team_note(&SandboxRef::parse("other").unwrap(), None, "a", "info", "x").is_err());
    // On the timeline, as notes, from whom to whom.
    let (_, v) = get(app.clone(), &format!("/api/teams/{}/timeline?group=notes", p.id)).await;
    let notes: Vec<(String, String, String)> = v["events"].as_array().unwrap().iter().rev().map(|e| (e["from"].as_str().unwrap().into(), e["to"].as_str().unwrap().into(), e["data"]["kind"].as_str().unwrap().into())).collect();
    assert_eq!(notes, [("lead".into(), "a".into(), "info".into()), ("a".into(), "team".into(), "warning".into())]);
    assert_eq!(v["standing"][0]["text"], "Do not touch the money helper today.", "a warning stands pinned");
    // Each addressee reads its own once; the sender is not handed its own.
    let mine = app.team_notes_read(&sb, Some(&a)).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["text"], "The fixtures moved to tests/data.");
    assert!(app.team_notes_read(&sb, Some(&a)).unwrap().is_empty(), "once");
    let lead = app.team_notes_read(&sb, None).unwrap();
    assert_eq!((lead.len(), lead[0]["kind"].as_str()), (1, Some("warning")));
    let b = app.team_notes_read(&sb, Some(&ThreadName::parse("b").unwrap())).unwrap();
    assert_eq!(b.len(), 1, "to the team reaches every other session");
}

#[tokio::test]
async fn a_plan_from_before_the_log_shows_with_unknown_times() {
    let (d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    let p = app.plan_propose(&sb, "Old", vec![t("a")], Variant::Split, "one", None).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a")], None, None, crate::app::now_ms()).unwrap();
    app.plan_report(&sb, &ThreadName::parse("a").unwrap(), ThreadState::Working, "going").unwrap();
    // As a state file from before logs: the plans only.
    std::fs::remove_dir_all(app.store.paths.plans().join("team")).unwrap();
    let old = app_in(d.path());
    let (_, v) = get(old, &format!("/api/teams/{}/timeline", p.id)).await;
    assert_eq!(kinds(&v), ["plan_proposed", "plan_approved", "thread_status"]);
    let approved = v["events"].as_array().unwrap().iter().find(|e| e["kind"] == "plan_approved").unwrap().clone();
    assert_eq!(approved["at"], Value::Null, "time unknown");
}

#[tokio::test]
async fn board_entries_name_their_plan_and_older_ones_are_a_guess() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    let p = app.plan_propose(&sb, "Shared", vec![t("a")], Variant::Split, "x", None).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a")], None, None, crate::app::now_ms()).unwrap();
    let e = app.board_note("myapp", Some(&p.id), "a", Kind::Decision, "named", None, &[]).unwrap();
    assert_eq!(e.plan.as_deref(), Some(p.id.as_str()));
    // An entry as an old board file holds it: no plan.
    app.with_board("myapp", None, |b| {
        b.set_call_plan(None);
        b.note("a", Kind::Warning, "old entry", None, &[], crate::app::now_ms()).unwrap();
    });
    let (_, v) = get(app.clone(), &format!("/api/teams/{}/timeline?group=notes", p.id)).await;
    let ev: Vec<(String, bool)> = v["events"].as_array().unwrap().iter().map(|e| (e["text"].as_str().unwrap().to_string(), e["guess"].as_bool().unwrap_or(false))).collect();
    assert!(ev.contains(&("named".into(), false)), "{ev:?}");
    assert!(ev.contains(&("old entry".into(), true)), "inferred from its time and author: {ev:?}");
}

#[tokio::test]
async fn the_live_event_goes_out_only_while_a_ui_listens() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    let mut rx = app.bus.subscribe();
    let p = app.plan_propose(&sb, "x", vec![t("a")], Variant::Split, "x", None).await.unwrap();
    let teams = |rx: &mut tokio::sync::broadcast::Receiver<Event>| {
        let mut n = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, Event::Team(ref id) if id == &p.id) {
                n += 1;
            }
        }
        n
    };
    assert_eq!(teams(&mut rx), 0, "no UI: no team event");
    app.bus.clients.send_modify(|n| *n += 1);
    app.plans.lock().unwrap().approve(&p.id, vec![t("a")], None, None, crate::app::now_ms()).unwrap();
    app.plan_report(&sb, &ThreadName::parse("a").unwrap(), ThreadState::Working, "x").unwrap();
    assert!(teams(&mut rx) >= 1, "a UI listens: the timeline grew");
    app.bus.clients.send_modify(|n| *n -= 1);
    app.plan_report(&sb, &ThreadName::parse("a").unwrap(), ThreadState::Blocked, "y").unwrap();
    assert_eq!(teams(&mut rx), 0);
}

/// A hook event as the status hook posts it, from thread `t` (none: the lead).
fn hook(app: &std::sync::Arc<crate::app::App>, sb: &SandboxRef, t: Option<&str>, mut ev: Value) {
    ev["session_id"] = format!("sid-{}", t.unwrap_or("lead")).into();
    ev["hook_version"] = 2.into();
    if let Some(t) = t {
        ev["sbxw_thread"] = t.into();
    }
    app.ingest_hook(sb, ev);
}

fn sent(to: &str, message: &str, ok: bool) -> Value {
    serde_json::json!({
        "hook_event_name": "PostToolUse", "tool_name": "SendMessage",
        "tool_input": { "to": to, "message": message, "summary": "a label" },
        "tool_response": { "success": ok, "message": format!("\"a label\" → {to} (another Claude session on this machine)") },
    })
}

#[tokio::test]
async fn the_team_s_direct_messages_are_on_its_timeline_and_nothing_else_of_the_hooks() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    let p = app.plan_propose(&sb, "Talk", vec![t("a"), t("b")], Variant::Split, "x", Some("Lead session")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a"), t("b")], None, None, crate::app::now_ms()).unwrap();
    // The lead to a; a back to the lead by name, then to its `from` address.
    hook(&app, &sb, None, sent("a", "Start with the routes, the page waits on them.", true));
    tick();
    hook(&app, &sb, Some("a"), sent("Lead session", "Routes done, tests green.", true));
    tick();
    let mut reply = sent("uds:/run/user/1000/cc-socks/1.sock", "One more question on the totals.", true);
    reply["tool_response"]["message"] = "\"q\" → Lead session (another Claude session on this machine)".into();
    hook(&app, &sb, Some("a"), reply);
    // Not recorded: twice the same send, a session outside the team, a
    // failed send, another tool's input, a hook without a response.
    hook(&app, &sb, Some("a"), sent("Lead session", "Routes done, tests green.", true));
    hook(&app, &sb, Some("a"), sent("someone-else", "Outside the team.", true));
    hook(&app, &sb, None, sent("b", "Never delivered.", false));
    hook(&app, &sb, Some("b"), serde_json::json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "to": "a", "message": "a command's words", "command": "echo hi" }, "tool_response": { "success": true } }));
    hook(&app, &sb, Some("b"), serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": "SendMessage", "tool_input": { "to": "a", "message": "Not sent yet." } }));
    let (_, v) = get(app.clone(), &format!("/api/teams/{}/timeline?group=messages", p.id)).await;
    let msgs: Vec<(String, String, String)> = v["events"].as_array().unwrap().iter().rev().map(|e| (e["from"].as_str().unwrap().into(), e["to"].as_str().unwrap().into(), e["text"].as_str().unwrap().into())).collect();
    assert_eq!(
        msgs,
        [
            ("lead".into(), "a".into(), "Start with the routes, the page waits on them.".into()),
            ("a".into(), "lead".into(), "Routes done, tests green.".into()),
            ("a".into(), "lead".into(), "One more question on the totals.".into()),
        ]
    );
    assert!(v["events"].as_array().unwrap().iter().all(|e| e["kind"] == "direct_message" && e["data"].is_null()), "only the addressee and the words");
    let file = std::fs::read_to_string(app.store.paths.plans().join("team").join("myapp.json")).unwrap();
    assert!(!file.contains("a label") && !file.contains("echo hi") && !file.contains("Not sent yet") && !file.contains("Never delivered"));
}

#[tokio::test]
async fn a_note_s_line_waits_while_its_session_is_asked_something_and_is_dropped_when_it_ended() {
    use sbxw_core::team::Delivery;
    let (_d, app) = app();
    let sb = SandboxRef::parse("myapp").unwrap();
    let p = app.plan_propose(&sb, "Lines", vec![t("a"), t("b"), t("c")], Variant::Split, "x", Some("Lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a"), t("b"), t("c")], None, None, crate::app::now_ms()).unwrap();
    // a waits on a question (its menu would take a typed line as a pick); b is
    // idle; c's session ended; the lead works.
    hook(&app, &sb, Some("a"), serde_json::json!({ "hook_event_name": "SessionStart" }));
    hook(&app, &sb, Some("a"), serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "tool_input": { "questions": [{ "question": "Which?", "options": [{ "label": "A" }, { "label": "B" }] }] } }));
    hook(&app, &sb, Some("b"), serde_json::json!({ "hook_event_name": "SessionStart" }));
    hook(&app, &sb, Some("b"), serde_json::json!({ "hook_event_name": "Stop", "last_assistant_message": "Done." }));
    hook(&app, &sb, Some("c"), serde_json::json!({ "hook_event_name": "SessionStart" }));
    hook(&app, &sb, Some("c"), serde_json::json!({ "hook_event_name": "SessionEnd" }));
    hook(&app, &sb, None, serde_json::json!({ "hook_event_name": "UserPromptSubmit", "prompt": "go" }));
    let who: Vec<String> = ["a", "b", "c", "lead"].iter().map(|s| s.to_string()).collect();
    let got: Vec<(String, Delivery)> = app.note_targets(&sb, &who, |_| true);
    assert_eq!(got, [("a".into(), Delivery::Hold), ("b".into(), Delivery::Type), ("c".into(), Delivery::Drop), ("lead".into(), Delivery::Type)]);
    // A session not open: never typed, the note waits for read_team_notes.
    assert!(app.note_targets(&sb, &who, |_| false).iter().all(|(_, d)| *d == Delivery::Drop));
    // a answered: idle again, the held line goes.
    hook(&app, &sb, Some("a"), serde_json::json!({ "hook_event_name": "Stop", "last_assistant_message": "Picked." }));
    assert_eq!(app.note_targets(&sb, &who[..1], |_| true), [("a".into(), Delivery::Type)]);
}
