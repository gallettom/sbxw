//! The native engines behind `svc::voice`, compiled only with the `voice`
//! feature: sherpa-onnx (statically linked through `sherpa-rs`) for speech.
//!
//! Parakeet TDT is a NeMo transducer: sherpa-onnx reads it with
//! `model_type = "nemo_transducer"`, and it detects the language itself.

use std::path::Path;

pub struct Stt(sherpa_rs::transducer::TransducerRecognizer);

impl Stt {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let p = |f: &str| dir.join(f).to_string_lossy().into_owned();
        let threads = std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2) as i32;
        let cfg = sherpa_rs::transducer::TransducerConfig {
            encoder: p("encoder.int8.onnx"),
            decoder: p("decoder.int8.onnx"),
            joiner: p("joiner.int8.onnx"),
            tokens: p("tokens.txt"),
            model_type: "nemo_transducer".into(),
            num_threads: threads,
            sample_rate: crate::svc::voice::SAMPLE_RATE as i32,
            feature_dim: 128,
            decoding_method: "greedy_search".into(),
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let r = sherpa_rs::transducer::TransducerRecognizer::new(cfg).map_err(|e| format!("loading the speech model: {e}"))?;
        tracing::info!("voice: speech model loaded in {} ms", t.elapsed().as_millis());
        Ok(Stt(r))
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> String {
        self.0.transcribe(crate::svc::voice::SAMPLE_RATE, samples)
    }
}

// ── The conversation: llama.cpp ──────────────────────────────────────────
//
// One model in memory, a fresh context per reply: the whole conversation is
// re-read each turn (prompt processing is fast on Metal), which keeps no KV
// state to go stale between turns and lets the page own the transcript.

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::sync::OnceLock;

