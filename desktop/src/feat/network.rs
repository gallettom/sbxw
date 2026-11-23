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
    screens: &[ScreenDecl { name: "network", src: include_str!("../../screens/network.luau"), slot: Slot::Panel { title: "Network", icon: "shield", width: 440.0 } }],
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
    Spec {
        name: "network_show",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, false), ("filter", Arg::Bool, false)],
        doc: "Opens the Network panel, on that sandbox's policy and ports, listing only that sandbox's requests (`filter` false: every sandbox's). Without a sandbox: every sandbox's requests, and the policy shown before.",
    },
    Spec { name: "network_filter", risk: Risk::Direct, args: &[("sandbox", Arg::Str, false)], doc: "Lists only that sandbox's requests in the Network panel (none: every sandbox's). View state: nothing is read or sent." },
    Spec { name: "network_refresh", risk: Risk::Direct, args: &[], doc: "Reads the requests, the policy and the ports again." },
    Spec {
        name: "approval_answer",
        risk: Risk::Approval("Answer a request from sbx"),
        args: &[("id", Arg::Str, true), ("option", Arg::Str, true)],
        doc: "Answers one of sbx's pending requests with one of its options, after the person confirms in the core's dialog (the request's own words).",
    },
    Spec {
        name: "approvals_dismiss",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, false), ("title", Arg::Str, false), ("expect", Arg::Int(1, i64::MAX), false)],
        doc: "Dismisses sbx's requests for hosts it already refused (kind `network`: nothing waits on them) of the listed sandboxes, narrowed to one sandbox and one host when given, with sbx's own `dismiss` option, one after the other. `expect`: how many the person saw; another number now is refused (the list changed). Requests that hold something (a kit's host port) are never dismissed here.",
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
        doc: "Publishes a sandbox port on a host port sbx picks (sbx 0.54; an older sbx publishes on the sandbox port and says so), on the loopback address. The port sbx gave shows on its row.",
    },
    Spec {
        name: "port_publish_fixed",
        risk: Risk::Approval("Publish a port on a fixed host port"),
        args: &[("sandbox", Arg::Str, true), ("sandbox_port", PORT, true), ("host_port", PORT, true), ("host_ip", Arg::Str, false), ("alias", Arg::Str, false)],
        doc: "Publishes a sandbox port on the host port (and address) given, after the core's dialog.",
    },
    Spec {
        name: "network_why",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("target", Arg::Str, true)],
        doc: "Asks sbx which rule let a host through (`sbx policy check network`, sbx 0.54), for the sandbox the panel shows; the answer shows under the host in Recent egress. Reads only: removing the rule is `network_rule_rm` or `network_unblock`.",
    },
    Spec { name: "network_why_close", risk: Risk::Direct, args: &[], doc: "Hides the answer of `network_why`. View state only." },
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

/// The sandbox whose requests alone the panel lists (`network_view.filter`):
/// view state, dropped when the panel closes (`<screen>_view`).
pub fn filter(cx: &Ctx) -> Option<String> {
    cx.get("network_view")["filter"].as_str().map(str::to_string)
}

fn set_filter(sb: Option<&str>) -> Action {
    Action::Set { key: "network_view".into(), value: sb.map(|s| json!({ "filter": s })).unwrap_or(J::Null) }
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

/// The requests the app counts and shows: those of a sandbox in the list,
/// exactly as the web UI does (`crates/daemon/src/svc/sandboxes.rs`,
/// `pending_for`, counts one sandbox's; `assets/js/approvals.js` lists one
/// sandbox's). `sbx policy approval ls` lists the whole machine's, so it also
/// has the requests of sandboxes sbxw does not list (removed ones, Jarvis's
/// `sbxw-jarvis`): phase 3 counted those in the top bar ("sbx asks 85").
pub fn waiting(cx: &Ctx) -> Vec<J> {
    let listed: Vec<&str> = cx.get("sandboxes").as_array().map(|l| l.iter().filter_map(|s| s["name"].as_str()).collect()).unwrap_or_default();
    approvals(cx).into_iter().filter(|a| a["sandbox"].as_str().is_some_and(|n| listed.contains(&n))).collect()
}

/// Whether a request holds something until answered. A `network` request is
/// a connection sbx already refused (allowing it helps the next try; nothing
/// waits); a `port` request is a kit waiting, and lapses after 45 s.
pub fn holds(a: &J) -> bool {
    a["kind"] != "network"
}

/// sbx's `dismiss` option of a request, if it offers one.
fn dismiss_option(a: &J) -> Option<String> {
    a["options"].as_array()?.iter().find(|o| o["id"] == "dismiss" || o["label"].as_str().is_some_and(|l| l.to_lowercase().starts_with("dismiss"))).and_then(|o| o["id"].as_str().map(str::to_string))
}

/// What the requests come to: `(requests, distinct (sandbox, kind, what), distinct hosts, sandboxes, held)`.
/// sbx lists the same refused host again each time the sandbox retries it.
pub fn summary(list: &[J]) -> (usize, usize, usize, usize, usize) {
    let mut groups = std::collections::HashSet::new();
    let mut hosts = std::collections::HashSet::new();
    let mut sbs = std::collections::HashSet::new();
    for a in list {
        let (sb, kind, what) = (a["sandbox"].as_str().unwrap_or(""), a["kind"].as_str().unwrap_or(""), a["title"].as_str().unwrap_or(""));
        groups.insert((sb, kind, what));
        hosts.insert((kind, what));
        sbs.insert(sb);
    }
    (list.len(), groups.len(), hosts.len(), sbs.len(), list.iter().filter(|a| holds(a)).count())
}

/// The bulk dismiss in progress: `(id, option)` still to send, done, failed.
static DISMISS: std::sync::Mutex<(Vec<(String, String)>, usize, usize)> = std::sync::Mutex::new((Vec::new(), 0, 0));

fn dismiss_call(id: &str, option: &str) -> Action {
    Action::Call(Call::post("/api/approvals/respond", json!({ "id": id, "option": option }), "dismiss a refused host".to_string()).tag("dismiss").quiet())
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

/// A host, or a host and a port, as the daemon's check takes it
/// (`api.github.com`, `api.github.com:443`); anything else is refused here.
pub fn target_ok(t: &str) -> bool {
    let (host, port) = match t.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (t, None),
    };
    let host_ok = !host.is_empty() && host.len() <= 253 && !host.starts_with(['-', '.']) && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    let port_ok = port.is_none_or(|p| p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n)));
    host_ok && port_ok
}

