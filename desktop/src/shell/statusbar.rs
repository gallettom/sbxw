//! The status bar: one thin line across the bottom of the window, drawn by
//! the core in every layout, under the panes (their area shrinks by its
//! height, it never covers a terminal).
//!
//! Left: the bring-ups in progress. Right, in this order: Help. Each entry
//! runs an intent that exists already, or opens a read-only popover. Its
//! code stays here so the window's structure can be rearranged around it.
//!
//! What fits ([`fit`]) follows the window's width: the bar never wraps and
//! never cuts a number. Under a width it drops, in this order, the
//! bring-up's step label, the word "CPU", the word "Help".

use super::{bar, App};
use crate::screen;
use crate::theme::{self, c, layout};
use rune::{el, row, text, Cursor, El};

/// The bar's height (12 pt text, 24 pt targets, a hairline above).
pub const H: f32 = layout::STATUS_BAR_H;
/// The bar's side padding and the gap between its right entries.
pub const PAD: f32 = 8.0;
pub const GAP: f32 = 4.0;
/// An entry: padding each side, its 12 pt icon, the gap to its text.
pub const ITEM_PX: f32 = 8.0;
pub const ICON: f32 = 12.0;
pub const ICON_GAP: f32 = 6.0;
/// The type: 12 pt; numbers in the monospace face are tabular already.
pub const TEXT: f32 = 12.0;

/// What a text of the bar measures (12 pt, `weight`).
pub fn measure(t: &str, weight: f32) -> f32 {
    bar::measure(t, TEXT, weight)
}

/// An entry's width: icon, then its text when it has one.
pub fn item_w(t: &str) -> f32 {
    ITEM_PX * 2.0 + ICON + if t.is_empty() { 0.0 } else { ICON_GAP + measure(t, 400.0) }
}

/// What shows at a width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    pub help_word: bool,
}

/// What the bar shows `width` points wide: everything when it fits, else
/// the parts dropped in their order until it does.
pub fn fit(width: f32) -> Fit {
    let need = |f: Fit| PAD * 2.0 + item_w(if f.help_word { "Help" } else { "" });
    let full = Fit { help_word: true };
    if need(full) <= width + 0.5 {
        return full;
    }
    Fit { help_word: false }
}

/// The bar (key `status-bar`).
pub fn view(s: &App) -> El {
    let f = fit(s.window.0);
    let right = row().key("status-right").gap(GAP).items_center().child(help_view(f.help_word));
    row()
        .key("status-bar")
        .role("status")
        .w_full()
        .h(H)
        .px(PAD)
        .items_center()
        .fill(c("status-bar"))
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), None).h(1.0).fill(c("status-line")))
        .child(row().key("status-left").grow(1.0).min_w(0.0).h_full().items_center())
        .child(right)
}

/// One pressable entry: its icon in `tone`'s colour (neutral: muted),
/// then its text, hovered on the quiet tint.
fn entry(key: &str, icon: &str, label: &str, color: &str, help: &str) -> El {
    let mut e = row()
        .key(key.to_string())
        .role("button")
        .label(if label.is_empty() { help.to_string() } else { label.to_string() })
        .help(help.to_string())
        .h(layout::DENSE_H)
        .px(ITEM_PX)
        .gap(ICON_GAP)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .hover(c("hover-subtle"))
        .focus_ring(2.0, -2.0, c("focus"))
        .cursor(Cursor::Pointer)
        .child(rune::icon(crate::icons::get(icon).unwrap(), ICON).color(c(if color == "text" { "text-muted" } else { color })));
    if !label.is_empty() {
        e = e.child(text(label.to_string()).font(screen::body(TEXT, 400.0)).color(c(color)));
    }
    e
}

/// Help at the far right: the existing `show_help` (the macOS Help menu
/// and Ctrl+Alt+H open the same dialog).
fn help_view(word: bool) -> El {
    entry("status-help", "help", if word { "Help" } else { "" }, "text", if cfg!(target_os = "macos") { "Help: shortcuts and how sbxw works (⌃⌥H)" } else { "Help: shortcuts and how sbxw works (Ctrl+Alt+H)" }).on_press(|s: &mut App| s.show_help())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_help_word_goes_under_a_narrow_window() {
        assert!(fit(1284.0).help_word);
        assert!(!fit(30.0).help_word);
    }
}
