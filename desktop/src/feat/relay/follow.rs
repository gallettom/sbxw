//! Following a sandbox other than Jarvis from the Jarvis panel
//! (`assets/js/concierge.js`, the parts that are not Jarvis): a target
//! picked at the top of the panel, the person's words going to that
//! sandbox's agent as a draft they confirm in the core's dialog, and its
//! agents' turns shown in the panel's log and read aloud **only** when the
//! person follows it, or a round of questions runs (`vcEngages`).
//!
//! The core (`src/shell/jarvis.rs`) asks this module three things: where
//! the words go ([`target`]), what a `session` event of the target means
//! ([`ingest`]), and which option words name ([`pick_option`]). Nothing here
//! reaches Jarvis's approvals: a followed agent's text only ever lands in the
//! log, and the person's words for the target never pass through Jarvis's
//! yes/no.

use serde_json::{json, Value as J};
use std::collections::HashMap;

pub const JARVIS: &str = super::JARVIS;

/// One followed session, as chat-core.js keeps it (`chatNew`).
#[derive(Debug, Default, Clone)]
struct Chat {
    state: String,
    /// The reply last shown, so the same turn is never shown twice.
    shown: Option<String>,
    /// The question last shown.
    asked: Option<String>,
}

#[derive(Debug, Default)]
pub struct Follow {
    /// The sandbox the words go to; none: Jarvis.
    pub target: Option<String>,
    /// Every turn of the target's agents is shown and read (else only rounds).
    pub on: bool,
    /// The target and Follow were read back from the core's store.
    pub loaded: bool,
    chats: HashMap<String, Chat>,
    /// The session the next words answer (the last one that spoke), when a thread.
    pub reply_to: Option<(String, Option<String>)>,
    /// A one-step multiple-choice question the next words may pick from.
    pub choose: Option<Choose>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choose {
    pub sandbox: String,
    pub thread: Option<String>,
    pub text: String,
    pub options: Vec<String>,
}

super::per_thread!(FOLLOW, Follow);

/// Where the person's words go: a sandbox, or none for Jarvis.
pub fn target() -> Option<String> {
    FOLLOW().lock().unwrap().target.clone()
}

/// The question the next words may answer, if the target asks one.
pub fn choosing() -> Option<Choose> {
    let f = FOLLOW().lock().unwrap();
    f.choose.clone().filter(|c| f.target.as_deref() == Some(c.sandbox.as_str()))
}

/// What a `session` event of the followed sandbox brings to the panel.
#[derive(Debug, Clone, PartialEq)]
pub enum News {
    /// A message to show in the log (who said it, the text), and whether to
    /// read it aloud.
    Say { from: String, text: String, read: bool },
    /// A round of questions (`/grill-me` format) in a turn: answered on one
    /// form in the Requests panel.
    Round { sandbox: String, thread: Option<String>, text: String },
}

/// `/grill-me` (or `/grilling`) as a prompt, or a reply in its round format:
/// engages the panel even with Follow off (concierge.js `VC_GRILL_*`).
pub fn is_grill_prompt(s: &str) -> bool {
    let t = s.trim_start();
    ["/grill-me", "/grilling"].iter().any(|p| t.strip_prefix(p).is_some_and(|r| r.chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))))
}

pub fn is_grill_reply(s: &str) -> bool {
    s.match_indices('❓').any(|(i, _)| {
        let rest = s[i + '❓'.len_utf8()..].trim_start();
        rest.strip_prefix("**Q").is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
    })
}

pub fn who(sandbox: &str, thread: Option<&str>) -> String {
    match thread {
        Some(t) => format!("{sandbox} · {t}"),
        None => sandbox.to_string(),
    }
}

/// A `session` event (a `SessionInfo`). Only the target's sessions that sbxw
/// drives (its agent and its threads, not a client of their own) count; a
/// turn is shown once, when it ends with a reply (`chatIngest`), and only
/// when engaged.
pub fn ingest(info: &J) -> Vec<News> {
    let mut f = FOLLOW().lock().unwrap();
    f.ingest(info)
}

