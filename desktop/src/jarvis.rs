//! Jarvis in the core: the conversation with Jarvis's Claude session, its
//! sandbox's state, and its proposals. The panel is a Luau screen
//! (`screens/jarvis.luau`) that draws what [`Jarvis::screen_data`] gives it
//! and asks with intents; the proposal dialog is the core's (`approval.rs`).
//!
//! Ported from the web UI (`assets/js/concierge.js`, `assets/js/jarvis.js`):
//! * **Talking.** What the person types or says is masked
//!   (`POST /api/voice/anonymise`), then typed into Jarvis's agent session
//!   (`POST /api/chat/push {text, name: "sbxw-jarvis"}`), as any agent's
//!   message. The typed text leaves the input only once it was taken.
//! * **Answers.** The agent's `session` events on `/api/stream`: when its
//!   turn ends (`state` goes from `working` or `attention` to `idle`) with a
//!   `reply`, that reply is Jarvis's answer (chat-core.js, `chatIngest`). The
//!   first look comes from `GET /api/sessions` (last prompt and reply).
//! * **Proposals.** `GET /api/jarvis` and the `jarvis` bus event (one
//!   proposal's `ui_view`). A new pending one opens the dialog and arms the
//!   next yes or no: a low-risk one is approved by a spoken or typed yes
//!   (`via: "voice"`), anything else only by a click on its dialog. The
//!   daemon enforces the same rule (409).

use serde_json::{json, Value as J};
use std::collections::HashSet;

/// The daemon's own Jarvis sandbox (`sbxw_core::jarvis::SANDBOX`).
pub const SANDBOX: &str = "sbxw-jarvis";
/// Panel width bounds and default, in points.
pub const MIN_W: f32 = 280.0;
pub const MAX_W: f32 = 720.0;
pub const DEFAULT_W: f32 = 380.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Me,
    Jarvis,
    /// A short line from the core (an approval's outcome, a click needed).
    Note,
    Error,
}

impl Who {
    pub fn name(self) -> &'static str {
        match self {
            Who::Me => "me",
            Who::Jarvis => "jarvis",
            Who::Note => "note",
            Who::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Msg {
    pub id: u64,
    pub who: Who,
    pub text: String,
}

/// Where the words came from: the panel's input, or the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Typed,
    Spoken,
}

/// What the words heard mean.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// A yes to the armed low-risk proposal: approve it (`via: "voice"`).
    Approve(String),
    /// A no to the armed proposal: refuse it.
    Refuse(String),
    /// A yes to a proposal that needs a click: say so, approve nothing.
    ClickNeeded(String),
    /// A message for Jarvis.
    Send(String),
}

/// The web UI's words (concierge.js, `VC_YES`, `VC_NO`): the first word decides.
pub fn is_yes(text: &str) -> bool {
    first_word_in(text, &["oui", "ouais", "ok", "okay", "d'accord", "vas-y", "envoie", "confirme", "yes", "send", "go"])
}
pub fn is_no(text: &str) -> bool {
    first_word_in(text, &["non", "annule", "stop", "laisse", "no", "cancel"])
}

