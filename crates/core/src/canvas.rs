//! Canvas: one review session of one sandbox. The agent adds shots of the
//! screens it works on (in rounds); the person places pins, notes and assets
//! on an infinite surface and sends them back as a round of remarks, which
//! the agent reads with `canvas_read`.
//!
//! This module is pure: the manifest, the limits, the checks of what comes
//! in (picture headers, texts), the layout of new shots and what the agent
//! is handed. The daemon (`svc::canvas`) keeps one directory per canvas on
//! the host's disk and serves it.
//!
//! Every text from an agent or a person is data: clipped, control
//! characters removed, never a path. File names are generated here, never
//! taken from outside.

use serde::{Deserialize, Serialize};

/// The manifest's format; older manifests (fewer fields) load with defaults.
pub const VERSION: u32 = 1;
/// One picture: encoded bytes.
pub const MAX_IMAGE_BYTES: usize = 20 << 20;
/// One picture: pixels a side.
pub const MAX_SIDE: u32 = 8192;
/// Shots one agent round may add.
pub const MAX_SHOTS_PER_ROUND: usize = 24;
/// What the guidance suggests per round.
pub const SUGGESTED_SHOTS_PER_ROUND: usize = 12;
/// Pictures of one canvas, in all (shots and assets).
pub const MAX_CANVAS_BYTES: u64 = 500 << 20;
/// Characters of a shot's title (the agent's), and of a canvas title.
pub const MAX_TITLE: usize = 120;
/// Characters of a shot's note (the agent's), and of one remark (the person's).
pub const MAX_NOTE: usize = 2000;
/// Characters of a screen key (`settings`, `home-mobile`).
pub const MAX_SCREEN: usize = 60;
/// Characters of an asset's file name as shown.
pub const MAX_NAME: usize = 120;
/// Annotations a canvas may hold, each kind.
pub const MAX_PINS: usize = 500;
pub const MAX_NOTES: usize = 300;
pub const MAX_ASSETS: usize = 200;
/// World points between two rounds' rows, and between two shots of a row.
pub const ROW_GAP: f32 = 240.0;
pub const COL_GAP: f32 = 120.0;
/// A note card's default size (world points).
pub const NOTE_W: f32 = 240.0;
pub const NOTE_H: f32 = 140.0;
/// A picture's longest side on the surface, when it says no width.
pub const DEFAULT_ASSET_W: f32 = 480.0;

// ---------------------------------------------------------------------------
// Pictures

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
        }
    }
    pub fn mime(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
            Format::Webp => "image/webp",
        }
    }
    /// From a generated file name's extension (what was stored).
    pub fn of_file(name: &str) -> Option<Format> {
        match name.rsplit('.').next()? {
            "png" => Some(Format::Png),
            "jpg" => Some(Format::Jpeg),
            "webp" => Some(Format::Webp),
            _ => None,
        }
    }
}

/// What the header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    pub width: u32,
    pub height: u32,
}

/// Checks a picture by its bytes only (never its name): PNG, JPEG or WebP,
/// its size read from the header, within [`MAX_IMAGE_BYTES`] and
/// [`MAX_SIDE`]; animated pictures refused. No pixel is decoded here.
pub fn sniff(b: &[u8]) -> Result<Header, String> {
    if b.len() > MAX_IMAGE_BYTES {
        return Err(format!("the picture is {} MB, more than {} MB", b.len() >> 20, MAX_IMAGE_BYTES >> 20));
    }
    let h = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(b)?
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        jpeg(b)?
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        webp(b)?
    } else {
        return Err("not a PNG, JPEG or WebP picture".into());
    };
    if h.width == 0 || h.height == 0 {
        return Err("the picture is empty".into());
    }
    if h.width > MAX_SIDE || h.height > MAX_SIDE {
        return Err(format!("the picture is {}x{} pixels, more than {MAX_SIDE} a side", h.width, h.height));
    }
    Ok(h)
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn png(b: &[u8]) -> Result<Header, String> {
    let bad = || "a PNG whose header cannot be read".to_string();
    if b.get(12..16) != Some(b"IHDR") {
        return Err(bad());
    }
    let (w, h) = (be32(b, 16).ok_or_else(bad)?, be32(b, 20).ok_or_else(bad)?);
    // APNG: an acTL chunk before the first IDAT.
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = be32(b, i).ok_or_else(bad)? as usize;
        let kind = &b[i + 4..i + 8];
        if kind == b"acTL" {
            return Err("animated pictures are not taken".into());
        }
        if kind == b"IDAT" || kind == b"IEND" {
            break;
        }
        i = i.saturating_add(12).saturating_add(len);
    }
    Ok(Header { format: Format::Png, width: w, height: h })
}

