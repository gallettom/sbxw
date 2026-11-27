//! `GET /api/plans`, the list the desktop's plan screens read: each plan as
//! the book holds it, plus the `merge_status` those screens draw, in every
//! merge state. And the desktop's fixture `threads/route-ci-building.json`
//! is this route's own answer for a CI plan that is being built.

use crate::app::App;
use crate::testing::app;
use sbxw_core::plans::{merge_status, MergeState, Plan, PlanBook};
use serde_json::{json, Value};
use std::sync::Arc;

const STATES: [(MergeState, bool); 12] = [
    (MergeState::Proposed, false),
    (MergeState::Merging, false),
    (MergeState::Merged, false),
    (MergeState::Failed, false),
    (MergeState::Abandoned, false),
    (MergeState::CiBuilding, true),
    (MergeState::CiTesting, true),
    (MergeState::MergedCi, true),
    (MergeState::CiStale, true),
    (MergeState::CiFailed, true),
    (MergeState::Merged, true),
    (MergeState::Proposed, true),
];

/// The desktop fixtures' CI plan (`fixtures/threads/ci-building.json`,
/// without the status the daemon derives), its merge in `state`.
fn plan(id: &str, state: MergeState, ci: bool) -> Plan {
    let mut v = json!({
        "id": id, "sandbox": "web-app",
        "summary": "Orders v2: the API routes, the web page that uses them, and their docs, in three threads that do not touch the same files.",
        "threads": [
            { "name": "api-routes", "goal": "Add the /v2/orders routes to the API.", "scope": "api/src/routes/v2/", "state": "done", "summary": "api-routes done: committed on sbxw/api-routes, tests pass" },
            { "name": "web-orders", "goal": "Build the orders page in the web app against the new routes.", "scope": "web/src/pages/orders/", "state": "done", "summary": "web-orders done: committed on sbxw/web-orders, tests pass" },
            { "name": "docs", "goal": "Document the v2 orders API in docs/api/orders.md.", "scope": "docs/api/orders.md", "state": "done", "summary": "docs done: committed on sbxw/docs, tests pass" }
        ],
        "state": "approved", "variant": "split",
        "justification": "The three areas live in different folders, so a branch each merges cleanly.",
        "lead": "web-app lead", "created_ms": 1763290000000u64, "updated_ms": 1763290000000u64,
        "approved_tests": { "command": "pnpm test && cargo test --workspace", "timeout_minutes": 60 },
        "merge": { "order": ["api-routes", "web-orders", "docs"], "note": "api first: the web page depends on its routes.", "state": state, "base": null, "results": [] }
    });
    if ci {
        v["merge"]["ci"] = json!({ "branch": "sbxw/ci-p-1", "worktree": ".sbxw-worktrees/.ci-p-1", "run": 1, "builds": 1 });
    }
    serde_json::from_value(v).unwrap()
}

async fn listed(app: &Arc<App>) -> Vec<Value> {
    use tower::ServiceExt;
    let req = axum::http::Request::builder().uri("/api/plans").header("host", "127.0.0.1:7681").body(axum::body::Body::empty()).unwrap();
    let res = crate::testing::signed_in(app.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn load(app: &App, plans: Vec<Plan>) {
    *app.plans.lock().unwrap() = PlanBook::load(plans);
}

#[tokio::test]
async fn every_merge_state_is_listed_with_its_merge_status_and_the_plan_unchanged() {
    let (_d, app) = app();
    let plans: Vec<Plan> = STATES.iter().enumerate().map(|(i, (s, ci))| plan(&format!("p-{i}"), *s, *ci)).collect();
    let mut none = plan("p-none", MergeState::Proposed, false);
    none.merge = None;
    load(&app, plans.iter().cloned().chain([none]).collect());
    let got = listed(&app).await;
    assert_eq!(got.len(), STATES.len() + 1);
    for p in &plans {
        let v = got.iter().find(|v| v["id"] == p.id.as_str()).unwrap_or_else(|| panic!("{} listed", p.id));
        let want = serde_json::to_value(merge_status(p.merge.as_ref().unwrap())).unwrap();
        for k in ["word", "detail", "can_merge", "ci"] {
            assert_eq!(v["merge_status"][k], want[k], "{}: {k}", p.id);
            assert!(!v["merge_status"][k].is_null(), "{}: {k} is there", p.id);
        }
        assert_eq!(v["merge_status"]["why_not"], want["why_not"], "{}", p.id);
        // Nothing a consumer read before is gone or changed.
        let bare = serde_json::to_value(p).unwrap();
        for (k, b) in bare.as_object().unwrap() {
            assert_eq!(&v[k], b, "{}: {k} as before", p.id);
        }
        assert_eq!(v.as_object().unwrap().len(), bare.as_object().unwrap().len() + 1, "{}: only merge_status added", p.id);
    }
    let v = got.iter().find(|v| v["id"] == "p-none").unwrap();
    assert!(v.get("merge_status").is_none(), "no merge, no status");
    // A split plan with tests, merged as is: why it took no CI branch.
    let mut asis = plan("p-asis", MergeState::Proposed, false);
    asis.threads[0].repo = Some("api".into());
    load(&app, vec![asis]);
    let v = &listed(&app).await[0];
    assert!(v["merge_status"]["no_ci_because"].as_str().is_some_and(|w| w.contains("api")), "{v}");
}

/// What the desktop's `merge_view` and the plan dialog read of a plan's
/// merge: the route's answer for a CI plan being built, written as the
/// fixture `desktop/fixtures/threads/route-ci-building.json` and checked
/// against it, so the desktop's tests draw what the daemon really sends.
/// `SBXW_WRITE_PLANS_FIXTURE=1 cargo test -p sbxw-daemon route_ci_building`
/// rewrites it.
#[tokio::test]
async fn the_desktops_route_ci_building_fixture_is_this_routes_answer() {
    let (_d, app) = app();
    load(&app, vec![plan("p-1", MergeState::CiBuilding, true)]);
    let plans = listed(&app).await;
    for k in ["word", "detail", "can_merge", "why_not", "ci"] {
        assert!(!plans[0]["merge_status"][k].is_null(), "merge_view reads merge_status.{k}");
    }
    let fixture = json!({
        "about": "GET /api/plans as the daemon answers it for a split plan whose CI branch is being built. Written by sbxw-daemon's the_desktops_route_ci_building_fixture_is_this_routes_answer; do not edit by hand.",
        "daemon": { "plans": plans },
    });
    let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../desktop/fixtures/threads/route-ci-building.json");
    if std::env::var_os("SBXW_WRITE_PLANS_FIXTURE").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(on_disk, text, "{} is not the route's answer: rewrite it with SBXW_WRITE_PLANS_FIXTURE=1", path.display());
}
