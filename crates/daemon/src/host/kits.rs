//! Mirroring kits to a host: each kit is copied once per content hash
//! (`~/.sbxw/kits/<hash>/` on the host), so a sandbox there is created from
//! the same bytes as locally and an unchanged kit is never sent twice.
//!
//! sbx re-resolves a kit by its path on every container swap, so a kit a
//! sandbox uses must stay where it is for that sandbox's whole life. Hence
//! the path carries the content: a changed kit lands in a new directory and
//! the old one is never deleted (a sandbox may still point at it).
//!
//! Only kits under the local `~/.sbxw` are mirrored. Any other reference (a
//! path or URL on the host itself, since sbxw.toml lives there) is passed
//! through unchanged. The OAuth kit and the my-skills kit carry secrets and go
//! only to a host that is trusted with them (`trust_secrets`).

use super::HostFs;
use crate::bundled;
use crate::store::Paths;
use anyhow::{bail, Context, Result};
use sbxw_core::kits::bundled_name;
use std::path::{Path, PathBuf};

/// What [`mirror`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mirrored {
    /// What to pass to `sbx create --kit` on the host, in the order asked.
    pub refs: Vec<String>,
    /// The refs that were sent just now (the others were already there).
    pub copied: Vec<String>,
}

/// One file of a kit, read from this machine.
struct Entry {
    rel: String,
    dir: bool,
    mode: u32,
    data: Vec<u8>,
}

/// Mirror `kits` to `<remote_root>/kits/<content hash>/` through `fs`.
/// `local_root` is this machine's `~/.sbxw`, `remote_root` the host's.
pub async fn mirror(
    fs: &dyn HostFs,
    local_root: &Path,
    remote_root: &Path,
    kits: &[String],
    secrets_ok: bool,
) -> Result<Mirrored> {
    let paths = Paths { root: local_root.to_path_buf(), chat_root: PathBuf::new() };
    let mut out = Mirrored::default();
    for kit in kits {
        // `sbxw:<name>` is written out under the local root first.
        let local = match bundled_name(kit) {
            Some(name) => Some(PathBuf::from(bundled::resolve(&paths, &[format!("sbxw:{name}")])?.remove(0))),
            None => under(local_root, kit),
        };
        let Some(local) = local else {
            out.refs.push(kit.clone());
            continue;
        };
        let read = {
            let dir = local.clone();
            tokio::task::spawn_blocking(move || read_tree(&dir))
                .await
                .context("kit read panicked")?
                .with_context(|| format!("reading kit {}", local.display()))?
        };
        if !secrets_ok && carries_secrets(&local, &read) {
            bail!(
                "kit {} carries credentials and goes only to a host with `trust_secrets = true` \
                 (sbxw host edit <host> --trust-secrets)",
                local.display()
            );
        }
        let h = hash(&read);
        let dest = remote_root.join("kits").join(&h);
        let done = remote_root.join("kits").join(format!(".{h}.ok"));
        // The marker, not the directory, says "complete": a copy cut short
        // leaves a directory but no marker, and is redone.
        if fs.lstat(&done).await?.is_none() {
            fs.create_dir_all(&dest).await?;
            // Directories first, in path order, so a parent exists before its files.
            for e in &read {
                let p = dest.join(&e.rel);
                if e.dir {
                    fs.create_dir_all(&p).await?;
                } else {
                    fs.write_atomic(&p, &e.data, e.mode).await?;
                }
            }
            fs.write_atomic(&done, b"", 0o644).await?;
            out.copied.push(dest.to_string_lossy().into_owned());
        }
        out.refs.push(dest.to_string_lossy().into_owned());
    }
    Ok(out)
}

/// `kit` as a directory under `root`, or `None` when it is anything else.
fn under(root: &Path, kit: &str) -> Option<PathBuf> {
    let p = Path::new(kit);
    if !p.is_absolute() {
        return None;
    }
    let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // `..` and links must not carry a path out of the root, so compare real paths.
    let real = std::fs::canonicalize(p).ok()?;
    (real.starts_with(&canon_root) && real.is_dir()).then_some(real)
}

