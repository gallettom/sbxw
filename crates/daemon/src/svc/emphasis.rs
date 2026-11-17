//! Status pills: the daemon's `[emphasis]`, live, and what the UIs and the
//! bring-up read from it.
//!
//! The daemon's settings are read at start and replaced when Jarvis applies
//! an `emphasis_*` change, which also sends the `emphasis` event: the UIs
//! redraw at once. A sandbox's own `sbxw.toml` may set `enabled` and
//! `guidance` for itself (its snapshot, as every other per-sandbox setting);
//! the vocabulary is the daemon's alone.

use crate::app::App;
use crate::bus::Event;
use sbxw_core::emphasis::{self, Contract, EmphasisSettings};
use sbxw_core::SandboxRef;

impl App {
    /// The daemon's `[emphasis]` as it stands now.
    pub fn emphasis_settings(&self) -> EmphasisSettings {
        self.emphasis.lock().unwrap().clone()
    }

    /// A sandbox's own switches: its snapshot's `[emphasis]`, unless it was
    /// brought up on the daemon's own file (whose switches are the daemon's,
    /// and may have changed since).
    fn own_emphasis(&self, sb: &SandboxRef) -> Option<EmphasisSettings> {
        let rec = self.store.get(sb);
        let daemon_file = self.cfg.source.as_ref().or(self.cfg.asked.as_ref());
        if rec.config.is_none() || rec.config.as_ref() == daemon_file {
            return None;
        }
        rec.config_snapshot.map(|c| c.emphasis)
    }

    /// `(draw pills, write the guidance)` for one sandbox.
    pub fn emphasis_for(&self, sb: &SandboxRef) -> (bool, bool) {
        emphasis::effective(&self.emphasis_settings(), self.own_emphasis(sb).as_ref())
    }

    /// The guidance a bring-up writes into `sb`, `None` when pills or their
    /// guidance are off there (labels nobody draws are noise).
    pub fn emphasis_guidance(&self, sb: &SandboxRef) -> Option<String> {
        let (enabled, guidance) = self.emphasis_for(sb);
        (enabled && guidance).then(|| emphasis::guidance(&self.emphasis_settings().vocabulary()))
    }

    /// `GET /api/emphasis`: every sandbox sbxw knows, with its switch.
    pub fn emphasis_contract(&self) -> Contract {
        let daemon = self.emphasis_settings();
        let sandboxes: Vec<(String, Option<bool>)> =
            self.store.names().into_iter().map(|sb| (sb.to_string(), self.own_emphasis(&sb).and_then(|e| e.enabled))).collect();
        emphasis::contract(&daemon, &sandboxes)
    }

    /// The daemon's settings after a change Jarvis applied; the UIs hear it.
    pub(crate) fn set_emphasis(&self, settings: EmphasisSettings) {
        *self.emphasis.lock().unwrap() = settings;
        self.emphasis_changed();
    }

    /// Tell the UIs to redraw with the contract as it is now (a setting
    /// changed, or a sandbox was brought up on a file of its own).
    pub(crate) fn emphasis_changed(&self) {
        self.send(Event::Emphasis(serde_json::to_value(self.emphasis_contract()).unwrap_or_default()));
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::app_with;
    use sbxw_core::emphasis::EmphasisSettings;
    use sbxw_core::SandboxRef;

    #[tokio::test]
    async fn a_sandbox_s_own_file_overrides_the_daemon_s_switch_and_the_daemon_s_own_does_not() {
        let (d, app, _) = app_with(|_| sbxw_sbx::runner::Output::ok(""));
        let own = SandboxRef::parse("own").unwrap();
        let plain = SandboxRef::parse("plain").unwrap();
        app.store
            .update(&own, |r| {
                r.config = Some(d.path().join("proj/sbxw.toml"));
                r.config_snapshot = Some(Box::new(sbxw_core::config::Config {
                    emphasis: EmphasisSettings { enabled: Some(false), guidance: Some(false), ..Default::default() },
                    ..Default::default()
                }));
            })
            .unwrap();
        app.store.update(&plain, |r| r.config = None).unwrap();
        let c = app.emphasis_contract();
        assert_eq!((c.sandboxes["own"], c.sandboxes["plain"]), (false, true));
        assert!(app.emphasis_guidance(&own).is_none());
        assert!(app.emphasis_guidance(&plain).unwrap().contains("[[Done]]"));

        let mut rx = app.bus.subscribe();
        app.set_emphasis(EmphasisSettings { enabled: Some(false), ..Default::default() });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.name(), "emphasis");
        let v: serde_json::Value = serde_json::from_str(&ev.data()).unwrap();
        assert_eq!((v["enabled"].as_bool(), v["sandboxes"]["plain"].as_bool()), (Some(false), Some(false)));
        assert_eq!(v["words"]["verified"], "green");
    }
}
