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
    /// Its preview's key for the `image` element (`relay-shot-N`).
    pub key: String,
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
        json!({ "n": n, "source": self.source, "w": self.w, "h": self.h, "kb": self.bytes().div_ceil(1024), "label": self.label(), "image": self.key })
    }
}

pub fn weight(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// The picture as it will be sent, or why not: decoded by the core's one
/// helper ([`crate::picture`], its limits), the longest edge scaled to
/// [`MAX_EDGE`], PNG (flat colour and text are smaller and sharper), else
/// JPEG when the PNG passes the per-image cap (a photograph, gradients).
/// Also the PNG of the preview the panel shows.
pub fn prepare(bytes: &[u8], source: &str) -> Result<(Shot, Vec<u8>), String> {
    use crate::picture::{self, Encode, Limits};
    let png = picture::prepare(bytes, Limits::default(), Some(MAX_EDGE), Encode::Png).map_err(|e| format!("{source}: {e}"))?;
    let preview = png.bytes.clone();
    let mut shot = Shot { source: source.to_string(), mime: png.mime, w: png.width, h: png.height, b64: base64(&png.bytes), key: String::new() };
    if shot.b64.len() > MAX_B64 {
        let jpeg = picture::prepare(bytes, Limits::default(), Some(MAX_EDGE), Encode::Jpeg(85)).map_err(|e| format!("{source}: {e}"))?;
        shot = Shot { mime: jpeg.mime, w: jpeg.width, h: jpeg.height, b64: base64(&jpeg.bytes), ..shot };
    }
    if shot.b64.len() > MAX_B64 {
        return Err(format!("{source}: that image is too large to send ({}): try a single window", weight(shot.bytes())));
    }
    Ok((shot, preview))
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

    /// A real PNG of that size (flat colour).
    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([30, 120, 200, 255]));
        let mut out = std::io::Cursor::new(vec![]);
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn pictures_are_scaled_and_checked_by_the_cores_helper() {
        assert!(prepare(b"not an image", "notes.txt").unwrap_err().starts_with("notes.txt: "));
        let (s, preview) = prepare(&png(3200, 2000), "big.png").unwrap();
        assert_eq!((s.w, s.h, s.mime), (1600, 1000, "image/png"), "the longest edge is 1600");
        assert!(s.data_url().starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert!(preview.starts_with(b"\x89PNG"));
        let (small, _) = prepare(&png(800, 600), "from clipboard").unwrap();
        assert_eq!((small.w, small.h), (800, 600), "never scaled up");
    }

    #[test]
    fn a_set_keeps_to_the_daemons_limits() {
        let (s, _) = prepare(&png(80, 60), "x").unwrap();
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
