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
//! The `engine` module (wasm32 only, Phase 5, step y2-engine-paths; Appendix A
//! rows 10, 11) wraps the four `silent_inference` engine policies behind the
//! typed `EngineEvent` / `HostCommand` JSON boundary: [`engine::WasmWhisperStream`]
//! (Whisper family + Moonshine solo loop), [`engine::WasmVoxtralRecycle`]
//! (Voxtral's two-cap recycle), [`engine::WasmSenseVoice`] (SenseVoice VAD
//! segmentation), and [`engine::WasmDual`] (the draft/refine coordinator). The
//! transformers.js worker / sherpa-onnx harness execute; the policy is Rust law.
//! The JS glue (`whisper-engine.js`, `voxtral-engine.js`, `sensevoice-engine.js`,
//! `dual-engine.js`) drives them, REPLACING the inline index.html engine loops.
//!
//! The `selection` module (wasm32 only, Phase 5, Task I3) wraps
//! `silent_inference::selection` behind a typed surface: the registry-driven ASR
//! picker option list (Appendix A rows 7, 8 — every engine sourced from the
//! embedded registry's `ui` entries), per-engine availability verdicts with
//! reasons + a CPU-tier recommendation (R1), device-tier resolution from a typed
//! `DeviceProbe` (R3), and the queued mid-recording switch with a friendly
//! "takes effect next meeting" notice (R3 decision log). The JS glue
//! (`selection-engine.js`) drives it; the picker render + persistence stay in
//! `index.html`.
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

/// Wasm-bindgen registry-driven engine-selection surface (Phase 5, Task I3;
/// Appendix A rows 7, 8). Wasm32 only. Wraps `silent_inference::selection` — the
/// embedded-registry-driven model picker policy, device-tier resolution,
/// availability verdicts with reasons, and the queued mid-recording switch. The
/// JS glue (`selection-engine.js`) drives it.
#[cfg(target_arch = "wasm32")]
pub mod selection;

/// Wasm-bindgen js-host engine surfaces (Phase 5, step y2-engine-paths; Appendix
/// A rows 10, 11). Wasm32 only. Wraps the four `silent_inference` engine
/// policies — Whisper/Moonshine streaming, Voxtral two-cap recycle, SenseVoice
/// VAD segmentation, and the Dual draft/refine coordinator — behind the typed
/// `EngineEvent` / `HostCommand` JSON boundary. The transformers.js worker /
/// sherpa-onnx harness are the executors; the policy is Rust law. The JS glue
/// (`whisper-engine.js`, `voxtral-engine.js`, `sensevoice-engine.js`,
/// `dual-engine.js`) drives these surfaces, REPLACING the inline index.html
/// engine loops (the strangler-fig).
#[cfg(target_arch = "wasm32")]
pub mod engine;

/// Wasm-bindgen Claude-bridge reconnect surface (Phase 4, Task x4). Wasm32 only.
/// Wraps `silent_core::bridge::ReconnectPolicy` — the deterministic
/// reconnect/backoff + status state machine. The JS executor (`bridge-engine.js`
/// driving the inline `ClaudeBridge`) keeps the WebSocket; only the policy moved
/// (Appendix A row 28).
#[cfg(target_arch = "wasm32")]
pub mod bridge;

/// Wasm-bindgen extension-host surface (Phase 6, Task J2; R7). Wasm32 only.
/// Wraps the j1 `silent-extension-sdk`: install-time manifest validation
/// (verbatim `ManifestError`s to the consent UI), grant-set persistence via
/// `silent-storage` (the `extensionGrants` store, schema v4), the per-extension
/// data/UI/network boundary checks (ungranted data silently omitted), and the
/// versioned `postMessage` envelope (a version-mismatched inbound envelope is
/// refused). The JS glue (`extension-host.js`) runs each extension in a
/// sandboxed iframe and drives this object.
#[cfg(target_arch = "wasm32")]
pub mod extension_host;

#[cfg(target_arch = "wasm32")]
pub use diarization::WasmDiarization;

#[cfg(target_arch = "wasm32")]
pub use nemotron::WasmNemotron;

#[cfg(target_arch = "wasm32")]
pub use notes::{WasmCorrections, WasmNoteEngine, WasmQuestionScheduler};

#[cfg(target_arch = "wasm32")]
pub use session::WasmSession;

#[cfg(target_arch = "wasm32")]
pub use bridge::WasmBridgeReconnect;

#[cfg(target_arch = "wasm32")]
pub use engine::{WasmDual, WasmSenseVoice, WasmVoxtralRecycle, WasmWhisperStream};

#[cfg(target_arch = "wasm32")]
pub use diag::WasmDiag;
