//! The voice concierge's models: which ones, where they come from, and whether
//! they are on disk. Plain data and pure functions — the daemon downloads.
//!
//! Two local models, no language model: speech becomes text (Parakeet, in the
//! daemon through sherpa-onnx) and an agent's text becomes speech (Voxtral
//! TTS, in a local `mlx-audio` server the daemon starts on Apple Silicon).
//! What is said goes to an agent as it was said, once the person has read it.
//!
//! Three rules shape it (see `.sbxw-artifacts/specs/concierge-vocal.md`):
//!
//! * **Nothing is downloaded on its own.** Several GB is a decision: `sbxw
//!   voice install` or the UI's button, never a first use.
//! * **Checksums are the host's.** A model is a Hugging Face repo at a revision;
//!   its tree listing carries each LFS file's sha256, so a re-uploaded file needs
//!   no re-pin. Only the revision is pinned; `main` is followed until then.
//! * **A model is installed when every file of its plan is present at its
//!   size** and the marker the installer writes after verifying is there.
//!   Half a download is never "installed".

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a model does in the pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Speech to text.
    Stt,
    /// Text to speech.
    Tts,
}

/// One downloadable model: a Hugging Face repo, at a revision, filtered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Model {
    /// Stable, used as the directory name under `~/.sbxw/models`.
    pub id: &'static str,
    pub role: Role,
    pub label: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    /// Paths (or path prefixes ending in `/`) to keep from the repo tree.
    /// Empty keeps everything except the repo's own docs.
    pub include: &'static [&'static str],
    /// The licence, shown before downloading.
    pub license: &'static str,
    /// Roughly what it weighs, for the confirmation before any listing is
    /// fetched; the plan's own sizes are what is downloaded and checked.
    pub approx_mb: u32,
}

/// Every model sbxw knows how to fetch. `[voice]` in sbxw.toml picks among
/// them by id.
pub const CATALOG: &[Model] = &[
    Model {
        id: "parakeet-tdt-0.6b-v3-int8",
        role: Role::Stt,
        label: "Parakeet TDT 0.6B v3 (int8)",
        repo: "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        revision: "main",
        include: &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
        license: "CC-BY-4.0",
        approx_mb: 670,
    },
    // Voxtral TTS (Mistral, 4B), converted for MLX: one model, three sizes.
    // French and English in the same sentence, which a French-only voice
    // reads badly. Non-commercial licence: personal use.
    Model {
        id: "voxtral-tts-4bit",
        role: Role::Tts,
        label: "Voxtral TTS 4B (MLX, 4-bit)",
        repo: "mlx-community/Voxtral-4B-TTS-2603-mlx-4bit",
        revision: "main",
        include: &[],
        license: "CC-BY-NC-4.0",
        approx_mb: 2550,
    },
    Model {
        id: "voxtral-tts-6bit",
        role: Role::Tts,
        label: "Voxtral TTS 4B (MLX, 6-bit)",
        repo: "mlx-community/Voxtral-4B-TTS-2603-mlx-6bit",
        revision: "main",
        include: &[],
        license: "CC-BY-NC-4.0",
        approx_mb: 3500,
    },
    Model {
        id: "voxtral-tts-bf16",
        role: Role::Tts,
        label: "Voxtral TTS 4B (MLX, full precision)",
        repo: "mlx-community/Voxtral-4B-TTS-2603-mlx-bf16",
        revision: "main",
        include: &[],
        license: "CC-BY-NC-4.0",
        approx_mb: 8040,
    },
];

pub fn model(id: &str) -> Option<&'static Model> {
    CATALOG.iter().find(|m| m.id == id)
}

/// The voices Voxtral TTS ships (`voice_embedding/<name>.safetensors`).
pub const VOICES: &[&str] = &[
    "fr_male", "fr_female", "casual_male", "casual_female", "cheerful_female", "neutral_male", "neutral_female",
    "es_male", "es_female", "de_male", "de_female", "it_male", "it_female", "pt_male", "pt_female",
    "nl_male", "nl_female", "ar_male", "hi_male", "hi_female",
];

