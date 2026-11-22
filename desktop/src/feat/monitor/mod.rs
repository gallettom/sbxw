//! Sessions (the feature `monitor`; the name code, tests and the state
//! file use): every Claude session of every sandbox in one view, who needs
//! the person first, the selected one's conversation as the terminal shows
//! it (its prompts, steps and replies, then the question it waits on), and
//! a composer to answer it. A base view of the pane area, the top bar's
//! GUI (SESSIONS.md and MONITOR.md in the artifacts have the design).
//!
//! * **Data.** The hook reports of `GET /api/sessions` (`data.sessions`,
//!   the threads feature's key) to start from when the view opens, then
//!   each `session` event, which changes one card ([`model::Model::upsert`]).
//!   The selected session's conversation is read from the daemon's history
//!   ([`conv`]): only that session, only while the view is drawn; the
//!   `history` nudge reads its newest page again; another selected or the
//!   view left deletes its subscription.
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
//! * **Agent text is data.** Prompts, replies, steps, questions and options
//!   are drawn as text; an option's act is its index, never its words; a
//!   conversation row's only act is its own fold, by its id.

pub mod attach;
pub mod conv;
pub mod model;
#[cfg(test)]
mod tests;

use super::{core_call, Action, Asked, Call, Confirm, Ctx, Feature, Incoming, Prepare, ScreenDecl, Shortcut, Slot};
use attach::{Att, Job, Payload, Waiting};
use crate::intent::{Arg, Risk, Spec};
use model::{Group, Model};
use serde_json::{json, Map, Value as J};

pub const SCREEN: &str = "monitor";
/// `data.monitor`: what the screen draws.
pub const KEY: &str = "monitor";

/// `escape` only reaches it while `monitor_ui.popup` is true (the core's
/// popup rule); the Monitor sets none yet, so it does nothing.
const KEYS: &[&str] = &["up", "down", "enter", "type", "escape"];

pub const FEATURE: Feature = Feature {
    name: "monitor",
    events: &["session", "history"],
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../../screens/monitor.luau"), slot: Slot::Base { title: "Sessions", icon: "inbox", open: "monitor_open" } }],
    shortcuts: &[Shortcut { key: 'i', intent: "monitor_toggle", args: "{}", help: "Switch between GUI and TUI" }],
    intents: INTENTS,
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    on_files: Some(on_files),
    on_dismiss: Some(on_dismiss),
    standin: Some(standin),
    ..Feature::EMPTY
};

use crate::intent::direct as d;

pub const INTENTS: &[Spec] = &[
    d("monitor_open", &[], "Makes Sessions the base view of the pane area (instead of the terminals): every session, who needs you first, read again from the daemon's list. An overlay over it closes."),
    d("monitor_toggle", &[], "Switches the pane area between the Terminals and Sessions."),
    d("monitor_select", &[("key", Arg::Str, true)], "Shows that session (a row's `key`) in the middle. View state only."),
    d("monitor_refresh", &[], "Draws Sessions again from what the app knows (the core runs it when Lead Mode changes). View state only."),
    d("monitor_expand", &[("key", Arg::Str, true), ("id", Arg::Str, true)], "Unfolds or folds again one row of the shown session's conversation (a group of steps, a step's result, a long message). View state only."),
    d("monitor_earlier", &[("key", Arg::Str, true)], "Shows earlier parts of the shown session's conversation (reads the older page when needed)."),
    d("monitor_retry", &[("key", Arg::Str, true)], "Reads the shown session's conversation again after a failed read."),
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
    d("monitor_attach", &[("key", Arg::Str, true)], "The system's file dialog, to attach a file to what you write to that session (the one shown). The file waits as a chip under the box until Send; files dropped on Sessions or a picture pasted there are attached the same way. Refused while the composer is."),
    d("monitor_attach_remove", &[("key", Arg::Str, true), ("id", Arg::Int(1, 1_000_000_000), true)], "Removes attached file `id` from that session's waiting files."),
    Spec {
        name: "monitor_send",
        risk: Risk::Approval("Send words to a sandbox's agent"),
        args: &[("key", Arg::Str, true), ("text", Arg::Str, false)],
        doc: "Words for that session's agent, with the files attached to it: masked, then ONE core dialog shows the words in full, each file (local path, size), the total and the session; Send there uploads the files one after the other, then types the words followed by the files' paths in the sandbox into the session as one message. A failed upload sends nothing. Refused while the session waits on a menu, while anything covers the view or a Jarvis proposal waits.",
    },
];

