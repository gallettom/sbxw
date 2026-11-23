//! Canvas surfaces: an infinite surface of pictures, pins and notes drawn by
//! the core, under one camera (Rune's `live_offset` and `live_scale`). A
//! feature hands it a [`Scene`] (plain data: items placed in world points,
//! pins on items, notes); the person pans and zooms here, in Rust, without
//! running Luau; presses that mean something (select, move, place a pin, a
//! note, an asset) go back to the feature as [`CanvasEvent`]s.
//!
//! * **World points**: a shot of stated width W is W points wide, so at
//!   100 % one CSS pixel of the captured page is one point here.
//! * **Camera**: `drawn = c + (world - c) * zoom + pan`, `c` the view's
//!   centre (Rune scales about the node's centre). Wheel and two-finger
//!   scroll pan; ⌘/Ctrl + wheel and a pinch zoom about the pointer; a drag on
//!   empty ground (or the middle button anywhere) pans. A pan or a pinch sets
//!   only the live values (`Tx::live_only`): the view is not built again.
//!   Zoom, buttons, fit and 100 % rebuild once a gesture settles, so pins
//!   keep their size on screen and the right picture level is chosen.
//! * **Pictures** ([`load`]): decoded by [`crate::picture::prepare`] (PNG,
//!   JPEG, WebP; size checked before decoding) on a worker thread, never on
//!   the UI's. Three levels: a thumb (384 px a side) and a preview (1280 px)
//!   kept while the canvas is open, and the full picture, asked for
//!   ([`CanvasEvent::NeedFull`]) only for a shot drawn larger than its
//!   preview, at most [`MAX_FULL`] at a time; [`close`] drops them all.
//! * Agent text never reaches this module as a control: captions are text.

use rune::{el, text, Cursor, Drag, El, Live, MagnifyEvent, Phase, PointerButton, Tx, WheelEvent};
use std::cell::RefCell;

/// Diagrams drawn as canvas items (a module of its own, kept beside this one).
#[path = "surface_diagram.rs"]
pub mod diagram;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 8.0;
/// The preview level's longest side, in pixels.
pub const PREVIEW_PX: u32 = 1024;
/// The thumb level's longest side, in pixels.
pub const THUMB_PX: u32 = 384;
/// Full pictures held at once (the ones drawn largest win).
pub const MAX_FULL: usize = 3;
/// What a picture may be here: 20 MB encoded, 8192 px a side.
pub fn limits() -> crate::picture::Limits {
    crate::picture::Limits { max_bytes: 20 << 20, max_side: 8192, max_pixels: 40_000_000 }
}

/// What a press does on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Pin,
    Note,
    Asset,
}

impl Tool {
    pub fn parse(s: &str) -> Option<Tool> {
        Some(match s {
            "select" => Tool::Select,
            "pin" => Tool::Pin,
            "note" => Tool::Note,
            "asset" => Tool::Asset,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Pin => "pin",
            Tool::Note => "note",
            Tool::Asset => "asset",
        }
    }
}

/// What an item shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// A picture (a shot or an asset), by its image id, with lines under it.
    Picture { image: String, caption: Vec<String>, asset: bool },
    /// A note card: the person's text.
    Note { text: String, n: Option<u32> },
    /// A round's label across the surface (not pressable).
    Band { label: String },
    /// A diagram an agent added ([`diagram`]): the item's rectangle is its
    /// content's box, `off` that box's corner in the diagram's own points
    /// (the diagram's (0, 0) sits at `x - off[0], y - off[1]`).
    Diagram { diagram: std::sync::Arc<diagram::Diagram>, off: [f32; 2], title: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Stable id (`shot-<id>`, `asset-<id>`, `note-<id>`, `band-<n>`).
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub body: Body,
    /// The person may move and resize it (not a band; not a past round's).
    pub movable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub n: u32,
    /// The item it sits on.
    pub item: String,
    /// Where on it, as fractions of its width and height.
    pub fx: f32,
    pub fy: f32,
    /// Sent in a past round: drawn muted, not movable.
    pub sent: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scene {
    pub items: Vec<Item>,
    pub pins: Vec<Pin>,
    /// An item id or `pin-<n>`.
    pub selected: Option<String>,
    pub tool: Tool,
    /// The ground the person chose (the canvas's bar).
    pub ground: Ground,
}

/// The canvas's ground, as the person chose it: white (the default), dark
/// (the theme's page colour), or the theme's (dark for the dark themes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ground {
    #[default]
    White,
    Dark,
    Follow,
}

impl Ground {
    pub const ALL: [Ground; 3] = [Ground::White, Ground::Dark, Ground::Follow];
    pub fn parse(s: &str) -> Option<Ground> {
        Ground::ALL.into_iter().find(|g| g.name() == s)
    }
    /// Its name in the state file and in `canvas_ground`.
    pub fn name(self) -> &'static str {
        match self {
            Ground::White => "white",
            Ground::Dark => "dark",
            Ground::Follow => "follow",
        }
    }
    /// Its name for a person.
    pub fn label(self) -> &'static str {
        match self {
            Ground::White => "White",
            Ground::Dark => "Dark",
            Ground::Follow => "Follow theme",
        }
    }
    /// Whether it draws the white ground now (Follow: the active theme's).
    pub fn light(self) -> bool {
        match self {
            Ground::White => true,
            Ground::Dark => false,
            Ground::Follow => crate::theme::mode().light(),
        }
    }
}

// ---------------------------------------------------------------------------
// The dot grid

/// Dots are this many world points apart at 100 %.
pub const DOT_BASE: f32 = 24.0;
/// The spacing on screen stays within these (points): zooming out doubles
/// the world spacing, zooming in halves it.
pub const DOT_MIN: f32 = 16.0;
pub const DOT_MAX: f32 = 32.0;
/// A dot's radius on screen (points), on the white and on the dark ground.
pub const DOT_R_WHITE: f32 = 1.0;
pub const DOT_R_DARK: f32 = 1.25;
/// The most dots one build draws. The 50 % margin around the view (so a pan
/// does not show a bare edge before the next build) shrinks first; on a
/// view too large even without it, the spacing doubles past [`DOT_MAX`].
/// A 2560 x 1440 pt view at 16 pt is about 14,600 dots.
pub const DOT_BUDGET: usize = 16_000;

/// The world spacing of the dots at `zoom`: 24 · 2^k, `k` chosen so it is
/// 16 to 32 pt on screen. None for a zoom that is not a positive number.
pub fn dot_step(zoom: f32) -> Option<f32> {
    if !(zoom.is_finite() && zoom > 0.0) {
        return None;
    }
    let mut s = DOT_BASE;
    while s * zoom < DOT_MIN {
        s *= 2.0;
    }
    while s * zoom > DOT_MAX {
        s /= 2.0;
    }
    Some(s)
}

/// The dots one build draws: in world points, inside `rect` (x, y, w, h,
/// where the element sits), `step` apart, on multiples of `step` so a dot
/// keeps its place across builds.
#[derive(Debug, Clone, PartialEq)]
pub struct Dots {
    pub step: f32,
    pub rect: [f32; 4],
    pub points: Vec<[f32; 2]>,
}

