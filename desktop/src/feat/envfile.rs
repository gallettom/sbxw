//! The environment file card (`assets/js/envfile.js`): a sandbox's
//! `sbxenv.yaml`, previewed (`POST /api/sandboxes/:name/envfile` with
//! `save: false`) and written beside its workspace (`save: true`).
//!
//! The one choice is the folder the file goes in: sbx resolves the file's
//! paths against the file itself, so the workspace is written relative to that
//! folder. The daemon answers the folder it used (`dir`): a folder that does
//! not contain the workspace becomes its parent, said under the field.
//!
//! Writing is direct, as in the web UI: it writes one file beside the
//! workspace, and refuses an existing one unless "overwrite" is ticked (the
//! daemon's 409, shown in the card).

use super::*;
use crate::intent::{Arg, Risk, Spec};
use serde_json::json;

pub const FEATURE: Feature = Feature {
    name: "envfile",
    screens: &[ScreenDecl { name: "envfile", src: include_str!("../../screens/envfile.luau"), slot: Slot::Modal { width: 600.0, light_dismiss: false } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

const INTENTS: &[Spec] = &[
    Spec { name: "envfile_show", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Opens a sandbox's environment file card (`sbxenv.yaml`) and previews it." },
    Spec {
        name: "envfile_preview",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("dir", Arg::Text, false)],
        doc: "Renders the file again with the folder it goes in (`dir`; empty: the daemon's choice), nothing written.",
    },
    Spec {
        name: "envfile_save",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("dir", Arg::Text, false), ("overwrite", Arg::Bool, false)],
        doc: "Writes the file in that folder; an existing file is replaced only with `overwrite`.",
    },
];

fn call(sb: &str, dir: &str, save: bool, force: bool) -> Call {
    let what = if save { format!("write the environment file of {sb}") } else { format!("preview the environment file of {sb}") };
    let c = Call::post(format!("/api/sandboxes/{}/envfile", crate::client::enc(sb)), json!({ "dir": dir, "save": save, "force": force }), what).tag(if save { "save" } else { "preview" });
    if save {
        c
    } else {
        c.quiet()
    }
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let sb = a.req("sandbox");
    cx.sandbox(sb).ok_or_else(|| format!("no sandbox named {sb} in the list"))?;
    let dir = a.s("dir").unwrap_or("").trim();
    Ok(match a.name {
        "envfile_show" => vec![
            Action::Set { key: "envfile".into(), value: json!({ "sandbox": sb, "loading": true }) },
            Action::OpenModal { screen: "envfile".into(), arg: J::String(sb.into()) },
            Action::Call(call(sb, "", false, false)),
        ],
        "envfile_preview" => vec![Action::Call(call(sb, dir, false, false))],
        "envfile_save" => vec![Action::Call(call(sb, dir, true, a.b("overwrite") == Some(true)))],
        other => return Err(format!("{other}: not an envfile intent")),
    })
}

/// The answer goes in `data.envfile`: `{sandbox, yaml, dest, dir, asked, written, error}`.
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let sb = crate::feat::match_path("/api/sandboxes/{sandbox}/envfile", &call.path).and_then(|c| c.into_iter().next()).map(|c| c.1).unwrap_or_default();
    // A slow answer for a sandbox the card no longer shows is dropped.
    let shown = cx.get("envfile")["sandbox"].as_str() == Some(sb.as_str());
    if !shown {
        return vec![];
    }
    let asked = call.body["dir"].as_str().unwrap_or("").trim_end_matches('/').to_string();
    let mut cur = cx.get("envfile").clone();
    cur["loading"] = J::Bool(false);
    let mut out = vec![];
    match r {
        Ok((c, b)) if (200..300).contains(&c) && b["ok"] != false => {
            for k in ["yaml", "dest", "dir", "written"] {
                cur[k] = b[k].clone();
            }
            cur["asked"] = J::String(asked);
            cur["error"] = J::Null;
            if let Some(w) = b["written"].as_str() {
                out.push(Action::notice(format!("Wrote {w}")));
            }
        }
        Ok((c, b)) => cur["error"] = J::String(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"))),
        Err(e) => cur["error"] = J::String(e),
    }
    out.insert(0, Action::Set { key: "envfile".into(), value: cur });
    out
}

/// "Env" on the pane bars of every sandbox with a workspace, as the web UI's pane bar.
fn extras(cx: &Ctx) -> Vec<Extra> {
    cx.get("sandboxes")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["workspace"].as_str().is_some_and(|w| !w.is_empty()))
        .filter_map(|s| s["name"].as_str())
        .map(|n| Extra {
            place: Place::Pane(n.to_string()),
            id: "envfile".into(),
            label: "Env".into(),
            icon: Some("file".into()),
            tone: "muted".into(),
            help: "Environment file (sbxenv.yaml): preview, copy, save".into(),
            count: None,
            intent: Some(("envfile_show".into(), json!({ "sandbox": n }))),
        })
        .collect()
}

// Fixture key `envfile_written`: paths already written (a second save without overwrite: 409).
fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    if method != "POST" {
        return None;
    }
    let sb = match_path("/api/sandboxes/{sandbox}/envfile", path)?.remove(0).1;
    let s = st.data["sandboxes"].as_array()?.iter().find(|s| s["name"] == sb.as_str())?.clone();
    let ws = s["workspace"].as_str().unwrap_or("").trim_end_matches('/').to_string();
    if ws.is_empty() {
        return Some((400, json!({ "ok": false, "error": format!("{sb} has no workspace folder") })));
    }
    let parent = ws.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
    let asked = body["dir"].as_str().unwrap_or("").trim_end_matches('/').to_string();
    let dir = if !asked.is_empty() && ws.starts_with(&format!("{asked}/")) { asked } else { parent };
    let rel = ws.strip_prefix(&format!("{dir}/")).unwrap_or(&ws);
    let yaml = format!("# sbxenv.yaml for {sb}, written by sbxw\nname: {sb}\nagent: {}\nworkspace: ./{rel}\n", s["agent"].as_str().unwrap_or("claude"));
    let dest = format!("{dir}/sbxenv.yaml");
    let mut written = J::Null;
    if body["save"] == true {
        let done = st.data["envfile_written"].as_array().is_some_and(|l| l.iter().any(|p| p == dest.as_str()));
        if done && body["force"] != true {
            return Some((409, json!({ "ok": false, "error": format!("{dest} already exists — tick overwrite to replace it") })));
        }
        if !st.data["envfile_written"].is_array() {
            st.data["envfile_written"] = json!([]);
        }
        st.data["envfile_written"].as_array_mut()?.push(J::String(dest.clone()));
        written = J::String(dest.clone());
    }
    Some((200, json!({ "ok": true, "yaml": yaml, "dest": dest, "dir": dir, "written": written })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_then_save_and_a_second_save_needs_overwrite() {
        let mut d = crate::fixture::Fixture::load(&crate::fixture::dir().join("envfile/basic.json")).unwrap().raw;
        let mut ev = |_: &str, _: J| {};
        let (c, b) = answer_from(&mut d, "POST", "/api/sandboxes/web-app/envfile", &json!({ "dir": "/elsewhere", "save": false }), &mut ev);
        assert_eq!(c, 200);
        assert_eq!(b["dir"], "/Users/me", "a folder above the workspace, not the one asked");
        assert!(b["yaml"].as_str().unwrap().contains("workspace: ./web"));
        assert_eq!(answer_from(&mut d, "POST", "/api/sandboxes/web-app/envfile", &json!({ "dir": "", "save": true }), &mut ev).0, 200);
        assert_eq!(answer_from(&mut d, "POST", "/api/sandboxes/web-app/envfile", &json!({ "dir": "", "save": true }), &mut ev).0, 409);
        assert_eq!(answer_from(&mut d, "POST", "/api/sandboxes/web-app/envfile", &json!({ "dir": "", "save": true, "force": true }), &mut ev).0, 200);
        // The answer lands in data.envfile only for the sandbox shown.
        d["envfile"] = json!({ "sandbox": "web-app", "loading": true });
        let cx = Ctx { data: &d };
        let c = call("web-app", "/Users/ana/", false, false);
        let a = on_answer(&cx, &c, Ok((200, json!({ "ok": true, "yaml": "x", "dest": "/Users/ana/sbxenv.yaml", "dir": "/Users/ana", "written": null }))));
        assert!(matches!(&a[0], Action::Set { value, .. } if value["asked"] == "/Users/ana" && value["loading"] == false));
        let other = call("docs", "", false, false);
        assert!(on_answer(&cx, &other, Ok((200, json!({ "ok": true })))).is_empty());
    }
}
