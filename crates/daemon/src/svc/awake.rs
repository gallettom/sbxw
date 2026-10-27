//! Keeping the Mac awake while Remote Control is on, and telling the agents
//! driven from claude.ai when its battery runs low. The rules are in
//! `sbxw_core::power`; this is the loop that reads the world and holds
//! `caffeinate`.

use crate::app::App;
use sbxw_core::power::{self, Battery, NoticeBook};
use sbxw_core::{remote, SandboxName};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// How often the battery and the Remote Control sessions are looked at.
const POLL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Awake {
    battery: Option<Battery>,
    /// Sandboxes with Remote Control on in a live session, at the last look.
    remote: HashSet<String>,
    /// Sandboxes Remote Control was turned on in from the web UI. Records
    /// flagged `remote_control` are looked at anyway; a sandbox already
    /// signed in to claude.ai never gets the flag.
    watched: HashSet<String>,
    caffeinate: Option<tokio::process::Child>,
    notices: NoticeBook,
}

impl Awake {
    fn set_held(&mut self, held: bool) {
        // One that exited on its own is not holding anything.
        if self.caffeinate.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None))) {
            self.caffeinate = None;
        }
        match (held, self.caffeinate.take()) {
            (true, None) => {
                let spawned = tokio::process::Command::new("caffeinate")
                    .args(power::caffeinate_args(std::process::id()))
                    .kill_on_drop(true)
                    .spawn();
                match spawned {
                    Ok(c) => {
                        tracing::info!("keep awake: holding the Mac awake (caffeinate)");
                        self.caffeinate = Some(c);
                    }
                    Err(e) => tracing::warn!("keep awake: could not start caffeinate: {e}"),
                }
            }
            (false, Some(mut c)) => {
                tracing::info!("keep awake: letting the Mac sleep");
                let _ = c.start_kill();
            }
            (_, held_one) => self.caffeinate = held_one,
        }
    }
}

impl App {
    /// The keep-awake loop. macOS only: `caffeinate` and `pmset` are Apple's.
    pub(crate) fn start_keep_awake(self: &Arc<Self>) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(POLL);
            loop {
                tick.tick().await;
                app.keep_awake_tick().await;
            }
        });
    }

    async fn keep_awake_tick(&self) {
        let battery = read_battery().await;
        let remote = self.remote_sessions().await;
        let mut a = self.awake.lock().unwrap();
        a.battery = battery;
        a.remote = remote;
        self.reassess(&mut a);
    }

    fn reassess(&self, a: &mut Awake) {
        if cfg!(target_os = "macos") {
            a.set_held(power::hold(self.cfg.keep_awake, !a.remote.is_empty(), a.battery, self.cfg.keep_awake_min_battery));
        }
    }

    /// Removed sandboxes: no longer looked at, and the Mac let go at once if
    /// one was the last on Remote Control — not a poll later.
    pub(crate) fn forget_remote(&self, names: &[&SandboxName]) {
        let mut a = self.awake.lock().unwrap();
        for n in names {
            a.watched.remove(n.as_str());
            a.remote.remove(n.as_str());
        }
        self.reassess(&mut a);
    }

    /// Sandboxes `sbx ls` no longer reports running are off Remote Control:
    /// one stopped or removed from the CLI tells the daemon nothing else.
    pub(crate) fn keep_remote_running(&self, running: &[String]) {
        let mut a = self.awake.lock().unwrap();
        let before = a.remote.len();
        a.remote.retain(|n| running.contains(n));
        if a.remote.len() != before {
            self.reassess(&mut a);
        }
    }

    /// Running sandboxes with Remote Control on in some live session. Only
    /// the ones it may be on in are probed: one `sbx exec` each.
    pub(crate) async fn remote_sessions(&self) -> HashSet<String> {
        let mut candidates: Vec<String> = self.store.names().into_iter().filter(|n| self.store.get(n).remote_control).collect();
        candidates.extend(self.awake.lock().unwrap().watched.iter().cloned());
        candidates.sort();
        candidates.dedup();
        let mut on = HashSet::new();
        for name in candidates {
            let Ok(sb) = SandboxName::parse(&name) else { continue };
            if !self.sbx.is_running(&sb).await {
                continue;
            }
            if let Ok(out) = self.sbx.exec(&sb, &["node", "-e", remote::PROBE_SCRIPT], None).await {
                if remote::any_active(&out.stdout_str()) {
                    on.insert(name);
                }
            }
        }
        on
    }

    /// Remote Control was turned on in `sandbox`: look at it from now on.
    pub(crate) fn watch_remote(&self, sandbox: &SandboxName) {
        self.awake.lock().unwrap().watched.insert(sandbox.to_string());
    }

    /// The battery note to add to `sandbox`'s next prompt, if one is due.
    /// Only for Remote Control sessions: at the Mac, the menu bar says it.
    pub fn battery_notice(&self, sandbox: &SandboxName) -> Option<String> {
        let mut a = self.awake.lock().unwrap();
        if !a.remote.contains(sandbox.as_str()) {
            return None;
        }
        let battery = a.battery;
        a.notices.due(sandbox.as_str(), battery, self.cfg.battery_warn_below, self.cfg.keep_awake_min_battery, self.cfg.keep_awake)
    }
}

