//! The sandbox's browser (`assets/js/browser.js`, `docs/chat.md`): the
//! sandbox's own Chromium, watched and driven from a docked panel.
//!
//! One WebSocket to the daemon (`/ws/browser?sandbox=`, [`proto`]) carries
//! pictures of the page, its address and the hosts the network policy
//! refused; clicks, keys, scrolling and navigation go back the same way.
//! Frames go from the socket's thread straight to the core's picture
//! surface `browser` ([`crate::surface::set_encoded`], decoded with the
//! core's limits): Luau never sees a pixel or an input event, only the
//! chrome's state, `data.browser_ui`.
//!
//! The rules of this area:
//!
//! * **What comes from the sandbox is only pixels or text.** A frame is a
//!   JPEG checked and decoded by the core; an address, a title, a note, a
//!   host are cleaned, bounded text ([`proto::clean`]). Nothing from the
//!   sandbox opens anything here or reaches the clipboard: the protocol has
//!   no way out but pixels and text, and this module calls neither
//!   `OpenLink` nor `copy_text`.
//! * **What the person types in the address bar is text sent to the
//!   sandbox's browser** (`nav`), never opened on their machine.
//! * **A paste is the person's own ⌘V / Ctrl+V on the page**, text only
//!   (the core reads the clipboard on that press, says how much, and
//!   hands the text here).
//! * **Allowing a refused host widens the sandbox's network**: an intent of
//!   risk Approval, the core's dialog built from the host the bridge
//!   reported (never from a screen's words), posted to the daemon's
//!   `/browser/allow`, which writes the rule for this sandbox only and
//!   remembers it as the panel's (the network feature's allow writes a
//!   plain rule, which closing the browser could not offer to remove).
//! * **Hiding the panel leaves the browser running** for the agent; closing
//!   the browser asks what to do with the hosts allowed from here.
//! * **No automatic reconnect**: a new socket runs a new bridge in the
//!   sandbox. A lost connection says so; opening the panel again reconnects.

pub mod proto;
pub mod standin;
#[cfg(test)]
mod tests;

use super::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, ScreenDecl, Slot};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use crate::socket::{Opts, SocketEvent};
use crate::surface::SurfaceEvent;
use proto::{In, Kind, Mods, Mouse};
use serde_json::{json, Value as J};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const SCREEN: &str = "browser";
pub const SETTLE: &str = "browser-settle";
/// The socket's and the surface's id.
pub const ID: &str = "browser";
/// `data.browser_ui`: the chrome's state, as the screen sees it.
pub const UI: &str = "browser_ui";
/// `data.browser_active`: `GET /api/browser/active`, the sandboxes whose agent uses its browser.
pub const ACTIVE: &str = "browser_active";
/// `data.browser_settle`: the question asked on close (`{sandbox, hosts}`).
pub const SETTLE_KEY: &str = "browser_settle";
/// The address bar's input id.
pub const URL_INPUT: &str = "browser-url";