/// Dots on the lines of `step` inside `r`: (columns, rows, first x, first y).
fn lattice(r: [f32; 4], step: f32) -> (usize, usize, f32, f32) {
    let (x0, y0) = ((r[0] / step).ceil() * step, (r[1] / step).ceil() * step);
    let n = |from: f32, to: f32| if to < from { 0 } else { ((to - from) / step).floor() as usize + 1 };
    (n(x0, r[0] + r[2]), n(y0, r[1] + r[3]), x0, y0)
}

/// The dots for camera `cam`: the view plus a margin, within [`DOT_BUDGET`].
pub fn dot_grid(cam: &Camera) -> Option<Dots> {
    let mut step = dot_step(cam.zoom)?;
    let vis = cam.visible();
    if !(vis[2] > 0.0 && vis[3] > 0.0) {
        return None;
    }
    let grow = |m: f32| [vis[0] - vis[2] * m, vis[1] - vis[3] * m, vis[2] * (1.0 + 2.0 * m), vis[3] * (1.0 + 2.0 * m)];
    let count = |r: [f32; 4], step: f32| {
        let (c, rw, _, _) = lattice(r, step);
        c.saturating_mul(rw)
    };
    let rect = loop {
        if let Some(r) = [0.5, 0.25, 0.125, 0.0].into_iter().map(grow).find(|r| count(*r, step) <= DOT_BUDGET) {
            break r;
        }
        step *= 2.0;
    };
    let (cols, rows, x0, y0) = lattice(rect, step);
    let mut points = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        for i in 0..cols {
            points.push([x0 + i as f32 * step, y0 + j as f32 * step]);
        }
    }
    Some(Dots { step, rect, points })
}

/// What the person did that the feature records.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasEvent {
    /// A press selected this (an item id, `pin-<n>`) or nothing.
    Select { target: Option<String> },
    /// An item was dragged to a new place (world points).
    Moved { item: String, x: f32, y: f32 },
    /// An item was resized from its corner (world points, aspect kept for pictures).
    Resized { item: String, w: f32, h: f32 },
    /// The Pin tool pressed a picture here.
    PinAt { item: String, fx: f32, fy: f32 },
    /// A pin was dragged to another point (of the same or another picture).
    PinMoved { n: u32, item: String, fx: f32, fy: f32 },
    /// The Note tool pressed here (world points).
    NoteAt { x: f32, y: f32 },
    /// The Asset tool pressed here (world points).
    AssetAt { x: f32, y: f32 },
    /// This picture is drawn larger than its preview: send its full bytes to [`load`].
    NeedFull { image: String },
    /// The zoom changed by a gesture (its readout, beside the zoom buttons, is the screen's).
    Zoomed { zoom: f32 },
}

// ---------------------------------------------------------------------------
// Camera

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub pan: [f32; 2],
    pub zoom: f32,
    /// The view's size in points.
    pub view: [f32; 2],
}

impl Default for Camera {
    fn default() -> Camera {
        Camera { pan: [0.0, 0.0], zoom: 1.0, view: [800.0, 600.0] }
    }
}

impl Camera {
    fn c(&self) -> [f32; 2] {
        [self.view[0] / 2.0, self.view[1] / 2.0]
    }
    /// The world point under view point `q`.
    pub fn world(&self, q: [f32; 2]) -> [f32; 2] {
        let c = self.c();
        [c[0] + (q[0] - c[0] - self.pan[0]) / self.zoom, c[1] + (q[1] - c[1] - self.pan[1]) / self.zoom]
    }
    /// Where world point `p` shows in the view.
    pub fn screen(&self, p: [f32; 2]) -> [f32; 2] {
        let c = self.c();
        [c[0] + (p[0] - c[0]) * self.zoom + self.pan[0], c[1] + (p[1] - c[1]) * self.zoom + self.pan[1]]
    }
    /// Zoom to `zoom` keeping the world point under view point `q` where it is.
    pub fn zoom_about(&mut self, q: [f32; 2], zoom: f32) {
        let p = self.world(q);
        let c = self.c();
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = [q[0] - c[0] - (p[0] - c[0]) * self.zoom, q[1] - c[1] - (p[1] - c[1]) * self.zoom];
    }
    /// Show world rectangle `r` (x, y, w, h) whole, centred, with a margin, at most `max` zoom.
    pub fn fit(&mut self, r: [f32; 4], max: f32) {
        self.fit_in(r, max, [FIT_MARGIN; 4]);
    }
    /// [`Camera::fit`] leaving `pad` view points (top, right, bottom, left)
    /// around it: room for a round's band above and captions below, which
    /// keep their size on screen.
    pub fn fit_in(&mut self, r: [f32; 4], max: f32, pad: [f32; 4]) {
        if r[2] <= 0.0 || r[3] <= 0.0 {
            return;
        }
        let (aw, ah) = ((self.view[0] - pad[1] - pad[3]).max(1.0), (self.view[1] - pad[0] - pad[2]).max(1.0));
        let z = (aw / r[2]).min(ah / r[3]).clamp(MIN_ZOOM, max.min(MAX_ZOOM));
        // The rectangle's centre at the padded area's centre.
        let c = self.c();
        let (qx, qy) = (pad[3] + aw / 2.0, pad[0] + ah / 2.0);
        self.zoom = z;
        let p = [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        self.pan = [qx - c[0] - (p[0] - c[0]) * z, qy - c[1] - (p[1] - c[1]) * z];
    }
    /// World point `p` in the view's middle at `zoom`.
    pub fn centre_on(&mut self, p: [f32; 2], zoom: f32) {
        let c = self.c();
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = [-(p[0] - c[0]) * self.zoom, -(p[1] - c[1]) * self.zoom];
    }
    /// The world rectangle the view shows.
    pub fn visible(&self) -> [f32; 4] {
        let a = self.world([0.0, 0.0]);
        let b = self.world(self.view);
        [a[0], a[1], b[0] - a[0], b[1] - a[1]]
    }
}

/// The margin a fit leaves on each side, view points.
pub const FIT_MARGIN: f32 = 24.0;
/// What a fit of the whole canvas leaves (top, right, bottom, left): a
/// round's band (about 30 pt) above the first row, the captions (two lines,
/// about 40 pt) under the last.
pub const FIT_PAD: [f32; 4] = [FIT_MARGIN + 32.0, FIT_MARGIN, FIT_MARGIN + 40.0, FIT_MARGIN];

/// A view command from a button or a key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomCmd {
    In,
    Out,
    /// Every item in view.
    Fit,
    /// 100 %, about the view's centre.
    Actual,
}

// ---------------------------------------------------------------------------
// State

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    Pan { pan0: [f32; 2] },
    Move { idx: usize, x0: f32, y0: f32 },
    Resize { idx: usize, w0: f32, h0: f32 },
    Pin { idx: usize },
    /// A press of a tool that acts on release (no drag past 4 pt).
    Tap,
}

struct Canvas {
    scene: Scene,
    cam: Camera,
    live_pan: Live<[f32; 2]>,
    live_zoom: Live<f32>,
    grab: Option<Grab>,
    /// Where a dragged pin is now (world), drawn there until released.
    pin_at: Option<[f32; 2]>,
    /// The first layout fits the scene.
    fitted: bool,
    /// A zoom that has not been rebuilt for yet (pins' size, levels).
    dirty: bool,
    /// The view was laid out (its size is known).
    sized: bool,
}

