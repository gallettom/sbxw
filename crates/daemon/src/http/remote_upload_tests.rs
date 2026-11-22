//! The phone's upload (API.md 2.6.1), end to end through the remote router:
//! the routes in both directions, ids bound to a device and a leader, the
//! exact bytes a message with files types, every refusal, the caps, the
//! kinds, the names, the logs and the journal, and that no answer of the
//! remote listener ever carries a path. Every test runs in a temp sbxw home.

use super::remote::{remote_router, ROUTES};
use super::remote_tests::{call, hook, leader, pair, send, typed, Capture, Reply, TN};
use crate::app::App;
use axum::body::{Body, Bytes};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::uploads::{SizeClass, FILE_MAX, TTL_MS};
use sbxw_core::remote_access::COOKIE;
use sbxw_sbx::runner::{Fake, Output};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn on() -> (tempfile::TempDir, Arc<App>, Arc<Fake>) {
    on_with(crate::testing::sbx_answer)
}

fn on_with(answer: impl Fn(&[String]) -> Output + Send + Sync + 'static) -> (tempfile::TempDir, Arc<App>, Arc<Fake>) {
    let (d, app, fake) = crate::testing::app_with(answer);
    app.remote_test_on(TN);
    (d, app, fake)
}

/// No answer of the remote listener names the upload folder, the spool,
/// or any path of the Mac's sbxw home.
fn clean(app: &App, r: &Reply) {
    let body = r.text();
    let spool = app.remote_uploads_for_tests().1;
    let home = app.remote_dir_for_tests();
    for leak in ["/tmp/sbxw-uploads", "sbxw-uploads/", &spool.display().to_string(), &home.display().to_string()] {
        assert!(!body.contains(leak), "{leak} in an answer: {body}");
    }
}

/// Percent-encode a name for `?name=`.
fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn up_req(sandbox: &str, name: Option<&str>, token: &str) -> axum::http::request::Builder {
    let q = name.map(|n| format!("?name={}", enc(n))).unwrap_or_default();
    Request::builder()
        .method("POST")
        .uri(format!("/r/leaders/{sandbox}/upload{q}"))
        .header(header::HOST, TN)
        .header(header::ORIGIN, format!("https://{TN}"))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::COOKIE, format!("{COOKIE}={token}"))
}

async fn go(app: &App, router: &Router, r: Request<Body>) -> Reply {
    let r = send(router, r).await;
    clean(app, &r);
    r
}

/// Upload `bytes` with a true `Content-Length`.
async fn upload(app: &App, router: &Router, token: &str, sandbox: &str, name: &str, bytes: &[u8]) -> Reply {
    let r = up_req(sandbox, Some(name), token).header(header::CONTENT_LENGTH, bytes.len()).body(Body::from(bytes.to_vec())).unwrap();
    go(app, router, r).await
}

/// Upload a stream of chunks with no `Content-Length`.
async fn upload_chunked(app: &App, router: &Router, token: &str, name: &str, chunks: Vec<Vec<u8>>) -> Reply {
    let s = futures_util::stream::iter(chunks.into_iter().map(|c| Ok::<_, std::io::Error>(Bytes::from(c))));
    go(app, router, up_req("lead1", Some(name), token).body(Body::from_stream(s)).unwrap()).await
}

fn id_of(r: &Reply) -> String {
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 32);
    id
}

fn reason(r: &Reply) -> (StatusCode, Value) {
    (r.status, r.json()["reason"].clone())
}

async fn send_msg<S: AsRef<str>>(app: &App, router: &Router, token: &str, sandbox: &str, text: &str, ids: &[S]) -> Reply {
    let ids: Vec<&str> = ids.iter().map(|s| s.as_ref()).collect();
    let r = call(router, "POST", &format!("/r/leaders/{sandbox}/message"), Some(token), json!({ "text": text, "attachments": ids })).await;
    clean(app, &r);
    r
}

/// The sandbox paths the fake `sbx exec` was asked to write, and the bytes.
fn copies(fake: &Fake) -> Vec<(String, Vec<u8>)> {
    let calls = fake.calls.lock().unwrap().clone();
    let stdins = fake.stdins.lock().unwrap().clone();
    // stdins are recorded only for calls that had one, in order.
    let mut out = vec![];
    let mut k = 0;
    for c in calls {
        let has_stdin = c.iter().any(|a| a == "-i");
        if has_stdin {
            if let Some(dest) = c.last().filter(|d| d.starts_with("/tmp/sbxw-uploads/")) {
                out.push((dest.clone(), stdins[k].clone()));
            }
            k += 1;
        }
    }
    out
}

