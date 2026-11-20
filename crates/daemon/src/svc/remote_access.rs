//! Remote access (`docs/remote-access.md`): the state on disk, the remote
//! listener's life, the tailnet sidecar, pairing, devices, the journal, and
//! what a paired phone may do to a running team leader. The rules are
//! `sbxw_core::remote_access`'s; the routes are `http::remote` (the phone)
//! and `http::remote_admin` (the Mac, on the UI listener).
//!
//! Never logged, never journaled: the text of a message, an answer or a
//! transcription. Log lines name a device id, a sandbox, a kind and sizes.

use crate::app::{now_ms, App};
use sbxw_core::agents::SessionState;
use sbxw_core::remote_access::devices::{DeviceBook, DeviceView};
use sbxw_core::remote_access::journal::{self, Entry, Kind};
use sbxw_core::remote_access::limits::Limiter;
use sbxw_core::remote_access::pairing::{PairingBook, RequestView, StartError, Status};
use sbxw_core::remote_access::view::{phone_activity, phone_page, phone_question, phone_state, PhonePage, PhoneQuestion};
use sbxw_core::remote_access::{self as rules, State, TailnetStatus};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::SandboxRef;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The sidecar, next to the daemon's own executable (no PATH lookup).
pub const SIDECAR_NAME: &str = "sbxw-tailnet";

/// How long a read of `tailnet.json` answers for.
const TAILNET_TTL_MS: u64 = 2_000;

/// What closes a phone's event stream at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kick {
    /// That device was revoked or logged out.
    Device(String),
    /// Remote access was turned off.
    All,
}

/// Which sidecar to start: the real one, another program (tests), or none
/// (tests of the listener alone).
#[derive(Clone, Debug, Default)]
pub enum Sidecar {
    #[default]
    Beside,
    Program(PathBuf),
    None,
}

/// The listener while it is bound.
struct Listener {
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    addr: std::net::SocketAddr,
}

pub struct RemoteRuntime {
    dir: PathBuf,
    enabled: bool,
    devices: DeviceBook,
    journal: Vec<Entry>,
    pairing: PairingBook,
    pair_limit: Limiter,
    status_limit: Limiter,
    bad_token_limit: Limiter,
    listener: Option<Listener>,
    sidecar: Option<tokio::process::Child>,
    pub sidecar_program: Sidecar,
    tailnet: Option<(u64, Option<TailnetStatus>)>,
    /// Tests: the tailnet name, as if a sidecar served it.
    pub tailnet_override: Option<String>,
    /// Tests: bind here instead of `remote_addr` (port 0 picks one).
    pub addr_override: Option<String>,
    kicks: tokio::sync::broadcast::Sender<Kick>,
}

