//! Where sound comes from and goes to: the [`Backend`] trait, its cpal
//! implementation, and the two ends the device callbacks hold.
//!
//! The callbacks only touch [`InputSink`] and [`OutputSource`]: fixed
//! buffers on the stack, the lock-free ring, atomics. No allocation, no lock,
//! no channel, no log. Everything else (opening, errors, resampling) happens
//! on the control thread.

use std::any::Any;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use super::ring::{Consumer, Producer};
use super::{AudioError, ErrorKind};

/// Frames converted per step inside a callback (a stack buffer this big).
const BLOCK: usize = 256;

/// An open stream: dropping `stream` closes it.
pub struct Opened {
    pub stream: Box<dyn Any>,
    pub device: String,
    pub rate: u32,
    pub channels: u16,
    /// The device's sample format, as cpal names it (`f32`, `i16`...).
    pub format: String,
}

/// A stream failed after it was opened; `serial` says which one.
pub struct StreamFailure {
    pub serial: u64,
    pub error: AudioError,
}

/// Given to the backend with each stream, called from the device's error
/// callback (not real-time: allowed to allocate).
#[derive(Clone)]
pub struct ErrorSink {
    pub serial: u64,
    pub tx: mpsc::Sender<StreamFailure>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl ErrorSink {
    pub fn report(&self, error: AudioError) {
        let _ = self.tx.send(StreamFailure { serial: self.serial, error });
        (self.wake)();
    }
}

/// Opens streams. Lives on the control thread (cpal streams are not `Send`
/// on every platform), so it is built there by a `Send` factory.
pub trait Backend {
    /// The default input, recording into a sink made by `make` for the
    /// device's rate (called again for each configuration tried).
    fn open_input(&mut self, make: &mut dyn FnMut(u32) -> InputSink, errors: ErrorSink) -> Result<Opened, AudioError>;
    /// The default output, playing from a source made by `make`.
    fn open_output(&mut self, make: &mut dyn FnMut(u32) -> OutputSource, errors: ErrorSink) -> Result<Opened, AudioError>;
    /// Whether any input or output device exists (for `available`).
    fn any_device(&mut self) -> bool;
}

/// The recording end, held by the input callback.
pub struct InputSink {
    pub ring: Producer,
    /// Samples lost because the ring was full (the control thread stalled).
    pub dropped: Arc<AtomicU64>,
}

impl InputSink {
    /// Interleaved device samples in, mono `f32` into the ring.
    pub fn write<T: Copy>(&mut self, data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) {
        let channels = channels.max(1);
        let mut mono = [0.0f32; BLOCK];
        let scale = 1.0 / channels as f32;
        for block in data.chunks(BLOCK * channels) {
            let mut n = 0;
            for frame in block.chunks_exact(channels) {
                let mut sum = 0.0;
                for &s in frame {
                    sum += to_f32(s);
                }
                mono[n] = sum * scale;
                n += 1;
            }
            let written = self.ring.push_slice(&mono[..n]);
            if written < n {
                self.dropped.fetch_add((n - written) as u64, Ordering::Relaxed);
            }
        }
    }
}

/// State the output callback shares with the control thread.
#[derive(Default)]
pub struct OutputShared {
    /// Samples taken out of the ring, played or flushed.
    pub consumed: AtomicU64,
    /// Bumped by the control thread to ask for a flush; the callback copies
    /// it to `flush_ack` once the ring is empty.
    pub flush_req: AtomicU64,
    pub flush_ack: AtomicU64,
    /// Highest absolute sample since the control thread last read it (f32 bits).
    pub peak: AtomicU32,
}

/// The playing end, held by the output callback.
pub struct OutputSource {
    pub ring: Consumer,
    pub shared: Arc<OutputShared>,
    /// Last sample written.
    last: f32,
    /// What was playing when the sound ran out (an underrun or a flush),
    /// decaying to zero instead of jumping there (a jump is a click). It
    /// keeps decaying under the new sound, so a short underrun crossfades.
    hold: f32,
    /// Gain of the ring's sound, ramping back to 1 after an underrun.
    gain: f32,
}

/// Per sample decay of the held value during an underrun (about 3 ms to -60 dB at 48 kHz).
const DECAY: f32 = 0.95;
/// Per sample gain step after an underrun (about 2.7 ms to full gain at 48 kHz).
const RAMP: f32 = 1.0 / 128.0;

impl OutputSource {
    pub fn new(ring: Consumer, shared: Arc<OutputShared>) -> OutputSource {
        // Gain starts at zero: the first sound ramps in.
        OutputSource { ring, shared, last: 0.0, hold: 0.0, gain: 0.0 }
    }

