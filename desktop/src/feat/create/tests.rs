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
    assert!(r.asked.contains(&Action::CloseModal) && r.asked.contains(&Action::OpenPanel(JOBS.into())));
    let kinds: Vec<&str> = r.events.iter().filter_map(|e| e["kind"].as_str()).collect();
    assert_eq!(kinds.first(), Some(&"plan"));
    assert_eq!(kinds.last(), Some(&"done"));
    assert!(r.asked.contains(&Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": "shop", "pane": "spare" }) }));
    assert!(r.c()["jobs"].as_array().unwrap().is_empty(), "a ready sandbox's card goes");
    assert!(r.asked.contains(&Action::ClosePanel(JOBS.into())));
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

/// Writes `fixtures/create/*.json` and `fixtures/create-jobs/*.json` from
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
        let mut r = Run::new(json!({ "sandboxes": sandboxes.clone(), "favourites": ["/home/you/dev"] }));
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
        "sandboxes": sandboxes.clone(), "favourites": ["/home/you/dev", "/home/you/gone"],
        "kit_mounts": [{ "kit": "sbxw:ssh-agent", "target": "/home/agent/.ssh", "host": "" }],
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
    r.st.form.error = Some("Could not update favourites: '/home/you/nope' is not a folder".into());
    r.ask("create_set", json!({ "field": "name", "value": "Docs!" })).unwrap();
    save("create", "workspace-errors", &r);

    // A remote host: no stars, no native picker, a git URL instead.
    let mut r = Run::new(json!({ "sandboxes": sandboxes.clone(), "remote_hosts": [
        { "name": "buildbox", "address": "you@buildbox.lan", "link": "up" },
        { "name": "gpu", "address": "gpu.lan", "link": "down" },
    ] }));
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

    // Bring-ups: one running mid-way, one failed, one waiting for the person.
    let mut r = Run::new(json!({ "sandboxes": sandboxes.clone() }));
    let steps = r#"[{"id":"workspace","label":"Prepare the workspace"},{"id":"create","label":"Create the sandbox"},{"id":"boot","label":"Start it"},{"id":"kits","label":"Install kits"},{"id":"ports","label":"Publish ports"}]"#;
    r.st.jobs.push(Job::new("shop", Some("create"), Some(json!({}))));
    for e in [
        format!(r#"{{"sandbox":"shop","kind":"plan","steps":{steps}}}"#),
        r#"{"sandbox":"shop","kind":"started","id":"create"}"#.into(),
        r#"{"sandbox":"shop","kind":"detail","text":"Pulling image docker/sandbox-templates:claude-code 63%"}"#.into(),
    ] {
        model::apply_provision(&mut r.st.jobs, &e);
    }
    let mut failed = Job::new("api", Some("create"), Some(json!({})));
    model::apply_provision(&mut vec![], "{}");
    failed.steps = vec![("workspace".into(), "Prepare the workspace".into()), ("create".into(), "Create the sandbox".into()), ("kits".into(), "Install kits".into())];
    failed.at = 2;
    failed.posting = false;
    failed.state = JobState::Failed("sbx: kit sbxw:node: npm ERR! network request to registry.npmjs.org failed (ECONNRESET)".into());
    r.st.jobs.push(failed);
    let mut asking = Job::new("tools", Some("create"), Some(json!({})));
    asking.posting = false;
    asking.state = JobState::Asking("Waiting for your answer: /home/you/dev/tools/sbxw.toml asks for host access you have not approved yet.".into());
    r.st.jobs.push(asking);
    r.data[KEY] = r.st.view(&[]);
    save("create-jobs", "basic", &r);
    r.st.jobs.clear();
    r.data[KEY] = r.st.view(&[]);
    save("create-jobs", "empty", &r);
}