impl RemoteRuntime {
    /// Read the state under `<sbxw home>/remote`. A file that does not parse
    /// leaves remote access off with no device: never on by accident.
    pub fn load(sbxw_home: &Path) -> Self {
        let dir = sbxw_home.join("remote");
        let state: State = std::fs::read(dir.join("state.json"))
            .ok()
            .and_then(|b| match serde_json::from_slice(&b) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!("remote access: state.json unreadable ({e}); remote access stays off");
                    None
                }
            })
            .unwrap_or_default();
        let journal = std::fs::read_to_string(dir.join("journal.jsonl")).map(|t| journal::parse_lines(&t)).unwrap_or_default();
        let (kicks, _) = tokio::sync::broadcast::channel(16);
        let mut rt = RemoteRuntime {
            dir,
            enabled: state.enabled,
            devices: DeviceBook::from_devices(state.devices),
            journal,
            pairing: PairingBook::new(),
            pair_limit: Limiter::new(60_000, 5, 20),
            status_limit: Limiter::new(60_000, 120, 600),
            bad_token_limit: Limiter::new(60_000, 10, 30),
            listener: None,
            sidecar: None,
            sidecar_program: Sidecar::Beside,
            tailnet: None,
            tailnet_override: None,
            addr_override: None,
            kicks,
        };
        rt.expire(now_ms());
        rt
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn tailnet_status_path(&self) -> PathBuf {
        self.dir.join("tailnet.json")
    }

    pub fn tailnet_state_dir(&self) -> PathBuf {
        self.dir.join("tailnet")
    }

    fn save(&self) {
        let state = State { enabled: self.enabled, devices: self.devices.devices().to_vec() };
        let bytes = serde_json::to_vec_pretty(&state).expect("serialisable");
        if let Err(e) = write_private(&self.dir, &self.state_path(), &bytes) {
            tracing::warn!("remote access: cannot write state.json: {e}");
        }
    }

    fn record(&mut self, device_id: &str, device_name: &str, kind: Kind) {
        self.journal.push(Entry { at_ms: now_ms(), device_id: device_id.into(), device_name: device_name.into(), kind });
        journal::bound(&mut self.journal);
        if let Err(e) = write_private(&self.dir, &self.dir.join("journal.jsonl"), journal::to_lines(&self.journal).as_bytes()) {
            tracing::warn!("remote access: cannot write the journal: {e}");
        }
    }

    fn expire(&mut self, now: u64) {
        let gone = self.devices.expire(now);
        if gone.is_empty() {
            return;
        }
        for d in &gone {
            tracing::info!("remote access: device {} dropped after 30 days idle", d.id);
            self.record(&d.id, &d.name, Kind::Expired);
            let _ = self.kicks.send(Kick::Device(d.id.clone()));
        }
        self.save();
    }

    fn sidecar_pid(&mut self) -> Option<u32> {
        let c = self.sidecar.as_mut()?;
        match c.try_wait() {
            Ok(None) => c.id(),
            _ => None,
        }
    }

    /// The tailnet name the Host and Origin checks may use, if the sidecar
    /// serves one. Re-read at most every 2 s.
    fn serving_name(&mut self) -> Option<String> {
        if let Some(n) = &self.tailnet_override {
            return Some(n.clone());
        }
        let now = now_ms();
        let fresh = self.tailnet.as_ref().is_some_and(|(at, _)| now.saturating_sub(*at) < TAILNET_TTL_MS);
        if !fresh {
            let st = read_tailnet(&self.tailnet_status_path());
            self.tailnet = Some((now, st));
        }
        let pid = self.sidecar_pid();
        let st = self.tailnet.as_ref().and_then(|(_, s)| s.as_ref())?;
        st.serving_name(now, pid).map(str::to_string)
    }

    fn tailnet_status(&mut self) -> Option<TailnetStatus> {
        self.tailnet = None;
        let _ = self.serving_name();
        self.tailnet.as_ref().and_then(|(_, s)| s.clone())
    }
}

/// The status file, as untrusted input: size cap first, then the schema.
fn read_tailnet(path: &Path) -> Option<TailnetStatus> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(rules::TAILNET_FILE_MAX + 1).read_to_end(&mut buf).ok()?;
    TailnetStatus::parse(&buf)
}

/// Write `bytes` to `path` in `dir`: the directory `0700`, the file created
/// `0600` under another name and renamed over.
pub fn write_private(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        open.mode(0o600);
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), sbxw_core::ids::random_hex(4)));
    let mut f = open.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// `GET /api/remote`.
#[derive(Serialize, Debug)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub remote_addr: String,
    pub listening: bool,
    pub tailnet: Option<TailnetView>,
    pub devices: usize,
    pub pending_requests: usize,
    pub keep_awake: &'static str,
}

#[derive(Serialize, Debug)]
pub struct TailnetView {
    pub state: sbxw_core::remote_access::TailnetState,
    pub name: Option<String>,
    pub serving: bool,
    pub login_url: Option<String>,
    pub detail: Option<String>,
}

/// A device's request, as the remote routes know it (from its cookie).
#[derive(Clone, Debug)]
pub struct DeviceCtx {
    pub id: String,
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// No cookie, or one that matches no device.
    NotPaired,
    /// Too many failed tokens: `retry_after_ms`.
    Limited(u64),
}

/// What a phone is refused, and why, in a status the route maps.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotFound,
    Forbidden(String),
    Conflict(String),
    Unavailable(String),
    Bad(String),
    Limited(u64),
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct LeaderView {
    pub sandbox: String,
    pub name: String,
    pub state: &'static str,
    pub activity: &'static str,
    pub last_at: u64,
    pub asks: bool,
    pub answerable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<PhoneQuestion>>,
}

