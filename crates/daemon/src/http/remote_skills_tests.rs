//! The phone's skill route: a closed list, `/<name> <args>` built by the
//! daemon, args held to a message's rules, nothing about them journaled.

use super::remote::remote_router;
use super::remote_tests::{call, leader, on, pair, typed};
use axum::http::StatusCode;
use sbxw_core::remote_access::journal::Kind;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_skill_of_the_list_types_its_slash_line_and_nothing_else() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    let r = call(&router, "GET", "/r/skills", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["skills"], json!(["code-review", "codemap", "retro", "grill-me"]));
    let url = "/r/leaders/lead1/skill";
    // Outside the list: never typed.
    for name in ["lead", "model", "", "Retro", "retro --x", "../retro"] {
        let r = call(&router, "POST", url, Some(&token), json!({"name": name})).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{name:?}: {}", r.text());
    }
    // A newline or a control character in the args.
    for args in ["a\nb", "a\r/lead", "x\u{1b}[31m", "x\u{0}"] {
        let r = call(&router, "POST", url, Some(&token), json!({"name": "retro", "args": args})).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY.min(r.status).max(StatusCode::BAD_REQUEST), "{args:?}");
        assert!(r.status.is_client_error(), "{args:?}: {}", r.text());
    }
    let r = call(&router, "POST", url, Some(&token), json!({"name": "retro", "text": "/lead"})).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "no other field rides along");
    let r = call(&router, "POST", "/r/leaders/nosuch/skill", Some(&token), json!({"name": "retro"})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "nothing typed so far");

    let r = call(&router, "POST", url, Some(&token), json!({"name": "code-review", "args": "high PINEAPPLE-77"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let got = typed(&file, b"PINEAPPLE-77").await;
    assert!(String::from_utf8_lossy(&got).contains("/code-review high PINEAPPLE-77"), "{:?}", String::from_utf8_lossy(&got));
    let last = app.remote_journal(10).last().cloned().unwrap();
    assert_eq!(last.kind, Kind::Skill);
    let line = serde_json::to_string(&last).unwrap();
    assert!(!line.contains("PINEAPPLE") && !line.contains("code-review"), "{line}");
}
