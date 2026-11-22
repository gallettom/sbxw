//! The canvas on the phone (API-canvas.md), through the remote router: a
//! reachable sandbox's canvases are read, annotated and sent; every other
//! canvas is the same 404; a picture only by the manifest's own name; a
//! round sent types its one fixed line, under the phone's message gate.

use super::remote::{remote_router, with_headers};
use super::remote_tests::{call, hook, leader, on, pair, send, req, typed, Reply};
use crate::app::App;
use crate::svc::canvas::ShotIn;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use sbxw_core::canvas::{self, Edit, Pin};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::COOKIE;
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend(13u32.to_be_bytes());
    b.extend(b"IHDR");
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b.extend(0u32.to_be_bytes());
    b.extend(b"IDAT");
    b
}

fn shot(title: &str, note: &str) -> ShotIn {
    ShotIn { b64: sbxw_core::mac::base64(&png(390, 844)), title: title.into(), note: note.into(), width: Some(390), screen: Some("home".into()) }
}

/// A canvas of `sb` with `n` shots in one round; its id.
fn canvas_of(app: &App, sb: &str, title: &str, n: usize) -> String {
    let sb = SandboxRef::parse(sb).unwrap();
    let mut id = None;
    for k in 0..n {
        let a = app.canvas_agent_add(&sb, id.as_deref(), title, "b1", shot(&format!("Screen {k}"), "a note")).unwrap();
        id = Some(a.canvas);
    }
    id.unwrap()
}

/// A running sandbox with sbxw's terminal on it (no team).
async fn plain(app: &Arc<App>, dir: &std::path::Path, name: &str) -> SandboxRef {
    let sb = SandboxRef::parse(name).unwrap();
    let file = dir.join(format!("{name}.typed"));
    let script = format!("printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat > '{}'", file.display());
    app.sessions
        .get_or_open(&SessionKey::agent(&sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) })
        .await
        .unwrap();
    hook(app, &sb, "s1", json!({"hook_event_name": "SessionStart"}));
    sb
}

async fn get(router: &Router, token: &str, path: &str) -> Reply {
    call(router, "GET", path, Some(token), json!({})).await
}

async fn remark(router: &Router, token: &str, id: &str, body: Value) -> Reply {
    call(router, "POST", &format!("/r/canvas/{id}/remarks"), Some(token), body).await
}

async fn submit(router: &Router, token: &str, id: &str) -> Reply {
    call(router, "POST", &format!("/r/canvas/{id}/submit"), Some(token), json!({})).await
}

/// Every canvas route of `id` answers exactly as an id that exists nowhere.
async fn refused_as_unknown(router: &Router, token: &str, id: &str, file: &str) {
    let unknown = "c-00000000000000ff";
    for (m, p, body) in [
        ("GET", "/r/canvas/{}", json!({})),
        ("GET", "/r/canvas/{}/images/{f}", json!({})),
        ("POST", "/r/canvas/{}/remarks", json!({ "op": "remove", "kind": "pin", "n": 1 })),
        ("POST", "/r/canvas/{}/submit", json!({})),
    ] {
        let u = call(router, m, &p.replace("{}", unknown).replace("{f}", file), Some(token), body.clone()).await;
        let got = call(router, m, &p.replace("{}", id).replace("{f}", file), Some(token), body).await;
        assert_eq!(u.status, StatusCode::NOT_FOUND, "{m} {p}: {}", u.text());
        assert_eq!((got.status, got.text()), (u.status, u.text()), "{m} {p} for {id}");
    }
}

