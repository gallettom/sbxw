//! What the web UI's main.js, lifecycle.js and help.js do that no other
//! area has: a sandbox's lifecycle actions (remove, recreate, duplicate,
//! close its terminals), the usage gauge in the header, the versions, the
//! help dialog and the keyboard shortcuts.
//!
//! Removing and recreating a sandbox destroy what is inside it: both are of
//! risk Approval, confirmed in the core's dialog with the sandbox's name and
//! its workspace from the daemon's list. Duplicating and closing terminals
//! are direct, as in the web UI.

use super::*;
use crate::intent::{Arg, Risk, Spec};
use serde_json::json;

pub const FEATURE: Feature = Feature {
    name: "shell",
    data: &[
        Data { key: "usage", path: "/api/usage", on: &[], every: 30, lazy: false },
        Data { key: "daemon_state", path: "/api/state", on: &[], every: 0, lazy: false },
    ],
    screens: &[
        ScreenDecl { name: "help", src: include_str!("../../screens/help.luau"), slot: Slot::Modal { width: 620.0 } },
        ScreenDecl { name: "sandbox-actions", src: include_str!("../../screens/sandbox-actions.luau"), slot: Slot::Modal { width: 460.0 } },
    ],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

const INTENTS: &[Spec] = &[
    Spec {
        name: "sandbox_remove",
        risk: Risk::Approval("Remove a sandbox"),
        args: &[("sandbox", Arg::Str, true)],
        doc: "Removes a sandbox for good (`sbx rm`), after the core's dialog; its terminals close.",
    },
    Spec {
        name: "sandbox_recreate",
        risk: Risk::Approval("Recreate a sandbox"),
        args: &[("sandbox", Arg::Str, true)],
        doc: "Removes a sandbox an older sbx made and creates it again with this sbx (its workspace is kept), after the core's dialog.",
    },
    Spec { name: "sandbox_duplicate", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("new_name", Arg::Str, true)], doc: "Creates a new sandbox on the same workspace (`POST .../duplicate`)." },
    Spec { name: "sandbox_detach", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Closes every terminal sbxw holds on the sandbox (`sbxw down`); the sandbox keeps running." },
    Spec { name: "show_help", risk: Risk::Direct, args: &[], doc: "Opens the help dialog: why the list may be empty, the keyboard shortcuts, the versions." },
    Spec {
        name: "screen_restore",
        risk: Risk::Approval("Restore a built-in screen"),
        args: &[("screen", Arg::Str, true)],
        doc: "Deletes a screen's override file (`~/.sbxw/desktop/screens/<name>.luau`) after the core's dialog, which names the file: the built-in screen comes back.",
    },
];

/// A valid sandbox name, as the web UI checks it.
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty() && b.len() <= 63 && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-') && b[0] != b'-' && b[b.len() - 1] != b'-'
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let enc = crate::client::enc;
    Ok(match a.name {
        "show_help" => vec![Action::OpenModal { screen: "help".into(), arg: J::Null }],
        "screen_restore" => {
            let name = a.req("screen");
            let o = &cx.get("app")["overrides"][name];
            let path = o["path"].as_str().ok_or_else(|| format!("the {name} screen has no override to remove"))?;
            let mut call = Call::post(format!("core:screen-restore/{name}"), J::Null, format!("restore the built-in {name} screen"));
            call.method = Method::Core;
            let mut c = Confirm::new(format!("Restore the built-in {name} screen?"), call).line(format!("This deletes your customised copy: {path}"));
            if o["stale"] == true {
                c = c.line(format!("It was made for an older version of the app, so features added since may be missing from your {name} screen."));
            }
            vec![c.line("Its changes are lost; Jarvis can make them again on the new built-in.").labels("Delete and restore", "Keep my copy").danger().into_action()]
        }
        "sandbox_remove" | "sandbox_recreate" => {
            let name = a.req("sandbox");
            let sb = cx.sandbox(name).ok_or_else(|| format!("no sandbox named {name} in the list"))?;
            let ws = sb["workspace"].as_str().filter(|w| !w.is_empty());
            if a.name == "sandbox_remove" {
                let mut c = Confirm::new(format!("Remove {name}?"), Call::post(format!("/api/sandboxes/{}/rm", enc(name)), json!({}), format!("remove {name}")).timeout(300).tag("rm"))
                    .line(format!("Sandbox: {name} ({}).", if sb["chat"] == true { "a chat" } else { sb["agent"].as_str().unwrap_or("agent") }))
                    .line("This is irreversible: everything inside the sandbox is lost (installed packages, files outside its workspace, its thread records). Its terminals close.");
                if let Some(w) = ws {
                    c = c.line(format!("Its workspace folder stays on this machine: {w}"));
                }
                vec![c.labels("Remove", "Cancel").danger().into_action()]
            } else {
                let mut c = Confirm::new(format!("Recreate {name} with this sbx?"), Call::post(format!("/api/sandboxes/{}/recreate", enc(name)), json!({}), format!("recreate {name}")).timeout(1800).tag("recreate"))
                    .line(format!("Sandbox: {name}."));
                c = c.line(match ws {
                    Some(w) => format!("Its workspace folder is kept: {w}"),
                    None => "It has no workspace folder: nothing of it is kept.".to_string(),
                });
                vec![c
                    .line("Everything else in the sandbox is lost: files outside the workspace, installed packages, unpushed commits in other folders, its thread records.")
                    .labels("Recreate", "Cancel")
                    .danger()
                    .into_action()]
            }
        }
        "sandbox_duplicate" => {
            let (src, new) = (a.req("sandbox"), a.req("new_name").trim());
            if !valid_name(new) {
                return Err("lowercase letters, digits and hyphens only, starting and ending with a letter or digit".into());
            }
            vec![Action::CloseModal, Action::Call(Call::post(format!("/api/sandboxes/{}/duplicate", enc(src)), json!({ "new_name": new }), format!("duplicate {src} as {new}")).timeout(1800).tag("duplicate"))]
        }
        "sandbox_detach" => vec![Action::Call(Call::post(format!("/api/sandboxes/{}/detach", enc(a.req("sandbox"))), json!({}), format!("close {}'s terminals", a.req("sandbox"))))],
        other => return Err(format!("{other}: not a shell intent")),
    })
}

