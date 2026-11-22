//! Level 2 on the remote listener, end to end through its router: passkey
//! registration behind a Mac grant, assertions bound to one action on one
//! target with a digest, each action through the core function, the
//! refusals and that nothing was done, the lockout, the journal and logs,
//! the route walk. A software authenticator (`webauthn::soft`) plays the
//! phone. Every test runs in a temporary sbxw home with a scripted sbx.

use super::remote::{remote_router, ROUTES};
use crate::app::App;
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use sbxw_core::plans::{MergeState, PlanState, PlanThread, ThreadState, Variant};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::webauthn::soft::{Authenticator, Twist};
use sbxw_core::remote_access::webauthn::{b64url_decode, b64url_encode, FLAG_UP, FLAG_UV};
use sbxw_core::remote_access::COOKIE;
use sbxw_sbx::runner::Output;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

const TN: &str = "sbxw.tail1.ts.net";

/// What the scripted sbx lists: sandboxes `(name, status, created_at)` and
/// pending approvals.
#[derive(Default)]
struct SbxState {
    sandboxes: Vec<(String, String, String)>,
    approvals: Vec<Value>,
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
    headers: HeaderMap,
    body: Vec<u8>,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.text())
    }
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
    // Tests running at the same time on other threads (no subscriber there)
    // can leave a log site's interest cached as "never"; the log test needs
    // every site live on its thread, so look again before each request.
    tracing::callsite::rebuild_interest_cache();
    let res = router.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = match tokio::time::timeout(Duration::from_secs(3), axum::body::to_bytes(res.into_body(), 16 << 20)).await {
        Ok(b) => b.unwrap().to_vec(),
        Err(_) => b"(still streaming)".to_vec(),
    };
    Reply { status, headers, body }
}

async fn call(router: &Router, method: &str, path: &str, cookie: Option<&str>, body: Value) -> Reply {
    let m: Method = method.parse().unwrap();
    let mut b = Request::builder().method(m.clone()).uri(path).header(header::HOST, TN);
    if m != Method::GET {
        b = b.header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(t) = cookie {
        b = b.header(header::COOKIE, format!("{COOKIE}={t}"));
    }
    let body = if m == Method::GET { Body::empty() } else { Body::from(body.to_string()) };
    send(router, b.body(body).unwrap()).await
}

/// A paired phone: its token and device id.
struct Phone {
    token: String,
    id: String,
    auth: Authenticator,
    /// The request id `/r/passkey/start` gave THIS client.
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

async fn opts(w: &World, p: &Phone, id: &str) -> Reply {
    call(&w.router, "POST", "/r/passkey/options", Some(&p.token), json!({ "request_id": id })).await
}

async fn st(w: &World, p: &Phone, id: &str) -> Reply {
    call(&w.router, "GET", &format!("/r/passkey/status?request_id={id}"), Some(&p.token), json!({})).await
}

fn rid(p: &Phone) -> String {
    p.rid.borrow().clone()
}

/// The phone asks, the Mac types its digits: a grant.
async fn grant(w: &World, p: &Phone) {
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&p.token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let check = r.json()["check"].as_str().unwrap().to_string();
    *p.rid.borrow_mut() = r.json()["request_id"].as_str().unwrap().to_string();
    w.app.remote_passkey_allow(&p.id, &check).unwrap();
}

async fn register_with(w: &World, p: &Phone, t: &Twist) -> Reply {
    let r = call(&w.router, "POST", "/r/passkey/options", Some(&p.token), json!({ "request_id": rid(p) })).await;
    if r.status != StatusCode::OK {
        return r;
    }
    let pk = &r.json()["public_key"];
    assert_eq!(pk["rp"]["id"], TN);
    assert_eq!(pk["authenticatorSelection"]["userVerification"], "required");
    assert_eq!(pk["attestation"], "none");
    assert_eq!(pk["pubKeyCredParams"], json!([{ "type": "public-key", "alg": -7 }]));
    let ch = b64url_decode(pk["challenge"].as_str().unwrap(), 64).unwrap();
    assert!(ch.len() >= 16, "at least 128 bits");
    let (id, cd, att) = p.auth.register(TN, &ch, t);
    call(&w.router, "POST", "/r/passkey/register", Some(&p.token), json!({ "request_id": rid(p), "id": b64url_encode(&id), "client_data_json": b64url_encode(&cd), "attestation_object": b64url_encode(&att) })).await
}

async fn enrolled(w: &World, name: &str) -> Phone {
    let p = pair(w, name).await;
    grant(w, &p).await;
    let r = register_with(w, &p, &Twist::default()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    p
}

/// A pending plan in `sandbox` whose goal carries `goal`.
fn pending_plan(w: &World, sandbox: &str, goal: &str) -> String {
    let id = sbxw_core::ids::plan_id();
    let t = PlanThread { name: "alpha".into(), goal: goal.into(), scope: "crates/x".into(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket: None };
    w.app.plans.lock().unwrap().propose(id.clone(), sandbox, "Do the thing", vec![t], Variant::Split, "independent work", Some("lead"), 1).unwrap();
    id
}

fn plan_state(w: &World, id: &str) -> PlanState {
    w.app.plan(id).unwrap().state
}

/// The detail's digest, then a challenge for `kind` on `target`.
async fn challenge(w: &World, p: &Phone, kind: &str, target: &str, detail: &str, option: Option<&str>) -> Result<Vec<u8>, Box<Reply>> {
    let d = call(&w.router, "GET", detail, Some(&p.token), json!({})).await;
    if d.status != StatusCode::OK {
        return Err(Box::new(d));
    }
    let mut body = json!({ "kind": kind, "target": target, "digest": d.json()["digest"] });
    if let Some(o) = option {
        body["option"] = json!(o);
    }
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&p.token), body).await;
    if r.status != StatusCode::OK {
        return Err(Box::new(r));
    }
    let pk = &r.json()["public_key"];
    assert_eq!(pk["rpId"], TN);
    assert_eq!(pk["userVerification"], "required");
    Ok(b64url_decode(pk["challenge"].as_str().unwrap(), 64).unwrap())
}

fn assertion(p: &mut Phone, ch: &[u8], t: &Twist) -> Value {
    let (id, cd, ad, sig) = p.auth.assert(TN, ch, t);
    json!({ "id": b64url_encode(&id), "client_data_json": b64url_encode(&cd), "authenticator_data": b64url_encode(&ad), "signature": b64url_encode(&sig) })
}

async fn act(w: &World, p: &Phone, path: &str, a: Value, option: Option<&str>) -> Reply {
    let mut body = json!({ "assertion": a });
    if let Some(o) = option {
        body["option"] = json!(o);
    }
    call(&w.router, "POST", path, Some(&p.token), body).await
}

/// The whole approval of a plan as the page does it.
async fn approve_plan(w: &World, p: &mut Phone, id: &str, t: &Twist) -> Reply {
    let ch = challenge(w, p, "plan_approve", id, &format!("/r/plans/{id}"), None).await.unwrap_or_else(|r| panic!("{}", r.text()));
    let a = assertion(p, &ch, t);
    act(w, p, &format!("/r/plans/{id}/approve"), a, None).await
}

fn journal(w: &World) -> Vec<sbxw_core::remote_access::journal::Entry> {
    w.app.remote_journal(1000)
}

// ── Registration ─────────────────────────────────────────────────────────

#[tokio::test]
async fn registration_needs_a_grant_made_at_the_mac() {
    let w = world();
    let a = pair(&w, "phone a").await;
    let b = pair(&w, "phone b").await;
    // No grant: no options, and a registration is refused.
    let r = opts(&w, &a, &"0".repeat(32)).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("no_grant")));
    let (id, cd, att) = a.auth.register(TN, &[0u8; 32], &Twist::default());
    let r = call(&w.router, "POST", "/r/passkey/register", Some(&a.token), json!({ "request_id": "0".repeat(32), "id": b64url_encode(&id), "client_data_json": b64url_encode(&cd), "attestation_object": b64url_encode(&att) })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // A pending request is not a grant; wrong digits are not.
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&a.token), json!({})).await;
    let check = r.json()["check"].as_str().unwrap().to_string();
    let a_rid = r.json()["request_id"].as_str().unwrap().to_string();
    assert_eq!(a_rid.len(), 32);
    *a.rid.borrow_mut() = a_rid.clone();
    assert_eq!(opts(&w, &a, &a_rid).await.status, StatusCode::FORBIDDEN);
    let wrong = if check == "000000" { "111111" } else { "000000" };
    assert!(w.app.remote_passkey_allow(&a.id, wrong).is_err());
    // Another device's grant: B is granted, A still is not, even with B's id.
    grant(&w, &b).await;
    assert_eq!(opts(&w, &a, &a_rid).await.status, StatusCode::FORBIDDEN, "B's grant is not A's");
    assert_eq!(opts(&w, &a, &rid(&b)).await.status, StatusCode::FORBIDDEN, "B's request id under A's cookie");
    assert!(w.app.remote_passkey_allow(&b.id, &check).is_err(), "A's digits do not grant B");
    // A's own grant: only with A's request id; status only for that id.
    w.app.remote_passkey_allow(&a.id, &check).unwrap();
    assert_eq!(st(&w, &a, &a_rid).await.json()["state"], "granted");
    assert_eq!(st(&w, &a, &"f".repeat(32)).await.json()["state"], "none", "another id learns nothing");
    assert_eq!(call(&w.router, "GET", "/r/passkey/status", Some(&a.token), json!({})).await.json()["state"], "none", "no id learns nothing");
    assert_eq!(register_with(&w, &a, &Twist::default()).await.status, StatusCode::OK);
    assert_eq!(call(&w.router, "GET", "/r/approvals", Some(&a.token), json!({})).await.json()["passkey"], true);
    assert_eq!(opts(&w, &a, &a_rid).await.status, StatusCode::FORBIDDEN, "a used grant is spent");
    // The Mac sees when it was registered.
    let pk = &w.app.remote_approvals_status()["devices"].as_array().unwrap().iter().find(|d| d["id"] == a.id.as_str()).unwrap()["passkey"].clone();
    assert!(pk["created_ms"].as_u64().unwrap() > 0, "{pk}");
    assert!(journal(&w).iter().any(|e| e.kind == Kind::PasskeyRegistered && e.device_id == a.id));
    // Stored at rest: public data only, 0600.
    let path = w.app.remote_dir_for_tests().join("passkeys.json");
    let stored = std::fs::read_to_string(&path).unwrap();
    assert!(stored.contains(&a.id) && stored.contains(TN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[tokio::test]
async fn a_failed_registration_spends_the_grant_and_each_check_holds() {
    let w = world();
    let p = pair(&w, "phone").await;
    let t = Twist::default;
    let cases = vec![
        ("wrong origin", Twist { origin: Some("https://evil.ts.net".into()), ..t() }),
        ("wrong type", Twist { typ: Some("webauthn.get".into()), ..t() }),
        ("wrong challenge", Twist { challenge: Some(vec![1u8; 32]), ..t() }),
        ("wrong rp id hash", Twist { rp_id: Some("evil.ts.net".into()), ..t() }),
        ("UV not set", Twist { flags: FLAG_UP, ..t() }),
        ("UP not set", Twist { flags: FLAG_UV, ..t() }),
        ("RS256", Twist { alg: -257, ..t() }),
        ("attestation", Twist { fmt: "packed".into(), ..t() }),
    ];
    for (what, twist) in cases {
        reset_failures(&w);
        grant(&w, &p).await;
        let r = register_with(&w, &p, &twist).await;
        assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("passkey")), "{what}: {}", r.text());
        assert_eq!(call(&w.router, "POST", "/r/passkey/options", Some(&p.token), json!({ "request_id": rid(&p) })).await.status, StatusCode::FORBIDDEN, "{what}: the grant is spent");
        assert!(w.app.remote_access.lock().unwrap().level2.passkeys.get(&p.id).is_none(), "{what}: nothing stored");
    }
    assert!(journal(&w).iter().filter(|e| e.kind == Kind::PasskeyFailed).count() >= 8);
}

