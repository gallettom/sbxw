//! [`HostFs`](super::HostFs) over any [`HostExec`](sbxw_sbx::host::HostExec):
//! each method is one POSIX `sh` script run on the host (no bash, no GNU-only
//! flags), paths passed as arguments rather than spliced into the script, and
//! file contents on stdin.
//!
//! SSH uses this path; the contract tests run it over `LocalExec`, beside
//! `LocalFs`, so it is tested without SSH.
//!
//! Metadata comes from `stat`, which POSIX does not specify: GNU (`-c`) and
//! BSD/macOS (`-f`) differ, so a script picks its format once ([`PRELUDE`]).
//! Both print the raw mode in hex, so the file type is read here rather than
//! from a locale-dependent word. A name holding a newline cannot be told apart
//! from two records and is skipped.

use super::{DirEntry, HostFs, Kind, Meta};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use sbxw_sbx::host::{HostCmd, HostExec};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A metadata call is a few stats; a host that takes longer is not answering.
/// Reads and writes move file contents, so they are not bounded by time.
const QUICK: Duration = Duration::from_secs(60);

/// Sets `$F`, `stat`'s arguments printing `mode(hex)|size|mtime|name`.
const PRELUDE: &str = r#"if stat -c %f / >/dev/null 2>&1; then F='-c %f|%s|%Y|%n'; else F='-f %Xp|%z|%m|%N'; fi
"#;

/// `$1`, followed (`$2` = F) or not (L); exit 10 when absent. A dangling link
/// is absent to a following stat, as `stat(2)` says.
const STAT_SCRIPT: &str = r#"if [ "$2" = F ]; then [ -e "$1" ] || exit 10; L=-L
else [ -e "$1" ] || [ -L "$1" ] || exit 10; L=''; fi
stat $L $F -- "$1" || exit 3
exit 0"#;

/// Everything under `$1` down to `$2` levels except entries named in `$3`…,
/// not descending into those; `find` never follows links. Paths are printed
/// relative to `$1`, as `./a/b`. Exit 3 when `$1` is not a directory.
const WALK_SCRIPT: &str = r#"root=$1; depth=$2; shift 2
[ -d "$root" ] || { echo "not a directory" >&2; exit 3; }
cd -- "$root" 2>/dev/null || { echo "not readable" >&2; exit 3; }
n=$#
for s in "$@"; do set -- "$@" -name "$s" -o; done
shift "$n"
find . -mindepth 1 -maxdepth "$depth" \( "$@" ! -name '*' \) -prune -o -exec stat $F {} + 2>/dev/null
exit 0"#;

/// A file's bytes, refused past `$2`. `wc -c <` is POSIX; BSD pads its number.
const READ_SCRIPT: &str = r#"[ -e "$1" ] || { echo "not found" >&2; exit 3; }
s=$(wc -c < "$1" 2>/dev/null) || { echo "not readable" >&2; exit 3; }
s=$(echo $s)
[ "$s" -le "$2" ] || { echo "longer than $2 bytes" >&2; exit 4; }
exec cat -- "$1""#;

/// Stdin to a temporary file beside `$1`, mode `$2`, renamed over it. The
/// umask keeps the file private until `chmod` sets what was asked.
const WRITE_SCRIPT: &str = r#"f=$1; mode=$2
[ -d "$f" ] && { echo "is a directory" >&2; exit 6; }
d=$(dirname -- "$f"); b=$(basename -- "$f")
tmp="$d/.$b.tmp-$$"
umask 077
trap 'rm -f -- "$tmp"' EXIT
cat > "$tmp" && chmod "$mode" "$tmp" && mv -f -- "$tmp" "$f" || exit 5
trap - EXIT
exit 0"#;

