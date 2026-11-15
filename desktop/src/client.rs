//! Every call to the sbxw daemon goes through this module: the [`Daemon`]
//! trait, [`HttpDaemon`] (the daemon's current HTTP and WebSocket API on
//! loopback), [`FixtureDaemon`] (a JSON file, for headless screens with no
//! daemon) and [`Link`], the thread that keeps a connection up, reconnects
//! when the daemon restarts and turns its event stream into list refreshes.
//! The rest of the app sees only the trait and [`LinkEvent`]s.
//!
//! What it speaks (daemon side, read from the code, not changed). The full
//! route list, covered or not, is `desktop/.sbxw-artifacts/daemon-api.md`.
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
//! [`FixtureDaemon`] from its JSON, and a line in `.sbxw-artifacts/daemon-api.md`.

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

    /// Any route, for features (`src/feat/`): `method` `GET` or `POST`, a
    /// JSON body for a POST. Answers the HTTP status and the JSON body, a
    /// non-2xx one included (a plain-text body comes as `{"error": text}`).
    /// `Err` only when there is no answer at all.
    fn request(&self, method: &str, path: &str, body: &serde_json::Value, timeout: Duration) -> Result<(u16, serde_json::Value)> {
        let _ = (body, timeout);
        bail!("{method} {path}: not supported by {}", self.describe())
    }

    // ---- Jarvis (crates/daemon/src/http/jarvis.rs) ----

    /// `GET /api/jarvis/status`: `{state: absent|creating|starting|running|stopped|failed, error?}`.
    fn jarvis_status(&self) -> Result<serde_json::Value> {
        bail!("{}: no Jarvis here", self.describe())
    }
    /// `POST /api/jarvis/start`: the same shape, at once (the work runs in the background).
    fn jarvis_start(&self) -> Result<serde_json::Value> {
        bail!("{}: no Jarvis here", self.describe())
    }
    /// `GET /api/jarvis`: `{ok, proposals: [ui_view, newest first]}`.
    fn jarvis_proposals(&self) -> Result<Vec<serde_json::Value>> {
        Ok(vec![])
    }
    /// `POST /api/jarvis/proposals/:id/approve {via, ack_host_command}`: the
    /// proposal as it now stands. 409 ([`HttpError`]) when the daemon refuses:
    /// voice on a high-risk one, no acknowledgement of a host command, a file
    /// changed since the proposal, already answered.
    fn jarvis_approve(&self, id: &str, via: &str, ack_host_command: bool) -> Result<serde_json::Value> {
        let _ = (id, via, ack_host_command);
        bail!("{}: no Jarvis here", self.describe())
    }
    /// `POST /api/jarvis/proposals/:id/refuse {reason?}`: the proposal.
    fn jarvis_refuse(&self, id: &str, reason: Option<&str>) -> Result<serde_json::Value> {
        let _ = (id, reason);
        bail!("{}: no Jarvis here", self.describe())
    }

    // ---- Agents' sessions and messages ----

    /// `GET /api/sessions`: every agent session's `SessionInfo` (state, last_input, reply...).
    fn sessions(&self) -> Result<Vec<serde_json::Value>> {
        Ok(vec![])
    }
    /// `POST /api/chat/push {text, name}`: types `text` into that sandbox's agent.
    fn chat_push(&self, name: &str, text: &str) -> Result<()> {
        let _ = (name, text);
        bail!("{}: cannot send messages", self.describe())
    }

    // ---- Voice (crates/daemon/src/http/ui.rs, "Voice") ----

    /// `GET /api/voice`: `{ok, compiled, ready, speech: {supported, ...}, push_to_talk_key, ...}`.
    fn voice_status(&self) -> Result<serde_json::Value> {
        bail!("{}: no voice", self.describe())
    }
    /// `POST /api/voice/anonymise {text}`: the masked text and how many were masked.
    fn anonymise(&self, text: &str) -> Result<(String, u64)> {
        Ok((text.to_string(), 0))
    }
    /// `POST /api/voice/transcribe`, body 16 kHz mono f32 LE PCM: the masked text.
    fn transcribe(&self, pcm: Vec<u8>) -> Result<String> {
        let _ = pcm;
        bail!("{}: no voice", self.describe())
    }
    /// `POST /api/voice/speak {text}` (at most 1000 characters): the sample
    /// rate (`x-sample-rate`) and the body, 16-bit LE mono PCM, as it streams.
    fn speak(&self, text: &str) -> Result<(u32, Box<dyn std::io::Read + Send>)> {
        let _ = text;
        bail!("{}: no voice", self.describe())
    }
    /// `POST /api/voice/cancel`: stops a model download in progress.
    fn voice_cancel(&self) -> Result<()> {
        bail!("{}: no voice", self.describe())
    }
}