fn jpeg(b: &[u8]) -> Result<Header, String> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        // Markers without a length.
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += 2;
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let sof = (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC);
        if sof {
            let h = b.get(i + 5..i + 7).map(|s| u16::from_be_bytes([s[0], s[1]]) as u32);
            let w = b.get(i + 7..i + 9).map(|s| u16::from_be_bytes([s[0], s[1]]) as u32);
            if let (Some(w), Some(h)) = (w, h) {
                return Ok(Header { format: Format::Jpeg, width: w, height: h });
            }
            break;
        }
        if len < 2 {
            break;
        }
        i += 2 + len;
    }
    Err("a JPEG whose size cannot be read".into())
}

fn webp(b: &[u8]) -> Result<Header, String> {
    let bad = || "a WebP whose header cannot be read".to_string();
    let chunk = b.get(12..16).ok_or_else(bad)?;
    let le24 = |i: usize| -> Option<u32> {
        let s = b.get(i..i + 3)?;
        Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
    };
    match chunk {
        b"VP8 " => {
            // A key frame: 3 bytes of tag, the start code, then 14-bit sizes.
            if b.get(23..26) != Some(&[0x9d, 0x01, 0x2a]) {
                return Err(bad());
            }
            let s = b.get(26..30).ok_or_else(bad)?;
            let w = u16::from_le_bytes([s[0], s[1]]) as u32 & 0x3fff;
            let h = u16::from_le_bytes([s[2], s[3]]) as u32 & 0x3fff;
            Ok(Header { format: Format::Webp, width: w, height: h })
        }
        b"VP8L" => {
            if b.get(20) != Some(&0x2f) {
                return Err(bad());
            }
            let s = b.get(21..25).ok_or_else(bad)?;
            let v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            Ok(Header { format: Format::Webp, width: (v & 0x3fff) + 1, height: ((v >> 14) & 0x3fff) + 1 })
        }
        b"VP8X" => {
            let flags = *b.get(20).ok_or_else(bad)?;
            if flags & 0x02 != 0 {
                return Err("animated pictures are not taken".into());
            }
            if b.len() < 30 {
                return Err(bad());
            }
            let w = le24(24).ok_or_else(bad)? + 1;
            let h = le24(27).ok_or_else(bad)? + 1;
            Ok(Header { format: Format::Webp, width: w, height: h })
        }
        _ => Err(bad()),
    }
}

/// Standard base64 (padding optional, no white space but new lines) to
/// bytes, refusing anything over `max` decoded bytes before decoding it.
pub fn unbase64(s: &str, max: usize) -> Result<Vec<u8>, String> {
    let s: Vec<u8> = s.bytes().filter(|b| *b != b'\n' && *b != b'\r').collect();
    let s = s.strip_suffix(b"==").or_else(|| s.strip_suffix(b"=")).unwrap_or(&s);
    if s.len() / 4 * 3 > max + 3 {
        return Err(format!("the picture is more than {} MB", max >> 20));
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    for chunk in s.chunks(4) {
        if chunk.len() == 1 {
            return Err("not base64".into());
        }
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            n |= val(*c).ok_or("not base64")? << (18 - 6 * k);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len() - 1]);
    }
    if out.len() > max {
        return Err(format!("the picture is more than {} MB", max >> 20));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Texts and ids

/// Agent or person text as data: control characters (but new lines when
/// `lines`) and bidirectional overrides removed, trimmed, cut to `max` characters.
pub fn clean(s: &str, max: usize, lines: bool) -> String {
    let t: String = s
        .chars()
        .filter(|c| !(c.is_control() && !(lines && *c == '\n')))
        .filter(|c| !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}'))
        .collect();
    t.trim().chars().take(max).collect()
}

/// A screen key: lower-case letters, digits and dashes, or nothing.
pub fn screen_key(s: &str) -> Option<String> {
    let k: String = s.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let k = k.trim_matches('-').chars().take(MAX_SCREEN).collect::<String>();
    (!k.is_empty()).then_some(k)
}

/// A canvas id: `c-` and 16 hex digits.
pub fn new_id() -> String {
    format!("c-{}", crate::ids::random_hex(8))
}

/// Whether `id` is a canvas id this module makes (checked before any path is built from it).
pub fn valid_id(id: &str) -> bool {
    id.len() == 18 && id.starts_with("c-") && id[2..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A generated picture file name: `<kind>-<12 hex>.<ext>`.
pub fn new_file(kind: char, f: Format) -> String {
    format!("{kind}-{}.{}", crate::ids::random_hex(6), f.ext())
}

/// Whether `name` is a picture file name of the kind [`new_file`] makes.
pub fn valid_file(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else { return false };
    let mut parts = stem.splitn(2, '-');
    let (k, hex) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    matches!(k, "s" | "a") && hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && Format::of_file(name).is_some() && ext.len() <= 4
}

fn item_id(prefix: &str) -> String {
    format!("{prefix}-{}", crate::ids::random_hex(4))
}

// ---------------------------------------------------------------------------
// The manifest

/// A shot the agent added.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shot {
    pub id: String,
    pub file: String,
    pub format: Option<Format>,
    /// The agent round it came in (1, 2, ...).
    pub round: u32,
    pub title: String,
    pub note: String,
    /// The viewport width the agent says it was taken at (CSS pixels).
    pub width: Option<u32>,
    /// The screen it shows, to line up the same screen across rounds.
    pub screen: Option<String>,
    pub px_w: u32,
    pub px_h: u32,
    pub bytes: u64,
    pub added_ms: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A picture the person added.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Asset {
    pub id: String,
    /// Numbered per canvas, for the remarks ("Asset 2").
    pub n: u32,
    pub file: String,
    pub format: Option<Format>,
    /// The person's file name, shown only.
    pub name: String,
    pub px_w: u32,
    pub px_h: u32,
    pub bytes: u64,
    pub added_ms: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// The review round that sent it; `None` while a draft.
    pub sent: Option<u32>,
    /// Where sbxw copied it in the sandbox, once sent.
    pub sandbox_path: Option<String>,
}

/// A numbered pin on a point of a shot (or an asset).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pin {
    pub n: u32,
    /// The shot's or asset's id.
    pub on: String,
    /// Where on it, as fractions of its width and height.
    pub fx: f32,
    pub fy: f32,
    pub text: String,
    pub sent: Option<u32>,
}

/// A note card placed anywhere.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Note {
    pub id: String,
    pub n: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub sent: Option<u32>,
}

/// One batch of shots from the agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Round {
    pub n: u32,
    pub at_ms: u64,
    pub shots: u32,
    /// The tool call's batch id, so its several posts form one round.
    pub batch: String,
}

