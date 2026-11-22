//! Remote access (`docs/security.md#remote-access`): the state on disk, the remote
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
use sbxw_core::remote_access::pairing::{ApproveError, PairingBook, RequestView, StartError, Status};
use sbxw_core::remote_access::reach::{self, ReachKind};
use sbxw_core::remote_access::view::{phone_activity, phone_page_with, phone_question, phone_state, PhonePage, PhoneQuestion};
use sbxw_core::remote_access::{self as rules, State, TailnetStatus};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::SandboxRef;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The sidecar, next to the daemon's own executable (no PATH lookup).
pub const SIDECAR_NAME: &str = "sbxw-tailnet";

/// Where the daemon looks for the sidecar: next to its own executable.
pub fn sidecar_beside() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(SIDECAR_NAME)))
}

/// What to say when the sidecar is not at `path`: the path looked at and
/// the one command that builds it there (from the sbxw source folder).
pub fn sidecar_missing_message(path: &Path) -> String {
    let goos = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    let goarch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        a => a,
    };
    format!(
        "{SIDECAR_NAME} is missing: sbxw looks for it at {p}\n\
         It is built by scripts/build-tailnet.sh. From the sbxw source folder, build it in place:\n  \
         scripts/build-tailnet.sh {goos}/{goarch} '{p}'",
        p = path.display()
    )
}

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
    /// Drops unsent uploads past their life, every minute.
    sweeper: tokio::task::JoinHandle<()>,
    addr: std::net::SocketAddr,
}

