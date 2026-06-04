//! Log-mel front-end for the Nemotron streaming encoder.
//!
//! This is a direct port of the `NeMo` / `parakeet-rs` preprocessing pipeline.
//! The chain is, in order:
//!
//! 1. Pre-emphasis filter (`y[0] = x[0]`, `y[i] = x[i] - 0.97 * x[i-1]`).
//! 2. Reflect-free zero padding of `N_FFT/2` samples front and back.
//! 3. Per-frame windowing with a **symmetric** Hann window applied to the
//!    first `WIN_LENGTH` samples, the remainder of the `N_FFT` buffer zero.
//! 4. Real FFT, then the **power** spectrum (`|X|^2`, via `norm_sqr`).
//! 5. Projection through a Slaney-normalised mel filterbank `[N_MELS, N_FFT/2+1]`.
//! 6. `ln(mel + 2^-24)` — `NeMo`'s additive `log_zero_guard`.
//!
//! Crucially the Nemotron path does **no** per-feature normalization; the raw
//! log-mel values are fed straight to the encoder. Getting any of these steps
//! wrong (window symmetry, power-vs-magnitude, the log guard, or the Slaney
//! norm) yields all-blank/garbage transcripts, so the math here mirrors the
//! reference and the Python golden harness exactly.

use crate::error::{Error, Result};
use ndarray::Array2;
use realfft::RealFftPlanner;
use std::f32::consts::PI;
use std::path::Path;

use crate::constants::{
    HOP_LENGTH, LOG_ZERO_GUARD, N_FFT, N_MELS, PREEMPH, SAMPLE_RATE, WIN_LENGTH,
};

/// Load a WAV file into mono `f32` samples in `[-1, 1]`.
///
/// Accepts 16-bit PCM or 32-bit float WAV. Multi-channel audio is downmixed to
/// mono by averaging channels. The sample rate is validated against
/// [`SAMPLE_RATE`]; resampling is the caller's responsibility.
///
/// # Errors
///
/// Returns [`crate::error::Error::Audio`] if the file cannot be opened or read,
/// if the format is unsupported, or if the sample rate does not match
/// [`SAMPLE_RATE`].
pub fn load_wav_mono<P: AsRef<Path>>(path: P) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    if spec.sample_rate as usize != SAMPLE_RATE {
        return Err(Error::Audio(format!(
            "expected {SAMPLE_RATE} Hz audio, got {} Hz — resample first",
            spec.sample_rate
        )));
    }

    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|s| f32::from(s) / 32_768.0))
            .collect::<std::result::Result<Vec<_>, _>>()?,
    };

    if channels > 1 {
        Ok(samples
            .chunks(channels)
            .map(|c| {
                // DSP downmix: channel count (≤ 8) fits exactly in f32.
                #[allow(clippy::cast_precision_loss)]
                let n = channels as f32;
                c.iter().sum::<f32>() / n
            })
            .collect())
    } else {
        Ok(samples)
    }
}

/// Apply a first-order pre-emphasis filter: `y[i] = x[i] - coef * x[i-1]`.
#[must_use]
pub fn apply_preemphasis(audio: &[f32], coef: f32) -> Vec<f32> {
    if audio.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(audio.len());
    out.push(audio[0]);
    for i in 1..audio.len() {
        out.push(audio[i] - coef * audio[i - 1]);
    }
    out
}

/// Symmetric Hann window of length `n` (`0.5 - 0.5*cos(2*pi*i / (n-1))`).
///
/// This matches `numpy.hanning` / `parakeet-rs`. It is *not* the periodic Hann
/// window used by some STFT libraries — the symmetric form is what the model
/// was trained on.
fn hann_window(n: usize) -> Vec<f32> {
    // DSP window: frame index i <= WIN_LENGTH = 400 fits within f32 mantissa.
    #[allow(clippy::cast_precision_loss)]
    (0..n)
        .map(|i| 0.5 - 0.5 * ((2.0 * PI * i as f32) / (n as f32 - 1.0)).cos())
        .collect()
}

/// Slaney mel scale, ported from librosa (`htk=false`).
const F_SP: f64 = 200.0 / 3.0;
const MIN_LOG_HZ: f64 = 1000.0;
const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
const LOG_STEP: f64 = 0.068_751_777_420_949_12;

