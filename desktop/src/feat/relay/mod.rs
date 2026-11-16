//! The relay: what an agent cannot settle from inside its own sandbox and
//! asks the person for (`assets/js/relay.js`, `docs/relay.md`), and the
//! multiple-choice questions agents wait on (`AskUserQuestion`,
//! `assets/js/chat-core.js`, `chatQuestion`).
//!
//! Kinds of relay request (`GET /api/relay`, the `relay` stream event):
//!
//! * **question**: about another sandbox's workspace. The person routes it
//!   to a running sandbox (or answers it), reads what comes back, edits it,
//!   and releases it, or refuses.
//! * **screenshot**: the person attaches images (paste, drop, file dialog)
//!   and/or words, or refuses. Never routed.
//! * **ask**: a round of questions for the person (`ask_user_questions`,
//!   `/grill-me`), every question on one form; may be routed to a sandbox for
//!   a draft the person edits and releases. Masked before it leaves.
//! * **mac**: a yes or a no to something on the virtual Mac.
//!
//! The rule of this area: **routing a request and releasing anything move
//! information between projects**, so both are intents of risk Approval: the
//! core's dialog says what leaves (the text itself, the images' count and
//! sizes) and where it goes, from the daemon's own list and the core's copy
//! of what was typed and attached. Refusing is direct. A request id or a
//! target the daemon did not list is refused. A path is never an argument:
//! files only come from the core (a drop or the native dialog).
//!
//! What the person types and attaches lives here ([`State`]), not in the
//! screen: the screen shows `data.relay_ui` and changes it one intent at a
//! time; images never reach Luau, only their labels.

pub mod round;
pub mod shots;
mod standin;
#[cfg(test)]
mod tests;

use super::{Action, Asked, Call, Confirm, Ctx, Data, Extra, Feature, Place, Prepare, ScreenDecl, Slot};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Map, Value as J};
use shots::Shot;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

pub const SCREEN: &str = "relay";
/// `data.relay_ui`: what the person typed and attached, as the screen sees it.
pub const UI: &str = "relay_ui";
pub const JARVIS: &str = "sbxw-jarvis";

const FIELDS: &[&str] = &["own", "review", "caption", "answer"];

pub const FEATURE: Feature = Feature {
    name: "relay",
    data: &[
        Data { key: "relay", path: "/api/relay", on: &["relay"], every: 0, lazy: false },
        Data { key: "agent_sessions", path: "/api/sessions", on: &["session"], every: 0, lazy: false },
    ],
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../../screens/relay.luau"), slot: Slot::Panel { title: "Requests", icon: "chat", width: 420.0 } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    on_data: Some(on_data),
    extras: Some(extras),
    standin: Some(standin::route),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "relay_open", risk: Risk::Direct, args: &[("id", Arg::Str, false)], doc: "Opens the Requests panel, on that request (none: the oldest waiting)." },
    Spec { name: "relay_later", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Puts a request aside (it stays in the list and the badge), or closes a settled one." },
    Spec {
        name: "relay_route",
        risk: Risk::Approval("Send a request to another sandbox"),
        args: &[("id", Arg::Str, true), ("to", Arg::Str, true)],
        doc: "Sends a question (or a round, for a draft) to another running sandbox's agent, after the core's dialog shows the text and where it goes.",
    },
    Spec { name: "relay_deny", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Refuses a request: nothing is released, and the agent is told not to ask again." },
    Spec {
        name: "relay_set",
        risk: Risk::Direct,
        args: &[("id", Arg::Str, true), ("field", Arg::OneOf(FIELDS), true), ("value", Arg::Str, false), ("index", Arg::Int(1, 64), false)],
        doc: "Sets what the person typed: `own` (their answer to a question), `review` (a routed answer, edited), `caption` (words with a screenshot), `answer` (question `index` of a round). Give it on every edit.",
    },
    Spec { name: "relay_use_rec", risk: Risk::Direct, args: &[("id", Arg::Str, true), ("index", Arg::Int(1, 64), true)], doc: "Fills question `index` of a round with the agent's recommendation." },
    Spec { name: "relay_clear", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Empties every answer of a round (a draft cleared, to answer yourself)." },
    Spec {
        name: "relay_send",
        risk: Risk::Approval("Release an answer to a sandbox"),
        args: &[("id", Arg::Str, true)],
        doc: "Releases what the person typed or attached to the asking sandbox (a round's answers masked first), or approves a virtual Mac request, after the core's dialog shows exactly what leaves.",
    },
    Spec { name: "relay_shot_choose", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Opens the system's file dialog to attach images to a screenshot request." },
    Spec { name: "relay_shot_paste", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Attaches the image on the clipboard to a screenshot request." },
    Spec { name: "relay_shot_remove", risk: Risk::Direct, args: &[("id", Arg::Str, true), ("index", Arg::Int(1, 6), false)], doc: "Removes attached image `index` (none: all of them)." },
    Spec {
        name: "agent_pick",
        risk: Risk::Direct,
        args: &[("sandbox", Arg::Str, true), ("thread", Arg::Str, false), ("step", Arg::Int(1, 16), true), ("option", Arg::Int(1, 32), true)],
        doc: "Picks option `option` of question `step` an agent waits on (`AskUserQuestion`); once every question has a pick, the answer is typed into that session.",
    },
];

// ---------------------------------------------------------------------------
// State

/// A round's form: its answers, and the draft they were filled from.
#[derive(Debug, Default, Clone)]
struct Form {
    basis: String,
    answers: Vec<String>,
}

/// What the person typed, attached and picked. One per app.
#[derive(Debug, Default)]
pub struct State {
    /// The request the panel shows, when the person picked one.
    current: Option<String>,
    /// Settled requests the person closed, and requests already announced.
    closed: HashSet<String>,
    announced: HashSet<String>,
    forms: HashMap<String, Form>,
    own: HashMap<String, String>,
    /// A routed answer as edited: (the answer it was edited from, the text).
    review: HashMap<String, (String, String)>,
    caption: HashMap<String, String>,
    shots: HashMap<String, Vec<Shot>>,
    /// AskUserQuestion picks, by session key: (the question's text, pick per step).
    picks: HashMap<String, (String, Vec<Option<usize>>)>,
    /// Answers on their way, by session key (the labels sent).
    answering: HashMap<String, String>,
}

pub static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    STATE.lock().unwrap().act(cx, a)
}
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    STATE.lock().unwrap().on_answer(cx, call, r)
}
fn on_data(cx: &Ctx, key: &str, old: &J, new: &J) -> Vec<Action> {
    STATE.lock().unwrap().on_data(cx, key, old, new)
}

// ---------------------------------------------------------------------------
// Reading the daemon's data

pub fn requests(cx: &Ctx) -> Vec<J> {
    cx.get("relay")["requests"].as_array().cloned().unwrap_or_default()
}

fn find(cx: &Ctx, id: &str) -> Result<J, String> {
    requests(cx).into_iter().find(|r| r["id"] == id).ok_or_else(|| format!("request {id} is not in the daemon's list: it was settled long ago, or never existed"))
}

fn s<'a>(r: &'a J, k: &str) -> &'a str {
    r[k].as_str().unwrap_or("")
}

