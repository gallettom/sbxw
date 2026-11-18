//! The explorer's reader: what a Markdown file becomes for the `markdown`
//! element (its relative links and `[[wiki links]]` turned into links the
//! explorer follows itself), its outline, one section of it, and a text
//! file with its line numbers.
//!
//! A followed link is a path the reader computed from the file the daemon
//! sent and the folder it was listed in; it only selects a row of a listing
//! the daemon answers for (`files_browse` then `files_select`), never reads
//! anything on its own. Links to `http`/`https` open in the browser through
//! the app's safe path (`open_link`).

use serde_json::{json, Value as J};

/// The made-up host of the reader's own links (never resolved: `.invalid`).
pub const HOST: &str = "files.sbxw.invalid";

/// `/a/b` for an internal link to `/a/b`, percent-encoded.
pub fn url(path: &str) -> String {
    let mut o = format!("https://{HOST}");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The path of one of the reader's links; `None` for any other URL.
pub fn parse_url(u: &str) -> Option<String> {
    let rest = u.strip_prefix(&format!("https://{HOST}"))?;
    let rest = rest.split('#').next().unwrap_or("");
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let p = String::from_utf8(out).ok()?;
    (p.starts_with('/') && !p.split('/').any(|c| c == "..")).then_some(p)
}

/// `rel` against the folder `dir` (both from the daemon), `..` and `.`
/// folded; `None` above the root.
pub fn join(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if rel.starts_with('/') { vec![] } else { dir.split('/').filter(|c| !c.is_empty()).collect() };
    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

fn fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// The Markdown text with its links made the reader's own: a relative link
/// `[x](notes/a.md)` and a wiki link `[[a]]`, `[[notes/a|label]]` (a name
/// without an extension is a `.md`) point at the file next to `dir`; `http`
/// and `https` links stay; code spans and fences are left as written.
pub fn rewrite(text: &str, dir: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if fence(line) {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        if in_fence {
            out.push(line.to_string());
            continue;
        }
        let l = rewrite_line(line, dir);
        // A paragraph's lines are one paragraph: the `markdown` element keeps
        // line breaks, so a file wrapped at 72 columns would read ragged at
        // the reader's own measure. Joined with a space, unless the line
        // before ends a block (blank, a heading, a list item's start is its
        // own) or holds a hard break (two trailing spaces).
        let plain = |t: &str| {
            let t2 = t.trim_start();
            !t2.is_empty() && !t2.starts_with(['#', '-', '*', '+', '>', '|']) && !t2.chars().next().is_some_and(|c| c.is_ascii_digit()) && !t.starts_with("    ") && !t.starts_with('\t')
        };
        match out.last_mut() {
            Some(prev) if plain(&l) && !prev.trim().is_empty() && !prev.ends_with("  ") && !fence(prev) && !prev.trim_start().starts_with('#') && !prev.trim_start().starts_with('|') && joined_ok(prev) => {
                prev.push(' ');
                prev.push_str(l.trim_start());
            }
            _ => out.push(l),
        }
    }
    out.join("\n")
}

/// The line before may take the next one (not a code line inside a fence
/// already pushed, not a table row).
fn joined_ok(prev: &str) -> bool {
    !prev.starts_with("    ") && !prev.starts_with('\t')
}

fn rewrite_line(line: &str, dir: &str) -> String {
    let mut s = String::new();
    let b = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        // A code span: as written.
        if b[i] == b'`' {
            match line[i + 1..].find('`') {
                Some(j) => {
                    s.push_str(&line[i..i + j + 2]);
                    i += j + 2;
                    continue;
                }
                None => {
                    s.push_str(&line[i..]);
                    break;
                }
            }
        }
        if line[i..].starts_with("[[") {
            if let Some(j) = line[i + 2..].find("]]") {
                let inner = &line[i + 2..i + 2 + j];
                let (target, label) = match inner.split_once('|') {
                    Some((t, l)) => (t.trim(), l.trim()),
                    None => (inner.trim(), inner.trim()),
                };
                let file = if target.rsplit('/').next().is_some_and(|n| n.contains('.')) { target.to_string() } else { format!("{target}.md") };
                match join(dir, &file) {
                    Some(p) if !target.is_empty() => s.push_str(&format!("[{}]({})", label.replace(['[', ']'], ""), url(&p))),
                    _ => s.push_str(&line[i..i + j + 4]),
                }
                i += j + 4;
                continue;
            }
        }
        if b[i] == b'[' {
            // `[label](target)`: a relative target becomes the reader's.
            if let Some(close) = line[i..].find("](") {
                let start = i + close + 2;
                if let Some(end) = line[start..].find(')') {
                    let target = &line[start..start + end];
                    let rel = !target.contains("://") && !target.starts_with('#') && !target.starts_with("mailto:") && !target.is_empty() && !target.contains(' ');
                    let path = target.split('#').next().unwrap_or("");
                    match (rel, join(dir, path)) {
                        (true, Some(p)) if !path.is_empty() => {
                            s.push_str(&line[i..start]);
                            s.push_str(&url(&p));
                            s.push(')');
                        }
                        _ => s.push_str(&line[i..start + end + 1]),
                    }
                    i = start + end + 1;
                    continue;
                }
            }
        }
        let ch = line[i..].chars().next().unwrap();
        s.push(ch);
        i += ch.len_utf8();
    }
    s
}

/// The headings, outside fences: `{level, text, index}`.
pub fn outline(text: &str) -> Vec<J> {
    let mut v = vec![];
    let mut in_fence = false;
    for line in text.split('\n') {
        if fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let level = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ') {
            let t = line[level..].trim().trim_end_matches('#').trim();
            if !t.is_empty() {
                v.push(json!({ "level": level, "text": t, "index": v.len() }));
            }
        }
    }
    v
}

