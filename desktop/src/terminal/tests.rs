use super::*;
use crate::client::{TermEvent, TermInput, TerminalConn};
use rune::{Key, KeyEvent, Mods, Phase, PointerButton};
use std::sync::mpsc::{self, Receiver, Sender};

/// A terminal over channels: what the "daemon" sends, what the terminal wrote.
fn term() -> (TerminalState, Sender<TermEvent>, Receiver<TermInput>) {
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    t.resize(40, 5);
    t.poll(None);
    (t, ev_tx, in_rx)
}

/// The bytes written since the last call (resizes dropped).
fn written(rx: &Receiver<TermInput>) -> Vec<u8> {
    let mut out = vec![];
    while let Ok(i) = rx.try_recv() {
        if let TermInput::Bytes(b) = i {
            out.extend(b);
        }
    }
    out
}

fn resizes(rx: &Receiver<TermInput>) -> Vec<(u16, u16)> {
    let mut out = vec![];
    while let Ok(i) = rx.try_recv() {
        if let TermInput::Resize { cols, rows } = i {
            out.push((cols, rows));
        }
    }
    out
}

fn key(key: Key, f: impl FnOnce(&mut Mods)) -> KeyEvent {
    let mut mods = Mods::default();
    f(&mut mods);
    KeyEvent { key, mods }
}

/// A point in the left half of cell (col, row), where a press starts a
/// selection that includes the cell.
fn at(col: usize, row: usize) -> (f32, f32) {
    ((col as f32 + 0.25) * cell_width(), (row as f32 + 0.5) * LINE_HEIGHT)
}

/// A point in the right half of the cell, where a drag ends to include it.
fn at_end(col: usize, row: usize) -> (f32, f32) {
    ((col as f32 + 0.75) * cell_width(), (row as f32 + 0.5) * LINE_HEIGHT)
}

fn drag(t: &mut TerminalState, phase: Phase, from: (usize, usize), to: (usize, usize), clicks: u32, mods: Mods) -> Option<String> {
    let (sx, sy) = at(from.0, from.1);
    let (x, y) = if to == from { (sx, sy) } else { at_end(to.0, to.1) };
    let d = DragInput { phase, x, y, start_x: sx, start_y: sy, dx: x - sx, dy: y - sy, clicks };
    t.drag(d, PointerButton::Primary, mods)
}

fn lines(n: usize) -> Vec<u8> {
    (1..=n).map(|i| format!("line {i}\r\n")).collect::<String>().into_bytes()
}

#[test]
fn scrollback_by_wheel_and_keys_and_back_on_input() {
    let (mut t, _ev, rx) = term();
    t.feed(&lines(100));
    // 100 lines and the empty last one on a 5-row screen: 96 in the history.
    assert_eq!(t.history_len(), 96);
    assert_eq!(t.visible_text().lines().next(), Some("line 97"));
    t.scroll_lines(10);
    assert_eq!(t.display_offset(), 10);
    assert_eq!(t.visible_text().lines().next(), Some("line 87"));
    // The live screen did not move.
    assert_eq!(t.screen_text().lines().next(), Some("line 97"));
    // Shift+PageUp: a page is the screen less one line.
    assert!(t.on_key(&key(Key::PageUp, |m| m.shift = true)));
    assert_eq!(t.display_offset(), 14);
    assert!(t.on_key(&key(Key::PageDown, |m| m.shift = true)));
    assert_eq!(t.display_offset(), 10);
    assert!(t.on_key(&key(Key::Home, |m| m.shift = true)));
    assert_eq!(t.display_offset(), 96);
    assert_eq!(t.visible_text().lines().next(), Some("line 1"));
    // Wheel: 3 lines' worth of points toward the history, then back.
    t.scroll_to_bottom();
    t.on_wheel(3.0 * LINE_HEIGHT, 0.0, 0.0, Mods::default());
    assert_eq!(t.display_offset(), 3);
    t.on_wheel(-LINE_HEIGHT * 0.5, 0.0, 0.0, Mods::default());
    assert_eq!(t.display_offset(), 3, "half a line waits");
    t.on_wheel(-LINE_HEIGHT * 0.5, 0.0, 0.0, Mods::default());
    assert_eq!(t.display_offset(), 2);
    // Typing jumps to the bottom; scrolling sent nothing.
    assert_eq!(written(&rx), b"");
    assert!(t.on_key(&key(Key::Text("x".into()), |_| {})));
    assert_eq!(t.display_offset(), 0);
    assert_eq!(written(&rx), b"x");
    // In a full-screen program Shift+PageUp goes to the program.
    t.feed(b"\x1b[?1049h");
    assert!(t.on_key(&key(Key::PageUp, |m| m.shift = true)));
    assert_eq!(t.display_offset(), 0);
    assert_eq!(written(&rx), b"\x1b[5;2~");
}

#[test]
fn history_is_capped() {
    let (mut t, _ev, _rx) = term();
    t.feed(&lines(SCROLLBACK + 500));
    assert_eq!(t.history_len(), SCROLLBACK);
}

#[test]
fn wheel_in_a_full_screen_program() {
    let (mut t, _ev, rx) = term();
    t.feed(&lines(50));
    // The alternate screen with no mouse reports: arrows (xterm's alternate scroll).
    t.feed(b"\x1b[?1049h");
    t.on_wheel(2.0 * LINE_HEIGHT, 0.0, 0.0, Mods::default());
    assert_eq!(written(&rx), b"\x1b[A\x1b[A");
    t.feed(b"\x1b[?1h");
    t.on_wheel(-LINE_HEIGHT, 0.0, 0.0, Mods::default());
    assert_eq!(written(&rx), b"\x1bOB");
    assert_eq!(t.display_offset(), 0);
    // With SGR mouse reports: wheel buttons at the pointer's cell.
    t.feed(b"\x1b[?1000h\x1b[?1006h");
    let (x, y) = at(4, 2);
    t.on_wheel(LINE_HEIGHT, x, y, Mods::default());
    assert_eq!(written(&rx), b"\x1b[<64;5;3M");
    // Leaving the alternate screen gives the history back.
    t.feed(b"\x1b[?1000l\x1b[?1049l");
    t.on_wheel(LINE_HEIGHT, 0.0, 0.0, Mods::default());
    assert_eq!(t.display_offset(), 1);
}

#[test]
fn selection_by_drag_word_and_line() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"hello wide world\r\nsecond line here");
    let none = Mods::default();
    // Drag from 'w' of "wide" to the 'd' of "world".
    drag(&mut t, Phase::Down, (6, 0), (6, 0), 1, none);
    drag(&mut t, Phase::Move, (6, 0), (15, 0), 1, none);
    drag(&mut t, Phase::Up, (6, 0), (15, 0), 1, none);
    assert_eq!(t.selection_text().as_deref(), Some("wide world"));
    // Across two lines.
    drag(&mut t, Phase::Down, (11, 0), (11, 0), 1, none);
    drag(&mut t, Phase::Move, (11, 0), (5, 1), 1, none);
    assert_eq!(t.selection_text().as_deref(), Some("world\nsecond"));
    // A click clears it.
    drag(&mut t, Phase::Down, (2, 0), (2, 0), 1, none);
    drag(&mut t, Phase::Up, (2, 0), (2, 0), 1, none);
    assert_eq!(t.selection_text(), None);
    // Double click: the word; triple: the line.
    drag(&mut t, Phase::Down, (8, 0), (8, 0), 2, none);
    assert_eq!(t.selection_text().as_deref(), Some("wide"));
    drag(&mut t, Phase::Down, (8, 1), (8, 1), 3, none);
    assert_eq!(t.selection_text().as_deref(), Some("second line here"));
    // Typing clears the selection.
    t.on_key(&key(Key::Text("a".into()), |_| {}));
    assert_eq!(t.selection_text(), None);
}

#[test]
fn selection_reaches_into_the_history() {
    let (mut t, _ev, _rx) = term();
    t.feed(&lines(20));
    t.scroll_lines(3);
    // 20 lines and the prompt's empty one: the top visible row is now "line 14".
    drag(&mut t, Phase::Down, (0, 0), (0, 0), 3, Mods::default());
    assert_eq!(t.selection_text().as_deref(), Some("line 14"));
    t.select_all();
    let all = t.selection_text().unwrap();
    assert!(all.starts_with("line 1\nline 2\n") && all.contains("line 20"), "{all}");
}

#[test]
fn wrapped_lines_select_as_one() {
    let (mut t, _ev, _rx) = term();
    let long = "x".repeat(50);
    t.feed(long.as_bytes());
    drag(&mut t, Phase::Down, (0, 0), (0, 0), 3, Mods::default());
    assert_eq!(t.selection_text().as_deref(), Some(long.as_str()));
}

#[test]
fn bracketed_paste() {
    let (mut t, _ev, rx) = term();
    t.paste("echo hi\nls\n");
    assert_eq!(written(&rx), b"echo hi\rls\r");
    t.feed(b"\x1b[?2004h");
    t.paste("echo hi\n");
    assert_eq!(written(&rx), b"\x1b[200~echo hi\r\x1b[201~");
    t.feed(b"\x1b[?2004l");
    t.paste("x");
    assert_eq!(written(&rx), b"x");
}

/// A paste's text is typed as ever; a picture with no text beside it is
/// typed as nothing and kept, once, for the shell to upload; with both, the
/// text wins (a file copied in Finder: its name and its icon).
#[test]
fn a_paste_types_its_text_and_keeps_a_picture_with_no_text_for_the_shell() {
    let (mut t, _ev, rx) = term();
    let png = crate::feat::canvas::model::mock_png(4, 3, 1);
    let img = || rune::ClipboardImage::from_png(png.clone()).unwrap();
    t.paste_content(&rune::ClipboardContent::new("ls -la"));
    assert_eq!(written(&rx), b"ls -la");
    assert_eq!(t.take_pasted_image(), None);
    t.paste_content(&rune::ClipboardContent::new("").with_image(img()));
    assert_eq!(written(&rx), b"", "a picture types nothing");
    assert_eq!(t.take_pasted_image().as_deref(), Some(png.as_slice()));
    assert_eq!(t.take_pasted_image(), None, "once");
    t.paste_content(&rune::ClipboardContent::new("shot.png").with_image(img()));
    assert_eq!(written(&rx), b"shot.png");
    assert_eq!(t.take_pasted_image(), None, "text wins");
    t.paste_content(&rune::ClipboardContent::new(""));
    assert_eq!((written(&rx), t.take_pasted_image()), (vec![], None), "nothing: nothing");
}

