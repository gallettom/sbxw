//! Dictation into a chat input: a screen's `input` or `textarea` with
//! `tone = "compose"` (the Sessions composer, Jarvis's input, Canvas's ask
//! and remarks, a request's answer).
//!
//! The same path as Jarvis's push-to-talk, and no other: the microphone
//! through [`AudioIo`](crate::voice::AudioIo) (`audio.rs`), then
//! `POST /api/voice/transcribe`, which turns the speech into text on this
//! machine and masks it with the person's anonymise rules before answering
//! (`crates/daemon/src/http/ui.rs`, `voice_transcribe`): the shell only ever
//! holds masked words, and when masking fails the daemon answers an error
//! and nothing is inserted. The words go into the field at its caret, as
//! if typed ([`crate::screen::type_into`]); nothing is sent: the person
//! presses Send and gets the core's Confirm as for typed words.
//!
//! Hold the dictate key (`[voice] dictate_key`, F9) while a chat input has
//! the keyboard, or press the composer's microphone (`dictate` intent) and
//! press again to stop; Escape cancels. With the Sessions composer shown as
//! the base view and nothing else holding the keyboard, the key focuses it
//! and dictates there. A terminal with the keyboard keeps the key.
//!
//! One capture at a time in the app: Jarvis's push-to-talk ends a
//! dictation (it is dropped, said so) and refuses one while it listens.
//! Words are never logged: seconds and counts only.

use super::App;
use crate::client;
use crate::jarvis;
use crate::screen;
use rune::{Key, KeyEvent, Tx};
use serde_json::{json, Value as J};
use std::time::Instant;

/// The key when the daemon does not say (`[voice] dictate_key`'s default).
pub const DEFAULT_KEY: &str = "F9";
/// Recordings shorter than this are taken for a slip of the key (0.3 s).
const MIN_CAPTURE_BYTES: usize = 16_000 * 4 * 3 / 10;

/// Where a dictation's words go.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub screen: String,
    /// The input's key (its id).
    pub input: String,
    /// The input's `arg` when the take began: the Sessions composer's
    /// session. Words for a session the composer left are not inserted.
    pub arg: J,
    /// Held by its key (released: stop), or started by a press (press again).
    pub held: bool,
    /// The field did not have the keyboard: the words go at its end.
    pub at_end: bool,
    pub started: Instant,
}

#[derive(Default)]
pub struct Dictation {
    /// The capture under way.
    pub on: Option<Take>,
    /// Words being transcribed, for this generation and take.
    pub pending: Option<(u64, Take)>,
    /// Bumped by every start and cancel: an older transcription is dropped.
    pub gen: u64,
    pub level: f32,
    /// `[voice] dictate_key`, as the daemon says it.
    pub key_name: Option<String>,
    /// The last refusal or failure, for the field it was about.
    pub notice: Option<(String, String, String)>,
}

/// Whether `key` is an `input` or `textarea` of tree `n`.
fn is_field(n: &crate::luau::Node, key: &str) -> bool {
    (matches!(n.kind, crate::luau::Kind::Input | crate::luau::Kind::Textarea) && screen::key_of(n).as_deref() == Some(key)) || n.children.iter().any(|c| is_field(c, key))
}

impl Dictation {
    pub fn key_name(&self) -> &str {
        self.key_name.as_deref().unwrap_or(DEFAULT_KEY)
    }

    fn key(&self) -> Option<Key> {
        jarvis::key_of(self.key_name()).ok().and_then(|k| k.first().cloned())
    }
}

