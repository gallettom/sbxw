//! Fixtures: a screen's daemon data as a JSON file, so every screen renders
//! headless without a daemon (`--fixture <file>`, or `scripts/screen.sh`).
//!
//! A fixture lives at `desktop/fixtures/<screen>/<name>.json`:
//!
//! ```json
//! {
//!   "daemon": { "sandboxes": [ { "name": "web-app", "agent": "claude", "status": "running" } ] },
//!   "app": { "selected": "web-app", "panes": 2, "split": "columns", "open": ["web-app"] }
//! }
//! ```
//!
//! `daemon` holds what each route answers, by the key the screen reads
//! (`sandboxes` is `GET /api/sandboxes`); later waves add their keys
//! (`threads`, `plans`, ...). `app` (optional) sets the core's state at
//! start: the selected sandbox, the layout, and sandboxes shown in panes
//! (with no terminal in them: a fixture has none).

use crate::client::FixtureData;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppInit {
    pub selected: Option<String>,
    pub panes: Option<usize>,
    pub split: Option<String>,
    #[serde(default)]
    pub open: Vec<String>,
    /// Pane that has the focus, from 1.
    pub focused: Option<usize>,
    /// Show this connection state instead of the fixture's own ("up").
    pub connection: Option<ConnInit>,
    /// The Jarvis panel is open at start.
    #[serde(default)]
    pub jarvis_open: bool,
    /// Use the scripted audio (`voice::FakeAudio`) instead of none.
    #[serde(default)]
    pub fake_audio: bool,
    /// Messages already in the conversation: `[who, text]` (`me`, `jarvis`, `note`, `error`).
    #[serde(default)]
    pub jarvis_log: Vec<(String, String)>,
    /// Features' panels open at start (screen names; the last is shown).
    #[serde(default)]
    pub panels: Vec<String>,
    /// A feature's modal open at start: `[screen, arg]`.
    #[serde(default)]
    pub modal: Option<(String, serde_json::Value)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnInit {
    pub state: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    /// What the fixture shows and why (not read).
    #[serde(default)]
    #[allow(dead_code)]
    about: Option<String>,
    #[serde(default)]
    daemon: serde_json::Value,
    #[serde(default)]
    app: AppInit,
}

pub struct Fixture {
    pub name: String,
    pub path: PathBuf,
    /// The raw `daemon` object, every key.
    pub raw: serde_json::Value,
    pub daemon: FixtureData,
    pub app: AppInit,
}

impl Fixture {
    pub fn load(path: &Path) -> Result<Fixture> {
        let text = std::fs::read_to_string(path).with_context(|| format!("fixture {}", path.display()))?;
        let f: File = serde_json::from_str(&text).with_context(|| format!("fixture {}", path.display()))?;
        let raw = if f.daemon.is_null() { serde_json::json!({}) } else { f.daemon };
        let daemon: FixtureData = serde_json::from_value(raw.clone()).with_context(|| format!("fixture {}: daemon", path.display()))?;
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Fixture { name, path: path.to_path_buf(), raw, daemon, app: f.app })
    }

    /// `desktop/fixtures/<screen>/<name>.json`, or `name` itself when it is a path.
    pub fn resolve(screen: &str, name: &str) -> PathBuf {
        let p = PathBuf::from(name);
        if p.extension().is_some() || p.components().count() > 1 {
            return p;
        }
        dir().join(screen).join(format!("{name}.json"))
    }

    /// What a screen receives with this fixture and the given core state.
    pub fn screen_data(&self, app: &AppView) -> serde_json::Value {
        screen_data(&self.raw, app)
    }
}

pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// The core's state as screens see it, under `data.app` (`docs/screens.md`, "Data").
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AppView {
    pub connection: Connection,
    pub selected: Option<String>,
    pub focused: usize,
    pub layout: LayoutView,
    pub panes: Vec<PaneView>,
    pub platform: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Connection {
    /// `connecting`, `up`, `down`.
    pub state: &'static str,
    pub text: String,
}

impl Default for Connection {
    fn default() -> Self {
        Connection { state: "connecting", text: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LayoutView {
    pub panes: usize,
    pub split: &'static str,
}

impl Default for LayoutView {
    fn default() -> Self {
        LayoutView { panes: 1, split: "columns" }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PaneView {
    pub index: usize,
    pub sandbox: Option<String>,
    pub mode: &'static str,
    /// The thread whose session it shows (`None`: the sandbox's own).
    pub thread: Option<String>,
    pub focused: bool,
    /// `empty`, `connecting`, `connected`, `closed`, `error`.
    pub status: String,
}

/// The daemon's data (every key at the top level) plus `app`.
pub fn screen_data(daemon: &serde_json::Value, app: &AppView) -> serde_json::Value {
    let mut d = match daemon {
        serde_json::Value::Object(o) => o.clone(),
        _ => serde_json::Map::new(),
    };
    d.entry("sandboxes").or_insert_with(|| serde_json::json!([]));
    if !d.get("jarvis").is_some_and(|j| j.is_object()) {
        let j = jarvis_of(daemon);
        d.insert("jarvis".into(), j.screen_data(false));
    }
    d.insert("app".into(), serde_json::to_value(app).expect("app view"));
    serde_json::Value::Object(d)
}

/// Jarvis as a fixture's daemon keys describe it (`jarvis_status`,
/// `jarvis_proposals`, `sessions`, `voice`), for a screen drawn alone.
pub fn jarvis_of(daemon: &serde_json::Value) -> crate::jarvis::Jarvis {
    let mut j = crate::jarvis::Jarvis::default();
    if let Some(s) = daemon["jarvis_status"]["state"].as_str() {
        j.status = s.to_string();
    }
    j.status_error = daemon["jarvis_status"]["error"].as_str().map(str::to_string);
    j.voice = daemon.get("voice").cloned();
    j.seed_proposals(daemon["jarvis_proposals"].as_array().cloned().unwrap_or_default());
    j.seed_sessions(daemon["sessions"].as_array().map(Vec::as_slice).unwrap_or(&[]));
    j.open = true;
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_parses() {
        let mut n = 0;
        for screen in std::fs::read_dir(dir()).unwrap() {
            for f in std::fs::read_dir(screen.unwrap().path()).unwrap() {
                let p = f.unwrap().path();
                Fixture::load(&p).unwrap_or_else(|e| panic!("{e:#}"));
                n += 1;
            }
        }
        assert!(n >= 3);
    }

    #[test]
    fn names_resolve_under_the_screen() {
        assert!(Fixture::resolve("sandboxes", "basic").ends_with("fixtures/sandboxes/basic.json"));
        assert_eq!(Fixture::resolve("sandboxes", "/x/y.json"), PathBuf::from("/x/y.json"));
    }
}
