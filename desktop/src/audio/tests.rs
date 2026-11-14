//! The whole of `Audio` against a fake device: real threads, a callback
//! thread per stream driven by the clock, as a sound card would.

use super::backend::{Backend, ErrorSink, InputSink, Opened, OutputSource};
use super::*;
use std::io::Cursor;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Instant;

const RATE: u32 = 48_000;
const CH: usize = 2;
/// Frames per callback: 5 ms at 48 kHz.
const FRAMES: usize = 240;
/// "Returns at once": far below the seconds a blocked reader or device
/// would take, with room for a loaded test machine.
const AT_ONCE: Duration = Duration::from_millis(200);

#[derive(Clone)]
struct Fake {
    /// The input signal, by frame number (same on both channels).
    signal: fn(u64) -> f32,
    /// The input delivers no callbacks at all.
    no_data: bool,
    /// Device time runs this many times faster than the clock.
    speed: u32,
    fail_open: Option<AudioError>,
    /// The input reports this error after this long.
    fail_input_after: Option<(Duration, AudioError)>,
    /// Everything the output played, mono (channel 0).
    played: Arc<Mutex<Vec<f32>>>,
    outputs_opened: Arc<AtomicU64>,
}

impl Default for Fake {
    fn default() -> Self {
        Fake {
            signal: |i| (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / RATE as f64).sin()) as f32,
            no_data: false,
            speed: 1,
            fail_open: None,
            fail_input_after: None,
            played: Arc::default(),
            outputs_opened: Arc::default(),
        }
    }
}

/// A fake stream: a thread that stops when this is dropped.
struct FakeStream {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn period(speed: u32) -> Duration {
    Duration::from_micros(5_000 / speed as u64)
}

impl Backend for Fake {
    fn open_input(&mut self, make: &mut dyn FnMut(u32) -> InputSink, errors: ErrorSink) -> Result<Opened, AudioError> {
        if let Some(e) = self.fail_open.clone() {
            return Err(e);
        }
        let mut sink = make(RATE);
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let f = self.clone();
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let mut frame = 0u64;
            let mut buf = vec![0.0f32; FRAMES * CH];
            let mut failed = false;
            while !s.load(Ordering::Acquire) {
                std::thread::sleep(period(f.speed));
                if let Some((after, e)) = &f.fail_input_after {
                    if !failed && start.elapsed() >= *after {
                        failed = true;
                        errors.report(e.clone());
                    }
                }
                if f.no_data || failed {
                    continue;
                }
                for (i, fr) in buf.chunks_exact_mut(CH).enumerate() {
                    let v = (f.signal)(frame + i as u64);
                    fr.fill(v);
                }
                frame += FRAMES as u64;
                sink.write(&buf, CH, |v| v);
            }
        });
        Ok(Opened { stream: Box::new(FakeStream { stop, thread: Some(thread) }), device: "fake in".into(), rate: RATE, channels: CH as u16, format: "f32".into() })
    }

    fn open_output(&mut self, make: &mut dyn FnMut(u32) -> OutputSource, _errors: ErrorSink) -> Result<Opened, AudioError> {
        if let Some(e) = self.fail_open.clone() {
            return Err(e);
        }
        self.outputs_opened.fetch_add(1, Ordering::Relaxed);
        let mut source = make(RATE);
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let played = self.played.clone();
        let speed = self.speed;
        let thread = std::thread::spawn(move || {
            let mut buf = vec![0.0f32; FRAMES * CH];
            while !s.load(Ordering::Acquire) {
                std::thread::sleep(period(speed));
                source.fill(&mut buf, CH, |v| v);
                assert!(buf.chunks_exact(CH).all(|f| f[0] == f[1]), "every channel gets the same sample");
                played.lock().unwrap().extend(buf.chunks_exact(CH).map(|f| f[0]));
            }
        });
        Ok(Opened { stream: Box::new(FakeStream { stop, thread: Some(thread) }), device: "fake out".into(), rate: RATE, channels: CH as u16, format: "f32".into() })
    }

    fn any_device(&mut self) -> bool {
        self.fail_open.is_none()
    }
}

