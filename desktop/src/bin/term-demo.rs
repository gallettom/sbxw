//! The terminal alone, for testing it: one pane against a daemon (usually
//! the stand-in, `src/bin/standin.rs`), with the input method registered and
//! the terminal's events shown in a banner.
//!
//! term-demo --daemon 127.0.0.1:7399 --token test-token [--sandbox web-app] [--mode bash]
//!           [--dump-text out.txt] [--simulate-drop] [--no-blink] [--theme '#fdf6e3' --text auto]
//! term-demo --bench-echo 300      key-to-echo latency of client.rs's socket thread
//!
//! Rune's flags work too (`--snap`, `--script`, `--at`, `--size`, `--scale`).
//! Script marks the demo reads (the virtual clock cannot wait for these):
//! * `drop`: the connection drops (with `--simulate-drop`: a proxy between
//!   the socket and the terminal that keeps the daemon's side open and replays
//!   what it saw, as the daemon's 256 KB replay does);
//! * `attention`: the daemon's `{"type":"attention"}` frame arrives;
//! * `theme:<background>:<text>`: new colours, as sbxw.toml spells them;
//! * `scroll:<lines>`: scrolls the history (as a wheel would).
//! Script `{"focus": false}` events blur the window (focus reports, hollow cursor).

use rune::{col, el, text, AppSpec, Cx, El, Ime, KeyEvent, TextStyle, Tx, Waker};
use sbxw_desktop::client::{Daemon, HttpDaemon, TermEvent, TermInput, TerminalConn, TerminalRequest};
use sbxw_desktop::terminal::{self, rgb, Reconnect, TerminalEvent, TerminalState, Theme};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Args {
    daemon: String,
    token: String,
    sandbox: String,
    mode: String,
    dump_text: Option<PathBuf>,
    simulate_drop: bool,
    /// A steady cursor, so a shot does not catch it blinked off.
    no_blink: bool,
    theme: Option<(String, String)>,
    bench_echo: Option<usize>,
}

fn args() -> Args {
    let mut a = Args { daemon: "127.0.0.1:7399".into(), sandbox: "web-app".into(), mode: "bash".into(), ..Args::default() };
    let mut text = "auto".to_string();
    let mut it = rune::extra_args().iter();
    while let Some(k) = it.next() {
        if k == "--simulate-drop" {
            a.simulate_drop = true;
            continue;
        }
        if k == "--no-blink" {
            a.no_blink = true;
            continue;
        }
        let v = it.next().cloned().unwrap_or_default();
        match k.as_str() {
            "--daemon" => a.daemon = v,
            "--token" => a.token = v,
            "--token-file" => a.token = std::fs::read_to_string(&v).map(|s| s.trim().to_string()).unwrap_or_default(),
            "--sandbox" => a.sandbox = v,
            "--mode" => a.mode = v,
            "--dump-text" => a.dump_text = Some(v.into()),
            "--theme" => a.theme = Some((v, text.clone())),
            "--text" => {
                text = v;
                if let Some(t) = &mut a.theme {
                    t.1 = text.clone();
                }
            }
            "--bench-echo" => a.bench_echo = v.parse().ok(),
            _ => eprintln!("unknown argument {k}"),
        }
    }
    a
}

/// Between the daemon's socket and the terminal: records the output (as the
/// daemon's session ring does), and on `drop` closes the terminal's side
/// only, so a reconnect gets the replay of a session that kept running.
struct Proxy {
    ring: Vec<u8>,
    ui: Option<mpsc::Sender<TermEvent>>,
    to_daemon: mpsc::Sender<TermInput>,
    wake: Waker,
}

const RING: usize = 256 * 1024;