// ── The actions ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_plan_is_approved_or_refused_with_a_passkey() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "Write the parser");
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&p.token), json!({})).await;
    assert_eq!(d.json()["plan"]["threads"][0]["goal"], "Write the parser", "the goal, whole, as data");
    assert_eq!(call(&w.router, "GET", "/r/plans", Some(&p.token), json!({})).await.json()["plans"][0]["id"], id);
    // Refuse.
    let ch = challenge(&w, &p, "plan_refuse", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, &format!("/r/plans/{id}/refuse"), a, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(plan_state(&w, &id), PlanState::Refused);
    assert!(journal(&w).iter().any(|e| e.kind == Kind::PlanRefuse && e.target.as_deref() == Some(id.as_str())));
    // Approve another: through the core's plan_approve (it starts threads).
    let id2 = pending_plan(&w, "other", "Second goal");
    let r = approve_plan(&w, &mut p, &id2, &Twist::default()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(plan_state(&w, &id2), PlanState::Approved);
    assert_eq!(w.app.plan(&id2).unwrap().threads[0].goal, "Second goal", "started as shown");
    assert!(journal(&w).iter().any(|e| e.kind == Kind::PlanApprove && e.target.as_deref() == Some(id2.as_str())));
}

/// Every assertion check, through the route: refused, nothing done.
#[tokio::test]
async fn each_forged_assertion_is_refused_and_nothing_is_done() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    let t = Twist::default;
    let cases = vec![
        ("wrong challenge", Twist { challenge: Some(vec![5u8; 32]), ..t() }),
        ("wrong origin", Twist { origin: Some("https://sbxw.tail2.ts.net".into()), ..t() }),
        ("wrong type", Twist { typ: Some("webauthn.create".into()), ..t() }),
        ("wrong rp id hash", Twist { rp_id: Some("sbxw.tail2.ts.net".into()), ..t() }),
        ("UV not set", Twist { flags: FLAG_UP, ..t() }),
        ("UP not set", Twist { flags: FLAG_UV, ..t() }),
        ("bad signature", Twist { bad_signature: true, ..t() }),
        ("another key", Twist { other_key: true, ..t() }),
        ("cross origin", Twist { cross_origin: Some(json!(true)), ..t() }),
    ];
    for (what, twist) in cases {
        let r = approve_plan(&w, &mut p, &id, &twist).await;
        assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("passkey")), "{what}: {}", r.text());
        assert_eq!(plan_state(&w, &id), PlanState::Pending, "{what}: nothing done");
        // The limiter would stop the run: start each case from a clean slate.
        reset_failures(&w);
    }
    // Malformed: not base64url, oversized, not JSON, a credential that is not this device's.
    for (what, mangle) in [
        ("padding", Box::new(|a: &mut Value| a["signature"] = json!(format!("{}==", a["signature"].as_str().unwrap()))) as Box<dyn Fn(&mut Value)>),
        ("oversized client data", Box::new(|a: &mut Value| a["client_data_json"] = json!(b64url_encode(&[b'a'; 2100])))),
        ("not JSON", Box::new(|a: &mut Value| a["client_data_json"] = json!(b64url_encode(b"not json")))),
        ("another credential", Box::new(|a: &mut Value| a["id"] = json!(b64url_encode(&[9u8; 32])))),
        ("truncated authData", Box::new(|a: &mut Value| a["authenticator_data"] = json!(b64url_encode(&[0u8; 36])))),
    ] {
        let ch = challenge(&w, &p, "plan_approve", &id, &format!("/r/plans/{id}"), None).await.unwrap();
        let mut a = assertion(&mut p, &ch, &t());
        mangle(&mut a);
        let r = act(&w, &p, &format!("/r/plans/{id}/approve"), a, None).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{what}: {}", r.text());
        assert_eq!(plan_state(&w, &id), PlanState::Pending, "{what}");
        reset_failures(&w);
    }
    // Hostile JSON shapes: refused by the extractor, no panic.
    for body in [json!({}), json!({"assertion": {}}), json!({"assertion": "x"}), json!({"assertion": {"id": 1}}), json!({"assertion": null, "extra": 1})] {
        let r = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&p.token), body.clone()).await;
        assert!(r.status.is_client_error(), "{body}: {}", r.status);
    }
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
    assert!(journal(&w).iter().any(|e| e.kind == Kind::PasskeyFailed));
}