/// `[voice]` in sbxw.toml, read from the daemon's config: one concierge per
/// host, not per project. Keys of an older sbxw (`llm`, `retention_days`) are
/// ignored.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct VoiceSettings {
    pub stt: String,
    pub tts: String,
    /// Which of [`VOICES`] reads aloud.
    pub tts_voice: String,
    /// The push-to-talk key, as a `KeyboardEvent.code`.
    pub push_to_talk_key: String,
    /// Held, it dictates into the focused terminal instead.
    pub dictate_key: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            stt: "parakeet-tdt-0.6b-v3-int8".into(),
            // The middle size: 3.5 GB in memory while it speaks.
            tts: "voxtral-tts-6bit".into(),
            tts_voice: "fr_male".into(),
            push_to_talk_key: "F8".into(),
            dictate_key: "F9".into(),
        }
    }
}

impl VoiceSettings {
    pub fn validate(&self) -> Result<(), String> {
        for (role, id) in [(Role::Stt, &self.stt), (Role::Tts, &self.tts)] {
            match model(id) {
                Some(m) if m.role == role => {}
                Some(_) => return Err(format!("[voice] '{id}' is not a {role:?} model")),
                None => {
                    let known: Vec<_> = CATALOG.iter().filter(|m| m.role == role).map(|m| m.id).collect();
                    return Err(format!("[voice] unknown model '{id}' (known: {})", known.join(", ")));
                }
            }
        }
        if !VOICES.contains(&self.tts_voice.as_str()) {
            return Err(format!("[voice] unknown tts_voice '{}' (known: {})", self.tts_voice, VOICES.join(", ")));
        }
        if self.push_to_talk_key.trim().is_empty() || self.dictate_key.trim().is_empty() {
            return Err("[voice] push_to_talk_key and dictate_key must be set".into());
        }
        if self.push_to_talk_key == self.dictate_key {
            return Err("[voice] push_to_talk_key and dictate_key must differ".into());
        }
        Ok(())
    }

    /// The models this configuration needs, in pipeline order.
    pub fn models(&self) -> Vec<&'static Model> {
        [self.stt.as_str(), self.tts.as_str()].into_iter().filter_map(model).collect()
    }
}

// ── The speech server ─────────────────────────────────────────────────────

/// What runs Voxtral: `mlx-audio`'s OpenAI-compatible server, fetched and
/// run by `uvx` at this version (MLX: Apple Silicon only). `tts` brings
/// `mistral-common`, which Voxtral's tokenizer needs: without it the server
/// starts and then fails to load the model.
pub const SPEECH_RUNTIME: &str = "mlx-audio[server,tts]==0.6.5";

/// Whether this machine can run [`SPEECH_RUNTIME`].
pub fn speech_supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// What Voxtral speaks at: its sound is 16-bit mono PCM at this rate.
pub const SPEECH_RATE: u32 = 24_000;

/// Seconds of sound per piece of a streamed answer: the first one is heard
/// after about this much has been made.
pub const SPEECH_PIECE_SECS: f32 = 1.0;

/// The body of `POST /v1/audio/speech` for `text`: `model` is the installed
/// model's directory, the answer raw PCM ([`SPEECH_RATE`]) sent as it is made.
pub fn speech_request(model: &Path, voice: &str, text: &str) -> String {
    serde_json::json!({
        "model": model.to_string_lossy(),
        "input": text,
        "voice": voice,
        "response_format": "pcm",
        "stream": true,
        "streaming_interval": SPEECH_PIECE_SECS,
        // In 80 ms frames: 200 s, above what the longest batch needs once
        // its symbols are said in words. The server stops at 96 s otherwise.
        "max_tokens": 2500,
    })
    .to_string()
}

/// Where the body of an HTTP response starts in `buf` (headers included, as
/// `curl -i` prints them) and its status, once the headers are whole. A
/// `100 Continue` block before the real one is skipped.
pub fn response_head(buf: &[u8]) -> Option<(u16, usize)> {
    let mut at = 0;
    loop {
        let end = at + buf[at..].windows(4).position(|w| w == b"\r\n\r\n")? + 4;
        let head = String::from_utf8_lossy(&buf[at..end]);
        let status: u16 = head.split_whitespace().nth(1)?.parse().ok()?;
        if status >= 200 {
            return Some((status, end));
        }
        at = end;
    }
}

// ── Plans ─────────────────────────────────────────────────────────────────

/// One file to fetch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlannedFile {
    /// Relative to the model's directory (and to the repo root).
    pub path: String,
    pub size: u64,
    /// Lowercase hex; `None` for a small non-LFS file, checked by size only.
    pub sha256: Option<String>,
    pub url: String,
}

