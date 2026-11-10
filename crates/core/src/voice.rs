//! The voice concierge's models: which ones, where they come from, and whether
//! they are on disk. Plain data and pure functions — the daemon downloads.
//!
//! Three rules shape it (see `.sbxw-artifacts/specs/concierge-vocal.md`):
//!
//! * **Nothing is downloaded on its own.** About 6 GB is a decision: `sbxw
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
    /// The conversation.
    Llm,
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
    /// For a voice with several speakers, which one (Piper's `upmc`: 1 is
    /// Pierre). Kokoro picks its speaker by language instead.
    pub speaker: i32,
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
        speaker: 0,
    },
    Model {
        id: "ministral-3-8b-instruct-q4",
        role: Role::Llm,
        label: "Ministral 3 8B Instruct (Q4_K_M)",
        repo: "mistralai/Ministral-3-8B-Instruct-2512-GGUF",
        revision: "main",
        include: &["Ministral-3-8B-Instruct-2512-Q4_K_M.gguf"],
        license: "Apache-2.0",
        approx_mb: 5200,
        speaker: 0,
    },
    Model {
        id: "ministral-3-3b-instruct-q4",
        role: Role::Llm,
        label: "Ministral 3 3B Instruct (Q4_K_M)",
        repo: "mistralai/Ministral-3-3B-Instruct-2512-GGUF",
        revision: "main",
        include: &["Ministral-3-3B-Instruct-2512-Q4_K_M.gguf"],
        license: "Apache-2.0",
        approx_mb: 2100,
        speaker: 0,
    },
    Model {
        id: "kokoro-multi-lang-v1_0",
        role: Role::Tts,
        label: "Kokoro 82M multilingual (voice ff_siwis)",
        repo: "csukuangfj/kokoro-multi-lang-v1_0",
        revision: "main",
        // A multilingual Kokoro (v1.0) won't start without its English and
        // Chinese lexicons and jieba's `dict/`, whatever the language; sherpa-onnx
        // exits the process over it.
        include: &["model.onnx", "voices.bin", "tokens.txt", "lexicon-us-en.txt", "lexicon-zh.txt", "dict/", "espeak-ng-data/"],
        license: "Apache-2.0",
        approx_mb: 360,
        speaker: 0,
    },
    Model {
        id: "piper-fr-siwis-medium",
        role: Role::Tts,
        label: "Piper fr_FR siwis (medium)",
        repo: "csukuangfj/vits-piper-fr_FR-siwis-medium",
        revision: "main",
        include: &["fr_FR-siwis-medium.onnx", "tokens.txt", "espeak-ng-data/"],
        license: "CC-BY-4.0",
        approx_mb: 90,
        speaker: 0,
    },
    // Male French voices (Piper, single language: English read with a French
    // accent).
    Model {
        id: "piper-fr-tom-medium",
        role: Role::Tts,
        label: "Piper fr_FR tom (medium, male)",
        repo: "csukuangfj/vits-piper-fr_FR-tom-medium",
        revision: "main",
        include: &["fr_FR-tom-medium.onnx", "tokens.txt", "espeak-ng-data/"],
        license: "CC-BY-4.0",
        approx_mb: 80,
        speaker: 0,
    },
    Model {
        id: "piper-fr-upmc-pierre",
        role: Role::Tts,
        label: "Piper fr_FR upmc, Pierre (medium, male)",
        repo: "csukuangfj/vits-piper-fr_FR-upmc-medium",
        revision: "main",
        include: &["fr_FR-upmc-medium.onnx", "tokens.txt", "espeak-ng-data/"],
        license: "CC-BY-4.0",
        approx_mb: 90,
        speaker: 1,
    },
    Model {
        id: "piper-fr-gilles-low",
        role: Role::Tts,
        label: "Piper fr_FR gilles (low, male)",
        repo: "csukuangfj/vits-piper-fr_FR-gilles-low",
        revision: "main",
        include: &["fr_FR-gilles-low.onnx", "tokens.txt", "espeak-ng-data/"],
        license: "CC-BY-4.0",
        approx_mb: 70,
        speaker: 0,
    },
];

