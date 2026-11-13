//! Try the native audio without the app: no window, nothing written to disk.
//!
//! ```text
//! audio-demo list                         the devices, defaults marked
//! audio-demo record <seconds>             record, show the level, play it back
//! audio-demo tone                         a one-second 440 Hz beep
//! audio-demo roundtrip [--daemon 127.0.0.1:7681] [--token <t>] [--seconds 4] [--say <text>]
//!                                         record, transcribe with the daemon, print
//!                                         the text, then have the daemon say a
//!                                         sentence and play it
//! ```
//!
//! Every command takes `--input <name>` and `--output <name>` to use a
//! device other than the default (names or ids as `list` prints them), and
//! `--no-silence-check` to keep recording a microphone that sends only exact
//! silence (by default that fails after 1 s: it is how macOS refuses one).
//!
//! Without `--token`, `roundtrip` reads `~/.sbxw/run/ui-token` (as the app
//! does). The recording lives in memory only; the log prints durations,
//! byte counts and device names, never sound or the token.

use std::io::{Cursor, Read};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use sbxw_desktop::audio::{backend, pcm, Audio, AudioEvent, Options, CAPTURE_RATE};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).format_timestamp_millis().init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let devices = (take_flag(&mut args, "--input"), take_flag(&mut args, "--output"));
    let no_silence = args.iter().position(|a| a == "--no-silence-check").map(|i| args.remove(i)).is_some();
    let devices = Devices { input: devices.0, output: devices.1, silence_check: !no_silence };
    let code = match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("record") => match args.get(1).and_then(|s| s.parse::<f64>().ok()) {
            Some(secs) if secs > 0.0 => record(secs, devices),
            _ => usage("record needs a number of seconds, e.g. `audio-demo record 3`"),
        },
        Some("tone") => tone(devices),
        Some("roundtrip") => roundtrip(&args[1..], devices),
        _ => usage(""),
    };
    std::process::exit(code);
}

fn usage(why: &str) -> i32 {
    if !why.is_empty() {
        eprintln!("{why}\n");
    }
    eprintln!("usage: audio-demo list | record <seconds> | tone | roundtrip [--daemon <host:port>] [--token <t>] [--seconds <n>] [--say <text>]");
    eprintln!("       any command: [--input <device name or id>] [--output <device name or id>] [--no-silence-check]");
    2
}