fn spool_files(app: &App) -> usize {
    std::fs::read_dir(app.remote_uploads_for_tests().1).map(|d| d.count()).unwrap_or(0)
}

async fn device_id(router: &Router, token: &str) -> String {
    call(router, "GET", "/r/me", Some(token), json!({})).await.json()["device"]["id"].as_str().unwrap().to_string()
}

// ── Routes, both directions ──────────────────────────────────────────────

#[tokio::test]
async fn the_upload_routes_exist_only_on_the_remote_listener_and_only_with_a_device() {
    assert!(ROUTES.contains(&("POST", "/r/leaders/:sandbox/upload")));
    assert!(ROUTES.contains(&("DELETE", "/r/leaders/:sandbox/upload/:id")));
    assert!(!ROUTES.iter().any(|(m, p)| p.contains("upload") && *m == "GET"), "no route reads a file back");
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _ = leader(&app, d.path(), "lead1").await;
    // The UI listener has no /r/ route; the remote listener no UI upload.
    let ui = crate::http::ui_router(app.clone());
    for path in ["/r/leaders/lead1/upload?name=a.png", "/r/leaders/lead1/upload/00000000000000000000000000000000"] {
        let r = Request::builder().method("POST").uri(path).header(header::HOST, "127.0.0.1:7681").header(header::AUTHORIZATION, format!("Bearer {}", app.ui_token)).body(Body::from(PNG.to_vec())).unwrap();
        assert_eq!(send(&ui, r).await.status, StatusCode::NOT_FOUND, "{path} on the UI listener");
    }
    let r = Request::builder().method("POST").uri("/api/sandboxes/lead1/upload?name=a.png").header(header::HOST, TN).header(header::ORIGIN, format!("https://{TN}")).header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::from(PNG.to_vec())).unwrap();
    assert_eq!(go(&app, &router, r).await.status, StatusCode::NOT_FOUND, "the UI's upload is absent");
    // No Origin, another Origin, no cookie, the UI token: refused.
    let mk = || up_req("lead1", Some("a.png"), &token);
    let no_origin = mk().body(Body::from(PNG.to_vec())).map(|mut r| {
        r.headers_mut().remove(header::ORIGIN);
        r
    });
    assert_eq!(go(&app, &router, no_origin.unwrap()).await.status, StatusCode::FORBIDDEN);
    let other = mk().body(Body::from(PNG.to_vec())).map(|mut r| {
        r.headers_mut().insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        r
    });
    assert_eq!(go(&app, &router, other.unwrap()).await.status, StatusCode::FORBIDDEN);
    for cookie in [String::new(), format!("{COOKIE}={}", app.ui_token), format!("sbxw_ui={}", app.ui_token)] {
        let r = mk().body(Body::from(PNG.to_vec())).map(|mut r| {
            r.headers_mut().insert(header::COOKIE, cookie.parse().unwrap());
            r
        });
        assert_eq!(go(&app, &router, r.unwrap()).await.status, StatusCode::UNAUTHORIZED, "cookie {cookie:?}");
    }
    // DELETE needs the Origin too.
    let r = Request::builder().method("DELETE").uri("/r/leaders/lead1/upload/00000000000000000000000000000000").header(header::HOST, TN).header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    assert_eq!(go(&app, &router, r).await.status, StatusCode::FORBIDDEN);
    // JSON is not a file; a non-leader is not found.
    let r = mk().body(Body::from(PNG.to_vec())).map(|mut r| {
        r.headers_mut().insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        r
    });
    assert_eq!(go(&app, &router, r.unwrap()).await.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(upload(&app, &router, &token, "plain", "a.png", PNG).await.status, StatusCode::NOT_FOUND);
    assert_eq!(spool_files(&app), 0, "nothing stored by any refusal");
}

// ── A message with files ─────────────────────────────────────────────────

