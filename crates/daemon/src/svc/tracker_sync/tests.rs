//! Ticket sync with a fake sbx and a fake script answering as the contract
//! says: import, update, removal, `closed upstream`; agents never see
//! Backlog, the phone shows it read-only; write-backs for every column, merged outside sbxw included; the
//! queue across a failure and a restart; auto-close; status and one Board
//! warning; a disconnect discards the queue; the sync sandbox's life; nothing of a ticket's text logged.

use super::*;
use crate::testing::{app_in_with, ls, sbx_answer};
use sbxw_core::plans::{Ext, MergeResult, PlanThread, ThreadState, Variant};
use sbxw_sbx::runner::Output;
use std::sync::Mutex as M;

const SANDBOX: &str = "web";
const NAME: &str = crate::svc::tracker_sandbox::NAME;

/// What the fake tracker and the fake sbx hold.
#[derive(Default)]
struct World {
    issues: M<Vec<Value>>,
    /// The script's requests, in order.
    requests: M<Vec<Value>>,
    /// An op that fails, and with which kind.
    fail: M<Option<(String, String)>>,
    created: M<bool>,
    running: M<bool>,
    no_server: M<bool>,
    /// Every sbx call (argv).
    calls: M<Vec<Vec<String>>>,
}

impl World {
    fn issue(&self, n: u64, title: &str, labels: &[&str]) {
        let mut g = self.issues.lock().unwrap();
        g.retain(|i| i["number"] != n);
        g.push(json!({ "key": format!("acme/web#{n}"), "number": n, "title": title, "body": format!("Body of {title}"), "labels": labels, "state": "open",
            "url": format!("https://github.com/acme/web/issues/{n}"), "updated_at": "2026-11-26T10:00:00Z", "attachments": [] }));
    }
    fn ops(&self, op: &str) -> Vec<Value> {
        self.requests.lock().unwrap().iter().filter(|r| r["op"] == op).cloned().collect()
    }
    fn called(&self, first: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| c.first().map(String::as_str) == Some(first)).count()
    }
}

fn answer(w: Arc<World>) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
    move |a: &[String]| {
        w.calls.lock().unwrap().push(a.to_vec());
        let first = a.first().map(String::as_str);
        match first {
            Some("mcp") if a.get(1).map(String::as_str) == Some("ls") => {
                if *w.no_server.lock().unwrap() {
                    Output::ok("[]")
                } else {
                    Output::ok(r#"[{"name":"github","type":"remote"}]"#)
                }
            }
            Some("ls") => {
                if *w.created.lock().unwrap() {
                    ls(&[(NAME, if *w.running.lock().unwrap() { "running" } else { "stopped" })])
                } else {
                    ls(&[])
                }
            }
            Some("create") => {
                *w.created.lock().unwrap() = true;
                Output::ok("")
            }
            Some("run") => {
                *w.running.lock().unwrap() = true;
                Output::ok("")
            }
            Some("stop") => {
                *w.running.lock().unwrap() = false;
                Output::ok("")
            }
            Some("exec") if a.iter().any(|x| x == "node") => {
                let req: Value = serde_json::from_str(a.last().unwrap().strip_prefix('=').expect("the =value form")).unwrap();
                w.requests.lock().unwrap().push(req.clone());
                let op = req["op"].as_str().unwrap_or("").to_string();
                if let Some((f, kind)) = w.fail.lock().unwrap().clone() {
                    if f == op {
                        return Output::ok(&format!("SBXW-TRACKER {}\n", json!({ "ok": false, "error": { "kind": kind, "message": "nope" } })));
                    }
                }
                let body = match op.as_str() {
                    "probe" => json!({ "ok": true, "tools": ["issue_read"], "identity": "someone", "via": "direct", "ops": { "list": "list_issues" } }),
                    "list" => json!({ "ok": true, "issues": w.issues.lock().unwrap().clone(), "next_page": null }),
                    "comment" => json!({ "ok": true, "comment_id": req["comment_id"].as_str().unwrap_or("c-77") }),
                    "fetch" => json!({ "ok": true, "path": "/tmp/sbxw-tracker/x1/shot.png", "name": "shot.png", "bytes": 72 }),
                    _ => json!({ "ok": true }),
                };
                Output::ok(&format!("some noise\nSBXW-TRACKER {body}\n"))
            }
            Some("exec") if a.iter().any(|x| x == "head") => {
                let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
                b.extend([0u8; 64]);
                Output { code: Some(0), stdout: b, stderr: vec![] }
            }
            _ => sbx_answer(a),
        }
    }
}

