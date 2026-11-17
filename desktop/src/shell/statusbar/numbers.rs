//! The status bar's numbers, from the daemon's data: total CPU (GET
//! /api/stats and the `stats` stream event, plus this app's own) and the
//! sessions' cost (`cost_usd` on GET /api/sandboxes items and on each
//! SessionInfo). Pure: the bar and its popovers read these.

use crate::client::Sandbox;
use serde_json::Value as J;

/// One line of the CPU popover.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuRow {
    pub name: String,
    /// `sandbox`, `daemon` or `app`.
    pub kind: &'static str,
    /// Percent of one core; none: no recent report.
    pub cpu: Option<f64>,
    /// Memory used and the sandbox's total, bytes.
    pub mem: Option<f64>,
    pub mem_total: Option<f64>,
}

/// The CPU entry: the total (percent of one core), whether it is partial,
/// who reports, and why it is unknown when it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Cpu {
    pub total: Option<f64>,
    pub partial: bool,
    /// Sandboxes with a value, of how many.
    pub counted: usize,
    pub sandboxes: usize,
    pub daemon_known: bool,
    pub app: Option<f64>,
    /// Sorted by CPU, unknown last.
    pub rows: Vec<CpuRow>,
    /// Why there is no number at all.
    pub why: Option<String>,
}

fn num(v: &J) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite() && *x >= 0.0)
}

/// The CPU entry from the last stats (or why there are none) and this
/// app's own percentage. `total_cpu_pct` is the daemon plus every sandbox
/// that reports; the app's is added on top.
pub fn cpu(stats: Option<&J>, error: Option<&str>, app: Option<f64>) -> Cpu {
    let Some(st) = stats else {
        let why = match error {
            Some(e) => format!("The daemon gives no CPU figures: {e}"),
            None => "No CPU figures from the daemon yet".into(),
        };
        return Cpu { total: None, partial: false, counted: 0, sandboxes: 0, daemon_known: false, app, rows: vec![], why: Some(why) };
    };
    let list = st["sandboxes"].as_array().cloned().unwrap_or_default();
    let mut rows: Vec<CpuRow> = list
        .iter()
        .map(|s| CpuRow { name: s["name"].as_str().unwrap_or("?").to_string(), kind: "sandbox", cpu: num(&s["cpu_pct"]), mem: num(&s["mem_bytes"]), mem_total: num(&s["mem_total_bytes"]) })
        .collect();
    let daemon = num(&st["daemon_cpu_pct"]);
    rows.push(CpuRow { name: "sbxw daemon".into(), kind: "daemon", cpu: daemon, mem: None, mem_total: None });
    rows.push(CpuRow { name: "this app".into(), kind: "app", cpu: app, mem: None, mem_total: None });
    rows.sort_by(|a, b| match (a.cpu, b.cpu) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });
    let counted = st["sandboxes_counted"].as_u64().map_or_else(|| list.iter().filter(|s| num(&s["cpu_pct"]).is_some()).count(), |n| n as usize);
    let unknown = st["sandboxes_unknown"].as_u64().map_or_else(|| list.len() - counted.min(list.len()), |n| n as usize);
    let total = num(&st["total_cpu_pct"]).map(|t| t + app.unwrap_or(0.0));
    let why = total.is_none().then(|| "Nothing reports CPU right now: no sandbox has a recent report and the daemon gives none".to_string());
    Cpu { total, partial: unknown > 0 || daemon.is_none(), counted, sandboxes: counted + unknown, daemon_known: daemon.is_some(), app, rows, why }
}

/// A percentage of one core: one decimal under 10, whole above.
pub fn pct(v: f64) -> String {
    if v < 9.95 {
        format!("{v:.1}%")
    } else {
        format!("{v:.0}%")
    }
}

/// The entry's text: "CPU 142%", "142%" without the word, "+" after it
/// when partial, "–" when unknown.
pub fn cpu_text(c: &Cpu, word: bool) -> String {
    let n = match c.total {
        Some(t) => format!("{}{}", pct(t), if c.partial { "+" } else { "" }),
        None => "–".into(),
    };
    if word {
        format!("CPU {n}")
    } else {
        n
    }
}