fn first_word_in(text: &str, words: &[&str]) -> bool {
    let t = text.trim_start().to_lowercase();
    words.iter().any(|w| {
        t.strip_prefix(w).is_some_and(|rest| rest.chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_')))
    })
}

/// A proposal is approved on screen only: high risk, or a host command to acknowledge.
pub fn screen_only(p: &J) -> bool {
    p["risk"] != "low" || needs_ack(p)
}

pub fn needs_ack(p: &J) -> bool {
    p["needs_ack"] == true || p["changes"].as_array().is_some_and(|c| c.iter().any(|c| c["host_command"].is_string()))
}

pub fn pending(p: &J) -> bool {
    p["state"] == "pending"
}

pub struct Jarvis {
    pub open: bool,
    pub width: f32,
    /// `unknown` until asked, then the daemon's `absent|creating|starting|running|stopped|failed`.
    pub status: String,
    pub status_error: Option<String>,
    pub log: Vec<Msg>,
    next_id: u64,
    /// Jarvis's session is working on a turn.
    pub working: bool,
    session_state: String,
    seeded: bool,
    /// Proposals as the daemon last sent them, oldest first.
    pub proposals: Vec<J>,
    /// The proposal the dialog shows.
    pub shown: Option<String>,
    /// Host commands the person ticked as read, by proposal.
    pub acked: HashSet<String>,
    /// The last refused answer to the shown proposal, shown in the dialog.
    pub dialog_error: Option<(String, String)>,
    /// An answer is on its way to the daemon.
    pub busy: bool,
    /// The proposal the next yes or no answers.
    pub armed: Option<String>,
    /// A message is on its way.
    pub sending: bool,
    /// `GET /api/voice`, or none (a daemon without voice, or not asked yet).
    pub voice: Option<J>,
    pub recording: bool,
    pub level: f32,
    /// The message being read aloud.
    pub reading: Option<u64>,
    /// Why push-to-talk has no key, when it has none.
    pub key_note: Option<String>,
    /// The push-to-talk key's name, as the daemon names it (`F8`).
    pub key_name: String,
}

impl Default for Jarvis {
    fn default() -> Self {
        Jarvis {
            open: false,
            width: DEFAULT_W,
            status: "unknown".into(),
            status_error: None,
            log: vec![],
            next_id: 1,
            working: false,
            session_state: String::new(),
            seeded: false,
            proposals: vec![],
            shown: None,
            acked: HashSet::new(),
            dialog_error: None,
            busy: false,
            armed: None,
            sending: false,
            voice: None,
            recording: false,
            level: 0.0,
            reading: None,
            key_note: None,
            key_name: "F8".into(),
        }
    }
}

/// What happened when a proposal came in, for the shell to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum ProposalNews {
    None,
    /// A new pending one: the dialog opens, its summary may be read aloud.
    New(String),
    /// A pending one was answered (here, by voice, or in another client).
    Settled(String, String),
}

impl Jarvis {
    pub fn say(&mut self, who: Who, text: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.log.push(Msg { id, who, text: text.into() });
        // The conversation keeps its last 200 lines.
        if self.log.len() > 200 {
            self.log.drain(..self.log.len() - 200);
        }
        id
    }

    pub fn running(&self) -> bool {
        self.status == "running"
    }

    pub fn proposal(&self, id: &str) -> Option<&J> {
        self.proposals.iter().find(|p| p["id"] == id)
    }

    pub fn pending_list(&self) -> Vec<&J> {
        let mut v: Vec<&J> = self.proposals.iter().filter(|p| pending(p)).collect();
        v.sort_by_key(|p| p["created"].as_u64().unwrap_or(0));
        v
    }

    /// What words heard or typed mean, given the armed proposal. A word that
    /// is neither yes nor no disarms it: it is a message, and the proposal
    /// waits in its dialog (concierge.js, `vcHear`).
    pub fn hear(&mut self, text: &str) -> Heard {
        if let Some(id) = self.armed.clone() {
            match self.proposal(&id).filter(|p| pending(p)).cloned() {
                Some(p) if is_yes(text) => return if screen_only(&p) { Heard::ClickNeeded(id) } else { Heard::Approve(id) },
                Some(_) if is_no(text) => return Heard::Refuse(id),
                _ => self.armed = None,
            }
        }
        Heard::Send(text.trim().to_string())
    }

    /// One `session` event (a `SessionInfo`): Jarvis's answer when its turn
    /// ended with one. Other sandboxes and threads are not Jarvis.
    pub fn ingest_session(&mut self, info: &J) -> Option<String> {
        if info["sandbox"] != SANDBOX || info["thread"].is_string() {
            return None;
        }
        let state = info["state"].as_str().unwrap_or("").to_string();
        let prev = std::mem::replace(&mut self.session_state, if state == "exited" { "idle".into() } else { state.clone() });
        self.working = state == "working";
        let reply = info["reply"].as_str().filter(|r| !r.trim().is_empty())?;
        if state == "idle" && (prev == "working" || prev == "attention") {
            let dup = self.log.iter().rev().find(|m| m.who == Who::Jarvis).is_some_and(|m| m.text == reply);
            if !dup {
                self.say(Who::Jarvis, reply);
                return Some(reply.to_string());
            }
        }
        None
    }

