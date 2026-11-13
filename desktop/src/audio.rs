//! Native audio for Jarvis: the microphone (push-to-talk) and the voice.
//!
//! Recognition and synthesis stay in the daemon; this module only records
//! and plays, in the daemon's own formats (see [`pcm`] for the file and line
//! of each):
//! * [`Audio::stop_capture`] returns the body of `POST /api/voice/transcribe`:
//!   16 kHz mono little-endian `f32` PCM, no header, at most 120 s;
//! * [`Audio::play`] takes the body of `POST /api/voice/speak` as it streams:
//!   16-bit little-endian mono PCM at the `x-sample-rate` rate (24000).
//!
//! No dependency on the UI, the shell or the daemon client. Audio is never
//! written anywhere and never logged: the log gets durations, byte counts
//! and device names only.
//!
//! # API (the seam `crate::voice::AudioIo`, thread desktop-jarvis)
//!
//! ```text
//! Audio::new(wake) / Audio::boxed(wake)    // spawns the control thread
//! available()          -> bool             // false: no device at all
//! start_capture()      -> Result<(), String>   // stops playback first
//! stop_capture()       -> Result<Vec<u8>, String>  // the recording, or empty
//! cancel_capture()
//! play(rate, reader)   -> Result<(), String>   // replaces what was playing
//! stop_playback()
//! events()             -> Vec<AudioEvent>  // drained on the UI thread
//! ```
//!
//! Every method returns at once except `stop_capture`, which waits for the
//! control thread to hand the recording over (a few ms; at most
//! [`STOP_WAIT`]). Opening a device happens on the control thread, so a
//! device that cannot be opened is reported as `CaptureFailed` or
//! `PlaybackFailed`, not as the method's `Err` (that one means the audio
//! thread itself is gone). After queueing events, the control thread calls
//! the `wake` it was given, as `client.rs` wakes the UI.
//!
//! Semantics agreed with desktop-jarvis through the lead:
//! 1. `play` while something plays replaces it with no `PlaybackEnded` for the
//!    replaced one; `stop_playback` gives one `PlaybackEnded` if something
//!    was playing.
//! 2. Events of a capture or playback that was stopped or replaced before the
//!    UI drained them are dropped by `events()` (a `CaptureFailed` is always
//!    kept).
//! 3. The reader of `play` is read on its own thread, never in the device
//!    callback: a `read` that blocks (the next batch being synthesised) is an
//!    underrun, heard as silence that fades and resumes softly, with no
//!    event. `PlaybackEnded` comes at the reader's end once its sound has
//!    been handed to the device; a read error is `PlaybackFailed`. A stop or
//!    a replacement returns at once even while that thread is blocked in
//!    `read`: it ends by itself at its next read and its output is dropped.
//! 4. Exact digital silence (or no data) for 1 s is reported as
//!    `CaptureFailed`, with what to do on macOS: that is how a refused
//!    microphone permission shows there.
//!
//! # Threads
//!
//! * The device callbacks (real time) touch only a lock-free SPSC ring
//!   ([`ring`]) and atomics: no allocation, no lock ([`backend`]).
//! * The control thread ([`engine`]) owns the streams (cpal streams are not
//!   `Send` everywhere), drains and fills the rings every 10 ms, resamples
//!   ([`resample`]), measures, and sends events.
//! * One reader thread per `play`, reading and decoding the body.

pub mod backend;
pub mod engine;
pub mod pcm;
pub mod resample;
pub mod ring;

use std::io::Read;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use backend::{Backend, CpalBackend};
use engine::{Cmd, Engine, Tag, AVAIL_NO};

// ------------------------------------------------------------ seam mirror
// A copy of `desktop/src/voice.rs` on branch sbxw/desktop-jarvis (commit
// a346ed8), kept identical so that after the merge the adapter is
// `impl voice::AudioIo for Audio` forwarding each method, and these two
// copies are deleted in favour of `crate::voice`.

/// Sample rate of the capture `stop_capture` returns.
pub const CAPTURE_RATE: u32 = 16_000;
/// Longest recording the daemon accepts (`svc::voice::MAX_SAMPLES`): 120 s.
pub const MAX_CAPTURE_SECS: u32 = 120;

/// What the audio side reports (see `voice.rs` for each variant).
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    CaptureStarted,
    CaptureLevel(f32),
    CaptureEnded,
    CaptureFailed(String),
    PlaybackStarted,
    PlaybackLevel(f32),
    PlaybackEnded,
    PlaybackFailed(String),
}

/// Called from an audio thread after queueing an event (the UI's waker).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Native audio, as the Jarvis panel uses it.
pub trait AudioIo: Send {
    fn available(&self) -> bool;
    fn start_capture(&mut self) -> Result<(), String>;
    fn stop_capture(&mut self) -> Result<Vec<u8>, String>;
    fn cancel_capture(&mut self);
    fn play(&mut self, rate: u32, pcm: Box<dyn Read + Send>) -> Result<(), String>;
    fn stop_playback(&mut self);
    fn events(&mut self) -> Vec<AudioEvent>;
}

