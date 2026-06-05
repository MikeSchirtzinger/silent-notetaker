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
//! The `notes` module (wasm32 only, Phase 3) wraps `silent-notes` behind a
//! typed surface: the live trigger extractor + open-question tracker
//! ([`notes::WasmNoteEngine`]), the smart-question teleprompter scheduler
//! ([`notes::WasmQuestionScheduler`]), the word-corrections policy
//! ([`notes::WasmCorrections`]), and the Qwen final-notes free functions. The JS
//! glue (`notes-engine.js`) drives it; the `question-worker.js` Qwen worker
//! stays the executor.
//!
//! The `session` module (wasm32 only, Phase 4) wraps `silent-core`'s
//! [`session::WasmSession`] — the recording-session state machine — behind the
//! typed `SessionEvent` / `SideEffect` boundary: start/stop/continue/new-meeting,
//! the Mic/Tab source set, the 120-char auto-title, the timer projection, and
//! the stop-time hooks (recluster / final notes / question recap / auto-summary).
//! The JS glue (`session-engine.js`) drives it; index.html executes the side
//! effects and renders the events (Appendix A rows 1, 2, 3, 6, 31).
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

/// Wasm-bindgen diarization surface (Phase 2). Wasm32 only.
#[cfg(target_arch = "wasm32")]
pub mod diarization;

/// Wasm-bindgen notes + smart-questions + Qwen surface (Phase 3). Wasm32 only.
#[cfg(target_arch = "wasm32")]
pub mod notes;

/// Wasm-bindgen recording-session surface (Phase 4, Task h1 wiring). Wasm32
/// only. Wraps `silent_core::session::SessionMachine` behind the typed
/// `SessionEvent` / `SideEffect` boundary (Appendix A rows 1, 2, 3, 6, 31).
#[cfg(target_arch = "wasm32")]
pub mod session;

/// Wasm-bindgen browser-storage surface (Phase 4, Task h2/x2 wiring). Wasm32
/// only. Wraps `silent-storage`: the live CRUD that replaces Dexie, the zero-loss
/// Dexie v2 migration with export-backup, history queries, and the durable
/// per-meeting speaker-rename map (Appendix A rows 1, 3, 16, 17, 19, 26, 27, 29,
/// 33, plus the Phase-F carry-forward).
#[cfg(target_arch = "wasm32")]
pub mod storage;

/// Wasm-bindgen export + history-duration formatting surface (Phase 4, Task
/// h3/x3). Wasm32 only. Wraps `silent_core::export` + `silent_core::timestamp`
/// (notes Markdown, timestamp-aware transcript text, summary AI-notes append,
/// history-replay markdown, executive line, `Nm Ns` duration — Appendix A
/// rows 24, 30).
#[cfg(target_arch = "wasm32")]
pub mod exports;

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

#[cfg(target_arch = "wasm32")]
pub use session::WasmSession;
