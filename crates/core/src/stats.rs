//! CPU and memory: each sandbox's, as it reports them, and the daemon's own.
//!
//! sbx exposes no CPU figure and its sandboxes are microVMs the host's Docker
//! does not list, so each sandbox measures itself: its status line script
//! (`assets/usage-statusline.js`) sends the raw first `cpu` line of the VM's
//! `/proc/stat`, its core count and two lines of `/proc/meminfo` with its
//! throttled report (about every 10 s while a Claude session renders). The
//! daemon turns two successive samples of the same sandbox into a percentage.
//! Nothing is sampled for that: a sandbox with no Claude session rendering
//! has no value, and says so (`null`), never `0`.
//!
//! Percentages are of **one core**: 4 busy cores read 400.

use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// A sample older than this is no longer the sandbox's CPU: `cpu_pct` is
/// `null` (the status line reports every ~10 s while an agent works).
pub const STALE_MS: u64 = 30_000;
/// Two samples further apart than this give no percentage: an average over
/// minutes is not what the sandbox is doing now.
pub const MAX_WINDOW_MS: u64 = 60_000;

/// The first line of `/proc/stat`, in jiffies over every CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

/// `cpu  user nice system idle iowait irq softirq steal guest guest_nice`.
/// Busy is everything but `idle` and `iowait`; `guest` is already in `user`,
/// so the total stops at `steal`. `None` for anything else.
pub fn parse_proc_stat_cpu(line: &str) -> Option<CpuTimes> {
    let mut f = line.split_whitespace();
    if f.next()? != "cpu" {
        return None;
    }
    let n: Vec<u64> = f.take(8).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    if n.len() < 4 {
        return None;
    }
    let total: u64 = n.iter().sum();
    let idle = n[3] + n.get(4).copied().unwrap_or(0);
    Some(CpuTimes { busy: total - idle, total })
}

/// `MemTotal` and `MemAvailable` from `/proc/meminfo` lines, in bytes.
pub fn parse_meminfo(text: &str) -> (Option<u64>, Option<u64>) {
    let field = |name: &str| {
        text.lines().find_map(|l| {
            let rest = l.strip_prefix(name)?.strip_prefix(':')?;
            let mut it = rest.split_whitespace();
            let v: u64 = it.next()?.parse().ok()?;
            match it.next() {
                Some("kB") | None => v.checked_mul(1024),
                _ => None,
            }
        })
    };
    (field("MemTotal"), field("MemAvailable"))
}

/// Percent of one core between two samples: the busy share of every CPU's
/// time, times the cores. `None` when the counters went backwards (the VM
/// restarted) or did not move.
pub fn pct_of_one_core(prev: CpuTimes, cur: CpuTimes, cores: u32) -> Option<f64> {
    let total = cur.total.checked_sub(prev.total).filter(|t| *t > 0)?;
    let busy = cur.busy.checked_sub(prev.busy)?.min(total);
    Some(round1(busy as f64 / total as f64 * cores.max(1) as f64 * 100.0))
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[derive(Debug, Clone)]
struct SandboxSample {
    times: Option<CpuTimes>,
    at_ms: u64,
    /// From the two latest samples, and when the latest came.
    pct: Option<f64>,
    cores: Option<u32>,
    mem_total: Option<u64>,
    mem_used: Option<u64>,
}

/// What every sandbox last said about itself, and which ones run.
#[derive(Debug, Default)]
pub struct StatsBook {
    samples: BTreeMap<String, SandboxSample>,
    /// The running sandboxes of the daemon's last good `sbx ls`.
    running: Vec<String>,
}

/// One sandbox in `/api/stats`. Every field but the name is `null` when
/// unknown.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SandboxStats {
    pub name: String,
    pub cpu_pct: Option<f64>,
    /// Seconds since its last sample; `null` when it never sent one.
    pub cpu_age_s: Option<u64>,
    pub cpu_cores: Option<u32>,
    pub mem_bytes: Option<u64>,
    pub mem_total_bytes: Option<u64>,
}