pub struct RemoteRuntime {
    pub(crate) dir: PathBuf,
    pub(crate) enabled: bool,
    pub(crate) devices: DeviceBook,
    journal: Vec<Entry>,
    pairing: PairingBook,
    pair_limit: Limiter,
    status_limit: Limiter,
    bad_token_limit: Limiter,
    /// Each paired device's own rate, keyed by device id: protects the
    /// daemon's work without letting one client starve another.
    device_limit: Limiter,
    listener: Option<Listener>,
    /// Files phones handed to leaders, not yet sent (`remote_upload`).
    pub(crate) uploads: sbxw_core::remote_access::uploads::UploadBook,
    /// Images phones sent, kept 7 days for their conversation
    /// (`svc::remote_sent`).
    pub(crate) sent: crate::svc::remote_sent::SentStore,
    /// Each device's upload rate, keyed by device id.
    pub(crate) upload_limit: Limiter,
    /// Devices with an upload in progress (one at a time each).
    pub(crate) uploading: std::collections::HashSet<String>,
    /// One copy into a sandbox at a time, for the whole daemon: a copy holds
    /// its file in memory (the sandbox writer takes bytes).
    pub(crate) copy_lock: Arc<tokio::sync::Mutex<()>>,
    /// The longest gap between two chunks of an upload (tests shorten it).
    pub(crate) upload_idle: Duration,
    pub(crate) sidecar: Option<tokio::process::Child>,
    pub sidecar_program: Sidecar,
    tailnet: Option<(u64, Option<TailnetStatus>)>,
    /// Tests: the tailnet name, as if a sidecar served it.
    pub tailnet_override: Option<String>,
    /// Tests: bind here instead of `remote_addr` (port 0 picks one).
    pub addr_override: Option<String>,
    kicks: tokio::sync::broadcast::Sender<Kick>,
    /// Level 2: approving with a passkey (`svc::remote_approvals`).
    pub(crate) level2: crate::svc::remote_approvals::Level2,
    /// Web Push: the key, the subscriptions, the sidecar's pipe
    /// (`svc::remote_push`).
    pub(crate) push: crate::svc::remote_push::Push,
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
        // Handles live in memory only: a spool left by an earlier run has
        // no owner any more.
        let _ = std::fs::remove_dir_all(dir.join(crate::svc::remote_upload::SPOOL_DIR));
        let level2 = crate::svc::remote_approvals::Level2::load(&dir, state.approvals && state.enabled);
        let push = crate::svc::remote_push::Push::load(&dir, |id| state.devices.iter().any(|d| d.id == id));
        let mut rt = RemoteRuntime {
            dir: dir.clone(),
            enabled: state.enabled,
            devices: DeviceBook::from_devices(state.devices),
            journal,
            pairing: PairingBook::new(),
            pair_limit: Limiter::new(60_000, 5, 20),
            status_limit: Limiter::new(60_000, 120, 600),
            bad_token_limit: Limiter::new(60_000, 10, 30),
            device_limit: Limiter::new(60_000, 600, u32::MAX),
            listener: None,
            uploads: Default::default(),
            sent: crate::svc::remote_sent::SentStore::open(&dir, now_ms()),
            upload_limit: Limiter::new(sbxw_core::remote_access::uploads::RATE_WINDOW_MS, sbxw_core::remote_access::uploads::RATE_PER_DEVICE, u32::MAX),
            uploading: Default::default(),
            copy_lock: Default::default(),
            upload_idle: crate::svc::remote_upload::CHUNK_IDLE,
            sidecar: None,
            sidecar_program: Sidecar::Beside,
            tailnet: None,
            tailnet_override: None,
            addr_override: None,
            kicks,
            level2,
            push,
        };
        // A passkey whose device is gone goes too.
        let RemoteRuntime { devices, level2, dir, .. } = &mut rt;
        if level2.passkeys.retain_devices(|id| devices.get(id).is_some()) {
            level2.save_passkeys(dir);
        }
        rt.expire(now_ms());
        rt
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Remote access is on and `id` is still a paired device.
    pub(crate) fn has_device(&self, id: &str) -> bool {
        self.enabled && self.devices.get(id).is_some()
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

    pub(crate) fn save(&self) {
        let state = State { enabled: self.enabled, devices: self.devices.devices().to_vec(), approvals: self.level2.enabled };
        let bytes = serde_json::to_vec_pretty(&state).expect("serialisable");
        if let Err(e) = write_private(&self.dir, &self.state_path(), &bytes) {
            tracing::warn!("remote access: cannot write state.json: {e}");
        }
    }

    pub(crate) fn record(&mut self, device_id: &str, device_name: &str, kind: Kind) {
        self.record_sized(device_id, device_name, kind, None);
    }

    /// An entry with a size class (`upload` only): never a name or a size.
    pub(crate) fn record_sized(&mut self, device_id: &str, device_name: &str, kind: Kind, size: Option<sbxw_core::remote_access::uploads::SizeClass>) {
        self.journal.push(Entry { at_ms: now_ms(), device_id: device_id.into(), device_name: device_name.into(), kind, size, target: None });
        self.write_journal();
    }

    /// A journal entry naming what was acted on (a plan id, a sandbox name,
    /// a host), never its content.
    pub(crate) fn record_target(&mut self, device_id: &str, device_name: &str, kind: Kind, target: Option<String>) {
        self.journal.push(Entry { at_ms: now_ms(), device_id: device_id.into(), device_name: device_name.into(), kind, size: None, target });
        self.write_journal();
    }

    fn write_journal(&mut self) {
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
            if self.level2.forget_device(&d.id) {
                self.level2.save_passkeys(&self.dir);
            }
            tracing::info!("remote access: device {} dropped after 30 days idle", d.id);
            let dir = self.dir.clone();
            self.push.forget_device(&dir, &d.id);
            let files = self.uploads.drop_device(&d.id);
            crate::svc::remote_upload::remove_spools(&self.dir, files);
            self.sent.drop_device(&d.id);
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

    /// A sidecar child is alive.
    pub(crate) fn sidecar_running(&mut self) -> bool {
        self.sidecar_pid().is_some()
    }

    /// The tailnet name the Host and Origin checks may use, if the sidecar
    /// serves one. Re-read at most every 2 s.
    pub(crate) fn serving_name(&mut self) -> Option<String> {
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
    /// Devices with notifications on (a push subscription).
    pub push_devices: usize,
    /// The path looked at, when the sidecar is not there.
    pub sidecar_missing: Option<String>,
    /// The team leaders a paired phone can see now (sandbox names).
    pub leaders: Vec<String>,
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

/// The fixed refusals of a message, each with a code the page can show.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Reason {
    /// The leader waits for an answer at the Mac (a permission, a question,
    /// a dialog), or its state is not known: 409.
    NeedsMac,
    /// A tool call was announced in the last seconds (its permission prompt
    /// may be on its way), or, for a line other than a message, the leader
    /// is working: 409.
    Busy,
    /// The leader's terminal has bracketed paste off: 409, fail closed.
    NoPaste,
    /// Typed, but the leader started asking something before the Return:
    /// the text waits unsent in the leader's input: 409.
    NotSubmitted,
    /// A line starts with `!`, `/` or `#`: 400.
    Command,
    /// A control character other than a line feed: 400.
    Control,
    /// Empty: 400.
    Empty,
    /// An upload over the per-file cap: 413.
    TooLarge,
    /// No room for one more file, or more ids than a message takes: 409 on
    /// an upload, 400 on a message.
    TooMany,
    /// A message names more files than it may: 400.
    TooManyIds,
    /// A refused kind of file (a program, an archive): 400.
    FileKind,
    /// A file name missing or far too long: 400.
    Name,
    /// An attachment id that is not this device's and leader's, spent,
    /// expired or unknown: 400, one answer for all.
    Attachment,
    /// This device already has an upload in progress: 409.
    Uploading,
    /// The upload's body was too slow: 408.
    Timeout,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::NeedsMac => "needs_mac",
            Reason::Busy => "busy",
            Reason::NoPaste => "no_paste",
            Reason::NotSubmitted => "not_submitted",
            Reason::Command => "command",
            Reason::Control => "control",
            Reason::Empty => "empty",
            Reason::TooLarge => "too_large",
            Reason::TooMany | Reason::TooManyIds => "too_many",
            Reason::FileKind => "kind",
            Reason::Name => "name",
            Reason::Attachment => "attachment",
            Reason::Uploading => "busy",
            Reason::Timeout => "timeout",
        }
    }
    pub fn status_conflict(self) -> bool {
        matches!(self, Reason::NeedsMac | Reason::Busy | Reason::NoPaste | Reason::NotSubmitted | Reason::TooMany | Reason::Uploading)
    }
    /// The HTTP status the route answers with.
    pub fn http_status(self) -> u16 {
        match self {
            Reason::TooLarge => 413,
            Reason::Timeout => 408,
            r if r.status_conflict() => 409,
            _ => 400,
        }
    }
    pub fn sentence(self) -> &'static str {
        match self {
            Reason::NeedsMac => "This needs your Mac: the session is waiting for an answer there.",
            Reason::Busy => "The session is busy right now: try again in a few seconds. Meanwhile you can read it and interrupt it.",
            Reason::NoPaste => "The session's terminal does not take a paste right now. Send it from the Mac.",
            Reason::NotSubmitted => "Not sent: the session started asking something at the Mac while the text was typed. It waits, unsent, in the session's input.",
            Reason::Command => "A message from the phone cannot have a line starting with !, / or #: shell and slash commands are for the Mac.",
            Reason::Control => "The message holds a control character. Only plain text and line breaks are sent.",
            Reason::Empty => "The message is empty.",
            Reason::TooLarge => "The file is too large: a phone can hand over files of up to 25 MB.",
            Reason::TooMany => "No room for another file: at most 5 files (60 MB together) can wait to be sent. Send or remove one first.",
            Reason::TooManyIds => "A message can carry at most 5 files.",
            Reason::FileKind => "This kind of file is refused from a phone: programs and archives (zip, tar, gzip, 7z, rar...). Send the files themselves, or use the Mac.",
            Reason::Name => "The file has no usable name.",
            Reason::Attachment => "A file of this message is no longer there: choose it again.",
            Reason::Uploading => "Another file is still being sent from this phone. Wait for it to finish.",
            Reason::Timeout => "The file took too long to arrive. Try again on a better connection.",
        }
    }
}

/// A tool call announced this recently may still bring a permission prompt
/// whose hook has not arrived: no message is typed meanwhile.
pub const TOOL_CALL_QUIET_MS: u64 = 3_000;
/// After the paste has echoed, how long to wait for a late hook before the
/// state is checked again and the Return sent.
pub const HOOK_GRACE: Duration = Duration::from_millis(600);

