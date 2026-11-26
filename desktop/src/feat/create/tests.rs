//! The create feature driven as the core drives it: intents in, actions
//! out, daemon answers back through `on_answer`, with the stand-in routes
//! answering the calls.

use super::*;
use crate::feat::Method;
use serde_json::Map;

struct Run {
    st: State,
    daemon: J,
    data: J,
    events: Vec<J>,
    /// Confirmations the core would draw.
    confirms: Vec<Confirm>,
    /// Intents and notices the core was asked for.
    asked: Vec<Action>,
}

impl Run {
    fn new(daemon: J) -> Run {
        Run { st: State::default(), daemon, data: json!({ "sandboxes": [] }), events: vec![], confirms: vec![], asked: vec![] }
    }

    fn ctx_data(&self) -> J {
        let mut d = self.data.clone();
        d["sandboxes"] = self.daemon["sandboxes"].clone();
        d
    }

    fn ask(&mut self, name: &'static str, args: J) -> Result<(), String> {
        let a = Asked { name, args: match args { J::Object(m) => m, _ => Map::new() } };
        let d = self.ctx_data();
        let out = self.st.act(&Ctx { data: &d }, &a)?;
        self.run(out);
        Ok(())
    }

    /// Runs actions as the core does, calls answered by the stand-in.
    fn run(&mut self, out: Vec<Action>) {
        for act in out {
            match act {
                Action::Set { key, value } => self.data[key] = value,
                Action::Call(c) => self.call(c),
                Action::Confirm(c) => self.confirms.push(*c),
                other => self.asked.push(other),
            }
        }
    }

    fn call(&mut self, c: Call) {
        let method = if c.method == Method::Get { "GET" } else { "POST" };
        let mut evs = vec![];
        let r = {
            let mut publish = |n: &str, v: J| evs.push((n.to_string(), v));
            crate::feat::answer_from(&mut self.daemon, method, &c.path, &c.body, &mut publish)
        };
        for (n, v) in evs {
            self.events.push(v.clone());
            let d = self.ctx_data();
            let out = self.st.on_event(&Ctx { data: &d }, &n, &v);
            self.run(out);
        }
        let r: Result<(u16, J), String> = Ok(r);
        let d = self.ctx_data();
        let out = self.st.on_answer(&Ctx { data: &d }, &c, r);
        self.run(out);
    }

    /// Presses Confirm on the oldest dialog.
    fn confirm(&mut self) {
        let c = self.confirms.remove(0);
        let mut call = c.call;
        // As the core does on Confirm (shell/features.rs, confirm_click).
        for g in &c.grants {
            call.body[g.field.as_str()] = match &g.lines {
                Some(l) => json!(l),
                None => J::Bool(true),
            };
        }
        self.call(call);
    }

    fn c(&self) -> &J {
        &self.data[KEY]
    }
}

