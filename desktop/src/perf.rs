//! Timing for finding what makes the app slow, behind `SBXW_DESKTOP_PERF`.
//! Off (the default), every probe is one load of a cached flag: no clock is
//! read, nothing is logged, nothing is counted.
//!
//! * `SBXW_DESKTOP_PERF=1`: each probe writes one line to the app log
//!   (`perf: <what> <ms> ms <details>`), so a run's log says where an input's
//!   time went: the intent, the data handed to the screens, each Luau render,
//!   the core's view (the element tree), and Rune's own phases (set
//!   `RUNE_PERF=1` with a local collector for those, see PERF.md). The
//!   counters below are kept too.
//! * `SBXW_DESKTOP_PERF=counts`: the counters alone, without a line per
//!   event (a line per push at 60 Hz would weigh in what it measures).
//!
//! The counters ([`C`], [`add`], [`keyed`]) say how often the work the CPU
//! audit of 2026-11-25 names happens: polls, views, data pushes and their
//! bytes, full builds of the screen data, Luau renders by screen, terminal
//! rows built for shown and hidden panes, the cursor's blinks, the watcher
//! threads' wake-ups, daemon reads by key. Once a second a thread logs what
//! moved (`perf: counts t=<s> cpu_ms=<n> polls=<n> ...`, one line), which
//! `scripts/measure.sh` reads by scenario.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::Instant;

/// 0: not read yet; 1: off; 2: lines and counters; 3: counters alone.
static STATE: AtomicU8 = AtomicU8::new(0);

#[inline]
fn state() -> u8 {
    match STATE.load(Ordering::Relaxed) {
        0 => {
            let s = match std::env::var("SBXW_DESKTOP_PERF").as_deref() {
                Ok("1") => 2,
                Ok("counts") => 3,
                _ => 1,
            };
            // A test may have set it meanwhile (`set_for_test`): its value wins.
            match STATE.compare_exchange(0, s, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => s,
                Err(now) => now,
            }
        }
        s => s,
    }
}

/// Whether timing lines are on (read once).
#[inline]
pub fn on() -> bool {
    state() == 2
}

/// Whether the counters are on (`SBXW_DESKTOP_PERF=1` or `counts`).
#[inline]
pub fn counting() -> bool {
    state() >= 2
}

/// Sets the switch for a test: `None` off, `Some(false)` counters alone,
/// `Some(true)` lines and counters. Tests that change it hold [`test_lock`].
#[cfg(test)]
pub fn set_for_test(mode: Option<bool>) {
    STATE.store(
        match mode {
            None => 1,
            Some(true) => 2,
            Some(false) => 3,
        },
        Ordering::Relaxed,
    );
}

/// Held by every test that turns the switch on or reads the counters.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

/// A clock started only when timing is on.
#[inline]
pub fn start() -> Option<Instant> {
    on().then(Instant::now)
}

/// Milliseconds since `t` (0 when timing is off).
#[inline]
pub fn ms(t: Option<Instant>) -> f64 {
    t.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0)
}

/// Logs `what` with the time since `t`, when timing is on.
#[inline]
pub fn done(t: Option<Instant>, what: impl FnOnce() -> String) {
    if t.is_some() {
        log::info!("perf: {} {:.2} ms", what(), ms(t));
    }
}

/// Logs, when dropped, the time since its clock (a span with several exits).
pub struct OnDrop<F: FnOnce() -> String>(Option<(Instant, F)>);

impl<F: FnOnce() -> String> OnDrop<F> {
    pub fn new(t: Option<Instant>, what: F) -> Self {
        OnDrop(t.map(|t| (t, what)))
    }
}