/// What a phone is refused, and why, in a status the route maps.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotFound,
    /// A refusal the page shows by its fixed code (`reason` in the answer).
    Reason(Reason),
    Forbidden(String),
    Conflict(String),
    Unavailable(String),
    Bad(String),
    Limited(u64),
    /// Something on the Mac failed (a copy into the sandbox): 502.
    Failed(String),
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct LeaderView {
    pub sandbox: String,
    pub name: String,
    /// `leader` (a team leader) or `sandbox` (a running sandbox with no team).
    pub kind: ReachKind,
    /// Why the phone can only read this one, when it can: `several_sessions`
    /// (sbxw cannot tell which session its terminal shows) or `no_terminal`
    /// (sbxw holds no terminal on it). The routes refuse by their own rules;
    /// this only lets the page say so before the person writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<&'static str>,
    pub state: &'static str,
    pub activity: &'static str,
    pub last_at: u64,
    pub asks: bool,
    pub answerable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<PhoneQuestion>>,
    /// The model the session's status line last reported (an id), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct PairingCode {
    pub code: String,
    /// What the requests made with this code carry (`code_id`).
    pub code_id: String,
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
        // Before the remote-access lock: the leaders take their own.
        let leaders = self.remote_leader_names();
        let mut rt = self.remote_access.lock().unwrap();
        let sidecar_missing = match rt.sidecar_program {
            Sidecar::Beside => sidecar_beside().filter(|p| !p.is_file()).map(|p| p.display().to_string()),
            _ => None,
        };
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
            push_devices: rt.push.devices_on(),
            sidecar_missing,
            leaders,
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
        // The one push trigger: it watches the bus whether remote access is
        // on or not (the edges stay current), and pushes only while on.
        self.remote_push_task();
        if self.remote_enabled() {
            if let Err(e) = self.remote_start().await {
                tracing::warn!("remote access: could not start: {e}");
            }
            self.remote_awake_changed();
        }
    }

    /// Start what is not running: the listener, the sidecar.
    pub(crate) async fn remote_start_again(self: &Arc<Self>) -> Result<(), String> {
        self.remote_start().await
    }

    async fn remote_start(self: &Arc<Self>) -> Result<(), String> {
        let (addr, program, state_dir, status_path, running) = {
            let mut rt = self.remote_access.lock().unwrap();
            let running = rt.listener.as_ref().is_some_and(|l| !l.task.is_finished());
            let addr = rt.addr_override.clone().unwrap_or_else(|| self.cfg.remote_addr.clone());
            let _ = rt.sidecar_pid();
            (addr, rt.sidecar_program.clone(), rt.tailnet_state_dir(), rt.tailnet_status_path(), running)
        };
        // `Config::validate` refused a non-loopback `remote_addr`; checked
        // again here, as the last step before a socket exists.
        if !addr.parse::<std::net::SocketAddr>().is_ok_and(|a| a.ip().is_loopback()) {
            return Err(format!("remote_addr {addr} is not a loopback address"));
        }
        let sidecar_bin = match &program {
            Sidecar::Beside => {
                let p = sidecar_beside().ok_or("cannot find the sbxw executable's folder")?;
                if !p.is_file() {
                    return Err(sidecar_missing_message(&p));
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
            let me = Arc::downgrade(self);
            let sweeper = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tick.tick().await;
                    let Some(app) = me.upgrade() else { break };
                    app.remote_sweep_uploads();
                }
            });
            tracing::info!("remote access: listening on {local} (reached only through {SIDECAR_NAME})");
            self.remote_access.lock().unwrap().listener = Some(Listener { shutdown: tx, task, sweeper, addr: local });
        }
        let has_sidecar = self.remote_access.lock().unwrap().sidecar_pid().is_some();
        if let (Some(bin), false) = (sidecar_bin, has_sidecar) {
            let local = self.remote_access.lock().unwrap().listener.as_ref().map(|l| l.addr.to_string()).unwrap_or(addr);
            // With `--push` only when a phone has notifications on.
            let push = self.remote_access.lock().unwrap().push.wanted();
            match spawn_sidecar(&bin, &local, &state_dir, &status_path, push.is_some()) {
                Ok(mut child) => {
                    tracing::info!("remote access: started {SIDECAR_NAME} (pid {:?}){}", child.id(), if push.is_some() { " with notifications" } else { "" });
                    if let (Some(config), Some(stdin), Some(stdout)) = (push, child.stdin.take(), child.stdout.take()) {
                        self.remote_push_attach(config, stdin, stdout);
                    }
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
            let files = rt.uploads.clear();
            crate::svc::remote_upload::remove_spools(&rt.dir, files);
            rt.sent.clear();
            // Level 2 goes off with it.
            let was = rt.level2.enabled;
            rt.level2.turn_off();
            if was {
                rt.save();
            }
        }
        self.remote_stop().await;
        self.remote_awake_changed();
        self.remote_status()
    }

    async fn remote_stop(&self) {
        let (listener, sidecar, kicks, status_path) = {
            let mut rt = self.remote_access.lock().unwrap();
            // The pipe goes first, so its close on stop is not reported.
            rt.push.detach();
            (rt.listener.take(), rt.sidecar.take(), rt.kicks.clone(), rt.tailnet_status_path())
        };
        let _ = kicks.send(Kick::All);
        if let Some(l) = listener {
            l.sweeper.abort();
            let _ = l.shutdown.send(true);
            // Streams were told to end; a connection that lingers is cut.
            let mut task = l.task;
            if tokio::time::timeout(Duration::from_secs(2), &mut task).await.is_err() {
                tracing::debug!("remote access: listener did not drain in 2 s, aborted");
                task.abort();
            }
        }
        if let Some(c) = sidecar {
            stop_sidecar(c, &status_path).await;
        }
        self.remote_access.lock().unwrap().tailnet = None;
    }

    /// "Forget this node" (Q24): with remote access off, the sidecar logs
    /// the node out and deletes its state. Refused while on.
    pub async fn remote_forget_node(&self) -> Result<(), String> {
        let (enabled, program, state_dir, status_path) = {
            let rt = self.remote_access.lock().unwrap();
            (rt.enabled, rt.sidecar_program.clone(), rt.tailnet_state_dir(), rt.tailnet_status_path())
        };
        if enabled {
            return Err("turn remote access off first".into());
        }
        let bin = match program {
            Sidecar::Beside => sidecar_beside().filter(|p| p.is_file()),
            Sidecar::Program(p) => Some(p),
            Sidecar::None => None,
        };
        let Some(bin) = bin else {
            if state_dir.exists() {
                let p = sidecar_beside().unwrap_or_else(|| PathBuf::from(SIDECAR_NAME));
                return Err(format!("{}\nIt is needed to log the node out.", sidecar_missing_message(&p)));
            }
            return Ok(());
        };
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("forget").arg("--state-dir").arg(&state_dir).arg("--status-file").arg(&status_path).env_clear();
        for key in ["HOME", "TMPDIR", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy"] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        let status = tokio::time::timeout(Duration::from_secs(30), cmd.status()).await.map_err(|_| format!("{SIDECAR_NAME} forget took over 30 s"))?.map_err(|e| e.to_string())?;
        if !status.success() || state_dir.exists() {
            return Err(format!("{SIDECAR_NAME} forget did not finish (exit {:?}); the node state is still at {}", status.code(), state_dir.display()));
        }
        tracing::info!("remote access: the tailnet node was forgotten");
        Ok(())
    }

    // ── Pairing (the Mac's side) ─────────────────────────────────────────

    pub fn remote_pairing_code(&self) -> Result<PairingCode, String> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.enabled {
            return Err("remote access is off: turn it on first".into());
        }
        let Some(name) = rt.serving_name() else {
            return Err("the tailnet machine is not serving yet: `sbxw remote status` says what it waits for".into());
        };
        let (code, code_id, expires_ms) = rt.pairing.new_code(now_ms());
        tracing::info!("remote access: new pairing code (valid 5 minutes; earlier requests voided)");
        Ok(PairingCode { url: format!("https://{name}/#pair={code}"), code, code_id, expires_ms })
    }

    pub fn remote_pairing_requests(&self) -> Vec<RequestView> {
        self.remote_access.lock().unwrap().pairing.pending(now_ms())
    }

    /// The person approves on the Mac with the six digits the phone shows.
    pub fn remote_pairing_approve(&self, id: &str, check: &str) -> Result<(), ApproveError> {
        let mut rt = self.remote_access.lock().unwrap();
        match rt.pairing.approve(id, check, now_ms()) {
            Ok(_) => {
                tracing::info!("remote access: pairing request approved on the Mac");
                Ok(())
            }
            Err(ApproveError::Refused) => {
                rt.record("", "", Kind::Refused);
                tracing::info!("remote access: pairing request refused after wrong digits");
                Err(ApproveError::Refused)
            }
            Err(e) => Err(e),
        }
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

    /// A phone starts pairing. The code is compared first: the request that
    /// holds the valid code is never limited by other clients' failures;
    /// only failures are counted and limited.
    pub fn remote_pair_start(&self, source: &str, code: &str, device_name: &str) -> Result<RequestView, Refusal> {
        let mut rt = self.remote_access.lock().unwrap();
        let now = now_ms();
        if !rt.pairing.code_valid(code, now) {
            rt.pair_limit.check(source, now).map_err(Refusal::Limited)?;
            let _ = rt.pair_limit.hit(source, now);
            tracing::info!("remote access: pairing start with a code that is not valid");
            return Err(Refusal::Forbidden("this pairing code is not valid".into()));
        }
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
        // A request id a phone was given is never limited; unknown ones are.
        if !rt.pairing.knows(id, now) {
            rt.status_limit.check(source, now).map_err(Refusal::Limited)?;
            let _ = rt.status_limit.hit(source, now);
            return Ok((Status::Expired, None));
        }
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
    /// The device a presented cookie belongs to. The token is looked up
    /// first (a hash and at most 32 comparisons): a paired device is limited
    /// only by its own rate (`device_limit`), never by other clients'
    /// failures. Only failures are counted against their source and in all.
    pub fn remote_auth(&self, source: &str, token: Option<&str>) -> Result<DeviceCtx, AuthError> {
        let mut rt = self.remote_access.lock().unwrap();
        let now = now_ms();
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return Err(AuthError::NotPaired);
        };
        rt.expire(now);
        let files = rt.uploads.expire(now);
        crate::svc::remote_upload::remove_spools(&rt.dir, files);
        match rt.devices.find(token, now).map(|d| DeviceCtx { id: d.id.clone(), name: d.name.clone() }) {
            Some(d) => {
                let id = d.id.clone();
                rt.device_limit.hit(&id, now).map_err(AuthError::Limited)?;
                if rt.devices.touch(&d.id, now) {
                    rt.save();
                }
                Ok(d)
            }
            None => {
                rt.bad_token_limit.check(source, now).map_err(AuthError::Limited)?;
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
        let dir = rt.dir.clone();
        rt.push.forget_device(&dir, &d.id);
        if rt.level2.forget_device(&d.id) {
            let dir = rt.dir.clone();
            rt.level2.save_passkeys(&dir);
        }
        rt.save();
        let files = rt.uploads.drop_device(&d.id);
        crate::svc::remote_upload::remove_spools(&rt.dir, files);
        rt.sent.drop_device(&d.id);
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

    // ── Who a phone reaches ──────────────────────────────────────────────

    /// Every sandbox a phone reaches now, each with its kind and the session
    /// it shows and types into: the ONE rule, `sbxw_core::remote_access::reach`.
    pub(crate) fn remote_reachable(&self) -> Vec<(SandboxRef, ReachKind, sbxw_core::agents::SessionInfo)> {
        let leading = self.leading();
        let infos = self.session_infos();
        let cands = self.reach_candidates(&infos);
        reach::reachable(&cands, &leading)
            .into_iter()
            .filter_map(|r| Some((SandboxRef::parse(&r.sandbox).ok()?, r.kind, infos[r.index].clone())))
            .collect()
    }

    /// `infos` as the reach rule sees them, each marked when it is the
    /// session sbxw's terminal types into (`AgentBook::terminal_session`,
    /// the rule of `can_type_into`, never relaxed here). Shared with
    /// `plans::main_session`, so the phone and Lead Mode agree on which
    /// session is a sandbox's main one.
    pub(crate) fn reach_candidates<'a>(&self, infos: &'a [sbxw_core::agents::SessionInfo]) -> Vec<reach::Candidate<'a>> {
        let terminal: std::collections::HashMap<String, String> = {
            let book = self.agents.lock().unwrap();
            let mut names: Vec<&str> = infos.iter().map(|i| i.sandbox.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            names.into_iter().filter_map(|n| book.terminal_session(n).map(|k| (n.to_string(), k.session_id))).collect()
        };
        infos
            .iter()
            .map(|i| reach::Candidate { terminal: i.session_id.is_some() && terminal.get(&i.sandbox) == i.session_id.as_ref(), ..reach::Candidate::from(i) })
            .collect()
    }

    /// `sb` as the phone reaches it, or `None` (unknown, stopped, a thread's,
    /// Jarvis's: all the same).
    fn remote_reach(&self, sb: &SandboxRef) -> Option<(ReachKind, sbxw_core::agents::SessionInfo)> {
        self.remote_reachable().into_iter().find(|(s, _, _)| s == sb).map(|(_, k, i)| (k, i))
    }

    /// The session the phone shows for a reachable `sb`.
    fn reach_info(&self, sb: &SandboxRef) -> Option<sbxw_core::agents::SessionInfo> {
        self.remote_reach(sb).map(|(_, i)| i)
    }

    /// `:sandbox` of every level 1 route: a sandbox the phone reaches, or not
    /// found, the same answer as no such sandbox.
    pub fn remote_target(&self, raw: &str) -> Result<SandboxRef, Refusal> {
        let sb = SandboxRef::parse(raw).map_err(|_| Refusal::NotFound)?;
        match self.remote_reach(&sb) {
            Some(_) => Ok(sb),
            None => Err(Refusal::NotFound),
        }
    }

    /// `:sandbox` as a current team leader only (the Kanban's routes: a
    /// sandbox with no team has no board), or not found.
    pub fn remote_leader(&self, raw: &str) -> Result<SandboxRef, Refusal> {
        let sb = SandboxRef::parse(raw).map_err(|_| Refusal::NotFound)?;
        if self.remote_reach(&sb).is_some_and(|(k, _)| k == ReachKind::Leader) {
            Ok(sb)
        } else {
            Err(Refusal::NotFound)
        }
    }

    /// Whether the pending prompt of `info` (the session the phone shows) is
    /// a question the phone may answer: a multiple-choice `AskUserQuestion`
    /// in the one session sbxw can type into. A permission prompt has no
    /// steps: never.
    fn remote_question_of(&self, sb: &SandboxRef, info: &sbxw_core::agents::SessionInfo) -> Option<Vec<PhoneQuestion>> {
        if info.state != SessionState::Attention {
            return None;
        }
        let steps = info.steps.as_ref().filter(|s| !s.is_empty())?;
        let book = self.agents.lock().unwrap();
        let key = book.answerable(&sb.to_string()).ok()?;
        // The session sbxw would type into must be the main one, the one
        // whose question the phone shows.
        if info.session_id.as_deref() != Some(key.session_id.as_str()) || book.prompt_sizes(&key).is_empty() {
            return None;
        }
        drop(book);
        let rules = self.anonymise_rules().unwrap_or_default();
        let m = |t: &str| sbxw_core::anonymise::mask(t, &rules).text;
        Some(steps.iter().map(phone_question).map(|q| PhoneQuestion { text: m(&q.text), options: q.options.iter().map(|o| m(o)).collect(), multi: q.multi }).collect())
    }

    fn remote_question(&self, sb: &SandboxRef) -> Option<Vec<PhoneQuestion>> {
        let info = self.reach_info(sb)?;
        self.remote_question_of(sb, &info)
    }

    /// Why the phone can only read `sb`, if it can: the existing rules,
    /// looked at ahead (`can_type_into`, a live terminal of sbxw's).
    pub(super) fn remote_read_only(&self, sb: &SandboxRef) -> Option<&'static str> {
        if self.agents.lock().unwrap().can_type_into(&sb.to_string()).is_err() {
            return Some("several_sessions");
        }
        if !self.sessions.is_live(&SessionKey::agent(sb)) {
            return Some("no_terminal");
        }
        None
    }

    /// The names of the team leaders `remote_leaders` lists (for `sbxw
    /// remote status` and the Kanban), without the other sandboxes.
    pub fn remote_leader_names(&self) -> Vec<String> {
        self.remote_reachable().into_iter().filter(|(_, k, _)| *k == ReachKind::Leader).map(|(s, _, _)| s.to_string()).collect()
    }

    /// `GET /r/leaders`: every sandbox a phone reaches (the route keeps its
    /// name: leaders first, then running sandboxes with no team).
    pub fn remote_leaders(&self) -> Vec<LeaderView> {
        let mut out = Vec::new();
        for (sb, kind, info) in self.remote_reachable() {
            let name = sb.to_string();
            let steps = self.remote_question_of(&sb, &info);
            let last_at = info
                .session_id
                .as_deref()
                .and_then(|sid| self.history_page(&sb, sid, None, Some(1)))
                .and_then(|p| p.items.last().map(|i| i.at))
                .unwrap_or(info.started_ms);
            out.push(LeaderView {
                sandbox: name.clone(),
                name,
                kind,
                read_only: self.remote_read_only(&sb),
                state: phone_state(info.state),
                activity: phone_activity(info.state, steps.is_some()),
                last_at,
                asks: info.state == SessionState::Attention,
                answerable: steps.is_some(),
                steps,
                model: info.model.clone(),
            });
        }
        out
    }

    /// The main session id the phone shows for `sb`, for its history.
    pub fn remote_leader_session(&self, sb: &SandboxRef) -> Option<String> {
        self.reach_info(sb)?.session_id
    }

    /// A page of the conversation the phone shows, counted in the phone's
    /// rows (a run of steps is one), not in history items: the last `limit`
    /// rows older than `before`, or the oldest `limit` rows newer than
    /// `after` (`more` when others follow: read again from `latest_seq`).
    /// The page carries the log's `epoch`: rows of another epoch are not
    /// comparable (a daemon restart, a reset) and the phone reads afresh.
    pub fn remote_history(&self, device_id: &str, sb: &SandboxRef, before: Option<u64>, after: Option<u64>, limit: Option<usize>) -> Result<(String, PhonePage), Refusal> {
        let sid = self.remote_leader_session(sb).ok_or(Refusal::NotFound)?;
        let max = sbxw_core::history::limits::PAGE_MAX;
        // The images this device sent, still kept: found by where each file
        // sat in the sandbox.
        let key = sb.to_string();
        let now = now_ms();
        let kept = |ms: u64, name: &str| self.remote_access.lock().unwrap().sent.find(device_id, &key, ms, name, now);
        let limit = limit.unwrap_or(sbxw_core::history::limits::PAGE_DEFAULT).clamp(1, max);
        let view = match after {
            Some(a) => {
                let page = self.history_page_after(sb, &sid, a, Some(max)).ok_or(Refusal::NotFound)?;
                let mut view = phone_page_with(&page, Some(a), &kept);
                if view.items.len() > limit {
                    view.items.truncate(limit);
                    view.more = true;
                }
                if view.more {
                    if let Some(last) = view.items.last() {
                        view.latest_seq = last.seq;
                    }
                }
                view
            }
            None => {
                let page = self.history_page(sb, &sid, before, Some(max)).ok_or(Refusal::NotFound)?;
                let mut view = phone_page_with(&page, None, &kept);
                if view.items.len() > limit {
                    view.items.drain(..view.items.len() - limit);
                    view.next_before = view.items.first().map(|i| i.first_seq);
                }
                view
            }
        };
        Ok((sid, view))
    }

    // ── Acting on a leader ───────────────────────────────────────────────

    /// Whether typed text would land in the leader's prompt right now, and
    /// its terminal. Only an IDLE leader is typed into (review S2): at its
    /// prompt, its turn over, nothing running; Claude Code does not open a
    /// permission dialog on its own while it waits for input. Refused (fail
    /// closed) when the leader is not a leader any more, is working (`busy`:
    /// it can be read and interrupted, not written to), waits for the person
    /// or its state is unknown (`needs_mac`), its last history item is a
    /// prompt or a tool call under [`TOOL_CALL_QUIET_MS`], or its terminal has
    /// bracketed paste off.
    pub(super) fn remote_can_type(&self, sb: &SandboxRef) -> Result<Arc<crate::pty::PtySession>, Reason> {
        // Not reachable any more (stopped, or no longer a main session):
        // nothing is typed.
        let info = self.reach_info(sb).ok_or(Reason::NeedsMac)?;
        if info.state == SessionState::Working {
            return Err(Reason::Busy);
        }
        self.remote_can_type_in(sb, info)
    }

    /// [`Self::remote_can_type`] for a phone message, the only caller that
    /// checks again between the paste and the Return
    /// ([`Self::remote_message`]): a WORKING leader takes it too. Claude Code
    /// queues a message typed during a turn, as at the Mac's keyboard. Every
    /// other refusal holds: waiting for the person (Attention, a question, a
    /// permission prompt), exited, unknown, a tool call under
    /// [`TOOL_CALL_QUIET_MS`], bracketed paste off. Lines typed with no
    /// re-check before their Return (`/model`, `/lead`, a skill) keep the
    /// idle-only gate.
    pub(super) fn remote_can_type_message(&self, sb: &SandboxRef) -> Result<Arc<crate::pty::PtySession>, Reason> {
        let info = self.reach_info(sb).ok_or(Reason::NeedsMac)?;
        self.remote_can_type_in(sb, info)
    }

    /// The checks both gates share, on the state `info` read.
    fn remote_can_type_in(&self, sb: &SandboxRef, info: sbxw_core::agents::SessionInfo) -> Result<Arc<crate::pty::PtySession>, Reason> {
        match info.state {
            SessionState::Idle | SessionState::Working => {}
            SessionState::Attention | SessionState::Exited => return Err(Reason::NeedsMac),
        }
        if info.question.is_some() || info.steps.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(Reason::NeedsMac);
        }
        {
            let book = self.agents.lock().unwrap();
            if let Ok(key) = book.answerable(&sb.to_string()) {
                if !book.prompt_sizes(&key).is_empty() {
                    return Err(Reason::NeedsMac);
                }
            }
        }
        let sid = info.session_id.as_deref().ok_or(Reason::NeedsMac)?;
        if let Some(last) = self.history_page(sb, sid, None, Some(1)).and_then(|p| p.items.last().cloned()) {
            use sbxw_core::history::Kind as H;
            match last.kind {
                H::Permission | H::Question => return Err(Reason::NeedsMac),
                H::ToolCall if now_ms().saturating_sub(last.at) < TOOL_CALL_QUIET_MS => return Err(Reason::Busy),
                _ => {}
            }
        }
        let s = self.sessions.live(&SessionKey::agent(sb)).ok_or(Reason::NeedsMac)?;
        if !sbxw_core::terminal::bracketed_paste_enabled(&s.replay()) {
            return Err(Reason::NoPaste);
        }
        Ok(s)
    }

    /// Type `text` into the leader as the person (F1, F6 of the review):
    /// 1. refuse a text with a command line or a control character;
    /// 2. resolve the attachment ids (this device, this leader, live);
    /// 3. mask the text; the refusals `chat_push` applies to a named sandbox;
    /// 4. with files: check the leader takes text now, then copy each file
    ///    into the sandbox ([`App::remote_copy_attachments`]);
    /// 5. refuse unless typed text lands in the prompt now, or in Claude
    ///    Code's queue while the leader works
    ///    ([`Self::remote_can_type_message`]), under the session's typing lock;
    /// 6. build the exact string typed (the masked text, then the fixed block
    ///    of the daemon's own paths) and check it whole, last;
    /// 7. send it as ONE bracketed paste, wait for the echo and [`HOOK_GRACE`];
    /// 8. check the same state again: a leader that started waiting for the
    ///    person (Working or Idle to Attention, a question, a permission
    ///    prompt), or any other refusal: no Return (`not_submitted`). Working
    ///    to Idle (or back) is still a prompt: the Return goes;
    /// 9. only then the Return; the attachment ids are spent.
    ///
    /// On every refusal the ids stay usable (with their first expiry).
    /// Residual window: a dialog Claude Code opens between the last check and
    /// the Return whose hook has not reached sbxw within [`HOOK_GRACE`]. It
    /// is wider while the leader works (a tool call can ask for permission
    /// any moment) than at an idle prompt.
    pub async fn remote_message(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, text: &str, attachments: &[String]) -> Result<usize, Refusal> {
        use sbxw_core::remote_access::MessageRefusal as M;
        let reason = |m: M| match m {
            M::Empty => Reason::Empty,
            M::Command => Reason::Command,
            M::Control => Reason::Control,
        };
        let files = !attachments.is_empty();
        let words = !text.trim().is_empty();
        // A message of files alone has no text of the person's to check;
        // the whole typed string is checked below either way.
        if words || !files {
            rules::check_message(text).map_err(|m| Refusal::Reason(reason(m)))?;
        }
        if files {
            self.remote_resolve_attachments(d, sb, attachments)?;
        }
        let (masked, count) = if words {
            let safe = self.anonymise(text.trim()).map_err(Refusal::Unavailable)?;
            (safe.text, safe.count)
        } else {
            (String::new(), 0)
        };
        if let Err(e) = self.agents.lock().unwrap().can_type_into(&sb.to_string()) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}': {e}")));
        }
        let Some(live) = self.sessions.live(&SessionKey::agent(sb)) else {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Reason(Reason::NeedsMac));
        };
        // Files only reach a sandbox whose leader takes a message now; the
        // copy runs before the typing lock (it can take seconds).
        let paths = if files {
            if let Err(r) = self.remote_can_type_message(sb) {
                self.remote_record(d, Kind::Refused);
                return Err(Refusal::Reason(r));
            }
            self.remote_copy_attachments(d, sb, attachments).await?
        } else {
            vec![]
        };
        // The session's typing lock (shared with every other line writer:
        // chat push, sbxw's own lines, answers, Interrupt), then the state.
        let _one_at_a_time = live.typing.lock().await;
        let session = match self.remote_can_type_message(sb) {
            Ok(s) if Arc::ptr_eq(&s, &live) => s,
            Ok(_) => {
                self.remote_record(d, Kind::Refused);
                return Err(Refusal::Reason(Reason::NeedsMac));
            }
            Err(r) => {
                self.remote_record(d, Kind::Refused);
                return Err(Refusal::Reason(r));
            }
        };
        if files {
            // Dropped or expired while copying: nothing typed.
            self.remote_resolve_attachments(d, sb, attachments)?;
        }
        // The last check runs on the EXACT string typed: after masking (an
        // alias could start with `/`), with the block of paths, and after
        // `sanitize_typed`.
        let whole = sbxw_core::uploads::with_attachments(&masked, &paths);
        let typed = sbxw_core::terminal::sanitize_typed(&whole);
        if typed != whole {
            return Err(Refusal::Reason(Reason::Control));
        }
        rules::check_message(&typed).map_err(|m| Refusal::Reason(reason(m)))?;
        let mut rx = session.subscribe();
        session.write(&[sbxw_core::terminal::PASTE_START, typed.as_bytes(), sbxw_core::terminal::PASTE_END]).await;
        crate::typing::settle(&mut rx, Some(crate::typing::ECHO_TIMEOUT), crate::typing::ECHO_QUIET).await;
        tokio::time::sleep(HOOK_GRACE).await;
        match self.remote_can_type_message(sb) {
            Ok(again) if Arc::ptr_eq(&again, &session) => {}
            _ => {
                tracing::info!("remote access: device {}: message not submitted (the leader's state changed)", d.id);
                self.remote_record(d, Kind::Refused);
                return Err(Refusal::Reason(Reason::NotSubmitted));
            }
        }
        session.write(&[b"\r"]).await;
        if files {
            self.remote_spend_attachments(attachments);
        }
        tracing::info!("remote access: device {}: message", d.id);
        tracing::debug!("remote access: message to '{sb}' ({} bytes, {} masked, {} files)", typed.len(), count, paths.len());
        self.remote_record(d, Kind::Message);
        Ok(count)
    }

    /// The 409 of an answer typed but not taken; the page shows it as is.
    pub const ANSWER_NOT_TAKEN: &'static str = "not taken: the question is still open at the Mac, answer it there";

    /// `answers[n]`: the 1-based options chosen at step `n`, exactly one for
    /// a single-select step, one or more (each once) for a multi-select one.
    pub async fn remote_answer(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, answers: Vec<Vec<u32>>) -> Result<(), Refusal> {
        use crate::svc::agents::AnswerError;
        const SHAPE: &str = "answers: per step, the 1-based options chosen";
        if answers.is_empty() || answers.len() > 20 || answers.iter().any(|a| a.is_empty() || a.len() > 50 || a.iter().any(|&i| i == 0 || i > 50)) {
            return Err(Refusal::Bad(SHAPE.into()));
        }
        let Some(steps) = self.remote_question(sb) else {
            let asks = self.reach_info(sb).is_some_and(|i| i.state == SessionState::Attention);
            self.remote_record(d, Kind::Refused);
            return Err(if asks { Refusal::Conflict("this prompt can only be answered at the Mac".into()) } else { Refusal::NotFound });
        };
        if answers.len() != steps.len() {
            return Err(Refusal::Bad(format!("the question has {} step(s)", steps.len())));
        }
        for (n, (a, step)) in answers.iter().zip(&steps).enumerate() {
            let mut seen = a.clone();
            seen.sort_unstable();
            seen.dedup();
            if seen.len() != a.len() || a.iter().any(|&i| i as usize > step.options.len()) {
                return Err(Refusal::Bad(format!("step {}: {SHAPE}, each once and among its {} options", n + 1, step.options.len())));
            }
            if !step.multi && a.len() != 1 {
                return Err(Refusal::Bad(format!("step {}: one option only", n + 1)));
            }
        }
        match self.answer(sb, SessionKind::Agent, answers).await {
            Ok(()) => {
                tracing::info!("remote access: device {}: answer", d.id);
                self.remote_record(d, Kind::Answer);
                Ok(())
            }
            Err(AnswerError::NoSession) => Err(Refusal::NotFound),
            Err(AnswerError::NoIndices) => Err(Refusal::Bad("no option chosen".into())),
            Err(AnswerError::Ambiguous(why)) => {
                self.remote_record(d, Kind::Refused);
                Err(Refusal::Conflict(why))
            }
            Err(AnswerError::NotTaken) => {
                tracing::info!("remote access: device {}: answer not taken", d.id);
                self.remote_record(d, Kind::Refused);
                Err(Refusal::Conflict(Self::ANSWER_NOT_TAKEN.into()))
            }
        }
    }

    /// One Escape into the leader's main session: what a person presses to
    /// interrupt Claude Code. Nothing else is typed.
    pub async fn remote_interrupt(&self, d: &DeviceCtx, sb: &SandboxRef) -> Result<(), Refusal> {
        // Only where the daemon can tell which session the Escape reaches
        // (a read-only entry, API.md 2.6): several sessions none of which is
        // known to be the terminal's, or no terminal of sbxw's at all. A
        // sandbox with several sessions of which exactly one is the
        // terminal's is interrupted as before.
        if let Err(e) = self.agents.lock().unwrap().can_type_into(&sb.to_string()) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}': {e}")));
        }
        // Under the session's typing lock: an Escape never lands between a
        // phone message's paste and its Return.
        let Some(live) = self.sessions.live(&SessionKey::agent(sb)) else {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Reason(Reason::NeedsMac));
        };
        let _line = live.typing.lock().await;
        if self.input(sb, SessionKind::Agent, b"\x1b").await {
            tracing::info!("remote access: device {}: interrupt", d.id);
            tracing::debug!("remote access: interrupt of '{sb}'");
            self.remote_record(d, Kind::Interrupt);
            Ok(())
        } else {
            Err(Refusal::NotFound)
        }
    }

    /// `GET /r/models`: the closed list the phone may choose from (the same
    /// one a thread is started on), and the daemon's default.
    pub fn remote_models(&self) -> serde_json::Value {
        serde_json::json!({ "default": self.cfg.claude_model, "models": self.cfg.models })
    }

    /// Switch the model of the session the phone shows: types `/model <id>`
    /// for an id of the configured list, nothing else. The phone sends an id,
    /// never text: the line is built here, so the `/` rule of messages stays
    /// whole. Only at an idle prompt, as a message.
    pub async fn remote_set_model(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, id: &str) -> Result<(), Refusal> {
        let id = sbxw_core::ModelId::parse(id).map_err(|_| Refusal::Bad("model: not a model id".into()))?;
        if !self.cfg.models.iter().any(|m| m.id == id.as_str()) {
            return Err(Refusal::Bad("model: not in the list of models".into()));
        }
        if let Some(why) = self.remote_read_only(sb) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}' is read only from the phone ({why})")));
        }
        if let Err(r) = self.remote_can_type(sb) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Reason(r));
        }
        self.push_as_person(sb, SessionKind::Agent, &format!("/model {}", id.as_str())).await.map_err(Refusal::Conflict)?;
        tracing::info!("remote access: device {}: model", d.id);
        self.remote_record(d, Kind::Model);
        Ok(())
    }

    /// Turn lead mode on or off for a sandbox the phone reaches: the same
    /// calls as the Mac's button. On types `/lead` (so only at an idle
    /// prompt); off types nothing the person did not choose. Idempotent.
    pub async fn remote_set_lead(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, on: bool) -> Result<(), Refusal> {
        let Some((kind, _)) = self.remote_reach(sb) else { return Err(Refusal::NotFound) };
        let leading = kind == ReachKind::Leader;
        if leading == on {
            return Ok(());
        }
        if let Some(why) = self.remote_read_only(sb) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}' is read only from the phone ({why})")));
        }
        if on {
            if let Err(r) = self.remote_can_type(sb) {
                self.remote_record(d, Kind::Refused);
                return Err(Refusal::Reason(r));
            }
            self.lead(sb).await.map_err(Refusal::Conflict)?;
            tracing::info!("remote access: device {}: lead on", d.id);
            self.remote_record(d, Kind::LeadOn);
        } else {
            self.lead_end(sb).await;
            tracing::info!("remote access: device {}: lead off", d.id);
            self.remote_record(d, Kind::LeadOff);
        }
        Ok(())
    }

    pub async fn remote_transcribe(self: &Arc<Self>, d: &DeviceCtx, body: &[u8]) -> Result<sbxw_core::anonymise::Masked, Refusal> {
        let samples = crate::svc::voice::samples_of(body).map_err(|e| Refusal::Bad(e.to_string()))?;
        let text = self.voice_transcribe(samples).await.map_err(Refusal::Unavailable)?;
        let safe = self.anonymise(text.trim()).map_err(Refusal::Unavailable)?;
        tracing::info!("remote access: device {}: transcribe", d.id);
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

/// Stop a sidecar: SIGTERM first (a clean stop writes `stopped`), SIGKILL
/// after 5 s; then no stale status file (a missing one means "not serving").
pub(crate) async fn stop_sidecar(mut c: tokio::process::Child, status_path: &Path) {
    if let Some(pid) = c.id() {
        let _ = tokio::process::Command::new(sbxw_core::sysbin::sys_bin("kill")).args(["-TERM", &pid.to_string()]).status().await;
    }
    if tokio::time::timeout(Duration::from_secs(5), c.wait()).await.is_err() {
        let _ = c.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), c.wait()).await;
    }
    let _ = std::fs::remove_file(status_path);
}