#[tokio::test]
async fn a_message_types_the_text_then_the_block_of_the_daemon_s_own_paths() {
    let (d, app, fake) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    let one = upload(&app, &router, &token, "lead1", "My photo (1).png", PNG).await;
    assert_eq!(one.json()["name"], "My_photo__1_.png");
    assert_eq!(one.json()["size"], PNG.len());
    assert!(one.json()["expires_ms"].as_u64().unwrap() > crate::app::now_ms() + TTL_MS - 60_000);
    let a = id_of(&one);
    let pdf = b"%PDF-1.7\n1 0 obj".to_vec();
    let b = id_of(&upload(&app, &router, &token, "lead1", "My photo (1).png", &pdf).await);
    assert!(copies(&fake).is_empty(), "an upload alone copies nothing into the sandbox");
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "an upload alone types nothing");
    assert_eq!(spool_files(&app), 2);

    let r = send_msg(&app, &router, &token, "lead1", "look at these", &[&a, &b]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let c = copies(&fake);
    assert_eq!(c.len(), 2, "{c:?}");
    let (d1, d2) = (&c[0].0, &c[1].0);
    assert!(d1.starts_with("/tmp/sbxw-uploads/") && d1.ends_with("/My_photo__1_.png"), "{d1}");
    assert_eq!(d2, &d1.replace("/My_photo__1_.png", "/2-My_photo__1_.png"), "same folder, the repeated name prefixed");
    assert_eq!((c[0].1.as_slice(), c[1].1.as_slice()), (PNG, pdf.as_slice()), "the bytes the phone sent");
    let want = format!("\x1b[200~look at these\n\nAttached files (uploaded to this sandbox):\n- {d1}\n- {d2}\x1b[201~\r");
    assert_eq!(String::from_utf8_lossy(&typed(&file, b"\x1b[201~\r").await), want);
    assert_eq!(spool_files(&app), 0, "spent: the spool files are gone");
    assert_eq!(app.remote_uploads_for_tests().0, 0);

    // Spent: a second use is refused and types nothing.
    let r = send_msg(&app, &router, &token, "lead1", "again", &[&a]).await;
    assert_eq!(reason(&r), (StatusCode::BAD_REQUEST, json!("attachment")), "{}", r.text());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(String::from_utf8_lossy(&std::fs::read(&file).unwrap()), want, "nothing more typed");

    // Files alone: the block only.
    let c3 = id_of(&upload(&app, &router, &token, "lead1", "note.txt", b"hello").await);
    let r = send_msg(&app, &router, &token, "lead1", "", &[&c3]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let d3 = copies(&fake)[2].0.clone();
    let tail = format!("\x1b[200~Attached files (uploaded to this sandbox):\n- {d3}\x1b[201~\r");
    assert!(String::from_utf8_lossy(&typed(&file, tail.as_bytes()).await).ends_with(&tail));
    // Neither text nor files: empty.
    assert_eq!(reason(&send_msg(&app, &router, &token, "lead1", "  ", &[] as &[&str]).await), (StatusCode::BAD_REQUEST, json!("empty")));

    // The history shows the prompt with its block, without the paths.
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": want.trim_start_matches("\x1b[200~").trim_end_matches("\x1b[201~\r")}));
    let h = call(&router, "GET", "/r/leaders/lead1/history", Some(&token), json!({})).await;
    clean(&app, &h);
    assert!(h.text().contains("- My_photo__1_.png") && h.text().contains("- 2-My_photo__1_.png"), "{}", h.text());
    for path in ["/r/leaders", "/r/me"] {
        clean(&app, &call(&router, "GET", path, Some(&token), json!({})).await);
    }
    let kinds: Vec<(Kind, Option<SizeClass>)> = app.remote_journal(100).iter().map(|e| (e.kind, e.size)).collect();
    assert_eq!(kinds.iter().filter(|k| **k == (Kind::Upload, Some(SizeClass::Under1Mb))).count(), 3, "{kinds:?}");
    assert!(kinds.iter().filter(|k| k.0 != Kind::Upload).all(|k| k.1.is_none()));
}

#[tokio::test]
async fn ids_of_another_device_or_leader_reused_expired_or_forged_are_refused() {
    let (d, app, fake) = on();
    let router = remote_router(app.clone());
    let ta = pair(&app, &router, "phone a").await;
    let tb = pair(&app, &router, "phone b").await;
    let (_, file) = leader(&app, d.path(), "lead1").await;
    let (_, file2) = leader(&app, d.path(), "lead2").await;
    let a1 = id_of(&upload(&app, &router, &ta, "lead1", "a.png", PNG).await);
    let a2 = id_of(&upload(&app, &router, &ta, "lead2", "a.png", PNG).await);
    let forged = "0123456789abcdef0123456789abcdef".to_string();
    let attachment = (StatusCode::BAD_REQUEST, json!("attachment"));
    assert_eq!(reason(&send_msg(&app, &router, &tb, "lead1", "hi", &[&a1]).await), attachment, "another device's id");
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead1", "hi", &[&a2]).await), attachment, "another leader's id");
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead1", "hi", &[&forged]).await), attachment, "forged");
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead1", "hi", &["x"]).await), attachment, "not even an id");
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead1", "hi", &[&a1, &a1]).await), attachment, "twice");
    let six: Vec<&str> = vec![a1.as_str(); 6];
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead1", "hi", &six).await), (StatusCode::BAD_REQUEST, json!("too_many")));
    // Another device cannot drop it either; its owner can.
    let del = |t: &str, sb: &str, id: &str| {
        let (t, sb, id) = (t.to_string(), sb.to_string(), id.to_string());
        let (app, router) = (app.clone(), router.clone());
        async move {
            let r = call(&router, "DELETE", &format!("/r/leaders/{sb}/upload/{id}"), Some(&t), json!({})).await;
            clean(&app, &r);
            r.status
        }
    };
    assert_eq!(del(&tb, "lead1", &a1).await, StatusCode::NOT_FOUND);
    assert_eq!(del(&ta, "lead2", &a1).await, StatusCode::NOT_FOUND, "through another leader");
    assert_eq!(del(&ta, "lead1", &forged).await, StatusCode::NOT_FOUND);
    // Expired.
    app.remote_uploads_tune_for_tests(TTL_MS, None);
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead2", "hi", &[&a2]).await), attachment, "expired");
    assert_eq!(del(&ta, "lead2", &a2).await, StatusCode::NOT_FOUND);
    app.remote_sweep_uploads();
    assert_eq!(spool_files(&app), 0, "expired files are removed from the Mac");
    // Dropped by its owner: gone.
    let a3 = id_of(&upload(&app, &router, &ta, "lead1", "b.png", PNG).await);
    assert_eq!(del(&ta, "lead1", &a3).await, StatusCode::OK);
    assert_eq!(spool_files(&app), 0);
    assert_eq!(reason(&send_msg(&app, &router, &ta, "lead1", "hi", &[&a3]).await), attachment, "dropped");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty() && std::fs::read(&file2).unwrap_or_default().is_empty(), "nothing typed");
    assert!(copies(&fake).is_empty(), "nothing copied");
}

#[tokio::test]
async fn a_message_with_files_is_refused_in_every_state_a_plain_one_is_and_keeps_its_ids() {
    let (d, app, fake) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    let id = id_of(&upload(&app, &router, &token, "lead1", "a.png", PNG).await);
    // The person's text is checked as a plain message's.
    assert_eq!(reason(&send_msg(&app, &router, &token, "lead1", "/login", &[&id]).await), (StatusCode::BAD_REQUEST, json!("command")));
    assert_eq!(reason(&send_msg(&app, &router, &token, "lead1", "ok\n!rm -rf /", &[&id]).await), (StatusCode::BAD_REQUEST, json!("command")));
    assert_eq!(reason(&send_msg(&app, &router, &token, "lead1", "\u{200b}#x", &[&id]).await), (StatusCode::BAD_REQUEST, json!("control")));
    // A question, then a tool call: needs_mac, busy. Nothing is copied.
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which?", "options": [{"label": "A"}, {"label": "B"}]}]}}));
    assert_eq!(reason(&send_msg(&app, &router, &token, "lead1", "A", &[&id]).await), (StatusCode::CONFLICT, json!("needs_mac")));
    hook(&app, &sb, "s1", json!({"hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "cargo test"}}));
    assert_eq!(reason(&send_msg(&app, &router, &token, "lead1", "hello", &[&id]).await), (StatusCode::CONFLICT, json!("busy")));
    assert!(copies(&fake).is_empty(), "a leader that takes no message gets no file");
    hook(&app, &sb, "s1", json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "done"}));
    // A prompt that appears while typing: not submitted, the id kept.
    let late = app.clone();
    let sb2 = sb.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        hook(&late, &sb2, "s1", json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}));
    });
    let r = send_msg(&app, &router, &token, "lead1", "keep going", &[&id]).await;
    assert_eq!(reason(&r), (StatusCode::CONFLICT, json!("not_submitted")), "{}", r.text());
    let c = copies(&fake);
    assert_eq!(c.len(), 1, "copied once, for the paste");
    let got = typed(&file, b"\x1b[201~").await;
    assert_eq!(String::from_utf8_lossy(&got), format!("\x1b[200~keep going\n\nAttached files (uploaded to this sandbox):\n- {}\x1b[201~", c[0].0), "the paste, never the Return");
    assert_eq!(app.remote_uploads_for_tests().0, 1, "the id is still usable");
    // Back at the prompt: the same id goes, the copy is not made again.
    hook(&app, &sb, "s1", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "done"}));
    let r = send_msg(&app, &router, &token, "lead1", "now", &[&id]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(copies(&fake).len(), 1, "a retry reuses the sandbox path");
    let tail = format!("\x1b[200~now\n\nAttached files (uploaded to this sandbox):\n- {}\x1b[201~\r", c[0].0);
    assert!(String::from_utf8_lossy(&typed(&file, tail.as_bytes()).await).ends_with(&tail));
    assert_eq!(app.remote_uploads_for_tests().0, 0, "spent on the Return");
}

