use super::*;
use crate::shell::{view, Args};
use serde_json::json;

fn answer(base: Option<&str>, head: &str, threads: J) -> J {
    json!({ "ok": true, "workspace": "/Users/me/dev/shop", "repos": [{ "path": "", "base": base, "head": head, "threads": threads }] })
}

fn threads() -> J {
    json!([{ "name": "t1", "branch": "sbxw/t1", "tip": "f00ba12345" }, { "name": "loose", "branch": "", "tip": "9f8e7d6c5b4a" }, { "name": "bare", "branch": "" }])
}

#[test]
fn the_item_follows_the_data_and_is_absent_rather_than_wrong() {
    let ws = Some("/Users/me/dev/shop/");
    let a = answer(Some("main"), "a1b2c3d4e5", threads());
    let i = |w: Option<&str>, a: Option<&J>, t: Option<&str>| item(w, a, t).map(|i| full(&i));
    // The main checkout: the base branch; a thread: its own branch.
    assert_eq!(i(ws, Some(&a), None).as_deref(), Some("shop/main"));
    assert_eq!(i(ws, Some(&a), Some("t1")).as_deref(), Some("shop/sbxw/t1"));
    // Nothing read yet: the repository alone, no slash.
    assert_eq!(i(ws, None, None).as_deref(), Some("shop"));
    assert_eq!(i(ws, None, Some("t1")).as_deref(), Some("shop"));
    // A thread the list does not have: the repository alone, never a guess from its name.
    assert_eq!(i(ws, Some(&a), Some("ghost")).as_deref(), Some("shop"));
    // A detached head: the short commit id, else "detached"; never an empty slash.
    let d = answer(None, "deadbeef0123456", threads());
    assert_eq!(i(ws, Some(&d), None).as_deref(), Some("shop/deadbee"));
    assert!(item(ws, Some(&d), None).unwrap().detached);
    assert_eq!(i(ws, Some(&d), Some("loose")).as_deref(), Some("shop/9f8e7d6"));
    assert_eq!(i(ws, Some(&d), Some("bare")).as_deref(), Some("shop/detached"));
    // A repository with no commit yet says no branch: the repository alone.
    let none = json!({ "workspace": "/w/shop", "repos": [{ "path": "", "base": null, "head": null, "threads": [] }] });
    assert_eq!(i(ws, Some(&none), None).as_deref(), Some("shop"));
    // Several repositories, or none: which one is on screen is not in the data.
    let two = json!({ "workspace": "/w", "repos": [{ "path": "a", "base": "x" }, { "path": "b", "base": "y" }] });
    assert_eq!(i(ws, Some(&two), None), None);
    assert_eq!(i(ws, Some(&json!({ "repos": [] })), None), None);
    // A repository in a folder of the workspace is named by that folder.
    let sub = json!({ "workspace": "/w", "repos": [{ "path": "apps/web", "base": "main", "head": "1" }] });
    assert_eq!(i(ws, Some(&sub), None).as_deref(), Some("web/main"));
    // No workspace known and nothing read: absent.
    assert_eq!(i(None, None, None), None);
    // Control characters never reach the bar.
    let odd = answer(Some("fe\nat\u{7}ure\t/x"), "1", json!([]));
    assert_eq!(i(ws, Some(&odd), None).as_deref(), Some("shop/feature/x"));
}