/// Start the sidecar: fixed argv, no shell, a cleared environment (tsnet
/// reads secrets and switches from `TS_*`), its stderr logged at debug
/// (state names only, by its contract). Without `push`: no stdin, stdout
/// dropped. With `push`: `--push` last, stdin and stdout piped
/// (API-push.md section 1); the caller takes both.
fn spawn_sidecar(bin: &Path, forward: &str, state_dir: &Path, status_file: &Path, push: bool) -> std::io::Result<tokio::process::Child> {
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
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if push {
        cmd.arg("--push").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    }
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

#[cfg(test)]
impl App {
    /// Tests: remote access on, as if the sidecar served `tailnet`, without
    /// binding a socket or starting a sidecar.
    pub(crate) fn remote_test_on(&self, tailnet: &str) {
        let mut rt = self.remote_access.lock().unwrap();
        rt.enabled = true;
        rt.sidecar_program = Sidecar::None;
        rt.tailnet_override = Some(tailnet.to_string());
        rt.save();
    }

    /// Tests only: forget every failure counted by the remote limiters, so
    /// a walk over every route is not answered by the global failure cap.
    pub(crate) fn remote_reset_limits_for_tests(&self) {
        let mut rt = self.remote_access.lock().unwrap();
        rt.pair_limit.clear();
        rt.status_limit.clear();
        rt.bad_token_limit.clear();
        rt.device_limit.clear();
    }

    /// Tests: where the listener is bound, while it is.
    pub(crate) fn remote_listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.remote_access.lock().unwrap().listener.as_ref().map(|l| l.addr)
    }

    /// Tests: the hourly drop of idle devices, at `now`.
    pub(crate) fn remote_expire_for_tests(&self, now: u64) {
        self.remote_access.lock().unwrap().expire(now);
    }

    pub(crate) fn remote_dir_for_tests(&self) -> PathBuf {
        self.remote_access.lock().unwrap().dir.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbxw_core::session::SessionKey;

    #[test]
    fn the_sidecar_missing_message_names_the_path_and_the_command() {
        let p = Path::new("/Users/x/dev/sbxw/target/debug/sbxw-tailnet");
        let m = sidecar_missing_message(p);
        assert!(m.contains("looks for it at /Users/x/dev/sbxw/target/debug/sbxw-tailnet"), "{m}");
        assert!(m.contains("scripts/build-tailnet.sh "), "{m}");
        assert!(m.ends_with(" '/Users/x/dev/sbxw/target/debug/sbxw-tailnet'"), "{m}");
        assert!(m.lines().count() <= 3, "{m}");
        assert!(!m.contains('\u{2014}'), "no em dash");
    }

    #[tokio::test]
    async fn turning_on_without_the_sidecar_says_where_and_how() {
        let (_d, app) = crate::testing::app();
        let err = app.remote_enable().await.unwrap_err();
        let beside = sidecar_beside().unwrap();
        assert_eq!(err, sidecar_missing_message(&beside));
        assert!(!app.remote_enabled(), "nothing changed");
        let st = app.remote_status();
        assert_eq!(st.sidecar_missing.as_deref(), Some(beside.display().to_string().as_str()));
        assert!(app.remote_listen_addr().is_none(), "no listener bound");
    }

    #[tokio::test]
    async fn status_lists_the_leaders_a_phone_sees() {
        let (_d, app) = crate::testing::app();
        app.remote_test_on("sbxw.tail1.ts.net");
        assert!(app.remote_status().leaders.is_empty());
        assert_eq!(app.remote_status().sidecar_missing, None, "a test sidecar is never missing");
        let sb = SandboxRef::parse("lead1").unwrap();
        let (s, _) = app
            .sessions
            .get_or_open(&SessionKey::agent(&sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), "sleep 30".into()], env: vec![] }) })
            .await
            .unwrap();
        app.leads.lock().unwrap().insert(sb.clone(), Arc::as_ptr(&s) as usize);
        app.ingest_hook(&sb, serde_json::json!({"hook_event_name": "SessionStart", "session_id": "s1", "hook_version": 2}));
        assert_eq!(app.remote_status().leaders, vec!["lead1".to_string()]);
        assert_eq!(serde_json::to_value(app.remote_status()).unwrap()["leaders"], serde_json::json!(["lead1"]));
    }
}