#[derive(Serialize, Debug)]
pub struct PairingCode {
    pub code: String,
    pub url: String,
    pub expires_ms: u64,
}

impl App {
    pub fn remote_kicks(&self) -> tokio::sync::broadcast::Receiver<Kick> {
        self.remote_access.lock().unwrap().kicks.subscribe()
    }

    /// Whether the remote routes may answer at all: a connection kept open
    /// from before `disable` gets nothing.
    pub fn remote_enabled(&self) -> bool {
        self.remote_access.lock().unwrap().enabled
    }

    /// The Host values the remote listener answers now, and the tailnet name.
    pub fn remote_hosts(&self) -> (Vec<String>, Option<String>) {
        let mut rt = self.remote_access.lock().unwrap();
        let name = rt.serving_name();
        let addr = rt.listener.as_ref().map(|l| l.addr.to_string()).unwrap_or_else(|| self.cfg.remote_addr.clone());
        (rules::allowed_hosts(&addr, name.as_deref()), name)
    }

    pub fn remote_status(&self) -> RemoteStatus {
        let mut rt = self.remote_access.lock().unwrap();
        let st = rt.tailnet_status();
        let serving = rt.serving_name();
        let tailnet = st.map(|s| TailnetView { serving: serving.is_some() && s.name == serving, state: s.state, name: s.name, login_url: s.login_url, detail: s.detail });
        let listening = rt.listener.as_ref().is_some_and(|l| !l.task.is_finished());
        RemoteStatus {
            enabled: rt.enabled,
            remote_addr: self.cfg.remote_addr.clone(),
            listening,
            tailnet,
            devices: rt.devices.len(),
            pending_requests: rt.pairing.pending(now_ms()).len(),
            keep_awake: self.remote_keep_awake(rt.enabled),
        }
    }

    /// Turn remote access on: bind the listener, start the sidecar, write
    /// `enabled`. Idempotent.
    pub async fn remote_enable(self: &Arc<Self>) -> Result<RemoteStatus, String> {
        self.remote_start().await?;
        {
            let mut rt = self.remote_access.lock().unwrap();
            if !rt.enabled {
                rt.enabled = true;
                rt.save();
                tracing::info!("remote access: turned on");
            }
        }
        self.remote_awake_changed();
        Ok(self.remote_status())
    }

    /// At daemon start: back on if it was on. A failure is logged and
    /// shown in the status; `enabled` stays as the person left it.
    pub async fn remote_boot(self: &Arc<Self>) {
        if self.remote_enabled() {
            if let Err(e) = self.remote_start().await {
                tracing::warn!("remote access: could not start: {e}");
            }
            self.remote_awake_changed();
        }
    }

