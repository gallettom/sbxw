//! Monitor: every Claude session of every sandbox in one view, who needs
//! the person first, the selected one as a short feed (the last prompt,
//! what it does now, its last reply, the question it waits on), and a
//! composer to answer it. An overlay over the pane area, opened from the
//! rail's activity zone (MONITOR.md in the artifacts has the design).
//!
//! * **Data.** What the daemon serves today: the hook reports of
//!   `GET /api/sessions` (`data.sessions`, the threads feature's key) to
//!   start from when the view opens, then each `session` event, which
//!   changes one card ([`model::Model::upsert`]). No transcript, no history:
//!   a working session shows one "Now" line.
//! * **Answering.** An option press is the relay's `agent_pick` (the
//!   Requests panel's path, the same picks), only for a session the daemon
//!   can answer. Typed words are `monitor_send`, of risk Approval like
//!   `follow_send`: masked, then the core's dialog shows them in full with
//!   the session they go to, and only Send there types them in
//!   (`POST /api/chat/push`).
//! * **The gate.** Nothing is sent while anything sits over the panes and
//!   the overlay (`data.app.covered`: the core's dialog, a modal, Jarvis's
//!   proposal, the palette) or while a Jarvis proposal waits: checked here,
//!   whatever the screen draws.
//! * **Agent text is data.** Prompts, replies, questions and options are
//!   drawn as text; an option's act is its index, never its words.

pub mod model;
#[cfg(test)]
mod tests;

use super::{Action, Asked, Call, Confirm, Ctx, Extra, Feature, Place, Prepare, ScreenDecl, Shortcut, Slot, Zone};
use crate::intent::{Arg, Risk, Spec};
use model::{Group, Model};
use serde_json::{json, Map, Value as J};

pub const SCREEN: &str = "monitor";
/// `data.monitor`: what the screen draws.
pub const KEY: &str = "monitor";

const KEYS: &[&str] = &["up", "down", "enter", "type"];

pub const FEATURE: Feature = Feature {
    name: "monitor",
    events: &["session"],
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../../screens/monitor.luau"), slot: Slot::Base { title: "Monitor", icon: "inbox", open: "monitor_open" } }],
    shortcuts: &[Shortcut { key: 'i', intent: "monitor_toggle", args: "{}", help: "Switch between the Terminals and the Monitor" }],
    intents: INTENTS,
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    extras: Some(extras),
    ..Feature::EMPTY
};

const fn d(name: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Direct, args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("monitor_open", &[], "Makes the Monitor the base view of the pane area (instead of the terminals): every session, who needs you first, read again from the daemon's list. An overlay over it closes."),
    d("monitor_toggle", &[], "Switches the pane area between the Terminals and the Monitor."),
    d("monitor_select", &[("key", Arg::Str, true)], "Shows that session (a row's `key`) in the middle. View state only."),
    d("monitor_key", &[("key", Arg::OneOf(KEYS), true), ("text", Arg::Text, false)], "The view's keys while none of its rows has the keyboard: Up and Down move the selection, Enter puts the keyboard on the list, N goes to the next session that needs you (rows: Up and Down move along them, Enter shows one)."),
    d("monitor_next", &[], "Selects the next session that needs you (round)."),
    d("monitor_more", &[], "Shows the next sessions of a long list."),
    d("monitor_terminal", &[("key", Arg::Str, true)], "Opens that session's terminal (the pane that shows it, else a spare one); the view closes."),
    d("monitor_commits", &[("key", Arg::Str, true)], "The Commits view on that session's sandbox, on its thread's branch."),
    d("monitor_files", &[("key", Arg::Str, true)], "The file explorer on that session's sandbox, at its thread's worktree."),
    d("monitor_worktrees", &[("key", Arg::Str, true)], "The Worktrees view, filtered to that session's thread (or sandbox)."),
    d(
        "monitor_pick",
        &[("key", Arg::Str, true), ("step", Arg::Int(1, 16), true), ("option", Arg::Int(1, 32), true)],
        "Picks option `option` of question `step` the session waits on: the relay's `agent_pick` (the Requests panel's path). Refused while anything covers the view or a Jarvis proposal waits.",
    ),
    Spec {
        name: "monitor_send",
        risk: Risk::Approval("Send words to a sandbox's agent"),
        args: &[("key", Arg::Str, true), ("text", Arg::Str, true)],
        doc: "Words for that session's agent: masked, then shown in full in the core's dialog with the session they go to; Send there types them into it. Refused while the session waits on a menu, while anything covers the view or a Jarvis proposal waits.",
    },
];