fn audio(fake: &Fake, opts: Options) -> Audio {
    let f = fake.clone();
    Audio::with_backend(move || Box::new(f), opts, Arc::new(|| {}))
}

/// Events until `pred` matches one (returned with all before it), or panic.
fn wait_for(a: &mut Audio, seen: &mut Vec<AudioEvent>, within: Duration, pred: impl Fn(&AudioEvent) -> bool) {
    let t = Instant::now();
    loop {
        let evs = a.events();
        let hit = evs.iter().any(&pred);
        seen.extend(evs);
        if hit {
            return;
        }
        assert!(t.elapsed() < within, "timed out; events so far: {seen:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn names(evs: &[AudioEvent]) -> Vec<&'static str> {
    evs.iter()
        .filter_map(|e| match e {
            AudioEvent::CaptureLevel(_) | AudioEvent::PlaybackLevel(_) => None,
            AudioEvent::CaptureStarted => Some("CaptureStarted"),
            AudioEvent::CaptureEnded => Some("CaptureEnded"),
            AudioEvent::CaptureFailed(_) => Some("CaptureFailed"),
            AudioEvent::PlaybackStarted => Some("PlaybackStarted"),
            AudioEvent::PlaybackEnded => Some("PlaybackEnded"),
            AudioEvent::PlaybackFailed(_) => Some("PlaybackFailed"),
        })
        .collect()
}

fn s16_sine(freq: f64, rate: u32, secs: f64, amp: f64) -> Vec<u8> {
    let n = (secs * rate as f64) as usize;
    let x: Vec<f32> = (0..n).map(|i| (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()) as f32).collect();
    pcm::s16_le_bytes(&x)
}

#[test]
fn a_recording_of_48k_stereo_comes_back_as_16k_mono_f32() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    a.start_capture().unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::CaptureStarted));
    std::thread::sleep(Duration::from_millis(400));
    seen.extend(a.events());
    let t = Instant::now();
    let bytes = a.stop_capture().unwrap();
    assert!(t.elapsed() < AT_ONCE, "stop_capture took {:?}", t.elapsed());
    assert_eq!(bytes.len() % 4, 0);
    let x = pcm::f32_from_le_bytes(&bytes);
    // About 0.4 s at 16 kHz (the clock is loose under test load).
    assert!(x.len() > 2_000 && x.len() < 9_600, "{} samples", x.len());
    // Still a 1 kHz sine of amplitude 0.5: count rising zero crossings.
    let body = &x[200..x.len() - 200];
    let rising = body.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64;
    let freq = rising / (body.len() as f64 / 16_000.0);
    assert!((freq - 1000.0).abs() < 15.0, "frequency {freq}");
    let peak = body.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
    // Levels came while recording: a 0.5 sine is about -9 dBFS RMS.
    let levels: Vec<f32> = seen.iter().filter_map(|e| if let AudioEvent::CaptureLevel(l) = e { Some(*l) } else { None }).collect();
    assert!(levels.len() >= 4, "{levels:?}");
    // A window can be empty when the fake device thread is starved.
    let near = levels.iter().filter(|l| (**l - 0.85).abs() < 0.05).count();
    assert!(near * 2 > levels.len() && levels.iter().all(|l| *l < 0.9), "{levels:?}");
    // Stopped: nothing more, and a second stop is empty.
    assert!(a.stop_capture().unwrap().is_empty());
}

#[test]
fn a_recording_stops_by_itself_at_the_limit_with_exactly_that_much() {
    let fake = Fake { speed: 4, ..Fake::default() };
    let mut a = audio(&fake, Options { max_capture: Duration::from_millis(300), ..Options::default() });
    let mut seen = Vec::new();
    a.start_capture().unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(3), |e| matches!(e, AudioEvent::CaptureEnded));
    assert_eq!(names(&seen), ["CaptureStarted", "CaptureEnded"]);
    let bytes = a.stop_capture().unwrap();
    assert_eq!(bytes.len(), 300 * 16 * 4);
}

