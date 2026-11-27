//! The stand-in daemon's thread, plan and lead routes (`src/bin/standin.rs`
//! and fixture daemons): answers from the fixture's `daemon` object, changed
//! as `crates/daemon/src/svc/plans.rs` would change it, with the daemon's
//! own refusals word for word (the session ceiling, claims at a shared
//! merge, uncommitted files at a close).
//!
//! Fixture keys, each one route's answer: `sessions`, `threads` (by
//! sandbox), `plans`, `leads`, `models`, `board` (by sandbox), plus:
//! * `repos_of` / `worktrees_of`: sandbox → `GET .../repos` / `.../worktrees`;
//! * `thread_status_of`: sandbox → thread → `GET .../threads/:t`'s `status`
//!   (default: a clean worktree with its branch);
//! * `remote_of`: `"sandbox/thread"` → `{login, active}` (default `full`, off);
//! * `standin_lead_plan`: a plan (`summary`, `threads`, `variant`,
//!   `justification`) the lead proposes, pending, when Team mode is turned on;
//! * `standin_plan_steps`: true: each `GET /api/plans` moves an approved
//!   plan one step (its threads starting, working, done with a report), then
//!   the lead proposes the merge (`standin_merge_note`); one step per
//!   `standin_plan_step_ms` at most (default 1200, 0 in tests);
//! * `standin_merge_fail`: the thread whose merge stops at a conflict, once
//!   (the retry after it merges);
//! * `standin_close_refuse`: threads a merge cannot close (their worktree is busy).
//! * `standin_ci_fail`: a CI plan's run fails, once: `"conflict"` (its last
//!   thread conflicts while the CI branch is built) or `"tests"` (the
//!   battery fails twice on the tip).
//!
//! A CI plan (split, with `approved_tests`) is merged as the daemon merges it
//! (`crates/core/src/plans/ci.rs`): the proposal is `ci_building`, the next
//! steps `ci_testing`, then `merged_ci` or `ci_failed`; a click in `merged_ci`
//! fast-forwards to `merged`, a click in any other CI state is refused with
//! `why_not`. Every plan the stand-in changes carries `merge_status` as the
//! daemon adds it ([`merge_status`], ported word for word).
//!
//! Abandon merge (`/api/plans/{id}/merge/abandon`) answers as the daemon:
//! a merge proposed or failed becomes abandoned; anything else is refused.

use crate::feat::{match_path, Standin};
use serde_json::{json, Value as J};

fn ok(v: J) -> Option<(u16, J)> {
    let mut b = json!({ "ok": true });
    if let (Some(o), J::Object(x)) = (b.as_object_mut(), v) {
        o.extend(x);
    }
    Some((200, b))
}

fn refuse(code: u16, msg: impl Into<String>) -> Option<(u16, J)> {
    Some((code, json!({ "ok": false, "error": msg.into() })))
}

use crate::feat::now_ms;

fn view<'a>(data: &'a mut J, sb: &str) -> &'a mut J {
    if !data["threads"].is_object() {
        data["threads"] = json!({});
    }
    let v = &mut data["threads"][sb];
    if !v.is_object() {
        *v = json!({ "ok": true, "threads": [], "open": 1, "max": 5 });
    }
    if !v["threads"].is_array() {
        v["threads"] = json!([]);
    }
    v
}

fn plans(data: &mut J) -> &mut Vec<J> {
    if !data["plans"].is_array() {
        data["plans"] = json!([]);
    }
    data["plans"].as_array_mut().unwrap()
}

fn plan_index(data: &J, id: &str) -> Option<usize> {
    data["plans"].as_array()?.iter().position(|p| p["id"] == id)
}

/// Starts a thread's session, as `launch_thread` does: listed open, counted.
fn launch(data: &mut J, sb: &str, name: &str, kind: &str, model: Option<&str>, repo: Option<&str>) {
    let v = view(data, sb);
    let list = v["threads"].as_array_mut().unwrap();
    match list.iter_mut().find(|t| t["name"] == name) {
        Some(t) if t["open"] == true => return,
        Some(t) => t["open"] = json!(true),
        None => list.push(json!({ "name": name, "open": true, "repo": repo.unwrap_or(""), "model": model, "worktree": kind })),
    }
    v["open"] = json!(v["open"].as_u64().unwrap_or(1) + 1);
}