crate::feat::relay::per_thread!(STATE, Model);

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `data.monitor` as the model stands.
pub fn view_action(m: &Model, cx: &Ctx) -> Action {
    Action::Set { key: KEY.into(), value: m.view(model::now_for(m, wall_ms()), &cx.data["app"]["panes"]) }
}

/// Drawn, or the base view under an overlay: its view is kept current
/// (a `Set` renders only screens that are drawn).
fn shown(cx: &Ctx) -> bool {
    cx.data["app"]["overlay"]["screen"] == SCREEN || cx.data["app"]["base"] == SCREEN
}

/// Why nothing may be sent now, or `None`: something over the panes and the
/// overlay (`data.app.covered`), or a Jarvis proposal waiting for the person.
pub fn held(cx: &Ctx) -> Option<String> {
    if cx.data["app"]["covered"] != false {
        return Some("Not sent: something is open over the monitor (a dialog, or a view such as Files). Close it first.".into());
    }
    if cx.data["jarvis"]["pending"].as_array().is_some_and(|p| !p.is_empty()) {
        return Some("Not sent: a Jarvis proposal waits for you. Approve or refuse it first.".into());
    }
    None
}

fn on_event(cx: &Ctx, name: &str, info: &J) -> Vec<Action> {
    if name != "session" {
        return vec![];
    }
    let mut m = STATE().lock().unwrap();
    // Only what changed is built again; the view is handed over only while it shows.
    if m.upsert(info) && shown(cx) {
        return vec![view_action(&m, cx)];
    }
    vec![]
}

fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let ok = matches!(&r, Ok((c, b)) if (200..300).contains(c) && b["ok"] != false);
    let why = match &r {
        Ok((c, b)) => b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}")),
        Err(e) => e.clone(),
    };
    let mut m = STATE().lock().unwrap();
    if let Some(key) = call.tag.strip_prefix("send:") {
        if ok {
            // The draft has left: the screen empties it.
            *m.sent.entry(key.to_string()).or_insert(0) += 1;
            return vec![Action::notice(format!("{}: sent", call.what)), view_action(&m, cx)];
        }
        return vec![Action::error(format!("{}: {why}. Your words are still in the box.", call.what))];
    }
    vec![if ok { Action::notice(format!("{}: done", call.what)) } else { Action::error(format!("{}: {why}", call.what)) }]
}