/// Heading `index` and what follows it, up to the next heading of its level
/// or above.
pub fn section(text: &str, index: usize) -> Option<String> {
    let mut in_fence = false;
    let mut n = 0;
    let mut start: Option<(usize, usize)> = None;
    let mut out = vec![];
    for line in text.split('\n') {
        let f = fence(line);
        if f {
            in_fence = !in_fence;
        }
        let level = if in_fence || f { 0 } else { line.bytes().take_while(|b| *b == b'#').count() };
        let heading = (1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ') && !line[level..].trim().is_empty();
        if heading {
            if let Some((_, l)) = start {
                if level <= l {
                    break;
                }
            }
            if n == index {
                start = Some((n, level));
            }
            n += 1;
        }
        if start.is_some() {
            out.push(line);
        }
    }
    start.map(|_| out.join("\n"))
}

/// A text file with its line numbers, right-aligned (`  12  code`).
pub fn numbered(text: &str) -> String {
    let n = text.lines().count().max(1);
    let w = n.to_string().len();
    text.lines().enumerate().map(|(i, l)| format!("{:>w$}  {l}", i + 1)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_followed_inside_and_code_is_left_alone() {
        let t = "See [notes](notes/a.md#x), [[b]], [[sub/c|the c]], [web](https://example.org), `[[not]]`\n```\n[[fenced]]\n```\n[up](../up.md)";
        let r = rewrite(t, "/w/docs");
        assert!(r.contains(&format!("[notes]({})", url("/w/docs/notes/a.md"))), "{r}");
        assert!(r.contains(&format!("[b]({})", url("/w/docs/b.md"))));
        assert!(r.contains(&format!("[the c]({})", url("/w/docs/sub/c.md"))));
        assert!(r.contains("[web](https://example.org)"));
        assert!(r.contains("`[[not]]`") && r.contains("\n[[fenced]]\n"));
        assert!(r.contains(&url("/w/up.md")));
        // A wrapped paragraph reads as one; lists, headings, fences keep their lines.
        let r = rewrite("one\ntwo\n\n- a\n- b\n# H\nthree  \nfour\n```\nx\ny\n```\nfive", "/w");
        assert_eq!(r, "one two\n\n- a\n- b\n# H\nthree  \nfour\n```\nx\ny\n```\nfive");
        assert_eq!(parse_url(&url("/w/docs/a b.md")).as_deref(), Some("/w/docs/a b.md"));
        assert_eq!(parse_url("https://example.org/x"), None);
        assert_eq!(parse_url(&format!("https://{HOST}/a/../../etc")), None, "never a path with ..");
        assert_eq!(join("/", "../x"), None);
    }

    #[test]
    fn outline_sections_and_numbers() {
        let t = "# A\nintro\n## B\nb\n```\n# not\n```\n## C\nc\n# D\n";
        let o = outline(t);
        assert_eq!(o.iter().map(|h| h["text"].as_str().unwrap()).collect::<Vec<_>>(), ["A", "B", "C", "D"]);
        assert_eq!(section(t, 1).unwrap(), "## B\nb\n```\n# not\n```");
        assert!(section(t, 0).unwrap().contains("## C") && !section(t, 0).unwrap().contains("# D"));
        assert_eq!(numbered("a\nb"), "1  a\n2  b");
        assert!(numbered(&"x\n".repeat(12)).starts_with(" 1  x"));
    }
}
