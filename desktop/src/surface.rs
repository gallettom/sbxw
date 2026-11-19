//! Picture surfaces: a remote picture (a sandbox's browser, a stream) drawn
//! by the core, with the person's pointer and keys handed back to the
//! feature's Rust in the PICTURE's own pixels. Luau never sees a frame or an
//! input event: a screen places the surface with `surface{ id = "..." }` and
//! draws only the chrome around it.
//!
//! * **Frames**: a feature's Rust, on any thread (its socket's), calls
//!   [`set_rgba`] with decoded pixels or [`set_encoded`] with a PNG, JPEG or
//!   WebP (decoded by [`crate::picture::decode_rgba`] with [`frame_limits`],
//!   on the caller's thread). Rune redraws the element on its next frame
//!   without running Luau again; frames set between two draws coalesce (the
//!   newest wins). A surface is owned by the feature that lists its id in
//!   [`crate::feat::Feature::surfaces`].
//! * **Placement**: scaled to fit, aspect ratio kept, letterboxed and
//!   centred ([`Fit`]). The element grows into the space its parent gives.
//! * **Events** ([`SurfaceEvent`]) go to the owner's
//!   [`crate::feat::Feature::on_surface`] on the UI thread: pointer move
//!   (with the buttons held), press and release (button, click count),
//!   wheel, keys down and up (DOM-style key name, physical code, text,
//!   modifiers), text from the input method, a paste (text only, from the
//!   person's own ⌘V / Ctrl+V), focus in and out, and the shown size
//!   (debounced). Pointer events outside the picture (in the letterbox) are
//!   dropped, except the moves and the release of a drag that started on
//!   it, which are clamped to its edge so a button never stays held.
//! * **Reserved keys**, kept by the app and never sent ([`reserved`]): the
//!   features' global shortcuts, the push-to-talk key, F6 (the feature hears
//!   [`SurfaceEvent::F6`], to move the focus to its own field), Cmd/Ctrl with
//!   `+ - = 0`, and the release key [`RELEASE_KEY`] (⌘Esc on macOS, Ctrl+Esc
//!   elsewhere), which always gives the keyboard back to the app. Escape
//!   alone goes to the picture.

use rune::{Key, KeyEvent, Mods, Tx};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Shown in the chrome: the key that gives the keyboard back to the app.
pub const RELEASE_KEY: &str = if cfg!(target_os = "macos") { "⌘Esc" } else { "Ctrl+Esc" };

/// How long the shown size must hold before [`SurfaceEvent::Resized`] goes out.
pub const RESIZE_DEBOUNCE: Duration = Duration::from_millis(200);

/// What a frame may be: 8 MB encoded, 4096 px a side, 16.8 megapixels
/// (2560x1600 is 4.1).
pub fn frame_limits() -> crate::picture::Limits {
    crate::picture::Limits { max_bytes: 8 << 20, max_side: 4096, max_pixels: 4096 * 4096 }
}

// ---------------------------------------------------------------------------
// Mapping

/// Where a picture of `pic` pixels shows in a view of `view` points: scaled
/// to fit, aspect kept, centred. All in the view's points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Points per picture pixel.
    pub scale: f32,
    /// The picture's top-left in the view.
    pub x: f32,
    pub y: f32,
    /// The picture's shown size.
    pub w: f32,
    pub h: f32,
    pub pic: (u32, u32),
}

impl Fit {
    pub fn new(view: (f32, f32), pic: (u32, u32)) -> Option<Fit> {
        let (vw, vh) = view;
        let (pw, ph) = (pic.0 as f32, pic.1 as f32);
        if !(vw > 0.0 && vh > 0.0 && pw > 0.0 && ph > 0.0) {
            return None;
        }
        let scale = (vw / pw).min(vh / ph);
        let (w, h) = (pw * scale, ph * scale);
        Some(Fit { scale, x: (vw - w) / 2.0, y: (vh - h) / 2.0, w, h, pic })
    }

    /// A point of the view in picture pixels; `None` outside the picture.
    pub fn to_picture(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (px, py) = ((x - self.x) / self.scale, (y - self.y) / self.scale);
        let (pw, ph) = (self.pic.0 as f32, self.pic.1 as f32);
        (px >= 0.0 && py >= 0.0 && px < pw && py < ph).then_some((px, py))
    }

