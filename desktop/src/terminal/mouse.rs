//! Mouse reports for programs that ask for them (DECSET 1000, 1002, 1003
//! with the 1006 SGR or 1005 UTF-8 encodings, or the X10 default), as xterm
//! sends them.

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// No button held (motion under 1003).
    None,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Whether the program asked for any mouse reports.
pub fn reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// The bytes that report `action` of `button` at cell (`col`, `row`), 0-based,
/// or `None` when the current mode does not report it.
pub fn report(mode: TermMode, button: Button, action: Action, col: usize, row: usize, mods: Mods) -> Option<Vec<u8>> {
    if !reporting(mode) {
        return None;
    }
    let motion = action == Action::Motion;
    if motion {
        let held = button != Button::None;
        let wanted = mode.contains(TermMode::MOUSE_MOTION) || (held && mode.contains(TermMode::MOUSE_DRAG));
        if !wanted {
            return None;
        }
    }
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code: u32 = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    // Without SGR, a release does not say which button.
    if action == Action::Release && !sgr {
        if matches!(button, Button::WheelUp | Button::WheelDown) {
            return None;
        }
        code = 3;
    }
    if motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    if sgr {
        let end = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    let mut out = b"\x1b[M".to_vec();
    out.push((32 + code) as u8);
    if mode.contains(TermMode::UTF8_MOUSE) {
        for v in [x, y] {
            let c = char::from_u32(32 + v).filter(|_| v <= 2015)?;
            let mut b = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        }
    } else {
        // X10 bytes stop at 255: 223 columns.
        if x > 223 || y > 223 {
            return None;
        }
        out.push((32 + x) as u8);
        out.push((32 + y) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false };

    #[test]
    fn sgr_reports() {
        let m = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(report(m, Button::Left, Action::Press, 0, 0, NONE), Some(b"\x1b[<0;1;1M".to_vec()));
        assert_eq!(report(m, Button::Left, Action::Release, 9, 4, NONE), Some(b"\x1b[<0;10;5m".to_vec()));
        assert_eq!(report(m, Button::Right, Action::Press, 300, 2, NONE), Some(b"\x1b[<2;301;3M".to_vec()));
        assert_eq!(report(m, Button::WheelUp, Action::Press, 1, 1, NONE), Some(b"\x1b[<64;2;2M".to_vec()));
        let ctrl = Mods { ctrl: true, ..NONE };
        assert_eq!(report(m, Button::WheelDown, Action::Press, 1, 1, ctrl), Some(b"\x1b[<81;2;2M".to_vec()));
        // 1000 alone reports no motion; 1002 reports it while a button is held; 1003 always.
        assert_eq!(report(m, Button::Left, Action::Motion, 3, 3, NONE), None);
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert_eq!(report(drag, Button::Left, Action::Motion, 3, 3, NONE), Some(b"\x1b[<32;4;4M".to_vec()));
        assert_eq!(report(drag, Button::None, Action::Motion, 3, 3, NONE), None);
        let any = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(report(any, Button::None, Action::Motion, 3, 3, NONE), Some(b"\x1b[<35;4;4M".to_vec()));
    }

    #[test]
    fn x10_and_utf8_reports() {
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(report(m, Button::Left, Action::Press, 0, 0, NONE), Some(b"\x1b[M\x20\x21\x21".to_vec()));
        assert_eq!(report(m, Button::Middle, Action::Release, 0, 0, NONE), Some(b"\x1b[M\x23\x21\x21".to_vec()));
        let shift = Mods { shift: true, ..NONE };
        assert_eq!(report(m, Button::Left, Action::Press, 2, 1, shift), Some(b"\x1b[M\x24\x23\x22".to_vec()));
        assert_eq!(report(m, Button::Left, Action::Press, 250, 0, NONE), None);
        let u = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
        assert_eq!(report(u, Button::Left, Action::Press, 250, 0, NONE), Some("\x1b[M\x20\u{11b}\x21".as_bytes().to_vec()));
        assert_eq!(report(TermMode::empty(), Button::Left, Action::Press, 0, 0, NONE), None);
    }
}