impl Canvas {
    fn new() -> Canvas {
        Canvas { scene: Scene::default(), cam: Camera::default(), live_pan: Live::new([0.0, 0.0]), live_zoom: Live::new(1.0), grab: None, pin_at: None, fitted: false, dirty: false, sized: false }
    }
    fn set_cam(&mut self, cam: Camera) {
        self.cam = cam;
        self.live_pan.set(cam.pan);
        self.live_zoom.set(cam.zoom);
    }
    fn bounds(&self) -> Option<[f32; 4]> {
        let mut it = self.scene.items.iter().filter(|i| !matches!(i.body, Body::Band { .. }));
        let f = it.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (f.x, f.y, f.x + f.w, f.y + f.h);
        for i in it {
            x0 = x0.min(i.x);
            y0 = y0.min(i.y);
            x1 = x1.max(i.x + i.w);
            y1 = y1.max(i.y + i.h);
        }
        Some([x0, y0, x1 - x0, y1 - y0])
    }
}

thread_local! {
    static CANVASES: RefCell<HashMap<String, Canvas>> = RefCell::new(HashMap::new());
}

fn with<R>(id: &str, f: impl FnOnce(&mut Canvas) -> R) -> R {
    CANVASES.with(|m| f(m.borrow_mut().entry(id.to_string()).or_insert_with(Canvas::new)))
}