/// The daemon answered with an error status: the code, and its `error` text.
#[derive(Debug, Clone)]
pub struct HttpError {
    pub path: String,
    pub code: u16,
    pub message: String,
}
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {} {}", self.path, self.code, self.message)
    }
}
impl std::error::Error for HttpError {}

/// The daemon's own words for a failed call: an [`HttpError`]'s message, or the chain.
pub fn reason(e: &anyhow::Error) -> String {
    match e.downcast_ref::<HttpError>() {
        Some(h) => h.message.clone(),
        None => format!("{e:#}"),
    }
}

/// The status of a failed call, when the daemon answered.
pub fn status_of(e: &anyhow::Error) -> Option<u16> {
    e.downcast_ref::<HttpError>().map(|h| h.code)
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

    /// `POST path` with a raw body, JSON answer.
    pub fn post_bytes<T: serde::de::DeserializeOwned>(&self, path: &str, content_type: &str, body: &[u8]) -> Result<T> {
        let r = self
            .agent
            .post(&format!("http://{}{path}", self.addr))
            .set("Authorization", &self.auth())
            .set("Content-Type", content_type)
            .timeout(Duration::from_secs(120))
            .send_bytes(body);
        Ok(answer(r, path)?.into_json()?)
    }
}

/// A body as JSON; plain text becomes `{"error": text}`, nothing `null`.
fn json_body(text: &str) -> serde_json::Value {
    if text.trim().is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({ "error": text.trim() }))
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
            Err(anyhow!(HttpError { path: path.to_string(), code, message: msg }))
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
        socket.get_ref().set_read_timeout(None)?;
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

    fn jarvis_status(&self) -> Result<serde_json::Value> {
        self.get("/api/jarvis/status")
    }
    fn jarvis_start(&self) -> Result<serde_json::Value> {
        self.post("/api/jarvis/start", &serde_json::json!({}))
    }
    fn jarvis_proposals(&self) -> Result<Vec<serde_json::Value>> {
        let v: serde_json::Value = self.get("/api/jarvis")?;
        Ok(v["proposals"].as_array().cloned().unwrap_or_default())
    }
    fn jarvis_approve(&self, id: &str, via: &str, ack_host_command: bool) -> Result<serde_json::Value> {
        let v: serde_json::Value = self.post(&format!("/api/jarvis/proposals/{}/approve", enc(id)), &serde_json::json!({ "via": via, "ack_host_command": ack_host_command }))?;
        Ok(v["proposal"].clone())
    }
    fn jarvis_refuse(&self, id: &str, reason: Option<&str>) -> Result<serde_json::Value> {
        let body = match reason {
            Some(r) => serde_json::json!({ "reason": r }),
            None => serde_json::json!({}),
        };
        let v: serde_json::Value = self.post(&format!("/api/jarvis/proposals/{}/refuse", enc(id)), &body)?;
        Ok(v["proposal"].clone())
    }
    fn sessions(&self) -> Result<Vec<serde_json::Value>> {
        self.get("/api/sessions")
    }
    fn chat_push(&self, name: &str, text: &str) -> Result<()> {
        let _: serde_json::Value = self.post("/api/chat/push", &serde_json::json!({ "text": text, "name": name }))?;
        Ok(())
    }
    fn voice_status(&self) -> Result<serde_json::Value> {
        self.get("/api/voice")
    }
    fn anonymise(&self, text: &str) -> Result<(String, u64)> {
        let v: serde_json::Value = self.post("/api/voice/anonymise", &serde_json::json!({ "text": text }))?;
        Ok((v["text"].as_str().unwrap_or_default().to_string(), v["masked"].as_u64().unwrap_or(0)))
    }
    fn transcribe(&self, pcm: Vec<u8>) -> Result<String> {
        let v: serde_json::Value = self.post_bytes("/api/voice/transcribe", "application/octet-stream", &pcm)?;
        Ok(v["text"].as_str().unwrap_or_default().to_string())
    }
    fn speak(&self, text: &str) -> Result<(u32, Box<dyn std::io::Read + Send>)> {
        let path = "/api/voice/speak";
        let r = self.agent.post(&format!("http://{}{path}", self.addr)).set("Authorization", &self.auth()).send_json(serde_json::json!({ "text": text }));
        let resp = answer(r, path)?;
        let rate = resp.header("x-sample-rate").and_then(|r| r.parse().ok()).unwrap_or(24_000);
        Ok((rate, Box::new(resp.into_reader())))
    }
    fn request(&self, method: &str, path: &str, body: &serde_json::Value, timeout: Duration) -> Result<(u16, serde_json::Value)> {
        let url = format!("http://{}{path}", self.addr);
        let req = match method {
            "GET" => self.agent.get(&url),
            "POST" => self.agent.post(&url),
            m => bail!("{m} {path}: only GET and POST"),
        }
        .set("Authorization", &self.auth())
        .timeout(timeout);
        let r = if method == "POST" { req.send_json(body.clone()) } else { req.call() };
        match r {
            Ok(r) => {
                let code = r.status();
                Ok((code, json_body(&r.into_string().unwrap_or_default())))
            }
            Err(ureq::Error::Status(code, r)) => Ok((code, json_body(&r.into_string().unwrap_or_default()))),
            Err(e) => Err(anyhow!(Unreachable(e.to_string()))),
        }
    }
    fn voice_cancel(&self) -> Result<()> {
        let _: serde_json::Value = self.post("/api/voice/cancel", &serde_json::json!({}))?;
        Ok(())
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

const SOCKET: mio::Token = mio::Token(0);
const INPUT: mio::Token = mio::Token(1);

/// The terminal's socket thread. It sleeps in `mio::Poll` until the socket
/// is readable or writable, or input is queued, so a key goes out and its
/// echo comes back with no polling delay, and an idle terminal costs no
/// wake-ups. The input side stays a plain `mpsc::Sender<TermInput>` (what
/// `TerminalConn::input` always was): a forwarder thread blocks on it and
/// rings a `mio::Waker` for each message.
fn pump(mut ws: tungstenite::WebSocket<TcpStream>, tx: Events, rx: mpsc::Receiver<TermInput>) {
    let why = match pump_loop(&mut ws, &tx, rx) {
        Ok(why) => why,
        Err(e) => e.to_string(),
    };
    let _ = tx.unbounded_send(TermEvent::Closed(why));
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted))
}

