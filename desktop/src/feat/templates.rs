//! Templates: the web UI's Templates dialog (`assets/js/templates.js`). The
//! saved templates (`GET /api/templates`, `sbx template ls` with sbxw's own
//! records), disk usage (`GET /api/disk-usage`, `sbx df`), save a sandbox as a
//! template, remove one.
//!
//! Saving is direct, as in the web UI (the daemon takes the sandbox's secrets
//! out first; it can take minutes). Removing asks first in the web UI, so here
//! it is of risk Approval: the core's dialog names the tag the daemon listed,
//! and says what happens to sandboxes made from it.

use super::*;
use crate::intent::{Arg, Risk, Spec};
use serde_json::json;

pub const FEATURE: Feature = Feature {
    name: "templates",
    data: &[
        Data { key: "templates", path: "/api/templates", on: &[], every: 0, lazy: true },
        Data { key: "disk_usage", path: "/api/disk-usage", on: &[], every: 0, lazy: true },
    ],
    screens: &[ScreenDecl { name: "templates", src: include_str!("../../screens/templates.luau"), slot: Slot::Modal { width: 560.0 } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    standin: Some(standin),
    ..Feature::EMPTY
};

const INTENTS: &[Spec] = &[
    Spec { name: "templates_show", risk: Risk::Direct, args: &[("sandbox", Arg::Str, false)], doc: "Opens the Templates dialog (saved templates, disk usage, save a sandbox as one); `sandbox`: the one preselected." },
    Spec {
        name: "template_save",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("tag", Arg::Str, true)],
        doc: "Saves a sandbox as template `tag` (`POST /api/templates/save`); the daemon takes its secrets out first. Can take minutes.",
    },
    Spec { name: "template_rm", risk: Risk::Approval("Remove a template"), args: &[("tag", Arg::Str, true)], doc: "Removes a saved template (`POST /api/templates/rm`), after the core's dialog." },
    Spec { name: "disk_usage", risk: Risk::Direct, args: &[("verbose", Arg::Bool, false)], doc: "Reads `sbx df` again (`GET /api/disk-usage`), with every item when `verbose`." },
];

/// A tag as sbx takes it: no blank, no leading dash.
fn valid_tag(t: &str) -> bool {
    !t.is_empty() && !t.starts_with('-') && !t.contains(char::is_whitespace) && t.len() <= 128
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    Ok(match a.name {
        "templates_show" => vec![
            Action::OpenModal { screen: "templates".into(), arg: a.s("sandbox").map(|s| J::String(s.into())).unwrap_or(J::Null) },
            Action::Refresh("templates"),
            Action::Refresh("disk_usage"),
        ],
        "template_save" => {
            let (sb, tag) = (a.req("sandbox"), a.req("tag").trim());
            cx.sandbox(sb).ok_or_else(|| format!("no sandbox named {sb} in the list"))?;
            if !valid_tag(tag) {
                return Err(format!("'{tag}' is not a template tag: no spaces, not starting with -"));
            }
            vec![
                Action::Set { key: "templates_saving".into(), value: json!({ "sandbox": sb, "tag": tag }) },
                Action::Call(Call::post("/api/templates/save", json!({ "sandbox": sb, "tag": tag }), format!("save {sb} as template {tag}")).timeout(1800).then(&["templates", "disk_usage"]).tag("save")),
            ]
        }
        "template_rm" => {
            let tag = a.req("tag");
            let list = cx.get("templates")["templates"].as_array().cloned().unwrap_or_default();
            let t = list.iter().find(|t| t["tag"] == tag).ok_or_else(|| format!("no saved template {tag}"))?;
            let mut c = Confirm::new(format!("Remove template {tag}?"), Call::post("/api/templates/rm", json!({ "tag": tag }), format!("remove template {tag}")).then(&["templates", "disk_usage"]))
                .line(format!("Template: {tag}{}", t["size"].as_str().map(|s| format!(" ({s})")).unwrap_or_default()))
                .line("Sandboxes already made from it keep working; new ones can no longer start from it.");
            if t["configured"] == true {
                c = c.line("sbxw.toml names it as the template new sandboxes start from: they will fail to start until it is saved again or the line changes.");
            }
            vec![c.labels("Remove", "Cancel").danger().into_action()]
        }
        "disk_usage" => vec![Action::Fetch { key: "disk_usage".into(), path: if a.b("verbose") == Some(true) { "/api/disk-usage?verbose=true".into() } else { "/api/disk-usage".into() } }],
        other => return Err(format!("{other}: not a templates intent")),
    })
}

fn on_answer(_cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    let mut v = vec![];
    if call.tag == "save" {
        v.push(Action::Set { key: "templates_saving".into(), value: J::Null });
    }
    v.push(match r {
        Ok((c, body)) if (200..300).contains(&c) && body["ok"] != false => match body["warnings"].as_array().and_then(|w| w.last()).and_then(J::as_str) {
            Some(w) => Action::error(format!("{}: done, but {w}", call.what)),
            None => Action::notice(format!("{}: done", call.what)),
        },
        Ok((c, body)) => Action::error(format!("{}: {}", call.what, body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}")))),
        Err(e) => Action::error(format!("{}: {e}", call.what)),
    });
    v
}

// Fixture keys: `templates` ({ok, templates, configured}), `disk_usage` ({ok, text}).
fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let p = path.split('?').next().unwrap_or("");
    match (method, p) {
        ("GET", "/api/templates") => {
            let v = st.data["templates"].clone();
            Some((200, if v.is_null() { json!({ "ok": true, "templates": [], "configured": null }) } else { v }))
        }
        ("GET", "/api/disk-usage") => {
            let v = st.data["disk_usage"].clone();
            let mut v = if v.is_null() { json!({ "ok": true, "text": "TYPE        TOTAL   ACTIVE   SIZE     RECLAIMABLE\nImages      3       2        4.1GB    1.2GB (29%)\nContainers  4       2        310MB    120MB (38%)" }) } else { v };
            if query(path, "verbose").as_deref() == Some("true") {
                v["text"] = J::String(format!("{}\n\n(verbose: every image and container listed)", v["text"].as_str().unwrap_or("")));
            }
            Some((200, v))
        }
        ("POST", "/api/templates/save") => {
            let tag = body["tag"].as_str().unwrap_or("").to_string();
            if !valid_tag(&tag) {
                return Some((400, json!({ "ok": false, "error": format!("'{tag}' is not a template tag") })));
            }
            if st.data["templates"].is_null() {
                st.data["templates"] = json!({ "ok": true, "templates": [], "configured": null });
            }
            if let Some(l) = st.data["templates"]["templates"].as_array_mut() {
                l.retain(|t| t["tag"] != tag.as_str());
                l.push(json!({ "tag": tag, "size": "1.4GB", "created": "just now", "configured": false, "auto": null }));
            }
            Some((200, json!({ "ok": true, "warnings": [] })))
        }
        ("POST", "/api/templates/rm") => {
            let tag = body["tag"].as_str().unwrap_or("");
            let l = st.data["templates"]["templates"].as_array_mut()?;
            let n = l.len();
            l.retain(|t| t["tag"] != tag);
            Some(if l.len() < n { (200, json!({ "ok": true })) } else { (400, json!({ "ok": false, "error": format!("no template {tag}") })) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> J {
        crate::fixture::Fixture::load(&crate::fixture::dir().join("templates/basic.json")).unwrap().raw
    }

    fn ask(name: &'static str, args: J) -> Asked {
        Asked { name, args: args.as_object().unwrap().clone() }
    }

    #[test]
    fn removing_is_the_core_dialog_and_saving_is_direct() {
        let d = data();
        let cx = Ctx { data: &d };
        let a = act(&cx, &ask("template_rm", json!({ "tag": "web-base" }))).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
        assert!(c.danger && c.title == "Remove template web-base?");
        assert!(c.lines.iter().any(|l| l.contains("sbxw.toml names it")), "the configured one says so: {:?}", c.lines);
        assert_eq!(c.call.body, json!({ "tag": "web-base" }));
        assert!(act(&cx, &ask("template_rm", json!({ "tag": "forged" }))).is_err(), "only a listed tag");
        let a = act(&cx, &ask("template_save", json!({ "sandbox": "web-app", "tag": "web-v2" }))).unwrap();
        assert!(matches!(&a[1], Action::Call(c) if c.path == "/api/templates/save" && c.timeout == 1800));
        assert!(act(&cx, &ask("template_save", json!({ "sandbox": "web-app", "tag": "-rf" }))).is_err());
        assert!(act(&cx, &ask("template_save", json!({ "sandbox": "ghost", "tag": "x" }))).is_err());
        for s in INTENTS {
            assert_eq!(matches!(s.risk, Risk::Approval(_)), s.name == "template_rm", "{}", s.name);
        }
    }

    #[test]
    fn the_stand_in_saves_and_removes() {
        let mut d = data();
        let mut ev = |_: &str, _: J| {};
        let r = answer_from(&mut d, "POST", "/api/templates/save", &json!({ "sandbox": "web-app", "tag": "web-v2" }), &mut ev);
        assert_eq!(r.0, 200);
        assert!(d["templates"]["templates"].as_array().unwrap().iter().any(|t| t["tag"] == "web-v2"));
        assert_eq!(answer_from(&mut d, "POST", "/api/templates/rm", &json!({ "tag": "web-v2" }), &mut ev).0, 200);
        assert_eq!(answer_from(&mut d, "POST", "/api/templates/rm", &json!({ "tag": "web-v2" }), &mut ev).0, 400);
        assert!(answer_from(&mut d, "GET", "/api/disk-usage?verbose=true", &J::Null, &mut ev).1["text"].as_str().unwrap().contains("verbose"));
    }
}
