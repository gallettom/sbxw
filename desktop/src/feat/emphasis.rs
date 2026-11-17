//! Keyword pills' switch ([`crate::emphasis`]): the daemon's `[emphasis]`
//! configuration read from `GET /api/emphasis` (again on its `emphasis`
//! stream event), and the app's own view preference per sandbox.
//!
//! * The daemon's switch is the source: `enabled`, and `sandboxes[ref]`, the
//!   effective switch per sandbox (emphasis-core combines the global and the
//!   per-sandbox settings). Changing it is Jarvis's (`emphasis_switch`), never
//!   the app's.
//! * The app's preference: "Keyword pills" in a pane bar's "⋯" menu turns
//!   them off (or back on) for that sandbox **in this app only**, kept in the
//!   app's state file (`data.app.kept.emphasis.off`, a list of sandboxes).
//! * The stricter wins: pills show only where both are on
//!   ([`crate::emphasis::enabled`]). The menu entry says when the daemon's
//!   switch is off, and offers nothing then.
//!
//! The core reads the result for each pane's terminal ([`for_pane`]) and the
//! screens read `data.emphasis` for their `pills` helper.

use super::*;
use crate::emphasis::{self, Vocab};
use crate::intent::{Arg, Risk, Spec};
use serde_json::json;
use std::rc::Rc;

pub const FEATURE: Feature = Feature {
    name: "emphasis",
    data: DATA,
    intents: INTENTS,
    act,
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

const DATA: &[Data] = &[Data { key: "emphasis", path: "/api/emphasis", on: &["emphasis"], every: 0, lazy: false }];

const INTENTS: &[Spec] = &[Spec {
    name: "pills_set",
    risk: Risk::Direct,
    args: &[("sandbox", Arg::Str, true), ("on", Arg::Bool, true)],
    doc: "Keyword pills in this sandbox's terminals, in this app only (a view preference kept in the app's state file). Off wins over the daemon's switch; on only takes effect where the daemon's switch is on.",
}];

/// The sandboxes whose pills the app's preference turns off.
pub fn app_off(kept: &J) -> Vec<String> {
    kept["off"].as_array().into_iter().flatten().filter_map(J::as_str).map(str::to_string).collect()
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let sb = a.req("sandbox");
    let on = a.b("on").unwrap_or(true);
    let mut off = app_off(&cx.get("app")["kept"]["emphasis"]);
    off.retain(|s| s != sb);
    if !on {
        off.push(sb.to_string());
        off.sort();
    }
    Ok(vec![Action::Remember { key: "off".into(), value: if off.is_empty() { J::Null } else { json!(off) } }])
}

/// The "⋯" menu entry of each running sandbox's panes (id `pills`, always in
/// the menu: `shell/bar.rs`, `MENU_ONLY`).
fn extras(cx: &Ctx) -> Vec<Extra> {
    let cfg = cx.get("emphasis");
    let cfg = (!cfg.is_null()).then_some(cfg);
    let off = app_off(&cx.get("app")["kept"]["emphasis"]);
    let mut v = vec![];
    for s in cx.get("sandboxes").as_array().into_iter().flatten() {
        let Some(name) = s["name"].as_str() else { continue };
        if s["status"].as_str() != Some("running") {
            continue;
        }
        let daemon_on = emphasis::enabled(cfg, Some(name), false);
        let mine_off = off.iter().any(|o| o == name);
        let (label, help, intent) = match (daemon_on, mine_off) {
            (false, _) => ("Keyword pills: off in sbxw.toml".to_string(), "The daemon's [emphasis] switch is off for this sandbox; Jarvis can turn it on.".to_string(), None),
            (true, false) => ("Keyword pills: on".to_string(), "Labels such as [[Done]] in this sandbox's terminals are drawn as coloured pills. Press to show them as plain text in this app.".to_string(), Some(("pills_set".to_string(), json!({ "sandbox": name, "on": false })))),
            (true, true) => ("Keyword pills: off in this app".to_string(), "Press to draw labels such as [[Done]] as coloured pills again in this sandbox's terminals.".to_string(), Some(("pills_set".to_string(), json!({ "sandbox": name, "on": true })))),
        };
        v.push(Extra { place: Place::Pane(name.to_string()), id: "pills".into(), label, icon: Some("tag".into()), tone: String::new(), help, count: None, intent });
    }
    v
}

thread_local! {
    /// The vocabulary built from the last `data.emphasis` seen, so every
    /// pane shares one `Rc` (and a terminal sees no change while it holds).
    static VOCAB: std::cell::RefCell<(J, Rc<Vocab>)> = std::cell::RefCell::new((J::Null, Rc::new(Vocab::default())));
}

/// The vocabulary of `cfg` (the daemon's payload; null: the built-in one).
pub fn vocab(cfg: &J) -> Rc<Vocab> {
    VOCAB.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != *cfg {
            c.1 = Rc::new(Vocab::from_json(cfg).unwrap_or_default());
            c.0 = cfg.clone();
        }
        c.1.clone()
    })
}

