//! The voice concierge's host side: downloading its models, turning speech
//! into text (a build with the `voice` feature) and text into speech.
//!
//! * **Downloads are `curl`, like `sbxw update`.** No TLS stack in the binary;
//!   `-C -` resumes a `.part` file, so a cancelled or interrupted install
//!   picks up where it stopped. A file becomes its real name only once its
//!   size and (for an LFS file) its sha256 match the listing.
//! * **One install at a time**, owned by the daemon: the CLI and every tab
//!   watch the same job (`voice` events on the stream), and cancelling from
//!   either stops it.
//! * **Speech is another process.** Voxtral TTS runs in `mlx-audio`'s server
//!   (Python, MLX), which `uvx` fetches at a pinned version during the install
//!   and the daemon starts on the first sentence to read, on 127.0.0.1 and a
//!   free port. The one exception to "no side process": no Rust runtime reads
//!   a mix of French and English this well.
//! * **Each model is loaded on first use and dropped after [`IDLE_UNLOAD`]**
//!   without one, the recogniser and the speech server each on its own
//!   clock: dictating does not keep the voice in memory.

use crate::app::App;
use crate::bus::Event;
use sbxw_core::voice::{self, Installed, Model, PlannedFile};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// Written beside the models once `uvx` has fetched the speech runtime: the
/// version it fetched.
const RUNTIME_MARKER: &str = ".speech-runtime";
/// How long the speech server may take to answer its first request (it
/// imports MLX), then to load the model, then to speak one sentence.
const SPEECH_LOG: &str = "server.err.log";
const SPEECH_START: Duration = Duration::from_secs(90);
const SPEECH_LOAD: Duration = Duration::from_secs(300);
const SPEECH_SAY: Duration = Duration::from_secs(240);

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
    stt: Option<Arc<std::sync::Mutex<crate::voice_engine::Stt>>>,
    #[cfg(feature = "voice")]
    stt_used: Option<Instant>,
    speech: Option<Arc<SpeechServer>>,
    speech_used: Option<Instant>,
    /// Held while a model loads (see `ensure_*`).
    loading: Arc<tokio::sync::Mutex<()>>,
}

/// The running `mlx_audio.server`, in a process group of its own: `uvx`
/// starts Python as its child, and stopping one must stop both.
pub struct SpeechServer {
    port: u16,
    /// Where its stderr goes.
    log: PathBuf,
    pid: Option<u32>,
    child: std::sync::Mutex<tokio::process::Child>,
}

impl SpeechServer {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    /// Have the server read the model at `dir` into memory.
    async fn load(&self, dir: &Path) -> Result<(), String> {
        let url = self.url(&format!("/v1/models?model_name={}", voice::url_component(&dir.to_string_lossy())));
        curl_local(&["-X", "POST", &url], None, SPEECH_LOAD).await.map(|_| ()).map_err(|e| format!("loading the voice: {e} (see {})", self.log.display()))
    }

    /// `text` as a WAV file, in `voice`.
    /// Ask for `text` aloud: the sound comes back as it is made. An error is
    /// the server's own words, known before the first piece of sound.
    async fn say(&self, dir: &Path, voice: &str, text: &str) -> Result<SpeechStream, String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body = voice::speech_request(dir, voice, text);
        // `-i`: the status line comes first on stdout, so a refusal is told
        // from sound without waiting for curl to end. `-N`: nothing is held back.
        let mut child = tokio::process::Command::new(sbxw_core::sysbin::sys_bin("curl"))
            .args(["-sS", "-N", "-i", "--noproxy", "*", "--max-time", &SPEECH_SAY.as_secs().to_string()])
            .args(["-H", "content-type: application/json", "--data-binary", "@-", &self.url("/v1/audio/speech")])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("could not run curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(body.as_bytes()).await.map_err(|e| e.to_string())?;
        }
        let mut out = child.stdout.take().ok_or("curl has no output")?;
        let mut got = Vec::new();
        let mut piece = vec![0u8; SPEECH_PIECE];
        // Headers, then the first bytes of the body: enough to tell sound
        // from a refusal.
        let (status, start) = loop {
            if let Some((status, start)) = voice::response_head(&got) {
                if got.len() > start {
                    break (status, start);
                }
            }
            let n = out.read(&mut piece).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break voice::response_head(&got).unwrap_or((0, got.len()));
            }
            got.extend_from_slice(&piece[..n]);
        };
        if status == 200 && got.len() > start {
            got.drain(..start);
            return Ok(SpeechStream { first: Some(got), out, child, piece, _server: None });
        }
        let mut said = got.split_off(start.min(got.len()));
        if status != 0 {
            let _ = (&mut out).take(4096).read_to_end(&mut said).await;
        }
        let mut said = String::from_utf8_lossy(&said).trim().to_string();
        if said.is_empty() {
            let mut err = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut err).await;
            }
            said = match (status, err.trim()) {
                (200, _) => "the voice returned no audio".into(),
                (_, "") => format!("the speech server answered {status}"),
                (_, err) => err.into(),
            };
        }
        Err(format!("speaking: {}", said.chars().take(300).collect::<String>()))
    }
}