    /// Fill interleaved device samples from the ring; silence (softly) when
    /// it is empty.
    pub fn fill<T: Copy>(&mut self, data: &mut [T], channels: usize, from_f32: impl Fn(f32) -> T) {
        let channels = channels.max(1);
        let req = self.shared.flush_req.load(Ordering::Acquire);
        if req != self.shared.flush_ack.load(Ordering::Relaxed) {
            let mut scratch = [0.0f32; BLOCK];
            let mut flushed = 0u64;
            loop {
                let n = self.ring.pop_slice(&mut scratch);
                if n == 0 {
                    break;
                }
                flushed += n as u64;
            }
            self.shared.consumed.fetch_add(flushed, Ordering::Release);
            self.shared.flush_ack.store(req, Ordering::Release);
        }
        let mut mono = [0.0f32; BLOCK];
        let mut peak = 0.0f32;
        let mut taken = 0u64;
        for block in data.chunks_mut(BLOCK * channels) {
            let frames = block.len() / channels;
            let got = self.ring.pop_slice(&mut mono[..frames]);
            taken += got as u64;
            for (i, frame) in block.chunks_exact_mut(channels).enumerate() {
                let v = if i < got {
                    self.hold *= DECAY;
                    if self.hold.abs() < 1e-9 {
                        self.hold = 0.0;
                    }
                    let v = mono[i] * self.gain + self.hold;
                    self.gain = (self.gain + RAMP).min(1.0);
                    v
                } else {
                    self.gain = 0.0;
                    self.hold = self.last * DECAY;
                    self.hold
                };
                self.last = v;
                peak = peak.max(v.abs());
                let s = from_f32(v);
                for out in frame.iter_mut() {
                    *out = s;
                }
            }
            // A trailing partial frame (never seen in practice) is silence.
            let whole = frames * channels;
            for out in block[whole..].iter_mut() {
                *out = from_f32(0.0);
            }
        }
        if taken > 0 {
            self.shared.consumed.fetch_add(taken, Ordering::Release);
        }
        // Positive floats order like their bits.
        self.shared.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- cpal

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// The machine's default host (CoreAudio, WASAPI, ALSA).
pub struct CpalBackend {
    host: cpal::Host,
}

impl CpalBackend {
    pub fn new() -> CpalBackend {
        CpalBackend { host: cpal::default_host() }
    }
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// A device's name for people: its description, else its id.
pub fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).or_else(|_| d.id().map(|i| i.to_string())).unwrap_or_else(|_| "unnamed device".into())
}

/// A cpal error as a value a person can read.
pub fn from_cpal(e: &cpal::Error, input: bool) -> AudioError {
    let what = if input { "microphone" } else { "speaker" };
    let kind = match e.kind() {
        cpal::ErrorKind::PermissionDenied => ErrorKind::Permission,
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::StreamInvalidated => ErrorKind::DeviceGone,
        cpal::ErrorKind::DeviceBusy => ErrorKind::Busy,
        cpal::ErrorKind::UnsupportedConfig => ErrorKind::Unsupported,
        cpal::ErrorKind::Xrun => ErrorKind::Glitch,
        _ => ErrorKind::Other,
    };
    let message = match kind {
        ErrorKind::Permission if input => format!(
            "The microphone was refused ({e}). On macOS, allow it for the app sbxw was started from: System Settings, Privacy & Security, Microphone."
        ),
        ErrorKind::Permission => format!("The speaker was refused: {e}."),
        ErrorKind::DeviceGone => format!("The {what} went away: {e}."),
        ErrorKind::Busy => format!("The {what} is busy with another program: {e}."),
        ErrorKind::Unsupported => format!("The {what} does not accept this format: {e}."),
        _ => format!("{} error: {e}.", if input { "Microphone" } else { "Speaker" }),
    };
    AudioError { kind, message }
}

/// The configurations to try, best first: the device's default, then each
/// supported range at a standard rate, then at its highest rate.
fn candidates(device: &cpal::Device, input: bool) -> Vec<cpal::SupportedStreamConfig> {
    let mut out = Vec::new();
    let default = if input { device.default_input_config() } else { device.default_output_config() };
    if let Ok(c) = default {
        out.push(c);
    }
    let ranges: Vec<cpal::SupportedStreamConfigRange> = if input {
        device.supported_input_configs().map(|i| i.collect()).unwrap_or_default()
    } else {
        device.supported_output_configs().map(|i| i.collect()).unwrap_or_default()
    };
    let mut ranges = ranges;
    ranges.sort_by(|a, b| b.cmp_default_heuristics(a));
    for r in ranges {
        let c = r.try_with_standard_sample_rate().unwrap_or_else(|| r.with_max_sample_rate());
        if !out.iter().any(|o| o == &c) {
            out.push(c);
        }
    }
    out
}

macro_rules! dispatch {
    ($fmt:expr, $build:ident, $($arg:expr),*) => {
        match $fmt {
            cpal::SampleFormat::F32 => $build::<f32>($($arg),*),
            cpal::SampleFormat::I16 => $build::<i16>($($arg),*),
            cpal::SampleFormat::I32 => $build::<i32>($($arg),*),
            cpal::SampleFormat::U16 => $build::<u16>($($arg),*),
            cpal::SampleFormat::I8 => $build::<i8>($($arg),*),
            cpal::SampleFormat::U8 => $build::<u8>($($arg),*),
            cpal::SampleFormat::I24 => $build::<cpal::I24>($($arg),*),
            cpal::SampleFormat::U32 => $build::<u32>($($arg),*),
            cpal::SampleFormat::F64 => $build::<f64>($($arg),*),
            other => Err(AudioError { kind: ErrorKind::Unsupported, message: format!("sample format {other} is not handled") }),
        }
    };
}

fn build_input<T>(device: &cpal::Device, config: cpal::StreamConfig, make: &mut dyn FnMut(u32) -> InputSink, errors: &ErrorSink) -> Result<cpal::Stream, AudioError>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let mut s = make(config.sample_rate);
    let errs = errors.clone();
    let result = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| s.write(data, channels, <f32 as cpal::FromSample<T>>::from_sample_),
        move |e| errs.report(from_cpal(&e, true)),
        None,
    );
    result.map_err(|e| from_cpal(&e, true))
}