/// The entry's tooltip: what it sums, and what is missing.
pub fn cpu_help(c: &Cpu) -> String {
    let mut t = "Total CPU, in percent of one core (4 busy cores read 400%): the sandboxes' agents, the sbxw daemon and this app.".to_string();
    if let Some(w) = &c.why {
        t.push_str(&format!("\n{w}."));
    } else if c.partial {
        if c.counted < c.sandboxes {
            t.push_str(&format!("\n{} of {} sandboxes report; the others have no recent report, so the total is at least this.", c.counted, c.sandboxes));
        }
        if !c.daemon_known {
            t.push_str("\nThe daemon gives no figure for itself.");
        }
    }
    if c.app.is_none() {
        t.push_str("\nThis app's own share is not measured on this system.");
    }
    t.push_str("\nClick for each sandbox.");
    t
}

/// Bytes as GB (one decimal) or MB.
pub fn bytes(b: f64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    if b >= G {
        format!("{:.1} GB", b / G)
    } else {
        format!("{:.0} MB", b / (1024.0 * 1024.0))
    }
}

/// Memory as "used / total" when the total is known.
pub fn mem_text(r: &CpuRow) -> String {
    match (r.mem, r.mem_total) {
        (Some(u), Some(t)) => format!("{} / {}", bytes(u), bytes(t)),
        (Some(u), None) => bytes(u),
        _ => String::new(),
    }
}

/// Above this share of every core, for [`HOT_FOR`], the number turns amber.
pub const HOT_SHARE: f64 = 0.9;
pub const HOT_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a total is hot on a machine with `cores` cores.
pub fn hot(total: Option<f64>, cores: usize) -> bool {
    total.is_some_and(|t| t >= HOT_SHARE * cores.max(1) as f64 * 100.0)
}

/// One session line of the cost popover.
#[derive(Clone, Debug, PartialEq)]
pub struct CostSession {
    /// "main" or the thread's name.
    pub name: String,
    pub model: Option<String>,
    pub cost: Option<f64>,
}

/// One sandbox of the cost popover.
#[derive(Clone, Debug, PartialEq)]
pub struct CostSandbox {
    pub name: String,
    pub cost: Option<f64>,
    pub sessions: Vec<CostSession>,
    /// What the sandbox counts beyond its listed sessions (sessions that ended).
    pub ended: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cost {
    /// The sum of the sandboxes' `cost_usd`; none when no sandbox reports one.
    pub total: Option<f64>,
    /// Sandboxes with a cost or a session, most expensive first.
    pub sandboxes: Vec<CostSandbox>,
    /// Listed sessions with no cost reported.
    pub unknown: usize,
}

/// "opus 5.5" from "Opus 5.5" or "claude-opus-5-5" (as the sandbox list
/// says it, screens/sandboxes.luau `short_model`).
pub fn short_model(name: Option<&str>, id: Option<&str>) -> Option<String> {
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        return Some(n.to_lowercase());
    }
    let m = id.filter(|i| !i.is_empty())?.to_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m).to_string();
    let m = m.split('[').next().unwrap_or(&m).to_string();
    let Some(rest) = m.strip_prefix("claude-") else { return Some(m) };
    let mut family = None;
    let mut nums = vec![];
    for part in rest.split('-') {
        if part.len() == 8 && part.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) {
            nums.push(part.to_string());
        } else if family.is_none() {
            family = Some(part.to_string());
        }
    }
    Some(match family {
        Some(f) if !nums.is_empty() => format!("{f} {}", nums.join(".")),
        Some(f) => f,
        None => m,
    })
}