fn forget(data: &mut J, sb: &str, name: &str) {
    let v = view(data, sb);
    let list = v["threads"].as_array_mut().unwrap();
    if let Some(i) = list.iter().position(|t| t["name"] == name) {
        let t = list.remove(i);
        if t["open"] == true {
            v["open"] = json!(v["open"].as_u64().unwrap_or(1).saturating_sub(1).max(1));
        }
    }
    if let Some(s) = data["sessions"].as_array_mut() {
        s.retain(|i| !(i["sandbox"] == sb && i["thread"] == name));
    }
}

pub fn answer(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let p = path.split('?').next().unwrap_or(path);
    let cap = |pat: &str| match_path(pat, p).map(|c| c.into_iter().map(|(_, v)| v).collect::<Vec<_>>());
    match method {
        // The stand-in's /ws opened a thread's session (`src/bin/standin.rs`):
        // recorded as the daemon does on its first open, with its repo,
        // model and kind.
        "OPEN" => {
            let q = |k: &str| crate::feat::query(path, k);
            let (Some(sb), Some(t)) = (q("sandbox"), q("thread")) else { return Some((400, J::Null)) };
            let kind = if q("checkout").as_deref() == Some("1") { "same" } else { "own" };
            launch(st.data, &sb, &t, kind, q("model").as_deref(), q("repo").as_deref());
            if !st.data["sessions"].is_array() {
                st.data["sessions"] = json!([]);
            }
            let info = json!({ "sandbox": sb, "mode": "claude", "state": "idle", "agent": "claude", "started_ms": now_ms(), "thread": t, "origin": "tty", "answerable": false, "ts": now_ms() });
            st.data["sessions"].as_array_mut().unwrap().push(info.clone());
            (st.publish)("session", info);
            Some((200, J::Null))
        }
        "GET" => {
            if p == "/api/plans" && st.data["standin_plan_steps"] == true {
                step_plans(st);
                return Some((200, st.data["plans"].clone()));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/repos") {
                let v = st.data["repos_of"][&c[0]].clone();
                return Some((200, if v.is_object() { v } else { json!({ "ok": true, "workspace": "/work", "repos": [{ "path": "", "branch": "main", "dirty": 0, "ahead": 0, "behind": 0, "detached": false }] }) }));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/worktrees") {
                let v = st.data["worktrees_of"][&c[0]].clone();
                return Some(if v.is_object() { (if v["ok"] == false { 404 } else { 200 }, v) } else { (200, json!({ "ok": true, "workspace": "/work", "repos": [] })) });
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/remote-control") {
                let t = crate::feat::query(path, "thread").unwrap_or_default();
                let v = &st.data["remote_of"][format!("{}/{t}", c[0])];
                return ok(json!({ "login": v["login"].as_str().unwrap_or("full"), "active": v["active"] == true, "advice": null }));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/threads/{thread}") {
                let (sb, t) = (&c[0], &c[1]);
                let kind = st.data["threads"][sb]["threads"].as_array().and_then(|a| a.iter().find(|x| x["name"] == t.as_str())).map(|x| x["worktree"].as_str().unwrap_or("own").to_string());
                let Some(kind) = kind else { return refuse(404, format!("no thread '{t}' of '{sb}'")) };
                if kind == "same" {
                    return ok(json!({ "status": { "worktree": "/work", "branch": "", "base": "", "has_branch": false, "dirty": 0, "ahead": 0, "same_worktree": true } }));
                }
                if kind == "team" {
                    let plan = st.data["plans"].as_array().and_then(|a| a.iter().find(|p| p["sandbox"] == sb.as_str() && p["threads"].as_array().is_some_and(|x| x.iter().any(|x| x["name"] == t.as_str())))).cloned().unwrap_or(J::Null);
                    let last = plan["threads"].as_array().is_some_and(|a| a.len() == 1);
                    return ok(json!({ "status": { "worktree": format!("/work/.sbxw-worktrees/team-{}", plan["id"].as_str().unwrap_or("")), "branch": "", "base": "", "has_branch": false, "dirty": 0, "ahead": 0, "team": plan["id"], "last": last } }));
                }
                let s = st.data["thread_status_of"][sb][t].clone();
                let mut d = json!({ "worktree": format!("/work/.sbxw-worktrees/{t}"), "branch": format!("sbxw/{t}"), "base": "main", "has_branch": true, "dirty": 0, "ahead": 0 });
                if let J::Object(o) = s {
                    d.as_object_mut().unwrap().extend(o);
                }
                return ok(json!({ "status": d }));
            }
            // A per-sandbox answer holding `{ok: false, error}` is served as the
            // daemon serves a failure, so the core files it under `data.app.errors`.
            for (pat, key) in [("/api/sandboxes/{sandbox}/threads", "threads"), ("/api/sandboxes/{sandbox}/board", "board")] {
                if let Some(c) = cap(pat) {
                    let v = &st.data[key][&c[0]];
                    if v["ok"] == false {
                        return Some((503, v.clone()));
                    }
                }
            }
            None
        }
        "POST" => {
            if let Some(c) = cap("/api/plans/{id}/approve") {
                return approve(st, &c[0], body);
            }
            if let Some(c) = cap("/api/plans/{id}/refuse") {
                let i = plan_index(st.data, &c[0])?;
                let p = &mut plans(st.data)[i];
                if p["state"] != "pending" {
                    return refuse(400, format!("plan {} is {}, not pending", c[0], p["state"].as_str().unwrap_or("?")));
                }
                p["state"] = json!("refused");
                if let Some(n) = body["note"].as_str() {
                    p["note"] = json!(n);
                }
                p["updated_ms"] = json!(now_ms());
                let p = p.clone();
                (st.publish)("plan", p.clone());
                return ok(json!({ "plan": p }));
            }
            if let Some(c) = cap("/api/plans/{id}/merge/abandon") {
                return abandon(st, &c[0]);
            }
            if let Some(c) = cap("/api/plans/{id}/merge") {
                return merge(st, &c[0], body["close_threads"] == true);
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/threads/{thread}/close") {
                let (sb, t) = (c[0].clone(), c[1].clone());
                if !st.data["threads"][&sb]["threads"].as_array().is_some_and(|a| a.iter().any(|x| x["name"] == t.as_str())) {
                    return refuse(404, format!("no thread '{t}' of '{sb}'"));
                }
                let dirty = st.data["thread_status_of"][&sb][&t]["dirty"].as_u64().unwrap_or(0);
                if dirty > 0 && body["discard"] != true {
                    // The daemon's words (`sbxw-thread-end`, crates/sbx/src/scripts.rs).
                    return refuse(500, format!("/work/.sbxw-worktrees/{t} has {dirty} uncommitted file(s)"));
                }
                forget(st.data, &sb, &t);
                (st.publish)("session", json!({ "sandbox": sb, "thread": t, "state": "exited", "mode": "claude" }));
                return ok(json!({}));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/lead") {
                let sb = c[0].clone();
                if !st.data["leads"].is_array() {
                    st.data["leads"] = json!([]);
                }
                let state = st.data["standin_lead_state"].as_str().unwrap_or("leading").to_string();
                let leads = st.data["leads"].as_array_mut().unwrap();
                // `standin_lead_state`: the state the lead stays in
                // (`waiting`, `activating`), to see the pill's in-between
                // states; none: it leads at once, as a confirmed lead.
                if state == "leading" && !leads.iter().any(|l| l == sb.as_str()) {
                    leads.push(json!(sb));
                }
                set_lead_state(st.data, &sb, Some(&state));
                (st.publish)("lead", json!(sb));
                if let J::Object(proto) = st.data["standin_lead_plan"].clone() {
                    let n = plans(st.data).len() + 1;
                    let mut p = json!({ "id": format!("p-standin-{n}"), "sandbox": sb, "summary": "", "threads": [], "state": "pending", "variant": "split", "justification": "", "lead": format!("lead of {sb}"), "created_ms": now_ms(), "updated_ms": now_ms() });
                    p.as_object_mut().unwrap().extend(proto);
                    plans(st.data).push(p.clone());
                    // The lead takes a moment to read the work and propose.
                    st.publish_after(900, "plan", p);
                }
                return ok(json!({}));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/lead/cancel") {
                let sb = c[0].clone();
                if !matches!(st.data["lead_states"][&sb]["state"].as_str(), Some("waiting" | "activating")) {
                    return refuse(409, format!("{sb} is not waiting to lead"));
                }
                set_lead_state(st.data, &sb, None);
                (st.publish)("lead-end", json!(sb));
                return ok(json!({}));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/lead/end") {
                let sb = c[0].clone();
                if let Some(l) = st.data["leads"].as_array_mut() {
                    l.retain(|x| x != sb.as_str());
                }
                set_lead_state(st.data, &sb, None);
                let mut ended = vec![];
                for p in plans(st.data).iter_mut().filter(|p| p["sandbox"] == sb.as_str() && p["closed"] != true) {
                    if p["state"] == "pending" {
                        p["state"] = json!("refused");
                    }
                    p["closed"] = json!(true);
                    ended.push(p.clone());
                }
                for p in ended {
                    (st.publish)("plan", p);
                }
                (st.publish)("lead-end", json!(sb));
                return ok(json!({}));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/remote-control") {
                let k = format!("{}/{}", c[0], body["thread"].as_str().unwrap_or(""));
                if !st.data["remote_of"].is_object() {
                    st.data["remote_of"] = json!({});
                }
                let v = &mut st.data["remote_of"][&k];
                if v["login"].as_str().is_some_and(|l| l != "full") {
                    return refuse(409, "Remote Control needs a claude.ai login (/login) in the session");
                }
                *v = json!({ "login": "full", "active": true });
                return ok(json!({}));
            }
            if let Some(c) = cap("/api/sandboxes/{sandbox}/remote-control/login") {
                let k = format!("{}/{}", c[0], body["thread"].as_str().unwrap_or(""));
                if !st.data["remote_of"].is_object() {
                    st.data["remote_of"] = json!({});
                }
                st.data["remote_of"][&k] = json!({ "login": "full", "active": false });
                return ok(json!({}));
            }
            if p == "/api/input" {
                if !st.data["standin_inputs"].is_array() {
                    st.data["standin_inputs"] = json!([]);
                }
                st.data["standin_inputs"].as_array_mut().unwrap().push(body.clone());
                return Some((200, J::Null));
            }
            None
        }
        _ => None,
    }
}

