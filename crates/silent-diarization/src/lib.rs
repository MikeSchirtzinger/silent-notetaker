//! Speaker diarization for Silent Notetaker (PRD Phase 2).
//!
//! Rust's first user-visible quality win: speaker labels driven by Rust policy,
//! with a new stop-time global recluster that the JS app could not do cleanly.
//!
//! # What lives here
//!
//! - `embedder` (native) / `embedder_web` (wasm32) — the TitaNet-small
//!   speaker embedder productionized from spike b1
//!   (`docs/research/spike-titanet.md`, cosine **1.000000**). The mel front-end
//!   itself lives in [`silent_audio`] (the 80-band periodic-Hann path); this
//!   crate adds the ONNX session on top. CPU by design (registry
//!   `execution_provider = "cpu"`) so it never contends with Voxtral for the GPU.
//! - [`tracker`] — the [`tracker::SpeakerTracker`] port: online leader
//!   clustering (thresholds as [`tracker::TrackerConfig`], not magic numbers),
//!   the 8-color rotation ([`tracker::SPEAKER_COLORS`]), the rename / merge /
//!   merge-by-rename policy, and the **stop-time global recluster**
//!   ([`tracker::SpeakerTracker::global_recluster`], `docs/DIARIZATION.md`).
//!
//! # Privacy (PRD R5/R7)
//!
//! Raw embeddings live only in the tracker's `utterances` log and never cross an
//! extension or network boundary. The embedder runs entirely local (WASM ORT in
//! the browser); the b1 spike proved zero egress.
//!
//! # The boundary
//!
//! The diarization command/event TYPES (rename, merge, recluster, label
//! changes) live in `silent-core`'s `diarization` module so the UI (Task F2)
//! wires against a contract, not invented shapes.
#![forbid(unsafe_code)]

pub mod error;
pub mod tracker;

pub use error::{DiarizationError, Result};
pub use tracker::{
    Identified, RenameOutcome, SPEAKER_COLORS, Speaker, SpeakerTracker, TrackerConfig, Utterance,
    cosine_normalized,
};

#[cfg(not(target_arch = "wasm32"))]
pub mod embedder;
#[cfg(not(target_arch = "wasm32"))]
pub use embedder::{TitaNetEmbedder, cosine_similarity};

#[cfg(target_arch = "wasm32")]
pub mod embedder_web;
#[cfg(target_arch = "wasm32")]
pub use embedder_web::{WasmTitaNetEmbedder, cosine_sim};