pub const FEATURE: Feature = Feature {
    name: "browser",
    data: &[Data { key: ACTIVE, path: "/api/browser/active", on: &["browser"], every: 0, lazy: false }],
    screens: &[
        ScreenDecl { name: SCREEN, src: include_str!("../../../screens/browser.luau"), slot: Slot::Panel { title: "Browser", icon: "link", width: 720.0 } },
        ScreenDecl { name: SETTLE, src: include_str!("../../../screens/browser-settle.luau"), slot: Slot::Modal { width: 440.0, light_dismiss: false } },
    ],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin::route),
    surfaces: &[ID],
    on_surface: Some(on_surface),
    on_socket: Some(on_socket),
    standin_ws: Some(standin::socket),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "browser_open", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Opens the Browser panel on that sandbox's own Chromium (started in the sandbox if it is not running), and connects to it." },
    Spec { name: "browser_hide", risk: Risk::Direct, args: &[], doc: "Hides the panel and disconnects; the browser keeps running for the agent." },
    Spec { name: "browser_close", risk: Risk::Direct, args: &[], doc: "Closes the sandbox's browser; then asks what to do with the hosts allowed from the panel." },
    Spec { name: "browser_nav", risk: Risk::Direct, args: &[("url", Arg::Str, true)], doc: "Sends an address to the sandbox's browser, as text (it takes http and https only). Never opened on this machine." },
    Spec { name: "browser_back", risk: Risk::Direct, args: &[], doc: "Back in the sandbox's browser." },
    Spec { name: "browser_forward", risk: Risk::Direct, args: &[], doc: "Forward in the sandbox's browser." },
    Spec { name: "browser_reload", risk: Risk::Direct, args: &[], doc: "Reloads the page in the sandbox's browser." },
    Spec { name: "browser_viewport", risk: Risk::Direct, args: &[("value", Arg::Str, true)], doc: "The device the page is laid out for: `auto` (fits the panel), `mobile`, `tablet`, `desktop`." },
    Spec {
        name: "browser_allow",
        risk: Risk::Approval("Allow a sandbox to reach a host"),
        args: &[("host", Arg::Str, true)],
        doc: "Allows a host the sandbox's browser was refused, for that sandbox only, after the core's dialog; remembered as the panel's until the browser closes.",
    },
    Spec { name: "browser_ignore", risk: Risk::Direct, args: &[("host", Arg::Str, true)], doc: "Leaves a refused host blocked and hides it from the panel." },
    Spec { name: "browser_settle", risk: Risk::Direct, args: &[("keep", Arg::Bool, true)], doc: "After the browser closed: keeps the hosts allowed from the panel, or removes their rules (narrows)." },
    Spec { name: "browser_page", risk: Risk::Direct, args: &[], doc: "Gives the keyboard to the page (the picture)." },
];

// ---------------------------------------------------------------------------
// The session

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Connecting, or the bridge has not sent a picture yet.
    Starting,
    Live,
    /// The person pressed Close browser: waiting for `closed`.
    Closing,
    /// The connection ended without the browser closing.
    Ended,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Starting => "starting",
            Status::Live => "live",
            Status::Closing => "closing",
            Status::Ended => "ended",
        }
    }
}

/// What the panel knows, kept here (not in Luau): the socket's thread and
/// the UI's both change it.
#[derive(Debug)]
struct Session {
    sandbox: String,
    status: Status,
    url: String,
    title: String,
    viewport: String,
    /// Hosts the proxy refused, in the order seen, with their kind.
    refused: Vec<(String, Kind)>,
    ignored: Vec<String>,
    /// Grants the daemon holds for this sandbox (allowed from a panel, not settled).
    allowed: usize,
    error: Option<String>,
    focused: bool,
    frame: (u32, u32),
    stats: Stats,
    last_move: Option<Instant>,
}

impl Session {
    fn new(sandbox: &str) -> Session {
        Session {
            sandbox: sandbox.into(),
            status: Status::Starting,
            url: String::new(),
            title: String::new(),
            viewport: "auto".into(),
            refused: vec![],
            ignored: vec![],
            allowed: 0,
            error: None,
            focused: false,
            frame: (0, 0),
            stats: Stats::default(),
            last_move: None,
        }
    }

    fn view(&self) -> J {
        let blocked: Vec<J> = self
            .refused
            .iter()
            .filter(|(h, _)| !self.ignored.contains(h))
            .map(|(h, k)| json!({ "host": h, "kind": k.word(), "fixed": k.fixed() }))
            .collect();
        let mut viewports: Vec<J> = proto::VIEWPORTS.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect();
        if !proto::VIEWPORTS.iter().any(|(id, _)| *id == self.viewport) {
            // A size the agent chose: shown as it is, until another is picked.
            viewports.push(json!({ "id": self.viewport, "label": self.viewport, "disabled": true }));
        }
        json!({
            "sandbox": self.sandbox,
            "status": self.status.word(),
            "url": self.url,
            "title": self.title,
            "viewport": self.viewport,
            "viewports": viewports,
            "blocked": blocked,
            "allowed": self.allowed,
            "error": self.error,
            "focused": self.focused,
            "release_key": crate::surface::RELEASE_KEY,
            "frame": { "w": self.frame.0, "h": self.frame.1 },
            "stats": self.stats.view(),
        })
    }
}