impl Follow {
    pub fn ingest(&mut self, info: &J) -> Vec<News> {
        let sandbox = info["sandbox"].as_str().unwrap_or("");
        let Some(target) = self.target.clone().filter(|t| t == sandbox) else { return vec![] };
        let thread = info["thread"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        if info["origin"] == "remote" && thread.is_none() {
            return vec![];
        }
        let key = format!("{target}::{}", thread.as_deref().unwrap_or(""));
        let raw = info["state"].as_str().unwrap_or("");
        let state = if raw == "exited" { "idle" } else { raw }.to_string();
        let c = self.chats.entry(key).or_default();
        let prev = std::mem::replace(&mut c.state, state.clone());
        let from = who(&target, thread.as_deref());
        let last_input = info["last_input"].as_str().unwrap_or("");
        let mut out = vec![];
        match state.as_str() {
            "working" => c.asked = None,
            "idle" if prev == "working" || prev == "attention" => {
                let Some(reply) = info["reply"].as_str().filter(|r| !r.trim().is_empty()) else { return out };
                if c.shown.as_deref() == Some(reply) {
                    return out;
                }
                c.shown = Some(reply.to_string());
                if is_grill_reply(reply) {
                    out.push(News::Round { sandbox: target.clone(), thread: thread.clone(), text: reply.to_string() });
                    let n = super::round::parse(reply).questions.len();
                    out.push(News::Say { from: from.clone(), text: format!("{from} asks you {}: answer on the form in the Requests panel.", if n == 1 { "one question".to_string() } else { format!("{n} questions") }), read: true });
                } else if self.on || is_grill_prompt(last_input) {
                    out.push(News::Say { from: from.clone(), text: reply.to_string(), read: true });
                    self.reply_to = Some((target.clone(), thread.clone()));
                }
            }
            "attention" if prev != "attention" => {
                let steps: Vec<J> = match info["steps"].as_array() {
                    Some(s) if !s.is_empty() => s.clone(),
                    _ if info["question"].is_object() => vec![info["question"].clone()],
                    _ => vec![],
                };
                if !(self.on || is_grill_prompt(last_input)) {
                    return out;
                }
                let text = steps.first().and_then(|s| s["text"].as_str()).or(info["activity"].as_str()).unwrap_or("Claude is waiting for you.").to_string();
                if c.asked.as_deref() == Some(text.as_str()) {
                    return out;
                }
                c.asked = Some(text.clone());
                let options: Vec<String> = steps.first().and_then(|s| s["options"].as_array()).map(|o| o.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                if steps.len() == 1 && !options.is_empty() && info["answerable"] == true {
                    self.choose = Some(Choose { sandbox: target.clone(), thread: thread.clone(), text: text.clone(), options: options.clone() });
                    let listed = options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect::<Vec<_>>().join("\n");
                    out.push(News::Say { from: from.clone(), text: format!("{text}\n\n{listed}\n\nSay or type your choice; you confirm it before it is sent."), read: true });
                } else if steps.len() > 1 {
                    out.push(News::Say { from: from.clone(), text: format!("{from} asks {} questions: answer them with the buttons in the Requests panel.", steps.len()), read: true });
                } else {
                    out.push(News::Say { from: from.clone(), text: format!("{from} waits for you in its terminal: {text}"), read: true });
                }
            }
            _ => {}
        }
        out
    }

    /// The screen's view (`data.relay_follow`).
    pub fn to_json(&self) -> J {
        json!({ "target": self.target, "follow": self.on, "choosing": self.choose.as_ref().filter(|c| self.target.as_deref() == Some(c.sandbox.as_str())).map(|c| c.text.clone()) })
    }
}

fn words(text: &str) -> Vec<String> {
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect();
    folded.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

const NUMBERS: &[&[&str]] = &[
    &["1", "un", "une", "one", "premier", "premiere", "first"],
    &["2", "deux", "two", "deuxieme", "second", "seconde"],
    &["3", "trois", "three", "troisieme", "third"],
    &["4", "quatre", "four", "quatrieme", "fourth"],
    &["5", "cinq", "five", "cinquieme", "fifth"],
    &["6", "six", "sixieme", "sixth"],
];

/// Which option the words name, from 1, or 0 (concierge.js `vcPickOption`):
/// a label said whole, a number or a rank, "the last one", or the one label
/// sharing the most words. A wrong guess is still shown and confirmed.
pub fn pick_option(said: &str, options: &[String]) -> usize {
    let w = words(said);
    if w.is_empty() {
        return 0;
    }
    if w.iter().any(|x| matches!(x.as_str(), "derniere" | "dernier" | "last")) {
        return options.len();
    }
    let scores: Vec<f64> = options
        .iter()
        .map(|o| {
            let label: Vec<String> = words(o).into_iter().filter(|x| x.len() > 2).collect();
            if label.is_empty() {
                0.0
            } else {
                label.iter().filter(|x| w.contains(x)).count() as f64 / label.len() as f64
            }
        })
        .collect();
    let best = scores.iter().cloned().fold(0.0, f64::max);
    let count = |v: f64| scores.iter().filter(|x| **x == v).count();
    if best == 1.0 && count(1.0) == 1 {
        return scores.iter().position(|x| *x == 1.0).unwrap() + 1;
    }
    if let Some(n) = NUMBERS.iter().position(|names| w.iter().any(|x| names.contains(&x.as_str()))) {
        if n < options.len() {
            return n + 1;
        }
    }
    if best > 0.0 && count(best) == 1 {
        return scores.iter().position(|x| *x == best).unwrap() + 1;
    }
    0
}

/// The commands said while a round is answered by voice (concierge.js
/// `VC_REPEAT`, `VC_SKIP`, `VC_BACK`), by the first words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Repeat,
    Skip,
    Back,
    Answer,
}

pub fn command(said: &str) -> Command {
    let w = words(said);
    let first = w.first().map(String::as_str).unwrap_or("");
    match first {
        "repete" | "encore" | "repeat" | "again" => Command::Repeat,
        "passe" | "suivante" | "skip" | "next" => Command::Skip,
        "je" if w.get(1).map(String::as_str) == Some("ne") && w.get(2).map(String::as_str) == Some("sais") => Command::Skip,
        "precedente" | "reviens" | "back" | "previous" => Command::Back,
        _ => Command::Answer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(state: &str, extra: J) -> J {
        let mut v = json!({ "sandbox": "web-app", "state": state, "origin": "tty", "answerable": true, "session_id": "s1" });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn a_followed_turn_is_shown_once_and_only_when_engaged() {
        let mut f = Follow { target: Some("web-app".into()), ..Follow::default() };
        // Follow off: an ordinary turn is not shown.
        f.ingest(&sess("working", json!({})));
        assert!(f.ingest(&sess("idle", json!({ "reply": "Done: tests pass." }))).is_empty());
        // Follow on: shown, read, once.
        f.on = true;
        f.ingest(&sess("working", json!({})));
        let n = f.ingest(&sess("idle", json!({ "reply": "Second turn." })));
        assert_eq!(n, vec![News::Say { from: "web-app".into(), text: "Second turn.".into(), read: true }]);
        assert!(f.ingest(&sess("idle", json!({ "reply": "Second turn." }))).is_empty(), "the same event again");
        // Another sandbox, a client of its own: nothing.
        assert!(f.ingest(&json!({ "sandbox": "api", "state": "idle", "reply": "x" })).is_empty());
        f.ingest(&sess("working", json!({ "origin": "remote" })));
        assert!(f.ingest(&sess("idle", json!({ "origin": "remote", "reply": "from an editor" }))).is_empty());
    }

    #[test]
    fn a_grill_engages_even_with_follow_off() {
        let mut f = Follow { target: Some("web-app".into()), ..Follow::default() };
        f.ingest(&sess("working", json!({ "thread": "t1", "last_input": "/grill-me the plan" })));
        let n = f.ingest(&sess("idle", json!({ "thread": "t1", "last_input": "/grill-me the plan", "reply": "❓ **Q1 - Port** : which?\n➡️ 8080" })));
        assert!(matches!(&n[0], News::Round { thread: Some(t), .. } if t == "t1"), "{n:?}");
        assert!(matches!(&n[1], News::Say { text, .. } if text.contains("one question")));
        // A plain answer to a /grill-me prompt is shown too.
        f.ingest(&sess("working", json!({ "last_input": "/grill-me" })));
        assert_eq!(f.ingest(&sess("idle", json!({ "last_input": "/grill-me", "reply": "No questions left." }))).len(), 1);
        assert!(is_grill_prompt("  /grilling now") && !is_grill_prompt("/grill-meeting"));
    }

    #[test]
    fn a_followed_question_offers_its_options_to_the_voice() {
        let mut f = Follow { target: Some("web-app".into()), on: true, ..Follow::default() };
        f.ingest(&sess("working", json!({})));
        let q = json!({ "text": "Which layout?", "options": ["Two columns", "One column", "Keep it"] });
        let n = f.ingest(&sess("attention", json!({ "steps": [q] })));
        assert!(matches!(&n[0], News::Say { text, .. } if text.contains("2. One column")));
        assert_eq!(f.choose.as_ref().unwrap().options.len(), 3);
        let o = f.choose.clone().unwrap().options;
        assert_eq!(pick_option("deux colonnes", &["Deux colonnes".into(), "Une colonne".into()]), 1);
        assert_eq!(pick_option("the second", &o), 2);
        assert_eq!(pick_option("one column please", &o), 2);
        assert_eq!(pick_option("the last", &o), 3);
        assert_eq!(pick_option("banana", &o), 0);
    }

    #[test]
    fn round_commands_are_first_words() {
        assert_eq!(command("Répète"), Command::Repeat);
        assert_eq!(command("skip it"), Command::Skip);
        assert_eq!(command("je ne sais pas"), Command::Skip);
        assert_eq!(command("back"), Command::Back);
        assert_eq!(command("8080 as in staging"), Command::Answer);
    }
}