fn approve(st: &mut Standin, id: &str, body: &J) -> Option<(u16, J)> {
    let Some(i) = plan_index(st.data, id) else { return refuse(400, format!("no plan '{id}'")) };
    let p = st.data["plans"][i].clone();
    if p["state"] != "pending" {
        return refuse(400, format!("plan {id} is {}, not pending", p["state"].as_str().unwrap_or("?")));
    }
    let sb = p["sandbox"].as_str().unwrap_or("").to_string();
    let threads: Vec<J> = body["threads"].as_array().cloned().unwrap_or_default();
    if threads.is_empty() {
        return refuse(400, "a plan needs at least one thread");
    }
    let v = view(st.data, &sb).clone();
    let open = v["open"].as_u64().unwrap_or(1);
    let max = v["max"].as_u64().unwrap_or(5).max(1);
    let new = threads.iter().filter(|t| !v["threads"].as_array().is_some_and(|a| a.iter().any(|x| x["name"] == t["name"] && x["open"] == true))).count() as u64;
    if open + new > max {
        // `svc/plans.rs`, word for word.
        return refuse(
            400,
            format!("{new} new thread(s) and {open} session(s) already open is more than max_agent_sessions = {max}. Remove threads, or raise the ceiling in sbxw.toml."),
        );
    }
    let variant = body["variant"].as_str().unwrap_or(p["variant"].as_str().unwrap_or("split")).to_string();
    let now = now_ms();
    let rows: Vec<J> = threads
        .iter()
        .map(|t| {
            let mut r = t.clone();
            r["state"] = json!("starting");
            r["summary"] = json!("");
            r["updated_ms"] = json!(now);
            r
        })
        .collect();
    {
        let pl = &mut plans(st.data)[i];
        pl["threads"] = J::Array(rows.clone());
        pl["variant"] = json!(variant);
        pl["state"] = json!("approved");
        pl["updated_ms"] = json!(now);
        // The proposed test command is approved with the plan, unless cleared.
        if pl["proposed_tests"]["command"].is_string() && body["clear_tests"] != true {
            pl["approved_tests"] = pl["proposed_tests"].clone();
        }
        if variant == "shared" {
            pl["worktree"] = json!(format!("/work/.sbxw-worktrees/team-{id}"));
        }
    }
    for t in &rows {
        let kind = if variant == "shared" { "team" } else { "own" };
        launch(st.data, &sb, t["name"].as_str().unwrap_or(""), kind, t["model"].as_str(), t["repo"].as_str());
    }
    let plan = st.data["plans"][i].clone();
    (st.publish)("plan", plan.clone());
    (st.publish)("session", json!({ "sandbox": sb, "mode": "claude", "state": "working" }));
    ok(json!({ "plan": plan }))
}