/// llama.cpp's backend is process-wide and initialised once.
fn backend() -> Result<&'static LlamaBackend, String> {
    static B: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    B.get_or_init(|| {
        let mut b = LlamaBackend::init().map_err(|e| e.to_string())?;
        b.void_logs();
        Ok(b)
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// One turn of the conversation, as the page sends it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

pub struct Llm {
    model: LlamaModel,
}

/// Context the model reads and writes in (Q24: 16k, ~1 GB more on an 8B).
pub const N_CTX: u32 = 16_384;
/// The longest spoken reply, and the longest brief, in tokens.
pub const MAX_REPLY: usize = 700;
pub const MAX_BRIEF: usize = 900;

impl Llm {
    pub fn load(gguf: &Path) -> Result<Self, String> {
        let t = std::time::Instant::now();
        // Every layer on the GPU where there is one (Metal); ignored on CPU.
        let params = LlamaModelParams::default().with_n_gpu_layers(999);
        let model = LlamaModel::load_from_file(backend()?, gguf, &params).map_err(|e| format!("loading the language model: {e}"))?;
        tracing::info!("voice: language model loaded in {} ms", t.elapsed().as_millis());
        Ok(Llm { model })
    }

    /// The prompt for `turns`: the model's own chat template, or Mistral's
    /// v7 format when llama.cpp does not recognise the template.
    fn prompt(&self, system: &str, turns: &[Turn]) -> String {
        let mut chat = vec![LlamaChatMessage::new("system".into(), system.into())];
        chat.extend(turns.iter().map(|t| LlamaChatMessage::new(t.role.clone(), t.content.replace('\0', ""))));
        let chat: Result<Vec<_>, _> = chat.into_iter().collect();
        if let (Ok(chat), Ok(tmpl)) = (chat, self.model.chat_template(None)) {
            if let Ok(p) = self.model.apply_chat_template(&tmpl, &chat, true) {
                return p;
            }
        }
        mistral_v7(system, turns)
    }

    /// Generate a reply, handing each piece of text to `emit` as it comes;
    /// `emit` returning false (the page went away) stops generation.
    pub fn reply(&self, system: &str, turns: &[Turn], max_reply: usize, temp: f32, mut emit: impl FnMut(&str) -> bool) -> Result<String, String> {
        let vocab = self.model.vocab();
        let mut turns = turns.to_vec();
        // Oldest exchanges go first when the conversation outgrows the
        // context (the rolling summary replaces this in a later lot).
        let tokens = loop {
            let text = self.prompt(system, &turns);
            let mut toks = vocab.tokenize(text.as_bytes(), false, true);
            if toks.first() != Some(&vocab.bos()) {
                toks.insert(0, vocab.bos());
            }
            if toks.len() + max_reply < N_CTX as usize || turns.len() <= 1 {
                break toks;
            }
            turns.drain(..turns.len().min(2));
        };
        if tokens.len() + max_reply >= N_CTX as usize {
            return Err("that message is too long for the conversation's context".into());
        }
        let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4);
        let cparams = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_batch(512)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = self.model.new_context(backend()?, cparams).map_err(|e| format!("context: {e}"))?;

        // The prompt, in batches of n_batch; logits only for its last token.
        let mut batch = LlamaBatch::new(512, 1);
        let last = tokens.len() - 1;
        for (chunk_i, chunk) in tokens.chunks(512).enumerate() {
            batch.clear();
            for (j, tok) in chunk.iter().enumerate() {
                let pos = chunk_i * 512 + j;
                batch.add(*tok, pos as i32, &[0], pos == last).map_err(|e| e.to_string())?;
            }
            ctx.decode(&mut batch).map_err(|e| format!("reading the prompt: {e}"))?;
        }

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::penalties(self.model.n_vocab(), 64, 1.1, 0.0, 0.0),
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.95, 1),
            LlamaSampler::min_p(0.05, 1),
            LlamaSampler::temp(temp),
            LlamaSampler::dist(rand_seed()),
        ]);
        let mut out = String::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut idx = batch.n_tokens() - 1;
        for pos in (tokens.len() as i32..).take(max_reply) {
            let tok = sampler.sample(&ctx, idx);
            sampler.accept(tok);
            if vocab.is_eog(tok) {
                break;
            }
            pending.extend(vocab.token_to_piece(tok, false, None));
            // Emit only whole UTF-8 characters: a token may end mid-character.
            let valid = match std::str::from_utf8(&pending) {
                Ok(s) => s.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid > 0 {
                let piece = String::from_utf8_lossy(&pending[..valid]).into_owned();
                pending.drain(..valid);
                out.push_str(&piece);
                if !emit(&piece) {
                    break;
                }
            }
            batch.clear();
            batch.add(tok, pos, &[0], true).map_err(|e| e.to_string())?;
            idx = 0;
            ctx.decode(&mut batch).map_err(|e| format!("generating: {e}"))?;
        }
        Ok(out)
    }
}

/// Mistral's v7 chat format (Ministral 3), for a template llama.cpp's
/// built-in matcher does not know.
pub fn mistral_v7(system: &str, turns: &[Turn]) -> String {
    let mut s = format!("<s>[SYSTEM_PROMPT]{system}[/SYSTEM_PROMPT]");
    for t in turns {
        if t.role == "assistant" {
            s.push_str(&t.content);
            s.push_str("</s>");
        } else {
            s.push_str(&format!("[INST]{}[/INST]", t.content));
        }
    }
    s
}

fn rand_seed() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

// ── The voice: sherpa-onnx TTS ───────────────────────────────────────────
//
// Kokoro (multilingual: a French and an English speaker, one engine per
// language since espeak's language is fixed at creation) or Piper's French
// siwis voice (one speaker, whatever the language).

/// Kokoro multi-lang v1.0 speaker ids: `ff_siwis`, `af_heart`.
pub const KOKORO_FR: i32 = 30;
pub const KOKORO_EN: i32 = 3;

