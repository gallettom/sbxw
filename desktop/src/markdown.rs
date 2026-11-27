//! The `markdown` element: just enough Markdown to read Claude's answers in a
//! narrow column, and nothing that could carry markup of its own.
//!
//! What is read (the web UI's `chatMarkdown`, `assets/js/chat-core.js`):
//! blank-line separated paragraphs that keep their line breaks; `#` to
//! `######` headings; `-`, `*`, `+` and `1.` / `1)` lists (an indented line
//! under an item carries on that item); fenced code blocks (```), shown as
//! monospace text; and inline: `code`, **bold** / __bold__, *italics* /
//! _italics_, [links](https://...) and bare `http(s)://` URLs. A backslash
//! escapes the next punctuation character. Anything else is plain text: no
//! HTML, no images. A table (a header row, a `|---|---|` separator, rows) is
//! drawn as a table whose cells wrap inside their columns; one that is not
//! well formed (cells counted differently in the header and the separator)
//! stays text, its pipes as typed.
//!
//! Code spans are read first, so bold, italics and link labels can hold a
//! code span (`**run `ls`**`, which the web UI got wrong once) and a code
//! span's content is never read as Markdown. Only `http` and `https` links
//! are links; any other scheme shows its label as plain text.

use rune::{col, el, row, text, El, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Run>),
    Paragraph(Vec<Run>),
    List { ordered: bool, start: u64, items: Vec<Vec<Run>> },
    Code(String),
    /// A header row and body rows of cells, every row as wide as the header.
    Table { head: Vec<Vec<Run>>, rows: Vec<Vec<Vec<Run>>> },
}

/// A piece of inline text with one style.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// An `http(s)` URL.
    pub link: Option<String>,
}