/// The body of `GET /api/stats` and of the `stats` event.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Stats {
    pub daemon_cpu_pct: Option<f64>,
    pub sandboxes: Vec<SandboxStats>,
    /// The daemon plus every sandbox with a `cpu_pct`; `null` only when none
    /// of them is known. Partial when `sandboxes_unknown > 0` or
    /// `daemon_cpu_pct` is `null`.
    pub total_cpu_pct: Option<f64>,
    pub sandboxes_counted: usize,
    pub sandboxes_unknown: usize,
    /// Unix milliseconds.
    pub ts: u64,
}

impl StatsBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// One report from a sandbox's status line: the raw `/proc/stat` cpu
    /// line, the core count and `/proc/meminfo`'s lines, each absent from an
    /// older script (then nothing changes).
    pub fn report(&mut self, sandbox: &str, proc_stat_cpu: Option<&str>, cores: Option<u32>, meminfo: Option<&str>, now: u64) {
        let times = proc_stat_cpu.and_then(parse_proc_stat_cpu);
        let (mem_total, mem_avail) = meminfo.map(parse_meminfo).unwrap_or_default();
        if times.is_none() && mem_total.is_none() {
            return;
        }
        let cores = cores.filter(|c| (1..=1024).contains(c));
        let prev = self.samples.get(sandbox);
        let pct = match (prev.and_then(|p| p.times.map(|t| (t, p.at_ms))), times, cores) {
            (Some((p, at)), Some(c), Some(n)) if now.saturating_sub(at) <= MAX_WINDOW_MS => pct_of_one_core(p, c, n),
            _ => None,
        };
        self.samples.insert(
            sandbox.to_string(),
            SandboxSample {
                times,
                at_ms: now,
                pct,
                cores,
                mem_total,
                mem_used: mem_total.zip(mem_avail).map(|(t, a)| t.saturating_sub(a)),
            },
        );
    }

    /// The running sandboxes of a good `sbx ls`: the others are dropped. An
    /// empty list is a failed `sbx ls`, never "all gone": nothing changes.
    pub fn set_running(&mut self, running: &[String]) {
        if running.is_empty() {
            return;
        }
        self.samples.retain(|sb, _| running.contains(sb));
        self.running = running.to_vec();
    }

    /// Every running sandbox and every one that reported, with the daemon's
    /// own share.
    pub fn view(&self, daemon_cpu_pct: Option<f64>, now: u64) -> Stats {
        let mut names: Vec<&String> = self.running.iter().chain(self.samples.keys()).collect();
        names.sort();
        names.dedup();
        let sandboxes: Vec<SandboxStats> = names
            .into_iter()
            .map(|name| {
                let s = self.samples.get(name);
                let age = s.map(|s| now.saturating_sub(s.at_ms));
                SandboxStats {
                    name: name.clone(),
                    cpu_pct: s.and_then(|s| s.pct).filter(|_| age.is_some_and(|a| a <= STALE_MS)),
                    cpu_age_s: age.map(|a| a / 1000),
                    cpu_cores: s.and_then(|s| s.cores),
                    mem_bytes: s.and_then(|s| s.mem_used),
                    mem_total_bytes: s.and_then(|s| s.mem_total),
                }
            })
            .collect();
        let known: Vec<f64> = sandboxes.iter().filter_map(|s| s.cpu_pct).collect();
        let counted = known.len();
        let total = (daemon_cpu_pct.is_some() || counted > 0).then(|| round1(daemon_cpu_pct.unwrap_or(0.0) + known.iter().sum::<f64>()));
        Stats {
            daemon_cpu_pct,
            sandboxes_unknown: sandboxes.len() - counted,
            sandboxes_counted: counted,
            sandboxes,
            total_cpu_pct: total,
            ts: now,
        }
    }
}