fn build_output<T>(device: &cpal::Device, config: cpal::StreamConfig, make: &mut dyn FnMut(u32) -> OutputSource, errors: &ErrorSink) -> Result<cpal::Stream, AudioError>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    let channels = config.channels as usize;
    let mut s = make(config.sample_rate);
    let errs = errors.clone();
    let result = device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], _| s.fill(data, channels, <T as cpal::FromSample<f32>>::from_sample_),
        move |e| errs.report(from_cpal(&e, false)),
        None,
    );
    result.map_err(|e| from_cpal(&e, false))
}

impl Backend for CpalBackend {
    fn open_input(&mut self, make: &mut dyn FnMut(u32) -> InputSink, errors: ErrorSink) -> Result<Opened, AudioError> {
        let device = self.host.default_input_device().ok_or_else(|| AudioError {
            kind: ErrorKind::NoDevice,
            message: "No microphone found. Plug one in or pick one as the default input in the system's sound settings.".into(),
        })?;
        let name = device_name(&device);
        // A configuration the device refuses: the next one is tried, and we
        // convert whatever format it uses.
        let mut last = None;
        for c in candidates(&device, true) {
            let (fmt, cfg) = (c.sample_format(), c.config());
            match dispatch!(fmt, build_input, &device, cfg.clone(), make, &errors) {
                Ok(stream) => {
                    stream.play().map_err(|e| from_cpal(&e, true))?;
                    return Ok(Opened { stream: Box::new(stream), device: name, rate: cfg.sample_rate, channels: cfg.channels, format: fmt.to_string() });
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| AudioError {
            kind: ErrorKind::Unsupported,
            message: format!("The microphone \"{name}\" offers no format this app can record."),
        }))
    }

    fn open_output(&mut self, make: &mut dyn FnMut(u32) -> OutputSource, errors: ErrorSink) -> Result<Opened, AudioError> {
        let device = self.host.default_output_device().ok_or_else(|| AudioError {
            kind: ErrorKind::NoDevice,
            message: "No speaker or headphones found. Pick an output in the system's sound settings.".into(),
        })?;
        let name = device_name(&device);
        let mut last = None;
        for c in candidates(&device, false) {
            let (fmt, cfg) = (c.sample_format(), c.config());
            match dispatch!(fmt, build_output, &device, cfg.clone(), make, &errors) {
                Ok(stream) => {
                    stream.play().map_err(|e| from_cpal(&e, false))?;
                    return Ok(Opened { stream: Box::new(stream), device: name, rate: cfg.sample_rate, channels: cfg.channels, format: fmt.to_string() });
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| AudioError {
            kind: ErrorKind::Unsupported,
            message: format!("The output \"{name}\" offers no format this app can play."),
        }))
    }

    fn any_device(&mut self) -> bool {
        self.host.default_input_device().is_some() || self.host.default_output_device().is_some()
    }
}

/// One device as `audio-demo list` shows it.
pub struct DeviceInfo {
    pub name: String,
    pub input: bool,
    pub output: bool,
    pub default_input: bool,
    pub default_output: bool,
    /// The default configuration, readable (`48000 Hz, 2 ch, f32`).
    pub input_config: Option<String>,
    pub output_config: Option<String>,
}

/// Every device of the default host. Blocking (the demo, not the UI thread).
pub fn list_devices() -> Result<(String, Vec<DeviceInfo>), AudioError> {
    let host = cpal::default_host();
    let id = |d: &cpal::Device| d.id().ok();
    let din = host.default_input_device().and_then(|d| id(&d));
    let dout = host.default_output_device().and_then(|d| id(&d));
    let describe = |c: Result<cpal::SupportedStreamConfig, cpal::Error>| {
        c.ok().map(|c| format!("{} Hz, {} ch, {}", c.sample_rate(), c.channels(), c.sample_format()))
    };
    let devices = host.devices().map_err(|e| from_cpal(&e, true))?;
    let mut out = Vec::new();
    for d in devices {
        let did = id(&d);
        out.push(DeviceInfo {
            name: device_name(&d),
            input: d.supports_input(),
            output: d.supports_output(),
            default_input: did.is_some() && did == din,
            default_output: did.is_some() && did == dout,
            input_config: describe(d.default_input_config()),
            output_config: describe(d.default_output_config()),
        });
    }
    Ok((format!("{:?}", host.id()), out))
}

#[cfg(test)]
mod tests {
    use super::super::ring::ring;
    use super::*;

    #[test]
    fn the_input_end_downmixes_and_converts_without_allocating_per_frame() {
        let (p, mut c) = ring(1024);
        let mut sink = InputSink { ring: p, dropped: Arc::new(AtomicU64::new(0)) };
        // Stereo i16: (16384, 0) is 0.25 after the average.
        let data: Vec<i16> = (0..600).flat_map(|_| [16384i16, 0]).collect();
        sink.write(&data, 2, |s| s as f32 / 32768.0);
        let mut out = vec![0.0; 1024];
        assert_eq!(c.pop_slice(&mut out), 600);
        assert!(out[..600].iter().all(|&v| (v - 0.25).abs() < 1e-6));
        // A full ring counts what it lost.
        sink.write(&vec![0i16; 2 * 2000], 2, |s| s as f32);
        assert_eq!(sink.dropped.load(Ordering::Relaxed), 2000 - 1024);
    }

    #[test]
    fn the_output_end_fills_every_channel_and_fades_on_underrun() {
        let (mut p, c) = ring(1024);
        let shared = Arc::new(OutputShared::default());
        let mut src = OutputSource::new(c, shared.clone());
        p.push_slice(&[0.5; 10]);
        let mut out = vec![0.0f32; 2 * 20];
        src.fill(&mut out, 2, |v| v);
        assert_eq!(shared.consumed.load(Ordering::Relaxed), 10);
        // The first sound ramps in from zero.
        assert!(out[0] == 0.0 && out[2] > 0.0 && out[18] > out[16]);
        // After the last sample: decaying, never a jump to zero.
        assert!(out[20] > 0.9 * out[18] && out[20] < out[18]);
        assert!(out.chunks(2).skip(10).all(|f| f[0] == f[1]));
        assert!(out[38] < out[20]);
        // Sound coming back crossfades from where the decay was.
        p.push_slice(&[0.5; 4]);
        let mut more = vec![0.0f32; 4];
        src.fill(&mut more, 1, |v| v);
        assert!((more[0] - out[38]).abs() < 0.03, "{} then {:?}", out[38], more);
        let peak = out.iter().chain(&more).fold(0.0f32, |m, v| m.max(v.abs()));
        assert_eq!(f32::from_bits(shared.peak.load(Ordering::Relaxed)), peak);
    }

    #[test]
    fn a_short_underrun_mid_sound_never_jumps() {
        let (mut p, c) = ring(1 << 12);
        let shared = Arc::new(OutputShared::default());
        let mut src = OutputSource::new(c, shared);
        let mut played = Vec::new();
        let mut buf = vec![0.0f32; 64];
        // A loud tone, with gaps of 3, 20 and 200 samples between bursts.
        let tone = |k: usize| 0.5 * (k as f32 * 0.05).cos();
        let mut k = 0;
        for gap in [3usize, 20, 200, 0] {
            let burst: Vec<f32> = (0..640).map(|_| {
                k += 1;
                tone(k)
            }).collect();
            p.push_slice(&burst);
            for _ in 0..10 {
                src.fill(&mut buf, 1, |v| v);
                played.extend_from_slice(&buf);
            }
            let mut tail = vec![0.0f32; gap];
            src.fill(&mut tail, 1, |v| v);
            played.extend_from_slice(&tail);
        }
        let step = played.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        // The tone's own steepest step is 0.5 * 0.05 = 0.025.
        assert!(step < 0.03, "largest step {step}");
        assert!(played.iter().any(|v| v.abs() > 0.45));
    }

    #[test]
    fn a_flush_empties_the_ring_and_counts_it_as_consumed() {
        let (mut p, c) = ring(4096);
        let shared = Arc::new(OutputShared::default());
        let mut src = OutputSource::new(c, shared.clone());
        p.push_slice(&[0.3; 3000]);
        shared.flush_req.store(1, Ordering::Release);
        let mut out = vec![0.0f32; 64];
        src.fill(&mut out, 1, |v| v);
        assert_eq!(shared.flush_ack.load(Ordering::Acquire), 1);
        assert_eq!(shared.consumed.load(Ordering::Acquire), 3000);
        assert!(out.iter().all(|&v| v == 0.0));
    }
}
