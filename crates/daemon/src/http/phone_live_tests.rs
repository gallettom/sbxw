//! Thread `phone-live`: a leader's reply reaches the open phone page by
//! itself (`.sbxw-artifacts/reports/phone-live.md`). Every text here is
//! invented.

use super::remote::remote_router;
use super::remote_tests::{call, leader, on, pair, req};
use crate::app::App;
use axum::body::Body;
use axum::http::{header, StatusCode};
use futures_util::StreamExt;
use sbxw_core::remote_access::COOKIE;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;

const REPLY: &str = "Reply nobody asked for again: all three threads are merged.";

/// A v3 status hook's event (it reads the transcript, so `Stop`'s reply is
/// provisional).
fn hook3(app: &App, sb: &SandboxRef, mut ev: Value) {
    ev["session_id"] = "s1".into();
    ev["hook_version"] = 3.into();
    ev["history"] = "transcript".into();
    app.ingest_hook(sb, ev);
}

fn post(app: &App, sb: &SandboxRef, from: u64, to: u64, items: Value) {
    app.history_post(sb, &json!({"session_id": "s1", "from": from, "to": to, "items": items})).unwrap();
}

/// Frames until `want` shows, or what was seen when `within` ran out.
async fn frames_until(s: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin), want: &str, within: Duration) -> Result<String, String> {
    let mut seen = String::new();
    let end = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(end, s.next()).await {
            Ok(Some(Ok(b))) => {
                seen.push_str(&String::from_utf8_lossy(&b));
                if seen.contains(want) {
                    return Ok(seen);
                }
            }
            _ => return Err(seen),
        }
    }
}

/// The defect as the person met it: a message sent, the leader answers with
/// text only, and at `Stop` the hook's two requests land event first, then a
/// transcript tail that does not hold the reply yet. Nobody sends anything
/// after that: the stream must say `history` within 2 s, and the history
/// route must then hold the reply. The frame itself carries no text.
#[tokio::test]
async fn the_reply_reaches_an_open_stream_with_nobody_sending() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    hook3(&app, &sb, json!({"hook_event_name": "UserPromptSubmit", "prompt": "How far along are the threads?"}));
    post(&app, &sb, 0, 100, json!([{"t": "user", "text": "How far along are the threads?", "at": 1}]));

    let r = req("GET", "/r/events").header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    let mut s = router.clone().oneshot(r).await.unwrap().into_body().into_data_stream();
    frames_until(&mut s, "event: leaders", Duration::from_secs(5)).await.expect("leaders at connect");
    let first = call(&router, "GET", "/r/leaders/lead1/history", Some(&token), json!({})).await;
    let latest = first.json()["page"]["latest_seq"].as_u64().unwrap();

    hook3(&app, &sb, json!({"hook_event_name": "Stop", "last_assistant_message": REPLY}));
    post(&app, &sb, 100, 180, json!([{"t": "system", "text": "turn over"}]));

    let frames = frames_until(&mut s, "event: history", Duration::from_secs(2)).await.unwrap_or_else(|seen| panic!("no history frame within 2 s; saw {seen:?}"));
    assert!(frames.contains("\"sandbox\":\"lead1\""), "{frames}");
    assert!(!frames.contains("threads are merged") && !frames.contains("How far along"), "a nudge, never text: {frames}");

    let r = call(&router, "GET", &format!("/r/leaders/lead1/history?after={latest}"), Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    let items = r.json()["page"]["items"].as_array().cloned().unwrap_or_default();
    assert!(
        items.iter().any(|i| i["kind"] == "assistant_text" && i["text"] == REPLY),
        "the reply is there for the page to draw: {items:?}"
    );

    // The transcript brings it at last: one reply, no double, and a nudge.
    post(&app, &sb, 180, 260, json!([{"t": "assistant", "text": REPLY, "at": 3}]));
    frames_until(&mut s, "event: history", Duration::from_secs(2)).await.expect("a nudge for the settled reply");
    let r = call(&router, "GET", "/r/leaders/lead1/history", Some(&token), json!({})).await;
    let replies: Vec<Value> = r.json()["page"]["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "assistant_text").cloned().collect();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0].get("provisional").is_none(), "{replies:?}");
}

/// A quiet stream says `ping` (a named event with an empty data line, so
/// EventSource dispatches it to the page), at PING_EVERY, and only that: no
/// payload.
#[tokio::test]
async fn a_quiet_stream_says_ping() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let r = req("GET", "/r/events").header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    let mut s = router.clone().oneshot(r).await.unwrap().into_body().into_data_stream();
    frames_until(&mut s, "event: leaders", Duration::from_secs(5)).await.expect("leaders at connect");
    let seen = frames_until(&mut s, "event: ping", super::remote::PING_EVERY * 10).await.unwrap_or_else(|seen| panic!("no ping; saw {seen:?}"));
    let ping = &seen[seen.find("event: ping").unwrap()..];
    let frame = &ping[..ping.find("\n\n").map_or(ping.len(), |i| i + 2)];
    assert!(frame == "event: ping\ndata: \n\n" || frame == "event: ping\ndata:\n\n", "{frame:?}");
}
