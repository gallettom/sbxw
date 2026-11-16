//! Threads, plans and the lead, at parity with the web UI's thread parts of
//! `panes.js` (sessions, New thread, Close thread, the pane's ⋯ Remote
//! control), `plans.js` (Team mode, the plan dialog, the merge), `board.js`
//! and `worktrees.js`.
//!
//! * **Sessions.** Every Claude session of a sandbox (the main one, sessions
//!   in the main checkout, threads on worktrees) with where it stands, from
//!   `GET /api/sessions` (hook reports), `GET /api/sandboxes/:name/threads`
//!   and the plans. Shown as rows under the sandbox in the sidebar, in the
//!   Threads panel, and on pane bars (extras).
//! * **Plans.** A plan the lead proposes opens the `plans` dialog. The
//!   person's edits (remove a thread, edit its text, repo, model, the
//!   variant) are kept HERE, in `data.threads_ui.drafts`, changed one intent
//!   at a time: the core's approval dialog lists the draft as this file
//!   built it, never text a screen made up.
//! * **Approval.** Approving a plan, merging, closing a thread with its
//!   worktree, and switching a sandbox's login for Remote Control are intents
//!   of risk `Approval`: each answers one `Confirm` or `Prepare` built from
//!   the daemon's data. A screen cannot approve or merge by itself.
//! * **The ceiling.** `max_agent_sessions` is said in plain words before the
//!   daemon refuses (the approval's `Prepare` reads the sandbox's sessions),
//!   and again if it refuses anyway: how many are open, which, how many
//!   places are free, how to free one ([`model::ceiling_text`]).

pub mod model;
pub mod standin;
#[cfg(test)]
mod tests;

use crate::feat::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, Prepare, ScreenDecl, Slot};
use crate::intent::{Arg, Risk, Spec};
use model::{Budget, Session};
use serde_json::{json, Map, Value as J};

pub const FEATURE: Feature = Feature {
    name: "threads",
    data: DATA,
    screens: &[
        ScreenDecl { name: "threads", src: include_str!("../../../screens/threads.luau"), slot: Slot::Panel { title: "Threads", icon: "terminal", width: 360.0 } },
        ScreenDecl { name: "plans", src: include_str!("../../../screens/plans.luau"), slot: Slot::Modal { width: 760.0 } },
        ScreenDecl { name: "threads-new", src: include_str!("../../../screens/threads-new.luau"), slot: Slot::Modal { width: 520.0 } },
    ],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    on_data: Some(on_data),
    extras: Some(extras),
    standin: Some(standin::answer),
    ..Feature::EMPTY
};

/// The data key the feature keeps its own state under (set, never read from a route).
pub const UI: &str = "threads_ui";

const DATA: &[Data] = &[
    // Hook reports: working, waiting for the person, idle.
    Data { key: "sessions", path: "/api/sessions", on: &["session"], every: 0, lazy: false },
    // `{ok, threads: [{name, open, repo, model, worktree}], open, max}`, per running sandbox.
    Data { key: "threads", path: "/api/sandboxes/{sandbox}/threads", on: &["session", "plan", "lead-end"], every: 15, lazy: false },
    // Every plan, as the daemon keeps them (`sbxw_core::plans::Plan`).
    Data { key: "plans", path: "/api/plans", on: &["plan", "lead-end"], every: 0, lazy: false },
    // Sandboxes whose main session was made lead.
    Data { key: "leads", path: "/api/leads", on: &["lead", "lead-end"], every: 0, lazy: false },
    // `{ok, default, models: [{id, label}]}`.
    Data::new("models", "/api/models"),
    // A shared plan's board, per running sandbox: `{sandbox, plan, claims, entries}`.
    Data { key: "board", path: "/api/sandboxes/{sandbox}/board", on: &["board", "plan"], every: 0, lazy: false },
];

const KINDS: &[&str] = &["thread", "session"];
const TABS: &[&str] = &["sessions", "worktrees", "board"];
const FIELDS: &[&str] = &["name", "goal", "scope", "repo", "model"];
const VARIANTS: &[&str] = &["split", "shared"];

const fn d(name: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Direct, args, doc }
}
const fn a(name: &'static str, asks: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Approval(asks), args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("threads_show", &[("sandbox", Arg::Str, false), ("tab", Arg::OneOf(TABS), false)], "Opens the Threads panel on a sandbox (none: the selected one), on a tab (`sessions`, `worktrees`, `board`)."),
    d("thread_new", &[("sandbox", Arg::Str, true), ("kind", Arg::OneOf(KINDS), false)], "Opens the New thread dialog (`kind = session`: a session in the main checkout, no worktree)."),
    d(
        "thread_open",
        &[("sandbox", Arg::Str, true), ("name", Arg::Str, true), ("repo", Arg::Str, false), ("model", Arg::Str, false), ("checkout", Arg::Bool, false), ("pane", Arg::Pane, false)],
        "Opens a thread's session in a pane (spare by default), creating it the first time: `repo` (`=` then its path; `=` alone is the workspace), `model`, `checkout` (in the main checkout) apply to a new one only.",
    ),
    d("thread_bash", &[("sandbox", Arg::Str, true), ("thread", Arg::Str, false)], "Opens a shell in a thread's worktree (none: the sandbox's own Bash)."),
    d("session_interrupt", &[("sandbox", Arg::Str, true), ("thread", Arg::Str, false)], "Sends Escape to a Claude session (the main one without `thread`): Claude Code stops what it is doing."),
    d("remote_control", &[("sandbox", Arg::Str, true), ("thread", Arg::Str, false)], "Turns Remote Control on for a session, after reading how Claude is signed in there (a setup token needs `remote_login`)."),
    a("remote_login", "Switch a sandbox to a claude.ai login", &[("sandbox", Arg::Str, true), ("thread", Arg::Str, false)], "Stops injecting the setup token into that sandbox for good and types /login in the session, for Remote Control."),
    d("worktrees_load", &[("sandbox", Arg::Str, true)], "Reads the sandbox's worktrees again (`data.worktrees`)."),
    d("lead_start", &[("sandbox", Arg::Str, true)], "Team mode: makes the sandbox's main session lead (it plans threads and follows them)."),
    d("lead_end", &[("sandbox", Arg::Str, true)], "Ends Team mode, after the core's dialog says what that leaves behind (threads run on, a waiting plan is refused)."),
    d("plan_review", &[("id", Arg::Str, true)], "Opens the plan dialog: the review while it waits, its threads' state once approved, the merge once proposed."),
    d("plans_reload", &[], "Reads the plans again."),
    d(
        "plan_edit",
        &[("id", Arg::Str, true), ("index", Arg::Int(1, 64), true), ("field", Arg::OneOf(FIELDS), true), ("value", Arg::Str, false)],
        "Edits one thread of a waiting plan's draft (`index` from 1): `name`, `goal`, `scope`, `repo` (`=` then its path, none: the default), `model` (none: the default).",
    ),
    d("plan_remove", &[("id", Arg::Str, true), ("index", Arg::Int(1, 64), true)], "Removes a thread from the draft."),
    d("plan_add", &[("id", Arg::Str, true)], "Adds an empty thread to the draft."),
    d("plan_variant", &[("id", Arg::Str, true), ("variant", Arg::OneOf(VARIANTS), true)], "Split (a worktree and a branch per thread) or shared (one worktree, files claimed on a board)."),
    d("plan_reset", &[("id", Arg::Str, true)], "Puts the draft back as the lead proposed it."),
    d("plan_refuse", &[("id", Arg::Str, true), ("note", Arg::Str, false)], "Refuses a waiting plan; the lead is told, with the note."),
    d("merge_close_set", &[("id", Arg::Str, true), ("on", Arg::Bool, true)], "Whether the merge closes each thread (session, worktree, branch) once all merged."),
    a("plan_approve", "Approve a plan and start its threads", &[("id", Arg::Str, true)], "Opens the core's dialog with the draft as it stands and the sessions it starts; Approve there starts the threads."),
    a("plan_merge", "Merge a plan's threads", &[("id", Arg::Str, true)], "Opens the core's dialog with the lead's merge order and note; Merge there merges each thread in order, stopping at the first failure."),
    a("thread_close", "Close a thread with its worktree", &[("sandbox", Arg::Str, true), ("thread", Arg::Str, true)], "Reads what closing would lose (uncommitted files, commits), then the core's dialog; Close there stops the session and removes its worktree."),
];

