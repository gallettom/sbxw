//! The stand-in daemon's browser: its HTTP routes (`/api/browser/active`,
//! `/api/sandboxes/:name/browser/{grants,allow,settle}`) over the fixture's
//! `browser_active` and `browser_grants` keys, and `/ws/browser`, which plays
//! `browser.mjs serve`: `ready`, the address, pictures, two refused hosts,
//! one malformed frame (refused by the app without a crash), and answers to
//! the app's input.
//!
//! The pictures are drawn here and JPEG-encoded as Chromium's screencast
//! sends them: a page with a header, blocks, a bar that moves while the page
//! "animates" (2 s after it loads and after each input; always on
//! `https://anim.example/`), a box at the last click and a bar as long as
//! what was typed. A press or a key is answered with a picture at once, so
//! the app's input-to-picture latency is measured through the whole path
//! (app, socket, stand-in, JPEG encode, base64, decode, surface).

use super::proto;
use crate::feat::{match_path, query, Standin, WsPeer};
use serde_json::{json, Value as J};
use std::time::{Duration, Instant};

/// Pictures a second while the page animates: `SBXW_STANDIN_BROWSER_FPS`
/// (default 30; 0: a picture only when the page loads or answers input).
fn frame_every() -> Option<Duration> {
    let fps: u64 = std::env::var("SBXW_STANDIN_BROWSER_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    (fps > 0).then(|| Duration::from_millis(1000 / fps.min(120)))
}
const ANIMATE_FOR: Duration = Duration::from_secs(2);

pub fn route(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    if (method, path.split('?').next().unwrap_or("")) == ("GET", "/api/browser/active") {
        let v = st.data[super::ACTIVE].clone();
        return Some((200, if v.is_null() { json!({ "ok": true, "sandboxes": [] }) } else { v }));
    }
    if let Some(c) = match_path("/api/sandboxes/{sandbox}/browser/grants", path) {
        if method != "GET" {
            return None;
        }
        let v = st.data["browser_grants"][&c[0].1].clone();
        return Some((200, if v.is_null() { json!({ "ok": true, "grants": [] }) } else { v }));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/browser/allow", path), method) {
        let sb = c[0].1.clone();
        let host = body["host"].as_str().unwrap_or("").trim().to_ascii_lowercase();
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c)) {
            return Some((400, json!({ "ok": false, "error": "not a host name" })));
        }
        let g = &mut st.data["browser_grants"][&sb];
        if g.is_null() {
            *g = json!({ "ok": true, "grants": [] });
        }
        let list = g["grants"].as_array_mut().unwrap();
        let n = list.len() + 1;
        let grant = json!({ "host": host, "rules": [format!("standin-rule-{n}")] });
        if !list.iter().any(|x| x["host"] == host.as_str()) {
            list.push(grant.clone());
        }
        return Some((200, json!({ "ok": true, "grant": grant })));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/browser/settle", path), method) {
        let sb = c[0].1.clone();
        let hosts: Vec<J> = st.data["browser_grants"][&sb]["grants"].as_array().into_iter().flatten().map(|g| g["host"].clone()).collect();
        st.data["browser_grants"][&sb] = json!({ "ok": true, "grants": [] });
        let keep = body["keep"] == true;
        return Some((200, json!({ "ok": true, "settled": { "kept": if keep { hosts.clone() } else { vec![] }, "removed": if keep { vec![] } else { hosts }, "failed": [] } })));
    }
    None
}

/// The page the stand-in's browser shows.
struct Page {
    history: Vec<String>,
    at: usize,
    size: (u32, u32),
    viewport: String,
    click: Option<(u32, u32)>,
    typed: usize,
    tick: u32,
    animate_until: Instant,
}

impl Page {
    fn url(&self) -> &str {
        &self.history[self.at]
    }
    fn title(&self) -> String {
        let host = self.url().split("//").nth(1).unwrap_or("").split('/').next().unwrap_or("");
        format!("{host}: a page of the stand-in")
    }
    fn animating(&self, now: Instant) -> bool {
        now < self.animate_until || self.url().starts_with("https://anim.example")
    }
    fn go(&mut self, url: String, now: Instant) {
        self.history.truncate(self.at + 1);
        self.history.push(url);
        self.at = self.history.len() - 1;
        self.typed = 0;
        self.click = None;
        self.animate_until = now + ANIMATE_FOR;
    }
    fn size(&self) -> (u32, u32) {
        match self.viewport.as_str() {
            "mobile" => (390, 844),
            "tablet" => (820, 1180),
            "desktop" => (1280, 800),
            _ => self.size,
        }
    }
}

