//! Sample-rate conversion and downmixing, on the control thread (never in a
//! device callback: it allocates).
//!
//! [`Resampler`] is a streaming windowed-sinc interpolator for any pair of
//! rates. The kernel is a sinc low-pass (cut at 85 % of the lower Nyquist
//! frequency when going down, 90 % of the input's when going up) under a
//! Blackman window 48 zero crossings wide (tabulated, 512 phases per sample,
//! linear interpolation), so going from 48 kHz to 16 kHz anything that would
//! fold back is attenuated by at least 60 dB (tested). Weights are
//! normalised per output sample, so DC passes exactly. Cost at 48 kHz to
//! 16 kHz: about 170 multiply-adds per output sample.
//!
//! Output length is exact: `n` input samples give `round(n * out / in)`
//! output samples once [`Resampler::finish`] is called, whatever the chunks.

use std::f64::consts::PI;

/// Zero crossings of the sinc kept on each side of the centre.
const ZERO_CROSSINGS: f64 = 24.0;

pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    /// Input samples per output sample.
    step: f64,
    /// Kernel half width, in input samples.
    half: usize,
    /// Input not yet consumed; `buf[0]` is input sample `base`.
    buf: Vec<f32>,
    base: i64,
    /// Input samples received, outputs made.
    n_in: u64,
    n_out: u64,
    /// The kernel sampled [`PHASES`] times per input sample over
    /// `[-half, half]`, read with linear interpolation.
    table: Vec<f32>,
}

/// Kernel samples per input sample in the table: interpolation error is far
/// below 16-bit resolution.
const PHASES: usize = 512;

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Resampler {
        assert!(in_rate > 0 && out_rate > 0, "sample rates must be positive");
        let step = in_rate as f64 / out_rate as f64;
        let cutoff = if out_rate < in_rate { 0.85 * out_rate as f64 / in_rate as f64 } else { 0.9 };
        let half = (ZERO_CROSSINGS / cutoff).ceil() as usize;
        let table = if in_rate == out_rate {
            Vec::new()
        } else {
            (0..=2 * half * PHASES + 1).map(|i| kernel(cutoff, half, i as f64 / PHASES as f64 - half as f64) as f32).collect()
        };
        // Input before the first sample counts as silence.
        Resampler { in_rate, out_rate, step, half, buf: vec![0.0; half], base: -(half as i64), n_in: 0, n_out: 0, table }
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    fn passthrough(&self) -> bool {
        self.in_rate == self.out_rate
    }

    /// Outputs owed for the input so far, once it is all known.
    fn total_out(&self) -> u64 {
        ((self.n_in as u128 * self.out_rate as u128 + self.in_rate as u128 / 2) / self.in_rate as u128) as u64
    }

    /// Feed `input`, append what can be computed to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.n_in += input.len() as u64;
        if self.passthrough() {
            out.extend_from_slice(input);
            self.n_out += input.len() as u64;
            return;
        }
        self.buf.extend_from_slice(input);
        self.run(out, u64::MAX);
    }

    /// The input has ended: append the last outputs (the kernel's tail sees
    /// silence after the end).
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.passthrough() {
            return;
        }
        let total = self.total_out();
        self.buf.extend(std::iter::repeat_n(0.0, self.half + self.step.ceil() as usize + 1));
        self.run(out, total);
    }

    fn run(&mut self, out: &mut Vec<f32>, limit: u64) {
        let half = self.half as i64;
        while self.n_out < limit {
            // Position of this output on the input's time line.
            let t = self.n_out as f64 * self.step;
            let centre = t.floor() as i64;
            let last = centre + half;
            if last - self.base >= self.buf.len() as i64 {
                break;
            }
            let first = centre - half + 1;
            let mut acc = 0.0f64;
            let mut weights = 0.0f64;
            for j in first..=last {
                let x = t - j as f64;
                let w = self.weight(x);
                acc += w * self.buf[(j - self.base) as usize] as f64;
                weights += w;
            }
            out.push(if weights != 0.0 { (acc / weights) as f32 } else { 0.0 });
            self.n_out += 1;
        }
        // Drop input no later output can reach.
        let next_first = (self.n_out as f64 * self.step).floor() as i64 - half + 1;
        let drop = (next_first - self.base).clamp(0, self.buf.len() as i64) as usize;
        if drop > 0 {
            self.buf.drain(..drop);
            self.base += drop as i64;
        }
    }

    fn weight(&self, x: f64) -> f64 {
        let pos = (x + self.half as f64) * PHASES as f64;
        if pos < 0.0 {
            return 0.0;
        }
        let i = pos as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let f = pos - i as f64;
        self.table[i] as f64 * (1.0 - f) + self.table[i + 1] as f64 * f
    }
}

/// Windowed sinc: `cutoff` is a fraction of the input Nyquist frequency,
/// `half` the half width in input samples.
fn kernel(cutoff: f64, half: usize, x: f64) -> f64 {
    let half = half as f64;
    if x.abs() >= half {
        return 0.0;
    }
    let a = PI * cutoff * x;
    let sinc = if a.abs() < 1e-12 { 1.0 } else { a.sin() / a };
    let u = x / half;
    let window = 0.42 + 0.5 * (PI * u).cos() + 0.08 * (2.0 * PI * u).cos();
    sinc * window
}

