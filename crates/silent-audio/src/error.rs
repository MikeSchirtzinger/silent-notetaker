//! Error type for the audio frontends.

use thiserror::Error;

/// Errors from mel feature computation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AudioError {
    /// Mel feature computation error (empty/too-short audio, FFT failure).
    #[error("feature error: {0}")]
    Feature(String),

    /// Shape mismatch or filterbank-dimension invariant violation.
    #[error("shape error: {0}")]
    Shape(String),
}

/// Convenience `Result` alias for this crate.
pub type Result<T> = std::result::Result<T, AudioError>;
