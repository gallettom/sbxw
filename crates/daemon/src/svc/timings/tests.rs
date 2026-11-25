use super::*;
use crate::provision::{BringUp, Consent};
use crate::testing::{self, app_in_with, sbx_answer, sbx_answer_booting};
use sbxw_core::config::Config;
use sbxw_core::progress::Progress;
use sbxw_core::templates;
use sbxw_sbx::runner::Output;

fn request(dir: &Path, cfg: Config) -> BringUp {
    let ws = dir.join("myapp");
    std::fs::create_dir_all(&ws).unwrap();
    BringUp {
        name: SandboxRef::parse("myapp").unwrap(),
        workspace: ws,
        ro: vec![],
        rw: vec![],
        carve: vec![],
        config: cfg,
        extra_ports: vec![],
        chat: false,
        api_key: None,
        template: templates::Choice::Default,
        remote_control: false,
        chrome: false,
        git: None,
        consent: Consent::Checked(vec![]),
    }
}

/// A local kit whose startup command the watch looks for.
fn kit_with_startup(dir: &Path) -> String {
    let kit = dir.join("kit");
    std::fs::create_dir_all(&kit).unwrap();
    std::fs::write(
        kit.join("spec.yaml"),
        "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\ncapabilities:\n  - type: com.docker.sandbox/lifecycle@1\n    config:\n      startup:\n        - command: [\"bash\", \"/k/install.sh\"]\n",
    )
    .unwrap();
    kit.to_string_lossy().into_owned()
}

fn bare() -> Config {
    Config { kits: vec![], network_allow: vec![], network_deny: vec![], ports: vec![], ..Config::default() }
}

/// The steps up to `done`, by name.
fn names_to_done(t: &Timeline) -> Vec<String> {
    let mut out = vec![];
    for s in &t.steps {
        out.push(s.name.clone());
        if s.name == step::DONE {
            break;
        }
    }
    out
}

fn once_each(t: &Timeline) {
    let mut seen = std::collections::HashSet::new();
    for s in &t.steps {
        assert!(seen.insert(s.name.clone()), "{} named twice in {:?}", s.name, t.steps);
    }
}

async fn eventually(done: impl Fn() -> bool) -> bool {
    for _ in 0..300 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    done()
}

/// Runs a bring-up, returns what its progress stream ended with.
async fn bring_up(app: &Arc<App>, req: BringUp) -> (anyhow::Result<Vec<String>>, Option<Progress>) {
    let last = Arc::new(Mutex::new(None));
    let l = last.clone();
    let sink: sbxw_core::progress::Sink = Arc::new(move |p| *l.lock().unwrap() = Some(p));
    let out = app.provision(req, Some(sink)).await;
    let end = last.lock().unwrap().clone();
    (out, end)
}

#[tokio::test]
async fn a_new_sandbox_without_a_template_times_every_step_once_in_order() {
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), sbx_answer_booting());
    let (out, end) = bring_up(&app, request(d.path(), Config { kits: vec![kit_with_startup(d.path())], ..bare() })).await;
    out.unwrap();
    let t = app.timelines().remove(0);
    assert_eq!(
        names_to_done(&t),
        ["prepare", "sbx_ls", "template_check", "record", "create_prepare", "sbx_create", "boot", "tooling", "policy_allow", "aliases", "done"]
    );
    once_each(&t);
    assert!(t.steps.iter().take_while(|s| s.name != step::DONE).all(|s| s.ms.is_some() && !s.unfinished), "{:?}", t.steps);
    assert_eq!((t.kind.as_str(), t.template.as_deref(), t.kits, t.host.as_str(), t.sandbox.as_str()), ("new", None, 2, "local", "myapp"));
    assert_eq!(t.sbx_version.as_deref(), Some("0.51.0"));
    assert!(t.error.is_none() && t.ready_ms.is_some());
    let create = t.span(step::SBX_CREATE).unwrap();
    assert!(create.detail.as_deref().unwrap().starts_with("no template, "), "{create:?}");
    assert!(t.span(step::BOOT).unwrap().detail.as_deref().unwrap().contains("sbx ls poll"));
    let classes: Vec<&str> = t.calls.iter().map(|c| c.class.as_str()).collect();
    for c in ["ls", "create", "exec", "policy allow"] {
        assert!(classes.contains(&c), "{c} not in {classes:?}");
    }
    // The stream ends with the same summary as the log.
    match end {
        Some(Progress::Done { error: None, summary: Some(s) }) => {
            assert_eq!(s, t.summary);
            assert!(s.starts_with("ready in "), "{s}");
        }
        other => panic!("{other:?}"),
    }
    // What the kits run after creation, watched in the background.
    assert!(eventually(|| app.timelines()[0].span(step::KIT_STARTUPS).is_some_and(|s| s.ms.is_some())).await);
    let t = app.timelines().remove(0);
    assert!(t.startups_ms.is_some(), "{:?}", t.steps);
    assert!(t.span(step::KIT_STARTUPS).unwrap().detail.as_deref().unwrap().contains("look"), "{:?}", t.steps);
}