// ---------------------------------------------------------------------------
// State kept in `data.threads_ui`

fn ui(ctx: &Ctx) -> J {
    match ctx.get(UI) {
        J::Object(o) => J::Object(o.clone()),
        _ => json!({}),
    }
}

fn set_ui(v: J) -> Action {
    Action::Set { key: UI.into(), value: v }
}

fn obj<'a>(v: &'a mut J, k: &str) -> &'a mut Map<String, J> {
    if !v[k].is_object() {
        v[k] = json!({});
    }
    v[k].as_object_mut().unwrap()
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o += &format!("%{b:02X}");
        }
    }
    o
}

fn sb_path(sandbox: &str) -> String {
    format!("/api/sandboxes/{}", enc(sandbox))
}

/// `"sandbox\nthread"` (thread empty for the main session): the key of a session in `threads_ui`.
fn skey(sandbox: &str, thread: Option<&str>) -> String {
    format!("{sandbox}\n{}", thread.unwrap_or(""))
}

fn who(sandbox: &str, thread: Option<&str>) -> String {
    match thread {
        Some(t) => format!("{t} of {sandbox}"),
        None => format!("the main session of {sandbox}"),
    }
}

/// Where a pane bar extra for one session shows.
fn on_session(sandbox: &str, thread: Option<&str>) -> Place {
    Place::Session { sandbox: sandbox.to_string(), thread: thread.map(str::to_string) }
}

/// The rows under a sandbox in the sidebar.
fn on_rows(sandbox: &str) -> Place {
    Place::SandboxRow(sandbox.to_string())
}

// ---------------------------------------------------------------------------
// Intents

