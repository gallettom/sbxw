//! The Kanban on the phone (level 1 reads; one write, a new ticket): one running leader's board
//! and one of its tickets, on the remote listener. The contract is
//! `.sbxw-artifacts/remote/API-kanban.md`.
//!
//! Nothing is computed here: the answer is the core's board (and ticket)
//! as the Mac's routes give it, reduced to the fields a phone may see. What
//! is dropped: every write permission (`can`), plan ids, file ids and
//! on-disk names, timestamps, the upload limits, other sandboxes. A word of
//! free text that starts like an absolute path becomes `[path]`. Reading
//! journals nothing; a new ticket journals its kind only. Nothing here logs
//! a title or a text.

use super::error::ApiError;
use crate::app::App;
use crate::bus::Event;
use crate::svc::remote_access::DeviceCtx;
use axum::extract::{Path, State};
use axum::response::sse::Event as Sse;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbxw_core::tickets::{clip, MAX_DESCRIPTION, MAX_FILES, MAX_HISTORY, MAX_NAME, MAX_TITLE, TITLE_LONG, TITLE_SHORT};
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

type S = State<Arc<App>>;

/// A sandbox name as the phone sends it: at most this many bytes.
const NAME_MAX: usize = 96;
/// A ticket id as the phone sends it (`T-` and nine digits at most).
const TICKET_MAX: usize = 11;
/// A history line.
const HISTORY_LINE: usize = 200;
/// A mark's label or help, a thread's name or a column's text.
const SHORT: usize = 200;

fn not_found() -> Response {
    ApiError::not_found("not found").into_response()
}

/// `:sandbox` as a running leader's, by the rule `/r/leaders` uses. Every
/// failure is the same answer.
fn leader(app: &App, raw: &str) -> Option<SandboxRef> {
    if raw.is_empty() || raw.len() > NAME_MAX || !raw.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '~')) {
        return None;
    }
    let sb = app.remote_leader(raw).ok()?;
    app.remote_leader_names().contains(&sb.to_string()).then_some(sb)
}

/// `:ticket` as `T-<n>` or `<n>`: ASCII digits, 1 to 9, not zero.
fn ticket_number(raw: &str) -> Option<u32> {
    if raw.is_empty() || raw.len() > TICKET_MAX {
        return None;
    }
    let digits = raw.strip_prefix("T-").unwrap_or(raw);
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().filter(|n| *n > 0)
}

// ── What is dropped from free text ───────────────────────────────────────

/// Whether `rest` starts like an absolute path: `/x`, `~/`, a drive
/// (`C:\`, `C:/`), a UNC share (`\\server`), `$VAR/`. After a `:`, a `//`
/// is a URL's authority (`https://host/...`), not a path, unless a third
/// slash follows (`file:///Users/...`).
fn starts_path(rest: &str, after_colon: bool) -> bool {
    let b = rest.as_bytes();
    if after_colon && rest.starts_with("//") {
        return rest[2..].starts_with('/');
    }
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    let unc = rest.starts_with("\\\\") && b.len() >= 3 && b[2].is_ascii_alphanumeric();
    let var = b.first() == Some(&b'$') && b.get(1).is_some_and(|c| c.is_ascii_uppercase() || *c == b'_') && {
        let name: usize = b[1..].iter().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || **c == b'_').count();
        name > 0 && b.get(1 + name) == Some(&b'/')
    };
    (b.len() >= 2 && b[0] == b'/') || rest.starts_with("~/") || drive || unc || var
}

/// `s` with every word that holds something shaped like an absolute path
/// cut from where that path starts: at the word's start or after `://`,
/// `=`, `:`, a quote, a bracket, `*` or `,` (`file:///Users/x` becomes
/// `file:[path]`, `key=/Users/x` becomes `key=[path]`). A best-effort filter
/// on the shape of a word, not a guarantee that no path leaves.
pub fn scrub(s: &str) -> String {
    const BEFORE: &[char] = &['(', '[', '{', '<', '"', '\'', '`', '=', ':', '*', ','];
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let mut cut = None;
        let mut prev: Option<char> = None;
        for (i, c) in word.char_indices() {
            let candidate = prev.is_none_or(|p| BEFORE.contains(&p));
            if candidate && starts_path(&word[i..], prev == Some(':')) {
                cut = Some(i);
                break;
            }
            prev = Some(c);
        }
        match cut {
            Some(i) => {
                out.push_str(&word[..i]);
                out.push_str("[path]");
            }
            None => out.push_str(word),
        }
        word.clear();
    };
    for c in s.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn text(v: &Value, key: &str, max: usize) -> Value {
    Value::String(clip(&scrub(v.get(key).and_then(Value::as_str).unwrap_or("")), max))
}

