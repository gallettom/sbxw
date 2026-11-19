//! Commit history of a sandbox's workspace, read-only: the shapes the daemon
//! answers (`GET /api/sandboxes/:name/commits`, `.../commits/:hash`), the
//! parsers for what git prints with the fixed formats below, the caps on a
//! patch, and the checks on what a client may name (a hash, a branch).
//!
//! Personal data: authors' **names only**. The formats never ask git for an
//! email (`%ae`, `%ce` are not in them), so none can leak into an answer.

use serde::Serialize;

/// Commits per page: the default and the most a client may ask.
pub const PAGE_DEFAULT: usize = 50;
pub const PAGE_MAX: usize = 200;
/// How deep a history is walked to find a page (`before`) and to tell
/// merged commits: past this many commits per repository the list ends.
pub const WALK_MAX: usize = 2000;
/// A commit body: its first lines, at most this many, then this many bytes.
pub const BODY_LINES: usize = 12;
pub const BODY_BYTES: usize = 2000;
/// A commit's patch: per file and in total (bytes of patch text, cut at a
/// line), and how many files are listed at most.
pub const PATCH_FILE_MAX: usize = 64 * 1024;
pub const PATCH_TOTAL_MAX: usize = 512 * 1024;
pub const FILES_MAX: usize = 300;
/// Files changed by more lines than this are listed with their counts but
/// their patch is not asked of git at all (cheap on a vendored tree).
pub const FILE_LINES_MAX: u64 = 4000;

/// The record git prints per commit for the list: fields split by US
/// (0x1f), records ended by RS (0x1e). Hash, short hash, author name, author
/// date (unix), subject, body. No email.
pub const LIST_FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s%x1f%b%x1e";
/// The walk: hash, committer date, and the ref it was reached by (`--source`).
pub const WALK_FORMAT: &str = "--format=%H%x1f%ct%x1f%S";

/// One commit in the list.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct CommitRow {
    pub hash: String,
    pub short: String,
    /// The author's name (never an email).
    pub author: String,
    /// Author date, unix seconds.
    pub at: i64,
    pub subject: String,
    /// The body's first lines ([`BODY_LINES`], [`BODY_BYTES`]).
    pub body: String,
    /// The branch it was reached from (`main`, `sbxw/<thread>`), or `HEAD`.
    pub branch: String,
    /// The thread when the branch is a thread's (`sbxw/<thread>`).
    pub thread: Option<String>,
    /// Reachable from the main checkout's HEAD (merged into its branch).
    pub merged: bool,
    /// Files it changed (against its first parent).
    pub files: u32,
    /// Lines added and removed (against its first parent).
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub removed: u64,
    /// The repository, relative to the workspace (`""`: the workspace).
    pub repo: String,
}

/// One file of a commit.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct FileChange {
    pub path: String,
    /// Lines added and removed; `None` for a binary file.
    pub added: Option<u64>,
    pub removed: Option<u64>,
    pub binary: bool,
    /// Its part of the patch (empty for a binary file, or one not asked).
    pub patch: String,
    /// This file's patch was cut or left out.
    pub truncated: bool,
}

/// Parse the walk ([`WALK_FORMAT`]): `(hash, committer time, ref)`.
pub fn parse_walk(raw: &str) -> Vec<(String, i64, String)> {
    raw.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\u{1f}');
            let h = f.next().filter(|h| is_full_hash(h))?.to_string();
            let t = f.next()?.trim().parse().unwrap_or(0);
            let r = f.next().unwrap_or("").trim().to_string();
            Some((h, t, r))
        })
        .collect()
}

/// A ref as `--source` prints it, shown: `refs/heads/x` is `x`.
pub fn branch_shown(source: &str) -> String {
    source.strip_prefix("refs/heads/").unwrap_or(source).to_string()
}

/// The thread a branch belongs to (`sbxw/<thread>`, a valid thread name).
pub fn thread_of(branch: &str) -> Option<String> {
    branch.strip_prefix("sbxw/").filter(|t| crate::names::is_valid_thread_name(t)).map(str::to_string)
}

