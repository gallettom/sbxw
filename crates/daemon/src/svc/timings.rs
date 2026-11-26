//! Where the time of a bring-up goes: one timeline per bring-up, every step
//! timed, every `sbx` call timed, and what follows `Done` (the attach, the
//! agent's first hook events, the kits' startups in the guest) added to the
//! same timeline as it happens.
//!
//! Measures only: nothing here changes what a bring-up does or waits for.
//! The kit-startup watch runs in the background, after `Done`, with a bound.
//!
//! * A timeline holds names, durations and counts: the sandbox's name, its
//!   host, the template's tag, the number of kits, the sbx version. Never a
//!   workspace path, an environment value or a token; an `sbx` call is kept
//!   by subcommand only (`sbxw_sbx::timing`).
//! * The last [`KEEP`] live in `state/timings.json` (atomic write, lenient
//!   load: an entry that does not read is dropped, not the file).
//! * Each finished bring-up writes one info line to the daemon log, and its
//!   progress stream ends with the same summary (`Progress::Done`).
//!
//! `docs/timings.md` says how to read one.

use crate::app::App;
use sbxw_core::SandboxRef;
use sbxw_sbx::timing::{Call, Recorder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Timelines kept.
pub const KEEP: usize = 50;
/// Steps shorter than this are left out of the summary line (kept in the
/// timeline).
const SHOWN_FROM_MS: u64 = 200;
/// How long after `Done` the attach and the agent's first events still count
/// for a bring-up.
const LATE_FOR: Duration = Duration::from_secs(30 * 60);
/// The kit-startup watch: one look this often, given up after this long.
/// Rare on purpose: each look is two calls, an `sbx ls` (is it still
/// running?) then an `sbx exec` (its process list).
pub(crate) const WATCH_POLL: Duration = if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(10) };
pub(crate) const WATCH_FOR: Duration = if cfg!(test) { Duration::from_millis(500) } else { Duration::from_secs(20 * 60) };
/// For a stopped sandbox, the watch starts when its agent's session starts
/// (the sandbox runs then): waited for this long at most.
const SESSION_WAIT: Duration = if cfg!(test) { Duration::from_millis(500) } else { Duration::from_secs(180) };

/// Stable step names, in the order a bring-up takes them.
pub mod step {
    pub const REMOTE_LINK: &str = "remote_link";
    pub const GIT_CLONE: &str = "git_clone";
    pub const REMOTE_CONFIG: &str = "remote_config";
    pub const PREPARE: &str = "prepare";
    pub const SBX_LS: &str = "sbx_ls";
    pub const TEMPLATE_CHECK: &str = "template_check";
    pub const SETTING_REMOTE: &str = "setting_remote_control";
    pub const RECORD: &str = "record";
    pub const SETTING_CHROME: &str = "setting_chrome";
    pub const CREATE_PREPARE: &str = "create_prepare";
    pub const SBX_CREATE: &str = "sbx_create";
    pub const RUN_DETACHED: &str = "run_detached";
    pub const BOOT: &str = "boot";
    pub const TOOLING: &str = "tooling";
    pub const POLICY_ALLOW: &str = "policy_allow";
    pub const POLICY_DENY: &str = "policy_deny";
    pub const KITS_ADD: &str = "kits_add";
    pub const SECRET: &str = "secret";
    pub const ALIASES: &str = "aliases";
    pub const TEMPLATE_SAVE: &str = "template_save";
    pub const DONE: &str = "done";
    // After `Done`.
    pub const ATTACH: &str = "attach";
    pub const ATTACH_LS: &str = "attach_ls";
    pub const ATTACH_SETTING: &str = "attach_setting";
    pub const AGENT_SESSION_START: &str = "agent_session_start";
    pub const AGENT_IDLE: &str = "agent_idle";
    pub const KIT_STARTUPS: &str = "kit_startups";
}

/// What the summary calls a step.
fn label(name: &str) -> String {
    match name {
        step::SBX_CREATE => "create".into(),
        step::SBX_LS => "sbx ls".into(),
        step::SETTING_REMOTE => "setting (Remote Control)".into(),
        step::SETTING_CHROME => "setting (Chrome)".into(),
        step::RUN_DETACHED => "run -d".into(),
        other => other.replace('_', " "),
    }
}

