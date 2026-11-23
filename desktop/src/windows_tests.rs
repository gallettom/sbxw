//! The window hub as a model: open, focus, one place, close, the messages
//! and the gates. (The canvas window driven with the app is in
//! `feat/canvas/tests.rs`, "The canvas window".)

use super::*;

fn gate() -> Option<Gate> {
    Some(Gate { owner: PRIMARY, what: "A confirmation waits for you in the main window.".into() })
}

fn intents() -> ToPrimary {
    ToPrimary::Intents { screen: CANVAS_SCREEN.into(), intents: vec![], editing: false }
}

#[test]
fn asking_again_focuses_the_one_canvas_window() {
    let mut h = Hub::default();
    assert!(!h.canvas_open());
    assert_eq!(h.open_canvas(), Opened::New);
    assert_eq!(h.open_canvas(), Opened::Focused, "never a second canvas window");
    assert!(h.focus_canvas());
    assert_eq!(h.take_native(), vec![Native::Open(CANVAS), Native::Focus(CANVAS), Native::Focus(CANVAS)]);
}

#[test]
fn closing_the_window_hands_the_view_back_once() {
    let mut h = Hub::default();
    h.open_canvas();
    h.window_closed(CANVAS);
    h.window_closed(CANVAS);
    let got = h.take_for_primary();
    assert!(matches!(got.as_slice(), [(CANVAS, ToPrimary::Closed)]), "{got:?}");
    assert!(!h.canvas_open());
    // Taken back from the main window: the window closes, and its on_close says nothing more.
    h.open_canvas();
    assert!(h.close_canvas());
    h.window_closed(CANVAS);
    assert!(h.take_for_primary().is_empty());
    assert_eq!(h.take_native().last(), Some(&Native::Close(CANVAS)));
}

#[test]
fn input_from_a_closed_window_is_dropped_but_its_close_is_not() {
    let mut h = Hub::default();
    assert!(!h.post(CANVAS, intents()));
    assert!(h.post(CANVAS, ToPrimary::Resized));
}

/// A gate up in the main window holds the canvas window's input, every
/// kind of it; what is not input (its size, its close) still goes.
#[test]
fn the_main_windows_gate_holds_every_input_of_the_canvas_window() {
    let mut h = Hub::default();
    h.open_canvas();
    h.set_gate(gate());
    for m in [intents(), ToPrimary::Key(serde_json::json!({ "key": "enter" })), ToPrimary::Files(vec!["/tmp/a.png".into()]), ToPrimary::Png(vec![1])] {
        assert!(!h.post(CANVAS, m));
    }
    assert_eq!(h.refused, 4);
    assert!(h.post(CANVAS, ToPrimary::Resized));
    // An answer to a confirmation that is not the canvas window's: refused.
    assert!(!h.post(CANVAS, ToPrimary::Answer { confirm: true, dialog: "x".into() }));
    h.set_gate(None);
    assert!(h.post(CANVAS, intents()));
}

/// A confirmation belongs to the window whose input raised it: only that
/// window's answer, to that very confirmation, is taken.
#[test]
fn a_confirmation_is_answered_by_its_own_window_only() {
    let mut h = Hub::default();
    h.open_canvas();
    h.dialog_raised("send-1".into(), CANVAS);
    assert_eq!(h.dialog_owner(), Some(CANVAS));
    // Its own view input waits behind it (it is modal there).
    assert!(!h.post(CANVAS, intents()));
    // From the main window: never.
    assert!(!h.post(PRIMARY, ToPrimary::Answer { confirm: true, dialog: "send-1".into() }));
    // Another confirmation's identity: never.
    assert!(!h.post(CANVAS, ToPrimary::Answer { confirm: true, dialog: "send-0".into() }));
    assert!(h.post(CANVAS, ToPrimary::Answer { confirm: true, dialog: "send-1".into() }));
    // Seen again at the end of a turn: still the canvas window's.
    h.note_dialog(Some("send-1".into()));
    assert_eq!(h.dialog_owner(), Some(CANVAS));
    // One the hub never saw came from the main window.
    h.note_dialog(Some("other".into()));
    assert_eq!(h.dialog_owner(), Some(PRIMARY));
    h.note_dialog(None);
    assert_eq!(h.dialog_owner(), None);
}

#[test]
fn a_confirmation_raised_from_a_closed_window_belongs_to_the_main_one() {
    let mut h = Hub::default();
    h.dialog_raised("x".into(), CANVAS);
    assert_eq!(h.dialog_owner(), Some(PRIMARY));
}

/// Closing the canvas window, or taking the view back, cancels its
/// confirmation (before the view comes back); never confirms it.
#[test]
fn closing_the_window_cancels_its_confirmation() {
    for by_person in [true, false] {
        let mut h = Hub::default();
        h.open_canvas();
        h.dialog_raised("ask-1".into(), CANVAS);
        if by_person {
            h.window_closed(CANVAS);
        } else {
            h.close_canvas();
        }
        let got = h.take_for_primary();
        assert!(matches!(got.first(), Some((CANVAS, ToPrimary::Answer { confirm: false, dialog })) if dialog == "ask-1"), "{got:?}");
        assert!(!got.iter().any(|(_, m)| matches!(m, ToPrimary::Answer { confirm: true, .. })));
    }
}

#[test]
fn the_canvas_window_draws_again_when_what_it_shows_changed() {
    let mut h = Hub::default();
    h.open_canvas();
    assert!(h.take_canvas_dirty(), "drawn when opened");
    h.note_drawn_from(1, 0);
    assert!(h.take_canvas_dirty());
    h.note_drawn_from(1, 0);
    assert!(!h.take_canvas_dirty(), "nothing changed");
    h.note_drawn_from(1, 1);
    assert!(h.take_canvas_dirty(), "the surface changed");
    h.set_gate(gate());
    assert!(h.take_canvas_dirty(), "a gate came up");
    h.set_gate(gate());
    assert!(!h.take_canvas_dirty());
}

#[test]
fn the_size_is_news_only_when_it_moved() {
    let mut h = Hub::default();
    assert!(h.set_canvas_size([800.0, 600.0]));
    assert!(!h.set_canvas_size([800.5, 600.0]));
    assert!(h.set_canvas_size([900.0, 600.0]));
}

#[test]
fn full_screen_is_the_platforms_key() {
    let ev = |key: rune::Key, mods: rune::Mods| KeyEvent { key, mods };
    if cfg!(target_os = "macos") {
        let m = rune::Mods { ctrl: true, logo: true, cmd: true, ..Default::default() };
        assert!(full_screen_key(&ev(rune::Key::Text("f".into()), m)));
        assert!(!full_screen_key(&ev(rune::Key::Text("f".into()), rune::Mods { logo: true, cmd: true, ..Default::default() })));
    } else {
        assert!(full_screen_key(&ev(rune::Key::Function(11), rune::Mods::default())));
        assert!(!full_screen_key(&ev(rune::Key::Function(10), rune::Mods::default())));
    }
}
