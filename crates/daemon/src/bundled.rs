//! The kits embedded in the binary by `build.rs`: what `sbxw:<name>` in `kits`
//! refers to. Each is a kit spec v3 directory — `spec.yaml` and the `files/`
//! tree its `build:` copies into the image. sbx applies a kit from a directory
//! and keeps its *path*, so each is written to a stable `~/.sbxw/kits/<name>/`
//! and a file is rewritten only when this build's copy differs: the path never
//! goes missing under sbx, and an unchanged tree keeps its build cache warm.

use crate::store::{write_atomic, Paths};
use anyhow::{anyhow, Context, Result};
use sbxw_core::kits::bundled_name;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/bundled_kits.rs"));

/// The kits that fill the shared cache, in the order they are applied.
pub const CACHING: [&str; 2] = ["agent-skills", "md-to-pdf-tools"];

/// `kits` with every `sbxw:<name>` replaced by its written-out directory.
pub fn resolve(paths: &Paths, kits: &[String]) -> Result<Vec<String>> {
    kits.iter()
        .map(|k| match bundled_name(k) {
            None => Ok(k.clone()),
            Some(name) => write_out(paths, name),
        })
        .collect()
}

/// [`sbxw_core::templates::fingerprint`] of the kits `kits` resolves to: each
/// local kit's content, else the reference itself.
pub fn fingerprint(paths: &Paths, kits: &[String]) -> Result<String> {
    let texts: Vec<String> = resolve(paths, kits)?.iter().map(|k| sbxw_core::kits::content_text(k)).collect();
    Ok(sbxw_core::templates::fingerprint(&texts))
}

fn write_out(paths: &Paths, name: &str) -> Result<String> {
    let (_, files) = KITS.iter().find(|(n, _)| *n == name).ok_or_else(|| anyhow!("no bundled kit named '{name}'"))?;
    let dir = paths.kits().join(name);
    for (rel, mode, bytes) in *files {
        let file = dir.join(rel);
        if std::fs::read(&file).ok().as_deref() != Some(*bytes) {
            write_atomic(&file, bytes).with_context(|| format!("writing the bundled kit '{name}'"))?;
        }
        set_mode(&file, *mode);
    }
    prune(&dir.join("files"), &dir, files);
    // Readable by sbx and its builder; nothing here is a secret.
    set_tree_mode(&dir);
    Ok(dir.to_string_lossy().into_owned())
}

/// Remove files under `files/` that this build no longer ships: the kit build
/// copies the whole tree, so a stale file would land in the sandbox.
fn prune(at: &Path, root: &Path, keep: &[(&str, u32, &[u8])]) {
    let Ok(entries) = std::fs::read_dir(at) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            prune(&p, root, keep);
            let _ = std::fs::remove_dir(&p); // only when empty
        } else {
            let rel = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if !keep.iter().any(|(k, _, _)| *k == rel) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

fn set_tree_mode(dir: &Path) {
    set_mode(dir, 0o755);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                set_tree_mode(&e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_references_are_written_once_at_a_stable_path() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        let kits = vec!["sbxw:codemap".to_string(), "/abs/other".to_string()];
        let r = resolve(&p, &kits).unwrap();
        assert_eq!(r[1], "/abs/other");
        let spec = p.kits().join("codemap/spec.yaml");
        assert!(std::fs::read_to_string(&spec).unwrap().contains("displayName: codemap"));
        let script = p.kits().join("codemap/files/home/agent/.sbxw/codemap/install-hooks.mjs");
        assert!(script.is_file());
        let before = std::fs::metadata(&spec).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        resolve(&p, &kits).unwrap();
        assert_eq!(std::fs::metadata(&spec).unwrap().modified().unwrap(), before, "unchanged, not rewritten");
        assert!(resolve(&p, &["sbxw:nope".into()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn scripts_keep_their_mode_and_stale_files_go() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        let stale = p.kits().join("retro/files/home/agent/.claude/commands/gone.md");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, "old").unwrap();
        resolve(&p, &["sbxw:retro".into()]).unwrap();
        assert!(!stale.exists(), "a file this build does not ship is removed");
        let digest = p.kits().join("retro/files/home/agent/.sbxw/retro/digest.mjs");
        assert_eq!(std::fs::metadata(&digest).unwrap().permissions().mode() & 0o777, 0o755);
        let cmd = p.kits().join("retro/files/home/agent/.claude/commands/retro.md");
        assert_eq!(std::fs::metadata(&cmd).unwrap().permissions().mode() & 0o777, 0o644);
    }

    #[test]
    fn the_base_kit_carries_the_relay_guidance_the_tooling_writes() {
        let (_, files) = KITS.iter().find(|(n, _)| *n == sbxw_core::kits::BASE).unwrap();
        let (_, _, spec) = files.iter().find(|(r, _, _)| *r == "spec.yaml").unwrap();
        let doc: serde_norway::Value = serde_norway::from_str(std::str::from_utf8(spec).unwrap()).unwrap();
        let ctx = doc["capabilities"].as_sequence().unwrap().iter().find(|c| c["type"] == "com.docker.sandbox/agent-context@2").expect("agent-context@2");
        assert_eq!(ctx["config"]["content"].as_str(), Some(sbxw_core::tooling::relay_guidance().as_str()), "edit both, or neither");
    }

    #[test]
    fn every_bundled_kit_is_embedded_and_v3() {
        for name in sbxw_core::kits::BUNDLED.iter().chain([&sbxw_core::kits::BASE]) {
            let (_, files) = KITS.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name}"));
            let (_, _, spec) = files.iter().find(|(r, _, _)| *r == "spec.yaml").unwrap();
            let spec = std::str::from_utf8(spec).unwrap();
            assert!(spec.starts_with("# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\n"), "{name}: v3 only since sbx 0.51");
            let doc: serde_norway::Value = serde_norway::from_str(spec).unwrap_or_else(|e| panic!("{name}: {e}"));
            // What the build copies is what is embedded.
            if let Some(build) = doc.get("build").and_then(|b| b.as_str()) {
                assert!(build.contains("COPY --chown=1000:1000 files/home/agent/ /home/agent/"), "{name}");
                assert!(files.iter().any(|(r, _, _)| r.starts_with("files/home/agent/")), "{name}: a build with nothing to copy");
            }
        }
    }
}