fn reset_failures(w: &World) {
    let mut rt = w.app.remote_access.lock().unwrap();
    let enabled = rt.level2.enabled;
    let passkeys = rt.level2.passkeys.clone();
    let dir = rt.dir.clone();
    rt.level2 = crate::svc::remote_approvals::Level2::load(&dir, enabled);
    rt.level2.passkeys = passkeys;
}

#[tokio::test]
async fn a_challenge_is_single_use_for_this_device_action_and_target() {
    let w = world();
    let mut a = enrolled(&w, "a").await;
    let mut b = enrolled(&w, "b").await;
    let id = pending_plan(&w, "myapp", "goal");
    let other = pending_plan(&w, "other", "goal 2");
    let ok = || Twist::default();
    // Reused: the same assertion twice; the second finds no challenge.
    let ch = challenge(&w, &a, "plan_refuse", &other, &format!("/r/plans/{other}"), None).await.unwrap();
    let asrt = assertion(&mut a, &ch, &ok());
    assert_eq!(act(&w, &a, &format!("/r/plans/{other}/refuse"), asrt.clone(), None).await.status, StatusCode::OK);
    let id3 = pending_plan(&w, "third", "goal 3");
    let r = act(&w, &a, &format!("/r/plans/{id3}/refuse"), asrt, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "a replay");
    assert_eq!(plan_state(&w, &id3), PlanState::Pending);
    // Another action's challenge: made for refuse, used to approve.
    let ch = challenge(&w, &a, "plan_refuse", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    let asrt = assertion(&mut a, &ch, &ok());
    assert_eq!(act(&w, &a, &format!("/r/plans/{id}/approve"), asrt, None).await.status, StatusCode::FORBIDDEN, "another action");
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
    // Another target's challenge.
    let ch = challenge(&w, &a, "plan_approve", &id3, &format!("/r/plans/{id3}"), None).await.unwrap();
    let asrt = assertion(&mut a, &ch, &ok());
    assert_eq!(act(&w, &a, &format!("/r/plans/{id}/approve"), asrt, None).await.status, StatusCode::FORBIDDEN, "another target");
    assert_eq!((plan_state(&w, &id), plan_state(&w, &id3)), (PlanState::Pending, PlanState::Pending));
    // Another device's challenge: B's challenge, signed by A, sent by A; and
    // signed by B, sent by A.
    let ch_b = challenge(&w, &b, "plan_approve", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    let asrt = assertion(&mut a, &ch_b, &ok());
    assert_eq!(act(&w, &a, &format!("/r/plans/{id}/approve"), asrt, None).await.status, StatusCode::FORBIDDEN, "A has no challenge of its own");
    let asrt_b = assertion(&mut b, &ch_b, &ok());
    assert_eq!(act(&w, &a, &format!("/r/plans/{id}/approve"), asrt_b.clone(), None).await.status, StatusCode::FORBIDDEN, "B's assertion under A's cookie");
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
    // B's own challenge was untouched by A's attempts: B approves.
    let r = act(&w, &b, &format!("/r/plans/{id}/approve"), asrt_b, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
}

#[tokio::test]
async fn an_expired_challenge_is_refused() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    let ch = challenge(&w, &p, "plan_approve", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    // Age it past its 60 s.
    w.app.remote_access.lock().unwrap().level2.challenges.age_for_tests(&p.id, 1);
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(act(&w, &p, &format!("/r/plans/{id}/approve"), a, None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
}

#[tokio::test]
async fn a_counter_that_goes_back_drops_the_passkey_and_zero_is_always_fine() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    // iCloud Keychain: always 0.
    for _ in 0..2 {
        let id = pending_plan(&w, &format!("sb{}", sbxw_core::ids::random_hex(3)), "g");
        let r = approve_plan(&w, &mut p, &id, &Twist { counter: Some(0), ..Twist::default() }).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    }
    let id = pending_plan(&w, "c1", "g");
    assert_eq!(approve_plan(&w, &mut p, &id, &Twist { counter: Some(7), ..Twist::default() }).await.status, StatusCode::OK);
    let id = pending_plan(&w, "c2", "g");
    let r = approve_plan(&w, &mut p, &id, &Twist { counter: Some(7), ..Twist::default() }).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "not above the stored 7");
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
    assert!(w.app.remote_access.lock().unwrap().level2.passkeys.get(&p.id).is_none(), "the passkey is dropped");
    let r = challenge(&w, &p, "plan_approve", &id, &format!("/r/plans/{id}"), None).await.unwrap_err();
    assert_eq!(r.json()["reason"], "no_passkey");
}

#[tokio::test]
async fn without_a_valid_assertion_nothing_is_done() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    // The device cookie alone.
    let r = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&p.token), json!({})).await;
    assert!(r.status.is_client_error());
    // A well-formed assertion with no challenge issued.
    let a = assertion(&mut p, &[1u8; 32], &Twist::default());
    assert_eq!(act(&w, &p, &format!("/r/plans/{id}/approve"), a, None).await.status, StatusCode::FORBIDDEN);
    // A device without a passkey: told so.
    let q = pair(&w, "no face id").await;
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&q.token), json!({})).await;
    assert_eq!(d.status, StatusCode::OK, "it sees the items");
    let body = json!({ "kind": "plan_approve", "target": id, "digest": d.json()["digest"] });
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&q.token), body).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("no_passkey")));
    assert!(r.json()["error"].as_str().unwrap().contains("Approving needs Face ID"));
    let ov = call(&w.router, "GET", "/r/approvals", Some(&q.token), json!({})).await.json();
    assert_eq!((ov["level2"].as_bool(), ov["passkey"].as_bool()), (Some(true), Some(false)));
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
}

#[tokio::test]
async fn a_plan_edited_after_the_challenge_is_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "the goal shown");
    let ch = challenge(&w, &p, "plan_approve", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    {
        let mut book = w.app.plans.lock().unwrap();
        let mut list = book.list();
        list.iter_mut().filter(|pl| pl.id == id).for_each(|pl| pl.threads[0].goal = "a different goal".into());
        *book = sbxw_core::plans::PlanBook::load(list);
    }
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, &format!("/r/plans/{id}/approve"), a, None).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("changed")), "{}", r.text());
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
    assert!(journal(&w).iter().any(|e| e.kind == Kind::Changed));
    // A digest the phone holds from before the change: no challenge either.
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&p.token), json!({ "kind": "plan_approve", "target": id, "digest": "0".repeat(64) })).await;
    assert_eq!(r.json()["reason"], "changed");
}

