//! The virtual Mac, on the host: installing the pinned terminal-mac, booting
//! its VM, the one lease, the approvals, and pausing a VM nobody uses.
//!
//! Everything goes through the `terminal-mac` CLI, whose replies are JSON and
//! whose failures exit nonzero. Its program is `~/.sbxw/terminal-mac/bin/
//! terminal-mac` (sbxw's own install), or `SBXW_TERMINAL_MAC` — the tests'
//! fake, or a copy the person installed themselves.
//!
//! What a sandbox asks for that costs or exposes something (a boot, a port) is a
//! relay request of kind `Mac`: the person's yes runs it, and the request is
//! settled with what came of it, so the agent reads the outcome rather than
//! "approved".

use crate::app::{now_ms, App};
use crate::bus::Event;
use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::mac::{self, Acquire, Ask, Lifecycle, MacSettings, VmState};
use sbxw_core::relay::{RelayKind, RelayState};
use sbxw_core::sysbin::sys_bin;
use sbxw_core::{SandboxRef, ThreadName};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// One CLI call that is not a boot.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// `start`/`resume`: a cold boot of a macOS guest.
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);
/// After a boot command returns, how long the VM has to report `running`.
pub const BOOT_WAIT: Duration = Duration::from_secs(120);
const BOOT_POLL: Duration = Duration::from_secs(2);
/// `up --detach` on a new VM downloads macOS (several GB) and installs it.
const CREATE_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
/// The archive download and install.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// A screenshot larger than this is not sent into an agent's context.
const MAX_SHOT_BYTES: u64 = 12 * 1024 * 1024;

/// What a person's approval will run.
#[derive(Debug, Clone)]
pub struct Pending {
    pub sandbox: SandboxRef,
    pub ask: Ask,
}

/// The daemon's side of the Mac, beside the lease book.
#[derive(Default)]
pub struct MacRuntime {
    pub book: mac::MacBook,
    /// Requests waiting on a person, by relay id.
    pub pending: std::collections::HashMap<String, Pending>,
    /// What sbxw is doing to the VM right now (installing, booting…), for
    /// the UI; `None` when idle.
    pub busy: Option<String>,
    /// The last thing that went wrong, until something succeeds.
    pub error: Option<String>,
    /// Ports published for the guest: (sandbox, publish spec).
    pub ports: Vec<(SandboxRef, String)>,
    /// When sbxw last asked for a normal shutdown that has not finished:
    /// past `force_stop_after_minutes` still `stopping`, it is forced.
    pub stop_requested_ms: Option<u64>,
}

#[derive(Serialize, Debug, Clone)]
pub struct MacStatus {
    pub supported: bool,
    pub installed: bool,
    pub program: Option<String>,
    pub pinned: &'static str,
    pub vm: String,
    pub state: VmState,
    pub guest_ready: bool,
    /// `action status` as terminal-mac gave it, for the setup checklist.
    pub guest: Value,
    pub holder: Option<String>,
    pub managed: bool,
    pub busy: Option<String>,
    pub error: Option<String>,
    pub ports: Vec<String>,
    pub settings: MacSettings,
    /// When sbxw asked for the shutdown still under way, if it did.
    pub stop_requested_ms: Option<u64>,
}

/// What a CLI call came back with.
struct Reply {
    ok: bool,
    json: Value,
    text: String,
}

impl App {
    fn mac_settings(&self) -> &MacSettings {
        &self.cfg.mac
    }

    /// The terminal-mac program, when there is one to run.
    pub fn mac_program(&self) -> Option<PathBuf> {
        if let Some(p) = self.mac_program_override.lock().unwrap().clone() {
            return Some(p);
        }
        if let Some(p) = std::env::var_os("SBXW_TERMINAL_MAC").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        let own = self.mac_prefix().join("bin").join("terminal-mac");
        own.exists().then_some(own)
    }

    /// Where sbxw installs terminal-mac: its own prefix, never `~/.local`.
    pub fn mac_prefix(&self) -> PathBuf {
        self.store.paths.root.join("terminal-mac")
    }

    /// Apple silicon, or a program named explicitly (the tests' fake).
    pub fn mac_supported(&self) -> bool {
        mac::host_supported(std::env::consts::OS, std::env::consts::ARCH)
            || self.mac_program_override.lock().unwrap().is_some()
            || std::env::var_os("SBXW_TERMINAL_MAC").is_some_and(|p| !p.is_empty())
    }

    fn mac_changed(&self) {
        self.send(Event::Mac);
    }

    fn mac_set_busy(&self, what: Option<&str>) {
        self.mac.lock().unwrap().busy = what.map(str::to_string);
        self.mac_changed();
    }

    fn mac_set_error(&self, e: Option<String>) {
        self.mac.lock().unwrap().error = e;
        self.mac_changed();
    }