impl<F: FnOnce() -> String> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some((t, f)) = self.0.take() {
            log::info!("perf: {} {:.2} ms", f(), t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

/// What is counted. Each name is the key of the per-second line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum C {
    /// `App::poll` calls: Rune calls it on every frame it draws and alone on
    /// a resting timer wake (frames themselves are Rune's, not visible here).
    Polls,
    /// Microseconds spent in `App::poll`.
    PollUs,
    /// The core's element tree built (`shell::view`).
    Views,
    ViewUs,
    /// `push_data` (each `push_data_once`).
    Pushes,
    PushUs,
    /// The bytes of the data each push handed the screens (estimated from
    /// its lengths, `mem::json_bytes`): what was copied to build it.
    PushBytes,
    /// Pushes at the end of `poll` that only a terminal's change asked for.
    PushesFromTerminals,
    /// Full builds of the screen data (`App::screen_data`), whatever called it.
    ScreenData,
    /// Of those, the ones for a feature's handler (`feat_ctx_data`).
    FeatCtx,
    /// Luau renders (also keyed by screen: `render.<name>`).
    Renders,
    RenderUs,
    /// A screen's element tree built from its last render (`screen::view`).
    PanelBuilds,
    /// Terminal rows built for panes on the current page.
    RowsShown,
    /// Terminal rows built for panes on other pages.
    RowsHidden,
    /// Polls of a pane on a hidden page that reported a change (each asks
    /// for the window's push and view).
    HiddenChanges,
    /// Messages a terminal took from its socket's channel (one UI wake each).
    TermMessages,
    /// The cursor blinked (a focused pane's change with no output).
    Blinks,
    /// Of those, while something covered the panes.
    BlinksCovered,
    /// The screens' file-watcher threads woke (one `stat` each).
    WatcherWakes,
    /// Events from the daemon's stream (also keyed: `event.<name>`).
    Events,
    /// Data keys read (also keyed: `fetch.<key>`), and the requests they made.
    Fetches,
    FetchRequests,
}

impl C {
    pub const ALL: [C; 23] = [
        C::Polls,
        C::PollUs,
        C::Views,
        C::ViewUs,
        C::Pushes,
        C::PushUs,
        C::PushBytes,
        C::PushesFromTerminals,
        C::ScreenData,
        C::FeatCtx,
        C::Renders,
        C::RenderUs,
        C::PanelBuilds,
        C::RowsShown,
        C::RowsHidden,
        C::HiddenChanges,
        C::TermMessages,
        C::Blinks,
        C::BlinksCovered,
        C::WatcherWakes,
        C::Events,
        C::Fetches,
        C::FetchRequests,
    ];

    /// The key in the per-second line.
    pub fn name(self) -> &'static str {
        match self {
            C::Polls => "polls",
            C::PollUs => "poll_us",
            C::Views => "views",
            C::ViewUs => "view_us",
            C::Pushes => "pushes",
            C::PushUs => "push_us",
            C::PushBytes => "push_bytes",
            C::PushesFromTerminals => "pushes_from_terminals",
            C::ScreenData => "screen_data",
            C::FeatCtx => "feat_ctx",
            C::Renders => "renders",
            C::RenderUs => "render_us",
            C::PanelBuilds => "panel_builds",
            C::RowsShown => "rows_shown",
            C::RowsHidden => "rows_hidden",
            C::HiddenChanges => "hidden_changes",
            C::TermMessages => "term_messages",
            C::Blinks => "blinks",
            C::BlinksCovered => "blinks_covered",
            C::WatcherWakes => "watcher_wakes",
            C::Events => "events",
            C::Fetches => "fetches",
            C::FetchRequests => "fetch_requests",
        }
    }
}

const N: usize = C::ALL.len();
static COUNTS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
/// Keyed counts (`render.<screen>`, `event.<name>`, `fetch.<key>`): a lock,
/// taken only when counting.
static KEYED: std::sync::Mutex<std::collections::BTreeMap<String, u64>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Adds `n` to `c` when counting (one cached load otherwise).
#[inline]
pub fn add(c: C, n: u64) {
    if counting() {
        COUNTS[c as usize].fetch_add(n, Ordering::Relaxed);
    }
}