/// Builds the implementation once the UI's waker exists.
pub type AudioFactory = fn(Wake) -> Box<dyn AudioIo>;
// ------------------------------------------------------- end seam mirror

/// The longest `stop_capture` waits for the control thread.
pub const STOP_WAIT: Duration = Duration::from_millis(500);

/// What went wrong, for a person: [`AudioError::message`] is shown as is.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NoDevice,
    DeviceGone,
    Permission,
    Busy,
    Unsupported,
    /// An overrun or underrun reported by the device: logged, not shown.
    Glitch,
    Other,
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AudioError {}

/// Limits, defaults for the app.
#[derive(Debug, Clone)]
pub struct Options {
    /// A recording stops by itself after this (`CaptureEnded`). The daemon
    /// refuses more than 120 s (svc/voice.rs:35, `MAX_SAMPLES`).
    pub max_capture: Duration,
    /// Exact silence this long fails the capture; `None` never checks.
    pub silence_check: Option<Duration>,
    /// The output device is closed after this long with nothing to play.
    pub idle_close: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_capture: Duration::from_secs(MAX_CAPTURE_SECS as u64),
            silence_check: Some(Duration::from_secs(1)),
            idle_close: Duration::from_secs(5),
        }
    }
}

/// The handle the UI holds. Dropping it ends the control thread.
pub struct Audio {
    cmd: mpsc::Sender<Cmd>,
    events: mpsc::Receiver<(Tag, AudioEvent)>,
    available: Arc<AtomicU8>,
    /// Current capture; bumped on start and on stop, so a stopped capture's
    /// late events are dropped.
    cap_gen: u64,
    capturing: bool,
    /// Current playback, shared with reader threads so a replaced reader
    /// stops at its next read.
    play_gen: Arc<AtomicU64>,
    playing: bool,
    /// Events made on this side (the `PlaybackEnded` of a stop).
    local: Vec<AudioEvent>,
}

impl Audio {
    /// The machine's default devices (cpal).
    pub fn new(wake: Wake) -> Audio {
        Audio::with_backend(|| Box::new(CpalBackend::new()), Options::default(), wake)
    }

    /// For [`AudioFactory`].
    pub fn boxed(wake: Wake) -> Box<dyn AudioIo> {
        Box::new(Audio::new(wake))
    }