fn act(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    match q.name {
        "threads_show" => {
            let sb = q.s("sandbox").map(str::to_string).or_else(|| ctx.data["app"]["selected"].as_str().map(str::to_string));
            let mut u = ui(ctx);
            if let Some(s) = &sb {
                u["sandbox"] = json!(s);
            }
            if let Some(t) = q.s("tab") {
                u["tab"] = json!(t);
            }
            let mut out = vec![set_ui(u), Action::OpenPanel("threads".into())];
            if let Some(s) = sb {
                out.insert(0, Action::Intent { name: "select".into(), args: json!({ "sandbox": s }) });
                if q.s("tab") == Some("worktrees") {
                    out.extend(load_worktrees(ctx, &s));
                }
            }
            Ok(out)
        }
        "thread_new" => {
            let sb = q.req("sandbox");
            if ctx.sandbox(sb).is_none() {
                return Err(format!("no sandbox {sb}"));
            }
            let kind = q.s("kind").unwrap_or("thread");
            let used: Vec<String> = model::sessions(ctx.data, sb).into_iter().filter_map(|s| s.thread).collect();
            let prefix = if kind == "session" { "s" } else { "t" };
            let n = (1..).find(|n| !used.contains(&format!("{prefix}{n}"))).unwrap_or(1);
            let mut u = ui(ctx);
            u["new"] = json!({ "sandbox": sb, "kind": kind, "suggest": format!("{prefix}{n}") });
            u["repos_for"] = json!(sb);
            Ok(vec![
                set_ui(u),
                Action::Set { key: "thread_repos".into(), value: J::Null },
                Action::Fetch { key: "thread_repos".into(), path: format!("{}/repos", sb_path(sb)) },
                Action::OpenModal { screen: "threads-new".into(), arg: json!(sb) },
            ])
        }
        "thread_open" => thread_open(ctx, q),
        "thread_bash" => {
            let mut args = json!({ "sandbox": q.req("sandbox"), "mode": "bash", "pane": "spare" });
            if let Some(t) = q.s("thread") {
                args["thread"] = json!(t);
            }
            Ok(vec![Action::Intent { name: "open_terminal".into(), args }])
        }
        "session_interrupt" => {
            let (sb, t) = (q.req("sandbox"), q.s("thread"));
            let mode = t.map(|t| format!("thread:{t}")).unwrap_or_else(|| "claude".into());
            Ok(vec![Action::Call(Call::post("/api/input", json!({ "sandbox": sb, "mode": mode, "data": "\u{1b}" }), format!("interrupt {}", who(sb, t))).tag("interrupt").quiet())])
        }
        "remote_control" => {
            let (sb, t) = (q.req("sandbox"), q.s("thread"));
            let path = match t {
                Some(t) => format!("{}/remote-control?thread={}", sb_path(sb), enc(t)),
                None => format!("{}/remote-control", sb_path(sb)),
            };
            Ok(vec![Action::Call(Call::get(path, format!("read Remote Control of {}", who(sb, t))).tag(&format!("rc-look:{}", skey(sb, t))))])
        }
        "remote_login" => {
            let (sb, t) = (q.req("sandbox"), q.s("thread"));
            Ok(vec![Confirm::new(
                format!("Switch {sb} to a claude.ai login?"),
                Call::post(format!("{}/remote-control/login", sb_path(sb)), json!({ "thread": t }), format!("switch {sb} to a claude.ai login")).tag(&format!("rc-login:{}", skey(sb, t))),
            )
            .line(format!("{sb} is signed in with a setup token (CLAUDE_CODE_OAUTH_TOKEN), which Remote Control refuses."))
            .line("sbxw stops injecting the token into this sandbox for good, and types /login in the session for you to finish.")
            .line("Then choose Remote control again.")
            .labels("Switch to a login", "Cancel")
            .into_action()])
        }
        "worktrees_load" => Ok(load_worktrees(ctx, q.req("sandbox"))),
        "lead_start" => {
            let sb = q.req("sandbox");
            Ok(vec![Action::Call(Call::post(format!("{}/lead", sb_path(sb)), json!({}), format!("Team mode for {sb}")).then(&["leads"]).tag(&format!("lead:{sb}")))])
        }
        "lead_end" => {
            let sb = q.req("sandbox");
            let mut running = 0;
            for p in ctx.get("plans").as_array().into_iter().flatten().filter(|p| p["sandbox"] == sb && p["closed"] != true && p["state"] == "approved") {
                running += p["threads"].as_array().into_iter().flatten().filter(|t| t["state"] != "done" && t["state"] != "failed").count();
            }
            let mut c = Confirm::new(format!("End Team mode for {sb}?"), Call::post(format!("{}/lead/end", sb_path(sb)), json!({}), format!("end Team mode for {sb}")).then(&["leads", "plans"]))
                .line(format!("The main session of {sb} stops leading and goes back to being an ordinary session."));
            if running > 0 {
                c = c.line(format!(
                    "{running} thread{} still at work. {} running (close {} with Close thread), but {} no longer reach the main session.",
                    if running == 1 { " is" } else { "s are" },
                    if running == 1 { "It keeps" } else { "They keep" },
                    if running == 1 { "it" } else { "them" },
                    if running == 1 { "its reports" } else { "their reports" },
                ));
            }
            if model::pending_plan(ctx.data, sb).is_some() {
                c = c.line("The plan waiting for your approval will be refused.");
            }
            Ok(vec![c.labels("End Team mode", "Cancel").into_action()])
        }
        "plan_review" => {
            let id = q.req("id");
            let p = model::plan(ctx.data, id).ok_or_else(|| format!("no plan {id}"))?;
            let mut u = ui(ctx);
            if p["state"] == "pending" && !u["drafts"][id].is_object() {
                obj(&mut u, "drafts").insert(id.into(), model::draft_of(p));
            }
            let sb = p["sandbox"].as_str().unwrap_or("");
            u["repos_for"] = json!(sb);
            Ok(vec![
                set_ui(u),
                Action::Refresh("plans"),
                Action::Set { key: "thread_repos".into(), value: J::Null },
                Action::Fetch { key: "thread_repos".into(), path: format!("{}/repos", sb_path(sb)) },
                Action::OpenModal { screen: "plans".into(), arg: json!(id) },
            ])
        }
        "plans_reload" => Ok(vec![Action::Refresh("plans"), Action::Refresh("board")]),
        "plan_edit" | "plan_remove" | "plan_add" | "plan_variant" | "plan_reset" => edit_draft(ctx, q),
        "plan_refuse" => {
            let id = q.req("id");
            let p = model::plan(ctx.data, id).ok_or_else(|| format!("no plan {id}"))?;
            let mut body = json!({});
            if let Some(n) = q.s("note") {
                body["note"] = json!(n);
            }
            Ok(vec![Action::Call(
                Call::post(format!("/api/plans/{}/refuse", enc(id)), body, format!("refuse the plan of {}", p["sandbox"].as_str().unwrap_or("?"))).then(&["plans"]).tag(&format!("refuse:{id}")),
            )])
        }
        "merge_close_set" => {
            let mut u = ui(ctx);
            obj(&mut u, "merge_close").insert(q.req("id").into(), json!(q.b("on").unwrap_or(true)));
            Ok(vec![set_ui(u)])
        }
        "plan_approve" => plan_approve(ctx, q),
        "plan_merge" => plan_merge(ctx, q),
        "thread_close" => {
            let (sb, t) = (q.req("sandbox"), q.req("thread"));
            if !model::valid_name(t) {
                return Err(format!("{t}: not a thread name"));
            }
            Ok(vec![Action::Prepare(Box::new(Prepare {
                title: format!("Reading what closing {t} would lose"),
                call: Call::get(format!("{}/threads/{}", sb_path(sb), enc(t)), format!("read thread {t} of {sb}")).timeout(30),
                asked: q.clone(),
                build: close_build,
            }))])
        }
        other => Err(format!("{other}: not a threads intent")),
    }
}

fn load_worktrees(ctx: &Ctx, sb: &str) -> Vec<Action> {
    let mut u = ui(ctx);
    u["worktrees_for"] = json!(sb);
    vec![set_ui(u), Action::Set { key: "worktrees".into(), value: J::Null }, Action::Fetch { key: "worktrees".into(), path: format!("{}/worktrees", sb_path(sb)) }]
}

/// The pane already showing that session, from 1.
fn pane_of(ctx: &Ctx, sandbox: &str, thread: Option<&str>) -> Option<u64> {
    ctx.data["app"]["panes"].as_array()?.iter().find(|p| p["sandbox"] == sandbox && p["thread"].as_str() == thread && p["mode"] == "agent").and_then(|p| p["index"].as_u64())
}

fn thread_open(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let (sb, name) = (q.req("sandbox"), q.req("name").trim());
    if !model::valid_name(name) {
        return Err(format!("\"{name}\": lower-case letters, digits and inner hyphens only, 40 at most"));
    }
    if ctx.sandbox(sb).is_none() {
        return Err(format!("no sandbox {sb}"));
    }
    let checkout = q.b("checkout").unwrap_or(false);
    let known = model::sessions(ctx.data, sb).into_iter().find(|s| s.thread.as_deref() == Some(name));
    if let Some(k) = &known {
        // A name is one session: taken by the other kind, refused here
        // rather than by the daemon after the pane opened.
        if q.args.contains_key("checkout") && (k.kind == "same") != checkout {
            return Err(if checkout { format!("{name} is a thread with a worktree of its own") } else { format!("{name} is a session on the main checkout") });
        }
        if let Some(p) = pane_of(ctx, sb, Some(name)) {
            return Ok(vec![Action::CloseModal, Action::notice(format!("{name} is already in pane {p}"))]);
        }
    }
    // A session not running yet takes a place under the ceiling.
    if known.as_ref().is_none_or(|k| !k.running) {
        if let Some(b) = Budget::of(&ctx.data["threads"][sb]) {
            if b.free() == 0 {
                let which = if b.names.is_empty() { String::new() } else { format!(" ({})", b.names.join(", ")) };
                return Err(format!(
                    "{name} cannot start: {sb} has {} of its {} Claude sessions open{which}, no place is free. Close a thread you no longer need, or raise max_agent_sessions in sbxw.toml.",
                    b.open, b.max
                ));
            }
        }
    }
    let mut args = json!({ "sandbox": sb, "thread": name, "pane": q.args.get("pane").cloned().unwrap_or(json!("spare")) });
    if known.is_none() {
        if checkout {
            args["checkout"] = json!(true);
        } else if let Some(r) = q.s("repo") {
            let r = r.strip_prefix('=').ok_or("repo: `=` then its path")?;
            args["repo"] = json!(r);
        }
        if let Some(m) = q.s("model") {
            if ctx.get("models")["default"].as_str() != Some(m) {
                args["model"] = json!(m);
            }
        }
    }
    let mut u = ui(ctx);
    u["new"] = J::Null;
    Ok(vec![set_ui(u), Action::CloseModal, Action::Intent { name: "open_terminal".into(), args }, Action::Refresh("threads")])
}