#[tokio::test]
async fn a_new_sandbox_from_a_template_names_it() {
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), sbx_answer_booting());
    let cfg = Config { template: "my-tpl:1".into(), ..bare() };
    bring_up(&app, request(d.path(), cfg)).await.0.unwrap();
    let t = app.timelines().remove(0);
    assert_eq!(
        names_to_done(&t),
        ["prepare", "sbx_ls", "template_check", "record", "create_prepare", "sbx_create", "boot", "tooling", "policy_allow", "aliases", "done"]
    );
    once_each(&t);
    assert_eq!(t.template.as_deref(), Some("my-tpl:1"));
    assert!(t.span(step::SBX_CREATE).unwrap().detail.as_deref().unwrap().starts_with("template my-tpl:1, "));
}

#[tokio::test]
async fn a_stopped_sandbox_is_timed_through_its_attach_and_its_agents_first_prompt() {
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), |a: &[String]| match a.first().map(String::as_str) {
        Some("ls") => testing::ls(&[("myapp", "stopped")]),
        _ => sbx_answer(a),
    });
    bring_up(&app, request(d.path(), bare())).await.0.unwrap();
    let t = app.timelines().remove(0);
    assert_eq!(names_to_done(&t), ["prepare", "sbx_ls", "template_check", "record", "create_prepare", "boot", "policy_allow", "aliases", "done"]);
    assert_eq!(t.kind, "stopped");
    assert!(t.span(step::BOOT).unwrap().detail.as_deref().unwrap().starts_with("stopped"));
    assert!(t.span(step::KIT_STARTUPS).is_none(), "nothing looked at before the attach starts it");

    // The attach: its `sbx ls`, then `sbx run` (which a test cannot spawn).
    let sb = SandboxRef::parse("myapp").unwrap();
    let _ = app.open_session(&sbxw_core::session::SessionKey::agent(&sb)).await;
    let t = app.timelines().remove(0);
    assert!(t.span(step::ATTACH_LS).is_some_and(|s| s.ms.is_some()), "{:?}", t.steps);

    // The agent's first events, as its hooks report them.
    app.ingest_hook(&sb, serde_json::json!({"hook_event_name": "SessionStart", "session_id": "s1", "hook_version": 5}));
    let t = app.timelines().remove(0);
    assert!(t.span(step::AGENT_SESSION_START).is_some() && t.span(step::AGENT_IDLE).is_some(), "{:?}", t.steps);
    assert!(t.agent_ms.is_some());
    once_each(&t);
    // A second start is not the first.
    app.ingest_hook(&sb, serde_json::json!({"hook_event_name": "SessionStart", "session_id": "s2", "hook_version": 5}));
    once_each(&app.timelines()[0]);
}

#[tokio::test]
async fn a_failed_creation_still_records_what_ran() {
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), |a: &[String]| match a.first().map(String::as_str) {
        Some("create") => Output::fail(1, "boom: /secret/workspace/path"),
        _ => sbx_answer(a),
    });
    let (out, end) = bring_up(&app, request(d.path(), bare())).await;
    assert!(out.is_err());
    let t = app.timelines().remove(0);
    assert_eq!(names_to_done(&t), ["prepare", "sbx_ls", "template_check", "record", "create_prepare", "sbx_create", "done"]);
    assert_eq!(t.error.as_deref(), Some("failed during create"));
    assert!(t.summary.starts_with("failed after "), "{}", t.summary);
    assert!(t.calls.iter().any(|c| c.class == "create" && !c.ok));
    assert!(!serde_json::to_string(&t).unwrap().contains("secret"), "sbx's message is not kept");
    assert!(matches!(end, Some(Progress::Done { error: Some(_), summary: Some(_) })));
    assert!(t.span(step::KIT_STARTUPS).is_none());
}