/// Bumped when what a canvas draws changes from outside its own gestures
/// (a scene, a zoom command, a picture decoded, a close): a window that
/// draws it elsewhere than where the change ran draws again (`crate::windows`).
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn changed() {
    GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// See [`GEN`].
pub fn generation() -> u64 {
    GEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Replaces what canvas `id` shows (the camera stays where it is).
pub fn set_scene(id: &str, scene: Scene) {
    changed();
    with(id, |c| {
        // The first items fit the view (a canvas laid out before its manifest came).
        let first = c.scene.items.is_empty() && !scene.items.is_empty();
        c.scene = scene;
        if first {
            c.fitted = false;
            if c.sized {
                let mut cam = c.cam;
                if let Some(b) = c.bounds() {
                    cam.fit_in(b, 1.0, FIT_PAD);
                    c.fitted = true;
                }
                c.set_cam(cam);
            }
        }
    });
}

pub fn scene(id: &str) -> Scene {
    with(id, |c| c.scene.clone())
}

pub fn camera(id: &str) -> Camera {
    with(id, |c| c.cam)
}

/// The zoom as the readout shows it ("75%").
pub fn zoom_label(id: &str) -> String {
    format!("{:.0}%", camera(id).zoom * 100.0)
}

pub fn zoom(id: &str, cmd: ZoomCmd) {
    changed();
    with(id, |c| {
        let mut cam = c.cam;
        let mid = cam.c();
        match cmd {
            ZoomCmd::In => cam.zoom_about(mid, cam.zoom * 1.25),
            ZoomCmd::Out => cam.zoom_about(mid, cam.zoom / 1.25),
            ZoomCmd::Actual => cam.zoom_about(mid, 1.0),
            ZoomCmd::Fit => {
                if let Some(b) = c.bounds() {
                    cam.fit_in(b, 1.0, FIT_PAD)
                }
            }
        }
        c.set_cam(cam);
    });
}

/// Brings world rectangle `r` into view (a pin chosen in the side list),
/// zooming in only as far as `max`.
pub fn show(id: &str, r: [f32; 4], max: f32) {
    changed();
    with(id, |c| {
        let mut cam = c.cam;
        let vis = cam.visible();
        let inside = r[0] >= vis[0] && r[1] >= vis[1] && r[0] + r[2] <= vis[0] + vis[2] && r[1] + r[3] <= vis[1] + vis[3];
        if !inside || cam.zoom < 0.25 {
            cam.fit(r, max);
        }
        c.set_cam(cam);
    });
}

/// Forgets canvas `id`: its scene, camera and every picture level.
pub fn close(id: &str) {
    changed();
    CANVASES.with(|m| m.borrow_mut().remove(id));
    let pre = prefix(id);
    pics(|m| m.retain(|k, _| !k.starts_with(&pre)));
}

// ---------------------------------------------------------------------------
// Pictures

/// One picture's levels. `Pixels` are shared handles: an element drawing
/// one keeps its texture; dropping the last handle frees it.
struct Pic {
    size: (u32, u32),
    thumb: rune::Pixels,
    preview: rune::Pixels,
    full: Option<rune::Pixels>,
    asked_full: bool,
    error: Option<String>,
    /// Bytes held, for [`mem`].
    bytes: u64,
}

static PICS: Mutex<Option<HashMap<String, Pic>>> = Mutex::new(None);
static WAKE: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

/// Who to wake when a decode lands (the app's waker; tests none).
pub fn set_wake(w: Arc<dyn Fn() + Send + Sync>) {
    *WAKE.lock().unwrap() = Some(w);
}

fn pics<R>(f: impl FnOnce(&mut HashMap<String, Pic>) -> R) -> R {
    let mut g = PICS.lock().unwrap();
    f(g.get_or_insert_with(HashMap::new))
}

thread_local! {
    /// This UI thread's own space in [`PICS`] (decodes land from worker
    /// threads; each test thread is a UI thread of its own).
    static SPACE: u64 = {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    };
}

fn space() -> u64 {
    SPACE.with(|s| *s)
}

fn pkey(id: &str, image: &str) -> String {
    format!("{}/{id}/{image}", space())
}

/// The prefix of canvas `id`'s pictures in [`PICS`].
fn prefix(id: &str) -> String {
    format!("{}/{id}/", space())
}

/// What decoding gave, or why not.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Ok { w: u32, h: u32, ms: u128 },
    Err(String),
}

/// Decodes `bytes` for picture `image` of canvas `id` on a worker thread:
/// the thumb and preview levels, or (`full`) the full level. Returns at once.
pub fn load(id: &str, image: &str, bytes: Vec<u8>, full: bool) {
    let key = pkey(id, image);
    std::thread::spawn(move || {
        let got = decode_levels(&key, &bytes, full);
        if let Loaded::Err(e) = &got {
            log::warn!("canvas: a picture could not be shown ({} bytes): {e}", bytes.len());
        }
        changed();
        if let Some(w) = WAKE.lock().unwrap().clone() {
            w();
        }
    });
}

/// [`load`] on this thread (tests, the prototype's measures).
pub fn decode_levels(key: &str, bytes: &[u8], full: bool) -> Loaded {
    let t = std::time::Instant::now();
    let p = match crate::picture::prepare(bytes, limits(), None, crate::picture::Encode::Pixels) {
        Ok(p) => p,
        Err(e) => {
            pics(|m| m.entry(key.to_string()).or_insert_with(|| empty_pic((0, 0))).error = Some(e.clone()));
            return Loaded::Err(e);
        }
    };
    let (w, h) = (p.width, p.height);
    if full {
        let px = rune::Pixels::new();
        px.set(w, h, p.rgba);
        pics(|m| {
            let e = m.entry(key.to_string()).or_insert_with(|| empty_pic((w, h)));
            e.bytes += w as u64 * h as u64 * 4;
            e.full = Some(px);
        });
    } else {
        let img = image::RgbaImage::from_raw(w, h, p.rgba).expect("rgba of its size");
        let scaled = |m: u32| -> (u32, u32, Vec<u8>) {
            if w <= m && h <= m {
                return (w, h, img.as_raw().clone());
            }
            let r = image::DynamicImage::ImageRgba8(img.clone()).resize(m, m, image::imageops::FilterType::Triangle).to_rgba8();
            (r.width(), r.height(), r.into_raw())
        };
        let (pw, ph, prev) = scaled(PREVIEW_PX);
        let (tw, th, thumb) = scaled(THUMB_PX);
        let pic = empty_pic((w, h));
        let bytes = (pw as u64 * ph as u64 + tw as u64 * th as u64) * 4;
        pic.preview.set(pw, ph, prev);
        pic.thumb.set(tw, th, thumb);
        pics(|m| {
            m.insert(key.to_string(), Pic { bytes, ..pic });
        });
    }
    Loaded::Ok { w, h, ms: t.elapsed().as_millis() }
}

fn empty_pic(size: (u32, u32)) -> Pic {
    Pic { size, thumb: rune::Pixels::new(), preview: rune::Pixels::new(), full: None, asked_full: false, error: None, bytes: 0 }
}

/// Pixels held by every canvas's pictures, and how many pictures (MEMORY.md).
pub fn mem() -> (u64, usize) {
    let pre = format!("{}/", space());
    pics(|m| {
        let mine: Vec<&Pic> = m.iter().filter(|(k, _)| k.starts_with(&pre)).map(|(_, p)| p).collect();
        (mine.iter().map(|p| p.bytes).sum(), mine.len())
    })
}

/// Full levels held now (tests).
pub fn full_levels(id: &str) -> usize {
    let pre = prefix(id);
    pics(|m| m.iter().filter(|(k, p)| k.starts_with(&pre) && p.full.is_some()).count())
}

/// Which level a picture drawn `drawn_px` device pixels wide uses.
fn level_for(drawn_px: f32) -> u8 {
    if drawn_px <= THUMB_PX as f32 * 1.1 {
        0
    } else if drawn_px <= PREVIEW_PX as f32 * 1.1 {
        1
    } else {
        2
    }
}

/// Keeps the full level only for the pictures that need it (the
/// [`MAX_FULL`] drawn largest among those in view); asks for the missing.
fn plan_levels(id: &str, cv: &Canvas, scale: f32) -> Vec<CanvasEvent> {
    let vis = cv.cam.visible();
    let mut want: Vec<(f32, String)> = cv
        .scene
        .items
        .iter()
        .filter_map(|i| match &i.body {
            Body::Picture { image, .. } => {
                let on = i.x < vis[0] + vis[2] && i.x + i.w > vis[0] && i.y < vis[1] + vis[3] && i.y + i.h > vis[1];
                let px = i.w * cv.cam.zoom * scale;
                (on && level_for(px) == 2).then(|| (px, image.clone()))
            }
            _ => None,
        })
        .collect();
    want.sort_by(|a, b| b.0.total_cmp(&a.0));
    want.truncate(MAX_FULL);
    let keep: Vec<String> = want.iter().map(|(_, k)| pkey(id, k)).collect();
    let pre = prefix(id);
    let mut out = vec![];
    pics(|m| {
        for (k, p) in m.iter_mut() {
            if !k.starts_with(&pre) {
                continue;
            }
            if keep.contains(k) {
                if p.full.is_none() && !p.asked_full && p.error.is_none() {
                    p.asked_full = true;
                    out.push(CanvasEvent::NeedFull { image: k[pre.len()..].to_string() });
                }
            } else if p.full.take().is_some() {
                p.asked_full = false;
                p.bytes = p.bytes.saturating_sub(p.size.0 as u64 * p.size.1 as u64 * 4);
            }
        }
    });
    out
}

fn pixels_for(id: &str, image: &str, drawn_px: f32) -> Option<rune::Pixels> {
    pics(|m| {
        let p = m.get(&pkey(id, image))?;
        let lvl = level_for(drawn_px);
        let ready = |px: &rune::Pixels| px.size().0 > 0;
        let pick = match lvl {
            2 => p.full.clone().filter(ready).or_else(|| Some(p.preview.clone())),
            1 => Some(p.preview.clone()),
            _ => Some(p.thumb.clone()),
        }?;
        if ready(&pick) {
            Some(pick)
        } else if ready(&p.preview) {
            Some(p.preview.clone())
        } else {
            None
        }
    })
}

fn pic_error(id: &str, image: &str) -> Option<String> {
    pics(|m| m.get(&pkey(id, image)).and_then(|p| p.error.clone()))
}

// ---------------------------------------------------------------------------
// Hit tests (world points)

/// A pin's radius on screen, points.
pub const PIN_R: f32 = 11.0;
/// A caption's or a band's text: a fixed size on screen, as map labels,
/// but at most this tall in world points, so a row's titles and the next
/// round's band fit in the gap between rounds (`ROUND_GAP`, 480) and a
/// row's titles in the gap between rows (`GRID_ROW_GAP`, 200) at any zoom.
const CAPTION_MAX: f32 = 100.0;
/// The resize handle: a square on the selected item's bottom-right corner,
/// its side a share ([`HANDLE_SHARE`]) of the item's drawn size, at most
/// [`HANDLE_MAX`] points, and no handle at all when that would be under
/// [`HANDLE_MIN`] (an item drawn about 80 points or less: zoom in to resize).
/// Its hit area is [`HANDLE_HIT`] points around the corner, whatever its size.
pub const HANDLE_SHARE: f32 = 0.08;
pub const HANDLE_MAX: f32 = 12.0;
pub const HANDLE_MIN: f32 = 6.0;
pub const HANDLE_HIT: f32 = 9.0;
/// The selection ring: this far outside the item, this thick (points).
const RING_GAP: f32 = 3.0;
const RING_W: f32 = 2.0;
/// A picture's mat: this many points around it on screen, at most
/// [`MAT_MAX`] world points (so it never fills the gap between shots).
const MAT: f32 = 4.0;
const MAT_MAX: f32 = 24.0;

/// The handle's side on screen for an item drawn `w` x `h` points, or
/// `None` when the item is drawn too small to carry one.
pub fn handle_side(w: f32, h: f32) -> Option<f32> {
    let side = (w.min(h) * HANDLE_SHARE).min(HANDLE_MAX);
    (side >= HANDLE_MIN).then_some(side)
}

fn pin_world(sc: &Scene, p: &Pin) -> Option<[f32; 2]> {
    let i = sc.items.iter().find(|i| i.id == p.item)?;
    Some([i.x + p.fx * i.w, i.y + p.fy * i.h])
}

/// What is under world point `w`: a pin, a selected item's handle, an item.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hit {
    Pin(usize),
    Handle(usize),
    Item(usize),
    Ground,
}

