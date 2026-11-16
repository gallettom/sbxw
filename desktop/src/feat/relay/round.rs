//! A round of questions for the person (`ask_user_questions`, `/grill-me`),
//! as the web UI cuts it (`assets/js/chat-core.js`, `chatParseRound`), and
//! the answers going back in the tool's format (`relay.js`,
//! `relayAskDraft`, `relayAskAnswer`).
//!
//! A round is an optional intro, then per question `❓ **Q1 - Title** : body`
//! and `➡️ the recommendation`, questions separated by `---`. Cut there, so
//! no question is lost or invented. Text with no question marker at all is
//! still one question.

use serde_json::{json, Value as J};

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub id: String,
    pub title: String,
    /// The question without its `Qn - Title` head.
    pub body: String,
    /// The agent's recommendation, or empty.
    pub rec: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    pub intro: String,
    pub questions: Vec<Question>,
}

impl Round {
    pub fn to_json(&self) -> J {
        json!({
            "intro": self.intro,
            "questions": self.questions.iter().map(|q| json!({ "id": q.id, "title": q.title, "body": q.body, "rec": q.rec })).collect::<Vec<_>>(),
        })
    }
}

/// Whether a line opens a part at `marker` (leading spaces allowed); the rest
/// of the line after the marker and its spaces.
fn opens<'a>(line: &'a str, markers: &[&str]) -> Option<&'a str> {
    let t = line.trim_start();
    markers.iter().find_map(|m| t.strip_prefix(m)).map(|r| r.trim_start_matches('\u{fe0f}').trim_start())
}

/// Cut `text` at lines starting with one of `markers`: what comes before the
/// first, then each part (its first line is what followed the marker).
fn cut(text: &str, markers: &[&str]) -> (String, Vec<String>) {
    let mut head = String::new();
    let mut parts: Vec<String> = vec![];
    for line in text.split('\n') {
        if let Some(rest) = opens(line, markers) {
            parts.push(rest.to_string());
            continue;
        }
        let first = parts.is_empty() && head.is_empty();
        let into = match parts.last_mut() {
            Some(p) => p,
            None => &mut head,
        };
        if !first {
            into.push('\n');
        }
        into.push_str(line);
    }
    (head, parts)
}

/// Drops a trailing `---` separator line (and what follows it, for a recommendation).
fn before_rule(s: &str, drop_after: bool) -> String {
    let lines: Vec<&str> = s.split('\n').collect();
    let is_rule = |l: &str| {
        let t = l.trim();
        t.len() >= 3 && t.chars().all(|c| c == '-')
    };
    if drop_after {
        if let Some(i) = lines.iter().skip(1).position(|l| is_rule(l)) {
            return lines[..i + 1].join("\n");
        }
        return s.to_string();
    }
    let mut end = lines.len();
    while end > 0 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if end > 0 && is_rule(lines[end - 1]) {
        end -= 1;
    }
    lines[..end].join("\n")
}

/// The `**Q1 - Title** :` head: (id, title, bytes consumed).
fn head(ask: &str) -> Option<(String, String, usize)> {
    let b: Vec<(usize, char)> = ask.char_indices().collect();
    let mut i = 0;
    let skip = |i: &mut usize, f: &dyn Fn(char) -> bool| {
        while *i < b.len() && f(b[*i].1) {
            *i += 1;
        }
    };
    skip(&mut i, &|c| c == '*');
    skip(&mut i, &char::is_whitespace);
    if i >= b.len() || b[i].1 != 'Q' {
        return None;
    }
    let start = i;
    i += 1;
    let digits = i;
    skip(&mut i, &|c| c.is_ascii_digit());
    if i == digits {
        return None;
    }
    let id: String = b[start..i].iter().map(|x| x.1).collect();
    skip(&mut i, &|c| c == '*');
    skip(&mut i, &char::is_whitespace);
    if i < b.len() && matches!(b[i].1, '-' | '–' | '—' | ':') {
        i += 1;
    }
    skip(&mut i, &char::is_whitespace);
    skip(&mut i, &|c| c == '*');
    skip(&mut i, &char::is_whitespace);
    let t0 = i;
    skip(&mut i, &|c| !matches!(c, '*' | '\n' | ':'));
    let title: String = b[t0..i].iter().map(|x| x.1).collect::<String>().trim().to_string();
    skip(&mut i, &|c| c == '*');
    skip(&mut i, &char::is_whitespace);
    if i < b.len() && b[i].1 == ':' {
        i += 1;
    }
    skip(&mut i, &char::is_whitespace);
    let consumed = b.get(i).map(|x| x.0).unwrap_or(ask.len());
    Some((id, title, consumed))
}