/// Adds the microseconds since `t` to `c` (`t` from [`count_start`]).
#[inline]
pub fn add_us(c: C, t: Option<Instant>) {
    if let Some(t) = t {
        COUNTS[c as usize].fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
}

/// A clock started only when counting.
#[inline]
pub fn count_start() -> Option<Instant> {
    counting().then(Instant::now)
}

/// Adds one to `<kind>.<key>` when counting; `key` is built only then.
#[inline]
pub fn keyed(kind: &str, key: impl FnOnce() -> String) {
    if counting() {
        let k = format!("{kind}.{}", key().replace(char::is_whitespace, "_"));
        *KEYED.lock().unwrap_or_else(|e| e.into_inner()).entry(k).or_default() += 1;
    }
}

/// Every counter that moved since start (or the last [`reset`]), keyed ones
/// included, by name. Empty when nothing was counted.
pub fn snapshot() -> std::collections::BTreeMap<String, u64> {
    let mut m: std::collections::BTreeMap<String, u64> = C::ALL.iter().map(|c| (c.name().to_string(), COUNTS[*c as usize].load(Ordering::Relaxed))).filter(|(_, v)| *v > 0).collect();
    m.extend(KEYED.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), *v)));
    m
}

/// Zeroes every counter (tests).
pub fn reset() {
    for c in &COUNTS {
        c.store(0, Ordering::Relaxed);
    }
    KEYED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The per-second line: what moved since `last`, after `t` seconds, with
/// the process's CPU time over the same second (`cpu_ms`).
pub fn line(t: u64, cpu_ms: Option<f64>, now: &std::collections::BTreeMap<String, u64>, last: &std::collections::BTreeMap<String, u64>) -> String {
    let mut s = format!("perf: counts t={t} cpu_ms={}", cpu_ms.map_or("-".to_string(), |c| format!("{c:.1}")));
    for (k, v) in now {
        let d = v - last.get(k).copied().unwrap_or(0);
        if d > 0 {
            s.push_str(&format!(" {k}={d}"));
        }
    }
    s
}

/// Labels the person's script asked for (`SBXW_DESKTOP_PERF_MARKS`), for
/// the UI thread: `perf:<label>` or `mem:<label>`, as a headless script's marks.
static MARKS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The marks asked since the last call (empty unless counting).
pub fn take_marks() -> Vec<String> {
    if !counting() {
        return vec![];
    }
    std::mem::take(&mut *MARKS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Starts the thread that logs [`line`] once a second, when counting (once).
/// With `SBXW_DESKTOP_PERF_MARKS=<file>`, it also takes the labels a script
/// writes there (one per line; the file is removed once read) and wakes the
/// UI with `wake` to act on them ([`take_marks`]): how `scripts/measure.sh`
/// cuts a real run into scenarios and asks for the memory report.
pub fn start_reporter(wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !counting() || STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let marks = std::env::var_os("SBXW_DESKTOP_PERF_MARKS").map(std::path::PathBuf::from);
    log::info!("perf: counts on (once a second; frames are Rune's and not visible to the app: polls bound them from above, views from below){}", marks.as_ref().map_or(String::new(), |m| format!(", marks read from {}", m.display())));
    let spawned = std::thread::Builder::new().name("perf-counts".into()).spawn(move || {
        let start = Instant::now();
        let mut last = snapshot();
        let mut cpu = crate::shell::statusbar::numbers::own_cpu_seconds();
        let mut n = 0u64;
        loop {
            n += 1;
            let next = start + std::time::Duration::from_secs(n);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            let now = snapshot();
            let c = crate::shell::statusbar::numbers::own_cpu_seconds();
            let cpu_ms = c.zip(cpu).map(|(a, b)| (a - b) * 1000.0);
            log::info!("{}", line(n, cpu_ms, &now, &last));
            last = now;
            cpu = c;
            if let Some(path) = &marks {
                if let Ok(text) = std::fs::read_to_string(path) {
                    let _ = std::fs::remove_file(path);
                    let got: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
                    if !got.is_empty() {
                        MARKS.lock().unwrap_or_else(|e| e.into_inner()).extend(got);
                        wake();
                    }
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("perf: the counters' thread did not start: {e}");
    }
}