/// A picture of the page, as JPEG (quality 60, as `browser.mjs`'s screencast).
pub fn picture(w: u32, h: u32, hue: u32, tick: u32, click: Option<(u32, u32)>, typed: usize) -> Vec<u8> {
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([246, 247, 249]));
    let mut rect = |x: u32, y: u32, rw: u32, rh: u32, c: [u8; 3]| {
        for yy in y.min(h)..(y + rh).min(h) {
            for xx in x.min(w)..(x + rw).min(w) {
                img.put_pixel(xx, yy, image::Rgb(c));
            }
        }
    };
    let pal = [[38, 99, 235], [22, 163, 74], [217, 119, 6], [147, 51, 234], [220, 38, 38]];
    let c = pal[(hue as usize) % pal.len()];
    // Header and a nav bar.
    rect(0, 0, w, 64, c);
    rect(24, 20, 160, 24, [255, 255, 255]);
    for i in 0..4 {
        rect(w.saturating_sub(120 * (i + 1)), 26, 90, 12, [235, 238, 245]);
    }
    // A hero block and three cards.
    rect(48, 104, w.saturating_sub(96), 180, [226, 232, 240]);
    rect(72, 140, (w / 2).saturating_sub(72), 28, [100, 116, 139]);
    rect(72, 188, (w / 3).saturating_sub(72), 16, [148, 163, 184]);
    let cw = (w.saturating_sub(96 + 48)) / 3;
    for i in 0..3 {
        rect(48 + i * (cw + 24), 316, cw, 220, [255, 255, 255]);
        rect(48 + i * (cw + 24) + 16, 332, cw.saturating_sub(32), 90, [203, 213, 225]);
    }
    // The animation: a bar that moves.
    let x = (tick * 12) % w.max(1);
    rect(x, 64, 120.min(w), 4, [250, 204, 21]);
    // What was typed, as a bar under the hero.
    rect(72, 230, (typed as u32 * 10).min(w.saturating_sub(144)), 18, [15, 23, 42]);
    // The last click.
    if let Some((cx, cy)) = click {
        rect(cx.saturating_sub(10), cy.saturating_sub(10), 20, 20, [239, 68, 68]);
    }
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 60);
    enc.encode(img.as_raw(), w, h, image::ExtendedColorType::Rgb8).expect("jpeg");
    out
}

