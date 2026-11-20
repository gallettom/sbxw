//! One persistent terminal session: a PTY running a command, its output
//! broadcast to every WebSocket attached, and the last 256 KB kept for replay.
//!
//! A session outlives its WebSockets (a reload replays and resumes), and its
//! *process* only until the reader sees EOF, when it calls `on_exit`, which the
//! registry uses to forget it. "There is a session" and "there is something to
//! type into" differ, so `alive` asks the process.
//!
//! # One size for every client
//!
//! A PTY has one size, and every client attached to it (web panes, desktop
//! panes) draws the program's output in a grid of that size or garbles it. So
//! each client says the size it *wants* ([`PtySession::want`]) and the session
//! applies ONE: the owner's. The owner is the client used last: the one that
//! last sent a size (it attached, or its own window changed) or typed
//! ([`PtySession::touch`]). A client that leaves while owning gives the size
//! back to the most recently used one still attached, but only after
//! [`HANDBACK_GRACE`]: a web page reloading detaches and attaches again
//! within that time and takes the size back without the PTY bouncing to
//! another client's size and back. Every change is published on
//! [`PtySession::sizes`] (size plus owner), which the WebSocket turns into a
//! `{"type":"size"}` frame for the clients that asked for them.

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use sbxw_core::terminal::REPLAY_BYTES;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

/// How long an owner that left keeps the size before it goes to another
/// client (a reload comes back sooner).
pub const HANDBACK_GRACE: Duration = Duration::from_secs(2);

/// The effective size and which client owns it (`None`: nobody, the size
/// stays where it was).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectiveSize {
    pub cols: u16,
    pub rows: u16,
    pub owner: Option<u64>,
}

struct Client {
    id: u64,
    want: Option<(u16, u16)>,
    /// When it was last used (the session's own counter).
    used: u64,
}

#[derive(Default)]
struct Sizing {
    clients: Vec<Client>,
    owner: Option<u64>,
    next_id: u64,
    seq: u64,
}

/// Tracks real bells across chunks: a BEL that ends an OSC string (a title,
/// a hyperlink, a clipboard write: `ESC ] ... BEL`) or sits inside a DCS,
/// SOS, PM or APC string is a terminator, not a bell. Programs that retitle
/// their window while they work (Claude Code does) would otherwise ring one
/// "attention" after another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BellScan {
    #[default]
    Ground,
    Esc,
    Str,
    StrEsc,
}

impl BellScan {
    /// Advances over `bytes`; true when a real BEL was seen.
    pub(crate) fn scan(&mut self, bytes: &[u8]) -> bool {
        let mut rang = false;
        for &b in bytes {
            *self = match (*self, b) {
                (BellScan::Ground, 0x07) => {
                    rang = true;
                    BellScan::Ground
                }
                (BellScan::Ground, 0x1b) => BellScan::Esc,
                (BellScan::Ground, _) => BellScan::Ground,
                (BellScan::Esc, b']' | b'P' | b'X' | b'^' | b'_') => BellScan::Str,
                (BellScan::Esc, 0x1b) => BellScan::Esc,
                (BellScan::Esc, 0x07) => {
                    rang = true;
                    BellScan::Ground
                }
                (BellScan::Esc, _) => BellScan::Ground,
                (BellScan::Str, 0x07) => BellScan::Ground,
                (BellScan::Str, 0x1b) => BellScan::StrEsc,
                (BellScan::Str, _) => BellScan::Str,
                (BellScan::StrEsc, b'\\') => BellScan::Ground,
                (BellScan::StrEsc, 0x1b) => BellScan::StrEsc,
                // ESC inside a string cancels it; what follows is a new escape.
                (BellScan::StrEsc, b']' | b'P' | b'X' | b'^' | b'_') => BellScan::Str,
                (BellScan::StrEsc, _) => BellScan::Ground,
            };
        }
        rang
    }
}