/// Frame rate and input-to-picture latency, measured on the socket's thread.
#[derive(Debug, Default)]
struct Stats {
    frames: u64,
    refused: u64,
    window_start: Option<Instant>,
    window_frames: u32,
    fps: f32,
    /// A press or key sent, waiting for the next picture.
    waiting: Option<Instant>,
    latencies: Vec<u32>,
    last_view: Option<Instant>,
}

impl Stats {
    fn frame(&mut self, now: Instant) -> Option<u32> {
        self.frames += 1;
        let start = *self.window_start.get_or_insert(now);
        self.window_frames += 1;
        let span = now.duration_since(start);
        if span >= Duration::from_secs(2) {
            self.fps = self.window_frames as f32 / span.as_secs_f32();
            log::info!("browser: {:.1} frames/s over {:.1} s ({} frames in all)", self.fps, span.as_secs_f32(), self.frames);
            self.window_start = Some(now);
            self.window_frames = 0;
        }
        let ms = self.waiting.take().map(|t| now.duration_since(t).as_millis().min(u32::MAX as u128) as u32);
        if let Some(ms) = ms {
            log::info!("browser: input to picture {ms} ms");
            self.latencies.push(ms);
            if self.latencies.len() > 200 {
                self.latencies.remove(0);
            }
        }
        ms
    }

    fn median(&self) -> Option<u32> {
        let mut v = self.latencies.clone();
        v.sort_unstable();
        v.get(v.len() / 2).copied()
    }

    fn view(&self) -> J {
        json!({ "frames": self.frames, "refused": self.refused, "fps": (self.fps as f64 * 10.0).round() / 10.0, "latency_ms": self.median() })
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Option<Session>) -> R) -> R {
    f(&mut SESSION.lock().unwrap())
}

/// The chrome's state for the screen.
fn ui() -> Action {
    Action::Set { key: UI.into(), value: with(|s| s.as_ref().map_or(json!({ "status": "idle" }), Session::view)) }
}

fn send(line: Option<String>) {
    if let Some(l) = line {
        if let Err(e) = crate::socket::send(ID, l) {
            log::debug!("browser: not sent: {e}");
        }
    }
}

fn sandbox_path(sb: &str, tail: &str) -> String {
    format!("/api/sandboxes/{}/browser/{tail}", enc(sb))
}

fn opts() -> Opts {
    Opts { reconnect: false, droppable: Some(proto::is_frame), max_in: proto::MAX_LINE, max_out: proto::MAX_OUT, one_line: true }
}