/// The body's first lines, capped.
pub fn body_head(body: &str) -> String {
    let lines: Vec<&str> = body.trim().lines().take(BODY_LINES).collect();
    let mut s = lines.join("\n");
    if s.len() > BODY_BYTES {
        let mut cut = BODY_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    } else if body.trim().lines().count() > BODY_LINES {
        s.push_str("\n…");
    }
    s
}

/// Parse the list ([`LIST_FORMAT`], then `--shortstat`): each record, with
/// the "N files changed" line git prints after it. `branch`, `merged` and
/// `repo` are filled by the caller.
pub fn parse_list(raw: &str) -> Vec<CommitRow> {
    let mut out: Vec<CommitRow> = vec![];
    for rec in raw.split('\u{1e}') {
        // What precedes a record's fields is the previous record's
        // shortstat line (" 3 files changed, ..."), git's own, space first.
        let mut rest = rec;
        loop {
            rest = rest.trim_start_matches(['\n', '\r']);
            let line = rest.lines().next().unwrap_or("");
            if line.starts_with(' ') && line.contains("changed") {
                if let (Some(prev), Some(n)) = (out.last_mut(), files_changed(line)) {
                    prev.files = n;
                    (prev.added, prev.removed) = lines_changed(line);
                }
                rest = &rest[line.len()..];
            } else {
                break;
            }
        }
        let mut f = rest.splitn(6, '\u{1f}');
        let (Some(hash), Some(short), Some(author), Some(at), Some(subject)) = (f.next(), f.next(), f.next(), f.next(), f.next()) else {
            continue;
        };
        if !is_full_hash(hash.trim()) {
            continue;
        }
        out.push(CommitRow {
            hash: hash.trim().to_string(),
            short: short.to_string(),
            author: author.to_string(),
            at: at.parse().unwrap_or(0),
            subject: subject.to_string(),
            body: body_head(f.next().unwrap_or("")),
            branch: String::new(),
            thread: None,
            merged: false,
            files: 0,
            added: 0,
            removed: 0,
            repo: String::new(),
        });
    }
    out
}

/// "3 files changed, 10 insertions(+)" → 3.
fn files_changed(stat: &str) -> Option<u32> {
    let l = stat.lines().find(|l| l.contains("changed"))?;
    l.split_whitespace().next()?.parse().ok()
}

/// `(added, removed)` of a shortstat line (` 3 files changed, 10
/// insertions(+), 2 deletions(-)`); a part git leaves out is 0.
fn lines_changed(stat: &str) -> (u64, u64) {
    let (mut a, mut r) = (0, 0);
    for part in stat.split(',') {
        let mut w = part.split_whitespace();
        let (Some(n), Some(what)) = (w.next().and_then(|n| n.parse::<u64>().ok()), w.next()) else { continue };
        if what.starts_with("insertion") {
            a = n;
        } else if what.starts_with("deletion") {
            r = n;
        }
    }
    (a, r)
}

/// Parse `--numstat -z`-less output (`added\tremoved\tpath` per line;
/// `-\t-\tpath` for a binary file). Renames are printed as git prints them
/// with `--no-renames` off: we pass `--no-renames`, so a path is one path.
pub fn parse_numstat(raw: &str) -> Vec<FileChange> {
    raw.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let (a, r, p) = (f.next()?, f.next()?, f.next()?);
            if p.is_empty() {
                return None;
            }
            let binary = a == "-" && r == "-";
            Some(FileChange { path: p.to_string(), added: a.parse().ok(), removed: r.parse().ok(), binary, ..Default::default() })
        })
        .collect()
}

/// Split a patch (`git show --format= --patch`) into its files, each cut at
/// [`PATCH_FILE_MAX`] on a line, and the whole at [`PATCH_TOTAL_MAX`]. Each
/// part is put on the file of the same path; `true` when anything was cut.
pub fn attach_patch(files: &mut [FileChange], patch: &str) -> bool {
    let mut cut = false;
    let mut total = 0usize;
    for part in split_patch(patch) {
        let Some(path) = part_path(part) else { continue };
        let Some(f) = files.iter_mut().find(|f| f.path == path) else { continue };
        if f.binary {
            continue;
        }
        let room = PATCH_FILE_MAX.min(PATCH_TOTAL_MAX.saturating_sub(total));
        let (text, was_cut) = cut_at_line(part, room);
        total += text.len();
        f.patch = text.to_string();
        f.truncated |= was_cut;
        cut |= was_cut;
    }
    cut
}

