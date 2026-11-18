//! The Project panel's routes in the stand-in daemon and in fixture
//! daemons, with the daemon's rules (`crates/daemon/src/http/ui.rs`,
//! `sandbox_fs.rs`): artifacts stay inside `.sbxw-artifacts`, deletes only
//! under the writable roots and never on the protected folders, one code map
//! run per sandbox at a time, a lens brief of 400 characters at most.
//!
//! Fixture keys (each a route's answer, in the daemon's shape):
//! `artifacts` (`GET .../artifacts`), `files_dir` (`GET .../fs`, the
//! workspace), `files_recent` (`GET .../fs/recent`), `codemap`
//! (`GET /api/codemap`). Stand-in only: `files_tree` (more `GET .../fs`
//! answers, by path), `files_content` (file text by absolute path, for
//! `GET .../fs/file` and `.../artifacts/download`), `files_map_after` (the
//! artifacts answer once a code map run is done).
//!
//! File bodies are answered raw, in the stand-in's form (`{"$raw": text,
//! "$type": ...}`, or `{"$raw_hex": ...}` for a picture).

use super::view;
use crate::feat::{match_path, query, Standin};
use serde_json::{json, Value as J};

fn err(code: u16, m: impl Into<String>) -> Option<(u16, J)> {
    Some((code, json!({ "ok": false, "error": m.into() })))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A listing by path: `files_tree[path]`, else `files_dir` when it is that path (or none was asked).
fn listing(data: &J, path: &str) -> Option<J> {
    if let Some(l) = data["files_tree"].get(path).filter(|l| l.is_object()) {
        // The daemon says where things are on every listing (the workspace,
        // home, mounts, worktrees): the stand-in's from `files_dir`.
        let mut l = l.clone();
        for k in ["workspace", "artifacts", "home", "mounts", "worktrees", "offline"] {
            if l.get(k).is_none() && !data["files_dir"][k].is_null() {
                l[k] = data["files_dir"][k].clone();
            }
        }
        return Some(l);
    }
    let d = &data["files_dir"];
    (d.is_object() && (path.is_empty() || d["path"] == path)).then(|| d.clone())
}

/// A file's body, raw (`{"$raw": text}` or the fixture's own `{"$raw_hex": ...}`).
fn content(data: &J, path: &str, max: Option<usize>) -> Option<(u16, J)> {
    let ty = match view::ext(path).as_str() {
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "text/plain",
    };
    match &data["files_content"][path] {
        J::String(t) => {
            let t = match max {
                Some(m) if t.len() > m => {
                    let mut c = m;
                    while !t.is_char_boundary(c) {
                        c -= 1;
                    }
                    &t[..c]
                }
                _ => t.as_str(),
            };
            Some((200, json!({ "$raw": t, "$type": ty })))
        }
        J::Object(o) if o.contains_key("$raw_hex") => Some((200, J::Object(o.clone()))),
        _ => err(404, format!("{path}: No such file or directory")),
    }
}

/// `crates/daemon/src/http/ui.rs`, `upload_name`.
fn upload_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let safe: String = safe.trim_start_matches('.').chars().take(120).collect();
    if safe.is_empty() {
        "upload".into()
    } else {
        safe
    }
}