fn current(cx: &Ctx) -> Result<String, String> {
    let _ = cx;
    with(|s| s.as_ref().map(|s| s.sandbox.clone())).ok_or_else(|| "the browser panel is not open".into())
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    Ok(match a.name {
        "browser_open" => {
            let sb = a.req("sandbox").to_string();
            let Some(row) = cx.sandbox(&sb) else { return Err(format!("no sandbox {sb}")) };
            if row["status"] != "running" {
                return Err(format!("{sb} is not running: start it first"));
            }
            crate::surface::clear(ID);
            with(|s| *s = Some(Session::new(&sb)));
            vec![
                ui(),
                Action::OpenPanel(SCREEN.into()),
                Action::OpenSocket { id: ID.into(), path: format!("/ws/browser?sandbox={}", enc(&sb)), opts: opts() },
                // What was allowed earlier and not settled yet still counts.
                Action::Call(Call::get(sandbox_path(&sb, "grants"), format!("read {sb}'s browser grants")).tag("grants").quiet()),
            ]
        }
        "browser_hide" => {
            with(|s| *s = None);
            crate::surface::clear(ID);
            vec![Action::CloseSocket(ID.into()), Action::ClosePanel(SCREEN.into()), ui()]
        }
        "browser_close" => {
            let sb = current(cx)?;
            let again = with(|s| {
                let s = s.as_mut().unwrap();
                let again = s.status == Status::Closing;
                s.status = Status::Closing;
                again
            });
            if !crate::socket::is_open(ID) || again {
                // Not connected, or a second press while Chromium does not
                // answer: settle now (the web UI's 2.5 s wait).
                ended(&sb)
            } else {
                send(Some(proto::simple("quit")));
                vec![ui()]
            }
        }
        "browser_nav" => {
            current(cx)?;
            let line = proto::nav(a.req("url")).ok_or("not an address")?;
            send(Some(line));
            vec![Action::Focus(crate::surface::key(ID))]
        }
        "browser_back" | "browser_forward" | "browser_reload" => {
            current(cx)?;
            send(Some(proto::simple(a.name.trim_start_matches("browser_"))));
            vec![]
        }
        "browser_viewport" => {
            current(cx)?;
            send(Some(proto::viewport(a.req("value")).ok_or_else(|| format!("{}: not a device", a.req("value")))?));
            vec![]
        }
        "browser_allow" => {
            let host = a.req("host").to_ascii_lowercase();
            let (sb, kind) = with(|s| s.as_ref().map(|s| (s.sandbox.clone(), s.refused.iter().find(|(h, _)| *h == host).map(|(_, k)| *k)))).ok_or("the browser panel is not open")?;
            // Only a host the bridge reported: a screen cannot name one.
            let kind = kind.ok_or_else(|| format!("{host} was not refused in this browser"))?;
            if let Some(why) = kind.fixed() {
                return Err(format!("{host}: {why}"));
            }
            vec![Confirm::new(
                format!("Allow {sb} to reach {host}?"),
                Call::post(sandbox_path(&sb, "allow"), json!({ "host": host }), format!("allow {host} for {sb}")).tag("allow"),
            )
            .line(format!("{sb}'s network policy refused {host} while its browser loaded a page. Allowing it lets everything in {sb} reach {host}, not only the browser."))
            .line(format!("For {sb} only. Closing the browser asks whether to keep it or remove it."))
            .labels("Allow", "Cancel")
            .into_action()]
        }
        "browser_ignore" => {
            let host = a.req("host").to_string();
            with(|s| s.as_mut().map(|s| s.ignored.push(host)));
            vec![ui()]
        }
        "browser_settle" => {
            let keep = a.b("keep") == Some(true);
            let q = cx.get(SETTLE_KEY);
            let sb = q["sandbox"].as_str().ok_or("nothing to settle")?.to_string();
            vec![
                Action::CloseModal,
                Action::Set { key: SETTLE_KEY.into(), value: J::Null },
                Action::Call(Call::post(sandbox_path(&sb, "settle"), json!({ "keep": keep }), if keep { format!("keep the hosts allowed for {sb}") } else { format!("remove the hosts allowed for {sb}") }).tag("settle")),
            ]
        }
        "browser_page" => {
            current(cx)?;
            vec![Action::Focus(crate::surface::key(ID))]
        }
        other => return Err(format!("{other}: not a browser intent")),
    })
}

/// The browser is gone (closed, or the connection ended while closing):
/// the panel hides, and the hosts allowed from here are settled.
fn ended(sb: &str) -> Vec<Action> {
    with(|s| *s = None);
    crate::surface::clear(ID);
    vec![
        Action::CloseSocket(ID.into()),
        Action::ClosePanel(SCREEN.into()),
        ui(),
        Action::Call(Call::get(sandbox_path(sb, "grants"), format!("read {sb}'s browser grants")).tag("settle-ask").quiet()),
    ]
}

