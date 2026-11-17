//! CPU and memory for the UI's status bar: `GET /api/stats` and the `stats`
//! event (see `sbxw_core::stats` for where each number comes from).
//!
//! Sandboxes cost nothing to watch: they report with their status line. The
//! daemon's own CPU is read from the OS (`/proc/self/schedstat` where there is
//! one, else `ps -o cputime=`, bounded by [`PROBE_TIMEOUT`]), at most once a
//! second, and only when asked: by a `GET /api/stats`, or every
//! [`TICK`] while at least one `/api/stream` is open. With no tab open,
//! nothing runs.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::stats::{parse_cputime, Stats};
use sbxw_core::SandboxRef;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How often the `stats` event goes out while someone listens.
pub const TICK: Duration = Duration::from_secs(5);
/// Longest a reading of the daemon's CPU may take; past it, unknown.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How the daemon reads its own CPU time.
#[derive(Debug, Clone)]
pub enum CpuProbe {
    /// `/proc/self/schedstat` when readable (Linux), else `ps`.
    Os,
    /// This program with `ps`'s arguments: tests put a script here.
    Command(PathBuf),
}

impl App {
    /// A sandbox's status line reported its `/proc` counters.
    pub fn report_sandbox_stats(&self, sandbox: &SandboxRef, proc_stat_cpu: Option<&str>, cores: Option<u32>, meminfo: Option<&str>) {
        self.stats.lock().unwrap().report(&sandbox.to_string(), proc_stat_cpu, cores, meminfo, now_ms());
    }

    /// What `GET /api/stats` answers and the `stats` event carries.
    pub async fn stats(&self) -> Stats {
        let due = self.own_cpu.lock().unwrap().due(now_ms());
        let daemon = if due {
            let cpu = self.read_own_cpu().await;
            self.own_cpu.lock().unwrap().update(cpu, now_ms())
        } else {
            self.own_cpu.lock().unwrap().pct()
        };
        self.stats.lock().unwrap().view(daemon, now_ms())
    }

    /// The daemon's CPU time so far; `None` when it cannot be read in time.
    async fn read_own_cpu(&self) -> Option<Duration> {
        let probe = self.cpu_probe.lock().unwrap().clone();
        let program = match probe {
            CpuProbe::Os => {
                // Nanoseconds on the CPU, first field: no clock-tick guess.
                if let Ok(s) = tokio::fs::read_to_string("/proc/self/schedstat").await {
                    return s.split_whitespace().next()?.parse().ok().map(Duration::from_nanos);
                }
                PathBuf::from("ps")
            }
            CpuProbe::Command(p) => p,
        };
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(["-o", "cputime=", "-p", &std::process::id().to_string()]).kill_on_drop(true).stdin(std::process::Stdio::null());
        let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await.ok()?.ok()?;
        if !out.status.success() {
            return None;
        }
        parse_cputime(&String::from_utf8_lossy(&out.stdout))
    }

    /// While at least one `/api/stream` is open, a `stats` event every
    /// [`TICK`]; with none, it waits without reading anything.
    pub fn start_stats_ticker(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut clients = app.bus.clients.subscribe();
            loop {
                if clients.wait_for(|n| *n > 0).await.is_err() {
                    return;
                }
                let mut tick = tokio::time::interval(TICK);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    if *app.bus.clients.borrow() == 0 {
                        break;
                    }
                    let s = app.stats().await;
                    app.send(Event::Stats(s));
                }
            }
        });
    }
}