/// One round of remarks sent to the agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Review {
    pub n: u32,
    pub at_ms: u64,
    /// The session it was sent to, as the person chose it (display only).
    pub session: String,
    pub pins: u32,
    pub notes: u32,
    pub assets: u32,
    /// When the agent read it with `canvas_read`.
    pub read_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Canvas {
    pub version: u32,
    pub id: String,
    /// The sandbox's ref, text form (`web-app`, `buildbox~api`).
    pub sandbox: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub shots: Vec<Shot>,
    pub assets: Vec<Asset>,
    pub pins: Vec<Pin>,
    pub notes: Vec<Note>,
    pub rounds: Vec<Round>,
    pub reviews: Vec<Review>,
}

/// The canvas as listed: no items.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub id: String,
    pub sandbox: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub shots: usize,
    pub rounds: usize,
    pub reviews: usize,
    pub bytes: u64,
    /// Remarks not sent yet (pins, notes, assets).
    pub draft: usize,
}

/// What a person's edit may set: the unsent annotations whole, and positions.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// The unsent pins, whole (sent ones are kept as they were).
    pub pins: Option<Vec<Pin>>,
    /// The unsent notes, whole.
    pub notes: Option<Vec<Note>>,
    /// Unsent assets removed by the person.
    pub remove_assets: Vec<String>,
    /// Item positions and sizes: `{id, x, y, w, h}`.
    pub layout: Vec<Place>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Place {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn finite(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        0.0
    }
}

/// World coordinates are kept within this square.
const WORLD: f32 = 1.0e6;

impl Canvas {
    pub fn new(id: String, sandbox: String, title: &str, now: u64) -> Canvas {
        let title = clean(title, MAX_TITLE, false);
        Canvas { version: VERSION, id, sandbox, title: if title.is_empty() { "Review".into() } else { title }, created_ms: now, updated_ms: now, ..Default::default() }
    }

    pub fn bytes(&self) -> u64 {
        self.shots.iter().map(|s| s.bytes).sum::<u64>() + self.assets.iter().map(|a| a.bytes).sum::<u64>()
    }

