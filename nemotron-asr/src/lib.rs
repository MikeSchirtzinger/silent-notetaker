//! # nemotron-asr
//!
//! A clean, native streaming ASR engine for NVIDIA's
//! `nemotron-speech-streaming-en-0.6b` model, running INT8 encoder +
//! FP32 decoder/joint ONNX graphs through ONNX Runtime via [`ort`].
//!
//! This is Phase 1 of a browser ASR engine: get a native crate working and
//! validated, structured so a `wasm32` / `ort-web` backend can slot in behind
//! the [`AsrBackend`] trait later. No WASM code lives here yet.
//!
//! ## Architecture
//!
//! - [`audio`] — the log-mel front-end (pre-emphasis, symmetric-Hann STFT,
//!   power spectrum, Slaney mel filterbank, `ln(x + 2^-24)`, **no**
//!   normalization).
//! - [`vocab`] — a pure-Rust SentencePiece detokenizer (no native C deps, so
//!   it stays wasm-friendly).
//! - [`model`] — the [`AsrBackend`] trait and the native [`OrtBackend`]
//!   implementation; the only place that links `ort`.
//! - [`streaming`] — the cache-aware RNN-T decode loop ([`StreamingAsr`]).
//!
//! ## Quick start
//!
//! ```no_run
//! use nemotron_asr::Nemotron;
//!
//! let mut asr = Nemotron::from_pretrained("models")?;
//! let audio = nemotron_asr::audio::load_wav_mono("test-assets/test_16k.wav")?;
//! let transcript = asr.transcribe_audio(&audio)?;
//! println!("{transcript}");
//! # Ok::<(), nemotron_asr::Error>(())
//! ```

pub mod audio;
pub mod constants;
pub mod error;
pub mod model;
pub mod streaming;
pub mod vocab;

pub use error::{Error, Result};
pub use model::{AsrBackend, EncoderCache};
pub use streaming::StreamingAsr;
pub use vocab::SentencePieceVocab;

#[cfg(not(target_arch = "wasm32"))]
pub use model::OrtBackend;

use std::path::Path;

use crate::audio::MelFrontend;

/// Convenience alias for the native ([`OrtBackend`]-backed) streaming engine.
#[cfg(not(target_arch = "wasm32"))]
pub type Nemotron = StreamingAsr<OrtBackend>;

#[cfg(not(target_arch = "wasm32"))]
impl StreamingAsr<OrtBackend> {
    /// Load the model and tokenizer from a directory.
    ///
    /// Expects `encoder.onnx`, `decoder_joint_fp32.onnx`, and
    /// `tokenizer.model` in `model_dir`.
    pub fn from_pretrained<P: AsRef<Path>>(model_dir: P) -> Result<Self> {
        let dir = model_dir.as_ref();
        let backend = OrtBackend::from_dir(dir)?;
        let frontend = MelFrontend::new();
        let vocab = SentencePieceVocab::from_file(dir.join("tokenizer.model"))?;
        Ok(StreamingAsr::new(backend, frontend, vocab))
    }
}
