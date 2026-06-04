//! Shared error types for the core contracts.
//!
//! Per the PRD "Core contracts": engines share **one** [`AsrError`] enum rather
//! than per-engine associated error types, because engine swapping needs
//! uniform error handling. Errors are domain-specific (`thiserror`); `anyhow` is
//! reserved for binary / `xtask` boundaries, never library crates.

use serde::{Deserialize, Serialize};

use crate::ids::ModelId;

/// The single error type returned by every [`crate::engine::AsrEngine`] method.
///
/// `#[non_exhaustive]` so new failure modes can be added without a breaking
/// change; callers must include a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AsrError {
    /// A required model artifact could not be resolved or verified. Carries the
    /// underlying [`ModelResolveError`] so the UI can show a precise reason
    /// (PRD R4: "a stale or moved model link fails loudly with a
    /// model-resolution error, not a broken meeting UI").
    #[error("model resolution failed: {0}")]
    ModelResolution(ModelResolveError),

    /// The selected engine requires a capability the device does not provide
    /// (for example WebGPU is unavailable). No silent fallback: the UI surfaces
    /// the reason and recommends a CPU-tier engine (PRD R1).
    #[error("device requirement not met: {reason}")]
    DeviceUnsupported {
        /// Human-readable reason, for example `"WebGPU adapter not available"`.
        reason: String,
    },

    /// Loading or building the model runtime session failed.
    #[error("engine load failed: {message}")]
    Load {
        /// Engine-reported message.
        message: String,
    },

    /// An inference / decode step failed mid-stream.
    #[error("inference failed: {message}")]
    Inference {
        /// Engine-reported message.
        message: String,
    },

    /// The audio fed to the engine had an unexpected shape (wrong sample rate,
    /// empty chunk, etc.).
    #[error("invalid audio: {message}")]
    InvalidAudio {
        /// What was wrong with the audio.
        message: String,
    },

    /// The host worker (transformers.js / sherpa-onnx) reported a protocol or
    /// runtime error across the typed command boundary.
    #[error("host error: {message}")]
    Host {
        /// Host-reported message.
        message: String,
    },

    /// The engine was used in an invalid lifecycle state (for example `feed`
    /// before `load`).
    #[error("invalid state: {message}")]
    InvalidState {
        /// Description of the lifecycle violation.
        message: String,
    },
}

/// Why a model artifact could not be resolved from the registry or verified.
///
/// `#[non_exhaustive]`; callers must include a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelResolveError {
    /// No registry entry exists for the requested id.
    #[error("no registry entry for model `{0}`")]
    UnknownModel(ModelId),

    /// A network fetch for a required artifact failed.
    #[error("fetch failed for `{path}`: {message}")]
    Fetch {
        /// The artifact path that failed.
        path: String,
        /// Transport-reported message.
        message: String,
    },

    /// A fetched artifact did not match its pinned sha256 (PRD R4: verify by
    /// hash; verify-once-per-revision).
    #[error("hash mismatch for `{path}`: expected {expected}, got {actual}")]
    HashMismatch {
        /// The artifact path.
        path: String,
        /// The pinned sha256 from the registry.
        expected: String,
        /// The sha256 actually computed over the fetched bytes.
        actual: String,
    },

    /// A registry entry pins a mutable revision (`main`) where an immutable one
    /// is required. Surfaced at audit time, not load time (PRD R4 / `xtask
    /// model-audit`), but represented here for completeness.
    #[error("registry entry for `{0}` pins a mutable revision")]
    MutableRevision(ModelId),
}
