//! Bytes on their way into and out of a terminal.
//!
//! Two jobs, both about not lying to a terminal emulator:
//!
//! * a **replay** (the last 256 KB repainted into a fresh xterm on reconnect)
//!   must not contain the sequences that *ask* or *act* — answered again by the
//!   new parser, they land as `37;3R…` on a bash command line;
//! * **typing** into an agent must deliver a block of text as one paste, with
//!   Return as its own keystroke after it, or Claude Code inserts the newline
//!   into the message instead of sending it.

/// Output bytes kept per session for replay.
pub const REPLAY_BYTES: usize = 256 * 1024;

/// Remove the sequences that *act* rather than paint from a byte stream.
///
/// The replay ring holds raw PTY output, questions included: a shell prompt
/// asks where the cursor is (`CSI 6n`) on nearly every redraw, a TUI asks what
/// the terminal is (`CSI c`) when it starts. Replayed into a fresh xterm, those
/// bytes are not history — the parser sees live requests and answers every one
/// of them back down the WebSocket, into the PTY, onto the shell's stdin. With
/// an agent's TUI in front, it eats them; at a bash prompt they land on the
/// command line as `37;3R37;3R…`, one per query in 256 KB of scrollback.
///
/// A clipboard write (OSC 52) is the same problem without a question mark: it
/// is an action the sequence performs, so replaying one silently re-copies a
/// word selected an hour ago over whatever the user is carrying now.
///
/// Repainting the screen needs none of it, so the replay drops them and the
/// live stream (where the answer, or the copy, is genuinely wanted) keeps them.
pub fn strip_replay_side_effects(input: &[u8]) -> Vec<u8> {
    const ESC: u8 = 0x1b;
    const BEL: u8 = 0x07;
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != ESC || i + 1 >= input.len() {
            out.push(input[i]);
            i += 1;
            continue;
        }
        match input[i + 1] {
            // CSI: parameter bytes, then intermediates, then one final byte.
            b'[' => {
                let params_start = i + 2;
                let mut j = params_start;
                while j < input.len() && (0x30..=0x3f).contains(&input[j]) {
                    j += 1;
                }
                let params_end = j;
                while j < input.len() && (0x20..=0x2f).contains(&input[j]) {
                    j += 1;
                }
                let Some(&final_byte) = input.get(j) else {
                    // Truncated by the ring's 256 KB cap: nothing to classify.
                    out.extend_from_slice(&input[i..]);
                    break;
                };
                if !is_csi_query(
                    &input[params_start..params_end],
                    &input[params_end..j],
                    final_byte,
                ) {
                    out.extend_from_slice(&input[i..=j]);
                }
                i = j + 1;
            }
            // OSC and DCS: a payload up to ST (`ESC \`), or BEL for OSC.
            kind @ (b']' | b'P') => {
                let payload_start = i + 2;
                let mut j = payload_start;
                let mut end = None;
                while j < input.len() {
                    if kind == b']' && input[j] == BEL {
                        end = Some((j, j + 1));
                        break;
                    }
                    if input[j] == ESC && input.get(j + 1) == Some(&b'\\') {
                        end = Some((j, j + 2));
                        break;
                    }
                    j += 1;
                }
                let Some((payload_end, next)) = end else {
                    out.extend_from_slice(&input[i..]);
                    break;
                };
                let payload = &input[payload_start..payload_end];
                let acts = if kind == b']' {
                    // A colour/clipboard read is the field `?` on its own.
                    payload.rsplit(|&b| b == b';').next() == Some(b"?")
                        // OSC 52 the other way round: a clipboard write.
                        || payload.starts_with(b"52;")
                } else {
                    // XTGETTCAP (`+q`) and DECRQSS (`$q`).
                    payload.starts_with(b"+q") || payload.starts_with(b"$q")
                };
                if !acts {
                    out.extend_from_slice(&input[i..next]);
                }
                i = next;
            }
            // Anything else after ESC is copied byte by byte.
            _ => {
                out.push(input[i]);
                i += 1;
            }
        }
    }
    out
}

