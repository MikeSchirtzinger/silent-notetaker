//! Parameterized mel front-ends.
//!
//! This is a productionized port of the pure-JS TitaNet front-end in
//! `eval/js/melfeat.mjs` (validated byte-for-byte against the Python reference
//! in `eval/dump_for_js.py`, spike b1, cosine **1.000000**). Every constant in
//! [`titanet_config`] matches the JS and Python.
//!
//! The pipeline (in order, matching the reference exactly):
//!
//! 1. **Pre-emphasis**: `y[0] = x[0]`, `y[i] = x[i] - 0.97 * x[i-1]` (f64).
//! 2. **Reflect padding**: `PAD` samples each side, numpy-style (edge sample
//!    NOT repeated: `out[pad-1-i] = x[i+1]`).
//! 3. **STFT**: window is a **periodic** Hann of length `win` zero-padded to
//!    `n_fft` and centred (`left = (n_fft - win) / 2`).
//! 4. **Power spectrum**: `|X[k]|^2 = re^2 + im^2`.
//! 5. **Mel projection**: `mel_fb @ power` using the pre-baked slaney matrix.
//! 6. **Log**: `log(s + log_guard)`.
//! 7. **Per-feature normalization** (TitaNet only): for each mel band, subtract
//!    its mean over time and divide by its std + 1e-5.
//! 8. **Cast to f32** for the ONNX model.
//!
//! # The periodic-Hann guard (do NOT change)
//!
//! The Hann window here is **periodic** (`w[n] = 0.5 - 0.5*cos(2πn/win)`), NOT
//! symmetric (`w[n] = 0.5 - 0.5*cos(2πn/(win-1))`). The TitaNet training
//! pipeline (NVIDIA `NeMo`) uses `librosa.filters.get_window('hann', 400,
//! fftbins=True)`, which
//! is the **periodic** form. Using the symmetric form (as `nemotron-asr` uses
//! for its 128-band frontend) with TitaNet produces a cosine gap of `~0.9997`
//! instead of `1.000000` — a measurable mismatch that degrades clustering.
//!
//! **The two front-ends must never be unified.** A PR that "deduplicates"
//! [`titanet_config`] and [`nemotron_config`] into one toggled frontend is a
//! correctness bug. They share a config *shape* for clarity, not a runtime path
//! that can flip the window denominator by accident.

// The mel pipeline computes in f64 (to match the JS Float64Array reference at
// cosine 1.000000, spike b1) and casts to f32 only at the very end for the ONNX
// model — these casts are INTENTIONAL and load-bearing, not accidental. The
// usize→f64 casts are frame/band/sample counts that never approach 2^52. The
// f64→f32 cast is the deliberate final precision step the reference performs.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "the frontend computes in f64 and casts to f32 at the end to match \
              the JS reference at cosine 1.000000 (spike b1); counts never \
              exceed f64 mantissa range"
)]

use crate::error::{AudioError, Result};
use ndarray::Array2;
use realfft::RealFftPlanner;

/// Which Hann window the frontend uses. The denominator differs by exactly one
/// (`win` vs `win - 1`); see the periodic-Hann guard in this module's docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HannWindow {
    /// `w[n] = 0.5 - 0.5*cos(2πn/win)` — TitaNet (librosa `fftbins=True`).
    Periodic,
    /// `w[n] = 0.5 - 0.5*cos(2πn/(win-1))` — Nemotron / numpy.hanning.
    Symmetric,
}

/// How the spectrum is computed before the mel projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumMode {
    /// `|X[k]|^2 = re^2 + im^2` (both frontends project this onto the mel basis).
    Power,
}

/// Post-log normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizationMode {
    /// No normalization (Nemotron).
    None,
    /// Per-mel-band mean/std over time, `+1e-5` stabiliser (TitaNet).
    PerFeatureMeanStd,
}

