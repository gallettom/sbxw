//! One persistent terminal session: a PTY running a command, its output
//! broadcast to every WebSocket attached, and the last 256 KB kept for replay.
//!
//! A session outlives its WebSockets (a reload replays and resumes), and it
//! outlives its *process* only until the reader sees EOF, when it calls
//! `on_exit` — which the registry uses to forget it. "There is a session" and
//! "there is something to type into" are still different questions, so
//! `alive` asks the process.

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use sbxw_core::terminal::REPLAY_BYTES;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub struct PtySession {
    tx: broadcast::Sender<Vec<u8>>,
    bell: broadcast::Sender<()>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    replay: Mutex<VecDeque<u8>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    pub started_ms: u64,
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
        let session = Arc::new(Self {
            tx,
            bell,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            replay: Mutex::new(VecDeque::with_capacity(REPLAY_BYTES)),
            child: Mutex::new(child),
            started_ms: now_ms,
        });
        let me = Arc::downgrade(&session);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // Agents ring several BELs for one prompt; one notification per burst.
            let mut last_bell = Instant::now() - Duration::from_secs(3);
            loop {
                let n = match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let Some(s) = me.upgrade() else { break };
                let chunk = buf[..n].to_vec();
                if chunk.contains(&0x07) && last_bell.elapsed() >= Duration::from_secs(3) {
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

    pub fn resize(&self, cols: u16, rows: u16) -> Result<Option<(u16, u16)>> {
        let m = self.master.lock().unwrap();
        let before = m.get_size().ok().map(|s| (s.cols, s.rows));
        m.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(before)
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
}
