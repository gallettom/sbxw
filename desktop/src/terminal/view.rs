//! The terminal as Rune elements, with its input wired: [`view`] is the grid
//! alone, [`pane`] the grid with its status line, padding and background,
//! laid out to the space it gets.

use super::{rgb, TerminalState, PAD, STATUS_H};
use rune::{col, el, grid, text, Cursor, Drag, El, KeyEvent, PointerButton, PointerEvent, TextStyle, Tx, WheelEvent};

/// The grid (key `grid`, or `grid-<id>` when [`TerminalState::id`] is set), sized to the terminal's cells: keys (focused),
/// copy and paste shortcuts, wheel, selection by drag and double and triple
/// clicks, mouse reports, OSC 8 links (⌘ or Ctrl + click), and the input
/// method's caret and composition at the cursor.
///
/// `lens` finds this terminal in the app's state (`None` when it is gone): a
/// closure, so a shell with several panes passes one per pane
/// (`move |s: &mut App| s.panes.get_mut(i)?.term.as_mut()`).
pub fn view<S: 'static, L>(t: &TerminalState, lens: L) -> El
where
    L: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
{
    let (cols, rows) = t.size;
    // The cell size the rows were built for: smaller while the grid is scaled
    // to fit (`TerminalState::metrics`); the handlers' points map through it.
    let m = t.metrics();
    let (cw, lh) = (m.cell_w, m.line_h);
    let mut g = grid(cols as u16, cw, lh, t.rows.clone())
        .key(t.key("grid"))
        .w(cols as f32 * cw)
        .h(rows as f32 * lh)
        .focusable(true)
        .cursor(if t.hovering_link() || t.file_hover_path().is_some() { Cursor::Pointer } else { Cursor::Text })
        .pointer_buttons([PointerButton::Middle])
        .on_key({
            let lens = lens.clone();
            move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                let again = lens.clone();
                lens(s).is_some_and(|t| t.on_key_tx(ev, tx, again))
            }
        })
        .on_drag({
            let lens = lens.clone();
            move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            let (button, mods) = (tx.button(), tx.mods());
            if let Some(url) = lens(s).and_then(|t| t.on_drag(d, button, mods)) {
                if let Err(e) = tx.open_url(&url) {
                    log::warn!("not opening {url}: {e:?}");
                }
            }
        }})
        .on_wheel({
            let lens = lens.clone();
            move |s: &mut S, ev: WheelEvent, _tx: &mut Tx| lens(s).map(|t| t.on_wheel(ev.dy, ev.x, ev.y, ev.mods)).is_some()
        })
        .on_pointer_move_local({
            let lens = lens.clone();
            move |s: &mut S, p: PointerEvent, tx: &mut Tx| {
                let mods = tx.mods();
                if let Some(t) = lens(s) {
                    t.on_pointer_move(p.x, p.y, mods);
                }
            }
        })
        .on_secondary_press_local(move |s: &mut S, p: PointerEvent, tx: &mut Tx| {
            let mods = tx.mods();
            if let Some(t) = lens(s) {
                t.on_secondary_click(p.x, p.y, mods);
            }
        });
    if let Some(p) = t.file_hover_path() {
        g = g.help(format!("Open {p} in Files (resolved in the workspace or thread folder)"));
    }
    if let Some((c, r)) = t.cursor {
        g = g.grid_caret(c, r);
        let (x, y) = (c as f32 * cw, r as f32 * lh);
        // Where the input method composes and puts its candidate window.
        let mut caret = el().key(t.key("term-ime")).absolute(x, y).w(cw).h(lh);
        if t.focused() {
            caret = caret.ime_cursor().ime_option_as_alt(t.keys.option_as_meta);
        }
        g = g.child(caret);
        if let Some(p) = &t.preedit {
            let theme = t.theme();
            g = g.child(
                el().key(t.key("term-preedit"))
                    .absolute(x, y)
                    .h(lh)
                    .fill(rgb(theme.background))
                    .border(1.0, rgb(theme.cursor))
                    .child(text(p.clone()).font(m.style()).color(rgb(theme.foreground))),
            );
        }
    }
    // A new size or cell size lands at once: Rune would spring both (about 0.3 s),
    // and a terminal is never drawn at a size between two.
    g.instant_changes()
}