/// One step: when it started, how long it took (milliseconds after the
/// bring-up's start). `ms` is 0 for a moment (`done`, the agent's events).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub name: String,
    pub start_ms: u64,
    /// `None` while it runs.
    pub ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Still running when the bring-up failed: `ms` is up to the failure.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unfinished: bool,
}

/// One bring-up, from the request to the agent's prompt.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Unique per daemon state: the start, in Unix milliseconds, made unique.
    pub id: u64,
    pub sandbox: String,
    /// `local`, or the remote host's name.
    pub host: String,
    /// Unix seconds.
    pub started_at: u64,
    /// `new`, `stopped` (started at attach), `running`, or empty when the
    /// bring-up failed before it looked.
    pub kind: String,
    #[serde(default)]
    pub template: Option<String>,
    /// The project's kits, sbxw's base kit included.
    #[serde(default)]
    pub kits: usize,
    #[serde(default)]
    pub sbx_version: Option<String>,
    #[serde(default)]
    pub steps: Vec<Span>,
    /// Every `sbx` call the bring-up made itself, by subcommand.
    #[serde(default)]
    pub calls: Vec<Call>,
    #[serde(default)]
    pub error: Option<String>,
    /// Milliseconds from the request to `Done`.
    #[serde(default)]
    pub ready_ms: Option<u64>,
    /// From the request to the agent idle at its prompt.
    #[serde(default)]
    pub agent_ms: Option<u64>,
    /// From the request to the last kit startup command's end.
    #[serde(default)]
    pub startups_ms: Option<u64>,
    #[serde(default)]
    pub summary: String,
}

impl Timeline {
    fn has(&self, name: &str) -> bool {
        self.steps.iter().any(|s| s.name == name)
    }

    fn span(&self, name: &str) -> Option<&Span> {
        self.steps.iter().find(|s| s.name == name)
    }

    /// Step `name` while it runs.
    fn running(&mut self, name: &str) -> Option<&mut Span> {
        self.steps.iter_mut().find(|s| s.name == name && s.ms.is_none())
    }
}

/// A duration as a person reads it: `350 ms`, `4.2 s`, `41 s`, `3 min 5 s`.
pub fn human(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms} ms"),
        1000..10_000 => format!("{:.1} s", ms as f64 / 1000.0),
        10_000..120_000 => format!("{} s", (ms + 500) / 1000),
        _ => {
            let s = (ms + 500) / 1000;
            format!("{} min {} s", s / 60, s % 60)
        }
    }
}

/// The bring-up's line: its total, then its steps of [`SHOWN_FROM_MS`] or
/// more, in order. `ready in 41 s: create 28 s, boot 6.0 s, tooling 3.1 s`.
pub fn summary(t: &Timeline) -> String {
    let total = t.ready_ms.unwrap_or(0);
    let head = match &t.error {
        None => format!("ready in {}", human(total)),
        Some(_) => format!("failed after {}", human(total)),
    };
    let parts: Vec<String> = t
        .steps
        .iter()
        .filter(|s| s.name != step::DONE)
        .filter_map(|s| s.ms.filter(|ms| *ms >= SHOWN_FROM_MS).map(|ms| (s, ms)))
        .map(|(s, ms)| format!("{} {}{}", label(&s.name), human(ms), if s.unfinished { " (unfinished)" } else { "" }))
        .collect();
    match parts.is_empty() {
        true => head,
        false => format!("{head}: {}", parts.join(", ")),
    }
}

/// What follows `Done`, once it is all there: the line a person reads.
pub fn after_summary(t: &Timeline) -> Option<String> {
    let agent = t.agent_ms?;
    let mut parts = vec![format!("bring-up {}", human(t.ready_ms.unwrap_or(0)))];
    if let Some(a) = t.span(step::ATTACH).and_then(|s| s.ms) {
        parts.push(format!("attach {}", human(a)));
    }
    if let (Some(start), Some(attach)) = (t.span(step::AGENT_SESSION_START), t.span(step::ATTACH)) {
        let attached = attach.start_ms + attach.ms.unwrap_or(0);
        parts.push(format!("agent start {}", human(start.start_ms.saturating_sub(attached))));
    }
    Some(format!("agent ready {} after the request ({})", human(agent), parts.join(", ")))
}

