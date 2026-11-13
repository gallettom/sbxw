//! The terminal as Rune elements, with its input wired: [`view`] is the grid
//! alone, [`pane`] the grid with its status line, padding and background,
//! laid out to the space it gets.

use super::{cell_width, rgb, style, TerminalState, LINE_HEIGHT, PAD, STATUS_H};
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
    let cw = cell_width();
    let mut g = grid(cols as u16, cw, LINE_HEIGHT, t.rows.clone())
        .key(t.key("grid"))
        .w(cols as f32 * cw)
        .h(rows as f32 * LINE_HEIGHT)
        .focusable(true)
        .cursor(if t.hovering_link() { Cursor::Pointer } else { Cursor::Text })
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
    if let Some((c, r)) = t.cursor {
        g = g.grid_caret(c, r);
        let (x, y) = (c as f32 * cw, r as f32 * LINE_HEIGHT);
        // Where the input method composes and puts its candidate window.
        let mut caret = el().key(t.key("term-ime")).absolute(x, y).w(cw).h(LINE_HEIGHT);
        if t.focused() {
            caret = caret.ime_cursor().ime_option_as_alt(t.keys.option_as_meta);
        }
        g = g.child(caret);
        if let Some(p) = &t.preedit {
            let theme = t.theme();
            g = g.child(
                el().key(t.key("term-preedit"))
                    .absolute(x, y)
                    .h(LINE_HEIGHT)
                    .fill(rgb(theme.background))
                    .border(1.0, rgb(theme.cursor))
                    .child(text(p.clone()).font(style()).color(rgb(theme.foreground))),
            );
        }
    }
    g
}

/// The whole pane (key `terminal`, or `terminal-<id>`): a status line (size and connection),
/// the grid, the theme's background; it fits the terminal to the space it is
/// given and tells the daemon after the size holds (debounced).
pub fn pane<S: 'static, L>(t: &TerminalState, lens: L) -> El
where
    L: Fn(&mut S) -> Option<&mut TerminalState> + Clone + 'static,
{
    let (cols, rows) = t.size;
    let theme = t.theme();
    let dim = rgb(super::theme::mix(theme.foreground, theme.background, 0.45));
    let scrolled = match t.display_offset() {
        0 => String::new(),
        n => format!("  scrolled {n} of {} lines (Shift+End: bottom)", t.history_len()),
    };
    col()
        .key(t.key("terminal"))
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .padding(PAD)
        .fill(rgb(theme.background))
        .on_layout_tx({
            let lens = lens.clone();
            move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
            let clock = tx.clock();
            if let Some(t) = lens(s) {
                let before = t.size;
                t.fit_at(r[2], r[3], clock);
                if t.size != before {
                    tx.wake_after(std::time::Duration::from_secs_f64(super::RESIZE_DEBOUNCE));
                }
            }
        }})
        .child(
            el().key(t.key("term-status"))
                .h(STATUS_H)
                .child(text(format!("terminal {cols}x{rows}  {}{scrolled}", t.status)).font(TextStyle::new(12.0, 400.0)).color(dim)),
        )
        .child(view(t, lens))
}
