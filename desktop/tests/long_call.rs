//! A long feature call (a bring-up takes minutes) never holds the link: the
//! stream's events keep arriving while the POST is pending, and the UI's
//! side only ever polls the link without blocking (`App::poll` drains with
//! no wait in a window; only headless runs wait for pending jobs, so the
//! virtual clock does not outrun the daemon). Against the stand-in daemon,
//! a real process: its bring-up publishes `provision` steps 350 ms apart,
//! then answers.

use sbxw_desktop::client::{Link, LinkEvent, Target, TokenSource};
use sbxw_desktop::feat::Call;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn stream_events_arrive_while_a_long_call_is_pending() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/create/standin.json");
    let mut child = Command::new(env!("CARGO_BIN_EXE_standin")).args([&addr, "long-token", "--fixture", fixture]).stderr(std::process::Stdio::null()).spawn().unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let link = Link::start(Target::Http { addr: addr.clone(), token: TokenSource::Fixed("long-token".into()) }, Arc::new(|| {}));
    let end = Instant::now() + Duration::from_secs(5);
    let daemon = loop {
        assert!(Instant::now() < end, "no connection");
        if let Some(LinkEvent::Up(d)) = link.recv_timeout(Duration::from_millis(50)) {
            break d;
        }
    };
    // Let the stream connect before the call.
    std::thread::sleep(Duration::from_millis(300));
    while link.try_recv().is_some() {}
    let t0 = Instant::now();
    link.call(daemon, 1, "create", Call::post("/api/sandboxes/chat", serde_json::json!({ "name": "chat-long" }), "create chat-long"));
    let mut steps = vec![];
    let mut answered = None;
    let mut longest_poll = Duration::ZERO;
    while t0.elapsed() < Duration::from_secs(15) && answered.is_none() {
        // What App::poll does in a window: take what is there, never wait.
        let p = Instant::now();
        let ev = link.try_recv();
        longest_poll = longest_poll.max(p.elapsed());
        match ev {
            Some(LinkEvent::Bus(b)) if b.name == "provision" && b.data.contains("chat-long") => steps.push(t0.elapsed()),
            Some(LinkEvent::Answer { result, .. }) => answered = Some((t0.elapsed(), result)),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let _ = child.kill();
    let (at, result) = answered.expect("the call answered");
    assert!(matches!(result, Ok((200, _))), "{result:?}");
    eprintln!("answered at {at:?}; provision events at {steps:?}; longest poll {longest_poll:?}");
    assert!(steps.len() >= 4, "steps seen while pending: {steps:?}");
    assert!(steps.iter().filter(|t| **t + Duration::from_millis(300) < at).count() >= 3, "steps came before the answer, spread over the call: {steps:?} answer {at:?}");
    // A poll that waited would wait for the stand-in's next step (350 ms
    // apart); 200 ms tells that from a loaded machine's scheduling.
    assert!(longest_poll < Duration::from_millis(200), "a poll never waits: {longest_poll:?}");
}
