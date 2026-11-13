//! Every call to the sbxw daemon goes through this module: the [`Daemon`]
//! trait, [`HttpDaemon`] (the daemon's current HTTP and WebSocket API on
//! loopback), [`FixtureDaemon`] (a JSON file, for headless screens with no
//! daemon) and [`Link`], the thread that keeps a connection up, reconnects
//! when the daemon restarts and turns its event stream into list refreshes.
//! The rest of the app sees only the trait and [`LinkEvent`]s.
//!
//! What it speaks (daemon side, read from the code, not changed). The full
//! route list, covered or not, is `desktop/docs/daemon-api.md`.
//! * `GET /api/health?nonce=`: `{ok, proof}`, `proof = sha256(token || nonce)`
//!   (`crates/daemon/src/http/guard.rs`, `token.rs`). Checked before any
//!   request carries the token, so a process squatting the port never sees it.
//! * Auth: native clients send no `Origin` and `Authorization: Bearer <ui token>`.
//!   The token is minted at every daemon start and written to
//!   `<sbxw home>/run/ui-token` (`~/.sbxw/run/ui-token`): after a restart the
//!   old token is refused, so [`TokenSource::File`] is read again on every
//!   reconnect.
//! * `GET /api/sandboxes`: a JSON array of `SandboxItem`
//!   (`crates/daemon/src/svc/sandboxes.rs:30`).
//! * `POST /api/sandboxes/:name/stop`: `{ok: true}` or an error.
//! * Starting: the web UI has no start route. "Connect" opens the agent's
//!   terminal (`/ws?mode=claude`), and the daemon runs `sbx run`, which starts
//!   a stopped sandbox (`assets/js/sandboxes.js:501`). So does this app.
//! * `GET /api/stream`: server-sent events, one named event per bus change
//!   (`crates/daemon/src/bus.rs`, `http/stream.rs`). There is no "sandbox list
//!   changed" event: the web UI reads the list again after its own actions and
//!   when other events arrive. [`Link`] does the same (see [`REFRESH_ON`]),
//!   plus a slow re-read in case `sbx` changed something behind the daemon.
//! * `GET /ws?sandbox=&mode=&thread=`: the terminal (`crates/daemon/src/http/ws.rs`).
//!   Binary frames both ways are PTY bytes; text frames from the client are JSON
//!   `{"type":"resize","cols","rows"}`; text frames from the daemon are an error
//!   line to show, or `{"type":"attention"}` (the bell).
//!
//! Adding a route is mechanical: one method on [`Daemon`], implemented on
//! [`HttpDaemon`] with [`HttpDaemon::get`] / [`HttpDaemon::post`], on
//! [`FixtureDaemon`] from its JSON, and a line in `docs/daemon-api.md`.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, ErrorKind};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

/// The daemon's default UI address (`DEFAULT_WEB_ADDR`, `crates/core/src/config.rs:19`).
pub const DEFAULT_ADDR: &str = "127.0.0.1:7681";

/// One sandbox as `GET /api/sandboxes` lists it (`SandboxItem`). Fields the
/// daemon adds later are kept in `extra` and reach the screens unchanged.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Sandbox {
    pub name: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default)]
    pub chat: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub detached: bool,
    #[serde(default)]
    pub remote_control: bool,
    #[serde(default)]
    pub approvals: usize,
    #[serde(default)]
    pub kits_waiting: usize,
    #[serde(default)]
    pub old_sbx: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable_since: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Sandbox {
    pub fn new(name: &str, agent: &str, status: &str) -> Self {
        serde_json::from_value(serde_json::json!({ "name": name, "agent": agent, "status": status })).expect("sandbox")
    }
}

/// Which terminal to open: the query string of `/ws`.
#[derive(Debug, Clone, Default)]
pub struct TerminalRequest {
    pub sandbox: String,
    /// `None` is the agent; `bash`, `monitor`, `thread`.
    pub mode: Option<String>,
    pub thread: Option<String>,
}

pub enum TermEvent {
    Output(Vec<u8>),
    /// A text frame that is not JSON: the daemon's error line for the pane.
    Notice(String),
    Attention,
    Closed(String),
}