fn on_answer(_cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let body = match r {
        Ok((code, b)) if (200..300).contains(&code) && b["ok"] != false => b,
        Ok((_, b)) => return failed(call, b["error"].as_str().unwrap_or("the daemon refused it")),
        Err(e) => return failed(call, &e),
    };
    match call.tag.as_str() {
        "grants" => {
            let n = body["grants"].as_array().map_or(0, Vec::len);
            let sb = call.path.split('/').nth(3).unwrap_or("");
            with(|s| s.as_mut().filter(|s| enc(&s.sandbox) == sb).map(|s| s.allowed = n));
            vec![ui()]
        }
        "allow" => {
            let host = call.body["host"].as_str().unwrap_or("").to_string();
            let sb = with(|s| {
                s.as_mut().map(|s| {
                    s.refused.retain(|(h, _)| *h != host);
                    s.allowed += 1;
                    s.sandbox.clone()
                })
            });
            // The page that failed tries again.
            send(Some(proto::simple("reload")));
            vec![ui(), Action::notice(format!("{host} allowed for {} only", sb.unwrap_or_default()))]
        }
        "settle-ask" => {
            let hosts: Vec<J> = body["grants"].as_array().into_iter().flatten().filter_map(|g| g["host"].as_str()).map(|h| J::String(proto::clean(h, proto::MAX_HOST))).collect();
            // Nothing was allowed: nothing to ask, the browser just closes.
            if hosts.is_empty() {
                return vec![];
            }
            let sb = decode_seg(call.path.split('/').nth(3).unwrap_or(""));
            vec![Action::Set { key: SETTLE_KEY.into(), value: json!({ "sandbox": sb, "hosts": hosts }) }, Action::OpenModal { screen: SETTLE.into(), arg: J::String(sb) }]
        }
        "settle" => {
            let s = &body["settled"];
            let sb = decode_seg(call.path.split('/').nth(3).unwrap_or(""));
            let list = |k: &str| s[k].as_array().map_or(0, Vec::len);
            let failed: Vec<String> = s["failed"].as_array().into_iter().flatten().map(|f| format!("{} ({})", f["host"].as_str().unwrap_or("?"), f["why"].as_str().unwrap_or("?"))).collect();
            let plural = |n: usize| if n == 1 { "" } else { "s" };
            vec![if !failed.is_empty() {
                Action::error(format!("Not removed: {}", failed.join("; ")))
            } else if call.body["keep"] == true {
                Action::notice(format!("{} network rule{} kept for {sb}", list("kept"), plural(list("kept"))))
            } else {
                Action::notice(format!("{} network rule{} removed from {sb}", list("removed"), plural(list("removed"))))
            }]
        }
        _ => vec![],
    }
}

fn failed(call: &Call, why: &str) -> Vec<Action> {
    if call.tag == "grants" || call.tag == "settle-ask" {
        log::warn!("browser: {}: {why}", call.what);
        return vec![];
    }
    vec![Action::error(format!("Could not {}: {why}", call.what))]
}

