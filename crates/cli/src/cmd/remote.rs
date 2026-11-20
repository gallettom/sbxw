//! `sbxw remote`: remote access from a terminal, until the desktop app has a
//! panel for it. Everything goes through the daemon's admin routes on the UI
//! listener (`/api/remote*`, see `docs/security.md#remote-access`).
//!
//! `pair` prints the link only: drawing a QR code in the terminal would need
//! a new crate (the workspace has no QR encoder). The confirmation on the Mac is the person TYPING the six
//! digits the phone shows: the Mac never prints them, so they must compare.

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
    /// Where the set-up stands (one line), what to do next, paired devices, the team leaders a phone sees.
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
    /// Pair a phone: prints a one-time link to open ON THE PHONE, then asks for the six digits it shows.
    Pair,
    /// The paired devices.
    Devices,
    /// Unpair a device now (its open pages lose access at once).
    Revoke { id: String },
    /// Turn a device's notifications off (it stays paired; the phone can turn them on again).
    NotificationsOff { id: String },
    /// Log the tailnet machine out and delete its state (remote access must be off).
    ForgetNode,
    /// What paired devices did, and when (never what they wrote).
    Journal {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },    /// Approving from the phone with Face ID (level 2): on, off, status, allow a device, revoke its passkey.
    #[command(subcommand)]
    Approvals(super::remote_approvals::ApprovalsCmd),
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
                let push = if d["notifications"].as_bool().unwrap_or(false) { "  notifications on" } else { "" };
                println!("{}  {}  paired {}  last seen {}{push}", s(&d["id"]), s(&d["name"]), ago(&d["created_ms"]), ago(&d["last_seen_ms"]));
            }
        }
        RemoteCmd::Revoke { id } => {
            check(&daemon.delete(&format!("/api/remote/devices/{}", url_segment(&id)))?)?;
            println!("device {id} is unpaired");
        }
        RemoteCmd::NotificationsOff { id } => {
            check(&daemon.delete(&format!("/api/remote/devices/{}/push", url_segment(&id)))?)?;
            println!("notifications are off for device {id}");
        }
        RemoteCmd::ForgetNode => {
            check(&daemon.post("/api/remote/tailnet/forget", json!({}))?)?;
            println!("the tailnet machine is forgotten. Remove `sbxw` from your tailnet's Machines page if it is still listed.");
        }
        RemoteCmd::Approvals(cmd) => super::remote_approvals::run(&daemon, cmd, terminal)?,
        RemoteCmd::Journal { limit } => {
            let list = daemon.get(&format!("/api/remote/journal?limit={limit}"))?;
            for e in list.as_array().cloned().unwrap_or_default() {
                println!("{}  {:<10}  {} {}", ago(&e["at_ms"]), s(&e["kind"]), s(&e["device_id"]), s(&e["device_name"]));
            }
        }
    }
    Ok(())
}