    async fn mac_run<S: AsRef<str> + Sync>(&self, args: &[S], timeout: Duration) -> Result<Reply> {
        let program = self.mac_program().ok_or_else(|| anyhow!("terminal-mac is not installed — install it from sbxw's Mac panel or with `sbxw mac install`"))?;
        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(args.iter().map(AsRef::as_ref)).stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| anyhow!("terminal-mac {} did not answer within {}s", args.first().map_or("", AsRef::as_ref), timeout.as_secs()))?
            .with_context(|| format!("running {}", program.display()))?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let json = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        let text = if out.status.success() || stderr.is_empty() { stdout } else { stderr };
        Ok(Reply { ok: out.status.success(), json, text })
    }

    /// The VM's state; `Missing` when terminal-mac does not know it.
    async fn mac_state(&self) -> VmState {
        let vm = self.mac_settings().vm.as_str();
        let state = match self.mac_run(&["status", vm], CALL_TIMEOUT).await {
            Ok(r) if r.ok => mac::parse_vm_state(&r.json),
            Ok(r) if r.text.to_ascii_lowercase().contains("not found") || r.text.to_ascii_lowercase().contains("no such") || r.text.to_ascii_lowercase().contains("does not exist") => VmState::Missing,
            _ => VmState::Unknown,
        };
        // Seen down: whoever starts it next, it is not the one sbxw brought
        // up, and lending it asks again.
        if matches!(state, VmState::Stopped | VmState::Missing) {
            self.mac.lock().unwrap().book.ours = false;
        }
        state
    }

    async fn mac_guest(&self) -> (bool, Value) {
        let vm = self.mac_settings().vm.as_str();
        match self.mac_run(&["action", vm, "status"], CALL_TIMEOUT).await {
            Ok(r) if r.ok => (mac::guest_ready(&r.json), r.json),
            Ok(r) => (false, json!({ "error": r.text })),
            Err(e) => (false, json!({ "error": e.to_string() })),
        }
    }

    pub async fn mac_status(&self) -> MacStatus {
        let supported = self.mac_supported();
        let program = self.mac_program();
        let installed = program.is_some();
        let (state, guest_ready, guest) = if installed {
            let state = self.mac_state().await;
            let (ready, guest) = if state == VmState::Running { self.mac_guest().await } else { (false, Value::Null) };
            (state, ready, guest)
        } else {
            (VmState::Missing, false, Value::Null)
        };
        let rt = self.mac.lock().unwrap();
        MacStatus {
            supported,
            installed,
            program: program.map(|p| p.to_string_lossy().into_owned()),
            pinned: mac::PIN.version,
            vm: self.mac_settings().vm.clone(),
            state,
            guest_ready,
            guest,
            holder: rt.book.holder().map(str::to_string),
            managed: rt.book.managed,
            busy: rt.busy.clone(),
            error: rt.error.clone(),
            ports: rt.ports.iter().map(|(sb, spec)| format!("{sb} {spec}")).collect(),
            settings: self.mac_settings().clone(),
            stop_requested_ms: rt.stop_requested_ms,
        }
    }

    // ── Install ───────────────────────────────────────────────────────────

    /// Download the pinned archive, check its SHA-256 against the one
    /// compiled in, and run its own installer into sbxw's prefix. Values go
    /// to the script as environment, never spliced into it.
    pub async fn mac_install(&self) -> Result<String> {
        if !mac::host_supported(std::env::consts::OS, std::env::consts::ARCH) {
            bail!("terminal-mac needs an Apple silicon Mac (macOS 14 or later)");
        }
        self.mac_set_busy(Some("installing terminal-mac"));
        const SCRIPT: &str = r#"set -euo pipefail
work="$(mktemp -d "${TMPDIR:-/tmp}/sbxw-terminal-mac.XXXXXX")"
trap 'rm -rf "$work"' EXIT
curl -fsSL "$TM_URL" -o "$work/a.zip"
echo "$TM_SHA  $work/a.zip" | shasum -a 256 -c - >/dev/null || { echo "checksum mismatch: nothing was installed" >&2; exit 1; }
ditto -x -k "$work/a.zip" "$work/u"
bash "$work/u/$TM_ARCHIVE/install.sh" --prefix "$TM_PREFIX"
"#;
        let mut cmd = tokio::process::Command::new(sys_bin("bash"));
        cmd.arg("-c")
            .arg(SCRIPT)
            .env("TM_URL", mac::PIN.url)
            .env("TM_SHA", mac::PIN.sha256)
            .env("TM_ARCHIVE", mac::PIN.archive)
            .env("TM_PREFIX", self.mac_prefix())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let res = tokio::time::timeout(INSTALL_TIMEOUT, cmd.output()).await;
        self.mac_set_busy(None);
        let out = res.map_err(|_| anyhow!("the terminal-mac install took longer than 15 minutes"))??;
        if !out.status.success() {
            let e = format!("installing terminal-mac failed: {}", String::from_utf8_lossy(&out.stderr).trim());
            self.mac_set_error(Some(e.clone()));
            bail!(e);
        }
        self.mac_set_error(None);
        tracing::info!("mac: terminal-mac {} installed under {}", mac::PIN.version, self.mac_prefix().display());
        Ok(format!("terminal-mac {} installed", mac::PIN.version))
    }

    // ── Boot and shutdown ─────────────────────────────────────────────────

    /// Bring the VM up from whatever state it is in. A VM that does not
    /// exist yet is created with `up --detach`: macOS is downloaded and
    /// installed, then the person finishes Setup Assistant in its window.
    pub async fn mac_boot(&self) -> Result<String> {
        if self.mac_program().is_none() {
            bail!("terminal-mac is not installed — install it first (`sbxw mac install`)");
        }
        if let Some(b) = self.mac.lock().unwrap().busy.clone() {
            bail!("already {b}");
        }
        let vm = self.mac_settings().vm.as_str();
        let state = self.mac_state().await;
        let (verb, busy, timeout): (&[&str], &str, Duration) = match state {
            VmState::Running => return Ok(format!("the virtual Mac '{vm}' is running")),
            VmState::Starting => return Ok(format!("the virtual Mac '{vm}' is starting")),
            VmState::Stopping => bail!(
                "the virtual Mac '{vm}' is still shutting down — wait for it to stop (if it hangs, look at its window: `terminal-mac show {vm}`)"
            ),
            VmState::Paused => (&["resume"], "resuming the VM", BOOT_TIMEOUT),
            VmState::Stopped | VmState::Unknown => (&["start"], "starting the VM", BOOT_TIMEOUT),
            VmState::Missing => (&["up", "--detach"], "creating the VM (downloading macOS)", CREATE_TIMEOUT),
        };
        let args: Vec<&str> = [verb[0], vm].into_iter().chain(verb[1..].iter().copied()).collect();
        self.mac_set_busy(Some(busy));
        let r = self.mac_run(&args, timeout).await;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                self.mac_set_busy(None);
                return Err(e);
            }
        };
        tracing::info!("mac: terminal-mac {} answered ({}): {}", verb[0], if r.ok { "ok" } else { "failed" }, r.text.chars().take(300).collect::<String>());
        if !r.ok {
            self.mac_set_busy(None);
            let e = format!("terminal-mac {}: {}", verb[0], r.text);
            self.mac_set_error(Some(e.clone()));
            bail!(e);
        }
        // A command that answered is not a VM that runs: wait until
        // terminal-mac itself says `running` before anyone gets the lease.
        let wait = *self.mac_boot_wait.lock().unwrap();
        let now = self.mac_wait_running(wait).await;
        self.mac_set_busy(None);
        if now != VmState::Running {
            let e = format!(
                "terminal-mac {} returned, but the VM is {} after {}s — open Setup → Mac, or run `terminal-mac status {vm}` on the host",
                verb[0],
                serde_json::to_value(now).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                wait.as_secs()
            );
            self.mac_set_error(Some(e.clone()));
            bail!(e);
        }
        self.mac.lock().unwrap().book.booted(now_ms());
        self.mac_set_error(None);
        tracing::info!("mac: '{vm}' {}", verb[0]);
        Ok(if state == VmState::Missing {
            format!("the virtual Mac '{vm}' was created; finish its one-time setup in its window (sbxw's Mac panel lists the steps)")
        } else {
            format!("the virtual Mac '{vm}' is up")
        })
    }

    /// Poll the VM's state until it runs, or `bound` passes.
    async fn mac_wait_running(&self, bound: Duration) -> VmState {
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            let s = self.mac_state().await;
            if s == VmState::Running || tokio::time::Instant::now() >= deadline {
                return s;
            }
            tokio::time::sleep(BOOT_POLL.min(bound)).await;
        }
    }

    /// A normal shutdown request. It is forced later, by the lifecycle loop,
    /// only if it hangs (see `mac::force_due`).
    pub async fn mac_stop(&self) -> Result<String> {
        let vm = self.mac_settings().vm.as_str();
        // A normal shutdown is asked of macOS inside the guest, and a paused
        // guest runs nothing: resume it first, or the request goes nowhere.
        if self.mac_state().await == VmState::Paused {
            let r = self.mac_run(&["resume", vm], BOOT_TIMEOUT).await?;
            if !r.ok {
                bail!("terminal-mac resume (before stopping): {}", r.text);
            }
            let wait = *self.mac_boot_wait.lock().unwrap();
            let now = self.mac_wait_running(wait).await;
            if now != VmState::Running {
                bail!("the virtual Mac '{vm}' did not resume before its shutdown (it is {now:?})");
            }
        }
        let r = self.mac_run(&["stop", vm], BOOT_TIMEOUT).await?;
        if !r.ok {
            bail!("terminal-mac stop: {}", r.text);
        }
        self.mac_unpublish_all().await;
        {
            let mut rt = self.mac.lock().unwrap();
            rt.book.stopped();
            rt.book.take_ended();
            rt.stop_requested_ms = Some(now_ms());
        }
        self.mac_changed();
        Ok(format!("the virtual Mac '{vm}' was asked to shut down"))
    }

    /// `terminal-mac stop --force`: the guest is powered off at once, and unsaved
    /// guest work is lost. A person's button, or the lifecycle loop for a
    /// shutdown of sbxw's that hung.
    pub async fn mac_force_off(&self) -> Result<String> {
        let vm = self.mac_settings().vm.as_str();
        let r = self.mac_run(&["stop", vm, "--force"], CALL_TIMEOUT).await?;
        if !r.ok {
            bail!("terminal-mac stop --force: {}", r.text);
        }
        self.mac_unpublish_all().await;
        {
            let mut rt = self.mac.lock().unwrap();
            rt.book.stopped();
            rt.book.take_ended();
            rt.stop_requested_ms = None;
        }
        self.mac_changed();
        tracing::warn!("mac: '{vm}' forced off");
        Ok(format!("the virtual Mac '{vm}' was forced off"))
    }

    pub async fn mac_pause(&self) -> Result<String> {
        let vm = self.mac_settings().vm.as_str();
        let r = self.mac_run(&["pause", vm], CALL_TIMEOUT).await?;
        if !r.ok {
            bail!("terminal-mac pause: {}", r.text);
        }
        self.mac_changed();
        Ok(format!("the virtual Mac '{vm}' is paused"))
    }

    /// The VM's own window, for the one-time guest setup.
    pub async fn mac_show(&self) -> Result<String> {
        let vm = self.mac_settings().vm.as_str();
        let r = self.mac_run(&["show", vm], CALL_TIMEOUT).await?;
        if !r.ok {
            bail!("terminal-mac show: {}", r.text);
        }
        Ok(format!("the window of '{vm}' is open"))
    }

    /// A person takes the VM back from whoever holds it.
    pub async fn mac_force_release(&self) -> Option<String> {
        let who = self.mac.lock().unwrap().book.force_release(now_ms());
        if let Some(w) = &who {
            tracing::info!("mac: lease taken back from '{w}'");
        }
        self.mac_settle_ended().await;
        self.mac_changed();
        who
    }

    /// Unpublish what every sandbox whose lease just ended had published for
    /// the guest. Called after anything that can end a lease: the ports a
    /// sandbox opened are its own, never the next holder's.
    async fn mac_settle_ended(&self) {
        let gone: Vec<(SandboxRef, String)> = {
            let mut rt = self.mac.lock().unwrap();
            let ended = rt.book.take_ended();
            if ended.is_empty() {
                return;
            }
            let (gone, kept) = std::mem::take(&mut rt.ports).into_iter().partition(|(sb, _)| ended.contains(&sb.to_string()));
            rt.ports = kept;
            gone
        };
        for (sb, spec) in &gone {
            match self.sbx.unpublish(&sb.name, spec).await {
                Ok(()) => tracing::info!("mac: '{sb}' no longer holds the Mac: unpublished {spec}"),
                Err(e) => tracing::warn!("mac: unpublishing {spec} for '{sb}': {e:#}"),
            }
        }
        if !gone.is_empty() {
            self.mac_changed();
        }
    }

    /// A sandbox was stopped or removed: its lease, if it holds one, ends now,
    /// and so does every port it published for the guest.
    pub async fn mac_sandbox_gone(&self, sb: &SandboxRef) {
        let me = sb.to_string();
        {
            let mut rt = self.mac.lock().unwrap();
            if !rt.book.release(&me, now_ms()) {
                // Not the holder, but ports of its own are still its own.
                rt.book.ended.push(me.clone());
            }
            rt.pending.retain(|_, p| &p.sandbox != sb);
        }
        self.mac_settle_ended().await;
        self.mac_changed();
    }

    // ── What a sandbox calls ──────────────────────────────────────────────

    fn mac_ready_to_lend(&self) -> Result<(), String> {
        if !self.mac_supported() {
            return Err("the virtual Mac needs an Apple silicon Mac host; this sandbox's host is not one. Carry on without it.".into());
        }
        if self.mac_program().is_none() {
            return Err("terminal-mac is not installed on the host. Ask the user to install it from sbxw's Mac panel (or `sbxw mac install`).".into());
        }
        Ok(())
    }

    /// Take the lease. A person approves first whatever brings the VM to a
    /// sandbox that sbxw has not already brought up itself: a boot, a
    /// resume, or lending a VM they started (see `MacBook::approval_needed`).
    pub async fn mac_start_for(self: &Arc<Self>, me: &SandboxRef, thread: Option<&ThreadName>, reason: &str, wait: Duration) -> Result<String, String> {
        self.mac_ready_to_lend()?;
        let idle = self.mac_settings().idle_release_ms();
        if let Some(l) = self.mac.lock().unwrap().book.held_by_other(&me.to_string(), now_ms(), idle) {
            let mins = mac::MacBook::free_by(l).saturating_sub(now_ms()).div_ceil(60_000);
            return Ok(format!(
                "The virtual Mac is in use by sandbox '{}'. It can be taken over in {mins} minutes at the latest. Do something else meanwhile and try later; do not loop on it.",
                l.holder
            ));
        }
        let state = self.mac_state().await;
        match state {
            VmState::Starting => return Ok("The virtual Mac is starting. Call mac_start again in a minute.".into()),
            VmState::Stopping => return Ok("The virtual Mac is shutting down. Call mac_start again in a minute or two; if it stays like this, tell the user it seems stuck.".into()),
            _ => {}
        }
        let needed = self.mac.lock().unwrap().book.approval_needed(state);
        match needed {
            None => self.mac_lease(me, false).await,
            Some(ask) => {
                let id = self.mac_ask(me, thread, ask, reason)?;
                self.mac_await(&id, me, wait).await
            }
        }
    }

    /// Poll the guest helper until it can act, or `bound` passes.
    async fn mac_wait_guest(&self, bound: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            if self.mac_guest().await.0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(BOOT_POLL.min(bound)).await;
        }
    }

    /// The lease for `me`, once the VM is up and its guest can act. `fresh`:
    /// the VM was just booted or resumed, and macOS's automatic login and the
    /// helper's launch take a minute more — wait for them, rather than call a
    /// guest that is still logging in "stuck".
    async fn mac_lease(&self, me: &SandboxRef, fresh: bool) -> Result<String, String> {
        let idle = self.mac_settings().idle_release_ms();
        let got = self.mac.lock().unwrap().book.acquire(&me.to_string(), now_ms(), idle);
        self.mac_settle_ended().await;
        self.mac_changed();
        match got {
            Acquire::Granted => {
                let held = format!(
                    "You hold the virtual Mac '{}'. It lapses after {} idle minutes, and after {} minutes in all another sandbox can take it over; call mac_release when done.",
                    self.mac_settings().vm,
                    self.mac_settings().idle_release_minutes,
                    mac::MAX_HOLD_MINUTES
                );
                // A guest with no helper cannot act: say so now, not on the
                // first screenshot.
                let ready = if fresh {
                    let wait = *self.mac_boot_wait.lock().unwrap();
                    self.mac_wait_guest(wait).await
                } else {
                    self.mac_guest().await.0
                };
                if ready {
                    Ok(format!("{held} Start with mac_screenshot."))
                } else {
                    let waited = if fresh { " after waiting for it to log in" } else { "" };
                    Ok(format!(
                        "{held} But the guest cannot act yet{waited}: its helper (TerminalMacGuest.app) does not answer, so it is probably at its login screen. Call mac_status in a minute; if it is still not ready, ask the user to look at the VM (sbxw → Setup → Mac → Open its window). Do not retry in a loop."
                    ))
                }
            }
            Acquire::Busy(l) => Ok(format!("The virtual Mac is in use by sandbox '{}'.", l.holder)),
        }
    }

    /// One card per sandbox and kind of request: asking again while one is
    /// open returns that one's id, so a sandbox cannot flood the person.
    fn mac_ask(&self, me: &SandboxRef, thread: Option<&ThreadName>, ask: Ask, reason: &str) -> Result<String, String> {
        {
            // A card that expired or was settled elsewhere is no longer open.
            // Never both locks at once.
            let ids: Vec<String> = self.mac.lock().unwrap().pending.keys().cloned().collect();
            let closed: Vec<String> = {
                let relay = self.relay.lock().unwrap();
                ids.into_iter().filter(|id| !relay.get(id).is_some_and(|r| r.state == RelayState::Pending)).collect()
            };
            let mut rt = self.mac.lock().unwrap();
            rt.pending.retain(|id, _| !closed.contains(id));
            let open = rt.pending.iter().find(|(_, p)| &p.sandbox == me && std::mem::discriminant(&p.ask) == std::mem::discriminant(&ask));
            if let Some((id, p)) = open {
                if p.ask == ask {
                    return Ok(id.clone());
                }
                let Ask::Port(other) = p.ask else { return Ok(id.clone()) };
                return Err(format!(
                    "port {other} is already waiting on the user (request {id}); one port request at a time. Pick up its outcome with check_sandbox_question, request_id \"{id}\", then ask again."
                ));
            }
        }
        let req = self.relay_open(me, thread, RelayKind::Mac, &mac::approval_text(&ask, reason))?;
        self.mac.lock().unwrap().pending.insert(req.id.clone(), Pending { sandbox: me.clone(), ask });
        Ok(req.id)
    }

    async fn mac_await(&self, id: &str, me: &SandboxRef, wait: Duration) -> Result<String, String> {
        let got = self.relay_wait(id, me, wait).await?;
        Ok(match got.state {
            RelayState::Approved => got.answer.unwrap_or_else(|| "Approved.".into()),
            RelayState::Denied => format!(
                "The user declined (request {id}){}. Nothing was done; carry on without it and do not ask again.",
                got.note.map(|n| format!(": {n}")).unwrap_or_default()
            ),
            _ => format!(
                "Waiting on the user (request {id}). Carry on with other work; pick up the outcome with check_sandbox_question, request_id \"{id}\", when you need it."
            ),
        })
    }

    /// The person said yes to request `id`: run it, then settle the request
    /// with what came of it. `false` when `id` is no Mac request in flight.
    pub fn mac_approved(self: &Arc<Self>, id: &str) -> bool {
        let Some(p) = self.mac.lock().unwrap().pending.remove(id) else { return false };
        // Off the "waiting on you" lists at once: the island, the badge.
        if let Ok(req) = self.relay.lock().unwrap().carry(id, now_ms()) {
            self.send(Event::Relay(req));
        }
        let app = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let text = match &p.ask {
                Ask::Boot | Ask::Create | Ask::Resume => match app.mac_boot().await {
                    Ok(done) => {
                        // The person said yes to this sandbox having it, even
                        // if somebody brought it up meanwhile.
                        app.mac.lock().unwrap().book.booted(now_ms());
                        match app.mac_lease(&p.sandbox, true).await {
                            Ok(lease) => format!("The user approved: {done}. {lease}"),
                            Err(e) => format!("The user approved: {done}, but {e}"),
                        }
                    }
                    Err(e) => format!("The user approved, but the boot failed: {e:#}"),
                },
                Ask::Lend => match app.mac_state().await {
                    VmState::Running => {
                        app.mac.lock().unwrap().book.booted(now_ms());
                        match app.mac_lease(&p.sandbox, false).await {
                            Ok(lease) => format!("The user approved. {lease}"),
                            Err(e) => format!("The user approved, but {e}"),
                        }
                    }
                    other => format!("The user approved, but the virtual Mac is no longer running (it is {other:?}). Call mac_start again."),
                },
                Ask::Port(port) => match app.mac_publish(&p.sandbox, *port).await {
                    Ok(url) => format!("The user approved. In the guest, open {url} (in Safari: mac_key cmd+l, mac_type the URL, mac_key return). Your app must listen on 0.0.0.0:{port} in this sandbox."),
                    Err(e) => format!("The user approved, but publishing port {port} failed: {e:#}"),
                },
            };
            app.relay_settle_mac(&id, &text);
        });
        true
    }

    pub fn mac_denied(&self, id: &str) {
        self.mac.lock().unwrap().pending.remove(id);
    }

    /// The lease, checked: a call from a sandbox that does not hold it takes
    /// it when it is free and the VM is up, and is refused otherwise.
    async fn mac_check_lease(&self, me: &SandboxRef) -> Result<(), String> {
        self.mac_ready_to_lend()?;
        // Nothing to drive, and nothing to hold, while the VM is not running:
        // say so now rather than let a screenshot time out.
        let state = self.mac_state().await;
        if state != VmState::Running {
            let word = serde_json::to_value(state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            return Err(format!("the virtual Mac is {word}, not running — call mac_start first (a person approves the boot)"));
        }
        if !self.mac.lock().unwrap().book.ours {
            return Err("sbxw did not start this virtual Mac: call mac_start first, and a person approves lending it".into());
        }
        let idle = self.mac_settings().idle_release_ms();
        let got = self.mac.lock().unwrap().book.acquire(&me.to_string(), now_ms(), idle);
        self.mac_settle_ended().await;
        match got {
            Acquire::Granted => Ok(()),
            Acquire::Busy(l) => Err(format!("the virtual Mac is in use by sandbox '{}' — call mac_start later", l.holder)),
        }
    }

    pub async fn mac_open_port_for(self: &Arc<Self>, me: &SandboxRef, thread: Option<&ThreadName>, port: u64, reason: &str, wait: Duration) -> Result<String, String> {
        let port = mac::check_guest_port(port)?;
        if !me.is_local() {
            return Err("this sandbox runs on another host: its ports cannot reach this host's virtual Mac".into());
        }
        self.mac_check_lease(me).await?;
        // Already approved (sbx still has it, even after a daemon restart): hand
        // the URL back rather than ask again, since a second publish of the same
        // spec is refused.
        if let Some(url) = self.mac_port_already(me, port).await {
            return Ok(format!(
                "Port {port} is already published for the virtual Mac. In the guest, open {url} (in Safari: mac_key cmd+l, mac_type the URL, mac_key return)."
            ));
        }
        let id = self.mac_ask(me, thread, Ask::Port(port), reason)?;
        self.mac_await(&id, me, wait).await
    }

    /// Where the guest reaches the host.
    async fn mac_guest_host_ip(&self) -> Result<String> {
        if let Some(ip) = self.mac_guest_ip_override.lock().unwrap().clone() {
            return Ok(ip);
        }
        let set = &self.mac_settings().guest_host_ip;
        if !set.is_empty() {
            return Ok(set.clone());
        }
        // Apple Virtualization's NAT puts the host on bridge100 while a VM runs.
        let out = tokio::process::Command::new(sys_bin("ifconfig")).arg("bridge100").output().await?;
        mac::bridge_ip(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| anyhow!("the VM's network bridge (bridge100) has no address — is the VM running? Set [mac] guest_host_ip in sbxw.toml to override"))
    }

    /// The guest URL of `port`, when sbx already publishes it on the VM
    /// bridge for `sb`; recorded, so it is unpublished with the VM.
    async fn mac_port_already(&self, sb: &SandboxRef, port: u16) -> Option<String> {
        let ip = self.mac_guest_host_ip().await.ok()?;
        let live = self.sbx.ports(&sb.name).await;
        live.iter().find(|m| m.host_ip == ip && m.sandbox_port == port && m.host_port == port)?;
        let spec = format!("{ip}:{port}:{port}");
        {
            let mut rt = self.mac.lock().unwrap();
            if !rt.ports.iter().any(|(s, p)| s == sb && *p == spec) {
                rt.ports.push((sb.clone(), spec));
            }
        }
        self.mac_changed();
        Some(format!("http://{ip}:{port}"))
    }

    async fn mac_publish(&self, sb: &SandboxRef, port: u16) -> Result<String> {
        // Approved while it held the Mac, but the lease may have ended since.
        if self.mac.lock().unwrap().book.holder() != Some(sb.to_string().as_str()) {
            bail!("'{sb}' no longer holds the virtual Mac: call mac_start, then ask for the port again");
        }
        if let Some(url) = self.mac_port_already(sb, port).await {
            return Ok(url);
        }
        let ip = self.mac_guest_host_ip().await?;
        let spec = format!("{ip}:{port}:{port}");
        self.sbx.publish(&sb.name, &spec).await?;
        self.mac.lock().unwrap().ports.push((sb.clone(), spec));
        self.mac_changed();
        tracing::info!("mac: published {ip}:{port} for '{sb}'");
        Ok(format!("http://{ip}:{port}"))
    }

    /// Ports opened for the guest go when the VM does.
    async fn mac_unpublish_all(&self) {
        let ports = std::mem::take(&mut self.mac.lock().unwrap().ports);
        for (sb, spec) in ports {
            if let Err(e) = self.sbx.unpublish(&sb.name, &spec).await {
                tracing::warn!("mac: unpublishing {spec} for '{sb}': {e:#}");
            }
        }
    }

    /// One guest action for the lease holder. `screenshot` after an action
    /// returns the screen as it now is.
    pub async fn mac_action_for(&self, me: &SandboxRef, action: &str, args: &Value) -> Result<Value, String> {
        self.mac_check_lease(me).await?;
        let vm = self.mac_settings().vm.as_str();
        let shot = self.mac_shot_path();
        let argv = mac::action_args(vm, action, args, &shot.to_string_lossy())?;
        let r = self.mac_run(&argv, CALL_TIMEOUT).await.map_err(|e| e.to_string())?;
        if !r.ok {
            return Err(format!("{action}: {}{}", r.text, mac_hint(&r.text)));
        }
        let wants_shot = action == "screenshot" || args.get("screenshot").and_then(Value::as_bool).unwrap_or(false);
        if !wants_shot {
            return Ok(json!({ "text": r.text }));
        }
        if action != "screenshot" {
            let argv = mac::action_args(vm, "screenshot", &Value::Null, &shot.to_string_lossy())?;
            let s = self.mac_run(&argv, CALL_TIMEOUT).await.map_err(|e| e.to_string())?;
            if !s.ok {
                return Ok(json!({ "text": format!("{}\n(The screenshot after it failed: {}{})", r.text, s.text, mac_hint(&s.text)) }));
            }
        }
        let image = read_shot(&shot).map_err(|e| format!("reading the screenshot: {e:#}"))?;
        let text = if action == "screenshot" { "The virtual Mac's screen. Coordinates are pixels of this image.".to_string() } else { r.text };
        Ok(json!({ "text": text, "image": { "mime": "image/png", "b64": image } }))
    }

    fn mac_shot_path(&self) -> PathBuf {
        let dir = self.store.paths.root.join("mac").join("shots");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("shot-{}-{}.png", std::process::id(), now_ms()))
    }

    pub async fn mac_release_for(&self, me: &SandboxRef) -> String {
        let done = self.mac.lock().unwrap().book.release(&me.to_string(), now_ms());
        self.mac_settle_ended().await;
        self.mac_changed();
        if done {
            "Released. Another sandbox can use the virtual Mac now.".into()
        } else {
            "You were not holding the virtual Mac.".into()
        }
    }

    pub async fn mac_status_text(&self, me: &SandboxRef) -> String {
        let s = self.mac_status().await;
        if !s.supported {
            return "No virtual Mac: the host is not an Apple silicon Mac.".into();
        }
        if !s.installed {
            return "terminal-mac is not installed on the host. Ask the user to install it from sbxw's Mac panel.".into();
        }
        let mine = me.to_string();
        let holder = match s.holder.as_deref() {
            Some(h) if h == mine => "you hold it".to_string(),
            Some(h) => format!("held by sandbox '{h}'"),
            None => "free".into(),
        };
        let guest = if s.state != VmState::Running {
            String::new()
        } else if s.guest_ready {
            ", the guest is ready".into()
        } else {
            ", but the guest is not ready (setup unfinished or not logged in) — ask the user".into()
        };
        format!("The virtual Mac '{}' is {}{guest}; {holder}.", s.vm, serde_json::to_value(s.state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default())
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Every half minute: lapse an idle lease, then pause or stop a VM sbxw
    /// manages that nobody holds. Only a VM whose guest is set up is paused:
    /// one still at Setup Assistant has a person in front of it.
    pub fn start_mac_lifecycle(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                app.mac_tick().await;
            }
        });
    }

    /// A holder that sbx lists as stopped, or no longer lists, lets go.
    /// An empty listing is sbx failing, not every sandbox gone.
    async fn mac_drop_absent_holder(&self) {
        let holder = self.mac.lock().unwrap().book.holder().and_then(|h| SandboxRef::parse(h).ok());
        let Some(sb) = holder.filter(SandboxRef::is_local) else { return };
        let all = self.sbx.ls().await;
        if all.is_empty() {
            return;
        }
        let gone = all.iter().find(|i| i.name == sb.name.as_str()).is_none_or(|i| i.is_stopped());
        if gone {
            tracing::info!("mac: '{sb}' is stopped or gone: its lease ends");
            self.mac_sandbox_gone(&sb).await;
        }
    }

    pub async fn mac_tick(&self) {
        if self.mac_program().is_none() {
            return;
        }
        let s = self.mac_settings();
        let now = now_ms();
        let lapsed = self.mac.lock().unwrap().book.expire(now, s.idle_release_ms());
        if let Some(who) = lapsed {
            tracing::info!("mac: '{who}' let its lease lapse");
            self.mac_changed();
        }
        self.mac_settle_ended().await;
        self.mac_drop_absent_holder().await;
        // A shutdown of ours under way: done, hung (forced), or overtaken.
        let requested = self.mac.lock().unwrap().stop_requested_ms;
        if let Some(requested) = requested {
            let state = self.mac_state().await;
            if mac::force_due(Some(requested), state, now, s) {
                match self.mac_force_off().await {
                    Ok(m) => tracing::warn!("mac: a shutdown hung in `stopping` for {} min — {m}", s.force_stop_after_minutes),
                    Err(e) => tracing::warn!("mac: forcing a hung shutdown failed: {e:#}"),
                }
            } else if state != VmState::Stopping {
                self.mac.lock().unwrap().stop_requested_ms = None;
                self.mac_changed();
            }
            return;
        }
        let (managed, busy) = {
            let rt = self.mac.lock().unwrap();
            (rt.book.managed && rt.book.lease.is_none(), rt.busy.is_some())
        };
        if !managed || busy {
            return;
        }
        let state = self.mac_state().await;
        if state == VmState::Stopped || state == VmState::Missing {
            {
                let mut rt = self.mac.lock().unwrap();
                rt.book.stopped();
                rt.book.take_ended();
            }
            self.mac_unpublish_all().await;
            self.mac_changed();
            return;
        }
        let due = self.mac.lock().unwrap().book.due(state, now, s);
        let res = match due {
            Some(Lifecycle::Pause) if self.mac_guest().await.0 => self.mac_pause().await,
            Some(Lifecycle::Stop) => self.mac_stop().await,
            _ => return,
        };
        match res {
            Ok(m) => tracing::info!("mac: idle — {m}"),
            Err(e) => tracing::warn!("mac: idle rule failed: {e:#}"),
        }
    }
}