fn hit(cv: &Canvas, w: [f32; 2]) -> Hit {
    let z = cv.cam.zoom;
    for (k, p) in cv.scene.pins.iter().enumerate().rev() {
        if let Some(q) = pin_world(&cv.scene, p) {
            if (q[0] - w[0]).hypot(q[1] - w[1]) <= (PIN_R + 2.0) / z {
                return Hit::Pin(k);
            }
        }
    }
    if let Some(sel) = &cv.scene.selected {
        if let Some((k, i)) = cv.scene.items.iter().enumerate().find(|(_, i)| &i.id == sel && i.movable && !matches!(i.body, Body::Diagram { .. })) {
            let (hx, hy) = (i.x + i.w, i.y + i.h);
            if handle_side(i.w * z, i.h * z).is_some() && (w[0] - hx).abs() <= HANDLE_HIT / z && (w[1] - hy).abs() <= HANDLE_HIT / z {
                return Hit::Handle(k);
            }
        }
    }
    for (k, i) in cv.scene.items.iter().enumerate().rev() {
        if matches!(i.body, Body::Band { .. }) {
            continue;
        }
        if w[0] >= i.x && w[0] <= i.x + i.w && w[1] >= i.y && w[1] <= i.y + i.h {
            return Hit::Item(k);
        }
    }
    Hit::Ground
}

/// A picture item under world point `w` (pins go on pictures only).
fn picture_at(sc: &Scene, w: [f32; 2]) -> Option<(String, f32, f32)> {
    sc.items.iter().rev().find_map(|i| {
        let inside = w[0] >= i.x && w[0] <= i.x + i.w && w[1] >= i.y && w[1] <= i.y + i.h;
        (inside && matches!(i.body, Body::Picture { .. })).then(|| (i.id.clone(), ((w[0] - i.x) / i.w).clamp(0.0, 1.0), ((w[1] - i.y) / i.h).clamp(0.0, 1.0)))
    })
}

// ---------------------------------------------------------------------------
// Input

fn on_drag(id: &str, d: &Drag, button: PointerButton) -> (Vec<CanvasEvent>, bool) {
    with(id, |cv| {
        let mut out = vec![];
        let q = [d.x, d.y];
        match d.phase {
            Phase::Down => {
                let w = cv.cam.world(q);
                cv.grab = Some(if button == PointerButton::Middle {
                    Grab::Pan { pan0: cv.cam.pan }
                } else {
                    match (cv.scene.tool, hit(cv, w)) {
                        (_, Hit::Pin(k)) => {
                            out.push(CanvasEvent::Select { target: Some(format!("pin-{}", cv.scene.pins[k].n)) });
                            if cv.scene.pins[k].sent {
                                Grab::Pan { pan0: cv.cam.pan }
                            } else {
                                Grab::Pin { idx: k }
                            }
                        }
                        (Tool::Select, Hit::Handle(k)) => Grab::Resize { idx: k, w0: cv.scene.items[k].w, h0: cv.scene.items[k].h },
                        (Tool::Select, Hit::Item(k)) => {
                            out.push(CanvasEvent::Select { target: Some(cv.scene.items[k].id.clone()) });
                            let i = &cv.scene.items[k];
                            if i.movable {
                                Grab::Move { idx: k, x0: i.x, y0: i.y }
                            } else {
                                Grab::Pan { pan0: cv.cam.pan }
                            }
                        }
                        (Tool::Select, Hit::Ground) => {
                            if cv.scene.selected.is_some() {
                                out.push(CanvasEvent::Select { target: None });
                            }
                            Grab::Pan { pan0: cv.cam.pan }
                        }
                        _ => Grab::Tap,
                    }
                });
                (out, false)
            }
            Phase::Move => {
                let z = cv.cam.zoom;
                match cv.grab {
                    Some(Grab::Pan { pan0 }) => {
                        let mut cam = cv.cam;
                        cam.pan = [pan0[0] + d.dx, pan0[1] + d.dy];
                        cv.set_cam(cam);
                        (out, true)
                    }
                    Some(Grab::Move { idx, x0, y0 }) => {
                        if let Some(i) = cv.scene.items.get_mut(idx) {
                            i.x = x0 + d.dx / z;
                            i.y = y0 + d.dy / z;
                        }
                        (out, false)
                    }
                    Some(Grab::Resize { idx, w0, h0 }) => {
                        if let Some(i) = cv.scene.items.get_mut(idx) {
                            let k = ((w0 + d.dx / z) / w0).max(40.0 / w0);
                            if matches!(i.body, Body::Picture { .. }) {
                                i.w = w0 * k;
                                i.h = h0 * k;
                            } else {
                                i.w = (w0 + d.dx / z).max(80.0);
                                i.h = (h0 + d.dy / z).max(48.0);
                            }
                        }
                        (out, false)
                    }
                    Some(Grab::Pin { .. }) => {
                        cv.pin_at = Some(cv.cam.world(q));
                        (out, false)
                    }
                    _ => (out, true),
                }
            }
            Phase::Up if !d.cancelled() => {
                let moved = d.dx.hypot(d.dy) > 4.0;
                let w = cv.cam.world(q);
                match cv.grab.take() {
                    Some(Grab::Move { idx, .. }) if moved => {
                        if let Some(i) = cv.scene.items.get(idx) {
                            out.push(CanvasEvent::Moved { item: i.id.clone(), x: i.x.round(), y: i.y.round() });
                        }
                    }
                    Some(Grab::Resize { idx, .. }) if moved => {
                        if let Some(i) = cv.scene.items.get(idx) {
                            out.push(CanvasEvent::Resized { item: i.id.clone(), w: i.w.round(), h: i.h.round() });
                        }
                    }
                    Some(Grab::Pin { idx }) => {
                        cv.pin_at = None;
                        if moved {
                            if let (Some(p), Some((item, fx, fy))) = (cv.scene.pins.get(idx), picture_at(&cv.scene, w)) {
                                let n = p.n;
                                if let Some(p) = cv.scene.pins.get_mut(idx) {
                                    (p.item, p.fx, p.fy) = (item.clone(), fx, fy);
                                }
                                out.push(CanvasEvent::PinMoved { n, item, fx, fy });
                            }
                        }
                    }
                    Some(Grab::Tap) if !moved => match cv.scene.tool {
                        Tool::Pin => {
                            if let Some((item, fx, fy)) = picture_at(&cv.scene, w) {
                                out.push(CanvasEvent::PinAt { item, fx, fy });
                            }
                        }
                        Tool::Note => out.push(CanvasEvent::NoteAt { x: w[0].round(), y: w[1].round() }),
                        Tool::Asset => out.push(CanvasEvent::AssetAt { x: w[0].round(), y: w[1].round() }),
                        Tool::Select => {}
                    },
                    _ => {}
                }
                (out, false)
            }
            _ => {
                // Cancelled: put back what moved.
                cv.pin_at = None;
                match cv.grab.take() {
                    Some(Grab::Move { idx, x0, y0 }) => {
                        if let Some(i) = cv.scene.items.get_mut(idx) {
                            (i.x, i.y) = (x0, y0);
                        }
                    }
                    Some(Grab::Resize { idx, w0, h0 }) => {
                        if let Some(i) = cv.scene.items.get_mut(idx) {
                            (i.w, i.h) = (w0, h0);
                        }
                    }
                    _ => {}
                }
                (out, false)
            }
        }
    })
}

