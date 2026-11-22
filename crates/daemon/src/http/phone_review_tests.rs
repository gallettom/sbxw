//! Independent review of the phone's three new lots (upload, Kanban, level 2),
//! `.sbxw-artifacts/reports/phone-review.md`. Test file only.
//!
//! Wire it on a checkout of `sbxw/phone-approve` (or its merge) with
//! `#[cfg(test)] mod phone_review_tests;` in `crates/daemon/src/http/mod.rs`, then
//! `CARGO_TARGET_DIR=$PWD/target CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1 -p sbxw-daemon phone_review`.
//!
//! Tests named `finding_*` PASS while the weakness exists: each asserts what
//! the report objects to, and its fix should turn it around (the message says
//! "FIXED?"). Tests named `checked_*` pin what held.

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
        let st = st.lock().unwrap();
        if a.iter().any(|x| x == "sbxw-thread-merge") {
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

// ── P1: the registration grant is the device id's, not the phone's ──────

/// The person starts Face ID on their phone, types its digits at the Mac.
/// A second phone holding a copy of the same device cookie (the threat the
/// six digits exist for, API-level2.md section 4 step 2) watches
/// `/r/passkey/status` until it says `granted`, then spends the person's
/// grant with ITS OWN authenticator, before the person's Face ID sheet is
/// even done. It can then approve with its own passkey.
#[tokio::test]
async fn finding_p1_a_copy_of_the_cookie_spends_the_persons_grant_and_approves() {
    let w = world();
    let person = pair(&w, "iPhone").await;
    let mut thief = Phone { token: person.token.clone(), id: person.id.clone(), auth: Authenticator::new(), rid: Default::default() };
    // The person's phone asks; the person types the digits only that phone shows.
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&person.token), json!({})).await;
    let digits = r.json()["check"].as_str().unwrap().to_string();
    *person.rid.borrow_mut() = r.json()["request_id"].as_str().unwrap().to_string();
    w.app.remote_passkey_allow(&person.id, &digits).unwrap();
    // FIXED (P1): the copy, without the request id only the person's phone
    // was given, sees nothing and can spend nothing.
    let st = call(&w.router, "GET", "/r/passkey/status", Some(&thief.token), json!({})).await;
    assert_eq!(st.json()["state"], "none", "the copy learns nothing of the grant");
    let r = register_with(&w, &thief).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("no_grant")), "{}", r.text());
    // The person's own registration still works: nothing was burned.
    let r = register_with(&w, &person).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    // And the copy cannot approve: its key is not the device's passkey.
    let id = pending_plan(&w, "myapp", "anything");
    let ch = challenge(&w, &thief, "plan_approve", &id, &format!("/r/plans/{id}")).await;
    let a = assertion(&mut thief, &ch);
    let r = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&thief.token), json!({ "assertion": a })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text());
    assert_eq!(w.app.plan(&id).unwrap().state, PlanState::Pending);
}

/// What does hold: a copy that starts its OWN request after the person's
/// voids it, so the digits the person types (from the real phone) no longer
/// match, and three wrong tries refuse it.
#[tokio::test]
async fn checked_the_digits_bind_the_grant_at_the_moment_it_is_made() {
    let w = world();
    let person = pair(&w, "iPhone").await;
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&person.token), json!({})).await;
    let digits = r.json()["check"].as_str().unwrap().to_string();
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&person.token), json!({})).await;
    let thief_digits = r.json()["check"].as_str().unwrap().to_string();
    if digits != thief_digits {
        assert!(w.app.remote_passkey_allow(&person.id, &digits).is_err(), "the person's digits belong to a voided request");
    }
}

// ── P2: a merge takes each branch as it is when the script runs ──────────