// ── Merges, on a real repository ─────────────────────────────────────────

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A workspace repo with `main` and a thread branch `sbxw/alpha` one commit
/// ahead; an approved plan whose thread is done; its merge proposed.
fn proposed_merge(w: &World, ws: &std::path::Path) -> String {
    std::fs::create_dir_all(ws).unwrap();
    git(ws, &["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("a.txt"), "one\n").unwrap();
    git(ws, &["add", "."]);
    git(ws, &["commit", "-qm", "base"]);
    git(ws, &["branch", "sbxw/alpha"]);
    git(ws, &["checkout", "-q", "sbxw/alpha"]);
    std::fs::write(ws.join("b.rs"), "SECRET_DIFF_LINE\nx\n").unwrap();
    git(ws, &["add", "."]);
    git(ws, &["commit", "-qm", "thread work"]);
    git(ws, &["checkout", "-q", "main"]);
    let sb = sbxw_core::SandboxRef::parse("myapp").unwrap();
    w.app.store.update(&sb, |r| r.workspace = Some(ws.to_path_buf())).unwrap();
    let id = pending_plan(w, "myapp", "merge me");
    let mut book = w.app.plans.lock().unwrap();
    let threads = book.get(&id).unwrap().threads.clone();
    book.approve(&id, threads, None, None, 2).unwrap();
    book.set_thread("myapp", "alpha", ThreadState::Done, "done", false, 3).unwrap();
    book.propose_merge(&id, "myapp", vec!["alpha".into()], "Merge alpha please", vec![], 4).unwrap();
    id
}

#[tokio::test]
async fn a_merge_shows_numbers_and_paths_and_a_moved_branch_is_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let ws = w._dir.path().join("ws");
    let id = proposed_merge(&w, &ws);
    let d = call(&w.router, "GET", &format!("/r/merges/{id}"), Some(&p.token), json!({})).await;
    assert_eq!(d.status, StatusCode::OK, "{}", d.text());
    let b = &d.json()["merge"]["branches"][0];
    assert_eq!((b["thread"].as_str(), b["ahead"].as_u64(), b["files_changed"].as_u64()), (Some("alpha"), Some(1), Some(1)));
    assert_eq!(b["files"][0], json!({ "path": "b.rs", "insertions": 2, "deletions": 0 }));
    assert_eq!(b["base"]["branch"], "main");
    assert_eq!(b["tip"].as_str().unwrap().len(), 7);
    assert!(!d.text().contains("SECRET_DIFF_LINE"), "no diff content");
    // The branch moves after the challenge: changed, nothing merged.
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}"), None).await.unwrap();
    git(&ws, &["checkout", "-q", "sbxw/alpha"]);
    std::fs::write(ws.join("c.rs"), "more\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "moved"]);
    git(&ws, &["checkout", "-q", "main"]);
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, &format!("/r/merges/{id}/merge"), a, None).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("changed")), "{}", r.text());
    assert_eq!(w.app.plan(&id).unwrap().merge.unwrap().state, MergeState::Proposed, "nothing merged");
    assert!(!w.fake.log().iter().any(|c| c.contains("merge")), "no merge ran: {:?}", w.fake.log());
    // The base moves: changed too.
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}"), None).await.unwrap();
    std::fs::write(ws.join("d.txt"), "base moved\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "base moved"]);
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(act(&w, &p, &format!("/r/merges/{id}/merge"), a, None).await.json()["reason"], "changed");
    // As shown now: the core's plan_merge runs (the scripted sbx cannot
    // really merge, so its result comes back as the desktop would get it).
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}"), None).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, &format!("/r/merges/{id}/merge"), a, None).await;
    assert!(r.status == StatusCode::OK || r.json()["reason"] == "refused", "{}", r.text());
    assert_ne!(w.app.plan(&id).unwrap().merge.unwrap().state, MergeState::Proposed, "the core's merge ran");
    assert!(journal(&w).iter().any(|e| matches!(e.kind, Kind::Merge | Kind::Refused) && e.target.as_deref() == Some(id.as_str())));
}

/// Merging from the phone closes the plan's threads, as the web does by
/// default, and keeps the distillate the digest covered.
#[tokio::test]
async fn a_phone_merge_closes_the_threads_and_keeps_the_distillate() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let ws = w._dir.path().join("ws5");
    let id = proposed_merge(&w, &ws);
    let kept = vec![sbxw_core::plans::DistillItem { kind: sbxw_core::plans::DistillKind::Decision, text: "prices in cents".into() }];
    w.app.plans.lock().unwrap().propose_merge(&id, "myapp", vec!["alpha".into()], "Merge alpha please", kept.clone(), 5).unwrap();
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}"), None).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, &format!("/r/merges/{id}/merge"), a, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let ended: Vec<String> = w.fake.log().into_iter().filter(|c| c.ends_with("sbxw-thread-end =alpha =close =0 =1") || c.contains("sbxw-thread-end") && c.ends_with("=alpha =close =0 =1")).collect();
    assert_eq!(ended.len(), 1, "the merged thread was closed, branch and all");
    assert_eq!(w.app.plan(&id).unwrap().merge.unwrap().distillate, kept);
}

// ── Network requests ─────────────────────────────────────────────────────

fn network_request(id: &str, host: &str, options: Value) -> Value {
    json!({ "id": id, "sandbox_name": "myapp", "title": host, "detail": "no matching allow rule", "created_at": "2026-11-20T10:00:00Z", "options": options })
}

/// A row as a real `sbx policy approval ls --format json` (0.51) prints it:
/// the shape of `core::approvals`' fixture and of the desktop's 83 rows.
fn real_sbx_row(id: &str, sandbox: &str, host: &str) -> Value {
    json!({
        "created_at": "2026-11-06T15:08:54.850902+01:00",
        "detail": "Protocol: TCP\nResource type: domain\nno matching allow rule (default deny)",
        "id": id,
        "options": [{ "id": "allow", "label": "Allow" }, { "id": "dismiss", "label": "Dismiss" }],
        "sandbox_name": sandbox,
        "title": host
    })
}

fn responded(w: &World, id: &str, option: &str) -> bool {
    w.fake.log().iter().any(|c| *c == format!("policy approval respond {id} --option {option}"))
}

/// Every option sbx 0.51 offers (`allow`, `dismiss`) is answered from the
/// phone with a valid assertion, and with a forged one nothing is sent.
#[tokio::test]
async fn each_sbx_network_option_is_answered_from_the_phone_with_face_id() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    w.sbx.lock().unwrap().approvals = vec![real_sbx_row("network:85TMuE6", "sbxw-2", "storage.googleapis.com:443")];
    let list = call(&w.router, "GET", "/r/network", Some(&p.token), json!({})).await.json();
    let opts = &list["requests"][0]["options"];
    assert_eq!(opts.as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(), ["allow", "dismiss"], "every option sbx offers");
    assert_eq!((opts[0]["lasting"].as_bool(), opts[1]["lasting"].as_bool()), (Some(true), Some(false)), "Allow is flagged");
    assert!(opts[0]["effect"].as_str().unwrap().starts_with("Adds a lasting network rule to sbxw-2"));
    assert!(list["requests"][0].get("mac_only").is_none());
    let detail = "/r/network/network%3A85TMuE6";
    for option in ["allow", "dismiss"] {
        // A forged assertion: refused, nothing sent to sbx.
        let ch = challenge(&w, &p, "network_respond", "network:85TMuE6", detail, Some(option)).await.unwrap();
        let a = assertion(&mut p, &ch, &Twist { bad_signature: true, ..Twist::default() });
        let r = act(&w, &p, "/r/network/network%3A85TMuE6/respond", a, Some(option)).await;
        assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("passkey")), "{option}: {}", r.text());
        // No assertion at all: refused by the extractor.
        let r = call(&w.router, "POST", "/r/network/network%3A85TMuE6/respond", Some(&p.token), json!({ "option": option })).await;
        assert!(r.status.is_client_error(), "{option}");
        assert!(!responded(&w, "network:85TMuE6", option), "{option}: nothing answered");
        reset_failures(&w);
        // A valid one: sbx's own answer, with that option.
        let ch = challenge(&w, &p, "network_respond", "network:85TMuE6", detail, Some(option)).await.unwrap();
        let a = assertion(&mut p, &ch, &Twist::default());
        let r = act(&w, &p, "/r/network/network%3A85TMuE6/respond", a, Some(option)).await;
        assert_eq!(r.status, StatusCode::OK, "{option}: {}", r.text());
        assert!(responded(&w, "network:85TMuE6", option), "{option}: {:?}", w.fake.log());
    }
    assert_eq!(journal(&w).iter().filter(|e| e.kind == Kind::NetworkRespond && e.target.as_deref() == Some("storage.googleapis.com:443")).count(), 2);
}

/// An option the request does not offer: refused at the challenge, and an
/// act naming another option than its challenge is refused too.
#[tokio::test]
async fn a_network_option_not_offered_is_refused() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    w.sbx.lock().unwrap().approvals = vec![
        network_request("network:abc", "api.example.com:443", json!([{ "id": "dismiss", "label": "Dismiss" }])),
        json!({ "id": "port:k1", "sandbox_name": "myapp", "title": "5173", "options": [{ "id": "allow", "label": "Allow" }] }),
    ];
    let list = call(&w.router, "GET", "/r/network", Some(&p.token), json!({})).await.json();
    assert_eq!(list["requests"].as_array().unwrap().len(), 1, "a port request is port publishing: never listed");
    assert_eq!(call(&w.router, "GET", "/r/network/port:k1", Some(&p.token), json!({})).await.status, StatusCode::NOT_FOUND);
    for o in ["allow", "allow-always", "Dismiss", "dismiss "] {
        let r = challenge(&w, &p, "network_respond", "network:abc", "/r/network/network:abc", Some(o)).await.unwrap_err();
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{o}: {}", r.text());
    }
    // No option, or an option on another kind: refused.
    assert!(challenge(&w, &p, "network_respond", "network:abc", "/r/network/network:abc", None).await.is_err());
    let ch = challenge(&w, &p, "network_respond", "network:abc", "/r/network/network:abc", Some("dismiss")).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(act(&w, &p, "/r/network/network:abc/respond", a, Some("allow")).await.status, StatusCode::FORBIDDEN, "bound to its option");
    assert!(!w.fake.called("policy approval respond"));
}

