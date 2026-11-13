//! Where sensitive approvals will live (wave 4: Jarvis proposals on a diff,
//! network rules, host commands). Not built in wave 1, on purpose.
//!
//! The rule this module exists for: **a screen never draws or answers an
//! approval.** An intent whose registered risk is
//! [`Risk::Approval`](crate::intent::Risk::Approval) is not run when a
//! screen returns it. `App::intents` (shell.rs) queues it here instead, and
//! [`view`] draws the dialog in the core's layer, above every screen, from
//! Rust only; only the person's press on that dialog runs the intent. Luau
//! cannot see the queue, cannot press the buttons, and cannot draw over the
//! dialog (screens are laid out under it).
//!
//! Until the dialogs exist, a queued approval is refused at once with a
//! notice (no intent of wave 1 has that risk, so none can reach here).

use crate::intent::Intent;
use rune::El;

/// An intent waiting for the person's answer.
pub struct Pending {
    /// The screen that asked.
    pub screen: String,
    pub intent: Intent,
    /// What the dialog will ask (the registry's text).
    pub asks: &'static str,
}

/// The dialog layer: `None` while nothing waits. Wave 4 draws a modal here
/// (`El::modal`, Allow / Refuse, the diff for a Jarvis proposal).
pub fn view<S: 'static>(pending: &[Pending]) -> Option<El> {
    let _ = pending;
    None
}

// ---- OSC 52: a program in the sandbox asking to write the clipboard ----
//
// The web UI's policy (assets/js/panes.js, "An OSC 52 copy: done, offered or
// dropped, by size"), kept the same so both clients behave alike. The person
// copies out of the agent's terminal all day (Claude Code's own copy is OSC
// 52, and while a TUI holds the mouse the terminal makes no selection of its
// own), so a click on every copy would be too much. By decoded size:
// - up to [`OSC_COPY_QUIET_MAX`]: copied at once, with a one-line notice so a
//   copy nobody asked for is still noticed;
// - up to [`OSC_COPY_MAX`]: offered by a card the core draws (this module:
//   it is an approval surface, a screen cannot draw or press it), which
//   names the sandbox and the size, shows the start of the text with its
//   control and invisible characters made visible, and copies only on a
//   click on Copy. One offer at a time; a newer one replaces it;
// - above: dropped, with a notice.
// Writes only: the read form never reaches here (terminal.rs, `new_term`).

use rune::{col, row, text, Cursor, TextStyle, Tx};
use std::time::Instant;

pub const OSC_COPY_QUIET_MAX: usize = 4 * 1024;
pub const OSC_COPY_MAX: usize = 100 * 1024;
pub const OSC_PREVIEW_CHARS: usize = 80;
/// An offer nobody answers goes away after this long (as in the web UI).
pub const OSC_OFFER_SECS: u64 = 30;

/// What happens to an OSC 52 copy of `bytes` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscTier {
    Copy,
    Offer,
    Drop,
}

pub fn osc_tier(bytes: usize) -> OscTier {
    if bytes <= OSC_COPY_QUIET_MAX {
        OscTier::Copy
    } else if bytes <= OSC_COPY_MAX {
        OscTier::Offer
    } else {
        OscTier::Drop
    }
}

