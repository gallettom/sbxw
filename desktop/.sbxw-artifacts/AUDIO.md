# sbxw desktop: native audio (thread desktop-audio)

Branch `sbxw/desktop-audio`. Microphone capture and speech playback for
Jarvis, with `cpal`, in `desktop/src/audio.rs` and `desktop/src/audio/`.
Recognition and synthesis stay in the daemon. No dependency on the UI, the
shell or the daemon client. Nothing is written to disk; the log gets
durations, byte counts and device names, never sound and never the token.

Built and tested on Linux (Debian trixie, aarch64, sbx sandbox) with **no
sound card**. What that proves and does not prove is spelled out in
"Evidence" and "Not verified" below.

## Formats (read from the code, not changed)

| What | Where | Format |
|---|---|---|
| `POST /api/voice/transcribe` body | `crates/daemon/src/http/ui.rs:1863-1866` ("16 kHz mono little-endian f32 PCM, the page resamples, so no audio container is parsed here"); `crates/daemon/src/svc/voice.rs:771-783` (`samples_of`: length a multiple of 4, `f32::from_le_bytes`, not empty) | raw f32 LE, mono, 16000 Hz, no header |
| Its limits | `svc/voice.rs:35-36` (`MAX_SAMPLES = 16_000 * 120`, `SAMPLE_RATE = 16_000`); `ui.rs:169-170` (body limit `MAX_SAMPLES * 4 + 1024`) | at most 120 s |
| Its answer | `ui.rs:1876` | JSON `{ok, text, masked, audio_s, ms}`, text already anonymised |
| `POST /api/voice/speak` answer | `ui.rs:1891-1901` (`content-type: audio/pcm`, header `x-sample-rate`, body streamed piece by piece); `svc/voice.rs:178` ("raw PCM (16-bit mono, SPEECH_RATE)"); `crates/core/src/voice.rs:195-196` (`SPEECH_RATE = 24_000`) | raw s16 LE, mono, 24000 Hz today, streamed |
| Its request | `ui.rs:1886-1889` (`VoiceSpeakBody`), `svc/voice.rs:759-769` | JSON `{text}`, 1 to 1000 characters |
| The page's capture | `assets/js/concierge.js:16-18`, `:34` (`VC_RATE = 16000`), `:186` (getUserMedia mono, echo cancellation, noise suppression), `:377-387` (`vcToPcm`: decode, render mono at 16 kHz), `:292-294` (POST as `application/octet-stream`), `:287` (under 300 ms is a tap, dropped) | |
| The page's playback | `concierge.js:714-720` (sentences in batches, one request each, pieces scheduled back to back), `:815-834` (rate from `x-sample-rate`, default 24000; an odd byte carried between pieces; `Int16` LE / 32768), `:175` (talking stops the voice), `:886-892` (mute stops it) | |

The seam fixed by desktop-jarvis (`desktop/src/voice.rs` on
`sbxw/desktop-jarvis`, commit a346ed8) states the same two formats; checked
against the lines above, they match.

## API

`audio.rs` holds a verbatim mirror of the seam (`AudioIo`, `AudioEvent`,
`Wake`, `AudioFactory`, `CAPTURE_RATE`, `MAX_CAPTURE_SECS`), marked
"seam mirror". After the merge, desktop-jarvis deletes the mirror and writes
`impl voice::AudioIo for audio::Audio` forwarding each method (the inherent
methods have the same names and signatures); the factory is `Audio::boxed`.

```text
Audio::new(wake) / Audio::boxed(wake)      spawns the control thread "sbxw-audio"
Audio::with_backend(make, Options, wake)   any backend (tests use a fake device)
available()        -> bool                 false only once it is known there is no device
start_capture()    -> Result<(), String>   stops playback first; device errors arrive as CaptureFailed
stop_capture()     -> Result<Vec<u8>, String>   16 kHz mono f32 LE, <= 120 s; empty if none
cancel_capture()
play(rate, Box<dyn Read + Send>) -> Result<(), String>   s16 LE mono; replaces what plays
stop_playback()
events()           -> Vec<AudioEvent>      oldest first; call after `wake`
is_capturing(), is_playing()               this side's view
backend::list_devices()                    blocking, for the demo
```