/// Parses `src` into blocks.
pub fn parse(src: &str) -> Vec<Block> {
    let mut out = vec![];
    let mut prose: Vec<&str> = vec![];
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```") {
            blocks(&prose.join("\n"), &mut out);
            prose.clear();
            let mut code = vec![];
            let mut closed = false;
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    closed = true;
                    break;
                }
                code.push(l);
            }
            let _ = closed; // an unclosed fence runs to the end, as in the web UI's split
            out.push(Block::Code(code.join("\n")));
        } else {
            prose.push(line);
        }
    }
    blocks(&prose.join("\n"), &mut out);
    out
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start();
    let n = t.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&n) && t[n..].starts_with([' ', '\t']) {
        return Some((n as u8, t[n..].trim()));
    }
    None
}

fn bullet(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let mut c = t.chars();
    match (c.next(), c.next()) {
        (Some('-' | '*' | '+'), Some(' ' | '\t')) => Some(t[2..].trim_start()),
        _ => None,
    }
}

fn numbered(line: &str) -> Option<(u64, &str)> {
    let t = line.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &t[digits..];
    if (rest.starts_with(". ") || rest.starts_with(") ")) || (rest.starts_with(".\t") || rest.starts_with(")\t")) {
        return Some((t[..digits].parse().ok()?, rest[2..].trim_start()));
    }
    None
}

fn blocks(prose: &str, out: &mut Vec<Block>) {
    for chunk in split_blank(prose) {
        let mut para: Vec<&str> = vec![];
        let mut list: Option<(bool, u64, Vec<String>)> = None;
        let flush_para = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
            if !para.is_empty() {
                out.push(Block::Paragraph(inline(&para.join("\n"))));
                para.clear();
            }
        };
        let flush_list = |list: &mut Option<(bool, u64, Vec<String>)>, out: &mut Vec<Block>| {
            if let Some((ordered, start, items)) = list.take() {
                out.push(Block::List { ordered, start, items: items.iter().map(|i| inline(i)).collect() });
            }
        };
        let lines: Vec<&str> = chunk.lines().collect();
        let mut li = 0;
        while li < lines.len() {
            let line = lines[li];
            li += 1;
            if line.trim().is_empty() {
                continue;
            }
            if let Some((head, used)) = table_head(line, lines.get(li).copied()) {
                flush_para(&mut para, out);
                flush_list(&mut list, out);
                li += 1;
                let mut rows = vec![];
                while li < lines.len() && lines[li].contains('|') && !lines[li].trim().is_empty() {
                    let mut cells = split_cells(lines[li]);
                    cells.resize(used, String::new());
                    rows.push(cells.iter().map(|c| inline(c)).collect());
                    li += 1;
                }
                out.push(Block::Table { head: head.iter().map(|c| inline(c)).collect(), rows });
            } else if let Some((level, t)) = heading(line) {
                flush_para(&mut para, out);
                flush_list(&mut list, out);
                out.push(Block::Heading(level, inline(t)));
            } else if let Some((ordered, start, item)) = bullet(line).map(|b| (false, 1, b)).or_else(|| numbered(line).map(|(n, t)| (true, n, t))) {
                flush_para(&mut para, out);
                if list.as_ref().is_some_and(|l| l.0 != ordered) {
                    flush_list(&mut list, out);
                }
                list.get_or_insert((ordered, start, vec![])).2.push(item.to_string());
            } else if list.is_some() && line.starts_with("  ") {
                let items = &mut list.as_mut().unwrap().2;
                let last = items.last_mut().unwrap();
                last.push(' ');
                last.push_str(line.trim());
            } else {
                flush_list(&mut list, out);
                para.push(line);
            }
        }
        flush_para(&mut para, out);
        flush_list(&mut list, out);
    }
}

/// A line's cells, split at the pipes that are not escaped; one pipe at each
/// end only frames the row.
fn split_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = vec![];
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    // The last cell, unless it is only what is after a closing pipe.
    if !cur.trim().is_empty() || !t.ends_with('|') {
        cells.push(cur.trim().to_string());
    }
    cells
}

/// Whether `|---|:--:|` is a table's separator row for `n` columns.
fn separator(line: &str, n: usize) -> bool {
    let cells = split_cells(line);
    cells.len() == n && cells.iter().all(|c| {
        let d = c.trim_matches(':');
        !d.is_empty() && d.bytes().all(|b| b == b'-') && c.matches(':').count() <= 2
    })
}

/// A table's header cells and column count when `line` and the `next` one
/// are its header and separator.
fn table_head(line: &str, next: Option<&str>) -> Option<(Vec<String>, usize)> {
    if !line.contains('|') {
        return None;
    }
    let head = split_cells(line);
    let n = head.len();
    (n >= 1 && separator(next?, n)).then_some((head, n))
}

fn split_blank(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Vec<&str> = vec![];
    for l in s.lines() {
        if l.trim().is_empty() {
            if !cur.is_empty() {
                out.push(cur.join("\n"));
                cur.clear();
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join("\n"));
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
}

/// Inline Markdown into runs.
pub fn inline(s: &str) -> Vec<Run> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = vec![];
    inline_into(&chars, Style::default(), None, &mut out);
    merge(out)
}

fn push(out: &mut Vec<Run>, t: &str, st: Style, link: &Option<String>, code: bool) {
    if !t.is_empty() {
        out.push(Run { text: t.to_string(), bold: st.bold, italic: st.italic, code, link: link.clone() });
    }
}

fn merge(runs: Vec<Run>) -> Vec<Run> {
    let mut out: Vec<Run> = vec![];
    for r in runs {
        match out.last_mut() {
            Some(l) if (l.bold, l.italic, l.code, &l.link) == (r.bold, r.italic, r.code, &r.link) => l.text.push_str(&r.text),
            _ => out.push(r),
        }
    }
    out
}

/// The end of a code span that opens with `n` backticks at `from` (its
/// content starts at `from + n`), or None.
fn code_end(c: &[char], from: usize, n: usize) -> Option<usize> {
    let mut i = from + n;
    while i < c.len() {
        if c[i] == '`' {
            let run = c[i..].iter().take_while(|&&x| x == '`').count();
            if run == n {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

fn is_punct(ch: char) -> bool {
    ch.is_ascii_punctuation()
}

/// The index of the closing `delim` (`**`, `__`, `*` or `_`) for one that
/// opened just before `from`, skipping code spans and escapes.
fn closer(c: &[char], from: usize, delim: &[char]) -> Option<usize> {
    let mut i = from;
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '`' => {
                let n = c[i..].iter().take_while(|&&x| x == '`').count();
                i = code_end(c, i, n).map_or(i + n, |e| e + n);
            }
            _ if c[i..].starts_with(delim) => {
                let single = delim.len() == 1;
                // `**` inside a `*` run is not its end; nor is a closer after a space.
                let doubled = single && c.get(i + 1) == Some(&delim[0]);
                let after_space = i > 0 && c[i - 1].is_whitespace();
                let word_after = delim[0] == '_' && c.get(i + delim.len()).is_some_and(|x| x.is_alphanumeric());
                if i > from && !after_space && !word_after && !doubled {
                    // `***`: the inner `*` closes first, the outer `**` after it.
                    if !single && c.get(i + 2) == Some(&delim[0]) {
                        return Some(i + 1);
                    }
                    return Some(i);
                }
                i += if doubled { 2 } else { 1 };
            }
            _ => i += 1,
        }
    }
    None
}