    pub fn draft(&self) -> usize {
        self.pins.iter().filter(|p| p.sent.is_none()).count() + self.notes.iter().filter(|n| n.sent.is_none()).count() + self.assets.iter().filter(|a| a.sent.is_none()).count()
    }

    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            sandbox: self.sandbox.clone(),
            title: self.title.clone(),
            created_ms: self.created_ms,
            updated_ms: self.updated_ms,
            shots: self.shots.len(),
            rounds: self.rounds.len(),
            reviews: self.reviews.len(),
            bytes: self.bytes(),
            draft: self.draft(),
        }
    }

    /// Every file the canvas names (to delete, to check).
    pub fn files(&self) -> Vec<&str> {
        self.shots.iter().map(|s| s.file.as_str()).chain(self.assets.iter().map(|a| a.file.as_str())).collect()
    }

    /// Whether `file` is one of this canvas's pictures, and its format.
    pub fn file_format(&self, file: &str) -> Option<Format> {
        if !valid_file(file) {
            return None;
        }
        self.shots.iter().find(|s| s.file == file).map(|s| s.format.or(Format::of_file(file))).or_else(|| self.assets.iter().find(|a| a.file == file).map(|a| a.format.or(Format::of_file(file)))).flatten()
    }

    /// The round a shot posted with `batch` goes in: the open one when the
    /// batch matches the last round, else a new one. `Err` when that round is full.
    pub fn round_for(&mut self, batch: &str, now: u64) -> Result<u32, String> {
        if let Some(last) = self.rounds.last() {
            if !batch.is_empty() && last.batch == batch {
                if last.shots as usize >= MAX_SHOTS_PER_ROUND {
                    return Err(format!("a round holds at most {MAX_SHOTS_PER_ROUND} shots; this one is full"));
                }
                return Ok(last.n);
            }
        }
        let n = self.rounds.last().map_or(1, |r| r.n + 1);
        self.rounds.push(Round { n, at_ms: now, shots: 0, batch: batch.to_string() });
        Ok(n)
    }

    /// Adds a shot (the picture already checked and stored under `file`),
    /// placed in its round's row: below every earlier row, under an earlier
    /// shot of the same screen when there is one, else after the row's last.
    #[allow(clippy::too_many_arguments)]
    pub fn add_shot(&mut self, round: u32, file: String, h: Header, bytes: u64, title: &str, note: &str, width: Option<u32>, screen: Option<&str>, now: u64) -> &Shot {
        let w = match width.filter(|w| (100..=MAX_SIDE).contains(w)) {
            Some(w) => w as f32,
            None => h.width as f32,
        };
        let sh = w * h.height as f32 / h.width as f32;
        let screen = screen.and_then(screen_key);
        // The row's top: under everything placed before this round.
        let earlier = |s: &&Shot| s.round < round;
        let row_y = self.shots.iter().filter(earlier).map(|s| s.y + s.h).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v)))).map_or(0.0, |b| b + ROW_GAP);
        let row_y = self.shots.iter().filter(|s| s.round == round).map(|s| s.y).fold(row_y, f32::min);
        let next_x = self.shots.iter().filter(|s| s.round == round).map(|s| s.x + s.w + COL_GAP).fold(0.0, f32::max);
        let column = screen.as_ref().and_then(|k| self.shots.iter().rev().find(|s| s.round < round && s.screen.as_deref() == Some(k)).map(|s| s.x));
        let mut x = match column {
            Some(cx) if cx >= next_x || !self.shots.iter().any(|s| s.round == round && s.x < cx + w && cx < s.x + s.w) => cx,
            _ => next_x,
        };
        // Never on top of a shot of the same row.
        while self.shots.iter().any(|s| s.round == round && x < s.x + s.w && s.x < x + w) {
            x = self.shots.iter().filter(|s| s.round == round).map(|s| s.x + s.w + COL_GAP).fold(x, f32::max);
        }
        self.shots.push(Shot {
            id: item_id("s"),
            file,
            format: Some(h.format),
            round,
            title: clean(title, MAX_TITLE, false),
            note: clean(note, MAX_NOTE, true),
            width,
            screen,
            px_w: h.width,
            px_h: h.height,
            bytes,
            added_ms: now,
            x,
            y: row_y,
            w,
            h: sh,
        });
        if let Some(r) = self.rounds.iter_mut().find(|r| r.n == round) {
            r.shots += 1;
        }
        self.updated_ms = now;
        self.shots.last().unwrap()
    }

    pub fn add_asset(&mut self, file: String, h: Header, bytes: u64, name: &str, at: Option<(f32, f32)>, now: u64) -> Result<&Asset, String> {
        if self.assets.len() >= MAX_ASSETS {
            return Err(format!("a canvas holds at most {MAX_ASSETS} assets"));
        }
        let k = DEFAULT_ASSET_W / h.width.max(h.height) as f32;
        let k = k.min(1.0);
        let (w, hh) = (h.width as f32 * k, h.height as f32 * k);
        let (x, y) = match at {
            Some((x, y)) => (finite(x, -WORLD, WORLD), finite(y, -WORLD, WORLD)),
            None => {
                let right = self.shots.iter().map(|s| s.x + s.w).chain(self.assets.iter().map(|a| a.x + a.w)).fold(0.0, f32::max);
                (right + COL_GAP, 0.0)
            }
        };
        let n = self.assets.iter().map(|a| a.n).max().unwrap_or(0) + 1;
        self.assets.push(Asset { id: item_id("a"), n, file, format: Some(h.format), name: clean(name, MAX_NAME, false), px_w: h.width, px_h: h.height, bytes, added_ms: now, x, y, w, h: hh, sent: None, sandbox_path: None });
        self.updated_ms = now;
        Ok(self.assets.last().unwrap())
    }

    /// The person's edit: unsent annotations replaced, positions moved.
    /// Sent annotations never change. Returns the assets removed (their files to delete).
    pub fn apply(&mut self, e: Edit, now: u64) -> Result<Vec<String>, String> {
        let items: Vec<String> = self.shots.iter().map(|s| s.id.clone()).chain(self.assets.iter().map(|a| a.id.clone())).collect();
        if let Some(pins) = e.pins {
            let sent: Vec<Pin> = self.pins.iter().filter(|p| p.sent.is_some()).cloned().collect();
            let used: Vec<u32> = sent.iter().map(|p| p.n).collect();
            if pins.len() + sent.len() > MAX_PINS {
                return Err(format!("a canvas holds at most {MAX_PINS} pins"));
            }
            let mut out = sent;
            for p in pins {
                if p.n == 0 || used.contains(&p.n) || out.iter().any(|q| q.n == p.n) {
                    return Err(format!("pin {} is already used", p.n));
                }
                if !items.contains(&p.on) {
                    return Err("a pin must sit on a shot or an asset of this canvas".into());
                }
                out.push(Pin { n: p.n, on: p.on, fx: finite(p.fx, 0.0, 1.0), fy: finite(p.fy, 0.0, 1.0), text: clean(&p.text, MAX_NOTE, true), sent: None });
            }
            self.pins = out;
        }
        if let Some(notes) = e.notes {
            let sent: Vec<Note> = self.notes.iter().filter(|n| n.sent.is_some()).cloned().collect();
            if notes.len() + sent.len() > MAX_NOTES {
                return Err(format!("a canvas holds at most {MAX_NOTES} notes"));
            }
            let mut out = sent;
            for n in notes {
                if n.n == 0 || out.iter().any(|q| q.n == n.n) {
                    return Err(format!("note {} is already used", n.n));
                }
                let id = if n.id.starts_with("n-") && n.id.len() <= 12 && n.id[2..].bytes().all(|b| b.is_ascii_hexdigit()) { n.id } else { item_id("n") };
                out.push(Note { id, n: n.n, x: finite(n.x, -WORLD, WORLD), y: finite(n.y, -WORLD, WORLD), w: finite(n.w, 120.0, 2000.0), h: finite(n.h, 60.0, 2000.0), text: clean(&n.text, MAX_NOTE, true), sent: None });
            }
            self.notes = out;
        }
        let mut removed = vec![];
        for id in &e.remove_assets {
            if let Some(i) = self.assets.iter().position(|a| &a.id == id && a.sent.is_none()) {
                let a = self.assets.remove(i);
                self.pins.retain(|p| p.on != a.id || p.sent.is_some());
                removed.push(a.file);
            }
        }
        for p in e.layout {
            let (x, y) = (finite(p.x, -WORLD, WORLD), finite(p.y, -WORLD, WORLD));
            let (w, h) = (finite(p.w, 20.0, 20_000.0), finite(p.h, 20.0, 20_000.0));
            if let Some(s) = self.shots.iter_mut().find(|s| s.id == p.id) {
                (s.x, s.y, s.w, s.h) = (x, y, w, h);
            } else if let Some(a) = self.assets.iter_mut().find(|a| a.id == p.id) {
                (a.x, a.y, a.w, a.h) = (x, y, w, h);
            } else if let Some(n) = self.notes.iter_mut().find(|n| n.id == p.id) {
                (n.x, n.y, n.w, n.h) = (x, y, w.max(120.0), h.max(60.0));
            }
        }
        self.updated_ms = now;
        Ok(removed)
    }

    /// Records a round of remarks: every unsent pin, note and asset now
    /// belongs to it. `paths`: each asset's id and where sbxw copied it in
    /// the sandbox. `Err` when there is nothing to send or an asset has no path.
    pub fn submit(&mut self, session: &str, paths: &[(String, String)], now: u64) -> Result<&Review, String> {
        if self.draft() == 0 {
            return Err("there is no new remark to send".into());
        }
        for a in self.assets.iter().filter(|a| a.sent.is_none()) {
            if !paths.iter().any(|(id, _)| id == &a.id) {
                return Err(format!("asset {} was not copied into the sandbox", a.n));
            }
        }
        let n = self.reviews.last().map_or(1, |r| r.n + 1);
        let (mut pins, mut notes, mut assets) = (0, 0, 0);
        for p in self.pins.iter_mut().filter(|p| p.sent.is_none()) {
            p.sent = Some(n);
            pins += 1;
        }
        for x in self.notes.iter_mut().filter(|x| x.sent.is_none()) {
            x.sent = Some(n);
            notes += 1;
        }
        for a in self.assets.iter_mut().filter(|a| a.sent.is_none()) {
            a.sent = Some(n);
            a.sandbox_path = paths.iter().find(|(id, _)| id == &a.id).map(|(_, p)| clean(p, 400, false));
            assets += 1;
        }
        self.reviews.push(Review { n, at_ms: now, session: clean(session, 120, false), pins, notes, assets, read_ms: None });
        self.updated_ms = now;
        Ok(self.reviews.last().unwrap())
    }

    /// The reviews the agent has not read, oldest first.
    pub fn unread(&self) -> Vec<u32> {
        self.reviews.iter().filter(|r| r.read_ms.is_none()).map(|r| r.n).collect()
    }

    /// The text `canvas_read` hands the agent for review `n`: the person's
    /// remarks, each tied to a named shot, framed as data.
    pub fn review_text(&self, n: u32) -> String {
        let mut out = String::new();
        let Some(r) = self.reviews.iter().find(|r| r.n == n) else { return out };
        out.push_str(&format!("Review round {n} of canvas \"{}\" ({}): {} pin(s), {} note(s), {} asset(s).\n", self.title, self.id, r.pins, r.notes, r.assets));
        let mut pins: Vec<&Pin> = self.pins.iter().filter(|p| p.sent == Some(n)).collect();
        pins.sort_by_key(|p| p.n);
        for p in pins {
            let on = match self.shots.iter().find(|s| s.id == p.on) {
                Some(s) => format!("shot \"{}\" (round {}{})", s.title, s.round, s.width.map(|w| format!(", {w} wide")).unwrap_or_default()),
                None => match self.assets.iter().find(|a| a.id == p.on) {
                    Some(a) => format!("asset {}", a.n),
                    None => "a removed item".into(),
                },
            };
            out.push_str(&format!("\nPin {} on {on}, at {:.0}% across and {:.0}% down ({}):\n{}\n", p.n, p.fx * 100.0, p.fy * 100.0, hint(p.fx, p.fy), quote(&p.text)));
        }
        let mut notes: Vec<&Note> = self.notes.iter().filter(|x| x.sent == Some(n)).collect();
        notes.sort_by_key(|x| x.n);
        for x in notes {
            let near = self.nearest_shot(x.x + x.w / 2.0, x.y + x.h / 2.0).map(|s| format!(" (next to shot \"{}\")", s.title)).unwrap_or_default();
            out.push_str(&format!("\nNote {}{near}:\n{}\n", x.n, quote(&x.text)));
        }
        for a in self.assets.iter().filter(|a| a.sent == Some(n)) {
            let near = self.nearest_shot(a.x + a.w / 2.0, a.y + a.h / 2.0).map(|s| format!(", placed next to shot \"{}\"", s.title)).unwrap_or_default();
            out.push_str(&format!("\nAsset {}: an example picture the person added ({}x{} px{near}), copied to {}\n", a.n, a.px_w, a.px_h, a.sandbox_path.as_deref().unwrap_or("(not copied)")));
        }
        out
    }

    fn nearest_shot(&self, x: f32, y: f32) -> Option<&Shot> {
        self.shots.iter().min_by(|a, b| {
            let d = |s: &Shot| (s.x + s.w / 2.0 - x).hypot(s.y + s.h / 2.0 - y);
            d(a).total_cmp(&d(b))
        })
    }
}

