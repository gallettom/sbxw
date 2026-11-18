//! Kits: the `kits` line of each sbxw.toml sbxw uses, and the kits that wait
//! for the person's answer on a running sandbox, as the web UI's Kits tab
//! (`assets/js/kits.js`). It was Setup's Kits tab; it is now a docked panel
//! of its own (`screens/kits.luau`), opened from the rail (its badge counts
//! the kits waiting), the palette, a sandbox's Manage dialog and a pane's
//! "⋯" menu. The intent and its dialog are the tab's, unchanged.
//!
//! **Which file.** Opened for a sandbox (`kits_show {sandbox}`), the panel is
//! on the sbxw.toml that sandbox uses (`kits_view`, view state dropped when
//! the panel closes); from the rail, on the last file or sandbox used (kept
//! in the app's state file); else on the daemon's own file.
//!
//! A kit waiting (`kits_waiting` on a sandbox) is answered in sbx's own
//! prompt, in a pane (`open_terminal`, mode `kit-add`): never here.

use crate::feat::setup::{list, str_of, strings};
use crate::feat::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, ScreenDecl, Slot};
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};

/// The panel's icon, used everywhere it appears.
pub const ICON: &str = "box";

pub const FEATURE: Feature = Feature {
    name: "kits",
    data: &[Data::new("kits", "/api/kits")],
    screens: &[ScreenDecl { name: "kits", src: include_str!("../../screens/kits.luau"), slot: Slot::Panel { title: "Kits", icon: ICON, width: 460.0 } }],
    intents: INTENTS,
    act,
    extras: Some(extras),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec {
        name: "kits_show",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, false)],
        doc: "Opens the Kits panel and reads the kits lines again; on the sbxw.toml that sandbox uses when given, else on the last one used. View state only.",
    },
    Spec { name: "kits_pick", risk: Risk::Direct, args: &[("path", Arg::Str, true)], doc: "Puts the Kits panel on one of the sbxw.toml files sbxw uses. View state only: nothing is read or sent." },
    Spec {
        name: "kits_save",
        risk: Risk::Approval("Change a kits line"),
        args: &[("path", Arg::Str, true), ("kits", Arg::Str, false), ("defaults", Arg::Bool, false)],
        doc: "Writes the `kits` line of one of the sbxw.toml files sbxw uses: `kits` one reference per line, or `defaults = true` to remove the line.",
    },
];

/// `kits = [...]` as the file will read; `None` is no line.
fn kits_line(list: Option<&[String]>) -> String {
    match list {
        None => "(no kits line: the default kits apply)".into(),
        Some(l) => format!("kits = [{}]", l.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", ")),
    }
}

/// What a kit reference is, for the dialog.
fn kit_kind(r: &str) -> &'static str {
    if r.starts_with("sbxw:") {
        "bundled"
    } else if r.contains("/kits/jarvis-") {
        "Jarvis kit"
    } else if r.starts_with(['.', '/', '~']) {
        "local kit"
    } else {
        "registry kit"
    }
}

/// Bundled kits that reach outside the sandbox (Jarvis's rule for a kits line,
/// `crates/core/src/jarvis.rs`): the person's SSH agent, git identity, ports, the Mac.
const BUNDLED_REACH: &[&str] = &["sbxw:ssh-agent", "sbxw:git-identity", "sbxw:dev-ports", "sbxw:terminal-mac"];

/// How many kits wait for the person's answer, over every sandbox.
pub fn waiting(ctx: &Ctx) -> u64 {
    ctx.get("sandboxes").as_array().map(|a| a.iter().map(|s| s["kits_waiting"].as_u64().unwrap_or(0)).sum()).unwrap_or(0)
}

/// The sbxw.toml a sandbox uses, from the daemon's list.
pub fn file_of(ctx: &Ctx, sandbox: &str) -> Option<String> {
    list(ctx.get("kits"), "files").iter().find(|f| f["sandboxes"].as_array().is_some_and(|l| l.iter().any(|n| n == sandbox))).and_then(|f| f["path"].as_str().map(str::to_string))
}

pub fn act(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    match q.name {
        "kits_show" => {
            let mut v = match q.s("sandbox") {
                Some(sb) => {
                    let path = file_of(ctx, sb);
                    let mut v = vec![Action::Set { key: "kits_view".into(), value: json!({ "sandbox": sb, "path": path }) }, Action::Remember { key: "sandbox".into(), value: json!(sb) }];
                    if let Some(p) = path {
                        v.push(Action::Remember { key: "path".into(), value: json!(p) });
                    }
                    v
                }
                None => vec![Action::Set { key: "kits_view".into(), value: J::Null }],
            };
            v.push(Action::Refresh("kits"));
            v.push(Action::OpenPanel("kits".into()));
            Ok(v)
        }
        "kits_pick" => {
            let path = q.req("path");
            if !list(ctx.get("kits"), "files").iter().any(|f| f["path"] == path) {
                return Err(format!("{path} is not an sbxw.toml sbxw uses"));
            }
            Ok(vec![Action::Set { key: "kits_view".into(), value: json!({ "path": path }) }, Action::Remember { key: "path".into(), value: json!(path) }])
        }
        "kits_save" => kits_save(ctx, q),
        n => Err(format!("{n}: not a kits intent")),
    }
}