fn on_answer(_cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let mut v = match r {
        Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
            let warn = body["warnings"].as_array().and_then(|w| w.last()).and_then(J::as_str);
            match warn {
                Some(w) => vec![Action::error(format!("{}: done, but {w}", call.what))],
                None => vec![Action::notice(format!("{}: done", call.what))],
            }
        }
        Ok((c, body)) => vec![Action::error(format!("{}: {}", call.what, body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"))))],
        Err(e) => vec![Action::error(format!("{}: {e}", call.what))],
    };
    if call.tag == "rm" || call.tag == "recreate" {
        v.push(Action::CloseModal);
    }
    v
}

/// `42%`, or nothing.
fn pct(v: &J) -> Option<String> {
    v.as_f64().map(|p| format!("{}%", p.round()))
}

fn extras(cx: &Ctx) -> Vec<Extra> {
    let mut v = vec![];
    let u = cx.get("usage");
    let (five, week) = (pct(&u["five_hour_pct"]), pct(&u["seven_day_pct"]));
    if u["updated_ms"].as_u64().unwrap_or(0) > 0 && (five.is_some() || week.is_some()) {
        let label = [five.map(|p| format!("5h {p}")), week.map(|p| format!("7d {p}"))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let high = [u["five_hour_pct"].as_f64(), u["seven_day_pct"].as_f64()].into_iter().flatten().any(|p| p >= 80.0);
        v.push(Extra {
            place: Place::Topbar,
            id: "usage".into(),
            label,
            icon: None,
            tone: if high { "amber".into() } else { "muted".into() },
            help: "Share of the subscription windows used (5 hours, 7 days)".into(),
            count: None,
            intent: None,
        });
    }
    v.push(Extra { place: Place::Topbar, id: "help".into(), label: String::new(), icon: Some("info".into()), tone: "muted".into(), help: "Help and keyboard shortcuts".into(), count: None, intent: Some(("show_help".into(), json!({}))) });
    for sb in cx.get("sandboxes").as_array().cloned().unwrap_or_default() {
        let name = sb["name"].as_str().unwrap_or("").to_string();
        v.push(Extra {
            place: Place::Sandbox(name.clone()),
            id: "more".into(),
            label: String::new(),
            icon: Some("more".into()),
            tone: "muted".into(),
            help: "Remove, duplicate, recreate…".into(),
            count: None,
            intent: Some(("open_modal".into(), json!({ "screen": "sandbox-actions", "arg": name }))),
        });
    }
    v
}

fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    if (method, path) == ("GET", "/api/usage") {
        let u = st.data["usage"].clone();
        return Some((200, if u.is_null() { json!({ "updated_ms": 0 }) } else { u }));
    }
    if (method, path) == ("GET", "/api/state") {
        let s = st.data["daemon_state"].clone();
        return Some((200, if s.is_null() { json!({ "versions": { "sbxw": "stand-in", "sbx": "0.53.0" } }) } else { s }));
    }
    if method != "POST" {
        return None;
    }
    let names = |d: &J| d["sandboxes"].as_array().map(|l| l.iter().filter_map(|s| s["name"].as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default();
    if let Some(c) = match_path("/api/sandboxes/{sandbox}/rm", path) {
        let n = c[0].1.clone();
        if !names(st.data).contains(&n) {
            return Some((404, json!({ "ok": false, "error": format!("no sandbox '{n}'") })));
        }
        if let Some(l) = st.data["sandboxes"].as_array_mut() {
            l.retain(|s| s["name"] != n.as_str());
        }
        return Some((200, json!({ "ok": true })));
    }
    if let Some(c) = match_path("/api/sandboxes/{sandbox}/duplicate", path) {
        let (src, new) = (c[0].1.clone(), body["new_name"].as_str().unwrap_or("").to_string());
        if names(st.data).contains(&new) {
            return Some((400, json!({ "ok": false, "error": format!("a sandbox named '{new}' already exists") })));
        }
        let Some(mut s) = st.data["sandboxes"].as_array().and_then(|l| l.iter().find(|s| s["name"] == src.as_str()).cloned()) else {
            return Some((404, json!({ "ok": false, "error": format!("no sandbox '{src}'") })));
        };
        s["name"] = new.into();
        s["status"] = "running".into();
        st.data["sandboxes"].as_array_mut().unwrap().push(s);
        return Some((200, json!({ "ok": true })));
    }
    if match_path("/api/sandboxes/{sandbox}/recreate", path).is_some() || match_path("/api/sandboxes/{sandbox}/detach", path).is_some() {
        return Some((200, json!({ "ok": true })));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> J {
        json!({ "sandboxes": [ { "name": "web-app", "agent": "claude", "status": "running", "workspace": "/Users/me/web" }, { "name": "chat-1", "agent": "claude", "status": "stopped", "chat": true } ],
                "usage": { "five_hour_pct": 42.4, "seven_day_pct": 81.0, "updated_ms": 1 } })
    }

    fn ask(name: &'static str, args: J) -> Asked {
        Asked { name, args: args.as_object().unwrap().clone() }
    }

    #[test]
    fn removing_is_the_core_dialog_with_the_lists_words() {
        let d = data();
        let cx = Ctx { data: &d };
        let a = act(&cx, &ask("sandbox_remove", json!({ "sandbox": "web-app" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
        assert!(c.danger && c.lines.iter().any(|l| l.ends_with("/Users/me/web")));
        assert_eq!(c.call.path, "/api/sandboxes/web-app/rm");
        assert!(act(&cx, &ask("sandbox_remove", json!({ "sandbox": "ghost" }))).is_err());
        assert!(matches!(act(&cx, &ask("sandbox_recreate", json!({ "sandbox": "chat-1" }))).unwrap().as_slice(), [Action::Confirm(_)]));
        assert!(act(&cx, &ask("sandbox_duplicate", json!({ "sandbox": "web-app", "new_name": "Bad Name" }))).is_err());
        for s in INTENTS {
            assert_eq!(matches!(s.risk, Risk::Approval(_)), matches!(s.name, "sandbox_remove" | "sandbox_recreate" | "screen_restore"), "{}", s.name);
        }
    }

    #[test]
    fn usage_shows_in_the_top_bar_and_turns_amber_high() {
        let d = data();
        let x = extras(&Ctx { data: &d });
        let u = x.iter().find(|e| e.id == "usage").unwrap();
        assert_eq!((u.label.as_str(), u.tone.as_str()), ("5h 42% · 7d 81%", "amber"));
        let d = json!({ "sandboxes": [], "usage": { "updated_ms": 0 } });
        assert!(extras(&Ctx { data: &d }).iter().all(|e| e.id != "usage"));
    }
}
