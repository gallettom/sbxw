//! The web UI's Kits dialog: the `kits` line of the sbxw.toml files sbxw
//! uses, read and rewritten from the page.
//!
//! Only a file sbxw already uses can be written — the daemon's own config, or
//! the one a sandbox was created from — never a path the page names, so the
//! dialog edits configuration and nothing else on the host. The line is
//! rewritten in place ([`kits::set_kits_line`]): the file is the person's,
//! kept by hand, comments and all.

use crate::app::App;
use anyhow::{bail, Context, Result};
use sbxw_core::config::Config;
use sbxw_core::kits;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Who uses one sbxw.toml: the daemon, and the sandboxes created from it.
#[derive(Default)]
struct Users {
    daemon: bool,
    sandboxes: Vec<String>,
}

impl App {
    /// The config the daemon was started with, its `kits` read again from the
    /// file: the Kits dialog may have rewritten them since, and a sandbox the
    /// web UI creates must get what the dialog shows. Only `kits` — the rest
    /// (listeners, models) is what the daemon is running with.
    pub fn daemon_config(&self) -> Config {
        let mut cfg = self.cfg.clone();
        if let Some(fresh) = cfg.source.as_deref().and_then(|p| Config::load(p, Path::new("/")).ok()) {
            cfg.kits = fresh.kits;
        }
        cfg
    }

    fn kit_files(&self) -> BTreeMap<PathBuf, Users> {
        let mut out: BTreeMap<PathBuf, Users> = BTreeMap::new();
        if let Some(p) = &self.cfg.source {
            out.entry(p.clone()).or_default().daemon = true;
        }
        for name in self.store.names() {
            if let Some(p) = self.store.get(&name).config {
                out.entry(p).or_default().sandboxes.push(name);
            }
        }
        out
    }

    /// Each editable file with its `kits` as written (`null`: no line, the
    /// defaults apply), and the bundled kits to offer.
    pub fn kits_panel(&self) -> Value {
        let files: Vec<Value> = self
            .kit_files()
            .into_iter()
            .map(|(path, users)| {
                let (kits, error) = match std::fs::read_to_string(&path) {
                    Ok(raw) => match kits::read_kits_line(&raw) {
                        Ok(v) => (v, None),
                        Err(e) => (None, Some(format!("not valid TOML: {e}"))),
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
                    Err(e) => (None, Some(format!("not readable: {e}"))),
                };
                json!({
                    "path": path,
                    "exists": path.exists(),
                    "daemon": users.daemon,
                    "sandboxes": users.sandboxes,
                    "kits": kits,
                    "error": error,
                })
            })
            .collect();
        let bundled: Vec<Value> = kits::BUNDLED
            .iter()
            .map(|b| {
                let r = format!("sbxw:{b}");
                json!({ "ref": r, "name": b, "default": kits::DEFAULT_KITS.contains(&r.as_str()), "about": kits::about(b) })
            })
            .collect();
        json!({ "files": files, "bundled": bundled, "defaults": kits::DEFAULT_KITS })
    }

    /// Rewrite the `kits` line of `path`, one of [`App::kit_files`]; `None`
    /// removes it. What comes back is worth saying: a local kit that is not
    /// there, and the sandboxes that get the change only at their next `up`.
    pub fn set_kits(&self, path: &Path, list: Option<Vec<String>>) -> Result<Vec<String>> {
        let files = self.kit_files();
        let Some(users) = files.get(path) else {
            bail!("{} is not an sbxw.toml sbxw uses", path.display());
        };
        let list = list.map(|l| {
            let mut seen = Vec::new();
            for k in l.into_iter().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
                if !seen.contains(&k) {
                    seen.push(k);
                }
            }
            seen
        });
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let next = kits::set_kits_line(&raw, list.as_deref()).with_context(|| format!("{} is not valid TOML", path.display()))?;
        Config::parse(&next).context("the new kits line")?;

        let mut warnings = Vec::new();
        let dir = path.parent().unwrap_or(Path::new("/"));
        for k in list.iter().flatten() {
            let local = k.starts_with('/') || k.starts_with("./") || k.starts_with("../");
            let resolved = kits::resolve_kit_ref(dir, k.clone());
            if local && !Path::new(&resolved).join("spec.yaml").exists() {
                warnings.push(format!("{k}: no spec.yaml there — sbx will refuse it at the next bring-up"));
            }
        }
        if next != raw {
            write_in_place(path, &next)?;
            tracing::info!("web UI: kits of {} set to {:?}", path.display(), list);
        }
        if !users.sandboxes.is_empty() {
            warnings.push(format!(
                "{} get{} the change at {} next `sbxw up` (a reload from the list does not apply kits)",
                users.sandboxes.join(", "),
                if users.sandboxes.len() == 1 { "s" } else { "" },
                if users.sandboxes.len() == 1 { "its" } else { "their" },
            ));
        }
        Ok(warnings)
    }
}

/// Replace a file the person owns: written beside it and renamed over it, so a
/// crash never leaves half a config, with its permissions kept. Not
/// [`crate::store::write_atomic`], which closes the directory to 0700 — right
/// for sbxw's state, wrong for a project folder.
fn write_in_place(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_file_name(format!(".{}.sbxw-tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("sbxw.toml")));
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use crate::testing::app_with;
    use crate::testing::sbx_answer;

    #[test]
    fn only_a_config_sbxw_uses_is_written_and_its_other_lines_are_kept() {
        let (dir, app, _) = app_with(sbx_answer);
        let cfg = dir.path().join("proj/sbxw.toml");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "# mine\ntemplate = \"auto\"\n\n[[ports]]\nalias = \"a.local\"\nhost_port = 3000\nsandbox_port = 3000\n").unwrap();
        app.store.update("neos", |r| r.config = Some(cfg.clone())).unwrap();

        let other = dir.path().join("elsewhere.toml");
        assert!(app.set_kits(&other, Some(vec!["sbxw:retro".into()])).is_err(), "a path the page names is refused");
        assert!(app.set_kits(&cfg, Some(vec!["sbxw:nope".into()])).is_err(), "an unknown bundled kit is refused");

        let w = app.set_kits(&cfg, Some(vec!["sbxw:codemap".into(), " sbxw:retro ".into(), "sbxw:codemap".into(), "./missing".into()])).unwrap();
        assert!(w.iter().any(|w| w.starts_with("./missing")), "{w:?}");
        assert!(w.iter().any(|w| w.starts_with("neos gets the change at its next `sbxw up`")), "{w:?}");
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.starts_with("# mine\ntemplate = \"auto\"\nkits = [\n    \"sbxw:codemap\",\n    \"sbxw:retro\",\n    \"./missing\",\n]\n"), "{text}");
        assert!(text.contains("[[ports]]\nalias = \"a.local\""));

        let panel = app.kits_panel();
        let file = panel["files"].as_array().unwrap().iter().find(|f| f["path"] == cfg.to_string_lossy().as_ref()).unwrap();
        assert_eq!(file["kits"][1], "sbxw:retro");
        assert_eq!(file["sandboxes"][0], "neos");

        app.set_kits(&cfg, None).unwrap();
        assert!(!std::fs::read_to_string(&cfg).unwrap().contains("kits"));
    }
}