/// The bug seen on an iPhone: the list showed host and sandbox, the detail
/// an empty "asked by", no host, no option. The detail carried the request
/// flat while the page (and the contract) read it under `request`.
#[tokio::test]
async fn the_network_detail_carries_its_request_as_the_list_does() {
    let w = world();
    let p = enrolled(&w, "phone").await;
    w.sbx.lock().unwrap().approvals = vec![real_sbx_row("network:85TMuE6", "sbxw-2", "storage.googleapis.com:443")];
    let list = call(&w.router, "GET", "/r/network", Some(&p.token), json!({})).await.json();
    let row = &list["requests"][0];
    let d = call(&w.router, "GET", "/r/network/network%3A85TMuE6", Some(&p.token), json!({})).await;
    assert_eq!(d.status, StatusCode::OK, "{}", d.text());
    let d = d.json();
    let r = &d["request"];
    assert_eq!((r["id"].as_str(), r["sandbox"].as_str(), r["host"].as_str()), (Some("network:85TMuE6"), Some("sbxw-2"), Some("storage.googleapis.com:443")));
    assert_eq!((&r["sandbox"], &r["host"], &r["options"]), (&row["sandbox"], &row["host"], &row["options"]), "what the list showed");
    assert_eq!(r["options"].as_array().unwrap().len(), 2);
    assert!(r["detail"].as_str().unwrap().contains("default deny"));
    assert!(d["digest"].as_str().is_some_and(|x| x.len() == 64));
    assert!(d.get("host").is_none(), "not flat any more");
}

#[tokio::test]
async fn a_network_request_answered_elsewhere_is_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    w.sbx.lock().unwrap().approvals = vec![network_request("network:abc", "api.example.com:443", json!([{ "id": "dismiss", "label": "Dismiss" }]))];
    let ch = challenge(&w, &p, "network_respond", "network:abc", "/r/network/network:abc", Some("dismiss")).await.unwrap();
    w.sbx.lock().unwrap().approvals.clear();
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, "/r/network/network:abc/respond", a, Some("dismiss")).await;
    assert_eq!(r.json()["reason"], "changed", "{}", r.text());
    assert!(!w.fake.called("policy approval respond"));
}

// ── Removing a stopped sandbox ───────────────────────────────────────────

#[tokio::test]
async fn only_a_stopped_sandbox_is_removed_and_a_started_or_recreated_one_is_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    w.sbx.lock().unwrap().sandboxes = vec![
        ("old".into(), "stopped".into(), "2026-10-01T00:00:00Z".into()),
        ("busy".into(), "running".into(), "2026-10-01T00:00:00Z".into()),
        ("sbxw-jarvis".into(), "stopped".into(), "2026-10-01T00:00:00Z".into()),
    ];
    let list = call(&w.router, "GET", "/r/stopped", Some(&p.token), json!({})).await.json();
    let names: Vec<&str> = list["sandboxes"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["old"], "running and Jarvis are not listed");
    assert_eq!(call(&w.router, "GET", "/r/stopped/busy", Some(&p.token), json!({})).await.status, StatusCode::NOT_FOUND);
    let d = call(&w.router, "GET", "/r/stopped/old", Some(&p.token), json!({})).await.json();
    assert_eq!(d["confirm"][0], "Remove old?");
    assert!(d["confirm"][2].as_str().unwrap().starts_with("This is irreversible"));
    // Started since the challenge: changed, nothing removed.
    let ch = challenge(&w, &p, "sandbox_remove", "old", "/r/stopped/old", None).await.unwrap();
    w.sbx.lock().unwrap().sandboxes[0].1 = "running".into();
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(act(&w, &p, "/r/stopped/old/remove", a, None).await.json()["reason"], "changed");
    assert!(!w.fake.called("rm"));
    // Removed and created again under the same name: changed.
    w.sbx.lock().unwrap().sandboxes[0].1 = "stopped".into();
    let ch = challenge(&w, &p, "sandbox_remove", "old", "/r/stopped/old", None).await.unwrap();
    w.sbx.lock().unwrap().sandboxes[0].2 = "2026-11-20T09:00:00Z".into();
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(act(&w, &p, "/r/stopped/old/remove", a, None).await.json()["reason"], "changed");
    assert!(!w.fake.called("rm"));
    // As shown: removed through the desktop's removal.
    let ch = challenge(&w, &p, "sandbox_remove", "old", "/r/stopped/old", None).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, "/r/stopped/old/remove", a, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(w.fake.log().iter().any(|c| c == "rm --force --format json old"), "{:?}", w.fake.log());
    assert!(journal(&w).iter().any(|e| e.kind == Kind::SandboxRemove && e.target.as_deref() == Some("old")));
}

// ── Level 2 off, the route walk, tokens ──────────────────────────────────

fn l2_routes() -> Vec<(&'static str, String)> {
    let concrete = |p: &str| p.replace(":id", "p-0123456789abcdef0123456789abcdef").replace(":sandbox", "old");
    ROUTES
        .iter()
        .filter(|(_, p)| ["/r/passkey", "/r/plans", "/r/merges", "/r/network", "/r/stopped", "/r/approvals/"].iter().any(|x| p.starts_with(x)))
        .map(|(m, p)| (*m, concrete(p)))
        .collect()
}

#[tokio::test]
async fn level_2_off_refuses_every_level_2_route_while_level_1_works() {
    let w = world();
    let p = enrolled(&w, "phone").await;
    w.app.remote_approvals_disable();
    let routes = l2_routes();
    assert_eq!(routes.len(), 18, "every level 2 route but the overview");
    let assertion = json!({ "id": "AA", "client_data_json": "AA", "authenticator_data": "AA", "signature": "AA" });
    for (m, path) in &routes {
        let body = match path.as_str() {
            "/r/passkey/register" => json!({ "request_id": "0".repeat(32), "id": "AA", "client_data_json": "AA", "attestation_object": "AA" }),
            "/r/passkey/options" => json!({ "request_id": "0".repeat(32) }),
            "/r/approvals/challenge" => json!({ "kind": "merge", "target": "p-0123456789abcdef0123456789abcdef", "digest": "0".repeat(64) }),
            p if p.ends_with("/respond") => json!({ "option": "dismiss", "assertion": assertion }),
            p if p.ends_with("/approve") || p.ends_with("/refuse") || p.ends_with("/merge") || p.ends_with("/remove") => json!({ "assertion": assertion }),
            _ => json!({}),
        };
        let r = call(&w.router, m, path, Some(&p.token), body).await;
        assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::FORBIDDEN, Some("level2_off")), "{m} {path}: {}", r.text());
    }
    assert_eq!(call(&w.router, "GET", "/r/approvals", Some(&p.token), json!({})).await.json()["level2"], false);
    assert_eq!(call(&w.router, "GET", "/r/leaders", Some(&p.token), json!({})).await.status, StatusCode::OK, "level 1 still works");
    // Turning remote access off turns level 2 off, and it stays off.
    w.app.remote_approvals_enable().unwrap();
    w.app.remote_disable().await;
    assert_eq!(w.app.remote_approvals_status()["enabled"], false);
    assert!(w.app.remote_approvals_enable().is_err(), "only with remote access on");
    let state = std::fs::read_to_string(w.app.remote_dir_for_tests().join("state.json")).unwrap();
    assert!(!state.contains("\"approvals\": true"), "{state}");
}

#[tokio::test]
async fn level_2_is_off_by_default_and_survives_a_restart_when_on() {
    let d = tempfile::tempdir().unwrap();
    let app = crate::testing::app_in(d.path());
    app.remote_test_on(TN);
    assert_eq!(app.remote_approvals_status()["enabled"], false, "off by default");
    app.remote_approvals_enable().unwrap();
    drop(app);
    let again = crate::testing::app_in(d.path());
    assert_eq!(again.remote_approvals_status()["enabled"], true);
}