/// Is this input a person using the terminal (keys, paste, mouse), as
/// opposed to the terminal answering the program (cursor position, device
/// attributes, mode reports, colours, focus)? Every attached client answers
/// the program's queries, so answers must not move the size around.
pub fn is_typed_input(data: &[u8]) -> bool {
    match data {
        [] => false,
        // OSC / DCS answers (colours, XTGETTCAP).
        [0x1b, b']' | b'P', ..] => false,
        [0x1b, b'[', rest @ ..] => {
            // Focus in and out: the window, not the person typing.
            if rest == b"I" || rest == b"O" {
                return false;
            }
            // A report: CSI [?>=] params final, where final is one of the
            // answers' (R: cursor position, c: device attributes, n: status,
            // t: window report, y: mode report). Keys end in A-D, H, F, ~, u,
            // P-S with modifiers (`1;5P`): `R` is F3 only with a modifier,
            // and then it carries `1;`, which a cursor report (`row;col`)
            // also carries. Taken as a report: one F3 lost as a claim.
            let last = rest.last().copied().unwrap_or(0);
            let params = &rest[..rest.len().saturating_sub(1)];
            let params_ok = params.iter().all(|c| c.is_ascii_digit() || b";?>=$ ".contains(c));
            !(params_ok && matches!(last, b'R' | b'c' | b'n' | b't' | b'y'))
        }
        _ => true,
    }
}

pub struct PtySession {
    tx: broadcast::Sender<Vec<u8>>,
    bell: broadcast::Sender<()>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    replay: Mutex<VecDeque<u8>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    sizing: Mutex<Sizing>,
    sizes: watch::Sender<EffectiveSize>,
    /// Every size applied to the PTY, in order (tests check for bounces).
    #[cfg(test)]
    applied: Mutex<Vec<(u16, u16)>>,
    pub started_ms: u64,
    /// Held by every writer that types a whole line into this session (the
    /// composers' chat push, sbxw's own lines, an answer, the phone's message
    /// and Interrupt), so two never interleave: one's Return cannot submit
    /// another's half-typed text. Raw terminal input does not take it.
    pub typing: tokio::sync::Mutex<()>,
}