#[tokio::test]
async fn a_reachable_sandboxes_canvas_is_read_and_every_other_is_not_found() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, _) = leader(&app, d.path(), "lead1").await;
    let ours = canvas_of(&app, "lead1", "Review for someone@example.org", 2);
    // A stopped sandbox (never ran here), a thread's, Jarvis's (running).
    let stopped = canvas_of(&app, "gone1", "Stopped", 1);
    let mut t = json!({"hook_event_name": "SessionStart"});
    t["sbxw_thread"] = "worker".into();
    hook(&app, &SandboxRef::parse("thread1").unwrap(), "s3", t);
    let thread = canvas_of(&app, "thread1", "Thread", 1);
    plain(&app, d.path(), crate::svc::jarvis_sandbox::NAME).await;
    let jarvis = canvas_of(&app, crate::svc::jarvis_sandbox::NAME, "Jarvis", 1);

    // The list.
    let r = get(&router, &token, "/r/leaders/lead1/canvas").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let list = r.json();
    assert_eq!(list["canvases"].as_array().unwrap().len(), 1, "{list}");
    let c = &list["canvases"][0];
    assert_eq!((c["id"].as_str(), c["shots"].as_u64(), c["rounds"].as_u64(), c["new_shots"].as_u64(), c["draft"].as_u64()), (Some(ours.as_str()), Some(2), Some(1), Some(2), Some(0)));
    assert!(!r.text().contains("someone@example.org"), "the title is masked: {}", r.text());
    for name in ["gone1", "thread1", crate::svc::jarvis_sandbox::NAME, "nosuch"] {
        let r = get(&router, &token, &format!("/r/leaders/{name}/canvas")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{name}");
    }

    // The manifest.
    let r = get(&router, &token, &format!("/r/canvas/{ours}")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let m = r.json();
    assert_eq!(m["sandbox"], "lead1");
    let shots = m["shots"].as_array().unwrap();
    assert_eq!(shots.iter().map(|s| s["title"].as_str().unwrap()).collect::<Vec<_>>(), ["Screen 0", "Screen 1"]);
    assert_eq!((shots[0]["width"].as_u64(), shots[0]["screen"].as_str(), shots[0]["round"].as_u64()), (Some(390), Some("home"), Some(1)));
    assert!(!r.text().contains("someone@example.org"));
    for absent in ["\"x\"", "\"moved\"", "\"bytes\"", "\"session\"", "\"sandbox_path\""] {
        assert!(!r.text().contains(absent), "{absent} in {}", r.text());
    }
    assert!(app.remote_journal(10).iter().any(|e| e.kind == Kind::CanvasRead));

    // The picture: by the manifest's name only.
    let file = shots[0]["file"].as_str().unwrap().to_string();
    let r = get(&router, &token, &format!("/r/canvas/{ours}/images/{file}")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store, private");
    assert_eq!(r.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(r.body, png(390, 844));
    let theirs = app.canvas_get(&stopped).unwrap().shots[0].file.clone();
    for forged in ["s-000000000000.png", "canvas.json", "..%2Fcanvas.json", "S-ABCDEF012345.png", &theirs, &format!("{file}x"), "a-000000000000.png"] {
        let r = get(&router, &token, &format!("/r/canvas/{ours}/images/{forged}")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{forged}: {}", r.text());
    }
    // An example picture the Mac added is not served to the phone.
    let with_asset = app.canvas_add_asset(&ours, "example.png", &png(10, 10), None).unwrap();
    let asset_file = with_asset.assets[0].file.clone();
    assert_eq!(get(&router, &token, &format!("/r/canvas/{ours}/images/{asset_file}")).await.status, StatusCode::NOT_FOUND);

    // Another sandbox's canvas, a thread's, Jarvis's, a malformed id: as unknown.
    for id in [&stopped, &thread, &jarvis] {
        let f = app.canvas_get(id).unwrap().shots[0].file.clone();
        refused_as_unknown(&router, &token, id, &f).await;
    }
    for bad in ["c-0123", "C-00000000000000FF", "x"] {
        assert_eq!(get(&router, &token, &format!("/r/canvas/{bad}")).await.status, StatusCode::NOT_FOUND);
    }
    // Nothing of another sandbox's canvas was changed.
    assert!(app.canvas_get(&stopped).unwrap().pins.is_empty());
}

#[tokio::test]
async fn remarks_go_through_the_macs_edit_with_its_limits() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    leader(&app, d.path(), "lead1").await;
    let id = canvas_of(&app, "lead1", "R", 2);
    let c = app.canvas_get(&id).unwrap();
    let (s0, s1) = (c.shots[0].id.clone(), c.shots[1].id.clone());

    // A pin and a note.
    let r = remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s0, "x": 42.5, "y": 10, "text": "write to someone@example.org" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let v = r.json();
    assert_eq!((v["n"].as_u64(), v["draft"].as_u64()), (Some(1), Some(1)));
    assert!(v["masked"].as_u64().unwrap() >= 1);
    assert_eq!((v["pins"][0]["x"].as_f64(), v["pins"][0]["y"].as_f64()), (Some(42.5), Some(10.0)));
    let stored = app.canvas_get(&id).unwrap();
    assert!(!stored.pins[0].text.contains("someone@example.org"), "masked before it is stored");
    assert!((stored.pins[0].fx - 0.425).abs() < 1e-6);
    let r = remark(&router, &token, &id, json!({ "op": "add", "kind": "note", "shot": s1, "text": "Needs a call to action" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!((r.json()["notes"][0]["shot"].as_str(), r.json()["draft"].as_u64()), (Some(s1.as_str()), Some(2)));
    let note = app.canvas_get(&id).unwrap().notes[0].clone();
    assert_eq!(note.on.as_deref(), Some(s1.as_str()));
    // The Mac's next edit (which knows nothing of `on`) keeps it.
    let mut mac_note = note.clone();
    mac_note.on = None;
    mac_note.x += 40.0;
    app.canvas_edit(&id, Edit { notes: Some(vec![mac_note]), ..Default::default() }).unwrap();
    assert_eq!(app.canvas_get(&id).unwrap().notes[0].on.as_deref(), Some(s1.as_str()));

    // Edit, then remove.
    let r = remark(&router, &token, &id, json!({ "op": "edit", "kind": "pin", "n": 1, "text": "Bigger", "x": 50, "y": 50 })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!((r.json()["pins"][0]["text"].as_str(), r.json()["pins"][0]["x"].as_f64()), (Some("Bigger"), Some(50.0)));
    let r = remark(&router, &token, &id, json!({ "op": "edit", "kind": "note", "n": 1, "text": "Calmer" })).await;
    assert_eq!(r.json()["notes"][0]["text"], "Calmer");
    let r = remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s1, "x": 0, "y": 100, "text": "x" })).await;
    assert_eq!(r.json()["n"], 2);
    let r = remark(&router, &token, &id, json!({ "op": "remove", "kind": "pin", "n": 2 })).await;
    assert_eq!((r.status, r.json()["pins"].as_array().unwrap().len()), (StatusCode::OK, 1));

    // Refusals.
    let bad = [
        json!({ "op": "add", "kind": "pin", "shot": s0, "x": 101, "y": 1, "text": "x" }),
        json!({ "op": "add", "kind": "pin", "shot": s0, "x": -1, "y": 1, "text": "x" }),
        json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "text": "x" }),
        json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "y": 1, "text": "   " }),
        json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "y": 1, "text": "x".repeat(canvas::MAX_NOTE + 1) }),
        json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "y": 1, "text": "a\u{0007}b" }),
        json!({ "op": "add", "kind": "note", "shot": s0, "x": 1, "y": 1, "text": "x" }),
        json!({ "op": "add", "kind": "note", "text": "x" }),
        json!({ "op": "edit", "kind": "pin", "text": "x" }),
    ];
    for b in bad {
        let r = remark(&router, &token, &id, b.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{b}: {}", r.text());
    }
    assert_eq!(remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "y": 1, "text": "x".repeat(canvas::MAX_NOTE) })).await.status, StatusCode::OK, "2,000 characters is the cap");
    for b in [json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "y": 1, "text": "x", "on": "s" }), json!({ "op": "move", "kind": "pin", "n": 1 }), json!({ "op": "add", "kind": "asset", "shot": s0, "text": "x" })] {
        let r = remark(&router, &token, &id, b.clone()).await;
        assert!(r.status.is_client_error() && r.status != StatusCode::NOT_FOUND, "{b}: {}", r.status);
    }
    for b in [json!({ "op": "add", "kind": "pin", "shot": "s-ffffffff", "x": 1, "y": 1, "text": "x" }), json!({ "op": "edit", "kind": "pin", "n": 99, "text": "x" }), json!({ "op": "remove", "kind": "note", "n": 99 })] {
        assert_eq!(remark(&router, &token, &id, b.clone()).await.status, StatusCode::NOT_FOUND, "{b}");
    }
    // Over the body cap.
    let r = remark(&router, &token, &id, json!({ "op": "add", "kind": "note", "shot": s0, "text": "é".repeat(9000) })).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // The canvas's own cap: 500 pins.
    let full: Vec<Pin> = (1..=canvas::MAX_PINS as u32).map(|n| Pin { n, on: s0.clone(), fx: 0.5, fy: 0.5, text: "p".into(), sent: None }).collect();
    app.canvas_edit(&id, Edit { pins: Some(full), ..Default::default() }).unwrap();
    let r = remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s0, "x": 1, "y": 1, "text": "one more" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    let kinds: Vec<Kind> = app.remote_journal(100).iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&Kind::CanvasRemark), "{kinds:?}");
    let journal = std::fs::read_to_string(app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    assert!(!journal.contains("Bigger") && !journal.contains("Calmer"), "{journal}");
}

