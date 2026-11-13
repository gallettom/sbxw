# Jarvis in the desktop client

Thread `desktop-jarvis`, branch `sbxw/desktop-jarvis`. Work in progress;
the audio seam below is fixed and was sent to the lead first.

## The audio seam (for thread desktop-audio)

Defined in `desktop/src/voice.rs` (owned by desktop-jarvis). The real
implementation goes in `desktop/src/audio.rs` (owned by desktop-audio) and
only has to implement this trait and give a constructor of type
`voice::AudioFactory` (`fn(voice::Wake) -> Box<dyn voice::AudioIo>`).

```rust
pub const CAPTURE_RATE: u32 = 16_000;
pub const MAX_CAPTURE_SECS: u32 = 120;

pub enum AudioEvent {
    CaptureStarted,
    CaptureLevel(f32),      // 0.0..=1.0, ~20 per second while recording
    CaptureEnded,           // stopped by itself (120 s cap, device gone)
    CaptureFailed(String),  // shown to the person
    PlaybackStarted,
    PlaybackLevel(f32),     // optional
    PlaybackEnded,          // reader drained, or stop_playback called
    PlaybackFailed(String),
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;   // call after queueing an event

pub trait AudioIo: Send {
    fn available(&self) -> bool;
    fn start_capture(&mut self) -> Result<(), String>;      // stops playback first
    fn stop_capture(&mut self) -> Result<Vec<u8>, String>;  // 16 kHz mono f32 LE PCM
    fn cancel_capture(&mut self);
    fn play(&mut self, rate: u32, pcm: Box<dyn Read + Send>) -> Result<(), String>;
    fn stop_playback(&mut self);
    fn events(&mut self) -> Vec<AudioEvent>;
}

pub type AudioFactory = fn(Wake) -> Box<dyn AudioIo>;
```

Rules for the implementation:

- Every method is called on the UI thread and returns at once; recording,
  resampling and playback run on the implementation's own threads. Events
  are queued there, then `wake()` is called; the shell drains `events()`.
- `stop_capture` returns exactly the body of `POST /api/voice/transcribe`:
  16 kHz, mono, little-endian `f32` samples, no header, at most
  120 s (1 920 000 samples, the daemon's `MAX_SAMPLES`). The device's own
  rate and channel count are converted by the implementation.
- `play` receives the body of `POST /api/voice/speak` as a reader, as it
  arrives: 16-bit signed little-endian mono PCM at `rate` (the
  `x-sample-rate` header, 24000 today). Read it on your thread and start
  playing as soon as there is enough; a new `play` replaces the old one.
- `start_capture` stops any playback (talking over Jarvis interrupts it, as
  in the web UI).
- `available() == false` hides the talk button and read aloud. The null
  implementation (`voice::NullAudio`) is used headless and in fixtures;
  `voice::FakeAudio` is a scripted one for tests.

The shell's side: F8 held = `start_capture`, F8 released =
`stop_capture`, then the bytes go to `/api/voice/transcribe` and the text
goes to Jarvis as a typed message would. The key is the daemon's
`push_to_talk_key` from `GET /api/voice` (F8 by default).