#[test]
fn mouse_reports_for_programs_and_shift_selects() {
    let (mut t, _ev, rx) = term();
    t.feed(b"some text\x1b[?1002h\x1b[?1006h");
    let none = Mods::default();
    drag(&mut t, Phase::Down, (2, 1), (2, 1), 1, none);
    drag(&mut t, Phase::Move, (2, 1), (3, 1), 1, none);
    drag(&mut t, Phase::Move, (2, 1), (3, 1), 1, none);
    drag(&mut t, Phase::Up, (2, 1), (3, 1), 1, none);
    assert_eq!(written(&rx), b"\x1b[<0;3;2M\x1b[<32;4;2M\x1b[<0;4;2m");
    assert_eq!(t.selection_text(), None);
    // Shift: a selection, no report.
    let shift = Mods { shift: true, ..Mods::default() };
    drag(&mut t, Phase::Down, (0, 0), (0, 0), 1, shift);
    drag(&mut t, Phase::Move, (0, 0), (3, 0), 1, shift);
    drag(&mut t, Phase::Up, (0, 0), (3, 0), 1, shift);
    assert_eq!(t.selection_text().as_deref(), Some("some"));
    assert_eq!(written(&rx), b"");
    // A right click reports a press and a release.
    let (x, y) = at(0, 0);
    assert!(t.on_secondary_click(x, y, none));
    assert_eq!(written(&rx), b"\x1b[<2;1;1M\x1b[<2;1;1m");
    // Motion with no button only under 1003.
    t.on_pointer_move(x, y, none);
    assert_eq!(written(&rx), b"");
    t.feed(b"\x1b[?1003h");
    let (x, y) = at(5, 4);
    t.on_pointer_move(x, y, none);
    assert_eq!(written(&rx), b"\x1b[<35;6;5M");
}

#[test]
fn focus_reports() {
    let (mut t, _ev, rx) = term();
    t.set_focused(false);
    assert_eq!(written(&rx), b"");
    t.set_focused(true);
    t.feed(b"\x1b[?1004h");
    t.set_focused(false);
    t.set_focused(true);
    assert_eq!(written(&rx), b"\x1b[O\x1b[I");
}

#[test]
fn cursor_shapes() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"$ ");
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Block));
    t.feed(b"\x1b[6 q");
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Beam));
    t.feed(b"\x1b[4 q");
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Underline));
    t.set_focused(false);
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Hollow));
    t.set_focused(true);
    t.feed(b"\x1b[?25l");
    assert_eq!(t.cursor, None);
    assert_eq!(t.cursor_shape(), None);
    t.feed(b"\x1b[?25h");
    // Scrolled into the history, the cursor is off screen.
    t.feed(&lines(20));
    t.scroll_lines(10);
    t.poll(None);
    assert_eq!(t.cursor, None);
}

#[test]
fn only_damaged_rows_are_rebuilt() {
    let (mut t, _ev, _rx) = term();
    t.resize(80, 24);
    t.feed(&lines(23));
    let before = t.rebuilt_rows;
    // One character on the prompt line: that row (and the cursor's) only.
    t.feed(b"a");
    assert!(t.rebuilt_rows - before <= 2, "{} rows rebuilt", t.rebuilt_rows - before);
    let before = t.rebuilt_rows;
    // Nothing new: at most the cursor's row, which alacritty always reports.
    t.feed(b"");
    assert!(t.rebuilt_rows - before <= 1);
    // A full repaint rebuilds every row.
    let before = t.rebuilt_rows;
    t.feed(b"\x1b[H\x1b[2J");
    assert_eq!(t.rebuilt_rows - before, 24);
    // A selection rebuilds the rows it covers.
    let before = t.rebuilt_rows;
    t.feed(&lines(5));
    let fed = t.rebuilt_rows - before;
    assert!(fed <= 7, "{fed}");
}

#[test]
fn wide_and_combining_characters_keep_their_columns() {
    let (mut t, _ev, _rx) = term();
    t.feed("wide:日本語|e\u{301}|".as_bytes());
    assert_eq!(t.screen_text().lines().next(), Some("wide:日本語|e\u{301}|"));
    // The bar after the ideographs sits in column 11.
    let (x, y) = at(11, 0);
    t.select_start(x, y, 1);
    let (x, y) = at_end(11, 0);
    t.select_update(x, y);
    assert_eq!(t.selection_text().as_deref(), Some("|"));
    // Double click on an ideograph selects the run between separators.
    let (x, y) = at(6, 0);
    t.select_start(x, y, 2);
    assert_eq!(t.selection_text().as_deref(), Some("日本語"));
}

#[test]
fn osc8_links_hover_and_open_with_the_primary_modifier() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"see \x1b]8;;https://example.org/x\x07the docs\x1b]8;;\x07 or \x1b]8;;file:///etc/passwd\x07this\x1b]8;;\x07");
    let (x, y) = at(6, 0);
    t.on_pointer_move(x, y, Mods::default());
    assert!(t.hovering_link());
    let (x2, y2) = at(1, 0);
    t.on_pointer_move(x2, y2, Mods::default());
    assert!(!t.hovering_link());
    let cmd = Mods { cmd: true, ..Mods::default() };
    assert_eq!(drag(&mut t, Phase::Down, (6, 0), (6, 0), 1, cmd), None);
    assert_eq!(drag(&mut t, Phase::Up, (6, 0), (6, 0), 1, cmd).as_deref(), Some("https://example.org/x"));
    // Without the modifier a click does not open; file links never do.
    drag(&mut t, Phase::Down, (6, 0), (6, 0), 1, Mods::default());
    assert_eq!(drag(&mut t, Phase::Up, (6, 0), (6, 0), 1, Mods::default()), None);
    drag(&mut t, Phase::Down, (18, 0), (18, 0), 1, cmd);
    assert_eq!(drag(&mut t, Phase::Up, (18, 0), (18, 0), 1, cmd), None);
}

#[test]
fn resize_is_debounced_with_poll_tx() {
    let (mut t, _ev, rx) = term();
    assert_eq!(resizes(&rx), vec![(40, 5)], "the default 100x30 is never sent");
    let mut tx = Tx::default();
    let w = |cols: usize| PAD * 2.0 + cols as f32 * cell_width() + 0.5;
    let h = PAD * 2.0 + STATUS_H + 10.0 * LINE_HEIGHT + 0.5;
    // A drag over several frames, 16 ms apart.
    for (i, cols) in [60, 61, 62, 63, 64].iter().enumerate() {
        let clock = 1.0 + i as f64 * 0.016;
        t.fit_at(w(*cols), h, clock);
        t.poll_tx(None, clock, &mut tx);
        assert_eq!(t.size, (*cols, 10), "the grid follows at once");
    }
    assert_eq!(resizes(&rx), vec![], "nothing sent while it moves");
    t.poll_tx(None, 1.064 + RESIZE_DEBOUNCE + 0.001, &mut tx);
    assert_eq!(resizes(&rx), vec![(64, 10)]);
    // The old API sends on the next poll.
    t.fit(w(70), h);
    t.poll(None);
    assert_eq!(resizes(&rx), vec![(70, 10)]);
}

#[test]
fn blink_follows_the_clock_and_rests_after_activity() {
    let (mut t, _ev, _rx) = term();
    let mut tx = Tx::default();
    t.poll_tx(None, 0.0, &mut tx);
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Block));
    t.poll_tx(None, BLINK + 0.01, &mut tx);
    assert_eq!(t.cursor_shape(), None, "blinked off");
    t.poll_tx(None, 2.0 * BLINK + 0.01, &mut tx);
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Block));
    // A key lights it for a full period.
    t.poll_tx(None, 3.0 * BLINK + 0.01, &mut tx);
    assert_eq!(t.cursor_shape(), None);
    t.on_key(&key(Key::Text("a".into()), |_| {}));
    t.poll_tx(None, 3.0 * BLINK + 0.02, &mut tx);
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Block));
    t.cursor_blink = false;
    t.poll_tx(None, 4.0 * BLINK + 0.1, &mut tx);
    assert_eq!(t.cursor_shape(), Some(CursorDraw::Block));
}

#[test]
fn reconnect_takes_the_replay_without_duplicating() {
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, _in_rx) = mpsc::channel();
    // The second connection the daemon gives: its replay first.
    let (ev2_tx, ev2_rx) = mpsc::channel::<TermEvent>();
    let (in2_tx, in2_rx) = mpsc::channel();
    let next = std::sync::Mutex::new(Some(TerminalConn { events: ev2_rx, input: in2_tx }));
    let reopen: Reconnect = Arc::new(move || next.lock().unwrap().take().ok_or_else(|| anyhow::anyhow!("no more")));
    let mut t = TerminalState::with_reconnect(TerminalConn { events: ev_rx, input: in_tx }, reopen);
    t.resize(40, 5);
    let session = lines(30);
    ev_tx.send(TermEvent::Output(session.clone())).unwrap();
    t.poll(None);
    let screen = t.screen_text();
    let history = t.history_len();
    ev_tx.send(TermEvent::Attention).unwrap();
    ev_tx.send(TermEvent::Closed("closed by the daemon".into())).unwrap();
    t.poll(None);
    assert!(t.status.starts_with("disconnected"), "{}", t.status);
    let evs = t.take_events();
    assert!(evs.contains(&TerminalEvent::Attention), "{evs:?}");
    assert!(evs.iter().any(|e| matches!(e, TerminalEvent::Disconnected(_))), "{evs:?}");
    // The daemon replays the same session on the new socket.
    ev2_tx.send(TermEvent::Output(session)).unwrap();
    t.settle_reconnect(Duration::from_secs(5));
    t.poll(None);
    assert_eq!(t.status, "connected");
    assert!(t.take_events().contains(&TerminalEvent::Reconnected));
    assert_eq!(t.screen_text(), screen);
    assert_eq!(t.history_len(), history, "the replay replaced the history, it did not add to it");
    // The new socket was told the size, and keys go to it.
    t.on_key(&key(Key::Text("q".into()), |_| {}));
    let got: Vec<TermInput> = in2_rx.try_iter().collect();
    assert!(matches!(got.first(), Some(TermInput::Resize { cols: 40, rows: 5 })));
    assert!(matches!(got.last(), Some(TermInput::Bytes(b)) if b == b"q"));
}

