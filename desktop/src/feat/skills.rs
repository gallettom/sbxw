//! Skills: the skills the person keeps for every new sandbox, and keeping one
//! a sandbox wrote, as the web UI's Skills dialog (`assets/js/skills.js`). It
//! was Setup's Skills tab; it is now a docked panel of its own
//! (`screens/skills.luau`), opened from the rail, the palette, a sandbox's
//! Manage dialog and a pane's "⋯" menu. The intents and their dialogs are
//! the tab's, unchanged.
//!
//! **Which sandbox.** "Keep a skill from a sandbox" looks at one sandbox.
//! Opened for a sandbox (`skills_show {sandbox}`) the picker is on it
//! (`skills_view`, view state dropped when the panel closes); from the rail,
//! on the last one used. Looking stays the person's press: it starts a
//! stopped sandbox.
//!
//! A kept skill reaches new sandboxes as the `sbxw-my-skills` kit (the code
//! map's generated-kits.md); keeping one is always the core's dialog.

use crate::feat::setup::{list, str_of};
use crate::feat::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, ScreenDecl, Slot};
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};

/// The panel's icon, used everywhere it appears.
pub const ICON: &str = "book";

pub const FEATURE: Feature = Feature {
    name: "skills",
    data: &[Data::new("my_skills", "/api/my-skills")],
    screens: &[ScreenDecl { name: "skills", src: include_str!("../../screens/skills.luau"), slot: Slot::Panel { title: "Skills", icon: ICON, width: 460.0 } }],
    intents: INTENTS,
    act,
    extras: Some(extras),
    ..Feature::EMPTY
};

const fn d(name: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Direct, args, doc }
}
const fn a(name: &'static str, asks: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Approval(asks), args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("skills_show", &[("sandbox", Arg::Str, false)], "Opens the Skills panel and reads the kept skills again; its picker on that sandbox when given, else on the last one used. View state only (looking stays a press)."),
    d("skills_pick", &[("sandbox", Arg::Str, true)], "Puts the Skills panel's picker on that sandbox. View state only: nothing is read or sent."),
    d("skills_look", &[("sandbox", Arg::Str, true)], "Lists the skills a sandbox has that could be kept (starts a stopped sandbox)."),
    a("skill_keep", "Keep a skill for every new sandbox", &[("sandbox", Arg::Str, true), ("name", Arg::Str, true)], "Keeps one of the skills `skills_look` found: every sandbox created from now on gets it."),
    a("skill_forget", "Stop keeping a skill", &[("name", Arg::Str, true)], "Stops giving a kept skill to new sandboxes (existing ones keep their copy)."),
];

fn confirm(c: Confirm) -> Result<Vec<Action>, String> {
    Ok(vec![c.into_action()])
}

fn view_on(sb: &str) -> Vec<Action> {
    vec![Action::Set { key: "skills_view".into(), value: json!({ "sandbox": sb }) }, Action::Remember { key: "sandbox".into(), value: json!(sb) }]
}