/// What a pane's terminal draws: the vocabulary when pills are on for its
/// sandbox (daemon's switch and the app's preference both on), else `None`.
/// No sandbox (a plain local shell): the daemon's global switch.
pub fn for_pane(cfg: Option<&J>, kept: &J, sandbox: Option<&str>) -> Option<Rc<Vocab>> {
    let cfg = cfg.filter(|c| !c.is_null());
    let mine_off = sandbox.is_some_and(|s| app_off(kept).iter().any(|o| o == s));
    emphasis::enabled(cfg, sandbox, mine_off).then(|| vocab(cfg.unwrap_or(&J::Null)))
}

/// The stand-in's default answer: the same shape as emphasis-core's
/// `GET /api/emphasis` with its default vocabulary.
pub fn default_payload() -> J {
    let fams = [
        ("done", "green", vec!["verified", "fixed", "passed"], "finished and checked"),
        ("failed", "red", vec!["error", "broken"], "something went wrong"),
        ("needs you", "amber", vec!["blocked", "decide", "question"], "the person must act or answer"),
        ("irreversible", "danger", vec!["destructive", "costly"], "cannot be undone, or costs money"),
        ("note", "blue", vec!["context", "fyi"], "context worth knowing"),
        ("working", "cyan", vec!["in progress", "next", "plan"], "the kind of work going on"),
        ("team", "purple", vec!["lead", "thread", "jarvis"], "Jarvis and the team"),
    ];
    let mut words = serde_json::Map::new();
    for (w, t, also, _) in &fams {
        words.insert(w.to_string(), json!(t));
        for a in also {
            words.insert(a.to_string(), json!(t));
        }
    }
    json!({
        "enabled": true,
        "guidance": true,
        "unknown_tone": "blue",
        "tones": ["green", "red", "amber", "blue", "purple", "cyan", "danger"],
        "max_word": emphasis::MAX_WORD,
        "max_detail": emphasis::MAX_DETAIL,
        "families": fams.iter().map(|(w, t, also, u)| json!({ "word": w, "tone": t, "also": also, "use": u })).collect::<Vec<_>>(),
        "words": words,
        "sandboxes": {},
    })
}

/// `GET /api/emphasis`: the fixture's `emphasis` (a fixture may set it, or
/// `{"variant": "custom"}` / `"quiet"`), else the default. `POST
/// /api/standin/emphasis` with `{"variant"}` or a whole payload replaces it
/// and publishes the `emphasis` event, as the daemon does on a change.
fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    let path = path.split('?').next().unwrap_or(path);
    match (method, path) {
        ("GET", "/api/emphasis") => Some((200, payload_of(&st.data["emphasis"]))),
        ("POST", "/api/standin/emphasis") => {
            st.data["emphasis"] = body.clone();
            let p = payload_of(body);
            (st.publish)("emphasis", p.clone());
            Some((200, p))
        }
        _ => None,
    }
}