#[test]
fn a_workspace_sandbox_from_the_chooser_to_its_terminal() {
    let mut r = Run::new(json!({ "sandboxes": [{ "name": "docs", "status": "running" }] }));
    r.ask("create_open", json!({})).unwrap();
    assert!(r.asked.contains(&Action::OpenModal { screen: "create".into(), arg: "chooser".into() }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    assert_eq!(r.c()["dialog"], "workspace");
    assert_eq!(r.c()["listing"]["path"], "/home/you");
    assert_eq!(r.c()["hosts"], json!([]), "no remote hosts: no host field");
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    assert_eq!(r.c()["path"], "/home/you/dev/shop");
    r.ask("create_set", json!({ "field": "name", "value": "Docs!" })).unwrap();
    assert_eq!(r.c()["name"], "docs");
    assert!(r.c()["name_note"].as_str().unwrap().contains("Lowercase"));
    assert!(r.c()["name_problem"].as_str().unwrap().contains("already exists"));
    assert_eq!(r.c()["can_create"], false);
    assert!(r.ask("create_submit", json!({})).unwrap_err().contains("already exists"));
    r.ask("create_set", json!({ "field": "name", "value": "shop" })).unwrap();
    assert_eq!(r.c()["can_create"], true);
    r.ask("create_submit", json!({})).unwrap();
    assert!(r.confirms.is_empty(), "the workspace alone asks nothing");
    assert!(r.asked.contains(&Action::CloseModal));
    assert!(!r.asked.iter().any(|a| matches!(a, Action::OpenPanel(_) | Action::ClosePanel(_))), "no panel: the status bar shows the bring-up");
    let kinds: Vec<&str> = r.events.iter().filter_map(|e| e["kind"].as_str()).collect();
    assert_eq!(kinds.first(), Some(&"plan"));
    assert_eq!(kinds.last(), Some(&"done"));
    assert!(r.asked.contains(&Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": "shop", "pane": "spare" }) }));
    assert!(r.c()["jobs"].as_array().unwrap().is_empty(), "a ready sandbox's card goes");
}

/// The daemon's timing summary on `done` is said where the bring-up is
/// said ready: in the ready notice when it came first, in a notice of its
/// own when the answer came first, and not at all from an older daemon.
#[test]
fn the_ready_notice_carries_the_daemons_timing_summary() {
    let ready = |r: &Run| r.asked.iter().find_map(|a| match a { Action::Notice { text, error: false, .. } if text.contains("is ready") => Some(text.clone()), _ => None });
    // The stand-in sends its `done` (with the summary) before the answer.
    let mut r = Run::new(json!({ "sandboxes": [] }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "shop" })).unwrap();
    r.ask("create_submit", json!({})).unwrap();
    assert_eq!(ready(&r).as_deref(), Some("Sandbox \"shop\" is ready (ready in 4.0 s: create 2.0 s, boot 1.0 s, kits 1.0 s)"));

    let d = json!({ "sandboxes": [] });
    let ctx = Ctx { data: &d };
    let done = |st: &mut State, v: J| st.on_event(&ctx, "provision", &J::String(v.to_string()));
    // The answer first: the job is gone, the late summary gets its own notice, as text.
    let mut st = State::default();
    st.mark_ready("web");
    let out = done(&mut st, json!({ "sandbox": "web", "kind": "done", "error": null, "summary": "<b>ready</b> in 41 s" }));
    assert_eq!(out, vec![Action::Notice { text: "Sandbox \"web\": <b>ready</b> in 41 s".into(), error: false, then: None }]);
    // An older daemon: no summary, nothing said.
    let out = done(&mut st, json!({ "sandbox": "web", "kind": "done", "error": null }));
    assert!(!out.iter().any(|a| matches!(a, Action::Notice { .. })), "{out:?}");
    // Not a bring-up this app saw end: nothing said.
    let out = done(&mut st, json!({ "sandbox": "ghost", "kind": "done", "summary": "ready in 1 s" }));
    assert!(!out.iter().any(|a| matches!(a, Action::Notice { .. })), "{out:?}");
    // A job that ends ready with no summary: the notice as before.
    let mut j = model::Job::new("old", Some("create"), Some(json!({})));
    j.posting = true;
    st.jobs.push(j);
    model::apply_provision(&mut st.jobs, r#"{"sandbox":"old","kind":"done","error":null}"#);
    assert_eq!(st.jobs[0].summary, None);
}

#[test]
fn extra_folders_are_confirmed_with_what_they_grant() {
    let mut r = Run::new(json!({ "sandboxes": [] }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_repo", json!({ "action": "toggle", "path": "/home/you/dev/shop/backend" })).unwrap();
    r.ask("create_repo", json!({ "action": "toggle", "path": "/home/you/dev/shop/frontend" })).unwrap();
    r.ask("create_repo", json!({ "action": "rw", "index": 2 })).unwrap();
    r.ask("create_port", json!({ "action": "add" })).unwrap();
    r.ask("create_set", json!({ "field": "port_sandbox", "value": "3000", "index": 1 })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "shop" })).unwrap();
    r.ask("create_submit", json!({})).unwrap();
    assert_eq!(r.confirms.len(), 1);
    let c = &r.confirms[0];
    assert!(c.lines[0].contains("/home/you/dev/shop/backend read-write"), "{:?}", c.lines);
    assert!(c.lines.iter().any(|l| l.contains("Mount /home/you/dev/shop/frontend") && l.contains("read-write")));
    assert!(c.lines.iter().any(|l| l.contains("Publish sandbox port 3000 on port 3000")));
    assert_eq!(c.call.body["mounts"], json!([{ "path": "/home/you/dev/shop/frontend", "readonly": false }]));
    assert_eq!(r.c()["jobs"][0]["state"], "asking", "shown as waiting for the answer");
    assert!(r.daemon["sandboxes"].as_array().unwrap().is_empty(), "nothing created before Confirm");
    r.confirm();
    assert_eq!(r.daemon["sandboxes"][0]["name"], "shop");
    assert_eq!(r.daemon["sandboxes"][0]["workspace"], "/home/you/dev/shop/backend");
}

#[test]
fn host_access_is_asked_with_the_daemons_lines_and_a_failure_can_be_retried() {
    let mut r = Run::new(json!({
        "sandboxes": [],
        "create_consent": { "config": "/home/you/dev/docs/sbxw.toml", "grants": ["network: api.example.com", "host file ~/.npmrc"] },
        "create_fail": { "docs": "sbx: no space left on device" },
    }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_pick", json!({})).unwrap();
    assert_eq!(r.c()["path"], "/home/you/dev/docs", "the native picker's folder is browsed to");
    r.ask("create_set", json!({ "field": "name", "value": "docs" })).unwrap();
    r.ask("create_submit", json!({})).unwrap();
    assert_eq!(r.confirms.len(), 1, "the 409 became the core's dialog");
    let c = &r.confirms[0];
    assert_eq!(c.title, "Allow host access?");
    assert_eq!(crate::approval::granted_lines(c), vec!["network: api.example.com".to_string(), "host file ~/.npmrc".into()], "the daemon's lines, shown as is");
    assert!(c.lines[0].contains("/home/you/dev/docs/sbxw.toml"));
    assert!(c.danger);
    assert_eq!(c.call.body.get("approved"), None, "set by the core on Allow only");
    r.confirm();
    assert!(
        r.asked.iter().any(|a| matches!(a, Action::Notice { text, error: true, .. } if text.contains("docs was not created: sbx: no space left on device"))),
        "a failure says so in a notice: {:?}",
        r.asked
    );
    // It stays in the status bar, however long, until dismissed or retried.
    let later = Instant::now() + Duration::from_secs(3600);
    assert!(matches!(r.st.bar_jobs(later).first().map(|j| &j.state), Some(model::BarState::Failed(_))));
    let job = &r.c()["jobs"][0];
    assert_eq!(job["state"], "failed");
    assert_eq!(job["error"], "sbx: no space left on device");
    assert_eq!(job["can_retry"], true);
    assert_eq!(job["steps"][3]["state"], "failed", "the kits step is where it stopped: {job}");
    r.ask("create_retry", json!({ "sandbox": "docs" })).unwrap();
    // Retry sends the body as first sent: the daemon asks again.
    assert_eq!(r.confirms.len(), 1);
    r.confirm();
    assert_eq!(r.daemon["sandboxes"][0]["name"], "docs");
    assert!(r.c()["jobs"].as_array().unwrap().is_empty());
}

#[test]
fn a_chat_sandbox_with_or_without_a_name() {
    let mut r = Run::new(json!({ "sandboxes": [{ "name": "talk" }] }));
    r.ask("create_open", json!({ "dialog": "chat" })).unwrap();
    let suggestion = r.c()["chat_suggestion"].as_str().unwrap().to_string();
    assert!(suggestion.starts_with("chat-") && suggestion.len() == 11);
    r.ask("create_set", json!({ "field": "chat_name", "value": "talk" })).unwrap();
    assert!(r.c()["chat_problem"].as_str().unwrap().contains("already exists"));
    assert!(r.ask("create_chat", json!({})).is_err());
    r.ask("create_set", json!({ "field": "chat_name" })).unwrap();
    assert_eq!(r.c()["chat_problem"], J::Null, "empty: the suggestion is used");
    r.ask("create_chat", json!({})).unwrap();
    assert!(r.daemon["sandboxes"].as_array().unwrap().iter().any(|s| s["name"] == suggestion.as_str() && s["chat"] == true));
}

#[test]
fn restrictions_preview_and_favourites_through_the_routes() {
    let mut r = Run::new(json!({ "sandboxes": [] }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_star", json!({ "path": "/home/you/dev" })).unwrap();
    assert_eq!(r.c()["favourites"][0]["name"], "dev");
    r.ask("create_star", json!({ "path": "/home/you/dev" })).unwrap();
    assert_eq!(r.c()["favourites"], json!([]), "a second press unstars");
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_carve", json!({ "action": "pick", "path": "/home/you/dev/shop" })).unwrap();
    assert_eq!(r.c()["carve_pick"]["root"], "/home/you/dev/shop");
    assert!(r.ask("create_browse", json!({ "path": "/home/you" })).unwrap_err().contains("inside /home/you/dev/shop"));
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop/secrets" })).unwrap();
    assert_eq!(r.c()["path"], "/home/you/dev/shop", "picking leaves the workspace alone");
    r.ask("create_carve", json!({ "action": "use" })).unwrap();
    assert_eq!(r.c()["carves"][0]["rel"], "secrets");
    assert_eq!(r.c()["listing"]["path"], "/home/you/dev/shop", "back where the person was");
    assert_eq!(r.c()["preview"]["summary"], "1 folder mounted, 1 excluded, 1 file not visible in the sandbox");
    assert_eq!(r.c()["carves"][0]["hidden_count"], 1);
    r.ask("create_carve", json!({ "action": "soft", "index": 1 })).unwrap();
    assert_eq!(r.c()["preview"]["summary"], "1 folder mounted, 1 excluded");
    r.ask("create_set", json!({ "field": "name", "value": "shop" })).unwrap();
    r.ask("create_submit", json!({})).unwrap();
    assert_eq!(r.daemon["sandboxes"][0]["name"], "shop");
}

#[test]
fn every_intent_is_documented() {
    let doc = include_str!("../../../docs/screens.md");
    for s in FEATURE.intents {
        assert!(doc.contains(&format!("`{}`", s.name)), "docs/screens.md does not describe {}", s.name);
    }
}

#[test]
fn approved_lines_only_ever_come_from_the_daemons_question() {
    // No intent can carry them: the core refuses an argument its row does not declare.
    for s in FEATURE.intents {
        assert!(s.args.iter().all(|(a, _, _)| *a != "approved"), "{} takes 'approved'", s.name);
    }
    // And act ignores one slipped in anyway: the first send approves nothing.
    let mut r = Run::new(json!({ "sandboxes": [], "create_consent": { "config": "/w/sbxw.toml", "grants": ["mount /etc"] } }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "web" })).unwrap();
    let d = r.ctx_data();
    let a = Asked { name: "create_submit", args: match json!({ "approved": ["mount /etc"] }) { J::Object(m) => m, _ => unreachable!() } };
    let out = r.st.act(&Ctx { data: &d }, &a).unwrap();
    let call = out.iter().find_map(|x| if let Action::Call(c) = x { Some(c.clone()) } else { None }).unwrap();
    assert_eq!(call.body.get("approved"), None, "the first send approves nothing");
    assert_eq!(FEATURE.core_only, &["approved"], "the core refuses it on any call not confirmed (shell::features::core_only_check)");
    r.run(vec![Action::Call(call)]);
    assert_eq!(r.confirms.len(), 1, "the daemon asked; only the person's Confirm sends the lines");
    assert!(r.daemon["sandboxes"].as_array().unwrap().is_empty());
}

/// Writes `fixtures/create/*.json` from
/// real runs through the stand-in routes, so fixtures match the code:
/// `cargo test write_create_fixtures -- --ignored`.
#[test]
#[ignore]
fn write_create_fixtures() {
    let sandboxes = json!([
        { "name": "docs", "agent": "claude", "status": "running", "workspace": "/home/you/dev/docs" },
        { "name": "web-app", "agent": "claude", "status": "stopped", "workspace": "/home/you/dev/web-app" },
    ]);
    let save = |screen: &str, name: &str, r: &Run| {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(screen);
        std::fs::create_dir_all(&dir).unwrap();
        let mut view = r.c().clone();
        if view["chat_suggestion"].as_str().is_some_and(|s| !s.is_empty()) {
            view["chat_suggestion"] = "chat-3f9a1c".into();
        }
        let f = json!({ "daemon": { "sandboxes": r.daemon["sandboxes"], "create": view } });
        std::fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&f).unwrap() + "\n").unwrap();
    };
    let base = || {
        let mut r = Run::new(json!({ "sandboxes": sandboxes.clone(), "favourites": { "ok": true, "favourites": [{ "path": "/home/you/dev" }] } }));
        r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
        r
    };

    let mut r = Run::new(json!({ "sandboxes": sandboxes.clone() }));
    r.ask("create_open", json!({})).unwrap();
    save("create", "chooser", &r);

    let mut r = base();
    save("create", "workspace-empty", &r);
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "shop" })).unwrap();
    save("create", "workspace-basic", &r);

    // Everything optional filled in, local, with the template offer and kit folders.
    let mut r = Run::new(json!({
        "sandboxes": sandboxes.clone(), "favourites": { "ok": true, "favourites": [{ "path": "/home/you/dev" }, { "path": "/home/you/gone" }] },
        "kit_mounts": { "mounts": [{ "kit": "sbxw:ssh-agent", "target": "/home/agent/.ssh", "host": "" }] },
        "template_status": { "auto": true, "template": { "state": "stale", "record": { "tag": "sbxw-auto:7" }, "changed": ["sbx 0.52.1 → 0.53.0"] } },
    }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "shop" })).unwrap();
    r.ask("create_repo", json!({ "action": "toggle", "path": "/home/you/dev/shop/backend" })).unwrap();
    r.ask("create_repo", json!({ "action": "toggle", "path": "/home/you/dev/shop/frontend" })).unwrap();
    r.ask("create_repo", json!({ "action": "rw", "index": 2 })).unwrap();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop/backend" })).unwrap();
    r.ask("create_carve", json!({ "action": "pick", "path": "/home/you/dev/shop/backend" })).unwrap();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop/backend/src" })).unwrap();
    r.ask("create_carve", json!({ "action": "use" })).unwrap();
    r.ask("create_port", json!({ "action": "add" })).unwrap();
    r.ask("create_set", json!({ "field": "port_sandbox", "value": "3000", "index": 1 })).unwrap();
    r.ask("create_set", json!({ "field": "port_alias", "value": "shop.local", "index": 1 })).unwrap();
    r.ask("create_set", json!({ "field": "kit_mount", "value": "/home/you/.ssh", "index": 1 })).unwrap();
    r.ask("create_toggle", json!({ "field": "chrome", "on": true })).unwrap();
    save("create", "workspace-full", &r);

    // Picking a restricted sub-folder.
    let mut r = base();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_carve", json!({ "action": "pick", "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop/secrets" })).unwrap();
    r.ask("create_set", json!({ "field": "carve_typed", "value": "../etc" })).unwrap();
    r.ask("create_carve", json!({ "action": "typed" })).unwrap();
    save("create", "workspace-carve-pick", &r);

    // What the core and the daemon refuse: a taken name with a bad character,
    // nested folders, a restriction the daemon cannot mount, a picker error.
    let mut r = base();
    r.ask("create_browse", json!({ "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "Docs!" })).unwrap();
    r.ask("create_repo", json!({ "action": "toggle", "path": "/home/you/dev/shop/backend" })).unwrap();
    r.ask("create_repo", json!({ "action": "rw", "index": 1 })).unwrap();
    r.ask("create_carve", json!({ "action": "pick", "path": "/home/you/dev/shop/backend" })).unwrap();
    r.ask("create_carve", json!({ "action": "cancel" })).unwrap();
    r.st.form.carves.push(model::Carve { path: "/home/you/dev/shop/backend/src".into(), access: "ro", enforce: "strict" });
    r.st.form.preview = Some(Preview::Answer(json!({ "ok": false, "error": "/home/you/dev/shop/backend/src: a sub-folder cannot be read-only inside a read-only folder" })));
    r.ask("create_repo", json!({ "action": "toggle", "path": "/home/you/dev/shop" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "Docs!" })).unwrap();
    r.st.form.error = Some("Could not update favourites: '/home/you/nope' is not a folder".into());
    r.st.form.preview = Some(Preview::Answer(json!({ "ok": false, "error": "/home/you/dev/shop/backend/src: cannot be read-only here, sbx shares /home/you/dev/shop once" })));
    r.data[KEY] = r.st.view(&super::taken(&Ctx { data: &r.ctx_data() }));
    save("create", "workspace-errors", &r);

    // A remote host: no stars, no native picker, a git URL instead.
    let mut r = Run::new(json!({ "sandboxes": sandboxes.clone(), "remote_hosts": { "ok": true, "hosts": [
        { "name": "buildbox", "address": "you@buildbox.lan", "link": "up" },
        { "name": "gpu", "address": "gpu.lan", "link": "down" },
    ] } }));
    r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
    r.ask("create_host", json!({ "host": "buildbox" })).unwrap();
    r.ask("create_set", json!({ "field": "name", "value": "api" })).unwrap();
    r.ask("create_set", json!({ "field": "git_url", "value": "https://github.com/you/api.git" })).unwrap();
    save("create", "workspace-remote", &r);

    let mut r = Run::new(json!({ "sandboxes": sandboxes.clone() }));
    r.ask("create_open", json!({ "dialog": "chat" })).unwrap();
    save("create", "chat", &r);
    r.ask("create_set", json!({ "field": "chat_name", "value": "Docs" })).unwrap();
    save("create", "chat-taken", &r);
}

/// The status bar's view of bring-ups, from `provision` events alone (a
/// bring-up another client started): the step and its detail's percentage,
/// several at once with the most recent first, a failure, a success that
/// says "created" for a moment.
#[test]
fn the_status_bar_follows_bring_ups_from_provision_events() {
    use model::BarState;
    let mut st = State::default();
    let d = json!({ "sandboxes": [] });
    let ctx = Ctx { data: &d };
    let ev = |st: &mut State, v: J| {
        st.on_event(&ctx, "provision", &J::String(v.to_string()));
    };
    let plan = json!([{ "id": "create", "label": "Create the sandbox" }, { "id": "boot", "label": "Start it" }, { "id": "kits", "label": "Install kits" }, { "id": "ports", "label": "Publish ports" }]);
    ev(&mut st, json!({ "sandbox": "better-api", "kind": "plan", "steps": plan }));
    let now = Instant::now();
    let b = st.bar_jobs(now);
    assert_eq!((b.len(), b[0].step.as_deref(), b[0].frac, &b[0].state), (1, None, 0.0, &BarState::Running), "before the first step");
    ev(&mut st, json!({ "sandbox": "better-api", "kind": "started", "id": "create" }));
    ev(&mut st, json!({ "sandbox": "better-api", "kind": "detail", "text": "Pulling image docker/sandbox-templates:claude-code 63%" }));
    let b = st.bar_jobs(now);
    assert_eq!(b[0].step.as_deref(), Some("Pulling image 63%"));
    assert!((b[0].frac - 0.63 / 4.0).abs() < 1e-4, "{}", b[0].frac);
    ev(&mut st, json!({ "sandbox": "better-api", "kind": "started", "id": "kits" }));
    let b = st.bar_jobs(now);
    assert_eq!((b[0].step.as_deref(), b[0].frac), (Some("Install kits"), 0.5), "the detail clears with the next step");

    // A second one, started later, shows first. (The first one's start is
    // set a second back: no sleep to tell two instants apart.)
    for j in st.jobs.iter_mut().filter(|j| j.sandbox == "better-api") {
        j.since = j.since.checked_sub(Duration::from_secs(1)).unwrap_or(j.since);
    }
    ev(&mut st, json!({ "sandbox": "web", "kind": "plan", "steps": plan }));
    let b = st.bar_jobs(Instant::now());
    assert_eq!(b.iter().map(|j| j.sandbox.as_str()).collect::<Vec<_>>(), ["web", "better-api"]);

    // better-api fails: it stays, in its failed state, with the reason, and a notice says so once.
    let out = st.on_event(&ctx, "provision", &J::String(json!({ "sandbox": "better-api", "kind": "done", "error": "sbx: kits failed" }).to_string()));
    assert!(out.iter().any(|a| matches!(a, Action::Notice { error: true, text, .. } if text.starts_with("better-api was not created"))), "{out:?}");
    let b = st.bar_jobs(Instant::now());
    let f = b.iter().find(|j| j.sandbox == "better-api").unwrap();
    assert_eq!((&f.state, f.step.as_deref()), (&BarState::Failed("sbx: kits failed".into()), Some("Install kits failed")));

    assert_eq!(st.view(&[])["jobs"].as_array().unwrap().len(), 2, "the popover lists both");

    // web succeeds: gone from the list, "created" for CREATED_FOR, then nothing.
    ev(&mut st, json!({ "sandbox": "web", "kind": "done" }));
    let t = Instant::now();
    let b = st.bar_jobs(t);
    let w = b.iter().find(|j| j.sandbox == "web").unwrap();
    assert_eq!((&w.state, w.frac), (&BarState::Created, 1.0));
    assert!(st.bar_jobs(t + CREATED_FOR).iter().all(|j| j.sandbox != "web"), "it clears");
    // Dismissed, the failure goes from the bar.
    let _ = st.act(&ctx, &Asked { name: "create_dismiss", args: json!({ "sandbox": "better-api" }).as_object().cloned().unwrap() });
    assert!(st.bar_jobs(Instant::now() + Duration::from_secs(3600)).iter().all(|j| j.sandbox != "better-api"));
    // A done for a sandbox never seen says nothing.
    ev(&mut st, json!({ "sandbox": "ghost", "kind": "done" }));
    assert!(st.bar_jobs(Instant::now()).iter().all(|j| j.sandbox != "ghost"));
}

#[test]
fn a_detail_line_gives_its_words_and_percentage() {
    use model::{percent, step_words};
    assert_eq!(percent("Pulling image x:y 63%"), Some(63.0));
    assert_eq!(percent("layers 12.5%,"), Some(12.5));
    assert_eq!(percent("no number here"), None);
    assert_eq!(step_words("Create the sandbox", "Pulling image docker/sandbox-templates:claude-code 63%"), "Pulling image 63%");
    assert_eq!(step_words("Create the sandbox", "63%"), "Create the sandbox 63%");
    assert_eq!(step_words("Create the sandbox", "Copying files"), "Create the sandbox");
}

// ---- sbx 0.54: a kit install's denied hosts, allowed, then Retry ----

/// The network feature's own intent, as the core runs it (Kits panel's
/// "Allow for ..." and the popover's): its dialog, then its call.
fn allow(r: &mut Run, sandbox: &str, host: &str, global: bool) -> Confirm {
    let a = Asked { name: "network_allow", args: json!({ "sandbox": sandbox, "resources": host, "global": global }).as_object().unwrap().clone() };
    let d = r.ctx_data();
    let mut v = (crate::feat::network::FEATURE.act)(&Ctx { data: &d }, &a).unwrap();
    let Action::Confirm(c) = v.remove(0) else { panic!("the core's dialog") };
    *c
}

#[test]
fn denied_hosts_are_listed_allowed_one_by_one_and_retried() {
    for (step, global) in [("kits", false), ("create", true)] {
        let mut r = Run::new(json!({ "sandboxes": [], "create_step_ms": 0, "create_denied": { "docs": { "hosts": ["registry.npmjs.org", "github.com"], "step": step, "kit": "sbxw:agent-skills" } } }));
        r.ask("create_open", json!({ "dialog": "workspace" })).unwrap();
        r.ask("create_pick", json!({})).unwrap();
        r.ask("create_set", json!({ "field": "name", "value": "docs" })).unwrap();
        r.ask("create_submit", json!({})).unwrap();
        // Two hosts, the scope the daemon gave, sbx's words, and a notice that opens Kits.
        let d = r.c()["denied"][0].clone();
        assert_eq!(d["hosts"], json!([{ "host": "registry.npmjs.org", "allowed": false }, { "host": "github.com", "allowed": false }]), "{step}: {d}");
        assert_eq!((d["allow_scope"].as_str(), d["can_retry"].as_bool(), d["left"].as_u64()), (Some(if global { "global" } else { "sandbox" }), Some(true), Some(2)));
        assert!(d["message"].as_str().unwrap().contains("network access denied"));
        assert!(r.asked.iter().any(|a| matches!(a, Action::Notice { text, then: Some((n, _)), .. } if n == "kits_show" && text.contains("sbx denied 2 hosts"))), "{:?}", r.asked);
        if step == "create" {
            // The bring-up failed: the popover's card carries them too.
            assert_eq!(r.c()["jobs"][0]["state"], "failed");
            assert_eq!(r.c()["jobs"][0]["denied"]["hosts"][1]["host"], "github.com");
        } else {
            assert!(r.c()["jobs"].as_array().unwrap().is_empty(), "ready with a warning; the failure stays for Kits");
        }
        // One allowed, with the scope said in the dialog: a missing sandbox's rule is every sandbox's.
        let c = allow(&mut r, "docs", "registry.npmjs.org", global);
        assert_eq!(c.call.body["global"], global);
        if global {
            assert!(c.title.contains("EVERY sandbox") && c.danger, "{}", c.title);
        } else {
            assert!(c.lines[0].contains("docs only"), "{:?}", c.lines);
        }
        r.call(c.call);
        assert!(r.st.mark_allowed("docs", "registry.npmjs.org", global));
        let v = r.st.view(&[]);
        assert_eq!((v["denied"][0]["hosts"][0]["allowed"].as_bool(), v["denied"][0]["left"].as_u64()), (Some(true), Some(1)));
        // Retry: the same create; the host still refused is listed alone.
        r.ask("create_retry", json!({ "sandbox": "docs" })).unwrap();
        assert_eq!(r.c()["denied"][0]["hosts"], json!([{ "host": "github.com", "allowed": false }]), "{step}");
        // The other allowed, Retry again: nothing denied, the sandbox ready.
        let c = allow(&mut r, "docs", "github.com", global);
        r.call(c.call);
        r.ask("create_retry", json!({ "sandbox": "docs" })).unwrap();
        assert!(r.c()["denied"].as_array().unwrap().is_empty(), "{step}: {}", r.c()["denied"]);
        assert!(r.c()["jobs"].as_array().unwrap().is_empty());
        assert_eq!(r.daemon["sandboxes"].as_array().unwrap().iter().filter(|s| s["name"] == "docs").count(), 1, "adopted, not created twice");
        // Dismiss forgets one.
        r.st.denied.push(model::Denied::from_event(&json!({ "sandbox": "docs", "kind": "denied", "hosts": ["a.dev"], "exists": true })).unwrap());
        r.ask("create_dismiss", json!({ "sandbox": "docs" })).unwrap();
        assert!(r.c()["denied"].as_array().unwrap().is_empty());
    }
}

#[test]
fn a_denied_event_keeps_only_hosts_and_never_says_this_sandbox_for_a_missing_one() {
    let d = model::Denied::from_event(&json!({ "sandbox": "x", "kind": "denied", "hosts": ["ok.dev", "bad host; rm", "ok.dev", ""], "exists": false, "allow_scope": "sandbox" })).unwrap();
    assert_eq!((d.hosts.clone(), d.allow_scope.as_str()), (vec!["ok.dev".to_string()], "global"));
    assert!(model::Denied::from_event(&json!({ "sandbox": "x", "kind": "denied", "hosts": [] })).is_none());
    assert!(model::Denied::from_event(&json!({ "sandbox": "x", "kind": "warning", "hosts": ["a.dev"] })).is_none());
}
