//! The line protocol of `/ws/browser` (`crates/daemon/src/http/browser.rs`,
//! which carries `browser.mjs serve`'s lines both ways), as the web UI's
//! `assets/js/browser.js` speaks it. Pure: no socket, no UI, so every rule
//! here is tested on its own.
//!
//! **In** (the sandbox's word, never trusted): `frame {d, w, h}` (a JPEG in
//! base64), `url {url, title}`, `blocked {host, kind}`, `viewport {name}`,
//! `ready`, `note {msg}`, `error {msg}`, `closed` (Chromium went away),
//! `gone` (the pipe ended). Everything is checked and bounded here: a frame
//! is only ever pixels for the core's decoder; an address, a title, a host,
//! a note are only ever text, cut to a length, control characters removed.
//! Anything else is dropped.
//!
//! **Out**: `mouse`, `key`, `paste`, `nav {url}`, `back`, `forward`,
//! `reload`, `size {w, h}`, `viewport {value}`, `quit`: one JSON object on
//! one line, under the daemon's 256 KB.

use serde_json::{json, Value as J};

/// The daemon's own line limit; a frame line is never longer.
pub const MAX_LINE: usize = 8 << 20;
/// What the daemon takes from the app: longer is dropped there.
pub const MAX_OUT: usize = 256 << 10;
/// A frame's declared size, each side.
pub const MAX_SIDE: u32 = 4096;
/// A paste, in characters (`browser.mjs` cuts at 100,000 too).
pub const MAX_PASTE: usize = 100_000;
/// An address typed in the bar.
pub const MAX_URL: usize = 8192;
/// Text from the sandbox shown in the chrome (an address, a title, a note).
pub const MAX_TEXT: usize = 2048;
/// A host name.
pub const MAX_HOST: usize = 253;

/// One message from the sandbox's browser, checked.
#[derive(Debug, Clone, PartialEq)]
pub enum In {
    /// A picture: the JPEG's bytes, and the page's size in CSS pixels.
    Frame { jpeg: Vec<u8>, w: u32, h: u32 },
    Url { url: String, title: String },
    /// A host the network policy refused: `kind` `""` (may be allowed),
    /// `org` (organisation policy) or `rule` (a deny rule).
    Blocked { host: String, kind: Kind },
    Viewport(String),
    Ready,
    Note(String),
    Error(String),
    /// Chromium itself went away (asked from here, or by the agent).
    Closed,
    /// The pipe ended.
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Allowable,
    Org,
    Rule,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Allowable => "",
            Kind::Org => "org",
            Kind::Rule => "rule",
        }
    }
    /// Why it cannot be allowed from here, if it cannot.
    pub fn fixed(self) -> Option<&'static str> {
        match self {
            Kind::Allowable => None,
            Kind::Org => Some("organisation policy, cannot be allowed"),
            Kind::Rule => Some("a deny rule, which an allow does not override"),
        }
    }
}

