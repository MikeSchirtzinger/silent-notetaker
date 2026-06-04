//! Crate-wide error type.

use std::path::PathBuf;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by `nemotron-asr`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A required model file (encoder/decoder ONNX, tokenizer) was missing.
    #[error("missing model file: {0}")]
    MissingFile(PathBuf),

    /// WAV reading / audio I/O failure.
    #[error("audio error: {0}")]
    Audio(String),

    /// SentencePiece tokenizer parsing or decoding failure.
    #[error("tokenizer error: {0}")]
    Tokenizer(String),

    /// Mel / FFT front-end failure.
    #[error("feature extraction error: {0}")]
    Feature(String),

    /// An error surfaced by the ONNX Runtime session, or a shape mismatch
    /// while marshalling tensors to/from it.
    #[error("model error: {0}")]
    Model(String),
}

// `hound` (WAV reading) is present on both targets: natively for the WAV
// loader/examples, and on wasm only so the unchanged `audio.rs::load_wav_mono`
// still compiles (it is dead code in the wasm binary — JS supplies samples).
impl From<hound::Error> for Error {
    fn from(e: hound::Error) -> Self {
        Error::Audio(e.to_string())
    }
}

impl From<ort::Error> for Error {
    fn from(e: ort::Error) -> Self {
        Error::Model(e.to_string())
    }
}

impl From<ndarray::ShapeError> for Error {
    fn from(e: ndarray::ShapeError) -> Self {
        Error::Model(format!("shape error: {e}"))
    }
}