#[tokio::test]
async fn nothing_secret_is_kept() {
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), sbx_answer_booting());
    let cfg = Config { env: [("MY_TOKEN".to_string(), "env-value-123".to_string())].into(), ..bare() };
    let mut req = request(d.path(), cfg);
    req.api_key = Some("sk-ant-key-456".into());
    bring_up(&app, req).await.0.unwrap();
    let t = app.timelines().remove(0);
    assert!(t.span(step::SECRET).is_some(), "{:?}", t.steps);
    let kept = std::fs::read_to_string(app.timings_file()).unwrap() + &serde_json::to_string(&t).unwrap();
    let ws = d.path().join("myapp").to_string_lossy().into_owned();
    for secret in ["env-value-123", "MY_TOKEN", "sk-ant-key-456", ws.as_str(), &app.store.token(&t_ref()).unwrap()] {
        assert!(!kept.contains(secret), "{secret} kept");
    }
}

fn t_ref() -> SandboxRef {
    SandboxRef::parse("myapp").unwrap()
}

#[tokio::test]
async fn fifty_are_kept_and_they_survive_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), sbx_answer);
    for i in 0..(KEEP + 3) {
        let clock = Clock::new(&SandboxRef::parse(&format!("s{i}")).unwrap());
        let t = clock.finish(false);
        app.timing_finished(&clock, t);
    }
    let all = app.timelines();
    assert_eq!(all.len(), KEEP);
    assert_eq!(all[0].sandbox, format!("s{}", KEEP + 2), "newest first");
    assert_eq!(all[KEEP - 1].sandbox, "s3", "the oldest went");
    let ids: std::collections::HashSet<u64> = all.iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), KEEP, "ids are unique");
    drop(app);
    let (again, _) = app_in_with(d.path(), sbx_answer);
    assert_eq!(again.timelines(), all);
}

#[test]
fn a_damaged_entry_costs_that_entry_only() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("timings.json");
    std::fs::write(&path, r#"{"timelines": [{"id": 1, "sandbox": "a", "host": "local", "started_at": 1, "kind": "new"}, {"id": "bad"}]}"#).unwrap();
    let book = load(&path);
    assert_eq!(book.timelines().len(), 1);
    std::fs::write(&path, "not json").unwrap();
    assert!(load(&path).timelines().is_empty());
}

#[test]
fn the_summary_line_reads_total_then_slow_steps_in_order() {
    let span = |name: &str, start_ms: u64, ms: u64| Span { name: name.into(), start_ms, ms: Some(ms), detail: None, unfinished: false };
    let mut t = Timeline {
        steps: vec![
            span("prepare", 0, 12),
            span("sbx_ls", 12, 420),
            span("sbx_create", 500, 28_300),
            span("boot", 28_800, 6_040),
            span("tooling", 34_900, 3_100),
            span("policy_allow", 34_900, 199),
            span("done", 41_000, 0),
        ],
        ready_ms: Some(41_000),
        ..Default::default()
    };
    assert_eq!(summary(&t), "ready in 41 s: sbx ls 420 ms, create 28 s, boot 6.0 s, tooling 3.1 s");
    t.error = Some("failed during tooling".into());
    t.steps[4].unfinished = true;
    assert_eq!(summary(&t), "failed after 41 s: sbx ls 420 ms, create 28 s, boot 6.0 s, tooling 3.1 s (unfinished)");
    assert_eq!(human(185_000), "3 min 5 s");
    assert_eq!(human(999), "999 ms");
}

#[tokio::test]
async fn the_route_serves_them_newest_first() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let d = tempfile::tempdir().unwrap();
    let (app, _) = app_in_with(d.path(), sbx_answer);
    for n in ["first", "second"] {
        let clock = Clock::new(&SandboxRef::parse(n).unwrap());
        let t = clock.finish(false);
        app.timing_finished(&clock, t);
    }
    let res = testing::signed_in(app.clone()).oneshot(Request::builder().uri("/api/timings").header("host", "127.0.0.1:7681").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), 200);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["timelines"][0]["sandbox"], "second");
    assert_eq!(v["timelines"][1]["sandbox"], "first");
}