fn decode_seg(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// The socket (its own thread)

fn on_socket(id: &str, ev: SocketEvent) -> Vec<Action> {
    if id != ID {
        return vec![];
    }
    match ev {
        SocketEvent::Open => {
            with(|s| s.as_mut().map(|s| s.status = Status::Starting));
            vec![ui()]
        }
        SocketEvent::Text(line) => incoming(&line, Instant::now()),
        // The bridge speaks text only.
        SocketEvent::Binary(_) | SocketEvent::Reconnecting(_) => vec![],
        // The app closed it (hidden, or opened again on another sandbox): nothing to say.
        SocketEvent::Closed(why) if why == "closed" => vec![],
        SocketEvent::Closed(why) => lost(Some(why)),
    }
}

/// One line from the bridge.
fn incoming(line: &str, now: Instant) -> Vec<Action> {
    let Some(m) = proto::parse(line) else {
        with(|s| s.as_mut().map(|s| s.stats.refused += 1));
        log::debug!("browser: a line dropped ({} bytes)", line.len());
        return vec![];
    };
    match m {
        In::Frame { jpeg, .. } => match crate::surface::set_encoded(ID, &jpeg) {
            Ok((w, h)) => with(|s| {
                let Some(s) = s.as_mut() else { return vec![] };
                s.stats.frame(now);
                let first = s.status == Status::Starting;
                if first {
                    s.status = Status::Live;
                    s.error = None;
                }
                let resized = s.frame != (w, h);
                s.frame = (w, h);
                // The chrome is told on a change, and about once a second for the numbers.
                let stale = s.stats.last_view.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
                if first || resized || stale {
                    s.stats.last_view = Some(now);
                    vec![Action::Set { key: UI.into(), value: s.view() }]
                } else {
                    vec![]
                }
            }),
            Err(e) => {
                with(|s| s.as_mut().map(|s| s.stats.refused += 1));
                log::warn!("browser: a frame refused: {e}");
                vec![]
            }
        },
        In::Url { url, title } => {
            with(|s| s.as_mut().map(|s| (s.url, s.title) = (url, title)));
            vec![ui()]
        }
        In::Blocked { host, kind } => {
            let changed = with(|s| {
                let Some(s) = s.as_mut() else { return false };
                // Said again every few seconds while the page retries: nothing new.
                if s.ignored.contains(&host) || s.refused.iter().any(|(h, k)| *h == host && *k == kind) {
                    return false;
                }
                s.refused.retain(|(h, _)| *h != host);
                s.refused.push((host, kind));
                if s.refused.len() > 50 {
                    s.refused.remove(0);
                }
                true
            });
            if changed { vec![ui()] } else { vec![] }
        }
        In::Viewport(name) => {
            with(|s| s.as_mut().map(|s| s.viewport = name));
            vec![ui()]
        }
        In::Ready => {
            let (w, h) = crate::surface::view_size(ID);
            send(proto::size(w, h));
            with(|s| s.as_mut().map(|s| s.error = None));
            vec![ui()]
        }
        In::Note(msg) => vec![Action::error(format!("Browser: {msg}"))],
        In::Error(msg) => {
            with(|s| s.as_mut().map(|s| s.error = Some(msg)));
            vec![ui()]
        }
        In::Closed => closed_now(),
        In::Gone => lost(Some("the browser's bridge ended".into())),
    }
}

/// Chromium itself went away: settle (once).
fn closed_now() -> Vec<Action> {
    let sb = with(|s| s.as_ref().map(|s| s.sandbox.clone()));
    sb.map(|sb| ended(&sb)).unwrap_or_default()
}

/// The pipe ended. While closing, that is the end of the browser: settle.
/// Otherwise the panel says the connection ended.
fn lost(why: Option<String>) -> Vec<Action> {
    let st = with(|s| s.as_ref().map(|s| (s.status, s.sandbox.clone())));
    match st {
        Some((Status::Closing, sb)) => ended(&sb),
        Some(_) => {
            with(|s| {
                let s = s.as_mut().unwrap();
                s.status = Status::Ended;
                if s.error.is_none() {
                    let why = why.filter(|w| w != "closed").map(|w| format!(" ({})", proto::clean(&w, 200))).unwrap_or_default();
                    s.error = Some(format!("The connection to the sandbox browser ended{why}. Close and open the panel to reconnect."));
                }
            });
            vec![ui()]
        }
        None => vec![],
    }
}

// ---------------------------------------------------------------------------
// The person's hands (UI thread)

fn mods(m: crate::surface::Mod) -> Mods {
    Mods { alt: m.alt, ctrl: m.ctrl, meta: m.meta, shift: m.shift }
}

fn button(b: crate::surface::Button) -> &'static str {
    match b {
        crate::surface::Button::Left => "left",
        crate::surface::Button::Middle => "middle",
        crate::surface::Button::Right => "right",
    }
}