fn setup() -> (tempfile::TempDir, Arc<App>, Arc<World>) {
    let d = tempfile::tempdir().unwrap();
    let w = Arc::new(World::default());
    let (app, _) = app_in_with(d.path(), answer(w.clone()));
    (d, app, w)
}

fn sb() -> SandboxRef {
    SandboxRef::parse(SANDBOX).unwrap()
}

fn set(app: &App, auto_close: bool) {
    app.tracker_source_set(&sb(), SourceDraft { repo: Some("acme/web".into()), auto_close: Some(auto_close), ..Default::default() }).unwrap();
}

fn col(app: &App, n: u32) -> Option<Column> {
    app.tickets_board(Some(&sb()), true).columns.iter().find(|c| c.cards.iter().any(|x| x.n == Some(n))).map(|c| c.id)
}

fn by_key(app: &App, key: &str) -> Option<tickets::Ticket> {
    app.tickets_with(SANDBOX, |b| b.by_key(key).cloned())
}

async fn refresh(app: &Arc<App>) {
    app.tracker_refresh(&sb()).unwrap();
    app.tracker_tick().await;
}

fn th(name: &str, ticket: Option<u32>) -> PlanThread {
    PlanThread { name: name.into(), goal: format!("Build {name}."), scope: String::new(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket }
}

#[tokio::test]
async fn issues_are_imported_updated_removed_and_closed_upstream_marked() {
    let (_d, app, w) = setup();
    w.issue(1, "Fix login", &["sbxw", "bug"]);
    w.issue(2, "Add export", &["sbxw"]);
    w.issue(3, "Not for sbxw", &["other"]);
    set(&app, false);
    app.tracker_tick().await;
    let a = by_key(&app, "acme/web#1").expect("imported");
    let b = by_key(&app, "acme/web#2").expect("imported");
    assert!(by_key(&app, "acme/web#3").is_none(), "an issue without the filter label is not imported");
    assert_eq!((a.title.as_str(), col(&app, a.n)), ("Fix login", Some(Column::Backlog)));
    assert_eq!(a.origin.as_ref().unwrap().labels, ["sbxw", "bug"]);
    let list = &w.ops("list")[0];
    assert_eq!((list["repo"].as_str(), list["label"].as_str()), (Some("acme/web"), Some("sbxw")));
    let v = app.tracker_source_view(&sb()).await;
    assert_eq!((v["configured"].as_bool(), v["status"]["state"].as_str(), v["status"]["via"].as_str()), (Some(true), Some("ok"), Some("direct")));
    assert!(v["status"]["last_ok_ms"].is_u64());

    // Updated from the tracker while in Backlog.
    w.issue(1, "Fix the login page", &["sbxw", "bug", "ui"]);
    refresh(&app).await;
    assert_eq!(by_key(&app, "acme/web#1").unwrap().title, "Fix the login page");
    assert!(w.ops("comment").is_empty(), "nothing is written back for Backlog");

    // Backlog does not count as Planned. The phone shows it, read-only:
    // each card with its issue, and no write permission.
    assert_eq!(app.tickets_planned_count(SANDBOX), 0);
    let phone = crate::http::remote_kanban::board_view(&app, &sb());
    let backlog = &phone["columns"][0];
    assert_eq!((backlog["id"].as_str(), backlog["count"].as_u64()), (Some("backlog"), Some(2)), "{phone}");
    let o = &backlog["cards"][0]["origin"];
    assert_eq!(o["tracker"], "github");
    assert!(o["key"].as_str().unwrap().starts_with("acme/web#"), "{o}");
    assert!(o["url"].as_str().unwrap().starts_with("https://github.com/acme/web/issues/"), "{o}");
    assert!(backlog["cards"][0].get("can").is_none() && backlog["cards"][0].get("can_plan").is_none(), "read-only on the phone");
    assert!(phone["summary"].as_str().unwrap().contains("2 backlog"), "{phone}");

    // B is planned; then both issues leave: A (Backlog) goes, B is marked.
    app.ticket_plan(&sb(), b.n).unwrap();
    assert_eq!(app.tickets_planned_count(SANDBOX), 1);
    w.issues.lock().unwrap().clear();
    refresh(&app).await;
    assert!(by_key(&app, "acme/web#1").is_none(), "a Backlog ticket whose issue went is removed");
    let b2 = by_key(&app, "acme/web#2").unwrap();
    assert!(b2.closed_upstream());
    assert_eq!(col(&app, b.n), Some(Column::Planned), "work is never stopped");
    let card = app.tickets_board(Some(&sb()), true).columns[1].cards.iter().find(|c| c.n == Some(b.n)).cloned().unwrap();
    assert!(card.marks.iter().any(|m| m.id == "closed-upstream"));
    // Back with its label: the mark goes, and it is not imported twice.
    w.issue(2, "Add export", &["sbxw"]);
    refresh(&app).await;
    assert!(!by_key(&app, "acme/web#2").unwrap().closed_upstream());
    assert_eq!(app.tickets_with(SANDBOX, |b| b.tickets.len()), 1);
    // Closed upstream, label still on: the same mark.
    w.issues.lock().unwrap()[0]["state"] = json!("closed");
    refresh(&app).await;
    assert!(by_key(&app, "acme/web#2").unwrap().closed_upstream(), "a closed issue marks it as an unlabelled one does");
    assert_eq!(col(&app, b.n), Some(Column::Planned));
}

#[tokio::test]
async fn an_attachment_is_imported_and_one_that_fails_never_fails_the_import() {
    let (_d, app, w) = setup();
    w.issue(1, "With a shot", &["sbxw"]);
    w.issues.lock().unwrap()[0]["attachments"] = json!([{ "name": "shot.png", "url": "https://github.com/user-attachments/assets/1" }]);
    set(&app, false);
    app.tracker_tick().await;
    let t = by_key(&app, "acme/web#1").unwrap();
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.origin.as_ref().unwrap().fetched.len(), 1);
    assert!(w.calls.lock().unwrap().iter().any(|c| c.iter().any(|x| x == "rm")), "the fetched file is removed from the sync sandbox");

    w.issue(2, "Private shot", &["sbxw"]);
    w.issues.lock().unwrap()[1]["attachments"] = json!([{ "name": "p.png", "url": "https://github.com/user-attachments/assets/2" }]);
    *w.fail.lock().unwrap() = Some(("fetch".into(), "network".into()));
    refresh(&app).await;
    let t = by_key(&app, "acme/web#2").expect("imported anyway");
    assert!(t.files.is_empty());
    assert_eq!(t.origin.as_ref().unwrap().not_fetched[0].url, "https://github.com/user-attachments/assets/2");
    assert_eq!(app.tracker_source_view(&sb()).await["status"]["state"], "ok", "a download never fails the source");
}