pub fn settled(r: &J) -> bool {
    matches!(s(r, "state"), "approved" | "denied")
}

/// Waiting on the person (not out with a sandbox, not settled), oldest first.
pub fn queue(reqs: &[J]) -> Vec<&J> {
    let mut v: Vec<&J> = reqs.iter().filter(|r| !settled(r) && s(r, "state") != "routed").collect();
    v.sort_by_key(|r| r["created_ms"].as_u64().unwrap_or(0));
    v
}

/// Who asks, with its thread.
pub fn who(r: &J) -> String {
    match r["from_thread"].as_str() {
        Some(t) if !t.is_empty() => format!("{} · {t}", s(r, "from")),
        _ => s(r, "from").to_string(),
    }
}

/// The first line of a question, short (a notice, a corner row).
pub fn gist(q: &str) -> String {
    let line = q.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_start_matches('❓').trim();
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(60).collect::<String>())
    } else {
        line.to_string()
    }
}

/// A running sandbox that is not the one asking: the sandboxes a request can go to.
fn can_answer(cx: &Ctx, r: &J, to: &str) -> Result<(), String> {
    if to == s(r, "from") {
        return Err(format!("{to} is the sandbox that asked"));
    }
    let sb = cx.sandbox(to).ok_or_else(|| format!("{to} is not a sandbox sbxw knows"))?;
    if sb["status"] != "running" {
        return Err(format!("{to} is not running: start it first"));
    }
    if sb["unreachable_since"].is_number() {
        return Err(format!("{to} is not reachable right now"));
    }
    Ok(())
}

/// Sessions waiting on an `AskUserQuestion` the app can show: sbxw's own
/// sessions (not a client of their own), not Jarvis's (its panel shows its
/// questions), the newest per sandbox and thread.
pub fn asking(cx: &Ctx) -> Vec<J> {
    let mut by: Vec<(String, J)> = vec![];
    for i in cx.get("agent_sessions").as_array().cloned().unwrap_or_default() {
        if i["state"] != "attention" {
            continue;
        }
        let thread = i["thread"].as_str().filter(|t| !t.is_empty());
        if i["origin"] == "remote" && thread.is_none() {
            continue;
        }
        if s(&i, "sandbox") == JARVIS && thread.is_none() {
            continue;
        }
        let key = session_key(s(&i, "sandbox"), thread);
        match by.iter_mut().find(|(k, _)| *k == key) {
            Some(e) if e.1["ts"].as_u64() < i["ts"].as_u64() => e.1 = i,
            Some(_) => {}
            None => by.push((key, i)),
        }
    }
    by.into_iter().map(|(_, i)| i).collect()
}