/// Whether one CSI sequence asks the terminal for something.
fn is_csi_query(params: &[u8], intermediates: &[u8], final_byte: u8) -> bool {
    // The first numeric parameter, for the window-report family.
    let first_param = || {
        params
            .split(|&b| b == b';')
            .next()
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|p| p.parse::<u16>().ok())
    };
    match final_byte {
        // Device Status Report (`5n`, `6n`, `?6n`) — the cursor-position one is
        // what floods a bash prompt. Every DSR variant is a question.
        b'n' => true,
        // Device Attributes (`c`, `>c`, `=c`).
        b'c' => true,
        // DECRQM, mode query: `CSI ? Ps $ p`.
        b'p' => intermediates == b"$",
        // XTVERSION is `CSI > Ps q`; plain `CSI Ps SP q` is DECSCUSR, which
        // sets the cursor shape and must survive.
        b'q' => params.first() == Some(&b'>'),
        // Window/size reports. 22 and 23 push/pop the title and are not asks.
        b't' => matches!(first_param(), Some(11 | 13..=16 | 18..=21)),
        _ => false,
    }
}

/// Bracketed paste, both halves of it.
///
/// `?2004h` / `?2004l` is the *app* telling the terminal "wrap pastes for me";
/// `200~` / `201~` is the terminal doing so. sbxw writes into the PTY master, so
/// it is playing the terminal here: when the app has asked, sending the markers
/// is exactly what a real one does when you hit ⌘V.
///
/// This matters because the alternative is a guess. Without markers Claude Code
/// infers a paste from arrival timing, and the Return meant to submit the
/// message can land inside that window and be swallowed into it — which is how a
/// relayed question ended up sitting in the prompt as `[Pasted text #1 +18
/// lines]`, typed but never sent. `201~` ends the paste on a byte rather than on
/// a stopwatch, so the Return after it can only be a Return.
///
/// Never sent blind: an app that has *not* enabled the mode would take the
/// markers as literal input, so `bracketed_paste_enabled` reads the answer off
/// what the app itself printed.
pub const BRACKETED_PASTE_ON: &[u8] = b"\x1b[?2004h";
pub const BRACKETED_PASTE_OFF: &[u8] = b"\x1b[?2004l";
pub const PASTE_START: &[u8] = b"\x1b[200~";
pub const PASTE_END: &[u8] = b"\x1b[201~";

/// Above this, a single-line message is still delivered as a burst and read as a
/// paste. Well under the terminal's own line buffer, and far above anything a
/// person types into the island's composer.
pub const BULK_TEXT_BYTES: usize = 200;

/// Index of the last occurrence of `needle` in `hay`.
pub fn last_index_of(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=(hay.len() - needle.len()))
        .rev()
        .find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Has the app on the other end asked for bracketed paste? Read off its own
