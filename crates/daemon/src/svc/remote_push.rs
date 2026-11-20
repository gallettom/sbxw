//! Web Push to a paired phone (`.sbxw-artifacts/remote/API.md` 2.8; the
//! pipe to the sidecar is `API-push.md`). The rules are
//! `sbxw_core::remote_access::push`'s; this module keeps the state
//! (`remote/push.json`: the VAPID key and one subscription per device),
//! drives the sidecar's pipes, and runs the ONE task that watches the bus
//! and decides when to push.
//!
//! What causes a push: only a change of state (a reachable session starts
//! to ask for the person, more plans or merges wait). No route does, and
//! a network request never does. A device whose event stream is open gets
//! no push (it has the toast). Only while remote access is on (D4): the
//! sidecar runs only then.
//!
//! Never logged, never journaled: the endpoint, the phone's keys, the VAPID
//! key, a job line. Log lines name a device id, a kind and a status word.

use crate::app::{now_ms, App};
use sbxw_core::agents::SessionState;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::push::{self as rules, Answer, Edges, Job, Limits, Outcome, PushFile, PushKind, Refused, Subscription, Vapid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The state file, beside `state.json`.
pub const FILE: &str = "push.json";
/// The longest answer line read from the sidecar (API-push.md section 2).
pub const LINE_MAX: usize = 4096;
/// Jobs written and not yet answered, at most (each can take 10 s).
const IN_FLIGHT_MAX: usize = 16;
/// How long after the first subscription the sidecar is restarted with
/// `--push`: the phone's subscribe request, which came through it, is
/// answered first.
pub const RESTART_DELAY: Duration = Duration::from_secs(2);
/// The trigger task looks again this often even with no event.
const TICK: Duration = Duration::from_secs(30);

/// The jobs pipe while a `--push` sidecar runs.
struct Pipe {
    tx: tokio::sync::mpsc::Sender<String>,
    gen: u64,
}

pub struct Push {
    file: PushFile,
    limits: Limits,
    edges: Edges,
    pipe: Option<Pipe>,
    /// Pipe generations: an old pipe's close is not this one's.
    gen: u64,
    /// Job id -> (device id, kind), until its answer.
    pending: BTreeMap<u64, (String, PushKind)>,
    seq: u64,
    task: bool,
    restart_pending: bool,
    pub(crate) restart_delay: Duration,
    /// Tests: how many times the sidecar was restarted for push.
    pub(crate) restarts: u32,
}

impl Push {
    /// `remote/push.json`; a subscription whose device is gone is dropped.
    pub fn load(dir: &Path, known: impl Fn(&str) -> bool) -> Self {
        let mut file = std::fs::read(dir.join(FILE)).ok().and_then(|b| PushFile::parse(&b)).unwrap_or_default();
        let before = file.subscriptions.len();
        file.subscriptions.retain(|id, _| known(id));
        let p = Push { file, limits: Limits::default(), edges: Edges::default(), pipe: None, gen: 0, pending: BTreeMap::new(), seq: 0, task: false, restart_pending: false, restart_delay: RESTART_DELAY, restarts: 0 };
        if p.file.subscriptions.len() != before {
            p.save(dir);
        }
        p
    }

    fn save(&self, dir: &Path) {
        if let Err(e) = super::remote_access::write_private(dir, &dir.join(FILE), &self.file.to_bytes()) {
            tracing::warn!("remote access: cannot write {FILE}: {e}");
        }
    }

    /// The VAPID key, made on first need.
    fn vapid(&mut self, dir: &Path) -> Vapid {
        if let Some(v) = &self.file.vapid {
            return v.clone();
        }
        let v = Vapid::generate();
        self.file.vapid = Some(v.clone());
        self.save(dir);
        tracing::info!("remote access: made this Mac's push key");
        v
    }

    /// Whether the sidecar should start with `--push`: a key and at least
    /// one subscription.
    pub(crate) fn wanted(&self) -> Option<String> {
        if self.file.subscriptions.is_empty() {
            return None;
        }
        self.file.vapid.as_ref().map(rules::config_line)
    }

    /// A device goes (revoked, logged out, 30 days idle): its subscription
    /// with it. Returns whether there was one.
    pub(crate) fn forget_device(&mut self, dir: &Path, id: &str) -> bool {
        self.limits.forget(id);
        let had = self.file.subscriptions.remove(id).is_some();
        if had {
            self.save(dir);
        }
        had
    }

    pub(crate) fn devices_on(&self) -> usize {
        self.file.subscriptions.len()
    }

    /// The pipe went away: on stop, or before a restart. Its close is then
    /// not reported.
    pub(crate) fn detach(&mut self) {
        self.pipe = None;
        self.gen += 1;
        self.pending.clear();
    }
}

/// Why `POST /r/push/subscribe` was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SubscribeError {
    Refused(Refused),
    /// The device is not paired any more.
    Gone,
}

