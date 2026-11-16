//! The stand-in daemon's create routes (`src/bin/standin.rs`): a small
//! folder tree, favourites, the carve preview, and bring-ups that report
//! their steps on the stream as the daemon does, so a whole creation runs
//! with no sbx.
//!
//! Fixture keys (all optional), in the fixture's `daemon` object:
//! * `fs_tree`: `{folder: [sub-folder names]}` (default: a small tree under `/home/you`);
//! * keys in their route's shape: `favourites` (`GET /api/fs/favourites`),
//!   `kit_mounts` (`GET /api/kit-mounts`), `template_status`; `remote_hosts`
//!   is desktop-setup's Data (`GET /api/remote-hosts`, `{ok, hosts}`), served
//!   by the core, not here;
//! * `create_consent`: `{config, grants}`: a create is refused with 409
//!   until those exact lines come back as `approved`;
//! * `create_fail`: `{name: reason}`: that bring-up fails once at its kits
//!   step, then works on Retry;
//! * `create_step_ms`: how long each step takes (default 350).

use super::super::Standin;
use serde_json::{json, Value as J};
use std::time::Duration;

/// Each step of a stand-in bring-up takes this long.
pub const STEP: Duration = Duration::from_millis(350);
pub const HOME: &str = "/home/you";

fn default_tree() -> J {
    json!({
        "/home/you": ["dev", "notes"],
        "/home/you/dev": ["docs", "shop"],
        "/home/you/dev/shop": ["backend", "frontend", "secrets"],
        "/home/you/dev/shop/backend": ["src"],
        "/home/you/dev/shop/frontend": ["src"],
        "/home/you/dev/docs": [],
        "/home/you/notes": [],
    })
}