#[test]
fn without_reconnect_a_close_is_final() {
    let (mut t, ev, _rx) = term();
    ev.send(TermEvent::Closed("gone".into())).unwrap();
    t.poll(None);
    assert_eq!(t.status, "closed: gone");
    drop(ev);
    t.poll(None);
    assert_eq!(t.status, "closed: gone");
}

#[test]
fn program_events_reach_the_shell() {
    let (mut t, _ev, rx) = term();
    t.feed(b"\x1b]2;my title\x07\x07\x1b]52;c;aGVsbG8=\x07");
    let evs = t.take_events();
    assert!(evs.contains(&TerminalEvent::Title("my title".into())), "{evs:?}");
    assert!(evs.contains(&TerminalEvent::Bell), "{evs:?}");
    assert!(evs.contains(&TerminalEvent::Clipboard("hello".into())), "{evs:?}");
    // Colour queries are answered from the theme.
    t.feed(b"\x1b]11;?\x07");
    let reply = String::from_utf8(written(&rx)).unwrap();
    assert!(reply.contains("rgb:0d0d/1111/1717"), "{reply:?}");
}

#[test]
fn ime_commit_types_and_preedit_shows() {
    let (mut t, _ev, rx) = term();
    t.on_ime(&rune::Ime::Preedit("ˆ".into(), None, vec![]));
    assert_eq!(t.preedit.as_deref(), Some("ˆ"));
    assert_eq!(t.cursor_shape(), None, "the composition covers the cursor");
    t.on_ime(&rune::Ime::Commit("ê".into()));
    assert_eq!(t.preedit, None);
    assert_eq!(written(&rx), "ê".as_bytes());
}

/// An open terminal follows the app's theme: its ground, text, ANSI set
/// and pills switch on its next poll, every row drawn again; a theme the
/// caller set stays (term-demo's `--theme`).
#[test]
fn an_open_terminal_follows_the_apps_theme() {
    use crate::theme::{with_mode, Mode};
    let (mut t, _ev, _rx) = term();
    t.feed(b"x");
    assert_eq!(t.theme().background, theme::WEB_BACKGROUND);
    with_mode(Mode::Graphite, || {
        let before = t.rebuilt_rows;
        t.poll(None);
        assert_eq!(*t.theme(), theme::GRAPHITE);
        assert!(t.rebuilt_rows > before, "redrawn");
        assert_eq!(t.pills.colors(crate::emphasis::Tone::Green), crate::emphasis::palette(crate::emphasis::Tone::Green, theme::GRAPHITE_BACKGROUND, theme::GRAPHITE_FOREGROUND));
    });
    t.poll(None);
    assert_eq!(t.theme().background, theme::WEB_BACKGROUND, "back to dark");
    t.set_theme(Theme::from_setting_strings("#fdf6e3", "auto"));
    with_mode(Mode::Graphite, || t.poll(None));
    assert_eq!(t.theme().background, 0xfdf6e3, "the caller's theme stays");
}

#[test]
fn theme_change_redraws_everything() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"x");
    let before = t.rebuilt_rows;
    t.set_theme(Theme::from_setting_strings("#fdf6e3", "auto"));
    t.poll(None);
    assert_eq!(t.rebuilt_rows - before, 5);
}

// ---- Copy and paste through a real Rune runtime (memory clipboard) ----
//
// Key events in the shapes Rune's window host makes of real presses
// (rune src/window/keys.rs, `map_mods`/`map_key`): on macOS ⌘ sets both
// `cmd` and `logo` and the letter arrives as `Key::Command`, lower case;
// off macOS Ctrl sets `cmd` and `ctrl`, and Shift may make the letter upper
// case before Rune lowers it. `Runtime::key` routes them as the host does
// once the physical-key step is past: the focused grid's own `on_key` first.

struct Win {
    t: TerminalState,
    root_paste: bool,
}

fn win_view(s: &Win, _cx: &mut rune::Cx<Win>) -> rune::El {
    let root = rune::col().w_full().h_full();
    let root = if s.root_paste { root.on_paste(|s: &mut Win, text: &str, _tx: &mut rune::Tx| s.t.paste(text)) } else { root };
    root.child(super::view(&s.t, |s: &mut Win| Some(&mut s.t)))
}

fn window(mac: bool, root_paste: bool) -> (rune::Runtime<Win>, rune::clipboard::MemoryClipboard, Sender<TermEvent>, Receiver<TermInput>) {
    let (mut t, ev, rx) = term();
    t.keys.mac = mac;
    let mut rt = rune::Runtime::new(Win { t, root_paste }, win_view, (800.0, 300.0), rune::Env::new(true, 1.0));
    let clip = rune::clipboard::memory();
    rt.use_clipboard(&clip);
    rt.frame(0.0);
    // A click in the grid gives it Rune's focus, as in the window.
    let (x, y) = rt.find("grid").expect("grid drawn");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
    rt.state.t.clear_selection();
    (rt, clip, ev, rx)
}

const MAC_CMD: fn(&mut Mods) = |m| (m.cmd, m.logo) = (true, true);
const PC_CTRL_SHIFT: fn(&mut Mods) = |m| (m.cmd, m.ctrl, m.shift) = (true, true, true);

#[test]
fn mac_cmd_c_copies_the_selection_held_while_the_program_has_the_mouse() {
    let (mut rt, clip, _ev, rx) = window(true, true);
    // A TUI holding the mouse (Claude Code does): a plain drag is the program's.
    rt.state.t.feed(b"echo alpha bravo\r\n\x1b[?1002h\x1b[?1006h");
    let none = Mods::default();
    drag(&mut rt.state.t, Phase::Down, (5, 0), (5, 0), 1, none);
    drag(&mut rt.state.t, Phase::Up, (5, 0), (9, 0), 1, none);
    assert_eq!(rt.state.t.selection_text(), None, "a plain drag is reported, not selected");
    let _ = written(&rx);
    // Shift+drag selects locally even then, as in xterm and iTerm2.
    let shift = Mods { shift: true, ..Mods::default() };
    drag(&mut rt.state.t, Phase::Down, (5, 0), (5, 0), 1, shift);
    drag(&mut rt.state.t, Phase::Move, (5, 0), (9, 0), 1, shift);
    drag(&mut rt.state.t, Phase::Up, (5, 0), (9, 0), 1, shift);
    assert_eq!(rt.state.t.selection_text().as_deref(), Some("alpha"));
    rt.key(key(Key::Command('c'), MAC_CMD));
    rt.frame(0.032);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("alpha"));
    assert_eq!(written(&rx), b"", "⌘C sends nothing to the program");
    // ⌘A selects everything; ⌘C again copies it all.
    rt.key(key(Key::Command('a'), MAC_CMD));
    rt.key(key(Key::Command('c'), MAC_CMD));
    rt.frame(0.048);
    assert!(clip.text().is_some_and(|t| t.starts_with("echo alpha bravo")), "{:?}", clip.text());
}

#[test]
fn mac_paste_through_the_root_reaches_the_terminal_bracketed_or_not() {
    let (mut rt, _clip, _ev, rx) = window(true, true);
    let _ = written(&rx);
    // What ⌘V becomes on macOS: Rune reads the clipboard and calls the root's
    // `on_paste` (rune src/runtime/input/physical.rs, `dispatch_physical`).
    rt.paste("one\ntwo");
    rt.frame(0.032);
    assert_eq!(written(&rx), b"one\rtwo");
    rt.state.t.feed(b"\x1b[?2004h");
    rt.paste("ls\n");
    rt.frame(0.048);
    assert_eq!(written(&rx), b"\x1b[200~ls\r\x1b[201~");
    // ⌘V as a key (Edit menu absent, a remapped equivalent): read, then pasted once.
    let clip_text = "from the clipboard";
    _clip.set_text(clip_text);
    rt.key(key(Key::Command('v'), MAC_CMD));
    rt.frame(0.064);
    rt.frame(0.080);
    assert_eq!(written(&rx), format!("\x1b[200~{clip_text}\x1b[201~").as_bytes());
}

#[test]
fn pc_ctrl_shift_c_and_v_copy_and_paste_while_ctrl_c_and_ctrl_v_stay_the_programs() {
    let (mut rt, clip, _ev, rx) = window(false, false);
    rt.state.t.feed(b"echo alpha bravo\r\n");
    let shift = Mods { shift: true, ..Mods::default() };
    drag(&mut rt.state.t, Phase::Down, (11, 0), (11, 0), 1, shift);
    drag(&mut rt.state.t, Phase::Move, (11, 0), (15, 0), 1, shift);
    drag(&mut rt.state.t, Phase::Up, (11, 0), (15, 0), 1, shift);
    let _ = written(&rx);
    rt.key(key(Key::Command('c'), PC_CTRL_SHIFT));
    rt.frame(0.032);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("bravo"));
    assert_eq!(written(&rx), b"");
    clip.set_text("pasted");
    rt.key(key(Key::Command('v'), PC_CTRL_SHIFT));
    rt.frame(0.048);
    rt.frame(0.064);
    assert_eq!(written(&rx), b"pasted");
    // Plain Ctrl+C is the interrupt, plain Ctrl+V the program's ^V.
    let ctrl = |m: &mut Mods| (m.cmd, m.ctrl) = (true, true);
    rt.key(key(Key::Command('c'), ctrl));
    rt.key(key(Key::Command('v'), ctrl));
    rt.frame(0.080);
    assert_eq!(written(&rx), vec![0x03, 0x16]);
}

