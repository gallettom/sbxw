//! Long-lived feature WebSockets through the app's link: a feature opens a
//! `/ws/...` route of the daemon with the app's authentication
//! ([`crate::feat::Action::OpenSocket`]), hears what arrives on a thread of
//! its own ([`crate::feat::Feature::on_socket`], never the UI thread, never
//! Luau), and sends with [`send`] from anywhere (its `act`, its surface
//! input, its socket handler).
//!
//! * One I/O thread per socket waits on the socket and on what is queued to
//!   send at once (`mio`, as the terminal's pump): no polling delay.
//! * A second thread runs the feature's handler. Between the two, a queue
//!   that keeps only the NEWEST of the messages the feature called
//!   droppable ([`Opts::droppable`], a picture frame) when the handler is
//!   behind; every other message is kept, in order.
//! * Incoming messages larger than [`Opts::max_in`] close the socket (with
//!   the reason); outgoing ones are checked by [`send`] ([`Opts::max_out`],
//!   no newline when [`Opts::one_line`]).
//! * Reconnect is an option ([`Opts::reconnect`], off by default): on, a
//!   closed socket is opened again with the daemon the link has NOW (a
//!   restarted daemon has a new token), 0.5 s then doubling up to 5 s, and
//!   the handler hears `Reconnecting`, then `Open` again.
//! * The actions a handler returns are run by the UI thread at its next
//!   poll ([`take_actions`]), woken by the link's waker.

use crate::client::Daemon;
use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tungstenite::Message;

/// The daemon the link has now (the app's `current`).
pub type Current = Arc<Mutex<Option<Arc<dyn Daemon>>>>;

/// How a socket behaves.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// Open it again when it closes (not when the feature closed it).
    pub reconnect: bool,
    /// A text message the handler may skip when a newer droppable one waits
    /// (a frame). Must be cheap: it runs on the I/O thread for each message.
    pub droppable: Option<fn(&str) -> bool>,
    /// Largest message taken, bytes (default 8 MB).
    pub max_in: usize,
    /// Largest message [`send`] takes, bytes (default 256 KB).
    pub max_out: usize,
    /// [`send`] refuses a message with a newline (one JSON object per line).
    pub one_line: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { reconnect: false, droppable: None, max_in: 8 << 20, max_out: 256 << 10, one_line: false }
    }
}

impl PartialEq for Opts {
    fn eq(&self, o: &Opts) -> bool {
        // Functions are told apart by whether there is one.
        (self.reconnect, self.droppable.is_some(), self.max_in, self.max_out, self.one_line) == (o.reconnect, o.droppable.is_some(), o.max_in, o.max_out, o.one_line)
    }
}

/// What a socket's handler hears.
#[derive(Debug, Clone, PartialEq)]
pub enum SocketEvent {
    /// Open (again, after `Reconnecting`).
    Open,
    Text(String),
    Binary(Vec<u8>),
    /// Lost; it will be opened again (only with [`Opts::reconnect`]).
    Reconnecting(String),
    /// Closed for good: why. The last event of a socket.
    Closed(String),
}

enum Out {
    Text(String),
    Close,
}

struct Handle {
    out: Arc<Mutex<VecDeque<Out>>>,
    waker: Arc<Mutex<Option<Arc<mio::Waker>>>>,
    opts: Opts,
    /// Which open this is (a reopen under the same id replaces it).
    gen: u64,
}

static SOCKETS: Mutex<Option<HashMap<String, Handle>>> = Mutex::new(None);
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Opens socket `id` on `path` (`/ws/browser?sandbox=web-app`); a socket
/// already open under `id` is closed first. `handler` runs on the socket's
/// own thread for every event, `Closed` last.
pub fn open(current: Current, id: &str, path: &str, opts: Opts, handler: Box<dyn FnMut(SocketEvent) + Send>) {
    close(id);
    let gen = GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = Arc::new(Mutex::new(VecDeque::new()));
    let waker = Arc::new(Mutex::new(None));
    SOCKETS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.to_string(), Handle { out: out.clone(), waker: waker.clone(), opts, gen });
    let inbox = Arc::new(Inbox::default());
    let (id2, path) = (id.to_string(), path.to_string());
    {
        let inbox = inbox.clone();
        std::thread::Builder::new().name(format!("socket-{id}")).spawn(move || io(current, &id2, gen, &path, opts, out, waker, &inbox)).expect("socket thread");
    }
    std::thread::Builder::new().name(format!("socket-{id}-handler")).spawn(move || inbox.run(handler)).expect("socket handler thread");
}

