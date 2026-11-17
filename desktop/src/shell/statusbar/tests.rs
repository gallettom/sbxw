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
    let all = Fit { step: true, cpu_word: true, help_word: true };
    assert_eq!(fit(1284.0, &jobs), all);
    assert_eq!(fit(720.0, &jobs), all);
    assert_eq!(fit(1284.0, &[]), all, "nothing in progress");
    // Shrink: the step goes first, then the CPU word, then the Help word.
    let mut last = all;
    let mut order = vec![];
    for w in (60..720).rev() {
        let f = fit(w as f32, &jobs);
        if f != last {
            order.push(f);
            last = f;
        }
    }
    assert_eq!(
        order,
        [
            Fit { step: false, cpu_word: true, help_word: true },
            Fit { step: false, cpu_word: false, help_word: false },
        ]
    );
}
