//! Timing for finding what makes the app slow, behind `SBXW_DESKTOP_PERF=1`.
//! Off (the default), every probe is one load of a cached flag: no clock is
//! read, nothing is logged. On, each probe writes one line to the app log
//! (`perf: <what> <ms> ms <details>`), so a run's log says where an input's
//! time went: the intent, the data handed to the screens, each Luau render,
//! the core's view (the element tree), and Rune's own phases (set
//! `RUNE_PERF=1` with a local collector for those, see PERF.md).

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

static STATE: AtomicU8 = AtomicU8::new(0);

/// Whether timing is on (read once).
#[inline]
pub fn on() -> bool {
    match STATE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let on = std::env::var("SBXW_DESKTOP_PERF").is_ok_and(|v| v == "1");
            STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
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