fn pick(v: &Value, key: &str) -> Value {
    v.get(key).cloned().unwrap_or(Value::Null)
}

/// One card, the fields a phone shows.
fn card(v: &Value) -> Value {
    let n = v.get("n").and_then(Value::as_u64);
    let thread = v.get("thread").and_then(Value::as_str).map(|t| clip(t, SHORT));
    let key = match (n, &thread) {
        (Some(n), _) => format!("t:{n}"),
        (None, Some(t)) => format!("th:{t}"),
        _ => "th:".into(),
    };
    let marks: Vec<Value> = v
        .get("marks")
        .and_then(Value::as_array)
        .map(|m| {
            m.iter()
                .take(12)
                .map(|m| json!({ "id": text(m, "id", 40), "label": text(m, "label", SHORT), "tone": text(m, "tone", 20), "help": text(m, "help", SHORT) }))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "key": key,
        "n": n,
        "num": text(v, "num", 12),
        "column": pick(v, "column"),
        "title": text(v, "title", MAX_TITLE),
        "title_short": text(v, "title_short", TITLE_SHORT),
        "title_long": text(v, "title_long", TITLE_LONG),
        "source": pick(v, "source"),
        "priority": pick(v, "priority"),
        "thread": thread,
        "thread_state": pick(v, "thread_state"),
        "thread_tone": text(v, "thread_tone", 20),
        "marks": marks,
        "files": v.get("files").and_then(Value::as_u64).unwrap_or(0).min(MAX_FILES as u64),
        "validated": v.get("validated").and_then(Value::as_bool).unwrap_or(false),
        "age": text(v, "age", 12),
    })
}