#[tokio::test]
async fn a_copy_that_fails_types_nothing_and_keeps_the_id() {
    let failed = std::sync::atomic::AtomicBool::new(false);
    let (d, app, fake) = on_with(move |a: &[String]| {
        if a.iter().any(|x| x.starts_with("/tmp/sbxw-uploads/")) && !failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Output::fail(1, "disk full");
        }
        crate::testing::sbx_answer(a)
    });
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_, file) = leader(&app, d.path(), "lead1").await;
    let id = id_of(&upload(&app, &router, &token, "lead1", "a.png", PNG).await);
    let r = send_msg(&app, &router, &token, "lead1", "see", &[&id]).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY, "{}", r.text());
    assert!(!r.text().contains("disk full"), "no detail of the Mac: {}", r.text());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty());
    assert_eq!(send_msg(&app, &router, &token, "lead1", "see", &[&id]).await.status, StatusCode::OK);
    assert_eq!(copies(&fake).len(), 2, "the failed copy, then the good one");
}

// ── Caps, kinds, names ───────────────────────────────────────────────────

#[tokio::test]
async fn the_size_cap_holds_with_and_without_a_content_length_and_a_slow_body_is_cut() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _ = leader(&app, d.path(), "lead1").await;
    let too_large = (StatusCode::PAYLOAD_TOO_LARGE, json!("too_large"));
    // Declared over the cap: refused before a byte is read.
    let r = up_req("lead1", Some("big.png"), &token).header(header::CONTENT_LENGTH, FILE_MAX + 1).body(Body::from(PNG.to_vec())).unwrap();
    assert_eq!(reason(&go(&app, &router, r).await), too_large);
    // No Content-Length: counted while reading.
    let mut chunks = vec![PNG.to_vec()];
    chunks.extend((0..26).map(|_| vec![0u8; 1 << 20]));
    assert_eq!(reason(&upload_chunked(&app, &router, &token, "big.png", chunks).await), too_large);
    // A Content-Length that lies: the count still holds.
    let mut body = PNG.to_vec();
    body.resize(FILE_MAX as usize + 1, 0);
    let r = up_req("lead1", Some("big.png"), &token).header(header::CONTENT_LENGTH, 10).body(Body::from(body.clone())).unwrap();
    assert_eq!(reason(&go(&app, &router, r).await), too_large);
    assert_eq!(spool_files(&app), 0, "nothing kept of a refused body");
    // Exactly the cap: taken.
    body.truncate(FILE_MAX as usize);
    let r = upload(&app, &router, &token, "lead1", "exact.png", &body).await;
    assert_eq!(r.json()["size"], FILE_MAX, "{}", r.text());
    assert_eq!(app.remote_journal(5).last().and_then(|e| e.size), Some(SizeClass::Over10Mb));
    // A slow body: no byte for the gap allowed.
    app.remote_uploads_tune_for_tests(0, Some(Duration::from_millis(300)));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
    tx.send(Ok(Bytes::from_static(PNG))).unwrap();
    let rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    let r = go(&app, &router, up_req("lead1", Some("slow.png"), &token).body(Body::from_stream(rx)).unwrap()).await;
    assert_eq!(reason(&r), (StatusCode::REQUEST_TIMEOUT, json!("timeout")));
    drop(tx);
    assert_eq!(spool_files(&app), 1, "only the exact-cap file");
    // The slot is free again.
    assert_eq!(upload(&app, &router, &token, "lead1", "after.png", PNG).await.status, StatusCode::OK);
}

