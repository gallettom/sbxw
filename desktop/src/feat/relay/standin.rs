//! The relay's routes in the stand-in daemon and in fixture daemons, with
//! the daemon's rules (`crates/core/src/relay.rs`): a screenshot or a virtual
//! Mac request is never routed, the asker is never its own target, something
//! has to go out on approval, a settled request takes nothing more. A routed
//! request is answered by a scripted agent a moment later.
//!
//! Fixture keys: `relay` (`{requests}`, as `GET /api/relay`),
//! `agent_sessions` (`GET /api/sessions`, a list of `SessionInfo`).

use super::round;
use crate::feat::{match_path, Standin};
use serde_json::{json, Value as J};

fn bad(m: impl Into<String>) -> Option<(u16, J)> {
    Some((400, json!({ "ok": false, "error": m.into() })))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What the scripted agent of `to` answers.
pub fn scripted_answer(req: &J, to: &str) -> String {
    let q = req["question"].as_str().unwrap_or("");
    if req["kind"] == "ask" {
        let rd = round::parse(q);
        return rd.questions.iter().map(|q| format!("{} : {}", q.id, if q.rec.is_empty() { "I cannot tell from this workspace" } else { q.rec.as_str() })).collect::<Vec<_>>().join("\n");
    }
    format!("From {to}'s workspace: `GET /v1/orders` returns `{{ \"orders\": [...], \"next\": \"cursor\" }}` in staging; `next` is null on the last page. (Scripted stand-in answer.)")
}

pub fn route(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let bare = path.split('?').next().unwrap_or("");
    if (method, bare) == ("GET", "/api/relay") {
        let v = &st.data["relay"];
        // A fixture's `{status, error}`: the read fails with the daemon's words.
        if let Some(code) = v["status"].as_u64().filter(|c| *c >= 400) {
            return Some((code as u16, json!({ "ok": false, "error": v["error"] })));
        }
        return Some((200, if v["requests"].is_array() { v.clone() } else { json!({ "requests": [] }) }));
    }
    if (method, bare) == ("POST", "/api/voice/anonymise") {
        // Fixture daemons only (the stand-in binary answers this itself), with its rule.
        let t = body["text"].as_str().unwrap_or("");
        let n = t.matches("Jeanne Martin").count();
        return Some((200, json!({ "ok": true, "text": t.replace("Jeanne Martin", "Personne A"), "masked": n })));
    }
    if (method, bare) == ("POST", "/api/answer") {
        let sb = body["sandbox"].as_str().unwrap_or("").to_string();
        let mode = body["mode"].as_str().unwrap_or("claude");
        let thread = mode.strip_prefix("thread:").map(str::to_string);
        let list = st.data["agent_sessions"].as_array_mut()?;
        let Some(i) = list.iter_mut().find(|i| i["sandbox"] == sb.as_str() && i["thread"].as_str().map(str::to_string) == thread && i["state"] == "attention") else {
            return Some((404, J::Null));
        };
        if i["answerable"] != true {
            return Some((409, J::Null));
        }
        let indices: Vec<u64> = body["indices"].as_array().map(|l| l.iter().filter_map(J::as_u64).collect()).unwrap_or_default();
        let picked: Vec<String> = indices.iter().zip(i["steps"].as_array().cloned().unwrap_or_default()).map(|(n, q)| q["options"][(*n as usize).saturating_sub(1)].as_str().unwrap_or("?").to_string()).collect();
        let o = i.as_object_mut().unwrap();
        o.remove("steps");
        o.remove("question");
        o.insert("state".into(), "working".into());
        o.insert("activity".into(), format!("Going on with {}", picked.join(", ")).into());
        let ev = J::Object(o.clone());
        (st.publish)("session", ev);
        return Some((200, J::Null));
    }
    let (id, action) = ["route", "approve", "deny"].iter().find_map(|a| match_path(&format!("/api/relay/{{id}}/{a}"), bare).map(|c| (c[0].1.clone(), *a)))?;
    if method != "POST" {
        return None;
    }
    let list = st.data["relay"]["requests"].as_array_mut()?;
    let Some(r) = list.iter_mut().find(|r| r["id"] == id.as_str()) else { return bad(format!("no request '{id}' — it may have been answered long ago")) };
    if matches!(r["state"].as_str(), Some("approved" | "denied")) {
        return bad(format!("request {id} is already settled"));
    }
    let now = now_ms();
    match action {
        "route" => {
            let to = body["to"].as_str().unwrap_or("").to_string();
            if r["kind"] == "screenshot" {
                return bad(format!("request {id} asks for a screenshot — no other sandbox can see this screen"));
            }
            if r["kind"] == "mac" {
                return bad(format!("request {id} asks you to approve something on the virtual Mac — only you can"));
            }
            if r["from"] == to.as_str() {
                return bad(format!("'{to}' is the sandbox that asked"));
            }
            r["to"] = to.clone().into();
            r["state"] = "routed".into();
            r.as_object_mut().unwrap().remove("answer");
            r.as_object_mut().unwrap().remove("note");
            r["updated_ms"] = now.into();
            let routed = r.clone();
            // The other agent answers a moment later; the person reviews it.
            r["answer"] = scripted_answer(&routed, &to).into();
            r["state"] = "answered".into();
            let answered = r.clone();
            (st.publish)("relay", routed);
            st.publish_after(900, "relay", answered);
        }
        "approve" => {
            let answer = body["answer"].as_str().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
            let images: Vec<&str> = body["images"].as_array().map(|l| l.iter().filter_map(J::as_str).filter(|u| !u.trim().is_empty()).collect()).unwrap_or_default();
            if images.len() > 6 {
                return bad("that is more than 6 images — send the ones that show it best");
            }
            if images.iter().any(|u| !(u.starts_with("data:image/png;base64,") || u.starts_with("data:image/jpeg;base64,") || u.starts_with("data:image/webp;base64,"))) {
                return bad("expected base64 image data");
            }
            if r["kind"] == "mac" {
                r["answer"] = "Booted the virtual Mac: open http://127.0.0.1:6080 to watch it (stand-in).".into();
            } else {
                if let Some(a) = answer {
                    r["answer"] = a.into();
                }
                if !images.is_empty() {
                    r["shot_count"] = images.len().into();
                }
                if r["answer"].is_null() && r["shot_count"].as_u64().unwrap_or(0) == 0 {
                    return bad("there is nothing to send yet");
                }
            }
            r["state"] = "approved".into();
            r["updated_ms"] = now.into();
            let v = r.clone();
            (st.publish)("relay", v);
        }
        _ => {
            r.as_object_mut().unwrap().remove("answer");
            r["shot_count"] = 0.into();
            r["state"] = "denied".into();
            r["updated_ms"] = now.into();
            let v = r.clone();
            (st.publish)("relay", v);
        }
    }
    Some((200, json!({ "ok": true })))
}
