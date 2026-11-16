//! The virtual Mac (`assets/js/mac.js`, `assets/terminal-mac/README.md`):
//! the terminal-mac VM sandboxes with the `sbxw:terminal-mac` kit borrow.
//!
//! One panel over `GET /api/mac` (key `mac`, the whole answer `{ok, mac}`): whether terminal-mac is
//! installed, the VM's state, who holds the lease, the guest's one-time
//! setup, the requests waiting on the person (sbxw's words and the sandbox's
//! reason apart), the idle timers. Buttons as the web UI's: install, start
//! (or resume, or create), pause, shut down, force off, take the lease back,
//! open its window. Install and boot take minutes: the daemon starts them
//! and answers at once; the panel follows `busy` on the `mac` event and a
//! read every 10 s.
//!
//! **The live view is the VM's own window** (`POST /api/mac/show`, opened on
//! the host by terminal-mac), as in the web UI. The daemon has no route that
//! hands the person's UI the guest's screen or takes its input (screenshots
//! and input exist only on the agent listener, for the lease holder), so an
//! in-app live screen waits on one (BROWSER-MAC.md says which).
//!
//! The rule of this area: **what downloads, what loses work and what takes
//! the Mac from a sandbox goes through the core's dialog**: installing
//! (a download, a self-signed release), creating the VM (macOS downloaded
//! from Apple, several GB), shutting down, forcing off, taking the lease
//! back. Starting or resuming an existing VM, pausing and opening its
//! window are direct, as in the web UI. Every dialog is built here from
//! `data.mac`, never from a screen's words. A request's reason is the
//! sandbox's own text: the screen draws it as text under a label naming that
//! sandbox, never as sbxw's.

#[cfg(test)]
mod tests;

use super::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, ScreenDecl, Slot, Standin};
use crate::intent::{Risk, Spec};
use serde_json::{json, Value as J};

pub const SCREEN: &str = "mac";
pub const KEY: &str = "mac";

pub const FEATURE: Feature = Feature {
    name: "mac",
    data: &[Data { key: KEY, path: "/api/mac", on: &["mac"], every: 10, lazy: false }],
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../../screens/mac.luau"), slot: Slot::Panel { title: "Virtual Mac", icon: "single", width: 440.0 } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "mac_open", risk: Risk::Direct, args: &[], doc: "Opens the Virtual Mac panel and reads the Mac's state." },
    Spec { name: "mac_refresh", risk: Risk::Direct, args: &[], doc: "Reads the Mac's state again." },
    Spec { name: "mac_install", risk: Risk::Approval("Install terminal-mac"), args: &[], doc: "Downloads and installs the pinned terminal-mac release (SHA-256 checked), after the core's dialog." },
    Spec {
        name: "mac_up",
        risk: Risk::Direct,
        args: &[],
        doc: "Starts or resumes the VM. Creating it (state `missing`: macOS is downloaded from Apple, several GB) goes through the core's dialog first.",
    },
    Spec { name: "mac_pause", risk: Risk::Direct, args: &[], doc: "Pauses the running VM (its memory stays reserved; resuming is quick)." },
    Spec { name: "mac_window", risk: Risk::Direct, args: &[], doc: "Opens the VM's own window on this Mac (terminal-mac shows it): the live view." },
    Spec { name: "mac_stop", risk: Risk::Approval("Shut the virtual Mac down"), args: &[], doc: "Asks the VM to shut down, after the core's dialog: a sandbox using it loses it." },
    Spec { name: "mac_force_off", risk: Risk::Approval("Force the virtual Mac off"), args: &[], doc: "Powers the VM off at once, after the core's dialog: anything unsaved in the guest is lost." },
    Spec { name: "mac_take_back", risk: Risk::Approval("Take the virtual Mac back"), args: &[], doc: "Ends the lease the sandbox holding the Mac has, after the core's dialog." },
];

fn state(m: &J) -> &str {
    m["state"].as_str().unwrap_or("unknown")
}

/// Why nothing may be asked of the Mac now, from what the daemon said.
fn unavailable(m: &J) -> Option<String> {
    if m.is_null() || m.get("supported").is_none() {
        return Some("the Mac's state is not known yet: refresh the panel".into());
    }
    if m["supported"] != true {
        return Some("the virtual Mac needs an Apple silicon Mac (macOS 14 or later)".into());
    }
    if let Some(b) = m["busy"].as_str() {
        return Some(format!("sbxw is busy with the Mac: {b}"));
    }
    None
}

fn need_installed(m: &J) -> Result<(), String> {
    if let Some(why) = unavailable(m) {
        return Err(why);
    }
    if m["installed"] != true {
        return Err("terminal-mac is not installed".into());
    }
    Ok(())
}