/// What to do about a failure terminal-mac reports in words an agent might
/// not act on well: a display gone to sleep is not a broken VM, and typing
/// at it blind sends the keys to whatever wakes up — a password field.
fn mac_hint(text: &str) -> &'static str {
    let t = text.to_ascii_lowercase();
    if t.contains("no guest display") || t.contains("display is asleep") {
        " — the guest's display is probably asleep (black screen, session still open). Wake it with mac_key shift (it types nothing), then mac_screenshot; if that fails twice, ask the user to set Lock Screen → Turn display off when inactive → Never in the guest. Do not type anything until a screenshot shows where the keys will go; if it shows a lock or login screen, stop and ask the user."
    } else if t.contains("no logged-in session") || t.contains("login screen") {
        " — the guest is at its login screen. Stop and ask the user to sign in (sbxw → Setup → Mac → Open its window). Never type a password."
    } else {
        ""
    }
}

fn read_shot(p: &std::path::Path) -> Result<String> {
    let len = std::fs::metadata(p).with_context(|| format!("{} was not written", p.display()))?.len();
    if len > MAX_SHOT_BYTES {
        let _ = std::fs::remove_file(p);
        bail!("the screenshot is {} MB, too large to send", len / 1_000_000);
    }
    let bytes = std::fs::read(p)?;
    let _ = std::fs::remove_file(p);
    Ok(mac::base64(&bytes))
}