pub fn model(id: &str) -> Option<&'static Model> {
    CATALOG.iter().find(|m| m.id == id)
}

/// `[voice]` in sbxw.toml, read from the daemon's config: one concierge per
/// host, not per project.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct VoiceSettings {
    pub stt: String,
    /// Empty: the platform default (8B on macOS, 3B elsewhere: no Metal, so a
    /// CPU-sized model).
    pub llm: String,
    pub tts: String,
    /// The push-to-talk key, as a `KeyboardEvent.code`.
    pub push_to_talk_key: String,
    /// Held, it dictates into the focused terminal instead.
    pub dictate_key: String,
    /// Conversations are kept this many days at most (never more than 7).
    pub retention_days: u32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            stt: "parakeet-tdt-0.6b-v3-int8".into(),
            llm: String::new(),
            // A male French voice, and Piper's VITS is quicker than Kokoro
            // on the CPU: the first words are heard sooner.
            tts: "piper-fr-tom-medium".into(),
            push_to_talk_key: "F8".into(),
            dictate_key: "F9".into(),
            retention_days: MAX_RETENTION_DAYS,
        }
    }
}

pub const MAX_RETENTION_DAYS: u32 = 7;

pub fn default_llm() -> &'static str {
    if cfg!(target_os = "macos") {
        "ministral-3-8b-instruct-q4"
    } else {
        "ministral-3-3b-instruct-q4"
    }
}

impl VoiceSettings {
    pub fn validate(&self) -> Result<(), String> {
        for (role, id) in [(Role::Stt, &self.stt), (Role::Llm, &self.llm), (Role::Tts, &self.tts)] {
            if id.is_empty() && role == Role::Llm {
                continue;
            }
            match model(id) {
                Some(m) if m.role == role => {}
                Some(_) => return Err(format!("[voice] '{id}' is not a {role:?} model")),
                None => {
                    let known: Vec<_> = CATALOG.iter().filter(|m| m.role == role).map(|m| m.id).collect();
                    return Err(format!("[voice] unknown model '{id}' (known: {})", known.join(", ")));
                }
            }
        }
        if self.retention_days == 0 || self.retention_days > MAX_RETENTION_DAYS {
            return Err(format!("[voice] retention_days must be 1–{MAX_RETENTION_DAYS}"));
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
        let llm = if self.llm.is_empty() { default_llm() } else { &self.llm };
        [self.stt.as_str(), llm, self.tts.as_str()].into_iter().filter_map(model).collect()
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
/// characters and `/`. espeak-ng data has names like `voices/!v/Mr serious`,
/// which curl refuses raw.
fn url_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
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

/// `fr` or `en`, for choosing a voice: French unless the text reads as
/// English (no French accents, more English function words than French).
pub fn guess_lang(text: &str) -> &'static str {
    if text.chars().any(|c| "éèêàâçùûôîïëœÉÈÀÇ".contains(c)) {
        return "fr";
    }
    const FR: &[&str] = &["le", "la", "les", "un", "une", "des", "est", "et", "je", "tu", "vous", "pour", "que", "qui", "pas", "de", "du", "ton", "ta"];
    const EN: &[&str] = &["the", "a", "an", "is", "are", "and", "you", "i", "to", "of", "for", "that", "it", "your", "what", "with"];
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    let count = |list: &[&str]| words.iter().filter(|w| list.contains(&w.as_str())).count();
    if count(EN) > count(FR) {
        "en"
    } else {
        "fr"
    }
}

/// Mono f32 samples as a 16-bit PCM WAV file, which every browser decodes.
pub fn wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// What the CLI and the UI say when the voice is not installed.
pub const INSTALL_HINT: &str = "sbxw voice install";

#[cfg(test)]
mod tests {
    use super::*;

    fn kokoro() -> &'static Model {
        model("kokoro-multi-lang-v1_0").unwrap()
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
        let p = plan(kokoro(), TREE).unwrap();
        let paths: Vec<_> = p.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["espeak-ng-data/fr_dict", "lexicon-us-en.txt", "lexicon-zh.txt", "model.onnx", "tokens.txt", "voices.bin"]
        );
        let m = p.iter().find(|f| f.path == "model.onnx").unwrap();
        assert_eq!(m.size, 330000000, "the LFS size, not the pointer's");
        assert_eq!(m.sha256.as_deref(), Some("abcdef"));
        assert_eq!(p.iter().find(|f| f.path == "tokens.txt").unwrap().sha256, None);
        assert!(m.url.ends_with("/csukuangfj/kokoro-multi-lang-v1_0/resolve/main/model.onnx"));
    }

