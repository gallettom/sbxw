//! Network: what sbx waits for a person to decide (`assets/js/approvals.js`),
//! a sandbox's network policy and published ports (`assets/js/ports.js`).
//!
//! The rule of this area: **every answer to an sbx request and every change
//! that widens what a sandbox can reach is confirmed in the core's dialog**,
//! whose text is built here from the daemon's own data (the request's title,
//! detail and option label; a blocked host's resource; a rule's host and
//! scope), never from what a screen passed. Allowing a host, publishing on a
//! fixed host port and removing a deny rule are of risk Approval. Denying,
//! removing an allow rule, unpublishing and publishing on a port sbx picks
//! are direct, as in the web UI.
//!
//! Data: `approvals` (`GET /api/approvals`, every 5 s and after `session`
//! events: sbx has no event of its own for a new request), and for the
//! sandbox the panel shows (`network_for`): `network_policy` (`GET
//! /api/sandboxes/:name/policy`), `network_ports` (`.../ports`),
//! `network_hosts` (`GET /api/hosts`), read when the panel opens and after
//! every change.

use super::*;
use crate::intent::{Arg, Risk, Spec};
use serde_json::json;

pub const FEATURE: Feature = Feature {
    name: "network",
    data: &[
        Data { key: "approvals", path: "/api/approvals", on: &["session"], every: 5, lazy: false },
        Data { key: "network_hosts", path: "/api/hosts", on: &[], every: 0, lazy: true },
    ],
    screens: &[ScreenDecl { name: "network", src: include_str!("../../screens/network.luau"), slot: Slot::Panel { title: "Network", icon: "ports", width: 440.0 } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    on_data: Some(on_data),
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

const PORT: Arg = Arg::Int(1, 65535);

const INTENTS: &[Spec] = &[
    Spec { name: "network_show", risk: Risk::Direct, args: &[("sandbox", Arg::Str, false)], doc: "Opens the Network panel, on that sandbox's policy and ports (none: the requests waiting only, or the sandbox shown before)." },
    Spec { name: "network_refresh", risk: Risk::Direct, args: &[], doc: "Reads the requests, the policy and the ports again." },
    Spec {
        name: "approval_answer",
        risk: Risk::Approval("Answer a request from sbx"),
        args: &[("id", Arg::Str, true), ("option", Arg::Str, true)],
        doc: "Answers one of sbx's pending requests with one of its options, after the person confirms in the core's dialog (the request's own words).",
    },
    Spec {
        name: "network_allow_blocked",
        risk: Risk::Approval("Allow a sandbox to reach a host"),
        args: &[("sandbox", Arg::Str, true), ("host", Arg::Str, true)],
        doc: "Allows a host the sandbox was refused (from the blocked list), for that sandbox only, after the core's dialog.",
    },
    Spec {
        name: "network_allow",
        risk: Risk::Approval("Allow a sandbox to reach a host"),
        args: &[("sandbox", Arg::Str, true), ("resources", Arg::Str, true), ("global", Arg::Bool, false), ("methods", Arg::Str, false), ("path", Arg::Str, false), ("query", Arg::Str, false)],
        doc: "Adds an allow rule (for the sandbox, or `global` for every sandbox), optionally narrowed to some HTTP requests, after the core's dialog.",
    },
    Spec {
        name: "network_deny",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("resources", Arg::Str, true), ("global", Arg::Bool, false), ("methods", Arg::Str, false), ("path", Arg::Str, false), ("query", Arg::Str, false)],
        doc: "Adds a deny rule (narrows what can be reached).",
    },
    Spec { name: "network_rule_rm", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("rule", Arg::Str, true)], doc: "Removes an allow rule (narrows). A deny rule is refused here: removing it is `network_unblock`." },
    Spec {
        name: "network_unblock",
        risk: Risk::Approval("Remove a deny rule"),
        args: &[("sandbox", Arg::Str, true), ("rule", Arg::Str, true)],
        doc: "Removes a deny rule (or a rule whose decision is unknown), after the core's dialog: it widens access.",
    },
    Spec { name: "policy_refresh", risk: Risk::Direct, args: &[], doc: "Asks sbx to re-read the network policy now (`sbx policy refresh`)." },
    Spec {
        name: "port_publish",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("sandbox_port", PORT, true), ("alias", Arg::Str, false)],
        doc: "Publishes a sandbox port on a host port sbx picks, on the loopback address.",
    },
    Spec {
        name: "port_publish_fixed",
        risk: Risk::Approval("Publish a port on a fixed host port"),
        args: &[("sandbox", Arg::Str, true), ("sandbox_port", PORT, true), ("host_port", PORT, true), ("host_ip", Arg::Str, false), ("alias", Arg::Str, false)],
        doc: "Publishes a sandbox port on the host port (and address) given, after the core's dialog.",
    },
    Spec { name: "port_unpublish", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("spec", Arg::Str, true)], doc: "Unpublishes one port mapping (its `spec`)." },
    Spec { name: "port_open", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true), ("spec", Arg::Str, true)], doc: "Opens a published port in the browser (http://host:port, from the daemon's list)." },
];