pub enum Tts {
    Kokoro { fr: Option<sherpa_rs::tts::KokoroTts>, en: Option<sherpa_rs::tts::KokoroTts>, dir: std::path::PathBuf },
    /// The voice and which of its speakers.
    Piper(sherpa_rs::tts::VitsTts, i32),
}

/// Speech for one sentence or so.
pub struct Speech {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

fn need(dir: &Path, files: &[&str]) -> Result<(), String> {
    for f in files {
        if !dir.join(f).exists() {
            return Err(format!("the voice model is missing {f}"));
        }
    }
    Ok(())
}

fn tts_threads() -> i32 {
    std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2) as i32
}

impl Tts {
    pub fn load(model_id: &str, speaker: i32, dir: &Path) -> Result<Self, String> {
        let p = |f: &str| dir.join(f).to_string_lossy().into_owned();
        if model_id.starts_with("kokoro") {
            // Everything sherpa-onnx checks: it exit()s the whole process on
            // a missing one instead of returning an error.
            need(dir, &["model.onnx", "voices.bin", "tokens.txt", "lexicon-us-en.txt", "lexicon-zh.txt", "dict", "espeak-ng-data"])?;
            return Ok(Tts::Kokoro { fr: None, en: None, dir: dir.to_path_buf() });
        }
        // Piper: the repo's one .onnx file, whatever the voice is called.
        let onnx = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "onnx"))
            .ok_or("the voice model has no .onnx file")?;
        need(dir, &["tokens.txt", "espeak-ng-data"])?;
        let cfg = sherpa_rs::tts::VitsTtsConfig {
            model: onnx.to_string_lossy().into_owned(),
            tokens: p("tokens.txt"),
            data_dir: p("espeak-ng-data"),
            length_scale: 1.0,
            noise_scale: 0.667,
            noise_scale_w: 0.8,
            onnx_config: sherpa_rs::OnnxConfig { provider: "cpu".into(), debug: false, num_threads: tts_threads() },
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let v = sherpa_rs::tts::VitsTts::new(cfg);
        tracing::info!("voice: Piper loaded in {} ms", t.elapsed().as_millis());
        Ok(Tts::Piper(v, speaker))
    }

    fn kokoro(dir: &Path, lang: &str) -> sherpa_rs::tts::KokoroTts {
        let p = |f: &str| dir.join(f).to_string_lossy().into_owned();
        let t = std::time::Instant::now();
        let k = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
            model: p("model.onnx"),
            voices: p("voices.bin"),
            tokens: p("tokens.txt"),
            data_dir: p("espeak-ng-data"),
            // Required together by a multilingual Kokoro; French goes through
            // espeak-ng (`lang`), the lexicons serve English and Chinese.
            lexicon: format!("{},{}", p("lexicon-us-en.txt"), p("lexicon-zh.txt")),
            dict_dir: p("dict"),
            lang: if lang == "en" { "en-us".into() } else { "fr".into() },
            length_scale: 1.0,
            onnx_config: sherpa_rs::OnnxConfig { provider: "cpu".into(), debug: false, num_threads: tts_threads() },
            ..Default::default()
        });
        tracing::info!("voice: Kokoro ({lang}) loaded in {} ms", t.elapsed().as_millis());
        k
    }

    /// `lang` is `fr` or `en`; Piper speaks French either way.
    pub fn speak(&mut self, text: &str, lang: &str, speed: f32) -> Result<Speech, String> {
        let audio = match self {
            Tts::Kokoro { fr, en, dir } => {
                let (slot, sid) = if lang == "en" { (en, KOKORO_EN) } else { (fr, KOKORO_FR) };
                let engine = slot.get_or_insert_with(|| Self::kokoro(dir, lang));
                engine.create(text, sid, speed)
            }
            Tts::Piper(v, sid) => v.create(text, *sid, speed),
        }
        .map_err(|e| format!("speaking: {e}"))?;
        Ok(Speech { samples: audio.samples, sample_rate: audio.sample_rate })
    }
}