/// How much sound is read from the speech server at a time.
const SPEECH_PIECE: usize = 16 * 1024;

/// Speech as it is made: raw PCM (16-bit mono, [`voice::SPEECH_RATE`]), in
/// pieces. Dropping it stops the request, and the server stops making sound.
pub struct SpeechStream {
    first: Option<Vec<u8>>,
    out: tokio::process::ChildStdout,
    child: tokio::process::Child,
    piece: Vec<u8>,
    /// Kept alive while it speaks: the idle clock must not stop it mid-sentence.
    _server: Option<Arc<SpeechServer>>,
}

impl SpeechStream {
    /// The next piece of sound, `None` at the end. A request that breaks
    /// midway ends the sound early; why is in the log.
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        if let Some(first) = self.first.take() {
            return Some(first);
        }
        match self.out.read(&mut self.piece).await {
            Ok(n) if n > 0 => Some(self.piece[..n].to_vec()),
            _ => {
                if let Ok(status) = self.child.wait().await {
                    if !status.success() {
                        tracing::warn!("voice: the speech request ended early ({status})");
                    }
                }
                None
            }
        }
    }
}

impl Drop for SpeechServer {
    fn drop(&mut self) {
        // TERM to the group, so Python frees the model; the handle's own
        // kill-on-drop covers a `uvx` that ignored it.
        if let Some(pid) = self.pid {
            let _ = std::process::Command::new(sbxw_core::sysbin::sys_bin("kill")).args(["-TERM", "--", &format!("-{pid}")]).stderr(std::process::Stdio::null()).status();
        }
    }
}

impl App {
    fn models_root(&self) -> PathBuf {
        self.store.paths.models()
    }

    /// What `/api/voice` answers and every `voice` event carries.
    pub fn voice_status(&self) -> Value {
        let root = self.models_root();
        let models: Vec<Value> = self
            .voice_models()
            .into_iter()
            .map(|m| {
                json!({
                    "id": m.id, "role": m.role, "label": m.label, "license": m.license,
                    "approx_mb": m.approx_mb, "installed": voice::installed(&root, m),
                })
            })
            .collect();
        let runtime = !voice::speech_supported() || self.speech_runtime_ready();
        let ready = runtime && models.iter().all(|m| m["installed"]["state"] == "ready");
        let rt = self.voice.lock().unwrap();
        json!({
            "compiled": cfg!(feature = "voice"),
            "ready": ready,
            "models": models,
            "approx_mb": self.voice_models().iter().map(|m| m.approx_mb).sum::<u32>(),
            // Reading aloud: whether this machine can, and with what.
            "speech": {
                "supported": voice::speech_supported(),
                "runtime": voice::SPEECH_RUNTIME,
                "runtime_ready": runtime,
                "voice": self.cfg.voice.tts_voice,
            },
            // What is masked in dictated words: see `anonymise_rules`.
            "anonymise": match self.anonymise_rules() {
                Ok(rules) => json!({ "file": self.store.paths.anonymise(), "terms": rules.terms(), "patterns": rules.patterns() }),
                Err(e) => json!({ "file": self.store.paths.anonymise(), "error": e }),
            },
            "job": rt.job,
            "hint": voice::INSTALL_HINT,
            "push_to_talk_key": self.cfg.voice.push_to_talk_key,
            "dictate_key": self.cfg.voice.dictate_key,
            "dir": root,
        })
    }

