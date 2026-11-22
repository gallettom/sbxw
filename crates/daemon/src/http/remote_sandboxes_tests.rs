//! Plan phone-sandboxes: besides team leaders, a paired phone reaches the
//! main session of every running sandbox that leads no team (API.md 2.4,
//! v1.5). Who is reached is decided in one place
//! (`sbxw_core::remote_access::reach`); these tests hold every `:sandbox`
//! route of level 1 to it, from outside, through the router.

use super::remote::{remote_router, ROUTES};
use super::remote_tests::{call, hook, leader, on, pair, read_until, req, send, typed, Capture, Reply, TN};
use crate::app::App;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::COOKIE;
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// A running sandbox with no team: sbxw's terminal on it (a fake agent that
/// turns bracketed paste on and records what is typed) and its main session.
async fn plain(app: &Arc<App>, dir: &std::path::Path, name: &str) -> (SandboxRef, PathBuf) {
    let sb = SandboxRef::parse(name).unwrap();
    let file = dir.join(format!("{name}.typed"));
    let script = format!("printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat > '{}'", file.display());
    let (s, _) = app
        .sessions
        .get_or_open(&SessionKey::agent(&sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) })
        .await
        .unwrap();
    for _ in 0..100 {
        if sbxw_core::terminal::bracketed_paste_enabled(&s.replay()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    hook(app, &sb, "s1", json!({"hook_event_name": "SessionStart"}));
    (sb, file)
}

fn entry(list: &Value, name: &str) -> Option<Value> {
    list["leaders"].as_array()?.iter().find(|l| l["sandbox"] == name).cloned()
}

/// The level 1 routes that take a `:sandbox` (the Kanban's included), from
/// the router's own list, with a body each route accepts, so that a refusal
/// can only come from resolving the name.
fn sandbox_routes() -> Vec<(&'static str, &'static str)> {
    let r: Vec<_> = ROUTES.iter().copied().filter(|(_, p)| p.starts_with("/r/leaders/:sandbox")).collect();
    assert_eq!(r.len(), 12, "history, message, answer, interrupt, model, skill, lead, upload, upload drop, kanban, kanban ticket, kanban create: {r:?}");
    r
}

async fn hit(router: &Router, token: &str, method: &str, path: &str, name: &str) -> Reply {
    let p = path.replace(":sandbox", name).replace(":id", &"a".repeat(32)).replace(":ticket", "1");
    if p.ends_with("/upload") {
        let r = Request::builder()
            .method("POST")
            .uri(format!("{p}?name=notes.txt"))
            .header(header::HOST, TN)
            .header(header::ORIGIN, format!("https://{TN}"))
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::COOKIE, format!("{COOKIE}={token}"))
            .body(Body::from("hello"))
            .unwrap();
        return send(router, r).await;
    }
    let body = if p.ends_with("/message") {
        json!({"text": "hello"})
    } else if p.ends_with("/answer") {
        json!({"indices": [1]})
    } else if p.ends_with("/model") {
        json!({"id": "x"})
    } else if p.ends_with("/skill") {
        json!({"name": "retro"})
    } else if p.ends_with("/lead") {
        json!({"on": true})
    } else if p.ends_with("/kanban") {
        json!({"title": "x"})
    } else {
        json!({})
    };
    call(router, method, &p, Some(token), body).await
}

/// `name` answers every `:sandbox` route exactly as a name that exists
/// nowhere: the same status and the same body.
async fn refused_as_unknown(router: &Router, token: &str, name: &str) {
    for (m, p) in sandbox_routes() {
        let unknown = hit(router, token, m, p, "nosuchsandbox").await;
        let got = hit(router, token, m, p, name).await;
        assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{m} {p}: {}", unknown.text());
        assert_eq!((got.status, got.text()), (unknown.status, unknown.text()), "{m} {p} for {name}");
    }
}

