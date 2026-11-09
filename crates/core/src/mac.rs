//! A virtual Mac for sandboxes: terminal-mac, driven by the daemon on the host.
//!
//! A sandbox is a Linux container and cannot run macOS. terminal-mac
//! (github.com/govijr/terminal-mac) runs a macOS guest under Apple
//! Virtualization on an Apple silicon host, with a helper *inside the guest*
//! that captures its screen and posts its input. The daemon runs it, and the
//! `sbxw:terminal-mac` kit gives a sandbox's agent tools that reach it through
//! the agent listener. This module is the rules only: no process, no clock.
//!
//! * **One VM, one holder at a time.** A guest takes gigabytes of RAM and a
//!   disk that grows to 64 GiB, so every sandbox shares one, and a sandbox
//!   holds a *lease* while it drives it. The lease lapses after
//!   `idle_release_minutes` without a call.
//! * **A person approves what costs or exposes something**: booting the VM,
//!   and publishing a port for the guest's browser. Clicks and keys need no
//!   approval: the guest is disposable, with no personal or organisation
//!   account in it.
//! * **Shut down normally first, force as a last resort.** terminal-mac
//!   0.2.0-6's normal stop was seen to leave the guest `stopping` for good
//!   (at the login screen, and with a session open), keeping its RAM. So a
//!   stop sbxw asked for that is still `stopping` after
//!   `force_stop_after_minutes` is forced; only unsaved guest work is lost.
//! * **The release is pinned.** sbxw installs one terminal-mac archive whose
//!   SHA-256 is compiled in here, never `latest`. Moving the pin is an sbxw
//!   release.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The terminal-mac release sbxw installs.
pub struct Release {
    pub version: &'static str,
    /// The archive's base name, also the directory it unpacks to.
    pub archive: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// terminal-mac 0.2.0 (build 6), the archive its own `get.sh` installs.
/// The hash was checked against a download of that archive.
pub const PIN: Release = Release {
    version: "0.2.0-6",
    archive: "terminal-mac-0.2.0-6-macos-arm64",
    url: "https://github.com/govijr/terminal-mac/releases/download/v0.2.0-6/terminal-mac-0.2.0-6-macos-arm64.zip",
    sha256: "f110d8eff04f7996fca1bf47dfc9b1a161e4b76f401481f32cbe62ac12338742",
};

/// terminal-mac runs on Apple silicon only (`std::env::consts` values).
pub fn host_supported(os: &str, arch: &str) -> bool {
    os == "macos" && arch == "aarch64"
}

/// `[mac]` in sbxw.toml. Read from the daemon's own config: there is one VM
/// per host, not one per project.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MacSettings {
    /// The terminal-mac VM sbxw drives.
    pub vm: String,
    /// A lease nobody used for this long is released.
    pub idle_release_minutes: u32,
    /// A VM sbxw booted, held by nobody for this long, is paused (its RAM
    /// stays reserved; resuming is quick).
    pub pause_after_minutes: u32,
    /// …and for this long, shut down: normally first (see `force_due`).
    pub stop_after_minutes: u32,
    /// A shutdown sbxw asked for that is still `stopping` after this long is
    /// forced (`terminal-mac stop --force`).
    pub force_stop_after_minutes: u32,
    /// The host address the guest reaches published ports on. Empty: the
    /// address of the VM's network bridge, looked up when a port is opened.
    pub guest_host_ip: String,
}

impl Default for MacSettings {
    fn default() -> Self {
        Self {
            vm: "default".into(),
            idle_release_minutes: 10,
            pause_after_minutes: 15,
            stop_after_minutes: 120,
            force_stop_after_minutes: 3,
            guest_host_ip: String::new(),
        }
    }
}