pub fn session_key(sandbox: &str, thread: Option<&str>) -> String {
    match thread {
        Some(t) => format!("{sandbox}::{t}"),
        None => sandbox.to_string(),
    }
}

fn steps(i: &J) -> Vec<J> {
    match i["steps"].as_array() {
        Some(s) if !s.is_empty() => s.clone(),
        _ if i["question"].is_object() => vec![i["question"].clone()],
        _ => vec![],
    }
}

fn signature(steps: &[J]) -> String {
    steps.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------------------
// Intents

impl State {
    fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        // Files only ever come from the core (a drop, the native dialog): an
        // intent naming one is refused whatever the registry let through.
        if a.args.keys().any(|k| matches!(k.as_str(), "path" | "paths" | "file" | "files" | "url" | "data")) {
            return Err(format!("{}: a file or image is never an argument; attach it by dropping it, pasting it or with the file dialog", a.name));
        }
        let id = a.req("id").to_string();
        let mut out = match a.name {
            "relay_open" => {
                if let Some(id) = a.s("id") {
                    find(cx, id)?;
                    self.current = Some(id.to_string());
                    self.closed.remove(id);
                }
                vec![Action::OpenPanel(SCREEN.into()), Action::Refresh("relay")]
            }
            "relay_later" => {
                let r = find(cx, &id)?;
                if settled(&r) {
                    self.closed.insert(id.clone());
                    self.forget(&id);
                }
                if self.current.as_deref() == Some(id.as_str()) {
                    self.current = None;
                }
                vec![]
            }
            "relay_route" => return self.route(cx, &id, a.req("to")),
            "relay_deny" => {
                let r = find(cx, &id)?;
                if settled(&r) {
                    return Err(format!("request {id} is already settled"));
                }
                vec![Action::Call(Call::post(format!("/api/relay/{}/deny", enc(&id)), json!({}), format!("refuse {}'s request", s(&r, "from"))).tag("deny"))]
            }
            "relay_set" => {
                let r = find(cx, &id)?;
                let v = a.s("value").unwrap_or("").to_string();
                match a.req("field") {
                    "own" => drop(self.own.insert(id.clone(), v)),
                    "caption" => drop(self.caption.insert(id.clone(), v)),
                    "review" => drop(self.review.insert(id.clone(), (s(&r, "answer").to_string(), v))),
                    _ => {
                        let n = round::parse(s(&r, "question")).questions.len();
                        let i = a.i("index").ok_or("which question: give `index`")? as usize;
                        if i > n {
                            return Err(format!("this round has {n} questions"));
                        }
                        let f = self.form(&r);
                        f.answers.resize(n, String::new());
                        f.answers[i - 1] = v;
                    }
                }
                vec![]
            }
            "relay_use_rec" => {
                let r = find(cx, &id)?;
                let rd = round::parse(s(&r, "question"));
                let i = a.i("index").unwrap_or(1) as usize;
                let q = rd.questions.get(i - 1).ok_or_else(|| format!("this round has {} questions", rd.questions.len()))?;
                let rec = q.rec.clone();
                let n = rd.questions.len();
                let f = self.form(&r);
                f.answers.resize(n, String::new());
                f.answers[i - 1] = rec;
                vec![]
            }
            "relay_clear" => {
                let r = find(cx, &id)?;
                let f = self.form(&r);
                f.answers.iter_mut().for_each(String::clear);
                vec![]
            }
            "relay_send" => return self.send(cx, &id),
            "relay_shot_choose" | "relay_shot_paste" => {
                let r = find(cx, &id)?;
                if s(&r, "kind") != "screenshot" || s(&r, "state") != "pending" {
                    return Err(format!("request {id} does not ask for a screenshot"));
                }
                if self.shots.get(&id).map(Vec::len).unwrap_or(0) >= shots::MAX_SHOTS {
                    return Err(format!("{} images is the most one reply carries: remove one first", shots::MAX_SHOTS));
                }
                self.current = Some(id.clone());
                vec![intake::ask(a.name == "relay_shot_paste")]
            }
            "relay_shot_remove" => {
                find(cx, &id)?;
                match a.i("index") {
                    Some(i) => {
                        let held = self.shots.entry(id.clone()).or_default();
                        if (i as usize) <= held.len() {
                            held.remove(i as usize - 1);
                        }
                    }
                    None => drop(self.shots.remove(&id)),
                }
                vec![]
            }
            "agent_pick" => return self.pick(cx, a),
            other => return Err(format!("{other}: not a relay intent")),
        };
        out.push(self.ui_action(cx));
        Ok(out)
    }

    fn form(&mut self, r: &J) -> &mut Form {
        let id = s(r, "id").to_string();
        let rd = round::parse(s(r, "question"));
        let answered = s(r, "state") == "answered";
        let draft = s(r, "answer").to_string();
        let f = self.forms.entry(id).or_default();
        // A draft arriving replaces the form on purpose (relay.js); a form
        // the person filled is kept otherwise.
        if answered && f.basis != draft {
            *f = Form { basis: draft.clone(), answers: round::fill(&rd, &draft) };
        }
        f.answers.resize(rd.questions.len(), String::new());
        f
    }

    fn forget(&mut self, id: &str) {
        self.forms.remove(id);
        self.own.remove(id);
        self.review.remove(id);
        self.caption.remove(id);
        self.shots.remove(id);
    }

    fn route(&mut self, cx: &Ctx, id: &str, to: &str) -> Result<Vec<Action>, String> {
        let r = find(cx, id)?;
        match s(&r, "kind") {
            "screenshot" => return Err(format!("request {id} asks for a screenshot: no other sandbox can see this screen")),
            "mac" => return Err(format!("request {id} asks you to approve something on the virtual Mac: only you can")),
            _ => {}
        }
        if settled(&r) {
            return Err(format!("request {id} is already settled"));
        }
        can_answer(cx, &r, to)?;
        let ask = s(&r, "kind") == "ask";
        let from = who(&r);
        let title = if ask { format!("Ask {to} to draft the answers for {from}?") } else { format!("Send {from}'s question to {to}?") };
        let mut c = Confirm::new(title, Call::post(format!("/api/relay/{}/route", enc(id)), json!({ "to": to }), format!("send {from}'s request to {to}")).tag("route"))
            .line(format!("From: {from}"))
            .line(format!("To: {to}. A one-line notice is typed into its agent, which then reads the text below, framed as untrusted text from another agent."));
        if let Some(prev) = r["to"].as_str().filter(|p| !p.is_empty() && *p != to) {
            c = c.line(format!("It was with {prev}: what {prev} wrote is dropped."));
        }
        c = c
            .line(if ask { format!("Its draft comes back to you; {from} receives only what you send.") } else { format!("Its answer comes back to you; nothing reaches {from} until you release it.") })
            .preview(s(&r, "question").to_string(), false)
            .labels(&format!("Send to {to}"), "Cancel");
        Ok(vec![c.into_action()])
    }

    fn send(&mut self, cx: &Ctx, id: &str) -> Result<Vec<Action>, String> {
        let r = find(cx, id)?;
        if settled(&r) {
            return Err(format!("request {id} is already settled"));
        }
        let from = who(&r);
        let path = format!("/api/relay/{}/approve", enc(id));
        let kind = s(&r, "kind");
        let state = s(&r, "state");
        match kind {
            "mac" => {
                if state != "pending" {
                    return Err("sbxw is already carrying it out".into());
                }
                Ok(vec![Confirm::new(format!("Approve {from}'s request on the virtual Mac?"), Call::post(path, json!({}), format!("approve {from}'s virtual Mac request")).tag("send"))
                    .line(format!("From: {from}"))
                    .line("sbxw carries it out (a boot, a published port) and tells the agent what came of it.")
                    .line("The virtual Mac is shared by every sandbox and has no personal or work account in it.")
                    .preview(s(&r, "question").to_string(), false)
                    .labels("Approve", "Cancel")
                    .into_action()])
            }
            "ask" => {
                if !matches!(state, "pending" | "answered") {
                    return Err(format!("this round is with {}: wait for its draft, or refuse", s(&r, "to")));
                }
                let rd = round::parse(s(&r, "question"));
                let answers = self.form(&r).answers.clone();
                let text = round::compose(&rd, &answers);
                if text.is_empty() {
                    return Err("No question answered: nothing to send".into());
                }
                let mut asked = Asked { name: "relay_send", args: Map::new() };
                asked.args.insert("id".into(), id.into());
                asked.args.insert("answer".into(), text.clone().into());
                Ok(vec![Action::Prepare(Box::new(Prepare {
                    title: format!("Masking personal data before it goes to {from}"),
                    call: Call::post("/api/voice/anonymise", json!({ "text": text }), "mask personal data").quiet().timeout(30),
                    asked,
                    build: build_ask,
                }))])
            }
            "screenshot" => {
                let held = self.shots.get(id).cloned().unwrap_or_default();
                let caption = self.caption.get(id).map(|c| c.trim().to_string()).unwrap_or_default();
                if held.is_empty() && caption.is_empty() {
                    return Err("Nothing to send: attach an image or describe it".into());
                }
                let mut body = json!({ "images": held.iter().map(Shot::data_url).collect::<Vec<_>>() });
                if !caption.is_empty() {
                    body["answer"] = caption.clone().into();
                }
                let n = held.len();
                let title = match (n, caption.is_empty()) {
                    (0, _) => format!("Send your description to {from}?"),
                    (1, true) => format!("Send 1 image to {from}?"),
                    (n, true) => format!("Send {n} images to {from}?"),
                    (1, false) => format!("Send 1 image and your words to {from}?"),
                    (n, false) => format!("Send {n} images and your words to {from}?"),
                };
                let mut c = Confirm::new(title, Call::post(path, body, format!("send the screenshot to {from}")).tag("send").timeout(60)).line(format!("To: {from}, which asked: \"{}\"", gist(s(&r, "question"))));
                for (i, sh) in held.iter().enumerate() {
                    c = c.line(format!("{}. {}", i + 1, sh.label()));
                }
                if n > 0 {
                    c = c
                        .line(format!("Total: {} in {n} image{}, in the order shown.", shots::weight(held.iter().map(Shot::bytes).sum()), if n == 1 { "" } else { "s" }))
                        .line(format!("The agent sees them in its tool result and keeps them in its sandbox as ~/.sbxw/shots/{id}-1.png and so on."));
                }
                if !caption.is_empty() {
                    c = c.preview(caption, false);
                }
                Ok(vec![c.labels(&format!("Send to {}", s(&r, "from")), "Cancel").into_action()])
            }
            _ => {
                let (text, by) = match state {
                    "pending" => (self.own.get(id).cloned().unwrap_or_default(), "you".to_string()),
                    "answered" => {
                        let answer = s(&r, "answer").to_string();
                        let edited = self.review.get(id).filter(|(basis, _)| *basis == answer).map(|(_, t)| t.clone());
                        let by = if edited.as_deref().is_some_and(|t| t != answer) { format!("{}, as you edited it", s(&r, "to")) } else { s(&r, "to").to_string() };
                        (edited.unwrap_or(answer), by)
                    }
                    _ => return Err(format!("this question is with {}: wait for its answer, or refuse", s(&r, "to"))),
                };
                let text = text.trim().to_string();
                if text.is_empty() {
                    return Err("Nothing to send: write an answer first".into());
                }
                Ok(vec![Confirm::new(format!("Send this answer to {from}?"), Call::post(path, json!({ "answer": text }), format!("send the answer to {from}")).tag("send"))
                    .line(format!("To: {from}, which asked: \"{}\"", gist(s(&r, "question"))))
                    .line(format!("Written by: {by}."))
                    .line(format!("{} characters, exactly as below.", text.chars().count()))
                    .preview(text, false)
                    .labels(&format!("Send to {}", s(&r, "from")), "Cancel")
                    .into_action()])
            }
        }
    }

    fn pick(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let sb = a.req("sandbox");
        let thread = a.s("thread").filter(|t| !t.is_empty());
        let key = session_key(sb, thread);
        let info = asking(cx).into_iter().find(|i| session_key(s(i, "sandbox"), i["thread"].as_str().filter(|t| !t.is_empty())) == key).ok_or_else(|| format!("{key} is not waiting on a question any more"))?;
        if info["answerable"] != true {
            return Err(format!("{key}'s question can only be answered in its terminal"));
        }
        let st = steps(&info);
        let (step, option) = (a.i("step").unwrap_or(1) as usize, a.i("option").unwrap_or(1) as usize);
        let q = st.get(step - 1).ok_or_else(|| format!("{key} asks {} question(s)", st.len()))?;
        let n_opts = q["options"].as_array().map(Vec::len).unwrap_or(0);
        if option > n_opts {
            return Err(format!("that question has {n_opts} options"));
        }
        if self.answering.contains_key(&key) {
            return Err("an answer is already on its way".into());
        }
        let sig = signature(&st);
        let e = self.picks.entry(key.clone()).or_insert_with(|| (sig.clone(), vec![]));
        if e.0 != sig {
            *e = (sig, vec![]);
        }
        e.1.resize(st.len(), None);
        e.1[step - 1] = Some(option);
        let mut out = vec![];
        if e.1.iter().all(Option::is_some) {
            let indices: Vec<usize> = e.1.iter().map(|p| p.unwrap()).collect();
            let labels: Vec<String> = indices.iter().zip(st.iter()).map(|(i, q)| q["options"][i - 1].as_str().unwrap_or("").to_string()).collect();
            let mode = match thread {
                Some(t) => format!("thread:{t}"),
                None => "claude".into(),
            };
            self.answering.insert(key.clone(), labels.join(" · "));
            out.push(Action::Call(Call::post("/api/answer", json!({ "sandbox": sb, "mode": mode, "index": indices[0], "indices": indices }), format!("answer {key}: {}", labels.join(" · "))).tag(&format!("pick:{key}")).timeout(30)));
        }
        out.push(self.ui_action(cx));
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Answers and news

    fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let ok = matches!(&r, Ok((c, b)) if (200..300).contains(c) && b["ok"] != false);
        let why = match &r {
            Ok((c, b)) => b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}")),
            Err(e) => e.clone(),
        };
        let mut v = vec![];
        if let Some(key) = call.tag.strip_prefix("pick:") {
            let labels = self.answering.remove(key).unwrap_or_default();
            self.picks.remove(key);
            v.push(if ok { Action::notice(format!("Answered {key}: {labels}")) } else { Action::error(format!("Answer not sent to {key}: {why}. Answer in its terminal instead.")) });
            v.push(Action::Refresh("agent_sessions"));
        } else {
            v.push(if ok { Action::notice(format!("{}: done", call.what)) } else { Action::error(format!("{}: {why}", call.what)) });
            if ok && call.tag == "send" {
                // What was attached has left: the core keeps no copy of someone's screen.
                if let Some(id) = call.path.split('/').nth(3) {
                    let id = id.to_string();
                    self.shots.remove(&id);
                }
            }
            v.push(Action::Refresh("relay"));
        }
        v.push(self.ui_action(cx));
        v
    }

    fn on_data(&mut self, cx: &Ctx, key: &str, old: &J, new: &J) -> Vec<Action> {
        let mut v = vec![];
        if key == "relay" {
            let before: Vec<J> = old["requests"].as_array().cloned().unwrap_or_default();
            let now: Vec<J> = new["requests"].as_array().cloned().unwrap_or_default();
            let first = old.is_null();
            let mut fresh = vec![];
            for r in &now {
                let id = s(r, "id").to_string();
                let prev = before.iter().find(|p| p["id"] == id.as_str());
                if settled(r) {
                    // Whatever was attached was sent or refused: dropped either way.
                    self.shots.remove(&id);
                    continue;
                }
                let st = s(r, "state");
                if prev.is_none() && st != "routed" && self.announced.insert(id.clone()) {
                    fresh.push(r.clone());
                } else if let Some(p) = prev {
                    let was = s(p, "state");
                    if was == "routed" && st == "answered" {
                        v.push(Action::Notice { text: format!("{} answered {}'s {}: review it", s(r, "to"), who(r), if s(r, "kind") == "ask" { "round" } else { "question" }), error: false, then: Some(("relay_open".into(), json!({ "id": id }))) });
                        if self.current.is_none() {
                            self.current = Some(id.clone());
                        }
                        v.push(Action::OpenPanel(SCREEN.into()));
                    } else if was == "routed" && st == "pending" {
                        v.push(Action::Notice { text: format!("{}'s request came back: {}", who(r), r["note"].as_str().unwrap_or("delivery failed")), error: true, then: Some(("relay_open".into(), json!({ "id": id }))) });
                    } else if st == "routed" && was != "routed" && self.current.as_deref() == Some(id.as_str()) {
                        // Out with a sandbox: the panel moves on to what waits on the person.
                        self.current = None;
                    }
                }
            }
            match fresh.as_slice() {
                [] => {}
                [r] if !first => {
                    let text = match s(r, "kind") {
                        "screenshot" => format!("{} asks for a screenshot: {}", who(r), gist(s(r, "question"))),
                        "ask" => {
                            let n = round::parse(s(r, "question")).questions.len();
                            format!("{} asks you {}", who(r), if n == 1 { "one question".to_string() } else { format!("{n} questions") })
                        }
                        "mac" => format!("{} asks to use the virtual Mac", who(r)),
                        _ => format!("{} asks: {}", who(r), gist(s(r, "question"))),
                    };
                    v.push(Action::Notice { text, error: false, then: Some(("relay_open".into(), json!({ "id": s(r, "id") }))) });
                    v.push(Action::OpenPanel(SCREEN.into()));
                }
                many => {
                    v.push(Action::Notice { text: format!("{} request{} from agents wait for you", many.len(), if many.len() == 1 { "" } else { "s" }), error: false, then: Some(("relay_open".into(), json!({}))) });
                    v.push(Action::OpenPanel(SCREEN.into()));
                }
            }
            // Forget what belongs to requests the daemon no longer lists.
            let live: HashSet<String> = now.iter().map(|r| s(r, "id").to_string()).collect();
            for map_id in self.forms.keys().chain(self.own.keys()).chain(self.review.keys()).chain(self.caption.keys()).chain(self.shots.keys()).cloned().collect::<Vec<_>>() {
                if !live.contains(&map_id) {
                    self.forget(&map_id);
                }
            }
            self.closed.retain(|id| live.contains(id));
        }
        if key == "agent_sessions" {
            let was: Vec<String> = asking(&Ctx { data: &json!({ "agent_sessions": old }) }).iter().map(|i| format!("{}{}", session_key(s(i, "sandbox"), i["thread"].as_str()), signature(&steps(i)))).collect();
            for i in asking(cx) {
                let k = session_key(s(&i, "sandbox"), i["thread"].as_str());
                if !old.is_null() && !was.contains(&format!("{k}{}", signature(&steps(&i)))) {
                    let q = steps(&i).first().and_then(|q| q["text"].as_str().map(str::to_string)).or_else(|| i["activity"].as_str().map(str::to_string)).unwrap_or_else(|| "Claude is waiting for you.".into());
                    v.push(Action::Notice { text: format!("{k} asks: {q}"), error: false, then: Some(("relay_open".into(), json!({}))) });
                }
            }
            let live: Vec<String> = asking(cx).iter().map(|i| session_key(s(i, "sandbox"), i["thread"].as_str())).collect();
            self.picks.retain(|k, _| live.contains(k));
        }
        v.push(self.ui_action(cx));
        v
    }

    // -----------------------------------------------------------------------
    // What the screen sees

    pub fn ui(&self, cx: &Ctx) -> J {
        let reqs = requests(cx);
        let mut rounds = Map::new();
        let mut forms = Map::new();
        let mut review = Map::new();
        for r in &reqs {
            let id = s(r, "id").to_string();
            if s(r, "kind") == "ask" {
                let rd = round::parse(s(r, "question"));
                let mut answers = match self.forms.get(&id) {
                    Some(f) if !(s(r, "state") == "answered" && f.basis != s(r, "answer")) => f.answers.clone(),
                    _ if s(r, "state") == "answered" => round::fill(&rd, s(r, "answer")),
                    _ => vec![],
                };
                answers.resize(rd.questions.len(), String::new());
                forms.insert(id.clone(), json!(answers));
                rounds.insert(id.clone(), rd.to_json());
            }
            if s(r, "state") == "answered" && s(r, "kind") != "ask" {
                let a = s(r, "answer");
                let t = self.review.get(&id).filter(|(b, _)| b == a).map(|(_, t)| t.as_str()).unwrap_or(a);
                review.insert(id, t.into());
            }
        }
        let shots: Map<String, J> = self.shots.iter().map(|(k, v)| (k.clone(), J::Array(v.iter().enumerate().map(|(i, s)| s.to_json(i + 1)).collect()))).collect();
        let asks: Vec<J> = asking(cx)
            .iter()
            .map(|i| {
                let thread = i["thread"].as_str().filter(|t| !t.is_empty());
                let key = session_key(s(i, "sandbox"), thread);
                let st = steps(i);
                let picks: Vec<J> = match self.picks.get(&key) {
                    Some((sig, p)) if *sig == signature(&st) => p.iter().map(|x| x.map(|n| J::from(n as u64)).unwrap_or(J::from(0))).collect(),
                    _ => vec![],
                };
                json!({
                    "key": key, "sandbox": s(i, "sandbox"), "thread": thread, "answerable": i["answerable"] == true,
                    "activity": i["activity"], "steps": st, "picks": picks, "sending": self.answering.get(&key),
                })
            })
            .collect();
        json!({
            "current": self.current,
            "closed": self.closed.iter().collect::<Vec<_>>(),
            "rounds": rounds,
            "forms": forms,
            "own": self.own,
            "review": review,
            "caption": self.caption,
            "shots": shots,
            "max_shots": shots::MAX_SHOTS,
            "asks": asks,
        })
    }

    fn ui_action(&self, cx: &Ctx) -> Action {
        Action::Set { key: UI.into(), value: self.ui(cx) }
    }

    /// Images the core handed over (a drop, a paste, the file dialog), for the
    /// screenshot request the panel shows. Each is checked and added in
    /// order; a refusal says why.
    pub fn take_images(&mut self, cx: &Ctx, items: Vec<(Vec<u8>, String)>) -> Vec<Action> {
        let reqs = requests(cx);
        let target = self
            .current
            .clone()
            .filter(|id| reqs.iter().any(|r| r["id"] == id.as_str() && s(r, "kind") == "screenshot" && s(r, "state") == "pending"))
            .or_else(|| queue(&reqs).into_iter().find(|r| s(r, "kind") == "screenshot").map(|r| s(r, "id").to_string()));
        let Some(id) = target else { return vec![Action::error("No screenshot request is waiting: nothing to attach that image to")] };
        let mut v = vec![];
        let held = self.shots.entry(id.clone()).or_default();
        for (bytes, source) in items {
            match shots::prepare(&bytes, &source).and_then(|sh| shots::admit(held, sh)) {
                Ok(()) => {}
                Err(e) => v.push(Action::error(e)),
            }
        }
        self.current = Some(id);
        v.push(self.ui_action(cx));
        v
    }
}