pub enum TermInput {
    Bytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

/// A live terminal: events arrive on `events`, and `wake` is called after
/// each one so the UI thread drains them; keys and resizes go out on `input`.
pub struct TerminalConn {
    pub events: mpsc::Receiver<TermEvent>,
    pub input: mpsc::Sender<TermInput>,
}

/// Called from a background thread when something is queued (the UI's waker).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// One event of `/api/stream`: its SSE name and its `data:` text.
#[derive(Debug, Clone, PartialEq)]
pub struct BusEvent {
    pub name: String,
    pub data: String,
}

pub trait Daemon: Send + Sync + 'static {
    /// Where it talks to, for the window's status line.
    fn describe(&self) -> String;
    /// `GET /api/sandboxes`.
    fn sandboxes(&self) -> Result<Vec<Sandbox>>;
    /// `POST /api/sandboxes/:name/stop`.
    fn stop_sandbox(&self, name: &str) -> Result<()>;
    /// `GET /ws`. Opening the agent (`mode: None`) of a stopped sandbox starts it.
    fn open_terminal(&self, req: &TerminalRequest, wake: Wake) -> Result<TerminalConn>;
    /// `GET /api/stream`: calls `sink` for each event until the stream ends
    /// (an error, or `sink` returning false). Blocks.
    fn subscribe(&self, sink: &mut dyn FnMut(BusEvent) -> bool) -> Result<()>;
}

// ---------------------------------------------------------------------------
// HTTP

pub struct HttpDaemon {
    addr: String,
    token: String,
    agent: ureq::Agent,
}

impl HttpDaemon {
    /// Connects and checks the daemon proves it knows `token` before any
    /// request carries it.
    pub fn connect(addr: &str, token: &str) -> Result<Self> {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).build();
        let nonce = random_hex(16);
        #[derive(Deserialize)]
        struct Health {
            proof: Option<String>,
        }
        let h: Health = agent
            .get(&format!("http://{addr}/api/health?nonce={nonce}"))
            .timeout(Duration::from_secs(3))
            .call()
            .map_err(|e| anyhow!(Unreachable(e.to_string())))?
            .into_json()?;
        if h.proof.as_deref() != Some(proof(token, &nonce).as_str()) {
            bail!(WrongToken);
        }
        Ok(Self { addr: addr.to_string(), token: token.to_string(), agent })
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// `GET path`, JSON answer.
    pub fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let r = self.agent.get(&format!("http://{}{path}", self.addr)).set("Authorization", &self.auth()).timeout(Duration::from_secs(30)).call();
        Ok(answer(r, path)?.into_json()?)
    }

    /// `POST path` with a JSON body, JSON answer.
    pub fn post<T: serde::de::DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> Result<T> {
        let r = self
            .agent
            .post(&format!("http://{}{path}", self.addr))
            .set("Authorization", &self.auth())
            .timeout(Duration::from_secs(120))
            .send_json(body.clone());
        Ok(answer(r, path)?.into_json()?)
    }
}

/// The daemon's errors come back as `{"error": "..."}` or plain text: show that, not ureq's chain.
fn answer(r: Result<ureq::Response, ureq::Error>, path: &str) -> Result<ureq::Response> {
    match r {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                .unwrap_or_else(|| body.trim().to_string());
            bail!("{path}: {code} {msg}")
        }
        Err(e) => Err(anyhow!(Unreachable(e.to_string()))),
    }
}

/// The daemon did not answer at all (connection refused, timeout).
#[derive(Debug)]
pub struct Unreachable(pub String);
impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "no answer: {}", self.0)
    }
}
impl std::error::Error for Unreachable {}

/// Something answered but could not prove it knows the token.
#[derive(Debug)]
pub struct WrongToken;
impl std::fmt::Display for WrongToken {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "the process there does not know this token: refusing to send it")
    }
}
impl std::error::Error for WrongToken {}