fn enc(s: &str) -> String {
    crate::client::enc(s)
}

/// The sandbox the panel shows.
fn shown(cx: &Ctx) -> Option<String> {
    cx.get("network_for").as_str().map(str::to_string)
}

/// Reads the panel's sandbox data again.
fn reload(sandbox: Option<&str>) -> Vec<Action> {
    let mut v = vec![Action::Refresh("approvals"), Action::Refresh("network_hosts")];
    if let Some(sb) = sandbox {
        v.push(Action::Fetch { key: "network_policy".into(), path: format!("/api/sandboxes/{}/policy", enc(sb)) });
        v.push(Action::Fetch { key: "network_ports".into(), path: format!("/api/sandboxes/{}/ports", enc(sb)) });
    }
    v
}

/// The pending requests, as the daemon listed them.
pub fn approvals(cx: &Ctx) -> Vec<J> {
    cx.get("approvals")["approvals"].as_array().cloned().unwrap_or_default()
}

/// What sbx's option means, in plain words, from its id and label.
fn option_words(id: &str, label: &str) -> String {
    let l = label.to_lowercase();
    let id_l = id.to_lowercase();
    let deny = id_l.contains("deny") || l.starts_with("deny") || l.contains("block") || l.contains("refuse");
    let once = l.contains("once") || id_l.contains("once");
    let always = l.contains("always") || l.contains("forever") || id_l.contains("always");
    match (deny, once, always) {
        (true, _, _) => "It stays refused.".into(),
        (false, true, _) => "For this one time only (sbx's words: \"".to_string() + label + "\").",
        (false, _, true) => "From now on, until you remove the rule in the network policy (sbx's words: \"".to_string() + label + "\").",
        _ => format!("As sbx's option says: \"{label}\"."),
    }
}

fn is_deny_option(id: &str, label: &str) -> bool {
    let (id, l) = (id.to_lowercase(), label.to_lowercase());
    id.contains("deny") || l.starts_with("deny") || l.contains("block") || l.contains("refuse")
}

fn http_lines(c: Confirm, methods: Option<&str>, path: Option<&str>, query: Option<&str>) -> Confirm {
    let parts: Vec<String> = [("methods", methods), ("path", path), ("query", query)]
        .iter()
        .filter_map(|(k, v)| v.filter(|v| !v.trim().is_empty()).map(|v| format!("{k} {}", v.trim())))
        .collect();
    if parts.is_empty() {
        c.line("Every request to it (no HTTP narrowing).")
    } else {
        c.line(format!("Only HTTP requests matching: {}.", parts.join(", ")))
    }
}

/// A rule of the policy's rule view, by id: `(host, action, scope, source)`.
fn rule(cx: &Ctx, sandbox: &str, id: &str) -> Option<(String, String, String, String)> {
    if shown(cx).as_deref() != Some(sandbox) {
        return None;
    }
    let v = &cx.get("network_policy")["rules"];
    let roles: Vec<&str> = v["roles"].as_array()?.iter().map(|r| r.as_str().unwrap_or("")).collect();
    let at = |row: &Vec<J>, role: &str| roles.iter().position(|r| *r == role).and_then(|i| row.get(i)).and_then(J::as_str).unwrap_or("").trim().to_string();
    let rows = v["rows"].as_array()?;
    let row = rows.iter().filter_map(|r| r.as_array()).find(|r| at(r, "rule_id") == id)?;
    let scope = {
        let own = if at(row, "applies").is_empty() { at(row, "sandbox") } else { at(row, "applies") };
        scope_label(&own)
    };
    Some((at(row, "host"), at(row, "action").to_lowercase(), scope, at(row, "source").to_lowercase()))
}

