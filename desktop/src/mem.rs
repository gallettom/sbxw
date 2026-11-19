//! Memory accounting (`.sbxw-artifacts/desktop/MEMORY.md`): the process's
//! resident memory, the heap's live bytes, and what each owner holds.
//!
//! * [`Counting`] is the binary's global allocator: the system allocator,
//!   plus three counters that move only when `SBXW_DESKTOP_MEM=1` was set at
//!   start ([`start`]). Off, each allocation and free pays one relaxed load
//!   of a flag that never changes (no atomic write, no lock).
//! * [`rss`] reads the resident size: `/proc/self/status` on Linux, `ps` on
//!   macOS (what Activity Monitor's "Memory" column is closest to).
//! * The owners' sizes are computed by each owner (`TerminalState::mem`,
//!   `ScreenState::mem`, `shell/memory.rs`): structures this app holds,
//!   counted from their lengths, not guessed.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::Relaxed};

static ON: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);

/// The system allocator, counting when [`start`] turned counting on.
pub struct Counting;

#[inline]
fn grew(n: usize) {
    let now = LIVE.fetch_add(n as i64, Relaxed) + n as i64;
    PEAK.fetch_max(now, Relaxed);
    ALLOCS.fetch_add(1, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if ON.load(Relaxed) && !p.is_null() {
            grew(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(l);
        if ON.load(Relaxed) && !p.is_null() {
            grew(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        if ON.load(Relaxed) {
            LIVE.fetch_sub(l.size() as i64, Relaxed);
        }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = System.realloc(p, l, new);
        if ON.load(Relaxed) && !q.is_null() {
            let d = new as i64 - l.size() as i64;
            let now = LIVE.fetch_add(d, Relaxed) + d;
            PEAK.fetch_max(now, Relaxed);
        }
        q
    }
}

/// Turns counting on when `SBXW_DESKTOP_MEM` is set (and not `0`). Call it
/// first in `main`: what was allocated before is not counted, and its frees
/// make the count read a few bytes low.
pub fn start() {
    if std::env::var_os("SBXW_DESKTOP_MEM").is_some_and(|v| !v.is_empty() && v != "0") {
        ON.store(true, Relaxed);
    }
}

/// Counting is on.
pub fn on() -> bool {
    ON.load(Relaxed)
}

/// Bytes allocated and not freed (counting on; else `None`).
pub fn live() -> Option<u64> {
    on().then(|| LIVE.load(Relaxed).max(0) as u64)
}

/// The most bytes live at once, and allocations made (counting on).
pub fn peak() -> Option<(u64, u64)> {
    on().then(|| (PEAK.load(Relaxed).max(0) as u64, ALLOCS.load(Relaxed)))
}

/// What allocating `f`'s result costs on the heap, exactly (counting on):
/// the live bytes after minus before, the result kept until counted. Used to
/// size a clone of a structure (`J`, a `Node` tree) without estimating.
pub fn heap_of<T>(f: impl FnOnce() -> T) -> Option<u64> {
    let before = live()?;
    let v = f();
    let after = live()?;
    drop(v);
    Some(after.saturating_sub(before))
}

/// The process's resident size in bytes, and its peak where the system says.
pub fn rss() -> (Option<u64>, Option<u64>) {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let field = |name: &str| {
            s.lines().find_map(|l| l.strip_prefix(name)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok()).map(|kb| kb * 1024)
        };
        (field("VmRSS:"), field("VmHWM:"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS and the BSDs: `ps` reports the resident size in KiB.
        let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok();
        let kb = out.and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| s.trim().parse::<u64>().ok());
        (kb.map(|k| k * 1024), None)
    }
}

/// An estimate of a JSON value's heap bytes, from its lengths (for when
/// counting is off). Each value is `size_of::<J>()`; a map entry adds its
/// key and about 16 bytes of the map's own bookkeeping.
pub fn json_bytes(v: &serde_json::Value) -> u64 {
    use serde_json::Value as J;
    let one = std::mem::size_of::<J>() as u64;
    match v {
        J::String(s) => s.capacity() as u64,
        J::Array(a) => a.capacity() as u64 * one + a.iter().map(json_bytes).sum::<u64>(),
        J::Object(o) => o.iter().map(|(k, v)| k.capacity() as u64 + 24 + one + 16 + json_bytes(v)).sum(),
        _ => 0,
    }
}

/// Bytes as megabytes with one decimal.
pub fn mb(b: u64) -> String {
    format!("{:.1}", b as f64 / 1048576.0)
}