/// The board of one sandbox as the phone reads it, with its `latest`.
pub fn board_view(app: &App, sb: &SandboxRef) -> Value {
    let b = serde_json::to_value(app.tickets_board(Some(sb), false)).unwrap_or(Value::Null);
    let columns: Vec<Value> = b
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .map(|c| {
                    let cards: Vec<Value> = c.get("cards").and_then(Value::as_array).map(|a| a.iter().map(card).collect()).unwrap_or_default();
                    json!({
                        "id": pick(c, "id"), "title": text(c, "title", SHORT), "tone": text(c, "tone", 20), "help": text(c, "help", SHORT), "empty": text(c, "empty", SHORT),
                        "count": c.get("count").and_then(Value::as_u64).unwrap_or(0), "older": c.get("older").and_then(Value::as_u64).unwrap_or(0), "cards": cards,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = json!({ "sandbox": sb.to_string(), "summary": text(&b, "counts_line", 400), "columns": columns });
    let latest = latest_of(&out);
    out["latest"] = Value::String(latest);
    out
}

/// A hash of the board without the ages: it moves when what a card shows
/// moves. 16 hex characters of FNV-1a.
fn latest_of(board: &Value) -> String {
    let mut b = board.clone();
    if let Some(cols) = b.get_mut("columns").and_then(Value::as_array_mut) {
        for c in cols {
            if let Some(cards) = c.get_mut("cards").and_then(Value::as_array_mut) {
                for card in cards {
                    if let Some(o) = card.as_object_mut() {
                        o.remove("age");
                    }
                }
            }
        }
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in b.to_string().bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn ticket_view(app: &App, sb: &SandboxRef, n: u32) -> Option<Value> {
    let v = app.ticket_view(sb, n)?;
    let card = card(v.get("card")?);
    let files: Vec<Value> = v
        .get("files")
        .and_then(Value::as_array)
        .map(|f| f.iter().take(MAX_FILES).map(|f| json!({ "name": text(f, "name", MAX_NAME), "size": f.get("bytes").and_then(Value::as_u64).unwrap_or(0), "kind": pick(f, "kind") })).collect())
        .unwrap_or_default();
    let history: Vec<Value> = v
        .get("history")
        .and_then(Value::as_array)
        .map(|h| {
            let skip = h.len().saturating_sub(MAX_HISTORY);
            h.iter().skip(skip).map(|s| json!({ "when": text(s, "when", 12), "text": text(s, "text", HISTORY_LINE), "tone": text(s, "tone", 20) })).collect()
        })
        .unwrap_or_default();
    Some(json!({
        "n": n,
        "num": text(&v, "num", 12),
        "title": text(&v, "title", MAX_TITLE),
        "description": text(&v, "description", MAX_DESCRIPTION),
        "priority": pick(&v, "priority"),
        "source": pick(&v, "source"),
        "column": card["column"].clone(),
        "card": card,
        "files": files,
        "history": history,
    }))
}

// ── Routes ───────────────────────────────────────────────────────────────

pub async fn board(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(sandbox): Path<String>) -> Response {
    let Some(sb) = leader(&app, &sandbox) else { return not_found() };
    Json(board_view(&app, &sb)).into_response()
}

/// A new ticket's body: 8000 characters of description at four bytes each,
/// the title and the JSON.
pub const BODY_MAX: usize = 40 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTicket {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    attachments: Vec<String>,
}

/// A ticket from the phone, in Planned: the Mac's writer, the text masked.
/// `attachments`: upload ids, at most as many as a message takes.
pub async fn create(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Json(b): Json<NewTicket>) -> Response {
    let Some(sb) = leader(&app, &sandbox) else { return not_found() };
    if b.attachments.len() > sbxw_core::remote_access::uploads::WAITING_MAX {
        return super::remote::refusal(crate::svc::remote_access::Refusal::Reason(sbxw_core::remote_access::Reason::TooManyIds));
    }
    match app.remote_ticket_create(&d, &sb, &b.title, &b.description, &b.attachments) {
        Ok(v) => Json(v).into_response(),
        Err(e) => super::remote::refusal(e),
    }
}

pub async fn ticket(State(app): S, Extension(_d): Extension<DeviceCtx>, Path((sandbox, ticket)): Path<(String, String)>) -> Response {
    let Some(sb) = leader(&app, &sandbox) else { return not_found() };
    let Some(n) = ticket_number(&ticket) else { return not_found() };
    match ticket_view(&app, &sb, n) {
        Some(t) => Json(json!({ "sandbox": sb.to_string(), "ticket": t })).into_response(),
        None => not_found(),
    }
}

// ── The event ────────────────────────────────────────────────────────────

/// What each leader's board looked like when this stream last told the
/// page: the hash only.
pub type Seen = HashMap<String, String>;

fn hash_of(app: &App, sandbox: &str) -> Option<String> {
    let sb = SandboxRef::parse(sandbox).ok()?;
    Some(board_view(app, &sb)["latest"].as_str()?.to_string())
}

/// The state of every leader's board as a stream opens: no event is sent
/// for it.
pub fn baseline(app: &App, leaders: &[crate::svc::remote_access::LeaderView]) -> Seen {
    leaders.iter().filter(|l| l.kind == sbxw_core::remote_access::reach::ReachKind::Leader).filter_map(|l| Some((l.sandbox.clone(), hash_of(app, &l.sandbox)?))).collect()
}

/// The `kanban` frame for a bus event, when it is a change of a listed
/// leader's board and of nothing else. `{sandbox, latest}` only.
pub fn changed(app: &App, leaders: &[crate::svc::remote_access::LeaderView], ev: &Event, seen: &mut Seen) -> Option<Sse> {
    let sandbox = match ev {
        Event::Tickets(t) => t.sandbox.as_str(),
        Event::Plan(p) => p.sandbox.as_str(),
        _ => return None,
    };
    if !leaders.iter().any(|l| l.sandbox == sandbox && l.kind == sbxw_core::remote_access::reach::ReachKind::Leader) {
        return None;
    }
    let now = hash_of(app, sandbox)?;
    if seen.get(sandbox) == Some(&now) {
        return None;
    }
    seen.insert(sandbox.to_string(), now.clone());
    Some(Sse::default().event("kanban").data(json!({ "sandbox": sandbox, "latest": now }).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::remote::{remote_router, ROUTES};
    use crate::testing::app;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use axum::Router;
    use futures_util::StreamExt;
    use sbxw_core::plans::{PlanThread, ThreadState, Variant};
    use sbxw_core::remote_access::COOKIE;
    use sbxw_core::session::SessionKey;
    use sbxw_core::tickets::Draft;
    use std::time::Duration;
    use tower::ServiceExt;

    const TN: &str = "sbxw.tail1.ts.net";
    const PHONE: &str = "100.64.0.10";

    fn on() -> (tempfile::TempDir, Arc<App>) {
        let (d, app) = app();
        app.remote_test_on(TN);
        (d, app)
    }

    fn req(method: &str, path: &str, cookie: Option<&str>) -> axum::http::request::Builder {
        let m: Method = method.parse().unwrap();
        let mut b = Request::builder().method(m.clone()).uri(path).header(header::HOST, TN).header("x-forwarded-for", PHONE);
        if m != Method::GET && m != Method::HEAD {
            b = b.header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(t) = cookie {
            b = b.header(header::COOKIE, format!("{COOKIE}={t}"));
        }
        b
    }

    struct Reply {
        status: StatusCode,
        headers: axum::http::HeaderMap,
        body: String,
    }
    impl Reply {
        fn json(&self) -> Value {
            serde_json::from_str(&self.body).unwrap_or(Value::Null)
        }
    }

    async fn call(router: &Router, method: &str, path: &str, cookie: Option<&str>) -> Reply {
        let r = req(method, path, cookie).body(Body::from(if method == "GET" { "" } else { "{}" })).unwrap();
        let res = router.clone().oneshot(r).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let body = String::from_utf8_lossy(&axum::body::to_bytes(res.into_body(), 16 << 20).await.unwrap()).into_owned();
        Reply { status, headers, body }
    }

    async fn pair(app: &Arc<App>, router: &Router) -> String {
        let code = app.remote_pairing_code().unwrap().code;
        let r = req("POST", "/r/pair/start", None).body(Body::from(json!({ "code": code, "device_name": "phone" }).to_string())).unwrap();
        let v: Value = serde_json::from_slice(&axum::body::to_bytes(router.clone().oneshot(r).await.unwrap().into_body(), 1 << 20).await.unwrap()).unwrap();
        let (id, check) = (v["request_id"].as_str().unwrap().to_string(), v["check"].as_str().unwrap().to_string());
        app.remote_pairing_approve(&id, &check).unwrap();
        let res = router.clone().oneshot(req("GET", &format!("/r/pair/status?request_id={id}"), None).body(Body::empty()).unwrap()).await.unwrap();
        let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
        cookie[COOKIE.len() + 1..].split(';').next().unwrap().to_string()
    }

    async fn lead(app: &Arc<App>, name: &str) -> SandboxRef {
        let sb = SandboxRef::parse(name).unwrap();
        let script = "printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat > /dev/null".to_string();
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
        app.leads.lock().unwrap().insert(sb.clone(), Arc::as_ptr(&s) as usize);
        let mut ev = json!({"hook_event_name": "SessionStart"});
        ev["session_id"] = "s1".into();
        ev["hook_version"] = 2.into();
        app.ingest_hook(&sb, ev);
        sb
    }

    fn ticket(app: &App, sb: &SandboxRef, title: &str, desc: &str) -> u32 {
        app.ticket_create(sb, Draft { title: Some(title.into()), description: Some(desc.into()), priority: None }).unwrap()["n"].as_u64().unwrap() as u32
    }

    fn png() -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend([0u8; 64]);
        b
    }

    /// Everything a phone must never see in an answer.
    fn assert_clean(body: &str) {
        for bad in ["\"/", "/Users/", "/home/", "/tmp/", "sbxw-uploads", ".sbxw-worktrees", "worktree", "\"file\":", "\"id\":\"f-", "added_ms", "\"can\"", "\"plan\":", "\"link\":", "created_ms", "updated_ms"] {
            assert!(!body.contains(bad), "{bad:?} in {body}");
        }
    }

    #[test]
    fn scrub_replaces_words_that_start_like_a_path_and_nothing_else() {
        assert_eq!(scrub("fix /Users/me/dev/x now"), "fix [path] now");
        assert_eq!(scrub("see (~/notes/a.txt) and \"/tmp/x\""), "see ([path] and \"[path]");
        assert_eq!(scrub(r"C:\Users\me and D:/x"), "[path] and [path]");
        assert_eq!(scrub("https://example.org/a and a/b and 1 / 2"), "https://example.org/a and a/b and 1 / 2");
        assert_eq!(scrub("line one\n  /etc/passwd\nend"), "line one\n  [path]\nend");
        // Inside a word (review P5), and the shapes it now knows.
        for (raw, want) in [
            ("open file:///Users/someone/.ssh/id_rsa", "open file:[path]"),
            ("path=/Users/someone/secret", "path=[path]"),
            ("see *~/notes/plan.md*", "see *[path]"),
            ("at \\\\server\\share\\x", "at [path]"),
            ("in $HOME/.aws/credentials", "in [path]"),
            ("key:/var/x and a,/tmp/y", "key:[path] and a,[path]"),
        ] {
            assert_eq!(scrub(raw), want, "{raw:?}");
        }
        // Not paths: URLs, ratios, plain words with a slash, prices.
        for keep in ["https://example.org/a/b", "http://x.y:8080/z", "10:30", "a=b", "and/or", "$5/month", "1/2"] {
            assert_eq!(scrub(keep), keep);
        }
    }

    #[test]
    fn ticket_ids_are_strict() {
        assert_eq!(ticket_number("T-12"), Some(12));
        assert_eq!(ticket_number("12"), Some(12));
        for bad in ["", "T-", "T-0", "0", "t-1", "T12", "T-1x", " 1", "1 ", "-1", "T--1", "1234567890", "T-1234567890", "٣", "%31", "1;2", "../1"] {
            assert_eq!(ticket_number(bad), None, "{bad:?}");
        }
    }

    async fn post_json(router: &Router, path: &str, token: &str, body: Value) -> Reply {
        let r = req("POST", path, Some(token)).body(Body::from(body.to_string())).unwrap();
        let res = router.clone().oneshot(r).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let body = String::from_utf8_lossy(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).into_owned();
        Reply { status, headers, body }
    }

    #[tokio::test]
    async fn a_paired_device_creates_a_ticket_in_planned() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let sb = lead(&app, "web").await;
        let path = "/r/leaders/web/kanban";

        let ok = post_json(&router, path, &token, json!({ "title": "  Add a search box ", "description": "On the members page" })).await;
        assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
        let n = ok.json()["n"].as_u64().unwrap() as u32;
        assert_eq!(ok.json()["num"], format!("T-{n}"));
        let one = call(&router, "GET", &format!("{path}/{n}"), Some(&token)).await;
        assert_eq!(one.json()["ticket"]["title"], "Add a search box");
        assert_eq!(one.json()["ticket"]["description"], "On the members page");
        assert_eq!(one.json()["ticket"]["column"], "planned");
        let board = call(&router, "GET", path, Some(&token)).await.json();
        let planned = board["columns"].as_array().unwrap().iter().find(|c| c["id"] == "planned").unwrap();
        assert_eq!(planned["count"], 1, "{board}");
        let _ = sb;

        // The journal names the kind, never the text.
        let journal = serde_json::to_string(&app.remote_journal(50)).unwrap();
        assert!(journal.contains("ticket_create") && !journal.contains("search box"), "{journal}");

        // Refused: an unknown field, no title, a title over the cap, no body at all.
        let r = post_json(&router, path, &token, json!({ "title": "x", "priority": "high" })).await;
        assert!(r.status.is_client_error() && r.status != StatusCode::NOT_FOUND, "{} {}", r.status, r.body);
        assert_eq!(post_json(&router, path, &token, json!({ "title": "   " })).await.status, StatusCode::BAD_REQUEST);
        assert_eq!(post_json(&router, path, &token, json!({ "title": "x".repeat(MAX_TITLE + 1) })).await.status, StatusCode::BAD_REQUEST);
        assert!(post_json(&router, path, &token, json!({})).await.status.is_client_error());
        assert_eq!(call(&router, "GET", path, Some(&token)).await.json()["columns"].as_array().unwrap().iter().find(|c| c["id"] == "planned").unwrap()["count"], 1, "nothing was added");

        // An unknown sandbox, and one with no team, are the same 404; no cookie is 401.
        assert_eq!(post_json(&router, "/r/leaders/nope/kanban", &token, json!({ "title": "x" })).await.status, StatusCode::NOT_FOUND);
        assert_eq!(post_json(&router, "/r/leaders/..%2Fx/kanban", &token, json!({ "title": "x" })).await.status, StatusCode::NOT_FOUND);
        let r = req("POST", path, None).body(Body::from(json!({ "title": "x" }).to_string())).unwrap();
        assert_eq!(router.clone().oneshot(r).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_paired_device_reads_the_board_and_a_ticket_of_a_running_leader() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let sb = lead(&app, "web").await;
        let n = ticket(&app, &sb, "Paginate the members /Users/me/dev/secret", "Line one\nsee /home/agent/x and ~/notes\n<img src=x onerror=alert(1)>");
        app.ticket_attach(&sb, n, "..//etc/shot.png", &png()).unwrap();
        // A plan working on it, so the card has a thread.
        let now = crate::app::now_ms();
        let th = PlanThread { name: "pages".into(), goal: "Build pages in /Users/me/dev/x".into(), scope: String::new(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket: Some(n) };
        let p = app.plans.lock().unwrap().propose("p-1".into(), "web", "s", vec![th], Variant::Split, "why", None, now).unwrap();
        app.tickets_follow(&p);

        let r = call(&router, "GET", "/r/leaders/web/kanban", Some(&token)).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
        assert!(r.headers.keys().all(|k| !k.as_str().starts_with("access-control-")));
        assert_clean(&r.body);
        let b = r.json();
        assert_eq!(b["sandbox"], "web");
        assert_eq!(b["latest"].as_str().unwrap().len(), 16);
        let cols: Vec<&str> = b["columns"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(cols, ["planned", "progress", "test", "ready", "merged"]);
        let c = &b["columns"][0]["cards"][0];
        assert_eq!((c["num"].as_str(), c["n"].as_u64(), c["thread"].as_str()), (Some("T-1"), Some(1), Some("pages")));
        assert_eq!(c["marks"][0]["label"], "proposed");
        assert_eq!(c["files"], 1);
        assert!(c["title"].as_str().unwrap().contains("[path]"));

        let r = call(&router, "GET", &format!("/r/leaders/web/kanban/T-{n}"), Some(&token)).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        assert_clean(&r.body);
        let t = &r.json()["ticket"];
        assert_eq!((t["num"].as_str(), t["column"].as_str()), (Some("T-1"), Some("planned")));
        assert!(t["description"].as_str().unwrap().contains("Line one\nsee [path] and [path]"));
        assert!(t["description"].as_str().unwrap().contains("<img src=x onerror=alert(1)>"), "data stays data, the page escapes it");
        assert_eq!(t["files"].as_array().unwrap().len(), 1);
        let f = t["files"][0].as_object().unwrap();
        assert_eq!(f.keys().collect::<Vec<_>>(), ["kind", "name", "size"]);
        assert_eq!((f["name"].as_str(), f["kind"].as_str(), f["size"].as_u64()), (Some("shot.png"), Some("png"), Some(72)));
        assert!(!t["history"].as_array().unwrap().is_empty());
        // The bare number is the same ticket.
        assert_eq!(call(&router, "GET", &format!("/r/leaders/web/kanban/{n}"), Some(&token)).await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn anything_but_a_running_leader_is_the_same_404() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let web = lead(&app, "web").await;
        ticket(&app, &web, "mine", "");
        // A sandbox with tickets that leads nothing, and one that never existed.
        let idle = SandboxRef::parse("idle").unwrap();
        ticket(&app, &idle, "theirs", "");
        let long = "a".repeat(4000);
        let mut paths: Vec<String> = ["idle", "ghost", "we%62", "we%20b", "web%2F..", "..", "a~b~c", "WEB!", "web;x", &long].iter().map(|s| format!("/r/leaders/{s}/kanban")).collect();
        paths.push("/r/leaders/idle/kanban/1".into());
        paths.push(format!("/r/leaders/{long}/kanban/1"));
        let mut bodies = std::collections::BTreeSet::new();
        for p in &paths {
            let r = call(&router, "GET", p, Some(&token)).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{p}");
            bodies.insert(r.body);
        }
        assert_eq!(bodies.len(), 1, "one body whatever the reason: {bodies:?}");
        // Stopped: the leader ends, the same answer.
        assert_eq!(call(&router, "GET", "/r/leaders/web/kanban", Some(&token)).await.status, StatusCode::OK);
        app.leads.lock().unwrap().clear();
        let r = call(&router, "GET", "/r/leaders/web/kanban", Some(&token)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert!(bodies.contains(&r.body));
    }

    #[tokio::test]
    async fn a_ticket_must_be_this_sandboxes() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let web = lead(&app, "web").await;
        let other = lead(&app, "api").await;
        ticket(&app, &web, "one", "");
        ticket(&app, &web, "two", "");
        ticket(&app, &other, "api only", "");
        // api has T-1 only; web has T-1 and T-2.
        assert_eq!(call(&router, "GET", "/r/leaders/api/kanban/2", Some(&token)).await.status, StatusCode::NOT_FOUND);
        let a = call(&router, "GET", "/r/leaders/api/kanban/1", Some(&token)).await;
        assert!(a.body.contains("api only") && !a.body.contains("\"one\""));
        for bad in ["99", "T-0", "T-", "t-1", "1x", "T-1/files", "..", "%31", "1234567890", "T-99999999999999999999"] {
            let r = call(&router, "GET", &format!("/r/leaders/web/kanban/{bad}"), Some(&token)).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{bad}");
            assert!(!r.body.contains("api only"));
        }
        // No file route, whatever the shape.
        for p in ["/r/leaders/web/kanban/1/files", "/r/leaders/web/kanban/1/files/f-0123456789abcdef", "/r/leaders/web/kanban/1/files/x.png"] {
            assert_eq!(call(&router, "GET", p, Some(&token)).await.status, StatusCode::NOT_FOUND, "{p}");
        }
    }

    #[tokio::test]
    async fn answers_are_bounded() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let sb = lead(&app, "web").await;
        let n = ticket(&app, &sb, &"T".repeat(600), &"d".repeat(20_000));
        for i in 0..25 {
            app.ticket_attach(&sb, n, &format!("{}-{i}.png", "n".repeat(300)), &png()).ok();
        }
        let r = call(&router, "GET", &format!("/r/leaders/web/kanban/{n}"), Some(&token)).await.json();
        let t = &r["ticket"];
        assert!(t["title"].as_str().unwrap().chars().count() <= MAX_TITLE);
        assert!(t["description"].as_str().unwrap().chars().count() <= MAX_DESCRIPTION);
        assert!(t["files"].as_array().unwrap().len() <= MAX_FILES);
        assert!(t["files"].as_array().unwrap().iter().all(|f| f["name"].as_str().unwrap().chars().count() <= MAX_NAME));
        assert!(t["history"].as_array().unwrap().len() <= MAX_HISTORY);
        let b = call(&router, "GET", "/r/leaders/web/kanban", Some(&token)).await.json();
        let c = &b["columns"][0]["cards"][0];
        assert!(c["title_short"].as_str().unwrap().chars().count() <= TITLE_SHORT);
        assert!(c["title_long"].as_str().unwrap().chars().count() <= TITLE_LONG);
    }

    #[tokio::test]
    async fn the_door_is_the_same_door_and_read_only() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let sb = lead(&app, "web").await;
        ticket(&app, &sb, "one", "");
        let paths = ["/r/leaders/web/kanban", "/r/leaders/web/kanban/1"];
        for p in paths {
            for m in ["POST", "PUT", "PATCH", "DELETE"] {
                // The one write: a new ticket, POST on the board (its own test).
                if m == "POST" && p == "/r/leaders/web/kanban" {
                    continue;
                }
                let r = call(&router, m, p, Some(&token)).await;
                assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{m} {p}");
            }
            // No token, the UI token, an agent-style token, a bad Host and a bad Origin.
            assert_eq!(call(&router, "GET", p, None).await.status, StatusCode::UNAUTHORIZED);
            for (name, value) in [("authorization", format!("Bearer {}", app.ui_token)), ("cookie", format!("sbxw_ui={}", app.ui_token)), ("cookie", format!("{COOKIE}={}", app.ui_token))] {
                let r = req("GET", p, None).header(name, value).body(Body::empty()).unwrap();
                assert_eq!(router.clone().oneshot(r).await.unwrap().status(), StatusCode::UNAUTHORIZED, "{p}");
            }
            let r = Request::builder().uri(p).header(header::HOST, "evil.example").header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
            assert_eq!(router.clone().oneshot(r).await.unwrap().status(), StatusCode::FORBIDDEN);
            let r = req("GET", p, Some(&token)).header(header::ORIGIN, "https://evil.example").body(Body::empty()).unwrap();
            assert_eq!(router.clone().oneshot(r).await.unwrap().status(), StatusCode::FORBIDDEN);
            assert_eq!(call(&router, "GET", p, Some(&token)).await.status, StatusCode::OK);
        }
        // Listed, and only on this listener.
        for p in ["/r/leaders/:sandbox/kanban", "/r/leaders/:sandbox/kanban/:ticket"] {
            assert!(ROUTES.contains(&("GET", p)), "{p}");
            let others = ROUTES.iter().filter(|(m, q)| *q == p && *m != "GET").count();
            assert_eq!(others, usize::from(p.ends_with("kanban")), "only the board takes a POST: {p}");
        }
        // The UI's ticket routes stay unreachable from here.
        for p in ["/api/tickets", "/api/tickets?sandbox=web", "/api/tickets/web/1", "/api/tickets/web/1/files/f-0123456789abcdef"] {
            assert_eq!(call(&router, "GET", p, Some(&token)).await.status, StatusCode::NOT_FOUND, "{p}");
        }
        // A revoked device.
        let id = app.remote_devices().pop().unwrap().id;
        app.remote_revoke(&id, sbxw_core::remote_access::journal::Kind::Revoked);
        assert_eq!(call(&router, "GET", "/r/leaders/web/kanban", Some(&token)).await.status, StatusCode::UNAUTHORIZED);
    }

    async fn read_until(s: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin), needle: &str) -> Option<String> {
        let mut got = String::new();
        let end = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < end {
            match tokio::time::timeout(Duration::from_millis(300), s.next()).await {
                Ok(Some(Ok(b))) => got.push_str(&String::from_utf8_lossy(&b)),
                Ok(_) => return None,
                Err(_) => {}
            }
            if got.contains(needle) {
                return Some(got);
            }
        }
        None
    }

    #[tokio::test]
    async fn the_kanban_event_names_the_sandbox_that_changed_and_no_other() {
        let (_d, app) = on();
        let router = remote_router(app.clone());
        let token = pair(&app, &router).await;
        let web = lead(&app, "web").await;
        let _api = lead(&app, "api").await;
        let other = SandboxRef::parse("elsewhere").unwrap();
        let r = req("GET", "/r/events", Some(&token)).body(Body::empty()).unwrap();
        let mut s = router.clone().oneshot(r).await.unwrap().into_body().into_data_stream();
        assert!(read_until(&mut s, "event: leaders").await.is_some());
        // A sandbox that is not a leader, then another leader's change, then ours.
        ticket(&app, &other, "not a leader", "SECRET TEXT");
        ticket(&app, &web, "secret title", "secret description");
        let got = read_until(&mut s, "event: kanban").await.expect("an event for web");
        assert!(got.contains("\"sandbox\":\"web\"") && got.contains("\"latest\":\""), "{got}");
        assert!(!got.contains("elsewhere") && !got.contains("secret") && !got.contains("SECRET"), "numbers and the name only: {got}");
        // Its hash is the board's.
        let latest = call(&router, "GET", "/r/leaders/web/kanban", Some(&token)).await.json()["latest"].as_str().unwrap().to_string();
        assert!(got.contains(&latest), "{got} vs {latest}");
        // An event that changes nothing the phone shows sends nothing.
        app.bus.send(Event::Tickets(crate::svc::tickets::TicketsNudge { sandbox: "web".into(), n: None, what: "followed" }));
        ticket(&app, &SandboxRef::parse("api").unwrap(), "x", "");
        let got = read_until(&mut s, "event: kanban").await.expect("api's own event");
        assert!(got.contains("\"sandbox\":\"api\"") && !got.contains("\"sandbox\":\"web\""), "{got}");
    }
}