#[tokio::test]
async fn agents_never_see_read_or_plan_a_backlog_ticket() {
    let (_d, app, w) = setup();
    w.issue(5, "Secret backlog idea", &["sbxw"]);
    set(&app, false);
    app.tracker_tick().await;
    let n = by_key(&app, "acme/web#5").unwrap().n;
    assert!(!app.tickets_agent_list(&sb()).contains("Secret backlog idea"));
    let e = app.tickets_agent_read(&sb(), &tickets::num(n)).await.unwrap_err();
    assert!(e.contains("has no ticket"), "reads as no ticket at all: {e}");
    let now = now_ms();
    let p = app.plans.lock().unwrap().propose("p-1".into(), SANDBOX, "s", vec![th("idea", Some(n))], Variant::Split, "why", None, now).unwrap();
    assert_eq!(app.ticket_link_notes(&p).len(), 1);
    app.tickets_follow(&p);
    let p = app.plans.lock().unwrap().approve("p-1", vec![th("idea", None)], None, None, now + 1).unwrap();
    app.tickets_follow(&p);
    assert!(by_key(&app, "acme/web#5").unwrap().link.is_none(), "a plan never takes a Backlog ticket");
    assert_eq!(col(&app, n), Some(Column::Backlog));
    // Once the person plans it, the agent sees it.
    app.ticket_plan(&sb(), n).unwrap();
    assert!(app.tickets_agent_list(&sb()).contains("Secret backlog idea"));
    assert!(app.tickets_agent_read(&sb(), &tickets::num(n)).await.unwrap().contains("From GitHub issue acme/web#5"));
}