#[test]
fn the_default_limit_is_the_daemons() {
    assert_eq!(Options::default().max_capture, Duration::from_secs(120));
    // svc/voice.rs:35: MAX_SAMPLES = 16_000 * 120.
    assert_eq!(MAX_CAPTURE_SECS * CAPTURE_RATE, 16_000 * 120);
}

#[test]
fn digital_silence_and_no_data_are_reported_as_a_refused_microphone() {
    for (no_data, word) in [(false, "only silence"), (true, "no sound at all")] {
        let fake = Fake { signal: |_| 0.0, no_data, ..Fake::default() };
        let mut a = audio(&fake, Options { silence_check: Some(Duration::from_millis(200)), ..Options::default() });
        let mut seen = Vec::new();
        a.start_capture().unwrap();
        wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::CaptureFailed(_)));
        let msg = seen.iter().find_map(|e| if let AudioEvent::CaptureFailed(m) = e { Some(m.clone()) } else { None }).unwrap();
        assert!(msg.contains(word) && msg.contains("Privacy & Security"), "{msg}");
        assert!(!a.is_capturing());
        assert!(a.stop_capture().unwrap().is_empty());
    }
}

#[test]
fn no_device_is_an_error_value_for_both_directions() {
    let none = AudioError { kind: ErrorKind::NoDevice, message: "No microphone found.".into() };
    let fake = Fake { fail_open: Some(none), ..Fake::default() };
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    a.start_capture().unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::CaptureFailed(_)));
    assert_eq!(seen, [AudioEvent::CaptureFailed("No microphone found.".into())]);
    a.play(24_000, Box::new(Cursor::new(s16_sine(300.0, 24_000, 0.1, 0.5)))).unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::PlaybackFailed(_)));
    assert!(!a.is_playing());
    // The probe saw no device.
    let t = Instant::now();
    while a.available() && t.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!a.available());
    assert!(a.play(0, Box::new(Cursor::new(Vec::new()))).is_err());
}

#[test]
fn a_microphone_that_goes_away_ends_the_recording_and_keeps_it() {
    let gone = AudioError { kind: ErrorKind::DeviceGone, message: "The microphone went away.".into() };
    let fake = Fake { fail_input_after: Some((Duration::from_millis(250), gone)), ..Fake::default() };
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    a.start_capture().unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::CaptureEnded));
    let x = pcm::f32_from_le_bytes(&a.stop_capture().unwrap());
    assert!(x.len() > 500, "{} samples kept", x.len());
}

#[test]
fn speech_at_24k_plays_at_the_device_rate_with_started_levels_and_ended() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    a.play(24_000, Box::new(Cursor::new(s16_sine(300.0, 24_000, 0.5, 0.5)))).unwrap();
    assert!(a.is_playing());
    wait_for(&mut a, &mut seen, Duration::from_secs(3), |e| matches!(e, AudioEvent::PlaybackEnded));
    assert_eq!(names(&seen), ["PlaybackStarted", "PlaybackEnded"]);
    assert!(seen.iter().any(|e| matches!(e, AudioEvent::PlaybackLevel(l) if *l > 0.7)), "{seen:?}");
    assert!(!a.is_playing());
    let played = fake.played.lock().unwrap().clone();
    // 0.5 s at 48 kHz, give or take the kernel's edges.
    let first = played.iter().position(|v| v.abs() > 1e-3).unwrap();
    let last = played.iter().rposition(|v| v.abs() > 1e-3).unwrap();
    let span = last - first;
    assert!((span as i64 - 24_000).abs() < 200, "{span} frames");
    let peak = played.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
}