#[tokio::test]
async fn count_total_and_one_at_a_time_caps() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let ta = pair(&app, &router, "phone a").await;
    let tb = pair(&app, &router, "phone b").await;
    let _ = leader(&app, d.path(), "lead1").await;
    let _ = leader(&app, d.path(), "lead2").await;
    let mut ids = vec![];
    for i in 0..5 {
        ids.push(id_of(&upload(&app, &router, &ta, "lead1", &format!("{i}.png"), PNG).await));
    }
    let too_many = (StatusCode::CONFLICT, json!("too_many"));
    assert_eq!(reason(&upload(&app, &router, &ta, "lead1", "6.png", PNG).await), too_many);
    assert_eq!(upload(&app, &router, &tb, "lead1", "b.png", PNG).await.status, StatusCode::OK, "another device has its own five");
    assert_eq!(call(&router, "DELETE", &format!("/r/leaders/lead1/upload/{}", ids[0]), Some(&ta), json!({})).await.status, StatusCode::OK);
    assert_eq!(upload(&app, &router, &ta, "lead1", "6.png", PNG).await.status, StatusCode::OK, "room again");
    // 60 MiB per device and leader.
    let mut big = PNG.to_vec();
    big.resize(FILE_MAX as usize, 0);
    id_of(&upload(&app, &router, &ta, "lead2", "x.png", &big).await);
    id_of(&upload(&app, &router, &ta, "lead2", "y.png", &big).await);
    let mut eleven = PNG.to_vec();
    eleven.resize(11 << 20, 0);
    assert_eq!(reason(&upload(&app, &router, &ta, "lead2", "z.png", &eleven).await), too_many);
    // One upload at a time per device: a second one while the first streams.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
    tx.send(Ok(Bytes::from_static(PNG))).unwrap();
    let rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    let first = {
        let (app, router, tb) = (app.clone(), router.clone(), tb.clone());
        tokio::spawn(async move { go(&app, &router, up_req("lead2", Some("slow.png"), &tb).body(Body::from_stream(rx)).unwrap()).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(reason(&upload(&app, &router, &tb, "lead1", "c.png", PNG).await), (StatusCode::CONFLICT, json!("busy")));
    assert_eq!(upload(&app, &router, &ta, "lead1", "d.png", PNG).await.status, StatusCode::CONFLICT, "a is full on lead1, not busy");
    drop(tx);
    assert_eq!(first.await.unwrap().status, StatusCode::OK);
}

#[tokio::test]
async fn the_upload_rate_limit_is_per_device() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let ta = pair(&app, &router, "phone a").await;
    let tb = pair(&app, &router, "phone b").await;
    let _ = leader(&app, d.path(), "lead1").await;
    for i in 0..sbxw_core::remote_access::uploads::RATE_PER_DEVICE {
        let id = id_of(&upload(&app, &router, &ta, "lead1", &format!("{i}.png"), PNG).await);
        assert_eq!(call(&router, "DELETE", &format!("/r/leaders/lead1/upload/{id}"), Some(&ta), json!({})).await.status, StatusCode::OK);
    }
    let r = upload(&app, &router, &ta, "lead1", "x.png", PNG).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.headers.contains_key(header::RETRY_AFTER));
    assert_eq!(upload(&app, &router, &tb, "lead1", "x.png", PNG).await.status, StatusCode::OK, "the other device is not limited");
    assert_eq!(call(&router, "GET", "/r/leaders", Some(&ta), json!({})).await.status, StatusCode::OK, "only uploads are limited");
}