/// A wheel delta: true when only the camera moved and the view need not be built.
fn on_wheel(id: &str, w: &WheelEvent) -> bool {
    with(id, |cv| {
        let mut cam = cv.cam;
        let zooming = w.mods.cmd || w.mods.ctrl;
        if zooming {
            cam.zoom_about([w.x, w.y], cam.zoom * (-w.dy * 0.01).exp());
            cv.dirty = true;
            zoomed(cv.cam.zoom, cam.zoom);
        } else {
            cam.pan = [cam.pan[0] + w.dx, cam.pan[1] + w.dy];
        }
        cv.set_cam(cam);
        !zooming
    })
}

fn on_magnify(id: &str, m: &MagnifyEvent) {
    with(id, |cv| {
        let mut cam = cv.cam;
        cam.zoom_about([m.x, m.y], cam.zoom * (1.0 + m.delta));
        zoomed(cv.cam.zoom, cam.zoom);
        cv.set_cam(cam);
        cv.dirty = true;
    })
}

/// A gesture moved the zoom across a whole percent: the feature hears it.
fn zoomed(before: f32, after: f32) {
    if (before * 100.0).round() != (after * 100.0).round() {
        PENDING.with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|e| !matches!(e, CanvasEvent::Zoomed { .. }));
            p.push(CanvasEvent::Zoomed { zoom: after });
        });
    }
}

/// The view was laid out at `size`: the first time, fit the scene.
fn laid_out(id: &str, size: [f32; 2]) -> bool {
    with(id, |cv| {
        if cv.cam.view == size && cv.fitted {
            return false;
        }
        cv.sized = true;
        let mut cam = cv.cam;
        // Keep the world centre where it was when the view changes size.
        let centre = cam.world(cam.c());
        cam.view = size;
        if !cv.fitted {
            cv.fitted = true;
            match cv.bounds() {
                Some(b) => cam.fit_in(b, 1.0, FIT_PAD),
                None => cam.centre_on([size[0] / 2.0, size[1] / 2.0], 1.0),
            }
        } else {
            let z = cam.zoom;
            cam.centre_on(centre, z);
        }
        cv.set_cam(cam);
        true
    })
}

/// Re-fit on the next layout (a canvas opened again, or another one shown).
pub fn refit(id: &str) {
    changed();
    with(id, |cv| cv.fitted = false);
}

// ---------------------------------------------------------------------------
// Drawing

fn small(size: f32, weight: f32) -> rune::TextStyle {
    crate::screen::body(size, weight)
}

/// The element for canvas `id`. `sink` gets what the person did (on the UI thread).
pub fn view<S: 'static>(id: &str, scale: f32, sink: fn(&mut S, &str, CanvasEvent, &mut Tx)) -> El {
    let (need, world, ground) = with(id, |cv| {
        cv.dirty = false;
        let need = plan_levels(id, cv, scale);
        (need, world(id, cv, scale), crate::theme::canvas_c(cv.scene.ground.light(), "canvas-ground"))
    });
    if !need.is_empty() {
        // Asked once each; the feature fetches and calls `load`.
        PENDING.with(|p| p.borrow_mut().extend(need));
    }
    let (i1, i2, i3, i4) = (id.to_string(), id.to_string(), id.to_string(), id.to_string());
    el().key(format!("canvas-{id}"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .fill(ground)
        .clip_soft(0.0)
        .cursor(Cursor::Default)
        .pointer_buttons([PointerButton::Middle])
        .on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
            let before = camera(&i1).zoom;
            if laid_out(&i1, [r[2], r[3]]) {
                tx.wake_after(std::time::Duration::from_millis(1));
                // The first fit changes the zoom the readout shows.
                let after = camera(&i1).zoom;
                if (before * 100.0).round() != (after * 100.0).round() {
                    sink(s, &i1, CanvasEvent::Zoomed { zoom: after }, tx);
                }
            }
        })
        .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            let (evs, live) = on_drag(&i2, d, tx.button());
            if live {
                tx.live_only();
            }
            for ev in evs {
                sink(s, &i2, ev, tx);
            }
            for ev in take_pending() {
                sink(s, &i2, ev, tx);
            }
        })
        .on_wheel(move |s: &mut S, w: WheelEvent, tx: &mut Tx| {
            if on_wheel(&i3, &w) && !matches!(w.phase, rune::GesturePhase::Ended) {
                tx.live_only();
            }
            for ev in take_pending() {
                sink(s, &i3, ev, tx);
            }
            true
        })
        .on_magnify(move |s: &mut S, m: MagnifyEvent, tx: &mut Tx| {
            on_magnify(&i4, &m);
            for ev in take_pending() {
                sink(s, &i4, ev, tx);
            }
            true
        })
        .child(world)
}

thread_local! {
    static PENDING: RefCell<Vec<CanvasEvent>> = const { RefCell::new(Vec::new()) };
}

