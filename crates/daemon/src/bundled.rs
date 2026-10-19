//! The kits that ship inside the binary, and where they are written out.
//!
//! A reference `sbxw:<name>` in `kits` is one of these. sbx applies a kit
//! from a directory and keeps its *path*, re-resolving it on every container
//! swap, so each is written to a stable `~/.sbxw/kits/<name>/spec.yaml` — and
//! rewritten only when this build's spec differs, so a path sbx holds never
//! goes missing under it.

use crate::store::{write_atomic, Paths};
use anyhow::{anyhow, Result};
use sbxw_core::kits::bundled_name;

const SPECS: &[(&str, &str)] = &[
    ("codemap", include_str!("../../../assets/codemap/spec.yaml")),
    ("agent-skills", include_str!("../../../assets/agent-skills/spec.yaml")),
    ("md-to-pdf-tools", include_str!("../../../assets/md-to-pdf-tools/spec.yaml")),
    ("k8s-tools", include_str!("../../../assets/k8s-tools/spec.yaml")),
];

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

fn write_out(paths: &Paths, name: &str) -> Result<String> {
    let (_, spec) = SPECS
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| anyhow!("no bundled kit named '{name}'"))?;
    let dir = paths.kits().join(name);
    let file = dir.join("spec.yaml");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(*spec) {
        write_atomic(&file, spec.as_bytes())?;
        // Readable by sbx; the spec holds no secret.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644));
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(dir.to_string_lossy().into_owned())
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
        assert!(std::fs::read_to_string(&spec).unwrap().contains("name: codemap"));
        let before = std::fs::metadata(&spec).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        resolve(&p, &kits).unwrap();
        assert_eq!(std::fs::metadata(&spec).unwrap().modified().unwrap(), before, "unchanged, not rewritten");
        assert!(resolve(&p, &["sbxw:nope".into()]).is_err());
    }

    #[test]
    fn every_bundled_name_has_a_spec() {
        for name in sbxw_core::kits::BUNDLED {
            assert!(SPECS.iter().any(|(n, _)| *n == name), "{name}");
        }
    }
}
