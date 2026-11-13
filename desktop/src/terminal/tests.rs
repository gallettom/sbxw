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
    assert_eq!(resizes(&rx), vec![(100, 30), (40, 5)]);
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

#[test]
fn theme_change_redraws_everything() {
    let (mut t, _ev, _rx) = term();
    t.feed(b"x");
    let before = t.rebuilt_rows;
    t.set_theme(Theme::from_setting_strings("#fdf6e3", "auto"));
    t.poll(None);
    assert_eq!(t.rebuilt_rows - before, 5);
}

/// Full-screen redraw cost, CPU side: every row of a 111x42 screen changes
/// each frame (colours on every cell, like `htop` or a `cat` of coloured
/// output), then Rune lays out and builds the frame's instances. GPU
/// raster and presentation are not included.
/// `cargo test --release --lib full_screen -- --ignored --nocapture`
#[test]
#[ignore]
fn full_screen_redraw_cost() {
    use rune::{Cx, El, Env, Runtime};

    struct S(TerminalState);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let (c, r) = s.0.size;
        rune::grid(c as u16, cell_width(), LINE_HEIGHT, s.0.rows.clone()).w(c as f32 * cell_width()).h(r as f32 * LINE_HEIGHT)
    }
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, _in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    t.resize(111, 42);
    let frame_bytes = |k: usize| {
        let mut out = String::from("\x1b[H");
        for r in 0..42 {
            for c in 0..111 {
                let col = 31 + (r + c + k) % 7;
                out += &format!("\x1b[{col}m{}", (b'a' + ((r * 7 + c + k) % 26) as u8) as char);
            }
            if r < 41 {
                out += "\r\n";
            }
        }
        out.into_bytes()
    };
    let mut rt = Runtime::new(S(t), view, (960.0, 800.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let (mut feed, mut frame) = (vec![], vec![]);
    for k in 0..120 {
        let bytes = frame_bytes(k);
        let a = Instant::now();
        ev_tx.send(TermEvent::Output(bytes)).unwrap();
        let mut changed = false;
        rt.update(|s, _| changed = s.0.poll(None));
        let b = Instant::now();
        rt.frame(1.0 / 60.0);
        let c = Instant::now();
        assert!(changed);
        feed.push((b - a).as_secs_f64() * 1e3);
        frame.push((c - b).as_secs_f64() * 1e3);
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (v[v.len() / 2], v[v.len() * 95 / 100])
    };
    let (f50, f95) = med(&mut feed);
    let (r50, r95) = med(&mut frame);
    println!("111x42, every row changed, 120 frames, scale 2:");
    println!("  parse + rebuild rows: p50 {f50:.2} ms, p95 {f95:.2} ms");
    println!("  Rune layout + paint (CPU): p50 {r50:.2} ms, p95 {r95:.2} ms");
    println!("  total p50 {:.2} ms", f50 + r50);
    println!("  instances last frame: {}", rt.instances.len());
}

/// Throughput of a large `cat`: 20 MB of mixed text (a log with some colour)
/// fed in 64 KB chunks, as the socket thread delivers them, with one frame
/// built per 1 MB (a frame's worth at 60 fps of a fast stream). Reports
/// MB/s of parse and the rows rebuilt.
/// `cargo test --release --lib big_cat -- --ignored --nocapture`
#[test]
#[ignore]
fn big_cat_throughput() {
    use rune::{Cx, El, Env, Runtime};

    struct S(TerminalState);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let (c, r) = s.0.size;
        rune::grid(c as u16, cell_width(), LINE_HEIGHT, s.0.rows.clone()).w(c as f32 * cell_width()).h(r as f32 * LINE_HEIGHT)
    }
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, _in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    t.resize(111, 42);
    let mut text = String::new();
    let mut i = 0;
    while text.len() < 20 << 20 {
        i += 1;
        text += &format!("2026-11-13T10:{:02}:{:02}Z \x1b[32mINFO\x1b[0m request {i} served in {} ms path=/api/sandboxes/{}\r\n", i / 60 % 60, i % 60, i % 97, i % 13);
    }
    let bytes = text.into_bytes();
    let mut rt = Runtime::new(S(t), view, (960.0, 800.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let start = Instant::now();
    let (mut parse, mut frames) = (Duration::ZERO, 0);
    for mb in bytes.chunks(1 << 20) {
        for c in mb.chunks(64 << 10) {
            ev_tx.send(TermEvent::Output(c.to_vec())).unwrap();
        }
        let a = Instant::now();
        rt.update(|s, _| {
            s.0.poll(None);
        });
        parse += a.elapsed();
        rt.frame(1.0 / 60.0);
        frames += 1;
    }
    let total = start.elapsed();
    let mb = bytes.len() as f64 / (1 << 20) as f64;
    let t = &rt.state.0;
    println!("{mb:.1} MB, {frames} frames, history {}:", t.history_len());
    println!("  parse + rebuild: {:.0} ms ({:.0} MB/s)", parse.as_secs_f64() * 1e3, mb / parse.as_secs_f64());
    println!("  with Rune layout + paint: {:.0} ms ({:.0} MB/s), {:.2} ms per frame", total.as_secs_f64() * 1e3, mb / total.as_secs_f64(), total.as_secs_f64() * 1e3 / frames as f64);
    println!("  rows rebuilt {} over {} rebuilds", t.rebuilt_rows, t.rebuilds);
}
