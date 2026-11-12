//! Mirroring kits to a host: each kit is copied once per content hash
//! (`~/.sbxw/kits/<hash>/` on the host), so a sandbox there is created from
//! the same bytes as locally and an unchanged kit is never sent twice.
//!
//! sbx re-resolves a kit by its path on every container swap, so a kit must
//! stay put for the whole life of a sandbox using it. Hence the path carries
//! the content: a changed kit lands in a new directory and the old one is
//! never deleted (a sandbox may still point at it).
//!
//! Only kits under the local `~/.sbxw` are mirrored. Any other reference (a
//! path or URL on the host itself, since sbxw.toml lives there) is passed
//! through unchanged. The OAuth kit and the my-skills kit carry secrets and go
//! only to a host that is trusted with them (`trust_secrets`).
//!
//! A kit is sent as one in-memory ustar archive extracted by a single exec
//! call ([`mirror_via`]): a per-file copy costs one SSH round trip per file,
//! and agent-skills has many. Without `tar` on the host, or without an exec,
//! the per-file copy through [`HostFs`] is the fallback.

use super::HostFs;
use crate::bundled;
use sbxw_sbx::host::{HostCmd, HostExec};
use crate::store::Paths;
use anyhow::{bail, Context, Result};
use sbxw_core::kits::bundled_name;
use std::path::{Path, PathBuf};

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

/// [`mirror_via`] without an exec: every kit goes file by file through `fs`.
pub async fn mirror(
    fs: &dyn HostFs,
    local_root: &Path,
    remote_root: &Path,
    kits: &[String],
    secrets_ok: bool,
) -> Result<Mirrored> {
    mirror_via(fs, None, local_root, remote_root, kits, secrets_ok).await
}

/// Mirror `kits` to `<remote_root>/kits/<content hash>/` through `fs`, using
/// `exec` (the same host's) to send each kit as one tar stream when given.
/// `local_root` is this machine's `~/.sbxw`, `remote_root` the host's.
pub async fn mirror_via(
    fs: &dyn HostFs,
    exec: Option<&dyn HostExec>,
    local_root: &Path,
    remote_root: &Path,
    kits: &[String],
    secrets_ok: bool,
) -> Result<Mirrored> {
    let paths = Paths { root: local_root.to_path_buf(), chat_root: PathBuf::new() };
    let mut out = Mirrored::default();
    // Learned from the first extract that finds no `tar`: not asked again.
    let mut tar_ok = exec.is_some();
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
        // The marker, not the directory, means complete: a copy cut short
        // leaves a directory but no marker, and is redone.
        let marker = fs.lstat(&done).await?;
        // A mirror used again says so, at most once a day: [`prune`] spares
        // what was used lately, which covers the moment between this and the
        // sandbox showing in `sbx ls`.
        if marker.as_ref().is_some_and(|m| age_secs(m.mtime_secs) > 86_400) {
            fs.write_atomic(&done, b"", 0o644).await?;
        }
        if marker.is_none() {
            let sent = match exec {
                Some(exec) if tar_ok => {
                    let archive = build_tar(&read)?;
                    let sent = extract(exec, &remote_root.join("kits"), &h, archive).await?;
                    tar_ok = sent;
                    sent
                }
                _ => false,
            };
            if !sent {
                copy_files(fs, &read, &dest, &done).await?;
            }
            out.copied.push(dest.to_string_lossy().into_owned());
        }
        out.refs.push(dest.to_string_lossy().into_owned());
    }
    Ok(out)
}

fn age_secs(mtime: u64) -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(mtime)
}

