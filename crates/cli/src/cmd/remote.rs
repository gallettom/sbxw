//! `sbxw remote`: remote access from a terminal, until the desktop app has a
//! panel for it. Everything goes through the daemon's admin routes on the UI
//! listener (`/api/remote*`, see `docs/remote-access.md`).
//!
//! `pair` prints the link only: drawing a QR code in the terminal would need
//! a new crate. The y/N prompt after the check is the confirmation on the Mac.

use super::{config_path, load_config};
use crate::client::Daemon;
use anyhow::{bail, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Subcommand, Clone, Default)]
pub enum RemoteCmd {
    /// On or off, the tailnet machine, paired devices, keeping the Mac awake.
    #[default]
    Status,
    /// Turn remote access on (asks first).
    On {
        /// Do not ask.
        #[arg(long, short)]
        yes: bool,
    },
    /// Turn remote access off: the listener and the tailnet machine stop; paired devices are kept.
    Off,
    /// Pair a phone: prints a one-time link, then asks you to confirm the phone it came from.
    Pair,
    /// The paired devices.
    Devices,
    /// Unpair a device now (its open pages lose access at once).
    Revoke { id: String },
    /// Log the tailnet machine out and delete its state (remote access must be off).
    ForgetNode,
    /// What paired devices did, and when (never what they wrote).
    Journal {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

/// What `on` asks, in one sentence.
pub const ON_QUESTION: &str = "Remote access lets a phone you pair read your running team leaders' conversations, \
type and dictate to them, answer their questions and interrupt them, over your private Tailscale network. Turn it on? [y/N] ";

pub fn run(cmd: RemoteCmd, config: &Path) -> Result<()> {
    let cfg = load_config(config)?;
    let daemon = Daemon::new(&cfg.web_addr);
    let paths = sbxw_daemon::store::Paths::from_home();
    super::approve_here(&sbxw_daemon::store::Store::open(paths.clone()), &cfg, super::Scope::Daemon, false)?;
    daemon.ensure(&paths, &config_path(&cfg, config)?, false)?;
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    match cmd {
        RemoteCmd::Status => print_status(&daemon.get("/api/remote")?),
        RemoteCmd::On { yes } => {
            if !yes && !ask(ON_QUESTION, terminal, &mut std::io::stdin().lock())? {
                bail!("not turned on: nothing was changed");
            }
            let r = daemon.post("/api/remote/enable", json!({}))?;
            check(&r)?;
            print_status(&r["status"]);
            println!("Next: `sbxw remote pair` once the tailnet machine says running.");
        }
        RemoteCmd::Off => {
            let r = daemon.post("/api/remote/disable", json!({}))?;
            check(&r)?;
            println!("remote access is off. Paired devices are kept: `sbxw remote revoke <id>` unpairs one.");
        }
        RemoteCmd::Pair => pair(&daemon, terminal)?,
        RemoteCmd::Devices => {
            let list = daemon.get("/api/remote/devices")?;
            let list = list.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("no paired device");
            }
            for d in list {
                println!("{}  {}  paired {}  last seen {}", s(&d["id"]), s(&d["name"]), ago(&d["created_ms"]), ago(&d["last_seen_ms"]));
            }
        }
        RemoteCmd::Revoke { id } => {
            check(&daemon.delete(&format!("/api/remote/devices/{}", url_segment(&id)))?)?;
            println!("device {id} is unpaired");
        }
        RemoteCmd::ForgetNode => {
            check(&daemon.post("/api/remote/tailnet/forget", json!({}))?)?;
            println!("the tailnet machine is forgotten. Remove `sbxw` from your tailnet's Machines page if it is still listed.");
        }
        RemoteCmd::Journal { limit } => {
            let list = daemon.get(&format!("/api/remote/journal?limit={limit}"))?;
            for e in list.as_array().cloned().unwrap_or_default() {
                println!("{}  {:<10}  {} {}", ago(&e["at_ms"]), s(&e["kind"]), s(&e["device_id"]), s(&e["device_name"]));
            }
        }
    }
    Ok(())
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// A device id is hex; anything else is sent percent-encoded, never as a path.
fn url_segment(id: &str) -> String {
    id.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn check(r: &Value) -> Result<()> {
    if r["ok"].as_bool() == Some(false) {
        bail!("{}", r["error"].as_str().unwrap_or("the daemon refused"));
    }
    Ok(())
}

fn ago(ms: &Value) -> String {
    let Some(ms) = ms.as_u64() else { return "-".into() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// y/N on the terminal; no terminal is a no.
pub fn ask(question: &str, terminal: bool, input: &mut impl BufRead) -> Result<bool> {
    if !terminal {
        bail!("this needs a yes in a terminal (or --yes where offered)");
    }
    eprint!("{question}");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn print_status(st: &Value) {
    let on = st["enabled"].as_bool().unwrap_or(false);
    println!("remote access: {}", if on { "on" } else { "off" });
    println!("listener:      {} ({})", if st["listening"].as_bool().unwrap_or(false) { "listening" } else { "not listening" }, s(&st["remote_addr"]));
    match st["tailnet"].as_object() {
        Some(t) => {
            println!("tailnet:       {} {}", t.get("state").and_then(Value::as_str).unwrap_or("?"), t.get("name").and_then(Value::as_str).unwrap_or(""));
            if let Some(u) = t.get("login_url").and_then(Value::as_str) {
                println!("               log in: {u}");
            }
            if let Some(d) = t.get("detail").and_then(Value::as_str) {
                println!("               {d}");
            }
        }
        None => println!("tailnet:       not running"),
    }
    println!("devices:       {} paired, {} waiting", st["devices"].as_u64().unwrap_or(0), st["pending_requests"].as_u64().unwrap_or(0));
    let awake = match st["keep_awake"].as_str().unwrap_or("") {
        "holding" => "kept awake (on mains power)",
        "on_battery" => "may sleep (on battery)",
        "not_macos" => "not handled on this system",
        _ => "not held",
    };
    println!("keep awake:    {awake}");
}

/// Show the link, wait for the phone, show its name and check, ask.
fn pair(daemon: &Daemon, terminal: bool) -> Result<()> {
    if !terminal {
        bail!("pairing needs your yes in a terminal");
    }
    let r = daemon.post("/api/remote/pairing", json!({}))?;
    check(&r)?;
    println!("Open this link on the phone (valid 5 minutes, once):\n\n  {}\n", s(&r["url"]));
    println!("(No QR code: drawing one in the terminal would need a new library.)");
    println!("Waiting for the phone…");
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    let req = loop {
        if Instant::now() > deadline {
            bail!("no phone used the link in 5 minutes: run `sbxw remote pair` again");
        }
        let list = daemon.get("/api/remote/pairing/requests")?;
        if let Some(first) = list.as_array().and_then(|l| l.first()) {
            break first.clone();
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    println!("\nA phone asks to pair: \"{}\"", s(&req["device_name"]).chars().filter(|c| !c.is_control()).collect::<String>());
    println!("Check that the phone shows the same number: {}\n", s(&req["check"]));
    let id = s(&req["id"]);
    if ask("Pair this phone? [y/N] ", true, &mut std::io::stdin().lock())? {
        check(&daemon.post(&format!("/api/remote/pairing/requests/{}/approve", url_segment(&id)), json!({}))?)?;
        println!("paired. `sbxw remote devices` lists it; `sbxw remote revoke <id>` unpairs it.");
    } else {
        check(&daemon.post(&format!("/api/remote/pairing/requests/{}/refuse", url_segment(&id)), json!({}))?)?;
        println!("refused: the phone is not paired.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_asks_and_only_yes_is_yes() {
        assert!(ask("? ", true, &mut "y\n".as_bytes()).unwrap());
        assert!(ask("? ", true, &mut "YES\n".as_bytes()).unwrap());
        assert!(!ask("? ", true, &mut "\n".as_bytes()).unwrap());
        assert!(!ask("? ", true, &mut "n\n".as_bytes()).unwrap());
        assert!(ask("? ", false, &mut "y\n".as_bytes()).is_err(), "no terminal, no yes");
        assert!(ON_QUESTION.contains("phone") && ON_QUESTION.ends_with("[y/N] "));
    }

    #[test]
    fn ids_never_become_paths() {
        assert_eq!(url_segment("ab12"), "ab12");
        assert_eq!(url_segment("../x"), "%2E%2E%2Fx");
    }
}