pub fn act(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    match q.name {
        "skills_show" => {
            let mut v = match q.s("sandbox") {
                Some(sb) => view_on(sb),
                None => vec![Action::Set { key: "skills_view".into(), value: J::Null }],
            };
            v.push(Action::Refresh("my_skills"));
            v.push(Action::OpenPanel("skills".into()));
            Ok(v)
        }
        "skills_pick" => {
            let sb = q.req("sandbox");
            if ctx.sandbox(sb).is_none() {
                return Err(format!("no sandbox named {sb}"));
            }
            Ok(view_on(sb))
        }
        "skills_look" => {
            let sb = q.req("sandbox");
            if ctx.sandbox(sb).is_none() {
                return Err(format!("no sandbox named {sb}"));
            }
            Ok(vec![
                Action::Set { key: "skills_for".into(), value: json!(sb) },
                Action::Set { key: "sandbox_skills".into(), value: J::Null },
                Action::Fetch { key: "sandbox_skills".into(), path: format!("/api/sandboxes/{}/skills", crate::client::enc(sb)) },
            ])
        }
        "skill_keep" => {
            let (sb, name) = (q.req("sandbox"), q.req("name"));
            if ctx.get("skills_for") != sb {
                return Err(format!("look at {sb}'s skills first"));
            }
            let s = list(ctx.get("sandbox_skills"), "skills").iter().find(|s| s["name"] == name).ok_or_else(|| format!("{sb} has no skill named {name}"))?;
            if s["keepable"] != true {
                return Err(format!("/{name} cannot be kept: lowercase letters, digits, - and _ only"));
            }
            let replace = list(ctx.get("my_skills"), "skills").iter().any(|k| k["name"] == name);
            let origin = s["origin"].clone();
            let mut c = Confirm::new(
                if replace { format!("Replace your kept /{name}") } else { format!("Keep /{name} for every new sandbox") },
                Call::post(format!("/api/sandboxes/{}/skills/keep", crate::client::enc(sb)), json!({ "origin": origin, "name": name, "replace": replace }), format!("keep /{name}")).then(&["my_skills"]),
            )
            .line(format!("From {sb}: {}", s["path"].as_str().unwrap_or("?")))
            .line(format!("{} files, {} bytes.", s["files"].as_u64().unwrap_or(0), s["bytes"].as_u64().unwrap_or(0)))
            .line(if replace { "Sandboxes created from now on get this version instead of the one you kept." } else { "Every sandbox created from now on gets it. Read its SKILL.md first: a skill is instructions the agent follows." })
            .labels(if replace { "Replace" } else { "Keep" }, "Cancel");
            if let Some(desc) = str_of(s, "description") {
                c = c.preview(desc, false);
            }
            confirm(c)
        }
        "skill_forget" => {
            let name = q.req("name");
            if !list(ctx.get("my_skills"), "skills").iter().any(|k| k["name"] == name) {
                return Err(format!("no kept skill named {name}"));
            }
            confirm(
                Confirm::new(format!("Stop keeping /{name}"), Call::post("/api/my-skills/rm", json!({ "name": name }), format!("forget /{name}")).then(&["my_skills"]))
                    .line("Sandboxes created from now on will not have it; existing ones keep their copy.")
                    .labels("Remove", "Cancel")
                    .danger(),
            )
        }
        n => Err(format!("{n}: not a skills intent")),
    }
}

/// Its rail entry: ONE declaration (a top bar extra the rail draws in its
/// configure zone; the palette lists it from the same extra).
pub fn rail_entry(ctx: &Ctx) -> Extra {
    let n = list(ctx.get("my_skills"), "skills").len();
    Extra {
        place: Place::Rail { zone: crate::feat::Zone::Configure, order: 20 },
        id: "skills".into(),
        label: "Skills".into(),
        icon: Some(ICON.into()),
        tone: "neutral".into(),
        help: format!("Kept for every new sandbox ({n} kept)"),
        count: None,
        // The rail marks it on while the panel shows, and a second press
        // closes it; the panel then opens on the last sandbox used.
        intent: Some(("open_panel".into(), json!({ "screen": "skills" }))),
    }
}

fn extras(ctx: &Ctx) -> Vec<Extra> {
    let mut v = vec![rail_entry(ctx)];
    for sb in ctx.get("sandboxes").as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(name) = sb["name"].as_str() else { continue };
        if sb["status"] != "running" {
            continue;
        }
        v.push(Extra {
            place: Place::Pane(name.to_string()),
            id: "skills-view".into(),
            label: "Skills…".into(),
            icon: Some(ICON.into()),
            tone: "muted".into(),
            help: format!("The Skills panel, on {name}: keep a skill it wrote"),
            count: None,
            intent: Some(("skills_show".into(), json!({ "sandbox": name }))),
        });
    }
    v
}

#[cfg(test)]
mod tests;