    /// The first look (`GET /api/sessions`): the last prompt and its answer,
    /// when the conversation is still empty.
    pub fn seed_sessions(&mut self, list: &[J]) {
        let Some(info) = list.iter().find(|i| i["sandbox"] == SANDBOX && !i["thread"].is_string()) else { return };
        self.session_state = info["state"].as_str().unwrap_or("").to_string();
        self.working = self.session_state == "working";
        if self.seeded || !self.log.is_empty() {
            return;
        }
        self.seeded = true;
        if let Some(q) = info["last_input"].as_str().filter(|s| !s.trim().is_empty()) {
            self.say(Who::Me, q);
            if let Some(r) = info["reply"].as_str().filter(|s| !s.trim().is_empty()) {
                self.say(Who::Jarvis, r);
            }
        }
    }

    /// One proposal as the daemon sent it. `seed`: the first list, whose
    /// pending ones open the dialog but are not news to announce.
    pub fn ingest_proposal(&mut self, p: J, seed: bool) -> ProposalNews {
        let Some(id) = p["id"].as_str().map(str::to_string) else { return ProposalNews::None };
        let prev = self.proposals.iter().position(|x| x["id"] == id.as_str());
        let was_pending = prev.is_some_and(|i| pending(&self.proposals[i]));
        let now_pending = pending(&p);
        let state = p["state"].as_str().unwrap_or("").to_string();
        match prev {
            Some(i) => self.proposals[i] = p,
            None => self.proposals.push(p),
        }
        if self.proposals.len() > 60 {
            let drop = self.proposals.iter().position(|x| !pending(x) && x["id"] != self.shown.as_deref().unwrap_or(""));
            if let Some(i) = drop {
                self.proposals.remove(i);
            }
        }
        if prev.is_none() && now_pending {
            if self.shown.is_none() {
                self.shown = Some(id.clone());
            }
            self.armed = Some(id.clone());
            return if seed { ProposalNews::None } else { ProposalNews::New(id) };
        }
        if was_pending && !now_pending {
            if self.armed.as_deref() == Some(id.as_str()) {
                self.armed = None;
            }
            self.acked.remove(&id);
            if self.dialog_error.as_ref().is_some_and(|e| e.0 == id) {
                self.dialog_error = None;
            }
            return ProposalNews::Settled(id, state);
        }
        ProposalNews::None
    }

    /// The first list (`GET /api/jarvis`, newest first).
    pub fn seed_proposals(&mut self, list: Vec<J>) -> Vec<ProposalNews> {
        let first = self.proposals.is_empty();
        list.into_iter().rev().map(|p| self.ingest_proposal(p, first)).collect()
    }

    /// The dialog moves on: the next pending proposal, or closed.
    pub fn close_dialog(&mut self) {
        self.shown = None;
    }

    /// What the Luau screen sees (`data.jarvis`; `docs/screens.md`). No diff,
    /// no dialog state: the dialog is the core's.
    pub fn screen_data(&self, audio: bool) -> J {
        let voice = self.voice.as_ref();
        let speaks = audio && voice.is_some_and(|v| v["ready"] == true && v["speech"]["supported"] == true);
        let hears = audio && voice.is_some_and(|v| v["ready"] == true);
        json!({
            "open": self.open,
            "status": self.status,
            "error": self.status_error,
            "working": self.working,
            "sending": self.sending,
            "log": self.log.iter().map(|m| json!({ "id": m.id, "who": m.who.name(), "text": m.text, "reading": self.reading == Some(m.id) })).collect::<Vec<_>>(),
            "pending": self.pending_list().iter().map(|p| json!({
                "id": p["id"], "summary": p["summary"], "risk": if screen_only(p) { "high" } else { "low" },
            })).collect::<Vec<_>>(),
            "voice": {
                "installed": voice.is_some(),
                "hears": hears,
                "speaks": speaks,
                "recording": self.recording,
                "level": self.level,
                "reading": self.reading.is_some(),
                "key": self.key_name,
                "key_note": self.key_note,
            },
        })
    }
}