/// The phone approves a merge whose digest pins `sbxw/alpha`'s tip. Between
/// the daemon's last digest check and the merge script (`plan_merge` starts
/// the sandbox and merges the threads one by one first), the thread's agent
/// commits once more. Nothing notices: the script is called with the thread
/// NAME and merges the branch as it then is (`scripts.rs`
/// `THREAD_MERGE_SCRIPT`: `git merge --no-ff "$br"`).
#[tokio::test]
async fn finding_p2_a_merge_runs_on_a_branch_that_moved_after_the_last_check() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let ws = w._dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("a.txt"), "one\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "base"]);
    git(&ws, &["branch", "sbxw/alpha"]);
    git(&ws, &["checkout", "-q", "sbxw/alpha"]);
    std::fs::write(ws.join("b.rs"), "shown\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "thread work"]);
    git(&ws, &["checkout", "-q", "main"]);
    let shown_tip = git(&ws, &["rev-parse", "sbxw/alpha"]);
    let sb = sbxw_core::SandboxRef::parse("myapp").unwrap();
    w.app.store.update(&sb, |r| r.workspace = Some(ws.clone())).unwrap();
    let id = pending_plan(&w, "myapp", "merge me");
    {
        let mut book = w.app.plans.lock().unwrap();
        let threads = book.get(&id).unwrap().threads.clone();
        book.approve(&id, threads, None, None, 2).unwrap();
        book.set_thread("myapp", "alpha", ThreadState::Done, "done", false, 3).unwrap();
        book.propose_merge(&id, "myapp", vec!["alpha".into()], "Merge alpha", vec![], 4).unwrap();
    }
    let d = call(&w.router, "GET", &format!("/r/merges/{id}"), Some(&p.token), json!({})).await;
    assert_eq!(d.json()["merge"]["branches"][0]["tip"].as_str().unwrap(), &shown_tip[..7]);
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}")).await;
    w.sbx.lock().unwrap().late_commit_in = Some(ws.clone());
    let a = assertion(&mut p, &ch);
    let r = call(&w.router, "POST", &format!("/r/merges/{id}/merge"), Some(&p.token), json!({ "assertion": a })).await;
    // FIXED (P2): the merge is pinned to the commit shown; the late commit
    // makes it stop as "changed", and nothing is merged.
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("changed")), "{}", r.text());
    assert!(r.json()["error"].as_str().unwrap().contains("Nothing was merged"), "{}", r.text());
    let merge_call = w.fake.log().into_iter().find(|c| c.contains("sbxw-thread-merge")).expect("the merge script ran");
    assert!(merge_call.contains(&shown_tip), "the merge names the commit shown: {merge_call}");
    assert_ne!(git(&ws, &["rev-parse", "sbxw/alpha"]), shown_tip, "the branch did move");
    assert!(!ws.join("late.rs").exists(), "the late commit was not merged into main");
    assert_eq!(git(&ws, &["log", "-1", "--format=%s", "main"]), "base", "main untouched");
}

// ── P3: each open event stream runs sbx on its own ───────────────────────