#[tokio::test]
async fn programs_archives_and_empty_files_are_refused_by_their_first_bytes() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _ = leader(&app, d.path(), "lead1").await;
    let mut tar = vec![b'a'; 600];
    tar[257..262].copy_from_slice(b"ustar");
    let refused: Vec<(&str, Vec<u8>)> = vec![
        ("elf", b"\x7fELF\x02\x01\x01".to_vec()),
        ("exe", b"MZ\x90\x00".to_vec()),
        ("macho", b"\xcf\xfa\xed\xfe\x07".to_vec()),
        ("fat", b"\xca\xfe\xba\xbe\0".to_vec()),
        ("zip", b"PK\x03\x04\x14\0".to_vec()),
        ("gzip", b"\x1f\x8b\x08\0".to_vec()),
        ("7z", b"7z\xbc\xaf\x27\x1c\0".to_vec()),
        ("tar", tar),
    ];
    for (what, bytes) in refused {
        // Named as a picture: the name never decides.
        assert_eq!(reason(&upload(&app, &router, &token, "lead1", "photo.png", &bytes).await), (StatusCode::BAD_REQUEST, json!("kind")), "{what}");
    }
    let empty = (StatusCode::BAD_REQUEST, json!("empty"));
    assert_eq!(reason(&upload(&app, &router, &token, "lead1", "e.txt", b"").await), empty);
    assert_eq!(reason(&upload_chunked(&app, &router, &token, "e.txt", vec![]).await), empty);
    assert_eq!(spool_files(&app), 0);
    for (name, bytes) in [("a.png", PNG.to_vec()), ("a.pdf", b"%PDF-1.4".to_vec()), ("run.sh", b"#!/bin/sh\necho hi\n".to_vec()), ("blob.bin", vec![0u8, 1, 2, 3, 250])] {
        assert_eq!(upload(&app, &router, &token, "lead1", name, &bytes).await.status, StatusCode::OK, "{name} is data");
    }
}