impl Proxy {
    fn start(real: TerminalConn, wake: Waker) -> (Arc<Mutex<Proxy>>, TerminalConn) {
        let p = Arc::new(Mutex::new(Proxy { ring: Vec::new(), ui: None, to_daemon: real.input, wake }));
        let conn = Proxy::attach(&p, false);
        let p2 = p.clone();
        std::thread::spawn(move || {
            for ev in real.events {
                let mut p = p2.lock().unwrap();
                if let TermEvent::Output(b) = &ev {
                    p.ring.extend_from_slice(b);
                    if p.ring.len() > RING {
                        let cut = p.ring.len() - RING;
                        p.ring.drain(..cut);
                    }
                }
                if let Some(ui) = &p.ui {
                    let _ = ui.send(ev);
                    (p.wake)();
                }
            }
        });
        (p, conn)
    }

    /// A new terminal-side connection; `replay` sends the ring first.
    fn attach(p: &Arc<Mutex<Proxy>>, replay: bool) -> TerminalConn {
        let (ev_tx, ev_rx) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel::<TermInput>();
        let mut g = p.lock().unwrap();
        if replay && !g.ring.is_empty() {
            let _ = ev_tx.send(TermEvent::Output(g.ring.clone()));
        }
        g.ui = Some(ev_tx);
        let to = g.to_daemon.clone();
        std::thread::spawn(move || {
            for i in in_rx {
                let _ = to.send(i);
            }
        });
        TerminalConn { events: ev_rx, input: in_tx }
    }

    fn drop_ui(p: &Arc<Mutex<Proxy>>) {
        let mut g = p.lock().unwrap();
        if let Some(ui) = g.ui.take() {
            let _ = ui.send(TermEvent::Closed("simulated drop".into()));
        }
        (g.wake)();
    }

    fn attention(p: &Arc<Mutex<Proxy>>) {
        let g = p.lock().unwrap();
        if let Some(ui) = &g.ui {
            let _ = ui.send(TermEvent::Attention);
        }
        (g.wake)();
    }
}

struct Demo {
    args: Args,
    daemon: Result<Arc<dyn Daemon>, String>,
    term: Option<TerminalState>,
    proxy: Option<Arc<Mutex<Proxy>>>,
    error: String,
    /// What the terminal told the shell, newest last.
    banner: Vec<String>,
    attention: bool,
    marks: Vec<(f64, String)>,
    last_dump: String,
    /// Frames drawn and the CPU time of poll (for the measurements).
    polls: usize,
    poll_time: Duration,
}

fn lens(s: &mut Demo) -> Option<&mut TerminalState> {
    s.term.as_mut()
}

fn init() -> Demo {
    let args = args();
    let daemon = HttpDaemon::connect(&args.daemon, &args.token).map(|d| Arc::new(d) as Arc<dyn Daemon>).map_err(|e| format!("{e:#}"));
    let marks = rune::tooling::headless_run().map(|r| r.marks().map(|(n, t)| (t, n.to_string())).collect()).unwrap_or_default();
    Demo {
        args,
        daemon,
        term: None,
        proxy: None,
        error: String::new(),
        banner: Vec::new(),
        attention: false,
        marks,
        last_dump: String::new(),
        polls: 0,
        poll_time: Duration::ZERO,
    }
}

fn setup(s: &mut Demo, waker: Waker) {
    let d = match &s.daemon {
        Ok(d) => d.clone(),
        Err(e) => {
            s.error = e.clone();
            return;
        }
    };
    let req = TerminalRequest { sandbox: s.args.sandbox.clone(), mode: Some(s.args.mode.clone()).filter(|m| m != "agent"), thread: None };
    let conn = match d.open_terminal(&req, waker.clone()) {
        Ok(c) => c,
        Err(e) => {
            s.error = format!("no terminal: {e:#}");
            return;
        }
    };
    let mut t = if s.args.simulate_drop {
        let (proxy, conn) = Proxy::start(conn, waker);
        let p = proxy.clone();
        s.proxy = Some(proxy);
        let reopen: Reconnect = Arc::new(move || Ok(Proxy::attach(&p, true)));
        TerminalState::with_reconnect(conn, reopen)
    } else {
        let reopen: Reconnect = Arc::new(move || d.open_terminal(&req, waker.clone()));
        TerminalState::with_reconnect(conn, reopen)
    };
    t.cursor_blink = !s.args.no_blink;
    if let Some((bg, text)) = &s.args.theme {
        t.set_theme(Theme::from_setting_strings(bg, text));
    }
    s.term = Some(t);
}