/// Shape of a mel front-end. Instantiated via the named presets
/// [`titanet_config`] / [`nemotron_config`]; never share a single live config.
///
/// Not `Eq` — it carries f64 fields (`preemph`, `log_guard`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MelConfig {
    /// Number of mel bands (80 TitaNet / 128 Nemotron).
    pub n_mels: usize,
    /// FFT size.
    pub n_fft: usize,
    /// Analysis window length (samples).
    pub win: usize,
    /// Hop between frames.
    pub hop: usize,
    /// Reflect-pad amount (numpy-style).
    pub pad: usize,
    /// Which Hann window — the critical TitaNet/Nemotron difference.
    pub window: HannWindow,
    /// Spectrum mode.
    pub spectrum: SpectrumMode,
    /// Pre-emphasis coefficient (0.0 disables).
    pub preemph: f64,
    /// Additive log guard.
    pub log_guard: f64,
    /// Post-log normalization.
    pub norm: NormalizationMode,
}

impl MelConfig {
    /// Number of FFT frequency bins (`n_fft / 2 + 1`).
    #[must_use]
    pub const fn n_freq(&self) -> usize {
        self.n_fft / 2 + 1
    }
}

/// The TitaNet-small front-end config (spike b1 golden, cosine 1.000000).
///
/// 80-band slaney, **periodic** Hann, magnitude→power, per-feature mean/std
/// normalization. Filterbank comes from `mel_fb.json` (pre-baked).
#[must_use]
pub fn titanet_config() -> MelConfig {
    MelConfig {
        n_mels: 80,
        n_fft: 512,
        win: 400,
        hop: 160,
        pad: 256,
        window: HannWindow::Periodic,
        spectrum: SpectrumMode::Power,
        preemph: 0.97,
        // 2^-24
        log_guard: 5.960_464_477_539_063e-8,
        norm: NormalizationMode::PerFeatureMeanStd,
    }
}

/// The Nemotron front-end config (128-band, symmetric Hann, NO normalization).
///
/// Provided so the "two frontends" contract is explicit and testable in one
/// place. The Nemotron ASR engine's frontend lives in `nemotron-asr/src/audio.rs`
/// (it computes its filterbank from constants at runtime). This preset exists to
/// pin the differing axes and to power the
/// [`nemotron_config_differs_from_titanet`](self) regression test — it must not
/// be merged with [`titanet_config`].
#[must_use]
pub fn nemotron_config() -> MelConfig {
    MelConfig {
        n_mels: 128,
        n_fft: 512,
        win: 400,
        hop: 160,
        pad: 256,
        window: HannWindow::Symmetric,
        spectrum: SpectrumMode::Power,
        preemph: 0.97,
        log_guard: 5.960_464_477_539_063e-8,
        norm: NormalizationMode::None,
    }
}

/// Build a Hann window of length `win` per `mode`, zero-padded to `n_fft` and
/// centred within the FFT buffer (= librosa `pad_center`).
///
/// JS reference (TitaNet, periodic):
/// ```js
/// const left = (NFFT - WIN) >> 1;  // = 56 for 512/400
/// for (let n = 0; n < WIN; n++)
///     w[left + n] = 0.5 - 0.5 * Math.cos((2 * Math.PI * n) / WIN);
/// ```
///
/// The denominator is `win` for [`HannWindow::Periodic`] and `win - 1` for
/// [`HannWindow::Symmetric`]. This single-character difference is the entire
/// TitaNet/Nemotron window distinction; see the module docs.
fn hann_padded(win: usize, n_fft: usize, mode: HannWindow) -> Vec<f64> {
    let mut w = vec![0.0_f64; n_fft];
    let left = (n_fft - win) / 2;
    let denom = match mode {
        HannWindow::Periodic => win as f64,
        HannWindow::Symmetric => (win - 1) as f64,
    };
    for n in 0..win {
        w[left + n] = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / denom).cos();
    }
    w
}

/// Numpy-style reflect padding: `out[pad-1-i] = x[i+1]`, edge sample NOT
/// repeated.
fn reflect_pad(x: &[f64], pad: usize) -> Vec<f64> {
    let n = x.len();
    let mut out = vec![0.0_f64; n + 2 * pad];
    out[pad..pad + n].copy_from_slice(x);
    for i in 0..pad {
        out[pad - 1 - i] = x[i + 1];
        out[pad + n + i] = x[n - 2 - i];
    }
    out
}

