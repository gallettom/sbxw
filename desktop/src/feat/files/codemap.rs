//! The code map: the linked Markdown a `/codemap` run leaves under
//! `.sbxw-artifacts/codemap/` (and each lens under `codemap-lenses/<slug>/`),
//! parsed as `assets/js/codemap.js` parses it: sections by heading,
//! `[[wiki links]]` (outside code spans and fences), resolved by full key or
//! unique file stem, case-insensitive headings, the H1 optional; backlinks.
//!
//! The screen draws a document with the `markdown` element, which only
//! knows `http(s)` links. So a resolved wiki link is rewritten as a link to
//! [`HOST`] (a reserved `.invalid` name that resolves nowhere): its press
//! comes back as `codemap_link`, which follows it inside the panel. A broken
//! link becomes its label and a mark, a source reference inline code. The
//! map's text is never run: it is Markdown drawn by the core.

use serde_json::{json, Value as J};
use std::collections::BTreeMap;

/// The host of the panel's own links (`.invalid` is reserved: RFC 2606).
pub const HOST: &str = "sbxw-codemap.invalid";

const SOURCE_EXTS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".rs", ".go", ".c", ".h", ".java", ".kt", ".rb", ".php", ".swift", ".cs", ".scala", ".sh"];

pub fn is_code_target(target: &str) -> bool {
    let head = target.split('#').next().unwrap_or("");
    head.rfind('.').is_some_and(|i| SOURCE_EXTS.contains(&head[i..].to_ascii_lowercase().as_str()))
}