/// Drives one planned imported ticket through every column; returns the
/// write-backs sent, as (comment, label) pairs, and whether a close went.
async fn through_the_columns(auto_close: bool, outside: bool) -> (Vec<(String, String)>, usize) {
    let (_d, app, w) = setup();
    w.issue(9, "Paginate", &["sbxw"]);
    set(&app, auto_close);
    app.tracker_tick().await;
    let n = by_key(&app, "acme/web#9").unwrap().n;
    app.ticket_plan(&sb(), n).unwrap();
    app.tracker_tick().await;
    let now = now_ms();
    let p = app.plans.lock().unwrap().propose("p-1".into(), SANDBOX, "s", vec![th("pages", Some(n))], Variant::Split, "why", None, now).unwrap();
    app.tickets_follow(&p);
    let p = app.plans.lock().unwrap().approve("p-1", vec![th("pages", None)], None, None, now + 1).unwrap();
    app.tickets_follow(&p);
    app.tracker_tick().await;
    let p = app.plans.lock().unwrap().set_thread(SANDBOX, "pages", ThreadState::Done, "", false, now_ms()).unwrap();
    app.tickets_follow(&p);
    app.tracker_tick().await;
    if outside {
        // Git shows the branch in its base: no plan event says so.
        let ext: HashMap<String, Ext> = [("pages".to_string(), Ext::Outside)].into_iter().collect();
        app.plan_ext.lock().unwrap().insert("p-1".into(), (std::time::Instant::now(), ext));
        app.tracker_tick().await;
    } else {
        let p = app.plans.lock().unwrap().propose_merge("p-1", SANDBOX, vec!["pages".into()], "", vec![], now_ms()).unwrap();
        app.tickets_follow(&p);
        app.tracker_tick().await;
        let p = {
            let mut g = app.plans.lock().unwrap();
            g.start_merge("p-1", None, None, now_ms()).unwrap();
            g.merge_result("p-1", MergeResult { thread: "pages".into(), ok: true, commit: Some("0123456789abcdef0123".into()), error: None, note: None }, now_ms()).unwrap();
            g.finish_merge("p-1", now_ms()).unwrap()
        };
        app.tickets_follow(&p);
        app.tracker_tick().await;
    }
    assert_eq!(col(&app, n), Some(Column::Merged));
    let comments: Vec<String> = w.ops("comment").iter().map(|r| r["body"].as_str().unwrap().to_string()).collect();
    let labels: Vec<String> = w.ops("label").iter().map(|r| r["add"].as_str().unwrap().to_string()).collect();
    for r in w.ops("label") {
        assert_eq!(r["remove_prefix"], "sbxw:");
    }
    // One comment, kept up to date: the first creates it, the rest update it.
    let ids: Vec<Value> = w.ops("comment").iter().map(|r| r["comment_id"].clone()).collect();
    assert_eq!(ids[0], Value::Null);
    assert!(ids[1..].iter().all(|i| i == "c-77"), "{ids:?}");
    assert_eq!(by_key(&app, "acme/web#9").unwrap().origin.unwrap().comment_id.as_deref(), Some("c-77"));
    assert!(app.tracker_with(SANDBOX, |x| x.queue.is_empty()));
    (comments.into_iter().zip(labels).collect(), w.ops("close").len())
}