/// What to run, and where.
pub struct Spawn {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Spawn {
    pub fn sbx(args: Vec<String>) -> Self {
        Self {
            program: "sbx".into(),
            args,
            env: vec![],
        }
    }
}

impl PtySession {
    /// Open a PTY and run `spawn` in it. `on_exit` runs on the reader thread
    /// once the process's output ends.
    pub fn spawn(spawn: Spawn, now_ms: u64, on_exit: impl FnOnce() + Send + 'static) -> Result<Arc<Self>> {
        let pair = native_pty_system().openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(&spawn.program);
        cmd.args(&spawn.args);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in &spawn.env {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, _) = broadcast::channel(256);
        let (bell, _) = broadcast::channel(16);
        let (sizes, _) = watch::channel(EffectiveSize { cols: 100, rows: 30, owner: None });
        let session = Arc::new(Self {
            tx,
            bell,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            replay: Mutex::new(VecDeque::with_capacity(REPLAY_BYTES)),
            child: Mutex::new(child),
            sizing: Mutex::new(Sizing::default()),
            sizes,
            #[cfg(test)]
            applied: Mutex::new(Vec::new()),
            started_ms: now_ms,
            typing: tokio::sync::Mutex::new(()),
        });
        let me = Arc::downgrade(&session);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // Agents ring several BELs for one prompt; one notification per burst.
            let mut last_bell = Instant::now() - Duration::from_secs(3);
            let mut bells = BellScan::default();
            loop {
                let n = match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let Some(s) = me.upgrade() else { break };
                let chunk = buf[..n].to_vec();
                if bells.scan(&chunk) && last_bell.elapsed() >= Duration::from_secs(3) {
                    last_bell = Instant::now();
                    let _ = s.bell.send(());
                }
                {
                    let mut r = s.replay.lock().unwrap();
                    let overflow = (r.len() + chunk.len()).saturating_sub(REPLAY_BYTES);
                    let cut = overflow.min(r.len());
                    r.drain(..cut);
                    r.extend(chunk.iter().skip(chunk.len().saturating_sub(REPLAY_BYTES)));
                }
                let _ = s.tx.send(chunk);
            }
            on_exit();
        });
        Ok(session)
    }

    /// Is the process still running? Unreadable status counts as alive: the
    /// pessimistic answer costs a check, never a lost message.
    pub fn alive(&self) -> bool {
        self.child.lock().unwrap().try_wait().map(|s| s.is_none()).unwrap_or(true)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.tx.subscribe()
    }

    pub fn subscribe_bell(&self) -> broadcast::Receiver<()> {
        self.bell.subscribe()
    }

    /// The replay buffer as it stands.
    pub fn replay(&self) -> Vec<u8> {
        self.replay.lock().unwrap().iter().copied().collect()
    }

    /// Type `parts` in order. Off the async runtime: a PTY whose reader is
    /// stalled can block a write.
    pub async fn write(self: &Arc<Self>, parts: &[&[u8]]) {
        let data: Vec<u8> = parts.concat();
        let me = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut w = me.writer.lock().unwrap();
            let _ = w.write_all(&data);
            let _ = w.flush();
        })
        .await;
    }

    /// Sets the PTY's size (the program gets SIGWINCH when it changed).
    /// Returns the size before. Clients go through [`Self::want`].
    fn resize(&self, cols: u16, rows: u16) -> Result<Option<(u16, u16)>> {
        let m = self.master.lock().unwrap();
        let before = m.get_size().ok().map(|s| (s.cols, s.rows));
        if before != Some((cols, rows)) {
            m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
            #[cfg(test)]
            self.applied.lock().unwrap().push((cols, rows));
        }
        Ok(before)
    }

    // ---- one size, many clients ----

    /// A client attached: its id for [`Self::want`], [`Self::touch`] and
    /// [`Self::detach`]. It owns nothing until it says a size.
    pub fn attach(&self) -> u64 {
        let mut s = self.sizing.lock().unwrap();
        s.next_id += 1;
        s.seq += 1;
        let (id, used) = (s.next_id, s.seq);
        s.clients.push(Client { id, want: None, used });
        id
    }

    /// The client wants `cols` x `rows` (it attached, or its window
    /// changed): it becomes the owner and the PTY takes that size.
    pub fn want(&self, id: u64, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let mut s = self.sizing.lock().unwrap();
        s.seq += 1;
        let seq = s.seq;
        let Some(c) = s.clients.iter_mut().find(|c| c.id == id) else { return };
        c.want = Some((cols, rows));
        c.used = seq;
        s.owner = Some(id);
        self.apply(&s);
    }

    /// The client typed: it becomes the owner if it has said a size.
    pub fn touch(&self, id: u64) {
        let mut s = self.sizing.lock().unwrap();
        s.seq += 1;
        let seq = s.seq;
        let Some(c) = s.clients.iter_mut().find(|c| c.id == id) else { return };
        c.used = seq;
        if c.want.is_some() && s.owner != Some(id) {
            s.owner = Some(id);
            self.apply(&s);
        }
    }

    /// The client left. True when it owned the size: call [`Self::hand_back`]
    /// after [`HANDBACK_GRACE`].
    pub fn detach(&self, id: u64) -> bool {
        let mut s = self.sizing.lock().unwrap();
        s.clients.retain(|c| c.id != id);
        if s.owner == Some(id) {
            s.owner = None;
            self.publish(&s);
            return true;
        }
        false
    }

    /// Nobody claimed the size since its owner left: it goes to the most
    /// recently used client that has said one.
    pub fn hand_back(&self) {
        let mut s = self.sizing.lock().unwrap();
        if s.owner.is_some() {
            return;
        }
        s.owner = s.clients.iter().filter(|c| c.want.is_some()).max_by_key(|c| c.used).map(|c| c.id);
        if s.owner.is_some() {
            self.apply(&s);
        }
    }

    /// The owner's size to the PTY, then to every client.
    fn apply(&self, s: &Sizing) {
        if let Some((cols, rows)) = s.owner.and_then(|o| s.clients.iter().find(|c| c.id == o)).and_then(|c| c.want) {
            match self.resize(cols, rows) {
                Ok(before) if before != Some((cols, rows)) => tracing::info!(
                    "pty resize: {} -> {cols}x{rows} (client {})",
                    before.map_or("?".into(), |(c, r)| format!("{c}x{r}")),
                    s.owner.unwrap_or(0)
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("pty resize to {cols}x{rows} failed: {e}"),
            }
        }
        self.publish(s);
    }

    fn publish(&self, s: &Sizing) {
        let (cols, rows) = self.size().unwrap_or((100, 30));
        let now = EffectiveSize { cols, rows, owner: s.owner };
        self.sizes.send_if_modified(|v| {
            let changed = *v != now;
            *v = now;
            changed
        });
    }

    /// The effective size and its owner, now and on every change.
    pub fn sizes(&self) -> watch::Receiver<EffectiveSize> {
        self.sizes.subscribe()
    }

    pub fn size(&self) -> Option<(u16, u16)> {
        self.master.lock().unwrap().get_size().ok().map(|s| (s.cols, s.rows))
    }

    pub fn kill(&self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_is_broadcast_replayed_and_the_end_is_reported() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let s = PtySession::spawn(
            Spawn {
                program: "sh".into(),
                args: vec!["-c".into(), "read x; echo got:$x".into()],
                env: vec![],
            },
            0,
            move || {
                let _ = done_tx.send(());
            },
        )
        .unwrap();
        let mut rx = s.subscribe();
        assert!(s.alive());
        s.write(&[b"hello\n"]).await;
        let mut seen = Vec::new();
        while let Ok(Ok(chunk)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            seen.extend(chunk);
            if String::from_utf8_lossy(&seen).contains("got:hello") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(&seen).contains("got:hello"));
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(String::from_utf8_lossy(&s.replay()).contains("got:hello"));
    }

    fn idle() -> Arc<PtySession> {
        PtySession::spawn(
            Spawn {
                program: "sh".into(),
                args: vec!["-c".into(), "sleep 30".into()],
                env: vec![],
            },
            0,
            || {},
        )
        .unwrap()
    }

    #[tokio::test]
    async fn the_client_used_last_sets_the_size_and_both_hear_it() {
        let s = idle();
        let (a, b) = (s.attach(), s.attach());
        let (mut ra, mut rb) = (s.sizes(), s.sizes());
        s.want(a, 80, 24);
        assert_eq!(s.size(), Some((80, 24)));
        s.want(b, 120, 40);
        assert_eq!(s.size(), Some((120, 40)));
        for r in [&mut ra, &mut rb] {
            assert!(r.has_changed().unwrap());
            assert_eq!(*r.borrow_and_update(), EffectiveSize { cols: 120, rows: 40, owner: Some(b) });
        }
        // a types: its size again, and both are told.
        s.touch(a);
        assert_eq!(s.size(), Some((80, 24)));
        assert_eq!(*ra.borrow_and_update(), EffectiveSize { cols: 80, rows: 24, owner: Some(a) });
        assert_eq!(rb.borrow_and_update().owner, Some(a));
        // Typing again changes nothing.
        s.touch(a);
        assert!(!ra.has_changed().unwrap());
        assert_eq!(*s.applied.lock().unwrap(), [(80, 24), (120, 40), (80, 24)]);
    }

    #[tokio::test]
    async fn a_client_that_never_said_a_size_takes_nothing_by_typing() {
        let s = idle();
        let (a, b) = (s.attach(), s.attach());
        s.want(a, 80, 24);
        s.touch(b);
        assert_eq!(s.size(), Some((80, 24)));
        assert_eq!(*s.sizes().borrow(), EffectiveSize { cols: 80, rows: 24, owner: Some(a) });
    }

    #[tokio::test]
    async fn the_owner_leaving_hands_the_size_back_to_the_one_left() {
        let s = idle();
        let (a, b) = (s.attach(), s.attach());
        s.want(a, 80, 24);
        s.want(b, 120, 40);
        assert!(!s.detach(a), "a did not own the size");
        let c = s.attach();
        s.want(c, 90, 20);
        s.touch(b);
        assert!(s.detach(b));
        assert_eq!(s.sizes().borrow().owner, None, "nobody owns it during the grace");
        assert_eq!(s.size(), Some((120, 40)), "the size stays during the grace");
        s.hand_back();
        assert_eq!(s.size(), Some((90, 20)));
        assert_eq!(s.sizes().borrow().owner, Some(c));
    }

    #[tokio::test]
    async fn a_reload_within_the_grace_does_not_bounce_the_size() {
        let s = idle();
        let (desktop, web) = (s.attach(), s.attach());
        s.want(desktop, 150, 38);
        s.want(web, 120, 40);
        // The web page reloads: its socket closes, a new one opens and says
        // its size before the grace ends.
        assert!(s.detach(web));
        let web2 = s.attach();
        s.want(web2, 120, 40);
        s.hand_back();
        assert_eq!(s.size(), Some((120, 40)));
        assert_eq!(*s.applied.lock().unwrap(), [(150, 38), (120, 40)], "never back to the desktop's size");
        assert_eq!(s.sizes().borrow().owner, Some(web2));
        // The desktop's window changes: it owns the size again.
        s.want(desktop, 150, 38);
        assert_eq!(s.size(), Some((150, 38)));
    }

    #[test]
    fn the_size_frame_says_who_owns_it() {
        let e = EffectiveSize { cols: 100, rows: 30, owner: Some(2) };
        let f = |me| serde_json::from_str::<serde_json::Value>(&crate::http::ws::size_frame(e, me)).unwrap();
        assert_eq!(f(2), serde_json::json!({ "type": "size", "cols": 100, "rows": 30, "owner": true }));
        assert_eq!(f(1)["owner"], false);
    }

    #[test]
    fn a_bel_that_ends_a_title_is_not_a_bell() {
        let mut b = BellScan::default();
        assert!(!b.scan(b"\x1b]0;\xe2\x9c\xb3 Working\x07text"));
        assert!(!b.scan(b"\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\"));
        assert!(b.scan(b"done\x07"));
        // Split across chunks.
        assert!(!b.scan(b"\x1b]2;tit"));
        assert!(!b.scan(b"le\x07"));
        assert!(b.scan(b"\x07"));
        // Inside a DCS string.
        assert!(!b.scan(b"\x1bPq\x07"));
        assert!(b.scan(b"\x1b[31m\x07"));
    }

    #[test]
    fn answers_to_the_program_are_not_typing() {
        for answer in [&b"\x1b[12;40R"[..], b"\x1b[?1;2c", b"\x1b[>0;276;0c", b"\x1b[0n", b"\x1b[?2004;1$y", b"\x1b[8;30;100t", b"\x1b]11;rgb:0d0d/1111/1717\x1b\\", b"\x1bP1+r\x1b\\", b"\x1b[I", b"\x1b[O", b""] {
            assert!(!is_typed_input(answer), "{answer:?}");
        }
        for key in [&b"a"[..], b"\r", b"\x03", b"\x1b[A", b"\x1b[1;5D", b"\x1b[3~", b"\x1b[200~paste\x1b[201~", b"\x1b[<0;10;5M", b"\x1b", b"\x1bb"] {
            assert!(is_typed_input(key), "{key:?}");
        }
    }
}