#[tokio::test]
async fn names_are_the_daemon_s_own() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _ = leader(&app, d.path(), "lead1").await;
    let long = "x".repeat(1000);
    let cases = [
        ("../../etc/passwd", "passwd"),
        ("a/b\\c.png", "c.png"),
        (".env", "env"),
        ("...", "upload"),
        ("..", "upload"),
        ("\u{0430}pple.png", "_pple.png"),
        ("x\u{ff0f}..\u{ff0f}y.png", "x_.._y.png"),
    ];
    for (i, (raw, want)) in cases.iter().enumerate() {
        let r = upload(&app, &router, &token, "lead1", raw, PNG).await;
        assert_eq!(r.json()["name"], *want, "{raw:?}: {}", r.text());
        if i % 4 == 3 {
            app.remote_access.lock().unwrap().uploads.clear();
        }
    }
    app.remote_access.lock().unwrap().uploads.clear();
    assert_eq!(upload(&app, &router, &token, "lead1", &long, PNG).await.json()["name"].as_str().unwrap().len(), 120);
    let name = (StatusCode::BAD_REQUEST, json!("name"));
    assert_eq!(reason(&upload(&app, &router, &token, "lead1", &"y".repeat(4000), PNG).await), name, "4,000 characters");
    assert_eq!(reason(&upload(&app, &router, &token, "lead1", "", PNG).await), name);
    let r = up_req("lead1", None, &token).body(Body::from(PNG.to_vec())).unwrap();
    assert_eq!(reason(&go(&app, &router, r).await), name, "no name at all");
}

// ── Logs, journal, off and revoke ────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn the_journal_and_the_logs_hold_no_name_and_no_content() {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(logs.clone());
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_, file) = leader(&app, d.path(), "lead1").await;
    // Other tests' threads may have cached a callsite's interest before this
    // capture existed: look again, so every line reaches it.
    tracing::callsite::rebuild_interest_cache();
    let id = id_of(&upload(&app, &router, &token, "lead1", "TOPSECRETNAME.txt", b"CONTENTSECRET 42").await);
    assert_eq!(send_msg(&app, &router, &token, "lead1", "read it", &[&id]).await.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&typed(&file, b"TOPSECRETNAME").await).contains("TOPSECRETNAME.txt"), "the leader is told");
    let id = id_of(&upload(&app, &router, &token, "lead1", "OTHERSECRET.png", PNG).await);
    assert_eq!(call(&router, "DELETE", &format!("/r/leaders/lead1/upload/{id}"), Some(&token), json!({})).await.status, StatusCode::OK);
    let journal = std::fs::read_to_string(app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    assert!(journal.contains("\"kind\":\"upload\"") && journal.contains("\"size\":\"under_1mb\""), "{journal}");
    let logged = logs.0.lock().unwrap().clone();
    assert!(logged.contains(": upload"), "the capture works: {logged}");
    for leak in ["TOPSECRETNAME", "OTHERSECRET", "CONTENTSECRET", "sbxw-uploads"] {
        assert!(!journal.contains(leak), "{leak} in the journal");
    }
    // Content: at no level. Names and paths: at none of the levels a log
    // file keeps (the sbx runner's own DEBUG line prints every command it
    // runs, the web UI's uploads included: see the report).
    assert!(!logged.contains("CONTENTSECRET"), "content in the logs:\n{logged}");
    for line in logged.lines().filter(|l| !l.starts_with("DEBUG") && !l.starts_with("TRACE")) {
        for leak in ["TOPSECRETNAME", "OTHERSECRET", "sbxw-uploads"] {
            assert!(!line.contains(leak), "{leak} in a log line: {line}");
        }
    }
}

