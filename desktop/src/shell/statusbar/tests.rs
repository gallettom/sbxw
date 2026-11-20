use super::*;

fn job(name: &str, state: BarState, step: Option<&str>, frac: f32, since: Instant) -> BarJob {
    BarJob { sandbox: name.into(), step: step.map(str::to_string), frac, state, since }
}

#[test]
fn several_bring_ups_show_the_latest_and_a_count() {
    let t = Instant::now();
    let one = vec![job("better-api", BarState::Running, Some("Pulling image 63%"), 0.4, t)];
    assert_eq!(more(&one), None);
    let three = vec![
        job("web", BarState::Running, Some("Start it"), 0.5, t),
        job("better-api", BarState::Running, None, 0.1, t),
        job("old", BarState::Failed("boom".into()), Some("Install kits failed"), 0.75, t),
    ];
    assert_eq!(more(&three), Some(("+2".into(), "red-text")), "a failure behind the count turns it red");
    let asking = vec![three[0].clone(), job("x", BarState::Asking, None, 0.0, t)];
    assert_eq!(more(&asking), Some(("+1".into(), "amber")));
    assert_eq!(job_words(&job("w", BarState::Created, None, 1.0, t)), "created");
    assert_eq!(job_words(&three[1]), "Starting", "before the plan");
}

#[test]
fn the_bar_drops_its_parts_in_order_and_never_wraps() {
    let t = Instant::now();
    let jobs = vec![job("better-api", BarState::Running, Some("Pulling image 63%"), 0.4, t), job("web", BarState::Running, None, 0.0, t)];
    let cpu = numbers::cpu(Some(&serde_json::json!({ "daemon_cpu_pct": 0.8, "sandboxes": [{ "name": "a", "cpu_pct": 1141.5 }], "total_cpu_pct": 1142.3 })), None, Some(2.0));
    let cost = numbers::Cost { total: Some(1240.5), sandboxes: vec![], unknown: 0 };
    let all = Fit { step: true, cpu_word: true, help_word: true };
    assert_eq!(fit(1284.0, &jobs, &cpu, &cost), all);
    assert_eq!(fit(720.0, &jobs, &cpu, &cost), all);
    assert_eq!(fit(1284.0, &[], &cpu, &cost), all, "nothing in progress");
    // Shrink: the step goes first, then the CPU word, then the Help word.
    let mut last = all;
    let mut order = vec![];
    for w in (60..720).rev() {
        let f = fit(w as f32, &jobs, &cpu, &cost);
        if f != last {
            order.push(f);
            last = f;
        }
    }
    assert_eq!(
        order,
        [
            Fit { step: false, cpu_word: true, help_word: true },
            Fit { step: false, cpu_word: false, help_word: true },
            Fit { step: false, cpu_word: false, help_word: false },
        ]
    );
}

fn sb(name: &str, cost: Option<f64>) -> crate::client::Sandbox {
    let mut s = crate::client::Sandbox::new(name, "claude", "running");
    if let Some(c) = cost {
        s.extra.insert("cost_usd".into(), serde_json::json!(c));
    }
    s
}

#[test]
fn cpu_sums_what_reports_and_says_when_partial_or_unknown() {
    use numbers::*;
    let full = serde_json::json!({ "daemon_cpu_pct": 0.8, "sandboxes": [{ "name": "myapp", "cpu_pct": 143.5, "mem_bytes": 2147483648u64, "mem_total_bytes": 8589934592u64 }], "total_cpu_pct": 144.3, "sandboxes_counted": 1, "sandboxes_unknown": 0, "ts": 1 });
    let c = cpu(Some(&full), None, Some(3.0));
    assert_eq!((c.total, c.partial), (Some(147.3), false), "the app's share on top");
    assert_eq!(cpu_text(&c, true), "CPU 147%");
    assert_eq!(cpu_text(&c, false), "147%");
    assert_eq!(c.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["myapp", "this app", "sbxw daemon"], "sorted by CPU");
    assert_eq!(mem_text(&c.rows[0]), "2.0 GB / 8.0 GB");

    let partial = serde_json::json!({ "daemon_cpu_pct": 0.8, "sandboxes": [{ "name": "myapp", "cpu_pct": 143.5 }, { "name": "cx", "cpu_pct": null, "mem_bytes": null }], "total_cpu_pct": 144.3, "sandboxes_counted": 1, "sandboxes_unknown": 1 });
    let c = cpu(Some(&partial), None, None);
    assert_eq!((c.total, c.partial, c.counted, c.sandboxes), (Some(144.3), true, 1, 2));
    assert_eq!(cpu_text(&c, true), "CPU 144%+");
    assert!(cpu_help(&c).contains("1 of 2 sandboxes report; the others have no recent report"), "{}", cpu_help(&c));
    assert_eq!(c.rows.last().unwrap().cpu, None, "unknown last, never 0");

    let none = serde_json::json!({ "daemon_cpu_pct": null, "sandboxes": [{ "name": "cx", "cpu_pct": null }], "total_cpu_pct": null, "sandboxes_counted": 0, "sandboxes_unknown": 1 });
    let c = cpu(Some(&none), None, Some(2.0));
    assert_eq!(cpu_text(&c, true), "CPU –", "the app alone is not a total");
    let c = cpu(None, Some("the daemon answered 404"), Some(2.0));
    assert_eq!(cpu_text(&c, true), "CPU –");
    assert!(cpu_help(&c).contains("404"));
    assert_eq!(pct(0.8), "0.8%");
    assert!(hot(Some(720.0), 8) && !hot(Some(700.0), 8));
}