impl App {
    /// Why no dictation may start into `screen`'s input `input` now, for a person.
    pub(super) fn dictate_refusal(&self, screen: &str, input: &str) -> Option<String> {
        if self.feats.dialog.is_some() {
            return Some("Not dictating: a dialog is open. Answer it first.".into());
        }
        if self.jarvis.shown.is_some() {
            return Some("Not dictating: Jarvis's proposal is waiting for your answer.".into());
        }
        if self.palette.is_some() {
            return Some("Not dictating: close the command palette first.".into());
        }
        if self.feats.modal.as_ref().is_some_and(|(m, _)| m != screen) {
            return Some("Not dictating: a window is open over this view.".into());
        }
        let over_base = self.feats.base.as_deref() == Some(screen) && self.feats.overlay.as_ref().is_some_and(|(o, _)| o != screen);
        if over_base {
            return Some("Not dictating: another view is open over this one.".into());
        }
        if self.jarvis.recording {
            return Some("Not dictating: Jarvis is listening. Release its key first.".into());
        }
        if !self.audio.available() {
            return Some("Not dictating: this build of the app has no audio.".into());
        }
        match &self.jarvis.voice {
            None => return Some("Not dictating: the daemon's voice is not reachable.".into()),
            Some(v) if v["compiled"] == false => return Some("Not dictating: this sbxw was built without voice.".into()),
            Some(v) if v["ready"] != true => return Some("Not dictating: voice is not installed. Install it in Setup, Voice (or run sbxw voice install).".into()),
            _ => {}
        }
        let st = self.screens.iter().find(|s| s.name == screen)?;
        match st.last_good.as_ref().and_then(|t| screen::find_compose(t, Some(input))) {
            None => Some("Not dictating: there is no chat input there.".into()),
            Some(n) if n.disabled => Some("Not dictating: this field takes no words now.".into()),
            Some(_) => None,
        }
    }

    /// A refusal or failure: a notice, and beside the field it was about.
    fn dictate_notice(&mut self, screen: &str, input: &str, text: String, error: bool) {
        self.dictation.notice = Some((screen.to_string(), input.to_string(), text.clone()));
        self.notify(text, error);
    }

    /// Starts a dictation into `screen`'s input `input`; true when it records.
    pub(super) fn dictate_start(&mut self, screen: &str, input: &str, held: bool, at_end: bool) -> bool {
        if self.dictation.on.is_some() {
            return false;
        }
        if let Some(why) = self.dictate_refusal(screen, input) {
            log::info!("dictation refused");
            self.dictate_notice(screen, input, why, false);
            return false;
        }
        let arg = self.screens.iter().find(|s| s.name == screen).and_then(|s| s.last_good.as_ref()).and_then(|t| screen::find_compose(t, Some(input))).map(|n| n.arg.clone()).unwrap_or(J::Null);
        // The microphone must not hear Jarvis read.
        self.stop_reading();
        match self.audio.start_capture() {
            Ok(()) => {
                self.dictation.gen += 1;
                self.dictation.level = 0.0;
                self.dictation.notice = None;
                self.dictation.on = Some(Take { screen: screen.into(), input: input.into(), arg, held, at_end, started: Instant::now() });
                log::info!("dictation: recording into {screen}");
                true
            }
            Err(e) => {
                self.dictate_notice(screen, input, format!("Cannot record: {e}"), true);
                false
            }
        }
    }

    /// Stops the recording; its words are transcribed (and masked) by the daemon.
    pub(super) fn dictate_stop(&mut self) {
        let Some(take) = self.dictation.on.take() else { return };
        match self.audio.stop_capture() {
            Ok(pcm) if pcm.len() < MIN_CAPTURE_BYTES => {
                let k = self.dictation.key_name().to_string();
                self.dictate_notice(&take.screen, &take.input, format!("Too short: hold {k} while you speak, or press the microphone, speak, and press it again."), false);
            }
            Ok(pcm) => {
                log::info!("dictation: {:.1} s recorded", pcm.len() as f32 / (16_000.0 * 4.0));
                let gen = self.dictation.gen;
                self.dictation.pending = Some((gen, take.clone()));
                if !self.jarvis_job(move |d| super::jarvis::JEvent::Dictated { gen, result: d.transcribe(pcm).map_err(|e| client::reason(&e)) }) {
                    self.dictation.pending = None;
                    self.dictate_notice(&take.screen, &take.input, "Not inserted: not connected to the daemon.".into(), true);
                }
            }
            Err(e) => self.dictate_notice(&take.screen, &take.input, format!("Recording failed: {e}"), true),
        }
    }