    async fn remote_start(self: &Arc<Self>) -> Result<(), String> {
        let (addr, program, state_dir, status_path, running) = {
            let mut rt = self.remote_access.lock().unwrap();
            let running = rt.listener.as_ref().is_some_and(|l| !l.task.is_finished());
            let addr = rt.addr_override.clone().unwrap_or_else(|| self.cfg.remote_addr.clone());
            let _ = rt.sidecar_pid();
            (addr, rt.sidecar_program.clone(), rt.tailnet_state_dir(), rt.tailnet_status_path(), running)
        };
        if !rules::is_loopback_addr(&addr) {
            return Err(format!("remote_addr {addr} is not a loopback address"));
        }
        let sidecar_bin = match &program {
            Sidecar::Beside => {
                let exe = std::env::current_exe().map_err(|e| format!("cannot find the sbxw executable: {e}"))?;
                let p = exe.parent().map(|d| d.join(SIDECAR_NAME)).ok_or("cannot find the sbxw executable's folder")?;
                if !p.is_file() {
                    return Err(format!("{SIDECAR_NAME} is not installed next to sbxw ({}): remote access needs it", p.display()));
                }
                Some(p)
            }
            Sidecar::Program(p) => Some(p.clone()),
            Sidecar::None => None,
        };
        if !running {
            let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| format!("cannot listen on {addr} ({e})"))?;
            let local = listener.local_addr().map_err(|e| e.to_string())?;
            let (tx, mut rx) = tokio::sync::watch::channel(false);
            let router = crate::http::remote::remote_router(self.clone());
            let task = tokio::spawn(async move {
                let stop = async move {
                    let _ = rx.wait_for(|v| *v).await;
                };
                if let Err(e) = axum::serve(listener, router).with_graceful_shutdown(stop).await {
                    tracing::warn!("remote access: listener ended: {e}");
                }
            });
            tracing::info!("remote access: listening on {local} (reached only through {SIDECAR_NAME})");
            self.remote_access.lock().unwrap().listener = Some(Listener { shutdown: tx, task, addr: local });
        }
        let has_sidecar = self.remote_access.lock().unwrap().sidecar_pid().is_some();
        if let (Some(bin), false) = (sidecar_bin, has_sidecar) {
            let local = self.remote_access.lock().unwrap().listener.as_ref().map(|l| l.addr.to_string()).unwrap_or(addr);
            match spawn_sidecar(&bin, &local, &state_dir, &status_path) {
                Ok(child) => {
                    tracing::info!("remote access: started {SIDECAR_NAME} (pid {:?})", child.id());
                    self.remote_access.lock().unwrap().sidecar = Some(child);
                }
                Err(e) => {
                    self.remote_stop().await;
                    return Err(format!("cannot start {SIDECAR_NAME}: {e}"));
                }
            }
        }
        Ok(())
    }

    /// Unbind, stop the sidecar, close every stream; pairing codes and
    /// pending requests go. Devices stay. Idempotent.
    pub async fn remote_disable(self: &Arc<Self>) -> RemoteStatus {
        {
            let mut rt = self.remote_access.lock().unwrap();
            if rt.enabled {
                rt.enabled = false;
                rt.save();
                tracing::info!("remote access: turned off");
            }
            rt.pairing.clear();
        }
        self.remote_stop().await;
        self.remote_awake_changed();
        self.remote_status()
    }

    async fn remote_stop(&self) {
        let (listener, sidecar, kicks, status_path) = {
            let mut rt = self.remote_access.lock().unwrap();
            (rt.listener.take(), rt.sidecar.take(), rt.kicks.clone(), rt.tailnet_status_path())
        };
        let _ = kicks.send(Kick::All);
        if let Some(l) = listener {
            let _ = l.shutdown.send(true);
            // Streams were told to end; a connection that lingers is cut.
            if tokio::time::timeout(Duration::from_secs(2), &mut { l.task }).await.is_err() {
                tracing::debug!("remote access: listener did not drain in 2 s");
            }
        }
        if let Some(mut c) = sidecar {
            let _ = c.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), c.wait()).await;
            // No stale file: a missing one means "not serving".
            let _ = std::fs::remove_file(&status_path);
        }
        self.remote_access.lock().unwrap().tailnet = None;
    }

    // ── Pairing (the Mac's side) ─────────────────────────────────────────

    pub fn remote_pairing_code(&self) -> Result<PairingCode, String> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.enabled {
            return Err("remote access is off: turn it on first".into());
        }
        let Some(name) = rt.serving_name() else {
            return Err("the tailnet is not serving yet: wait until the sidecar says running".into());
        };
        let (code, expires_ms) = rt.pairing.new_code(now_ms());
        tracing::info!("remote access: new pairing code (valid 5 minutes)");
        Ok(PairingCode { url: format!("https://{name}/#pair={code}"), code, expires_ms })
    }

    pub fn remote_pairing_requests(&self) -> Vec<RequestView> {
        self.remote_access.lock().unwrap().pairing.pending(now_ms())
    }

    pub fn remote_pairing_approve(&self, id: &str) -> bool {
        let ok = self.remote_access.lock().unwrap().pairing.approve(id, now_ms());
        if ok {
            tracing::info!("remote access: pairing request approved on the Mac");
        }
        ok
    }

    pub fn remote_pairing_refuse(&self, id: &str) -> bool {
        let mut rt = self.remote_access.lock().unwrap();
        match rt.pairing.refuse(id, now_ms()) {
            Some(name) => {
                rt.record("", &name, Kind::Refused);
                tracing::info!("remote access: pairing request refused on the Mac");
                true
            }
            None => false,
        }
    }

    // ── Pairing (the phone's side) ───────────────────────────────────────

    pub fn remote_pair_start(&self, source: &str, code: &str, device_name: &str) -> Result<RequestView, Refusal> {
        let mut rt = self.remote_access.lock().unwrap();
        let now = now_ms();
        rt.pair_limit.hit(source, now).map_err(Refusal::Limited)?;
        let name = rules::clean_device_name(device_name).ok_or_else(|| Refusal::Bad("device_name must be 1 to 64 plain characters".into()))?;
        match rt.pairing.start(code, &name, now) {
            Ok(v) => {
                tracing::info!("remote access: pairing request from a phone, waiting for the Mac");
                Ok(v)
            }
            Err(StartError::InvalidCode) => {
                tracing::info!("remote access: pairing start with a code that is not valid");
                Err(Refusal::Forbidden("this pairing code is not valid".into()))
            }
            Err(StartError::TooMany) => Err(Refusal::Conflict("too many pairing requests wait on the Mac".into())),
        }
    }

    /// A phone polls; on `paired`, the device and its token (the one time
    /// the token exists in the daemon).
    pub fn remote_pair_status(&self, source: &str, id: &str) -> Result<(Status, Option<(DeviceView, String)>), Refusal> {
        let mut rt = self.remote_access.lock().unwrap();
        let now = now_ms();
        rt.status_limit.hit(source, now).map_err(Refusal::Limited)?;
        match rt.pairing.status(id, now) {
            Status::Approved { device_name } => match rt.devices.mint(&device_name, now) {
                Some((d, token)) => {
                    rt.save();
                    rt.record(&d.id, &d.name, Kind::Paired);
                    tracing::info!("remote access: device {} paired", d.id);
                    Ok((Status::Approved { device_name }, Some((d.view(), token))))
                }
                None => Err(Refusal::Conflict("too many paired devices: revoke one on the Mac".into())),
            },
            s => Ok((s, None)),
        }
    }

    // ── Devices ──────────────────────────────────────────────────────────

    /// The device a presented cookie belongs to.
    pub fn remote_auth(&self, source: &str, token: Option<&str>) -> Result<DeviceCtx, AuthError> {
        let mut rt = self.remote_access.lock().unwrap();
        let now = now_ms();
        rt.bad_token_limit.check(source, now).map_err(AuthError::Limited)?;
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return Err(AuthError::NotPaired);
        };
        rt.expire(now);
        match rt.devices.find(token, now).map(|d| DeviceCtx { id: d.id.clone(), name: d.name.clone() }) {
            Some(d) => {
                if rt.devices.touch(&d.id, now) {
                    rt.save();
                }
                Ok(d)
            }
            None => {
                let _ = rt.bad_token_limit.hit(source, now);
                tracing::info!("remote access: a device cookie that matches no device");
                Err(AuthError::NotPaired)
            }
        }
    }

    pub fn remote_device(&self, id: &str) -> Option<DeviceView> {
        self.remote_access.lock().unwrap().devices.get(id).map(|d| d.view())
    }

    pub fn remote_devices(&self) -> Vec<DeviceView> {
        let mut rt = self.remote_access.lock().unwrap();
        rt.expire(now_ms());
        rt.devices.views()
    }

    /// Revoke a device (from the Mac) or log it out (from itself): its
    /// streams close now.
    pub fn remote_revoke(&self, id: &str, kind: Kind) -> bool {
        let mut rt = self.remote_access.lock().unwrap();
        let Some(d) = rt.devices.revoke(id) else { return false };
        rt.save();
        rt.record(&d.id, &d.name, kind);
        let _ = rt.kicks.send(Kick::Device(d.id.clone()));
        tracing::info!("remote access: device {} {}", d.id, if kind == Kind::Logout { "logged out" } else { "revoked" });
        true
    }

    pub fn remote_journal(&self, limit: usize) -> Vec<Entry> {
        let rt = self.remote_access.lock().unwrap();
        let skip = rt.journal.len().saturating_sub(limit);
        rt.journal[skip..].to_vec()
    }

    pub(crate) fn remote_record(&self, d: &DeviceCtx, kind: Kind) {
        self.remote_access.lock().unwrap().record(&d.id, &d.name, kind);
    }

    // ── Leaders ──────────────────────────────────────────────────────────

    /// A leader's main session: the newest live one with no thread.
    fn leader_info(&self, sb: &SandboxRef) -> Option<sbxw_core::agents::SessionInfo> {
        let key = sb.to_string();
        self.session_infos()
            .into_iter()
            .filter(|i| i.sandbox == key && i.thread.is_none())
            .max_by_key(|i| (i.state != SessionState::Exited, i.started_ms))
    }

    /// `:sandbox` as a current leader, or not found (the same answer as no
    /// such sandbox).
    pub fn remote_leader(&self, raw: &str) -> Result<SandboxRef, Refusal> {
        let sb = SandboxRef::parse(raw).map_err(|_| Refusal::NotFound)?;
        if self.leading().contains(&sb.to_string()) {
            Ok(sb)
        } else {
            Err(Refusal::NotFound)
        }
    }

    /// Whether the leader's pending prompt is a question the phone may
    /// answer: a multiple-choice `AskUserQuestion` in the one session sbxw
    /// can type into. A permission prompt has no steps: never.
    fn remote_question(&self, sb: &SandboxRef) -> Option<Vec<PhoneQuestion>> {
        let info = self.leader_info(sb)?;
        if info.state != SessionState::Attention {
            return None;
        }
        let steps = info.steps.filter(|s| !s.is_empty())?;
        let book = self.agents.lock().unwrap();
        let key = book.answerable(&sb.to_string()).ok()?;
        if book.prompt_sizes(&key).is_empty() {
            return None;
        }
        let rules = self.anonymise_rules().unwrap_or_default();
        let m = |t: &str| sbxw_core::anonymise::mask(t, &rules).text;
        Some(steps.iter().map(phone_question).map(|q| PhoneQuestion { text: m(&q.text), options: q.options.iter().map(|o| m(o)).collect() }).collect())
    }

    pub fn remote_leaders(&self) -> Vec<LeaderView> {
        let mut out = Vec::new();
        for name in self.leading() {
            let Ok(sb) = SandboxRef::parse(&name) else { continue };
            let Some(info) = self.leader_info(&sb) else { continue };
            let steps = self.remote_question(&sb);
            let last_at = info
                .session_id
                .as_deref()
                .and_then(|sid| self.history_page(&sb, sid, None, Some(1)))
                .and_then(|p| p.items.last().map(|i| i.at))
                .unwrap_or(info.started_ms);
            out.push(LeaderView {
                sandbox: name.clone(),
                name,
                state: phone_state(info.state),
                activity: phone_activity(info.state, steps.is_some()),
                last_at,
                asks: info.state == SessionState::Attention,
                answerable: steps.is_some(),
                steps,
            });
        }
        out
    }

    /// The leader's main session id, for its history.
    pub fn remote_leader_session(&self, sb: &SandboxRef) -> Option<String> {
        self.leader_info(sb)?.session_id
    }

    pub fn remote_history(&self, sb: &SandboxRef, before: Option<u64>, after: Option<u64>, limit: Option<usize>) -> Result<(String, PhonePage), Refusal> {
        let sid = self.remote_leader_session(sb).ok_or(Refusal::NotFound)?;
        let max = sbxw_core::history::limits::PAGE_MAX;
        let limit = limit.unwrap_or(sbxw_core::history::limits::PAGE_DEFAULT).clamp(1, max);
        let page = match after {
            Some(_) => self.history_page(sb, &sid, None, Some(max)),
            None => self.history_page(sb, &sid, before, Some(limit)),
        }
        .ok_or(Refusal::NotFound)?;
        let mut view = phone_page(&page, after);
        if after.is_some() && view.items.len() > limit {
            view.items.truncate(limit);
        }
        Ok((sid, view))
    }

    // ── Acting on a leader ───────────────────────────────────────────────

    /// Type `text` into the leader as the person: masked first, then the
    /// refusals `chat_push` applies to a named sandbox; never creates one.
    pub async fn remote_message(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, text: &str) -> Result<usize, Refusal> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Refusal::Bad("empty message".into()));
        }
        let safe = self.anonymise(text).map_err(Refusal::Unavailable)?;
        if let Err(e) = self.agents.lock().unwrap().can_type_into(&sb.to_string()) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}': {e}")));
        }
        if !self.sessions.is_live(&SessionKey::agent(sb)) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}' has no live agent session")));
        }
        let n = safe.text.len();
        match self.push_as_person(sb, SessionKind::Agent, &safe.text).await {
            Ok(()) => {
                tracing::info!("remote access: device {} sent a message to '{sb}' ({n} bytes, {} masked)", d.id, safe.count);
                self.remote_record(d, Kind::Message);
                Ok(safe.count)
            }
            Err(e) => {
                self.remote_record(d, Kind::Refused);
                Err(Refusal::Conflict(e))
            }
        }
    }

    pub async fn remote_answer(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, indices: Vec<u32>) -> Result<(), Refusal> {
        use crate::svc::agents::AnswerError;
        if indices.is_empty() || indices.len() > 20 || indices.iter().any(|&i| i == 0 || i > 50) {
            return Err(Refusal::Bad("indices: one 1-based option per step".into()));
        }
        let Some(steps) = self.remote_question(sb) else {
            let asks = self.leader_info(sb).is_some_and(|i| i.state == SessionState::Attention);
            self.remote_record(d, Kind::Refused);
            return Err(if asks { Refusal::Conflict("this prompt can only be answered at the Mac".into()) } else { Refusal::NotFound });
        };
        if indices.len() != steps.len() {
            return Err(Refusal::Bad(format!("the question has {} step(s)", steps.len())));
        }
        match self.answer(sb, SessionKind::Agent, indices).await {
            Ok(()) => {
                tracing::info!("remote access: device {} answered '{sb}'", d.id);
                self.remote_record(d, Kind::Answer);
                Ok(())
            }
            Err(AnswerError::NoSession) => Err(Refusal::NotFound),
            Err(AnswerError::NoIndices) => Err(Refusal::Bad("no option chosen".into())),
            Err(AnswerError::Ambiguous(why)) => {
                self.remote_record(d, Kind::Refused);
                Err(Refusal::Conflict(why))
            }
        }
    }

    /// One Escape into the leader's main session: what a person presses to
    /// interrupt Claude Code. Nothing else is typed.
    pub async fn remote_interrupt(&self, d: &DeviceCtx, sb: &SandboxRef) -> Result<(), Refusal> {
        if self.input(sb, SessionKind::Agent, b"\x1b").await {
            tracing::info!("remote access: device {} interrupted '{sb}'", d.id);
            self.remote_record(d, Kind::Interrupt);
            Ok(())
        } else {
            Err(Refusal::NotFound)
        }
    }

    pub async fn remote_transcribe(self: &Arc<Self>, d: &DeviceCtx, body: &[u8]) -> Result<sbxw_core::anonymise::Masked, Refusal> {
        let samples = crate::svc::voice::samples_of(body).map_err(|e| Refusal::Bad(format!("{e}")))?;
        let text = self.voice_transcribe(samples).await.map_err(Refusal::Unavailable)?;
        let safe = self.anonymise(text.trim()).map_err(Refusal::Unavailable)?;
        tracing::info!("remote access: device {} transcribed {} bytes of audio", d.id, body.len());
        self.remote_record(d, Kind::Transcribe);
        Ok(safe)
    }

    // ── Keep awake ───────────────────────────────────────────────────────

    /// What `GET /api/remote` says of keeping the Mac awake.
    fn remote_keep_awake(&self, enabled: bool) -> &'static str {
        if !enabled {
            return "off";
        }
        if !cfg!(target_os = "macos") {
            return "not_macos";
        }
        if self.awake_on_battery() {
            "on_battery"
        } else {
            "holding"
        }
    }
}

/// Start the sidecar: fixed argv, no shell, a cleared environment (tsnet
/// reads secrets and switches from `TS_*`), no stdin, its stdout dropped and
/// its stderr logged at debug (state names only, by its contract).
fn spawn_sidecar(bin: &Path, forward: &str, state_dir: &Path, status_file: &Path) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--forward")
        .arg(forward)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--status-file")
        .arg(status_file)
        .arg("--hostname")
        .arg("sbxw")
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for key in ["HOME", "TMPDIR", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    let mut child = cmd.spawn()?;
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!("{SIDECAR_NAME}: {}", l.chars().take(200).collect::<String>());
            }
        });
    }
    Ok(child)
}