fn query(path: &str) -> (&str, Vec<(String, String)>) {
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let kv = q
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (k, v) = s.split_once('=').unwrap_or((s, ""));
            (k.to_string(), decode(v))
        })
        .collect();
    (p, kv)
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `{ok, favourites: [{path, name, missing}]}`, `missing` checked against the tree.
fn favourites(d: &J) -> J {
    let tree = d.get("fs_tree").cloned().unwrap_or_else(default_tree);
    let list: Vec<J> = d["favourites"]["favourites"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .map(|p| json!({ "path": p, "name": p.rsplit('/').next().unwrap_or(p), "missing": tree.get(p).is_none() }))
        .collect();
    json!({ "ok": true, "favourites": list })
}

/// The create routes; `None` for any other.
pub fn route(s: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let (p, q) = query(path);
    let arg = |k: &str| q.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    Some(match (method, p) {
        ("GET", "/api/fs") => {
            let tree = s.data.get("fs_tree").cloned().unwrap_or_else(default_tree);
            let dir = arg("path").unwrap_or_else(|| HOME.into());
            let dir = dir.trim_end_matches('/').to_string();
            let Some(kids) = tree.get(&dir).and_then(J::as_array) else { return Some((200, json!({ "path": dir, "parent": parent(&dir), "entries": [] }))) };
            let entries: Vec<J> = kids.iter().filter_map(J::as_str).map(|k| json!({ "name": k, "path": format!("{dir}/{k}") })).collect();
            (200, json!({ "path": dir, "parent": parent(&dir), "entries": entries }))
        }
        ("POST", "/api/fs/pick") => (200, json!({ "ok": true, "path": format!("{HOME}/dev/docs") })),
        ("GET", "/api/fs/favourites") => (200, favourites(s.data)),
        ("POST", "/api/fs/favourites") => {
            let path = body["path"].as_str().unwrap_or("").to_string();
            let tree = s.data.get("fs_tree").cloned().unwrap_or_else(default_tree);
            if body["favourite"] == true && tree.get(&path).is_none() {
                return Some((400, json!({ "ok": false, "error": format!("'{path}' is not a folder") })));
            }
            let mut list: Vec<J> = s.data["favourites"]["favourites"].as_array().cloned().unwrap_or_default();
            list.retain(|x| x["path"] != path.as_str());
            if body["favourite"] == true {
                list.push(json!({ "path": path }));
            }
            s.data["favourites"] = json!({ "ok": true, "favourites": list });
            (200, favourites(s.data))
        }
        ("GET", "/api/kit-mounts") => (200, s.data.get("kit_mounts").cloned().unwrap_or(json!({ "mounts": [] }))),
        ("GET", "/api/template/status") => (200, s.data.get("template_status").cloned().unwrap_or(json!({ "auto": false }))),
        ("POST", "/api/carve/preview") => (200, preview(s.data, body)),
        ("POST", "/api/sandboxes/create") => bring_up(s, body, false),
        ("POST", "/api/sandboxes/chat") => bring_up(s, body, true),
        _ => return None,
    })
}

fn parent(dir: &str) -> J {
    match dir.rfind('/') {
        Some(0) if dir.len() > 1 => "/".into(),
        Some(i) if i > 0 => dir[..i].into(),
        _ => J::Null,
    }
}

/// Strict restrictions hide the files next to them; one outside the
/// workspace is refused, as the daemon's planner does.
fn preview(d: &J, b: &J) -> J {
    let main = b["path"].as_str().unwrap_or("");
    let mut unshared = vec![];
    for c in b["carve"].as_array().cloned().unwrap_or_default() {
        let p = c["path"].as_str().unwrap_or("");
        let ok_parent = std::iter::once(main.to_string()).chain(b["mounts"].as_array().cloned().unwrap_or_default().iter().filter(|m| m["readonly"] == false).filter_map(|m| m["path"].as_str().map(str::to_string))).any(|r| super::model::inside(p, &r));
        if !ok_parent {
            return json!({ "ok": false, "error": format!("{p} is not inside a read-write folder of this sandbox") });
        }
        if c["enforce"] == "strict" {
            let dir = &p[..p.rfind('/').unwrap_or(0)];
            unshared.push(format!("{dir}/README.md"));
        }
    }
    let _ = d;
    json!({ "ok": true, "mounts": [{ "host": main, "target": main, "readonly": false, "mask": false }], "unshared": unshared, "warnings": [], "stub": false })
}

fn bring_up(s: &mut Standin, b: &J, chat: bool) -> (u16, J) {
    let name = b["name"].as_str().unwrap_or("").to_string();
    if !super::model::valid_name(name.rsplit('~').next().unwrap_or("")) {
        return (400, json!({ "ok": false, "error": format!("'{name}' is not a valid sandbox name") }));
    }
    if s.data["sandboxes"].as_array().is_some_and(|a| a.iter().any(|x| x["name"] == name.as_str())) {
        return (400, json!({ "ok": false, "error": format!("a sandbox named {name} already exists") }));
    }
    if !chat {
        if let Some(c) = s.data.get("create_consent").cloned() {
            if b["approved"] != c["grants"] {
                return (409, json!({ "ok": false, "error": "this sandbox's sbxw.toml asks for host access not approved yet", "consent": c }));
            }
        }
    }
    let mut steps = vec![("workspace", "Prepare the workspace"), ("create", "Create the sandbox"), ("boot", "Start it"), ("kits", "Install kits")];
    if !chat && b["ports"].as_array().is_some_and(|p| !p.is_empty()) {
        steps.push(("ports", "Publish ports"));
    }
    let ev = |kind: J| {
        let mut v = json!({ "sandbox": name });
        v.as_object_mut().unwrap().extend(kind.as_object().unwrap().clone());
        v
    };
    let ms = s.data["create_step_ms"].as_u64().unwrap_or(STEP.as_millis() as u64);
    s.publish_after(0, "provision", ev(json!({ "kind": "plan", "steps": steps.iter().map(|(i, l)| json!({ "id": i, "label": l })).collect::<Vec<_>>() })));
    let fail = s.data["create_fail"][&name].as_str().map(str::to_string);
    for (id, _) in &steps {
        s.publish_after(ms, "provision", ev(json!({ "kind": "started", "id": id })));
        if *id == "create" {
            s.publish_after(ms / 2, "provision", ev(json!({ "kind": "detail", "text": "Pulling image docker/sandbox-templates:claude-code 63%" })));
        }
        if *id == "kits" {
            if let Some(why) = &fail {
                s.data["create_fail"].as_object_mut().unwrap().remove(&name);
                s.publish_after(ms, "provision", ev(json!({ "kind": "done", "error": why })));
                return (502, json!({ "ok": false, "error": why }));
            }
        }
    }
    s.publish_after(ms, "provision", ev(json!({ "kind": "done" })));
    let ws = if chat { format!("/tmp/sbxw-chat/{name}") } else { b["path"].as_str().unwrap_or("").to_string() };
    if let Some(a) = s.data["sandboxes"].as_array_mut() {
        a.push(json!({ "name": name, "agent": "claude", "status": "running", "workspace": ws, "chat": chat }));
    }
    let mut out = json!({ "ok": true, "warnings": [] });
    if chat {
        out["name"] = name.into();
    }
    (200, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(data: &mut J, method: &str, path: &str, body: J) -> ((u16, J), Vec<J>) {
        let mut evs = vec![];
        let r = {
            let mut publish = |_: &str, v: J| evs.push(v);
            crate::feat::answer_from(data, method, path, &body, &mut publish)
        };
        (r, evs)
    }

    #[test]
    fn folders_and_favourites() {
        let mut d = json!({ "sandboxes": [] });
        let ((_, v), _) = run(&mut d, "GET", "/api/fs", J::Null);
        assert_eq!(v["path"], HOME);
        assert_eq!(v["entries"][0]["path"], "/home/you/dev");
        let ((_, v), _) = run(&mut d, "GET", "/api/fs?path=%2Fhome%2Fyou%2Fdev%2Fshop", J::Null);
        assert_eq!(v["entries"].as_array().unwrap().len(), 3);
        assert_eq!(v["parent"], "/home/you/dev");
        let ((s, _), _) = run(&mut d, "POST", "/api/fs/favourites", json!({ "path": "/nope", "favourite": true }));
        assert_eq!(s, 400);
        let ((_, v), _) = run(&mut d, "POST", "/api/fs/favourites", json!({ "path": "/home/you/dev", "favourite": true }));
        assert_eq!(v["favourites"][0]["name"], "dev");
    }

    #[test]
    fn a_bring_up_reports_its_steps_and_can_fail_once() {
        let mut d = json!({ "sandboxes": [], "create_fail": { "web": "sbx: no space left on device" }, "create_consent": { "config": "/w/sbxw.toml", "grants": ["mount /etc"] } });
        let ((s, v), _) = run(&mut d, "POST", "/api/sandboxes/create", json!({ "name": "web", "path": "/w", "approved": [] }));
        assert_eq!((s, v["consent"]["grants"][0].as_str()), (409, Some("mount /etc")));
        let ((s, v), evs) = run(&mut d, "POST", "/api/sandboxes/create", json!({ "name": "web", "path": "/w", "approved": ["mount /etc"] }));
        assert_eq!((s, v["error"].as_str()), (502, Some("sbx: no space left on device")));
        assert_eq!(evs.last().unwrap()["kind"], "done");
        let ((s, _), evs) = run(&mut d, "POST", "/api/sandboxes/create", json!({ "name": "web", "path": "/w", "approved": ["mount /etc"] }));
        assert_eq!(s, 200);
        assert_eq!(evs[0]["kind"], "plan");
        assert_eq!(d["sandboxes"][0]["name"], "web");
        let ((s, v), _) = run(&mut d, "POST", "/api/sandboxes/create", json!({ "name": "web", "path": "/w", "approved": ["mount /etc"] }));
        assert!(s == 400 && v["error"].as_str().unwrap().contains("already exists"));
    }
}