    /// Drops the recording, or the words on their way: nothing is inserted.
    pub(super) fn dictate_cancel(&mut self, why: Option<&str>) -> bool {
        let had = self.dictation.on.take();
        if had.is_some() {
            self.audio.cancel_capture();
        }
        let pending = self.dictation.pending.take();
        if had.is_none() && pending.is_none() {
            return false;
        }
        self.dictation.gen += 1;
        log::info!("dictation: cancelled");
        if let (Some(t), Some(w)) = (had.or(pending.map(|p| p.1)), why) {
            self.dictate_notice(&t.screen, &t.input, w.to_string(), false);
        }
        true
    }

    /// What the daemon answered: masked words into the field, or why not.
    pub(super) fn dictated(&mut self, gen: u64, result: Result<String, String>) {
        let Some((g, take)) = self.dictation.pending.take() else { return };
        if g != gen || gen != self.dictation.gen {
            self.dictation.pending = Some((g, take));
            log::info!("dictation: a stale transcription dropped");
            return;
        }
        match result {
            Ok(words) if words.trim().is_empty() => self.dictate_notice(&take.screen, &take.input, "Nothing heard.".into(), false),
            Ok(words) => match screen::type_into(self, &take.screen, &take.input, &take.arg, &words, take.at_end) {
                Ok(n) => log::info!("dictation: {n} characters inserted"),
                Err(why) => self.dictate_notice(&take.screen, &take.input, format!("Not inserted: {why}."), true),
            },
            // The daemon masks before it answers: no answer, no words.
            Err(e) => self.dictate_notice(&take.screen, &take.input, format!("Not inserted: the words could not be transcribed and masked ({e})."), true),
        }
    }

    /// The composer's microphone (`dictate` intent): starts, or stops the
    /// dictation into that input.
    pub(super) fn dictate_press(&mut self, screen: &str, input: &str) {
        match &self.dictation.on {
            Some(t) if t.screen == screen && t.input == input => self.dictate_stop(),
            Some(_) => {
                self.dictate_notice(screen, input, "Not dictating: a dictation into another field is under way.".into(), false);
            }
            None => {
                self.dictate_start(screen, input, false, false);
            }
        }
    }

