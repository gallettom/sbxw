//! Second pass of the independent review of the phone's lots, after the
//! authors' fixes (`.sbxw-artifacts/reports/phone-review.md`, "Second pass").
//! Test file only. Helpers copied from `phone_review_tests.rs`.
//!
//! Wire it on a checkout of `sbxw/phone-approve` (or its merge) with
//! `#[cfg(test)] mod phone_review2_tests;` in `crates/daemon/src/http/mod.rs`, then
//! `CARGO_TARGET_DIR=$PWD/target CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1 -p sbxw-daemon phone_review2`.
//!
//! Tests named `finding_*` PASS while the weakness exists: each asserts what
//! the report objects to, and its fix should turn it around (the message says
//! "FIXED?"). Tests named `checked_*` pin what held.

#![allow(unused_imports, dead_code)]

use super::remote::remote_router;
use crate::app::App;
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use sbxw_core::plans::{PlanState, PlanThread, ThreadState, Variant};
use sbxw_core::remote_access::cbor;
use sbxw_core::remote_access::level2;
use sbxw_core::remote_access::webauthn::soft::{Authenticator, Twist};
use sbxw_core::remote_access::webauthn::{self as wa, b64url_decode, b64url_encode};
use sbxw_core::remote_access::COOKIE;
use sbxw_sbx::runner::Output;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

const TN: &str = "sbxw.tail1.ts.net";

#[derive(Default)]
struct SbxState {
    sandboxes: Vec<(String, String, String)>,
    approvals: Vec<Value>,
    /// When set: the workspace repo in which a "thread agent" commits to
    /// `sbxw/alpha` at the moment the merge script is invoked.
    late_commit_in: Option<PathBuf>,
    /// Run the real merge script for every merge.
    real_merge: bool,
    /// Commit once to this branch at the next merge call, before it runs.
    move_on_next_merge: Option<(PathBuf, String)>,
}

struct World {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    fake: Arc<sbxw_sbx::runner::Fake>,
    sbx: Arc<Mutex<SbxState>>,
    router: Router,
}

fn world() -> World {
    let sbx = Arc::new(Mutex::new(SbxState::default()));
    let st = sbx.clone();
    let (dir, app, fake) = crate::testing::app_with(move |a: &[String]| {
        let mut st = st.lock().unwrap();
        if a.iter().any(|x| x == "sbxw-thread-merge") {
            if let Some((ws, br)) = st.move_on_next_merge.take() {
                // Another thread's agent commits while an earlier merge runs.
                git(&ws, &["checkout", "-q", &br]);
                std::fs::write(ws.join("moved.rs"), "moved\n").unwrap();
                git(&ws, &["add", "."]);
                git(&ws, &["commit", "-qm", "moved meanwhile"]);
                git(&ws, &["checkout", "-q", "main"]);
            }
            if st.late_commit_in.is_none() && st.real_merge {
                let at = a.iter().position(|x| x == "sbxw-thread-merge").unwrap();
                let out = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(&a[at - 1])
                    .args(&a[at..])
                    .envs([("GIT_AUTHOR_NAME", "t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_EMAIL", "t@t")])
                    .output()
                    .unwrap();
                return Output::ok(&String::from_utf8_lossy(&out.stdout));
            }
            if let Some(ws) = &st.late_commit_in {
                // The thread's agent commits while sbxw starts the merge.
                git(ws, &["checkout", "-q", "sbxw/alpha"]);
                std::fs::write(ws.join("late.rs"), "NOT_SHOWN_ON_THE_PHONE\n").unwrap();
                git(ws, &["add", "."]);
                git(ws, &["commit", "-qm", "late work"]);
                git(ws, &["checkout", "-q", "main"]);
                // Then the real merge script runs, as in the sandbox (P2 fix).
                let at = a.iter().position(|x| x == "sbxw-thread-merge").unwrap();
                let out = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(&a[at - 1])
                    .args(&a[at..])
                    .envs([("GIT_AUTHOR_NAME", "t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_EMAIL", "t@t")])
                    .output()
                    .unwrap();
                return Output::ok(&String::from_utf8_lossy(&out.stdout));
            }
        }
        match (a.first().map(String::as_str), a.get(1).map(String::as_str), a.get(2).map(String::as_str)) {
            (Some("ls"), _, _) => {
                let rows: Vec<Value> = st.sandboxes.iter().map(|(n, s, c)| json!({ "name": n, "agent": "claude", "status": s, "created_at": c })).collect();
                Output::ok(&Value::Array(rows).to_string())
            }
            (Some("policy"), Some("approval"), Some("ls")) => Output::ok(&Value::Array(st.approvals.clone()).to_string()),
            _ => crate::testing::sbx_answer(a),
        }
    });
    app.remote_test_on(TN);
    app.remote_approvals_enable().unwrap();
    let router = remote_router(app.clone());
    World { _dir: dir, app, fake, sbx, router }
}

