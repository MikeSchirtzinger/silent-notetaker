//! The wasm-bindgen UI↔core boundary (Phase 2: diarization surface).
//!
//! # What this crate provides
//!
//! The `diag` module (Phase 5, Appendix A row 34) is the crash-diagnostics
//! `tracing` subscriber: [`diag::DiagLayer`] folds the `silent.diag` tracing
//! events the engine emits into a `silent_core::Diag` sampler and writes the
//! bounded `notetakerDiag` ring through a `StorageSink`. The translation logic
//! is browser-free (native-testable); only its `LocalStorageSink` and the
//! `performance.memory` / `performance.now()` readers are `wasm32`-only. The
//! `window.dumpDiag` and prior-trail banner glue stay in the UI (wiring); the
//! row format and prior-trail strings are owned and byte-pinned by `silent-core`.
//!
//! The `diarization` module (wasm32 only) wraps `silent-diarization` behind a
//! typed wasm-bindgen surface: commands in (embed+track, rename, merge,
//! recluster), typed events out (JSON-serialized, matching the `DiarizationEvent`
//! shapes from `silent-core`). The JS glue (`diarization-engine.js`) loads the
//! wasm-pack output (`pkg/`) and drives it — the same pattern as
//! `nemotron-engine.js` drives `crates/nemotron-asr/pkg/`.
//!
//! The `notes` module (wasm32 only, Phase 3) wraps `silent-notes` behind a
//! typed surface: the live trigger extractor + open-question tracker
//! ([`notes::WasmNoteEngine`]), the smart-question teleprompter scheduler
//! ([`notes::WasmQuestionScheduler`]), the word-corrections policy
//! ([`notes::WasmCorrections`]), and the Qwen final-notes free functions. The JS
//! glue (`notes-engine.js`) drives it; the `question-worker.js` Qwen worker
//! stays the executor.
//!
//! The `nemotron` module (wasm32 only, Phase 3, Task w4) wraps `nemotron-asr`'s
//! `WasmAsr` ([`nemotron::WasmNemotron`]) behind the typed `EngineEvent`
//! boundary: model-download progress (Appendix A row 9) and PerfMonitor stats
//! (row 35) now arrive as `silent_core::EngineEvent::{LoadProgress, Ready,
//! Partial, Final, Stats}` JSON instead of the ad-hoc `onStatus`/`onText`
//! callbacks `nemotron-engine.js` used to invent. The decode itself is
//! unchanged — the wrapper only adds the event glue. The thin loader
//! (`nemotron-engine.js`) drives it.
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

/// Crash-diagnostics `tracing` subscriber (Phase 5, Appendix A row 34). The
/// [`DiagLayer`](diag::DiagLayer) translation logic is browser-free and
/// available on ALL targets (native-testable against an in-memory store); only
/// its `LocalStorageSink` + `performance.memory` readers are `wasm32`-only.
pub mod diag;

/// Wasm-bindgen diarization surface (Phase 2). Wasm32 only.
#[cfg(target_arch = "wasm32")]
pub mod diarization;

/// Wasm-bindgen notes + smart-questions + Qwen surface (Phase 3). Wasm32 only.
#[cfg(target_arch = "wasm32")]
pub mod notes;

/// Wasm-bindgen Nemotron ASR surface (Phase 3, Task w4). Wasm32 only. Wraps
/// `nemotron-asr`'s `WasmAsr` behind the typed `EngineEvent` boundary.
#[cfg(target_arch = "wasm32")]
pub mod nemotron;

#[cfg(target_arch = "wasm32")]
pub use diarization::WasmDiarization;

#[cfg(target_arch = "wasm32")]
pub use nemotron::WasmNemotron;

#[cfg(target_arch = "wasm32")]
pub use notes::{WasmCorrections, WasmNoteEngine, WasmQuestionScheduler};