#[tokio::test]
async fn a_running_plain_sandbox_is_listed_and_reached_on_the_level_1_routes() {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(logs.clone());
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _lead = leader(&app, d.path(), "lead1").await;
    let (sb, file) = plain(&app, d.path(), "app1").await;

    let list = call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json();
    let names: Vec<(&str, &str)> = list["leaders"].as_array().unwrap().iter().map(|l| (l["sandbox"].as_str().unwrap(), l["kind"].as_str().unwrap())).collect();
    assert_eq!(names, vec![("lead1", "leader"), ("app1", "sandbox")], "leaders first");
    let e = entry(&list, "app1").unwrap();
    assert!(e.get("read_only").is_none(), "{e}");
    assert_eq!(e["state"], "idle");

    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "earlier words"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "done"}));
    let r = call(&router, "GET", "/r/leaders/app1/history", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.text().contains("earlier words"));

    // A message, journalled as a message with no text, never logged.
    let secret = "MANGO-42 the plain sandbox";
    let r = call(&router, "POST", "/r/leaders/app1/message", Some(&token), json!({ "text": secret })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(String::from_utf8_lossy(&typed(&file, b"MANGO-42").await).contains(secret));
    assert_eq!(app.remote_journal(10).last().map(|e| e.kind), Some(Kind::Message));
    let journal = std::fs::read_to_string(app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    assert!(!journal.contains("MANGO") && !journal.contains("app1"), "{journal}");
    let logged = logs.0.lock().unwrap().clone();
    assert!(!logged.contains("MANGO"), "{logged}");

    // A file: accepted under the same limits as for a leader.
    let r = hit(&router, &token, "POST", "/r/leaders/:sandbox/upload", "app1").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    let r = call(&router, "DELETE", &format!("/r/leaders/app1/upload/{id}"), Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());

    // A question, answered.
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which?", "options": [{"label": "A"}, {"label": "B"}]}]}}));
    let e = entry(&call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json(), "app1").unwrap();
    assert_eq!((e["asks"].clone(), e["answerable"].clone()), (json!(true), json!(true)), "{e}");
    super::remote_tests::takes_answer(&app, &sb, &file, b"\x1b[B\r");
    let r = call(&router, "POST", "/r/leaders/app1/answer", Some(&token), json!({"indices": [2]})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());

    // Interrupt: one Escape.
    let r = call(&router, "POST", "/r/leaders/app1/interrupt", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(app.remote_journal(10).last().map(|e| e.kind), Some(Kind::Interrupt));

    // Its Kanban: a sandbox with no team reads its board (tickets only, no
    // thread columns) and creates a ticket.
    let r = call(&router, "GET", "/r/leaders/app1/kanban", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["sandbox"], "app1");
    let r = call(&router, "POST", "/r/leaders/app1/kanban", Some(&token), json!({"title": "from the phone"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let n = r.json()["n"].as_u64().unwrap();
    let r = call(&router, "GET", &format!("/r/leaders/app1/kanban/{n}"), Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let board = call(&router, "GET", "/r/leaders/app1/kanban", Some(&token), json!({})).await.json();
    assert!(board.to_string().contains("from the phone"), "{board}");
    assert_eq!(app.remote_status().leaders, vec!["lead1".to_string()], "sbxw remote status names leaders only");
}

#[tokio::test]
async fn stopped_thread_background_and_jarvis_are_refused_as_an_unknown_name() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (lead, _) = leader(&app, d.path(), "lead1").await;

    // A stopped sandbox: its session ended.
    let (gone, _) = plain(&app, d.path(), "gone1").await;
    hook(&app, &gone, "s1", json!({"hook_event_name": "SessionEnd", "reason": "exit"}));
    // A sandbox whose only session is a thread's.
    let mut t = json!({"hook_event_name": "SessionStart"});
    t["sbxw_thread"] = "worker".into();
    hook(&app, &SandboxRef::parse("thread1").unwrap(), "s3", t);
    // A sandbox whose only session is a harness background session.
    let mut b = json!({"hook_event_name": "SessionStart"});
    b["session_kind"] = "bg".into();
    hook(&app, &SandboxRef::parse("bg1").unwrap(), "s4", b);
    // A leader (sbxw's terminal made lead) whose only session is a
    // background one: no main session, so not reached either.
    let (bglead, _) = plain(&app, d.path(), "bglead").await;
    hook(&app, &bglead, "s1", json!({"hook_event_name": "SessionEnd", "reason": "exit"}));
    let pty = app.sessions.live(&SessionKey::agent(&bglead)).unwrap();
    app.leads.lock().unwrap().insert(bglead.clone(), Arc::as_ptr(&pty) as usize);
    let mut b = json!({"hook_event_name": "SessionStart"});
    b["session_kind"] = "bg".into();
    hook(&app, &bglead, "s5", b);
    assert!(app.leading().contains(&"bglead".to_string()), "a leader by the Lead Mode rule");
    // The Jarvis sandbox, running, with sbxw's terminal on it.
    let (_j, _) = plain(&app, d.path(), crate::svc::jarvis_sandbox::NAME).await;

    let list = call(&router, "GET", "/r/leaders", Some(&token), json!({})).await;
    let names: Vec<String> = list.json()["leaders"].as_array().unwrap().iter().map(|l| l["sandbox"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, vec!["lead1".to_string()], "{}", list.text());
    for name in ["gone1", "thread1", "bg1", "bglead", crate::svc::jarvis_sandbox::NAME] {
        refused_as_unknown(&router, &token, name).await;
    }

    // A background session in a leader's or a plain sandbox is never the
    // session shown or typed into: the main one is.
    let (app1, _) = plain(&app, d.path(), "app1").await;
    for sb in [&lead, &app1] {
        hook(&app, sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "MAIN WORDS"}));
        hook(&app, sb, "s1", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "ok"}));
        let mut b = json!({"hook_event_name": "UserPromptSubmit", "prompt": "BACKGROUND WORDS"});
        b["session_kind"] = "bg".into();
        hook(&app, sb, "s8", b);
        let r = call(&router, "GET", &format!("/r/leaders/{sb}/history"), Some(&token), json!({})).await;
        assert_eq!(r.json()["session_id"], "s1", "{}", r.text());
        assert!(r.text().contains("MAIN WORDS") && !r.text().contains("BACKGROUND WORDS"), "{}", r.text());
    }
}

#[tokio::test]
async fn a_sandbox_that_stops_while_the_phone_is_on_it_is_gone() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = plain(&app, d.path(), "app1").await;
    let r = req("GET", "/r/events").header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    let mut s = router.clone().oneshot(r).await.unwrap().into_body().into_data_stream();
    let first = read_until(&mut s, "\"kind\":\"sandbox\"").await.expect("listed at connect");
    assert!(first.contains("app1"), "{first}");
    assert_eq!(call(&router, "GET", "/r/leaders/app1/history", Some(&token), json!({})).await.status, StatusCode::OK);

    // Stopped out of band: the daemon's reconcile drops its sessions.
    let gone = app.agents.lock().unwrap().reconcile(&["elsewhere".to_string()], crate::app::now_ms());
    for info in gone {
        app.send(crate::bus::Event::Session(info));
    }
    let frame = read_until(&mut s, "{\"leaders\":[]}").await;
    assert!(frame.is_some(), "the stream says it left");
    refused_as_unknown(&router, &token, "app1").await;
    let _ = sb;
}

#[tokio::test]
async fn the_list_follows_a_sandbox_becoming_a_leader() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let r = req("GET", "/r/events").header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    let mut s = router.clone().oneshot(r).await.unwrap().into_body().into_data_stream();
    assert!(read_until(&mut s, "{\"leaders\":[]}").await.is_some(), "nothing runs");
    let (sb, _) = plain(&app, d.path(), "app1").await;
    assert!(read_until(&mut s, "\"kind\":\"sandbox\"").await.is_some(), "started: in, as a sandbox");
    let pty = app.sessions.live(&SessionKey::agent(&sb)).unwrap();
    app.leads.lock().unwrap().insert(sb.clone(), Arc::as_ptr(&pty) as usize);
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "go"}));
    let frame = read_until(&mut s, "\"kind\":\"leader\"").await.expect("now a leader");
    assert!(frame.contains("app1"), "{frame}");
}

