//! The scope picker (scope.rs): its model, its colours against the
//! sidebar's and the pane header's, contrast on every group hue in both
//! themes, and in the whole window: Files and Commits draw it in their
//! title, its list opens, moves, filters, picks and closes (Escape, a press
//! outside), pre-scoped opens show their scope, names are text only, and
//! nothing leaves the overlay or overlaps in its title row at any width.

use super::*;
use crate::luau::Item;
use crate::shell::{view, App, Args};
use crate::theme::{Mode, Need};
use rune::{Key, KeyEvent, Runtime};
use serde_json::json;

fn items(ids: &[&str]) -> Vec<Item> {
    ids.iter().map(|i| Item::plain(i, &format!("label of {i}"), None)).collect()
}

fn data() -> J {
    json!({
        "sandboxes": [
            { "name": "web-app", "status": "running", "agent": "claude", "workspace": "/w/web" },
            { "name": "docs", "status": "stopped", "agent": "codex", "workspace": "/w/docs/" },
            { "name": "web-api", "status": "running", "agent": "claude", "workspace": "/w/web" },
            { "name": "chat-1", "status": "running", "agent": "claude", "chat": true, "workspace": "/c/1" },
            { "name": "loose", "status": "running", "agent": "claude" },
        ],
        "sessions": [ { "sandbox": "web-app" }, { "sandbox": "web-app" }, { "sandbox": "docs", "ended": true } ],
        "app": { "groups": { "web-app": "#d95c7a", "web-api": "#d95c7a", "docs": "#5cd98a", "chat-1": "#9d7ad9" } },
    })
}

#[test]
fn the_scope_prop_is_read_and_an_unknown_kind_is_a_plain_select() {
    assert_eq!(spec(&json!("sandbox")).map(|s| s.kind), Some("sandbox".into()));
    let s = spec(&json!({ "kind": "sandbox", "narrow": "thread-a", "narrow_icon": "branch", "clear": "branch_clear" })).unwrap();
    assert_eq!((s.narrow.as_deref(), s.narrow_icon.as_str(), s.clear.as_deref()), (Some("thread-a"), "branch", Some("branch_clear")));
    // An icon the app does not have falls back; an empty narrow is none.
    let s = spec(&json!({ "kind": "sandbox", "narrow": "", "narrow_icon": "no-such-icon" })).unwrap();
    assert_eq!((s.narrow, s.narrow_icon.as_str()), (None, "branch"));
    for v in [json!("team"), json!({ "kind": "canvas" }), json!(true), J::Null, json!(3)] {
        assert!(spec(&v).is_none(), "{v}");
    }
}

#[test]
fn choices_come_from_the_core_s_data_grouped_with_all_first() {
    let d = data();
    let c = choices(&d, &items(&["docs", "web-app", "", "nobody", "chat-1", "web-api", "web-app", "loose"]), &Spec::default());
    let ids: Vec<&str> = c.iter().map(|c| c.id.as_str()).collect();
    // All first; a sandbox the core does not know is left out; a repeat once;
    // web-api joins web-app's group (the group's place is its first sandbox's).
    assert_eq!(ids, ["", "docs", "web-app", "web-api", "chat-1", "loose"]);
    let web = c.iter().find(|c| c.id == "web-app").unwrap();
    assert_eq!((web.status.as_str(), web.group.as_deref(), web.color, web.agent.as_str(), web.sessions), ("running", Some("web"), Some(0xd95c7a), "claude", 2));
    // An ended session is not counted; the trailing slash does not hide the folder.
    let docs = c.iter().find(|c| c.id == "docs").unwrap();
    assert_eq!((docs.sessions, docs.group.as_deref()), (0, Some("docs")));
    assert_eq!(c.iter().find(|c| c.id == "chat-1").unwrap().group.as_deref(), Some("Chats"));
    assert_eq!(c.iter().find(|c| c.id == "loose").unwrap().group, None);
    // No "" item: no All.
    assert!(choices(&d, &items(&["docs"]), &Spec::default()).iter().all(|c| !c.all()));
    // No items: every sandbox the core knows, All with `all = true`, only the running ones with `running = true`.
    let every = |sp: Spec| choices(&d, &[], &sp).iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    assert_eq!(every(spec(&json!("sandbox")).unwrap()), ["web-app", "web-api", "docs", "chat-1", "loose"]);
    assert_eq!(every(spec(&json!({ "kind": "sandbox", "all": true })).unwrap())[0], "");
    assert!(!every(spec(&json!({ "kind": "sandbox", "running": true })).unwrap()).contains(&"docs".to_string()));
}

