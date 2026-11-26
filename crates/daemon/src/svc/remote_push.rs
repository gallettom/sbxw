//! Web Push to a paired phone (`.sbxw-artifacts/remote/API.md` 2.8; the
//! pipe to the sidecar is `API-push.md`). The rules are
//! `sbxw_core::remote_access::push`'s; this module keeps the state
//! (`remote/push.json`: the VAPID key and one subscription per device),
//! drives the sidecar's pipes, and runs the ONE task that watches the bus
//! and decides when to push.
//!
//! What causes a push: only a change of state (a reachable session starts
//! to ask for the person, more plans or merges wait, an agent of a
//! reachable sandbox posts a new round of shots to a canvas, a relay request
//! starts to wait for the person, or the daemon reminds the person of one
//! after 10 minutes, `reminded_ms`). No route does, and
//! a network request never does. A device whose event stream is open gets
//! no push (it has the toast). Only while remote access is on (D4): the
//! sidecar runs only then.
//!
//! Never logged, never journaled: the endpoint, the phone's keys, the VAPID
//! key, a job line. Log lines name a device id, a kind and a status word.

use crate::app::{now_ms, App};
use sbxw_core::agents::SessionState;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::push::{self as rules, Answer, Asking, Edges, Job, Limits, Outcome, PushFile, PushKind, Refused, Subscription, Vapid, Waiting};
use sbxw_core::remote_access::reach::ReachKind;
use sbxw_core::relay::{RelayRequest, RelayState};
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

/// One relay request as the trigger sees it: a request waits for the person
/// while it is `pending` (to answer or route) or `answered` (to review). A
/// request is told apart by its id AND its creation time, so a reused id is
/// never taken for an older request; and by the state it waits in, so a
/// routed question that comes back for review waits anew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RelayWait {
    pub key: (String, u64, &'static str),
    /// The asking sandbox, as the push names it ([`rules::push_name`]).
    pub name: String,
    pub lead: bool,
    /// The daemon reminded the person of it (`reminded_ms`, once per
    /// request, after `relay::REMIND_AFTER_MS` waiting: D17).
    pub due: bool,
}

impl RelayWait {
    /// `None` for a request that does not wait for the person (routed, or
    /// settled). Never the question, the answer or the note: a push carries
    /// the sandbox's name and the kind letter only.
    pub(crate) fn of(r: &RelayRequest, lead: bool) -> Option<RelayWait> {
        let phase = match r.state {
            RelayState::Pending => "pending",
            RelayState::Answered => "answered",
            _ => return None,
        };
        Some(RelayWait { key: (r.id.clone(), r.created_ms, phase), name: rules::push_name(&r.from), lead, due: r.reminded_ms.is_some() })
    }
}