/// Apply first-order pre-emphasis: `y[0] = x[0]`, `y[i] = x[i] - coef*x[i-1]`.
fn preemphasis(x: &[f32], coef: f64) -> Vec<f64> {
    let mut y = Vec::with_capacity(x.len());
    if x.is_empty() {
        return y;
    }
    y.push(f64::from(x[0]));
    for i in 1..x.len() {
        y.push(f64::from(x[i]) - coef * f64::from(x[i - 1]));
    }
    y
}

/// A reusable mel front-end: a [`MelConfig`] plus a pre-baked filterbank and a
/// cached window. Construct once via [`MelFrontend::new`]; call
/// [`MelFrontend::log_mel`] per utterance.
pub struct MelFrontend {
    config: MelConfig,
    /// Pre-baked `n_mels × n_freq` slaney mel filterbank.
    mel_basis: Vec<Vec<f64>>,
    /// Hann window (length `win`) padded to `n_fft`, per `config.window`.
    window: Vec<f64>,
}

impl MelFrontend {
    /// Construct from a config and a pre-loaded mel filterbank
    /// (`n_mels × n_freq`, row-major; rows are mel bands, columns FFT bins).
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Shape`] if the filterbank dimensions do not match
    /// `config.n_mels × config.n_freq()`.
    pub fn new(config: MelConfig, mel_basis: Vec<Vec<f64>>) -> Result<Self> {
        if mel_basis.len() != config.n_mels {
            return Err(AudioError::Shape(format!(
                "expected {} mel rows, got {}",
                config.n_mels,
                mel_basis.len()
            )));
        }
        let n_freq = config.n_freq();
        for (i, row) in mel_basis.iter().enumerate() {
            if row.len() != n_freq {
                return Err(AudioError::Shape(format!(
                    "mel row {i}: expected {n_freq} cols, got {}",
                    row.len()
                )));
            }
        }
        let window = hann_padded(config.win, config.n_fft, config.window);
        Ok(Self {
            config,
            mel_basis,
            window,
        })
    }

    /// Construct the TitaNet front-end from a `mel_fb.json` byte payload (the
    /// pre-baked 80×257 slaney matrix; fetched from the registry at runtime).
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Shape`] if the JSON lacks a `matrix` field, has a
    /// non-array row, a non-numeric value, or the wrong dimensions.
    pub fn titanet_from_mel_fb_json(json_bytes: &[u8]) -> Result<Self> {
        let mel_basis = parse_mel_fb_json(json_bytes)?;
        Self::new(titanet_config(), mel_basis)
    }

    /// The active config.
    #[must_use]
    pub fn config(&self) -> &MelConfig {
        &self.config
    }

    /// Compute the log-mel feature matrix from `audio` (16 kHz mono f32).
    ///
    /// Returns `Array2<f32>` of shape `[n_mels, T]` (row-major, mel-first) — the
    /// format the TitaNet ONNX model expects as `audio_signal`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Feature`] for empty/too-short audio or an FFT
    /// failure; [`AudioError::Shape`] if the output cannot be reshaped.
    pub fn log_mel(&self, audio: &[f32]) -> Result<Array2<f32>> {
        if audio.is_empty() {
            return Err(AudioError::Feature("empty audio".into()));
        }

        let cfg = &self.config;

        // Step 1: pre-emphasis (f64 for numerical precision).
        let pre = preemphasis(audio, cfg.preemph);

        // Step 2: reflect padding.
        let padded = reflect_pad(&pre, cfg.pad);

        if padded.len() < cfg.n_fft {
            return Err(AudioError::Feature(
                "audio too short to produce any frames".into(),
            ));
        }

        // Step 3+: STFT → power → mel projection → log → normalize.
        let t = 1 + (padded.len() - cfg.n_fft) / cfg.hop;
        if t == 0 {
            return Err(AudioError::Feature(
                "audio too short to produce any frames".into(),
            ));
        }

        // STFT via realfft (f64 for parity with the JS Float64Array reference).
        let mut planner = RealFftPlanner::<f64>::new();
        let plan = planner.plan_fft_forward(cfg.n_fft);
        let mut fft_in = plan.make_input_vec();
        let mut fft_out = plan.make_output_vec();
        let mut scratch = plan.make_scratch_vec();

        let n_mels = cfg.n_mels;
        let n_freq = cfg.n_freq();
        // feat[m * T + frame] — f64 accumulator.
        let mut feat = vec![0.0_f64; n_mels * t];

        for frame in 0..t {
            let off = frame * cfg.hop;
            for i in 0..cfg.n_fft {
                fft_in[i] = padded[off + i] * self.window[i];
            }
            plan.process_with_scratch(&mut fft_in, &mut fft_out, &mut scratch)
                .map_err(|e| AudioError::Feature(format!("FFT failed: {e}")))?;

            // Power spectrum projected inline onto the mel basis (no power buf).
            for m in 0..n_mels {
                let row = &self.mel_basis[m];
                let mut s = 0.0_f64;
                for k in 0..n_freq {
                    let c = fft_out[k];
                    let power = c.re * c.re + c.im * c.im; // SpectrumMode::Power
                    s += row[k] * power;
                }
                feat[m * t + frame] = (s + cfg.log_guard).ln();
            }
        }

        // Step 7: per-feature (per mel band) mean/std normalization (TitaNet).
        if matches!(cfg.norm, NormalizationMode::PerFeatureMeanStd) {
            for m in 0..n_mels {
                let base = m * t;
                let slice = &feat[base..base + t];
                let mean = slice.iter().sum::<f64>() / t as f64;
                let var = slice.iter().map(|&v| (v - mean) * (v - mean)).sum::<f64>() / t as f64;
                let std = var.sqrt() + 1e-5;
                for frame in 0..t {
                    feat[base + frame] = (feat[base + frame] - mean) / std;
                }
            }
        }

        // Step 8: cast to f32 for ONNX.
        let feat_f32: Vec<f32> = feat.iter().map(|&v| v as f32).collect();
        Array2::from_shape_vec((n_mels, t), feat_f32)
            .map_err(|e| AudioError::Shape(format!("ndarray shape: {e}")))
    }
}