#[tokio::test]
async fn a_round_sent_types_the_fixed_line_once_and_a_sent_remark_never_changes() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    let id = canvas_of(&app, "lead1", "R", 1);
    let s0 = app.canvas_get(&id).unwrap().shots[0].id.clone();
    assert_eq!(submit(&router, &token, &id).await.status, StatusCode::BAD_REQUEST, "nothing to send");
    remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s0, "x": 10, "y": 10, "text": "Move the button" })).await;
    let r = submit(&router, &token, &id).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["review"], 1);
    let line = canvas::ready_line(&id, 1);
    let got = typed(&file, b"\x1b[201~\r").await;
    let want = [b"\x1b[200~".as_slice(), line.as_bytes(), b"\x1b[201~\r"].concat();
    assert_eq!(got, want, "one paste, one Return: {:?}", String::from_utf8_lossy(&got));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read(&file).unwrap(), want, "typed once");
    let c = app.canvas_get(&id).unwrap();
    assert_eq!((c.reviews.len(), c.draft(), c.pins[0].sent), (1, 0, Some(1)));
    assert!(app.canvas_agent_read(&SandboxRef::parse("lead1").unwrap(), None).unwrap().contains("> Move the button"));
    assert!(app.remote_journal(10).iter().any(|e| e.kind == Kind::CanvasSubmit));
    // Sent: read only, and nothing new to send.
    assert_eq!(remark(&router, &token, &id, json!({ "op": "edit", "kind": "pin", "n": 1, "text": "x" })).await.status, StatusCode::NOT_FOUND);
    assert_eq!(submit(&router, &token, &id).await.status, StatusCode::BAD_REQUEST);
    let m = get(&router, &token, &format!("/r/canvas/{id}")).await.json();
    assert_eq!((m["sent"]["pins"].as_u64(), m["pins"].as_array().unwrap().len(), m["reviews"][0]["read"].as_bool()), (Some(1), 0, Some(true)));
    // The next pin is number 2.
    let r = remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s0, "x": 10, "y": 10, "text": "again" })).await;
    assert_eq!(r.json()["n"], 2);
    // An example picture added on the Mac: the round is the Mac's to send.
    app.canvas_add_asset(&id, "x.png", &png(10, 10), None).unwrap();
    let r = submit(&router, &token, &id).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("examples")));
}

