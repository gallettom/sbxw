//! The voice concierge's host side: downloading its models, and — in a build
//! with the `voice` feature — turning speech into text.
//!
//! * **Downloads are `curl`, like `sbxw update`.** No TLS stack in the binary;
//!   `-C -` resumes a `.part` file, so a cancelled or interrupted install
//!   picks up where it stopped. A file becomes its real name only once its
//!   size and (for an LFS file) its sha256 match the listing.
//! * **One install at a time**, owned by the daemon: the CLI and every tab
//!   watch the same job (`voice` events on the stream), and cancelling from
//!   either stops it.
//! * **The recogniser is loaded on first use and dropped after
//!   [`IDLE_UNLOAD`]** without one: the memory is spent while someone speaks,
//!   not while sandboxes need it.

use crate::app::App;
use crate::bus::Event;
use sbxw_core::voice::{self, Installed, Model, PlannedFile};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[cfg(feature = "voice")]
use std::time::Instant;

/// A loaded model nobody used for this long is dropped.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// How often a running download reports.
const POLL: Duration = Duration::from_millis(400);
/// The longest utterance accepted, in samples at 16 kHz (two minutes).
pub const MAX_SAMPLES: usize = 16_000 * 120;
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Running,
    Verifying,
    Done,
    Cancelled,
    Failed,
}

/// The install the daemon is running, or ran last.
#[derive(Clone, Serialize, Debug)]
pub struct Job {
    pub state: JobState,
    pub model: String,
    pub file: String,
    /// Bytes of the current model on disk, and what it weighs in all.
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct VoiceRuntime {
    job: Option<Job>,
    #[cfg(feature = "voice")]
    stt: Option<(Instant, Arc<std::sync::Mutex<crate::voice_engine::Stt>>)>,
    #[cfg(feature = "voice")]
    last_used: Option<Instant>,
}

impl App {
    fn models_root(&self) -> PathBuf {
        self.store.paths.models()
    }

    /// What `/api/voice` answers and every `voice` event carries.
    pub fn voice_status(&self) -> Value {
        let root = self.models_root();
        let models: Vec<Value> = self
            .cfg
            .voice
            .models()
            .into_iter()
            .map(|m| {
                json!({
                    "id": m.id, "role": m.role, "label": m.label, "license": m.license,
                    "approx_mb": m.approx_mb, "installed": voice::installed(&root, m),
                })
            })
            .collect();
        let ready = models.iter().all(|m| m["installed"]["state"] == "ready");
        let rt = self.voice.lock().unwrap();
        json!({
            "compiled": cfg!(feature = "voice"),
            "ready": ready,
            "models": models,
            "approx_mb": self.cfg.voice.models().iter().map(|m| m.approx_mb).sum::<u32>(),
            "job": rt.job,
            "hint": voice::INSTALL_HINT,
            "push_to_talk_key": self.cfg.voice.push_to_talk_key,
            "dir": root,
        })
    }

    fn voice_publish(&self) {
        self.bus.send(Event::Voice(self.voice_status()));
    }

    fn voice_job(&self, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.voice.lock().unwrap().job.as_mut() {
            f(j);
        }
        self.voice_publish();
    }