crate::feat::relay::per_thread!(STATE, Model);

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `data.monitor` as the model stands.
pub fn view_action(m: &mut Model, cx: &Ctx) -> Action {
    let now = model::now_for(m, wall_ms());
    let mut v = m.view(now, &cx.data["app"]["panes"]);
    // The shown session's conversation, as far as it was read.
    if let Some(k) = v["detail"]["key"].as_str().map(str::to_string) {
        v["detail"]["conv"] = conv_view(m, &k, now);
    }
    // The shown session's attached files, as chips, and whether a send of them runs.
    if let Some(k) = v["detail"]["key"].as_str().map(str::to_string) {
        let sending = m.jobs.values().any(|j| j.key == k && j.sent);
        let files = m.att.get(&k).map(Vec::as_slice).unwrap_or(&[]);
        v["detail"]["files"] = attach::chips(files, &cx.data["app"]["uploads"], sending);
        v["detail"]["sending"] = sending.into();
        v["detail"]["attach_max"] = (files.len() >= attach::MAX).into();
    }
    Action::Set { key: KEY.into(), value: v }
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
        return Some("Not sent: something is open over Sessions (a dialog, or a view such as Files). Close it first.".into());
    }
    if cx.data["jarvis"]["pending"].as_array().is_some_and(|p| !p.is_empty()) {
        return Some("Not sent: a Jarvis proposal waits for you. Approve or refuse it first.".into());
    }
    None
}

fn on_event(cx: &Ctx, name: &str, info: &J) -> Vec<Action> {
    let mut m = STATE().lock().unwrap();
    if name == "history" {
        // A nudge (no text): only the session followed is read, never another.
        let id = (info["sandbox"].as_str().unwrap_or("").to_string(), info["session_id"].as_str().unwrap_or("").to_string());
        if !shown(cx) {
            return leave(&mut m);
        }
        if m.conv.sub.as_ref() == Some(&id) {
            return read_new(&mut m, &id).into_iter().collect();
        }
        return vec![];
    }
    if name != "session" {
        return vec![];
    }
    m.sync_leads(&cx.data["app"]);
    if !shown(cx) {
        m.upsert(info);
        return leave(&mut m);
    }
    // Only what changed is built again; the view is handed over only while it shows.
    let changed = m.upsert(info);
    let mut out = follow(&mut m, cx, true);
    // The session followed reported: its subscription renewed if the last read is old
    // (an idle session's lapses; its next report comes before anything new).
    let id = (info["sandbox"].as_str().unwrap_or("").to_string(), info["session_id"].as_str().unwrap_or("").to_string());
    if m.conv.sub.as_ref() == Some(&id) && out.is_empty() && m.conv.get(&id).is_some_and(|h| wall_ms().saturating_sub(h.read_ms) > conv::RENEW_MS) {
        out.extend(read_new(&mut m, &id));
    }
    if changed {
        out.insert(0, view_action(&mut m, cx));
    }
    out
}

/// The selected card's session: `(sandbox, session_id)`, when its report names one.
fn sid_of(m: &Model) -> Option<conv::Sid> {
    let k = m.selected()?;
    let i = &m.cards.get(&k)?.info;
    Some((i["sandbox"].as_str()?.to_string(), i["session_id"].as_str().filter(|s| !s.is_empty())?.to_string()))
}

fn hist_path(id: &conv::Sid) -> String {
    format!("/api/sessions/{}/{}/history", crate::client::enc(&id.0), crate::client::enc(&id.1))
}

fn tag_of(kind: &str, id: &conv::Sid) -> String {
    format!("hist:{kind}:{}\t{}", id.0, id.1)
}

/// The newest page of that log (a read renews its subscription); a read
/// under way is asked again once it answers.
fn read_new(m: &mut Model, id: &conv::Sid) -> Option<Action> {
    let h = m.conv.touch(id);
    if h.loading {
        h.again = true;
        return None;
    }
    h.loading = true;
    h.read_ms = wall_ms();
    Some(Action::Call(Call::get(format!("{}?limit={}", hist_path(id), conv::PAGE), "read the conversation").tag(&tag_of("new", id))))
}

/// The page before what is read.
fn read_old(m: &mut Model, id: &conv::Sid) -> Option<Action> {
    let h = m.conv.touch(id);
    let before = h.next_before?;
    if h.older_loading {
        return None;
    }
    h.older_loading = true;
    h.read_ms = wall_ms();
    Some(Action::Call(Call::get(format!("{}?before={before}&limit={}", hist_path(id), conv::PAGE), "read the earlier conversation").tag(&tag_of("old", id))))
}

/// The subscription of the session followed, deleted (the only DELETE
/// Sessions sends: its path from the card's sandbox and session id).
fn unsubscribe(id: &conv::Sid) -> Action {
    Action::Call(Call::delete(format!("{}/subscription", hist_path(id)), "stop following the conversation").tag("hist:del"))
}

/// The view is left: no session is followed any more.
fn leave(m: &mut Model) -> Vec<Action> {
    m.conv.sub.take().map(|id| unsubscribe(&id)).into_iter().collect()
}

/// Follows the selected session while the view shows (`shown`): another
/// selected, the last one's subscription is deleted and the new one read.
fn follow(m: &mut Model, cx: &Ctx, shown_now: bool) -> Vec<Action> {
    let _ = cx;
    let target = if shown_now { sid_of(m) } else { None };
    if m.conv.sub == target {
        return vec![];
    }
    let mut out = leave(m);
    if let Some(t) = target {
        let h = m.conv.touch(&t);
        h.window = conv::WINDOW;
        out.extend(read_new(m, &t));
        m.conv.sub = Some(t);
    }
    out
}