fn url_end(c: &[char], from: usize) -> usize {
    let mut e = from;
    while e < c.len() && !c[e].is_whitespace() && !matches!(c[e], '<' | '>' | '"' | '`') {
        e += 1;
    }
    // Trailing punctuation belongs to the sentence.
    while e > from && matches!(c[e - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')' | '\'' | '*' | '_') {
        e -= 1;
    }
    e
}

fn starts(c: &[char], i: usize, s: &str) -> bool {
    let mut k = i;
    for ch in s.chars() {
        if c.get(k) != Some(&ch) {
            return false;
        }
        k += 1;
    }
    true
}

pub fn safe_url(u: &str) -> bool {
    let l = u.to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && u.len() > l.find("://").unwrap() + 3 && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn inline_into(c: &[char], st: Style, link: Option<String>, out: &mut Vec<Run>) {
    let mut buf = String::new();
    let mut i = 0;
    macro_rules! flush {
        () => {
            push(out, &buf, st, &link, false);
            buf.clear();
        };
    }
    while i < c.len() {
        let ch = c[i];
        // Escapes.
        if ch == '\\' && c.get(i + 1).is_some_and(|&n| is_punct(n)) {
            buf.push(c[i + 1]);
            i += 2;
            continue;
        }
        // Code spans first: their content is never read.
        if ch == '`' {
            let n = c[i..].iter().take_while(|&&x| x == '`').count();
            if let Some(e) = code_end(c, i, n) {
                flush!();
                let inner: String = c[i + n..e].iter().collect();
                let inner = if inner.len() > 2 && inner.starts_with(' ') && inner.ends_with(' ') { inner[1..inner.len() - 1].to_string() } else { inner };
                push(out, &inner, st, &link, true);
                i = e + n;
                continue;
            }
            buf.extend(std::iter::repeat_n('`', n));
            i += n;
            continue;
        }
        // Bold, then italics.
        let mut matched = false;
        for (delim, bold) in [(&['*', '*'][..], true), (&['_', '_'][..], true), (&['*'][..], false), (&['_'][..], false)] {
            if !c[i..].starts_with(delim) {
                continue;
            }
            let open = i + delim.len();
            // An opener is followed by a non-space; `_` must not sit inside a word.
            if c.get(open).is_none_or(|x| x.is_whitespace()) {
                continue;
            }
            if delim[0] == '_' && i > 0 && c[i - 1].is_alphanumeric() {
                continue;
            }
            if !bold && c.get(open) == Some(&delim[0]) {
                continue;
            }
            if let Some(e) = closer(c, open, delim) {
                flush!();
                let inner = Style { bold: st.bold || bold, italic: st.italic || !bold };
                inline_into(&c[open..e], inner, link.clone(), out);
                i = e + delim.len();
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // [label](url)
        if ch == '[' && link.is_none() {
            if let Some(close) = (i + 1..c.len()).find(|&k| c[k] == ']' && c[k - 1] != '\\') {
                if c.get(close + 1) == Some(&'(') {
                    if let Some(end) = (close + 2..c.len()).find(|&k| c[k] == ')') {
                        let url: String = c[close + 2..end].iter().collect();
                        let url = url.trim().to_string();
                        flush!();
                        let label = &c[i + 1..close];
                        // Only http(s) is a link; any other scheme is its label as text.
                        inline_into(label, st, safe_url(&url).then_some(url), out);
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        // A bare URL.
        if link.is_none() && (starts(c, i, "https://") || starts(c, i, "http://")) && (i == 0 || !c[i - 1].is_alphanumeric()) {
            let e = url_end(c, i);
            let url: String = c[i..e].iter().collect();
            if safe_url(&url) {
                flush!();
                push(out, &url, st, &Some(url.clone()), false);
                i = e;
                continue;
            }
        }
        buf.push(ch);
        i += 1;
    }
    flush!();
}

/// All the text of the blocks, as read aloud or copied (no marks).
pub fn plain(blocks: &[Block]) -> String {
    let runs = |r: &[Run]| r.iter().map(|x| x.text.as_str()).collect::<String>();
    blocks
        .iter()
        .map(|b| match b {
            Block::Heading(_, r) | Block::Paragraph(r) => runs(r),
            Block::List { items, .. } => items.iter().map(|i| runs(i)).collect::<Vec<_>>().join("\n"),
            Block::Code(c) => c.clone(),
            Block::Table { head, rows } => std::iter::once(head).chain(rows.iter()).map(|r| r.iter().map(|c| runs(c)).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------------------
// Drawing

/// How the element looks: text colour and size, from the screen's props.
pub struct Look {
    pub color: u32,
    pub size: f32,
    /// Keyword pills in the prose (not in code or links), when on.
    pub pills: Option<std::rc::Rc<crate::emphasis::Vocab>>,
}

/// One rich text element for `runs`; `on_link(url)` when a link is pressed.
fn rich_runs<S: 'static>(runs: &[Run], look: &Look, weight: f32, size: f32, key: &str, on_link: &Option<std::rc::Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>>, sel: bool) -> El {
    use crate::theme::{c, hex};
    let mut content = String::new();
    let mut spans = vec![];
    let mut links = vec![];
    let palette = look.pills.as_ref().map(|_| crate::emphasis::screen_palette());
    let (mut pilled, mut coded) = (false, false);
    for r in runs {
        coded |= r.code;
        // Plain prose with markers: its pills (emphasis.rs); code and links stay as written.
        if let (Some(v), Some(p), false, false, false, false) = (&look.pills, &palette, r.code, r.link.is_some(), r.bold, r.italic) {
            if r.text.contains("[[") {
                crate::emphasis::rich_into(&r.text, v, p, &mut content, &mut spans);
                pilled = true;
                continue;
            }
        }
        let (a, b) = (content.len(), content.len() + r.text.len());
        content.push_str(&r.text);
        if a == b {
            continue;
        }
        let mut s = Span::new(a, b);
        let mut styled = false;
        if r.bold {
            s = s.bold();
            styled = true;
        }
        if r.italic {
            s = s.italic();
            styled = true;
        }
        if r.code {
            s = s.mono().bg(c("elevated"), 2.0).size(size * 0.92);
            styled = true;
        }
        if let Some(u) = &r.link {
            s = s.color(c("link")).underline(c("link"));
            styled = true;
            links.push((a..b, u.clone()));
        }
        if styled {
            spans.push(s);
        }
    }
    let mut e = rune::rich(content, spans).font(crate::theme::prose(size, weight)).color(hex(look.color)).w_full().min_w(0.0).key(key.to_string());
    if sel {
        e = e.selectable(true);
    }
    // A paragraph with pills (and no inline code, whose chips keep their corners): capsules.
    if pilled && !coded {
        e = e.span_radius(crate::emphasis::span_radius(size));
    }
    if !links.is_empty() {
        for (n, (range, url)) in links.into_iter().enumerate() {
            e = e.link(rune::Link::new(range, url.clone()).label(url.clone()).help(url).role("link")).key(format!("{key}-l{n}"));
        }
        e = e.key(key.to_string());
        if let Some(f) = on_link.clone() {
            e = e.on_link(move |s: &mut S, id: &str, tx: &mut rune::Tx| f(s, id, tx));
        }
    }
    e
}

/// A table: one flex row per table row, every cell of a column taking the
/// same share of the width (its weight: the longest text in the column, kept
/// between 6 and 40 characters), so the columns line up, the cells wrap
/// inside them and the whole is never wider than its container.
fn table<S: 'static>(head: &[Vec<Run>], rows: &[Vec<Vec<Run>>], look: &Look, key: &str, on_link: &Option<std::rc::Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>>, sel: bool) -> El {
    use crate::theme::{c, radius};
    let len = |cell: &[Run]| cell.iter().map(|r| r.text.chars().count()).sum::<usize>();
    let weights: Vec<f32> = (0..head.len()).map(|i| std::iter::once(head).chain(rows.iter().map(|r| r.as_slice())).map(|r| len(&r[i])).max().unwrap_or(0).clamp(6, 40) as f32).collect();
    let bright = Look { color: crate::theme::color_value("text-bright").unwrap(), size: look.size, pills: look.pills.clone() };
    let line = |cells: &[Vec<Run>], n: usize, head: bool| {
        let mut r = row().w_full().min_w(0.0).items_start().gap(12.0).px(8.0).py(crate::theme::spacing::XS);
        if head {
            r = r.fill(c("elevated"));
        }
        for (i, cell) in cells.iter().enumerate() {
            let k = format!("{key}-r{n}-c{i}");
            let e = if head { rich_runs::<S>(cell, &bright, 600.0, look.size, &k, on_link, sel) } else { rich_runs::<S>(cell, look, 400.0, look.size, &k, on_link, sel) };
            r = r.child(col().grow(weights[i]).min_w(0.0).child(e));
        }
        r
    };
    let mut t = col().key(key.to_string()).w_full().min_w(0.0).border(1.0, c("elevated")).radius(radius("sm").unwrap()).child(line(head, 0, true));
    for (n, r) in rows.iter().enumerate() {
        t = t.child(el().w_full().h(1.0).fill(c("elevated"))).child(line(r, n + 1, false));
    }
    t
}

/// The element: blocks stacked with a paragraph's gap. `key` prefixes every
/// part's key (`<key>-b<N>`), so scripts can find them.
pub fn view<S: 'static>(src: &str, look: &Look, key: &str, on_link: Option<std::rc::Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>>) -> El {
    view_blocks::<S>(&cache::parsed(src), look, key, on_link)
}

/// [`view`] of blocks already parsed. Everything but the parse (look, theme,
/// pills, key, link handler) is read here, on every call.
pub(crate) fn view_blocks<S: 'static>(blocks: &[Block], look: &Look, key: &str, on_link: Option<std::rc::Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>>) -> El {
    blocks_el::<S>(blocks, look, key, on_link, false)
}

/// [`view`] whose text the person can select and copy (`markdown{ selectable
/// = true }`): every paragraph, heading, list item, table cell and code block
/// is a selectable text (the list markers are not). No selection group of its
/// own, so a group around it (a conversation) selects across it.
pub fn view_selectable<S: 'static>(src: &str, look: &Look, key: &str, on_link: Option<std::rc::Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>>) -> El {
    blocks_el::<S>(&cache::parsed(src), look, key, on_link, true)
}

fn blocks_el<S: 'static>(blocks: &[Block], look: &Look, key: &str, on_link: Option<std::rc::Rc<dyn Fn(&mut S, &str, &mut rune::Tx)>>, sel: bool) -> El {
    use crate::theme::{c, hex, radius};
    let mut out = col().w_full().min_w(0.0).gap(8.0);
    for (i, b) in blocks.iter().enumerate() {
        let k = format!("{key}-b{i}");
        out = out.child(match b {
            Block::Heading(level, runs) => {
                let size = look.size + match level {
                    1 => 3.0,
                    2 => 2.0,
                    3 => 1.0,
                    _ => 0.0,
                };
                rich_runs::<S>(runs, &Look { color: crate::theme::color_value("text-bright").unwrap(), size, pills: look.pills.clone() }, 600.0, size, &k, &on_link, sel)
            }
            Block::Paragraph(runs) => rich_runs::<S>(runs, look, 400.0, look.size, &k, &on_link, sel),
            Block::List { ordered, start, items } => {
                let mut l = col().w_full().min_w(0.0).gap(crate::theme::spacing::XXS).key(k.clone());

                for (n, it) in items.iter().enumerate() {
                    let mark = if *ordered { format!("{}.", start + n as u64) } else { "•".to_string() };
                    l = l.child(
                        row()
                            .w_full()
                            .min_w(0.0)
                            .items_start()
                            .gap(crate::theme::spacing::INLINE)
                            .child(text(mark).font(crate::theme::prose(look.size, 400.0)).color(c("text-muted")).min_w(if *ordered { 16.0 } else { 8.0 }))
                            .child(col().grow(1.0).min_w(0.0).child(rich_runs::<S>(it, look, 400.0, look.size, &format!("{k}-i{n}"), &on_link, sel))),
                    );
                }
                l
            }
            Block::Table { head, rows } => table::<S>(head, rows, look, &k, &on_link, sel),
            Block::Code(code) => col()
                .key(k.clone())
                .w_full()
                .min_w(0.0)
                .padding(8.0)
                .radius(radius("sm").unwrap())
                .fill(c("bg"))
                .border(1.0, c("elevated"))
                .child(text(code.clone()).font(crate::screen::body((look.size * 0.92).max(12.0), 400.0).mono()).color(hex(look.color)).wrap_anywhere().w_full().min_w(0.0).selectable(sel)),
        });
    }
    out
}

/// Parsed blocks by text, so a long conversation is not parsed again on each
/// core `view` (400 messages: about 5 ms per view in a release build).
///
/// The key is the text itself (hashed, then compared in full on a hit), never
/// a pointer: a different text is a different entry, so a changed message is
/// parsed again. Parsing reads nothing but the text, so the theme, the width,
/// the pills and the link handler need no invalidation: they are applied by
/// [`view_blocks`] on every call. Bounded: two generations of at most
/// `GEN_ENTRIES` texts and `GEN_BYTES` bytes each; when the current one is
/// full it becomes the old one and the old one is dropped, and a text found in
/// the old one moves back to the current one (least recently used, roughly).
/// One per thread: the UI thread's views share it.
mod cache {
    use super::{parse, Block};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    pub(super) const GEN_ENTRIES: usize = 1024;
    pub(super) const GEN_BYTES: usize = 2 << 20;
    /// A longer text is parsed each time and not kept.
    pub(super) const MAX_TEXT: usize = 256 << 10;

    type Map = HashMap<Box<str>, Rc<Vec<Block>>>;

    #[derive(Default)]
    pub(super) struct Cache {
        now: Map,
        old: Map,
        /// The text bytes in `now`.
        bytes: usize,
    }

    impl Cache {
        pub(super) fn get(&mut self, src: &str) -> Rc<Vec<Block>> {
            if let Some(b) = self.now.get(src) {
                return b.clone();
            }
            if src.len() > MAX_TEXT {
                return Rc::new(parse(src));
            }
            let (k, b) = self.old.remove_entry(src).unwrap_or_else(|| (src.into(), Rc::new(parse(src))));
            if self.now.len() >= GEN_ENTRIES || self.bytes + src.len() > GEN_BYTES {
                self.old = std::mem::take(&mut self.now);
                self.bytes = 0;
            }
            self.bytes += src.len();
            self.now.insert(k, b.clone());
            b
        }

        #[cfg(test)]
        pub(super) fn len(&self) -> (usize, usize) {
            (self.now.len(), self.old.len())
        }
    }

    thread_local! {
        static CACHE: RefCell<Cache> = RefCell::default();
    }

    pub(super) fn parsed(src: &str) -> Rc<Vec<Block>> {
        CACHE.with(|c| c.borrow_mut().get(src))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: &str) -> Run {
        Run { text: t.into(), ..Run::default() }
    }

    #[test]
    fn bold_holding_inline_code_renders_both() {
        // The web UI's bug: `**run `ls` now**` lost its bold.
        let runs = inline("**run `ls -la` now** then");
        assert_eq!(
            runs,
            vec![
                Run { text: "run ".into(), bold: true, ..Run::default() },
                Run { text: "ls -la".into(), bold: true, code: true, ..Run::default() },
                Run { text: " now".into(), bold: true, ..Run::default() },
                r(" then"),
            ]
        );
    }

    #[test]
    fn code_spans_are_never_read_as_markdown() {
        assert_eq!(inline("`**x** [a](https://b)`"), vec![Run { text: "**x** [a](https://b)".into(), code: true, ..Run::default() }]);
        assert_eq!(inline("``a ` b``"), vec![Run { text: "a ` b".into(), code: true, ..Run::default() }]);
    }

    #[test]
    fn italics_bold_and_words_with_underscores() {
        assert_eq!(inline("*it* and _it_"), vec![Run { text: "it".into(), italic: true, ..Run::default() }, r(" and "), Run { text: "it".into(), italic: true, ..Run::default() }]);
        assert_eq!(inline("snake_case_name and 2 * 3 * 4"), vec![r("snake_case_name and 2 * 3 * 4")]);
        assert_eq!(inline("__b__"), vec![Run { text: "b".into(), bold: true, ..Run::default() }]);
        assert_eq!(inline("***both***")[0], Run { text: "both".into(), bold: true, italic: true, ..Run::default() });
        assert_eq!(inline(r"\*not\*"), vec![r("*not*")]);
        assert_eq!(inline("**unclosed"), vec![r("**unclosed")]);
    }

    #[test]
    fn only_http_links_are_links() {
        let l = |t: &str, u: &str| Run { text: t.into(), link: Some(u.into()), ..Run::default() };
        assert_eq!(inline("see [the docs](https://x.org/a) now"), vec![r("see "), l("the docs", "https://x.org/a"), r(" now")]);
        assert_eq!(inline("[run](javascript:alert(1))"), vec![r("run)")]);
        assert_eq!(inline("[f](file:///etc/passwd)"), vec![r("f")]);
        assert_eq!(inline("at https://example.com/p?q=1."), vec![r("at "), l("https://example.com/p?q=1", "https://example.com/p?q=1"), r(".")]);
        assert_eq!(inline("**[b](http://h)**"), vec![Run { text: "b".into(), bold: true, link: Some("http://h".into()), ..Run::default() }]);
        assert!(!safe_url("https://"));
        assert!(!safe_url("https://a b"));
    }

    #[test]
    fn blocks_paragraphs_lists_headings_code() {
        let b = parse("# Title\n\nline one\nline two\n\n- a\n- b **x**\n  more of b\n1. one\n2) two\n\n```rust\nfn x() {}\n**not bold**\n```\nafter");
        assert_eq!(b[0], Block::Heading(1, vec![r("Title")]));
        assert_eq!(b[1], Block::Paragraph(vec![r("line one\nline two")]));
        match &b[2] {
            Block::List { ordered: false, items, .. } => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[1].last().unwrap().text, " more of b");
            }
            x => panic!("{x:?}"),
        }
        assert!(matches!(&b[3], Block::List { ordered: true, start: 1, items } if items.len() == 2));
        assert_eq!(b[4], Block::Code("fn x() {}\n**not bold**".into()));
        assert_eq!(b[5], Block::Paragraph(vec![r("after")]));
        assert_eq!(parse("<b>hi</b> & <script>"), vec![Block::Paragraph(vec![r("<b>hi</b> & <script>")])]);
        assert!(plain(&b).contains("Title\n\nline one"));
    }

    #[test]
    fn tables_are_read_when_well_formed_and_stay_text_otherwise() {
        let b = parse("before\n\n| # | **Decision** |\n|---|:--:|\n| Q1 | read a \\| b |\n| Q2 |\n\nafter");
        assert_eq!(b.len(), 3, "{b:?}");
        let Block::Table { head, rows } = &b[1] else { panic!("{b:?}") };
        assert_eq!((head.len(), rows.len()), (2, 2));
        assert!(head[1][0].bold);
        assert_eq!(rows[0][1][0].text, "read a | b");
        assert_eq!(rows[1].len(), 2, "short rows are padded");
        // Not a table: the separator counts other cells, or is missing.
        for t in ["| a | b |\n|---|\n| 1 | 2 |", "| a | b |\n| 1 | 2 |", "a | b\nnot a separator", "|a|b|\n|--x|---|"] {
            assert!(parse(t).iter().all(|b| matches!(b, Block::Paragraph(_))), "{t}");
        }
        // Without outer pipes.
        assert!(matches!(&parse("a | b\n--|--\n1 | 2")[0], Block::Table { .. }));
        assert_eq!(plain(&b[1..2]), "#\tDecision\nQ1\tread a | b\nQ2\t");
    }

    /// Messages of every kind the views show: code blocks, lists, links,
    /// status labels, tables, headings, long text, empty.
    fn varied() -> Vec<String> {
        let mut v: Vec<String> = [
            "",
            "plain line",
            "# Title\n\nline one\nline two",
            "Step 3 is **done**: `npm test pay` passes.\n\n- one change\n- [[Done]]",
            "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\nafter the fence",
            "```\nunclosed fence\n**not bold**",
            "1. one\n2) two\n   carried on\n- bullet",
            "See [the docs](https://example.com/a?b=c) and https://example.org/x, not [x](javascript:alert(1)).",
            "[[Needs you: pick one]] then [[Failed]] and *italics* and __bold__",
            "| a | **b** |\n|---|:--:|\n| 1 | `2` |\n| 3 |",
            "\u{e9}t\u{e9} \u{20ac} \u{1f600} \\*escaped\\*",
            "<script>alert(1)</script> & <b>no html</b>",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.push("A long paragraph with **bold**, `code` and a [link](https://example.com). ".repeat(300));
        v.push(v[3..10].join("\n\n").repeat(8));
        v
    }

    /// The cache hands `view` exactly what a fresh parse gives, on a miss, on
    /// a hit and after eviction; and both paths build an element. (Rune's `El`
    /// has no public inspection, so the trees are compared through the one
    /// input they are built from: `view` and `view_blocks` share every other
    /// line.)
    #[test]
    fn cached_blocks_are_the_fresh_parse() {
        let look = Look { color: 0xdddddd, size: 13.0, pills: None };
        let mut c = cache::Cache::default();
        for m in varied() {
            let fresh = parse(&m);
            let miss = c.get(&m);
            assert_eq!(*miss, fresh, "{m:?}");
            let hit = c.get(&m);
            assert!(std::rc::Rc::ptr_eq(&miss, &hit), "a second view is a hit: {m:?}");
            assert_eq!(*hit, fresh);
            let _ = view::<()>(&m, &look, "k", None);
            let _ = view_blocks::<()>(&fresh, &look, "k", None);
        }
        // Through the thread's cache, as `view` uses it.
        for m in varied() {
            assert_eq!(*cache::parsed(&m), parse(&m));
            assert_eq!(*cache::parsed(&m), parse(&m));
        }
    }

    /// A changed text is a new entry (parsed again); a text never matches
    /// another by its length or a shared prefix.
    #[test]
    fn a_changed_text_is_parsed_again() {
        let mut c = cache::Cache::default();
        let a = c.get("- one\n- two");
        let b = c.get("- one\n- twO");
        assert!(!std::rc::Rc::ptr_eq(&a, &b));
        assert_eq!(*b, parse("- one\n- twO"));
        assert_eq!(*c.get("- one"), parse("- one"));
        assert_eq!(*c.get("- one\n- two"), *a);
    }

    #[test]
    fn the_cache_is_bounded() {
        let mut c = cache::Cache::default();
        let first = c.get("text 0");
        for i in 1..cache::GEN_ENTRIES * 3 {
            c.get(&format!("text {i}"));
            let (now, old) = c.len();
            assert!(now <= cache::GEN_ENTRIES && old <= cache::GEN_ENTRIES, "{now} {old}");
        }
        // "text 0" was dropped two generations ago: parsed again.
        assert!(!std::rc::Rc::ptr_eq(&first, &c.get("text 0")));
        // By bytes: 40 texts of 100 KB never keep more than two generations' worth.
        let mut c = cache::Cache::default();
        for i in 0..40 {
            c.get(&format!("{i} {}", "x".repeat(100 << 10)));
            let (now, old) = c.len();
            assert!((now + old) * (100 << 10) <= 2 * cache::GEN_BYTES + (200 << 10), "{now} {old}");
        }
        // Over the per-text cap: parsed, not kept.
        let mut c = cache::Cache::default();
        let big = "y".repeat(cache::MAX_TEXT + 1);
        assert_eq!(*c.get(&big), parse(&big));
        assert_eq!(c.len(), (0, 0));
    }

    /// A text read again from the old generation moves back to the current
    /// one, so the messages on screen survive a rotation.
    #[test]
    fn a_text_in_use_survives_a_rotation() {
        let mut c = cache::Cache::default();
        let kept = c.get("on screen");
        for i in 0..cache::GEN_ENTRIES {
            c.get(&format!("other {i}"));
        }
        assert_eq!(c.len().1, cache::GEN_ENTRIES, "rotated once");
        assert!(std::rc::Rc::ptr_eq(&kept, &c.get("on screen")), "found in the old generation");
        for i in 0..cache::GEN_ENTRIES - 1 {
            c.get(&format!("again {i}"));
        }
        assert!(std::rc::Rc::ptr_eq(&kept, &c.get("on screen")), "moved back on its read: it survives a second rotation");
    }
}