fn edit_draft(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let id = q.req("id");
    let p = model::plan(ctx.data, id).ok_or_else(|| format!("no plan {id}"))?;
    if p["state"] != "pending" {
        return Err(format!("the plan of {} is no longer waiting", p["sandbox"].as_str().unwrap_or("?")));
    }
    let mut u = ui(ctx);
    let drafts = obj(&mut u, "drafts");
    let draft = drafts.entry(id.to_string()).or_insert_with(|| model::draft_of(p));
    if q.name == "plan_reset" {
        *draft = model::draft_of(p);
        return Ok(vec![set_ui(u)]);
    }
    if q.name == "plan_variant" {
        draft["variant"] = json!(q.req("variant"));
        return Ok(vec![set_ui(u)]);
    }
    let list = draft["threads"].as_array_mut().ok_or("draft without threads")?;
    if q.name == "plan_add" {
        if list.len() >= 64 {
            return Err("64 threads at most".into());
        }
        list.push(json!({ "name": "", "goal": "", "scope": "" }));
        return Ok(vec![set_ui(u)]);
    }
    let i = q.i("index").unwrap_or(1) as usize - 1;
    if i >= list.len() {
        return Err(format!("no thread {} in the draft", i + 1));
    }
    if q.name == "plan_remove" {
        list.remove(i);
        return Ok(vec![set_ui(u)]);
    }
    let t = &mut list[i];
    let v = q.s("value");
    match q.req("field") {
        "repo" => match v {
            None => {
                t.as_object_mut().unwrap().remove("repo");
            }
            Some(r) => t["repo"] = json!(r.strip_prefix('=').ok_or("repo: `=` then its path")?),
        },
        "model" => match v {
            None => {
                t.as_object_mut().unwrap().remove("model");
            }
            Some(m) => t["model"] = json!(m),
        },
        f => t[f] = json!(v.unwrap_or("")),
    }
    Ok(vec![set_ui(u)])
}

fn check_draft(draft: &J) -> Result<(), String> {
    let list = draft["threads"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Err("Keep at least one thread, or refuse the plan.".into());
    }
    let mut seen = vec![];
    for t in &list {
        let n = t["name"].as_str().unwrap_or("").trim().to_string();
        if !model::valid_name(&n) {
            return Err(format!("\"{}\": a thread's name is lower-case letters, digits and inner hyphens only. Edit it first.", if n.is_empty() { "Unnamed thread" } else { &n }));
        }
        if t["goal"].as_str().unwrap_or("").trim().is_empty() {
            return Err(format!("{n} has no goal. Edit it first."));
        }
        if seen.contains(&n) {
            return Err(format!("{n} is in the plan twice."));
        }
        seen.push(n);
    }
    Ok(())
}

fn plan_approve(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let id = q.req("id");
    let p = model::plan(ctx.data, id).ok_or_else(|| format!("no plan {id}"))?;
    if p["state"] != "pending" || p["closed"] == true {
        return Err(format!("the plan of {} is no longer waiting", p["sandbox"].as_str().unwrap_or("?")));
    }
    let draft = ui(ctx)["drafts"][id].clone();
    let draft = if draft.is_object() { draft } else { model::draft_of(p) };
    check_draft(&draft)?;
    let sb = p["sandbox"].as_str().unwrap_or("");
    // The sessions open now are read by the core itself, then the dialog is built.
    Ok(vec![Action::Prepare(Box::new(Prepare {
        title: format!("Reading the sessions of {sb}"),
        call: Call::get(format!("{}/threads", sb_path(sb)), format!("read the sessions of {sb}")).timeout(30),
        asked: q.clone(),
        build: approve_build,
    }))])
}

/// The approval dialog, from the plan the daemon holds, the draft this file
/// keeps, and the sessions the core just read.
fn approve_build(ctx: &Ctx, q: &Asked, view: &J) -> Result<Confirm, String> {
    let id = q.req("id");
    let p = model::plan(ctx.data, id).ok_or_else(|| format!("no plan {id}"))?;
    let sb = p["sandbox"].as_str().unwrap_or("").to_string();
    let draft = ui(ctx)["drafts"][id].clone();
    let draft = if draft.is_object() { draft } else { model::draft_of(p) };
    check_draft(&draft)?;
    let data = json!({ "threads": { &sb: view } });
    let new = model::new_sessions(&data, &sb, &draft);
    let budget = Budget::of(view);
    if let Some(b) = &budget {
        if b.open + new > b.max {
            return Err(model::ceiling_text(&sb, b, new));
        }
    }
    let shared = draft["variant"] == "shared";
    let models = ctx.get("models");
    let model_label = |m: &str| models["models"].as_array().and_then(|l| l.iter().find(|x| x["id"] == m)).and_then(|x| x["label"].as_str()).unwrap_or(m).to_string();
    let mut c = Confirm::new(
        format!("Approve the plan of {sb}?"),
        Call::post(format!("/api/plans/{}/approve", enc(id)), model::approve_body(&draft), format!("approve the plan of {sb}")).then(&["plans", "threads"]).tag(&format!("approve:{id}")),
    );
    if let Some(s) = p["summary"].as_str().filter(|s| !s.is_empty()) {
        c = c.line(s.to_string());
    }
    c = c.line(if shared { "Shared: one worktree and one branch for every thread, files claimed on a board." } else { "Split: a worktree and a branch per thread, each merged at the end." });
    let mut list = String::new();
    for (i, t) in draft["threads"].as_array().into_iter().flatten().enumerate() {
        let m = t["model"].as_str().filter(|m| !m.is_empty()).map(model_label).unwrap_or_else(|| "default model".into());
        let repo = t["repo"].as_str().map(|r| format!(", in {}", model::repo_label(Some(r)))).unwrap_or_default();
        list += &format!("{}. {} ({m}{repo})\n   {}\n", i + 1, t["name"].as_str().unwrap_or(""), model::first_sentence(t["goal"].as_str().unwrap_or("")));
        if let Some(s) = t["scope"].as_str().filter(|s| !s.trim().is_empty()) {
            list += &format!("   Owns: {}\n", s.trim());
        }
    }
    c = c.preview(list.trim_end().to_string(), false);
    let changes = model::draft_changes(p, &draft);
    if !changes.is_empty() {
        c = c.line(format!("Your changes: {}.", changes.join("; ")));
    }
    if let Some(b) = &budget {
        let which = if b.names.is_empty() { String::new() } else { format!(" ({})", b.names.join(", ")) };
        c = c.line(format!(
            "Starts {new} new Claude session{}: {sb} has {} of {} open now{which}, {} free.",
            if new == 1 { "" } else { "s" },
            b.open,
            b.max,
            b.free()
        ));
    }
    Ok(c.labels("Approve and start", "Cancel"))
}

