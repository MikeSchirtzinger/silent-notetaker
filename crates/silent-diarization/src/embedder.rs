//! Native (non-wasm) TitaNet-small speaker embedder using `ort` (ONNX Runtime).
//!
//! Productionized from spike b1 (`docs/research/spike-titanet.md`, cosine
//! **1.000000** against the `eval/` fixtures). The mel front-end is the shared
//! [`silent_audio::MelFrontend`] in [`silent_audio::titanet_config`] — the same
//! 80-band slaney / periodic-Hann / per-feature-normalized path the spike
//! validated. This crate adds only the ONNX session on top.
//!
//! The weights are NOT committed to the repo (E1 removed `titanet.onnx` and
//! `mel_fb.json`); they are fetched via the registry pin
//! (`registry/models.toml` → `speaker_embedding.titanet.small`, sha256-pinned)
//! at runtime. Construct from in-memory bytes ([`TitaNetEmbedder::from_bytes`])
//! or, for local golden runs, from paths ([`TitaNetEmbedder::from_files`]).

// The ONNX tensor shape dims (mel bands, frame count) are cast usize→i64 for the
// `Tensor::from_array` shape, matching the ort API. These are tiny model
// dimensions (80 bands, a few thousand frames) that cannot wrap an i64.
#![allow(
    clippy::cast_possible_wrap,
    reason = "ONNX tensor shape dims (mel bands, frame count) are small model \
              dimensions that cannot wrap an i64"
)]

use crate::error::Result;
use ndarray::Array2;
use ort::session::Session;
use ort::value::Tensor;
use silent_audio::MelFrontend;

/// Native TitaNet-small speaker embedder (synchronous, CPU).
pub struct TitaNetEmbedder {
    session: Session,
    frontend: MelFrontend,
}

impl TitaNetEmbedder {
    /// Build from in-memory ONNX bytes and `mel_fb.json` bytes (the registry
    /// path: fetch both via the model fetcher, then hand the bytes here).
    ///
    /// # Errors
    ///
    /// Returns an error if `mel_fb.json` is malformed or the ORT session cannot
    /// be created from `onnx_bytes`.
    pub fn from_bytes(onnx_bytes: &[u8], mel_fb_json: &[u8]) -> Result<Self> {
        let frontend = MelFrontend::titanet_from_mel_fb_json(mel_fb_json)?;
        let session = Session::builder()?.commit_from_memory(onnx_bytes)?;
        Ok(Self { session, frontend })
    }

    /// Build from filesystem paths (local golden runs only — production fetches
    /// via the registry pin and uses [`TitaNetEmbedder::from_bytes`]).
    ///
    /// # Errors
    ///
    /// Returns an error if either file cannot be read/parsed or the ORT session
    /// cannot be created.
    pub fn from_files(onnx_path: &str, mel_fb_path: &str) -> Result<Self> {
        let mel_fb_json = std::fs::read(mel_fb_path)?;
        let frontend = MelFrontend::titanet_from_mel_fb_json(&mel_fb_json)?;
        let session = Session::builder()?.commit_from_file(onnx_path)?;
        Ok(Self { session, frontend })
    }

    /// Compute the 192-d L2-normalized speaker embedding for `audio`
    /// (16 kHz mono f32).
    ///
    /// # Errors
    ///
    /// Returns an error if the mel front-end or ONNX inference fails, or the
    /// model output shape is unexpected.
    pub fn embed(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
        let mel: Array2<f32> = self.frontend.log_mel(audio)?;
        let n_mels = mel.shape()[0];
        let t = mel.shape()[1];

        let audio_data = mel.into_raw_vec_and_offset().0;
        let audio_tensor = Tensor::from_array(([1_i64, n_mels as i64, t as i64], audio_data))?;
        let length_tensor = Tensor::from_array(([1_i64], vec![t as i64]))?;

        let outputs = self.session.run(ort::inputs![
            "audio_signal" => audio_tensor,
            "length" => length_tensor,
        ])?;

        let (_, emb_data) = outputs["embs"].try_extract_tensor::<f32>()?;
        Ok(emb_data.to_vec())
    }
}

/// Cosine similarity between two equal-length vectors (full form — normalizes
/// both, for golden checks against externally-computed embeddings).
///
/// # Panics
///
/// Panics if the slices differ in length.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "cosine: length mismatch");
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}