/// `chatParseRound`, plus relay.js's rule: text with no marker is one question.
pub fn parse(text: &str) -> Round {
    let (intro, parts) = cut(text, &["❓"]);
    let mut questions = vec![];
    for raw in parts {
        let block = before_rule(&raw, false).trim().to_string();
        let (ask, recs) = cut(&block, &["➡️", "➡"]);
        let rec = recs.join(" ");
        let rec = before_rule(&rec, true).trim().to_string();
        let ask = ask.trim().to_string();
        let (id, title, body) = match head(&ask) {
            Some((id, title, n)) => (id, title, ask[n..].trim().to_string()),
            None => (format!("Q{}", questions.len() + 1), String::new(), ask.clone()),
        };
        questions.push(Question { id, title, body, rec });
    }
    let intro = intro.trim().to_string();
    if questions.is_empty() {
        return Round { intro: String::new(), questions: vec![Question { id: "Q1".into(), title: String::new(), body: intro, rec: String::new() }] };
    }
    Round { intro, questions }
}

/// A routed sandbox's draft as answers by question id (`relayAskDraft`):
/// each `Q1 : …` line goes to its question; a line without an id of its own
/// continues the one before.
pub fn parse_draft(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    let mut at: Option<usize> = None;
    for line in text.split('\n') {
        if let Some((id, rest)) = draft_line(line) {
            let i = match out.iter().position(|(k, _)| *k == id) {
                Some(i) => {
                    out[i].1 = rest;
                    i
                }
                None => {
                    out.push((id, rest));
                    out.len() - 1
                }
            };
            at = Some(i);
            continue;
        }
        if let (Some(i), false) = (at, line.trim().is_empty()) {
            out[i].1 = format!("{} {}", out[i].1, line.trim()).trim().to_string();
        }
    }
    out
}