/// While level 2 is on, every `/r/events` stream starts its own badge task,
/// and each look at the counts runs `sbx ls` and `sbx policy approval ls`
/// (at connect, every 15 s, and on every plan event). Nothing caps the
/// streams of one device beyond its 600 requests a minute.
#[tokio::test]
async fn finding_p3_every_event_stream_runs_sbx_itself() {
    let w = world();
    let p = pair(&w, "phone").await;
    let before = w.fake.log().len();
    let mut held = vec![];
    for _ in 0..25 {
        let res = w.router.clone().oneshot(request("GET", "/r/events", Some(&p.token), json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        held.push(res);
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let calls: Vec<String> = w.fake.log()[before..].to_vec();
    let lists = calls.iter().filter(|c| c.starts_with("policy approval ls")).count();
    // FIXED (P3): one shared count for every stream, refreshed at most every 5 s.
    assert!(lists <= 1, "{lists} `policy approval ls` for 25 streams");
    // And at most 4 streams per device: the oldest ends with bye too_many.
    use futures_util::StreamExt;
    let mut first = held.remove(0).into_body().into_data_stream();
    let mut seen = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(Ok(b)) = first.next().await {
            seen.push_str(&String::from_utf8_lossy(&b));
        }
    })
    .await;
    assert!(seen.contains("event: bye") && seen.contains("too_many"), "{seen}");
    assert_eq!(w.app.remote_access.lock().unwrap().level2.streams.get(&p.id).map(|l| l.len()), Some(4));
    drop(held);
}

// ── P4: how network options are read ─────────────────────────────────────

/// `phone_option` matched substrings: "unblock" and "do not block" read as a
/// refusal, "deny" also matched a lasting deny-list entry.
#[test]
fn finding_p4_phone_option_reads_substrings() {
    // FIXED (P4), kept in round 4: every option sbx offers reaches the phone
    // (behind Face ID), and its wording goes by its exact id: only `dismiss`
    // reads as leaving no rule; no word inside an id or label counts.
    for (id, label) in [("unblock", "Unblock this host"), ("allow", "Allow (do not block)"), ("denylist", "Add to the denylist"), ("allow-always", "Always allow"), ("allow", "Allow"), ("x", "Dismiss")] {
        assert!(level2::option_meaning(id, label, "s", "h").lasting, "{id:?} {label:?}");
    }
    assert!(!level2::option_meaning("dismiss", "Dismiss", "s", "h").lasting);
}

// ── P5: the Kanban's `[path]` filter ─────────────────────────────────────

#[test]
fn finding_p5_the_path_filter_misses_paths_inside_a_word() {
    let s = super::remote_kanban::scrub;
    // FIXED (P5): a path that starts inside a word is cut from there.
    for (raw, leaked) in [
        ("open file:///Users/someone/.ssh/id_rsa", "/Users/someone"),
        ("path=/Users/someone/secret", "/Users/someone"),
        ("see *~/notes/plan.md*", "~/notes"),
        ("at \\\\server\\share\\x", "\\\\server"),
        ("in $HOME/.aws/credentials", "/.aws"),
    ] {
        assert!(!s(raw).contains(leaked), "{raw:?} -> {:?}", s(raw));
    }
    // What holds.
    assert_eq!(s("fix /api route"), "fix [path] route");
    assert_eq!(s("(\"/etc/hosts\")"), "(\"[path]");
}

// ── Checked: parsers under hostile bytes ─────────────────────────────────

/// xorshift64, fixed seed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn mutate(r: &mut Rng, src: &[u8]) -> Vec<u8> {
    let mut b = src.to_vec();
    for _ in 0..1 + r.next() % 4 {
        let n = b.len().max(1);
        match r.next() % 6 {
            0 if !b.is_empty() => {
                let i = (r.next() as usize) % n;
                b[i] ^= 1 << (r.next() % 8);
            }
            1 if !b.is_empty() => {
                let i = (r.next() as usize) % n;
                b[i] = [0x00, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1f, 0x5f, 0x7f, 0x9f, 0xbf, 0xff][(r.next() % 12) as usize];
            }
            2 if !b.is_empty() => {
                let i = (r.next() as usize) % n;
                b.truncate(i);
            }
            3 => {
                let i = (r.next() as usize) % (b.len() + 1);
                b.insert(i, r.next() as u8);
            }
            4 if b.len() > 2 => {
                let i = (r.next() as usize) % (b.len() - 1);
                let j = i + 1 + (r.next() as usize) % (b.len() - i - 1);
                let chunk: Vec<u8> = b[i..j].to_vec();
                b.splice(i..i, chunk);
            }
            _ => b.extend(std::iter::repeat_n(0xa1, (r.next() % 8) as usize)),
        }
    }
    b
}

/// 40,000 mutations of a real attestation object, its authenticatorData and
/// an assertion's authenticatorData: no panic, and whatever passes is still
/// exactly the registered credential (a mutation that keeps a registration
/// valid can only touch bytes the RP does not read: none here but the AAGUID
/// and the counter).
#[test]
fn checked_hostile_cbor_and_authenticator_data_never_panic_or_change_the_key() {
    let auth = Authenticator::new();
    let ch = [7u8; 32];
    let (raw_id, cd, att) = auth.register(TN, &ch, &Twist::default());
    let good = wa::verify_registration(TN, &ch, &raw_id, &cd, &att).expect("the good one passes");
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let mut passed = 0;
    for _ in 0..20_000 {
        let m = mutate(&mut r, &att);
        let _ = cbor::decode_exact(&m);
        let _ = cbor::decode_prefix(&m);
        if let Ok(n) = wa::verify_registration(TN, &ch, &raw_id, &cd, &m) {
            passed += 1;
            assert_eq!((n.id.as_slice(), n.public_key.as_slice()), (good.id.as_slice(), good.public_key.as_slice()), "a mutated attestation yielded another key");
        }
    }
    let ad = auth.auth_data(TN, wa::FLAG_UP | wa::FLAG_UV, 1, None);
    for _ in 0..20_000 {
        let m = mutate(&mut r, &ad);
        let _ = wa::parse_auth_data(&m);
    }
    // Deep nesting, huge counts and lengths: refused at once.
    for bad in [vec![0x81; 10_000], vec![0xa1; 10_000], vec![0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], vec![0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], vec![0xbb, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]] {
        assert!(cbor::decode_exact(&bad).is_err());
    }
    // Duplicate key written two ways ({1: 1, 0x18 0x01: 2}).
    assert!(cbor::decode_exact(&[0xa2, 0x01, 0x01, 0x18, 0x01, 0x02]).is_err(), "a non-canonical duplicate key");
    let _ = passed;
}

