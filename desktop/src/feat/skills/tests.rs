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

/// What `GET /api/sandboxes/web-app/commands` answers in these tests: a kit
/// that names itself, two kit commands, a kept skill's command, sbx's shared
/// store, the project and a built-in (as sbxw-2 has them).
fn commands() -> J {
    json!({ "ok": true, "complete": true, "commands": [
        { "name": "archify", "description": "Draw a validated architecture diagram.", "kind": "command", "origin": "kit:agent-skills" },
        { "name": "clear", "description": "Clear the conversation history", "kind": "command", "origin": "built-in" },
        { "name": "deploy", "description": "Deploy the web app to staging, then smoke test it.", "kind": "skill", "origin": "workspace" },
        { "name": "frontend-design", "description": "Distinctive, intentional visual design.", "kind": "skill", "origin": "home" },
        { "name": "grill-me", "description": "Grill the user about a plan.", "kind": "command", "origin": "home" },
        { "name": "handoff", "description": "Hand the work to the next session.", "kind": "command", "origin": "home" },
        { "name": "release-notes", "description": "Write release notes.", "kind": "command", "origin": "home" },
    ] })
}

fn find_key<'a>(n: &'a crate::luau::Node, k: &str) -> Option<&'a crate::luau::Node> {
    if crate::screen::key_of(n).as_deref() == Some(k) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_key(c, k))
}

fn handlers(n: &crate::luau::Node, out: &mut Vec<String>) {
    if let Some(on) = &n.on {
        out.push(on.clone());
    }
    for c in &n.children {
        handlers(c, out);
    }
}

#[test]
fn look_also_reads_what_is_installed_and_never_starts_anything_more() {
    let data = fixture("full");
    let v = run(&data, "skills_look", json!({ "sandbox": "web-app" })).unwrap();
    assert!(v.contains(&Action::Set { key: "sandbox_commands".into(), value: J::Null }), "{v:?}");
    let paths: Vec<&str> = v.iter().filter_map(|a| if let Action::Fetch { path, .. } = a { Some(path.as_str()) } else { None }).collect();
    assert_eq!(paths, ["/api/sandboxes/web-app/skills", "/api/sandboxes/web-app/commands"]);
    // The stand-in (the monitor's) answers it from `commands.<name>`, and
    // without an entry is a daemon without the route.
    let mut d = data.clone();
    let (c, _) = crate::feat::answer_from(&mut d, "GET", "/api/sandboxes/web-app/commands", &J::Null, &mut |_, _| {});
    assert_eq!(c, 404);
    d["commands"] = json!({ "web-app": commands() });
    let (c, b) = crate::feat::answer_from(&mut d, "GET", "/api/sandboxes/web-app/commands", &J::Null, &mut |_, _| {});
    assert_eq!((c, b["commands"].as_array().unwrap().len()), (200, 7));
}

#[test]
fn installed_lists_every_entry_by_origin_read_only() {
    let (t, tree) = render_panel("skills", SRC, "looked", json!({ "sandbox_commands": commands() }));
    assert!(t.iter().any(|x| x == "Installed in web-app"), "{t:?}");
    assert!(t.iter().any(|x| x == "7 installed"), "{t:?}");
    let at = |s: &str| t.iter().position(|x| x == s).unwrap_or_else(|| panic!("no {s:?} in {t:?}"));
    // Grouped, in plain words: the kit that names itself, kits, your kept
    // skills, sbx's shared store, the project, Claude Code's own.
    let order = ["Kit agent-skills", "Kits", "Your kept skills", "sbx shared skills", "Project", "Built-in"];
    for w in order.windows(2) {
        assert!(at(w[0]) < at(w[1]), "{} before {}: {t:?}", w[0], w[1]);
    }
    for (name, desc) in [("/handoff", "Hand the work to the next session."), ("/frontend-design", "Distinctive, intentional visual design."), ("/clear", "Clear the conversation history")] {
        assert!(at(name) < at(desc));
    }
    assert!(at("/grill-me") > at("Kits") && at("/grill-me") < at("Your kept skills"));
    assert!(at("/frontend-design") > at("sbx shared skills") && at("/frontend-design") < at("Project"));
    // Read only: no Keep (nor anything else to press) on any of them, while
    // the keepable list keeps its Keep.
    let inst = find_key(&tree, "skills-installed").expect("the installed section");
    let mut on = Vec::new();
    handlers(inst, &mut on);
    assert!(on.is_empty(), "{on:?}");
    for n in ["archify", "clear", "handoff", "frontend-design", "grill-me"] {
        assert!(find_key(&tree, &format!("installed-{n}")).is_some(), "{n}");
        assert!(find_key(&tree, &format!("keep-{n}")).is_none(), "{n}");
    }
    assert!(find_key(&tree, "keep-deploy").is_some());
    assert_eq!(press("skills", SRC, "looked", json!({ "sandbox_commands": commands() }), "keep-deploy")[0], json!({ "name": "skill_keep", "args": { "sandbox": "web-app", "name": "deploy" } }));
}

