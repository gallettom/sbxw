//! A stand-in for the sbxw daemon, for when none is reachable: the same
//! routes and terminal protocol, a local shell instead of a sandbox.
//!
//! It speaks what the real daemon speaks (see `src/client.rs` for the file
//! and line references): `GET /api/health?nonce=` (proof), `GET
//! /api/sandboxes` (fixture list) and `GET /ws` (binary PTY bytes both ways,
//! JSON `resize` text frames in), all behind `Authorization: Bearer`.
//! What it does not do: keep the PTY across reconnects, replay the last
//! 256 KB, Host/Origin checks, threads, monitor mode.
//!
//! Usage: `standin <addr> <token>`, e.g. `standin 127.0.0.1:7399 test-token`.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use sbxw_desktop::client::{fixture_sandboxes, proof};
use std::io::{Cursor, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;
use tungstenite::handshake::server::{Request, Response};
use tungstenite::Message;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:7399".into());
    let token = args.next().unwrap_or_else(|| "test-token".into());
    let listener = TcpListener::bind(&addr)?;
    eprintln!("standin: listening on {addr}");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let token = token.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, &token) {
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

fn serve(mut s: TcpStream, token: &str) -> anyhow::Result<()> {
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
    let target = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let bearer = text
        .lines()
        .find_map(|l| l.strip_prefix("Authorization: Bearer ").or_else(|| l.strip_prefix("authorization: Bearer ")))
        .map(str::trim);
    let authed = bearer == Some(token);
    let reply = |s: &mut TcpStream, code: &str, body: String| {
        let _ = write!(s, "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    };
    match path {
        "/api/health" => {
            let nonce = query.split('&').find_map(|kv| kv.strip_prefix("nonce=")).unwrap_or("");
            reply(&mut s, "200 OK", serde_json::json!({ "ok": true, "proof": proof(token, nonce) }).to_string());
        }
        _ if !authed => reply(&mut s, "401 Unauthorized", "\"not signed in\"".into()),
        "/api/sandboxes" => reply(&mut s, "200 OK", serde_json::to_string(&fixture_sandboxes())?),
        "/ws" => {
            let stream = Prefixed { pre: Cursor::new(head), s: s.try_clone()? };
            let ws = tungstenite::accept_hdr(stream, |_req: &Request, resp: Response| Ok(resp))?;
            eprintln!("standin: terminal opened ({query})");
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