/// The relay's edges: a request that starts to wait for the person pushes
/// `n` (the sandbox named, never the request's text), and so does the
/// daemon's reminder of one, once. The first look sets
/// the baseline: what already waits pushes nothing, and what was already due
/// is not reminded (it may have been before a restart). A settled or routed
/// request is forgotten.
#[derive(Debug, Default)]
pub(crate) struct RelayEdges {
    seen: Option<BTreeSet<(String, u64, &'static str)>>,
    reminded: BTreeSet<(String, u64, &'static str)>,
}

impl RelayEdges {
    pub(crate) fn step(&mut self, waits: &[RelayWait]) -> Option<rules::Fire> {
        let live: BTreeSet<_> = waits.iter().map(|w| w.key.clone()).collect();
        self.reminded.retain(|k| live.contains(k));
        let Some(seen) = &self.seen else {
            self.reminded.extend(waits.iter().filter(|w| w.due).map(|w| w.key.clone()));
            self.seen = Some(live);
            return None;
        };
        let fresh = waits.iter().find(|w| !seen.contains(&w.key));
        let due: Vec<&RelayWait> = waits.iter().filter(|w| w.due && !self.reminded.contains(&w.key)).collect();
        // A request that is new and already due (a slow look) is one push.
        self.reminded.extend(due.iter().map(|w| w.key.clone()));
        self.reminded.extend(waits.iter().filter(|w| w.due && !seen.contains(&w.key)).map(|w| w.key.clone()));
        self.seen = Some(live);
        fresh.or(due.first().copied()).map(|w| rules::Fire { kind: PushKind::Needs, name: w.name.clone(), lead: w.lead })
    }
}

/// The jobs pipe while a `--push` sidecar runs.
struct Pipe {
    tx: tokio::sync::mpsc::Sender<String>,
    gen: u64,
}

pub struct Push {
    file: PushFile,
    limits: Limits,
    edges: Edges,
    relay: RelayEdges,
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
        let p = Push { file, limits: Limits::default(), edges: Edges::default(), relay: RelayEdges::default(), pipe: None, gen: 0, pending: BTreeMap::new(), seq: 0, task: false, restart_pending: false, restart_delay: RESTART_DELAY, restarts: 0 };
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
    /// THIS device's subscription as the page may see it: `None`, or the
    /// tag of its endpoint ([`rules::endpoint_tag`]), so the page can tell
    /// that its own subscription is not the one the Mac sends to.
    pub fn remote_push_key(&self, device: &str) -> (String, Option<String>) {
        let mut rt = self.remote_access.lock().unwrap();
        let dir = rt.dir.clone();
        let key = rt.push.vapid(&dir).public_b64();
        (key, rt.push.file.subscriptions.get(device).map(|s| rules::endpoint_tag(&s.endpoint)))
    }

    /// `POST /r/push/subscribe`: checked, then stored. One per device: a new
    /// one replaces the device's old one (an app removed and added again
    /// subscribes afresh, and the old endpoint, which Apple still accepts
    /// but no app shows, is never sent to again). One device per endpoint:
    /// another device holding the same endpoint loses it. The first subscription while the sidecar runs
    /// without `--push` restarts it with `--push`, once, shortly after.
    pub fn remote_push_subscribe(self: &Arc<Self>, device: &str, endpoint: &str, p256dh: &str, auth: &str) -> Result<(), SubscribeError> {
        let sub = Subscription::parse(endpoint, p256dh, auth, now_ms()).map_err(SubscribeError::Refused)?;
        let restart = {
            let mut rt = self.remote_access.lock().unwrap();
            let Some(name) = rt.devices.get(device).map(|d| d.name.clone()) else { return Err(SubscribeError::Gone) };
            let dir = rt.dir.clone();
            let _ = rt.push.vapid(&dir);
            rt.push.file.subscriptions.retain(|id, s| id == device || s.endpoint != sub.endpoint);
            if rt.push.file.subscriptions.insert(device.to_string(), sub).is_some() {
                tracing::info!("remote access: device {device} replaced its push subscription");
            }
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
                        Ok(Event::Canvas(n)) if n.what == "shots" => {}
                        Ok(Event::Relay(_)) => {}
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
        let reachable = self.remote_reachable();
        let reach: BTreeMap<String, bool> = reachable.iter().map(|(s, k, _)| (s.to_string(), *k == ReachKind::Leader)).collect();
        let rounds = self.remote_canvas_rounds();
        let asking: BTreeMap<String, Asking> = reachable
            .into_iter()
            .filter(|(_, _, i)| i.state == SessionState::Attention)
            .map(|(s, k, _)| (s.to_string(), Asking { name: rules::push_name(s.name.as_str()), lead: k == ReachKind::Leader }))
            .collect();
        let waiting_by = self.remote_waiting();
        let waiting: usize = waiting_by.values().map(|w| w.plans + w.merges).sum();
        let now = now_ms();
        let relay: Vec<RelayWait> = self.relay_list().iter().filter_map(|r| RelayWait::of(r, reach.get(&r.from).copied().unwrap_or(false))).collect();
        let mut guard = self.remote_access.lock().unwrap();
        let rt = &mut *guard;
        let asking_n = asking.len();
        let mut fires = rt.push.edges.step(asking, waiting_by);
        fires.extend(rt.push.edges.shots(rounds, &reach));
        fires.extend(rt.push.relay.step(&relay));
        if fires.is_empty() || !rt.enabled {
            return;
        }
        let Some(tx) = rt.push.pipe.as_ref().map(|p| p.tx.clone()) else { return };
        // With level 2 off the phone is told nothing of approvals but the
        // content-free `p` push: the badge then counts sessions only. Relay
        // requests waiting for the person count as plans and merges do.
        let badge = rules::badge(asking_n + if rt.level2.enabled { waiting + relay.len() } else { 0 });
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
            let Some(fire) = fires.iter().find(|f| rt.push.limits.take(&device, f.kind, now)) else { continue };
            let kind = fire.kind;
            if rt.push.pending.len() >= IN_FLIGHT_MAX {
                tracing::info!("remote access: push to device {device} skipped, {IN_FLIGHT_MAX} unanswered");
                continue;
            }
            rt.push.seq += 1;
            let seq = rt.push.seq;
            let id = format!("j{seq}");
            let line = Job { id: &id, endpoint: &sub.endpoint, p256dh: &sub.p256dh, auth: &sub.auth, kind, name: &fire.name, lead: fire.lead, badge, ttl: rules::TTL_S }.line();
            if tx.try_send(line).is_ok() {
                rt.push.pending.insert(seq, (device, kind));
            }
        }
    }

    /// Rounds of shots per sandbox, every canvas's (the `c` edge).
    fn remote_canvas_rounds(&self) -> BTreeMap<String, usize> {
        let mut out: BTreeMap<String, usize> = BTreeMap::new();
        for s in self.canvas_list().0 {
            *out.entry(s.sandbox).or_default() += s.rounds;
        }
        out
    }

    /// Plans waiting for the person's approval plus merges proposed, per
    /// sandbox.
    fn remote_waiting(&self) -> BTreeMap<String, Waiting> {
        use sbxw_core::plans::{MergeState, PlanState};
        let mut out: BTreeMap<String, Waiting> = BTreeMap::new();
        for p in self.plans_list().iter() {
            let plan = p.state == PlanState::Pending && !p.closed;
            let merge = p.merge.as_ref().is_some_and(|m| matches!(m.state, MergeState::Proposed | MergeState::MergedCi));
            if plan || merge {
                let w = out.entry(p.sandbox.clone()).or_default();
                // A plan still pending counts as a plan, else its merge.
                if plan {
                    w.plans += 1;
                } else {
                    w.merges += 1;
                }
            }
        }
        out
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