#[test]
fn the_filter_matches_names_and_groups_and_keeps_all() {
    let d = data();
    let c = choices(&d, &items(&["", "web-app", "docs", "web-api"]), &Spec::default());
    let ids = |q: &str| filter(&c, q).iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids("API"), ["", "web-api"]);
    assert_eq!(ids("web"), ["", "web-app", "web-api"]);
    assert_eq!(ids("  "), ["", "web-app", "web-api", "docs"]);
    assert_eq!(ids("zzz"), [""]);
}

/// Every group colour a theme can give: each hue, at each lightness step, and the chats'.
fn every_group_colour() -> Vec<u32> {
    let g = theme::group_look();
    let mut out: Vec<u32> = (0..360).step_by(5).flat_map(|h| g.lightness.iter().map(move |l| theme::hsl(h as f32, g.saturation, *l))).collect();
    out.push(theme::hsl(265.0, g.chat.0, g.chat.1));
    out
}

#[test]
fn the_capsule_reads_at_4_5_on_every_group_hue_in_both_themes() {
    let mut bad = vec![];
    for m in Mode::ALL {
        theme::set_mode(m);
        let mut grounds: Vec<(Option<u32>, Look)> = every_group_colour().into_iter().map(|g| (Some(g), look(Some(g)))).collect();
        grounds.push((None, look(None)));
        for (g, lk) in grounds {
            for (what, fg, need) in [("name", lk.text, Need::Text), ("chevron and secondary", lk.muted, Need::Text)] {
                let r = theme::contrast(fg, lk.fill);
                if r < need.min() {
                    bad.push(format!("{m:?} {g:06x?}: {what} {r:.2}"));
                }
            }
            // The status marks on the capsule: 3:1.
            for tone in ["green", "blue", "red"] {
                let mk = theme::mark_on(crate::screen::tone(tone).0, lk.fill);
                if theme::contrast(mk, lk.fill) < Need::Mark.min() {
                    bad.push(format!("{m:?} {g:06x?}: {tone} dot"));
                }
            }
            // The frame's title band (the pane header's tint): the title and "in" read there.
            if let Some(w) = frame(g).wash {
                for (what, fg) in [("title", theme::color_value("text-bright").unwrap()), ("in", theme::muted_on(w))] {
                    let r = theme::contrast(fg, w);
                    if r < Need::Text.min() {
                        bad.push(format!("{m:?} {g:06x?}: {what} on the band {r:.2}"));
                    }
                }
            }
        }
        // The list's group headings: the sidebar's own tint and label (3:1 at least, as a label at 12 pt semibold there).
        for g in every_group_colour() {
            let (tint, _, label) = theme::group_tints(g);
            let r = theme::contrast(label, tint);
            if r < Need::Text.min() {
                bad.push(format!("{m:?} {g:06x}: heading {r:.2}"));
            }
        }
    }
    theme::set_mode(Mode::Dark);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn the_scope_s_colours_are_the_sidebar_s_and_the_pane_header_s() {
    for m in Mode::ALL {
        theme::set_mode(m);
        for g in every_group_colour() {
            // The capsule's edge and the overlay's strip: the group's own colour (the sidebar's square).
            assert_eq!(look(Some(g)).border, g);
            let f = frame(Some(g));
            assert_eq!(f.strip, Some(g));
            // The title band: exactly a focused pane header of that group.
            assert_eq!(f.wash, Some(theme::bar_ground(Some(g), true)));
        }
        // Not scoped, or All: today's frame.
        assert_eq!(frame(None), Frame { strip: None, wash: None, edge: theme::color_value("elevated").unwrap() });
    }
    theme::set_mode(Mode::Dark);
}

// ── In the whole window ───────────────────────────────────────────────────

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
    }
}

fn window(fixture: &str, w: f32, h: f32) -> Runtime<App> {
    let path = format!("{}/fixtures/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
    // Never the person's state file.
    a.store = crate::shell::store::Store::memory();
    let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn run(rt: &mut Runtime<App>, name: &str, args: J) {
    let v = json!({ "kind": "intent", "name": name, "args": args });
    for i in crate::intent::parse(&v).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(rt);
}

fn press(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is not drawn"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn key(rt: &mut Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: rune::Mods::default() });
    settle(rt);
}

fn label_of(rt: &Runtime<App>, key: &str) -> Option<String> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).and_then(|e| e.label)
}