/// Level requests made while building the view, handed to the feature at the next input.
pub fn take_pending() -> Vec<CanvasEvent> {
    PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// The dot grid for camera `cam`, as one shape: a zero-length round-capped
/// line per dot (one small quad each on the GPU), in world points, sized by
/// `1 / zoom` so a dot is [`DOT_R_WHITE`] (or [`DOT_R_DARK`]) on screen.
fn dots(id: &str, cam: &Camera, light: bool) -> Option<El> {
    let t = crate::perf::start();
    let d = dot_grid(cam)?;
    let inv = 1.0 / cam.zoom;
    let (r, color) = if light {
        (DOT_R_WHITE, crate::theme::canvas_c(true, "canvas-dot"))
    } else {
        (DOT_R_DARK, crate::theme::canvas_c(false, "canvas-dot").alpha(crate::theme::CANVAS_DOT_ALPHA_DARK))
    };
    let (ox, oy, wd) = (d.rect[0], d.rect[1], 2.0 * r * inv);
    let mut shape = rune::Shape::new();
    for p in &d.points {
        let q = [p[0] - ox, p[1] - oy];
        shape = shape.line(q, q, wd, rune::LineCap::Round, color);
    }
    let n = d.points.len();
    let e = rune::canvas(shape).key(format!("canvas-dots-{id}")).pin(Some(ox), Some(oy), None, None).w(d.rect[2]).h(d.rect[3]);
    crate::perf::done(t, || format!("canvas dots: {n} at {} pt", d.step));
    Some(e)
}

/// A diagram's gesture under way, drawn as it goes: a new edge's rubber line
/// (from a node to a point, the diagram's points), a message's new row.
#[allow(clippy::type_complexity)]
fn diagram_drag(_cv: &Canvas, _item: &str) -> (Option<(String, [f32; 2])>, Option<(String, u32)>) {
    (None, None)
}

fn world(id: &str, cv: &Canvas, scale: f32) -> El {
    let z = cv.cam.zoom;
    let inv = 1.0 / z;
    let light = cv.scene.ground.light();
    let k = |name: &str| crate::theme::canvas_c(light, name);
    let mut w = el().key(format!("canvas-world-{id}")).w(cv.cam.view[0]).h(cv.cam.view[1]).live_offset(&cv.live_pan).live_scale(&cv.live_zoom);
    // The dot grid first, under every item, inside the world so a live pan
    // or pinch moves it without a build.
    if let Some(d) = dots(id, &cv.cam, light) {
        w = w.child(d);
    }
    let vis = cv.cam.visible();
    // A margin so a pan does not show a blank edge before the next build.
    let (mx, my) = (vis[2] * 0.5, vis[3] * 0.5);
    for i in &cv.scene.items {
        if let Body::Band { label } = &i.body {
            // The label stays readable at any zoom and always sits above
            // its row (`i.y` is the row's top): its height is taken above it.
            let fs = (13.0 * inv).min(CAPTION_MAX);
            let lh = (fs * 1.5).ceil();
            w = w.child(
                el().key(format!("canvas-{}", i.id))
                    .pin(Some(i.x), Some(i.y - lh - (10.0 * inv).min(16.0)), None, None)
                    .w(i.w)
                    .h(lh + 4.0 * inv)
                    .child(text(label.clone()).font(small(fs, 600.0)).color(k("canvas-text-muted")).truncate().w(i.w))
                    .child(el().pin(Some(0.0), Some(lh + 2.0 * inv), None, None).w(i.w).h(inv).fill(k("canvas-line"))),
            );
            continue;
        }
        let on = i.x < vis[0] + vis[2] + mx && i.x + i.w > vis[0] - mx && i.y < vis[1] + vis[3] + my && i.y + i.h > vis[1] - my;
        let selected = cv.scene.selected.as_deref() == Some(&i.id);
        if let Body::Diagram { diagram: d, off, title } = &i.body {
            // Out of view (and the margin): nothing built.
            if !on {
                continue;
            }
            let sel = cv.scene.selected.as_deref().and_then(diagram::parse_sel).filter(|(did, _)| *did == i.id).map(|(_, p)| p);
            if sel == Some(diagram::Part::Whole) {
                let o = RING_GAP * inv + 8.0;
                w = w.child(el().key(format!("canvas-ring-{}", i.id)).pin(Some(i.x - o), Some(i.y - o), None, None).w(i.w + 2.0 * o).h(i.h + 2.0 * o).border(RING_W * inv, k("canvas-accent")).radius(6.0));
            }
            let (rubber, row_drag) = diagram_drag(cv, &i.id);
            let look = diagram::Look { id: &i.id, d, origin: [i.x - off[0], i.y - off[1]], zoom: z, light, sel: sel.as_ref(), rubber: rubber.as_ref().map(|(f, p)| (f.as_str(), *p)), row_drag: row_drag.as_ref().map(|(e, r)| (e.as_str(), *r)), title };
            for e in diagram::draw(&look) {
                w = w.child(e);
            }
            continue;
        }
        if matches!(i.body, Body::Picture { .. }) {
            // A mat a shade lighter than the ground, edged by a hairline: dark
            // screens stand apart from the surface and from each other.
            let m = (MAT * inv).min(MAT_MAX);
            w = w.child(el().key(format!("canvas-mat-{}", i.id)).pin(Some(i.x - m), Some(i.y - m), None, None).w(i.w + 2.0 * m).h(i.h + 2.0 * m).fill(k("canvas-mat")).border(inv, k("canvas-mat-edge")).radius(3.0 * inv));
        }
        if selected {
            // The selection ring, outside the item (and its mat): it reads on any picture, at any zoom.
            let o = (MAT * inv).min(MAT_MAX) + RING_GAP * inv;
            w = w.child(el().key(format!("canvas-ring-{}", i.id)).pin(Some(i.x - o), Some(i.y - o), None, None).w(i.w + 2.0 * o).h(i.h + 2.0 * o).border(RING_W * inv, k("canvas-accent")).radius(4.0 * inv));
        }
        let mut b = el().key(format!("canvas-{}", i.id)).pin(Some(i.x), Some(i.y), None, None).w(i.w).h(i.h);
        match &i.body {
            Body::Picture { image, caption, asset } => {
                b = b.fill(k("canvas-picture")).radius(2.0 * inv);
                if on {
                    if let Some(px) = pixels_for(id, image, i.w * z * scale) {
                        b = b.pixels_fit(&px, rune::SurfaceFit::Fill);
                    } else if let Some(e) = pic_error(id, image) {
                        b = b.child(text(format!("This picture cannot be shown: {e}")).font(small(12.0 * inv, 400.0)).color(k("canvas-text-muted")).wrap().w(i.w));
                    }
                }
                b = b.border(inv, if selected { k("canvas-accent") } else if *asset { k("canvas-asset") } else { k("canvas-frame") });
                // The caption under the picture, at a size that stays readable.
                // Zoomed far out, the title alone, and never taller than the gap between rows.
                let mut cap = el().pin(Some(0.0), Some(i.h + (6.0 * inv).min(16.0)), None, None).w(i.w);
                let mut lines = rune::col().gap((2.0 * inv).min(6.0));
                let far = z < 0.25;
                for (line_no, l) in caption.iter().enumerate().take(if far { 1 } else { 2 }) {
                    let (sz, wt, col) = if line_no == 0 { (12.0, 600.0, "canvas-text") } else { (11.0, 400.0, "canvas-text-muted") };
                    lines = lines.child(text(l.clone()).font(small((sz * inv).min(CAPTION_MAX), wt)).color(k(col)).truncate().w(i.w));
                }
                cap = cap.child(lines);
                b = b.child(cap);
            }
            Body::Note { text: t, n } => {
                // A note is a thing on the surface: its text scales with it.
                b = b
                    .fill(k("canvas-note"))
                    .border(if selected { 2.0 * inv } else { 1.0 * inv }, if selected { k("canvas-accent") } else { k("canvas-note-edge") })
                    .radius(crate::theme::corner::CONTROL)
                    .padding(crate::theme::spacing::MD)

                    .child(
                        rune::col().gap(4.0).w(i.w - 20.0)
                            .child(text(match n { Some(n) => format!("Note {n}"), None => "Note".into() }).font(small(12.0, 600.0)).color(k("canvas-text-muted")))
                            .child(text(if t.trim().is_empty() { "(no text yet)".to_string() } else { t.clone() }).font(small(14.0, 400.0)).color(k("canvas-text")).wrap().w(i.w - 20.0)),
                    );
            }
            Body::Band { .. } | Body::Diagram { .. } => {}
        }
        if let Some(side) = handle_side(i.w * z, i.h * z).filter(|_| selected && i.movable) {
            let hs = side / 2.0 * inv;
            b = b.child(el().key(format!("canvas-handle-{}", i.id)).pin(Some(i.w - hs), Some(i.h - hs), None, None).w(2.0 * hs).h(2.0 * hs).radius(inv).fill(k("canvas-accent")).border(inv, k("canvas-handle-edge")));
        }
        w = w.child(b);
    }
    for p in &cv.scene.pins {
        let mut at = match pin_world(&cv.scene, p) {
            Some(a) => a,
            None => continue,
        };
        let grabbed = matches!(cv.grab, Some(Grab::Pin { idx }) if cv.scene.pins.get(idx).map(|q| q.n) == Some(p.n));
        if grabbed {
            if let Some(a) = cv.pin_at {
                at = a;
            }
        }
        let selected = cv.scene.selected.as_deref() == Some(&format!("pin-{}", p.n));
        let r = PIN_R * inv;
        let fill = if p.sent { k("canvas-pin-sent") } else if selected { k("canvas-pin-selected") } else { k("canvas-pin") };
        w = w.child(
            el().key(format!("canvas-pin-{}", p.n))
                .pin(Some(at[0] - r), Some(at[1] - r), None, None)
                .w(2.0 * r)
                .h(2.0 * r)
                .radius(r)
                .fill(fill)
                .border(2.0 * inv, if selected { k("canvas-pin-ring-selected") } else { k("canvas-pin-ring") })
                .items_center()
                .justify_center()
                .child(text(p.n.to_string()).font(small(11.0 * inv, 600.0)).color(k("canvas-pin-text"))),
        );
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
    }

    #[test]
    fn zooming_about_a_point_keeps_it_under_the_pointer() {
        let mut cam = Camera { pan: [13.0, -7.0], zoom: 0.8, view: [1000.0, 700.0] };
        let q = [312.0, 455.0];
        let p = cam.world(q);
        for z in [0.05, 0.3, 1.0, 2.5, 8.0] {
            cam.zoom_about(q, z);
            assert!(near(cam.screen(p), q), "zoom {z}: {:?}", cam.screen(p));
            assert!(near(cam.world(q), p));
        }
        cam.zoom_about(q, 100.0);
        assert_eq!(cam.zoom, MAX_ZOOM);
    }

    #[test]
    fn fit_shows_the_rectangle_whole() {
        let mut cam = Camera { view: [900.0, 600.0], ..Camera::default() };
        let r = [100.0, 50.0, 4000.0, 2000.0];
        cam.fit(r, 1.0);
        let vis = cam.visible();
        assert!(vis[0] <= r[0] && vis[1] <= r[1] && vis[0] + vis[2] >= r[0] + r[2] && vis[1] + vis[3] >= r[1] + r[3], "{vis:?}");
        // A small thing is not blown up past 100 %.
        cam.fit([0.0, 0.0, 50.0, 50.0], 1.0);
        assert_eq!(cam.zoom, 1.0);
    }

    #[test]
    fn levels_follow_the_drawn_size() {
        assert_eq!(level_for(200.0), 0);
        assert_eq!(level_for(900.0), 1);
        assert_eq!(level_for(2880.0), 2);
    }

    /// The dots are 16 to 32 pt apart on screen at every zoom, on a power
    /// of two times 24 world points.
    #[test]
    fn the_dots_stay_16_to_32_pt_apart_on_screen_at_any_zoom() {
        let mut z = MIN_ZOOM;
        while z <= MAX_ZOOM {
            let s = dot_step(z).unwrap();
            assert!((DOT_MIN..=DOT_MAX).contains(&(s * z)), "zoom {z}: {s} world = {} pt", s * z);
            let k = (s / DOT_BASE).log2();
            assert!((k - k.round()).abs() < 1e-4, "zoom {z}: {s} is 24 · 2^k");
            z *= 1.07;
        }
        assert_eq!(dot_step(1.0), Some(24.0));
        assert_eq!(dot_step(0.5), Some(48.0), "zoomed out: doubled");
        assert_eq!(dot_step(0.25), Some(96.0));
        assert_eq!(dot_step(2.0), Some(12.0), "zoomed in: halved");
        assert_eq!(dot_step(8.0), Some(3.0));
        assert_eq!(dot_step(0.0), None);
        assert_eq!(dot_step(f32::NAN), None);
    }

    /// Only the dots in view (and the margin) are drawn: a known view counts
    /// exactly, every dot sits in the element's rect on a multiple of the
    /// spacing, and no view draws more than the budget.
    #[test]
    fn only_the_dots_in_view_are_drawn() {
        // 800 x 600 at 100 %: the view [0, 0, 800, 600] plus 50 % each side,
        // [-400, -300, 1600, 1200]; columns -384..=1200 (67), rows -288..=888 (50).
        let cam = Camera { pan: [0.0, 0.0], zoom: 1.0, view: [800.0, 600.0] };
        let d = dot_grid(&cam).unwrap();
        assert_eq!(d.step, 24.0);
        assert_eq!(d.rect, [-400.0, -300.0, 1600.0, 1200.0]);
        assert_eq!(d.points.len(), 67 * 50);
        for p in &d.points {
            assert!(p[0] >= d.rect[0] && p[0] <= d.rect[0] + d.rect[2] && p[1] >= d.rect[1] && p[1] <= d.rect[1] + d.rect[3], "{p:?}");
            assert!((p[0] / d.step).fract() == 0.0 && (p[1] / d.step).fract() == 0.0, "{p:?} on the lattice");
        }
        // Panned and zoomed: the same rule, the lattice of that zoom.
        let mut cam = Camera { pan: [0.0, 0.0], zoom: 1.0, view: [800.0, 600.0] };
        cam.zoom_about([123.0, 45.0], 0.3);
        cam.pan[0] += 777.0;
        let d = dot_grid(&cam).unwrap();
        assert_eq!(d.step, 96.0);
        let vis = cam.visible();
        let (cols, rows, _, _) = lattice(d.rect, d.step);
        assert_eq!(d.points.len(), cols * rows);
        assert!(d.rect[0] <= vis[0] && d.rect[0] + d.rect[2] >= vis[0] + vis[2], "it covers the view");
        // A full-screen 5K view (2560 x 1440 pt) at the densest spacing: the
        // margin shrinks, the count stays in the budget.
        for z in [16.0 / 24.0, 1.0, 0.7, 0.02, 8.0] {
            let cam = Camera { pan: [0.0, 0.0], zoom: z, view: [2560.0, 1440.0] };
            let d = dot_grid(&cam).unwrap();
            assert!(d.points.len() <= DOT_BUDGET, "zoom {z}: {}", d.points.len());
            assert!((DOT_MIN..=DOT_MAX).contains(&(d.step * z)), "zoom {z}: still in the band");
            let vis = cam.visible();
            assert!(d.rect[2] >= vis[2] - 0.01 && d.rect[3] >= vis[3] - 0.01, "zoom {z}: the whole view");
        }
        // A view larger than any display: the spacing doubles past the band
        // rather than the count passing the budget.
        let cam = Camera { pan: [0.0, 0.0], zoom: 16.0 / 24.0, view: [6000.0, 4000.0] };
        let d = dot_grid(&cam).unwrap();
        assert!(d.points.len() <= DOT_BUDGET && d.step * cam.zoom > DOT_MAX, "{} dots at {} pt", d.points.len(), d.step * cam.zoom);
    }

    /// The cost of a build's dots on the CPU, the worst case (a 2560 x 1440
    /// pt view at 16 pt): the grid and its shape. Prints the numbers
    /// (`cargo test dots_cost -- --nocapture`); the bound is loose (a
    /// debug build on a loaded machine), the GPU side is for a Mac.
    #[test]
    fn dots_cost_of_the_worst_case() {
        let cam = Camera { pan: [0.0, 0.0], zoom: 16.0 / 24.0, view: [2560.0, 1440.0] };
        let runs = 20;
        let t = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..runs {
            let el = dots("canvas", &cam, true).unwrap();
            n = dot_grid(&cam).unwrap().points.len();
            std::hint::black_box(el);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!("canvas dots: {n} dots, {ms:.2} ms a build (grid + shape + element)");
        assert!(n > 10_000 && n <= DOT_BUDGET, "{n}");
        assert!(ms < 250.0, "{ms:.1} ms a build");
    }
}