fn post(route: &str, what: &str) -> Call {
    Call::post(format!("/api/mac/{route}"), json!({}), what).then(&[KEY]).tag(route)
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let m = &cx.get(KEY)["mac"];
    let st = state(m);
    Ok(match a.name {
        "mac_open" => vec![Action::OpenPanel(SCREEN.into()), Action::Refresh(KEY)],
        "mac_refresh" => vec![Action::Refresh(KEY)],
        "mac_install" => {
            if let Some(why) = unavailable(m) {
                return Err(why);
            }
            if m["installed"] == true {
                return Err("terminal-mac is already installed".into());
            }
            let pinned = m["pinned"].as_str().unwrap_or("?");
            vec![Confirm::new("Install terminal-mac?", post("install", "install terminal-mac"))
                .line(format!("sbxw downloads release {pinned} of terminal-mac (github.com/govijr/terminal-mac) and installs it under ~/.sbxw/terminal-mac, its SHA-256 checked against the one built into sbxw."))
                .line("It is an early release candidate from one author, self-signed and not notarised by Apple.")
                .line("Starting the VM afterwards downloads macOS from Apple (several GB).")
                .labels("Install", "Cancel")
                .into_action()]
        }
        "mac_up" => {
            need_installed(m)?;
            match st {
                "running" => return Err("the virtual Mac is already running".into()),
                "stopping" => return Err("the virtual Mac is shutting down: start it once it has stopped".into()),
                "starting" => return Err("the virtual Mac is already starting".into()),
                _ => {}
            }
            let call = post("up", if st == "paused" { "resume the virtual Mac" } else { "start the virtual Mac" }).timeout(30);
            if st == "missing" {
                let vm = m["vm"].as_str().unwrap_or("?");
                vec![Confirm::new(format!("Create the virtual Mac \"{vm}\"?"), call)
                    .line("terminal-mac downloads macOS from Apple (several GB) and installs it in a new VM on this Mac. It takes a while; the panel follows it.")
                    .line("Then you finish the guest's setup once, in its window (the panel lists the steps).")
                    .labels("Create and start", "Cancel")
                    .into_action()]
            } else {
                vec![Action::Call(call)]
            }
        }
        "mac_pause" => {
            need_installed(m)?;
            if st != "running" {
                return Err("only a running virtual Mac can be paused".into());
            }
            vec![Action::Call(post("pause", "pause the virtual Mac"))]
        }
        "mac_window" => {
            need_installed(m)?;
            if !matches!(st, "running" | "paused" | "stopping") {
                return Err("the virtual Mac is not running: start it first".into());
            }
            vec![Action::Call(post("show", "open the virtual Mac's window"))]
        }
        "mac_stop" => {
            need_installed(m)?;
            if !matches!(st, "running" | "paused") {
                return Err("the virtual Mac is not running".into());
            }
            let mut c = Confirm::new("Shut the virtual Mac down?", post("stop", "shut the virtual Mac down"))
                .line("The guest is asked to shut down normally.")
                .line("A sandbox using it loses it, and the ports opened for it are closed.");
            if let Some(h) = m["holder"].as_str() {
                c = c.line(format!("It is held by {h} now."));
            }
            vec![c.labels("Shut down", "Cancel").danger().into_action()]
        }
        "mac_force_off" => {
            need_installed(m)?;
            if !matches!(st, "running" | "paused" | "stopping") {
                return Err("the virtual Mac is not running".into());
            }
            let mut c = Confirm::new("Force the virtual Mac off now?", post("force-off", "force the virtual Mac off"))
                .line("It is powered off at once: anything unsaved in the guest is lost.");
            if let Some(since) = stopping_for(m) {
                c = c.line(format!("sbxw asked it to shut down {since} ago. A shutdown that hangs is usually a dialog in the guest: Open its window shows it."));
            }
            vec![c.labels("Force off", "Cancel").danger().into_action()]
        }
        "mac_take_back" => {
            let Some(h) = m["holder"].as_str() else { return Err("nobody holds the virtual Mac".into()) };
            vec![Confirm::new(format!("Take the virtual Mac back from {h}?"), post("take-back", format!("take the virtual Mac back from {h}").as_str()))
                .line(format!("{h}'s lease ends now: its agent's next mac_* call is refused until it asks again."))
                .line("The VM keeps running.")
                .labels("Take it back", "Cancel")
                .into_action()]
        }
        other => return Err(format!("{other}: not a Mac intent")),
    })
}

/// How long ago sbxw asked for the shutdown still under way, readable.
fn stopping_for(m: &J) -> Option<String> {
    let at = m["stop_requested_ms"].as_u64()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    let s = now.saturating_sub(at) / 1000;
    Some(if s < 120 { format!("{s} s") } else { format!("{} min", s / 60) })
}

/// The daemon's `text` (what terminal-mac said) as the notice, as the web
/// UI's toast; a failure in red with the daemon's words.
fn on_answer(_cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    match r {
        Err(e) => vec![Action::error(format!("Could not {}: {e}", call.what))],
        Ok((code, b)) if !(200..300).contains(&code) || b["ok"] == false => {
            vec![Action::error(format!("Could not {}: {}", call.what, b["error"].as_str().unwrap_or("sbxw refused it")))]
        }
        Ok((_, b)) => match (b["text"].as_str(), b["started"] == true) {
            (Some(t), _) if !t.trim().is_empty() => vec![Action::notice(one_line(t))],
            (_, true) => vec![Action::notice(format!("Started: {}. The panel follows it.", call.what))],
            _ => vec![Action::notice(format!("Done: {}", call.what))],
        },
    }
}