/// The cost entry: the sandboxes' `cost_usd` summed (a sandbox's figure
/// already holds every session it ran, ended ones included), and the live
/// sessions under each from GET /api/sessions.
pub fn cost(sandboxes: &[Sandbox], sessions: Option<&J>) -> Cost {
    let live: Vec<&J> = sessions.and_then(J::as_array).map(|a| a.iter().filter(|s| s["state"] != "exited").collect()).unwrap_or_default();
    let mut out = vec![];
    let mut unknown = 0;
    let mut total: Option<f64> = None;
    for sb in sandboxes {
        let cost = sb.extra.get("cost_usd").and_then(num);
        let mut list: Vec<CostSession> = live
            .iter()
            .filter(|s| s["sandbox"] == sb.name.as_str())
            .map(|s| CostSession {
                name: crate::feat::threads::model::thread_of(s).unwrap_or_else(|| "main".into()),
                model: short_model(s["model_name"].as_str(), s["model"].as_str()),
                cost: num(&s["cost_usd"]),
            })
            .collect();
        if cost.is_none() && list.is_empty() {
            continue;
        }
        list.sort_by(|a, b| (a.name != "main", &a.name).cmp(&(b.name != "main", &b.name)));
        unknown += list.iter().filter(|s| s.cost.is_none()).count();
        let listed: f64 = list.iter().filter_map(|s| s.cost).sum();
        let ended = cost.map(|c| c - listed).filter(|d| *d >= 0.005);
        if let Some(c) = cost {
            total = Some(total.unwrap_or(0.0) + c);
        }
        out.push(CostSandbox { name: sb.name.clone(), cost, sessions: list, ended });
    }
    out.sort_by(|a, b| b.cost.unwrap_or(-1.0).total_cmp(&a.cost.unwrap_or(-1.0)).then(a.name.cmp(&b.name)));
    Cost { total, sandboxes: out, unknown }
}

/// Dollars: two decimals, whole dollars from 1000.
pub fn usd(v: f64) -> String {
    if v >= 1000.0 {
        format!("${v:.0}")
    } else {
        format!("${v:.2}")
    }
}

pub fn cost_text(c: &Cost) -> String {
    c.total.map_or_else(|| "$–".into(), usd)
}

pub fn cost_help(c: &Cost) -> String {
    let mut t = "What these sessions cost, as Claude Code estimates it (not an invoice).".to_string();
    if c.total.is_none() {
        t.push_str("\nNo session has reported a cost yet.");
    } else if c.unknown > 0 {
        t.push_str(&format!("\n{} session{} not reported a cost yet and {} left out.", c.unknown, if c.unknown == 1 { " has" } else { "s have" }, if c.unknown == 1 { "is" } else { "are" }));
    }
    t.push_str("\nClick for each sandbox and session.");
    t
}

/// This process's CPU time so far, in seconds (`getrusage`, user plus
/// system, every thread). None where it is not measured (Windows).
#[cfg(unix)]
pub fn own_cpu_seconds() -> Option<f64> {
    // `struct rusage`: two `timeval`s then fourteen longs, 144 bytes on
    // 64-bit Linux and macOS. macOS's `tv_usec` is 32-bit and padded.
    #[repr(C)]
    struct Rusage([i64; 18]);
    extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    let mut r = Rusage([0; 18]);
    // SAFETY: `r` is as large and aligned as `struct rusage`; RUSAGE_SELF is 0.
    if unsafe { getrusage(0, &mut r) } != 0 {
        return None;
    }
    let usec = |v: i64| if cfg!(target_os = "macos") { (v as u64 & 0xffff_ffff) as i64 } else { v } as f64;
    Some(r.0[0] as f64 + usec(r.0[1]) / 1e6 + r.0[2] as f64 + usec(r.0[3]) / 1e6)
}
#[cfg(not(unix))]
pub fn own_cpu_seconds() -> Option<f64> {
    None
}

/// This app's CPU between two samples, percent of one core.
#[derive(Default)]
pub struct AppCpu {
    last: Option<(std::time::Instant, f64)>,
    pub pct: Option<f64>,
}

impl AppCpu {
    pub fn sample(&mut self, now: std::time::Instant) {
        let Some(t) = own_cpu_seconds() else { return };
        if let Some((t0, c0)) = self.last {
            let dt = now.saturating_duration_since(t0).as_secs_f64();
            if dt < 0.5 {
                return;
            }
            self.pct = Some(((t - c0) / dt * 100.0).max(0.0));
        }
        self.last = Some((now, t));
    }
}