/// output (`replay`): the *last* of on/off wins, since an app turns the mode
/// off for a full-screen view and back on for its prompt.
pub fn bracketed_paste_enabled(replay: &[u8]) -> bool {
    match (
        last_index_of(replay, BRACKETED_PASTE_ON),
        last_index_of(replay, BRACKETED_PASTE_OFF),
    ) {
        (Some(on), Some(off)) => on > off,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Is `text` a paste rather than typing? A newline, or a burst long enough
/// that Claude Code coalesces it anyway.
pub fn is_bulky(text: &str) -> bool {
    text.contains('\n') || text.len() > BULK_TEXT_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_asks_the_terminal_nothing() {
        let history = b"user@box:~$ \x1b[6nls\r\n\x1b[?6n\x1b[5n\x1b[c\x1b[>c";
        assert_eq!(
            strip_replay_side_effects(history),
            b"user@box:~$ ls\r\n".to_vec()
        );
    }

    /// Everything that paints has to come through untouched — a replay whose
    /// colours, cursor moves or title are missing is worse than the flood.
    #[test]
    fn a_replay_keeps_everything_that_paints() {
        for painting in [
            &b"\x1b[1;32mgreen\x1b[0m"[..],
            &b"\x1b[2J\x1b[H"[..],
            &b"\x1b[10;40H"[..],
            // DECSCUSR: final `q` like XTVERSION, but it *sets* the cursor.
            &b"\x1b[2 q"[..],
            &b"\x1b]0;a title\x07"[..],
            &b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"[..],
            &b"\x1b[22;0t\x1b[23;0t"[..],
            &b"\x1b[?2004h\x1b[?25l"[..],
        ] {
            assert_eq!(
                strip_replay_side_effects(painting),
                painting.to_vec(),
                "{:?} must survive the replay filter",
                String::from_utf8_lossy(painting)
            );
        }
    }

    /// A clipboard write is not history: the pane the user reconnects would
    /// re-copy whatever the agent last selected, over whatever they are
    /// carrying now. The live stream still delivers it — that is the copy the
    /// browser is meant to make (see the OSC 52 handler in `panes.js`).
    #[test]
    fn a_replay_does_not_touch_the_clipboard() {
        let history = b"out\x1b]52;c;aGVsbG8=\x07put\x1b]52;c;aGk=\x1b\\ ";
        assert_eq!(strip_replay_side_effects(history), b"output ".to_vec());
        // A title shares the `]` opener; only 52 goes.
        assert_eq!(
            strip_replay_side_effects(b"\x1b]0;52;not this\x07"),
            b"\x1b]0;52;not this\x07".to_vec()
        );
    }

    #[test]
    fn the_other_question_shapes_go_too() {
        // DECRQM, XTVERSION, a window-size report, an OSC colour read, and
        // XTGETTCAP — each answered by xterm.js, each pointless on a repaint.
        let asks = b"a\x1b[?2026$pb\x1b[>0qc\x1b[18td\x1b]11;?\x1b\\e\x1bP+q544e\x1b\\f";
        assert_eq!(strip_replay_side_effects(asks), b"abcdef".to_vec());
    }

    /// The ring buffer cuts at 256 KB wherever that lands, so both ends of a
    /// replay can hold half a sequence. Half of something is not a question,
    /// and dropping bytes we cannot classify would corrupt the repaint.
    #[test]
    fn a_sequence_cut_by_the_ring_is_left_alone() {
        assert_eq!(
            strip_replay_side_effects(b"text\x1b[6"),
            b"text\x1b[6".to_vec()
        );
        assert_eq!(strip_replay_side_effects(b"text\x1b"), b"text\x1b".to_vec());
        assert_eq!(
            strip_replay_side_effects(b"text\x1b]11;?"),
            b"text\x1b]11;?".to_vec()
        );
        // A trailing fragment of an answered query still goes, up to the cut.
        assert_eq!(strip_replay_side_effects(b"6n\x1b[6n"), b"6n".to_vec());
    }


    #[test]
    fn bracketed_paste_is_read_from_the_last_thing_the_app_said() {
        assert!(!bracketed_paste_enabled(b"plain"));
        assert!(bracketed_paste_enabled(b"\x1b[?2004h prompt"));
        assert!(!bracketed_paste_enabled(b"\x1b[?2004h tui \x1b[?2004l"));
        assert!(bracketed_paste_enabled(b"\x1b[?2004l \x1b[?2004h"));
    }

    #[test]
    fn last_index_of_finds_the_final_match_and_nothing_else() {
        assert_eq!(last_index_of(b"abcabc", b"bc"), Some(4));
        assert_eq!(last_index_of(b"abc", b"x"), None);
        assert_eq!(last_index_of(b"ab", b"abc"), None);
        assert_eq!(last_index_of(b"abc", b""), None);
    }

    #[test]
    fn bulk_is_a_newline_or_a_long_burst() {
        assert!(is_bulky("a\nb"));
        assert!(is_bulky(&"x".repeat(BULK_TEXT_BYTES + 1)));
        assert!(!is_bulky("hello"));
    }
}