fn plan_merge(ctx: &Ctx, q: &Asked) -> Result<Vec<Action>, String> {
    let id = q.req("id");
    let p = model::plan(ctx.data, id).ok_or_else(|| format!("no plan {id}"))?;
    let m = &p["merge"];
    let sb = p["sandbox"].as_str().unwrap_or("");
    match m["state"].as_str() {
        Some("proposed") | Some("failed") => {}
        Some("merging") => return Err(format!("the threads of {sb} are being merged")),
        Some("merged") => return Err(format!("the threads of {sb} are merged already")),
        _ => return Err(format!("the lead of {sb} has proposed no merge")),
    }
    let shared = p["variant"] == "shared";
    if shared {
        let claims = ctx.get("board")[sb]["claims"].as_array().cloned().unwrap_or_default();
        if !claims.is_empty() {
            let list: Vec<String> = claims.iter().map(|c| format!("{} ({})", c["file"].as_str().unwrap_or("?"), c["owner"].as_str().unwrap_or("?"))).collect();
            return Err(format!("{} still claimed: {}. Merge waits until they are released.", if list.len() == 1 { "1 file is".to_string() } else { format!("{} files are", list.len()) }, list.join(", ")));
        }
    }
    let close = ui(ctx)["merge_close"][id].as_bool().unwrap_or(true);
    let (rows, _) = model::merge_rows(p);
    let done: Vec<&str> = m["results"].as_array().into_iter().flatten().filter(|r| r["ok"] == true).filter_map(|r| r["thread"].as_str()).collect();
    let into = match m["base"].as_str() {
        Some(b) => format!("into {b}"),
        None => "into the branch its repo's main checkout is on".to_string(),
    };
    let mut c = Confirm::new(
        format!("Merge the threads of {sb}?"),
        Call::post(format!("/api/plans/{}/merge", enc(id)), json!({ "close_threads": close }), format!("merge the threads of {sb}")).then(&["plans", "threads"]).timeout(600).tag(&format!("merge:{id}")),
    );
    c = c.line(if shared {
        format!("One branch, sbxw/team-{id}, merged once {into}.")
    } else {
        format!("Each thread's branch, in this order, {into}. A failure stops the run: what follows it is not merged.")
    });
    let order: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let n = r["name"].as_str().unwrap_or("");
            format!("{}. {n}{}", i + 1, if done.contains(&n) { " (merged already, skipped)" } else { "" })
        })
        .collect();
    c = c.preview(order.join("\n"), false);
    if let Some(n) = m["note"].as_str().filter(|n| !n.is_empty()) {
        c = c.line(format!("The lead's note: {n}"));
    }
    let kept: Vec<String> = m["distillate"].as_array().into_iter().flatten().map(|d| format!("{} {}", if d["kind"] == "warning" { "Warning:" } else { "Decision:" }, d["text"].as_str().unwrap_or(""))).collect();
    if !kept.is_empty() {
        c = c.line(format!("Kept in the sandbox's auto memory with the merge: {}", kept.join(" | ")));
    }
    c = c.line(if close { "Then each merged thread is closed: its session, its worktree and its branch." } else { "The threads stay open after the merge." });
    Ok(vec![c.labels(if m["state"] == "failed" { "Retry merge" } else { "Merge" }, "Cancel").into_action()])
}

/// The close dialog, from what the daemon says closing would lose.
fn close_build(_ctx: &Ctx, q: &Asked, v: &J) -> Result<Confirm, String> {
    let (sb, t) = (q.req("sandbox"), q.req("thread"));
    let st = &v["status"];
    if !st.is_object() {
        return Err(format!("Could not read thread {t}: {}", v["error"].as_str().unwrap_or("no status in the answer")));
    }
    let wt = st["worktree"].as_str().unwrap_or("?");
    let url = format!("{}/threads/{}/close", sb_path(sb), enc(t));
    let what = format!("close {t} of {sb}");
    if st["same_worktree"] == true {
        return Ok(Confirm::new(format!("Close session {t}?"), Call::post(url, json!({ "discard": false, "delete_branch": false }), what).then(&["threads", "sessions"]).tag(&format!("close:{sb}\n{t}")))
            .line(format!("The session stops. It works on the main checkout, which stays as it is: {wt}"))
            .labels("Close session", "Cancel")
            .danger());
    }
    if st["team"].is_string() {
        let line = if st["last"] == true {
            format!("The session stops and leaves its Team plan, which ends with it. The shared worktree is removed if it holds no change and no commit: {wt}")
        } else {
            format!("The session stops and leaves its Team plan. The shared worktree stays for the plan's other threads: {wt}")
        };
        return Ok(Confirm::new(format!("Close thread {t}?"), Call::post(url, json!({ "discard": false, "delete_branch": false }), what).then(&["threads", "sessions", "plans"]).tag(&format!("close:{sb}\n{t}")))
            .line(line)
            .labels("Close thread", "Cancel")
            .danger());
    }
    let dirty = st["dirty"].as_u64().unwrap_or(0);
    let ahead = st["ahead"].as_u64().unwrap_or(0);
    let has_branch = st["has_branch"] == true;
    let (branch, base) = (st["branch"].as_str().unwrap_or("its branch"), st["base"].as_str().unwrap_or("its base"));
    let delete_branch = has_branch && ahead == 0;
    let mut c = Confirm::new(
        format!("Close thread {t}?"),
        Call::post(url, json!({ "discard": dirty > 0, "delete_branch": delete_branch }), what).then(&["threads", "sessions"]).tag(&format!("close:{sb}\n{t}")),
    )
    .line(format!("The session stops and its worktree is removed: {wt}"));
    if dirty > 0 {
        c = c.line(format!("It has {dirty} uncommitted file{}: closing loses {}.", if dirty == 1 { "" } else { "s" }, if dirty == 1 { "it" } else { "them" }));
        c = c.ack(&format!("Discard {dirty} uncommitted file{} in it", if dirty == 1 { "" } else { "s" }));
    }
    if has_branch {
        c = c.line(if delete_branch {
            format!("Branch {branch} is deleted too (nothing in it that {base} lacks).")
        } else {
            format!("Branch {branch} is kept: its {ahead} commit{} not in {base} stay there.", if ahead == 1 { " is" } else { "s are" })
        });
    }
    Ok(c.labels("Close thread", "Cancel").danger())
}