Events: `CaptureStarted`, `CaptureLevel(0..1)` every 50 ms (RMS, -60 dBFS
is 0, 0 dBFS is 1, linear in dB), `CaptureEnded` (120 s cap reached, or the
device went away; the recording is kept for `stop_capture`),
`CaptureFailed(text)`, `PlaybackStarted` (first sound taken by the device),
`PlaybackLevel(0..1)` every 50 ms (peak), `PlaybackEnded`,
`PlaybackFailed(text)`.

Semantics (sent to the lead, accepted by desktop-jarvis):

1. `play` while something plays replaces it, with no `PlaybackEnded` for
   the replaced one. `stop_playback` (and `start_capture`) give exactly one
   `PlaybackEnded` if something was playing, none otherwise.
2. Events of a capture or playback already stopped or replaced when the UI
   drains them are dropped. A `CaptureFailed` is always kept.
3. The reader of `play` is read on its own thread ("sbxw-audio-read"),
   never in a device callback. A `read` that blocks (desktop-jarvis chains
   speech batches inside one reader) is an underrun: it fades to silence in
   about 3 ms and crossfades back in, with no event. `PlaybackEnded` comes
   only at the reader's end (`Ok(0)`) once its sound has been handed to the
   device. A read error is `PlaybackFailed("Could not read the speech: ...")`.
   `stop_playback` and a replacing `play` return at once even when that
   reader is blocked: the thread is not joined; it sees at its next read
   that it was replaced and ends, and its sound is dropped.
4. Exact digital silence, or no data at all, for 1 s fails the capture with
   a message pointing at macOS's microphone permission (that is how a
   refusal shows there). `Options::silence_check: None` turns it off.
5. `stop_capture` is the only call that waits: for the control thread to
   flush the last 10 ms and hand the bytes over. Asserted under 200 ms in
   the tests, on a loaded machine; capped at `STOP_WAIT` (500 ms), after
   which it returns an error rather than block.

All errors are values: `AudioError { kind, message }` inside the module,
the message as `String` at the seam. Kinds: `NoDevice`, `DeviceGone`,
`Permission`, `Busy`, `Unsupported`, `Glitch` (an xrun: logged only),
`Other`. If the control thread ever panics, both a `CaptureFailed` and a
`PlaybackFailed` say so and the methods return `Err` afterwards; nothing
panics into the UI thread.

## Threads and real time

