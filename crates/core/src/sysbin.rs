//! Host helpers sbxw runs (`osascript`, `sudo`, `caffeinate`, `curl`…),
//! found only in the system's own directories, never through `PATH`.
//!
//! `PATH` often holds a directory a sandbox can write — a workspace's
//! `bin/`, `node_modules/.bin` — and the daemon inherits it. A helper looked
//! up there would be the sandbox's program running on the host with the
//! person's rights (sbx 0.50 pinned its own browser, clipboard and update
//! helpers the same way). `git`, `node` and `sbx` are the person's own
//! installs and stay on `PATH`.

use std::path::PathBuf;

/// Where a stock macOS or Linux keeps its tools; writable by root only.
pub const TRUSTED_DIRS: &[&str] = &["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// The helper's absolute path in [`TRUSTED_DIRS`]. When it is in none, the
/// first candidate is returned anyway, so the spawn fails as "not found"
/// rather than falling back to `PATH`.
pub fn sys_bin(name: &str) -> PathBuf {
    TRUSTED_DIRS
        .iter()
        .map(|d| PathBuf::from(d).join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(TRUSTED_DIRS[0]).join(name))
}

/// Where a package manager installs `gh`: Homebrew (Apple silicon, Intel,
/// Linux) or the distribution. Never `PATH`, for the reason above — and sbx
/// itself runs a `--command` secret's helper with no confinement.
pub const GH_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin", "/usr/bin"];

/// The GitHub CLI's absolute path, if one of [`GH_DIRS`] has it.
pub fn gh() -> Option<PathBuf> {
    GH_DIRS.iter().map(|d| PathBuf::from(d).join("gh")).find(|p| p.is_file())
}

/// The shell command sbx runs to read the token of `gh`'s login: its path
/// single-quoted, so a space in it stays one word.
pub fn gh_token_command(gh: &std::path::Path) -> String {
    format!("'{}' auth token", gh.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_helper_is_always_an_absolute_system_path() {
        let p = sys_bin("sh");
        assert!(TRUSTED_DIRS.iter().any(|d| p.starts_with(d)), "{}", p.display());
        let missing = sys_bin("no-such-helper-sbxw");
        assert_eq!(missing, PathBuf::from("/usr/bin/no-such-helper-sbxw"));
    }

    #[test]
    fn the_gh_command_quotes_its_path() {
        assert_eq!(gh_token_command(std::path::Path::new("/opt/homebrew/bin/gh")), "'/opt/homebrew/bin/gh' auth token");
        assert_eq!(gh_token_command(std::path::Path::new("/a b/it's/gh")), "'/a b/it'\\''s/gh' auth token");
    }
}