pub(super) fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// A device id is hex; anything else is sent percent-encoded, never as a path.
pub(super) fn url_segment(id: &str) -> String {
    id.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub(super) fn check(r: &Value) -> Result<()> {
    if r["ok"].as_bool() == Some(false) {
        bail!("{}", r["error"].as_str().unwrap_or("the daemon refused"));
    }
    Ok(())
}

pub(super) fn ago(ms: &Value) -> String {
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
    let (out, hints) = render_status(st);
    for l in out {
        println!("{l}");
    }
    for h in hints {
        eprintln!("{h}");
    }
}

/// The sentence shown when no leader is running.
pub const NO_LEADER: &str = "No team leader: turn the team on in a sandbox (the Team button of its pane, or /lead).";

/// One line per stage of the set-up.
fn stage(st: &Value) -> String {
    if st["sidecar_missing"].is_string() {
        return "sidecar missing".into();
    }
    if !st["enabled"].as_bool().unwrap_or(false) {
        return "off".into();
    }
    let t = &st["tailnet"];
    let name = t["name"].as_str().unwrap_or("");
    match t["state"].as_str() {
        None if st["listening"].as_bool().unwrap_or(false) => "starting".into(),
        None => "not running".into(),
        Some("starting") => "starting".into(),
        Some("needs_login") if t["login_url"].is_string() => "waiting for login".into(),
        Some("needs_login") => "waiting for approval".into(),
        Some("running") if t["serving"].as_bool().unwrap_or(false) => format!("running at {name}"),
        Some("running") => "starting".into(),
        Some("https_off") => "https unavailable".into(),
        Some("stopped") => "stopped".into(),
        Some(_) => "error".into(),
    }
}

/// What `status` prints: the data on stdout, then what to do on stderr.
/// Never a token, a pairing code, a node key or a login name: the login
/// link is the one thing the person must open.
pub fn render_status(st: &Value) -> (Vec<String>, Vec<String>) {
    let on = st["enabled"].as_bool().unwrap_or(false);
    let stage = stage(st);
    let t = &st["tailnet"];
    let mut out = vec![format!("remote access: {}", if on { "on" } else { "off" }), format!("state:         {stage}")];
    let mut hints = Vec::new();
    if let Some(u) = t["login_url"].as_str() {
        out.push(format!("login link:    {u}"));
    }
    if let Some(d) = t["detail"].as_str() {
        if stage == "error" || stage == "https unavailable" {
            out.push(format!("detail:        {d}"));
        }
    }
    out.push(format!("listener:      {} ({})", if st["listening"].as_bool().unwrap_or(false) { "listening" } else { "not listening" }, s(&st["remote_addr"])));
    out.push(format!("devices:       {} paired, {} waiting", st["devices"].as_u64().unwrap_or(0), st["pending_requests"].as_u64().unwrap_or(0)));
    let push = st["push_devices"].as_u64().unwrap_or(0);
    out.push(format!("notifications: on for {push} device{}", if push == 1 { "" } else { "s" }));
    let leaders: Vec<String> = st["leaders"].as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    out.push(format!("team leaders:  {}", if leaders.is_empty() { "none".to_string() } else { leaders.join(", ") }));
    let awake = match st["keep_awake"].as_str().unwrap_or("") {
        "holding" => "kept awake (on mains power)",
        "on_battery" => "may sleep (on battery)",
        "not_macos" => "not handled on this system",
        _ => "not held",
    };
    out.push(format!("keep awake:    {awake}"));

    match stage.as_str() {
        "sidecar missing" => {
            let path = std::path::PathBuf::from(s(&st["sidecar_missing"]));
            hints.extend(sbxw_daemon::svc::remote_access::sidecar_missing_message(&path).lines().map(str::to_string));
            hints.push("Then: sbxw remote on".into());
        }
        "off" => hints.push("Turn it on: sbxw remote on".into()),
        "not running" => hints.push("It could not start. Turn it off and on to see why: sbxw remote off, then sbxw remote on".into()),
        "starting" => hints.push("The tailnet machine is starting: run sbxw remote status again in a few seconds.".into()),
        "waiting for login" => hints.push("Next: open the login link in any browser (on this Mac is fine) and add the machine to your tailnet.".into()),
        "waiting for approval" => {
            hints.push("Next: approve the machine \"sbxw\" on the Machines page of your Tailscale admin console (login.tailscale.com).".into())
        }
        "https unavailable" => hints.push(
            "Next: in the Tailscale admin console (login.tailscale.com), open the DNS page and turn on MagicDNS and HTTPS Certificates. Then: sbxw remote off, then sbxw remote on".into(),
        ),
        "error" | "stopped" => hints.push("Turn it off and on to try again: sbxw remote off, then sbxw remote on".into()),
        _ => {
            if st["devices"].as_u64().unwrap_or(0) == 0 {
                hints.push("Next: pair your phone with sbxw remote pair".into());
            }
        }
    }
    if on && leaders.is_empty() {
        hints.push(NO_LEADER.into());
    }
    (out, hints)
}

/// Show the link, wait for the phone, show its name and check, ask.
fn pair(daemon: &Daemon, terminal: bool) -> Result<()> {
    if !terminal {
        bail!("pairing needs you at a terminal");
    }
    let r = daemon.post("/api/remote/pairing", json!({}))?;
    check(&r)?;
    let code_id = s(&r["code_id"]);
    for l in pair_intro(&s(&r["url"])) {
        println!("{l}");
    }
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    // Only a request made with THIS link: an older one died with the new code.
    let req = loop {
        if Instant::now() > deadline {
            bail!("no phone used the link in 5 minutes: run `sbxw remote pair` again");
        }
        let list = daemon.get("/api/remote/pairing/requests")?;
        if let Some(mine) = list.as_array().and_then(|l| l.iter().find(|q| s(&q["code_id"]) == code_id)) {
            break mine.clone();
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    println!("\nA phone asks to pair: \"{}\"", s(&req["device_name"]).chars().filter(|c| !c.is_control()).collect::<String>());
    println!("The phone shows six digits. Type them here to pair it (Enter alone refuses).\n");
    let id = s(&req["id"]);
    loop {
        let digits = read_digits(&mut std::io::stdin().lock())?;
        if digits.is_empty() {
            check(&daemon.post(&format!("/api/remote/pairing/requests/{}/refuse", url_segment(&id)), json!({}))?)?;
            println!("refused: the phone is not paired.");
            return Ok(());
        }
        match daemon.post(&format!("/api/remote/pairing/requests/{}/approve", url_segment(&id)), json!({ "check": digits })) {
            Ok(r) => {
                check(&r)?;
                println!("paired. `sbxw remote devices` lists it; `sbxw remote revoke <id>` unpairs it.");
                return Ok(());
            }
            Err(e) if e.to_string().contains("tries left") => println!("{e:#}"),
            Err(e) => bail!("{e:#}"),
        }
    }
}

/// What `pair` says before it waits: where to open the link, then the link.
pub fn pair_intro(url: &str) -> Vec<String> {
    vec![
        "Open this link on your phone (Tailscale on and signed in to the same account). It does not open on this Mac.".into(),
        "It works once and expires in 5 minutes. The whole link is needed, including the part after #.".into(),
        String::new(),
        format!("  {url}"),
        String::new(),
        "To get it to the phone: AirDrop it, or paste it in a note that syncs (Notes), then open it there.".into(),
        "Waiting for the phone...".into(),
    ]
}

/// The digits as typed: spaces dropped; anything but six digits asks again.
pub(super) fn read_digits(input: &mut impl BufRead) -> Result<String> {
    loop {
        eprint!("Digits on the phone: ");
        std::io::stderr().flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(String::new());
        }
        let d: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if d.is_empty() || (d.len() == 6 && d.bytes().all(|b| b.is_ascii_digit())) {
            return Ok(d);
        }
        eprintln!("six digits, please (Enter alone refuses)");
    }
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
    fn digits_are_six_or_nothing() {
        assert_eq!(read_digits(&mut "12 34 56\n".as_bytes()).unwrap(), "123456");
        assert_eq!(read_digits(&mut "\n".as_bytes()).unwrap(), "");
        assert_eq!(read_digits(&mut "abc\n1234567\n654321\n".as_bytes()).unwrap(), "654321");
        assert_eq!(read_digits(&mut "".as_bytes()).unwrap(), "", "end of input refuses");
    }

    fn base() -> Value {
        json!({"enabled": true, "remote_addr": "127.0.0.1:7683", "listening": true, "tailnet": null, "devices": 0, "pending_requests": 0,
               "keep_awake": "holding", "sidecar_missing": null, "leaders": ["lead1"]})
    }

    fn render(v: Value) -> String {
        let (out, hints) = render_status(&v);
        format!("{}\n--\n{}", out.join("\n"), hints.join("\n"))
    }

    #[test]
    fn status_off() {
        let mut v = base();
        v["enabled"] = false.into();
        v["listening"] = false.into();
        v["leaders"] = json!([]);
        assert_eq!(
            render(v),
            "remote access: off\nstate:         off\nlistener:      not listening (127.0.0.1:7683)\ndevices:       0 paired, 0 waiting\nnotifications: on for 0 devices\n\
             team leaders:  none\nkeep awake:    kept awake (on mains power)\n--\nTurn it on: sbxw remote on"
        );
    }

    #[test]
    fn status_counts_devices_with_notifications_on() {
        let mut v = base();
        v["push_devices"] = 1.into();
        assert!(render(v.clone()).contains("\nnotifications: on for 1 device\n"), "{}", render(v.clone()));
        v["push_devices"] = 2.into();
        assert!(render(v).contains("\nnotifications: on for 2 devices\n"));
    }

    #[test]
    fn status_sidecar_missing() {
        let mut v = base();
        v["enabled"] = false.into();
        v["sidecar_missing"] = "/Users/x/dev/sbxw/target/debug/sbxw-tailnet".into();
        let r = render(v);
        assert!(r.contains("state:         sidecar missing\n"), "{r}");
        let hints = r.split("\n--\n").nth(1).unwrap();
        assert!(hints.starts_with("sbxw-tailnet is missing: sbxw looks for it at /Users/x/dev/sbxw/target/debug/sbxw-tailnet\n"), "{hints}");
        assert!(hints.contains("scripts/build-tailnet.sh "), "{hints}");
        assert!(hints.ends_with("'/Users/x/dev/sbxw/target/debug/sbxw-tailnet'\nThen: sbxw remote on"), "{hints}");
    }

    #[test]
    fn status_starting() {
        let r = render(base());
        assert!(r.contains("state:         starting\n"), "{r}");
        assert!(r.ends_with("--\nThe tailnet machine is starting: run sbxw remote status again in a few seconds."), "{r}");
        let mut v = base();
        v["tailnet"] = json!({"state": "running", "name": "sbxw.tail1.ts.net", "serving": false});
        assert!(render(v).contains("state:         starting\n"), "a running node not serving yet is still starting");
        let mut v = base();
        v["listening"] = false.into();
        assert!(render(v).contains("state:         not running\n"));
    }

    #[test]
    fn status_waiting_for_login() {
        let mut v = base();
        v["tailnet"] = json!({"state": "needs_login", "name": null, "serving": false, "login_url": "https://login.tailscale.com/a/xyz",
                              "detail": "Open the link to add this Mac's sbxw node to your tailnet."});
        assert_eq!(
            render(v),
            "remote access: on\nstate:         waiting for login\nlogin link:    https://login.tailscale.com/a/xyz\n\
             listener:      listening (127.0.0.1:7683)\ndevices:       0 paired, 0 waiting\nnotifications: on for 0 devices\nteam leaders:  lead1\nkeep awake:    kept awake (on mains power)\n--\n\
             Next: open the login link in any browser (on this Mac is fine) and add the machine to your tailnet."
        );
    }

    #[test]
    fn status_waiting_for_approval() {
        let mut v = base();
        v["tailnet"] = json!({"state": "needs_login", "name": null, "serving": false, "login_url": null, "detail": "Approve the sbxw node."});
        let r = render(v);
        assert!(r.contains("state:         waiting for approval\n") && !r.contains("login link"), "{r}");
        assert!(r.contains("Machines page of your Tailscale admin console (login.tailscale.com)"), "{r}");
    }

    #[test]
    fn status_https_unavailable() {
        let mut v = base();
        v["tailnet"] = json!({"state": "https_off", "name": "sbxw.tail1.ts.net", "serving": false,
                              "detail": "MagicDNS is off in this tailnet. Turn it on at https://login.tailscale.com/admin/dns, then turn remote access off and on."});
        let r = render(v);
        assert!(r.contains("state:         https unavailable\ndetail:        MagicDNS is off"), "{r}");
        assert!(r.contains("open the DNS page and turn on MagicDNS and HTTPS Certificates"), "{r}");
    }

    #[test]
    fn status_running_lists_leaders_or_says_how_to_start_one() {
        let mut v = base();
        v["tailnet"] = json!({"state": "running", "name": "sbxw.tail1.ts.net", "serving": true});
        v["leaders"] = json!(["alpha", "beta"]);
        let r = render(v.clone());
        assert!(r.contains("state:         running at sbxw.tail1.ts.net\n"), "{r}");
        assert!(r.contains("team leaders:  alpha, beta\n"), "{r}");
        assert!(r.ends_with("--\nNext: pair your phone with sbxw remote pair"), "{r}");
        v["leaders"] = json!([]);
        v["devices"] = 1.into();
        let r = render(v);
        assert!(r.contains("team leaders:  none\n"), "{r}");
        assert!(r.ends_with(&format!("--\n{NO_LEADER}")), "{r}");
    }

    #[test]
    fn status_error() {
        let mut v = base();
        v["tailnet"] = json!({"state": "error", "detail": "No login link from Tailscale yet."});
        let r = render(v);
        assert!(r.contains("state:         error\ndetail:        No login link from Tailscale yet.\n"), "{r}");
    }

    #[test]
    fn pair_says_the_phone_not_the_mac() {
        let intro = pair_intro("https://sbxw.tail1.ts.net/#pair=abc").join("\n");
        let link = intro.find("  https://sbxw.tail1.ts.net/#pair=abc").unwrap();
        let phone = intro.find("Open this link on your phone").unwrap();
        assert!(phone < link, "the instruction comes before the link");
        assert!(intro.contains("It does not open on this Mac."));
        assert!(intro.contains("expires in 5 minutes") && intro.contains("including the part after #"));
        assert!(intro.contains("AirDrop"));
        assert_eq!(intro.matches("abc").count(), 1, "the code only inside the link");
    }

    #[test]
    fn no_em_dash_anywhere() {
        for t in [render(base()), pair_intro("u").join(" "), NO_LEADER.into(), ON_QUESTION.into()] {
            assert!(!t.contains('\u{2014}'), "{t}");
        }
    }

    #[test]
    fn ids_never_become_paths() {
        assert_eq!(url_segment("ab12"), "ab12");
        assert_eq!(url_segment("../x"), "%2E%2E%2Fx");
    }
}