fn headless() -> bool {
    rune::tooling::headless_run().is_some()
}

fn poll(s: &mut Demo, clock: f64, tx: &mut Tx) -> bool {
    let start = Instant::now();
    let mut changed = false;
    while s.marks.first().is_some_and(|(t, _)| *t <= clock) {
        let (_, m) = s.marks.remove(0);
        changed = true;
        match m.as_str() {
            "drop" => {
                if let Some(p) = &s.proxy {
                    Proxy::drop_ui(p);
                    if let Some(t) = &mut s.term {
                        // Take the close now, then let the reconnect finish (real time).
                        t.poll(None);
                        t.settle_reconnect(Duration::from_secs(3));
                    }
                }
            }
            "attention" => {
                if let Some(p) = &s.proxy {
                    Proxy::attention(p);
                }
            }
            m if m.starts_with("theme:") => {
                let mut parts = m.splitn(3, ':').skip(1);
                let (bg, text) = (parts.next().unwrap_or(""), parts.next().unwrap_or("auto"));
                if let Some(t) = &mut s.term {
                    t.set_theme(Theme::from_setting_strings(bg, text));
                }
            }
            m if m.starts_with("scroll:") => {
                if let (Some(t), Ok(n)) = (&mut s.term, m["scroll:".len()..].parse::<i32>()) {
                    t.scroll_lines(n);
                }
            }
            _ => changed = false,
        }
    }
    if let Some(t) = &mut s.term {
        // Headless frames run on a virtual clock: wait for the shell instead.
        changed |= t.poll_tx(headless().then_some(Duration::from_millis(1500)), clock, tx);
        for ev in t.take_events() {
            changed = true;
            let line = match ev {
                TerminalEvent::Attention => {
                    s.attention = true;
                    "attention: the session wants you".to_string()
                }
                TerminalEvent::Bell => "bell".into(),
                TerminalEvent::Title(t) => format!("title: {t}"),
                TerminalEvent::Clipboard(c) => {
                    // The web UI copies short text at once; so does the demo.
                    if c.len() <= 4096 {
                        tx.copy(c.clone());
                    }
                    format!("copy request: {} bytes", c.len())
                }
                TerminalEvent::Disconnected(why) => format!("disconnected: {why}"),
                TerminalEvent::Reconnected => "reconnected, replay follows".into(),
            };
            log::info!("terminal event: {line}");
            s.banner.push(line);
            if s.banner.len() > 3 {
                s.banner.remove(0);
            }
        }
        if let Some(p) = &s.args.dump_text {
            let dump = format!(
                "{}x{} offset {} history {} status {}\nselection: {:?}\nevents: {:?}\nbytes in {}, rows rebuilt {} in {} rebuilds, build {:.1} ms, polls {} ({:.1} ms)\n{}\n",
                t.size.0,
                t.size.1,
                t.display_offset(),
                t.history_len(),
                t.status,
                t.selection_text(),
                s.banner,
                t.bytes_in,
                t.rebuilt_rows,
                t.rebuilds,
                t.build_time.as_secs_f64() * 1e3,
                s.polls,
                s.poll_time.as_secs_f64() * 1e3,
                t.visible_text().trim_end()
            );
            if dump != s.last_dump {
                let _ = std::fs::write(p, &dump);
                s.last_dump = dump;
            }
        }
    }
    s.polls += 1;
    s.poll_time += start.elapsed();
    changed
}