/// The daemon's push-to-talk key name (`[voice] push_to_talk_key`, a
/// `KeyboardEvent.code` such as `F8`) as Rune's key. Only F1 to F24, which
/// a terminal can give up without losing anything typed.
pub fn key_of(code: &str) -> Result<&'static [rune::Key], String> {
    static KEYS: [rune::Key; 24] = [
        rune::Key::Function(1),
        rune::Key::Function(2),
        rune::Key::Function(3),
        rune::Key::Function(4),
        rune::Key::Function(5),
        rune::Key::Function(6),
        rune::Key::Function(7),
        rune::Key::Function(8),
        rune::Key::Function(9),
        rune::Key::Function(10),
        rune::Key::Function(11),
        rune::Key::Function(12),
        rune::Key::Function(13),
        rune::Key::Function(14),
        rune::Key::Function(15),
        rune::Key::Function(16),
        rune::Key::Function(17),
        rune::Key::Function(18),
        rune::Key::Function(19),
        rune::Key::Function(20),
        rune::Key::Function(21),
        rune::Key::Function(22),
        rune::Key::Function(23),
        rune::Key::Function(24),
    ];
    let n = code.strip_prefix('F').and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=24).contains(n));
    match n {
        Some(n) => Ok(std::slice::from_ref(&KEYS[n - 1])),
        None => Err(format!("push_to_talk_key \"{code}\" is not a function key (F1 to F24): no key, use the talk button")),
    }
}

