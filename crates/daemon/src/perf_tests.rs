//! Timings of the daemon's hot paths at a realistic size: 20 sandboxes,
//! each with its main session and two threads, and one conversation of 2000
//! history items. Ignored by default (a timing is not a pass or a fail);
//! run with
//! `cargo test -p sbxw-daemon --release perf_ -- --ignored --nocapture --test-threads=1`.
//! Every run is in a temporary sbxw home.

use crate::app::App;
use crate::testing::app;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

const SANDBOXES: usize = 20;
const HISTORY_ITEMS: usize = 2000;

fn hook(app: &App, sb: &SandboxRef, sid: &str, thread: Option<&str>, mut ev: Value) {
    ev["session_id"] = sid.into();
    ev["hook_version"] = 2.into();
    if let Some(t) = thread {
        ev["sbxw_thread"] = t.into();
    }
    app.ingest_hook(sb, ev);
}

/// 20 sandboxes with a record each (a config snapshot, two threads), a main
/// session and two thread sessions each; `sb0`'s main session holds
/// [`HISTORY_ITEMS`] history items.
fn world() -> (tempfile::TempDir, Arc<App>, Vec<SandboxRef>) {
    let (d, app) = app();
    let mut sbs = Vec::new();
    for i in 0..SANDBOXES {
        let sb = SandboxRef::parse(&format!("sb{i}")).unwrap();
        let ws = d.path().join(format!("ws{i}"));
        app.store
            .update(&sb, |r| {
                r.workspace = Some(ws);
                r.config_snapshot = Some(Box::new(sbxw_core::config::Config::default()));
                for t in ["t1", "t2"] {
                    r.threads.insert(t.into(), crate::store::ThreadRecord { repo: "repo".into(), ..Default::default() });
                }
            })
            .unwrap();
        hook(&app, &sb, &format!("s{i}"), None, json!({"hook_event_name": "SessionStart"}));
        for t in ["t1", "t2"] {
            hook(&app, &sb, &format!("s{i}-{t}"), Some(t), json!({"hook_event_name": "SessionStart"}));
        }
        sbs.push(sb);
    }
    // 2000 items, posted as the status hook does: at most 400 a post.
    let mut items = Vec::new();
    for i in 0..HISTORY_ITEMS / 4 {
        items.push(json!({"t": "user", "text": format!("prompt {i}"), "at": i}));
        items.push(json!({"t": "tool_use", "id": format!("t{i}"), "name": "Bash", "input": {"command": "ls"}, "at": i}));
        items.push(json!({"t": "tool_result", "id": format!("t{i}"), "size": 1, "head": "x", "at": i}));
        items.push(json!({"t": "assistant", "text": format!("reply {i}"), "at": i}));
    }
    let mut from = 0u64;
    for chunk in items.chunks(400) {
        let to = from + 1000;
        app.history_post(&sbs[0], &json!({"session_id": "s0", "from": from, "to": to, "reset": from == 0, "items": chunk})).unwrap();
        from = to;
    }
    (d, app, sbs)
}

/// Mean microseconds of `f` over `n` runs, after one warm-up run.
fn time<T>(label: &str, n: u32, mut f: impl FnMut() -> T) -> f64 {
    std::hint::black_box(f());
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(f());
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    println!("perf: {label:<40} {us:>10.1} us/op");
    us
}

#[tokio::test]
#[ignore = "a timing, run on demand"]
async fn perf_hot_paths() {
    let (_d, app, sbs) = world();
    assert_eq!(app.session_infos().len(), SANDBOXES * 3);
    assert_eq!(app.remote_leaders().len(), SANDBOXES);
    let page = app.history_page(&sbs[0], "s0", None, None).unwrap();
    assert!(!page.items.is_empty());

    time("session_infos (60 sessions)", 200, || app.session_infos());
    time("remote_reachable", 200, || app.remote_reachable());
    time("remote_leaders (GET /r/leaders)", 200, || app.remote_leaders());
    time("remote_target (every :sandbox route)", 200, || app.remote_target("sb7").unwrap());
    time("remote_history (2000 items)", 200, || app.remote_history("dev", &sbs[0], None, None, None).unwrap());
    time("store.get (one record)", 2000, || app.store.get(&sbs[3]));
    time("anonymise_rules (no file)", 2000, || app.anonymise_rules().unwrap());
    let mut n = 0u64;
    time("ingest_hook (PreToolUse, no rules file)", 2000, || {
        n += 1;
        hook(&app, &sbs[1], "s1", None, json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": format!("ls {n}")}}));
    });
    // A person's rules: 30 names.
    let names: String = (0..30).map(|i| format!("\"Person Name{i}\" = \"P{i}\"\n")).collect();
    std::fs::write(app.store.paths.anonymise(), format!("[replace]\n{names}")).unwrap();
    assert_eq!(app.anonymise_rules().unwrap().terms(), 30);
    time("anonymise_rules (30 names)", 2000, || app.anonymise_rules().unwrap());
    time("ingest_hook (PreToolUse, 30 names)", 2000, || {
        n += 1;
        hook(&app, &sbs[1], "s1", None, json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": format!("ls {n}")}}));
    });
}
