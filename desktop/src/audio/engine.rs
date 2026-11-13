//! The control thread: owns the streams, drains and fills the rings,
//! resamples, measures, and turns all of it into events.
//!
//! It sleeps in `recv()` when nothing is open, and ticks every
//! [`TICK`] while recording or playing. Commands come from the UI side
//! ([`super::Audio`]) and from the reader threads of `play`.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use super::backend::{Backend, ErrorSink, InputSink, OutputShared, OutputSource, StreamFailure};
use super::pcm::{f32_le_bytes, meter, meter_peak};
use super::resample::Resampler;
use super::ring::{ring, Consumer, Producer};
use super::{AudioError, AudioEvent, ErrorKind, Options, CAPTURE_RATE};

pub const TICK: Duration = Duration::from_millis(10);
/// How often a level is reported (about 20 per second).
const LEVEL_EVERY: Duration = Duration::from_millis(50);
/// Ring sizes, in mono samples: about 2.7 s at 48 kHz; the control thread
/// drains or refills every 10 ms.
const RING: usize = 1 << 17;
/// How much sound is handed to the output ahead of the device, in seconds:
/// enough to ride out a late tick, small enough that the ring never fills.
const AHEAD_SECS: f64 = 0.25;

/// Which capture or playback an event belongs to: the UI side drops events
/// of one it has already replaced or stopped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tag {
    Capture(u64),
    Play(u64),
}

pub type EventTx = mpsc::Sender<(Tag, AudioEvent)>;

pub enum Cmd {
    StartCapture { gen: u64 },
    /// Reply with the recording (`Err` only if the thread failed).
    StopCapture { gen: u64, reply: mpsc::SyncSender<Result<Vec<u8>, String>> },
    CancelCapture { gen: u64 },
    /// A new playback at `rate`; whatever played before is dropped.
    Play { gen: u64, rate: u32 },
    /// Decoded sound of playback `gen`, from its reader thread.
    Pcm { gen: u64, samples: Vec<f32> },
    /// The reader reached its end (`Ok`) or failed.
    PcmEnd { gen: u64, result: Result<(), String> },
    StopPlayback { gen: u64 },
    /// Ends the thread (the `Audio` was dropped).
    Quit,
}

/// `available`: not known yet, yes, no.
pub const AVAIL_UNKNOWN: u8 = 0;
pub const AVAIL_YES: u8 = 1;
pub const AVAIL_NO: u8 = 2;

struct Capture {
    gen: u64,
    serial: u64,
    _stream: Box<dyn std::any::Any>,
    ring: Consumer,
    dropped: Arc<AtomicU64>,
    resampler: Resampler,
    /// The recording so far, at [`CAPTURE_RATE`].
    out: Vec<f32>,
    max_out: usize,
    started: Instant,
    device_samples: u64,
    any_sound: bool,
    /// Sum of squares since the last level, and how many.
    level_acc: f64,
    level_n: u64,
    last_level: Instant,
}

/// A recording that is over (by itself, or failed) and waits for `stop_capture`.
struct Finished {
    gen: u64,
    samples: Vec<f32>,
}

struct Output {
    serial: u64,
    _stream: Box<dyn std::any::Any>,
    ring: Producer,
    shared: Arc<OutputShared>,
    rate: u32,
    /// Samples written into the ring since it was opened.
    pushed: u64,
    /// Since when nothing has played (closed after `Options::idle_close`).
    idle_since: Option<Instant>,
}

struct Playing {
    gen: u64,
    resampler: Resampler,
    /// Decoded sound at the clip's rate, not yet resampled (from `taken` on).
    /// Resampled only as it is about to be pushed, so a tick's work is
    /// bounded by real time however fast the reader is.
    input: Vec<f32>,
    taken: usize,
    /// Resampled sound not yet in the ring, and how much of it was pushed.
    pending: Vec<f32>,
    sent: usize,
    /// The reader ended; `finished` once the resampler's tail is out too.
    eof: bool,
    finished: bool,
    /// `pushed` when this playback began: it has started once the device
    /// took more than this.
    mark: u64,
    /// Since when a flush has gone unconfirmed.
    waiting_since: Option<Instant>,
    started: bool,
    last_level: Instant,
}

