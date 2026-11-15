//! The create feature driven as the core drives it: intents in, actions
//! out, daemon answers back through `on_answer`, with the stand-in routes
//! answering the calls.

use super::*;
use crate::feat::{Method, Standin};
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
            let mut s = Standin { data: &mut self.daemon, publish: &mut publish };
            standin::route(&mut s, method, &c.path, &c.body)
        };
        for (n, v) in evs {
            self.events.push(v.clone());
            let d = self.ctx_data();
            let out = self.st.on_event(&Ctx { data: &d }, &n, &v);
            self.run(out);
        }
        let r = r.ok_or_else(|| format!("no stand-in route for {method} {}", c.path));
        let d = self.ctx_data();
        let out = self.st.on_answer(&Ctx { data: &d }, &c, r);
        self.run(out);
    }

    /// Presses Confirm on the oldest dialog.
    fn confirm(&mut self) {
        let c = self.confirms.remove(0);
        self.call(c.call);
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
    r.ask("create_set", json!({ "field": "name", "value": "Docs" })).unwrap();
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
    assert!(c.lines.iter().any(|l| l.contains("network: api.example.com")) && c.lines.iter().any(|l| l.contains("~/.npmrc")));
    assert!(c.danger);
    assert_eq!(c.call.body["approved"], json!(["network: api.example.com", "host file ~/.npmrc"]));
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