/// A reader that hands out `chunks` in turn, sleeping before some, or
/// failing at the end.
struct Script {
    steps: Vec<(Duration, Option<Vec<u8>>)>,
    at: usize,
    pos: usize,
    fail_at_end: bool,
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let Some((sleep, chunk)) = self.steps.get_mut(self.at) else {
                return if self.fail_at_end { Err(std::io::Error::other("connection reset by the daemon")) } else { Ok(0) };
            };
            if self.pos == 0 && !sleep.is_zero() {
                std::thread::sleep(*sleep);
                *sleep = Duration::ZERO;
            }
            let Some(c) = chunk else {
                self.at += 1;
                continue;
            };
            if self.pos >= c.len() {
                self.at += 1;
                self.pos = 0;
                continue;
            }
            // Odd sizes on purpose: samples are cut between reads.
            let n = buf.len().min(c.len() - self.pos).min(1001);
            buf[..n].copy_from_slice(&c[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
    }
}

#[test]
fn a_reader_that_blocks_mid_clip_is_a_soft_gap_not_an_end() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    let part = s16_sine(300.0, 24_000, 0.3, 0.5);
    let script = Script { steps: vec![(Duration::ZERO, Some(part.clone())), (Duration::from_millis(600), Some(part))], at: 0, pos: 0, fail_at_end: false };
    a.play(24_000, Box::new(script)).unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(4), |e| matches!(e, AudioEvent::PlaybackEnded));
    assert_eq!(names(&seen), ["PlaybackStarted", "PlaybackEnded"]);
    let played = fake.played.lock().unwrap().clone();
    let first = played.iter().position(|v| v.abs() > 1e-3).unwrap();
    let last = played.iter().rposition(|v| v.abs() > 1e-3).unwrap();
    let body = &played[first..=last];
    // A gap of silence in the middle...
    let quiet = body.windows(480).any(|w| w.iter().all(|v| v.abs() < 1e-4));
    assert!(quiet, "no gap heard");
    // ...entered and left without a click: no step bigger than the sine's
    // own (2 pi 300 / 48000 * 0.5 = 0.02) plus the fade's.
    let (at, step) = body.windows(2).map(|w| (w[1] - w[0]).abs()).enumerate().fold((0, 0.0f32), |m, (i, d)| if d > m.1 { (i, d) } else { m });
    assert!(step < 0.04, "largest step {step} at {at} of {}: {:?}", body.len(), &body[at.saturating_sub(6)..(at + 6).min(body.len())]);
    // Both halves were played: 0.6 s of sound at 48 kHz.
    let loud = body.iter().filter(|v| v.abs() > 0.05).count();
    assert!(loud > 25_000, "{loud} loud frames");
}

#[test]
fn a_reader_that_fails_mid_clip_is_playback_failed_with_its_message() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    let script = Script { steps: vec![(Duration::ZERO, Some(s16_sine(300.0, 24_000, 0.2, 0.5)))], at: 0, pos: 0, fail_at_end: true };
    a.play(24_000, Box::new(script)).unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(3), |e| matches!(e, AudioEvent::PlaybackFailed(_)));
    let msg = seen.iter().find_map(|e| if let AudioEvent::PlaybackFailed(m) = e { Some(m.clone()) } else { None }).unwrap();
    assert!(msg.contains("connection reset by the daemon"), "{msg}");
    assert!(!names(&seen).contains(&"PlaybackEnded"));
    assert!(!a.is_playing());
}

/// A reader that never returns until the test lets it.
struct Stuck {
    first: Option<Vec<u8>>,
    release: mpsc::Receiver<()>,
    reads_after: Arc<AtomicU64>,
}

impl Read for Stuck {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(f) = self.first.take() {
            buf[..f.len()].copy_from_slice(&f);
            return Ok(f.len());
        }
        let _ = self.release.recv();
        self.reads_after.fetch_add(1, Ordering::Relaxed);
        buf[..2].copy_from_slice(&[0x10, 0x10]);
        Ok(2)
    }
}

#[test]
fn stop_and_replace_return_at_once_while_the_reader_is_blocked() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    let (release, rx) = mpsc::channel();
    let reads_after = Arc::new(AtomicU64::new(0));
    let stuck = Stuck { first: Some(s16_sine(300.0, 24_000, 0.15, 0.5)), release: rx, reads_after: reads_after.clone() };
    a.play(24_000, Box::new(stuck)).unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::PlaybackStarted));
    // Its 0.15 s are over; the reader is blocked: no end yet.
    std::thread::sleep(Duration::from_millis(400));
    seen.extend(a.events());
    assert_eq!(names(&seen), ["PlaybackStarted"]);
    let t = Instant::now();
    a.stop_playback();
    assert!(t.elapsed() < AT_ONCE, "stop took {:?}", t.elapsed());
    seen.extend(a.events());
    assert_eq!(names(&seen), ["PlaybackStarted", "PlaybackEnded"]);
    // Let the reader go: it notices it was stopped and its sound is dropped.
    release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(reads_after.load(Ordering::Relaxed), 1);
    assert!(names(&a.events()).is_empty());
}

