use super::*;
use crate::feat::{Standin, WsPeer};
use crate::surface::{Button, Mod};
use serde_json::Map;
use std::collections::VecDeque;

/// The session is one per app: tests that touch it take turns.
static ONE: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

fn fixture(name: &str) -> J {
    let raw = match name {
        "basic" => include_str!("../../../fixtures/browser/basic.json"),
        _ => include_str!("../../../fixtures/browser/starting.json"),
    };
    let v: J = serde_json::from_str(raw).unwrap();
    v["daemon"].clone()
}

fn ask(name: &'static str, args: J) -> Asked {
    Asked { name, args: args.as_object().cloned().unwrap_or_else(Map::new) }
}

fn run(d: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    act(&Ctx { data: d }, &ask(name, args))
}

fn spec(name: &str) -> &'static Spec {
    INTENTS.iter().find(|s| s.name == name).unwrap()
}

fn jpeg_line(w: u32, h: u32) -> String {
    let j = standin::picture(w, h, 1, 3, Some((10, 10)), 2);
    json!({ "t": "frame", "d": proto::b64_encode(&j), "w": w, "h": h }).to_string()
}

/// Every way an action could open something on the person's machine or
/// touch their clipboard.
fn leaks(acts: &[Action]) -> Option<String> {
    acts.iter().find_map(|a| match a {
        Action::OpenLink(u) => Some(format!("OpenLink {u}")),
        Action::ReadClipboard => Some("ReadClipboard".into()),
        Action::PickFiles { .. } => Some("PickFiles".into()),
        Action::Intent { name, .. } if name == "copy_text" || name == "open_link" => Some(format!("intent {name}")),
        Action::Notice { then: Some((n, _)), .. } if n == "copy_text" || n == "open_link" => Some(format!("notice offering {n}")),
        _ => None,
    })
}

fn opened(sb: &str) {
    with(|s| *s = Some(Session::new(sb)));
}

fn view() -> J {
    with(|s| s.as_ref().map(Session::view).unwrap_or(J::Null))
}

// ---------------------------------------------------------------------------
// The protocol