#[tokio::test]
async fn every_column_is_written_back_and_auto_close_closes_after_the_merge() {
    let (sent, closes) = through_the_columns(true, false).await;
    assert_eq!(
        sent,
        [
            ("[sbxw] planned in sbxw", "sbxw:planned"),
            ("[sbxw] in progress on branch sbxw/pages", "sbxw:in-progress"),
            ("[sbxw] in test on branch sbxw/pages", "sbxw:in-test"),
            ("[sbxw] ready to merge on branch sbxw/pages", "sbxw:ready"),
            ("[sbxw] merged in 012345678", "sbxw:merged"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    assert_eq!(closes, 1);
    let (_, closes) = through_the_columns(false, false).await;
    assert_eq!(closes, 0, "no auto-close unless the source says so");
}

#[tokio::test]
async fn a_merge_outside_sbxw_is_written_back_too() {
    let (sent, closes) = through_the_columns(true, true).await;
    assert_eq!(sent.last().unwrap(), &("[sbxw] merged outside sbxw on branch sbxw/pages".to_string(), "sbxw:merged".to_string()));
    assert_eq!(closes, 1);
}

#[tokio::test]
async fn the_queue_survives_a_failure_and_a_restart_and_is_never_dropped() {
    let d = tempfile::tempdir().unwrap();
    let w = Arc::new(World::default());
    let (app, _) = app_in_with(d.path(), answer(w.clone()));
    w.issue(1, "A", &["sbxw"]);
    set(&app, false);
    app.tracker_tick().await;
    let n = by_key(&app, "acme/web#1").unwrap().n;
    *w.fail.lock().unwrap() = Some(("label".into(), "network".into()));
    app.ticket_plan(&sb(), n).unwrap();
    app.tracker_tick().await;
    let q = app.tracker_with(SANDBOX, |x| x.queue.clone());
    assert_eq!(q.len(), 1);
    assert_eq!((q[0].attempts, q[0].error, q[0].done.clone()), (1, Some(ErrorKind::Network), vec!["comment".to_string()]));
    assert!(q[0].next_ms > now_ms(), "retried after a backoff");
    let v = app.tracker_source_view(&sb()).await;
    assert_eq!((v["status"]["state"].as_str(), v["status"]["error"].as_str(), v["status"]["pending"].as_u64()), (Some("failed"), Some("network"), Some(1)));
    drop(app);

    // A restart: the queue is read back, and sent once the tracker answers.
    *w.fail.lock().unwrap() = None;
    let (again, _) = app_in_with(d.path(), answer(w.clone()));
    let _ = again.tracker_update(SANDBOX, |x| x.queue[0].next_ms = 0);
    again.tracker_tick().await;
    assert!(again.tracker_with(SANDBOX, |x| x.queue.is_empty()));
    assert_eq!(w.ops("comment").len(), 1, "the comment already sent is not sent twice");
    assert_eq!(w.ops("label").len(), 2);
    assert_eq!(again.tracker_source_view(&sb()).await["status"]["state"], "ok");
}

#[tokio::test]
async fn a_failure_sets_the_status_with_a_hint_and_warns_the_board_once() {
    let (_d, app, w) = setup();
    *w.no_server.lock().unwrap() = true;
    set(&app, false);
    app.tracker_tick().await;
    let v = app.tracker_source_view(&sb()).await;
    assert_eq!((v["status"]["state"].as_str(), v["status"]["error"].as_str()), (Some("failed"), Some("missing_server")));
    assert!(v["status"]["hint"].as_str().unwrap().contains("Add the GitHub MCP server in Setup, MCP"));
    assert_eq!(w.called("create"), 0, "nothing starts before the server is there");
    refresh(&app).await;
    let warnings = || app.board_view(SANDBOX).to_string().matches("Ticket sync with GitHub").count();
    assert_eq!(warnings(), 1, "one warning per kind");
    // Another kind: a second warning; a success clears them for next time.
    *w.no_server.lock().unwrap() = false;
    *w.fail.lock().unwrap() = Some(("list".into(), "auth".into()));
    refresh(&app).await;
    assert_eq!(warnings(), 2);
    assert!(app.tracker_source_view(&sb()).await["status"]["hint"].as_str().unwrap().contains("Sign in to GitHub again"));
    *w.fail.lock().unwrap() = Some(("list".into(), "rate_limit".into()));
    refresh(&app).await;
    let v = app.tracker_source_view(&sb()).await;
    assert_eq!(v["status"]["error"], "rate_limit");
    *w.fail.lock().unwrap() = None;
    refresh(&app).await;
    assert_eq!(app.tracker_source_view(&sb()).await["status"]["state"], "ok");
    assert!(app.tracker_with(SANDBOX, |x| x.status.warned.is_empty()));
}

#[tokio::test]
async fn the_sync_sandbox_is_hidden_small_and_follows_the_sources() {
    let (_d, app, w) = setup();
    app.tracker_tick().await;
    assert_eq!(w.called("create"), 0, "nothing on by default");
    set(&app, false);
    app.tracker_tick().await;
    let create = w.calls.lock().unwrap().iter().find(|c| c[0] == "create").cloned().unwrap();
    assert!(create.windows(2).any(|p| p == ["--name", NAME]));
    assert!(create.windows(2).any(|p| p == ["--cpus", "1"]) && create.windows(2).any(|p| p == ["--memory", "1g"]));
    let kits: Vec<&String> = create.windows(2).filter(|p| p[0] == "--kit").map(|p| &p[1]).collect();
    assert_eq!(kits.len(), 1);
    assert!(kits[0].ends_with("/kits/base"), "{kits:?}");
    assert!(!create.iter().any(|x| x == "-p" || x == "--mount" || x.ends_with(":ro")), "nothing of a project: {create:?}");
    let loads = || w.calls.lock().unwrap().iter().filter(|c| c.starts_with(&["mcp".to_string(), "load".to_string()])).count();
    assert_eq!(loads(), 1);
    assert!(!app.sandboxes().await.iter().any(|s| s.name == NAME), "hidden from the list");
    assert!(app.open_session(&sbxw_core::session::SessionKey::agent(crate::svc::tracker_sandbox::sandbox())).await.is_err(), "no agent ever");
    // Off: stopped. On again: started, and the server loaded again.
    app.tracker_source_remove(&sb()).unwrap();
    app.tracker_tick().await;
    assert!(!*w.running.lock().unwrap());
    set(&app, false);
    app.tracker_tick().await;
    assert!(*w.running.lock().unwrap());
    assert_eq!(loads(), 2, "loaded again after every start");
    assert_eq!(w.called("create"), 1);
}

#[tokio::test]
async fn removing_the_source_drops_backlog_and_keeps_the_rest() {
    let (_d, app, w) = setup();
    w.issue(1, "A", &["sbxw"]);
    w.issue(2, "B", &["sbxw"]);
    set(&app, false);
    app.tracker_tick().await;
    let b = by_key(&app, "acme/web#2").unwrap().n;
    // Its write-back fails, so it waits in the queue.
    *w.fail.lock().unwrap() = Some(("comment".into(), "network".into()));
    app.ticket_plan(&sb(), b).unwrap();
    app.tracker_tick().await;
    assert_eq!(app.tracker_with(SANDBOX, |x| x.queue.len()), 1);
    app.tracker_source_remove(&sb()).unwrap();
    assert!(by_key(&app, "acme/web#1").is_none());
    assert!(by_key(&app, "acme/web#2").unwrap().origin.is_some());
    assert!(app.tracker_with(SANDBOX, |x| x.queue.is_empty()), "a disconnect discards the queued write-backs");
    let v = app.tracker_source_view(&sb()).await;
    assert_eq!((v["configured"].as_bool(), v["status"]["state"].as_str(), v["status"]["pending"].as_u64()), (Some(false), Some("off"), Some(0)));
    assert!(app.tracker_refresh(&sb()).is_err());
    // On again: nothing of before is replayed.
    *w.fail.lock().unwrap() = None;
    let before = w.ops("comment").len();
    set(&app, false);
    app.tracker_tick().await;
    assert_eq!(w.ops("comment").len(), before, "no replay on reconnect");
    assert!(app.tracker_with(SANDBOX, |x| x.queue.is_empty()));
    // Another repository set over a queued write-back: discarded too.
    let _ = app.tracker_update(SANDBOX, |x| x.push(WriteBack { n: b, key: "acme/web#2".into(), ..Default::default() }));
    app.tracker_source_set(&sb(), SourceDraft { repo: Some("acme/other".into()), ..Default::default() }).unwrap();
    assert!(app.tracker_with(SANDBOX, |x| x.queue.is_empty()), "nothing queued for the old repository is sent");
}

#[tokio::test]
async fn an_imported_ticket_is_not_edited_in_sbxw() {
    let (_d, app, w) = setup();
    w.issue(1, "A", &["sbxw"]);
    set(&app, false);
    app.tracker_tick().await;
    let n = by_key(&app, "acme/web#1").unwrap().n;
    let d = tickets::Draft { title: Some("changed".into()), ..Default::default() };
    assert!(app.ticket_edit(&sb(), n, d, None).unwrap_err().contains("follows its issue"));
    assert!(app.ticket_attach(&sb(), n, "x.txt", b"words").is_err());
    // A deleted one is not imported again.
    app.ticket_delete(&sb(), n).unwrap();
    refresh(&app).await;
    assert!(by_key(&app, "acme/web#1").is_none());
}

// ── Nothing of a ticket's text in the log ─────────────────────────────────

struct Capture(Arc<M<String>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, _f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{v:?}\n"));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        let mut s = String::new();
        e.record(&mut Fields(&mut s));
        self.0.lock().unwrap().push_str(&s);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_of_a_ticket_s_text_is_logged() {
    let log = Arc::new(M::new(String::new()));
    let _g = tracing::subscriber::set_default(Capture(log.clone()));
    let (_d, app, w) = setup();
    w.issue(1, "Zebra title words", &["sbxw", "Llama-label"]);
    w.issues.lock().unwrap()[0]["attachments"] = json!([{ "name": "Okapi-file.png", "url": "https://github.com/user-attachments/assets/1" }]);
    set(&app, false);
    app.tracker_tick().await;
    let n = by_key(&app, "acme/web#1").unwrap().n;
    app.ticket_plan(&sb(), n).unwrap();
    *w.fail.lock().unwrap() = Some(("comment".into(), "auth".into()));
    app.tracker_tick().await;
    let text = log.lock().unwrap().clone();
    assert!(text.contains("tracker sync of web"), "the capture works: {text}");
    for secret in ["Zebra", "Body of", "Llama", "Okapi", "nope"] {
        assert!(!text.contains(secret), "{secret} logged:\n{text}");
    }
}