pub fn proof(token: &str, nonce: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.update(nonce.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

impl Daemon for HttpDaemon {
    fn describe(&self) -> String {
        format!("daemon at {}", self.addr)
    }

    fn sandboxes(&self) -> Result<Vec<Sandbox>> {
        self.get("/api/sandboxes")
    }

    fn stop_sandbox(&self, name: &str) -> Result<()> {
        let _: serde_json::Value = self.post(&format!("/api/sandboxes/{}/stop", enc(name)), &serde_json::json!({}))?;
        Ok(())
    }

    fn open_terminal(&self, req: &TerminalRequest, wake: Wake) -> Result<TerminalConn> {
        let mut url = format!("ws://{}/ws?sandbox={}", self.addr, enc(&req.sandbox));
        if let Some(m) = &req.mode {
            url += &format!("&mode={}", enc(m));
        }
        if let Some(t) = &req.thread {
            url += &format!("&thread={}", enc(t));
        }
        let mut request = url.as_str().into_client_request()?;
        request.headers_mut().insert("Authorization", self.auth().parse()?);
        let sock = self.addr.to_socket_addrs()?.next().ok_or_else(|| anyhow!("no address for {}", self.addr))?;
        let stream = TcpStream::connect_timeout(&sock, Duration::from_secs(3))?;
        stream.set_nodelay(true)?;
        let (socket, resp) = tungstenite::client(request, stream).map_err(|e| anyhow!("WebSocket handshake: {e}"))?;
        log::info!("terminal socket open ({})", resp.status());
        // Short read timeout: one thread both reads and drains what to send.
        socket.get_ref().set_read_timeout(Some(Duration::from_millis(5)))?;
        let (ev_tx, ev_rx) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel();
        let send = Events { tx: ev_tx, wake };
        std::thread::Builder::new().name("terminal-ws".into()).spawn(move || pump(socket, send, in_rx))?;
        Ok(TerminalConn { events: ev_rx, input: in_tx })
    }

    fn subscribe(&self, sink: &mut dyn FnMut(BusEvent) -> bool) -> Result<()> {
        // No overall timeout (the stream is long-lived); the daemon sends a
        // keep-alive comment every 15 s, so 45 s of silence means it is gone.
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout_read(Duration::from_secs(45)).build();
        let r = agent.get(&format!("http://{}/api/stream", self.addr)).set("Authorization", &self.auth()).set("Accept", "text/event-stream").call();
        let resp = answer(r, "/api/stream")?;
        read_sse(BufReader::new(resp.into_reader()), sink)
    }
}

/// Reads server-sent events until the stream ends or `sink` says stop.
pub fn read_sse(mut r: impl BufRead, sink: &mut dyn FnMut(BusEvent) -> bool) -> Result<()> {
    let (mut name, mut data) = (String::new(), Vec::<String>::new());
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            bail!("the event stream ended");
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if !data.is_empty() || !name.is_empty() {
                let ev = BusEvent { name: if name.is_empty() { "message".into() } else { std::mem::take(&mut name) }, data: data.join("\n") };
                data.clear();
                if !sink(ev) {
                    return Ok(());
                }
            }
        } else if let Some(v) = l.strip_prefix("event:") {
            name = v.trim_start().to_string();
        } else if let Some(v) = l.strip_prefix("data:") {
            data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
        }
        // `:` comments (keep-alives), `id:` and `retry:` are ignored.
    }
}

/// The socket thread's side of `events`: queue, then wake the UI.
struct Events {
    tx: mpsc::Sender<TermEvent>,
    wake: Wake,
}

impl Events {
    fn unbounded_send(&self, ev: TermEvent) -> Result<(), ()> {
        self.tx.send(ev).map_err(|_| ())?;
        (self.wake)();
        Ok(())
    }
}

fn pump(mut ws: tungstenite::WebSocket<TcpStream>, tx: Events, rx: mpsc::Receiver<TermInput>) {
    let why = loop {
        match ws.read() {
            Ok(Message::Binary(b)) => {
                let _ = tx.unbounded_send(TermEvent::Output(b.to_vec()));
            }
            Ok(Message::Text(t)) => {
                let ev = match serde_json::from_str::<serde_json::Value>(&t) {
                    Ok(v) if v["type"] == "attention" => TermEvent::Attention,
                    Ok(_) => continue,
                    Err(_) => TermEvent::Notice(t.to_string()),
                };
                let _ = tx.unbounded_send(ev);
            }
            Ok(Message::Close(_)) => break "closed by the daemon".to_string(),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => break e.to_string(),
        }
        loop {
            match rx.try_recv() {
                Ok(TermInput::Bytes(b)) => {
                    if let Err(e) = ws.send(Message::Binary(b.into())) {
                        let _ = tx.unbounded_send(TermEvent::Closed(e.to_string()));
                        return;
                    }
                }
                Ok(TermInput::Resize { cols, rows }) => {
                    let msg = serde_json::json!({ "type": "resize", "cols": cols, "rows": rows }).to_string();
                    let _ = ws.send(Message::Text(msg.into()));
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    return;
                }
            }
        }
    };
    let _ = tx.unbounded_send(TermEvent::Closed(why));
}