#[test]
fn a_frame_is_a_checked_jpeg_and_anything_else_is_dropped() {
    let good = jpeg_line(64, 40);
    assert!(proto::is_frame(&good));
    assert!(proto::is_frame(&format!(r#"{{"t":"frame","d":"{}"}}"#, "A".repeat(4000))));
    assert!(!proto::is_frame(&format!(r#"{{"t":"url","url":"{}"}}"#, "A".repeat(4000))));
    assert!(matches!(proto::parse(&good), Some(In::Frame { w: 64, h: 40, ref jpeg }) if jpeg.starts_with(&[0xFF, 0xD8])));
    let bad = [
        r#"{"t":"frame","d":"!!!!","w":10,"h":10}"#.to_string(),
        r#"{"t":"frame","d":"QUJD","w":10,"h":10}"#.to_string(),
        // Not a JPEG: a PNG's signature, base64 fine.
        json!({ "t": "frame", "d": proto::b64_encode(b"\x89PNG\r\n\x1a\nxxxx"), "w": 10, "h": 10 }).to_string(),
        json!({ "t": "frame", "d": proto::b64_encode(&[0xFF, 0xD8, 0xFF, 0]), "w": 99999, "h": 10 }).to_string(),
        json!({ "t": "frame", "d": proto::b64_encode(&[0xFF, 0xD8, 0xFF, 0]), "w": -3, "h": 10 }).to_string(),
        json!({ "t": "frame", "d": proto::b64_encode(&[0xFF, 0xD8, 0xFF, 0]), "w": "wide", "h": 10 }).to_string(),
        json!({ "t": "frame", "d": proto::b64_encode(&[0xFF, 0xD8, 0xFF, 0]) }).to_string(),
        json!({ "t": "frame", "d": 7, "w": 10, "h": 10 }).to_string(),
        // Padding in the middle, a length not a multiple of 4, a non-canonical end.
        r#"{"t":"frame","d":"/9j/=AAA","w":10,"h":10}"#.to_string(),
        r#"{"t":"frame","d":"/9j/AA","w":10,"h":10}"#.to_string(),
        r#"{"t":"frame","d":"/9j/AB==","w":10,"h":10}"#.to_string(),
        "not json".into(),
        r#"{"no":"t"}"#.into(),
        r#"{"t":"launch-missiles"}"#.into(),
        r#"{"t":"blocked","host":"evil host with spaces"}"#.into(),
        r#"{"t":"blocked","host":""}"#.into(),
        "x".repeat(proto::MAX_LINE + 1),
    ];
    for l in &bad {
        assert_eq!(proto::parse(l), None, "{}", &l[..l.len().min(80)]);
    }
    // Truncated mid-way: never a panic.
    for cut in [1, 10, good.len() / 2, good.len() - 1] {
        let _ = proto::parse(&good[..cut]);
    }
}

#[test]
fn base64_round_trips_and_is_strict() {
    for n in 0..40u8 {
        let b: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
        if b.is_empty() {
            continue;
        }
        assert_eq!(proto::b64_decode(&proto::b64_encode(&b)).unwrap(), b);
    }
    assert_eq!(proto::b64_decode("TWFu").unwrap(), b"Man");
    assert_eq!(proto::b64_decode("TWE=").unwrap(), b"Ma");
    assert!(proto::b64_decode("TW==").is_none() || proto::b64_decode("TW==").unwrap() == b"M");
    assert!(proto::b64_decode("TWFu\n").is_none());
    assert!(proto::b64_decode("TW-_").is_none());
}

#[test]
fn what_the_sandbox_says_is_bounded_one_line_text() {
    let l = json!({ "t": "url", "url": "https://x.test/\u{1b}[31mred\nline\u{202E}evil", "title": "a".repeat(10_000) }).to_string();
    let Some(In::Url { url, title }) = proto::parse(&l) else { panic!() };
    assert!(!url.chars().any(|c| c.is_control() || c == '\u{202E}'), "{url:?}");
    assert_eq!(title.chars().count(), proto::MAX_TEXT);
    let Some(In::Blocked { host, kind }) = proto::parse(r#"{"t":"blocked","host":"API.Example.com:443","kind":"weird"}"#) else { panic!() };
    // Unknown kinds are not allowable from here.
    assert_eq!((host.as_str(), kind), ("api.example.com:443", Kind::Rule));
    assert_eq!(proto::parse(r#"{"t":"error"}"#), Some(In::Error("The browser could not start.".into())));
}

#[test]
fn reserved_keys_are_never_sent_to_the_page() {
    let cmd = Mods { meta: true, ..Mods::default() };
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    for (k, m) in [("F6", Mods::default()), ("=", cmd), ("+", ctrl), ("-", cmd), ("0", ctrl), ("v", cmd), ("V", ctrl)] {
        assert_eq!(proto::key(true, k, None, None, m), None, "{k} {m:?}");
        assert_eq!(proto::key(false, k, None, None, m), None, "{k} up");
    }
    // Escape, Tab and plain letters go to the page, as the web UI.
    let esc: J = serde_json::from_str(&proto::key(true, "Escape", Some("Escape"), None, Mods::default()).unwrap()).unwrap();
    assert_eq!((esc["type"].as_str(), esc["vk"].as_u64()), (Some("rawKeyDown"), Some(27)));
    let tab: J = serde_json::from_str(&proto::key(true, "Tab", Some("Tab"), None, Mods::default()).unwrap()).unwrap();
    assert_eq!(tab["text"], "\t");
    let a: J = serde_json::from_str(&proto::key(true, "a", Some("KeyA"), Some("a"), Mods::default()).unwrap()).unwrap();
    assert_eq!((a["type"].as_str(), a["text"].as_str(), a["vk"].as_u64()), (Some("keyDown"), Some("a"), Some(65)));
    // Ctrl+A selects all: no text.
    let ca: J = serde_json::from_str(&proto::key(true, "a", Some("KeyA"), None, ctrl).unwrap()).unwrap();
    assert_eq!((ca["type"].as_str(), ca.get("text"), ca["modifiers"].as_u64()), (Some("rawKeyDown"), None, Some(2)));
    assert_eq!(proto::vk("ArrowLeft", "ArrowLeft"), 37);
    assert_eq!(proto::vk("é", "Digit2"), 50);
    assert_eq!(proto::vk("F5", ""), 116);
}

#[test]
fn the_surface_never_sends_a_kept_key_or_clipboard_text_except_on_a_paste() {
    let _t = turn();
    opened("web-app");
    let m = Mod::default();
    let s = || "browser".to_string();
    let now = Instant::now();
    let events = vec![
        SurfaceEvent::Move { surface: s(), x: 3.0, y: 4.0, buttons: 1, mods: m },
        SurfaceEvent::Press { surface: s(), x: 3.0, y: 4.0, button: Button::Right, clicks: 2, mods: m },
        SurfaceEvent::Release { surface: s(), x: 3.0, y: 4.0, button: Button::Right, mods: m },
        SurfaceEvent::Wheel { surface: s(), x: 3.0, y: 4.0, dx: 0.0, dy: 120.0, mods: m },
        SurfaceEvent::Key { surface: s(), down: true, key: "F6".into(), code: Some("F6".into()), text: None, mods: m },
        SurfaceEvent::Key { surface: s(), down: true, key: "v".into(), code: Some("KeyV".into()), text: None, mods: Mod { meta: true, ..m } },
        SurfaceEvent::Key { surface: s(), down: true, key: "x".into(), code: Some("KeyX".into()), text: Some("x".into()), mods: m },
        SurfaceEvent::Focus { surface: s(), focused: true },
        SurfaceEvent::F6 { surface: s() },
        SurfaceEvent::Resized { surface: s(), w: 700.0, h: 500.0 },
    ];
    let mut lines = vec![];
    for ev in &events {
        let (l, acts) = input(ev, now);
        assert!(leaks(&acts).is_none(), "{ev:?}");
        if let Some(l) = l {
            let v: J = serde_json::from_str(&l).unwrap();
            assert_ne!(v["t"], "paste", "{ev:?} sent text");
            assert!(!l.contains('\n'));
            lines.push(v);
        }
    }
    let ts: Vec<&str> = lines.iter().map(|v| v["type"].as_str().unwrap_or(v["t"].as_str().unwrap())).collect();
    assert_eq!(ts, ["mouseMoved", "mousePressed", "mouseReleased", "mouseWheel", "keyDown", "size"]);
    assert_eq!((lines[1]["button"].as_str(), lines[1]["clickCount"].as_u64()), (Some("right"), Some(2)));
    // A narrow panel lays out a desktop page, scaled down.
    assert_eq!((lines[5]["w"].as_u64(), lines[5]["h"].as_u64()), (Some(1024), Some(731)));
    // F6 moves the keyboard to the address bar.
    let (_, a) = input(&SurfaceEvent::F6 { surface: s() }, now);
    assert_eq!(a, vec![Action::Focus(URL_INPUT.into())]);
    // The person's paste, and an input method's text: the only clipboard-like lines.
    let (l, _) = input(&SurfaceEvent::Paste { surface: s(), text: "hello\nworld".into() }, now);
    let v: J = serde_json::from_str(&l.unwrap()).unwrap();
    assert_eq!((v["t"].as_str(), v["text"].as_str()), (Some("paste"), Some("hello\nworld")));
    let (l, _) = input(&SurfaceEvent::Text { surface: s(), text: "é".into() }, now);
    assert!(l.unwrap().contains("\"paste\""));
    // A huge paste is cut to what the daemon takes.
    let big = "€".repeat(proto::MAX_PASTE + 10);
    let (l, _) = input(&SurfaceEvent::Paste { surface: s(), text: big }, now);
    assert!(l.unwrap().len() <= proto::MAX_OUT);
    // Moves are throttled to 25 a second.
    let mv = SurfaceEvent::Move { surface: s(), x: 1.0, y: 1.0, buttons: 0, mods: m };
    assert!(input(&mv, now + Duration::from_millis(10)).0.is_none());
    assert!(input(&mv, now + Duration::from_millis(60)).0.is_some());
    // No panel: nothing goes anywhere.
    with(|s| *s = None);
    assert_eq!(input(&events[1], now).0, None);
}

#[test]
fn the_address_bar_is_text_for_the_sandbox_never_opened_here() {
    let _t = turn();
    let d = fixture("basic");
    opened("web-app");
    for url in ["https://example.org/", "file:///etc/passwd", "javascript:alert(1)", "sbxw://x", "example.org"] {
        let a = run(&d, "browser_nav", json!({ "url": url })).unwrap();
        assert!(leaks(&a).is_none(), "{url}: {a:?}");
        assert_eq!(a, vec![Action::Focus("surface-browser".into())]);
        let line: J = serde_json::from_str(&proto::nav(url).unwrap()).unwrap();
        assert_eq!(line["url"], url);
    }
    assert!(run(&d, "browser_nav", json!({ "url": "two\nlines" })).is_err());
    assert!(proto::nav(&"a".repeat(proto::MAX_URL + 1)).is_none());
    // Whatever the page says (an address, a note, an error) is text in the chrome.
    let lines = [
        json!({ "t": "url", "url": "https://evil.test/", "title": "Click https://evil.test" }).to_string(),
        json!({ "t": "note", "msg": "open https://evil.test now" }).to_string(),
        json!({ "t": "error", "msg": "see file:///etc" }).to_string(),
        json!({ "t": "viewport", "name": "https://x" }).to_string(),
        json!({ "t": "blocked", "host": "evil.test", "kind": "" }).to_string(),
    ];
    for l in &lines {
        let a = incoming(l, Instant::now());
        assert!(leaks(&a).is_none(), "{l}: {a:?}");
    }
    assert_eq!(view()["url"], "https://evil.test/");
    with(|s| *s = None);
}

#[test]
fn no_intent_and_no_message_reaches_the_persons_machine() {
    let _t = turn();
    let d = fixture("basic");
    for s in INTENTS {
        opened("web-app");
        incoming(r#"{"t":"blocked","host":"fonts.googleapis.com","kind":""}"#, Instant::now());
        let args = match s.name {
            "browser_open" => json!({ "sandbox": "web-app" }),
            "browser_nav" => json!({ "url": "https://x.test" }),
            "browser_viewport" => json!({ "value": "mobile" }),
            "browser_allow" | "browser_ignore" => json!({ "host": "fonts.googleapis.com" }),
            "browser_settle" => json!({ "keep": true }),
            _ => json!({}),
        };
        let mut d = d.clone();
        d["browser_settle"] = json!({ "sandbox": "web-app", "hosts": ["a.test"] });
        if let Ok(a) = run(&d, s.name, args) {
            assert!(leaks(&a).is_none(), "{}: {a:?}", s.name);
            if let Risk::Approval(_) = s.risk {
                assert!(matches!(a.as_slice(), [Action::Confirm(_)]), "{}: {a:?}", s.name);
            }
        }
    }
    with(|s| *s = None);
}

#[test]
fn only_a_host_the_bridge_refused_can_be_allowed_and_never_an_org_one() {
    let _t = turn();
    let d = fixture("basic");
    assert_eq!(spec("browser_allow").risk, Risk::Approval("Allow a sandbox to reach a host"));
    opened("web-app");
    assert!(run(&d, "browser_allow", json!({ "host": "anything.test" })).unwrap_err().contains("not refused"));
    incoming(r#"{"t":"blocked","host":"fonts.googleapis.com","kind":""}"#, Instant::now());
    incoming(r#"{"t":"blocked","host":"pastebin.com","kind":"org"}"#, Instant::now());
    assert!(run(&d, "browser_allow", json!({ "host": "pastebin.com" })).unwrap_err().contains("organisation"));
    let a = run(&d, "browser_allow", json!({ "host": "FONTS.googleapis.com" })).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
    assert_eq!(c.call.path, "/api/sandboxes/web-app/browser/allow");
    assert_eq!(c.call.body, json!({ "host": "fonts.googleapis.com" }));
    assert!(c.title.contains("web-app") && c.lines.iter().any(|l| l.contains("only")));
    // Allowed: the chip goes, the count grows, a notice says it.
    let after = on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "ok": true, "grant": { "host": "fonts.googleapis.com", "rules": ["r"] } }))));
    assert!(after.iter().any(|a| matches!(a, Action::Notice { text, .. } if text.contains("fonts.googleapis.com allowed for web-app only"))));
    let v = view();
    assert_eq!(v["allowed"], 1);
    assert_eq!(v["blocked"].as_array().unwrap().len(), 1);
    // The same host said again is not shown twice; an ignored one never again.
    incoming(r#"{"t":"blocked","host":"pastebin.com","kind":"org"}"#, Instant::now());
    run(&d, "browser_ignore", json!({ "host": "pastebin.com" })).unwrap();
    incoming(r#"{"t":"blocked","host":"pastebin.com","kind":"org"}"#, Instant::now());
    assert_eq!(view()["blocked"].as_array().unwrap().len(), 0);
    with(|s| *s = None);
}

#[test]
fn frames_go_to_the_surface_and_a_bad_one_changes_nothing() {
    let _t = turn();
    opened("web-app");
    let t0 = Instant::now();
    let a = incoming(r#"{"t":"frame","d":"!!","w":10,"h":10}"#, t0);
    assert!(a.is_empty());
    assert_eq!(view()["status"], "starting");
    // A JPEG header on garbage: the core's decoder refuses it, no crash.
    let junk = json!({ "t": "frame", "d": proto::b64_encode(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4]), "w": 10, "h": 10 }).to_string();
    assert!(incoming(&junk, t0).is_empty());
    assert_eq!(view()["stats"]["refused"], 2);
    let a = incoming(&jpeg_line(320, 200), t0);
    assert!(matches!(a.as_slice(), [Action::Set { key, value }] if key == UI && value["status"] == "live"));
    assert_eq!(crate::surface::picture_size(ID), (320, 200));
    // The same size within a second: no re-render of the chrome.
    assert!(incoming(&jpeg_line(320, 200), t0 + Duration::from_millis(30)).is_empty());
    // A press, then the next picture: the latency is measured.
    let (_, _) = input(&SurfaceEvent::Press { surface: ID.into(), x: 1.0, y: 1.0, button: Button::Left, clicks: 1, mods: Mod::default() }, t0 + Duration::from_millis(40));
    incoming(&jpeg_line(320, 200), t0 + Duration::from_millis(52));
    assert_eq!(with(|s| s.as_ref().unwrap().stats.median()), Some(12));
    with(|s| *s = None);
    crate::surface::clear(ID);
}

#[test]
fn closing_settles_the_hosts_and_a_lost_connection_says_so() {
    let _t = turn();
    let d = fixture("basic");
    opened("web-app");
    // The connection drops on its own: said, nothing settled.
    let a = on_socket(ID, SocketEvent::Closed("the daemon went away".into()));
    assert!(a.iter().all(|a| !matches!(a, Action::Call(_))));
    let v = view();
    assert_eq!(v["status"], "ended");
    assert!(v["error"].as_str().unwrap().contains("the daemon went away"));
    // The app closed it itself: nothing to say.
    opened("web-app");
    assert!(on_socket(ID, SocketEvent::Closed("closed".into())).is_empty());
    assert_eq!(view()["status"], "starting");
    // Chromium closed (the agent, or Close browser): the grants are read, then asked about.
    let a = incoming(r#"{"t":"closed"}"#, Instant::now());
    let call = a.iter().find_map(|a| if let Action::Call(c) = a { Some(c.clone()) } else { None }).expect("grants read");
    assert_eq!((call.path.as_str(), call.tag.as_str()), ("/api/sandboxes/web-app/browser/grants", "settle-ask"));
    assert!(a.contains(&Action::ClosePanel(SCREEN.into())));
    assert!(view().is_null());
    let q = on_answer(&Ctx { data: &d }, &call, Ok((200, json!({ "ok": true, "grants": [{ "host": "a.test\u{1b}[0m", "rules": [] }] }))));
    assert!(q.contains(&Action::OpenModal { screen: SETTLE.into(), arg: json!("web-app") }));
    assert!(matches!(&q[0], Action::Set { value, .. } if value["hosts"] == json!(["a.test [0m"])));
    // Nothing allowed: nothing asked.
    assert!(on_answer(&Ctx { data: &d }, &call, Ok((200, json!({ "ok": true, "grants": [] })))).is_empty());
    // The answer.
    let mut d2 = d.clone();
    d2["browser_settle"] = json!({ "sandbox": "web-app", "hosts": ["a.test"] });
    let a = run(&d2, "browser_settle", json!({ "keep": false })).unwrap();
    let Some(Action::Call(c)) = a.iter().find(|a| matches!(a, Action::Call(_))) else { panic!() };
    assert_eq!((c.path.as_str(), &c.body), ("/api/sandboxes/web-app/browser/settle", &json!({ "keep": false })));
    let n = on_answer(&Ctx { data: &d2 }, c, Ok((200, json!({ "ok": true, "settled": { "kept": [], "removed": ["a.test"], "failed": [] } }))));
    assert_eq!(n, vec![Action::notice("1 network rule removed from web-app")]);
    let n = on_answer(&Ctx { data: &d2 }, c, Ok((200, json!({ "ok": true, "settled": { "kept": [], "removed": [], "failed": [{ "host": "a.test", "why": "sbx failed" }] } }))));
    assert!(matches!(&n[0], Action::Notice { error: true, text, .. } if text.contains("a.test (sbx failed)")));
}

#[test]
fn opening_needs_a_running_sandbox_and_connects_without_reconnect() {
    let _t = turn();
    let d = fixture("basic");
    assert!(run(&d, "browser_open", json!({ "sandbox": "data-pipeline" })).unwrap_err().contains("not running"));
    assert!(run(&d, "browser_open", json!({ "sandbox": "nope" })).is_err());
    let a = run(&d, "browser_open", json!({ "sandbox": "web-app" })).unwrap();
    let sock = a.iter().find_map(|a| if let Action::OpenSocket { id, path, opts } = a { Some((id.clone(), path.clone(), *opts)) } else { None }).unwrap();
    assert_eq!((sock.0.as_str(), sock.1.as_str()), ("browser", "/ws/browser?sandbox=web-app"));
    assert!(!sock.2.reconnect && sock.2.one_line && sock.2.droppable.is_some());
    assert!(a.contains(&Action::OpenPanel(SCREEN.into())));
    // Hiding leaves the browser running: the socket closes, no quit is sent.
    let h = run(&d, "browser_hide", json!({})).unwrap();
    assert!(h.contains(&Action::CloseSocket(ID.into())));
    assert!(view().is_null());
}

#[test]
fn the_pane_bar_offers_the_browser_green_while_the_agent_uses_it() {
    let _t = turn();
    with(|s| *s = None);
    let d = fixture("basic");
    let e = extras(&Ctx { data: &d });
    let names: Vec<_> = e.iter().map(|x| (x.place.clone(), x.tone.clone())).collect();
    assert_eq!(names, vec![(Place::Pane("web-app".into()), "green".to_string()), (Place::Pane("docs".into()), "muted".to_string())]);
}

// ---------------------------------------------------------------------------
// The stand-in

struct Peer {
    to_app: Vec<String>,
    from_app: VecDeque<String>,
    closed: bool,
}

impl WsPeer for Peer {
    fn send(&mut self, text: &str) -> bool {
        self.to_app.push(text.to_string());
        !self.closed
    }
    fn send_binary(&mut self, _: &[u8]) -> bool {
        false
    }
    fn recv(&mut self, wait: Duration) -> Result<Option<String>, String> {
        match self.from_app.pop_front() {
            Some(m) => Ok(Some(m)),
            None if self.closed => Err("closed".into()),
            None => {
                std::thread::sleep(wait.min(Duration::from_millis(5)));
                // Out of script: the app leaves.
                self.closed = true;
                Ok(None)
            }
        }
    }
    fn close(&mut self) {
        self.closed = true;
    }
}

#[test]
fn the_standin_plays_the_bridge_and_answers_input_with_a_picture() {
    assert!(!standin::socket("/ws/other", &mut Peer { to_app: vec![], from_app: VecDeque::new(), closed: false }));
    let script = [
        r#"{"t":"size","w":1024,"h":700}"#,
        r#"{"t":"mouse","type":"mousePressed","x":100,"y":80,"button":"left","clickCount":1,"modifiers":0}"#,
        r#"{"t":"nav","url":"https://registry.npmjs.org/"}"#,
        r#"{"t":"nav","url":"file:///etc/passwd"}"#,
        r#"{"t":"quit"}"#,
    ];
    let mut p = Peer { to_app: vec![], from_app: script.iter().map(|s| s.to_string()).collect(), closed: false };
    assert!(standin::socket("/ws/browser?sandbox=web-app", &mut p));
    let parsed: Vec<Option<In>> = p.to_app.iter().map(|l| proto::parse(l)).collect();
    assert_eq!(parsed[0], Some(In::Ready));
    // The two broken frames it sends on purpose are refused by the parser.
    assert_eq!(parsed.iter().filter(|m| m.is_none()).count(), 2);
    assert!(parsed.iter().any(|m| matches!(m, Some(In::Frame { w: 1024, h: 700, .. }))));
    assert!(parsed.iter().any(|m| matches!(m, Some(In::Blocked { host, .. }) if host == "registry.npmjs.org")));
    assert!(parsed.iter().any(|m| matches!(m, Some(In::Note(n)) if n.contains("http"))));
    assert_eq!(parsed[parsed.len() - 2..], [Some(In::Closed), Some(In::Gone)]);
}

#[test]
fn the_standin_routes_keep_and_settle_grants() {
    let mut d = fixture("starting");
    let mut publish = |_: &str, _: J| {};
    let mut st = Standin { data: &mut d, publish: &mut publish };
    assert_eq!(standin::route(&mut st, "GET", "/api/browser/active", &J::Null).unwrap().1["sandboxes"], json!(["web-app"]));
    assert_eq!(standin::route(&mut st, "POST", "/api/sandboxes/docs/browser/allow", &json!({ "host": "a.test" })).unwrap().0, 200);
    assert_eq!(standin::route(&mut st, "POST", "/api/sandboxes/docs/browser/allow", &json!({ "host": "bad host" })).unwrap().0, 400);
    assert_eq!(standin::route(&mut st, "GET", "/api/sandboxes/docs/browser/grants", &J::Null).unwrap().1["grants"][0]["host"], "a.test");
    let s = standin::route(&mut st, "POST", "/api/sandboxes/docs/browser/settle", &json!({ "keep": false })).unwrap().1;
    assert_eq!(s["settled"]["removed"], json!(["a.test"]));
    assert_eq!(standin::route(&mut st, "GET", "/api/sandboxes/docs/browser/grants", &J::Null).unwrap().1["grants"], json!([]));
}

/// Where the time of one frame goes, on this machine: run by hand,
/// `cargo test [--release] --lib frame_cost -- --ignored --nocapture`.
/// Numbers in BROWSER-MAC.md.
#[test]
#[ignore = "a measurement, run by hand"]
fn frame_cost() {
    let _t = turn();
    let jpeg = standin::picture(1280, 800, 1, 7, Some((300, 200)), 12);
    let line = format!(r#"{{"t":"frame","d":"{}","w":1280,"h":800}}"#, proto::b64_encode(&jpeg));
    let n = 60;
    let time = |what: &str, f: &mut dyn FnMut()| {
        f();
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("{what:<44} {per:8.2} ms");
        per
    };
    println!("one 1280x800 frame: JPEG {} KB, line {} KB", jpeg.len() / 1024, line.len() / 1024);
    time("stand-in: draw and JPEG-encode", &mut || {
        let _ = standin::picture(1280, 800, 1, 7, Some((300, 200)), 12);
    });
    time("is_frame (socket I/O thread)", &mut || assert!(proto::is_frame(&line)));
    let a = time("parse: JSON + base64 + checks", &mut || assert!(proto::parse(&line).is_some()));
    let b = time("decode: picture::decode_rgba", &mut || {
        crate::picture::decode_rgba(&jpeg, crate::surface::frame_limits()).unwrap();
    });
    let (w, h, rgba) = crate::picture::decode_rgba(&jpeg, crate::surface::frame_limits()).unwrap();
    let c = time("hand-over: surface::set_rgba (copy)", &mut || crate::surface::set_rgba("bench", w, h, rgba.clone()).unwrap());
    opened("web-app");
    let t0 = Instant::now();
    let mut k = 0u32;
    let d = time("all of `incoming` (parse, decode, set, stats)", &mut || {
        k += 1;
        incoming(&line, t0 + Duration::from_millis(k as u64 * 5));
    });
    println!("parse + decode + hand-over {:.2} ms; the feature's whole path {d:.2} ms: at most {:.0} frames/s on the socket's thread", a + b + c, 1000.0 / d);
    with(|s| *s = None);
    crate::surface::clear("bench");
    crate::surface::clear(ID);
}