#[test]
fn osc52_read_form_is_never_answered() {
    let (mut t, _ev, rx) = term();
    t.feed(b"\x1b]52;c;?\x07\x1b]52;p;?\x1b\\");
    assert_eq!(written(&rx), b"", "nothing goes back down the PTY");
    let evs = t.take_events();
    assert!(!evs.iter().any(|e| matches!(e, TerminalEvent::Clipboard(_))), "{evs:?}");
    // The write form still arrives, decoded (UTF-8 kept whole).
    t.feed(b"\x1b]52;c;w6p0ZQ==\x07");
    assert!(t.take_events().contains(&TerminalEvent::Clipboard("ête".into())));
}

/// Push-to-talk: a key the shell reserves is not sent and is passed on
/// (false); with Ctrl it is the program's again; an empty list changes nothing.
#[test]
fn reserved_keys_stay_with_the_shell() {
    static F8: [Key; 1] = [Key::Function(8)];
    let (mut t, _ev, rx) = term();
    let mut tx = rune::Tx::default();
    fn lens(_: &mut ()) -> Option<&mut TerminalState> {
        None
    }
    assert!(t.on_key_tx(&key(Key::Function(8), |_| {}), &mut tx, lens), "not reserved: sent");
    assert_eq!(written(&rx), b"\x1b[19~");
    t.keys.reserved = &F8;
    assert!(!t.on_key_tx(&key(Key::Function(8), |_| {}), &mut tx, lens), "reserved: passed on");
    assert!(!t.on_key_tx(&key(Key::Function(8), |m| m.shift = true), &mut tx, lens), "Shift is not a modifier that frees it");
    assert_eq!(written(&rx), b"", "nothing reached the program");
    assert!(t.on_key_tx(&key(Key::Function(8), |m| m.ctrl = true), &mut tx, lens));
    assert_eq!(written(&rx), b"\x1b[19;5~", "Ctrl+F8 is the program's");
    assert!(t.on_key_tx(&key(Key::Function(9), |_| {}), &mut tx, lens));
    assert_eq!(written(&rx), b"\x1b[20~", "other keys unchanged");
}

#[test]
fn nothing_is_sent_before_the_pane_has_a_layout() {
    let (ev_tx, ev_rx) = mpsc::channel::<TermEvent>();
    let (in_tx, in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    let mut tx = Tx::default();
    t.poll_tx(None, 0.0, &mut tx);
    t.poll(None);
    assert_eq!(resizes(&in_rx), vec![]);
    assert_eq!(t.wanted_size(), None);
    drop(ev_tx);
}

#[test]
fn a_size_frame_sets_the_grid_and_sends_nothing_back() {
    let (mut t, ev, rx) = term();
    let mut tx = Tx::default();
    let _ = resizes(&rx);
    // Another window owns the PTY at 100x30.
    ev.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    t.poll_tx(None, 1.0, &mut tx);
    assert_eq!(t.size, (100, 30), "the grid is the PTY's size");
    assert_eq!(t.wanted_size(), Some((40, 5)), "what this pane wants is kept");
    assert!(t.sized_elsewhere());
    t.poll_tx(None, 2.0, &mut tx);
    assert_eq!(resizes(&rx), vec![], "a size frame is never answered with a resize");
    // The same layout again (a frame redrawn): still nothing.
    let w = PAD * 2.0 + 40.0 * cell_width() + 0.5;
    let h = PAD * 2.0 + STATUS_H + 5.0 * LINE_HEIGHT + 0.5;
    t.fit_at(w, h, 3.0);
    t.poll_tx(None, 4.0, &mut tx);
    assert_eq!(resizes(&rx), vec![]);
    assert_eq!(t.size, (100, 30));
    // Output drawn for 100 columns lands in 100 columns.
    ev.send(TermEvent::Output(format!("{}|", "x".repeat(99)).into_bytes())).unwrap();
    t.poll(None);
    assert_eq!(t.screen_text().lines().next().unwrap().trim_end(), format!("{}|", "x".repeat(99)));
    // The daemon gives the size back to this pane: the grid follows.
    ev.send(TermEvent::Size { cols: 40, rows: 5, owner: true }).unwrap();
    t.poll(None);
    assert_eq!(t.size, (40, 5));
    assert!(!t.sized_elsewhere());
    assert_eq!(resizes(&rx), vec![]);
}

#[test]
fn typing_or_clicking_takes_the_size_back() {
    let (mut t, ev, rx) = term();
    let _ = resizes(&rx);
    ev.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    t.poll(None);
    // A key is plain input: the daemon counts it as use, no resize needed.
    t.on_key(&key(Key::Text("a".into()), |_| {}));
    let got: Vec<TermInput> = rx.try_iter().collect();
    assert!(matches!(got.as_slice(), [TermInput::Bytes(b)] if b == b"a"));
    // A click sends the wanted size again, which claims it.
    drag(&mut t, Phase::Down, (1, 1), (1, 1), 1, Mods::default());
    assert_eq!(resizes(&rx), vec![(40, 5)]);
    // As owner, a click sends nothing.
    ev.send(TermEvent::Size { cols: 40, rows: 5, owner: true }).unwrap();
    t.poll(None);
    drag(&mut t, Phase::Up, (1, 1), (1, 1), 1, Mods::default());
    drag(&mut t, Phase::Down, (1, 1), (1, 1), 1, Mods::default());
    assert_eq!(resizes(&rx), vec![]);
}

#[test]
fn the_own_window_changing_claims_the_size_at_once() {
    let (mut t, ev, rx) = term();
    let _ = resizes(&rx);
    ev.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    t.poll(None);
    t.resize(50, 8);
    assert_eq!(t.size, (50, 8), "the grid follows this window");
    t.poll(None);
    assert_eq!(resizes(&rx), vec![(50, 8)]);
}

#[test]
fn a_reconnect_sends_the_wanted_size_again_and_forgets_the_old_owner() {
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, _in_rx) = mpsc::channel();
    let (ev2_tx, ev2_rx) = mpsc::channel::<TermEvent>();
    let (in2_tx, in2_rx) = mpsc::channel();
    let next = std::sync::Mutex::new(Some(TerminalConn { events: ev2_rx, input: in2_tx }));
    let reopen: Reconnect = Arc::new(move || next.lock().unwrap().take().ok_or_else(|| anyhow::anyhow!("no more")));
    let mut t = TerminalState::with_reconnect(TerminalConn { events: ev_rx, input: in_tx }, reopen);
    t.resize(60, 12);
    t.poll(None);
    ev_tx.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    ev_tx.send(TermEvent::Closed("daemon restarted".into())).unwrap();
    t.poll(None);
    // The new socket: size frame first (the PTY kept the other size), then the replay.
    ev2_tx.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    t.settle_reconnect(Duration::from_secs(5));
    t.poll(None);
    assert_eq!(resizes(&in2_rx), vec![(60, 12)], "the new socket is told the wanted size");
    assert_eq!(t.size, (100, 30), "until the daemon answers, the grid is the PTY's");
    ev2_tx.send(TermEvent::Size { cols: 60, rows: 12, owner: true }).unwrap();
    t.poll(None);
    assert_eq!(t.size, (60, 12));
    assert!(!t.sized_elsewhere());
}