/// A fixture's `emphasis`: a whole payload, or a named variant of the default.
/// `custom`: a word of the person's own (`shipped`, purple) and `quiet`: the
/// sandbox `api` with pills off.
pub fn payload_of(v: &J) -> J {
    let mut p = default_payload();
    match v.get("variant").and_then(J::as_str) {
        Some("custom") => {
            p["families"].as_array_mut().unwrap().push(json!({ "word": "shipped", "tone": "purple", "also": ["out the door"], "use": "a word of this team's own" }));
            p["words"]["shipped"] = json!("purple");
            p["words"]["out the door"] = json!("purple");
        }
        Some("quiet") => {
            p["sandboxes"] = json!({ "api": false, "web-app": true });
        }
        Some("off") => {
            p["enabled"] = json!(false);
        }
        _ if v.is_object() && v.get("words").is_some() => return v.clone(),
        _ => {}
    }
    if let Some(sb) = v.get("sandboxes").filter(|s| s.is_object()) {
        p["sandboxes"] = sb.clone();
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_data(kept_off: J) -> J {
        json!({
            "emphasis": payload_of(&json!({"variant": "quiet"})),
            "sandboxes": [{"name": "web-app", "status": "running"}, {"name": "api", "status": "running"}, {"name": "old", "status": "stopped"}],
            "app": {"kept": {"emphasis": {"off": kept_off}}},
        })
    }

    #[test]
    fn the_menu_entry_says_which_switch_is_off() {
        let data = ctx_data(json!([]));
        let v = extras(&Ctx { data: &data });
        let label = |sb: &str| v.iter().find(|e| e.place == Place::Pane(sb.into())).map(|e| (e.label.clone(), e.intent.is_some()));
        assert_eq!(label("web-app"), Some(("Keyword pills: on".into(), true)));
        assert_eq!(label("api"), Some(("Keyword pills: off in sbxw.toml".into(), false)), "the daemon's off: nothing to press");
        assert_eq!(label("old"), None);
        let data = ctx_data(json!(["web-app"]));
        let v = extras(&Ctx { data: &data });
        assert_eq!(v[0].label, "Keyword pills: off in this app");
    }

    #[test]
    fn the_preference_is_kept_and_off_wins() {
        let data = ctx_data(json!([]));
        let asked = Asked { name: "pills_set", args: json!({"sandbox": "web-app", "on": false}).as_object().unwrap().clone() };
        let acts = act(&Ctx { data: &data }, &asked).unwrap();
        assert_eq!(acts, vec![Action::Remember { key: "off".into(), value: json!(["web-app"]) }]);
        let cfg = &data["emphasis"];
        assert!(for_pane(Some(cfg), &json!({"off": []}), Some("web-app")).is_some());
        assert!(for_pane(Some(cfg), &json!({"off": ["web-app"]}), Some("web-app")).is_none(), "the app's off wins");
        assert!(for_pane(Some(cfg), &json!({"off": []}), Some("api")).is_none(), "the daemon's off wins");
        assert!(for_pane(Some(cfg), &json!({}), Some("unlisted")).is_some(), "not listed: the global switch");
        assert!(for_pane(None, &J::Null, Some("web-app")).is_some(), "no answer yet: the built-in default, on");
        // Back on: forgotten.
        let data = ctx_data(json!(["web-app"]));
        let asked = Asked { name: "pills_set", args: json!({"sandbox": "web-app", "on": true}).as_object().unwrap().clone() };
        assert_eq!(act(&Ctx { data: &data }, &asked).unwrap(), vec![Action::Remember { key: "off".into(), value: J::Null }]);
    }

    #[test]
    fn one_vocabulary_while_the_payload_holds() {
        let p = payload_of(&json!({"variant": "custom"}));
        let a = vocab(&p);
        let b = vocab(&p.clone());
        assert!(Rc::ptr_eq(&a, &b));
        assert_eq!(a.tone_of_str("shipped"), Some(emphasis::Tone::Purple));
        let c = vocab(&default_payload());
        assert!(!Rc::ptr_eq(&a, &c));
        assert_eq!(c.tone_of_str("shipped"), None);
    }
}
