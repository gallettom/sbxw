//! A stand-in for the sbxw daemon, for when none is reachable: the same
//! routes and protocols as the routes the app covers, a local shell instead
//! of a sandbox.
//!
//! It speaks what the real daemon speaks (see `src/client.rs` and
//! the route list in `.sbxw-artifacts/desktop/daemon-api.md` at the repository root, outside git): `GET /api/health?nonce=` (proof), `GET
//! /api/sandboxes`, `POST /api/sandboxes/:name/stop`, `GET /api/stream`
//! (server-sent events) and `GET /ws` (binary PTY bytes both ways, JSON
//! `resize` text frames in, `size` frames out with `size=1`), all behind
//! `Authorization: Bearer`.
//!
//! Sandbox states change as on the real daemon, roughly: opening the agent
//! (`/ws` without `mode`, or `mode=claude`) of a stopped sandbox makes it
//! `running` half a second later; a stop makes it `stopped`. Each change is
//! followed by a `session` event on the stream, which is what makes the app
//! read the list again (the real daemon has no "list changed" event either).
//!
//! Jarvis (`--jarvis absent|running`): `GET /api/jarvis/status`, `POST
//! /api/jarvis/start` (creating, then running 1.5 s later), `GET
//! /api/jarvis`, `POST /api/jarvis/proposals/:id/approve|refuse` (the
//! daemon's rule: voice only for low risk, `ack_host_command` for a host
//! command, else 409), `GET /api/sessions`, `POST /api/chat/push` and `POST
//! /api/voice/anonymise` ("Jeanne Martin" is masked). A scripted Jarvis
//! answers what is pushed to `sbxw-jarvis` with a `session` event (working,
//! then idle with a `reply`); words in the message make it propose: "english"
//! a low-risk setting, "review" a high-risk kit, "command" a stdio MCP
//! server, "screen" a change of the `sandboxes` screen, written to
//! `--screens-dir` when approved (so the app's hot reload shows it). `GET
//! /api/voice` answers 404 (a daemon without voice). "voix" or "change the
//! voice" makes it ask a question with options (`AskUserQuestion`: state
//! `attention` with `steps`), which the next message answers; "options" makes
//! it answer a sentence and a numbered list. Every hook publishes one
//! `session` event per session of the sandbox, with `session_id` and
//! `started_ms`, as the daemon does; `--stale-session yes` keeps an older
//! session left as `working`. `--clients N` reports N stream clients.
//! A fixture's `daemon.session_events` (`[{after, session}]`, the Monitor's
//! demo) replace a session's report `after` seconds from the first stream
//! client and publish it as a `session` event.
//!
//! Terminal size, as the daemon: the PTY has one size, the client used last
//! sets it, and a socket opened with `size=1` hears
//! `{"type":"size","cols","rows","owner"}` first and on every change. There
//! is one client per PTY here, so a second client is simulated: `POST
//! /api/standin/other-client` with `{"cols","rows","type"}` makes "another
//! window" take every open terminal's size (and type `type` into it, as that
//! window's person would); typing in the app, or its `resize`, takes it back.
//!
//! What it does not do: keep the PTY across reconnects, replay the last
//! 256 KB, Host/Origin checks, threads, monitor mode.
//!
//! Usage: `standin <addr> <token> [--fixture file.json] [--token-file path]
//! [--change SECS:NAME=STATUS]...`. `--change` sets a sandbox's status that
//! many seconds after the first `/api/stream` client connects (a change made
//! behind the app's back, for the live-update demo). `--token-file` writes
//! the token there, as the daemon writes `~/.sbxw/run/ui-token`.
//! `--stats full|partial|none`: `GET /api/stats` and its `stats` event every
//! 5 s (default partial: every other running sandbox without a report), and
//! a `cost_usd` on each running sandbox; `none` is a daemon without them.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use sbxw_desktop::client::{proof, Sandbox};
use std::io::{Cursor, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tungstenite::handshake::server::{Request, Response};
use tungstenite::Message;

const DEFAULT_FIXTURE: &str = include_str!("../../fixtures/sandboxes/basic.json");

#[derive(Default)]
struct State {
    sandboxes: Vec<Sandbox>,
    streams: Vec<mpsc::Sender<String>>,
    streamed: bool,
    jarvis: String,
    proposals: Vec<serde_json::Value>,
    /// Jarvis's sessions as the daemon keeps them, by session id (one
    /// event per session of the sandbox on every hook, as agents.rs does).
    sessions: std::collections::BTreeMap<String, serde_json::Value>,
    /// Jarvis asked a question (`AskUserQuestion`): the next words answer it.
    asked: bool,
    /// `--clients N`: other clients of the stream, reported in `clients`.
    others: u64,
    screens_dir: Option<std::path::PathBuf>,
    next: u64,
    voice: bool,
    /// The fixture's whole `daemon` object: features' routes answer from it
    /// and change it (`Feature::standin`, `feat::answer_later`).
    raw: serde_json::Value,
    /// `--approval SECS`: sbx asks (a host for web-app) that long after the first stream client.
    approval_after: Option<f64>,
    /// `--stats full|partial|none`: GET /api/stats and the `stats` event.
    stats: String,
    /// Stats sent so far (the figures move a little each time).
    stats_n: u64,
}

type Shared = Arc<Mutex<State>>;

/// "Another window" takes the size: (cols, rows, text it types).
type OtherClient = (u16, u16, String);

/// Every open terminal's line for [`OtherClient`] messages.
static TERMINALS: Mutex<Vec<mpsc::Sender<OtherClient>>> = Mutex::new(Vec::new());

/// The daemon's rule (`crates/daemon/src/pty.rs`, `is_typed_input`), short:
/// the terminal's answers to the program (cursor, attributes, focus) are not
/// someone typing.
fn is_typed(d: &[u8]) -> bool {
    match d {
        [] | [0x1b, b']' | b'P', ..] => false,
        [0x1b, b'[', rest @ ..] => {
            let last = rest.last().copied().unwrap_or(0);
            !(rest == b"I" || rest == b"O" || matches!(last, b'R' | b'c' | b'n' | b't' | b'y') && rest[..rest.len() - 1].iter().all(|c| c.is_ascii_digit() || b";?>=$ ".contains(c)))
        }
        _ => true,
    }
}

fn publish(st: &Shared, name: &str, data: &str) {
    let frame = format!("event: {name}\ndata: {data}\n\n");
    st.lock().unwrap().streams.retain(|t| t.send(frame.clone()).is_ok());
}

/// GET /api/stats as the daemon answers it (`--stats`): each running
/// sandbox's CPU (percent of one core) and memory, or nulls where it has no
/// recent report; the daemon's own; their total.
fn stats_body(st: &Shared) -> Option<serde_json::Value> {
    let mut g = st.lock().unwrap();
    if g.stats == "none" {
        return None;
    }
    g.stats_n += 1;
    let n = g.stats_n;
    let partial = g.stats == "partial";
    let gb = 1024.0 * 1024.0 * 1024.0;
    let mut total = 0.8;
    let (mut counted, mut unknown) = (0, 0);
    let list: Vec<serde_json::Value> = g
        .sandboxes
        .iter()
        .filter(|s| s.status == "running")
        .enumerate()
        .map(|(i, s)| {
            if partial && i % 2 == 1 {
                unknown += 1;
                return serde_json::json!({ "name": s.name, "cpu_pct": null, "cpu_age_s": null, "cpu_cores": null, "mem_bytes": null, "mem_total_bytes": null });
            }
            counted += 1;
            let cpu = ((40.0 + 30.0 * i as f64 + ((n + i as u64) % 5) as f64 * 3.5) * 10.0).round() / 10.0;
            total += cpu;
            serde_json::json!({ "name": s.name, "cpu_pct": cpu, "cpu_age_s": 4, "cpu_cores": 4, "mem_bytes": ((1.2 + i as f64 * 0.4) * gb) as u64, "mem_total_bytes": (8.0 * gb) as u64 })
        })
        .collect();
    Some(serde_json::json!({ "daemon_cpu_pct": 0.8, "sandboxes": list, "total_cpu_pct": (total * 10.0f64).round() / 10.0, "sandboxes_counted": counted, "sandboxes_unknown": unknown, "ts": 1763380000000u64 + n * 5000 }))
}

fn set_status(st: &Shared, name: &str, status: &str) {
    let found = {
        let mut s = st.lock().unwrap();
        match s.sandboxes.iter_mut().find(|x| x.name == name) {
            Some(x) => {
                x.status = status.to_string();
                true
            }
            None => false,
        }
    };
    if found {
        eprintln!("standin: {name} is now {status}");
        let state = if status == "running" { "working" } else { "exited" };
        publish(st, "session", &serde_json::json!({ "sandbox": name, "state": state }).to_string());
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:7399".into());
    let token = args.next().unwrap_or_else(|| "test-token".into());
    let mut fixture = DEFAULT_FIXTURE.to_string();
    let mut changes: Vec<(f64, String, String)> = vec![];
    let mut jarvis = "absent".to_string();
    let mut screens_dir = None;
    let mut voice = false;
    let mut stale = false;
    let mut others = 0;
    let mut approval_after = None;
    let mut stats = "partial".to_string();
    while let Some(k) = args.next() {
        let v = args.next().unwrap_or_default();
        match k.as_str() {
            "--fixture" => fixture = std::fs::read_to_string(&v)?,
            "--token-file" => {
                if let Some(d) = std::path::Path::new(&v).parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&v, &token)?;
            }
            "--change" => {
                let (t, rest) = v.split_once(':').ok_or_else(|| anyhow::anyhow!("--change SECS:NAME=STATUS"))?;
                let (n, s) = rest.split_once('=').ok_or_else(|| anyhow::anyhow!("--change SECS:NAME=STATUS"))?;
                changes.push((t.parse()?, n.into(), s.into()));
            }
            "--jarvis" => jarvis = v,
            // `--voice TEXT`: the voice is ready, and every recording is heard as TEXT.
            "--voice" => {
                voice = true;
                std::env::set_var("STANDIN_HEARD", v);
            }
            "--screens-dir" => screens_dir = Some(std::path::PathBuf::from(v)),
            // `--stale-session yes`: an older Jarvis session left as `working`
            // (its hooks stopped), as the first real run on a Mac suggested.
            "--stale-session" => stale = v == "yes",
            "--approval" => approval_after = Some(v.parse()?),
            "--clients" => others = v.parse::<u64>().unwrap_or(1).saturating_sub(1),
            // `--stats full`: every running sandbox reports its CPU; `partial`
            // (the default): every other one has no recent report; `none`:
            // a daemon without the route (404, no event).
            "--stats" => stats = v,
            _ => anyhow::bail!("unknown argument {k}"),
        }
    }
    let fx: serde_json::Value = serde_json::from_str(&fixture)?;
    let mut sandboxes: Vec<Sandbox> = serde_json::from_value(fx["daemon"]["sandboxes"].clone()).unwrap_or_default();
    // What the running sandboxes' sessions cost so far (`cost_usd`, the
    // daemon's sum per sandbox), unless the fixture says.
    if stats != "none" {
        for (i, sb) in sandboxes.iter_mut().filter(|s| s.status == "running").enumerate() {
            sb.extra.entry("cost_usd").or_insert_with(|| serde_json::json!(((i as f64 + 1.0) * 1.37 * 100.0).round() / 100.0));
        }
    }
    let mut sessions = std::collections::BTreeMap::new();
    if stale {
        sessions.insert("z-old".to_string(), serde_json::json!({ "sandbox": "sbxw-jarvis", "mode": "claude", "state": "working", "agent": "claude",
            "started_ms": 1, "session_id": "z-old", "origin": "tty", "answerable": false, "ts": 0 }));
    }
    let raw = if fx["daemon"].is_object() { fx["daemon"].clone() } else { serde_json::json!({}) };
    let st: Shared = Arc::new(Mutex::new(State { sandboxes, jarvis, screens_dir, voice, sessions, others, raw, approval_after, stats, ..Default::default() }));
    let listener = TcpListener::bind(&addr)?;
    eprintln!("standin: listening on {addr}");
    let changes = Arc::new(Mutex::new(Some(changes)));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (token, st, changes) = (token.clone(), st.clone(), changes.clone());
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, &token, &st, &changes) {
                eprintln!("standin: {e:#}");
            }
        });
    }
    Ok(())
}