    /// Any backend (tests use a fake one). `make` runs on the control thread.
    pub fn with_backend(make: impl FnOnce() -> Box<dyn Backend> + Send + 'static, opts: Options, wake: Wake) -> Audio {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let available = Arc::new(AtomicU8::new(engine::AVAIL_UNKNOWN));
        let avail = available.clone();
        let tx = cmd_tx.clone();
        let spawned = std::thread::Builder::new().name("sbxw-audio".into()).spawn(move || {
            let ev = ev_tx.clone();
            let w = wake.clone();
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                Engine::new(make(), opts, ev_tx, wake, tx).run(cmd_rx, avail.clone());
            }));
            if let Err(p) = run {
                let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                log::error!("audio: the audio thread stopped: {why}");
                let msg = format!("Audio stopped working ({why}). Restart the app to use the voice again.");
                // Tagged so that no generation filter drops them.
                let _ = ev.send((Tag::Capture(u64::MAX), AudioEvent::CaptureFailed(msg.clone())));
                let _ = ev.send((Tag::Play(u64::MAX), AudioEvent::PlaybackFailed(msg)));
                w();
            }
        });
        if let Err(e) = spawned {
            log::error!("audio: no audio thread: {e}");
            available.store(AVAIL_NO, Ordering::Release);
        }
        Audio { cmd: cmd_tx, events: ev_rx, available, cap_gen: 0, capturing: false, play_gen: Arc::new(AtomicU64::new(0)), playing: false, local: Vec::new() }
    }

    fn send(&self, c: Cmd) -> Result<(), String> {
        self.cmd.send(c).map_err(|_| "Audio stopped working. Restart the app to use the voice again.".to_string())
    }

    /// Whether this machine has a device to record or play with. True until
    /// the control thread has looked (a few ms after `new`).
    pub fn available(&self) -> bool {
        self.available.load(Ordering::Acquire) != AVAIL_NO
    }

    /// Whether a recording is open (from this side's view).
    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    /// Whether a playback is going on (from this side's view: until its
    /// `PlaybackEnded` or `PlaybackFailed` has been drained).
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn start_capture(&mut self) -> Result<(), String> {
        self.stop_playback();
        self.cap_gen += 1;
        self.capturing = true;
        self.send(Cmd::StartCapture { gen: self.cap_gen })
    }

    pub fn stop_capture(&mut self) -> Result<Vec<u8>, String> {
        if !self.capturing {
            return Ok(Vec::new());
        }
        self.capturing = false;
        let gen = self.cap_gen;
        self.cap_gen += 1;
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Cmd::StopCapture { gen, reply: tx })?;
        match rx.recv_timeout(STOP_WAIT) {
            Ok(r) => r,
            Err(_) => Err("The audio thread did not hand the recording over in time.".into()),
        }
    }

    pub fn cancel_capture(&mut self) {
        if !self.capturing {
            return;
        }
        self.capturing = false;
        let gen = self.cap_gen;
        self.cap_gen += 1;
        let _ = self.send(Cmd::CancelCapture { gen });
    }

    pub fn play(&mut self, rate: u32, pcm: Box<dyn Read + Send>) -> Result<(), String> {
        if rate == 0 || rate > 384_000 {
            return Err(format!("cannot play sound at {rate} Hz"));
        }
        let gen = self.play_gen.fetch_add(1, Ordering::AcqRel) + 1;
        self.playing = true;
        self.send(Cmd::Play { gen, rate })?;
        let tx = self.cmd.clone();
        let current = self.play_gen.clone();
        std::thread::Builder::new()
            .name("sbxw-audio-read".into())
            .spawn(move || read_speech(pcm, gen, current, tx))
            .map_err(|e| format!("cannot start reading the speech: {e}"))?;
        Ok(())
    }

    pub fn stop_playback(&mut self) {
        // New generation: the reader stops at its next read, and late events
        // of what played are dropped.
        let gen = self.play_gen.fetch_add(1, Ordering::AcqRel);
        if self.playing {
            self.playing = false;
            self.local.push(AudioEvent::PlaybackEnded);
        }
        let _ = self.send(Cmd::StopPlayback { gen });
    }

    pub fn events(&mut self) -> Vec<AudioEvent> {
        let mut out = std::mem::take(&mut self.local);
        let play_gen = self.play_gen.load(Ordering::Acquire);
        while let Ok((tag, ev)) = self.events.try_recv() {
            let keep = match tag {
                Tag::Capture(g) => g == u64::MAX || (g == self.cap_gen && self.capturing) || matches!(ev, AudioEvent::CaptureFailed(_)),
                Tag::Play(g) => g == u64::MAX || g == play_gen,
            };
            if !keep {
                continue;
            }
            match ev {
                AudioEvent::CaptureFailed(_) if matches!(tag, Tag::Capture(g) if g == self.cap_gen || g == u64::MAX) => self.capturing = false,
                AudioEvent::PlaybackEnded | AudioEvent::PlaybackFailed(_) => self.playing = false,
                _ => {}
            }
            out.push(ev);
        }
        out
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.play_gen.fetch_add(1, Ordering::AcqRel);
        let _ = self.cmd.send(Cmd::Quit);
    }
}

impl AudioIo for Audio {
    fn available(&self) -> bool {
        Audio::available(self)
    }
    fn start_capture(&mut self) -> Result<(), String> {
        Audio::start_capture(self)
    }
    fn stop_capture(&mut self) -> Result<Vec<u8>, String> {
        Audio::stop_capture(self)
    }
    fn cancel_capture(&mut self) {
        Audio::cancel_capture(self)
    }
    fn play(&mut self, rate: u32, pcm: Box<dyn Read + Send>) -> Result<(), String> {
        Audio::play(self, rate, pcm)
    }
    fn stop_playback(&mut self) {
        Audio::stop_playback(self)
    }
    fn events(&mut self) -> Vec<AudioEvent> {
        Audio::events(self)
    }
}

/// The reader thread of one `play`: body in, decoded sound to the control
/// thread, until the end, an error, or a newer `play`/stop.
fn read_speech(mut pcm: Box<dyn Read + Send>, gen: u64, current: Arc<AtomicU64>, tx: mpsc::Sender<Cmd>) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut dec = pcm::S16Decoder::default();
    let mut bytes = 0u64;
    let t = std::time::Instant::now();
    loop {
        if current.load(Ordering::Acquire) != gen {
            log::debug!("audio: speech reader {gen} replaced after {bytes} bytes");
            return;
        }
        match pcm.read(&mut buf) {
            Ok(0) => {
                log::info!("audio: speech read, {bytes} bytes in {:.1} s", t.elapsed().as_secs_f32());
                let _ = tx.send(Cmd::PcmEnd { gen, result: Ok(()) });
                return;
            }
            Ok(n) => {
                bytes += n as u64;
                let mut samples = Vec::with_capacity(n / 2 + 1);
                dec.push(&buf[..n], &mut samples);
                if !samples.is_empty() && tx.send(Cmd::Pcm { gen, samples }).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                if current.load(Ordering::Acquire) == gen {
                    log::warn!("audio: speech reader failed after {bytes} bytes: {e}");
                    let _ = tx.send(Cmd::PcmEnd { gen, result: Err(e.to_string()) });
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