pub fn route(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    if (method, path.split('?').next().unwrap_or("")) == ("GET", "/api/codemap") {
        // A run settles 1.5 s after it started (its `done` event goes then),
        // its map then in the listing.
        let now = now_ms();
        let mut settled = false;
        if let Some(runs) = st.data["codemap"].as_array_mut() {
            for r in runs.iter_mut() {
                if r["phase"] == "writing" && r["started"].as_u64().is_some_and(|t| t + 1500 <= now) {
                    r["phase"] = "done".into();
                    r["ended"] = now.into();
                    settled = true;
                }
            }
        }
        if settled && st.data["files_map_after"].is_object() {
            st.data["artifacts"] = st.data["files_map_after"].clone();
        }
        return Some((200, if st.data["codemap"].is_array() { st.data["codemap"].clone() } else { json!([]) }));
    }
    if let Some(c) = match_path("/api/sandboxes/{name}/artifacts/download", path) {
        let _ = c;
        let rel = query(path, "path").unwrap_or_default();
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|p| p == ".." || p == ".") {
            return err(403, "invalid path");
        }
        let dir = st.data["artifacts"]["dir"].as_str().unwrap_or("").to_string();
        return content(st.data, &view::join(&dir, &rel), None);
    }
    if match_path("/api/sandboxes/{name}/fs/file", path).is_some() && method == "GET" {
        let p = query(path, "path").unwrap_or_default();
        let max = query(path, "max").and_then(|m| m.parse().ok());
        let head = query(path, "head").is_some();
        let size = st.data["files_content"][p.as_str()].as_str().map(str::len).unwrap_or_else(|| st.data["files_content"][p.as_str()]["$raw_hex"].as_str().map_or(0, |h| h.len() / 2));
        if let (Some(m), false) = (max, head) {
            if size > m {
                return err(413, format!("{p} is larger than {m} bytes"));
            }
        }
        return content(st.data, &p, max);
    }
    if match_path("/api/sandboxes/{name}/fs", path).is_some() && method == "GET" {
        let p = query(path, "path").unwrap_or_default();
        return match listing(st.data, &p) {
            Some(l) => Some((200, l)),
            None => err(404, format!("{p}: No such file or directory")),
        };
    }
    if let Some(c) = match_path("/api/sandboxes/{name}/fs/delete", path).filter(|_| method == "POST") {
        let _ = c;
        let p = body["path"].as_str().unwrap_or("").to_string();
        let roots = listing(st.data, "").unwrap_or(J::Null);
        if !view::can_delete(&p, &roots) {
            return err(403, format!("{p} is not something to delete from here"));
        }
        let parent = view::parent(&p);
        let name = p.rsplit('/').next().unwrap_or("").to_string();
        if let Some(tree) = st.data["files_tree"].as_object_mut() {
            for l in tree.values_mut() {
                if l["path"] == parent.as_str() {
                    if let Some(e) = l["entries"].as_array_mut() {
                        e.retain(|x| x["name"] != name.as_str());
                    }
                }
            }
        }
        if st.data["files_dir"]["path"] == parent.as_str() {
            if let Some(e) = st.data["files_dir"]["entries"].as_array_mut() {
                e.retain(|x| x["name"] != name.as_str());
            }
        }
        if let Some(f) = st.data["files_recent"]["files"].as_array_mut() {
            f.retain(|x| x["path"] != p.as_str());
        }
        let dir = st.data["artifacts"]["dir"].as_str().unwrap_or("").to_string();
        if let Some(rel) = p.strip_prefix(&format!("{dir}/")) {
            if let Some(e) = st.data["artifacts"]["entries"].as_array_mut() {
                e.retain(|x| x["path"] != rel);
            }
        }
        if let Some(c) = st.data["files_content"].as_object_mut() {
            c.remove(&p);
        }
        return Some((200, json!({ "ok": true, "path": p })));
    }
    for (pat, paste) in [("/api/sandboxes/{name}/upload", false), ("/api/sandboxes/{name}/paste-image", true)] {
        if match_path(pat, path).is_none() || method != "POST" {
            continue;
        }
        // The daemon's rules: an empty body is refused; the name becomes one
        // safe component (`upload_name`); a new folder each time.
        if body["$bytes"].as_u64().unwrap_or(0) == 0 {
            return err(400, "empty file");
        }
        let dest = if paste { format!("/tmp/sbxw-pastes/paste-{}.png", now_ms()) } else { format!("/tmp/sbxw-uploads/{}/{}", now_ms(), upload_name(&query(path, "name").unwrap_or_default())) };
        return Some((200, json!({ "ok": true, "path": dest })));
    }
    for (pat, lens) in [("/api/sandboxes/{name}/codemap", false), ("/api/sandboxes/{name}/codemap/lens", true)] {
        let Some(c) = match_path(pat, path).filter(|_| method == "POST") else { continue };
        let sb = c[0].1.clone();
        let running = st.data["sandboxes"].as_array().into_iter().flatten().any(|s| s["name"] == sb.as_str() && s["status"] == "running");
        if !running {
            return err(409, format!("'{sb}' is not running: start it, then ask again"));
        }
        if lens {
            let o = body["objective"].as_str().unwrap_or("").trim().to_string();
            if o.is_empty() || o.chars().count() > 400 {
                return err(400, "a lens needs a reader, in 400 characters at most");
            }
        }
        let runs = st.data["codemap"].as_array().cloned().unwrap_or_default();
        if runs.iter().any(|r| r["sandbox"] == sb.as_str() && r["phase"] == "writing") {
            return err(409, format!("'{sb}' is already writing {}", if lens { "a lens" } else { "its code map" }));
        }
        let kind = if lens { "lens" } else { "map" };
        let run = json!({ "sandbox": sb, "kind": kind, "phase": "writing", "started": now_ms() });
        let mut runs: Vec<J> = runs.into_iter().filter(|r| r["sandbox"] != sb.as_str()).collect();
        runs.push(run.clone());
        st.data["codemap"] = J::Array(runs.clone());
        (st.publish)("codemap", run.clone());
        // The scripted agent is quick: done 1.5 s later, its map in the listing.
        let mut done = run;
        done["phase"] = "done".into();
        done["ended"] = now_ms().into();
        st.publish_after(1500, "codemap", done);
        return Some((200, json!({ "ok": true, "run": { "sandbox": sb, "kind": kind, "phase": "writing" }, "dir": format!(".sbxw-artifacts/{}", if lens { "codemap-lenses" } else { "codemap" }) })));
    }
    None
}
