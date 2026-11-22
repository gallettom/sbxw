//! Thread `phone-history-empty`: the phone's conversation reads lazily and
//! survives a daemon restart (`docs/session-history.md`). Every text here is
//! invented.

use super::remote::remote_router;
use super::remote_tests::{call, leader, on, pair};
use crate::app::App;
use axum::http::StatusCode;
use sbxw_core::history::HistoryBook;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};

fn post(app: &App, sb: &SandboxRef, from: u64, to: u64, reset: bool, items: Value) {
    app.history_post(sb, &json!({"session_id": "s1", "from": from, "to": to, "reset": reset, "items": items})).unwrap();
}

fn hook3(app: &App, sb: &SandboxRef, mut ev: Value) {
    ev["session_id"] = "s1".into();
    ev["hook_version"] = 3.into();
    ev["history"] = "transcript".into();
    app.ingest_hook(sb, ev);
}

/// `n` exchanges: a prompt, two tool calls with results, a reply.
fn exchanges(n: usize) -> Value {
    let mut items = Vec::new();
    for i in 0..n {
        items.push(json!({"t": "user", "text": format!("prompt {i}"), "at": i}));
        for k in 0..2 {
            items.push(json!({"t": "tool_use", "id": format!("t{i}-{k}"), "name": "Bash", "input": {"command": "ls"}, "at": i}));
            items.push(json!({"t": "tool_result", "id": format!("t{i}-{k}"), "size": 1, "head": "x", "at": i}));
        }
        items.push(json!({"t": "assistant", "text": format!("reply {i}"), "at": i}));
    }
    Value::Array(items)
}

async fn get(router: &axum::Router, token: &str, query: &str) -> Value {
    let r = call(router, "GET", &format!("/r/leaders/lead1/history{query}"), Some(token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{query}");
    r.json()["page"].clone()
}

fn texts(page: &Value) -> Vec<String> {
    page["items"].as_array().unwrap().iter().map(|i| if i["kind"] == "steps" { format!("steps:{}", i["count"]) } else { i["text"].as_str().unwrap_or("").to_string() }).collect()
}

fn seqs(page: &Value) -> Vec<u64> {
    page["items"].as_array().unwrap().iter().map(|i| i["seq"].as_u64().unwrap()).collect()
}

/// A small first page in rows (a run of steps is one row), then older pages
/// by `before` that join it with no row lost or repeated.
#[tokio::test]
async fn the_first_page_is_the_last_rows_and_older_pages_join_it() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    post(&app, &sb, 0, 100, false, exchanges(20));

    let first = get(&router, &token, "?limit=6").await;
    assert_eq!(texts(&first), ["prompt 18", "steps:2", "reply 18", "prompt 19", "steps:2", "reply 19"]);
    let before = first["next_before"].as_u64().expect("more before it");
    assert_eq!(before, seqs(&first)[0]);

    let older = get(&router, &token, &format!("?before={before}&limit=6")).await;
    assert_eq!(texts(&older), ["prompt 16", "steps:2", "reply 16", "prompt 17", "steps:2", "reply 17"]);
    // A cut inside a run of steps: the next page starts where the run did.
    let cut = get(&router, &token, "?limit=5").await;
    assert_eq!(texts(&cut), ["steps:2", "reply 18", "prompt 19", "steps:2", "reply 19"]);
    let run_start = cut["next_before"].as_u64().unwrap();
    assert!(run_start < seqs(&cut)[0], "the steps row stands for 4 items: the page before ends at its first");
    let rest = get(&router, &token, &format!("?before={run_start}&limit=1")).await;
    assert_eq!(texts(&rest), ["prompt 18"]);
}

/// `?after=` gives only what is newer than the page's last row, oldest
/// first, `limit` rows at a time with `more`; never the end of the log.
#[tokio::test]
async fn after_gives_only_the_newer_rows_page_by_page() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    post(&app, &sb, 0, 100, false, exchanges(2));
    let first = get(&router, &token, "").await;
    let mut latest = first["latest_seq"].as_u64().unwrap();
    let epoch = first["epoch"].as_str().unwrap().to_string();
    assert_eq!(*seqs(&first).last().unwrap(), latest);

    // Nothing new: nothing.
    assert!(texts(&get(&router, &token, &format!("?after={latest}")).await).is_empty());

    post(&app, &sb, 100, 200, false, exchanges(10));
    let mut seen = Vec::new();
    for _ in 0..20 {
        let p = get(&router, &token, &format!("?after={latest}&limit=4")).await;
        assert_eq!(p["epoch"], epoch.as_str());
        assert!(seqs(&p).iter().all(|s| *s > latest), "only newer rows");
        seen.extend(texts(&p));
        latest = p["latest_seq"].as_u64().unwrap();
        if p.get("more").is_none() {
            break;
        }
        assert_eq!(seqs(&p).len(), 4);
        assert_eq!(*seqs(&p).last().unwrap(), latest, "more: read again from the last row");
    }
    let want: Vec<String> = (0..10).flat_map(|i| [format!("prompt {i}"), "steps:2".into(), format!("reply {i}")]).collect();
    assert_eq!(seen, want, "every new row once, in order");
}

/// The bug's other half: after a daemon restart (or a reset backfill) the
/// numbering starts again, so the phone's last seq means nothing. The
/// epoch says so.
#[tokio::test]
async fn a_restart_or_a_reset_changes_the_epoch() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    post(&app, &sb, 0, 100, false, exchanges(3));
    let e1 = get(&router, &token, "").await["epoch"].as_str().unwrap().to_string();
    post(&app, &sb, 100, 150, false, exchanges(1));
    assert_eq!(get(&router, &token, "").await["epoch"], e1.as_str(), "a plain post keeps it");

    // The hook backfills again from the tail: a reset.
    post(&app, &sb, 50, 150, true, exchanges(2));
    let e2 = get(&router, &token, "").await["epoch"].as_str().unwrap().to_string();
    assert_ne!(e1, e2);

    // The daemon restarts: every log gone. An event creates the log again
    // (empty), then the hook's backfill fills it.
    *app.history.lock().unwrap() = HistoryBook::new();
    hook3(&app, &sb, json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "waiting"}));
    let empty = get(&router, &token, "").await;
    assert!(texts(&empty).is_empty());
    let e3 = empty["epoch"].as_str().unwrap().to_string();
    assert_ne!(e3, e2);
    assert_ne!(e3, e1);
    post(&app, &sb, 50, 150, true, exchanges(2));
    let full = get(&router, &token, "").await;
    assert_eq!(texts(&full).len(), 6);
    assert_ne!(full["epoch"], e3.as_str());
}