/// A short location hint: top-left, middle, bottom-right...
pub fn hint(fx: f32, fy: f32) -> &'static str {
    let col = if fx < 1.0 / 3.0 { 0 } else if fx < 2.0 / 3.0 { 1 } else { 2 };
    let row = if fy < 1.0 / 3.0 { 0 } else if fy < 2.0 / 3.0 { 1 } else { 2 };
    [["top-left", "top", "top-right"], ["left", "middle", "right"], ["bottom-left", "bottom", "bottom-right"]][row][col]
}

/// Remark text as a quoted block, so it reads as the person's words.
fn quote(t: &str) -> String {
    if t.trim().is_empty() {
        return "> (no text)".into();
    }
    t.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
}

/// The fixed line sbxw types into the session when a review round is sent.
pub fn ready_line(canvas: &str, n: u32) -> String {
    format!("[sbxw canvas · {canvas}] review round {n} is ready: run canvas_read to read the person's remarks")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend(13u32.to_be_bytes());
        b.extend(b"IHDR");
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b.extend(0u32.to_be_bytes());
        b.extend(b"IDAT");
        b
    }

    #[test]
    fn pictures_are_known_by_their_bytes() {
        assert_eq!(sniff(&png_bytes(1440, 900)).unwrap(), Header { format: Format::Png, width: 1440, height: 900 });
        // A JPEG: SOI, an APP0 segment, then SOF0 with 900 high, 1440 wide.
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08];
        j.extend(900u16.to_be_bytes());
        j.extend(1440u16.to_be_bytes());
        j.extend([0u8; 12]);
        assert_eq!(sniff(&j).unwrap(), Header { format: Format::Jpeg, width: 1440, height: 900 });
        // WebP lossless: 390x844.
        let v = (390 - 1) | ((844 - 1) << 14);
        let mut w = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        w.extend((v as u32).to_le_bytes());
        assert_eq!(sniff(&w).unwrap(), Header { format: Format::Webp, width: 390, height: 844 });
        // WebP extended, animated: refused.
        let mut x = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0".to_vec();
        x.extend([0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(sniff(&x).unwrap_err().contains("animated"));
        // A name never matters: an SVG, HTML, text are refused by content.
        for bad in [&b"<svg xmlns='http://www.w3.org/2000/svg'/>"[..], b"<html><script>x</script>", b"GIF89a....", b""] {
            assert!(sniff(bad).is_err());
        }
        // Too large a side, too many bytes, empty.
        assert!(sniff(&png_bytes(8193, 10)).unwrap_err().contains("8192"));
        assert!(sniff(&png_bytes(0, 10)).is_err());
        let mut big = png_bytes(10, 10);
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(sniff(&big).unwrap_err().contains("MB"));
        // APNG.
        let mut a = png_bytes(10, 10);
        a.truncate(33);
        a.extend(8u32.to_be_bytes());
        a.extend(b"acTL");
        a.extend([0u8; 12]);
        assert!(sniff(&a).unwrap_err().contains("animated"));
        // Truncated headers never panic.
        for n in 0..40 {
            let _ = sniff(&png_bytes(10, 10)[..n.min(33)]);
            let _ = sniff(&j[..n.min(j.len())]);
            let _ = sniff(&w[..n.min(w.len())]);
        }
    }

    #[test]
    fn generated_names_and_ids_are_the_only_ones_taken() {
        let f = new_file('s', Format::Png);
        assert!(valid_file(&f), "{f}");
        for bad in ["../x.png", "s-../../etc.png", "s-0123456789ab.svg", "x-0123456789ab.png", "s-0123456789AB.png", "/etc/passwd", "s-0123456789ab.png/..", ""] {
            assert!(!valid_file(bad), "{bad}");
        }
        let id = new_id();
        assert!(valid_id(&id), "{id}");
        for bad in ["c-../../../tmp", "c-0123", "../c-0123456789abcdef", "c-0123456789ABCDEF", "c-0123456789abcdeg"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn texts_are_cleaned_and_capped() {
        assert_eq!(clean("  a\u{1b}[31mb\u{202e}c\n d ", 100, false), "a[31mbc d");
        assert_eq!(clean("a\nb", 100, true), "a\nb");
        assert_eq!(clean(&"é".repeat(300), MAX_TITLE, false).chars().count(), MAX_TITLE);
        assert_eq!(screen_key(" Settings / Mobile "), Some("settings---mobile".into()));
        assert_eq!(screen_key("../.."), None);
    }

    fn hdr(w: u32, h: u32) -> Header {
        Header { format: Format::Png, width: w, height: h }
    }

    #[test]
    fn rounds_form_rows_and_the_same_screen_lines_up() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        let r1 = c.round_for("b1", 1).unwrap();
        c.add_shot(r1, "s-000000000001.png".into(), hdr(2880, 1800), 10, "Home", "", Some(1440), Some("home"), 1);
        c.add_shot(r1, "s-000000000002.png".into(), hdr(2880, 1800), 10, "Settings", "", Some(1440), Some("settings"), 1);
        assert_eq!(c.round_for("b1", 1).unwrap(), 1, "the same batch is the same round");
        let (a, b) = (&c.shots[0], &c.shots[1]);
        assert_eq!((a.x, a.y, a.w, a.h), (0.0, 0.0, 1440.0, 900.0));
        assert_eq!((b.x, b.y), (1440.0 + COL_GAP, 0.0));
        let r2 = c.round_for("b2", 2).unwrap();
        assert_eq!(r2, 2);
        c.add_shot(r2, "s-000000000003.png".into(), hdr(1440, 900), 10, "Settings after", "", None, Some("settings"), 2);
        let s = c.shots[2].clone();
        assert_eq!((s.x, s.y), (1440.0 + COL_GAP, 900.0 + ROW_GAP), "under the same screen, a row below");
        c.add_shot(r2, "s-000000000004.png".into(), hdr(1440, 900), 10, "New", "", None, None, 2);
        assert!(c.shots[3].x >= s.x + s.w, "a new screen after the row's last");
        // A full round refuses the 25th.
        let r3 = c.round_for("b3", 3).unwrap();
        for k in 0..MAX_SHOTS_PER_ROUND {
            c.round_for("b3", 3).unwrap();
            c.add_shot(r3, format!("s-{k:012}.png"), hdr(100, 100), 1, "x", "", None, None, 3);
        }
        assert!(c.round_for("b3", 3).is_err());
    }

    #[test]
    fn a_review_takes_the_draft_and_is_read_only_after() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        let r = c.round_for("b", 1).unwrap();
        c.add_shot(r, "s-000000000001.png".into(), hdr(1440, 900), 10, "Home page", "", Some(1440), None, 1);
        let shot = c.shots[0].id.clone();
        assert!(c.submit("web · main", &[], 2).is_err(), "nothing to send");
        let a = c.add_asset("a-000000000001.png".into(), hdr(800, 600), 5, "example.png", Some((2000.0, 0.0)), 2).unwrap().id.clone();
        let e = Edit { pins: Some(vec![Pin { n: 1, on: shot.clone(), fx: 0.1, fy: 0.05, text: "Logo too big".into(), sent: None }]), notes: Some(vec![Note { n: 1, x: 10.0, y: 950.0, w: 240.0, h: 140.0, text: "Calmer colours".into(), ..Default::default() }]), ..Default::default() };
        c.apply(e, 3).unwrap();
        assert_eq!(c.draft(), 3);
        assert!(c.submit("web · main", &[], 4).unwrap_err().contains("asset 1"), "an asset must have been copied");
        let r = c.submit("web · main", &[(a.clone(), "/tmp/sbxw-uploads/x/example.png".into())], 4).unwrap().clone();
        assert_eq!((r.n, r.pins, r.notes, r.assets), (1, 1, 1, 1));
        assert_eq!(c.draft(), 0);
        let t = c.review_text(1);
        assert!(t.contains("Pin 1 on shot \"Home page\" (round 1, 1440 wide), at 10% across and 5% down (top-left):\n> Logo too big"), "{t}");
        assert!(t.contains("Note 1 (next to shot \"Home page\"):\n> Calmer colours"));
        assert!(t.contains("copied to /tmp/sbxw-uploads/x/example.png"));
        // Sent pins cannot be changed or reused by an edit.
        let e = Edit { pins: Some(vec![Pin { n: 1, on: shot.clone(), fx: 0.5, fy: 0.5, text: "changed".into(), sent: None }]), ..Default::default() };
        assert!(c.apply(e, 5).is_err());
        let e = Edit { pins: Some(vec![]), notes: Some(vec![]), remove_assets: vec![a], ..Default::default() };
        assert!(c.apply(e, 5).unwrap().is_empty(), "a sent asset is not removed");
        assert_eq!((c.pins.len(), c.notes.len(), c.assets.len()), (1, 1, 1));
        assert_eq!(c.pins[0].text, "Logo too big");
        assert_eq!(c.unread(), vec![1]);
    }

    #[test]
    fn hostile_edits_are_bounded() {
        let mut c = Canvas::new(new_id(), "web".into(), "\u{1b}]0;x\u{7}Title", 1);
        assert_eq!(c.title, "]0;xTitle");
        let e = Edit { pins: Some(vec![Pin { n: 1, on: "s-nothere".into(), ..Default::default() }]), ..Default::default() };
        assert!(c.apply(e, 2).is_err(), "a pin on nothing");
        let e = Edit { notes: Some(vec![Note { id: "../../x".into(), n: 1, x: f32::NAN, y: f32::INFINITY, w: 1e30, h: -5.0, text: "x".repeat(5000), sent: Some(9) }]), ..Default::default() };
        c.apply(e, 2).unwrap();
        let n = &c.notes[0];
        assert!(n.id.starts_with("n-") && n.id != "../../x");
        assert_eq!((n.x, n.y, n.w, n.h), (0.0, 0.0, 2000.0, 60.0));
        assert_eq!(n.text.chars().count(), MAX_NOTE);
        assert_eq!(n.sent, None, "a client cannot mark a note sent");
    }

    #[test]
    fn older_manifests_load() {
        let c: Canvas = serde_json::from_str(r#"{"id":"c-0123456789abcdef","sandbox":"web","title":"Old","shots":[{"id":"s-1","file":"s-000000000001.png","title":"Home"}]}"#).unwrap();
        assert_eq!(c.version, 0);
        assert_eq!(c.shots[0].round, 0);
        assert!(c.pins.is_empty() && c.reviews.is_empty());
        assert_eq!(c.file_format("s-000000000001.png"), Some(Format::Png));
        assert_eq!(c.file_format("s-000000000009.png"), None);
    }

    #[test]
    fn base64_decodes_within_its_cap() {
        assert_eq!(unbase64(&crate::mac::base64(b"PNGDATA!"), 100).unwrap(), b"PNGDATA!");
        assert_eq!(unbase64(&crate::mac::base64(b"ab"), 100).unwrap(), b"ab");
        assert!(unbase64("<script>", 100).is_err());
        assert!(unbase64(&"A".repeat(400), 100).is_err());
    }

    #[test]
    fn hints_name_the_part_of_a_shot() {
        assert_eq!(hint(0.05, 0.05), "top-left");
        assert_eq!(hint(0.5, 0.5), "middle");
        assert_eq!(hint(0.9, 0.95), "bottom-right");
    }
}