/// `detail.conv`: the shown session's conversation as the screen draws it.
fn conv_view(m: &mut Model, key: &str, now: u64) -> J {
    let Some(c) = m.cards.get(key) else { return J::Null };
    let sb = c.info["sandbox"].as_str().unwrap_or("").to_string();
    let Some(sid) = c.info["session_id"].as_str().filter(|s| !s.is_empty()).map(str::to_string) else { return J::Null };
    let continues = c.info["continues"].as_str().is_some_and(|x| !x.is_empty());
    let id = (sb.clone(), sid);
    let Some(h) = m.conv.logs.get_mut(&id) else { return json!({ "state": "loading" }) };
    if !h.loaded {
        return json!({ "state": if h.error.is_some() { "error" } else { "loading" }, "error": h.error });
    }
    let state = if h.partial.is_some() { "partial" } else if h.missing { "missing" } else { "ready" };
    let mut v = h.rows(now / 1000);
    let at_start = h.next_before.is_none() && v["hidden"] == 0;
    v["state"] = state.into();
    v["key"] = key.into();
    v["earlier"] = (v["hidden"].as_u64().unwrap_or(0) > 0 || h.next_before.is_some()).into();
    v["older_loading"] = h.older_loading.into();
    v["truncated"] = (h.truncated && at_start).into();
    v["continues"] = (continues && at_start).into();
    v["error"] = h.error.clone().map_or(J::Null, J::String);
    let bg = model::is_background(&c.info);
    v["partial"] = h.partial.as_deref().map_or(J::Null, |p| conv::partial_line(p, &sb, bg).into());
    v["rev"] = h.rev.into();
    v
}

fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let ok = matches!(&r, Ok((c, b)) if (200..300).contains(c) && b["ok"] != false);
    let why = match &r {
        Ok((c, b)) => b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}")),
        Err(e) => e.clone(),
    };
    let mut m = STATE().lock().unwrap();
    if call.tag == "hist:del" {
        return vec![];
    }
    if let Some(rest) = call.tag.strip_prefix("hist:") {
        return history_answer(&mut m, cx, rest, r);
    }
    if let Some(n) = call.tag.strip_prefix("attach:").and_then(|n| n.parse::<u64>().ok()) {
        return uploaded(&mut m, cx, n, r);
    }
    if let Some(key) = call.tag.strip_prefix("send:") {
        if ok {
            // The draft has left: the screen empties it, and its files went with it.
            *m.sent.entry(key.to_string()).or_insert(0) += 1;
            let gone = m.att.remove(key).unwrap_or_default();
            let mut out = vec![Action::notice(format!("{}: sent", call.what)), view_action(&mut m, cx)];
            out.extend(forget(&gone));
            return out;
        }
        let kept = if m.att.get(key).is_some_and(|v| !v.is_empty()) { " Your words and files are still in the box; the files already uploaded go with the next Send." } else { " Your words are still in the box." };
        return vec![Action::error(format!("{}: {why}.{kept}", call.what)), view_action(&mut m, cx)];
    }
    vec![if ok { Action::notice(format!("{}: done", call.what)) } else { Action::error(format!("{}: {why}", call.what)) }]
}