#[cfg(test)]
mod tests {
    use crate::testing::app;
    use sbxw_core::relay::RelayState;
    use sbxw_core::SandboxRef;
    use serde_json::json;
    use std::time::Duration;

    fn sb(n: &str) -> SandboxRef {
        SandboxRef::parse(n).unwrap()
    }

    /// A fake terminal-mac: a shell script answering from a state file, and
    /// logging every call. `state` is the VM's starting state.
    fn fake(dir: &std::path::Path, state: &str) -> std::path::PathBuf {
        let st = dir.join("vm-state");
        std::fs::write(&st, state).unwrap();
        let script = dir.join("terminal-mac");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
echo "$@" >> "{log}"
st="{st}"
case "$1" in
  status) if [ "$(cat "$st")" = missing ]; then echo "VM not found" >&2; exit 1; fi
          printf '{{"name":"%s","state":"%s"}}\n' "$2" "$(cat "$st")" ;;
  start|resume|up) echo running > "$st"; echo '{{"ok":true}}' ;;
  pause) echo paused > "$st"; echo '{{"ok":true}}' ;;
  stop) echo stopped > "$st"; echo '{{"ok":true}}' ;;
  action)
    case "$3" in
      status) echo '{{"screenRecording":true,"accessibility":true}}' ;;
      screenshot) printf 'PNGDATA' > "$5"; printf '{{"path":"%s"}}\n' "$5" ;;
      *) echo '{{"ok":true}}' ;;
    esac ;;
  *) echo "unknown" >&2; exit 2 ;;
