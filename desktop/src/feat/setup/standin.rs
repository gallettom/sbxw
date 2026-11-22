//! The stand-in daemon's Setup routes (`src/bin/standin.rs`): answers from
//! the fixture's `daemon` object, changed as the daemon would change it.
//!
//! Like the daemon, it never keeps a secret's value: a token or a value is
//! checked, then only its name, or "set", is recorded. A test reads the
//! whole data object after a set to prove it.
//!
//! Fixture keys (each one route's answer): `secrets`, `github_token`, `kits`,
//! `my_skills`, `mcp`, `remote_hosts`, `voice`, plus `sandbox_skills_of`
//! (sandbox name → `GET /api/sandboxes/:name/skills`) and `mcp_inspect_of`
//! (server → its inspect text), and `standin_voice_steps` (true: each
//! `GET /api/voice` moves a running download a quarter on).

use crate::feat::Standin;
use serde_json::{json, Value as J};

fn ok() -> Option<(u16, J)> {
    Some((200, json!({ "ok": true })))
}

fn bad(msg: impl Into<String>) -> Option<(u16, J)> {
    Some((400, json!({ "ok": false, "error": msg.into() })))
}

/// A route's answer from the fixture: one holding `{ok: false, error}` is
/// served as the daemon serves a failure (503), so the core files it under
/// `data.app.errors`.
fn served(v: &J) -> Option<(u16, J)> {
    Some((if v["ok"] == false { 503 } else { 200 }, v.clone()))
}

fn arr<'a>(data: &'a mut J, key: &str, inner: &str) -> &'a mut Vec<J> {
    if !data[key].is_object() {
        data[key] = json!({ "ok": true });
    }
    if !data[key][inner].is_array() {
        data[key][inner] = json!([]);
    }
    data[key][inner].as_array_mut().expect("array")
}

fn decode(seg: &str) -> String {
    crate::pct::decode(seg, false)
}

/// One step of a running voice download per request, so a scripted run sees
/// progress, then the models installed.
fn voice_tick(st: &mut Standin) {
    // Only where the fixture asks (`standin_voice_steps`): a fixture showing
    // a download in progress stays as it is.
    if st.data["standin_voice_steps"] != true {
        return;
    }
    let v = &mut st.data["voice"];
    if v["job"]["state"] != "running" {
        return;
    }
    let total = v["job"]["total"].as_u64().unwrap_or(1);
    let done = (v["job"]["done"].as_u64().unwrap_or(0) + total / 4).min(total);
    v["job"]["done"] = json!(done);
    if done >= total {
        v["job"] = json!({ "state": "done", "model": v["job"]["model"], "file": "", "done": total, "total": total });
        if let Some(ms) = v["models"].as_array_mut() {
            for m in ms {
                m["installed"] = json!({ "state": "ready" });
            }
        }
        v["ready"] = json!(true);
    }
    let status = without_ok(&st.data["voice"]);
    (st.publish)("voice", status);
}

fn without_ok(v: &J) -> J {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("ok");
    }
    v
}