    #[test]
    fn urls_encode_spaces_and_punctuation_but_keep_slashes() {
        assert_eq!(url_path("espeak-ng-data/voices/!v/Mr serious"), "espeak-ng-data/voices/%21v/Mr%20serious");
        assert_eq!(url_path("a/é+b"), "a/%C3%A9%2Bb");
        let tree = r#"[{"type":"file","path":"model.onnx","size":1},{"type":"file","path":"voices.bin","size":1},
          {"type":"file","path":"tokens.txt","size":1},{"type":"file","path":"lexicon-us-en.txt","size":1},
          {"type":"file","path":"lexicon-zh.txt","size":1},
          {"type":"file","path":"espeak-ng-data/voices/!v/Mr serious","size":1}]"#;
        let p = plan(kokoro(), tree).unwrap();
        let f = p.iter().find(|f| f.path.ends_with("serious")).unwrap();
        assert!(f.url.ends_with("/resolve/main/espeak-ng-data/voices/%21v/Mr%20serious"), "{}", f.url);
        assert_eq!(f.path, "espeak-ng-data/voices/!v/Mr serious", "on disk under its real name");
    }

    #[test]
    fn the_voice_follows_the_language() {
        assert_eq!(guess_lang("Tu veux donc un mode clair pour ton site."), "fr");
        assert_eq!(guess_lang("Je veux un mode clair"), "fr");
        assert_eq!(guess_lang("So you want a light mode for your site?"), "en");
        assert_eq!(guess_lang("OK"), "fr", "French by default");
    }

    #[test]
    fn wav_has_a_header_and_two_bytes_a_sample() {
        let w = wav(&[0.0, 1.0, -1.0], 24_000);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 24_000);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 32767);
    }

    #[test]
    fn a_missing_exact_file_is_an_error() {
        let tree = r#"[{"type":"file","path":"model.onnx","size":1}]"#;
        let e = plan(kokoro(), tree).unwrap_err();
        assert!(e.contains("voices.bin"), "{e}");
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let m = Model {
            include: &[],
            ..kokoro().clone()
        };
        let tree = r#"[{"type":"file","path":"a/../evil","size":1}]"#;
        assert!(plan(&m, tree).unwrap_err().contains("refusing"));
    }

    #[test]
    fn installed_needs_the_marker_and_every_size() {
        let root = tempfile::tempdir().unwrap();
        let m = kokoro();
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
    fn settings_validate_roles_and_retention() {
        assert!(VoiceSettings::default().validate().is_ok());
        let bad_role = VoiceSettings {
            stt: "kokoro-multi-lang-v1_0".into(),
            ..Default::default()
        };
        assert!(bad_role.validate().unwrap_err().contains("not a Stt"));
        let long = VoiceSettings {
            retention_days: 30,
            ..Default::default()
        };
        assert!(long.validate().is_err());
        assert_eq!(VoiceSettings::default().models().len(), 3);
    }
}
