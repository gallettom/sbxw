//! Every call to the sbxw daemon goes through this module: the [`Daemon`]
//! trait, and [`HttpDaemon`], its implementation over the daemon's current
//! HTTP and WebSocket API on loopback. The rest of the app sees only the
//! trait, so the transport can change without touching a screen.
//!
//! What it speaks (daemon side, read from the code, not changed):
//! * `GET /api/sandboxes`: a JSON array of sandboxes (`crates/daemon/src/http/ui.rs:55,279`,
//!   items are `SandboxItem` in `crates/daemon/src/svc/sandboxes.rs:30`).
//! * `GET /ws?sandbox=&mode=&thread=`: the terminal (`crates/daemon/src/http/ws.rs`).
//!   Binary frames both ways are PTY bytes; text frames from the client are JSON
//!   `{"type":"resize","cols","rows"}`; text frames from the daemon are an error
//!   line to show, or `{"type":"attention"}` (the bell).
//! * Auth (`crates/daemon/src/http/guard.rs`, `token.rs`): native clients send
//!   no `Origin` and `Authorization: Bearer <ui token>`, after checking
//!   `/api/health?nonce=` answers `sha256(token || nonce)`, so a process
//!   squatting the port never sees the token.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Sandbox {
    pub name: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub workspace: Option<String>,
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

/// Called from the socket thread when an event is queued (the UI's waker).
pub type Wake = std::sync::Arc<dyn Fn() + Send + Sync>;

pub trait Daemon: Send + Sync + 'static {
    fn sandboxes(&self) -> Result<Vec<Sandbox>>;
    fn open_terminal(&self, req: &TerminalRequest, wake: Wake) -> Result<TerminalConn>;
    /// Where it talks to, for the window's status line.
    fn describe(&self) -> String;
}

pub struct HttpDaemon {
    addr: String,
    token: String,
}

impl HttpDaemon {
    /// Connects and checks the daemon proves it knows `token` before any
    /// request carries it.
    pub fn connect(addr: &str, token: &str) -> Result<Self> {
        let nonce = random_hex(16);
        #[derive(Deserialize)]
        struct Health {
            proof: Option<String>,
        }
        let h: Health = ureq::get(&format!("http://{addr}/api/health?nonce={nonce}"))
            .timeout(Duration::from_secs(3))
            .call()
            .with_context(|| format!("no sbxw daemon at {addr}"))?
            .into_json()?;
        if h.proof.as_deref() != Some(proof(token, &nonce).as_str()) {
            bail!("the process at {addr} does not know this token: refusing to send it");
        }
        Ok(Self { addr: addr.to_string(), token: token.to_string() })
    }
}

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

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

impl Daemon for HttpDaemon {
    fn sandboxes(&self) -> Result<Vec<Sandbox>> {
        Ok(ureq::get(&format!("http://{}/api/sandboxes", self.addr))
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(10))
            .call()?
            .into_json()?)
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
        request.headers_mut().insert("Authorization", format!("Bearer {}", self.token).parse()?);
        let stream = TcpStream::connect(&self.addr)?;
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

    fn describe(&self) -> String {
        format!("daemon at {}", self.addr)
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

/// Fixture data for a screen when no daemon is reachable.
pub fn fixture_sandboxes() -> Vec<Sandbox> {
    [("web-app", "claude", "running"), ("data-pipeline", "claude", "stopped"), ("docs", "codex", "running")]
        .into_iter()
        .map(|(n, a, s)| Sandbox { name: n.into(), agent: a.into(), status: s.into(), workspace: None })
        .collect()
}
