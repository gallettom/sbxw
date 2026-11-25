//! The counters of perf.rs: absent when the switch is off, and each one
//! moves in a scripted scenario when it is on (scripts/measure.sh reads them).

use super::layout_tests::Rig;
use super::*;
use crate::client::TermEvent;
use crate::perf::{self, C};

/// Two panes on two pages, output in both, a second of frames (the cursor
/// blinks), the panes covered by an overlay for another second, a stream
/// event, and a screen's file watcher for a moment.
fn scenario() {
    let mut r = Rig::new(&["web-app", "docs"], 1284.0, 816.0);
    // The app's poll on every frame, as in a real run (`main.rs`).
    r.rt.poll = Some(|s: &mut App, clock: f64, tx: &mut Tx| s.poll(clock, tx));
    r.op(PageOp::Per(1));
    assert_eq!(r.rt.state.pages.list.len(), 2, "one pane per page");
    let lines: String = (0..200).map(|i| format!("line {i}\r\n")).collect();
    for tx in &r.events {
        tx.send(TermEvent::Size { cols: 80, rows: 24, owner: true }).unwrap();
        tx.send(TermEvent::Output(lines.clone().into_bytes())).unwrap();
    }
    r.settle();
    r.rt.update(|s, _| {
        s.focused = 0;
        s.refocus = true;
        s.window_focused = true;
    });
    r.settle();
    // Covered: the help over the panes, the cursor still blinking under it.
    r.rt.update(|s, _| s.show_help());
    r.settle();
    r.rt.update(|s, _| {
        s.feats_bus("session", "{}");
    });
    r.settle();
    // A screen's watcher thread (started by `setup` in a real run).
    let mut sc = ScreenState::lazy("help", std::path::Path::new("/nonexistent"), serde_json::json!({}));
    sc.watch(Arc::new(|| {}));
    std::thread::sleep(Duration::from_millis(400));
}

/// Off (the default), nothing is counted: every probe is one cached load.
#[test]
fn nothing_is_counted_without_the_switch() {
    let _l = perf::test_lock();
    perf::set_for_test(None);
    perf::reset();
    scenario();
    assert!(perf::snapshot().is_empty(), "counted with the switch off: {:?}", perf::snapshot());
    assert!(perf::take_marks().is_empty());
}

/// On (`SBXW_DESKTOP_PERF=counts`), every counter the CPU audit's findings
/// turn on moves in the scenario.
#[test]
fn each_counter_moves_with_the_switch_on() {
    let _l = perf::test_lock();
    perf::set_for_test(Some(false));
    perf::reset();
    scenario();
    let got = perf::snapshot();
    perf::set_for_test(None);
    for c in C::ALL {
        assert!(got.get(c.name()).is_some_and(|v| *v > 0), "{} did not move: {got:?}", c.name());
    }
    assert!(got.keys().any(|k| k.starts_with("render.")), "renders by screen: {got:?}");
    assert_eq!(got.get("event.session"), Some(&1), "{got:?}");
    assert!(got.contains_key("fetch.sessions") && got.contains_key("fetch.sandboxes"), "reads by key: {got:?}");
    assert!(got["blinks_covered"] <= got["blinks"]);
}

/// The per-second line: only what moved, by name, after the time and CPU.
#[test]
fn the_line_names_what_moved() {
    let now: std::collections::BTreeMap<String, u64> = [("polls".to_string(), 7), ("views".to_string(), 3), ("render.sandboxes".to_string(), 2)].into();
    let last: std::collections::BTreeMap<String, u64> = [("polls".to_string(), 5), ("views".to_string(), 3)].into();
    assert_eq!(perf::line(4, Some(12.5), &now, &last), "perf: counts t=4 cpu_ms=12.5 polls=2 render.sandboxes=2");
    assert_eq!(perf::line(1, None, &now, &now), "perf: counts t=1 cpu_ms=-");
}