/// An sbx whose created sandboxes run until `stopped`, whose kit install is
/// always seen running, and which counts each process listing that came
/// after an `sbx ls` reported the sandbox stopped.
fn stoppable() -> (impl Fn(&[String]) -> Output + Send + Sync + 'static, Arc<std::sync::atomic::AtomicBool>, Arc<std::sync::atomic::AtomicUsize>, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    let stopped = Arc::new(AtomicBool::new(false));
    let said_stopped = Arc::new(AtomicBool::new(false));
    let looks = Arc::new(AtomicUsize::new(0));
    let late = Arc::new(AtomicUsize::new(0));
    let (st, ss, lk, lt) = (stopped.clone(), said_stopped.clone(), looks.clone(), late.clone());
    let made = Mutex::new(Vec::<String>::new());
    let answer = move |a: &[String]| {
        let joined = a.join(" ");
        match a.first().map(String::as_str) {
            Some("create") => {
                made.lock().unwrap().push(a[4].clone());
                Output::ok("")
            }
            Some("ls") => {
                let state = if st.load(SeqCst) { "stopped" } else { "running" };
                if st.load(SeqCst) {
                    ss.store(true, SeqCst);
                }
                testing::ls(&made.lock().unwrap().iter().map(|n| (n.as_str(), state)).collect::<Vec<_>>())
            }
            Some("rm") => Output::ok(r#"[{"action":"removed","name":"myapp"}]"#),
            Some("exec") if joined.contains("/proc/") => {
                lk.fetch_add(1, SeqCst);
                if ss.load(SeqCst) {
                    lt.fetch_add(1, SeqCst);
                }
                Output::ok("/sbin/init \nbash /k/install.sh \n")
            }
            _ => sbx_answer(a),
        }
    };
    (answer, stopped, looks, late)
}

#[tokio::test]
async fn a_sandbox_found_stopped_ends_the_startup_watch_without_another_look() {
    use std::sync::atomic::Ordering::SeqCst;
    let d = tempfile::tempdir().unwrap();
    let (answer, stopped, looks, late) = stoppable();
    let (app, _) = app_in_with(d.path(), answer);
    bring_up(&app, request(d.path(), Config { kits: vec![kit_with_startup(d.path())], ..bare() })).await.0.unwrap();
    assert!(eventually(|| looks.load(SeqCst) >= 2).await, "the watch looks while it runs");
    // Stopped outside the daemon (`sbx stop`, sbx's idle stop).
    stopped.store(true, SeqCst);
    assert!(eventually(|| app.timelines()[0].span(step::KIT_STARTUPS).is_some_and(|s| s.ms.is_some())).await);
    let t = app.timelines().remove(0);
    let s = t.span(step::KIT_STARTUPS).unwrap();
    assert!(s.detail.as_deref().unwrap().starts_with("the sandbox stopped before the startups ended"), "{s:?}");
    assert!(s.unfinished && t.startups_ms.is_none());
    tokio::time::sleep(WATCH_POLL * 5).await;
    assert_eq!(late.load(SeqCst), 0, "no sbx exec after sbx ls said stopped");
}

#[tokio::test]
async fn the_daemons_own_remove_ends_the_startup_watch_at_once() {
    use std::sync::atomic::Ordering::SeqCst;
    let d = tempfile::tempdir().unwrap();
    // `ls` keeps saying running: only the cancel can end this watch.
    let (answer, _, looks, _) = stoppable();
    let (app, fake) = app_in_with(d.path(), answer);
    bring_up(&app, request(d.path(), Config { kits: vec![kit_with_startup(d.path())], ..bare() })).await.0.unwrap();
    let id = app.timelines()[0].id;
    assert!(eventually(|| looks.load(SeqCst) >= 2).await);
    app.remove(&[t_ref()]).await.unwrap();
    let rm_at = fake.calls.lock().unwrap().iter().position(|c| c[0] == "rm").unwrap();
    tokio::time::sleep(WATCH_POLL * 5).await;
    let after: Vec<String> = fake.calls.lock().unwrap()[rm_at..].iter().map(|c| c.join(" ")).collect();
    assert!(!after.iter().any(|c| c.starts_with("exec ")), "no look after the remove: {after:?}");
    let t = app.timelines().into_iter().find(|t| t.id == id).unwrap();
    let s = t.span(step::KIT_STARTUPS).unwrap();
    assert!(s.detail.as_deref().unwrap().contains("stopped or removed"), "{s:?}");
}