#[tokio::test]
async fn disable_revoke_and_logout_drop_pending_uploads() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let ta = pair(&app, &router, "phone a").await;
    let tb = pair(&app, &router, "phone b").await;
    let tc = pair(&app, &router, "phone c").await;
    let _ = leader(&app, d.path(), "lead1").await;
    let a = id_of(&upload(&app, &router, &ta, "lead1", "a.png", PNG).await);
    id_of(&upload(&app, &router, &tb, "lead1", "b.png", PNG).await);
    id_of(&upload(&app, &router, &tc, "lead1", "c.png", PNG).await);
    assert_eq!(spool_files(&app), 3);
    assert!(app.remote_revoke(&device_id(&router, &ta).await, Kind::Revoked));
    assert_eq!((app.remote_uploads_for_tests().0, spool_files(&app)), (2, 2), "a's file went with a");
    assert_eq!(call(&router, "POST", "/r/logout", Some(&tc), json!({})).await.status, StatusCode::OK);
    assert_eq!((app.remote_uploads_for_tests().0, spool_files(&app)), (1, 1), "c logged out: its file went");
    app.remote_disable().await;
    assert_eq!((app.remote_uploads_for_tests().0, spool_files(&app)), (0, 0), "off: everything went");
    app.remote_test_on(TN);
    assert_eq!(call(&router, "POST", "/r/leaders/lead1/message", Some(&ta), json!({"text": "hi", "attachments": [a]})).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_daemon_restart_empties_the_spool() {
    let d = tempfile::tempdir().unwrap();
    let app = crate::testing::app_in(d.path());
    app.remote_test_on(TN);
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _ = leader(&app, d.path(), "lead1").await;
    id_of(&upload(&app, &router, &token, "lead1", "a.png", PNG).await);
    let spool = app.remote_uploads_for_tests().1;
    assert_eq!(std::fs::read_dir(&spool).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let f = std::fs::read_dir(&spool).unwrap().next().unwrap().unwrap();
        assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&spool).unwrap().permissions().mode() & 0o777, 0o700);
    }
    drop(router);
    let again = crate::testing::app_in(d.path());
    assert!(!again.remote_uploads_for_tests().1.exists(), "handles live in memory: an old spool is removed at start");
}

// ── A new ticket with files ──────────────────────────────────────────────

#[tokio::test]
async fn a_new_ticket_takes_uploads_and_a_bad_id_refuses_the_whole_create() {
    let (d, app, _) = on();
    let router = remote_router(app.clone());
    let ta = pair(&app, &router, "phone a").await;
    let tb = pair(&app, &router, "phone b").await;
    let _ = leader(&app, d.path(), "lead1").await;
    let _ = leader(&app, d.path(), "lead2").await;
    let path = "/r/leaders/lead1/kanban";
    let a1 = id_of(&upload(&app, &router, &ta, "lead1", "one.png", PNG).await);
    let a2 = id_of(&upload(&app, &router, &ta, "lead1", "two.png", PNG).await);
    let b1 = id_of(&upload(&app, &router, &tb, "lead1", "b.png", PNG).await);
    let other_leader = id_of(&upload(&app, &router, &ta, "lead2", "c.png", PNG).await);
    let forged = "0123456789abcdef0123456789abcdef".to_string();
    let count = |app: &App| app.tickets_board(Some(&sbxw_core::SandboxRef::parse("lead1").unwrap()), false).columns.iter().map(|c| c.cards.len()).sum::<usize>();
    for bad in [&b1, &other_leader, &forged] {
        let r = call(&router, "POST", path, Some(&ta), json!({ "title": "t", "attachments": [&a1, bad] })).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
        assert_eq!(count(&app), 0, "no ticket");
    }
    let six = vec![a1.as_str(); 6];
    assert_eq!(call(&router, "POST", path, Some(&ta), json!({ "title": "t", "attachments": six })).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(count(&app), 0);
    let r = call(&router, "POST", path, Some(&ta), json!({ "title": "With files", "attachments": [&a1, &a2] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["files"], 2);
    let n = r.json()["n"].as_u64().unwrap();
    let one = call(&router, "GET", &format!("{path}/{n}"), Some(&ta), json!({})).await.json();
    assert_eq!(one["ticket"]["files"].as_array().unwrap().len(), 2, "{one}");
    // Spent: the ids do not work twice, and the spool holds only the others.
    let r = call(&router, "POST", path, Some(&ta), json!({ "title": "again", "attachments": [&a1] })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(count(&app), 1);
    let journal = serde_json::to_string(&app.remote_journal(50)).unwrap();
    assert!(journal.contains("ticket_create") && !journal.contains("one.png"), "{journal}");
}