impl MacSettings {
    pub fn validate(&self) -> Result<(), String> {
        check_vm_name(&self.vm)?;
        if self.idle_release_minutes == 0 || self.pause_after_minutes == 0 || self.stop_after_minutes == 0 || self.force_stop_after_minutes == 0 {
            return Err("[mac]: the minutes settings must be at least 1".into());
        }
        if self.stop_after_minutes < self.pause_after_minutes {
            return Err("[mac]: stop_after_minutes cannot be shorter than pause_after_minutes".into());
        }
        if !self.guest_host_ip.is_empty() && self.guest_host_ip.parse::<std::net::Ipv4Addr>().is_err() {
            return Err(format!("[mac]: guest_host_ip '{}' is not an IPv4 address", self.guest_host_ip));
        }
        Ok(())
    }

    pub fn idle_release_ms(&self) -> u64 {
        u64::from(self.idle_release_minutes) * 60_000
    }
    pub fn force_stop_after_ms(&self) -> u64 {
        u64::from(self.force_stop_after_minutes) * 60_000
    }
    pub fn pause_after_ms(&self) -> u64 {
        u64::from(self.pause_after_minutes) * 60_000
    }
    pub fn stop_after_ms(&self) -> u64 {
        u64::from(self.stop_after_minutes) * 60_000
    }
}

/// A VM name goes to terminal-mac as an argument: keep it one that cannot be
/// read as a flag.
pub fn check_vm_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(format!("[mac]: '{name}' is not a VM name (letters, digits, '-', '_', '.')"))
    }
}

// ── The VM as terminal-mac reports it ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    /// No such VM yet: `up` creates it.
    Missing,
    Stopped,
    /// A shutdown is under way. terminal-mac's `start` answers "VM started"
    /// while one is, and starts nothing: wait for it to finish.
    Stopping,
    Starting,
    Running,
    Paused,
    /// terminal-mac answered something this build does not recognise.
    Unknown,
}

impl VmState {
    pub fn from_word(w: &str) -> Option<Self> {
        Some(match w.to_ascii_lowercase().as_str() {
            "running" => VmState::Running,
            "paused" | "pausing" => VmState::Paused,
            "stopping" | "shutting_down" | "shuttingdown" => VmState::Stopping,
            "stopped" | "off" | "shutdown" | "error" => VmState::Stopped,
            "starting" | "resuming" | "booting" | "installing" => VmState::Starting,
            "missing" | "absent" | "notfound" | "not_found" | "none" => VmState::Missing,
            _ => return None,
        })
    }
}

/// The state in `terminal-mac status NAME`'s JSON. Its exact shape is not
/// documented, so this looks for the first string under a key that names a
/// state (`state`, `status`, `vmState`…), at the top or one level down.
pub fn parse_vm_state(v: &Value) -> VmState {
    fn scan(v: &Value, depth: u8) -> Option<VmState> {
        let obj = v.as_object()?;
        for (k, val) in obj {
            let k = k.to_ascii_lowercase();
            if k == "state" || k == "status" || k == "vmstate" || k == "machinestate" {
                if let Some(s) = val.as_str().and_then(VmState::from_word) {
                    return Some(s);
                }
            }
        }
        if depth > 0 {
            for val in obj.values() {
                if let Some(s) = scan(val, depth - 1) {
                    return Some(s);
                }
            }
        }
        None
    }
    scan(v, 1).unwrap_or(VmState::Unknown)
}

/// Whether `terminal-mac action NAME status` says the guest helper can act:
/// it answered, and nothing in it about a permission or a session is `false`.
/// A guest still at Setup Assistant, or without Screen Recording or
/// Accessibility granted, is not ready.
pub fn guest_ready(v: &Value) -> bool {
    fn any_false(v: &Value) -> bool {
        match v {
            Value::Object(o) => o.iter().any(|(k, val)| {
                let k = k.to_ascii_lowercase();
                let relevant = ["screen", "accessib", "permission", "session", "logged", "ready", "helper"]
                    .iter()
                    .any(|w| k.contains(w));
                // terminal-mac 0.2.0 sends its booleans as strings
                // (`"accessibility" : "true"`), so "false" counts as false.
                let no = val == &Value::Bool(false) || val.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("false"));
                (relevant && no) || any_false(val)
            }),
            Value::Array(a) => a.iter().any(any_false),
            _ => false,
        }
    }
    v.is_object() && !any_false(v)
}