/// The daemon's own CPU time, from `ps -o cputime=` (`0:01.23` on macOS,
/// `00:00:01` or `1-02:03:04` on Linux): `[[D-]H:]M:S[.frac]`.
pub fn parse_cputime(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let secs: f64 = parts.last()?.parse().ok().filter(|x: &f64| x.is_finite() && *x >= 0.0)?;
    let mut whole = 0u64;
    for p in &parts[..parts.len() - 1] {
        whole = whole * 60 + p.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs_f64((days * 86_400 + whole * 60) as f64 + secs))
}

/// The daemon's CPU, from successive readings of its CPU time: what the
/// previous reading to this one used, as a percent of one core.
#[derive(Debug, Default)]
pub struct OwnCpu {
    last: Option<(Duration, u64)>,
    pct: Option<f64>,
}

/// Readings closer than this reuse the last percentage instead of reading
/// again: a burst of requests costs one reading.
pub const OWN_MIN_GAP_MS: u64 = 1_000;

impl OwnCpu {
    /// Whether a new reading is worth taking now.
    pub fn due(&self, now: u64) -> bool {
        self.last.is_none_or(|(_, at)| now.saturating_sub(at) >= OWN_MIN_GAP_MS)
    }

    /// The last percentage, without a new reading.
    pub fn pct(&self) -> Option<f64> {
        self.pct
    }