#[test]
fn a_replacing_play_gives_no_end_for_the_old_one_and_stop_gives_exactly_one() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    a.play(24_000, Box::new(Cursor::new(s16_sine(300.0, 24_000, 3.0, 0.5)))).unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::PlaybackStarted));
    let second = s16_sine(600.0, 24_000, 3.0, 0.25);
    let t = Instant::now();
    a.play(24_000, Box::new(Cursor::new(second))).unwrap();
    assert!(t.elapsed() < AT_ONCE);
    let before = seen.len();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::PlaybackStarted));
    assert!(seen.len() > before);
    std::thread::sleep(Duration::from_millis(100));
    seen.extend(a.events());
    assert_eq!(names(&seen), ["PlaybackStarted", "PlaybackStarted"]);
    // The first clip is gone from the device: the new one is quieter.
    let n = fake.played.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(150));
    let recent = fake.played.lock().unwrap()[n..].to_vec();
    let peak = recent.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.25).abs() < 0.02, "peak {peak}");
    a.stop_playback();
    a.stop_playback();
    std::thread::sleep(Duration::from_millis(100));
    seen.extend(a.events());
    assert_eq!(names(&seen), ["PlaybackStarted", "PlaybackStarted", "PlaybackEnded"]);
    // Silent soon after the stop (the ring was flushed, not played out).
    let n = fake.played.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(100));
    let after = fake.played.lock().unwrap()[n..].to_vec();
    assert!(after.iter().all(|v| v.abs() < 1e-4));
    // One output stream served both clips.
    assert_eq!(fake.outputs_opened.load(Ordering::Relaxed), 1);
}

#[test]
fn talking_stops_the_voice() {
    let fake = Fake::default();
    let mut a = audio(&fake, Options::default());
    let mut seen = Vec::new();
    a.play(24_000, Box::new(Cursor::new(s16_sine(300.0, 24_000, 3.0, 0.5)))).unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::PlaybackStarted));
    a.start_capture().unwrap();
    wait_for(&mut a, &mut seen, Duration::from_secs(2), |e| matches!(e, AudioEvent::CaptureStarted));
    assert_eq!(names(&seen), ["PlaybackStarted", "PlaybackEnded", "CaptureStarted"]);
    assert!(!a.is_playing() && a.is_capturing());
    a.cancel_capture();
    assert!(!a.is_capturing());
    assert!(a.stop_capture().unwrap().is_empty());
}

#[test]
fn the_thing_can_be_a_trait_object_and_dropping_it_ends_the_thread() {
    let fake = Fake::default();
    let f = fake.clone();
    let mut b: Box<dyn AudioIo> = Box::new(Audio::with_backend(move || Box::new(f), Options::default(), Arc::new(|| {})));
    assert!(b.available());
    b.play(24_000, Box::new(Cursor::new(s16_sine(300.0, 24_000, 2.0, 0.5)))).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    drop(b);
    // The output stream is closed with the thread: the device stops being fed.
    let t = Instant::now();
    loop {
        let n = fake.played.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(50));
        if fake.played.lock().unwrap().len() == n {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(2), "the output still runs after the drop");
    }
}

#[test]
fn the_waker_is_called_when_events_are_queued() {
    let fake = Fake::default();
    let f = fake.clone();
    let rung = Arc::new(AtomicU64::new(0));
    let r = rung.clone();
    let mut a = Audio::with_backend(move || Box::new(f), Options::default(), Arc::new(move || {
        r.fetch_add(1, Ordering::Relaxed);
    }));
    a.start_capture().unwrap();
    let t = Instant::now();
    while rung.load(Ordering::Relaxed) < 2 {
        assert!(t.elapsed() < Duration::from_secs(2), "never woken");
        std::thread::sleep(Duration::from_millis(5));
    }
    a.cancel_capture();
}
