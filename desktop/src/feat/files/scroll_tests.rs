//! The reader scrolls: a Markdown file longer than the window is drawn
//! inside a panel of its own that the wheel moves, the Outline beside it
//! stays in view, and a heading pressed there is brought into view.

use super::*;
use crate::shell::{view, App, Args};
use rune::Runtime;

const SPEC: &str = include_str!("../../../fixtures/text/spec.md");
const README: &str = "/Users/me/web/README.md";

/// The stand-in's fixture with `text` as the workspace's README.
fn fixture_with(text: &str) -> String {
    let mut v: J = serde_json::from_str(include_str!("../../../fixtures/files/standin.json")).unwrap();
    v["daemon"]["files_content"][README] = J::String(text.into());
    let p = std::env::temp_dir().join(format!("sbxw-files-scroll-{}-{}.json", std::process::id(), text.len()));
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
    p.to_string_lossy().to_string()
}

fn settle(rt: &mut Runtime<App>) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(240);
    let mut quiet = 0;
    while quiet < 12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        let busy = !rt.state.link_quiet();
        quiet = if busy { 0 } else { quiet + 1 };
        if busy {
            assert!(std::time::Instant::now() < end, "the fixture's link never settled");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn intent(rt: &mut Runtime<App>, name: &str, args: J) {
    for i in crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(rt);
}

/// The window with the README open in the Files overlay.
pub(super) fn window(text: &str, w: f32, h: f32) -> Runtime<App> {
    let a = App::new(Args { fixture: Some(fixture_with(text)), ..Args::default() });
    let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
    settle(&mut rt);
    intent(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    intent(&mut rt, "files_browse", json!({ "path": "/Users/me/web" }));
    intent(&mut rt, "files_select", json!({ "id": README }));
    rt
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.presented(key).unwrap_or_else(|| panic!("{key} not drawn")).0
}

#[test]
fn a_long_markdown_file_scrolls_inside_its_panel() {
    for (w, h) in [(1284.0, 816.0), (1284.0, 500.0), (1800.0, 900.0)] {
        let mut rt = window(SPEC, w, h);
        let (doc, md) = (rect(&rt, "files-doc"), rect(&rt, "files-md"));
        assert!(md[3] > h, "the document ({}) is longer than the window ({h})", md[3]);
        assert!(doc[1] + doc[3] <= h + 0.5, "the panel {doc:?} ends inside the window ({h})");
        assert!(md[3] > doc[3] + 100.0, "the document {md:?} is longer than its panel {doc:?}");
        // The header and the action row stay put; the Outline stays in view.
        let head = rect(&rt, "files-download");
        let out = rect(&rt, "files-outline");
        assert!(out[1] + out[3] <= h + 0.5, "the outline {out:?} stays inside the window");
        // The wheel over the document moves it.
        let top = md[1];
        rt.wheel(doc[0] + doc[2] / 2.0, doc[1] + doc[3] / 2.0, -300.0);
        settle(&mut rt);
        let after = rect(&rt, "files-md");
        assert!(after[1] < top - 100.0, "the wheel moved the document: {top} -> {}", after[1]);
        assert_eq!(rect(&rt, "files-download"), head, "the action row did not move");
        assert_eq!(rect(&rt, "files-doc"), doc, "the panel did not move");
    }
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let r = rect(rt, key);
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

#[test]
fn an_outline_entry_brings_its_heading_into_view() {
    let mut rt = window(SPEC, 1800.0, 900.0);
    let doc = rect(&rt, "files-doc");
    let last = rt.state.feats.data.get(UI).unwrap()["preview"]["outline"].as_array().unwrap().len() - 1;
    let n = {
        let t = SPEC.to_string();
        explorer::heading_block(&explorer::rewrite(&t, "/"), last).unwrap()
    };
    assert!(rect(&rt, &format!("files-text-b{n}"))[1] > doc[1] + doc[3], "the last heading starts below the panel");
    tap(&mut rt, &format!("files-outline-{last}"));
    let h = rect(&rt, &format!("files-text-b{n}"));
    assert!(h[1] >= doc[1] - 0.5 && h[1] + h[3] <= doc[1] + doc[3] + 0.5, "the heading {h:?} is in the panel {doc:?}");
    // The Outline itself did not move, and the entry is the one marked.
    assert!(rt.state.feats.data.get(UI).unwrap()["preview"]["outline"][last]["on"] == true);
}

#[test]
fn a_table_is_drawn_inside_the_panel_with_its_columns_aligned() {
    let mut rt = window(SPEC, 1800.0, 900.0);
    let doc = rect(&rt, "files-doc");
    let ks: Vec<String> = (0..8).map(|b| format!("files-text-b{b}")).collect();
    let t = ks.iter().find_map(|k| rt.presented(&format!("{k}-r1-c0")).map(|_| k.clone())).expect("a table is drawn");
    let whole = rect(&rt, &t);
    assert!(whole[0] >= doc[0] - 0.5 && whole[0] + whole[2] <= doc[0] + doc[2] + 0.5, "the table {whole:?} is inside the panel {doc:?}");
    let (a, b) = (rect(&rt, &format!("{t}-r1-c1")), rect(&rt, &format!("{t}-r2-c1")));
    assert_eq!(a[0], b[0], "the second column starts at the same place in every row");
    let (h, c) = (rect(&rt, &format!("{t}-r0-c1")), rect(&rt, &format!("{t}-r5-c1")));
    assert!(h[0] == c[0] && c[0] + c[2] <= whole[0] + whole[2] + 0.5);
}

/// Plain text, code and a very long file scroll as well, and stay inside the budgets.
#[test]
fn text_and_very_long_files_scroll_and_stay_inside_the_budgets() {
    let many: String = (0..1000).map(|i| format!("## Part {i}\n\nSome words about part {i}, with `code`, **bold** and a [link](https://example.org/{i}).\n\n| a | b |\n|---|---|\n| {i} | x |\n\n")).collect();
    let huge: String = SPEC.repeat(40);
    for (name, text) in [("1000 sections (150 KB)", many), ("200 KB", huge)] {
        let mut rt = window(&text, 1800.0, 900.0);
        let t0 = std::time::Instant::now();
        for _ in 0..10 {
            rt.frame(0.016);
        }
        let doc = rect(&rt, "files-doc");
        assert!(doc[1] + doc[3] <= 900.5, "{name}: {doc:?}");
        let note = rt.state.feats.data.get(UI).unwrap()["preview"]["note"].as_str().unwrap_or("").to_string();
        assert!(note.contains("download it for the rest"), "{name}: a cut file says so: {note:?}");
        let err = rt.state.screens.iter().find(|s| s.name == SCREEN).and_then(|s| s.error.clone());
        assert!(err.is_none(), "{name}: {err:?}");
        eprintln!("{name}: ten frames in {:?}", t0.elapsed());
    }
}