fn frame_line(p: &Page) -> String {
    let (w, h) = p.size();
    let hue = p.history.iter().take(p.at + 1).count() as u32 + p.url().len() as u32;
    let jpeg = picture(w, h, hue, p.tick, p.click, p.typed);
    // In `browser.mjs`'s order: `t` first.
    format!(r#"{{"t":"frame","d":"{}","w":{w},"h":{h}}}"#, proto::b64_encode(&jpeg))
}

/// `/ws/browser?sandbox=`: plays `browser.mjs serve` until the app leaves or
/// asks it to quit.
pub fn socket(target: &str, peer: &mut dyn WsPeer) -> bool {
    if target.split('?').next() != Some("/ws/browser") {
        return false;
    }
    let sb = query(target, "sandbox").unwrap_or_default();
    eprintln!("standin: browser of {sb} opened");
    let start = Instant::now();
    let mut p = Page { history: vec!["https://example.com/".into()], at: 0, size: (1280, 800), viewport: "auto".into(), click: None, typed: 0, tick: 0, animate_until: start + ANIMATE_FOR };
    let say = |peer: &mut dyn WsPeer, v: J| peer.send(&v.to_string());
    if !say(peer, json!({ "t": "ready", "browser": "Chrome/stand-in" })) {
        return true;
    }
    say(peer, json!({ "t": "viewport", "name": "auto" }));
    say(peer, json!({ "t": "url", "url": p.url(), "title": p.title() }));
    // What a broken bridge could send: not base64, then not a JPEG. Both refused.
    say(peer, json!({ "t": "frame", "d": "!!not base64!!", "w": 1280, "h": 800 }));
    say(peer, json!({ "t": "frame", "d": proto::b64_encode(b"GIF89a not a jpeg"), "w": 1280, "h": 800 }));
    let mut told_blocked = false;
    let every = frame_every();
    let tick = every.unwrap_or(Duration::from_millis(200));
    let mut last_frame = start - tick;
    let mut sent = 0u64;
    loop {
        let now = Instant::now();
        if !told_blocked && now.duration_since(start) > Duration::from_millis(600) {
            told_blocked = true;
            say(peer, json!({ "t": "blocked", "host": "fonts.googleapis.com", "kind": "" }));
            say(peer, json!({ "t": "blocked", "host": "pastebin.com", "kind": "org" }));
            // Said again while the page retries: the app shows it once.
            say(peer, json!({ "t": "blocked", "host": "fonts.googleapis.com", "kind": "" }));
        }
        // With no frame rate, one picture when the page loads.
        if (p.animating(now) && every.is_some() || sent == 0) && now.duration_since(last_frame) >= tick {
            p.tick += 1;
            if !peer.send(&frame_line(&p)) {
                break;
            }
            sent += 1;
            last_frame = now;
        }
        let wait = if p.animating(now) && every.is_some() { tick.saturating_sub(now.duration_since(last_frame)).max(Duration::from_millis(2)) } else { Duration::from_millis(200) };
        let msg = match peer.recv(wait) {
            Ok(Some(m)) => m,
            Ok(None) => continue,
            Err(_) => break,
        };
        let Ok(m) = serde_json::from_str::<J>(&msg) else { continue };
        eprintln!("standin: browser got {}", short(&msg));
        let now = Instant::now();
        let mut answer = false;
        match m["t"].as_str().unwrap_or("") {
            "mouse" => {
                if m["type"] == "mousePressed" {
                    p.click = Some((m["x"].as_u64().unwrap_or(0) as u32, m["y"].as_u64().unwrap_or(0) as u32));
                    answer = true;
                }
            }
            "key" => {
                if m["type"] == "keyDown" {
                    p.typed += 1;
                    answer = true;
                } else if m["type"] == "rawKeyDown" {
                    if m["key"] == "Backspace" {
                        p.typed = p.typed.saturating_sub(1);
                    }
                    answer = true;
                }
            }
            "paste" => {
                p.typed += m["text"].as_str().map_or(0, |t| t.chars().count());
                answer = true;
            }
            "nav" => {
                let u = m["url"].as_str().unwrap_or("").trim();
                let u = if u.contains("://") { u.to_string() } else { format!("https://{u}") };
                if u.starts_with("http://") || u.starts_with("https://") {
                    p.go(u, now);
                    say(peer, json!({ "t": "url", "url": p.url(), "title": p.title() }));
                    if p.url().contains("registry.npmjs.org") {
                        say(peer, json!({ "t": "blocked", "host": "registry.npmjs.org", "kind": "" }));
                    }
                    answer = true;
                } else {
                    say(peer, json!({ "t": "note", "msg": "not an http(s) address" }));
                }
            }
            "back" | "forward" => {
                let to = if m["t"] == "back" { p.at.checked_sub(1) } else { Some(p.at + 1).filter(|i| *i < p.history.len()) };
                if let Some(i) = to {
                    p.at = i;
                    p.animate_until = now + ANIMATE_FOR;
                    say(peer, json!({ "t": "url", "url": p.url(), "title": p.title() }));
                    answer = true;
                }
            }
            "reload" => {
                p.animate_until = now + ANIMATE_FOR;
                answer = true;
            }
            "size" => {
                let (w, h) = (m["w"].as_u64().unwrap_or(1280).clamp(320, 2560) as u32, m["h"].as_u64().unwrap_or(800).clamp(320, 1600) as u32);
                p.size = (w, h);
                answer = true;
            }
            "viewport" => {
                p.viewport = m["value"].as_str().unwrap_or("auto").to_string();
                say(peer, json!({ "t": "viewport", "name": p.viewport }));
                answer = true;
            }
            "quit" => {
                say(peer, json!({ "t": "closed" }));
                say(peer, json!({ "t": "gone" }));
                peer.close();
                eprintln!("standin: browser of {sb} closed ({sent} frames sent)");
                return true;
            }
            _ => {}
        }
        if answer {
            p.animate_until = p.animate_until.max(now + Duration::from_millis(800));
            p.tick += 1;
            if !peer.send(&frame_line(&p)) {
                break;
            }
            sent += 1;
            last_frame = Instant::now();
        }
    }
    eprintln!("standin: browser of {sb}: the app left ({sent} frames sent); the browser keeps running");
    true
}

fn short(s: &str) -> String {
    if s.len() > 120 { format!("{}…", &s[..s.floor_char_boundary(120)]) } else { s.to_string() }
}