fn focused(rt: &Runtime<App>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn files_shows_its_sandbox_in_its_title_and_the_list_picks_another() {
    let mut rt = window("scope/many", 1284.0, 816.0);
    run(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    // Pre-scoped: the capsule says web-app, in the overlay's header, after "Files in".
    assert_eq!(label_of(&rt, "files-sandbox").as_deref(), Some("Scope: web-app (running)"));
    let (cap, _) = rt.presented("files-sandbox").unwrap();
    let (head, _) = rt.presented("overlay-head-files").unwrap();
    assert!(cap[1] >= head[1] && cap[1] + cap[3] <= head[1] + head[3] + 0.5, "the capsule is in the title row: {cap:?} {head:?}");
    assert!(rt.find("overlay-strip-files").is_some() && rt.find("overlay-wash-files").is_some(), "the frame has web-app's colour");
    // Opens: the list, its filter (12 sandboxes), focused.
    press(&mut rt, "files-sandbox");
    assert!(rt.find("files-sandbox-list").is_some());
    assert_eq!(focused(&rt).as_deref(), Some("files-sandbox-filter"));
    // Files has no All.
    assert!(rt.find("files-sandbox-").is_none());
    // Typing narrows; Enter picks the first match.
    for ch in "doc".chars() {
        rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: rune::Mods::default() });
    }
    settle(&mut rt);
    assert!(rt.find("files-sandbox-docs").is_some() && rt.find("files-sandbox-web-app").is_none());
    key(&mut rt, Key::Enter);
    assert!(rt.find("files-sandbox-list").is_none(), "a pick closes the list");
    assert_eq!(label_of(&rt, "files-sandbox").as_deref(), Some("Scope: docs (running)"));
    assert!(rt.state.feats.overlay.as_ref().is_some_and(|o| o.0 == "files"));
    // Reopened: the filter is empty again.
    press(&mut rt, "files-sandbox");
    assert!(rt.find("files-sandbox-web-app").is_some(), "the filter starts empty");
}

