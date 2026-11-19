//! The core's notice (`notice_view`): its kind, its ground, where it sits.

use super::*;

fn window(w: f32, h: f32) -> rune::Runtime<App> {
    let fixture = format!("{}/fixtures/shell/two-panes.json", env!("CARGO_MANIFEST_DIR"));
    let a = App::new(Args { fixture: Some(fixture), ..Args::default() });
    let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// Error, needs you (an offer to act), news: red, amber, blue, each on its own ground.
#[test]
fn each_kind_has_its_colour_ground_and_icon() {
    assert_eq!(notice_kind(true, true), ("red", "notice-error", "warning"));
    assert_eq!(notice_kind(false, true), ("amber", "notice-ask", "ask"));
    assert_eq!(notice_kind(false, false), ("blue", "notice-info", "info"));
}

/// Three notices in a row: one is drawn, the last; a newer one replaces
/// the older (when and what a notice does is unchanged).
#[test]
fn three_notices_in_a_row_draw_one() {
    let mut rt = window(1284.0, 816.0);
    for t in ["Copied 4.9 KB from web-app", "Not sent: the session ended", "Sent to web-app"] {
        rt.update(move |s, _| s.notify(t, t.starts_with("Not")));
        rt.frame(0.05);
    }
    assert_eq!(rt.state.notice.as_ref().map(|n| n.0.as_str()), Some("Sent to web-app"));
    assert!(rt.presented("notice").is_some());
    assert!(rt.find("notice-close").is_some());
}

/// In the top bar, at its right end, clear of its controls and of the
/// macOS window buttons' room, at 1284 and at 720.
#[test]
fn the_notice_sits_in_the_bar_clear_of_its_controls() {
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let mut rt = window(w, h);
        rt.update(|s, _| {
            s.notify("billing-api · retry-queue asks: Claude needs a decision about the retry queue before it goes on", false);
            s.feats.notice_then = Some(("relay_open".into(), J::Null));
        });
        for _ in 0..4 {
            rt.frame(0.05);
        }
        let (n, _) = rt.presented("notice").expect("drawn");
        let (bar, _) = rt.presented("top-bar").expect("the bar");
        assert!(n[0] >= bar[0] && n[0] + n[2] <= bar[0] + bar[2] + 0.5 && n[1] >= bar[1] && n[1] + n[3] <= bar[1] + bar[3] + 0.5, "{w}: inside the bar: {n:?} in {bar:?}");
        for k in ["traffic-lights-room", "top-view-gui", "top-view-tui"] {
            if let Some((r, _)) = rt.presented(k) {
                assert!(!overlaps(n, r), "{w}: the notice covers {k}: {n:?} {r:?}");
            }
        }
        assert!(n[2] <= 380.0 + 0.5, "{w}: at most 380 pt wide");
    }
}
