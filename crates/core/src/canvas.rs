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
/// World points between two rounds' rows in the one-row layout before
/// grids (kept to recognise a canvas laid out so), and between two shots of a row.
pub const ROW_GAP: f32 = 320.0;
/// World points between two rounds' grids: the last row's titles and the
/// next round's band, each at most 100 world points tall (they keep a fixed
/// size on screen), both fit at any zoom.
pub const ROUND_GAP: f32 = 480.0;
pub const COL_GAP: f32 = 120.0;
/// World points between two rows of one round's grid: the titles under a row sit in it.
pub const GRID_ROW_GAP: f32 = 200.0;
/// World points between two narrow shots (phones) sharing one cell.
pub const NARROW_GAP: f32 = 48.0;
/// The shape a round's grid aims for: a laptop's canvas, wider than tall.
pub const GRID_ASPECT: f32 = 1.6;
/// A round's grid has at most this many columns.
pub const MAX_COLS: u32 = 8;
/// A cell's slot meaning "the whole cell" (a shot wider than half a cell).
pub const WHOLE: u32 = u32::MAX;
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
    /// Moved or resized by the person: the layout never places it again.
    pub moved: bool,
    /// Its cell in its round's grid: row, column and slot (narrow shots
    /// share a cell side by side; [`WHOLE`] for a shot that fills one).
    /// `None` for a shot laid out before grids (one long row).
    pub cell: Option<[u32; 3]>,
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
    /// The shot it was written about, when the phone wrote it (the phone
    /// has no free placement). Absent from notes the Mac placed; kept
    /// across the Mac's edits by the note's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
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
    /// Its grid: columns, a cell's width, a narrow slot's width (world
    /// points). `cols` 0: laid out before grids, as one long row.
    pub cols: u32,
    pub cell_w: f32,
    pub slot_w: f32,
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
        self.rounds.push(Round { n, at_ms: now, shots: 0, batch: batch.to_string(), ..Default::default() });
        Ok(n)
    }

    /// Adds a shot (the picture already checked and stored under `file`)
    /// to its round's grid ([`Canvas::layout_round`]); notes and assets
    /// beside a shot that moves follow it.
    #[allow(clippy::too_many_arguments)]
    pub fn add_shot(&mut self, round: u32, file: String, h: Header, bytes: u64, title: &str, note: &str, width: Option<u32>, screen: Option<&str>, now: u64) -> &Shot {
        let (w, sh) = auto_size(width, h.width, h.height);
        let screen = screen.and_then(screen_key);
        let near = self.nearest_map();
        let id = item_id("s");
        self.shots.push(Shot {
            id: id.clone(),
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
            x: 0.0,
            y: 0.0,
            w,
            h: sh,
            moved: false,
            cell: None,
        });
        if let Some(r) = self.rounds.iter_mut().find(|r| r.n == round) {
            r.shots += 1;
        }
        let moves = self.layout_round(round);
        self.follow(&near, &moves);
        self.updated_ms = now;
        self.shots.iter().find(|s| s.id == id).unwrap()
    }

    /// Lays round `round` out as a grid, below every earlier round, and
    /// returns the shots it moved (id, dx, dy). Shots the person moved keep
    /// their place (and their cell, if they had one).
    ///
    /// * Cells are as wide as the round's widest shot. A shot at most half a
    ///   cell wide (a phone beside desktop shots) is narrow: narrow shots
    ///   share a cell side by side, [`NARROW_GAP`] apart.
    /// * Columns: the count whose block is closest to [`GRID_ASPECT`]
    ///   ([`grid_cols`]): 12 desktop shots make 4 x 3, 9 make 3 x 3, 4 make
    ///   2 x 2, 24 make 5 x 5.
    /// * Rows are as tall as their tallest shot; a row with nothing in this
    ///   round takes no room (its order is kept).
    /// * A shot whose `screen` key was in an earlier grid takes the SAME
    ///   cell (row, column, slot) as its latest earlier shot, and the round
    ///   takes that round's columns and cell width, so before and after line
    ///   up. A new screen takes the first free cell (row by row) where that
    ///   earlier round showed nothing: it never sits under another screen.
    pub fn layout_round(&mut self, round: u32) -> Vec<(String, f32, f32)> {
        let idx: Vec<usize> = (0..self.shots.len()).filter(|&i| self.shots[i].round == round).collect();
        if idx.is_empty() {
            return vec![];
        }
        let top = self.shots.iter().filter(|s| s.round < round).map(|s| s.y + s.h).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v)))).map_or(0.0, |b| b + ROUND_GAP);
        let keys: Vec<&str> = idx.iter().filter_map(|&i| self.shots[i].screen.as_deref()).collect();
        // The earlier grid this round compares with: the latest with a screen of this round.
        let rref = self.shots.iter().filter(|s| s.round < round && s.cell.is_some() && s.screen.as_deref().is_some_and(|k| keys.contains(&k))).map(|s| s.round).max();
        let rinfo = rref.and_then(|n| self.rounds.iter().find(|r| r.n == n && r.cols > 0)).cloned();
        let ref_used: Vec<[u32; 3]> = match rref {
            Some(n) => self.shots.iter().filter(|s| s.round == n).filter_map(|s| s.cell).collect(),
            None => vec![],
        };
        let mut cell_w = idx.iter().map(|&i| self.shots[i].w).fold(0.0, f32::max);
        if let Some(r) = &rinfo {
            cell_w = cell_w.max(r.cell_w);
        }
        let narrow = |w: f32| w <= (cell_w - NARROW_GAP) / 2.0;
        let mut slot_w = idx.iter().map(|&i| self.shots[i].w).filter(|&w| narrow(w)).fold(0.0, f32::max);
        if let Some(r) = &rinfo {
            if r.slot_w > 0.0 && narrow(r.slot_w) {
                slot_w = slot_w.max(r.slot_w);
            }
        }
        let per = if slot_w > 0.0 { (((cell_w + NARROW_GAP) / (slot_w + NARROW_GAP)).floor() as u32).max(1) } else { 1 };
        let cell_h = idx.iter().map(|&i| self.shots[i].h).fold(0.0, f32::max);
        let auto: Vec<usize> = idx.iter().copied().filter(|&i| !self.shots[i].moved).collect();
        let wide_n = idx.iter().filter(|&&i| !narrow(self.shots[i].w)).count();
        let cells = wide_n + (idx.len() - wide_n).div_ceil(per as usize);
        let cols = match &rinfo {
            Some(r) => r.cols,
            None => grid_cols(cells, cell_w, cell_h),
        };
        // What this round's cells hold: (row, col) -> slots, WHOLE for a whole cell.
        let mut taken: Vec<([u32; 2], u32)> = idx.iter().filter(|&&i| self.shots[i].moved).filter_map(|&i| self.shots[i].cell).map(|c| ([c[0], c[1]], c[2])).collect();
        let free = |taken: &Vec<([u32; 2], u32)>, rc: [u32; 2], k: u32| -> bool {
            let here: Vec<u32> = taken.iter().filter(|(c, _)| *c == rc).map(|(_, k)| *k).collect();
            if k == WHOLE {
                here.is_empty()
            } else {
                !here.contains(&WHOLE) && !here.contains(&k)
            }
        };
        let mut place: Vec<(usize, [u32; 3])> = vec![];
        // Same screen, same cell.
        for &i in &auto {
            let s = &self.shots[i];
            let want = if narrow(s.w) { None } else { Some(WHOLE) };
            let Some(k) = s.screen.as_deref() else { continue };
            let earlier = self.shots.iter().filter(|e| e.round < round && e.screen.as_deref() == Some(k) && e.cell.is_some()).max_by_key(|e| e.round);
            let Some(c) = earlier.and_then(|e| e.cell) else { continue };
            let fits = c[1] < cols && match want {
                Some(_) => c[2] == WHOLE,
                None => c[2] != WHOLE && c[2] < per,
            };
            if fits && free(&taken, [c[0], c[1]], c[2]) {
                taken.push(([c[0], c[1]], c[2]));
                place.push((i, c));
            }
        }
        // New screens: the first free cell the earlier round left empty.
        for &i in &auto {
            if place.iter().any(|(j, _)| *j == i) {
                continue;
            }
            let wide = !narrow(self.shots[i].w);
            let mut n = 0u32;
            let cell = loop {
                let rc = [n / cols, n % cols];
                let in_ref = |k: u32| ref_used.iter().any(|c| [c[0], c[1]] == rc && (c[2] == WHOLE || k == WHOLE || c[2] == k));
                let got = if wide {
                    (free(&taken, rc, WHOLE) && !in_ref(WHOLE)).then_some(WHOLE)
                } else {
                    (0..per).find(|&k| free(&taken, rc, k) && !in_ref(k))
                };
                if let Some(k) = got {
                    break [rc[0], rc[1], k];
                }
                n += 1;
            };
            taken.push(([cell[0], cell[1]], cell[2]));
            place.push((i, cell));
        }
        // Rows as tall as their tallest shot; a row this round leaves empty
        // (screens that did not come back) takes no room.
        let rows = place.iter().map(|(_, c)| c[0] + 1).max().unwrap_or(0);
        let mut row_y = Vec::with_capacity(rows as usize);
        let mut y = top;
        for r in 0..rows {
            row_y.push(y);
            let rh = place.iter().filter(|(_, c)| c[0] == r).map(|(i, _)| self.shots[*i].h).fold(0.0, f32::max);
            if place.iter().any(|(_, c)| c[0] == r) {
                y += rh + GRID_ROW_GAP;
            }
        }
        let mut moves = vec![];
        for (i, c) in place {
            let x = c[1] as f32 * (cell_w + COL_GAP) + if c[2] == WHOLE { 0.0 } else { c[2] as f32 * (slot_w + NARROW_GAP) };
            let s = &mut self.shots[i];
            let (dx, dy) = (x - s.x, row_y[c[0] as usize] - s.y);
            if dx != 0.0 || dy != 0.0 {
                moves.push((s.id.clone(), dx, dy));
            }
            (s.x, s.y, s.cell) = (x, row_y[c[0] as usize], Some(c));
        }
        if let Some(r) = self.rounds.iter_mut().find(|r| r.n == round) {
            (r.cols, r.cell_w, r.slot_w) = (cols, cell_w, slot_w);
        }
        moves
    }

    /// Each note's and asset's nearest shot (as `review_text` names it).
    fn nearest_map(&self) -> (Vec<Option<String>>, Vec<Option<String>>) {
        let notes = self.notes.iter().map(|n| self.nearest_shot(n.x + n.w / 2.0, n.y + n.h / 2.0).map(|s| s.id.clone())).collect();
        let assets = self.assets.iter().map(|a| self.nearest_shot(a.x + a.w / 2.0, a.y + a.h / 2.0).map(|s| s.id.clone())).collect();
        (notes, assets)
    }

    /// Moves each note and asset whose nearest shot moved by as much, so
    /// it stays beside the shot it was placed next to.
    fn follow(&mut self, near: &(Vec<Option<String>>, Vec<Option<String>>), moves: &[(String, f32, f32)]) {
        let d = |id: &Option<String>| id.as_ref().and_then(|id| moves.iter().find(|m| &m.0 == id)).map(|m| (m.1, m.2));
        for (n, id) in self.notes.iter_mut().zip(&near.0) {
            if let Some((dx, dy)) = d(id) {
                (n.x, n.y) = (finite(n.x + dx, -WORLD, WORLD), finite(n.y + dy, -WORLD, WORLD));
            }
        }
        for (a, id) in self.assets.iter_mut().zip(&near.1) {
            if let Some((dx, dy)) = d(id) {
                (a.x, a.y) = (finite(a.x + dx, -WORLD, WORLD), finite(a.y + dy, -WORLD, WORLD));
            }
        }
    }

    /// Where the one-row layout (before grids) put shot `i`, from the shots
    /// before it: to tell a round nobody moved from one the person arranged.
    fn legacy_spot(&self, i: usize) -> (f32, f32) {
        let prev = &self.shots[..i];
        let s = &self.shots[i];
        let (round, w) = (s.round, s.w);
        let row_y = prev.iter().filter(|p| p.round < round).map(|p| p.y + p.h).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v)))).map_or(0.0, |b| b + ROW_GAP);
        let row_y = prev.iter().filter(|p| p.round == round).map(|p| p.y).fold(row_y, f32::min);
        let next_x = prev.iter().filter(|p| p.round == round).map(|p| p.x + p.w + COL_GAP).fold(0.0, f32::max);
        let column = s.screen.as_ref().and_then(|k| prev.iter().rev().find(|p| p.round < round && p.screen.as_deref() == Some(k)).map(|p| p.x));
        let mut x = match column {
            Some(cx) if cx >= next_x || !prev.iter().any(|p| p.round == round && p.x < cx + w && cx < p.x + p.w) => cx,
            _ => next_x,
        };
        while prev.iter().any(|p| p.round == round && x < p.x + p.w && p.x < x + w) {
            x = prev.iter().filter(|p| p.round == round).map(|p| p.x + p.w + COL_GAP).fold(x, f32::max);
        }
        (x, row_y)
    }

    /// A canvas saved before grids: each round still in its one long row,
    /// as placed, is laid out again as a grid; a round where the person
    /// moved or resized a shot is left as it is (moved down as a block by
    /// what the rounds above it grew, so nothing overlaps). Notes and
    /// assets follow their nearest shot. Pins are fractions of their shot
    /// and follow on their own. Returns whether anything changed.
    pub fn relayout_legacy(&mut self) -> bool {
        let mut rounds: Vec<u32> = self.shots.iter().map(|s| s.round).collect();
        rounds.sort_unstable();
        rounds.dedup();
        let gridded = |n: u32| self.rounds.iter().any(|r| r.n == n && r.cols > 0) || self.shots.iter().any(|s| s.round == n && s.cell.is_some());
        // Judged on the canvas as loaded, before anything moves.
        let as_placed: Vec<u32> = rounds
            .iter()
            .copied()
            .filter(|&n| !gridded(n))
            .filter(|&n| {
                (0..self.shots.len()).filter(|&i| self.shots[i].round == n).all(|i| {
                    let s = &self.shots[i];
                    let (aw, ah) = auto_size(s.width, s.px_w, s.px_h);
                    let (x, y) = self.legacy_spot(i);
                    !s.moved && (s.x - x).abs() < 0.5 && (s.y - y).abs() < 0.5 && (s.w - aw).abs() < 0.5 && (s.h - ah).abs() < 0.5
                })
            })
            .collect();
        if as_placed.is_empty() {
            return false;
        }
        let near = self.nearest_map();
        let before: Vec<(String, f32, f32)> = self.shots.iter().map(|s| (s.id.clone(), s.x, s.y)).collect();
        let bottom = |c: &Canvas, below: u32| c.shots.iter().filter(|s| s.round < below).map(|s| s.y + s.h).fold(f32::MIN, f32::max);
        for &n in &rounds {
            if as_placed.contains(&n) {
                self.layout_round(n);
            } else {
                // Grown by the rounds above it, as they were and as they are.
                let was = before.iter().zip(&self.shots).filter(|(_, s)| s.round < n).map(|((_, _, y), s)| y + s.h).fold(f32::MIN, f32::max);
                let grew = bottom(self, n) - was;
                if was > f32::MIN && grew > 0.0 {
                    for s in self.shots.iter_mut().filter(|s| s.round == n) {
                        s.y += grew;
                    }
                }
            }
        }
        let moves: Vec<(String, f32, f32)> = before.iter().zip(&self.shots).filter(|((_, x, y), s)| s.x != *x || s.y != *y).map(|((id, x, y), s)| (id.clone(), s.x - x, s.y - y)).collect();
        self.follow(&near, &moves);
        !moves.is_empty()
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
                // The shot a note is about: as given, else as the same unsent
                // note had it (the Mac's edits do not carry it); a shot of this canvas only.
                let was = self.notes.iter().find(|o| o.id == id && o.sent.is_none()).and_then(|o| o.on.clone());
                let on = n.on.or(was).filter(|o| self.shots.iter().any(|s| &s.id == o));
                out.push(Note { id, n: n.n, x: finite(n.x, -WORLD, WORLD), y: finite(n.y, -WORLD, WORLD), w: finite(n.w, 120.0, 2000.0), h: finite(n.h, 60.0, 2000.0), text: clean(&n.text, MAX_NOTE, true), sent: None, on });
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
                if (s.x - x).abs() >= 0.5 || (s.y - y).abs() >= 0.5 || (s.w - w).abs() >= 0.5 || (s.h - h).abs() >= 0.5 {
                    s.moved = true;
                }
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
                return Err(format!("example {} was not copied into the sandbox", a.n));
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
        out.push_str(&format!("Review round {n} of canvas \"{}\" ({}): {} pin(s), {} note(s), {} example picture(s).\n", self.title, self.id, r.pins, r.notes, r.assets));
        let mut pins: Vec<&Pin> = self.pins.iter().filter(|p| p.sent == Some(n)).collect();
        pins.sort_by_key(|p| p.n);
        for p in pins {
            let on = match self.shots.iter().find(|s| s.id == p.on) {
                Some(s) => format!("shot \"{}\" (round {}{})", s.title, s.round, s.width.map(|w| format!(", {w} wide")).unwrap_or_default()),
                None => match self.assets.iter().find(|a| a.id == p.on) {
                    Some(a) => format!("example {}", a.n),
                    None => "a removed item".into(),
                },
            };
            out.push_str(&format!("\nPin {} on {on}, at {:.0}% across and {:.0}% down ({}):\n{}\n", p.n, p.fx * 100.0, p.fy * 100.0, hint(p.fx, p.fy), quote(&p.text)));
        }
        let mut notes: Vec<&Note> = self.notes.iter().filter(|x| x.sent == Some(n)).collect();
        notes.sort_by_key(|x| x.n);
        for x in notes {
            let near = match self.note_shot(x).and_then(|id| self.shots.iter().find(|s| s.id == id)) {
                Some(s) if x.on.as_deref() == Some(s.id.as_str()) => format!(" (about shot \"{}\")", s.title),
                Some(s) => format!(" (next to shot \"{}\")", s.title),
                None => String::new(),
            };
            out.push_str(&format!("\nNote {}{near}:\n{}\n", x.n, quote(&x.text)));
        }
        for a in self.assets.iter().filter(|a| a.sent == Some(n)) {
            let near = self.nearest_shot(a.x + a.w / 2.0, a.y + a.h / 2.0).map(|s| format!(", placed next to shot \"{}\"", s.title)).unwrap_or_default();
            out.push_str(&format!("\nExample {}: a picture the person added as an example ({}x{} px{near}), copied to {}\n", a.n, a.px_w, a.px_h, a.sandbox_path.as_deref().unwrap_or("(not copied)")));
        }
        out
    }

    /// The shot a note is about: the one it names, else the nearest one.
    pub fn note_shot(&self, n: &Note) -> Option<&str> {
        if let Some(s) = n.on.as_deref().and_then(|o| self.shots.iter().find(|s| s.id == o)) {
            return Some(s.id.as_str());
        }
        self.nearest_shot(n.x + n.w / 2.0, n.y + n.h / 2.0).map(|s| s.id.as_str())
    }

    /// Where a new note about shot `on` goes on the surface: under the
    /// shot, centred, below the notes already about it. `None`: no such shot.
    pub fn note_spot(&self, on: &str) -> Option<(f32, f32, f32, f32)> {
        let s = self.shots.iter().find(|s| s.id == on)?;
        let k = self.notes.iter().filter(|n| n.on.as_deref() == Some(on)).count() as f32;
        Some((s.x + (s.w - NOTE_W) / 2.0, s.y + s.h + 24.0 + k * (NOTE_H + 16.0), NOTE_W, NOTE_H))
    }

    /// The next free number of a pin and of a note (sent ones count).
    pub fn next_numbers(&self) -> (u32, u32) {
        (self.pins.iter().map(|p| p.n).max().unwrap_or(0) + 1, self.notes.iter().map(|n| n.n).max().unwrap_or(0) + 1)
    }

    fn nearest_shot(&self, x: f32, y: f32) -> Option<&Shot> {
        self.shots.iter().min_by(|a, b| {
            let d = |s: &Shot| (s.x + s.w / 2.0 - x).hypot(s.y + s.h / 2.0 - y);
            d(a).total_cmp(&d(b))
        })
    }
}