/// `512 B`, `4.0 KB` (the web UI's `oscSize`).
pub fn size_label(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// The first [`OSC_PREVIEW_CHARS`] characters, with line breaks, tabs, C0
/// and C1 controls, DEL and the bidi and zero-width marks shown as visible
/// marks, so the preview is what will be pasted (the web UI's `oscPreview`).
pub fn osc_preview(text: &str) -> String {
    let mut out: String = text
        .chars()
        .take(OSC_PREVIEW_CHARS)
        .map(|c| match c as u32 {
            0x0a => '↵',
            0x0d => '␍',
            0x09 => '⇥',
            n @ 0..=0x1f => char::from_u32(0x2400 + n).unwrap_or('�'),
            0x7f => '␡',
            0x80..=0x9f | 0x200b..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff => '�',
            _ => c,
        })
        .collect();
    if text.chars().count() > OSC_PREVIEW_CHARS {
        out.push('…');
    }
    out
}

/// A copy waiting for the person's click.
pub struct CopyOffer {
    pub sandbox: String,
    pub text: String,
    pub bytes: usize,
    pub until: Instant,
}

impl CopyOffer {
    pub fn new(sandbox: &str, text: String, now: Instant) -> CopyOffer {
        let bytes = text.len();
        CopyOffer { sandbox: sandbox.to_string(), text, bytes, until: now + std::time::Duration::from_secs(OSC_OFFER_SECS) }
    }

    /// "web-app wants to copy 6.0 KB to your clipboard:"
    pub fn headline(&self) -> String {
        let who = if self.sandbox.is_empty() { "A sandbox" } else { &self.sandbox };
        format!("{who} wants to copy {} to your clipboard:", size_label(self.bytes))
    }
}

/// The card (key `copy-offer`; buttons `copy-offer-copy`,
/// `copy-offer-dismiss`), drawn by the core above every screen.
pub fn copy_offer_view<S: 'static>(o: &CopyOffer, copy: impl Fn(&mut S, &mut Tx) + 'static, dismiss: impl Fn(&mut S) + 'static) -> El {
    use crate::theme::{c, radius};
    let button = |key: &str, label: &str, primary: bool| {
        rune::el()
            .key(key)
            .role("button")
            .label(label)
            .px(10.0)
            .py(4.0)
            .radius(radius("sm").unwrap())
            .fill(c(if primary { "green-deep" } else { "elevated" }))
            .border(1.0, c(if primary { "green-strong" } else { "border" }))
            .hover(c(if primary { "green-strong" } else { "dim" }))
            .cursor(Cursor::Pointer)
            .child(text(label.to_string()).font(TextStyle::new(12.0, 500.0)).color(c("text-bright")))
    };
    col()
        .key("copy-offer")
        .role("alertdialog")
        .label("Clipboard copy from a sandbox")
        .pin(None, None, Some(16.0), Some(64.0))
        .w(460.0)
        .padding(12.0)
        .gap(8.0)
        .radius(radius("md").unwrap())
        .fill(c("surface"))
        .border(1.0, c("amber"))
        .child(text(o.headline()).font(TextStyle::new(12.0, 500.0)).color(c("text-bright")).wrap())
        .child(
            rune::el()
                .w_full()
                .padding(8.0)
                .radius(radius("sm").unwrap())
                .fill(c("bg"))
                .child(text(osc_preview(&o.text)).font(TextStyle::new(12.0, 400.0)).color(c("text")).wrap()),
        )
        .child(
            row()
                .w_full()
                .gap(8.0)
                .child(rune::el().grow(1.0))
                .child(button("copy-offer-dismiss", "Dismiss", false).on_press(dismiss))
                .child(button("copy-offer-copy", "Copy", true).on_press_tx(copy)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_copies_by_size() {
        assert_eq!(osc_tier(40), OscTier::Copy);
        assert_eq!(osc_tier(4096), OscTier::Copy);
        assert_eq!(osc_tier(4097), OscTier::Offer);
        assert_eq!(osc_tier(100 * 1024), OscTier::Offer);
        assert_eq!(osc_tier(100 * 1024 + 1), OscTier::Drop);
        assert_eq!(size_label(40), "40 B");
        assert_eq!(size_label(6144), "6.0 KB");
    }

    #[test]
    fn preview_shows_what_would_be_pasted() {
        assert_eq!(osc_preview("ls\n\trm\r"), "ls↵⇥rm␍");
        assert_eq!(osc_preview("a\x1bb\x7fc\u{9b}"), "a␛b␡c�");
        // Bidi overrides, zero-width space and joiner, isolates, BOM.
        assert_eq!(osc_preview("x\u{202e}y\u{200b}z\u{200d}\u{2066}\u{feff}"), "x�y�z���");
        assert_eq!(osc_preview("é ü 日本"), "é ü 日本");
        let long = "x".repeat(81);
        assert_eq!(osc_preview(&long), format!("{}…", "x".repeat(80)));
        assert_eq!(osc_preview(&"x".repeat(80)), "x".repeat(80));
    }
}