    /// The dictate key pressed: true when it was taken.
    pub(super) fn dictate_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let m = ev.mods;
        if m.cmd || m.ctrl || m.alt || m.logo || Some(&ev.key) != self.dictation.key().as_ref() {
            return false;
        }
        if let Some(t) = &self.dictation.on {
            // A held key repeats; a press during a pressed-on dictation stops it.
            if !t.held {
                self.dictate_stop();
            }
            return true;
        }
        let focused = tx.focused().map(str::to_string);
        // The chat input that has the keyboard.
        if let Some(k) = &focused {
            let hit = self.screens.iter().find(|s| s.last_good.as_ref().is_some_and(|t| screen::find_compose(t, Some(k)).is_some())).map(|s| s.name.clone());
            if let Some(sc) = hit {
                self.dictate_start(&sc, k, true, false);
                return true;
            }
        }
        // A terminal, or another field (a filter, a form), keeps its key.
        if let Some(k) = &focused {
            let field = self.screens.iter().any(|s| s.last_good.as_ref().is_some_and(|t| is_field(t, k)));
            if k.starts_with("grid") || field || crate::surface::focused().is_some() {
                return false;
            }
        }
        // Nothing else that types has the keyboard: the base view's chat
        // input (the Sessions composer), when nothing is over it.
        let Some(base) = self.feats.base.clone() else { return false };
        if self.feats.overlay.as_ref().is_some_and(|(o, _)| *o != base) {
            return false;
        }
        let key = self.screens.iter().find(|s| s.name == base).and_then(|s| s.last_good.as_ref()).and_then(|t| screen::find_compose(t, None)).and_then(screen::key_of);
        let Some(key) = key else { return false };
        tx.focus(&key, true);
        self.dictate_start(&base, &key, true, true);
        true
    }

    /// The dictate key released: a held dictation stops.
    pub(super) fn dictate_key_up(&mut self, ev: &KeyEvent) -> bool {
        if Some(&ev.key) == self.dictation.key().as_ref() && self.dictation.on.as_ref().is_some_and(|t| t.held) {
            self.dictate_stop();
            return true;
        }
        false
    }

    /// An audio event of the capture, while it is a dictation's: true when taken.
    pub(super) fn dictate_audio(&mut self, ev: &crate::voice::AudioEvent) -> bool {
        use crate::voice::AudioEvent as E;
        let Some(t) = self.dictation.on.clone() else { return false };
        match ev {
            E::CaptureStarted => {}
            E::CaptureLevel(l) => self.dictation.level = l.clamp(0.0, 1.0),
            E::CaptureEnded => self.dictate_stop(),
            E::CaptureFailed(m) => {
                self.dictation.on = None;
                self.dictate_notice(&t.screen, &t.input, format!("The microphone: {m}"), true);
            }
            _ => return false,
        }
        true
    }

    /// `data.app.dictation`: what the chat inputs show (the field being
    /// dictated into, its level and seconds, the words on their way, the
    /// last notice), and the key.
    pub(super) fn dictation_data(&self) -> J {
        let d = &self.dictation;
        let mut v = json!({ "key": d.key_name(), "available": self.audio.available() && self.jarvis.voice.as_ref().is_some_and(|v| v["ready"] == true) });
        if let Some(t) = &d.on {
            v["recording"] = json!({ "screen": t.screen, "input": t.input, "level": d.level, "secs": t.started.elapsed().as_secs(), "held": t.held });
        }
        if let Some((_, t)) = &d.pending {
            v["transcribing"] = json!({ "screen": t.screen, "input": t.input });
        }
        if let Some((s, i, text)) = &d.notice {
            v["notice"] = json!({ "screen": s, "input": i, "text": text });
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Host;
    use crate::shell::{view, Args};
    use rune::Mods;

    const F9: Key = Key::Function(9);

    fn frames(rt: &mut rune::Runtime<App>, n: usize) {
        for _ in 0..n {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    }

    fn tap(rt: &mut rune::Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        frames(rt, 3);
    }

    fn press(rt: &mut rune::Runtime<App>, k: Key) {
        rt.key(KeyEvent { key: k, mods: Mods::default() });
        frames(rt, 2);
    }

    fn release(rt: &mut rune::Runtime<App>, k: Key) {
        rt.key_up(KeyEvent { key: k, mods: Mods::default() });
        frames(rt, 4);
    }

    fn focused(rt: &mut rune::Runtime<App>) -> String {
        let mut f = String::new();
        rt.update(|_, tx| f = tx.focused().unwrap_or("").to_string());
        f
    }

    /// The window on `fixture`, the Sessions view shown (GUI) on a session
    /// that takes words.
    fn sessions(fixture: &str) -> rune::Runtime<App> {
        let fx = format!("{}/fixtures/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
        let a = App::new(Args { fixture: Some(fx), ..Default::default() });
        let mut rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        rt.frame(0.0);
        frames(&mut rt, 4);
        tap(&mut rt, "top-view-gui");
        frames(&mut rt, 4);
        pick(&mut rt, true);
        rt
    }

    /// Shows a session whose composer takes words (`open`) or one that does not.
    fn pick(rt: &mut rune::Runtime<App>, open: bool) {
        let keys: Vec<String> = rt.state.screen_data()["monitor"]["rows"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["key"].as_str().map(str::to_string)).collect();
        for k in keys {
            rt.state.intents("monitor", vec![crate::intent::Intent::Feature(crate::feat::Asked { name: "monitor_select", args: serde_json::json!({ "key": k }).as_object().cloned().unwrap() })]);
            frames(rt, 3);
            if rt.state.screen_data()["monitor"]["detail"]["compose"]["can"] == open {
                return;
            }
        }
        panic!("no session with can = {open}");
    }

    fn draft(rt: &mut rune::Runtime<App>) -> String {
        rt.state.screen_mut("monitor").unwrap().ui.edits.get("monitor-compose").map(|e| e.0.text.clone()).unwrap_or_default()
    }

    fn sent(rt: &rune::Runtime<App>) -> bool {
        rt.state.feats.dialog.is_some()
    }

    fn notice(rt: &rune::Runtime<App>) -> String {
        rt.state.dictation.notice.as_ref().map(|n| n.2.clone()).unwrap_or_default()
    }

    /// F9 held in the composer: the daemon's (masked) words land at the
    /// caret, after what was typed, and the screen's draft has them; no
    /// Send, no dialog, no Enter.
    #[test]
    fn f9_held_in_the_composer_types_masked_words_and_sends_nothing() {
        let mut rt = sessions("monitor/dictate");
        tap(&mut rt, "monitor-compose");
        assert_eq!(focused(&mut rt), "monitor-compose");
        rt.key(KeyEvent { key: Key::Text("Hi,".into()), mods: Mods::default() });
        frames(&mut rt, 2);
        press(&mut rt, F9);
        assert!(rt.state.dictation.on.as_ref().is_some_and(|t| t.held && t.input == "monitor-compose"), "recording");
        assert_eq!(rt.state.screen_data()["app"]["dictation"]["recording"]["input"], "monitor-compose");
        // The key repeats while held: still one take.
        press(&mut rt, F9);
        release(&mut rt, F9);
        assert!(rt.state.dictation.on.is_none() && rt.state.dictation.pending.is_none());
        assert_eq!(draft(&mut rt), "Hi, please send the report to [email] today");
        assert!(!sent(&rt), "dictation opens no Confirm");
        // Send is still the person's press, and its one Confirm shows the
        // screen's draft, which has the words (and no Enter).
        tap(&mut rt, "monitor-send");
        frames(&mut rt, 6);
        let body = rt.state.feats.dialog.as_ref().and_then(|d| d.confirm.as_ref()).map(|c| c.call.body.to_string()).unwrap_or_default();
        assert!(body.contains("Hi, please send the report to [email] today"), "{body}");
        assert!(!body.contains("\\n"), "no Enter: {body}");
    }

    /// The microphone: press, speak, press again; Escape cancels.
    #[test]
    fn the_microphone_starts_and_stops_and_escape_cancels() {
        let mut rt = sessions("monitor/dictate");
        tap(&mut rt, "monitor-dictate");
        assert!(rt.state.dictation.on.as_ref().is_some_and(|t| !t.held), "recording by press");
        assert!(rt.find("monitor-dictating").is_some(), "the recording state shows");
        tap(&mut rt, "monitor-dictate");
        assert_eq!(draft(&mut rt), "please send the report to [email] today");
        // Again, then Escape: nothing is inserted, and the view stays.
        tap(&mut rt, "monitor-dictate");
        assert!(rt.state.dictation.on.is_some());
        press(&mut rt, Key::Escape);
        assert!(rt.state.dictation.on.is_none(), "cancelled");
        assert_eq!(draft(&mut rt), "please send the report to [email] today", "nothing more");
        assert!(notice(&rt).contains("cancelled"));
        assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"));
    }

    /// F9 with nothing typing: the base composer gets the keyboard and the
    /// words, at its end.
    #[test]
    fn f9_unfocused_goes_to_the_composer() {
        let mut rt = sessions("monitor/dictate");
        rt.update(|_, tx| tx.blur());
        frames(&mut rt, 1);
        press(&mut rt, F9);
        assert_eq!(focused(&mut rt), "monitor-compose");
        release(&mut rt, F9);
        assert_eq!(draft(&mut rt), "please send the report to [email] today");
    }

    /// Masking failed (here the daemon gives no transcript): nothing is
    /// inserted, and the notice says so.
    #[test]
    fn a_failed_masking_inserts_nothing() {
        let mut rt = sessions("monitor/dictate-unmasked");
        tap(&mut rt, "monitor-compose");
        press(&mut rt, F9);
        release(&mut rt, F9);
        assert_eq!(draft(&mut rt), "");
        assert!(notice(&rt).starts_with("Not inserted: the words could not be transcribed and masked"), "{}", notice(&rt));
    }

    /// Refused in Rust, with what to do: a closed composer, the palette,
    /// Jarvis's proposal, a dialog, no voice installed; and while Jarvis
    /// listens. Nothing records.
    #[test]
    fn refusals_say_why() {
        let mut rt = sessions("monitor/dictate");
        let refused = |rt: &mut rune::Runtime<App>| {
            let r = rt.state.dictate_start("monitor", "monitor-compose", false, false);
            assert!(!r && rt.state.dictation.on.is_none());
            notice(rt)
        };
        rt.state.jarvis.voice.as_mut().unwrap()["ready"] = false.into();
        assert!(refused(&mut rt).contains("voice is not installed"));
        rt.state.jarvis.voice.as_mut().unwrap()["ready"] = true.into();
        rt.state.jarvis.recording = true;
        assert!(refused(&mut rt).contains("Jarvis is listening"));
        rt.state.jarvis.recording = false;
        rt.state.jarvis.shown = Some("p1".into());
        assert!(refused(&mut rt).contains("Jarvis's proposal"));
        rt.state.jarvis.shown = None;
        rt.state.palette_open();
        assert!(refused(&mut rt).contains("palette"));
        rt.state.palette = None;
        rt.state.feats.overlay = Some(("files".into(), J::Null));
        assert!(refused(&mut rt).contains("over this one"));
        rt.state.feats.overlay = Some(("monitor".into(), J::Null));
        pick(&mut rt, false);
        assert!(refused(&mut rt).contains("takes no words now"));
        assert!(rt.state.screen_data()["monitor"]["detail"]["compose"]["can"] == false);
    }

    /// Jarvis's push-to-talk comes first: it ends a dictation (nothing
    /// inserted), and one capture runs at a time.
    #[test]
    fn push_to_talk_ends_a_dictation() {
        let mut rt = sessions("monitor/dictate");
        tap(&mut rt, "monitor-dictate");
        assert!(rt.state.dictation.on.is_some());
        press(&mut rt, Key::Function(8));
        assert!(rt.state.jarvis.recording && rt.state.dictation.on.is_none(), "Jarvis listens, the dictation is gone");
        assert!(notice(&rt).contains("Jarvis is listening"));
        rt.state.dictate_press("monitor", "monitor-compose");
        assert!(rt.state.dictation.on.is_none(), "refused while Jarvis listens");
        release(&mut rt, Key::Function(8));
        assert_eq!(draft(&mut rt), "", "the dictation's words never came");
    }

    /// The words follow the session the dictation began in: another
    /// session shown meanwhile gets nothing.
    #[test]
    fn the_words_stay_with_their_session() {
        let mut rt = sessions("monitor/dictate");
        tap(&mut rt, "monitor-dictate");
        let first = rt.state.screen_data()["monitor"]["sel"].clone();
        let keys: Vec<String> = rt.state.screen_data()["monitor"]["rows"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["key"].as_str().map(str::to_string)).collect();
        let other = keys.into_iter().find(|k| J::String(k.clone()) != first).unwrap();
        rt.state.intents("monitor", vec![crate::intent::Intent::Feature(crate::feat::Asked { name: "monitor_select", args: serde_json::json!({ "key": other }).as_object().cloned().unwrap() })]);
        frames(&mut rt, 3);
        rt.state.dictate_stop();
        frames(&mut rt, 4);
        assert_eq!(draft(&mut rt), "", "nothing in the other session's draft");
        assert!(notice(&rt).contains("session changed") || notice(&rt).contains("takes no words"), "{}", notice(&rt));
    }

    /// A terminal with the keyboard keeps F9 (the panes, no base view): no
    /// dictation starts.
    #[test]
    fn f9_in_a_terminal_is_not_a_chat_dictation() {
        let fx = format!("{}/fixtures/monitor/dictate.json", env!("CARGO_MANIFEST_DIR"));
        let a = App::new(Args { fixture: Some(fx), ..Default::default() });
        let mut rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        frames(&mut rt, 4);
        rt.update(|_, tx| tx.focus("grid-1", false));
        press(&mut rt, F9);
        assert!(rt.state.dictation.on.is_none());
    }
}