fn hz_to_mel_slaney(hz: f64) -> f64 {
    if hz < MIN_LOG_HZ {
        hz / F_SP
    } else {
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / LOG_STEP
    }
}

fn mel_to_hz_slaney(mel: f64) -> f64 {
    if mel < MIN_LOG_MEL {
        mel * F_SP
    } else {
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * LOG_STEP).exp()
    }
}

/// Build a Slaney-normalised mel filterbank of shape `[n_mels, n_fft/2 + 1]`.
///
/// Equivalent to `librosa.filters.mel(sr, n_fft, n_mels, fmin=0, fmax=sr/2,
/// htk=False, norm="slaney")`.
///
/// # DSP casting notes
///
/// Intermediate arithmetic uses `f64` (matching Python/librosa precision).
/// The arguments `n_fft` <= 512, `n_mels` <= 128, `sample_rate` = 16 000
/// are small constants that fit exactly in `f64`. The final filterbank is
/// stored as `f32`, so `f64 as f32` narrowings are intentional precision
/// reductions that match the numpy/librosa f32 output the model was trained on.
#[must_use]
#[allow(clippy::cast_precision_loss)] // see DSP casting notes in doc comment
pub fn create_mel_filterbank(n_fft: usize, n_mels: usize, sample_rate: usize) -> Array2<f32> {
    let freq_bins = n_fft / 2 + 1;
    let mut filterbank = Array2::<f32>::zeros((n_mels, freq_bins));

    let fmax = sample_rate as f64 / 2.0;
    let mel_min = hz_to_mel_slaney(0.0);
    let mel_max = hz_to_mel_slaney(fmax);

    // n_mels + 2 mel-spaced centre frequencies (lower edge .. upper edge).
    let mel_points: Vec<f64> = (0..=n_mels + 1)
        .map(|i| mel_to_hz_slaney(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64))
        .collect();

    let fft_freqs: Vec<f64> = (0..freq_bins)
        .map(|i| i as f64 * sample_rate as f64 / n_fft as f64)
        .collect();

    let fdiff: Vec<f64> = mel_points.windows(2).map(|w| w[1] - w[0]).collect();

    for i in 0..n_mels {
        for (k, &freq) in fft_freqs.iter().enumerate() {
            let lower = (freq - mel_points[i]) / fdiff[i];
            let upper = (mel_points[i + 2] - freq) / fdiff[i + 1];
            #[allow(clippy::cast_possible_truncation)] // f64->f32 intentional; see doc
            {
                filterbank[[i, k]] = 0.0f64.max(lower.min(upper)) as f32;
            }
        }
    }

    // Slaney energy normalisation.
    for i in 0..n_mels {
        let enorm = 2.0 / (mel_points[i + 2] - mel_points[i]);
        for k in 0..freq_bins {
            #[allow(clippy::cast_possible_truncation)] // f64->f32 intentional; see doc
            {
                filterbank[[i, k]] *= enorm as f32;
            }
        }
    }

    filterbank
}

/// Short-time Fourier transform producing a **power** spectrogram
/// `[n_fft/2 + 1, num_frames]`.
///
/// We use a proper real FFT (`realfft` over `RustFFT`) rather than a naive DFT:
/// the model was trained on numerically-correct spectrograms and a naive DFT
/// produces wrong bins, which collapses the decoder to all-blank output.
///
/// # Errors
///
/// Returns [`crate::error::Error::Feature`] if the FFT plan fails.
fn stft(audio: &[f32], n_fft: usize, hop_length: usize, win_length: usize) -> Result<Array2<f32>> {
    let pad = n_fft / 2;
    let mut padded = vec![0.0f32; pad];
    padded.extend_from_slice(audio);
    padded.resize(padded.len() + pad, 0.0);

    if padded.len() < n_fft {
        // Not enough audio for a single frame.
        return Ok(Array2::zeros((n_fft / 2 + 1, 0)));
    }

    let window = hann_window(win_length);
    let num_frames = (padded.len() - n_fft) / hop_length + 1;
    let freq_bins = n_fft / 2 + 1;
    let mut spectrogram = Array2::<f32>::zeros((freq_bins, num_frames));

    let mut planner = RealFftPlanner::<f32>::new();
    let plan = planner.plan_fft_forward(n_fft);

    let mut input = vec![0.0f32; n_fft];
    let mut output = plan.make_output_vec();
    let mut scratch = plan.make_scratch_vec();

    for frame_idx in 0..num_frames {
        let start = frame_idx * hop_length;

        input.fill(0.0);
        let take = win_length.min(padded.len() - start);
        for i in 0..take {
            input[i] = padded[start + i] * window[i];
        }

        plan.process_with_scratch(&mut input, &mut output, &mut scratch)
            .map_err(|e| Error::Feature(format!("FFT failed: {e}")))?;

        for k in 0..freq_bins {
            // Power spectrum: |X|^2 == norm_sqr.
            spectrogram[[k, frame_idx]] = output[k].norm_sqr();
        }
    }

    Ok(spectrogram)
}

