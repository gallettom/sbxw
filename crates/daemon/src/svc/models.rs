//! The live model list: read through a running sandbox (whose proxy adds the
//! account's credential, as for subscription usage) at start and then once an
//! hour, never on a request. The last good list stays in memory; until one is
//! read, the offered list is `models` from `sbxw.toml` alone.
//!
//! Logged: counts and a reason class, never a header value or a body.

use crate::app::App;
use sbxw_core::config::ModelChoice;
use sbxw_core::models::{self, LiveModel};
use sbxw_core::SandboxName;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

/// First retry after a failed read; doubles up to the hourly period.
const RETRY_SECS: u64 = 300;

/// What a read returned: the kept models and how many entries it saw.
pub(crate) struct Read {
    pub models: Vec<LiveModel>,
    pub seen: usize,
}

impl App {
    /// The models a selector offers and a chosen model is checked against.
    pub fn offered_models(&self) -> Vec<ModelChoice> {
        let live = self.models_live.lock().unwrap();
        models::offered(&self.cfg.models, live.as_deref())
    }

    pub fn model_offered(&self, id: &str) -> bool {
        models::is_offered(&self.offered_models(), id)
    }

    /// One whole list, through whichever running sandbox answers first.
    /// The error is a reason class, safe to log.
    async fn fetch_models(&self) -> Result<Read, String> {
        let running: Vec<String> = self.sbx.ls().await.into_iter().filter(|s| s.is_running()).map(|s| s.name).collect();
        if running.is_empty() {
            return Err("no sandbox is running to ask through".into());
        }
        let mut headers: Vec<(&str, &str)> = models::MODELS_HEADERS.to_vec();
        if self.use_api_key {
            headers.push(("x-api-key", "proxy-managed"));
        }
        let mut last = String::new();
        for name in running {
            let Ok(sb) = SandboxName::parse(&name) else { continue };
            match self.fetch_models_via(&sb, &headers).await {
                Ok(r) => return Ok(r),
                Err(why) => last = format!("via {name}: {why}"),
            }
        }
        Err(last)
    }

    async fn fetch_models_via(&self, sb: &SandboxName, headers: &[(&str, &str)]) -> Result<Read, String> {
        let mut read = Read { models: Vec::new(), seen: 0 };
        let mut after: Option<String> = None;
        for _ in 0..models::MAX_PAGES {
            let url = models::page_url(after.as_deref());
            let body = self.sbx.curl_with_headers(sb, &url, headers).await.map_err(|_| "the request did not complete".to_string())?;
            let page = models::from_api(&body)?;
            read.seen += page.seen;
            read.models.extend(page.models);
            match page.next {
                Some(next) => after = Some(next),
                None => return Ok(read),
            }
        }
        Err(format!("more than {} pages", models::MAX_PAGES))
    }

    pub(crate) async fn poll_models(&self) {
        let mut last_line: Option<String> = None;
        let mut delay = Duration::ZERO;
        loop {
            delay = refresh_once(&self.models_live, || self.fetch_models(), delay, &mut last_line).await;
            tokio::time::sleep(delay).await;
        }
    }
}

/// One read; stores a good list, keeps the last one on failure, logs when the
/// outcome changes. Returns how long to wait before the next read: an hour
/// after a good read; after a failure, five minutes doubling to an hour
/// (`prev` is the wait that preceded this read).
pub(crate) async fn refresh_once<F, Fut>(store: &Mutex<Option<Vec<LiveModel>>>, fetch: F, prev: Duration, last_line: &mut Option<String>) -> Duration
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Read, String>>,
{
    let hour = Duration::from_secs(models::REFRESH_SECS);
    let (line, next) = match fetch().await {
        Ok(r) => {
            let line = format!("model list: {} Claude models of {} listed", r.models.len(), r.seen);
            *store.lock().unwrap() = Some(r.models);
            (line, hour)
        }
        Err(why) => {
            let next = if prev < Duration::from_secs(RETRY_SECS) || prev >= hour { Duration::from_secs(RETRY_SECS) } else { (prev * 2).min(hour) };
            let kept = store.lock().unwrap().as_ref().map_or("sbxw.toml's list".to_string(), |l| format!("the last list ({})", l.len()));
            (format!("could not read the model list ({why}); offering {kept}, retrying in {}s", next.as_secs()), next)
        }
    };
    if last_line.as_deref() != Some(&line) {
        tracing::info!("{line}");
        *last_line = Some(line);
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn m(id: &str) -> LiveModel {
        LiveModel { id: id.into(), label: id.into(), created_at: 1 }
    }

    /// Runs the schedule on a virtual clock for `until` seconds, answering
    /// each read from `answers` (failure when empty); the read times.
    async fn run(store: &Mutex<Option<Vec<LiveModel>>>, mut answers: VecDeque<Result<Vec<LiveModel>, String>>, until: u64) -> Vec<u64> {
        let (mut t, mut delay, mut line, mut reads) = (0u64, Duration::ZERO, None, Vec::new());
        while t <= until {
            reads.push(t);
            let answer = answers.pop_front().unwrap_or_else(|| Err("down".into()));
            delay = refresh_once(store, || async move { answer.map(|models| Read { seen: models.len(), models }) }, delay, &mut line).await;
            t += delay.as_secs();
        }
        reads
    }

    #[tokio::test]
    async fn a_good_list_is_read_at_start_then_once_an_hour() {
        let store = Mutex::new(None);
        let answers = (0..4).map(|_| Ok(vec![m("claude-fable-5-2")])).collect();
        assert_eq!(run(&store, answers, 3 * 3600).await, [0, 3600, 7200, 10800], "no read between hours");
        assert_eq!(store.lock().unwrap().as_ref().unwrap()[0].id, "claude-fable-5-2");
    }

    #[tokio::test]
    async fn a_failed_read_keeps_the_last_list_and_backs_off_to_the_hour() {
        let store = Mutex::new(None);
        let answers = VecDeque::from([Ok(vec![m("claude-fable-5-2")])]);
        let reads = run(&store, answers, 3 * 3600).await;
        // Good at 0; then failures at 3600 and back-off 300, 600, 1200, 2400, 3600.
        assert_eq!(reads, [0, 3600, 3900, 4500, 5700, 8100]);
        assert_eq!(store.lock().unwrap().as_ref().unwrap().len(), 1, "the last good list stays");
    }

    #[tokio::test]
    async fn never_read_leaves_the_store_empty_so_the_configured_list_is_offered() {
        let store = Mutex::new(None);
        let reads = run(&store, VecDeque::new(), 900).await;
        assert_eq!(reads, [0, 300, 900]);
        assert!(store.lock().unwrap().is_none());
        assert_eq!(models::offered(&sbxw_core::config::default_models(), store.lock().unwrap().as_deref()), sbxw_core::config::default_models());
    }
}