#[tokio::test]
async fn the_admin_routes_live_on_the_ui_listener_only() {
    let w = world();
    let p = pair(&w, "phone").await;
    let ui = crate::http::ui_router(w.app.clone());
    let ui_paths = format!("{ui:?}");
    for (m, path) in super::remote_approvals_admin::ROUTES {
        assert!(ui_paths.contains(&format!("\"{path}\"")), "{path} on the UI router");
        let concrete = path.replace(":id", &p.id);
        let r = call(&w.router, m, &concrete, Some(&p.token), json!({ "check": "000000" })).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{m} {path} on the remote listener");
        // The UI listener with the device cookie: refused.
        let req = Request::builder().method(*m).uri(&concrete).header(header::HOST, "127.0.0.1:7681").header(header::COOKIE, format!("{COOKIE}={}", p.token)).header(header::CONTENT_TYPE, "application/json").body(Body::from("{\"check\":\"000000\"}")).unwrap();
        assert_eq!(send(&ui, req).await.status, StatusCode::UNAUTHORIZED, "{m} {path} with a device cookie");
        // An agent token (the relay, a sandbox, Jarvis): the agent listener has none of these.
        let token = w.app.store.token(sbxw_core::SandboxRef::parse("myapp").unwrap()).unwrap();
        let req = Request::builder().method(*m).uri(&concrete).header(header::AUTHORIZATION, format!("Bearer {token}")).header(header::CONTENT_TYPE, "application/json").body(Body::from("{}")).unwrap();
        let s = send(&crate::http::agent_router(w.app.clone()), req).await.status;
        assert!(s == StatusCode::NOT_FOUND || s == StatusCode::METHOD_NOT_ALLOWED || s == StatusCode::UNAUTHORIZED, "{m} {path} on the agent listener: {s}");
    }
    assert_eq!(w.app.remote_approvals_status()["enabled"], true, "nothing changed");
    // With the UI token: grant and revoke, as the CLI does.
    let signed = crate::testing::signed_in(w.app.clone());
    let req = Request::builder().uri("/api/remote/approvals").header(header::HOST, "127.0.0.1:7681").body(Body::empty()).unwrap();
    let r = send(&signed, req).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["devices"][0]["id"], p.id.as_str());
}

#[tokio::test]
async fn failed_assertions_lock_out_only_their_own_device() {
    let w = world();
    let mut a = enrolled(&w, "a").await;
    let mut b = enrolled(&w, "b").await;
    let id = pending_plan(&w, "myapp", "goal");
    let bad = Twist { bad_signature: true, ..Twist::default() };
    for _ in 0..5 {
        assert_eq!(approve_plan(&w, &mut a, &id, &bad).await.status, StatusCode::FORBIDDEN);
    }
    let r = approve_plan(&w, &mut a, &id, &Twist::default()).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "a's own good try waits: {}", r.text());
    assert!(r.headers.contains_key(header::RETRY_AFTER));
    // Strangers' bad cookies count nothing against b; b approves.
    for _ in 0..40 {
        let _ = call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some("sbxw_dev_nope"), json!({})).await;
    }
    let r = approve_plan(&w, &mut b, &id, &Twist::default()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
}

#[tokio::test]
async fn a_revoked_device_or_passkey_approves_nothing() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    let ch = challenge(&w, &p, "plan_approve", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    // Passkey revoked at the Mac: its challenge goes too.
    assert!(w.app.remote_passkey_revoke(&p.id));
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, &format!("/r/plans/{id}/approve"), a.clone(), None).await;
    assert_eq!(r.json()["reason"], "no_passkey");
    // Re-enrolled, then the device revoked: its passkey is dropped with it.
    grant(&w, &p).await;
    assert_eq!(register_with(&w, &p, &Twist::default()).await.status, StatusCode::OK);
    assert!(w.app.remote_revoke(&p.id, Kind::Revoked));
    assert!(w.app.remote_access.lock().unwrap().level2.passkeys.get(&p.id).is_none());
    assert!(!std::fs::read_to_string(w.app.remote_dir_for_tests().join("passkeys.json")).unwrap().contains(&p.id));
    assert_eq!(act(&w, &p, &format!("/r/plans/{id}/approve"), a, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
}

#[tokio::test]
async fn a_new_tailnet_name_voids_the_passkey() {
    let w = world();
    let p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    w.app.remote_access.lock().unwrap().tailnet_override = Some("sbxw.tail9.ts.net".into());
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&p.token), json!({})).await;
    let mut b = Request::builder().method("POST").uri("/r/approvals/challenge").header(header::HOST, "sbxw.tail9.ts.net");
    b = b.header(header::ORIGIN, "https://sbxw.tail9.ts.net").header(header::CONTENT_TYPE, "application/json").header(header::COOKIE, format!("{COOKIE}={}", p.token));
    let _ = d;
    let r = send(&w.router, b.body(Body::from(json!({ "kind": "plan_approve", "target": id, "digest": "0".repeat(64) }).to_string())).unwrap()).await;
    assert_eq!(r.json()["reason"], "no_passkey", "{}", r.text());
    assert!(r.json()["error"].as_str().unwrap().contains("tailnet name changed"));
}

// ── The journal, the logs, the badge ─────────────────────────────────────

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<String>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={v:?} ", f.name());
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        let mut line = format!("{} ", e.metadata().level());
        e.record(&mut Fields(&mut line));
        let mut all = self.0.lock().unwrap();
        all.push_str(&line);
        all.push('\n');
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test(flavor = "current_thread")]
async fn the_journal_and_logs_carry_no_goal_and_no_diff() {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(logs.clone());
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    // Other tests running at the same time can leave a log site's interest
    // cached as "never" (no subscriber on their thread): look again now that
    // this thread's capture is set.
    tracing::callsite::rebuild_interest_cache();
    let id = pending_plan(&w, "myapp", "GOAL-TEXT-PINEAPPLE refactor the parser");
    let ch = challenge(&w, &p, "plan_refuse", &id, &format!("/r/plans/{id}"), None).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(act(&w, &p, &format!("/r/plans/{id}/refuse"), a, None).await.status, StatusCode::OK);
    let ws = w._dir.path().join("ws2");
    let mid = proposed_merge(&w, &ws);
    let _ = call(&w.router, "GET", &format!("/r/merges/{mid}"), Some(&p.token), json!({})).await;
    let _ = approve_plan(&w, &mut p, &pending_plan(&w, "other", "GOAL-TEXT-PINEAPPLE again"), &Twist { bad_signature: true, ..Twist::default() }).await;
    let journal = std::fs::read_to_string(w.app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    assert!(journal.contains(&format!("\"target\":\"{id}\"")), "{journal}");
    assert!(journal.contains("\"kind\":\"plan_refuse\"") && journal.contains("\"kind\":\"passkey_failed\""), "{journal}");
    // What this thread captured (log sites muted by other tests' threads are
    // covered by `every_level_2_log_line_names_a_device_and_a_kind_only`).
    let logged = logs.0.lock().unwrap().clone();
    for leak in ["PINEAPPLE", "SECRET_DIFF_LINE", "b.rs", "Merge alpha please", &p.token[..]] {
        assert!(!journal.contains(leak), "{leak} in the journal:\n{journal}");
        assert!(!logged.contains(leak), "{leak} in the logs:\n{logged}");
    }
}

/// Every info and warn line of level 2, as written in the source: a device
/// id and a kind, or a fixed sentence. A new line fails this test until it is
/// added here, so a reviewer sees each one.
#[test]
fn every_level_2_log_line_names_a_device_and_a_kind_only() {
    let src = include_str!("../svc/remote_approvals.rs");
    let found: Vec<&str> = src.lines().map(str::trim).filter(|l| l.contains("tracing::info!(") || l.contains("tracing::warn!(")).collect();
    let allowed = [
        r#"tracing::warn!("remote access: passkeys.json unreadable ({e}); no passkey is kept");"#,
        r#"tracing::warn!("remote access: cannot write passkeys.json: {e}");"#,
        r#"tracing::info!("remote access: approving from the phone turned on");"#,
        r#"tracing::info!("remote access: approving from the phone turned off");"#,
        r#"Ok(_) => tracing::info!("remote access: device {device_id}: passkey registration granted on the Mac"),"#,
        r#"tracing::info!("remote access: device {device_id}: passkey request refused after wrong digits");"#,
        r#"tracing::info!("remote access: device {device_id}: passkey revoked on the Mac");"#,
        r#"tracing::info!("remote access: device {}: passkey requested", d.id);"#,
        r#"tracing::info!("remote access: device {}: passkey_registered", d.id);"#,
        r#"tracing::info!("remote access: device {}: passkey_failed", d.id);"#,
        r#"tracing::warn!("remote access: device {}: its passkey's counter went back, a sign it was copied; the passkey is dropped", d.id);"#,
        r#"tracing::info!("remote access: device {}: {code}", d.id);"#,
    ];
    for l in &found {
        assert!(allowed.contains(l), "a log line not reviewed for level 2: {l}");
    }
    assert_eq!(found.len(), 13, "{found:#?}");
    // `{code}` is a journal kind's name.
    assert!(src.contains("let code = serde_json::to_value(kind)"));
}

#[tokio::test]
async fn the_event_stream_carries_a_badge_of_numbers() {
    use futures_util::StreamExt;
    let w = world();
    let p = pair(&w, "phone").await;
    let _id = pending_plan(&w, "myapp", "GOAL-TEXT");
    let req = Request::builder().uri("/r/events").header(header::HOST, TN).header(header::COOKIE, format!("{COOKIE}={}", p.token)).body(Body::empty()).unwrap();
    let res = w.router.clone().oneshot(req).await.unwrap();
    let mut body = res.into_body().into_data_stream();
    let mut seen = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(b)) = body.next().await {
            seen.push_str(&String::from_utf8_lossy(&b));
            if seen.contains("event: approvals") && seen.contains("\"stopped\"") {
                break;
            }
        }
    })
    .await;
    assert!(seen.contains("event: approvals"), "{seen}");
    assert!(seen.contains("\"plans\":1"), "{seen}");
    assert!(!seen.contains("GOAL-TEXT"), "numbers only");
}