/// The reusable mel filterbank for the Nemotron front-end.
///
/// The filterbank is deterministic from the model constants, so it is built
/// once and reused for every chunk / utterance.
pub struct MelFrontend {
    mel_basis: Array2<f32>,
}

impl Default for MelFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl MelFrontend {
    /// Build the front-end with the Nemotron mel parameters.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mel_basis: create_mel_filterbank(N_FFT, N_MELS, SAMPLE_RATE),
        }
    }

    /// Compute the **un-normalised** log-mel spectrogram of `audio`.
    ///
    /// Output shape is `[N_MELS, num_frames]`. Returns an empty `[N_MELS, 0]`
    /// array for empty input.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Feature`] if the FFT computation fails.
    pub fn log_mel(&self, audio: &[f32]) -> Result<Array2<f32>> {
        if audio.is_empty() {
            return Ok(Array2::zeros((N_MELS, 0)));
        }
        let pre = apply_preemphasis(audio, PREEMPH);
        let power = stft(&pre, N_FFT, HOP_LENGTH, WIN_LENGTH)?;
        let mel = self.mel_basis.dot(&power);
        Ok(mel.mapv(|x| (x + LOG_ZERO_GUARD).ln()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // unwrap in tests is idiomatic; failures are bugs, not user errors
    use super::*;

    fn sine(freq_hz: f32, n: usize) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss)] // test helper; precision irrelevant
        (0..n)
            .map(|i| (2.0 * PI * freq_hz * i as f32 / SAMPLE_RATE as f32).sin())
            .collect()
    }

    #[test]
    fn stft_concentrates_power_at_expected_bin() {
        // 1 kHz tone => bin = 1000 * 512 / 16000 = 32.
        let audio = sine(1000.0, SAMPLE_RATE);
        let spec = stft(&audio, N_FFT, HOP_LENGTH, WIN_LENGTH).unwrap();
        let freq_bins = N_FFT / 2 + 1;
        let num_frames = spec.shape()[1];

        let mut correct = 0;
        for frame in 2..num_frames.saturating_sub(2) {
            let mut max_bin = 0;
            let mut max_power = 0.0f32;
            for bin in 0..freq_bins {
                if spec[[bin, frame]] > max_power {
                    max_power = spec[[bin, frame]];
                    max_bin = bin;
                }
            }
            if max_bin == 32 {
                correct += 1;
            }
        }
        let interior = num_frames.saturating_sub(4);
        assert!(
            correct > interior / 2,
            "expected bin 32 to dominate, got {correct}/{interior}"
        );
    }

    #[test]
    fn log_mel_shape_is_n_mels_by_frames() {
        let fe = MelFrontend::new();
        let mel = fe.log_mel(&vec![0.0f32; SAMPLE_RATE]).unwrap();
        assert_eq!(mel.shape()[0], N_MELS);
        assert!(mel.shape()[1] > 0);
    }

    #[test]
    fn hann_window_is_symmetric_and_zero_at_edges() {
        let w = hann_window(WIN_LENGTH);
        assert!(w[0].abs() < 1e-6);
        assert!(w[WIN_LENGTH - 1].abs() < 1e-6);
        // Symmetry: w[i] == w[n-1-i].
        for i in 0..WIN_LENGTH / 2 {
            assert!((w[i] - w[WIN_LENGTH - 1 - i]).abs() < 1e-6);
        }
    }
}