// ── The lease ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lease {
    pub holder: String,
    pub since_ms: u64,
    pub last_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acquire {
    /// The caller holds it now (or already did).
    Granted,
    /// Another sandbox holds it.
    Busy(Lease),
}

/// Who holds the VM, and since when nobody has.
#[derive(Debug, Default, Clone)]
pub struct MacBook {
    pub lease: Option<Lease>,
    /// When the last lease ended, or sbxw booted the VM: the clock the
    /// pause and stop rules run on. `None` while a lease is held, and for a
    /// VM sbxw did not start, which it never pauses or stops on its own.
    pub idle_since_ms: Option<u64>,
    /// sbxw booted this VM or handed it out in this daemon's life, so it
    /// may manage its lifecycle. A VM the person started themselves is
    /// theirs until a sandbox has used it.
    pub managed: bool,
}

impl MacBook {
    /// Take the lease, or refresh it for its holder. A lease past its idle
    /// bound is free for anyone.
    pub fn acquire(&mut self, who: &str, now: u64, idle_ms: u64) -> Acquire {
        if let Some(l) = self.held_by_other(who, now, idle_ms) {
            return Acquire::Busy(l.clone());
        }
        if let Some(l) = self.lease.as_mut().filter(|l| l.holder == who) {
            l.last_ms = now;
            return Acquire::Granted;
        }
        self.lease = Some(Lease { holder: who.to_string(), since_ms: now, last_ms: now });
        self.idle_since_ms = None;
        self.managed = true;
        Acquire::Granted
    }

    /// The live lease that would refuse `who`, without taking anything.
    pub fn held_by_other(&self, who: &str, now: u64, idle_ms: u64) -> Option<&Lease> {
        self.lease.as_ref().filter(|l| l.holder != who && now.saturating_sub(l.last_ms) < idle_ms)
    }

    pub fn holder(&self) -> Option<&str> {
        self.lease.as_ref().map(|l| l.holder.as_str())
    }

    /// The holder lets go. Anyone else's release changes nothing.
    pub fn release(&mut self, who: &str, now: u64) -> bool {
        if self.holder() != Some(who) {
            return false;
        }
        self.lease = None;
        self.idle_since_ms = Some(now);
        true
    }

    /// A person takes it back from whoever holds it.
    pub fn force_release(&mut self, now: u64) -> Option<String> {
        let l = self.lease.take()?;
        self.idle_since_ms = Some(now);
        Some(l.holder)
    }

    /// Drop a lease nobody used for `idle_ms`. Returns its holder.
    pub fn expire(&mut self, now: u64, idle_ms: u64) -> Option<String> {
        let stale = self.lease.as_ref().is_some_and(|l| now.saturating_sub(l.last_ms) >= idle_ms);
        if !stale {
            return None;
        }
        let l = self.lease.take()?;
        // Idle since the last call, not since the expiry noticed it.
        self.idle_since_ms = Some(l.last_ms);
        Some(l.holder)
    }

    /// sbxw just booted the VM: it is managed, idle from now.
    pub fn booted(&mut self, now: u64) {
        self.managed = true;
        if self.lease.is_none() {
            self.idle_since_ms = Some(now);
        }
    }

    /// The VM went away (stopped, by sbxw or anyone): nobody holds it.
    pub fn stopped(&mut self) {
        self.lease = None;
        self.idle_since_ms = None;
        self.managed = false;
    }