/// One step of every approved plan per read: threads starting, then
/// working, then done with a report; then the lead's merge proposal.
fn step_plans(st: &mut Standin) {
    // At most one step per `standin_plan_step_ms` (default 1200): the reads a
    // plan event itself causes do not run the whole plan at once.
    let every = st.data["standin_plan_step_ms"].as_u64().unwrap_or(1200);
    let now = now_ms();
    if now.saturating_sub(st.data["standin_last_step"].as_u64().unwrap_or(0)) < every {
        return;
    }
    st.data["standin_last_step"] = json!(now);
    let note = st.data["standin_merge_note"].as_str().unwrap_or("api first: web depends on its routes").to_string();
    let mut changed = vec![];
    for p in plans(st.data).iter_mut().filter(|p| p["state"] == "approved" && p["closed"] != true && p["merge"].is_null()) {
        let list = p["threads"].as_array_mut().unwrap();
        let next = |s: &str| match s {
            "starting" => Some("working"),
            "working" => Some("done"),
            _ => None,
        };
        let mut moved = false;
        for t in list.iter_mut() {
            if let Some(n) = next(t["state"].as_str().unwrap_or("")) {
                t["state"] = json!(n);
                if n == "done" {
                    t["summary"] = json!(format!("{} done: changes committed on sbxw/{}, tests pass", t["name"].as_str().unwrap_or(""), t["name"].as_str().unwrap_or("")));
                } else {
                    t["summary"] = json!("working on it");
                }
                moved = true;
            }
        }
        if !moved {
            let order: Vec<J> = list.iter().map(|t| t["name"].clone()).collect();
            p["merge"] = json!({ "order": order, "note": note, "state": "proposed", "base": null, "results": [] });
            if p["variant"] != "shared" && p["approved_tests"]["command"].is_string() {
                let id = p["id"].as_str().unwrap_or("").to_string();
                p["merge"]["state"] = json!("ci_building");
                p["merge"]["ci"] = json!({ "branch": format!("sbxw/ci-{id}"), "worktree": format!(".sbxw-worktrees/.ci-{id}"), "run": 1, "builds": 1 });
            }
        }
        restatus(p);
        p["updated_ms"] = json!(now_ms());
        changed.push(p.clone());
    }
    let fail = st.data["standin_ci_fail"].as_str().map(str::to_string);
    let mut failed_once = false;
    for p in plans(st.data).iter_mut().filter(|p| p["state"] == "approved" && p["closed"] != true && p["merge"]["ci"].is_object()) {
        if !step_ci(p, fail.as_deref()) {
            continue;
        }
        failed_once |= p["merge"]["state"] == "ci_failed";
        restatus(p);
        p["updated_ms"] = json!(now_ms());
        changed.push(p.clone());
    }
    if failed_once {
        st.data["standin_ci_fail"] = J::Null;
    }
    for p in changed {
        (st.publish)("plan", p);
    }
}

