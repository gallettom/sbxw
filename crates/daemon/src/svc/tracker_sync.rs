//! Ticket sync with a tracker (plan A: GitHub issues), per sandbox:
//! issues carrying the source's filter label become tickets in Backlog, and
//! each column change of an imported ticket is written back to its issue as
//! one fixed comment and one `sbxw:` label (`sbxw_core::tracker`).
//!
//! * The source is the person's, set per sandbox from the UI and kept beside
//!   the ticket book (`state/tickets/<sandbox>/tracker.json`), never in
//!   `sbxw.toml`. Nothing is on by default; the repository git shows only
//!   pre-fills the dialog.
//! * Every tracker call is the sync script, run in the daemon's sync
//!   sandbox (`svc::tracker_sandbox`), bounded in time; its answers are data.
//! * A source is polled every [`POLL_MS`] and on a refresh. Write-backs are
//!   queued on disk the moment a column changes (the board's live column,
//!   so a merge outside sbxw counts too), sent at once, retried with backoff,
//!   never dropped while the source is on, in order; turning the source off
//!   discards them.
//! * A failure is never silent: the source's status says what and what to
//!   do, and the Board gets one warning per kind until a success.
//! * The log gets numbers, keys and kinds, never a title, a body, a label or
//!   a file name.

use crate::app::{now_ms, App};
use sbxw_core::tickets::{self, Column};
use sbxw_core::tracker::{self, ErrorKind, Failure, Issue, Source, SourceDraft, Status, Sync as SyncFile, SyncState, WriteBack, POLL_MS};
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the engine looks at its sources and queues (a refresh or a
/// column change wakes it at once).
pub const TICK: Duration = Duration::from_secs(30);
const PROBE_BOUND: Duration = Duration::from_secs(60);
const LIST_BOUND: Duration = Duration::from_secs(120);
const WRITE_BOUND: Duration = Duration::from_secs(60);
const FETCH_BOUND: Duration = Duration::from_secs(180);
/// A probe is asked again after this.
const PROBE_EVERY_MS: u64 = 24 * 3600 * 1000;

/// The engine's state: each sandbox's sync, loaded on first use.
#[derive(Default)]
pub struct Engine {
    syncs: Mutex<HashMap<String, SyncFile>>,
    listed: Mutex<bool>,
    refresh: Mutex<HashSet<String>>,
    wake: tokio::sync::Notify,
    /// One pass at a time.
    run: tokio::sync::Mutex<()>,
}

impl App {
    fn tracker_file(&self, sb: &str) -> Option<std::path::PathBuf> {
        self.tickets_dir(sb).map(|d| d.join("tracker.json"))
    }

    fn tracker_load(&self, sb: &str) -> SyncFile {
        let Some(f) = self.tracker_file(sb) else { return SyncFile::default() };
        match std::fs::read_to_string(&f) {
            Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
                tracing::warn!("tracker sync of {sb}: unreadable at line {} column {}; set aside", e.line(), e.column());
                let _ = std::fs::rename(&f, f.with_file_name(format!("tracker.unreadable-{}.json", now_ms())));
                SyncFile::default()
            }),
            Err(_) => SyncFile::default(),
        }
    }

    /// Runs `f` on `sb`'s sync (loaded on first use).
    pub(crate) fn tracker_with<R>(&self, sb: &str, f: impl FnOnce(&mut SyncFile) -> R) -> R {
        let fresh = !self.tracker.syncs.lock().unwrap().contains_key(sb);
        if fresh {
            let s = self.tracker_load(sb);
            self.tracker.syncs.lock().unwrap().entry(sb.to_string()).or_insert(s);
        }
        let mut g = self.tracker.syncs.lock().unwrap();
        f(g.get_mut(sb).expect("loaded above"))
    }

    /// Changes `sb`'s sync with `f` and saves it; nothing changes in memory
    /// when the save fails.
    fn tracker_update<R>(&self, sb: &str, f: impl FnOnce(&mut SyncFile) -> R) -> Result<R, String> {
        let file = self.tracker_file(sb).ok_or("bad sandbox name")?;
        self.tracker_with(sb, |s| {
            let mut next = s.clone();
            let r = f(&mut next);
            let body = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
            crate::store::write_atomic(&file, &body).map_err(|e| format!("could not save the tracker source: {e:#}"))?;
            *s = next;
            Ok(r)
        })
    }

    /// Every sandbox with a sync file, read once from disk.
    fn tracker_names(&self) -> Vec<String> {
        let mut listed = self.tracker.listed.lock().unwrap();
        if !*listed {
            let names: Vec<String> = std::fs::read_dir(self.tickets_root())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| self.tickets_dir(n).is_some_and(|d| d.join("tracker.json").is_file()))
                .collect();
            drop(listed);
            for n in names {
                self.tracker_with(&n, |_| ());
            }
            listed = self.tracker.listed.lock().unwrap();
            *listed = true;
        }
        drop(listed);
        let mut v: Vec<String> = self.tracker.syncs.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }

    fn tracker_sources(&self) -> Vec<(String, Source)> {
        self.tracker_names().into_iter().filter_map(|n| self.tracker_with(&n, |s| s.source.clone()).map(|s| (n, s))).collect()
    }

    fn tracker_wake(&self) {
        self.tracker.wake.notify_one();
    }

    // ── The person's acts ────────────────────────────────────────────────

    /// What `GET /api/tickets/:sandbox/source` answers.
    pub async fn tracker_source_view(&self, sb: &SandboxRef) -> Value {
        let s = sb.to_string();
        let sync = self.tracker_with(&s, |x| x.clone());
        let detected = self.tracker_detect(sb).await.map(|repo| json!({ "tracker": tracker::Tracker::Github, "repo": repo }));
        let src = sync.source.clone();
        let st = &sync.status;
        let state = if src.is_none() { SyncState::Off } else { st.state };
        json!({
            "configured": src.is_some(),
            "tracker": src.as_ref().map(|x| x.tracker).unwrap_or_default(),
            "repo": src.as_ref().map(|x| x.repo.clone()).unwrap_or_default(),
            "label": src.as_ref().map(|x| x.label.clone()).unwrap_or_else(|| tracker::DEFAULT_LABEL.to_string()),
            "auto_close": src.as_ref().is_some_and(|x| x.auto_close),
            "detected": detected,
            "status": {
                "state": state,
                "last_ok_ms": st.last_ok_ms,
                "error": if src.is_some() { st.error } else { None },
                "hint": if src.is_some() { st.hint.clone() } else { None },
                "retry_at_ms": st.retry_at_ms,
                "pending": sync.queue.len(),
                "via": st.via,
                "ops": st.ops,
            },
        })
    }

    /// The GitHub repository the sandbox's checkout points at, to pre-fill.
    async fn tracker_detect(&self, sb: &SandboxRef) -> Option<String> {
        let ws = self.store.workspace(sb.clone())?;
        let host = self.host_for(sb).await.ok()?;
        let url = crate::git::origin_url(&host, &ws).await?;
        tracker::detect_github(&url)
    }

    /// The person sets (or changes) the source; a sync follows at once.
    pub fn tracker_source_set(&self, sb: &SandboxRef, d: SourceDraft) -> Result<(), String> {
        let s = sb.to_string();
        let src = d.check()?;
        self.tracker_update(&s, |x| {
            let same = x.source.as_ref().is_some_and(|o| o.tracker == src.tracker && o.repo == src.repo && o.label == src.label);
            x.source = Some(src.clone());
            if !same {
                x.status = Status { state: SyncState::Syncing, last_ok_ms: x.status.last_ok_ms, ..Default::default() };
                x.last_poll_ms = 0;
            }
        })?;
        tracing::info!("tracker sync of {s}: source set ({}, auto-close {})", src.tracker.id(), src.auto_close);
        self.tracker.refresh.lock().unwrap().insert(s.clone());
        self.tickets_changed(&s, None, "source");
        self.tracker_wake();
        Ok(())
    }

    /// The person turns the source off: imported Backlog tickets go, the
    /// others keep their origin, and the write-backs still queued are
    /// discarded (a source set again later replays none of them).
    pub fn tracker_source_remove(&self, sb: &SandboxRef) -> Result<(), String> {
        let s = sb.to_string();
        let dropped = self.tracker_update(&s, |x| {
            x.source = None;
            x.status = Status::default();
            std::mem::take(&mut x.queue).len()
        })?;
        let gone = self.tickets_with(&s, |b| {
            let mut next = b.clone();
            let gone = next.drop_backlog();
            if !gone.is_empty() {
                self.tickets_save(&next)?;
                *b = next;
            }
            Ok::<_, String>(gone)
        })?;
        self.tickets_remove_files(&s, &gone);
        tracing::info!("tracker sync of {s}: source removed, {} Backlog ticket(s) dropped, {dropped} queued write-back(s) discarded", gone.len());
        self.tickets_changed(&s, None, "source");
        self.tracker_wake();
        Ok(())
    }

    pub fn tracker_refresh(&self, sb: &SandboxRef) -> Result<(), String> {
        let s = sb.to_string();
        if self.tracker_with(&s, |x| x.source.is_none()) {
            return Err("no tracker source is set for this sandbox".into());
        }
        self.tracker.refresh.lock().unwrap().insert(s);
        self.tracker_wake();
        Ok(())
    }

    // ── Column changes ───────────────────────────────────────────────────

    /// Queues a write-back for each imported ticket past Backlog whose live
    /// column differs from the one last queued. The queue is saved before
    /// the book, so a crash between them repeats a write-back, never loses one.
    pub(crate) fn tracker_column_check(&self, sb: &str) {
        let Some(src) = self.tracker_with(sb, |x| x.source.clone()) else { return };
        let plans = self.plans_of(Some(sb));
        let by: HashMap<&str, &sbxw_core::plans::Plan> = plans.iter().map(|p| (p.id.as_str(), p)).collect();
        let book = self.tickets_with(sb, |b| b.clone());
        let now = now_ms();
        let mut items = vec![];
        let mut moved: Vec<(u32, Column)> = vec![];
        for t in &book.tickets {
            let Some(o) = &t.origin else { continue };
            if o.planned_ms.is_none() {
                continue;
            }
            let (col, marks, _) = tickets::ticket_column(t, &by);
            if o.reported == Some(col) {
                continue;
            }
            moved.push((t.n, col));
            let thread = t.link.as_ref().map(|l| l.thread.as_str());
            let commit = t.link.as_ref().and_then(|l| by.get(l.plan.as_str()).and_then(|p| p.merged_results().filter(|r| r.thread == l.thread).find_map(|r| r.commit.clone())));
            let (Some(label), Some(comment)) = (tracker::state_label(col), tracker::comment_text(col, &marks, thread, commit.as_deref())) else { continue };
            items.push(WriteBack {
                n: t.n,
                key: o.key.clone(),
                column: col,
                label: label.into(),
                comment,
                close: src.auto_close && tracker::closes(col, &marks),
                queued_ms: now,
                next_ms: now,
                ..Default::default()
            });
        }
        if moved.is_empty() {
            return;
        }
        let queued = items.len();
        if let Err(e) = self.tracker_update(sb, |x| {
            for w in items {
                x.push(w);
            }
        }) {
            tracing::warn!("tracker sync of {sb}: write-backs not queued, tried again at the next change: {e}");
            return;
        }
        let saved = self.tickets_with(sb, |b| {
            let mut next = b.clone();
            for (n, col) in &moved {
                if let Some(o) = next.tickets.iter_mut().find(|t| t.n == *n).and_then(|t| t.origin.as_mut()) {
                    o.reported = Some(*col);
                }
            }
            self.tickets_save(&next)?;
            *b = next;
            Ok::<_, String>(())
        });
        if let Err(e) = saved {
            tracing::warn!("tracker sync of {sb}: {e}");
        }
        tracing::info!("tracker sync of {sb}: {queued} write-back(s) queued");
        self.tracker_wake();
    }

    // ── The engine ───────────────────────────────────────────────────────

    pub fn start_tracker_sync(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                app.tracker_tick().await;
                tokio::select! {
                    _ = tokio::time::sleep(TICK) => {}
                    _ = app.tracker.wake.notified() => {}
                }
            }
        });
    }

    /// One pass: the sync sandbox up or down, column changes caught up,
    /// each due source polled, each due queue sent.
    pub(crate) async fn tracker_tick(self: &Arc<Self>) {
        let _one = self.tracker.run.lock().await;
        let on = self.tracker_sources();
        if on.is_empty() {
            self.sync_sandbox_down().await;
            return;
        }
        let names: Vec<String> = on.iter().map(|(n, _)| n.clone()).collect();
        // What git shows of done threads (merged outside sbxw, branch gone).
        self.plans_ext_refresh(Some(&names)).await;
        for n in &names {
            self.tracker_column_check(n);
        }
        for (sb, src) in on {
            let now = now_ms();
            let sync = self.tracker_with(&sb, |x| x.clone());
            let refresh = self.tracker.refresh.lock().unwrap().remove(&sb);
            if sync.status.retry_at_ms.is_some_and(|t| t > now) {
                continue;
            }
            let poll = refresh || now >= sync.last_poll_ms + POLL_MS;
            let write = sync.queue.first().is_some_and(|w| w.next_ms <= now);
            if !poll && !write {
                continue;
            }
            let n = match self.sync_sandbox_up(src.tracker).await {
                Ok(n) => n,
                Err(f) => {
                    let _ = self.tracker_update(&sb, |x| {
                        if poll {
                            x.last_poll_ms = now;
                        }
                        if write {
                            bump(x, &f, now);
                        }
                    });
                    self.tracker_failed(&sb, &src, &f);
                    continue;
                }
            };
            if poll {
                self.tracker_poll(&sb, &src, &n, refresh).await;
            }
            self.tracker_drain(&sb, &src, &n).await;
        }
    }

    /// Reads the open labelled issues and brings the Backlog in line.
    async fn tracker_poll(&self, sb: &str, src: &Source, n: &sbxw_core::SandboxName, refresh: bool) {
        let _ = self.tracker_update(sb, |x| x.status.state = SyncState::Syncing);
        self.tickets_changed(sb, None, "source");
        let now = now_ms();
        let probe_due = refresh || self.tracker_with(sb, |x| x.status.probed_ms.is_none_or(|t| now >= t + PROBE_EVERY_MS));
        if probe_due {
            match self.sync_script(n, &tracker::probe(src.tracker), PROBE_BOUND).await {
                Ok(v) => {
                    let _ = self.tracker_update(sb, |x| x.status.set_probe(&v, now));
                }
                Err(f) => return self.tracker_poll_failed(sb, src, &f),
            }
        }
        let mut issues: Vec<Issue> = vec![];
        let mut page = Value::Null;
        let mut complete = false;
        for _ in 0..tracker::MAX_PAGES {
            match self.sync_script(n, &tracker::list(src, page.clone()), LIST_BOUND).await {
                Ok(v) => {
                    let got = v.get("issues").and_then(Value::as_array).cloned().unwrap_or_default();
                    issues.extend(got.iter().filter_map(|i| Issue::cleaned(i, src)).filter(|i| i.state == tracker::Upstream::Open && i.has_label(&src.label)));
                    page = v.get("next_page").cloned().unwrap_or(Value::Null);
                    if page.is_null() {
                        complete = true;
                        break;
                    }
                }
                Err(f) => return self.tracker_poll_failed(sb, src, &f),
            }
        }
        let (imported, removed, changed) = match self.tracker_apply(sb, src, &issues, complete) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("tracker sync of {sb}: {e}");
                return self.tracker_poll_failed(sb, src, &Failure::new(ErrorKind::BadAnswer, "the tickets could not be saved"));
            }
        };
        let sbr = SandboxRef::parse(sb).ok();
        for (tn, atts) in &imported {
            for a in atts {
                if let Some(r) = &sbr {
                    self.tracker_fetch(r, src, n, *tn, a).await;
                }
            }
        }
        let now = now_ms();
        let _ = self.tracker_update(sb, |x| {
            x.last_poll_ms = now;
            x.status.state = SyncState::Ok;
            x.status.last_ok_ms = Some(now);
            x.status.error = None;
            x.status.hint = None;
            x.status.retry_at_ms = None;
            x.status.warned.clear();
        });
        tracing::info!(
            "tracker sync of {sb}: {} issue(s) listed{}, {} imported, {removed} removed or marked",
            issues.len(),
            if complete { "" } else { " (more pages than read: none removed)" },
            imported.len()
        );
        if changed || !imported.is_empty() {
            self.tracker_column_check(sb);
        }
        self.tickets_changed(sb, None, "synced");
    }

    /// The listed issues into the book: new ones imported into Backlog,
    /// known ones updated, and (from a complete listing only) the ones no
    /// longer listed dropped from Backlog or marked `closed upstream`.
    #[allow(clippy::type_complexity)]
    fn tracker_apply(&self, sb: &str, src: &Source, issues: &[Issue], complete: bool) -> Result<(Vec<(u32, Vec<tracker::Attachment>)>, usize, bool), String> {
        let plans = self.plans_of(Some(sb));
        let by: HashMap<&str, &sbxw_core::plans::Plan> = plans.iter().map(|p| (p.id.as_str(), p)).collect();
        let now = now_ms();
        let seen: HashSet<&str> = issues.iter().map(|i| i.key.as_str()).collect();
        let ours = format!("{}#", src.repo);
        let (r, gone) = self.tickets_with(sb, |b| {
            let mut next = b.clone();
            let mut imported = vec![];
            let mut gone = vec![];
            let mut marked = 0;
            let mut changed = false;
            for i in issues {
                if next.skipped.contains(&i.key) {
                    continue;
                }
                match next.by_key(&i.key).map(|t| (t.n, tickets::ticket_column(t, &by).0)) {
                    Some((tn, col)) => changed |= next.refresh_from(tn, i, col, now),
                    None => match next.import(src.tracker, i, now) {
                        Ok(tn) => {
                            imported.push((tn, i.attachments.clone()));
                            changed = true;
                        }
                        Err(e) => {
                            tracing::warn!("tracker sync of {sb}: issue {} not imported: {e}", i.key);
                            break;
                        }
                    },
                }
            }
            if complete {
                let stale: Vec<(u32, bool)> = next
                    .tickets
                    .iter()
                    .filter_map(|t| {
                        let o = t.origin.as_ref()?;
                        let mine = o.tracker == src.tracker && o.key.starts_with(&ours);
                        (!seen.contains(o.key.as_str()) && (mine || t.in_backlog())).then_some((t.n, t.closed_upstream()))
                    })
                    .collect();
                for (tn, was) in stale {
                    match next.drop_upstream(tn, false, now) {
                        Some(t) => {
                            gone.push(t);
                            changed = true;
                        }
                        None if !was && next.get(tn).is_some_and(|t| t.closed_upstream()) => {
                            marked += 1;
                            changed = true;
                        }
                        None => {}
                    }
                }
            }
            if changed {
                if let Err(e) = self.tickets_save(&next) {
                    return (Err(e), vec![]);
                }
                *b = next;
            }
            let removed = gone.len() + marked;
            (Ok((imported, removed, changed)), gone)
        });
        self.tickets_remove_files(sb, &gone);
        r
    }

    /// One attachment of a new ticket: fetched by the script, read out of
    /// the sync sandbox, attached through the person's checks. One that
    /// fails is noted on the ticket, with its link; the import goes on.
    async fn tracker_fetch(&self, sb: &SandboxRef, src: &Source, n: &sbxw_core::SandboxName, tn: u32, a: &tracker::Attachment) {
        let got: Result<(), String> = async {
            let v = self.sync_script(n, &tracker::fetch(src.tracker, &a.url), FETCH_BOUND).await.map_err(|f| f.kind.id().to_string())?;
            let path = v.get("path").and_then(Value::as_str).ok_or("no path")?;
            let name = v.get("name").and_then(Value::as_str).map(tickets::shown_name).unwrap_or_else(|| a.name.clone());
            let bytes = self.sync_read_out(n, path, tracker::MAX_ATTACHMENT).await?;
            self.ticket_attach_checked(sb, tn, &name, &bytes).map(|_| ())
        }
        .await;
        let s = sb.to_string();
        let ok = got.is_ok();
        let _ = self.tickets_with(&s, |b| {
            let mut next = b.clone();
            let Some(t) = next.tickets.iter_mut().find(|t| t.n == tn) else { return Ok(()) };
            let Some(o) = t.origin.as_mut() else { return Ok(()) };
            if ok {
                o.fetched.push(a.url.clone());
            } else {
                o.not_fetched.push(a.clone());
                t.history.push(tickets::Step { at: now_ms(), kind: "file".into(), text: "An attachment of its issue was not fetched: its link is kept".into() });
            }
            self.tickets_save(&next)?;
            *b = next;
            Ok::<_, String>(())
        });
        if let Err(e) = got {
            // The reason's kind only: an error may quote a name.
            let why = if e.len() <= 20 && e.chars().all(|c| c.is_ascii_lowercase() || c == '_') { e } else { "refused".into() };
            tracing::info!("tracker sync of {s}: an attachment of {} not fetched ({why})", tickets::num(tn));
        }
    }

    /// Sends the due write-backs, in order; the first failure stops the
    /// queue (order kept) and is retried with backoff.
    async fn tracker_drain(&self, sb: &str, src: &Source, n: &sbxw_core::SandboxName) {
        let mut sent = 0;
        loop {
            let now = now_ms();
            let Some(w) = self.tracker_with(sb, |x| x.queue.first().cloned()) else { break };
            if w.next_ms > now {
                break;
            }
            match self.tracker_send(sb, src, n, &w).await {
                Ok(()) => {
                    let _ = self.tracker_update(sb, |x| {
                        if x.queue.first().is_some_and(|h| h.n == w.n && h.key == w.key && h.queued_ms == w.queued_ms) {
                            x.queue.remove(0);
                        }
                        if x.status.state == SyncState::Failed {
                            x.status = Status { state: SyncState::Ok, last_ok_ms: Some(now_ms()), via: x.status.via.clone(), ops: x.status.ops.clone(), probed_ms: x.status.probed_ms, ..Default::default() };
                        }
                    });
                    sent += 1;
                }
                Err(f) => {
                    let _ = self.tracker_update(sb, |x| bump(x, &f, now));
                    self.tracker_failed(sb, src, &f);
                    break;
                }
            }
        }
        if sent > 0 {
            tracing::info!("tracker sync of {sb}: {sent} write-back(s) sent");
            self.tickets_changed(sb, None, "synced");
        }
    }

    /// One write-back's steps, each recorded once done.
    async fn tracker_send(&self, sb: &str, src: &Source, n: &sbxw_core::SandboxName, w: &WriteBack) -> Result<(), Failure> {
        let t = src.tracker;
        let done = |step: &str| w.done.iter().any(|d| d == step);
        let mark = |step: &'static str| {
            let _ = self.tracker_update(sb, |x| {
                if let Some(h) = x.queue.first_mut().filter(|h| h.n == w.n && h.queued_ms == w.queued_ms) {
                    h.done.push(step.into());
                }
            });
        };
        if !done("comment") {
            let id = self
                .tickets_with(sb, |b| b.by_key(&w.key).and_then(|t| t.origin.as_ref()).and_then(|o| o.comment_id.clone()))
                .or_else(|| self.tracker_with(sb, |x| x.comments.get(&w.key).cloned()));
            let v = self.sync_script(n, &tracker::comment(t, &w.key, id.as_deref(), &w.comment), WRITE_BOUND).await?;
            if let Some(id) = v.get("comment_id").and_then(tracker::comment_id) {
                let _ = self.tracker_update(sb, |x| x.comments.insert(w.key.clone(), id.clone()));
                let _ = self.tickets_with(sb, |b| {
                    let mut next = b.clone();
                    if let Some(o) = next.tickets.iter_mut().filter_map(|t| t.origin.as_mut()).find(|o| o.key == w.key) {
                        o.comment_id = Some(id.clone());
                    }
                    self.tickets_save(&next)?;
                    *b = next;
                    Ok::<_, String>(())
                });
            }
            mark("comment");
        }
        if !done("label") {
            self.sync_script(n, &tracker::label(t, &w.key, &w.label), WRITE_BOUND).await?;
            mark("label");
        }
        if w.close && !done("close") {
            self.sync_script(n, &tracker::close(t, &w.key), WRITE_BOUND).await?;
            mark("close");
        }
        Ok(())
    }

    fn tracker_poll_failed(&self, sb: &str, src: &Source, f: &Failure) {
        let now = now_ms();
        let _ = self.tracker_update(sb, |x| x.last_poll_ms = now);
        self.tracker_failed(sb, src, f);
    }

    /// A failure in the source's status, with what to do; one Board warning
    /// per kind until the next success.
    fn tracker_failed(&self, sb: &str, src: &Source, f: &Failure) {
        let now = now_ms();
        let until = f.retry_after_s.map(|s| now + s * 1000);
        let when = f.retry_after_s.map(|s| format!("{} minute(s)", s.div_ceil(60).max(1)));
        let hint = f.kind.hint(src.tracker, when.as_deref());
        let warn = self
            .tracker_update(sb, |x| {
                x.status.state = SyncState::Failed;
                x.status.error = Some(f.kind);
                x.status.hint = Some(hint.clone());
                x.status.retry_at_ms = until;
                let first = !x.status.warned.contains(&f.kind);
                if first {
                    x.status.warned.push(f.kind);
                }
                first
            })
            .unwrap_or(true);
        // The kind only: the message may quote what the tracker said.
        tracing::warn!("tracker sync of {sb}: failed ({})", f.kind.id());
        if warn {
            if let Err(e) = self.board_note(sb, None, "sbxw", sbxw_core::board::Kind::Warning, &f.kind.warning(src.tracker, &src.repo), None, &[]) {
                tracing::warn!("tracker sync of {sb}: could not post the warning on its board: {e}");
            }
        }
        self.tickets_changed(sb, None, "source");
    }
}

/// The queue's head failed: tried again after a backoff (or the tracker's
/// rate limit), never dropped.
fn bump(x: &mut SyncFile, f: &Failure, now: u64) {
    if let Some(h) = x.queue.first_mut() {
        h.attempts += 1;
        h.error = Some(f.kind);
        let wait = tracker::backoff_ms(h.attempts - 1).max(f.retry_after_s.unwrap_or(0) * 1000);
        h.next_ms = now + wait;
    }
}

#[cfg(test)]
mod tests;