#[tokio::test]
async fn a_round_is_refused_while_the_session_waits_for_the_person_or_is_read_only() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    let id = canvas_of(&app, "lead1", "R", 1);
    let s0 = app.canvas_get(&id).unwrap().shots[0].id.clone();
    remark(&router, &token, &id, json!({ "op": "add", "kind": "pin", "shot": s0, "x": 10, "y": 10, "text": "x" })).await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which?", "options": [{"label": "A"}, {"label": "B"}]}]}}));
    let r = submit(&router, &token, &id).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("needs_mac")), "{}", r.text());
    assert!(app.canvas_get(&id).unwrap().reviews.is_empty(), "nothing recorded");
    assert_eq!(app.remote_journal(10).last().map(|e| e.kind), Some(Kind::Refused));
    hook(&app, &sb, "s1", json!({"hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion"}));
    // A permission prompt while it works: still refused.
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "go"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}));
    let r = submit(&router, &token, &id).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("needs_mac")), "{}", r.text());
    // A second session: read only from the phone.
    hook(&app, &sb, "s2", json!({"hook_event_name": "SessionStart"}));
    let r = submit(&router, &token, &id).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert!(r.json()["error"].as_str().unwrap().contains("read only"), "{}", r.text());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "nothing typed");
    assert!(app.canvas_get(&id).unwrap().reviews.is_empty());
}

#[tokio::test]
async fn a_working_session_takes_the_round_as_it_takes_a_message() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    let id = canvas_of(&app, "lead1", "R", 1);
    let s0 = app.canvas_get(&id).unwrap().shots[0].id.clone();
    remark(&router, &token, &id, json!({ "op": "add", "kind": "note", "shot": s0, "text": "x" })).await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "build it"}));
    let r = submit(&router, &token, &id).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(typed(&file, b"\x1b[201~\r").await.ends_with(b"\x1b[201~\r"));
}

#[tokio::test]
async fn the_canvas_routes_need_the_cookie_and_the_origin() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    leader(&app, d.path(), "lead1").await;
    let id = canvas_of(&app, "lead1", "R", 1);
    assert_eq!(call(&router, "GET", &format!("/r/canvas/{id}"), None, json!({})).await.status, StatusCode::UNAUTHORIZED);
    // A write without the page's Origin.
    let r = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/r/canvas/{id}/submit"))
        .header(header::HOST, super::remote_tests::TN)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("{COOKIE}={token}"))
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(send(&router, r).await.status, StatusCode::FORBIDDEN);
    let r = req("GET", &format!("/r/canvas/{id}")).header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    assert_eq!(send(&router, r).await.headers[header::CACHE_CONTROL], "no-store");
}

#[test]
fn a_route_cannot_weaken_no_store() {
    for (set, want) in [
        (Some("public, max-age=60"), "no-store"),
        (Some("private, max-age=31536000, immutable"), "no-store"),
        (Some("max-age=0, no-store"), "no-store"),
        (None, "no-store"),
        (Some("no-store, private"), "no-store, private"),
    ] {
        let mut res = "x".into_response();
        if let Some(v) = set {
            res.headers_mut().insert(header::CACHE_CONTROL, v.parse().unwrap());
        }
        let out = with_headers(res, true);
        assert_eq!(out.headers()[header::CACHE_CONTROL], want, "{set:?}");
    }
}