/// Text read aloud: the Markdown's words, cut in pieces of at most `max`
/// characters at sentence or line ends (the daemon speaks 1000 at a time).
pub fn speech_pieces(text: &str, max: usize) -> Vec<String> {
    let blocks = crate::markdown::parse(text);
    let plain: String = blocks
        .iter()
        .filter(|b| !matches!(b, crate::markdown::Block::Code(_)))
        .map(|b| crate::markdown::plain(std::slice::from_ref(b)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = vec![];
    let mut cur = String::new();
    for sentence in plain.split_inclusive(['.', '!', '?', '\n']) {
        let mut s = sentence.to_string();
        while s.chars().count() > max {
            let cut: String = s.chars().take(max).collect();
            let at = cut.rfind(' ').filter(|&i| i > 0).unwrap_or(cut.len());
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur).trim().to_string());
            }
            out.push(s[..at].trim().to_string());
            s = s[at..].to_string();
        }
        if cur.chars().count() + s.chars().count() > max && !cur.trim().is_empty() {
            out.push(std::mem::take(&mut cur).trim().to_string());
        }
        cur.push_str(&s);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.retain(|p| !p.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(id: &str, risk: &str, ack: bool) -> J {
        let mut p = json!({ "id": id, "summary": format!("change {id}"), "risk": risk, "state": "pending", "created": 1, "changes": [] });
        if ack {
            p["needs_ack"] = true.into();
        }
        p
    }

    #[test]
    fn yes_and_no_are_first_words() {
        assert!(is_yes("Oui, vas-y") && is_yes("yes") && is_yes("  OK.") && is_yes("d'accord"));
        assert!(!is_yes("yesterday") && !is_yes("say yes") && !is_yes("okayish"));
        assert!(is_no("non merci") && is_no("No.") && !is_no("nothing"));
    }

    #[test]
    fn a_yes_approves_only_a_low_risk_proposal_and_anything_else_disarms() {
        let mut j = Jarvis::default();
        assert_eq!(j.ingest_proposal(prop("a", "low", false), false), ProposalNews::New("a".into()));
        assert_eq!(j.hear("oui"), Heard::Approve("a".into()));
        assert_eq!(j.hear("non"), Heard::Refuse("a".into()));
        assert_eq!(j.hear("what does it change?"), Heard::Send("what does it change?".into()));
        assert_eq!(j.hear("yes"), Heard::Send("yes".into()), "disarmed by the message in between");

        let mut j = Jarvis::default();
        j.ingest_proposal(prop("h", "high", false), false);
        assert_eq!(j.hear("yes"), Heard::ClickNeeded("h".into()));
        assert_eq!(j.hear("yes please"), Heard::ClickNeeded("h".into()), "still armed: said again");
        let mut j = Jarvis::default();
        j.ingest_proposal(prop("c", "low", true), false);
        assert_eq!(j.hear("ok"), Heard::ClickNeeded("c".into()), "a host command is never approved by a yes");
    }

    #[test]
    fn a_settled_proposal_is_news_once_and_disarms() {
        let mut j = Jarvis::default();
        j.ingest_proposal(prop("a", "low", false), false);
        let mut done = prop("a", "low", false);
        done["state"] = "applied".into();
        assert_eq!(j.ingest_proposal(done.clone(), false), ProposalNews::Settled("a".into(), "applied".into()));
        assert_eq!(j.ingest_proposal(done, false), ProposalNews::None);
        assert!(j.armed.is_none());
        assert_eq!(j.hear("yes"), Heard::Send("yes".into()));
        // The first list: pending ones show, nothing announced.
        let mut j = Jarvis::default();
        assert_eq!(j.seed_proposals(vec![prop("b", "low", false)]), vec![ProposalNews::None]);
        assert_eq!(j.shown.as_deref(), Some("b"));
    }

    #[test]
    fn answers_come_from_jarvis_session_turns() {
        let mut j = Jarvis::default();
        let ev = |state: &str, reply: Option<&str>| json!({ "sandbox": SANDBOX, "state": state, "reply": reply });
        assert_eq!(j.ingest_session(&ev("working", None)), None);
        assert!(j.working);
        assert_eq!(j.ingest_session(&ev("idle", Some("Hello."))), Some("Hello.".into()));
        assert_eq!(j.ingest_session(&ev("idle", Some("Hello."))), None, "an idle repeat is not a new answer");
        assert_eq!(j.ingest_session(&json!({ "sandbox": "web", "state": "idle", "reply": "x" })), None);
        assert_eq!(j.ingest_session(&json!({ "sandbox": SANDBOX, "thread": "t", "state": "idle", "reply": "x" })), None);
        assert_eq!(j.log.iter().map(|m| (m.who, m.text.as_str())).collect::<Vec<_>>(), vec![(Who::Jarvis, "Hello.")]);
        let mut j = Jarvis::default();
        j.seed_sessions(&[json!({ "sandbox": SANDBOX, "state": "idle", "last_input": "hi", "reply": "Hello." })]);
        assert_eq!(j.log.len(), 2);
    }

    #[test]
    fn push_to_talk_keys() {
        assert_eq!(key_of("F8").unwrap(), &[rune::Key::Function(8)]);
        assert!(key_of("KeyJ").unwrap_err().contains("not a function key"));
        assert!(key_of("F25").is_err());
    }

    #[test]
    fn speech_is_cut_in_pieces_the_daemon_takes() {
        let long = "One sentence here. ".repeat(120);
        let p = speech_pieces(&long, 1000);
        assert!(p.len() >= 3 && p.iter().all(|x| x.chars().count() <= 1000), "{:?}", p.iter().map(|x| x.len()).collect::<Vec<_>>());
        assert_eq!(speech_pieces("**Bold** and `code`.\n\n```\nnot read\n```", 1000), vec!["Bold and code."]);
        assert!(speech_pieces(&"x".repeat(2500), 1000).iter().all(|x| x.chars().count() <= 1000));
    }
}