/// A pane laid out in Rune: the first layout fits the terminal, and a
/// terminal that replaces another in the same pane (same keys, same rect,
/// in the same update) is fitted too (it used to keep the default 100x30).
#[test]
fn a_terminal_is_fitted_at_open_even_when_it_replaces_another() {
    use rune::{Cx, El, Env, Runtime};
    struct S(Option<TerminalState>, Vec<Receiver<TermInput>>, Vec<Sender<TermEvent>>);
    fn lens(s: &mut S) -> Option<&mut TerminalState> {
        s.0.as_mut()
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        rune::row().w_full().h_full().child(super::pane(s.0.as_ref().unwrap(), lens))
    }
    fn open(s: &mut S) {
        let (ev_tx, ev_rx) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel();
        let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
        t.id = "1".into();
        s.0 = Some(t);
        s.1.push(in_rx);
        s.2.push(ev_tx);
    }
    let mut st = S(None, vec![], vec![]);
    open(&mut st);
    let mut rt = Runtime::new(st, view, (900.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.016);
    let want = |w: f32, h: f32| (((w - 2.0 * PAD) / cell_width()).floor() as usize, ((h - 2.0 * PAD - STATUS_H) / LINE_HEIGHT).floor() as usize);
    let first = rt.state.0.as_ref().unwrap().wanted_size();
    assert_eq!(first, Some(want(900.0, 600.0)), "fitted at the first layout");
    rt.update(|s, _| open(s));
    rt.frame(0.032);
    rt.frame(0.048);
    let t = rt.state.0.as_mut().unwrap();
    assert_eq!(t.wanted_size(), first, "the replacing terminal is fitted too");
    assert_eq!(t.size, first.unwrap());
    t.poll(None);
    let f = first.unwrap();
    assert_eq!(resizes(&rt.state.1[1]), vec![(f.0 as u16, f.1 as u16)]);
}

/// Two panes on one sandbox (its main session and a thread): only the pane
/// whose session waits shows "wants you", and it clears when that session
/// works again or the person types there.
#[test]
fn wants_you_follows_the_pane_s_own_session() {
    use serde_json::json;
    let (mut main, _ev1, _rx1) = term();
    let (mut thread, ev2, _rx2) = term();
    let sessions = |main_state: &str, thread_state: &str, ts: u64| {
        json!([
            { "sandbox": "web-app", "mode": "claude", "state": main_state, "ts": ts, "session_id": "a" },
            { "sandbox": "web-app", "mode": "claude", "state": thread_state, "ts": ts, "session_id": "b", "thread": "fix-login" },
            { "sandbox": "docs", "mode": "claude", "state": "attention", "ts": ts, "session_id": "c" },
        ])
    };
    let check = |main: &mut TerminalState, thread: &mut TerminalState, data: &serde_json::Value| {
        let m = session_for(Some(data), Some("web-app"), None, true);
        let t = session_for(Some(data), Some("web-app"), Some("fix-login"), true);
        (main.wants_you(m.as_ref()), thread.wants_you(t.as_ref()))
    };
    let d = sessions("working", "working", 1);
    assert_eq!(check(&mut main, &mut thread, &d), (false, false), "both working: no badge (docs waits, not these)");
    let d = sessions("working", "attention", 2);
    assert_eq!(check(&mut main, &mut thread, &d), (false, true), "only the thread's pane");
    let d = sessions("working", "working", 3);
    assert_eq!(check(&mut main, &mut thread, &d), (false, false), "cleared when it works again");
    // The thread's PTY rings: lit until the session reports working.
    ev2.send(TermEvent::Attention).unwrap();
    thread.poll(None);
    assert_eq!(check(&mut main, &mut thread, &d), (false, true), "a bell, no report since");
    let d = sessions("working", "working", 4);
    assert_eq!(check(&mut main, &mut thread, &d), (false, false), "worked since the bell");
    // Waiting again; the person types in the pane: cleared, until a new report asks.
    let d = sessions("working", "attention", 5);
    assert!(check(&mut main, &mut thread, &d).1);
    thread.on_key(&key(Key::Text("y".into()), |_| {}));
    assert!(!check(&mut main, &mut thread, &d).1, "typed in the pane");
    let d = sessions("working", "attention", 6);
    assert!(check(&mut main, &mut thread, &d).1, "asked again after the key");
    // An exited session is no session: the bell rule.
    let d = sessions("working", "exited", 7);
    assert!(session_for(Some(&d), Some("web-app"), Some("fix-login"), true).is_none());
}

#[test]
fn a_plain_shell_keeps_the_bell_rule() {
    let (mut t, ev, _rx) = term();
    let sessions = serde_json::json!([{ "sandbox": "web-app", "mode": "claude", "state": "attention", "ts": 1 }]);
    // A bash pane is never matched to the agent's session.
    let s = session_for(Some(&sessions), Some("web-app"), None, false);
    assert!(s.is_none());
    assert!(!t.wants_you(None));
    ev.send(TermEvent::Attention).unwrap();
    t.poll(None);
    assert!(t.wants_you(None));
    t.on_key(&key(Key::Text("x".into()), |_| {}));
    assert!(!t.wants_you(None));
    // No sessions data at all (the feature not loaded): the bell rule too.
    assert!(session_for(None, Some("web-app"), None, true).is_none());
}

// ---- A grid sized by another window, drawn scaled to fit ----

/// The pane's whole size (points) for a room of exactly `cols` x `rows` cells.
fn pane_for(cols: usize, rows: usize) -> (f32, f32) {
    (PAD * 2.0 + cols as f32 * cell_width() + 0.5, PAD * 2.0 + STATUS_H + rows as f32 * LINE_HEIGHT + 0.5)
}

/// A terminal whose pane has room for `cols` x `rows`, its resize sent.
fn client(cols: usize, rows: usize) -> (TerminalState, Sender<TermEvent>, Receiver<TermInput>) {
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    let (w, h) = pane_for(cols, rows);
    t.fit(w, h);
    t.poll(None);
    assert_eq!(t.wanted_size(), Some((cols, rows)));
    (t, ev_tx, in_rx)
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// The scaled grid: no larger than the ideal scale, within two font steps
/// of it, and its drawn cells fit the room.
fn check_fits(t: &TerminalState, ideal: f32, room: (usize, usize)) {
    let (k, cut) = t.display_scale();
    let m = t.metrics();
    let (rw, rh) = (room.0 as f32 * cell_width() + 0.5, room.1 as f32 * LINE_HEIGHT + 0.5);
    assert!(!cut, "{k}");
    assert!(k <= ideal + 1e-4 && k > ideal - 2.0 * 0.25 / FONT_SIZE - 1e-4, "k {k}, ideal {ideal}");
    assert!(t.size.0 as f32 * m.cell_w <= rw && t.size.1 as f32 * m.line_h <= rh, "{m:?} for {:?} in {rw}x{rh}", t.size);
    assert_eq!(m.font, k * FONT_SIZE);
    assert!(approx(m.line_h, LINE_HEIGHT * k));
}

#[test]
fn a_larger_grid_from_another_window_is_scaled_to_fit_both_ways() {
    let (mut t, ev, rx) = client(40, 10);
    let _ = resizes(&rx);
    let other = |t: &mut TerminalState, cols: u16, rows: u16| {
        ev.send(TermEvent::Size { cols, rows, owner: false }).unwrap();
        t.poll(None);
        t.display_scale()
    };
    // Room 40x10 (plus half a point). Too wide: the width decides.
    other(&mut t, 64, 10);
    check_fits(&t, 40.0 / 64.0, (40, 10));
    // Too tall: the height decides.
    other(&mut t, 40, 16);
    check_fits(&t, 10.0 / 16.0, (40, 10));
    // Both: the smaller of the two.
    other(&mut t, 50, 16);
    check_fits(&t, 10.0 / 16.0, (40, 10));
    // Smaller than the room: never above 1, top-left as before.
    assert_eq!(other(&mut t, 20, 5), (1.0, false));
    assert_eq!(t.metrics(), Metrics::normal());
    // Larger than twice the room: the floor (7 pt), and cut.
    assert_eq!(other(&mut t, 200, 60), (MIN_SCALE, true));
    assert_eq!(t.metrics().font, 7.0);
    // Owning the size again: normal size.
    ev.send(TermEvent::Size { cols: 40, rows: 10, owner: true }).unwrap();
    t.poll(None);
    assert_eq!(t.display_scale(), (1.0, false));
    // Another window at exactly this size, not owned: normal too.
    other(&mut t, 40, 10);
    assert_eq!(t.display_scale(), (1.0, false));
    // The scale is drawing only: nothing was ever sent because of it.
    assert_eq!(resizes(&rx), vec![]);
    assert_eq!(t.wanted_size(), Some((40, 10)));
}

/// Under scale, a point maps to the cell drawn under it: the selection takes
/// the words drawn where the drag ran.
#[test]
fn under_scale_a_selection_takes_the_cells_drawn_under_the_pointer() {
    let (mut t, ev, _rx) = client(40, 10);
    ev.send(TermEvent::Size { cols: 80, rows: 20, owner: false }).unwrap();
    ev.send(TermEvent::Output(b"\r\n\r\n                                              alpha bravo charlie".to_vec())).unwrap();
    t.poll(None);
    let m = t.metrics();
    assert!(m.font < FONT_SIZE);
    // "bravo" starts at column 52 of row 2, drawn at 52 * the scaled cell.
    let at = |c: usize, r: usize, f: f32| ((c as f32 + f) * m.cell_w, (r as f32 + 0.5) * m.line_h);
    let ((sx, sy), (x, y)) = (at(52, 2, 0.25), at(56, 2, 0.75));
    let shift = Mods { shift: true, ..Mods::default() };
    for (phase, x, y) in [(Phase::Down, sx, sy), (Phase::Move, x, y), (Phase::Up, x, y)] {
        t.drag(DragInput { phase, x, y, start_x: sx, start_y: sy, dx: x - sx, dy: y - sy, clicks: 1 }, PointerButton::Primary, shift);
    }
    assert_eq!(t.selection_text().as_deref(), Some("bravo"));
    // The same points at full-size cells would have been columns 26 to 29: blanks.
}

#[test]
fn no_resize_is_sent_while_scaled() {
    let (mut t, ev, rx) = client(40, 10);
    let _ = resizes(&rx);
    let mut tx = Tx::default();
    ev.send(TermEvent::Size { cols: 70, rows: 20, owner: false }).unwrap();
    t.poll_tx(None, 1.0, &mut tx);
    assert!(t.display_scale().0 < 1.0);
    // Frames redrawn, the same layout heard again, output, time passing.
    let (w, h) = pane_for(40, 10);
    for i in 0..5 {
        t.fit_at(w, h, 2.0 + i as f64);
        ev.send(TermEvent::Output(b"tick\r\n".to_vec())).unwrap();
        t.poll_tx(None, 2.5 + i as f64, &mut tx);
    }
    assert_eq!(resizes(&rx), vec![], "a scaled pane never sends its size");
    assert_eq!(t.size, (70, 20), "the grid stays the PTY's");
    // A wheel and a selection under scale send no size either.
    t.on_wheel(LINE_HEIGHT * 2.0, 10.0, 10.0, Mods::default());
    let shift = Mods { shift: true, ..Mods::default() };
    drag(&mut t, Phase::Down, (1, 1), (1, 1), 1, shift);
    assert_eq!(resizes(&rx), vec![(40, 10)], "only the click, which takes the size back");
}

/// A pane in Rune with the grid scaled: the drawn grid fits the room, lands
/// at once (no spring), and a click lands on the cell drawn under it.
#[test]
fn under_scale_the_grid_fits_and_the_mouse_maps_to_the_cell_under_it() {
    use rune::{Cx, El, Env, Runtime};
    struct S(TerminalState);
    fn lens(s: &mut S) -> Option<&mut TerminalState> {
        Some(&mut s.0)
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        rune::row().w_full().h_full().child(super::pane(&s.0, lens))
    }
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    t.id = "1".into();
    let mut rt = Runtime::new(S(t), view, (700.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.016);
    let (wc, wr) = rt.state.0.wanted_size().unwrap();
    // Another window owns the PTY at 1.6 times this pane's cells.
    let (pc, pr) = ((wc as f32 * 1.6) as u16, (wr as f32 * 1.6) as u16);
    ev_tx.send(TermEvent::Size { cols: pc, rows: pr, owner: false }).unwrap();
    // A program that wants the mouse (SGR reports).
    ev_tx.send(TermEvent::Output(b"\x1b[?1000h\x1b[?1006h".to_vec())).unwrap();
    rt.update(|s, _| {
        s.0.poll(None);
    });
    rt.frame(0.032);
    rt.frame(0.048);
    let _ = resizes(&in_rx);
    let (k, cut) = rt.state.0.display_scale();
    assert!(k < 1.0 && k >= MIN_SCALE && !cut, "{k}");
    let m = rt.state.0.metrics();
    let (grid, _) = rt.presented("grid-1").expect("grid drawn");
    let fit_key = rt.state.0.fit_key();
    let (room, _) = rt.presented(&fit_key).expect("room drawn");
    assert!(approx(grid[2], pc as f32 * m.cell_w) && approx(grid[3], pr as f32 * m.line_h), "{grid:?}: {m:?}, at once (no spring)");
    assert!(grid[0] + grid[2] <= room[0] + room[2] + 0.5 && grid[1] + grid[3] <= room[1] + room[3] + 0.5, "nothing cut: {grid:?} in {room:?}");
    assert!(approx(grid[0], room[0]) && approx(grid[1], room[1]), "top-left");
    // A click on the cell drawn at (col 30, row 12): reported as that cell.
    let (c, r) = (30usize, 12usize);
    let x = grid[0] + (c as f32 + 0.5) * m.cell_w;
    let y = grid[1] + (r as f32 + 0.5) * m.line_h;
    rt.pointer_down(x, y);
    rt.frame(0.064);
    rt.pointer_up(x, y);
    rt.frame(0.080);
    let all: Vec<TermInput> = in_rx.try_iter().collect();
    let bytes: Vec<u8> = all.iter().filter_map(|i| if let TermInput::Bytes(b) = i { Some(b.clone()) } else { None }).flatten().collect();
    let want = format!("\x1b[<0;{};{}M", c + 1, r + 1);
    assert!(String::from_utf8_lossy(&bytes).contains(&want), "{:?} lacks {want:?}", String::from_utf8_lossy(&bytes));
    // The click took the size back: this pane's size went once.
    let sent: Vec<(u16, u16)> = all.iter().filter_map(|i| if let TermInput::Resize { cols, rows } = i { Some((*cols, *rows)) } else { None }).collect();
    assert_eq!(sent, vec![(wc as u16, wr as u16)]);
    // The daemon answers: owner, normal size, drawn unscaled.
    ev_tx.send(TermEvent::Size { cols: wc as u16, rows: wr as u16, owner: true }).unwrap();
    rt.update(|s, _| {
        s.0.poll(None);
    });
    rt.frame(0.096);
    rt.frame(0.112);
    assert_eq!(rt.state.0.display_scale(), (1.0, false));
    let (grid, _) = rt.presented("grid-1").unwrap();
    assert!(approx(grid[2], wc as f32 * cell_width()), "{grid:?}");
}

/// Two clients of different sizes on one PTY, with the daemon's rule
/// (`crates/daemon/src/pty.rs`: the client that last sent a size or typed
/// owns it; every client hears the size and whether it is the owner). Each
/// change of hands is one message from the client taking it and one frame
/// to each: then nothing moves, and the client that does not own it is
/// scaled, never answering.
#[test]
fn two_clients_of_different_sizes_settle_after_one_exchange() {
    struct Daemon {
        pty: (u16, u16),
        owner: Option<usize>,
        wanted: [Option<(u16, u16)>; 2],
    }
    let (a, a_ev, a_rx) = client(60, 15);
    let (b, b_ev, b_rx) = client(80, 20);
    let mut ts = [a, b];
    let evs = [a_ev, b_ev];
    let rxs = [a_rx, b_rx];
    let mut d = Daemon { pty: (0, 0), owner: None, wanted: [None, None] };
    // Delivers what the clients sent until nothing moves; returns the messages carried.
    let mut pump = |ts: &mut [TerminalState; 2], d: &mut Daemon| -> usize {
        let mut carried = 0;
        for _round in 0..10 {
            let mut moved = false;
            for i in 0..2 {
                for input in rxs[i].try_iter() {
                    carried += 1;
                    moved = true;
                    match input {
                        TermInput::Resize { cols, rows } => {
                            d.wanted[i] = Some((cols, rows));
                            d.owner = Some(i);
                        }
                        TermInput::Bytes(_) if d.wanted[i].is_some() => d.owner = Some(i),
                        _ => {}
                    }
                }
            }
            if !moved {
                return carried;
            }
            let o = d.owner.unwrap();
            d.pty = d.wanted[o].unwrap();
            for (j, ev) in evs.iter().enumerate() {
                ev.send(TermEvent::Size { cols: d.pty.0, rows: d.pty.1, owner: j == o }).unwrap();
            }
            for t in ts.iter_mut() {
                t.poll(None);
            }
        }
        panic!("never settled");
    };
    // Both attach; B came last and owns 80x20. A draws it at 3/4.
    assert_eq!(pump(&mut ts, &mut d), 2);
    assert_eq!(d.pty, (80, 20));
    assert!(approx(ts[0].display_scale().0, 0.75), "{:?}", ts[0].display_scale());
    assert_eq!(ts[1].display_scale(), (1.0, false));
    // A click in A: one resize, then quiet. B's grid (60x15) is smaller than its pane: normal size.
    drag(&mut ts[0], Phase::Down, (1, 1), (1, 1), 1, Mods::default());
    drag(&mut ts[0], Phase::Up, (1, 1), (1, 1), 1, Mods::default());
    assert_eq!(pump(&mut ts, &mut d), 1, "one message, the claim");
    assert_eq!(d.pty, (60, 15));
    assert_eq!((ts[0].display_scale(), ts[1].display_scale()), ((1.0, false), (1.0, false)));
    assert!(ts[1].sized_elsewhere() && !ts[0].sized_elsewhere());
    // A key in B: one message (the key), then quiet; A scales again.
    ts[1].on_key(&key(Key::Text("x".into()), |_| {}));
    assert_eq!(pump(&mut ts, &mut d), 1, "one message, the key");
    assert_eq!(d.pty, (80, 20));
    assert!(approx(ts[0].display_scale().0, 0.75));
    // Quiet afterwards, frames and polls included.
    for t in ts.iter_mut() {
        t.poll(None);
    }
    assert_eq!(pump(&mut ts, &mut d), 0, "settled");
}

// Keyword pills (terminal/pills.rs, emphasis.rs).

fn pilled() -> (TerminalState, Sender<TermEvent>, Receiver<TermInput>) {
    let (mut t, ev, rx) = term();
    t.set_pills(Some(Rc::new(crate::emphasis::Vocab::default())));
    t.poll(None);
    (t, ev, rx)
}

/// (start, end, tone) of each piece on row `r`.
fn pieces(t: &TerminalState, r: usize) -> Vec<(u16, u16, crate::emphasis::Tone)> {
    t.row_pills(r).iter().map(|p| (p.start, p.end, p.tone)).collect()
}

/// The columns of row `r` drawn over a pill background, as a string of `#` and `.`.
/// The columns each drawn pill covers on row `r` (views2: a capsule over
/// its cells' fill, `render::pill_shape`), as `#`.
fn pill_cols(t: &TerminalState, r: usize) -> String {
    let (text, _, _) = &t.test_parts[r];
    let mut cols = vec!['.'; text.chars().count()];
    for (a, b, _, _) in t.test_pills.get(r).cloned().unwrap_or_default() {
        cols[a as usize..b as usize].iter_mut().for_each(|c| *c = '#');
    }
    cols.into_iter().collect::<String>().trim_end_matches('.').to_string()
}

#[test]
fn a_marker_is_drawn_as_a_pill_over_exactly_its_cells() {
    use crate::emphasis::Tone;
    let (mut t, _ev, _rx) = pilled();
    t.feed(b"Tests [[Done]] and [[Failed: 3 tests]]\r\n");
    assert_eq!(pieces(&t, 0), vec![(6, 14, Tone::Green), (19, 38, Tone::Red)]);
    assert_eq!(pill_cols(&t, 0), "......########.....###################");
    // The brackets are padding; the cells' text is untouched.
    let (text, spans, _) = &t.test_parts[0];
    assert_eq!(text.trim_end(), "Tests   Done   and   Failed: 3 tests");
    assert_eq!(text.chars().count(), 40, "one character per cell: nothing shifts");
    assert_eq!(t.screen_text().lines().next(), Some("Tests [[Done]] and [[Failed: 3 tests]]"));
    // Regular weight (views2): the word in the body text tinted toward the
    // tone, the detail in the body text; the fill is the cells' (under the
    // text), its round ends the row's shape: no span background.
    let word = spans.iter().find(|s| text[s.start as usize..s.end as usize].contains("Failed")).unwrap();
    let detail = spans.iter().find(|s| text[s.start as usize..s.end as usize].contains("3 tests")).unwrap();
    assert_eq!((word.weight, detail.weight), (None, None));
    assert_ne!(word.color, detail.color);
    assert!(spans.iter().all(|s| s.bg.is_none()));
    let red = t.pills.colors(Tone::Red).bg;
    assert!(t.test_parts[0].2.iter().any(|f| (f.0, f.1, f.2) == (19, 38, red)), "the pill's fill over exactly its cells");
    assert_eq!(t.test_pills[0], vec![(6, 14, true, true), (19, 38, true, true)], "both ends round");
}

#[test]
fn the_programs_colours_are_replaced_inside_the_pill_only() {
    let (mut t, _ev, _rx) = pilled();
    // Bold yellow on blue over the marker and beyond it.
    t.feed(b"\x1b[1;33;44m[[Done]] x\x1b[0m\r\n");
    let (_, _, fills) = &t.test_parts[0];
    // The program's blue background is kept outside the pill; the pill's
    // fill is the tone over that blue (it blends into the coloured line).
    assert_eq!(fills.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>(), vec![(0, 8), (8, 10)]);
    assert_ne!(fills[0].2, t.pills.colors(crate::emphasis::Tone::Green).bg, "tinted over the blue, not the terminal's ground");
    let (text, spans, _) = &t.test_parts[0];
    let x = spans.iter().find(|s| text[s.start as usize..s.end as usize].contains('x')).unwrap();
    assert!(x.bg.is_none() && x.color != spans[0].color);
}

#[test]
fn copy_selection_and_input_are_unchanged() {
    let (mut t, _ev, rx) = pilled();
    t.feed(b"say [[Needs you]] now\r\n");
    written(&rx);
    let (x, y) = at(0, 0);
    t.select_start(x, y, 3);
    assert_eq!(t.selection_text().as_deref(), Some("say [[Needs you]] now"));
    // The pill yields to the selection: drawn plain under the highlight.
    t.poll(None);
    assert_eq!(pill_cols(&t, 0), "");
    assert_eq!(t.row_pills(0).len(), 1, "still matched; only its drawing yields");
    t.clear_selection();
    t.poll(None);
    assert_eq!(pill_cols(&t, 0), "....#############");
    // A selection elsewhere on the row leaves the pill drawn.
    let (x, y) = at(18, 0);
    t.select_start(x, y, 1);
    let (x, y) = at_end(19, 0);
    t.select_update(x, y);
    t.poll(None);
    assert_eq!(t.selection_text().as_deref(), Some("no"));
    assert_eq!(pill_cols(&t, 0), "....#############");
    assert!(written(&rx).is_empty(), "nothing went to the program");
}

#[test]
fn not_while_the_cursor_is_on_the_line() {
    let (mut t, _ev, _rx) = pilled();
    t.feed(b"typing [[Do");
    assert!(pieces(&t, 0).is_empty());
    t.feed(b"ne]]");
    assert!(pieces(&t, 0).is_empty(), "complete, but the cursor is still on its line");
    t.feed(b"\r\n");
    assert_eq!(pieces(&t, 0).len(), 1, "drawn once the cursor left");
    // The cursor going back up to the line hides it again (a program editing it).
    t.feed(b"\x1b[1A");
    assert!(pieces(&t, 0).is_empty());
    // A hidden cursor (a full-screen program) does not hold pills back.
    t.feed(b"\x1b[?25l");
    assert_eq!(pieces(&t, 0).len(), 1);
}

#[test]
fn wrap_splits_a_pill_and_a_hard_break_does_not_match() {
    let (mut t, _ev, _rx) = pilled();
    // 40 columns: the marker starts at 35 and wraps.
    t.feed(format!("{}[[Done: wrap]]\r\n", "x".repeat(35)).as_bytes());
    let row0 = t.row_pills(0)[0];
    let row1 = t.row_pills(1)[0];
    assert_eq!((row0.start, row0.end, row0.cut_left, row0.cut_right), (35, 40, false, true));
    assert_eq!((row1.start, row1.end, row1.cut_left, row1.cut_right), (0, 9, true, false));
    assert_eq!(t.test_pills[0], vec![(35, 40, true, false)], "square end at the cut");
    assert_eq!(t.test_pills[1], vec![(0, 9, false, true)]);
    // A hard line break is the end of the line.
    t.feed(b"[[Do\r\nne]]\r\n");
    assert!(pieces(&t, 2).is_empty() && pieces(&t, 3).is_empty());
}

#[test]
fn wide_characters_before_and_inside() {
    use crate::emphasis::Tone;
    let (mut t, _ev, _rx) = pilled();
    t.feed("日本 [[Done]] [[Done: 完了]]\r\n".as_bytes());
    assert_eq!(pieces(&t, 0), vec![(5, 13, Tone::Green), (14, 28, Tone::Green)]);
    assert_eq!(pill_cols(&t, 0), ".....########.##############");
}

#[test]
fn on_the_alternate_screen_and_scrolled_back() {
    let (mut t, _ev, _rx) = pilled();
    t.feed(b"\x1b[?1049h\x1b[H[[Note]] full screen\x1b[3;1H");
    assert_eq!(pieces(&t, 0).len(), 1);
    t.feed(b"\x1b[?1049l");
    t.feed(b"[[Done]]\r\n");
    t.feed(&lines(10));
    t.scroll_lines(8);
    t.poll(None);
    let r = t.visible_text().lines().position(|l| l.contains("[[Done]]")).expect("in view");
    assert_eq!(pieces(&t, r).len(), 1);
}

#[test]
fn pills_follow_the_switch_live() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"[[Done]]\r\n");
    assert!(pieces(&t, 0).is_empty(), "off by default");
    t.set_pills(Some(Rc::new(crate::emphasis::Vocab::default())));
    t.poll(None);
    assert_eq!(pill_cols(&t, 0), "########");
    t.set_pills(None);
    t.poll(None);
    assert!(pieces(&t, 0).is_empty());
    assert_eq!(pill_cols(&t, 0), "");
}

#[test]
fn unchanged_rows_are_not_matched_again() {
    let (mut t, _ev, _rx) = pilled();
    t.resize(80, 24);
    t.feed(b"[[Done]] one\r\nplain\r\n[[Failed]] two\r\n");
    let (scanned, matched) = (t.pills.scanned, t.pills.matched);
    // Frames with nothing new: no line is looked at again.
    t.poll(None);
    t.poll(None);
    assert_eq!((t.pills.scanned, t.pills.matched), (scanned, matched));
    // Output on one row: that row's line (and the cursor's) only.
    t.feed(b"x");
    assert!(t.pills.scanned - scanned <= 2, "{}", t.pills.scanned - scanned);
    assert_eq!(t.pills.matched, matched, "no [[ on it: the pre-check stops there");
    assert_eq!(pieces(&t, 0).len(), 1);
}

// ---- Redraw: the nudge that makes a program draw its whole screen ----

/// Polls with the app clock from `from` to `to` in 30 ms frames.
fn polls(t: &mut TerminalState, from: f64, to: f64) {
    let mut tx = Tx::default();
    let mut c = from;
    while c <= to {
        t.poll_tx(None, c, &mut tx);
        c += 0.03;
    }
}

#[test]
fn a_new_socket_nudges_once_when_this_client_owns_the_size() {
    let (mut t, ev, rx) = client(40, 5);
    let _ = resizes(&rx);
    // The daemon: this client owns the PTY at its size; the replay follows.
    ev.send(TermEvent::Size { cols: 40, rows: 5, owner: true }).unwrap();
    ev.send(TermEvent::Output(b"stale diffs".to_vec())).unwrap();
    polls(&mut t, 0.0, 0.1);
    assert_eq!(resizes(&rx), vec![], "not before the replay had its moment");
    polls(&mut t, 0.13, 1.0);
    assert_eq!(resizes(&rx), vec![(39, 5), (40, 5)], "one column narrower, then the size back");
    polls(&mut t, 1.03, 3.0);
    assert_eq!(resizes(&rx), vec![], "once a socket");
}

#[test]
fn no_nudge_while_another_window_owns_the_size() {
    let (mut t, ev, rx) = client(40, 5);
    let _ = resizes(&rx);
    ev.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    polls(&mut t, 0.0, 6.0);
    assert_eq!(resizes(&rx), vec![], "an automatic nudge never takes the size");
    assert!(t.sized_elsewhere());
    // Then this client owns it again: the socket's chance has passed.
    ev.send(TermEvent::Size { cols: 40, rows: 5, owner: true }).unwrap();
    polls(&mut t, 6.03, 8.0);
    assert_eq!(resizes(&rx), vec![]);
    // The person's Redraw, while another window owns it: takes it back, once.
    ev.send(TermEvent::Size { cols: 100, rows: 30, owner: false }).unwrap();
    t.poll(None);
    assert!(t.redraw());
    assert_eq!(resizes(&rx), vec![(40, 5)]);
}

#[test]
fn redraw_sends_two_sizes_in_order_once() {
    let (mut t, ev, rx) = client(40, 5);
    ev.send(TermEvent::Size { cols: 40, rows: 5, owner: true }).unwrap();
    polls(&mut t, 0.0, 1.0);
    let _ = resizes(&rx);
    assert!(t.redraw());
    polls(&mut t, 1.03, 2.0);
    assert_eq!(resizes(&rx), vec![(39, 5), (40, 5)]);
    assert_eq!(t.wanted_size(), Some((40, 5)), "the pane keeps its size");
    polls(&mut t, 2.03, 4.0);
    assert_eq!(resizes(&rx), vec![]);
}

#[test]
fn reconnects_nudge_once_per_socket() {
    // Three sockets in a row (a daemon restarting): one nudge each, none in between.
    let conns: Vec<(Sender<TermEvent>, Receiver<TermInput>, TerminalConn)> = (0..3)
        .map(|_| {
            let (ev_tx, ev_rx) = mpsc::channel();
            let (in_tx, in_rx) = mpsc::channel();
            (ev_tx, in_rx, TerminalConn { events: ev_rx, input: in_tx })
        })
        .collect();
    let mut evs = vec![];
    let mut ins = vec![];
    let mut next = vec![];
    for (e, i, c) in conns {
        evs.push(e);
        ins.push(i);
        next.push(c);
    }
    let first = next.remove(0);
    let queue = std::sync::Mutex::new(next);
    let reopen: Reconnect = Arc::new(move || {
        let mut q = queue.lock().unwrap();
        if q.is_empty() { Err(anyhow::anyhow!("no more")) } else { Ok(q.remove(0)) }
    });
    let mut t = TerminalState::with_reconnect(first, reopen);
    let (w, h) = pane_for(40, 5);
    t.fit(w, h);
    let mut clock = 0.0;
    for k in 0..3 {
        evs[k].send(TermEvent::Size { cols: 40, rows: 5, owner: true }).unwrap();
        polls(&mut t, clock, clock + 1.0);
        clock += 1.03;
        let sizes = resizes(&ins[k]);
        assert_eq!(sizes.iter().filter(|s| **s == (39, 5)).count(), 1, "socket {k}: one nudge: {sizes:?}");
        if k < 2 {
            evs[k].send(TermEvent::Closed("daemon restarted".into())).unwrap();
            t.poll(None);
            t.settle_reconnect(Duration::from_secs(5));
            t.poll(None);
        }
    }
}

// ---- file paths in terminal text (terminal/filelink.rs) ----

fn cmd() -> Mods {
    Mods { cmd: true, ..Mods::default() }
}

fn click(t: &mut TerminalState, cell: (usize, usize), mods: Mods) -> Option<String> {
    drag(t, Phase::Down, cell, cell, 1, mods);
    drag(t, Phase::Up, cell, cell, 1, mods)
}

#[test]
fn a_file_path_is_underlined_and_clickable_only_with_the_modifier() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"see src/main.rs:42 ok");
    let (x, y) = at(6, 0);
    // No modifier: nothing is detected, no work done, plain clicks do as before.
    t.on_pointer_move(x, y, Mods::default());
    assert_eq!((t.file_hover_path(), t.file_probes), (None, 0));
    assert_eq!(click(&mut t, (6, 0), Mods::default()), None);
    assert_eq!((t.take_file_click(), t.file_probes), (None, 0));
    // With it: the path (not the :line) is hovered, on this row only.
    t.on_pointer_move(x, y, cmd());
    assert_eq!(t.file_hover_path(), Some("src/main.rs"));
    assert_eq!(t.file_probes, 1);
    assert_eq!(t.file_hover.as_ref().unwrap().rows, vec![(0, 4, 18)]);
    let (x2, y2) = at(1, 0);
    t.on_pointer_move(x2, y2, cmd());
    assert_eq!(t.file_hover_path(), None);
    // Back to the path, then the modifier released at the next move: gone.
    t.on_pointer_move(x, y, cmd());
    t.on_pointer_move(x, y, Mods::default());
    assert_eq!(t.file_hover_path(), None);
    assert_eq!(t.file_probes, 3);
}