/// The parts of a patch, one per `diff --git` header.
fn split_patch(patch: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = patch.match_indices("diff --git ").filter(|(i, _)| *i == 0 || patch.as_bytes()[i - 1] == b'\n').map(|(i, _)| i).collect();
    starts.push(patch.len());
    starts.windows(2).map(|w| &patch[w[0]..w[1]]).collect()
}

/// The path a part is about: its `+++ b/<path>` line, else (a deletion)
/// its `--- a/<path>` line.
fn part_path(part: &str) -> Option<String> {
    let mut minus = None;
    for l in part.lines().take(12) {
        if let Some(p) = l.strip_prefix("+++ b/") {
            return Some(p.to_string());
        }
        if let Some(p) = l.strip_prefix("--- a/") {
            minus = Some(p.to_string());
        }
        if l.starts_with("@@") {
            break;
        }
    }
    minus.or_else(|| {
        // A mode change or an empty file: `diff --git a/x b/x`.
        let head = part.lines().next()?.strip_prefix("diff --git a/")?;
        let half = head.len().checked_sub(2)? / 2;
        head.get(..half).filter(|a| head.get(half + 3..) == Some(*a)).map(str::to_string)
    })
}

/// `s` cut to at most `max` bytes on a line end; whether it was cut.
pub fn cut_at_line(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let end = s[..end].rfind('\n').map_or(0, |i| i + 1);
    (&s[..end], true)
}

/// A full commit hash: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
pub fn is_full_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A hash a client may name: 7 to 64 lowercase hex digits, nothing else.
pub fn valid_hash(s: &str) -> Result<&str, String> {
    if (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(s)
    } else {
        Err("a commit is named by 7 to 64 lowercase hex digits".into())
    }
}

/// A branch a client may name: `sbxw/<thread>` with a valid thread name, or
/// one plain name of letters, digits, `.`, `_`, `-` and `/` parts (each part
/// starting with a letter or digit, no `..`, no `.lock` end, at most 200
/// bytes). It is passed to git as `refs/heads/<name>`, after
/// `--end-of-options`, so it can never read as an option.
pub fn valid_branch(s: &str) -> Result<&str, String> {
    let bad = || Err(format!("'{s}' is not a branch name sbxw reads"));
    if let Some(t) = s.strip_prefix("sbxw/") {
        return if crate::names::is_valid_thread_name(t) { Ok(s) } else { bad() };
    }
    if s.is_empty() || s.len() > 200 || s.contains("..") || s.ends_with(".lock") || s.ends_with('/') {
        return bad();
    }
    let part_ok = |p: &str| p.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if s.split('/').all(part_ok) { Ok(s) } else { bad() }
}