fn extras(cx: &Ctx) -> Vec<Extra> {
    let (mut you, mut working, mut all) = (0, 0, 0);
    for i in cx.get("sessions").as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if i["sandbox"] == model::JARVIS {
            continue;
        }
        all += 1;
        match Group::of(i["state"].as_str().unwrap_or("")) {
            Group::You => you += 1,
            Group::Working => working += 1,
            _ => {}
        }
    }
    let help = match (all, you, working) {
        (0, _, _) => "Every session in one view. None is running".to_string(),
        (_, 0, 0) => format!("Every session in one view. {all} idle, none needs you"),
        _ => format!("Every session in one view. {you} need{} you, {working} working", if you == 1 { "s" } else { "" }),
    };
    // No count badge: the Requests entry already counts what waits.
    vec![Extra { place: Place::Rail { zone: Zone::Activity, order: 5 }, id: SCREEN.into(), label: "Monitor".into(), icon: Some("inbox".into()), tone: "blue".into(), help, count: None, intent: Some(("monitor_open".into(), json!({}))) }]
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let mut m = STATE().lock().unwrap();
    match a.name {
        "monitor_open" => {
            m.seed(cx.get("sessions"));
            // The sidebar's sandbox, its most urgent session, when nothing was picked yet.
            if m.sel.is_none() {
                if let Some(sb) = cx.data["app"]["selected"].as_str() {
                    m.sel = m.first_of(sb);
                }
            }
            let mut out = vec![view_action(&m, cx), Action::OpenPanel(SCREEN.into())];
            // The keyboard to the list, on the session shown: Up, Down and Enter from there.
            if let Some(k) = m.selected() {
                out.push(Action::Focus(format!("monitor-row-{k}")));
            }
            Ok(out)
        }
        // The Monitor is the base: back to the terminals; else it becomes the base.
        "monitor_toggle" if cx.data["app"]["base"] == SCREEN => Ok(vec![Action::ClosePanel(SCREEN.into())]),
        "monitor_toggle" => {
            drop(m);
            act(cx, &Asked { name: "monitor_open", args: Map::new() })
        }
        "monitor_select" => {
            let k = a.req("key");
            if !m.cards.contains_key(k) {
                return Err(format!("{k} is not a session the monitor lists"));
            }
            m.sel = Some(k.to_string());
            Ok(vec![view_action(&m, cx)])
        }
        "monitor_more" => {
            m.pages = m.pages.max(1) + 1;
            Ok(vec![view_action(&m, cx)])
        }
        "monitor_next" => {
            if !m.next_needing() {
                return Ok(vec![Action::notice("No session needs you")]);
            }
            Ok(vec![view_action(&m, cx)])
        }
        "monitor_key" => match a.req("key") {
            "up" | "down" => {
                m.step(if a.req("key") == "up" { -1 } else { 1 });
                // The keyboard follows the selection (its row's focus ring).
                let focus = m.selected().map(|k| Action::Focus(format!("monitor-row-{k}")));
                Ok([Some(view_action(&m, cx)), focus].into_iter().flatten().collect())
            }
            // Nothing of the view has the keyboard: Enter gives it to the list, on the session shown.
            "enter" => Ok(m.selected().map(|k| Action::Focus(format!("monitor-row-{k}"))).into_iter().collect()),
            _ => match a.s("text") {
                Some("n") | Some("N") => {
                    if !m.next_needing() {
                        return Ok(vec![Action::notice("No session needs you")]);
                    }
                    let focus = m.selected().map(|k| Action::Focus(format!("monitor-row-{k}")));
                    Ok([Some(view_action(&m, cx)), focus].into_iter().flatten().collect())
                }
                _ => Ok(vec![]),
            },
        },
        "monitor_terminal" | "monitor_commits" | "monitor_files" | "monitor_worktrees" => links(&m, cx, a.name, a.req("key")),
        "monitor_pick" => {
            if let Some(why) = held(cx) {
                return Err(why);
            }
            let k = a.req("key");
            let c = m.cards.get(k).ok_or_else(|| format!("{k} is not a session the monitor lists"))?;
            let (sb, thread) = (c.info["sandbox"].as_str().unwrap_or(""), model::thread_of(&c.info));
            // The daemon's list as it stands, not the card: still waiting, still answerable.
            let live = cx.get("sessions").as_array().and_then(|l| l.iter().find(|i| i["sandbox"] == sb && model::thread_of(i) == thread && i["state"] == "attention").cloned());
            let live = live.ok_or_else(|| format!("{} is not waiting on a question any more", model::who(sb, thread)))?;
            if live["answerable"] != true {
                return Err(format!("{} can only be answered in its terminal", model::who(sb, thread)));
            }
            let mut args = json!({ "sandbox": sb, "step": a.i("step").unwrap_or(1), "option": a.i("option").unwrap_or(1) });
            if let Some(t) = thread {
                args["thread"] = t.into();
            }
            // The Requests panel's own path: the same picks, the same checks, `/api/answer`.
            Ok(vec![Action::Intent { name: "agent_pick".into(), args }])
        }
        "monitor_send" => send(&m, cx, a),
        other => Err(format!("{other}: not a monitor intent")),
    }
}

/// Why that card cannot take typed words now, or its target.
fn reachable(m: &Model, cx: &Ctx, key: &str) -> Result<(String, Option<String>), String> {
    let c = m.cards.get(key).ok_or_else(|| format!("Not sent: {key} is not a session the monitor lists any more."))?;
    let sb = c.info["sandbox"].as_str().unwrap_or("").to_string();
    let thread = model::thread_of(&c.info).map(str::to_string);
    let to = model::who(&sb, thread.as_deref());
    match Group::of(c.info["state"].as_str().unwrap_or("")) {
        Group::You => return Err(format!("Not sent: {to} waits on a question or a permission; answer that first.")),
        Group::Ended => return Err(format!("Not sent: {to} has ended.")),
        _ => {}
    }
    if thread.is_none() && c.info["answerable"] != true {
        return Err(format!("Not sent: sbxw cannot tell which terminal is {to}'s. Answer in its terminal."));
    }
    if cx.sandbox(&sb).is_some_and(|x| x["status"] != "running") {
        return Err(format!("Not sent: {sb} is not running."));
    }
    Ok((sb, thread))
}

