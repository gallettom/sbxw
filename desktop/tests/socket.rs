//! Feature sockets (`src/socket.rs`) against the stand-in daemon (a real
//! process, real sockets, its `/ws/echo` route): messages both ways off the
//! UI thread, the newest frame kept when the handler is behind, no reconnect
//! unless asked, and a reconnect with the NEW token after a daemon restart.

use sbxw_desktop::client::{Link, LinkEvent, Target, TokenSource};
use sbxw_desktop::socket::{self, Current, Opts, SocketEvent};
use std::process::{Child, Command};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn standin(addr: &str, token: &str, extra: &[&str]) -> Child {
    let c = Command::new(env!("CARGO_BIN_EXE_standin")).arg(addr).arg(token).args(extra).stderr(std::process::Stdio::null()).spawn().unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect(addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    c
}

/// A link kept up on its own thread, as the app's `current`.
fn link_current(addr: &str, token: TokenSource) -> Current {
    let current: Current = Arc::new(Mutex::new(None));
    let link = Link::start(Target::Http { addr: addr.to_string(), token }, Arc::new(|| {}));
    let c = current.clone();
    std::thread::spawn(move || loop {
        match link.recv_timeout(Duration::from_millis(50)) {
            Some(LinkEvent::Up(d)) => *c.lock().unwrap() = Some(d),
            Some(LinkEvent::Down(_)) => *c.lock().unwrap() = None,
            _ => {}
        }
    });
    let end = Instant::now() + Duration::from_secs(5);
    while current.lock().unwrap().is_none() {
        assert!(Instant::now() < end, "no connection");
        std::thread::sleep(Duration::from_millis(20));
    }
    current
}

fn next(rx: &mpsc::Receiver<SocketEvent>, secs: u64, mut want: impl FnMut(&SocketEvent) -> bool) -> Option<SocketEvent> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Ok(e) = rx.recv_timeout(Duration::from_millis(50)) {
            if want(&e) {
                return Some(e);
            }
        }
    }
    None
}

fn text(e: &SocketEvent) -> Option<serde_json::Value> {
    match e {
        SocketEvent::Text(t) => serde_json::from_str(t).ok(),
        _ => None,
    }
}

#[test]
fn messages_go_both_ways_and_a_slow_handler_gets_the_newest_frame() {
    let addr = format!("127.0.0.1:{}", free_port());
    let mut d = standin(&addr, "tok", &[]);
    let current = link_current(&addr, TokenSource::Fixed("tok".into()));
    let (tx, rx) = mpsc::channel();
    let opts = Opts { droppable: Some(|t: &str| t.contains("\"frame\"")), one_line: true, ..Opts::default() };
    let slow = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = slow.clone();
    socket::open(current, "t-echo", "/ws/echo", opts, Box::new(move |e| {
        if s2.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = tx.send(e);
    }));
    assert_eq!(next(&rx, 5, |e| matches!(e, SocketEvent::Open)), Some(SocketEvent::Open));
    assert!(next(&rx, 5, |e| text(e).is_some_and(|v| v["t"] == "hello")).is_some());
    socket::send("t-echo", r#"{"t":"ping","x":1}"#).unwrap();
    assert!(next(&rx, 5, |e| text(e).is_some_and(|v| v["t"] == "ping")).is_some(), "the echo came back");
    // A slow handler: 200 frames at once, then `done`.
    slow.store(true, std::sync::atomic::Ordering::SeqCst);
    socket::send("t-echo", "frames 200").unwrap();
    let mut frames = vec![];
    let done = next(&rx, 10, |e| {
        if let Some(v) = text(e) {
            if v["t"] == "frame" {
                frames.push(v["i"].as_u64().unwrap());
            }
            return v["t"] == "done";
        }
        false
    });
    assert!(done.is_some(), "the non-frame message after the frames is never dropped");
    eprintln!("the handler saw {} of 200 frames", frames.len());
    assert!(frames.len() < 200, "late frames were dropped");
    assert_eq!(frames.last(), Some(&199), "the newest frame is kept");
    assert!(frames.windows(2).all(|w| w[0] < w[1]), "in order");
    // The app closes it: `Closed("closed")`, and sends are refused after.
    socket::close("t-echo");
    assert_eq!(next(&rx, 5, |e| matches!(e, SocketEvent::Closed(_))), Some(SocketEvent::Closed("closed".into())));
    assert!(socket::send("t-echo", "x").is_err());
    d.kill().ok();
}

#[test]
fn without_reconnect_a_lost_socket_is_closed_with_its_reason() {
    let addr = format!("127.0.0.1:{}", free_port());
    let mut d = standin(&addr, "tok", &[]);
    let current = link_current(&addr, TokenSource::Fixed("tok".into()));
    let (tx, rx) = mpsc::channel();
    socket::open(current, "t-once", "/ws/echo", Opts::default(), Box::new(move |e| {
        let _ = tx.send(e);
    }));
    assert!(next(&rx, 5, |e| matches!(e, SocketEvent::Open)).is_some());
    socket::send("t-once", "bye").unwrap();
    let closed = next(&rx, 5, |e| matches!(e, SocketEvent::Closed(_) | SocketEvent::Reconnecting(_)));
    assert!(matches!(&closed, Some(SocketEvent::Closed(w)) if w != "closed"), "{closed:?}");
    assert!(!socket::is_open("t-once"));
    // A route no feature serves: closed with the stand-in's word.
    let (tx, rx) = mpsc::channel();
    let c2 = Arc::new(Mutex::new(None));
    socket::open(c2, "t-none", "/ws/echo", Opts::default(), Box::new(move |e| {
        let _ = tx.send(e);
    }));
    assert!(matches!(next(&rx, 5, |_| true), Some(SocketEvent::Closed(w)) if w.contains("not connected")));
    d.kill().ok();
}

#[test]
fn a_restarted_daemon_with_a_new_token_gets_the_socket_back() {
    let addr = format!("127.0.0.1:{}", free_port());
    let dir = std::env::temp_dir().join(format!("sbxw-socket-{}", std::process::id()));
    let file = dir.join("ui-token");
    let mut d = standin(&addr, "first", &["--token-file", file.to_str().unwrap()]);
    let current = link_current(&addr, TokenSource::File(file.clone()));
    let (tx, rx) = mpsc::channel();
    socket::open(current, "t-again", "/ws/echo", Opts { reconnect: true, ..Opts::default() }, Box::new(move |e| {
        let _ = tx.send(e);
    }));
    assert!(next(&rx, 5, |e| text(e).is_some_and(|v| v["t"] == "hello")).is_some());
    d.kill().ok();
    d.wait().ok();
    assert!(next(&rx, 5, |e| matches!(e, SocketEvent::Reconnecting(_))).is_some(), "it says it is reconnecting");
    // Messages sent meanwhile wait for the new connection.
    socket::send("t-again", "while away").unwrap();
    let mut d2 = standin(&addr, "second", &["--token-file", file.to_str().unwrap()]);
    let t = Instant::now();
    assert!(next(&rx, 15, |e| matches!(e, SocketEvent::Open)).is_some(), "never opened again");
    eprintln!("socket open again {:?} after the restart", t.elapsed());
    assert!(next(&rx, 5, |e| matches!(e, SocketEvent::Text(t) if t == "while away")).is_some(), "the queued message went out on the new socket");
    socket::close("t-again");
    assert!(next(&rx, 5, |e| matches!(e, SocketEvent::Closed(_))).is_some());
    d2.kill().ok();
    std::fs::remove_dir_all(&dir).ok();
}
