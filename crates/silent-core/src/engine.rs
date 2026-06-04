//! Engine trait contracts and the named dispatch strategy.
//!
//! Per the PRD "Core contracts":
//!
//! - The v1 synchronous trait was unimplementable: `ort-web` is async-only and
//!   the browser main thread cannot block. The contract is therefore
//!   **async-first** and event-shaped.
//! - `async fn` in traits is **not** dyn-safe, so engine selection uses **enum
//!   dispatch** ([`AnyAsrEngine`]) rather than `Box<dyn AsrEngine>`. The
//!   dispatch strategy is named here so implementers do not invent five
//!   different ones.
//! - Engines share one [`AsrError`] (see [`crate::error`]).
//! - The JS host adapter (`JsHostEngine`, lands in `silent-inference`)
//!   implements these same traits by driving a transformers.js worker over the
//!   versioned command protocol; **policy stays in Rust**, the worker only
//!   executes.

use crate::error::AsrError;
use crate::events::{AsrCapabilities, EngineEvent, EngineStats};
use crate::ids::ModelId;

/// A chunk of 16 kHz mono PCM audio handed to an engine.
///
/// The samples are owned `f32` in `[-1.0, 1.0]`. `start_ms` lets the engine
/// stamp emitted [`EngineEvent`]s with absolute session time. Audio never
/// leaves the device (PRD R5); this type exists only inside the Rust core and
/// the local host worker.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    /// 16 kHz mono samples in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Milliseconds from session start at which this chunk begins.
    pub start_ms: u64,
}

impl AudioChunk {
    /// Construct an [`AudioChunk`].
    #[must_use]
    pub fn new(samples: Vec<f32>, start_ms: u64) -> Self {
        Self { samples, start_ms }
    }

    /// Duration of this chunk in milliseconds, assuming 16 kHz.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        (self.samples.len() as u64 * 1000) / 16_000
    }
}

/// A synchronous sink the engine pushes lifecycle events into.
///
/// Deliberately **synchronous** and therefore dyn-safe (`&mut dyn EventSink`):
/// the engine emits events as it produces them (mirroring the sync
/// `onStatus` / `onText` callbacks in `nemotron-engine.js`); the *consumer*
/// drains and dispatches them to the UI asynchronously. Keeping the sink sync
/// is what lets the async engine methods take `&mut dyn EventSink` without the
/// (unstable, un-dyn-safe) async-in-dyn machinery.
pub trait EventSink {
    /// Emit one event. Implementations must not block.
    fn send(&mut self, event: EngineEvent);
}

/// A streaming ASR engine.
///
/// Async-first and event-shaped (PRD "Core contracts"). Not object-safe by
/// design — select between engines with [`AnyAsrEngine`], not `dyn AsrEngine`.
///
/// Lifecycle: [`load`](AsrEngine::load) → [`warm_up`](AsrEngine::warm_up) →
/// repeated [`feed`](AsrEngine::feed) → [`finalize`](AsrEngine::finalize), with
/// [`reset`](AsrEngine::reset) between utterances/sessions.
pub trait AsrEngine {
    /// The registry id of the model this engine runs.
    fn id(&self) -> ModelId;

    /// What the engine can do (streaming, drafts, WebGPU requirement, sample
    /// rate). The selection policy reads this.
    fn capabilities(&self) -> AsrCapabilities;

    /// Fetch artifacts (emitting [`EngineEvent::LoadProgress`] per file) and
    /// build the runtime session. Emits [`EngineEvent::Ready`] when done.
    fn load(
        &mut self,
        events: &mut dyn EventSink,
    ) -> impl std::future::Future<Output = Result<(), AsrError>>;

    /// Pay one-time JIT / arena-growth costs up front so the user's first spoken
    /// words are not garbled (the `nemotron-engine.js` warm-up trick).
    fn warm_up(&mut self) -> impl std::future::Future<Output = Result<(), AsrError>>;

    /// Feed one [`AudioChunk`]; returns the events produced by decoding it
    /// (drafts, partials, finals, stats).
    fn feed(
        &mut self,
        chunk: AudioChunk,
    ) -> impl std::future::Future<Output = Result<Vec<EngineEvent>, AsrError>>;

    /// Drain buffered audio and decode the trailing partial chunk. Called once
    /// at end of stream.
    fn finalize(&mut self)
    -> impl std::future::Future<Output = Result<Vec<EngineEvent>, AsrError>>;

    /// Clear all streaming state for a fresh utterance/session.
    fn reset(&mut self);