/// Bytes already read, then the socket.
struct Prefixed {
    pre: Cursor<Vec<u8>>,
    s: TcpStream,
}
impl Read for Prefixed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.pre.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        self.s.read(buf)
    }
}
impl Write for Prefixed {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.s.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.s.flush()
    }
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
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

type Changes = Arc<Mutex<Option<Vec<(f64, String, String)>>>>;

fn serve(mut s: TcpStream, token: &str, st: &Shared, changes: &Changes) -> anyhow::Result<()> {
    let mut head = Vec::new();
    let mut b = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut b)?;
        if n == 0 || head.len() > 16 * 1024 {
            return Ok(());
        }
        head.extend_from_slice(&b[..n]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let method = text.split_whitespace().next().unwrap_or("GET").to_string();
    let target = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let bearer = text.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim())).and_then(|v| v.strip_prefix("Bearer "));
    let authed = bearer == Some(token);
    let reply = |s: &mut TcpStream, code: &str, body: String| {
        let _ = write!(s, "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    };
    let stop = path.strip_prefix("/api/sandboxes/").and_then(|r| r.strip_suffix("/stop")).map(decode);
    if authed && method == "POST" && path == "/api/standin/other-client" {
        let body = read_body(&mut s, &head, &text)?;
        let cols = body["cols"].as_u64().unwrap_or(70) as u16;
        let rows = body["rows"].as_u64().unwrap_or(16) as u16;
        let typed = body["type"].as_str().unwrap_or("").to_string();
        let mut list = TERMINALS.lock().unwrap();
        list.retain(|t| t.send((cols, rows, typed.clone())).is_ok());
        eprintln!("standin: another client takes {cols}x{rows} ({} terminals)", list.len());
        reply(&mut s, "200 OK", serde_json::json!({ "ok": true, "terminals": list.len() }).to_string());
        return Ok(());
    }
    if authed && (path.starts_with("/api/jarvis") || matches!(path, "/api/sessions" | "/api/chat/push" | "/api/voice" | "/api/voice/anonymise" | "/api/voice/transcribe")) {
        let body = read_body(&mut s, &head, &text)?;
        // A push to another sandbox than Jarvis's: features' stand-ins (relay follows one).
        if path == "/api/chat/push" && body["name"].as_str().is_some_and(|n| n != "sbxw-jarvis") {
            feature_route(st, &method, &target, &body, &mut s);
            return Ok(());
        }
        let (code, out) = jarvis_route(st, &method, path, &body);
        reply(&mut s, code, out.to_string());
        return Ok(());
    }
    match (method.as_str(), path) {
        ("GET", "/api/health") => {
            let nonce = query_value(query, "nonce").unwrap_or("");
            reply(&mut s, "200 OK", serde_json::json!({ "ok": true, "proof": proof(token, nonce) }).to_string());
        }
        _ if !authed => reply(&mut s, "401 Unauthorized", "\"not signed in\"".into()),
        ("GET", "/api/sandboxes") => {
            let list = st.lock().unwrap().sandboxes.clone();
            reply(&mut s, "200 OK", serde_json::to_string(&list)?);
        }
        ("POST", _) if stop.is_some() => {
            let name = stop.unwrap();
            if st.lock().unwrap().sandboxes.iter().any(|x| x.name == name) {
                // `sbx stop` takes a moment.
                std::thread::sleep(Duration::from_millis(300));
                set_status(st, &name, "stopped");
                reply(&mut s, "200 OK", "{\"ok\":true}".into());
            } else {
                reply(&mut s, "404 Not Found", serde_json::json!({ "error": format!("no sandbox '{name}'") }).to_string());
            }
        }
        ("GET", "/api/stats") => match stats_body(st) {
            Some(v) => reply(&mut s, "200 OK", v.to_string()),
            None => reply(&mut s, "404 Not Found", serde_json::json!({ "error": "no route GET /api/stats" }).to_string()),
        },
        ("GET", "/api/stream") => {
            let (tx, rx) = mpsc::channel::<String>();
            {
                let mut g = st.lock().unwrap();
                g.streams.push(tx);
                if !g.streamed {
                    g.streamed = true;
                    // The daemon's `stats` event: every 5 s while a stream is open.
                    if g.stats != "none" {
                        let st = st.clone();
                        std::thread::spawn(move || loop {
                            std::thread::sleep(Duration::from_secs(5));
                            if let Some(v) = stats_body(&st) {
                                publish(&st, "stats", &v.to_string());
                            }
                        });
                    }
                    if let Some(t) = g.approval_after {
                        let st = st.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_secs_f64(t));
                            new_approval(&st);
                        });
                    }
                    // The fixture's `session_events` (the Monitor's demo): each
                    // `{after, session}` replaces that session's report in the
                    // list `after` seconds from the first stream client, then
                    // goes out as a `session` event, as the daemon's hooks do.
                    for e in g.raw["session_events"].as_array().cloned().unwrap_or_default() {
                        let st = st.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_secs_f64(e["after"].as_f64().unwrap_or(1.0)));
                            let info = e["session"].clone();
                            {
                                let mut g = st.lock().unwrap();
                                if let Some(list) = g.raw["sessions"].as_array_mut() {
                                    let same = |i: &serde_json::Value| i["sandbox"] == info["sandbox"] && i["session_id"] == info["session_id"];
                                    match list.iter_mut().find(|i| same(i)) {
                                        Some(i) => *i = info.clone(),
                                        None => list.push(info.clone()),
                                    }
                                }
                            }
                            publish(&st, "session", &info.to_string());
                        });
                    }
                    if let Some(list) = changes.lock().unwrap().take() {
                        for (t, name, status) in list {
                            let st = st.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(Duration::from_secs_f64(t));
                                set_status(&st, &name, &status);
                            });
                        }
                    }
                }
            }
            let clients = { let g = st.lock().unwrap(); g.streams.len() as u64 + g.others };
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\nevent: clients\ndata: {clients}\n\n")?;
            s.flush()?;
            loop {
                match rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(frame) => s.write_all(frame.as_bytes())?,
                    Err(mpsc::RecvTimeoutError::Timeout) => s.write_all(b":\n\n")?,
                    Err(_) => break,
                }
                s.flush()?;
            }
        }
        // Features' sockets (`/ws/<route>`): the stand-in's own echo route
        // (tests), else the first feature whose `standin_ws` takes it.
        ("GET", p) if p.starts_with("/ws/") => {
            let stream = Prefixed { pre: Cursor::new(head), s: s.try_clone()? };
            let ws = tungstenite::accept_hdr(stream, |_req: &Request, resp: Response| Ok(resp))?;
            eprintln!("standin: socket opened ({target})");
            let mut peer = Peer { ws, raw: s.try_clone()? };
            if p == "/ws/echo" {
                echo(&mut peer);
            } else if !sbxw_desktop::feat::standin_socket(&target, &mut peer) {
                use sbxw_desktop::feat::WsPeer;
                peer.send(&serde_json::json!({ "error": format!("no feature serves {p}") }).to_string());
                peer.close();
            }
        }
        ("GET", "/ws") => {
            let sandbox = query_value(query, "sandbox").map(decode).unwrap_or_default();
            let mode = query_value(query, "mode").map(decode);
            let stream = Prefixed { pre: Cursor::new(head), s: s.try_clone()? };
            let ws = tungstenite::accept_hdr(stream, |_req: &Request, resp: Response| Ok(resp))?;
            eprintln!("standin: terminal opened ({query})");
            // A thread's session opened: features' stand-ins hear it as `OPEN /ws?<query>`
            // (threads records a new thread, as the daemon does on its first open).
            if mode.as_deref() == Some("thread") {
                let mut events = vec![];
                {
                    let mut g = st.lock().unwrap();
                    let mut raw = std::mem::take(&mut g.raw);
                    raw["sandboxes"] = serde_json::to_value(&g.sandboxes).unwrap_or_default();
                    let _ = sbxw_desktop::feat::answer_later(&mut raw, "OPEN", &target, &serde_json::Value::Null, &mut |n, d| events.push((n.to_string(), d)));
                    g.raw = raw;
                }
                for (n, d) in events {
                    publish(st, &n, &d.to_string());
                }
            }
            // The agent of a stopped sandbox: the daemon runs `sbx run`, which starts it.
            if matches!(mode.as_deref(), None | Some("claude")) {
                let stopped = st.lock().unwrap().sandboxes.iter().any(|x| x.name == sandbox && x.status != "running");
                if stopped {
                    let st = st.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        set_status(&st, &sandbox, "running");
                    });
                }
            }
            terminal(ws, &s, query_value(query, "size") == Some("1"))?;
        }
        // Features' routes (src/feat/): their stand-in, else their data keys.
        (m, p) if p.starts_with("/api/") => {
            let body = read_body(&mut s, &head, &text)?;
            feature_route(st, m, &target, &body, &mut s);
        }
        _ => reply(&mut s, "404 Not Found", "\"no such route\"".into()),
    }
    Ok(())
}