    /// Start downloading every model the config names that is not ready.
    pub fn voice_install(self: &Arc<Self>) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut rt = self.voice.lock().unwrap();
            if rt.job.as_ref().is_some_and(|j| matches!(j.state, JobState::Running | JobState::Verifying)) {
                return Err("the voice models are already downloading".into());
            }
            rt.job = Some(Job {
                state: JobState::Running,
                model: String::new(),
                file: String::new(),
                done: 0,
                total: 0,
                error: None,
                cancel: cancel.clone(),
            });
        }
        self.voice_publish();
        let app = self.clone();
        tokio::spawn(async move {
            let res = app.run_install(&cancel).await;
            app.voice_job(|j| match res {
                Ok(()) => j.state = JobState::Done,
                Err(_) if cancel.load(Ordering::SeqCst) => j.state = JobState::Cancelled,
                Err(e) => {
                    tracing::warn!("voice: install failed: {e}");
                    j.state = JobState::Failed;
                    j.error = Some(e);
                }
            });
        });
        Ok(())
    }

    /// Stop the running install; what was downloaded stays, to resume.
    pub fn voice_cancel(&self) -> Result<(), String> {
        let rt = self.voice.lock().unwrap();
        match rt.job.as_ref() {
            Some(j) if matches!(j.state, JobState::Running | JobState::Verifying) => {
                j.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err("no voice download is running".into()),
        }
    }

    /// Delete every downloaded model (not only the configured ones).
    pub fn voice_remove(&self) -> Result<(), String> {
        {
            let mut rt = self.voice.lock().unwrap();
            if rt.job.as_ref().is_some_and(|j| matches!(j.state, JobState::Running | JobState::Verifying)) {
                return Err("cancel the download first".into());
            }
            rt.job = None;
            #[cfg(feature = "voice")]
            {
                rt.stt = None;
            }
        }
        let root = self.models_root();
        for m in voice::CATALOG {
            let dir = voice::model_dir(&root, m);
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))?;
            }
        }
        self.voice_publish();
        Ok(())
    }

    async fn run_install(&self, cancel: &AtomicBool) -> Result<(), String> {
        let root = self.models_root();
        for m in self.cfg.voice.models() {
            if matches!(voice::installed(&root, m), Installed::Ready { .. }) {
                continue;
            }
            self.voice_job(|j| {
                j.model = m.id.into();
                j.file = "listing".into();
                j.done = 0;
                j.total = m.approx_mb as u64 * 1_000_000;
            });
            let tree = curl_text(&voice::tree_url(m)).await?;
            let files = voice::plan(m, &tree)?;
            self.install_model(&root, m, &files, cancel).await?;
        }
        Ok(())
    }

    async fn install_model(&self, root: &Path, m: &Model, files: &[PlannedFile], cancel: &AtomicBool) -> Result<(), String> {
        let dir = voice::model_dir(root, m);
        let total: u64 = files.iter().map(|f| f.size).sum();
        let mut done: u64 = 0;
        for f in files {
            let dest = dir.join(&f.path);
            if file_len(&dest) == Some(f.size) {
                done += f.size;
                continue;
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let part = part_path(&dest);
            if file_len(&part).is_some_and(|n| n > f.size) {
                let _ = std::fs::remove_file(&part);
            }
            self.voice_job(|j| {
                j.state = JobState::Running;
                j.file = f.path.clone();
                j.done = done;
                j.total = total;
            });
            if file_len(&part) != Some(f.size) {
                self.curl_file(&f.url, &part, done, cancel).await?;
            }
            let got = file_len(&part).unwrap_or(0);
            if got != f.size {
                return Err(format!("{}: {got} bytes, expected {}", f.path, f.size));
            }
            if let Some(want) = &f.sha256 {
                self.voice_job(|j| j.state = JobState::Verifying);
                let have = sha256_of(&part).await?;
                if !have.eq_ignore_ascii_case(want) {
                    let _ = std::fs::remove_file(&part);
                    return Err(format!("{}: checksum mismatch (expected {want}, got {have})", f.path));
                }
            }
            std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            done += f.size;
        }
        sbxw_daemon_write(&dir.join(voice::MARKER), voice::marker_json(files).as_bytes())?;
        self.voice_job(|j| {
            j.done = total;
            j.total = total;
        });
        Ok(())
    }

    /// `curl -C -` into `part`, reporting its growth until it exits; killed
    /// when `cancel` is raised.
    async fn curl_file(&self, url: &str, part: &Path, before: u64, cancel: &AtomicBool) -> Result<(), String> {
        let mut child = tokio::process::Command::new(sbxw_core::sysbin::sys_bin("curl"))
            .args(["-fsSL", "--retry", "3", "-C", "-", "-o"])
            .arg(part)
            .arg(url)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("could not run curl: {e}"))?;
        loop {
            tokio::select! {
                st = child.wait() => {
                    let st = st.map_err(|e| e.to_string())?;
                    return if st.success() { Ok(()) } else { Err(format!("curl failed ({st}) for {url}")) };
                }
                _ = tokio::time::sleep(POLL) => {
                    if cancel.load(Ordering::SeqCst) {
                        let _ = child.kill().await;
                        return Err("cancelled".into());
                    }
                    let now = file_len(part).unwrap_or(0);
                    self.voice_job(|j| j.done = before + now);
                }
            }
        }
    }

    /// Text for 16 kHz mono samples.
    #[cfg(feature = "voice")]
    pub async fn voice_transcribe(self: &Arc<Self>, samples: Vec<f32>) -> Result<String, String> {
        let m = voice::model(&self.cfg.voice.stt).ok_or("no speech-to-text model configured")?;
        let root = self.models_root();
        if !matches!(voice::installed(&root, m), Installed::Ready { .. }) {
            return Err(format!("the voice models are not installed: run `{}`", voice::INSTALL_HINT));
        }
        let loaded = self.voice.lock().unwrap().stt.as_ref().map(|(_, s)| s.clone());
        let stt = match loaded {
            Some(s) => s,
            None => {
                let dir = voice::model_dir(&root, m);
                let s = tokio::task::spawn_blocking(move || crate::voice_engine::Stt::load(&dir))
                    .await
                    .map_err(|e| e.to_string())??;
                let s = Arc::new(std::sync::Mutex::new(s));
                self.voice.lock().unwrap().stt = Some((Instant::now(), s.clone()));
                s
            }
        };
        self.voice.lock().unwrap().last_used = Some(Instant::now());
        self.schedule_unload();
        tokio::task::spawn_blocking(move || stt.lock().unwrap().transcribe(&samples))
            .await
            .map_err(|e| e.to_string())
    }

    #[cfg(not(feature = "voice"))]
    pub async fn voice_transcribe(self: &Arc<Self>, _samples: Vec<f32>) -> Result<String, String> {
        Err("this sbxw was built without voice (cargo build --features voice)".into())
    }

    #[cfg(feature = "voice")]
    fn schedule_unload(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(IDLE_UNLOAD + Duration::from_secs(1)).await;
            let mut rt = app.voice.lock().unwrap();
            if rt.last_used.is_some_and(|t| t.elapsed() >= IDLE_UNLOAD) {
                rt.stt = None;
                tracing::info!("voice: speech model unloaded after {}s idle", IDLE_UNLOAD.as_secs());
            }
        });
    }
}