#[test]
fn a_modifier_click_hands_the_path_on_once() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"see src/main.rs:42 ok");
    assert_eq!(click(&mut t, (6, 0), cmd()), None, "no URL: a path is not opened as one");
    assert_eq!(t.take_file_click().as_deref(), Some("src/main.rs"));
    assert_eq!(t.take_file_click(), None, "once");
    // On other text, nothing; a drag is a selection, not a click.
    click(&mut t, (1, 0), cmd());
    assert_eq!(t.take_file_click(), None);
    drag(&mut t, Phase::Down, (4, 0), (4, 0), 1, cmd());
    drag(&mut t, Phase::Move, (4, 0), (12, 0), 1, cmd());
    drag(&mut t, Phase::Up, (4, 0), (12, 0), 1, cmd());
    assert_eq!(t.take_file_click(), None);
}

#[test]
fn an_osc8_link_keeps_priority_over_a_path_in_it() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"\x1b]8;;https://example.org/x\x07a/b.md\x1b]8;;\x07 \x1b]8;;file:///etc/passwd\x07c/d.md\x1b]8;;\x07 e/f.md");
    assert_eq!(click(&mut t, (2, 0), cmd()).as_deref(), Some("https://example.org/x"));
    assert_eq!(t.take_file_click(), None);
    // A link that is not a web link is inert, and its text is not a file link either.
    assert_eq!(click(&mut t, (9, 0), cmd()), None);
    assert_eq!(t.take_file_click(), None);
    t.on_pointer_move(at(9, 0).0, at(9, 0).1, cmd());
    assert_eq!(t.file_hover_path(), None);
    // The plain path after them still links.
    assert_eq!(click(&mut t, (15, 0), cmd()), None);
    assert_eq!(t.take_file_click().as_deref(), Some("e/f.md"));
}