    /// The models this machine can use: without MLX the voice is not fetched.
    fn voice_models(&self) -> Vec<&'static Model> {
        self.cfg.voice.models().into_iter().filter(|m| m.role != voice::Role::Tts || voice::speech_supported()).collect()
    }

    fn speech_runtime_ready(&self) -> bool {
        std::fs::read_to_string(self.models_root().join(RUNTIME_MARKER)).is_ok_and(|t| t.trim() == voice::SPEECH_RUNTIME)
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

    /// Drop every loaded model now (the daemon is about to exit).
    pub fn voice_unload(&self) {
        let mut rt = self.voice.lock().unwrap();
        #[cfg(feature = "voice")]
        {
            rt.stt = None;
        }
        rt.speech = None;
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
            rt.speech = None;
        }
        // Whatever is there, models of an older catalog included.
        let root = self.models_root();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let p = e.path();
                let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                r.map_err(|e| format!("removing {}: {e}", p.display()))?;
            }
        }
        self.voice_publish();
        Ok(())
    }

    async fn run_install(&self, cancel: &AtomicBool) -> Result<(), String> {
        let root = self.models_root();
        for m in self.voice_models() {
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
        if voice::speech_supported() && !self.speech_runtime_ready() {
            self.voice_job(|j| {
                j.state = JobState::Running;
                j.model = "speech runtime".into();
                j.file = voice::SPEECH_RUNTIME.into();
            });
            self.fetch_speech_runtime(cancel).await?;
            sbxw_daemon_write(&root.join(RUNTIME_MARKER), voice::SPEECH_RUNTIME.as_bytes())?;
        }
        Ok(())
    }

    /// Have `uvx` download the runtime's packages now, by running its server
    /// for `--help`: starting it later then needs no network.
    async fn fetch_speech_runtime(&self, cancel: &AtomicBool) -> Result<(), String> {
        let mut child = speech_command(&uvx()?)
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("could not run uvx: {e}"))?;
        let mut stderr = child.stderr.take();
        loop {
            tokio::select! {
                st = child.wait() => {
                    let st = st.map_err(|e| e.to_string())?;
                    if st.success() {
                        return Ok(());
                    }
                    let mut why = String::new();
                    if let Some(e) = stderr.as_mut() {
                        let _ = tokio::io::AsyncReadExt::read_to_string(e, &mut why).await;
                    }
                    let last = why.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
                    return Err(format!("uvx could not fetch {} ({st}): {last}", voice::SPEECH_RUNTIME));
                }
                _ = tokio::time::sleep(POLL) => {
                    if cancel.load(Ordering::SeqCst) {
                        let _ = child.kill().await;
                        return Err("cancelled".into());
                    }
                }
            }
        }
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
                j.file.clone_from(&f.path);
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
            // HTTPS only, also across Hugging Face's redirect to its CDN.
            .args(["--proto", "=https", "--proto-redir", "=https", "-fsSL", "--retry", "3", "-C", "-", "-o"])
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

    /// The installed model for `role`, or why not.
    fn ready_model(&self, role: voice::Role) -> Result<(&'static Model, PathBuf), String> {
        let m = self.cfg.voice.models().into_iter().find(|m| m.role == role).ok_or("no model configured for this role")?;
        let root = self.models_root();
        if !matches!(voice::installed(&root, m), Installed::Ready { .. }) {
            return Err(format!("the voice models are not installed: run `{}`", voice::INSTALL_HINT));
        }
        Ok((m, voice::model_dir(&root, m)))
    }

    // Each `ensure_*` loads its model once: loads are serialised by `loading`,
    // so a warm-up and a first request never load the same model twice.

    #[cfg(feature = "voice")]
    async fn ensure_stt(self: &Arc<Self>) -> Result<Arc<std::sync::Mutex<crate::voice_engine::Stt>>, String> {
        let lock = self.voice.lock().unwrap().loading.clone();
        let _g = lock.lock().await;
        self.voice.lock().unwrap().stt_used = Some(Instant::now());
        self.schedule_unload();
        if let Some(s) = self.voice.lock().unwrap().stt.as_ref() {
            return Ok(s.clone());
        }
        let (_, dir) = self.ready_model(voice::Role::Stt)?;
        let s = tokio::task::spawn_blocking(move || crate::voice_engine::Stt::load(&dir)).await.map_err(|e| e.to_string())??;
        let s = Arc::new(std::sync::Mutex::new(s));
        self.voice.lock().unwrap().stt = Some(s.clone());
        Ok(s)
    }

    /// The speech server, started and holding the model: the first call
    /// takes as long as loading a few GB does.
    async fn ensure_speech(self: &Arc<Self>) -> Result<(Arc<SpeechServer>, PathBuf), String> {
        if !voice::speech_supported() {
            return Err("reading aloud needs a Mac with Apple Silicon (the voice runs on MLX)".into());
        }
        let (_, dir) = self.ready_model(voice::Role::Tts)?;
        if !self.speech_runtime_ready() {
            return Err(format!("the speech runtime is not installed: run `{}`", voice::INSTALL_HINT));
        }
        let lock = self.voice.lock().unwrap().loading.clone();
        let _g = lock.lock().await;
        self.voice.lock().unwrap().speech_used = Some(Instant::now());
        self.schedule_unload();
        let running = self.voice.lock().unwrap().speech.clone();
        if let Some(s) = running.filter(|s| s.alive()) {
            return Ok((s, dir));
        }
        let t = Instant::now();
        let server = Arc::new(start_speech_server(&uvx()?, &self.store.paths.run()).await?);
        // Loaded here, so the first sentence does not time out on it.
        server.load(&dir).await?;
        tracing::info!("voice: speech server ready on port {} in {} ms", server.port, t.elapsed().as_millis());
        self.voice.lock().unwrap().speech = Some(server.clone());
        Ok((server, dir))
    }

    /// Load in the background what is about to be used, so the first words
    /// wait for nothing: the recogniser when a talk key goes down, the voice
    /// too (`speech`) when the panel opens.
    pub fn voice_warm(self: &Arc<Self>, speech: bool) {
        let app = self.clone();
        tokio::spawn(async move {
            #[cfg(feature = "voice")]
            if let Err(e) = app.ensure_stt().await {
                tracing::debug!("voice: recogniser not warmed: {e}");
            }
            if speech {
                if let Err(e) = app.ensure_speech().await {
                    tracing::debug!("voice: speech not warmed: {e}");
                }
            }
        });
    }

    /// Text for 16 kHz mono samples.
    #[cfg(feature = "voice")]
    pub async fn voice_transcribe(self: &Arc<Self>, samples: Vec<f32>) -> Result<String, String> {
        let stt = self.ensure_stt().await?;
        tokio::task::spawn_blocking(move || stt.lock().unwrap().transcribe(&samples))
            .await
            .map_err(|e| e.to_string())
    }

    /// `text` read aloud, the sound coming back as it is made. The page sends
    /// a message a few sentences at a time.
    pub async fn voice_speak(self: &Arc<Self>, text: String) -> Result<SpeechStream, String> {
        check_speech(&text)?;
        let (server, dir) = self.ensure_speech().await?;
        let voice = &self.cfg.voice.tts_voice;
        // Symbols are said in words: see `sbxw_core::spoken`.
        let text = sbxw_core::spoken::speakable(&text, sbxw_core::spoken::Lang::of_voice(voice));
        let mut speech = server.say(&dir, voice, &text).await?;
        speech._server = Some(server);
        self.speech_used();
        Ok(speech)
    }

    /// The person's rules for what is masked, read from `anonymise.toml` each
    /// time, so a name added is hidden from the next sentence on. No file:
    /// the shapes alone. A file that does not parse is an error, never
    /// "nothing to hide": words must not leave unmasked because of a typo.
    pub fn anonymise_rules(&self) -> Result<sbxw_core::anonymise::Rules, String> {
        let path = self.store.paths.anonymise();
        match std::fs::read_to_string(&path) {
            Ok(text) => sbxw_core::anonymise::Rules::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// `text` as it may leave the machine: every dictated word goes through
    /// here, and so does a message on its way to an agent.
    pub fn anonymise(&self, text: &str) -> Result<sbxw_core::anonymise::Masked, String> {
        Ok(sbxw_core::anonymise::mask(text, &self.anonymise_rules()?))
    }

    /// The voice is in use: the idle clock starts again.
    pub fn speech_used(&self) {
        self.voice.lock().unwrap().speech_used = Some(Instant::now());
    }

    #[cfg(not(feature = "voice"))]
    pub async fn voice_transcribe(self: &Arc<Self>, _samples: Vec<f32>) -> Result<String, String> {
        Err("this sbxw was built without voice (cargo build --features voice)".into())
    }

    /// Drop what nobody used for [`IDLE_UNLOAD`], each on its own clock.
    fn schedule_unload(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(IDLE_UNLOAD + Duration::from_secs(1)).await;
            let idle = |t: Option<Instant>| t.is_none_or(|t| t.elapsed() >= IDLE_UNLOAD);
            let mut rt = app.voice.lock().unwrap();
            #[cfg(feature = "voice")]
            if rt.stt.is_some() && idle(rt.stt_used) {
                rt.stt = None;
                tracing::info!("voice: recogniser unloaded after {}s idle", IDLE_UNLOAD.as_secs());
            }
            if rt.speech.is_some() && idle(rt.speech_used) {
                rt.speech = None;
                tracing::info!("voice: speech server stopped after {}s idle", IDLE_UNLOAD.as_secs());
            }
        });
    }
}