    /// Current telemetry snapshot.
    fn stats(&self) -> EngineStats;
}

/// The named enum-dispatch strategy for engine selection.
///
/// `async fn` in traits is not dyn-safe, so the orchestrator holds an
/// `AnyAsrEngine` and `match`es on the active engine rather than a
/// `Box<dyn AsrEngine>`. The concrete variants (each wrapping a real engine that
/// pulls in browser / `ort` dependencies that must **not** live in
/// `silent-core`) are added in their home crates:
///
/// ```text
/// // (lands in silent-inference / nemotron-asr — Phase D/I)
/// #[non_exhaustive]
/// pub enum AnyAsrEngine {
///     Nemotron(nemotron::NemotronEngine),  // rust-ort-web host
///     JsHost(jshost::JsHostEngine),        // transformers.js host (Voxtral, Whisper, Moonshine)
///     Sherpa(sherpa::SherpaEngine),        // sherpa-onnx host (SenseVoice)
/// }
/// ```
///
/// Each method forwards to the active variant:
///
/// ```text
/// fn id(&self) -> ModelId {
///     match self {
///         AnyAsrEngine::Nemotron(e) => e.id(),
///         AnyAsrEngine::JsHost(e)   => e.id(),
///         AnyAsrEngine::Sherpa(e)   => e.id(),
///     }
/// }
/// ```
///
/// This skeleton lives in `silent-core` to **name** the strategy (so no one
/// invents an ad-hoc one) while keeping the crate dependency-free. It is
/// `#[non_exhaustive]` and currently carries only [`AnyAsrEngine::Unset`]; real
/// variants are added without a breaking change.
#[non_exhaustive]
#[derive(Debug, Default)]
pub enum AnyAsrEngine {
    /// No engine selected yet. The orchestrator starts here before the user
    /// picks an ASR model; concrete variants replace it at recording start.
    #[default]
    Unset,
}

impl AnyAsrEngine {
    /// The id of the active engine, if one is selected.
    ///
    /// Returns `None` for [`AnyAsrEngine::Unset`]. When concrete variants are
    /// added this `match` forwards to each engine's `id()`; the `#[non_exhaustive]`
    /// arm keeps it forward-compatible.
    #[must_use]
    pub fn id(&self) -> Option<ModelId> {
        match self {
            AnyAsrEngine::Unset => None,
        }
    }

    /// Whether an engine is selected.
    #[must_use]
    pub fn is_set(&self) -> bool {
        !matches!(self, AnyAsrEngine::Unset)
    }
}

/// The kind of generation requested from a [`NotesEngine`] or
/// [`QuestionGenerator`], so one trait shape serves the live / stop-time / final
/// passes (PRD "Core contracts").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GenerationKind {
    /// A live smart question shown in the teleprompter mid-meeting.
    LiveQuestion,
    /// A stop-time recap question or summary.
    StopTimeRecap,
    /// The final notes pass at Stop (Qwen chunking / dedup / TAG parsing).
    FinalNotes,
}

/// A model that turns transcript text into notes (PRD: zero or one; "no notes
/// model" is a first-class supported state — transcript-only mode).
///
/// The orchestrator handles `None` in the notes slot directly; this trait is
/// only implemented when a notes model is selected. Same `load`/`generate`
/// shape as [`QuestionGenerator`].
pub trait NotesEngine {
    /// The registry id of the notes model.
    fn id(&self) -> ModelId;

    /// Load the notes model (host-executed; policy stays in `silent-notes`).
    fn load(&mut self) -> impl std::future::Future<Output = Result<(), AsrError>>;

    /// Generate notes of the given [`GenerationKind`] from transcript text.
    fn generate(
        &mut self,
        kind: GenerationKind,
        transcript: &str,
    ) -> impl std::future::Future<Output = Result<String, AsrError>>;
}

/// A model that generates smart questions (clarify / risk / follow-up) for the
/// teleprompter. Same shape as [`NotesEngine`]; scheduling and type rotation are
/// Rust policy in `silent-notes`, the model only executes.
pub trait QuestionGenerator {
    /// The registry id of the question model.
    fn id(&self) -> ModelId;

    /// Load the question model.
    fn load(&mut self) -> impl std::future::Future<Output = Result<(), AsrError>>;

    /// Generate a question of the given [`GenerationKind`] from transcript text.
    fn generate(
        &mut self,
        kind: GenerationKind,
        transcript: &str,
    ) -> impl std::future::Future<Output = Result<String, AsrError>>;
}
