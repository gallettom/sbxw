//! Keys to bytes (xterm style), the app's own terminal shortcuts (copy,
//! paste, scrollback), and paste encoding.

use rune::{Key, KeyEvent};

/// xterm's modifier parameter: 1 + shift + 2 alt + 4 ctrl.
fn mod_param(m: rune::Mods) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' | '~' => Some(0x1e),
        '_' | '-' | '/' | '7' => Some(0x1f),
        ' ' | '@' | '2' => Some(0),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// How keys map on this machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyOpts {
    /// macOS rules: ⌘ is the app's, Option composes characters.
    pub mac: bool,
    /// macOS "Option as Meta": Option+key sends ESC + key instead of the
    /// character the layout composes. Off by default, as in Terminal.app and
    /// iTerm2. (Rune delivers the unshifted key with `alt` while the terminal's
    /// caret sets `ime_option_as_alt`, which the view does when this is on.)
    pub option_as_meta: bool,
    /// Keys the shell keeps for itself (push-to-talk, F8 by default): pressed
    /// with no Cmd, Ctrl or Alt, [`TerminalState::on_key_tx`](super::TerminalState::on_key_tx)
    /// sends nothing and returns false, so the key bubbles to the shell. With
    /// a modifier they still reach the program. Empty by default. Releases
    /// never reach the program (the terminal sends no key-up bytes).
    pub reserved: &'static [Key],
}

impl Default for KeyOpts {
    fn default() -> Self {
        KeyOpts { mac: cfg!(target_os = "macos"), option_as_meta: false, reserved: &[] }
    }
}

impl KeyOpts {
    /// `ev` is one of [`Self::reserved`], pressed alone (Shift allowed).
    pub fn is_reserved(&self, ev: &KeyEvent) -> bool {
        let m = ev.mods;
        !(m.cmd || m.ctrl || m.alt || m.logo) && self.reserved.contains(&ev.key)
    }
}

/// What a key sends to the program, xterm style. `app_cursor` is DECCKM
/// (arrows as `ESC O A`), which full-screen programs turn on. This
/// platform's rules ([`KeyOpts::default`]).
pub fn key_bytes(ev: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    key_bytes_with(ev, app_cursor, KeyOpts::default())
}

