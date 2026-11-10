//! The web UI's Kits dialog: the `kits` line of the sbxw.toml files sbxw
//! uses, read and rewritten from the page.
//!
//! Only a file sbxw already uses can be written — the daemon's own config, or
//! the one a sandbox was created from — never a path the page names, so the
//! dialog edits nothing else on the host. The line is rewritten in place
//! ([`kits::set_kits_line`]): the file is hand-kept, comments and all.

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
    /// The config the daemon started with, `kits` as the Kits dialog last
    /// set them: a sandbox the web UI creates gets what the dialog shows. Never
    /// read again from the file, which an agent may be able to write; the rest
    /// (listeners, models) is what the daemon runs with.
    pub fn daemon_config(&self) -> Config {
        let mut cfg = self.cfg.clone();
        cfg.kits = self.daemon_kits.lock().unwrap().clone();
        cfg
    }

    fn kit_files(&self) -> BTreeMap<PathBuf, Users> {
        let mut out: BTreeMap<PathBuf, Users> = BTreeMap::new();
        if let Some(p) = &self.cfg.source {
            out.entry(p.clone()).or_default().daemon = true;
        }
        for name in self.store.names() {
            if let Some(p) = self.store.get(&name).config {
                out.entry(p).or_default().sandboxes.push(name.to_string());
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
    /// removes it. Returns warnings: a local kit that is not there, the
    /// sandboxes that get the change only at their next `up`.
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
        let parsed = Config::parse(&next).context("the new kits line")?;

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
        // A kit the person has just added here is their decision, made on
        // the spot: its grant line is recorded as approved, so the next
        // bring-up does not ask again. Only the lines this write adds: a kit
        // that was already in the file (an agent may have put it there) is
        // still asked about.
        let before = Config::parse(&raw).map(|c| c.kits).unwrap_or_else(|_| Config::default().kits);
        let added = sbxw_core::consent::new_grants(&kit_lines(dir, parsed.kits.clone()), &kit_lines(dir, before));
        if !added.is_empty() {
            let all = sbxw_core::consent::grants(&Config::load(path, Path::new("/"))?);
            self.store.check_grants(&crate::store::grant_key(path), &all, &added, &added)?;
        }
        if users.daemon {
            // As `Config::load` resolves them, from what was just written.
            *self.daemon_kits.lock().unwrap() = parsed.kits.into_iter().map(|k| kits::resolve_kit_ref(dir, k)).collect();
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

/// The grant lines of a kits list alone, resolved against `dir` as
/// `Config::load` resolves them.
fn kit_lines(dir: &Path, list: Vec<String>) -> Vec<String> {
    let kits = list.into_iter().map(|k| kits::resolve_kit_ref(dir, k)).collect();
    sbxw_core::consent::grants(&Config { kits, ..Config::default() })
}

/// Replace a file the person owns: written beside it and renamed over it, so a
/// crash never leaves half a config, with its permissions kept. Not
/// [`crate::store::write_atomic`], which closes the directory to 0700 — right
/// for sbxw's state, wrong for a project folder.
///
/// The directory is the agent's to write, so nothing in it is trusted: a
/// destination that is a symlink is refused, and the temporary file has a
/// random name and is created with `create_new`, which never opens an existing
/// file and never follows a symlink planted under that name (`O_CREAT|O_EXCL`
/// fails on one, POSIX).
fn write_in_place(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("{} is a symlink: sbxw does not write through it", path.display());
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("sbxw.toml");
    let tmp = path.with_file_name(format!(".{name}.{}.sbxw-tmp", sbxw_core::ids::random_hex(8)));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let mut file = open.open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let written = (|| -> Result<()> {
        file.write_all(text.as_bytes())?;
        if let Ok(meta) = std::fs::metadata(path) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        Ok(())
    })()
    .with_context(|| format!("writing {}", tmp.display()));
    drop(file);
    let done = written.and_then(|()| std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display())));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
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
        app.store.update(sbxw_core::SandboxRef::parse("myapp").unwrap(), |r| r.config = Some(cfg.clone())).unwrap();

        let other = dir.path().join("elsewhere.toml");
        assert!(app.set_kits(&other, Some(vec!["sbxw:retro".into()])).is_err(), "a path the page names is refused");
        assert!(app.set_kits(&cfg, Some(vec!["sbxw:nope".into()])).is_err(), "an unknown bundled kit is refused");

        let w = app.set_kits(&cfg, Some(vec!["sbxw:codemap".into(), " sbxw:retro ".into(), "sbxw:codemap".into(), "./missing".into()])).unwrap();
        assert!(w.iter().any(|w| w.starts_with("./missing")), "{w:?}");
        assert!(w.iter().any(|w| w.starts_with("myapp gets the change at its next `sbxw up`")), "{w:?}");
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.starts_with("# mine\ntemplate = \"auto\"\nkits = [\n    \"sbxw:codemap\",\n    \"sbxw:retro\",\n    \"./missing\",\n]\n"), "{text}");
        assert!(text.contains("[[ports]]\nalias = \"a.local\""));

        let panel = app.kits_panel();
        let file = panel["files"].as_array().unwrap().iter().find(|f| f["path"] == cfg.to_string_lossy().as_ref()).unwrap();
        assert_eq!(file["kits"][1], "sbxw:retro");
        assert_eq!(file["sandboxes"][0], "myapp");

        app.set_kits(&cfg, None).unwrap();
        assert!(!std::fs::read_to_string(&cfg).unwrap().contains("kits"));
    }

    use sbxw_core::config::Config;
    use std::path::Path;

    #[test]
    fn the_daemons_kits_change_only_through_the_dialog() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sbxw.toml");
        std::fs::write(&file, "kits = [\"sbxw:codemap\"]\n").unwrap();
        let mut cfg = Config::load(&file, Path::new("/")).unwrap();
        cfg.source = Some(file.clone());
        let app = crate::app::AppBuilder {
            paths: crate::store::Paths::under(d.path()),
            cfg,
            sbx: sbxw_sbx::Sbx::new(sbxw_sbx::runner::Fake::new(sbx_answer)),
            hosts: crate::hostsys::for_tests(&d.path().join("hosts")),
            use_api_key: true,
            sbx_version: Some("0.51.0".into()),
        }
        .build();
        assert_eq!(app.daemon_config().kits, vec!["sbxw:codemap".to_string()]);

        // An agent edits the file: the next web UI sandbox does not see it.
        std::fs::write(&file, "kits = [\"./planted\"]\n").unwrap();
        assert_eq!(app.daemon_config().kits, vec!["sbxw:codemap".to_string()]);

        // The dialog's write is the one way in, resolved as a load would.
        app.set_kits(&file, Some(vec!["sbxw:retro".into(), "./mine".into()])).unwrap();
        let mine = d.path().join("mine").to_string_lossy().into_owned();
        assert_eq!(app.daemon_config().kits, vec!["sbxw:retro".to_string(), mine]);
    }

    /// The kit the person adds in the dialog is approved; one already in
    /// the file, which the dialog did not add, is still asked about.
    #[test]
    fn a_kit_added_in_the_dialog_is_approved_and_no_other() {
        let (dir, app, _) = app_with(sbx_answer);
        let file = dir.path().join("proj/sbxw.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "kits = [\"sbxw:codemap\", \"/planted/kit\"]\nnetwork_allow = [\"x.com\"]\n").unwrap();
        app.store.update(sbxw_core::SandboxRef::parse("myapp").unwrap(), |r| r.config = Some(file.clone())).unwrap();
        app.set_kits(&file, Some(vec!["sbxw:codemap".into(), "/planted/kit".into(), "sbxw:k8s-tools".into()])).unwrap();
        let key = crate::store::grant_key(&file);
        assert_eq!(app.store.all_approved_grants()[&key], ["kit: sbxw:k8s-tools"]);
        let cfg = Config::load(&file, Path::new("/")).unwrap();
        let e = app.check_consent(&cfg, None, &[]).unwrap_err();
        assert_eq!(crate::store::needs_approval(&e).unwrap().grants, ["local kit: /planted/kit", "network: x.com"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_refused_and_its_target_untouched() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("victim");
        std::fs::write(&target, "precious").unwrap();
        let link = d.path().join("sbxw.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(super::write_in_place(&link, "kits = []\n").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());

        // The old fixed temp name, planted as a symlink, is not followed.
        let real = d.path().join("real.toml");
        std::fs::write(&real, "old\n").unwrap();
        std::os::unix::fs::symlink(&target, d.path().join(".real.toml.sbxw-tmp")).unwrap();
        super::write_in_place(&real, "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 4, "no temporary file left behind: {left:?}");
    }
}
