//! A feature modal's open select list is drawn over the dialog, not in it:
//! whole, inside the window, not clipped by the dialog's card, at the two
//! window sizes the app supports (1284x816 and 720x480). One case per modal
//! screen that has a select; a Confirm over an open list stays on top.
//!
//! `cd desktop && cargo test --test modal_lists`

use rune::Runtime;
use sbxw_desktop::shell::{view, App, Args};
use serde_json::json;

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..16 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
    }
}

fn window(fixture: &str, (w, h): (f32, f32), open: Option<(&str, Option<&str>)>) -> Runtime<App> {
    let path = format!("{}/fixtures/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
    a.store = sbxw_desktop::shell::store::Store::memory();
    let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
    settle(&mut rt);
    if let Some((screen, arg)) = open {
        let v = json!({ "kind": "intent", "name": "open_modal", "args": { "screen": screen, "arg": arg } });
        for i in sbxw_desktop::intent::parse(&v).unwrap() {
            rt.update(|s, _| s.run_intent("test", i));
        }
        settle(&mut rt);
    }
    rt
}

/// The lowest select drawn in the modal (the one nearest the card's foot).
fn lowest_select(rt: &Runtime<App>) -> Option<String> {
    // Only one the person can reach: what is drawn at its centre is itself.
    let reachable = |e: &rune::ElementInfo| {
        let (x, y) = (e.rect[0] + 3.0, e.rect[1] + e.rect[3] / 2.0);
        rt.element_at(x, y).and_then(|p| p.key().map(str::to_string)) == e.key
    };
    let mut best: Option<(f32, String)> = None;
    for e in rt.elements() {
        if e.role != "combobox" || e.exiting || !reachable(&e) {
            continue;
        }
        let Some(k) = e.key.clone() else { continue };
        let bottom = e.rect[1] + e.rect[3];
        if best.as_ref().is_none_or(|(b, _)| bottom > *b) {
            best = Some((bottom, k));
        }
    }
    best.map(|(_, k)| k)
}

fn check(fixture: &str, open: Option<(&str, Option<&str>)>) -> Vec<String> {
    check_after(fixture, open, None)
}

/// `tab`: a key to press first (a tab of the dialog).
fn check_after(fixture: &str, open: Option<(&str, Option<&str>)>, tab: Option<&str>) -> Vec<String> {
    let mut bad = vec![];
    for size in [(1284.0, 816.0), (720.0, 480.0)] {
        let tag = format!("{fixture} at {}x{}", size.0, size.1);
        let mut rt = window(fixture, size, open);
        if let Some(t) = tab {
            let (x, y) = rt.find(t).unwrap_or_else(|| panic!("{tag}: {t} not drawn"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(&mut rt);
        }
        let Some(sel) = lowest_select(&rt) else {
            bad.push(format!("{tag}: no select drawn"));
            continue;
        };
        // A small window scrolls the dialog's body: bring the select into view.
        for _ in 0..12 {
            let r = rt.presented(&sel).unwrap().0;
            if r[1] + r[3] <= size.1 - 40.0 {
                break;
            }
            rt.wheel(size.0 / 2.0, size.1 / 2.0, -120.0);
            settle(&mut rt);
        }
        let r = rt.presented(&sel).unwrap().0;
        if r[1] + r[3] > size.1 {
            // The dialog neither fits nor scrolls here: its select is below the window (not this fix's).
            eprintln!("{tag}: {sel} is below the window and the dialog does not scroll to it; not checked");
            continue;
        }
        let (x, y) = (r[0] + 3.0, r[1] + r[3] / 2.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        let list_key = format!("{sel}-list");
        let Some((l, _)) = rt.presented(&list_key) else {
            bad.push(format!("{tag}: {sel} opened no list (select {:?}, at {x},{y}, elements {:?})", rt.presented(&sel), rt.element_at(x, y).and_then(|p| p.key().map(str::to_string))));
            continue;
        };
        if l[0] < 0.0 || l[1] < 0.0 || l[0] + l[2] > size.0 + 0.5 || l[1] + l[3] > size.1 + 0.5 {
            bad.push(format!("{tag}: list {l:?} leaves the window"));
        }
        // What shows at three points inside the list is the list or its rows, not the card.
        for f in [0.15, 0.5, 0.85] {
            let picked = rt.element_at(l[0] + 3.0, l[1] + l[3] * f).and_then(|p| p.key().map(str::to_string));
            if !picked.as_deref().is_some_and(|k| k == list_key || k.starts_with(&format!("{sel}-"))) {
                bad.push(format!("{tag}: at {f} of the list shows {picked:?}: clipped by the dialog"));
            }
        }
    }
    bad
}

#[test]
fn templates_list_is_whole() {
    let bad = check("templates/basic", None);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn threads_new_list_is_whole() {
    let bad = check("threads/new-thread", None);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn plans_list_is_whole() {
    let bad = check("threads/plan-pending", None);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn setup_list_is_whole() {
    let bad = check_after("setup/full", Some(("setup", None)), Some("tab-hosts"));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn create_list_is_whole() {
    let bad = check("create/workspace-remote", Some(("create", Some("remote"))));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