#[test]
fn escape_closes_the_list_and_then_the_overlay() {
    let mut rt = window("scope/many", 1284.0, 816.0);
    run(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    press(&mut rt, "files-sandbox");
    assert!(rt.find("files-sandbox-list").is_some());
    key(&mut rt, Key::Escape);
    assert!(rt.find("files-sandbox-list").is_none(), "Escape closes the list");
    assert!(rt.state.feats.overlay.is_some(), "and only the list");
    key(&mut rt, Key::Escape);
    assert!(rt.state.feats.overlay.is_none(), "a second Escape closes Files");
}

#[test]
fn a_press_outside_closes_the_list_and_does_nothing_else() {
    let mut rt = window("scope/many", 1284.0, 816.0);
    run(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    press(&mut rt, "files-sandbox");
    // Over the sidebar's row of docs: the list closes, nothing is selected.
    let before = rt.state.selected.clone();
    let (x, y) = rt.find("row-docs").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(rt.find("files-sandbox-list").is_none());
    assert_eq!(rt.state.selected, before, "the press went nowhere else");
    assert_eq!(label_of(&rt, "files-sandbox").as_deref(), Some("Scope: web-app (running)"));
}

#[test]
fn commits_offers_all_and_the_keyboard_moves_and_picks() {
    let mut rt = window("shell/two-panes", 1284.0, 816.0);
    run(&mut rt, "commits_show", json!({}));
    assert_eq!(label_of(&rt, "commits-sandbox").as_deref(), Some("Scope: All sandboxes"));
    // All: no colour on the frame.
    assert!(rt.find("overlay-strip-commits").is_none());
    press(&mut rt, "commits-sandbox");
    // Few sandboxes: no filter; the current row (All) has the keyboard.
    assert!(rt.find("commits-sandbox-filter").is_none());
    assert_eq!(focused(&rt).as_deref(), Some("commits-sandbox-"));
    key(&mut rt, Key::Down);
    let to = focused(&rt).expect("a row has the keyboard");
    assert!(to.starts_with("commits-sandbox-") && to != "commits-sandbox-", "{to}");
    key(&mut rt, Key::Enter);
    let sb = to.trim_start_matches("commits-sandbox-").to_string();
    assert!(label_of(&rt, "commits-sandbox").is_some_and(|l| l.starts_with(&format!("Scope: {sb} ("))), "{sb}");
    assert!(rt.find("overlay-strip-commits").is_some(), "scoped: the frame takes its colour at once");
    // Back to All with the pointer.
    press(&mut rt, "commits-sandbox");
    press(&mut rt, "commits-sandbox-");
    assert_eq!(label_of(&rt, "commits-sandbox").as_deref(), Some("Scope: All sandboxes"));
}

#[test]
fn a_branch_shows_as_the_second_level_and_its_cross_clears_it() {
    let mut rt = window("shell/two-panes", 1284.0, 816.0);
    run(&mut rt, "commits_show", json!({ "sandbox": "web-app", "branch": "checkout-copy" }));
    assert_eq!(label_of(&rt, "commits-sandbox").as_deref(), Some("Scope: web-app (running)"));
    assert!(rt.find("commits-sandbox-narrow").is_some(), "the branch after the capsule");
    press(&mut rt, "commits-sandbox-narrow-clear");
    assert!(rt.find("commits-sandbox-narrow").is_none());
    assert_eq!(label_of(&rt, "commits-sandbox").as_deref(), Some("Scope: web-app (running)"), "the sandbox stays");
}

#[test]
fn the_scope_is_remembered_per_screen() {
    let mut rt = window("scope/many", 1284.0, 816.0);
    run(&mut rt, "files_open", json!({ "sandbox": "docs" }));
    key(&mut rt, Key::Escape);
    // Opened again from the rail (no sandbox): the last one.
    run(&mut rt, "files_show", json!({}));
    assert_eq!(label_of(&rt, "files-sandbox").as_deref(), Some("Scope: docs (running)"));
}

#[test]
fn hostile_and_long_names_are_text_only_and_clipped() {
    let hostile = "<b>hostile</b> [[Done]] {on=\"stop\"}";
    let long = "a-very-long-sandbox-name-for-the-quarterly-reporting-pipeline-2026";
    for name in [hostile, long] {
        let mut rt = window("scope/many", 1284.0, 816.0);
        run(&mut rt, "files_open", json!({ "sandbox": name }));
        assert_eq!(label_of(&rt, "files-sandbox"), Some(format!("Scope: {name} (running)")));
        let (cap, _) = rt.presented("files-sandbox").unwrap();
        assert!(cap[2] <= NAME_MAX_W + 80.0, "{name}: the capsule is clipped: {cap:?}");
        // Its whole name in a tooltip-bearing element; no pill, no control made from it.
        press(&mut rt, "files-sandbox");
        let els = rt.elements();
        let inside: Vec<_> = els.iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("files-sandbox-") && k.contains("hostile") && k != format!("files-sandbox-{hostile}") && !k.starts_with("files-sandbox-face-"))).collect();
        assert!(inside.is_empty(), "{name}: {:?}", inside.iter().map(|e| &e.key).collect::<Vec<_>>());
        assert!(!els.iter().any(|e| e.text.as_deref() == Some("Done")), "{name}: no keyword pill from a name");
    }
}

