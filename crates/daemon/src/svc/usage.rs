//! Subscription usage: what sandboxes report, and what the daemon asks for
//! itself — through a sandbox, whose proxy authenticates the request with the
//! account's own credentials (on a Mac there is reliably no host token with
//! the scope it needs).

use crate::app::{now_ms, App};
use sbxw_core::usage::{self, Reading, Usage};
use sbxw_core::SandboxName;
use std::time::Duration;

impl App {
    pub fn usage(&self) -> Usage {
        self.usage.lock().unwrap().clone()
    }

    pub fn report_usage(&self, r: Reading) {
        self.usage.lock().unwrap().apply(r, now_ms());
    }

    /// One reading through whichever running sandbox answers first.
    async fn fetch_usage(&self) -> Result<(Reading, String), String> {
        let running: Vec<String> = self.sbx.ls().await.into_iter().filter(|s| s.is_running()).map(|s| s.name).collect();
        if running.is_empty() {
            return Err("no sandbox is running to ask through".into());
        }
        let mut last = String::new();
        for name in running {
            let Ok(sb) = SandboxName::parse(&name) else { continue };
            match self.sbx.curl(&sb, usage::USAGE_URL).await {
                Ok(body) => match usage::from_api(&body) {
                    Ok(r) => return Ok((r, name)),
                    Err(e) => last = format!("{name}: {e}"),
                },
                Err(e) => last = format!("{name}: {e:#}"),
            }
        }
        Err(last)
    }

    /// Every five minutes, backing off (doubling to half an hour) on failure:
    /// the endpoint rate-limits. Logged when what it says changes.
    pub(crate) async fn poll_usage(&self) {
        let mut delay = Duration::from_secs(usage::POLL_SECS);
        let mut last: Option<String> = None;
        loop {
            let report = match self.fetch_usage().await {
                Ok((r, via)) => {
                    delay = Duration::from_secs(usage::POLL_SECS);
                    let show = |p: Option<f64>| p.map_or("—".to_string(), |p| format!("{}%", p.round()));
                    let line = format!("subscription usage: 5h {}, 7d {} (asked from sandbox {via})", show(r.five_hour_pct), show(r.seven_day_pct));
                    self.report_usage(r);
                    line
                }
                Err(why) => {
                    delay = (delay * 2).min(Duration::from_secs(usage::POLL_MAX_SECS));
                    format!("could not read subscription usage: {why} — retrying in {}s", delay.as_secs())
                }
            };
            if last.as_deref() != Some(&report) {
                tracing::info!("{report}");
                last = Some(report);
            }
            tokio::time::sleep(delay).await;
        }
    }
}