/// One shot: the whole of `input` at `in_rate`, as `out_rate`.
pub fn resample(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    let mut r = Resampler::new(in_rate, out_rate);
    let mut out = Vec::with_capacity((input.len() as u64 * out_rate as u64 / in_rate as u64) as usize + 8);
    r.process(input, &mut out);
    r.finish(&mut out);
    out
}

/// Interleaved frames of `channels` samples, averaged to one channel,
/// written to `out` (as many frames as fit). Returns the frames written.
/// Allocation-free: callable from a device callback.
pub fn downmix_into(interleaved: &[f32], channels: usize, out: &mut [f32]) -> usize {
    if channels <= 1 {
        let n = interleaved.len().min(out.len());
        out[..n].copy_from_slice(&interleaved[..n]);
        return n;
    }
    let frames = (interleaved.len() / channels).min(out.len());
    let scale = 1.0 / channels as f32;
    for (o, frame) in out[..frames].iter_mut().zip(interleaved.chunks_exact(channels)) {
        *o = frame.iter().sum::<f32>() * scale;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, n: usize, amp: f64) -> Vec<f32> {
        (0..n).map(|i| (amp * (2.0 * PI * freq * i as f64 / rate as f64).sin()) as f32).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// Compare `got` with the ideal sine at the output rate, away from the edges.
    fn max_error(got: &[f32], freq: f64, rate: u32, amp: f64, skip: usize) -> f64 {
        let want = sine(freq, rate, got.len(), amp);
        got[skip..got.len() - skip].iter().zip(&want[skip..]).map(|(a, b)| (*a as f64 - *b as f64).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn a_1khz_sine_at_48k_becomes_the_same_sine_at_16k() {
        let input = sine(1000.0, 48_000, 48_000, 0.5);
        let out = resample(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        let err = max_error(&out, 1000.0, 16_000, 0.5, 200);
        assert!(err < 2e-3, "max error {err}");
        let ratio = rms(&out[200..15_800]) / rms(&input[600..47_400]);
        assert!((ratio - 1.0).abs() < 0.01, "energy ratio {ratio}");
    }

    #[test]
    fn a_440hz_sine_at_44k1_becomes_the_same_sine_at_16k() {
        let input = sine(440.0, 44_100, 44_100 * 2, 0.8);
        let out = resample(&input, 44_100, 16_000);
        assert_eq!(out.len(), 32_000);
        assert!(max_error(&out, 440.0, 16_000, 0.8, 200) < 3e-3);
    }

    #[test]
    fn speech_at_24k_goes_up_to_48k_and_44k1() {
        let input = sine(300.0, 24_000, 24_000, 0.5);
        let up = resample(&input, 24_000, 48_000);
        assert_eq!(up.len(), 48_000);
        assert!(max_error(&up, 300.0, 48_000, 0.5, 400) < 2e-3);
        let odd = resample(&input, 24_000, 44_100);
        assert_eq!(odd.len(), 44_100);
        assert!(max_error(&odd, 300.0, 44_100, 0.5, 400) < 2e-3);
    }

    #[test]
    fn what_cannot_be_represented_at_16k_is_removed_not_folded() {
        // 10 kHz is above 16 kHz's Nyquist: it would alias to 6 kHz.
        let input = sine(10_000.0, 48_000, 48_000, 0.5);
        let out = resample(&input, 48_000, 16_000);
        let level = rms(&out[200..15_800]);
        assert!(level < 0.5 * 0.707 * 0.001, "alias level {level}");
    }

    #[test]
    fn chunks_of_any_size_give_the_same_samples_as_one_shot() {
        let input = sine(700.0, 48_000, 20_000, 0.3);
        let whole = resample(&input, 48_000, 16_000);
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        let mut i = 0;
        let mut k = 1;
        while i < input.len() {
            let n = (k * 37 % 997).min(input.len() - i);
            r.process(&input[i..i + n], &mut out);
            i += n;
            k += 1;
        }
        r.finish(&mut out);
        assert_eq!(out.len(), whole.len());
        assert!(out.iter().zip(&whole).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn equal_rates_pass_through_and_short_input_has_exact_length() {
        let x = [0.1, 0.2, 0.3];
        assert_eq!(resample(&x, 16_000, 16_000), x.to_vec());
        assert_eq!(resample(&[0.5; 7], 48_000, 16_000).len(), 2);
        assert_eq!(resample(&[], 48_000, 16_000).len(), 0);
        // DC stays DC.
        let dc = resample(&[0.25; 4800], 48_000, 16_000);
        assert!(dc[100..1500].iter().all(|v| (v - 0.25).abs() < 1e-4));
    }

    #[test]
    fn downmix_averages_each_frame() {
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        let mut out = [9.0; 3];
        assert_eq!(downmix_into(&stereo, 2, &mut out), 3);
        assert_eq!(out, [0.5, 0.5, 0.0]);
        let mut short = [0.0; 2];
        assert_eq!(downmix_into(&stereo, 2, &mut short), 2);
        let quad = [0.4, 0.4, 0.4, 0.4];
        let mut one = [0.0];
        downmix_into(&quad, 4, &mut one);
        assert!((one[0] - 0.4).abs() < 1e-6);
        let mut mono = [0.0; 2];
        assert_eq!(downmix_into(&[0.3, 0.6], 1, &mut mono), 2);
        assert_eq!(mono, [0.3, 0.6]);
    }
}
