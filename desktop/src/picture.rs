//! Pictures from outside the app (a pasted screenshot, a dropped or picked
//! file, an image a feature read from the daemon): ONE place decodes them,
//! with hard limits, for every feature. Nothing here panics on hostile data:
//! a malformed, truncated, animated or oversized picture is an `Err` with a
//! readable reason.
//!
//! The limits apply before any pixel is decoded: the bytes' length, then the
//! dimensions read from the header, then the decoder's own allocation cap.
//! PNG, JPEG and WebP only (the crate is built with those three).

use image::{DynamicImage, ImageFormat, ImageReader};
use std::io::Cursor;

/// What a caller accepts. [`Limits::default`] is the core's ceiling; a
/// feature applies its own policy on top (fewer bytes, a smaller side).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Encoded bytes in.
    pub max_bytes: usize,
    /// Width and height, each.
    pub max_side: u32,
    /// Width times height.
    pub max_pixels: u64,
}

impl Default for Limits {
    /// 32 MB in, 16384 px a side, 40 megapixels (a 5K screenshot is 15).
    fn default() -> Limits {
        Limits { max_bytes: 32 << 20, max_side: 16_384, max_pixels: 40_000_000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encode {
    Png,
    /// Quality 1 to 100.
    Jpeg(u8),
    /// No bytes out, the pixels only (to show it).
    Pixels,
}

/// A decoded picture, as RGBA, and its re-encoded bytes.
#[derive(Clone, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Encoded as asked ([`Encode`]).
    pub bytes: Vec<u8>,
    /// `image/png` or `image/jpeg`.
    pub mime: &'static str,
    /// 8-bit RGBA, row by row (for the `image` element).
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Picture({}x{}, {} {} bytes)", self.width, self.height, self.mime, self.bytes.len())
    }
}

/// The format, from the first bytes only.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::WebP)
    } else {
        None
    }
}

/// Animated pictures are refused (a frame would be a guess): APNG has an
/// `acTL` chunk before its first `IDAT`, an animated WebP an `ANIM` chunk.
fn animated(fmt: ImageFormat, b: &[u8]) -> bool {
    match fmt {
        ImageFormat::Png => {
            let mut i = 8;
            while i + 8 <= b.len() {
                let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
                let kind = &b[i + 4..i + 8];
                if kind == b"acTL" {
                    return true;
                }
                if kind == b"IDAT" {
                    return false;
                }
                i = i.saturating_add(12).saturating_add(len);
            }
            false
        }
        ImageFormat::WebP => b.windows(4).take(4096).any(|w| w == b"ANIM"),
        _ => false,
    }
}

/// Decodes `bytes` within `limits`; `fit`: scales down (never up) so that
/// neither side exceeds it; then re-encodes as asked.
pub fn prepare(bytes: &[u8], limits: Limits, fit: Option<u32>, out: Encode) -> Result<Picture, String> {
    // The decoders should never panic; if one does on a hostile file, it is
    // still only a refused picture.
    std::panic::catch_unwind(|| prepare_inner(bytes, limits, fit, out)).unwrap_or_else(|_| Err("the picture could not be read (the decoder failed)".into()))
}

/// The checks [`prepare`] makes before decoding any pixel (length, format,
/// animation, the header's dimensions within `limits`), without decoding:
/// the width and height. For a picture sent on as it is (a pasted one,
/// uploaded to a sandbox).
pub fn check(bytes: &[u8], limits: Limits) -> Result<(u32, u32), String> {
    std::panic::catch_unwind(|| header(bytes, limits).map(|(_, w, h)| (w, h))).unwrap_or_else(|_| Err("the picture could not be read (the decoder failed)".into()))
}

fn header(bytes: &[u8], limits: Limits) -> Result<(ImageFormat, u32, u32), String> {
    if bytes.len() > limits.max_bytes {
        return Err(format!("the picture is {} MB, more than {} MB", bytes.len() >> 20, limits.max_bytes >> 20));
    }
    let fmt = sniff(bytes).ok_or("not a PNG, JPEG or WebP picture")?;
    if animated(fmt, bytes) {
        return Err("animated pictures are not taken".into());
    }
    // The header first: no pixel is decoded for a picture too large.
    let (w, h) = reader(bytes, fmt, limits).into_dimensions().map_err(|e| format!("the picture could not be read: {e}"))?;
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    if w > limits.max_side || h > limits.max_side || (w as u64) * (h as u64) > limits.max_pixels {
        return Err(format!("the picture is {w}x{h} pixels, more than allowed ({} a side, {} megapixels)", limits.max_side, limits.max_pixels / 1_000_000));
    }
    Ok((fmt, w, h))
}