impl App {
    // ── The phone's routes ───────────────────────────────────────────────

    /// `GET /r/push/key`: the VAPID public key (made on first need) and
    /// whether THIS device has a subscription.
    pub fn remote_push_key(&self, device: &str) -> (String, bool) {
        let mut rt = self.remote_access.lock().unwrap();
        let dir = rt.dir.clone();
        let key = rt.push.vapid(&dir).public_b64();
        (key, rt.push.file.subscriptions.contains_key(device))
    }

    /// `POST /r/push/subscribe`: checked, then stored (one per device: a new
    /// one replaces the old). The first subscription while the sidecar runs
    /// without `--push` restarts it with `--push`, once, shortly after.
    pub fn remote_push_subscribe(self: &Arc<Self>, device: &str, endpoint: &str, p256dh: &str, auth: &str) -> Result<(), SubscribeError> {
        let sub = Subscription::parse(endpoint, p256dh, auth, now_ms()).map_err(SubscribeError::Refused)?;
        let restart = {
            let mut rt = self.remote_access.lock().unwrap();
            let Some(name) = rt.devices.get(device).map(|d| d.name.clone()) else { return Err(SubscribeError::Gone) };
            let dir = rt.dir.clone();
            let _ = rt.push.vapid(&dir);
            rt.push.file.subscriptions.insert(device.to_string(), sub);
            rt.push.save(&dir);
            rt.record(device, &name, Kind::PushOn);
            tracing::info!("remote access: device {device} turned notifications on");
            let restart = rt.enabled && rt.push.pipe.is_none() && rt.sidecar_running() && !rt.push.restart_pending;
            if restart {
                rt.push.restart_pending = true;
            }
            restart.then_some(rt.push.restart_delay)
        };
        if let Some(delay) = restart {
            let me = Arc::downgrade(self);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                if let Some(app) = me.upgrade() {
                    app.remote_push_restart().await;
                }
            });
        }
        Ok(())
    }

    /// `POST /r/push/unsubscribe`, and the Mac's switch: idempotent; `false`
    /// when there was nothing to turn off.
    pub fn remote_push_off(&self, device: &str) -> bool {
        let mut rt = self.remote_access.lock().unwrap();
        let dir = rt.dir.clone();
        if !rt.push.forget_device(&dir, device) {
            return false;
        }
        let name = rt.devices.get(device).map(|d| d.name.clone()).unwrap_or_default();
        rt.record(device, &name, Kind::PushOff);
        tracing::info!("remote access: notifications off for device {device}");
        true
    }

    /// For `GET /api/remote`: devices with notifications on.
    pub fn remote_push_devices(&self) -> usize {
        self.remote_access.lock().unwrap().push.devices_on()
    }

    /// The Mac's list: which devices have notifications on.
    pub fn remote_push_on(&self, device: &str) -> bool {
        self.remote_access.lock().unwrap().push.file.subscriptions.contains_key(device)
    }

    // ── The sidecar's pipes ──────────────────────────────────────────────

    /// Restart the running sidecar so it starts with `--push`. Only when it
    /// still runs without it and remote access is on.
    async fn remote_push_restart(self: &Arc<Self>) {
        let child = {
            let mut rt = self.remote_access.lock().unwrap();
            rt.push.restart_pending = false;
            if !rt.enabled || rt.push.pipe.is_some() || rt.push.wanted().is_none() || !rt.sidecar_running() {
                return;
            }
            rt.push.restarts += 1;
            rt.sidecar.take()
        };
        tracing::info!("remote access: restarting {} for notifications", super::remote_access::SIDECAR_NAME);
        if let Some(c) = child {
            let status = self.remote_access.lock().unwrap().tailnet_status_path();
            super::remote_access::stop_sidecar(c, &status).await;
        }
        if let Err(e) = self.remote_start_again().await {
            tracing::warn!("remote access: could not restart {}: {e}", super::remote_access::SIDECAR_NAME);
        }
    }

    /// A `--push` sidecar was spawned: write the configuration line, then
    /// jobs; read its answers. A closed pipe or a dead sidecar stops pushing
    /// (one log line, one journal line); the proxy is not touched.
    pub(crate) fn remote_push_attach<W, R>(self: &Arc<Self>, config: String, stdin: W, stdout: R)
    where
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(IN_FLIGHT_MAX);
        let gen = {
            let mut rt = self.remote_access.lock().unwrap();
            rt.push.detach();
            let gen = rt.push.gen;
            rt.push.pipe = Some(Pipe { tx, gen });
            gen
        };
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut stdin = stdin;
            let mut ok = stdin.write_all(config.as_bytes()).await.is_ok() && stdin.flush().await.is_ok();
            drop(config);
            while ok {
                let Some(line) = rx.recv().await else { break };
                ok = stdin.write_all(line.as_bytes()).await.is_ok() && stdin.flush().await.is_ok();
            }
            if !ok {
                if let Some(app) = me.upgrade() {
                    app.remote_push_closed(gen);
                }
            }
        });
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut r = tokio::io::BufReader::new(stdout);
            loop {
                match read_line(&mut r, LINE_MAX).await {
                    Some(Some(line)) => match me.upgrade() {
                        Some(app) => app.remote_push_answer(gen, &line),
                        None => break,
                    },
                    Some(None) => tracing::warn!("remote access: an answer line from the sidecar over {LINE_MAX} bytes, dropped"),
                    None => break,
                }
            }
            if let Some(app) = me.upgrade() {
                app.remote_push_closed(gen);
            }
        });
    }

    /// The pipe of generation `gen` closed: pushing stops until the sidecar
    /// is started again. Quiet when that pipe was already detached.
    fn remote_push_closed(&self, gen: u64) {
        let mut rt = self.remote_access.lock().unwrap();
        if rt.push.pipe.as_ref().is_none_or(|p| p.gen != gen) {
            return;
        }
        rt.push.detach();
        tracing::warn!("remote access: the sidecar's push pipe closed; notifications stop until remote access starts again");
        rt.record_target("", "", Kind::PushFailed, Some("sidecar".into()));
    }

    /// One answer line (API-push.md section 5).
    pub(crate) fn remote_push_answer(&self, gen: u64, line: &str) {
        let Ok(a) = serde_json::from_str::<Answer>(line.trim()) else {
            tracing::debug!("remote access: a sidecar answer that does not parse, ignored");
            return;
        };
        let mut rt = self.remote_access.lock().unwrap();
        if rt.push.gen != gen {
            return;
        }
        let id = a.id.strip_prefix('j').and_then(|n| n.parse::<u64>().ok());
        let Some((device, kind)) = id.and_then(|n| rt.push.pending.remove(&n)) else {
            if a.status == "bad_config" {
                tracing::warn!("remote access: the sidecar refused the push configuration");
                rt.record_target("", "", Kind::PushFailed, Some("bad_config".into()));
            }
            return;
        };
        let name = rt.devices.get(&device).map(|d| d.name.clone()).unwrap_or_default();
        let dir = rt.dir.clone();
        match rules::outcome(&a.status) {
            Outcome::Sent => {
                tracing::info!("remote access: push sent to device {device}, kind {}", kind.letter());
                rt.record_target(&device, &name, Kind::PushSent, Some(kind.letter().into()));
            }
            Outcome::Gone => {
                tracing::info!("remote access: push subscription of device {device} gone, deleted");
                rt.push.forget_device(&dir, &device);
                rt.record(&device, &name, Kind::PushGone);
            }
            Outcome::Backoff => {
                tracing::info!("remote access: push to device {device} rate-limited by Apple, paused");
                rt.push.limits.pause(&device, now_ms());
                rt.record_target(&device, &name, Kind::PushFailed, Some("4xx".into()));
            }
            Outcome::Failed(class) => {
                tracing::info!("remote access: push to device {device} failed ({class})");
                rt.record_target(&device, &name, Kind::PushFailed, Some(class.into()));
            }
        }
    }

    // ── The trigger ──────────────────────────────────────────────────────

    /// Start the ONE task that watches the bus (once per daemon).
    pub(crate) fn remote_push_task(self: &Arc<Self>) {
        {
            let mut rt = self.remote_access.lock().unwrap();
            if rt.push.task {
                return;
            }
            rt.push.task = true;
        }
        let mut bus = self.bus.subscribe();
        let me = Arc::downgrade(self);
        if let Some(app) = me.upgrade() {
            app.remote_push_step();
        }
        tokio::spawn(async move {
            use crate::bus::Event;
            let mut tick = tokio::time::interval(TICK);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    ev = bus.recv() => match ev {
                        Ok(Event::Session(_) | Event::Lead(_) | Event::LeadEnd(_) | Event::Plan(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Ok(_) => continue,
                        Err(_) => break,
                    },
                }
                let Some(app) = me.upgrade() else { break };
                app.remote_push_step();
            }
        });
    }

    /// Look at the state once: the edges always move (so turning remote
    /// access on later does not push for what was already there); a push
    /// goes only while remote access is on and the pipe is open, to each
    /// device with a subscription, no stream open, and room in its limits.
    pub(crate) fn remote_push_step(&self) {
        let asking: BTreeSet<String> = self.remote_reachable().into_iter().filter(|(_, _, i)| i.state == SessionState::Attention).map(|(s, _, _)| s.to_string()).collect();
        let waiting = self.remote_waiting_count();
        let now = now_ms();
        let mut guard = self.remote_access.lock().unwrap();
        let rt = &mut *guard;
        let asking_n = asking.len();
        let kinds = rt.push.edges.step(asking, waiting);
        if kinds.is_empty() || !rt.enabled {
            return;
        }
        let Some(tx) = rt.push.pipe.as_ref().map(|p| p.tx.clone()) else { return };
        // With level 2 off the phone is told nothing of approvals but the
        // content-free `p` push: the badge then counts sessions only.
        let badge = rules::badge(asking_n + if rt.level2.enabled { waiting } else { 0 });
        let targets: Vec<(String, Subscription)> = rt
            .push
            .file
            .subscriptions
            .iter()
            .filter(|(id, _)| rt.devices.get(id).is_some())
            .filter(|(id, _)| rt.level2.streams.get(*id).is_none_or(|l| l.is_empty()))
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect();
        for (device, sub) in targets {
            let Some(kind) = kinds.iter().copied().find(|k| rt.push.limits.take(&device, *k, now)) else { continue };
            if rt.push.pending.len() >= IN_FLIGHT_MAX {
                tracing::info!("remote access: push to device {device} skipped, {IN_FLIGHT_MAX} unanswered");
                continue;
            }
            rt.push.seq += 1;
            let seq = rt.push.seq;
            let id = format!("j{seq}");
            let line = Job { id: &id, endpoint: &sub.endpoint, p256dh: &sub.p256dh, auth: &sub.auth, kind, badge, ttl: rules::TTL_S }.line();
            if tx.try_send(line).is_ok() {
                rt.push.pending.insert(seq, (device, kind));
            }
        }
    }

    /// Plans waiting for the person's approval plus merges proposed.
    fn remote_waiting_count(&self) -> usize {
        use sbxw_core::plans::{MergeState, PlanState};
        self.plans_list().iter().filter(|p| (p.state == PlanState::Pending && !p.closed) || p.merge.as_ref().is_some_and(|m| m.state == MergeState::Proposed)).count()
    }
}

/// One line of at most `max` bytes: `Some(Some(line))`, `Some(None)` for a
/// longer line (read to its end and dropped), `None` at the end.
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R, max: usize) -> Option<Option<String>> {
    use tokio::io::AsyncBufReadExt;
    let mut buf = Vec::new();
    let mut over = false;
    loop {
        let chunk = match r.fill_buf().await {
            Ok(c) => c,
            Err(_) => return None,
        };
        if chunk.is_empty() {
            return if buf.is_empty() && !over { None } else { Some((!over).then(|| String::from_utf8_lossy(&buf).into_owned())) };
        }
        let (take, done) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        let part = &chunk[..if done { take - 1 } else { take }];
        if !over {
            if buf.len() + part.len() > max {
                over = true;
                buf.clear();
            } else {
                buf.extend_from_slice(part);
            }
        }
        r.consume(take);
        if done {
            return Some((!over).then(|| String::from_utf8_lossy(&buf).into_owned()));
        }
    }
}