pub fn answer(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let path = path.split('?').next().unwrap_or(path);
    let segs: Vec<String> = path.trim_start_matches('/').split('/').map(decode).collect();
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    let mine = matches!(s.as_slice(), ["api", "secrets", ..] | ["api", "github-token", ..] | ["api", "kits"] | ["api", "my-skills", ..] | ["api", "mcp", ..] | ["api", "remote-hosts", ..] | ["api", "voice", ..])
        || matches!(s.as_slice(), ["api", "sandboxes", _, "skills", ..] | ["api", "sandboxes", _, "mcp", _]);
    if !mine {
        return None;
    }
    let get = method.eq_ignore_ascii_case("GET");
    let d = &mut *st.data;
    match (get, s.as_slice()) {
        (true, ["api", "secrets"]) => served(&d["secrets"]),
        (true, ["api", "github-token"]) => served(&d["github_token"]),
        (true, ["api", "kits"]) => served(&d["kits"]),
        (true, ["api", "my-skills"]) => served(&d["my_skills"]),
        (true, ["api", "mcp"]) => served(&d["mcp"]),
        (true, ["api", "remote-hosts"]) => served(&d["remote_hosts"]),
        (true, ["api", "voice"]) => {
            voice_tick(st);
            served(&st.data["voice"])
        }
        (true, ["api", "sandboxes", sb, "skills"]) => {
            served(&d["sandbox_skills_of"].get(*sb).cloned().unwrap_or_else(|| json!({ "ok": true, "skills": [] })))
        }
        (true, ["api", "mcp", server]) => {
            let text = d["mcp_inspect_of"][*server].as_str().unwrap_or("").to_string();
            Some((200, json!({ "ok": true, "text": text })))
        }

        // ── Secrets: names only, never a value ───────────────────────────
        (false, ["api", "github-token"]) => {
            let token = body["token"].as_str().unwrap_or("").trim();
            if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return bad("a GitHub token is one word with no spaces — paste it again");
            }
            eprintln!("standin: github token stored ({} characters, not kept)", token.len());
            github(d, true);
            ok()
        }
        (false, ["api", "github-token", "gh"]) => {
            if d["github_token"]["gh"].is_null() {
                return bad("no GitHub CLI found — install it, run `gh auth login`, then try again");
            }
            github(d, true);
            ok()
        }
        (false, ["api", "github-token", "rm"]) => {
            github(d, false);
            ok()
        }
        (false, ["api", "secrets", "custom"]) => {
            let env = body["env"].as_str().unwrap_or("").trim().to_string();
            let hosts = body["hosts"].as_array().map(Vec::len).unwrap_or(0);
            if hosts == 0 {
                return bad("name at least one host the secret is for (api.example.com, *.example.com)");
            }
            if !super::env_ok(&env) {
                return bad("the variable name is letters, digits and _, not starting with a digit");
            }
            if body["value"].as_str().unwrap_or("").is_empty() {
                return bad("paste the secret's value");
            }
            eprintln!("standin: custom secret ${env} stored for {hosts} host(s) (value not kept)");
            let list = arr(d, "secrets", "secrets");
            list.retain(|x| x["name"] != env.as_str());
            list.push(json!({ "scope": "global", "kind": "custom", "name": env }));
            ok()
        }
        (false, ["api", "secrets", "rm"]) => {
            let name = body["name"].as_str().unwrap_or("").to_string();
            arr(d, "secrets", "secrets").retain(|x| x["name"] != name.as_str());
            if name == "github" {
                d["github_token"]["set"] = json!(false);
            }
            ok()
        }

        // ── Kits ─────────────────────────────────────────────────────────
        (false, ["api", "kits"]) => {
            let path = body["path"].as_str().unwrap_or("");
            let Some(f) = d["kits"]["files"].as_array_mut().and_then(|fs| fs.iter_mut().find(|f| f["path"] == path)) else {
                return bad(format!("{path} is not an sbxw.toml sbxw uses"));
            };
            f["kits"] = body["kits"].clone();
            f["exists"] = json!(true);
            eprintln!("standin: kits of {path} = {}", body["kits"]);
            Some((200, json!({ "ok": true, "warnings": [] })))
        }

        // ── Skills ───────────────────────────────────────────────────────
        (false, ["api", "my-skills", "rm"]) => {
            let name = body["name"].as_str().unwrap_or("").to_string();
            arr(d, "my_skills", "skills").retain(|x| x["name"] != name.as_str());
            ok()
        }
        (false, ["api", "sandboxes", sb, "skills", "keep"]) => {
            let name = body["name"].as_str().unwrap_or("").to_string();
            let found = d["sandbox_skills_of"][*sb]["skills"].as_array().and_then(|l| l.iter().find(|x| x["name"] == name.as_str()).cloned());
            let Some(found) = found else { return bad(format!("{sb} has no skill named {name}")) };
            let list = arr(d, "my_skills", "skills");
            let had = list.iter().any(|x| x["name"] == name.as_str());
            if had && body["replace"] != true {
                return bad(format!("you already keep /{name}: replace it"));
            }
            list.retain(|x| x["name"] != name.as_str());
            list.push(json!({ "name": name, "description": found["description"], "sandbox": sb, "path": found["path"], "saved": 1_763_200_000u64 }));
            Some((200, json!({ "ok": true, "warnings": [] })))
        }

        // ── MCP ──────────────────────────────────────────────────────────
        (false, ["api", "mcp"]) => {
            let spec = &body["spec"];
            let name = spec["name"].as_str().unwrap_or("").to_string();
            if !crate::feat::mcp::mcp_name_ok(&name) {
                return bad("not a server name");
            }
            let secrets = body["secrets"].as_object().map(|m| m.len()).unwrap_or(0);
            let list = arr(d, "mcp", "servers");
            if let Some(old) = body["replace"].as_str() {
                list.retain(|x| x["name"] != old);
            } else if list.iter().any(|x| x["name"] == name.as_str()) {
                return bad(format!("{name} is already registered"));
            }
            let kind = if spec["kind"] == "stdio" { "stdio" } else { "remote" };
            list.push(json!({ "name": name, "type": kind }));
            eprintln!("standin: MCP server {name} ({kind}) registered, {secrets} secret(s) (values not kept)");
            ok()
        }
        (false, ["api", "mcp", server, "rm"]) => {
            let server = server.to_string();
            arr(d, "mcp", "servers").retain(|x| x["name"] != server.as_str());
            ok()
        }
        (false, ["api", "mcp", server, "auth"]) => Some((200, json!({ "ok": true, "text": format!("signed in to {server}") }))),
        (false, ["api", "sandboxes", sb, "mcp", verb]) => {
            eprintln!("standin: {verb} {} in {sb}", body["server"]);
            ok()
        }

        // ── Remote hosts ─────────────────────────────────────────────────
        (false, ["api", "remote-hosts", "key"]) => {
            let host = body["host"].as_str().or(body["ssh"].as_str()).unwrap_or("").to_string();
            let port = body["port"].as_u64().unwrap_or(22);
            let known_as = if port == 22 { host.clone() } else { format!("[{host}]:{port}") };
            if host.contains("known") {
                return Some((200, json!({ "ok": true, "known_as": known_as, "known": true, "fingerprints": [] })));
            }
            Some((200, json!({ "ok": true, "known_as": known_as, "known": false, "fingerprints": [
                format!("256 SHA256:Qm9vdGgxMjM0NTY3ODlhYmNkZWZnaGlqa2xtbm9w {host} (ED25519)"),
                format!("3072 SHA256:c3RhbmRpbi1yc2Eta2V5LWZvci10ZXN0aW5nLW9ubHk {host} (RSA)"),
            ] })))
        }
        (false, ["api", "remote-hosts", "add"]) | (false, ["api", "remote-hosts", _, "edit"]) => {
            let editing = if s.len() == 4 { Some(s[2].to_string()) } else { None };
            let name = editing.clone().unwrap_or_else(|| body["name"].as_str().unwrap_or("").to_string());
            if name.is_empty() {
                return bad("give the host a name");
            }
            let list = arr(d, "remote_hosts", "hosts");
            let old = list.iter().position(|h| h["name"] == name.as_str());
            match (&editing, old) {
                (None, Some(_)) => return bad(format!("{name} is already in hosts.toml")),
                (Some(_), None) => return bad(format!("no host {name}")),
                _ => {}
            }
            let sandboxes = old.map(|i| list[i]["sandboxes"].clone()).unwrap_or(json!(0));
            let mut h = json!({ "name": name, "link": "never", "sandboxes": sandboxes, "trust_secrets": body["trust_secrets"] == true });
            for k in ["ssh", "host", "user", "port", "key", "agent_port", "sbx_path"] {
                if !body[k].is_null() {
                    h[k] = body[k].clone();
                }
            }
            h["address"] = json!(body["ssh"].as_str().or(body["host"].as_str()).unwrap_or(""));
            eprintln!("standin: host {name} saved (fingerprints confirmed: {})", body["fingerprints"].as_array().map(Vec::len).unwrap_or(0));
            match old {
                Some(i) => list[i] = h,
                None => list.push(h),
            }
            ok()
        }
        (false, ["api", "remote-hosts", host, "rm"]) => {
            let host = host.to_string();
            let list = arr(d, "remote_hosts", "hosts");
            let held = list.iter().find(|h| h["name"] == host.as_str()).and_then(|h| h["sandboxes"].as_u64()).unwrap_or(0);
            if held > 0 && body["force"] != true {
                return bad(format!("{host} still holds {held} sandbox record(s)"));
            }
            list.retain(|h| h["name"] != host.as_str());
            ok()
        }
        (false, ["api", "remote-hosts", host, "test"]) => {
            let host = host.to_string();
            if let Some(h) = arr(d, "remote_hosts", "hosts").iter_mut().find(|h| h["name"] == host.as_str()) {
                h["link"] = json!("up");
                h["latency_ms"] = json!(42);
            }
            Some((200, json!({ "ok": true, "version": "sbx 0.53.0", "latency_ms": 42 })))
        }
        (false, ["api", "remote-hosts", "reload"]) => ok(),

        // ── Voice ────────────────────────────────────────────────────────
        (false, ["api", "voice", "install"]) => {
            if d["voice"]["compiled"] == false {
                return Some((409, json!({ "ok": false, "error": "this sbxw was built without voice" })));
            }
            let total = d["voice"]["approx_mb"].as_u64().unwrap_or(2000) * 1_000_000;
            d["voice"]["job"] = json!({ "state": "running", "model": "parakeet-tdt-0.6b-v3", "file": "model.safetensors", "done": 0, "total": total });
            let status = without_ok(&d["voice"]);
            (st.publish)("voice", status);
            Some((200, st.data["voice"].clone()))
        }
        (false, ["api", "voice", "cancel"]) => {
            if d["voice"]["job"]["state"] != "running" {
                return Some((409, json!({ "ok": false, "error": "no download is running" })));
            }
            d["voice"]["job"]["state"] = json!("cancelled");
            let status = without_ok(&d["voice"]);
            (st.publish)("voice", status);
            ok()
        }
        (false, ["api", "voice", "rm"]) => {
            if let Some(ms) = d["voice"]["models"].as_array_mut() {
                for m in ms {
                    m["installed"] = json!({ "state": "missing" });
                }
            }
            d["voice"]["ready"] = json!(false);
            d["voice"]["job"] = J::Null;
            let status = without_ok(&d["voice"]);
            (st.publish)("voice", status);
            Some((200, st.data["voice"].clone()))
        }
        (false, ["api", "voice", "warm"]) => ok(),
        _ => None,
    }
}

fn github(d: &mut J, set: bool) {
    d["github_token"]["set"] = json!(set);
    let list = arr(d, "secrets", "secrets");
    list.retain(|x| x["name"] != "github");
    if set {
        list.push(json!({ "scope": "global", "kind": "service", "name": "github" }));
    }
}
