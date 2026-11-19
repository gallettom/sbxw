//! The shell's side of Jarvis: the daemon calls (off the UI thread), the
//! bus events, the intents, push-to-talk and reading aloud through
//! [`AudioIo`](crate::voice::AudioIo), the docked panel and the proposal
//! dialog. The rules themselves are in `crate::jarvis`.

use super::{headless, App};
use crate::approval::{self, ProposalAnswers};
use crate::client::{self, Daemon};
use crate::feat::relay;
use crate::intent::Intent;
use crate::jarvis::{self, Heard, ProposalNews, Who};
use crate::theme::{c, layout};
use crate::voice::AudioEvent;
use rune::{col, el, Cursor, El, Key, KeyEvent, TextEdit, Tx};
use serde_json::Value as J;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The input of the `jarvis` screen the voice fills when Jarvis cannot take the words.
pub const INPUT: &str = "jarvis-input";
/// The panel's key off macOS (⌘J on macOS).
pub const PANEL_KEY: Key = Key::Function(7);
/// Recordings shorter than this are taken for a slip of the key.
const MIN_CAPTURE_BYTES: usize = 16_000 * 4 * 3 / 10;

/// What a background call to the daemon brought back.
pub enum JEvent {
    Status(Result<J, String>),
    Proposals(Result<Vec<J>, String>),
    Sessions(Vec<J>),
    Voice(Option<J>),
    /// A message left (masked as shown) or did not (the reason).
    Sent { input: Option<String>, result: Result<(String, u64), String> },
    Approved { id: String, via: &'static str, result: Result<J, (Option<u16>, String)> },
    Refused { id: String, result: Result<J, String> },
    Transcribed(Result<String, String>),
    /// The speech of message `msg` for reading `gen` (a newer reading, a stop
    /// or the microphone makes it stale: it is then dropped, never played).
    Speech { msg: u64, gen: u64, result: Result<(u32, crate::voice::Chain), String> },
}

pub struct Wiring {
    pub tx: std::sync::mpsc::Sender<JEvent>,
    pub rx: std::sync::mpsc::Receiver<JEvent>,
    pub pending: Arc<std::sync::atomic::AtomicUsize>,
    /// The next status check, while Jarvis is being created or started.
    pub status_due: Option<Instant>,
    /// The refusal reason's text (the dialog's input).
    pub reason: TextEdit,
    /// Links to open on the next poll (opening one takes a `Tx`).
    pub urls: Vec<String>,
    /// Push-to-talk is held by its key.
    pub key_held: bool,
    /// The person's last message was spoken: Jarvis's answer is read aloud.
    pub spoken: bool,
    /// The keys terminals leave to the shell (push-to-talk, the panel's F7).
    pub reserved: &'static [Key],
    /// The panel's width while its handle is dragged.
    pub drag_from: f32,
    /// The current reading; bumped by every new reading and every stop.
    pub read_gen: u64,
    /// Stops the current reading's chain of speech requests for good.
    pub read_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The intents being run hold `start_jarvis` and `send_to_jarvis`: the
    /// person pressed "Start Jarvis and send".
    pub start_and_send: bool,
}

impl Wiring {
    pub fn new() -> Wiring {
        let (tx, rx) = std::sync::mpsc::channel();
        Wiring {
            tx,
            rx,
            pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            status_due: None,
            reason: TextEdit::new(""),
            urls: vec![],
            key_held: false,
            spoken: false,
            reserved: reserved_for("F8"),
            drag_from: 0.0,
            read_gen: 0,
            read_stop: None,
            start_and_send: false,
        }
    }
}

/// The push-to-talk key (when it is one) and F7, for terminals to leave alone.
pub fn reserved_for(code: &str) -> &'static [Key] {
    let mut keys = vec![PANEL_KEY];
    if let Ok(k) = jarvis::key_of(code) {
        keys.extend(k.iter().cloned());
    }
    Box::leak(keys.into_boxed_slice())
}

/// The panel's width and whether it is open, kept between runs (not in headless runs).
fn prefs_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("SBXW_DESKTOP_PREFS").filter(|p| !p.is_empty()) {
        return p.into();
    }
    crate::builtin::default_dir().with_file_name("window.json")
}

pub fn load_prefs(j: &mut jarvis::Jarvis) {
    // Never the person's file from a test: tests side by side would read
    // what another wrote (an open Jarvis panel narrows every view).
    if headless() || cfg!(test) {
        return;
    }
    if let Ok(v) = std::fs::read_to_string(prefs_path()).map(|t| serde_json::from_str::<J>(&t).unwrap_or_default()) {
        if let Some(w) = v["jarvis_width"].as_f64() {
            j.width = (w as f32).clamp(jarvis::MIN_W, jarvis::MAX_W);
        }
        if let Some(o) = v["jarvis_open"].as_bool() {
            j.open = o;
        }
    }
}

