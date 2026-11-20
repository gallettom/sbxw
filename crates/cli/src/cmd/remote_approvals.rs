//! `sbxw remote approvals`: approving from the phone with Face ID (level 2,
//! `docs/security.md#approving-from-the-phone`). Everything goes through the
//! daemon's admin routes on the UI listener (`/api/remote/approvals*`).
//!
//! `on` asks a yes at a terminal and has no `--yes`: an agent never runs it.
//! `allow` makes the person TYPE the six digits the phone shows, as pairing
//! does: the Mac never prints them, so the grant goes to the phone in hand.

use super::remote::{ago, ask, check, read_digits, s, url_segment};
use crate::client::Daemon;
use anyhow::{bail, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Subcommand, Clone)]
pub enum ApprovalsCmd {
    /// On or off, and each paired device: Face ID set up or not, a request waiting.
    Status,
    /// Turn approving from the phone on (asks first; needs remote access on).
    On,
    /// Turn it off. Passkeys are kept; nothing can be approved from a phone.
    Off,
    /// Let a paired device register Face ID once: type the six digits its phone shows.
    Allow { device: String },
    /// Drop a device's Face ID passkey: it can no longer approve.
    Revoke { device: String },
}

/// What `on` asks, in one sentence.
pub const ON_QUESTION: &str = "Approving from the phone lets a paired phone with Face ID set up approve or refuse a plan, \
run a merge a lead proposed, dismiss a network request and remove a stopped sandbox, with Face ID at each action. Turn it on? [y/N] ";

pub fn run(daemon: &Daemon, cmd: ApprovalsCmd, terminal: bool) -> Result<()> {
    match cmd {
        ApprovalsCmd::Status => {
            for l in render_status(&daemon.get("/api/remote/approvals")?) {
                println!("{l}");
            }
        }
        ApprovalsCmd::On => {
            if !ask(ON_QUESTION, terminal, &mut std::io::stdin().lock())? {
                bail!("not turned on: nothing was changed");
            }
            check(&daemon.post("/api/remote/approvals/enable", json!({}))?)?;
            println!("approving from the phone is on. Each phone needs Face ID set up once: on the phone, tap Enable Face ID, then run `sbxw remote approvals allow <device>` here.");
        }
        ApprovalsCmd::Off => {
            check(&daemon.post("/api/remote/approvals/disable", json!({}))?)?;
            println!("approving from the phone is off. Passkeys are kept; `sbxw remote approvals revoke <device>` drops one.");
        }
        ApprovalsCmd::Allow { device } => allow(daemon, &device, terminal)?,
        ApprovalsCmd::Revoke { device } => {
            check(&daemon.delete(&format!("/api/remote/approvals/devices/{}/passkey", url_segment(&device)))?)?;
            println!("device {device} can no longer approve from the phone.");
        }
    }
    Ok(())
}

fn allow(daemon: &Daemon, device: &str, terminal: bool) -> Result<()> {
    if !terminal {
        bail!("allowing a device needs you at a terminal");
    }
    let st = daemon.get("/api/remote/approvals")?;
    if st["enabled"] != true {
        bail!("approving from the phone is off: `sbxw remote approvals on` first");
    }
    let find = |st: &Value| st["devices"].as_array().and_then(|l| l.iter().find(|d| s(&d["id"]) == device).cloned());
    let Some(d) = find(&st) else { bail!("no paired device '{device}': `sbxw remote devices` lists them") };
    let name: String = s(&d["name"]).chars().filter(|c| !c.is_control()).collect();
    if d["request"].is_null() {
        println!("On the phone \"{name}\", tap Enable Face ID. Waiting for it (5 minutes)...");
    }
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    loop {
        let st = daemon.get("/api/remote/approvals")?;
        if find(&st).is_some_and(|d| !d["request"].is_null()) {
            break;
        }
        if Instant::now() > deadline {
            bail!("the phone did not ask in 5 minutes: run this again");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("\n\"{name}\" asks to approve with Face ID.");
    println!("The phone shows six digits. Type them here to allow it (Enter alone refuses).\n");
    loop {
        let digits = read_digits(&mut std::io::stdin().lock())?;
        if digits.is_empty() {
            check(&daemon.post(&format!("/api/remote/approvals/devices/{}/refuse", url_segment(device)), json!({}))?)?;
            println!("refused: nothing changed.");
            return Ok(());
        }
        match daemon.post(&format!("/api/remote/approvals/devices/{}/allow", url_segment(device)), json!({ "check": digits })) {
            Ok(r) => {
                check(&r)?;
                println!("allowed for 5 minutes, once: finish on the phone with Face ID.");
                return Ok(());
            }
            Err(e) if e.to_string().contains("tries left") => println!("{e:#}"),
            Err(e) => bail!("{e:#}"),
        }
    }
}

pub fn render_status(st: &Value) -> Vec<String> {
    let mut out = vec![format!("approving from the phone: {}", if st["enabled"] == true { "on" } else { "off" })];
    let devices = st["devices"].as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        out.push("no paired device".into());
    }
    for d in devices {
        let face = match (&d["passkey"], &d["request"], &d["granted_until_ms"]) {
            (_, _, g) if !g.is_null() => "allowed, waiting for Face ID on the phone".to_string(),
            (_, r, _) if !r.is_null() => "asks for Face ID: sbxw remote approvals allow <device>".to_string(),
            (p, _, _) if !p.is_null() => format!("Face ID set up {}", ago(&p["created_ms"])),
            _ => "no Face ID".to_string(),
        };
        let name: String = s(&d["name"]).chars().filter(|c| !c.is_control()).collect();
        out.push(format!("{}  {}  {face}", s(&d["id"]), name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines() {
        let st = json!({ "enabled": true, "devices": [
            { "id": "a1", "name": "iPhone", "passkey": null, "request": null, "granted_until_ms": null },
            { "id": "b2", "name": "iPad\u{1b}[2J", "passkey": { "created_ms": 0, "rp_id": "x.y" }, "request": null, "granted_until_ms": null },
            { "id": "c3", "name": "c", "passkey": null, "request": { "id": "r" }, "granted_until_ms": null },
        ]});
        let l = render_status(&st);
        assert_eq!(l[0], "approving from the phone: on");
        assert!(l[1].ends_with("no Face ID"));
        assert!(l[2].contains("Face ID set up") && !l[2].contains('\u{1b}'), "{:?}", l[2]);
        assert!(l[3].contains("allow"));
        assert_eq!(render_status(&json!({ "enabled": false, "devices": [] })), ["approving from the phone: off", "no paired device"]);
    }

    #[test]
    fn on_asks_and_says_what_it_allows() {
        assert!(ON_QUESTION.contains("Face ID at each action") && ON_QUESTION.ends_with("[y/N] "));
        assert!(!ask(ON_QUESTION, true, &mut "\n".as_bytes()).unwrap());
        assert!(ask(ON_QUESTION, false, &mut "y\n".as_bytes()).is_err(), "no terminal, no yes");
        for t in [ON_QUESTION] {
            assert!(!t.contains('\u{2014}'), "no em dash");
        }
    }
}
