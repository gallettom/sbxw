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