fn save_prefs(j: &jarvis::Jarvis) {
    if headless() || cfg!(test) {
        return;
    }
    let p = prefs_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::json!({ "jarvis_width": j.width, "jarvis_open": j.open }).to_string();
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// What the person hears when a proposal needs a click (jarvis.js, `JV_HIGH`).
const CLICK_NEEDED: &str = "High risk: approve it on screen, with the Approve button. A yes does not approve this one.";

impl App {
    /// Runs `job` with the daemon on a thread; its answer arrives in `poll`.
    pub(super) fn jarvis_job(&mut self, job: impl FnOnce(&dyn Daemon) -> JEvent + Send + 'static) -> bool {
        let Some(d) = self.daemon.clone() else {
            return false;
        };
        let (tx, pending, slot) = (self.wiring.tx.clone(), self.wiring.pending.clone(), self.wake_slot.clone());
        pending.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let ev = job(d.as_ref());
            let _ = tx.send(ev);
            pending.fetch_sub(1, Ordering::SeqCst);
            if let Some(w) = slot.lock().unwrap().as_ref() {
                w();
            }
        });
        true
    }

    /// Connected (again): read where Jarvis stands, its proposals, its last turn, the voice.
    pub(super) fn jarvis_connected(&mut self) {
        self.jarvis.reconnected();
        self.jarvis_job(|d| JEvent::Status(d.jarvis_status().map_err(|e| client::reason(&e))));
        self.jarvis_job(|d| JEvent::Proposals(d.jarvis_proposals().map_err(|e| client::reason(&e))));
        self.jarvis_job(|d| JEvent::Sessions(d.sessions().unwrap_or_default()));
        self.jarvis_job(|d| JEvent::Voice(d.voice_status().ok()));
    }

    /// A bus event: `jarvis` (one proposal), `session` (Jarvis's turns), `voice`.
    pub(super) fn jarvis_bus(&mut self, name: &str, data: &str) -> bool {
        match name {
            "jarvis" => {
                let v: J = serde_json::from_str(data).unwrap_or_default();
                if let Some(list) = v["proposals"].as_array() {
                    for p in list.clone() {
                        self.take_proposal(p, false);
                    }
                } else if v["id"].is_string() {
                    self.take_proposal(v, false);
                } else {
                    self.jarvis_job(|d| JEvent::Proposals(d.jarvis_proposals().map_err(|e| client::reason(&e))));
                }
                true
            }
            "session" => {
                let v: J = serde_json::from_str(data).unwrap_or_default();
                if v["sandbox"] != jarvis::SANDBOX {
                    // The sandbox the panel follows, if any (feat/relay/follow.rs).
                    return self.follow_session(&v);
                }
                if !self.jarvis.running() {
                    self.wiring.status_due = Some(Instant::now());
                }
                // Shown first (in the log), then perhaps read: never the other way.
                if let Some(id) = self.jarvis.ingest_session(&v) {
                    self.auto_read(id);
                }
                true
            }
            "clients" => {
                self.jarvis.clients = data.trim().parse().unwrap_or(1);
                true
            }
            "voice" => {
                let v: J = serde_json::from_str(data).unwrap_or_default();
                self.set_voice(Some(v));
                true
            }
            "provision" if !self.jarvis.running() => {
                self.wiring.status_due = Some(Instant::now());
                false
            }
            _ => false,
        }
    }

    /// A `session` event of the followed sandbox: its agents' turns shown in
    /// the log (read only when Follow is on or a round runs), a round of
    /// questions put on the Requests panel's form. Their text never answers
    /// a proposal: it does not go through `hear`.
    fn follow_session(&mut self, v: &J) -> bool {
        let news = relay::follow::ingest(v);
        let changed = !news.is_empty();
        for n in news {
            match n {
                relay::follow::News::Say { from, text, read } => {
                    let id = self.jarvis.say_from(&from, text);
                    if read {
                        self.follow_read(id);
                    }
                }
                relay::follow::News::Round { sandbox, thread, text } => {
                    let id = relay::add_local_round(&sandbox, thread.as_deref(), &text);
                    self.relay_intent("relay_open", serde_json::json!({ "id": id }));
                }
            }
        }
        changed
    }

    /// The followed sandbox stopped or is gone: the panel says so and its
    /// words go to Jarvis again. Nothing is sent. Only once the list is known.
    fn follow_check(&mut self) -> bool {
        let Some(t) = relay::follow::target() else { return false };
        if self.sandboxes.is_empty() {
            return false;
        }
        let why = match self.sandboxes.iter().find(|s| s.name == t) {
            Some(s) if s.status == "running" => return false,
            Some(s) => format!("{t} is {}", s.status.trim_end_matches('…')),
            None => format!("{t} is gone"),
        };
        self.relay_intent("follow_target", serde_json::json!({}));
        let line = format!("{why}: your words go to Jarvis again. Nothing was sent.");
        log::info!("follow: {line}");
        self.jarvis.say(Who::Note, line.clone());
        self.notify(line, false);
        true
    }

    /// Reads a message of the followed sandbox (or the relay's voice) once,
    /// never while the microphone is open. Shown first, by the caller.
    fn follow_read(&mut self, msg: u64) {
        if self.jarvis.recording || !self.jarvis.auto_read.insert(msg) {
            return;
        }
        self.read(msg);
    }

    /// One of the relay feature's intents, from the core (checked as any).
    fn relay_intent(&mut self, name: &'static str, args: J) {
        let a = crate::feat::Asked { name, args: args.as_object().cloned().unwrap_or_default() };
        self.run_feature_intent("jarvis", Intent::Feature(a));
    }

    /// What the relay's voice queued: shown in the log, then read.
    fn relay_speech(&mut self) -> bool {
        let said = relay::take_speech();
        let changed = !said.is_empty();
        for t in said {
            let id = self.jarvis.say(Who::Note, t);
            self.follow_read(id);
        }
        changed
    }

    pub(super) fn set_voice(&mut self, v: Option<J>) {
        let code = v.as_ref().and_then(|v| v["push_to_talk_key"].as_str()).unwrap_or("F8").to_string();
        self.jarvis.key_name = code.clone();
        self.jarvis.key_note = jarvis::key_of(&code).err();
        if self.jarvis.key_note.is_some() {
            log::warn!("{}", self.jarvis.key_note.as_deref().unwrap_or(""));
        }
        self.wiring.reserved = reserved_for(&code);
        let r = self.wiring.reserved;
        for p in &mut self.panes {
            if let Some(t) = &mut p.term {
                t.keys.reserved = r;
            }
        }
        self.jarvis.voice = v;
    }

    fn take_proposal(&mut self, p: J, seed: bool) {
        match self.jarvis.ingest_proposal(p, seed) {
            ProposalNews::New(id) => {
                let summary = self.jarvis.proposal(&id).and_then(|p| p["summary"].as_str()).unwrap_or("").to_string();
                log::info!("jarvis proposes {id}: {summary}");
                // The dialog shows it; when the person talks, the summary is
                // said too, and what is said is in the log first.
                if self.wiring.spoken {
                    let high = self.jarvis.proposal(&id).is_some_and(jarvis::screen_only);
                    let say = if high { format!("Jarvis proposes: {summary}. {CLICK_NEEDED}") } else { format!("Jarvis proposes: {summary}. Shall I apply it?") };
                    let msg = self.jarvis.say(Who::Note, say);
                    self.auto_read(msg);
                }
            }
            ProposalNews::Settled(id, state) => {
                let p = self.jarvis.proposal(&id).cloned().unwrap_or_default();
                let summary = p["summary"].as_str().unwrap_or("");
                let line = match state.as_str() {
                    "applied" => format!("Applied: {summary}"),
                    "refused" => format!("Refused: {summary}"),
                    _ => format!("Failed: {summary}{}", p["error"].as_str().map(|e| format!(": {e}")).unwrap_or_default()),
                };
                log::info!("jarvis proposal {id}: {line}");
                self.jarvis.say(if state == "failed" { Who::Error } else { Who::Note }, line);
            }
            ProposalNews::None => {}
        }
    }

    /// Drains the calls' answers and the audio's events; checks Jarvis's
    /// status while it is being created or started.
    pub(super) fn poll_jarvis(&mut self, tx: &mut Tx) -> bool {
        let mut changed = self.relay_speech() | self.follow_check();
        // Headless: the virtual clock does not wait for the daemon.
        let until = headless().then(|| Instant::now() + Duration::from_millis(1500));
        loop {
            match self.wiring.rx.try_recv() {
                Ok(ev) => {
                    self.on_jevent(ev);
                    changed = true;
                }
                Err(_) if until.is_some_and(|u| Instant::now() < u) && self.wiring.pending.load(Ordering::SeqCst) > 0 => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        for ev in self.audio.events() {
            changed = true;
            match ev {
                AudioEvent::CaptureStarted => self.jarvis.recording = true,
                AudioEvent::CaptureLevel(l) => self.jarvis.level = l.clamp(0.0, 1.0),
                AudioEvent::CaptureEnded => self.talk_stop(),
                AudioEvent::CaptureFailed(m) => {
                    self.jarvis.recording = false;
                    self.wiring.key_held = false;
                    self.jarvis.say(Who::Error, format!("The microphone: {m}"));
                }
                AudioEvent::PlaybackStarted | AudioEvent::PlaybackLevel(_) => {}
                AudioEvent::PlaybackEnded => self.jarvis.reading = None,
                AudioEvent::PlaybackFailed(m) => {
                    self.jarvis.reading = None;
                    self.jarvis.say(Who::Error, format!("Could not read aloud: {m}"));
                }
            }
        }
        let now = Instant::now();
        if self.wiring.status_due.is_some_and(|d| d <= now) {
            self.wiring.status_due = None;
            self.jarvis_job(|d| JEvent::Status(d.jarvis_status().map_err(|e| client::reason(&e))));
        } else if let Some(d) = self.wiring.status_due {
            tx.wake_after(d.saturating_duration_since(now));
        }
        for url in std::mem::take(&mut self.wiring.urls) {
            if let Err(e) = tx.open_url(&url) {
                self.notify(format!("Could not open {url}: {e:?}"), true);
            }
        }
        changed
    }

    pub(super) fn on_jevent(&mut self, ev: JEvent) {
        match ev {
            JEvent::Status(Ok(v)) => {
                let state = v["state"].as_str().unwrap_or("unknown").to_string();
                if state != self.jarvis.status {
                    log::info!("jarvis: {state}");
                }
                self.jarvis.status_error = v["error"].as_str().map(str::to_string);
                if matches!(state.as_str(), "creating" | "starting") {
                    self.wiring.status_due = Some(Instant::now() + Duration::from_millis(1500));
                }
                let was_running = self.jarvis.running();
                self.jarvis.status = state.clone();
                if state == "running" && !was_running {
                    self.jarvis_job(|d| JEvent::Sessions(d.sessions().unwrap_or_default()));
                    // Running: a "not running" error is over; a send the
                    // person asked for with "Start Jarvis and send" leaves now.
                    if let Some(e) = self.jarvis.send_error.take() {
                        if e.queued {
                            self.send_kept();
                        }
                    }
                } else if state != "running" {
                    self.jarvis.not_awaiting();
                    if state == "failed" || state == "stopped" || state == "absent" {
                        if let Some(e) = self.jarvis.send_error.as_mut().filter(|e| e.queued) {
                            e.queued = false;
                            e.offer = "start";
                            e.text = format!("Not sent: Jarvis could not start{}.", self.jarvis.status_error.as_deref().map(|x| format!(" ({x})")).unwrap_or_default());
                        }
                    }
                }
            }
            JEvent::Status(Err(e)) => {
                self.jarvis.status = "unknown".into();
                self.jarvis.status_error = Some(e);
                self.jarvis.not_awaiting();
            }
            JEvent::Proposals(Ok(list)) => {
                let first = self.jarvis.proposals.is_empty();
                for p in list.into_iter().rev() {
                    self.take_proposal(p, first);
                }
            }
            JEvent::Proposals(Err(e)) => log::warn!("jarvis proposals: {e}"),
            JEvent::Sessions(list) => self.jarvis.seed_sessions(&list),
            JEvent::Voice(v) => self.set_voice(v),
            JEvent::Sent { input, result } => {
                self.jarvis.sending = false;
                match result {
                    Ok((text, masked)) => {
                        if let Some(i) = input {
                            self.screen_mut("jarvis").map(|s| s.ui.clear(&i));
                        }
                        self.jarvis.send_error = None;
                        if masked > 0 {
                            self.jarvis.say(Who::Note, format!("Sent with {masked} name{} masked: {text}", if masked == 1 { "" } else { "s" }));
                        }
                    }
                    Err(e) => {
                        self.jarvis.not_awaiting();
                        self.send_failed(format!("Not sent: {e}"), "none", false);
                    }
                }
            }
            JEvent::Approved { id, via, result } => {
                self.jarvis.busy = false;
                match result {
                    Ok(p) => {
                        self.jarvis.dialog_error = None;
                        if p["id"].is_string() {
                            self.take_proposal(p, false);
                        }
                    }
                    Err((Some(409), _)) if via == "voice" => self.click_needed(&id),
                    Err((_, msg)) => {
                        // A refusal is an answer, not a silence: beside the buttons, and in the log.
                        self.jarvis.dialog_error = Some((id.clone(), format!("Not applied: {msg}")));
                        self.jarvis.say(Who::Error, format!("Proposal not applied: {msg}"));
                        self.jarvis.shown = Some(id);
                        self.jarvis_job(|d| JEvent::Proposals(d.jarvis_proposals().map_err(|e| client::reason(&e))));
                    }
                }
            }
            JEvent::Refused { id, result } => {
                self.jarvis.busy = false;
                match result {
                    Ok(p) if p["id"].is_string() => self.take_proposal(p, false),
                    Ok(_) => {}
                    Err(e) => {
                        self.jarvis.dialog_error = Some((id, format!("Not refused: {e}")));
                        self.jarvis.say(Who::Error, format!("Proposal not refused: {e}"));
                    }
                }
            }
            JEvent::Transcribed(Ok(text)) => {
                if text.trim().is_empty() {
                    self.jarvis.say(Who::Note, "Nothing heard.");
                } else {
                    self.hear(&text, true, None);
                }
            }
            JEvent::Transcribed(Err(e)) => {
                self.jarvis.say(Who::Error, format!("Could not transcribe: {e}"));
            }
            // A reading stopped, replaced, or overtaken by the microphone
            // while its speech was being made is dropped here: never played.
            JEvent::Speech { gen, .. } if gen != self.wiring.read_gen || self.jarvis.recording => {
                log::info!("jarvis: a stale reading dropped");
            }
            JEvent::Speech { msg, result, .. } => match result {
                Ok((rate, chain)) => match self.audio.play(rate, Box::new(chain)) {
                    Ok(()) => self.jarvis.reading = Some(msg),
                    Err(e) => {
                        self.jarvis.reading = None;
                        self.jarvis.say(Who::Error, format!("Could not read aloud: {e}"));
                    }
                },
                Err(e) => {
                    self.jarvis.reading = None;
                    self.jarvis.say(Who::Error, format!("Could not read aloud: {e}"));
                }
            },
        }
    }

    pub(super) fn screen_mut(&mut self, name: &str) -> Option<&mut crate::screen::ScreenState> {
        self.screens.iter_mut().find(|s| s.name == name)
    }

    /// Words for Jarvis: typed (`input`: the input they came from, emptied
    /// once they left; `person`: the person typed and sent them) or spoken.
    /// A yes or no answers the armed proposal; anything else is a message.
    fn hear(&mut self, text: &str, spoken: bool, input: Option<(String, bool)>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.wiring.spoken = spoken;
        // A round answered by voice takes what is said (feat/relay).
        if spoken && relay::voice_armed() {
            self.jarvis.say(Who::Me, &text);
            self.relay_intent("relay_heard", serde_json::json!({ "text": text }));
            self.relay_speech();
            return;
        }
        // Another sandbox picked at the top: the words are a draft for its
        // agent, confirmed in the core's dialog. Never a yes to Jarvis.
        if relay::follow::target().is_some() {
            self.jarvis.say(Who::Me, &text);
            self.clear_input(&input.map(|i| i.0));
            let name = if relay::follow::choosing().is_some() { "follow_choose" } else { "follow_send" };
            self.relay_intent(name, serde_json::json!({ "text": text }));
            return;
        }
        // Only words the person said or typed and sent may answer a proposal.
        let person = spoken || input.as_ref().is_some_and(|i| i.1);
        let heard = if person { self.jarvis.hear(&text) } else { Heard::Send(text.clone()) };
        let input_id = input.map(|i| i.0);
        match heard {
            Heard::Approve(id) => {
                self.jarvis.say(Who::Me, &text);
                self.clear_input(&input_id);
                self.answer(&id, "voice");
            }
            Heard::Refuse(id) => {
                self.jarvis.say(Who::Me, &text);
                self.clear_input(&input_id);
                self.refuse(&id, None);
            }
            Heard::ClickNeeded(id) => {
                self.jarvis.say(Who::Me, &text);
                self.clear_input(&input_id);
                self.click_needed(&id);
            }
            Heard::Send(t) => self.send(t, input_id),
        }
    }

    fn clear_input(&mut self, input: &Option<String>) {
        if let Some(i) = input {
            if let Some(s) = self.screen_mut("jarvis") {
                s.ui.clear(i);
            }
        }
    }

    /// A yes that cannot approve: shown, said, the dialog brought back.
    fn click_needed(&mut self, id: &str) {
        self.jarvis.shown = Some(id.to_string());
        self.jarvis.armed = Some(id.to_string());
        let msg = self.jarvis.say(Who::Note, CLICK_NEEDED);
        self.auto_read(msg);
    }

    /// The input's text now.
    fn input_text(&mut self) -> String {
        self.screen_mut("jarvis").and_then(|s| s.ui.edits.get(INPUT).map(|e| e.0.text.clone())).unwrap_or_default()
    }

    /// A message could not leave: why, beside the input (not in the
    /// conversation), with what the panel offers; the text stays in the input.
    fn send_failed(&mut self, text: String, offer: &'static str, queued: bool) {
        let input = self.input_text();
        self.jarvis.send_error = Some(jarvis::SendError { text, offer, queued, input });
    }

    /// Jarvis runs and the person had pressed "Start Jarvis and send": the
    /// input's text leaves now (emptied by the person: nothing leaves).
    fn send_kept(&mut self) {
        let text = self.input_text();
        if text.trim().is_empty() {
            return;
        }
        log::info!("jarvis: running, sending the kept message as asked");
        // Never an answer to a proposal: only a message.
        self.send(text.trim().to_string(), Some(INPUT.to_string()));
    }

    /// A message to Jarvis: masked, then typed into its session.
    fn send(&mut self, text: String, input: Option<String>) {
        if !self.jarvis.running() {
            // Spoken words are not lost: they wait in the input.
            if input.is_none() {
                if let Some(s) = self.screen_mut("jarvis") {
                    if let Some(e) = s.ui.edits.get_mut(INPUT) {
                        e.0.replace_text(text);
                    }
                }
            }
            let starting = matches!(self.jarvis.status.as_str(), "creating" | "starting");
            let queued = self.wiring.start_and_send;
            let (why, offer) = match (starting || queued, queued) {
                (_, true) => ("Starting Jarvis: your message leaves once it runs. Empty the input to cancel.", "none"),
                (true, false) => ("Not sent: Jarvis is still starting.", "wait"),
                _ => ("Not sent: Jarvis is not running.", "start"),
            };
            self.send_failed(why.into(), offer, queued);
            return;
        }
        if self.jarvis.sending {
            self.send_failed("Not sent: the last message is still on its way.".into(), "none", false);
            return;
        }
        self.jarvis.say(Who::Me, &text);
        self.jarvis.sending = true;
        self.jarvis.sent();
        let started = self.jarvis_job(move |d| {
            let r = d.anonymise(&text).and_then(|(safe, n)| d.chat_push(jarvis::SANDBOX, &safe).map(|()| (safe, n)));
            JEvent::Sent { input, result: r.map_err(|e| client::reason(&e)) }
        });
        if !started {
            self.jarvis.sending = false;
            self.jarvis.not_awaiting();
            let why = self.connection.text.clone();
            self.send_failed(format!("Not sent: {why}"), "none", false);
        }
    }

    pub(super) fn answer(&mut self, id: &str, via: &'static str) {
        let Some(p) = self.jarvis.proposal(id).cloned() else { return };
        if !jarvis::pending(&p) {
            self.jarvis.say(Who::Error, "Jarvis's proposal is no longer waiting for an answer.");
            return;
        }
        if self.jarvis.busy {
            self.jarvis.say(Who::Error, "Wait: an answer to a Jarvis proposal is already on its way.");
            return;
        }
        // Never weakened here: a voice yes only for a low-risk one, the
        // acknowledgement only from the ticked box. The daemon checks both again.
        if via == "voice" && jarvis::screen_only(&p) {
            return self.click_needed(id);
        }
        let ack = via == "click" && jarvis::needs_ack(&p) && self.jarvis.acked.contains(id);
        if via == "click" && jarvis::needs_ack(&p) && !ack {
            self.jarvis.dialog_error = Some((id.to_string(), "Tick \"I have read this command\" first.".into()));
            return;
        }
        self.jarvis.busy = true;
        self.jarvis.dialog_error = None;
        let id = id.to_string();
        log::info!("jarvis: approve {id} via {via}{}", if ack { " (host command acknowledged)" } else { "" });
        let ok = self.jarvis_job(move |d| {
            let result = d.jarvis_approve(&id, via, ack).map_err(|e| (client::status_of(&e), client::reason(&e)));
            JEvent::Approved { id, via, result }
        });
        if !ok {
            self.jarvis.busy = false;
        }
    }

    fn refuse(&mut self, id: &str, reason: Option<String>) {
        if self.jarvis.busy || !self.jarvis.proposal(id).is_some_and(jarvis::pending) {
            return;
        }
        self.jarvis.busy = true;
        let id = id.to_string();
        log::info!("jarvis: refuse {id}");
        let ok = self.jarvis_job(move |d| {
            let result = d.jarvis_refuse(&id, reason.as_deref()).map_err(|e| client::reason(&e));
            JEvent::Refused { id, result }
        });
        if !ok {
            self.jarvis.busy = false;
        }
    }

    pub(super) fn talk_start(&mut self) {
        if self.jarvis.recording {
            return;
        }
        if let Some(n) = &self.jarvis.key_note {
            log::info!("{n}");
        }
        // The microphone must not hear Jarvis: the reading ends for good,
        // including speech still being made for it.
        self.stop_reading();
        match self.audio.start_capture() {
            Ok(()) => {
                self.jarvis.recording = true;
                self.jarvis.level = 0.0;
            }
            Err(e) => {
                self.jarvis.say(Who::Error, format!("Cannot record: {e}"));
            }
        }
    }

    pub(super) fn talk_stop(&mut self) {
        if !self.jarvis.recording {
            return;
        }
        self.jarvis.recording = false;
        self.wiring.key_held = false;
        match self.audio.stop_capture() {
            Ok(pcm) if pcm.len() < MIN_CAPTURE_BYTES => {
                self.jarvis.say(Who::Note, format!("Too short: hold {} while you speak.", self.jarvis.key_name));
            }
            Ok(pcm) => {
                log::info!("jarvis: {} s recorded", pcm.len() / (16_000 * 4));
                if !self.jarvis_job(move |d| JEvent::Transcribed(d.transcribe(pcm).map_err(|e| client::reason(&e)))) {
                    self.jarvis.say(Who::Error, "Not transcribed: not connected to the daemon.");
                }
            }
            Err(e) => {
                self.jarvis.say(Who::Error, format!("Recording failed: {e}"));
            }
        }
    }

    /// A message of the log read by itself (an answer to words spoken):
    /// once per message, never while the microphone is open.
    fn auto_read(&mut self, msg: u64) {
        if !self.wiring.spoken || self.jarvis.recording || !self.jarvis.auto_read.insert(msg) {
            return;
        }
        self.read(msg);
    }

    /// Reads message `msg` of the log aloud: only what is shown is said.
    fn read(&mut self, msg: u64) {
        let Some(text) = self.jarvis.log.iter().find(|m| m.id == msg).map(|m| m.text.clone()) else { return };
        if self.jarvis.recording || !self.audio.available() || !self.jarvis.voice.as_ref().is_some_and(|v| v["speech"]["supported"] == true && v["ready"] == true) {
            return;
        }
        let pieces = jarvis::speech_pieces(&text, 1000);
        if pieces.is_empty() {
            return;
        }
        self.stop_reading();
        let Some(d) = self.daemon.clone() else { return };
        let gen = self.wiring.read_gen;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.wiring.read_stop = Some(stop.clone());
        self.jarvis.reading = Some(msg);
        self.jarvis_job(move |_| JEvent::Speech { msg, gen, result: crate::voice::Chain::start(d, pieces, stop).map_err(|e| client::reason(&e)) });
    }

    /// Ends the reading for good: what plays stops, speech still being made
    /// for it is dropped when it arrives, and its chain fetches nothing more.
    pub(super) fn stop_reading(&mut self) {
        self.wiring.read_gen += 1;
        if let Some(s) = self.wiring.read_stop.take() {
            s.store(true, Ordering::Release);
        }
        self.audio.stop_playback();
        self.jarvis.reading = None;
    }

    /// The Jarvis intents.
    pub(super) fn run_jarvis_intent(&mut self, from: &str, intent: Intent) {
        match intent {
            Intent::StartJarvis => {
                self.jarvis.status_error = None;
                if !self.jarvis_job(|d| JEvent::Status(d.jarvis_start().map_err(|e| client::reason(&e)))) {
                    let why = self.connection.text.clone();
                    self.jarvis.say(Who::Error, format!("Cannot start Jarvis: {why}"));
                }
            }
            Intent::SendToJarvis { input } => {
                let Some(s) = self.screen_mut(from) else { return };
                let editing = s.ui.editing;
                let text = s.ui.edits.get(&input).map(|e| e.0.text.clone()).unwrap_or_default();
                // The person's own words, sent by Enter or a press: not set by the screen.
                let person = !editing && s.ui.typed(&input).is_some();
                self.hear(&text, false, Some((input, person)));
            }
            Intent::ToggleJarvisPanel { open } => self.toggle_panel(open),
            Intent::TalkStart => self.talk_start(),
            Intent::TalkStop => self.talk_stop(),
            Intent::ReadAloud { id } => {
                if self.jarvis.reading == Some(id) {
                    self.stop_reading();
                } else {
                    self.read(id);
                }
            }
            Intent::StopReading => self.stop_reading(),
            Intent::RefuseProposal { id, reason } => self.refuse(&id, reason),
            Intent::OpenLink { url } => {
                if crate::markdown::safe_url(&url) {
                    log::info!("open link {url}");
                    self.wiring.urls.push(url);
                }
            }
            // `approve_proposal` never reaches here: it is held for the dialog (`App::intents`).
            other => log::warn!("{other:?} is not a Jarvis intent"),
        }
    }

    /// A screen asked to see a proposal (`approve_proposal`, risk approval):
    /// the core shows its dialog. Approving takes the person's click there.
    pub(super) fn show_proposal(&mut self, id: &str) {
        if self.jarvis.proposal(id).is_some() {
            self.jarvis.shown = Some(id.to_string());
        } else {
            self.notify(format!("No Jarvis proposal {id}"), true);
        }
    }

    pub(super) fn toggle_panel(&mut self, open: Option<bool>) {
        self.jarvis.open = open.unwrap_or(!self.jarvis.open);
        self.render_shown();
        save_prefs(&self.jarvis);
        if self.jarvis.open && self.jarvis.status == "unknown" {
            self.jarvis_job(|d| JEvent::Status(d.jarvis_status().map_err(|e| client::reason(&e))));
        }
    }

    /// The core's keys, before a terminal: push-to-talk held, the panel's key.
    pub(super) fn jarvis_key(&mut self, ev: &KeyEvent, _tx: &mut Tx) -> bool {
        let m = ev.mods;
        let bare = !(m.cmd || m.ctrl || m.alt || m.logo);
        let ptt = jarvis::key_of(&self.jarvis.key_name).ok().and_then(|k| k.first().cloned());
        if bare && Some(&ev.key) == ptt.as_ref() {
            // A held key repeats: only the first press starts.
            if !self.wiring.key_held {
                self.wiring.key_held = true;
                self.jarvis.open = true;
                self.render_shown();
                self.talk_start();
            }
            return true;
        }
        let mac_toggle = cfg!(target_os = "macos") && ev.key == Key::Command('j') && m.cmd && !m.shift && !m.alt;
        if (bare && ev.key == PANEL_KEY) || mac_toggle {
            self.toggle_panel(None);
            return true;
        }
        false
    }

    /// A key came up: push-to-talk's release sends what was said.
    pub(super) fn jarvis_key_up(&mut self, ev: &KeyEvent) -> bool {
        let ptt = jarvis::key_of(&self.jarvis.key_name).ok().and_then(|k| k.first().cloned());
        if Some(&ev.key) == ptt.as_ref() && self.wiring.key_held {
            self.talk_stop();
            return true;
        }
        false
    }

    /// The dialog for the shown proposal, when one is shown.
    pub(super) fn dialog_view(&self) -> Option<El> {
        let id = self.jarvis.shown.as_deref()?;
        let p = self.jarvis.proposal(id)?;
        let waiting = self.jarvis.pending_list().iter().filter(|x| x["id"] != id).count();
        let panel = if self.jarvis.open { self.jarvis.width + layout::GUTTER * 2.0 } else { 0.0 };
        let top = layout::GUTTER * 2.0 + layout::TOP_BAR_H + 8.0;
        let d = approval::DialogState {
            proposal: p,
            waiting,
            acked: self.jarvis.acked.contains(id),
            error: self.jarvis.dialog_error.as_ref().filter(|e| e.0 == id).map(|e| e.1.as_str()),
            busy: self.jarvis.busy,
            reason: &self.wiring.reason,
            left: layout::GUTTER * 2.0 + if self.sidebar_shown() { self.sidebar_w() } else { 0.0 } + 16.0,
            top,
            right: panel + layout::GUTTER + 16.0,
            max_h: self.window.1 - top - 16.0,
        };
        Some(approval::proposal_view::<App>(&d))
    }

    /// The docked panel: a 6 pt handle that drags its width, and the tile
    /// with the `jarvis` screen.
    pub(super) fn panel_view(&self, screen: El) -> El {
        let handle = el()
            .key("jarvis-handle")
            .w(layout::GUTTER)
            .h_full()
            .cursor(Cursor::ResizeColumn)
            .hover(c("border"))
            .on_drag(|s: &mut App, d: &mut rune::Drag, _tx| {
                if matches!(d.phase, rune::Phase::Down) {
                    s.wiring.drag_from = s.jarvis.width;
                }
                let most = (s.window.0 - s.sidebar_w() - 320.0).clamp(jarvis::MIN_W, jarvis::MAX_W);
                s.jarvis.width = (s.wiring.drag_from - d.dx).clamp(jarvis::MIN_W, most);
                if matches!(d.phase, rune::Phase::Up) {
                    save_prefs(&s.jarvis);
                }
            });
        let tile = col()
            .key("jarvis-panel")
            .role("complementary")
            .label("Jarvis")
            .w(self.jarvis.width.min((self.window.0 - self.sidebar_w() - 320.0).max(jarvis::MIN_W)))
            .h_full()
            .min_h(0.0)
            .padding(12.0)
            .fill(c("bg"))
            .border(1.0, c("elevated"))
            .radius(crate::theme::radius("md").unwrap())
            .child(screen);
        rune::row().h_full().min_h(0.0).child(handle).child(tile)
    }
}

impl ProposalAnswers for App {
    fn approve_click(&mut self) {
        if let Some(id) = self.jarvis.shown.clone() {
            self.answer(&id, "click");
        }
    }
    fn refuse_click(&mut self) {
        if let Some(id) = self.jarvis.shown.clone() {
            let reason = Some(self.wiring.reason.text.trim().to_string()).filter(|r| !r.is_empty());
            self.wiring.reason.replace_text(String::new());
            self.refuse(&id, reason);
        }
    }
    fn toggle_ack(&mut self) {
        if let Some(id) = self.jarvis.shown.clone() {
            if !self.jarvis.acked.remove(&id) {
                self.jarvis.acked.insert(id.clone());
            }
            if self.jarvis.dialog_error.as_ref().is_some_and(|e| e.0 == id) {
                self.jarvis.dialog_error = None;
            }
        }
    }
    fn close_dialog(&mut self) {
        self.jarvis.close_dialog();
    }
    fn next_dialog(&mut self) {
        let next = self.jarvis.pending_list().first().map(|p| p["id"].as_str().unwrap_or("").to_string());
        self.jarvis.shown = next;
    }
    fn reason_edit(&mut self) -> &mut TextEdit {
        &mut self.wiring.reason
    }
}

/// The first real run on a Mac (JARVIS.md, "First real run: two fixes"),
/// replayed with the daemon's own event shapes.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{FixtureDaemon, FixtureData};
    use crate::screen::Host;
    use crate::shell::Args;
    use crate::voice::{AudioIo, FakeAudio};
    use serde_json::json;
    use std::sync::Mutex;

    /// FakeAudio the test can look into while the app owns it.
    struct Shared(Arc<Mutex<FakeAudio>>);
    impl AudioIo for Shared {
        fn available(&self) -> bool {
            true
        }
        fn start_capture(&mut self) -> Result<(), String> {
            self.0.lock().unwrap().start_capture()
        }
        fn stop_capture(&mut self) -> Result<Vec<u8>, String> {
            self.0.lock().unwrap().stop_capture()
        }
        fn cancel_capture(&mut self) {
            self.0.lock().unwrap().cancel_capture()
        }
        fn play(&mut self, rate: u32, pcm: Box<dyn std::io::Read + Send>) -> Result<(), String> {
            self.0.lock().unwrap().play(rate, pcm)
        }
        fn stop_playback(&mut self) {
            self.0.lock().unwrap().stop_playback()
        }
        fn events(&mut self) -> Vec<AudioEvent> {
            self.0.lock().unwrap().events()
        }
    }

    struct Run {
        a: App,
        d: Arc<FixtureDaemon>,
        audio: Arc<Mutex<FakeAudio>>,
    }

    impl Run {
        /// The panel open on a daemon where Jarvis is `status`, with a voice that speaks.
        fn new(status: &str) -> Run {
            let mut a = App::new(Args { fixture: Some(format!("{}/fixtures/jarvis/absent.json", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
            let voice = json!({ "ok": true, "ready": true, "speech": { "supported": true }, "push_to_talk_key": "F8" });
            let data: FixtureData = serde_json::from_value(json!({ "jarvis_status": { "state": status }, "voice": voice })).unwrap();
            let d = Arc::new(FixtureDaemon::new("first-run", data));
            a.daemon = Some(d.clone());
            let audio = Arc::new(Mutex::new(FakeAudio { capture: vec![0u8; 16_000 * 4], ..Default::default() }));
            a.audio = Box::new(Shared(audio.clone()));
            a.jarvis.voice = Some(voice);
            a.jarvis.open = true;
            let mut r = Run { a, d, audio };
            r.a.on_jevent(JEvent::Status(Ok(json!({ "state": status }))));
            r.settle();
            r
        }
        /// Every daemon call answered, every audio event drained.
        fn settle(&mut self) {
            let until = Instant::now() + Duration::from_secs(3);
            loop {
                while self.a.wiring.pending.load(Ordering::SeqCst) > 0 && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let mut any = false;
                while let Ok(ev) = self.a.wiring.rx.try_recv() {
                    self.a.on_jevent(ev);
                    any = true;
                }
                any |= self.a.poll_jarvis(&mut Tx::default());
                if !any && self.a.wiring.pending.load(Ordering::SeqCst) == 0 {
                    break;
                }
            }
        }
        fn input(&mut self) -> String {
            self.a.screen_mut("jarvis").unwrap().ui.edits.get(INPUT).map(|e| e.0.text.clone()).unwrap_or_default()
        }
        /// The person types `text` and presses Enter.
        fn enter(&mut self, text: &str) {
            self.a.screen_mut("jarvis").unwrap().ui.edits.insert(INPUT.into(), (TextEdit::new(text), None));
            self.a.intents("jarvis", vec![Intent::SendToJarvis { input: INPUT.into() }]);
            self.settle();
        }
        fn pushed(&self) -> Vec<String> {
            self.d.pushed().into_iter().map(|p| p.1).collect()
        }
        /// One hook of the daemon: a `session` event for EACH session of the
        /// sandbox, in session id order (agents.rs `apply` then `infos_for`).
        fn hook(&mut self, sessions: &[J]) {
            for s in sessions {
                self.a.jarvis_bus("session", &s.to_string());
            }
            self.settle();
        }
        fn shown(&self) -> Vec<(Who, String)> {
            self.a.jarvis.log.iter().map(|m| (m.who, m.text.clone())).collect()
        }
        fn played(&self) -> usize {
            self.audio.lock().unwrap().played.len()
        }
    }

    fn sess(id: &str, started: u64, state: &str, extra: J) -> J {
        let mut v = json!({ "sandbox": jarvis::SANDBOX, "mode": "claude", "state": state, "agent": "claude", "started_ms": started,
            "session_id": id, "origin": "tty", "answerable": true, "ts": 0 });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    }

    const OPTIONS: &str = "Which voice do you want?\n\n1. fr_female\n2. en_female\n3. de_male";

    /// The sequence of the first real run: panel open while Jarvis is not
    /// running, a message refused, Jarvis started, Enter again, a first
    /// answer offering numbered options (a question, `AskUserQuestion`),
    /// "2", the second answer. Jarvis's sandbox holds an older session left
    /// behind as `working` (its hooks stopped), so every hook of the new
    /// session comes with that one's event too.
    #[test]
    fn first_answer_with_options_is_shown_and_working_clears() {
        let mut r = Run::new("absent");
        r.enter("changez la voix");
        assert!(r.pushed().is_empty());
        r.a.intents("jarvis", vec![Intent::StartJarvis]);
        r.settle();
        r.a.on_jevent(JEvent::Status(Ok(json!({ "state": "running" }))));
        r.settle();
        let old = |state: &str| sess("z-old", 1, state, json!({}));
        r.hook(&[sess("a-new", 2, "idle", json!({})), old("working")]);
        r.enter("changez la voix");
        assert_eq!(r.pushed(), vec!["changez la voix".to_string()]);
        r.hook(&[sess("a-new", 2, "working", json!({ "last_input": "changez la voix" })), old("working")]);
        let ask = json!({ "steps": [{ "text": "Which voice do you want?", "options": ["fr_female", "en_female", "de_male"] }], "activity": "Which voice do you want?" });
        r.hook(&[sess("a-new", 2, "attention", ask.clone()), old("working")]);
        let jarvis_lines = |r: &Run| r.shown().into_iter().filter(|m| m.0 == Who::Jarvis).map(|m| m.1).collect::<Vec<_>>();
        assert_eq!(jarvis_lines(&r).len(), 1, "the first answer (a question with options) is shown: {:?}", r.shown());
        assert!(jarvis_lines(&r)[0].starts_with(OPTIONS), "{:?}", jarvis_lines(&r));
        assert!(!r.a.jarvis.working, "a question waits for the person: no \"…\"");
        // The same state again (another hook): not shown twice.
        r.hook(&[sess("a-new", 2, "attention", ask), old("working")]);
        assert_eq!(jarvis_lines(&r).len(), 1);
        r.enter("2");
        r.hook(&[sess("a-new", 2, "working", json!({})), old("working")]);
        assert!(r.a.jarvis.working);
        let second = "Done: I propose the English voice.";
        r.hook(&[sess("a-new", 2, "idle", json!({ "reply": second })), old("working")]);
        r.hook(&[sess("a-new", 2, "idle", json!({ "reply": second })), old("working")]);
        assert_eq!(jarvis_lines(&r).len(), 2, "{:?}", r.shown());
        assert_eq!(jarvis_lines(&r)[1], second);
        assert!(!r.a.jarvis.working, "the \"…\" ends with the turn, whatever the older session says");
        assert_eq!(r.a.jarvis.screen_data(true)["working"], false);
    }

    /// A first turn whose working state was never seen (the session is new
    /// to the panel): its reply is still the answer to the message sent.
    #[test]
    fn a_reply_from_a_session_never_seen_is_the_answer_once() {
        let mut r = Run::new("running");
        r.enter("hello");
        r.hook(&[sess("s1", 5, "idle", json!({ "reply": "Hello, I am Jarvis.\n\n1. Voice\n2. Kits" }))]);
        r.hook(&[sess("s1", 5, "idle", json!({ "reply": "Hello, I am Jarvis.\n\n1. Voice\n2. Kits" }))]);
        let answers: Vec<_> = r.shown().into_iter().filter(|m| m.0 == Who::Jarvis).collect();
        assert_eq!(answers.len(), 1, "{:?}", r.shown());
        assert!(!r.a.jarvis.working);
        // Reconnected: the "…" is not carried over.
        r.a.jarvis.working = true;
        r.a.jarvis_connected();
        assert!(!r.a.jarvis.working);
    }

    /// Spoken words: whatever is read aloud is in the log first, each answer
    /// read once; the microphone stops the reading for good, including the
    /// speech still being made, and nothing is read while it is open.
    fn follow(target: Option<&str>, on: bool) {
        let mut f = relay::follow::FOLLOW().lock().unwrap();
        f.target = target.map(str::to_string);
        f.on = on;
        f.loaded = true;
    }

    fn other(state: &str, extra: J) -> J {
        let mut v = sess("w1", 5, state, extra);
        v["sandbox"] = "web-app".into();
        v
    }

    /// Another sandbox picked: a typed yes is a draft for its agent, in the
    /// core's dialog; it never approves Jarvis's low-risk proposal, and
    /// nothing is pushed until the person confirms.
    #[test]
    fn words_for_a_followed_sandbox_are_a_draft_and_never_a_yes_to_jarvis() {
        let mut r = Run::new("running");
        r.a.take_proposal(json!({ "id": "p-1", "summary": "Use the English voice", "risk": "low", "state": "pending", "created": 1, "changes": [] }), false);
        assert_eq!(r.a.jarvis.armed.as_deref(), Some("p-1"));
        follow(Some("web-app"), false);
        r.enter("yes");
        assert!(r.a.jarvis.proposal("p-1").is_some_and(jarvis::pending), "the proposal still waits");
        assert!(r.pushed().is_empty(), "nothing typed into any session");
        let d = r.a.feats.dialog.as_ref().expect("the core's dialog");
        assert_eq!(d.feature, "relay");
        let p = d.prepare.as_ref().expect("masked first");
        assert_eq!((p.asked.name, p.asked.req("text"), p.asked.req("sandbox")), ("follow_send", "yes", "web-app"));
        assert_eq!(r.input(), "", "the words left the input for the dialog");
        // Back to Jarvis: a yes answers its proposal again.
        r.a.feats.dialog = None;
        follow(None, false);
        assert_eq!(r.a.jarvis.hear("yes"), Heard::Approve("p-1".into()), "still armed: a yes for Jarvis answers it");
    }

    /// The followed sandbox stops: the panel says so, the words go to Jarvis
    /// again, nothing is sent.
    #[test]
    fn a_followed_sandbox_that_stops_falls_back_to_jarvis() {
        let mut r = Run::new("running");
        r.a.sandboxes = serde_json::from_value(json!([{ "name": "web-app", "status": "running" }, { "name": "docs", "status": "running" }])).unwrap();
        follow(Some("web-app"), true);
        r.settle();
        assert_eq!(relay::follow::target().as_deref(), Some("web-app"), "running: kept");
        r.a.sandboxes[0].status = "stopped".into();
        r.settle();
        assert_eq!(relay::follow::target(), None);
        assert!(r.shown().iter().any(|(w, t)| *w == Who::Note && t == "web-app is stopped: your words go to Jarvis again. Nothing was sent."), "{:?}", r.shown());
        assert!(r.pushed().is_empty());
        follow(Some("docs"), false);
        r.a.sandboxes.retain(|s| s.name != "docs");
        r.settle();
        assert_eq!(relay::follow::target(), None, "gone: back to Jarvis");
        assert!(r.shown().iter().any(|(_, t)| t.starts_with("docs is gone")));
    }

    /// A followed agent's turn: shown once, read only with Follow on, never
    /// while the microphone is open; a round goes to the Requests panel.
    #[test]
    fn a_followed_agent_is_shown_once_and_read_only_when_followed() {
        let mut r = Run::new("running");
        follow(Some("web-app"), false);
        r.hook(&[other("working", json!({}))]);
        r.hook(&[other("idle", json!({ "reply": "Tests pass." }))]);
        assert!(r.shown().iter().all(|(w, _)| *w != Who::Agent), "Follow off: an ordinary turn is not shown");
        follow(Some("web-app"), true);
        r.hook(&[other("working", json!({}))]);
        r.hook(&[other("idle", json!({ "reply": "Migration written." }))]);
        r.hook(&[other("idle", json!({ "reply": "Migration written." }))]);
        let agent: Vec<_> = r.a.jarvis.log.iter().filter(|m| m.who == Who::Agent).collect();
        assert_eq!(agent.len(), 1, "shown once");
        assert_eq!(agent[0].from.as_deref(), Some("web-app"));
        assert_eq!(r.played(), 1, "read once");
        r.a.intents("jarvis", vec![Intent::TalkStart]);
        r.hook(&[other("working", json!({}))]);
        r.hook(&[other("idle", json!({ "reply": "Pushed the branch." }))]);
        assert_eq!(r.a.jarvis.log.last().unwrap().text, "Pushed the branch.");
        assert_eq!(r.played(), 1, "nothing read while the microphone is open");
        r.a.intents("jarvis", vec![Intent::TalkStop]);
        r.settle();
        // A round with Follow off: put on the form, said in one line.
        follow(Some("web-app"), false);
        r.hook(&[other("working", json!({ "thread": "t1" }))]);
        r.hook(&[other("idle", json!({ "thread": "t1", "reply": "❓ **Q1 - Port** : which port?\n➡️ 8080" }))]);
        assert!(r.shown().iter().any(|(w, t)| *w == Who::Agent && t.contains("one question")), "{:?}", r.shown());
        assert!(relay::requests(&crate::feat::Ctx { data: &json!({}) }).iter().any(|q| q["local"] == true && q["from_thread"] == "t1"));
        follow(None, false);
    }

    #[test]
    fn talk_stops_the_reading_for_good_and_nothing_is_read_twice() {
        let mut r = Run::new("running");
        r.a.wiring.spoken = true;
        r.a.jarvis.sent();
        r.hook(&[sess("s1", 5, "working", json!({}))]);
        r.hook(&[sess("s1", 5, "idle", json!({ "reply": "First answer." }))]);
        assert_eq!(r.played(), 1, "read once");
        assert_eq!(r.a.jarvis.log.last().unwrap().text, "First answer.", "shown, and before it was read");
        r.hook(&[sess("s1", 5, "idle", json!({ "reply": "First answer." }))]);
        assert_eq!(r.played(), 1, "the same answer again is not read again");

        // The next answer: its speech is being made when talk is pressed.
        r.hook(&[sess("s1", 5, "working", json!({}))]);
        r.a.jarvis_bus("session", &sess("s1", 5, "idle", json!({ "reply": "Second answer." })).to_string());
        assert!(r.a.jarvis.reading.is_some());
        r.a.intents("jarvis", vec![Intent::TalkStart]);
        assert!(r.a.jarvis.reading.is_none(), "the panel says at once that the reading ended");
        r.settle();
        assert_eq!(r.played(), 1, "the speech that arrived after talk was pressed is never played");
        assert!(r.a.jarvis.recording);
        // An answer arriving while the microphone is open is shown, not read.
        r.hook(&[sess("s1", 5, "working", json!({}))]);
        r.hook(&[sess("s1", 5, "idle", json!({ "reply": "Third answer." }))]);
        assert_eq!(r.a.jarvis.log.last().unwrap().text, "Third answer.");
        assert_eq!(r.played(), 1);
        // Released: nothing comes back by itself.
        r.a.intents("jarvis", vec![Intent::TalkStop]);
        r.settle();
        assert_eq!(r.played(), 1, "no reading resumes after the capture");
        // Its read-aloud button still reads it, on demand.
        let id = r.a.jarvis.log.iter().find(|m| m.text == "Second answer.").unwrap().id;
        r.a.intents("jarvis", vec![Intent::ReadAloud { id }]);
        r.settle();
        assert_eq!(r.played(), 2);
    }

    /// A send refused because Jarvis is not running: no line in the
    /// conversation, the text kept, the error beside the input with "Start
    /// Jarvis and send"; gone once Jarvis runs or a send succeeds; the kept
    /// text leaves by itself only when that button was pressed.
    #[test]
    fn a_refused_send_offers_start_and_send_and_clears() {
        let mut r = Run::new("absent");
        r.enter("changez la voix");
        assert!(r.a.jarvis.log.is_empty(), "not in the conversation: {:?}", r.shown());
        assert_eq!(r.input(), "changez la voix", "kept");
        let e = r.a.jarvis.send_error.clone().expect("shown beside the input");
        assert_eq!((e.offer, e.queued, e.input.as_str()), ("start", false, "changez la voix"));
        assert_eq!(r.a.jarvis.screen_data(true)["send"]["offer"], "start");
        // Plain Start: once running the error goes, and nothing is sent.
        r.a.intents("jarvis", vec![Intent::StartJarvis]);
        r.settle();
        r.a.on_jevent(JEvent::Status(Ok(json!({ "state": "running" }))));
        r.settle();
        assert!(r.a.jarvis.send_error.is_none());
        assert!(r.pushed().is_empty(), "never sent without the person asking");
        r.enter("changez la voix");
        assert_eq!(r.pushed().len(), 1);
        assert!(r.a.jarvis.send_error.is_none());

        // "Start Jarvis and send": sent once running, the input emptied.
        let mut r = Run::new("stopped");
        r.enter("use the english voice");
        r.a.intents("jarvis", vec![Intent::StartJarvis, Intent::SendToJarvis { input: INPUT.into() }]);
        r.settle();
        assert!(r.a.jarvis.send_error.as_ref().is_some_and(|e| e.queued));
        assert!(r.pushed().is_empty());
        r.a.on_jevent(JEvent::Status(Ok(json!({ "state": "running" }))));
        r.settle();
        assert_eq!(r.pushed(), vec!["use the english voice".to_string()]);
        assert_eq!(r.input(), "");
        assert!(r.a.jarvis.send_error.is_none());
        // A later Enter while not running and no button: nothing queued.
        let mut r = Run::new("starting");
        r.enter("hello");
        assert_eq!(r.a.jarvis.send_error.as_ref().map(|e| (e.offer, e.queued)), Some(("wait", false)));
        r.a.on_jevent(JEvent::Status(Ok(json!({ "state": "running" }))));
        r.settle();
        assert!(r.pushed().is_empty());
    }
}