#[tokio::test]
async fn the_typing_refusals_are_unchanged_and_a_read_only_entry_says_why() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;

    // Working: read, written to (Claude Code queues the message), interrupted.
    let (busy, _) = plain(&app, d.path(), "busy1").await;
    hook(&app, &busy, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "work"}));
    let r = call(&router, "POST", "/r/leaders/busy1/message", Some(&token), json!({"text": "hi"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(call(&router, "POST", "/r/leaders/busy1/interrupt", Some(&token), json!({})).await.status, StatusCode::OK);

    // A permission prompt: the Mac's.
    let (perm, _) = plain(&app, d.path(), "perm1").await;
    hook(&app, &perm, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}}));
    hook(&app, &perm, "s1", json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "needs permission"}));
    let r = call(&router, "POST", "/r/leaders/perm1/message", Some(&token), json!({"text": "hi"})).await;
    assert_eq!((r.status, r.json()["reason"].clone()), (StatusCode::CONFLICT, json!("needs_mac")), "{}", r.text());
    assert_eq!(call(&router, "POST", "/r/leaders/perm1/answer", Some(&token), json!({"indices": [1]})).await.status, StatusCode::CONFLICT);

    // Two sessions from the terminal: sbxw cannot tell which is its own.
    let (two, file) = plain(&app, d.path(), "two1").await;
    hook(&app, &two, "s2", json!({"hook_event_name": "SessionStart"}));
    let why = app.agents.lock().unwrap().can_type_into("two1").unwrap_err().to_string();
    let e = entry(&call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json(), "two1").unwrap();
    assert_eq!(e["read_only"], "several_sessions", "{e}");
    for path in ["/r/leaders/two1/message", "/r/leaders/two1/interrupt"] {
        let body = if path.ends_with("message") { json!({"text": "hi"}) } else { json!({}) };
        let r = call(&router, "POST", path, Some(&token), body).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{path}: {}", r.text());
        assert!(r.json()["error"].as_str().unwrap().contains(&why), "{path}: {}", r.text());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "nothing typed, no Escape");

    // Running, but sbxw holds no terminal on it (started elsewhere).
    let bare = SandboxRef::parse("bare1").unwrap();
    hook(&app, &bare, "s1", json!({"hook_event_name": "SessionStart"}));
    let e = entry(&call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json(), "bare1").unwrap();
    assert_eq!(e["read_only"], "no_terminal", "{e}");
    assert_eq!(call(&router, "GET", "/r/leaders/bare1/history", Some(&token), json!({})).await.status, StatusCode::OK, "it can be read");
    for path in ["/r/leaders/bare1/message", "/r/leaders/bare1/interrupt"] {
        let body = if path.ends_with("message") { json!({"text": "hi"}) } else { json!({}) };
        let r = call(&router, "POST", path, Some(&token), body).await;
        assert_eq!((r.status, r.json()["reason"].clone()), (StatusCode::CONFLICT, json!("needs_mac")), "{path}: {}", r.text());
    }
}