#[test]
fn a_long_branch_shortens_in_its_middle_then_hides() {
    let long: String = format!("feature/{}end", "long-".repeat(40));
    assert!(long.chars().count() > 200);
    let it = Item { repo: "sbxw".into(), branch: Some(long.clone()), detached: false };
    assert_eq!(fit_text(&it, 10_000.0).unwrap(), full(&it));
    let mut last = 0;
    for w in [900.0, 600.0, 300.0, 200.0, 120.0] {
        let t = fit_text(&it, w).unwrap_or_else(|| panic!("{w}"));
        assert!(item_w(&t) <= w, "{w}: {t} measures {}", item_w(&t));
        assert!(t.starts_with("sbxw/") && t.contains('…') && t.ends_with("end"), "{t}");
        assert!(t.chars().count() >= last || last == 0 || w < 900.0);
        last = t.chars().count();
    }
    // A short branch is never cut by an ellipsis that saves nothing.
    let short = Item { repo: "sbxw".into(), branch: Some("main".into()), detached: false };
    assert_eq!(fit_text(&short, 400.0).as_deref(), Some("sbxw/main"));
    // Too narrow for even a stub of the branch: hidden.
    assert_eq!(fit_text(&it, 60.0), None);
    assert_eq!(fit_text(&short, 60.0), None);
    // The repository alone has nothing to shorten.
    assert_eq!(fit_text(&Item { repo: "sbxw".into(), branch: None, detached: false }, 20.0), None);
}

const T: &str = "fixtures/shell/statusbar-git.json";

