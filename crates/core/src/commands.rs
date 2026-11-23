//! The skills and slash commands a person can type in a sandbox's Claude
//! session, for the desktop input's autocompletion
//! (`GET /api/sandboxes/:name/commands`). Pure: the daemon reads the files,
//! this decides what of them may leave it.
//!
//! Only a name and a one-line description ever do. A name is narrow
//! ([`valid_name`]); a description is one line of visible text
//! ([`one_line`]), taken from the file's frontmatter
//! ([`crate::my_skills::description`]).

use crate::invisible::is_invisible;
use serde::{Deserialize, Serialize};

/// At most this many entries in one answer; files past it are not read.
pub const MAX_COMMANDS: usize = 300;
/// Longest description, in characters, the ellipsis included.
pub const MAX_DESCRIPTION: usize = 160;
/// Bytes of a file read for its frontmatter.
pub const HEAD_BYTES: usize = 4096;

/// `^[a-z0-9][a-z0-9:_-]{0,63}$`: what `/` may be followed by in the list.
/// The phone's `rcSkills` holds the same rule minus `:` and `_`;
/// plugin-style `plugin:skill` names fit this one.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b':' | b'_' | b'-'))
}

/// `text` as one line: control and invisible characters
/// ([`crate::invisible`]) become nothing (line breaks and tabs a space),
/// runs of spaces one, cut to [`MAX_DESCRIPTION`] characters with `…`.
pub fn one_line(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c.is_control() || is_invisible(c) {
            continue;
        }
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_end();
    if out.chars().count() <= MAX_DESCRIPTION {
        return out.to_string();
    }
    let cut: String = out.chars().take(MAX_DESCRIPTION - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Where an entry was found, which is also its precedence: a workspace
/// entry hides a home one of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// The project's `.claude/skills` or `.claude/commands`.
    Workspace,
    /// `~/.claude/skills` (sbx's shared store included) or
    /// `~/.claude/commands` in the sandbox: kits, kept skills.
    Home,
    /// Claude Code's own commands ([`BUILT_IN`]).
    BuiltIn,
}

/// A skill (`skills/<name>/SKILL.md`) hides a command
/// (`commands/<name>.md`) of the same name and origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Skill,
    Command,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    pub description: String,
    pub kind: Kind,
    pub origin: Origin,
}

/// One file as read: its name and the start of its text.
#[derive(Clone, Debug)]
pub struct Found {
    pub name: String,
    pub kind: Kind,
    pub origin: Origin,
    /// The file's first [`HEAD_BYTES`] bytes, or less; may be empty.
    pub head: String,
}

/// Claude Code's own commands worth completing from a text input: they act
/// on the session without opening a terminal dialog. Checked against Claude
/// Code 2.1.330's command table (name and wording); the descriptions are
/// shortened from its own.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("clear", "Start a new session with empty context"),
    ("compact", "Free up context by summarizing the conversation so far"),
    ("context", "Show what fills the context"),
    ("model", "Set the AI model for Claude Code"),
];

/// The answer's list: names checked, descriptions cleaned, one entry per
/// name (workspace over home over built-in, skill over command), sorted by
/// name, at most [`MAX_COMMANDS`].
pub fn merge(found: impl IntoIterator<Item = Found>, built_in: bool) -> Vec<Entry> {
    let mut all: Vec<Entry> = found
        .into_iter()
        .filter(|f| valid_name(&f.name))
        .map(|f| Entry {
            description: crate::my_skills::description(&f.head).map(|d| one_line(&d)).unwrap_or_default(),
            name: f.name,
            kind: f.kind,
            origin: f.origin,
        })
        .collect();
    if built_in {
        all.extend(BUILT_IN.iter().map(|(n, d)| Entry {
            name: n.to_string(),
            description: d.to_string(),
            kind: Kind::Command,
            origin: Origin::BuiltIn,
        }));
    }
    all.sort_by(|a, b| (&a.name, a.origin, a.kind).cmp(&(&b.name, b.origin, b.kind)));
    all.dedup_by(|later, first| later.name == first.name);
    all.truncate(MAX_COMMANDS);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, kind: Kind, origin: Origin, desc: &str) -> Found {
        Found { name: name.into(), kind, origin, head: format!("---\ndescription: {desc}\n---\nbody") }
    }

    #[test]
    fn names_are_narrow() {
        for ok in ["handoff", "grill-me", "plugin:skill", "a_b", "0", &"a".repeat(64)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "Handoff", "-x", ":x", "_x", "a/b", "../x", "a b", "a.md", "é", "a\u{200b}", &"a".repeat(65), "<script>"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_description_is_one_visible_line() {
        assert_eq!(one_line("  Two\nlines\tand\r\n  spaces  "), "Two lines and spaces");
        assert_eq!(one_line("a\u{202e}b\u{200b}c\u{0007}d\u{e0041}e"), "abcde");
        let long = one_line(&"word ".repeat(100));
        assert_eq!(long.chars().count(), MAX_DESCRIPTION, "{long}");
        assert!(long.ends_with('…'));
        assert_eq!(one_line(&"é".repeat(500)).chars().count(), MAX_DESCRIPTION);
    }

    #[test]
    fn one_entry_per_name_workspace_first() {
        let got = merge(
            [
                found("retro", Kind::Command, Origin::Home, "home command"),
                found("retro", Kind::Skill, Origin::Home, "home skill"),
                found("retro", Kind::Command, Origin::Workspace, "workspace command"),
                found("clear", Kind::Command, Origin::Workspace, "mine"),
                found("Bad/Name", Kind::Skill, Origin::Workspace, "x"),
                Found { name: "plain".into(), kind: Kind::Command, origin: Origin::Home, head: "no frontmatter".into() },
            ],
            true,
        );
        let names: Vec<(&str, Origin, &str)> = got.iter().map(|e| (e.name.as_str(), e.origin, e.description.as_str())).collect();
        assert_eq!(
            names,
            [
                ("clear", Origin::Workspace, "mine"),
                ("compact", Origin::BuiltIn, "Free up context by summarizing the conversation so far"),
                ("context", Origin::BuiltIn, "Show what fills the context"),
                ("model", Origin::BuiltIn, "Set the AI model for Claude Code"),
                ("plain", Origin::Home, ""),
                ("retro", Origin::Workspace, "workspace command"),
            ]
        );
        let json = serde_json::to_value(&got[1]).unwrap();
        assert_eq!(json, serde_json::json!({ "name": "compact", "description": "Free up context by summarizing the conversation so far", "kind": "command", "origin": "built-in" }));
    }

    #[test]
    fn the_list_is_capped() {
        let many = (0..1000).map(|i| found(&format!("s{i:04}"), Kind::Skill, Origin::Home, "d"));
        assert_eq!(merge(many, false).len(), MAX_COMMANDS);
    }
}
