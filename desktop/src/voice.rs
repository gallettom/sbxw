//! The seam between the Jarvis panel and native audio.
//!
//! Thread desktop-audio writes the real implementation in `src/audio.rs`;
//! this file only fixes what the shell calls and what comes back, so both
//! sides can be built at once. [`NullAudio`] is used headless, in fixtures
//! and until the real one is plugged in: it records nothing and plays
//! nothing, and says so through [`AudioIo::available`].
//!
//! Formats are the daemon's, so no conversion happens in the shell:
//! * capture: what `POST /api/voice/transcribe` takes, 16 kHz mono
//!   little-endian `f32` PCM, no header ([`CAPTURE_RATE`]);
//! * playback: what `POST /api/voice/speak` answers, 16-bit signed
//!   little-endian mono PCM, its rate in the `x-sample-rate` header (24000
//!   today), streamed: the body is handed over as a reader so the first
//!   words are heard while the rest is still being made.

use std::io::Read;
use std::sync::Arc;

/// Sample rate of the capture `stop_capture` returns.
pub const CAPTURE_RATE: u32 = 16_000;
/// Longest recording the daemon accepts (`svc::voice::MAX_SAMPLES`): 120 s.
pub const MAX_CAPTURE_SECS: u32 = 120;

/// What the audio side reports, from any thread, through the `wake` it was
/// given: the shell drains them with [`AudioIo::events`] on its own thread.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// The microphone is open and recording.
    CaptureStarted,
    /// Input loudness, 0.0 to 1.0, about 20 times a second while recording.
    CaptureLevel(f32),
    /// The recording stopped by itself (the 120 s cap, the device went away);
    /// the shell then calls `stop_capture` to get what was recorded.
    CaptureEnded,
    /// The microphone could not be opened or failed while recording
    /// (permission refused, no device). The text is shown to the person.
    CaptureFailed(String),
    /// The first sound of a `play` was handed to the device.
    PlaybackStarted,
    /// Output loudness, 0.0 to 1.0, while playing (optional, for the ring).
    PlaybackLevel(f32),
    /// Everything of the reader was played, or `stop_playback` was called.
    PlaybackEnded,
    /// The device or the reader failed; the text is shown to the person.
    PlaybackFailed(String),
}

/// Called from an audio thread after queueing an event (the UI's waker).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Native audio, as the Jarvis panel uses it. All methods are called on the
/// UI thread and must return at once: work happens on the implementation's
/// own threads.
pub trait AudioIo: Send {
    /// Whether this machine can record and play at all. `false` hides the
    /// talk button and the read-aloud action (the null implementation).
    fn available(&self) -> bool;
    /// Open the default input and start recording (push-to-talk pressed).
    /// Stops any playback first. An error is shown to the person.
    fn start_capture(&mut self) -> Result<(), String>;
    /// Stop recording (push-to-talk released) and return the whole
    /// recording: 16 kHz mono f32 LE PCM bytes, at most 120 s. Empty when
    /// nothing was recorded.
    fn stop_capture(&mut self) -> Result<Vec<u8>, String>;
    /// Drop the recording without returning it (Escape, the window lost focus).
    fn cancel_capture(&mut self);
    /// Play 16-bit LE mono PCM at `rate` Hz, read from `pcm` as it arrives
    /// (the body of `/api/voice/speak`). Replaces whatever was playing.
    fn play(&mut self, rate: u32, pcm: Box<dyn Read + Send>) -> Result<(), String>;
    /// Stop the playback now.
    fn stop_playback(&mut self);
    /// Events since the last call, oldest first.
    fn events(&mut self) -> Vec<AudioEvent>;
}

/// Builds the implementation once the UI's waker exists.
pub type AudioFactory = fn(Wake) -> Box<dyn AudioIo>;

/// No audio: headless runs, fixtures, and builds without `audio.rs`.
#[derive(Default)]
pub struct NullAudio {
    queued: Vec<AudioEvent>,
}

impl NullAudio {
    pub fn boxed(_wake: Wake) -> Box<dyn AudioIo> {
        Box::new(NullAudio::default())
    }
}

impl AudioIo for NullAudio {
    fn available(&self) -> bool {
        false
    }
    fn start_capture(&mut self) -> Result<(), String> {
        Err("no audio in this build".into())
    }
    fn stop_capture(&mut self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    fn cancel_capture(&mut self) {}
    fn play(&mut self, _rate: u32, _pcm: Box<dyn Read + Send>) -> Result<(), String> {
        Err("no audio in this build".into())
    }
    fn stop_playback(&mut self) {}
    fn events(&mut self) -> Vec<AudioEvent> {
        std::mem::take(&mut self.queued)
    }
}

/// A scripted implementation for tests and headless demos: capture returns
/// the samples it was given, playback is recorded and ends at once.
#[derive(Default)]
pub struct FakeAudio {
    pub capture: Vec<u8>,
    pub recording: bool,
    pub played: Vec<(u32, usize)>,
    pub queued: Vec<AudioEvent>,
}

impl AudioIo for FakeAudio {
    fn available(&self) -> bool {
        true
    }
    fn start_capture(&mut self) -> Result<(), String> {
        self.recording = true;
        self.queued.push(AudioEvent::CaptureStarted);
        self.queued.push(AudioEvent::CaptureLevel(0.4));
        Ok(())
    }
    fn stop_capture(&mut self) -> Result<Vec<u8>, String> {
        self.recording = false;
        Ok(self.capture.clone())
    }
    fn cancel_capture(&mut self) {
        self.recording = false;
    }
    fn play(&mut self, rate: u32, mut pcm: Box<dyn Read + Send>) -> Result<(), String> {
        let mut b = Vec::new();
        pcm.read_to_end(&mut b).map_err(|e| e.to_string())?;
        self.played.push((rate, b.len()));
        self.queued.push(AudioEvent::PlaybackStarted);
        self.queued.push(AudioEvent::PlaybackEnded);
        Ok(())
    }
    fn stop_playback(&mut self) {
        self.queued.push(AudioEvent::PlaybackEnded);
    }
    fn events(&mut self) -> Vec<AudioEvent> {
        std::mem::take(&mut self.queued)
    }
}

/// One reader over several `/api/voice/speak` answers: the daemon speaks at
/// most 1000 characters at a time, so a long answer is several requests,
/// fetched one after the other as the previous body is drained. Playback
/// sees one stream at one rate, with no gap between pieces.
/// `stop` ends it for good: once set, nothing more is read or fetched.
pub struct Chain {
    daemon: std::sync::Arc<dyn crate::client::Daemon>,
    rest: std::collections::VecDeque<String>,
    cur: Box<dyn Read + Send>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Chain {
    /// Speaks the first piece now (its errors are the caller's), the rest as read.
    pub fn start(daemon: std::sync::Arc<dyn crate::client::Daemon>, pieces: Vec<String>, stop: Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<(u32, Chain)> {
        let mut rest: std::collections::VecDeque<String> = pieces.into();
        let first = rest.pop_front().ok_or_else(|| anyhow::anyhow!("nothing to read"))?;
        let (rate, cur) = daemon.speak(&first)?;
        Ok((rate, Chain { daemon, rest, cur, stop }))
    }

    fn stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Read for Chain {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.stopped() {
                return Ok(0);
            }
            let n = self.cur.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            let Some(next) = self.rest.pop_front() else { return Ok(0) };
            if self.stopped() {
                return Ok(0);
            }
            match self.daemon.speak(&next) {
                Ok((_, r)) => self.cur = r,
                Err(e) => return Err(std::io::Error::other(format!("{e:#}"))),
            }
        }
    }
}