#[test]
fn with_mouse_reports_on_only_modifier_shift_click_opens_a_path() {
    let (mut t, _ev, rx) = term();
    t.feed(b"\x1b[?1000h\x1b[?1006hsee src/main.rs ok");
    let shift_cmd = Mods { shift: true, ..cmd() };
    // Cmd + click goes to the program, as today (and never to Files).
    assert_eq!(click(&mut t, (6, 0), cmd()), None);
    assert_eq!(t.take_file_click(), None);
    assert!(!written(&rx).is_empty(), "the program heard the press and release");
    // Cmd + Shift + click is the selection gesture: it opens the path (and an OSC 8 link).
    assert_eq!(click(&mut t, (6, 0), shift_cmd), None);
    assert_eq!(t.take_file_click().as_deref(), Some("src/main.rs"));
    assert!(written(&rx).is_empty(), "the program heard nothing");
    t.feed(b"\r\n\x1b]8;;https://example.org/x\x07link\x1b]8;;\x07");
    assert_eq!(click(&mut t, (1, 1), shift_cmd).as_deref(), Some("https://example.org/x"));
}

#[test]
fn a_path_wrapped_by_the_terminal_is_one_link_a_hard_break_is_not() {
    let (mut t, _ev, _rx) = term();
    // 40 columns: the path starts at column 30 and wraps into the next row.
    t.feed(format!("{}docs/guides/long-name.md end", " ".repeat(30)).as_bytes());
    for cell in [(32, 0), (3, 1)] {
        t.on_pointer_move(at(cell.0, cell.1).0, at(cell.0, cell.1).1, cmd());
        assert_eq!(t.file_hover_path(), Some("docs/guides/long-name.md"), "{cell:?}");
        assert_eq!(t.file_hover.as_ref().unwrap().rows, vec![(0, 30, 40), (1, 0, 14)]);
    }
    assert_eq!(click(&mut t, (3, 1), cmd()), None);
    assert_eq!(t.take_file_click().as_deref(), Some("docs/guides/long-name.md"));
    // A line break the program wrote is a break: two words, no path.
    let (mut t, _ev, _rx) = term();
    t.feed(format!("{}docs/guides/\r\nlong-name.md", " ".repeat(30)).as_bytes());
    t.on_pointer_move(at(32, 0).0, at(32, 0).1, cmd());
    assert_eq!(t.file_hover_path(), None);
}