/// A section's id: its heading chain, slugged (as `codemap.js`'s `slug`).
pub fn slug(parts: &[String]) -> String {
    let joined = parts.join("--").to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in joined.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() {
        "section".into()
    } else {
        t
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub chain: Vec<String>,
    pub level: usize,
    /// 1-based line in the file.
    pub line: usize,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub target: String,
    pub code: bool,
    pub line: usize,
    /// The heading chain it sits under.
    pub from: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    /// Path under the map's folder, without `.md` (`cli`, `areas/net`).
    pub key: String,
    /// Path under `.sbxw-artifacts`.
    pub path: String,
    pub text: String,
    pub title: String,
    pub sections: Vec<Section>,
    pub links: Vec<Link>,
}

/// Lines with fenced blocks blanked (headings and links inside them do not count).
fn mask_fences(lines: &[&str]) -> Vec<String> {
    let mut fence: Option<String> = None;
    lines
        .iter()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let opener = if indent <= 3 {
                let tick = trimmed.chars().take_while(|c| *c == '`').count();
                let tilde = trimmed.chars().take_while(|c| *c == '~').count();
                if tick >= 3 {
                    Some("`".repeat(tick))
                } else if tilde >= 3 {
                    Some("~".repeat(tilde))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(f) = &fence {
                if opener.is_some() && trimmed.starts_with(f.as_str()) {
                    fence = None;
                }
                return String::new();
            }
            if let Some(o) = opener {
                fence = Some(o);
                return String::new();
            }
            line.to_string()
        })
        .collect()
}

/// `[[links]]` and code spans in one pass, whichever starts first:
/// `(start, end, is_link)` byte ranges.
fn link_or_code(line: &str) -> Vec<(usize, usize, bool)> {
    let b = line.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' && b.get(i + 1) == Some(&b'[') {
            // `\[\[[^\]]+\]\]`
            let mut j = i + 2;
            while j < b.len() && b[j] != b']' {
                j += 1;
            }
            if j > i + 2 && j + 1 < b.len() && b[j] == b']' && b[j + 1] == b']' {
                out.push((i, j + 2, true));
                i = j + 2;
                continue;
            }
            i += 1;
            continue;
        }
        if b[i] == b'`' {
            // `` `+[^`]*`+ ``
            let mut j = i;
            while j < b.len() && b[j] == b'`' {
                j += 1;
            }
            let mut k = j;
            while k < b.len() && b[k] != b'`' {
                k += 1;
            }
            if k < b.len() {
                let mut e = k;
                while e < b.len() && b[e] == b'`' {
                    e += 1;
                }
                out.push((i, e, false));
                i = e;
                continue;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

fn strip_frontmatter(text: &str) -> (&str, usize) {
    if !text.starts_with("---\n") {
        return (text, 0);
    }
    let Some(close) = text[3..].find("\n---").map(|i| i + 3) else { return (text, 0) };
    let after = match text[close + 1..].find('\n') {
        Some(n) => close + 1 + n + 1,
        None => text.len(),
    };
    (&text[after..], text[..after].matches('\n').count())
}

fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let level = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim();
    (!title.is_empty()).then(|| (level, title.to_string()))
}

pub fn parse_doc(key: &str, path: &str, text: &str) -> Doc {
    let (body, offset) = strip_frontmatter(text);
    let raw: Vec<&str> = body.split('\n').collect();
    let masked = mask_fences(&raw);
    let mut sections: Vec<Section> = vec![];
    let mut links = vec![];
    let mut stack: Vec<Section> = vec![];
    for (i, line) in masked.iter().enumerate() {
        if let Some((level, title)) = heading(line).and_then(|_| heading(raw[i])) {
            while stack.last().is_some_and(|s| s.level >= level) {
                stack.pop();
            }
            let mut chain: Vec<String> = stack.iter().map(|s| s.title.clone()).collect();
            chain.push(title.clone());
            let s = Section { id: slug(&chain), title, chain, level, line: i + 1 + offset };
            sections.push(s.clone());
            stack.push(s);
            continue;
        }
        for (a, b, is_link) in link_or_code(line) {
            if !is_link {
                continue;
            }
            let inner = &line[a + 2..b - 2];
            let target = inner.split('|').next().unwrap_or("").trim();
            if target.is_empty() {
                continue;
            }
            links.push(Link { target: target.to_string(), code: is_code_target(target), line: i + 1 + offset, from: stack.last().map(|s| s.chain.clone()).unwrap_or_default() });
        }
    }
    let title = match sections.first() {
        Some(s) if s.level == 1 => s.title.clone(),
        _ => key.rsplit('/').next().unwrap_or(key).to_string(),
    };
    Doc { key: key.to_string(), path: path.to_string(), text: text.to_string(), title, sections, links }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved<'a> {
    Doc(&'a Doc, Option<&'a Section>),
    Broken(String),
}

pub fn resolve<'a>(target: &str, docs: &'a BTreeMap<String, Doc>) -> Resolved<'a> {
    let mut parts = target.split('#');
    let head = parts.next().unwrap_or("").trim();
    let head = head.strip_prefix("./").unwrap_or(head);
    let head = head.strip_suffix(".md").unwrap_or(head);
    let chain: Vec<String> = parts.map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect();
    let doc = match docs.get(head) {
        Some(d) => d,
        None if !head.contains('/') => {
            let hits: Vec<&Doc> = docs.values().filter(|d| d.key.rsplit('/').next() == Some(head)).collect();
            match hits.as_slice() {
                [d] => *d,
                [] => return Resolved::Broken("no such file".into()),
                _ => return Resolved::Broken(format!("ambiguous: {}", hits.iter().map(|d| d.key.as_str()).collect::<Vec<_>>().join(", "))),
            }
        }
        None => return Resolved::Broken("no such file".into()),
    };
    if chain.is_empty() {
        return Resolved::Doc(doc, None);
    }
    let find = |offset: usize| {
        doc.sections.iter().find(|s| {
            let c: Vec<String> = s.chain.iter().skip(offset).map(|x| x.to_lowercase()).collect();
            c == chain
        })
    };
    match find(0).or_else(|| find(1)) {
        Some(s) => Resolved::Doc(doc, Some(s)),
        None => Resolved::Broken("no such section".into()),
    }
}

/// Who links to each document: `doc key -> [(from key, from section chain)]`.
pub fn backlinks(docs: &BTreeMap<String, Doc>) -> BTreeMap<String, Vec<(String, Vec<String>)>> {
    let mut back: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for d in docs.values() {
        for l in d.links.iter().filter(|l| !l.code) {
            if let Resolved::Doc(t, _) = resolve(&l.target, docs) {
                if t.key != d.key {
                    back.entry(t.key.clone()).or_default().push((d.key.clone(), l.from.clone()));
                }
            }
        }
    }
    back
}

/// Broken links in the whole map.
pub fn broken_count(docs: &BTreeMap<String, Doc>) -> usize {
    docs.values().flat_map(|d| d.links.iter().filter(|l| !l.code)).filter(|l| matches!(resolve(&l.target, docs), Resolved::Broken(_))).count()
}

/// A panel link to a document (and a section).
pub fn panel_url(doc: &str, anchor: Option<&str>) -> String {
    let mut u = format!("https://{HOST}/{}", enc(doc));
    if let Some(a) = anchor.filter(|a| !a.is_empty()) {
        u.push('#');
        u.push_str(&enc(a));
    }
    u
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// A panel link pressed: `(doc, anchor)`; `None` for any other URL.
pub fn parse_panel_url(url: &str) -> Option<(String, Option<String>)> {
    let rest = url.strip_prefix(&format!("https://{HOST}/"))?;
    let (doc, anchor) = match rest.split_once('#') {
        Some((d, a)) => (d, Some(a)),
        None => (rest, None),
    };
    let dec = |s: &str| crate::pct::decode(s, false);
    let doc = dec(doc);
    (!doc.is_empty()).then(|| (doc, anchor.map(dec).filter(|a| !a.is_empty())))
}

/// Markdown has no escape for these in a link label: kept out.
fn label_text(s: &str) -> String {
    s.replace(['[', ']'], "")
}

/// The document's text for the `markdown` element: wiki links rewritten
/// (see the module's doc), code spans and fences left as written.
pub fn rewrite(text: &str, docs: &BTreeMap<String, Doc>) -> String {
    let (body, _) = strip_frontmatter(text);
    let raw: Vec<&str> = body.split('\n').collect();
    let masked = mask_fences(&raw);
    let mut out = Vec::with_capacity(raw.len());
    for (i, line) in raw.iter().enumerate() {
        // A fenced line (masked to "") stays as written.
        if masked[i].is_empty() && !line.trim().is_empty() {
            out.push(line.to_string());
            continue;
        }
        let mut s = String::new();
        let mut last = 0;
        for (a, b, is_link) in link_or_code(line) {
            if !is_link {
                continue;
            }
            s.push_str(&line[last..a]);
            let inner = &line[a + 2..b - 2];
            let (target, label) = match inner.split_once('|') {
                Some((t, l)) => (t.trim(), l.trim()),
                None => (inner.trim(), inner.trim()),
            };
            let label = if label.is_empty() { target } else { label };
            if is_code_target(target) {
                // Source is out of the panel's reach: shown as code (the side list copies it).
                s.push_str(&format!("`{}`", target.replace('`', "")));
            } else {
                match resolve(target, docs) {
                    Resolved::Doc(d, sec) => s.push_str(&format!("[{}]({})", label_text(label), panel_url(&d.key, sec.map(|x| x.id.as_str())))),
                    Resolved::Broken(why) => s.push_str(&format!("{} *(broken link: {why})*", label_text(label))),
                }
            }
            last = b;
        }
        s.push_str(&line[last..]);
        out.push(s);
    }
    out.join("\n")
}

/// One section's lines: its heading to the next heading of its level or above.
pub fn section_text(doc: &Doc, id: &str) -> Option<String> {
    let (body, offset) = strip_frontmatter(&doc.text);
    let i = doc.sections.iter().position(|s| s.id == id)?;
    let s = &doc.sections[i];
    let end = doc.sections[i + 1..].iter().find(|n| n.level <= s.level).map(|n| n.line);
    let lines: Vec<&str> = body.split('\n').collect();
    let from = s.line - 1 - offset;
    let to = end.map(|e| e - 1 - offset).unwrap_or(lines.len()).min(lines.len());
    Some(lines[from..to].join("\n"))
}

/// The view of a map as the screen sees it (`data.codemap_ui.map`).
pub fn view(docs: &BTreeMap<String, Doc>, current: Option<&str>, anchor: Option<&str>, filter: &str) -> J {
    let back = backlinks(docs);
    let f = filter.trim().to_lowercase();
    let list: Vec<J> = docs
        .values()
        .filter(|d| f.is_empty() || d.key.to_lowercase().contains(&f) || d.title.to_lowercase().contains(&f) || d.text.to_lowercase().contains(&f))
        .map(|d| json!({ "key": d.key, "title": d.title, "sections": d.sections.len(), "current": Some(d.key.as_str()) == current }))
        .collect();
    let doc = current.and_then(|k| docs.get(k));
    let shown = doc.map(|d| {
        let sec = anchor.and_then(|a| d.sections.iter().find(|s| s.id == a));
        let text = match sec.and_then(|s| section_text(d, &s.id)) {
            Some(t) => t,
            None => d.text.clone(),
        };
        let backs: Vec<J> = back.get(&d.key).map(|v| v.iter().map(|(from, chain)| json!({ "from": from, "section": chain.join(" › "), "anchor": if chain.is_empty() { String::new() } else { slug(chain) } })).collect()).unwrap_or_default();
        let mut codes: Vec<String> = d.links.iter().filter(|l| l.code).map(|l| l.target.clone()).collect();
        codes.dedup();
        json!({
            "key": d.key, "title": d.title, "path": d.path,
            "section": sec.map(|s| json!({ "id": s.id, "title": s.chain.join(" › ") })),
            "markdown": rewrite(&text, docs),
            "outline": d.sections.iter().map(|s| json!({ "id": s.id, "title": s.title, "level": s.level })).collect::<Vec<_>>(),
            "backlinks": backs,
            "code": codes,
        })
    });
    json!({ "docs": list, "count": docs.len(), "broken": broken_count(docs), "doc": shown })
}