fn on_key(s: &mut Demo, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // Any key answers the attention banner.
    s.attention = false;
    s.term.as_mut().is_some_and(|t| t.on_key_tx(ev, tx, lens))
}

fn on_ime(s: &mut Demo, ime: &Ime, _tx: &mut Tx) -> bool {
    s.term.as_mut().is_some_and(|t| t.on_ime(ime))
}

fn view(s: &Demo, _cx: &mut Cx<Demo>) -> El {
    let bg = s.term.as_ref().map_or(terminal::BG, |t| t.theme().background);
    let mut root = col()
        .w_full()
        .h_full()
        .fill(rgb(bg))
        .on_window_focus(|s: &mut Demo, focused: bool, _tx: &mut Tx| {
            if let Some(t) = &mut s.term {
                t.set_focused(focused);
            }
        })
        // The window's paste (the macOS Edit menu's Paste, which takes ⌘V before
        // any key handler; a script's `paste`) goes to the terminal.
        .on_paste(|s: &mut Demo, text: &str, _tx: &mut Tx| {
            if let Some(t) = &mut s.term {
                t.paste(text);
            }
        });
    if !s.banner.is_empty() {
        let color = if s.attention { 0xd29922 } else { 0x30363d };
        root = root.child(
            el().key("term-banner").w_full().padding(6.0).fill(rgb(color)).child(
                text(s.banner.join("   |   ")).font(TextStyle::new(12.0, 600.0)).color(rgb(0xffffff)),
            ),
        );
    }
    match &s.term {
        // The pane fills a row (its width grows, its height is the row's).
        Some(t) => root.child(rune::row().w_full().grow(1.0).min_h(0.0).child(terminal::pane(t, lens))),
        None => root.child(text(s.error.clone()).color(rgb(0xf85149)).wrap()),
    }
}