/// A bring-up's timeline while it runs. Clones share it.
#[derive(Clone)]
pub struct Clock {
    start: Instant,
    rec: Recorder,
    t: Arc<Mutex<Timeline>>,
    watch: Arc<Mutex<Option<Vec<String>>>>,
}

impl Clock {
    pub fn new(sb: &SandboxRef) -> Self {
        let start = Instant::now();
        let t = Timeline {
            sandbox: sb.name.to_string(),
            host: sb.host.to_string(),
            started_at: unix_now_ms() / 1000,
            ..Default::default()
        };
        Self { start, rec: Recorder::new(start), t: Arc::new(Mutex::new(t)), watch: Arc::default() }
    }

    pub fn recorder(&self) -> &Recorder {
        &self.rec
    }

    pub fn start(&self) -> Instant {
        self.start
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Start step `name`; the index ends it.
    pub fn begin(&self, name: &str) -> usize {
        let at = self.now_ms();
        let mut t = self.t.lock().unwrap();
        t.steps.push(Span { name: name.into(), start_ms: at, ms: None, detail: None, unfinished: false });
        t.steps.len() - 1
    }

    pub fn end(&self, i: usize) {
        self.end_with(i, None::<String>);
    }

    pub fn end_with(&self, i: usize, detail: Option<impl Into<String>>) {
        let now = self.now_ms();
        let mut t = self.t.lock().unwrap();
        if let Some(s) = t.steps.get_mut(i) {
            s.ms = Some(now.saturating_sub(s.start_ms));
            if let Some(d) = detail {
                s.detail = Some(d.into());
            }
        }
    }

    /// Time `f` as step `name`.
    pub async fn time<F: Future>(&self, name: &str, f: F) -> F::Output {
        let i = self.begin(name);
        let out = f.await;
        self.end(i);
        out
    }

    pub fn set(&self, f: impl FnOnce(&mut Timeline)) {
        f(&mut self.t.lock().unwrap());
    }

    /// After the bring-up, watch for the kits' startups at `resolved`
    /// (local kit directories, never kept in the timeline).
    pub fn watch_after(&self, resolved: Vec<String>) {
        *self.watch.lock().unwrap() = Some(resolved);
    }

    pub fn kind(&self) -> String {
        self.t.lock().unwrap().kind.clone()
    }

    pub fn take_watch(&self) -> Option<Vec<String>> {
        self.watch.lock().unwrap().take()
    }

    /// The bring-up is over: `done` noted, unfinished steps closed at now,
    /// the calls copied in, the summary written. A failure is recorded as
    /// the step it stopped in, never as its message (which may quote a path
    /// or what sbx printed).
    pub fn finish(&self, failed: bool) -> Timeline {
        let now = self.now_ms();
        let mut t = self.t.lock().unwrap();
        let mut stopped_in = None;
        for s in t.steps.iter_mut().filter(|s| s.ms.is_none()) {
            s.ms = Some(now.saturating_sub(s.start_ms));
            s.unfinished = true;
            stopped_in = Some(s.name.clone());
        }
        let error = failed.then(|| match stopped_in {
            Some(step) => format!("failed during {}", label(&step)),
            None => "failed".to_string(),
        });
        t.steps.push(Span { name: step::DONE.into(), start_ms: now, ms: Some(0), detail: None, unfinished: false });
        t.calls = self.rec.calls();
        t.ready_ms = Some(now);
        t.error = error;
        t.summary = summary(&t);
        t.clone()
    }
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The kept timelines, newest last, and the bring-ups whose aftermath is
/// still awaited (by sandbox: its timeline and its start).
#[derive(Default)]
pub struct Book {
    list: VecDeque<Timeline>,
    live: HashMap<String, (u64, Instant)>,
    /// Kit startups to watch from the attach, by timeline: the kits as
    /// local directories (in memory only).
    at_attach: HashMap<u64, Vec<String>>,
    /// The running kit-startup watches, by sandbox: what ends one at once.
    watches: HashMap<String, Arc<tokio::sync::Notify>>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    timelines: &'a VecDeque<Timeline>,
}

impl Book {
    pub fn timelines(&self) -> Vec<Timeline> {
        self.list.iter().rev().cloned().collect()
    }

    fn push(&mut self, mut t: Timeline) -> u64 {
        let last = self.list.back().map_or(0, |l| l.id);
        t.id = t.started_at.saturating_mul(1000).max(last + 1);
        let id = t.id;
        self.list.push_back(t);
        while self.list.len() > KEEP {
            self.list.pop_front();
        }
        id
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Timeline> {
        self.list.iter_mut().find(|t| t.id == id)
    }
}

pub(crate) fn load(path: &Path) -> Book {
    let Ok(text) = std::fs::read_to_string(path) else { return Book::default() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        tracing::warn!("timings: could not read {}; starting with none", path.display());
        return Book::default();
    };
    let list: VecDeque<Timeline> = v["timelines"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| serde_json::from_value(t.clone()).ok()).collect())
        .unwrap_or_default();
    let skip = list.len().saturating_sub(KEEP);
    Book { list: list.into_iter().skip(skip).collect(), ..Default::default() }
}

/// The kit-startup watch found the sandbox not running: it ends there,
/// without an `sbx exec`, which would start it again.
#[derive(Debug)]
pub struct NotRunning;

impl std::fmt::Display for NotRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the sandbox stopped before the startups ended")
    }
}

