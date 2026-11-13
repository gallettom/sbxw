//! A stand-in for the sbxw daemon, for when none is reachable: the same
//! routes and protocols as the routes the app covers, a local shell instead
//! of a sandbox.
//!
//! It speaks what the real daemon speaks (see `src/client.rs` and
//! `.sbxw-artifacts/daemon-api.md`): `GET /api/health?nonce=` (proof), `GET
//! /api/sandboxes`, `POST /api/sandboxes/:name/stop`, `GET /api/stream`
//! (server-sent events) and `GET /ws` (binary PTY bytes both ways, JSON
//! `resize` text frames in), all behind `Authorization: Bearer`.
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
//! /api/voice` answers 404 (a daemon without voice).
//!
//! What it does not do: keep the PTY across reconnects, replay the last
//! 256 KB, Host/Origin checks, threads, monitor mode.
//!
//! Usage: `standin <addr> <token> [--fixture file.json] [--token-file path]
//! [--change SECS:NAME=STATUS]...`. `--change` sets a sandbox's status that
//! many seconds after the first `/api/stream` client connects (a change made
//! behind the app's back, for the live-update demo). `--token-file` writes
//! the token there, as the daemon writes `~/.sbxw/run/ui-token`.

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
    session: serde_json::Value,
    screens_dir: Option<std::path::PathBuf>,
    next: u64,
    voice: bool,
}

type Shared = Arc<Mutex<State>>;

fn publish(st: &Shared, name: &str, data: &str) {
    let frame = format!("event: {name}\ndata: {data}\n\n");
    st.lock().unwrap().streams.retain(|t| t.send(frame.clone()).is_ok());
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
            _ => anyhow::bail!("unknown argument {k}"),
        }
    }
    let fx: serde_json::Value = serde_json::from_str(&fixture)?;
    let sandboxes: Vec<Sandbox> = serde_json::from_value(fx["daemon"]["sandboxes"].clone()).unwrap_or_default();
    let st: Shared = Arc::new(Mutex::new(State { sandboxes, jarvis, screens_dir, voice, ..Default::default() }));
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
    if authed && (path.starts_with("/api/jarvis") || matches!(path, "/api/sessions" | "/api/chat/push" | "/api/voice" | "/api/voice/anonymise" | "/api/voice/transcribe")) {
        let body = read_body(&mut s, &head, &text)?;
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
        ("GET", "/api/stream") => {
            let (tx, rx) = mpsc::channel::<String>();
            {
                let mut g = st.lock().unwrap();
                g.streams.push(tx);
                if !g.streamed {
                    g.streamed = true;
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
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\nevent: clients\ndata: 1\n\n")?;
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
        ("GET", "/ws") => {
            let sandbox = query_value(query, "sandbox").map(decode).unwrap_or_default();
            let mode = query_value(query, "mode").map(decode);
            let stream = Prefixed { pre: Cursor::new(head), s: s.try_clone()? };
            let ws = tungstenite::accept_hdr(stream, |_req: &Request, resp: Response| Ok(resp))?;
            eprintln!("standin: terminal opened ({query})");
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
            terminal(ws, &s)?;
        }
        _ => reply(&mut s, "404 Not Found", "\"no such route\"".into()),
    }
    Ok(())
}

fn terminal(mut ws: tungstenite::WebSocket<Prefixed>, raw: &TcpStream) -> anyhow::Result<()> {
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
    'outer: loop {
        match ws.read() {
            Ok(Message::Binary(d)) => writer.write_all(&d)?,
            Ok(Message::Text(t)) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "resize" {
                    let cols = v["cols"].as_u64().unwrap_or(100) as u16;
                    let rows = v["rows"].as_u64().unwrap_or(30) as u16;
                    pty.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
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
    Ok(serde_json::from_slice(&body).unwrap_or_default())
}

fn publish_session(st: &Shared, state: &str, reply: Option<&str>, last_input: &str) {
    let info = serde_json::json!({ "sandbox": "sbxw-jarvis", "mode": "claude", "state": state, "agent": "claude", "started_ms": 0,
        "last_input": last_input, "reply": reply, "origin": "tty", "answerable": false, "ts": 0 });
    st.lock().unwrap().session = info.clone();
    publish(st, "session", &info.to_string());
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
    (format!("You said: *{}*. Ask me for the **english** voice, a **review** command, an MCP **command** or a **screen** change.", text.trim()), None)
}

const SCREEN_EDIT: &str = include_str!("../../screens/sandboxes.luau");

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
                publish_session(&st2, "idle", None, "");
            });
            ("200 OK", json!({ "state": "creating" }))
        }
        ("GET", "/api/jarvis") => {
            let mut list = st.lock().unwrap().proposals.clone();
            list.reverse();
            ("200 OK", json!({ "ok": true, "proposals": list }))
        }
        ("GET", "/api/sessions") => {
            let s = st.lock().unwrap().session.clone();
            ("200 OK", if s.is_null() { json!([]) } else { json!([s]) })
        }
        ("GET", "/api/voice") if st.lock().unwrap().voice => ("200 OK", json!({ "ok": true, "compiled": true, "ready": true, "speech": { "supported": false }, "push_to_talk_key": "F8" })),
        ("GET", "/api/voice") => err("404 Not Found", "not found".into()),
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
            publish_session(st, "working", None, &text);
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
                publish_session(&st2, "idle", Some(&reply), &text);
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