struct Reply {
    status: StatusCode,
    #[allow(dead_code)]
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn send(router: &Router, r: Request<Body>) -> Reply {
    let res = router.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = match tokio::time::timeout(Duration::from_secs(3), axum::body::to_bytes(res.into_body(), 16 << 20)).await {
        Ok(b) => b.unwrap().to_vec(),
        Err(_) => b"(still streaming)".to_vec(),
    };
    Reply { status, headers, body }
}

fn request(method: &str, path: &str, cookie: Option<&str>, body: Value) -> Request<Body> {
    let m: Method = method.parse().unwrap();
    let mut b = Request::builder().method(m.clone()).uri(path).header(header::HOST, TN);
    if m != Method::GET {
        b = b.header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(t) = cookie {
        b = b.header(header::COOKIE, format!("{COOKIE}={t}"));
    }
    let body = if m == Method::GET { Body::empty() } else { Body::from(body.to_string()) };
    b.body(body).unwrap()
}

async fn call(router: &Router, method: &str, path: &str, cookie: Option<&str>, body: Value) -> Reply {
    send(router, request(method, path, cookie, body)).await
}

struct Phone {
    token: String,
    id: String,
    auth: Authenticator,
    /// The request id `/r/passkey/start` gave THIS client (P1 fix).
    rid: std::cell::RefCell<String>,
}

async fn pair(w: &World, name: &str) -> Phone {
    let code = w.app.remote_pairing_code().unwrap().code;
    let r = call(&w.router, "POST", "/r/pair/start", None, json!({ "code": code, "device_name": name })).await;
    let id = r.json()["request_id"].as_str().unwrap().to_string();
    let check = r.json()["check"].as_str().unwrap().to_string();
    w.app.remote_pairing_approve(&id, &check).unwrap();
    let r = call(&w.router, "GET", &format!("/r/pair/status?request_id={id}"), None, json!({})).await;
    let cookie = r.headers[header::SET_COOKIE].to_str().unwrap().to_string();
    let token = cookie[COOKIE.len() + 1..].split(';').next().unwrap().to_string();
    let me = call(&w.router, "GET", "/r/me", Some(&token), json!({})).await;
    Phone { token, id: me.json()["device"]["id"].as_str().unwrap().to_string(), auth: Authenticator::new(), rid: Default::default() }
}

async fn register_with(w: &World, p: &Phone) -> Reply {
    let rid = p.rid.borrow().clone();
    register_for(w, p, &rid).await
}

/// Registration as a client that holds `rid` (the id `/r/passkey/start`
/// gave it): after the P1 fix, options and register carry it.
async fn register_for(w: &World, p: &Phone, rid: &str) -> Reply {
    let r = call(&w.router, "POST", "/r/passkey/options", Some(&p.token), json!({ "request_id": rid })).await;
    if r.status != StatusCode::OK {
        return r;
    }
    let ch = b64url_decode(r.json()["public_key"]["challenge"].as_str().unwrap(), 64).unwrap();
    let (id, cd, att) = p.auth.register(TN, &ch, &Twist::default());
    call(&w.router, "POST", "/r/passkey/register", Some(&p.token), json!({ "request_id": rid, "id": b64url_encode(&id), "client_data_json": b64url_encode(&cd), "attestation_object": b64url_encode(&att) })).await
}

async fn enrolled(w: &World, name: &str) -> Phone {
    let p = pair(w, name).await;
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&p.token), json!({})).await;
    *p.rid.borrow_mut() = r.json()["request_id"].as_str().unwrap().to_string();
    w.app.remote_passkey_allow(&p.id, r.json()["check"].as_str().unwrap()).unwrap();
    assert_eq!(register_with(w, &p).await.status, StatusCode::OK);
    p
}