/// An entry of `GET /api/models/<repo>/tree/<rev>?recursive=1`.
#[derive(Deserialize, Debug)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    lfs: Option<Lfs>,
}

#[derive(Deserialize, Debug)]
struct Lfs {
    oid: String,
    size: u64,
}

pub const HF: &str = "https://huggingface.co";

/// Where the tree of `m` is listed.
pub fn tree_url(m: &Model) -> String {
    format!("{HF}/api/models/{}/tree/{}?recursive=1", m.repo, m.revision)
}

fn wanted(m: &Model, path: &str) -> bool {
    if m.include.is_empty() {
        let lower = path.to_ascii_lowercase();
        return !(lower.starts_with('.') || lower.ends_with(".md"));
    }
    m.include.iter().any(|inc| match inc.strip_suffix('/') {
        Some(dir) => path.starts_with(dir) && path[dir.len()..].starts_with('/'),
        None => path == *inc,
    })
}

/// A repo path as a URL path: percent-encode everything but RFC 3986 unreserved
/// characters and `/`. A name with a space or a `!` is refused raw by curl.
fn url_path(path: &str) -> String {
    url_encode(path, true)
}

/// A value for a URL's query string: `/` is encoded too.
pub fn url_component(value: &str) -> String {
    url_encode(value, false)
}