#[test]
fn cost_sums_the_sandboxes_and_lists_sessions_and_what_ended() {
    use numbers::*;
    let sessions = serde_json::json!([
        { "sandbox": "web", "state": "idle", "cost_usd": 1.2, "model": "claude-opus-5-5" },
        { "sandbox": "web", "state": "working", "thread": "payments", "cost_usd": 0.4, "model_name": "Sonnet 5.5" },
        { "sandbox": "web", "state": "working", "thread": "docs" },
        { "sandbox": "web", "state": "exited", "thread": "old", "cost_usd": 9.0 },
        { "sandbox": "api", "state": "idle" },
    ]);
    let c = cost(&[sb("web", Some(2.0)), sb("api", None), sb("quiet", None), sb("chat", Some(10.4))], Some(&sessions));
    assert_eq!(c.total.map(|t| (t * 100.0).round()), Some(1240.0), "the sandboxes' figures, sessions not added again");
    assert_eq!(cost_text(&c), "$12.40");
    assert_eq!(c.sandboxes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["chat", "web", "api"], "quiet has neither cost nor session");
    let web = &c.sandboxes[1];
    assert_eq!(web.sessions.iter().map(|s| (s.name.as_str(), s.model.as_deref(), s.cost)).collect::<Vec<_>>(), [("main", Some("opus 5.5"), Some(1.2)), ("docs", None, None), ("payments", Some("sonnet 5.5"), Some(0.4))]);
    assert_eq!(web.ended.map(|e| (e * 100.0).round()), Some(40.0), "2.00 - 1.60 listed: ended sessions");
    assert_eq!(c.unknown, 2, "docs and api's main have none");
    assert!(cost_help(&c).contains("2 sessions have not reported"));
    let none = cost(&[sb("a", None)], None);
    assert_eq!((cost_text(&none), none.sandboxes.len()), ("$–".to_string(), 0));
    assert_eq!(usd(1240.5), "$1240");
    assert_eq!(short_model(None, Some("claude-sonnet-5-5-20261001")).as_deref(), Some("sonnet 5.5"));
}