// ---------------------------------------------------------------------------
// Answers

fn refusal(r: &Result<(u16, J), String>) -> Option<String> {
    match r {
        Err(e) => Some(e.clone()),
        Ok((s, v)) if !(200..300).contains(s) || v["ok"] == false => Some(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {s}"))),
        _ => None,
    }
}

/// Opens a pane for each of the plan's threads while the window has room
/// (plans.js `openPlanPanes`): the empty panes, then new ones up to 4. A
/// thread that does not fit still runs; the panel lists it.
fn open_plan_panes(ctx: &Ctx, plan: &J) -> Vec<Action> {
    let panes = ctx.data["app"]["panes"].as_array().cloned().unwrap_or_default();
    let shown = ctx.data["app"]["layout"]["panes"].as_u64().unwrap_or(panes.len() as u64);
    let mut room = panes.iter().filter(|p| p["sandbox"].is_null()).count() as u64 + 4u64.saturating_sub(shown.max(panes.len() as u64));
    let sb = plan["sandbox"].as_str().unwrap_or("");
    let mut out = vec![];
    for t in plan["threads"].as_array().into_iter().flatten() {
        let Some(name) = t["name"].as_str() else { continue };
        if pane_of(ctx, sb, Some(name)).is_some() {
            continue;
        }
        if room == 0 {
            break;
        }
        room -= 1;
        out.push(Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": sb, "thread": name, "pane": "spare" }) });
    }
    out
}

fn on_answer(ctx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let (kind, rest) = call.tag.split_once(':').unwrap_or((call.tag.as_str(), ""));
    let err = refusal(&r);
    let body = r.as_ref().map(|(_, v)| v.clone()).unwrap_or(J::Null);
    let mut u = ui(ctx);
    match kind {
        "approve" => match err {
            Some(e) => {
                let sb = model::plan(ctx.data, rest).and_then(|p| p["sandbox"].as_str()).unwrap_or("").to_string();
                let text = model::explain_refusal(ctx.data, &sb, &e);
                obj(&mut u, "plan_error").insert(rest.into(), json!(text));
                vec![set_ui(u), Action::OpenModal { screen: "plans".into(), arg: json!(rest) }, Action::error(text)]
            }
            None => {
                obj(&mut u, "plan_error").remove(rest);
                obj(&mut u, "drafts").remove(rest);
                let plan = &body["plan"];
                let n = plan["threads"].as_array().map(|a| a.len()).unwrap_or(0);
                let mut out = vec![];
                let opened = u["opened"].as_array().cloned().unwrap_or_default();
                if !opened.iter().any(|x| x == rest) {
                    out = open_plan_panes(ctx, plan);
                    let mut o = opened;
                    o.push(json!(rest));
                    u["opened"] = J::Array(o);
                }
                let mut acts = vec![set_ui(u), Action::CloseModal, Action::notice(format!("Plan approved: {n} thread{} starting", if n == 1 { "" } else { "s" }))];
                acts.extend(out);
                acts
            }
        },
        "merge" => {
            let sb = model::plan(ctx.data, rest).and_then(|p| p["sandbox"].as_str()).unwrap_or("").to_string();
            match err {
                Some(e) => {
                    let text = format!("Merge refused: {e}");
                    obj(&mut u, "plan_error").insert(rest.into(), json!(text));
                    vec![set_ui(u), Action::error(text)]
                }
                None => {
                    obj(&mut u, "plan_error").remove(rest);
                    let m = &body["merge"];
                    let mut acts = vec![];
                    let close = u["merge_close"][rest].as_bool().unwrap_or(true);
                    let order: Vec<String> = m["order"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
                    let msg = match m["state"].as_str() {
                        Some("merged") => format!("Merged the threads of {sb}"),
                        _ => {
                            let (_, outcome) = model::merge_rows(&json!({ "merge": m, "threads": [] }));
                            format!("Merge of {sb} stopped: {outcome}")
                        }
                    };
                    if close && m["state"] == "merged" {
                        // Which threads are still there once closed: read again now.
                        obj(&mut u, "merge_check").insert(rest.into(), json!({ "sandbox": sb, "threads": order }));
                        acts.push(Action::Call(Call::get(format!("{}/threads", sb_path(&sb)), "read the threads after the merge").tag(&format!("merge-check:{rest}"))));
                    }
                    let tone = if m["state"] == "merged" { Action::notice(msg) } else { Action::error(msg) };
                    let mut out = vec![set_ui(u), tone];
                    out.extend(acts);
                    out
                }
            }
        }
        "merge-check" => {
            let check = u["merge_check"][rest].clone();
            obj(&mut u, "merge_check").remove(rest);
            let listed: Vec<&str> = body["threads"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).collect();
            let left: Vec<String> = check["threads"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).filter(|t| listed.contains(t)).map(str::to_string).collect();
            // The panes of the threads closed go, as after a Close.
            let sb = check["sandbox"].as_str().unwrap_or("").to_string();
            let closed: Vec<Action> = check["threads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str())
                .filter(|t| !listed.contains(t))
                .map(|t| Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": sb, "thread": t }) })
                .chain([Action::Refresh("threads"), Action::Refresh("sessions")])
                .collect();
            if left.is_empty() {
                obj(&mut u, "merge_note").insert(rest.into(), json!("Every merged thread was closed."));
                let mut out = vec![set_ui(u)];
                out.extend(closed);
                out
            } else {
                let text = format!(
                    "Not closed: {}. {} still listed after the merge; the daemon gave the reason to the lead's session. Close {} from the Threads panel to see why.",
                    left.join(", "),
                    if left.len() == 1 { "It is" } else { "They are" },
                    if left.len() == 1 { "it" } else { "them" }
                );
                obj(&mut u, "merge_note").insert(rest.into(), json!(text));
                let mut out = vec![set_ui(u), Action::error(text)];
                out.extend(closed);
                out
            }
        }
        "close" => {
            let (sb, t) = rest.split_once('\n').unwrap_or((rest, ""));
            match err {
                Some(e) => vec![Action::error(format!("{t} of {sb} was not closed: {e}"))],
                // Its panes go, as panes.js `afterThreadClosed` does.
                None => vec![Action::notice(format!("Closed {t} of {sb}")), Action::Intent { name: "close_panes".into(), args: json!({ "sandbox": sb, "thread": t }) }],
            }
        }
        "refuse" => match err {
            Some(e) => vec![Action::error(format!("Could not refuse the plan: {e}"))],
            None => {
                obj(&mut u, "drafts").remove(rest);
                vec![set_ui(u), Action::CloseModal, Action::notice("Plan refused; the lead is told")]
            }
        },
        "rc-look" => {
            let (sb, t) = rest.split_once('\n').unwrap_or((rest, ""));
            let t = Some(t).filter(|t| !t.is_empty());
            if let Some(e) = err {
                return vec![Action::error(format!("Remote control: {e}"))];
            }
            obj(&mut u, "remote").insert(rest.into(), json!({ "login": body["login"], "active": body["active"] }));
            let tail = vec![set_ui(u)];
            let mut more = match (body["active"] == true, body["login"].as_str()) {
                (true, _) => vec![Action::notice(format!("Remote control is on for {}", who(sb, t)))],
                (_, Some("helper")) => vec![Action::error(body["advice"].as_str().map(str::to_string).unwrap_or_else(|| "Remote control: an apiKeyHelper outranks the login; recreate this sandbox on sbx 0.50+".into()))],
                (_, Some("missing")) => vec![Action::error("Remote control: not logged in to claude.ai. Run /login in the session, then choose Remote control again.")],
                (_, Some("token")) => vec![Action::Notice {
                    text: format!("Remote control: {sb} is signed in with a setup token, which Remote Control refuses. Click to switch it to a claude.ai login."),
                    error: true,
                    then: Some(("remote_login".into(), json!({ "sandbox": sb, "thread": t }))),
                }],
                _ => vec![Action::Call(
                    Call::post(format!("{}/remote-control", sb_path(sb)), json!({ "thread": t }), format!("Remote control for {}", who(sb, t))).tag(&format!("rc-start:{rest}")),
                )],
            };
            more.splice(0..0, tail);
            more
        }
        "rc-start" => match err {
            Some(e) => vec![Action::error(format!("Remote control: {e}"))],
            None => vec![Action::notice("Remote control starting: it shows on once Claude has connected")],
        },
        "rc-login" => match err {
            Some(e) => vec![Action::error(format!("Remote control: {e}"))],
            None => vec![Action::notice("Finish the login in the session, then choose Remote control again")],
        },
        "lead" => match err {
            Some(e) => vec![Action::error(format!("Team mode for {rest}: {e}"))],
            None => vec![Action::notice(format!("Team mode on for {rest}: its main session plans the work, you approve the plan"))],
        },
        "interrupt" => match err {
            Some(e) => vec![Action::error(format!("{}: {e}", call.what))],
            None => vec![],
        },
        _ => match err {
            Some(e) => vec![Action::error(format!("{}: {e}", call.what))],
            None if call.notify => vec![Action::notice(format!("{}: done", call.what))],
            None => vec![],
        },
    }
}

/// A plan arrives: its dialog opens (if none is up); a plan approved
/// elsewhere opens its panes; a merge proposed says so.
fn on_data(ctx: &Ctx, key: &str, old: &J, new: &J) -> Vec<Action> {
    let mut out = vec![Action::Set { key: VIEW.into(), value: view(ctx) }];
    if key == "plans" {
        out.extend(plans_changed(ctx, old, new));
    }
    out
}

/// `data.threads_view`: every sandbox's sessions as [`model::sessions`]
/// says, with the ceiling (`open`, `max`), whether it leads, and its plans
/// (pending, merge) by id, so screens draw what the extras say.
pub fn view(ctx: &Ctx) -> J {
    let mut v = Map::new();
    for s in ctx.get("sandboxes").as_array().into_iter().flatten() {
        let Some(sb) = s["name"].as_str() else { continue };
        let t = &ctx.data["threads"][sb];
        v.insert(
            sb.into(),
            json!({
                "sessions": model::sessions(ctx.data, sb).iter().map(Session::to_json).collect::<Vec<_>>(),
                "open": t["open"], "max": t["max"], "leading": model::leading(ctx.data, sb),
                "pending": model::pending_plan(ctx.data, sb).map(|p| p["id"].clone()),
                "merge": model::merge_plan(ctx.data, sb).map(|p| p["id"].clone()),
            }),
        );
    }
    J::Object(v)
}

pub const VIEW: &str = "threads_view";

fn plans_changed(ctx: &Ctx, old: &J, new: &J) -> Vec<Action> {
    let seed = old.is_null();
    let mut u = ui(ctx);
    let mut out = vec![];
    let mut changed = false;
    let mut modal_up = !ctx.data["app"]["modal"].is_null();
    for p in new.as_array().into_iter().flatten() {
        let Some(id) = p["id"].as_str() else { continue };
        let prev = old.as_array().and_then(|a| a.iter().find(|x| x["id"] == id));
        let sb = p["sandbox"].as_str().unwrap_or("");
        if p["state"] == "pending" && p["closed"] != true && prev.is_none() {
            if !u["drafts"][id].is_object() {
                obj(&mut u, "drafts").insert(id.into(), model::draft_of(p));
                changed = true;
            }
            if !modal_up {
                modal_up = true;
                u["repos_for"] = json!(sb);
                changed = true;
                out.push(Action::Set { key: "thread_repos".into(), value: J::Null });
                out.push(Action::Fetch { key: "thread_repos".into(), path: format!("{}/repos", sb_path(sb)) });
                out.push(Action::OpenModal { screen: "plans".into(), arg: json!(id) });
            }
        }
        let was_pending = prev.is_some_and(|x| x["state"] == "pending");
        if !seed && p["state"] == "approved" && was_pending && !u["opened"].as_array().is_some_and(|o| o.iter().any(|x| x == id)) {
            out.extend(open_plan_panes(ctx, p));
            let mut o = u["opened"].as_array().cloned().unwrap_or_default();
            o.push(json!(id));
            u["opened"] = J::Array(o);
            changed = true;
        }
        if !seed && p["merge"]["state"] == "proposed" && prev.is_some_and(|x| x["merge"].is_null()) {
            out.push(Action::Notice { text: format!("The lead of {sb} proposes merging its threads"), error: false, then: Some(("plan_review".into(), json!({ "id": id }))) });
        }
    }
    if changed {
        out.insert(0, set_ui(u));
    }
    out
}

// ---------------------------------------------------------------------------
// Extras

fn extra(place: Place, id: &str, label: &str, icon: Option<&str>, tone: &str, help: &str, intent: Option<(&str, J)>) -> Extra {
    Extra { place, id: id.into(), label: label.into(), icon: icon.map(str::to_string), tone: tone.into(), help: help.into(), count: None, intent: intent.map(|(n, a)| (n.to_string(), a)) }
}

fn extras(ctx: &Ctx) -> Vec<Extra> {
    let mut out = vec![];
    let u = ui(ctx);
    let mut waiting = vec![];
    for s in ctx.get("sandboxes").as_array().into_iter().flatten() {
        let Some(sb) = s["name"].as_str() else { continue };
        let up = s["status"] == "running";
        let list: Vec<Session> = model::sessions(ctx.data, sb);
        let threads = list.len() - 1;
        // Rows under the sandbox: once there is a second session to tell apart.
        if threads > 0 {
            for x in &list {
                let id = x.thread.clone().unwrap_or_else(|| "main".into());
                let glyph = match (&x.thread, x.kind.as_str()) {
                    (None, _) => "✦",
                    (_, "same") => "✦",
                    _ => "⑂",
                };
                let word = x.word();
                let label = if word.is_empty() { format!("{glyph} {}", x.label()) } else { format!("{glyph} {} · {word}", x.label()) };
                let help = match (&x.thread, pane_of(ctx, sb, x.thread.as_deref())) {
                    (_, Some(p)) => format!("In pane {p}"),
                    (None, None) => "Show the main session".into(),
                    (Some(t), None) if x.running => format!("Show {t}"),
                    (Some(t), None) if x.kind == "same" => format!("Start {t} again: a fresh conversation in the main checkout"),
                    (Some(t), None) => format!("Start {t} again: its worktree is kept, its session had stopped"),
                };
                let mut args = json!({ "sandbox": sb, "pane": "focused" });
                if let Some(t) = &x.thread {
                    args["thread"] = json!(t);
                }
                let _ = &help;
                let mut e = extra(on_rows(sb), &id, &label, None, x.tone(), "", Some(("open_terminal", args)));
                if let Some(p) = pane_of(ctx, sb, x.thread.as_deref()) {
                    e.count = Some(p);
                }
                out.push(e);
            }
        }
        let wants = list.iter().filter(|x| x.state == "wants").count();
        let mut e = extra(
            Place::Sandbox(sb.into()),
            "threads",
            "⑂",
            None,
            if wants > 0 { "amber" } else { "muted" },
            if wants > 0 { "A session waits for you: open the Threads panel" } else { "Threads, plans and worktrees of this sandbox" },
            Some(("threads_show", json!({ "sandbox": sb }))),
        );
        if threads > 0 {
            e.count = Some(threads as u64);
        }
        out.push(e);
        if let Some(p) = model::pending_plan(ctx.data, sb) {
            let id = p["id"].as_str().unwrap_or("");
            waiting.push(id.to_string());
            out.push(extra(on_session(sb, None), "plan", "Review plan", Some("flag"), "amber", "This lead's plan waits for you", Some(("plan_review", json!({ "id": id })))));
        }
        if let Some(p) = model::merge_plan(ctx.data, sb) {
            let label = match p["merge"]["state"].as_str() {
                Some("merging") => "Merging…",
                Some("failed") => "Retry merge",
                _ => "Merge",
            };
            out.push(extra(on_session(sb, None), "merge", label, Some("check"), "green", "This lead's threads are reviewed and ready to merge", Some(("plan_review", json!({ "id": p["id"] })))));
        }
        if !up {
            continue;
        }
        // The main session's pane bar.
        let lead = model::leading(ctx.data, sb);
        out.push(if lead {
            extra(on_session(sb, None), "lead", "Team on", Some("sparkle"), "purple", "Team mode: this session plans threads and follows them. Click to end it", Some(("lead_end", json!({ "sandbox": sb }))))
        } else {
            extra(on_session(sb, None), "lead", "Team", Some("sparkle"), "muted", "Team mode: make this session plan the work into threads, have you approve the plan, and follow them", Some(("lead_start", json!({ "sandbox": sb }))))
        });
        // Each session's own controls.
        for x in list.iter().filter(|x| x.running || x.thread.is_none()) {
            let t = x.thread.as_deref();
            let mut args = json!({ "sandbox": sb });
            if let Some(t) = t {
                args["thread"] = json!(t);
            }
            if let Some(m) = &x.model {
                let label = ctx.get("models")["models"].as_array().and_then(|l| l.iter().find(|y| y["id"] == m.as_str())).and_then(|y| y["label"].as_str()).unwrap_or(m);
                out.push(extra(on_session(sb, t), "model", label, None, "muted", &format!("This session runs {m}"), None));
            }
            if t.is_some() && !x.word().is_empty() {
                let help = if x.summary.is_empty() { format!("{}: {}", x.label(), x.word()) } else { x.summary.clone() };
                out.push(extra(on_session(sb, t), "state", x.word(), None, x.tone(), &help, None));
            }
            out.push(extra(on_session(sb, t), "interrupt", "Esc", Some("stop"), "muted", "Interrupt: sends Escape, Claude Code stops what it is doing", Some(("session_interrupt", args.clone()))));
            let on = u["remote"][skey(sb, t)]["active"] == true;
            out.push(extra(
                on_session(sb, t),
                "remote",
                if on { "Remote on" } else { "Remote" },
                Some("link"),
                if on { "green" } else { "muted" },
                if on { "Reachable from claude.ai: turn off with /remote-control in the session" } else { "Remote control: drive this session from claude.ai/code or the Claude app" },
                Some(("remote_control", args.clone())),
            ));
            if let Some(t) = t {
                let (label, help) = if x.kind == "same" { ("Close session", "Close this session: stop it; the main checkout stays as it is") } else { ("Close thread", "Close this thread: stop it, remove its worktree, and its branch") };
                out.push(extra(on_session(sb, Some(t)), "close", label, Some("close"), "red", help, Some(("thread_close", json!({ "sandbox": sb, "thread": t })))));
            }
        }
        out.push(extra(on_session(sb, None), "threads", "Threads", Some("terminal"), "muted", "Every session of this sandbox; open a new thread", Some(("threads_show", json!({ "sandbox": sb })))));
    }
    if let Some(id) = waiting.first() {
        let mut e = extra(Place::Topbar, "plan", "Plan waiting", Some("flag"), "amber", "A lead's plan waits for your approval", Some(("plan_review", json!({ "id": id }))));
        if waiting.len() > 1 {
            e.count = Some(waiting.len() as u64);
        }
        out.push(e);
    }
    out
}