/// One step of a CI run, as the daemon's build and test queue: built (or a
/// conflict), then tested (passed, or failed twice). False: nothing moved.
fn step_ci(p: &mut J, fail: Option<&str>) -> bool {
    let order: Vec<String> = p["merge"]["order"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
    let m = &mut p["merge"];
    let ci = &mut m["ci"];
    let run = ci["run"].as_u64().unwrap_or(1);
    match m["state"].as_str() {
        Some("ci_building") => {
            let merged: Vec<J> = order.iter().enumerate().map(|(k, t)| json!({ "thread": t, "tip": format!("{:07x}{k}aa", 0x7ab000 + k), "commit": format!("{:07x}{k}cc", 0xc1c000 + k) })).collect();
            ci["base_branch"] = json!("main");
            m["base"] = json!("main");
            if fail == Some("conflict") && !order.is_empty() {
                let last = order.last().unwrap().clone();
                ci["merged"] = J::Array(merged[..merged.len() - 1].to_vec());
                ci["failure"] = json!({ "kind": "conflict", "thread": last, "files": ["src/routes.rs", "src/api/orders.ts"], "text": format!("sbxw/{last} conflicts in src/routes.rs, src/api/orders.ts") });
                m["state"] = json!("ci_failed");
                return true;
            }
            ci["base"] = json!(format!("ba5e{run:03}0000000000000000000000000000000"));
            ci["tip"] = json!(format!("71b{run:03}00000000000000000000000000000000"));
            ci["merged"] = J::Array(merged);
            ci["job"] = json!(format!("j-ci-{run}"));
            m["state"] = json!("ci_testing");
            true
        }
        Some("ci_testing") => {
            let tip = ci["tip"].as_str().unwrap_or("").to_string();
            let job = ci["job"].as_str().unwrap_or("j").to_string();
            if fail == Some("tests") {
                let tail = "running 412 tests\ntest feat::orders::lists_v2 ... FAILED\n\nfailures:\n    feat::orders::lists_v2\n\ntest result: FAILED. 411 passed; 1 failed";
                let a = json!({ "outcome": "failed", "duration_ms": 183000, "log": "/work/.sbxw-worktrees/.tests/log-1", "tail": tail });
                ci["tests"] = json!({ "job": job, "outcome": "failed", "reused": false, "commit": tip, "attempts": [a.clone(), a] });
                ci["failure"] = json!({ "kind": "tests", "files": [], "text": "failed" });
                m["state"] = json!("ci_failed");
            } else {
                ci["tests"] = json!({ "job": job, "outcome": "passed", "reused": false, "commit": tip, "attempts": [{ "outcome": "passed", "duration_ms": 171000, "log": "/work/.sbxw-worktrees/.tests/log-1", "tail": "test result: ok. 412 passed; 0 failed" }] });
                ci["passed_ms"] = json!(now_ms());
                m["state"] = json!("merged_ci");
            }
            true
        }
        _ => false,
    }
}

/// `merge_status` of a merge, word for word the daemon's
/// (`crates/core/src/plans/ci.rs`, `merge_status`): the stand-in's plans
/// carry it as the daemon's do.
pub fn merge_status(m: &J) -> J {
    let c = m["ci"].is_object().then_some(&m["ci"]);
    let has = c.is_some();
    let s = |word: &str, detail: String, can: bool, why: Option<String>| {
        let mut v = json!({ "word": word, "detail": detail, "can_merge": can, "ci": has });
        if let Some(w) = why {
            v["why_not"] = json!(w);
        }
        v
    };
    let short = |x: Option<&str>| x.map(|t| t.chars().take(7).collect::<String>()).unwrap_or_else(|| "?".into());
    let branch = c.and_then(|c| c["branch"].as_str()).unwrap_or("").to_string();
    let base = c.and_then(|c| c["base_branch"].as_str()).unwrap_or("main").to_string();
    let rebuilt = c.and_then(|c| c["rebuilt_because"].as_str());
    let tip = short(c.and_then(|c| c["tip"].as_str()));
    match m["state"].as_str().unwrap_or("") {
        "proposed" => s("Merge proposed", "The lead proposed the merge: it waits for your click".into(), true, None),
        "failed" => s("Merge failed", "The merge stopped: the results say why. It can be retried".into(), true, None),
        "abandoned" => s("Merge abandoned", "You abandoned this merge: nothing was merged".into(), false, None),
        "merging" => s("Merging", format!("Merging into {base}"), false, Some("the merge is running".into())),
        "merged" if has => s("Merged to main", format!("{base} is now the tested tip of {branch}"), false, None),
        "merged" => s("Merged to main", format!("Merged into {}", m["base"].as_str().unwrap_or("main")), false, None),
        "ci_building" => {
            let why = match rebuilt {
                Some(r) => format!("{r}: sbxw is rebuilding {branch} on the new {base} and will test it again; Merge comes back when it passes"),
                None => format!("sbxw is building {branch} from {base} with the threads' branches; the tests run next, then Merge is offered"),
            };
            s("Building the CI branch", format!("Merging the threads into {branch}"), false, Some(why))
        }
        "ci_testing" => {
            let why = match rebuilt {
                Some(r) => format!("{r}: the rebuilt {branch} is being tested; Merge comes back when it passes"),
                None => format!("the plan's tests run on {branch}; Merge is offered when they pass"),
            };
            s("Testing the CI branch", format!("The plan's tests run on {branch} at {tip}"), false, Some(why))
        }
        "merged_ci" => {
            let mut detail = match c.and_then(|c| c["click_refused"].as_str()) {
                Some(e) => format!("Tests passed on {branch} at {tip}. The last Merge was refused: {e}"),
                None => format!("Tests passed on {branch} at {tip}: Merge fast-forwards {base} to exactly that commit"),
            };
            let skipped: Vec<String> = c.and_then(|c| c["skipped"].as_array()).into_iter().flatten().map(|k| format!("{} ({})", k["thread"].as_str().unwrap_or(""), k["why"].as_str().unwrap_or(""))).collect();
            if !skipped.is_empty() {
                detail += &format!(". Nothing to merge for: {}", skipped.join(", "));
            }
            s("Merged to CI", detail, true, None)
        }
        "ci_stale" => match c.and_then(|c| c["stale_because"].as_str()) {
            Some(r) => s(
                "CI rebuild waits",
                "Merged to CI without the newest commits of a thread".into(),
                false,
                Some(format!("{r}: sbxw rebuilds {branch} with it and tests it again before Merge is offered. One plan rebuilds at a time, the oldest first, so this one waits its turn")),
            ),
            None => s(
                "Main moved: CI rebuild waits",
                format!("Merged to CI on an older {base}"),
                false,
                Some(format!("{base} moved since {branch} was built: sbxw rebuilds it on the new {base} and tests it again before Merge is offered. One plan rebuilds at a time, the oldest first, so this one waits its turn")),
            ),
        },
        "ci_failed" => {
            let f = c.map(|c| &c["failure"]).filter(|f| f.is_object());
            let detail = match f {
                Some(f) if f["kind"] == "conflict" => {
                    let files: Vec<&str> = f["files"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
                    format!("sbxw/{} conflicts in {}: nothing reached {base}, no tests ran", f["thread"].as_str().unwrap_or("?"), if files.is_empty() { "?".to_string() } else { files.join(", ") })
                }
                Some(f) if f["kind"] == "tests" => format!("The tests {} on {branch}: nothing reached {base}", f["text"].as_str().unwrap_or("")),
                Some(f) => format!("{branch} could not be built: {}", f["text"].as_str().unwrap_or("")),
                None => format!("{branch} failed: nothing reached {base}"),
            };
            s("CI failed", detail, false, Some("the lead has the threads fix it, then proposes the merge again, which rebuilds and retests the CI branch".into()))
        }
        _ => J::Null,
    }
}

/// The plan's `merge_status`, as the daemon's plan view adds it.
fn restatus(p: &mut J) {
    if p["merge"].is_object() {
        let st = merge_status(&p["merge"]);
        if !st.is_null() {
            p["merge_status"] = st;
        }
    }
}

/// Abandon merge, as the daemon answers it (`PlanBook::abandon_merge`):
/// a merge proposed or failed, or a CI run not being built; nothing merged,
/// closed or deleted.
fn abandon(st: &mut Standin, id: &str) -> Option<(u16, J)> {
    let Some(i) = plan_index(st.data, id) else { return refuse(400, format!("no plan '{id}'")) };
    let m = &mut plans(st.data)[i]["merge"];
    match m["state"].as_str() {
        Some("proposed") | Some("failed") => {}
        Some("ci_testing" | "merged_ci" | "ci_stale" | "ci_failed") => {}
        Some("ci_building") => return refuse(400, format!("the CI branch of plan {id} is being built: wait a moment, then abandon")),
        Some("merging") => return refuse(400, format!("plan {id} is merging: wait for the merge to end")),
        Some("merged") => return refuse(400, format!("plan {id} is already merged")),
        Some("abandoned") => return refuse(400, format!("the merge of plan {id} is already abandoned")),
        _ => return refuse(400, format!("plan {id} has no merge proposed")),
    }
    m["state"] = json!("abandoned");
    m["abandoned_ms"] = json!(now_ms());
    restatus(&mut plans(st.data)[i]);
    plans(st.data)[i]["updated_ms"] = json!(now_ms());
    let plan = st.data["plans"][i].clone();
    (st.publish)("plan", plan.clone());
    ok(json!({ "plan": plan }))
}

fn merge(st: &mut Standin, id: &str, close: bool) -> Option<(u16, J)> {
    let Some(i) = plan_index(st.data, id) else { return refuse(400, format!("no plan '{id}'")) };
    let p = st.data["plans"][i].clone();
    if p["merge"].is_null() {
        return refuse(400, format!("plan {id} has no merge proposed"));
    }
    let sb = p["sandbox"].as_str().unwrap_or("").to_string();
    if p["merge"]["ci"].is_object() {
        return merge_ci(st, i, id, &sb, close);
    }
    if p["variant"] == "shared" {
        let claims = st.data["board"][&sb]["claims"].as_array().cloned().unwrap_or_default();
        if !claims.is_empty() {
            let list: Vec<String> = claims.iter().map(|c| format!("{} ({})", c["file"].as_str().unwrap_or(""), c["owner"].as_str().unwrap_or(""))).collect();
            let n = list.len();
            return refuse(400, format!("{n} file{} still claimed: {} - release them or force_release first", if n == 1 { "" } else { "s" }, list.join(", ")));
        }
    }
    let fail = st.data["standin_merge_fail"].as_str().map(str::to_string);
    let order: Vec<String> = p["merge"]["order"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
    let mut results: Vec<J> = p["merge"]["results"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| r["ok"] == true).collect();
    {
        let m = &mut plans(st.data)[i]["merge"];
        m["state"] = json!("merging");
        m["base"] = json!("main");
        m["results"] = J::Array(results.clone());
    }
    st.publish_after(200, "plan", st.data["plans"][i].clone());
    let mut failed = false;
    let targets: Vec<String> = if p["variant"] == "shared" { vec![format!("team-{id}")] } else { order.clone() };
    for (k, t) in targets.iter().enumerate() {
        if results.iter().any(|r| r["thread"] == t.as_str() && r["ok"] == true) {
            continue;
        }
        let r = if fail.as_deref() == Some(t.as_str()) {
            json!({ "thread": t, "ok": false, "error": "CONFLICT (content): Merge conflict in src/routes.rs; the merge was aborted, main is as it was" })
        } else {
            json!({ "thread": t, "ok": true, "commit": format!("{:07x}{k}", 0x5eed00 + k * 0x1111) })
        };
        let bad = r["ok"] != true;
        results.push(r);
        plans(st.data)[i]["merge"]["results"] = J::Array(results.clone());
        st.publish_after(300, "plan", st.data["plans"][i].clone());
        if bad {
            // Once: a person resolves the conflict, then a retry works.
            st.data["standin_merge_fail"] = J::Null;
            failed = true;
            break;
        }
    }
    plans(st.data)[i]["merge"]["state"] = json!(if failed { "failed" } else { "merged" });
    restatus(&mut plans(st.data)[i]);
    plans(st.data)[i]["updated_ms"] = json!(now_ms());
    if !failed {
        if close {
            let refuse_close: Vec<String> = st.data["standin_close_refuse"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
            for t in order.iter().filter(|t| !refuse_close.contains(t)) {
                forget(st.data, &sb, t);
            }
        }
    }
    let plan = st.data["plans"][i].clone();
    (st.publish)("plan", plan.clone());
    ok(json!({ "merge": plan["merge"] }))
}

/// The click on a CI plan (`plan_merge_ci`): refused with `why_not` unless
/// Merged to CI; then main fast-forwarded to the tested tip, each thread's
/// result noted as the daemon notes it, the threads closed if asked.
fn merge_ci(st: &mut Standin, i: usize, id: &str, sb: &str, close: bool) -> Option<(u16, J)> {
    let m = st.data["plans"][i]["merge"].clone();
    if m["state"] != "merged_ci" {
        let s = merge_status(&m);
        let why = s["why_not"].as_str().or(s["detail"].as_str()).unwrap_or("").to_string();
        return refuse(400, format!("plan {id} cannot be merged yet: {why}"));
    }
    let c = &m["ci"];
    let base = c["base_branch"].as_str().unwrap_or("main").to_string();
    let tip: String = c["tip"].as_str().unwrap_or("").chars().take(7).collect();
    {
        let p = &mut plans(st.data)[i];
        p["merge"]["state"] = json!("merging");
        p["merge"]["results"] = json!([]);
        restatus(p);
    }
    st.publish_after(200, "plan", st.data["plans"][i].clone());
    let order: Vec<String> = m["order"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
    let results: Vec<J> = order
        .iter()
        .map(|t| match (c["merged"].as_array().and_then(|a| a.iter().find(|s| s["thread"] == t.as_str())), c["skipped"].as_array().and_then(|a| a.iter().find(|s| s["thread"] == t.as_str()))) {
            (Some(s), _) => json!({ "thread": t, "ok": true, "commit": s["commit"].as_str().map(|x| x.chars().take(7).collect::<String>()), "note": format!("fast-forwarded {base} to the tested CI tip {tip}") }),
            (None, Some(k)) => json!({ "thread": t, "ok": true, "note": format!("nothing to merge, {}", k["why"].as_str().unwrap_or("")) }),
            _ => json!({ "thread": t, "ok": true, "note": format!("nothing to merge: not in {}", c["branch"].as_str().unwrap_or("")) }),
        })
        .collect();
    {
        let p = &mut plans(st.data)[i];
        p["merge"]["results"] = J::Array(results);
        p["merge"]["state"] = json!("merged");
        p["updated_ms"] = json!(now_ms());
        restatus(p);
    }
    if close {
        let refuse_close: Vec<String> = st.data["standin_close_refuse"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
        for t in order.iter().filter(|t| !refuse_close.contains(t)) {
            forget(st.data, sb, t);
        }
    }
    let plan = st.data["plans"][i].clone();
    (st.publish)("plan", plan.clone());
    ok(json!({ "merge": plan["merge"] }))
}

/// `data.lead_states[sb]` as the daemon keeps it: `state` with its `since_ms`, none: off (the entry goes).
fn set_lead_state(data: &mut J, sb: &str, state: Option<&str>) {
    if !data["lead_states"].is_object() {
        data["lead_states"] = json!({});
    }
    let m = data["lead_states"].as_object_mut().unwrap();
    match state {
        Some(s) => {
            m.insert(sb.into(), json!({ "state": s, "since_ms": now_ms() }));
        }
        None => {
            m.remove(sb);
        }
    }
}