fn pending_plan(w: &World, sandbox: &str, goal: &str) -> String {
    let id = sbxw_core::ids::plan_id();
    let t = PlanThread { name: "alpha".into(), goal: goal.into(), scope: "crates/x".into(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket: None };
    w.app.plans.lock().unwrap().propose(id.clone(), sandbox, "Do the thing", vec![t], Variant::Split, "independent work", Some("lead"), 1).unwrap();
    id
}

async fn challenge(w: &World, p: &Phone, kind: &str, target: &str, detail: &str) -> Vec<u8> {
    let d = call(&w.router, "GET", detail, Some(&p.token), json!({})).await;
    assert_eq!(d.status, StatusCode::OK, "{}", d.text());
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&p.token), json!({ "kind": kind, "target": target, "digest": d.json()["digest"] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    b64url_decode(r.json()["public_key"]["challenge"].as_str().unwrap(), 64).unwrap()
}

fn assertion(p: &mut Phone, ch: &[u8]) -> Value {
    let (id, cd, ad, sig) = p.auth.assert(TN, ch, &Twist::default());
    json!({ "id": b64url_encode(&id), "client_data_json": b64url_encode(&cd), "authenticator_data": b64url_encode(&ad), "signature": b64url_encode(&sig) })
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}


// ── Second pass ──────────────────────────────────────────────────────────

/// A copy of the device cookie (P1's attacker) can no longer spend the
/// grant, but it shares the device's ONE request and its start limit: each
/// start voids the person's request, and five of them in ten minutes leave
/// the person's own phone at 429 for the rest of the window.
#[tokio::test]
async fn finding_q1_a_copy_voids_the_persons_request_and_spends_the_start_limit() {
    let w = world();
    let person = pair(&w, "iPhone").await;
    let copy_token = person.token.clone();
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&person.token), json!({})).await;
    let rid = r.json()["request_id"].as_str().unwrap().to_string();
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&copy_token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    let st = call(&w.router, "GET", &format!("/r/passkey/status?request_id={rid}"), Some(&person.token), json!({})).await;
    assert_eq!(st.json()["state"], "none", "FIXED? the copy's start voided the person's request");
    for _ in 0..3 {
        call(&w.router, "POST", "/r/passkey/start", Some(&copy_token), json!({})).await;
    }
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&person.token), json!({})).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "FIXED? the person's start is limited by the copy's: {}", r.text());
}

/// The same copy, once the person has a passkey: its challenges replace the
/// person's (one per device), and its failed assertions count against the
/// device, so five of them lock the person's approvals for a minute (twenty,
/// for an hour).
#[tokio::test]
async fn finding_q2_a_copy_voids_the_persons_challenge_and_locks_the_device() {
    let w = world();
    let mut person = enrolled(&w, "iPhone").await;
    let mut copy = Phone { token: person.token.clone(), id: person.id.clone(), auth: Authenticator::new(), rid: Default::default() };
    let id = pending_plan(&w, "myapp", "g");
    let path = format!("/r/plans/{id}/approve");
    // The person's challenge, then the copy's: the person's Face ID fails.
    let ch = challenge(&w, &person, "plan_approve", &id, &format!("/r/plans/{id}")).await;
    let _ = challenge(&w, &copy, "plan_approve", &id, &format!("/r/plans/{id}")).await;
    let a = assertion(&mut person, &ch);
    let r = call(&w.router, "POST", &path, Some(&person.token), json!({ "assertion": a })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "FIXED? the copy's challenge no longer voids the person's: {}", r.text());
    // Four more failures by the copy: the device is locked.
    for _ in 0..4 {
        let ch = challenge(&w, &copy, "plan_approve", &id, &format!("/r/plans/{id}")).await;
        let a = assertion(&mut copy, &ch);
        call(&w.router, "POST", &path, Some(&copy.token), json!({ "assertion": a })).await;
    }
    let ch = challenge(&w, &person, "plan_approve", &id, &format!("/r/plans/{id}")).await;
    let a = assertion(&mut person, &ch);
    let r = call(&w.router, "POST", &path, Some(&person.token), json!({ "assertion": a })).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "FIXED? {}", r.text());
    assert_eq!(w.app.plan(&id).unwrap().state, PlanState::Pending);
}

/// The stream cap counts per device: a copy that opens four streams closes
/// the person's (`bye` too_many).
#[tokio::test]
async fn finding_q3_a_copy_closes_the_persons_event_stream() {
    use futures_util::StreamExt;
    let w = world();
    let person = pair(&w, "iPhone").await;
    let mine = w.router.clone().oneshot(request("GET", "/r/events", Some(&person.token), json!({}))).await.unwrap();
    let mut held = vec![];
    for _ in 0..4 {
        held.push(w.router.clone().oneshot(request("GET", "/r/events", Some(&person.token), json!({}))).await.unwrap());
    }
    let mut body = mine.into_body().into_data_stream();
    let mut seen = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(Ok(b)) = body.next().await {
            seen.push_str(&String::from_utf8_lossy(&b));
            if seen.contains("too_many") {
                break;
            }
        }
    })
    .await;
    assert!(seen.contains("event: bye") && seen.contains("too_many"), "FIXED? {seen}");
    drop(held);
}

