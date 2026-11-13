//! The shell's side of Jarvis: the daemon calls (off the UI thread), the
//! bus events, the intents, push-to-talk and reading aloud through
//! [`AudioIo`](crate::voice::AudioIo), the docked panel and the proposal
//! dialog. The rules themselves are in `crate::jarvis`.

use super::{headless, App};
use crate::approval::{self, ProposalAnswers};
use crate::client::{self, Daemon};
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
    Speech { msg: u64, result: Result<(u32, crate::voice::Chain), String> },
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
    if headless() {
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
    if headless() {
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
                    return false;
                }
                if !self.jarvis.running() {
                    self.wiring.status_due = Some(Instant::now());
                }
                if let Some(reply) = self.jarvis.ingest_session(&v) {
                    if self.wiring.spoken {
                        let id = self.jarvis.log.last().map(|m| m.id).unwrap_or(0);
                        self.read(id, reply);
                    }
                }
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
                // The dialog shows it; the voice reads the summary when the person talks.
                if self.wiring.spoken {
                    let high = self.jarvis.proposal(&id).is_some_and(jarvis::screen_only);
                    let say = if high { format!("Jarvis proposes: {summary}. {CLICK_NEEDED}") } else { format!("Jarvis proposes: {summary}. Shall I apply it?") };
                    let msg = self.jarvis.log.last().map(|m| m.id).unwrap_or(0);
                    self.read(msg, say);
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
        let mut changed = false;
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
                if state == "running" && self.jarvis.status != "running" {
                    self.jarvis_job(|d| JEvent::Sessions(d.sessions().unwrap_or_default()));
                }
                self.jarvis.status = state;
            }
            JEvent::Status(Err(e)) => {
                self.jarvis.status = "unknown".into();
                self.jarvis.status_error = Some(e);
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
                        self.jarvis.working = true;
                        if masked > 0 {
                            self.jarvis.say(Who::Note, format!("Sent with {masked} name{} masked: {text}", if masked == 1 { "" } else { "s" }));
                        }
                    }
                    Err(e) => {
                        self.jarvis.say(Who::Error, format!("Not sent: {e}"));
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
            JEvent::Speech { msg, result } => match result {
                Ok((rate, chain)) => match self.audio.play(rate, Box::new(chain)) {
                    Ok(()) => self.jarvis.reading = Some(msg),
                    Err(e) => {
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
        self.jarvis.say(Who::Note, CLICK_NEEDED);
        if self.wiring.spoken {
            let msg = self.jarvis.log.last().map(|m| m.id).unwrap_or(0);
            self.read(msg, CLICK_NEEDED.to_string());
        }
    }

    /// A message to Jarvis: masked, then typed into its session.
    fn send(&mut self, text: String, input: Option<String>) {
        if !self.jarvis.running() {
            let why = match self.jarvis.status.as_str() {
                "creating" | "starting" => "Not sent: Jarvis is starting. Press Enter again once it runs.",
                _ => "Not sent: Jarvis is not running. Start it, then press Enter again.",
            };
            self.jarvis.say(Who::Error, why);
            // Spoken words are not lost: they wait in the input.
            if input.is_none() {
                if let Some(s) = self.screen_mut("jarvis") {
                    if let Some(e) = s.ui.edits.get_mut(INPUT) {
                        e.0.replace_text(text);
                    }
                }
            }
            return;
        }
        if self.jarvis.sending {
            self.jarvis.say(Who::Error, "Not sent: the last message is still on its way.");
            return;
        }
        self.jarvis.say(Who::Me, &text);
        self.jarvis.sending = true;
        let started = self.jarvis_job(move |d| {
            let r = d.anonymise(&text).and_then(|(safe, n)| d.chat_push(jarvis::SANDBOX, &safe).map(|()| (safe, n)));
            JEvent::Sent { input, result: r.map_err(|e| client::reason(&e)) }
        });
        if !started {
            self.jarvis.sending = false;
            let why = self.connection.text.clone();
            self.jarvis.say(Who::Error, format!("Not sent: {why}"));
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
        self.jarvis.reading = None;
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

    /// Reads `text` aloud, for message `msg`.
    fn read(&mut self, msg: u64, text: String) {
        if !self.audio.available() || !self.jarvis.voice.as_ref().is_some_and(|v| v["speech"]["supported"] == true && v["ready"] == true) {
            return;
        }
        let pieces = jarvis::speech_pieces(&text, 1000);
        if pieces.is_empty() {
            return;
        }
        self.audio.stop_playback();
        self.jarvis.reading = Some(msg);
        let Some(d) = self.daemon.clone() else { return };
        self.jarvis_job(move |_| JEvent::Speech { msg, result: crate::voice::Chain::start(d, pieces).map_err(|e| client::reason(&e)) });
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
                    self.audio.stop_playback();
                    self.jarvis.reading = None;
                } else if let Some(m) = self.jarvis.log.iter().find(|m| m.id == id).cloned() {
                    self.read(id, m.text);
                }
            }
            Intent::StopReading => {
                self.audio.stop_playback();
                self.jarvis.reading = None;
            }
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
            left: layout::GUTTER * 2.0 + layout::SIDEBAR_W + 16.0,
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
                let most = (s.window.0 - layout::SIDEBAR_W - 320.0).clamp(jarvis::MIN_W, jarvis::MAX_W);
                s.jarvis.width = (s.wiring.drag_from - d.dx).clamp(jarvis::MIN_W, most);
                if matches!(d.phase, rune::Phase::Up) {
                    save_prefs(&s.jarvis);
                }
            });
        let tile = col()
            .key("jarvis-panel")
            .role("complementary")
            .label("Jarvis")
            .w(self.jarvis.width)
            .h_full()
            .min_h(0.0)
            .padding(10.0)
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