/// The page after `before` (a full hash in the walk), or the first page:
/// at most `limit` entries, and whether more follow.
pub fn page<T: Clone>(walk: &[T], hash_of: impl Fn(&T) -> &str, before: Option<&str>, limit: usize) -> (Vec<T>, bool) {
    let start = match before {
        Some(b) => match walk.iter().position(|e| hash_of(e).starts_with(b)) {
            Some(i) => i + 1,
            None => return (vec![], false),
        },
        None => 0,
    };
    let end = (start + limit).min(walk.len());
    (walk[start..end].to_vec(), end < walk.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn hashes_and_branches_are_checked() {
        assert!(valid_hash("abc1234").is_ok() && valid_hash(H1).is_ok());
        for bad in ["abc", "ABC1234", "abc1234;rm", "--all", "HEAD", "abc 1234", &"a".repeat(65)] {
            assert!(valid_hash(bad).is_err(), "{bad}");
        }
        for ok in ["main", "sbxw/desktop-views", "feature/x-1", "release-2.0"] {
            assert!(valid_branch(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "--all", "a..b", "x.lock", "a/", "/a", "a b", "a;b", "sbxw/../x", "sbxw/", "a/-b", "refs/heads/../../x", "$(x)"] {
            assert!(valid_branch(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_list_parses_with_its_stats_and_no_email() {
        let raw = format!("{H1}\x1f1111111\x1fAda\x1f1700000000\x1fFix the thing\x1fLine one\nLine two\n\x1e\n 3 files changed, 4 insertions(+)\n{H2}\x1f2222222\x1fBo\x1f1690000000\x1fFirst\x1f\x1e\n 1 file changed, 1 insertion(+)\n");
        let v = parse_list(&raw);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].author.as_str(), v[0].files, v[0].body.as_str()), ("Ada", 3, "Line one\nLine two"));
        assert_eq!((v[1].subject.as_str(), v[1].files, v[1].at), ("First", 1, 1690000000));
        assert_eq!((v[0].added, v[0].removed, v[1].added), (4, 0, 1));
        assert_eq!(lines_changed(" 2 files changed, 5 insertions(+), 3 deletions(-)"), (5, 3));
        assert_eq!(lines_changed(" 1 file changed, 1 deletion(-)"), (0, 1));
        assert!(!LIST_FORMAT.contains("%ae") && !LIST_FORMAT.contains("%ce") && !WALK_FORMAT.contains('e'));
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("email") && !json.contains('@'));
    }

    #[test]
    fn bodies_are_capped() {
        let long = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let b = body_head(&long);
        assert_eq!(b.lines().count(), BODY_LINES + 1);
        assert!(b.ends_with('…'));
        let huge = "é".repeat(5000);
        assert!(body_head(&huge).len() <= BODY_BYTES + 3);
    }

    #[test]
    fn patches_are_cut_per_file_and_in_total_and_binaries_are_named_only() {
        let mut files = parse_numstat("1\t1\ta.txt\n-\t-\tlogo.png\n9000\t0\tbig.txt\n");
        assert!(files[1].binary && files[1].added.is_none());
        let big: String = (0..9000).map(|i| format!("+line {i:05} of the big file\n")).collect();
        let patch = format!("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\ndiff --git a/big.txt b/big.txt\n--- /dev/null\n+++ b/big.txt\n@@ -0,0 +1,9000 @@\n{big}");
        assert!(attach_patch(&mut files, &patch));
        assert!(files[0].patch.contains("+b") && !files[0].truncated);
        assert!(files[1].patch.is_empty(), "a binary file is named, not dumped");
        assert!(files[2].truncated && files[2].patch.len() <= PATCH_FILE_MAX && files[2].patch.ends_with('\n'));
        // The total: many files of 60 KB each stop at 512 KB.
        let n = 20;
        let mut many: Vec<FileChange> = (0..n).map(|i| FileChange { path: format!("f{i}"), added: Some(1), removed: Some(0), ..Default::default() }).collect();
        let body: String = (0..2000).map(|_| "+0123456789012345678901234567890\n").collect();
        let p: String = (0..n).map(|i| format!("diff --git a/f{i} b/f{i}\n--- a/f{i}\n+++ b/f{i}\n@@ -0,0 +1 @@\n{body}")).collect();
        assert!(attach_patch(&mut many, &p));
        let total: usize = many.iter().map(|f| f.patch.len()).sum();
        assert!(total <= PATCH_TOTAL_MAX, "{total}");
        assert!(many.last().unwrap().truncated);
        // A deleted file's part names it by its `--- a/` line.
        let mut del = parse_numstat("0\t2\tgone.rs\n");
        attach_patch(&mut del, "diff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n");
        assert!(del[0].patch.contains("-b"));
    }

    #[test]
    fn walks_page_and_threads_are_named() {
        let walk = parse_walk(&format!("{H1}\x1f10\x1frefs/heads/sbxw/a-b\n{H2}\x1f9\x1fHEAD\nbad\x1f1\x1fx\n"));
        assert_eq!(walk.len(), 2);
        assert_eq!(branch_shown(&walk[0].2), "sbxw/a-b");
        assert_eq!(thread_of("sbxw/a-b").as_deref(), Some("a-b"));
        assert_eq!(thread_of("main"), None);
        let (p, more) = page(&walk, |e| e.0.as_str(), None, 1);
        assert_eq!((p.len(), more), (1, true));
        let (p, more) = page(&walk, |e| e.0.as_str(), Some("1111111"), 5);
        assert_eq!((p[0].0.as_str(), more), (H2, false));
        assert!(page(&walk, |e| e.0.as_str(), Some("abcdef0"), 5).0.is_empty(), "an unknown cursor ends the list");
    }
}