fn url_encode(path: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'/' if keep_slash => out.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A path from the remote listing that is safe to join under a directory:
/// relative, no `..`, no empty or `.` segments.
fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// Files to fetch for `m`, from the tree JSON. Every exact `include` must be
/// listed: a renamed upstream file is an error here, not a model that fails to
/// load later.
pub fn plan(m: &Model, tree_json: &str) -> Result<Vec<PlannedFile>, String> {
    let entries: Vec<TreeEntry> =
        serde_json::from_str(tree_json).map_err(|e| format!("{}: unreadable file listing: {e}", m.repo))?;
    let mut files: Vec<PlannedFile> = entries
        .into_iter()
        .filter(|e| e.kind == "file" && wanted(m, &e.path))
        .map(|e| {
            if !safe_rel(&e.path) {
                return Err(format!("{}: refusing the path '{}'", m.repo, e.path));
            }
            let (size, sha256) = match e.lfs {
                Some(l) => (l.size, Some(l.oid.to_ascii_lowercase())),
                None => (e.size, None),
            };
            Ok(PlannedFile {
                url: format!("{HF}/{}/resolve/{}/{}", m.repo, m.revision, url_path(&e.path)),
                path: e.path,
                size,
                sha256,
            })
        })
        .collect::<Result<_, _>>()?;
    for inc in m.include.iter().filter(|i| !i.ends_with('/')) {
        if !files.iter().any(|f| f.path == *inc) {
            return Err(format!("{}: '{inc}' is not in the repo at '{}'", m.repo, m.revision));
        }
    }
    if files.is_empty() {
        return Err(format!("{}: nothing to download", m.repo));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

// ── On disk ───────────────────────────────────────────────────────────────

/// `~/.sbxw/models/<id>`.
pub fn model_dir(models_root: &Path, m: &Model) -> PathBuf {
    models_root.join(m.id)
}

/// Written last, after every file was verified: the plan that was installed.
pub const MARKER: &str = ".sbxw-installed.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase", tag = "state")]
pub enum Installed {
    /// Nothing there.
    Missing,
    /// Some bytes, no marker: an interrupted download (resumable).
    Partial { have: u64 },
    /// Marker present and every file it names is at its size.
    Ready { bytes: u64 },
}

/// What is on disk for `m` — size checks only, cheap enough for every status.
pub fn installed(models_root: &Path, m: &Model) -> Installed {
    let dir = model_dir(models_root, m);
    let marker = std::fs::read_to_string(dir.join(MARKER))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<MarkerFile>>(&t).ok());
    if let Some(list) = marker {
        // The catalog may have grown since install: a marker missing an included
        // file or folder is an install to complete.
        let covers = m.include.iter().all(|inc| match inc.strip_suffix('/') {
            Some(d) => list.iter().any(|f| f.path.starts_with(d) && f.path[d.len()..].starts_with('/')),
            None => list.iter().any(|f| f.path == *inc),
        });
        let ok = !list.is_empty()
            && covers
            && list.iter().all(|f| {
                safe_rel(&f.path) && std::fs::metadata(dir.join(&f.path)).map(|md| md.len() == f.size).unwrap_or(false)
            });
        if ok {
            return Installed::Ready {
                bytes: list.iter().map(|f| f.size).sum(),
            };
        }
    }
    let have = dir_bytes(&dir);
    if have == 0 {
        Installed::Missing
    } else {
        Installed::Partial { have }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MarkerFile {
    pub path: String,
    pub size: u64,
}

/// The marker's content for a verified plan.
pub fn marker_json(files: &[PlannedFile]) -> String {
    let list: Vec<MarkerFile> = files
        .iter()
        .map(|f| MarkerFile {
            path: f.path.clone(),
            size: f.size,
        })
        .collect();
    serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// What the CLI and the UI say when the voice is not installed.
pub const INSTALL_HINT: &str = "sbxw voice install";

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that keeps named files and folders of its repo.
    fn listed() -> &'static Model {
        static M: Model = Model {
            id: "listed",
            role: Role::Tts,
            label: "test",
            repo: "someone/listed",
            revision: "main",
            include: &["model.onnx", "voices.bin", "tokens.txt", "lexicon-us-en.txt", "lexicon-zh.txt", "dict/", "espeak-ng-data/"],
            license: "Apache-2.0",
            approx_mb: 360,
        };
        &M
    }

    const TREE: &str = r#"[
      {"type":"file","path":"README.md","size":10},
      {"type":"file","path":"model.onnx","size":134,"lfs":{"oid":"ABCDEF","size":330000000}},
      {"type":"file","path":"voices.bin","size":134,"lfs":{"oid":"0123","size":27000000}},
      {"type":"file","path":"tokens.txt","size":1000},
      {"type":"file","path":"lexicon-us-en.txt","size":5000},
      {"type":"directory","path":"espeak-ng-data","size":0},
      {"type":"file","path":"espeak-ng-data/fr_dict","size":40000},
      {"type":"file","path":"espeak-ng-data-old/x","size":1},
      {"type":"file","path":"lexicon-zh.txt","size":9}
    ]"#;

    #[test]
    fn plan_keeps_included_files_and_lfs_checksums() {
        let p = plan(listed(), TREE).unwrap();
        let paths: Vec<_> = p.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["espeak-ng-data/fr_dict", "lexicon-us-en.txt", "lexicon-zh.txt", "model.onnx", "tokens.txt", "voices.bin"]
        );
        let m = p.iter().find(|f| f.path == "model.onnx").unwrap();
        assert_eq!(m.size, 330000000, "the LFS size, not the pointer's");
        assert_eq!(m.sha256.as_deref(), Some("abcdef"));
        assert_eq!(p.iter().find(|f| f.path == "tokens.txt").unwrap().sha256, None);
        assert!(m.url.ends_with("/someone/listed/resolve/main/model.onnx"));
    }

    #[test]
    fn urls_encode_spaces_and_punctuation_but_keep_slashes() {
        assert_eq!(url_path("espeak-ng-data/voices/!v/Mr serious"), "espeak-ng-data/voices/%21v/Mr%20serious");
        assert_eq!(url_path("a/é+b"), "a/%C3%A9%2Bb");
        assert_eq!(url_component("/Users/a b/m"), "%2FUsers%2Fa%20b%2Fm");
        let tree = r#"[{"type":"file","path":"model.onnx","size":1},{"type":"file","path":"voices.bin","size":1},
          {"type":"file","path":"tokens.txt","size":1},{"type":"file","path":"lexicon-us-en.txt","size":1},
          {"type":"file","path":"lexicon-zh.txt","size":1},
          {"type":"file","path":"espeak-ng-data/voices/!v/Mr serious","size":1}]"#;
        let p = plan(listed(), tree).unwrap();
        let f = p.iter().find(|f| f.path.ends_with("serious")).unwrap();
        assert!(f.url.ends_with("/resolve/main/espeak-ng-data/voices/%21v/Mr%20serious"), "{}", f.url);
        assert_eq!(f.path, "espeak-ng-data/voices/!v/Mr serious", "on disk under its real name");
    }

    #[test]
    fn a_whole_repo_is_fetched_without_its_docs() {
        let m = model("voxtral-tts-6bit").unwrap();
        let tree = r#"[{"type":"file","path":"README.md","size":9},{"type":"file","path":".gitattributes","size":9},
          {"type":"file","path":"config.json","size":4140},
          {"type":"file","path":"model.safetensors","size":134,"lfs":{"oid":"AB","size":3400000000}},
          {"type":"file","path":"voice_embedding/fr_male.safetensors","size":596056}]"#;
        let paths: Vec<_> = plan(m, tree).unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(paths, ["config.json", "model.safetensors", "voice_embedding/fr_male.safetensors"]);
    }

    #[test]
    fn a_speech_request_names_the_local_model_and_asks_for_a_stream() {
        let v: serde_json::Value = serde_json::from_str(&speech_request(Path::new("/m/voxtral"), "fr_male", "Salut \"toi\".")).unwrap();
        assert_eq!(v["model"], "/m/voxtral");
        assert_eq!(v["input"], "Salut \"toi\".");
        assert_eq!(v["voice"], "fr_male");
        assert_eq!(v["response_format"], "pcm");
        assert_eq!(v["stream"], true);
        assert!(SPEECH_RUNTIME.contains("=="), "the runtime is pinned");
        assert!(SPEECH_RUNTIME.contains("tts"), "with Voxtral's own dependencies");
    }

    #[test]
    fn a_response_head_gives_the_status_and_where_the_body_starts() {
        let ok = b"HTTP/1.1 200 OK\r\ncontent-type: audio/pcm\r\n\r\n\x01\x02";
        assert_eq!(response_head(ok), Some((200, ok.len() - 2)));
        assert_eq!(response_head(&ok[..20]), None, "headers not whole yet");
        let continued = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 500 Internal Server Error\r\n\r\nboom";
        assert_eq!(response_head(continued), Some((500, continued.len() - 4)));
        assert_eq!(response_head(b"HTTP/1.1 100 Continue\r\n\r\n"), None);
        assert_eq!(response_head(b"not http\r\n\r\n"), None);
    }

    #[test]
    fn a_missing_exact_file_is_an_error() {
        let tree = r#"[{"type":"file","path":"model.onnx","size":1}]"#;
        let e = plan(listed(), tree).unwrap_err();
        assert!(e.contains("voices.bin"), "{e}");
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let m = Model {
            include: &[],
            ..listed().clone()
        };
        let tree = r#"[{"type":"file","path":"a/../evil","size":1}]"#;
        assert!(plan(&m, tree).unwrap_err().contains("refusing"));
    }

    #[test]
    fn installed_needs_the_marker_and_every_size() {
        let root = tempfile::tempdir().unwrap();
        let m = listed();
        assert_eq!(installed(root.path(), m), Installed::Missing);
        let dir = model_dir(root.path(), m);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tokens.txt"), b"abc").unwrap();
        assert_eq!(installed(root.path(), m), Installed::Partial { have: 3 });
        let files = vec![PlannedFile {
            path: "tokens.txt".into(),
            size: 3,
            sha256: None,
            url: String::new(),
        }];
        std::fs::write(dir.join(MARKER), marker_json(&files)).unwrap();
        assert!(
            matches!(installed(root.path(), m), Installed::Partial { .. }),
            "a marker missing files the catalog now includes is not ready"
        );
        let m = &Model { include: &["tokens.txt"], ..m.clone() };
        assert_eq!(installed(root.path(), m), Installed::Ready { bytes: 3 });
        std::fs::write(dir.join("tokens.txt"), b"abcd").unwrap();
        assert!(matches!(installed(root.path(), m), Installed::Partial { .. }), "a changed file is not ready");
    }

    #[test]
    fn settings_validate_roles_and_the_voice() {
        assert!(VoiceSettings::default().validate().is_ok());
        let bad_role = VoiceSettings {
            stt: "voxtral-tts-4bit".into(),
            ..Default::default()
        };
        assert!(bad_role.validate().unwrap_err().contains("not a Stt"));
        let gone = VoiceSettings {
            tts: "some-older-voice".into(),
            ..Default::default()
        };
        assert!(gone.validate().unwrap_err().contains("voxtral-tts-6bit"), "the error lists what exists");
        let voice = VoiceSettings {
            tts_voice: "pierre".into(),
            ..Default::default()
        };
        assert!(voice.validate().unwrap_err().contains("fr_male"));
        assert_eq!(VoiceSettings::default().models().len(), 2);
        let old: VoiceSettings = toml::from_str("llm = \"x\"\nretention_days = 7\ndictate_key = \"F10\"").unwrap();
        assert_eq!(old.dictate_key, "F10", "keys of the LLM era are ignored");
    }

}