#[test]
fn a_skill_the_agent_wrote_at_home_is_named_so_and_texts_are_cut() {
    let long = "x".repeat(400);
    let found = json!({ "ok": true, "skills": [{ "name": "notes", "origin": "home", "path": "~/.claude/skills/notes", "description": null, "files": 1, "bytes": 10, "kept": false, "keepable": true }] });
    let cmds = json!({ "ok": true, "complete": true, "commands": [
        { "name": "notes", "description": long, "kind": "skill", "origin": "home" },
    ] });
    let (t, _) = render_panel("skills", SRC, "looked", json!({ "sandbox_skills": found, "sandbox_commands": cmds }));
    assert!(t.iter().any(|x| x == "Written in this sandbox"), "{t:?}");
    assert!(!t.iter().any(|x| x == "sbx shared skills"), "{t:?}");
    let d = t.iter().find(|x| x.starts_with("xxx")).unwrap();
    assert_eq!(d.chars().count(), 160);
    assert!(d.ends_with('…'));
}

#[test]
fn nothing_to_keep_no_longer_says_no_skill() {
    let empty = json!({ "ok": true, "skills": [] });
    let (t, _) = render_panel("skills", SRC, "looked", json!({ "sandbox_skills": empty, "sandbox_commands": commands() }));
    assert!(t.iter().any(|x| x.starts_with("Nothing to keep: no skill the agent wrote under the project's .claude/skills/")), "{t:?}");
    assert!(!t.iter().any(|x| x.starts_with("No skill")), "{t:?}");
    assert!(t.iter().any(|x| x == "7 installed"), "{t:?}");
}

#[test]
fn a_stopped_sandbox_shows_what_the_route_read_and_its_reason() {
    let empty = json!({ "ok": true, "skills": [] });
    let cmds = json!({ "ok": true, "complete": false, "reason": "The sandbox is stopped: only the project's skills and commands and the built-in ones are listed.", "commands": [
        { "name": "deploy", "description": "Deploy it.", "kind": "skill", "origin": "workspace" },
        { "name": "compact", "description": "Free up context", "kind": "command", "origin": "built-in" },
    ] });
    let (t, tree) = render_panel("skills", SRC, "looked", json!({ "sandbox_skills": empty, "sandbox_commands": cmds }));
    assert!(t.iter().any(|x| x.starts_with("The sandbox is stopped: only the project's")), "{t:?}");
    assert!(find_key(&tree, "skills-installed-reason").is_some());
    assert!(t.iter().any(|x| x == "2 installed"), "{t:?}");
    assert!(!t.iter().any(|x| x == "Kits" || x == "sbx shared skills"), "{t:?}");
    // No start offered: the only press in the card is Look, as before.
    let mut on = Vec::new();
    handlers(find_key(&tree, "skills-found").unwrap(), &mut on);
    assert_eq!(on, ["look"]);
}

#[test]
fn a_daemon_without_the_route_shows_the_panel_as_before() {
    let empty = json!({ "ok": true, "skills": [] });
    // A 404 (or any failed read) leaves `sandbox_commands` empty: the
    // error goes to `app.errors`, never on the panel.
    let (t, tree) = render_panel("skills", SRC, "looked", json!({ "sandbox_skills": empty }));
    assert!(find_key(&tree, "skills-installed").is_none());
    assert!(!t.iter().any(|x| x.starts_with("Installed in")), "{t:?}");
    assert!(t.iter().any(|x| x == "No skill in web-app: none under the project's .claude/skills/ or the sandbox's own ~/.claude/skills/."), "{t:?}");
    let (t, _) = render_panel("skills", SRC, "looked", json!({}));
    assert!(t.iter().any(|x| x == "/deploy") && !t.iter().any(|x| x.starts_with("Installed in")), "{t:?}");
}