/// A page of a conversation answered: merged; a nudge that came meanwhile
/// reads the newest page again. A log evicted meanwhile is not brought back.
fn history_answer(m: &mut Model, cx: &Ctx, rest: &str, r: Result<(u16, J), String>) -> Vec<Action> {
    let Some((kind, id)) = rest.split_once(':') else { return vec![] };
    let Some((sb, sid)) = id.split_once('\t') else { return vec![] };
    let id = (sb.to_string(), sid.to_string());
    let newest = kind == "new";
    let Some(h) = m.conv.logs.get_mut(&id) else { return vec![] };
    if newest {
        h.loading = false;
    } else {
        h.older_loading = false;
    }
    match r {
        Ok((c, b)) if (200..300).contains(&c) && b["page"].is_object() => h.merge(&b["page"], newest),
        Ok((404, _)) if newest => h.none(),
        Ok((c, b)) => h.fail(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"))),
        Err(e) => h.fail(e),
    }
    let again = std::mem::take(&mut h.again) && newest;
    let mut out = vec![];
    if again && m.conv.sub.as_ref() == Some(&id) {
        out.extend(read_new(m, &id));
    }
    if shown(cx) && m.conv.sub.as_ref() == Some(&id) {
        out.insert(0, view_action(m, cx));
    }
    out
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let mut out = act_on(cx, a)?;
    let mut m = STATE().lock().unwrap();
    match a.name {
        // Leaving the view: no session followed.
        "monitor_toggle" if cx.data["app"]["base"] == SCREEN => out.extend(leave(&mut m)),
        "monitor_terminal" => out.extend(leave(&mut m)),
        "monitor_open" | "monitor_toggle" => out.extend(follow(&mut m, cx, true)),
        n if n.starts_with("monitor_") => out.extend(follow(&mut m, cx, shown(cx))),
        _ => {}
    }
    Ok(out)
}

fn act_on(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let mut m = STATE().lock().unwrap();
    // The rail's Team leads (the core's state) filters the list.
    m.sync_leads(&cx.data["app"]);
    match a.name {
        "monitor_refresh" => Ok(if shown(cx) { vec![view_action(&mut m, cx)] } else { vec![] }),
        "monitor_expand" | "monitor_earlier" | "monitor_retry" => {
            let k = a.req("key");
            if m.selected().as_deref() != Some(k) {
                return Err("That session is not the one shown".into());
            }
            let Some(id) = sid_of(&m) else { return Ok(vec![]) };
            let mut out = vec![];
            match a.name {
                "monitor_expand" => m.conv.touch(&id).toggle(a.req("id")),
                "monitor_earlier" => {
                    let h = m.conv.touch(&id);
                    let hidden = h.rows(wall_ms() / 1000)["hidden"].as_u64().unwrap_or(0);
                    h.window += conv::WINDOW;
                    h.rev += 1;
                    // Nothing more drawn here: the page before.
                    if hidden == 0 {
                        out.extend(read_old(&mut m, &id));
                    }
                }
                _ => {
                    if m.conv.sub.as_ref() == Some(&id) {
                        out.extend(read_new(&mut m, &id));
                    }
                }
            }
            out.insert(0, view_action(&mut m, cx));
            Ok(out)
        }
        "monitor_open" => {
            m.seed(cx.get("sessions"));
            // The sidebar's sandbox, its most urgent session, when nothing was picked yet.
            if m.sel.is_none() {
                if let Some(sb) = cx.data["app"]["selected"].as_str() {
                    m.sel = m.first_of(sb);
                }
            }
            let mut out = vec![view_action(&mut m, cx), Action::OpenPanel(SCREEN.into())];
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
            if !m.order().iter().any(|o| o == k) {
                return Err(format!("{k} is not a session Sessions lists"));
            }
            m.sel = Some(k.to_string());
            Ok(vec![view_action(&mut m, cx)])
        }
        "monitor_more" => {
            m.pages = m.pages.max(1) + 1;
            Ok(vec![view_action(&mut m, cx)])
        }
        "monitor_next" => {
            if !m.next_needing() {
                return Ok(vec![Action::notice("No session needs you")]);
            }
            Ok(vec![view_action(&mut m, cx)])
        }
        "monitor_key" => match a.req("key") {
            "up" | "down" => {
                m.step(if a.req("key") == "up" { -1 } else { 1 });
                // The keyboard follows the selection (its row's focus ring).
                let focus = m.selected().map(|k| Action::Focus(format!("monitor-row-{k}")));
                Ok([Some(view_action(&mut m, cx)), focus].into_iter().flatten().collect())
            }
            // Nothing of the view has the keyboard: Enter gives it to the list, on the session shown.
            "enter" => Ok(m.selected().map(|k| Action::Focus(format!("monitor-row-{k}"))).into_iter().collect()),
            _ => match a.s("text") {
                Some("n") | Some("N") => {
                    if !m.next_needing() {
                        return Ok(vec![Action::notice("No session needs you")]);
                    }
                    let focus = m.selected().map(|k| Action::Focus(format!("monitor-row-{k}")));
                    Ok([Some(view_action(&mut m, cx)), focus].into_iter().flatten().collect())
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
            let c = m.cards.get(k).ok_or_else(|| format!("{k} is not a session Sessions lists"))?;
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
        "monitor_send" => send(&mut m, cx, a),
        "monitor_attach" => {
            let k = a.req("key");
            if let Some(why) = held(cx) {
                return Err(why.replacen("Not sent", "Nothing attached", 1));
            }
            reachable(&m, cx, k).map_err(|e| e.replacen("Not sent", "Nothing attached", 1))?;
            if m.selected().as_deref() != Some(k) {
                return Err("Nothing attached: that session is not the one shown".into());
            }
            if m.att.get(k).is_some_and(|v| v.len() >= attach::MAX) {
                return Err(format!("{} files at a time at most: send or remove one first", attach::MAX));
            }
            m.pick_for = Some(k.to_string());
            let c = m.cards.get(k).map(|c| model::who(c.info["sandbox"].as_str().unwrap_or(""), model::thread_of(&c.info))).unwrap_or_default();
            Ok(vec![Action::PickFiles { title: format!("Attach a file for {c}"), extensions: vec![], multiple: false }])
        }
        "monitor_attach_remove" => {
            let (k, id) = (a.req("key"), a.i("id").unwrap_or(0) as u64);
            if m.jobs.values().any(|j| j.key == k && j.sent) {
                return Err("A send of these files is under way: wait for it to end".into());
            }
            let list = m.att.get_mut(k).ok_or("no file is attached there")?;
            let i = list.iter().position(|x| x.id == id).ok_or("that file is not attached any more")?;
            let gone = list.remove(i);
            let mut out = vec![view_action(&mut m, cx)];
            out.extend(forget(std::slice::from_ref(&gone)));
            Ok(out)
        }
        other => Err(format!("{other}: not a Sessions intent")),
    }
}

/// Why that card cannot take typed words now, or its target.
fn reachable(m: &Model, cx: &Ctx, key: &str) -> Result<(String, Option<String>), String> {
    let c = m.cards.get(key).ok_or_else(|| format!("Not sent: {key} is not a session Sessions lists any more."))?;
    let sb = c.info["sandbox"].as_str().unwrap_or("").to_string();
    let thread = model::thread_of(&c.info).map(str::to_string);
    let to = model::who(&sb, thread.as_deref());
    match Group::of(c.info["state"].as_str().unwrap_or("")) {
        Group::You => return Err(format!("Not sent: {to} waits on a question or a permission; answer that first.")),
        Group::Ended => return Err(format!("Not sent: {to} has ended.")),
        _ => {}
    }
    if let Some(why) = model::unreachable_line(&c.info) {
        return Err(format!("Not sent to {to}: {why}."));
    }
    if cx.sandbox(&sb).is_some_and(|x| x["status"] != "running") {
        return Err(format!("Not sent: {sb} is not running."));
    }
    Ok((sb, thread))
}

/// Typed words and the session's attached files: checked, the words
/// masked, then the core's dialog.
fn send(m: &mut Model, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    if let Some(why) = held(cx) {
        return Err(why);
    }
    let key = a.req("key");
    let text = a.s("text").unwrap_or("").trim();
    let files = m.att.get(key).cloned().unwrap_or_default();
    if text.is_empty() && files.is_empty() {
        return Err("Nothing to send".into());
    }
    if m.jobs.values().any(|j| j.key == key && j.sent) {
        return Err("Not sent: the files of your last Send are still uploading.".into());
    }
    // A file that cannot go stops the whole send, before anything is asked.
    if let Some((f, p)) = files.iter().find_map(|f| f.problem().map(|p| (f, p))) {
        return Err(format!("Not sent: {} cannot go: {p}. Remove it first.", attach::clipped(&f.name)));
    }
    let (sb, thread) = reachable(m, cx, key)?;
    let mut asked = Asked { name: "monitor_send", args: Map::new() };
    asked.args.insert("key".into(), key.into());
    asked.args.insert("text".into(), text.into());
    if text.is_empty() {
        // Files alone: nothing to mask, the dialog at once.
        return Ok(vec![Action::Confirm(Box::new(confirm(m, cx, key, "", 0)?))]);
    }
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
    let mut m = STATE().lock().unwrap();
    confirm(&mut m, cx, asked.req("key"), safe, masked["masked"].as_u64().unwrap_or(0))
}

/// The ONE dialog of a send: the words in full (`safe`, already masked),
/// each attached file (local path, size), the total, the session. Without
/// files its Send posts the words; with files it uploads them first.
fn confirm(m: &mut Model, cx: &Ctx, key: &str, safe: &str, n: u64) -> Result<Confirm, String> {
    // The session as it is now, not as it was when the person pressed Send.
    let (sb, thread) = reachable(m, cx, key)?;
    let to = model::who(&sb, thread.as_deref());
    let files = m.att.get(key).cloned().unwrap_or_default();
    if let Some((f, p)) = files.iter().find_map(|f| f.problem().map(|p| (f, p))) {
        return Err(format!("Not sent: {} cannot go: {p}. Remove it first.", attach::clipped(&f.name)));
    }
    let chars = safe.chars().count();
    let masked_line = (n > 0).then(|| format!("{n} piece{} of personal data masked before sending.", if n == 1 { "" } else { "s" }));
    if files.is_empty() {
        let mut c = Confirm::new(format!("Send this to {to}?"), push_call(&sb, thread.as_deref(), safe.to_string(), key, &to))
            .line(format!("To: {to}'s agent, typed into its session as one message."))
            .line(format!("{chars} character{}, shown in full below.", if chars == 1 { "" } else { "s" }));
        if let Some(l) = masked_line {
            c = c.line(l);
        }
        return Ok(c.preview(safe.to_string(), false).labels(&format!("Send to {sb}"), "Cancel"));
    }
    m.job_seq += 1;
    let job = m.job_seq;
    let tag = format!("attach:{job}");
    let todo: Vec<&Att> = files.iter().filter(|f| f.uploaded.is_none()).collect();
    let paths: Vec<&Att> = todo.iter().copied().filter(|f| matches!(f.payload, Payload::Path(_))).collect();
    let nf = files.len();
    let what = format!("upload {} file{} for {to}", todo.len(), if todo.len() == 1 { "" } else { "s" });
    let (call, waiting) = if !paths.is_empty() {
        let items: Vec<J> = paths.iter().map(|f| {
            let Payload::Path(p) = &f.payload else { unreachable!() };
            json!({ "route": format!("{}{}", crate::feat::files::upload_route_prefix(&sb), crate::client::enc(&f.name)), "file": p, "content_type": "application/octet-stream", "tag": f.tag() })
        }).collect();
        (core_call("uploads", json!({ "sandbox": sb, "items": items }), what, &tag), Some(Waiting::Batch))
    } else if let Some(f) = todo.first() {
        (png_call(&sb, f, &what, &tag), Some(Waiting::Png(f.id)))
    } else {
        // Every file is in the sandbox already (a send that stopped after its uploads): the push itself.
        (push_call(&sb, thread.as_deref(), message(safe, &files), key, &to), None)
    };
    if let Some(waiting) = waiting {
        // Until an answer comes, a Cancel of the dialog drops it (`on_dismiss`).
        m.jobs.insert(job, Job { key: key.to_string(), text: safe.to_string(), ids: files.iter().map(|f| f.id).collect(), waiting, sent: false });
    }
    let title = if chars == 0 { format!("Send {nf} file{} to {to}?", if nf == 1 { "" } else { "s" }) } else { format!("Send this and {nf} file{} to {to}?", if nf == 1 { "" } else { "s" }) };
    let mut c = Confirm::new(title, call).line(format!("To: {to}'s agent, typed into its session as one message."));
    for f in &files {
        let state = if f.uploaded.is_some() { " · already in the sandbox" } else if !f.look.is_empty() { " · look before you send" } else { "" };
        c = c.line(format!("{} · {}{state}", f.source(), crate::feat::files::view::size_label(f.size)));
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    c = c.line(format!("{nf} file{}, {} in all. Copied into {sb} first, one after the other (under /tmp/sbxw-uploads/, a picture under /tmp/sbxw-pastes/); a failure stops there and sends nothing.", if nf == 1 { "" } else { "s" }, crate::feat::files::view::size_label(total)));
    c = c.line(if chars == 0 { "The message is the files' paths in the sandbox.".to_string() } else { format!("Your words ({chars} character{}, in full below), then the files' paths in the sandbox.", if chars == 1 { "" } else { "s" }) });
    if let Some(l) = masked_line {
        c = c.line(l);
    }
    let look = attach::look_all(&files);
    for w in &look {
        c = c.line(format!("Look before you send: {w}."));
    }
    if !look.is_empty() {
        c = c.ack("I have looked at these files and want to send them");
    }
    let preview = if chars == 0 { "(no words: the files' paths only)".to_string() } else { safe.to_string() };
    Ok(c.preview(preview, false).labels(&format!("Upload and send to {sb}"), "Cancel"))
}

/// What is typed into the session: the words, then each file's path in the sandbox.
fn message(safe: &str, files: &[Att]) -> String {
    let mut parts: Vec<&str> = vec![];
    if !safe.is_empty() {
        parts.push(safe);
    }
    parts.extend(files.iter().filter_map(|f| f.uploaded.as_deref()));
    parts.join(" ")
}

fn push_call(sb: &str, thread: Option<&str>, text: String, key: &str, to: &str) -> Call {
    let mut body = json!({ "text": text, "name": sb });
    if let Some(t) = thread {
        body["thread"] = t.into();
    }
    Call::post("/api/chat/push", body, format!("send your words to {to}")).tag(&format!("send:{key}"))
}

fn png_call(sb: &str, f: &Att, what: &str, tag: &str) -> Call {
    let Payload::Png(b) = &f.payload else { unreachable!("a picture") };
    Call::post_bytes(format!("/api/sandboxes/{}/paste-image", crate::client::enc(sb)), "image/png", b.clone(), what.to_string()).tag(tag)
}

/// The core forgets these files' local paths (nothing can upload them any more).
fn forget(files: &[Att]) -> Option<Action> {
    let p: Vec<std::path::PathBuf> = files.iter().filter_map(|f| match &f.payload {
        Payload::Path(p) => Some(p.clone()),
        Payload::Png(_) => None,
    }).collect();
    (!p.is_empty()).then_some(Action::ForgetFiles(p))
}

/// An upload of send `n` answered: each file's daemon path kept; the next
/// picture, or, once every file is in, the words and the paths pushed. A
/// failure pushes nothing and says what went and what did not.
fn uploaded(m: &mut Model, cx: &Ctx, n: u64, r: Result<(u16, J), String>) -> Vec<Action> {
    let Some(job) = m.jobs.get_mut(&n).map(|j| {
        j.sent = true;
        j.clone()
    }) else {
        return vec![];
    };
    let key = job.key.clone();
    let mut failure: Option<String> = None;
    {
        let files = m.att.entry(key.clone()).or_default();
        let take = |f: &mut Att, code: u16, body: &J| -> Result<(), String> {
            if !(200..300).contains(&code) || body["ok"] == false {
                return Err(body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}")));
            }
            match body["path"].as_str() {
                Some(p) if crate::feat::files::typable(p) => {
                    f.uploaded = Some(p.to_string());
                    Ok(())
                }
                Some(_) => Err("uploaded, but the daemon's path is not one the app types into a prompt".into()),
                None => Err("uploaded, but the daemon gave no path".into()),
            }
        };
        match (&job.waiting, r) {
            (_, Err(e)) => failure = Some(format!("the upload did not run ({e})")),
            (Waiting::Batch, Ok((_, body))) => {
                let results = body["results"].as_array().cloned().unwrap_or_default();
                for f in files.iter_mut().filter(|f| job.ids.contains(&f.id) && f.uploaded.is_none() && matches!(f.payload, Payload::Path(_))) {
                    let Some(res) = results.iter().find(|x| x["tag"] == f.tag().as_str()) else {
                        failure.get_or_insert_with(|| format!("{} was not uploaded (a file before it failed)", attach::clipped(&f.name)));
                        break;
                    };
                    let out = match res["error"].as_str() {
                        Some(e) => Err(e.to_string()),
                        None => take(f, res["code"].as_u64().unwrap_or(0) as u16, &res["body"]),
                    };
                    if let Err(e) = out {
                        failure = Some(format!("{} was not uploaded: {e}", attach::clipped(&f.name)));
                        break;
                    }
                }
            }
            (Waiting::Png(id), Ok((code, body))) => {
                if let Some(f) = files.iter_mut().find(|f| f.id == *id) {
                    if let Err(e) = take(f, code, &body) {
                        failure = Some(format!("{} was not uploaded: {e}", attach::clipped(&f.name)));
                    }
                }
            }
        }
    }
    let files: Vec<Att> = m.att.get(&key).cloned().unwrap_or_default().into_iter().filter(|f| job.ids.contains(&f.id)).collect();
    // The core forgets an uploaded path: a file in the sandbox now keeps only its daemon path.
    let done: Vec<Att> = files.iter().filter(|f| f.uploaded.is_some()).cloned().collect();
    let mut out: Vec<Action> = forget(&done).into_iter().collect();
    if let Some(f) = failure {
        m.jobs.remove(&n);
        let names = |v: Vec<&Att>| v.iter().map(|f| attach::clipped(&f.name)).collect::<Vec<_>>().join(", ");
        let (inn, notin): (Vec<&Att>, Vec<&Att>) = files.iter().partition(|f| f.uploaded.is_some());
        let mut msg = format!("Not sent: {f}.");
        if !inn.is_empty() {
            msg.push_str(&format!(" In the sandbox already (they go with your next Send): {}.", names(inn)));
        }
        if !notin.is_empty() {
            msg.push_str(&format!(" Not uploaded: {}.", names(notin)));
        }
        msg.push_str(" Nothing was typed into the session; your words are still in the box.");
        out.push(Action::error(msg));
        out.push(view_action(m, cx));
        return out;
    }
    // The next pasted picture, one at a time.
    let (sb, thread) = match m.cards.get(&key) {
        Some(c) => (c.info["sandbox"].as_str().unwrap_or("").to_string(), model::thread_of(&c.info).map(str::to_string)),
        None => (String::new(), None),
    };
    let to = model::who(&sb, thread.as_deref());
    if let Some(f) = files.iter().find(|f| f.uploaded.is_none()) {
        let what = format!("upload a picture for {to}");
        let call = png_call(&sb, f, &what, &format!("attach:{n}"));
        if let Some(j) = m.jobs.get_mut(&n) {
            j.waiting = Waiting::Png(f.id);
        }
        out.push(Action::Call(call));
        out.push(view_action(m, cx));
        return out;
    }
    m.jobs.remove(&n);
    // Every file in: the session as it is now must still take typed words.
    match reachable(m, cx, &key) {
        Err(e) => {
            out.push(Action::error(format!("{} The files are in the sandbox and go with your next Send; your words are still in the box.", e.replacen("Not sent", "Uploaded, not sent", 1))));
        }
        Ok((sb, thread)) => {
            let to = model::who(&sb, thread.as_deref());
            out.push(Action::Call(push_call(&sb, thread.as_deref(), message(&job.text, &files), &key, &to)));
        }
    }
    out.push(view_action(m, cx));
    out
}

/// Files the person handed the Monitor: the file dialog's answer to
/// Attach, files dropped on it, a picture pasted while it shows. They wait
/// under the shown session's box; nothing is uploaded until its Send.
fn on_files(cx: &Ctx, inc: Incoming) -> Vec<Action> {
    let mut m = STATE().lock().unwrap();
    let picked = m.pick_for.take();
    let paths: Vec<std::path::PathBuf> = match &inc {
        Incoming::Files(p) => p.clone(),
        Incoming::Png(_) => vec![],
    };
    let refuse = |why: String| {
        let mut out = vec![Action::error(why)];
        if !paths.is_empty() {
            out.push(Action::ForgetFiles(paths.clone()));
        }
        out
    };
    if let Some(why) = held(cx) {
        return refuse(why.replacen("Not sent", "Nothing attached", 1));
    }
    let Some(key) = m.selected() else { return refuse("Nothing attached: no session is shown".into()) };
    if picked.as_ref().is_some_and(|p| *p != key) {
        return refuse("Nothing attached: the session shown changed while the file dialog was open".into());
    }
    if let Err(e) = reachable(&m, cx, &key) {
        return refuse(e.replacen("Not sent", "Nothing attached", 1));
    }
    if m.jobs.values().any(|j| j.key == key && j.sent) {
        return refuse("Nothing attached: a send of this session's files is under way".into());
    }
    let mut notes = vec![];
    let mut left: Vec<std::path::PathBuf> = vec![];
    match inc {
        Incoming::Files(list) => {
            for p in list {
                if m.att.get(&key).is_some_and(|v| v.len() >= attach::MAX) {
                    notes.push(format!("{} files at a time at most: the rest were left out", attach::MAX));
                    left.push(p);
                    continue;
                }
                m.att_seq += 1;
                let id = m.att_seq;
                match Att::of_path(id, p.clone()) {
                    Ok(a) => m.att.entry(key.clone()).or_default().push(a),
                    Err(e) => {
                        notes.push(format!("Not attached: {e}"));
                        left.push(p);
                    }
                }
            }
        }
        Incoming::Png(bytes) => {
            if m.att.get(&key).is_some_and(|v| v.len() >= attach::MAX) {
                notes.push(format!("{} files at a time at most", attach::MAX));
            } else {
                m.att_seq += 1;
                m.pastes += 1;
                let (id, n) = (m.att_seq, m.pastes);
                m.att.entry(key.clone()).or_default().push(Att::of_png(id, n, bytes));
            }
        }
    }
    notes.dedup();
    let mut out: Vec<Action> = notes.into_iter().map(Action::error).collect();
    if !left.is_empty() {
        out.push(Action::ForgetFiles(left));
    }
    out.push(view_action(&mut m, cx));
    out
}

/// A send's dialog closed unsent: its job goes; the files stay attached.
fn on_dismiss(_cx: &Ctx, tag: &str) -> Vec<Action> {
    if let Some(n) = tag.strip_prefix("attach:").and_then(|n| n.parse::<u64>().ok()) {
        let mut m = STATE().lock().unwrap();
        if m.jobs.get(&n).is_some_and(|j| !j.sent) {
            m.jobs.remove(&n);
        }
    }
    vec![]
}

/// The links of a card: its terminal, its commits, files and worktree, each
/// through the intent that owns it.
fn links(m: &Model, cx: &Ctx, name: &str, key: &str) -> Result<Vec<Action>, String> {
    let c = m.cards.get(key).ok_or_else(|| format!("{key} is not a session Sessions lists"))?;
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
        // Scoped to the sandbox (its capsule, scope.rs); a thread's row
        // also filters to that thread, a main session clears an old filter.
        _ => vec![intent("worktrees_show", json!({ "sandbox": sb })), intent("worktrees_filter", json!({ "text": thread.unwrap_or_default() }))],
    })
}

/// The stand-in daemon's history routes, from a fixture's
/// `daemon.history` (`{"<sandbox>/<session_id>": {items, truncated_before,
/// partial, partial_reason, ended}}`), as `crates/core/src/history.rs`
/// pages them. Every read and delete is noted in `history_calls` (tests
/// and demo marks read it); a read subscribes (`history_subs`), a delete
/// ends it.
fn standin(st: &mut super::Standin, method: &str, path: &str, _body: &J) -> Option<(u16, J)> {
    let (p, query) = path.split_once('?').unwrap_or((path, ""));
    let rest = p.strip_prefix("/api/sessions/")?;
    let parts: Vec<String> = rest.split('/').map(pct_decode).collect();
    let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).and_then(|v| v.parse::<u64>().ok());
    let (sb, sid) = match parts.as_slice() {
        [sb, sid, h] if h == "history" && method == "GET" => (sb.clone(), sid.clone()),
        [sb, sid, h, s] if h == "history" && s == "subscription" && method == "DELETE" => {
            let k = format!("{sb}/{sid}");
            note(st.data, format!("DELETE {k}"));
            if let Some(o) = st.data["history_subs"].as_object_mut() {
                o.remove(&k);
            }
            return Some((200, json!({ "ok": true })));
        }
        _ => return None,
    };
    let k = format!("{sb}/{sid}");
    note(st.data, format!("GET {k} before={} limit={}", q("before").map_or("-".into(), |b| b.to_string()), q("limit").unwrap_or(100)));
    let log = st.data["history"][&k].clone();
    if log.is_null() {
        return Some((404, json!({ "ok": false, "error": format!("no history for {k}") })));
    }
    if !st.data["history_subs"].is_object() {
        st.data["history_subs"] = json!({});
    }
    st.data["history_subs"][&k] = true.into();
    let items = log["items"].as_array().cloned().unwrap_or_default();
    let limit = q("limit").unwrap_or(100).clamp(1, 200) as usize;
    let older: Vec<&J> = items.iter().filter(|i| q("before").is_none_or(|b| i["seq"].as_u64().unwrap_or(0) < b)).collect();
    let start = older.len().saturating_sub(limit);
    let page: Vec<J> = older[start..].iter().map(|i| (*i).clone()).collect();
    let next_before = (start > 0).then(|| page.first().and_then(|i| i["seq"].as_u64())).flatten();
    let latest = items.iter().filter_map(|i| i["seq"].as_u64()).max().map_or(0, |m| m + 1);
    let mut out = json!({ "items": page, "latest_seq": latest, "truncated_before": log["truncated_before"] == true, "partial": log["partial"] == true, "ended": log["ended"] == true });
    if let Some(b) = next_before {
        out["next_before"] = b.into();
    }
    if let Some(r) = log["partial_reason"].as_str() {
        out["partial_reason"] = r.into();
    }
    Some((200, json!({ "ok": true, "sandbox": sb, "session_id": sid, "page": out, "limits": { "page_max": 200 } })))
}

fn note(data: &mut J, line: String) {
    if !data["history_calls"].is_array() {
        data["history_calls"] = json!([]);
    }
    data["history_calls"].as_array_mut().unwrap().push(line.into());
}

/// `%XX` undone (the stand-in's paths).
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