/// Parse a `mel_fb.json` payload into a `Vec<Vec<f64>>` filterbank
/// (`{ "matrix": [[...], ...] }`).
///
/// # Errors
///
/// Returns [`AudioError::Shape`] on malformed JSON or a missing/ill-typed
/// `matrix` field.
pub fn parse_mel_fb_json(json_bytes: &[u8]) -> Result<Vec<Vec<f64>>> {
    let val: serde_json::Value = serde_json::from_slice(json_bytes)
        .map_err(|e| AudioError::Shape(format!("mel_fb.json parse: {e}")))?;
    val["matrix"]
        .as_array()
        .ok_or_else(|| AudioError::Shape("mel_fb.json missing 'matrix'".into()))?
        .iter()
        .map(|row| {
            row.as_array()
                .ok_or_else(|| AudioError::Shape("mel row is not an array".into()))
                .and_then(|r| {
                    r.iter()
                        .map(|v| {
                            v.as_f64()
                                .ok_or_else(|| AudioError::Shape("mel value not f64".into()))
                        })
                        .collect::<Result<Vec<f64>>>()
                })
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests use unwrap as the assertion mechanism (PRD lint config)"
)]
#[allow(
    clippy::float_cmp,
    reason = "the zero-pad slots of the window buffer are exactly 0.0 (never \
              written); an exact compare is the intended contract"
)]
mod tests {
    use super::*;

    fn uniform_fb(n_mels: usize, n_freq: usize) -> Vec<Vec<f64>> {
        vec![vec![1.0_f64 / n_freq as f64; n_freq]; n_mels]
    }