/// [`key_bytes`] with explicit platform rules.
///
/// Text a layout composed reaches the program as typed: Option combinations
/// on macOS (`|` is Option+7 on Swiss and French layouts), AltGr on Windows
/// (which Rune reports as Ctrl+Alt with the text). Alt is Meta (an ESC prefix)
/// for keys that typed their own plain character, off macOS, or with
/// [`KeyOpts::option_as_meta`]. Dead keys and input methods arrive as
/// `Ime::Commit`, not here (see `TerminalState::on_ime`).
pub fn key_bytes_with(ev: &KeyEvent, app_cursor: bool, opts: KeyOpts) -> Option<Vec<u8>> {
    let m = ev.mods;
    let p = mod_param(m);
    let csi = |final_: char| {
        Some(if p > 1 {
            format!("\x1b[1;{p}{final_}")
        } else if app_cursor {
            format!("\x1bO{final_}")
        } else {
            format!("\x1b[{final_}")
        })
        .map(String::into_bytes)
    };
    let tilde = |n: u8| Some(if p > 1 { format!("\x1b[{n};{p}~") } else { format!("\x1b[{n}~") }.into_bytes());
    let alt = |mut b: Vec<u8>| {
        if m.alt {
            b.insert(0, 0x1b);
        }
        Some(b)
    };
    match &ev.key {
        Key::Enter => alt(b"\r".to_vec()),
        Key::Backspace if m.ctrl => alt(vec![0x08]),
        Key::Backspace => alt(vec![0x7f]),
        Key::Tab if m.shift => Some(b"\x1b[Z".to_vec()),
        Key::Tab => alt(b"\t".to_vec()),
        Key::Escape => alt(vec![0x1b]),
        Key::Up => csi('A'),
        Key::Down => csi('B'),
        Key::Right => csi('C'),
        Key::Left => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Function(n @ 1..=4) => {
            let f = (b'P' + n - 1) as char;
            Some(if p > 1 { format!("\x1b[1;{p}{f}") } else { format!("\x1bO{f}") }.into_bytes())
        }
        Key::Function(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(*n - 5) as usize]),
        // Ctrl+<key> off macOS (Rune reports Ctrl as the primary modifier there).
        Key::Command(c) if m.ctrl => alt(vec![ctrl_byte(*c)?]),
        // ⌘ shortcuts on macOS belong to the app, not the shell.
        Key::Command(_) => None,
        // AltGr on Windows: Left Ctrl + Left Alt and the character it typed.
        Key::Text(s) if m.ctrl && m.alt && !opts.mac => Some(s.as_bytes().to_vec()),
        // Ctrl+<key> as text (macOS, where Ctrl is not the primary modifier; Ctrl+Space).
        Key::Text(s) if m.ctrl && s.chars().count() == 1 => alt(vec![ctrl_byte(s.chars().next()?)?]),
        // Cmd+Space on macOS (the only Text with cmd and no ctrl): the app's.
        Key::Text(_) if m.cmd && !m.ctrl => None,
        // Option composes on macOS: send what it typed.
        Key::Text(s) if m.alt && opts.mac && !opts.option_as_meta => Some(s.as_bytes().to_vec()),
        Key::Text(s) => alt(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// A terminal shortcut the app handles itself instead of sending it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Copy,
    Paste,
    SelectAll,
    /// Scroll the history by pages (negative: up), or to an end.
    ScrollPage(i32),
    ScrollTop,
    ScrollBottom,
    /// Ctrl/Cmd + `=`, `-`, `0`: bigger, smaller, reset (for the caller).
    FontBigger,
    FontSmaller,
    FontReset,
}

/// The platform's terminal shortcuts: ⌘C, ⌘V, ⌘A on macOS; Ctrl+Shift+C,
/// Ctrl+Shift+V, Ctrl+Shift+A and Shift+Insert elsewhere (plain Ctrl+C stays
/// the shell's interrupt). Shift+PageUp/PageDown/Home/End scroll the history.
pub fn shortcut(ev: &KeyEvent, mac: bool) -> Option<Shortcut> {
    let m = ev.mods;
    let chord = |c: char| match &ev.key {
        Key::Command(k) if mac => m.logo && !m.ctrl && k.eq_ignore_ascii_case(&c),
        Key::Command(k) => m.ctrl && m.shift && !m.alt && k.eq_ignore_ascii_case(&c),
        Key::Text(s) if !mac => m.ctrl && m.shift && !m.alt && s.chars().count() == 1 && s.eq_ignore_ascii_case(&c.to_string()),
        _ => false,
    };
    let font = |c: char| match &ev.key {
        Key::Command(k) if mac => m.logo && *k == c,
        Key::Command(k) => m.ctrl && !m.alt && *k == c,
        _ => false,
    };
    if chord('c') {
        return Some(Shortcut::Copy);
    }
    if chord('v') || (!mac && ev.key == Key::Insert && m.shift && !m.ctrl) {
        return Some(Shortcut::Paste);
    }
    if chord('a') {
        return Some(Shortcut::SelectAll);
    }
    if font('=') || font('+') {
        return Some(Shortcut::FontBigger);
    }
    if font('-') {
        return Some(Shortcut::FontSmaller);
    }
    if font('0') {
        return Some(Shortcut::FontReset);
    }
    if m.shift && !m.ctrl && !m.alt {
        return match ev.key {
            Key::PageUp => Some(Shortcut::ScrollPage(-1)),
            Key::PageDown => Some(Shortcut::ScrollPage(1)),
            Key::Home => Some(Shortcut::ScrollTop),
            Key::End => Some(Shortcut::ScrollBottom),
            _ => None,
        };
    }
    None
}

/// What a paste sends: line breaks as Enter (`\r`), as xterm.js does, and
/// inside `ESC [200~ ... ESC [201~` when the program asked for bracketed paste
/// (with any end marker inside the text removed, so a paste cannot end the
/// bracket early and type the rest as keys).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text.into_bytes();
    }
    let inner = text.replace("\x1b[201~", "").replace("\x1b[200~", "");
    let mut out = b"\x1b[200~".to_vec();
    out.extend_from_slice(inner.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Key, KeyEvent, Mods};

    fn k(key: Key, f: impl FnOnce(&mut Mods)) -> KeyEvent {
        let mut mods = Mods::default();
        f(&mut mods);
        KeyEvent { key, mods }
    }
    fn none(_: &mut Mods) {}

    #[test]
    fn keys_become_xterm_bytes() {
        assert_eq!(key_bytes(&k(Key::Text("a".into()), none), false), Some(b"a".to_vec()));
        assert_eq!(key_bytes(&k(Key::Text("A".into()), |m| m.shift = true), false), Some(b"A".to_vec()));
        assert_eq!(key_bytes(&k(Key::Text("é".into()), none), false), Some("é".as_bytes().to_vec()));
        assert_eq!(key_bytes(&k(Key::Command('c'), |m| (m.ctrl, m.cmd) = (true, true)), false), Some(vec![3]));
        assert_eq!(key_bytes(&k(Key::Command('c'), |m| (m.logo, m.cmd) = (true, true)), false), None);
        assert_eq!(key_bytes(&k(Key::Enter, none), false), Some(b"\r".to_vec()));
        assert_eq!(key_bytes(&k(Key::Enter, |m| m.alt = true), false), Some(b"\x1b\r".to_vec()));
        assert_eq!(key_bytes(&k(Key::Up, none), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&k(Key::Up, none), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(&k(Key::Right, |m| m.ctrl = true), false), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(key_bytes(&k(Key::Left, |m| (m.shift, m.alt) = (true, true)), true), Some(b"\x1b[1;4D".to_vec()));
        assert_eq!(key_bytes(&k(Key::Tab, |m| m.shift = true), false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_bytes_with(&k(Key::Text("b".into()), |m| m.alt = true), false, LINUX), Some(b"\x1bb".to_vec()));
        assert_eq!(key_bytes(&k(Key::Delete, none), false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(key_bytes(&k(Key::PageUp, |m| m.ctrl = true), false), Some(b"\x1b[5;5~".to_vec()));
        assert_eq!(key_bytes(&k(Key::Function(1), none), false), Some(b"\x1bOP".to_vec()));
        assert_eq!(key_bytes(&k(Key::Function(5), none), false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_bytes(&k(Key::Function(12), |m| m.shift = true), false), Some(b"\x1b[24;2~".to_vec()));
        assert_eq!(key_bytes(&k(Key::Backspace, none), false), Some(vec![0x7f]));
        assert_eq!(key_bytes(&k(Key::Backspace, |m| m.ctrl = true), false), Some(vec![0x08]));
        assert_eq!(key_bytes(&k(Key::Text(" ".into()), |m| (m.ctrl, m.cmd) = (true, true)), false), Some(vec![0]));
        assert_eq!(key_bytes(&k(Key::Command('['), |m| (m.ctrl, m.cmd) = (true, true)), false), Some(vec![0x1b]));
        assert_eq!(key_bytes(&k(Key::Command('d'), |m| (m.ctrl, m.cmd, m.alt) = (true, true, true)), false), Some(vec![0x1b, 4]));
        assert_eq!(key_bytes(&k(Key::Text(" ".into()), |m| (m.cmd, m.logo) = (true, true)), false), None);
    }

    const MAC: KeyOpts = KeyOpts { mac: true, option_as_meta: false, reserved: &[] };
    const LINUX: KeyOpts = KeyOpts { mac: false, option_as_meta: false, reserved: &[] };

    #[test]
    fn composed_characters_are_text_not_meta() {
        // Swiss/French Mac: Option+7 is `|`, Option+Shift+L too on French; Option+G is `@`.
        let opt = |s: &str| k(Key::Text(s.into()), |m| m.alt = true);
        assert_eq!(key_bytes_with(&opt("|"), false, MAC), Some(b"|".to_vec()));
        assert_eq!(key_bytes_with(&opt("@"), false, MAC), Some(b"@".to_vec()));
        assert_eq!(key_bytes_with(&k(Key::Text("|".into()), |m| (m.alt, m.shift) = (true, true)), false, MAC), Some(b"|".to_vec()));
        // With Option as Meta, Rune delivers the plain key with alt: ESC prefix.
        let meta = KeyOpts { option_as_meta: true, ..MAC };
        assert_eq!(key_bytes_with(&opt("b"), false, meta), Some(b"\x1bb".to_vec()));
        // Windows AltGr (Ctrl+Alt, cmd set with ctrl): the character, not a control byte.
        let altgr = |s: &str| k(Key::Text(s.into()), |m| (m.ctrl, m.cmd, m.alt) = (true, true, true));
        assert_eq!(key_bytes_with(&altgr("@"), false, LINUX), Some(b"@".to_vec()));
        assert_eq!(key_bytes_with(&altgr("\\"), false, LINUX), Some(b"\\".to_vec()));
        // Linux AltGr sets no modifier: plain text.
        assert_eq!(key_bytes_with(&k(Key::Text("|".into()), none), false, LINUX), Some(b"|".to_vec()));
        // Plain Alt+letter off macOS is Meta.
        assert_eq!(key_bytes_with(&opt("b"), false, LINUX), Some(b"\x1bb".to_vec()));
        // Ctrl+letter on macOS arrives as text with ctrl: still a control byte.
        assert_eq!(key_bytes_with(&k(Key::Text("c".into()), |m| m.ctrl = true), false, MAC), Some(vec![3]));
        // A dead key's result is an input-method commit, sent as typed (TerminalState::on_ime).
    }

    #[test]
    fn shortcuts_per_platform() {
        let cmd = |c| k(Key::Command(c), |m| (m.cmd, m.logo) = (true, true));
        let ctrl_shift = |c| k(Key::Command(c), |m| (m.cmd, m.ctrl, m.shift) = (true, true, true));
        let ctrl = |c| k(Key::Command(c), |m| (m.cmd, m.ctrl) = (true, true));
        assert_eq!(shortcut(&cmd('c'), true), Some(Shortcut::Copy));
        assert_eq!(shortcut(&cmd('v'), true), Some(Shortcut::Paste));
        // Ctrl+C on macOS is the shell's, and so is plain Ctrl+C elsewhere.
        assert_eq!(shortcut(&ctrl('c'), true), None);
        assert_eq!(shortcut(&ctrl('c'), false), None);
        assert_eq!(shortcut(&ctrl_shift('c'), false), Some(Shortcut::Copy));
        // Shift may make the character upper case on some platforms.
        assert_eq!(shortcut(&ctrl_shift('V'), false), Some(Shortcut::Paste));
        assert_eq!(shortcut(&k(Key::Text("V".into()), |m| (m.ctrl, m.shift) = (true, true)), false), Some(Shortcut::Paste));
        assert_eq!(shortcut(&k(Key::Insert, |m| m.shift = true), false), Some(Shortcut::Paste));
        assert_eq!(shortcut(&cmd('c'), false), None);
        assert_eq!(shortcut(&k(Key::PageUp, |m| m.shift = true), false), Some(Shortcut::ScrollPage(-1)));
        assert_eq!(shortcut(&k(Key::PageDown, |m| m.shift = true), true), Some(Shortcut::ScrollPage(1)));
        assert_eq!(shortcut(&k(Key::PageUp, none), false), None);
        assert_eq!(shortcut(&k(Key::End, |m| m.shift = true), false), Some(Shortcut::ScrollBottom));
    }

    /// The shapes Rune's window host makes of real presses (rune
    /// src/window/keys.rs: `map_mods` sets `cmd` from Super on macOS and
    /// from Ctrl elsewhere, `logo` from Super; `map_key` lowers the letter of
    /// a `cmd` chord into `Key::Command`). On macOS ⌘ switches the layout to
    /// its Latin one, so ⌘C is `Command('c')` on French, Swiss and Russian
    /// layouts alike.
    #[test]
    fn real_event_shapes() {
        let mac_cmd = |c, shift| k(Key::Command(c), |m| (m.cmd, m.logo, m.shift) = (true, true, shift));
        assert_eq!(shortcut(&mac_cmd('c', false), true), Some(Shortcut::Copy));
        assert_eq!(shortcut(&mac_cmd('v', false), true), Some(Shortcut::Paste));
        assert_eq!(shortcut(&mac_cmd('a', false), true), Some(Shortcut::SelectAll));
        // ⇧⌘C copies too (Shift is not checked on macOS); no ⌘ chord reaches the program.
        assert_eq!(shortcut(&mac_cmd('c', true), true), Some(Shortcut::Copy));
        assert_eq!(key_bytes_with(&mac_cmd('c', true), false, MAC), None);
        assert_eq!(key_bytes_with(&mac_cmd('v', false), false, MAC), None);
        // ⌃C on macOS: `cmd` off, the letter as text with `ctrl`: the interrupt.
        let mac_ctrl = |c: &str| k(Key::Text(c.into()), |m| m.ctrl = true);
        assert_eq!(shortcut(&mac_ctrl("c"), true), None);
        assert_eq!(key_bytes_with(&mac_ctrl("c"), false, MAC), Some(vec![3]));
        assert_eq!(key_bytes_with(&mac_ctrl("v"), false, MAC), Some(vec![0x16]));
        // Linux and Windows: Ctrl is `cmd` and `ctrl`; Ctrl+Shift+C/V/A are the
        // app's, plain Ctrl+C and Ctrl+V the program's.
        let pc = |c, shift| k(Key::Command(c), |m| (m.cmd, m.ctrl, m.shift) = (true, true, shift));
        assert_eq!(shortcut(&pc('c', true), false), Some(Shortcut::Copy));
        assert_eq!(shortcut(&pc('v', true), false), Some(Shortcut::Paste));
        assert_eq!(shortcut(&pc('c', false), false), None);
        assert_eq!(shortcut(&pc('v', false), false), None);
        assert_eq!(key_bytes_with(&pc('c', false), false, LINUX), Some(vec![3]));
        assert_eq!(key_bytes_with(&pc('v', false), false, LINUX), Some(vec![0x16]));
    }

    #[test]
    fn paste_is_bracketed_and_safe() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc".to_vec());
        assert_eq!(paste_bytes("ls\n", true), b"\x1b[200~ls\r\x1b[201~".to_vec());
        assert_eq!(paste_bytes("x\x1b[201~rm -rf ~\n", true), b"\x1b[200~xrm -rf ~\r\x1b[201~".to_vec());
    }
}
