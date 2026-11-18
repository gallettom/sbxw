//! The terminal socket thread, timed against an in-process echo server
//! (blocking reads on its side, so the numbers are the client's):
//! key-to-echo latency, and wake-ups of an idle terminal.
//!
//! `cargo test --release --test pump -- --nocapture`

use sbxw_desktop::client::{proof, Daemon, HttpDaemon, TermEvent, TermInput, TerminalRequest};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::Message;

fn serve(s: TcpStream, token: &str) {
    let mut peek = [0u8; 1024];
    let n = s.peek(&mut peek).unwrap_or(0);
    let head = String::from_utf8_lossy(&peek[..n]).to_string();
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    if target.starts_with("/api/health") {
        let mut s = s;
        let mut buf = [0u8; 2048];
        let _ = s.read(&mut buf);
        let nonce = target.split("nonce=").nth(1).unwrap_or("");
        let body = serde_json::json!({ "ok": true, "proof": proof(token, nonce) }).to_string();
        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        return;
    }
    s.set_nodelay(true).unwrap();
    let mut ws = tungstenite::accept(s).unwrap();
    while let Ok(m) = ws.read() {
        if let Message::Binary(b) = m {
            if ws.send(Message::Binary(b)).is_err() {
                break;
            }
        }
    }
}

#[test]
fn key_to_echo_and_idle_wakeups() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || serve(s, "tok"));
        }
    });
    let d = HttpDaemon::connect(&addr, "tok").unwrap();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let conn = d.open_terminal(&TerminalRequest { sandbox: "x".into(), ..Default::default() }, Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    let mut ms = vec![];
    for i in 0..200u32 {
        let t = Instant::now();
        conn.input.send(TermInput::Bytes(vec![b'a' + (i % 26) as u8])).unwrap();
        match conn.events.recv_timeout(Duration::from_secs(2)) {
            Ok(TermEvent::Output(_)) => ms.push(t.elapsed().as_secs_f64() * 1000.0),
            _ => panic!("no echo for key {i}"),
        }
    }
    ms.sort_by(f64::total_cmp);
    let q = |p: f64| ms[((ms.len() - 1) as f64 * p) as usize];
    eprintln!("key to echo over {} keys: p50 {:.3} ms, p90 {:.3}, p99 {:.3}, max {:.3}", ms.len(), q(0.5), q(0.9), q(0.99), ms[ms.len() - 1]);
    // The last echo's wake-up may land just after its event was taken (the
    // socket thread sends, then wakes): let it, so a loaded machine (several
    // builds at once) does not count it as idle traffic.
    std::thread::sleep(Duration::from_millis(100));
    let before = wakes.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_secs(1));
    let idle = wakes.load(Ordering::SeqCst) - before;
    eprintln!("idle: {idle} wake-ups in 1 s");
    assert_eq!(idle, 0, "an idle terminal must not wake the UI");
    // The latency budget is a release build's (`cargo test --release --test
    // pump -- --nocapture`); a debug build on a loaded machine measures and
    // prints it, and would fail it for reasons that are not the app's.
    if !cfg!(debug_assertions) {
        assert!(q(0.5) < 2.0, "p50 {:.3} ms", q(0.5));
    }
    // A resize still goes out, and dropping the input closes the socket.
    conn.input.send(TermInput::Resize { cols: 80, rows: 24 }).unwrap();
    drop(conn.input);
    let closed = (0..100).any(|_| matches!(conn.events.recv_timeout(Duration::from_millis(50)), Ok(TermEvent::Closed(_))));
    assert!(closed, "dropping the input side closes the terminal");
}