fn reader(b: &[u8], fmt: ImageFormat, limits: Limits) -> ImageReader<Cursor<Vec<u8>>> {
    let mut r = ImageReader::with_format(Cursor::new(b.to_vec()), fmt);
    let mut l = image::Limits::default();
    l.max_image_width = Some(limits.max_side);
    l.max_image_height = Some(limits.max_side);
    l.max_alloc = Some(limits.max_pixels.saturating_mul(4).saturating_add(1 << 20));
    r.limits(l);
    r
}

fn prepare_inner(bytes: &[u8], limits: Limits, fit: Option<u32>, out: Encode) -> Result<Picture, String> {
    let (fmt, _, _) = header(bytes, limits)?;
    let mut img = reader(bytes, fmt, limits).decode().map_err(|e| format!("the picture could not be read: {e}"))?;
    if let Some(m) = fit.filter(|m| *m > 0) {
        if img.width() > m || img.height() > m {
            img = img.resize(m, m, image::imageops::FilterType::Triangle);
        }
    }
    encode(img, out)
}

/// The stream path (a surface's frames, [`crate::surface::set_encoded`]):
/// the same checks and limits as [`prepare`], then the RGBA pixels only (no
/// scaling, no re-encoding). Any thread.
pub fn decode_rgba(bytes: &[u8], limits: Limits) -> Result<(u32, u32, Vec<u8>), String> {
    let p = prepare(bytes, limits, None, Encode::Pixels)?;
    Ok((p.width, p.height, p.rgba))
}

fn encode(img: DynamicImage, out: Encode) -> Result<Picture, String> {
    let (width, height) = (img.width(), img.height());
    let rgba = img.to_rgba8().into_raw();
    let mut bytes = vec![];
    let mime = match out {
        Encode::Png => {
            img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png).map_err(|e| format!("could not encode PNG: {e}"))?;
            "image/png"
        }
        Encode::Jpeg(q) => {
            let rgb = img.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, q.clamp(1, 100)).encode_image(&rgb).map_err(|e| format!("could not encode JPEG: {e}"))?;
            "image/jpeg"
        }
        Encode::Pixels => "",
    };
    Ok(Picture { width, height, bytes, mime, rgba })
}

// ---------------------------------------------------------------------------
// Pictures shown by the `image` element