/// `realpath` is not POSIX and `readlink -f` is not on every host, so links are
/// followed by hand and the directory part resolved with `cd -P`.
const CANON_SCRIPT: &str = r#"p=$1
[ -e "$p" ] || { echo "not found" >&2; exit 3; }
n=0
while [ -L "$p" ]; do
  n=$((n+1)); [ "$n" -le 40 ] || { echo "too many levels of symbolic links" >&2; exit 3; }
  t=$(readlink -- "$p") || exit 3
  case "$t" in /*) p=$t ;; *) p="$(dirname -- "$p")/$t" ;; esac
done
if [ -d "$p" ]; then cd -P -- "$p" && pwd -P || exit 3
else
  d=$(cd -P -- "$(dirname -- "$p")" && pwd -P) || exit 3
  printf '%s/%s\n' "${d%/}" "$(basename -- "$p")"
fi"#;

#[derive(Clone)]
pub struct ExecFs {
    pub exec: Arc<dyn HostExec>,
}

impl ExecFs {
    pub fn new(exec: Arc<dyn HostExec>) -> Self {
        Self { exec }
    }

    async fn sh(&self, script: &str, args: &[&str], stdin: Option<Vec<u8>>, timeout: Option<Duration>) -> Result<sbxw_sbx::runner::Output> {
        let mut a = vec!["-c".to_string(), format!("{PRELUDE}{script}"), "sh".to_string()];
        a.extend(args.iter().map(|s| s.to_string()));
        self.exec.exec(HostCmd { stdin, timeout, ..HostCmd::new("sh", a) }).await
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// What the script said on stderr (its last line), for an error message.
fn why(out: &sbxw_sbx::runner::Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr);
    err.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_else(|| match out.code {
        Some(c) => format!("failed (exit {c})"),
        None => "timed out".to_string(),
    })
}

fn glob_escape(name: &str) -> String {
    let mut o = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn kind_of_mode(hex: &str) -> Option<Kind> {
    Some(match u32::from_str_radix(hex, 16).ok()? & 0o170000 {
        0o100000 => Kind::File,
        0o040000 => Kind::Dir,
        0o120000 => Kind::Symlink,
        _ => Kind::Other,
    })
}

/// `mode|size|mtime|name`, name last because it may hold `|`.
fn parse_record(line: &str) -> Option<(String, Meta)> {
    let mut f = line.splitn(4, '|');
    let kind = kind_of_mode(f.next()?)?;
    let len = f.next()?.parse().ok()?;
    let mtime_secs = f.next()?.parse().unwrap_or(0);
    let name = f.next()?.to_string();
    Some((name, Meta { kind, len, mtime_secs }))
}

#[async_trait]
impl HostFs for ExecFs {
    async fn stat(&self, p: &Path) -> Result<Option<Meta>> {
        self.stat_with(p, true).await
    }

    async fn lstat(&self, p: &Path) -> Result<Option<Meta>> {
        self.stat_with(p, false).await
    }

    async fn read_dir(&self, p: &Path) -> Result<Vec<DirEntry>> {
        let all = self.walk(p, 1, &[]).await?;
        Ok(all.into_iter().map(|(name, meta)| DirEntry { name, meta }).collect())
    }

    async fn read(&self, p: &Path, max: u64) -> Result<Vec<u8>> {
        let out = self.sh(READ_SCRIPT, &[&s(p), &max.to_string()], None, None).await?;
        if !out.success() {
            bail!("{}: {}", p.display(), why(&out));
        }
        Ok(out.stdout)
    }

    async fn write_atomic(&self, p: &Path, data: &[u8], mode: u32) -> Result<()> {
        if p.file_name().is_none() {
            bail!("{}: no file name", p.display());
        }
        let out = self.sh(WRITE_SCRIPT, &[&s(p), &format!("{mode:04o}")], Some(data.to_vec()), None).await?;
        if !out.success() {
            bail!("{}: {}", p.display(), why(&out));
        }
        Ok(())
    }

    async fn create_dir_all(&self, p: &Path) -> Result<()> {
        let out = self.sh(r#"mkdir -p -- "$1""#, &[&s(p)], None, Some(QUICK)).await?;
        if !out.success() {
            bail!("{}: {}", p.display(), why(&out));
        }
        Ok(())
    }

    async fn remove(&self, p: &Path) -> Result<()> {
        // An empty argument would be `rm -rf ''`: harmless, but never meant.
        if p.as_os_str().is_empty() {
            bail!("nothing to remove: empty path");
        }
        let out = self.sh(r#"rm -rf -- "$1""#, &[&s(p)], None, None).await?;
        if !out.success() {
            bail!("{}: {}", p.display(), why(&out));
        }
        Ok(())
    }

    async fn canonicalize(&self, p: &Path) -> Result<PathBuf> {
        let out = self.sh(CANON_SCRIPT, &[&s(p)], None, Some(QUICK)).await?;
        if !out.success() {
            bail!("{}: {}", p.display(), why(&out));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.strip_suffix('\n').unwrap_or(&text);
        if line.is_empty() {
            return Err(anyhow!("{}: no path came back", p.display()));
        }
        Ok(PathBuf::from(line))
    }

    async fn walk(&self, root: &Path, depth: usize, skip: &[&str]) -> Result<Vec<(String, Meta)>> {
        if depth == 0 {
            return Ok(Vec::new());
        }
        let root = s(root);
        let depth = depth.to_string();
        // `find -name` takes a glob; a skip entry is a plain name.
        let skip: Vec<String> = skip.iter().map(|n| glob_escape(n)).collect();
        let mut args = vec![root.as_str(), depth.as_str()];
        args.extend(skip.iter().map(String::as_str));
        let out = self.sh(WALK_SCRIPT, &args, None, None).await?;
        if !out.success() {
            bail!("{root}: {}", why(&out));
        }
        let mut v: Vec<(String, Meta)> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(parse_record)
            .map(|(n, m)| (n.strip_prefix("./").unwrap_or(&n).to_string(), m))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(v)
    }
}

impl ExecFs {
    async fn stat_with(&self, p: &Path, follow: bool) -> Result<Option<Meta>> {
        let out = self.sh(STAT_SCRIPT, &[&s(p), if follow { "F" } else { "L" }], None, Some(QUICK)).await?;
        match out.code {
            Some(10) => Ok(None),
            Some(0) => {
                let text = String::from_utf8_lossy(&out.stdout);
                text.lines().find_map(parse_record).map(|(_, m)| Some(m)).ok_or_else(|| anyhow!("{}: unreadable stat output", p.display()))
            }
            _ => bail!("{}: {}", p.display(), why(&out)),
        }
    }
}

/// The contract tests of `host/mod.rs`, run against `LocalFs` and against
/// `ExecFs` over `LocalExec`: the second is what SSH runs, minus SSH.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::LocalFs;
    use sbxw_sbx::host::LocalExec;

    fn both() -> Vec<(&'static str, Arc<dyn HostFs>)> {
        vec![("local", Arc::new(LocalFs)), ("exec", Arc::new(ExecFs::new(Arc::new(LocalExec))))]
    }

    #[tokio::test]
    async fn stat_and_lstat_follow_or_not_and_absent_is_none() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            let f = d.path().join("f");
            std::fs::write(&f, b"hello").unwrap();
            std::os::unix::fs::symlink(&f, d.path().join("l")).unwrap();
            std::os::unix::fs::symlink(d.path().join("gone"), d.path().join("dangling")).unwrap();

            let m = fs.stat(&f).await.unwrap().unwrap();
            assert_eq!((m.kind, m.len), (Kind::File, 5), "{n}");
            assert!(m.mtime_secs > 0, "{n}");
            assert_eq!(fs.stat(&d.path().join("l")).await.unwrap().unwrap().kind, Kind::File, "{n}");
            assert_eq!(fs.lstat(&d.path().join("l")).await.unwrap().unwrap().kind, Kind::Symlink, "{n}");
            assert_eq!(fs.stat(d.path()).await.unwrap().unwrap().kind, Kind::Dir, "{n}");
            assert_eq!(fs.stat(&d.path().join("nope")).await.unwrap(), None, "{n}");
            assert_eq!(fs.lstat(&d.path().join("nope")).await.unwrap(), None, "{n}");
            assert_eq!(fs.stat(&d.path().join("dangling")).await.unwrap(), None, "{n}");
            assert_eq!(fs.lstat(&d.path().join("dangling")).await.unwrap().unwrap().kind, Kind::Symlink, "{n}");
        }
    }

    #[tokio::test]
    async fn paths_with_spaces_quotes_and_leading_dashes_are_data_not_script() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            for name in ["a b", "it's", "$(touch pwned)", "-rf", "semi;colon"] {
                let p = d.path().join(name);
                fs.write_atomic(&p, name.as_bytes(), 0o644).await.unwrap();
                assert_eq!(fs.read(&p, 100).await.unwrap(), name.as_bytes(), "{n} {name}");
                assert_eq!(fs.stat(&p).await.unwrap().unwrap().len, name.len() as u64, "{n} {name}");
            }
            assert!(!d.path().join("pwned").exists(), "{n}");
            assert_eq!(fs.read_dir(d.path()).await.unwrap().len(), 5, "{n}");
            fs.remove(&d.path().join("-rf")).await.unwrap();
            assert_eq!(fs.read_dir(d.path()).await.unwrap().len(), 4, "{n}");
        }
    }

    #[tokio::test]
    async fn read_dir_is_sorted_with_lstat_kinds() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("b"), b"").unwrap();
            std::fs::create_dir(d.path().join("a")).unwrap();
            std::os::unix::fs::symlink("b", d.path().join("c")).unwrap();
            std::fs::write(d.path().join(".hidden"), b"").unwrap();
            let v = fs.read_dir(d.path()).await.unwrap();
            let got: Vec<(&str, Kind)> = v.iter().map(|e| (e.name.as_str(), e.meta.kind)).collect();
            assert_eq!(got, [(".hidden", Kind::File), ("a", Kind::Dir), ("b", Kind::File), ("c", Kind::Symlink)], "{n}");
            assert!(fs.read_dir(&d.path().join("nope")).await.is_err(), "{n}");
            assert!(fs.read_dir(&d.path().join("b")).await.is_err(), "{n}: a file is not a directory");
            std::fs::create_dir(d.path().join("empty")).unwrap();
            assert!(fs.read_dir(&d.path().join("empty")).await.unwrap().is_empty(), "{n}");
        }
    }

    #[tokio::test]
    async fn read_refuses_past_max() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            let f = d.path().join("f");
            std::fs::write(&f, b"12345").unwrap();
            assert_eq!(fs.read(&f, 5).await.unwrap(), b"12345", "{n}");
            assert_eq!(fs.read(&f, 100).await.unwrap(), b"12345", "{n}");
            assert!(fs.read(&f, 4).await.is_err(), "{n}");
            assert!(fs.read(&d.path().join("nope"), 10).await.is_err(), "{n}");
            std::fs::write(d.path().join("empty"), b"").unwrap();
            assert!(fs.read(&d.path().join("empty"), 0).await.unwrap().is_empty(), "{n}");
            // Bytes survive untouched: NUL, a trailing newline, invalid UTF-8.
            std::fs::write(d.path().join("bin"), b"\0\xff\n\n").unwrap();
            assert_eq!(fs.read(&d.path().join("bin"), 10).await.unwrap(), b"\0\xff\n\n", "{n}");
        }
    }

    #[tokio::test]
    async fn write_atomic_replaces_and_sets_mode() {
        use std::os::unix::fs::PermissionsExt;
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            let f = d.path().join("f");
            fs.write_atomic(&f, b"one", 0o600).await.unwrap();
            assert_eq!(std::fs::read(&f).unwrap(), b"one", "{n}");
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o600, "{n}");
            fs.write_atomic(&f, b"two", 0o755).await.unwrap();
            assert_eq!(std::fs::read(&f).unwrap(), b"two", "{n}");
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o755, "{n}");
            // No temporary file left behind.
            assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "{n}");
            assert!(fs.write_atomic(&d.path().join("no/parent"), b"x", 0o644).await.is_err(), "{n}");
            assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "{n}");
            // Empty content is a file, not an absent one.
            fs.write_atomic(&f, b"", 0o644).await.unwrap();
            assert_eq!(std::fs::read(&f).unwrap(), b"", "{n}");
        }
    }

    #[tokio::test]
    async fn create_dir_all_and_remove() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            let deep = d.path().join("a/b/c");
            fs.create_dir_all(&deep).await.unwrap();
            fs.create_dir_all(&deep).await.unwrap();
            std::fs::write(deep.join("f"), b"x").unwrap();
            let target = d.path().join("keep");
            std::fs::write(&target, b"x").unwrap();
            std::os::unix::fs::symlink(&target, d.path().join("link")).unwrap();

            fs.remove(&d.path().join("link")).await.unwrap();
            assert!(target.exists(), "{n}: removing a symlink keeps its target");
            fs.remove(&target).await.unwrap();
            assert!(!target.exists(), "{n}");
            fs.remove(&d.path().join("a")).await.unwrap();
            assert!(!d.path().join("a").exists(), "{n}");
            fs.remove(&d.path().join("nope")).await.unwrap();
            assert!(fs.create_dir_all(&d.path().join("link2/x")).await.is_ok(), "{n}");
            std::fs::write(d.path().join("file"), b"x").unwrap();
            assert!(fs.create_dir_all(&d.path().join("file/x")).await.is_err(), "{n}");
        }
    }

    #[tokio::test]
    async fn canonicalize_resolves_links() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            let real = std::fs::canonicalize(d.path()).unwrap();
            std::fs::create_dir(real.join("x")).unwrap();
            std::fs::write(real.join("x/file"), b"x").unwrap();
            std::os::unix::fs::symlink(real.join("x"), real.join("l")).unwrap();
            std::os::unix::fs::symlink("x/file", real.join("rel")).unwrap();
            std::os::unix::fs::symlink(real.join("gone"), real.join("dangling")).unwrap();
            assert_eq!(fs.canonicalize(&real.join("l/../x")).await.unwrap(), real.join("x"), "{n}");
            assert_eq!(fs.canonicalize(&real.join("l")).await.unwrap(), real.join("x"), "{n}");
            assert_eq!(fs.canonicalize(&real.join("rel")).await.unwrap(), real.join("x/file"), "{n}");
            assert_eq!(fs.canonicalize(&real.join("x/file")).await.unwrap(), real.join("x/file"), "{n}");
            assert_eq!(fs.canonicalize(Path::new("/")).await.unwrap(), Path::new("/"), "{n}");
            assert!(fs.canonicalize(&real.join("nope")).await.is_err(), "{n}");
            assert!(fs.canonicalize(&real.join("dangling")).await.is_err(), "{n}");
        }
    }

    #[tokio::test]
    async fn walk_respects_depth_and_skip_and_does_not_follow_links() {
        for (n, fs) in both() {
            let d = tempfile::tempdir().unwrap();
            let r = d.path();
            std::fs::create_dir_all(r.join("src/deep")).unwrap();
            std::fs::create_dir_all(r.join(".git/objects")).unwrap();
            std::fs::create_dir_all(r.join("lib/node_modules/x")).unwrap();
            std::fs::write(r.join("src/main.rs"), b"fn main(){}").unwrap();
            std::fs::write(r.join("src/deep/a.rs"), b"").unwrap();
            std::fs::write(r.join("README"), b"hi").unwrap();
            std::os::unix::fs::symlink(r.join("src"), r.join("ln")).unwrap();

            let names = |v: Vec<(String, Meta)>| v.into_iter().map(|(p, _)| p).collect::<Vec<_>>();
            let one = fs.walk(r, 1, &[".git"]).await.unwrap();
            assert_eq!(names(one), ["README", "lib", "ln", "src"], "{n}");

            let all = fs.walk(r, 10, &[".git", "node_modules"]).await.unwrap();
            let kinds: std::collections::BTreeMap<_, _> = all.iter().map(|(p, m)| (p.clone(), m.kind)).collect();
            assert_eq!(kinds["ln"], Kind::Symlink, "{n}");
            assert_eq!(kinds["src/main.rs"], Kind::File, "{n}");
            assert_eq!(all.iter().find(|(p, _)| p == "src/main.rs").unwrap().1.len, 11, "{n}");
            assert_eq!(names(all), ["README", "lib", "ln", "src", "src/deep", "src/deep/a.rs", "src/main.rs"], "{n}");

            let two = fs.walk(r, 2, &[]).await.unwrap();
            assert!(names(two.clone()).contains(&".git/objects".to_string()), "{n}");
            assert!(!names(two).contains(&"src/deep/a.rs".to_string()), "{n}");
            assert!(fs.walk(r, 0, &[]).await.unwrap().is_empty(), "{n}");
            assert!(fs.walk(&r.join("nope"), 1, &[]).await.is_err(), "{n}");
            // A skip name that looks like a glob is a name, not a pattern.
            std::fs::create_dir(r.join("star*")).unwrap();
            let v = names(fs.walk(r, 1, &["*"]).await.unwrap());
            assert!(v.contains(&"README".to_string()), "{n}");
            assert!(names(fs.walk(r, 1, &["star*"]).await.unwrap()).iter().all(|p| p != "star*"), "{n}");
        }
    }

    #[tokio::test]
    async fn an_unreachable_host_is_an_unreachable_error_not_a_missing_file() {
        use sbxw_sbx::host::{is_unreachable, FakeExec, HostUnreachable};
        let f = FakeExec::new(|_| Err(HostUnreachable { host: "bb".into(), reason: "timed out".into() }.into()));
        let fs = ExecFs::new(f);
        let e = fs.stat(Path::new("/x")).await.unwrap_err();
        assert!(is_unreachable(&e));
        assert!(is_unreachable(&fs.read(Path::new("/x"), 1).await.unwrap_err()));
    }

    #[tokio::test]
    async fn the_data_goes_on_stdin_and_never_into_argv() {
        use sbxw_sbx::host::FakeExec;
        let f = FakeExec::ok();
        let fs = ExecFs::new(f.clone());
        fs.write_atomic(Path::new("/p/f"), b"SECRET", 0o600).await.unwrap();
        assert_eq!(f.stdins(), [b"SECRET".to_vec()]);
        assert!(!f.log().iter().any(|l| l.contains("SECRET")));
        assert!(f.cmds.lock().unwrap()[0].args.contains(&"/p/f".to_string()), "the path is an argument");
    }
}
