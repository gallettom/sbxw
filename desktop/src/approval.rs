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