/// The badge and the Kanban nudge share one stream: each arrives, neither
/// holds the other back, and `bye` still ends the stream.
#[tokio::test]
async fn approvals_and_kanban_events_share_the_stream() {
    use futures_util::StreamExt;
    use sbxw_core::session::SessionKey;
    let w = world();
    let p = pair(&w, "phone").await;
    let sb = sbxw_core::SandboxRef::parse("web").unwrap();
    let script = "printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat > /dev/null".to_string();
    let (s, _) = w.app.sessions.get_or_open(&SessionKey::agent(&sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) }).await.unwrap();
    w.app.leads.lock().unwrap().insert(sb.clone(), Arc::as_ptr(&s) as usize);
    w.app.ingest_hook(&sb, json!({"hook_event_name": "SessionStart", "session_id": "s1", "hook_version": 2}));
    let req = Request::builder().uri("/r/events").header(header::HOST, TN).header(header::COOKIE, format!("{COOKIE}={}", p.token)).body(Body::empty()).unwrap();
    let mut body = w.router.clone().oneshot(req).await.unwrap().into_body().into_data_stream();
    let mut seen = String::new();
    assert!(until(&mut body, &mut seen, "\"plans\":0").await, "the badge at connect");
    // A plan arrives (a plan event), then a ticket on the leader (a kanban event).
    let id = pending_plan(&w, "web", "g");
    let plan = w.app.plan(&id).unwrap();
    w.app.bus.send(crate::bus::Event::Plan(plan));
    w.app.ticket_create(&sb, sbxw_core::tickets::Draft { title: Some("t".into()), description: None, priority: None }).unwrap();
    assert!(until(&mut body, &mut seen, "event: kanban").await, "the kanban nudge");
    assert!(until(&mut body, &mut seen, "\"plans\":1").await, "the badge moved");
    // Revoked: bye, and the stream ends although the badge task holds a sender.
    w.app.remote_revoke(&p.id, Kind::Revoked);
    assert!(until(&mut body, &mut seen, "event: bye").await);
    let ended = tokio::time::timeout(Duration::from_secs(5), async { while let Some(Ok(_)) = body.next().await {} }).await;
    assert!(ended.is_ok(), "the stream ended after bye");
}

async fn until(body: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin), seen: &mut String, want: &str) -> bool {
    use futures_util::StreamExt;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while !seen.contains(want) {
            match body.next().await {
                Some(Ok(b)) => seen.push_str(&String::from_utf8_lossy(&b)),
                _ => break,
            }
        }
    })
    .await;
    seen.contains(want)
}

/// Review P1 turned around, in this file's terms: a second client holding a
/// copy of the cookie cannot read, use or burn the person's grant.
#[tokio::test]
async fn a_copy_of_the_cookie_cannot_use_or_burn_the_persons_grant() {
    let w = world();
    let person = pair(&w, "iPhone").await;
    let thief = Phone { token: person.token.clone(), id: person.id.clone(), auth: Authenticator::new(), rid: Default::default() };
    grant(&w, &person).await;
    // The copy knows no request id: status says none, options and register refuse.
    assert_eq!(call(&w.router, "GET", "/r/passkey/status", Some(&thief.token), json!({})).await.json()["state"], "none");
    let r = register_with(&w, &thief, &Twist::default()).await;
    assert_eq!(r.json()["reason"], "no_grant", "{}", r.text());
    // Its attempts burned nothing: the person registers.
    assert_eq!(register_with(&w, &person, &Twist::default()).await.status, StatusCode::OK);
    let stored = w.app.remote_access.lock().unwrap().level2.passkeys.get(&person.id).unwrap().credential_id.clone();
    assert_eq!(stored, b64url_encode(&person.auth.id), "the person's key, not the copy's");
    // A copy that starts its own request voids the person's: the digits the
    // person types (from their own phone) then match nothing.
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&person.token), json!({})).await;
    let digits = r.json()["check"].as_str().unwrap().to_string();
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&thief.token), json!({})).await;
    if r.json()["check"].as_str().unwrap() != digits {
        assert!(w.app.remote_passkey_allow(&person.id, &digits).is_err());
    }
}

/// Review P2, both callers of the merge: the desktop's and web UI's
/// `plan_merge` call the script exactly as before (by name, no pins); the
/// phone's passes the approved tip and base.
#[tokio::test]
async fn the_desktop_merge_is_unpinned_and_the_phones_is_pinned() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let ws = w._dir.path().join("ws3");
    let id = proposed_merge(&w, &ws);
    let tip = git(&ws, &["rev-parse", "sbxw/alpha"]);
    let base = git(&ws, &["rev-parse", "main"]);
    let ch = challenge(&w, &p, "merge", &id, &format!("/r/merges/{id}"), None).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    let _ = act(&w, &p, &format!("/r/merges/{id}/merge"), a, None).await;
    let calls: Vec<String> = w.fake.log().into_iter().filter(|c| c.contains("sbxw-thread-merge")).collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].ends_with(&format!("sbxw-thread-merge ={} =alpha ={tip} ={base}", ws.display())), "{}", calls[0]);
    // The desktop's merge of another proposed merge: by name only.
    let ws4 = w._dir.path().join("ws4");
    let id2 = {
        let sb = sbxw_core::SandboxRef::parse("other").unwrap();
        std::fs::create_dir_all(&ws4).unwrap();
        git(&ws4, &["init", "-q", "-b", "main"]);
        std::fs::write(ws4.join("a"), "a").unwrap();
        git(&ws4, &["add", "."]);
        git(&ws4, &["commit", "-qm", "base"]);
        git(&ws4, &["branch", "sbxw/alpha"]);
        w.app.store.update(&sb, |r| r.workspace = Some(ws4.clone())).unwrap();
        let id = pending_plan(&w, "other", "g");
        let mut book = w.app.plans.lock().unwrap();
        let threads = book.get(&id).unwrap().threads.clone();
        book.approve(&id, threads, None, None, 2).unwrap();
        book.set_thread("other", "alpha", ThreadState::Done, "done", false, 3).unwrap();
        book.propose_merge(&id, "other", vec!["alpha".into()], "n", vec![], 4).unwrap();
        id
    };
    let _ = w.app.plan_merge(&id2, false, None).await;
    let last = w.fake.log().into_iter().rfind(|c| c.contains("sbxw-thread-merge")).unwrap();
    assert!(last.ends_with(&format!("sbxw-thread-merge ={} =alpha", ws4.display())), "{last}");
}