    /// The same, clamped to the picture's edge (a drag leaving it).
    pub fn to_picture_clamped(&self, x: f32, y: f32) -> (f32, f32) {
        let (px, py) = ((x - self.x) / self.scale, (y - self.y) / self.scale);
        let (pw, ph) = (self.pic.0 as f32, self.pic.1 as f32);
        (px.clamp(0.0, (pw - 1.0).max(0.0)), py.clamp(0.0, (ph - 1.0).max(0.0)))
    }
}

// ---------------------------------------------------------------------------
// Events

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    /// The DOM `MouseEvent.button` number.
    pub fn dom(self) -> u8 {
        match self {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
        }
    }
    fn bit(self) -> u8 {
        1 << self.dom()
    }
}

/// Modifiers on every event, DOM names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mod {
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
}

impl From<Mods> for Mod {
    fn from(m: Mods) -> Mod {
        // Rune's `cmd` is ⌘ on macOS (also `logo`), Ctrl elsewhere (also `ctrl`).
        Mod { alt: m.alt, ctrl: m.ctrl, meta: m.logo || (cfg!(target_os = "macos") && m.cmd), shift: m.shift }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceEvent {
    /// The pointer moved over the picture; `buttons` held (DOM bit mask: 1 left, 2 right, 4 middle).
    Move { surface: String, x: f32, y: f32, buttons: u8, mods: Mod },
    Press { surface: String, x: f32, y: f32, button: Button, clicks: u32, mods: Mod },
    Release { surface: String, x: f32, y: f32, button: Button, mods: Mod },
    /// DOM `WheelEvent` convention: positive `dy` scrolls down. In points.
    Wheel { surface: String, x: f32, y: f32, dx: f32, dy: f32, mods: Mod },
    /// A key while the surface has the keyboard. `key`: DOM `KeyboardEvent.key`
    /// (`a`, `A`, `Enter`, `ArrowLeft`, `F5`); `code`: the physical key
    /// (`KeyA`, `Enter`) when the platform says it; `text`: what it typed,
    /// if anything (no Ctrl or ⌘ held). Key-up events carry no text.
    Key { surface: String, down: bool, key: String, code: Option<String>, text: Option<String>, mods: Mod },
    /// Text the input method committed (dead keys, CJK, emoji).
    Text { surface: String, text: String },
    /// The person pasted (⌘V, Ctrl+V): the clipboard's text.
    Paste { surface: String, text: String },
    /// The surface took the keyboard (`true`) or gave it back.
    Focus { surface: String, focused: bool },
    /// F6 on the surface: the feature may move the focus to its own field ([`crate::feat::Action::Focus`]).
    F6 { surface: String },
    /// The shown area's size in points, logical pixels (debounced).
    Resized { surface: String, w: f32, h: f32 },
}

impl SurfaceEvent {
    pub fn surface(&self) -> &str {
        match self {
            SurfaceEvent::Move { surface, .. }
            | SurfaceEvent::Press { surface, .. }
            | SurfaceEvent::Release { surface, .. }
            | SurfaceEvent::Wheel { surface, .. }
            | SurfaceEvent::Key { surface, .. }
            | SurfaceEvent::Text { surface, .. }
            | SurfaceEvent::Paste { surface, .. }
            | SurfaceEvent::Focus { surface, .. }
            | SurfaceEvent::F6 { surface }
            | SurfaceEvent::Resized { surface, .. } => surface,
        }
    }
}

/// The DOM `KeyboardEvent.key` of a Rune key.
pub fn dom_key(k: &Key, shift: bool) -> String {
    match k {
        Key::Text(t) => t.clone(),
        Key::Command(c) => if shift { c.to_ascii_uppercase() } else { *c }.to_string(),
        Key::Enter => "Enter".into(),
        Key::Backspace => "Backspace".into(),
        Key::Delete => "Delete".into(),
        Key::Left => "ArrowLeft".into(),
        Key::Right => "ArrowRight".into(),
        Key::Up => "ArrowUp".into(),
        Key::Down => "ArrowDown".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        Key::Insert => "Insert".into(),
        Key::Escape => "Escape".into(),
        Key::Tab => "Tab".into(),
        Key::Function(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

/// The physical key's DOM `code` (`KeyA`, `Digit1`, `Enter`), from Rune's.
pub fn dom_code(c: Option<rune::PhysicalKeyCode>) -> Option<String> {
    c.map(|c| format!("{c:?}"))
}

/// Whether the app keeps this key while a surface has the keyboard. `ptt`:
/// the push-to-talk key; `shortcut`: the features' global shortcuts said yes.
pub fn reserved(ev: &KeyEvent, ptt: &[Key], shortcut: bool) -> bool {
    let m = ev.mods;
    if shortcut || is_release(ev) || ev.key == Key::Function(6) {
        return true;
    }
    if !(m.cmd || m.ctrl || m.alt || m.logo) && ptt.contains(&ev.key) {
        return true;
    }
    // The app's font size, as in a terminal.
    m.cmd && matches!(&ev.key, Key::Command('+' | '-' | '=' | '0') | Key::Text(_)) && matches!(dom_key(&ev.key, false).as_str(), "+" | "-" | "=" | "0")
}

/// ⌘Esc (Ctrl+Esc off macOS): gives the keyboard back.
pub fn is_release(ev: &KeyEvent) -> bool {
    ev.key == Key::Escape && ev.mods.cmd
}

/// Whether this is the paste chord (⌘V, Ctrl+V, Shift+Insert off macOS).
pub fn is_paste(ev: &KeyEvent) -> bool {
    matches!(ev.key, Key::Command('v')) && ev.mods.cmd || (!cfg!(target_os = "macos") && ev.key == Key::Insert && ev.mods.shift)
}

// ---------------------------------------------------------------------------
// The registry: pixels by surface id, shared with features' threads

struct Entry {
    pixels: rune::Pixels,
    pic: (u32, u32),
    /// The view's size in points, as last laid out.
    view: (f32, f32),
    /// Buttons held in a drag that started on the picture.
    held: u8,
    /// The size not yet told, and since when it holds.
    resize: Option<((f32, f32), Instant)>,
    told: (f32, f32),
}

static SURFACES: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);
/// The surface that has the keyboard (set by the shell from Rune's focus).
static FOCUSED: Mutex<Option<String>> = Mutex::new(None);

fn with<R>(id: &str, f: impl FnOnce(&mut Entry) -> R) -> R {
    let mut g = SURFACES.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    let e = m.entry(id.to_string()).or_insert_with(|| Entry { pixels: rune::Pixels::new(), pic: (0, 0), view: (0.0, 0.0), held: 0, resize: None, told: (0.0, 0.0) });
    f(e)
}

/// Bytes of the surfaces' last frames, and how many surfaces (MEMORY.md).
pub fn mem() -> (u64, usize) {
    let g = SURFACES.lock().unwrap();
    g.as_ref().map_or((0, 0), |m| (m.values().map(|e| e.pic.0 as u64 * e.pic.1 as u64 * 4).sum(), m.len()))
}

/// Shows a decoded frame (`w * h * 4` bytes of RGBA, rows top first). Any thread.
pub fn set_rgba(id: &str, w: u32, h: u32, rgba: Vec<u8>) -> Result<(), String> {
    let l = frame_limits();
    if w == 0 || h == 0 || w > l.max_side || h > l.max_side || (w as u64) * (h as u64) > l.max_pixels {
        return Err(format!("a frame of {w}x{h} pixels is not taken"));
    }
    if rgba.len() as u64 != (w as u64) * (h as u64) * 4 {
        return Err(format!("a frame of {w}x{h} needs {} bytes, got {}", w as u64 * h as u64 * 4, rgba.len()));
    }
    let px = with(id, |e| {
        e.pic = (w, h);
        e.pixels.clone()
    });
    px.set(w, h, rgba);
    Ok(())
}

/// Decodes a PNG, JPEG or WebP frame within [`frame_limits`] on this thread,
/// then shows it. Any thread (a feature's socket thread).
pub fn set_encoded(id: &str, bytes: &[u8]) -> Result<(u32, u32), String> {
    let (w, h, rgba) = crate::picture::decode_rgba(bytes, frame_limits())?;
    set_rgba(id, w, h, rgba)?;
    Ok((w, h))
}

/// Forgets the surface's picture (it shows empty).
pub fn clear(id: &str) {
    let mut g = SURFACES.lock().unwrap();
    if let Some(m) = g.as_mut() {
        m.remove(id);
    }
}

/// The picture's size in pixels (0x0: none yet).
pub fn picture_size(id: &str) -> (u32, u32) {
    with(id, |e| e.pic)
}

/// The shown area in points, as last laid out.
pub fn view_size(id: &str) -> (f32, f32) {
    with(id, |e| e.view)
}

/// The surface that has the keyboard, if any.
pub fn focused() -> Option<String> {
    FOCUSED.lock().unwrap().clone()
}

/// The element key of surface `id` (focusable).
pub fn key(id: &str) -> String {
    format!("surface-{id}")
}

/// The shell tells which element has Rune's keyboard focus: the surface
/// events for a change (out of the old, into the new).
pub fn focus_changed(rune_focus: Option<&str>) -> Vec<SurfaceEvent> {
    let now = rune_focus.and_then(|k| k.strip_prefix("surface-")).map(str::to_string);
    let mut g = FOCUSED.lock().unwrap();
    if *g == now {
        return vec![];
    }
    let mut out = vec![];
    if let Some(old) = g.take() {
        out.push(SurfaceEvent::Focus { surface: old, focused: false });
    }
    if let Some(new) = &now {
        out.push(SurfaceEvent::Focus { surface: new.clone(), focused: true });
    }
    *g = now;
    out
}

/// Sizes that held for [`RESIZE_DEBOUNCE`]: their `Resized` events, and when
/// to look again for the others.
pub fn due_resizes(now: Instant) -> (Vec<SurfaceEvent>, Option<Instant>) {
    let mut g = SURFACES.lock().unwrap();
    let Some(m) = g.as_mut() else { return (vec![], None) };
    let (mut out, mut next) = (vec![], None::<Instant>);
    for (id, e) in m.iter_mut() {
        if let Some((sz, since)) = e.resize {
            if now.duration_since(since) >= RESIZE_DEBOUNCE {
                e.resize = None;
                if sz != e.told {
                    e.told = sz;
                    out.push(SurfaceEvent::Resized { surface: id.clone(), w: sz.0, h: sz.1 });
                }
            } else {
                let at = since + RESIZE_DEBOUNCE;
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
    }
    (out, next)
}

/// The view was laid out at `size` points (Rune's `on_layout`).
pub fn laid_out(id: &str, size: (f32, f32), now: Instant) -> bool {
    with(id, |e| {
        if e.view == size {
            return false;
        }
        e.view = size;
        e.resize = Some((size, now));
        true
    })
}

fn fit_of(id: &str) -> Option<Fit> {
    with(id, |e| Fit::new(e.view, e.pic))
}

/// A pointer gesture phase in view points: the event for the feature, if any.
pub fn pointer(id: &str, phase: rune::Phase, x: f32, y: f32, button: Button, clicks: u32, mods: Mod) -> Option<SurfaceEvent> {
    let fit = fit_of(id)?;
    let surface = id.to_string();
    match phase {
        rune::Phase::Down => {
            let (px, py) = fit.to_picture(x, y)?;
            with(id, |e| e.held |= button.bit());
            Some(SurfaceEvent::Press { surface, x: px, y: py, button, clicks: clicks.clamp(1, 3), mods })
        }
        rune::Phase::Move => {
            let held = with(id, |e| e.held);
            let (px, py) = if held != 0 { fit.to_picture_clamped(x, y) } else { fit.to_picture(x, y)? };
            Some(SurfaceEvent::Move { surface, x: px, y: py, buttons: dom_buttons(held), mods })
        }
        rune::Phase::Up => {
            let was = with(id, |e| {
                let was = e.held;
                e.held &= !button.bit();
                was
            });
            if was & button.bit() == 0 {
                return None;
            }
            let (px, py) = fit.to_picture_clamped(x, y);
            Some(SurfaceEvent::Release { surface, x: px, y: py, button, mods })
        }
    }
}

/// DOM `MouseEvent.buttons`: 1 left, 2 right, 4 middle (from our bits by button number).
fn dom_buttons(held: u8) -> u8 {
    (if held & 1 != 0 { 1 } else { 0 }) | (if held & 4 != 0 { 2 } else { 0 }) | (if held & 2 != 0 { 4 } else { 0 })
}

/// A hover move (no gesture): the event, if over the picture.
pub fn hover(id: &str, x: f32, y: f32, mods: Mod) -> Option<SurfaceEvent> {
    if with(id, |e| e.held) != 0 {
        return None; // the drag reports it
    }
    pointer(id, rune::Phase::Move, x, y, Button::Left, 0, mods)
}

/// A wheel at view point `(x, y)`, Rune's deltas (content movement): the DOM-style event.
pub fn wheel(id: &str, x: f32, y: f32, dx: f32, dy: f32, mods: Mod) -> Option<SurfaceEvent> {
    let (px, py) = fit_of(id)?.to_picture(x, y)?;
    Some(SurfaceEvent::Wheel { surface: id.to_string(), x: px, y: py, dx: -dx, dy: -dy, mods })
}

/// A key on the surface: the event, or `None` when the app keeps it.
pub fn key_event(id: &str, ev: &KeyEvent, code: Option<rune::PhysicalKeyCode>, down: bool) -> SurfaceEvent {
    let m = ev.mods;
    let text = match (&ev.key, down && !(m.cmd || m.ctrl || m.logo)) {
        (Key::Text(t), true) => Some(t.clone()),
        _ => None,
    };
    SurfaceEvent::Key { surface: id.to_string(), down, key: dom_key(&ev.key, m.shift), code: dom_code(code), text, mods: m.into() }
}

// ---------------------------------------------------------------------------
// The element

/// The surface element, key `surface-<id>`: grows into its parent, draws the
/// picture letterboxed, takes the keyboard on a press. `sink` hands each
/// event to the app (which routes it to the owner feature); `keys` says
/// whether a key (down, or up) is the app's: not sent, passed on to the
/// window (it may act on it first: the release key, F6, a paste).
pub fn view<S: 'static>(id: &str, sink: fn(&mut S, SurfaceEvent, &mut Tx), keys: fn(&mut S, &KeyEvent, bool, &mut Tx) -> bool) -> rune::El {
    let px = with(id, |e| e.pixels.clone());
    let has = picture_size(id) != (0, 0);
    let id1 = id.to_string();
    let (i2, i3, i4, i5, i6, i7) = (id1.clone(), id1.clone(), id1.clone(), id1.clone(), id1.clone(), id1.clone());
    let mut e = rune::el()
        .key(key(id))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .fill(crate::theme::c("media"))
        .focusable(true)
        .cursor(rune::Cursor::Default)
        .pointer_buttons([rune::PointerButton::Middle])
        .on_layout_tx(move |_s: &mut S, r: [f32; 4], tx: &mut Tx| {
            if laid_out(&id1, (r[2], r[3]), Instant::now()) {
                tx.wake_after(RESIZE_DEBOUNCE);
            }
        })
        .on_drag(move |s: &mut S, d: &mut rune::Drag, tx: &mut Tx| {
            let b = match tx.button() {
                rune::PointerButton::Middle => Button::Middle,
                rune::PointerButton::Secondary => Button::Right,
                _ => Button::Left,
            };
            if d.phase == rune::Phase::Down {
                tx.focus(key(&i2), false);
            }
            if let Some(ev) = pointer(&i2, d.phase, d.x, d.y, b, d.clicks, tx.mods().into()) {
                sink(s, ev, tx);
            }
        })
        .on_pointer_move_local(move |s: &mut S, p: rune::PointerEvent, tx: &mut Tx| {
            if let Some(ev) = hover(&i3, p.x, p.y, tx.mods().into()) {
                sink(s, ev, tx);
            }
        })
        .on_secondary_press_local(move |s: &mut S, p: rune::PointerEvent, tx: &mut Tx| {
            let m: Mod = tx.mods().into();
            tx.focus(key(&i4), false);
            for ph in [rune::Phase::Down, rune::Phase::Up] {
                if let Some(ev) = pointer(&i4, ph, p.x, p.y, Button::Right, 1, m) {
                    sink(s, ev, tx);
                }
            }
        })
        .on_wheel(move |s: &mut S, w: rune::WheelEvent, tx: &mut Tx| match wheel(&i5, w.x, w.y, w.dx, w.dy, w.mods.into()) {
            Some(ev) => {
                sink(s, ev, tx);
                true
            }
            None => false,
        })
        .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
            if keys(s, ev, true, tx) {
                return false; // the app's: passed on to the window
            }
            let code = tx.physical_key();
            sink(s, key_event(&i6, ev, code, true), tx);
            true
        })
        .on_key_up(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
            if keys(s, ev, false, tx) {
                return false;
            }
            let code = tx.physical_key();
            sink(s, key_event(&i7, ev, code, false), tx);
            true
        });
    if has {
        e = e.pixels_fit(&px, rune::SurfaceFit::Fit);
    }
    if focused().as_deref() == Some(id) {
        // Where the input method composes: its commits come as `Text`.
        e = e.child(rune::el().key(format!("surface-ime-{id}")).absolute(0.0, 0.0).w(1.0).h(1.0).ime_cursor());
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn mapping_keeps_the_aspect_and_letterboxes_at_several_sizes() {
        // (view, picture, a view point, the picture point expected or None)
        let cases: &[((f32, f32), (u32, u32), (f32, f32), Option<(f32, f32)>)] = &[
            // Same aspect, half size: x2.
            ((640.0, 400.0), (1280, 800), (320.0, 200.0), Some((640.0, 400.0))),
            ((640.0, 400.0), (1280, 800), (0.0, 0.0), Some((0.0, 0.0))),
            // A wide view: bars left and right (picture 1280x800 in 1000x400 → 640x400 at x 180).
            ((1000.0, 400.0), (1280, 800), (180.0, 0.0), Some((0.0, 0.0))),
            ((1000.0, 400.0), (1280, 800), (179.0, 200.0), None),
            ((1000.0, 400.0), (1280, 800), (820.0, 200.0), None),
            ((1000.0, 400.0), (1280, 800), (819.0, 399.0), Some((1278.0, 798.0))),
            // A tall view: bars top and bottom (2560x1600 in 400x900 → 400x250 at y 325).
            ((400.0, 900.0), (2560, 1600), (200.0, 325.0), Some((1280.0, 0.0))),
            ((400.0, 900.0), (2560, 1600), (200.0, 324.0), None),
            ((400.0, 900.0), (2560, 1600), (200.0, 575.0), None),
            ((400.0, 900.0), (2560, 1600), (399.0, 574.0), Some((2553.6, 1593.6))),
            // A picture smaller than the view is scaled up (x2.5).
            ((800.0, 600.0), (320, 240), (400.0, 300.0), Some((160.0, 120.0))),
            // A portrait picture in a landscape view.
            ((1280.0, 720.0), (390, 844), (640.0, 360.0), Some((195.0, 422.0))),
            ((1280.0, 720.0), (390, 844), (400.0, 360.0), None),
        ];
        for (view, pic, at, want) in cases {
            let f = Fit::new(*view, *pic).unwrap();
            // The aspect ratio is kept, and the picture fits inside the view.
            assert!(((f.w / f.h) - pic.0 as f32 / pic.1 as f32).abs() < 1e-3, "{view:?} {pic:?}");
            assert!(f.w <= view.0 + 0.01 && f.h <= view.1 + 0.01 && (f.w - view.0).abs().min((f.h - view.1).abs()) < 0.01);
            let got = f.to_picture(at.0, at.1);
            match (got, want) {
                (Some(g), Some(w)) => assert!(close(g, *w), "{view:?} {pic:?} {at:?}: {g:?} != {w:?}"),
                (None, None) => {}
                _ => panic!("{view:?} {pic:?} {at:?}: {got:?} != {want:?}"),
            }
        }
        assert!(Fit::new((0.0, 10.0), (10, 10)).is_none());
        assert!(Fit::new((10.0, 10.0), (0, 10)).is_none());
    }

    #[test]
    fn a_drag_is_clamped_and_its_release_always_goes_out() {
        let id = "t-drag";
        set_rgba(id, 100, 50, vec![0; 100 * 50 * 4]).unwrap();
        laid_out(id, (200.0, 200.0), Instant::now()); // 200x100 at y 50
        let m = Mod::default();
        // A press in the letterbox is dropped, and so is its release.
        assert_eq!(pointer(id, rune::Phase::Down, 100.0, 10.0, Button::Left, 1, m), None);
        assert_eq!(pointer(id, rune::Phase::Up, 100.0, 10.0, Button::Left, 1, m), None);
        let p = pointer(id, rune::Phase::Down, 100.0, 100.0, Button::Left, 2, m).unwrap();
        assert_eq!(p, SurfaceEvent::Press { surface: id.into(), x: 50.0, y: 25.0, button: Button::Left, clicks: 2, mods: m });
        // Out of the picture while held: clamped, buttons say left.
        let mv = pointer(id, rune::Phase::Move, 250.0, 0.0, Button::Left, 2, m).unwrap();
        assert_eq!(mv, SurfaceEvent::Move { surface: id.into(), x: 99.0, y: 0.0, buttons: 1, mods: m });
        assert_eq!(hover(id, 100.0, 100.0, m), None, "the drag reports moves while held");
        let up = pointer(id, rune::Phase::Up, 250.0, 0.0, Button::Left, 2, m).unwrap();
        assert!(matches!(up, SurfaceEvent::Release { x, y, .. } if x == 99.0 && y == 0.0));
        // Released: hover outside is dropped again, inside goes out.
        assert_eq!(hover(id, 100.0, 10.0, m), None);
        assert!(matches!(hover(id, 0.0, 50.0, m), Some(SurfaceEvent::Move { x, y, buttons: 0, .. }) if x == 0.0 && y == 0.0));
        // Wheel: DOM direction (content moving up is a scroll down).
        assert!(matches!(wheel(id, 100.0, 100.0, 0.0, -30.0, m), Some(SurfaceEvent::Wheel { dy, .. }) if dy == 30.0));
        assert_eq!(wheel(id, 100.0, 199.0, 0.0, -30.0, m), None);
        clear(id);
    }

    #[test]
    fn resizes_are_told_once_the_size_holds() {
        let id = "t-resize";
        let t0 = Instant::now();
        laid_out(id, (300.0, 200.0), t0);
        laid_out(id, (310.0, 200.0), t0 + Duration::from_millis(50));
        let (ev, next) = due_resizes(t0 + Duration::from_millis(100));
        assert!(ev.iter().all(|e| e.surface() != id) && next.is_some());
        let (ev, _) = due_resizes(t0 + Duration::from_millis(300));
        assert!(ev.contains(&SurfaceEvent::Resized { surface: id.into(), w: 310.0, h: 200.0 }), "{ev:?}");
        assert!(!laid_out(id, (310.0, 200.0), t0 + Duration::from_millis(400)), "same size: nothing");
        clear(id);
    }

    #[test]
    fn the_apps_keys_stay_the_apps() {
        let k = |key: Key, cmd: bool| KeyEvent { key, mods: Mods { cmd, ..Mods::default() } };
        let ptt = [Key::Function(8)];
        assert!(reserved(&k(Key::Function(8), false), &ptt, false), "push-to-talk");
        assert!(reserved(&k(Key::Function(6), false), &ptt, false), "F6");
        assert!(reserved(&k(Key::Escape, true), &ptt, false), "the release key");
        assert!(reserved(&k(Key::Command('='), true), &ptt, false), "font size");
        assert!(reserved(&k(Key::Command('0'), true), &ptt, false));
        assert!(reserved(&k(Key::Text("n".into()), false), &ptt, true), "a global shortcut");
        assert!(!reserved(&k(Key::Escape, false), &ptt, false), "Escape goes to the page");
        assert!(!reserved(&k(Key::Command('a'), true), &ptt, false));
        assert!(!reserved(&k(Key::Text("a".into()), false), &ptt, false));
        assert!(!reserved(&k(Key::Function(5), false), &ptt, false));
        let ev = key_event("s", &k(Key::Text("A".into()), false), None, true);
        assert!(matches!(ev, SurfaceEvent::Key { ref key, ref text, down: true, .. } if key == "A" && text.as_deref() == Some("A")));
        let ev = key_event("s", &k(Key::Command('c'), true), None, true);
        assert!(matches!(ev, SurfaceEvent::Key { ref key, text: None, .. } if key == "c"));
        assert!(matches!(key_event("s", &k(Key::Left, false), None, false), SurfaceEvent::Key { ref key, down: false, .. } if key == "ArrowLeft"));
    }

    #[test]
    fn frames_of_the_wrong_size_are_refused() {
        assert!(set_rgba("t-bad", 2, 2, vec![0; 15]).is_err());
        assert!(set_rgba("t-bad", 5000, 2, vec![0; 5000 * 2 * 4]).is_err());
        assert!(set_encoded("t-bad", b"not a picture").is_err());
        assert_eq!(picture_size("t-bad"), (0, 0));
    }
}