fn governed(source: &str) -> bool {
    matches!(source, "org" | "corporate" | "system")
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let sb = a.s("sandbox").map(str::to_string);
    let rules_path = |sb: &str| format!("/api/sandboxes/{}/policy/rules", enc(sb));
    Ok(match a.name {
        "network_show" => {
            let target = sb.clone().or_else(|| shown(cx));
            // A sandbox's entries (its bubble, its pane bar's chip, its menu)
            // list its requests only; the top bar's lists every sandbox's.
            let only = sb.as_deref().filter(|_| a.b("filter") != Some(false));
            let mut v = vec![Action::Set { key: "network_for".into(), value: target.clone().map(J::String).unwrap_or(J::Null) }, set_filter(only)];
            if target != shown(cx) {
                v.push(Action::Set { key: "network_policy".into(), value: J::Null });
                v.push(Action::Set { key: "network_ports".into(), value: J::Null });
                v.push(Action::Set { key: "network_why".into(), value: J::Null });
                v.push(Action::Set { key: "network_publish".into(), value: J::Null });
            }
            v.extend(reload(target.as_deref()));
            v.push(Action::OpenPanel("network".into()));
            v
        }
        "network_refresh" => reload(shown(cx).as_deref()),
        "network_filter" => vec![set_filter(sb.as_deref())],
        "approvals_dismiss" => {
            let title = a.s("title");
            let mut todo: Vec<(String, String)> = waiting(cx)
                .iter()
                .filter(|r| !holds(r))
                .filter(|r| sb.as_deref().is_none_or(|s| r["sandbox"] == s))
                .filter(|r| title.is_none_or(|t| r["title"] == t))
                .filter_map(|r| Some((r["id"].as_str()?.to_string(), dismiss_option(r)?)))
                .collect();
            if todo.is_empty() {
                return Err("nothing to dismiss: no refused host listed here offers sbx's Dismiss".into());
            }
            // The person saw a number on the button: act on that many or not at all.
            if let Some(n) = a.i("expect").filter(|n| *n as usize != todo.len()) {
                return Err(format!("the list changed since you looked: {} refused host request{} now, not {n}; look again", todo.len(), if todo.len() == 1 { " is listed" } else { "s are listed" }));
            }
            let mut q = DISMISS.lock().unwrap_or_else(|e| e.into_inner());
            if !q.0.is_empty() {
                return Err("a dismiss is already running".into());
            }
            // One request at a time: each answer is one `sbx policy approval respond`.
            todo.reverse();
            let (id, opt) = todo.pop().unwrap();
            *q = (todo, 0, 0);
            let of = sb.as_deref().map(|s| format!(" of {s}")).unwrap_or_default();
            vec![Action::notice(format!("Dismissing {} refused host request{}{of}…", q.0.len() + 1, if q.0.is_empty() { "" } else { "s" })), dismiss_call(&id, &opt)]
        }
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
        "network_why" => {
            let sb = sb.unwrap();
            let target = a.req("target").trim().to_string();
            if shown(cx).as_deref() != Some(sb.as_str()) {
                return Err(format!("{sb}'s policy is not the one shown: open it first"));
            }
            if !target_ok(&target) {
                return Err(format!("{target:?} is not a host (or host:port) sbx can check"));
            }
            let path = format!("/api/sandboxes/{}/policy/check?target={}", enc(&sb), enc(&target));
            vec![
                Action::Set { key: "network_why".into(), value: json!({ "sandbox": sb, "target": target, "busy": true }) },
                Action::Call(Call::get(path, format!("ask sbx why {target} was allowed")).tag("why").quiet()),
            ]
        }
        "network_why_close" => vec![Action::Set { key: "network_why".into(), value: J::Null }],
        "policy_refresh" => vec![Action::Call(Call::post("/api/policy/refresh", json!({}), "re-read the network policy").tag("refresh"))],
        "port_publish" | "port_publish_fixed" => {
            let sb = sb.unwrap();
            let sport = a.i("sandbox_port").unwrap_or(0);
            let mut body = json!({ "sandbox_port": sport });
            if let Some(al) = a.s("alias").filter(|s| !s.trim().is_empty()) {
                body["alias"] = al.trim().into();
            }
            if a.name == "port_publish" {
                // No host port: sbx 0.54 picks one; the answer says which.
                return Ok(vec![
                    Action::Set { key: "network_publish".into(), value: json!({ "sandbox": sb, "sandbox_port": sport, "busy": true }) },
                    Action::Call(Call::post(format!("/api/sandboxes/{}/ports/publish", enc(&sb)), body, format!("publish {sb}:{sport}")).tag("publish")),
                ]);
            }
            let hport = a.i("host_port").unwrap_or(0);
            body["host_port"] = hport.into();
            let ip = a.s("host_ip").map(str::trim).filter(|s| !s.is_empty());
            if let Some(ip) = ip {
                body["host_ip"] = ip.into();
            }
            let open = matches!(ip, Some("0.0.0.0") | Some("::"));
            let mut c = Confirm::new(format!("Publish {sb}'s port {sport} on host port {hport}?"), Call::post(format!("/api/sandboxes/{}/ports/publish", enc(&sb)), body, format!("publish {sb}:{sport} on {hport}")).tag("publish"))
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
    if call.tag == "dismiss" {
        let ok = matches!(&r, Ok((c, b)) if (200..300).contains(c) && b["ok"] != false);
        let mut q = DISMISS.lock().unwrap_or_else(|e| e.into_inner());
        if ok { q.1 += 1 } else { q.2 += 1 }
        if let Some((id, opt)) = q.0.pop() {
            return vec![dismiss_call(&id, &opt)];
        }
        let (done, failed) = (q.1, q.2);
        *q = (Vec::new(), 0, 0);
        let mut v = vec![if failed == 0 {
            Action::notice(format!("Dismissed {done} refused host request{}", if done == 1 { "" } else { "s" }))
        } else {
            Action::error(format!("Dismissed {done}; {failed} could not be (answered or gone already)"))
        }];
        v.extend(reload(shown(cx).as_deref()));
        return v;
    }
    if call.tag == "why" {
        return vec![Action::Set { key: "network_why".into(), value: why_answer(cx, call, r) }];
    }
    if call.tag == "publish" {
        let (value, mut v) = published(call, &r);
        v.insert(0, Action::Set { key: "network_publish".into(), value });
        v.extend(reload(shown(cx).as_deref()));
        return v;
    }
    let mut v = match r {
        Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
            let mut v = vec![Action::notice(format!("{}: done", call.what))];
            if let Some(w) = body["hosts_warning"].as_str() {
                v.push(Action::error(format!("{}: {w}", call.what)));
            }
            // An allowed host a kit install was denied: its failure says so.
            if call.path.ends_with("/policy/rules") && call.body["decision"] == "allow" {
                let sb = sandbox_of(&call.path);
                let global = call.body["global"] == true;
                for h in call.body["resources"].as_str().unwrap_or("").split([',', ' ']).map(str::trim).filter(|h| !h.is_empty()) {
                    v.extend(crate::feat::create::mark_allowed(cx, &sb, h, global));
                }
            }
            v
        }
        Ok((c, body)) => vec![Action::error(format!("{}: {}", call.what, body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"))))],
        Err(e) => vec![Action::error(format!("{}: {e}", call.what))],
    };
    v.extend(reload(shown(cx).as_deref()));
    v
}

/// The sandbox a call's path names (`/api/sandboxes/<name>/...`).
fn sandbox_of(path: &str) -> String {
    let seg = path.strip_prefix("/api/sandboxes/").and_then(|r| r.split('/').next()).unwrap_or("");
    crate::pct::decode(seg, false)
}

/// `network_why` as the screen reads it: the target asked, and sbx's
/// answer (`supported`, `allowed`, `rule`, `rule_id`, `raw`, `note`) or why
/// there is none. A daemon without the route (older than sbx 0.54's
/// features) is "needs sbx 0.54", never an error.
fn why_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> J {
    let sb = sandbox_of(&call.path);
    let target = call.path.split_once("target=").map(|(_, t)| crate::pct::decode(t, true)).unwrap_or_default();
    let mut v = json!({ "sandbox": sb, "target": target, "busy": false });
    match r {
        Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => {
            for k in ["supported", "allowed", "rule", "rule_id", "raw", "note"] {
                v[k] = body[k].clone();
            }
            // The remove button follows only a rule id the panel lists.
            if let Some(id) = body["rule_id"].as_str() {
                v["removable"] = match rule(cx, &sb, id) {
                    Some((_, action, _, source)) if !governed(&source) => json!(if action == "allow" { "rule_rm" } else { "unblock" }),
                    _ => J::Null,
                };
            }
        }
        Ok((404, _)) => {
            v["supported"] = false.into();
            v["note"] = "needs sbx 0.54 (and an sbxw daemon that knows `sbx policy check`)".into();
        }
        Ok((c, body)) => v["error"] = body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}")).into(),
        Err(e) => v["error"] = e.into(),
    }
    v
}

/// A publish's answer: `network_publish` (the port sbx gave, or why not)
/// and the notices.
fn published(call: &Call, r: &Result<(u16, J), String>) -> (J, Vec<Action>) {
    let sb = sandbox_of(&call.path);
    let sport = call.body["sandbox_port"].as_u64().unwrap_or(0);
    match r {
        Ok((c, b)) if (200..300).contains(c) && b["ok"] != false => {
            let ip = b["host_ip"].as_str().filter(|s| !s.is_empty()).unwrap_or("127.0.0.1").to_string();
            let hport = b["host_port"].as_u64();
            let assigned = b["assigned"] == true;
            let value = json!({ "sandbox": sb, "sandbox_port": sport, "host_ip": ip, "host_port": hport, "assigned": assigned,
                "note": b["note"], "forwarded_port": b["forwarded_port"], "busy": false });
            let mut text = match hport {
                Some(h) => format!("Published {sb}:{sport} on {ip}:{h}{}", if assigned { " (the port sbx picked)" } else { "" }),
                None => format!("{}: done", call.what),
            };
            if let Some(f) = b["forwarded_port"].as_u64() {
                text.push_str(&format!(", here on 127.0.0.1:{f}"));
            }
            let mut v = vec![Action::notice(text)];
            if let Some(n) = b["note"].as_str().filter(|n| !n.is_empty()) {
                v.push(Action::notice(format!("{sb}:{sport}: {n}")));
            }
            if let Some(w) = b["hosts_warning"].as_str() {
                v.push(Action::error(format!("{}: {w}", call.what)));
            }
            (value, v)
        }
        Ok((c, b)) => {
            let why = b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"));
            (json!({ "sandbox": sb, "sandbox_port": sport, "error": why, "busy": false }), vec![Action::error(format!("{}: {why}", call.what))])
        }
        Err(e) => (json!({ "sandbox": sb, "sandbox_port": sport, "error": e, "busy": false }), vec![Action::error(format!("{}: {e}", call.what))]),
    }
}

/// New requests from sbx: a notice that opens the panel on that sandbox.
fn on_data(cx: &Ctx, key: &str, old: &J, new: &J) -> Vec<Action> {
    if key != "approvals" {
        return vec![];
    }
    // The first read is what was already there: said only if something waits.
    let first = old.is_null();
    let seen: Vec<&J> = old["approvals"].as_array().map(|l| l.iter().map(|a| &a["id"]).collect()).unwrap_or_default();
    let listed = waiting(&Ctx { data: &json!({ "sandboxes": cx.get("sandboxes"), "approvals": new }) });
    let fresh: Vec<&J> = listed.iter().filter(|a| !seen.contains(&&a["id"]) && (!first || holds(a))).collect();
    match fresh.as_slice() {
        [] => vec![],
        [a] => {
            let who = a["sandbox"].as_str().unwrap_or("a sandbox");
            vec![Action::Notice { text: format!("{who} asks: {}", a["title"].as_str().unwrap_or("a decision")), error: false, then: Some(("network_show".into(), json!({ "sandbox": who }))) }]
        }
        many => vec![Action::Notice { text: format!("{} requests from sbx wait for you", many.len()), error: false, then: Some(("network_show".into(), json!({}))) }],
    }
}

/// What a pane bar's shield and number say in full: `n` requests from sbx
/// for `name`, `held` of them waiting on a decision.
fn pane_help(name: &str, n: u64, held: usize) -> String {
    let req = if n == 1 { "network request" } else { "network requests" };
    match held {
        0 => format!("{n} {req} sbx refused for {name}: open them"),
        h if h as u64 >= n => format!("{n} {req} from {name} wait for your decision: open them"),
        h => format!("{n} {req} for {name}, {h} waiting for your decision: open them"),
    }
}

fn extras(cx: &Ctx) -> Vec<Extra> {
    let all = waiting(cx);
    let known = cx.get("approvals")["approvals"].is_array();
    // Amber (and a primary button) only when something is held until
    // answered; refused hosts alone are a quiet count.
    let tone = |l: &[&J]| if l.iter().any(|a| holds(a)) { "amber" } else { "muted" };
    let mut v = vec![];
    if !all.is_empty() {
        let refs: Vec<&J> = all.iter().collect();
        // The web UI has no total: each sandbox's row says its own count
        // (the pane bars and rows here say the same). The top bar counts
        // what there is to decide: one per (sandbox, host), as sbx lists a
        // refused host again at each retry.
        let (n, groups, hosts, sbs, held) = summary(&all);
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let mut help = format!("{} from sbx: {} in {}", plural(n, "request"), plural(hosts, "distinct host"), plural(sbs, "sandbox").replace("sandboxs", "sandboxes"));
        help.push_str(&if held > 0 { format!("; {held} hold something until you answer.") } else { "; all are hosts sbx already refused (nothing waits on them).".into() });
        if groups < n {
            help.push_str(&format!(" The number counts each sandbox's host once ({groups}); each sandbox's own count, as in the web UI, counts every request."));
        }
        v.push(Extra {
            place: Place::Topbar,
            id: "approvals".into(),
            label: "sbx asks".into(),
            icon: Some("flag".into()),
            tone: tone(&refs).into(),
            help,
            count: Some(groups as u64),
            intent: Some(("network_show".into(), json!({}))),
        });
    }
    for sb in cx.get("sandboxes").as_array().cloned().unwrap_or_default() {
        let name = sb["name"].as_str().unwrap_or("").to_string();
        let mine: Vec<&J> = all.iter().filter(|a| a["sandbox"] == name.as_str()).collect();
        let n = if known { mine.len() as u64 } else { sb["approvals"].as_u64().unwrap_or(0) };
        let tone = if known { tone(&mine) } else { "amber" };
        if n > 0 {
            v.push(Extra {
                place: Place::Sandbox(name.clone()),
                id: "approvals".into(),
                label: "⚑".into(),
                icon: None,
                tone: tone.into(),
                help: format!("sbx waits for you to decide something for {name}"),
                count: Some(n),
                intent: Some(("network_show".into(), json!({ "sandbox": name }))),
            });
            v.push(Extra {
                place: Place::Pane(name.clone()),
                id: "approvals".into(),
                // The words stay for the overflow menu; inline the chip is
                // the shield and the number (shell/bar.rs, `COUNT_ONLY`),
                // the whole sentence in its tooltip.
                label: "sbx asks".into(),
                icon: Some("shield".into()),
                tone: tone.into(),
                help: pane_help(&name, n, if known { mine.iter().filter(|a| holds(a)).count() } else { n as usize }),
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
        // sbx 0.54 picks a free host port when none is given (from 49153
        // here); an older sbx (`standin_sbx_old`) publishes on the sandbox port.
        let old = st.data["standin_sbx_old"] == true;
        let n = st.data["ports"][sb.as_str()]["ports"].as_array().map(Vec::len).unwrap_or(0) as u64;
        let ip = body["host_ip"].as_str().unwrap_or("127.0.0.1").to_string();
        // As the daemon: sbx picks a host port only on 127.0.0.1.
        if body["host_port"].is_null() && ip != "127.0.0.1" {
            return Some((500, json!({ "ok": false, "error": "sbx picks a host port only on the loopback address 127.0.0.1: name the host port" })));
        }
        let (hport, assigned, note) = match body["host_port"].as_u64() {
            Some(h) => (h, false, J::Null),
            None if old => (sport, false, json!("automatic host port needs sbx 0.54: published on the sandbox port")),
            None => (49153 + n, true, J::Null),
        };
        // The daemon's row: the spec is `IP:HOST:SANDBOX`, and a spec
        // without a protocol is tcp4 on sbx 0.54.
        let spec = format!("{ip}:{hport}:{sport}");
        let entry = json!({ "spec": spec, "sandbox_port": sport, "proto": if old { "tcp" } else { "tcp4" }, "host_ip": ip, "host_port": hport });
        let ports = &mut st.data["ports"][sb.as_str()]["ports"];
        if !ports.is_array() {
            *ports = json!([]);
        }
        ports.as_array_mut().unwrap().push(entry);
        return Some((200, json!({ "ok": true, "sandbox_port": sport, "host_ip": ip, "host_port": hport, "assigned": assigned, "note": note, "forwarded_port": J::Null })));
    }
    if let (Some(c), "POST") = (match_path("/api/sandboxes/{sandbox}/ports/unpublish", path), method) {
        let spec = body["spec"].as_str().unwrap_or("");
        if let Some(l) = st.data["ports"][c[0].1.as_str()]["ports"].as_array_mut() {
            l.retain(|p| p["spec"] != spec);
        }
        return Some((200, json!({ "ok": true })));
    }
    if let (Some(c), "GET") = (match_path("/api/sandboxes/{sandbox}/policy/check", path), method) {
        return Some(policy_check(st.data, &c[0].1, &query(path, "target").unwrap_or_default()));
    }
    if (method, path) == ("GET", "/api/hosts") {
        let v = st.data["network_hosts"].clone();
        return Some((200, if v.is_null() { json!([]) } else { v }));
    }
    None
}

/// `GET .../policy/check?target=`: the fixture's `policy_check[sandbox][target]`
/// when it has one, else the rule of the stand-in's own list whose host is
/// the target's (its id), else an answer that names no rule.
fn policy_check(data: &J, sb: &str, target: &str) -> (u16, J) {
    if target.is_empty() || !target_ok(target) {
        return (400, json!({ "ok": false, "error": format!("not a target: {target:?}") }));
    }
    if data["standin_sbx_old"] == true {
        return (200, json!({ "ok": true, "supported": false, "target": target, "note": "needs sbx 0.54: `sbx policy check` is new there" }));
    }
    let fixed = &data["policy_check"][sb][target];
    if !fixed.is_null() {
        return (200, fixed.clone());
    }
    let host = host_of(target);
    let rules = &data["policy"][sb]["rules"];
    let roles: Vec<&str> = rules["roles"].as_array().map(|r| r.iter().filter_map(J::as_str).collect()).unwrap_or_default();
    let col = |role: &str| roles.iter().position(|r| *r == role);
    let found = rules["rows"].as_array().into_iter().flatten().filter_map(J::as_array).find(|r| col("host").and_then(|i| r.get(i)).and_then(J::as_str) == Some(host.as_str()));
    match found.and_then(|r| col("rule_id").and_then(|i| r.get(i)).and_then(J::as_str)) {
        Some(id) => (200, json!({ "ok": true, "supported": true, "target": target, "allowed": true, "rule": id, "rule_id": id, "raw": format!("allowed by rule {id} ({host})") })),
        None => (200, json!({ "ok": true, "supported": true, "target": target, "allowed": J::Null, "rule": J::Null, "rule_id": J::Null, "raw": format!("{target}: no decision recorded") })),
    }
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
        assert!(matches!(act(&cx, &ask("port_publish", json!({ "sandbox": "web-app", "sandbox_port": 3000 }))).unwrap().as_slice(), [Action::Set { .. }, Action::Call(_)]));
        let a = act(&cx, &ask("port_publish_fixed", json!({ "sandbox": "web-app", "sandbox_port": 3000, "host_port": 3000, "host_ip": "0.0.0.0" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!() };
        assert!(c.danger && c.lines.iter().any(|l| l.contains("other machines")));
        // Open: the URL comes from the daemon's list.
        assert_eq!(act(&cx, &ask("port_open", json!({ "sandbox": "web-app", "spec": "127.0.0.1:43000:3000/tcp" }))).unwrap(), vec![Action::OpenLink("http://127.0.0.1:43000".into())]);
    }

    /// Phase 4 (user, macOS): "sbx asks 88" in the top bar, "sbx asks 2" on
    /// test-sbx's panes. The web UI has no total; each sandbox's count is
    /// every request of it (`pending_for`), the pane bars say the same. The
    /// top bar counts one per (sandbox, host): sbx lists a refused host again
    /// at each retry. Refused hosts are dismissed in bulk, one call after the
    /// other, never a request that holds something.
    /// The bulk dismiss's queue (`DISMISS`) is process-wide: the tests that
    /// run one hold this for their whole run and start from an empty queue,
    /// so two of them in parallel never meet in it.
    fn one_dismiss_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let g = ONE.lock().unwrap_or_else(|e| e.into_inner());
        *DISMISS.lock().unwrap_or_else(|e| e.into_inner()) = (Vec::new(), 0, 0);
        g
    }

    #[test]
    fn repeated_hosts_count_once_and_refused_ones_dismiss_in_bulk() {
        let _one = one_dismiss_at_a_time();
        let row = |id: &str, sb: &str, kind: &str, title: &str, at: &str| json!({ "id": id, "kind": kind, "sandbox": sb, "title": title, "detail": "", "created_at": at,
            "options": [{ "id": "allow", "label": "Allow" }, { "id": "dismiss", "label": "Dismiss" }] });
        let d = json!({
            "sandboxes": [{ "name": "test-sbx", "status": "running" }, { "name": "sbxw", "status": "running" }],
            "approvals": { "ok": true, "approvals": [
                row("network:1", "test-sbx", "network", "a.com:443", "1"), row("network:2", "test-sbx", "network", "a.com:443", "2"),
                row("network:3", "sbxw", "network", "a.com:443", "1"), row("network:4", "sbxw", "network", "b.com:443", "1"),
                row("network:5", "sbxw", "network", "b.com:443", "2"), row("port:6", "sbxw", "port", "5173", "3"),
                row("network:7", "gone", "network", "c.com:443", "1"),
            ] },
        });
        let cx = Ctx { data: &d };
        assert_eq!(summary(&waiting(&cx)), (6, 4, 3, 2, 1), "requests, (sandbox, host), hosts, sandboxes, held");
        let x = extras(&cx);
        let top = x.iter().find(|e| e.place == Place::Topbar).unwrap();
        assert_eq!(top.count, Some(4));
        assert!(top.help.contains("6 requests") && top.help.contains("3 distinct hosts") && top.help.contains("2 sandboxes"), "{}", top.help);
        let pane = |sb: &str| x.iter().find(|e| e.place == Place::Pane(sb.into()) && e.id == "approvals").unwrap().count;
        assert_eq!((pane("test-sbx"), pane("sbxw")), (Some(2), Some(4)), "each sandbox's own count, as the web UI's row");
        // Dismiss all: the refused hosts of listed sandboxes, one at a time; the port request stays.
        let a = act(&cx, &ask("approvals_dismiss", json!({}))).unwrap();
        let first = a.iter().find_map(|a| if let Action::Call(c) = a { Some(c.clone()) } else { None }).unwrap();
        assert_eq!((first.path.as_str(), first.body["id"].as_str(), first.body["option"].as_str()), ("/api/approvals/respond", Some("network:1"), Some("dismiss")));
        assert!(act(&cx, &ask("approvals_dismiss", json!({}))).is_err(), "one bulk dismiss at a time");
        let mut sent = vec![first.body["id"].as_str().unwrap().to_string()];
        let mut call = first;
        loop {
            let next = on_answer(&cx, &call, Ok((200, json!({ "ok": true }))));
            match next.as_slice() {
                [Action::Call(c)] => {
                    sent.push(c.body["id"].as_str().unwrap().to_string());
                    call = c.clone();
                }
                v => {
                    assert!(matches!(&v[0], Action::Notice { text, error: false, .. } if text == "Dismissed 5 refused host requests"), "{v:?}");
                    break;
                }
            }
        }
        assert_eq!(sent, ["network:1", "network:2", "network:3", "network:4", "network:5"]);
        // One host of one sandbox.
        let a = act(&cx, &ask("approvals_dismiss", json!({ "sandbox": "sbxw", "title": "b.com:443" }))).unwrap();
        let c = a.iter().find_map(|a| if let Action::Call(c) = a { Some(c.clone()) } else { None }).unwrap();
        assert_eq!(c.body["id"], "network:4");
        let v = on_answer(&cx, &c, Ok((200, json!({ "ok": true }))));
        let [Action::Call(c)] = v.as_slice() else { panic!("{v:?}") };
        assert_eq!(c.body["id"], "network:5");
        let v = on_answer(&cx, c, Ok((400, json!({ "ok": false, "error": "gone" }))));
        assert!(matches!(&v[0], Action::Notice { error: true, .. }), "{v:?}");
        assert!(act(&cx, &ask("approvals_dismiss", json!({ "sandbox": "sbxw", "title": "5173" }))).is_err(), "a port request is never dismissed here");
    }

    /// Phase 3 bug (user, macOS): "sbx asks 85" in the top bar. The daemon's
    /// `GET /api/approvals` is `sbx policy approval ls`, the whole machine's;
    /// the web UI counts only a listed sandbox's. Refused hosts alone are quiet.
    #[test]
    fn only_listed_sandboxes_requests_count_and_refused_hosts_are_quiet() {
        let f = crate::fixture::Fixture::load(&crate::fixture::dir().join("network/machine-wide.json")).unwrap();
        let d = f.raw.clone();
        let cx = Ctx { data: &d };
        assert_eq!(approvals(&cx).len(), 83);
        assert_eq!(waiting(&cx).len(), 2);
        let x = extras(&cx);
        let top = x.iter().find(|e| e.place == Place::Topbar).unwrap();
        // Two requests for two hosts of web-app: one each.
        assert_eq!((top.count, top.tone.as_str()), (Some(2), "muted"));
        let row = x.iter().find(|e| e.place == Place::Sandbox("web-app".into()) && e.id == "approvals").unwrap();
        assert_eq!((row.count, row.tone.as_str()), (Some(2), "muted"));
        // A kit's port request holds something: amber.
        let mut d2 = d.clone();
        d2["approvals"]["approvals"].as_array_mut().unwrap().push(json!({ "id": "port:k1", "kind": "port", "sandbox": "web-app", "title": "5173", "detail": "", "created_at": "", "options": [{ "id": "allow", "label": "Allow" }] }));
        let top = extras(&Ctx { data: &d2 }).into_iter().find(|e| e.place == Place::Topbar).unwrap();
        assert_eq!((top.count, top.tone.as_str()), (Some(3), "amber"));
        // The first read says nothing about refused hosts; a new one later is said.
        assert!(on_data(&cx, "approvals", &J::Null, &d["approvals"]).is_empty());
        let a = on_data(&Ctx { data: &d2 }, "approvals", &J::Null, &d2["approvals"]);
        assert!(matches!(a.as_slice(), [Action::Notice { text, .. }] if text == "web-app asks: 5173"), "{a:?}");
        // Requests of unlisted sandboxes never make a notice.
        let mut d3 = d.clone();
        d3["approvals"]["approvals"].as_array_mut().unwrap().push(json!({ "id": "network:x", "kind": "network", "sandbox": "sbxw-jarvis", "title": "x.com:443", "options": [{ "id": "allow" }] }));
        assert!(on_data(&Ctx { data: &d3 }, "approvals", &d["approvals"], &d3["approvals"]).is_empty());
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

    // ---- the filter (phase 4, user, macOS): "clicking the blue bubble shows
    // the 113 waiting instead of the 2 of the sandbox" ----

    fn filtered() -> J {
        let f = crate::fixture::Fixture::load(&crate::fixture::dir().join("network/filtered.json")).unwrap();
        f.raw.clone()
    }

    /// The view the panel's actions set: `network_view`, or none.
    fn view_set(v: &[Action]) -> Option<J> {
        v.iter().find_map(|a| match a {
            Action::Set { key, value } if key == "network_view" => Some(value.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_sandboxs_entries_filter_and_the_top_bars_does_not() {
        let d = filtered();
        let cx = Ctx { data: &d };
        let x = extras(&cx);
        // The bubble and the row menu's "Network requests (2)" are the row's
        // `approvals` extra; the pane bar's chip is the pane's; the row's
        // Network item is `network`.
        let entry = |p: Place, id: &str| x.iter().find(|e| e.place == p && e.id == id).and_then(|e| e.intent.clone()).unwrap();
        for (n, args) in [
            entry(Place::Sandbox("test-sbx".into()), "approvals"),
            entry(Place::Pane("test-sbx".into()), "approvals"),
            entry(Place::Sandbox("test-sbx".into()), "network"),
        ] {
            let v = act(&cx, &Asked { name: Box::leak(n.into_boxed_str()), args: args.as_object().unwrap().clone() }).unwrap();
            assert_eq!(view_set(&v), Some(json!({ "filter": "test-sbx" })), "{args}");
            assert!(v.contains(&Action::OpenPanel("network".into())));
        }
        let (n, args) = entry(Place::Topbar, "approvals");
        let v = act(&cx, &Asked { name: Box::leak(n.into_boxed_str()), args: args.as_object().unwrap().clone() }).unwrap();
        assert_eq!(view_set(&v), Some(J::Null), "the global entry lists every sandbox's");
        // The panel's own sandbox picker keeps the filter on or off.
        assert_eq!(view_set(&act(&cx, &ask("network_show", json!({ "sandbox": "api", "filter": false }))).unwrap()), Some(J::Null));
        // Show all, the chip's x: view state only, nothing read or sent.
        let v = act(&cx, &ask("network_filter", json!({}))).unwrap();
        assert_eq!(v, vec![Action::Set { key: "network_view".into(), value: J::Null }]);
        let v = act(&cx, &ask("network_filter", json!({ "sandbox": "api" }))).unwrap();
        assert_eq!(v, vec![Action::Set { key: "network_view".into(), value: json!({ "filter": "api" }) }]);
    }

    #[test]
    fn under_a_filter_the_bulk_dismiss_sends_exactly_that_sandboxs_ids() {
        let _one = one_dismiss_at_a_time();
        let d = filtered();
        let cx = Ctx { data: &d };
        let a = act(&cx, &ask("approvals_dismiss", json!({ "sandbox": "test-sbx", "expect": 2 }))).unwrap();
        assert!(matches!(&a[0], Action::Notice { text, .. } if text == "Dismissing 2 refused host requests of test-sbx…"), "{a:?}");
        let mut call = a.iter().find_map(|a| if let Action::Call(c) = a { Some(c.clone()) } else { None }).unwrap();
        let mut sent = vec![call.body["id"].as_str().unwrap().to_string()];
        loop {
            let next = on_answer(&cx, &call, Ok((200, json!({ "ok": true }))));
            let [Action::Call(c)] = next.as_slice() else {
                assert!(matches!(&next[0], Action::Notice { text, error: false, .. } if text == "Dismissed 2 refused host requests"), "{next:?}");
                break;
            };
            sent.push(c.body["id"].as_str().unwrap().to_string());
            call = c.clone();
        }
        assert_eq!(sent, ["network:f000", "network:f001"]);
        // The number on the button no longer matches: nothing is sent.
        let e = act(&cx, &ask("approvals_dismiss", json!({ "sandbox": "test-sbx", "expect": 3 }))).unwrap_err();
        assert!(e.contains("changed") && e.contains("2 refused host requests are listed now, not 3"), "{e}");
        // Unfiltered: every listed one.
        let a = act(&cx, &ask("approvals_dismiss", json!({ "expect": 113 }))).unwrap();
        assert!(matches!(&a[0], Action::Notice { text, .. } if text == "Dismissing 113 refused host requests…"), "{a:?}");
        *DISMISS.lock().unwrap() = (Vec::new(), 0, 0);
    }

    /// Each element's texts, keys and helps, and the button args by key.
    fn walk(n: &crate::luau::Node, out: &mut Vec<String>, args: &mut Vec<(String, J)>) {
        out.push(n.text.clone());
        out.extend(n.key.clone());
        out.extend(n.help.clone());
        if let (Some(k), false) = (&n.key, n.arg.is_null()) {
            args.push((k.clone(), n.arg.clone()));
        }
        for c in &n.children {
            walk(c, out, args);
        }
    }

    fn render(mut data: J, filter: Option<&str>, more: usize) -> (Vec<String>, Vec<(String, J)>, crate::luau::LuauHost) {
        data["network_view"] = filter.map(|f| json!({ "filter": f })).unwrap_or(J::Null);
        if data["app"].is_null() {
            data["app"] = json!({});
        }
        let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
        h.load("network", include_str!("../../screens/network.luau")).unwrap();
        h.render(&data).unwrap();
        for _ in 0..more {
            h.act("more", &J::Null, &J::Null).unwrap();
        }
        let tree = h.render(&data).unwrap();
        let (mut out, mut args) = (vec![], vec![]);
        walk(&tree, &mut out, &mut args);
        (out, args, h)
    }

    fn cards(out: &[String]) -> usize {
        out.iter().filter(|s| s.starts_with("req-")).count()
    }

    #[test]
    fn the_panel_filtered_counts_and_labels_only_what_it_shows() {
        let d = filtered();
        // Filtered from test-sbx's bubble.
        let (out, args, h) = render(d.clone(), Some("test-sbx"), 0);
        let all = out.join("\n");
        assert_eq!(cards(&out), 2);
        assert!(all.contains("net-filter-chip") && all.contains("Show all 113"), "{all}");
        assert!(all.contains("2 requests: 2 distinct hosts in test-sbx."), "{all}");
        assert!(all.contains("Dismiss these 2") && !all.contains("Dismiss all"), "{all}");
        let bulk = args.iter().find(|(k, _)| k == "approvals-dismiss-all").unwrap();
        assert_eq!(bulk.1, json!({ "sandbox": "test-sbx", "expect": 2 }));
        // The press asks exactly that.
        let asks = h.act("dismiss_all", &bulk.1, &J::Null).unwrap();
        assert_eq!(asks["name"], "approvals_dismiss", "{asks}");
        assert_eq!(asks["args"], json!({ "sandbox": "test-sbx", "expect": 2 }));
        // An empty Luau table comes as `[]`: no sandbox, the filter cleared.
        let clear = h.act("filter_clear", &J::Null, &J::Null).unwrap();
        assert_eq!(clear["name"], "network_filter", "{clear}");
        assert!(clear["args"]["sandbox"].is_null(), "{clear}");
        // Unfiltered: 113 cards' worth, 25 drawn, "Show more"; every sandbox's.
        let (out, args, _) = render(d.clone(), None, 0);
        let all = out.join("\n");
        assert_eq!(cards(&out), 25);
        assert!(all.contains("113 requests: 33 distinct hosts in 9 sandboxes. Showing 25 of 113."), "{all}");
        assert!(all.contains("Show 25 more") && all.contains("Dismiss all 113") && !all.contains("net-filter-chip"), "{all}");
        assert_eq!(args.iter().find(|(k, _)| k == "approvals-dismiss-all").unwrap().1, json!({ "expect": 113 }));
        let (out, _, _) = render(d.clone(), None, 4);
        assert_eq!(cards(&out), 113);
        assert!(!out.iter().any(|s| s == "net-req-more"));
        // A sandbox with nothing left: said so, and the way back.
        let (out, _, _) = render(d.clone(), Some("notes"), 0);
        let all = out.join("\n");
        assert_eq!(cards(&out), 0);
        assert!(all.contains("Nothing waits in notes.") && all.contains("Show all 113") && !all.contains("approvals-dismiss-all"), "{all}");
    }

    /// The filter is the panel's view state: closing the panel drops it,
    /// so the next opening (the rail, a tab) lists every sandbox's.
    #[test]
    fn closing_the_panel_drops_the_filter() {
        use crate::screen::Host;
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/network/filtered.json");
        let mut a = crate::shell::App::new(crate::shell::Args { fixture: Some(fixture.into()), ..Default::default() });
        let run = |a: &mut crate::shell::App, v: J| {
            let list = crate::intent::parse(&v).unwrap();
            a.intents("sandboxes", list);
        };
        run(&mut a, json!({ "kind": "intent", "name": "network_show", "args": { "sandbox": "test-sbx" } }));
        assert_eq!(a.screen_data()["network_view"], json!({ "filter": "test-sbx" }));
        assert_eq!(a.feats.panel.as_deref(), Some("network"));
        run(&mut a, json!({ "kind": "intent", "name": "close_panel", "args": { "screen": "network" } }));
        assert!(a.screen_data()["network_view"].is_null(), "dropped on close");
        run(&mut a, json!({ "kind": "intent", "name": "open_panel", "args": { "screen": "network" } }));
        assert!(a.screen_data()["network_view"].is_null(), "reopened unfiltered");
    }

    // ---- sbx 0.54: an automatic host port, and which rule allowed a host ----

    /// The stand-in daemon's answer to a call, through the feature's routes.
    fn answer(d: &mut J, call: &Call) -> (u16, J) {
        let mut publish = |_: &str, _: J| {};
        let mut st = Standin { data: d, publish: &mut publish };
        standin(&mut st, if call.method == Method::Get { "GET" } else { "POST" }, &call.path, &call.body).expect("a stand-in route")
    }

    fn the_call(v: &[Action]) -> Call {
        v.iter().find_map(|a| if let Action::Call(c) = a { Some(c.clone()) } else { None }).expect("a call")
    }

    fn set_of(v: &[Action], key: &str) -> Option<J> {
        v.iter().find_map(|a| match a {
            Action::Set { key: k, value } if k == key => Some(value.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_publish_without_host_port_shows_the_port_sbx_picked() {
        let mut d = cx_data();
        let v = act(&Ctx { data: &d }, &ask("port_publish", json!({ "sandbox": "web-app", "sandbox_port": 5173 }))).unwrap();
        // Busy while it runs; no host_port in the body: sbx picks.
        assert_eq!(set_of(&v, "network_publish").unwrap()["busy"], true);
        let call = the_call(&v);
        assert!(call.body["host_port"].is_null(), "{}", call.body);
        let r = answer(&mut d, &call);
        assert_eq!((r.1["assigned"].clone(), r.1["host_port"].clone()), (json!(true), json!(49154)));
        let after = on_answer(&Ctx { data: &d }, &call, Ok(r));
        let pubd = set_of(&after, "network_publish").unwrap();
        assert_eq!((pubd["host_port"].clone(), pubd["assigned"].clone(), pubd["busy"].clone()), (json!(49154), json!(true), json!(false)));
        assert!(after.iter().any(|a| matches!(a, Action::Notice { text, error: false, .. } if text == "Published web-app:5173 on 127.0.0.1:49154 (the port sbx picked)")), "{after:?}");
        // The panel: the new row says sbx picked its port, and the line under the form.
        d["network_ports"] = d["ports"]["web-app"].clone();
        d["network_publish"] = pubd;
        let (out, _, _) = render(d.clone(), None, 0);
        let all = out.join("\n");
        assert!(all.contains("port-picked-127.0.0.1:49154:5173\n") && all.contains("sbx picked"), "{all}");
        assert!(all.contains("Published 5173 on 127.0.0.1:49154, the port sbx picked."), "{all}");
        // While it runs, the button is busy.
        d["network_publish"] = json!({ "sandbox": "web-app", "sandbox_port": 5173, "busy": true });
        d["app"] = json!({});
        let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
        h.load("network", include_str!("../../screens/network.luau")).unwrap();
        let tree = h.render(&d).unwrap();
        assert!(find(&tree, "port-publish").is_some_and(|b| b.busy), "busy");
    }

    #[test]
    fn an_older_sbx_publishes_on_the_sandbox_port_and_says_so() {
        let mut d = cx_data();
        d["standin_sbx_old"] = true.into();
        let call = the_call(&act(&Ctx { data: &d }, &ask("port_publish", json!({ "sandbox": "web-app", "sandbox_port": 8080 }))).unwrap());
        let r = answer(&mut d, &call);
        let after = on_answer(&Ctx { data: &d }, &call, Ok(r));
        let pubd = set_of(&after, "network_publish").unwrap();
        assert_eq!((pubd["host_port"].clone(), pubd["assigned"].clone()), (json!(8080), json!(false)));
        assert!(pubd["note"].as_str().unwrap().contains("needs sbx 0.54"));
        d["network_ports"] = d["ports"]["web-app"].clone();
        d["network_publish"] = pubd;
        let (out, _, _) = render(d, None, 0);
        let all = out.join("\n");
        assert!(all.contains("automatic host port needs sbx 0.54") && !all.contains("port-picked-"), "{all}");
    }

    #[test]
    fn a_host_address_without_a_host_port_is_refused_as_text_and_nothing_is_asked() {
        let mut d = cx_data();
        d["network_for"] = "web-app".into();
        d["network_ports"] = d["ports"]["web-app"].clone();
        d["app"] = json!({});
        let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
        h.load("network", include_str!("../../screens/network.luau")).unwrap();
        h.render(&d).unwrap();
        h.act("sport", &J::Null, &json!("3000")).unwrap();
        h.act("hip", &J::Null, &json!("127.0.0.2")).unwrap();
        let asked = h.act("publish", &J::Null, &J::Null).unwrap();
        assert!(asked.is_null(), "nothing asked: {asked}");
        let (mut out, mut args) = (vec![], vec![]);
        walk(&h.render(&d).unwrap(), &mut out, &mut args);
        let all = out.join("\n");
        assert!(all.contains("sbx picks a host port only on the loopback address 127.0.0.1: name the host port"), "{all}");
        // With a host port, the same address is the fixed publish (its dialog).
        h.act("hport", &J::Null, &json!("4200")).unwrap();
        let asked = h.act("publish", &J::Null, &J::Null).unwrap();
        assert_eq!((asked["name"].as_str(), asked["args"]["host_ip"].as_str()), (Some("port_publish_fixed"), Some("127.0.0.2")), "{asked}");
        // 127.0.0.1 typed, no host port: sbx picks.
        h.act("sport", &J::Null, &json!("3000")).unwrap();
        h.act("hip", &J::Null, &json!("127.0.0.1")).unwrap();
        assert_eq!(h.act("publish", &J::Null, &J::Null).unwrap()["name"], "port_publish");
    }

    #[test]
    fn why_shows_the_rule_with_its_remove_or_an_unknown_answer() {
        let mut d = cx_data();
        let cx = Ctx { data: &d };
        // Only allowed rows have the button; the target is checked first.
        let (out, args, _) = render(d.clone(), None, 0);
        assert!(out.iter().any(|s| s == "why-api.anthropic.com:443") && !out.iter().any(|s| s == "why-registry.npmjs.org:443"), "{out:?}");
        assert_eq!(args.iter().find(|(k, _)| k == "why-api.anthropic.com:443").unwrap().1, json!("api.anthropic.com:443"));
        assert!(act(&cx, &ask("network_why", json!({ "sandbox": "web-app", "target": "x.com; rm -rf /" }))).is_err());
        let v = act(&cx, &ask("network_why", json!({ "sandbox": "web-app", "target": "api.anthropic.com:443" }))).unwrap();
        assert_eq!(set_of(&v, "network_why").unwrap()["busy"], true);
        let call = the_call(&v);
        assert_eq!(call.path, "/api/sandboxes/web-app/policy/check?target=api.anthropic.com%3A443");
        // A rule (the fixture's r-1): named, and the rules list's remove (an allow rule: direct).
        let r = answer(&mut d, &call);
        let w = why_answer(&Ctx { data: &d }, &call, Ok(r));
        assert_eq!((w["rule"].clone(), w["rule_id"].clone(), w["removable"].clone(), w["target"].clone()), (json!("r-1"), json!("r-1"), json!("rule_rm"), json!("api.anthropic.com:443")));
        d["network_why"] = w;
        let (out, args, h) = render(d.clone(), None, 0);
        let all = out.join("\n");
        assert!(all.contains("Rule: r-1") && all.contains("allowed by rule r-1 (api.anthropic.com)") && all.contains("why-rm"), "{all}");
        let rm = args.iter().find(|(k, _)| k == "why-rm").unwrap().1.clone();
        let asks = h.act("rule_rm", &rm, &J::Null).unwrap();
        assert_eq!((asks["name"].clone(), asks["args"]["rule"].clone()), (json!("network_rule_rm"), json!("r-1")));
        // sbx names no rule: said so, no remove.
        let mut d2 = cx_data();
        d2["policy_check"] = json!({ "web-app": { "api.anthropic.com:443": { "ok": true, "supported": true, "target": "api.anthropic.com:443", "allowed": J::Null, "rule": J::Null, "rule_id": J::Null, "raw": "unknown: no matching entry" } } });
        let r = answer(&mut d2, &call);
        d2["network_why"] = why_answer(&Ctx { data: &d2 }, &call, Ok(r));
        let (out, _, _) = render(d2, None, 0);
        let all = out.join("\n");
        assert!(all.contains("sbx does not say") && all.contains("No rule named") && all.contains("unknown: no matching entry") && !all.contains("why-rm"), "{all}");
        // An older sbx, and a daemon without the route: "needs sbx 0.54", never red.
        let mut d3 = cx_data();
        d3["standin_sbx_old"] = true.into();
        let r = answer(&mut d3, &call);
        let w = why_answer(&Ctx { data: &d3 }, &call, Ok(r));
        assert_eq!(w["supported"], false);
        assert!(w["note"].as_str().unwrap().contains("needs sbx 0.54"));
        let w = why_answer(&Ctx { data: &d3 }, &call, Ok((404, json!({ "ok": false, "error": "not found" }))));
        assert!(w["error"].is_null() && w["note"].as_str().unwrap().starts_with("needs sbx 0.54"), "{w}");
    }

    fn find<'a>(n: &'a crate::luau::Node, key: &str) -> Option<&'a crate::luau::Node> {
        if n.key.as_deref() == Some(key) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, key))
    }
}