/// Typed words: checked, masked, then the core's dialog.
fn send(m: &Model, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    if let Some(why) = held(cx) {
        return Err(why);
    }
    let key = a.req("key");
    let text = a.req("text").trim();
    if text.is_empty() {
        return Err("Nothing to send".into());
    }
    let (sb, thread) = reachable(m, cx, key)?;
    let mut asked = Asked { name: "monitor_send", args: Map::new() };
    asked.args.insert("key".into(), key.into());
    asked.args.insert("text".into(), text.into());
    Ok(vec![Action::Prepare(Box::new(Prepare {
        title: format!("Masking personal data before it goes to {}", model::who(&sb, thread.as_deref())),
        call: Call::post("/api/voice/anonymise", json!({ "text": text }), "mask personal data").quiet().timeout(30),
        asked,
        build,
    }))])
}

fn build(cx: &Ctx, asked: &Asked, masked: &J) -> Result<Confirm, String> {
    if masked["ok"] == false {
        return Err(format!("Not sent: the masking of personal data failed ({}).", masked["error"].as_str().unwrap_or("no reason given")));
    }
    let safe = masked["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("Not sent: the masking of personal data gave nothing back.")?;
    let key = asked.req("key");
    // The session as it is now, not as it was when the person pressed Send.
    let m = STATE().lock().unwrap();
    let (sb, thread) = reachable(&m, cx, key)?;
    let to = model::who(&sb, thread.as_deref());
    let mut body = json!({ "text": safe, "name": sb });
    if let Some(t) = &thread {
        body["thread"] = t.clone().into();
    }
    let n = masked["masked"].as_u64().unwrap_or(0);
    let chars = safe.chars().count();
    let mut c = Confirm::new(format!("Send this to {to}?"), Call::post("/api/chat/push", body, format!("send your words to {to}")).tag(&format!("send:{key}")))
        .line(format!("To: {to}'s agent, typed into its session as one message."))
        .line(format!("{chars} character{}, shown in full below.", if chars == 1 { "" } else { "s" }));
    if n > 0 {
        c = c.line(format!("{n} piece{} of personal data masked before sending.", if n == 1 { "" } else { "s" }));
    }
    Ok(c.preview(safe.to_string(), false).labels(&format!("Send to {sb}"), "Cancel"))
}

/// The links of a card: its terminal, its commits, files and worktree, each
/// through the intent that owns it.
fn links(m: &Model, cx: &Ctx, name: &str, key: &str) -> Result<Vec<Action>, String> {
    let c = m.cards.get(key).ok_or_else(|| format!("{key} is not a session the monitor lists"))?;
    let sb = c.info["sandbox"].as_str().unwrap_or("").to_string();
    let thread = model::thread_of(&c.info).map(str::to_string);
    let worktree = c.info["cwd"].as_str().is_some_and(|p| p.contains(".sbxw-worktrees/"));
    let intent = |n: &str, args: J| Action::Intent { name: n.into(), args };
    Ok(match name {
        "monitor_terminal" => {
            let mut out = vec![Action::ClosePanel(SCREEN.into())];
            match model::pane_of(&cx.data["app"]["panes"], &sb, thread.as_deref()) {
                // Already in a pane: that pane's page, and the keyboard to it.
                Some(p) => {
                    let page = cx.data["app"]["pages"]["panes"].as_array().and_then(|l| l.iter().position(|pg| pg.as_array().is_some_and(|v| v.iter().any(|x| x.as_u64() == Some(p))))).map(|i| i + 1).unwrap_or(1);
                    out.push(intent("page_show", json!({ "page": page })));
                    out.push(Action::Focus(format!("grid-{p}")));
                }
                None => out.push(match &thread {
                    Some(t) => intent("thread_open", json!({ "sandbox": sb, "name": t })),
                    None => intent("open_terminal", json!({ "sandbox": sb, "pane": "spare" })),
                }),
            }
            out
        }
        "monitor_commits" => {
            let mut args = json!({ "sandbox": sb });
            if let (Some(t), true) = (&thread, worktree) {
                args["branch"] = format!("sbxw/{t}").into();
            }
            vec![intent("commits_show", args)]
        }
        "monitor_files" => {
            let mut args = json!({ "sandbox": sb });
            if let (Some(t), true) = (&thread, worktree) {
                args["worktree"] = t.clone().into();
            }
            vec![intent("files_open", args)]
        }
        _ => vec![intent("worktrees_show", json!({})), intent("worktrees_filter", json!({ "text": thread.unwrap_or(sb) }))],
    })
}