    /// What the lifecycle rules want done to a VM in `state`, now.
    pub fn due(&self, state: VmState, now: u64, s: &MacSettings) -> Option<Lifecycle> {
        if !self.managed || self.lease.is_some() {
            return None;
        }
        let idle = now.saturating_sub(self.idle_since_ms?);
        match state {
            VmState::Running | VmState::Paused if idle >= s.stop_after_ms() => Some(Lifecycle::Stop),
            VmState::Running if idle >= s.pause_after_ms() => Some(Lifecycle::Pause),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Pause,
    /// A normal shutdown request; forced later only by `force_due`.
    Stop,
}

/// Whether a shutdown sbxw asked for at `requested_ms` has hung long enough
/// to be forced. Only a VM still `stopping` is: one that stopped needs
/// nothing, and one that came back up was started by somebody since.
pub fn force_due(requested_ms: Option<u64>, state: VmState, now: u64, s: &MacSettings) -> bool {
    match requested_ms {
        Some(t) => state == VmState::Stopping && now.saturating_sub(t) >= s.force_stop_after_ms(),
        None => false,
    }
}

// ── What an agent may ask the guest to do ─────────────────────────────────

/// The guest actions a sandbox may call, as the tools name them.
pub const ACTIONS: [&str; 8] = ["status", "screenshot", "click", "move", "drag", "scroll", "type", "key"];

/// Text typed in one call: enough for a URL or a form field.
pub const MAX_TYPE: usize = 2000;

fn coord(args: &Value, key: &str) -> Result<String, String> {
    let v = args.get(key).and_then(Value::as_f64).ok_or_else(|| format!("'{key}' is required, a number"))?;
    if !(0.0..=20_000.0).contains(&v) {
        return Err(format!("'{key}' is off the screen"));
    }
    Ok(format!("{}", v.round() as i64))
}

fn signed(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_f64().ok_or_else(|| format!("'{key}' must be a number"))?;
            if n.abs() > 100_000.0 {
                return Err(format!("'{key}' is too large"));
            }
            Ok(Some(format!("{}", n.round() as i64)))
        }
    }
}

/// A key chord: `return`, `cmd+space`, `cmd+shift+4`. Letters, digits and
/// `+` only, so it can never be read as a flag.
fn chord(raw: &str) -> Result<String, String> {
    let k = raw.trim().to_ascii_lowercase();
    let ok = !k.is_empty()
        && k.len() <= 40
        && !k.starts_with('+')
        && !k.ends_with('+')
        && k.split('+').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()));
    if ok {
        Ok(k)
    } else {
        Err(format!("'{raw}' is not a key chord (for example return, tab, cmd+space, cmd+shift+4)"))
    }
}

/// The argv after `terminal-mac` for one guest action. `screenshot_to` is
/// where a screenshot is written on the host; the daemon picks it.
pub fn action_args(vm: &str, action: &str, args: &Value, screenshot_to: &str) -> Result<Vec<String>, String> {
    check_vm_name(vm)?;
    let mut a: Vec<String> = vec!["action".into(), vm.into(), action.into()];
    match action {
        "status" => {}
        "screenshot" => {
            a.push("--output".into());
            a.push(screenshot_to.into());
        }
        "click" => {
            a.push(coord(args, "x")?);
            a.push(coord(args, "y")?);
            match args.get("clicks").and_then(Value::as_u64).unwrap_or(1) {
                1 => {}
                2 => a.push("--double".into()),
                n => return Err(format!("{n} clicks: terminal-mac's CLI clicks once or twice")),
            }
            match args.get("button").and_then(Value::as_str).unwrap_or("left") {
                "left" => {}
                "right" => {
                    a.push("--button".into());
                    a.push("right".into());
                }
                b => return Err(format!("'{b}': the button is left or right")),
            }
        }
        "move" => {
            a.push(coord(args, "x")?);
            a.push(coord(args, "y")?);
        }
        "drag" => {
            for k in ["x", "y", "x2", "y2"] {
                a.push(coord(args, k)?);
            }
        }
        "scroll" => {
            let dy = signed(args, "dy")?.ok_or("'dy' is required (negative scrolls down)")?;
            a.extend(["--dy".into(), dy]);
            if args.get("x").is_some() || args.get("y").is_some() {
                a.extend(["--x".into(), coord(args, "x")?, "--y".into(), coord(args, "y")?]);
            }
        }
        "type" => {
            let text = args.get("text").and_then(Value::as_str).ok_or("'text' is required")?;
            if text.is_empty() {
                return Err("'text' is empty".into());
            }
            if text.chars().count() > MAX_TYPE {
                return Err(format!("at most {MAX_TYPE} characters per call"));
            }
            if text.contains('\0') {
                return Err("'text' cannot contain a NUL".into());
            }
            // `--` ends option parsing: text that starts with '-' is text.
            a.push("--".into());
            a.push(text.into());
        }
        "key" => {
            let k = args.get("keys").or_else(|| args.get("key")).and_then(Value::as_str).ok_or("'keys' is required")?;
            a.push(chord(k)?);
        }
        other => return Err(format!("'{other}' is not a guest action ({})", ACTIONS.join(", "))),
    }
    Ok(a)
}

