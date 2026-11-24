//! File and folder paths in terminal text: the same rule as the web UI's
//! `chatFilePath` (assets/js/chat-core.js), so both clients link the same
//! words; both are tested on one table (tests/js/file-link-cases.json).
//!
//! STANDING RULE, CHANGED ON THE PERSON'S REQUEST AND ONLY THIS FAR: until
//! now the desktop took file paths only from core events or daemon answers,
//! never from agent or terminal text. A path READ FROM TERMINAL TEXT (a file
//! or a folder) may now be handed to the Files screen, and only so: after a
//! deliberate gesture by the person (the primary modifier held, then a
//! click), as a request to OPEN THE FILES SCREEN on that path in that pane's
//! sandbox (`files_open_path`). The daemon's file routes decide what can be
//! listed and read, and whether the path is a folder or a file. Nothing is
//! executed, nothing is typed into a terminal, nothing is opened in the
//! system (no `open`, no app, no URL handler), and no approval path or input
//! gate changes. The path travels as a value, never formatted into a command,
//! a URL or a log line.
//!
//! Detection: ASCII path characters, a slash (or a leading `./`, `~/`, `/`),
//! no `..` segment, no scheme, at most [`MAX`] characters, and one of: a file
//! name with an extension from [`EXT`] and an optional `:line` (and `:col`);
//! a FOLDER ending with `/` (`src/app/`, `~/projects/`, no line); a path with
//! no extension that is ANCHORED (`/`, `~/`, `./`) and two segments deep
//! (`/tmp/x/y`, `~/dev/sbxw`), a folder or a file such as a Makefile, so
//! `and/or`, `TCP/IP` and `Rust/Luau` stay prose. Trailing punctuation is not
//! part of the path. It runs on the one row (and the rows it wraps into)
//! under the pointer, only while the modifier is held.
//!
//! The click does what the underline promises, also while the program takes
//! mouse reports (Claude Code turns them on): a modifier press on a link (a
//! path here, or an OSC 8 web link) is the link's, and the program hears
//! neither that press nor its release. Off a link, or with no modifier, the
//! program hears every click as before. This only withholds a report: nothing
//! is sent to the program that it would not have had.

/// Extensions the Files preview handles; the web's `CHAT_FILE_EXT`, in the
/// same order (a test reads the web's list and compares).
pub const EXT: &[&str] = &[
    "md", "markdown", "txt", "html", "htm", "json", "csv", "log", "pdf", "png", "jpg", "jpeg", "gif", "webp", "svg", "js", "mjs", "ts", "tsx", "jsx", "py", "rs", "go", "toml", "yaml", "yml",
    "sh", "css", "xml", "sql", "rb", "java", "c", "h", "cpp",
];
/// The web's `CHAT_FILE_MAX`.
pub const MAX: usize = 300;

/// A path worth linking, as written (without the line). `folder` when it
/// ends with `/` (the path keeps it): the explorer lists it. An anchored path
/// with no extension may be either; the daemon's listing decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePath {
    pub path: String,
    pub line: Option<u32>,
    pub folder: bool,
}

fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '@' | '%' | '-' | '/' | '~')
}

fn name_char(c: char, first: bool) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '@' | '%' | '-') || (!first && c == '.')
}

/// `token` as a path when it is one (`chatFilePath`).
pub fn file_path(token: &str) -> Option<FilePath> {
    if token.is_empty() || token.len() > MAX {
        return None;
    }
    let mut t = token;
    let mut line = None;
    // /:(\d{1,7})(?::\d{1,7})?$/, leftmost match.
    for (i, _) in token.match_indices(':') {
        let parts: Vec<&str> = token[i + 1..].split(':').collect();
        if parts.len() <= 2 && parts.iter().all(|p| (1..=7).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit())) {
            line = parts[0].parse().ok();
            t = &token[..i];
            break;
        }
    }
    if t.is_empty() || !t.chars().all(path_char) || !t.contains('/') {
        return None;
    }
    if t.starts_with('~') && !t.starts_with("~/") {
        return None;
    }
    let folder = t.ends_with('/');
    let anchored = t.starts_with('/') || t.starts_with("~/") || t.starts_with("./");
    let mut segs: Vec<&str> = t.strip_suffix('/').unwrap_or(t).split('/').collect();
    if matches!(segs[0], "" | "~" | ".") {
        segs.remove(0);
    }
    if segs.is_empty() || segs.iter().any(|s| matches!(*s, "" | "." | "..")) {
        return None;
    }
    let name = segs.last()?;
    let found = |folder| Some(FilePath { path: t.to_string(), line, folder });
    if folder {
        // `src/app/`, `./build/`, `~/projects/`: a folder, never with a line.
        return if line.is_none() && name.chars().all(|c| name_char(c, false)) { found(true) } else { None };
    }
    if !name.chars().enumerate().all(|(i, c)| name_char(c, i == 0) || (i == 0 && c == '.')) {
        return None;
    }
    match name[1..].rfind('.').map(|d| d + 1) {
        Some(dot) if !name.starts_with('.') => EXT.contains(&name[dot + 1..].to_ascii_lowercase().as_str()).then(|| FilePath { path: t.to_string(), line, folder: false }),
        Some(_) => None,
        // No extension: a folder or a file such as a Makefile; only anchored
        // and two segments deep (`/tmp/x/y`, `~/dev/app`), so `and/or` stays prose.
        None if anchored && segs.len() >= 2 => found(false),
        None => None,
    }
}

/// What cuts a token: whitespace, quotes, brackets (the web's separators).
fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '|')
}