/// The fit is the one laid out: at every width the window allows, the
/// right entries are as wide as measured, side by side inside the bar, and
/// the bring-up's entry ends before them.
#[test]
fn the_status_bar_fit_measures_what_is_drawn() {
    use crate::shell::{view, Args};
    // Under 720 (the window's least width): what the app's zoom gives (720 at 200% is 360).
    for w in [360.0, 420.0, 480.0, 560.0, 720.0, 1284.0, 1800.0] {
        let a = App::new(Args { fixture: Some(format!("{}/fixtures/shell/statusbar-many.json", env!("CARGO_MANIFEST_DIR")).into()), ..Args::default() });
        let mut rt = rune::Runtime::new(a, view, (w, 480.0), rune::Env::new(true, 1.0));
        for _ in 0..60 {
            rt.update(|s, _| {
                let now = Instant::now();
                s.statusbar_poll(now);
            });
            rt.frame(0.016);
        }
        let (bar_r, _) = rt.presented("status-bar").unwrap();

        assert!((bar_r[3] - H).abs() < 0.5 && (bar_r[0] + bar_r[2] - w).abs() < 0.5, "{w}: {bar_r:?}");
        let (cpu, cost) = (rt.state.status.cpu.clone().unwrap(), rt.state.status.cost.clone().unwrap());
        let f = fit(w, &rt.state.status.jobs, &cpu, &cost);
        let mut x = None;
        for (key, t) in [("status-cpu", numbers::cpu_text(&cpu, f.cpu_word)), ("status-cost", numbers::cost_text(&cost)), ("status-help", if f.help_word { "Help".into() } else { String::new() })] {
            let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("{w}: {key}"));
            assert!((r[2] - item_w(&t)).abs() < 1.0, "{w}: {key} drawn {} wide, measured {}", r[2], item_w(&t));
            assert!(r[0] + r[2] <= bar_r[0] + bar_r[2] + 0.5, "{w}: {key} past the bar");
            if let Some(px) = x {
                assert!(r[0] >= px - 0.5, "{w}: {key} overlaps the entry before it");
            }
            x = Some(r[0] + r[2]);
        }
        let (right, _) = rt.presented("status-right").unwrap();
        let (jobs, _) = rt.presented("status-jobs").expect("a bring-up in the fixture");
        assert!(jobs[0] + jobs[2] <= right[0] + 0.5, "{w}: the bring-up runs under the numbers");
        assert_eq!(rt.presented("status-jobs-step").is_some(), f.step, "{w}");
        eprintln!("status bar at {w}: {f:?}");
    }
}

/// The theme's switch: a press goes to the next theme at once and is
/// kept; its menu lists every theme by name; the screens get their data
/// again; an older state file's "light" falls back to Dark.
#[test]
fn the_theme_switch_switches_at_once_and_is_kept() {
    use crate::shell::{view, Args};
    use crate::theme::{self, Mode};
    let app = || App::new(Args { fixture: Some(format!("{}/fixtures/shell/design-2.json", env!("CARGO_MANIFEST_DIR")).into()), ..Args::default() });
    let mut rt = rune::Runtime::new(app(), view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    for _ in 0..6 {
        rt.frame(0.016);
    }
    assert_eq!(rt.state.theme_choice, Mode::Dark, "a new install: Dark");
    assert_eq!(theme::mode(), Mode::Dark);
    let tap = |rt: &mut rune::Runtime<App>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.016);
        rt.frame(0.016);
    };
    tap(&mut rt, "status-theme");
    assert_eq!(theme::mode(), Mode::Graphite, "one press");
    assert_eq!(rt.state.theme_choice, Mode::Graphite);
    assert_eq!(rt.state.store.get("theme"), "graphite", "kept in the state file");
    assert_eq!(rt.state.screen_data()["app"]["theme"], "graphite", "the screens' data says so");
    // The menu: every theme by name.
    tap(&mut rt, "status-theme-more");
    for m in Mode::ALL {
        assert!(rt.find(&format!("status-theme-{}", m.name())).is_some(), "{m:?} listed");
    }
    tap(&mut rt, "status-theme-dark");
    assert_eq!(theme::mode(), Mode::Dark);
    assert!(rt.find("status-theme-pop").is_none(), "the menu closed");
    // A state file from the light-theme days: Dark.
    let mut a = app();
    a.store.set("theme", serde_json::json!("light"));
    assert_eq!(a.store.get("theme").as_str().and_then(Mode::parse), None);
}

