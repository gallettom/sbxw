//! The text of sbxw's `/etc/hosts` block, and the rules for changing it.
//!
//! Everything sbxw writes to `/etc/hosts` sits between two marker lines, so it
//! is trivial to inspect and remove. This module is the *text*: parsing the
//! block out of a file, merging aliases into it, rendering the file back. The
//! privileged write itself (sudo, the macOS authentication panel) belongs to
//! the daemon's host layer, which is the only code that runs anything.
//!
//! The rule that shapes the merge: **the steady state needs no privilege.**
//! Provisioning a second sandbox must neither evict the first one's alias nor
//! rewrite a block that already says what it should, because every write is a
//! `sudo` — and the daemon has no terminal to answer a password prompt on.

use serde::{Deserialize, Serialize};

pub const BEGIN: &str = "# >>> sbxw managed block >>>";
pub const END: &str = "# <<< sbxw managed block <<<";

/// Where a write that could not get root is redone: a terminal is the one
/// place a `sudo` password prompt can be answered.
pub const SYNC_HINT: &str = "run `sbxw hosts sync` in a terminal to apply it";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAlias {
    pub hostname: String,
    pub ip: String,
}

impl HostAlias {
    pub fn new(hostname: impl Into<String>, ip: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
            ip: ip.into(),
        }
    }
}

/// Every line of `content` but the markers, tagged with whether it sits inside
/// the managed block.
fn tagged_lines(content: &str) -> impl Iterator<Item = (bool, &str)> {
    let mut in_block = false;
    content.lines().filter_map(move |line| match line.trim() {
        l if l == BEGIN => {
            in_block = true;
            None
        }
        l if l == END => {
            in_block = false;
            None
        }
        _ => Some((in_block, line)),
    })
}

/// The aliases currently in the managed block of `content`.
pub fn parse_block(content: &str) -> Vec<HostAlias> {
    tagged_lines(content)
        .filter(|(inside, _)| *inside)
        .filter_map(|(_, line)| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(ip), Some(host)) => Some(HostAlias::new(host, ip)),
                _ => None,
            }
        })
        .collect()
}