async fn read_battery() -> Option<Battery> {
    let out = tokio::process::Command::new("pmset").args(["-g", "batt"]).output().await.ok()?;
    power::parse_pmset(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::power::Battery;
    use sbxw_core::SandboxName;
    use sbxw_sbx::runner::Output;

    #[tokio::test]
    async fn only_sandboxes_with_remote_control_on_are_found() {
        let (_d, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => Output::ok("SANDBOX   AGENT    STATUS\nneos      claude   running\nplain     claude   running\n"),
            Some("exec") if a.iter().any(|x| x == "-e") => Output::ok(r#"{"sessions":[{"cwd":"/w","rc":true}]}"#),
            _ => sbx_answer(a),
        });
        app.store.update("neos", |r| r.remote_control = true).unwrap();
        app.store.update("plain", |r| r.workspace = Some("/w".into())).unwrap();
        let on = app.remote_sessions().await;
        assert_eq!(on.into_iter().collect::<Vec<_>>(), ["neos"]);
        // `plain` was never probed: Remote Control was not turned on there.
        assert!(!fake.log().iter().any(|c| c.starts_with("exec") && c.contains("plain")));

        app.watch_remote(&SandboxName::parse("plain").unwrap());
        assert_eq!(app.remote_sessions().await.len(), 2);
    }

    #[tokio::test]
    async fn the_low_battery_note_goes_to_remote_sessions_once() {
        let (_d, app, _fake) = app_with(sbx_answer);
        let neos = SandboxName::parse("neos").unwrap();
        {
            let mut a = app.awake.lock().unwrap();
            a.battery = Some(Battery { percent: 12, on_battery: true });
            a.remote.insert("neos".into());
        }
        assert!(app.battery_notice(&neos).unwrap().contains("12%"));
        assert!(app.battery_notice(&neos).is_none());
        assert!(app.battery_notice(&SandboxName::parse("other").unwrap()).is_none());
    }

    #[tokio::test]
    async fn a_removed_or_stopped_sandbox_is_off_remote_control_at_once() {
        let (_d, app, _fake) = app_with(sbx_answer);
        let neos = SandboxName::parse("neos").unwrap();
        app.watch_remote(&neos);
        {
            let mut a = app.awake.lock().unwrap();
            a.remote.extend(["neos".to_string(), "other".to_string()]);
        }
        app.keep_remote_running(&["neos".into()]);
        assert_eq!(app.awake.lock().unwrap().remote.len(), 1);
        app.forget_remote(&[&neos]);
        let a = app.awake.lock().unwrap();
        assert!(a.remote.is_empty() && a.watched.is_empty());
    }
}