/// `^\s*\**\s*(Q\d+)\s*\**\s*[:\-–]\s*(.*)$`, case-insensitive.
fn draft_line(line: &str) -> Option<(String, String)> {
    let t = line.trim_start().trim_start_matches('*').trim_start();
    let mut chars = t.char_indices();
    let (_, q) = chars.next()?;
    if q != 'Q' && q != 'q' {
        return None;
    }
    let digits_end = t[1..].find(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(t.len());
    if digits_end == 1 {
        return None;
    }
    let id = format!("Q{}", &t[1..digits_end]);
    let rest = t[digits_end..].trim_start().trim_start_matches('*').trim_start();
    let sep = rest.chars().next()?;
    if !matches!(sep, ':' | '-' | '–') {
        return None;
    }
    Some((id, rest[sep.len_utf8()..].trim().to_string()))
}

/// The form's answers in the tool's format (`relayAskAnswer`): one line per
/// answered question, `Q1 : …`; a line break in an answer becomes a space.
pub fn compose(round: &Round, answers: &[String]) -> String {
    round
        .questions
        .iter()
        .zip(answers.iter().chain(std::iter::repeat(&String::new())))
        .filter_map(|(q, a)| {
            let a = a.split_whitespace().collect::<Vec<_>>().join(" ");
            (!a.is_empty()).then(|| format!("{} : {a}", q.id))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The form filled from a draft: each question's answer, or empty.
pub fn fill(round: &Round, draft: &str) -> Vec<String> {
    let d = parse_draft(draft);
    round.questions.iter().map(|q| d.iter().find(|(k, _)| k.eq_ignore_ascii_case(&q.id)).map(|(_, v)| v.clone()).unwrap_or_default()).collect()
}

/// Plain words for reading aloud (concierge.js `vcPlain`): code blocks and
/// markdown signs left out.
pub fn plain(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in md.split('\n') {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(' ');
            continue;
        }
        if in_code {
            continue;
        }
        let l = line.trim_start();
        let l = l.trim_start_matches('#').trim_start_matches('>').trim_start();
        let l = l.strip_prefix("- ").or_else(|| l.strip_prefix("• ")).unwrap_or(l);
        out.push_str(&l.replace(['`', '*'], ""));
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What is read for a round: the intro, then each question with its recommendation.
pub fn spoken(round: &Round) -> String {
    let mut parts = vec![];
    if !round.intro.is_empty() {
        parts.push(plain(&round.intro));
    }
    for q in &round.questions {
        let mut s = q.id.clone();
        if !q.title.is_empty() {
            s.push_str(", ");
            s.push_str(&plain(&q.title));
        }
        s.push_str(". ");
        s.push_str(&plain(&q.body));
        if !q.rec.is_empty() {
            s.push_str(" Recommended: ");
            s.push_str(&plain(&q.rec));
        }
        parts.push(s);
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRILL: &str = "Two things to settle before I write the migration.\n\n❓ **Q1 - Port** : which port should the API listen on?\n➡️ 8080, as in staging.\n\n---\n\n❓ **Q2 - Auth** : keep the session cookie, or move to tokens?\nIt changes the mobile client too.\n➡️ Keep the cookie for now.\n---\n";

    #[test]
    fn a_grill_round_is_cut_into_its_questions() {
        let r = parse(GRILL);
        assert_eq!(r.intro, "Two things to settle before I write the migration.");
        assert_eq!(r.questions.len(), 2);
        assert_eq!((r.questions[0].id.as_str(), r.questions[0].title.as_str()), ("Q1", "Port"));
        assert_eq!(r.questions[0].body, "which port should the API listen on?");
        assert_eq!(r.questions[0].rec, "8080, as in staging.");
        assert_eq!(r.questions[1].title, "Auth");
        assert_eq!(r.questions[1].body, "keep the session cookie, or move to tokens?\nIt changes the mobile client too.");
        assert_eq!(r.questions[1].rec, "Keep the cookie for now.");
    }

    #[test]
    fn text_without_markers_is_one_question() {
        let r = parse("Which branch should I base this on?");
        assert_eq!(r.intro, "");
        assert_eq!(r.questions.len(), 1);
        assert_eq!(r.questions[0].id, "Q1");
        assert_eq!(r.questions[0].body, "Which branch should I base this on?");
    }

    #[test]
    fn a_head_without_stars_reads_as_chat_core_reads_it() {
        // chat-core.js's head pattern takes `Q3: what now?` as the title, the body empty.
        let r = parse("❓ Q3: what now?\n➡ nothing");
        assert_eq!((r.questions[0].id.as_str(), r.questions[0].title.as_str(), r.questions[0].body.as_str(), r.questions[0].rec.as_str()), ("Q3", "what now?", "", "nothing"));
        let r = parse("❓ **Q4** : where?\n");
        assert_eq!((r.questions[0].id.as_str(), r.questions[0].title.as_str()), ("Q4", "where?"));
    }

    #[test]
    fn a_draft_fills_the_form_and_answers_go_back_one_line_each() {
        let r = parse(GRILL);
        let f = fill(&r, "Here is my draft:\nQ1 : 8081\n**Q2** - tokens,\nbecause the mobile app already has them");
        assert_eq!(f, vec!["8081".to_string(), "tokens, because the mobile app already has them".to_string()]);
        assert_eq!(compose(&r, &["  9000 \n please ".into(), "".into()]), "Q1 : 9000 please");
        assert_eq!(compose(&r, &f), "Q1 : 8081\nQ2 : tokens, because the mobile app already has them");
    }

    #[test]
    fn a_round_is_read_as_words() {
        let r = parse(GRILL);
        let s = spoken(&r);
        assert!(s.starts_with("Two things to settle"), "{s}");
        assert!(s.contains("Q1, Port. which port should the API listen on? Recommended: 8080, as in staging."), "{s}");
        assert_eq!(plain("**bold** `code`\n```\nskip me\n```\n# Head"), "bold code Head");
    }
}