esac
"#,
                log = dir.join("calls").display(),
                st = st.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    fn calls(dir: &std::path::Path) -> String {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default()
    }

    #[tokio::test]
    async fn a_running_vm_is_lent_to_one_sandbox_at_a_time() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        let got = app.mac_start_for(&sb("alpha"), None, "Safari check", Duration::from_secs(1)).await.unwrap();
        assert!(got.contains("You hold"), "{got}");
        let busy = app.mac_start_for(&sb("beta"), None, "me too", Duration::from_secs(1)).await.unwrap();
        assert!(busy.contains("in use by sandbox 'alpha'"), "{busy}");
        assert!(app.mac_action_for(&sb("beta"), "key", &json!({"keys":"return"})).await.is_err());
        assert!(app.mac_release_for(&sb("alpha")).await.starts_with("Released"));
        assert!(app.mac_action_for(&sb("beta"), "key", &json!({"keys":"return"})).await.is_ok(), "free again: taken on use");
    }

    #[tokio::test]
    async fn a_screenshot_comes_back_as_base64_and_leaves_no_file() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        let v = app.mac_action_for(&sb("alpha"), "screenshot", &json!({})).await.unwrap();
        assert_eq!(v["image"]["b64"], sbxw_core::mac::base64(b"PNGDATA"));
        let after = app.mac_action_for(&sb("alpha"), "click", &json!({"x":1,"y":2,"screenshot":true})).await.unwrap();
        assert!(after["image"]["b64"].is_string());
        let shots = std::fs::read_dir(app.store.paths.root.join("mac/shots")).unwrap().count();
        assert_eq!(shots, 0);
        assert!(calls(d.path()).contains("action default click 1 2"));
    }

    #[tokio::test]
    async fn booting_a_stopped_vm_waits_for_a_person_and_reports_what_happened() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "stopped"));
        let a = app.clone();
        let asker = tokio::spawn(async move { a.mac_start_for(&sb("alpha"), None, "Safari", Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!calls(d.path()).contains("start"), "nothing boots before the yes");
        let req = app.relay_list().into_iter().find(|r| r.kind == sbxw_core::relay::RelayKind::Mac).expect("a request");
        assert!(req.question.contains("Start the virtual Mac"));
        app.relay_approve(&req.id, None, &[]).unwrap();
        let got = asker.await.unwrap().unwrap();
        assert!(got.contains("is up") && got.contains("You hold"), "{got}");
        assert!(calls(d.path()).contains("start default"));
        assert_eq!(app.mac.lock().unwrap().book.holder(), Some("alpha"));
    }

    #[tokio::test]
    async fn a_refused_boot_starts_nothing() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "stopped"));
        let a = app.clone();
        let asker = tokio::spawn(async move { a.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let req = app.relay_list().pop().unwrap();
        app.relay_deny(&req.id, Some("not now")).unwrap();
        let got = asker.await.unwrap().unwrap();
        assert!(got.contains("declined") && got.contains("not now"), "{got}");
        assert!(!calls(d.path()).contains("start"));
        assert_eq!(app.relay_list().pop().unwrap().state, RelayState::Denied);
    }

    #[tokio::test]
    async fn a_paused_vm_resumes_only_after_a_yes() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "paused"));
        // Even one sbxw paused itself.
        app.mac.lock().unwrap().book.booted(0);
        let a = app.clone();
        let asker = tokio::spawn(async move { a.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!calls(d.path()).contains("resume"), "nothing resumes before the yes");
        let req = app.relay_list().pop().expect("a card");
        assert!(req.question.starts_with("Resume the paused virtual Mac"), "{}", req.question);
        app.relay_approve(&req.id, None, &[]).unwrap();
        let got = asker.await.unwrap().unwrap();
        assert!(got.contains("You hold"), "{got}");
        assert!(calls(d.path()).contains("resume default"));
        assert!(app.mac_action_for(&sb("alpha"), "key", &json!({"keys":"return"})).await.is_ok());
    }

    #[tokio::test]
    async fn a_running_vm_sbxw_did_not_start_is_lent_only_after_a_yes() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        // No mac_start: a click alone takes nothing.
        let e = app.mac_action_for(&sb("alpha"), "screenshot", &json!({})).await.unwrap_err();
        assert!(e.contains("mac_start") && e.contains("approves"), "{e}");
        assert!(!calls(d.path()).contains("screenshot"));
        let a = app.clone();
        let asker = tokio::spawn(async move { a.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(app.mac.lock().unwrap().book.holder(), None);
        let req = app.relay_list().pop().expect("a card");
        assert!(req.question.contains("sbxw did not start it"), "{}", req.question);
        app.relay_approve(&req.id, None, &[]).unwrap();
        let got = asker.await.unwrap().unwrap();
        assert!(got.contains("You hold"), "{got}");
        // Once lent with a yes, later leases need none, as for a VM sbxw booted.
        app.mac_release_for(&sb("alpha")).await;
        let got = app.mac_start_for(&sb("beta"), None, "x", Duration::from_secs(1)).await.unwrap();
        assert!(got.contains("You hold"), "{got}");
        assert_eq!(app.relay_list().len(), 1);
        // Seen stopped: it is no longer the one sbxw was lent.
        app.mac_release_for(&sb("beta")).await;
        std::fs::write(d.path().join("vm-state"), "stopped").unwrap();
        app.mac_status().await;
        std::fs::write(d.path().join("vm-state"), "running").unwrap();
        let pending = app.mac_start_for(&sb("beta"), None, "x", Duration::from_millis(50)).await.unwrap();
        assert!(pending.contains("Waiting on the user"), "{pending}");
    }

    #[tokio::test]
    async fn a_port_needs_the_lease_then_a_yes() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        assert!(app.mac_open_port_for(&sb("beta"), None, 3000, "x", Duration::from_secs(1)).await.is_err(), "beta holds nothing");
        assert!(app.mac_open_port_for(&sb("alpha"), None, 22, "x", Duration::from_secs(1)).await.is_err());
        let pending = app.mac_open_port_for(&sb("alpha"), None, 3000, "login page", Duration::from_millis(100)).await.unwrap();
        assert!(pending.contains("check_sandbox_question"), "{pending}");
    }

    #[tokio::test]
    async fn an_action_on_a_stopped_vm_is_refused_at_once_and_takes_no_lease() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "stopped"));
        let e = app.mac_action_for(&sb("alpha"), "screenshot", &json!({})).await.unwrap_err();
        assert!(e.contains("mac_start"), "{e}");
        assert_eq!(app.mac.lock().unwrap().book.holder(), None);
        assert!(!calls(d.path()).contains("screenshot"));
    }

    #[tokio::test]
    async fn a_boot_that_returns_without_a_running_vm_grants_no_lease() {
        let (d, app) = app();
        let script = fake(d.path(), "stopped");
        // `start` answers ok but the VM never comes up.
        let body = std::fs::read_to_string(&script).unwrap().replace("start|resume|up) echo running > \"$st\";", "start|resume|up)");
        std::fs::write(&script, body).unwrap();
        *app.mac_program_override.lock().unwrap() = Some(script);
        *app.mac_boot_wait.lock().unwrap() = Duration::from_millis(300);
        let got = app.mac_boot().await.unwrap_err().to_string();
        assert!(got.contains("returned, but the VM is stopped"), "{got}");
        assert_eq!(app.mac.lock().unwrap().book.holder(), None);
        assert!(app.mac.lock().unwrap().busy.is_none());
    }

    #[tokio::test]
    async fn a_vm_still_shutting_down_is_neither_booted_nor_offered_for_approval() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "stopping"));
        let got = app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        assert!(got.contains("shutting down"), "{got}");
        assert!(app.relay_list().is_empty(), "no card for a boot that cannot happen");
        assert!(app.mac_boot().await.is_err());
        assert!(!calls(d.path()).contains("start default"));
    }

    #[tokio::test]
    async fn a_guest_at_its_login_screen_is_lent_with_a_warning() {
        let (d, app) = app();
        let script = fake(d.path(), "running");
        // What 0.2.0-6 does with nobody logged in: the helper is not there.
        let body = std::fs::read_to_string(&script).unwrap().replace(
            r#"status) echo '{"screenRecording":true,"accessibility":true}' ;;"#,
            r#"status) echo "The virtual Mac has no logged-in session" >&2; exit 1 ;;"#,
        );
        std::fs::write(&script, body).unwrap();
        *app.mac_program_override.lock().unwrap() = Some(script);
        app.mac.lock().unwrap().book.booted(0);
        let got = app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        assert!(got.contains("login screen") && got.contains("cannot act yet"), "{got}");
    }

    #[tokio::test]
    async fn a_port_sbx_already_publishes_for_the_guest_is_reused_without_asking() {
        let (d, app, fake_sbx) = crate::testing::app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ports") => sbxw_sbx::runner::Output::ok(r#"[{"sandbox_port":5173,"host_port":5173,"host_ip":"192.168.64.1","protocol":"tcp"}]"#),
            _ => crate::testing::sbx_answer(a),
        });
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        *app.mac_guest_ip_override.lock().unwrap() = Some("192.168.64.1".into());
        app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        let got = app.mac_open_port_for(&sb("alpha"), None, 5173, "x", Duration::from_secs(1)).await.unwrap();
        assert!(got.contains("http://192.168.64.1:5173"), "{got}");
        assert!(app.relay_list().is_empty(), "no second approval");
        assert!(fake_sbx.log().iter().all(|c| !c.contains("--publish")), "{:?}", fake_sbx.log());
        assert_eq!(app.mac.lock().unwrap().ports.len(), 1, "recorded, so it goes with the VM");
    }

    /// An app whose fake sbx lists `ls` (shared, so a test can change it)
    /// and logs every call, with alpha holding a running VM and port 3000
    /// published for the guest.
    async fn alpha_with_a_port(
        ls: std::sync::Arc<std::sync::Mutex<Vec<(&'static str, &'static str)>>>,
    ) -> (tempfile::TempDir, std::sync::Arc<crate::app::App>, std::sync::Arc<sbxw_sbx::runner::Fake>) {
        let (d, app, fake_sbx) = crate::testing::app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&ls.lock().unwrap()),
            _ => crate::testing::sbx_answer(a),
        });
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        *app.mac_guest_ip_override.lock().unwrap() = Some("192.168.64.1".into());
        app.mac.lock().unwrap().book.booted(0);
        app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        app.mac_publish(&sb("alpha"), 3000).await.unwrap();
        assert!(fake_sbx.log().iter().any(|c| c.contains("--publish 192.168.64.1:3000:3000")), "{:?}", fake_sbx.log());
        (d, app, fake_sbx)
    }

    fn unpublished(fake_sbx: &sbxw_sbx::runner::Fake) -> bool {
        fake_sbx.log().iter().any(|c| c == "ports alpha --unpublish 192.168.64.1:3000:3000")
    }

    fn shared_ls(rows: &[(&'static str, &'static str)]) -> std::sync::Arc<std::sync::Mutex<Vec<(&'static str, &'static str)>>> {
        std::sync::Arc::new(std::sync::Mutex::new(rows.to_vec()))
    }

    #[tokio::test]
    async fn a_released_lease_takes_its_ports_with_it() {
        let (_d, app, fake_sbx) = alpha_with_a_port(shared_ls(&[("alpha", "running")])).await;
        app.mac_release_for(&sb("alpha")).await;
        assert!(unpublished(&fake_sbx), "{:?}", fake_sbx.log());
        assert!(app.mac.lock().unwrap().ports.is_empty());
    }

    #[tokio::test]
    async fn a_lapsed_lease_takes_its_ports_with_it() {
        let (_d, app, fake_sbx) = alpha_with_a_port(shared_ls(&[("alpha", "running")])).await;
        app.mac_tick().await;
        assert!(!unpublished(&fake_sbx), "still held");
        app.mac.lock().unwrap().book.lease.as_mut().unwrap().last_ms = crate::app::now_ms() - 11 * 60_000;
        app.mac_tick().await;
        assert!(unpublished(&fake_sbx), "{:?}", fake_sbx.log());
        assert!(app.mac.lock().unwrap().ports.is_empty());
    }

    #[tokio::test]
    async fn a_lease_taken_over_takes_its_ports_with_it() {
        let (_d, app, fake_sbx) = alpha_with_a_port(shared_ls(&[("alpha", "running"), ("beta", "running")])).await;
        // Lapsed, and beta's call gets there before the tick does.
        app.mac.lock().unwrap().book.lease.as_mut().unwrap().last_ms = crate::app::now_ms() - 11 * 60_000;
        app.mac_action_for(&sb("beta"), "key", &json!({"keys":"return"})).await.unwrap();
        assert_eq!(app.mac.lock().unwrap().book.holder(), Some("beta"));
        assert!(unpublished(&fake_sbx), "beta must not see alpha's app: {:?}", fake_sbx.log());
        assert!(app.mac.lock().unwrap().ports.is_empty());
    }

    #[tokio::test]
    async fn a_lease_taken_back_by_a_person_takes_its_ports_with_it() {
        let (_d, app, fake_sbx) = alpha_with_a_port(shared_ls(&[("alpha", "running")])).await;
        assert_eq!(app.mac_force_release().await.as_deref(), Some("alpha"));
        assert!(unpublished(&fake_sbx), "{:?}", fake_sbx.log());
    }

    #[tokio::test]
    async fn a_stopped_or_removed_holder_loses_the_lease_and_its_ports() {
        let ls = shared_ls(&[("alpha", "running")]);
        let (_d, app, fake_sbx) = alpha_with_a_port(ls.clone()).await;
        *ls.lock().unwrap() = vec![];
        app.mac_tick().await;
        assert_eq!(app.mac.lock().unwrap().book.holder(), Some("alpha"), "an empty `sbx ls` is sbx failing, not alpha gone");
        *ls.lock().unwrap() = vec![("alpha", "stopped")];
        app.mac_tick().await;
        assert_eq!(app.mac.lock().unwrap().book.holder(), None);
        assert!(unpublished(&fake_sbx), "{:?}", fake_sbx.log());

        let (_d, app, fake_sbx) = alpha_with_a_port(shared_ls(&[("alpha", "running")])).await;
        app.mac_sandbox_gone(&sb("alpha")).await;
        assert_eq!(app.mac.lock().unwrap().book.holder(), None, "removed: told at once");
        assert!(unpublished(&fake_sbx));
    }

    #[tokio::test]
    async fn a_port_approved_after_the_lease_ended_is_not_published() {
        let (_d, app, fake_sbx) = alpha_with_a_port(shared_ls(&[("alpha", "running")])).await;
        app.mac_release_for(&sb("alpha")).await;
        assert!(app.mac_publish(&sb("alpha"), 4000).await.is_err());
        assert!(!fake_sbx.log().iter().any(|c| c.contains(":4000:4000")));
    }

    #[tokio::test]
    async fn asking_again_while_a_card_is_open_opens_no_second_one() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "stopped"));
        let short = Duration::from_millis(50);
        let first = app.mac_start_for(&sb("alpha"), None, "x", short).await.unwrap();
        for _ in 0..5 {
            assert_eq!(app.mac_start_for(&sb("alpha"), None, "again", short).await.unwrap(), first);
        }
        assert_eq!(app.relay_list().len(), 1, "one boot card for alpha");
        app.mac_start_for(&sb("beta"), None, "x", short).await.unwrap();
        assert_eq!(app.relay_list().len(), 2, "beta has its own");
        // Refused: the next ask is a new card, not the settled one.
        let id = app.relay_list().into_iter().find(|r| r.from == "alpha").unwrap().id;
        app.relay_deny(&id, None).unwrap();
        app.mac_start_for(&sb("alpha"), None, "x", short).await.unwrap();
        assert_eq!(app.relay_list().len(), 3);
    }

    #[tokio::test]
    async fn one_port_card_at_a_time_per_sandbox() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        *app.mac_guest_ip_override.lock().unwrap() = Some("192.168.64.1".into());
        app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        let short = Duration::from_millis(50);
        let a = app.mac_open_port_for(&sb("alpha"), None, 3000, "x", short).await.unwrap();
        let b = app.mac_open_port_for(&sb("alpha"), None, 3000, "x", short).await.unwrap();
        assert_eq!(a, b);
        let e = app.mac_open_port_for(&sb("alpha"), None, 4000, "x", short).await.unwrap_err();
        assert!(e.contains("already waiting"), "{e}");
        assert_eq!(app.relay_list().len(), 1);
    }

    #[test]
    fn a_sleeping_display_is_explained_not_reported_raw() {
        assert!(super::mac_hint("No guest display is available").contains("Do not type"));
        assert!(super::mac_hint("The virtual Mac has no logged-in session").contains("Never type a password"));
        assert_eq!(super::mac_hint("boom"), "");
    }

    #[tokio::test]
    async fn after_a_boot_the_lease_waits_for_the_guest_to_log_in() {
        let (d, app) = app();
        let script = fake(d.path(), "paused");
        // The helper answers only from its third look: auto-login takes a while.
        let looks = d.path().join("looks");
        let ready = format!(
            "status) echo x >> \"{n}\"; if [ \"$(wc -l < \"{n}\")\" -lt 3 ]; then echo \"no logged-in session\" >&2; exit 1; fi; echo '{{\"screenRecording\":\"true\",\"accessibility\":\"true\"}}' ;;",
            n = looks.display()
        );
        let body = std::fs::read_to_string(&script)
            .unwrap()
            .replace(r#"status) echo '{"screenRecording":true,"accessibility":true}' ;;"#, &ready);
        std::fs::write(&script, body).unwrap();
        *app.mac_program_override.lock().unwrap() = Some(script);
        *app.mac_boot_wait.lock().unwrap() = Duration::from_secs(10);
        let a = app.clone();
        let asker = tokio::spawn(async move { a.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(20)).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let req = app.relay_list().pop().expect("a resume card");
        app.relay_approve(&req.id, None, &[]).unwrap();
        let got = asker.await.unwrap().unwrap();
        assert!(got.contains("Start with mac_screenshot"), "{got}");
        assert!(std::fs::read_to_string(&looks).unwrap().lines().count() >= 3);
    }

    #[tokio::test]
    async fn without_terminal_mac_the_tools_say_what_to_do() {
        let (_d, app) = app();
        let e = app.mac_action_for(&sb("alpha"), "screenshot", &json!({})).await.unwrap_err();
        assert!(e.contains("Apple silicon") || e.contains("not installed"), "{e}");
    }

    #[tokio::test]
    async fn an_idle_managed_vm_is_paused_then_stopped() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac.lock().unwrap().book.booted(0);
        app.mac_start_for(&sb("alpha"), None, "x", Duration::from_secs(1)).await.unwrap();
        app.mac_release_for(&sb("alpha")).await;
        app.mac_tick().await;
        assert!(!calls(d.path()).contains("pause"), "not idle long enough");
        let long_ago = crate::app::now_ms() - 16 * 60_000;
        app.mac.lock().unwrap().book.idle_since_ms = Some(long_ago);
        app.mac_tick().await;
        assert!(calls(d.path()).contains("pause default"));
        app.mac.lock().unwrap().book.idle_since_ms = Some(crate::app::now_ms() - 121 * 60_000);
        app.mac_tick().await;
        assert!(calls(d.path()).contains("stop default"));
        assert!(!calls(d.path()).contains("--force"), "a shutdown that worked is never forced");
        app.mac_tick().await;
        assert_eq!(app.mac.lock().unwrap().stop_requested_ms, None, "stopped: nothing left to watch");
    }

    #[tokio::test]
    async fn a_paused_vm_is_resumed_before_it_is_shut_down() {
        let (d, app) = app();
        let script = fake(d.path(), "paused");
        // As a real guest: a paused VM cannot run the shutdown it is asked for.
        let body = std::fs::read_to_string(&script)
            .unwrap()
            .replace("stop) echo stopped", r#"stop) if [ "$(cat "$st")" = paused ]; then echo "VM is paused" >&2; exit 1; fi; echo stopped"#);
        std::fs::write(&script, body).unwrap();
        *app.mac_program_override.lock().unwrap() = Some(script);
        app.mac_stop().await.unwrap();
        let log = calls(d.path());
        let (resume, stop) = (log.find("resume default").expect("resumed"), log.find("stop default").expect("stopped"));
        assert!(resume < stop, "{log}");
        assert!(!log.contains("--force"));
    }

    #[tokio::test]
    async fn a_shutdown_that_hangs_in_stopping_is_forced_after_its_grace() {
        let (d, app) = app();
        let script = fake(d.path(), "running");
        // As terminal-mac 0.2.0-6 was seen to: a normal stop never finishes.
        let body = std::fs::read_to_string(&script).unwrap().replace(
            "stop) echo stopped > \"$st\";",
            r#"stop) if [ "$3" = --force ]; then echo stopped > "$st"; else echo stopping > "$st"; fi;"#,
        );
        std::fs::write(&script, body).unwrap();
        *app.mac_program_override.lock().unwrap() = Some(script);
        app.mac_stop().await.unwrap();
        app.mac_tick().await;
        assert!(!calls(d.path()).contains("--force"), "not before its grace");
        app.mac.lock().unwrap().stop_requested_ms = Some(crate::app::now_ms() - 4 * 60_000);
        app.mac_tick().await;
        assert!(calls(d.path()).contains("stop default --force"));
        assert_eq!(app.mac.lock().unwrap().stop_requested_ms, None);
    }

    #[tokio::test]
    async fn a_stopping_vm_sbxw_did_not_stop_is_never_forced() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "stopping"));
        app.mac.lock().unwrap().book.booted(0);
        app.mac_tick().await;
        assert!(!calls(d.path()).contains("--force"));
    }

    #[tokio::test]
    async fn a_vm_the_person_started_is_never_paused() {
        let (d, app) = app();
        *app.mac_program_override.lock().unwrap() = Some(fake(d.path(), "running"));
        app.mac_tick().await;
        assert!(!calls(d.path()).contains("pause"));
    }
}