/// The whole pane (key `terminal`, or `terminal-<id>`): a status line (size,
/// who sets it, connection), the grid, the theme's background; it fits the
/// terminal to the space it is given and tells the daemon after the size
/// holds (debounced). The grid has the PTY's size (see `terminal.rs`, "Size").
pub fn pane<S: 'static, L>(t: &TerminalState, lens: L) -> El
where
    L: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
{
    pane_with(t, lens.clone(), lens)
}

/// [`pane`] with a separate lens for the layout (fitting the terminal to the
/// pane): the shell keeps fitting a terminal its input lens refuses (under a
/// dialog), so the input gate never changes a terminal's size.
pub fn pane_with<S: 'static, L, F>(t: &TerminalState, lens: L, layout: F) -> El
where
    L: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
    F: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
{
    pane_held(t, lens, layout, false)
}

/// [`pane_with`], `held` while something is drawn over the pane area (the
/// base view, an overlay): the room then has no layout handler, so the
/// terminal keeps its size whatever moves under it (a layout change, a
/// window resize). Rune reports a rect to a handler that was not there on
/// the frame before, so the first frame the panes show again fits each
/// terminal to its rect at that moment: one resize for a terminal whose
/// room changed meanwhile, none for the others (RESIZE.md).
pub fn pane_held<S: 'static, L, F>(t: &TerminalState, lens: L, layout: F, held: bool) -> El
where
    L: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
    F: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
{
    let (cols, rows) = t.size;
    let theme = t.theme();
    let dim = rgb(super::theme::mix(theme.foreground, theme.background, 0.45));
    let scrolled = match t.display_offset() {
        0 => String::new(),
        n => format!("  scrolled {n} of {} lines (Shift+End: bottom)", t.history_len()),
    };
    let owner = match (t.sized_elsewhere(), t.display_scale().1) {
        (false, _) => "",
        (true, false) => ", sized by another window (type or click here to take it)",
        (true, true) => ", sized by another window, too large to show whole (type or click here to take it)",
    };
    // The room the grid has. Its layout fits the terminal: keyed per
    // terminal, so a terminal replacing another in this pane (same keys,
    // same rect) still hears its first layout. The grid has the PTY's size,
    // which may be larger than the room when another window sets it: drawn
    // with smaller cells to fit (`metrics`), top-left, and clipped only
    // below the floor (`MIN_SCALE`).
    let room = col().key(t.fit_key()).grow(1.0).min_w(0.0).min_h(0.0).clip_rounded(0.0);
    let room = if held {
        room
    } else {
        room.on_layout_tx({
            move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
                let clock = tx.clock();
                if let Some(t) = layout(s) {
                    let before = t.wanted_size();
                    // fit_at takes the whole pane: add back the padding and the status line.
                    t.fit_at(r[2] + 2.0 * PAD, r[3] + 2.0 * PAD + STATUS_H, clock);
                    if t.wanted_size() != before {
                        tx.wake_after(std::time::Duration::from_secs_f64(super::RESIZE_DEBOUNCE));
                    }
                }
            }
        })
    };
    col()
        .key(t.key("terminal"))
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .padding(PAD)
        .fill(rgb(theme.background))
        .child(
            el().key(t.key("term-status"))
                .h(STATUS_H)
                .child(text(format!("terminal {cols}x{rows}{owner}  {}{scrolled}", t.status)).font(TextStyle::new(crate::theme::font_size::CAPTION, 400.0)).color(dim)),
        )
        .child(room.child(view(t, lens)))
}