fn kits_save(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let path = q.req("path");
    let panel = ctx.get("kits");
    let file = list(panel, "files").iter().find(|f| f["path"] == path).ok_or_else(|| format!("{path} is not an sbxw.toml sbxw uses"))?;
    if let Some(e) = str_of(file, "error") {
        return Err(format!("{path} cannot be edited: {e}"));
    }
    let old = strings(&file["kits"]);
    let new: Option<Vec<String>> = if q.b("defaults") == Some(true) {
        None
    } else {
        let mut seen: Vec<String> = Vec::new();
        for k in q.s("kits").unwrap_or("").lines().map(str::trim).filter(|k| !k.is_empty()) {
            if !seen.iter().any(|x| x == k) {
                seen.push(k.to_string());
            }
        }
        Some(seen)
    };
    if new == old {
        return Err("nothing to save: the line is unchanged".into());
    }
    if new.as_deref().is_some_and(|l| l.iter().any(|k| k == "sbxw:jarvis")) {
        return Err("sbxw:jarvis is the kit of Jarvis's own sandbox: a project's kits cannot list it".into());
    }
    let defaults = strings(&panel["defaults"]).unwrap_or_default();
    let before = old.clone().unwrap_or_else(|| defaults.clone());
    let after = new.clone().unwrap_or_else(|| defaults.clone());
    let added: Vec<&String> = after.iter().filter(|k| !before.contains(k)).collect();
    let removed: Vec<&String> = before.iter().filter(|k| !after.contains(k)).collect();

    let short = path.rsplit('/').next().unwrap_or(path);
    let mut c = Confirm::new(
        if new.is_none() { format!("Use the default kits in {short}") } else { format!("Change the kits of {short}") },
        Call::post("/api/kits", json!({ "path": path, "kits": new }), format!("save the kits of {short}")).then(&["kits"]),
    )
    .line(path.to_string());
    for k in &added {
        c = c.line(format!("Adds {k} ({}).", kit_kind(k)));
    }
    for k in &removed {
        c = c.line(format!("Removes {k}."));
    }
    let users: Vec<String> = file["daemon"].as_bool().filter(|d| *d).map(|_| "new sandboxes from this app".to_string()).into_iter().chain(strings(&file["sandboxes"]).unwrap_or_default()).collect();
    if !users.is_empty() {
        c = c.line(format!("Used by {}; a running sandbox gets the change at its next up.", users.join(", ")));
    }
    let reach: Vec<&&String> = added.iter().filter(|k| !k.starts_with("sbxw:") || BUNDLED_REACH.contains(&k.as_str())).collect();
    if !reach.is_empty() {
        c = c.line("A kit can bring scripts that run in the sandbox, network rules, and access to this machine (an SSH agent, ports, credentials). Read what it declares before saving.").ack(&format!(
            "I know what {} brings",
            reach.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let preview = format!("- {}\n+ {}", kits_line(old.as_deref()), kits_line(new.as_deref()));
    Ok(vec![c.preview(preview, true).labels("Save", "Cancel").into_action()])
}

/// Its rail entry: ONE declaration (a top bar extra the rail draws in its
/// configure zone; the palette lists it from the same extra). Amber with the
/// count while kits wait for the person.
pub fn rail_entry(ctx: &Ctx) -> Extra {
    let n = waiting(ctx);
    Extra {
        place: Place::Rail { zone: crate::feat::Zone::Configure, order: 30 },
        id: "kits".into(),
        label: "Kits".into(),
        icon: Some(ICON.into()),
        tone: if n > 0 { "amber" } else { "neutral" }.into(),
        help: if n > 0 {
            format!("{n} kit{} waiting for your answer on a running sandbox", if n == 1 { "" } else { "s" })
        } else {
            "What each project's sandboxes start with: the kits line of its sbxw.toml".into()
        },
        count: (n > 0).then_some(n),
        // The rail marks it on while the panel shows, and a second press
        // closes it; the panel then opens on the last sandbox used.
        intent: Some(("open_panel".into(), json!({ "screen": "kits" }))),
    }
}

fn extras(ctx: &Ctx) -> Vec<Extra> {
    let mut out = vec![rail_entry(ctx)];
    for sb in ctx.get("sandboxes").as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(name) = sb["name"].as_str() else { continue };
        let n = sb["kits_waiting"].as_u64().unwrap_or(0);
        if n > 0 {
            out.push(Extra {
                place: Place::Sandbox(name.to_string()),
                id: "kit-add".into(),
                label: String::new(),
                icon: Some("check".into()),
                tone: "amber".into(),
                help: format!("{n} kit{} waiting for your approval: answer sbx's prompt in a pane", if n == 1 { "" } else { "s" }),
                count: Some(n),
                intent: Some(("open_terminal".into(), json!({ "sandbox": name, "mode": "kit-add", "pane": "spare" }))),
            });
        }
        if sb["status"] == "running" {
            out.push(Extra {
                place: Place::Pane(name.to_string()),
                id: "kits-view".into(),
                label: "Kits…".into(),
                icon: Some(ICON.into()),
                tone: "muted".into(),
                help: format!("The Kits panel, on the sbxw.toml {name} uses"),
                count: None,
                intent: Some(("kits_show".into(), json!({ "sandbox": name }))),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests;