    #[test]
    fn titanet_periodic_hann_peak_is_one() {
        let cfg = titanet_config();
        let w = hann_padded(cfg.win, cfg.n_fft, cfg.window);
        let left = (cfg.n_fft - cfg.win) / 2;
        // Periodic Hann: 0.5 - 0.5*cos(2π*(WIN/2)/WIN) = 0.5 - 0.5*cos(π) = 1.0.
        let peak = w[left + cfg.win / 2];
        assert!(
            (peak - 1.0).abs() < 1e-12,
            "periodic hann peak should be exactly 1.0: {peak}"
        );
        // Zero outside the centred window — the buffer is `vec![0.0; n_fft]` and
        // those slots are never written, so an exact-zero compare is the contract.
        for &v in w.iter().take(left) {
            assert!(v == 0.0, "left zero-pad must be exactly 0.0, got {v}");
        }
        for &v in w.iter().skip(left + cfg.win) {
            assert!(v == 0.0, "right zero-pad must be exactly 0.0, got {v}");
        }
    }

    #[test]
    fn periodic_hann_differs_from_symmetric() {
        // The single-character difference (WIN vs WIN-1) is the guard. At the
        // window peak the periodic form is exactly 1.0; the symmetric is not.
        let cfg = titanet_config();
        let left = (cfg.n_fft - cfg.win) / 2;
        let periodic = hann_padded(cfg.win, cfg.n_fft, HannWindow::Periodic);
        let symmetric = hann_padded(cfg.win, cfg.n_fft, HannWindow::Symmetric);
        let p = periodic[left + cfg.win / 2];
        let s = symmetric[left + cfg.win / 2];
        assert!((p - 1.0).abs() < 1e-12, "periodic peak should be 1.0: {p}");
        assert!(
            (s - 1.0).abs() > 1e-6,
            "symmetric peak should differ from 1.0: {s}"
        );
    }

    #[test]
    fn nemotron_config_differs_from_titanet() {
        // The two frontends must never collapse into one. Pin the differing axes.
        let t = titanet_config();
        let n = nemotron_config();
        assert_ne!(t.n_mels, n.n_mels, "mel-band count differs (80 vs 128)");
        assert_ne!(t.window, n.window, "Hann window differs (periodic vs sym)");
        assert_ne!(
            t.norm, n.norm,
            "normalization differs (per-feature vs none)"
        );
    }

    #[test]
    fn reflect_pad_matches_numpy() {
        // numpy.pad([1,2,3,4,5], 2, mode='reflect') = [3,2,1,2,3,4,5,4,3]
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let padded = reflect_pad(&x, 2);
        let expected = [3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0];
        assert_eq!(padded.len(), expected.len());
        for (a, b) in padded.iter().zip(expected.iter()) {
            assert!((a - b).abs() < 1e-12, "pad mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn titanet_log_mel_shape() {
        let cfg = titanet_config();
        let fe = MelFrontend::new(cfg, uniform_fb(cfg.n_mels, cfg.n_freq())).unwrap();
        let audio = vec![0.0_f32; 16000 * 4];
        let mel = fe.log_mel(&audio).unwrap();
        assert_eq!(mel.shape()[0], cfg.n_mels);
        assert!(mel.shape()[1] > 0);
    }

    #[test]
    fn log_mel_rejects_empty() {
        let cfg = titanet_config();
        let fe = MelFrontend::new(cfg, uniform_fb(cfg.n_mels, cfg.n_freq())).unwrap();
        assert!(fe.log_mel(&[]).is_err());
    }

    #[test]
    fn new_rejects_wrong_filterbank_shape() {
        let cfg = titanet_config();
        // 79 rows instead of 80.
        let bad = vec![vec![0.0_f64; cfg.n_freq()]; cfg.n_mels - 1];
        assert!(MelFrontend::new(cfg, bad).is_err());
        // Wrong column count.
        let bad_cols = vec![vec![0.0_f64; cfg.n_freq() - 1]; cfg.n_mels];
        assert!(MelFrontend::new(cfg, bad_cols).is_err());
    }
}