/// What a surface event sends, and what the UI does. Pure but for the
/// session's clock and stats, so the tests see each line.
fn input(ev: &SurfaceEvent, now: Instant) -> (Option<String>, Vec<Action>) {
    let live = with(|s| s.is_some());
    if !live {
        return (None, vec![]);
    }
    let waiting = |s: &mut Option<Session>| {
        if let Some(s) = s.as_mut() {
            s.stats.waiting.get_or_insert(now);
        }
    };
    match ev {
        SurfaceEvent::Move { x, y, buttons, mods: m, .. } => {
            // At most 25 a second, as the web UI.
            let send = with(|s| {
                let s = s.as_mut().unwrap();
                if s.last_move.is_some_and(|t| now.duration_since(t) < Duration::from_millis(40)) {
                    return false;
                }
                s.last_move = Some(now);
                true
            });
            (send.then(|| proto::mouse(Mouse::Moved, *x, *y, if buttons & 1 != 0 { "left" } else { "none" }, 0, mods(*m))), vec![])
        }
        SurfaceEvent::Press { x, y, button: b, clicks, mods: m, .. } => {
            with(waiting);
            (Some(proto::mouse(Mouse::Pressed, *x, *y, button(*b), *clicks, mods(*m))), vec![])
        }
        SurfaceEvent::Release { x, y, button: b, mods: m, .. } => (Some(proto::mouse(Mouse::Released, *x, *y, button(*b), 1, mods(*m))), vec![]),
        SurfaceEvent::Wheel { x, y, dx, dy, mods: m, .. } => (Some(proto::wheel(*x, *y, *dx, *dy, mods(*m))), vec![]),
        SurfaceEvent::Key { down, key, code, text, mods: m, .. } => {
            let l = proto::key(*down, key, code.as_deref(), text.as_deref(), mods(*m));
            if l.is_some() && *down {
                with(waiting);
            }
            (l, vec![])
        }
        SurfaceEvent::Text { text, .. } => {
            with(waiting);
            (proto::paste(text), vec![])
        }
        // The core already said how much was pasted, on the person's own press.
        SurfaceEvent::Paste { text, .. } => (proto::paste(text), vec![]),
        SurfaceEvent::Focus { focused, .. } => {
            with(|s| s.as_mut().map(|s| s.focused = *focused));
            (None, vec![ui()])
        }
        SurfaceEvent::F6 { .. } => (None, vec![Action::Focus(URL_INPUT.into())]),
        SurfaceEvent::Resized { w, h, .. } => (proto::size(*w, *h), vec![]),
    }
}

fn on_surface(ev: &SurfaceEvent) -> Vec<Action> {
    if ev.surface() != ID {
        return vec![];
    }
    let (line, acts) = input(ev, Instant::now());
    send(line);
    acts
}

// ---------------------------------------------------------------------------
// Extras

/// On the pane bars of every running sandbox: Browser, blue while its agent
/// is using it (the web UI's tab on the terminal's edge).
fn extras(cx: &Ctx) -> Vec<Extra> {
    let active: Vec<&str> = cx.get(ACTIVE)["sandboxes"].as_array().into_iter().flatten().filter_map(J::as_str).collect();
    let shown = with(|s| s.as_ref().map(|s| s.sandbox.clone()));
    cx.data["sandboxes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["status"] == "running")
        .filter_map(|s| s["name"].as_str())
        .map(|name| {
            let using = active.contains(&name);
            let open = shown.as_deref() == Some(name);
            Extra {
                place: Place::Pane(name.to_string()),
                id: "browser".into(),
                label: "Browser".into(),
                icon: Some("link".into()),
                tone: if using { "blue".into() } else { "muted".into() },
                help: match (open, using) {
                    (true, _) => "Hide the browser: it keeps running".into(),
                    (false, true) => format!("{name}'s agent is using its browser: watch it"),
                    (false, false) => format!("Open {name}'s own browser"),
                },
                count: None,
                intent: Some(if open { ("browser_hide".into(), json!({})) } else { ("browser_open".into(), json!({ "sandbox": name })) }),
            }
        })
        .collect()
}