    /// Fold in a reading (`None`: it failed, and the value is unknown until
    /// two good readings in a row).
    pub fn update(&mut self, cpu: Option<Duration>, now: u64) -> Option<f64> {
        let Some(cpu) = cpu else {
            self.last = None;
            self.pct = None;
            return None;
        };
        self.pct = match self.last {
            Some((prev, at)) if now > at && cpu >= prev => Some(round1((cpu - prev).as_secs_f64() * 1000.0 / (now - at) as f64 * 100.0)),
            _ => None,
        };
        self.last = Some((cpu, now));
        self.pct
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded in an sbx sandbox (sbx 0.53, 10 cores), two reads 10 s apart.
    const STAT_A: &str = "cpu  9617884 113451 1415149 177022282 68855 0 474660 0 0 0";
    const STAT_B: &str = "cpu  9619884 113451 1415649 177030282 68855 0 474760 0 0 0";
    const MEMINFO: &str = "MemTotal:       12417904 kB\nMemAvailable:    8297264 kB";

    #[test]
    fn proc_stat_and_meminfo_are_read() {
        let a = parse_proc_stat_cpu(STAT_A).unwrap();
        assert_eq!(a.total, 9617884 + 113451 + 1415149 + 177022282 + 68855 + 474660);
        assert_eq!(a.busy, 9617884 + 113451 + 1415149 + 474660);
        assert_eq!(parse_proc_stat_cpu("cpu0 1 2 3 4"), None, "a per-core line is not the total");
        assert_eq!(parse_proc_stat_cpu("cpu  1 2 x 4"), None);
        assert_eq!(parse_proc_stat_cpu("cpu  1 2"), None);
        assert_eq!(parse_proc_stat_cpu(""), None);
        assert_eq!(parse_meminfo(MEMINFO), (Some(12417904 * 1024), Some(8297264 * 1024)));
        assert_eq!(parse_meminfo("MemTotalX: 5 kB\nnonsense"), (None, None));
    }

    #[test]
    fn two_samples_make_a_percent_of_one_core() {
        let (a, b) = (parse_proc_stat_cpu(STAT_A).unwrap(), parse_proc_stat_cpu(STAT_B).unwrap());
        // 2600 busy jiffies of 10600 over 10 cores: 2.45 cores busy.
        assert_eq!(pct_of_one_core(a, b, 10), Some(245.3));
        assert_eq!(pct_of_one_core(b, a, 10), None, "counters going back: a restarted VM");
        assert_eq!(pct_of_one_core(a, a, 10), None);
    }

    #[test]
    fn a_sandbox_is_known_only_while_it_reports() {
        let mut book = StatsBook::new();
        book.set_running(&["a".into(), "cx".into()]);
        book.report("a", Some(STAT_A), Some(10), Some(MEMINFO), 1_000);
        let v = book.view(Some(1.5), 2_000);
        let a = &v.sandboxes[0];
        assert_eq!((a.cpu_pct, a.cpu_age_s, a.mem_bytes), (None, Some(1), Some((12417904 - 8297264) * 1024)), "one sample: no percent yet");
        assert_eq!((v.total_cpu_pct, v.sandboxes_counted, v.sandboxes_unknown), (Some(1.5), 0, 2));
        book.report("a", Some(STAT_B), Some(10), Some(MEMINFO), 11_000);
        let v = book.view(Some(1.5), 12_000);
        assert_eq!(v.sandboxes[0].cpu_pct, Some(245.3));
        assert_eq!(v.sandboxes[1], SandboxStats { name: "cx".into(), cpu_pct: None, cpu_age_s: None, cpu_cores: None, mem_bytes: None, mem_total_bytes: None });
        assert_eq!((v.total_cpu_pct, v.sandboxes_counted, v.sandboxes_unknown), (Some(246.8), 1, 1));
        // Stale: no number, the age says why.
        let v = book.view(None, 11_000 + STALE_MS + 1);
        assert_eq!((v.sandboxes[0].cpu_pct, v.sandboxes[0].cpu_age_s), (None, Some(30)));
        assert_eq!(v.total_cpu_pct, None, "nothing known at all");
        // Samples too far apart give none.
        book.report("a", Some(STAT_A), Some(10), None, 100_000);
        book.report("a", Some(STAT_B), Some(10), None, 100_000 + MAX_WINDOW_MS + 1);
        assert_eq!(book.view(None, 100_000 + MAX_WINDOW_MS + 2).sandboxes[0].cpu_pct, None);
        // An older script sends none of it: nothing changes.
        book.report("a", None, None, None, 500_000);
        assert_eq!(book.view(None, 500_001).sandboxes[0].cpu_age_s, Some(340), "still the last real sample");
        // A failed `sbx ls` keeps everything; a good one drops what stopped.
        book.set_running(&[]);
        assert_eq!(book.view(None, 1).sandboxes.len(), 2);
        book.set_running(&["cx".into()]);
        assert_eq!(book.view(None, 1).sandboxes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["cx"]);
        let json = serde_json::to_value(book.view(None, 1)).unwrap();
        assert!(json["sandboxes"][0]["cpu_pct"].is_null() && json["total_cpu_pct"].is_null(), "unknown is null, never 0: {json}");
    }

    #[test]
    fn cputime_reads_both_ps_formats() {
        assert_eq!(parse_cputime("0:01.50"), Some(Duration::from_millis(1500)), "macOS");
        assert_eq!(parse_cputime(" 1:02:03.25\n"), Some(Duration::from_millis(3_723_250)));
        assert_eq!(parse_cputime("00:00:07"), Some(Duration::from_secs(7)), "Linux");
        assert_eq!(parse_cputime("1-00:00:01"), Some(Duration::from_secs(86_401)));
        for bad in ["", "abc", "1:2:3:4", "-5", "0:-1"] {
            assert_eq!(parse_cputime(bad), None, "{bad}");
        }
    }

    #[test]
    fn own_cpu_needs_two_good_readings() {
        let mut c = OwnCpu::default();
        assert!(c.due(0));
        assert_eq!(c.update(Some(Duration::from_millis(1000)), 10_000), None, "first reading");
        assert!(!c.due(10_500));
        assert_eq!(c.update(Some(Duration::from_millis(1500)), 15_000), Some(10.0), "0.5 s of CPU in 5 s");
        assert_eq!(c.update(None, 20_000), None, "a failed reading is unknown, not 0");
        assert_eq!(c.update(Some(Duration::from_millis(2000)), 25_000), None, "and needs two again");
        assert_eq!(c.update(Some(Duration::from_millis(4000)), 26_000), Some(200.0), "can exceed 100");
    }
}