/// The path in the token that holds `cells[at]`: its first and one past its
/// last index in `cells`, and the path. `*` before it and `.,;:!?*` after it
/// are not part of it, and `at` must be inside the rest. Work is bounded by
/// the token's length (a longer token is dropped before any pattern runs).
pub fn find_at(cells: &[char], at: usize) -> Option<(usize, usize, FilePath)> {
    if at >= cells.len() || separator(cells[at]) {
        return None;
    }
    let cap = MAX + 40;
    let mut a = at;
    while a > 0 && !separator(cells[a - 1]) {
        a -= 1;
        if at - a > cap {
            return None;
        }
    }
    let mut b = at + 1;
    while b < cells.len() && !separator(cells[b]) {
        b += 1;
        if b - a > cap {
            return None;
        }
    }
    while a < b && cells[a] == '*' {
        a += 1;
    }
    while b > a && matches!(cells[b - 1], '*' | '.' | ',' | ';' | ':' | '!' | '?') {
        b -= 1;
    }
    if at < a || at >= b {
        return None;
    }
    let s: String = cells[a..b].iter().collect();
    file_path(&s).map(|f| (a, b, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every path in `text` that links, in order (the shared table's shape).
    fn linked(text: &str) -> Vec<String> {
        let cells: Vec<char> = text.chars().collect();
        let mut out = vec![];
        let mut i = 0;
        while i < cells.len() {
            match find_at(&cells, i) {
                Some((_, b, f)) => {
                    out.push(f.path);
                    i = b;
                }
                None => i += 1,
            }
        }
        out
    }

    /// The table the web's tests read too (tests/js/file-link-cases.json).
    const CASES: &str = include_str!("../../../tests/js/file-link-cases.json");

    #[test]
    fn the_shared_table_links_what_it_says_and_nothing_else() {
        let t: serde_json::Value = serde_json::from_str(CASES).unwrap();
        for case in t["links"].as_array().unwrap() {
            let text = case[0].as_str().unwrap();
            let want: Vec<&str> = case[1].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
            assert_eq!(linked(text), want, "{text}");
            for w in want {
                assert_eq!(file_path(w).unwrap().folder, w.ends_with('/'), "{w}");
            }
        }
        let long = format!("a/{}.md", "b".repeat(4000));
        let long_dir = format!("/a/{}/", "b".repeat(400));
        let plain: Vec<&str> = t["plain"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        for t in plain.into_iter().chain([long.as_str(), long_dir.as_str()]) {
            assert_eq!(file_path(t), None, "{}", &t[..t.len().min(40)]);
            assert!(linked(t).is_empty(), "{}", &t[..t.len().min(40)]);
        }
        assert_eq!(file_path("src/main.rs:42:7").unwrap().line, Some(42));
        assert_eq!(file_path("src/main.rs").unwrap().line, None);
        assert_eq!(file_path("~/dev/sbxw/Makefile:12").unwrap().line, Some(12));
        // The web links a code span only when it is just a path, and no text inside a
        // fence or a link label; a terminal has no such context: a path is a path between
        // spaces, quotes and brackets (the one difference, said in TTYLINKS.md).
        assert_eq!(linked("`cat a/b.md`"), ["a/b.md"]);
    }

    #[test]
    fn hostile_text_yields_nothing_or_one_inert_path() {
        for t in [
            "<img src=x onerror=alert(1)> a/b.md", "a/\"b.md", "a/'b.md", "a/<b>.md", "javascript:alert(1)//a/b.md", "a/b c.md", "a/b.md?x=<svg>", "`a/b`.md", "\u{1b}[31ma/b.md\u{1b}[0m",
            "a/b\u{202e}.md", "a/\u{430}.md", "a/b.md\u{0}",
        ] {
            let got = linked(t);
            assert!(got.len() <= 1, "{t:?} {got:?}");
            for p in got {
                assert!(p.chars().all(path_char) && !p.contains(".."), "{p}");
            }
        }
        // 10,000-character lines, and tokens over the cap, are dropped fast.
        for t in ["a/".repeat(5000), "x".repeat(10_000), "a/b.md ".repeat(1500), "*".repeat(10_000), format!("{}a/b.md", "/".repeat(9000))] {
            let t0 = std::time::Instant::now();
            let cells: Vec<char> = t.chars().collect();
            for at in (0..cells.len()).step_by(97) {
                let _ = find_at(&cells, at);
            }
            assert!(t0.elapsed().as_millis() < 1500);
        }
    }

    /// The allow-list and the length cap are the web's: read from its source.
    #[test]
    fn allow_list_and_cap_equal_the_webs() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/js/chat-core.js");
        let Ok(src) = std::fs::read_to_string(&p) else { return };
        let set = src.split("const CHAT_FILE_EXT = new Set([").nth(1).and_then(|s| s.split("]);").next()).expect("CHAT_FILE_EXT");
        let web: Vec<&str> = set.split(',').map(|x| x.trim().trim_matches('\'')).filter(|x| !x.is_empty()).collect();
        assert_eq!(web, EXT);
        assert!(src.contains(&format!("const CHAT_FILE_MAX = {MAX};")));
        assert!(src.contains("const CHAT_FILE_NAME = /^[A-Za-z0-9_+@%-][A-Za-z0-9_.+@%-]*$/;"));
        assert!(src.contains("const CHAT_FILE_CHARS = /^[A-Za-z0-9_.+@%\\-\\/~]+$/;"));
        assert!(src.contains("const CHAT_FOLDER_NAME = /^[A-Za-z0-9_.+@%-]+$/;"));
    }
}