#[test]
fn the_screenshots_folder_path_links_without_its_comma_and_prose_does_not() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"In /tmp/sbxw-chat/test/graph-lookup/, run cp .env.example .env");
    t.on_pointer_move(at(10, 0).0, at(10, 0).1, cmd());
    assert_eq!(t.file_hover_path(), Some("/tmp/sbxw-chat/test/graph-lookup/"));
    assert_eq!(t.file_hover.as_ref().unwrap().rows, vec![(0, 3, 36)]);
    assert_eq!(click(&mut t, (10, 0), cmd()), None, "no URL: a folder is not opened as one");
    assert_eq!(t.take_file_click().as_deref(), Some("/tmp/sbxw-chat/test/graph-lookup/"));
    // Its comma, `cp` and the dotfiles are not links.
    for col in [36, 39] {
        click(&mut t, (col, 0), cmd());
        assert_eq!(t.take_file_click(), None, "{col}");
    }
    let (mut t, _ev, _rx) = term();
    t.feed(b"and/or TCP/IP ~/dev/sbxw Rust/Luau");
    for (col, want) in [(1, None), (9, None), (17, Some("~/dev/sbxw")), (27, None)] {
        click(&mut t, (col, 0), cmd());
        assert_eq!(t.take_file_click().as_deref(), want, "{col}");
    }
}

#[test]
fn a_wrapped_folder_path_is_one_link() {
    let (mut t, _ev, _rx) = term();
    t.feed(format!("{}/tmp/sbxw-chat/test/graph-lookup/ end", " ".repeat(30)).as_bytes());
    for cell in [(31, 0), (20, 1)] {
        t.on_pointer_move(at(cell.0, cell.1).0, at(cell.0, cell.1).1, cmd());
        assert_eq!(t.file_hover_path(), Some("/tmp/sbxw-chat/test/graph-lookup/"), "{cell:?}");
        assert_eq!(t.file_hover.as_ref().unwrap().rows, vec![(0, 30, 40), (1, 0, 23)]);
    }
    click(&mut t, (5, 1), cmd());
    assert_eq!(t.take_file_click().as_deref(), Some("/tmp/sbxw-chat/test/graph-lookup/"));
}

#[test]
fn hostile_output_yields_no_link_or_one_inert_one() {
    let (mut t, _ev, _rx) = term();
    let mut hostile: Vec<Vec<u8>> = vec![
        b"\x1b[31ma/b.md\x1b[0m".to_vec(),
        "a/\u{430}.md \u{202e}x/y.md a∕b.md".as_bytes().to_vec(),
        b"\x07\x08a/b\x00.md \x1b]52;c;aGk=\x07 \x1b[2Jc/d.md".to_vec(),
        "a/".repeat(5000).into_bytes(),
        vec![b'x'; 10_000],
        format!("{}a/b.md", "/".repeat(9000)).into_bytes(),
        b"javascript:alert(1)//a/b.md 'a/b.md' http://x.com/a/b.md".to_vec(),
    ];
    hostile.push(b"\xff\xfe a/b.md \xc3\x28".to_vec());
    for h in hostile {
        t.feed(&h);
        t.feed(b"\r\n");
        for row in 0..5 {
            for col in (0..40).step_by(3) {
                let (x, y) = at(col, row);
                t.on_pointer_move(x, y, cmd());
                if let Some(p) = t.file_hover_path() {
                    assert!(p.len() <= 300 && p.chars().all(|c| c.is_ascii_alphanumeric() || "_.+@%-/~".contains(c)) && !p.contains(".."), "{p}");
                }
                click(&mut t, (col, row), cmd());
                if let Some(p) = t.take_file_click() {
                    assert!(!p.contains("..") && p.len() <= 300, "{p}");
                }
            }
        }
    }
}

#[test]
fn five_thousand_lines_cost_one_row_per_pointer_move() {
    let (mut t, _ev, _rx) = term();
    let body: String = (0..5000).map(|i| format!("{i} src/dir{i}/file.rs:{i} done\r\n")).collect();
    t.feed(body.as_bytes());
    let t0 = std::time::Instant::now();
    for i in 0..1000 {
        let (x, y) = at(8 + i % 10, i % 5);
        t.on_pointer_move(x, y, cmd());
    }
    // 1000 moves, a row (and its wraps) each: far below a frame budget apiece.
    assert!(t0.elapsed().as_millis() < 500, "{:?}", t0.elapsed());
    assert_eq!(t.file_probes, 1000);
}
