//! `sbxw mac`: the virtual Mac from a terminal. The daemon does the work (see
//! `svc::mac`); this starts it and follows `GET /api/mac` until the job ends, so
//! the terminal shows what the web UI's Mac panel shows.

use super::{config_path, load_config};
use crate::client::Daemon;
use anyhow::{bail, Result};
use clap::Subcommand;
use sbxw_core::mac::PIN;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

#[derive(Subcommand, Clone, Copy)]
pub enum MacCmd {
    /// Download and install the pinned terminal-mac (SHA-256 checked) under ~/.sbxw/terminal-mac.
    Install,
    /// Start the VM (installing terminal-mac first if needed); creates it on first use.
    Up,
    /// Where the VM stands: installed, running, guest ready, who holds it.
    Status,
    /// Pause the VM (its RAM stays reserved).
    Pause,
    /// Ask the VM to shut down normally (forced by the daemon if it hangs).
    Stop,
    /// Power the VM off at once (`terminal-mac stop --force`); unsaved guest work is lost.
    ForceOff,
    /// Open the VM's own window (for the one-time guest setup).
    Show,
    /// Take the VM back from the sandbox holding it.
    Release,
}

/// The guest's one-time setup, which terminal-mac cannot automate.
pub const SETUP_STEPS: [&str; 6] = [
    "In the VM window, finish macOS Setup Assistant and create the guest's user account. Sign in to no personal or work account: the guest is shared by every sandbox.",
    "In the guest, open Finder → /Volumes/My Shared Files/tools → TerminalMacGuest.app.",
    "Use its two buttons to grant Screen Recording and Accessibility in the guest's System Settings (reopen it if macOS asks).",
    "Approve its login item if macOS asks, and keep the guest logged in.",
    "Turn on automatic login for that account (System Settings → Users & Groups; FileVault must be off in the guest), so a VM sbxw boots reaches the desktop and its helper without you.",
    "Stop the guest from sleeping or locking: System Settings → Lock Screen, set 'Turn display off when inactive' and 'Require password after the screen saver begins or display is turned off' to Never.",
];

pub fn run(cmd: MacCmd, config: &Path) -> Result<()> {
    let cfg = load_config(config)?;
    let daemon = Daemon::new(&cfg.web_addr);
    // The daemon may start with this file: its listeners and host command
    // need the person's yes first, as for `sbxw up`.
    let paths = sbxw_daemon::store::Paths::from_home();
    super::approve_here(&sbxw_daemon::store::Store::open(paths.clone()), &cfg, super::Scope::Daemon, false)?;
    daemon.ensure(&paths, &config_path(&cfg, config)?, false)?;
    match cmd {
        MacCmd::Status => print_status(&status(&daemon)?),
        MacCmd::Install => {
            let s = status(&daemon)?;
            if !s["supported"].as_bool().unwrap_or(false) {
                bail!("terminal-mac needs an Apple silicon Mac (macOS 14 or later)");
            }
            println!("installing terminal-mac {} (SHA-256 checked) under ~/.sbxw/terminal-mac…", PIN.version);
            daemon.post("/api/mac/install", json!({}))?;
            let s = follow(&daemon)?;
            if !s["installed"].as_bool().unwrap_or(false) {
                bail!("{}", s["error"].as_str().unwrap_or("the install did not finish"));
            }
            println!("installed. Next: `sbxw mac up`");
        }
        MacCmd::Up => {
            let mut s = status(&daemon)?;
            if !s["supported"].as_bool().unwrap_or(false) {
                bail!("terminal-mac needs an Apple silicon Mac (macOS 14 or later)");
            }
            if !s["installed"].as_bool().unwrap_or(false) {
                println!("terminal-mac is not installed: installing {} first…", PIN.version);
                daemon.post("/api/mac/install", json!({}))?;
                s = follow(&daemon)?;
                if !s["installed"].as_bool().unwrap_or(false) {
                    bail!("{}", s["error"].as_str().unwrap_or("the install did not finish"));
                }
            }
            if s["state"] == "missing" {
                println!("creating the VM '{}': macOS is downloaded from Apple (several GB) and installed. This takes a while.", s["vm"].as_str().unwrap_or("default"));
            }
            daemon.post("/api/mac/up", json!({}))?;
            let s = follow(&daemon)?;
            if let Some(e) = s["error"].as_str() {
                bail!("{e}");
            }
            print_status(&s);
        }
        MacCmd::Pause => println!("{}", daemon.post("/api/mac/pause", json!({}))?["text"].as_str().unwrap_or("")),
        MacCmd::Stop => println!("{}", daemon.post("/api/mac/stop", json!({}))?["text"].as_str().unwrap_or("")),
        MacCmd::ForceOff => println!("{}", daemon.post("/api/mac/force-off", json!({}))?["text"].as_str().unwrap_or("")),
        MacCmd::Show => println!("{}", daemon.post("/api/mac/show", json!({}))?["text"].as_str().unwrap_or("")),
        MacCmd::Release => println!("{}", daemon.post("/api/mac/take-back", json!({}))?["text"].as_str().unwrap_or("")),
    }
    Ok(())
}

fn status(daemon: &Daemon) -> Result<Value> {
    Ok(daemon.get("/api/mac")?["mac"].clone())
}

/// Poll until the daemon has nothing in progress, printing each new step.
fn follow(daemon: &Daemon) -> Result<Value> {
    let mut last = String::new();
    // Give the job a moment to mark itself busy.
    std::thread::sleep(Duration::from_millis(500));
    loop {
        let s = status(daemon)?;
        match s["busy"].as_str() {
            Some(b) => {
                if b != last {
                    println!("  … {b}");
                    last = b.to_string();
                }
                std::thread::sleep(Duration::from_secs(3));
            }
            None => return Ok(s),
        }
    }
}

fn print_status(s: &Value) {
    let word = |k: &str| s[k].as_str().unwrap_or("?").to_string();
    if !s["supported"].as_bool().unwrap_or(false) {
        println!("virtual Mac: not available (the host is not an Apple silicon Mac)");
        return;
    }
    if !s["installed"].as_bool().unwrap_or(false) {
        println!("virtual Mac: terminal-mac is not installed — `sbxw mac install` (pinned {})", PIN.version);
        return;
    }
    println!("vm        {} ({})", word("vm"), word("state"));
    println!("guest     {}", if s["guest_ready"].as_bool().unwrap_or(false) { "ready" } else { "not ready" });
    println!("held by   {}", s["holder"].as_str().unwrap_or("nobody"));
    if let Some(ports) = s["ports"].as_array().filter(|p| !p.is_empty()) {
        println!("ports     {}", ports.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
    }
    if let Some(e) = s["error"].as_str() {
        println!("error     {e}");
    }
    if s["state"] == "running" && !s["guest_ready"].as_bool().unwrap_or(false) {
        println!("\nThe guest needs its one-time setup (`sbxw mac show` opens its window):");
        for (i, step) in SETUP_STEPS.iter().enumerate() {
            println!("  {}. {step}", i + 1);
        }
    }
}
