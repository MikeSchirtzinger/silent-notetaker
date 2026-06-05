//! Engine host adapters and selection policy.
//!
//! Implements the concrete engine variants behind the [`AnyAsrEngine`]
//! enum-dispatch (the local mirror of [`silent_core::engine::AnyAsrEngine`],
//! whose dyn-incompatible async trait means dispatch is by `match`, not
//! `dyn`): the `js-transformers` host (Voxtral, Whisper, Moonshine, Qwen)
//! driven over a typed command protocol, and the `js-sherpa` host (SenseVoice).
//! Policy — chunk sizes, Voxtral's two-cap recycle, when to feed/recycle, the
//! Dual draft/refine coordination — stays here in Rust; the JS workers only
//! execute (PRD R2). Registry-driven selection + device tiers live in
//! [`selection`] (Task I3).
//!
//! # What's implemented
//!
//! ## Task I1 — Voxtral two-cap recycle
//!
//! [`voxtral_recycle`] — Voxtral's token/audio **two-cap context recycle**, the
//! hardest-won bug fix in the app (PRD Appendix A row 10; the JS source is
//! `index.html` `_runVoxtralTranscription`). A deterministic, natively
//! unit-tested Rust policy emitting typed `HostCommand`s; the transformers.js
//! worker (later wiring, in `silent-web`) is the executor and holds **no policy**.
//!
//! ## Task I2 — Whisper / Moonshine / SenseVoice / Dual
//!
//! - [`whisper_stream`] — the Whisper-family + Moonshine streaming loop
//!   (chunking, VAD gate, hallucination filter, tail-dedup). Final-only.
//!   Source: `index.html`'s inlined transcription worker + `startMoonshine`.
//! - [`sensevoice`] — SenseVoice's solo VAD segmentation policy (the 30 s window
//!   cap, circular-buffer windowing, the > 0.3 s decode gate). Source:
//!   `index.html`'s `SenseVoiceEngine`. Appendix A row 11.
//! - [`dual`] — Dual mode's draft/refine coordination: Moonshine instant drafts
//!   interleaved with SenseVoice's refined finals, with the supersede-drafts rule
//!   (keep at most one as a preview). Source: `index.html` ~lines 2797-3014 +
//!   the dual draft handling. Appendix A row 11; the interleaving is golden-first
//!   from a DOM-free JS reference generator.
//!
//! ## Task I3 — registry-driven selection + device tiers ([`selection`])
//!
//! The model picker becomes registry-driven (Appendix A rows 7, 8): the embedded
//! `registry/models.toml` is the single source of truth (R4), the
//! [`selection::DeviceProbe`] → [`selection::DeviceTier`] resolver generalizes the
//! Qwen tier mechanism (R3), [`selection::asr_picker_options`] builds the picker
//! list from registry `ui` entries (so adding a model is zero code), engines
//! carry [`selection::Availability`] verdicts with reasons + a CPU-tier
//! recommendation (R1), and [`selection::apply_selection`] queues a mid-recording
//! switch with a friendly "takes effect next meeting" notice (R3 decision log).
//!
//! ## Engine dispatch — [`engine::AnyAsrEngine`] / [`engine::JsHostEngine`]
//!
//! The i1 hand-off TODO: the concrete [`engine::AnyAsrEngine::JsHost`] variant
//! and the [`engine::JsHostEngine`] adapter that drives a transformers.js worker
//! under any of the above policies. It lives here (not `silent-core`) because the
//! variants pull in host-specific shapes that must not contaminate the
//! dependency-free contracts crate (PRD "Core contracts": *"concrete variants …
//! are added in their home crates"*).
//!
//! # Shared text-event type
//!
//! [`TextEvent`] (partial / final transcript deltas) is shared by every policy in
//! this crate — the in-place Voxtral/Nemotron-style partials and the Whisper/
//! Moonshine/SenseVoice finals all speak it. It is the UI-facing transcript
//! boundary; rendering stays in JS, the segmentation policy is Rust.
#![forbid(unsafe_code)]

pub mod dual;
pub mod engine;
pub mod selection;
pub mod sensevoice;
pub mod voxtral_recycle;
pub mod whisper_stream;

use serde::{Deserialize, Serialize};

/// A transcript text event produced by an engine policy.
///
/// Maps to the JS callbacks: [`Partial`](TextEvent::Partial) is `onPartial`
/// (the live element updated in place, e.g. Voxtral/Nemotron streaming),
/// [`Final`](TextEvent::Final) is `onFinal` (promoted, sentence/segment complete,
/// e.g. Whisper/Moonshine/SenseVoice). The UI renders these (rendering stays in
/// JS); the *segmentation policy* — what counts as a sentence/segment, what the
/// in-place buffer is — is Rust.
///
/// `#[non_exhaustive]` so an additive event kind (e.g. an explicit draft variant,
/// were drafts not already modeled by the Dual coordinator's re-labeling) is not
/// a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TextEvent {
    /// The current in-place sentence buffer (JS `onPartial(sentenceBuffer)`). The
    /// UI overwrites the live element's text with this each time.
    Partial(String),
    /// A completed sentence/segment (JS `onFinal(...)`), promoted out of the live
    /// element. Will not be revised.
    Final(String),
}
