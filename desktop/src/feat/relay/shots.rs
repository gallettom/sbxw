//! Images attached to a screenshot request, held by the core until the
//! person releases them in the core's dialog. A screen sees only their
//! labels, sizes and weights, never their bytes.
//!
//! The policy is the web UI's (`assets/js/relay.js`) and the daemon's
//! (`crates/core/src/relay.rs`): at most [`MAX_SHOTS`] images, each at most
//! [`MAX_B64`] of base64 and the set at most [`MAX_TOTAL_B64`], PNG, JPEG or
//! WebP, the longest edge scaled to [`MAX_EDGE`].
//!
//! Decoding and scaling belong to the core's image helper (desktop-integ);
//! until it is in, [`prepare`] only reads the size from the file's header and
//! passes the bytes on unchanged when they already fit. It is the one
//! function to swap.

use serde_json::{json, Value as J};

pub const MAX_SHOTS: usize = 6;
pub const MAX_B64: usize = 6 * 1024 * 1024;
pub const MAX_TOTAL_B64: usize = 16 * 1024 * 1024;
pub const MAX_EDGE: u32 = 1600;
/// A file read from a drop or the file dialog larger than this is not read at all.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    /// Where it came from: `from clipboard`, a file's name.
    pub source: String,
    pub mime: &'static str,
    pub w: u32,
    pub h: u32,
    pub b64: String,
}

impl Shot {
    /// `data:image/png;base64,...`, what the daemon takes.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.b64)
    }
    /// The bytes sent, about (base64 is 4/3 of them).
    pub fn bytes(&self) -> usize {
        self.b64.len() / 4 * 3
    }
    pub fn label(&self) -> String {
        format!("{} · {}×{} · {}", self.source, self.w, self.h, weight(self.bytes()))
    }
    pub fn to_json(&self, n: usize) -> J {
        json!({ "n": n, "source": self.source, "w": self.w, "h": self.h, "kb": self.bytes().div_ceil(1024), "label": self.label() })
    }
}

pub fn weight(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// Type and size from the header: PNG, JPEG or WebP. Nothing else is read.
pub fn sniff(b: &[u8]) -> Option<(&'static str, u32, u32)> {
    let be32 = |i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    let be16 = |i: usize| b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]) as u32);
    let le16 = |i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u32);
    let le24 = |i: usize| b.get(i..i + 3).map(|s| s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16);
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(b"IHDR") {
        return Some(("image/png", be32(16)?, be32(20)?));
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        // Walk the segments to a start-of-frame.
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                return None;
            }
            let m = b[i + 1];
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            let len = be16(i + 2)? as usize;
            if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                return Some(("image/jpeg", be16(i + 7)?, be16(i + 5)?));
            }
            if len < 2 {
                return None;
            }
            i += 2 + len;
        }
        return None;
    }
    if b.get(0..4) == Some(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8X" => Some(("image/webp", le24(24)? + 1, le24(27)? + 1)),
            b"VP8L" => {
                let v = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(("image/webp", (v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1))
            }
            b"VP8 " => Some(("image/webp", le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            _ => None,
        };
    }
    None
}

/// The image as it will be sent, or why not. Swap for the core's helper
/// (decode with limits, scale to [`MAX_EDGE`], re-encode PNG, else JPEG).
pub fn prepare(bytes: &[u8], source: &str) -> Result<Shot, String> {
    let (mime, w, h) = sniff(bytes).ok_or_else(|| format!("{source}: not a PNG, JPEG or WebP image"))?;
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(format!("{source}: that image's size ({w}×{h}) is not one sbxw sends"));
    }
    if w.max(h) > MAX_EDGE {
        return Err(format!("{source}: {w}×{h} is larger than {MAX_EDGE} pixels and this build cannot scale it down yet: capture a window rather than the whole screen"));
    }
    let b64 = base64(bytes);
    if b64.len() > MAX_B64 {
        return Err(format!("{source}: that image is too large to send ({}): try a single window", weight(bytes.len())));
    }
    Ok(Shot { source: source.to_string(), mime, w, h, b64 })
}

/// Adds `shot` to `held` if the set stays within the limits.
pub fn admit(held: &mut Vec<Shot>, shot: Shot) -> Result<(), String> {
    if held.len() >= MAX_SHOTS {
        return Err(format!("{MAX_SHOTS} images is the most one reply carries: remove one first"));
    }
    let total: usize = held.iter().map(|s| s.b64.len()).sum::<usize>() + shot.b64.len();
    if total > MAX_TOTAL_B64 {
        return Err("that is more than one reply carries: remove an image first".into());
    }
    held.push(shot);
    Ok(())
}

pub fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A valid 1×1 PNG (any size can be faked in the header for the size rules).
    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[test]
    fn headers_give_type_and_size() {
        assert_eq!(sniff(&png(800, 600)), Some(("image/png", 800, 600)));
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 0x01, 0x2C, 0x01, 0x90, 3, 0, 0];
        assert_eq!(sniff(&jpeg), Some(("image/jpeg", 400, 300)));
        assert_eq!(sniff(b"GIF89a.........."), None);
        assert_eq!(sniff(b"\x89PNG"), None);
    }

    #[test]
    fn limits_are_the_daemons() {
        assert!(prepare(b"not an image", "x").is_err());
        assert!(prepare(&png(3000, 2000), "big.png").unwrap_err().contains("1600"));
        let s = prepare(&png(800, 600), "from clipboard").unwrap();
        assert!(s.data_url().starts_with("data:image/png;base64,iVBORw0KGgo"));
        let mut held = vec![];
        for _ in 0..MAX_SHOTS {
            admit(&mut held, s.clone()).unwrap();
        }
        assert!(admit(&mut held, s.clone()).unwrap_err().contains("6 images"));
        let heavy = Shot { b64: "A".repeat(MAX_B64), ..s.clone() };
        let mut held = vec![heavy.clone(), heavy.clone()];
        assert!(admit(&mut held, heavy).unwrap_err().contains("more than one reply"));
    }

    #[test]
    fn base64_is_standard() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b""), "");
    }
}