pub fn scope_label(v: &str) -> String {
    let v = v.trim();
    if v.is_empty() {
        "an unknown set of sandboxes (the listing does not say)".into()
    } else if v.eq_ignore_ascii_case("all") {
        "every sandbox on this machine".into()
    } else if v.to_lowercase().starts_with("sandbox:") {
        format!("sandbox {}", &v[8..])
    } else {
        v.to_string()
    }
}

fn governed(source: &str) -> bool {
    matches!(source, "org" | "corporate" | "system")
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let sb = a.s("sandbox").map(str::to_string);
    let rules_path = |sb: &str| format!("/api/sandboxes/{}/policy/rules", enc(sb));
    Ok(match a.name {
        "network_show" => {
            let target = sb.or_else(|| shown(cx));
            let mut v = vec![Action::Set { key: "network_for".into(), value: target.clone().map(J::String).unwrap_or(J::Null) }];
            if target != shown(cx) {
                v.push(Action::Set { key: "network_policy".into(), value: J::Null });
                v.push(Action::Set { key: "network_ports".into(), value: J::Null });
            }
            v.extend(reload(target.as_deref()));
            v.push(Action::OpenPanel("network".into()));
            v
        }
        "network_refresh" => reload(shown(cx).as_deref()),
        "approval_answer" => {
            let (id, option) = (a.req("id"), a.req("option"));
            let all = approvals(cx);
            let req = all.iter().find(|x| x["id"] == id).ok_or("this request is no longer pending: it was answered, or it lapsed")?;
            let opt = req["options"].as_array().and_then(|o| o.iter().find(|x| x["id"] == option)).ok_or_else(|| format!("\"{option}\" is not one of this request's options"))?;
            let label = opt["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(option);
            let who = req["sandbox"].as_str().unwrap_or("a sandbox");
            let title = req["title"].as_str().unwrap_or("");
            let kind = match req["kind"].as_str().unwrap_or("") {
                "network" => "reach a host",
                "port" => "use a host port",
                k if k.is_empty() => "something",
                k => k,
            };
            let deny = is_deny_option(option, label);
            let mut c = Confirm::new(format!("{label}: {who} asks to {kind}"), Call::post("/api/approvals/respond", json!({ "id": id, "option": option }), format!("answer {who}'s request ({label})")).tag("approval"))
                .line(format!("Sandbox: {who}"))
                .line(format!("Asks for: {title}"));
            for d in req["detail"].as_str().unwrap_or("").lines().filter(|l| !l.trim().is_empty()) {
                c = c.line(d.trim().to_string());
            }
            c = c.line(format!("Your answer: {label}.")).line(format!("How long: {}", option_words(option, label)));
            if req["kind"] == "port" {
                c = c.line("A host port request lapses about 45 s after sbx asked.");
            }
            c = c.labels(label, "Cancel");
            if !deny && req["kind"] == "network" {
                c = c.line(format!("Only {who} is concerned, as sbx scopes this request to it."));
            }
            vec![c.into_action()]
        }
        "network_allow_blocked" => {
            let sb = sb.unwrap();
            let host = a.req("host");
            if shown(cx).as_deref() != Some(sb.as_str()) {
                return Err(format!("{sb}'s policy is not the one shown: open it first"));
            }
            let b = cx.get("network_policy")["blocked"].as_array().and_then(|l| l.iter().find(|b| b["host"] == host).cloned()).ok_or_else(|| format!("{host} is not in {sb}'s blocked list"))?;
            if b["governed"] == true {
                return Err(format!("{host} is blocked by organisation policy: sbx refuses an allow"));
            }
            let res = b["resource"].as_str().filter(|r| !r.is_empty()).ok_or_else(|| format!("{host} is not one destination: nothing to allow"))?;
            vec![Confirm::new(format!("Allow {sb} to reach {res}?"), Call::post(rules_path(&sb), json!({ "resources": res, "decision": "allow", "global": false }), format!("allow {res} for {sb}")))
                .line(format!("Sandbox: {sb} only. Other sandboxes are not changed."))
                .line(format!("Host: {res} (it was refused {} time{}, last {}).", b["count"].as_u64().unwrap_or(1), if b["count"].as_u64().unwrap_or(1) == 1 { "" } else { "s" }, b["last_seen"].as_str().unwrap_or("recently")))
                .line("Every request to it, any port.")
                .line("How long: until you remove the rule in this panel.")
                .labels("Allow", "Cancel")
                .into_action()]
        }
        "network_allow" | "network_deny" => {
            let sb = sb.unwrap();
            let resources = a.req("resources").trim().to_string();
            let global = a.b("global").unwrap_or(false);
            let decision = if a.name == "network_allow" { "allow" } else { "deny" };
            let body = json!({ "resources": resources, "decision": decision, "global": global,
                "methods": a.s("methods").unwrap_or(""), "path": a.s("path").unwrap_or(""), "query": a.s("query").unwrap_or("") });
            let call = Call::post(rules_path(&sb), body, format!("{decision} {resources} for {}", if global { "every sandbox".to_string() } else { sb.clone() }));
            if decision == "deny" {
                return Ok(vec![Action::Call(call)]);
            }
            let mut c = Confirm::new(if global { format!("Allow EVERY sandbox to reach {resources}?") } else { format!("Allow {sb} to reach {resources}?") }, call)
                .line(if global { "Every sandbox on this machine, including ones created later.".to_string() } else { format!("Sandbox: {sb} only. Other sandboxes are not changed.") })
                .line(format!("Resources: {resources}"));
            c = http_lines(c, a.s("methods"), a.s("path"), a.s("query"));
            c = c.line("How long: until you remove the rule in this panel.").labels("Allow", "Cancel");
            if global {
                c = c.danger();
            }
            vec![c.into_action()]
        }
        "network_rule_rm" | "network_unblock" => {
            let sb = sb.unwrap();
            let id = a.req("rule");
            let (host, action, scope, source) = rule(cx, &sb, id).ok_or_else(|| format!("rule {id} is not in {sb}'s policy as shown: refresh it first"))?;
            if governed(&source) {
                return Err("set by organisation policy: sbx does not let sbxw remove it".into());
            }
            let call = Call::post(format!("/api/sandboxes/{}/policy/rules/rm", enc(&sb)), json!({ "rule": id }), format!("remove the {action} rule for {host}"));
            if a.name == "network_rule_rm" {
                if action != "allow" {
                    return Err(format!("removing a {} rule widens what can be reached: that needs your confirmation (network_unblock)", if action.is_empty() { "rule whose decision is unknown, or a deny" } else { action.as_str() }));
                }
                return Ok(vec![Action::Call(call)]);
            }
            if action == "allow" {
                // Narrowing: no need for the dialog, but the person asked for it; keep it plain.
                return Ok(vec![Confirm::new(format!("Remove the allow rule for {host}?"), call).line(format!("It applies to {scope}.")).labels("Remove", "Cancel").into_action()]);
            }
            vec![Confirm::new(format!("Remove the {} rule for {}?", if action.is_empty() { "policy" } else { &action }, if host.is_empty() { id } else { &host }), call)
                .line(format!("It applies to {scope}."))
                .line(format!("Once removed, {host} may be reachable again, until a new rule says otherwise."))
                .labels("Remove", "Cancel")
                .danger()
                .into_action()]
        }
        "policy_refresh" => vec![Action::Call(Call::post("/api/policy/refresh", json!({}), "re-read the network policy").tag("refresh"))],
        "port_publish" | "port_publish_fixed" => {
            let sb = sb.unwrap();
            let sport = a.i("sandbox_port").unwrap_or(0);
            let mut body = json!({ "sandbox_port": sport });
            if let Some(al) = a.s("alias").filter(|s| !s.trim().is_empty()) {
                body["alias"] = al.trim().into();
            }
            if a.name == "port_publish" {
                return Ok(vec![Action::Call(Call::post(format!("/api/sandboxes/{}/ports/publish", enc(&sb)), body, format!("publish {sb}:{sport}")))]);
            }
            let hport = a.i("host_port").unwrap_or(0);
            body["host_port"] = hport.into();
            let ip = a.s("host_ip").map(str::trim).filter(|s| !s.is_empty());
            if let Some(ip) = ip {
                body["host_ip"] = ip.into();
            }
            let open = matches!(ip, Some("0.0.0.0") | Some("::"));
            let mut c = Confirm::new(format!("Publish {sb}'s port {sport} on host port {hport}?"), Call::post(format!("/api/sandboxes/{}/ports/publish", enc(&sb)), body, format!("publish {sb}:{sport} on {hport}")))
                .line(format!("Sandbox: {sb}, port {sport}."))
                .line(format!("On this machine: {}:{hport}.", ip.unwrap_or("127.0.0.1 (sbx's default address)")))
                .line(if open { "Reachable from other machines on your network, not only this one.".to_string() } else { "Reachable from this machine.".to_string() })
                .line("How long: until you unpublish it or the sandbox stops.")
                .labels("Publish", "Cancel");
            if open {
                c = c.danger();
            }
            vec![c.into_action()]
        }
        "port_unpublish" => {
            let sb = sb.unwrap();
            vec![Action::Call(Call::post(format!("/api/sandboxes/{}/ports/unpublish", enc(&sb)), json!({ "spec": a.req("spec") }), format!("unpublish {}", a.req("spec"))))]
        }
        "port_open" => {
            let sb = sb.unwrap();
            if shown(cx).as_deref() != Some(sb.as_str()) {
                return Err(format!("{sb}'s ports are not the ones shown"));
            }
            let p = cx.get("network_ports")["ports"].as_array().and_then(|l| l.iter().find(|p| p["spec"] == a.req("spec")).cloned()).ok_or("no such published port")?;
            let ip = match p["host_ip"].as_str().unwrap_or("") {
                "" | "0.0.0.0" => "127.0.0.1".to_string(),
                "::" => "[::1]".to_string(),
                ip if ip.contains(':') => format!("[{ip}]"),
                ip => ip.to_string(),
            };
            vec![Action::OpenLink(format!("http://{ip}:{}", p["host_port"].as_u64().unwrap_or(0)))]
        }
        other => return Err(format!("{other}: not a network intent")),
    })
}

fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let mut v = match r {
        Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
            let mut v = vec![Action::notice(format!("{}: done", call.what))];
            if let Some(w) = body["hosts_warning"].as_str() {
                v.push(Action::error(format!("{}: {w}", call.what)));
            }
            v
        }
        Ok((c, body)) => vec![Action::error(format!("{}: {}", call.what, body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"))))],
        Err(e) => vec![Action::error(format!("{}: {e}", call.what))],
    };
    v.extend(reload(shown(cx).as_deref()));
    v
}

/// New requests from sbx: a notice that opens the panel on that sandbox.
fn on_data(cx: &Ctx, key: &str, old: &J, new: &J) -> Vec<Action> {
    if key != "approvals" {
        return vec![];
    }
    let seen: Vec<&J> = old["approvals"].as_array().map(|l| l.iter().map(|a| &a["id"]).collect()).unwrap_or_default();
    let fresh: Vec<&J> = new["approvals"].as_array().map(|l| l.iter().filter(|a| !seen.contains(&&a["id"])).collect()).unwrap_or_default();
    match fresh.as_slice() {
        [] => vec![],
        [a] => {
            let who = a["sandbox"].as_str().unwrap_or("a sandbox");
            let _ = cx;
            vec![Action::Notice { text: format!("{who} asks: {}", a["title"].as_str().unwrap_or("a decision")), error: false, then: Some(("network_show".into(), json!({ "sandbox": who }))) }]
        }
        many => vec![Action::Notice { text: format!("{} requests from sbx wait for you", many.len()), error: false, then: Some(("network_show".into(), json!({}))) }],
    }
}

fn extras(cx: &Ctx) -> Vec<Extra> {
    let all = approvals(cx);
    let known = cx.get("approvals")["approvals"].is_array();
    let mut v = vec![];
    if !all.is_empty() {
        v.push(Extra {
            place: Place::Topbar,
            id: "approvals".into(),
            label: "sbx asks".into(),
            icon: Some("flag".into()),
            tone: "amber".into(),
            help: "Requests from sbx wait for your answer".into(),
            count: Some(all.len() as u64),
            intent: Some(("network_show".into(), json!({}))),
        });
    }
    for sb in cx.get("sandboxes").as_array().cloned().unwrap_or_default() {
        let name = sb["name"].as_str().unwrap_or("").to_string();
        let n = if known { all.iter().filter(|a| a["sandbox"] == name.as_str()).count() as u64 } else { sb["approvals"].as_u64().unwrap_or(0) };
        if n > 0 {
            v.push(Extra {
                place: Place::Sandbox(name.clone()),
                id: "approvals".into(),
                label: "⚑".into(),
                icon: None,
                tone: "amber".into(),
                help: format!("sbx waits for you to decide something for {name}"),
                count: Some(n),
                intent: Some(("network_show".into(), json!({ "sandbox": name }))),
            });
            v.push(Extra {
                place: Place::Pane(name.clone()),
                id: "approvals".into(),
                label: "sbx asks".into(),
                icon: Some("flag".into()),
                tone: "amber".into(),
                help: "Answer in the Network panel".into(),
                count: Some(n),
                intent: Some(("network_show".into(), json!({ "sandbox": name }))),
            });
        }
        if sb["status"] == "running" {
            v.push(Extra {
                place: Place::Sandbox(name.clone()),
                id: "network".into(),
                label: String::new(),
                icon: Some("ports".into()),
                tone: "muted".into(),
                help: "Network: policy, blocked hosts, ports".into(),
                count: None,
                intent: Some(("network_show".into(), json!({ "sandbox": name }))),
            });
        }
    }
    v
}

// ---- stand-in routes (fixtures and src/bin/standin.rs) ----
//
// Fixture keys: `approvals` ({ok, approvals}), `policy` (by sandbox, the
// policy panel), `ports` (by sandbox, {ports}), `network_hosts` (a list).

fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let bad = |m: String| Some((400, json!({ "ok": false, "error": m })));
    if (method, path) == ("POST", "/api/approvals/respond") {
        let id = body["id"].as_str().unwrap_or("");
        let option = body["option"].as_str().unwrap_or("");
        let list = st.data["approvals"]["approvals"].as_array().cloned().unwrap_or_default();
        let Some(a) = list.iter().find(|a| a["id"] == id).cloned() else { return bad("this request is no longer pending: it was answered, or it lapsed".into()) };
        if !a["options"].as_array().is_some_and(|o| o.iter().any(|x| x["id"] == option)) {
            return bad(format!("\"{option}\" is not one of this request's options"));
        }
        st.data["approvals"]["approvals"] = J::Array(list.into_iter().filter(|x| x["id"] != id).collect());
        let sb = a["sandbox"].as_str().unwrap_or("").to_string();
        // An allowed host becomes a rule of that sandbox's policy.
        if a["kind"] == "network" && !is_deny_option(option, "") {
            let host = crate::feat::network::host_of(a["title"].as_str().unwrap_or(""));
            add_rule(st.data, &sb, &host, "allow", false);
        }
        if let Some(s) = st.data["sandboxes"].as_array_mut().and_then(|l| l.iter_mut().find(|s| s["name"] == sb.as_str())) {
            let n = s["approvals"].as_u64().unwrap_or(1).saturating_sub(1);
            s["approvals"] = n.into();
        }
        (st.publish)("session", json!({ "sandbox": sb }));
        return Some((200, json!({ "ok": true })));
    }
    if (method, path) == ("POST", "/api/policy/refresh") {
        return Some((200, json!({ "ok": true })));
    }
    if (method, path.split('?').next().unwrap_or("")) == ("GET", "/api/approvals") {
        let v = st.data["approvals"].clone();
        return Some((200, if v.is_null() { json!({ "ok": true, "approvals": [] }) } else { v }));
    }
    if let Some(c) = match_path("/api/sandboxes/{sandbox}/policy", path) {
        let sb = &c[0].1;
        let v = st.data["policy"][sb.as_str()].clone();
        return Some((200, if v.is_null() { json!({ "ok": true, "policies": {}, "rules": { "columns": [], "roles": [], "rows": [] }, "log": {}, "blocked": [], "configured_allow": [], "configured_deny": [] }) } else { v }));
    }
    if let Some(c) = match_path("/api/sandboxes/{sandbox}/ports", path) {
        let v = st.data["ports"][c[0].1.as_str()].clone();
        return Some((200, if v.is_null() { json!({ "ports": [] }) } else { v }));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/policy/rules", path), method) {
        let res = body["resources"].as_str().unwrap_or("").trim().to_string();
        if res.is_empty() {
            return bad("name at least one resource".into());
        }
        add_rule(st.data, &c[0].1, &res, body["decision"].as_str().unwrap_or("allow"), body["global"] == true);
        return Some((200, json!({ "ok": true })));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/policy/rules/rm", path), method) {
        let rule = body["rule"].as_str().unwrap_or("");
        let rows = &mut st.data["policy"][c[0].1.as_str()]["rules"]["rows"];
        let before = rows.as_array().map(Vec::len).unwrap_or(0);
        if let Some(l) = rows.as_array_mut() {
            l.retain(|r| r[1] != rule);
        }
        if rows.as_array().map(Vec::len).unwrap_or(0) == before {
            return bad(format!("no rule {rule}"));
        }
        return Some((200, json!({ "ok": true })));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/ports/publish", path), method) {
        let sb = c[0].1.clone();
        let sport = body["sandbox_port"].as_u64().unwrap_or(0);
        let hport = body["host_port"].as_u64().unwrap_or(40000 + sport % 1000);
        let ip = body["host_ip"].as_str().unwrap_or("127.0.0.1").to_string();
        let spec = format!("{ip}:{hport}:{sport}/tcp");
        let entry = json!({ "spec": spec, "sandbox_port": sport, "proto": "tcp", "host_ip": ip, "host_port": hport });
        let ports = &mut st.data["ports"][sb.as_str()]["ports"];
        if !ports.is_array() {
            *ports = json!([]);
        }
        ports.as_array_mut().unwrap().push(entry);
        return Some((200, json!({ "ok": true })));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/ports/unpublish", path), method) {
        let spec = body["spec"].as_str().unwrap_or("");
        if let Some(l) = st.data["ports"][c[0].1.as_str()]["ports"].as_array_mut() {
            l.retain(|p| p["spec"] != spec);
        }
        return Some((200, json!({ "ok": true })));
    }
    if (method, path) == ("GET", "/api/hosts") {
        let v = st.data["network_hosts"].clone();
        return Some((200, if v.is_null() { json!([]) } else { v }));
    }
    None
}

/// The host of a request's title (`api.github.com:443` → `api.github.com`).
pub fn host_of(title: &str) -> String {
    let t = title.trim();
    match t.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.contains(':') => h.to_string(),
        _ => t.to_string(),
    }
}

