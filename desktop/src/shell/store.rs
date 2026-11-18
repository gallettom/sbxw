//! The app's own state across runs: `~/.sbxw/desktop/state.json` (or
//! `$SBXW_DESKTOP_STATE`). One JSON object, written whole on each change
//! (to a temporary file, then renamed). Headless runs without
//! `$SBXW_DESKTOP_STATE` keep it in memory only, so a demo never touches the
//! person's file.
//!
//! Keys (each owned by one part of the core; features never write here
//! directly, they ask through [`crate::feat::Action::Remember`]):
//!
//! * `overrides`: `{<screen>: {builtin, override}}`, the hash of the
//!   built-in screen that was current when the override file appeared or
//!   last changed, and the override's own hash (see `shell.rs`,
//!   `check_overrides`).
//! * `features`: `{<feature>: {<key>: value}}`, what a feature asked to
//!   keep (the remote host last used, Setup's last tab).
//! * `uploads`: `{skip_drop_dialog: true}` once the person ticked "Do not
//!   ask again for files I drop" in the core's upload dialog; only that box
//!   sets it, Setup's "Ask before uploading dropped files" clears it
//!   (`shell/features.rs`, "Files dropped on a pane").
//! * `theme`: `"dark"` or `"graphite"`, the person's choice from the status
//!   bar's theme switch (`set_theme`); absent, or a name no theme has: Dark.

use serde_json::{Map, Value as J};
use std::path::PathBuf;

pub struct Store {
    path: Option<PathBuf>,
    v: Map<String, J>,
}

impl Store {
    /// The state file for this run (see the module's doc).
    pub fn open() -> Store {
        let path = match std::env::var_os("SBXW_DESKTOP_STATE").filter(|p| !p.is_empty()) {
            Some(p) => Some(PathBuf::from(p)),
            None if super::headless() || cfg!(test) => None,
            None => Some(crate::builtin::default_dir().with_file_name("state.json")),
        };
        Store::at(path)
    }

    /// In memory only (tests).
    pub fn memory() -> Store {
        Store { path: None, v: Map::new() }
    }

    pub fn at(path: Option<PathBuf>) -> Store {
        let v = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<J>(&t).ok()).and_then(|j| j.as_object().cloned()).unwrap_or_default();
        Store { path, v }
    }

    pub fn get(&self, key: &str) -> &J {
        self.v.get(key).unwrap_or(&J::Null)
    }

    /// Sets `key` and writes the file (a failure is logged, never fatal).
    pub fn set(&mut self, key: &str, value: J) {
        if self.v.get(key) == Some(&value) {
            return;
        }
        if value.is_null() {
            self.v.remove(key);
        } else {
            self.v.insert(key.to_string(), value);
        }
        self.save();
    }

    /// `key.sub` (one level down).
    pub fn set_in(&mut self, key: &str, sub: &str, value: J) {
        let mut o = self.get(key).as_object().cloned().unwrap_or_default();
        if value.is_null() {
            o.remove(sub);
        } else {
            o.insert(sub.to_string(), value);
        }
        self.set(key, if o.is_empty() { J::Null } else { J::Object(o) });
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(&self.v).unwrap_or_default();
        if let Err(e) = std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, p)) {
            log::warn!("could not write {}: {e}", p.display());
        }
    }
}

/// A stable 64-bit hash of a text (FNV-1a), the same across builds and
/// Rust versions (`DefaultHasher` promises neither).
pub fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_survives_a_restart_and_hashes_are_stable() {
        let p = std::env::temp_dir().join(format!("sbxw-store-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut s = Store::at(Some(p.clone()));
        s.set_in("features", "create", serde_json::json!({ "host": "build-box" }));
        let s2 = Store::at(Some(p.clone()));
        assert_eq!(s2.get("features")["create"]["host"], "build-box");
        let mut s2 = s2;
        s2.set_in("features", "create", J::Null);
        assert!(Store::at(Some(p.clone())).get("features").is_null());
        std::fs::remove_file(&p).ok();
        assert_eq!(hash(""), "cbf29ce484222325");
        assert_eq!(hash("a"), "af63dc4c8601ec8c");
    }
}