/// A mirror's directory name: the 16 hex digits of its content hash. Nothing
/// else under `kits/` is a mirror (a host that runs its own sbxw keeps its
/// bundled kits there, by name).
fn is_mirror(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Remove the mirrored kits under `<remote_root>/kits/` that no path in
/// `in_use` names and that were not used for `unused_for`: a changed kit
/// lands in a new directory at every sbxw release, and the old one is only
/// needed while a sandbox still points at it. `in_use` must be every kit of
/// every sandbox on the host, as its sbx says: the caller gives up rather
/// than guess. The marker goes first, so a removal cut short leaves an
/// incomplete mirror, which is copied again, never a broken one that counts.
pub async fn prune(fs: &dyn HostFs, remote_root: &Path, in_use: &[String], unused_for: std::time::Duration) -> Result<Vec<String>> {
    let dir = remote_root.join("kits");
    if fs.stat(&dir).await?.is_none() {
        return Ok(vec![]);
    }
    let entries = fs.read_dir(&dir).await?;
    let mut removed = Vec::new();
    for e in entries.iter().filter(|e| e.meta.kind == super::Kind::Dir && is_mirror(&e.name)) {
        let dest = dir.join(&e.name);
        if in_use.iter().any(|k| Path::new(k) == dest) {
            continue;
        }
        let marker = format!(".{}.ok", e.name);
        // Judged on the marker's time (the last use); a mirror without one
        // is a copy cut short, judged on the directory's.
        let mtime = entries.iter().find(|m| m.name == marker).map_or(e.meta.mtime_secs, |m| m.meta.mtime_secs);
        // 0 = the host cannot say: left alone.
        if mtime == 0 || age_secs(mtime) < unused_for.as_secs() {
            continue;
        }
        fs.remove(&dir.join(&marker)).await?;
        fs.remove(&dest).await?;
        removed.push(dest.to_string_lossy().into_owned());
    }
    Ok(removed)
}

/// The per-file copy: one `HostFs` call per file.
async fn copy_files(fs: &dyn HostFs, read: &[Entry], dest: &Path, done: &Path) -> Result<()> {
    fs.create_dir_all(dest).await?;
    // Directories first, in path order, so a parent exists before its files.
    for e in read {
        let p = dest.join(&e.rel);
        if e.dir {
            fs.create_dir_all(&p).await?;
        } else {
            fs.write_atomic(&p, &e.data, e.mode).await?;
        }
    }
    fs.write_atomic(done, b"", 0o644).await
}

/// Exit code of [`EXTRACT_SCRIPT`] when the host has no `tar`.
const NO_TAR: i32 = 42;

/// `$1` = the kits directory, `$2` = the hash, archive on stdin. Unpacked
/// beside the target and renamed into place; the marker comes last (written
/// then renamed), so a copy cut anywhere leaves no marker. `-p` keeps the
/// archive's modes whatever the umask; the root is made 0755 because the
/// archive has no entry for it. A cut-short directory has no marker and is
/// replaced.
const EXTRACT_SCRIPT: &str = r#"command -v tar >/dev/null 2>&1 || exit 42
k=$1; h=$2
tmp="$k/.$h.tmp-$$"
mkdir -p -- "$k" || exit 5
rm -rf -- "$tmp"
mkdir -- "$tmp" || exit 5
trap 'rm -rf -- "$tmp" "$k/.$h.ok.tmp-$$"' EXIT
tar -xpf - -C "$tmp" || exit 6
chmod 755 "$tmp" || exit 5
rm -rf -- "$k/$h"
mv -- "$tmp" "$k/$h" || exit 7
: > "$k/.$h.ok.tmp-$$" && chmod 644 "$k/.$h.ok.tmp-$$" && mv -f -- "$k/.$h.ok.tmp-$$" "$k/.$h.ok" || exit 8
trap - EXIT
exit 0"#;

/// Unpack `archive` as `<kits_dir>/<hash>` in one exec call. `Ok(false)`: the
/// host has no `tar`, use the per-file copy.
async fn extract(exec: &dyn HostExec, kits_dir: &Path, hash: &str, archive: Vec<u8>) -> Result<bool> {
    let args = vec![
        "-c".to_string(),
        EXTRACT_SCRIPT.to_string(),
        "sh".to_string(),
        kits_dir.to_string_lossy().into_owned(),
        hash.to_string(),
    ];
    let out = exec.exec(HostCmd { stdin: Some(archive), ..HostCmd::new("sh", args) }).await?;
    match out.code {
        Some(0) => Ok(true),
        Some(NO_TAR) => Ok(false),
        code => {
            let err = String::from_utf8_lossy(&out.stderr);
            let why = err.lines().rev().find(|l| !l.trim().is_empty()).map(str::trim).map(String::from);
            bail!(
                "copying kit {hash}: {}",
                why.unwrap_or_else(|| code.map_or("timed out".to_string(), |c| format!("failed (exit {c})")))
            )
        }
    }
}

/// The tree as a POSIX ustar archive: regular files and directories only,
/// their modes kept, owner and time zeroed (the hash does not see them, and
/// neither should the host). Written by hand to avoid a dependency.
fn build_tar(files: &[Entry]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for e in files {
        let name = if e.dir { format!("{}/", e.rel) } else { e.rel.clone() };
        let (prefix, short) = split_name(&name).with_context(|| format!("{}: path too long for a tar archive", e.rel))?;
        let mut h = [0u8; 512];
        put(&mut h[0..100], short.as_bytes());
        put_octal(&mut h[100..108], u64::from(e.mode & 0o7777));
        put_octal(&mut h[108..116], 0);
        put_octal(&mut h[116..124], 0);
        put_octal(&mut h[124..136], e.data.len() as u64);
        put_octal(&mut h[136..148], 0);
        h[148..156].fill(b' ');
        h[156] = if e.dir { b'5' } else { b'0' };
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        put(&mut h[345..500], prefix.as_bytes());
        let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
        h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        h[154] = 0;
        out.extend_from_slice(&h);
        out.extend_from_slice(&e.data);
        out.resize(out.len().next_multiple_of(512), 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    Ok(out)
}

fn put(field: &mut [u8], v: &[u8]) {
    field[..v.len()].copy_from_slice(v);
}

/// `v` as zero-padded octal filling the field, last byte NUL.
fn put_octal(field: &mut [u8], v: u64) {
    let w = field.len() - 1;
    field[..w].copy_from_slice(format!("{v:0w$o}").as_bytes());
}

/// ustar keeps 100 bytes of name and 155 of prefix, split at a `/`.
fn split_name(name: &str) -> Option<(&str, &str)> {
    if name.len() <= 100 {
        return Some(("", name));
    }
    let t = name.trim_end_matches('/');
    name.char_indices()
        .filter(|(i, c)| *c == '/' && *i < t.len())
        .map(|(i, _)| (&name[..i], &name[i + 1..]))
        .find(|(p, n)| p.len() <= 155 && n.len() <= 100)
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
        let first = run(&r, std::slice::from_ref(&k), false).await.unwrap();
        assert_eq!(first.copied, first.refs);
        let dir = PathBuf::from(&first.refs[0]);
        assert!(dir.starts_with(r.remote.join("kits")));
        assert_eq!(std::fs::read_to_string(dir.join("spec.yaml")).unwrap(), "name: a\n");
        assert_eq!(std::fs::metadata(dir.join("bin/run.sh")).unwrap().permissions().mode() & 0o7777, 0o755);

        let again = run(&r, std::slice::from_ref(&k), false).await.unwrap();
        assert_eq!(again.refs, first.refs);
        assert!(again.copied.is_empty(), "unchanged: nothing sent");

        std::fs::write(r.local.join("kits/mine/spec.yaml"), "name: b\n").unwrap();
        let changed = run(&r, &[k], false).await.unwrap();
        assert_ne!(changed.refs, first.refs);
        assert_eq!(changed.copied, changed.refs);
        assert_eq!(std::fs::read_to_string(dir.join("spec.yaml")).unwrap(), "name: a\n", "the old one stays");
    }

    #[tokio::test]
    async fn only_mirrors_nobody_uses_and_nobody_used_lately_are_pruned() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "name: a\n", 0o644)]);
        let old = run(&r, std::slice::from_ref(&k), false).await.unwrap().refs.remove(0);
        std::fs::write(r.local.join("kits/mine/spec.yaml"), "name: b\n").unwrap();
        let new = run(&r, &[k], false).await.unwrap().refs.remove(0);
        // What a host running its own sbxw keeps beside the mirrors.
        std::fs::create_dir_all(r.remote.join("kits/codemap")).unwrap();
        let week = std::time::Duration::from_secs(7 * 86_400);
        let none: Vec<String> = prune(&LocalFs, &r.remote, std::slice::from_ref(&new), week).await.unwrap();
        assert!(none.is_empty(), "used a moment ago: spared, in use or not");
        // Both last used ten days ago; a sandbox still points at the new one.
        let ten_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86_400);
        for dir in [&old, &new] {
            let name = Path::new(dir).file_name().unwrap().to_string_lossy().into_owned();
            let marker = r.remote.join("kits").join(format!(".{name}.ok"));
            std::fs::File::options().write(true).open(&marker).unwrap().set_modified(ten_days_ago).unwrap();
        }
        assert_eq!(prune(&LocalFs, &r.remote, std::slice::from_ref(&new), week).await.unwrap(), std::slice::from_ref(&old));
        assert!(!Path::new(&old).exists() && Path::new(&new).join("spec.yaml").is_file());
        assert!(r.remote.join("kits/codemap").is_dir(), "not a mirror: never touched");
        // Used again, the old marker is renewed, so a prune racing a create spares it.
        std::fs::write(r.local.join("kits/mine/spec.yaml"), "name: b\n").unwrap();
        let again = run(&r, &[r.local.join("kits/mine").to_string_lossy().into_owned()], false).await.unwrap();
        assert!(again.copied.is_empty());
        assert!(prune(&LocalFs, &r.remote, &[], week).await.unwrap().is_empty(), "its last use is now");
    }

    #[tokio::test]
    async fn an_interrupted_copy_is_redone() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "x", 0o644)]);
        let first = run(&r, std::slice::from_ref(&k), false).await.unwrap();
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
        let oauth = kit(&r.local, "state/kits/myapp-oauth", &[("spec.yaml", "setup: {}\n", 0o600)]);
        let skills = kit(&r.local, "state/kits/sbxw-my-skills", &[("spec.yaml", "name: s\n", 0o644)]);
        // Renamed, but still writes the credentials file.
        let renamed = kit(&r.local, "state/kits/other", &[("spec.yaml", "path: /home/agent/.claude/.credentials.json", 0o644)]);
        for k in [&oauth, &skills, &renamed] {
            let e = run(&r, std::slice::from_ref(k), false).await.unwrap_err().to_string();
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
        assert!(spec.contains("displayName: codemap"));
        assert!(run(&r, &["sbxw:nope".into()], false).await.is_err());
    }

    #[tokio::test]
    async fn a_link_inside_a_kit_is_refused() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "x", 0o644)]);
        std::os::unix::fs::symlink("/etc/passwd", r.local.join("kits/mine/l")).unwrap();
        assert!(format!("{:#}", run(&r, &[k], true).await.unwrap_err()).contains("symbolic link"));
    }

    use async_trait::async_trait;
    use sbxw_sbx::host::LocalExec;
    use sbxw_sbx::runner::Output;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts exec calls; `no_tar` answers the way a host without tar does.
    struct Counting {
        calls: AtomicUsize,
        no_tar: bool,
    }

    #[async_trait]
    impl HostExec for Counting {
        async fn exec(&self, cmd: HostCmd) -> Result<Output> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.no_tar {
                return Ok(Output { code: Some(NO_TAR), stdout: vec![], stderr: vec![] });
            }
            LocalExec.exec(cmd).await
        }
        fn tty_argv(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
            LocalExec.tty_argv(program, args)
        }
        async fn home(&self) -> Result<PathBuf> {
            LocalExec.home().await
        }
    }

    fn mode_of(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn the_archive_round_trips_through_the_system_tar() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        kit(d.path(), "src", &[
            ("my spec.yaml", "name: a\n", 0o600),
            ("bin/run.sh", "#!/bin/sh\n", 0o755),
            ("deep/er/still/empty", "", 0o644),
        ]);
        std::fs::set_permissions(src.join("deep"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let tar = build_tar(&read_tree(&src).unwrap()).unwrap();
        assert_eq!(tar.len() % 512, 0);
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let mut c = std::process::Command::new("tar")
            .args(["-xpf", "-", "-C"])
            .arg(&out)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(c.stdin.as_mut().unwrap(), &tar).unwrap();
        assert!(c.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(out.join("my spec.yaml")).unwrap(), "name: a\n");
        assert_eq!(mode_of(&out.join("my spec.yaml")), 0o600);
        assert_eq!(mode_of(&out.join("bin/run.sh")), 0o755);
        assert_eq!(std::fs::metadata(out.join("deep/er/still/empty")).unwrap().len(), 0);
        assert_eq!(mode_of(&out.join("deep")), 0o700);
    }

    #[test]
    fn long_names_use_the_prefix_field_and_too_long_ones_are_refused() {
        let seg = "d".repeat(60);
        let long = format!("{seg}/{seg}/{seg}/f.txt");
        let e = |rel: String| Entry { rel, dir: false, mode: 0o644, data: b"x".to_vec() };
        let d = tempfile::tempdir().unwrap();
        let tar = build_tar(&[e(long.clone())]).unwrap();
        let mut c = std::process::Command::new("tar")
            .args(["-xf", "-", "-C"])
            .arg(d.path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(c.stdin.as_mut().unwrap(), &tar).unwrap();
        assert!(c.wait().unwrap().success());
        assert!(d.path().join(&long).is_file());
        assert!(build_tar(&[e("x".repeat(300))]).is_err());
    }

    #[tokio::test]
    async fn one_kit_copy_is_one_exec_call_and_keeps_directory_modes() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "a", 0o600), ("a b/c.sh", "#!/bin/sh\n", 0o755), ("a b/e", "", 0o644)]);
        std::fs::set_permissions(r.local.join("kits/mine/a b"), std::fs::Permissions::from_mode(0o750)).unwrap();
        let exec = Counting { calls: AtomicUsize::new(0), no_tar: false };
        let m = mirror_via(&LocalFs, Some(&exec), &r.local, &r.remote, std::slice::from_ref(&k), false).await.unwrap();
        assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
        let dir = PathBuf::from(&m.refs[0]);
        assert_eq!(mode_of(&dir.join("spec.yaml")), 0o600);
        assert_eq!(mode_of(&dir.join("a b/c.sh")), 0o755);
        assert_eq!(mode_of(&dir.join("a b")), 0o750);
        let h = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(r.remote.join("kits").join(format!(".{h}.ok")).exists());
        // Nothing is left beside it, and an unchanged kit costs no call.
        let names: Vec<_> = std::fs::read_dir(r.remote.join("kits")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(mirror_via(&LocalFs, Some(&exec), &r.local, &r.remote, &[k], false).await.unwrap().copied.is_empty());
        assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn without_tar_the_files_are_copied_one_by_one_and_tar_is_not_asked_again() {
        let r = rig();
        let a = kit(&r.local, "kits/a", &[("spec.yaml", "a", 0o644), ("bin/x", "x", 0o755)]);
        let b = kit(&r.local, "kits/b", &[("spec.yaml", "b", 0o644)]);
        let exec = Counting { calls: AtomicUsize::new(0), no_tar: true };
        let m = mirror_via(&LocalFs, Some(&exec), &r.local, &r.remote, &[a, b], false).await.unwrap();
        assert_eq!(exec.calls.load(Ordering::SeqCst), 1, "detected once");
        assert_eq!(m.copied.len(), 2);
        assert_eq!(mode_of(&PathBuf::from(&m.refs[0]).join("bin/x")), 0o755);
        assert_eq!(std::fs::read_to_string(PathBuf::from(&m.refs[1]).join("spec.yaml")).unwrap(), "b");
    }

    #[tokio::test]
    async fn an_interrupted_extract_leaves_no_marker_and_no_leftovers() {
        let r = rig();
        let k = kit(&r.local, "kits/mine", &[("spec.yaml", "x".repeat(5000).as_str(), 0o644)]);
        let read = read_tree(Path::new(&k)).unwrap();
        let mut tar = build_tar(&read).unwrap();
        tar.truncate(700); // cut inside the file's data
        let kits = r.remote.join("kits");
        let err = extract(&LocalExec, &kits, "abc", tar).await.unwrap_err().to_string();
        assert!(err.contains("copying kit abc"), "{err}");
        let left: Vec<_> = std::fs::read_dir(&kits).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert!(left.is_empty(), "{left:?}");
        // A good run afterwards heals it.
        assert!(extract(&LocalExec, &kits, "abc", build_tar(&read).unwrap()).await.unwrap());
        assert!(kits.join(".abc.ok").exists() && kits.join("abc/spec.yaml").exists());
    }
}