/// Columns of the stand-in's rule view: `POLICY, RULE ID, SOURCE, APPLIES TO, ACTION, HOST`.
fn add_rule(data: &mut J, sandbox: &str, host: &str, decision: &str, global: bool) {
    let p = &mut data["policy"][sandbox];
    if p.is_null() {
        *p = json!({ "ok": true, "policies": {}, "log": {}, "blocked": [], "configured_allow": [], "configured_deny": [] });
    }
    let rules = &mut p["rules"];
    if rules["rows"].as_array().is_none_or(|r| r.is_empty()) {
        *rules = json!({ "columns": ["POLICY", "RULE ID", "SOURCE", "APPLIES TO", "ACTION", "HOST"], "roles": ["id", "rule_id", "source", "applies", "action", "host"], "rows": [], "other_sandboxes": 0, "truncated": 0, "sandbox_scoped": true, "raw": "" });
    }
    let n = rules["rows"].as_array().map(Vec::len).unwrap_or(0) + 1;
    let scope = if global { "all".to_string() } else { format!("sandbox:{sandbox}") };
    rules["rows"].as_array_mut().unwrap().push(json!(["local-policy", format!("r-new-{n}"), "local", scope, decision, host]));
    if let Some(b) = p["blocked"].as_array_mut() {
        if decision == "allow" {
            b.retain(|x| x["resource"] != host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> J {
        let v: J = serde_json::from_str(include_str!("../../fixtures/network/basic.json")).unwrap();
        v["daemon"].clone()
    }

    fn cx_data() -> J {
        let mut d = fixture();
        d["network_for"] = "web-app".into();
        d["network_policy"] = d["policy"]["web-app"].clone();
        d["network_ports"] = d["ports"]["web-app"].clone();
        d
    }

    fn ask(name: &'static str, args: J) -> Asked {
        Asked { name, args: args.as_object().unwrap().clone() }
    }

    #[test]
    fn every_answer_and_every_widening_is_the_core_dialog_with_the_daemons_words() {
        let d = cx_data();
        let cx = Ctx { data: &d };
        // An answer: the dialog says who, what, the option's label and how long, from the daemon's list.
        let a = act(&cx, &ask("approval_answer", json!({ "id": "network:abc", "option": "allow" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
        assert!(c.lines.iter().any(|l| l == "Sandbox: web-app"), "{:?}", c.lines);
        assert!(c.lines.iter().any(|l| l == "Asks for: api.github.com:443"));
        assert!(c.lines.iter().any(|l| l.starts_with("How long: From now on")), "{:?}", c.lines);
        assert_eq!(c.call.body, json!({ "id": "network:abc", "option": "allow" }));
        // A request or an option the daemon did not list: refused, nothing to confirm.
        assert!(act(&cx, &ask("approval_answer", json!({ "id": "network:forged", "option": "allow" }))).is_err());
        assert!(act(&cx, &ask("approval_answer", json!({ "id": "network:abc", "option": "allow-everything" }))).is_err());
        // A blocked host: the resource comes from the daemon's blocked list, not the screen.
        let a = act(&cx, &ask("network_allow_blocked", json!({ "sandbox": "web-app", "host": "registry.npmjs.org:443" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!() };
        assert_eq!(c.call.body["resources"], "registry.npmjs.org");
        assert_eq!(c.call.body["global"], false);
        assert!(c.lines[0].contains("web-app only"));
        assert!(act(&cx, &ask("network_allow_blocked", json!({ "sandbox": "web-app", "host": "pastebin.com:443" }))).unwrap_err().contains("organisation"));
        // A global allow is said so, in red.
        let a = act(&cx, &ask("network_allow", json!({ "sandbox": "web-app", "resources": "*.acme.dev", "global": true }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!() };
        assert!(c.danger && c.title.contains("EVERY sandbox"));
        // Narrowing is direct; widening through the direct intent is refused.
        assert!(matches!(act(&cx, &ask("network_deny", json!({ "sandbox": "web-app", "resources": "evil.com" }))).unwrap().as_slice(), [Action::Call(_)]));
        assert!(matches!(act(&cx, &ask("network_rule_rm", json!({ "sandbox": "web-app", "rule": "r-1" }))).unwrap().as_slice(), [Action::Call(_)]));
        assert!(act(&cx, &ask("network_rule_rm", json!({ "sandbox": "web-app", "rule": "r-2" }))).unwrap_err().contains("widens"));
        let a = act(&cx, &ask("network_unblock", json!({ "sandbox": "web-app", "rule": "r-2" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!() };
        assert!(c.lines[0].contains("sandbox web-app"), "{:?}", c.lines);
        assert!(act(&cx, &ask("network_unblock", json!({ "sandbox": "web-app", "rule": "r-org" }))).unwrap_err().contains("organisation"));
        // A fixed host port: the dialog; sbx's choice: direct.
        assert!(matches!(act(&cx, &ask("port_publish", json!({ "sandbox": "web-app", "sandbox_port": 3000 }))).unwrap().as_slice(), [Action::Call(_)]));
        let a = act(&cx, &ask("port_publish_fixed", json!({ "sandbox": "web-app", "sandbox_port": 3000, "host_port": 3000, "host_ip": "0.0.0.0" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!() };
        assert!(c.danger && c.lines.iter().any(|l| l.contains("other machines")));
        // Open: the URL comes from the daemon's list.
        assert_eq!(act(&cx, &ask("port_open", json!({ "sandbox": "web-app", "spec": "127.0.0.1:43000:3000/tcp" }))).unwrap(), vec![Action::OpenLink("http://127.0.0.1:43000".into())]);
    }

    #[test]
    fn every_approval_intent_is_declared_so() {
        for s in INTENTS {
            let widening = matches!(s.name, "approval_answer" | "network_allow_blocked" | "network_allow" | "network_unblock" | "port_publish_fixed");
            assert_eq!(matches!(s.risk, Risk::Approval(_)), widening, "{}", s.name);
        }
    }

    #[test]
    fn a_new_request_is_a_notice_that_opens_the_panel() {
        let d = cx_data();
        let cx = Ctx { data: &d };
        let new = d["approvals"].clone();
        let a = on_data(&cx, "approvals", &json!({ "ok": true, "approvals": [] }), &new);
        let [Action::Notice { text, then: Some((n, args)), .. }] = a.as_slice() else { panic!("{a:?}") };
        assert_eq!((text.as_str(), n.as_str(), &args["sandbox"]), ("web-app asks: api.github.com:443", "network_show", &json!("web-app")));
        assert!(on_data(&cx, "approvals", &new, &new).is_empty());
        let x = extras(&cx);
        assert!(x.iter().any(|e| e.place == Place::Sandbox("web-app".into()) && e.count == Some(1)));
        assert!(x.iter().any(|e| e.place == Place::Topbar));
    }

    #[test]
    fn the_stand_in_answers_and_the_list_empties() {
        let mut d = fixture();
        let mut events = vec![];
        let mut publish = |n: &str, v: J| events.push((n.to_string(), v));
        let mut st = Standin { data: &mut d, publish: &mut publish };
        let (c, _) = standin(&mut st, "POST", "/api/approvals/respond", &json!({ "id": "network:abc", "option": "allow" })).unwrap();
        assert_eq!(c, 200);
        assert_eq!(d["approvals"]["approvals"].as_array().unwrap().len(), 0);
        assert!(d["policy"]["web-app"]["rules"]["rows"].as_array().unwrap().iter().any(|r| r[5] == "api.github.com" && r[4] == "allow"));
    }
}