/// Sends a text message on socket `id`. Refused: no such socket, too long,
/// a newline when the socket is one-line. Queued while it reconnects.
pub fn send(id: &str, text: impl Into<String>) -> Result<(), String> {
    let text = text.into();
    let g = SOCKETS.lock().unwrap();
    let h = g.as_ref().and_then(|m| m.get(id)).ok_or_else(|| format!("no socket {id} open"))?;
    if text.len() > h.opts.max_out {
        return Err(format!("{id}: a message of {} bytes is more than {}", text.len(), h.opts.max_out));
    }
    if h.opts.one_line && text.contains(['\n', '\r']) {
        return Err(format!("{id}: a message must be one line"));
    }
    h.out.lock().unwrap().push_back(Out::Text(text));
    if let Some(w) = h.waker.lock().unwrap().as_ref() {
        let _ = w.wake();
    }
    Ok(())
}

/// Closes socket `id` (its handler hears `Closed("closed")`). Nothing if none.
pub fn close(id: &str) {
    let h = SOCKETS.lock().unwrap().as_mut().and_then(|m| m.remove(id));
    if let Some(h) = h {
        h.out.lock().unwrap().push_back(Out::Close);
        if let Some(w) = h.waker.lock().unwrap().as_ref() {
            let _ = w.wake();
        }
    }
}

/// Whether socket `id` is open (or reconnecting).
pub fn is_open(id: &str) -> bool {
    SOCKETS.lock().unwrap().as_ref().is_some_and(|m| m.contains_key(id))
}

fn closed_by_app(id: &str, gen: u64) -> bool {
    !SOCKETS.lock().unwrap().as_ref().and_then(|m| m.get(id)).is_some_and(|h| h.gen == gen)
}

fn forget(id: &str, gen: u64) {
    let mut g = SOCKETS.lock().unwrap();
    if let Some(m) = g.as_mut() {
        if m.get(id).is_some_and(|h| h.gen == gen) {
            m.remove(id);
        }
    }
}

// ---------------------------------------------------------------------------
// The queue between the I/O thread and the handler

#[derive(Default)]
struct Inbox {
    q: Mutex<(VecDeque<(SocketEvent, bool)>, bool)>,
    cv: Condvar,
}

impl Inbox {
    /// Queues `ev`; a droppable one replaces droppable ones still waiting.
    fn push(&self, ev: SocketEvent, droppable: bool) {
        let mut g = self.q.lock().unwrap();
        if droppable {
            g.0.retain(|(_, d)| !d);
        }
        let last = matches!(ev, SocketEvent::Closed(_));
        g.0.push_back((ev, droppable));
        g.1 |= last;
        self.cv.notify_one();
    }

    fn run(&self, mut handler: Box<dyn FnMut(SocketEvent) + Send>) {
        loop {
            let ev = {
                let mut g = self.q.lock().unwrap();
                while g.0.is_empty() {
                    g = self.cv.wait(g).unwrap();
                }
                g.0.pop_front().unwrap().0
            };
            let last = matches!(ev, SocketEvent::Closed(_));
            handler(ev);
            if last {
                return;
            }
        }
    }