fn carries_secrets(dir: &Path, files: &[Entry]) -> bool {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let by_name = name == sbxw_core::my_skills::KIT_NAME || name.ends_with("-oauth");
    // A renamed copy is still the OAuth kit: look at what it writes.
    let by_content = files
        .iter()
        .any(|e| !e.dir && String::from_utf8_lossy(&e.data).contains("/.claude/.credentials.json"));
    by_name || by_content
}

/// [`sbxw_core::templates::fingerprint`] over the tree: paths, modes and
/// bytes, so a changed file, a changed mode and a renamed file all move it.
fn hash(files: &[Entry]) -> String {
    let parts: Vec<String> = files
        .iter()
        .map(|e| {
            let hex: String = e.data.iter().map(|b| format!("{b:02x}")).collect();
            format!("{}\u{1}{}\u{1}{:o}\u{1}{hex}", e.rel, e.dir, e.mode)
        })
        .collect();
    sbxw_core::templates::fingerprint(&parts)
}

/// Every file and directory under `dir`, sorted by path. A link inside a kit
/// is an error: it cannot be recreated through [`HostFs`], and copying what it
/// points at would silently change the kit.
fn read_tree(dir: &Path) -> Result<Vec<Entry>> {
    fn go(dir: &Path, rel: &str, out: &mut Vec<Entry>) -> Result<()> {
        let mut names: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        names.sort_by_key(|e| e.file_name());
        for e in names {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let meta = e.metadata()?;
            let t = e.file_type()?;
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o7777
            };
            #[cfg(not(unix))]
            let mode = 0o644;
            if t.is_symlink() {
                bail!("{rel} is a symbolic link; kits are mirrored as plain files");
            } else if t.is_dir() {
                out.push(Entry { rel: rel.clone(), dir: true, mode, data: vec![] });
                go(&e.path(), &rel, out)?;
            } else if t.is_file() {
                out.push(Entry { rel, dir: false, mode, data: std::fs::read(e.path())? });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(dir, "", &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::LocalFs;
    use std::os::unix::fs::PermissionsExt;

    struct Rig {
        _d: tempfile::TempDir,
        local: PathBuf,
        remote: PathBuf,
    }

    fn rig() -> Rig {
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("local/.sbxw");
        let remote = d.path().join("remote/.sbxw");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        Rig { local, remote, _d: d }
    }

    fn kit(root: &Path, rel: &str, files: &[(&str, &str, u32)]) -> String {
        let dir = root.join(rel);
        for (name, body, mode) in files {
            let f = dir.join(name);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, body).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(*mode)).unwrap();
        }
        dir.to_string_lossy().into_owned()
    }

    async fn run(r: &Rig, kits: &[String], secrets: bool) -> Result<Mirrored> {
        mirror(&LocalFs, &r.local, &r.remote, kits, secrets).await
    }

    #[tokio::test]
    async fn a_second_run_copies_nothing_and_a_changed_kit_gets_a_new_directory() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "name: a\n", 0o644), ("bin/run.sh", "#!/bin/sh\n", 0o755)]);
        let first = run(&r, &[k.clone()], false).await.unwrap();
        assert_eq!(first.copied, first.refs);
        let dir = PathBuf::from(&first.refs[0]);
        assert!(dir.starts_with(r.remote.join("kits")));
        assert_eq!(std::fs::read_to_string(dir.join("spec.yaml")).unwrap(), "name: a\n");
        assert_eq!(std::fs::metadata(dir.join("bin/run.sh")).unwrap().permissions().mode() & 0o7777, 0o755);

        let again = run(&r, &[k.clone()], false).await.unwrap();
        assert_eq!(again.refs, first.refs);
        assert!(again.copied.is_empty(), "unchanged: nothing sent");

        std::fs::write(r.local.join("kits/mine/spec.yaml"), "name: b\n").unwrap();
        let changed = run(&r, &[k], false).await.unwrap();
        assert_ne!(changed.refs, first.refs);
        assert_eq!(changed.copied, changed.refs);
        assert_eq!(std::fs::read_to_string(dir.join("spec.yaml")).unwrap(), "name: a\n", "the old one stays");
    }

    #[tokio::test]
    async fn an_interrupted_copy_is_redone() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "x", 0o644)]);
        let first = run(&r, &[k.clone()], false).await.unwrap();
        let dir = PathBuf::from(&first.refs[0]);
        let marker = r.remote.join("kits").join(format!(".{}.ok", dir.file_name().unwrap().to_string_lossy()));
        std::fs::remove_file(&marker).unwrap();
        std::fs::remove_file(dir.join("spec.yaml")).unwrap();
        assert_eq!(run(&r, &[k], false).await.unwrap().copied.len(), 1);
        assert!(dir.join("spec.yaml").exists());
    }

    #[tokio::test]
    async fn secret_kits_need_trust_and_keep_0600() {
        let r = rig();
        let oauth = kit(&r.local, "state/kits/neos-oauth", &[("spec.yaml", "setup: {}\n", 0o600)]);
        let skills = kit(&r.local, "state/kits/sbxw-my-skills", &[("spec.yaml", "name: s\n", 0o644)]);
        // Renamed, but still writes the credentials file.
        let renamed = kit(&r.local, "state/kits/other", &[("spec.yaml", "path: /home/agent/.claude/.credentials.json", 0o644)]);
        for k in [&oauth, &skills, &renamed] {
            let e = run(&r, &[k.clone()], false).await.unwrap_err().to_string();
            assert!(e.contains("trust_secrets"), "{e}");
        }
        assert!(!r.remote.join("kits").exists(), "nothing was sent");
        let ok = run(&r, &[oauth], true).await.unwrap();
        let f = PathBuf::from(&ok.refs[0]).join("spec.yaml");
        assert_eq!(std::fs::metadata(f).unwrap().permissions().mode() & 0o7777, 0o600);
        run(&r, &[skills], true).await.unwrap();
    }

    #[tokio::test]
    async fn other_references_pass_through_in_order() {
        let r = rig();
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = kit(outside.path(), "k", &[("spec.yaml", "x", 0o644)]);
        let mine = kit(&r.local, "kits/mine", &[("spec.yaml", "y", 0o644)]);
        let escape = format!("{}/../../{}", r.local.display(), outside.path().strip_prefix("/").unwrap().join("k").display());
        let kits = vec![
            "docker.io/sbx/k:1".to_string(),
            elsewhere.clone(),
            mine,
            "git+https://example.com/k.git".to_string(),
            "./relative".to_string(),
            escape.clone(),
        ];
        let m = run(&r, &kits, false).await.unwrap();
        assert_eq!(m.refs.len(), 6);
        assert_eq!([&m.refs[0], &m.refs[1], &m.refs[3], &m.refs[4], &m.refs[5]], [&kits[0], &kits[1], &kits[3], &kits[4], &kits[5]]);
        assert!(m.refs[2].starts_with(&r.remote.join("kits").to_string_lossy().into_owned()));
        assert_eq!(m.copied, [m.refs[2].clone()]);
    }

    #[tokio::test]
    async fn bundled_references_are_written_out_then_mirrored() {
        let r = rig();
        let m = run(&r, &["sbxw:codemap".into()], false).await.unwrap();
        let spec = std::fs::read_to_string(PathBuf::from(&m.refs[0]).join("spec.yaml")).unwrap();
        assert!(spec.contains("name: codemap"));
        assert!(run(&r, &["sbxw:nope".into()], false).await.is_err());
    }

    #[tokio::test]
    async fn a_link_inside_a_kit_is_refused() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "x", 0o644)]);
        std::os::unix::fs::symlink("/etc/passwd", r.local.join("kits/mine/l")).unwrap();
        assert!(format!("{:#}", run(&r, &[k], true).await.unwrap_err()).contains("symbolic link"));
    }
}