/// Little-endian f32 PCM, as the page sends it.
pub fn samples_of(body: &[u8]) -> Result<Vec<f32>, String> {
    if !body.len().is_multiple_of(4) {
        return Err("audio must be 32-bit float samples".into());
    }
    let n = body.len() / 4;
    if n == 0 {
        return Err("no audio".into());
    }
    if n > MAX_SAMPLES {
        return Err(format!("audio longer than {} s", MAX_SAMPLES as u32 / SAMPLE_RATE));
    }
    Ok(body.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn file_len(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

fn sbxw_daemon_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::store::write_atomic(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

async fn curl_text(url: &str) -> Result<String, String> {
    let out = tokio::process::Command::new(sbxw_core::sysbin::sys_bin("curl"))
        .args(["-fsSL", "--retry", "2", url])
        .output()
        .await
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("could not list {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// `sha256sum`, or macOS's `shasum -a 256`.
async fn sha256_of(path: &Path) -> Result<String, String> {
    let run = |prog: &str, args: &[&str]| {
        let mut c = tokio::process::Command::new(sbxw_core::sysbin::sys_bin(prog));
        c.args(args).arg(path);
        c
    };
    let out = match run("sha256sum", &[]).output().await {
        Ok(o) if o.status.success() => o,
        _ => run("shasum", &["-a", "256"]).output().await.map_err(|e| format!("no sha256 tool: {e}"))?,
    };
    if !out.status.success() {
        return Err(format!("could not checksum {}", path.display()));
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| "empty checksum".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_little_endian_f32_and_bounded() {
        let bytes: Vec<u8> = [0.5f32, -1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(samples_of(&bytes).unwrap(), vec![0.5, -1.0]);
        assert!(samples_of(&[0, 0, 0]).is_err());
        assert!(samples_of(&[]).is_err());
        assert!(samples_of(&vec![0u8; (MAX_SAMPLES + 1) * 4]).is_err());
    }

    #[tokio::test]
    async fn sha256_matches_a_known_digest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_of(&p).await.unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