/// clientDataJSON: a challenge that matches only after normalisation
/// (padding, standard alphabet), a duplicate `type`, `crossOrigin` as a
/// string, an origin with a trailing slash or a port: all refused.
#[test]
fn checked_client_data_is_matched_exactly() {
    let ch = [0xfbu8; 32];
    let o = wa::origin_of(TN);
    let enc = b64url_encode(&ch);
    let std_alphabet = enc.replace('-', "+").replace('_', "/");
    let cases = [
        format!(r#"{{"type":"webauthn.get","challenge":"{enc}=","origin":"{o}"}}"#),
        format!(r#"{{"type":"webauthn.get","challenge":"{std_alphabet}","origin":"{o}"}}"#),
        format!(r#"{{"type":"webauthn.get","challenge":"{enc}","origin":"{o}/"}}"#),
        format!(r#"{{"type":"webauthn.get","challenge":"{enc}","origin":"{o}:443"}}"#),
        format!(r#"{{"type":"webauthn.get","challenge":"{enc}","origin":"{o}","crossOrigin":"false"}}"#),
        format!(r#"{{"type":"webauthn.get","challenge":"{enc}","origin":"{o}","topOrigin":"{o}"}}"#),
        format!(r#"{{"type":"webauthn.create","challenge":"{enc}","origin":"{o}"}}"#),
        format!(r#"{{"type":"webauthn.get","challenge":"{enc}","origin":"HTTPS://{}"}}"#, TN.to_uppercase()),
    ];
    for c in cases {
        assert!(wa::check_client_data(c.as_bytes(), "webauthn.get", &ch, &o).is_err(), "{c}");
    }
    // NOTE: serde_json keeps the LAST of duplicate keys; harmless, since only
    // the browser writes clientDataJSON and the signature covers its bytes.
    let dup = format!(r#"{{"type":"webauthn.create","type":"webauthn.get","challenge":"{enc}","origin":"{o}"}}"#);
    assert!(wa::check_client_data(dup.as_bytes(), "webauthn.get", &ch, &o).is_ok());
    assert!(wa::check_client_data(format!(r#"{{"type":"webauthn.get","challenge":"{enc}","origin":"{o}"}}"#).as_bytes(), "webauthn.get", &ch, &o).is_ok());
}

/// The counter rule as written: once a passkey has counted, a counter of 0
/// is still accepted and leaves the stored value (a copy that reports 0 is
/// never detected). The spec chose this for iCloud Keychain; pinned here so
/// it is a decision, not an accident.
#[test]
fn note_a_zero_counter_after_a_counting_one_is_accepted() {
    let mut auth = Authenticator::new();
    let ch = [3u8; 32];
    let (raw_id, cd, att) = auth.register(TN, &ch, &Twist::default());
    let cred = wa::verify_registration(TN, &ch, &raw_id, &cd, &att).unwrap();
    let (_, cd, ad, sig) = auth.assert(TN, &ch, &Twist { counter: Some(0), ..Twist::default() });
    assert_eq!(wa::verify_assertion(TN, &ch, &cred.public_key, 41, &cd, &ad, &sig), Ok(41));
}

// ── Checked: level 2 scope ───────────────────────────────────────────────

/// A challenge for one action cannot be spent on another; a refuse
/// challenge does not approve; one assertion acts once; a passkey of
/// another device does not fit; the Jarvis sandbox and a running one are
/// never removable; a `port:` id is never answerable.
#[tokio::test]
async fn checked_one_assertion_one_action_one_target() {
    let w = world();
    let mut a = enrolled(&w, "a").await;
    let mut b = enrolled(&w, "b").await;
    let id = pending_plan(&w, "myapp", "g");
    let ch = challenge(&w, &a, "plan_refuse", &id, &format!("/r/plans/{id}")).await;
    let asr = assertion(&mut a, &ch);
    let r = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&a.token), json!({ "assertion": asr })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "a refuse challenge does not approve: {}", r.text());
    assert_eq!(w.app.plan(&id).unwrap().state, PlanState::Pending);
    // B's assertion over A's challenge (B's key, A's cookie): refused.
    let ch = challenge(&w, &a, "plan_approve", &id, &format!("/r/plans/{id}")).await;
    let asr = assertion(&mut b, &ch);
    let r = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&a.token), json!({ "assertion": asr })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // One good assertion, sent twice at once: one action.
    let ch = challenge(&w, &a, "plan_approve", &id, &format!("/r/plans/{id}")).await;
    let asr = assertion(&mut a, &ch);
    let path = format!("/r/plans/{id}/approve");
    let (r1, r2) = tokio::join!(
        call(&w.router, "POST", &path, Some(&a.token), json!({ "assertion": asr.clone() })),
        call(&w.router, "POST", &path, Some(&a.token), json!({ "assertion": asr.clone() }))
    );
    let oks = [&r1, &r2].iter().filter(|r| r.status == StatusCode::OK).count();
    assert_eq!(oks, 1, "{} / {}", r1.text(), r2.text());
    // Jarvis and running sandboxes, odd names, a port request.
    w.sbx.lock().unwrap().sandboxes = vec![("sbxw-jarvis".into(), "stopped".into(), "2026-10-01T00:00:00Z".into()), ("run".into(), "running".into(), "2026-10-01T00:00:00Z".into())];
    w.sbx.lock().unwrap().approvals = vec![json!({ "id": "port:k1", "sandbox_name": "myapp", "kind": "port", "title": "5173", "options": [{ "id": "dismiss", "label": "Dismiss" }] })];
    for path in ["/r/stopped/sbxw-jarvis", "/r/stopped/run", "/r/stopped/..%2Fx", "/r/stopped/RUN", "/r/network/port:k1", "/r/network/network:..", "/r/plans/p-../x", "/r/merges/p-0123456789abcdef0123456789abcdef"] {
        let r = call(&w.router, "GET", path, Some(&b.token), json!({})).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}: {}", r.text());
    }
    for (kind, target) in [("sandbox_remove", "sbxw-jarvis"), ("sandbox_remove", "run")] {
        let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&b.token), json!({ "kind": kind, "target": target, "digest": "0".repeat(64) })).await;
        assert_eq!(r.json()["reason"], "changed", "{kind} {target}");
    }
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&b.token), json!({ "kind": "network_respond", "target": "port:k1", "digest": "0".repeat(64), "option": "dismiss" })).await;
    assert!(r.status.is_client_error());
    assert!(!w.fake.called("rm") && !w.fake.called("policy approval respond"));
}

/// A device without a passkey, or with level 2 off, gets no challenge; a
/// registration challenge does not act; an action challenge does not
/// register.
#[tokio::test]
async fn checked_register_and_act_challenges_do_not_cross() {
    let w = world();
    let mut a = enrolled(&w, "a").await;
    let plain = pair(&w, "plain").await;
    let id = pending_plan(&w, "myapp", "g");
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&plain.token), json!({})).await;
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&plain.token), json!({ "kind": "plan_approve", "target": id, "digest": d.json()["digest"] })).await;
    assert_eq!(r.json()["reason"], "no_passkey");
    // A grant for A, options taken (a register challenge), then an act with it.
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&a.token), json!({})).await;
    w.app.remote_passkey_allow(&a.id, r.json()["check"].as_str().unwrap()).unwrap();
    let o = call(&w.router, "POST", "/r/passkey/options", Some(&a.token), json!({ "request_id": r.json()["request_id"] })).await;
    let ch = b64url_decode(o.json()["public_key"]["challenge"].as_str().unwrap(), 64).unwrap();
    let asr = assertion(&mut a, &ch);
    let r = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&a.token), json!({ "assertion": asr })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(w.app.plan(&id).unwrap().state, PlanState::Pending);
}