#[tokio::test]
async fn a_leader_whose_terminal_session_is_known_is_still_interrupted() {
    // Decision I1's limit: a leader with threads (sessions of their own,
    // not the terminal's) still resolves to one session: Escape as before.
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    let mut t = json!({"hook_event_name": "SessionStart"});
    t["sbxw_thread"] = "worker".into();
    hook(&app, &sb, "s9", t);
    let e = entry(&call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json(), "lead1").unwrap();
    assert!(e.get("read_only").is_none(), "{e}");
    let r = call(&router, "POST", "/r/leaders/lead1/interrupt", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(typed(&file, b"\x1b").await, b"\x1b");
}

#[tokio::test]
async fn level_2_answers_do_not_change_with_a_running_sandbox() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let level2 = ["/r/approvals", "/r/plans", "/r/merges", "/r/network", "/r/stopped", "/r/stopped/app1"];
    let mut before = Vec::new();
    for p in level2 {
        let r = call(&router, "GET", p, Some(&token), json!({})).await;
        before.push((r.status, r.text()));
    }
    let _ = plain(&app, d.path(), "app1").await;
    for (p, was) in level2.iter().zip(before) {
        let r = call(&router, "GET", p, Some(&token), json!({})).await;
        assert_eq!((r.status, r.text()), was, "{p}");
    }
}

#[tokio::test]
async fn a_plain_sandbox_ticket_sends_a_kanban_frame() {
    // Every reached sandbox has a board, so its changes are told too.
    let (d, app) = on();
    let (lead, _) = leader(&app, d.path(), "lead1").await;
    let (app1, _) = plain(&app, d.path(), "app1").await;
    let list = app.remote_leaders();
    assert_eq!(list.len(), 2, "both reached");
    let mut seen = super::remote_kanban::baseline(&app, &list);
    assert!(seen.contains_key("app1"), "a baseline for a sandbox with no team");
    let draft = || sbxw_core::tickets::Draft { title: Some("t".into()), description: Some("".into()), priority: None };
    app.ticket_create(&app1, draft()).unwrap();
    let ev = crate::bus::Event::Tickets(crate::svc::tickets::TicketsNudge { sandbox: "app1".into(), n: None, what: "created" });
    assert!(super::remote_kanban::changed(&app, &list, &ev, &mut seen).is_some());
    app.ticket_create(&lead, draft()).unwrap();
    let ev = crate::bus::Event::Tickets(crate::svc::tickets::TicketsNudge { sandbox: "lead1".into(), n: None, what: "created" });
    assert!(super::remote_kanban::changed(&app, &list, &ev, &mut seen).is_some(), "a leader's still does");
}