// ---------------------------------------------------------------------------
// Fixture

/// A daemon read from a JSON fixture (`desktop/fixtures/*.json`, key
/// `daemon`): what each route would answer. No terminals, no events. Stops
/// change the fixture's copy in memory, so a script can press Stop.
pub struct FixtureDaemon {
    name: String,
    data: std::sync::Mutex<FixtureData>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FixtureData {
    #[serde(default)]
    pub sandboxes: Vec<Sandbox>,
}

impl FixtureDaemon {
    pub fn new(name: &str, data: FixtureData) -> Self {
        Self { name: name.to_string(), data: std::sync::Mutex::new(data) }
    }
}

impl Daemon for FixtureDaemon {
    fn describe(&self) -> String {
        format!("fixture {}", self.name)
    }
    fn sandboxes(&self) -> Result<Vec<Sandbox>> {
        Ok(self.data.lock().unwrap().sandboxes.clone())
    }
    fn stop_sandbox(&self, name: &str) -> Result<()> {
        let mut d = self.data.lock().unwrap();
        let s = d.sandboxes.iter_mut().find(|s| s.name == name).ok_or_else(|| anyhow!("no sandbox {name}"))?;
        s.status = "stopped".into();
        Ok(())
    }
    fn open_terminal(&self, req: &TerminalRequest, _wake: Wake) -> Result<TerminalConn> {
        bail!("fixture {}: no terminal for {} (use the stand-in daemon for terminals)", self.name, req.sandbox)
    }
    fn subscribe(&self, _sink: &mut dyn FnMut(BusEvent) -> bool) -> Result<()> {
        // A fixture has no events: hold the stream open forever.
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

// ---------------------------------------------------------------------------
// Link: one connection kept up, events turned into refreshes

/// Where the UI token comes from.
#[derive(Debug, Clone)]
pub enum TokenSource {
    Fixed(String),
    /// Read on every (re)connect: the daemon mints a new one each start.
    File(PathBuf),
}

impl TokenSource {
    /// `$HOME/.sbxw/run/ui-token`, as the daemon writes it (`crates/daemon/src/store.rs`).
    pub fn default_file() -> PathBuf {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        home.join(".sbxw").join("run").join("ui-token")
    }

    fn read(&self) -> Result<String> {
        match self {
            TokenSource::Fixed(t) => Ok(t.clone()),
            TokenSource::File(p) => Ok(std::fs::read_to_string(p).with_context(|| format!("no UI token at {}", p.display()))?.trim().to_string()),
        }
    }
}

/// How to reach a daemon: a real one, or a fixture.
#[derive(Clone)]
pub enum Target {
    Http { addr: String, token: TokenSource },
    Fixture(Arc<FixtureDaemon>),
}

/// What the link tells the UI.
pub enum LinkEvent {
    /// Connected (again): use this daemon for terminals and actions.
    Up(Arc<dyn Daemon>),
    /// Not connected; a readable reason. The link keeps retrying.
    Down(String),
    Sandboxes(Vec<Sandbox>),
    /// A `/api/stream` event, for screens of later waves.
    Bus(BusEvent),
    /// An action the UI asked for finished.
    Done { what: String, result: Result<(), String> },
}

/// Events on `/api/stream` after which the sandbox list is read again: the
/// ones after which the web UI re-reads it (`loadSandboxes` callers in
/// `assets/js/`), plus `session` (an agent exiting can mean a stop).
pub const REFRESH_ON: &[&str] = &["session", "provision", "lead", "lead-end", "codemap"];
/// The list is also re-read this often while connected.
pub const REFRESH_EVERY: Duration = Duration::from_secs(15);

/// Keeps one daemon connection up on a thread of its own and reports through
/// a channel. The UI drains [`Link::try_recv`] in its `poll`.
pub struct Link {
    rx: mpsc::Receiver<LinkEvent>,
    tx: mpsc::Sender<LinkEvent>,
    refresh: mpsc::Sender<Duration>,
    wake: Wake,
    /// Background jobs not yet answered (headless runs wait for them).
    pub pending: Arc<std::sync::atomic::AtomicUsize>,
}

impl Link {
    pub fn start(target: Target, wake: Wake) -> Link {
        let (tx, rx) = mpsc::channel();
        let (refresh, kicks) = mpsc::channel::<Duration>();
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let link = Link { rx, tx: tx.clone(), refresh: refresh.clone(), wake: wake.clone(), pending };
        let send = Sender { tx, wake };
        let current: Current = Arc::new(std::sync::Mutex::new(None));
        {
            let (send, current) = (send.clone(), current.clone());
            std::thread::Builder::new().name("daemon-list".into()).spawn(move || refresher(current, send, kicks)).expect("list thread");
        }
        std::thread::Builder::new().name("daemon-link".into()).spawn(move || supervise(target, send, current, refresh)).expect("link thread");
        link
    }

    pub fn try_recv(&self) -> Option<LinkEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<LinkEvent> {
        self.rx.recv_timeout(d).ok()
    }

    /// Read the list again after `delay` (0 for now).
    pub fn refresh(&self, delay: Duration) {
        let _ = self.refresh.send(delay);
    }

    /// Runs `job` on a thread with `daemon`; reports `Done`, then the list read again.
    pub fn run(&self, daemon: Arc<dyn Daemon>, what: String, job: impl FnOnce(&dyn Daemon) -> Result<()> + Send + 'static) {
        use std::sync::atomic::Ordering;
        self.pending.fetch_add(1, Ordering::SeqCst);
        let send = Sender { tx: self.tx.clone(), wake: self.wake.clone() };
        let pending = self.pending.clone();
        std::thread::spawn(move || {
            let result = job(daemon.as_ref()).map_err(|e| format!("{e:#}"));
            let list = daemon.sandboxes();
            send.send(LinkEvent::Done { what, result });
            if let Ok(l) = list {
                send.send(LinkEvent::Sandboxes(l));
            }
            pending.fetch_sub(1, Ordering::SeqCst);
            (send.wake)();
        });
    }
}

/// The daemon the link is connected to right now, if any.
type Current = Arc<std::sync::Mutex<Option<Arc<dyn Daemon>>>>;

#[derive(Clone)]
struct Sender {
    tx: mpsc::Sender<LinkEvent>,
    wake: Wake,
}

impl Sender {
    fn send(&self, ev: LinkEvent) -> bool {
        let ok = self.tx.send(ev).is_ok();
        (self.wake)();
        ok
    }
}

/// A short reason for the window, not the error chain.
pub fn readable(target: &Target, e: &anyhow::Error) -> String {
    let addr = match target {
        Target::Http { addr, .. } => addr.as_str(),
        Target::Fixture(_) => "the fixture",
    };
    if e.downcast_ref::<Unreachable>().is_some() {
        format!("No sbxw daemon at {addr}. Start it with `sbxw up`; retrying.")
    } else if e.downcast_ref::<WrongToken>().is_some() {
        format!("The process at {addr} does not know sbxw's UI token; retrying.")
    } else if let (Target::Http { token: TokenSource::File(p), .. }, Some(_)) = (target, e.downcast_ref::<std::io::Error>()) {
        format!("No UI token at {}. Start the daemon with `sbxw up`; retrying.", p.display())
    } else {
        format!("{e:#}")
    }
}

fn connect(target: &Target) -> Result<Arc<dyn Daemon>> {
    match target {
        Target::Http { addr, token } => {
            let t = token.read().map_err(|e| anyhow!(std::io::Error::new(ErrorKind::NotFound, format!("{e:#}"))))?;
            Ok(Arc::new(HttpDaemon::connect(addr, &t)?))
        }
        Target::Fixture(f) => Ok(f.clone()),
    }
}

fn supervise(target: Target, send: Sender, current: Current, kick: mpsc::Sender<Duration>) {
    let mut backoff = Duration::from_millis(250);
    let mut last_down = String::new();
    loop {
        let daemon = match connect(&target) {
            Ok(d) => d,
            Err(e) => {
                let why = readable(&target, &e);
                if why != last_down {
                    log::warn!("daemon: {e:#}");
                    send.send(LinkEvent::Down(why.clone()));
                    last_down = why;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(3));
                continue;
            }
        };
        backoff = Duration::from_millis(250);
        last_down.clear();
        log::info!("daemon: connected to {}", daemon.describe());
        if !send.send(LinkEvent::Up(daemon.clone())) {
            return;
        }
        match daemon.sandboxes() {
            Ok(l) => {
                send.send(LinkEvent::Sandboxes(l));
            }
            Err(e) => {
                send.send(LinkEvent::Down(readable(&target, &e)));
            }
        }
        *current.lock().unwrap() = Some(daemon.clone());
        let mut sink = |ev: BusEvent| {
            if REFRESH_ON.contains(&ev.name.as_str()) {
                // A burst of events (a bring-up's progress lines) makes one read.
                let _ = kick.send(Duration::from_millis(300));
            }
            send.send(LinkEvent::Bus(ev))
        };
        let why = match daemon.subscribe(&mut sink) {
            Ok(()) => return,
            Err(e) => e,
        };
        *current.lock().unwrap() = None;
        log::warn!("daemon: stream ended: {why:#}");
        let msg = match &target {
            Target::Http { addr, .. } => format!("Lost the sbxw daemon at {addr}; reconnecting."),
            Target::Fixture(_) => "The fixture's stream ended.".to_string(),
        };
        send.send(LinkEvent::Down(msg.clone()));
        last_down = msg;
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Reads the list when kicked (kicks that land before the read make one), and
/// every [`REFRESH_EVERY`], with whatever daemon is connected.
fn refresher(current: Current, send: Sender, kicks: mpsc::Receiver<Duration>) {
    let mut due: Option<Instant> = None;
    let mut slow = Instant::now() + REFRESH_EVERY;
    loop {
        let next = due.map_or(slow, |d| d.min(slow));
        match kicks.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(d) => {
                let at = Instant::now() + d;
                due = Some(due.map_or(at, |x| x.min(at)));
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if due.is_some_and(|d| d <= now) || slow <= now {
            due = None;
            slow = now + REFRESH_EVERY;
            let d = current.lock().unwrap().clone();
            if let Some(d) = d {
                match d.sandboxes() {
                    Ok(l) => {
                        if !send.send(LinkEvent::Sandboxes(l)) {
                            return;
                        }
                    }
                    Err(e) => log::warn!("daemon: list: {e:#}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_frames_are_parsed() {
        let raw = ": keep-alive\n\nevent: clients\ndata: 2\n\nevent: session\ndata: {\"a\":1}\ndata: x\n\ndata: plain\n\n";
        let mut got = vec![];
        let r = read_sse(raw.as_bytes(), &mut |e| {
            got.push(e);
            true
        });
        assert!(r.is_err(), "a stream that ends is an error (reconnect)");
        assert_eq!(
            got,
            vec![
                BusEvent { name: "clients".into(), data: "2".into() },
                BusEvent { name: "session".into(), data: "{\"a\":1}\nx".into() },
                BusEvent { name: "message".into(), data: "plain".into() },
            ]
        );
    }

    #[test]
    fn sandbox_keeps_fields_it_does_not_know() {
        let s: Sandbox = serde_json::from_str(r#"{"name":"a","agent":"claude","status":"running","chat":true,"approvals":2,"new_field":[1]}"#).unwrap();
        assert!(s.chat);
        assert_eq!(s.approvals, 2);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["new_field"], serde_json::json!([1]));
    }

    #[test]
    fn readable_reasons() {
        let t = Target::Http { addr: "127.0.0.1:1".into(), token: TokenSource::Fixed("x".into()) };
        let e = HttpDaemon::connect("127.0.0.1:1", "x").err().unwrap();
        assert_eq!(readable(&t, &e), "No sbxw daemon at 127.0.0.1:1. Start it with `sbxw up`; retrying.");
        let t = Target::Http { addr: "a".into(), token: TokenSource::File("/nonexistent/ui-token".into()) };
        let e = connect(&t).err().unwrap();
        assert!(readable(&t, &e).starts_with("No UI token at /nonexistent/ui-token."), "{}", readable(&t, &e));
    }
}
