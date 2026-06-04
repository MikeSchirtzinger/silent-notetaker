//! Engine lifecycle events and telemetry (the streaming contract).
//!
//! These mirror the lifecycle `nemotron-engine.js` discovered empirically:
//! load (with per-file progress), warm-up, feed, finalize, reset, stats. The
//! enum is `#[non_exhaustive]` so additive variants (a new draft kind, a new
//! stat) do not break the boundary; the TypeScript consumer handles this with a
//! mandatory wildcard `switch` arm (A3 spike escape-hatch pattern).

use serde::{Deserialize, Serialize};

use crate::ids::TimeRange;

/// An event emitted by an [`crate::engine::AsrEngine`] during its lifecycle.
///
/// Tagged as `{ "tag": "...", "payload": ... }` (the discriminant layout the A3
/// spike validated compiles cleanly as a TypeScript discriminated union).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EngineEvent {
    /// Per-file model download progress. Multi-file artifact sets (Voxtral
    /// ~2.7 GB, Nemotron three files) emit one stream of these per file.
    LoadProgress {
        /// Artifact path being fetched, for example `encoder.onnx`.
        file: String,
        /// Bytes fetched so far.
        loaded: u64,
        /// Total bytes for this file (`0` if the length is unknown).
        total: u64,
    },

    /// The engine has loaded, warmed up, and is ready to accept audio.
    Ready,

    /// A fast, low-accuracy draft (for example Moonshine in Dual mode) that a
    /// refiner pass will replace.
    Draft {
        /// Draft transcript text.
        text: String,
        /// The audio span this text covers.
        range: TimeRange,
    },

    /// A streaming partial that will be revised by a later event covering the
    /// same range (in-place partial rendering).
    Partial {
        /// Partial transcript text.
        text: String,
        /// The audio span this text covers.
        range: TimeRange,
    },

    /// A finalized transcript segment; it will not be revised.
    Final {
        /// Final transcript text.
        text: String,
        /// The audio span this text covers.
        range: TimeRange,
    },

    /// A telemetry snapshot (TTFT, RTF, chunk-ms). Drives the PerfMonitor UI and
    /// the R9 performance gates.
    Stats(EngineStats),

    /// A non-fatal warning the UI may surface (for example a thread-count
    /// downgrade). Distinct from an [`crate::error::AsrError`], which aborts the
    /// call.
    Warning {
        /// Human-readable warning message.
        message: String,
    },
}

/// Latency / throughput telemetry for an engine (PRD R9, Appendix A row 35).
///
/// Field semantics match `nemotron-engine.js`'s `stats()`: RTF below `1.0` is
/// faster than realtime; `ttft_ms` is the felt lag from first audio to first
/// visible text.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct EngineStats {
    /// One-time model load + warm-up cost, milliseconds.
    pub load_ms: u32,
    /// Number of audio chunks decoded this session.
    pub chunks: u32,
    /// Mean decode time per chunk, milliseconds.
    pub avg_chunk_ms: u32,
    /// Most recent chunk decode time, milliseconds.
    pub last_chunk_ms: u32,
    /// Audio seconds processed this session.
    pub audio_secs: f32,
    /// Real-time factor: processing-time / audio-duration. `< 1.0` beats
    /// realtime. The R9 Nemotron browser gate is `<= 0.5`.
    pub rtf: f32,
    /// Time to first text, milliseconds (first audio → first visible text). The
    /// R9 Nemotron browser gate is `<= 1000`.
    pub ttft_ms: u32,
    /// Samples still buffered awaiting a whole chunk; sustained growth means the
    /// decoder cannot keep up.
    pub pending_samples: u32,
}

/// Declared capabilities of an engine, used by the selection policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct AsrCapabilities {
    /// Whether the engine emits [`EngineEvent::Partial`] events (streaming) as
    /// opposed to only [`EngineEvent::Final`] segments.
    pub streaming: bool,
    /// Whether the engine emits [`EngineEvent::Draft`] events (Dual / draft
    /// mode).
    pub drafts: bool,
    /// Whether the engine requires WebGPU (Voxtral) versus running on CPU/WASM.
    pub requires_webgpu: bool,
    /// Sample rate the engine expects on [`crate::engine::AudioChunk`], in hertz
    /// (16 000 for every current engine).
    pub sample_rate_hz: u32,
}