/// Nothing of the title row overlaps, nothing leaves the overlay: Files and
/// Commits at every width, scoped and All, the list open.
#[test]
fn the_title_row_fits_and_the_list_stays_in_the_window_at_every_width() {
    let mut bad = vec![];
    for (w, h) in [(1284.0, 816.0), (760.0, 600.0), (500.0, 600.0), (420.0, 700.0)] {
        for (fixture, intent, args, screen, sel) in [
            ("scope/many", "files_open", json!({ "sandbox": "a-very-long-sandbox-name-for-the-quarterly-reporting-pipeline-2026" }), "files", "files-sandbox"),
            ("shell/two-panes", "commits_show", json!({ "sandbox": "web-app", "branch": "checkout-copy-with-a-long-branch-name" }), "commits", "commits-sandbox"),
            ("shell/two-panes", "commits_show", json!({}), "commits", "commits-sandbox"),
        ] {
            let mut rt = window(fixture, w, h);
            if w < 700.0 {
                rt.update(|s, _| s.toggle_sidebar());
                settle(&mut rt);
            }
            run(&mut rt, intent, args.clone());
            let Some((p, _)) = rt.presented(&format!("overlay-{screen}")) else { panic!("{screen} not drawn at {w}") };
            let (head, _) = rt.presented(&format!("overlay-head-{screen}")).unwrap();
            // The title row's parts, left to right, none over the next, all inside.
            let row: Vec<(String, [f32; 4])> = rt.elements().into_iter().filter(|e| !e.exiting && e.parent.is_some()).filter_map(|e| {
                let k = e.key.clone()?;
                let r = e.rect;
                let in_head = r[1] >= head[1] - 0.5 && r[1] + r[3] <= head[1] + head[3] + 0.5;
                (in_head && (k == sel || k == format!("{sel}-narrow") || k == "overlay-close")).then_some((k, r))
            }).collect();
            for (k, r) in &row {
                if r[0] < p[0] - 0.5 || r[0] + r[2] > p[0] + p[2] + 0.5 {
                    bad.push(format!("{screen} {args} at {w}: {k} {r:?} outside {p:?}"));
                }
            }
            let mut sorted = row.clone();
            sorted.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
            for pair in sorted.windows(2) {
                if pair[0].1[0] + pair[0].1[2] > pair[1].1[0] + 0.5 {
                    bad.push(format!("{screen} {args} at {w}: {} over {}", pair[0].0, pair[1].0));
                }
            }
            // The list open: inside the window.
            press(&mut rt, sel);
            let (l, _) = rt.presented(&format!("{sel}-list")).unwrap_or_else(|| panic!("{screen} list at {w}"));
            if l[0] < -0.5 || l[0] + l[2] > w + 0.5 || l[1] + l[3] > h + 0.5 {
                bad.push(format!("{screen} {args} at {w}: the list {l:?} leaves the window"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn renders(rt: &Runtime<App>, screen: &str) -> u64 {
    rt.state.screens.iter().find(|s| s.name == screen).map_or(0, |s| s.renders)
}

fn fill_of(rt: &Runtime<App>, key: &str) -> Option<rune::Color> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.fill)
}

/// The capsule and the list read the core's data when the view is built
/// (screen.rs `Build::data`, the data every push hands the screen), not
/// through the screen's Luau reads: a change of what they show is drawn
/// at once without the screen's script running again, and a push that
/// changes nothing they show runs nothing.
#[test]
fn the_scope_follows_the_core_s_data_without_running_the_screen_again() {
    let mut rt = window("scope/many", 1284.0, 816.0);
    run(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    press(&mut rt, "files-sandbox");
    key(&mut rt, Key::Escape);
    // (A push: the screens' data again, as any intent a screen returns. Each
    // below selects another sandbox in the sidebar, which the scope does not show.)
    rt.update(|s, _| crate::screen::Host::intents(s, "test", vec![crate::intent::Intent::Select { sandbox: Some("docs".into()) }]));
    settle(&mut rt);
    let r0 = renders(&rt, "files");
    let strip0 = fill_of(&rt, "overlay-strip-files");
    // A push that changes nothing the scope shows: nothing renders, nothing changes.
    rt.update(|s, _| crate::screen::Host::intents(s, "test", vec![crate::intent::Intent::Select { sandbox: Some("infra".into()) }]));
    settle(&mut rt);
    assert_eq!(renders(&rt, "files"), r0, "an unchanged push renders nothing");
    // Its state: the label (and the dot) follow at once; the script did not run.
    rt.update(|s, _| {
        s.sandboxes.iter_mut().find(|x| x.name == "web-app").unwrap().status = "stopped".into();
        crate::screen::Host::intents(s, "test", vec![crate::intent::Intent::Select { sandbox: Some("billing".into()) }]);
    });
    settle(&mut rt);
    assert_eq!(label_of(&rt, "files-sandbox").as_deref(), Some("Scope: web-app (stopped)"));
    // Its group: another workspace, another colour on the frame.
    rt.update(|s, _| {
        s.sandboxes.iter_mut().find(|x| x.name == "web-app").unwrap().workspace = Some("/Users/ana/dev/elsewhere".into());
        crate::screen::Host::intents(s, "test", vec![crate::intent::Intent::Select { sandbox: Some("search".into()) }]);
    });
    settle(&mut rt);
    assert_ne!(fill_of(&rt, "overlay-strip-files"), strip0, "the frame takes the new group's colour");
    // A sandbox added: in the open list.
    rt.update(|s, _| {
        let mut x = s.sandboxes[0].clone();
        x.name = "fresh-one".into();
        s.sandboxes.push(x);
        crate::screen::Host::intents(s, "test", vec![crate::intent::Intent::Select { sandbox: Some("mobile".into()) }]);
    });
    settle(&mut rt);
    press(&mut rt, "files-sandbox");
    // (At the end of the list: in it, maybe scrolled out of view. A loaded
    // machine may take a few more frames.)
    for _ in 0..5 {
        if rt.elements().iter().any(|e| !e.exiting && e.key.as_deref() == Some("files-sandbox-fresh-one")) {
            break;
        }
        settle(&mut rt);
    }
    assert!(rt.elements().iter().any(|e| !e.exiting && e.key.as_deref() == Some("files-sandbox-fresh-one")), "a new sandbox is offered");
    key(&mut rt, Key::Escape);
    // The theme: the capsule's colours switch with it.
    let cap0 = fill_of(&rt, "files-sandbox");
    rt.update(|s, _| s.set_theme(Mode::Graphite));
    settle(&mut rt);
    assert_ne!(fill_of(&rt, "files-sandbox"), cap0, "the capsule follows the theme");
    rt.update(|s, _| s.set_theme(Mode::Dark));
}

fn in_view(rt: &Runtime<App>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.key).filter_map(|k| k.strip_prefix("in-view-").map(str::to_string)).collect()
}

/// The sidebar's "in view" eye: on the row of the sandbox the overlay is
/// scoped to, at once on a pick, none for All, gone when the overlay closes.
#[test]
fn the_sidebar_marks_the_sandbox_in_view() {
    let mut rt = window("scope/many", 1284.0, 816.0);
    assert!(in_view(&rt).is_empty());
    run(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    assert_eq!(in_view(&rt), ["web-app"]);
    press(&mut rt, "files-sandbox");
    press(&mut rt, "files-sandbox-docs");
    assert_eq!(in_view(&rt), ["docs"], "it follows the scope at once");
    key(&mut rt, Key::Escape);
    assert!(in_view(&rt).is_empty(), "gone with the overlay");
    run(&mut rt, "commits_show", json!({}));
    assert!(in_view(&rt).is_empty(), "nothing for All sandboxes");
}

/// Worktrees: All by default, one sandbox from a sandbox's link.
#[test]
fn worktrees_has_the_scope_all_by_default() {
    let mut rt = window("worktrees/standin", 1284.0, 816.0);
    run(&mut rt, "worktrees_show", json!({}));
    assert_eq!(label_of(&rt, "worktrees-sandbox").as_deref(), Some("Scope: All sandboxes"));
    run(&mut rt, "worktrees_show", json!({ "sandbox": "web-app" }));
    assert!(label_of(&rt, "worktrees-sandbox").is_some_and(|l| l.starts_with("Scope: web-app")));
    assert!(rt.find("overlay-strip-worktrees").is_some());
}

/// The extra push that carries a moved scope to the sidebar is one-shot by
/// construction: a screen whose drawn scope changes on every render (and
/// reads it back) costs exactly one more render per push, never a loop.
#[test]
fn a_scope_that_moves_on_every_render_pushes_once_more_and_no_more() {
    let dir = std::env::temp_dir().join(format!("sbxw-scope-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("commits.luau");
    std::fs::write(&path, r#"
local n = 0
return {
  render = function(_, data)
    n = n + 1
    local _ = ((data.app or {}).overlay or {}).scope
    return column{ row{ key = "title-slot",
      select{ id = "commits-sandbox", value = (n % 2 == 0) and "web-app" or "docs", on = "x", scope = "sandbox" } } }
  end,
  actions = { x = function() end },
}
"#).unwrap();
    let mut rt = window("scope/many", 1284.0, 816.0);
    run(&mut rt, "commits_show", json!({}));
    rt.update(|s, _| s.screens.iter_mut().find(|x| x.name == "commits").unwrap().load_file(path.clone()));
    settle(&mut rt);
    let mut extra = vec![];
    for sb in ["billing", "search", "mobile"] {
        rt.update(|s, _| {
            let r0 = s.screens.iter().find(|x| x.name == "commits").unwrap().renders;
            crate::screen::Host::intents(s, "test", vec![crate::intent::Intent::Select { sandbox: Some(sb.into()) }]);
            extra.push(s.screens.iter().find(|x| x.name == "commits").unwrap().renders - r0);
        });
    }
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(extra, [2, 2, 2], "one render for the push, one for the single push after it");
}