thread_local! {
    /// Registered by `Action::Image` (UI thread only): key, pixels, size.
    static SHOWN: std::cell::RefCell<std::collections::HashMap<String, (rune::Pixels, (u32, u32))>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// At most this many pictures are kept for `image` elements; the oldest
/// registered goes first.
pub const MAX_SHOWN: usize = 64;

thread_local! {
    static ORDER: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Registers a decoded picture under `key` for `image{ key = ... }`.
pub fn show(key: &str, p: &Picture) {
    let px = rune::Pixels::new();
    px.set(p.width, p.height, p.rgba.clone());
    SHOWN.with(|m| m.borrow_mut().insert(key.to_string(), (px, (p.width, p.height))));
    ORDER.with(|o| {
        let mut o = o.borrow_mut();
        o.retain(|k| k != key);
        o.push(key.to_string());
        while o.len() > MAX_SHOWN {
            let old = o.remove(0);
            SHOWN.with(|m| m.borrow_mut().remove(&old));
        }
    });
}

/// Forgets a registered picture.
pub fn forget(key: &str) {
    SHOWN.with(|m| m.borrow_mut().remove(key));
    ORDER.with(|o| o.borrow_mut().retain(|k| k != key));
}

/// Bytes of the registered pictures' pixels, and how many (MEMORY.md).
pub fn mem() -> (u64, usize) {
    SHOWN.with(|m| {
        let m = m.borrow();
        (m.values().map(|(_, (w, h))| *w as u64 * *h as u64 * 4).sum(), m.len())
    })
}

/// The picture registered under `key`, and its size.
pub fn shown(key: &str) -> Option<(rune::Pixels, (u32, u32))> {
    SHOWN.with(|m| m.borrow().get(key).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 255) as u8, (y % 255) as u8, 90, 255])));
        let mut b = vec![];
        img.write_to(&mut Cursor::new(&mut b), ImageFormat::Png).unwrap();
        b
    }

    /// A PNG whose header claims `w`x`h` with almost no data (a decompression bomb's shape).
    fn bomb(w: u32, h: u32) -> Vec<u8> {
        let mut b = png(1, 1);
        b[16..20].copy_from_slice(&w.to_be_bytes());
        b[20..24].copy_from_slice(&h.to_be_bytes());
        // A valid CRC over "IHDR" + its 13 bytes, so only the size is wrong.
        let crc = crc32(&b[12..29]);
        b[29..33].copy_from_slice(&crc.to_be_bytes());
        b
    }

    fn crc32(d: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &x in d {
            c ^= x as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    #[test]
    fn a_picture_is_decoded_scaled_and_reencoded() {
        let p = prepare(&png(800, 400), Limits::default(), Some(200), Encode::Png).unwrap();
        assert_eq!((p.width, p.height, p.mime), (200, 100, "image/png"));
        assert_eq!(p.rgba.len(), 200 * 100 * 4);
        assert_eq!(sniff(&p.bytes), Some(ImageFormat::Png));
        let j = prepare(&png(30, 20), Limits::default(), Some(200), Encode::Jpeg(80)).unwrap();
        assert_eq!((j.width, j.height, j.mime), (30, 20, "image/jpeg"), "never scaled up");
        assert_eq!(sniff(&j.bytes), Some(ImageFormat::Jpeg));
        // A JPEG in, PNG out.
        assert_eq!(prepare(&j.bytes, Limits::default(), None, Encode::Png).unwrap().width, 30);
    }

    #[test]
    fn hostile_pictures_are_refused_without_panicking() {
        // A header claiming 100000 x 100000 (40 GB of pixels): refused before decoding.
        // (the decoder's own limits, set from ours, say it first).
        let e = prepare(&bomb(100_000, 100_000), Limits::default(), None, Encode::Png).unwrap_err();
        assert!(e.contains("exceeds limit") || e.contains("more than"), "{e}");
        // Within the side limit but over the pixel count.
        let e = prepare(&bomb(10_000, 10_000), Limits::default(), None, Encode::Png).unwrap_err();
        assert!(e.contains("megapixels"), "{e}");
        // A legal size with the data missing: an error, not a panic.
        assert!(prepare(&bomb(4000, 4000), Limits::default(), None, Encode::Png).is_err());
        // Truncated at every length.
        let good = png(64, 64);
        // (a file cut inside its last, empty IEND chunk still has every pixel: not counted)
        for n in [0, 1, 8, 16, 24, 33, 50, good.len() / 2, good.len() - 13] {
            assert!(prepare(&good[..n], Limits::default(), None, Encode::Png).is_err(), "truncated at {n}");
        }
        // Too many bytes, the wrong format, garbage after a valid header.
        assert!(prepare(&good, Limits { max_bytes: 100, ..Limits::default() }, None, Encode::Png).unwrap_err().contains("MB"));
        assert!(prepare(b"GIF89a\x01\x00\x01\x00", Limits::default(), None, Encode::Png).unwrap_err().contains("not a PNG"));
        let mut junk = good[..33].to_vec();
        junk.extend(std::iter::repeat_n(0xAB, 5000));
        assert!(prepare(&junk, Limits::default(), None, Encode::Png).is_err());
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0x03];
        jpeg.extend(std::iter::repeat_n(0u8, 64));
        assert!(prepare(&jpeg, Limits::default(), None, Encode::Png).is_err());
        assert!(prepare(b"RIFF\x10\x00\x00\x00WEBPVP8 ", Limits::default(), None, Encode::Png).is_err());
        // A feature's own, smaller policy.
        assert!(prepare(&png(2000, 100), Limits { max_side: 1600, ..Limits::default() }, None, Encode::Png).is_err());
    }

    #[test]
    fn animated_pictures_are_refused() {
        let mut apng = png(2, 2);
        // An acTL chunk right after IHDR (8 + 25 bytes).
        let actl = [0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0];
        apng.splice(33..33, actl);
        assert!(prepare(&apng, Limits::default(), None, Encode::Png).unwrap_err().contains("animated"));
        assert!(prepare(b"RIFF\x20\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00\x02\x00\x00\x00ANIM", Limits::default(), None, Encode::Png).unwrap_err().contains("animated"));
    }
}
