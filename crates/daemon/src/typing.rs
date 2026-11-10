//! Typing a message into an agent's terminal as if a person had.
//!
//! Three details are easy to get wrong, and each one cost a bug:
//!
//! * **Wait for the TUI to go quiet before typing**: Claude Code redraws its
//!   whole frame at start-up and drops keystrokes into a half-painted screen.
//!   Quiet is measured on the output *stream*: the replay ring's length stops
//!   changing once full, so silence would look like a flood.
//! * **Return is its own keystroke, after the paste is closed.** Claude Code
//!   reads a burst of bytes as a paste and folds a newline arriving inside it
//!   into the message. Bracketed paste markers end the paste when the app has
//!   asked for them; otherwise the echo has to go quiet first.
//! * **A warm session is not charged for a cold start**: an attached, drawn
//!   session waits ~180 ms, not the 900 ms a first frame needs, but only after
//!   the echo has *started*: a PTY that has not turned the write around yet
//!   reads as quiet, and Return would join the paste.

use crate::pty::PtySession;
use sbxw_core::terminal::{self, PASTE_END, PASTE_START};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// Silence that means a cold TUI finished its first frame.
pub const SETTLE_COLD: Duration = Duration::from_millis(900);
/// The same for a session already drawn: guarding a redraw, not a boot.
pub const SETTLE_WARM: Duration = Duration::from_millis(180);
/// A cold sandbox boots its agent first.
pub const READY_TIMEOUT: Duration = Duration::from_secs(45);
/// The echo of a typed message starts within this, or never.
pub const ECHO_TIMEOUT: Duration = Duration::from_secs(3);
/// Silence ending an echo, before Return.
pub const ECHO_QUIET: Duration = Duration::from_millis(350);
/// The same for a block sent without paste markers.
pub const ECHO_QUIET_BULK: Duration = Duration::from_millis(900);
/// Between two keystrokes answering a menu.
pub const KEY_DELAY: Duration = Duration::from_millis(60);

/// Wait until the stream is quiet for `quiet`. With `first`, wait (up to it)
/// for a first byte before timing silence. `rx` is subscribed by the caller
/// *before* the write it waits on, or the echo can slip between the two.
pub async fn settle(rx: &mut broadcast::Receiver<Vec<u8>>, first: Option<Duration>, quiet: Duration) {
    if let Some(bound) = first {
        if tokio::time::timeout(bound, rx.recv()).await.is_err() {
            return;
        }
    }
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Err(_) => return,
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return,
        }
    }
}

async fn until_dead(s: &PtySession) {
    while s.alive() {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Type `text` and submit it. `fresh`: the session was just started, so wait
/// for its first frame (racing its death, so an agent that cannot start does
/// not cost the whole ready timeout).
pub async fn type_and_submit(session: &Arc<PtySession>, text: &str, fresh: bool) -> Result<(), String> {
    let mut rx = session.subscribe();
    if fresh {
        tokio::select! {
            _ = settle(&mut rx, Some(READY_TIMEOUT), SETTLE_COLD) => {}
            _ = until_dead(session) => {}
        }
    } else {
        settle(&mut rx, None, SETTLE_WARM).await;
    }
    if !session.alive() {
        return Err("its Claude Code process is not running".into());
    }
    // Text from another agent: an ESC could close the paste and turn the
    // rest into keystrokes (a Return, a `!command`), so no control survives.
    let text = terminal::sanitize_typed(text);
    let text = text.as_str();
    let bulky = terminal::is_bulky(text);
    let bracketed = bulky && terminal::bracketed_paste_enabled(&session.replay());
    if bracketed {
        session.write(&[PASTE_START, text.as_bytes(), PASTE_END]).await;
    } else {
        session.write(&[text.as_bytes()]).await;
    }
    let quiet = if bulky && !bracketed { ECHO_QUIET_BULK } else { ECHO_QUIET };
    settle(&mut rx, Some(ECHO_TIMEOUT), quiet).await;
    session.write(&[b"\r"]).await;
    Ok(())
}

/// Send keystrokes one at a time, letting the TUI redraw between them.
pub async fn keys(session: &Arc<PtySession>, keys: &[&[u8]]) {
    for k in keys {
        session.write(&[k]).await;
        tokio::time::sleep(KEY_DELAY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::Spawn;

    /// A fake TUI: prints a prompt, echoes what it reads, and reports a line
    /// only on a *separate* Return.
    #[tokio::test]
    async fn a_message_is_echoed_then_submitted() {
        let s = PtySession::spawn(
            Spawn {
                program: "sh".into(),
                args: vec!["-c".into(), "printf 'ready> '; read line; printf '\\nSUBMITTED[%s]\\n' \"$line\"; sleep 1".into()],
                env: vec![],
            },
            0,
            || {},
        )
        .unwrap();
        let mut rx = s.subscribe();
        type_and_submit(&s, "hello there", true).await.unwrap();
        let mut seen = String::new();
        while let Ok(Ok(c)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            seen.push_str(&String::from_utf8_lossy(&c));
            if seen.contains("SUBMITTED[") {
                break;
            }
        }
        assert!(seen.contains("SUBMITTED[hello there]"), "{seen:?}");
    }

    /// What reaches the PTY: the fake TUI asks for bracketed paste, then
    /// records every byte it reads, raw, into a file.
    #[tokio::test]
    async fn no_control_character_reaches_the_terminal() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("got");
        let script = format!(
            "stty raw -echo; printf '\\033[?2004hready> '; timeout --foreground 10 dd bs=1 of={} 2>/dev/null",
            out.display()
        );
        let s = PtySession::spawn(Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }, 0, || {}).unwrap();
        let text = format!("{}\x1b[201~\r!id\nDéjà vu 👋🏽", "x".repeat(250));
        type_and_submit(&s, &text, true).await.unwrap();
        let mut got = Vec::new();
        for _ in 0..50 {
            got = std::fs::read(&out).unwrap_or_default();
            if got.ends_with(b"\r") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let body = &got[PASTE_START.len()..got.len() - PASTE_END.len() - 1];
        assert!(got.starts_with(PASTE_START), "{:?}", String::from_utf8_lossy(&got));
        assert!(got.ends_with(&[PASTE_END, b"\r"].concat()), "{:?}", String::from_utf8_lossy(&got));
        assert!(!body.contains(&0x1b), "ESC inside the paste: {:?}", String::from_utf8_lossy(body));
        assert!(!body.contains(&b'\r'), "CR inside the paste: {:?}", String::from_utf8_lossy(body));
        assert!(String::from_utf8_lossy(body).ends_with("[201~!id\nDéjà vu 👋🏽"));
    }

    #[tokio::test]
    async fn a_dead_session_is_reported_not_typed_into() {
        let s = PtySession::spawn(
            Spawn { program: "sh".into(), args: vec!["-c".into(), "exit 0".into()], env: vec![] },
            0,
            || {},
        )
        .unwrap();
        let r = tokio::time::timeout(Duration::from_secs(5), type_and_submit(&s, "x", true)).await.unwrap();
        assert!(r.is_err());
    }
}