    #[cfg(test)]
    fn waiting(&self) -> Vec<SocketEvent> {
        self.q.lock().unwrap().0.iter().map(|(e, _)| e.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// The I/O thread

const SOCKET: mio::Token = mio::Token(0);
const OUT: mio::Token = mio::Token(1);

#[allow(clippy::too_many_arguments)]
fn io(current: Current, id: &str, gen: u64, path: &str, opts: Opts, out: Arc<Mutex<VecDeque<Out>>>, waker: Arc<Mutex<Option<Arc<mio::Waker>>>>, inbox: &Inbox) {
    let mut backoff = Duration::from_millis(500);
    let mut opened_once = false;
    loop {
        let daemon = current.lock().unwrap().clone();
        let why = match daemon.ok_or_else(|| anyhow::anyhow!("not connected to the daemon")).and_then(|d| d.open_socket(path)) {
            Ok(ws) => {
                opened_once = true;
                backoff = Duration::from_millis(500);
                log::info!("socket {id}: open ({path})");
                inbox.push(SocketEvent::Open, false);
                match pump(ws, id, gen, opts, &out, &waker, inbox) {
                    Ok(why) => why,
                    Err(e) => e.to_string(),
                }
            }
            Err(e) => format!("{e:#}"),
        };
        *waker.lock().unwrap() = None;
        if closed_by_app(id, gen) {
            inbox.push(SocketEvent::Closed("closed".into()), false);
            return;
        }
        if !opts.reconnect {
            log::info!("socket {id}: {why}");
            forget(id, gen);
            inbox.push(SocketEvent::Closed(why), false);
            return;
        }
        log::info!("socket {id}: {why}; again in {backoff:?}");
        inbox.push(SocketEvent::Reconnecting(why), false);
        let _ = opened_once;
        // Wait, but wake for a close asked meanwhile.
        let until = std::time::Instant::now() + backoff;
        while std::time::Instant::now() < until {
            if closed_by_app(id, gen) {
                inbox.push(SocketEvent::Closed("closed".into()), false);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted))
}

/// One connection: until it closes; the reason.
fn pump(mut ws: tungstenite::WebSocket<TcpStream>, id: &str, gen: u64, opts: Opts, out: &Mutex<VecDeque<Out>>, waker: &Mutex<Option<Arc<mio::Waker>>>, inbox: &Inbox) -> std::io::Result<String> {
    let mut poll = mio::Poll::new()?;
    *waker.lock().unwrap() = Some(Arc::new(mio::Waker::new(poll.registry(), OUT)?));
    ws.get_ref().set_nonblocking(true)?;
    let mut source = mio::net::TcpStream::from_std(ws.get_ref().try_clone()?);
    poll.registry().register(&mut source, SOCKET, mio::Interest::READABLE | mio::Interest::WRITABLE)?;
    let mut events = mio::Events::with_capacity(8);
    loop {
        loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    if t.len() > opts.max_in {
                        let _ = ws.close(None);
                        return Ok(format!("a message of {} bytes is more than {}", t.len(), opts.max_in));
                    }
                    let drop = opts.droppable.is_some_and(|f| f(&t));
                    inbox.push(SocketEvent::Text(t.to_string()), drop);
                }
                Ok(Message::Binary(b)) => {
                    if b.len() > opts.max_in {
                        let _ = ws.close(None);
                        return Ok(format!("a message of {} bytes is more than {}", b.len(), opts.max_in));
                    }
                    inbox.push(SocketEvent::Binary(b.to_vec()), false);
                }
                Ok(Message::Close(f)) => return Ok(f.map(|f| f.reason.to_string()).filter(|r| !r.is_empty()).unwrap_or_else(|| "closed by the daemon".into())),
                Ok(_) => {}
                Err(e) if would_block(&e) => break,
                Err(tungstenite::Error::ConnectionClosed) => return Ok("closed by the daemon".into()),
                Err(e) => return Ok(e.to_string()),
            }
        }
        loop {
            let next = out.lock().unwrap().pop_front();
            match next {
                Some(Out::Text(t)) => {
                    if let Err(e) = ws.send(Message::Text(t.into())) {
                        if !would_block(&e) {
                            return Ok(e.to_string());
                        }
                    }
                }
                Some(Out::Close) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return Ok("closed".into());
                }
                None => break,
            }
        }
        if closed_by_app(id, gen) {
            let _ = ws.close(None);
            let _ = ws.flush();
            return Ok("closed".into());
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
// Actions handed to the UI thread

type Outbox = Vec<(&'static str, Vec<crate::feat::Action>)>;
static OUTBOX: Mutex<Outbox> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<crate::client::Wake>> = Mutex::new(None);

/// The UI's waker (the app sets it once it has one).
pub fn set_wake(w: crate::client::Wake) {
    *WAKE.lock().unwrap() = Some(w);
}

/// A feature's actions, from its socket thread: the UI runs them next poll.
pub fn post(feature: &'static str, acts: Vec<crate::feat::Action>) {
    if acts.is_empty() {
        return;
    }
    OUTBOX.lock().unwrap().push((feature, acts));
    if let Some(w) = WAKE.lock().unwrap().clone() {
        w();
    }
}

/// What features' socket threads asked, in order (the UI thread runs it).
pub fn take_actions() -> Outbox {
    std::mem::take(&mut *OUTBOX.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_handler_gets_only_the_newest_frame_and_every_other_message() {
        let q = Inbox::default();
        q.push(SocketEvent::Text("a".into()), false);
        q.push(SocketEvent::Text("frame 1".into()), true);
        q.push(SocketEvent::Text("b".into()), false);
        q.push(SocketEvent::Text("frame 2".into()), true);
        q.push(SocketEvent::Text("frame 3".into()), true);
        q.push(SocketEvent::Text("c".into()), false);
        let t = |s: &str| SocketEvent::Text(s.into());
        assert_eq!(q.waiting(), vec![t("a"), t("b"), t("frame 3"), t("c")]);
    }

    #[test]
    fn sends_are_checked() {
        assert!(send("nope", "x").is_err());
        let opts = Opts { one_line: true, max_out: 10, ..Opts::default() };
        // Not connected: it opens, stays queued, and is checked all the same.
        let current: Current = Arc::new(Mutex::new(None));
        let (tx, rx) = std::sync::mpsc::channel();
        open(current, "t-send", "/ws/none", opts, Box::new(move |e| {
            let _ = tx.send(e);
        }));
        assert!(send("t-send", "two\nlines").is_err());
        assert!(send("t-send", "much too long").is_err());
        // Without reconnect, a socket that could not open is closed with the reason.
        let ev = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(&ev, SocketEvent::Closed(why) if why.contains("not connected")), "{ev:?}");
        assert!(!is_open("t-send"));
    }
}