/// After a theme switch, a screen that read a theme colour shows the new
/// one though no data it read changed: its colour props (`color = "text"`)
/// and `theme.color` reads are resolved when it renders, so a switch must
/// render every screen again. Fails if renders are skipped on a switch.
#[test]
fn a_theme_switch_renders_the_screens_again() {
    use crate::shell::{view, Args};
    use crate::theme::{self, Mode};
    let dir = std::env::temp_dir().join(format!("sbxw-theme-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let probe = dir.join("probe.luau");
    // Reads no data at all: only the theme.
    std::fs::write(&probe, r#"return { render = function() return column{ key = "probe", fill = "surface", text{ theme.color.bg, key = "probe-bg", color = "text" } } end }"#).unwrap();
    let a = App::new(Args { fixture: Some(format!("{}/fixtures/shell/design-2.json", env!("CARGO_MANIFEST_DIR")).into()), ..Args::default() });
    let mut rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    rt.update(|s, _| s.screens.iter_mut().find(|x| x.name == "sandboxes").unwrap().load_file(probe.clone()));
    for _ in 0..4 {
        rt.frame(0.016);
    }
    let read = |rt: &rune::Runtime<App>| {
        let sc = rt.state.screens.iter().find(|x| x.name == "sandboxes").unwrap();
        let n = sc.last_good.as_ref().expect("the probe rendered");
        let t = &n.children[0];
        (t.text.clone(), t.style.color, n.style.fill)
    };
    let dark = read(&rt);
    assert_eq!(dark.0, format!("#{:06x}", theme::color_in(Mode::Dark, "bg").unwrap()));
    rt.update(|s, _| s.set_theme(Mode::Graphite));
    for _ in 0..4 {
        rt.frame(0.016);
    }
    let graphite = read(&rt);
    assert_eq!(graphite.0, format!("#{:06x}", theme::color_in(Mode::Graphite, "bg").unwrap()), "theme.color read again");
    assert_eq!(graphite.1, theme::color_in(Mode::Graphite, "text"), "a colour prop resolved again");
    assert_eq!(graphite.2, theme::color_in(Mode::Graphite, "surface"));
    std::fs::remove_dir_all(&dir).ok();
}

/// The "New sandboxes" panel is gone: a press on the bar's bring-up lists
/// them in a popover above it, inside the window at 1284 x 816 and 720 x
/// 480, bounded, with the failure's reason and its Dismiss; Escape and a
/// press outside close it; the old `open_panel create-jobs` opens the same
/// popover and docks nothing.
#[test]
fn the_bring_ups_popover_replaces_the_panel() {
    use crate::shell::{view, Args};
    assert!(crate::feat::screen("create-jobs").is_none(), "no jobs panel screen");
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let a = App::new(Args { fixture: Some(format!("{}/fixtures/shell/statusbar-many.json", env!("CARGO_MANIFEST_DIR")).into()), ..Args::default() });
        let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        let settle = |rt: &mut rune::Runtime<App>| {
            for _ in 0..8 {
                rt.update(|s, _| {
                    s.statusbar_poll(Instant::now());
                });
                rt.frame(0.016);
            }
        };
        settle(&mut rt);
        assert!(rt.state.feats.panel.is_none() && rt.state.feats.panels.is_empty(), "{w}: a bring-up opens no panel");
        assert!(rt.find("status-jobs-pop").is_none(), "{w}: closed until pressed");
        let tap = |rt: &mut rune::Runtime<App>, key: &str| {
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        };
        tap(&mut rt, "status-jobs");
        settle(&mut rt);
        let (p, _) = rt.presented("status-jobs-pop").unwrap_or_else(|| panic!("{w}: the popover opens"));
        assert!(p[0] >= -0.5 && p[1] >= -0.5 && p[0] + p[2] <= w + 0.5 && p[1] + p[3] <= h + 0.5, "{w}x{h}: the popover {p:?} inside the window");
        let (bar, _) = rt.presented("status-bar").unwrap();
        assert!(p[1] + p[3] <= bar[1] + 0.5, "{w}: above the bar");
        for k in ["job-payments-v2", "job-payments-v2-error", "job-dismiss-payments-v2", "job-better-api"] {
            assert!(rt.find(k).is_some(), "{w}: {k} listed");
        }
        assert!(rt.find("job-retry-payments-v2").is_none(), "{w}: started elsewhere, no Retry (it was never offered)");
        assert!(rt.find("job-dismiss-better-api").is_none(), "{w}: a running one cannot be dismissed");
        // Escape closes it; a second Escape is not taken (it goes to the terminal).
        assert!(rt.state.statusbar_escape());
        assert!(!rt.state.statusbar_escape());
        settle(&mut rt);
        assert!(rt.find("status-jobs-pop").is_none());
        // A press outside closes it and does nothing else.
        tap(&mut rt, "status-jobs");
        settle(&mut rt);
        assert!(rt.find("status-jobs-pop").is_some());
        rt.pointer_down(w / 2.0, 60.0);
        rt.pointer_up(w / 2.0, 60.0);
        settle(&mut rt);
        assert!(rt.find("status-jobs-pop").is_none(), "{w}: light dismiss");
        // The old panel's intent: the same popover, nothing docked.
        for i in crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": "open_panel", "args": { "screen": "create-jobs" } })).unwrap() {
            rt.update(|s, _| s.run_intent("test", i));
        }
        settle(&mut rt);
        assert_eq!(rt.state.status.pop, Some(Pop::Jobs));
        assert!(rt.state.feats.panel.is_none(), "{w}: nothing docked");
    }
}