/// Key-to-echo latency of the client's socket thread, without the UI and
/// without the stand-in's own 5 ms polling: an in-process daemon that answers
/// the health check and echoes every WebSocket frame at once (blocking reads),
/// then `HttpDaemon::open_terminal`, one byte at a time. A baseline client
/// with a blocking reader thread (no polling) runs against the same server.
fn bench_echo(n: usize) {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let token = "bench-token".to_string();
    let tok = token.clone();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let tok = tok.clone();
            std::thread::spawn(move || {
                let _ = s.set_nodelay(true);
                let mut peek = [0u8; 1024];
                let n = s.peek(&mut peek).unwrap_or(0);
                let head = String::from_utf8_lossy(&peek[..n]).to_string();
                if head.starts_with("GET /api/health") {
                    let mut s = s;
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf);
                    let nonce = head.split("nonce=").nth(1).and_then(|r| r.split([' ', '&']).next()).unwrap_or("");
                    let body = serde_json::json!({ "ok": true, "proof": sbxw_desktop::client::proof(&tok, nonce) }).to_string();
                    let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    return;
                }
                let Ok(mut ws) = tungstenite::accept(s) else { return };
                while let Ok(m) = ws.read() {
                    if let tungstenite::Message::Binary(b) = m {
                        if ws.send(tungstenite::Message::Binary(b)).is_err() {
                            break;
                        }
                    }
                }
            });
        }
    });
    let stats = |name: &str, v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        println!(
            "{name}: n {} min {:.2} p50 {:.2} p90 {:.2} p99 {:.2} max {:.2} mean {:.2} ms",
            v.len(),
            v[0],
            v[v.len() / 2],
            v[v.len() * 9 / 10],
            v[(v.len() * 99 / 100).min(v.len() - 1)],
            v[v.len() - 1],
            mean
        );
    };
    // Through client.rs (its 5 ms read timeout loop).
    let d = HttpDaemon::connect(&addr, &token).expect("bench daemon");
    let conn = d.open_terminal(&TerminalRequest { sandbox: "x".into(), mode: None, thread: None }, Arc::new(|| {})).unwrap();
    let mut v = vec![];
    for i in 0..n {
        // Spread the keys like typing, out of phase with the 5 ms loop.
        std::thread::sleep(Duration::from_micros(7_000 + (i as u64 * 1_337) % 9_000));
        let t = Instant::now();
        conn.input.send(TermInput::Bytes(vec![b'a'])).unwrap();
        loop {
            match conn.events.recv_timeout(Duration::from_secs(2)) {
                Ok(TermEvent::Output(_)) => break,
                Ok(_) => continue,
                Err(e) => panic!("no echo: {e}"),
            }
        }
        v.push(t.elapsed().as_secs_f64() * 1e3);
    }
    stats("client.rs socket thread (5 ms read timeout)", &mut v);
    // What the polling costs at rest: the socket thread's CPU over 5 idle seconds.
    let ws_ticks = || -> Option<u64> {
        for e in std::fs::read_dir("/proc/self/task").ok()?.flatten() {
            let p = e.path();
            if std::fs::read_to_string(p.join("comm")).ok()?.trim() == "terminal-ws" {
                let stat = std::fs::read_to_string(p.join("stat")).ok()?;
                let f: Vec<&str> = stat.rsplit(')').next()?.split_whitespace().collect();
                return Some(f[11].parse::<u64>().ok()? + f[12].parse::<u64>().ok()?);
            }
        }
        None
    };
    let wakeups = |pid_task: &str| -> Option<u64> {
        let s = std::fs::read_to_string(format!("/proc/self/task/{pid_task}/status")).ok()?;
        let n = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok());
        Some(n("voluntary_ctxt_switches:")? + n("nonvoluntary_ctxt_switches:")?)
    };
    let ws_tid = std::fs::read_dir("/proc/self/task").ok().and_then(|d| {
        d.flatten().find(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "terminal-ws")).map(|e| e.file_name().to_string_lossy().to_string())
    });
    if let (Some(t0), Some(tid)) = (ws_ticks(), ws_tid) {
        let w0 = wakeups(&tid).unwrap_or(0);
        std::thread::sleep(Duration::from_secs(5));
        let (t1, w1) = (ws_ticks().unwrap_or(t0), wakeups(&tid).unwrap_or(w0));
        let hz = 100.0; // USER_HZ on Linux
        println!(
            "socket thread at rest: {:.1} ms CPU per second, {:.0} wake-ups per second",
            (t1 - t0) as f64 / hz * 1000.0 / 5.0,
            (w1 - w0) as f64 / 5.0
        );
    }
    // Baseline: blocking reads on a cloned stream, writes from the caller.
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{addr}/ws?sandbox=x").as_str()).unwrap();
    req.headers_mut().insert("Authorization", format!("Bearer {token}").parse().unwrap());
    let stream = TcpStream::connect(&addr).unwrap();
    stream.set_nodelay(true).unwrap();
    let (mut ws, _) = tungstenite::client(req, stream).unwrap();
    let mut v = vec![];
    for i in 0..n {
        std::thread::sleep(Duration::from_micros(7_000 + (i as u64 * 1_337) % 9_000));
        let t = Instant::now();
        ws.send(tungstenite::Message::Binary(vec![b'a'].into())).unwrap();
        loop {
            if let Ok(tungstenite::Message::Binary(_)) = ws.read() {
                break;
            }
        }
        v.push(t.elapsed().as_secs_f64() * 1e3);
    }
    stats("baseline, blocking read (no polling)", &mut v);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn")).init();
    if let Some(n) = args().bench_echo {
        bench_echo(n);
        return;
    }
    let spec = AppSpec::<Demo> {
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        families: ("Inter", terminal::MONO_FAMILIES),
        ..AppSpec::new("sbxw terminal", (960.0, 640.0), init, view)
    };
    rune::App::new(spec).on_ime(on_ime).run();
}
