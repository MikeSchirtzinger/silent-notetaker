//! The wasm-bindgen UI↔core boundary (Phase 2: diarization surface).
//!
//! # What this crate provides
//!
//! The `diarization` module (wasm32 only) wraps `silent-diarization` behind a
//! typed wasm-bindgen surface: commands in (embed+track, rename, merge,
//! recluster), typed events out (JSON-serialized, matching the `DiarizationEvent`
//! shapes from `silent-core`). The JS glue (`diarization-engine.js`) loads the
//! wasm-pack output (`pkg/`) and drives it — the same pattern as
//! `nemotron-engine.js` drives `crates/nemotron-asr/pkg/`.
//!
//! Phase 3+ will add the ASR and notes surfaces here. For now the diarization
//! surface is the sole export.
//!
//! # TypeScript types
//!
//! The boundary types (`DiarizationCommand`, `DiarizationEvent`, etc.) are
//! defined in `silent-core/src/diarization.rs` and generated into
//! `crates/silent-core/bindings/` via `cargo test -p silent-core export_bindings`
//! (A3 decision: type generation decoupled from the wasm build). The
//! `diarization-engine.js` glue uses those types.
//!
//! # No browser dependencies on the native target
//!
//! The `diarization` module is `#[cfg(target_arch = "wasm32")]` so
//! `cargo check --workspace --all-targets` compiles this crate without any
//! wasm-bindgen or ort-web deps. The native `rlib` exists solely to keep the
//! workspace gate green; the `cdylib` is built only by `wasm-pack`.
#![forbid(unsafe_code)]

pub use silent_core;

/// Wasm-bindgen diarization surface (Phase 2). Wasm32 only.
#[cfg(target_arch = "wasm32")]
pub mod diarization;

#[cfg(target_arch = "wasm32")]
pub use diarization::WasmDiarization;
