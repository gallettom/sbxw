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
    let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
    // In memory: a press here (GUI, Lead Mode, a layout) must not reach the
    // state file the next window, or the person's app, starts from.
    a.store = sbxw_desktop::shell::store::Store::memory();
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
        // The top bar's GUI | TUI and Lead Mode (two-state: each press must switch).
        ("monitor/basic", vec![], vec!["top-view-gui", "top-view-tui", "top-lead", "top-lead", "top-view-gui", "top-lead", "top-view-tui", "top-lead"]),
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

/// Sessions: a folded group of steps opens to its steps, and a step to its
/// output (the conversation from the fixture's history).
#[test]
fn a_step_group_and_a_step_unfold_in_sessions() {
    let mut rt = window("monitor/basic", &[intent("monitor_open", json!({})), intent("monitor_select", json!({ "key": "billing-api:s-bill-main" }))]);
    // The last one drawn: the conversation opens at its end (the first ones are scrolled out of view).
    let key_of = |rt: &Runtime<App>, prefix: &str| drawn(rt).into_iter().rev().find_map(|d| d.split('|').next().and_then(|k| k.strip_prefix(&format!("Some(\"{prefix}"))).map(|k| format!("{prefix}{}", k.trim_end_matches("\")"))));
    let group = key_of(&rt, "monitor-steps-g").expect("a folded group of steps is drawn");
    // (Pressed once: it opens downwards, its head stays where it was.)
    for k in [group.clone()] {
        let before = drawn(&rt);
        let (x, y) = rt.find(&k).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        assert_ne!(drawn(&rt), before, "pressing {k} changed nothing drawn");
    }
    // Its first step, right under its head (the last ones run below the view).
    let step = drawn(&rt).into_iter().find_map(|d| d.split('|').next().and_then(|k| k.strip_prefix("Some(\"monitor-step-s")).map(|k| format!("monitor-step-s{}", k.trim_end_matches("\")")))).expect("a step is drawn");
    let before = drawn(&rt);
    let (x, y) = rt.find(&step).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_ne!(drawn(&rt), before, "pressing {step} changed nothing drawn");
}

/// What the rail opens, and which entries are lit.
fn opened(rt: &Runtime<App>) -> String {
    let d = rt.state.screen_data();
    let lit: Vec<String> = { let (a, b) = rt.state.rail_layout_keys(); a.into_iter().chain(b).filter(|(_, on)| *on).map(|(k, _)| k).collect() };
    format!("overlay {} modal {} panels {} lit {lit:?}", d["app"]["overlay"]["screen"], d["app"]["modal"]["screen"], d["app"]["panels"]["shown"])
}

/// Every rail entry that opens a view, pressed twice, puts back what was
/// there before the first press (the panel, overlay or dialog it opened
/// closes; nothing else stays lit).
#[test]
fn every_rail_entry_pressed_twice_puts_back_what_was_there() {
    let mut rt = window("shell/two-panes", &[]);
    let keys = ["rail-requests", "rail-network", "jarvis-toggle", "rail-board", "rail-commits", "rail-worktrees", "rail-files", "rail-mac", "rail-mcp", "rail-skills", "rail-kits"];
    // (Settings opens a dialog whose backdrop covers the rail: a second press
    // on the rail does not reach it; Escape, its Cancel or a press outside close it.)
    let mut bad = vec![];
    let mut seen = 0;
    for k in keys {
        if rt.find(k).is_none() {
            continue;
        }
        seen += 1;
        let before = opened(&rt);
        for n in 0..2 {
            let Some((x, y)) = rt.find(k) else {
                bad.push(format!("{k}: not drawn for press {}", n + 1));
                break;
            };
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(&mut rt);
            if n == 0 && opened(&rt) == before {
                bad.push(format!("{k}: the first press opened nothing"));
            }
        }
        let after = opened(&rt);
        if after != before {
            bad.push(format!("{k}: pressed twice: {after}, before: {before}"));
        }
    }
    assert!(seen >= 9, "only {seen} rail entries drawn");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