fn list() -> i32 {
    match backend::list_devices() {
        Ok((host, devices)) => {
            println!("audio host: {host}");
            if devices.is_empty() {
                println!("no devices");
            }
            for d in devices {
                let mut marks = Vec::new();
                if d.default_input {
                    marks.push("default input");
                }
                if d.default_output {
                    marks.push("default output");
                }
                let marks = if marks.is_empty() { String::new() } else { format!("  [{}]", marks.join(", ")) };
                println!("- {}{marks}", d.name);
                println!("    id:     {}", d.id);
                if let Some(c) = d.input_config {
                    println!("    input:  {c}");
                }
                if let Some(c) = d.output_config {
                    println!("    output: {c}");
                }
                if !d.input && !d.output {
                    println!("    (no usable configuration)");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

struct Devices {
    input: Option<String>,
    output: Option<String>,
    silence_check: bool,
}

/// Remove `name <value>` from `args`.
fn take_flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    if i + 1 >= args.len() {
        return None;
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Some(v)
}

/// An `Audio` whose waker wakes this thread.
fn open(devices: Devices) -> (Audio, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let wake = Arc::new(move || {
        let _ = tx.send(());
    });
    let Devices { input, output, silence_check } = devices;
    let opts = Options { silence_check: if silence_check { Options::default().silence_check } else { None }, ..Options::default() };
    let audio = Audio::with_backend(move || Box::new(backend::CpalBackend::named(input, output)), opts, wake);
    (audio, rx)
}

fn bar(level: f32) -> String {
    let n = (level * 40.0).round() as usize;
    format!("{}{}", "#".repeat(n), ".".repeat(40 - n.min(40)))
}

/// Record `secs` (or until the cap), printing the level ten times a second.
/// `None` when it failed (the reason is printed).
fn capture(audio: &mut Audio, wake: &mpsc::Receiver<()>, secs: f64) -> Option<Vec<u8>> {
    if let Err(e) = audio.start_capture() {
        eprintln!("error: {e}");
        return None;
    }
    let start = Instant::now();
    let mut started = false;
    let mut printed = Instant::now() - Duration::from_secs(1);
    let mut loudest = 0.0f32;
    let deadline = Duration::from_secs_f64(secs);
    println!("recording for {secs} s: speak now");
    while start.elapsed() < deadline {
        let _ = wake.recv_timeout(Duration::from_millis(20));
        for ev in audio.events() {
            match ev {
                AudioEvent::CaptureStarted => {
                    started = true;
                    println!("microphone open after {} ms", start.elapsed().as_millis());
                }
                AudioEvent::CaptureLevel(l) => {
                    loudest = loudest.max(l);
                    if printed.elapsed() >= Duration::from_millis(100) {
                        printed = Instant::now();
                        println!("{:5.1} s  {}  {:4.2}", start.elapsed().as_secs_f32(), bar(l), l);
                    }
                }
                AudioEvent::CaptureEnded => {
                    println!("the recording stopped by itself (the 120 s limit, or the device went away)");
                    return finish(audio, start, loudest);
                }
                AudioEvent::CaptureFailed(m) => {
                    eprintln!("error: {m}");
                    return None;
                }
                _ => {}
            }
        }
    }
    if !started {
        eprintln!("error: the microphone did not open in {secs} s");
    }
    finish(audio, start, loudest)
}

fn finish(audio: &mut Audio, start: Instant, loudest: f32) -> Option<Vec<u8>> {
    match audio.stop_capture() {
        Ok(bytes) => {
            let secs = bytes.len() as f32 / 4.0 / CAPTURE_RATE as f32;
            println!("recorded {secs:.2} s: {} bytes of 16 kHz mono f32 (wall clock {:.2} s, loudest level {loudest:.2})", bytes.len(), start.elapsed().as_secs_f32());
            if loudest < 0.25 {
                println!("note: that was very quiet; check the input level in the system's sound settings");
            }
            Some(bytes)
        }
        Err(e) => {
            eprintln!("error: {e}");
            None
        }
    }
}

/// Play `pcm` (s16 LE mono at `rate`) to the end; false on failure.
fn play(audio: &mut Audio, wake: &mpsc::Receiver<()>, rate: u32, pcm: Box<dyn Read + Send>, what: &str) -> bool {
    let t = Instant::now();
    if let Err(e) = audio.play(rate, pcm) {
        eprintln!("error: {e}");
        return false;
    }
    println!("playing {what}");
    let mut peak = 0.0f32;
    loop {
        if wake.recv_timeout(Duration::from_secs(60)).is_err() {
            eprintln!("error: no news from the audio thread for 60 s");
            return false;
        }
        for ev in audio.events() {
            match ev {
                AudioEvent::PlaybackStarted => println!("first sound after {} ms", t.elapsed().as_millis()),
                AudioEvent::PlaybackLevel(l) => peak = peak.max(l),
                AudioEvent::PlaybackEnded => {
                    println!("played: {:.2} s (output level up to {peak:.2})", t.elapsed().as_secs_f32());
                    return true;
                }
                AudioEvent::PlaybackFailed(m) => {
                    eprintln!("error: {m}");
                    return false;
                }
                _ => {}
            }
        }
    }
}

fn record(secs: f64, devices: Devices) -> i32 {
    let (mut audio, wake) = open(devices);
    let Some(bytes) = capture(&mut audio, &wake, secs) else { return 1 };
    if bytes.is_empty() {
        eprintln!("nothing was recorded");
        return 1;
    }
    // Back through the speech path: f32 16 kHz to s16, played at 16 kHz.
    let s16 = pcm::s16_le_bytes(&pcm::f32_from_le_bytes(&bytes));
    if play(&mut audio, &wake, CAPTURE_RATE, Box::new(Cursor::new(s16)), "the recording back") {
        0
    } else {
        1
    }
}

fn tone(devices: Devices) -> i32 {
    let (mut audio, wake) = open(devices);
    let rate = 24_000u32;
    let samples: Vec<f32> = (0..rate)
        .map(|i| {
            let t = i as f32 / rate as f32;
            // 10 ms fades at both ends.
            let env = (t / 0.01).min((1.0 - t) / 0.01).min(1.0);
            0.3 * env * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
        .collect();
    if play(&mut audio, &wake, rate, Box::new(Cursor::new(pcm::s16_le_bytes(&samples))), "a 1 s 440 Hz tone at 24 kHz") {
        0
    } else {
        1
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// A daemon error as its text (`{"ok":false,"error":"..."}`), else the status.
fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
            format!("HTTP {code}: {}", msg.unwrap_or(body))
        }
        other => other.to_string(),
    }
}

fn roundtrip(args: &[String], devices: Devices) -> i32 {
    let daemon = flag(args, "--daemon").unwrap_or_else(|| "127.0.0.1:7681".into());
    let base = if daemon.starts_with("http") { daemon.clone() } else { format!("http://{daemon}") };
    let token = match flag(args, "--token") {
        Some(t) => t,
        None => {
            let path = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default().join(".sbxw/run/ui-token");
            match std::fs::read_to_string(&path) {
                Ok(t) => t.trim().to_string(),
                Err(e) => {
                    eprintln!("error: no --token and cannot read {}: {e}", path.display());
                    return 2;
                }
            }
        }
    };
    let secs = flag(args, "--seconds").and_then(|s| s.parse::<f64>().ok()).unwrap_or(4.0);
    let say = flag(args, "--say").unwrap_or_else(|| "Hello, this is Jarvis, speaking through the desktop app.".into());
    let auth = format!("Bearer {token}");
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(3)).timeout_read(Duration::from_secs(120)).build();

    let (mut audio, wake) = open(devices);
    let Some(bytes) = capture(&mut audio, &wake, secs) else { return 1 };
    if bytes.is_empty() {
        eprintln!("nothing was recorded");
        return 1;
    }
    let t = Instant::now();
    println!("POST {base}/api/voice/transcribe ({} bytes)", bytes.len());
    let r = agent.post(&format!("{base}/api/voice/transcribe")).set("Authorization", &auth).set("Content-Type", "application/octet-stream").send_bytes(&bytes);
    match r.map_err(http_error).and_then(|r| r.into_json::<serde_json::Value>().map_err(|e| e.to_string())) {
        Ok(v) if v["ok"].as_bool() == Some(true) => {
            println!("heard in {:.1} s: \"{}\"", t.elapsed().as_secs_f32(), v["text"].as_str().unwrap_or(""));
            if let Some(a) = v["audio_s"].as_f64() {
                println!("the daemon counted {a:.2} s of audio");
            }
        }
        Ok(v) => {
            eprintln!("error: transcribe: {}", v["error"].as_str().unwrap_or("unknown"));
            return 1;
        }
        Err(e) => {
            eprintln!("error: transcribe: {e}");
            return 1;
        }
    }
    let t = Instant::now();
    println!("POST {base}/api/voice/speak ({} characters)", say.chars().count());
    let r = agent.post(&format!("{base}/api/voice/speak")).set("Authorization", &auth).send_json(serde_json::json!({ "text": say }));
    let resp = match r {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: speak: {}", http_error(e));
            return 1;
        }
    };
    let rate = resp.header("x-sample-rate").and_then(|s| s.parse::<u32>().ok()).unwrap_or(24_000);
    println!("answer headers after {} ms: {} at {rate} Hz", t.elapsed().as_millis(), resp.content_type());
    // Streamed: the voice starts while the daemon is still making it.
    let reader = Box::new(resp.into_reader());
    if play(&mut audio, &wake, rate, reader, "the daemon's voice") {
        0
    } else {
        1
    }
}