fn pump_loop(ws: &mut tungstenite::WebSocket<TcpStream>, tx: &Events, rx: mpsc::Receiver<TermInput>) -> std::io::Result<String> {
    let mut poll = mio::Poll::new()?;
    let waker = Arc::new(mio::Waker::new(poll.registry(), INPUT)?);
    let (fwd_tx, fwd_rx) = mpsc::channel::<TermInput>();
    {
        let waker = waker.clone();
        std::thread::Builder::new().name("terminal-input".into()).spawn(move || {
            for m in rx {
                if fwd_tx.send(m).is_err() {
                    return;
                }
                let _ = waker.wake();
            }
            // The terminal was dropped: dropping `fwd_tx` tells the pump.
            drop(fwd_tx);
            let _ = waker.wake();
        })?;
    }
    ws.get_ref().set_nonblocking(true)?;
    // A second handle on the same socket, for readiness only: tungstenite
    // keeps reading and writing through its own.
    let mut source = mio::net::TcpStream::from_std(ws.get_ref().try_clone()?);
    poll.registry().register(&mut source, SOCKET, mio::Interest::READABLE | mio::Interest::WRITABLE)?;
    let mut events = mio::Events::with_capacity(8);
    loop {
        // Everything readable now (tungstenite may also hold frames the
        // handshake read): readiness is edge-triggered, so drain to WouldBlock.
        loop {
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
                Ok(Message::Close(_)) => return Ok("closed by the daemon".into()),
                Ok(_) => {}
                Err(e) if would_block(&e) => break,
                Err(e) => return Ok(e.to_string()),
            }
        }
        // Everything queued to send. A full socket buffer keeps the frame in
        // tungstenite's buffer (WouldBlock); the next writable event flushes it.
        loop {
            match fwd_rx.try_recv() {
                Ok(TermInput::Bytes(b)) => {
                    if let Err(e) = ws.send(Message::Binary(b.into())) {
                        if !would_block(&e) {
                            return Ok(e.to_string());
                        }
                    }
                }
                Ok(TermInput::Resize { cols, rows }) => {
                    let msg = serde_json::json!({ "type": "resize", "cols": cols, "rows": rows }).to_string();
                    if let Err(e) = ws.send(Message::Text(msg.into())) {
                        if !would_block(&e) {
                            return Ok(e.to_string());
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return Ok("closed".into());
                }
            }
        }
        match ws.flush() {
            Ok(()) => {}
            Err(e) if would_block(&e) => {}
            Err(e) => return Ok(e.to_string()),
        }
        loop {
            match poll.poll(&mut events, None) {
                Ok(()) => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture

/// A daemon read from a JSON fixture (`desktop/fixtures/*.json`, key
/// `daemon`): what each route would answer. No terminals, no events. Stops
/// change the fixture's copy in memory, so a script can press Stop.
pub struct FixtureDaemon {
    name: String,
    data: std::sync::Mutex<FixtureData>,
    /// The fixture's whole `daemon` object: features' keys, answered by
    /// their stand-in routes (`Feature::standin`) or by their data keys.
    raw: std::sync::Mutex<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FixtureData {
    #[serde(default)]
    pub sandboxes: Vec<Sandbox>,
    /// `GET /api/jarvis/status`; absent: `{state: "absent"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jarvis_status: Option<serde_json::Value>,
    /// `GET /api/jarvis`'s proposals, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jarvis_proposals: Vec<serde_json::Value>,
    /// `GET /api/sessions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<serde_json::Value>,
    /// `GET /api/voice`; absent: a daemon built without voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<serde_json::Value>,
    /// What an approval answers with, as a 409 (a file changed since the proposal...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jarvis_approve_error: Option<String>,
    /// Messages pushed, kept for tests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pushed: Vec<(String, String)>,
    /// What `POST /api/voice/transcribe` answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

impl FixtureDaemon {
    pub fn new(name: &str, data: FixtureData) -> Self {
        Self { name: name.to_string(), data: std::sync::Mutex::new(data), raw: std::sync::Mutex::new(serde_json::json!({})) }
    }

    /// With the fixture's whole `daemon` object, for features' routes.
    pub fn with_raw(self, raw: serde_json::Value) -> Self {
        *self.raw.lock().unwrap() = raw;
        self
    }

    /// The `daemon` object now (features' routes may have changed it).
    pub fn raw(&self) -> serde_json::Value {
        self.raw.lock().unwrap().clone()
    }

    /// The messages pushed so far, `(name, text)`.
    pub fn pushed(&self) -> Vec<(String, String)> {
        self.data.lock().unwrap().pushed.clone()
    }
}

impl Daemon for FixtureDaemon {
    fn describe(&self) -> String {
        format!("fixture {}", self.name)
    }
    fn sandboxes(&self) -> Result<Vec<Sandbox>> {
        Ok(self.data.lock().unwrap().sandboxes.clone())
    }
    fn request(&self, method: &str, path: &str, body: &serde_json::Value, _timeout: Duration) -> Result<(u16, serde_json::Value)> {
        let mut raw = self.raw.lock().unwrap();
        if let Some(sb) = self.data.lock().ok().map(|d| d.sandboxes.clone()) {
            raw["sandboxes"] = serde_json::to_value(sb).unwrap_or_default();
        }
        Ok(crate::feat::answer_from(&mut raw, method, path, body, &mut |_, _| {}))
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
    fn jarvis_status(&self) -> Result<serde_json::Value> {
        Ok(self.data.lock().unwrap().jarvis_status.clone().unwrap_or_else(|| serde_json::json!({ "state": "absent" })))
    }
    fn jarvis_start(&self) -> Result<serde_json::Value> {
        let mut d = self.data.lock().unwrap();
        let next = match d.jarvis_status.as_ref().and_then(|v| v["state"].as_str()) {
            Some("running") => "running",
            Some("stopped") => "starting",
            _ => "creating",
        };
        d.jarvis_status = Some(serde_json::json!({ "state": next }));
        Ok(d.jarvis_status.clone().unwrap())
    }
    fn jarvis_proposals(&self) -> Result<Vec<serde_json::Value>> {
        Ok(self.data.lock().unwrap().jarvis_proposals.clone())
    }
    fn jarvis_approve(&self, id: &str, via: &str, ack: bool) -> Result<serde_json::Value> {
        let mut d = self.data.lock().unwrap();
        let err = d.jarvis_approve_error.clone();
        let p = d.jarvis_proposals.iter_mut().find(|p| p["id"] == id).ok_or_else(|| anyhow!(HttpError { path: "approve".into(), code: 404, message: format!("no proposal {id}") }))?;
        fixture_rule(p, via, ack)?;
        if let Some(e) = err {
            return Err(anyhow!(HttpError { path: "approve".into(), code: 409, message: e }));
        }
        p["state"] = "applied".into();
        p["via"] = via.into();
        Ok(p.clone())
    }
    fn jarvis_refuse(&self, id: &str, reason: Option<&str>) -> Result<serde_json::Value> {
        let mut d = self.data.lock().unwrap();
        let p = d.jarvis_proposals.iter_mut().find(|p| p["id"] == id).ok_or_else(|| anyhow!("no proposal {id}"))?;
        p["state"] = "refused".into();
        if let Some(r) = reason {
            p["reason"] = r.into();
        }
        Ok(p.clone())
    }
    fn sessions(&self) -> Result<Vec<serde_json::Value>> {
        Ok(self.data.lock().unwrap().sessions.clone())
    }
    fn chat_push(&self, name: &str, text: &str) -> Result<()> {
        self.data.lock().unwrap().pushed.push((name.to_string(), text.to_string()));
        Ok(())
    }
    fn voice_status(&self) -> Result<serde_json::Value> {
        self.data.lock().unwrap().voice.clone().ok_or_else(|| anyhow!(HttpError { path: "/api/voice".into(), code: 404, message: "not found".into() }))
    }
    fn transcribe(&self, _pcm: Vec<u8>) -> Result<String> {
        self.data.lock().unwrap().transcript.clone().ok_or_else(|| anyhow!("fixture {}: no transcript", self.name))
    }
    fn speak(&self, text: &str) -> Result<(u32, Box<dyn std::io::Read + Send>)> {
        // Silence, 10 ms per character: enough for a scripted playback.
        Ok((24_000, Box::new(std::io::Cursor::new(vec![0u8; text.chars().count() * 480]))))
    }
}

/// The daemon's approval rule, as `sbxw_core::jarvis` enforces it: a voice
/// approval only of a low-risk proposal, a host command only acknowledged,
/// only a pending one. Used by the fixture daemon and the stand-in.
pub fn fixture_rule(p: &serde_json::Value, via: &str, ack: bool) -> Result<()> {
    let refuse = |m: &str| Err(anyhow!(HttpError { path: "approve".into(), code: 409, message: m.to_string() }));
    if p["state"] != "pending" {
        return refuse(&format!("proposal {} is already {}", p["id"].as_str().unwrap_or(""), p["state"].as_str().unwrap_or("")));
    }
    if via == "voice" && (p["risk"] != "low" || p["needs_ack"] == true) {
        return refuse("a high-risk proposal is approved on screen only, not by voice");
    }
    if p["needs_ack"] == true && !ack {
        return refuse("this proposal runs a command on this machine: tick that you read it first");
    }
    Ok(())
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
    /// A feature's call answered (`Link::call`): status and JSON body, or why there was no answer.
    Answer { id: u64, feature: &'static str, call: crate::feat::Call, result: Result<(u16, serde_json::Value), String> },
    /// A feature's data was read (`Link::fetch`).
    Data { key: String, result: Result<serde_json::Value, String> },
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

impl Link {
    fn job(&self, f: impl FnOnce() -> LinkEvent + Send + 'static) {
        use std::sync::atomic::Ordering;
        self.pending.fetch_add(1, Ordering::SeqCst);
        let send = Sender { tx: self.tx.clone(), wake: self.wake.clone() };
        let pending = self.pending.clone();
        std::thread::spawn(move || {
            send.send(f());
            pending.fetch_sub(1, Ordering::SeqCst);
            (send.wake)();
        });
    }

    /// Runs a feature's call off the UI thread; answers `Answer` with `id`.
    pub fn call(&self, daemon: Arc<dyn Daemon>, id: u64, feature: &'static str, call: crate::feat::Call) {
        self.job(move || {
            let method = match call.method {
                crate::feat::Method::Get => "GET",
                crate::feat::Method::Post => "POST",
            };
            let result = daemon.request(method, &call.path, &call.body, Duration::from_secs(call.timeout.max(1))).map_err(|e| format!("{e:#}"));
            LinkEvent::Answer { id, feature, call, result }
        });
    }

    /// Reads data key `key`: one path, or one per sandbox (`(sandbox, path)`),
    /// gathered in an object by sandbox name (a sandbox whose read failed is left out).
    pub fn fetch(&self, daemon: Arc<dyn Daemon>, key: String, paths: Vec<(Option<String>, String)>) {
        self.job(move || {
            let get = |p: &str| match daemon.request("GET", p, &serde_json::Value::Null, Duration::from_secs(30)) {
                Ok((c, v)) if (200..300).contains(&c) => Ok(v),
                Ok((c, v)) => Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"))),
                Err(e) => Err(format!("{e:#}")),
            };
            let result = match paths.as_slice() {
                [(None, p)] => get(p),
                _ => {
                    let mut o = serde_json::Map::new();
                    for (sb, p) in &paths {
                        match get(p) {
                            Ok(v) => {
                                o.insert(sb.clone().unwrap_or_default(), v);
                            }
                            Err(e) => log::warn!("{key}: {e}"),
                        }
                    }
                    Ok(serde_json::Value::Object(o))
                }
            };
            LinkEvent::Data { key, result }
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
