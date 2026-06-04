//! Error types for speaker diarization.

use thiserror::Error;

/// Errors from the TitaNet embedder and the speaker tracker.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DiarizationError {
    /// Mel front-end error (propagated from `silent-audio`).
    #[error("audio frontend error: {0}")]
    Audio(#[from] silent_audio::AudioError),

    /// ONNX session / inference error.
    #[cfg(not(target_arch = "wasm32"))]
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),

    /// I/O error (loading weights from a path, native only).
    #[cfg(not(target_arch = "wasm32"))]
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON parsing error (`mel_fb.json`).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Shape mismatch or model-output invariant violation.
    #[error("shape error: {0}")]
    Shape(String),
}

/// Convenience `Result` alias for this crate.
pub type Result<T> = std::result::Result<T, DiarizationError>;