/// What terminal-mac said, on one line and bounded, for a notice.
fn one_line(t: &str) -> String {
    let s: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s
    }
}

/// A top bar button on a host that can run the Mac: its state at a glance,
/// amber when something waits on the person.
fn extras(cx: &Ctx) -> Vec<Extra> {
    let m = &cx.get(KEY)["mac"];
    if m["supported"] != true {
        return vec![];
    }
    let pending = m["pending"].as_array().map_or(0, Vec::len) as u64;
    let st = state(m);
    let (tone, help) = if pending > 0 {
        ("amber", format!("The virtual Mac: {pending} request(s) wait for you"))
    } else if m["error"].is_string() {
        ("red", "The virtual Mac: the last action failed".to_string())
    } else if m["busy"].is_string() {
        ("blue", format!("The virtual Mac: {}", m["busy"].as_str().unwrap_or("")))
    } else if m["installed"] != true {
        ("muted", "The virtual Mac: terminal-mac is not installed".to_string())
    } else if st == "running" && m["guest_ready"] != true {
        ("amber", "The virtual Mac is running; its guest setup is not finished".to_string())
    } else if st == "running" {
        ("green", match m["holder"].as_str() { Some(h) => format!("The virtual Mac is running, held by {h}"), None => "The virtual Mac is running, free".into() })
    } else {
        ("muted", format!("The virtual Mac is {st}"))
    };
    vec![Extra {
        place: Place::Topbar,
        id: "mac".into(),
        label: "Mac".into(),
        icon: Some("single".into()),
        tone: tone.into(),
        help,
        count: (pending > 0).then_some(pending),
        intent: Some(("mac_open".into(), json!({}))),
    }]
}

/// The stand-in daemon's Mac: the fixture's `mac` answers `GET /api/mac`;
/// the buttons move it as the daemon would, a job taking one `mac` event.
fn standin(st: &mut Standin, method: &str, path: &str, _body: &J) -> Option<(u16, J)> {
    let path = path.split('?').next().unwrap_or("");
    if (method, path) == ("GET", "/api/mac") {
        // The fixture's key is the route's whole answer.
        let a = st.data[KEY].clone();
        return Some((200, if a.is_null() { json!({ "ok": true, "mac": unsupported() }) } else { a }));
    }
    let route = path.strip_prefix("/api/mac/")?;
    if method != "POST" || !["install", "up", "stop", "force-off", "pause", "show", "take-back"].contains(&route) {
        return None;
    }
    if st.data[KEY].is_null() {
        st.data[KEY] = json!({ "ok": true, "mac": unsupported() });
    }
    let m = &mut st.data[KEY]["mac"];
    let bad = |e: &str| Some((500, json!({ "ok": false, "error": e })));
    if m["supported"] != true {
        return bad("terminal-mac needs an Apple silicon Mac (macOS 14 or later)");
    }
    let s = state(m).to_string();
    let text = |t: &str| Some((200, json!({ "ok": true, "text": t })));
    let answer = match route {
        "install" => {
            m["installed"] = true.into();
            m["state"] = "missing".into();
            m["program"] = "/Users/me/.sbxw/terminal-mac/bin/terminal-mac".into();
            Some((200, json!({ "ok": true, "started": true })))
        }
        "up" => {
            if m["installed"] != true {
                return bad("terminal-mac is not installed");
            }
            m["state"] = "running".into();
            m["managed"] = true.into();
            // A VM just made has its guest setup still to do.
            if s == "missing" {
                m["guest_ready"] = false.into();
            }
            Some((200, json!({ "ok": true, "started": true })))
        }
        "pause" if s == "running" => {
            m["state"] = "paused".into();
            text("VM paused")
        }
        "stop" if s == "running" || s == "paused" => {
            m["state"] = "stopped".into();
            m["holder"] = J::Null;
            m["ports"] = json!([]);
            text("VM stopped")
        }
        "force-off" => {
            m["state"] = "stopped".into();
            m["holder"] = J::Null;
            m["stop_requested_ms"] = J::Null;
            text("VM forced off")
        }
        "show" => text("VM window shown"),
        "take-back" => {
            let who = m["holder"].as_str().map(str::to_string);
            m["holder"] = J::Null;
            text(&match who {
                Some(w) => format!("taken back from '{w}'"),
                None => "nobody held it".into(),
            })
        }
        _ => return bad(&format!("the VM is {s}")),
    };
    (st.publish)("mac", json!({}));
    answer
}

fn unsupported() -> J {
    json!({ "supported": false, "installed": false, "program": null, "pinned": "", "vm": "", "state": "missing", "guest_ready": false, "guest": null,
            "holder": null, "managed": false, "busy": null, "error": null, "ports": [], "settings": {}, "stop_requested_ms": null, "pending": [] })
}
