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

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

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
                let leads = st.data["leads"].as_array_mut().unwrap();
                if !leads.iter().any(|l| l == sb.as_str()) {
                    leads.push(json!(sb));
                }
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
            if let Some(c) = cap("/api/sandboxes/{sandbox}/lead/end") {
                let sb = c[0].clone();
                if let Some(l) = st.data["leads"].as_array_mut() {
                    l.retain(|x| x != sb.as_str());
                }
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
        }
        p["updated_ms"] = json!(now_ms());
        changed.push(p.clone());
    }
    for p in changed {
        (st.publish)("plan", p);
    }
}

fn merge(st: &mut Standin, id: &str, close: bool) -> Option<(u16, J)> {
    let Some(i) = plan_index(st.data, id) else { return refuse(400, format!("no plan '{id}'")) };
    let p = st.data["plans"][i].clone();
    if p["merge"].is_null() {
        return refuse(400, format!("plan {id} has no merge proposed"));
    }
    let sb = p["sandbox"].as_str().unwrap_or("").to_string();
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