fn rig(w: f32) -> rune::Runtime<App> {
    let a = App::new(Args { fixture: Some(format!("{}/{T}", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
    rune::Runtime::new(a, view, (w, 480.0), rune::Env::new(true, 1.0))
}

/// Polls as the app would: over `secs` seconds from `t0`, a frame each.
fn run(rt: &mut rune::Runtime<App>, t0: Instant, from: f32, secs: f32) {
    let mut t = from;
    while t <= secs {
        rt.update(|s, _| {
            s.statusbar_poll(t0 + Duration::from_secs_f32(t));
        });
        rt.frame(0.016);
        t += 0.1;
    }
    // Let a leaving element finish going.
    for _ in 0..40 {
        rt.frame(0.016);
    }
}

/// The app's own poll on the real clock (screens' answers come through it).
fn run_real(rt: &mut rune::Runtime<App>, secs: f32) {
    let start = Instant::now();
    let mut clock = 0.0;
    while start.elapsed().as_secs_f32() < secs {
        clock += 0.03;
        rt.update(|s, tx| {
            s.poll(clock, tx);
        });
        rt.frame(0.03);
        std::thread::sleep(Duration::from_millis(30));
    }
    for _ in 0..40 {
        rt.frame(0.016);
    }
}

fn shown(rt: &rune::Runtime<App>) -> Option<String> {
    rt.presented("status-git")?;
    rt.elements().iter().find(|e| e.key.as_deref() == Some("status-git")).and_then(|e| e.label.clone())
}

fn point(rt: &mut rune::Runtime<App>, sandbox: &str, thread: Option<&str>) {
    let n = rt.state.sandboxes.iter().position(|s| s.name == sandbox).expect("sandbox");
    let _ = n;
    rt.update(|s, _| {
        s.feats.base = None;
        s.feats.overlay = None;
        s.panes[0].sandbox = Some(sandbox.into());
        s.panes[0].thread = thread.map(str::to_string);
        s.focused = 0;
    });
}

#[test]
fn each_view_has_its_rule_and_the_focus_moves_it() {
    let t0 = Instant::now();
    let mut rt = rig(1800.0);
    // The first frames: the sandbox just came on screen, nothing read yet (debounce).
    run(&mut rt, t0, 0.0, 0.2);
    assert_eq!(shown(&rt).as_deref(), Some("shop"), "repository alone while the first read is pending");
    run(&mut rt, t0, 0.2, 1.0);
    assert_eq!(shown(&rt).as_deref(), Some("shop/main"), "TUI: the focused pane's main checkout");
    // The focused pane is a thread's: its own branch (from the data).
    point(&mut rt, "better-checkout", Some("checkout-v2"));
    run(&mut rt, t0, 1.0, 1.3);
    assert_eq!(shown(&rt).as_deref(), Some("shop/sbxw/checkout-v2"));
    point(&mut rt, "better-checkout", Some("loose"));
    run(&mut rt, t0, 1.3, 1.6);
    assert_eq!(shown(&rt).as_deref(), Some("shop/9f8e7d6"), "a detached thread worktree: the short commit id");
    // Another sandbox: its repository, read after the debounce.
    point(&mut rt, "sbxw-jarvis", None);
    run(&mut rt, t0, 1.6, 1.8);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw"));
    run(&mut rt, t0, 1.8, 2.5);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw/main"));
    point(&mut rt, "sbxw-jarvis", Some("leader-mark"));
    run(&mut rt, t0, 2.5, 2.7);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw/sbxw/leader-mark"));
    // A detached main checkout.
    point(&mut rt, "sbxw-desktop-design", None);
    run(&mut rt, t0, 2.7, 4.0);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw/deadbee"));
    // Several repositories: absent.
    point(&mut rt, "docs", None);
    run(&mut rt, t0, 4.0, 5.5);
    assert_eq!(shown(&rt), None);
    // A sandbox with no pane at all, and an empty pane: absent.
    rt.update(|s, _| s.panes[0].sandbox = None);
    run(&mut rt, t0, 5.5, 5.8);
    assert_eq!(shown(&rt), None);

    // The Sessions view: the selected session, same rule.
    point(&mut rt, "better-checkout", None);
    rt.update(|s, _| {
        s.feats.base = Some("monitor".into());
        s.feats.data.insert("monitor".into(), json!({ "detail": { "sandbox": "sbxw-jarvis", "thread": "leader-mark" } }));
    });
    run(&mut rt, t0, 5.8, 6.0);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw/sbxw/leader-mark"));
    rt.update(|s, _| {
        s.feats.data.insert("monitor".into(), json!({ "detail": { "sandbox": "sbxw-jarvis", "thread": null } }));
    });
    run(&mut rt, t0, 6.0, 6.2);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw/main"));
    rt.update(|s, _| {
        s.feats.data.insert("monitor".into(), json!({ "detail": null }));
    });
    run(&mut rt, t0, 6.2, 6.4);
    assert_eq!(shown(&rt), None, "no session selected");
    // A view about no sandbox (an overlay with no scope): absent, not the pane under it.
    rt.update(|s, _| {
        s.feats.base = None;
        s.feats.overlay = Some(("kits".into(), J::Null));
    });
    run(&mut rt, t0, 6.4, 6.6);
    assert_eq!(shown(&rt), None);
}

#[test]
fn a_scoped_screen_gives_its_sandbox_and_all_gives_nothing() {
    use crate::intent::Intent;
        let mut rt = rig(1800.0);
    run_real(&mut rt, 1.0);
    let ask = |sb: Option<&str>| {
        let args = match sb {
            Some(s) => json!({ "sandbox": s }),
            None => json!({}),
        };
        Intent::Feature(crate::feat::Asked { name: "worktrees_show", args: args.as_object().cloned().unwrap() })
    };
    rt.update(|s, _| s.run_intent("statusbar", ask(Some("sbxw-jarvis"))));
    run_real(&mut rt, 1.5);
    assert_eq!(rt.state.feats.overlay.as_ref().map(|o| o.0.as_str()), Some("worktrees"));
    assert_eq!(shown(&rt).as_deref(), Some("sbxw/main"), "scope: that sandbox's main checkout");
    rt.update(|s, _| s.run_intent("statusbar", ask(None)));
    run_real(&mut rt, 1.5);
    assert_eq!(shown(&rt), None, "scope All");
}

#[test]
fn reads_are_rare_and_never_for_a_hidden_window_or_a_stopped_sandbox() {
    let t0 = Instant::now();
    let mut rt = rig(1800.0);
    let at = |rt: &rune::Runtime<App>, sb: &str| rt.state.status.git.cache.get(sb).map(|e| e.1);
    run(&mut rt, t0, 0.0, 0.2);
    assert_eq!(at(&rt, "better-checkout"), None, "debounced: nothing in the first 300 ms");
    run(&mut rt, t0, 0.2, 1.0);
    let first = at(&rt, "better-checkout").expect("read");
    run(&mut rt, t0, 1.0, 29.0);
    assert_eq!(at(&rt, "better-checkout"), Some(first), "no second read inside 30 s");
    run(&mut rt, t0, 29.0, 32.0);
    assert!(at(&rt, "better-checkout") > Some(first), "again after 30 s");
    // Moving away and back inside 30 s reuses the cache.
    let again = at(&rt, "better-checkout").unwrap();
    point(&mut rt, "sbxw-jarvis", None);
    run(&mut rt, t0, 32.0, 33.0);
    point(&mut rt, "better-checkout", None);
    run(&mut rt, t0, 33.0, 35.0);
    assert_eq!(at(&rt, "better-checkout"), Some(again));
    // A hidden window reads nothing.
    point(&mut rt, "docs", None);
    rt.update(|s, _| s.statusbar_visible(false));
    run(&mut rt, t0, 35.0, 37.0);
    assert_eq!(at(&rt, "docs"), None);
    rt.update(|s, _| s.statusbar_visible(true));
    run(&mut rt, t0, 37.0, 38.0);
    assert!(at(&rt, "docs").is_some());
    // A stopped sandbox: the repository, no read.
    point(&mut rt, "sbxw-voice", None);
    run(&mut rt, t0, 38.0, 40.0);
    assert_eq!(at(&rt, "sbxw-voice"), None);
    assert_eq!(shown(&rt).as_deref(), Some("sbxw"));
}

#[test]
fn a_failed_read_keeps_the_last_value_and_says_nothing() {
    let t0 = Instant::now();
    let mut rt = rig(1800.0);
    run_real(&mut rt, 1.0);
    assert_eq!(shown(&rt).as_deref(), Some("shop/main"));
    rt.update(|s, _| s.git_data(&format!("{KEY_PREFIX}better-checkout"), Err("the daemon answered 502".into()), t0 + Duration::from_secs(2)));
    run(&mut rt, t0, 2.0, 3.0);
    assert_eq!(shown(&rt).as_deref(), Some("shop/main"));
    assert!(rt.state.notice.as_ref().is_none_or(|n| !n.0.contains("502")));
}

/// A 200-character branch at the two widths: the item shortens (the end of
/// the branch stays), the entries on the right stay whole, the full name is in its tooltip.
#[test]
fn a_200_character_branch_never_pushes_the_fixed_items_out() {
    for w in [1284.0_f32, 720.0, 420.0] {
        let t0 = Instant::now();
        let mut rt = rig(w);
        point(&mut rt, "data-pipeline-with-a-long-name", None);
        run(&mut rt, t0, 0.0, 1.5);
        let (cpu, cost) = (rt.state.status.cpu.clone().unwrap(), rt.state.status.cost.clone().unwrap());
        let f = super::super::fit(w, &rt.state.status.jobs, &cpu, &cost);
        let (bar, _) = rt.presented("status-bar").unwrap();
        let (right, _) = rt.presented("status-right").unwrap();
        for (key, t) in [("status-cpu", super::super::numbers::cpu_text(&cpu, f.cpu_word)), ("status-cost", super::super::numbers::cost_text(&cost)), ("status-help", if f.help_word { "Help".into() } else { String::new() })] {
            let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("{w}: {key}"));
            assert!((r[2] - item_w(&t)).abs() < 1.0, "{w}: {key} whole");
            assert!(r[0] + r[2] <= bar[0] + bar[2] + 0.5);
        }
        let label = shown(&rt);
        // The bring-ups' list is one for the whole process: another test's may show.
        let Some((g, _)) = rt.presented("status-git") else {
            assert!(!rt.state.status.jobs.is_empty(), "{w}: the item hides with nothing else in the bar: {label:?}");
            continue;
        };
        assert!(g[0] + g[2] <= right[0] + 0.5, "{w}: the item {g:?} runs under the numbers {right:?}");
        let label = label.unwrap();
        assert!(label.starts_with("data/feature/") || label.starts_with("data/fe"), "{w}: {label}");
        assert!(label.ends_with("-end") || label.ends_with("nd"), "{w}: {label}");
        if w < 1000.0 {
            assert!(label.contains('…'), "{w}: {label}");
        }
        let el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("status-git")).unwrap();
        assert!(el.help.as_deref().is_some_and(|h| h.contains(&"x".repeat(190))), "{w}: the tooltip has the branch whole");
    }
}