/// A port a sandbox asks to show the guest: an app port, never a
/// privileged one.
pub fn check_guest_port(port: u64) -> Result<u16, String> {
    match u16::try_from(port) {
        Ok(p) if p >= 1024 => Ok(p),
        _ => Err(format!("{port}: pick a port from 1024 to 65535")),
    }
}

/// The text a person reads on the approval card. The agent's reason is
/// untrusted and goes last, quoted.
pub fn approval_text(what: &Ask, reason: &str) -> String {
    let head = match what {
        Ask::Boot => "Start the virtual Mac (terminal-mac). It reserves several GB of RAM until it is paused or stopped.".to_string(),
        Ask::Port(p) => format!("Publish this sandbox's port {p} on the host's VM bridge, so the virtual Mac's browser can open it."),
    };
    let reason = reason.trim();
    if reason.is_empty() {
        head
    } else {
        format!("{head}\n\nThe agent says: \"{reason}\"")
    }
}

/// What a sandbox asks a person to approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Boot,
    Port(u16),
}

/// Standard base64 with padding: a screenshot travels to the sandbox as the
/// MCP image block's `data`.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The IPv4 address of `ifconfig <bridge>`'s output: where the guest, on
/// Apple Virtualization's NAT, reaches the host.
pub fn bridge_ip(ifconfig: &str) -> Option<String> {
    ifconfig.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        (w.next()? == "inet").then(|| w.next()).flatten().filter(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok()).map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const S: MacSettings = MacSettings {
        vm: String::new(),
        idle_release_minutes: 10,
        pause_after_minutes: 15,
        stop_after_minutes: 120,
        force_stop_after_minutes: 3,
        guest_host_ip: String::new(),
    };
    const MIN: u64 = 60_000;

    #[test]
    fn the_pin_is_a_full_sha256_of_a_versioned_archive() {
        assert_eq!(PIN.sha256.len(), 64);
        assert!(PIN.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(PIN.url.ends_with(&format!("{}.zip", PIN.archive)));
        assert!(!PIN.url.contains("latest"));
    }

    #[test]
    fn only_apple_silicon_runs_it() {
        assert!(host_supported("macos", "aarch64"));
        assert!(!host_supported("macos", "x86_64"));
        assert!(!host_supported("linux", "aarch64"));
    }

    #[test]
    fn settings_are_checked() {
        assert!(MacSettings::default().validate().is_ok());
        assert!(MacSettings { vm: "--force".into(), ..Default::default() }.validate().is_err());
        assert!(MacSettings { stop_after_minutes: 5, ..Default::default() }.validate().is_err());
        assert!(MacSettings { idle_release_minutes: 0, ..Default::default() }.validate().is_err());
        assert!(MacSettings { guest_host_ip: "bridge".into(), ..Default::default() }.validate().is_err());
        assert!(MacSettings { guest_host_ip: "192.168.64.1".into(), ..Default::default() }.validate().is_ok());
    }

    #[test]
    fn the_mac_table_is_read_from_sbxw_toml_and_checked() {
        let cfg = crate::config::Config::parse("[mac]\npause_after_minutes = 5\n").unwrap();
        assert_eq!(cfg.mac.pause_after_minutes, 5);
        assert_eq!(cfg.mac.vm, "default", "the rest keep their defaults");
        assert!(crate::config::Config::parse("[mac]\nvm = \"--force\"\n").is_err());
        assert_eq!(crate::config::Config::default().mac, MacSettings::default());
    }

    #[test]
    fn one_holder_at_a_time_until_it_goes_idle() {
        let mut b = MacBook::default();
        assert_eq!(b.acquire("a", 0, 10 * MIN), Acquire::Granted);
        assert!(matches!(b.acquire("b", MIN, 10 * MIN), Acquire::Busy(l) if l.holder == "a"));
        assert_eq!(b.acquire("a", 5 * MIN, 10 * MIN), Acquire::Granted, "refreshed");
        assert!(matches!(b.acquire("b", 14 * MIN, 10 * MIN), Acquire::Busy(_)));
        assert_eq!(b.acquire("b", 15 * MIN, 10 * MIN), Acquire::Granted, "a's lapsed");
        assert_eq!(b.holder(), Some("b"));
    }

    #[test]
    fn only_the_holder_releases_but_a_person_can_take_it_back() {
        let mut b = MacBook::default();
        b.acquire("a", 0, MIN);
        assert!(!b.release("b", 1));
        assert!(b.release("a", 2));
        assert_eq!(b.idle_since_ms, Some(2));
        b.acquire("a", 3, MIN);
        assert_eq!(b.force_release(4).as_deref(), Some("a"));
        assert_eq!(b.holder(), None);
    }

    #[test]
    fn an_expired_lease_is_idle_since_its_last_call() {
        let mut b = MacBook::default();
        b.acquire("a", 0, 10 * MIN);
        assert_eq!(b.expire(9 * MIN, 10 * MIN), None);
        assert_eq!(b.expire(12 * MIN, 10 * MIN).as_deref(), Some("a"));
        assert_eq!(b.idle_since_ms, Some(0));
    }

    #[test]
    fn a_managed_idle_vm_is_paused_then_stopped_never_while_held() {
        let mut b = MacBook::default();
        b.booted(0);
        assert_eq!(b.due(VmState::Running, 14 * MIN, &S), None);
        assert_eq!(b.due(VmState::Running, 15 * MIN, &S), Some(Lifecycle::Pause));
        assert_eq!(b.due(VmState::Paused, 60 * MIN, &S), None);
        assert_eq!(b.due(VmState::Paused, 120 * MIN, &S), Some(Lifecycle::Stop));
        assert_eq!(b.due(VmState::Stopped, 500 * MIN, &S), None);
        b.acquire("a", 200 * MIN, 10 * MIN);
        assert_eq!(b.due(VmState::Running, 900 * MIN, &S), None, "held");
    }

    #[test]
    fn only_a_stop_sbxw_asked_for_that_hangs_is_forced() {
        assert!(!force_due(None, VmState::Stopping, 100 * MIN, &S), "not ours");
        assert!(!force_due(Some(0), VmState::Stopping, 2 * MIN, &S), "not long enough");
        assert!(force_due(Some(0), VmState::Stopping, 3 * MIN, &S));
        assert!(!force_due(Some(0), VmState::Stopped, 9 * MIN, &S), "it stopped");
        assert!(!force_due(Some(0), VmState::Running, 9 * MIN, &S), "someone started it again");
        assert!(MacSettings { force_stop_after_minutes: 0, ..Default::default() }.validate().is_err());
    }

    #[test]
    fn a_vm_the_person_started_is_left_alone() {
        let b = MacBook::default();
        assert_eq!(b.due(VmState::Running, 1_000 * MIN, &S), None);
    }

    #[test]
    fn states_are_read_from_whatever_key_names_one() {
        assert_eq!(parse_vm_state(&json!({"name":"default","state":"running"})), VmState::Running);
        assert_eq!(parse_vm_state(&json!({"vm":{"status":"Paused"}})), VmState::Paused);
        assert_eq!(parse_vm_state(&json!({"state":"teleporting"})), VmState::Unknown);
        // terminal-mac 0.2.0-6, mid-shutdown, as printed on a real host.
        assert_eq!(parse_vm_state(&json!({"name":"default","pid":"54964","state":"stopping"})), VmState::Stopping);
        assert_eq!(parse_vm_state(&json!("running")), VmState::Unknown);
    }

    #[test]
    fn a_guest_without_its_permissions_is_not_ready() {
        assert!(guest_ready(&json!({"screenRecording":true,"accessibility":true})));
        assert!(!guest_ready(&json!({"screenRecording":true,"accessibility":false})));
        assert!(!guest_ready(&json!({"permissions":{"screenRecording":false}})));
        assert!(!guest_ready(&json!({"loggedIn":false})));
        assert!(guest_ready(&json!({"display":{"hidpi":false}})), "an unrelated false is not a permission");
        assert!(!guest_ready(&json!(null)));
    }

    #[test]
    fn terminal_macs_string_booleans_are_read_as_booleans() {
        // `terminal-mac action default status`, 0.2.0-6, as printed on a real guest.
        let real = json!({"accessibility":"true","height":"800","screenRecording":"true","version":"0.2.0","width":"1280"});
        assert!(guest_ready(&real));
        assert!(!guest_ready(&json!({"accessibility":"false","screenRecording":"true"})));
        assert!(!guest_ready(&json!({"accessibility":"true","screenRecording":"FALSE"})));
        assert_eq!(parse_vm_state(&json!({"name":"default","pid":"64155","state":"running"})), VmState::Running);
    }

    #[test]
    fn actions_become_argv_that_cannot_carry_a_flag() {
        let a = |act: &str, v: Value| action_args("default", act, &v, "/tmp/s.png");
        assert_eq!(a("screenshot", json!({})).unwrap(), ["action", "default", "screenshot", "--output", "/tmp/s.png"]);
        assert_eq!(a("click", json!({"x": 640.4, "y": 400, "clicks": 2})).unwrap(), ["action", "default", "click", "640", "400", "--double"]);
        assert_eq!(a("click", json!({"x": 1, "y": 2, "button": "right"})).unwrap()[5..], ["--button", "right"]);
        assert!(a("click", json!({"x": 1, "y": 2, "clicks": 3})).is_err());
        assert!(a("click", json!({"x": -1, "y": 2})).is_err());
        assert!(a("click", json!({"x": "1", "y": 2})).is_err());
        assert_eq!(a("type", json!({"text": "--help"})).unwrap()[3..], ["--", "--help"]);
        assert!(a("type", json!({"text": "x".repeat(MAX_TYPE + 1)})).is_err());
        assert_eq!(a("key", json!({"keys": "CMD+Space"})).unwrap()[3], "cmd+space");
        assert!(a("key", json!({"keys": "--force"})).is_err());
        assert!(a("key", json!({"keys": "cmd+"})).is_err());
        assert_eq!(a("scroll", json!({"dy": -300, "x": 10, "y": 20})).unwrap()[3..], ["--dy", "-300", "--x", "10", "--y", "20"]);
        assert!(a("scroll", json!({})).is_err());
        assert_eq!(a("drag", json!({"x":1,"y":2,"x2":3,"y2":4})).unwrap().len(), 7);
        assert!(a("shell", json!({})).is_err());
        assert!(action_args("-x", "status", &json!({}), "").is_err());
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe]), "//4=");
    }

    #[test]
    fn the_bridge_address_is_its_inet_line() {
        let out = "bridge100: flags=8a63<UP>\n\tether 36:1a\n\tinet 192.168.64.1 netmask 0xffffff00 broadcast 192.168.64.255\n\tinet6 fe80::1%bridge100";
        assert_eq!(bridge_ip(out).as_deref(), Some("192.168.64.1"));
        assert_eq!(bridge_ip("ifconfig: interface bridge100 does not exist"), None);
    }

    #[test]
    fn ports_are_app_ports() {
        assert_eq!(check_guest_port(3000), Ok(3000));
        assert!(check_guest_port(80).is_err());
        assert!(check_guest_port(70_000).is_err());
    }

    #[test]
    fn the_card_says_what_happens_before_what_the_agent_says() {
        let t = approval_text(&Ask::Port(5173), "  check the login page in Safari ");
        assert!(t.starts_with("Publish this sandbox's port 5173"));
        assert!(t.ends_with("\"check the login page in Safari\""));
        assert!(!approval_text(&Ask::Boot, "").contains("agent says"));
    }
}
