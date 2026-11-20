//! The keyboard gate (`App::terminal_may_take_keys`), layer by layer.
//!
//! Rule (docs/security.md): while anything sits over the panes (a core
//! dialog, a modal, a feature's overlay, a focused picture surface, Jarvis's
//! dialog, the palette, a menu), no key, chord or paste reaches an agent's
//! terminal, even when the terminal's grid still holds Rune's focus (a layer
//! that opens by itself, a request or an approval, does not take the focus).

use super::*;
use rune::Key;

fn window() -> (rune::Runtime<App>, std::sync::mpsc::Receiver<crate::client::TermInput>) {
    window_on("shell/two-panes")
}

fn window_on(fixture: &str) -> (rune::Runtime<App>, std::sync::mpsc::Receiver<crate::client::TermInput>) {
    let (rt, rx, ev) = window_with_events(fixture);
    std::mem::forget(ev);
    (rt, rx)
}

/// [`window_on`], keeping the terminal's event sender: what the program
/// prints (here, the escape that turns its mouse reporting on).
fn window_with_events(fixture: &str) -> (rune::Runtime<App>, std::sync::mpsc::Receiver<crate::client::TermInput>, std::sync::mpsc::Sender<crate::client::TermEvent>) {
    let fixture = format!("{}/fixtures/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    let (in_tx, in_rx) = std::sync::mpsc::channel();
    let mut t = TerminalState::new(crate::client::TerminalConn { events: ev_rx, input: in_tx });
    t.resize(40, 5);
    t.id = "1".into();
    a.panes[0].sandbox = Some("web-app".into());
    a.panes[0].term = Some(t);
    a.focused = 0;
    let mut rt = rune::Runtime::new(a, view, (1280.0, 800.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
    rt.use_clipboard(&rune::clipboard::memory());
    rt.frame(0.0);
    (rt, in_rx, ev_tx)
}

fn sent(rx: &std::sync::mpsc::Receiver<crate::client::TermInput>) -> Vec<u8> {
    let mut out = vec![];
    while let Ok(i) = rx.try_recv() {
        if let crate::client::TermInput::Bytes(b) = i {
            out.extend(b);
        }
    }
    out
}

fn tap(rt: &mut rune::Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
}

/// Frames, then the app's per-frame work as main.rs runs it (`App::poll`:
/// the window requests, the focus hand-off, what the fixture daemon answered).
fn settle(rt: &mut rune::Runtime<App>, frames: usize) {
    for _ in 0..frames {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
    }
}

fn ctrl_c() -> KeyEvent {
    KeyEvent { key: Key::Text("c".into()), mods: rune::Mods { ctrl: true, ..Default::default() } }
}

/// Types a letter, Ctrl+C, Enter and a paste with the grid holding Rune's
/// focus, and asserts the terminal got none of it.
fn nothing_reaches_the_terminal(rt: &mut rune::Runtime<App>, rx: &std::sync::mpsc::Receiver<crate::client::TermInput>, layer: &str) {
    rt.update(|_, tx| tx.focus("grid-1", false));
    assert!(!rt.state.terminal_may_take_keys(Some("grid-1")), "the gate let keys through with {layer} up");
    let mut keys = vec![KeyEvent { key: Key::Text("x".into()), mods: rune::Mods::default() }, ctrl_c()];
    // Enter runs the palette's first command: the person's own choice there.
    if layer != "the palette" {
        keys.push(KeyEvent { key: Key::Enter, mods: rune::Mods::default() });
    }
    for ev in keys {
        rt.key(ev.clone());
        rt.frame(0.02);
        assert_eq!(sent(rx), b"", "{ev:?} reached the terminal with {layer} up");
    }
    rt.paste("rm -rf ~");
    rt.frame(0.02);
    assert_eq!(sent(rx), b"", "a paste reached the terminal with {layer} up");
}

/// Once the layer is gone, the grid types again (the gate is not shut for good).
fn the_terminal_types_again(rt: &mut rune::Runtime<App>, rx: &std::sync::mpsc::Receiver<crate::client::TermInput>, after: &str) {
    rt.update(|_, tx| tx.focus("grid-1", false));
    rt.key(KeyEvent { key: Key::Text("w".into()), mods: rune::Mods::default() });
    rt.frame(0.02);
    assert_eq!(sent(rx), b"w", "the grid does not type again after {after}");
}

#[test]
fn every_layer_over_the_panes_keeps_keys_from_the_terminal() {
    let (mut rt, rx) = window();
    the_terminal_types_again(&mut rt, &rx, "start");

    // A modal (Setup, Templates...), opened while the grid kept the keyboard.
    rt.update(|s, _| s.open_modal("templates", J::Null));
    rt.frame(0.02);
    assert!(rt.state.feats.modal.is_some());
    nothing_reaches_the_terminal(&mut rt, &rx, "a modal");
    rt.update(|s, _| s.feats.modal = None);
    rt.frame(0.02);
    the_terminal_types_again(&mut rt, &rx, "the modal");

    // The core's dialog (a Confirm with a secret field).
    rt.update(|s, _| {
        let f = App::feature_of("network").unwrap();
        let c = crate::feat::Confirm::new("Save a secret", crate::feat::Call::post("/api/x", serde_json::json!({}), "x")).secret("Token", "value");
        s.feature_actions(f, "network", vec![c.into_action()]);
    });
    rt.frame(0.02);
    assert!(rt.state.feats.dialog.is_some());
    nothing_reaches_the_terminal(&mut rt, &rx, "the core dialog");
    rt.update(|s, _| s.feats.dialog = None);
    rt.frame(0.02);
    the_terminal_types_again(&mut rt, &rx, "the dialog");

    // A feature's overlay over the pane area (Commits, from the rail).
    tap(&mut rt, "rail-commits");
    for _ in 0..10 {
        rt.frame(0.05);
    }
    assert!(rt.state.feats.overlay.is_some());
    nothing_reaches_the_terminal(&mut rt, &rx, "an overlay");
    rt.update(|s, _| s.feats.overlay = None);
    rt.frame(0.02);
    the_terminal_types_again(&mut rt, &rx, "the overlay");

    // The palette.
    rt.update(|s, _| s.palette_open());
    rt.frame(0.02);
    assert!(rt.state.palette.is_some());
    nothing_reaches_the_terminal(&mut rt, &rx, "the palette");
    rt.update(|s, _| s.palette = None);
    rt.frame(0.02);
    the_terminal_types_again(&mut rt, &rx, "the palette");

    // A menu (a pane's ⋯): it takes the keyboard, the terminal gets nothing.
    tap(&mut rt, "pane-1-more");
    for ev in [KeyEvent { key: Key::Text("x".into()), mods: rune::Mods::default() }, ctrl_c()] {
        rt.key(ev.clone());
        rt.frame(0.02);
        assert_eq!(sent(&rx), b"", "{ev:?} reached the terminal with a pane menu open");
    }
    rt.update(|_, tx| assert!(tx.focused().is_none_or(|f| !f.starts_with("grid")), "the menu left the keyboard with the grid"));
    rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
    rt.frame(0.02);
    assert_eq!(sent(&rx), b"", "the Escape that closed the menu reached the terminal");
    the_terminal_types_again(&mut rt, &rx, "the menu");
}

/// Jarvis's dialog for a proposal (high risk: a click only).
/// (Found 2026-11-18: the grid's own handlers, through `lens`, checked only
/// the overlay, so keys reached the terminal under this dialog.)
#[test]
fn jarvis_s_dialog_keeps_keys_from_the_terminal() {
    let (mut rt, rx) = window_on("jarvis/proposal-high");
    rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
    for _ in 0..10 {
        rt.frame(0.05);
    }
    assert!(rt.state.jarvis.proposal("p-high-1").is_some(), "the fixture's proposal");
    assert!(rt.presented("jv-high-note").is_some(), "the high-risk dialog is drawn");
    nothing_reaches_the_terminal(&mut rt, &rx, "Jarvis's dialog");
    rt.update(|s, _| s.jarvis.close_dialog());
    rt.frame(0.02);
    the_terminal_types_again(&mut rt, &rx, "Jarvis's dialog");
}

/// A focused picture surface (the browser, the Mac) holds the keyboard for
/// itself: the gate is shut whatever Rune's focus says.
///
/// `surface::FOCUSED` is process-wide and every runtime frame of another test
/// running in parallel sets it again (`focus_changed`), so one set-then-check
/// can be overwritten in between: it is tried until one check sees the set
/// (with the clause removed, none ever does).
#[test]
fn a_focused_surface_keeps_keys_from_the_terminal() {
    let (rt, _rx) = window();
    let shut = (0..1000).any(|_| {
        crate::surface::focus_changed(Some("surface-gate-test"));
        let seen = crate::surface::focused().as_deref() == Some("gate-test");
        let shut = !rt.state.terminal_may_take_keys(Some("grid-1"));
        crate::surface::focus_changed(None);
        seen && shut && crate::surface::focused().is_none()
    });
    assert!(shut, "the gate let keys through with a picture surface focused");
}

/// Every way input reaches a terminal, under every layer that covers the
/// panes: a key, a chord, text from the input method, the window's paste,
/// Ctrl/Cmd+V (the terminal's own clipboard read), the wheel and a press on
/// the grid. The program has mouse reporting on, so a press or a wheel that
/// got through would send bytes. Each case starts with nothing over the panes.
#[test]
fn no_input_path_reaches_a_terminal_under_a_covering_layer() {
    type Layer = (&'static str, fn(&mut rune::Runtime<App>));
    let layers: [Layer; 5] = [
        ("the core dialog", |rt| {
            rt.update(|s, _| {
                let f = App::feature_of("network").unwrap();
                let c = crate::feat::Confirm::new("Allow it?", crate::feat::Call::post("/api/x", serde_json::json!({}), "x"));
                s.feature_actions(f, "network", vec![c.into_action()]);
            });
        }),
        ("Jarvis's proposal", |rt| rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()))),
        ("a modal", |rt| rt.update(|s, _| s.open_modal("templates", J::Null))),
        // Typed text under an overlay is the overlay's (its `<screen>_key`
        // filter, once it takes text), never a terminal's.
        ("an overlay", |rt| tap(rt, "rail-commits")),
        ("the palette", |rt| rt.update(|s, _| s.palette_open())),
    ];
    type Input = (&'static str, fn(&mut rune::Runtime<App>, (f32, f32)));
    let inputs: [Input; 7] = [
        ("a key", |rt, _| rt.key(KeyEvent { key: Key::Text("x".into()), mods: rune::Mods::default() })),
        ("Ctrl+C", |rt, _| rt.key(ctrl_c())),
        ("input-method text", |rt, _| rt.update(|s, tx| {
            s.on_ime(&rune::Ime::Commit("é".into()), tx.focused());
        })),
        ("the window's paste", |rt, _| rt.paste("rm -rf ~")),
        ("Ctrl/Cmd+V", |rt, _| {
            let mods = if cfg!(target_os = "macos") { rune::Mods { cmd: true, logo: true, ..Default::default() } } else { rune::Mods { ctrl: true, shift: true, ..Default::default() } };
            rt.key(KeyEvent { key: Key::Command('v'), mods });
        }),
        ("the wheel", |rt, (x, y)| rt.wheel_xy(x, y, 0.0, -120.0)),
        ("a press on the grid", |rt, (x, y)| {
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        }),
    ];
    // One window per layer; the layer is closed after each input, so each
    // case starts with nothing over the panes.
    let close_all = |rt: &mut rune::Runtime<App>| {
        rt.update(|s, _| {
            s.feats.dialog = None;
            s.feats.modal = None;
            s.feats.overlay = None;
            s.jarvis.shown = None;
            s.palette = None;
        });
        settle(rt, 2);
    };
    for (layer, open) in layers {
        let (mut rt, rx, ev) = window_with_events("jarvis/proposal-high");
        rt.update(|s, _| s.jarvis.shown = None);
        ev.send(crate::client::TermEvent::Output(b"\x1b[?1000h\x1b[?1006h".to_vec())).unwrap();
        settle(&mut rt, 1);
        for (what, input) in inputs {
            // The window's paste is wired on macOS and in headless runs only.
            if what == "the window's paste" && !super::window_paste() {
                continue;
            }
            close_all(&mut rt);
            let grid = rt.presented("grid-1").expect("the grid is drawn").0;
            let at = (grid[0] + grid[2] / 2.0, grid[1] + grid[3] / 2.0);
            // With nothing over the panes, this input does reach the terminal
            // (so the check below is not passing on a dead input).
            if what != "a press on the grid" && what != "Ctrl/Cmd+V" {
                tap(&mut rt, "grid-1");
                sent(&rx);
                input(&mut rt, at);
                rt.frame(0.02);
                assert!(!sent(&rx).is_empty(), "{what} reaches the terminal with nothing over it");
            }
            open(&mut rt);
            settle(&mut rt, 10);
            assert!(rt.state.panes_covered(), "{layer} covers the panes");
            rt.update(|_, tx| tx.focus("grid-1", false));
            sent(&rx);
            input(&mut rt, at);
            for _ in 0..3 {
                rt.frame(0.02);
            }
            assert_eq!(sent(&rx), b"", "{what} reached the terminal under {layer}");
        }
    }
}

/// A focused surface and a menu keep the keyboard; the OSC 52 copy card is a
/// corner notice, not a layer: keys still go to the terminal, and no key ever
/// accepts the card (only a press on its own button does).
#[test]
fn a_menu_and_the_copy_card_keep_their_own_rules() {
    let (mut rt, rx) = window();
    tap(&mut rt, "pane-1-more");
    rt.update(|s, tx| {
        s.on_ime(&rune::Ime::Commit("é".into()), tx.focused());
    });
    rt.frame(0.02);
    assert_eq!(sent(&rx), b"", "input-method text reached the terminal with a pane menu open");
    rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
    rt.frame(0.02);
    sent(&rx);
    rt.update(|s, _| s.copy_offer = Some(crate::approval::CopyOffer::new("web-app", "secret text".into(), Instant::now())));
    rt.frame(0.02);
    tap(&mut rt, "grid-1");
    for ev in [KeyEvent { key: Key::Text("y".into()), mods: rune::Mods::default() }, KeyEvent { key: Key::Enter, mods: rune::Mods::default() }] {
        rt.key(ev);
        rt.frame(0.02);
    }
    assert!(rt.state.copy_offer.is_some(), "a key accepted the copy card");
    assert_eq!(sent(&rx), b"y\r", "the card is not over the panes: the terminal still types");
}

/// While Jarvis's proposal shows, a scrim covers the pane area: a press,
/// the wheel or a drop on it reaches no terminal and answers nothing; the
/// Jarvis panel stays usable; the keyboard leaves the grid and comes back.
#[test]
fn the_proposal_scrim_holds_the_panes_and_gives_the_keyboard_back() {
    let (mut rt, rx) = window_on("jarvis/proposal-high");
    rt.update(|s, _| s.jarvis.shown = None);
    rt.frame(0.02);
    tap(&mut rt, "grid-1");
    rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
    settle(&mut rt, 5);
    let scrim = rt.presented("jv-scrim").expect("the scrim is drawn").0;
    let panes = rt.presented("panes").unwrap().0;
    assert!((scrim[0] - panes[0]).abs() < 1.0 && (scrim[2] - panes[2]).abs() < 1.0 && (scrim[3] - panes[3]).abs() < 1.0, "it covers the pane area: {scrim:?} {panes:?}");
    let panel = rt.presented("jarvis-panel").expect("the Jarvis panel is open").0;
    assert!(panel[0] >= scrim[0] + scrim[2] - 1.0 || panel[0] + panel[2] <= scrim[0] + 1.0, "the Jarvis panel is not under the scrim");
    rt.update(|_, tx| assert_eq!(tx.focused(), Some(super::jarvis::INPUT), "the keyboard went to Jarvis's input"));
    let grid = rt.presented("grid-1").unwrap().0;
    let (x, y) = (grid[0] + grid[2] / 2.0, grid[1] + grid[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.wheel_xy(x, y, 0.0, -120.0);
    rt.frame(0.02);
    assert_eq!(sent(&rx), b"", "a press or the wheel on the scrim reached the terminal");
    assert_eq!(rt.state.jarvis.shown.as_deref(), Some("p-high-1"), "a press on the scrim closed the proposal");
    assert_eq!(rt.state.jarvis.proposal("p-high-1").unwrap()["state"], "pending", "a press on the scrim answered the proposal");
    // The dialog closes (its ×): the grid has the keyboard again.
    rt.update(|s, _| s.jarvis.close_dialog());
    settle(&mut rt, 10);
    assert!(!rt.state.proposal_up(), "the dialog closed");
    assert!(rt.presented("jv-scrim").is_none(), "the scrim goes with the dialog");
    rt.update(|_, tx| assert_eq!(tx.focused(), Some("grid-1"), "the keyboard came back to the grid"));
    rt.key(KeyEvent { key: Key::Text("w".into()), mods: rune::Mods::default() });
    rt.frame(0.02);
    assert_eq!(sent(&rx), b"w");
}

/// The typed yes still approves a low-risk proposal while its dialog and the
/// scrim show: Jarvis's input is outside the scrim.
#[test]
fn the_typed_yes_still_works_with_the_scrim_up() {
    let (mut rt, rx) = window_on("jarvis/proposal-low");
    rt.update(|s, _| s.jarvis.shown = Some("p-low-1".into()));
    settle(&mut rt, 5);
    assert!(rt.presented("jv-scrim").is_some());
    tap(&mut rt, super::jarvis::INPUT);
    for c in "yes".chars() {
        rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: rune::Mods::default() });
        rt.frame(0.02);
    }
    rt.key(KeyEvent { key: Key::Enter, mods: rune::Mods::default() });
    rt.frame(0.02);
    // The daemon's answer comes back on the app's own thread (fixture mode).
    let until = Instant::now() + Duration::from_secs(3);
    while rt.state.jarvis.proposal("p-low-1").is_some_and(|p| p["state"] == "pending") && Instant::now() < until {
        settle(&mut rt, 1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(rt.state.jarvis.proposal("p-low-1").unwrap()["state"], "applied", "the typed yes did not approve");
    assert_eq!(sent(&rx), b"", "the typed yes reached the terminal");
}

/// `PANE_DROPS` counts drops for the whole process: the tests that read it
/// run one at a time, or one's own drop would count in the other.
fn one_drop_test_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Files dropped on a pane while something covers the panes start nothing:
/// no upload, and no dialog skipped behind the one that shows. Over an
/// overlay a drop is the overlay's (the explorer's upload card, once it
/// takes drops), never the pane's under it: counted where a pane takes one.
#[test]
fn a_drop_on_a_covered_pane_starts_nothing() {
    let _one = one_drop_test_at_a_time();
    let drops = || super::PANE_DROPS.load(std::sync::atomic::Ordering::SeqCst);
    let before = drops();
    let (mut rt, _rx) = window_on("jarvis/proposal-high");
    rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
    settle(&mut rt, 5);
    let grid = rt.presented("grid-1").unwrap().0;
    let file = std::env::temp_dir().join(format!("sbxw-gate-drop-{}.txt", std::process::id()));
    std::fs::write(&file, "x").unwrap();
    rt.drop_files(vec![file.clone()], Some((grid[0] + 10.0, grid[1] + 10.0)));
    settle(&mut rt, 3);
    assert!(rt.state.feats.dialog.is_none(), "a drop under Jarvis's proposal opened the upload dialog");
    rt.update(|s, _| s.jarvis.shown = None);
    rt.update(|s, _| s.open_modal("templates", J::Null));
    settle(&mut rt, 3);
    rt.drop_files(vec![file.clone()], Some((grid[0] + 10.0, grid[1] + 10.0)));
    settle(&mut rt, 3);
    assert!(rt.state.feats.dialog.is_none(), "a drop under a modal opened the upload dialog");
    let (mut rt, _rx) = window_on("jarvis/proposal-high");
    rt.update(|s, _| s.jarvis.shown = None);
    settle(&mut rt, 2);
    tap(&mut rt, "rail-commits");
    settle(&mut rt, 5);
    assert!(rt.state.feats.overlay.is_some());
    rt.drop_files(vec![file.clone()], Some((grid[0] + 10.0, grid[1] + 10.0)));
    settle(&mut rt, 3);
    assert_eq!(drops(), before, "a drop over a covering layer reached a pane");
    let _ = std::fs::remove_file(&file);
    // With nothing over the pane, the same drop opens the upload's dialog.
    let (mut rt, _rx) = window_on("jarvis/proposal-high");
    rt.update(|s, _| s.jarvis.shown = None);
    settle(&mut rt, 2);
    std::fs::write(&file, "x").unwrap();
    rt.drop_files(vec![file.clone()], Some((grid[0] + 10.0, grid[1] + 10.0)));
    settle(&mut rt, 3);
    let _ = std::fs::remove_file(&file);
    assert!(rt.state.feats.dialog.is_some(), "a drop on an uncovered pane opens the upload's dialog");
    assert_eq!(drops(), before + 1, "the uncovered pane took its drop");
}

/// An overlay over the panes (the explorer, Commits) with Jarvis's proposal
/// on top: keys an overlay takes for itself (typed text, Left, Right,
/// Backspace, Up, Down, Enter, forwarded to its `<screen>_key`) reach no
/// terminal; files dragged over it are not offered to it, and dropped on it
/// they start no upload, neither the overlay's (the explorer's card) nor the
/// pane's under it.
#[test]
fn an_overlay_under_jarvis_s_proposal_takes_no_key_or_file_for_a_terminal() {
    let _one = one_drop_test_at_a_time();
    let drops = || super::PANE_DROPS.load(std::sync::atomic::Ordering::SeqCst);
    for overlay in ["rail-files", "rail-commits"] {
        let (mut rt, rx) = window_on("jarvis/proposal-high");
        rt.update(|s, _| s.jarvis.shown = None);
        settle(&mut rt, 2);
        tap(&mut rt, overlay);
        settle(&mut rt, 5);
        assert!(rt.state.feats.overlay.is_some(), "{overlay} opens an overlay");
        let over = rt.presented(&format!("overlay-{}", rt.state.feats.overlay.as_ref().unwrap().0)).expect("the overlay is drawn").0;
        rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
        settle(&mut rt, 5);
        assert!(rt.state.proposal_up());
        rt.update(|_, tx| tx.focus("grid-1", false));
        for key in [Key::Text("a".into()), Key::Left, Key::Right, Key::Backspace, Key::Up, Key::Down, Key::Enter] {
            rt.key(KeyEvent { key: key.clone(), mods: rune::Mods::default() });
            rt.frame(0.02);
            assert_eq!(sent(&rx), b"", "{key:?} reached a terminal under {overlay}'s overlay and Jarvis's proposal");
        }
        let file = std::env::temp_dir().join(format!("sbxw-gate-overlay-{}-{overlay}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        let at = (over[0] + over[2] / 2.0, over[1] + over[3] / 2.0);
        let before = drops();
        rt.drag_files(vec![file.clone()], Some(at));
        settle(&mut rt, 2);
        let hovered = rt.drop_hovered();
        assert!(!hovered.as_deref().is_some_and(|k| k.starts_with("overlay") || k.starts_with("pane")), "{overlay}: a drag under the proposal is offered to {hovered:?}");
        rt.drop_files(vec![file.clone()], Some(at));
        settle(&mut rt, 3);
        assert!(!crate::files::was_handed(&file), "{overlay}: a drop under the proposal was handed to an upload");
        assert_eq!(drops(), before, "{overlay}: a drop under the proposal reached a pane");
        let _ = std::fs::remove_file(&file);
    }
}

/// The Monitor as the base view of the pane area (`Slot::Base`): a file
/// dropped where a terminal is goes to the Monitor (its own drop, never an
/// upload without its dialog), never to the pane under it; with Jarvis's
/// high-risk proposal over the Monitor, nothing is sent and no key reaches
/// the terminal.
#[test]
fn a_drop_with_the_monitor_as_the_base_reaches_no_pane() {
    let _one = one_drop_test_at_a_time();
    let drops = || super::PANE_DROPS.load(std::sync::atomic::Ordering::SeqCst);
    let before = drops();
    let (mut rt, rx) = window_on("jarvis/proposal-high");
    rt.update(|s, _| s.jarvis.shown = None);
    settle(&mut rt, 2);
    let grid = rt.presented("grid-1").unwrap().0;
    tap(&mut rt, "top-view-gui");
    settle(&mut rt, 5);
    assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"));
    let file = std::env::temp_dir().join(format!("sbxw-gate-base-drop-{}.txt", std::process::id()));
    std::fs::write(&file, "x").unwrap();
    rt.drop_files(vec![file.clone()], Some((grid[0] + 10.0, grid[1] + 10.0)));
    settle(&mut rt, 3);
    let _ = std::fs::remove_file(&file);
    assert_eq!(drops(), before, "a drop on the Monitor reached the pane under it");
    assert!(rt.state.feats.dialog.is_none(), "a drop on the Monitor opened an upload dialog by itself");
    // Jarvis's proposal over the Monitor: covered; no key reaches the terminal.
    rt.update(|s, _| s.jarvis.shown = Some("p-high-1".into()));
    settle(&mut rt, 2);
    assert!(rt.state.covered() && rt.state.panes_covered());
    rt.update(|_, tx| tx.focus("grid-1", false));
    rt.key(KeyEvent { key: Key::Text("y".into()), mods: rune::Mods::default() });
    rt.key(KeyEvent { key: Key::Enter, mods: rune::Mods::default() });
    settle(&mut rt, 2);
    assert_eq!(sent(&rx), b"", "a key reached the terminal under the Monitor and the proposal");
}

// ---------------------------------------------------------------------------
// Dictation into a terminal (dictate.rs): F9 held in the pane that has the
// keyboard, the daemon's masked words typed there at release, never Enter.

const F9: Key = Key::Function(9);

/// Two terminals (panes 1 and 2), voice ready, a fake microphone; the grid
/// of pane 1 has the keyboard.
fn voice_window() -> (rune::Runtime<App>, std::sync::mpsc::Receiver<crate::client::TermInput>, std::sync::mpsc::Receiver<crate::client::TermInput>) {
    let (mut rt, rx) = window_on("shell/two-panes-voice");
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    std::mem::forget(ev_tx);
    let (in_tx, rx2) = std::sync::mpsc::channel();
    rt.update(move |s, _| {
        let mut t = TerminalState::new(crate::client::TerminalConn { events: ev_rx, input: in_tx });
        t.resize(40, 5);
        t.id = "2".into();
        if s.panes.len() < 2 {
            let id = s.fresh();
            s.panes.push(Pane::empty(id));
        }
        s.panes[1].sandbox = Some("web-app".into());
        s.panes[1].term = Some(t);
        // As the shell gives every terminal it opens (shell.rs).
        for p in &mut s.panes {
            if let Some(t) = &mut p.term {
                t.keys.reserved = s.wiring.term_reserved;
            }
        }
    });
    rt.frame(0.02);
    rt.update(|_, tx| tx.focus("grid-1", false));
    settle(&mut rt, 1);
    (rt, rx, rx2)
}

fn f9(rt: &mut rune::Runtime<App>, down: bool) {
    let ev = KeyEvent { key: F9, mods: rune::Mods::default() };
    if down {
        rt.key(ev);
    } else {
        rt.key_up(ev);
    }
    settle(rt, 4);
}

fn dictation_notice(rt: &rune::Runtime<App>) -> String {
    rt.state.dictation.notice.as_ref().map(|n| n.2.clone()).unwrap_or_default()
}

#[test]
fn dictated_words_reach_the_focused_terminal_only_and_never_press_enter() {
    let (mut rt, rx, rx2) = voice_window();
    f9(&mut rt, true);
    assert!(rt.state.dictation.on.as_ref().is_some_and(|t| t.term.is_some()), "recording into pane 1: {} focused {}", dictation_notice(&rt), rt.state.focused);
    assert_eq!(sent(&rx), b"", "F9 itself is not sent to the program");
    f9(&mut rt, false);
    let got = String::from_utf8(sent(&rx)).unwrap();
    assert!(got.contains("please send the report to [email] today"), "{got:?}");
    assert!(!got.contains('\r') && !got.contains('\n'), "no Enter: {got:?}");
    assert_eq!(sent(&rx2), b"", "the other terminal gets nothing");
}

#[test]
fn a_cover_or_a_focus_change_during_the_capture_drops_the_words() {
    type Cover = fn(&mut App);
    let covers: [(&str, Cover); 5] = [
        ("a modal", |s| s.open_modal("templates", J::Null)),
        ("the palette", |s| s.palette_open()),
        ("Jarvis's proposal", |s| s.jarvis.shown = Some("p1".into())),
        ("the Sessions base view", |s| s.feats.base = Some("monitor".into())),
        ("an overlay", |s| s.feats.overlay = Some(("commits".into(), J::Null))),
    ];
    for (what, cover) in covers {
        let (mut rt, rx, _rx2) = voice_window();
        f9(&mut rt, true);
        assert!(rt.state.dictation.on.is_some(), "{what}: recording");
        rt.update(move |s, _| cover(s));
        settle(&mut rt, 2);
        f9(&mut rt, false);
        assert_eq!(sent(&rx), b"", "{what}: nothing typed");
        assert!(rt.state.dictation.on.is_none() && rt.state.dictation.pending.is_none(), "{what}");
        assert!(dictation_notice(&rt).contains("cancelled"), "{what}: {}", dictation_notice(&rt));
    }
    // The keyboard moves to the other pane meanwhile.
    let (mut rt, rx, rx2) = voice_window();
    f9(&mut rt, true);
    rt.update(|s, tx| {
        s.focused = 1;
        tx.focus("grid-2", false);
    });
    settle(&mut rt, 2);
    f9(&mut rt, false);
    assert_eq!(sent(&rx), b"");
    assert_eq!(sent(&rx2), b"", "not the pane that got the keyboard either");
}

#[test]
fn the_gate_closed_when_the_words_arrive_drops_them() {
    let (mut rt, rx, _rx2) = voice_window();
    f9(&mut rt, true);
    // Released, then a dialog before the words are typed: the words find
    // the gate closed (the capture was over, so nothing cancelled it).
    rt.update(|s, _| {
        s.dictate_stop();
        let f = App::feature_of("network").unwrap();
        let c = crate::feat::Confirm::new("Save a secret", crate::feat::Call::post("/api/x", serde_json::json!({}), "x"));
        s.feature_actions(f, "network", vec![c.into_action()]);
    });
    settle(&mut rt, 4);
    assert_eq!(sent(&rx), b"", "nothing under the dialog");
    assert!(dictation_notice(&rt).starts_with("Not typed:"), "{}", dictation_notice(&rt));
}

#[test]
fn f9_under_a_layer_starts_nothing_and_types_nothing() {
    let (mut rt, rx, _rx2) = voice_window();
    rt.update(|s, _| s.open_modal("templates", J::Null));
    rt.frame(0.02);
    rt.update(|_, tx| tx.focus("grid-1", false));
    f9(&mut rt, true);
    f9(&mut rt, false);
    assert!(rt.state.dictation.on.is_none());
    assert_eq!(sent(&rx), b"");
}

#[test]
fn without_voice_f9_stays_the_programs_key() {
    let (mut rt, rx) = window();
    rt.update(|s, tx| {
        let r = s.wiring.term_reserved;
        s.panes[0].term.as_mut().unwrap().keys.reserved = r;
        tx.focus("grid-1", false);
    });
    rt.frame(0.02);
    rt.key(KeyEvent { key: F9, mods: rune::Mods::default() });
    rt.frame(0.02);
    assert_eq!(sent(&rx), b"\x1b[20~", "a daemon without voice: F9 goes to the program");
    assert!(rt.state.dictation.on.is_none());
}

/// Sessions (the base view) has no head band: both columns start at the
/// panel's padding under its top edge, and nothing is drawn for a head row
/// or a close cross. Another overlay (Files) keeps its head with title and
/// close. The same with Lead Mode on.
#[test]
fn sessions_has_no_head_band_and_files_keeps_its_head() {
    for lead in [false, true] {
        let (mut rt, _rx) = window_on("monitor/basic");
        settle(&mut rt, 2);
        tap(&mut rt, "top-view-gui");
        settle(&mut rt, 5);
        if lead {
            tap(&mut rt, "top-lead");
            settle(&mut rt, 5);
        }
        assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"));
        let over = rt.presented("overlay-monitor").unwrap().0;
        assert!(rt.presented("overlay-head-monitor").is_none(), "lead {lead}: a head row over Sessions");
        assert!(rt.presented("overlay-close").is_none(), "lead {lead}: a close cross over Sessions");
        let list = rt.presented("monitor-list-col").unwrap().0;
        let pad = list[1] - over[1];
        assert!((11.0..=14.0).contains(&pad), "lead {lead}: the list starts {pad} under the panel's top");
        if let Some((d, _)) = rt.presented("monitor-detail") {
            assert!((d[1] - list[1]).abs() < 0.5, "lead {lead}: the detail starts with the list");
        }
    }
    let (mut rt, _rx) = window_on("scope/many");
    settle(&mut rt, 2);
    let v = serde_json::json!({ "kind": "intent", "name": "files_open", "args": { "sandbox": "web-app" } });
    for i in crate::intent::parse(&v).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(&mut rt, 5);
    let over = rt.presented("overlay-files").expect("Files open").0;
    let head = rt.presented("overlay-head-files").expect("Files keeps its head row").0;
    assert!(head[1] - over[1] >= 8.0 && head[3] >= crate::theme::layout::CONTROL_H - 0.5, "{head:?} in {over:?}");
    assert!(rt.presented("overlay-close").is_some(), "Files keeps its close cross");
}