/// Is this line a frame? Cheap, for the socket's I/O thread (a late handler
/// keeps only the newest frame): `browser.mjs` writes `{"t":"frame",` first;
/// a writer that sorts its keys puts `"t"` last, after the picture, so the
/// end is looked at too. Never the whole line.
pub fn is_frame(line: &str) -> bool {
    let b = line.as_bytes();
    let near = |s: &[u8]| s.windows(11).any(|w| w == br#""t":"frame""#);
    near(&b[..b.len().min(48)]) || near(&b[b.len().saturating_sub(48)..])
}

/// Text from the sandbox, made safe to show: no control characters, one
/// line, at most `max` characters.
pub fn clean(s: &str, max: usize) -> String {
    let mut out: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).filter(|c| !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')).take(max).collect();
    out.truncate(out.trim_end().len());
    out
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= MAX_HOST && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_' | '*'))
}

/// One line from the socket. `None`: not one of the bridge's messages, or
/// malformed (dropped, as the web UI does).
pub fn parse(line: &str) -> Option<In> {
    if line.len() > MAX_LINE {
        return None;
    }
    let m: J = serde_json::from_str(line).ok()?;
    let s = |k: &str| m.get(k).and_then(J::as_str).unwrap_or("");
    Some(match m.get("t")?.as_str()? {
        "frame" => {
            let d = m.get("d")?.as_str()?;
            let jpeg = b64_decode(d)?;
            // The bridge sends JPEG only (`Page.startScreencast`, format jpeg).
            if !jpeg.starts_with(&[0xFF, 0xD8, 0xFF]) {
                return None;
            }
            let side = |k: &str| m.get(k).and_then(J::as_f64).filter(|v| v.is_finite() && *v >= 1.0 && *v <= MAX_SIDE as f64).map(|v| v.round() as u32);
            In::Frame { jpeg, w: side("w")?, h: side("h")? }
        }
        "url" => In::Url { url: clean(s("url"), MAX_TEXT), title: clean(s("title"), MAX_TEXT) },
        "blocked" => {
            let host = s("host").trim().to_ascii_lowercase();
            if !host_ok(&host) {
                return None;
            }
            let kind = match s("kind") {
                "" => Kind::Allowable,
                "org" => Kind::Org,
                // Anything else is not allowable from here.
                _ => Kind::Rule,
            };
            In::Blocked { host, kind }
        }
        "viewport" => In::Viewport({
            let n = clean(s("name"), 40);
            if n.is_empty() { "auto".into() } else { n }
        }),
        "ready" => In::Ready,
        "note" => In::Note(clean(s("msg"), MAX_TEXT)),
        "error" => In::Error({
            let e = clean(s("msg"), MAX_TEXT);
            if e.is_empty() { "The browser could not start.".into() } else { e }
        }),
        "closed" => In::Closed,
        "gone" => In::Gone,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Out

/// DOM modifier names, as `browser.js`'s bitmask: alt 1, ctrl 2, meta 4, shift 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
}

impl Mods {
    pub fn bits(self) -> u8 {
        (self.alt as u8) | (self.ctrl as u8) << 1 | (self.meta as u8) << 2 | (self.shift as u8) << 3
    }
    fn command(self) -> bool {
        self.ctrl || self.meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    Moved,
    Pressed,
    Released,
}

fn line(v: J) -> String {
    // serde_json escapes control characters: a value never breaks the line.
    v.to_string()
}

fn coord(v: f32) -> i64 {
    if v.is_finite() { v.round().clamp(0.0, 20000.0) as i64 } else { 0 }
}

/// A pointer event, in the picture's pixels. `button`: `left`, `middle`,
/// `right`, or `none` for a move with nothing held.
pub fn mouse(kind: Mouse, x: f32, y: f32, button: &str, clicks: u32, mods: Mods) -> String {
    let button = if ["left", "middle", "right"].contains(&button) { button } else { "none" };
    let ty = match kind {
        Mouse::Moved => "mouseMoved",
        Mouse::Pressed => "mousePressed",
        Mouse::Released => "mouseReleased",
    };
    let mut v = json!({ "t": "mouse", "type": ty, "x": coord(x), "y": coord(y), "button": button, "modifiers": mods.bits() });
    if kind != Mouse::Moved {
        v["clickCount"] = clicks.clamp(1, 3).into();
    }
    line(v)
}

pub fn wheel(x: f32, y: f32, dx: f32, dy: f32, mods: Mods) -> String {
    let d = |v: f32| if v.is_finite() { (v.clamp(-10000.0, 10000.0) * 100.0).round() / 100.0 } else { 0.0 };
    line(json!({ "t": "mouse", "type": "mouseWheel", "x": coord(x), "y": coord(y), "deltaX": d(dx), "deltaY": d(dy), "modifiers": mods.bits() }))
}

/// Keys the app keeps, never sent to the page (as `browser.js`'s
/// `browserKeptKey`, plus the core's own): F6 (the address bar), Cmd/Ctrl
/// with `+ - = 0` (the app's zoom), and Cmd/Ctrl+V (the paste arrives on its
/// own, with the text). The core keeps its shortcuts and the release key
/// before this; this is the feature's own check, so a key the core let
/// through by mistake still does not reach the page.
pub fn kept(key: &str, mods: Mods) -> bool {
    key == "F6" || (mods.command() && ["+", "-", "=", "0"].contains(&key)) || (mods.command() && key.eq_ignore_ascii_case("v"))
}

/// A key down or up, as `browser.js` sends it; `None` when the key is kept.
/// `text` is what the key typed (none with Ctrl, Cmd or Alt held, as the
/// web UI); Enter and Tab type `\r` and `\t`.
pub fn key(down: bool, key: &str, code: Option<&str>, text: Option<&str>, mods: Mods) -> Option<String> {
    if kept(key, mods) || key.is_empty() {
        return None;
    }
    let key: String = key.chars().take(32).collect();
    let code = code.unwrap_or("");
    let code: String = code.chars().take(32).collect();
    let vk = vk(&key, &code);
    if !down {
        return Some(line(json!({ "t": "key", "type": "keyUp", "key": key, "code": code, "vk": vk, "modifiers": mods.bits() })));
    }
    let plain = !mods.ctrl && !mods.meta && !mods.alt;
    let typed: Option<String> = if !plain {
        None
    } else if key == "Enter" {
        Some("\r".into())
    } else if key == "Tab" {
        Some("\t".into())
    } else if key.chars().count() == 1 {
        Some(key.clone())
    } else {
        text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)).map(|t| t.chars().take(8).collect())
    };
    let mut v = json!({ "t": "key", "type": if typed.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key, "code": code, "vk": vk, "modifiers": mods.bits() });
    if let Some(t) = typed {
        v["text"] = t.into();
    }
    Some(line(v))
}

/// Text the person pasted, or an input method committed. `None` when empty.
pub fn paste(text: &str) -> Option<String> {
    let t: String = text.chars().take(MAX_PASTE).collect();
    if t.is_empty() {
        return None;
    }
    let l = line(json!({ "t": "paste", "text": t }));
    // A paste of multi-byte characters can be longer in bytes: cut until it fits.
    if l.len() <= MAX_OUT {
        return Some(l);
    }
    let mut n = t.chars().count();
    loop {
        n = n * 3 / 4;
        let l = line(json!({ "t": "paste", "text": t.chars().take(n).collect::<String>() }));
        if l.len() <= MAX_OUT || n == 0 {
            return (n > 0).then_some(l);
        }
    }
}

/// An address typed in the bar: sent to the sandbox's browser as text,
/// never opened here. The bridge itself takes only http(s) (`cleanUrl`).
pub fn nav(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() || u.len() > MAX_URL || u.chars().any(char::is_control) {
        return None;
    }
    Some(line(json!({ "t": "nav", "url": u })))
}

pub fn simple(t: &str) -> String {
    debug_assert!(["back", "forward", "reload", "quit"].contains(&t));
    line(json!({ "t": t }))
}

/// The viewports `browser.js` offers, and the one the agent chose.
pub const VIEWPORTS: &[(&str, &str)] = &[("auto", "Auto"), ("mobile", "Mobile · 390"), ("tablet", "Tablet · 820"), ("desktop", "Desktop · 1280")];

pub fn viewport(value: &str) -> Option<String> {
    VIEWPORTS.iter().any(|(v, _)| *v == value).then(|| line(json!({ "t": "viewport", "value": value })))
}

/// Narrower than this and many sites switch to their phone layout: a narrow
/// panel shows the desktop page scaled down (`browserFit`).
pub const MIN_LAYOUT_W: f32 = 1024.0;

/// The size the page is laid out at, for a shown area of `w` x `h` points.
pub fn size(w: f32, h: f32) -> Option<String> {
    if !(w >= 50.0 && h >= 50.0) || !w.is_finite() || !h.is_finite() {
        return None;
    }
    let lw = w.max(MIN_LAYOUT_W);
    let lh = h * (lw / w);
    Some(line(json!({ "t": "size", "w": lw.min(1600.0).round() as u32, "h": lh.clamp(320.0, 1600.0).round() as u32 })))
}

/// Windows virtual key code (`KeyboardEvent.keyCode`), which CDP needs for
/// keys that type nothing (arrows, Backspace): from the physical code, else
/// the key name.
pub fn vk(key: &str, code: &str) -> u32 {
    if let Some(c) = code.strip_prefix("Key").filter(|c| c.len() == 1) {
        return c.as_bytes()[0].to_ascii_uppercase() as u32;
    }
    if let Some(d) = code.strip_prefix("Digit").filter(|c| c.len() == 1) {
        return d.as_bytes()[0] as u32;
    }
    if let Some(n) = code.strip_prefix("Numpad").and_then(|n| n.parse::<u32>().ok()).filter(|n| *n <= 9) {
        return 0x60 + n;
    }
    let named = |k: &str| -> Option<u32> {
        Some(match k {
            "Backspace" => 8,
            "Tab" => 9,
            "Enter" | "NumpadEnter" => 13,
            "Shift" | "ShiftLeft" | "ShiftRight" => 16,
            "Control" | "ControlLeft" | "ControlRight" => 17,
            "Alt" | "AltLeft" | "AltRight" => 18,
            "Pause" => 19,
            "CapsLock" => 20,
            "Escape" => 27,
            " " | "Space" => 32,
            "PageUp" => 33,
            "PageDown" => 34,
            "End" => 35,
            "Home" => 36,
            "ArrowLeft" => 37,
            "ArrowUp" => 38,
            "ArrowRight" => 39,
            "ArrowDown" => 40,
            "Insert" => 45,
            "Delete" => 46,
            "Meta" | "MetaLeft" | "MetaRight" | "OSLeft" | "OSRight" => 91,
            "ContextMenu" => 93,
            ";" | "Semicolon" => 186,
            "=" | "Equal" => 187,
            "," | "Comma" => 188,
            "-" | "Minus" => 189,
            "." | "Period" => 190,
            "/" | "Slash" => 191,
            "`" | "Backquote" => 192,
            "[" | "BracketLeft" => 219,
            "\\" | "Backslash" => 220,
            "]" | "BracketRight" => 221,
            "'" | "Quote" => 222,
            _ => {
                let f = k.strip_prefix('F')?.parse::<u32>().ok().filter(|n| (1..=24).contains(n))?;
                111 + f
            }
        })
    };
    if let Some(v) = named(code) {
        return v;
    }
    if let Some(v) = named(key) {
        return v;
    }
    match key.chars().next() {
        Some(c) if key.chars().count() == 1 && c.is_ascii_alphabetic() => c.to_ascii_uppercase() as u32,
        Some(c) if key.chars().count() == 1 && c.is_ascii_digit() => c as u32,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Base64 (RFC 4648, standard alphabet, as `browser.js` checks it)

fn b64_val(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// Strict: the standard alphabet, padding only at the end, a length that is
/// a multiple of 4. Anything else is refused.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.is_empty() || b.len() % 4 != 0 {
        return None;
    }
    let pad = b.iter().rev().take_while(|c| **c == b'=').count();
    if pad > 2 {
        return None;
    }
    let body = &b[..b.len() - pad];
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in body {
        acc = (acc << 6) | b64_val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // Leftover bits must be zero (a canonical encoding).
    if acc != 0 {
        return None;
    }
    Some(out)
}

pub fn b64_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for ch in b.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if ch.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if ch.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}
