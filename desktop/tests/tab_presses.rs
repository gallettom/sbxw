//! Every tab-like control, pressed in the whole app: the drawn tree must
//! change. A press whose data changes but whose screen is not drawn again
//! (a second `Set` of the same key in one batch dropping the first, a read
//! the tracking missed, a lazy screen never shown) fails here.
//!
//! `cd desktop && cargo test --test tab_presses`

use rune::Runtime;
use sbxw_desktop::shell::{view, App, Args};
use serde_json::{json, Value as J};

/// What the window shows: each element's key, role, text and label.
fn drawn(rt: &Runtime<App>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .map(|e| format!("{:?}|{}|{:?}|{:?}", e.key, e.role, e.text, e.label))
        .collect()
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
    }
}

/// The app on `fixture` (under `fixtures/`), opened with `open` (intents as
/// a screen returns them).
fn window(fixture: &str, open: &[J]) -> Runtime<App> {
    let path = format!("{}/fixtures/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    let a = App::new(Args { fixture: Some(path), ..Args::default() });
    let mut rt = Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    settle(&mut rt);
    for o in open {
        let v = json!({ "kind": "intent", "name": o["name"], "args": o["args"] });
        for i in sbxw_desktop::intent::parse(&v).unwrap_or_else(|e| panic!("{fixture}: {e}")) {
            rt.update(|s, _| s.run_intent("test", i));
        }
        settle(&mut rt);
    }
    rt
}

/// Presses each key in turn; the keys that changed nothing, or were not drawn.
fn press_each(fixture: &str, open: &[J], keys: &[&str]) -> Vec<String> {
    let mut rt = window(fixture, open);
    let mut bad = vec![];
    for k in keys {
        let before = drawn(&rt);
        let Some((x, y)) = rt.find(k) else {
            bad.push(format!("{fixture}: {k} is not drawn"));
            continue;
        };
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        if drawn(&rt) == before {
            bad.push(format!("{fixture}: pressing {k} changed nothing drawn"));
        }
    }
    bad
}

fn intent(name: &str, args: J) -> J {
    json!({ "name": name, "args": args })
}

#[test]
fn every_tab_like_control_changes_what_is_drawn() {
    let cases: Vec<(&str, Vec<J>, Vec<&str>)> = vec![
        // The Threads panel's tabs (the Worktrees one fetches as it switches).
        ("threads/basic", vec![intent("threads_show", json!({ "sandbox": "web-app" }))], vec!["tab-worktrees", "tab-board", "tab-sessions", "tab-worktrees"]),
        // Setup's tabs.
        ("setup/full", vec![intent("open_modal", json!({ "screen": "setup", "arg": "secrets" }))], vec!["tab-hosts", "tab-voice", "tab-secrets"]),
        // The explorer's locations (wide: one row each).
        ("files/files-browse", vec![intent("files_open", json!({ "sandbox": "web-app" }))], vec!["files-loc-/tmp", "files-loc-recent", "files-loc-/Users/me/web"]),
        // MCP's server rows (select, then the next).
        ("mcp/basic", vec![], vec!["mcp-sel-linear", "mcp-sel-playwright"]),
        // A kit's row.
        ("kits/basic", vec![], vec!["kit-sel-codemap"]),
        // The Requests panel: one waiting request, then another.
        ("relay/basic", vec![], vec!["relay-req-r-2", "relay-req-r-3", "relay-req-r-1"]),
        // The plan dialog's "Show more" on a long goal and on many scope chips, and back.
        ("threads/plan-long", vec![], vec!["pt-1-more", "pt-1-owns-more", "pt-1-more"]),
        // The Board: its kind filter and the narrow tabs' content (wide here: the filter).
        ("board/basic", vec![intent("board_show", json!({ "team": "p-run" }))], vec!["board-group-notes", "board-group-questions", "board-group-all", "board-thread-web-page"]),
        // The pager's Tabs and Grid.
        ("shell/finish-grid", vec![], vec!["pager-view-tabs", "pager-view-grid"]),
    ];
    let mut bad = vec![];
    for (fixture, open, keys) in &cases {
        bad.extend(press_each(fixture, open, keys));
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Commits: a row expands into its detail (the stand-in's built-in history).
#[test]
fn a_commit_row_expands() {
    let mut rt = window("shell/two-panes", &[intent("commits_show", json!({ "sandbox": "web-app" }))]);
    let row = drawn(&rt).into_iter().find_map(|d| d.split('|').next().and_then(|k| k.strip_prefix("Some(\"commits-row-")).map(|k| k.trim_end_matches("\")").to_string()));
    let Some(id) = row else { panic!("no commit row drawn") };
    let key = format!("commits-row-{id}");
    let before = drawn(&rt);
    let (x, y) = rt.find(&key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_ne!(drawn(&rt), before, "pressing {key} changed nothing drawn");
}