fn uvx() -> Result<PathBuf, String> {
    sbxw_core::sysbin::uvx().ok_or_else(|| "uv is not installed: reading aloud needs it (https://docs.astral.sh/uv/, or `brew install uv`)".into())
}

/// `uvx --from <runtime> mlx_audio.server`, ready for its arguments.
fn speech_command(uvx: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(uvx);
    c.args(["--from", voice::SPEECH_RUNTIME, "mlx_audio.server"]);
    c
}

/// Creates `dir` (and its parents) and makes it this user's only, mode 0700
/// on unix: the speech logs hold what was read aloud. A directory left by an
/// older build with default permissions is tightened too.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    builder.create(dir)
}

/// Start the server on a free local port and wait until it answers.
async fn start_speech_server(uvx: &Path, run_dir: &Path) -> Result<SpeechServer, String> {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()).map_err(|e| format!("no free port: {e}"))?.port();
    let logs = run_dir.join("speech-logs");
    private_dir(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
    let mut cmd = speech_command(uvx);
    cmd.args(["--host", "127.0.0.1", "--port", &port.to_string()])
        // Only this machine's pages, not any site the browser has open.
        .args(["--allowed-origins", "http://127.0.0.1", "http://localhost"])
        .arg("--log-dir")
        .arg(&logs)
        .current_dir(&logs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        // A failed load is a Python traceback, and only here.
        .stderr(std::fs::File::create(logs.join(SPEECH_LOG)).map(std::process::Stdio::from).unwrap_or_else(|_| std::process::Stdio::null()))
        .process_group(0)
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| format!("could not start the speech server: {e}"))?;
    let server = SpeechServer { port, log: logs.join(SPEECH_LOG), pid: child.id(), child: std::sync::Mutex::new(child) };
    let t = Instant::now();
    let health = server.url("/v1/models");
    loop {
        if curl_local(&[&health], None, Duration::from_secs(3)).await.is_ok() {
            return Ok(server);
        }
        if !server.alive() {
            return Err(format!("the speech server stopped at once (see {})", server.log.display()));
        }
        if t.elapsed() > SPEECH_START {
            return Err(format!("the speech server did not answer in {}s", SPEECH_START.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

/// One request to the speech server through `curl`: the response body, or
/// the server's own words for a failure. `body` goes in on stdin.
async fn curl_local(args: &[&str], body: Option<String>, limit: Duration) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(sbxw_core::sysbin::sys_bin("curl"))
        .args(["-sS", "--fail-with-body", "--noproxy", "*", "--max-time", &limit.as_secs().to_string()])
        .args(args)
        .stdin(if body.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if let (Some(body), Some(mut stdin)) = (body, child.stdin.take()) {
        stdin.write_all(body.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let said = String::from_utf8_lossy(if out.stdout.is_empty() { &out.stderr } else { &out.stdout });
    Err(said.trim().chars().take(300).collect())
}

/// A few sentences: the page splits a message into batches before asking for speech.
pub const MAX_SPEECH_CHARS: usize = 1_000;

pub fn check_speech(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("nothing to say".into());
    }
    if text.chars().count() > MAX_SPEECH_CHARS {
        return Err(format!("speak at most {MAX_SPEECH_CHARS} characters at a time"));
    }
    Ok(())
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
        .args(["--proto", "=https", "--proto-redir", "=https", "-fsSL", "--retry", "2", url])
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
    #[cfg(unix)]
    #[test]
    fn speech_log_dir_is_private_even_if_it_existed() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("sbxw-speech-logs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fresh = root.join("run").join("speech-logs");
        super::private_dir(&fresh).unwrap();
        assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o700);

        let old = root.join("old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::private_dir(&old).unwrap();
        assert_eq!(std::fs::metadata(&old).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::*;

    #[test]
    fn speech_is_a_sentence_or_so() {
        assert!(check_speech("Bonjour.").is_ok());
        assert!(check_speech("  ").is_err());
        assert!(check_speech(&"a".repeat(MAX_SPEECH_CHARS + 1)).is_err());
    }

    /// A stand-in for `uvx … mlx_audio.server`: checks how it is started,
    /// then answers the three requests the daemon makes.
    const FAKE_UVX: &str = r#"#!/bin/sh
[ "$1" = "--from" ] && [ "$3" = "mlx_audio.server" ] || exit 64
shift 3
exec /usr/bin/python3 -I - "$@" <<'PY'
import sys, json, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs
a = sys.argv[1:]
host, port = a[a.index("--host") + 1], int(a[a.index("--port") + 1])
assert "--allowed-origins" in a and "--log-dir" in a
loaded = []
class H(BaseHTTPRequestHandler):
    def log_message(self, *x): pass
    def send(self, code, body, kind="application/json"):
        self.send_response(code); self.send_header("content-type", kind)
        self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self): self.send(200, b'{"object":"list","data":[]}')
    def do_POST(self):
        u = urlparse(self.path)
        if u.path == "/v1/models":
            loaded.append(parse_qs(u.query)["model_name"][0]); return self.send(200, b"{}")
        req = json.loads(self.rfile.read(int(self.headers["content-length"])))
        if req["model"] not in loaded or req["response_format"] != "pcm" or not req["stream"]:
            return self.send(500, b'{"detail":"model not loaded"}')
        self.send_response(200); self.send_header("content-type", "audio/pcm"); self.end_headers()
        self.wfile.write((req["voice"] + "|" + req["input"]).encode()); self.wfile.flush()
        time.sleep(0.2); self.wfile.write(b"|end")
HTTPServer((host, port), H).serve_forever()
PY
"#;

    #[tokio::test]
    async fn the_speech_server_is_started_loaded_asked_and_stopped() {
        if !Path::new("/usr/bin/python3").is_file() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let uvx = d.path().join("uvx");
        std::fs::write(&uvx, FAKE_UVX).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&uvx, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let model = d.path().join("my models/voxtral");
        let server = start_speech_server(&uvx, &d.path().join("run")).await.unwrap();
        assert!(server.alive());
        let e = server.say(&model, "fr_male", "Salut.").await.err().unwrap();
        assert!(e.contains("model not loaded"), "the server's own words: {e}");
        server.load(&model).await.unwrap();
        let mut speech = server.say(&model, "fr_male", "Salut « toi ».").await.unwrap();
        let mut pcm = Vec::new();
        while let Some(piece) = speech.next().await {
            pcm.extend(piece);
        }
        let heard = String::from_utf8_lossy(&pcm);
        assert!(heard.starts_with("fr_male|Salut « toi »."), "the sound alone, headers gone, the text whole: {heard}");
        assert!(heard.ends_with("|end"), "every piece arrives: {heard}");

        let (pid, health) = (server.pid.unwrap(), server.url("/v1/models"));
        drop(server);
        let mut gone = false;
        for _ in 0..50 {
            gone = curl_local(&[&health], None, Duration::from_secs(1)).await.is_err();
            if gone {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(gone, "dropping the server stops its process group ({pid})");
    }

    #[tokio::test]
    async fn a_server_that_exits_is_reported_at_once() {
        let d = tempfile::tempdir().unwrap();
        let e = start_speech_server(Path::new("/usr/bin/false"), d.path()).await.err().unwrap();
        assert!(e.contains("stopped at once"), "{e}");
    }

    #[tokio::test]
    async fn a_failed_local_request_reports_what_was_said() {
        // Nothing listens on port 9: curl's own error comes back, not a panic.
        let e = curl_local(&["http://127.0.0.1:9/v1/models"], None, Duration::from_secs(2)).await.unwrap_err();
        assert!(!e.is_empty());
    }

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
