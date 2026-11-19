//! Its intents, compared with Setup's former tab tests (moved here as they
//! were: same intents, same arguments, same dialogs), and its panel.

use super::*;
use crate::feat::setup::tests::{asked, fixture, one_confirm, press, render_panel};

fn run(data: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    act(&Ctx { data }, &asked(name, args))
}

const SRC: &str = include_str!("../../../screens/skills.luau");

#[test]
fn every_approval_intent_answers_one_confirm() {
    let data = fixture("full");
    let c = one_confirm(run(&data, "skill_forget", json!({ "name": "release-notes" })));
    assert_eq!(c.call.path, "/api/my-skills/rm");
    for (n, risk) in [("skill_keep", true), ("skill_forget", true), ("skills_look", false), ("skills_show", false), ("skills_pick", false)] {
        assert_eq!(matches!(INTENTS.iter().find(|s| s.name == n).unwrap().risk, Risk::Approval(_)), risk, "{n}");
    }
}

#[test]
fn opened_for_a_sandbox_its_picker_is_on_it() {
    let data = fixture("full");
    let v = run(&data, "skills_show", json!({ "sandbox": "docs" })).unwrap();
    assert!(v.contains(&Action::Set { key: "skills_view".into(), value: json!({ "sandbox": "docs" }) }));
    assert!(v.contains(&Action::Remember { key: "sandbox".into(), value: json!("docs") }));
    assert!(!v.iter().any(|a| matches!(a, Action::Fetch { .. })), "looking stays a press: it starts a stopped sandbox");
    let look = |extra: J| press("skills", SRC, "full", extra, "skills-look");
    assert_eq!(look(json!({ "skills_view": { "sandbox": "docs" } }))[0], json!({ "name": "skills_look", "args": { "sandbox": "docs" } }));
    assert_eq!(look(json!({ "kept": { "skills": { "sandbox": "web-api" } } }))[0], json!({ "name": "skills_look", "args": { "sandbox": "web-api" } }));
    assert_eq!(look(json!({}))[0], json!({ "name": "skills_look", "args": { "sandbox": "web-app" } }));
}

#[test]
fn every_former_action_is_on_the_panel_with_the_same_intent() {
    assert_eq!(press("skills", SRC, "full", json!({}), "forget-release-notes")[0], json!({ "name": "skill_forget", "args": { "name": "release-notes" } }));
    assert_eq!(press("skills", SRC, "looked", json!({}), "keep-deploy")[0], json!({ "name": "skill_keep", "args": { "sandbox": "web-app", "name": "deploy" } }));
    assert_eq!(press("skills", SRC, "full", json!({}), "skills-sandbox")[0]["name"], "skills_pick");
    let (t, _) = render_panel("skills", SRC, "looking", json!({}));
    assert!(t.iter().any(|x| x.starts_with("Reading web-app")), "{t:?}");
}

#[test]
fn empty_and_failed_say_what_to_do() {
    let (t, _) = render_panel("skills", SRC, "empty", json!({}));
    assert!(t.iter().any(|x| x.starts_with("None kept yet")), "{t:?}");
}

#[test]
fn its_rail_entry() {
    let data = fixture("full");
    let r = rail_entry(&Ctx { data: &data });
    assert_eq!((r.id.as_str(), r.icon.as_deref(), r.count), ("skills", Some("book"), None));
    assert_eq!(r.intent.as_ref().unwrap(), &("open_panel".to_string(), json!({ "screen": "skills" })));
}

#[test]
fn keeping_a_skill_uses_the_cores_listing_not_the_screens_words() {
    let mut data = fixture("full");
    assert!(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "deploy" })).unwrap_err().contains("look at"));
    let v = run(&data, "skills_look", json!({ "sandbox": "web-app" })).unwrap();
    assert!(matches!(&v[2], Action::Fetch { key, path } if key == "sandbox_skills" && path == "/api/sandboxes/web-app/skills"));
    data["skills_for"] = json!("web-app");
    data["sandbox_skills"] = data["sandbox_skills_of"]["web-app"].clone();
    let c = one_confirm(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "deploy" })));
    assert_eq!(c.call.body, json!({ "origin": "project", "name": "deploy", "replace": false }));
    let c = one_confirm(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "release-notes" })));
    assert_eq!(c.call.body["replace"], true);
    assert!(run(&data, "skill_keep", json!({ "sandbox": "web-app", "name": "Bad Name" })).unwrap_err().contains("cannot be kept"));
}

/// One column: the sandbox's skills (Look, Keep) on top, your kept skills
/// under it, the whole width, at every width.
#[test]
fn the_sandbox_s_skills_are_above_yours_at_every_width() {
    use crate::shell::{view, App, Args};
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/skills/waiting.json");
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0), (420.0, 700.0)] {
        let a = App::new(Args { fixture: Some(path.into()), ..Args::default() });
        let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        for _ in 0..30 {
            rt.frame(0.016);
        }
        if w < 700.0 {
            rt.update(|s, _| s.toggle_sidebar());
            for _ in 0..30 {
                rt.frame(0.016);
            }
        }
        let (o, _) = rt.presented("overlay-skills").unwrap_or_else(|| panic!("no overlay at {w}"));
        let (f, _) = rt.presented("skills-found").unwrap_or_else(|| panic!("no Look card at {w}"));
        let (k, _) = rt.presented("skills-kept").unwrap_or_else(|| panic!("no kept list at {w}"));
        assert!(f[1] + f[3] <= k[1] + 0.5, "at {w}: {f:?} above {k:?}");
        assert!(f[2] >= o[2] - 64.0 && k[2] >= o[2] - 64.0, "at {w}: full width in {o:?}");
        assert!(rt.presented("skills-split").is_none());
    }
}