/// The dialog for a round's answers, once the daemon masked them: what it
/// shows is what is sent. A masking that failed sends nothing.
fn build_ask(cx: &Ctx, asked: &Asked, masked: &J) -> Result<Confirm, String> {
    let id = asked.req("id");
    let r = find(cx, id)?;
    if masked["ok"] == false {
        return Err(format!("Not sent: the masking of personal data failed ({}).", masked["error"].as_str().unwrap_or("no reason given")));
    }
    let safe = masked["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("Not sent: the masking of personal data gave nothing back.")?;
    let n = masked["masked"].as_u64().unwrap_or(0);
    let from = who(&r);
    let lines = safe.lines().count();
    let mut c = Confirm::new(format!("Send {lines} answer{} to {from}?", if lines == 1 { "" } else { "s" }), Call::post(format!("/api/relay/{}/approve", enc(id)), json!({ "answer": safe }), format!("send the answers to {from}")).tag("send"))
        .line(format!("To: {from}. Its tool call returns these lines; nothing is typed into a terminal."));
    if s(&r, "state") == "answered" {
        c = c.line(format!("Drafted by {}, as you left them.", s(&r, "to")));
    }
    c = c.line(if n > 0 { format!("{n} piece{} of personal data masked before sending.", if n == 1 { "" } else { "s" }) } else { "Personal data checked: nothing to mask.".to_string() });
    Ok(c.preview(safe.to_string(), false).labels(&format!("Send to {}", s(&r, "from")), "Cancel"))
}

/// Buttons and badges: the top bar counts what waits on the person; the
/// asking sandbox's row and pane bars say it asks.
fn extras(cx: &Ctx) -> Vec<Extra> {
    let reqs = requests(cx);
    let q = queue(&reqs);
    let asks = asking(cx);
    let out_with: Vec<&J> = reqs.iter().filter(|r| s(r, "state") == "routed").collect();
    let mut v = vec![];
    let n = q.len() + asks.len();
    if n > 0 {
        v.push(Extra {
            place: Place::Topbar,
            id: "relay".into(),
            label: "Requests".into(),
            icon: Some("chat".into()),
            tone: "amber".into(),
            help: "Agents wait for you: questions, screenshots, choices".into(),
            count: Some(n as u64),
            intent: Some(("relay_open".into(), json!({}))),
        });
    } else if !out_with.is_empty() {
        v.push(Extra {
            place: Place::Topbar,
            id: "relay".into(),
            label: "Relay".into(),
            icon: Some("chat".into()),
            tone: "blue".into(),
            help: format!("{} answering for another sandbox; you review the answer before it is sent", out_with.iter().map(|r| s(r, "to")).collect::<Vec<_>>().join(", ")),
            count: Some(out_with.len() as u64),
            intent: Some(("relay_open".into(), json!({}))),
        });
    }
    let mut by: Vec<(String, u64, Option<String>)> = vec![];
    for r in &q {
        let from = s(r, "from").to_string();
        match by.iter_mut().find(|e| e.0 == from) {
            Some(e) => e.1 += 1,
            None => by.push((from, 1, Some(s(r, "id").to_string()))),
        }
    }
    for i in &asks {
        let sb = s(i, "sandbox").to_string();
        match by.iter_mut().find(|e| e.0 == sb) {
            Some(e) => e.1 += 1,
            None => by.push((sb, 1, None)),
        }
    }
    for (sb, n, first) in by {
        let args = match &first {
            Some(id) => json!({ "id": id }),
            None => json!({}),
        };
        v.push(Extra { place: Place::Sandbox(sb.clone()), id: "relay".into(), label: "?".into(), icon: None, tone: "amber".into(), help: format!("{sb} waits for your answer"), count: Some(n), intent: Some(("relay_open".into(), args.clone())) });
        v.push(Extra { place: Place::Pane(sb.clone()), id: "relay".into(), label: "asks you".into(), icon: Some("chat".into()), tone: "amber".into(), help: "Answer in the Requests panel".into(), count: Some(n), intent: Some(("relay_open".into(), args)) });
    }
    v
}

/// Where images come from. The core's file intake (a drop, a paste, the
/// native dialog) is being added by the core's owner; until then the
/// buttons say so.
pub mod intake {
    use super::Action;

    pub fn ask(paste: bool) -> Action {
        Action::error(if paste { "Pasting an image is not in this build yet: drop the image on the panel instead" } else { "The file dialog is not in this build yet: drop the image on the panel instead" })
    }
}
