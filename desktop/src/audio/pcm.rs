//! The two wire formats of the daemon, and the loudness meter.
//!
//! * Capture, to `POST /api/voice/transcribe`: little-endian `f32` samples,
//!   mono, 16 kHz, no header (crates/daemon/src/http/ui.rs:1863-1866,
//!   crates/daemon/src/svc/voice.rs:771-783).
//! * Playback, from `POST /api/voice/speak`: little-endian signed 16-bit
//!   samples, mono, the rate in `x-sample-rate` (ui.rs:1891-1901), streamed
//!   in pieces that may cut a sample in two (concierge.js:829-831 carries the
//!   odd byte over; so does [`S16Decoder`]).

/// `f32` samples as the bytes `/api/voice/transcribe` takes.
pub fn f32_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// The reverse of [`f32_le_bytes`] (tests and the demo). A trailing partial
/// sample is ignored.
pub fn f32_from_le_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// `f32` samples in `[-1, 1]` as 16-bit LE PCM, clipped (the demo plays a
/// recording back through the same path as speech).
pub fn s16_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Streaming 16-bit LE decoder: bytes in as they arrive, `f32` out.
#[derive(Default)]
pub struct S16Decoder {
    odd: Option<u8>,
}

impl S16Decoder {
    /// Decode `bytes`, appending to `out`; a last odd byte waits for the next piece.
    pub fn push(&mut self, mut bytes: &[u8], out: &mut Vec<f32>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(lo) = self.odd.take() {
            out.push(i16::from_le_bytes([lo, bytes[0]]) as f32 / 32768.0);
            bytes = &bytes[1..];
        }
        let (pairs, rest) = bytes.as_chunks::<2>();
        if let [last] = rest {
            self.odd = Some(*last);
        }
        out.extend(pairs.iter().map(|p| i16::from_le_bytes(*p) as f32 / 32768.0));
    }

    /// Whether half a sample is left over (an odd-length body).
    pub fn has_partial(&self) -> bool {
        self.odd.is_some()
    }
}

/// A loudness for a meter, 0.0 to 1.0, from an RMS value: -60 dBFS and
/// below is 0, full scale is 1, linear in decibels in between.
pub fn meter(rms: f32) -> f32 {
    if rms <= 1e-6 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Peak to meter value (the playback side measures peaks in the callback).
pub fn meter_peak(peak: f32) -> f32 {
    // A sine's RMS is its peak / sqrt(2); keep both meters on one scale.
    meter(peak * std::f32::consts::FRAC_1_SQRT_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_bytes_are_little_endian_and_round_trip() {
        let b = f32_le_bytes(&[0.5, -1.0]);
        // The daemon's own test vector (svc/voice.rs samples_are_little_endian_f32_and_bounded).
        assert_eq!(b, [0.5f32.to_le_bytes(), (-1.0f32).to_le_bytes()].concat());
        assert_eq!(&b[..4], &[0x00, 0x00, 0x00, 0x3f]);
        assert_eq!(f32_from_le_bytes(&b), vec![0.5, -1.0]);
    }

    #[test]
    fn s16_decodes_across_pieces_that_cut_a_sample() {
        let samples: Vec<i16> = vec![0, 1, -1, i16::MAX, i16::MIN, 12345];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let want: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        for cut in 0..bytes.len() {
            let mut d = S16Decoder::default();
            let mut out = Vec::new();
            d.push(&bytes[..cut], &mut out);
            d.push(&bytes[cut..], &mut out);
            assert_eq!(out, want, "cut at {cut}");
            assert!(!d.has_partial());
        }
        // One byte at a time.
        let mut d = S16Decoder::default();
        let mut out = Vec::new();
        for b in &bytes {
            d.push(std::slice::from_ref(b), &mut out);
        }
        assert_eq!(out, want);
    }

    #[test]
    fn s16_encode_then_decode_is_within_one_step() {
        let x = [0.0f32, 0.25, -0.5, 0.999, -1.0, 2.0];
        let mut out = Vec::new();
        S16Decoder::default().push(&s16_le_bytes(&x), &mut out);
        for (a, b) in x.iter().zip(&out) {
            assert!((a.clamp(-1.0, 1.0) - b).abs() <= 1.0 / 16384.0, "{a} {b}");
        }
    }

    #[test]
    fn the_meter_is_logarithmic_and_bounded() {
        assert_eq!(meter(0.0), 0.0);
        assert_eq!(meter(1.0), 1.0);
        assert!((meter(0.001) - 0.0).abs() < 1e-6);
        assert!((meter(0.0316) - 0.5).abs() < 0.01);
        assert_eq!(meter(5.0), 1.0);
    }
}