/// `content` without the managed block, everything else untouched.
pub fn strip_block(content: &str) -> String {
    let mut out = String::new();
    for (inside, line) in tagged_lines(content) {
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `content` with its block replaced by exactly `aliases`.
pub fn with_block(content: &str, aliases: &[HostAlias]) -> String {
    let mut next = strip_block(content);
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(BEGIN);
    next.push('\n');
    for a in aliases {
        next.push_str(&format!("{}\t{}\n", a.ip, a.hostname));
    }
    next.push_str(END);
    next.push('\n');
    next
}

/// `stored` with `wanted` folded in: order kept, no hostname twice, and a
/// hostname in both takes `wanted`'s IP — an alias that moved is updated in
/// place, not listed twice.
pub fn merged(stored: Vec<HostAlias>, wanted: &[HostAlias]) -> Vec<HostAlias> {
    let mut out: Vec<HostAlias> = stored
        .into_iter()
        .filter(|old| !wanted.iter().any(|new| new.hostname == old.hostname))
        .collect();
    out.extend(wanted.iter().cloned());
    out
}

/// What `content` would become with `wanted` merged into its block, or `None`
/// when it already says that — the case that must cost no privilege.
pub fn merge_into(content: &str, wanted: &[HostAlias]) -> Option<String> {
    let next = with_block(content, &merged(parse_block(content), wanted));
    (next != content).then_some(next)
}

/// The hostnames of `wanted` the block does not hold under the wanted IP.
pub fn missing(have: &[HostAlias], wanted: &[HostAlias]) -> Vec<String> {
    wanted
        .iter()
        .filter(|a| !a.hostname.is_empty() && !have.contains(a))
        .map(|a| a.hostname.clone())
        .collect()
}

/// A plain dotted-quad IPv4 literal. The alias IP can arrive from an HTTP body
/// and, on the macOS escalation path, ends up inside a shell string.
pub fn is_ipv4_literal(s: &str) -> bool {
    let mut parts = s.split('.');
    let ok = (0..4).all(|_| parts.next().is_some_and(|p| p.parse::<u8>().is_ok()));
    ok && parts.next().is_none()
}

/// A hostname fit for `/etc/hosts`: one DNS name, nothing that could start a
/// second field or a comment.
pub fn is_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Quote one argument for `/bin/sh`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Escape a string for an AppleScript string literal.
pub fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn first_line(stderr: &str) -> &str {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no output")
}

/// Is `stderr` sudo declining to authenticate, as opposed to the command
/// failing on its own terms? A refusal is worth asking somebody else about
/// (the macOS panel, a terminal); a read-only `/etc` fails however you ask.
pub fn sudo_refused(stderr: &str) -> bool {
    let lower = first_line(stderr).to_ascii_lowercase();
    lower.starts_with("sudo:")
        || ["password", "no tty", "terminal", "askpass"]
            .iter()
            .any(|s| lower.contains(s))
}

/// A failed privileged command as one line that says what to do next.
pub fn explain(stderr: &str) -> String {
    let detail = first_line(stderr);
    if sudo_refused(stderr) {
        format!("{detail} — {SYNC_HINT}")
    } else {
        detail.to_string()
    }
}

/// Parse `ifconfig lo0` for the IPv4 addresses already on it.
pub fn parse_loopback_addresses(ifconfig: &str) -> Vec<String> {
    ifconfig
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("inet")).then(|| it.next()).flatten()
        })
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(h: &str, ip: &str) -> HostAlias {
        HostAlias::new(h, ip)
    }

    #[test]
    fn merging_keeps_what_other_sandboxes_put_there() {
        let m = merged(vec![a("neos.local", "127.0.0.1"), a("x", "127.0.0.1")], &[a("y", "127.0.0.1")]);
        assert_eq!(m, [a("neos.local", "127.0.0.1"), a("x", "127.0.0.1"), a("y", "127.0.0.1")]);
    }

    #[test]
    fn a_hostname_that_moved_is_updated_not_duplicated() {
        let m = merged(vec![a("app", "127.0.0.2"), a("o", "127.0.0.1")], &[a("app", "127.0.0.3")]);
        assert_eq!(m, [a("o", "127.0.0.1"), a("app", "127.0.0.3")]);
    }

    #[test]
    fn re_stating_the_block_needs_no_write() {
        let base = "127.0.0.1\tlocalhost\n";
        let once = merge_into(base, &[a("sbxw.localhost", "127.0.0.1")]).unwrap();
        assert_eq!(merge_into(&once, &[a("sbxw.localhost", "127.0.0.1")]), None);
        assert!(once.starts_with(base));
        assert_eq!(parse_block(&once), [a("sbxw.localhost", "127.0.0.1")]);
    }

    #[test]
    fn only_the_managed_block_is_stripped() {
        let file = format!("127.0.0.1\tlocalhost\n{BEGIN}\n127.0.0.1\tapp\n{END}\n255.255.255.255\tb\n");
        assert_eq!(strip_block(&file), "127.0.0.1\tlocalhost\n255.255.255.255\tb\n");
    }

    #[test]
    fn missing_compares_name_and_ip() {
        let have = [a("x", "127.0.0.1")];
        assert!(missing(&have, &[a("x", "127.0.0.1")]).is_empty());
        assert_eq!(missing(&have, &[a("x", "127.0.0.2")]), ["x"]);
    }

    #[test]
    fn a_refusal_is_told_apart_from_a_failed_write() {
        assert!(sudo_refused("sudo: a password is required"));
        assert!(sudo_refused("sudo: no tty present and no askpass program specified"));
        assert!(!sudo_refused("tee: /etc/hosts: Read-only file system"));
        assert!(!sudo_refused(""));
        assert!(!sudo_refused(&explain("tee: /etc/hosts: No such file")));
        assert!(explain("sudo: a password is required").ends_with(SYNC_HINT));
        assert_eq!(explain(""), "no output");
    }

    #[test]
    fn only_a_dotted_quad_passes_for_an_ip() {
        assert!(is_ipv4_literal("127.0.0.255"));
        for bad in ["127.0.0.256", "127.0.0", "127.0.0.1.2", "127.0.0.1 up; rm -rf /", "::1", ""] {
            assert!(!is_ipv4_literal(bad), "{bad}");
        }
    }

    #[test]
    fn hostnames_are_one_dns_name() {
        assert!(is_hostname("api.neos.local"));
        for bad in ["", "a b", "a\tb", "#x", "-a.local", "a..b"] {
            assert!(!is_hostname(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_path_survives_both_quoting_layers() {
        assert_eq!(shell_quote("/Users/o'brien/x"), "'/Users/o'\\''brien/x'");
        assert_eq!(applescript_quote(r#"/bin/cp 'a"b\c'"#), r#""/bin/cp 'a\"b\\c'""#);
    }

    #[test]
    fn loopback_addresses_are_read_from_ifconfig() {
        let out = "lo0: flags=8049\n\tinet 127.0.0.1 netmask 0xff000000\n\tinet6 ::1\n\tinet 127.0.0.2 netmask 0xff000000\n";
        assert_eq!(parse_loopback_addresses(out), ["127.0.0.1", "127.0.0.2"]);
    }
}