/// A shot's size on the surface: its stated viewport width (else its
/// pixels) wide, its picture's aspect kept.
pub fn auto_size(width: Option<u32>, px_w: u32, px_h: u32) -> (f32, f32) {
    let w = match width.filter(|w| (100..=MAX_SIDE).contains(w)) {
        Some(w) => w as f32,
        None => px_w.max(1) as f32,
    };
    (w, w * px_h as f32 / px_w.max(1) as f32)
}

/// The columns of a round's grid of `cells` cells, each `cell_w` x
/// `cell_h`: the count whose block is closest to [`GRID_ASPECT`], an empty
/// cell costing a little.
pub fn grid_cols(cells: usize, cell_w: f32, cell_h: f32) -> u32 {
    if cells <= 1 || cell_w <= 0.0 || cell_h <= 0.0 {
        return 1;
    }
    let mut best = (f32::MAX, 1);
    for c in 1..=(cells as u32).min(MAX_COLS) {
        let rows = (cells as u32).div_ceil(c);
        let w = c as f32 * cell_w + (c - 1) as f32 * COL_GAP;
        let h = rows as f32 * cell_h + (rows - 1) as f32 * GRID_ROW_GAP;
        let empty = (rows * c) as usize - cells;
        let cost = (w / h / GRID_ASPECT).ln().abs() + 0.05 * empty as f32;
        if cost < best.0 - 1e-4 {
            best = (cost, c);
        }
    }
    best.1
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

    fn round_of(c: &mut Canvas, batch: &str, shots: &[(u32, u32, &str)]) -> u32 {
        let r = c.round_for(batch, 1).unwrap();
        for (k, (w, h, screen)) in shots.iter().enumerate() {
            let key = (!screen.is_empty()).then_some(*screen);
            c.add_shot(r, format!("s-{:012}.png", c.shots.len()), hdr(*w * 2, *h * 2), 10, &format!("{screen} {k}"), "", Some(*w), key, 1);
        }
        r
    }

    fn desk(n: usize) -> Vec<(u32, u32, &'static str)> {
        const KEYS: [&str; 24] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x"];
        (0..n).map(|k| (1284, 816, KEYS[k])).collect()
    }

    fn cells(c: &Canvas, round: u32) -> Vec<[u32; 3]> {
        c.shots.iter().filter(|s| s.round == round).map(|s| s.cell.unwrap()).collect()
    }

    #[test]
    fn a_round_is_a_grid_whose_columns_follow_its_shots() {
        for (n, cols) in [(1, 1), (2, 2), (4, 2), (9, 3), (12, 4), (16, 4), (24, 5)] {
            let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
            let r = round_of(&mut c, "b", &desk(n));
            assert_eq!(c.rounds[0].cols, cols, "{n} desktop shots");
            for (k, s) in c.shots.iter().enumerate() {
                let (row, col) = (k as u32 / cols, k as u32 % cols);
                assert_eq!(s.cell, Some([row, col, WHOLE]), "{n} shots, shot {k}");
                assert_eq!((s.x, s.y), (col as f32 * (1284.0 + COL_GAP), row as f32 * (816.0 + GRID_ROW_GAP)), "{n} shots, shot {k}");
            }
            assert_eq!(r, 1);
        }
        // 12 desktop shots: 4 x 3, a block wider than tall, not one row 16 000 points long.
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        round_of(&mut c, "b", &desk(12));
        let right = c.shots.iter().map(|s| s.x + s.w).fold(0.0, f32::max);
        let bottom = c.shots.iter().map(|s| s.y + s.h).fold(0.0, f32::max);
        assert_eq!((right, bottom), (4.0 * 1284.0 + 3.0 * COL_GAP, 3.0 * 816.0 + 2.0 * GRID_ROW_GAP));
        // Phones alone: cells as wide as a phone, more columns.
        assert_eq!(grid_cols(6, 390.0, 844.0), 5);
        assert_eq!(grid_cols(12, 390.0, 844.0), 6);
    }

    #[test]
    fn phones_share_a_cell_beside_desktop_shots() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        let mut shots = desk(8);
        shots.extend([(390, 844, "p1"), (390, 844, "p2"), (390, 844, "p3"), (390, 844, "p4")]);
        round_of(&mut c, "b", &shots);
        // 8 whole cells and two phone cells (3 phones a cell): 10 cells, 4 columns.
        assert_eq!(c.rounds[0].cols, 4);
        let ph: Vec<[u32; 3]> = c.shots[8..].iter().map(|s| s.cell.unwrap()).collect();
        assert_eq!(ph, vec![[2, 0, 0], [2, 0, 1], [2, 0, 2], [2, 1, 0]]);
        let p = &c.shots[9];
        assert_eq!(p.x, 390.0 + NARROW_GAP);
        // The phones' row is as tall as a phone.
        assert_eq!(p.y, 2.0 * (816.0 + GRID_ROW_GAP));
        // Nothing overlaps.
        for a in &c.shots {
            for b in &c.shots {
                if a.id != b.id {
                    assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, "{} over {}", a.title, b.title);
                }
            }
        }
    }

    #[test]
    fn the_same_screen_takes_the_same_cell_in_a_later_round() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        round_of(&mut c, "b1", &desk(12));
        let r1_bottom = c.shots.iter().map(|s| s.y + s.h).fold(0.0, f32::max);
        // Round 2: three screens of round 1, out of order, and a new one.
        round_of(&mut c, "b2", &[(1284, 816, "k"), (1284, 816, "new"), (1284, 816, "b"), (1284, 816, "f")]);
        assert_eq!(c.rounds[1].cols, 4, "the earlier round's columns");
        let r2: Vec<&Shot> = c.shots.iter().filter(|s| s.round == 2).collect();
        let r1 = |k: &str| c.shots.iter().find(|s| s.round == 1 && s.screen.as_deref() == Some(k)).unwrap();
        for s in r2.iter().filter(|s| s.screen.as_deref() != Some("new")) {
            let e = r1(s.screen.as_deref().unwrap());
            assert_eq!(s.cell, e.cell, "{}", s.title);
            assert_eq!(s.x, e.x, "{}: the same column", s.title);
        }
        // Round 1 filled its 12 cells: the new screen goes after them (row 3), never under another screen.
        let new = r2.iter().find(|s| s.screen.as_deref() == Some("new")).unwrap();
        assert_eq!(new.cell, Some([3, 0, WHOLE]));
        let top = r2.iter().map(|s| s.y).fold(f32::MAX, f32::min);
        assert_eq!(top, r1_bottom + ROUND_GAP, "newest round below");
        // Round 3 of two screens of round 2 and one new: round 2 left (0,0) empty, the new one takes it.
        round_of(&mut c, "b3", &[(1284, 816, "new"), (1284, 816, "z")]);
        let r3 = cells(&c, 3);
        assert_eq!(r3, vec![[3, 0, WHOLE], [0, 0, WHOLE]]);
        // Rows 1 and 2 hold nothing in round 3: they take no room.
        let r3: Vec<&Shot> = c.shots.iter().filter(|s| s.round == 3).collect();
        assert_eq!(r3[0].y - r3[1].y, 816.0 + GRID_ROW_GAP);
    }

    #[test]
    fn shots_moved_by_hand_keep_their_place() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        round_of(&mut c, "b", &desk(3));
        let id = c.shots[1].id.clone();
        let (w, h) = (c.shots[1].w, c.shots[1].h);
        c.apply(Edit { layout: vec![Place { id: id.clone(), x: 9000.0, y: -400.0, w, h }], ..Default::default() }, 2).unwrap();
        assert!(c.shots[1].moved);
        assert!(!c.shots[0].moved, "only the shot the person moved");
        // More shots of the same round (10 in all: the grid grows to 4 columns).
        let r = c.round_for("b", 3).unwrap();
        for k in 0..7 {
            c.add_shot(r, format!("s-9{k:011}.png"), hdr(2568, 1632), 10, "more", "", Some(1284), None, 3);
        }
        let s = c.shots.iter().find(|s| s.id == id).unwrap();
        assert_eq!((s.x, s.y), (9000.0, -400.0));
        // Its cell stays its own: no other shot took it.
        assert!(c.shots.iter().filter(|o| o.id != id).all(|o| o.cell != s.cell));
        // Placing the same spot again is not a move.
        let mut d = c.clone();
        let o = d.shots[0].clone();
        d.apply(Edit { layout: vec![Place { id: o.id.clone(), x: o.x, y: o.y, w: o.w, h: o.h }], ..Default::default() }, 4).unwrap();
        assert!(!d.shots[0].moved);
    }

    /// A canvas as the one-row layout left it (before grids).
    fn legacy(rounds: &[usize]) -> Canvas {
        let mut c = Canvas::new(new_id(), "web".into(), "Old", 1);
        for (n, &count) in rounds.iter().enumerate() {
            let n = n as u32 + 1;
            c.rounds.push(Round { n, at_ms: 1, shots: count as u32, batch: format!("b{n}"), ..Default::default() });
            for (k, (w, h, key)) in desk(count).into_iter().enumerate() {
                let (sw, sh) = auto_size(Some(w), w * 2, h * 2);
                c.shots.push(Shot { id: format!("s-{n}{k:03}"), file: format!("s-{n:06}{k:06}.png"), round: n, title: key.into(), screen: Some(key.into()), width: Some(w), px_w: w * 2, px_h: h * 2, w: sw, h: sh, ..Default::default() });
                let i = c.shots.len() - 1;
                let (x, y) = c.legacy_spot(i);
                (c.shots[i].x, c.shots[i].y) = (x, y);
            }
        }
        c
    }

    #[test]
    fn an_old_one_row_canvas_is_laid_out_again_on_load() {
        let mut c = legacy(&[12]);
        assert_eq!(c.shots[11].x, 11.0 * (1284.0 + COL_GAP), "one long row");
        // A note under shot 6 ("f", the sixth), an asset beside the last one, a pin on shot 6.
        let f = c.shots[5].clone();
        c.notes.push(Note { id: "n-0001".into(), n: 1, x: f.x + 100.0, y: f.y + f.h + 40.0, w: NOTE_W, h: NOTE_H, ..Default::default() });
        let l = c.shots[11].clone();
        c.assets.push(Asset { id: "a-0001".into(), n: 1, x: l.x + l.w + 60.0, y: 0.0, w: 400.0, h: 300.0, ..Default::default() });
        c.pins.push(Pin { n: 1, on: f.id.clone(), fx: 0.25, fy: 0.5, ..Default::default() });
        assert!(c.relayout_legacy());
        assert_eq!(c.rounds[0].cols, 4);
        let nf = c.shots[5].clone();
        assert_eq!(nf.cell, Some([1, 1, WHOLE]));
        // The note and the asset kept their place beside their shot; the pin is a fraction of it.
        assert_eq!((c.notes[0].x - nf.x, c.notes[0].y - nf.y), (100.0, f.h + 40.0));
        let nl = &c.shots[11];
        assert_eq!((c.assets[0].x - nl.x, c.assets[0].y - nl.y), (nl.w + 60.0, 0.0), "the asset stays to the right of the last shot");
        assert_eq!((c.pins[0].fx, c.pins[0].fy), (0.25, 0.5));
        // Loaded again: nothing more to do.
        assert!(!c.relayout_legacy());
    }

    #[test]
    fn an_old_round_the_person_arranged_is_left_alone() {
        // Round 1 untouched, round 2 with a shot dragged out of its row.
        let mut c = legacy(&[12, 3]);
        let moved = c.shots[13].id.clone();
        c.shots[13].y += 500.0;
        let r2_before: Vec<(f32, f32)> = c.shots[12..].iter().map(|s| (s.x, s.y)).collect();
        let r1_bottom_before = c.shots[..12].iter().map(|s| s.y + s.h).fold(0.0, f32::max);
        assert!(c.relayout_legacy());
        // Round 1 is a grid now, 3 rows tall: round 2 went down by what round 1 grew, as one block.
        let r1_bottom = c.shots[..12].iter().map(|s| s.y + s.h).fold(0.0, f32::max);
        let grew = r1_bottom - r1_bottom_before;
        assert!(grew > 0.0);
        for (s, (x, y)) in c.shots[12..].iter().zip(r2_before) {
            assert_eq!((s.x, s.y), (x, y + grew), "{}", s.id);
            assert_eq!(s.cell, None);
        }
        assert_eq!(c.rounds[1].cols, 0);
        assert!(c.shots.iter().any(|s| s.id == moved));
        // A one-round canvas with a resized shot: nothing changes.
        let mut d = legacy(&[4]);
        d.shots[2].w *= 0.5;
        d.shots[2].h *= 0.5;
        let same = d.clone();
        assert!(!d.relayout_legacy());
        assert_eq!(d, same);
    }

    #[test]
    fn a_full_round_refuses_more() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        assert_eq!(c.round_for("b1", 1).unwrap(), 1);
        assert_eq!(c.round_for("b1", 1).unwrap(), 1, "the same batch is the same round");
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
        assert!(c.submit("web · main", &[], 4).unwrap_err().contains("example 1"), "an example must have been copied");
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
        let e = Edit { notes: Some(vec![Note { id: "../../x".into(), n: 1, x: f32::NAN, y: f32::INFINITY, w: 1e30, h: -5.0, text: "x".repeat(5000), sent: Some(9), on: None }]), ..Default::default() };
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

    #[test]
    fn a_note_about_a_shot_keeps_it_across_the_macs_edits() {
        let mut c = Canvas::new(new_id(), "web".into(), "Review", 1);
        let r = c.round_for("b", 1).unwrap();
        c.add_shot(r, "s-000000000001.png".into(), hdr(780, 1688), 10, "Home", "", Some(390), None, 1);
        c.add_shot(r, "s-000000000002.png".into(), hdr(780, 1688), 10, "Settings", "", Some(390), None, 1);
        let (a, b) = (c.shots[0].id.clone(), c.shots[1].id.clone());
        assert_eq!(c.next_numbers(), (1, 1));
        let (x, y, w, h) = c.note_spot(&b).unwrap();
        assert!(y > c.shots[1].y + c.shots[1].h && (x + w / 2.0 - (c.shots[1].x + c.shots[1].w / 2.0)).abs() < 1.0, "under its shot, centred");
        assert!(c.note_spot("s-nope").is_none());
        let phone = Note { n: 1, x, y, w, h, text: "About settings".into(), on: Some(b.clone()), ..Default::default() };
        let stray = Note { n: 2, x: 0.0, y: 0.0, w, h, text: "x".into(), on: Some("s-nope".into()), ..Default::default() };
        c.apply(Edit { notes: Some(vec![phone, stray]), ..Default::default() }, 2).unwrap();
        assert_eq!(c.notes[0].on.as_deref(), Some(b.as_str()));
        assert_eq!(c.notes[1].on, None, "only a shot of this canvas");
        assert_eq!(c.note_shot(&c.notes[0]), Some(b.as_str()));
        assert_eq!(c.note_shot(&c.notes[1]), Some(a.as_str()), "a placed note: the nearest shot");
        // The Mac sends the note back without `on`, moved onto the other shot: it keeps it.
        let mut mac = c.notes[0].clone();
        mac.on = None;
        (mac.x, mac.y) = (c.shots[0].x, c.shots[0].y);
        c.apply(Edit { notes: Some(vec![mac]), ..Default::default() }, 3).unwrap();
        assert_eq!(c.notes[0].on.as_deref(), Some(b.as_str()));
        let (_, y2, _, _) = c.note_spot(&b).unwrap();
        assert!(y2 > y, "the next note about it goes below");
        assert_eq!(c.next_numbers(), (1, 2));
        let n = c.submit("web · main", &[], 4).unwrap().n;
        assert!(c.review_text(n).contains("Note 1 (about shot \"Settings\")"), "{}", c.review_text(n));
        // Serialised: absent when none, so older manifests read the same.
        assert!(!serde_json::to_string(&Note::default()).unwrap().contains("\"on\""));
    }
}