/// Two main sessions in one sandbox: the phone shows and types into the one
/// sbxw's terminal types into (the rule of `can_type_into`), even when it is
/// the older; when the daemon cannot tell, the entry is read only.
#[tokio::test]
async fn the_shown_session_is_the_terminals_one_or_the_entry_is_read_only() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    // A client session (an `ssh <name>.sbx`), newer than the terminal's.
    let client = |words: &str| json!({"hook_event_name": "UserPromptSubmit", "prompt": words, "ancestry": ["sshd", "bash", "claude"]});
    for lead in [false, true] {
        let name = if lead { "lead2" } else { "app2" };
        let (sb, file) = if lead { leader(&app, d.path(), name).await } else { plain(&app, d.path(), name).await };
        hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "TERMINAL WORDS"}));
        hook(&app, &sb, "s1", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "ok"}));
        hook(&app, &sb, "s2", client("CLIENT WORDS"));
        hook(&app, &sb, "s2", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "ok", "ancestry": ["sshd", "bash", "claude"]}));
        let e = entry(&call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json(), name).unwrap();
        assert!(e.get("read_only").is_none(), "{name}: {e}");
        let r = call(&router, "GET", &format!("/r/leaders/{name}/history"), Some(&token), json!({})).await;
        assert_eq!(r.json()["session_id"], "s1", "{name}: the terminal's, though older: {}", r.text());
        assert!(r.text().contains("TERMINAL WORDS") && !r.text().contains("CLIENT WORDS"), "{name}");
        let r = call(&router, "POST", &format!("/r/leaders/{name}/message"), Some(&token), json!({"text": "to the terminal"})).await;
        assert_eq!(r.status, StatusCode::OK, "{name}: {}", r.text());
        assert!(String::from_utf8_lossy(&typed(&file, b"to the terminal").await).contains("to the terminal"));
        // A third, also from the terminal's place: the daemon cannot tell.
        hook(&app, &sb, "s3", json!({"hook_event_name": "SessionStart"}));
        let e = entry(&call(&router, "GET", "/r/leaders", Some(&token), json!({})).await.json(), name).unwrap();
        assert_eq!(e["read_only"], "several_sessions", "{name}: {e}");
        let r = call(&router, "POST", &format!("/r/leaders/{name}/message"), Some(&token), json!({"text": "hi"})).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{name}: {}", r.text());
    }
}

// ── Defect report: "Team on" in the app, no leader for the phone ─────────
// After a daemon restart a lead is unbound and waits in `lead_recs` for its
// recorded session; `leading()` (plans.rs) re-binds it only when
// `main_session` (the newest thread-less live session) IS that id. These
// reproduce the two ways that failed. The background spare is fixed (the
// shared main-session rule). The resumed conversation stays refused: whether
// a session that continues the recorded one may lead again is the person's
// decision (lead-persist's trust limit), still open; after a resume the
// person turns Team on again. Run it with `--ignored`.

/// The daemon restarted: the lead is on disk, its terminal not bound yet.
async fn restarted_lead(app: &Arc<App>, dir: &std::path::Path, name: &str) -> SandboxRef {
    let (sb, _) = plain(app, dir, name).await;
    let now = crate::app::now_ms();
    app.lead_recs.lock().unwrap().insert(sb.clone(), crate::store::LeadRecord { since_ms: now, session_id: Some("s1".into()), bound_ms: now });
    sb
}

#[tokio::test]
async fn control_a_restarted_lead_with_its_own_session_leads_again() {
    let (d, app) = on();
    let sb = restarted_lead(&app, d.path(), "sbxw2").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "hi"}));
    assert!(app.leading().contains(&"sbxw2".to_string()));
}

#[tokio::test]
async fn defect_a_background_spare_drops_a_restarted_lead() {
    let (d, app) = on();
    let sb = restarted_lead(&app, d.path(), "sbxw2").await;
    let mut b = json!({"hook_event_name": "SessionStart"});
    b["session_kind"] = "bg".into();
    hook(&app, &sb, "s5", b);
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "hi"}));
    assert!(app.leading().contains(&"sbxw2".to_string()), "a newer background spare is not the main session");
}

#[tokio::test]
#[ignore = "open decision: a resumed conversation has a new session id and is not re-bound (lead-persist trust limit); the person turns Team on again"]
async fn defect_a_resumed_main_session_drops_a_restarted_lead() {
    let (d, app) = on();
    let sb = restarted_lead(&app, d.path(), "sbxw2").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "SessionEnd", "reason": "resume"}));
    hook(&app, &sb, "s2", json!({"hook_event_name": "SessionStart", "source": "resume"}));
    assert!(app.leading().contains(&"sbxw2".to_string()), "s2 continues s1, but is not s1: lead dropped");
}
