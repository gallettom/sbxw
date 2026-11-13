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
            _ => anyhow::bail!("unknown argument {k}"),
        }
    }
    let fx: serde_json::Value = serde_json::from_str(&fixture)?;
    let sandboxes: Vec<Sandbox> = serde_json::from_value(fx["daemon"]["sandboxes"].clone()).unwrap_or_default();
    let st: Shared = Arc::new(Mutex::new(State { sandboxes, ..Default::default() }));
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