/// sbx asks: web-app tried to reach api.github.com (`--approval`).
/// The stand-in's end of a feature socket.
struct Peer {
    ws: tungstenite::WebSocket<Prefixed>,
    raw: TcpStream,
}

impl sbxw_desktop::feat::WsPeer for Peer {
    fn send(&mut self, text: &str) -> bool {
        self.ws.send(Message::Text(text.to_string().into())).is_ok()
    }
    fn send_binary(&mut self, bytes: &[u8]) -> bool {
        self.ws.send(Message::Binary(bytes.to_vec().into())).is_ok()
    }
    fn recv(&mut self, wait: Duration) -> Result<Option<String>, String> {
        let _ = self.raw.set_read_timeout(Some(wait.max(Duration::from_millis(1))));
        match self.ws.read() {
            Ok(Message::Text(t)) => Ok(Some(t.to_string())),
            Ok(Message::Close(_)) => Err("closed by the app".into()),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn close(&mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

/// How many `/ws/echo` sockets were opened (each hello says its number).
static ECHOES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `/ws/echo`, for tests of feature sockets: `{"t":"hello","n":N}` on open,
/// then every text message back; `frames K` sends K frames
/// (`{"t":"frame","i":i}`) at once then `{"t":"done"}`; `bye` closes.
fn echo(peer: &mut Peer) {
    use sbxw_desktop::feat::WsPeer;
    let n = ECHOES.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    peer.send(&serde_json::json!({ "t": "hello", "n": n }).to_string());
    loop {
        match peer.recv(Duration::from_millis(200)) {
            Ok(Some(t)) if t == "bye" => return peer.close(),
            Ok(Some(t)) if t.starts_with("frames ") => {
                let k: u64 = t[7..].trim().parse().unwrap_or(0);
                for i in 0..k {
                    peer.send(&serde_json::json!({ "t": "frame", "i": i }).to_string());
                }
                peer.send("{\"t\":\"done\"}");
            }
            Ok(Some(t)) => {
                if !peer.send(&t) {
                    return;
                }
            }
            Ok(None) => {}
            Err(_) => return,
        }
    }
}

fn new_approval(st: &Shared) {
    {
        let mut g = st.lock().unwrap();
        let a = serde_json::json!({ "id": "network:demo", "kind": "network", "sandbox": "web-app", "title": "api.github.com:443",
            "detail": "HTTPS through sbx's proxy\nNo rule allows this host", "created_at": "2026-11-15T22:00:00Z",
            "options": [{ "id": "allow", "label": "Allow always" }, { "id": "allow-once", "label": "Allow once" }, { "id": "deny", "label": "Deny" }] });
        if !g.raw["approvals"]["approvals"].is_array() {
            g.raw["approvals"] = serde_json::json!({ "ok": true, "approvals": [] });
        }
        g.raw["approvals"]["approvals"].as_array_mut().unwrap().push(a);
        if let Some(sb) = g.sandboxes.iter_mut().find(|s| s.name == "web-app") {
            sb.approvals += 1;
        }
    }
    eprintln!("standin: sbx asks for api.github.com:443 (web-app)");
    publish(st, "session", &serde_json::json!({ "sandbox": "web-app", "state": "working" }).to_string());
}

fn terminal(mut ws: tungstenite::WebSocket<Prefixed>, raw: &TcpStream, sizes: bool) -> anyhow::Result<()> {
    let pty = native_pty_system().openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    // A UTF-8 locale, as a sandbox has: under POSIX bash counts a prompt's
    // multibyte characters as several columns and wraps long lines wrongly.
    if std::env::var("LANG").map_or(true, |l| l.is_empty()) {
        cmd.env("LANG", "C.UTF-8");
    }
    let mut child = pty.slave.spawn_command(cmd)?;
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader()?;
    let mut writer = pty.master.take_writer()?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut b = [0u8; 8192];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    raw.set_read_timeout(Some(Duration::from_millis(5)))?;
    let (other_tx, other_rx) = mpsc::channel::<OtherClient>();
    TERMINALS.lock().unwrap().push(other_tx);
    // The app's wanted size, whether it owns the PTY's, and the size now.
    let (mut want, mut mine, mut now): (Option<(u16, u16)>, bool, (u16, u16)) = (None, false, (100, 30));
    let size_frame = |ws: &mut tungstenite::WebSocket<Prefixed>, (cols, rows): (u16, u16), owner: bool| {
        if sizes {
            let _ = ws.send(Message::Text(serde_json::json!({ "type": "size", "cols": cols, "rows": rows, "owner": owner }).to_string().into()));
        }
    };
    size_frame(&mut ws, now, false);
    'outer: loop {
        if let Ok((cols, rows, typed)) = other_rx.try_recv() {
            mine = false;
            now = (cols, rows);
            pty.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
            size_frame(&mut ws, now, false);
            if !typed.is_empty() {
                writer.write_all(typed.as_bytes())?;
            }
        }
        match ws.read() {
            Ok(Message::Binary(d)) => {
                if !mine && is_typed(&d) {
                    if let Some(w) = want {
                        mine = true;
                        now = w;
                        pty.master.resize(PtySize { rows: w.1, cols: w.0, pixel_width: 0, pixel_height: 0 })?;
                        size_frame(&mut ws, now, true);
                    }
                }
                writer.write_all(&d)?
            }
            Ok(Message::Text(t)) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "resize" {
                    let cols = v["cols"].as_u64().unwrap_or(100) as u16;
                    let rows = v["rows"].as_u64().unwrap_or(30) as u16;
                    want = Some((cols, rows));
                    mine = true;
                    now = (cols, rows);
                    pty.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
                    size_frame(&mut ws, now, true);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        loop {
            match rx.try_recv() {
                Ok(chunk) => {
                    if ws.send(Message::Binary(chunk.into())).is_err() {
                        break 'outer;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    break 'outer;
                }
            }
        }
    }
    let _ = child.kill();
    eprintln!("standin: terminal closed");
    Ok(())
}

/// The request's body after its head (`Content-Length` bytes).
fn read_body(s: &mut TcpStream, head: &[u8], text: &str) -> anyhow::Result<serde_json::Value> {
    let len: usize = text.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse().ok())).unwrap_or(0);
    let start = head.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(head.len());
    let mut body = head[start..].to_vec();
    let mut b = [0u8; 4096];
    while body.len() < len {
        let n = s.read(&mut b)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&b[..n]);
    }
    let ct = text.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
    // A raw body (an upload): what the feature's stand-in sees is its size and type.
    if !body.is_empty() && !ct.starts_with("application/json") {
        return Ok(serde_json::json!({ "$bytes": body.len(), "$type": ct }));
    }
    Ok(serde_json::from_slice(&body).unwrap_or_default())
}

/// One hook of Jarvis's live session ("a-jarvis"): its new state, then one
/// `session` event per session of the sandbox, in session id order (the
/// real daemon's `Agents::apply` returns `infos_for(sandbox)`).
fn publish_session(st: &Shared, state: &str, reply: Option<&str>, last_input: &str, extra: serde_json::Value) {
    let mut info = serde_json::json!({ "sandbox": "sbxw-jarvis", "mode": "claude", "state": state, "agent": "claude", "started_ms": 2,
        "session_id": "a-jarvis", "last_input": last_input, "reply": reply, "origin": "tty", "answerable": true, "ts": 0 });
    if let (Some(o), Some(e)) = (info.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    let all: Vec<serde_json::Value> = {
        let mut g = st.lock().unwrap();
        g.sessions.insert("a-jarvis".into(), info);
        g.sessions.values().cloned().collect()
    };
    for i in all {
        publish(st, "session", &i.to_string());
    }
}

/// What the scripted Jarvis answers, and the proposal it makes.
fn jarvis_answer(text: &str, n: u64) -> (String, Option<serde_json::Value>) {
    let t = text.to_lowercase();
    let id = format!("p-{n}");
    let created = n;
    if t.contains("english") {
        return ("I propose switching the voice to `en_female`. It is a **low-risk** setting: say or type *yes* to apply it.".into(), Some(serde_json::json!({
            "id": id, "summary": "Read answers with the English voice", "risk": "low", "state": "pending", "created": created,
            "changes": [{ "kind": "setting", "title": "voice.tts_voice in ~/.sbxw/sbxw.toml", "risk": "low",
                "diff": "--- ~/.sbxw/sbxw.toml\n+++ ~/.sbxw/sbxw.toml\n@@ -1,2 +1,2 @@\n [voice]\n-tts_voice = \"fr_female\"\n+tts_voice = \"en_female\"\n" }] })));
    }
    if t.contains("review") {
        return ("I wrote a kit with a `/review` command. A kit is **high risk**: read the diff and click Approve.".into(), Some(serde_json::json!({
            "id": id, "summary": "Add a /review command to web-app", "risk": "high", "state": "pending", "created": created,
            "changes": [{ "kind": "kit_write", "title": "kit jarvis-review: 1 file", "risk": "high",
                "diff": "--- /dev/null\n+++ ~/.sbxw/state/kits/jarvis-review/files/home/agent/.claude/commands/review.md\n@@ -0,0 +1,2 @@\n+Review the staged diff.\n+List each bug with its file and line.\n" }] })));
    }
    if t.contains("command") {
        return ("A stdio MCP server runs on this machine: tick that you read its command.".into(), Some(serde_json::json!({
            "id": id, "summary": "Add the Playwright MCP server", "risk": "high", "state": "pending", "created": created, "needs_ack": true,
            "changes": [{ "kind": "mcp_server", "title": "MCP server pw (stdio)", "risk": "high", "host_command": "npx -y @playwright/mcp@latest",
                "diff": "--- sbx mcp ls\n+++ sbx mcp ls\n@@ -0,0 +1,1 @@\n+pw  stdio  npx -y @playwright/mcp@latest\n" }] })));
    }
    if t.contains("screen") {
        return ("I propose a new title for the sandbox list. A screen change is **high risk**: click Approve.".into(), Some(serde_json::json!({
            "id": id, "summary": "Rename the sandbox list's title", "risk": "high", "state": "pending", "created": created,
            "changes": [{ "kind": "desktop_screen", "title": "desktop screen sandboxes (override ~/.sbxw/desktop/screens/sandboxes.luau)", "risk": "high",
                "intents": [{ "name": "open_terminal", "risk": "direct", "new": false }, { "name": "select", "risk": "direct", "new": false }],
                "content": SCREEN_EDIT,
                "diff": "# This screen can ask the app to: open_terminal, select.\n--- builtin/screens/sandboxes.luau\n+++ ~/.sbxw/desktop/screens/sandboxes.luau\n@@ -1,1 +1,1 @@\n-SANDBOXES\n+MY SANDBOXES (by Jarvis)\n" }] })));
    }
    if t.contains("options") {
        return ("I can change three things for you:\n\n1. The voice that reads my answers\n2. The kits of a sandbox\n3. The MCP servers\n\nType the number of your choice.".into(), None);
    }
    (format!("You said: *{}*. Ask me for the **english** voice, a **review** command, an MCP **command** or a **screen** change.", text.trim()), None)
}

const SCREEN_EDIT: &str = include_str!("../../screens/sandboxes.luau");


/// A feature's route (`feat::answer_later`): its stand-in, else its data keys.
fn feature_route(st: &Shared, m: &str, target: &str, body: &serde_json::Value, s: &mut TcpStream) {
    let mut events = vec![];
    let (code, out, later, list) = {
        let mut g = st.lock().unwrap();
        let mut raw = std::mem::take(&mut g.raw);
        raw["sandboxes"] = serde_json::to_value(&g.sandboxes).unwrap_or_default();
        let before = raw["sandboxes"].clone();
        let ((code, out), later) = sbxw_desktop::feat::answer_later(&mut raw, m, target, body, &mut |n, d| events.push((n.to_string(), d)));
        // A route that changed the list (a sandbox removed, created): the list follows.
        let list = (raw["sandboxes"] != before).then(|| serde_json::from_value::<Vec<Sandbox>>(raw["sandboxes"].clone()).unwrap_or_default());
        if let Some(l) = &list {
            g.sandboxes = l.clone();
        }
        g.raw = raw;
        (code, out, later, list)
    };
    eprintln!("standin: {m} {target} -> {code}");
    for (n, d) in events {
        publish(st, &n, &d.to_string());
    }
    for (ms, n, d) in later {
        std::thread::sleep(Duration::from_millis(ms));
        publish(st, &n, &d.to_string());
    }
    if list.is_some() {
        publish(st, "session", "{}");
    }
    let status = match code {
        200 => "200 OK".to_string(),
        c => format!("{c} {}", if c < 300 { "OK" } else if c == 404 { "Not Found" } else if c == 409 { "Conflict" } else if c < 500 { "Bad Request" } else { "Error" }),
    };
    // A raw answer (`{"$raw": ...}`, `{"$raw_hex": ...}`): those bytes, with their type.
    if out.get("$raw").is_some() || out.get("$raw_hex").is_some() {
        let (bytes, ct) = sbxw_desktop::client::raw_of(&out);
        let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
        let _ = s.write_all(&bytes);
        return;
    }
    let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", out.to_string().len(), out);
}

fn jarvis_route(st: &Shared, method: &str, path: &str, body: &serde_json::Value) -> (&'static str, serde_json::Value) {
    use serde_json::json;
    let err = |code: &'static str, m: String| (code, json!({ "ok": false, "error": m }));
    match (method, path) {
        ("GET", "/api/jarvis/status") => ("200 OK", json!({ "state": st.lock().unwrap().jarvis })),
        ("POST", "/api/jarvis/start") => {
            let now = st.lock().unwrap().jarvis.clone();
            if now == "running" || now == "creating" {
                return ("200 OK", json!({ "state": now }));
            }
            st.lock().unwrap().jarvis = "creating".into();
            let st2 = st.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                st2.lock().unwrap().jarvis = "running".into();
                eprintln!("standin: jarvis is running");
                publish_session(&st2, "idle", None, "", serde_json::json!({}));
            });
            ("200 OK", json!({ "state": "creating" }))
        }
        ("GET", "/api/jarvis") => {
            let mut list = st.lock().unwrap().proposals.clone();
            list.reverse();
            ("200 OK", json!({ "ok": true, "proposals": list }))
        }
        // Jarvis's sessions, then the fixture's `sessions` (other sandboxes' agents and threads):
        // the one key every reader of this route uses (threads' and relay's data, Jarvis's seed).
        ("GET", "/api/sessions") => {
            let g = st.lock().unwrap();
            let mut list: Vec<serde_json::Value> = g.sessions.values().cloned().collect();
            list.extend(g.raw["sessions"].as_array().cloned().unwrap_or_default());
            ("200 OK", json!(list))
        }
        ("GET", "/api/voice") if st.lock().unwrap().voice => ("200 OK", json!({ "ok": true, "compiled": true, "ready": true, "speech": { "supported": false }, "push_to_talk_key": "F8" })),
        // No voice of its own: the fixture's `voice` key if it has one (Setup's Voice tab), else 404.
        ("GET", "/api/voice") => {
            let v = st.lock().unwrap().raw["voice"].clone();
            if v.is_object() { ("200 OK", v) } else { err("404 Not Found", "not found".into()) }
        }
        ("POST", "/api/voice/transcribe") => {
            let heard = std::env::var("STANDIN_HEARD").unwrap_or_default();
            eprintln!("standin: transcribed as {heard:?}");
            ("200 OK", json!({ "ok": true, "text": heard, "masked": 0 }))
        }
        ("POST", "/api/voice/anonymise") => {
            let t = body["text"].as_str().unwrap_or("");
            let n = t.matches("Jeanne Martin").count();
            ("200 OK", json!({ "ok": true, "text": t.replace("Jeanne Martin", "Personne A"), "masked": n }))
        }
        ("POST", "/api/chat/push") => {
            let (text, name) = (body["text"].as_str().unwrap_or("").to_string(), body["name"].as_str().unwrap_or(""));
            if name != "sbxw-jarvis" {
                return ("200 OK", json!({ "ok": true, "name": name }));
            }
            if st.lock().unwrap().jarvis != "running" {
                return err("409 Conflict", "Jarvis is not running".into());
            }
            eprintln!("standin: jarvis heard {text:?}");
            let answering = std::mem::take(&mut st.lock().unwrap().asked);
            if answering {
                // The question's answer: typed into the prompt, the turn goes on.
                publish_session(st, "working", None, "change the voice", json!({}));
                let st2 = st.clone();
                let pick = text.trim().to_string();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(700));
                    let voice = match pick.as_str() { "1" => "fr_female", "2" => "en_female", _ => "de_male" };
                    let reply = format!("You chose **{voice}**. Say *use the english voice* and I will propose the change; nothing is applied until you approve it.");
                    publish_session(&st2, "idle", Some(&reply), "change the voice", json!({}));
                });
                return ("200 OK", json!({ "ok": true, "name": "sbxw-jarvis" }));
            }
            publish_session(st, "working", None, &text, json!({}));
            let lower = text.to_lowercase();
            if lower.contains("voix") || lower.contains("change the voice") {
                // `AskUserQuestion`: the session waits on a question with options.
                let st2 = st.clone();
                let text2 = text.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(700));
                    st2.lock().unwrap().asked = true;
                    let q = json!({ "text": "Which voice should read my answers?", "options": ["fr_female", "en_female", "de_male"] });
                    publish_session(&st2, "attention", None, &text2, json!({ "steps": [q], "question": q, "activity": "Which voice should read my answers?" }));
                });
                return ("200 OK", json!({ "ok": true, "name": "sbxw-jarvis" }));
            }
            let st2 = st.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(700));
                let n = {
                    let mut g = st2.lock().unwrap();
                    g.next += 1;
                    g.next
                };
                let (reply, proposal) = jarvis_answer(&text, n);
                if let Some(p) = proposal {
                    st2.lock().unwrap().proposals.push(p.clone());
                    publish(&st2, "jarvis", &p.to_string());
                    eprintln!("standin: jarvis proposes {}", p["id"]);
                }
                publish_session(&st2, "idle", Some(&reply), &text, json!({}));
            });
            ("200 OK", json!({ "ok": true, "name": "sbxw-jarvis" }))
        }
        _ => {
            let Some(rest) = path.strip_prefix("/api/jarvis/proposals/") else { return err("404 Not Found", "no such route".into()) };
            let (id, action) = rest.rsplit_once('/').unwrap_or((rest, ""));
            let id = decode(id);
            let mut g = st.lock().unwrap();
            let dir = g.screens_dir.clone();
            let Some(p) = g.proposals.iter_mut().find(|p| p["id"] == id.as_str()) else { return err("404 Not Found", format!("no proposal {id}")) };
            match action {
                "approve" => {
                    let via = body["via"].as_str().unwrap_or("click");
                    if let Err(e) = sbxw_desktop::client::fixture_rule(p, via, body["ack_host_command"] == true) {
                        return err("409 Conflict", sbxw_desktop::client::reason(&e));
                    }
                    p["state"] = "applied".into();
                    p["via"] = via.into();
                    if let (Some(dir), Some(content)) = (dir, p["changes"][0]["content"].as_str()) {
                        let _ = std::fs::create_dir_all(&dir);
                        let edited = content.replace("\"SANDBOXES\"", "\"MY SANDBOXES (by Jarvis)\"");
                        let _ = std::fs::write(dir.join("sandboxes.luau"), edited);
                        eprintln!("standin: wrote {}", dir.join("sandboxes.luau").display());
                    }
                }
                "refuse" => {
                    if p["state"] != "pending" {
                        return err("409 Conflict", "already answered".into());
                    }
                    p["state"] = "refused".into();
                    if let Some(r) = body["reason"].as_str() {
                        p["reason"] = r.into();
                    }
                }
                _ => return err("404 Not Found", "no such route".into()),
            }
            let p = p.clone();
            drop(g);
            eprintln!("standin: proposal {id} is {}", p["state"]);
            publish(st, "jarvis", &p.to_string());
            ("200 OK", json!({ "ok": true, "proposal": p }))
        }
    }
}