pub struct Engine {
    backend: Box<dyn Backend>,
    opts: Options,
    events: EventTx,
    wake: Arc<dyn Fn() + Send + Sync>,
    cmd_tx: mpsc::Sender<Cmd>,
    fail_tx: mpsc::Sender<StreamFailure>,
    fail_rx: mpsc::Receiver<StreamFailure>,
    serial: u64,
    capture: Option<Capture>,
    finished: Option<Finished>,
    output: Option<Output>,
    playing: Option<Playing>,
    scratch: Vec<f32>,
}

impl Engine {
    pub fn new(backend: Box<dyn Backend>, opts: Options, events: EventTx, wake: Arc<dyn Fn() + Send + Sync>, cmd_tx: mpsc::Sender<Cmd>) -> Engine {
        let (fail_tx, fail_rx) = mpsc::channel();
        Engine {
            backend,
            opts,
            events,
            wake,
            cmd_tx,
            fail_tx,
            fail_rx,
            serial: 0,
            capture: None,
            finished: None,
            output: None,
            playing: None,
            scratch: vec![0.0; 4096],
        }
    }

    fn emit(&self, tag: Tag, ev: AudioEvent) {
        if self.events.send((tag, ev)).is_ok() {
            (self.wake)();
        }
    }

    fn errors(&mut self) -> ErrorSink {
        self.serial += 1;
        // Stream errors arrive on their own channel; one empty command wakes
        // the loop if it is asleep in `recv`.
        let fail_tx = self.fail_tx.clone();
        let cmd = self.cmd_tx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = cmd.send(Cmd::Pcm { gen: u64::MAX, samples: Vec::new() });
        });
        ErrorSink { serial: self.serial, tx: fail_tx, wake }
    }

    /// Runs until `Quit` or until every sender is gone.
    pub fn run(mut self, rx: mpsc::Receiver<Cmd>, available: Arc<AtomicU8>) {
        let any = self.backend.any_device();
        available.store(if any { AVAIL_YES } else { AVAIL_NO }, Ordering::Release);
        loop {
            let busy = self.capture.is_some() || self.output.is_some();
            let cmd = if busy {
                match rx.recv_timeout(TICK) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            };
            if let Some(cmd) = cmd {
                if matches!(cmd, Cmd::Quit) {
                    return;
                }
                self.command(cmd);
                // Take every command already queued before ticking.
                while let Ok(c) = rx.try_recv() {
                    if matches!(c, Cmd::Quit) {
                        return;
                    }
                    self.command(c);
                }
            }
            while let Ok(f) = self.fail_rx.try_recv() {
                self.stream_failed(f);
            }
            self.tick_capture();
            self.tick_output();
        }
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::StartCapture { gen } => self.start_capture(gen),
            Cmd::StopCapture { gen, reply } => {
                let samples = self.end_capture(gen);
                // The reply goes first: closing the stream may take a moment.
                let _ = reply.send(Ok(f32_le_bytes(&samples)));
                log::info!("audio: recording handed over, {:.2} s, {} bytes", samples.len() as f32 / CAPTURE_RATE as f32, samples.len() * 4);
            }
            Cmd::CancelCapture { gen } => {
                let samples = self.end_capture(gen);
                log::info!("audio: recording dropped, {:.2} s", samples.len() as f32 / CAPTURE_RATE as f32);
            }
            Cmd::Play { gen, rate } => self.start_play(gen, rate),
            Cmd::Pcm { gen, samples } => {
                if let Some(p) = self.playing.as_mut().filter(|p| p.gen == gen) {
                    p.input.extend_from_slice(&samples);
                }
            }
            Cmd::PcmEnd { gen, result } => {
                let Some(p) = self.playing.as_mut().filter(|p| p.gen == gen) else { return };
                match result {
                    Ok(()) => p.eof = true,
                    Err(e) => {
                        // Failed, not ended: the one event says why.
                        self.stop_play(false);
                        self.emit(Tag::Play(gen), AudioEvent::PlaybackFailed(format!("Could not read the speech: {e}")));
                    }
                }
            }
            Cmd::StopPlayback { gen } => {
                if self.playing.as_ref().is_some_and(|p| p.gen <= gen) {
                    self.stop_play(true);
                }
            }
            Cmd::Quit => {}
        }
    }

    // ------------------------------------------------------------ capture

    fn start_capture(&mut self, gen: u64) {
        // A capture still open (start twice) ends without being handed over.
        self.capture = None;
        self.finished = None;
        let errors = self.errors();
        let serial = errors.serial;
        let dropped = Arc::new(AtomicU64::new(0));
        let mut consumer = None;
        let d = dropped.clone();
        let mut make = |_rate: u32| {
            let (p, c) = ring(RING);
            consumer = Some(c);
            InputSink { ring: p, dropped: d.clone() }
        };
        match self.backend.open_input(&mut make, errors) {
            Ok(opened) => {
                log::info!("audio: recording from \"{}\" at {} Hz, {} ch, {}", opened.device, opened.rate, opened.channels, opened.format);
                let now = Instant::now();
                self.capture = Some(Capture {
                    gen,
                    serial,
                    _stream: opened.stream,
                    ring: consumer.expect("a sink was made"),
                    dropped,
                    resampler: Resampler::new(opened.rate, CAPTURE_RATE),
                    out: Vec::with_capacity(CAPTURE_RATE as usize * 10),
                    max_out: (self.opts.max_capture.as_secs_f64() * CAPTURE_RATE as f64) as usize,
                    started: now,
                    device_samples: 0,
                    any_sound: false,
                    level_acc: 0.0,
                    level_n: 0,
                    last_level: now,
                });
                self.emit(Tag::Capture(gen), AudioEvent::CaptureStarted);
            }
            Err(e) => {
                log::warn!("audio: no recording: {e}");
                self.emit(Tag::Capture(gen), AudioEvent::CaptureFailed(e.message));
            }
        }
    }

    /// Drain the ring into the recording.
    fn drain_capture(&mut self) {
        let Some(c) = self.capture.as_mut() else { return };
        // At most one ring's worth per call: a device that delivers faster
        // than real time (ALSA's null device does) must not keep this
        // thread from its commands. The excess is dropped at the ring.
        let mut budget = RING;
        while budget > 0 {
            let n = c.ring.pop_slice(&mut self.scratch[..budget.min(4096)]);
            if n == 0 {
                break;
            }
            budget -= n;
            let block = &self.scratch[..n];
            c.device_samples += n as u64;
            for &v in block {
                if v != 0.0 {
                    c.any_sound = true;
                }
                c.level_acc += v as f64 * v as f64;
            }
            c.level_n += n as u64;
            c.resampler.process(block, &mut c.out);
        }
    }

    fn tick_capture(&mut self) {
        self.drain_capture();
        let opts_silence = self.opts.silence_check;
        let Some(c) = self.capture.as_mut() else { return };
        let gen = c.gen;
        // The level, about 20 times a second.
        if c.last_level.elapsed() >= LEVEL_EVERY {
            let rms = if c.level_n > 0 { (c.level_acc / c.level_n as f64).sqrt() as f32 } else { 0.0 };
            c.level_acc = 0.0;
            c.level_n = 0;
            c.last_level = Instant::now();
            let level = meter(rms);
            self.emit(Tag::Capture(gen), AudioEvent::CaptureLevel(level));
        }
        let Some(c) = self.capture.as_mut() else { return };
        // Nothing, or only exact zeros: the system gives no access (macOS
        // without the microphone permission records digital silence).
        if let Some(after) = opts_silence {
            if !c.any_sound && c.started.elapsed() >= after {
                let msg = if c.device_samples == 0 {
                    "The microphone sends no sound at all. On macOS, allow the microphone for the app sbxw was started from (System Settings, Privacy & Security, Microphone), or check that the input device is connected."
                } else {
                    "The microphone sends only silence. On macOS this is what a refused microphone permission looks like: allow it for the app sbxw was started from (System Settings, Privacy & Security, Microphone). Otherwise check the input is not muted."
                };
                log::warn!("audio: recording failed after {:.1} s: {} device samples, all zero", c.started.elapsed().as_secs_f32(), c.device_samples);
                self.capture = None;
                self.emit(Tag::Capture(gen), AudioEvent::CaptureFailed(msg.into()));
                return;
            }
        }
        let Some(c) = self.capture.as_mut() else { return };
        if c.out.len() >= c.max_out {
            c.out.truncate(c.max_out);
            let samples = std::mem::take(&mut c.out);
            log::info!("audio: recording stopped by itself at the {:.0} s limit", self.opts.max_capture.as_secs_f32());
            self.capture = None;
            self.finished = Some(Finished { gen, samples });
            self.emit(Tag::Capture(gen), AudioEvent::CaptureEnded);
        }
    }

    /// The recording of `gen`, closed; empty if there is none.
    fn end_capture(&mut self, gen: u64) -> Vec<f32> {
        if self.capture.as_ref().is_some_and(|c| c.gen == gen) {
            self.drain_capture();
            let mut c = self.capture.take().expect("checked");
            let lost = c.dropped.load(Ordering::Relaxed);
            if lost > 0 {
                log::warn!("audio: {lost} input samples were lost (the control thread was late)");
            }
            if !c.any_sound && self.opts.silence_check.is_some() {
                // Silence only (shorter than the check): nothing to transcribe.
                return Vec::new();
            }
            c.resampler.finish(&mut c.out);
            c.out.truncate(c.max_out);
            return c.out;
        }
        match self.finished.take() {
            Some(f) if f.gen == gen => f.samples,
            other => {
                self.finished = other;
                Vec::new()
            }
        }
    }

    // ------------------------------------------------------------ playback

    fn ensure_output(&mut self) -> Result<(), AudioError> {
        if self.output.is_some() {
            return Ok(());
        }
        let errors = self.errors();
        let serial = errors.serial;
        let shared = Arc::new(OutputShared::default());
        let mut producer = None;
        let s = shared.clone();
        let mut make = |_rate: u32| {
            let (p, c) = ring(RING);
            producer = Some(p);
            OutputSource::new(c, s.clone())
        };
        let opened = self.backend.open_output(&mut make, errors)?;
        log::info!("audio: playing on \"{}\" at {} Hz, {} ch, {}", opened.device, opened.rate, opened.channels, opened.format);
        self.output = Some(Output {
            serial,
            _stream: opened.stream,
            ring: producer.expect("a source was made"),
            shared,
            rate: opened.rate,
            pushed: 0,
            idle_since: None,
        });
        Ok(())
    }

    fn start_play(&mut self, gen: u64, rate: u32) {
        self.stop_play(false);
        if let Err(e) = self.ensure_output() {
            log::warn!("audio: no playback: {e}");
            self.emit(Tag::Play(gen), AudioEvent::PlaybackFailed(e.message));
            return;
        }
        let out = self.output.as_mut().expect("opened");
        out.idle_since = None;
        self.playing = Some(Playing {
            gen,
            resampler: Resampler::new(rate, out.rate),
            input: Vec::new(),
            taken: 0,
            pending: Vec::new(),
            sent: 0,
            eof: false,
            finished: false,
            mark: out.pushed,
            waiting_since: None,
            started: false,
            last_level: Instant::now(),
        });
    }

    /// Drop the current playback; its sound already in the ring is flushed.
    /// `ended`: say so with `PlaybackEnded` (a stop, not a replacement).
    fn stop_play(&mut self, ended: bool) {
        let Some(p) = self.playing.take() else { return };
        if let Some(out) = self.output.as_mut() {
            out.shared.flush_req.fetch_add(1, Ordering::AcqRel);
            out.idle_since = Some(Instant::now());
        }
        log::info!("audio: playback stopped");
        if ended {
            self.emit(Tag::Play(p.gen), AudioEvent::PlaybackEnded);
        }
    }

    fn tick_output(&mut self) {
        let Some(out) = self.output.as_mut() else { return };
        let consumed = out.shared.consumed.load(Ordering::Acquire);
        let flushed = out.shared.flush_ack.load(Ordering::Acquire) == out.shared.flush_req.load(Ordering::Acquire);
        let Some(p) = self.playing.as_mut() else {
            if out.idle_since.is_none() {
                out.idle_since = Some(Instant::now());
            }
            if out.idle_since.is_some_and(|t| t.elapsed() >= self.opts.idle_close) {
                log::info!("audio: output closed after {:.0} s idle", self.opts.idle_close.as_secs_f32());
                self.output = None;
            }
            return;
        };
        // A flush asked for by a stop or a replacement: no new sound before
        // the callback confirms it (it would be flushed with the old one).
        // Once confirmed, everything pushed so far is consumed, so `mark`
        // (set to `pushed` at the start) is where this playback begins.
        if !flushed {
            // A callback that stopped running never confirms: reopen.
            if p.waiting_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_millis(500) {
                log::warn!("audio: the output stopped answering; reopening it");
                self.output = None;
                if let Err(e) = self.ensure_output() {
                    let gen = self.playing.take().map(|p| p.gen).unwrap_or(0);
                    self.emit(Tag::Play(gen), AudioEvent::PlaybackFailed(e.message));
                } else if let (Some(p), Some(out)) = (self.playing.as_mut(), self.output.as_ref()) {
                    p.mark = out.pushed;
                    p.waiting_since = None;
                    p.sent = 0;
                    p.resampler = Resampler::new(p.resampler.in_rate(), out.rate);
                    // What was resampled for the old device and not played
                    // is lost: rare (a device that stalls), and logged.
                    p.pending.clear();
                    p.finished = false;
                }
            }
            return;
        }
        p.waiting_since = None;
        let ahead = out.pushed.saturating_sub(consumed);
        let limit = (AHEAD_SECS * out.rate as f64) as u64;
        // Resample just enough to fill the room.
        let room = limit.saturating_sub(ahead) as usize;
        while p.pending.len() - p.sent < room && p.taken < p.input.len() {
            let end = (p.taken + 1024).min(p.input.len());
            p.resampler.process(&p.input[p.taken..end], &mut p.pending);
            p.taken = end;
        }
        if p.taken > 1 << 16 {
            p.input.drain(..p.taken);
            p.taken = 0;
        }
        if p.eof && !p.finished && p.taken >= p.input.len() {
            p.resampler.finish(&mut p.pending);
            p.finished = true;
        }
        if ahead < limit && p.sent < p.pending.len() {
            let room = (limit - ahead) as usize;
            let end = (p.sent + room).min(p.pending.len());
            let n = out.ring.push_slice(&p.pending[p.sent..end]);
            p.sent += n;
            out.pushed += n as u64;
            // Keep the buffer small on long readings.
            if p.sent > 1 << 16 {
                p.pending.drain(..p.sent);
                p.sent = 0;
            }
        }
        let gen = p.gen;
        if !p.started && consumed > p.mark {
            p.started = true;
            self.emit(Tag::Play(gen), AudioEvent::PlaybackStarted);
        }
        let Some(out) = self.output.as_mut() else { return };
        let Some(p) = self.playing.as_mut() else { return };
        if p.started && p.last_level.elapsed() >= LEVEL_EVERY {
            p.last_level = Instant::now();
            let peak = f32::from_bits(out.shared.peak.swap(0, Ordering::Relaxed));
            self.emit(Tag::Play(gen), AudioEvent::PlaybackLevel(meter_peak(peak)));
        }
        let Some(out) = self.output.as_mut() else { return };
        let Some(p) = self.playing.as_ref() else { return };
        if p.finished && p.sent >= p.pending.len() && consumed >= out.pushed {
            let played = (out.pushed - p.mark.min(out.pushed)) as f32 / out.rate as f32;
            log::info!("audio: playback finished, {played:.2} s");
            out.idle_since = Some(Instant::now());
            self.playing = None;
            self.emit(Tag::Play(gen), AudioEvent::PlaybackEnded);
        }
    }

    fn stream_failed(&mut self, f: StreamFailure) {
        if f.error.kind == ErrorKind::Glitch {
            log::debug!("audio: {}", f.error);
            return;
        }
        if self.capture.as_ref().is_some_and(|c| c.serial == f.serial) {
            let gen = self.capture.as_ref().map(|c| c.gen).expect("checked");
            log::warn!("audio: recording interrupted: {}", f.error);
            if f.error.kind == ErrorKind::DeviceGone {
                // What was recorded is kept for `stop_capture`.
                self.drain_capture();
                let mut c = self.capture.take().expect("checked");
                c.resampler.finish(&mut c.out);
                c.out.truncate(c.max_out);
                let samples = if c.any_sound { c.out } else { Vec::new() };
                self.finished = Some(Finished { gen, samples });
                self.emit(Tag::Capture(gen), AudioEvent::CaptureEnded);
            } else {
                self.capture = None;
                self.emit(Tag::Capture(gen), AudioEvent::CaptureFailed(f.error.message));
            }
        } else if self.output.as_ref().is_some_and(|o| o.serial == f.serial) {
            log::warn!("audio: playback interrupted: {}", f.error);
            self.output = None;
            if let Some(p) = self.playing.take() {
                self.emit(Tag::Play(p.gen), AudioEvent::PlaybackFailed(f.error.message));
            }
        }
    }
}
