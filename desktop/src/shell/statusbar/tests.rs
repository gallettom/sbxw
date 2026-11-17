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
fn a_failure_stays_until_looked_at() {
    let t = Instant::now();
    let all = vec![job("a", BarState::Failed("x".into()), None, 0.2, t), job("b", BarState::Running, None, 0.2, t)];
    assert_eq!(shown_jobs(all.clone(), &[]).len(), 2);
    let seen = vec![("a".to_string(), t)];
    assert_eq!(shown_jobs(all.clone(), &seen).iter().map(|j| j.sandbox.as_str()).collect::<Vec<_>>(), ["b"]);
    // Retried (a new start), it fails again: shown again.
    let again = vec![job("a", BarState::Failed("x".into()), None, 0.2, t + Duration::from_secs(1))];
    assert_eq!(shown_jobs(again, &seen).len(), 1);
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

#[cfg(unix)]
#[test]
fn this_apps_cpu_time_is_read() {
    let a = numbers::own_cpu_seconds().unwrap();
    let mut x = 0u64;
    for i in 0..30_000_000u64 {
        x = x.wrapping_add(i * i);
    }
    std::hint::black_box(x);
    let b = numbers::own_cpu_seconds().unwrap();
    assert!(b >= a && b - a < 60.0, "{a} {b}");
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
