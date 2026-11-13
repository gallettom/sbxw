//! The daemon link against the stand-in daemon (a real process, real
//! sockets): live list updates from `/api/stream`, and reconnecting when the
//! daemon restarts with a new token.

use sbxw_desktop::client::{Link, LinkEvent, Target, TokenSource};
use std::process::{Child, Command};
use std::sync::Arc;
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

/// Waits for an event `want` accepts; returns what it said about it.
fn wait_for(link: &Link, secs: u64, mut want: impl FnMut(&LinkEvent) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Some(ev) = link.recv_timeout(Duration::from_millis(50)) {
            if want(&ev) {
                return true;
            }
        }
    }
    false
}

fn status_of(ev: &LinkEvent, name: &str) -> Option<String> {
    match ev {
        LinkEvent::Sandboxes(l) => l.iter().find(|s| s.name == name).map(|s| s.status.clone()),
        _ => None,
    }
}

#[test]
fn a_change_behind_the_app_reaches_the_list() {
    let addr = format!("127.0.0.1:{}", free_port());
    let mut d = standin(&addr, "tok", &["--change", "1.0:web-api=running"]);
    let link = Link::start(Target::Http { addr: addr.clone(), token: TokenSource::Fixed("tok".into()) }, Arc::new(|| {}));
    assert!(wait_for(&link, 5, |e| status_of(e, "web-api").as_deref() == Some("stopped")), "first list");
    let t = Instant::now();
    assert!(wait_for(&link, 5, |e| status_of(e, "web-api").as_deref() == Some("running")), "the change never reached the list");
    eprintln!("change seen {:?} after the first list (the stand-in makes it 1 s after the stream opens)", t.elapsed());
    d.kill().ok();
}

#[test]
fn a_restarted_daemon_with_a_new_token_is_found_again() {
    let addr = format!("127.0.0.1:{}", free_port());
    let dir = std::env::temp_dir().join(format!("sbxw-link-{}", std::process::id()));
    let file = dir.join("ui-token");
    let mut d = standin(&addr, "first", &["--token-file", file.to_str().unwrap()]);
    let link = Link::start(Target::Http { addr: addr.clone(), token: TokenSource::File(file.clone()) }, Arc::new(|| {}));
    assert!(wait_for(&link, 5, |e| matches!(e, LinkEvent::Up(_))), "first connection");
    d.kill().ok();
    d.wait().ok();
    let mut down = String::new();
    assert!(wait_for(&link, 5, |e| match e {
        LinkEvent::Down(w) => {
            down = w.clone();
            true
        }
        _ => false,
    }));
    eprintln!("down: {down}");
    assert!(down.contains(&addr), "{down}");
    // A new daemon, a new token in the same file.
    let mut d2 = standin(&addr, "second", &["--token-file", file.to_str().unwrap()]);
    let t = Instant::now();
    assert!(wait_for(&link, 8, |e| matches!(e, LinkEvent::Up(_))), "never reconnected");
    eprintln!("reconnected {:?} after the restart", t.elapsed());
    assert!(wait_for(&link, 3, |e| status_of(e, "web-app").is_some()), "list after reconnect");
    d2.kill().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn no_daemon_says_so_in_one_line() {
    let addr = format!("127.0.0.1:{}", free_port());
    let link = Link::start(Target::Http { addr: addr.clone(), token: TokenSource::Fixed("x".into()) }, Arc::new(|| {}));
    let mut why = String::new();
    assert!(wait_for(&link, 5, |e| match e {
        LinkEvent::Down(w) => {
            why = w.clone();
            true
        }
        _ => false,
    }));
    assert_eq!(why, format!("No sbxw daemon at {addr}. Start it with `sbxw up`; retrying."));
}