/// Review Q1: each Face ID request is journaled (device, time, kind), so a
/// flood from a copy of the cookie shows on the Mac; it carries no digits,
/// no request id, no target.
#[tokio::test]
async fn a_face_id_request_is_journaled_and_carries_nothing_else() {
    let w = world();
    let p = pair(&w, "phone").await;
    let r = call(&w.router, "POST", "/r/passkey/start", Some(&p.token), json!({})).await;
    let (rid, digits) = (r.json()["request_id"].as_str().unwrap().to_string(), r.json()["check"].as_str().unwrap().to_string());
    let e = journal(&w).into_iter().rfind(|e| e.kind == Kind::PasskeyRequested).expect("journaled");
    assert_eq!((e.device_id.as_str(), e.target.as_deref(), e.size), (p.id.as_str(), None, None));
    let text = std::fs::read_to_string(w.app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    let line = text.lines().rfind(|l| l.contains("passkey_requested")).unwrap();
    let v: Value = serde_json::from_str(line).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["at_ms", "device_id", "device_name", "kind"], "{line}");
    // Its only fields are the four above, so neither the digits nor the
    // request id can be in it.
    assert!(!text.contains(&rid), "{line}");
    let _ = digits;
}

/// The page builds both links with `encodeURIComponent`, so a real sbx id
/// (`network:85TMuE6`) reaches the listener as `network%3A85TMuE6`.
#[tokio::test]
async fn a_network_request_opens_and_answers_by_the_pages_encoded_link() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    w.sbx.lock().unwrap().approvals = vec![network_request("network:85TMuE6", "api.github.com:443", json!([{ "id": "allow", "label": "Allow" }, { "id": "dismiss", "label": "Dismiss" }]))];
    let list = call(&w.router, "GET", "/r/network", Some(&p.token), json!({})).await.json();
    assert_eq!(list["requests"][0]["id"], "network:85TMuE6");
    let d = call(&w.router, "GET", "/r/network/network%3A85TMuE6", Some(&p.token), json!({})).await;
    assert_eq!(d.status, StatusCode::OK, "{}", d.text());
    let ch = challenge(&w, &p, "network_respond", "network:85TMuE6", "/r/network/network%3a85TMuE6", Some("dismiss")).await.unwrap();
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = act(&w, &p, "/r/network/network%3A85TMuE6/respond", a, Some("dismiss")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(w.fake.log().iter().any(|c| c == "policy approval respond network:85TMuE6 --option dismiss"), "{:?}", w.fake.log());
    // Any other encoded byte is still refused, as is an id with one decoded.
    for bad in ["/r/network/network%2F85TMuE6", "/r/network/network%3A..%2Fx", "/r/network/network%3A85TMuE6%20", "/r/network/%6Eetwork:85TMuE6", "/r/network/network%253A85TMuE6", "/r/network/network%3A85TMuE6%"] {
        assert_eq!(call(&w.router, "GET", bad, Some(&p.token), json!({})).await.status, StatusCode::NOT_FOUND, "{bad}");
    }
}

// ── Plan approval with a model changed on the phone ──────────────────────

/// A challenge for approving plan `id` with model changes.
async fn challenge_models(w: &World, p: &Phone, id: &str, models: Value) -> Reply {
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&p.token), json!({})).await;
    call(&w.router, "POST", "/r/approvals/challenge", Some(&p.token), json!({ "kind": "plan_approve", "target": id, "digest": d.json()["digest"], "models": models })).await
}

fn challenge_bytes(r: &Reply) -> Vec<u8> {
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    b64url_decode(r.json()["public_key"]["challenge"].as_str().unwrap(), 64).unwrap()
}

async fn approve_with(w: &World, p: &Phone, id: &str, a: Value, models: Value) -> Reply {
    call(&w.router, "POST", &format!("/r/plans/{id}/approve"), Some(&p.token), json!({ "assertion": a, "models": models })).await
}

#[tokio::test]
async fn a_plan_is_approved_with_a_model_the_phone_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&p.token), json!({})).await.json();
    let listed: Vec<&str> = d["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert!(listed.contains(&"claude-haiku-5-5"), "the detail carries the configured list: {listed:?}");
    let ch = challenge_bytes(&challenge_models(&w, &p, &id, json!({ "alpha": "claude-haiku-5-5" })).await);
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = approve_with(&w, &p, &id, a, json!({ "alpha": "claude-haiku-5-5" })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let plan = w.app.plan(&id).unwrap();
    assert_eq!(plan.state, PlanState::Approved);
    assert_eq!(plan.threads[0].model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!((plan.threads[0].goal.as_str(), plan.threads[0].scope.as_str()), ("goal", "crates/x"), "nothing else of the thread changed");
}

#[tokio::test]
async fn a_model_not_in_the_list_or_on_another_thread_is_refused() {
    let w = world();
    let p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    for (models, what) in [
        (json!({ "alpha": "claude-made-up-9" }), "not in the list"),
        (json!({ "alpha": "Claude-Haiku-5-5" }), "not exactly in the list"),
        (json!({ "gamma": "claude-haiku-5-5" }), "not a thread of the plan"),
        (json!({ "alpha": 3 }), "not an id"),
    ] {
        let r = challenge_models(&w, &p, &id, models).await;
        assert!(r.status.is_client_error(), "{what}: {}", r.text());
        assert_ne!(r.json()["reason"], "changed", "{what}");
    }
    // Models on another action: refused.
    let d = call(&w.router, "GET", &format!("/r/plans/{id}"), Some(&p.token), json!({})).await;
    let r = call(&w.router, "POST", "/r/approvals/challenge", Some(&p.token), json!({ "kind": "plan_refuse", "target": id, "digest": d.json()["digest"], "models": { "alpha": "claude-haiku-5-5" } })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert_eq!(plan_state(&w, &id), PlanState::Pending);
}

#[tokio::test]
async fn a_model_changed_after_the_challenge_is_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let id = pending_plan(&w, "myapp", "goal");
    // Another model at the act than at the challenge.
    let ch = challenge_bytes(&challenge_models(&w, &p, &id, json!({ "alpha": "claude-haiku-5-5" })).await);
    let a = assertion(&mut p, &ch, &Twist::default());
    let r = approve_with(&w, &p, &id, a, json!({ "alpha": "claude-opus-5-5" })).await;
    assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::CONFLICT, Some("changed")), "{}", r.text());
    // The change dropped at the act.
    let ch = challenge_bytes(&challenge_models(&w, &p, &id, json!({ "alpha": "claude-haiku-5-5" })).await);
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(approve_with(&w, &p, &id, a, json!({})).await.json()["reason"], "changed");
    // A change added at the act to a challenge made without one.
    let ch = challenge_bytes(&challenge_models(&w, &p, &id, json!({})).await);
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(approve_with(&w, &p, &id, a, json!({ "alpha": "claude-haiku-5-5" })).await.json()["reason"], "changed");
    // The plan's proposed model changes on the Mac after the challenge.
    let ch = challenge_bytes(&challenge_models(&w, &p, &id, json!({ "alpha": "claude-haiku-5-5" })).await);
    {
        let mut book = w.app.plans.lock().unwrap();
        let mut list = book.list();
        list.iter_mut().filter(|pl| pl.id == id).for_each(|pl| pl.threads[0].model = Some("claude-opus-5-5".into()));
        *book = sbxw_core::plans::PlanBook::load(list);
    }
    let a = assertion(&mut p, &ch, &Twist::default());
    assert_eq!(approve_with(&w, &p, &id, a, json!({ "alpha": "claude-haiku-5-5" })).await.json()["reason"], "changed");
    assert_eq!(plan_state(&w, &id), PlanState::Pending, "nothing started");
    assert!(journal(&w).iter().filter(|e| e.kind == Kind::Changed).count() >= 4);
}