impl std::error::Error for NotRunning {}

/// A step after `Done`, begun on a bring-up's timeline.
pub struct Late {
    id: u64,
    start: Instant,
    name: String,
}

impl App {
    pub(crate) fn timings_file(&self) -> std::path::PathBuf {
        self.store.paths.state().join("timings.json")
    }

    fn persist_timings(&self, book: &Book) {
        let path = self.timings_file();
        let bytes = serde_json::to_vec_pretty(&FileOut { timelines: &book.list });
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| crate::store::write_atomic(&path, &b)) {
            tracing::warn!("timings: could not save {}: {e:#}", path.display());
        }
    }

    /// The kept timelines, newest first.
    pub fn timelines(&self) -> Vec<Timeline> {
        self.timings.lock().unwrap().timelines()
    }

    /// A finished bring-up: logged, kept, and open for its aftermath.
    /// Returns its id.
    pub(crate) fn timing_finished(&self, clock: &Clock, t: Timeline) -> u64 {
        let sb = match t.host.as_str() {
            "local" | "" => t.sandbox.clone(),
            h => format!("{h}:{}", t.sandbox),
        };
        tracing::info!("bring-up of '{sb}' ({}): {}", if t.kind.is_empty() { "?" } else { &t.kind }, t.summary);
        let key = t.host.clone() + "/" + &t.sandbox;
        let ok = t.error.is_none();
        let mut book = self.timings.lock().unwrap();
        let id = book.push(t);
        if ok {
            book.live.insert(key, (id, clock.start));
        } else {
            book.live.remove(&key);
        }
        self.persist_timings(&book);
        id
    }

    /// The bring-up whose aftermath `sb`'s events still belong to.
    fn live_for(book: &mut Book, sb: &SandboxRef) -> Option<(u64, Instant)> {
        let key = format!("{}/{}", sb.host, sb.name);
        let (id, start) = *book.live.get(&key)?;
        if start.elapsed() > LATE_FOR || book.get_mut(id).is_none() {
            book.live.remove(&key);
            return None;
        }
        Some((id, start))
    }

    /// Begin step `name` on `sb`'s last bring-up, when its aftermath is
    /// still awaited and the step is not there yet (only the first counts).
    pub(crate) fn timing_begin(&self, sb: &SandboxRef, name: &str) -> Option<Late> {
        let mut book = self.timings.lock().unwrap();
        let (id, start) = Self::live_for(&mut book, sb)?;
        let t = book.get_mut(id)?;
        if t.has(name) {
            return None;
        }
        let at = start.elapsed().as_millis() as u64;
        t.steps.push(Span { name: name.into(), start_ms: at, ms: None, detail: None, unfinished: false });
        Some(Late { id, start, name: name.into() })
    }

    pub(crate) fn timing_end(&self, late: Late, detail: Option<String>) {
        let now = late.start.elapsed().as_millis() as u64;
        let mut book = self.timings.lock().unwrap();
        let Some(t) = book.get_mut(late.id) else { return };
        if let Some(s) = t.running(&late.name) {
            s.ms = Some(now.saturating_sub(s.start_ms));
            s.detail = detail;
        }
        self.persist_timings(&book);
    }

    /// A late step that never happened after all: taken out again.
    pub(crate) fn timing_drop(&self, late: Late) {
        let mut book = self.timings.lock().unwrap();
        if let Some(t) = book.get_mut(late.id) {
            t.steps.retain(|s| s.name != late.name || s.ms.is_some());
        }
    }

    /// The agent's first hook events after a bring-up: its session started,
    /// then idle at its prompt, which closes the bring-up's aftermath and
    /// writes the line a person reads.
    pub(crate) fn timing_agent_event(&self, sb: &SandboxRef, event: &str, idle: bool) {
        let mut book = self.timings.lock().unwrap();
        let Some((id, start)) = Self::live_for(&mut book, sb) else { return };
        let at = start.elapsed().as_millis() as u64;
        let Some(t) = book.get_mut(id) else { return };
        let mut changed = false;
        for (name, now) in [(step::AGENT_SESSION_START, event == "SessionStart"), (step::AGENT_IDLE, idle)] {
            if now && !t.has(name) {
                t.steps.push(Span { name: name.into(), start_ms: at, ms: Some(0), detail: None, unfinished: false });
                changed = true;
            }
        }
        if !changed {
            return;
        }
        let mut close = false;
        if idle && t.agent_ms.is_none() {
            t.agent_ms = Some(at);
            if let Some(line) = after_summary(t) {
                tracing::info!("bring-up of '{sb}': {line}");
            }
            close = true;
        }
        if close {
            book.live.remove(&format!("{}/{}", sb.host, sb.name));
        }
        self.persist_timings(&book);
    }

    /// Watch timeline `id`'s kit startups from its sandbox's attach, which
    /// starts the stopped sandbox (a look before would start it itself).
    pub(crate) fn timing_watch_later(&self, id: u64, resolved: Vec<String>) {
        self.timings.lock().unwrap().at_attach.insert(id, resolved);
    }

    /// The watch [`Self::timing_watch_later`] left for `sb`'s attach, taken.
    pub(crate) fn timing_watch_at_attach(&self, sb: &SandboxRef) -> Option<(u64, Instant, Vec<String>)> {
        let mut book = self.timings.lock().unwrap();
        let (id, start) = Self::live_for(&mut book, sb)?;
        let resolved = book.at_attach.remove(&id)?;
        Some((id, start, resolved))
    }

    /// End `sb`'s kit-startup watch now, before the daemon stops or
    /// removes it: no look may follow (it would start the sandbox again).
    pub(crate) fn timing_stop_watch(&self, sb: &SandboxRef) {
        let mut book = self.timings.lock().unwrap();
        if let Some(cancel) = book.watches.remove(&format!("{}/{}", sb.host, sb.name)) {
            cancel.notify_one();
        }
        if let Some((id, _)) = book.live.remove(&format!("{}/{}", sb.host, sb.name)) {
            book.at_attach.remove(&id);
        }
    }

    fn timing_has(&self, id: u64, name: &str) -> bool {
        self.timings.lock().unwrap().get_mut(id).is_some_and(|t| t.has(name))
    }

    /// Watch, in the background, for the last of the kits' startup commands
    /// to end in `sb`, and note when on timeline `id`. `after_session`: wait
    /// first for the agent's session to start (a stopped sandbox runs only
    /// then; a look before would start it). Each look is an `sbx ls`, then
    /// an `sbx exec` only while it runs, every [`WATCH_POLL`], for
    /// [`WATCH_FOR`] at most; their number and cost go in the step's detail.
    /// A sandbox found stopped, or stopped or removed by the daemon
    /// ([`Self::timing_stop_watch`]), ends the watch: a measurement never
    /// starts a sandbox.
    pub(crate) fn watch_kit_startups(self: &Arc<Self>, sb: SandboxRef, resolved: Vec<String>, id: u64, start: Instant, after_session: bool) {
        let (commands, marked) = self.startup_commands(&resolved);
        let app = self.clone();
        let key = format!("{}/{}", sb.host, sb.name);
        let cancel = Arc::new(tokio::sync::Notify::new());
        {
            let mut book = self.timings.lock().unwrap();
            let Some(t) = book.get_mut(id) else { return };
            if t.has(step::KIT_STARTUPS) {
                return;
            }
            t.steps.push(Span { name: step::KIT_STARTUPS.into(), start_ms: start.elapsed().as_millis() as u64, ms: None, detail: None, unfinished: false });
            book.watches.insert(key.clone(), cancel.clone());
        }
        tokio::spawn(async move {
            // Dropped mid-look when the daemon stops or removes the sandbox
            // (`timing_stop_watch`): the child `sbx` is killed with it.
            let watched = app.clone();
            // The cancel first, at every wake-up: a look due at the same
            // moment must not run once the daemon has asked for the stop.
            let done = tokio::select! {
                biased;
                _ = cancel.notified() => (Err(anyhow::Error::new(NotRunning)), "the daemon stopped or removed the sandbox before the startups ended".to_string(), start.elapsed().as_millis() as u64),
                d = watched.kit_startups_watch(sb, commands, marked, id, start, after_session) => d,
            };
            let mut book = app.timings.lock().unwrap();
            if book.watches.get(&key).is_some_and(|c| Arc::ptr_eq(c, &cancel)) {
                book.watches.remove(&key);
            }
            app.kit_startups_noted(&mut book, id, start, done);
        });
    }

    /// The watch itself: its outcome, what it says, and when it began
    /// looking (milliseconds after the request).
    async fn kit_startups_watch(self: &Arc<Self>, sb: SandboxRef, commands: Vec<String>, marked: Vec<String>, id: u64, start: Instant, after_session: bool) -> (anyhow::Result<()>, String, u64) {
        let app = self;
        if after_session {
            let deadline = Instant::now() + SESSION_WAIT;
            while !app.timing_has(id, step::AGENT_SESSION_START) && Instant::now() < deadline {
                tokio::time::sleep(WATCH_POLL.min(Duration::from_secs(1))).await;
            }
        }
        let rec = Recorder::new(Instant::now());
        let began = start.elapsed().as_millis() as u64;
        if commands.is_empty() {
            return (Ok(()), "no kit startup commands".to_string(), began);
        }
        if after_session && !app.timing_has(id, step::AGENT_SESSION_START) {
            return (Err(anyhow::anyhow!("not watched")), "not watched: the agent's session did not start".to_string(), began);
        }
        let sink: sbxw_core::progress::Sink = Arc::new(|_| {});
        let out = rec.scope(app.watch_startups(&sb, &commands, &marked, &sink)).await;
        let looks = rec.count_since("exec", 0);
        let cost: u64 = rec.calls().iter().map(|c| c.ms).sum();
        let said = match &out {
            Ok(()) => format!("{} startup command(s) ended; {looks} look(s), {} of sbx calls", commands.len(), human(cost)),
            Err(e) if e.downcast_ref::<NotRunning>().is_some() => format!("{e}; {looks} look(s), {} of sbx calls", human(cost)),
            Err(e) => format!("not seen to end ({e:#}); {looks} look(s), {} of sbx calls", human(cost)),
        };
        (out, said, began)
    }

    /// Write a watch's end on timeline `id`, and its log line (info at most).
    fn kit_startups_noted(&self, book: &mut Book, id: u64, start: Instant, (outcome, finished, began): (anyhow::Result<()>, String, u64)) {
        let now = start.elapsed().as_millis() as u64;
        let Some(t) = book.get_mut(id) else { return };
        if let Some(s) = t.running(step::KIT_STARTUPS) {
            s.start_ms = s.start_ms.max(began.min(now));
            s.ms = Some(now.saturating_sub(s.start_ms));
            s.detail = Some(finished.clone());
            s.unfinished = outcome.is_err();
        }
        let who = match t.host.as_str() {
            "local" | "" => t.sandbox.clone(),
            h => format!("{h}:{}", t.sandbox),
        };
        match outcome {
            Ok(()) => {
                t.startups_ms = Some(now);
                tracing::info!("bring-up of '{who}': kit startups finished after {} ({finished})", human(now));
            }
            Err(_) => tracing::info!("bring-up of '{who}': kit startups {finished}"),
        }
        self.persist_timings(book);
    }
}

#[cfg(test)]
mod tests;