fn two_thread_merge(w: &World, ws: &std::path::Path) -> (String, String, String) {
    std::fs::create_dir_all(ws).unwrap();
    git(ws, &["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("a.txt"), "one\n").unwrap();
    git(ws, &["add", "."]);
    git(ws, &["commit", "-qm", "base"]);
    for (t, f) in [("alpha", "x.rs"), ("beta", "y.rs")] {
        git(ws, &["checkout", "-q", "-b", &format!("sbxw/{t}"), "main"]);
        std::fs::write(ws.join(f), format!("{t}\n")).unwrap();
        git(ws, &["add", "."]);
        git(ws, &["commit", "-qm", t]);
    }
    git(ws, &["checkout", "-q", "main"]);
    let sb = sbxw_core::SandboxRef::parse("myapp").unwrap();
    w.app.store.update(&sb, |r| r.workspace = Some(ws.to_path_buf())).unwrap();
    let id = sbxw_core::ids::plan_id();
    let th = |n: &str| PlanThread { name: n.into(), goal: "g".into(), scope: String::new(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket: None };
    let mut book = w.app.plans.lock().unwrap();
    book.propose(id.clone(), "myapp", "two", vec![th("alpha"), th("beta")], Variant::Split, "independent", Some("lead"), 1).unwrap();
    let threads = book.get(&id).unwrap().threads.clone();
    book.approve(&id, threads, None, None, 2).unwrap();
    book.set_thread("myapp", "alpha", ThreadState::Done, "done", false, 3).unwrap();
    book.set_thread("myapp", "beta", ThreadState::Done, "done", false, 3).unwrap();
    book.propose_merge(&id, "myapp", vec!["alpha".into(), "beta".into()], "both", vec![], 4).unwrap();
    drop(book);
    (id, git(ws, &["rev-parse", "sbxw/alpha"]), git(ws, &["rev-parse", "sbxw/beta"]))
}

/// Two threads in one repo, approved from the phone, with the real script:
/// the second pinned merge finds the base the first one made, and both
/// commits merged are exactly the ones shown.
#[tokio::test]
async fn checked_two_pinned_merges_in_one_repo_take_exactly_what_was_shown() {
    let w = world();
    w.sbx.lock().unwrap().real_merge = true;
    let mut p = enrolled(&w, "phone").await;
    let ws = w._dir.path().join("ws2");
    let (id, a_tip, b_tip) = two_thread_merge(&w, &ws);
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}")).await;
    let a = assertion(&mut p, &ch);
    let r = call(&w.router, "POST", &format!("/r/merges/{id}/merge"), Some(&p.token), json!({ "assertion": a })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(git(&ws, &["rev-parse", "HEAD^2"]), b_tip);
    assert_eq!(git(&ws, &["rev-parse", "HEAD^1^2"]), a_tip);
}

/// The second branch moves while the first merges: the first is merged, the
/// second stops as "changed", and the phone is told which one.
#[tokio::test]
async fn checked_a_branch_moved_during_an_earlier_merge_stops_the_merge_there() {
    let w = world();
    w.sbx.lock().unwrap().real_merge = true;
    let mut p = enrolled(&w, "phone").await;
    let ws = w._dir.path().join("ws3");
    let (id, a_tip, b_tip) = two_thread_merge(&w, &ws);
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}")).await;
    w.sbx.lock().unwrap().move_on_next_merge = Some((ws.clone(), "sbxw/beta".into()));
    let a = assertion(&mut p, &ch);
    let r = call(&w.router, "POST", &format!("/r/merges/{id}/merge"), Some(&p.token), json!({ "assertion": a })).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("changed")), "{}", r.text());
    assert!(r.text().contains("sbxw/beta") && r.text().contains("alpha"), "{}", r.text());
    assert_eq!(git(&ws, &["rev-parse", "HEAD^2"]), a_tip, "alpha merged");
    assert_ne!(git(&ws, &["rev-parse", "sbxw/beta"]), b_tip);
    assert!(git(&ws, &["log", "--format=%s", "main"]).lines().all(|l| l != "moved meanwhile" && l != "beta"), "beta not merged");
}

/// The desktop's merge, with the same real script: by name, no pin, so a
/// moved branch is merged as it is (as before the fix, by design).
#[tokio::test]
async fn checked_the_desktop_merge_is_still_by_name() {
    let w = world();
    w.sbx.lock().unwrap().real_merge = true;
    let ws = w._dir.path().join("ws4");
    let (id, _a, b_tip) = two_thread_merge(&w, &ws);
    w.sbx.lock().unwrap().move_on_next_merge = Some((ws.clone(), "sbxw/beta".into()));
    let m = w.app.plan_merge(&id, false, None).await.unwrap();
    assert!(m.results.iter().all(|r| r.ok), "{:?}", m.results);
    let moved = git(&ws, &["rev-parse", "sbxw/beta"]);
    assert_ne!(moved, b_tip);
    assert_eq!(git(&ws, &["rev-parse", "HEAD^2"]), moved, "the desktop merges the branch as it is");
    let calls: Vec<String> = w.fake.log().into_iter().filter(|c| c.contains("sbxw-thread-merge")).collect();
    assert!(calls.iter().all(|c| c.ends_with("=alpha") || c.ends_with("=beta")), "{calls:?}");
}