| Thread | Does | Never |
|---|---|---|
| Device callbacks (cpal's) | `backend::InputSink::write`: convert the device's format to f32, average the channels, push into the ring. `backend::OutputSource::fill`: honour a pending flush, pop from the ring, write the sample to every channel, fade on underrun, track the peak | allocate, lock, log, send on a channel. Only stack buffers (256 frames), the ring, atomics |
| Control "sbxw-audio" | Owns the cpal streams (not `Send` on every platform). Every 10 ms while something is open: drains the capture ring and resamples it to 16 kHz as it goes (so `stop_capture` has little left to do), measures levels, checks the cap and the silence; resamples only as much of the speech as it is about to push (0.25 s ahead), so its work per tick is bounded by real time. Sleeps in `recv()` when nothing is open. Closes the output after 5 s idle | touch the UI |
| Reader "sbxw-audio-read", one per `play` | Reads the body in 16 KB reads, decodes s16 (carrying an odd byte), sends to the control thread | |
| UI thread | Calls the methods; they send a command and return | wait, except `stop_capture` (bounded) |

The ring (`audio/ring.rs`) is a hand-written single-producer single-consumer
ring of f32 with Acquire/Release head and tail; 2^17 samples. A stop is a
flush request counter: the callback empties the ring and acknowledges, and
the control thread pushes the next sound only after the acknowledgement. A
callback that stops acknowledging for 500 ms gets its stream reopened.

Waking: the caller's `Wake` (`Arc<dyn Fn() + Send + Sync>`) is called after
each event is queued, as `client.rs` (`Wake`, line 117) wakes the UI. No
dependency on `client.rs`.

Resampling (`audio/resample.rs`): streaming windowed sinc, Blackman window,
48 zero crossings, cut-off at 85 % of the lower Nyquist frequency when going
down (90 % when going up), kernel tabulated at 512 phases with linear
interpolation, weights normalised per output (DC exact), exact positions,
exact output length `round(n * out / in)`. No crate: `rubato` would bring
FFT crates for a job this size does not need.

## Dependencies

One direct dependency, in `desktop/Cargo.toml`: `cpal = "=0.18.2"`
(released 2026-08-16, the latest stable). The lock gained these packages and
changed no existing version (two existing entries, `objc2-core-audio` and
`objc2-foundation 0.3.2`, gained feature dependencies):

| Crate | Version | Licence | Used on |
|---|---|---|---|
| cpal | 0.18.2 | Apache-2.0 | all |
| alsa | 0.11.0 | Apache-2.0/MIT | Linux, BSD |
| alsa-sys | 0.4.0 | MIT | Linux, BSD |
| coreaudio-rs | 0.14.2 | MIT/Apache-2.0 | macOS, iOS |
| objc2-audio-toolbox | 0.3.2 | Zlib OR Apache-2.0 OR MIT | macOS, iOS |
| objc2-avf-audio | 0.3.2 | Zlib OR Apache-2.0 OR MIT | iOS family (in the lock, not built on macOS) |
| mach2 | 0.6.0 | BSD-2-Clause OR MIT OR Apache-2.0 | macOS |
| dasp_sample | 0.11.0 | MIT OR Apache-2.0 | all (sample conversion) |
| num-derive | 0.4.2 | MIT OR Apache-2.0 | all (proc macro) |

No `ringbuf`, `rubato` or `hound`: the ring, the resampler and the PCM
codecs are hand-written and tested (the daemon's audio has no container).
No cpal feature is enabled (no JACK, PipeWire, PulseAudio or ASIO).

Build needs:
- **Linux**: `libasound2-dev` and `pkg-config` (alsa-sys links libasound
  through pkg-config). Installed in this sandbox with
  `sudo apt-get install -y libasound2-dev`; the app and every binary build.
  At run time, `libasound2`. PulseAudio and PipeWire systems work through
  ALSA's `default` device (their ALSA plugin).
- **macOS**: nothing to install: CoreAudio and AudioToolbox are system
  frameworks, found by the Xcode command line tools the app already needs.
  The microphone needs the person's permission (below).
- **Windows**: nothing to install: WASAPI through the `windows` crate
  (already in the lock at 0.62.2, now with cpal's features).

## Evidence

`cargo test` in `desktop/`: **99 passed, 2 ignored** (67 before this
thread, plus 32): 95 unit tests (63 + 32), 3 in `tests/link.rs`, 1 in
`tests/pump.rs`. The audio tests also passed 12 runs out of 12 with six
copies of the suite running at once (deliberately overloaded machine).

The 32 tests, by part:
- **Ring** (`ring.rs`, 3): power-of-two capacity and a full ring refusing
  more; order across the wrap; two threads, 200 000 samples, every one seen
  once in order.
- **Resampler and downmix** (`resample.rs`, 7): 1 kHz at 48 kHz to 16 kHz
  (length exactly 16 000, max error under 0.002, RMS ratio within 1 %);
  440 Hz at 44.1 kHz to 16 kHz; 300 Hz at 24 kHz up to 48 kHz and 44.1 kHz;
  a 10 kHz tone at 48 kHz (would fold to 6 kHz) attenuated by more than
  60 dB; chunked input equal to one-shot within 1e-6; pass-through, exact
  short lengths, DC unchanged; downmix averages stereo and 4 channels.
- **PCM** (`pcm.rs`, 4): f32 LE bytes, with the daemon's own test vector;
  s16 decoded identically whatever byte a piece is cut at, and one byte at a
  time; s16 encode then decode within one step; the meter's scale.
- **Callback ends** (`backend.rs`, 4): input end downmixes i16 stereo and
  counts what a full ring loses; output end fills every channel, ramps in,
  decays on underrun; a 3, 20 and 200 sample underrun mid-tone never steps
  more than the tone itself (added after a click was found and fixed: the
  decay used to jump to zero when sound came back); a flush empties the
  ring and is acknowledged.
- **Whole `Audio`, fake device** (`audio/tests.rs`, 14): a fake backend runs
  one thread per stream, called every 5 ms like a sound card (48 kHz,
  stereo). 48 kHz stereo recording returns 16 kHz mono f32 (frequency within
  15 Hz of 1000, peak 0.5 within 0.01), levels arrive about 20 per second at
  the expected value; the cap stops it by itself with exactly 300 ms worth
  (`CaptureEnded`, then `stop_capture` returns 19 200 bytes); the default cap
  is the daemon's 120 s; silence and no data fail with the permission text;
  no device fails both directions with the text, and `available()` turns
  false; a microphone that goes away ends the recording and keeps it; 24 kHz
  speech plays at 48 kHz for its exact length with Started, levels, Ended; a
  reader that sleeps 600 ms mid-clip: one Started, one Ended, a silent gap,
  no step bigger than 0.04, both halves played; a reader that fails
  mid-clip: `PlaybackFailed` with its message, no Ended; stop and replace
  return at once while the reader is blocked in `read`, the reader later
  ends by itself and its sound is dropped; a replacing play gives no Ended,
  the new sound replaces the old on the device, two stops give one Ended,
  silence right after; one output stream served both; talking stops the
  voice (Ended, then CaptureStarted); the trait object, and dropping it
  stops the device; the waker is called.

Real cpal streams, real ALSA, no sound card: ALSA here offers only its
`null` device (`aplay -L`: `null`; there is no `default`, no card, no
loopback module). With `audio-demo`:
- `list`: `audio host: Alsa`, one device `alsa:null`, 48000 Hz, 2 ch, f32
  both ways.
- `tone` and `record` on the default device: ALSA cannot open `default`;
  both print `error: No speaker could be opened (The requested audio device
  is not available...)` (resp. microphone) and exit 1. No panic.
- `record 2 --input alsa:null --output alsa:null`: the input stream opens
  (48000 Hz, 2 ch, f32), levels print at 0.00, and after 1.2 s
  `The microphone sends only silence. On macOS this is what a refused
  microphone permission looks like...` (the silence check on a real stream).
- `record 1.5 --input alsa:null --output alsa:null --no-silence-check`:
  `recorded 27.31 s: 1747628 bytes` (exactly 4 bytes per 16 kHz sample;
  the null device delivers far faster than real time, so more seconds than
  the 1.7 s of wall clock, and the log says how many samples the ring
  dropped), then the recording played back through a real output stream to
  `PlaybackEnded`.
- `roundtrip --daemon 127.0.0.1:9 ... --input alsa:null --no-silence-check`:
  records, then `error: transcribe: ... Connection refused (os error 111)`.
- `tone --output alsa:null`: the output opens, first sound after 43 ms,
  `PlaybackEnded`.

Other targets: `audio.rs` and its directory, copied into a scratch crate
with the same `cpal = "=0.18.2"`, pass `cargo check` for
`aarch64-apple-darwin` and `x86_64-pc-windows-msvc` with no warning. The
whole app cannot be checked for those targets here (Rune's `blake3` needs a
C cross compiler), and nothing was linked for them.

`cargo clippy` reports nothing in the files of this thread.

## Not verified

- **Every real device.** No microphone, speaker or headphones were used.
  Nobody has heard a sound from this code, or spoken into it.
- **macOS microphone permission.** Whether macOS gives silence, an error
  or no callbacks when the permission is refused is inferred, not observed;
  all three end in a `CaptureFailed` with the same advice. Whether the
  first-time permission dialog blocks the opening of the stream is unknown:
  if it does, `CaptureStarted` comes late, and a `stop_capture` during the
  dialog may return an error or an empty recording.
- **Windows**, entirely: type-checked only.
- **CoreAudio and WASAPI behaviour**: formats other than f32 (handled:
  i8, i16, i24, i32, u8, u16, u32, f64), a device removed while streaming
  (handled through cpal's error callback, tested only with the fake), the
  default device changing while a stream is open.
- **Latency and CPU on a real device**, in release.
- Integration with the shell (desktop-jarvis owns it).

## Known limits

- **No echo cancellation, no noise suppression.** The web UI asks the
  browser for both (`concierge.js:186`); cpal gives the raw microphone.
  Push-to-talk stops the voice before recording, which avoids the worst
  case; a noisy room is transcribed as is.
- **No live preview.** The web UI transcribes the recording so far every
  couple of seconds while the key is held (`concierge.js:205-250`). The seam
  has no call for that; a `peek()` returning a copy of the recording so far
  would be a few lines on the control thread.
- **Short taps.** The web UI drops recordings under 300 ms
  (`concierge.js:287`); `stop_capture` returns them. The caller decides.
- **The default device is fixed per stream.** A capture uses the default
  input at the moment it starts. The output stays open while it plays and
  for 5 s after, so a default output changed during that time is followed
  only once it has been idle 5 s (or failed). Reopening after idle adds the
  device's opening time (tens of ms) to the first sound.
- **A stalled output** (callbacks stop without an error) is reopened after
  500 ms; the speech already resampled for the old stream is lost (logged).
- **Bluetooth headsets**: opening their microphone switches macOS to the
  headset profile with lower quality output; nothing is done about it.
- **Debug builds**: the resampler is several times slower without
  optimisation. The control thread drained about 16 times real time in a
  debug build here (27 s of input in 1.7 s), so it keeps up, but measure in
  release before drawing conclusions about CPU.

## Test procedure on macOS (for the person)

Prerequisites: the repository at this branch, the Rust toolchain the app
already uses, a Mac with a microphone and speakers or headphones. For step
5, a running daemon with the voice installed (`sbxw up`, `sbxw voice
install`; speech needs Apple silicon).

1. Build:
   ```sh
   cd desktop
   cargo build --release --bin audio-demo
   ```
   Expect `Finished`. The binary is `target/release/audio-demo` (or under
   `$CARGO_TARGET_DIR` if you set one).

2. Devices:
   ```sh
   target/release/audio-demo list
   ```
   Expect `audio host: CoreAudio`, then one entry per device, the built-in
   microphone marked `[default input]` and the speakers or headphones
   `[default output]`, each with a line like `input:  48000 Hz, 1 ch, f32`.

3. A beep:
   ```sh
   target/release/audio-demo tone
   ```
   Hear a clean one-second 440 Hz tone (the A above middle C), no click at
   the start or end. See `playing a 1 s 440 Hz tone at 24 kHz`, `first sound
   after N ms` (N under about 100), `played: 1.0x s`.

4. Record and play back:
   ```sh
   target/release/audio-demo record 4
   ```
   The first time, macOS asks whether your terminal app (Terminal, iTerm,
   ...) may use the microphone: allow it, then run the command again. Speak
   for four seconds. See `microphone open after N ms`, then ten lines a
   second like ` 1.3 s  ##################......................  0.47`; the
   bars grow when you speak and shrink when you stop. Then `recorded 4.0x s:
   25xxxx bytes of 16 kHz mono f32` (64 000 bytes per second) and hear your
   voice played back, at the right pitch and speed, slightly muffled (16 kHz
   keeps nothing above 8 kHz, by design).
   - To see the refusal path: System Settings, Privacy & Security,
     Microphone, turn your terminal off, run `record 4` again. Expect, after
     about one second, `error: The microphone sends only silence...` (or
     `...sends no sound at all...`, or `The microphone was refused...`),
     exit code 1, no crash. Turn it back on afterwards.
   - To see the cap: `record 130`, speak a little, wait. After 120 s:
     `the recording stopped by itself`, then `recorded 120.00 s: 7680000
     bytes`.

5. With the daemon:
   ```sh
   target/release/audio-demo roundtrip --seconds 4
   ```
   (Reads the token from `~/.sbxw/run/ui-token`; `--daemon host:port` and
   `--token` override.) Say a sentence. See `POST
   .../api/voice/transcribe (256000 bytes)` or close, then `heard in N s:
   "your sentence"` and `the daemon counted 4.0x s of audio`. Then `POST
   .../api/voice/speak`, `answer headers after N ms: audio/pcm at 24000 Hz`,
   and hear Jarvis say "Hello, this is Jarvis, speaking through the desktop
   app." (`--say "..."` picks the sentence).

6. Unplug or switch headphones during `record 10`: expect either
   `the recording stopped by itself` and a recording played back, or an
   error line; no crash, no hang.

If something fails, paste:
- the output of `target/release/audio-demo list`;
- the failing command run again with
  `RUST_LOG=sbxw_desktop=debug target/release/audio-demo ...`, all of its
  output (it contains device names and timings, never sound or the token);
- what you heard, or did not hear, and the macOS version
  (`sw_vers -productVersion`).
